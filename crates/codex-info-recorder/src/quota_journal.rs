//! Durable acquisition input. Quota interpretation and history projection do
//! not own these records and cannot remove an unrecognized response.

use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

// Receipt IDs are allocated once by the acquisition owner. This tracks only
// that owner's lifetime, never quota validity or durable application status.
static ACTIVE_RECEIPTS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

pub(super) struct QuotaReceipt {
    id: String,
    pub observed_at: i64,
    account_scope: Option<String>,
    plan_type: String,
    payload: String,
}

#[derive(Default)]
pub(super) struct ReplayBatch {
    pub receipts: Vec<codex_info_db_writer::QuotaReceipt>,
    pub unresolved: usize,
    /// Failure of the latest settled input, not the historical backlog size.
    pub latest_input_unresolved: Option<bool>,
}

pub(super) fn read_unapplied(
    data_root: &Path,
    account_scope: &str,
    applied: &BTreeSet<String>,
) -> Result<ReplayBatch, String> {
    let path = data_root.join("history/quota/observations.sqlite3");
    match fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ReplayBatch::default())
        }
        Err(_) => return Err("quota acquisition journal could not be read".into()),
        Ok(_) => {}
    }
    let read = || -> Result<_, Box<dyn std::error::Error>> {
        // Keep owner registration/release ordered with the SQLite snapshot:
        // a raw commit cannot appear before registration, and an owner cannot
        // disappear while an older unverified row is being classified.
        let active = ACTIVE_RECEIPTS.lock().unwrap_or_else(|e| e.into_inner());
        let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(Duration::from_secs(1))?;
        db.execute_batch("PRAGMA query_only=ON; BEGIN;")?;
        let mut statement = db.prepare(
            "SELECT receipt_id, sequence, observed_at, plan_type, payload, account_verified
             FROM quota_observations WHERE account_scope=?1
             ORDER BY sequence",
        )?;
        let mut rows = statement.query([account_scope])?;
        let mut pending = ReplayBatch::default();
        while let Some(row) = rows.next()? {
            let receipt_id: String = row.get(0)?;
            if applied.contains(&receipt_id) {
                pending.latest_input_unresolved = Some(false);
                continue;
            }
            if row.get::<_, i64>(5)? != 1 {
                if !active.contains(&receipt_id) {
                    pending.unresolved += 1;
                    pending.latest_input_unresolved = Some(true);
                }
                continue;
            }
            let plan: String = row.get(3)?;
            let payload: String = row.get(4)?;
            let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                pending.unresolved += 1;
                pending.latest_input_unresolved = Some(true);
                continue;
            };
            let Ok(snapshot) = super::parse_app_server_quota(&value, &plan) else {
                pending.unresolved += 1;
                pending.latest_input_unresolved = Some(true);
                // This record remains unapplied and is retried by a later
                // parser. It cannot prevent subsequent valid inputs advancing.
                continue;
            };
            let Some(remaining_percent) = snapshot.remaining_percent else {
                pending.unresolved += 1;
                pending.latest_input_unresolved = Some(true);
                continue;
            };
            pending.latest_input_unresolved = Some(false);
            pending.receipts.push(codex_info_db_writer::QuotaReceipt {
                receipt_id,
                sequence: row.get(1)?,
                observed_at: row.get(2)?,
                reset_at: snapshot.reset_at,
                window_seconds: snapshot.window_seconds,
                remaining_percent,
            });
        }
        Ok(pending)
    };
    read().map_err(|_| "quota acquisition journal could not be read".into())
}

impl QuotaReceipt {
    pub fn new(
        observed_at: i64,
        account_scope: Option<String>,
        plan_type: &str,
        payload: &Value,
    ) -> Self {
        // Generate identity before acquisition (at the caller). A retry keeps
        // exactly this receipt, including its original timestamp and scope.
        Self {
            id: String::new(),
            observed_at,
            account_scope,
            plan_type: plan_type.to_owned(),
            payload: payload.to_string(),
        }
    }

    pub fn with_id(mut self, id: String) -> Self {
        let mut active = ACTIVE_RECEIPTS.lock().unwrap_or_else(|e| e.into_inner());
        active.remove(&self.id);
        active.insert(id.clone());
        self.id = id;
        self
    }

    pub fn save(&self, data_root: &Path, verified: bool) -> Result<(), String> {
        self.save_inner(data_root, verified)
            .map_err(|_| "quota acquisition journal could not be committed".to_owned())
    }

    fn save_inner(
        &self,
        data_root: &Path,
        verified: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = journal_path(data_root)?;
        let mut db = Connection::open(&path)?;
        db.busy_timeout(Duration::from_secs(1))?;
        db.execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS quota_observations (
               sequence INTEGER PRIMARY KEY AUTOINCREMENT,
               receipt_id TEXT NOT NULL UNIQUE,
               observed_at INTEGER NOT NULL,
               account_scope TEXT,
               plan_type TEXT NOT NULL,
               payload TEXT NOT NULL,
               account_verified INTEGER NOT NULL DEFAULT 0
                 CHECK(account_verified IN (0,1))
             );",
        )?;
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO quota_observations
             (receipt_id, observed_at, account_scope, plan_type, payload, account_verified)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(receipt_id) DO UPDATE SET
               account_verified = MAX(account_verified, excluded.account_verified)",
            params![
                self.id,
                self.observed_at,
                self.account_scope,
                self.plan_type,
                self.payload,
                verified && self.account_scope.is_some(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
}

impl Drop for QuotaReceipt {
    fn drop(&mut self) {
        // Success, Err and unwind all release ownership. On restart no owner
        // survives, so abandoned unverified inputs remain visible failures.
        ACTIVE_RECEIPTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

fn journal_path(data_root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    codex_info_account_locator::prepare_recorder_data_root(data_root)?;
    let directory = data_root.join("history/quota");
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(&directory) {
        Ok(()) => {
            File::open(data_root.join("history"))?.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("quota journal directory is not a directory".into());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err("quota journal directory is not private".into());
    }
    let path = directory.join("observations.sqlite3");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    let file = options.open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err("quota journal is not a regular file".into());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err("quota journal is not private".into());
    }
    File::open(&directory)?.sync_all()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn receipt(id: &str, payload: Value) -> QuotaReceipt {
        QuotaReceipt::new(1791763418, Some("account".into()), "pro", &payload).with_id(id.into())
    }

    fn valid_payload() -> Value {
        json!({"rateLimits": {"primary": {
            "usedPercent": 56, "resetsAt": 1800000000, "windowDurationMins": 10080
        }}})
    }

    fn raw_count(root: &Path) -> i64 {
        Connection::open(root.join("history/quota/observations.sqlite3"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM quota_observations", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn active_verification_is_pending_then_replayable_without_losing_raw_input() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = receipt("active-verification", valid_payload());
        receipt.save(temp.path(), false).unwrap();
        let pending = read_unapplied(temp.path(), "account", &BTreeSet::new()).unwrap();
        assert!(pending.receipts.is_empty());
        assert_eq!(
            pending.unresolved, 0,
            "a live verification is not an acquisition failure"
        );
        assert_eq!(raw_count(temp.path()), 1);

        receipt.save(temp.path(), true).unwrap();
        let verified = read_unapplied(temp.path(), "account", &BTreeSet::new()).unwrap();
        assert_eq!(verified.unresolved, 0);
        assert_eq!(verified.receipts.len(), 1);
        assert_eq!(verified.receipts[0].remaining_percent, 44.0);
        assert_eq!(verified.receipts[0].observed_at, 1791763418);
        let applied = BTreeSet::from(["active-verification".to_owned()]);
        assert!(read_unapplied(temp.path(), "account", &applied)
            .unwrap()
            .receipts
            .is_empty());
        assert_eq!(raw_count(temp.path()), 1);
    }

    #[test]
    fn abandoned_verification_remains_unresolved_and_does_not_block_valid_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let abandoned = receipt("abandoned-verification", valid_payload());
        abandoned.save(temp.path(), false).unwrap();
        drop(abandoned);
        let valid = receipt("next-valid", valid_payload());
        valid.save(temp.path(), true).unwrap();
        let batch = read_unapplied(temp.path(), "account", &BTreeSet::new()).unwrap();
        assert_eq!(batch.unresolved, 1);
        assert_eq!(batch.receipts.len(), 1);
        assert_eq!(batch.receipts[0].receipt_id, "next-valid");
        assert_eq!(raw_count(temp.path()), 2);
    }

    #[test]
    fn verified_unrecognized_input_is_retained_and_reported_even_while_owned() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = receipt("unrecognized-input", json!({"unknown": true}));
        receipt.save(temp.path(), true).unwrap();
        let batch = read_unapplied(temp.path(), "account", &BTreeSet::new()).unwrap();
        assert_eq!(batch.unresolved, 1);
        assert!(batch.receipts.is_empty());
        assert_eq!(raw_count(temp.path()), 1);
    }
}

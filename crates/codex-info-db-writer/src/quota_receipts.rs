//! Accepted quota input is committed independently of Session/model work.
//! The receipt itself is the application marker; it cannot be acknowledged
//! without the accepted value and publication generation in the same commit.

use super::{
    canonical_u64_text, observation_data_hash, observation_json_with_source_timestamp,
    sqlite_exact_float_json, Result, UsageHistoryObservation, UsageStore, UsageStoreError,
    MAX_PUBLIC_UNIX_SECONDS,
};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS quota_receipts (
    receipt_id TEXT PRIMARY KEY NOT NULL,
    sequence INTEGER NOT NULL UNIQUE CHECK(sequence > 0),
    observed_at INTEGER NOT NULL CHECK(observed_at > 0),
    reset_at INTEGER NOT NULL CHECK(reset_at > 0),
    window_seconds INTEGER NOT NULL CHECK(window_seconds > 0),
    remaining_percent REAL NOT NULL CHECK(remaining_percent >= 0 AND remaining_percent <= 100),
    applied_generation TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS quota_receipts_period_observed_idx
    ON quota_receipts(reset_at, observed_at, sequence);
CREATE TABLE IF NOT EXISTS quota_acquisition_state (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    degraded INTEGER NOT NULL DEFAULT 0 CHECK(degraded IN (0,1)),
    input_degraded INTEGER NOT NULL DEFAULT 0 CHECK(input_degraded IN (0,1))
);";

pub(super) fn ensure_health_schema(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let present: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='quota_acquisition_state')",
        [],
        |row| row.get(0),
    )?;
    if present {
        let input_present: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('quota_acquisition_state') WHERE name='input_degraded')",
            [],
            |row| row.get(0),
        )?;
        if !input_present {
            // A legacy acquisition failure has no reliable source identity.
            // Preserve it until the acquisition owner confirms recovery.
            tx.execute_batch("ALTER TABLE quota_acquisition_state ADD COLUMN input_degraded INTEGER NOT NULL DEFAULT 0 CHECK(input_degraded IN(0,1));")?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaReceipt {
    pub receipt_id: String,
    pub sequence: i64,
    pub observed_at: i64,
    pub reset_at: i64,
    pub window_seconds: i64,
    pub remaining_percent: f64,
}

pub(super) fn remaining_for_exact_minute(
    connection: &rusqlite::Connection,
    reset_at: i64,
    minute: i64,
) -> Result<Option<f64>> {
    // Unpartitioned legacy stores have no accepted official receipt ledger.
    let has_receipts: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='quota_receipts')",
        [],
        |row| row.get(0),
    )?;
    if !has_receipts {
        return Ok(None);
    }
    Ok(connection
        .query_row(
            "SELECT remaining_percent FROM quota_receipts
         WHERE reset_at=?1 AND observed_at>=?2 AND observed_at<?3
         ORDER BY sequence DESC LIMIT 1",
            params![reset_at, minute, minute + 60],
            |row| row.get(0),
        )
        .optional()?)
}

/// Materialize only quota at one exact key. The immutable receipt remains the
/// authority; model collection, completeness and pricing do not participate.
/// Older acquisitions arriving later cannot replace a newer accepted value.
fn project_exact_minute(tx: &rusqlite::Transaction<'_>, reset_at: i64, minute: i64) -> Result<()> {
    let Some(remaining) = remaining_for_exact_minute(tx, reset_at, minute)? else {
        return Ok(());
    };
    tx.execute(
        "UPDATE usage_history SET remaining_percent=?3
         WHERE reset_at=?1 AND timestamp=?2 AND remaining_percent IS NOT ?3",
        params![reset_at, minute, remaining],
    )?;
    let hash = observation_data_hash(reset_at, minute);
    let observation_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM durable_state WHERE singleton>=2 AND data_hash=?1)",
        [&hash],
        |row| row.get(0),
    )?;
    if observation_exists {
        // json_set retains the other numeric literals and metadata. Rebuilding
        // an observation from a quota-only vector would discard model evidence.
        let encoded = sqlite_exact_float_json(tx, remaining)?;
        tx.execute(
            "UPDATE durable_state SET snapshot_json=json_set(snapshot_json,'$.remaining_percent',json(?2))
             WHERE singleton>=2 AND data_hash=?1
               AND json_extract(snapshot_json,'$.remaining_percent') IS NOT ?3",
            params![hash, encoded, remaining],
        )?;
    } else {
        let model_observation = tx
            .query_row(
                "SELECT sol_dollars,terra_dollars,luna_dollars,sol_tokens,terra_tokens,luna_tokens
             FROM usage_history WHERE reset_at=?1 AND timestamp=?2",
                params![reset_at, minute],
                |row| {
                    Ok(UsageHistoryObservation {
                        timestamp: minute,
                        reset_at,
                        remaining_percent: Some(remaining),
                        sol_dollars: row.get(0)?,
                        terra_dollars: row.get(1)?,
                        luna_dollars: row.get(2)?,
                        sol_tokens: row.get(3)?,
                        terra_tokens: row.get(4)?,
                        luna_tokens: row.get(5)?,
                        model_source: super::ModelSource::LegacyUnknown,
                        model_totals: None,
                        model_totals_complete: false,
                    })
                },
            )
            .optional()?;
        let observation = model_observation.unwrap_or_else(|| {
            UsageHistoryObservation::unavailable(minute, reset_at, Some(remaining))
        });
        let json = observation_json_with_source_timestamp(tx, &observation, None)?;
        tx.execute(
            "INSERT INTO durable_state(singleton,data_generation,data_hash,snapshot_json)
             SELECT COALESCE(MAX(singleton),1)+1,?1,?2,?3 FROM durable_state",
            params![minute, hash, json],
        )?;
    }
    Ok(())
}

fn stored_receipt(connection: &rusqlite::Connection, id: &str) -> Result<Option<QuotaReceipt>> {
    Ok(connection
        .query_row(
            "SELECT receipt_id,sequence,observed_at,reset_at,window_seconds,remaining_percent
         FROM quota_receipts WHERE receipt_id=?1",
            [id],
            |row| {
                Ok(QuotaReceipt {
                    receipt_id: row.get(0)?,
                    sequence: row.get(1)?,
                    observed_at: row.get(2)?,
                    reset_at: row.get(3)?,
                    window_seconds: row.get(4)?,
                    remaining_percent: row.get(5)?,
                })
            },
        )
        .optional()?)
}

fn projection_matches(connection: &rusqlite::Connection, receipt: &QuotaReceipt) -> Result<bool> {
    let minute = receipt.observed_at.div_euclid(60) * 60;
    let history: Option<Option<f64>> = connection
        .query_row(
            "SELECT remaining_percent FROM usage_history WHERE reset_at=?1 AND timestamp=?2",
            params![receipt.reset_at, minute],
            |row| row.get(0),
        )
        .optional()?;
    if history.is_some_and(|value| value != Some(receipt.remaining_percent)) {
        return Ok(false);
    }
    let observation: Option<(Option<f64>, String)> = connection.query_row(
        "SELECT json_extract(snapshot_json,'$.remaining_percent'),json_extract(snapshot_json,'$.model_source')
         FROM durable_state WHERE singleton>=2 AND data_hash=?1",
        [observation_data_hash(receipt.reset_at, minute)], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    Ok(observation.is_some_and(|(remaining, source)| {
        remaining == Some(receipt.remaining_percent)
            && (source == "unavailable") == history.is_none()
    }))
}

impl UsageStore {
    /// Offline repair for a caller that has qualified the immutable official
    /// journal and owns a separate, non-rotating backup. This entrypoint must
    /// not open the database through a schema-migrating writer constructor.
    pub fn repair_quota_history_after_backup(
        database: &std::path::Path,
        backup: &std::path::Path,
        identity: &super::StoragePartitionIdentity,
        intervals: &[super::SessionLifecycleInterval],
        receipts: &[QuotaReceipt],
    ) -> Result<usize> {
        let invalid = |message: &str| UsageStoreError::InvalidImport(message.into());
        if receipts.is_empty() || receipts.len() > 10_000 || intervals.is_empty() {
            return Err(invalid(
                "quota repair requires a bounded nonempty receipt and lifecycle set",
            ));
        }
        let intervals = super::canonical_lifecycle_intervals(intervals)?;
        super::validate_partition_file(database)?;
        super::validate_partition_file(backup)?;
        if std::fs::canonicalize(database)? == std::fs::canonicalize(backup)? {
            return Err(invalid("quota repair backup must be a separate file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let live = std::fs::metadata(database)?;
            let saved = std::fs::metadata(backup)?;
            if (live.dev(), live.ino()) == (saved.dev(), saved.ino()) {
                return Err(invalid(
                    "quota repair backup must not alias the live database",
                ));
            }
        }
        let saved = Self::open_read_only_partitioned(backup, identity)?;
        saved
            .connection
            .execute_batch("PRAGMA query_only=ON; BEGIN")?;
        saved.verify_integrity()?;
        let saved_history = super::legacy_raw_evidence(&saved.connection)?;
        let saved_version = super::account_db_schema_version(&saved.connection)?;
        let mut connection = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::validate_storage_partition_metadata(&tx, identity)?;
        if super::account_db_schema_version(&tx)? != saved_version {
            return Err(invalid(
                "quota repair cannot migrate or cross schema versions",
            ));
        }
        let mut keys = std::collections::BTreeSet::new();
        let mut pending = Vec::new();
        for receipt in receipts {
            if !intervals
                .iter()
                .any(|interval| interval.contains(receipt.observed_at))
            {
                return Err(invalid(
                    "quota repair receipt is outside the account lifecycle",
                ));
            }
            if stored_receipt(&tx, &receipt.receipt_id)?.as_ref() != Some(receipt)
                || stored_receipt(&saved.connection, &receipt.receipt_id)?.as_ref() != Some(receipt)
            {
                return Err(invalid(
                    "quota repair receipt differs from the accepted backup or live receipt",
                ));
            }
            let minute = receipt.observed_at.div_euclid(60) * 60;
            if !keys.insert((receipt.reset_at, minute)) {
                return Err(invalid("quota repair contains a duplicate exact minute"));
            }
            let latest: String = tx.query_row(
                "SELECT receipt_id FROM quota_receipts WHERE reset_at=?1 AND observed_at>=?2 AND observed_at<?3
                 ORDER BY sequence DESC LIMIT 1", params![receipt.reset_at, minute, minute + 60], |row| row.get(0),
            )?;
            if latest != receipt.receipt_id {
                return Err(invalid(
                    "quota repair requires the latest official acquisition at each exact minute",
                ));
            }
            if !projection_matches(&tx, receipt)? {
                pending.push(receipt);
            }
        }
        if pending.is_empty() {
            tx.commit()?;
            return Ok(0);
        }
        if super::legacy_raw_evidence(&tx)? != saved_history {
            return Err(invalid(
                "quota repair history changed after its preservation backup",
            ));
        }
        for receipt in &pending {
            project_exact_minute(
                &tx,
                receipt.reset_at,
                receipt.observed_at.div_euclid(60) * 60,
            )?;
            if !projection_matches(&tx, receipt)? {
                return Err(invalid(
                    "quota repair did not preserve an exact durable quota projection",
                ));
            }
        }
        let generation: String = tx.query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let next = canonical_u64_text(&generation, "collection generation")?
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        tx.execute(
            "UPDATE collection_generation SET data_generation=?1 WHERE singleton=1",
            [next.to_string()],
        )?;
        let changed = pending.len();
        tx.commit()?;
        Ok(changed)
    }

    /// Quota health must not depend on an active-thread snapshot existing.
    pub fn set_quota_acquisition_degraded(&mut self, degraded: bool) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<bool> = tx
            .query_row(
                "SELECT degraded FROM quota_acquisition_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if previous.unwrap_or(false) == degraded {
            return Ok(());
        }
        let generation: String = tx.query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let next = canonical_u64_text(&generation, "collection generation")?
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        tx.execute(
            "INSERT INTO quota_acquisition_state(singleton,degraded) VALUES(1,?1)
             ON CONFLICT(singleton) DO UPDATE SET degraded=excluded.degraded",
            [degraded],
        )?;
        tx.execute(
            "UPDATE collection_generation SET data_generation=?1 WHERE singleton=1",
            [next.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Current quota follows acquisition order, independently of Session
    /// checkpoints and the wall clock recorded by a prior observation.
    pub fn latest_quota_receipt(&self) -> Result<Option<QuotaReceipt>> {
        Ok(self.connection.query_row(
            "SELECT receipt_id, sequence, observed_at, reset_at, window_seconds, remaining_percent
             FROM quota_receipts ORDER BY sequence DESC LIMIT 1",
            [],
            |row| Ok(QuotaReceipt {
                receipt_id: row.get(0)?,
                sequence: row.get(1)?,
                observed_at: row.get(2)?,
                reset_at: row.get(3)?,
                window_seconds: row.get(4)?,
                remaining_percent: row.get(5)?,
            }),
        ).optional()?)
    }

    pub fn applied_quota_receipt_ids(&self) -> Result<std::collections::BTreeSet<String>> {
        let mut statement = self
            .connection
            .prepare("SELECT receipt_id FROM quota_receipts")?;
        let rows = statement.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Idempotently accepts source receipts, without parsing Session files or
    /// validating their model totals. The same transaction updates quota at
    /// each exact history minute; it neither waits for Session collection nor
    /// rewrites the independently saved model evidence.
    pub fn commit_quota_receipts(
        &mut self,
        receipts: &[QuotaReceipt],
        input_degraded: impl Into<Option<bool>>,
    ) -> Result<Option<u64>> {
        let input_degraded = input_degraded.into();
        if receipts.is_empty() && input_degraded.is_none() {
            return Ok(None);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let generation: String = tx.query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let next = canonical_u64_text(&generation, "collection generation")?
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        let mut inserted = 0;
        let mut minutes = std::collections::BTreeSet::new();
        for receipt in receipts {
            if receipt.receipt_id.len() != 64
                || !receipt
                    .receipt_id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || receipt.sequence <= 0
                || !(1..=MAX_PUBLIC_UNIX_SECONDS).contains(&receipt.observed_at)
                || !(1..=MAX_PUBLIC_UNIX_SECONDS).contains(&receipt.reset_at)
                || receipt.window_seconds <= 0
                || !receipt.remaining_percent.is_finite()
                || !(0.0..=100.0).contains(&receipt.remaining_percent)
            {
                return Err(UsageStoreError::InvalidImport(
                    "quota receipt is invalid".into(),
                ));
            }
            let changed = tx.execute(
                "INSERT INTO quota_receipts
                 (receipt_id,sequence,observed_at,reset_at,window_seconds,remaining_percent,applied_generation)
                 VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(receipt_id) DO NOTHING",
                params![receipt.receipt_id,receipt.sequence,receipt.observed_at,receipt.reset_at,
                    receipt.window_seconds,receipt.remaining_percent,next.to_string()],
            )?;
            inserted += changed;
            if changed > 0 {
                minutes.insert((receipt.reset_at, receipt.observed_at.div_euclid(60) * 60));
            }
        }
        for (reset_at, minute) in minutes {
            project_exact_minute(&tx, reset_at, minute)?;
        }
        // Interpretation owns only the latest settled input. It may recover
        // after restart without a new poll, while a real acquisition/storage
        // failure remains independently degraded.
        let health_changed = if let Some(degraded) = input_degraded {
            tx.execute(
                "INSERT INTO quota_acquisition_state(singleton,degraded,input_degraded) VALUES(1,0,?1)
                 ON CONFLICT(singleton) DO UPDATE SET input_degraded=excluded.input_degraded
                 WHERE input_degraded!=excluded.input_degraded",
                [degraded],
            )?
        } else {
            0
        };
        if inserted == 0 && health_changed == 0 {
            return Ok(None);
        }
        tx.execute(
            "UPDATE collection_generation SET data_generation=?1 WHERE singleton=1",
            [next.to_string()],
        )?;
        tx.commit()?;
        Ok(Some(next))
    }
}

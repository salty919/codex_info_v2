//! Accepted quota input is committed independently of Session/model work.
//! The receipt itself is the application marker; it cannot be acknowledged
//! without the accepted value and publication generation in the same commit.

use super::{canonical_u64_text, Result, UsageStore, UsageStoreError, MAX_PUBLIC_UNIX_SECONDS};
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

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaReceipt {
    pub receipt_id: String,
    pub sequence: i64,
    pub observed_at: i64,
    pub reset_at: i64,
    pub window_seconds: i64,
    pub remaining_percent: f64,
}

impl UsageStore {
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
    /// validating their model totals. History projection consumes this input
    /// separately and must not advance or replace its acquisition order.
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
            inserted += tx.execute(
                "INSERT INTO quota_receipts
                 (receipt_id,sequence,observed_at,reset_at,window_seconds,remaining_percent,applied_generation)
                 VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(receipt_id) DO NOTHING",
                params![receipt.receipt_id,receipt.sequence,receipt.observed_at,receipt.reset_at,
                    receipt.window_seconds,receipt.remaining_percent,next.to_string()],
            )?;
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

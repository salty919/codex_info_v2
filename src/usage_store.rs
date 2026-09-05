// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

use chrono::{DateTime, Months, Utc};
use rusqlite::types::Value;
use rusqlite::{
    params, Connection, DatabaseName, OpenFlags, OptionalExtension, TransactionBehavior,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS usage_history (
    timestamp INTEGER NOT NULL CHECK (timestamp > 0),
    reset_at INTEGER NOT NULL CHECK (reset_at > 0),
    remaining_percent REAL,
    sol_dollars REAL NOT NULL,
    terra_dollars REAL NOT NULL,
    luna_dollars REAL NOT NULL,
    sol_tokens INTEGER NOT NULL DEFAULT 0,
    terra_tokens INTEGER NOT NULL DEFAULT 0,
    luna_tokens INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (reset_at, timestamp)
);
CREATE INDEX IF NOT EXISTS usage_history_timestamp_idx
    ON usage_history (timestamp);
CREATE INDEX IF NOT EXISTS usage_history_timestamp_reset_idx
    ON usage_history (
        timestamp,
        reset_at,
        remaining_percent,
        sol_dollars,
        terra_dollars,
        luna_dollars,
        sol_tokens,
        terra_tokens,
        luna_tokens
    );

CREATE TABLE IF NOT EXISTS durable_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton >= 1),
    data_generation INTEGER NOT NULL CHECK (data_generation >= 0),
    data_hash TEXT NOT NULL,
    snapshot_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS recorded_sessions (
    root_identity TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    file_bytes INTEGER NOT NULL CHECK (file_bytes >= 0),
    modified_nanos TEXT NOT NULL,
    file_device TEXT NOT NULL,
    file_inode TEXT NOT NULL,
    PRIMARY KEY (
        root_identity,
        relative_path,
        file_bytes,
        modified_nanos,
        file_device,
        file_inode
    )
) WITHOUT ROWID;
"#;

const PARTITION_SCHEMA: &str = r#"
CREATE TABLE storage_partition (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version TEXT NOT NULL,
    profile_scope_id TEXT NOT NULL,
    account_scope_id TEXT NOT NULL,
    storage_epoch TEXT NOT NULL,
    partition_id TEXT NOT NULL
);

CREATE TABLE collection_generation (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    data_generation TEXT NOT NULL,
    reset_at INTEGER NOT NULL CHECK (reset_at >= 0),
    window_seconds INTEGER NOT NULL CHECK (window_seconds >= 0),
    collector_epoch TEXT,
    cycle_seq TEXT NOT NULL,
    CHECK (
        collector_epoch IS NULL OR (
            length(collector_epoch) = 32
            AND collector_epoch NOT GLOB '*[^0-9a-f]*'
        )
    )
);
INSERT INTO collection_generation (
    singleton, data_generation, reset_at, window_seconds, collector_epoch, cycle_seq
) VALUES (1, '0', 0, 0, NULL, '0');

CREATE TABLE session_checkpoints (
    root_identity TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    file_device TEXT NOT NULL,
    file_inode TEXT NOT NULL,
    committed_offset INTEGER NOT NULL CHECK (committed_offset >= 0),
    discard_until_lf INTEGER NOT NULL CHECK (discard_until_lf IN (0, 1)),
    collector_epoch TEXT NOT NULL CHECK (
        length(collector_epoch) = 32
        AND collector_epoch NOT GLOB '*[^0-9a-f]*'
    ),
    cycle_seq TEXT NOT NULL,
    prefix_generation TEXT NOT NULL CHECK (
        length(prefix_generation) = 32
        AND prefix_generation NOT GLOB '*[^0-9a-f]*'
    ),
    prefix_sha256 TEXT NOT NULL CHECK (
        length(prefix_sha256) = 64
        AND prefix_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    fully_attributed_from_zero INTEGER NOT NULL CHECK (fully_attributed_from_zero IN (0, 1)),
    token_baseline_known INTEGER NOT NULL CHECK (token_baseline_known IN (0, 1)),
    last_model TEXT,
    previous_total TEXT NOT NULL,
    previous_input TEXT NOT NULL,
    previous_cached_input TEXT NOT NULL,
    previous_output TEXT NOT NULL,
    last_task_running INTEGER CHECK (last_task_running IS NULL OR last_task_running IN (0, 1)),
    previous_cache_write_input TEXT,
    PRIMARY KEY (
        root_identity,
        relative_path,
        file_device,
        file_inode,
        prefix_generation
    )
) WITHOUT ROWID;

CREATE TABLE session_ranges (
    root_identity TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    file_device TEXT NOT NULL,
    file_inode TEXT NOT NULL,
    start_offset INTEGER NOT NULL CHECK (start_offset >= 0),
    end_offset INTEGER NOT NULL CHECK (end_offset > start_offset),
    collector_epoch TEXT NOT NULL CHECK (
        length(collector_epoch) = 32
        AND collector_epoch NOT GLOB '*[^0-9a-f]*'
    ),
    cycle_seq TEXT NOT NULL,
    prefix_generation TEXT NOT NULL CHECK (
        length(prefix_generation) = 32
        AND prefix_generation NOT GLOB '*[^0-9a-f]*'
    ),
    record_sha256 TEXT NOT NULL CHECK (
        length(record_sha256) = 64
        AND record_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY (
        root_identity,
        relative_path,
        file_device,
        file_inode,
        prefix_generation,
        start_offset,
        end_offset,
        record_sha256
    )
) WITHOUT ROWID;

CREATE TABLE session_model_totals (
    model TEXT PRIMARY KEY,
    total_tokens TEXT NOT NULL,
    input_tokens TEXT NOT NULL,
    cached_input_tokens TEXT NOT NULL,
    output_tokens TEXT NOT NULL,
    cache_write_input_tokens TEXT
) WITHOUT ROWID;

CREATE TABLE usage_model_history (
    reset_at INTEGER NOT NULL CHECK (reset_at > 0),
    timestamp INTEGER NOT NULL CHECK (timestamp > 0),
    model TEXT NOT NULL CHECK (length(model) BETWEEN 1 AND 512),
    total_tokens TEXT NOT NULL,
    input_tokens TEXT NOT NULL,
    cached_input_tokens TEXT NOT NULL,
    output_tokens TEXT NOT NULL,
    cache_write_input_tokens TEXT,
    model_set_complete INTEGER NOT NULL CHECK (model_set_complete IN (0, 1)),
    PRIMARY KEY (reset_at, timestamp, model)
) WITHOUT ROWID;

CREATE TABLE history_continuity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    source_fingerprint TEXT NOT NULL CHECK (
        length(source_fingerprint) = 16
        AND source_fingerprint NOT GLOB '*[^0-9a-f]*'
    ),
    source_rows INTEGER NOT NULL CHECK (source_rows > 0),
    boundary_timestamp INTEGER NOT NULL CHECK (boundary_timestamp > 0),
    reset_at INTEGER NOT NULL CHECK (reset_at > 0),
    remaining_percent REAL NOT NULL CHECK (
        remaining_percent >= 0.0 AND remaining_percent <= 100.0
    ),
    sol_dollars REAL NOT NULL CHECK (sol_dollars >= 0.0),
    terra_dollars REAL NOT NULL CHECK (terra_dollars >= 0.0),
    luna_dollars REAL NOT NULL CHECK (luna_dollars >= 0.0),
    sol_tokens TEXT NOT NULL,
    terra_tokens TEXT NOT NULL,
    luna_tokens TEXT NOT NULL
);

CREATE TABLE recorder_gap_ledger (
    gap_id TEXT PRIMARY KEY CHECK (
        length(gap_id) = 32 AND gap_id NOT GLOB '*[^0-9a-f]*'
    ),
    partition_id TEXT NOT NULL CHECK (
        length(partition_id) = 64 AND partition_id NOT GLOB '*[^0-9a-f]*'
    ),
    source_identity_before TEXT NOT NULL CHECK (length(source_identity_before) BETWEEN 1 AND 512),
    source_identity_after TEXT NOT NULL CHECK (length(source_identity_after) BETWEEN 1 AND 512),
    cursor_before TEXT NOT NULL CHECK (length(cursor_before) BETWEEN 1 AND 512),
    cursor_after TEXT NOT NULL CHECK (length(cursor_after) BETWEEN 1 AND 512),
    stopped_at_monotonic_ns INTEGER NOT NULL CHECK (stopped_at_monotonic_ns > 0),
    resumed_at_monotonic_ns INTEGER CHECK (
        resumed_at_monotonic_ns IS NULL OR resumed_at_monotonic_ns >= stopped_at_monotonic_ns
    ),
    start_at INTEGER NOT NULL CHECK (start_at > 0),
    end_at INTEGER NOT NULL CHECK (end_at >= start_at),
    reset_at INTEGER CHECK (reset_at IS NULL OR reset_at > 0),
    reason TEXT NOT NULL CHECK (
        reason IN ('daemon_stop_unrecoverable', 'reset_hint_expired', 'auth_epoch_tombstoned')
    ),
    state TEXT NOT NULL CHECK (
        state IN ('pending', 'confirmed', 'recovered', 'rejected')
    ),
    owner_collector_epoch TEXT NOT NULL CHECK (
        length(owner_collector_epoch) = 32
        AND owner_collector_epoch NOT GLOB '*[^0-9a-f]*'
    ),
    confirmation_cycle_seq TEXT NOT NULL CHECK (
        length(confirmation_cycle_seq) BETWEEN 1 AND 20
        AND confirmation_cycle_seq NOT GLOB '*[^0-9]*'
    )
);
"#;

const GAP_LEDGER_REASONS: [&str; 3] = [
    "daemon_stop_unrecoverable",
    "reset_hint_expired",
    "auth_epoch_tombstoned",
];
const GAP_LEDGER_STATES: [&str; 4] = ["pending", "confirmed", "recovered", "rejected"];
const RECORDER_GAP_ID_BYTES: usize = 16;
const RECORDER_GAP_TEXT_BYTES: usize = 512;
const MAX_RECORDER_GAP_SOURCE_MINUTES: usize = 31 * 24 * 60;

const RESET_GROUP_TOLERANCE_SECONDS: i128 = 60;
const HISTORY_TIMESTAMP_RESET_INDEX: &str = "usage_history_timestamp_reset_idx";
const HISTORY_TIMESTAMP_RESET_INDEX_COLUMNS: &[&str] = &[
    "timestamp",
    "reset_at",
    "remaining_percent",
    "sol_dollars",
    "terra_dollars",
    "luna_dollars",
    "sol_tokens",
    "terra_tokens",
    "luna_tokens",
];
const DURABLE_STATE_OBSERVATION_MIN_SINGLETON: i64 = 2;
const MAX_OBSERVATION_JSON_BYTES: usize = 16 * 1024;
const OBSERVATION_JSON_KIND: &str = "codex-info-usage-observation-v1";
pub const MAX_SESSION_MODEL_BYTES: usize = 512;
const ACCOUNT_DB_SCHEMA_VERSION: i64 = 2;
const OBSERVATION_JSON_KEYS: &[&str] = &[
    "kind",
    "timestamp",
    "reset_at",
    "remaining_percent",
    "sol_dollars",
    "terra_dollars",
    "luna_dollars",
    "sol_tokens",
    "terra_tokens",
    "luna_tokens",
    "model_source",
];
const MAX_RECORDED_ROOT_IDENTITY_BYTES: usize = 256;
const MAX_RECORDED_RELATIVE_PATH_BYTES: usize = 4_096;
/// Maximum minute buckets materialized by a single one-month history read.
/// Persistent retention is independently three calendar months; callers must
/// never materialize that whole retention window merely to serve one request.
pub const MAX_RECENT_HISTORY_SAMPLES: usize = 31 * 24 * 60;
static BACKUP_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

const UPSERT_SAMPLE: &str = r#"
INSERT INTO usage_history (
    timestamp,
    reset_at,
    remaining_percent,
    sol_dollars,
    terra_dollars,
    luna_dollars,
    sol_tokens,
    terra_tokens,
    luna_tokens
)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
ON CONFLICT (reset_at, timestamp) DO UPDATE SET
    remaining_percent = excluded.remaining_percent,
    sol_dollars = excluded.sol_dollars,
    terra_dollars = excluded.terra_dollars,
    luna_dollars = excluded.luna_dollars,
    sol_tokens = excluded.sol_tokens,
    terra_tokens = excluded.terra_tokens,
    luna_tokens = excluded.luna_tokens
"#;

/// Returns the UTC instant three calendar months before `now`.
///
/// Chrono clamps an end-of-month date to the last valid day in the target
/// month, so May 31 minus three months is February 29 in a leap year (and
/// February 28 otherwise), rather than an arbitrary 90-day duration.
fn three_months_before(now: DateTime<Utc>) -> DateTime<Utc> {
    now.checked_sub_months(Months::new(3))
        .expect("subtracting three calendar months from UTC now must be representable")
}

/// Returns the UTC instant one calendar month before `now`.
///
/// History reads use the half-open interval `(cutoff, now]`: a 31-day month
/// therefore contains at most exactly 44,640 one-minute buckets, not 44,641.
fn one_month_before(now: DateTime<Utc>) -> DateTime<Utc> {
    now.checked_sub_months(Months::new(1))
        .expect("subtracting one calendar month from UTC now must be representable")
}

/// Upper bound for the serialized durable snapshot kept in SQLite.
pub const MAX_SNAPSHOT_JSON_BYTES: usize = 1024 * 1024;

/// One minute of usage history for a particular reset window.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageHistorySample {
    pub timestamp: i64,
    pub reset_at: i64,
    pub remaining_percent: Option<f64>,
    pub sol_dollars: f64,
    pub terra_dollars: f64,
    pub luna_dollars: f64,
    pub sol_tokens: u64,
    pub terra_tokens: u64,
    pub luna_tokens: u64,
}

/// Provenance of the local model vector for one history observation.
///
/// The legacy `usage_history` table cannot be extended without breaking the
/// v1.0.28 reader.  New observations therefore use this explicit sidecar
/// record while the old nine-column row remains the v1 projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelSource {
    Confirmed,
    Unavailable,
    LegacyUnknown,
}

impl ModelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Unavailable => "unavailable",
            Self::LegacyUnknown => "legacy-unknown",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "confirmed" => Some(Self::Confirmed),
            "unavailable" => Some(Self::Unavailable),
            "legacy-unknown" => Some(Self::LegacyUnknown),
            _ => None,
        }
    }
}

/// One bounded model/quota observation, including local-source provenance.
/// Model fields are all present for `confirmed` and `legacy-unknown`, and all
/// absent for `unavailable`; mixed vectors are rejected at the storage edge.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageHistoryObservation {
    pub timestamp: i64,
    pub reset_at: i64,
    pub remaining_percent: Option<f64>,
    pub sol_dollars: Option<f64>,
    pub terra_dollars: Option<f64>,
    pub luna_dollars: Option<f64>,
    pub sol_tokens: Option<u64>,
    pub terra_tokens: Option<u64>,
    pub luna_tokens: Option<u64>,
    pub model_source: ModelSource,
    /// Canonical per-model cumulative token facts for extensible consumers.
    /// Legacy v1 sidecars legitimately deserialize as `None`.
    pub model_totals: Option<Vec<SessionModelTotal>>,
    /// False means absent models are unknown, never zero.
    pub model_totals_complete: bool,
}

impl UsageHistoryObservation {
    pub fn confirmed(sample: &UsageHistorySample) -> Self {
        Self {
            timestamp: sample.timestamp,
            reset_at: sample.reset_at,
            remaining_percent: sample.remaining_percent,
            sol_dollars: Some(sample.sol_dollars),
            terra_dollars: Some(sample.terra_dollars),
            luna_dollars: Some(sample.luna_dollars),
            sol_tokens: Some(sample.sol_tokens),
            terra_tokens: Some(sample.terra_tokens),
            luna_tokens: Some(sample.luna_tokens),
            model_source: ModelSource::Confirmed,
            model_totals: None,
            model_totals_complete: false,
        }
    }

    pub fn confirmed_with_models(
        sample: &UsageHistorySample,
        model_totals: Vec<SessionModelTotal>,
    ) -> Self {
        let mut observation = Self::confirmed(sample);
        observation.model_totals = Some(model_totals);
        observation.model_totals_complete = true;
        observation
    }

    pub fn unavailable(timestamp: i64, reset_at: i64, remaining_percent: Option<f64>) -> Self {
        Self {
            timestamp,
            reset_at,
            remaining_percent,
            sol_dollars: None,
            terra_dollars: None,
            luna_dollars: None,
            sol_tokens: None,
            terra_tokens: None,
            luna_tokens: None,
            model_source: ModelSource::Unavailable,
            model_totals: None,
            model_totals_complete: false,
        }
    }

    pub fn legacy_unknown(sample: &UsageHistorySample) -> Self {
        Self {
            timestamp: sample.timestamp,
            reset_at: sample.reset_at,
            remaining_percent: sample.remaining_percent,
            sol_dollars: Some(sample.sol_dollars),
            terra_dollars: Some(sample.terra_dollars),
            luna_dollars: Some(sample.luna_dollars),
            sol_tokens: Some(sample.sol_tokens),
            terra_tokens: Some(sample.terra_tokens),
            luna_tokens: Some(sample.luna_tokens),
            model_source: ModelSource::LegacyUnknown,
            model_totals: None,
            model_totals_complete: false,
        }
    }

    fn validate(&self) -> Result<()> {
        // Existing usage_history rows may retain their original positive
        // event second. Keep the sidecar on that exact storage key; the
        // public history canonicalizer owns minute-start projection.
        if self.timestamp <= 0 || self.reset_at <= 0 {
            return Err(UsageStoreError::InvalidTimestamp {
                field: "observation timestamp",
                value: self.timestamp,
            });
        }
        if let Some(value) = self.remaining_percent {
            if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                return Err(UsageStoreError::InvalidImport(
                    "observation remaining_percent is invalid".into(),
                ));
            }
        }
        let values = [self.sol_dollars, self.terra_dollars, self.luna_dollars];
        let tokens = [self.sol_tokens, self.terra_tokens, self.luna_tokens];
        let all_values = values.iter().all(Option::is_some);
        let all_tokens = tokens.iter().all(Option::is_some);
        let any_values = values.iter().any(Option::is_some);
        let any_tokens = tokens.iter().any(Option::is_some);
        match self.model_source {
            ModelSource::Unavailable if any_values || any_tokens || self.model_totals.is_some() => {
                return Err(UsageStoreError::InvalidImport(
                    "unavailable observation contains model values".into(),
                ));
            }
            ModelSource::Confirmed | ModelSource::LegacyUnknown if !all_values || !all_tokens => {
                return Err(UsageStoreError::InvalidImport(
                    "confirmed observation has a partial model vector".into(),
                ));
            }
            _ => {}
        }
        if let Some(model_totals) = self.model_totals.as_ref() {
            let canonical = canonicalize_model_totals(model_totals)?;
            if canonical != *model_totals {
                return Err(UsageStoreError::InvalidImport(
                    "observation model totals are not canonical".into(),
                ));
            }
        }
        if self.model_totals_complete && self.model_totals.is_none() {
            return Err(UsageStoreError::InvalidImport(
                "complete observation model totals are missing".into(),
            ));
        }
        for (field, value) in [
            ("sol_dollars", self.sol_dollars),
            ("terra_dollars", self.terra_dollars),
            ("luna_dollars", self.luna_dollars),
        ] {
            if value.is_some_and(|value| !value.is_finite() || value < 0.0) {
                return Err(UsageStoreError::InvalidImport(format!(
                    "observation {field} is invalid"
                )));
            }
        }
        if tokens
            .into_iter()
            .flatten()
            .any(|value| value > i64::MAX as u64)
        {
            return Err(UsageStoreError::InvalidImport(
                "observation token count exceeds SQLite INTEGER range".into(),
            ));
        }
        Ok(())
    }
}

/// Exact identity of one session source whose bounded usage was committed.
///
/// The root identity is derived from the canonical sessions directory rather
/// than persisting its absolute path. Values wider than SQLite INTEGER use
/// canonical decimal text so a read-back never loses identity bits.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RecordedSessionSource {
    pub root_identity: String,
    pub relative_path: String,
    pub file_bytes: u64,
    pub modified_nanos: u128,
    pub file_device: u64,
    pub file_inode: u64,
}

/// A reset period identified only by the canonical reset timestamp.
///
/// The identifier is intentionally opaque to storage consumers. In
/// particular, it is not a formatted local-time label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetPeriod {
    pub canonical_id: i64,
    pub start_timestamp: i64,
    pub end_timestamp: i64,
}

/// Backwards-compatible descriptive alias for callers that use the history
/// terminology rather than the reset-period terminology.
pub type UsageHistoryPeriod = ResetPeriod;

/// The singleton durable snapshot associated with a committed history batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableRecord {
    pub data_generation: u64,
    pub data_hash: String,
    pub snapshot_json: String,
}

/// Durable identity that must match the one and only partition row in a
/// physical account database. All values are opaque lower-hex identifiers;
/// no raw account identifier is accepted by this layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoragePartitionIdentity {
    pub schema_version: String,
    pub profile_scope_id: String,
    pub account_scope_id: String,
    pub storage_epoch: u64,
    pub partition_id: String,
}

impl StoragePartitionIdentity {
    fn validate(&self) -> Result<()> {
        fn lower_hex(value: &str, bytes: usize) -> bool {
            value.len() == bytes * 2
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        if self.schema_version != "codex-info-account-db-v1"
            || !lower_hex(&self.profile_scope_id, 16)
            || !lower_hex(&self.account_scope_id, 32)
            || self.storage_epoch == 0
            || !lower_hex(&self.partition_id, 32)
        {
            return Err(UsageStoreError::InvalidImport(
                "storage partition identity is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCheckpoint {
    pub root_identity: String,
    pub relative_path: String,
    pub file_device: u64,
    pub file_inode: u64,
    pub committed_offset: u64,
    pub discard_until_lf: bool,
    pub collector_epoch: u128,
    pub cycle_seq: u64,
    pub prefix_generation: u128,
    pub prefix_sha256: String,
    pub fully_attributed_from_zero: bool,
    pub token_baseline_known: bool,
    pub last_model: Option<String>,
    pub last_task_running: Option<bool>,
    pub previous_total: u64,
    pub previous_input: u64,
    pub previous_cached_input: u64,
    pub previous_output: u64,
    pub previous_cache_write_input: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionRange {
    pub root_identity: String,
    pub relative_path: String,
    pub file_device: u64,
    pub file_inode: u64,
    pub start_offset: u64,
    pub end_offset: u64,
    pub collector_epoch: u128,
    pub cycle_seq: u64,
    pub prefix_generation: u128,
    pub record_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionModelTotal {
    pub model: String,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_input_tokens: Option<u64>,
}

/// Legacy history offset that still needs an exact component baseline.
///
/// Older account-partition hand-off records retained model total tokens and
/// dollars, but not the input/cache/output components used by the Main view.
/// The session worker may perform one bounded replay of the current latest
/// 2 GiB prefix. It accepts components only when every contributing source
/// has a durable checkpoint and the replay exactly matches this immutable
/// per-model token/cost authority.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryContinuityRecovery {
    pub source_fingerprint: String,
    pub source_rows: usize,
    pub boundary_timestamp: i64,
    pub reset_at: i64,
    pub sol_dollars: f64,
    pub terra_dollars: f64,
    pub luna_dollars: f64,
    pub sol_tokens: u64,
    pub terra_tokens: u64,
    pub luna_tokens: u64,
}

impl HistoryContinuityRecovery {
    pub fn matches_reset_at(&self, reset_at: i64) -> bool {
        reset_at > 0 && reset_at.abs_diff(self.reset_at) <= RESET_GROUP_TOLERANCE_SECONDS as u64
    }

    pub fn matches_dollar_totals(&self, sol: f64, terra: f64, luna: f64) -> bool {
        const MICRO_DOLLAR: f64 = 0.000_001;
        [
            (sol, self.sol_dollars),
            (terra, self.terra_dollars),
            (luna, self.luna_dollars),
        ]
        .into_iter()
        .all(|(actual, expected)| {
            actual.is_finite() && expected.is_finite() && (actual - expected).abs() <= MICRO_DOLLAR
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryContinuityModelRecovery {
    pub authority: HistoryContinuityRecovery,
    pub model_totals: Vec<SessionModelTotal>,
    /// Exact generation before the optional continuity offset was added.
    /// The recorder commits this payload when the independent recovery
    /// transaction is rejected, so recovery failure never blocks ordinary
    /// Session progress or leaks an uncommitted offset into durable state.
    pub fallback_samples: Vec<UsageHistorySample>,
    pub fallback_model_totals: Vec<SessionModelTotal>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionQuotaObservation {
    pub observed_at: i64,
    pub remaining_percent: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionCollectionState {
    pub data_generation: u64,
    pub reset_at: i64,
    pub window_seconds: i64,
    pub collector_epoch: Option<u128>,
    pub cycle_seq: u64,
    pub last_quota_observation: Option<SessionQuotaObservation>,
    pub checkpoints: Vec<SessionCheckpoint>,
    pub model_totals: Vec<SessionModelTotal>,
}

/// A source-proven recorder availability interval.  This is deliberately
/// separate from session ranges: session backfill can recover token usage but
/// cannot prove a point-in-time quota observation existed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecorderGap {
    pub gap_id: String,
    pub partition_id: String,
    pub source_identity_before: String,
    pub source_identity_after: String,
    pub cursor_before: String,
    pub cursor_after: String,
    pub stopped_at_monotonic_ns: u64,
    pub resumed_at_monotonic_ns: Option<u64>,
    pub start_at: i64,
    pub end_at: i64,
    pub reset_at: Option<i64>,
    pub reason: String,
    pub state: String,
    pub owner_collector_epoch: u128,
    pub confirmation_cycle_seq: u64,
}

pub struct SessionCollectionCommit<'a> {
    pub reset_at: i64,
    pub window_seconds: i64,
    pub collector_epoch: u128,
    pub cycle_seq: u64,
    pub samples: &'a [UsageHistorySample],
    pub checkpoints: &'a [SessionCheckpoint],
    pub ranges: &'a [SessionRange],
    pub model_totals: &'a [SessionModelTotal],
    pub recorded_sessions: &'a [RecordedSessionSource],
}

pub struct SessionCollectionCommitResult {
    pub data_generation: u64,
    pub canonical_samples: Vec<UsageHistorySample>,
    pub canonical_observations: Vec<UsageHistoryObservation>,
}

#[derive(Clone, Debug, PartialEq)]
struct HistoryContinuity {
    source_fingerprint: String,
    source_rows: usize,
    boundary_timestamp: i64,
    reset_at: i64,
    remaining_percent: f64,
    sol_dollars: f64,
    terra_dollars: f64,
    luna_dollars: f64,
    sol_tokens: u64,
    terra_tokens: u64,
    luna_tokens: u64,
    model_totals_applied: bool,
}

/// Result of a verified, candidate-database migration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationReport {
    pub source_rows: usize,
    pub candidate_rows: usize,
    pub source_fingerprint: String,
    pub candidate_fingerprint: String,
    pub preserved_backup: std::path::PathBuf,
}

/// Errors returned while opening or using a usage history database.
#[derive(Debug)]
pub enum UsageStoreError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    InvalidImport(String),
    InvalidDurableRecord(String),
    InvalidTimestamp { field: &'static str, value: i64 },
    NonFiniteValue { field: &'static str },
    GenerationConflict { expected: u64, actual: u64 },
    GenerationOverflow,
}

pub type Result<T> = std::result::Result<T, UsageStoreError>;

impl fmt::Display for UsageStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "database directory error: {error}"),
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::InvalidImport(error) => write!(formatter, "invalid usage import: {error}"),
            Self::InvalidDurableRecord(error) => {
                write!(formatter, "invalid durable record: {error}")
            }
            Self::InvalidTimestamp { field, value } => write!(
                formatter,
                "invalid {field} timestamp {value}; expected a positive Unix timestamp"
            ),
            Self::NonFiniteValue { field } => {
                write!(formatter, "{field} must be finite")
            }
            Self::GenerationConflict { expected, actual } => write!(
                formatter,
                "durable generation conflict: expected {expected}, found {actual}"
            ),
            Self::GenerationOverflow => write!(formatter, "durable generation overflow"),
        }
    }
}

impl std::error::Error for UsageStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::InvalidImport(_)
            | Self::InvalidDurableRecord(_)
            | Self::InvalidTimestamp { .. }
            | Self::NonFiniteValue { .. }
            | Self::GenerationConflict { .. }
            | Self::GenerationOverflow => None,
        }
    }
}

impl From<std::io::Error> for UsageStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for UsageStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl UsageHistorySample {
    fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("sol_dollars", self.sol_dollars),
            ("terra_dollars", self.terra_dollars),
            ("luna_dollars", self.luna_dollars),
        ] {
            if !value.is_finite() {
                return Err(UsageStoreError::NonFiniteValue { field });
            }
            if value < 0.0 {
                return Err(UsageStoreError::InvalidImport(format!(
                    "{field} must be finite and non-negative"
                )));
            }
        }

        if let Some(value) = self.remaining_percent {
            if !value.is_finite() {
                return Err(UsageStoreError::NonFiniteValue {
                    field: "remaining_percent",
                });
            }
            if !(0.0..=100.0).contains(&value) {
                return Err(UsageStoreError::InvalidImport(
                    "remaining_percent must be finite and between 0 and 100".into(),
                ));
            }
        }

        if self.timestamp <= 0 {
            return Err(UsageStoreError::InvalidTimestamp {
                field: "timestamp",
                value: self.timestamp,
            });
        }
        if self.reset_at <= 0 {
            return Err(UsageStoreError::InvalidTimestamp {
                field: "reset_at",
                value: self.reset_at,
            });
        }
        if [self.sol_tokens, self.terra_tokens, self.luna_tokens]
            .into_iter()
            .any(|tokens| tokens > i64::MAX as u64)
        {
            return Err(UsageStoreError::InvalidImport(
                "token count exceeds SQLite INTEGER range".into(),
            ));
        }

        Ok(())
    }
}

impl RecordedSessionSource {
    fn validate(&self) -> Result<()> {
        if self.root_identity.is_empty()
            || self.root_identity.len() > MAX_RECORDED_ROOT_IDENTITY_BYTES
            || !self.root_identity.is_ascii()
        {
            return Err(UsageStoreError::InvalidImport(
                "recorded session root identity is invalid".into(),
            ));
        }
        if self.relative_path.is_empty()
            || self.relative_path.len() > MAX_RECORDED_RELATIVE_PATH_BYTES
        {
            return Err(UsageStoreError::InvalidImport(
                "recorded session relative path is invalid".into(),
            ));
        }
        let relative = Path::new(&self.relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(UsageStoreError::InvalidImport(
                "recorded session relative path is invalid".into(),
            ));
        }
        if self.file_bytes > i64::MAX as u64 {
            return Err(UsageStoreError::InvalidImport(
                "recorded session size exceeds SQLite INTEGER range".into(),
            ));
        }
        Ok(())
    }
}

fn canonicalize_recorded_sessions(
    sources: &[RecordedSessionSource],
) -> Result<Vec<RecordedSessionSource>> {
    let mut canonical = BTreeSet::new();
    for source in sources {
        source.validate()?;
        canonical.insert(source.clone());
    }
    Ok(canonical.into_iter().collect())
}

fn canonical_u64_text(value: &str, field: &'static str) -> Result<u64> {
    let parsed = value.parse::<u64>().map_err(|_| {
        UsageStoreError::InvalidImport(format!("{field} is not a canonical unsigned integer"))
    })?;
    if parsed.to_string() != value {
        return Err(UsageStoreError::InvalidImport(format!(
            "{field} is not a canonical unsigned integer"
        )));
    }
    Ok(parsed)
}

fn canonical_u128_hex(value: &str, field: &'static str) -> Result<u128> {
    if value.len() != 32
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(UsageStoreError::InvalidImport(format!(
            "{field} is not canonical lower hexadecimal"
        )));
    }
    let parsed = u128::from_str_radix(value, 16).map_err(|_| {
        UsageStoreError::InvalidImport(format!("{field} is not canonical lower hexadecimal"))
    })?;
    if parsed == 0 {
        return Err(UsageStoreError::InvalidImport(format!(
            "{field} must be non-zero"
        )));
    }
    Ok(parsed)
}

fn validate_sha256(value: &str, field: &'static str) -> Result<()> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(UsageStoreError::InvalidImport(format!(
            "{field} is not a SHA-256 digest"
        )));
    }
    Ok(())
}

fn validate_lower_hex(value: &str, bytes: usize, field: &'static str) -> Result<()> {
    if value.len() != bytes.saturating_mul(2)
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(UsageStoreError::InvalidImport(format!(
            "{field} is not canonical lower hexadecimal"
        )));
    }
    Ok(())
}

fn validate_gap_text(value: &str, field: &'static str) -> Result<()> {
    if value.is_empty()
        || value.len() > RECORDER_GAP_TEXT_BYTES
        || !value.is_ascii()
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(UsageStoreError::InvalidImport(format!(
            "{field} is invalid"
        )));
    }
    Ok(())
}

fn validate_recorder_gap(gap: &RecorderGap, expected_partition_id: Option<&str>) -> Result<()> {
    validate_lower_hex(&gap.gap_id, RECORDER_GAP_ID_BYTES, "gap_id")?;
    validate_lower_hex(&gap.partition_id, 32, "partition_id")?;
    if expected_partition_id.is_some_and(|expected| expected != gap.partition_id) {
        return Err(UsageStoreError::InvalidImport(
            "recorder gap partition identity mismatch".into(),
        ));
    }
    for (field, value) in [
        ("source_identity_before", &gap.source_identity_before),
        ("source_identity_after", &gap.source_identity_after),
        ("cursor_before", &gap.cursor_before),
        ("cursor_after", &gap.cursor_after),
    ] {
        validate_gap_text(value, field)?;
    }
    if gap.stopped_at_monotonic_ns == 0
        || gap
            .resumed_at_monotonic_ns
            .is_some_and(|resumed| resumed < gap.stopped_at_monotonic_ns)
        || gap.start_at <= 0
        || gap.end_at < gap.start_at
        || gap.reset_at.is_some_and(|reset| reset <= 0)
        || !GAP_LEDGER_REASONS.contains(&gap.reason.as_str())
        || !GAP_LEDGER_STATES.contains(&gap.state.as_str())
        || gap.owner_collector_epoch == 0
        || gap.confirmation_cycle_seq == 0
    {
        return Err(UsageStoreError::InvalidImport(
            "recorder gap bounds or state are invalid".into(),
        ));
    }
    Ok(())
}

fn validate_gap_repair_proof(gap: &RecorderGap) -> Result<()> {
    if gap.resumed_at_monotonic_ns.is_none()
        || gap.source_identity_after == "unresolved"
        || gap.cursor_after == "unresolved"
        || gap.reset_at.is_none_or(|reset_at| reset_at < gap.end_at)
    {
        return Err(UsageStoreError::InvalidImport(
            "recorder gap terminal transition lacks source proof".into(),
        ));
    }
    Ok(())
}

fn validate_recorder_source_rescan(
    source_identity_after: &str,
    cursor_after: &str,
    resumed_at_monotonic_ns: u64,
    reset_at: i64,
    owner_collector_epoch: u128,
    confirmation_cycle_seq: u64,
    source_minutes: &[i64],
) -> Result<()> {
    validate_gap_text(source_identity_after, "source_identity_after")?;
    validate_gap_text(cursor_after, "cursor_after")?;
    if source_identity_after == "unresolved" || cursor_after == "unresolved" {
        return Err(UsageStoreError::InvalidImport(
            "recorder source proof is unresolved".into(),
        ));
    }
    if resumed_at_monotonic_ns == 0
        || reset_at <= 0
        || owner_collector_epoch == 0
        || confirmation_cycle_seq == 0
        || source_minutes.len() > MAX_RECORDER_GAP_SOURCE_MINUTES
    {
        return Err(UsageStoreError::InvalidImport(
            "recorder source proof bounds are invalid".into(),
        ));
    }
    let mut previous = None;
    for minute in source_minutes {
        if *minute <= 0 || minute.rem_euclid(60) != 0 {
            return Err(UsageStoreError::InvalidImport(
                "recorder source proof minute is not canonical".into(),
            ));
        }
        if previous.is_some_and(|previous| *minute <= previous) {
            return Err(UsageStoreError::InvalidImport(
                "recorder source proof minutes are not unique and sorted".into(),
            ));
        }
        previous = Some(*minute);
    }
    Ok(())
}

fn gap_expected_source_minutes(gap: &RecorderGap) -> Option<(i64, i64)> {
    // History rows represent minute starts. A partial first/last minute is
    // not considered sourced unless its complete bucket is present; this
    // avoids treating a nearby observation as proof for a closed interval.
    let first = gap
        .start_at
        .div_euclid(60)
        .checked_add(1)?
        .checked_mul(60)?;
    let last = gap.end_at.div_euclid(60).checked_mul(60)?;
    (first <= last).then_some((first, last))
}

fn source_minutes_cover_gap(gap: &RecorderGap, source_minutes: &[i64]) -> bool {
    let Some((first, last)) = gap_expected_source_minutes(gap) else {
        return false;
    };
    let required = last
        .checked_sub(first)
        .and_then(|span| usize::try_from(span / 60).ok())
        .and_then(|count| count.checked_add(1));
    let Some(required) = required else {
        return false;
    };
    if required == 0 || required > MAX_RECORDER_GAP_SOURCE_MINUTES {
        return false;
    }
    let start_index = source_minutes.partition_point(|minute| *minute < first);
    source_minutes
        .get(start_index..start_index.saturating_add(required))
        .is_some_and(|minutes| {
            minutes.iter().enumerate().all(|(index, minute)| {
                first
                    .checked_add((index as i64).saturating_mul(60))
                    .is_some_and(|expected| *minute == expected)
            })
        })
}

fn source_minutes_overlap_gap(gap: &RecorderGap, source_minutes: &[i64]) -> bool {
    source_minutes
        .iter()
        .any(|minute| *minute >= gap.start_at && *minute <= gap.end_at)
}

fn recorder_gap_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecorderGap> {
    let stopped_at_monotonic_ns = row.get::<_, i64>(6)?;
    let resumed_at_monotonic_ns = row.get::<_, Option<i64>>(7)?;
    let owner_collector_epoch = row.get::<_, String>(13)?;
    let confirmation_cycle_seq = row.get::<_, String>(14)?;
    Ok(RecorderGap {
        gap_id: row.get(0)?,
        partition_id: row.get(1)?,
        source_identity_before: row.get(2)?,
        source_identity_after: row.get(3)?,
        cursor_before: row.get(4)?,
        cursor_after: row.get(5)?,
        stopped_at_monotonic_ns: u64::try_from(stopped_at_monotonic_ns)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        resumed_at_monotonic_ns: resumed_at_monotonic_ns
            .map(u64::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        start_at: row.get(8)?,
        end_at: row.get(9)?,
        reset_at: row.get(10)?,
        reason: row.get(11)?,
        state: row.get(12)?,
        owner_collector_epoch: u128::from_str_radix(&owner_collector_epoch, 16)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        confirmation_cycle_seq: confirmation_cycle_seq
            .parse::<u64>()
            .ok()
            .filter(|value| value.to_string() == confirmation_cycle_seq)
            .ok_or(rusqlite::Error::InvalidQuery)?,
    })
}

fn validate_session_key(root_identity: &str, relative_path: &str) -> Result<()> {
    RecordedSessionSource {
        root_identity: root_identity.to_owned(),
        relative_path: relative_path.to_owned(),
        file_bytes: 0,
        modified_nanos: 0,
        file_device: 0,
        file_inode: 0,
    }
    .validate()
}

fn validate_session_checkpoint(checkpoint: &SessionCheckpoint) -> Result<()> {
    validate_session_key(&checkpoint.root_identity, &checkpoint.relative_path)?;
    if checkpoint.committed_offset > i64::MAX as u64
        || checkpoint.collector_epoch == 0
        || checkpoint.cycle_seq == 0
        || checkpoint.prefix_generation == 0
        || checkpoint
            .last_model
            .as_deref()
            .is_some_and(|model| !valid_session_model(model))
        || checkpoint.previous_cached_input > checkpoint.previous_input
        || checkpoint.previous_cache_write_input.is_some_and(|writes| {
            checkpoint
                .previous_cached_input
                .checked_add(writes)
                .is_none_or(|cached| cached > checkpoint.previous_input)
        })
    {
        return Err(UsageStoreError::InvalidImport(
            "session checkpoint is invalid".into(),
        ));
    }
    validate_sha256(&checkpoint.prefix_sha256, "session checkpoint prefix")?;
    Ok(())
}

fn valid_session_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= MAX_SESSION_MODEL_BYTES
        && model.trim() == model
        && !model.chars().any(char::is_control)
}

fn validate_session_range(range: &SessionRange) -> Result<()> {
    validate_session_key(&range.root_identity, &range.relative_path)?;
    if range.start_offset >= range.end_offset
        || range.end_offset > i64::MAX as u64
        || range.collector_epoch == 0
        || range.cycle_seq == 0
        || range.prefix_generation == 0
    {
        return Err(UsageStoreError::InvalidImport(
            "session range is invalid".into(),
        ));
    }
    validate_sha256(&range.record_sha256, "session range record")?;
    Ok(())
}

fn canonicalize_model_totals(totals: &[SessionModelTotal]) -> Result<Vec<SessionModelTotal>> {
    let mut canonical = BTreeMap::new();
    for total in totals {
        if !valid_session_model(&total.model)
            || total.cached_input_tokens > total.input_tokens
            || total.cache_write_input_tokens.is_some_and(|writes| {
                total
                    .cached_input_tokens
                    .checked_add(writes)
                    .is_none_or(|cached| cached > total.input_tokens)
            })
        {
            return Err(UsageStoreError::InvalidImport(
                "session model total is invalid".into(),
            ));
        }
        if canonical
            .insert(total.model.clone(), total.clone())
            .is_some()
        {
            return Err(UsageStoreError::InvalidImport(
                "duplicate session model total".into(),
            ));
        }
    }
    Ok(canonical.into_values().collect())
}

fn recorded_session_matches_in(
    connection: &Connection,
    source: &RecordedSessionSource,
) -> Result<bool> {
    source.validate()?;
    let matched: i64 = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM recorded_sessions
            WHERE root_identity = ?1
              AND relative_path = ?2
              AND file_bytes = ?3
              AND modified_nanos = ?4
              AND file_device = ?5
              AND file_inode = ?6
        )",
        params![
            &source.root_identity,
            &source.relative_path,
            source.file_bytes as i64,
            source.modified_nanos.to_string(),
            source.file_device.to_string(),
            source.file_inode.to_string(),
        ],
        |row| row.get(0),
    )?;
    Ok(matched == 1)
}

fn validate_recorded_sessions_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let mut statement = transaction.prepare(
        "SELECT cid, name, type, \"notnull\", dflt_value, pk \
         FROM pragma_table_info('recorded_sessions') ORDER BY cid ASC",
    )?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let expected = vec![
        (0, "root_identity".to_owned(), "TEXT".to_owned(), 1, None, 1),
        (1, "relative_path".to_owned(), "TEXT".to_owned(), 1, None, 2),
        (2, "file_bytes".to_owned(), "INTEGER".to_owned(), 1, None, 3),
        (
            3,
            "modified_nanos".to_owned(),
            "TEXT".to_owned(),
            1,
            None,
            4,
        ),
        (4, "file_device".to_owned(), "TEXT".to_owned(), 1, None, 5),
        (5, "file_inode".to_owned(), "TEXT".to_owned(), 1, None, 6),
    ];
    if columns != expected {
        return Err(UsageStoreError::InvalidImport(
            "recorded session schema mismatch".into(),
        ));
    }
    Ok(())
}

fn usage_vector_dominates(candidate: &UsageHistorySample, observed: &UsageHistorySample) -> bool {
    candidate.sol_dollars >= observed.sol_dollars
        && candidate.terra_dollars >= observed.terra_dollars
        && candidate.luna_dollars >= observed.luna_dollars
        && candidate.sol_tokens >= observed.sol_tokens
        && candidate.terra_tokens >= observed.terra_tokens
        && candidate.luna_tokens >= observed.luna_tokens
}

fn canonicalize_sample_group(samples: &[UsageHistorySample]) -> Result<UsageHistorySample> {
    debug_assert!(!samples.is_empty());

    let quota = samples
        .iter()
        .filter_map(|sample| sample.remaining_percent)
        .try_fold(None, |quota: Option<f64>, observed| {
            if let Some(existing) = quota {
                if existing != observed {
                    return Err(UsageStoreError::InvalidImport(format!(
                        "conflicting remaining_percent values for ({}, {})",
                        samples[0].reset_at, samples[0].timestamp
                    )));
                }
                Ok(Some(existing))
            } else {
                Ok(Some(observed))
            }
        })?;

    let canonical = samples
        .iter()
        .find(|candidate| {
            samples
                .iter()
                .all(|observed| usage_vector_dominates(candidate, observed))
        })
        .cloned()
        .ok_or_else(|| {
            UsageStoreError::InvalidImport(format!(
                "non-comparable usage vectors for ({}, {})",
                samples[0].reset_at, samples[0].timestamp
            ))
        })?;

    // The usage vector remains an observed whole vector. A single non-null
    // quota is the only value allowed to be carried across observations when
    // the dominating observation omitted it.
    Ok(UsageHistorySample {
        remaining_percent: quota,
        ..canonical
    })
}

fn reconcile_existing_sample(
    existing: UsageHistorySample,
    incoming: UsageHistorySample,
) -> Result<UsageHistorySample> {
    let incoming_dominates = usage_vector_dominates(&incoming, &existing);
    let existing_dominates = usage_vector_dominates(&existing, &incoming);
    if !incoming_dominates && !existing_dominates {
        return Err(UsageStoreError::InvalidImport(format!(
            "non-comparable usage vectors for ({}, {})",
            incoming.reset_at, incoming.timestamp
        )));
    }

    // A newer observation may contain a quota even when its usage vector is
    // behind the row already stored. Keep the non-regressing usage vector but
    // still honor that observation's quota; a missing quota carries forward a
    // previously observed value.
    let mut reconciled = if incoming_dominates {
        incoming.clone()
    } else {
        existing.clone()
    };
    reconciled.remaining_percent = incoming.remaining_percent.or(existing.remaining_percent);
    Ok(reconciled)
}

fn canonicalize_samples(
    transaction: &rusqlite::Transaction<'_>,
    samples: &[UsageHistorySample],
) -> Result<Vec<UsageHistorySample>> {
    let mut grouped = std::collections::BTreeMap::<(i64, i64), Vec<UsageHistorySample>>::new();
    for sample in samples {
        sample.validate()?;
        grouped
            .entry((sample.reset_at, sample.timestamp))
            .or_default()
            .push(sample.clone());
    }

    let incoming = grouped
        .into_values()
        .map(|observations| canonicalize_sample_group(&observations))
        .collect::<Result<Vec<_>>>()?;

    let mut existing_statement = transaction.prepare(
        "SELECT timestamp, reset_at, remaining_percent, sol_dollars, \
                terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens \
         FROM usage_history WHERE reset_at = ?1 AND timestamp = ?2",
    )?;
    let mut canonical = Vec::with_capacity(incoming.len());
    for incoming in incoming {
        let existing = existing_statement
            .query_row(params![incoming.reset_at, incoming.timestamp], |row| {
                let sample = valid_sample_from_row(row)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                sample.ok_or(rusqlite::Error::InvalidQuery)
            })
            .optional()?;
        canonical.push(match existing {
            Some(existing) => reconcile_existing_sample(existing, incoming)?,
            None => incoming,
        });
    }

    Ok(canonical)
}

fn upsert_canonical_samples(
    transaction: &rusqlite::Transaction<'_>,
    samples: &[UsageHistorySample],
) -> Result<()> {
    let mut statement = transaction.prepare(UPSERT_SAMPLE)?;
    for sample in samples {
        statement.execute(params![
            sample.timestamp,
            sample.reset_at,
            sample.remaining_percent,
            sample.sol_dollars,
            sample.terra_dollars,
            sample.luna_dollars,
            sample.sol_tokens as i64,
            sample.terra_tokens as i64,
            sample.luna_tokens as i64,
        ])?;
    }
    Ok(())
}

fn canonicalize_observations(
    transaction: &rusqlite::Transaction<'_>,
    observations: &[UsageHistoryObservation],
    canonical_samples: &[UsageHistorySample],
) -> Result<Vec<UsageHistoryObservation>> {
    let canonical_samples = canonical_samples
        .iter()
        .map(|sample| ((sample.reset_at, sample.timestamp), sample))
        .collect::<BTreeMap<_, _>>();
    let mut canonical = BTreeMap::new();
    for observation in observations {
        observation.validate()?;
        let key = (observation.reset_at, observation.timestamp);
        if canonical.insert(key, observation.clone()).is_some() {
            return Err(UsageStoreError::InvalidImport(
                "duplicate usage observation key".into(),
            ));
        }
    }
    for (key, observation) in canonical.iter_mut() {
        match observation.model_source {
            ModelSource::Confirmed | ModelSource::LegacyUnknown => {
                let sample = if let Some(sample) = canonical_samples.get(key) {
                    Some((*sample).clone())
                } else {
                    transaction
                        .query_row(
                            "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                                    terra_dollars, luna_dollars, sol_tokens, terra_tokens,
                                    luna_tokens
                             FROM usage_history WHERE reset_at = ?1 AND timestamp = ?2",
                            params![key.0, key.1],
                            |row| {
                                valid_sample_from_row(row).map_err(|error| {
                                    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                                })
                            },
                        )
                        .optional()?
                        .flatten()
                };
                let Some(sample) = sample else {
                    return Err(UsageStoreError::InvalidImport(
                        "model observation has no usage_history vector".into(),
                    ));
                };
                let observed_vector_matches = observation.sol_dollars == Some(sample.sol_dollars)
                    && observation.terra_dollars == Some(sample.terra_dollars)
                    && observation.luna_dollars == Some(sample.luna_dollars)
                    && observation.sol_tokens == Some(sample.sol_tokens)
                    && observation.terra_tokens == Some(sample.terra_tokens)
                    && observation.luna_tokens == Some(sample.luna_tokens);
                // `canonicalize_samples` deliberately retains an already
                // stored dominant vector when a later read regresses. Such a
                // retained value was not confirmed by this observation, so do
                // not promote it to a solid-line source until a full vector
                // actually recovers.
                if observation.model_source == ModelSource::Confirmed && !observed_vector_matches {
                    observation.model_source = ModelSource::LegacyUnknown;
                }
                observation.remaining_percent =
                    observation.remaining_percent.or(sample.remaining_percent);
                observation.sol_dollars = Some(sample.sol_dollars);
                observation.terra_dollars = Some(sample.terra_dollars);
                observation.luna_dollars = Some(sample.luna_dollars);
                observation.sol_tokens = Some(sample.sol_tokens);
                observation.terra_tokens = Some(sample.terra_tokens);
                observation.luna_tokens = Some(sample.luna_tokens);
            }
            ModelSource::Unavailable => {
                if canonical_samples.contains_key(key) {
                    return Err(UsageStoreError::InvalidImport(
                        "unavailable observation conflicts with usage_history vector".into(),
                    ));
                }
            }
        }
        observation.validate()?;
    }
    Ok(canonical.into_values().collect())
}

fn upsert_observations(
    transaction: &rusqlite::Transaction<'_>,
    observations: &[UsageHistoryObservation],
) -> Result<Vec<UsageHistoryObservation>> {
    if observations.is_empty() {
        return Ok(Vec::new());
    }
    let mut next_singleton: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(singleton), 1) FROM durable_state",
        [],
        |row| row.get(0),
    )?;
    if next_singleton < 1 {
        return Err(UsageStoreError::InvalidDurableRecord(
            "durable_state contains an invalid singleton".into(),
        ));
    }
    let mut persisted = BTreeMap::new();
    for observation in observations {
        let snapshot_json = observation_json(observation)?;
        let data_hash = observation_data_hash(observation.reset_at, observation.timestamp);
        let existing: Option<(i64, i64, String)> = transaction
            .query_row(
                "SELECT singleton, data_generation, snapshot_json FROM durable_state
                 WHERE singleton >= ?1 AND data_hash = ?2",
                params![DURABLE_STATE_OBSERVATION_MIN_SINGLETON, &data_hash],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((singleton, data_generation, existing_json)) = existing else {
            next_singleton = next_singleton
                .checked_add(1)
                .ok_or(UsageStoreError::GenerationOverflow)?;
            transaction.execute(
                "INSERT INTO durable_state (singleton, data_generation, data_hash, snapshot_json)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    next_singleton,
                    observation.timestamp,
                    &data_hash,
                    &snapshot_json,
                ],
            )?;
            persisted.insert(
                (observation.reset_at, observation.timestamp),
                observation.clone(),
            );
            continue;
        };
        let existing = observation_from_sql(data_generation, data_hash.clone(), existing_json)?;
        let selected = match (existing.model_source, observation.model_source) {
            (ModelSource::Unavailable, ModelSource::Confirmed | ModelSource::LegacyUnknown) => {
                observation.clone()
            }
            (ModelSource::Confirmed | ModelSource::LegacyUnknown, ModelSource::Unavailable) => {
                existing.clone()
            }
            (ModelSource::Confirmed, ModelSource::Confirmed)
            | (ModelSource::LegacyUnknown, ModelSource::Confirmed)
            | (ModelSource::LegacyUnknown, ModelSource::LegacyUnknown) => observation.clone(),
            (ModelSource::Confirmed, ModelSource::LegacyUnknown) => existing.clone(),
            (ModelSource::Unavailable, ModelSource::Unavailable) => observation.clone(),
        };
        if selected != existing {
            let selected_json = observation_json(&selected)?;
            transaction.execute(
                "UPDATE durable_state SET data_generation = ?1, snapshot_json = ?2
                 WHERE singleton = ?3",
                params![selected.timestamp, &selected_json, singleton],
            )?;
        }
        persisted.insert((selected.reset_at, selected.timestamp), selected);
    }
    Ok(persisted.into_values().collect())
}

fn upsert_observation_model_totals(
    transaction: &rusqlite::Transaction<'_>,
    observations: &[UsageHistoryObservation],
) -> Result<()> {
    let mut delete = transaction
        .prepare("DELETE FROM usage_model_history WHERE reset_at = ?1 AND timestamp = ?2")?;
    let mut insert = transaction.prepare(
        "INSERT INTO usage_model_history (
            reset_at, timestamp, model, total_tokens, input_tokens,
            cached_input_tokens, output_tokens, cache_write_input_tokens,
            model_set_complete
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    for observation in observations {
        let Some(model_totals) = observation.model_totals.as_ref() else {
            continue;
        };
        let model_totals = canonicalize_model_totals(model_totals)?;
        delete.execute(params![observation.reset_at, observation.timestamp])?;
        for total in model_totals {
            insert.execute(params![
                observation.reset_at,
                observation.timestamp,
                &total.model,
                total.total_tokens.to_string(),
                total.input_tokens.to_string(),
                total.cached_input_tokens.to_string(),
                total.output_tokens.to_string(),
                total
                    .cache_write_input_tokens
                    .map(|value| value.to_string()),
                i64::from(observation.model_totals_complete),
            ])?;
        }
    }
    Ok(())
}

fn numeric_sqlite_value(value: Value) -> Option<f64> {
    match value {
        Value::Integer(value) => {
            let value_as_f64 = value as f64;
            (value_as_f64 as i128 == i128::from(value)).then_some(value_as_f64)
        }
        Value::Real(value) => Some(value),
        _ => None,
    }
}

fn valid_sample_from_row(row: &rusqlite::Row<'_>) -> Result<Option<UsageHistorySample>> {
    let timestamp = match row.get::<_, Value>(0)? {
        Value::Integer(value) => value,
        _ => return Ok(None),
    };
    let reset_at = match row.get::<_, Value>(1)? {
        Value::Integer(value) => value,
        _ => return Ok(None),
    };
    let remaining_percent = match row.get::<_, Value>(2)? {
        Value::Null => None,
        value => {
            let Some(value) = numeric_sqlite_value(value) else {
                return Ok(None);
            };
            Some(value)
        }
    };
    let sol_dollars = match numeric_sqlite_value(row.get(3)?) {
        Some(value) => value,
        _ => return Ok(None),
    };
    let terra_dollars = match numeric_sqlite_value(row.get(4)?) {
        Some(value) => value,
        _ => return Ok(None),
    };
    let luna_dollars = match numeric_sqlite_value(row.get(5)?) {
        Some(value) => value,
        _ => return Ok(None),
    };
    let sol_tokens = match row.get::<_, Value>(6)? {
        Value::Integer(value) if value >= 0 => match u64::try_from(value) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        },
        _ => return Ok(None),
    };
    let terra_tokens = match row.get::<_, Value>(7)? {
        Value::Integer(value) if value >= 0 => match u64::try_from(value) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        },
        _ => return Ok(None),
    };
    let luna_tokens = match row.get::<_, Value>(8)? {
        Value::Integer(value) if value >= 0 => match u64::try_from(value) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        },
        _ => return Ok(None),
    };

    let sample = UsageHistorySample {
        timestamp,
        reset_at,
        remaining_percent,
        sol_dollars,
        terra_dollars,
        luna_dollars,
        sol_tokens,
        terra_tokens,
        luna_tokens,
    };
    if sample.validate().is_err() {
        return Ok(None);
    }
    Ok(Some(sample))
}

fn samples_fingerprint(samples: &[UsageHistorySample]) -> String {
    // A deterministic, dependency-free fingerprint is sufficient for the
    // migration gate: it detects any row/value/order change between the
    // source and candidate snapshots without persisting credentials or data.
    let mut hash = 0xcbf29ce484222325_u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    for sample in samples {
        feed(&sample.timestamp.to_le_bytes());
        feed(&sample.reset_at.to_le_bytes());
        feed(
            &sample
                .remaining_percent
                .unwrap_or(f64::NAN)
                .to_bits()
                .to_le_bytes(),
        );
        feed(&sample.sol_dollars.to_bits().to_le_bytes());
        feed(&sample.terra_dollars.to_bits().to_le_bytes());
        feed(&sample.luna_dollars.to_bits().to_le_bytes());
        feed(&sample.sol_tokens.to_le_bytes());
        feed(&sample.terra_tokens.to_le_bytes());
        feed(&sample.luna_tokens.to_le_bytes());
    }
    format!("{hash:016x}")
}

fn load_history_continuity(connection: &Connection) -> Result<Option<HistoryContinuity>> {
    let present: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='history_continuity')",
        [],
        |row| row.get(0),
    )?;
    if !present {
        return Ok(None);
    }
    let applied_column_present: bool = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('history_continuity')
            WHERE name = 'model_totals_applied'
        )",
        [],
        |row| row.get(0),
    )?;
    let applied_column = if applied_column_present {
        "model_totals_applied"
    } else {
        // A read-only lane can observe the previous schema immediately
        // before the serialized recorder owner performs its migration.
        "0"
    };
    let query = format!(
        "SELECT source_fingerprint, source_rows, boundary_timestamp, reset_at,
                remaining_percent, sol_dollars, terra_dollars, luna_dollars,
                sol_tokens, terra_tokens, luna_tokens, {applied_column}
         FROM history_continuity WHERE singleton=1"
    );
    let row = connection
        .query_row(&query, [], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, i64>(11)?,
            ))
        })
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.0.len() != 16
        || row
            .0
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        || row.1 <= 0
        || row.2 <= 0
        || row.3 <= 0
        || !row.4.is_finite()
        || !(0.0..=100.0).contains(&row.4)
        || [row.5, row.6, row.7]
            .into_iter()
            .any(|value| !value.is_finite() || value < 0.0)
        || !matches!(row.11, 0 | 1)
    {
        return Err(UsageStoreError::InvalidImport(
            "history continuity record is invalid".into(),
        ));
    }
    Ok(Some(HistoryContinuity {
        source_fingerprint: row.0,
        source_rows: usize::try_from(row.1).map_err(|_| {
            UsageStoreError::InvalidImport("history continuity row count is invalid".into())
        })?,
        boundary_timestamp: row.2,
        reset_at: row.3,
        remaining_percent: row.4,
        sol_dollars: row.5,
        terra_dollars: row.6,
        luna_dollars: row.7,
        sol_tokens: canonical_u64_text(&row.8, "history continuity SOL tokens")?,
        terra_tokens: canonical_u64_text(&row.9, "history continuity TERRA tokens")?,
        luna_tokens: canonical_u64_text(&row.10, "history continuity LUNA tokens")?,
        model_totals_applied: row.11 == 1,
    }))
}

fn apply_history_continuity(
    connection: &Connection,
    samples: &[UsageHistorySample],
) -> Result<Vec<UsageHistorySample>> {
    let Some(offset) = load_history_continuity(connection)? else {
        return Ok(samples.to_vec());
    };
    if offset.model_totals_applied {
        return Ok(samples.to_vec());
    }
    samples
        .iter()
        .map(|sample| {
            if sample.timestamp < offset.boundary_timestamp
                || sample.reset_at.abs_diff(offset.reset_at) > RESET_GROUP_TOLERANCE_SECONDS as u64
            {
                return Ok(sample.clone());
            }
            let mut adjusted = sample.clone();
            adjusted.sol_dollars += offset.sol_dollars;
            adjusted.terra_dollars += offset.terra_dollars;
            adjusted.luna_dollars += offset.luna_dollars;
            adjusted.sol_tokens = adjusted
                .sol_tokens
                .checked_add(offset.sol_tokens)
                .ok_or(UsageStoreError::GenerationOverflow)?;
            adjusted.terra_tokens = adjusted
                .terra_tokens
                .checked_add(offset.terra_tokens)
                .ok_or(UsageStoreError::GenerationOverflow)?;
            adjusted.luna_tokens = adjusted
                .luna_tokens
                .checked_add(offset.luna_tokens)
                .ok_or(UsageStoreError::GenerationOverflow)?;
            adjusted.validate()?;
            Ok(adjusted)
        })
        .collect()
}

fn validate_migration_samples(samples: &[UsageHistorySample]) -> Result<()> {
    let mut keys = BTreeSet::new();
    for sample in samples {
        sample.validate()?;
        if !keys.insert((sample.reset_at, sample.timestamp)) {
            return Err(UsageStoreError::InvalidImport(
                "migration candidate contains duplicate history keys".into(),
            ));
        }
    }
    Ok(())
}

fn quick_check_database(path: &Path) -> Result<()> {
    let connection = Connection::open(path)?;
    let result: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if result != "ok" {
        return Err(UsageStoreError::InvalidImport(format!(
            "migration candidate quick_check failed: {result}"
        )));
    }
    Ok(())
}

fn validate_data_hash(data_hash: &str) -> Result<()> {
    if data_hash.len() != 64
        || !data_hash
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    {
        return Err(UsageStoreError::InvalidDurableRecord(
            "data_hash must be exactly 64 lowercase hexadecimal characters".into(),
        ));
    }
    Ok(())
}

fn validate_snapshot_json(snapshot_json: &str) -> Result<()> {
    if snapshot_json.len() <= MAX_SNAPSHOT_JSON_BYTES {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(snapshot_json) {
            if !value.is_object() {
                return Err(UsageStoreError::InvalidImport(
                    "snapshot_json must be a JSON object".into(),
                ));
            }
        }
    }
    if snapshot_json.len() > MAX_SNAPSHOT_JSON_BYTES {
        return Err(UsageStoreError::InvalidDurableRecord(format!(
            "snapshot_json exceeds {MAX_SNAPSHOT_JSON_BYTES} bytes"
        )));
    }
    serde_json::from_str::<serde_json::Value>(snapshot_json)
        .map_err(|error| UsageStoreError::InvalidDurableRecord(error.to_string()))?;
    Ok(())
}

fn observation_data_hash(reset_at: i64, timestamp: i64) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codex-info-usage-observation-v1\0");
    digest.update(reset_at.to_be_bytes());
    digest.update(timestamp.to_be_bytes());
    format!("{:x}", digest.finalize())
}

fn observation_json_value(observation: &UsageHistoryObservation) -> serde_json::Value {
    serde_json::json!({
        "kind": OBSERVATION_JSON_KIND,
        "timestamp": observation.timestamp,
        "reset_at": observation.reset_at,
        "remaining_percent": observation.remaining_percent,
        "sol_dollars": observation.sol_dollars,
        "terra_dollars": observation.terra_dollars,
        "luna_dollars": observation.luna_dollars,
        "sol_tokens": observation.sol_tokens,
        "terra_tokens": observation.terra_tokens,
        "luna_tokens": observation.luna_tokens,
        "model_source": observation.model_source.as_str(),
    })
}

fn observation_json(observation: &UsageHistoryObservation) -> Result<String> {
    observation.validate()?;
    let encoded = serde_json::to_string(&observation_json_value(observation)).map_err(|error| {
        UsageStoreError::InvalidDurableRecord(format!(
            "observation JSON serialization failed: {error}"
        ))
    })?;
    validate_observation_json(&encoded)?;
    Ok(encoded)
}

fn validate_observation_json(snapshot_json: &str) -> Result<serde_json::Value> {
    if snapshot_json.is_empty() || snapshot_json.len() > MAX_OBSERVATION_JSON_BYTES {
        return Err(UsageStoreError::InvalidDurableRecord(
            "observation snapshot_json is outside its bounded size".into(),
        ));
    }
    let value: serde_json::Value = serde_json::from_str(snapshot_json).map_err(|error| {
        UsageStoreError::InvalidDurableRecord(format!("observation JSON is invalid: {error}"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        UsageStoreError::InvalidDurableRecord("observation JSON must be an object".into())
    })?;
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = OBSERVATION_JSON_KEYS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(UsageStoreError::InvalidDurableRecord(
            "observation JSON fields differ from the strict contract".into(),
        ));
    }
    if object.get("kind").and_then(serde_json::Value::as_str) != Some(OBSERVATION_JSON_KIND) {
        return Err(UsageStoreError::InvalidDurableRecord(
            "observation JSON kind is invalid".into(),
        ));
    }
    Ok(value)
}

fn observation_from_sql(
    data_generation: i64,
    data_hash: String,
    snapshot_json: String,
) -> Result<UsageHistoryObservation> {
    if data_generation <= 0 {
        return Err(UsageStoreError::InvalidDurableRecord(
            "observation data_generation must be a positive timestamp".into(),
        ));
    }
    let expected_hash = observation_data_hash(
        validate_observation_json(&snapshot_json)?
            .get("reset_at")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                UsageStoreError::InvalidDurableRecord(
                    "observation reset_at is not an integer".into(),
                )
            })?,
        data_generation,
    );
    if data_hash != expected_hash {
        return Err(UsageStoreError::InvalidDurableRecord(
            "observation data_hash does not match its key".into(),
        ));
    }
    let value = validate_observation_json(&snapshot_json)?;
    let integer = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                UsageStoreError::InvalidDurableRecord(format!("observation {name} is invalid"))
            })
    };
    let optional_f64 = |name: &str| {
        let value = value.get(name).ok_or_else(|| {
            UsageStoreError::InvalidDurableRecord(format!("observation {name} is missing"))
        })?;
        if value.is_null() {
            Ok(None)
        } else {
            value.as_f64().map(Some).ok_or_else(|| {
                UsageStoreError::InvalidDurableRecord(format!("observation {name} is invalid"))
            })
        }
    };
    let optional_u64 = |name: &str| {
        let value = value.get(name).ok_or_else(|| {
            UsageStoreError::InvalidDurableRecord(format!("observation {name} is missing"))
        })?;
        if value.is_null() {
            Ok(None)
        } else {
            value.as_u64().map(Some).ok_or_else(|| {
                UsageStoreError::InvalidDurableRecord(format!("observation {name} is invalid"))
            })
        }
    };
    let timestamp = integer("timestamp")?;
    let reset_at = integer("reset_at")?;
    if timestamp != data_generation
        || value.get("timestamp").and_then(serde_json::Value::as_i64) != Some(timestamp)
        || value.get("reset_at").and_then(serde_json::Value::as_i64) != Some(reset_at)
    {
        return Err(UsageStoreError::InvalidDurableRecord(
            "observation key does not match its JSON".into(),
        ));
    }
    let model_source = value
        .get("model_source")
        .and_then(serde_json::Value::as_str)
        .and_then(ModelSource::parse)
        .ok_or_else(|| {
            UsageStoreError::InvalidDurableRecord("observation model_source is invalid".into())
        })?;
    let observation = UsageHistoryObservation {
        timestamp,
        reset_at,
        remaining_percent: optional_f64("remaining_percent")?,
        sol_dollars: optional_f64("sol_dollars")?,
        terra_dollars: optional_f64("terra_dollars")?,
        luna_dollars: optional_f64("luna_dollars")?,
        sol_tokens: optional_u64("sol_tokens")?,
        terra_tokens: optional_u64("terra_tokens")?,
        luna_tokens: optional_u64("luna_tokens")?,
        model_source,
        model_totals: None,
        model_totals_complete: false,
    };
    observation.validate()?;
    Ok(observation)
}

impl DurableRecord {
    fn validate(&self) -> Result<()> {
        validate_data_hash(&self.data_hash)?;
        validate_snapshot_json(&self.snapshot_json)
    }
}

fn durable_record_from_sql(
    data_generation: i64,
    data_hash: String,
    snapshot_json: String,
) -> Result<DurableRecord> {
    if data_generation < 0 {
        return Err(UsageStoreError::InvalidDurableRecord(
            "data_generation must not be negative".into(),
        ));
    }
    let record = DurableRecord {
        data_generation: data_generation as u64,
        data_hash,
        snapshot_json,
    };
    record.validate()?;
    Ok(record)
}

struct ResetPeriodAccumulator {
    min_reset_at: i64,
    canonical_id: i64,
    start_timestamp: i64,
}

fn build_reset_periods(samples: &[UsageHistorySample]) -> Vec<ResetPeriod> {
    let mut ordered = samples.to_vec();
    ordered.sort_by(|left, right| {
        left.reset_at
            .cmp(&right.reset_at)
            .then_with(|| left.timestamp.cmp(&right.timestamp))
    });

    let mut groups = Vec::<ResetPeriodAccumulator>::new();
    for sample in ordered {
        let Some(current) = groups.last_mut() else {
            groups.push(ResetPeriodAccumulator {
                min_reset_at: sample.reset_at,
                canonical_id: sample.reset_at,
                start_timestamp: sample.timestamp,
            });
            continue;
        };

        let reset_distance = i128::from(sample.reset_at) - i128::from(current.min_reset_at);
        if reset_distance <= RESET_GROUP_TOLERANCE_SECONDS {
            current.canonical_id = current.canonical_id.max(sample.reset_at);
            current.start_timestamp = current.start_timestamp.min(sample.timestamp);
        } else {
            groups.push(ResetPeriodAccumulator {
                min_reset_at: sample.reset_at,
                canonical_id: sample.reset_at,
                start_timestamp: sample.timestamp,
            });
        }
    }

    let mut periods = groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let end_timestamp = groups
                .get(index + 1)
                .map(|next| group.canonical_id.min(next.start_timestamp))
                .unwrap_or(group.canonical_id);
            ResetPeriod {
                canonical_id: group.canonical_id,
                start_timestamp: group.start_timestamp,
                end_timestamp,
            }
        })
        .collect::<Vec<_>>();
    periods.sort_by(|left, right| {
        right
            .start_timestamp
            .cmp(&left.start_timestamp)
            .then_with(|| right.canonical_id.cmp(&left.canonical_id))
    });
    periods
}

/// Groups samples by reset timestamps using only deterministic UTC epoch
/// values. Reset timestamps within sixty seconds of a group's first reset are
/// one period; sixty-one seconds starts a distinct period.
pub fn group_reset_periods(samples: &[UsageHistorySample]) -> Vec<ResetPeriod> {
    build_reset_periods(samples)
}

/// Persistent SQLite storage for minute-level usage samples.
pub struct UsageStore {
    connection: Connection,
}

fn validate_partition_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(UsageStoreError::InvalidImport(
            "partition database must be a regular file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err(UsageStoreError::InvalidImport(
                "partition database must be owner-private".into(),
            ));
        }
    }
    Ok(())
}

fn account_db_schema_version(connection: &Connection) -> Result<i64> {
    let version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !(0..=ACCOUNT_DB_SCHEMA_VERSION).contains(&version) {
        return Err(UsageStoreError::InvalidImport(
            "account partition schema version is unsupported".into(),
        ));
    }
    Ok(version)
}

fn stamp_current_account_db_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.pragma_update(None, "user_version", ACCOUNT_DB_SCHEMA_VERSION)?;
    Ok(())
}

fn validate_partition_schema(connection: &Connection, schema_version: i64) -> Result<()> {
    let allow_unversioned_legacy = schema_version == 0;
    type ColumnContract = (&'static str, &'static str, i64);
    type TableContract = (&'static str, &'static [ColumnContract]);
    const TABLES: &[TableContract] = &[
        (
            "usage_history",
            &[
                ("timestamp", "INTEGER", 2),
                ("reset_at", "INTEGER", 1),
                ("remaining_percent", "REAL", 0),
                ("sol_dollars", "REAL", 0),
                ("terra_dollars", "REAL", 0),
                ("luna_dollars", "REAL", 0),
                ("sol_tokens", "INTEGER", 0),
                ("terra_tokens", "INTEGER", 0),
                ("luna_tokens", "INTEGER", 0),
            ],
        ),
        (
            "durable_state",
            &[
                ("singleton", "INTEGER", 1),
                ("data_generation", "INTEGER", 0),
                ("data_hash", "TEXT", 0),
                ("snapshot_json", "TEXT", 0),
            ],
        ),
        (
            "recorded_sessions",
            &[
                ("root_identity", "TEXT", 1),
                ("relative_path", "TEXT", 2),
                ("file_bytes", "INTEGER", 3),
                ("modified_nanos", "TEXT", 4),
                ("file_device", "TEXT", 5),
                ("file_inode", "TEXT", 6),
            ],
        ),
        (
            "storage_partition",
            &[
                ("singleton", "INTEGER", 1),
                ("schema_version", "TEXT", 0),
                ("profile_scope_id", "TEXT", 0),
                ("account_scope_id", "TEXT", 0),
                ("storage_epoch", "TEXT", 0),
                ("partition_id", "TEXT", 0),
            ],
        ),
        (
            "collection_generation",
            &[
                ("singleton", "INTEGER", 1),
                ("data_generation", "TEXT", 0),
                ("reset_at", "INTEGER", 0),
                ("window_seconds", "INTEGER", 0),
                ("collector_epoch", "TEXT", 0),
                ("cycle_seq", "TEXT", 0),
            ],
        ),
        (
            "session_checkpoints",
            &[
                ("root_identity", "TEXT", 1),
                ("relative_path", "TEXT", 2),
                ("file_device", "TEXT", 3),
                ("file_inode", "TEXT", 4),
                ("committed_offset", "INTEGER", 0),
                ("discard_until_lf", "INTEGER", 0),
                ("collector_epoch", "TEXT", 0),
                ("cycle_seq", "TEXT", 0),
                ("prefix_generation", "TEXT", 5),
                ("prefix_sha256", "TEXT", 0),
                ("fully_attributed_from_zero", "INTEGER", 0),
                ("token_baseline_known", "INTEGER", 0),
                ("last_model", "TEXT", 0),
                ("previous_total", "TEXT", 0),
                ("previous_input", "TEXT", 0),
                ("previous_cached_input", "TEXT", 0),
                ("previous_output", "TEXT", 0),
                ("last_task_running", "INTEGER", 0),
                ("previous_cache_write_input", "TEXT", 0),
            ],
        ),
        (
            "session_ranges",
            &[
                ("root_identity", "TEXT", 1),
                ("relative_path", "TEXT", 2),
                ("file_device", "TEXT", 3),
                ("file_inode", "TEXT", 4),
                ("start_offset", "INTEGER", 6),
                ("end_offset", "INTEGER", 7),
                ("collector_epoch", "TEXT", 0),
                ("cycle_seq", "TEXT", 0),
                ("prefix_generation", "TEXT", 5),
                ("record_sha256", "TEXT", 8),
            ],
        ),
        (
            "session_model_totals",
            &[
                ("model", "TEXT", 1),
                ("total_tokens", "TEXT", 0),
                ("input_tokens", "TEXT", 0),
                ("cached_input_tokens", "TEXT", 0),
                ("output_tokens", "TEXT", 0),
                ("cache_write_input_tokens", "TEXT", 0),
            ],
        ),
        (
            "usage_model_history",
            &[
                ("reset_at", "INTEGER", 1),
                ("timestamp", "INTEGER", 2),
                ("model", "TEXT", 3),
                ("total_tokens", "TEXT", 0),
                ("input_tokens", "TEXT", 0),
                ("cached_input_tokens", "TEXT", 0),
                ("output_tokens", "TEXT", 0),
                ("cache_write_input_tokens", "TEXT", 0),
                ("model_set_complete", "INTEGER", 0),
            ],
        ),
        (
            "history_continuity",
            &[
                ("singleton", "INTEGER", 1),
                ("source_fingerprint", "TEXT", 0),
                ("source_rows", "INTEGER", 0),
                ("boundary_timestamp", "INTEGER", 0),
                ("reset_at", "INTEGER", 0),
                ("remaining_percent", "REAL", 0),
                ("sol_dollars", "REAL", 0),
                ("terra_dollars", "REAL", 0),
                ("luna_dollars", "REAL", 0),
                ("sol_tokens", "TEXT", 0),
                ("terra_tokens", "TEXT", 0),
                ("luna_tokens", "TEXT", 0),
                ("model_totals_applied", "INTEGER", 0),
            ],
        ),
        (
            "recorder_gap_ledger",
            &[
                ("gap_id", "TEXT", 1),
                ("partition_id", "TEXT", 0),
                ("source_identity_before", "TEXT", 0),
                ("source_identity_after", "TEXT", 0),
                ("cursor_before", "TEXT", 0),
                ("cursor_after", "TEXT", 0),
                ("stopped_at_monotonic_ns", "INTEGER", 0),
                ("resumed_at_monotonic_ns", "INTEGER", 0),
                ("start_at", "INTEGER", 0),
                ("end_at", "INTEGER", 0),
                ("reset_at", "INTEGER", 0),
                ("reason", "TEXT", 0),
                ("state", "TEXT", 0),
                ("owner_collector_epoch", "TEXT", 0),
                ("confirmation_cycle_seq", "TEXT", 0),
            ],
        ),
    ];

    let mut table_statement = connection.prepare(
        "SELECT name FROM sqlite_schema
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let actual_tables = table_statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    let expected_tables = TABLES
        .iter()
        .map(|(table, _)| (*table).to_owned())
        .collect::<BTreeSet<_>>();
    let mut pre_continuity_tables = expected_tables.clone();
    pre_continuity_tables.remove("history_continuity");
    pre_continuity_tables.remove("usage_model_history");
    let mut pre_model_history_tables = expected_tables.clone();
    pre_model_history_tables.remove("usage_model_history");
    if actual_tables != expected_tables
        && !(allow_unversioned_legacy && actual_tables == pre_continuity_tables)
        && !(schema_version < 2 && actual_tables == pre_model_history_tables)
    {
        return Err(UsageStoreError::InvalidImport(
            "account partition table set mismatch".into(),
        ));
    }

    for (table, expected) in TABLES {
        if (*table == "history_continuity" || *table == "usage_model_history")
            && !actual_tables.contains(*table)
        {
            continue;
        }
        let mut statement = connection.prepare(&format!(
            "SELECT name, type, pk FROM pragma_table_info('{table}') ORDER BY cid"
        ))?;
        let actual = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let expected = expected
            .iter()
            .map(|(name, kind, pk)| ((*name).to_owned(), (*kind).to_owned(), *pk))
            .collect::<Vec<_>>();
        let legacy_history_continuity = *table == "history_continuity"
            && actual.len() + 1 == expected.len()
            && actual == expected[..actual.len()];
        let legacy_cache_write_columns =
            matches!(*table, "session_checkpoints" | "session_model_totals")
                && actual.len() + 1 == expected.len()
                && actual == expected[..actual.len()];
        if actual != expected
            && !(allow_unversioned_legacy
                && ((*table == "recorder_gap_ledger"
                    && actual == legacy_recorder_gap_ledger_columns())
                    || (*table == "session_checkpoints"
                        && actual == legacy_session_checkpoint_columns())
                    || legacy_history_continuity
                    || legacy_cache_write_columns))
        {
            return Err(UsageStoreError::InvalidImport(format!(
                "account partition {table} schema mismatch"
            )));
        }
    }
    Ok(())
}

fn legacy_session_checkpoint_columns() -> Vec<(String, String, i64)> {
    vec![
        ("root_identity".to_owned(), "TEXT".to_owned(), 1),
        ("relative_path".to_owned(), "TEXT".to_owned(), 2),
        ("file_device".to_owned(), "TEXT".to_owned(), 3),
        ("file_inode".to_owned(), "TEXT".to_owned(), 4),
        ("committed_offset".to_owned(), "INTEGER".to_owned(), 0),
        ("discard_until_lf".to_owned(), "INTEGER".to_owned(), 0),
        ("collector_epoch".to_owned(), "TEXT".to_owned(), 0),
        ("cycle_seq".to_owned(), "TEXT".to_owned(), 0),
        ("prefix_generation".to_owned(), "TEXT".to_owned(), 5),
        ("prefix_sha256".to_owned(), "TEXT".to_owned(), 0),
        (
            "fully_attributed_from_zero".to_owned(),
            "INTEGER".to_owned(),
            0,
        ),
        ("token_baseline_known".to_owned(), "INTEGER".to_owned(), 0),
        ("last_model".to_owned(), "TEXT".to_owned(), 0),
        ("previous_total".to_owned(), "TEXT".to_owned(), 0),
        ("previous_input".to_owned(), "TEXT".to_owned(), 0),
        ("previous_cached_input".to_owned(), "TEXT".to_owned(), 0),
        ("previous_output".to_owned(), "TEXT".to_owned(), 0),
    ]
}

fn recorder_gap_ledger_columns(connection: &Connection) -> Result<Vec<(String, String, i64)>> {
    let mut statement = connection.prepare(
        "SELECT name, type, pk FROM pragma_table_info('recorder_gap_ledger') ORDER BY cid",
    )?;
    let columns = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into);
    columns
}

fn ensure_history_continuity_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS history_continuity (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            source_fingerprint TEXT NOT NULL CHECK (
                length(source_fingerprint) = 16
                AND source_fingerprint NOT GLOB '*[^0-9a-f]*'
            ),
            source_rows INTEGER NOT NULL CHECK (source_rows > 0),
            boundary_timestamp INTEGER NOT NULL CHECK (boundary_timestamp > 0),
            reset_at INTEGER NOT NULL CHECK (reset_at > 0),
            remaining_percent REAL NOT NULL CHECK (
                remaining_percent >= 0.0 AND remaining_percent <= 100.0
            ),
            sol_dollars REAL NOT NULL CHECK (sol_dollars >= 0.0),
            terra_dollars REAL NOT NULL CHECK (terra_dollars >= 0.0),
            luna_dollars REAL NOT NULL CHECK (luna_dollars >= 0.0),
            sol_tokens TEXT NOT NULL,
            terra_tokens TEXT NOT NULL,
            luna_tokens TEXT NOT NULL,
            model_totals_applied INTEGER NOT NULL DEFAULT 0 CHECK (
                model_totals_applied IN (0, 1)
            )
        );
        "#,
    )?;
    let applied_column_present: bool = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('history_continuity')
            WHERE name = 'model_totals_applied'
        )",
        [],
        |row| row.get(0),
    )?;
    if !applied_column_present {
        transaction.execute(
            "ALTER TABLE history_continuity
             ADD COLUMN model_totals_applied INTEGER NOT NULL DEFAULT 0
             CHECK (model_totals_applied IN (0, 1))",
            [],
        )?;
    }
    Ok(())
}

fn ensure_usage_model_history_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS usage_model_history (
            reset_at INTEGER NOT NULL CHECK (reset_at > 0),
            timestamp INTEGER NOT NULL CHECK (timestamp > 0),
            model TEXT NOT NULL CHECK (length(model) BETWEEN 1 AND 512),
            total_tokens TEXT NOT NULL,
            input_tokens TEXT NOT NULL,
            cached_input_tokens TEXT NOT NULL,
            output_tokens TEXT NOT NULL,
            cache_write_input_tokens TEXT,
            model_set_complete INTEGER NOT NULL CHECK (model_set_complete IN (0, 1)),
            PRIMARY KEY (reset_at, timestamp, model)
        ) WITHOUT ROWID;
        "#,
    )?;
    Ok(())
}

/// Adds the nullable task-state column to an existing account partition.
/// NULL is intentional: older checkpoints do not carry task lifecycle state.
fn session_checkpoint_running_column_present(connection: &Connection) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_checkpoints') WHERE name = 'last_task_running')",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn ensure_session_checkpoint_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    if !session_checkpoint_running_column_present(transaction)? {
        transaction.execute(
            "ALTER TABLE session_checkpoints ADD COLUMN last_task_running INTEGER
             CHECK (last_task_running IS NULL OR last_task_running IN (0, 1))",
            [],
        )?;
    }
    for (table, column) in [
        ("session_checkpoints", "previous_cache_write_input"),
        ("session_model_totals", "cache_write_input_tokens"),
    ] {
        if !cache_write_column_present(transaction, table, column)? {
            transaction.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} TEXT"), [])?;
        }
    }
    Ok(())
}

fn cache_write_column_present(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
            params![table, column],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn legacy_recorder_gap_ledger_columns() -> Vec<(String, String, i64)> {
    vec![
        ("data_generation".to_owned(), "TEXT".to_owned(), 1),
        ("observed_at".to_owned(), "INTEGER".to_owned(), 0),
        ("reason".to_owned(), "TEXT".to_owned(), 0),
    ]
}

fn create_recorder_gap_ledger(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        r#"
        CREATE TABLE recorder_gap_ledger (
            gap_id TEXT PRIMARY KEY CHECK (
                length(gap_id) = 32 AND gap_id NOT GLOB '*[^0-9a-f]*'
            ),
            partition_id TEXT NOT NULL CHECK (
                length(partition_id) = 64 AND partition_id NOT GLOB '*[^0-9a-f]*'
            ),
            source_identity_before TEXT NOT NULL CHECK (length(source_identity_before) BETWEEN 1 AND 512),
            source_identity_after TEXT NOT NULL CHECK (length(source_identity_after) BETWEEN 1 AND 512),
            cursor_before TEXT NOT NULL CHECK (length(cursor_before) BETWEEN 1 AND 512),
            cursor_after TEXT NOT NULL CHECK (length(cursor_after) BETWEEN 1 AND 512),
            stopped_at_monotonic_ns INTEGER NOT NULL CHECK (stopped_at_monotonic_ns > 0),
            resumed_at_monotonic_ns INTEGER CHECK (
                resumed_at_monotonic_ns IS NULL OR resumed_at_monotonic_ns >= stopped_at_monotonic_ns
            ),
            start_at INTEGER NOT NULL CHECK (start_at > 0),
            end_at INTEGER NOT NULL CHECK (end_at >= start_at),
            reset_at INTEGER CHECK (reset_at IS NULL OR reset_at > 0),
            reason TEXT NOT NULL CHECK (
                reason IN ('daemon_stop_unrecoverable', 'reset_hint_expired', 'auth_epoch_tombstoned')
            ),
            state TEXT NOT NULL CHECK (
                state IN ('pending', 'confirmed', 'recovered', 'rejected')
            ),
            owner_collector_epoch TEXT NOT NULL CHECK (
                length(owner_collector_epoch) = 32
                AND owner_collector_epoch NOT GLOB '*[^0-9a-f]*'
            ),
            confirmation_cycle_seq TEXT NOT NULL CHECK (
                length(confirmation_cycle_seq) BETWEEN 1 AND 20
                AND confirmation_cycle_seq NOT GLOB '*[^0-9]*'
            )
        );
        "#,
    )?;
    Ok(())
}

fn legacy_gap_id(data_generation: &str, observed_at: i64, reason: &str, rowid: i64) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data_generation.hash(&mut hasher);
    observed_at.hash(&mut hasher);
    reason.hash(&mut hasher);
    rowid.hash(&mut hasher);
    format!("{:032x}", hasher.finish())
}

/// Upgrade the fixture-era three-column ledger without deleting its rows.
/// Legacy observations have no source proof, so they remain visible only as
/// rejected records; no point-in-time quota interval is fabricated.
fn ensure_recorder_gap_ledger_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let table_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'recorder_gap_ledger')",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return create_recorder_gap_ledger(transaction);
    }

    let actual = recorder_gap_ledger_columns(transaction)?;
    let expected = vec![
        ("gap_id".to_owned(), "TEXT".to_owned(), 1),
        ("partition_id".to_owned(), "TEXT".to_owned(), 0),
        ("source_identity_before".to_owned(), "TEXT".to_owned(), 0),
        ("source_identity_after".to_owned(), "TEXT".to_owned(), 0),
        ("cursor_before".to_owned(), "TEXT".to_owned(), 0),
        ("cursor_after".to_owned(), "TEXT".to_owned(), 0),
        (
            "stopped_at_monotonic_ns".to_owned(),
            "INTEGER".to_owned(),
            0,
        ),
        (
            "resumed_at_monotonic_ns".to_owned(),
            "INTEGER".to_owned(),
            0,
        ),
        ("start_at".to_owned(), "INTEGER".to_owned(), 0),
        ("end_at".to_owned(), "INTEGER".to_owned(), 0),
        ("reset_at".to_owned(), "INTEGER".to_owned(), 0),
        ("reason".to_owned(), "TEXT".to_owned(), 0),
        ("state".to_owned(), "TEXT".to_owned(), 0),
        ("owner_collector_epoch".to_owned(), "TEXT".to_owned(), 0),
        ("confirmation_cycle_seq".to_owned(), "TEXT".to_owned(), 0),
    ];
    if actual == expected {
        return Ok(());
    }

    let legacy = legacy_recorder_gap_ledger_columns();
    if actual != legacy {
        return Err(UsageStoreError::InvalidImport(
            "recorder gap ledger schema mismatch".into(),
        ));
    }

    let partition_id: String = transaction
        .query_row(
            "SELECT partition_id FROM storage_partition WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "0".repeat(64));
    transaction.execute(
        "ALTER TABLE recorder_gap_ledger RENAME TO recorder_gap_ledger_legacy",
        [],
    )?;
    create_recorder_gap_ledger(transaction)?;
    let mut statement = transaction.prepare(
        "SELECT rowid, data_generation, observed_at, reason
         FROM recorder_gap_ledger_legacy ORDER BY rowid",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for (rowid, data_generation, observed_at, legacy_reason) in rows {
        // The fixture-era table did not constrain its timestamp. Preserve
        // every legacy row as a rejected record, using the smallest valid
        // sentinel for an invalid timestamp; no such row can be projected as
        // a public quota gap without a later source-proof transition.
        let legacy_timestamp = observed_at.max(1);
        let gap_id = legacy_gap_id(&data_generation, observed_at, "legacy", rowid);
        // Legacy fixture generations were unconstrained text.  Keep a safe,
        // bounded cursor when an old value cannot satisfy the new ledger's
        // printable-text contract; this row remains rejected and therefore
        // cannot become a fabricated public quota gap.
        let legacy_cursor = if data_generation.len() <= RECORDER_GAP_TEXT_BYTES
            && !data_generation.is_empty()
            && data_generation.is_ascii()
            && !data_generation.bytes().any(|byte| byte.is_ascii_control())
        {
            data_generation
        } else {
            format!("legacy-{gap_id}")
        };
        let migrated_reason = GAP_LEDGER_REASONS
            .contains(&legacy_reason.as_str())
            .then_some(legacy_reason.as_str())
            .unwrap_or("auth_epoch_tombstoned");
        transaction.execute(
            "INSERT INTO recorder_gap_ledger (
                gap_id, partition_id, source_identity_before, source_identity_after,
                cursor_before, cursor_after, stopped_at_monotonic_ns,
                resumed_at_monotonic_ns, start_at, end_at, reset_at, reason, state,
                owner_collector_epoch, confirmation_cycle_seq
             ) VALUES (?1, ?2, 'legacy', 'legacy', ?3, ?3, ?4, NULL, ?5, ?5, ?5,
                       ?6, 'rejected', ?7, '1')",
            params![
                gap_id,
                &partition_id,
                &legacy_cursor,
                legacy_timestamp,
                legacy_timestamp,
                migrated_reason,
                format!("{:032x}", 1_u128),
            ],
        )?;
    }
    transaction.execute("DROP TABLE recorder_gap_ledger_legacy", [])?;
    Ok(())
}

fn validate_storage_partition(
    connection: &Connection,
    expected: &StoragePartitionIdentity,
) -> Result<()> {
    let quick_check: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick_check != "ok" {
        return Err(UsageStoreError::InvalidImport(
            "account partition quick_check failed".into(),
        ));
    }
    validate_storage_partition_metadata(connection, expected)
}

fn validate_storage_partition_metadata(
    connection: &Connection,
    expected: &StoragePartitionIdentity,
) -> Result<()> {
    expected.validate()?;
    let schema_version = account_db_schema_version(connection)?;
    validate_partition_schema(connection, schema_version)?;
    let table_type: Option<String> = connection
        .query_row(
            "SELECT type FROM sqlite_master WHERE name = 'storage_partition'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if table_type.as_deref() != Some("table") {
        return Err(UsageStoreError::InvalidImport(
            "storage partition table is missing".into(),
        ));
    }
    let count: i64 = connection.query_row("SELECT COUNT(*) FROM storage_partition", [], |row| {
        row.get(0)
    })?;
    if count != 1 {
        return Err(UsageStoreError::InvalidImport(
            "storage partition row cardinality mismatch".into(),
        ));
    }
    let actual: (i64, String, String, String, String, String) = connection.query_row(
        "SELECT singleton, schema_version, profile_scope_id, account_scope_id, \
                storage_epoch, partition_id FROM storage_partition",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;
    let actual_epoch = canonical_u64_text(&actual.4, "storage epoch")?;
    if actual
        != (
            1,
            expected.schema_version.clone(),
            expected.profile_scope_id.clone(),
            expected.account_scope_id.clone(),
            expected.storage_epoch.to_string(),
            expected.partition_id.clone(),
        )
        || actual_epoch != expected.storage_epoch
    {
        return Err(UsageStoreError::InvalidImport(
            "storage partition identity mismatch".into(),
        ));
    }
    Ok(())
}

/// Read only the partition identity before a compatible schema transition.
/// This preserves the fail-closed account boundary while still allowing the
/// recorder owner to upgrade the old fixture-era gap table transactionally.
fn validate_storage_partition_identity(
    connection: &Connection,
    expected: &StoragePartitionIdentity,
) -> Result<()> {
    expected.validate()?;
    account_db_schema_version(connection)?;
    let actual: (i64, String, String, String, String, String) = connection.query_row(
        "SELECT singleton, schema_version, profile_scope_id, account_scope_id,
                storage_epoch, partition_id FROM storage_partition",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;
    let actual_epoch = canonical_u64_text(&actual.4, "storage epoch")?;
    if actual.0 != 1
        || actual.1 != expected.schema_version
        || actual.2 != expected.profile_scope_id
        || actual.3 != expected.account_scope_id
        || actual.5 != expected.partition_id
        || actual_epoch != expected.storage_epoch
    {
        return Err(UsageStoreError::InvalidImport(
            "storage partition identity mismatch".into(),
        ));
    }
    Ok(())
}

/// Upgrade the old singleton-only durable-state CHECK without changing the
/// table/column contract.  The migration runs inside the caller's transaction
/// and is deliberately placed before partition schema validation so a legacy
/// account database is never half-opened or partially inspected.
fn ensure_durable_state_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let sql: String = transaction.query_row(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'durable_state'",
        [],
        |row| row.get(0),
    )?;
    let normalized = sql
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    if normalized.contains("singleton>=1") {
        transaction.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS durable_state_observation_key_idx
             ON durable_state (data_hash) WHERE singleton >= 2",
            [],
        )?;
        transaction.execute(
            "CREATE INDEX IF NOT EXISTS durable_state_observation_time_idx
             ON durable_state (data_generation, singleton) WHERE singleton >= 2",
            [],
        )?;
        return Ok(());
    }
    if !normalized.contains("singleton=1") {
        return Err(UsageStoreError::InvalidImport(
            "durable_state singleton CHECK is not recognized".into(),
        ));
    }

    transaction.execute(
        "ALTER TABLE durable_state RENAME TO durable_state_legacy",
        [],
    )?;
    transaction.execute_batch(
        "CREATE TABLE durable_state (
            singleton INTEGER PRIMARY KEY CHECK (singleton >= 1),
            data_generation INTEGER NOT NULL CHECK (data_generation >= 0),
            data_hash TEXT NOT NULL,
            snapshot_json TEXT NOT NULL
        );",
    )?;
    // A legacy row may already violate its CHECK because of earlier storage
    // corruption. Preserve that evidence during the shape-only migration so
    // `load_durable_state` can report it without making the whole usage store
    // unopenable (which would stop subsequent recording).
    transaction.execute_batch("PRAGMA ignore_check_constraints = ON;")?;
    let copy_result = transaction.execute(
        "INSERT INTO durable_state (singleton, data_generation, data_hash, snapshot_json)
         SELECT singleton, data_generation, data_hash, snapshot_json
         FROM durable_state_legacy",
        [],
    );
    let restore_result = transaction.execute_batch("PRAGMA ignore_check_constraints = OFF;");
    copy_result?;
    restore_result?;
    transaction.execute("DROP TABLE durable_state_legacy", [])?;
    transaction.execute(
        "CREATE UNIQUE INDEX durable_state_observation_key_idx
         ON durable_state (data_hash) WHERE singleton >= 2",
        [],
    )?;
    transaction.execute(
        "CREATE INDEX durable_state_observation_time_idx
         ON durable_state (data_generation, singleton) WHERE singleton >= 2",
        [],
    )?;
    Ok(())
}

#[allow(dead_code)]
impl UsageStore {
    /// Creates a brand-new account partition. Existing paths are recovery
    /// evidence and are never opened or replaced by this constructor.
    pub fn create_partitioned<P: AsRef<Path>>(
        path: P,
        identity: &StoragePartitionIdentity,
    ) -> Result<Self> {
        identity.validate()?;
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(UsageStoreError::InvalidImport(
                "partition database path must be absolute".into(),
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            UsageStoreError::InvalidImport("partition database parent is missing".into())
        })?;
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        let mut options = OpenOptions::new();
        options.write(true).read(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)?;
        validate_partition_file(path)?;

        let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(SCHEMA)?;
        transaction.execute_batch(PARTITION_SCHEMA)?;
        ensure_durable_state_schema(&transaction)?;
        ensure_recorder_gap_ledger_schema(&transaction)?;
        ensure_session_checkpoint_schema(&transaction)?;
        ensure_history_continuity_schema(&transaction)?;
        ensure_usage_model_history_schema(&transaction)?;
        stamp_current_account_db_schema(&transaction)?;
        transaction.execute(
            "INSERT INTO storage_partition (
                singleton, schema_version, profile_scope_id, account_scope_id,
                storage_epoch, partition_id
            ) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![
                &identity.schema_version,
                &identity.profile_scope_id,
                &identity.account_scope_id,
                identity.storage_epoch.to_string(),
                &identity.partition_id,
            ],
        )?;
        validate_recorded_sessions_schema(&transaction)?;
        Self::ensure_recent_history_covering_index(&transaction)?;
        validate_storage_partition(&transaction, identity)?;
        transaction.commit()?;
        Ok(Self { connection })
    }

    /// Opens an initialized account partition only after its durable identity
    /// matches. A legacy root DB or another account's DB is rejected before
    /// any schema or data mutation can occur.
    pub fn open_partitioned<P: AsRef<Path>>(
        path: P,
        identity: &StoragePartitionIdentity,
    ) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(UsageStoreError::InvalidImport(
                "partition database path must be absolute".into(),
            ));
        }
        validate_partition_file(path)?;
        let probe = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // Check the account identity before opening a writable connection. A
        // database belonging to another account must never be migrated merely
        // because it happens to contain the legacy fixture ledger.
        validate_storage_partition_identity(&probe, identity)?;
        drop(probe);
        let mut store = Self::open(path)?;
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_recorder_gap_ledger_schema(&transaction)?;
        ensure_session_checkpoint_schema(&transaction)?;
        ensure_history_continuity_schema(&transaction)?;
        ensure_usage_model_history_schema(&transaction)?;
        stamp_current_account_db_schema(&transaction)?;
        validate_storage_partition(&transaction, identity)?;
        transaction.commit()?;
        Ok(store)
    }

    /// Read-only counterpart of [`Self::open_partitioned`].
    pub fn open_read_only_partitioned<P: AsRef<Path>>(
        path: P,
        identity: &StoragePartitionIdentity,
    ) -> Result<Self> {
        let path = path.as_ref();
        validate_partition_file(path)?;
        let store = Self::open_read_only(path)?;
        // The serialized writer performs one full quick_check when the
        // partition is activated. Resident readers reopen only to obtain a
        // consistent SQLite snapshot, so repeating an O(database-size)
        // integrity scan on every minute poll is not an access check.
        validate_storage_partition_metadata(&store.connection, identity)?;
        Ok(store)
    }

    /// Performs the full SQLite integrity proof only when a retained backup
    /// is selected as recovery authority. Steady-state readers intentionally
    /// use the cheaper schema/identity validation above.
    pub fn verify_integrity(&self) -> Result<()> {
        let quick_check: String = self
            .connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if quick_check != "ok" {
            return Err(UsageStoreError::InvalidImport(
                "account partition quick_check failed".into(),
            ));
        }
        Ok(())
    }

    /// Creates and rotates backups only for one already-verified partition.
    pub fn backup_generations_partitioned<P: AsRef<Path>>(
        path: P,
        identity: &StoragePartitionIdentity,
        generations: usize,
    ) -> Result<()> {
        let path = path.as_ref();
        let source = Self::open_read_only_partitioned(path, identity)?;
        drop(source);
        // `backup_generations` validates the live source, creates one
        // SQLite-consistent copy, and quick-checks that copy before rotation.
        // Retained generations are cold recovery inputs: scanning every one
        // both before and after every daemon restart duplicates that proof and
        // can delay the recorder beyond its activation deadline. Validate a
        // retained generation when it is actually selected for recovery.
        Self::backup_generations(path, generations)?;
        Ok(())
    }

    /// Opens `path`, creating its parent directories and schema as needed.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(UsageStoreError::InvalidImport(
                "database path must be absolute".into(),
            ));
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
                }
            }
        }

        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(UsageStoreError::InvalidImport(
                    "database path must be a regular file".into(),
                ));
            }
        } else {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(path)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }

        let mut connection = Connection::open(path)?;
        // Multiple Codex Info instances are allowed to observe the same
        // history DB. SQLite remains the serialization authority; a bounded
        // busy timeout prevents a transient writer collision from discarding
        // an otherwise valid batch.
        connection.busy_timeout(Duration::from_secs(2))?;
        let transaction = connection.transaction()?;
        transaction.execute_batch(SCHEMA)?;
        ensure_durable_state_schema(&transaction)?;
        // A database must already have the current schema. Older formats are
        // intentionally not migrated or read.
        for column in ["sol_tokens", "terra_tokens", "luna_tokens"] {
            let present: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_history') WHERE name = ?1)",
                [column],
                |row| row.get(0),
            )?;
            if !present {
                return Err(UsageStoreError::InvalidImport(
                    "database schema mismatch".into(),
                ));
            }
        }
        validate_recorded_sessions_schema(&transaction)?;
        Self::ensure_recent_history_covering_index(&transaction)?;
        transaction.commit()?;
        Ok(Self { connection })
    }

    /// Opens an existing current-schema database without creating files,
    /// changing permissions, running schema DDL, or repairing indexes.
    /// Resident presentation assemblers use this path so the recorder remains
    /// the sole durable writer.
    pub fn open_read_only<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(UsageStoreError::InvalidImport(
                "database path must be absolute".into(),
            ));
        }
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(UsageStoreError::InvalidImport(
                "database path must be a regular file".into(),
            ));
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        for column in ["sol_tokens", "terra_tokens", "luna_tokens"] {
            let present: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_history') WHERE name = ?1)",
                [column],
                |row| row.get(0),
            )?;
            if !present {
                return Err(UsageStoreError::InvalidImport(
                    "database schema mismatch".into(),
                ));
            }
        }
        Ok(Self { connection })
    }

    /// Keeps the bounded recent-history read covered even for databases that
    /// were created before the index included the projected value columns.
    /// Only the index is replaced; rows and the primary-key schema are never
    /// mutated. The replacement is part of the open transaction, so a failed
    /// rebuild rolls back without leaving a partially upgraded index.
    fn ensure_recent_history_covering_index(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
        let metadata = transaction
            .query_row(
                "SELECT \"unique\", origin, partial \
                 FROM pragma_index_list('usage_history') WHERE name = ?1",
                [HISTORY_TIMESTAMP_RESET_INDEX],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let mut statement =
            transaction.prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno ASC")?;
        let actual = statement
            .query_map([HISTORY_TIMESTAMP_RESET_INDEX], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if metadata == Some((0, "c".to_owned(), 0))
            && actual
                == HISTORY_TIMESTAMP_RESET_INDEX_COLUMNS
                    .iter()
                    .map(|column| (*column).to_owned())
                    .collect::<Vec<_>>()
        {
            return Ok(());
        }

        transaction.execute("DROP INDEX IF EXISTS usage_history_timestamp_reset_idx", [])?;
        transaction.execute(
            "CREATE INDEX usage_history_timestamp_reset_idx ON usage_history (
                timestamp,
                reset_at,
                remaining_percent,
                sol_dollars,
                terra_dollars,
                luna_dollars,
                sol_tokens,
                terra_tokens,
                luna_tokens
            )",
            [],
        )?;
        Ok(())
    }

    /// Alias for callers that prefer constructor-style naming.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open(path)
    }

    /// Create bounded, SQLite-consistent backup generations before a
    /// destructive maintenance operation. The source is never replaced; a
    /// failed backup leaves all existing generations untouched. Rotation is
    /// staged inside the same directory and rolled back if any rename fails;
    /// this matters because a backup failure must not silently consume the
    /// only older generation that could be used for manual recovery.
    pub fn backup_generations<P: AsRef<Path>>(path: P, generations: usize) -> Result<()> {
        let path = path.as_ref();
        if generations == 0 {
            return Ok(());
        }
        if !path.is_absolute() {
            return Err(UsageStoreError::InvalidImport(
                "database backup path must be absolute".into(),
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            UsageStoreError::InvalidImport("database backup parent is missing".into())
        })?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| UsageStoreError::InvalidImport("database filename is invalid".into()))?;

        // Validate the source without changing it before creating any
        // temporary or generation file. This rejects corrupt/old-schema input
        // at the read boundary and leaves every existing generation intact.
        let source_store = Self::open(path)?;
        drop(source_store);
        let source = Connection::open(path)?;
        source.busy_timeout(Duration::from_secs(2))?;
        let source_check: String = source.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if source_check != "ok" {
            return Err(UsageStoreError::InvalidImport(format!(
                "source database quick_check failed: {source_check}"
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::create_dir_all(parent)?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }

        let counter = BACKUP_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{file_name}.backup.tmp-{}-{counter}",
            std::process::id(),
        ));
        if fs::symlink_metadata(&temporary).is_ok() {
            return Err(UsageStoreError::InvalidImport(
                "stale backup temporary exists; inspect before retry".into(),
            ));
        }
        let backup_result = source.backup(DatabaseName::Main, &temporary, None);
        if let Err(error) = backup_result {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(error) = fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)) {
                let _ = fs::remove_file(&temporary);
                return Err(error.into());
            }
        }
        if let Err(error) = quick_check_database(&temporary) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        drop(source);

        let stage_prefix = format!(
            ".{file_name}.backup-rotate-{}-{counter}",
            std::process::id()
        );
        let mut staged = Vec::with_capacity(generations);
        for generation in 1..=generations {
            let final_path = path.with_extension(format!("sqlite3.bak.{generation}"));
            let stage_path = parent.join(format!("{stage_prefix}-{generation}"));
            if fs::symlink_metadata(&stage_path).is_ok() {
                let _ = fs::remove_file(&temporary);
                return Err(UsageStoreError::InvalidImport(
                    "stale backup rotation file exists; inspect before retry".into(),
                ));
            }
            if let Ok(metadata) = fs::symlink_metadata(&final_path) {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    let _ = fs::remove_file(&temporary);
                    return Err(UsageStoreError::InvalidImport(
                        "backup generation is not a regular file".into(),
                    ));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o077 != 0 {
                        let _ = fs::remove_file(&temporary);
                        return Err(UsageStoreError::InvalidImport(
                            "backup generation is not private".into(),
                        ));
                    }
                }
                staged.push((final_path, stage_path));
            }
        }

        // Move every existing generation out of the way first. Since each
        // destination is now empty, a later failure can restore the exact
        // original names without overwriting a racing file.
        let mut moved = Vec::new();
        let rotation_result = (|| -> Result<()> {
            for (final_path, stage_path) in &staged {
                fs::rename(final_path, stage_path)?;
                moved.push((final_path.clone(), stage_path.clone()));
            }
            let first = path.with_extension("sqlite3.bak.1");
            fs::rename(&temporary, &first)?;
            for generation in (2..=generations).rev() {
                let source_stage = parent.join(format!("{stage_prefix}-{}", generation - 1));
                if fs::symlink_metadata(&source_stage).is_ok() {
                    let destination = path.with_extension(format!("sqlite3.bak.{generation}"));
                    fs::rename(&source_stage, destination)?;
                }
            }
            // The oldest generation is intentionally discarded only after
            // every retained generation has been installed. Failure to remove
            // this private staging file is non-fatal; it is not an advertised
            // generation and can be inspected/cleaned on the next run.
            let oldest_stage = parent.join(format!("{stage_prefix}-{generations}"));
            let _ = fs::remove_file(oldest_stage);
            Ok(())
        })();

        if let Err(error) = rotation_result {
            // Restore installed generations to their staging names. The new
            // generation is moved back to its temporary name so the caller
            // never observes a partially rotated set on a failed operation.
            let first = path.with_extension("sqlite3.bak.1");
            if fs::symlink_metadata(&first).is_ok() {
                let _ = fs::rename(&first, &temporary);
            }
            for generation in 2..=generations {
                let destination = path.with_extension(format!("sqlite3.bak.{generation}"));
                let source_stage = parent.join(format!("{stage_prefix}-{}", generation - 1));
                if fs::symlink_metadata(&destination).is_ok() {
                    let _ = fs::rename(destination, source_stage);
                }
            }
            for (final_path, stage_path) in moved.into_iter().rev() {
                if fs::symlink_metadata(&stage_path).is_ok() {
                    let _ = fs::rename(stage_path, final_path);
                }
            }
            let _ = fs::remove_file(&temporary);
            for (_, stage_path) in staged {
                let _ = fs::remove_file(stage_path);
            }
            return Err(error);
        }

        Ok(())
    }

    /// Migrate through a separately validated candidate database.
    ///
    /// The caller supplies an explicit transformation, so no schema or row
    /// value is guessed implicitly. The source remains untouched until the
    /// candidate has passed validation, quick_check, row/fingerprint equality
    /// and reset-period boundary comparison. The old file is retained beside
    /// the new file for manual rollback; a failure restores the source.
    pub fn migrate_verified<P, F>(path: P, transform: F) -> Result<MigrationReport>
    where
        P: AsRef<Path>,
        F: FnOnce(&[UsageHistorySample]) -> Result<Vec<UsageHistorySample>>,
    {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(UsageStoreError::InvalidImport(
                "database path must be absolute".into(),
            ));
        }
        let parent = path.parent().ok_or_else(|| {
            UsageStoreError::InvalidImport("database migration parent is missing".into())
        })?;
        fs::create_dir_all(parent)?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| UsageStoreError::InvalidImport("database filename is invalid".into()))?;
        let pid = std::process::id();
        let candidate = parent.join(format!(".{file_name}.migration-{pid}.candidate"));
        let rollback = parent.join(format!(".{file_name}.migration-{pid}.original"));
        let lock_path = parent.join(format!(".{file_name}.migration.lock"));

        let mut lock_options = OpenOptions::new();
        lock_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            lock_options.mode(0o600);
        }
        let _lock = lock_options.open(&lock_path).map_err(|error| {
            UsageStoreError::Io(std::io::Error::new(
                error.kind(),
                format!("database migration is already running: {error}"),
            ))
        })?;

        let result = (|| {
            if candidate.exists() || rollback.exists() {
                return Err(UsageStoreError::InvalidImport(
                    "stale migration candidate/original exists; inspect before retry".into(),
                ));
            }
            let source_store = Self::open(path)?;
            let source_samples = source_store.load_all()?;
            let source_periods = build_reset_periods(&source_samples);
            let source_fingerprint = samples_fingerprint(&source_samples);
            let candidate_samples = transform(&source_samples)?;
            validate_migration_samples(&candidate_samples)?;
            let mut candidate_store = Self::open(&candidate)?;
            candidate_store.upsert_samples(&candidate_samples)?;
            drop(candidate_store);
            quick_check_database(&candidate)?;
            let candidate_store = Self::open(&candidate)?;
            let verified_samples = candidate_store.load_all()?;
            let candidate_periods = build_reset_periods(&verified_samples);
            let candidate_fingerprint = samples_fingerprint(&verified_samples);
            if verified_samples.len() != source_samples.len()
                || verified_samples.len() != candidate_samples.len()
                || candidate_fingerprint != samples_fingerprint(&candidate_samples)
                || source_periods != candidate_periods
            {
                return Err(UsageStoreError::InvalidImport(
                    "migration candidate row/fingerprint/period validation failed".into(),
                ));
            }
            drop(candidate_store);
            drop(source_store);

            // Preserve the current database before the atomic path switch.
            Self::backup_generations(path, 3)?;
            // Keep a separately named old generation for manual rollback.
            // The source is closed and validated above, so a byte copy here
            // cannot observe an in-flight transaction. On Unix, replacing the
            // path with one same-directory rename is the atomic publication
            // boundary; the old DB remains available at `rollback` and in the
            // online backup generations. Windows cannot replace an open file
            // through `rename`, so it uses the conservative two-rename path.
            let preserve_result = (|| -> Result<()> {
                fs::copy(path, &rollback)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&rollback, fs::Permissions::from_mode(0o600))?;
                }
                quick_check_database(&rollback)
            })();
            if let Err(error) = preserve_result {
                let _ = fs::remove_file(&rollback);
                return Err(error);
            }
            #[cfg(unix)]
            {
                if let Err(error) = fs::rename(&candidate, path) {
                    let _ = fs::remove_file(&rollback);
                    return Err(UsageStoreError::Io(error));
                }
            }
            #[cfg(not(unix))]
            {
                fs::rename(path, &rollback)?;
                if let Err(error) = fs::rename(&candidate, path) {
                    let _ = fs::rename(&rollback, path);
                    return Err(UsageStoreError::Io(error));
                }
            }

            Ok(MigrationReport {
                source_rows: source_samples.len(),
                candidate_rows: verified_samples.len(),
                source_fingerprint,
                candidate_fingerprint,
                preserved_backup: rollback,
            })
        })();

        if result.is_err() {
            let _ = fs::remove_file(&candidate);
        }
        let _ = fs::remove_file(&lock_path);
        result
    }

    /// Loads all samples in reset-window and timestamp order.
    pub fn load_all(&self) -> Result<Vec<UsageHistorySample>> {
        let mut statement = self.connection.prepare(
            "SELECT timestamp, reset_at, remaining_percent, sol_dollars, \
                    terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens \
             FROM usage_history \
             ORDER BY reset_at ASC, timestamp ASC",
        )?;
        let mut rows = statement.query([])?;
        let mut samples = Vec::new();

        while let Some(row) = rows.next()? {
            if let Some(sample) = valid_sample_from_row(row)? {
                samples.push(sample);
            }
        }

        Ok(samples)
    }

    fn load_recent_history_impl(&self, now: DateTime<Utc>) -> Result<Vec<UsageHistorySample>> {
        let cutoff = one_month_before(now).timestamp();
        let now_timestamp = now.timestamp();
        let mut statement = self.connection.prepare(
            "SELECT timestamp, reset_at, remaining_percent, sol_dollars, \
                    terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens \
             FROM usage_history \
             WHERE timestamp > ?1 AND timestamp <= ?2 \
             ORDER BY timestamp DESC, reset_at DESC",
        )?;
        let mut rows = statement.query(params![cutoff, now_timestamp])?;
        let mut samples = Vec::with_capacity(MAX_RECENT_HISTORY_SAMPLES);
        while let Some(row) = rows.next()? {
            if let Some(sample) = valid_sample_from_row(row)? {
                samples.push(sample);
            }
        }
        samples.sort_by_key(|sample| (sample.reset_at, sample.timestamp));
        Ok(samples)
    }

    /// Loads valid samples from `(one calendar month before now, now]`.
    ///
    /// The database retains three months independently of this bounded read.
    pub fn load_recent_one_month(&self, now: DateTime<Utc>) -> Result<Vec<UsageHistorySample>> {
        self.load_recent_history_impl(now)
    }

    /// Alias for the same bounded read, retaining the history terminology.
    pub fn load_recent_history(&self, now: DateTime<Utc>) -> Result<Vec<UsageHistorySample>> {
        self.load_recent_history_impl(now)
    }

    /// Loads the bounded recent observation timeline. Existing rows without a
    /// sidecar provenance record are intentionally labelled `legacy-unknown`;
    /// auxiliary unavailable rows remain visible here while staying absent from
    /// the v1 `usage_history` projection.
    pub fn load_recent_observations(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<UsageHistoryObservation>> {
        let cutoff = one_month_before(now).timestamp();
        let now_timestamp = now.timestamp();
        let mut observations = BTreeMap::<(i64, i64), UsageHistoryObservation>::new();
        for sample in self.load_recent_history_impl(now)? {
            let observation = UsageHistoryObservation::legacy_unknown(&sample);
            observations.insert((sample.reset_at, sample.timestamp), observation);
        }

        let mut statement = self.connection.prepare(
            "SELECT data_generation, data_hash, snapshot_json
             FROM durable_state
             WHERE singleton >= ?1 AND data_generation > ?2 AND data_generation <= ?3
             ORDER BY data_generation ASC, singleton ASC",
        )?;
        let mut rows = statement.query(params![
            DURABLE_STATE_OBSERVATION_MIN_SINGLETON,
            cutoff,
            now_timestamp,
        ])?;
        while let Some(row) = rows.next()? {
            let observation = observation_from_sql(row.get(0)?, row.get(1)?, row.get(2)?)?;
            observations.insert((observation.reset_at, observation.timestamp), observation);
        }
        drop(rows);
        drop(statement);
        let mut model_rows = BTreeMap::<(i64, i64), (Vec<SessionModelTotal>, bool)>::new();
        let mut model_statement = self.connection.prepare(
            "SELECT reset_at, timestamp, model, total_tokens, input_tokens,
                    cached_input_tokens, output_tokens, cache_write_input_tokens,
                    model_set_complete
             FROM usage_model_history
             WHERE timestamp > ?1 AND timestamp <= ?2
             ORDER BY reset_at, timestamp, model",
        )?;
        let mut rows = model_statement.query(params![cutoff, now_timestamp])?;
        while let Some(row) = rows.next()? {
            let reset_at: i64 = row.get(0)?;
            let timestamp: i64 = row.get(1)?;
            if !observations.contains_key(&(reset_at, timestamp)) {
                continue;
            }
            let cache_write: Option<String> = row.get(7)?;
            let complete = match row.get::<_, i64>(8)? {
                0 => false,
                1 => true,
                _ => {
                    return Err(UsageStoreError::InvalidImport(
                        "history model completeness is invalid".into(),
                    ));
                }
            };
            let entry = model_rows
                .entry((reset_at, timestamp))
                .or_insert_with(|| (Vec::new(), complete));
            if entry.1 != complete {
                return Err(UsageStoreError::InvalidImport(
                    "history model completeness is inconsistent".into(),
                ));
            }
            entry.0.push(SessionModelTotal {
                model: row.get(2)?,
                total_tokens: canonical_u64_text(&row.get::<_, String>(3)?, "history total")?,
                input_tokens: canonical_u64_text(&row.get::<_, String>(4)?, "history input")?,
                cached_input_tokens: canonical_u64_text(
                    &row.get::<_, String>(5)?,
                    "history cached input",
                )?,
                output_tokens: canonical_u64_text(&row.get::<_, String>(6)?, "history output")?,
                cache_write_input_tokens: cache_write
                    .as_deref()
                    .map(|value| canonical_u64_text(value, "history cache write"))
                    .transpose()?,
            });
        }
        for (key, (totals, complete)) in model_rows {
            let canonical = canonicalize_model_totals(&totals)?;
            if let Some(observation) = observations.get_mut(&key) {
                observation.model_totals = Some(canonical);
                observation.model_totals_complete = complete;
            }
        }
        Ok(observations.into_values().collect())
    }

    /// Pure grouping helper exposed beside the store API for callers that
    /// already have a bounded sample slice.
    pub fn group_reset_periods(samples: &[UsageHistorySample]) -> Vec<ResetPeriod> {
        build_reset_periods(samples)
    }

    /// Returns the immutable legacy hand-off values only while their exact
    /// input/cache/output baseline has not yet been applied.
    pub fn pending_history_continuity_recovery(&self) -> Result<Option<HistoryContinuityRecovery>> {
        Ok(load_history_continuity(&self.connection)?
            .filter(|continuity| !continuity.model_totals_applied)
            .map(|continuity| HistoryContinuityRecovery {
                source_fingerprint: continuity.source_fingerprint,
                source_rows: continuity.source_rows,
                boundary_timestamp: continuity.boundary_timestamp,
                reset_at: continuity.reset_at,
                sol_dollars: continuity.sol_dollars,
                terra_dollars: continuity.terra_dollars,
                luna_dollars: continuity.luna_dollars,
                sol_tokens: continuity.sol_tokens,
                terra_tokens: continuity.terra_tokens,
                luna_tokens: continuity.luna_tokens,
            }))
    }

    /// Bridges the one verified hand-off from the pre-account legacy store to
    /// the first account partition. The legacy database stays read-only; all
    /// imported rows, the offset, and the generation bump are one transaction.
    pub fn bridge_verified_legacy_history<P: AsRef<Path>>(
        &mut self,
        legacy_path: P,
    ) -> Result<bool> {
        if load_history_continuity(&self.connection)?.is_some() {
            return Ok(false);
        }
        let storage_epoch: String = self.connection.query_row(
            "SELECT storage_epoch FROM storage_partition WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if canonical_u64_text(&storage_epoch, "storage epoch")? != 1 {
            return Ok(false);
        }
        let legacy_path = legacy_path.as_ref();
        if !legacy_path.is_absolute() || !legacy_path.exists() {
            return Ok(false);
        }
        let legacy = Self::open_read_only(legacy_path)?;
        let legacy_samples = legacy.load_all()?;
        let current_samples = self.load_all()?;
        let Some(boundary_current) = current_samples
            .iter()
            .min_by_key(|sample| (sample.timestamp, sample.reset_at))
            .cloned()
        else {
            return Ok(false);
        };
        if boundary_current.sol_dollars != 0.0
            || boundary_current.terra_dollars != 0.0
            || boundary_current.luna_dollars != 0.0
            || boundary_current.sol_tokens != 0
            || boundary_current.terra_tokens != 0
            || boundary_current.luna_tokens != 0
        {
            return Ok(false);
        }
        let Some(boundary_legacy) = legacy_samples
            .iter()
            .find(|sample| {
                sample.timestamp == boundary_current.timestamp
                    && sample.reset_at == boundary_current.reset_at
                    && sample.remaining_percent == boundary_current.remaining_percent
            })
            .cloned()
        else {
            return Ok(false);
        };
        if boundary_legacy.sol_dollars == 0.0
            && boundary_legacy.terra_dollars == 0.0
            && boundary_legacy.luna_dollars == 0.0
            && boundary_legacy.sol_tokens == 0
            && boundary_legacy.terra_tokens == 0
            && boundary_legacy.luna_tokens == 0
        {
            return Ok(false);
        }
        let selected_legacy = legacy_samples
            .into_iter()
            .filter(|sample| {
                sample.reset_at == boundary_current.reset_at
                    && sample.timestamp <= boundary_current.timestamp
            })
            .collect::<Vec<_>>();
        if selected_legacy.len() < 2
            || selected_legacy.last().map(|sample| sample.timestamp)
                != Some(boundary_current.timestamp)
        {
            return Ok(false);
        }

        let mut legacy_sources = BTreeSet::new();
        {
            let mut statement = legacy.connection.prepare(
                "SELECT root_identity, relative_path, file_device, file_inode FROM recorded_sessions",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                legacy_sources.insert(row?);
            }
        }
        let source_identity_matches = {
            let mut statement = self.connection.prepare(
                "SELECT root_identity, relative_path, file_device, file_inode FROM session_checkpoints",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            let mut matched = false;
            for row in rows {
                if legacy_sources.contains(&row?) {
                    matched = true;
                    break;
                }
            }
            matched
        };
        if !source_identity_matches {
            return Ok(false);
        }

        let continuity = HistoryContinuity {
            source_fingerprint: samples_fingerprint(&selected_legacy),
            source_rows: selected_legacy.len(),
            boundary_timestamp: boundary_legacy.timestamp,
            reset_at: boundary_legacy.reset_at,
            remaining_percent: boundary_legacy.remaining_percent.ok_or_else(|| {
                UsageStoreError::InvalidImport("legacy boundary has no quota observation".into())
            })?,
            sol_dollars: boundary_legacy.sol_dollars,
            terra_dollars: boundary_legacy.terra_dollars,
            luna_dollars: boundary_legacy.luna_dollars,
            sol_tokens: boundary_legacy.sol_tokens,
            terra_tokens: boundary_legacy.terra_tokens,
            luna_tokens: boundary_legacy.luna_tokens,
            model_totals_applied: false,
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_history_continuity_schema(&transaction)?;
        transaction.execute(
            "INSERT INTO history_continuity (
                singleton, source_fingerprint, source_rows, boundary_timestamp,
                reset_at, remaining_percent, sol_dollars, terra_dollars,
                luna_dollars, sol_tokens, terra_tokens, luna_tokens,
                model_totals_applied
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0)",
            params![
                &continuity.source_fingerprint,
                continuity.source_rows as i64,
                continuity.boundary_timestamp,
                continuity.reset_at,
                continuity.remaining_percent,
                continuity.sol_dollars,
                continuity.terra_dollars,
                continuity.luna_dollars,
                continuity.sol_tokens.to_string(),
                continuity.terra_tokens.to_string(),
                continuity.luna_tokens.to_string(),
            ],
        )?;
        let historical = selected_legacy
            .iter()
            .filter(|sample| sample.timestamp < continuity.boundary_timestamp)
            .cloned()
            .collect::<Vec<_>>();
        let historical = canonicalize_samples(&transaction, &historical)?;
        upsert_canonical_samples(&transaction, &historical)?;
        let adjusted_current = apply_history_continuity(&transaction, &current_samples)?;
        let adjusted_current = canonicalize_samples(&transaction, &adjusted_current)?;
        upsert_canonical_samples(&transaction, &adjusted_current)?;
        let generation: String = transaction.query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let next = canonical_u64_text(&generation, "collection generation")?
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        transaction.execute(
            "UPDATE collection_generation SET data_generation=?1 WHERE singleton=1",
            [next.to_string()],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// Inserts a sample or canonicalizes it with the row at the same exact key.
    pub fn upsert_sample(&self, sample: &UsageHistorySample) -> Result<()> {
        // Even the one-row convenience path uses an explicit transaction, so
        // every history mutation has the same all-or-nothing boundary as a
        // batch write. `new_unchecked` preserves this method's shared-
        // reference API while taking the immediate writer lock.
        let transaction =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let adjusted = apply_history_continuity(&transaction, std::slice::from_ref(sample))?;
        let canonical = canonicalize_samples(&transaction, &adjusted)?;
        upsert_canonical_samples(&transaction, &canonical)?;
        transaction.commit()?;
        Ok(())
    }

    /// Atomically upserts several samples after validating the complete batch.
    pub fn upsert_samples(&mut self, samples: &[UsageHistorySample]) -> Result<()> {
        self.upsert_samples_and_recorded_sessions(samples, &[])
    }

    /// Atomically commits one local collection generation.
    ///
    /// A session is durable evidence for cleanup only when its exact marker
    /// and the generation's canonical usage rows commit together. Marker
    /// read-back is performed inside the transaction as well as later through
    /// a fresh read-only connection before any source file is removed.
    pub fn upsert_samples_and_recorded_sessions(
        &mut self,
        samples: &[UsageHistorySample],
        sources: &[RecordedSessionSource],
    ) -> Result<()> {
        let sources = canonicalize_recorded_sessions(sources)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let adjusted = apply_history_continuity(&transaction, samples)?;
        let canonical = canonicalize_samples(&transaction, &adjusted)?;
        upsert_canonical_samples(&transaction, &canonical)?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO recorded_sessions (
                    root_identity,
                    relative_path,
                    file_bytes,
                    modified_nanos,
                    file_device,
                    file_inode
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT DO NOTHING",
            )?;
            for source in &sources {
                statement.execute(params![
                    &source.root_identity,
                    &source.relative_path,
                    source.file_bytes as i64,
                    source.modified_nanos.to_string(),
                    source.file_device.to_string(),
                    source.file_inode.to_string(),
                ])?;
            }
        }
        for source in &sources {
            if !recorded_session_matches_in(&transaction, source)? {
                return Err(UsageStoreError::InvalidImport(
                    "recorded session transaction read-back failed".into(),
                ));
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Loads the durable append checkpoint and absolute model totals for this
    /// account partition. The caller decides whether the stored reset period
    /// is still current; file checkpoints remain valid across quota resets.
    pub fn load_session_collection_state(&self) -> Result<SessionCollectionState> {
        // The generation, quota observation, cursors and absolute totals are
        // one logical state. Hold one deferred read transaction so a recorder
        // commit cannot be observed between these SELECTs.
        let transaction =
            rusqlite::Transaction::new_unchecked(&self.connection, TransactionBehavior::Deferred)?;
        let (generation, reset_at, window_seconds, collector_epoch, cycle_seq): (
            String,
            i64,
            i64,
            Option<String>,
            String,
        ) = transaction.query_row(
            "SELECT data_generation, reset_at, window_seconds, collector_epoch, cycle_seq
             FROM collection_generation WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        let data_generation = canonical_u64_text(&generation, "collection generation")?;
        let collector_epoch = collector_epoch
            .as_deref()
            .map(|value| canonical_u128_hex(value, "collector epoch"))
            .transpose()?;
        let cycle_seq = canonical_u64_text(&cycle_seq, "cycle sequence")?;
        if collector_epoch.is_none() != (cycle_seq == 0) {
            return Err(UsageStoreError::InvalidImport(
                "collector generation is inconsistent".into(),
            ));
        }
        let task_running_column = if session_checkpoint_running_column_present(&transaction)? {
            "last_task_running"
        } else {
            // A read-only resident can reach the legacy partition before
            // its serialized writer performs the migration. Preserve the
            // unknown state without making that read depend on a column
            // which does not exist yet.
            "NULL"
        };
        let checkpoint_query = format!(
            "SELECT root_identity, relative_path, file_device, file_inode,
                    committed_offset, discard_until_lf, collector_epoch, cycle_seq,
                    prefix_generation, prefix_sha256, fully_attributed_from_zero,
                    token_baseline_known, last_model, {task_running_column}, previous_total, previous_input,
                    previous_cached_input, previous_output, {cache_write_column}
             FROM session_checkpoints
             ORDER BY root_identity, relative_path, file_device, file_inode, prefix_generation"
        , cache_write_column = if cache_write_column_present(&transaction, "session_checkpoints", "previous_cache_write_input")? {
            "previous_cache_write_input"
        } else { "NULL" });
        let checkpoints = {
            let mut checkpoint_statement = transaction.prepare(&checkpoint_query)?;
            let rows = checkpoint_statement.query_map([], |row| {
                let committed_offset = row.get::<_, i64>(4)?;
                let collector_epoch = row.get::<_, String>(6)?;
                let cycle_seq = row.get::<_, String>(7)?;
                let cycle_seq = cycle_seq
                    .parse::<u64>()
                    .ok()
                    .filter(|parsed| parsed.to_string() == cycle_seq)
                    .ok_or(rusqlite::Error::InvalidQuery)?;
                Ok(SessionCheckpoint {
                    root_identity: row.get(0)?,
                    relative_path: row.get(1)?,
                    file_device: row
                        .get::<_, String>(2)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    file_inode: row
                        .get::<_, String>(3)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    committed_offset: u64::try_from(committed_offset)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    discard_until_lf: row.get::<_, i64>(5)? == 1,
                    collector_epoch: u128::from_str_radix(&collector_epoch, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    cycle_seq,
                    prefix_generation: u128::from_str_radix(&row.get::<_, String>(8)?, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    prefix_sha256: row.get(9)?,
                    fully_attributed_from_zero: row.get::<_, i64>(10)? == 1,
                    token_baseline_known: row.get::<_, i64>(11)? == 1,
                    last_model: row.get(12)?,
                    last_task_running: match row.get::<_, Option<i64>>(13)? {
                        None => None,
                        Some(0) => Some(false),
                        Some(1) => Some(true),
                        Some(_) => return Err(rusqlite::Error::InvalidQuery),
                    },
                    previous_total: row
                        .get::<_, String>(14)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    previous_input: row
                        .get::<_, String>(15)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    previous_cached_input: row
                        .get::<_, String>(16)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    previous_output: row
                        .get::<_, String>(17)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    previous_cache_write_input: row
                        .get::<_, Option<String>>(18)?
                        .map(|text| {
                            text.parse::<u64>()
                                .map_err(|_| rusqlite::Error::InvalidQuery)
                        })
                        .transpose()?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for checkpoint in &checkpoints {
            validate_session_checkpoint(checkpoint)?;
        }

        let model_totals = {
            let write_column = if cache_write_column_present(
                &transaction,
                "session_model_totals",
                "cache_write_input_tokens",
            )? {
                "cache_write_input_tokens"
            } else {
                "NULL"
            };
            let mut totals_statement = transaction.prepare(&format!(
                "SELECT model, total_tokens, input_tokens, cached_input_tokens, output_tokens, {write_column}
                 FROM session_model_totals ORDER BY model"
            ))?;
            let rows = totals_statement.query_map([], |row| {
                Ok(SessionModelTotal {
                    model: row.get(0)?,
                    cache_write_input_tokens: row
                        .get::<_, Option<String>>(5)?
                        .map(|text| {
                            text.parse::<u64>()
                                .map_err(|_| rusqlite::Error::InvalidQuery)
                        })
                        .transpose()?,
                    total_tokens: row
                        .get::<_, String>(1)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    input_tokens: row
                        .get::<_, String>(2)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    cached_input_tokens: row
                        .get::<_, String>(3)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    output_tokens: row
                        .get::<_, String>(4)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let model_totals = canonicalize_model_totals(&model_totals)?;
        let last_quota_observation = transaction
            .query_row(
                "SELECT timestamp, remaining_percent
                 FROM usage_history
                 WHERE remaining_percent IS NOT NULL
                 ORDER BY timestamp DESC, reset_at DESC
                 LIMIT 1",
                [],
                |row| {
                    Ok(SessionQuotaObservation {
                        observed_at: row.get(0)?,
                        remaining_percent: row.get(1)?,
                    })
                },
            )
            .optional()?;
        if last_quota_observation.as_ref().is_some_and(|observation| {
            observation.observed_at <= 0
                || !observation.remaining_percent.is_finite()
                || !(0.0..=100.0).contains(&observation.remaining_percent)
        }) {
            return Err(UsageStoreError::InvalidImport(
                "last quota observation is invalid".into(),
            ));
        }
        transaction.commit()?;
        Ok(SessionCollectionState {
            data_generation,
            reset_at,
            window_seconds,
            collector_epoch,
            cycle_seq,
            last_quota_observation,
            checkpoints,
            model_totals,
        })
    }

    /// Read every ledger row for this account partition. Rows are returned in
    /// deterministic interval order and each value is validated at the read
    /// boundary; malformed legacy data can therefore never become public data.
    pub fn load_recorder_gaps(&self) -> Result<Vec<RecorderGap>> {
        if recorder_gap_ledger_columns(&self.connection)? == legacy_recorder_gap_ledger_columns() {
            // A read-only caller may inspect an existing pre-v1 partition
            // before the serialized writer has had a chance to migrate the
            // fixture-era table. Those rows have no source proof and are not
            // exposed as public gaps until the writer performs its atomic
            // schema transition.
            return Ok(Vec::new());
        }
        let partition_id: String = self.connection.query_row(
            "SELECT partition_id FROM storage_partition WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let mut statement = self.connection.prepare(
            "SELECT gap_id, partition_id, source_identity_before, source_identity_after,
                    cursor_before, cursor_after, stopped_at_monotonic_ns,
                    resumed_at_monotonic_ns, start_at, end_at, reset_at, reason, state,
                    owner_collector_epoch, confirmation_cycle_seq
             FROM recorder_gap_ledger
             ORDER BY start_at, end_at, gap_id",
        )?;
        let gaps = statement
            .query_map([], recorder_gap_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for gap in &gaps {
            validate_recorder_gap(gap, Some(&partition_id))?;
        }
        Ok(gaps)
    }

    /// Only source-proven closed gaps are allowed to cross into the public
    /// history projection. Missing rows, transport errors, and session
    /// backfill are intentionally not converted into gaps here.
    pub fn load_confirmed_recorder_gaps(&self) -> Result<Vec<RecorderGap>> {
        let mut gaps = self
            .load_recorder_gaps()?
            .into_iter()
            .filter(|gap| gap.state == "confirmed" && gap.reset_at.is_some())
            .collect::<Vec<_>>();
        gaps.sort_by_key(|gap| (gap.start_at, gap.end_at, gap.gap_id.clone()));
        let mut furthest_end = None;
        for gap in &gaps {
            if furthest_end.is_some_and(|end| gap.start_at <= end) {
                return Err(UsageStoreError::InvalidImport(
                    "confirmed recorder gaps overlap".into(),
                ));
            }
            furthest_end = Some(furthest_end.unwrap_or(gap.end_at).max(gap.end_at));
        }
        gaps.sort_by(|left, right| {
            (
                left.reset_at,
                left.start_at,
                left.end_at,
                left.gap_id.as_str(),
            )
                .cmp(&(
                    right.reset_at,
                    right.start_at,
                    right.end_at,
                    right.gap_id.as_str(),
                ))
        });
        Ok(gaps)
    }

    /// Insert or idempotently replay one ledger record. A duplicate gap ID is
    /// accepted only when every logical field matches byte-for-byte; a
    /// confirmed interval may not overlap another confirmed interval in the
    /// same partition.
    pub fn upsert_recorder_gap(&mut self, gap: &RecorderGap) -> Result<()> {
        let partition_id: String = self.connection.query_row(
            "SELECT partition_id FROM storage_partition WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        validate_recorder_gap(gap, Some(&partition_id))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT gap_id, partition_id, source_identity_before, source_identity_after,
                        cursor_before, cursor_after, stopped_at_monotonic_ns,
                        resumed_at_monotonic_ns, start_at, end_at, reset_at, reason, state,
                        owner_collector_epoch, confirmation_cycle_seq
                 FROM recorder_gap_ledger WHERE gap_id = ?1",
                [&gap.gap_id],
                recorder_gap_from_row,
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing != *gap {
                return Err(UsageStoreError::InvalidImport(
                    "recorder gap replay conflicts with stored record".into(),
                ));
            }
            transaction.commit()?;
            return Ok(());
        }
        if gap.state != "pending" {
            return Err(UsageStoreError::InvalidImport(
                "new recorder gaps must start pending".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO recorder_gap_ledger (
                gap_id, partition_id, source_identity_before, source_identity_after,
                cursor_before, cursor_after, stopped_at_monotonic_ns,
                resumed_at_monotonic_ns, start_at, end_at, reset_at, reason, state,
                owner_collector_epoch, confirmation_cycle_seq
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                &gap.gap_id,
                &gap.partition_id,
                &gap.source_identity_before,
                &gap.source_identity_after,
                &gap.cursor_before,
                &gap.cursor_after,
                i64::try_from(gap.stopped_at_monotonic_ns).map_err(|_| {
                    UsageStoreError::InvalidImport(
                        "gap monotonic timestamp exceeds SQLite range".into(),
                    )
                })?,
                gap.resumed_at_monotonic_ns
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| {
                        UsageStoreError::InvalidImport(
                            "gap monotonic timestamp exceeds SQLite range".into(),
                        )
                    })?,
                gap.start_at,
                gap.end_at,
                gap.reset_at,
                &gap.reason,
                &gap.state,
                format!("{:032x}", gap.owner_collector_epoch),
                gap.confirmation_cycle_seq.to_string(),
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Start a pending interval. This convenience method makes stop/restart
    /// bookkeeping explicit while retaining the same idempotent writer path.
    pub fn begin_recorder_gap(&mut self, gap: &RecorderGap) -> Result<()> {
        if gap.state != "pending" {
            return Err(UsageStoreError::InvalidImport(
                "new recorder gaps must start pending".into(),
            ));
        }
        self.upsert_recorder_gap(gap)
    }

    /// Record a source-rescan recovery or a source-proven unrecoverable
    /// interval. The caller must supply the complete record, so identity,
    /// reset, and range changes cannot be smuggled into a state transition.
    pub fn record_recorder_gap(&mut self, gap: &RecorderGap) -> Result<()> {
        self.transition_recorder_gap(gap)
    }

    pub fn recover_recorder_gap(&mut self, gap: &RecorderGap) -> Result<()> {
        if gap.state != "recovered" {
            return Err(UsageStoreError::InvalidImport(
                "recovered recorder gaps must use recovered state".into(),
            ));
        }
        validate_gap_repair_proof(gap)?;
        self.transition_recorder_gap(gap)
    }

    pub fn confirm_recorder_gap(&mut self, gap: &RecorderGap) -> Result<()> {
        if gap.state != "confirmed" {
            return Err(UsageStoreError::InvalidImport(
                "confirmed recorder gaps must use confirmed state".into(),
            ));
        }
        validate_gap_repair_proof(gap)?;
        self.transition_recorder_gap(gap)
    }

    /// Apply the only permitted state transitions (`pending` → one terminal
    /// state). Static identity/time fields remain immutable; a changed reset,
    /// cursor, or source identity is rejected rather than reinterpreted.
    pub fn transition_recorder_gap(&mut self, gap: &RecorderGap) -> Result<()> {
        let partition_id: String = self.connection.query_row(
            "SELECT partition_id FROM storage_partition WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        validate_recorder_gap(gap, Some(&partition_id))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "SELECT gap_id, partition_id, source_identity_before, source_identity_after,
                        cursor_before, cursor_after, stopped_at_monotonic_ns,
                        resumed_at_monotonic_ns, start_at, end_at, reset_at, reason, state,
                        owner_collector_epoch, confirmation_cycle_seq
                 FROM recorder_gap_ledger WHERE gap_id = ?1",
                [&gap.gap_id],
                recorder_gap_from_row,
            )
            .optional()?;
        let Some(existing) = existing else {
            return Err(UsageStoreError::InvalidImport(
                "recorder gap transition requires an existing pending record".into(),
            ));
        };
        if existing == *gap {
            transaction.commit()?;
            return Ok(());
        }
        if existing.state != "pending"
            || !matches!(gap.state.as_str(), "recovered" | "confirmed" | "rejected")
            || existing.partition_id != gap.partition_id
            || existing.source_identity_before != gap.source_identity_before
            || existing.cursor_before != gap.cursor_before
            || existing.stopped_at_monotonic_ns != gap.stopped_at_monotonic_ns
            || existing.start_at != gap.start_at
            || existing.end_at != gap.end_at
            || existing.reset_at != gap.reset_at
            || existing.reason != gap.reason
            || existing.owner_collector_epoch != gap.owner_collector_epoch
            || (existing.resumed_at_monotonic_ns.is_some()
                && existing.resumed_at_monotonic_ns != gap.resumed_at_monotonic_ns)
        {
            return Err(UsageStoreError::InvalidImport(
                "recorder gap transition contradicts its pending record".into(),
            ));
        }
        if matches!(gap.state.as_str(), "recovered" | "confirmed") {
            validate_gap_repair_proof(gap)?;
        }
        if gap.state == "confirmed" {
            let overlaps: i64 = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM recorder_gap_ledger
                    WHERE partition_id = ?1 AND state = 'confirmed'
                      AND gap_id <> ?4 AND start_at <= ?3 AND end_at >= ?2
                )",
                params![&gap.partition_id, gap.start_at, gap.end_at, &gap.gap_id],
                |row| row.get(0),
            )?;
            if overlaps != 0 {
                return Err(UsageStoreError::InvalidImport(
                    "confirmed recorder gap overlaps an existing interval".into(),
                ));
            }
        }
        transaction.execute(
            "UPDATE recorder_gap_ledger
             SET source_identity_after = ?2, cursor_after = ?3,
                 resumed_at_monotonic_ns = ?4, reason = ?5, state = ?6,
                 confirmation_cycle_seq = ?7
             WHERE gap_id = ?1",
            params![
                &gap.gap_id,
                &gap.source_identity_after,
                &gap.cursor_after,
                gap.resumed_at_monotonic_ns
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| {
                        UsageStoreError::InvalidImport(
                            "gap monotonic timestamp exceeds SQLite range".into(),
                        )
                    })?,
                &gap.reason,
                &gap.state,
                gap.confirmation_cycle_seq.to_string(),
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Reconcile pending stop/restart intervals with one bounded source
    /// rescan result.  The caller supplies only minute starts backed by
    /// actual quota observations from the just-acknowledged collector
    /// generation; session-derived rows (whose remaining value is null) must
    /// not be passed as quota proof.
    ///
    /// A source result is deliberately conservative:
    ///
    /// * every complete minute in the interval proves `recovered`;
    /// * an explicit `source_closed` proof with no source minute in the
    ///   interval proves `confirmed`;
    /// * a reset-period contradiction proves `rejected`;
    /// * incomplete but otherwise consistent evidence leaves the row
    ///   `pending` for the next bounded cycle.
    ///
    /// The persisted transition remains the single `pending` → terminal
    /// writer path, so repeating an acknowledged source result is a no-op and
    /// cannot create a duplicate history gap.
    pub fn reconcile_pending_recorder_gaps(
        &mut self,
        source_identity_after: &str,
        cursor_after: &str,
        resumed_at_monotonic_ns: u64,
        reset_at: i64,
        owner_collector_epoch: u128,
        confirmation_cycle_seq: u64,
        source_minutes: &[i64],
        source_closed: bool,
    ) -> Result<Vec<RecorderGap>> {
        validate_recorder_source_rescan(
            source_identity_after,
            cursor_after,
            resumed_at_monotonic_ns,
            reset_at,
            owner_collector_epoch,
            confirmation_cycle_seq,
            source_minutes,
        )?;

        let pending = self
            .load_recorder_gaps()?
            .into_iter()
            .filter(|gap| gap.state == "pending")
            .collect::<Vec<_>>();
        let mut transitioned = Vec::new();
        for gap in pending {
            // A source proof from the same collector generation/cycle that
            // created the stop marker has no new evidence.  Waiting here is
            // important: a heartbeat must never turn into a commit claim.
            if gap.owner_collector_epoch == owner_collector_epoch
                && confirmation_cycle_seq <= gap.confirmation_cycle_seq
            {
                continue;
            }

            let state = match gap.reset_at {
                None => "rejected",
                Some(gap_reset) if gap_reset != reset_at => "rejected",
                Some(_) if source_minutes_cover_gap(&gap, source_minutes) => "recovered",
                Some(_) if source_closed && !source_minutes_overlap_gap(&gap, source_minutes) => {
                    "confirmed"
                }
                Some(_) => continue,
            };
            let mut terminal = gap;
            terminal.source_identity_after = source_identity_after.to_owned();
            terminal.cursor_after = cursor_after.to_owned();
            terminal.resumed_at_monotonic_ns = Some(resumed_at_monotonic_ns);
            terminal.state = state.to_owned();
            terminal.confirmation_cycle_seq = confirmation_cycle_seq;
            if state == "confirmed" {
                let overlaps: i64 = self.connection.query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM recorder_gap_ledger
                        WHERE partition_id = ?1 AND state = 'confirmed'
                          AND start_at <= ?3 AND end_at >= ?2
                    )",
                    params![&terminal.partition_id, terminal.start_at, terminal.end_at],
                    |row| row.get(0),
                )?;
                if overlaps != 0 {
                    // An overlap is a source contradiction. Keep the
                    // evidence as a rejected terminal row instead of
                    // allowing the confirmed projection to become
                    // ambiguous.
                    terminal.state = "rejected".into();
                }
            }
            match state {
                "recovered" => self.recover_recorder_gap(&terminal)?,
                "confirmed" if terminal.state == "confirmed" => {
                    self.confirm_recorder_gap(&terminal)?
                }
                "confirmed" | "rejected" => self.record_recorder_gap(&terminal)?,
                _ => unreachable!("source resolver selected only terminal states"),
            }
            transitioned.push(terminal);
        }
        Ok(transitioned)
    }

    /// Commits one verified append collection as a single account-local
    /// transaction. Intersecting source ranges are rejected; exact repeats
    /// are idempotent because all stored usage/model values are absolute.
    pub fn commit_session_collection(
        &mut self,
        commit: SessionCollectionCommit<'_>,
    ) -> Result<u64> {
        self.commit_session_collection_with_samples(commit)
            .map(|result| result.data_generation)
    }

    /// Backwards-compatible collection entry point. A normal sample is a
    /// confirmed local observation; callers that need quota-only/unavailable
    /// observations use [`Self::commit_session_collection_with_observations`].
    pub fn commit_session_collection_with_samples(
        &mut self,
        commit: SessionCollectionCommit<'_>,
    ) -> Result<SessionCollectionCommitResult> {
        let observations = commit
            .samples
            .iter()
            .map(UsageHistoryObservation::confirmed)
            .collect::<Vec<_>>();
        self.commit_session_collection_with_observations_inner(commit, &observations)
    }

    /// Atomically commits ordinary session samples and provenance observations
    /// in the same SQLite transaction. The observation list may contain
    /// unavailable quota-only rows that have no corresponding usage_history
    /// row; every supplied key is still deduplicated and strictly validated.
    pub fn commit_session_collection_with_observations(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
    ) -> Result<SessionCollectionCommitResult> {
        self.commit_session_collection_with_observations_inner(commit, observations)
    }

    fn commit_session_collection_with_observations_inner(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
    ) -> Result<SessionCollectionCommitResult> {
        let SessionCollectionCommit {
            reset_at,
            window_seconds,
            collector_epoch,
            cycle_seq,
            samples,
            checkpoints,
            ranges,
            model_totals,
            recorded_sessions,
        } = commit;
        if reset_at <= 0 || window_seconds <= 0 || collector_epoch == 0 || cycle_seq == 0 {
            return Err(UsageStoreError::InvalidImport(
                "session collection period is invalid".into(),
            ));
        }
        let mut canonical_checkpoints = BTreeMap::new();
        for checkpoint in checkpoints {
            validate_session_checkpoint(checkpoint)?;
            let key = (
                checkpoint.root_identity.clone(),
                checkpoint.relative_path.clone(),
                checkpoint.file_device,
                checkpoint.file_inode,
                checkpoint.prefix_generation,
            );
            if canonical_checkpoints
                .insert(key, checkpoint.clone())
                .is_some()
            {
                return Err(UsageStoreError::InvalidImport(
                    "duplicate session checkpoint".into(),
                ));
            }
            if checkpoint.collector_epoch != collector_epoch || checkpoint.cycle_seq != cycle_seq {
                return Err(UsageStoreError::InvalidImport(
                    "checkpoint admission generation mismatch".into(),
                ));
            }
        }
        let mut canonical_ranges = BTreeMap::new();
        for range in ranges {
            validate_session_range(range)?;
            let key = (
                range.root_identity.clone(),
                range.relative_path.clone(),
                range.file_device,
                range.file_inode,
                range.prefix_generation,
                range.start_offset,
                range.end_offset,
                range.record_sha256.clone(),
            );
            if canonical_ranges.insert(key, range.clone()).is_some() {
                return Err(UsageStoreError::InvalidImport(
                    "duplicate session range".into(),
                ));
            }
            if range.collector_epoch != collector_epoch || range.cycle_seq != cycle_seq {
                return Err(UsageStoreError::InvalidImport(
                    "range admission generation mismatch".into(),
                ));
            }
        }
        let model_totals = canonicalize_model_totals(model_totals)?;
        let recorded_sessions = canonicalize_recorded_sessions(recorded_sessions)?;
        for marker in &recorded_sessions {
            let checkpoint = canonical_checkpoints
                .values()
                .find(|checkpoint| {
                    checkpoint.root_identity == marker.root_identity
                        && checkpoint.relative_path == marker.relative_path
                        && checkpoint.file_device == marker.file_device
                        && checkpoint.file_inode == marker.file_inode
                })
                .ok_or_else(|| {
                    UsageStoreError::InvalidImport(
                        "cleanup marker has no session checkpoint".into(),
                    )
                })?;
            if !checkpoint.fully_attributed_from_zero
                || checkpoint.discard_until_lf
                || !checkpoint.token_baseline_known
                || checkpoint.file_device != marker.file_device
                || checkpoint.file_inode != marker.file_inode
                || checkpoint.committed_offset != marker.file_bytes
            {
                return Err(UsageStoreError::InvalidImport(
                    "cleanup marker is not fully attributed".into(),
                ));
            }
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let adjusted_samples = apply_history_continuity(&transaction, samples)?;
        let canonical_samples = canonicalize_samples(&transaction, &adjusted_samples)?;
        let canonical_observations =
            canonicalize_observations(&transaction, observations, &canonical_samples)?;
        let current_generation: (String, i64, i64, Option<String>, String) = transaction
            .query_row(
                "SELECT data_generation, reset_at, window_seconds, collector_epoch, cycle_seq
                 FROM collection_generation WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
        let current_data_generation =
            canonical_u64_text(&current_generation.0, "collection generation")?;
        let current_cycle_seq = canonical_u64_text(&current_generation.4, "cycle sequence")?;
        let current_epoch = current_generation
            .3
            .as_deref()
            .map(|value| canonical_u128_hex(value, "collector epoch"))
            .transpose()?;
        if current_epoch == Some(collector_epoch) {
            if current_cycle_seq == cycle_seq {
                if current_generation.1 != reset_at || current_generation.2 != window_seconds {
                    return Err(UsageStoreError::InvalidImport(
                        "replayed collection generation has a different period".into(),
                    ));
                }
                if current_data_generation == u64::MAX {
                    return Err(UsageStoreError::GenerationOverflow);
                }
                // The complete transaction for this epoch/cycle already
                // committed. Return its exact generation rather than
                // incrementing durable state on an acknowledgement retry.
                let replay_observations =
                    upsert_observations(&transaction, &canonical_observations)?;
                upsert_observation_model_totals(&transaction, &replay_observations)?;
                transaction.commit()?;
                return Ok(SessionCollectionCommitResult {
                    data_generation: current_data_generation,
                    canonical_samples,
                    canonical_observations: replay_observations,
                });
            }
            if current_cycle_seq > cycle_seq {
                return Err(UsageStoreError::InvalidImport(
                    "collection cycle moved backwards".into(),
                ));
            }
        }
        for range in canonical_ranges.values() {
            let intersects: i64 = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM session_ranges
                    WHERE root_identity = ?1 AND relative_path = ?2
                      AND file_device = ?3 AND file_inode = ?4
                      AND prefix_generation = ?5
                      AND start_offset < ?7 AND end_offset > ?6
                      AND NOT (
                          start_offset = ?6 AND end_offset = ?7 AND record_sha256 = ?8
                      )
                )",
                params![
                    &range.root_identity,
                    &range.relative_path,
                    range.file_device.to_string(),
                    range.file_inode.to_string(),
                    format!("{:032x}", range.prefix_generation),
                    range.start_offset as i64,
                    range.end_offset as i64,
                    &range.record_sha256,
                ],
                |row| row.get(0),
            )?;
            if intersects == 1 {
                return Err(UsageStoreError::InvalidImport(
                    "session source range intersects a committed range".into(),
                ));
            }
        }
        for checkpoint in canonical_checkpoints.values() {
            let current: Option<i64> = transaction
                .query_row(
                    "SELECT committed_offset
                     FROM session_checkpoints
                     WHERE root_identity = ?1 AND relative_path = ?2
                       AND file_device = ?3 AND file_inode = ?4
                       AND prefix_generation = ?5",
                    params![
                        &checkpoint.root_identity,
                        &checkpoint.relative_path,
                        checkpoint.file_device.to_string(),
                        checkpoint.file_inode.to_string(),
                        format!("{:032x}", checkpoint.prefix_generation),
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            if current.is_some_and(|offset| {
                u64::try_from(offset).unwrap_or(u64::MAX) > checkpoint.committed_offset
            }) {
                return Err(UsageStoreError::InvalidImport(
                    "session checkpoint moved backwards".into(),
                ));
            }
        }

        // A checkpoint is the current append cursor for one physical session
        // identity, not an audit log. Session ranges retain the immutable
        // append evidence; superseded cursor lineages only make every later
        // collection read and rewrite stale rows.
        {
            let mut statement = transaction.prepare(
                "DELETE FROM session_checkpoints
                 WHERE root_identity = ?1 AND relative_path = ?2
                   AND file_device = ?3 AND file_inode = ?4
                   AND prefix_generation <> ?5",
            )?;
            for checkpoint in canonical_checkpoints.values() {
                statement.execute(params![
                    &checkpoint.root_identity,
                    &checkpoint.relative_path,
                    checkpoint.file_device.to_string(),
                    checkpoint.file_inode.to_string(),
                    format!("{:032x}", checkpoint.prefix_generation),
                ])?;
            }
        }

        upsert_canonical_samples(&transaction, &canonical_samples)?;
        let persisted_observations = upsert_observations(&transaction, &canonical_observations)?;
        upsert_observation_model_totals(&transaction, &persisted_observations)?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO session_ranges (
                    root_identity, relative_path, file_device, file_inode,
                    start_offset, end_offset, collector_epoch, cycle_seq,
                    prefix_generation, record_sha256
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT DO NOTHING",
            )?;
            for range in canonical_ranges.values() {
                statement.execute(params![
                    &range.root_identity,
                    &range.relative_path,
                    range.file_device.to_string(),
                    range.file_inode.to_string(),
                    range.start_offset as i64,
                    range.end_offset as i64,
                    format!("{:032x}", range.collector_epoch),
                    range.cycle_seq.to_string(),
                    format!("{:032x}", range.prefix_generation),
                    &range.record_sha256,
                ])?;
            }
        }
        {
            let mut statement = transaction.prepare(
                "INSERT INTO session_checkpoints (
                    root_identity, relative_path, file_device, file_inode,
                    committed_offset, discard_until_lf, collector_epoch, cycle_seq,
                    prefix_generation, prefix_sha256, fully_attributed_from_zero,
                    token_baseline_known, last_model, last_task_running, previous_total, previous_input,
                    previous_cached_input, previous_output, previous_cache_write_input
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, ?19
                 )
                 ON CONFLICT (
                    root_identity, relative_path, file_device, file_inode, prefix_generation
                 ) DO UPDATE SET
                    committed_offset = excluded.committed_offset,
                    discard_until_lf = excluded.discard_until_lf,
                    collector_epoch = excluded.collector_epoch,
                    cycle_seq = excluded.cycle_seq,
                    prefix_sha256 = excluded.prefix_sha256,
                    fully_attributed_from_zero = excluded.fully_attributed_from_zero,
                    token_baseline_known = excluded.token_baseline_known,
                    last_model = excluded.last_model,
                    last_task_running = excluded.last_task_running,
                    previous_total = excluded.previous_total,
                    previous_input = excluded.previous_input,
                    previous_cached_input = excluded.previous_cached_input,
                    previous_output = excluded.previous_output,
                    previous_cache_write_input = excluded.previous_cache_write_input",
            )?;
            for checkpoint in canonical_checkpoints.values() {
                statement.execute(params![
                    &checkpoint.root_identity,
                    &checkpoint.relative_path,
                    checkpoint.file_device.to_string(),
                    checkpoint.file_inode.to_string(),
                    checkpoint.committed_offset as i64,
                    i64::from(checkpoint.discard_until_lf),
                    format!("{:032x}", checkpoint.collector_epoch),
                    checkpoint.cycle_seq.to_string(),
                    format!("{:032x}", checkpoint.prefix_generation),
                    &checkpoint.prefix_sha256,
                    i64::from(checkpoint.fully_attributed_from_zero),
                    i64::from(checkpoint.token_baseline_known),
                    checkpoint.last_model.as_deref(),
                    checkpoint.last_task_running.map(i64::from),
                    checkpoint.previous_total.to_string(),
                    checkpoint.previous_input.to_string(),
                    checkpoint.previous_cached_input.to_string(),
                    checkpoint.previous_output.to_string(),
                    checkpoint
                        .previous_cache_write_input
                        .map(|value| value.to_string()),
                ])?;
            }
        }
        transaction.execute("DELETE FROM session_model_totals", [])?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO session_model_totals (
                    model, total_tokens, input_tokens, cached_input_tokens, output_tokens, cache_write_input_tokens
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for total in &model_totals {
                statement.execute(params![
                    &total.model,
                    total.total_tokens.to_string(),
                    total.input_tokens.to_string(),
                    total.cached_input_tokens.to_string(),
                    total.output_tokens.to_string(),
                    total
                        .cache_write_input_tokens
                        .map(|value| value.to_string()),
                ])?;
            }
        }
        {
            let mut statement = transaction.prepare(
                "INSERT INTO recorded_sessions (
                    root_identity, relative_path, file_bytes, modified_nanos,
                    file_device, file_inode
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT DO NOTHING",
            )?;
            for source in &recorded_sessions {
                statement.execute(params![
                    &source.root_identity,
                    &source.relative_path,
                    source.file_bytes as i64,
                    source.modified_nanos.to_string(),
                    source.file_device.to_string(),
                    source.file_inode.to_string(),
                ])?;
            }
        }
        let next = current_data_generation
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        transaction.execute(
            "UPDATE collection_generation
             SET data_generation = ?1, reset_at = ?2, window_seconds = ?3,
                 collector_epoch = ?4, cycle_seq = ?5
             WHERE singleton = 1",
            params![
                next.to_string(),
                reset_at,
                window_seconds,
                format!("{collector_epoch:032x}"),
                cycle_seq.to_string(),
            ],
        )?;
        transaction.commit()?;
        Ok(SessionCollectionCommitResult {
            data_generation: next,
            canonical_samples,
            canonical_observations: persisted_observations,
        })
    }

    /// Checks one exact source marker on the current connection.
    pub fn recorded_session_matches(&self, source: &RecordedSessionSource) -> Result<bool> {
        recorded_session_matches_in(&self.connection, source)
    }

    /// Adds one source-verified legacy component baseline to the current
    /// absolute totals. The totals, one-shot marker, and data generation move
    /// in the same transaction, so a crash or acknowledgement retry cannot
    /// apply the baseline twice.
    pub fn apply_history_continuity_model_totals(
        &mut self,
        recovery: &HistoryContinuityModelRecovery,
    ) -> Result<u64> {
        let recovered = canonicalize_model_totals(&recovery.model_totals)?;
        let authority = &recovery.authority;
        let recovered_tokens = |model: &str| {
            recovered
                .iter()
                .find(|total| total.model == model)
                .map(|total| total.total_tokens)
                .unwrap_or(0)
        };
        if recovered_tokens("SOL") != authority.sol_tokens
            || recovered_tokens("TERRA") != authority.terra_tokens
            || recovered_tokens("LUNA") != authority.luna_tokens
        {
            return Err(UsageStoreError::InvalidImport(
                "legacy component recovery token totals mismatch".into(),
            ));
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let continuity = load_history_continuity(&transaction)?.ok_or_else(|| {
            UsageStoreError::InvalidImport("history continuity recovery is missing".into())
        })?;
        let durable_recovery = HistoryContinuityRecovery {
            source_fingerprint: continuity.source_fingerprint.clone(),
            source_rows: continuity.source_rows,
            boundary_timestamp: continuity.boundary_timestamp,
            reset_at: continuity.reset_at,
            sol_dollars: continuity.sol_dollars,
            terra_dollars: continuity.terra_dollars,
            luna_dollars: continuity.luna_dollars,
            sol_tokens: continuity.sol_tokens,
            terra_tokens: continuity.terra_tokens,
            luna_tokens: continuity.luna_tokens,
        };
        if &durable_recovery != authority {
            return Err(UsageStoreError::InvalidImport(
                "history continuity recovery authority changed".into(),
            ));
        }
        let (generation, current_reset): (String, i64) = transaction.query_row(
            "SELECT data_generation, reset_at
             FROM collection_generation WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let generation = canonical_u64_text(&generation, "collection generation")?;
        if continuity.model_totals_applied {
            transaction.commit()?;
            return Ok(generation);
        }
        if !authority.matches_reset_at(current_reset) {
            return Err(UsageStoreError::InvalidImport(
                "history continuity recovery period changed".into(),
            ));
        }

        let current = {
            let mut statement = transaction.prepare(
                "SELECT model, total_tokens, input_tokens, cached_input_tokens, output_tokens, cache_write_input_tokens
                 FROM session_model_totals ORDER BY model",
            )?;
            let rows = statement.query_map([], |row| {
                let total_tokens = row.get::<_, String>(1)?;
                let input_tokens = row.get::<_, String>(2)?;
                let cached_input_tokens = row.get::<_, String>(3)?;
                let output_tokens = row.get::<_, String>(4)?;
                Ok(SessionModelTotal {
                    model: row.get(0)?,
                    cache_write_input_tokens: row
                        .get::<_, Option<String>>(5)?
                        .map(|text| {
                            text.parse::<u64>()
                                .map_err(|_| rusqlite::Error::InvalidQuery)
                        })
                        .transpose()?,
                    total_tokens: total_tokens
                        .parse::<u64>()
                        .ok()
                        .filter(|value| value.to_string() == total_tokens)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    input_tokens: input_tokens
                        .parse::<u64>()
                        .ok()
                        .filter(|value| value.to_string() == input_tokens)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    cached_input_tokens: cached_input_tokens
                        .parse::<u64>()
                        .ok()
                        .filter(|value| value.to_string() == cached_input_tokens)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    output_tokens: output_tokens
                        .parse::<u64>()
                        .ok()
                        .filter(|value| value.to_string() == output_tokens)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut combined = canonicalize_model_totals(&current)?
            .into_iter()
            .map(|total| (total.model.clone(), total))
            .collect::<BTreeMap<_, _>>();
        for offset in &recovered {
            let total = combined
                .entry(offset.model.clone())
                .or_insert_with(|| SessionModelTotal {
                    model: offset.model.clone(),
                    total_tokens: 0,
                    input_tokens: 0,
                    cached_input_tokens: 0,
                    output_tokens: 0,
                    cache_write_input_tokens: Some(0),
                });
            total.cache_write_input_tokens = match (
                total.cache_write_input_tokens,
                offset.cache_write_input_tokens,
            ) {
                (Some(left), Some(right)) => Some(
                    left.checked_add(right)
                        .ok_or(UsageStoreError::GenerationOverflow)?,
                ),
                _ => None,
            };
            total.total_tokens = total
                .total_tokens
                .checked_add(offset.total_tokens)
                .ok_or(UsageStoreError::GenerationOverflow)?;
            total.input_tokens = total
                .input_tokens
                .checked_add(offset.input_tokens)
                .ok_or(UsageStoreError::GenerationOverflow)?;
            total.cached_input_tokens = total
                .cached_input_tokens
                .checked_add(offset.cached_input_tokens)
                .ok_or(UsageStoreError::GenerationOverflow)?;
            total.output_tokens = total
                .output_tokens
                .checked_add(offset.output_tokens)
                .ok_or(UsageStoreError::GenerationOverflow)?;
        }
        let combined = canonicalize_model_totals(&combined.into_values().collect::<Vec<_>>())?;
        transaction.execute("DELETE FROM session_model_totals", [])?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO session_model_totals (
                    model, total_tokens, input_tokens, cached_input_tokens, output_tokens, cache_write_input_tokens
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for total in &combined {
                statement.execute(params![
                    &total.model,
                    total.total_tokens.to_string(),
                    total.input_tokens.to_string(),
                    total.cached_input_tokens.to_string(),
                    total.output_tokens.to_string(),
                    total
                        .cache_write_input_tokens
                        .map(|value| value.to_string()),
                ])?;
            }
        }
        transaction.execute(
            "UPDATE history_continuity SET model_totals_applied=1 WHERE singleton=1",
            [],
        )?;
        let next = generation
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        transaction.execute(
            "UPDATE collection_generation SET data_generation=?1 WHERE singleton=1",
            [next.to_string()],
        )?;
        transaction.commit()?;
        Ok(next)
    }

    /// Removes only markers whose complete identity still matches.
    ///
    /// This is marker lifecycle maintenance after source unlink; it never
    /// changes usage history or durable state. A stale/replaced marker yields
    /// zero affected rows rather than deleting a newer source's authority.
    pub fn forget_recorded_sessions(&mut self, sources: &[RecordedSessionSource]) -> Result<usize> {
        let sources = canonicalize_recorded_sessions(sources)?;
        if sources.is_empty() {
            return Ok(0);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut removed = 0usize;
        {
            let mut statement = transaction.prepare(
                "DELETE FROM recorded_sessions
                 WHERE root_identity = ?1
                   AND relative_path = ?2
                   AND file_bytes = ?3
                   AND modified_nanos = ?4
                   AND file_device = ?5
                   AND file_inode = ?6",
            )?;
            for source in &sources {
                removed = removed.saturating_add(statement.execute(params![
                    &source.root_identity,
                    &source.relative_path,
                    source.file_bytes as i64,
                    source.modified_nanos.to_string(),
                    source.file_device.to_string(),
                    source.file_inode.to_string(),
                ])?);
            }
        }
        transaction.commit()?;
        Ok(removed)
    }

    /// Inserts already-decoded samples, replacing rows with matching keys.
    ///
    /// Validation and exact-key canonicalization happen inside the immediate
    /// transaction before any row is changed, and writes are atomic.
    pub fn import_samples(&mut self, samples: &[UsageHistorySample]) -> Result<usize> {
        self.upsert_samples(samples)?;
        Ok(samples.len())
    }

    fn commit_durable_state_inner(
        &mut self,
        expected_generation: Option<u64>,
        samples: &[UsageHistorySample],
        data_hash: &str,
        snapshot_json: &str,
    ) -> Result<DurableRecord> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let canonical = canonicalize_samples(&transaction, samples)?;
        validate_data_hash(data_hash)?;
        validate_snapshot_json(snapshot_json)?;
        let current_raw: Option<(i64, String, String)> = transaction
            .query_row(
                "SELECT data_generation, data_hash, snapshot_json \
                 FROM durable_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let current = current_raw
            .map(|(data_generation, data_hash, snapshot_json)| {
                durable_record_from_sql(data_generation, data_hash, snapshot_json)
            })
            .transpose()?;
        let current_generation = current
            .as_ref()
            .map(|record| record.data_generation)
            .unwrap_or(0);
        if let Some(expected_generation) = expected_generation {
            if expected_generation != current_generation {
                return Err(UsageStoreError::GenerationConflict {
                    expected: expected_generation,
                    actual: current_generation,
                });
            }
        }
        let next_generation = current_generation
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        let sqlite_generation =
            i64::try_from(next_generation).map_err(|_| UsageStoreError::GenerationOverflow)?;

        upsert_canonical_samples(&transaction, &canonical)?;
        transaction.execute(
            "INSERT INTO durable_state (singleton, data_generation, data_hash, snapshot_json) \
             VALUES (1, ?1, ?2, ?3) \
             ON CONFLICT (singleton) DO UPDATE SET \
                 data_generation = excluded.data_generation, \
                 data_hash = excluded.data_hash, \
                 snapshot_json = excluded.snapshot_json",
            params![sqlite_generation, data_hash, snapshot_json],
        )?;
        transaction.commit()?;

        Ok(DurableRecord {
            data_generation: next_generation,
            data_hash: data_hash.to_owned(),
            snapshot_json: snapshot_json.to_owned(),
        })
    }

    /// Atomically upserts `samples` and commits the next durable snapshot.
    /// The first committed generation is one; all validation occurs before
    /// the transaction can change either history or durable state.
    pub fn commit_durable_state<H: AsRef<str>, J: AsRef<str>>(
        &mut self,
        samples: &[UsageHistorySample],
        data_hash: H,
        snapshot_json: J,
    ) -> Result<DurableRecord> {
        self.commit_durable_state_inner(None, samples, data_hash.as_ref(), snapshot_json.as_ref())
    }

    /// Atomically commits only when the currently stored generation matches
    /// `expected_generation`; zero denotes an empty durable-state table.
    pub fn commit_durable_state_if_generation<H: AsRef<str>, J: AsRef<str>>(
        &mut self,
        expected_generation: u64,
        samples: &[UsageHistorySample],
        data_hash: H,
        snapshot_json: J,
    ) -> Result<DurableRecord> {
        self.commit_durable_state_inner(
            Some(expected_generation),
            samples,
            data_hash.as_ref(),
            snapshot_json.as_ref(),
        )
    }

    /// Descriptive alias for the optimistic-generation commit operation.
    pub fn commit_durable_state_with_expected_generation<H: AsRef<str>, J: AsRef<str>>(
        &mut self,
        expected_generation: u64,
        samples: &[UsageHistorySample],
        data_hash: H,
        snapshot_json: J,
    ) -> Result<DurableRecord> {
        self.commit_durable_state_if_generation(
            expected_generation,
            samples,
            data_hash,
            snapshot_json,
        )
    }

    /// Reads and validates the singleton durable snapshot, if one exists.
    pub fn load_durable_record(&self) -> Result<Option<DurableRecord>> {
        let raw: Option<(i64, String, String)> = self
            .connection
            .query_row(
                "SELECT data_generation, data_hash, snapshot_json \
                 FROM durable_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        raw.map(|(data_generation, data_hash, snapshot_json)| {
            durable_record_from_sql(data_generation, data_hash, snapshot_json)
        })
        .transpose()
    }

    /// Alias for callers that refer to the table as durable state.
    pub fn load_durable_state(&self) -> Result<Option<DurableRecord>> {
        self.load_durable_record()
    }

    /// Removes observations older than the exclusive UTC calendar-month cutoff.
    ///
    /// This is the only destructive usage-history operation in the store.
    /// Exact recorded-source marker lifecycle is independent. The cutoff is
    /// strictly exclusive, so observations at the cutoff or in the future
    /// remain stored regardless of reset period.
    pub fn prune_older_than_three_months(&mut self, now: DateTime<Utc>) -> Result<usize> {
        let cutoff = three_months_before(now).timestamp();
        let transaction = self.connection.transaction()?;
        let deleted = transaction.execute(
            "DELETE FROM usage_history WHERE timestamp < ?1",
            params![cutoff],
        )?;
        transaction.execute(
            "DELETE FROM durable_state
             WHERE singleton >= ?1 AND data_generation < ?2",
            params![DURABLE_STATE_OBSERVATION_MIN_SINGLETON, cutoff],
        )?;
        transaction.execute(
            "DELETE FROM usage_model_history WHERE timestamp < ?1",
            params![cutoff],
        )?;
        transaction.commit()?;
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::Instant;

    fn database_path(test_name: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "codex-info-usage-store-{test_name}-{}-{id}",
                std::process::id()
            ))
            .join("nested")
            .join("usage.sqlite3")
    }

    fn remove_database(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::remove_dir_all(parent.parent().unwrap_or(parent))
                .expect("failed to remove test database directory");
        }
    }

    fn sample(
        timestamp: i64,
        reset_at: i64,
        remaining_percent: Option<f64>,
        sol_dollars: f64,
    ) -> UsageHistorySample {
        UsageHistorySample {
            timestamp,
            reset_at,
            remaining_percent,
            sol_dollars,
            terra_dollars: 2.0,
            luna_dollars: 3.0,
            sol_tokens: 11,
            terra_tokens: 22,
            luna_tokens: 33,
        }
    }

    fn recorded_source(relative_path: &str, inode: u64) -> RecordedSessionSource {
        RecordedSessionSource {
            root_identity: "unix:10:20".into(),
            relative_path: relative_path.into(),
            file_bytes: 123,
            modified_nanos: 1_700_000_000_000_000_000,
            file_device: 10,
            file_inode: inode,
        }
    }

    fn partition_identity(account_byte: char, epoch: u64) -> StoragePartitionIdentity {
        StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: account_byte.to_string().repeat(64),
            storage_epoch: epoch,
            partition_id: account_byte.to_string().repeat(64),
        }
    }

    fn recorder_gap(
        partition_id: &str,
        id: char,
        state: &str,
        start_at: i64,
        end_at: i64,
    ) -> RecorderGap {
        RecorderGap {
            gap_id: id.to_string().repeat(32),
            partition_id: partition_id.to_owned(),
            source_identity_before: "source-before".into(),
            source_identity_after: "source-after".into(),
            cursor_before: "cursor-before".into(),
            cursor_after: "cursor-after".into(),
            stopped_at_monotonic_ns: 100,
            resumed_at_monotonic_ns: Some(200),
            start_at,
            end_at,
            reset_at: Some(1_800_604_800),
            reason: "daemon_stop_unrecoverable".into(),
            state: state.into(),
            owner_collector_epoch: 0x1234,
            confirmation_cycle_seq: 1,
        }
    }

    fn checkpoint(source: &RecordedSessionSource, offset: u64) -> SessionCheckpoint {
        SessionCheckpoint {
            previous_cache_write_input: None,
            root_identity: source.root_identity.clone(),
            relative_path: source.relative_path.clone(),
            file_device: source.file_device,
            file_inode: source.file_inode,
            committed_offset: offset,
            discard_until_lf: false,
            collector_epoch: 0x1234,
            cycle_seq: 1,
            prefix_generation: 0x5678,
            prefix_sha256: "00".repeat(32),
            fully_attributed_from_zero: true,
            token_baseline_known: true,
            last_model: Some("SOL".into()),
            last_task_running: None,
            previous_total: 20,
            previous_input: 12,
            previous_cached_input: 2,
            previous_output: 8,
        }
    }

    #[test]
    fn account_schema_v1_migrates_without_loss_and_roundtrips_unknown_models() {
        let path = database_path("partition-session-running-state");
        let identity = partition_identity('d', 18);
        let reset_at = 1_800_604_800;
        let source = recorded_source("2026/09/session-running.jsonl", 30);
        let checkpoint = checkpoint(&source, 10);
        let legacy_total = SessionModelTotal {
            cache_write_input_tokens: None,
            model: "SOL".into(),
            total_tokens: 20,
            input_tokens: 12,
            cached_input_tokens: 2,
            output_tokens: 8,
        };
        let legacy_sample = sample(1_800_000_000, reset_at, Some(70.0), 3.0);

        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        store
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: checkpoint.collector_epoch,
                cycle_seq: checkpoint.cycle_seq,
                samples: std::slice::from_ref(&legacy_sample),
                checkpoints: std::slice::from_ref(&checkpoint),
                ranges: &[],
                model_totals: std::slice::from_ref(&legacy_total),
                recorded_sessions: &[],
            })
            .unwrap();
        let legacy_history = store.load_all().unwrap();
        let legacy_state = store.load_session_collection_state().unwrap();
        store
            .connection
            .execute(
                "ALTER TABLE session_checkpoints DROP COLUMN last_task_running",
                [],
            )
            .unwrap();
        store
            .connection
            .execute(
                "ALTER TABLE session_checkpoints DROP COLUMN previous_cache_write_input",
                [],
            )
            .unwrap();
        store
            .connection
            .execute(
                "ALTER TABLE session_model_totals DROP COLUMN cache_write_input_tokens",
                [],
            )
            .unwrap();
        store
            .connection
            .execute("DROP TABLE usage_model_history", [])
            .unwrap();
        store
            .connection
            .pragma_update(None, "user_version", 0)
            .unwrap();
        drop(store);

        let legacy_reader = UsageStore::open_read_only_partitioned(&path, &identity).unwrap();
        assert_eq!(legacy_reader.load_all().unwrap(), legacy_history);
        let legacy_read_state = legacy_reader.load_session_collection_state().unwrap();
        assert_eq!(legacy_read_state, legacy_state);
        assert_eq!(legacy_read_state.checkpoints.len(), 1);
        assert_eq!(legacy_read_state.checkpoints[0].last_task_running, None);
        assert_eq!(
            legacy_read_state.checkpoints[0].previous_cache_write_input,
            None
        );
        assert_eq!(legacy_read_state.model_totals, [legacy_total.clone()]);
        drop(legacy_reader);

        // Retained generations from the previous executable remain valid
        // recovery inputs; only the writable current DB is migrated.
        UsageStore::backup_generations_partitioned(&path, &identity, 1).unwrap();

        let mut migrated = UsageStore::open_partitioned(&path, &identity).unwrap();
        let migrated_state = migrated.load_session_collection_state().unwrap();
        assert_eq!(migrated.load_all().unwrap(), legacy_history);
        assert_eq!(migrated_state, legacy_state);
        assert_eq!(migrated_state.checkpoints.len(), 1);
        assert_eq!(migrated_state.checkpoints[0].committed_offset, 10);
        assert_eq!(migrated_state.checkpoints[0].last_task_running, None);
        assert_eq!(
            migrated_state.checkpoints[0].previous_cache_write_input,
            None
        );
        assert_eq!(migrated_state.model_totals, [legacy_total]);

        let mut running = checkpoint.clone();
        running.last_model = Some("gpt-7-nova".into());
        running.last_task_running = Some(true);
        running.previous_cache_write_input = Some(0);
        running.cycle_seq = 2;
        let astra_total = SessionModelTotal {
            cache_write_input_tokens: Some(7),
            model: "ASTRA".into(),
            total_tokens: 20,
            input_tokens: 12,
            cached_input_tokens: 2,
            output_tokens: 8,
        };
        let sol_total = SessionModelTotal {
            cache_write_input_tokens: Some(0),
            model: "SOL".into(),
            total_tokens: 20,
            input_tokens: 12,
            cached_input_tokens: 2,
            output_tokens: 8,
        };
        let terra_total = SessionModelTotal {
            cache_write_input_tokens: None,
            model: "TERRA".into(),
            total_tokens: 30,
            input_tokens: 20,
            cached_input_tokens: 5,
            output_tokens: 10,
        };
        let future_total = SessionModelTotal {
            cache_write_input_tokens: Some(3),
            model: "gpt-7-nova".into(),
            total_tokens: 15,
            input_tokens: 10,
            cached_input_tokens: 2,
            output_tokens: 5,
        };
        let current_totals = [
            astra_total.clone(),
            sol_total.clone(),
            terra_total.clone(),
            future_total.clone(),
        ];
        let model_observation =
            UsageHistoryObservation::confirmed_with_models(&legacy_sample, current_totals.to_vec());
        migrated
            .commit_session_collection_with_observations(
                SessionCollectionCommit {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: running.collector_epoch,
                    cycle_seq: 2,
                    samples: &[],
                    checkpoints: std::slice::from_ref(&running),
                    ranges: &[],
                    model_totals: &current_totals,
                    recorded_sessions: &[],
                },
                std::slice::from_ref(&model_observation),
            )
            .unwrap();
        let roundtripped = migrated.load_session_collection_state().unwrap();
        assert_eq!(roundtripped.checkpoints[0].last_task_running, Some(true));
        assert_eq!(
            roundtripped.checkpoints[0].last_model,
            Some("gpt-7-nova".into())
        );
        assert_eq!(
            roundtripped.checkpoints[0].previous_cache_write_input,
            Some(0)
        );
        assert_eq!(roundtripped.model_totals, current_totals);
        let observations = migrated
            .load_recent_observations(Utc.timestamp_opt(1_800_000_060, 0).unwrap())
            .unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0].model_totals.as_deref(),
            Some(current_totals.as_slice())
        );
        let schema_version: i64 = migrated
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schema_version, ACCOUNT_DB_SCHEMA_VERSION);

        migrated
            .connection
            .pragma_update(None, "user_version", ACCOUNT_DB_SCHEMA_VERSION + 1)
            .unwrap();
        drop(migrated);
        assert!(UsageStore::open_read_only_partitioned(&path, &identity).is_err());

        remove_database(&path);
    }

    #[test]
    fn account_partitions_isolate_same_keys_metadata_backups_and_gap_ledgers() {
        let path_a = database_path("partition-a");
        let path_b = database_path("partition-b");
        let identity_a = partition_identity('a', 1);
        let identity_b = partition_identity('b', 2);
        let mut store_a = UsageStore::create_partitioned(&path_a, &identity_a).unwrap();
        let mut store_b = UsageStore::create_partitioned(&path_b, &identity_b).unwrap();
        let reset_at = 1_800_604_800;
        let sample_a = sample(1_800_000_000, reset_at, Some(70.0), 1.0);
        let sample_b = sample(1_800_000_000, reset_at, Some(30.0), 9.0);
        let source_a = recorded_source("same/session.jsonl", 30);
        let source_b = recorded_source("same/session.jsonl", 30);
        let mut checkpoint_a = checkpoint(&source_a, 10);
        checkpoint_a.collector_epoch = 1;
        let mut checkpoint_b = checkpoint(&source_b, 20);
        checkpoint_b.collector_epoch = 2;

        store_a
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 1,
                cycle_seq: 1,
                samples: std::slice::from_ref(&sample_a),
                checkpoints: std::slice::from_ref(&checkpoint_a),
                ranges: &[],
                model_totals: &[],
                recorded_sessions: &[],
            })
            .unwrap();
        store_b
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 2,
                cycle_seq: 1,
                samples: std::slice::from_ref(&sample_b),
                checkpoints: std::slice::from_ref(&checkpoint_b),
                ranges: &[],
                model_totals: &[],
                recorded_sessions: &[],
            })
            .unwrap();
        store_a
            .begin_recorder_gap(&RecorderGap {
                gap_id: "01".repeat(16),
                partition_id: identity_a.partition_id.clone(),
                source_identity_before: "account-a-source".into(),
                source_identity_after: "account-a-source".into(),
                cursor_before: "account-a-cursor".into(),
                cursor_after: "account-a-cursor".into(),
                stopped_at_monotonic_ns: 1,
                resumed_at_monotonic_ns: None,
                start_at: 1_800_000_000,
                end_at: 1_800_000_060,
                reset_at: Some(reset_at),
                reason: "daemon_stop_unrecoverable".into(),
                state: "pending".into(),
                owner_collector_epoch: 1,
                confirmation_cycle_seq: 1,
            })
            .unwrap();
        store_b
            .begin_recorder_gap(&RecorderGap {
                gap_id: "02".repeat(16),
                partition_id: identity_b.partition_id.clone(),
                source_identity_before: "account-b-source".into(),
                source_identity_after: "account-b-source".into(),
                cursor_before: "account-b-cursor".into(),
                cursor_after: "account-b-cursor".into(),
                stopped_at_monotonic_ns: 2,
                resumed_at_monotonic_ns: None,
                start_at: 1_800_000_000,
                end_at: 1_800_000_060,
                reset_at: Some(reset_at),
                reason: "daemon_stop_unrecoverable".into(),
                state: "pending".into(),
                owner_collector_epoch: 2,
                confirmation_cycle_seq: 1,
            })
            .unwrap();
        drop((store_a, store_b));

        let opened_a = UsageStore::open_read_only_partitioned(&path_a, &identity_a).unwrap();
        let opened_b = UsageStore::open_read_only_partitioned(&path_b, &identity_b).unwrap();
        assert_eq!(opened_a.load_all().unwrap(), vec![sample_a]);
        assert_eq!(opened_b.load_all().unwrap(), vec![sample_b]);
        assert_eq!(
            opened_a
                .load_session_collection_state()
                .unwrap()
                .checkpoints,
            vec![checkpoint_a]
        );
        assert_eq!(
            opened_b
                .load_session_collection_state()
                .unwrap()
                .checkpoints,
            vec![checkpoint_b]
        );
        let gap_a = opened_a.load_recorder_gaps().unwrap();
        let gap_b = opened_b.load_recorder_gaps().unwrap();
        assert_eq!(gap_a.len(), 1);
        assert_eq!(gap_b.len(), 1);
        assert_eq!(gap_a[0].partition_id, identity_a.partition_id);
        assert_eq!(gap_b[0].partition_id, identity_b.partition_id);
        assert_eq!(gap_a[0].reason, "daemon_stop_unrecoverable");
        assert_eq!(gap_b[0].reason, "daemon_stop_unrecoverable");
        drop((opened_a, opened_b));
        assert!(UsageStore::open_partitioned(&path_a, &identity_b).is_err());

        UsageStore::backup_generations_partitioned(&path_a, &identity_a, 3).unwrap();
        let backup_a = path_a.with_extension("sqlite3.bak.1");
        assert!(backup_a.is_file());
        assert!(!path_b.with_extension("sqlite3.bak.1").exists());
        assert!(UsageStore::open_read_only_partitioned(&backup_a, &identity_a).is_ok());
        assert!(UsageStore::open_read_only_partitioned(&backup_a, &identity_b).is_err());

        remove_database(&path_a);
        remove_database(&path_b);
    }

    #[test]
    fn recorder_gap_ledger_is_idempotent_and_projects_only_confirmed_source_proof() {
        let path = database_path("recorder-gap-authority");
        let identity = partition_identity('c', 3);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();

        let pending = recorder_gap(
            &identity.partition_id,
            '1',
            "pending",
            1_800_000_000,
            1_800_000_060,
        );
        store.begin_recorder_gap(&pending).unwrap();
        // Exact replay is accepted, while any changed logical field is a
        // contradiction rather than a second interpretation of the interval.
        store.begin_recorder_gap(&pending).unwrap();
        let mut conflicting = pending.clone();
        conflicting.cursor_before = "different-cursor".into();
        assert!(store.begin_recorder_gap(&conflicting).is_err());

        let mut recovered = pending.clone();
        recovered.state = "recovered".into();
        store.recover_recorder_gap(&recovered).unwrap();
        assert!(store.load_confirmed_recorder_gaps().unwrap().is_empty());

        let pending_confirmed = recorder_gap(
            &identity.partition_id,
            '2',
            "pending",
            1_800_000_120,
            1_800_000_180,
        );
        store.begin_recorder_gap(&pending_confirmed).unwrap();
        let mut confirmed = pending_confirmed.clone();
        confirmed.state = "confirmed".into();
        store.confirm_recorder_gap(&confirmed).unwrap();
        // Confirmed replay is idempotent and is the only state projected by
        // the public-gap read helper.
        store.confirm_recorder_gap(&confirmed).unwrap();
        let public = store.load_confirmed_recorder_gaps().unwrap();
        assert_eq!(public, vec![confirmed.clone()]);

        let overlapping_pending = recorder_gap(
            &identity.partition_id,
            '3',
            "pending",
            1_800_000_150,
            1_800_000_210,
        );
        store.begin_recorder_gap(&overlapping_pending).unwrap();
        let mut overlapping_confirmed = overlapping_pending.clone();
        overlapping_confirmed.state = "confirmed".into();
        assert!(store.confirm_recorder_gap(&overlapping_confirmed).is_err());
        assert_eq!(
            store.load_confirmed_recorder_gaps().unwrap(),
            vec![confirmed.clone()]
        );

        let pending_rejected = recorder_gap(
            &identity.partition_id,
            '4',
            "pending",
            1_800_000_300,
            1_800_000_360,
        );
        store.begin_recorder_gap(&pending_rejected).unwrap();
        let mut rejected = pending_rejected;
        rejected.state = "rejected".into();
        store.record_recorder_gap(&rejected).unwrap();
        assert_eq!(
            store.load_confirmed_recorder_gaps().unwrap(),
            vec![confirmed]
        );
        remove_database(&path);
    }

    #[test]
    fn recorder_gap_source_rescan_reaches_recovered_confirmed_rejected_idempotently() {
        let path = database_path("recorder-gap-source-rescan");
        let identity = partition_identity('e', 5);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let reset_at = 1_800_604_800;

        let recovered_pending = recorder_gap(
            &identity.partition_id,
            '5',
            "pending",
            1_800_000_000,
            1_800_000_180,
        );
        store.begin_recorder_gap(&recovered_pending).unwrap();
        let recovered = store
            .reconcile_pending_recorder_gaps(
                "authenticated-quota:partition-e",
                "collector:00000000000000000000000000005678:cycle:2",
                200,
                reset_at,
                0x5678,
                2,
                &[1_800_000_060, 1_800_000_120, 1_800_000_180],
                false,
            )
            .unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, "recovered");
        assert!(store.load_confirmed_recorder_gaps().unwrap().is_empty());

        let mut confirmed_pending = recorder_gap(
            &identity.partition_id,
            '6',
            "pending",
            1_800_000_240,
            1_800_000_360,
        );
        confirmed_pending.resumed_at_monotonic_ns = None;
        store.begin_recorder_gap(&confirmed_pending).unwrap();
        let confirmed = store
            .reconcile_pending_recorder_gaps(
                "authenticated-quota:partition-e",
                "collector:00000000000000000000000000009999:cycle:3",
                300,
                reset_at,
                0x9999,
                3,
                &[],
                true,
            )
            .unwrap();
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].state, "confirmed");
        assert_eq!(store.load_confirmed_recorder_gaps().unwrap().len(), 1);

        // A reset-period contradiction is a source proof that the pending
        // interval cannot be attributed to the current period. It is
        // retained as rejected and never crosses the public projection.
        let mut rejected_pending = recorder_gap(
            &identity.partition_id,
            '7',
            "pending",
            1_800_000_420,
            1_800_000_480,
        );
        rejected_pending.resumed_at_monotonic_ns = None;
        store.begin_recorder_gap(&rejected_pending).unwrap();
        let rejected = store
            .reconcile_pending_recorder_gaps(
                "authenticated-quota:partition-e",
                "collector:0000000000000000000000000000aaaa:cycle:4",
                400,
                reset_at + 604_800,
                0xaaaa,
                4,
                &[],
                false,
            )
            .unwrap();
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].state, "rejected");
        assert_eq!(store.load_confirmed_recorder_gaps().unwrap().len(), 1);

        let mut overlapping_pending = recorder_gap(
            &identity.partition_id,
            '8',
            "pending",
            1_800_000_300,
            1_800_000_420,
        );
        overlapping_pending.resumed_at_monotonic_ns = None;
        store.begin_recorder_gap(&overlapping_pending).unwrap();
        let overlap_result = store
            .reconcile_pending_recorder_gaps(
                "authenticated-quota:partition-e",
                "collector:0000000000000000000000000000bbbb:cycle:5",
                500,
                reset_at,
                0xbbbb,
                5,
                &[],
                true,
            )
            .unwrap();
        assert_eq!(overlap_result.len(), 1);
        assert_eq!(overlap_result[0].state, "rejected");
        assert_eq!(store.load_confirmed_recorder_gaps().unwrap().len(), 1);

        // The same source result is a no-op after terminal persistence: no
        // duplicate row or second public interval is created.
        assert!(store
            .reconcile_pending_recorder_gaps(
                "authenticated-quota:partition-e",
                "collector:0000000000000000000000000000aaaa:cycle:4",
                400,
                reset_at + 604_800,
                0xaaaa,
                4,
                &[],
                false,
            )
            .unwrap()
            .is_empty());
        let all = store.load_recorder_gaps().unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(
            all.iter().map(|gap| gap.state.as_str()).collect::<Vec<_>>(),
            vec!["recovered", "confirmed", "rejected", "rejected"]
        );
        remove_database(&path);
    }

    #[test]
    fn persisted_restart_gap_accepts_new_boot_monotonic_value_without_reversal() {
        let path = database_path("restart-gap-monotonic");
        let identity = partition_identity('a', 6);
        let stopped_at_monotonic_ns = 9_000_100_000_001;
        let mut pending = recorder_gap(
            &identity.partition_id,
            '9',
            "pending",
            1_800_000_000,
            1_800_000_180,
        );
        pending.resumed_at_monotonic_ns = None;
        pending.stopped_at_monotonic_ns = stopped_at_monotonic_ns;

        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        store.begin_recorder_gap(&pending).unwrap();
        drop(store);

        // Reopening the same SQLite file models a new process consuming a
        // previous owner's persisted stop marker. A lower value is rejected
        // by the DB invariant; the next boot-wide value transitions safely.
        let mut restarted = UsageStore::open_partitioned(&path, &identity).unwrap();
        let mut reversed = pending.clone();
        reversed.state = "recovered".into();
        reversed.resumed_at_monotonic_ns = Some(stopped_at_monotonic_ns - 1);
        assert!(restarted.recover_recorder_gap(&reversed).is_err());
        assert_eq!(
            restarted
                .load_recorder_gaps()
                .unwrap()
                .first()
                .map(|gap| gap.state.as_str()),
            Some("pending")
        );

        let mut resumed = pending;
        resumed.state = "recovered".into();
        resumed.resumed_at_monotonic_ns = Some(stopped_at_monotonic_ns + 1);
        restarted.recover_recorder_gap(&resumed).unwrap();
        let persisted = restarted.load_recorder_gaps().unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].state, "recovered");
        assert_eq!(
            persisted[0].resumed_at_monotonic_ns,
            Some(stopped_at_monotonic_ns + 1)
        );
        remove_database(&path);
    }

    #[test]
    fn legacy_gap_ledger_migrates_transactionally_without_rewriting_history_or_sessions() {
        let path = database_path("legacy-gap-migration");
        let identity = partition_identity('d', 4);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection.execute_batch(PARTITION_SCHEMA).unwrap();
        connection
            .execute("DROP TABLE recorder_gap_ledger", [])
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE recorder_gap_ledger (
                    data_generation TEXT PRIMARY KEY,
                    observed_at INTEGER,
                    reason TEXT
                )",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO storage_partition (
                    singleton, schema_version, profile_scope_id, account_scope_id,
                    storage_epoch, partition_id
                 ) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
                params![
                    &identity.schema_version,
                    &identity.profile_scope_id,
                    &identity.account_scope_id,
                    identity.storage_epoch.to_string(),
                    &identity.partition_id,
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO usage_history (
                    timestamp, reset_at, remaining_percent, sol_dollars,
                    terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
                 ) VALUES (1_800_000_000, 1_800_604_800, 70.0, 1.0, 2.0, 3.0, 11, 22, 33)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO recorded_sessions (
                    root_identity, relative_path, file_bytes, modified_nanos,
                    file_device, file_inode
                 ) VALUES ('unix:10:20', 'same/session.jsonl', 123, '1700000000000000000', 10, 20)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO recorder_gap_ledger(data_generation, observed_at, reason)
                 VALUES ('legacy-generation', 1_800_000_060, 'fixture')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO recorder_gap_ledger(data_generation, observed_at, reason)
                 VALUES ('invalid-timestamp', 0, 'fixture')",
                [],
            )
            .unwrap();
        drop(connection);
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let opened = UsageStore::open_partitioned(&path, &identity).unwrap();
        assert_eq!(opened.load_all().unwrap().len(), 1);
        let recorded_count: i64 = opened
            .connection
            .query_row("SELECT COUNT(*) FROM recorded_sessions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(recorded_count, 1);
        let migrated = opened.load_recorder_gaps().unwrap();
        assert_eq!(migrated.len(), 2);
        assert!(migrated.iter().all(|gap| gap.state == "rejected"));
        assert!(migrated
            .iter()
            .all(|gap| gap.reason == "auth_epoch_tombstoned"));
        assert!(migrated.iter().all(|gap| gap.reset_at.is_some()));
        assert!(migrated.iter().any(|gap| gap.start_at == 1));
        assert!(migrated.iter().any(|gap| gap.start_at == 1_800_000_060));
        remove_database(&path);
    }

    #[test]
    fn session_range_checkpoint_marker_and_generation_commit_atomically() {
        let path = database_path("partition-session-atomic");
        let identity = partition_identity('c', 3);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let reset_at = 1_800_604_800;
        let mut source = recorded_source("2026/09/session.jsonl", 30);
        source.file_bytes = 10;
        let mut checkpoint = checkpoint(&source, 10);
        checkpoint.last_model = Some("ASTRA".into());
        checkpoint.previous_cache_write_input = Some(0);
        let range = SessionRange {
            root_identity: source.root_identity.clone(),
            relative_path: source.relative_path.clone(),
            file_device: source.file_device,
            file_inode: source.file_inode,
            start_offset: 0,
            end_offset: 10,
            collector_epoch: 0x1234,
            cycle_seq: 1,
            prefix_generation: 0x5678,
            record_sha256: "11".repeat(32),
        };
        let committed_sample = sample(1_800_000_000, reset_at, Some(50.0), 3.0);
        let expected_model_totals = [
            SessionModelTotal {
                cache_write_input_tokens: Some(7),
                model: "ASTRA".into(),
                total_tokens: 20,
                input_tokens: 12,
                cached_input_tokens: 2,
                output_tokens: 8,
            },
            SessionModelTotal {
                cache_write_input_tokens: None,
                model: "SOL".into(),
                total_tokens: 20,
                input_tokens: 12,
                cached_input_tokens: 2,
                output_tokens: 8,
            },
        ];
        let committed = store
            .commit_session_collection_with_samples(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 0x1234,
                cycle_seq: 1,
                samples: std::slice::from_ref(&committed_sample),
                checkpoints: std::slice::from_ref(&checkpoint),
                ranges: std::slice::from_ref(&range),
                model_totals: &expected_model_totals,
                recorded_sessions: std::slice::from_ref(&source),
            })
            .unwrap();
        assert_eq!(committed.data_generation, 1);
        assert_eq!(committed.canonical_samples, [committed_sample.clone()]);
        let replayed = store
            .commit_session_collection_with_samples(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 0x1234,
                cycle_seq: 1,
                samples: std::slice::from_ref(&committed_sample),
                checkpoints: std::slice::from_ref(&checkpoint),
                ranges: std::slice::from_ref(&range),
                model_totals: &expected_model_totals,
                recorded_sessions: std::slice::from_ref(&source),
            })
            .unwrap();
        assert_eq!(replayed.data_generation, committed.data_generation);
        assert_eq!(replayed.canonical_samples, committed.canonical_samples);
        assert_eq!(
            store
                .load_session_collection_state()
                .unwrap()
                .data_generation,
            committed.data_generation
        );
        assert!(store.recorded_session_matches(&source).unwrap());

        let mut overlapping_checkpoint = checkpoint.clone();
        overlapping_checkpoint.committed_offset = 12;
        overlapping_checkpoint.cycle_seq = 2;
        let overlapping = SessionRange {
            start_offset: 5,
            end_offset: 12,
            cycle_seq: 2,
            record_sha256: "22".repeat(32),
            ..range
        };
        assert!(store
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 0x1234,
                cycle_seq: 2,
                samples: &[sample(1_800_000_060, reset_at, Some(49.0), 4.0)],
                checkpoints: &[overlapping_checkpoint],
                ranges: &[overlapping],
                model_totals: &[],
                recorded_sessions: &[],
            })
            .is_err());
        let state = store.load_session_collection_state().unwrap();
        assert_eq!(state.data_generation, 1);
        assert_eq!(state.collector_epoch, Some(0x1234));
        assert_eq!(state.cycle_seq, 1);
        assert_eq!(
            state.last_quota_observation,
            Some(SessionQuotaObservation {
                observed_at: committed_sample.timestamp,
                remaining_percent: 50.0,
            })
        );
        assert_eq!(state.checkpoints, vec![checkpoint]);
        assert_eq!(state.model_totals, expected_model_totals);
        assert_eq!(store.load_all().unwrap(), vec![committed_sample]);
        let range_count: i64 = store
            .connection
            .query_row("SELECT count(*) FROM session_ranges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(range_count, 1);
        assert!(store.recorded_session_matches(&source).unwrap());

        remove_database(&path);
    }

    #[test]
    fn session_checkpoint_commit_prunes_superseded_lineage() {
        let path = database_path("partition-checkpoint-head");
        let identity = partition_identity('d', 11);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let reset_at = 1_800_604_800;
        let source = recorded_source("2026/09/session.jsonl", 30);
        let first = checkpoint(&source, 10);
        store
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 0x1234,
                cycle_seq: 1,
                samples: &[],
                checkpoints: std::slice::from_ref(&first),
                ranges: &[],
                model_totals: &[],
                recorded_sessions: &[],
            })
            .unwrap();

        let mut replacement = first;
        replacement.cycle_seq = 2;
        replacement.prefix_generation = 0x9876;
        replacement.prefix_sha256 = "22".repeat(32);
        store
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 0x1234,
                cycle_seq: 2,
                samples: &[],
                checkpoints: std::slice::from_ref(&replacement),
                ranges: &[],
                model_totals: &[],
                recorded_sessions: &[],
            })
            .unwrap();

        let state = store.load_session_collection_state().unwrap();
        assert_eq!(state.checkpoints, [replacement]);
        remove_database(&path);
    }

    #[test]
    fn injected_checkpoint_write_failure_rolls_back_the_entire_collection_generation() {
        let path = database_path("partition-session-rollback");
        let identity = partition_identity('d', 4);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let reset_at = 1_800_604_800;
        let mut source = recorded_source("2026/09/rollback.jsonl", 40);
        source.file_bytes = 10;
        let checkpoint = checkpoint(&source, 10);
        let range = SessionRange {
            root_identity: source.root_identity.clone(),
            relative_path: source.relative_path.clone(),
            file_device: source.file_device,
            file_inode: source.file_inode,
            start_offset: 0,
            end_offset: 10,
            collector_epoch: 0x1234,
            cycle_seq: 1,
            prefix_generation: 0x5678,
            record_sha256: "11".repeat(32),
        };
        store
            .connection
            .execute_batch(
                "CREATE TEMP TRIGGER inject_checkpoint_failure
                 BEFORE INSERT ON session_checkpoints
                 BEGIN SELECT RAISE(ABORT, 'injected checkpoint failure'); END;",
            )
            .unwrap();

        let error = store
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 0x1234,
                cycle_seq: 1,
                samples: &[sample(1_800_000_000, reset_at, Some(50.0), 3.0)],
                checkpoints: &[checkpoint],
                ranges: &[range],
                model_totals: &[SessionModelTotal {
                    cache_write_input_tokens: None,
                    model: "SOL".into(),
                    total_tokens: 20,
                    input_tokens: 12,
                    cached_input_tokens: 2,
                    output_tokens: 8,
                }],
                recorded_sessions: &[source],
            })
            .unwrap_err();
        assert!(matches!(error, UsageStoreError::Sqlite(_)));

        let state = store.load_session_collection_state().unwrap();
        assert_eq!(state, SessionCollectionState::default());
        assert!(store.load_all().unwrap().is_empty());
        for table in [
            "session_ranges",
            "session_checkpoints",
            "session_model_totals",
            "recorded_sessions",
        ] {
            let count: i64 = store
                .connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "table {table}");
        }

        remove_database(&path);
    }

    #[test]
    fn replacement_prunes_old_checkpoints_and_keeps_exact_cleanup_markers() {
        let path = database_path("partition-session-replacement-retention");
        let identity = partition_identity('e', 5);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let reset_at = 1_800_604_800;
        let mut old_source = recorded_source("2026/09/replaced.jsonl", 50);
        old_source.file_bytes = 10;
        let old_checkpoint = checkpoint(&old_source, 10);
        store
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 0x1234,
                cycle_seq: 1,
                samples: &[],
                checkpoints: std::slice::from_ref(&old_checkpoint),
                ranges: &[],
                model_totals: &[],
                recorded_sessions: std::slice::from_ref(&old_source),
            })
            .unwrap();

        let mut replacement_checkpoint = old_checkpoint.clone();
        replacement_checkpoint.collector_epoch = 0x4321;
        replacement_checkpoint.cycle_seq = 2;
        replacement_checkpoint.prefix_generation = 0x9876;
        replacement_checkpoint.prefix_sha256 = "22".repeat(32);
        replacement_checkpoint.fully_attributed_from_zero = false;
        replacement_checkpoint.token_baseline_known = false;
        replacement_checkpoint.last_model = None;
        store
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 0x4321,
                cycle_seq: 2,
                samples: &[],
                checkpoints: std::slice::from_ref(&replacement_checkpoint),
                ranges: &[],
                model_totals: &[],
                recorded_sessions: &[],
            })
            .unwrap();

        let mut new_source = old_source.clone();
        new_source.modified_nanos += 1;
        let mut new_checkpoint = replacement_checkpoint.clone();
        new_checkpoint.cycle_seq = 3;
        new_checkpoint.prefix_generation = 0xabcd;
        new_checkpoint.prefix_sha256 = "33".repeat(32);
        new_checkpoint.fully_attributed_from_zero = true;
        new_checkpoint.token_baseline_known = true;
        store
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 0x4321,
                cycle_seq: 3,
                samples: &[],
                checkpoints: std::slice::from_ref(&new_checkpoint),
                ranges: &[],
                model_totals: &[],
                recorded_sessions: std::slice::from_ref(&new_source),
            })
            .unwrap();

        let state = store.load_session_collection_state().unwrap();
        assert_eq!(state.data_generation, 3);
        assert_eq!(state.checkpoints, [new_checkpoint]);
        assert!(store.recorded_session_matches(&old_source).unwrap());
        assert!(store.recorded_session_matches(&new_source).unwrap());
        assert_eq!(
            store
                .forget_recorded_sessions(std::slice::from_ref(&new_source))
                .unwrap(),
            1
        );
        assert!(store.recorded_session_matches(&old_source).unwrap());
        assert!(!store.recorded_session_matches(&new_source).unwrap());

        remove_database(&path);
    }

    #[test]
    fn partition_generations_and_storage_epoch_use_the_full_u64_domain() {
        let path = database_path("partition-u64-generation");
        let identity = partition_identity('f', u64::MAX);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        store
            .connection
            .execute(
                "UPDATE collection_generation SET data_generation = ?1 WHERE singleton = 1",
                [u64::MAX.saturating_sub(1).to_string()],
            )
            .unwrap();
        assert_eq!(
            store
                .commit_session_collection(SessionCollectionCommit {
                    reset_at: 1_800_604_800,
                    window_seconds: 604_800,
                    collector_epoch: 0xffff,
                    cycle_seq: u64::MAX,
                    samples: &[],
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &[],
                    recorded_sessions: &[],
                })
                .unwrap(),
            u64::MAX
        );
        let state = store.load_session_collection_state().unwrap();
        assert_eq!(state.data_generation, u64::MAX);
        assert_eq!(state.cycle_seq, u64::MAX);
        assert!(matches!(
            store.commit_session_collection(SessionCollectionCommit {
                reset_at: 1_800_604_800,
                window_seconds: 604_800,
                collector_epoch: 0xffff,
                cycle_seq: u64::MAX,
                samples: &[],
                checkpoints: &[],
                ranges: &[],
                model_totals: &[],
                recorded_sessions: &[],
            }),
            Err(UsageStoreError::GenerationOverflow)
        ));
        assert_eq!(
            store
                .load_session_collection_state()
                .unwrap()
                .data_generation,
            u64::MAX
        );
        drop(store);
        assert!(UsageStore::open_partitioned(&path, &identity).is_ok());

        remove_database(&path);
    }

    #[test]
    fn wrong_partition_schema_is_rejected_by_a_read_only_probe() {
        let path = database_path("partition-wrong-schema");
        let identity = partition_identity('9', 9);
        drop(UsageStore::create_partitioned(&path, &identity).unwrap());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "ALTER TABLE storage_partition ADD COLUMN unexpected TEXT",
                [],
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        let wal = path.with_file_name(format!(
            "{}-wal",
            path.file_name().unwrap().to_string_lossy()
        ));
        let shm = path.with_file_name(format!(
            "{}-shm",
            path.file_name().unwrap().to_string_lossy()
        ));
        assert!(!wal.exists());
        assert!(!shm.exists());

        assert!(UsageStore::open_partitioned(&path, &identity).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!wal.exists());
        assert!(!shm.exists());

        remove_database(&path);
    }

    #[test]
    #[ignore = "explicit host SQLite latency SLO gate"]
    fn recent_history_query_uses_the_one_month_index_and_meets_manual_latency_slo() {
        let path = database_path("history-slo");
        let now = Utc.timestamp_opt(1_800_000_000, 0).single().unwrap();
        let mut store = UsageStore::open(&path).unwrap();
        {
            let transaction = store.connection.transaction().unwrap();
            {
                let mut insert = transaction
                    .prepare(
                        "INSERT INTO usage_history (timestamp, reset_at, remaining_percent, \
                         sol_dollars, terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    )
                    .unwrap();
                for index in 0..MAX_RECENT_HISTORY_SAMPLES {
                    let timestamp =
                        now.timestamp() - (MAX_RECENT_HISTORY_SAMPLES - 1 - index) as i64 * 60;
                    insert
                        .execute(params![
                            timestamp,
                            now.timestamp() + 604_800,
                            48.0,
                            index as f64,
                            index as f64,
                            index as f64,
                            index as i64,
                            index as i64,
                            index as i64,
                        ])
                        .unwrap();
                }
                // A full additional month remains in the retained three-month
                // database but is outside the one-month acquisition window.
                // Its presence must not change query cardinality or force a
                // scan of the retained table.
                let cutoff = one_month_before(now).timestamp();
                for index in 0..MAX_RECENT_HISTORY_SAMPLES {
                    let timestamp = cutoff - 1 - index as i64 * 60;
                    insert
                        .execute(params![
                            timestamp,
                            now.timestamp() - 604_800,
                            48.0,
                            index as f64,
                            index as f64,
                            index as f64,
                            index as i64,
                            index as i64,
                            index as i64,
                        ])
                        .unwrap();
                }
            }
            transaction.commit().unwrap();
        }

        let cutoff = one_month_before(now).timestamp();
        let plan = store
            .connection
            .prepare(
                "EXPLAIN QUERY PLAN SELECT timestamp, reset_at, remaining_percent, sol_dollars, \
                 terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens \
                 FROM usage_history WHERE timestamp > ?1 AND timestamp <= ?2 \
                 ORDER BY timestamp DESC, reset_at DESC",
            )
            .unwrap()
            .query_map(params![cutoff, now.timestamp()], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" | ");
        assert!(
            plan.contains("usage_history_timestamp_reset_idx"),
            "unexpected query plan: {plan}"
        );
        assert!(!plan.contains("SCAN usage_history"), "full scan: {plan}");

        let mut elapsed = Vec::with_capacity(30);
        for _ in 0..30 {
            let started = Instant::now();
            let rows = store.load_recent_one_month(now).unwrap();
            elapsed.push(started.elapsed().as_secs_f64() * 1_000.0);
            assert_eq!(rows.len(), MAX_RECENT_HISTORY_SAMPLES);
            assert!(rows.windows(2).all(|pair| {
                (pair[0].reset_at, pair[0].timestamp) <= (pair[1].reset_at, pair[1].timestamp)
            }));
        }
        elapsed.sort_by(f64::total_cmp);
        let p90 = elapsed[26];
        let p95 = elapsed[28];
        let maximum = elapsed[29];
        eprintln!(
            "SLO db=recent_history rows={} n=30 p90={p90:.3}ms p95={p95:.3}ms max={maximum:.3}ms plan={plan}",
            MAX_RECENT_HISTORY_SAMPLES
        );
        assert!(p90 <= 100.0, "DB p90 {p90:.3}ms exceeds 100ms");
        assert!(p95 <= 150.0, "DB p95 {p95:.3}ms exceeds 150ms");

        // A second reset alias in one minute is legitimate raw evidence. It is
        // resolved by the public canonicalizer, not rejected by this reader.
        let before_count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM usage_history", [], |row| row.get(0))
            .unwrap();
        store
            .connection
            .execute(
                "INSERT INTO usage_history (timestamp, reset_at, remaining_percent, \
                 sol_dollars, terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    now.timestamp(),
                    now.timestamp() + 1_209_600,
                    48.0,
                    1.0,
                    1.0,
                    1.0,
                    1_i64,
                    1_i64,
                    1_i64,
                ],
            )
            .unwrap();
        assert_eq!(
            store.load_recent_one_month(now).unwrap().len(),
            MAX_RECENT_HISTORY_SAMPLES + 1
        );
        let after_count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM usage_history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(after_count, before_count + 1);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn reopening_a_legacy_history_index_rebuilds_only_the_covering_index() {
        let path = database_path("legacy-history-index");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE usage_history (
                    timestamp INTEGER NOT NULL CHECK (timestamp > 0),
                    reset_at INTEGER NOT NULL CHECK (reset_at > 0),
                    remaining_percent REAL,
                    sol_dollars REAL NOT NULL,
                    terra_dollars REAL NOT NULL,
                    luna_dollars REAL NOT NULL,
                    sol_tokens INTEGER NOT NULL DEFAULT 0,
                    terra_tokens INTEGER NOT NULL DEFAULT 0,
                    luna_tokens INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (reset_at, timestamp)
                );
                CREATE INDEX usage_history_timestamp_idx
                    ON usage_history (timestamp);
                CREATE INDEX usage_history_timestamp_reset_idx
                    ON usage_history (timestamp, reset_at) WHERE timestamp > 0;",
            )
            .unwrap();
        drop(connection);

        let store = UsageStore::open(&path).unwrap();
        let plan = store
            .connection
            .prepare(
                "EXPLAIN QUERY PLAN SELECT timestamp, reset_at, remaining_percent, \
                 sol_dollars, terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens \
                 FROM usage_history WHERE timestamp > ?1 AND timestamp <= ?2 \
                 ORDER BY timestamp DESC, reset_at DESC",
            )
            .unwrap()
            .query_map(params![1_i64, 2_i64], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" | ");
        assert!(plan.contains("USING COVERING INDEX usage_history_timestamp_reset_idx"));

        let columns = store
            .connection
            .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno ASC")
            .unwrap()
            .query_map([HISTORY_TIMESTAMP_RESET_INDEX], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            columns,
            HISTORY_TIMESTAMP_RESET_INDEX_COLUMNS
                .iter()
                .map(|column| (*column).to_owned())
                .collect::<Vec<_>>()
        );
        let index_shape = store
            .connection
            .query_row(
                "SELECT \"unique\", origin, partial FROM pragma_index_list('usage_history') \
                 WHERE name = ?1",
                [HISTORY_TIMESTAMP_RESET_INDEX],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(index_shape, (0, "c".to_owned(), 0));

        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_reopen_persists_samples() {
        let path = database_path("reopen");
        let expected = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);

        {
            let store = UsageStore::open(&path).unwrap();
            store.upsert_sample(&expected).unwrap();
        }

        let actual = UsageStore::open(&path).unwrap().load_all().unwrap();
        assert_eq!(actual, vec![expected]);
        assert_eq!(actual[0].sol_tokens, 11);
        assert_eq!(actual[0].terra_tokens, 22);
        assert_eq!(actual[0].luna_tokens, 33);
        remove_database(&path);
    }

    #[test]
    fn recorded_session_marker_commits_atomically_with_new_usage_and_preserves_existing_state() {
        let path = database_path("recorded-session-atomic");
        let existing = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let added = sample(1_700_000_120, 1_700_604_800, Some(74.0), 2.5);
        let marker = recorded_source("2026/09/session.jsonl", 30);
        let durable_hash = "0".repeat(64);

        let mut store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&existing).unwrap();
        let durable = store
            .commit_durable_state(&[], &durable_hash, "{}")
            .unwrap();
        // Simulate the current additive-upgrade source: both existing tables
        // and rows are valid, while the new marker table is absent.
        store
            .connection
            .execute("DROP TABLE recorded_sessions", [])
            .unwrap();
        drop(store);

        let mut upgraded = UsageStore::open(&path).unwrap();
        assert_eq!(upgraded.load_all().unwrap(), vec![existing.clone()]);
        assert_eq!(
            upgraded.load_durable_record().unwrap(),
            Some(durable.clone())
        );
        upgraded
            .upsert_samples_and_recorded_sessions(
                std::slice::from_ref(&added),
                std::slice::from_ref(&marker),
            )
            .unwrap();
        drop(upgraded);

        let reopened = UsageStore::open_read_only(&path).unwrap();
        assert_eq!(
            reopened.load_all().unwrap(),
            vec![existing.clone(), added.clone()]
        );
        assert_eq!(
            reopened.load_durable_record().unwrap(),
            Some(durable.clone())
        );
        assert!(reopened.recorded_session_matches(&marker).unwrap());
        drop(reopened);

        let mut writer = UsageStore::open(&path).unwrap();
        writer
            .connection
            .execute_batch(
                "CREATE TRIGGER reject_recorded_session_insert
                 BEFORE INSERT ON recorded_sessions
                 BEGIN
                    SELECT RAISE(ABORT, 'marker rejected');
                 END;",
            )
            .unwrap();
        let rejected_sample = sample(1_700_000_180, 1_700_604_800, Some(73.0), 3.5);
        let rejected_marker = recorded_source("2026/09/rejected.jsonl", 31);
        assert!(writer
            .upsert_samples_and_recorded_sessions(
                std::slice::from_ref(&rejected_sample),
                std::slice::from_ref(&rejected_marker),
            )
            .is_err());
        assert_eq!(writer.load_all().unwrap(), vec![existing, added]);
        assert_eq!(writer.load_durable_record().unwrap(), Some(durable));
        assert!(!writer.recorded_session_matches(&rejected_marker).unwrap());
        drop(writer);
        remove_database(&path);
    }

    #[test]
    fn recorded_session_marker_delete_failure_keeps_marker_and_protected_rows() {
        let path = database_path("recorded-session-delete-failure");
        let existing = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let marker = recorded_source("2026/09/session.jsonl", 30);
        let durable_hash = "1".repeat(64);
        let mut store = UsageStore::open(&path).unwrap();
        store
            .upsert_samples_and_recorded_sessions(
                std::slice::from_ref(&existing),
                std::slice::from_ref(&marker),
            )
            .unwrap();
        let durable = store
            .commit_durable_state(&[], &durable_hash, "{}")
            .unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER reject_recorded_session_delete
                 BEFORE DELETE ON recorded_sessions
                 BEGIN
                    SELECT RAISE(ABORT, 'marker delete rejected');
                 END;",
            )
            .unwrap();

        assert!(store
            .forget_recorded_sessions(std::slice::from_ref(&marker))
            .is_err());
        assert!(store.recorded_session_matches(&marker).unwrap());
        assert_eq!(store.load_all().unwrap(), vec![existing]);
        assert_eq!(store.load_durable_record().unwrap(), Some(durable));
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn opening_an_old_schema_is_rejected_without_migration() {
        let path = database_path("old-schema");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE usage_history (
                    timestamp INTEGER NOT NULL,
                    reset_at INTEGER NOT NULL,
                    remaining_percent REAL,
                    sol_dollars REAL NOT NULL,
                    terra_dollars REAL NOT NULL,
                    luna_dollars REAL NOT NULL,
                    PRIMARY KEY (reset_at, timestamp)
                );
                INSERT INTO usage_history
                    (timestamp, reset_at, remaining_percent,
                     sol_dollars, terra_dollars, luna_dollars)
                VALUES (1700000060, 1700000000, 75.0, 1.25, 2.0, 3.0);",
            )
            .unwrap();
        drop(connection);

        assert!(UsageStore::open(&path).is_err());
        let connection = Connection::open(&path).unwrap();
        let token_columns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('usage_history')
                 WHERE name IN ('sol_tokens', 'terra_tokens', 'luna_tokens')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(token_columns, 0);
        let durable_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'durable_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(durable_tables, 0);
        drop(connection);
        remove_database(&path);
    }

    #[test]
    fn usage_store_same_key_dominant_replaces_whole_value() {
        let path = database_path("replacement");
        let first = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let replacement = sample(1_700_000_060, 1_700_604_800, Some(75.0), 9.5);

        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&first).unwrap();
        store.upsert_sample(&replacement).unwrap();

        assert_eq!(store.load_all().unwrap(), vec![replacement]);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_rejects_same_batch_key_quota_conflict_atomically() {
        let path = database_path("quota-conflict");
        let existing = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.0);
        let conflicting = sample(1_700_000_060, 1_700_604_800, Some(60.0), 2.0);

        let mut store = UsageStore::open(&path).unwrap();
        assert!(store.upsert_samples(&[existing, conflicting]).is_err());
        assert!(store.load_all().unwrap().is_empty());
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_commit_accepts_new_quota_for_existing_key_and_advances_generation() {
        let path = database_path("commit-quota-update");
        let first = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.0);
        let second = sample(1_700_000_060, 1_700_604_800, Some(70.0), 2.0);
        let mut store = UsageStore::open(&path).unwrap();

        let first_record = store
            .commit_durable_state(&[first], "a".repeat(64), r#"{"generation":1}"#)
            .unwrap();
        assert_eq!(first_record.data_generation, 1);

        let second_record = store
            .commit_durable_state_if_generation(
                first_record.data_generation,
                std::slice::from_ref(&second),
                "b".repeat(64),
                r#"{"generation":2}"#,
            )
            .unwrap();
        assert_eq!(second_record.data_generation, 2);
        assert_eq!(store.load_all().unwrap(), vec![second]);
        assert_eq!(store.load_durable_record().unwrap(), Some(second_record));

        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_batch_rejects_noncomparable_existing_and_rolls_back_new_rows() {
        let path = database_path("batch-rollback");
        let mut existing = sample(1_700_000_060, 1_700_604_800, Some(75.0), 5.0);
        existing.terra_dollars = 1.0;
        let mut conflicting = existing.clone();
        conflicting.sol_dollars = 1.0;
        conflicting.terra_dollars = 5.0;
        let new_row = sample(1_700_000_120, 1_700_604_800, Some(75.0), 9.0);

        let mut store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&existing).unwrap();
        assert!(store.upsert_samples(&[new_row, conflicting]).is_err());
        assert_eq!(store.load_all().unwrap(), vec![existing]);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_batch_keeps_observed_dominant_vector_and_unique_quota() {
        let path = database_path("batch-dominant");
        let existing = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.0);
        let mut lower_observation = existing.clone();
        lower_observation.remaining_percent = None;
        let mut dominant = existing.clone();
        dominant.remaining_percent = None;
        dominant.sol_dollars = 2.0;
        dominant.terra_dollars = 4.0;
        dominant.sol_tokens = 44;

        let mut store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&existing).unwrap();
        store
            .upsert_samples(&[lower_observation, dominant.clone()])
            .unwrap();

        dominant.remaining_percent = Some(75.0);
        assert_eq!(store.load_all().unwrap(), vec![dominant]);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_upsert_keeps_existing_rows() {
        let path = database_path("append-only");
        let first_period = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let second_period = sample(1_700_000_060, 1_701_209_600, Some(95.0), 8.0);

        let mut store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&first_period).unwrap();
        store
            .upsert_samples(std::slice::from_ref(&second_period))
            .unwrap();

        assert_eq!(store.load_all().unwrap(), vec![first_period, second_period]);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_import_is_additive_and_idempotent() {
        let path = database_path("import-idempotent");
        let first = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let second = sample(1_700_000_120, 1_700_604_800, Some(70.0), 2.5);

        let mut store = UsageStore::open(&path).unwrap();
        assert_eq!(
            store
                .import_samples(&[first.clone(), second.clone()])
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .import_samples(&[first.clone(), second.clone()])
                .unwrap(),
            2
        );
        assert_eq!(
            store.load_all().unwrap(),
            vec![first.clone(), second.clone()]
        );
        drop(store);

        assert_eq!(
            UsageStore::open(&path).unwrap().load_all().unwrap(),
            vec![first, second]
        );
        remove_database(&path);
    }

    #[test]
    fn concurrent_collectors_merge_one_minute_without_duplicate_rows() {
        let path = database_path("concurrent-merge");
        drop(UsageStore::open(&path).unwrap());
        let barrier = Arc::new(Barrier::new(2));
        let first = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let mut second = first.clone();
        second.sol_dollars = 4.5;
        second.sol_tokens = 900;
        let expected_second = second.clone();
        let left_path = path.clone();
        let left_barrier = Arc::clone(&barrier);
        let left = std::thread::spawn(move || {
            let store = UsageStore::open(left_path).unwrap();
            left_barrier.wait();
            store.upsert_sample(&first).unwrap();
        });
        let right_path = path.clone();
        let right_barrier = Arc::clone(&barrier);
        let right = std::thread::spawn(move || {
            let store = UsageStore::open(right_path).unwrap();
            right_barrier.wait();
            store.upsert_sample(&second).unwrap();
        });
        left.join().unwrap();
        right.join().unwrap();
        let rows = UsageStore::open(&path).unwrap().load_all().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], expected_second);
        remove_database(&path);
    }

    #[test]
    fn backup_generations_are_sqlite_consistent_and_bounded() {
        let path = database_path("backup-generations");
        let first = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&first).unwrap();
        drop(store);
        for _ in 0..4 {
            UsageStore::backup_generations(&path, 3).unwrap();
        }
        for generation in 1..=3 {
            let backup = path.with_extension(format!("sqlite3.bak.{generation}"));
            assert!(backup.is_file(), "missing backup generation {generation}");
            let connection = Connection::open(&backup).unwrap();
            let quick_check: String = connection
                .query_row("PRAGMA quick_check", [], |row| row.get(0))
                .unwrap();
            assert_eq!(quick_check, "ok");
            assert_eq!(
                UsageStore::open(&backup).unwrap().load_all().unwrap(),
                vec![first.clone()]
            );
        }
        assert!(!path.with_extension("sqlite3.bak.4").exists());
        remove_database(&path);
    }

    #[test]
    fn failed_backup_rotation_keeps_existing_generation_untouched() {
        let path = database_path("backup-rotation-failure");
        let original = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&original).unwrap();
        drop(store);
        UsageStore::backup_generations(&path, 3).unwrap();
        let first = path.with_extension("sqlite3.bak.1");
        let before = fs::read(&first).unwrap();

        // A non-regular generation is rejected before rotation starts. The
        // current DB and the already usable generation must remain intact.
        let blocked = path.with_extension("sqlite3.bak.2");
        fs::create_dir(&blocked).unwrap();
        assert!(UsageStore::backup_generations(&path, 3).is_err());
        assert_eq!(fs::read(&first).unwrap(), before);
        assert_eq!(
            UsageStore::open(&path).unwrap().load_all().unwrap(),
            vec![original]
        );
        fs::remove_dir(&blocked).unwrap();
        remove_database(&path);
    }

    #[test]
    fn verified_migration_switches_only_after_candidate_validation() {
        let path = database_path("verified-migration");
        let original = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&original).unwrap();
        drop(store);

        let report = UsageStore::migrate_verified(&path, |samples| {
            let mut migrated = samples.to_vec();
            migrated[0].sol_dollars = 2.5;
            Ok(migrated)
        })
        .unwrap();

        assert_eq!(report.source_rows, 1);
        assert_eq!(report.candidate_rows, 1);
        assert!(report.preserved_backup.is_file());
        let migrated = UsageStore::open(&path).unwrap().load_all().unwrap();
        assert_eq!(migrated[0].sol_dollars, 2.5);
        let preserved = UsageStore::open(&report.preserved_backup)
            .unwrap()
            .load_all()
            .unwrap();
        assert_eq!(preserved, vec![original]);
        remove_database(&path);
        let _ = fs::remove_file(report.preserved_backup);
    }

    #[test]
    fn invalid_migration_candidate_leaves_source_untouched() {
        let path = database_path("invalid-migration");
        let original = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&original).unwrap();
        drop(store);

        let result = UsageStore::migrate_verified(&path, |samples| {
            let mut migrated = samples.to_vec();
            migrated[0].remaining_percent = Some(101.0);
            Ok(migrated)
        });
        assert!(result.is_err());
        assert_eq!(
            UsageStore::open(&path).unwrap().load_all().unwrap(),
            vec![original]
        );
        assert!(!path
            .parent()
            .unwrap()
            .join(format!(
                ".{}.migration.lock",
                path.file_name().unwrap().to_string_lossy()
            ))
            .exists());
        remove_database(&path);
    }

    #[test]
    fn migration_that_drops_a_valid_row_is_rejected_before_switch() {
        let path = database_path("migration-row-drop");
        let first = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let second = sample(1_700_000_120, 1_700_604_800, Some(70.0), 2.5);
        let mut store = UsageStore::open(&path).unwrap();
        store
            .upsert_samples(&[first.clone(), second.clone()])
            .unwrap();
        drop(store);

        let result = UsageStore::migrate_verified(&path, |samples| Ok(vec![samples[0].clone()]));
        assert!(result.is_err());
        assert_eq!(
            UsageStore::open(&path).unwrap().load_all().unwrap(),
            vec![first, second]
        );
        remove_database(&path);
    }

    #[test]
    fn usage_store_missing_remaining_keeps_existing_quota_for_dominant_update() {
        let path = database_path("nullable-update");
        let observed = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let missing = sample(1_700_000_060, 1_700_604_800, None, 9.5);

        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&observed).unwrap();
        store.upsert_sample(&missing).unwrap();

        let actual = store.load_all().unwrap();
        assert_eq!(actual.len(), 1);
        let expected = UsageHistorySample {
            remaining_percent: Some(75.0),
            ..missing
        };
        assert_eq!(actual, vec![expected]);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_smaller_whole_vector_does_not_replace_existing_value() {
        let path = database_path("cumulative-cost");
        let larger = sample(1_700_000_060, 1_700_604_800, Some(75.0), 9.5);
        let smaller = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);

        let mut store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&larger).unwrap();
        store.upsert_sample(&smaller).unwrap();

        let actual = store.load_all().unwrap();
        assert_eq!(actual.len(), 1);
        assert_eq!(actual, vec![larger]);

        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let canonical_samples =
            canonicalize_samples(&transaction, std::slice::from_ref(&smaller)).unwrap();
        let canonical_observations = canonicalize_observations(
            &transaction,
            std::slice::from_ref(&UsageHistoryObservation::confirmed(&smaller)),
            &canonical_samples,
        )
        .unwrap();
        assert_eq!(canonical_observations.len(), 1);
        assert_eq!(
            canonical_observations[0].model_source,
            ModelSource::LegacyUnknown
        );
        assert_eq!(canonical_observations[0].sol_dollars, Some(9.5));
        drop(transaction);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_distinct_reset_periods_keep_same_timestamp() {
        let path = database_path("reset-periods");
        let first_period = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let second_period = sample(1_700_000_060, 1_701_209_600, Some(95.0), 8.0);

        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&first_period).unwrap();
        store.upsert_sample(&second_period).unwrap();

        assert_eq!(store.load_all().unwrap(), vec![first_period, second_period]);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn usage_store_nullable_remaining_quota_round_trips_as_sql_null() {
        let path = database_path("nullable");
        let expected = sample(1_700_000_060, 1_700_604_800, None, 1.25);

        {
            let store = UsageStore::open(&path).unwrap();
            store.upsert_sample(&expected).unwrap();
            let stored: Option<f64> = store
                .connection
                .query_row(
                    "SELECT remaining_percent FROM usage_history \
                     WHERE reset_at = ?1 AND timestamp = ?2",
                    params![expected.reset_at, expected.timestamp],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(stored, None);
        }

        assert_eq!(
            UsageStore::open(&path).unwrap().load_all().unwrap(),
            vec![expected]
        );
        remove_database(&path);
    }

    #[test]
    fn unavailable_observation_rejects_partial_model_vector() {
        let mut observation =
            UsageHistoryObservation::unavailable(1_700_000_040, 1_700_604_800, Some(75.0));
        observation.sol_dollars = Some(1.0);
        assert!(matches!(
            observation.validate(),
            Err(UsageStoreError::InvalidImport(_))
        ));
    }

    #[test]
    fn observation_source_transitions_are_monotonic_and_same_minute_updates_commit() {
        let path = database_path("observation-source-transitions");
        let timestamp = 1_700_000_040;
        let reset_at = 1_700_604_800;
        let mut store = UsageStore::open(&path).unwrap();

        let unavailable = UsageHistoryObservation::unavailable(timestamp, reset_at, Some(90.0));
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            upsert_observations(&transaction, std::slice::from_ref(&unavailable)).unwrap(),
            vec![unavailable.clone()]
        );
        transaction.commit().unwrap();

        // A later quota-only reading for the same minute must update the
        // unavailable sidecar instead of producing a permanent batch conflict.
        let later_unavailable =
            UsageHistoryObservation::unavailable(timestamp, reset_at, Some(80.0));
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            upsert_observations(&transaction, std::slice::from_ref(&later_unavailable),).unwrap(),
            vec![later_unavailable.clone()]
        );
        transaction.commit().unwrap();

        let confirmed_sample = sample(timestamp, reset_at, Some(82.0), 4.0);
        let confirmed = UsageHistoryObservation::confirmed(&confirmed_sample);
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            upsert_observations(&transaction, std::slice::from_ref(&confirmed)).unwrap(),
            vec![confirmed.clone()]
        );
        transaction.commit().unwrap();

        // An unavailable retry cannot downgrade an already confirmed vector.
        let attempted_downgrade =
            UsageHistoryObservation::unavailable(timestamp, reset_at, Some(70.0));
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            upsert_observations(&transaction, std::slice::from_ref(&attempted_downgrade),).unwrap(),
            vec![confirmed.clone()]
        );
        transaction.commit().unwrap();

        let observations = store
            .load_recent_observations(Utc.timestamp_opt(timestamp + 60, 0).unwrap())
            .unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0], confirmed);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn old_durable_state_check_migrates_before_validation_and_preserves_row_one() {
        let path = database_path("durable-state-check-migration");
        let expected = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let durable = {
            let mut store = UsageStore::open(&path).unwrap();
            store.upsert_sample(&expected).unwrap();
            store
                .commit_durable_state(&[], "0".repeat(64), r#"{"kind":"legacy-row-one"}"#)
                .unwrap()
        };

        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE durable_state RENAME TO durable_state_legacy;
                 CREATE TABLE durable_state (
                     singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                     data_generation INTEGER NOT NULL CHECK (data_generation >= 0),
                     data_hash TEXT NOT NULL,
                     snapshot_json TEXT NOT NULL
                 );
                 INSERT INTO durable_state
                     (singleton, data_generation, data_hash, snapshot_json)
                 SELECT singleton, data_generation, data_hash, snapshot_json
                 FROM durable_state_legacy;
                 DROP TABLE durable_state_legacy;",
            )
            .unwrap();
        drop(connection);

        let reopened = UsageStore::open(&path).unwrap();
        assert_eq!(reopened.load_all().unwrap(), vec![expected]);
        assert_eq!(reopened.load_durable_record().unwrap(), Some(durable));
        drop(reopened);

        let connection = Connection::open(&path).unwrap();
        let durable_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'table' AND name = 'durable_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let normalized = durable_sql
            .to_ascii_lowercase()
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        assert!(normalized.contains("singleton>=1"));
        let singleton_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM durable_state", [], |row| row.get(0))
            .unwrap();
        assert_eq!(singleton_count, 1);
        drop(connection);
        remove_database(&path);
    }

    #[test]
    fn session_sample_and_observation_commit_or_rollback_as_one_transaction() {
        let path = database_path("session-observation-atomic");
        let committed = sample(1_700_000_040, 1_700_604_800, Some(64.0), 1.5);
        let observation = UsageHistoryObservation::confirmed(&committed);
        let commit = || SessionCollectionCommit {
            reset_at: committed.reset_at,
            window_seconds: 604_800,
            collector_epoch: 1,
            cycle_seq: 1,
            samples: std::slice::from_ref(&committed),
            checkpoints: &[],
            ranges: &[],
            model_totals: &[],
            recorded_sessions: &[],
        };
        let identity = partition_identity('a', 1);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER reject_collection_generation_update
                 BEFORE UPDATE ON collection_generation
                 BEGIN SELECT RAISE(ABORT, 'collection generation rejected'); END;",
            )
            .unwrap();
        assert!(store
            .commit_session_collection_with_observations(
                commit(),
                std::slice::from_ref(&observation),
            )
            .is_err());
        assert!(store.load_all().unwrap().is_empty());
        let sidecar_count: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM durable_state WHERE singleton >= 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sidecar_count, 0);

        store
            .connection
            .execute_batch("DROP TRIGGER reject_collection_generation_update")
            .unwrap();
        let result = store
            .commit_session_collection_with_observations(
                commit(),
                std::slice::from_ref(&observation),
            )
            .unwrap();
        assert_eq!(result.canonical_observations, vec![observation.clone()]);
        assert_eq!(store.load_all().unwrap(), vec![committed]);
        assert_eq!(
            store
                .load_recent_observations(Utc.timestamp_opt(1_700_000_100, 0).unwrap())
                .unwrap(),
            vec![observation]
        );
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn three_month_prune_removes_old_sidecars_but_keeps_new_rows_and_row_one() {
        let path = database_path("prune-observation-sidecars");
        let now = Utc.with_ymd_and_hms(2024, 5, 31, 12, 34, 56).unwrap();
        let cutoff = three_months_before(now).timestamp();
        let old = sample(cutoff - 60, 1_700_604_800, Some(10.0), 1.0);
        let retained = sample(cutoff, 1_700_604_800, Some(20.0), 2.0);
        let old_timestamp = cutoff.div_euclid(60) * 60 - 60;
        let new_timestamp = cutoff.div_euclid(60) * 60 + 60;
        let old_observation =
            UsageHistoryObservation::unavailable(old_timestamp, 1_700_604_800, Some(90.0));
        let new_observation =
            UsageHistoryObservation::unavailable(new_timestamp, 1_700_604_800, Some(80.0));
        let mut store = UsageStore::open(&path).unwrap();
        store.upsert_samples(&[old, retained.clone()]).unwrap();
        let durable = store
            .commit_durable_state(&[], "0".repeat(64), r#"{"kind":"row-one"}"#)
            .unwrap();
        for (singleton, observation) in [(2_i64, &old_observation), (3_i64, &new_observation)] {
            let snapshot_json = observation_json(observation).unwrap();
            let data_hash = observation_data_hash(observation.reset_at, observation.timestamp);
            store
                .connection
                .execute(
                    "INSERT INTO durable_state
                        (singleton, data_generation, data_hash, snapshot_json)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![singleton, observation.timestamp, data_hash, snapshot_json],
                )
                .unwrap();
        }

        assert_eq!(store.prune_older_than_three_months(now).unwrap(), 1);
        assert_eq!(store.load_all().unwrap(), vec![retained]);
        assert_eq!(store.load_durable_record().unwrap(), Some(durable));
        let singletons = store
            .connection
            .prepare("SELECT singleton FROM durable_state ORDER BY singleton")
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(singletons, vec![1, 3]);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn three_month_cutoff_clamps_end_of_month_by_calendar_rule() {
        let now = Utc.with_ymd_and_hms(2024, 5, 31, 12, 34, 56).unwrap();
        let expected = Utc.with_ymd_and_hms(2024, 2, 29, 12, 34, 56).unwrap();

        assert_eq!(three_months_before(now), expected);
    }

    #[test]
    fn pruning_removes_only_old_rows_and_preserves_boundary_across_reset_periods() {
        let path = database_path("prune");
        let now = Utc.with_ymd_and_hms(2024, 5, 31, 12, 34, 56).unwrap();
        let cutoff = 1_709_210_096_i64;
        let old = sample(cutoff - 1, 1_700_604_800, Some(10.0), 1.0);
        let old_other_period = sample(cutoff - 1, 1_701_209_600, Some(11.0), 1.1);
        let boundary = sample(cutoff, 1_700_604_800, Some(20.0), 2.0);
        let boundary_other_period = sample(cutoff, 1_701_209_600, Some(21.0), 2.1);
        let newer = sample(cutoff + 1, 1_701_814_400, Some(30.0), 3.0);
        let future = sample(now.timestamp() + 1, 1_701_814_400, Some(40.0), 4.0);

        let mut store = UsageStore::open(&path).unwrap();
        store
            .upsert_samples(&[
                old,
                old_other_period,
                boundary.clone(),
                boundary_other_period.clone(),
                newer.clone(),
                future.clone(),
            ])
            .unwrap();
        assert_eq!(store.prune_older_than_three_months(now).unwrap(), 2);
        assert_eq!(
            store.load_all().unwrap(),
            vec![
                boundary.clone(),
                boundary_other_period.clone(),
                newer.clone(),
                future.clone()
            ]
        );

        // Reopening must not perform another implicit destructive operation.
        drop(store);
        let mut reopened = UsageStore::open(&path).unwrap();
        assert_eq!(
            reopened.load_all().unwrap(),
            vec![boundary, boundary_other_period, newer, future]
        );
        assert_eq!(reopened.prune_older_than_three_months(now).unwrap(), 0);
        drop(reopened);
        remove_database(&path);
    }

    #[cfg(unix)]
    #[test]
    fn storage_directory_and_database_modes_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let path = database_path("private-modes");
        let store = UsageStore::open(&path).unwrap();
        drop(store);
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        remove_database(&path);
    }

    #[cfg(unix)]
    #[test]
    fn database_symlink_relative_path_and_token_overflow_are_rejected() {
        use std::fs::File;
        use std::os::unix::fs::symlink;

        assert!(UsageStore::open(Path::new("relative.sqlite3")).is_err());
        let path = database_path("unsafe-paths");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let target = path.with_file_name("target.sqlite3");
        File::create(&target).unwrap();
        symlink(&target, &path).unwrap();
        assert!(UsageStore::open(&path).is_err());
        fs::remove_file(&path).unwrap();

        let store = UsageStore::open(&path).unwrap();
        let mut oversized = sample(100, 200, Some(50.0), 1.0);
        oversized.sol_tokens = i64::MAX as u64 + 1;
        assert!(store.upsert_sample(&oversized).is_err());
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn read_only_open_never_creates_or_repairs_the_store() {
        let path = database_path("read-only-open");
        assert!(UsageStore::open_read_only(&path).is_err());
        assert!(!path.exists());

        let row = sample(1_700_000_000, 1_700_604_800, Some(25.0), 4.0);
        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&row).unwrap();
        drop(store);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute("DROP INDEX usage_history_timestamp_reset_idx", [])
            .unwrap();
        drop(connection);

        let reader = UsageStore::open_read_only(&path).unwrap();
        assert_eq!(reader.load_all().unwrap(), vec![row]);
        drop(reader);
        let connection = Connection::open(&path).unwrap();
        let index_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_index_list('usage_history') WHERE name = ?1)",
                [HISTORY_TIMESTAMP_RESET_INDEX],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!index_exists);
        drop(connection);
        remove_database(&path);
    }
}
#[cfg(test)]
mod wave_b_correction_tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use rusqlite::{params, Connection, OptionalExtension};
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    const VALID_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn database_path(label: &str) -> PathBuf {
        let serial = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "codex-info-wave-b-{label}-{}-{serial}",
            std::process::id()
        ));
        assert!(!directory.exists(), "fixture directory unexpectedly exists");
        fs::create_dir(&directory).expect("create private fixture directory");
        #[cfg(unix)]
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("make fixture directory private");
        directory.join("usage.sqlite")
    }

    fn cleanup(path: &Path) {
        if path.exists() {
            fs::remove_file(path).expect("remove fixture database");
        }
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{}", path.display(), suffix));
            if sidecar.exists() {
                fs::remove_file(&sidecar).expect("remove fixture database sidecar");
            }
        }
        if let Some(parent) = path.parent() {
            fs::remove_dir(parent).expect("remove private fixture directory");
        }
    }

    fn sample(
        timestamp: i64,
        reset_at: i64,
        remaining_percent: Option<f64>,
        sol_dollars: f64,
    ) -> UsageHistorySample {
        UsageHistorySample {
            timestamp,
            reset_at,
            remaining_percent,
            sol_dollars,
            terra_dollars: sol_dollars + 1.0,
            luna_dollars: sol_dollars + 2.0,
            sol_tokens: 1,
            terra_tokens: 1,
            luna_tokens: 1,
        }
    }

    fn overflowing_token_sample() -> UsageHistorySample {
        UsageHistorySample {
            timestamp: 1_700_000_123,
            reset_at: 1_700_000_000,
            remaining_percent: Some(50.0),
            sol_dollars: 1.0,
            terra_dollars: 2.0,
            luna_dollars: 3.0,
            sol_tokens: u64::MAX,
            terra_tokens: u64::MAX,
            luna_tokens: u64::MAX,
        }
    }

    fn history_rows(path: &Path) -> Vec<(i64, i64, Option<f64>, f64, f64, f64)> {
        let connection = Connection::open(path).expect("history inspection connection");
        let mut statement = connection
            .prepare(
                "SELECT timestamp, reset_at, remaining_percent, sol_dollars, \
                        terra_dollars, luna_dollars \
                 FROM usage_history ORDER BY reset_at ASC, timestamp ASC",
            )
            .expect("history inspection query");
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .expect("history inspection rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("history inspection values")
    }

    fn durable_row(path: &Path) -> Option<(i64, String, String)> {
        let connection = Connection::open(path).expect("durable inspection connection");
        connection
            .query_row(
                "SELECT data_generation, data_hash, snapshot_json \
                 FROM durable_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .expect("durable inspection query")
    }

    fn singleton_count(path: &Path) -> i64 {
        let connection = Connection::open(path).expect("singleton inspection connection");
        connection
            .query_row("SELECT COUNT(*) FROM durable_state", [], |row| row.get(0))
            .expect("singleton inspection count")
    }

    fn reset_period_values(periods: &[ResetPeriod]) -> Vec<(i64, i64, i64)> {
        periods
            .iter()
            .map(|period| {
                (
                    period.canonical_id,
                    period.start_timestamp,
                    period.end_timestamp,
                )
            })
            .collect()
    }

    #[test]
    fn recent_read_uses_one_month_half_open_interval_at_month_ends() {
        let cases = [
            // 2024-05-31T12:00:00Z -> 2024-04-30T12:00:00Z.
            (1_717_156_800_i64, 1_714_478_400_i64),
            // 2023-05-31T12:00:00Z -> 2023-04-30T12:00:00Z.
            (1_685_534_400_i64, 1_682_856_000_i64),
        ];
        for (case_number, (now_epoch, cutoff_epoch)) in cases.into_iter().enumerate() {
            let path = database_path(&format!("recent-{case_number}"));
            let now = Utc.timestamp_opt(now_epoch, 0).single().unwrap();
            let reset_at = 1_700_000_000 + case_number as i64;
            let mut store = UsageStore::open(&path).unwrap();
            store
                .upsert_samples(&[
                    sample(cutoff_epoch - 1, reset_at, Some(10.0), 1.0),
                    sample(cutoff_epoch, reset_at, Some(20.0), 2.0),
                    sample(cutoff_epoch + 1, reset_at, Some(30.0), 3.0),
                    sample(now_epoch - 1, reset_at, Some(40.0), 4.0),
                    sample(now_epoch, reset_at, Some(50.0), 5.0),
                    sample(now_epoch + 1, reset_at, Some(60.0), 6.0),
                ])
                .unwrap();
            let timestamps = store
                .load_recent_one_month(now)
                .unwrap()
                .into_iter()
                .map(|row| row.timestamp)
                .collect::<Vec<_>>();
            assert_eq!(timestamps, vec![cutoff_epoch + 1, now_epoch - 1, now_epoch]);
            assert_eq!(history_rows(&path).len(), 6);
            drop(store);
            cleanup(&path);
        }
    }

    #[test]
    fn recent_read_filters_invalid_values_without_deleting_rows() {
        let path = database_path("recent-invalid");
        let now_epoch = 1_717_156_800_i64;
        let cutoff_epoch = 1_714_478_400_i64;
        let now = Utc.timestamp_opt(now_epoch, 0).single().unwrap();
        let store = UsageStore::open(&path).unwrap();
        store
            .upsert_sample(&sample(cutoff_epoch, 1_700_000_000, Some(50.0), 1.0))
            .unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO usage_history
                    (timestamp, reset_at, remaining_percent, sol_dollars, terra_dollars, luna_dollars)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![cutoff_epoch + 10, 1_700_000_010_i64, -1.0, 1.0, 2.0, 3.0],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO usage_history
                    (timestamp, reset_at, remaining_percent, sol_dollars, terra_dollars, luna_dollars)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![cutoff_epoch + 11, 1_700_000_011_i64, 101.0, 1.0, 2.0, 3.0],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO usage_history
                    (timestamp, reset_at, remaining_percent, sol_dollars, terra_dollars, luna_dollars)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![cutoff_epoch + 12, 1_700_000_012_i64, 50.0, -1.0, 2.0, 3.0],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO usage_history
                    (timestamp, reset_at, remaining_percent, sol_dollars, terra_dollars, luna_dollars)
                 VALUES (?1, ?2, 1e999, 1e999, 2.0, 3.0)",
                params![cutoff_epoch + 13, 1_700_000_013_i64],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            store
                .load_recent_one_month(now)
                .unwrap()
                .into_iter()
                .map(|row| row.timestamp)
                .collect::<Vec<_>>(),
            Vec::<i64>::new()
        );
        assert_eq!(history_rows(&path).len(), 5);
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn load_all_filters_negative_token_rows_without_coercion_or_deletion() {
        let path = database_path("load-all-negative-tokens");
        let valid_timestamp = 1_700_000_000_i64;
        let valid_reset_at = 1_700_000_100_i64;
        let store = UsageStore::open(&path).unwrap();
        store
            .upsert_sample(&sample(valid_timestamp, valid_reset_at, Some(50.0), 1.0))
            .unwrap();
        drop(store);

        let token_columns = ["sol_tokens", "terra_tokens", "luna_tokens"];
        let connection = Connection::open(&path).unwrap();
        for (offset, token_column) in token_columns.iter().enumerate() {
            let timestamp = valid_timestamp + offset as i64 + 1;
            let reset_at = valid_reset_at + offset as i64 + 1;
            let statement = format!(
                "INSERT INTO usage_history
                    (timestamp, reset_at, remaining_percent, sol_dollars, terra_dollars, luna_dollars, {token_column})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            );
            connection
                .execute(
                    &statement,
                    params![timestamp, reset_at, 50.0_f64, 1.0_f64, 2.0_f64, 3.0_f64, -1_i64],
                )
                .unwrap();
        }
        drop(connection);

        let store = UsageStore::open(&path).unwrap();
        let samples = store.load_all().unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].timestamp, valid_timestamp);
        drop(store);

        let connection = Connection::open(&path).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM usage_history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 4);
        for (offset, token_column) in token_columns.iter().enumerate() {
            let timestamp = valid_timestamp + offset as i64 + 1;
            let statement =
                format!("SELECT {token_column} FROM usage_history WHERE timestamp = ?1");
            let value: i64 = connection
                .query_row(&statement, params![timestamp], |row| row.get(0))
                .unwrap();
            assert_eq!(value, -1);
        }
        drop(connection);
        cleanup(&path);
    }

    #[test]
    fn grouping_has_sixty_second_boundary_canonical_ids_and_explicit_order() {
        let samples = vec![
            sample(100, 1_000, Some(1.0), 1.0),
            sample(200, 1_060, Some(2.0), 2.0),
            sample(300, 1_061, Some(3.0), 3.0),
        ];
        assert_eq!(
            reset_period_values(&group_reset_periods(&samples)),
            vec![(1_061, 300, 1_061), (1_060, 100, 300)]
        );
    }

    #[test]
    fn grouping_handles_same_timestamp_periods_mid_week_and_permutation_invariance() {
        let samples = vec![
            sample(604_700, 604_800, Some(1.0), 1.0),
            sample(604_750, 604_805, Some(2.0), 2.0),
            sample(604_900, 605_000, Some(3.0), 3.0),
            sample(605_100, 605_000, Some(4.0), 4.0),
            sample(605_200, 605_100, Some(5.0), 5.0),
            sample(605_200, 605_300, Some(6.0), 6.0),
        ];
        let expected = vec![
            (605_300, 605_200, 605_300),
            (605_100, 605_200, 605_100),
            (605_000, 604_900, 605_000),
            (604_805, 604_700, 604_805),
        ];
        assert_eq!(
            reset_period_values(&group_reset_periods(&samples)),
            expected
        );
        let mut permutation = samples.clone();
        permutation.reverse();
        assert_eq!(
            group_reset_periods(&samples),
            group_reset_periods(&permutation)
        );
    }

    #[test]
    fn grouping_orders_equal_starts_by_canonical_id_descending() {
        let samples = vec![
            sample(100, 1_000, Some(1.0), 1.0),
            sample(300, 2_000, Some(2.0), 2.0),
            sample(300, 2_061, Some(3.0), 3.0),
            sample(300, 2_122, Some(4.0), 4.0),
        ];
        assert_eq!(
            reset_period_values(&group_reset_periods(&samples)),
            vec![
                (2_122, 300, 2_122),
                (2_061, 300, 300),
                (2_000, 300, 300),
                (1_000, 100, 300)
            ]
        );
    }

    #[test]
    fn corrupt_database_error_preserves_the_original_file() {
        let path = database_path("corrupt");
        let bytes = b"this is not a sqlite database".to_vec();
        fs::write(&path, &bytes).unwrap();
        assert!(UsageStore::open(&path).is_err());
        assert!(path.exists());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        cleanup(&path);
    }

    #[test]
    fn durable_commit_is_one_transaction_and_is_visible_to_a_separate_connection() {
        let path = database_path("commit");
        let committed = sample(1_700_000_123, 1_700_000_000, Some(64.0), 1.5);
        let mut store = UsageStore::open(&path).unwrap();
        let record = store
            .commit_durable_state(
                std::slice::from_ref(&committed),
                VALID_HASH,
                r#"{"ok":true}"#,
            )
            .unwrap();
        assert_eq!(record.data_generation, 1);
        assert_eq!(
            history_rows(&path),
            vec![(
                committed.timestamp,
                committed.reset_at,
                committed.remaining_percent,
                committed.sol_dollars,
                committed.terra_dollars,
                committed.luna_dollars
            )]
        );
        assert_eq!(
            durable_row(&path),
            Some((1, VALID_HASH.to_owned(), r#"{"ok":true}"#.to_owned()))
        );
        assert_eq!(singleton_count(&path), 1);
        drop(store);
        let reopened = UsageStore::open(&path).unwrap();
        assert_eq!(reopened.load_durable_state().unwrap().unwrap(), record);
        drop(reopened);
        cleanup(&path);
    }

    #[test]
    fn validation_conflict_overflow_and_sql_failures_leave_prior_state_unchanged() {
        let path = database_path("rollback");
        let baseline = sample(1_700_000_100, 1_700_000_000, Some(70.0), 7.0);
        let mut store = UsageStore::open(&path).unwrap();
        store
            .commit_durable_state(
                std::slice::from_ref(&baseline),
                VALID_HASH,
                r#"{"generation":1}"#,
            )
            .unwrap();
        let prior_history = history_rows(&path);
        let prior_durable = durable_row(&path);

        let invalid_row = sample(1_700_000_101, 0, Some(60.0), 6.0);
        assert!(store
            .commit_durable_state(
                &[baseline.clone(), invalid_row],
                "f".repeat(64),
                r#"{"generation":2}"#,
            )
            .is_err());
        assert_eq!(history_rows(&path), prior_history);
        assert_eq!(durable_row(&path), prior_durable);

        for invalid_hash in ["A".repeat(64), "a".repeat(63), "g".repeat(64)] {
            assert!(store
                .commit_durable_state(
                    std::slice::from_ref(&baseline),
                    invalid_hash,
                    r#"{"generation":2}"#,
                )
                .is_err());
            assert_eq!(history_rows(&path), prior_history);
            assert_eq!(durable_row(&path), prior_durable);
        }

        let oversized_json = "x".repeat(MAX_SNAPSHOT_JSON_BYTES + 1);
        for invalid_json in ["{", "[]"] {
            assert!(store
                .commit_durable_state(std::slice::from_ref(&baseline), VALID_HASH, invalid_json,)
                .is_err());
            assert_eq!(history_rows(&path), prior_history);
            assert_eq!(durable_row(&path), prior_durable);
        }
        assert!(store
            .commit_durable_state(std::slice::from_ref(&baseline), VALID_HASH, &oversized_json,)
            .is_err());
        assert_eq!(history_rows(&path), prior_history);
        assert_eq!(durable_row(&path), prior_durable);

        assert!(store
            .commit_durable_state_if_generation(0, &[], VALID_HASH, r#"{"generation":2}"#,)
            .is_err());
        assert_eq!(history_rows(&path), prior_history);
        assert_eq!(durable_row(&path), prior_durable);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE durable_state SET data_generation = ?1 WHERE singleton = 1",
                params![i64::MAX],
            )
            .unwrap();
        drop(connection);
        let overflow_history = history_rows(&path);
        let overflow_durable = durable_row(&path);
        assert!(store
            .commit_durable_state_if_generation(
                u64::try_from(i64::MAX).unwrap(),
                std::slice::from_ref(&baseline),
                VALID_HASH,
                r#"{"generation":"overflow"}"#,
            )
            .is_err());
        assert_eq!(history_rows(&path), overflow_history);
        assert_eq!(durable_row(&path), overflow_durable);

        drop(store);
        cleanup(&path);
    }

    #[test]
    fn durable_update_trigger_rolls_back_history_and_durable_state() {
        let path = database_path("durable-update-trigger");
        let mut store = UsageStore::open(&path).unwrap();
        let baseline = sample(1_700_000_100, 1_700_000_000, Some(50.0), 1.0);
        store
            .commit_durable_state(
                std::slice::from_ref(&baseline),
                VALID_HASH,
                r#"{"generation":1}"#,
            )
            .unwrap();
        let captured_history = history_rows(&path);
        let captured_durable = durable_row(&path);

        let trigger_connection = Connection::open(&path).unwrap();
        trigger_connection
            .execute_batch(
                "CREATE TRIGGER wave_b_fail_durable_update
                 BEFORE UPDATE ON durable_state
                 BEGIN SELECT RAISE(ABORT, 'wave-b fault'); END;",
            )
            .unwrap();
        drop(trigger_connection);

        assert!(store
            .commit_durable_state(
                &[sample(1_700_000_200, 1_700_000_000, Some(55.0), 5.5)],
                VALID_HASH,
                r#"{"generation":2}"#,
            )
            .is_err());
        assert_eq!(history_rows(&path), captured_history);
        assert_eq!(durable_row(&path), captured_durable);

        let trigger_connection = Connection::open(&path).unwrap();
        trigger_connection
            .execute_batch("DROP TRIGGER wave_b_fail_durable_update")
            .unwrap();
        drop(trigger_connection);
        drop(store);

        let reopened = UsageStore::open(&path).unwrap();
        assert_eq!(history_rows(&path), captured_history);
        assert_eq!(durable_row(&path), captured_durable);
        drop(reopened);
        cleanup(&path);
    }

    #[test]
    fn storage_focus11_durable_absence_and_malformed_presence_are_distinct() {
        let empty_path = database_path("storage-focus11-durable-empty");
        let mut empty_store = UsageStore::open(&empty_path).unwrap();
        assert_eq!(singleton_count(&empty_path), 0);
        assert_eq!(empty_store.load_durable_state().unwrap(), None);
        let empty_record = empty_store
            .commit_durable_state_if_generation(0, &[], VALID_HASH, r#"{"kind":"empty"}"#)
            .unwrap();
        assert_eq!(empty_record.data_generation, 1);
        assert_eq!(
            durable_row(&empty_path),
            Some((1, VALID_HASH.to_owned(), r#"{"kind":"empty"}"#.to_owned()))
        );
        drop(empty_store);
        let reopened_empty = UsageStore::open(&empty_path).unwrap();
        assert_eq!(
            reopened_empty.load_durable_state().unwrap(),
            Some(empty_record)
        );
        drop(reopened_empty);
        cleanup(&empty_path);

        for (label, generation, data_hash, snapshot_json, ignore_check_constraints) in [
            (
                "negative-generation",
                -1_i64,
                VALID_HASH,
                r#"{"kind":"negative"}"#,
                true,
            ),
            (
                "invalid-hash",
                1_i64,
                "not-a-valid-hash",
                r#"{"kind":"invalid-hash"}"#,
                false,
            ),
            ("non-object-json", 1_i64, VALID_HASH, "[]", false),
        ] {
            let path = database_path(&format!("storage-focus11-durable-{label}"));
            let fixture = Connection::open(&path).unwrap();
            fixture
                .execute_batch(
                    "CREATE TABLE usage_history (
                        timestamp INTEGER NOT NULL CHECK (timestamp > 0),
                        reset_at INTEGER NOT NULL CHECK (reset_at > 0),
                        remaining_percent REAL,
                        sol_dollars REAL NOT NULL,
                        terra_dollars REAL NOT NULL,
                        luna_dollars REAL NOT NULL,
                        sol_tokens INTEGER NOT NULL DEFAULT 0,
                        terra_tokens INTEGER NOT NULL DEFAULT 0,
                        luna_tokens INTEGER NOT NULL DEFAULT 0,
                        PRIMARY KEY (reset_at, timestamp)
                    );
                    CREATE TABLE durable_state (
                        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                        data_generation INTEGER NOT NULL CHECK (data_generation >= 0),
                        data_hash TEXT NOT NULL,
                        snapshot_json TEXT NOT NULL
                    );
                    INSERT INTO usage_history (
                        timestamp, reset_at, remaining_percent,
                        sol_dollars, terra_dollars, luna_dollars,
                        sol_tokens, terra_tokens, luna_tokens
                    ) VALUES (1700000010, 1700000000, 77.0, 1.25, 2.50, 3.75, 10, 20, 30);",
                )
                .unwrap();
            if ignore_check_constraints {
                fixture
                    .execute_batch("PRAGMA ignore_check_constraints = ON;")
                    .unwrap();
            }
            fixture
                .execute(
                    "INSERT INTO durable_state
                        (singleton, data_generation, data_hash, snapshot_json)
                     VALUES (1, ?1, ?2, ?3)",
                    params![generation, data_hash, snapshot_json],
                )
                .unwrap();
            if ignore_check_constraints {
                fixture
                    .execute_batch("PRAGMA ignore_check_constraints = OFF;")
                    .unwrap();
            }
            drop(fixture);

            let store = UsageStore::open(&path).unwrap();
            assert!(store.load_durable_state().is_err());
            drop(store);

            assert_eq!(
                history_rows(&path),
                vec![(1700000010, 1700000000, Some(77.0), 1.25, 2.50, 3.75,)]
            );
            assert_eq!(
                durable_row(&path),
                Some((generation, data_hash.to_owned(), snapshot_json.to_owned()))
            );
            cleanup(&path);
        }
    }

    #[test]
    fn storage_focus11_first_insert_failure_rolls_back_history_and_durable() {
        let path = database_path("storage-focus11-first-insert-failure");
        let mut store = UsageStore::open(&path).unwrap();
        assert!(history_rows(&path).is_empty());
        assert_eq!(singleton_count(&path), 0);

        let trigger_connection = Connection::open(&path).unwrap();
        trigger_connection
            .execute_batch(
                "CREATE TRIGGER wave_b_fail_durable_insert
                 BEFORE INSERT ON durable_state
                 BEGIN SELECT RAISE(ABORT, 'wave-b first insert fault'); END;",
            )
            .unwrap();
        drop(trigger_connection);

        let candidate = sample(1_700_000_200, 1_700_000_000, Some(55.0), 5.5);
        assert!(store
            .commit_durable_state(
                std::slice::from_ref(&candidate),
                VALID_HASH,
                r#"{"generation":1}"#,
            )
            .is_err());
        assert!(history_rows(&path).is_empty());
        assert_eq!(singleton_count(&path), 0);

        drop(store);
        let reopened = UsageStore::open(&path).unwrap();
        assert!(history_rows(&path).is_empty());
        assert_eq!(singleton_count(&path), 0);
        drop(reopened);
        cleanup(&path);
    }

    #[test]
    fn invalid_input_boundaries_cover_remaining_dollars_and_token_sql_limits() {
        let path = database_path("input-boundaries");
        let mut store = UsageStore::open(&path).unwrap();
        for (index, invalid) in [
            sample(1_700_000_001, 0, Some(50.0), 1.0),
            sample(1_700_000_002, -1, Some(50.0), 1.0),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                store
                    .upsert_samples(std::slice::from_ref(&invalid))
                    .is_err(),
                "invalid fixture {index}"
            );
        }
        for (index, invalid) in [
            sample(1_700_000_003, 1_700_000_000, Some(-1.0), 1.0),
            sample(1_700_000_004, 1_700_000_000, Some(101.0), 1.0),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                store.upsert_sample(&invalid).is_err(),
                "single-row remaining_percent fixture {index}"
            );
            assert!(
                store
                    .upsert_samples(std::slice::from_ref(&invalid))
                    .is_err(),
                "batch remaining_percent fixture {index}"
            );
        }
        for (index, field) in ["sol_dollars", "terra_dollars", "luna_dollars"]
            .into_iter()
            .enumerate()
        {
            let mut invalid = sample(1_700_000_005 + index as i64, 1_700_000_000, Some(50.0), 1.0);
            match field {
                "sol_dollars" => invalid.sol_dollars = -1.0,
                "terra_dollars" => invalid.terra_dollars = -1.0,
                "luna_dollars" => invalid.luna_dollars = -1.0,
                _ => unreachable!(),
            }
            assert!(
                store.upsert_sample(&invalid).is_err(),
                "single-row negative {field} fixture"
            );
            assert!(
                store
                    .upsert_samples(std::slice::from_ref(&invalid))
                    .is_err(),
                "batch negative {field} fixture"
            );
        }
        let valid = sample(1_700_000_010, 1_700_000_000, Some(50.0), 1.0);
        let mut invalid = valid.clone();
        invalid.timestamp += 1;
        invalid.sol_dollars = -1.0;
        assert!(store.upsert_samples(&[valid, invalid]).is_err());
        assert!(history_rows(&path).is_empty());
        let overflowing = overflowing_token_sample();
        assert!(store
            .upsert_samples(std::slice::from_ref(&overflowing))
            .is_err());
        assert!(history_rows(&path).is_empty());
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn storage_focus11_public_write_numeric_partition_table() {
        let path = database_path("storage-focus11-public-write-numeric-partitions");
        let mut store = UsageStore::open(&path).unwrap();

        let sql_rows = |path: &std::path::Path| {
            let connection = Connection::open(path).unwrap();
            let mut statement = connection
                .prepare(
                    "SELECT timestamp, reset_at, remaining_percent,
                            sol_dollars, terra_dollars, luna_dollars,
                            sol_tokens, terra_tokens, luna_tokens
                     FROM usage_history ORDER BY reset_at, timestamp",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, f64>(4)?,
                        row.get::<_, f64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                    ))
                })
                .unwrap()
                .map(|row| row.unwrap())
                .collect::<Vec<_>>()
        };
        let durable_sql = |path: &std::path::Path| {
            let connection = Connection::open(path).unwrap();
            match connection.query_row(
                "SELECT data_generation, data_hash, snapshot_json
                 FROM durable_state WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            ) {
                Ok(value) => Some(value),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(error) => panic!("durable query failed: {error}"),
            }
        };

        let valid_none = UsageHistorySample {
            timestamp: 1,
            reset_at: 1,
            remaining_percent: None,
            sol_dollars: 0.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: 0,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        store.upsert_sample(&valid_none).unwrap();

        let valid_zero = UsageHistorySample {
            timestamp: 2,
            reset_at: 1,
            remaining_percent: Some(0.0),
            sol_dollars: 0.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: i64::MAX as u64,
            terra_tokens: i64::MAX as u64,
            luna_tokens: i64::MAX as u64,
        };
        store
            .upsert_samples(std::slice::from_ref(&valid_zero))
            .unwrap();

        let valid_full = UsageHistorySample {
            timestamp: 3,
            reset_at: 1,
            remaining_percent: Some(100.0),
            sol_dollars: 0.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: 0,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        store
            .commit_durable_state(
                std::slice::from_ref(&valid_full),
                VALID_HASH,
                r#"{"kind":"focus11b"}"#,
            )
            .unwrap();

        assert_eq!(
            store.load_all().unwrap(),
            vec![valid_none.clone(), valid_zero.clone(), valid_full.clone()]
        );
        assert_eq!(
            sql_rows(&path),
            vec![
                (1, 1, None, 0.0, 0.0, 0.0, 0, 0, 0),
                (2, 1, Some(0.0), 0.0, 0.0, 0.0, i64::MAX, i64::MAX, i64::MAX,),
                (3, 1, Some(100.0), 0.0, 0.0, 0.0, 0, 0, 0),
            ]
        );
        assert_eq!(
            durable_sql(&path),
            Some((
                1,
                VALID_HASH.to_owned(),
                r#"{"kind":"focus11b"}"#.to_owned()
            ))
        );

        let baseline_history = sql_rows(&path);
        let baseline_durable = durable_sql(&path);
        let base_invalid = UsageHistorySample {
            timestamp: 10,
            reset_at: 10,
            remaining_percent: Some(50.0),
            sol_dollars: 1.0,
            terra_dollars: 2.0,
            luna_dollars: 3.0,
            sol_tokens: 4,
            terra_tokens: 5,
            luna_tokens: 6,
        };
        let invalids = vec![
            (
                "timestamp-zero",
                UsageHistorySample {
                    timestamp: 0,
                    ..base_invalid.clone()
                },
            ),
            (
                "timestamp-negative",
                UsageHistorySample {
                    timestamp: -1,
                    ..base_invalid.clone()
                },
            ),
            (
                "reset-zero",
                UsageHistorySample {
                    reset_at: 0,
                    ..base_invalid.clone()
                },
            ),
            (
                "reset-negative",
                UsageHistorySample {
                    reset_at: -1,
                    ..base_invalid.clone()
                },
            ),
            (
                "remaining-negative",
                UsageHistorySample {
                    remaining_percent: Some(-1.0),
                    ..base_invalid.clone()
                },
            ),
            (
                "remaining-101",
                UsageHistorySample {
                    remaining_percent: Some(101.0),
                    ..base_invalid.clone()
                },
            ),
            (
                "remaining-nan",
                UsageHistorySample {
                    remaining_percent: Some(f64::NAN),
                    ..base_invalid.clone()
                },
            ),
            (
                "remaining-positive-infinity",
                UsageHistorySample {
                    remaining_percent: Some(f64::INFINITY),
                    ..base_invalid.clone()
                },
            ),
            (
                "remaining-negative-infinity",
                UsageHistorySample {
                    remaining_percent: Some(f64::NEG_INFINITY),
                    ..base_invalid.clone()
                },
            ),
            (
                "sol-negative",
                UsageHistorySample {
                    sol_dollars: -1.0,
                    ..base_invalid.clone()
                },
            ),
            (
                "sol-nan",
                UsageHistorySample {
                    sol_dollars: f64::NAN,
                    ..base_invalid.clone()
                },
            ),
            (
                "sol-positive-infinity",
                UsageHistorySample {
                    sol_dollars: f64::INFINITY,
                    ..base_invalid.clone()
                },
            ),
            (
                "sol-negative-infinity",
                UsageHistorySample {
                    sol_dollars: f64::NEG_INFINITY,
                    ..base_invalid.clone()
                },
            ),
            (
                "terra-negative",
                UsageHistorySample {
                    terra_dollars: -1.0,
                    ..base_invalid.clone()
                },
            ),
            (
                "terra-nan",
                UsageHistorySample {
                    terra_dollars: f64::NAN,
                    ..base_invalid.clone()
                },
            ),
            (
                "terra-positive-infinity",
                UsageHistorySample {
                    terra_dollars: f64::INFINITY,
                    ..base_invalid.clone()
                },
            ),
            (
                "terra-negative-infinity",
                UsageHistorySample {
                    terra_dollars: f64::NEG_INFINITY,
                    ..base_invalid.clone()
                },
            ),
            (
                "luna-negative",
                UsageHistorySample {
                    luna_dollars: -1.0,
                    ..base_invalid.clone()
                },
            ),
            (
                "luna-nan",
                UsageHistorySample {
                    luna_dollars: f64::NAN,
                    ..base_invalid.clone()
                },
            ),
            (
                "luna-positive-infinity",
                UsageHistorySample {
                    luna_dollars: f64::INFINITY,
                    ..base_invalid.clone()
                },
            ),
            (
                "luna-negative-infinity",
                UsageHistorySample {
                    luna_dollars: f64::NEG_INFINITY,
                    ..base_invalid.clone()
                },
            ),
            (
                "token-overflow",
                UsageHistorySample {
                    sol_tokens: i64::MAX as u64 + 1,
                    ..base_invalid.clone()
                },
            ),
        ];
        for (label, invalid) in invalids {
            assert!(store.upsert_sample(&invalid).is_err(), "single {label}");
            assert_eq!(sql_rows(&path), baseline_history);
            assert_eq!(durable_sql(&path), baseline_durable);
            assert!(
                store
                    .upsert_samples(std::slice::from_ref(&invalid))
                    .is_err(),
                "batch {label}"
            );
            assert_eq!(sql_rows(&path), baseline_history);
            assert_eq!(durable_sql(&path), baseline_durable);
            assert!(
                store
                    .commit_durable_state(
                        std::slice::from_ref(&invalid),
                        VALID_HASH,
                        r#"{"kind":"invalid"}"#,
                    )
                    .is_err(),
                "durable {label}"
            );
            assert_eq!(sql_rows(&path), baseline_history);
            assert_eq!(durable_sql(&path), baseline_durable);
        }

        let mixed_valid = UsageHistorySample {
            timestamp: 100,
            reset_at: 100,
            remaining_percent: Some(75.0),
            sol_dollars: 0.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: 0,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        let mixed_invalid = UsageHistorySample {
            timestamp: 101,
            reset_at: 100,
            sol_dollars: -1.0,
            ..mixed_valid.clone()
        };
        assert!(store
            .upsert_samples(&[mixed_valid.clone(), mixed_invalid.clone()])
            .is_err());
        assert_eq!(sql_rows(&path), baseline_history);
        assert_eq!(durable_sql(&path), baseline_durable);
        assert!(store
            .commit_durable_state(
                &[mixed_valid, mixed_invalid],
                VALID_HASH,
                r#"{"kind":"mixed-invalid"}"#,
            )
            .is_err());
        assert_eq!(sql_rows(&path), baseline_history);
        assert_eq!(durable_sql(&path), baseline_durable);

        drop(store);
        cleanup(&path);
    }

    #[test]
    fn storage_focus11_nonpruning_uses_fixed_utc_epoch_oracle() {
        use chrono::TimeZone;

        let now = Utc.timestamp_opt(1715156800, 0).single().unwrap();
        let old = UsageHistorySample {
            timestamp: 1600000000,
            reset_at: 1600000000,
            remaining_percent: Some(10.0),
            sol_dollars: 1.0,
            terra_dollars: 2.0,
            luna_dollars: 3.0,
            sol_tokens: 4,
            terra_tokens: 5,
            luna_tokens: 6,
        };
        let recent = UsageHistorySample {
            timestamp: 1715156700,
            reset_at: 1715156800,
            remaining_percent: Some(20.0),
            sol_dollars: 7.0,
            terra_dollars: 8.0,
            luna_dollars: 9.0,
            sol_tokens: 10,
            terra_tokens: 11,
            luna_tokens: 12,
        };
        let path = database_path("storage-focus11-nonpruning-fixed-utc");
        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&old).unwrap();
        store.upsert_sample(&recent).unwrap();

        let count_rows = |path: &std::path::Path| -> i64 {
            let connection = Connection::open(path).unwrap();
            connection
                .query_row("SELECT COUNT(*) FROM usage_history", [], |row| row.get(0))
                .unwrap()
        };
        let old_is_present = |path: &std::path::Path| -> bool {
            let connection = Connection::open(path).unwrap();
            connection
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM usage_history WHERE timestamp = 1600000000
                    )",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };

        assert_eq!(count_rows(&path), 2);
        assert!(old_is_present(&path));
        assert_eq!(
            store.load_recent_one_month(now).unwrap(),
            vec![recent.clone()]
        );
        assert_eq!(count_rows(&path), 2);
        assert!(old_is_present(&path));

        store.upsert_sample(&recent).unwrap();
        assert_eq!(count_rows(&path), 2);
        assert!(old_is_present(&path));
        drop(store);
        cleanup(&path);
    }

    #[test]
    fn history_component_recovery_is_atomic_idempotent_and_disables_sample_offset() {
        let path = database_path("history-component-recovery");
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "f".repeat(64),
            storage_epoch: 1,
            partition_id: "f".repeat(64),
        };
        let reset_at = 1_800_604_800;
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let current = SessionModelTotal {
            cache_write_input_tokens: None,
            model: "SOL".into(),
            total_tokens: 100,
            input_tokens: 80,
            cached_input_tokens: 30,
            output_tokens: 20,
        };
        assert_eq!(
            store
                .commit_session_collection(SessionCollectionCommit {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: 0x138,
                    cycle_seq: 1,
                    samples: &[],
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: std::slice::from_ref(&current),
                    recorded_sessions: &[],
                })
                .unwrap(),
            1
        );
        store
            .connection
            .execute(
                "INSERT INTO history_continuity (
                    singleton, source_fingerprint, source_rows, boundary_timestamp,
                    reset_at, remaining_percent, sol_dollars, terra_dollars,
                    luna_dollars, sol_tokens, terra_tokens, luna_tokens,
                    model_totals_applied
                 ) VALUES (1, ?1, 2, ?2, ?3, 50.0, 8.65, 0.0, 0.0,
                           '1000000', '0', '0', 0)",
                params!["aaaaaaaaaaaaaaaa", 1_800_000_120_i64, reset_at],
            )
            .unwrap();
        let authority = store
            .pending_history_continuity_recovery()
            .unwrap()
            .unwrap();
        let offset = SessionModelTotal {
            cache_write_input_tokens: None,
            model: "SOL".into(),
            total_tokens: 1_000_000,
            input_tokens: 800_000,
            cached_input_tokens: 300_000,
            output_tokens: 200_000,
        };

        let wrong = HistoryContinuityModelRecovery {
            authority: authority.clone(),
            model_totals: vec![SessionModelTotal {
                total_tokens: 999_999,
                ..offset.clone()
            }],
            fallback_samples: Vec::new(),
            fallback_model_totals: Vec::new(),
        };
        assert!(store.apply_history_continuity_model_totals(&wrong).is_err());
        assert_eq!(
            store
                .load_session_collection_state()
                .unwrap()
                .data_generation,
            1
        );
        assert!(store
            .pending_history_continuity_recovery()
            .unwrap()
            .is_some());

        let recovery = HistoryContinuityModelRecovery {
            authority,
            model_totals: vec![offset],
            fallback_samples: Vec::new(),
            fallback_model_totals: Vec::new(),
        };
        assert_eq!(
            store
                .apply_history_continuity_model_totals(&recovery)
                .unwrap(),
            2
        );
        assert_eq!(
            store.load_session_collection_state().unwrap().model_totals,
            vec![SessionModelTotal {
                cache_write_input_tokens: None,
                model: "SOL".into(),
                total_tokens: 1_000_100,
                input_tokens: 800_080,
                cached_input_tokens: 300_030,
                output_tokens: 200_020,
            }]
        );
        assert!(store
            .pending_history_continuity_recovery()
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .apply_history_continuity_model_totals(&recovery)
                .unwrap(),
            2
        );

        let combined_sample = UsageHistorySample {
            timestamp: 1_800_000_180,
            reset_at,
            remaining_percent: Some(49.0),
            sol_dollars: 8.651,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: 1_000_100,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        assert_eq!(
            store
                .commit_session_collection_with_samples(SessionCollectionCommit {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: 0x138,
                    cycle_seq: 2,
                    samples: std::slice::from_ref(&combined_sample),
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &[SessionModelTotal {
                        cache_write_input_tokens: None,
                        model: "SOL".into(),
                        total_tokens: 1_000_100,
                        input_tokens: 800_080,
                        cached_input_tokens: 300_030,
                        output_tokens: 200_020,
                    }],
                    recorded_sessions: &[],
                })
                .unwrap()
                .data_generation,
            3
        );
        assert_eq!(store.load_all().unwrap(), vec![combined_sample]);
        drop(store);
        cleanup(&path);
    }
}

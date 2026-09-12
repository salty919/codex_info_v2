// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

use chrono::{DateTime, Months, Utc};
use codex_info_db_reader::{
    canonicalize_history_for_storage_with_sources, RawSample as CanonicalRawSample,
    HISTORY_CANONICAL_SCHEMA_VERSION,
};
use rusqlite::types::Value;
use rusqlite::{
    params, Connection, DatabaseName, OpenFlags, OptionalExtension, TransactionBehavior,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
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
    partition_id TEXT NOT NULL,
    login_id TEXT
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

CREATE TABLE session_pending_ranges (
    root_identity TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    file_device TEXT NOT NULL,
    file_inode TEXT NOT NULL,
    start_offset INTEGER NOT NULL CHECK (start_offset >= 0),
    end_offset INTEGER NOT NULL CHECK (end_offset >= start_offset),
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
    parser_version TEXT NOT NULL CHECK (length(parser_version) BETWEEN 1 AND 128),
    reason TEXT NOT NULL CHECK (length(reason) BETWEEN 1 AND 512),
    complete INTEGER NOT NULL CHECK (complete IN (0, 1)),
    PRIMARY KEY (
        root_identity,
        relative_path,
        file_device,
        file_inode,
        prefix_generation,
        start_offset
    )
) WITHOUT ROWID;

CREATE TABLE session_events (
    root_identity TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    file_device TEXT NOT NULL,
    file_inode TEXT NOT NULL,
    prefix_generation TEXT NOT NULL CHECK (
        length(prefix_generation) = 32
        AND prefix_generation NOT GLOB '*[^0-9a-f]*'
    ),
    range_start INTEGER NOT NULL CHECK (range_start >= 0),
    range_end INTEGER NOT NULL CHECK (range_end > range_start),
    record_sha256 TEXT NOT NULL CHECK (
        length(record_sha256) = 64
        AND record_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    event_index INTEGER NOT NULL CHECK (event_index >= 0),
    timestamp INTEGER NOT NULL CHECK (timestamp > 0),
    model TEXT NOT NULL CHECK (length(model) BETWEEN 1 AND 512),
    total_tokens TEXT NOT NULL,
    input_tokens TEXT NOT NULL,
    cached_input_tokens TEXT NOT NULL,
    output_tokens TEXT NOT NULL,
    cache_write_input_tokens TEXT,
    PRIMARY KEY (
        root_identity,
        relative_path,
        file_device,
        file_inode,
        prefix_generation,
        range_start,
        range_end,
        record_sha256,
        event_index
    )
) WITHOUT ROWID;

CREATE TABLE session_task_events (
    root_identity TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    file_device TEXT NOT NULL,
    file_inode TEXT NOT NULL,
    prefix_generation TEXT NOT NULL CHECK (
        length(prefix_generation) = 32
        AND prefix_generation NOT GLOB '*[^0-9a-f]*'
    ),
    start_offset INTEGER NOT NULL CHECK (start_offset >= 0),
    end_offset INTEGER NOT NULL CHECK (end_offset > start_offset),
    record_sha256 TEXT NOT NULL CHECK (
        length(record_sha256) = 64
        AND record_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    event_index INTEGER NOT NULL CHECK (event_index >= 0),
    timestamp INTEGER NOT NULL CHECK (timestamp > 0),
    running INTEGER NOT NULL CHECK (running IN (0, 1)),
    PRIMARY KEY (
        root_identity,
        relative_path,
        file_device,
        file_inode,
        prefix_generation,
        start_offset,
        end_offset,
        record_sha256,
        event_index
    )
) WITHOUT ROWID;

CREATE TABLE session_task_indexed_ranges (
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

CREATE TABLE session_cumulative_recoveries (
    recovery_id TEXT PRIMARY KEY CHECK (
        length(recovery_id) = 64
        AND recovery_id NOT GLOB '*[^0-9a-f]*'
    ),
    payload_json TEXT NOT NULL CHECK (
        length(payload_json) BETWEEN 2 AND 1048576
    ),
    applied_generation TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE session_timeline_recoveries (
    recovery_id TEXT PRIMARY KEY CHECK (
        length(recovery_id) = 64
        AND recovery_id NOT GLOB '*[^0-9a-f]*'
    ),
    payload_json TEXT NOT NULL CHECK (
        length(payload_json) BETWEEN 2 AND 67108864
    ),
    applied_generation TEXT NOT NULL
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

CREATE TABLE active_thread_snapshot (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    observed_at INTEGER NOT NULL CHECK (observed_at > 0),
    threads_json TEXT NOT NULL CHECK (length(threads_json) BETWEEN 2 AND 1048576),
    acquisition_degraded INTEGER NOT NULL DEFAULT 0 CHECK (acquisition_degraded IN (0, 1))
);
"#;

const HISTORY_CANONICAL_CONSTRAINTS: &str = r#"
CREATE UNIQUE INDEX usage_history_canonical_timestamp_idx
    ON usage_history (timestamp);
CREATE UNIQUE INDEX usage_model_history_canonical_timestamp_model_idx
    ON usage_model_history (timestamp, model);

CREATE TRIGGER usage_history_canonical_insert_guard
BEFORE INSERT ON usage_history
WHEN typeof(NEW.timestamp) <> 'integer'
  OR typeof(NEW.reset_at) <> 'integer'
  OR NEW.timestamp <= 0 OR NEW.reset_at <= 0 OR NEW.timestamp > NEW.reset_at
  OR (NEW.remaining_percent IS NOT NULL AND (
      typeof(NEW.remaining_percent) NOT IN ('integer', 'real')
      OR NEW.remaining_percent < 0.0 OR NEW.remaining_percent > 100.0
  ))
  OR typeof(NEW.sol_dollars) NOT IN ('integer', 'real')
  OR typeof(NEW.terra_dollars) NOT IN ('integer', 'real')
  OR typeof(NEW.luna_dollars) NOT IN ('integer', 'real')
  OR NEW.sol_dollars < 0.0 OR NEW.sol_dollars >= 1e999
  OR NEW.terra_dollars < 0.0 OR NEW.terra_dollars >= 1e999
  OR NEW.luna_dollars < 0.0 OR NEW.luna_dollars >= 1e999
  OR typeof(NEW.sol_tokens) <> 'integer' OR NEW.sol_tokens < 0
  OR typeof(NEW.terra_tokens) <> 'integer' OR NEW.terra_tokens < 0
  OR typeof(NEW.luna_tokens) <> 'integer' OR NEW.luna_tokens < 0
BEGIN
    SELECT RAISE(ABORT, 'invalid canonical usage history row');
END;

CREATE TRIGGER usage_history_canonical_update_guard
BEFORE UPDATE ON usage_history
WHEN typeof(NEW.timestamp) <> 'integer'
  OR typeof(NEW.reset_at) <> 'integer'
  OR NEW.timestamp <= 0 OR NEW.reset_at <= 0 OR NEW.timestamp > NEW.reset_at
  OR (NEW.remaining_percent IS NOT NULL AND (
      typeof(NEW.remaining_percent) NOT IN ('integer', 'real')
      OR NEW.remaining_percent < 0.0 OR NEW.remaining_percent > 100.0
  ))
  OR typeof(NEW.sol_dollars) NOT IN ('integer', 'real')
  OR typeof(NEW.terra_dollars) NOT IN ('integer', 'real')
  OR typeof(NEW.luna_dollars) NOT IN ('integer', 'real')
  OR NEW.sol_dollars < 0.0 OR NEW.sol_dollars >= 1e999
  OR NEW.terra_dollars < 0.0 OR NEW.terra_dollars >= 1e999
  OR NEW.luna_dollars < 0.0 OR NEW.luna_dollars >= 1e999
  OR typeof(NEW.sol_tokens) <> 'integer' OR NEW.sol_tokens < 0
  OR typeof(NEW.terra_tokens) <> 'integer' OR NEW.terra_tokens < 0
  OR typeof(NEW.luna_tokens) <> 'integer' OR NEW.luna_tokens < 0
BEGIN
    SELECT RAISE(ABORT, 'invalid canonical usage history row');
END;

CREATE TRIGGER usage_model_history_canonical_insert_guard
BEFORE INSERT ON usage_model_history
WHEN NOT EXISTS (
    SELECT 1 FROM usage_history
     WHERE timestamp=NEW.timestamp AND reset_at=NEW.reset_at
)
BEGIN
    SELECT RAISE(ABORT, 'orphan usage model history row');
END;

CREATE TRIGGER usage_model_history_canonical_update_guard
BEFORE UPDATE ON usage_model_history
WHEN NOT EXISTS (
    SELECT 1 FROM usage_history
     WHERE timestamp=NEW.timestamp AND reset_at=NEW.reset_at
)
BEGIN
    SELECT RAISE(ABORT, 'orphan usage model history row');
END;

CREATE TRIGGER durable_history_observation_insert_guard
BEFORE INSERT ON durable_state
WHEN NEW.singleton >= 2 AND CASE
    WHEN NOT json_valid(NEW.snapshot_json) THEN 1
    ELSE
        json_extract(NEW.snapshot_json, '$.kind') IS NOT 'codex-info-usage-observation-v1'
        OR json_type(NEW.snapshot_json, '$.timestamp') IS NOT 'integer'
        OR json_extract(NEW.snapshot_json, '$.timestamp') IS NOT NEW.data_generation
        OR json_type(NEW.snapshot_json, '$.reset_at') IS NOT 'integer'
        OR json_extract(NEW.snapshot_json, '$.reset_at') <= 0
        OR json_type(NEW.snapshot_json, '$.remaining_percent') IS NULL
        OR json_type(NEW.snapshot_json, '$.remaining_percent') NOT IN ('null', 'integer', 'real')
        OR json_extract(NEW.snapshot_json, '$.remaining_percent') < 0.0
        OR json_extract(NEW.snapshot_json, '$.remaining_percent') > 100.0
        OR json_extract(NEW.snapshot_json, '$.model_source') IS NULL
        OR json_extract(NEW.snapshot_json, '$.model_source') NOT IN (
            'confirmed', 'reconstructed-from-session', 'unavailable', 'legacy-unknown'
        )
        OR (
            json_extract(NEW.snapshot_json, '$.model_source') <> 'unavailable'
            AND NOT EXISTS (
                SELECT 1 FROM usage_history AS history
                 WHERE history.timestamp=NEW.data_generation
                   AND history.reset_at=json_extract(NEW.snapshot_json, '$.reset_at')
                   AND json_extract(NEW.snapshot_json, '$.remaining_percent') IS history.remaining_percent
                   AND json_extract(NEW.snapshot_json, '$.sol_dollars') IS history.sol_dollars
                   AND json_extract(NEW.snapshot_json, '$.terra_dollars') IS history.terra_dollars
                   AND json_extract(NEW.snapshot_json, '$.luna_dollars') IS history.luna_dollars
                   AND json_extract(NEW.snapshot_json, '$.sol_tokens') IS history.sol_tokens
                   AND json_extract(NEW.snapshot_json, '$.terra_tokens') IS history.terra_tokens
                   AND json_extract(NEW.snapshot_json, '$.luna_tokens') IS history.luna_tokens
            )
        )
        OR (
            json_extract(NEW.snapshot_json, '$.model_source') = 'unavailable'
            AND (
                json_type(NEW.snapshot_json, '$.sol_dollars') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.terra_dollars') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.luna_dollars') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.sol_tokens') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.terra_tokens') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.luna_tokens') IS NOT 'null'
                OR EXISTS (
                    SELECT 1 FROM usage_history AS history
                     WHERE history.timestamp=NEW.data_generation
                )
            )
        )
END
BEGIN
    SELECT RAISE(ABORT, 'orphan durable history observation');
END;

CREATE TRIGGER durable_history_observation_update_guard
BEFORE UPDATE ON durable_state
WHEN NEW.singleton >= 2 AND CASE
    WHEN NOT json_valid(NEW.snapshot_json) THEN 1
    ELSE
        json_extract(NEW.snapshot_json, '$.kind') IS NOT 'codex-info-usage-observation-v1'
        OR json_type(NEW.snapshot_json, '$.timestamp') IS NOT 'integer'
        OR json_extract(NEW.snapshot_json, '$.timestamp') IS NOT NEW.data_generation
        OR json_type(NEW.snapshot_json, '$.reset_at') IS NOT 'integer'
        OR json_extract(NEW.snapshot_json, '$.reset_at') <= 0
        OR json_type(NEW.snapshot_json, '$.remaining_percent') IS NULL
        OR json_type(NEW.snapshot_json, '$.remaining_percent') NOT IN ('null', 'integer', 'real')
        OR json_extract(NEW.snapshot_json, '$.remaining_percent') < 0.0
        OR json_extract(NEW.snapshot_json, '$.remaining_percent') > 100.0
        OR json_extract(NEW.snapshot_json, '$.model_source') IS NULL
        OR json_extract(NEW.snapshot_json, '$.model_source') NOT IN (
            'confirmed', 'reconstructed-from-session', 'unavailable', 'legacy-unknown'
        )
        OR (
            json_extract(NEW.snapshot_json, '$.model_source') <> 'unavailable'
            AND NOT EXISTS (
                SELECT 1 FROM usage_history AS history
                 WHERE history.timestamp=NEW.data_generation
                   AND history.reset_at=json_extract(NEW.snapshot_json, '$.reset_at')
                   AND json_extract(NEW.snapshot_json, '$.remaining_percent') IS history.remaining_percent
                   AND json_extract(NEW.snapshot_json, '$.sol_dollars') IS history.sol_dollars
                   AND json_extract(NEW.snapshot_json, '$.terra_dollars') IS history.terra_dollars
                   AND json_extract(NEW.snapshot_json, '$.luna_dollars') IS history.luna_dollars
                   AND json_extract(NEW.snapshot_json, '$.sol_tokens') IS history.sol_tokens
                   AND json_extract(NEW.snapshot_json, '$.terra_tokens') IS history.terra_tokens
                   AND json_extract(NEW.snapshot_json, '$.luna_tokens') IS history.luna_tokens
            )
        )
        OR (
            json_extract(NEW.snapshot_json, '$.model_source') = 'unavailable'
            AND (
                json_type(NEW.snapshot_json, '$.sol_dollars') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.terra_dollars') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.luna_dollars') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.sol_tokens') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.terra_tokens') IS NOT 'null'
                OR json_type(NEW.snapshot_json, '$.luna_tokens') IS NOT 'null'
                OR EXISTS (
                    SELECT 1 FROM usage_history AS history
                     WHERE history.timestamp=NEW.data_generation
                )
            )
        )
END
BEGIN
    SELECT RAISE(ABORT, 'orphan durable history observation');
END;

CREATE TRIGGER usage_history_sidecar_update_guard
BEFORE UPDATE OF timestamp, reset_at ON usage_history
WHEN EXISTS (
    SELECT 1 FROM usage_model_history
     WHERE timestamp=OLD.timestamp AND reset_at=OLD.reset_at
) OR EXISTS (
    SELECT 1 FROM durable_state
     WHERE singleton >= 2 AND data_generation=OLD.timestamp
       AND json_valid(snapshot_json)
       AND json_extract(snapshot_json, '$.reset_at')=OLD.reset_at
)
BEGIN
    SELECT RAISE(ABORT, 'canonical history key still has sidecars');
END;

CREATE TRIGGER usage_history_sidecar_delete_guard
BEFORE DELETE ON usage_history
WHEN EXISTS (
    SELECT 1 FROM usage_model_history
     WHERE timestamp=OLD.timestamp AND reset_at=OLD.reset_at
) OR EXISTS (
    SELECT 1 FROM durable_state
     WHERE singleton >= 2 AND data_generation=OLD.timestamp
       AND json_valid(snapshot_json)
       AND json_extract(snapshot_json, '$.reset_at')=OLD.reset_at
)
BEGIN
    SELECT RAISE(ABORT, 'canonical history key still has sidecars');
END;
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
const ACCOUNT_DB_SCHEMA_VERSION: i64 = HISTORY_CANONICAL_SCHEMA_VERSION;
const MAX_LOGIN_ID_SCALARS: usize = 254;
const MAX_ACTIVE_THREADS: usize = 256;
const MAX_ACTIVE_THREAD_ID_SCALARS: usize = 512;
const MAX_ACTIVE_THREAD_TITLE_SCALARS: usize = 512;
const MAX_ACTIVE_THREAD_MODEL_SCALARS: usize = 128;
const MAX_ACTIVE_THREAD_MODEL_LABEL_SCALARS: usize = 24;
const MAX_ACTIVE_THREAD_JSON_BYTES: usize = 1024 * 1024;
const MAX_PUBLIC_UNIX_SECONDS: i64 = 253_402_300_799;
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

const INSERT_SAMPLE_IF_ABSENT: &str = r#"
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
ON CONFLICT (reset_at, timestamp) DO NOTHING
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

/// One active Session thread accepted for publication.  This is intentionally
/// a writer-owned DTO: the recorder validates the native candidate before it
/// crosses into the account partition, while the read-only reader projects
/// the same JSON shape into its public REST DTO.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveThreadRecord {
    pub id: String,
    pub updated_at: i64,
    pub title: String,
    pub parent_thread_id: Option<String>,
    pub model: String,
    pub model_label: String,
    pub total_tokens: Option<u64>,
    pub context_usage_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub created_at: Option<i64>,
    pub last_user_message_at: Option<i64>,
    pub is_subagent: bool,
    pub depth: Option<i32>,
}

/// A complete active-thread publication candidate.  An empty `threads` value
/// is meaningful and commits as a verified empty set; callers must not use a
/// failed/partial candidate as an empty replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveThreadSnapshot {
    pub observed_at: i64,
    pub threads: Vec<ActiveThreadRecord>,
}

fn active_thread_text_valid(value: &str, max_scalars: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= max_scalars
        && !value.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
}

fn valid_login_id(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= MAX_LOGIN_ID_SCALARS
        && !value.chars().any(char::is_control)
}

fn validate_active_thread_record(record: &ActiveThreadRecord) -> Result<()> {
    if record.updated_at <= 0
        || record.updated_at > MAX_PUBLIC_UNIX_SECONDS
        || !active_thread_text_valid(&record.id, MAX_ACTIVE_THREAD_ID_SCALARS)
        || !active_thread_text_valid(&record.title, MAX_ACTIVE_THREAD_TITLE_SCALARS)
        || !active_thread_text_valid(&record.model, MAX_ACTIVE_THREAD_MODEL_SCALARS)
        || !active_thread_text_valid(&record.model_label, MAX_ACTIVE_THREAD_MODEL_LABEL_SCALARS)
        || record
            .parent_thread_id
            .as_deref()
            .is_some_and(|value| !active_thread_text_valid(value, MAX_ACTIVE_THREAD_ID_SCALARS))
        || record
            .created_at
            .is_some_and(|value| !(1..=MAX_PUBLIC_UNIX_SECONDS).contains(&value))
        || record
            .last_user_message_at
            .is_some_and(|value| !(1..=MAX_PUBLIC_UNIX_SECONDS).contains(&value))
        || record
            .depth
            .is_some_and(|value| !(0..=1024).contains(&value))
    {
        return Err(UsageStoreError::InvalidImport(
            "active thread contains an invalid scalar".into(),
        ));
    }
    Ok(())
}

fn canonical_active_thread_snapshot(snapshot: &ActiveThreadSnapshot) -> Result<(i64, String)> {
    if !(1..=MAX_PUBLIC_UNIX_SECONDS).contains(&snapshot.observed_at) {
        return Err(UsageStoreError::InvalidTimestamp {
            field: "active thread snapshot",
            value: snapshot.observed_at,
        });
    }
    if snapshot.threads.len() > MAX_ACTIVE_THREADS {
        return Err(UsageStoreError::InvalidImport(
            "active thread snapshot contains too many threads".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut threads = snapshot.threads.clone();
    for thread in &threads {
        validate_active_thread_record(thread)?;
        if !ids.insert(thread.id.as_str()) {
            return Err(UsageStoreError::InvalidImport(
                "active thread snapshot contains duplicate ids".into(),
            ));
        }
    }
    // The public contract's canonical order is newest activity first, then
    // descending id.  Sorting at the storage edge makes equivalent recorder
    // candidates serialize to one byte representation.
    threads.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    let rows = threads
        .iter()
        .map(|thread| {
            serde_json::json!({
                "id": &thread.id,
                "updated_at": thread.updated_at,
                "title": &thread.title,
                "parent_thread_id": &thread.parent_thread_id,
                "model": &thread.model,
                "model_label": &thread.model_label,
                "total_tokens": thread.total_tokens,
                "context_usage_tokens": thread.context_usage_tokens,
                "context_window_tokens": thread.context_window_tokens,
                "created_at": thread.created_at,
                "last_user_message_at": thread.last_user_message_at,
                "is_subagent": thread.is_subagent,
                "depth": thread.depth,
            })
        })
        .collect::<Vec<_>>();
    let encoded = serde_json::to_string(&rows).map_err(|error| {
        UsageStoreError::InvalidImport(format!(
            "active thread snapshot is not serializable: {error}"
        ))
    })?;
    if encoded.len() > MAX_ACTIVE_THREAD_JSON_BYTES {
        return Err(UsageStoreError::InvalidImport(
            "active thread snapshot JSON is too large".into(),
        ));
    }
    Ok((snapshot.observed_at, encoded))
}

/// Provenance of the local model vector for one history observation.
///
/// The legacy `usage_history` table cannot be extended without breaking the
/// v1.0.28 reader.  New observations therefore use this explicit sidecar
/// record while the old nine-column row remains the v1 projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelSource {
    Confirmed,
    ReconstructedFromSession,
    Unavailable,
    LegacyUnknown,
}

impl ModelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::ReconstructedFromSession => "reconstructed-from-session",
            Self::Unavailable => "unavailable",
            Self::LegacyUnknown => "legacy-unknown",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "confirmed" => Some(Self::Confirmed),
            "reconstructed-from-session" => Some(Self::ReconstructedFromSession),
            "unavailable" => Some(Self::Unavailable),
            "legacy-unknown" => Some(Self::LegacyUnknown),
            _ => None,
        }
    }
}

/// One bounded internal model/quota record, including local-source provenance.
/// Model fields are all present for `confirmed`, internal audit-only
/// `reconstructed-from-session`, and `legacy-unknown`, and all absent for
/// `unavailable`; mixed vectors are rejected at the storage edge. Public
/// readers must strip reconstructed model numerics and expose only their
/// source/time/quota metadata.
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
            ModelSource::Confirmed
            | ModelSource::ReconstructedFromSession
            | ModelSource::LegacyUnknown
                if !all_values || !all_tokens =>
            {
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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

/// A verified source span whose task lifecycle records have been inspected.
/// The range identity is the same immutable identity used by `session_ranges`;
/// a marker may cover a checkpoint prefix or an older accepted range.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionTaskIndexedRange {
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

/// One canonical Session task lifecycle observation. `event_index` is the
/// stable record index within the exact source span, not an index assigned by
/// the current quota projection.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionTaskEvent {
    pub root_identity: String,
    pub relative_path: String,
    pub file_device: u64,
    pub file_inode: u64,
    pub prefix_generation: u128,
    pub start_offset: u64,
    pub end_offset: u64,
    pub record_sha256: String,
    pub event_index: u64,
    pub timestamp: i64,
    pub running: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTaskEvidence {
    pub events: Vec<SessionTaskEvent>,
    pub indexed_ranges: Vec<SessionTaskIndexedRange>,
    pub all_ranges_indexed: bool,
}

type SessionTaskIndexedRangeKey = (String, String, u64, u64, u128, u64, u64, String);
type SessionTaskEventKey = (String, String, u64, u64, u128, u64, u64, String, u64);

/// Exact source evidence which was read but could not be attributed to a
/// trusted usage vector.  Pending rows are keyed by source lineage and start
/// offset so a later parser can re-evaluate the same bytes without moving the
/// source checkpoint or manufacturing usage.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionPendingRange {
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
    pub parser_version: String,
    pub reason: String,
    pub complete: bool,
}

/// A source-proven token delta retained independently of quota period
/// authority.  The range key makes the event idempotent across process
/// restarts while its timestamp lets a later accepted quota boundary
/// re-materialize the correct period without rereading already checkpointed
/// bytes.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SessionEvent {
    pub root_identity: String,
    pub relative_path: String,
    pub file_device: u64,
    pub file_inode: u64,
    pub prefix_generation: u128,
    pub range_start: u64,
    pub range_end: u64,
    pub record_sha256: String,
    pub event_index: u64,
    pub timestamp: i64,
    pub model: String,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_input_tokens: Option<u64>,
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

/// Cumulative Session-derived delta at one canonical minute. These values are
/// reconstructed from an exact, committed JSONL byte range; they are not a
/// point-in-time measurement and never replace an existing DB value.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionTimelineRecoveryPoint {
    pub timestamp: i64,
    pub offset_model_totals: Vec<SessionModelTotal>,
    pub offset_sol_dollars: f64,
    pub offset_terra_dollars: f64,
    pub offset_luna_dollars: f64,
}

/// One atomic catch-up repair for source bytes that were not reflected in the
/// minute history. Raw history stays unchanged; readers add the cumulative
/// offset only before the first corrected current anchor.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionTimelineRecovery {
    pub recovery_id: String,
    pub canonical_reset_at: i64,
    pub window_seconds: i64,
    pub source_data_generation: u64,
    pub projection_end_exclusive: i64,
    pub source_model_totals: Vec<SessionModelTotal>,
    pub ranges: Vec<SessionRange>,
    pub points: Vec<SessionTimelineRecoveryPoint>,
    pub final_offset_model_totals: Vec<SessionModelTotal>,
    pub final_offset_sol_dollars: f64,
    pub final_offset_terra_dollars: f64,
    pub final_offset_luna_dollars: f64,
}

/// One source-proven correction for a cumulative Session counter that was
/// reset while the authoritative quota period was still active.
///
/// The raw history rows remain immutable. `before_model_totals` retains the
/// complete boundary evidence, while `offset_model_totals` contains only the
/// models whose own counters prove a regression (or disappear as unknown).
/// `before_*_dollars` retains the complete legacy dollar evidence separately
/// from the selective dollar offsets. Only the offsets are projected over the
/// bounded raw suffix identified by the two exact keys. The recorder stores
/// the payload and corrected durable totals atomically under `recovery_id`.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionCumulativeRecoverySource {
    pub data_generation: u64,
    pub reset_at: i64,
    pub window_seconds: i64,
    pub observed_at: i64,
    pub remaining_percent: f64,
    pub model_totals: Vec<SessionModelTotal>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionCumulativeRecovery {
    pub recovery_id: String,
    pub canonical_reset_at: i64,
    pub window_seconds: i64,
    pub before_reset_at: i64,
    pub before_timestamp: i64,
    pub first_reset_at: i64,
    pub first_timestamp: i64,
    pub through_reset_at: i64,
    pub through_timestamp: i64,
    pub before_model_totals: Vec<SessionModelTotal>,
    pub offset_model_totals: Vec<SessionModelTotal>,
    pub first_model_totals: Vec<SessionModelTotal>,
    pub source_current_model_totals: Vec<SessionModelTotal>,
    pub before_sol_dollars: f64,
    pub before_terra_dollars: f64,
    pub before_luna_dollars: f64,
    pub offset_sol_dollars: f64,
    pub offset_terra_dollars: f64,
    pub offset_luna_dollars: f64,
    /// Exact mutable generation observed by the read-only planner. It is not
    /// part of the immutable recovery identity: the writer consumes it only
    /// as an optimistic transaction precondition. Stored/read-projection
    /// recoveries therefore carry `None` after the marker has committed.
    pub source_generation: Option<SessionCumulativeRecoverySource>,
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

pub struct SessionTaskEvidenceInput<'a> {
    pub events: &'a [SessionEvent],
    pub pending_ranges: &'a [SessionPendingRange],
    pub task_events: &'a [SessionTaskEvent],
    pub task_indexed_ranges: &'a [SessionTaskIndexedRange],
}

struct CollectionEvidence<'a> {
    cumulative_recovery: Option<&'a SessionCumulativeRecovery>,
    timeline_recovery: Option<&'a SessionTimelineRecovery>,
    pending_ranges: Option<&'a [SessionPendingRange]>,
    events: Option<&'a [SessionEvent]>,
    task_events: Option<&'a [SessionTaskEvent]>,
    task_indexed_ranges: Option<&'a [SessionTaskIndexedRange]>,
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

/// Opaque proof that `.bak.1` was created and read back for one exact account
/// partition before canonical history replacement. Only this module can
/// construct the proof; the migration rechecks its raw fingerprint while
/// holding the SQLite write transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedPartitionBackup {
    database: PathBuf,
    partition_id: String,
    raw_rows: usize,
    raw_fingerprint: String,
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
        if self.timestamp > self.reset_at {
            return Err(UsageStoreError::InvalidImport(
                "timestamp must not exceed reset_at".into(),
            ));
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

fn canonicalize_recorded_sessions_for_commit(
    sources: &[RecordedSessionSource],
) -> Result<Vec<RecordedSessionSource>> {
    let canonical = canonicalize_recorded_sessions(sources)?;
    let mut paths = BTreeSet::new();
    for source in &canonical {
        if !paths.insert((&source.root_identity, &source.relative_path)) {
            return Err(UsageStoreError::InvalidImport(
                "multiple recorded session fingerprints for one path".into(),
            ));
        }
    }
    Ok(canonical)
}

fn replace_recorded_session_markers(
    transaction: &rusqlite::Transaction<'_>,
    sources: &[RecordedSessionSource],
) -> Result<()> {
    let mut delete = transaction.prepare(
        "DELETE FROM recorded_sessions
         WHERE root_identity = ?1 AND relative_path = ?2",
    )?;
    let mut insert = transaction.prepare(
        "INSERT INTO recorded_sessions (
            root_identity, relative_path, file_bytes, modified_nanos,
            file_device, file_inode
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for source in sources {
        delete.execute(params![&source.root_identity, &source.relative_path])?;
        insert.execute(params![
            &source.root_identity,
            &source.relative_path,
            source.file_bytes as i64,
            source.modified_nanos.to_string(),
            source.file_device.to_string(),
            source.file_inode.to_string(),
        ])?;
    }
    Ok(())
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

fn validate_session_task_indexed_range(range: &SessionTaskIndexedRange) -> Result<()> {
    validate_session_key(&range.root_identity, &range.relative_path)?;
    if range.start_offset >= range.end_offset
        || range.end_offset > i64::MAX as u64
        || range.collector_epoch == 0
        || range.cycle_seq == 0
        || range.prefix_generation == 0
    {
        return Err(UsageStoreError::InvalidImport(
            "session task indexed range is invalid".into(),
        ));
    }
    validate_sha256(&range.record_sha256, "session task indexed range record")?;
    Ok(())
}

fn validate_session_task_event(event: &SessionTaskEvent) -> Result<()> {
    validate_session_key(&event.root_identity, &event.relative_path)?;
    if event.start_offset >= event.end_offset
        || event.end_offset > i64::MAX as u64
        || event.event_index > i64::MAX as u64
        || event.prefix_generation == 0
        || event.timestamp <= 0
    {
        return Err(UsageStoreError::InvalidImport(
            "session task event is invalid".into(),
        ));
    }
    validate_sha256(&event.record_sha256, "session task event record")?;
    Ok(())
}

fn validate_session_pending_range(range: &SessionPendingRange) -> Result<()> {
    validate_session_key(&range.root_identity, &range.relative_path)?;
    if range.start_offset > i64::MAX as u64
        || range.end_offset > i64::MAX as u64
        || range.end_offset < range.start_offset
        || range.collector_epoch == 0
        || range.cycle_seq == 0
        || range.prefix_generation == 0
        || range.parser_version.is_empty()
        || range.parser_version.len() > 128
        || range.parser_version.chars().any(char::is_control)
        || range.reason.is_empty()
        || range.reason.len() > 512
        || range.reason.chars().any(char::is_control)
    {
        return Err(UsageStoreError::InvalidImport(
            "session pending range is invalid".into(),
        ));
    }
    validate_sha256(&range.record_sha256, "session pending range record")?;
    Ok(())
}

fn validate_session_event(event: &SessionEvent) -> Result<()> {
    validate_session_key(&event.root_identity, &event.relative_path)?;
    if event.range_start >= event.range_end
        || event.range_start > i64::MAX as u64
        || event.range_end > i64::MAX as u64
        || event.event_index > i64::MAX as u64
        || event.prefix_generation == 0
        || event.timestamp <= 0
        || !valid_session_model(&event.model)
        || event.cached_input_tokens > event.input_tokens
        || event.cache_write_input_tokens.is_some_and(|writes| {
            event
                .cached_input_tokens
                .checked_add(writes)
                .is_none_or(|total| total > event.input_tokens)
        })
        || [
            event.total_tokens,
            event.input_tokens,
            event.cached_input_tokens,
            event.output_tokens,
        ]
        .into_iter()
        .any(|value| value > i64::MAX as u64)
        || event
            .cache_write_input_tokens
            .is_some_and(|value| value > i64::MAX as u64)
    {
        return Err(UsageStoreError::InvalidImport(
            "session event is invalid".into(),
        ));
    }
    validate_sha256(&event.record_sha256, "session event record")?;
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

fn model_total_dominates(left: &SessionModelTotal, right: &SessionModelTotal) -> bool {
    left.model == right.model
        && left.total_tokens >= right.total_tokens
        && left.input_tokens >= right.input_tokens
        && left.cached_input_tokens >= right.cached_input_tokens
        && left.output_tokens >= right.output_tokens
        && match (
            left.cache_write_input_tokens,
            right.cache_write_input_tokens,
        ) {
            (Some(left), Some(right)) => left >= right,
            (None, None) => true,
            _ => false,
        }
}

fn model_total_reset_is_proven(baseline: &SessionModelTotal, observed: &SessionModelTotal) -> bool {
    if baseline.model != observed.model {
        return false;
    }
    let known_components = [
        (baseline.total_tokens, observed.total_tokens),
        (baseline.input_tokens, observed.input_tokens),
        (baseline.cached_input_tokens, observed.cached_input_tokens),
        (baseline.output_tokens, observed.output_tokens),
    ];
    if known_components
        .into_iter()
        .any(|(before, after)| before > 0 && after >= before)
    {
        return false;
    }
    match (
        baseline.cache_write_input_tokens,
        observed.cache_write_input_tokens,
    ) {
        (Some(before), Some(after)) if before > 0 && after >= before => false,
        (Some(_), Some(_)) | (None, None) => true,
        _ => false,
    }
}

fn cumulative_model_totals_are_complete(totals: &[SessionModelTotal]) -> bool {
    totals
        .iter()
        .all(|total| total.cache_write_input_tokens.is_some())
}

fn model_totals_dominate(left: &[SessionModelTotal], right: &[SessionModelTotal]) -> bool {
    let left = left
        .iter()
        .map(|total| (total.model.as_str(), total))
        .collect::<BTreeMap<_, _>>();
    right.iter().all(|required| {
        left.get(required.model.as_str())
            .is_some_and(|candidate| model_total_dominates(candidate, required))
    })
}

/// Timeline offsets may lose knowledge of the optional cache-write component
/// when a newer Session producer omits it. Unknown is not a numeric rollback:
/// all required counters must still be monotonic, and optional knowledge may
/// only move from a measured value to unknown, never the reverse.
fn timeline_model_totals_dominate(left: &[SessionModelTotal], right: &[SessionModelTotal]) -> bool {
    let left = left
        .iter()
        .map(|total| (total.model.as_str(), total))
        .collect::<BTreeMap<_, _>>();
    right.iter().all(|required| {
        left.get(required.model.as_str()).is_some_and(|candidate| {
            candidate.total_tokens >= required.total_tokens
                && candidate.input_tokens >= required.input_tokens
                && candidate.cached_input_tokens >= required.cached_input_tokens
                && candidate.output_tokens >= required.output_tokens
                && match (
                    candidate.cache_write_input_tokens,
                    required.cache_write_input_tokens,
                ) {
                    (Some(candidate), Some(required)) => candidate >= required,
                    (None, _) => true,
                    (Some(_), None) => false,
                }
        })
    })
}

fn same_reset_group(left: i64, right: i64) -> bool {
    left > 0 && right > 0 && left.abs_diff(right) <= RESET_GROUP_TOLERANCE_SECONDS as u64
}

fn reset_window_started_between_observations(
    previous_reset_at: i64,
    previous_observed_at: i64,
    next_reset_at: i64,
    next_window_seconds: i64,
    observed_at: i64,
) -> bool {
    if next_reset_at <= previous_reset_at || observed_at <= previous_observed_at {
        return false;
    }
    let Some(next_start_at) = next_reset_at.checked_sub(next_window_seconds) else {
        return false;
    };
    let reset_advance = next_reset_at.saturating_sub(previous_reset_at);
    let observation_advance = observed_at.saturating_sub(previous_observed_at);
    let tracks_observation_clock =
        reset_advance.abs_diff(observation_advance) <= RESET_GROUP_TOLERANCE_SECONDS as u64;
    next_start_at > previous_observed_at
        && next_start_at <= observed_at
        && !tracks_observation_clock
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaTransition {
    Initial,
    SamePeriod,
    Boundary,
    Rejected,
}

/// Classify a raw quota observation before choosing its durable period key.
/// `reset_at` is a mutable provider observation, so a replacement timestamp is
/// never a boundary by itself. A boundary needs either a quota recovery at the
/// replacement window's observed start or a successor window whose start
/// matches the accepted deadline.
pub fn classify_quota_transition(
    previous_reset_at: Option<i64>,
    previous_window_seconds: i64,
    previous_observed_at: Option<i64>,
    previous_remaining_percent: Option<f64>,
    next_reset_at: i64,
    next_window_seconds: i64,
    next_remaining_percent: Option<f64>,
    observed_at: i64,
) -> QuotaTransition {
    let next_start_at = next_reset_at.checked_sub(next_window_seconds);
    let candidate_is_valid = observed_at > 0
        && next_reset_at > observed_at
        && next_window_seconds > 0
        && next_start_at.is_some()
        && next_remaining_percent
            .is_some_and(|value| value.is_finite() && (0.0..=100.0).contains(&value));
    if !candidate_is_valid {
        return QuotaTransition::Rejected;
    }
    let Some(previous_reset_at) = previous_reset_at else {
        return QuotaTransition::Initial;
    };
    let Some(previous_observed_at) = previous_observed_at else {
        return QuotaTransition::Rejected;
    };
    let Some(previous_remaining_percent) = previous_remaining_percent else {
        return QuotaTransition::Rejected;
    };
    if previous_reset_at <= 0
        || previous_window_seconds <= 0
        || previous_observed_at <= 0
        || observed_at < previous_observed_at
        || !previous_remaining_percent.is_finite()
        || !(0.0..=100.0).contains(&previous_remaining_percent)
    {
        return QuotaTransition::Rejected;
    }
    let newly_started_window = reset_window_started_between_observations(
        previous_reset_at,
        previous_observed_at,
        next_reset_at,
        next_window_seconds,
        observed_at,
    );
    let successor_window_started_at_accepted_deadline = previous_reset_at <= observed_at
        && next_reset_at > previous_reset_at
        && next_start_at
            .is_some_and(|next_start_at| same_reset_group(previous_reset_at, next_start_at));
    let quota_recovered =
        next_remaining_percent.is_some_and(|next| next > previous_remaining_percent);
    if successor_window_started_at_accepted_deadline
        || (next_reset_at > previous_reset_at && newly_started_window && quota_recovered)
    {
        return QuotaTransition::Boundary;
    }
    if next_window_seconds == previous_window_seconds
        && (same_reset_group(previous_reset_at, next_reset_at)
            || (next_reset_at > previous_reset_at
                && next_remaining_percent.is_some_and(|next| next <= previous_remaining_percent)))
    {
        return QuotaTransition::SamePeriod;
    }
    QuotaTransition::Rejected
}

/// Selects the newest retained state only when all usable retained states
/// agree on one still-live period and the current state is its valid but
/// premature replacement. This is the shared upgrade authority used before
/// Session replay.
pub fn select_predeadline_quota_authority(
    current: &SessionCollectionState,
    retained: &[SessionCollectionState],
) -> Option<SessionCollectionState> {
    let current_observation = current.last_quota_observation.as_ref()?;
    if current.data_generation == 0
        || classify_quota_transition(
            None,
            0,
            None,
            None,
            current.reset_at,
            current.window_seconds,
            Some(current_observation.remaining_percent),
            current_observation.observed_at,
        ) != QuotaTransition::Initial
    {
        return None;
    }
    let mut candidates = retained
        .iter()
        .filter(|candidate| {
            let Some(observation) = candidate.last_quota_observation.as_ref() else {
                return false;
            };
            candidate.data_generation > 0
                && candidate.data_generation < current.data_generation
                && observation.observed_at <= current_observation.observed_at
                && candidate.reset_at > current_observation.observed_at
                && classify_quota_transition(
                    None,
                    0,
                    None,
                    None,
                    candidate.reset_at,
                    candidate.window_seconds,
                    Some(observation.remaining_percent),
                    observation.observed_at,
                ) == QuotaTransition::Initial
                && matches!(
                    classify_quota_transition(
                        Some(candidate.reset_at),
                        candidate.window_seconds,
                        Some(observation.observed_at),
                        Some(observation.remaining_percent),
                        current.reset_at,
                        current.window_seconds,
                        Some(current_observation.remaining_percent),
                        current_observation.observed_at,
                    ),
                    QuotaTransition::SamePeriod | QuotaTransition::Rejected
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.data_generation));
    let selected = candidates.first()?.clone();
    if candidates.iter().any(|candidate| {
        candidate.window_seconds != selected.window_seconds
            || !same_reset_group(candidate.reset_at, selected.reset_at)
    }) {
        return None;
    }
    Some(selected)
}

fn checked_add_model_totals(
    left: &[SessionModelTotal],
    right: &[SessionModelTotal],
) -> Option<Vec<SessionModelTotal>> {
    let mut combined = left
        .iter()
        .cloned()
        .map(|total| (total.model.clone(), total))
        .collect::<BTreeMap<_, _>>();
    for offset in right {
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
        total.total_tokens = total.total_tokens.checked_add(offset.total_tokens)?;
        total.input_tokens = total.input_tokens.checked_add(offset.input_tokens)?;
        total.cached_input_tokens = total
            .cached_input_tokens
            .checked_add(offset.cached_input_tokens)?;
        total.output_tokens = total.output_tokens.checked_add(offset.output_tokens)?;
        total.cache_write_input_tokens = match (
            total.cache_write_input_tokens,
            offset.cache_write_input_tokens,
        ) {
            (Some(left), Some(right)) => Some(left.checked_add(right)?),
            _ => None,
        };
    }
    canonicalize_model_totals(&combined.into_values().collect::<Vec<_>>()).ok()
}

/// Reconciles the last raw vector of a retained canonical period with the
/// current vector stored under a valid-but-rejected period alias. Each model
/// has exactly one admissible interpretation: monotonic continuation is an
/// absolute fact, a reset of every known component adds the retained
/// baseline, a missing model carries its known baseline, and a new model is
/// accepted as observed. Mixed component motion remains ambiguous.
pub fn reconcile_rejected_generation_model_totals(
    canonical: &[SessionModelTotal],
    rejected: &[SessionModelTotal],
) -> Result<Option<Vec<SessionModelTotal>>> {
    let canonical = canonicalize_model_totals(canonical)?;
    let rejected = canonicalize_model_totals(rejected)?;
    if !cumulative_model_totals_are_complete(&canonical)
        || !cumulative_model_totals_are_complete(&rejected)
    {
        return Ok(None);
    }
    let canonical_by_model = canonical
        .iter()
        .map(|total| (total.model.as_str(), total))
        .collect::<BTreeMap<_, _>>();
    let rejected_by_model = rejected
        .iter()
        .map(|total| (total.model.as_str(), total))
        .collect::<BTreeMap<_, _>>();
    let models = canonical_by_model
        .keys()
        .chain(rejected_by_model.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut reconciled = Vec::with_capacity(models.len());
    for model in models {
        match (
            canonical_by_model.get(model).copied(),
            rejected_by_model.get(model).copied(),
        ) {
            (Some(baseline), Some(current)) if model_total_dominates(current, baseline) => {
                reconciled.push(current.clone());
            }
            (Some(baseline), Some(current)) if model_total_reset_is_proven(baseline, current) => {
                let Some(mut combined) = checked_add_model_totals(
                    std::slice::from_ref(baseline),
                    std::slice::from_ref(current),
                ) else {
                    return Ok(None);
                };
                reconciled.push(combined.remove(0));
            }
            (Some(baseline), None) => reconciled.push(baseline.clone()),
            (None, Some(current)) => reconciled.push(current.clone()),
            (Some(_), Some(_)) => return Ok(None),
            (None, None) => unreachable!("model comes from one of the two canonical maps"),
        }
    }
    Ok(Some(canonicalize_model_totals(&reconciled)?))
}

type CumulativeModelPayload = (String, u64, u64, u64, u64, Option<u64>);
type CumulativeDollarPayload = (f64, f64, f64);
type TimelineRangePayload = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);
type TimelinePointPayload = (i64, Vec<CumulativeModelPayload>, CumulativeDollarPayload);
type TimelineRecoveryPayload = (
    String,
    i64,
    i64,
    u64,
    i64,
    Vec<CumulativeModelPayload>,
    Vec<TimelineRangePayload>,
    Vec<TimelinePointPayload>,
    Vec<CumulativeModelPayload>,
    CumulativeDollarPayload,
);
type CumulativeRecoveryPayload = (
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    Vec<CumulativeModelPayload>,
    Vec<CumulativeModelPayload>,
    Vec<CumulativeModelPayload>,
    Vec<CumulativeModelPayload>,
    CumulativeDollarPayload,
    CumulativeDollarPayload,
);

const MAX_SESSION_TIMELINE_RECOVERY_BYTES: usize = 64 * 1024 * 1024;

fn model_payload_rows(totals: &[SessionModelTotal]) -> Vec<CumulativeModelPayload> {
    totals
        .iter()
        .map(|total| {
            (
                total.model.clone(),
                total.total_tokens,
                total.input_tokens,
                total.cached_input_tokens,
                total.output_tokens,
                total.cache_write_input_tokens,
            )
        })
        .collect()
}

fn model_totals_from_payload(values: Vec<CumulativeModelPayload>) -> Vec<SessionModelTotal> {
    values
        .into_iter()
        .map(
            |(
                model,
                total_tokens,
                input_tokens,
                cached_input_tokens,
                output_tokens,
                cache_write_input_tokens,
            )| SessionModelTotal {
                model,
                total_tokens,
                input_tokens,
                cached_input_tokens,
                output_tokens,
                cache_write_input_tokens,
            },
        )
        .collect()
}

fn timeline_recovery_payload(
    partition_id: &str,
    recovery: &SessionTimelineRecovery,
) -> Result<String> {
    let ranges = recovery
        .ranges
        .iter()
        .map(|range| {
            (
                range.root_identity.clone(),
                range.relative_path.clone(),
                range.file_device.to_string(),
                range.file_inode.to_string(),
                range.start_offset.to_string(),
                range.end_offset.to_string(),
                format!("{:032x}", range.collector_epoch),
                range.cycle_seq.to_string(),
                format!("{:032x}", range.prefix_generation),
                range.record_sha256.clone(),
            )
        })
        .collect::<Vec<_>>();
    let points = recovery
        .points
        .iter()
        .map(|point| {
            (
                point.timestamp,
                model_payload_rows(&point.offset_model_totals),
                (
                    point.offset_sol_dollars,
                    point.offset_terra_dollars,
                    point.offset_luna_dollars,
                ),
            )
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&(
        partition_id,
        recovery.canonical_reset_at,
        recovery.window_seconds,
        recovery.source_data_generation,
        recovery.projection_end_exclusive,
        model_payload_rows(&recovery.source_model_totals),
        ranges,
        points,
        model_payload_rows(&recovery.final_offset_model_totals),
        (
            recovery.final_offset_sol_dollars,
            recovery.final_offset_terra_dollars,
            recovery.final_offset_luna_dollars,
        ),
    ))
    .map_err(|_| UsageStoreError::InvalidImport("timeline recovery is not serializable".into()))
}

fn validate_timeline_recovery_content(
    partition_id: &str,
    recovery: &SessionTimelineRecovery,
) -> Result<String> {
    if partition_id.len() != 64
        || !partition_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || recovery.canonical_reset_at <= 0
        || recovery.window_seconds <= 0
        || recovery.source_data_generation == 0
        || recovery.projection_end_exclusive <= 0
        || recovery.projection_end_exclusive > recovery.canonical_reset_at
        || recovery.ranges.is_empty()
        || recovery.points.is_empty()
    {
        return Err(UsageStoreError::InvalidImport(
            "timeline recovery authority is invalid".into(),
        ));
    }
    let period_start = recovery
        .canonical_reset_at
        .checked_sub(recovery.window_seconds)
        .ok_or_else(|| UsageStoreError::InvalidImport("timeline period underflow".into()))?;
    let source_model_totals = canonicalize_model_totals(&recovery.source_model_totals)?;
    let final_offset_model_totals = canonicalize_model_totals(&recovery.final_offset_model_totals)?;
    if source_model_totals != recovery.source_model_totals {
        return Err(UsageStoreError::InvalidImport(
            "timeline source totals are not canonical".into(),
        ));
    }
    if final_offset_model_totals != recovery.final_offset_model_totals
        || final_offset_model_totals.is_empty()
        || !final_offset_model_totals.iter().any(|total| {
            total.total_tokens > 0
                || total.input_tokens > 0
                || total.cached_input_tokens > 0
                || total.output_tokens > 0
                || total
                    .cache_write_input_tokens
                    .is_some_and(|value| value > 0)
        })
        || [
            recovery.final_offset_sol_dollars,
            recovery.final_offset_terra_dollars,
            recovery.final_offset_luna_dollars,
        ]
        .into_iter()
        .any(|value| !value.is_finite() || value < 0.0)
    {
        return Err(UsageStoreError::InvalidImport(
            "timeline final offset is invalid".into(),
        ));
    }
    let mut canonical_ranges = recovery.ranges.clone();
    canonical_ranges.sort();
    canonical_ranges.dedup();
    if canonical_ranges != recovery.ranges {
        return Err(UsageStoreError::InvalidImport(
            "timeline source ranges are not canonical".into(),
        ));
    }
    let owner = (
        recovery.ranges[0].collector_epoch,
        recovery.ranges[0].cycle_seq,
    );
    for (index, range) in recovery.ranges.iter().enumerate() {
        validate_session_range(range)?;
        if (range.collector_epoch, range.cycle_seq) != owner {
            return Err(UsageStoreError::InvalidImport(
                "timeline source ranges span collector generations".into(),
            ));
        }
        if recovery.ranges[..index].iter().any(|previous| {
            previous.root_identity == range.root_identity
                && previous.relative_path == range.relative_path
                && previous.file_device == range.file_device
                && previous.file_inode == range.file_inode
                && previous.prefix_generation == range.prefix_generation
                && previous.start_offset < range.end_offset
                && previous.end_offset > range.start_offset
        }) {
            return Err(UsageStoreError::InvalidImport(
                "timeline source ranges overlap".into(),
            ));
        }
    }
    let maximum_points = recovery
        .window_seconds
        .div_euclid(60)
        .checked_add(2)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(UsageStoreError::GenerationOverflow)?;
    if recovery.points.len() > maximum_points {
        return Err(UsageStoreError::InvalidImport(
            "timeline recovery has more than one point per minute".into(),
        ));
    }
    let mut previous_timestamp = None;
    let mut previous_totals: Option<Vec<SessionModelTotal>> = None;
    let mut previous_dollars = (0.0, 0.0, 0.0);
    for point in &recovery.points {
        let totals = canonicalize_model_totals(&point.offset_model_totals)?;
        let dollars = (
            point.offset_sol_dollars,
            point.offset_terra_dollars,
            point.offset_luna_dollars,
        );
        if point.timestamp.rem_euclid(60) != 0
            || point.timestamp < period_start.div_euclid(60) * 60
            || point.timestamp >= recovery.projection_end_exclusive
            || previous_timestamp.is_some_and(|previous| point.timestamp <= previous)
        {
            return Err(UsageStoreError::InvalidImport(
                "timeline recovery point timestamp is invalid".into(),
            ));
        }
        if totals.is_empty()
            || totals != point.offset_model_totals
            || !totals.iter().any(|total| {
                total.total_tokens > 0
                    || total.input_tokens > 0
                    || total.cached_input_tokens > 0
                    || total.output_tokens > 0
                    || total
                        .cache_write_input_tokens
                        .is_some_and(|value| value > 0)
            })
        {
            return Err(UsageStoreError::InvalidImport(
                "timeline recovery point totals are invalid".into(),
            ));
        }
        if previous_totals
            .as_ref()
            .is_some_and(|previous| !timeline_model_totals_dominate(&totals, previous))
        {
            return Err(UsageStoreError::InvalidImport(
                "timeline recovery point totals moved backwards".into(),
            ));
        }
        if [dollars.0, dollars.1, dollars.2]
            .into_iter()
            .any(|value| !value.is_finite() || value < 0.0)
            || dollars.0 < previous_dollars.0
            || dollars.1 < previous_dollars.1
            || dollars.2 < previous_dollars.2
        {
            return Err(UsageStoreError::InvalidImport(
                "timeline recovery point dollars are invalid".into(),
            ));
        }
        previous_timestamp = Some(point.timestamp);
        previous_totals = Some(totals);
        previous_dollars = dollars;
    }
    if checked_add_model_totals(&source_model_totals, &final_offset_model_totals).is_none() {
        return Err(UsageStoreError::InvalidImport(
            "timeline recovery source and offset totals are inconsistent".into(),
        ));
    }
    if !timeline_model_totals_dominate(
        &final_offset_model_totals,
        &recovery
            .points
            .last()
            .expect("non-empty timeline recovery")
            .offset_model_totals,
    ) {
        return Err(UsageStoreError::InvalidImport(
            "timeline recovery final offset moved backwards".into(),
        ));
    }
    if recovery.final_offset_sol_dollars < previous_dollars.0
        || recovery.final_offset_terra_dollars < previous_dollars.1
        || recovery.final_offset_luna_dollars < previous_dollars.2
    {
        return Err(UsageStoreError::InvalidImport(
            "timeline recovery final dollars moved backwards".into(),
        ));
    }
    let payload = timeline_recovery_payload(partition_id, recovery)?;
    if payload.len() > MAX_SESSION_TIMELINE_RECOVERY_BYTES {
        return Err(UsageStoreError::InvalidImport(
            "timeline recovery payload exceeds one collector cycle".into(),
        ));
    }
    Ok(payload)
}

fn validate_timeline_recovery(
    partition_id: &str,
    recovery: &SessionTimelineRecovery,
) -> Result<String> {
    let payload = validate_timeline_recovery_content(partition_id, recovery)?;
    let expected = format!("{:x}", Sha256::digest(payload.as_bytes()));
    if recovery.recovery_id != expected {
        return Err(UsageStoreError::InvalidImport(
            "timeline recovery identity mismatch".into(),
        ));
    }
    Ok(payload)
}

pub fn finalize_session_timeline_recovery(
    partition_id: &str,
    mut recovery: SessionTimelineRecovery,
) -> Result<SessionTimelineRecovery> {
    if !recovery.recovery_id.is_empty() {
        return Err(UsageStoreError::InvalidImport(
            "unfinalized timeline recovery already has an identity".into(),
        ));
    }
    let payload = timeline_recovery_payload(partition_id, &recovery)?;
    recovery.recovery_id = format!("{:x}", Sha256::digest(payload.as_bytes()));
    validate_timeline_recovery(partition_id, &recovery)?;
    Ok(recovery)
}

fn cumulative_recovery_payload(
    partition_id: &str,
    recovery: &SessionCumulativeRecovery,
) -> Result<String> {
    let rows = |totals: &[SessionModelTotal]| -> Vec<CumulativeModelPayload> {
        totals
            .iter()
            .map(|total| {
                (
                    total.model.clone(),
                    total.total_tokens,
                    total.input_tokens,
                    total.cached_input_tokens,
                    total.output_tokens,
                    total.cache_write_input_tokens,
                )
            })
            .collect::<Vec<_>>()
    };
    serde_json::to_string(&(
        partition_id,
        recovery.canonical_reset_at,
        recovery.window_seconds,
        recovery.before_reset_at,
        recovery.before_timestamp,
        recovery.first_reset_at,
        recovery.first_timestamp,
        recovery.through_reset_at,
        recovery.through_timestamp,
        rows(&recovery.before_model_totals),
        rows(&recovery.offset_model_totals),
        rows(&recovery.first_model_totals),
        rows(&recovery.source_current_model_totals),
        (
            recovery.before_sol_dollars,
            recovery.before_terra_dollars,
            recovery.before_luna_dollars,
        ),
        (
            recovery.offset_sol_dollars,
            recovery.offset_terra_dollars,
            recovery.offset_luna_dollars,
        ),
    ))
    .map_err(|_| UsageStoreError::InvalidImport("cumulative recovery is not serializable".into()))
}

fn validate_cumulative_recovery(
    partition_id: &str,
    recovery: &SessionCumulativeRecovery,
) -> Result<String> {
    if partition_id.len() != 64
        || !partition_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || recovery.canonical_reset_at <= 0
        || recovery.window_seconds <= 0
        || recovery.before_reset_at <= 0
        || recovery.before_timestamp <= 0
        || recovery.first_reset_at <= 0
        || recovery.first_timestamp <= recovery.before_timestamp
        || recovery.through_reset_at <= 0
        || recovery.through_timestamp < recovery.first_timestamp
        || same_reset_group(recovery.before_reset_at, recovery.first_reset_at)
        || !same_reset_group(recovery.through_reset_at, recovery.canonical_reset_at)
        || !same_reset_group(recovery.first_reset_at, recovery.canonical_reset_at)
        || [
            recovery.before_sol_dollars,
            recovery.before_terra_dollars,
            recovery.before_luna_dollars,
            recovery.offset_sol_dollars,
            recovery.offset_terra_dollars,
            recovery.offset_luna_dollars,
        ]
        .into_iter()
        .any(|value| !value.is_finite() || value < 0.0)
    {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery authority is invalid".into(),
        ));
    }
    let before = canonicalize_model_totals(&recovery.before_model_totals)?;
    let offset = canonicalize_model_totals(&recovery.offset_model_totals)?;
    let first = canonicalize_model_totals(&recovery.first_model_totals)?;
    let current = canonicalize_model_totals(&recovery.source_current_model_totals)?;
    let before_by_model = before
        .iter()
        .map(|total| (total.model.as_str(), total))
        .collect::<BTreeMap<_, _>>();
    let offset_by_model = offset
        .iter()
        .map(|total| total.model.as_str())
        .collect::<BTreeSet<_>>();
    let expected_offset_dollars = [
        (
            recovery.before_sol_dollars,
            recovery.offset_sol_dollars,
            "SOL",
        ),
        (
            recovery.before_terra_dollars,
            recovery.offset_terra_dollars,
            "TERRA",
        ),
        (
            recovery.before_luna_dollars,
            recovery.offset_luna_dollars,
            "LUNA",
        ),
    ];
    if before != recovery.before_model_totals
        || offset != recovery.offset_model_totals
        || first != recovery.first_model_totals
        || current != recovery.source_current_model_totals
        || before.is_empty()
        || offset.is_empty()
        || first.is_empty()
        || current.is_empty()
        || !cumulative_model_totals_are_complete(&before)
        || !cumulative_model_totals_are_complete(&offset)
        || !cumulative_model_totals_are_complete(&first)
        || !cumulative_model_totals_are_complete(&current)
        || !offset.iter().any(|total| {
            total.total_tokens > 0
                || total.input_tokens > 0
                || total.cached_input_tokens > 0
                || total.output_tokens > 0
                || total
                    .cache_write_input_tokens
                    .is_some_and(|value| value > 0)
        })
        || offset
            .iter()
            .any(|total| before_by_model.get(total.model.as_str()).copied() != Some(total))
        || expected_offset_dollars
            .into_iter()
            .any(|(before, offset, model)| {
                offset
                    != if offset_by_model.contains(model) {
                        before
                    } else {
                        0.0
                    }
            })
        || model_totals_dominate(&first, &offset)
        || !model_totals_dominate(&current, &first)
        || checked_add_model_totals(&current, &offset).is_none()
    {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery model vector is invalid".into(),
        ));
    }
    let payload = cumulative_recovery_payload(partition_id, recovery)?;
    if payload.len() > MAX_SNAPSHOT_JSON_BYTES {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery payload is too large".into(),
        ));
    }
    let expected = format!("{:x}", Sha256::digest(payload.as_bytes()));
    if recovery.recovery_id != expected {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery identity mismatch".into(),
        ));
    }
    Ok(payload)
}

fn cumulative_recovery_point_from_storage(
    transaction: &rusqlite::Transaction<'_>,
    reset_at: i64,
    timestamp: i64,
) -> Result<Option<CumulativeRecoveryPoint>> {
    let dollars: Option<(f64, f64, f64)> = transaction
        .query_row(
            "SELECT sol_dollars, terra_dollars, luna_dollars
             FROM usage_history WHERE reset_at=?1 AND timestamp=?2",
            params![reset_at, timestamp],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((sol_dollars, terra_dollars, luna_dollars)) = dollars else {
        return Ok(None);
    };
    let mut statement = transaction.prepare(
        "SELECT model, total_tokens, input_tokens, cached_input_tokens,
                output_tokens, cache_write_input_tokens
         FROM usage_model_history
         WHERE reset_at=?1 AND timestamp=?2 ORDER BY model",
    )?;
    let rows = statement.query_map(params![reset_at, timestamp], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut model_totals = Vec::new();
    for row in rows {
        let (model, total, input, cached, output, cache_write) = row?;
        model_totals.push(SessionModelTotal {
            model,
            total_tokens: canonical_u64_text(&total, "recovery evidence total")?,
            input_tokens: canonical_u64_text(&input, "recovery evidence input")?,
            cached_input_tokens: canonical_u64_text(&cached, "recovery evidence cached input")?,
            output_tokens: canonical_u64_text(&output, "recovery evidence output")?,
            cache_write_input_tokens: cache_write
                .as_deref()
                .map(|value| canonical_u64_text(value, "recovery evidence cache write"))
                .transpose()?,
        });
    }
    let model_totals = canonicalize_model_totals(&model_totals)?;
    if model_totals.is_empty()
        || !cumulative_model_totals_are_complete(&model_totals)
        || [sol_dollars, terra_dollars, luna_dollars]
            .into_iter()
            .any(|value| !value.is_finite() || value < 0.0)
    {
        return Ok(None);
    }
    Ok(Some(CumulativeRecoveryPoint {
        timestamp,
        reset_at,
        sol_dollars,
        terra_dollars,
        luna_dollars,
        model_totals,
    }))
}

fn validate_cumulative_recovery_storage_evidence(
    transaction: &rusqlite::Transaction<'_>,
    recovery: &SessionCumulativeRecovery,
) -> Result<()> {
    let before = cumulative_recovery_point_from_storage(
        transaction,
        recovery.before_reset_at,
        recovery.before_timestamp,
    )?;
    let first = cumulative_recovery_point_from_storage(
        transaction,
        recovery.first_reset_at,
        recovery.first_timestamp,
    )?;
    let through = cumulative_recovery_point_from_storage(
        transaction,
        recovery.through_reset_at,
        recovery.through_timestamp,
    )?;
    let evidence_matches = before.as_ref().is_some_and(|point| {
        point.model_totals == recovery.before_model_totals
            && point.sol_dollars == recovery.before_sol_dollars
            && point.terra_dollars == recovery.before_terra_dollars
            && point.luna_dollars == recovery.before_luna_dollars
    }) && first
        .as_ref()
        .is_some_and(|point| point.model_totals == recovery.first_model_totals)
        && through
            .as_ref()
            .is_some_and(|point| point.model_totals == recovery.source_current_model_totals);
    if !evidence_matches {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery raw evidence changed".into(),
        ));
    }
    for timestamp in [
        recovery.before_timestamp,
        recovery.first_timestamp,
        recovery.through_timestamp,
    ] {
        let count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM usage_history WHERE timestamp=?1",
            [timestamp],
            |row| row.get(0),
        )?;
        if count != 1 {
            return Err(UsageStoreError::InvalidImport(
                "cumulative recovery boundary timestamp is ambiguous".into(),
            ));
        }
    }
    Ok(())
}

fn validate_cumulative_recovery_source_generation(
    transaction: &rusqlite::Transaction<'_>,
    recovery: &SessionCumulativeRecovery,
    source: &SessionCumulativeRecoverySource,
) -> Result<()> {
    let source_models = canonicalize_model_totals(&source.model_totals)?;
    if source.data_generation == 0
        || source.reset_at <= 0
        || source.window_seconds <= 0
        || source.observed_at <= 0
        || source.reset_at <= source.observed_at
        || !source.remaining_percent.is_finite()
        || !(0.0..=100.0).contains(&source.remaining_percent)
        || source_models != source.model_totals
        || !cumulative_model_totals_are_complete(&source_models)
    {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery source generation is invalid".into(),
        ));
    }
    let current: (String, i64, i64) = transaction.query_row(
        "SELECT data_generation, reset_at, window_seconds
         FROM collection_generation WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let current_generation = canonical_u64_text(&current.0, "collection generation")?;
    let current_observation = last_quota_observation_for_reset(transaction, current.1)?;
    if current_generation != source.data_generation
        || current.1 != source.reset_at
        || current.2 != source.window_seconds
        || current_observation
            != Some(SessionQuotaObservation {
                observed_at: source.observed_at,
                remaining_percent: source.remaining_percent,
            })
        || session_model_totals_from_transaction(transaction)? != source_models
    {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery source generation changed".into(),
        ));
    }

    let canonical_observation =
        last_quota_observation_for_reset(transaction, recovery.canonical_reset_at)?.ok_or_else(
            || {
                UsageStoreError::InvalidImport(
                    "cumulative recovery canonical observation is missing".into(),
                )
            },
        )?;
    if canonical_observation.observed_at > source.observed_at
        || recovery.canonical_reset_at <= canonical_observation.observed_at
    {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery quota authority moved backwards".into(),
        ));
    }
    let transition = classify_quota_transition(
        Some(recovery.canonical_reset_at),
        recovery.window_seconds,
        Some(canonical_observation.observed_at),
        Some(canonical_observation.remaining_percent),
        source.reset_at,
        source.window_seconds,
        Some(source.remaining_percent),
        source.observed_at,
    );
    match transition {
        QuotaTransition::SamePeriod => {
            if source_models != recovery.source_current_model_totals {
                return Err(UsageStoreError::InvalidImport(
                    "cumulative recovery source vector changed".into(),
                ));
            }
        }
        QuotaTransition::Rejected
            if recovery.canonical_reset_at > source.observed_at
                && (source.reset_at != recovery.canonical_reset_at
                    || source.window_seconds != recovery.window_seconds) => {}
        QuotaTransition::Initial | QuotaTransition::Boundary | QuotaTransition::Rejected => {
            return Err(UsageStoreError::InvalidImport(
                "cumulative recovery source period is not recoverable".into(),
            ));
        }
    }
    Ok(())
}

fn session_model_totals_from_transaction(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Vec<SessionModelTotal>> {
    let mut statement = transaction.prepare(
        "SELECT model, total_tokens, input_tokens, cached_input_tokens,
                output_tokens, cache_write_input_tokens
         FROM session_model_totals ORDER BY model",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut totals = Vec::new();
    for row in rows {
        let (model, total, input, cached, output, cache_write) = row?;
        totals.push(SessionModelTotal {
            model,
            total_tokens: canonical_u64_text(&total, "current recovery total")?,
            input_tokens: canonical_u64_text(&input, "current recovery input")?,
            cached_input_tokens: canonical_u64_text(&cached, "current recovery cached input")?,
            output_tokens: canonical_u64_text(&output, "current recovery output")?,
            cache_write_input_tokens: cache_write
                .as_deref()
                .map(|value| canonical_u64_text(value, "current recovery cache write"))
                .transpose()?,
        });
    }
    canonicalize_model_totals(&totals)
}

fn last_quota_observation_for_reset(
    connection: &Connection,
    reset_at: i64,
) -> Result<Option<SessionQuotaObservation>> {
    if reset_at <= 0 {
        return Ok(None);
    }
    let tolerance = i64::try_from(RESET_GROUP_TOLERANCE_SECONDS)
        .map_err(|_| UsageStoreError::GenerationOverflow)?;
    let lower = reset_at.saturating_sub(tolerance).max(1);
    let upper = reset_at.saturating_add(tolerance);
    let observation = connection
        .query_row(
            "SELECT timestamp, remaining_percent
             FROM usage_history
             WHERE reset_at BETWEEN ?1 AND ?2
               AND remaining_percent IS NOT NULL
             ORDER BY timestamp DESC, reset_at DESC
             LIMIT 1",
            params![lower, upper],
            |row| {
                Ok(SessionQuotaObservation {
                    observed_at: row.get(0)?,
                    remaining_percent: row.get(1)?,
                })
            },
        )
        .optional()?;
    if observation.as_ref().is_some_and(|value| {
        value.observed_at <= 0
            || !value.remaining_percent.is_finite()
            || !(0.0..=100.0).contains(&value.remaining_percent)
    }) {
        return Err(UsageStoreError::InvalidImport(
            "last quota observation is invalid".into(),
        ));
    }
    Ok(observation)
}

fn cumulative_recovery_from_payload(
    recovery_id: &str,
    payload_json: &str,
) -> Result<(String, SessionCumulativeRecovery)> {
    if payload_json.is_empty() || payload_json.len() > MAX_SNAPSHOT_JSON_BYTES {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery payload length is invalid".into(),
        ));
    }
    let (
        partition_id,
        canonical_reset_at,
        window_seconds,
        before_reset_at,
        before_timestamp,
        first_reset_at,
        first_timestamp,
        through_reset_at,
        through_timestamp,
        before_rows,
        offset_rows,
        first_rows,
        current_rows,
        before_dollars,
        offset_dollars,
    ): CumulativeRecoveryPayload = serde_json::from_str(payload_json).map_err(|_| {
        UsageStoreError::InvalidImport("cumulative recovery payload is invalid".into())
    })?;
    let (before_sol_dollars, before_terra_dollars, before_luna_dollars) = before_dollars;
    let (offset_sol_dollars, offset_terra_dollars, offset_luna_dollars) = offset_dollars;
    let rows = |values: Vec<CumulativeModelPayload>| {
        values
            .into_iter()
            .map(
                |(
                    model,
                    total_tokens,
                    input_tokens,
                    cached_input_tokens,
                    output_tokens,
                    cache_write_input_tokens,
                )| SessionModelTotal {
                    model,
                    total_tokens,
                    input_tokens,
                    cached_input_tokens,
                    output_tokens,
                    cache_write_input_tokens,
                },
            )
            .collect::<Vec<_>>()
    };
    let recovery = SessionCumulativeRecovery {
        recovery_id: recovery_id.to_owned(),
        canonical_reset_at,
        window_seconds,
        before_reset_at,
        before_timestamp,
        first_reset_at,
        first_timestamp,
        through_reset_at,
        through_timestamp,
        before_model_totals: rows(before_rows),
        offset_model_totals: rows(offset_rows),
        first_model_totals: rows(first_rows),
        source_current_model_totals: rows(current_rows),
        before_sol_dollars,
        before_terra_dollars,
        before_luna_dollars,
        offset_sol_dollars,
        offset_terra_dollars,
        offset_luna_dollars,
        source_generation: None,
    };
    validate_cumulative_recovery(&partition_id, &recovery)?;
    if cumulative_recovery_payload(&partition_id, &recovery)? != payload_json {
        return Err(UsageStoreError::InvalidImport(
            "cumulative recovery payload is not canonical".into(),
        ));
    }
    Ok((partition_id, recovery))
}

fn timeline_recovery_from_payload(
    recovery_id: &str,
    payload_json: &str,
) -> Result<(String, SessionTimelineRecovery)> {
    if payload_json.is_empty() || payload_json.len() > MAX_SESSION_TIMELINE_RECOVERY_BYTES {
        return Err(UsageStoreError::InvalidImport(
            "timeline recovery payload length is invalid".into(),
        ));
    }
    let stored_identity = format!("{:x}", Sha256::digest(payload_json.as_bytes()));
    if recovery_id != stored_identity {
        return Err(UsageStoreError::InvalidImport(
            "timeline recovery stored identity mismatch".into(),
        ));
    }
    let (
        partition_id,
        canonical_reset_at,
        window_seconds,
        source_data_generation,
        projection_end_exclusive,
        source_rows,
        range_rows,
        point_rows,
        final_offset_rows,
        final_offset_dollars,
    ): TimelineRecoveryPayload = serde_json::from_str(payload_json).map_err(|_| {
        UsageStoreError::InvalidImport("timeline recovery payload is invalid".into())
    })?;
    let ranges = range_rows
        .into_iter()
        .map(
            |(
                root_identity,
                relative_path,
                file_device,
                file_inode,
                start_offset,
                end_offset,
                collector_epoch,
                cycle_seq,
                prefix_generation,
                record_sha256,
            )| {
                Ok(SessionRange {
                    root_identity,
                    relative_path,
                    file_device: canonical_u64_text(&file_device, "timeline file device")?,
                    file_inode: canonical_u64_text(&file_inode, "timeline file inode")?,
                    start_offset: canonical_u64_text(&start_offset, "timeline start offset")?,
                    end_offset: canonical_u64_text(&end_offset, "timeline end offset")?,
                    collector_epoch: canonical_u128_hex(
                        &collector_epoch,
                        "timeline collector epoch",
                    )?,
                    cycle_seq: canonical_u64_text(&cycle_seq, "timeline cycle sequence")?,
                    prefix_generation: canonical_u128_hex(
                        &prefix_generation,
                        "timeline prefix generation",
                    )?,
                    record_sha256,
                })
            },
        )
        .collect::<Result<Vec<_>>>()?;
    let points = point_rows
        .into_iter()
        .map(|(timestamp, rows, dollars)| {
            let (offset_sol_dollars, offset_terra_dollars, offset_luna_dollars) = dollars;
            SessionTimelineRecoveryPoint {
                timestamp,
                offset_model_totals: model_totals_from_payload(rows),
                offset_sol_dollars,
                offset_terra_dollars,
                offset_luna_dollars,
            }
        })
        .collect();
    let recovery = SessionTimelineRecovery {
        recovery_id: recovery_id.to_owned(),
        canonical_reset_at,
        window_seconds,
        source_data_generation,
        projection_end_exclusive,
        source_model_totals: model_totals_from_payload(source_rows),
        ranges,
        points,
        final_offset_model_totals: model_totals_from_payload(final_offset_rows),
        final_offset_sol_dollars: final_offset_dollars.0,
        final_offset_terra_dollars: final_offset_dollars.1,
        final_offset_luna_dollars: final_offset_dollars.2,
    };
    // The durable identity authenticates the exact stored bytes above.
    // Re-serializing parsed finite f64 values is not a stable byte-level
    // canonicalization contract across serde_json versions, so readback
    // validates the complete logical structure without replacing that exact
    // byte authority.
    validate_timeline_recovery_content(&partition_id, &recovery)?;
    Ok((partition_id, recovery))
}

fn timeline_recovery_point_for(
    recovery: &SessionTimelineRecovery,
    reset_at: i64,
    timestamp: i64,
) -> Option<&SessionTimelineRecoveryPoint> {
    if !same_reset_group(reset_at, recovery.canonical_reset_at)
        || timestamp >= recovery.projection_end_exclusive
    {
        return None;
    }
    let index = recovery
        .points
        .partition_point(|point| point.timestamp <= timestamp);
    index
        .checked_sub(1)
        .and_then(|index| recovery.points.get(index))
}

fn add_timeline_offsets_to_models(
    totals: &[SessionModelTotal],
    offsets: &[SessionModelTotal],
) -> Result<Vec<SessionModelTotal>> {
    checked_add_model_totals(totals, offsets).ok_or(UsageStoreError::GenerationOverflow)
}

fn apply_timeline_recoveries_to_observation<'a>(
    observation: &mut UsageHistoryObservation,
    recoveries: impl Iterator<Item = &'a SessionTimelineRecovery>,
) -> Result<()> {
    for recovery in recoveries {
        let Some(point) =
            timeline_recovery_point_for(recovery, observation.reset_at, observation.timestamp)
        else {
            continue;
        };
        for (value, offset) in [
            (&mut observation.sol_dollars, point.offset_sol_dollars),
            (&mut observation.terra_dollars, point.offset_terra_dollars),
            (&mut observation.luna_dollars, point.offset_luna_dollars),
        ] {
            if let Some(value) = value.as_mut() {
                *value += offset;
                if !value.is_finite() || *value < 0.0 {
                    return Err(UsageStoreError::InvalidImport(
                        "timeline recovery observation dollars overflowed".into(),
                    ));
                }
            }
        }
        for (value, model) in [
            (&mut observation.sol_tokens, "SOL"),
            (&mut observation.terra_tokens, "TERRA"),
            (&mut observation.luna_tokens, "LUNA"),
        ] {
            if let Some(value) = value.as_mut() {
                *value = value
                    .checked_add(
                        point
                            .offset_model_totals
                            .iter()
                            .find(|total| total.model == model)
                            .map(|total| total.total_tokens)
                            .unwrap_or(0),
                    )
                    .ok_or(UsageStoreError::GenerationOverflow)?;
                if *value > i64::MAX as u64 {
                    return Err(UsageStoreError::GenerationOverflow);
                }
            }
        }
        if let Some(totals) = observation.model_totals.as_ref() {
            observation.model_totals = Some(add_timeline_offsets_to_models(
                totals,
                &point.offset_model_totals,
            )?);
        }
        observation.model_source = ModelSource::ReconstructedFromSession;
    }
    observation.validate()
}

#[derive(Clone, Debug, PartialEq)]
struct CumulativeRecoveryPoint {
    timestamp: i64,
    reset_at: i64,
    sol_dollars: f64,
    terra_dollars: f64,
    luna_dollars: f64,
    model_totals: Vec<SessionModelTotal>,
}

/// Derives one bounded correction from immutable observations. The function
/// is deliberately total over ordinary ambiguity: conflicting timestamps,
/// malformed vectors, a non-monotonic suffix, or a current vector which does
/// not equal the latest raw endpoint produce no candidate rather than making
/// history or the recorder unavailable.
pub fn derive_session_cumulative_recovery(
    partition_id: &str,
    canonical_reset_at: i64,
    window_seconds: i64,
    now: i64,
    current_model_totals: &[SessionModelTotal],
    observations: &[UsageHistoryObservation],
) -> Result<Option<SessionCumulativeRecovery>> {
    if canonical_reset_at <= 0 || window_seconds <= 0 || now <= 0 || canonical_reset_at <= now {
        return Ok(None);
    }
    let current_model_totals = canonicalize_model_totals(current_model_totals)?;
    if current_model_totals.is_empty()
        || !cumulative_model_totals_are_complete(&current_model_totals)
    {
        return Ok(None);
    }
    let period_started_at = canonical_reset_at
        .checked_sub(window_seconds)
        .ok_or_else(|| UsageStoreError::InvalidImport("cumulative period underflow".into()))?;
    let period_start = period_started_at.div_euclid(60) * 60;
    let first_period_minute = if period_started_at.rem_euclid(60) == 0 {
        period_started_at
    } else {
        period_start.checked_add(60).ok_or_else(|| {
            UsageStoreError::InvalidImport("cumulative period minute overflow".into())
        })?
    };
    let canonical_period_start_was_observed = observations.iter().any(|observation| {
        observation.timestamp == first_period_minute
            && same_reset_group(observation.reset_at, canonical_reset_at)
            && observation
                .remaining_percent
                .is_some_and(|value| value.is_finite() && (0.0..=100.0).contains(&value))
    });
    let mut grouped = BTreeMap::<i64, Vec<CumulativeRecoveryPoint>>::new();
    for observation in observations {
        if observation.timestamp < period_start || observation.timestamp > now {
            continue;
        }
        let (Some(sol_dollars), Some(terra_dollars), Some(luna_dollars), Some(totals)) = (
            observation.sol_dollars,
            observation.terra_dollars,
            observation.luna_dollars,
            observation.model_totals.as_ref(),
        ) else {
            continue;
        };
        let totals = canonicalize_model_totals(totals)?;
        if totals.is_empty() || !cumulative_model_totals_are_complete(&totals) {
            return Ok(None);
        }
        if [sol_dollars, terra_dollars, luna_dollars]
            .into_iter()
            .any(|value| !value.is_finite() || value < 0.0)
        {
            continue;
        }
        grouped
            .entry(observation.timestamp)
            .or_default()
            .push(CumulativeRecoveryPoint {
                timestamp: observation.timestamp,
                reset_at: observation.reset_at,
                sol_dollars,
                terra_dollars,
                luna_dollars,
                model_totals: totals,
            });
    }
    let mut points = Vec::new();
    for candidates in grouped.into_values() {
        let first = &candidates[0];
        if candidates.iter().all(|candidate| candidate == first) {
            points.push(first.clone());
        }
    }
    let Some(endpoint_index) = points.iter().rposition(|point| {
        same_reset_group(point.reset_at, canonical_reset_at)
            && point.model_totals == current_model_totals
    }) else {
        return Ok(None);
    };
    if endpoint_index + 1 != points.len() {
        return Ok(None);
    }

    let candidate_index = (1..=endpoint_index).rev().find(|index| {
        let before = &points[index - 1];
        let after = &points[*index];
        !same_reset_group(before.reset_at, after.reset_at)
            && same_reset_group(after.reset_at, canonical_reset_at)
            && !model_totals_dominate(&after.model_totals, &before.model_totals)
    });
    let Some(candidate_index) = candidate_index else {
        return Ok(None);
    };
    let before = &points[candidate_index - 1];
    let first = &points[candidate_index];
    if canonical_period_start_was_observed {
        // The canonical cumulative stream was observed at its first
        // representable minute. A later stale reset followed by a return to
        // that stream is not a same-period counter regression and must never
        // inherit the stale period's totals.
        return Ok(None);
    }
    if reset_window_started_between_observations(
        before.reset_at,
        before.timestamp,
        first.reset_at,
        window_seconds,
        first.timestamp,
    ) {
        return Ok(None);
    }

    let mut known = BTreeMap::<String, SessionModelTotal>::new();
    for point in &points[candidate_index..=endpoint_index] {
        if !same_reset_group(point.reset_at, canonical_reset_at) {
            return Ok(None);
        }
        for total in &point.model_totals {
            if known
                .get(&total.model)
                .is_some_and(|previous| !model_total_dominates(total, previous))
            {
                return Ok(None);
            }
            known.insert(total.model.clone(), total.clone());
        }
    }
    let suffix_endpoint = canonicalize_model_totals(&known.into_values().collect::<Vec<_>>())?;
    if suffix_endpoint != current_model_totals {
        return Ok(None);
    }

    // A reset alias is only the boundary candidate. Recovery authority is
    // established independently for every model: a model which continues
    // monotonically across the alias is already an exact absolute fact and
    // must not receive the baseline a second time. A missing model remains
    // unknown, so its last known baseline is carried. Mixed component motion
    // is ambiguous and rejects this recovery instead of guessing.
    let mut offset_model_totals = Vec::new();
    for baseline in &before.model_totals {
        let first_after_boundary =
            points[candidate_index..=endpoint_index]
                .iter()
                .find_map(|point| {
                    point
                        .model_totals
                        .iter()
                        .find(|total| total.model == baseline.model)
                });
        match first_after_boundary {
            None => offset_model_totals.push(baseline.clone()),
            Some(observed) if model_total_dominates(observed, baseline) => {}
            Some(observed) if model_total_reset_is_proven(baseline, observed) => {
                offset_model_totals.push(baseline.clone());
            }
            Some(_) => return Ok(None),
        }
    }
    let offset_model_totals = canonicalize_model_totals(&offset_model_totals)?;
    if offset_model_totals.is_empty()
        || checked_add_model_totals(&current_model_totals, &offset_model_totals).is_none()
    {
        return Ok(None);
    }

    let offsets_model = |model: &str| offset_model_totals.iter().any(|total| total.model == model);
    let offset_sol_dollars = if offsets_model("SOL") {
        before.sol_dollars
    } else {
        0.0
    };
    let offset_terra_dollars = if offsets_model("TERRA") {
        before.terra_dollars
    } else {
        0.0
    };
    let offset_luna_dollars = if offsets_model("LUNA") {
        before.luna_dollars
    } else {
        0.0
    };

    let through = &points[endpoint_index];
    let mut recovery = SessionCumulativeRecovery {
        recovery_id: String::new(),
        canonical_reset_at,
        window_seconds,
        before_reset_at: before.reset_at,
        before_timestamp: before.timestamp,
        first_reset_at: first.reset_at,
        first_timestamp: first.timestamp,
        through_reset_at: through.reset_at,
        through_timestamp: through.timestamp,
        before_model_totals: before.model_totals.clone(),
        offset_model_totals,
        first_model_totals: first.model_totals.clone(),
        source_current_model_totals: current_model_totals,
        before_sol_dollars: before.sol_dollars,
        before_terra_dollars: before.terra_dollars,
        before_luna_dollars: before.luna_dollars,
        offset_sol_dollars,
        offset_terra_dollars,
        offset_luna_dollars,
        source_generation: None,
    };
    let payload = cumulative_recovery_payload(partition_id, &recovery)?;
    recovery.recovery_id = format!("{:x}", Sha256::digest(payload.as_bytes()));
    validate_cumulative_recovery(partition_id, &recovery)?;
    Ok(Some(recovery))
}

fn recovery_contains_raw_key(
    recovery: &SessionCumulativeRecovery,
    reset_at: i64,
    timestamp: i64,
) -> bool {
    same_reset_group(reset_at, recovery.canonical_reset_at)
        && timestamp >= recovery.first_timestamp
        && timestamp <= recovery.through_timestamp
}

fn apply_cumulative_recoveries_to_sample<'a>(
    sample: &mut UsageHistorySample,
    recoveries: impl Iterator<Item = &'a SessionCumulativeRecovery>,
) -> Result<()> {
    for recovery in recoveries {
        if !recovery_contains_raw_key(recovery, sample.reset_at, sample.timestamp) {
            continue;
        }
        sample.sol_dollars += recovery.offset_sol_dollars;
        sample.terra_dollars += recovery.offset_terra_dollars;
        sample.luna_dollars += recovery.offset_luna_dollars;
        sample.sol_tokens = sample
            .sol_tokens
            .checked_add(
                recovery
                    .offset_model_totals
                    .iter()
                    .find(|total| total.model == "SOL")
                    .map(|total| total.total_tokens)
                    .unwrap_or(0),
            )
            .ok_or(UsageStoreError::GenerationOverflow)?;
        sample.terra_tokens = sample
            .terra_tokens
            .checked_add(
                recovery
                    .offset_model_totals
                    .iter()
                    .find(|total| total.model == "TERRA")
                    .map(|total| total.total_tokens)
                    .unwrap_or(0),
            )
            .ok_or(UsageStoreError::GenerationOverflow)?;
        sample.luna_tokens = sample
            .luna_tokens
            .checked_add(
                recovery
                    .offset_model_totals
                    .iter()
                    .find(|total| total.model == "LUNA")
                    .map(|total| total.total_tokens)
                    .unwrap_or(0),
            )
            .ok_or(UsageStoreError::GenerationOverflow)?;
        if [
            sample.sol_dollars,
            sample.terra_dollars,
            sample.luna_dollars,
        ]
        .into_iter()
        .any(|value| !value.is_finite() || value < 0.0)
        {
            return Err(UsageStoreError::InvalidImport(
                "cumulative recovery dollar projection overflowed".into(),
            ));
        }
    }
    Ok(())
}

fn apply_cumulative_recoveries_to_observation<'a>(
    observation: &mut UsageHistoryObservation,
    recoveries: impl Iterator<Item = &'a SessionCumulativeRecovery>,
) -> Result<()> {
    for recovery in recoveries {
        if !recovery_contains_raw_key(recovery, observation.reset_at, observation.timestamp) {
            continue;
        }
        let Some(model_totals) = observation.model_totals.as_ref() else {
            continue;
        };
        let combined = checked_add_model_totals(model_totals, &recovery.offset_model_totals)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        observation.model_totals = Some(combined);
        for (value, offset) in [
            (&mut observation.sol_dollars, recovery.offset_sol_dollars),
            (
                &mut observation.terra_dollars,
                recovery.offset_terra_dollars,
            ),
            (&mut observation.luna_dollars, recovery.offset_luna_dollars),
        ] {
            if let Some(value) = value.as_mut() {
                *value += offset;
                if !value.is_finite() || *value < 0.0 {
                    return Err(UsageStoreError::InvalidImport(
                        "cumulative recovery observation dollars overflowed".into(),
                    ));
                }
            }
        }
        for (value, model) in [
            (&mut observation.sol_tokens, "SOL"),
            (&mut observation.terra_tokens, "TERRA"),
            (&mut observation.luna_tokens, "LUNA"),
        ] {
            if let Some(value) = value.as_mut() {
                *value = value
                    .checked_add(
                        recovery
                            .offset_model_totals
                            .iter()
                            .find(|total| total.model == model)
                            .map(|total| total.total_tokens)
                            .unwrap_or(0),
                    )
                    .ok_or(UsageStoreError::GenerationOverflow)?;
            }
        }
    }
    Ok(())
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

#[derive(Debug)]
struct CanonicalizedSamples {
    rows: Vec<UsageHistorySample>,
    source_to_canonical: BTreeMap<(i64, i64), (i64, i64)>,
}

fn canonicalize_samples_with_sources(
    transaction: &rusqlite::Transaction<'_>,
    samples: &[UsageHistorySample],
    preserve_existing: bool,
    preserve_existing_before: Option<i64>,
) -> Result<CanonicalizedSamples> {
    let canonical_storage = canonical_history_constraints_present(transaction)?;
    let incoming = if canonical_storage {
        let (current_reset_at, window_seconds): (i64, i64) = transaction.query_row(
            "SELECT reset_at, window_seconds FROM collection_generation WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let raw = samples
            .iter()
            .map(|sample| {
                sample.validate()?;
                Ok(CanonicalRawSample {
                    timestamp: sample.timestamp,
                    reset_at: sample.reset_at,
                    remaining_percent: sample.remaining_percent,
                    sol_dollars: sample.sol_dollars,
                    terra_dollars: sample.terra_dollars,
                    luna_dollars: sample.luna_dollars,
                    sol_tokens: sample.sol_tokens,
                    terra_tokens: sample.terra_tokens,
                    luna_tokens: sample.luna_tokens,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let canonical = canonicalize_history_for_storage_with_sources(
            &raw,
            (current_reset_at > 0).then_some(current_reset_at),
            window_seconds.max(0),
        )
        .map_err(|error| {
            UsageStoreError::InvalidImport(format!(
                "incoming history canonicalization failed: {error}"
            ))
        })?;
        canonical
            .into_iter()
            .map(|canonical| {
                let sample = canonical.sample;
                (
                    UsageHistorySample {
                        timestamp: sample.timestamp,
                        reset_at: sample.reset_at,
                        remaining_percent: sample.remaining_percent,
                        sol_dollars: sample.sol_dollars,
                        terra_dollars: sample.terra_dollars,
                        luna_dollars: sample.luna_dollars,
                        sol_tokens: sample.sol_tokens,
                        terra_tokens: sample.terra_tokens,
                        luna_tokens: sample.luna_tokens,
                    },
                    (canonical.source_reset_at, canonical.source_timestamp),
                )
            })
            .collect::<Vec<_>>()
    } else {
        let mut grouped = std::collections::BTreeMap::<(i64, i64), Vec<UsageHistorySample>>::new();
        for sample in samples {
            sample.validate()?;
            grouped
                .entry((sample.reset_at, sample.timestamp))
                .or_default()
                .push(sample.clone());
        }
        grouped
            .into_iter()
            .map(|(source_key, observations)| {
                canonicalize_sample_group(&observations).map(|sample| (sample, source_key))
            })
            .collect::<Result<Vec<_>>>()?
    };

    let query = if canonical_storage {
        "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
         FROM usage_history WHERE timestamp = ?1"
    } else {
        "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
         FROM usage_history WHERE reset_at = ?1 AND timestamp = ?2"
    };
    let mut existing_statement = transaction.prepare(query)?;
    let mut canonical = Vec::with_capacity(incoming.len());
    let mut source_to_canonical = BTreeMap::new();
    for (mut incoming, source_key) in incoming {
        let decode = |row: &rusqlite::Row<'_>| {
            let sample = valid_sample_from_row(row)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            sample.ok_or(rusqlite::Error::InvalidQuery)
        };
        let existing = if canonical_storage {
            existing_statement
                .query_row([incoming.timestamp], decode)
                .optional()?
        } else {
            existing_statement
                .query_row(params![incoming.reset_at, incoming.timestamp], decode)
                .optional()?
        };
        if let Some(existing) = existing.as_ref() {
            // The canonical timestamp already owns its one durable period
            // key. An incoming reset alias cannot create or rename that row.
            incoming.reset_at = existing.reset_at;
        }
        let selected = match existing {
            Some(existing)
                if preserve_existing
                    || preserve_existing_before
                        .is_some_and(|cutoff| incoming.timestamp < cutoff) =>
            {
                existing
            }
            Some(existing) => reconcile_existing_sample(existing, incoming)?,
            None => incoming,
        };
        source_to_canonical.insert(source_key, (selected.reset_at, selected.timestamp));
        canonical.push(selected);
    }

    if canonical_storage {
        let mut timestamps = BTreeSet::new();
        for sample in &canonical {
            sample.validate()?;
            if sample.timestamp.rem_euclid(60) != 0 || !timestamps.insert(sample.timestamp) {
                return Err(UsageStoreError::InvalidImport(
                    "incoming canonical timestamp is not globally unique".into(),
                ));
            }
        }
    }

    Ok(CanonicalizedSamples {
        rows: canonical,
        source_to_canonical,
    })
}

fn canonicalize_samples(
    transaction: &rusqlite::Transaction<'_>,
    samples: &[UsageHistorySample],
    preserve_existing: bool,
    preserve_existing_before: Option<i64>,
) -> Result<Vec<UsageHistorySample>> {
    canonicalize_samples_with_sources(
        transaction,
        samples,
        preserve_existing,
        preserve_existing_before,
    )
    .map(|canonical| canonical.rows)
}

fn upsert_canonical_samples(
    transaction: &rusqlite::Transaction<'_>,
    samples: &[UsageHistorySample],
    preserve_existing: bool,
    preserve_existing_before: Option<i64>,
) -> Result<()> {
    let mut insert_if_absent = transaction.prepare(INSERT_SAMPLE_IF_ABSENT)?;
    let mut upsert = transaction.prepare(UPSERT_SAMPLE)?;
    for sample in samples {
        let statement = if preserve_existing
            || preserve_existing_before.is_some_and(|cutoff| sample.timestamp < cutoff)
        {
            &mut insert_if_absent
        } else {
            &mut upsert
        };
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

#[allow(clippy::type_complexity)]
fn replace_session_pending_ranges(
    transaction: &rusqlite::Transaction<'_>,
    pending_ranges: &BTreeMap<(String, String, u64, u64, u128, u64), SessionPendingRange>,
    accepted_ranges: &BTreeMap<(String, String, u64, u64, u128, u64, u64, String), SessionRange>,
    replace_incomplete: bool,
) -> Result<()> {
    if replace_incomplete {
        // Incomplete rows describe only the latest full source inventory
        // (budget backlog, an open tail, or a transient read failure). A new
        // recorder cycle supplies that complete set, so retaining an omitted
        // prior-cycle row would manufacture a permanent degraded state.
        // Complete malformed-byte evidence remains until an accepted range
        // explicitly supersedes the same source offset below.
        transaction.execute("DELETE FROM session_pending_ranges WHERE complete=0", [])?;
    }
    for range in accepted_ranges.values() {
        let pending_same_start = pending_ranges.values().any(|pending| {
            pending.root_identity == range.root_identity
                && pending.relative_path == range.relative_path
                && pending.file_device == range.file_device
                && pending.file_inode == range.file_inode
                && pending.start_offset == range.start_offset
        });
        if pending_same_start {
            continue;
        }
        transaction.execute(
            "DELETE FROM session_pending_ranges
             WHERE root_identity=?1 AND relative_path=?2
               AND file_device=?3 AND file_inode=?4
               AND start_offset=?5",
            params![
                &range.root_identity,
                &range.relative_path,
                range.file_device.to_string(),
                range.file_inode.to_string(),
                range.start_offset as i64,
            ],
        )?;
    }
    for pending in pending_ranges.values() {
        let file_device = pending.file_device.to_string();
        let file_inode = pending.file_inode.to_string();
        let collector_epoch = format!("{:032x}", pending.collector_epoch);
        let cycle_seq = pending.cycle_seq.to_string();
        let prefix_generation = format!("{:032x}", pending.prefix_generation);
        // cycle_seq records when this evidence was observed, not its replay
        // identity. Retries naturally use a new cycle sequence.
        let existing: Option<(i64, String, String, String, String, i64)> = transaction
            .query_row(
                "SELECT end_offset, collector_epoch, record_sha256,
                        parser_version, reason, complete
                 FROM session_pending_ranges
                 WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3
                   AND file_inode=?4 AND prefix_generation=?5 AND start_offset=?6",
                params![
                    &pending.root_identity,
                    &pending.relative_path,
                    &file_device,
                    &file_inode,
                    &prefix_generation,
                    pending.start_offset as i64,
                ],
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
            )
            .optional()?;
        if let Some(existing) = existing {
            let expected = (
                pending.end_offset as i64,
                collector_epoch,
                pending.record_sha256.clone(),
                pending.parser_version.clone(),
                pending.reason.clone(),
                i64::from(pending.complete),
            );
            if existing != expected {
                return Err(UsageStoreError::InvalidImport(
                    "session pending range replay conflicts with its evidence".into(),
                ));
            }
            continue;
        }
        transaction.execute(
            "INSERT INTO session_pending_ranges (
                root_identity, relative_path, file_device, file_inode,
                start_offset, end_offset, collector_epoch, cycle_seq,
                prefix_generation, record_sha256, parser_version, reason, complete
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                &pending.root_identity,
                &pending.relative_path,
                &file_device,
                &file_inode,
                pending.start_offset as i64,
                pending.end_offset as i64,
                &collector_epoch,
                &cycle_seq,
                &prefix_generation,
                &pending.record_sha256,
                &pending.parser_version,
                &pending.reason,
                i64::from(pending.complete),
            ],
        )?;
    }
    Ok(())
}

#[allow(clippy::type_complexity)]
fn upsert_session_events(
    transaction: &rusqlite::Transaction<'_>,
    events: &BTreeMap<(String, String, u64, u64, u128, u64, u64, String, u64), SessionEvent>,
) -> Result<()> {
    let mut insert = transaction.prepare(
        "INSERT INTO session_events (
            root_identity, relative_path, file_device, file_inode,
            prefix_generation, range_start, range_end, record_sha256,
            event_index, timestamp, model, total_tokens, input_tokens,
            cached_input_tokens, output_tokens, cache_write_input_tokens
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT DO NOTHING",
    )?;
    for event in events.values() {
        insert.execute(params![
            &event.root_identity,
            &event.relative_path,
            event.file_device.to_string(),
            event.file_inode.to_string(),
            format!("{:032x}", event.prefix_generation),
            event.range_start as i64,
            event.range_end as i64,
            &event.record_sha256,
            event.event_index as i64,
            event.timestamp,
            &event.model,
            event.total_tokens.to_string(),
            event.input_tokens.to_string(),
            event.cached_input_tokens.to_string(),
            event.output_tokens.to_string(),
            event
                .cache_write_input_tokens
                .map(|value| value.to_string()),
        ])?;
    }
    drop(insert);
    for event in events.values() {
        let existing: Option<SessionEvent> = transaction
            .query_row(
                "SELECT root_identity, relative_path, file_device, file_inode,
                        prefix_generation, range_start, range_end, record_sha256,
                        event_index, timestamp, model, total_tokens, input_tokens,
                        cached_input_tokens, output_tokens, cache_write_input_tokens
                 FROM session_events
                 WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3
                   AND file_inode=?4 AND prefix_generation=?5 AND range_start=?6
                   AND range_end=?7 AND record_sha256=?8 AND event_index=?9",
                params![
                    &event.root_identity,
                    &event.relative_path,
                    event.file_device.to_string(),
                    event.file_inode.to_string(),
                    format!("{:032x}", event.prefix_generation),
                    event.range_start as i64,
                    event.range_end as i64,
                    &event.record_sha256,
                    event.event_index as i64,
                ],
                |row| {
                    Ok(SessionEvent {
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
                        prefix_generation: u128::from_str_radix(&row.get::<_, String>(4)?, 16)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        range_start: u64::try_from(row.get::<_, i64>(5)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        range_end: u64::try_from(row.get::<_, i64>(6)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        record_sha256: row.get(7)?,
                        event_index: u64::try_from(row.get::<_, i64>(8)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        timestamp: row.get(9)?,
                        model: row.get(10)?,
                        total_tokens: row
                            .get::<_, String>(11)?
                            .parse()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        input_tokens: row
                            .get::<_, String>(12)?
                            .parse()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cached_input_tokens: row
                            .get::<_, String>(13)?
                            .parse()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        output_tokens: row
                            .get::<_, String>(14)?
                            .parse()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cache_write_input_tokens: row
                            .get::<_, Option<String>>(15)?
                            .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
                            .transpose()?,
                    })
                },
            )
            .optional()?;
        if existing.as_ref() != Some(event) {
            return Err(UsageStoreError::InvalidImport(
                "session event replay conflicts with stored evidence".into(),
            ));
        }
        validate_session_event(existing.as_ref().expect("event row was inserted"))?;
    }
    Ok(())
}

fn upsert_session_task_indexed_ranges(
    transaction: &rusqlite::Transaction<'_>,
    ranges: &BTreeMap<SessionTaskIndexedRangeKey, SessionTaskIndexedRange>,
) -> Result<()> {
    let mut insert = transaction.prepare(
        "INSERT INTO session_task_indexed_ranges (
            root_identity, relative_path, file_device, file_inode,
            start_offset, end_offset, collector_epoch, cycle_seq,
            prefix_generation, record_sha256
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT DO NOTHING",
    )?;
    for range in ranges.values() {
        insert.execute(params![
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
    drop(insert);
    for range in ranges.values() {
        let stored: Option<SessionTaskIndexedRange> = transaction
            .query_row(
                "SELECT root_identity, relative_path, file_device, file_inode,
                        start_offset, end_offset, collector_epoch, cycle_seq,
                        prefix_generation, record_sha256
                 FROM session_task_indexed_ranges
                 WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3
                   AND file_inode=?4 AND prefix_generation=?5 AND start_offset=?6
                   AND end_offset=?7 AND record_sha256=?8",
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
                |row| {
                    let start_offset = row.get::<_, i64>(4)?;
                    let end_offset = row.get::<_, i64>(5)?;
                    Ok(SessionTaskIndexedRange {
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
                        start_offset: u64::try_from(start_offset)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        end_offset: u64::try_from(end_offset)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        collector_epoch: u128::from_str_radix(&row.get::<_, String>(6)?, 16)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        cycle_seq: row
                            .get::<_, String>(7)?
                            .parse()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        prefix_generation: u128::from_str_radix(&row.get::<_, String>(8)?, 16)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        record_sha256: row.get(9)?,
                    })
                },
            )
            .optional()?;
        let same_identity = stored.as_ref().is_some_and(|stored| {
            stored.root_identity == range.root_identity
                && stored.relative_path == range.relative_path
                && stored.file_device == range.file_device
                && stored.file_inode == range.file_inode
                && stored.start_offset == range.start_offset
                && stored.end_offset == range.end_offset
                && stored.prefix_generation == range.prefix_generation
                && stored.record_sha256 == range.record_sha256
        });
        if !same_identity {
            return Err(UsageStoreError::InvalidImport(
                "session task indexed range replay conflicts with stored evidence".into(),
            ));
        }
        validate_session_task_indexed_range(stored.as_ref().expect("indexed range was inserted"))?;
    }
    Ok(())
}

fn upsert_session_task_events(
    transaction: &rusqlite::Transaction<'_>,
    events: &BTreeMap<SessionTaskEventKey, SessionTaskEvent>,
) -> Result<()> {
    let mut insert = transaction.prepare(
        "INSERT INTO session_task_events (
            root_identity, relative_path, file_device, file_inode,
            prefix_generation, start_offset, end_offset, record_sha256,
            event_index, timestamp, running
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT DO NOTHING",
    )?;
    for event in events.values() {
        insert.execute(params![
            &event.root_identity,
            &event.relative_path,
            event.file_device.to_string(),
            event.file_inode.to_string(),
            format!("{:032x}", event.prefix_generation),
            event.start_offset as i64,
            event.end_offset as i64,
            &event.record_sha256,
            event.event_index as i64,
            event.timestamp,
            i64::from(event.running),
        ])?;
    }
    drop(insert);
    for event in events.values() {
        let stored: Option<SessionTaskEvent> = transaction
            .query_row(
                "SELECT root_identity, relative_path, file_device, file_inode,
                        prefix_generation, start_offset, end_offset, record_sha256,
                        event_index, timestamp, running
                 FROM session_task_events
                 WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3
                   AND file_inode=?4 AND prefix_generation=?5 AND start_offset=?6
                   AND end_offset=?7 AND record_sha256=?8 AND event_index=?9",
                params![
                    &event.root_identity,
                    &event.relative_path,
                    event.file_device.to_string(),
                    event.file_inode.to_string(),
                    format!("{:032x}", event.prefix_generation),
                    event.start_offset as i64,
                    event.end_offset as i64,
                    &event.record_sha256,
                    event.event_index as i64,
                ],
                |row| {
                    Ok(SessionTaskEvent {
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
                        prefix_generation: u128::from_str_radix(&row.get::<_, String>(4)?, 16)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        start_offset: u64::try_from(row.get::<_, i64>(5)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        end_offset: u64::try_from(row.get::<_, i64>(6)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        record_sha256: row.get(7)?,
                        event_index: u64::try_from(row.get::<_, i64>(8)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        timestamp: row.get(9)?,
                        running: match row.get::<_, i64>(10)? {
                            0 => false,
                            1 => true,
                            _ => return Err(rusqlite::Error::InvalidQuery),
                        },
                    })
                },
            )
            .optional()?;
        if stored.as_ref() != Some(event) {
            return Err(UsageStoreError::InvalidImport(
                "session task event replay conflicts with stored evidence".into(),
            ));
        }
        validate_session_task_event(stored.as_ref().expect("task event was inserted"))?;
    }
    Ok(())
}

fn canonicalize_task_evidence(
    task_events: Option<&[SessionTaskEvent]>,
    indexed_ranges: Option<&[SessionTaskIndexedRange]>,
) -> Result<(
    BTreeMap<SessionTaskIndexedRangeKey, SessionTaskIndexedRange>,
    BTreeMap<SessionTaskEventKey, SessionTaskEvent>,
)> {
    let mut canonical_ranges: BTreeMap<SessionTaskIndexedRangeKey, SessionTaskIndexedRange> =
        BTreeMap::new();
    for range in indexed_ranges.unwrap_or(&[]) {
        validate_session_task_indexed_range(range)?;
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
        if let Some(existing) = canonical_ranges.get(&key) {
            if (range.collector_epoch, range.cycle_seq)
                < (existing.collector_epoch, existing.cycle_seq)
            {
                canonical_ranges.insert(key, range.clone());
            }
            continue;
        }
        canonical_ranges.insert(key, range.clone());
    }
    let mut canonical_events: BTreeMap<SessionTaskEventKey, SessionTaskEvent> = BTreeMap::new();
    for event in task_events.unwrap_or(&[]) {
        validate_session_task_event(event)?;
        let matching_range = canonical_ranges.values().any(|range| {
            range.root_identity == event.root_identity
                && range.relative_path == event.relative_path
                && range.file_device == event.file_device
                && range.file_inode == event.file_inode
                && range.prefix_generation == event.prefix_generation
                && range.start_offset == event.start_offset
                && range.end_offset == event.end_offset
                && range.record_sha256 == event.record_sha256
        });
        if !matching_range {
            return Err(UsageStoreError::InvalidImport(
                "session task event has no indexed source range".into(),
            ));
        }
        let key = (
            event.root_identity.clone(),
            event.relative_path.clone(),
            event.file_device,
            event.file_inode,
            event.prefix_generation,
            event.start_offset,
            event.end_offset,
            event.record_sha256.clone(),
            event.event_index,
        );
        if let Some(existing) = canonical_events.get(&key) {
            if existing.timestamp != event.timestamp || existing.running != event.running {
                return Err(UsageStoreError::InvalidImport(
                    "duplicate session task event conflicts with its evidence".into(),
                ));
            }
            continue;
        }
        canonical_events.insert(key, event.clone());
    }
    Ok((canonical_ranges, canonical_events))
}

fn canonicalize_observations(
    transaction: &rusqlite::Transaction<'_>,
    observations: &[UsageHistoryObservation],
    canonical_samples: &[UsageHistorySample],
    source_to_canonical: &BTreeMap<(i64, i64), (i64, i64)>,
) -> Result<Vec<UsageHistoryObservation>> {
    let canonical_storage = canonical_history_constraints_present(transaction)?;
    let samples_by_timestamp = canonical_samples
        .iter()
        .map(|sample| (sample.timestamp, sample))
        .collect::<BTreeMap<_, _>>();
    let canonical_samples = canonical_samples
        .iter()
        .map(|sample| ((sample.reset_at, sample.timestamp), sample))
        .collect::<BTreeMap<_, _>>();
    let mut canonical = BTreeMap::new();
    for observation in observations {
        observation.validate()?;
        let mut observation = observation.clone();
        if canonical_storage && observation.model_source != ModelSource::Unavailable {
            let source_key = (observation.reset_at, observation.timestamp);
            let Some((canonical_reset_at, canonical_timestamp)) =
                source_to_canonical.get(&source_key)
            else {
                // The single history authority rejected this raw minute (for
                // example, conflicting quota or a non-comparable whole
                // vector). Its dependent model observation must be excluded
                // with it rather than attached to another retained row.
                continue;
            };
            observation.reset_at = *canonical_reset_at;
            observation.timestamp = *canonical_timestamp;
        }
        let key = (observation.reset_at, observation.timestamp);
        if canonical.insert(key, observation).is_some() {
            return Err(UsageStoreError::InvalidImport(
                "duplicate usage observation key".into(),
            ));
        }
    }
    for (key, observation) in canonical.iter_mut() {
        match observation.model_source {
            ModelSource::Confirmed
            | ModelSource::ReconstructedFromSession
            | ModelSource::LegacyUnknown => {
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
                if canonical_samples.contains_key(key)
                    || (canonical_storage && samples_by_timestamp.contains_key(&key.1))
                {
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
        let source_rank = |source: ModelSource| match source {
            ModelSource::Unavailable => 0,
            ModelSource::LegacyUnknown => 1,
            ModelSource::ReconstructedFromSession => 2,
            ModelSource::Confirmed => 3,
        };
        let selected =
            if source_rank(observation.model_source) >= source_rank(existing.model_source) {
                observation.clone()
            } else {
                existing.clone()
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
    preserve_existing: bool,
    preserve_existing_before: Option<i64>,
) -> Result<()> {
    let mut delete = transaction.prepare("DELETE FROM usage_model_history WHERE timestamp = ?1")?;
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
        if preserve_existing
            || preserve_existing_before.is_some_and(|cutoff| observation.timestamp < cutoff)
        {
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM usage_model_history
                    WHERE timestamp = ?1
                )",
                [observation.timestamp],
                |row| row.get(0),
            )?;
            if exists {
                continue;
            }
        }
        delete.execute([observation.timestamp])?;
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

fn sample_from_row_with_quota_policy(
    row: &rusqlite::Row<'_>,
    legacy_minus_one_quota_is_missing: bool,
) -> Result<Option<UsageHistorySample>> {
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
            if legacy_minus_one_quota_is_missing && value == -1.0 {
                None
            } else {
                Some(value)
            }
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

fn valid_sample_from_row(row: &rusqlite::Row<'_>) -> Result<Option<UsageHistorySample>> {
    sample_from_row_with_quota_policy(row, false)
}

fn canonicalizable_legacy_sample_from_row(
    row: &rusqlite::Row<'_>,
) -> Result<Option<UsageHistorySample>> {
    // Only the exact sentinel emitted by the legacy writer is migration
    // input. Every other out-of-domain value remains corruption and aborts
    // the partition transaction.
    sample_from_row_with_quota_policy(row, true)
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

fn load_valid_samples_from_table(
    connection: &Connection,
    table: &str,
    cutoff: Option<i64>,
) -> Result<Vec<UsageHistorySample>> {
    if table != "usage_history" {
        return Err(UsageStoreError::InvalidImport(
            "history table selector is invalid".into(),
        ));
    }
    let query = if cutoff.is_some() {
        format!(
            "SELECT timestamp, reset_at, remaining_percent, sol_dollars, \
                    terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens \
             FROM {table} WHERE timestamp > ?1 ORDER BY reset_at, timestamp"
        )
    } else {
        format!(
            "SELECT timestamp, reset_at, remaining_percent, sol_dollars, \
                    terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens \
             FROM {table} ORDER BY reset_at, timestamp"
        )
    };
    let mut statement = connection.prepare(&query)?;
    let mut rows = match cutoff {
        Some(cutoff) => statement.query([cutoff])?,
        None => statement.query([])?,
    };
    let mut samples = Vec::new();
    while let Some(row) = rows.next()? {
        let sample = valid_sample_from_row(row)?;
        let Some(sample) = sample else {
            return Err(UsageStoreError::InvalidImport(format!(
                "{table} contains an invalid row"
            )));
        };
        samples.push(sample);
    }
    Ok(samples)
}

fn legacy_raw_evidence(connection: &Connection) -> Result<(usize, String)> {
    let mut statement = connection.prepare(
        "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
         FROM usage_history ORDER BY reset_at, timestamp",
    )?;
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();
    digest.update(b"codex-info-legacy-usage-history-sqlite-values-v1\0");
    let mut row_count = 0_usize;
    while let Some(row) = rows.next()? {
        if canonicalizable_legacy_sample_from_row(row)?.is_none() {
            return Err(UsageStoreError::InvalidImport(
                "usage_history contains an invalid raw row".into(),
            ));
        }
        row_count = row_count
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        for column in 0..9 {
            match row.get::<_, Value>(column)? {
                Value::Null => digest.update([0]),
                Value::Integer(value) => {
                    digest.update([1]);
                    digest.update(value.to_be_bytes());
                }
                Value::Real(value) => {
                    digest.update([2]);
                    digest.update(value.to_bits().to_be_bytes());
                }
                Value::Text(value) => {
                    digest.update([3]);
                    digest.update((value.len() as u64).to_be_bytes());
                    digest.update(value.as_bytes());
                }
                Value::Blob(value) => {
                    digest.update([4]);
                    digest.update((value.len() as u64).to_be_bytes());
                    digest.update(&value);
                }
            }
        }
    }
    Ok((row_count, format!("{:x}", digest.finalize())))
}

const HISTORY_CANONICAL_INDEX_NAMES: [&str; 2] = [
    "usage_history_canonical_timestamp_idx",
    "usage_model_history_canonical_timestamp_model_idx",
];
const HISTORY_CANONICAL_TRIGGER_NAMES: [&str; 8] = [
    "usage_history_canonical_insert_guard",
    "usage_history_canonical_update_guard",
    "usage_model_history_canonical_insert_guard",
    "usage_model_history_canonical_update_guard",
    "durable_history_observation_insert_guard",
    "durable_history_observation_update_guard",
    "usage_history_sidecar_update_guard",
    "usage_history_sidecar_delete_guard",
];

#[derive(Clone, Debug)]
struct HistoryModelGroup {
    timestamp: i64,
    reset_at: i64,
    totals: Vec<SessionModelTotal>,
    complete: bool,
}

#[derive(Clone, Debug)]
struct CanonicalHistoryMigrationRow {
    sample: UsageHistorySample,
    source_timestamp: i64,
    source_reset_at: i64,
}

fn named_schema_object_count(connection: &Connection, kind: &str, names: &[&str]) -> Result<usize> {
    let mut count = 0_usize;
    for name in names {
        let present: bool = connection.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_schema WHERE type=?1 AND name=?2
             )",
            params![kind, name],
            |row| row.get(0),
        )?;
        count += usize::from(present);
    }
    Ok(count)
}

fn canonical_history_constraints_present(connection: &Connection) -> Result<bool> {
    Ok(
        named_schema_object_count(connection, "index", &HISTORY_CANONICAL_INDEX_NAMES)?
            == HISTORY_CANONICAL_INDEX_NAMES.len()
            && named_schema_object_count(connection, "trigger", &HISTORY_CANONICAL_TRIGGER_NAMES)?
                == HISTORY_CANONICAL_TRIGGER_NAMES.len(),
    )
}

fn load_history_model_groups(connection: &Connection) -> Result<Vec<HistoryModelGroup>> {
    let mut statement = connection.prepare(
        "SELECT reset_at, timestamp, model, total_tokens, input_tokens,
                cached_input_tokens, output_tokens, cache_write_input_tokens,
                model_set_complete
         FROM usage_model_history
         ORDER BY timestamp, reset_at, model",
    )?;
    let mut rows = statement.query([])?;
    let mut groups = BTreeMap::<(i64, i64), (Vec<SessionModelTotal>, bool)>::new();
    while let Some(row) = rows.next()? {
        let reset_at: i64 = row.get(0)?;
        let timestamp: i64 = row.get(1)?;
        let complete = match row.get::<_, i64>(8)? {
            0 => false,
            1 => true,
            _ => {
                return Err(UsageStoreError::InvalidImport(
                    "history model completeness is invalid".into(),
                ));
            }
        };
        if reset_at <= 0 || timestamp <= 0 || timestamp > reset_at {
            return Err(UsageStoreError::InvalidImport(
                "history model key is invalid".into(),
            ));
        }
        let cache_write: Option<String> = row.get(7)?;
        let total = SessionModelTotal {
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
        };
        let entry = groups
            .entry((timestamp, reset_at))
            .or_insert_with(|| (Vec::new(), complete));
        if entry.1 != complete {
            return Err(UsageStoreError::InvalidImport(
                "history model completeness differs inside one observation".into(),
            ));
        }
        entry.0.push(total);
    }
    groups
        .into_iter()
        .map(|((timestamp, reset_at), (totals, complete))| {
            Ok(HistoryModelGroup {
                timestamp,
                reset_at,
                totals: canonicalize_model_totals(&totals)?,
                complete,
            })
        })
        .collect()
}

fn observation_matches_sample(
    observation: &UsageHistoryObservation,
    sample: &UsageHistorySample,
) -> bool {
    observation.remaining_percent == sample.remaining_percent
        && observation.sol_dollars == Some(sample.sol_dollars)
        && observation.terra_dollars == Some(sample.terra_dollars)
        && observation.luna_dollars == Some(sample.luna_dollars)
        && observation.sol_tokens == Some(sample.sol_tokens)
        && observation.terra_tokens == Some(sample.terra_tokens)
        && observation.luna_tokens == Some(sample.luna_tokens)
}

fn validate_canonical_history_storage(connection: &Connection) -> Result<()> {
    if !canonical_history_constraints_present(connection)? {
        return Err(UsageStoreError::InvalidImport(
            "canonical history constraint set is incomplete".into(),
        ));
    }
    let invalid: i64 = connection.query_row(
        "SELECT COUNT(*) FROM usage_history
         WHERE typeof(timestamp) <> 'integer' OR timestamp <= 0 OR timestamp % 60 <> 0
            OR typeof(reset_at) <> 'integer' OR reset_at <= 0 OR timestamp > reset_at
            OR (remaining_percent IS NOT NULL AND (
                typeof(remaining_percent) NOT IN ('integer', 'real')
                OR remaining_percent < 0.0 OR remaining_percent > 100.0
            ))
            OR typeof(sol_dollars) NOT IN ('integer', 'real')
            OR typeof(terra_dollars) NOT IN ('integer', 'real')
            OR typeof(luna_dollars) NOT IN ('integer', 'real')
            OR sol_dollars < 0.0 OR sol_dollars >= 1e999
            OR terra_dollars < 0.0 OR terra_dollars >= 1e999
            OR luna_dollars < 0.0 OR luna_dollars >= 1e999
            OR typeof(sol_tokens) <> 'integer' OR sol_tokens < 0
            OR typeof(terra_tokens) <> 'integer' OR terra_tokens < 0
            OR typeof(luna_tokens) <> 'integer' OR luna_tokens < 0",
        [],
        |row| row.get(0),
    )?;
    if invalid != 0 {
        return Err(UsageStoreError::InvalidImport(
            "canonical usage history contains invalid rows".into(),
        ));
    }
    let samples = load_valid_samples_from_table(connection, "usage_history", None)?;
    let samples_by_key = samples
        .iter()
        .map(|sample| ((sample.timestamp, sample.reset_at), sample))
        .collect::<BTreeMap<_, _>>();
    for group in load_history_model_groups(connection)? {
        if !samples_by_key.contains_key(&(group.timestamp, group.reset_at)) {
            return Err(UsageStoreError::InvalidImport(
                "history model row has no canonical usage parent".into(),
            ));
        }
    }
    let mut statement = connection.prepare(
        "SELECT data_generation, data_hash, snapshot_json
         FROM durable_state WHERE singleton >= ?1 ORDER BY singleton",
    )?;
    let mut rows = statement.query([DURABLE_STATE_OBSERVATION_MIN_SINGLETON])?;
    while let Some(row) = rows.next()? {
        let observation = observation_from_sql(row.get(0)?, row.get(1)?, row.get(2)?)?;
        if observation.model_source == ModelSource::Unavailable {
            if samples
                .iter()
                .any(|sample| sample.timestamp == observation.timestamp)
            {
                return Err(UsageStoreError::InvalidImport(
                    "unavailable durable observation conflicts with canonical usage".into(),
                ));
            }
            continue;
        }
        let Some(sample) = samples_by_key.get(&(observation.timestamp, observation.reset_at))
        else {
            return Err(UsageStoreError::InvalidImport(
                "durable history observation has no canonical usage parent".into(),
            ));
        };
        if !observation_matches_sample(&observation, sample) {
            let mismatched_fields = [
                (
                    "remaining_percent",
                    observation.remaining_percent == sample.remaining_percent,
                ),
                (
                    "sol_dollars",
                    observation.sol_dollars == Some(sample.sol_dollars),
                ),
                (
                    "terra_dollars",
                    observation.terra_dollars == Some(sample.terra_dollars),
                ),
                (
                    "luna_dollars",
                    observation.luna_dollars == Some(sample.luna_dollars),
                ),
                (
                    "sol_tokens",
                    observation.sol_tokens == Some(sample.sol_tokens),
                ),
                (
                    "terra_tokens",
                    observation.terra_tokens == Some(sample.terra_tokens),
                ),
                (
                    "luna_tokens",
                    observation.luna_tokens == Some(sample.luna_tokens),
                ),
            ]
            .into_iter()
            .filter_map(|(field, matches)| (!matches).then_some(field))
            .collect::<Vec<_>>()
            .join(",");
            return Err(UsageStoreError::InvalidImport(
                format!(
                    "durable history observation differs from canonical usage at timestamp={} reset_at={} fields={mismatched_fields}",
                    observation.timestamp, observation.reset_at
                ),
            ));
        }
    }
    Ok(())
}

fn ensure_canonical_history_constraints(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let index_count =
        named_schema_object_count(transaction, "index", &HISTORY_CANONICAL_INDEX_NAMES)?;
    let trigger_count =
        named_schema_object_count(transaction, "trigger", &HISTORY_CANONICAL_TRIGGER_NAMES)?;
    if index_count == 0 && trigger_count == 0 {
        transaction.execute_batch(HISTORY_CANONICAL_CONSTRAINTS)?;
    } else if index_count != HISTORY_CANONICAL_INDEX_NAMES.len()
        || trigger_count != HISTORY_CANONICAL_TRIGGER_NAMES.len()
    {
        return Err(UsageStoreError::InvalidImport(
            "partial canonical history constraints are not recoverable automatically".into(),
        ));
    }
    validate_canonical_history_storage(transaction)
}

fn load_legacy_samples_for_migration(connection: &Connection) -> Result<Vec<UsageHistorySample>> {
    let mut statement = connection.prepare(
        "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
         FROM usage_history ORDER BY timestamp, reset_at",
    )?;
    let mut rows = statement.query([])?;
    let mut samples = Vec::new();
    while let Some(row) = rows.next()? {
        let Some(sample) = canonicalizable_legacy_sample_from_row(row)? else {
            return Err(UsageStoreError::InvalidImport(
                "usage_history contains a non-migratable row".into(),
            ));
        };
        samples.push(sample);
    }
    Ok(samples)
}

fn canonicalize_legacy_usage_history(
    connection: &Connection,
    legacy: &[UsageHistorySample],
) -> Result<Vec<CanonicalHistoryMigrationRow>> {
    let (current_reset_at, window_seconds): (i64, i64) = connection.query_row(
        "SELECT reset_at, window_seconds FROM collection_generation WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let raw = legacy
        .iter()
        .map(|sample| CanonicalRawSample {
            timestamp: sample.timestamp,
            reset_at: sample.reset_at,
            remaining_percent: sample.remaining_percent,
            sol_dollars: sample.sol_dollars,
            terra_dollars: sample.terra_dollars,
            luna_dollars: sample.luna_dollars,
            sol_tokens: sample.sol_tokens,
            terra_tokens: sample.terra_tokens,
            luna_tokens: sample.luna_tokens,
        })
        .collect::<Vec<_>>();
    let canonical = canonicalize_history_for_storage_with_sources(
        &raw,
        (current_reset_at > 0).then_some(current_reset_at),
        window_seconds.max(0),
    )
    .map_err(|error| {
        UsageStoreError::InvalidImport(format!("history canonicalization failed: {error}"))
    })?;
    if !legacy.is_empty() && canonical.is_empty() {
        return Err(UsageStoreError::InvalidImport(
            "non-empty history has no unambiguous canonical rows".into(),
        ));
    }
    let mut timestamps = BTreeSet::new();
    canonical
        .into_iter()
        .map(|canonical| {
            let sample = canonical.sample;
            let sample = UsageHistorySample {
                timestamp: sample.timestamp,
                reset_at: sample.reset_at,
                remaining_percent: sample.remaining_percent,
                sol_dollars: sample.sol_dollars,
                terra_dollars: sample.terra_dollars,
                luna_dollars: sample.luna_dollars,
                sol_tokens: sample.sol_tokens,
                terra_tokens: sample.terra_tokens,
                luna_tokens: sample.luna_tokens,
            };
            sample.validate()?;
            if sample.timestamp.rem_euclid(60) != 0 || !timestamps.insert(sample.timestamp) {
                return Err(UsageStoreError::InvalidImport(
                    "canonical history timestamp is not globally unique".into(),
                ));
            }
            Ok(CanonicalHistoryMigrationRow {
                sample,
                source_timestamp: canonical.source_timestamp,
                source_reset_at: canonical.source_reset_at,
            })
        })
        .collect()
}

fn canonicalize_history_model_groups(
    legacy_groups: Vec<HistoryModelGroup>,
    canonical_rows: &[CanonicalHistoryMigrationRow],
) -> Result<Vec<HistoryModelGroup>> {
    let canonical_by_source = canonical_rows
        .iter()
        .map(|row| ((row.source_timestamp, row.source_reset_at), &row.sample))
        .collect::<BTreeMap<_, _>>();
    let mut canonical = Vec::new();
    for group in legacy_groups {
        let Some(sample) = canonical_by_source.get(&(group.timestamp, group.reset_at)) else {
            continue;
        };
        canonical.push(HistoryModelGroup {
            timestamp: sample.timestamp,
            reset_at: sample.reset_at,
            totals: group.totals,
            complete: group.complete,
        });
    }
    Ok(canonical)
}

fn model_source_rank(source: ModelSource) -> u8 {
    match source {
        ModelSource::Unavailable => 0,
        ModelSource::LegacyUnknown => 1,
        ModelSource::ReconstructedFromSession => 2,
        ModelSource::Confirmed => 3,
    }
}

fn canonicalize_durable_history_observations(
    connection: &Connection,
    canonical_rows: &[CanonicalHistoryMigrationRow],
) -> Result<Vec<(i64, UsageHistoryObservation)>> {
    let canonical_by_source = canonical_rows
        .iter()
        .map(|row| ((row.source_timestamp, row.source_reset_at), &row.sample))
        .collect::<BTreeMap<_, _>>();
    let mut statement = connection.prepare(
        "SELECT singleton, data_generation, data_hash, snapshot_json
         FROM durable_state WHERE singleton >= ?1 ORDER BY singleton",
    )?;
    let mut rows = statement.query([DURABLE_STATE_OBSERVATION_MIN_SINGLETON])?;
    let mut selected = BTreeMap::<i64, (i64, UsageHistoryObservation)>::new();
    while let Some(row) = rows.next()? {
        let singleton: i64 = row.get(0)?;
        let mut observation = observation_from_sql(row.get(1)?, row.get(2)?, row.get(3)?)?;
        let Some(sample) = canonical_by_source.get(&(observation.timestamp, observation.reset_at))
        else {
            continue;
        };
        if observation.model_source == ModelSource::Unavailable
            || !observation_matches_sample(&observation, sample)
        {
            continue;
        }
        observation.timestamp = sample.timestamp;
        observation.reset_at = sample.reset_at;
        observation.validate()?;
        let replace = selected
            .get(&sample.timestamp)
            .is_none_or(|(stored_singleton, stored)| {
                model_source_rank(observation.model_source) > model_source_rank(stored.model_source)
                    || (model_source_rank(observation.model_source)
                        == model_source_rank(stored.model_source)
                        && singleton < *stored_singleton)
            });
        if replace {
            selected.insert(sample.timestamp, (singleton, observation));
        }
    }
    Ok(selected.into_values().collect())
}

fn rewrite_canonical_history(
    transaction: &rusqlite::Transaction<'_>,
    canonical_samples: &[UsageHistorySample],
    canonical_models: &[HistoryModelGroup],
    canonical_observations: &[(i64, UsageHistoryObservation)],
) -> Result<()> {
    transaction.execute("DELETE FROM usage_model_history", [])?;
    transaction.execute(
        "DELETE FROM durable_state WHERE singleton >= ?1",
        [DURABLE_STATE_OBSERVATION_MIN_SINGLETON],
    )?;
    transaction.execute("DELETE FROM usage_history", [])?;
    {
        let mut insert = transaction.prepare(
            "INSERT INTO usage_history (
                 timestamp, reset_at, remaining_percent, sol_dollars,
                 terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for sample in canonical_samples {
            insert.execute(params![
                sample.timestamp,
                sample.reset_at,
                sample.remaining_percent,
                sample.sol_dollars,
                sample.terra_dollars,
                sample.luna_dollars,
                i64::try_from(sample.sol_tokens)
                    .map_err(|_| UsageStoreError::GenerationOverflow)?,
                i64::try_from(sample.terra_tokens)
                    .map_err(|_| UsageStoreError::GenerationOverflow)?,
                i64::try_from(sample.luna_tokens)
                    .map_err(|_| UsageStoreError::GenerationOverflow)?,
            ])?;
        }
    }
    {
        let mut insert = transaction.prepare(
            "INSERT INTO usage_model_history (
                 reset_at, timestamp, model, total_tokens, input_tokens,
                 cached_input_tokens, output_tokens, cache_write_input_tokens,
                 model_set_complete
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for group in canonical_models {
            for total in &group.totals {
                insert.execute(params![
                    group.reset_at,
                    group.timestamp,
                    &total.model,
                    total.total_tokens.to_string(),
                    total.input_tokens.to_string(),
                    total.cached_input_tokens.to_string(),
                    total.output_tokens.to_string(),
                    total
                        .cache_write_input_tokens
                        .map(|value| value.to_string()),
                    i64::from(group.complete),
                ])?;
            }
        }
    }
    {
        let mut insert = transaction.prepare(
            "INSERT INTO durable_state (singleton, data_generation, data_hash, snapshot_json)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (singleton, observation) in canonical_observations {
            insert.execute(params![
                singleton,
                observation.timestamp,
                observation_data_hash(observation.reset_at, observation.timestamp),
                observation_json(observation)?,
            ])?;
        }
    }
    Ok(())
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
                ("login_id", "TEXT", 0),
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
            "session_pending_ranges",
            &[
                ("root_identity", "TEXT", 1),
                ("relative_path", "TEXT", 2),
                ("file_device", "TEXT", 3),
                ("file_inode", "TEXT", 4),
                ("start_offset", "INTEGER", 6),
                ("end_offset", "INTEGER", 0),
                ("collector_epoch", "TEXT", 0),
                ("cycle_seq", "TEXT", 0),
                ("prefix_generation", "TEXT", 5),
                ("record_sha256", "TEXT", 0),
                ("parser_version", "TEXT", 0),
                ("reason", "TEXT", 0),
                ("complete", "INTEGER", 0),
            ],
        ),
        (
            "session_events",
            &[
                ("root_identity", "TEXT", 1),
                ("relative_path", "TEXT", 2),
                ("file_device", "TEXT", 3),
                ("file_inode", "TEXT", 4),
                ("prefix_generation", "TEXT", 5),
                ("range_start", "INTEGER", 6),
                ("range_end", "INTEGER", 7),
                ("record_sha256", "TEXT", 8),
                ("event_index", "INTEGER", 9),
                ("timestamp", "INTEGER", 0),
                ("model", "TEXT", 0),
                ("total_tokens", "TEXT", 0),
                ("input_tokens", "TEXT", 0),
                ("cached_input_tokens", "TEXT", 0),
                ("output_tokens", "TEXT", 0),
                ("cache_write_input_tokens", "TEXT", 0),
            ],
        ),
        (
            "session_task_events",
            &[
                ("root_identity", "TEXT", 1),
                ("relative_path", "TEXT", 2),
                ("file_device", "TEXT", 3),
                ("file_inode", "TEXT", 4),
                ("prefix_generation", "TEXT", 5),
                ("start_offset", "INTEGER", 6),
                ("end_offset", "INTEGER", 7),
                ("record_sha256", "TEXT", 8),
                ("event_index", "INTEGER", 9),
                ("timestamp", "INTEGER", 0),
                ("running", "INTEGER", 0),
            ],
        ),
        (
            "session_task_indexed_ranges",
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
            "session_cumulative_recoveries",
            &[
                ("recovery_id", "TEXT", 1),
                ("payload_json", "TEXT", 0),
                ("applied_generation", "TEXT", 0),
            ],
        ),
        (
            "session_timeline_recoveries",
            &[
                ("recovery_id", "TEXT", 1),
                ("payload_json", "TEXT", 0),
                ("applied_generation", "TEXT", 0),
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
        (
            "active_thread_snapshot",
            &[
                ("singleton", "INTEGER", 1),
                ("observed_at", "INTEGER", 0),
                ("threads_json", "TEXT", 0),
                ("acquisition_degraded", "INTEGER", 0),
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
    let expected_for_version = expected_tables.clone();
    let mut pre_continuity_tables = expected_for_version.clone();
    pre_continuity_tables.remove("history_continuity");
    pre_continuity_tables.remove("usage_model_history");
    pre_continuity_tables.remove("session_cumulative_recoveries");
    pre_continuity_tables.remove("session_timeline_recoveries");
    pre_continuity_tables.remove("session_pending_ranges");
    pre_continuity_tables.remove("session_events");
    pre_continuity_tables.remove("active_thread_snapshot");
    let mut pre_model_history_tables = expected_for_version.clone();
    pre_model_history_tables.remove("usage_model_history");
    pre_model_history_tables.remove("session_cumulative_recoveries");
    pre_model_history_tables.remove("session_timeline_recoveries");
    pre_model_history_tables.remove("session_pending_ranges");
    pre_model_history_tables.remove("session_events");
    pre_model_history_tables.remove("active_thread_snapshot");
    let mut pre_cumulative_recovery_tables = expected_for_version.clone();
    pre_cumulative_recovery_tables.remove("session_cumulative_recoveries");
    pre_cumulative_recovery_tables.remove("session_timeline_recoveries");
    pre_cumulative_recovery_tables.remove("session_pending_ranges");
    pre_cumulative_recovery_tables.remove("session_events");
    pre_cumulative_recovery_tables.remove("active_thread_snapshot");
    let mut pre_timeline_recovery_tables = expected_for_version.clone();
    pre_timeline_recovery_tables.remove("session_timeline_recoveries");
    pre_timeline_recovery_tables.remove("session_pending_ranges");
    pre_timeline_recovery_tables.remove("session_events");
    pre_timeline_recovery_tables.remove("active_thread_snapshot");
    let mut pre_pending_range_tables = expected_for_version.clone();
    pre_pending_range_tables.remove("session_pending_ranges");
    pre_pending_range_tables.remove("session_events");
    pre_pending_range_tables.remove("active_thread_snapshot");
    let mut pre_session_event_tables = expected_for_version.clone();
    pre_session_event_tables.remove("session_events");
    pre_session_event_tables.remove("active_thread_snapshot");
    let mut pre_session_task_evidence_tables = expected_for_version.clone();
    pre_session_task_evidence_tables.remove("session_task_events");
    pre_session_task_evidence_tables.remove("session_task_indexed_ranges");
    pre_session_task_evidence_tables.remove("active_thread_snapshot");
    let mut actual_tables_without_active = actual_tables.clone();
    actual_tables_without_active.remove("active_thread_snapshot");
    if actual_tables != expected_for_version
        && !(allow_unversioned_legacy
            && (actual_tables == pre_continuity_tables
                || actual_tables_without_active == pre_continuity_tables))
        && !(schema_version < 2
            && (actual_tables == pre_model_history_tables
                || actual_tables_without_active == pre_model_history_tables))
        && !(schema_version < 3
            && (actual_tables == pre_cumulative_recovery_tables
                || actual_tables_without_active == pre_cumulative_recovery_tables))
        && !(schema_version < 4
            && (actual_tables == pre_timeline_recovery_tables
                || actual_tables_without_active == pre_timeline_recovery_tables))
        && !(schema_version < 5
            && (actual_tables == pre_pending_range_tables
                || actual_tables_without_active == pre_pending_range_tables))
        && !(schema_version < 6
            && (actual_tables == pre_session_event_tables
                || actual_tables_without_active == pre_session_event_tables))
        && !(schema_version < 8
            && (actual_tables == pre_session_task_evidence_tables
                || actual_tables_without_active == pre_session_task_evidence_tables))
    {
        return Err(UsageStoreError::InvalidImport(
            "account partition table set mismatch".into(),
        ));
    }

    for (table, expected) in TABLES {
        if (*table == "history_continuity"
            || *table == "usage_model_history"
            || *table == "session_cumulative_recoveries"
            || *table == "session_timeline_recoveries"
            || *table == "session_pending_ranges"
            || *table == "session_events"
            || *table == "session_task_events"
            || *table == "session_task_indexed_ranges"
            || *table == "active_thread_snapshot")
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
        let legacy_storage_partition_login_id = *table == "storage_partition"
            && schema_version < 9
            && actual.len() + 1 == expected.len()
            && actual == expected[..actual.len()];
        let legacy_active_thread_snapshot_columns = *table == "active_thread_snapshot"
            && schema_version < 7
            && actual == legacy_active_thread_snapshot_columns();
        if actual != expected
            && !(allow_unversioned_legacy
                && ((*table == "recorder_gap_ledger"
                    && actual == legacy_recorder_gap_ledger_columns())
                    || (*table == "session_checkpoints"
                        && actual == legacy_session_checkpoint_columns())
                    || legacy_history_continuity
                    || legacy_cache_write_columns
                    || legacy_storage_partition_login_id))
            && !legacy_active_thread_snapshot_columns
            && !legacy_storage_partition_login_id
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

fn legacy_active_thread_snapshot_columns() -> Vec<(String, String, i64)> {
    vec![
        ("singleton".to_owned(), "INTEGER".to_owned(), 1),
        ("observed_at".to_owned(), "INTEGER".to_owned(), 0),
        ("threads_json".to_owned(), "TEXT".to_owned(), 0),
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

fn ensure_session_cumulative_recovery_schema(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<()> {
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS session_cumulative_recoveries (
            recovery_id TEXT PRIMARY KEY CHECK (
                length(recovery_id) = 64
                AND recovery_id NOT GLOB '*[^0-9a-f]*'
            ),
            payload_json TEXT NOT NULL CHECK (
                length(payload_json) BETWEEN 2 AND 1048576
            ),
            applied_generation TEXT NOT NULL
        ) WITHOUT ROWID;
        "#,
    )?;
    Ok(())
}

fn ensure_session_timeline_recovery_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS session_timeline_recoveries (
            recovery_id TEXT PRIMARY KEY CHECK (
                length(recovery_id) = 64
                AND recovery_id NOT GLOB '*[^0-9a-f]*'
            ),
            payload_json TEXT NOT NULL CHECK (
                length(payload_json) BETWEEN 2 AND 67108864
            ),
            applied_generation TEXT NOT NULL
        ) WITHOUT ROWID;
        "#,
    )?;
    Ok(())
}

fn ensure_session_pending_range_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS session_pending_ranges (
            root_identity TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            file_device TEXT NOT NULL,
            file_inode TEXT NOT NULL,
            start_offset INTEGER NOT NULL CHECK (start_offset >= 0),
            end_offset INTEGER NOT NULL CHECK (end_offset >= start_offset),
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
            parser_version TEXT NOT NULL CHECK (length(parser_version) BETWEEN 1 AND 128),
            reason TEXT NOT NULL CHECK (length(reason) BETWEEN 1 AND 512),
            complete INTEGER NOT NULL CHECK (complete IN (0, 1)),
            PRIMARY KEY (
                root_identity,
                relative_path,
                file_device,
                file_inode,
                prefix_generation,
                start_offset
            )
        ) WITHOUT ROWID;
        "#,
    )?;
    Ok(())
}

fn ensure_session_event_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS session_events (
            root_identity TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            file_device TEXT NOT NULL,
            file_inode TEXT NOT NULL,
            prefix_generation TEXT NOT NULL CHECK (
                length(prefix_generation) = 32
                AND prefix_generation NOT GLOB '*[^0-9a-f]*'
            ),
            range_start INTEGER NOT NULL CHECK (range_start >= 0),
            range_end INTEGER NOT NULL CHECK (range_end > range_start),
            record_sha256 TEXT NOT NULL CHECK (
                length(record_sha256) = 64
                AND record_sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            event_index INTEGER NOT NULL CHECK (event_index >= 0),
            timestamp INTEGER NOT NULL CHECK (timestamp > 0),
            model TEXT NOT NULL CHECK (length(model) BETWEEN 1 AND 512),
            total_tokens TEXT NOT NULL,
            input_tokens TEXT NOT NULL,
            cached_input_tokens TEXT NOT NULL,
            output_tokens TEXT NOT NULL,
            cache_write_input_tokens TEXT,
            PRIMARY KEY (
                root_identity,
                relative_path,
                file_device,
                file_inode,
                prefix_generation,
                range_start,
                range_end,
                record_sha256,
                event_index
            )
        ) WITHOUT ROWID;
        "#,
    )?;
    Ok(())
}

fn ensure_session_task_event_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS session_task_events (
            root_identity TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            file_device TEXT NOT NULL,
            file_inode TEXT NOT NULL,
            prefix_generation TEXT NOT NULL CHECK (
                length(prefix_generation) = 32
                AND prefix_generation NOT GLOB '*[^0-9a-f]*'
            ),
            start_offset INTEGER NOT NULL CHECK (start_offset >= 0),
            end_offset INTEGER NOT NULL CHECK (end_offset > start_offset),
            record_sha256 TEXT NOT NULL CHECK (
                length(record_sha256) = 64
                AND record_sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            event_index INTEGER NOT NULL CHECK (event_index >= 0),
            timestamp INTEGER NOT NULL CHECK (timestamp > 0),
            running INTEGER NOT NULL CHECK (running IN (0, 1)),
            PRIMARY KEY (
                root_identity,
                relative_path,
                file_device,
                file_inode,
                prefix_generation,
                start_offset,
                end_offset,
                record_sha256,
                event_index
            )
        ) WITHOUT ROWID;

        CREATE TABLE IF NOT EXISTS session_task_indexed_ranges (
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
        "#,
    )?;
    Ok(())
}

/// Add the active-thread publication row without rewriting any existing
/// candidate.  Older partitions either have no table or the original
/// three-column table; the nullable-looking state is represented by a
/// non-null default so an old complete row remains a healthy snapshot.
fn ensure_active_thread_snapshot_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS active_thread_snapshot (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            observed_at INTEGER NOT NULL CHECK (observed_at > 0),
            threads_json TEXT NOT NULL CHECK (length(threads_json) BETWEEN 2 AND 1048576),
            acquisition_degraded INTEGER NOT NULL DEFAULT 0 CHECK (acquisition_degraded IN (0, 1))
        );
        "#,
    )?;
    let degraded_present: bool = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('active_thread_snapshot')
            WHERE name = 'acquisition_degraded'
        )",
        [],
        |row| row.get(0),
    )?;
    if !degraded_present {
        transaction.execute(
            "ALTER TABLE active_thread_snapshot
             ADD COLUMN acquisition_degraded INTEGER NOT NULL DEFAULT 0
             CHECK (acquisition_degraded IN (0, 1))",
            [],
        )?;
    }
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

/// Add the nullable display-only login identifier to a legacy partition.
/// This shape-only migration intentionally does not touch collection state or
/// any usage/session rows; the caller stamps schema version 9 only after all
/// partition migrations and validation succeed in the same transaction.
fn ensure_storage_partition_login_id_schema(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let table_exists: bool = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_schema
            WHERE type = 'table' AND name = 'storage_partition'
        )",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return Ok(());
    }
    let login_id_present: bool = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('storage_partition')
            WHERE name = 'login_id'
        )",
        [],
        |row| row.get(0),
    )?;
    if !login_id_present {
        transaction.execute("ALTER TABLE storage_partition ADD COLUMN login_id TEXT", [])?;
    }
    Ok(())
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
        let migrated_reason = if GAP_LEDGER_REASONS.contains(&legacy_reason.as_str()) {
            legacy_reason.as_str()
        } else {
            "auth_epoch_tombstoned"
        };
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
        ensure_storage_partition_login_id_schema(&transaction)?;
        ensure_durable_state_schema(&transaction)?;
        ensure_recorder_gap_ledger_schema(&transaction)?;
        ensure_session_checkpoint_schema(&transaction)?;
        ensure_history_continuity_schema(&transaction)?;
        ensure_usage_model_history_schema(&transaction)?;
        ensure_session_cumulative_recovery_schema(&transaction)?;
        ensure_session_timeline_recovery_schema(&transaction)?;
        ensure_session_pending_range_schema(&transaction)?;
        ensure_session_event_schema(&transaction)?;
        ensure_session_task_event_schema(&transaction)?;
        ensure_active_thread_snapshot_schema(&transaction)?;
        ensure_canonical_history_constraints(&transaction)?;
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
        let schema_version = account_db_schema_version(&probe)?;
        if schema_version != HISTORY_CANONICAL_SCHEMA_VERSION {
            return Err(UsageStoreError::InvalidImport(
                "account partition canonical history migration is required".into(),
            ));
        }
        validate_canonical_history_storage(&probe)?;
        drop(probe);
        let mut store = Self::open(path)?;
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_storage_partition_login_id_schema(&transaction)?;
        ensure_recorder_gap_ledger_schema(&transaction)?;
        ensure_session_checkpoint_schema(&transaction)?;
        ensure_history_continuity_schema(&transaction)?;
        ensure_usage_model_history_schema(&transaction)?;
        ensure_session_cumulative_recovery_schema(&transaction)?;
        ensure_session_timeline_recovery_schema(&transaction)?;
        ensure_session_pending_range_schema(&transaction)?;
        ensure_session_event_schema(&transaction)?;
        ensure_session_task_event_schema(&transaction)?;
        ensure_active_thread_snapshot_schema(&transaction)?;
        ensure_canonical_history_constraints(&transaction)?;
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

    /// Read the persisted display-only login identifier without mutating the
    /// partition.  The caller must have opened this store through the
    /// identity-validating partition constructor.
    pub fn partition_login_id(&self) -> Result<Option<String>> {
        let login_id: Option<String> = self.connection.query_row(
            "SELECT login_id FROM storage_partition WHERE singleton = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )?;
        if login_id
            .as_deref()
            .is_some_and(|value| !valid_login_id(value))
        {
            return Err(UsageStoreError::InvalidImport(
                "stored partition login id is invalid".into(),
            ));
        }
        Ok(login_id)
    }

    /// Persist the authenticated account's display-only login identifier.
    ///
    /// The serialized partition writer owns this mutation.  The value is
    /// stored only in `storage_partition`; it is never part of the opaque
    /// partition identity or collection generation.  Replaying the same
    /// value commits no UPDATE and remains a metadata-only no-op.
    pub fn set_partition_login_id(&mut self, login_id: &str) -> Result<()> {
        if !valid_login_id(login_id) {
            return Err(UsageStoreError::InvalidImport(
                "partition login id is invalid".into(),
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_storage_partition_login_id_schema(&transaction)?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT login_id FROM storage_partition WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        if existing.as_deref() == Some(login_id) {
            transaction.commit()?;
            return Ok(());
        }
        let changed = transaction.execute(
            "UPDATE storage_partition SET login_id = ?1 WHERE singleton = 1",
            [login_id],
        )?;
        if changed != 1 {
            return Err(UsageStoreError::InvalidImport(
                "storage partition singleton is missing".into(),
            ));
        }
        transaction.commit()?;
        Ok(())
    }

    /// Atomically publishes one complete active-thread candidate.  The
    /// payload and collection generation share one transaction so a failed
    /// candidate leaves the preceding complete row untouched.  Replaying the
    /// exact canonical thread payload is idempotent even when only the
    /// producer's observation time advances; that private timestamp is not a
    /// publication change.  A degraded row is cleared only by a successful
    /// complete publication.
    pub fn commit_active_thread_snapshot(
        &mut self,
        snapshot: &ActiveThreadSnapshot,
    ) -> Result<u64> {
        self.commit_active_thread_snapshot_with_health(snapshot, false)
    }

    /// Atomically publishes a complete candidate and the acquisition health
    /// that applies to it. This prevents a successful thread refresh from
    /// exposing a transient healthy generation while another acquisition
    /// lane is still failed or has not completed its first probe.
    pub fn commit_active_thread_snapshot_with_health(
        &mut self,
        snapshot: &ActiveThreadSnapshot,
        acquisition_degraded: bool,
    ) -> Result<u64> {
        let (observed_at, threads_json) = canonical_active_thread_snapshot(snapshot)?;
        let acquisition_degraded = i64::from(acquisition_degraded);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let generation_text: String = transaction.query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let generation = canonical_u64_text(&generation_text, "collection generation")?;
        let existing: Option<(i64, String, i64)> = transaction
            .query_row(
                "SELECT observed_at, threads_json, acquisition_degraded
                 FROM active_thread_snapshot WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((_existing_observed_at, existing_json, existing_degraded)) = &existing {
            if !matches!(*existing_degraded, 0 | 1) {
                return Err(UsageStoreError::InvalidImport(
                    "active thread acquisition state is invalid".into(),
                ));
            }
            if existing_json == &threads_json && *existing_degraded == acquisition_degraded {
                transaction.commit()?;
                return Ok(generation);
            }
        }
        let next = generation
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        transaction.execute(
            "INSERT INTO active_thread_snapshot (
                 singleton, observed_at, threads_json, acquisition_degraded
             ) VALUES (1, ?1, ?2, ?3)
             ON CONFLICT (singleton) DO UPDATE SET
                 observed_at = excluded.observed_at,
                 threads_json = excluded.threads_json,
                 acquisition_degraded = excluded.acquisition_degraded",
            params![observed_at, &threads_json, acquisition_degraded],
        )?;
        let changed = transaction.execute(
            "UPDATE collection_generation SET data_generation = ?1 WHERE singleton = 1",
            [next.to_string()],
        )?;
        if changed != 1 {
            return Err(UsageStoreError::InvalidImport(
                "collection generation singleton is missing".into(),
            ));
        }
        transaction.commit()?;
        Ok(next)
    }

    /// Marks acquisition failure while retaining the last complete thread
    /// payload.  A missing row is a no-op: it must never manufacture an empty
    /// snapshot or a synthetic generation.
    pub fn mark_acquisition_degraded(&mut self) -> Result<u64> {
        self.set_acquisition_degraded(true)
    }

    /// Clears the acquisition failure after all recorder lanes have succeeded.
    /// The retained payload is unchanged; only the degraded marker and
    /// publication generation move together.
    pub fn clear_acquisition_degraded(&mut self) -> Result<u64> {
        self.set_acquisition_degraded(false)
    }

    /// Compatibility alias for the explicit active-thread wording used by
    /// older recorder call sites.
    pub fn clear_active_thread_snapshot_degraded(&mut self) -> Result<u64> {
        self.clear_acquisition_degraded()
    }

    /// Compatibility alias for callers that name the failure after the
    /// active-thread lane.  The persisted state is shared with quota and
    /// other acquisition failures.
    pub fn mark_active_thread_snapshot_degraded(&mut self) -> Result<u64> {
        self.mark_acquisition_degraded()
    }

    fn set_acquisition_degraded(&mut self, degraded: bool) -> Result<u64> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let generation_text: String = transaction.query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let generation = canonical_u64_text(&generation_text, "collection generation")?;
        let table_exists: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema
                WHERE type = 'table' AND name = 'active_thread_snapshot'
            )",
            [],
            |row| row.get(0),
        )?;
        if !table_exists {
            transaction.commit()?;
            return Ok(generation);
        }
        let existing: Option<i64> = transaction
            .query_row(
                "SELECT acquisition_degraded FROM active_thread_snapshot
                 WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(existing) = existing else {
            transaction.commit()?;
            return Ok(generation);
        };
        if !matches!(existing, 0 | 1) {
            return Err(UsageStoreError::InvalidImport(
                "active thread acquisition state is invalid".into(),
            ));
        }
        let requested = i64::from(degraded);
        if existing == requested {
            transaction.commit()?;
            return Ok(generation);
        }
        let next = generation
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        transaction.execute(
            "UPDATE active_thread_snapshot SET acquisition_degraded = ?1
             WHERE singleton = 1",
            [requested],
        )?;
        let changed = transaction.execute(
            "UPDATE collection_generation SET data_generation = ?1 WHERE singleton = 1",
            [next.to_string()],
        )?;
        if changed != 1 {
            return Err(UsageStoreError::InvalidImport(
                "collection generation singleton is missing".into(),
            ));
        }
        transaction.commit()?;
        Ok(next)
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
        if generations == 0 {
            let source = Self::open_read_only_partitioned(path, identity)?;
            drop(source);
            return Ok(());
        }
        Self::backup_generations_partitioned_verified(path, identity, generations).map(|_| ())
    }

    /// Backup variant that returns an opaque source proof for the one caller
    /// authorized to replace legacy aliases in the live history table.
    pub fn backup_generations_partitioned_verified<P: AsRef<Path>>(
        path: P,
        identity: &StoragePartitionIdentity,
        generations: usize,
    ) -> Result<VerifiedPartitionBackup> {
        let path = path.as_ref();
        if generations == 0 {
            return Err(UsageStoreError::InvalidImport(
                "verified partition backup requires at least one generation".into(),
            ));
        }
        let source = Self::open_read_only_partitioned(path, identity)?;
        drop(source);
        // `backup_generations` validates the live source, creates one
        // SQLite-consistent copy, and quick-checks that copy before rotation.
        // Retained generations are cold recovery inputs: scanning every one
        // both before and after every daemon restart duplicates that proof and
        // can delay the recorder beyond its activation deadline. Validate a
        // retained generation when it is actually selected for recovery.
        Self::backup_generations(path, generations)?;
        let latest = path.with_extension("sqlite3.bak.1");
        let backup = Self::open_read_only_partitioned(&latest, identity)?;
        let (raw_rows, raw_fingerprint) = legacy_raw_evidence(&backup.connection)?;
        Ok(VerifiedPartitionBackup {
            database: path.to_owned(),
            partition_id: identity.partition_id.clone(),
            raw_rows,
            raw_fingerprint,
        })
    }

    /// Returns true only when this exact partition already has the current,
    /// internally consistent single-table canonical history. A legacy schema
    /// is a normal `false`; malformed or mismatched partitions remain errors.
    pub fn partition_history_is_current<P: AsRef<Path>>(
        path: P,
        identity: &StoragePartitionIdentity,
    ) -> Result<bool> {
        let path = path.as_ref();
        let store = Self::open_read_only_partitioned(path, identity)?;
        let version = account_db_schema_version(&store.connection)?;
        if version < HISTORY_CANONICAL_SCHEMA_VERSION {
            return Ok(false);
        }
        validate_canonical_history_storage(&store.connection)?;
        Ok(true)
    }

    /// Replaces legacy aliases in `usage_history` after a verified online
    /// backup. History and both existing sidecars move in one transaction.
    pub fn migrate_partition_history_after_verified_backup<P: AsRef<Path>>(
        path: P,
        identity: &StoragePartitionIdentity,
        backup: &VerifiedPartitionBackup,
    ) -> Result<bool> {
        let path = path.as_ref();
        if backup.database != path
            || backup.partition_id != identity.partition_id
            || !path.is_absolute()
        {
            return Err(UsageStoreError::InvalidImport(
                "verified backup does not belong to this partition".into(),
            ));
        }
        validate_partition_file(path)?;
        let probe = Self::open_read_only_partitioned(path, identity)?;
        let version = account_db_schema_version(&probe.connection)?;
        if version > HISTORY_CANONICAL_SCHEMA_VERSION {
            return Err(UsageStoreError::InvalidImport(
                "account partition schema is newer than this executable".into(),
            ));
        }
        if version == HISTORY_CANONICAL_SCHEMA_VERSION {
            validate_canonical_history_storage(&probe.connection)?;
            return Ok(false);
        }
        let source_evidence = legacy_raw_evidence(&probe.connection)?;
        if source_evidence != (backup.raw_rows, backup.raw_fingerprint.clone()) {
            return Err(UsageStoreError::InvalidImport(
                "partition changed after its verified backup".into(),
            ));
        }
        drop(probe);

        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_storage_partition_metadata(&transaction, identity)?;
        if legacy_raw_evidence(&transaction)? != source_evidence {
            return Err(UsageStoreError::InvalidImport(
                "partition raw history changed during migration admission".into(),
            ));
        }
        ensure_storage_partition_login_id_schema(&transaction)?;
        ensure_durable_state_schema(&transaction)?;
        ensure_recorder_gap_ledger_schema(&transaction)?;
        ensure_session_checkpoint_schema(&transaction)?;
        ensure_history_continuity_schema(&transaction)?;
        ensure_usage_model_history_schema(&transaction)?;
        ensure_session_cumulative_recovery_schema(&transaction)?;
        ensure_session_timeline_recovery_schema(&transaction)?;
        ensure_session_pending_range_schema(&transaction)?;
        ensure_session_event_schema(&transaction)?;
        ensure_session_task_event_schema(&transaction)?;
        ensure_active_thread_snapshot_schema(&transaction)?;
        let legacy_samples = load_legacy_samples_for_migration(&transaction)?;
        let canonical_rows = canonicalize_legacy_usage_history(&transaction, &legacy_samples)?;
        let canonical_samples = canonical_rows
            .iter()
            .map(|row| row.sample.clone())
            .collect::<Vec<_>>();
        let canonical_models = canonicalize_history_model_groups(
            load_history_model_groups(&transaction)?,
            &canonical_rows,
        )?;
        let canonical_observations =
            canonicalize_durable_history_observations(&transaction, &canonical_rows)?;
        rewrite_canonical_history(
            &transaction,
            &canonical_samples,
            &canonical_models,
            &canonical_observations,
        )?;
        ensure_canonical_history_constraints(&transaction)?;
        stamp_current_account_db_schema(&transaction)?;
        validate_storage_partition(&transaction, identity)?;
        transaction.commit()?;

        let read_back = Self::open_read_only_partitioned(path, identity)?;
        validate_canonical_history_storage(&read_back.connection)?;
        Ok(true)
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
        // temporary or generation file. Schema repair belongs to a separately
        // verified candidate, never to the source being protected.
        let source_store = Self::open_read_only(path)?;
        drop(source_store);
        let source = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
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

    /// Migrate a legacy, unpartitioned history database through a separately
    /// validated candidate database.
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
            let source_store = Self::open_read_only(path)?;
            let is_account_partition: bool = source_store.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master \
                 WHERE type = 'table' AND name = 'storage_partition')",
                [],
                |row| row.get(0),
            )?;
            if is_account_partition {
                return Err(UsageStoreError::InvalidImport(
                    "account partitions use versioned in-place schema upgrades".into(),
                ));
            }
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
        let mut samples = self.load_all_raw()?;
        let recoveries = self.load_session_cumulative_recoveries()?;
        for sample in &mut samples {
            apply_cumulative_recoveries_to_sample(
                sample,
                recoveries.iter().map(|(recovery, _, _)| recovery),
            )?;
        }
        Ok(samples)
    }

    fn load_all_raw(&self) -> Result<Vec<UsageHistorySample>> {
        load_valid_samples_from_table(&self.connection, "usage_history", None)
    }

    fn load_recent_history_raw(&self, now: DateTime<Utc>) -> Result<Vec<UsageHistorySample>> {
        let cutoff = one_month_before(now).timestamp();
        let now_timestamp = now.timestamp();
        let mut statement = self.connection.prepare(
            "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                    terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
             FROM usage_history
             WHERE timestamp > ?1 AND timestamp <= ?2
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
        self.load_recent_history(now)
    }

    /// Alias for the same bounded read, retaining the history terminology.
    pub fn load_recent_history(&self, now: DateTime<Utc>) -> Result<Vec<UsageHistorySample>> {
        let mut samples = self.load_recent_history_raw(now)?;
        let recoveries = self.load_session_cumulative_recoveries()?;
        for sample in &mut samples {
            apply_cumulative_recoveries_to_sample(
                sample,
                recoveries.iter().map(|(recovery, _, _)| recovery),
            )?;
        }
        Ok(samples)
    }

    /// Loads the bounded recent observation timeline. Existing rows without a
    /// sidecar provenance record are intentionally labelled `legacy-unknown`;
    /// auxiliary unavailable rows remain visible here while staying absent from
    /// the v1 `usage_history` projection.
    fn load_recent_observations_raw(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<UsageHistoryObservation>> {
        let cutoff = one_month_before(now).timestamp();
        let now_timestamp = now.timestamp();
        let mut observations = BTreeMap::<(i64, i64), UsageHistoryObservation>::new();
        for sample in self.load_recent_history_raw(now)? {
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
        let has_model_history: bool = self.connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'usage_model_history'
            )",
            [],
            |row| row.get(0),
        )?;
        if has_model_history {
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

    pub fn load_recent_observations(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<UsageHistoryObservation>> {
        let mut observations = self.load_recent_observations_raw(now)?;
        let recoveries = self.load_session_cumulative_recoveries()?;
        for observation in &mut observations {
            apply_cumulative_recoveries_to_observation(
                observation,
                recoveries.iter().map(|(recovery, _, _)| recovery),
            )?;
        }
        Ok(observations)
    }

    fn load_session_timeline_recoveries(
        &self,
    ) -> Result<Vec<(SessionTimelineRecovery, String, u64)>> {
        let present: bool = self.connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'session_timeline_recoveries'
            )",
            [],
            |row| row.get(0),
        )?;
        if !present {
            return Ok(Vec::new());
        }
        let partition_id: String = self.connection.query_row(
            "SELECT partition_id FROM storage_partition WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let generation: String = self.connection.query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let generation = canonical_u64_text(&generation, "collection generation")?;
        let mut statement = self.connection.prepare(
            "SELECT recovery_id, payload_json, applied_generation
             FROM session_timeline_recoveries ORDER BY applied_generation, recovery_id",
        )?;
        let mut rows = statement.query([])?;
        let mut recoveries = Vec::new();
        while let Some(row) = rows.next()? {
            let recovery_id: String = row.get(0)?;
            let payload_json: String = row.get(1)?;
            let applied_generation: String = row.get(2)?;
            let applied_generation =
                canonical_u64_text(&applied_generation, "timeline recovery generation")?;
            if applied_generation == 0 || applied_generation > generation {
                return Err(UsageStoreError::InvalidImport(
                    "timeline recovery generation is invalid".into(),
                ));
            }
            let (payload_partition, recovery) =
                timeline_recovery_from_payload(&recovery_id, &payload_json)?;
            if payload_partition != partition_id
                || recovery.source_data_generation.checked_add(1) != Some(applied_generation)
            {
                return Err(UsageStoreError::InvalidImport(
                    "timeline recovery partition or generation changed".into(),
                ));
            }
            recoveries.push((recovery, payload_json, applied_generation));
        }
        Ok(recoveries)
    }

    fn load_session_cumulative_recoveries(
        &self,
    ) -> Result<Vec<(SessionCumulativeRecovery, String, u64)>> {
        let present: bool = self.connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'session_cumulative_recoveries'
            )",
            [],
            |row| row.get(0),
        )?;
        if !present {
            return Ok(Vec::new());
        }
        let partition_id: String = self.connection.query_row(
            "SELECT partition_id FROM storage_partition WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let generation: String = self.connection.query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let generation = canonical_u64_text(&generation, "collection generation")?;
        let mut statement = self.connection.prepare(
            "SELECT recovery_id, payload_json, applied_generation
             FROM session_cumulative_recoveries ORDER BY recovery_id",
        )?;
        let mut rows = statement.query([])?;
        let mut recoveries = Vec::new();
        while let Some(row) = rows.next()? {
            let recovery_id: String = row.get(0)?;
            let payload_json: String = row.get(1)?;
            let applied_generation: String = row.get(2)?;
            let applied_generation =
                canonical_u64_text(&applied_generation, "cumulative recovery generation")?;
            if applied_generation == 0 || applied_generation > generation {
                return Err(UsageStoreError::InvalidImport(
                    "cumulative recovery generation is invalid".into(),
                ));
            }
            let (payload_partition, recovery) =
                cumulative_recovery_from_payload(&recovery_id, &payload_json)?;
            if payload_partition != partition_id {
                return Err(UsageStoreError::InvalidImport(
                    "cumulative recovery partition changed".into(),
                ));
            }
            recoveries.push((recovery, payload_json, applied_generation));
        }
        Ok(recoveries)
    }

    /// Returns a correction only when the latest durable vector exactly
    /// matches a monotonic raw suffix after a reset-alias regression. Applied
    /// recovery IDs are filtered before any Session scan is requested.
    pub fn pending_session_cumulative_recovery(
        &self,
        canonical_reset_at: i64,
        window_seconds: i64,
        now: i64,
        current_model_totals: &[SessionModelTotal],
    ) -> Result<Option<SessionCumulativeRecovery>> {
        let partition_id: String = self.connection.query_row(
            "SELECT partition_id FROM storage_partition WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let Some(now) = DateTime::<Utc>::from_timestamp(now, 0) else {
            return Ok(None);
        };
        let source_state = self.load_session_collection_state()?;
        let mut observations = self.load_recent_observations_raw(now)?;
        let stored_recoveries = self.load_session_cumulative_recoveries()?;
        let timeline_recoveries = self.load_session_timeline_recoveries()?;
        for observation in &mut observations {
            apply_cumulative_recoveries_to_observation(
                observation,
                stored_recoveries.iter().map(|(recovery, _, _)| recovery),
            )?;
            apply_timeline_recoveries_to_observation(
                observation,
                timeline_recoveries.iter().map(|(recovery, _, _)| recovery),
            )?;
        }
        if source_state.reset_at != canonical_reset_at
            || source_state.window_seconds != window_seconds
        {
            let Some(source_observation) = source_state.last_quota_observation.as_ref() else {
                return Ok(None);
            };
            let canonical_observation = observations
                .iter()
                .filter(|observation| {
                    same_reset_group(observation.reset_at, canonical_reset_at)
                        && observation.remaining_percent.is_some_and(|value| {
                            value.is_finite() && (0.0..=100.0).contains(&value)
                        })
                })
                .max_by_key(|observation| observation.timestamp);
            let Some(canonical_observation) = canonical_observation else {
                return Ok(None);
            };
            let transition = classify_quota_transition(
                Some(canonical_reset_at),
                window_seconds,
                Some(canonical_observation.timestamp),
                canonical_observation.remaining_percent,
                source_state.reset_at,
                source_state.window_seconds,
                Some(source_observation.remaining_percent),
                source_observation.observed_at,
            );
            if canonical_reset_at <= source_observation.observed_at
                || !matches!(
                    transition,
                    QuotaTransition::SamePeriod | QuotaTransition::Rejected
                )
            {
                return Ok(None);
            }
            let endpoint_timestamp = observations
                .iter()
                .filter(|observation| {
                    same_reset_group(observation.reset_at, canonical_reset_at)
                        && observation.model_totals.as_deref() == Some(current_model_totals)
                })
                .map(|observation| observation.timestamp)
                .max();
            let Some(endpoint_timestamp) = endpoint_timestamp else {
                return Ok(None);
            };
            if observations.iter().any(|observation| {
                observation.timestamp > endpoint_timestamp
                    && !same_reset_group(observation.reset_at, source_state.reset_at)
            }) {
                return Ok(None);
            }
            observations.retain(|observation| observation.timestamp <= endpoint_timestamp);
        }
        let candidate = derive_session_cumulative_recovery(
            &partition_id,
            canonical_reset_at,
            window_seconds,
            now.timestamp(),
            current_model_totals,
            &observations,
        )?;
        let Some(mut candidate) = candidate else {
            return Ok(None);
        };
        let source_observation = source_state
            .last_quota_observation
            .as_ref()
            .ok_or_else(|| {
                UsageStoreError::InvalidImport(
                    "cumulative recovery source has no quota observation".into(),
                )
            })?;
        candidate.source_generation = Some(SessionCumulativeRecoverySource {
            data_generation: source_state.data_generation,
            reset_at: source_state.reset_at,
            window_seconds: source_state.window_seconds,
            observed_at: source_observation.observed_at,
            remaining_percent: source_observation.remaining_percent,
            model_totals: source_state.model_totals,
        });
        let payload = cumulative_recovery_payload(&partition_id, &candidate)?;
        if let Some((_, stored_payload, _)) = stored_recoveries
            .into_iter()
            .find(|(stored, _, _)| stored.recovery_id == candidate.recovery_id)
        {
            if stored_payload != payload {
                return Err(UsageStoreError::InvalidImport(
                    "cumulative recovery replay conflicts with its marker".into(),
                ));
            }
            return Ok(None);
        }
        Ok(Some(candidate))
    }

    /// Returns the latest unambiguous raw model vector for one reset group.
    /// This is used only to reconnect a retained post-regression checkpoint
    /// to immutable history; it never rewrites or max-clamps a counter.
    pub fn latest_raw_session_model_totals_for_period(
        &self,
        canonical_reset_at: i64,
        now: i64,
    ) -> Result<Option<Vec<SessionModelTotal>>> {
        let Some(now) = DateTime::<Utc>::from_timestamp(now, 0) else {
            return Ok(None);
        };
        let observations = self.load_recent_observations_raw(now)?;
        let Some(latest_timestamp) = observations
            .iter()
            .filter(|observation| {
                same_reset_group(observation.reset_at, canonical_reset_at)
                    && observation
                        .model_totals
                        .as_ref()
                        .is_some_and(|rows| !rows.is_empty())
            })
            .map(|observation| observation.timestamp)
            .max()
        else {
            return Ok(None);
        };
        let mut candidates = observations
            .iter()
            .filter(|observation| {
                observation.timestamp == latest_timestamp
                    && same_reset_group(observation.reset_at, canonical_reset_at)
            })
            .filter_map(|observation| observation.model_totals.as_deref())
            .map(canonicalize_model_totals)
            .collect::<Result<Vec<_>>>()?;
        let Some(first) = candidates.pop() else {
            return Ok(None);
        };
        if first.is_empty() || candidates.iter().any(|candidate| candidate != &first) {
            return Ok(None);
        }
        Ok(Some(first))
    }

    /// Returns the latest retained quota observation in one reset group.
    /// Rejected aliases in other groups remain immutable but cannot become
    /// this period's observation authority.
    pub fn latest_raw_quota_observation_for_period(
        &self,
        canonical_reset_at: i64,
    ) -> Result<Option<SessionQuotaObservation>> {
        last_quota_observation_for_reset(&self.connection, canonical_reset_at)
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
        let historical = canonicalize_samples(&transaction, &historical, false, None)?;
        upsert_canonical_samples(&transaction, &historical, false, None)?;
        let adjusted_current = apply_history_continuity(&transaction, &current_samples)?;
        let adjusted_current = canonicalize_samples(&transaction, &adjusted_current, false, None)?;
        upsert_canonical_samples(&transaction, &adjusted_current, false, None)?;
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
        let canonical = canonicalize_samples(&transaction, &adjusted, false, None)?;
        upsert_canonical_samples(&transaction, &canonical, false, None)?;
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
        let sources = canonicalize_recorded_sessions_for_commit(sources)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let adjusted = apply_history_continuity(&transaction, samples)?;
        let canonical = canonicalize_samples(&transaction, &adjusted, false, None)?;
        upsert_canonical_samples(&transaction, &canonical, false, None)?;
        replace_recorded_session_markers(&transaction, &sources)?;
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
        // The accepted reset in collection_generation is the period
        // authority. Rows retained from a rejected reset alias are immutable
        // evidence, not candidates for the next durable observation after a
        // restart or recovery.
        let last_quota_observation = last_quota_observation_for_reset(&transaction, reset_at)?;
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

    /// Read the exact source ranges which still require parser attribution.
    /// Rows are keyed by source lineage and start offset, so replaying a
    /// process cannot manufacture a second usage range for the same bytes.
    pub fn load_session_pending_ranges(&self) -> Result<Vec<SessionPendingRange>> {
        let mut statement = self.connection.prepare(
            "SELECT root_identity, relative_path, file_device, file_inode,
                    start_offset, end_offset, collector_epoch, cycle_seq,
                    prefix_generation, record_sha256, parser_version, reason, complete
             FROM session_pending_ranges
             ORDER BY root_identity, relative_path, file_device, file_inode,
                      prefix_generation, start_offset",
        )?;
        let rows = statement
            .query_map([], |row| {
                let start_offset = row.get::<_, i64>(4)?;
                let end_offset = row.get::<_, i64>(5)?;
                let collector_epoch = row.get::<_, String>(6)?;
                let cycle_seq = row.get::<_, String>(7)?;
                let prefix_generation = row.get::<_, String>(8)?;
                Ok(SessionPendingRange {
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
                    start_offset: u64::try_from(start_offset)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    end_offset: u64::try_from(end_offset)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    collector_epoch: u128::from_str_radix(&collector_epoch, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    cycle_seq: cycle_seq
                        .parse()
                        .ok()
                        .filter(|value: &u64| value.to_string() == cycle_seq)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    prefix_generation: u128::from_str_radix(&prefix_generation, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    record_sha256: row.get(9)?,
                    parser_version: row.get(10)?,
                    reason: row.get(11)?,
                    complete: row.get::<_, i64>(12)? == 1,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for pending in &rows {
            validate_session_pending_range(pending)?;
        }
        Ok(rows)
    }

    /// Read the accepted source ranges for commit acknowledgement.  A
    /// recorder must verify that every range it submitted is durable before
    /// acknowledging the corresponding source checkpoint.
    pub fn load_session_ranges(&self) -> Result<Vec<SessionRange>> {
        let mut statement = self.connection.prepare(
            "SELECT root_identity, relative_path, file_device, file_inode,
                    start_offset, end_offset, collector_epoch, cycle_seq,
                    prefix_generation, record_sha256
             FROM session_ranges
             ORDER BY root_identity, relative_path, file_device, file_inode,
                      prefix_generation, start_offset, end_offset, record_sha256",
        )?;
        let rows = statement
            .query_map([], |row| {
                let start_offset = row.get::<_, i64>(4)?;
                let end_offset = row.get::<_, i64>(5)?;
                let collector_epoch = row.get::<_, String>(6)?;
                let cycle_seq = row.get::<_, String>(7)?;
                let prefix_generation = row.get::<_, String>(8)?;
                Ok(SessionRange {
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
                    start_offset: u64::try_from(start_offset)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    end_offset: u64::try_from(end_offset)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    collector_epoch: u128::from_str_radix(&collector_epoch, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    cycle_seq: cycle_seq
                        .parse()
                        .ok()
                        .filter(|value: &u64| value.to_string() == cycle_seq)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    prefix_generation: u128::from_str_radix(&prefix_generation, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    record_sha256: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for range in &rows {
            validate_session_range(range)?;
        }
        Ok(rows)
    }

    /// Verify only the bounded evidence submitted by one recorder commit.
    /// This deliberately uses primary-key lookups instead of loading the
    /// complete range/event/pending ledgers on every sixty-second cycle.
    pub fn verify_session_collection_batch(
        &self,
        ranges: &[SessionRange],
        events: &[SessionEvent],
        pending_ranges: &[SessionPendingRange],
    ) -> Result<bool> {
        for range in ranges {
            validate_session_range(range)?;
            let exists: i64 = self.connection.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM session_ranges
                    WHERE root_identity=?1 AND relative_path=?2
                      AND file_device=?3 AND file_inode=?4
                      AND prefix_generation=?5 AND start_offset=?6
                      AND end_offset=?7 AND record_sha256=?8
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
            if exists == 0 {
                return Ok(false);
            }
        }
        for event in events {
            validate_session_event(event)?;
            let stored: Option<(String, String, String, String, String, Option<String>)> = self
                .connection
                .query_row(
                    "SELECT model, total_tokens, input_tokens, cached_input_tokens,
                                output_tokens, cache_write_input_tokens
                         FROM session_events
                         WHERE root_identity=?1 AND relative_path=?2
                           AND file_device=?3 AND file_inode=?4
                           AND prefix_generation=?5 AND range_start=?6
                           AND range_end=?7 AND record_sha256=?8 AND event_index=?9",
                    params![
                        &event.root_identity,
                        &event.relative_path,
                        event.file_device.to_string(),
                        event.file_inode.to_string(),
                        format!("{:032x}", event.prefix_generation),
                        event.range_start as i64,
                        event.range_end as i64,
                        &event.record_sha256,
                        event.event_index as i64,
                    ],
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
                )
                .optional()?;
            let expected = Some((
                event.model.clone(),
                event.total_tokens.to_string(),
                event.input_tokens.to_string(),
                event.cached_input_tokens.to_string(),
                event.output_tokens.to_string(),
                event
                    .cache_write_input_tokens
                    .map(|value| value.to_string()),
            ));
            if stored != expected {
                return Ok(false);
            }
        }
        for pending in pending_ranges {
            validate_session_pending_range(pending)?;
            let stored: Option<(i64, String, String, String, String, i64)> = self
                .connection
                .query_row(
                    "SELECT end_offset, collector_epoch, record_sha256,
                            parser_version, reason, complete
                     FROM session_pending_ranges
                     WHERE root_identity=?1 AND relative_path=?2
                       AND file_device=?3 AND file_inode=?4
                       AND prefix_generation=?5 AND start_offset=?6",
                    params![
                        &pending.root_identity,
                        &pending.relative_path,
                        pending.file_device.to_string(),
                        pending.file_inode.to_string(),
                        format!("{:032x}", pending.prefix_generation),
                        pending.start_offset as i64,
                    ],
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
                )
                .optional()?;
            let expected = Some((
                pending.end_offset as i64,
                format!("{:032x}", pending.collector_epoch),
                pending.record_sha256.clone(),
                pending.parser_version.clone(),
                pending.reason.clone(),
                i64::from(pending.complete),
            ));
            if stored != expected {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn verify_session_task_batch(
        &self,
        indexed_ranges: &[SessionTaskIndexedRange],
        events: &[SessionTaskEvent],
    ) -> Result<bool> {
        for range in indexed_ranges {
            validate_session_task_indexed_range(range)?;
            let exists: i64 = self.connection.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM session_task_indexed_ranges
                    WHERE root_identity=?1 AND relative_path=?2
                      AND file_device=?3 AND file_inode=?4
                      AND prefix_generation=?5 AND start_offset=?6
                      AND end_offset=?7 AND record_sha256=?8
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
            if exists == 0 {
                return Ok(false);
            }
        }
        for event in events {
            validate_session_task_event(event)?;
            let stored: Option<(i64, i64)> = self
                .connection
                .query_row(
                    "SELECT timestamp, running
                     FROM session_task_events
                     WHERE root_identity=?1 AND relative_path=?2
                       AND file_device=?3 AND file_inode=?4
                       AND prefix_generation=?5 AND start_offset=?6
                       AND end_offset=?7 AND record_sha256=?8 AND event_index=?9",
                    params![
                        &event.root_identity,
                        &event.relative_path,
                        event.file_device.to_string(),
                        event.file_inode.to_string(),
                        format!("{:032x}", event.prefix_generation),
                        event.start_offset as i64,
                        event.end_offset as i64,
                        &event.record_sha256,
                        event.event_index as i64,
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if stored != Some((event.timestamp, i64::from(event.running))) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Return pending-evidence visibility without materializing the ledger.
    pub fn session_pending_range_count(&self) -> Result<usize> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM session_pending_ranges",
            [],
            |row| row.get(0),
        )?;
        usize::try_from(count).map_err(|_| {
            UsageStoreError::InvalidImport(
                "session pending range count exceeds memory limits".into(),
            )
        })
    }

    /// Read source-proven token deltas independently of quota period
    /// materialization. This bounded projection is used only during an
    /// accepted period boundary to reconstruct the current window.
    pub fn load_session_events(&self) -> Result<Vec<SessionEvent>> {
        let mut statement = self.connection.prepare(
            "SELECT root_identity, relative_path, file_device, file_inode,
                    prefix_generation, range_start, range_end, record_sha256,
                    event_index, timestamp, model, total_tokens, input_tokens,
                    cached_input_tokens, output_tokens, cache_write_input_tokens
             FROM session_events
             ORDER BY timestamp, root_identity, relative_path, range_start, event_index",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(SessionEvent {
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
                    prefix_generation: u128::from_str_radix(&row.get::<_, String>(4)?, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    range_start: u64::try_from(row.get::<_, i64>(5)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    range_end: u64::try_from(row.get::<_, i64>(6)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    record_sha256: row.get(7)?,
                    event_index: u64::try_from(row.get::<_, i64>(8)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    timestamp: row.get(9)?,
                    model: row.get(10)?,
                    total_tokens: row
                        .get::<_, String>(11)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    input_tokens: row
                        .get::<_, String>(12)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    cached_input_tokens: row
                        .get::<_, String>(13)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    output_tokens: row
                        .get::<_, String>(14)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    cache_write_input_tokens: row
                        .get::<_, Option<String>>(15)?
                        .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
                        .transpose()?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for event in &rows {
            validate_session_event(event)?;
        }
        Ok(rows)
    }

    /// Load the source-proven task lifecycle evidence and its fail-closed
    /// coverage verdict. The coverage check is intentionally independent of
    /// the event rows: an empty event list is valid only when every inspected
    /// span is itself durably marked.
    pub fn load_session_task_evidence(&self) -> Result<SessionTaskEvidence> {
        let events = self.load_session_task_events()?;
        let indexed_ranges = self.load_session_task_indexed_ranges()?;
        let all_ranges_indexed = self.session_task_coverage_complete()?;
        Ok(SessionTaskEvidence {
            events,
            indexed_ranges,
            all_ranges_indexed,
        })
    }

    pub fn load_session_task_events(&self) -> Result<Vec<SessionTaskEvent>> {
        if !self.session_task_tables_present()? {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT root_identity, relative_path, file_device, file_inode,
                    prefix_generation, start_offset, end_offset, record_sha256,
                    event_index, timestamp, running
             FROM session_task_events
             ORDER BY timestamp, root_identity, relative_path, start_offset, event_index",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(SessionTaskEvent {
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
                    prefix_generation: u128::from_str_radix(&row.get::<_, String>(4)?, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    start_offset: u64::try_from(row.get::<_, i64>(5)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    end_offset: u64::try_from(row.get::<_, i64>(6)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    record_sha256: row.get(7)?,
                    event_index: u64::try_from(row.get::<_, i64>(8)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    timestamp: row.get(9)?,
                    running: match row.get::<_, i64>(10)? {
                        0 => false,
                        1 => true,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for event in &rows {
            validate_session_task_event(event)?;
        }
        Ok(rows)
    }

    pub fn load_session_task_indexed_ranges(&self) -> Result<Vec<SessionTaskIndexedRange>> {
        if !self.session_task_tables_present()? {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT root_identity, relative_path, file_device, file_inode,
                    start_offset, end_offset, collector_epoch, cycle_seq,
                    prefix_generation, record_sha256
             FROM session_task_indexed_ranges
             ORDER BY root_identity, relative_path, file_device, file_inode,
                      prefix_generation, start_offset, end_offset, record_sha256",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(SessionTaskIndexedRange {
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
                    start_offset: u64::try_from(row.get::<_, i64>(4)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    end_offset: u64::try_from(row.get::<_, i64>(5)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    collector_epoch: u128::from_str_radix(&row.get::<_, String>(6)?, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    cycle_seq: row
                        .get::<_, String>(7)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    prefix_generation: u128::from_str_radix(&row.get::<_, String>(8)?, 16)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    record_sha256: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for range in &rows {
            validate_session_task_indexed_range(range)?;
        }
        Ok(rows)
    }

    pub fn session_task_coverage_complete(&self) -> Result<bool> {
        if !self.session_task_tables_present()? {
            return Ok(false);
        }
        let indexed_ranges = self.load_session_task_indexed_ranges()?;
        let mut checkpoint_statement = self.connection.prepare(
            "SELECT root_identity, relative_path, file_device, file_inode,
                    committed_offset, prefix_generation
             FROM session_checkpoints
             ORDER BY root_identity, relative_path, file_device, file_inode, prefix_generation",
        )?;
        let checkpoints = checkpoint_statement
            .query_map([], |row| {
                let committed_offset = row.get::<_, i64>(4)?;
                let prefix_generation = u128::from_str_radix(&row.get::<_, String>(5)?, 16)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    u64::try_from(committed_offset).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    prefix_generation,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (root, path, device, inode, end, prefix_generation) in checkpoints {
            if end == 0 {
                continue;
            }
            let mut spans = indexed_ranges
                .iter()
                .filter(|range| {
                    range.root_identity == root
                        && range.relative_path == path
                        && range.file_device.to_string() == device
                        && range.file_inode.to_string() == inode
                        && range.prefix_generation == prefix_generation
                })
                .collect::<Vec<_>>();
            spans.sort_by_key(|range| (range.start_offset, range.end_offset));
            let mut covered = 0_u64;
            for span in spans {
                if span.start_offset > covered {
                    break;
                }
                covered = covered.max(span.end_offset);
                if covered >= end {
                    break;
                }
            }
            if covered < end {
                return Ok(false);
            }
        }

        for session_range in self.load_session_ranges()? {
            // A task-indexed super-range authenticates and parses the same
            // immutable source bytes. Its digest covers a different span and
            // therefore must not be compared with the sub-range digest.
            let contained = indexed_ranges.iter().any(|indexed| {
                indexed.root_identity == session_range.root_identity
                    && indexed.relative_path == session_range.relative_path
                    && indexed.file_device == session_range.file_device
                    && indexed.file_inode == session_range.file_inode
                    && indexed.prefix_generation == session_range.prefix_generation
                    && indexed.start_offset <= session_range.start_offset
                    && indexed.end_offset >= session_range.end_offset
            });
            if !contained {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn session_task_tables_present(&self) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT COUNT(*) = 2 FROM sqlite_schema
                 WHERE type='table' AND name IN ('session_task_events', 'session_task_indexed_ranges')",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
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
    #[allow(clippy::too_many_arguments)]
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
        self.commit_session_collection_with_observations_inner(
            commit,
            &observations,
            CollectionEvidence {
                cumulative_recovery: None,
                timeline_recovery: None,
                pending_ranges: None,
                events: None,
                task_events: None,
                task_indexed_ranges: None,
            },
        )
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
        self.commit_session_collection_with_observations_inner(
            commit,
            observations,
            CollectionEvidence {
                cumulative_recovery: None,
                timeline_recovery: None,
                pending_ranges: None,
                events: None,
                task_events: None,
                task_indexed_ranges: None,
            },
        )
    }

    /// Commits an ordinary recorder generation and one source-proven
    /// cumulative correction under the same SQLite transaction. Existing
    /// history rows are not rewritten; the recovery marker controls their
    /// bounded read projection.
    pub fn commit_session_collection_with_cumulative_recovery(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
        recovery: &SessionCumulativeRecovery,
    ) -> Result<SessionCollectionCommitResult> {
        self.commit_session_collection_with_observations_inner(
            commit,
            observations,
            CollectionEvidence {
                cumulative_recovery: Some(recovery),
                timeline_recovery: None,
                pending_ranges: None,
                events: None,
                task_events: None,
                task_indexed_ranges: None,
            },
        )
    }

    /// Commits exact Session byte ranges together with their reconstructed
    /// minute deltas. The marker, ranges, checkpoint, corrected current total,
    /// and generation share one transaction.
    pub fn commit_session_collection_with_timeline_recovery(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
        recovery: &SessionTimelineRecovery,
    ) -> Result<SessionCollectionCommitResult> {
        self.commit_session_collection_with_observations_inner(
            commit,
            observations,
            CollectionEvidence {
                cumulative_recovery: None,
                timeline_recovery: Some(recovery),
                pending_ranges: None,
                events: None,
                task_events: None,
                task_indexed_ranges: None,
            },
        )
    }

    /// Commits a collection generation and the exact source bytes that could
    /// not yet be attributed. Pending evidence shares the collection
    /// transaction, so a failed write cannot advance either checkpoint or
    /// pending acknowledgement independently.
    pub fn commit_session_collection_with_pending_ranges(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
        pending_ranges: &[SessionPendingRange],
    ) -> Result<SessionCollectionCommitResult> {
        self.commit_session_collection_with_observations_inner(
            commit,
            observations,
            CollectionEvidence {
                cumulative_recovery: None,
                timeline_recovery: None,
                pending_ranges: Some(pending_ranges),
                events: None,
                task_events: None,
                task_indexed_ranges: None,
            },
        )
    }

    /// Timeline-recovery variant of
    /// [`Self::commit_session_collection_with_pending_ranges`].
    pub fn commit_session_collection_with_timeline_recovery_and_pending_ranges(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
        recovery: &SessionTimelineRecovery,
        pending_ranges: &[SessionPendingRange],
    ) -> Result<SessionCollectionCommitResult> {
        self.commit_session_collection_with_observations_inner(
            commit,
            observations,
            CollectionEvidence {
                cumulative_recovery: None,
                timeline_recovery: Some(recovery),
                pending_ranges: Some(pending_ranges),
                events: None,
                task_events: None,
                task_indexed_ranges: None,
            },
        )
    }

    /// Commits source-proven Session deltas and unresolved ranges in one
    /// generation. Events remain independent of quota/reset authority so a
    /// later accepted boundary can reconstruct its period without replaying
    /// already checkpointed source bytes.
    pub fn commit_session_collection_with_events_and_pending_ranges(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
        events: &[SessionEvent],
        pending_ranges: &[SessionPendingRange],
    ) -> Result<SessionCollectionCommitResult> {
        self.commit_session_collection_with_observations_inner(
            commit,
            observations,
            CollectionEvidence {
                cumulative_recovery: None,
                timeline_recovery: None,
                pending_ranges: Some(pending_ranges),
                events: Some(events),
                task_events: None,
                task_indexed_ranges: None,
            },
        )
    }

    /// Timeline-recovery/event variant used by the independent recorder.
    pub fn commit_session_collection_with_timeline_recovery_events_and_pending_ranges(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
        recovery: &SessionTimelineRecovery,
        events: &[SessionEvent],
        pending_ranges: &[SessionPendingRange],
    ) -> Result<SessionCollectionCommitResult> {
        self.commit_session_collection_with_observations_inner(
            commit,
            observations,
            CollectionEvidence {
                cumulative_recovery: None,
                timeline_recovery: Some(recovery),
                pending_ranges: Some(pending_ranges),
                events: Some(events),
                task_events: None,
                task_indexed_ranges: None,
            },
        )
    }

    /// Commits Session token evidence and task lifecycle evidence together.
    /// The indexed spans and task events are written by the same transaction
    /// that advances the recorder generation, so an acknowledgement cannot
    /// expose only one half of a source inspection.
    pub fn commit_session_collection_with_task_evidence(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
        evidence: SessionTaskEvidenceInput<'_>,
    ) -> Result<SessionCollectionCommitResult> {
        self.commit_session_collection_with_observations_inner(
            commit,
            observations,
            CollectionEvidence {
                cumulative_recovery: None,
                timeline_recovery: None,
                pending_ranges: Some(evidence.pending_ranges),
                events: Some(evidence.events),
                task_events: Some(evidence.task_events),
                task_indexed_ranges: Some(evidence.task_indexed_ranges),
            },
        )
    }

    pub fn commit_session_collection_with_timeline_recovery_and_task_evidence(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
        recovery: &SessionTimelineRecovery,
        evidence: SessionTaskEvidenceInput<'_>,
    ) -> Result<SessionCollectionCommitResult> {
        self.commit_session_collection_with_observations_inner(
            commit,
            observations,
            CollectionEvidence {
                cumulative_recovery: None,
                timeline_recovery: Some(recovery),
                pending_ranges: Some(evidence.pending_ranges),
                events: Some(evidence.events),
                task_events: Some(evidence.task_events),
                task_indexed_ranges: Some(evidence.task_indexed_ranges),
            },
        )
    }

    fn commit_session_collection_with_observations_inner(
        &mut self,
        commit: SessionCollectionCommit<'_>,
        observations: &[UsageHistoryObservation],
        evidence: CollectionEvidence<'_>,
    ) -> Result<SessionCollectionCommitResult> {
        let CollectionEvidence {
            cumulative_recovery,
            timeline_recovery,
            pending_ranges,
            events,
            task_events,
            task_indexed_ranges,
        } = evidence;
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
        if reset_at < 0
            || window_seconds < 0
            || (reset_at == 0) != (window_seconds == 0)
            || collector_epoch == 0
            || cycle_seq == 0
        {
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
        let replace_incomplete_pending_ranges = pending_ranges.is_some();
        let mut canonical_pending_ranges = BTreeMap::new();
        for pending in pending_ranges.unwrap_or(&[]) {
            validate_session_pending_range(pending)?;
            if pending.collector_epoch != collector_epoch || pending.cycle_seq != cycle_seq {
                return Err(UsageStoreError::InvalidImport(
                    "pending range admission generation mismatch".into(),
                ));
            }
            let key = (
                pending.root_identity.clone(),
                pending.relative_path.clone(),
                pending.file_device,
                pending.file_inode,
                pending.prefix_generation,
                pending.start_offset,
            );
            if canonical_pending_ranges
                .insert(key, pending.clone())
                .is_some()
            {
                return Err(UsageStoreError::InvalidImport(
                    "duplicate session pending range".into(),
                ));
            }
        }
        let mut canonical_events = BTreeMap::new();
        for event in events.unwrap_or(&[]) {
            validate_session_event(event)?;
            let matching_range = canonical_ranges.values().any(|range| {
                range.root_identity == event.root_identity
                    && range.relative_path == event.relative_path
                    && range.file_device == event.file_device
                    && range.file_inode == event.file_inode
                    && range.prefix_generation == event.prefix_generation
                    && range.start_offset == event.range_start
                    && range.end_offset == event.range_end
                    && range.record_sha256 == event.record_sha256
            });
            if !matching_range {
                return Err(UsageStoreError::InvalidImport(
                    "session event has no committed source range".into(),
                ));
            }
            let key = (
                event.root_identity.clone(),
                event.relative_path.clone(),
                event.file_device,
                event.file_inode,
                event.prefix_generation,
                event.range_start,
                event.range_end,
                event.record_sha256.clone(),
                event.event_index,
            );
            if canonical_events.insert(key, event.clone()).is_some() {
                return Err(UsageStoreError::InvalidImport(
                    "duplicate session event".into(),
                ));
            }
        }
        let (canonical_task_indexed_ranges, canonical_task_events) =
            canonicalize_task_evidence(task_events, task_indexed_ranges)?;
        let model_totals = canonicalize_model_totals(model_totals)?;
        let recorded_sessions = canonicalize_recorded_sessions_for_commit(recorded_sessions)?;
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

        // A timeline recovery is an immutable correction for the historical
        // prefix ending at `projection_end_exclusive`. Later recorder cycles
        // may replay the same Session events while materializing minutes that
        // were absent during the outage. Existing raw rows in that proven
        // prefix remain the original evidence; only previously absent minutes
        // may be inserted. Current and future minutes still use the ordinary
        // reconciliation path.
        let preserve_existing_before = self
            .load_session_timeline_recoveries()?
            .into_iter()
            .map(|(recovery, _, _)| recovery.projection_end_exclusive)
            .chain(
                timeline_recovery
                    .iter()
                    .map(|recovery| recovery.projection_end_exclusive),
            )
            .max();

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let adjusted_samples = apply_history_continuity(&transaction, samples)?;
        let preserve_all_existing_history = cumulative_recovery.is_some();
        let canonical_projection = canonicalize_samples_with_sources(
            &transaction,
            &adjusted_samples,
            cumulative_recovery.is_some(),
            preserve_existing_before,
        )?;
        let canonical_samples = canonical_projection.rows;
        let canonical_observations = canonicalize_observations(
            &transaction,
            observations,
            &canonical_samples,
            &canonical_projection.source_to_canonical,
        )?;
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
        let next = current_data_generation
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        if cumulative_recovery.is_some() && timeline_recovery.is_some() {
            return Err(UsageStoreError::InvalidImport(
                "independent recovery classes must commit in separate generations".into(),
            ));
        }
        let cumulative_payload = if let Some(recovery) = cumulative_recovery {
            if recovery.canonical_reset_at != reset_at || recovery.window_seconds != window_seconds
            {
                return Err(UsageStoreError::InvalidImport(
                    "cumulative recovery period changed".into(),
                ));
            }
            let partition_id: String = transaction.query_row(
                "SELECT partition_id FROM storage_partition WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            let payload = validate_cumulative_recovery(&partition_id, recovery)?;
            let reconciled_source = if let Some(source) = recovery.source_generation.as_ref() {
                reconcile_rejected_generation_model_totals(
                    &recovery.source_current_model_totals,
                    &source.model_totals,
                )?
                .ok_or_else(|| {
                    UsageStoreError::InvalidImport(
                        "cumulative recovery source vectors are ambiguous".into(),
                    )
                })?
            } else {
                recovery.source_current_model_totals.clone()
            };
            let corrected_source =
                checked_add_model_totals(&reconciled_source, &recovery.offset_model_totals)
                    .ok_or(UsageStoreError::GenerationOverflow)?;
            if !model_totals_dominate(&model_totals, &corrected_source) {
                return Err(UsageStoreError::InvalidImport(
                    "cumulative recovery is not present in the committed totals".into(),
                ));
            }
            let existing: Option<(String, String)> = transaction
                .query_row(
                    "SELECT payload_json, applied_generation
                     FROM session_cumulative_recoveries WHERE recovery_id=?1",
                    [&recovery.recovery_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((stored_payload, applied_generation)) = existing {
                let applied_generation =
                    canonical_u64_text(&applied_generation, "cumulative recovery generation")?;
                if stored_payload != payload {
                    return Err(UsageStoreError::InvalidImport(
                        "cumulative recovery replay conflicts with its marker".into(),
                    ));
                }
                if current_epoch != Some(collector_epoch)
                    || current_cycle_seq != cycle_seq
                    || applied_generation != current_data_generation
                {
                    return Err(UsageStoreError::InvalidImport(
                        "cumulative recovery was already applied".into(),
                    ));
                }
                Some((payload, true))
            } else {
                validate_cumulative_recovery_storage_evidence(&transaction, recovery)?;
                let source = recovery.source_generation.as_ref().ok_or_else(|| {
                    UsageStoreError::InvalidImport(
                        "cumulative recovery source generation is unbound".into(),
                    )
                })?;
                validate_cumulative_recovery_source_generation(&transaction, recovery, source)?;
                Some((payload, false))
            }
        } else {
            None
        };
        let timeline_payload = if let Some(recovery) = timeline_recovery {
            if recovery.canonical_reset_at != reset_at || recovery.window_seconds != window_seconds
            {
                return Err(UsageStoreError::InvalidImport(
                    "timeline recovery source generation changed".into(),
                ));
            }
            let partition_id: String = transaction.query_row(
                "SELECT partition_id FROM storage_partition WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            let payload = validate_timeline_recovery(&partition_id, recovery)?;
            let mut committed_ranges = canonical_ranges.values().cloned().collect::<Vec<_>>();
            committed_ranges.sort();
            if recovery.ranges != committed_ranges
                || recovery.ranges.iter().any(|range| {
                    range.collector_epoch != collector_epoch || range.cycle_seq != cycle_seq
                })
            {
                return Err(UsageStoreError::InvalidImport(
                    "timeline recovery ranges differ from the committed source".into(),
                ));
            }
            let expected_totals = checked_add_model_totals(
                &recovery.source_model_totals,
                &recovery.final_offset_model_totals,
            )
            .ok_or(UsageStoreError::GenerationOverflow)?;
            if expected_totals != model_totals {
                return Err(UsageStoreError::InvalidImport(
                    "timeline recovery does not equal the committed model totals".into(),
                ));
            }
            let existing: Option<(String, String)> = transaction
                .query_row(
                    "SELECT payload_json, applied_generation
                     FROM session_timeline_recoveries WHERE recovery_id=?1",
                    [&recovery.recovery_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((stored_payload, applied_generation)) = existing {
                let applied_generation =
                    canonical_u64_text(&applied_generation, "timeline recovery generation")?;
                if stored_payload != payload {
                    return Err(UsageStoreError::InvalidImport(
                        "timeline recovery replay conflicts with its marker".into(),
                    ));
                }
                if current_epoch != Some(collector_epoch)
                    || current_cycle_seq != cycle_seq
                    || applied_generation != current_data_generation
                    || recovery.source_data_generation.checked_add(1)
                        != Some(current_data_generation)
                {
                    return Err(UsageStoreError::InvalidImport(
                        "timeline recovery was already applied".into(),
                    ));
                }
                Some((payload, true))
            } else {
                if recovery.source_data_generation != current_data_generation {
                    return Err(UsageStoreError::InvalidImport(
                        "timeline recovery source generation changed".into(),
                    ));
                }
                if session_model_totals_from_transaction(&transaction)?
                    != recovery.source_model_totals
                {
                    return Err(UsageStoreError::InvalidImport(
                        "timeline recovery source totals changed".into(),
                    ));
                }
                for range in &recovery.ranges {
                    let exists: i64 = transaction.query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM session_ranges
                            WHERE root_identity=?1 AND relative_path=?2
                              AND file_device=?3 AND file_inode=?4
                              AND prefix_generation=?5 AND start_offset=?6
                              AND end_offset=?7 AND record_sha256=?8
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
                    if exists != 0 {
                        return Err(UsageStoreError::InvalidImport(
                            "timeline recovery source range was already committed".into(),
                        ));
                    }
                }
                Some((payload, false))
            }
        } else {
            None
        };
        if current_epoch == Some(collector_epoch) {
            if current_cycle_seq == cycle_seq {
                if current_generation.1 != reset_at || current_generation.2 != window_seconds {
                    return Err(UsageStoreError::InvalidImport(
                        "replayed collection generation has a different period".into(),
                    ));
                }
                if cumulative_payload
                    .as_ref()
                    .is_some_and(|(_, marker_exists)| !marker_exists)
                {
                    return Err(UsageStoreError::InvalidImport(
                        "replayed collection generation has no cumulative recovery marker".into(),
                    ));
                }
                if timeline_payload
                    .as_ref()
                    .is_some_and(|(_, marker_exists)| !marker_exists)
                {
                    return Err(UsageStoreError::InvalidImport(
                        "replayed collection generation has no timeline recovery marker".into(),
                    ));
                }
                // The complete transaction for this epoch/cycle already
                // committed. Return its exact generation rather than
                // incrementing durable state on an acknowledgement retry.
                let replay_observations =
                    upsert_observations(&transaction, &canonical_observations)?;
                upsert_observation_model_totals(&transaction, &replay_observations, true, None)?;
                replace_session_pending_ranges(
                    &transaction,
                    &canonical_pending_ranges,
                    &canonical_ranges,
                    replace_incomplete_pending_ranges,
                )?;
                upsert_session_events(&transaction, &canonical_events)?;
                upsert_session_task_indexed_ranges(&transaction, &canonical_task_indexed_ranges)?;
                upsert_session_task_events(&transaction, &canonical_task_events)?;
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

        upsert_canonical_samples(
            &transaction,
            &canonical_samples,
            preserve_all_existing_history,
            preserve_existing_before,
        )?;
        let persisted_observations = upsert_observations(&transaction, &canonical_observations)?;
        upsert_observation_model_totals(
            &transaction,
            &persisted_observations,
            preserve_all_existing_history,
            preserve_existing_before,
        )?;
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
        replace_session_pending_ranges(
            &transaction,
            &canonical_pending_ranges,
            &canonical_ranges,
            replace_incomplete_pending_ranges,
        )?;
        upsert_session_events(&transaction, &canonical_events)?;
        upsert_session_task_indexed_ranges(&transaction, &canonical_task_indexed_ranges)?;
        upsert_session_task_events(&transaction, &canonical_task_events)?;
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
        if let (Some(recovery), Some((payload, false))) =
            (cumulative_recovery, cumulative_payload.as_ref())
        {
            transaction.execute(
                "INSERT INTO session_cumulative_recoveries (
                    recovery_id, payload_json, applied_generation
                 ) VALUES (?1, ?2, ?3)",
                params![&recovery.recovery_id, payload, next.to_string()],
            )?;
        }
        if let (Some(recovery), Some((payload, false))) =
            (timeline_recovery, timeline_payload.as_ref())
        {
            transaction.execute(
                "INSERT INTO session_timeline_recoveries (
                    recovery_id, payload_json, applied_generation
                 ) VALUES (?1, ?2, ?3)",
                params![&recovery.recovery_id, payload, next.to_string()],
            )?;
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
        replace_recorded_session_markers(&transaction, &recorded_sessions)?;
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
        let canonical = canonicalize_samples(&transaction, samples, false, None)?;
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

        upsert_canonical_samples(&transaction, &canonical, false, None)?;
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
    /// Account partitions prune the canonical table and its existing
    /// sidecars in one transaction. Exact recorded-source marker lifecycle is
    /// independent. The cutoff is
    /// strictly exclusive, so projected observations at the cutoff or in the
    /// future remain visible regardless of reset period.
    pub fn prune_older_than_three_months(&mut self, now: DateTime<Utc>) -> Result<usize> {
        let cutoff = three_months_before(now).timestamp();
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM durable_state
             WHERE singleton >= ?1 AND data_generation < ?2",
            params![DURABLE_STATE_OBSERVATION_MIN_SINGLETON, cutoff],
        )?;
        transaction.execute(
            "DELETE FROM usage_model_history WHERE timestamp < ?1",
            params![cutoff],
        )?;
        let deleted = transaction.execute(
            "DELETE FROM usage_history WHERE timestamp < ?1",
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

    #[test]
    fn quota_transition_uses_quota_and_window_evidence_not_reset_at_alone() {
        const WINDOW: i64 = 7 * 24 * 60 * 60;
        let previous_observed = 1_789_200_605;
        let previous_reset = 1_789_773_095;
        let boundary_observed = 1_789_200_649;
        let boundary_reset = 1_789_805_415;

        assert_eq!(
            classify_quota_transition(
                Some(previous_reset),
                WINDOW,
                Some(previous_observed),
                Some(61.0),
                boundary_reset,
                WINDOW,
                Some(100.0),
                boundary_observed,
            ),
            QuotaTransition::Boundary,
            "quota recovery at the replacement window start is one real boundary"
        );

        assert_eq!(
            classify_quota_transition(
                Some(boundary_reset),
                WINDOW,
                Some(boundary_observed),
                Some(100.0),
                boundary_reset + 134,
                WINDOW,
                Some(100.0),
                boundary_observed + 180,
            ),
            QuotaTransition::SamePeriod,
            "a corrected deadline without quota recovery stays in the accepted period"
        );

        for start_drift in [-60, 0, 60] {
            assert_eq!(
                classify_quota_transition(
                    Some(boundary_reset),
                    WINDOW,
                    Some(boundary_observed),
                    Some(1.0),
                    boundary_reset + WINDOW + start_drift,
                    WINDOW,
                    Some(100.0),
                    boundary_reset,
                ),
                QuotaTransition::Boundary,
                "the next full window is a boundary at the existing tolerance endpoints"
            );
        }

        let alias_a = 1_789_437_490;
        let alias_b = 1_789_300_251;
        assert_eq!(
            classify_quota_transition(
                Some(alias_a),
                WINDOW,
                Some(1_788_972_900),
                Some(29.0),
                alias_b,
                WINDOW,
                Some(17.0),
                1_788_975_540,
            ),
            QuotaTransition::Rejected
        );
        assert_eq!(
            classify_quota_transition(
                Some(alias_b),
                WINDOW,
                Some(1_788_975_540),
                Some(17.0),
                alias_a,
                WINDOW,
                Some(29.0),
                1_788_975_600,
            ),
            QuotaTransition::Rejected,
            "quota recovery without a matching time boundary is not a rollover"
        );

        for (next_reset, next_window, next_remaining, next_observed) in [
            (
                boundary_reset - 61,
                WINDOW,
                Some(99.0),
                boundary_observed + 60,
            ),
            (
                boundary_reset,
                WINDOW + 1,
                Some(99.0),
                boundary_observed + 60,
            ),
            (boundary_reset, WINDOW, None, boundary_observed + 60),
            (
                boundary_reset,
                WINDOW,
                Some(f64::NAN),
                boundary_observed + 60,
            ),
            (boundary_reset, WINDOW, Some(99.0), boundary_observed - 1),
        ] {
            assert_eq!(
                classify_quota_transition(
                    Some(boundary_reset),
                    WINDOW,
                    Some(boundary_observed),
                    Some(100.0),
                    next_reset,
                    next_window,
                    next_remaining,
                    next_observed,
                ),
                QuotaTransition::Rejected
            );
        }
    }

    fn active_thread(id: &str, updated_at: i64) -> ActiveThreadRecord {
        ActiveThreadRecord {
            id: id.into(),
            updated_at,
            title: format!("thread {id}"),
            parent_thread_id: None,
            model: "gpt-5".into(),
            model_label: "SOL".into(),
            total_tokens: Some(12),
            context_usage_tokens: Some(8),
            context_window_tokens: Some(128),
            created_at: Some(updated_at - 60),
            last_user_message_at: Some(updated_at),
            is_subagent: false,
            depth: Some(0),
        }
    }

    #[test]
    fn active_thread_snapshot_roundtrips_and_empty_is_real() {
        let path = database_path("active-thread-roundtrip");
        let identity = partition_identity('a', 1);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let snapshot = ActiveThreadSnapshot {
            observed_at: 1_800_000_060,
            threads: vec![active_thread("thread-a", 1_800_000_000)],
        };
        assert_eq!(store.commit_active_thread_snapshot(&snapshot).unwrap(), 1);
        let row: (i64, String, i64) = store
            .connection
            .query_row(
                "SELECT observed_at, threads_json, acquisition_degraded
                 FROM active_thread_snapshot WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0, snapshot.observed_at);
        assert_eq!(row.2, 0);
        let json: serde_json::Value = serde_json::from_str(&row.1).unwrap();
        assert_eq!(json[0]["id"], "thread-a");
        assert_eq!(json[0]["updated_at"], 1_800_000_000_i64);

        // Exact canonical replay is a no-op, including generation.
        assert_eq!(store.commit_active_thread_snapshot(&snapshot).unwrap(), 1);
        let observed_later = ActiveThreadSnapshot {
            observed_at: 1_800_000_120,
            threads: snapshot.threads.clone(),
        };
        assert_eq!(
            store
                .commit_active_thread_snapshot(&observed_later)
                .unwrap(),
            1
        );
        let empty = ActiveThreadSnapshot {
            observed_at: 1_800_000_120,
            threads: Vec::new(),
        };
        assert_eq!(store.commit_active_thread_snapshot(&empty).unwrap(), 2);
        let empty_json: String = store
            .connection
            .query_row(
                "SELECT threads_json FROM active_thread_snapshot WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(empty_json, "[]");
        remove_database(&path);
    }

    #[test]
    fn active_thread_snapshot_failure_retains_row_and_generation() {
        let path = database_path("active-thread-atomic");
        let identity = partition_identity('b', 1);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let before = ActiveThreadSnapshot {
            observed_at: 1_800_000_060,
            threads: vec![active_thread("thread-before", 1_800_000_000)],
        };
        store.commit_active_thread_snapshot(&before).unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER reject_active_thread_update
                 BEFORE UPDATE ON active_thread_snapshot
                 BEGIN SELECT RAISE(ABORT, 'reject active thread update'); END;",
            )
            .unwrap();
        let after = ActiveThreadSnapshot {
            observed_at: 1_800_000_120,
            threads: vec![active_thread("thread-after", 1_800_000_060)],
        };
        assert!(store.commit_active_thread_snapshot(&after).is_err());
        let row: (String, String) = store
            .connection
            .query_row(
                "SELECT threads_json, data_generation FROM active_thread_snapshot
                 JOIN collection_generation USING (singleton)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(row.0.contains("thread-before"));
        assert!(!row.0.contains("thread-after"));
        assert_eq!(row.1, "1");
        remove_database(&path);
    }

    #[test]
    fn acquisition_degraded_marks_and_clears_without_changing_payload() {
        let path = database_path("active-thread-degraded-transition");
        let identity = partition_identity('e', 1);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let snapshot = ActiveThreadSnapshot {
            observed_at: 1_800_000_060,
            threads: vec![active_thread("retained", 1_800_000_000)],
        };
        store.commit_active_thread_snapshot(&snapshot).unwrap();
        let before: String = store
            .connection
            .query_row(
                "SELECT threads_json FROM active_thread_snapshot WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            store
                .commit_active_thread_snapshot_with_health(&snapshot, true)
                .unwrap(),
            2
        );
        assert_eq!(store.mark_acquisition_degraded().unwrap(), 2);
        let degraded: (String, i64) = store
            .connection
            .query_row(
                "SELECT threads_json, acquisition_degraded
                 FROM active_thread_snapshot WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(degraded.0, before);
        assert_eq!(degraded.1, 1);
        assert_eq!(store.clear_acquisition_degraded().unwrap(), 3);
        assert_eq!(store.clear_acquisition_degraded().unwrap(), 3);
        let recovered: (String, i64) = store
            .connection
            .query_row(
                "SELECT threads_json, acquisition_degraded
                 FROM active_thread_snapshot WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(recovered.0, before);
        assert_eq!(recovered.1, 0);
        remove_database(&path);
    }

    #[test]
    fn active_thread_snapshot_invalid_migration_preserves_existing_table_and_row() {
        let path = database_path("active-thread-migration-invalid");
        let identity = partition_identity('c', 1);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let snapshot = ActiveThreadSnapshot {
            observed_at: 1_800_000_060,
            threads: vec![active_thread("migration-row", 1_800_000_000)],
        };
        store.commit_active_thread_snapshot(&snapshot).unwrap();
        drop(store);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE active_thread_snapshot RENAME TO active_thread_snapshot_old;
                 CREATE TABLE active_thread_snapshot(
                     singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                     observed_at INTEGER NOT NULL CHECK(observed_at>0),
                     threads_json TEXT NOT NULL,
                     acquisition_degraded INTEGER NOT NULL DEFAULT 0
                         CHECK(acquisition_degraded IN (0,1)),
                     unexpected TEXT NOT NULL
                 );
                 INSERT INTO active_thread_snapshot(
                     singleton, observed_at, threads_json, acquisition_degraded, unexpected
                 ) SELECT singleton, observed_at, threads_json, acquisition_degraded, 'keep-me'
                   FROM active_thread_snapshot_old;
                 DROP TABLE active_thread_snapshot_old;
                 PRAGMA user_version = 6;",
            )
            .unwrap();
        drop(connection);

        assert!(UsageStore::open_partitioned(&path, &identity).is_err());
        let retained = Connection::open(&path).unwrap();
        let unexpected: String = retained
            .query_row(
                "SELECT unexpected FROM active_thread_snapshot WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let json: String = retained
            .query_row(
                "SELECT threads_json FROM active_thread_snapshot WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unexpected, "keep-me");
        assert!(json.contains("migration-row"));
        remove_database(&path);
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

    fn downgrade_canonical_history_to_v9(connection: &Connection) {
        connection
            .execute_batch(
                "DROP TRIGGER usage_history_canonical_insert_guard;
                 DROP TRIGGER usage_history_canonical_update_guard;
                 DROP TRIGGER usage_model_history_canonical_insert_guard;
                 DROP TRIGGER usage_model_history_canonical_update_guard;
                 DROP TRIGGER durable_history_observation_insert_guard;
                 DROP TRIGGER durable_history_observation_update_guard;
                 DROP TRIGGER usage_history_sidecar_update_guard;
                 DROP TRIGGER usage_history_sidecar_delete_guard;
                 DROP INDEX usage_history_canonical_timestamp_idx;
                 DROP INDEX usage_model_history_canonical_timestamp_model_idx;
                 PRAGMA user_version = 9;",
            )
            .unwrap();
    }

    #[test]
    fn account_history_migration_replaces_aliases_and_sidecars_and_enforces_boundary() {
        let path = database_path("account-history-canonical-migration");
        let identity = partition_identity('9', 61);
        drop(UsageStore::create_partitioned(&path, &identity).unwrap());

        let minute = 1_800_000_000_i64;
        let reset_a = 1_800_604_800_i64;
        let reset_b = reset_a + 60;
        let weaker = sample(minute + 5, reset_a, Some(71.0), 1.0);
        let stronger = sample(minute + 40, reset_b, Some(70.0), 2.0);
        let weaker_model = SessionModelTotal {
            model: "SOL".into(),
            total_tokens: 10,
            input_tokens: 8,
            cached_input_tokens: 2,
            output_tokens: 2,
            cache_write_input_tokens: Some(0),
        };
        let stronger_model = SessionModelTotal {
            total_tokens: 20,
            input_tokens: 16,
            cached_input_tokens: 4,
            output_tokens: 4,
            ..weaker_model.clone()
        };
        let connection = Connection::open(&path).unwrap();
        downgrade_canonical_history_to_v9(&connection);
        connection
            .execute(
                "UPDATE collection_generation SET reset_at=?1, window_seconds=?2
                 WHERE singleton=1",
                params![reset_b, 604_800_i64],
            )
            .unwrap();
        for (row, raw_remaining) in [(&weaker, Some(71.0_f64)), (&stronger, Some(70.0_f64))] {
            connection
                .execute(
                    "INSERT INTO usage_history (
                         timestamp, reset_at, remaining_percent, sol_dollars,
                         terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        row.timestamp,
                        row.reset_at,
                        raw_remaining,
                        row.sol_dollars,
                        row.terra_dollars,
                        row.luna_dollars,
                        row.sol_tokens as i64,
                        row.terra_tokens as i64,
                        row.luna_tokens as i64,
                    ],
                )
                .unwrap();
        }
        for (row, total, complete) in [
            (&weaker, &weaker_model, 0_i64),
            (&stronger, &stronger_model, 1_i64),
        ] {
            connection
                .execute(
                    "INSERT INTO usage_model_history (
                         reset_at, timestamp, model, total_tokens, input_tokens,
                         cached_input_tokens, output_tokens, cache_write_input_tokens,
                         model_set_complete
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        row.reset_at,
                        row.timestamp,
                        &total.model,
                        total.total_tokens.to_string(),
                        total.input_tokens.to_string(),
                        total.cached_input_tokens.to_string(),
                        total.output_tokens.to_string(),
                        total
                            .cache_write_input_tokens
                            .map(|value| value.to_string()),
                        complete,
                    ],
                )
                .unwrap();
        }
        for (singleton, observation) in [
            (2_i64, UsageHistoryObservation::legacy_unknown(&weaker)),
            (3_i64, UsageHistoryObservation::confirmed(&stronger)),
        ] {
            connection
                .execute(
                    "INSERT INTO durable_state (
                         singleton, data_generation, data_hash, snapshot_json
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        singleton,
                        observation.timestamp,
                        observation_data_hash(observation.reset_at, observation.timestamp),
                        observation_json(&observation).unwrap(),
                    ],
                )
                .unwrap();
        }
        let raw_before = legacy_raw_evidence(&connection).unwrap();
        drop(connection);

        let backup =
            UsageStore::backup_generations_partitioned_verified(&path, &identity, 1).unwrap();
        assert!(UsageStore::migrate_partition_history_after_verified_backup(
            &path, &identity, &backup
        )
        .unwrap());
        assert!(
            !UsageStore::migrate_partition_history_after_verified_backup(&path, &identity, &backup)
                .unwrap()
        );

        let mut store = UsageStore::open_partitioned(&path, &identity).unwrap();
        let backup_path = path.with_extension("sqlite3.bak.1");
        let backup_store = Connection::open_with_flags(
            backup_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        assert_eq!(legacy_raw_evidence(&backup_store).unwrap(), raw_before);
        let canonical = store.load_all_raw().unwrap();
        assert_eq!(canonical.len(), 1);
        assert_eq!(canonical[0].timestamp, minute);
        assert_eq!(canonical[0].reset_at, reset_b);
        assert_eq!(canonical[0].sol_dollars, stronger.sol_dollars);
        assert_eq!(canonical[0].remaining_percent, Some(70.0));
        let model_groups = load_history_model_groups(&store.connection).unwrap();
        assert_eq!(model_groups.len(), 1);
        assert_eq!(model_groups[0].timestamp, minute);
        assert_eq!(model_groups[0].reset_at, reset_b);
        assert_eq!(model_groups[0].totals, vec![stronger_model]);
        assert!(model_groups[0].complete);
        let observations = store
            .load_recent_observations(Utc.timestamp_opt(minute + 60, 0).unwrap())
            .unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].timestamp, minute);
        assert_eq!(observations[0].reset_at, reset_b);
        assert_eq!(observations[0].model_source, ModelSource::Confirmed);
        assert!(store
            .connection
            .execute(
                "INSERT INTO usage_history (
                     timestamp, reset_at, remaining_percent, sol_dollars,
                     terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
                 ) VALUES (?1, ?2, -1, 1, 1, 1, 1, 1, 1)",
                params![minute + 60, reset_b],
            )
            .is_err());
        assert!(store
            .connection
            .execute(
                "INSERT INTO usage_history (
                     timestamp, reset_at, remaining_percent, sol_dollars,
                     terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
                 ) VALUES (?1, ?2, 70, 2, 2, 3, 11, 22, 33)",
                params![minute, reset_b + 60],
            )
            .is_err());
        assert!(store
            .connection
            .execute(
                "INSERT INTO usage_model_history (
                     reset_at, timestamp, model, total_tokens, input_tokens,
                     cached_input_tokens, output_tokens, cache_write_input_tokens,
                     model_set_complete
                 ) VALUES (?1, ?2, 'SOL', '1', '1', '0', '0', '0', 1)",
                params![reset_b, minute + 60],
            )
            .is_err());
        let orphan_observation =
            UsageHistoryObservation::confirmed(&sample(minute + 180, reset_b, Some(66.0), 5.0));
        assert!(store
            .connection
            .execute(
                "INSERT INTO durable_state (
                     singleton, data_generation, data_hash, snapshot_json
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    99_i64,
                    orphan_observation.timestamp,
                    observation_data_hash(
                        orphan_observation.reset_at,
                        orphan_observation.timestamp
                    ),
                    observation_json(&orphan_observation).unwrap(),
                ],
            )
            .is_err());

        // Normal post-migration writes must collapse observations in one
        // minute onto the existing canonical timestamp and reset key. A
        // rolling reset alias cannot recreate a second period or a second
        // row for that minute.
        let next_observation = sample(minute + 65, reset_b, Some(69.0), 3.0);
        store.upsert_sample(&next_observation).unwrap();
        let replay_with_reset_alias = sample(minute + 80, reset_b + 30, Some(68.0), 4.0);
        store.upsert_sample(&replay_with_reset_alias).unwrap();
        let canonical = store.load_all_raw().unwrap();
        assert_eq!(canonical.len(), 2);
        assert_eq!(canonical[1].timestamp, minute + 60);
        assert_eq!(canonical[1].reset_at, reset_b);
        assert_eq!(canonical[1].remaining_percent, Some(68.0));
        assert_eq!(canonical[1].sol_dollars, 4.0);

        // Quota-only acquisition remains an independent durable fact when no
        // usage vector exists for that timestamp.
        let unavailable = UsageHistoryObservation::unavailable(minute + 120, reset_b, Some(67.0));
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            upsert_observations(&transaction, std::slice::from_ref(&unavailable)).unwrap(),
            vec![unavailable.clone()]
        );
        transaction.commit().unwrap();
        let observations = store
            .load_recent_observations(Utc.timestamp_opt(minute + 180, 0).unwrap())
            .unwrap();
        assert!(observations.contains(&unavailable));

        drop(backup_store);
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn account_history_migration_rolls_back_incoherent_owned_minute() {
        let path = database_path("account-history-canonical-incoherent-minute");
        let identity = partition_identity('8', 62);
        drop(UsageStore::create_partitioned(&path, &identity).unwrap());

        let minute = 1_800_000_000_i64;
        let reset_at = 1_800_604_800_i64;
        let rows = [
            sample(minute + 5, reset_at, Some(99.0), 1.0),
            sample(minute + 45, reset_at, Some(100.0), 2.0),
        ];
        let connection = Connection::open(&path).unwrap();
        downgrade_canonical_history_to_v9(&connection);
        connection
            .execute(
                "UPDATE collection_generation SET reset_at=?1, window_seconds=?2
                 WHERE singleton=1",
                params![reset_at, 604_800_i64],
            )
            .unwrap();
        for row in &rows {
            connection
                .execute(
                    "INSERT INTO usage_history (
                         timestamp, reset_at, remaining_percent, sol_dollars,
                         terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        row.timestamp,
                        row.reset_at,
                        row.remaining_percent,
                        row.sol_dollars,
                        row.terra_dollars,
                        row.luna_dollars,
                        row.sol_tokens as i64,
                        row.terra_tokens as i64,
                        row.luna_tokens as i64,
                    ],
                )
                .unwrap();
        }
        let raw_before = legacy_raw_evidence(&connection).unwrap();
        drop(connection);

        let backup =
            UsageStore::backup_generations_partitioned_verified(&path, &identity, 1).unwrap();
        let error =
            UsageStore::migrate_partition_history_after_verified_backup(&path, &identity, &backup)
                .expect_err("an increasing remaining quota must abort the whole migration");
        assert!(error
            .to_string()
            .contains("remaining quota increases within minute"));

        let retained = Connection::open(&path).unwrap();
        assert_eq!(legacy_raw_evidence(&retained).unwrap(), raw_before);
        assert_eq!(
            retained
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            9
        );
        drop(retained);
        let retained_backup = Connection::open_with_flags(
            path.with_extension("sqlite3.bak.1"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        assert_eq!(legacy_raw_evidence(&retained_backup).unwrap(), raw_before);
        drop(retained_backup);

        remove_database(&path);
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
    fn current_incomplete_pending_inventory_replaces_the_previous_cycle() {
        let path = database_path("pending-inventory-replacement");
        let identity = partition_identity('d', 19);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let key = (
            "unix:10:20".to_owned(),
            "2026/09/recovered.jsonl".to_owned(),
            10_u64,
            30_u64,
            0x5678_u128,
            0_u64,
        );
        let backlog = SessionPendingRange {
            root_identity: key.0.clone(),
            relative_path: key.1.clone(),
            file_device: key.2,
            file_inode: key.3,
            start_offset: key.5,
            end_offset: key.5,
            collector_epoch: 0x1234,
            cycle_seq: 1,
            prefix_generation: key.4,
            record_sha256: "00".repeat(32),
            parser_version: "v1".into(),
            reason: "cycle-budget-backlog".into(),
            complete: false,
        };
        {
            let transaction = store.connection.transaction().unwrap();
            replace_session_pending_ranges(
                &transaction,
                &BTreeMap::from([(key.clone(), backlog)]),
                &BTreeMap::new(),
                true,
            )
            .unwrap();
            transaction.commit().unwrap();
        }
        let malformed = SessionPendingRange {
            end_offset: 10,
            record_sha256: "11".repeat(32),
            reason: "malformed-json-record".into(),
            complete: true,
            ..SessionPendingRange {
                root_identity: key.0.clone(),
                relative_path: key.1.clone(),
                file_device: key.2,
                file_inode: key.3,
                start_offset: key.5,
                end_offset: key.5,
                collector_epoch: 0x1234,
                cycle_seq: 2,
                prefix_generation: key.4,
                record_sha256: "00".repeat(32),
                parser_version: "v1".into(),
                reason: "cycle-budget-backlog".into(),
                complete: false,
            }
        };
        {
            let transaction = store.connection.transaction().unwrap();
            replace_session_pending_ranges(
                &transaction,
                &BTreeMap::from([(key, malformed)]),
                &BTreeMap::new(),
                true,
            )
            .unwrap();
            transaction.commit().unwrap();
        }

        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT reason, complete FROM session_pending_ranges",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("malformed-json-record".to_owned(), 1)
        );
        remove_database(&path);
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
            .execute("DROP TABLE session_cumulative_recoveries", [])
            .unwrap();
        store
            .connection
            .execute("DROP TABLE session_timeline_recoveries", [])
            .unwrap();
        store
            .connection
            .execute("DROP TABLE session_pending_ranges", [])
            .unwrap();
        store
            .connection
            .execute("DROP TABLE session_events", [])
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
        assert_eq!(
            legacy_read_state.model_totals,
            std::slice::from_ref(&legacy_total)
        );
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
        assert_eq!(
            committed.canonical_samples,
            std::slice::from_ref(&committed_sample)
        );
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
    fn storage_partition_login_id_v8_migration_preserves_existing_state() {
        let path = database_path("partition-login-id-v8-migration");
        let identity = partition_identity('7', 41);
        let reset_at = 1_800_604_800;
        let mut source = recorded_source("2026/09/login-id-migration.jsonl", 77);
        source.file_bytes = 10;
        let checkpoint = checkpoint(&source, 10);
        let committed_sample = sample(1_800_000_000, reset_at, Some(73.0), 4.0);

        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        store
            .commit_session_collection(SessionCollectionCommit {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: checkpoint.collector_epoch,
                cycle_seq: checkpoint.cycle_seq,
                samples: std::slice::from_ref(&committed_sample),
                checkpoints: std::slice::from_ref(&checkpoint),
                ranges: &[],
                model_totals: &[],
                recorded_sessions: std::slice::from_ref(&source),
            })
            .unwrap();
        let samples_before = store.load_all_raw().unwrap();
        let state_before = store.load_session_collection_state().unwrap();
        assert_eq!(state_before.data_generation, 1);
        assert!(store.recorded_session_matches(&source).unwrap());
        drop(store);

        // Reproduce the v8 shape exactly: only the new nullable column is
        // absent and every pre-existing row remains in place.
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE storage_partition DROP COLUMN login_id;
                 PRAGMA user_version = 8;",
            )
            .unwrap();
        drop(connection);

        let migrated = UsageStore::open_partitioned(&path, &identity).unwrap();
        let schema_version: i64 = migrated
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schema_version, 9);
        let login_id: Option<String> = migrated
            .connection
            .query_row(
                "SELECT login_id FROM storage_partition WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(login_id, None);
        assert_eq!(migrated.partition_login_id().unwrap(), None);
        assert_eq!(migrated.load_all_raw().unwrap(), samples_before);
        assert_eq!(
            migrated.load_session_collection_state().unwrap(),
            state_before
        );
        assert!(migrated.recorded_session_matches(&source).unwrap());

        let stored_identity: (i64, String, String, String, String, String) = migrated
            .connection
            .query_row(
                "SELECT singleton, schema_version, profile_scope_id, account_scope_id,
                        storage_epoch, partition_id
                 FROM storage_partition WHERE singleton = 1",
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
            )
            .unwrap();
        assert_eq!(
            stored_identity,
            (
                1,
                identity.schema_version,
                identity.profile_scope_id,
                identity.account_scope_id,
                identity.storage_epoch.to_string(),
                identity.partition_id,
            )
        );
        drop(migrated);
        remove_database(&path);
    }

    #[test]
    fn partition_login_id_roundtrips_and_changes_only_storage_metadata() {
        let path = database_path("partition-login-id-roundtrip");
        let identity = partition_identity('8', 42);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let generation_before: String = store
            .connection
            .query_row(
                "SELECT data_generation FROM collection_generation WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generation_before, "0");

        let maximum = "名".repeat(MAX_LOGIN_ID_SCALARS);
        store.set_partition_login_id(&maximum).unwrap();
        assert_eq!(
            store.partition_login_id().unwrap().as_deref(),
            Some(maximum.as_str())
        );
        store.set_partition_login_id("user@example.com").unwrap();
        assert_eq!(
            store.partition_login_id().unwrap().as_deref(),
            Some("user@example.com")
        );
        let stored: Option<String> = store
            .connection
            .query_row(
                "SELECT login_id FROM storage_partition WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(stored.as_deref(), Some("user@example.com"));

        // A trigger that rejects every login-id UPDATE proves the equal-value
        // path is a genuine no-op rather than an UPDATE with identical data.
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER reject_login_id_update
                 BEFORE UPDATE OF login_id ON storage_partition
                 BEGIN SELECT RAISE(ABORT, 'login id update must be skipped'); END;",
            )
            .unwrap();
        store.set_partition_login_id("user@example.com").unwrap();
        let generation_after_same: String = store
            .connection
            .query_row(
                "SELECT data_generation FROM collection_generation WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generation_after_same, generation_before);
        store
            .connection
            .execute_batch("DROP TRIGGER reject_login_id_update")
            .unwrap();

        store.set_partition_login_id("other@example.com").unwrap();
        let changed: Option<String> = store
            .connection
            .query_row(
                "SELECT login_id FROM storage_partition WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(changed.as_deref(), Some("other@example.com"));
        let generation_after_change: String = store
            .connection
            .query_row(
                "SELECT data_generation FROM collection_generation WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generation_after_change, generation_before);

        drop(store);
        let reopened = UsageStore::open_partitioned(&path, &identity).unwrap();
        assert_eq!(
            reopened.partition_login_id().unwrap().as_deref(),
            Some("other@example.com")
        );
        let roundtripped: Option<String> = reopened
            .connection
            .query_row(
                "SELECT login_id FROM storage_partition WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(roundtripped.as_deref(), Some("other@example.com"));
        let reopened_generation: String = reopened
            .connection
            .query_row(
                "SELECT data_generation FROM collection_generation WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reopened_generation, generation_before);
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn partition_login_id_rejects_empty_oversized_and_control_values() {
        let path = database_path("partition-login-id-invalid");
        let identity = partition_identity('9', 43);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let oversized = "x".repeat(MAX_LOGIN_ID_SCALARS + 1);
        for invalid in [
            "",
            " leading",
            "trailing ",
            "line\nfeed",
            "delete\u{007f}",
            oversized.as_str(),
        ] {
            assert!(
                store.set_partition_login_id(invalid).is_err(),
                "invalid login id was accepted: {invalid:?}"
            );
        }
        let stored: Option<String> = store
            .connection
            .query_row(
                "SELECT login_id FROM storage_partition WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(stored, None);
        let generation: String = store
            .connection
            .query_row(
                "SELECT data_generation FROM collection_generation WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generation, "0");
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn session_task_schema_v8_migrates_legacy_partition() {
        let path = database_path("session-task-schema-migration");
        let identity = partition_identity('f', 21);
        let store = UsageStore::create_partitioned(&path, &identity).unwrap();
        drop(store);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE session_task_events;
                 DROP TABLE session_task_indexed_ranges;
                 PRAGMA user_version = 7;",
            )
            .unwrap();
        drop(connection);

        let migrated = UsageStore::open_partitioned(&path, &identity).unwrap();
        let schema_version: i64 = migrated
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schema_version, ACCOUNT_DB_SCHEMA_VERSION);
        for table in ["session_task_events", "session_task_indexed_ranges"] {
            let present: i64 = migrated
                .connection
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1
                     )",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(present, 1, "table {table}");
        }
        remove_database(&path);
    }

    #[test]
    fn session_task_evidence_is_zero_event_idempotent_and_complete() {
        let path = database_path("session-task-zero-event");
        let identity = partition_identity('a', 22);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let reset_at = 1_800_604_800;
        let mut source = recorded_source("2026/09/session-task.jsonl", 31);
        source.file_bytes = 10;
        let mut checkpoint = checkpoint(&source, 10);
        checkpoint.prefix_sha256 = "11".repeat(32);
        let range = SessionRange {
            root_identity: source.root_identity.clone(),
            relative_path: source.relative_path.clone(),
            file_device: source.file_device,
            file_inode: source.file_inode,
            start_offset: 0,
            end_offset: 10,
            collector_epoch: checkpoint.collector_epoch,
            cycle_seq: checkpoint.cycle_seq,
            prefix_generation: checkpoint.prefix_generation,
            record_sha256: checkpoint.prefix_sha256.clone(),
        };
        let indexed = SessionTaskIndexedRange {
            root_identity: range.root_identity.clone(),
            relative_path: range.relative_path.clone(),
            file_device: range.file_device,
            file_inode: range.file_inode,
            start_offset: range.start_offset,
            end_offset: range.end_offset,
            collector_epoch: range.collector_epoch,
            cycle_seq: range.cycle_seq,
            prefix_generation: range.prefix_generation,
            record_sha256: range.record_sha256.clone(),
        };
        let first = store
            .commit_session_collection_with_task_evidence(
                SessionCollectionCommit {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: checkpoint.collector_epoch,
                    cycle_seq: checkpoint.cycle_seq,
                    samples: &[],
                    checkpoints: std::slice::from_ref(&checkpoint),
                    ranges: std::slice::from_ref(&range),
                    model_totals: &[],
                    recorded_sessions: &[],
                },
                &[],
                SessionTaskEvidenceInput {
                    events: &[],
                    pending_ranges: &[],
                    task_events: &[],
                    task_indexed_ranges: std::slice::from_ref(&indexed),
                },
            )
            .unwrap();
        assert_eq!(first.data_generation, 1);
        assert!(store.load_session_task_events().unwrap().is_empty());
        assert_eq!(
            store.load_session_task_indexed_ranges().unwrap(),
            std::slice::from_ref(&indexed)
        );
        assert!(store.session_task_coverage_complete().unwrap());

        let replay = store
            .commit_session_collection_with_task_evidence(
                SessionCollectionCommit {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: checkpoint.collector_epoch,
                    cycle_seq: checkpoint.cycle_seq,
                    samples: &[],
                    checkpoints: std::slice::from_ref(&checkpoint),
                    ranges: std::slice::from_ref(&range),
                    model_totals: &[],
                    recorded_sessions: &[],
                },
                &[],
                SessionTaskEvidenceInput {
                    events: &[],
                    pending_ranges: &[],
                    task_events: &[],
                    task_indexed_ranges: std::slice::from_ref(&indexed),
                },
            )
            .unwrap();
        assert_eq!(replay.data_generation, first.data_generation);
        assert_eq!(store.load_session_task_indexed_ranges().unwrap(), [indexed]);
        assert!(store.session_task_coverage_complete().unwrap());
        remove_database(&path);
    }

    #[test]
    fn session_task_evidence_failure_rolls_back_collection_and_marker() {
        let path = database_path("session-task-atomic-failure");
        let identity = partition_identity('b', 23);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let reset_at = 1_800_604_800;
        let mut source = recorded_source("2026/09/session-task-failure.jsonl", 32);
        source.file_bytes = 10;
        let checkpoint = checkpoint(&source, 10);
        let range = SessionRange {
            root_identity: source.root_identity.clone(),
            relative_path: source.relative_path.clone(),
            file_device: source.file_device,
            file_inode: source.file_inode,
            start_offset: 0,
            end_offset: 10,
            collector_epoch: checkpoint.collector_epoch,
            cycle_seq: checkpoint.cycle_seq,
            prefix_generation: checkpoint.prefix_generation,
            record_sha256: "00".repeat(32),
        };
        let indexed = SessionTaskIndexedRange {
            root_identity: range.root_identity.clone(),
            relative_path: range.relative_path.clone(),
            file_device: range.file_device,
            file_inode: range.file_inode,
            start_offset: range.start_offset,
            end_offset: range.end_offset,
            collector_epoch: range.collector_epoch,
            cycle_seq: range.cycle_seq,
            prefix_generation: range.prefix_generation,
            record_sha256: range.record_sha256.clone(),
        };
        store
            .connection
            .execute_batch(
                "CREATE TEMP TRIGGER reject_task_marker
                 BEFORE INSERT ON session_task_indexed_ranges
                 BEGIN SELECT RAISE(ABORT, 'reject task marker'); END;",
            )
            .unwrap();
        assert!(store
            .commit_session_collection_with_task_evidence(
                SessionCollectionCommit {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: checkpoint.collector_epoch,
                    cycle_seq: checkpoint.cycle_seq,
                    samples: &[],
                    checkpoints: std::slice::from_ref(&checkpoint),
                    ranges: std::slice::from_ref(&range),
                    model_totals: &[],
                    recorded_sessions: &[],
                },
                &[],
                SessionTaskEvidenceInput {
                    events: &[],
                    pending_ranges: &[],
                    task_events: &[],
                    task_indexed_ranges: std::slice::from_ref(&indexed),
                },
            )
            .is_err());
        assert_eq!(
            store.load_session_collection_state().unwrap(),
            SessionCollectionState::default()
        );
        for table in [
            "session_ranges",
            "session_checkpoints",
            "session_task_indexed_ranges",
            "session_task_events",
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
    fn replacement_prunes_old_checkpoints_and_replaces_stale_cleanup_marker() {
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
        assert!(!store.recorded_session_matches(&old_source).unwrap());
        assert!(store.recorded_session_matches(&new_source).unwrap());
        let recorded_count: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM recorded_sessions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(recorded_count, 1);
        assert_eq!(
            store
                .forget_recorded_sessions(std::slice::from_ref(&new_source))
                .unwrap(),
            1
        );
        assert!(!store.recorded_session_matches(&old_source).unwrap());
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
        let mut replacement = marker.clone();
        replacement.modified_nanos += 1;
        upgraded
            .upsert_samples_and_recorded_sessions(&[], std::slice::from_ref(&replacement))
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
        assert!(!reopened.recorded_session_matches(&marker).unwrap());
        assert!(reopened.recorded_session_matches(&replacement).unwrap());
        let recorded_count: i64 = reopened
            .connection
            .query_row("SELECT COUNT(*) FROM recorded_sessions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(recorded_count, 1);
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
    fn backup_and_migration_never_repair_the_protected_source() {
        let path = database_path("backup-migration-read-only-source");
        let original = sample(1_700_000_060, 1_700_604_800, Some(75.0), 1.25);
        let store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&original).unwrap();
        drop(store);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute("DROP INDEX usage_history_timestamp_reset_idx", [])
            .unwrap();
        drop(connection);
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let source_before = fs::read(&path).unwrap();

        let blocked = path.with_extension("sqlite3.bak.2");
        fs::create_dir(&blocked).unwrap();
        assert!(UsageStore::backup_generations(&path, 3).is_err());
        assert_eq!(fs::read(&path).unwrap(), source_before);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        fs::remove_dir(&blocked).unwrap();

        let report = UsageStore::migrate_verified(&path, |samples| Ok(samples.to_vec())).unwrap();
        assert_eq!(fs::read(&report.preserved_backup).unwrap(), source_before);
        let migrated =
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let index_present: bool = migrated
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_index_list('usage_history') WHERE name = ?1)",
                [HISTORY_TIMESTAMP_RESET_INDEX],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            index_present,
            "only the validated candidate may repair schema"
        );
        drop(migrated);
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
    fn verified_migration_rejects_account_partition_before_transform() {
        let path = database_path("verified-migration-account-partition");
        let identity = partition_identity('a', 23);
        let store = UsageStore::create_partitioned(&path, &identity).unwrap();
        drop(store);
        let source_before = fs::read(&path).unwrap();
        let transform_called = std::cell::Cell::new(false);

        let result = UsageStore::migrate_verified(&path, |_| {
            transform_called.set(true);
            Ok(Vec::new())
        });

        assert!(matches!(result, Err(UsageStoreError::InvalidImport(_))));
        assert!(!transform_called.get());
        assert_eq!(fs::read(&path).unwrap(), source_before);
        let parent = path.parent().unwrap();
        let file_name = path.file_name().unwrap().to_string_lossy();
        assert!(!parent.join(format!(".{file_name}.migration.lock")).exists());
        assert!(!parent
            .read_dir()
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains(".migration-")));
        UsageStore::open_read_only_partitioned(&path, &identity).unwrap();
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
            canonicalize_samples(&transaction, std::slice::from_ref(&smaller), false, None)
                .unwrap();
        let canonical_observations = canonicalize_observations(
            &transaction,
            std::slice::from_ref(&UsageHistoryObservation::confirmed(&smaller)),
            &canonical_samples,
            &BTreeMap::new(),
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
    fn recovered_prefix_keeps_existing_non_comparable_row_without_blocking_append() {
        let path = database_path("recovered-prefix-preserves-existing");
        let timestamp = 1_700_000_060;
        let reset_at = 1_700_604_800;
        let existing = sample(timestamp, reset_at, Some(75.0), 10.0);
        let mut replayed = sample(timestamp, reset_at, None, 1.0);
        replayed.luna_dollars = 4.0;

        let mut store = UsageStore::open(&path).unwrap();
        store.upsert_sample(&existing).unwrap();
        let transaction = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();

        let preserved = canonicalize_samples(
            &transaction,
            std::slice::from_ref(&replayed),
            false,
            Some(timestamp + 60),
        )
        .unwrap();
        assert_eq!(preserved, vec![existing]);
        assert!(canonicalize_samples(
            &transaction,
            std::slice::from_ref(&replayed),
            false,
            Some(timestamp),
        )
        .is_err());

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
    fn observation_json_round_trip_preserves_binary64_storage_value() {
        let mut expected = sample(1_700_000_040, 1_700_604_800, Some(71.0), 1.0);
        // This exact value exposed the production startup failure: without
        // serde_json's round-trip parser the JSON path moved by one ULP from
        // the same value stored as SQLite REAL.
        expected.luna_dollars = f64::from_bits(0x3ffcd65251dc6ba7);
        let observation = UsageHistoryObservation::confirmed(&expected);
        let decoded = observation_from_sql(
            observation.timestamp,
            observation_data_hash(observation.reset_at, observation.timestamp),
            observation_json(&observation).unwrap(),
        )
        .unwrap();

        assert_eq!(
            decoded.luna_dollars.map(f64::to_bits),
            Some(expected.luna_dollars.to_bits())
        );
        assert!(observation_matches_sample(&decoded, &expected));
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
    fn canonical_commit_projects_usage_and_model_observation_to_one_storage_key() {
        let path = database_path("canonical-commit-source-key");
        let identity = partition_identity('a', 1);
        let minute = 1_800_000_000_i64;
        let reset_at = minute + 604_800;
        let observed = sample(minute + 20, reset_at, Some(64.0), 1.5);
        let model = SessionModelTotal {
            model: "SOL".into(),
            total_tokens: 11,
            input_tokens: 8,
            cached_input_tokens: 2,
            output_tokens: 3,
            cache_write_input_tokens: Some(0),
        };
        let observation =
            UsageHistoryObservation::confirmed_with_models(&observed, vec![model.clone()]);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();

        let result = store
            .commit_session_collection_with_observations(
                SessionCollectionCommit {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: 1,
                    cycle_seq: 1,
                    samples: std::slice::from_ref(&observed),
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: std::slice::from_ref(&model),
                    recorded_sessions: &[],
                },
                std::slice::from_ref(&observation),
            )
            .unwrap();

        assert_eq!(result.canonical_samples.len(), 1);
        assert_eq!(result.canonical_samples[0].timestamp, minute);
        assert_eq!(result.canonical_samples[0].reset_at, reset_at);
        assert_eq!(result.canonical_observations.len(), 1);
        assert_eq!(result.canonical_observations[0].timestamp, minute);
        assert_eq!(result.canonical_observations[0].reset_at, reset_at);
        assert_eq!(store.load_all_raw().unwrap(), result.canonical_samples);
        let model_groups = load_history_model_groups(&store.connection).unwrap();
        assert_eq!(model_groups.len(), 1);
        assert_eq!(model_groups[0].timestamp, minute);
        assert_eq!(model_groups[0].reset_at, reset_at);
        assert_eq!(model_groups[0].totals, vec![model]);

        drop(store);
        remove_database(&path);
    }

    #[test]
    fn canonical_commit_excludes_one_conflicted_minute_with_its_sidecars() {
        let path = database_path("canonical-commit-conflicted-minute");
        let identity = partition_identity('b', 1);
        let minute = 1_800_000_000_i64;
        let reset_at = minute + 604_800;
        let first = sample(minute + 5, reset_at, Some(64.0), 1.5);
        let second = sample(minute + 40, reset_at, Some(63.0), 1.5);
        let model = SessionModelTotal {
            model: "SOL".into(),
            total_tokens: 11,
            input_tokens: 8,
            cached_input_tokens: 2,
            output_tokens: 3,
            cache_write_input_tokens: Some(0),
        };
        let samples = vec![first.clone(), second.clone()];
        let observations = samples
            .iter()
            .map(|sample| {
                UsageHistoryObservation::confirmed_with_models(sample, vec![model.clone()])
            })
            .collect::<Vec<_>>();
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();

        let result = store
            .commit_session_collection_with_observations(
                SessionCollectionCommit {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: 1,
                    cycle_seq: 1,
                    samples: &samples,
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: std::slice::from_ref(&model),
                    recorded_sessions: &[],
                },
                &observations,
            )
            .unwrap();

        assert_eq!(result.data_generation, 1);
        assert!(result.canonical_samples.is_empty());
        assert!(result.canonical_observations.is_empty());
        assert!(store.load_all_raw().unwrap().is_empty());
        assert!(load_history_model_groups(&store.connection)
            .unwrap()
            .is_empty());
        assert!(store
            .load_recent_observations(Utc.timestamp_opt(minute + 60, 0).unwrap())
            .unwrap()
            .is_empty());

        drop(store);
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
        let identity = partition_identity('a', 1);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
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

        let identity = partition_identity('a', 1);
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
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
        let mut reopened = UsageStore::open_partitioned(&path, &identity).unwrap();
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

    #[test]
    fn session_timeline_recovery_is_atomic_projected_and_exactly_once() {
        let path = database_path("session-timeline-recovery");
        let identity = partition_identity('a', 1);
        let reset_at = 1_800_000_600;
        let window_seconds = 600;
        let first_at = 1_800_000_120;
        let second_at = 1_800_000_180;
        let anchor_at = 1_800_000_240;
        let luna = SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 50,
            input_tokens: 40,
            cached_input_tokens: 10,
            output_tokens: 10,
            cache_write_input_tokens: Some(0),
        };
        let sol = SessionModelTotal {
            model: "SOL".into(),
            total_tokens: 100,
            input_tokens: 90,
            cached_input_tokens: 50,
            output_tokens: 10,
            cache_write_input_tokens: Some(0),
        };
        let source_models = vec![luna.clone(), sol.clone()];
        let base = |timestamp, remaining_percent| UsageHistorySample {
            timestamp,
            reset_at,
            remaining_percent: Some(remaining_percent),
            sol_dollars: 1.0,
            terra_dollars: 0.0,
            luna_dollars: 0.5,
            sol_tokens: 100,
            terra_tokens: 0,
            luna_tokens: 50,
        };
        let stale_anchor = base(anchor_at, 88.0);
        let base_samples = vec![base(first_at, 90.0), base(second_at, 89.0), stale_anchor];
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        let base_observations = base_samples
            .iter()
            .map(|sample| {
                UsageHistoryObservation::confirmed_with_models(sample, source_models.clone())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            store
                .commit_session_collection_with_observations(
                    SessionCollectionCommit {
                        reset_at,
                        window_seconds,
                        collector_epoch: 0x111,
                        cycle_seq: 1,
                        samples: &base_samples,
                        checkpoints: &[],
                        ranges: &[],
                        model_totals: &source_models,
                        recorded_sessions: &[],
                    },
                    &base_observations,
                )
                .unwrap()
                .data_generation,
            1
        );

        let range = SessionRange {
            root_identity: "unix:10:20".into(),
            relative_path: "2026/recovery.jsonl".into(),
            file_device: 10,
            file_inode: 20,
            start_offset: 100,
            end_offset: 200,
            collector_epoch: 0x222,
            cycle_seq: 1,
            prefix_generation: 0x333,
            record_sha256: "ab".repeat(32),
        };
        let offset =
            |timestamp, total, input, cached, output, dollars| SessionTimelineRecoveryPoint {
                timestamp,
                offset_model_totals: vec![SessionModelTotal {
                    model: "SOL".into(),
                    total_tokens: total,
                    input_tokens: input,
                    cached_input_tokens: cached,
                    output_tokens: output,
                    cache_write_input_tokens: Some(0),
                }],
                offset_sol_dollars: dollars,
                offset_terra_dollars: 0.0,
                offset_luna_dollars: 0.0,
            };
        let recovery = finalize_session_timeline_recovery(
            &identity.partition_id,
            SessionTimelineRecovery {
                recovery_id: String::new(),
                canonical_reset_at: reset_at,
                window_seconds,
                source_data_generation: 1,
                projection_end_exclusive: anchor_at,
                source_model_totals: source_models.clone(),
                ranges: vec![range.clone()],
                points: vec![
                    offset(first_at, 10, 9, 5, 1, 0.1),
                    offset(second_at, 20, 18, 10, 2, 0.2),
                ],
                final_offset_model_totals: vec![SessionModelTotal {
                    model: "SOL".into(),
                    total_tokens: 25,
                    input_tokens: 23,
                    cached_input_tokens: 12,
                    output_tokens: 2,
                    cache_write_input_tokens: None,
                }],
                final_offset_sol_dollars: 0.25,
                final_offset_terra_dollars: 0.0,
                final_offset_luna_dollars: 0.0,
            },
        )
        .unwrap();
        let corrected_sol = SessionModelTotal {
            total_tokens: 125,
            input_tokens: 113,
            cached_input_tokens: 62,
            output_tokens: 12,
            cache_write_input_tokens: None,
            ..sol.clone()
        };
        let corrected_models = vec![luna.clone(), corrected_sol.clone()];
        let anchor = UsageHistorySample {
            timestamp: anchor_at,
            reset_at,
            remaining_percent: Some(88.0),
            sol_dollars: 1.25,
            terra_dollars: 0.0,
            luna_dollars: 0.5,
            sol_tokens: 125,
            terra_tokens: 0,
            luna_tokens: 50,
        };
        let commit_samples = vec![base_samples[0].clone(), anchor.clone()];
        let commit_observations = vec![
            base_observations[0].clone(),
            UsageHistoryObservation::confirmed_with_models(&anchor, corrected_models.clone()),
        ];
        let commit = || SessionCollectionCommit {
            reset_at,
            window_seconds,
            collector_epoch: 0x222,
            cycle_seq: 1,
            samples: &commit_samples,
            checkpoints: &[],
            ranges: std::slice::from_ref(&range),
            model_totals: &corrected_models,
            recorded_sessions: &[],
        };
        store
            .connection
            .execute_batch(&format!(
                "CREATE TEMP TRIGGER timeline_keep_usage_update
                 BEFORE UPDATE ON usage_history WHEN OLD.timestamp < {anchor_at}
                 BEGIN
                     SELECT RAISE(ABORT, 'timeline rewrote measured usage');
                 END;
                 CREATE TEMP TRIGGER timeline_keep_usage_delete
                 BEFORE DELETE ON usage_history WHEN OLD.timestamp < {anchor_at}
                 BEGIN
                     SELECT RAISE(ABORT, 'timeline deleted measured usage');
                 END;
                 CREATE TEMP TRIGGER timeline_keep_models_update
                 BEFORE UPDATE ON usage_model_history WHEN OLD.timestamp < {anchor_at}
                 BEGIN
                     SELECT RAISE(ABORT, 'timeline rewrote measured models');
                 END;
                 CREATE TEMP TRIGGER timeline_keep_models_delete
                 BEFORE DELETE ON usage_model_history WHEN OLD.timestamp < {anchor_at}
                 BEGIN
                     SELECT RAISE(ABORT, 'timeline deleted measured models');
                 END;
                 CREATE TEMP TRIGGER timeline_recovery_commit_failure
                 BEFORE UPDATE ON collection_generation
                 BEGIN
                     SELECT RAISE(ABORT, 'injected timeline recovery failure');
                 END;"
            ))
            .unwrap();
        assert!(store
            .commit_session_collection_with_timeline_recovery(
                commit(),
                &commit_observations,
                &recovery,
            )
            .is_err());
        assert_eq!(store.load_all_raw().unwrap(), base_samples);
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM session_ranges", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM session_timeline_recoveries",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .load_session_collection_state()
                .unwrap()
                .data_generation,
            1
        );
        store
            .connection
            .execute_batch("DROP TRIGGER timeline_recovery_commit_failure;")
            .unwrap();
        let committed = store
            .commit_session_collection_with_timeline_recovery(
                commit(),
                &commit_observations,
                &recovery,
            )
            .unwrap();
        assert_eq!(committed.data_generation, 2);

        assert_eq!(
            store.load_all_raw().unwrap(),
            vec![
                base_samples[0].clone(),
                base_samples[1].clone(),
                anchor.clone()
            ]
        );
        let logical = store.load_all().unwrap();
        assert_eq!(logical[0].sol_tokens, 100);
        assert_eq!(logical[0].sol_dollars, 1.0);
        assert_eq!(logical[0].luna_tokens, 50);
        assert_eq!(logical[0].remaining_percent, Some(90.0));
        assert_eq!(logical[1].sol_tokens, 100);
        assert_eq!(logical[1].sol_dollars, 1.0);
        assert_eq!(logical[1].remaining_percent, Some(89.0));
        assert_eq!(logical[2], anchor);
        let observations = store
            .load_recent_observations(Utc.timestamp_opt(anchor_at + 1, 0).unwrap())
            .unwrap();
        assert_eq!(observations[0].model_source, ModelSource::Confirmed);
        assert_eq!(observations[1].model_source, ModelSource::Confirmed);
        assert_eq!(observations[2].model_source, ModelSource::Confirmed);
        assert_eq!(
            observations[1]
                .model_totals
                .as_ref()
                .and_then(|totals| totals.iter().find(|total| total.model == "SOL"))
                .map(|total| total.total_tokens),
            Some(100)
        );
        assert_eq!(
            observations[2]
                .model_totals
                .as_ref()
                .and_then(|totals| totals.iter().find(|total| total.model == "SOL")),
            Some(&SessionModelTotal {
                model: "SOL".into(),
                total_tokens: 125,
                input_tokens: 113,
                cached_input_tokens: 62,
                output_tokens: 12,
                cache_write_input_tokens: None,
            })
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM session_timeline_recoveries",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );

        let replay = store
            .commit_session_collection_with_timeline_recovery(
                commit(),
                &commit_observations,
                &recovery,
            )
            .unwrap();
        assert_eq!(replay.data_generation, 2);
        assert_eq!(store.load_all().unwrap(), logical);

        let conflicting_range = SessionRange {
            collector_epoch: 0x222,
            cycle_seq: 2,
            ..range
        };
        let conflict = finalize_session_timeline_recovery(
            &identity.partition_id,
            SessionTimelineRecovery {
                recovery_id: String::new(),
                canonical_reset_at: reset_at,
                window_seconds,
                source_data_generation: 2,
                projection_end_exclusive: anchor_at + 60,
                source_model_totals: corrected_models.clone(),
                ranges: vec![conflicting_range.clone()],
                points: vec![offset(anchor_at, 5, 5, 0, 0, 0.05)],
                final_offset_model_totals: vec![SessionModelTotal {
                    model: "SOL".into(),
                    total_tokens: 5,
                    input_tokens: 5,
                    cached_input_tokens: 0,
                    output_tokens: 0,
                    cache_write_input_tokens: Some(0),
                }],
                final_offset_sol_dollars: 0.05,
                final_offset_terra_dollars: 0.0,
                final_offset_luna_dollars: 0.0,
            },
        )
        .unwrap();
        let conflicting_models = vec![
            luna,
            SessionModelTotal {
                total_tokens: 130,
                input_tokens: 118,
                ..corrected_sol
            },
        ];
        assert!(store
            .commit_session_collection_with_timeline_recovery(
                SessionCollectionCommit {
                    reset_at,
                    window_seconds,
                    collector_epoch: 0x222,
                    cycle_seq: 2,
                    samples: &[],
                    checkpoints: &[],
                    ranges: std::slice::from_ref(&conflicting_range),
                    model_totals: &conflicting_models,
                    recorded_sessions: &[],
                },
                &[],
                &conflict,
            )
            .is_err());
        assert_eq!(
            store
                .load_session_collection_state()
                .unwrap()
                .data_generation,
            2
        );
        assert_eq!(store.load_all().unwrap(), logical);
        drop(store);
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

    fn cumulative_observation(
        timestamp: i64,
        reset_at: i64,
        remaining_percent: f64,
        sol_dollars: f64,
        luna_dollars: f64,
        model_totals: Vec<SessionModelTotal>,
    ) -> UsageHistoryObservation {
        let model_tokens = |model: &str| {
            model_totals
                .iter()
                .find(|total| total.model == model)
                .map(|total| total.total_tokens)
                .unwrap_or(0)
        };
        UsageHistoryObservation {
            timestamp,
            reset_at,
            remaining_percent: Some(remaining_percent),
            sol_dollars: Some(sol_dollars),
            terra_dollars: Some(0.0),
            luna_dollars: Some(luna_dollars),
            sol_tokens: Some(model_tokens("SOL")),
            terra_tokens: Some(0),
            luna_tokens: Some(model_tokens("LUNA")),
            model_source: ModelSource::Confirmed,
            model_totals: Some(model_totals),
            model_totals_complete: false,
        }
    }

    fn commit_cumulative_point(
        store: &mut UsageStore,
        collector_epoch: u128,
        cycle_seq: u64,
        observation: &UsageHistoryObservation,
    ) -> SessionCollectionCommitResult {
        let sample = UsageHistorySample {
            timestamp: observation.timestamp,
            reset_at: observation.reset_at,
            remaining_percent: observation.remaining_percent,
            sol_dollars: observation.sol_dollars.unwrap(),
            terra_dollars: observation.terra_dollars.unwrap(),
            luna_dollars: observation.luna_dollars.unwrap(),
            sol_tokens: observation.sol_tokens.unwrap(),
            terra_tokens: observation.terra_tokens.unwrap(),
            luna_tokens: observation.luna_tokens.unwrap(),
        };
        store
            .commit_session_collection_with_observations(
                SessionCollectionCommit {
                    reset_at: observation.reset_at,
                    window_seconds: 604_800,
                    collector_epoch,
                    cycle_seq,
                    samples: std::slice::from_ref(&sample),
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: observation.model_totals.as_deref().unwrap(),
                    recorded_sessions: &[],
                },
                std::slice::from_ref(observation),
            )
            .unwrap()
    }

    fn raw_cumulative_history_fingerprint(store: &UsageStore) -> String {
        let mut source = String::new();
        let mut history = store
            .connection
            .prepare(
                "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                        terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
                 FROM usage_history ORDER BY reset_at, timestamp",
            )
            .unwrap();
        let rows = history
            .query_map([], |row| {
                Ok(format!(
                    "{:?}",
                    (
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, f64>(4)?,
                        row.get::<_, f64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                    )
                ))
            })
            .unwrap();
        for row in rows {
            source.push_str(&row.unwrap());
            source.push('\n');
        }
        drop(history);
        let mut models = store
            .connection
            .prepare(
                "SELECT reset_at, timestamp, model, total_tokens, input_tokens,
                        cached_input_tokens, output_tokens, cache_write_input_tokens,
                        model_set_complete
                 FROM usage_model_history ORDER BY reset_at, timestamp, model",
            )
            .unwrap();
        let rows = models
            .query_map([], |row| {
                Ok(format!(
                    "{:?}",
                    (
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, i64>(8)?,
                    )
                ))
            })
            .unwrap();
        for row in rows {
            source.push_str(&row.unwrap());
            source.push('\n');
        }
        format!("{:x}", Sha256::digest(source.as_bytes()))
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

    #[test]
    fn cumulative_recovery_uses_the_real_future_reset_round_trip_oracle() {
        let partition_id = "42".repeat(32);
        let reset_a = 1_789_437_490;
        let reset_b = 1_789_300_251;
        let baseline = vec![
            SessionModelTotal {
                model: "LUNA".into(),
                total_tokens: 22_816_483,
                input_tokens: 22_343_994,
                cached_input_tokens: 20_068_352,
                output_tokens: 472_489,
                cache_write_input_tokens: Some(0),
            },
            SessionModelTotal {
                model: "SOL".into(),
                total_tokens: 555_312_427,
                input_tokens: 553_537_987,
                cached_input_tokens: 544_468_480,
                output_tokens: 1_774_440,
                cache_write_input_tokens: Some(0),
            },
        ];
        let first_suffix = vec![SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 91_512,
            input_tokens: 84_194,
            cached_input_tokens: 73_472,
            output_tokens: 7_318,
            cache_write_input_tokens: Some(0),
        }];
        let current = vec![SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 2_446_273,
            input_tokens: 2_382_039,
            cached_input_tokens: 2_035_200,
            output_tokens: 64_234,
            cache_write_input_tokens: Some(0),
        }];
        let observation = |timestamp, reset_at, sol, luna, totals: Vec<SessionModelTotal>| {
            UsageHistoryObservation {
                timestamp,
                reset_at,
                remaining_percent: Some(if reset_at == reset_a { 29.0 } else { 17.0 }),
                sol_dollars: Some(sol),
                terra_dollars: Some(0.0),
                luna_dollars: Some(luna),
                sol_tokens: Some(
                    totals
                        .iter()
                        .find(|total| total.model == "SOL")
                        .map(|total| total.total_tokens)
                        .unwrap_or(0),
                ),
                terra_tokens: Some(0),
                luna_tokens: Some(
                    totals
                        .iter()
                        .find(|total| total.model == "LUNA")
                        .map(|total| total.total_tokens)
                        .unwrap_or(0),
                ),
                model_source: ModelSource::Confirmed,
                model_totals: Some(totals),
                // Missing models are unknown. The exact rows which are
                // present remain valid recovery facts.
                model_totals_complete: false,
            }
        };
        let observations = vec![
            observation(
                1_788_972_900,
                reset_a,
                370.814_975,
                1.402_420_84,
                vec![
                    baseline[1].clone(),
                    SessionModelTotal {
                        total_tokens: 22_488_065,
                        input_tokens: 22_017_725,
                        cached_input_tokens: 19_808_512,
                        output_tokens: 470_340,
                        ..baseline[0].clone()
                    },
                ],
            ),
            observation(
                1_788_975_480,
                reset_b,
                370.814_975,
                1.423_482_24,
                baseline.clone(),
            ),
            observation(
                1_788_975_540,
                reset_b,
                370.814_975,
                1.423_482_24,
                baseline.clone(),
            ),
            observation(1_788_975_600, reset_a, 0.0, 0.012_395_44, first_suffix),
            // The retained production rows also contain three one-second
            // reset aliases. They belong to the same canonical quota period
            // under the existing reset-group contract and must not make an
            // otherwise source-proven suffix unrecoverable.
            observation(
                1_788_985_140,
                reset_a + 1,
                0.0,
                0.092_958_24,
                vec![SessionModelTotal {
                    model: "LUNA".into(),
                    total_tokens: 1_523_427,
                    input_tokens: 1_480_000,
                    cached_input_tokens: 1_280_000,
                    output_tokens: 43_427,
                    cache_write_input_tokens: Some(0),
                }],
            ),
            observation(1_788_996_000, reset_a, 0.0, 0.187_152_6, current.clone()),
        ];

        let recovery = derive_session_cumulative_recovery(
            &partition_id,
            reset_a,
            604_800,
            1_788_996_001,
            &current,
            &observations,
        )
        .unwrap()
        .expect("the source-proven reset regression is recoverable");
        assert_eq!(recovery.offset_model_totals, baseline);
        assert_eq!(recovery.first_timestamp, 1_788_975_600);
        assert_eq!(recovery.through_timestamp, 1_788_996_000);
        assert_eq!(recovery.offset_sol_dollars, 370.814_975);
        assert_eq!(recovery.offset_luna_dollars, 1.423_482_24);
        let corrected = checked_add_model_totals(
            &recovery.source_current_model_totals,
            &recovery.offset_model_totals,
        )
        .unwrap();
        assert_eq!(
            corrected
                .iter()
                .find(|total| total.model == "SOL")
                .unwrap()
                .total_tokens,
            555_312_427
        );
        assert_eq!(
            corrected
                .iter()
                .find(|total| total.model == "LUNA")
                .unwrap()
                .total_tokens,
            25_262_756
        );
        let exact_dollars =
            recovery.offset_sol_dollars + recovery.offset_luna_dollars + 0.187_152_6;
        assert!((exact_dollars - 372.425_609_84).abs() < f64::EPSILON);

        let mut conflicting = observations;
        let mut conflict = conflicting.last().unwrap().clone();
        conflict.reset_at = reset_b;
        conflict.model_totals.as_mut().unwrap()[0].total_tokens += 1;
        conflict.luna_tokens = Some(2_446_274);
        conflicting.push(conflict);
        assert!(derive_session_cumulative_recovery(
            &partition_id,
            reset_a,
            604_800,
            1_788_996_001,
            &current,
            &conflicting,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn cumulative_recovery_does_not_cross_a_time_proven_new_window() {
        let reset_a = 1_789_437_490;
        let reset_c = 1_789_623_591;
        let old = vec![SessionModelTotal {
            model: "SOL".into(),
            total_tokens: 555_312_427,
            input_tokens: 553_537_987,
            cached_input_tokens: 544_468_480,
            output_tokens: 1_774_440,
            cache_write_input_tokens: Some(0),
        }];
        let current = vec![SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 686_397,
            input_tokens: 674_095,
            cached_input_tokens: 546_560,
            output_tokens: 12_302,
            cache_write_input_tokens: Some(0),
        }];
        let first = vec![SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 0,
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            cache_write_input_tokens: Some(0),
        }];
        let observations = vec![
            cumulative_observation(1_789_018_740, reset_a, 0.0, 1.0, 0.3, old.clone()),
            cumulative_observation(1_789_018_800, reset_c, 100.0, 0.0, 0.0, first),
            cumulative_observation(
                1_789_027_320,
                reset_c,
                93.0,
                0.0,
                0.051_200_6,
                current.clone(),
            ),
            // A stale daemon generation can publish the old period again
            // after the new period was already observed and persisted.
            cumulative_observation(1_789_030_680, reset_a, 0.0, 1.0, 0.3, old),
            cumulative_observation(
                1_789_030_800,
                reset_c,
                91.0,
                0.0,
                0.051_200_6,
                current.clone(),
            ),
        ];

        assert!(derive_session_cumulative_recovery(
            &"44".repeat(32),
            reset_c,
            604_800,
            1_789_030_801,
            &current,
            &observations,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn cumulative_recovery_offsets_only_models_proven_to_have_regressed() {
        let partition_id = "43".repeat(32);
        let reset_a = 1_789_437_490;
        let reset_b = 1_789_300_251;
        let luna_baseline = SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 50,
            input_tokens: 40,
            cached_input_tokens: 30,
            output_tokens: 10,
            cache_write_input_tokens: Some(0),
        };
        let sol_baseline = SessionModelTotal {
            model: "SOL".into(),
            total_tokens: 100,
            input_tokens: 80,
            cached_input_tokens: 60,
            output_tokens: 20,
            cache_write_input_tokens: Some(0),
        };
        let first = vec![
            SessionModelTotal {
                total_tokens: 2,
                input_tokens: 2,
                cached_input_tokens: 1,
                output_tokens: 0,
                ..luna_baseline.clone()
            },
            SessionModelTotal {
                total_tokens: 101,
                input_tokens: 81,
                cached_input_tokens: 61,
                output_tokens: 20,
                ..sol_baseline.clone()
            },
        ];
        let current = vec![
            SessionModelTotal {
                total_tokens: 5,
                input_tokens: 4,
                cached_input_tokens: 2,
                output_tokens: 1,
                ..luna_baseline.clone()
            },
            SessionModelTotal {
                total_tokens: 105,
                input_tokens: 84,
                cached_input_tokens: 63,
                output_tokens: 21,
                ..sol_baseline.clone()
            },
        ];
        let observations = vec![
            cumulative_observation(
                1_788_975_540,
                reset_b,
                17.0,
                10.0,
                1.0,
                vec![luna_baseline.clone(), sol_baseline],
            ),
            cumulative_observation(1_788_975_600, reset_a, 29.0, 10.1, 0.02, first),
            cumulative_observation(1_788_996_000, reset_a, 28.0, 10.5, 0.10, current.clone()),
        ];

        let recovery = derive_session_cumulative_recovery(
            &partition_id,
            reset_a,
            604_800,
            1_788_996_001,
            &current,
            &observations,
        )
        .unwrap()
        .expect("the LUNA regression is independently recoverable");
        assert_eq!(
            recovery.offset_model_totals,
            std::slice::from_ref(&luna_baseline)
        );
        assert_eq!(recovery.before_sol_dollars, 10.0);
        assert_eq!(recovery.before_luna_dollars, 1.0);
        assert_eq!(recovery.offset_sol_dollars, 0.0);
        assert_eq!(recovery.offset_luna_dollars, 1.0);
        let corrected = checked_add_model_totals(
            &recovery.source_current_model_totals,
            &recovery.offset_model_totals,
        )
        .unwrap();
        assert_eq!(
            corrected
                .iter()
                .find(|total| total.model == "SOL")
                .unwrap()
                .total_tokens,
            105,
            "the independently monotonic SOL counter must not be doubled"
        );
        assert_eq!(
            corrected
                .iter()
                .find(|total| total.model == "LUNA")
                .unwrap()
                .total_tokens,
            55
        );

        for partial_index in 0..observations.len() {
            let mut partial = observations.clone();
            partial[partial_index].model_totals.as_mut().unwrap()[0].cache_write_input_tokens =
                None;
            let partial_current = partial
                .last()
                .and_then(|observation| observation.model_totals.clone())
                .unwrap();
            assert!(
                derive_session_cumulative_recovery(
                    &partition_id,
                    reset_a,
                    604_800,
                    1_788_996_001,
                    &partial_current,
                    &partial,
                )
                .unwrap()
                .is_none(),
                "a missing component at recovery point {partial_index} is not exact evidence"
            );
        }

        let path = database_path("cumulative-recovery-selective-model");
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "45".repeat(16),
            account_scope_id: "46".repeat(32),
            storage_epoch: 1,
            partition_id,
        };
        let collector_epoch = 0x258;
        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        for (index, observation) in observations.iter().enumerate() {
            assert_eq!(
                commit_cumulative_point(
                    &mut store,
                    collector_epoch,
                    u64::try_from(index + 1).unwrap(),
                    observation,
                )
                .data_generation,
                u64::try_from(index + 1).unwrap()
            );
        }
        let stored_recovery = store
            .pending_session_cumulative_recovery(reset_a, 604_800, 1_788_996_001, &current)
            .unwrap()
            .expect("the stored LUNA-only regression remains recoverable");
        assert!(stored_recovery.source_generation.is_some());
        let mut stored_evidence = stored_recovery.clone();
        stored_evidence.source_generation = None;
        assert_eq!(stored_evidence, recovery);
        let committed = store
            .commit_session_collection_with_cumulative_recovery(
                SessionCollectionCommit {
                    reset_at: reset_a,
                    window_seconds: 604_800,
                    collector_epoch,
                    cycle_seq: 4,
                    samples: &[],
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &corrected,
                    recorded_sessions: &[],
                },
                &[],
                &stored_recovery,
            )
            .expect("selective model recovery must commit from the full boundary evidence");
        assert_eq!(committed.data_generation, 4);
        assert_eq!(
            store.load_session_collection_state().unwrap().model_totals,
            corrected
        );
        let projected = store.load_all().unwrap();
        let endpoint = projected
            .iter()
            .find(|sample| sample.timestamp == 1_788_996_000)
            .unwrap();
        assert_eq!(endpoint.sol_tokens, 105);
        assert_eq!(endpoint.luna_tokens, 55);
        assert_eq!(endpoint.sol_dollars, 10.5);
        assert_eq!(endpoint.luna_dollars, 1.10);
    }

    #[test]
    fn cumulative_recovery_rejects_mixed_component_regression() {
        let partition_id = "44".repeat(32);
        let reset_a = 1_789_437_490;
        let reset_b = 1_789_300_251;
        let baseline = SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 50,
            input_tokens: 40,
            cached_input_tokens: 30,
            output_tokens: 10,
            cache_write_input_tokens: Some(0),
        };
        let first = SessionModelTotal {
            total_tokens: 2,
            input_tokens: 2,
            cached_input_tokens: 1,
            // This component did not regress, so adding its baseline would
            // be a guess rather than a source-proven recovery.
            output_tokens: 10,
            ..baseline.clone()
        };
        let current = SessionModelTotal {
            total_tokens: 5,
            input_tokens: 4,
            cached_input_tokens: 2,
            output_tokens: 11,
            ..baseline.clone()
        };
        let observations = vec![
            cumulative_observation(1_788_975_540, reset_b, 17.0, 0.0, 1.0, vec![baseline]),
            cumulative_observation(1_788_975_600, reset_a, 29.0, 0.0, 0.02, vec![first]),
            cumulative_observation(
                1_788_996_000,
                reset_a,
                28.0,
                0.0,
                0.10,
                vec![current.clone()],
            ),
        ];

        assert!(derive_session_cumulative_recovery(
            &partition_id,
            reset_a,
            604_800,
            1_788_996_001,
            &[current],
            &observations,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn rejected_generation_reconciliation_uses_exact_model_component_facts() {
        let canonical = vec![
            SessionModelTotal {
                model: "LUNA".into(),
                total_tokens: 50,
                input_tokens: 40,
                cached_input_tokens: 30,
                output_tokens: 10,
                cache_write_input_tokens: Some(0),
            },
            SessionModelTotal {
                model: "SOL".into(),
                total_tokens: 100,
                input_tokens: 80,
                cached_input_tokens: 60,
                output_tokens: 20,
                cache_write_input_tokens: Some(0),
            },
            SessionModelTotal {
                model: "TERRA".into(),
                total_tokens: 7,
                input_tokens: 6,
                cached_input_tokens: 5,
                output_tokens: 1,
                cache_write_input_tokens: Some(0),
            },
        ];
        let rejected = vec![
            SessionModelTotal {
                model: "ASTRA".into(),
                total_tokens: 3,
                input_tokens: 2,
                cached_input_tokens: 1,
                output_tokens: 1,
                cache_write_input_tokens: Some(0),
            },
            SessionModelTotal {
                model: "LUNA".into(),
                total_tokens: 5,
                input_tokens: 4,
                cached_input_tokens: 2,
                output_tokens: 1,
                cache_write_input_tokens: Some(0),
            },
            SessionModelTotal {
                model: "SOL".into(),
                total_tokens: 105,
                input_tokens: 84,
                cached_input_tokens: 63,
                output_tokens: 21,
                cache_write_input_tokens: Some(0),
            },
        ];
        let merged = reconcile_rejected_generation_model_totals(&canonical, &rejected)
            .unwrap()
            .expect("every model has one exact continuation rule");
        assert_eq!(
            merged
                .iter()
                .find(|total| total.model == "LUNA")
                .unwrap()
                .total_tokens,
            55,
            "the fully regressed model carries the canonical baseline"
        );
        assert_eq!(
            merged
                .iter()
                .find(|total| total.model == "SOL")
                .unwrap()
                .total_tokens,
            105,
            "the monotonic model is already an exact absolute fact"
        );
        assert_eq!(
            merged
                .iter()
                .find(|total| total.model == "TERRA")
                .unwrap()
                .total_tokens,
            7,
            "a missing model carries its last known exact baseline"
        );
        assert_eq!(
            merged
                .iter()
                .find(|total| total.model == "ASTRA")
                .unwrap()
                .total_tokens,
            3,
            "a newly observed model remains an exact fact"
        );

        let mut mixed = rejected;
        let luna = mixed
            .iter_mut()
            .find(|total| total.model == "LUNA")
            .unwrap();
        luna.output_tokens = 10;
        assert!(
            reconcile_rejected_generation_model_totals(&canonical, &mixed)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cumulative_recovery_commit_is_atomic_bounded_and_restart_idempotent() {
        let path = database_path("cumulative-recovery-atomic");
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "4".repeat(64),
            storage_epoch: 1,
            partition_id: "4".repeat(64),
        };
        let reset_a = 1_789_437_490;
        let reset_b = 1_789_300_251;
        let collector_epoch = 0x258;
        let baseline = vec![
            SessionModelTotal {
                model: "LUNA".into(),
                total_tokens: 50,
                input_tokens: 40,
                cached_input_tokens: 30,
                output_tokens: 10,
                cache_write_input_tokens: Some(0),
            },
            SessionModelTotal {
                model: "SOL".into(),
                total_tokens: 100,
                input_tokens: 80,
                cached_input_tokens: 60,
                output_tokens: 20,
                cache_write_input_tokens: Some(0),
            },
        ];
        let first = vec![SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 2,
            input_tokens: 2,
            cached_input_tokens: 1,
            output_tokens: 0,
            cache_write_input_tokens: Some(0),
        }];
        let current = vec![SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 5,
            input_tokens: 4,
            cached_input_tokens: 2,
            output_tokens: 1,
            cache_write_input_tokens: Some(0),
        }];
        let before =
            cumulative_observation(1_788_975_540, reset_b, 17.0, 10.0, 1.0, baseline.clone());
        let after =
            cumulative_observation(1_788_975_600, reset_a + 1, 29.0, 0.0, 0.02, first.clone());
        let endpoint =
            cumulative_observation(1_788_996_000, reset_a, 28.0, 0.0, 0.10, current.clone());

        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        assert_eq!(
            commit_cumulative_point(&mut store, collector_epoch, 1, &before).data_generation,
            1
        );
        assert_eq!(
            commit_cumulative_point(&mut store, collector_epoch, 2, &after).data_generation,
            2
        );
        assert_eq!(
            commit_cumulative_point(&mut store, collector_epoch, 3, &endpoint).data_generation,
            3
        );
        let recovery = store
            .pending_session_cumulative_recovery(reset_a, 604_800, 1_788_996_001, &current)
            .unwrap()
            .expect("reset-alias regression must have one finite recovery");
        assert_eq!(recovery.offset_model_totals, baseline);
        assert_eq!(recovery.first_model_totals, first);
        let corrected = checked_add_model_totals(
            &recovery.source_current_model_totals,
            &recovery.offset_model_totals,
        )
        .unwrap();
        let raw_before = raw_cumulative_history_fingerprint(&store);

        let mut incomplete = recovery.clone();
        for totals in [
            &mut incomplete.before_model_totals,
            &mut incomplete.offset_model_totals,
            &mut incomplete.first_model_totals,
            &mut incomplete.source_current_model_totals,
        ] {
            for total in totals {
                total.cache_write_input_tokens = None;
            }
        }
        let incomplete_payload =
            cumulative_recovery_payload(&identity.partition_id, &incomplete).unwrap();
        incomplete.recovery_id = format!("{:x}", Sha256::digest(incomplete_payload.as_bytes()));
        assert!(
            validate_cumulative_recovery(&identity.partition_id, &incomplete).is_err(),
            "a recovery payload with a missing component must fail validation"
        );

        let mut invalid = recovery.clone();
        invalid.first_timestamp += 1;
        let invalid_commit = store.commit_session_collection_with_cumulative_recovery(
            SessionCollectionCommit {
                reset_at: reset_a,
                window_seconds: 604_800,
                collector_epoch,
                cycle_seq: 4,
                samples: &[],
                checkpoints: &[],
                ranges: &[],
                model_totals: &corrected,
                recorded_sessions: &[],
            },
            &[],
            &invalid,
        );
        assert!(invalid_commit.is_err());
        assert_eq!(
            store
                .load_session_collection_state()
                .unwrap()
                .data_generation,
            3
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM session_cumulative_recoveries",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(raw_cumulative_history_fingerprint(&store), raw_before);

        store
            .connection
            .execute(
                "INSERT INTO usage_history (
                    timestamp, reset_at, remaining_percent, sol_dollars,
                    terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
                 )
                 SELECT timestamp, ?1, remaining_percent, sol_dollars,
                        terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
                 FROM usage_history WHERE reset_at=?2 AND timestamp=?3",
                params![reset_a + 120, reset_a, endpoint.timestamp],
            )
            .unwrap();
        let through_conflict = store.commit_session_collection_with_cumulative_recovery(
            SessionCollectionCommit {
                reset_at: reset_a,
                window_seconds: 604_800,
                collector_epoch,
                cycle_seq: 4,
                samples: &[],
                checkpoints: &[],
                ranges: &[],
                model_totals: &corrected,
                recorded_sessions: &[],
            },
            &[],
            &recovery,
        );
        assert!(
            through_conflict.is_err(),
            "a conflicting reset alias at the recovery endpoint must reject only the recovery"
        );
        assert_eq!(
            store
                .load_session_collection_state()
                .unwrap()
                .data_generation,
            3
        );
        store
            .connection
            .execute(
                "DELETE FROM usage_history WHERE reset_at=?1 AND timestamp=?2",
                params![reset_a + 120, endpoint.timestamp],
            )
            .unwrap();
        assert_eq!(raw_cumulative_history_fingerprint(&store), raw_before);

        let committed = store
            .commit_session_collection_with_cumulative_recovery(
                SessionCollectionCommit {
                    reset_at: reset_a,
                    window_seconds: 604_800,
                    collector_epoch,
                    cycle_seq: 4,
                    samples: &[],
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &corrected,
                    recorded_sessions: &[],
                },
                &[],
                &recovery,
            )
            .unwrap();
        assert_eq!(committed.data_generation, 4);
        assert_eq!(raw_cumulative_history_fingerprint(&store), raw_before);
        assert_eq!(
            store.load_session_collection_state().unwrap().model_totals,
            corrected
        );
        let projected = store.load_all().unwrap();
        let projected_endpoint = projected
            .iter()
            .find(|sample| sample.reset_at == reset_a && sample.timestamp == endpoint.timestamp)
            .unwrap();
        assert_eq!(projected_endpoint.sol_tokens, 100);
        assert_eq!(projected_endpoint.luna_tokens, 55);
        assert_eq!(projected_endpoint.sol_dollars, 10.0);
        assert_eq!(projected_endpoint.luna_dollars, 1.10);
        let projected_before = projected
            .iter()
            .find(|sample| sample.reset_at == reset_b && sample.timestamp == before.timestamp)
            .unwrap();
        assert_eq!(projected_before.sol_tokens, 100);
        assert_eq!(projected_before.luna_tokens, 50);
        let projected_observations = store
            .load_recent_observations(Utc.timestamp_opt(1_788_996_001, 0).single().unwrap())
            .unwrap();
        let projected_models = projected_observations
            .iter()
            .find(|observation| observation.timestamp == endpoint.timestamp)
            .and_then(|observation| observation.model_totals.as_ref())
            .unwrap();
        assert_eq!(projected_models, &corrected);

        let replay = store
            .commit_session_collection_with_cumulative_recovery(
                SessionCollectionCommit {
                    reset_at: reset_a,
                    window_seconds: 604_800,
                    collector_epoch,
                    cycle_seq: 4,
                    samples: &[],
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &corrected,
                    recorded_sessions: &[],
                },
                &[],
                &recovery,
            )
            .unwrap();
        assert_eq!(replay.data_generation, 4);
        drop(store);

        let mut restarted = UsageStore::open_partitioned(&path, &identity).unwrap();
        assert_eq!(restarted.load_all().unwrap(), projected);
        assert_eq!(
            restarted
                .load_session_collection_state()
                .unwrap()
                .model_totals,
            corrected
        );
        assert!(restarted
            .pending_session_cumulative_recovery(reset_a, 604_800, 1_788_996_001, &corrected,)
            .unwrap()
            .is_none());

        let next_raw = vec![SessionModelTotal {
            total_tokens: 7,
            input_tokens: 6,
            cached_input_tokens: 3,
            output_tokens: 1,
            ..current[0].clone()
        }];
        let next_corrected = checked_add_model_totals(&next_raw, &baseline).unwrap();
        let next = cumulative_observation(
            1_788_996_060,
            reset_a,
            28.0,
            10.0,
            1.14,
            next_corrected.clone(),
        );
        assert_eq!(
            commit_cumulative_point(&mut restarted, collector_epoch, 5, &next).data_generation,
            5
        );
        let logical = restarted.load_all().unwrap();
        let logical_next = logical
            .iter()
            .find(|sample| sample.timestamp == next.timestamp)
            .unwrap();
        assert_eq!(logical_next.sol_tokens, 100);
        assert_eq!(logical_next.luna_tokens, 57);
        assert_eq!(
            restarted
                .load_session_collection_state()
                .unwrap()
                .model_totals,
            next_corrected
        );
        drop(restarted);
        cleanup(&path);
    }

    #[test]
    fn cumulative_recovery_restores_a_predeadline_corrupted_generation_atomically() {
        let path = database_path("cumulative-recovery-corrupted-generation");
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "47".repeat(16),
            account_scope_id: "48".repeat(32),
            storage_epoch: 1,
            partition_id: "49".repeat(32),
        };
        let reset_a = 1_789_437_490;
        let reset_b = 1_789_300_251;
        // This replacement has no newly started window between the adjacent
        // observations, so it remains a genuine pre-deadline corruption.
        let reset_c = reset_a + 60_000;
        let collector_epoch = 0x258;
        let baseline = vec![SessionModelTotal {
            model: "LUNA".into(),
            total_tokens: 50,
            input_tokens: 40,
            cached_input_tokens: 30,
            output_tokens: 10,
            cache_write_input_tokens: Some(0),
        }];
        let first = vec![SessionModelTotal {
            total_tokens: 2,
            input_tokens: 2,
            cached_input_tokens: 1,
            output_tokens: 0,
            ..baseline[0].clone()
        }];
        let current_a = vec![SessionModelTotal {
            total_tokens: 5,
            input_tokens: 4,
            cached_input_tokens: 2,
            output_tokens: 1,
            ..baseline[0].clone()
        }];
        let before =
            cumulative_observation(1_788_975_540, reset_b, 17.0, 0.0, 1.0, baseline.clone());
        let after = cumulative_observation(1_788_975_600, reset_a, 29.0, 0.0, 0.02, first);
        let endpoint =
            cumulative_observation(1_788_996_000, reset_a, 28.0, 0.0, 0.10, current_a.clone());
        let rejected_suffix = vec![SessionModelTotal {
            total_tokens: 2,
            input_tokens: 2,
            cached_input_tokens: 1,
            output_tokens: 0,
            ..baseline[0].clone()
        }];
        let corrupt = cumulative_observation(
            1_789_018_800,
            reset_c,
            100.0,
            0.0,
            0.04,
            rejected_suffix.clone(),
        );

        let mut store = UsageStore::create_partitioned(&path, &identity).unwrap();
        for (cycle, observation) in [&before, &after, &endpoint].into_iter().enumerate() {
            commit_cumulative_point(
                &mut store,
                collector_epoch,
                u64::try_from(cycle + 1).unwrap(),
                observation,
            );
        }
        let canonical_state = store.load_session_collection_state().unwrap();
        assert_eq!(canonical_state.reset_at, reset_a);
        commit_cumulative_point(&mut store, collector_epoch, 4, &corrupt);
        let corrupted_state = store.load_session_collection_state().unwrap();
        assert_eq!(corrupted_state.reset_at, reset_c);
        assert_eq!(corrupted_state.model_totals, rejected_suffix);
        let raw_before = raw_cumulative_history_fingerprint(&store);

        let recovery = store
            .pending_session_cumulative_recovery(
                reset_a,
                604_800,
                corrupt.timestamp + 1,
                &current_a,
            )
            .unwrap()
            .expect("the retained canonical suffix remains source-proven");
        let reconciled = reconcile_rejected_generation_model_totals(
            &recovery.source_current_model_totals,
            &corrupted_state.model_totals,
        )
        .unwrap()
        .unwrap();
        let corrected =
            checked_add_model_totals(&reconciled, &recovery.offset_model_totals).unwrap();
        let committed = store
            .commit_session_collection_with_cumulative_recovery(
                SessionCollectionCommit {
                    reset_at: reset_a,
                    window_seconds: 604_800,
                    collector_epoch,
                    cycle_seq: 5,
                    samples: &[],
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &corrected,
                    recorded_sessions: &[],
                },
                &[],
                &recovery,
            )
            .expect("one transaction restores the canonical period and cumulative vector");
        assert_eq!(committed.data_generation, 5);
        assert_eq!(raw_cumulative_history_fingerprint(&store), raw_before);
        let restored = store.load_session_collection_state().unwrap();
        assert_eq!(restored.reset_at, reset_a);
        assert_eq!(restored.window_seconds, 604_800);
        assert_eq!(restored.model_totals, corrected);
        assert_eq!(restored.model_totals[0].total_tokens, 57);
        assert_eq!(
            restored.last_quota_observation,
            Some(SessionQuotaObservation {
                observed_at: endpoint.timestamp,
                remaining_percent: 28.0,
            }),
            "the rejected future alias must not remain the accepted observation authority"
        );
        cleanup(&path);
    }

    /// PR gate for the retained production incident. The source connection is
    /// read-only and SQLite Backup creates a transactionally consistent,
    /// private copy; every migration and recovery write targets only that
    /// copy. Run with CODEX_INFO_REAL_DB_GATE set to the account database.
    #[test]
    #[ignore = "requires an explicit read-only production database source"]
    fn real_database_recovery_changes_only_the_proven_current_suffix() {
        let source_path = std::env::var_os("CODEX_INFO_REAL_DB_GATE")
            .map(PathBuf::from)
            .expect("CODEX_INFO_REAL_DB_GATE must name the source database");
        let copied_path = database_path("real-cumulative-recovery-gate");
        let source = Connection::open_with_flags(
            &source_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .expect("open source database read-only");
        source
            .pragma_update(None, "query_only", true)
            .expect("source remains query-only");
        let mut destination = Connection::open(&copied_path).expect("open isolated destination");
        {
            let backup = rusqlite::backup::Backup::new(&source, &mut destination)
                .expect("create consistent SQLite backup");
            assert!(matches!(
                backup.step(-1).expect("copy complete source snapshot"),
                rusqlite::backup::StepResult::Done
            ));
        }
        drop(destination);
        drop(source);
        #[cfg(unix)]
        fs::set_permissions(&copied_path, fs::Permissions::from_mode(0o600))
            .expect("make isolated database owner-private");

        let identity_connection = Connection::open(&copied_path).unwrap();
        let identity = identity_connection
            .query_row(
                "SELECT schema_version, profile_scope_id, account_scope_id,
                        storage_epoch, partition_id
                 FROM storage_partition WHERE singleton=1",
                [],
                |row| {
                    Ok(StoragePartitionIdentity {
                        schema_version: row.get(0)?,
                        profile_scope_id: row.get(1)?,
                        account_scope_id: row.get(2)?,
                        storage_epoch: row
                            .get::<_, String>(3)?
                            .parse()
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        partition_id: row.get(4)?,
                    })
                },
            )
            .expect("read copied partition identity");
        drop(identity_connection);

        let mut store = UsageStore::open_partitioned(&copied_path, &identity)
            .expect("migrate only the isolated copy");
        let raw_fingerprint_before = raw_cumulative_history_fingerprint(&store);
        let raw_counts_before = (
            store
                .connection
                .query_row("SELECT COUNT(*) FROM usage_history", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            store
                .connection
                .query_row("SELECT COUNT(*) FROM usage_model_history", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
        );
        let raw_samples_before = store.load_all_raw().unwrap();
        let periods_before = group_reset_periods(&raw_samples_before);
        let latest_timestamp = raw_samples_before
            .iter()
            .map(|sample| sample.timestamp)
            .max()
            .expect("production copy has history");
        let now = latest_timestamp
            .checked_add(1)
            .expect("fixture time is finite");
        let raw_observations_before = store
            .load_recent_observations_raw(Utc.timestamp_opt(now, 0).single().unwrap())
            .unwrap();
        let state_before = store.load_session_collection_state().unwrap();
        assert!(
            state_before.reset_at > now,
            "gate must target the live period"
        );

        let mut retained = Vec::new();
        for generation in 1..=3 {
            let retained_path = source_path.with_extension(format!("sqlite3.bak.{generation}"));
            match fs::symlink_metadata(&retained_path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => panic!("inspect retained generation: {error}"),
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                    panic!("retained generation is not a regular file")
                }
                Ok(_) => {}
            }
            let retained_store = UsageStore::open_read_only_partitioned(&retained_path, &identity)
                .expect("open retained generation read-only");
            retained_store
                .verify_integrity()
                .expect("retained generation passes SQLite integrity check");
            retained.push(
                retained_store
                    .load_session_collection_state()
                    .expect("load retained collection state"),
            );
        }
        let retained_authority = select_predeadline_quota_authority(&state_before, &retained);
        let (canonical_state, recovery) = if let Some(canonical) = retained_authority {
            let canonical_models = store
                .latest_raw_session_model_totals_for_period(canonical.reset_at, now)
                .unwrap()
                .expect("canonical period has one latest raw model vector");
            let recovery = store
                .pending_session_cumulative_recovery(
                    canonical.reset_at,
                    canonical.window_seconds,
                    now,
                    &canonical_models,
                )
                .unwrap()
                .expect("retained A→B→A regression has one source-proven recovery");
            (canonical, recovery)
        } else {
            let recovery = store
                .pending_session_cumulative_recovery(
                    state_before.reset_at,
                    state_before.window_seconds,
                    now,
                    &state_before.model_totals,
                )
                .unwrap()
                .expect("current period has one source-proven recovery");
            (state_before.clone(), recovery)
        };
        let baseline_sol = recovery
            .offset_model_totals
            .iter()
            .find(|total| total.model == "SOL")
            .unwrap();
        let baseline_luna = recovery
            .offset_model_totals
            .iter()
            .find(|total| total.model == "LUNA")
            .unwrap();
        assert_eq!(baseline_sol.total_tokens, 555_312_427);
        assert_eq!(baseline_luna.total_tokens, 22_816_483);
        assert_eq!(recovery.offset_sol_dollars, 370.814_975);
        assert_eq!(recovery.offset_luna_dollars, 1.423_482_24);

        let reconciled = reconcile_rejected_generation_model_totals(
            &recovery.source_current_model_totals,
            &state_before.model_totals,
        )
        .unwrap()
        .expect("canonical and rejected generation components have one interpretation");
        let corrected = checked_add_model_totals(&reconciled, &recovery.offset_model_totals)
            .expect("all current components add without overflow");
        let corrected_endpoint = checked_add_model_totals(
            &recovery.source_current_model_totals,
            &recovery.offset_model_totals,
        )
        .expect("the projected history endpoint adds without overflow");
        let collector_epoch = state_before
            .collector_epoch
            .expect("committed collector epoch");
        let next_cycle = state_before
            .cycle_seq
            .checked_add(1)
            .expect("finite cycle sequence");
        let committed = store
            .commit_session_collection_with_cumulative_recovery(
                SessionCollectionCommit {
                    reset_at: recovery.canonical_reset_at,
                    window_seconds: recovery.window_seconds,
                    collector_epoch,
                    cycle_seq: next_cycle,
                    samples: &[],
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &corrected,
                    recorded_sessions: &[],
                },
                &[],
                &recovery,
            )
            .expect("atomic recovery commit succeeds in the isolated copy");
        assert_eq!(committed.data_generation, state_before.data_generation + 1);

        let raw_fingerprint_after = raw_cumulative_history_fingerprint(&store);
        let raw_counts_after = (
            store
                .connection
                .query_row("SELECT COUNT(*) FROM usage_history", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            store
                .connection
                .query_row("SELECT COUNT(*) FROM usage_model_history", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
        );
        assert_eq!(raw_counts_after, raw_counts_before);
        assert_eq!(raw_fingerprint_after, raw_fingerprint_before);
        let state_after = store.load_session_collection_state().unwrap();
        assert_eq!(state_after.reset_at, recovery.canonical_reset_at);
        assert_eq!(state_after.window_seconds, recovery.window_seconds);
        assert_eq!(state_after.model_totals, corrected);

        let logical_samples = store.load_all().unwrap();
        assert_eq!(group_reset_periods(&logical_samples), periods_before);
        assert_eq!(logical_samples.len(), raw_samples_before.len());
        for (raw, logical) in raw_samples_before.iter().zip(&logical_samples) {
            let in_recovered_suffix = same_reset_group(raw.reset_at, recovery.canonical_reset_at)
                && raw.timestamp >= recovery.first_timestamp
                && raw.timestamp <= recovery.through_timestamp;
            if !in_recovered_suffix {
                assert_eq!(logical, raw, "unrelated period or row changed: {raw:?}");
                continue;
            }
            assert_eq!(logical.timestamp, raw.timestamp);
            assert_eq!(logical.reset_at, raw.reset_at);
            assert_eq!(logical.remaining_percent, raw.remaining_percent);
            assert_eq!(
                logical.sol_dollars,
                raw.sol_dollars + recovery.offset_sol_dollars
            );
            assert_eq!(
                logical.terra_dollars,
                raw.terra_dollars + recovery.offset_terra_dollars
            );
            assert_eq!(
                logical.luna_dollars,
                raw.luna_dollars + recovery.offset_luna_dollars
            );
        }

        let logical_observations = store
            .load_recent_observations(Utc.timestamp_opt(now, 0).single().unwrap())
            .unwrap();
        assert_eq!(logical_observations.len(), raw_observations_before.len());
        for raw in &raw_observations_before {
            let logical = logical_observations
                .iter()
                .find(|candidate| {
                    candidate.reset_at == raw.reset_at && candidate.timestamp == raw.timestamp
                })
                .expect("raw observation remains addressable");
            let in_recovered_suffix = same_reset_group(raw.reset_at, recovery.canonical_reset_at)
                && raw.timestamp >= recovery.first_timestamp
                && raw.timestamp <= recovery.through_timestamp;
            if !in_recovered_suffix || raw.model_totals.is_none() {
                assert_eq!(logical, raw, "unrelated observation changed");
            }
        }
        let endpoint = logical_observations
            .iter()
            .find(|observation| {
                observation.reset_at == recovery.through_reset_at
                    && observation.timestamp == recovery.through_timestamp
            })
            .expect("corrected current history endpoint exists");
        assert_eq!(
            endpoint.model_totals.as_deref(),
            Some(corrected_endpoint.as_slice())
        );
        assert_eq!(endpoint.sol_dollars, Some(recovery.offset_sol_dollars));
        assert_eq!(
            endpoint.luna_dollars,
            raw_observations_before
                .iter()
                .find(|observation| {
                    observation.reset_at == recovery.through_reset_at
                        && observation.timestamp == recovery.through_timestamp
                })
                .and_then(|observation| observation.luna_dollars)
                .map(|value| value + recovery.offset_luna_dollars)
        );
        assert!(store
            .pending_session_cumulative_recovery(
                recovery.canonical_reset_at,
                recovery.window_seconds,
                now,
                &corrected,
            )
            .unwrap()
            .is_none());

        println!(
            "real-db-gate periods={} raw_rows={}/{} generation={}→{} reset={}→{} retained_generation={} current_models={:?}",
            periods_before.len(),
            raw_counts_before.0,
            raw_counts_before.1,
            state_before.data_generation,
            committed.data_generation,
            state_before.reset_at,
            recovery.canonical_reset_at,
            canonical_state.data_generation,
            corrected
        );
        drop(store);
        cleanup(&copied_path);
    }
}

-- preparation source=current writer schemas + PRODUCT:153-185 + owner fixed case
-- started 2026-11-03T08:38:58Z; preparation end in attempt8-tests-manifest.json

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
    latest_quota_reset_at INTEGER NOT NULL DEFAULT 0 CHECK (latest_quota_reset_at >= 0),
    CHECK (
        collector_epoch IS NULL OR (
            length(collector_epoch) = 32
            AND collector_epoch NOT GLOB '*[^0-9a-f]*'
        )
    )
);
INSERT INTO collection_generation (
    singleton, data_generation, reset_at, window_seconds, collector_epoch, cycle_seq,
    latest_quota_reset_at
) VALUES (1, '0', 0, 0, NULL, '0', 0);

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
    history_base_pending INTEGER NOT NULL DEFAULT 0 CHECK (history_base_pending IN (0, 1)),
    context_usage_tokens TEXT,
    context_window_tokens TEXT,
    last_model_context_kind TEXT CHECK (
        last_model_context_kind IS NULL
        OR (last_model_context_kind = 'none' AND last_model IS NULL)
        OR (last_model_context_kind IN ('initial-settings', 'turn') AND last_model IS NOT NULL)
        OR last_model_context_kind = 'unknown'
    ),
    last_model_interval TEXT CHECK (
        last_model_interval IS NULL
        OR last_model_interval IN ('none', 'mixed', 'unknown')
        OR (substr(last_model_interval, 1, 6) = 'model:' AND length(last_model_interval) > 6)
    ),
    source_admission TEXT,
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

CREATE TABLE session_token_anchor_recoveries (
    recovery_id TEXT PRIMARY KEY CHECK (
        length(recovery_id) = 64
        AND recovery_id NOT GLOB '*[^0-9a-f]*'
    ),
    payload_json TEXT NOT NULL CHECK (length(payload_json) BETWEEN 2 AND 1048576)
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
    reset_at INTEGER,
    window_seconds INTEGER,
    admission_json TEXT,
    CHECK (
        (reset_at IS NULL AND window_seconds IS NULL)
        OR (reset_at IS NOT NULL AND window_seconds IS NOT NULL
            AND reset_at BETWEEN 1 AND 253402300799
            AND window_seconds > 0 AND reset_at-window_seconds >= 1
            AND admission_json IS NOT NULL)
    ),
    CHECK (admission_json IS NULL OR json_valid(admission_json)),
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
    total_dollars REAL,
    price_version TEXT,
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


CREATE TABLE IF NOT EXISTS recorded_usage_facts (
 fact_id TEXT NOT NULL PRIMARY KEY,
 source_key TEXT NOT NULL UNIQUE,
 timestamp INTEGER NOT NULL CHECK(timestamp>0),
 model TEXT NOT NULL,
 tokens_json TEXT NOT NULL,
 evidence_sha256 TEXT NOT NULL,
 source_identity TEXT NOT NULL,
 ambiguity_key TEXT,
 eligible INTEGER NOT NULL CHECK(eligible IN (0,1))
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS recorded_usage_facts_time ON recorded_usage_facts(timestamp,model,fact_id);
CREATE INDEX IF NOT EXISTS recorded_usage_facts_model ON recorded_usage_facts(model,timestamp,fact_id);
CREATE INDEX IF NOT EXISTS recorded_usage_facts_ambiguity ON recorded_usage_facts(ambiguity_key,source_identity);
CREATE TABLE IF NOT EXISTS recorded_usage_prefixes (
 model TEXT NOT NULL, timestamp INTEGER NOT NULL CHECK(timestamp>0),
 prefix_json TEXT NOT NULL, prefix_sha256 TEXT NOT NULL,
 PRIMARY KEY(model,timestamp)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS recorded_usage_prefixes_time ON recorded_usage_prefixes(timestamp,model);
CREATE TABLE IF NOT EXISTS recorded_usage_state (
 singleton INTEGER NOT NULL PRIMARY KEY CHECK(singleton=1),
 format_version INTEGER NOT NULL CHECK(format_version=1),
 initialized INTEGER NOT NULL CHECK(initialized IN (0,1)),
 fact_revision TEXT NOT NULL,
 models_json TEXT NOT NULL,
 coverage TEXT NOT NULL,
 dirty_json TEXT NOT NULL
) WITHOUT ROWID;
INSERT INTO recorded_usage_state VALUES(1,1,0,'0','[]','unknown','{}') ON CONFLICT(singleton) DO NOTHING;
CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY NOT NULL,sequence INTEGER NOT NULL UNIQUE,observed_at INTEGER NOT NULL,reset_at INTEGER NOT NULL,window_seconds INTEGER NOT NULL,remaining_percent REAL NOT NULL,applied_generation TEXT NOT NULL) WITHOUT ROWID;

PRAGMA user_version=16;
INSERT INTO storage_partition VALUES(1,'codex-info-account-db-v1','11111111111111111111111111111111','2222222222222222222222222222222222222222222222222222222222222222','13','3333333333333333333333333333333333333333333333333333333333333333',NULL);
UPDATE collection_generation SET data_generation='82',reset_at=1803021660,window_seconds=10800,latest_quota_reset_at=1803021660;
INSERT INTO quota_receipts VALUES('r76',76,1803000121,1803010860,10800,99,'82');
INSERT INTO quota_receipts VALUES('r78',78,1803000180,1803010860,10800,90,'82');
INSERT INTO quota_receipts VALUES('r79',79,1803000181,1803010865,10800,79,'82');
INSERT INTO quota_receipts VALUES('r80',80,1803000241,1803010860,10800,76,'82');
INSERT INTO quota_receipts VALUES('r81',81,1803000300,1803010920,10800,75,'82');
INSERT INTO quota_receipts VALUES('r82',82,1803010920,1803021660,10800,74,'82');
INSERT INTO usage_history VALUES(1803000120,1803010860,99,0.0,0.0,0.0,240,0,0);
INSERT INTO usage_history VALUES(1803000180,1803010860,90,0.0,0.0,0.0,240,0,0);
INSERT INTO usage_history VALUES(1803000180,1803010865,79,0.0,0.0,0.0,60,0,0);
INSERT INTO usage_history VALUES(1803000240,1803010860,76,0.0,0.0,0.0,300,0,0);
INSERT INTO usage_history VALUES(1803000300,1803010920,75,0.0,0.0,0.0,1,0,0);
INSERT INTO usage_history VALUES(1803010920,1803021660,74,0.0,0.0,0.0,0,0,0);
INSERT INTO usage_model_history VALUES(1803010860,1803000120,'gpt-6-sol','240','220','80','20','0',0,0.000496,'1791430564-183968cc652f6276');
INSERT INTO usage_model_history VALUES(1803010860,1803000180,'gpt-6-sol','240','220','80','20','0',0,0.000496,'1791430564-183968cc652f6276');
INSERT INTO usage_model_history VALUES(1803010865,1803000180,'gpt-6-sol','60','55','15','5','0',0,0.000133,'1791430564-183968cc652f6276');
INSERT INTO usage_model_history VALUES(1803010865,1803000180,'SOL','99','90','45','9','0',0,0.0001845,'1791430564-183968cc652f6276');
INSERT INTO usage_model_history VALUES(1803010860,1803000240,'gpt-6-sol','300','275','95','25','0',0,NULL,'1791430564-183968cc652f6276');
INSERT INTO usage_model_history VALUES(1803010860,1803000240,'gpt-6-luna','55','50','20','5','0',0,5.7e-06,'1791430564-183968cc652f6276');
INSERT INTO usage_model_history VALUES(1803010860,1803000240,'UNATTRIBUTED','13','12','0','1','0',0,NULL,'1791430564-183968cc652f6276');
INSERT INTO usage_model_history VALUES(1803010920,1803000300,'gpt-6-sol','1','1','0','0','0',0,NULL,'1791430564-183968cc652f6276');
INSERT INTO session_events VALUES('fixture-root','fact-0.jsonl','1','1','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',0,100,'0000000000000000000000000000000000000000000000000000000000000001',0,1803000121,'gpt-6-sol','240','220','80','20','0',1803010860,10800,'{}');
INSERT INTO recorded_usage_facts VALUES('0000000000000000000000000000000000000000000000000000000000000001','["fixture-root","fact-0.jsonl","1","1","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","0","100","0000000000000000000000000000000000000000000000000000000000000001","0"]',1803000121,'gpt-6-sol','{"total":240,"input":220,"cached_input":80,"cache_write_input":0,"output":20}','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','fixture-source',NULL,1);
INSERT INTO session_events VALUES('fixture-root','fact-1.jsonl','1','2','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',100,200,'0000000000000000000000000000000000000000000000000000000000000002',0,1803000184,'gpt-6-sol','60','55','15','5','0',1803010865,10800,'{}');
INSERT INTO recorded_usage_facts VALUES('0000000000000000000000000000000000000000000000000000000000000002','["fixture-root","fact-1.jsonl","1","2","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","100","200","0000000000000000000000000000000000000000000000000000000000000002","0"]',1803000184,'gpt-6-sol','{"total":60,"input":55,"cached_input":15,"cache_write_input":0,"output":5}','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','fixture-source',NULL,1);
INSERT INTO session_events VALUES('fixture-root','fact-2.jsonl','1','3','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',200,300,'0000000000000000000000000000000000000000000000000000000000000003',0,1803000186,'SOL','99','90','45','9','0',1803010865,10800,'{}');
INSERT INTO recorded_usage_facts VALUES('0000000000000000000000000000000000000000000000000000000000000003','["fixture-root","fact-2.jsonl","1","3","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","200","300","0000000000000000000000000000000000000000000000000000000000000003","0"]',1803000186,'gpt-6.1-sol','{"total":99,"input":90,"cached_input":45,"cache_write_input":0,"output":9}','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','fixture-source',NULL,1);
INSERT INTO session_events VALUES('fixture-root','fact-3.jsonl','1','4','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',300,400,'0000000000000000000000000000000000000000000000000000000000000004',0,1803000245,'gpt-6-luna','55','50','20','5','0',1803010860,10800,'{}');
INSERT INTO recorded_usage_facts VALUES('0000000000000000000000000000000000000000000000000000000000000004','["fixture-root","fact-3.jsonl","1","4","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","300","400","0000000000000000000000000000000000000000000000000000000000000004","0"]',1803000245,'gpt-6-luna','{"total":55,"input":50,"cached_input":20,"cache_write_input":0,"output":5}','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','fixture-source',NULL,1);
INSERT INTO session_events VALUES('fixture-root','fact-4.jsonl','1','5','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',400,500,'0000000000000000000000000000000000000000000000000000000000000005',0,1803000240,'UNATTRIBUTED','13','12','0','1','0',1803010860,10800,'{}');
INSERT INTO recorded_usage_facts VALUES('0000000000000000000000000000000000000000000000000000000000000005','["fixture-root","fact-4.jsonl","1","5","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","400","500","0000000000000000000000000000000000000000000000000000000000000005","0"]',1803000240,'UNATTRIBUTED','{"total":13,"input":12,"cached_input":0,"cache_write_input":0,"output":1}','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','fixture-source',NULL,1);
INSERT INTO session_events VALUES('fixture-root','fact-5.jsonl','1','6','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',500,600,'0000000000000000000000000000000000000000000000000000000000000006',0,1803000210,'gpt-6-sol','11000','10000','0','1000','0',NULL,NULL,NULL);
INSERT INTO recorded_usage_facts VALUES('0000000000000000000000000000000000000000000000000000000000000006','["fixture-root","fact-5.jsonl","1","6","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","500","600","0000000000000000000000000000000000000000000000000000000000000006","0"]',1803000210,'gpt-6-sol','{"total":11000,"input":10000,"cached_input":0,"cache_write_input":0,"output":1000}','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','fixture-source',NULL,1);
CREATE TRIGGER usage_history_canonical_insert_guard BEFORE INSERT ON usage_history WHEN 0 BEGIN SELECT 1; END;
CREATE TRIGGER usage_history_canonical_update_guard BEFORE UPDATE ON usage_history WHEN 0 BEGIN SELECT 1; END;
CREATE TRIGGER usage_model_history_canonical_insert_guard BEFORE INSERT ON usage_model_history WHEN 0 BEGIN SELECT 1; END;
CREATE TRIGGER usage_model_history_canonical_update_guard BEFORE UPDATE ON usage_model_history WHEN 0 BEGIN SELECT 1; END;
CREATE TRIGGER durable_history_observation_insert_guard BEFORE INSERT ON durable_state WHEN 0 BEGIN SELECT 1; END;
CREATE TRIGGER durable_history_observation_update_guard BEFORE UPDATE ON durable_state WHEN 0 BEGIN SELECT 1; END;
CREATE TRIGGER usage_history_sidecar_update_guard BEFORE UPDATE ON usage_history WHEN 0 BEGIN SELECT 1; END;
CREATE TRIGGER usage_history_sidecar_delete_guard BEFORE DELETE ON usage_history WHEN 0 BEGIN SELECT 1; END;

-- preparation_end=2026-11-03T08:49:33.813018+00:00; result=prepared_not_executed; adoption=unjudged

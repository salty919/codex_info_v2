// Reader-only first RED input derived from the period-spec-owner packet.
// This file is copied into the product worktree only by the prepared apply
// operation. It intentionally uses the existing Reader observable API and
// does not mention HistoryArchive::raw or any implementation-only field.

const PERIOD_READER_S: i64 = 1_803_000_060;
const PERIOD_READER_A: i64 = PERIOD_READER_S + 10_800;
const PERIOD_READER_B: i64 = PERIOD_READER_A + 5;
const PERIOD_READER_W: i64 = 10_800;
const PERIOD_READER_TAIL: i64 = PERIOD_READER_S + 240;
const PERIOD_READER_GROUP_ID: &str = "rg1-1803000060-10800";

fn period_reader_fixture(name: &str) -> (PathBuf, StoragePartitionIdentity) {
    let path = temp_db(name);
    Connection::open(&path)
        .unwrap()
        .execute_batch(include_str!(
            "../../../tests/fixtures/report_groups.sqlite.fixture"
        ))
        .unwrap();
    let identity = StoragePartitionIdentity {
        schema_version: "codex-info-account-db-v1".into(),
        profile_scope_id: "11".repeat(16),
        account_scope_id: "22".repeat(32),
        storage_epoch: 13,
        partition_id: "33".repeat(32),
    };
    (path, identity)
}

fn period_reader_snapshot(path: &Path, identity: &StoragePartitionIdentity) -> DbSnapshot {
    DbReader::open_partitioned(path, identity)
        .unwrap()
        .read_committed_snapshot_at(PERIOD_READER_S + 10_920)
        .unwrap()
        .snapshot
}

fn period_reader_case<F>(name: &str, mutate: F) -> DbSnapshot
where
    F: FnOnce(&Connection),
{
    let (path, identity) = period_reader_fixture(name);
    let connection = Connection::open(&path).unwrap();
    mutate(&connection);
    drop(connection);
    let snapshot = period_reader_snapshot(&path, &identity);
    fs::remove_file(path).unwrap();
    snapshot
}

fn period_reader_expect_group(snapshot: &DbSnapshot, expected_end: i64) {
    let groups = snapshot
        .retained_history
        .periods
        .iter()
        .filter(|period| period.id == PERIOD_READER_GROUP_ID)
        .collect::<Vec<_>>();
    assert_eq!(
        groups.len(),
        1,
        "canonical directory must contain one group: {groups:?}"
    );
    let group = groups[0];
    assert_eq!(
        (group.start_at, group.end_at, group.reset_at, group.current),
        (PERIOD_READER_S, expected_end, PERIOD_READER_B, false),
    );
    assert_eq!(
        snapshot.retained_history.origins.get(&PERIOD_READER_B),
        Some(&PERIOD_READER_S),
    );
}

fn period_reader_expect_one_member_period(snapshot: &DbSnapshot) {
    let periods = snapshot
        .retained_history
        .periods
        .iter()
        .filter(|period| {
            period.reset_at == PERIOD_READER_A
                || period.reset_at == PERIOD_READER_B
                || period.id == PERIOD_READER_GROUP_ID
        })
        .collect::<Vec<_>>();
    assert_eq!(
        periods.len(),
        1,
        "same display key must have one public period, not duplicates: {periods:?}"
    );
}

fn period_reader_sample(snapshot: &DbSnapshot, timestamp: i64) -> &PublicHistoryObservationV3 {
    let samples = snapshot
        .retained_history
        .samples
        .iter()
        .filter(|sample| sample.timestamp == timestamp && sample.reset_at == PERIOD_READER_B)
        .collect::<Vec<_>>();
    assert_eq!(
        samples.len(),
        1,
        "canonical projection must expose one transport sample at {timestamp}: {samples:?}"
    );
    samples[0]
}

fn period_reader_model<'a>(
    sample: &'a PublicHistoryObservationV3,
    model: &str,
) -> &'a PublicHistoryModelUsageV3 {
    sample
        .models
        .as_ref()
        .and_then(|models| models.iter().find(|candidate| candidate.model == model))
        .unwrap_or_else(|| {
            panic!(
                "missing model {model} at {}: {:?}",
                sample.timestamp, sample.models
            )
        })
}

struct PeriodReaderFact<'a> {
    id_char: char,
    record_char: char,
    evidence_char: char,
    file_name: &'a str,
    inode: &'a str,
    range_start: i64,
    range_end: i64,
    timestamp: i64,
    model: &'a str,
    tokens_json: &'a str,
}

fn period_reader_insert_fact(connection: &Connection, fact: PeriodReaderFact<'_>) {
    let PeriodReaderFact {
        id_char,
        record_char,
        evidence_char,
        file_name,
        inode,
        range_start,
        range_end,
        timestamp,
        model,
        tokens_json,
    } = fact;
    let fact_id = id_char.to_string().repeat(64);
    let prefix_generation = "a".repeat(32);
    let record_sha = record_char.to_string().repeat(64);
    let evidence_sha = evidence_char.to_string().repeat(64);
    let source_key = format!(
        "[\"fixture-root\",\"{file_name}\",\"1\",\"{inode}\",\"{prefix_generation}\",\"{range_start}\",\"{range_end}\",\"{record_sha}\",\"0\"]"
    );
    connection
        .execute(
            "INSERT INTO session_events VALUES(?1,?2,'1',?3,?4,?5,?6,?7,0,?8,?9,'10','8','2','0','0',?10,?11,'{}')",
            params![
                "fixture-root",
                file_name,
                inode,
                prefix_generation,
                range_start,
                range_end,
                record_sha,
                timestamp,
                model,
                PERIOD_READER_B,
                PERIOD_READER_W,
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO recorded_usage_facts VALUES(?1,?2,?3,?4,?5,?6,'fixture-source',NULL,1)",
            params![
                fact_id,
                source_key,
                timestamp,
                model,
                tokens_json,
                evidence_sha,
            ],
        )
        .unwrap();
}

fn period_reader_add_non_containing_sample(connection: &Connection) {
    connection
        .execute(
            "INSERT INTO usage_history VALUES(?1,?2,79.0,0.0,0.0,0.0,0,0,0)",
            params![PERIOD_READER_TAIL, PERIOD_READER_B],
        )
        .unwrap();
}

fn period_reader_add_legacy_positive_mismatch(connection: &Connection) {
    connection
        .execute(
            "UPDATE usage_model_history SET total_tokens='9999',input_tokens='9999',cached_input_tokens='0',output_tokens='0' WHERE reset_at=?1 AND timestamp=?2 AND model='gpt-6-sol'",
            params![PERIOD_READER_B, PERIOD_READER_S + 120],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO usage_model_history VALUES(?1,?2,'legacy-only','42','42','0','0','0',0,0.000042,NULL)",
            params![PERIOD_READER_B, PERIOD_READER_S + 120],
        )
        .unwrap();
}

type PeriodReaderModelRow = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    Option<u64>,
    Option<String>,
);
type PeriodReaderHistoryRow = (i64, i64, Option<u64>, u64, u64, u64, i64, i64, i64);

#[derive(Debug, PartialEq)]
struct PeriodReaderSourceRows {
    model: Vec<PeriodReaderModelRow>,
    history: Vec<PeriodReaderHistoryRow>,
}

fn period_reader_source_rows(path: &Path) -> PeriodReaderSourceRows {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let model = {
        let mut query = connection
            .prepare(
                "SELECT reset_at,timestamp,model,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens,model_set_complete,total_dollars,price_version FROM usage_model_history ORDER BY reset_at,timestamp,model",
            )
            .unwrap();
        query
            .query_map([], |row| {
                let dollars = row.get::<_, Option<f64>>(9)?.map(|value| value.to_bits());
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    dollars,
                    row.get(10)?,
                ))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    };
    let history = {
        let mut query = connection
            .prepare(
                "SELECT timestamp,reset_at,remaining_percent,sol_dollars,terra_dollars,luna_dollars,sol_tokens,terra_tokens,luna_tokens FROM usage_history ORDER BY reset_at,timestamp",
            )
            .unwrap();
        query
            .query_map([], |row| {
                let remaining = row.get::<_, Option<f64>>(2)?.map(|value| value.to_bits());
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    remaining,
                    row.get::<_, f64>(3)?.to_bits(),
                    row.get::<_, f64>(4)?.to_bits(),
                    row.get::<_, f64>(5)?.to_bits(),
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    };
    PeriodReaderSourceRows { model, history }
}

#[test]
fn period_groups_identity_ranges_and_fact_tail() {
    let baseline = period_reader_case("period-groups-baseline", |_| {});
    let non_containing = period_reader_case("period-groups-non-containing", |connection| {
        period_reader_add_non_containing_sample(connection);
    });
    period_reader_expect_one_member_period(&non_containing);
    period_reader_expect_group(&non_containing, PERIOD_READER_TAIL);
    period_reader_expect_group(&baseline, PERIOD_READER_S + 185);

    let fact_tail = period_reader_case("period-groups-fact-tail", |connection| {
        period_reader_insert_fact(
            connection,
            PeriodReaderFact {
                id_char: '7',
                record_char: 'c',
                evidence_char: 'd',
                file_name: "fact-tail.jsonl",
                inode: "7",
                range_start: 600,
                range_end: 700,
                timestamp: PERIOD_READER_TAIL,
                model: "gpt-6-tail",
                tokens_json: r#"{"total":7,"input":7,"cached_input":0,"cache_write_input":0,"output":0}"#,
            },
        );
    });
    period_reader_expect_group(&fact_tail, PERIOD_READER_TAIL);
    let tail = period_reader_sample(&fact_tail, PERIOD_READER_TAIL);
    assert_eq!(tail.reset_at, PERIOD_READER_B);
    assert_eq!(period_reader_model(tail, "gpt-6-tail").total_tokens, 7);

    let mismatch = period_reader_case("period-groups-legacy-mismatch", |connection| {
        period_reader_add_legacy_positive_mismatch(connection);
    });
    period_reader_expect_group(&mismatch, PERIOD_READER_S + 185);
    let legacy_point = period_reader_sample(&mismatch, PERIOD_READER_S + 120);
    assert!(!legacy_point
        .models
        .as_ref()
        .is_some_and(|models| { models.iter().any(|model| model.model == "legacy-only") }));

    for candidate in [&non_containing, &fact_tail, &mismatch] {
        assert_eq!(candidate.current, baseline.current);
        assert_eq!(candidate.details.quota, baseline.details.quota);
    }
}

#[test]
fn period_groups_unique_prefix_unknowns_and_prices() {
    let saved = 0.0001845_f64;
    let snapshot = period_reader_case("period-groups-prefixes", |connection| {
        period_reader_insert_fact(
            connection,
            PeriodReaderFact {
                id_char: '8',
                record_char: 'e',
                evidence_char: 'f',
                file_name: "fact-partial.jsonl",
                inode: "8",
                range_start: 700,
                range_end: 800,
                timestamp: PERIOD_READER_TAIL,
                model: "partial-model",
                tokens_json: r#"{"total":10,"input":8,"cached_input":2,"cache_write_input":null,"output":null}"#,
            },
        );
    });
    let tail_samples = snapshot
        .retained_history
        .samples
        .iter()
        .filter(|sample| {
            sample.timestamp == PERIOD_READER_TAIL && sample.reset_at == PERIOD_READER_B
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tail_samples.len(),
        1,
        "fact-only tail must be visible as one sample: {tail_samples:?}"
    );
    let partial = period_reader_sample(&snapshot, PERIOD_READER_TAIL);
    let partial_model = period_reader_model(partial, "partial-model");
    assert_eq!(
        (
            partial_model.total_tokens,
            partial_model.input_tokens,
            partial_model.cached_input_tokens,
            partial_model.output_tokens,
            partial_model.total_dollars,
        ),
        (10, Some(8), Some(2), None, None),
    );
    assert!(!partial.models_complete);
    period_reader_expect_group(&snapshot, PERIOD_READER_TAIL);

    let at_60 = period_reader_sample(&snapshot, PERIOD_READER_S + 60);
    assert_eq!(period_reader_model(at_60, "gpt-6-sol").total_tokens, 240);

    let at_120 = period_reader_sample(&snapshot, PERIOD_READER_S + 120);
    let sol = period_reader_model(at_120, "gpt-6-sol");
    assert_eq!(
        (
            sol.total_tokens,
            sol.input_tokens,
            sol.cached_input_tokens,
            sol.cache_write_input_tokens,
            sol.output_tokens,
        ),
        (300, Some(275), Some(95), Some(0), Some(25)),
    );
    let ordinary = (180_f64 / 1_000_000.) * 2.;
    let cached = (95_f64 / 1_000_000.) * 0.2;
    let write = (0_f64 / 1_000_000.) * 2.5;
    let output = (25_f64 / 1_000_000.) * 10.;
    let expected = ((ordinary + cached) + write) + output;
    assert_eq!(sol.total_dollars.unwrap().to_bits(), expected.to_bits());

    let at_180 = period_reader_sample(&snapshot, PERIOD_READER_S + 180);
    let exact = period_reader_model(at_180, "gpt-6.1-sol");
    assert_eq!(exact.total_tokens, 99);
    assert_eq!(exact.total_dollars.unwrap().to_bits(), saved.to_bits());
    assert_eq!(period_reader_model(at_180, "gpt-6-luna").total_tokens, 55);
    assert_eq!(
        period_reader_model(at_180, "UNATTRIBUTED").total_dollars,
        None
    );

    assert!(!snapshot.retained_history.samples.iter().any(|sample| {
        sample.models.as_ref().is_some_and(|models| {
            models
                .iter()
                .any(|model| model.model == "gpt-6-sol" && model.total_tokens >= 11_000)
        })
    }));
}

#[test]
fn period_groups_preserve_source_rows() {
    let raw_saved = f64::from_bits(0x3ff01a35a0cbbd03);
    let (path, identity) = period_reader_fixture("period-groups-source-rows");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE usage_model_history SET total_dollars=?1 WHERE reset_at=?2 AND timestamp=?3 AND model='SOL'",
            params![raw_saved, PERIOD_READER_B, PERIOD_READER_S + 120],
        )
        .unwrap();
    period_reader_add_legacy_positive_mismatch(&connection);
    drop(connection);

    let before = period_reader_source_rows(&path);
    let _snapshot = period_reader_snapshot(&path, &identity);
    let after = period_reader_source_rows(&path);
    fs::remove_file(path).unwrap();

    assert_eq!(before, after);
    assert!(before.model.iter().any(|row| {
        row.0 == PERIOD_READER_B
            && row.1 == PERIOD_READER_S + 120
            && row.2 == "SOL"
            && row.9 == Some(raw_saved.to_bits())
    }));
}

fn period_reader_add_unqualified_source_quota(connection: &Connection) {
    let points = [
        (1_803_000_300_i64, Some(100.0_f64)),
        (1_803_000_360_i64, Some(40.0_f64)),
        (1_803_000_420_i64, None),
    ];
    for (index, (timestamp, remaining)) in points.into_iter().enumerate() {
        let minute = timestamp - timestamp.rem_euclid(60);
        let receipt_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM quota_receipts
                 WHERE reset_at IN (?1,?2)
                   AND observed_at>=?3 AND observed_at<?3+60",
                params![PERIOD_READER_A, PERIOD_READER_B, minute],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            receipt_count, 0,
            "test input must have no same-minute A/B receipt"
        );

        connection
            .execute(
                "INSERT INTO usage_history VALUES(?1,?2,?3,0.0,0.0,0.0,0,0,0)",
                params![timestamp, PERIOD_READER_B, remaining],
            )
            .unwrap();

        let total = (index + 1).to_string();
        connection
            .execute(
                "INSERT INTO usage_model_history VALUES(?1,?2,'gpt-6-sol',?3,?3,'0','0','0',1,NULL,NULL)",
                params![PERIOD_READER_B, timestamp, total],
            )
            .unwrap();

        let remaining_json = remaining
            .map(|value| value.to_string())
            .unwrap_or_else(|| "null".to_owned());
        let snapshot_json = format!(
            r#"{{"kind":"codex-info-usage-observation-v1","timestamp":{timestamp},"reset_at":{PERIOD_READER_B},"remaining_percent":{remaining_json},"model_source":"confirmed"}}"#,
        );
        let data_hash = format!("{:064x}", index + 1);
        connection
            .execute(
                "INSERT INTO durable_state(singleton,data_generation,data_hash,snapshot_json)
                 VALUES(?1,?2,?3,?4)",
                params![9000 + index as i64, timestamp, data_hash, snapshot_json],
            )
            .unwrap();
    }
}

#[test]
fn period_groups_unqualified_source_quota_stays_raw_only() {
    let expected = [
        (1_803_000_300_i64, Some(100.0_f64)),
        (1_803_000_360_i64, Some(40.0_f64)),
        (1_803_000_420_i64, None),
    ];
    let snapshot = period_reader_case("period-groups-unqualified-source-quota", |connection| {
        period_reader_add_unqualified_source_quota(connection);
    });
    let raw = snapshot
        .retained_history
        .raw
        .as_ref()
        .expect("retained history must keep the raw archive");

    for (timestamp, raw_remaining) in expected {
        let canonical = period_reader_sample(&snapshot, timestamp);
        assert_eq!(
            canonical.remaining_percent, None,
            "unqualified source quota must not be published canonically at {timestamp}"
        );

        let raw_samples = raw
            .samples
            .iter()
            .filter(|sample| sample.timestamp == timestamp && sample.reset_at == PERIOD_READER_B)
            .collect::<Vec<_>>();
        assert_eq!(
            raw_samples.len(),
            1,
            "raw archive must retain one original sample at {timestamp}: {raw_samples:?}"
        );
        assert_eq!(raw_samples[0].remaining_percent, raw_remaining);
        assert_eq!(raw_samples[0].model_source, "confirmed");
    }
}

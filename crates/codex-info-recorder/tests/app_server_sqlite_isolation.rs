#![cfg(unix)]

use codex_info_db_writer::SessionCheckpoint;
use codex_info_recorder::{
    probe_codex_authentication_state, ActiveThreadPollResult, CodexAuthenticationState,
    QuotaPoller, ThreadPoller,
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

static ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());

struct QuotaJournalFixture {
    _root: tempfile::TempDir,
    child: ChildGuard,
    calls: PathBuf,
    database: PathBuf,
    expected: Value,
}

impl QuotaJournalFixture {
    fn start(malformed: bool, storage_failure: bool, responses: usize) -> Self {
        Self::start_process(malformed, storage_failure, responses, None)
    }

    fn start_process(
        malformed: bool,
        storage_failure: bool,
        responses: usize,
        resident_interval: Option<u64>,
    ) -> Self {
        let root = tempfile::tempdir().expect("quota fixture root");
        let home = root.path().join("home");
        let data = root.path().join("data");
        private_directory(&home);
        private_directory(&data);
        private_directory(&home.join("sessions"));
        let auth = home.join("auth.json");
        fs::write(
            &auth,
            br#"{"tokens":{"account_id":"fixture-private-account"}}"#,
        )
        .unwrap();
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o600)).unwrap();
        codex_info_account_locator::ensure_partition(&home, &data).unwrap();
        let journal = data.join("history/quota");
        private_directory(&journal);
        let database = journal.join("observations.sqlite3");
        if storage_failure {
            fs::create_dir(&database).unwrap();
        }
        let expected = json!({"rateLimits":{"primary":{
            "usedPercent": if malformed { json!("future-format") } else { json!(1) },
            "resetsAt":1792295861_i64,"windowDurationMins":10080
        }},"futureField":{"retained":true}});
        let payload = root.path().join("quota.json");
        fs::write(&payload, expected.to_string()).unwrap();
        let calls = root.path().join("calls");
        let server = root.path().join("server.py");
        fs::write(&server, r#"#!/usr/bin/env python3
import json, os, sys, time
for line in sys.stdin:
    req = json.loads(line)
    if 'id' not in req:
        continue
    result = {}
    if req['method'] == 'account/read':
        result = {'requiresOpenaiAuth':False,'account':{'type':'chatgpt','email':'fixture@example.invalid','planType':'pro'}}
    if req['method'] == 'account/rateLimits/read':
        calls = os.environ['QUOTA_FIXTURE_CALLS']
        count = len(open(calls).readlines()) if os.path.exists(calls) else 0
        with open(calls, 'a') as f:
            f.write(str(count) + '\n')
        if count >= int(os.environ['QUOTA_FIXTURE_RESPONSES']):
            time.sleep(30)
            break
        with open(os.environ['QUOTA_FIXTURE_PAYLOAD']) as f:
            result = json.load(f)
        result['fixtureSequence'] = count
    print(json.dumps({'jsonrpc':'2.0','id':req['id'],'result':result}), flush=True)
"#).unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = match resident_interval {
            Some(interval) => {
                let mut command = Command::new(env!("CARGO_BIN_EXE_codex_info_recorder"));
                command.process_group(0);
                command.args(["--interval-secs", &interval.to_string()]);
                command
            }
            None => {
                let mut command = Command::new(env::current_exe().unwrap());
                command.args(["--exact", "quota_journal_hold_poller", "--ignored"]);
                command
            }
        };
        let child = ChildGuard(
            command
                .env("CODEX_HOME", home)
                .env("CODEX_INFO_DATA_DIR", data)
                .env("CODEX_INFO_CODEX_BIN", server)
                .env("QUOTA_FIXTURE_CALLS", &calls)
                .env("QUOTA_FIXTURE_PAYLOAD", payload)
                .env("QUOTA_FIXTURE_RESPONSES", responses.to_string())
                .env("QUOTA_FIXTURE_OWNER_ROOT", root.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(File::create(root.path().join("owner.log")).unwrap())
                .spawn()
                .unwrap(),
        );
        Self {
            _root: root,
            child,
            calls,
            database,
            expected,
        }
    }

    fn calls(&self) -> usize {
        fs::read_to_string(&self.calls)
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn wait_for_calls(&mut self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while self.calls() < count {
            assert!(
                self.child.0.try_wait().unwrap().is_none(),
                "fixture child exited"
            );
            assert!(
                Instant::now() < deadline,
                "fixture did not reach quota call {count}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn assert_saved(&self, count: usize) {
        assert!(
            self.database.is_file(),
            "received quota responses have no durable journal"
        );
        let db =
            Connection::open_with_flags(&self.database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let mut stmt = db.prepare("SELECT payload, account_scope, observed_at FROM quota_observations ORDER BY sequence").unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), count, "every received response must survive");
        for (index, (payload, scope, observed)) in rows.into_iter().enumerate() {
            let mut expected = self.expected.clone();
            expected["fixtureSequence"] = json!(index);
            assert_eq!(serde_json::from_str::<Value>(&payload).unwrap(), expected);
            assert!(!scope.is_empty());
            assert!(!scope.contains("fixture-private-account"));
            assert!(!payload.contains("fixture@example.invalid"));
            assert!(observed > 0);
        }
        assert_eq!(fs::metadata(&self.database).unwrap().mode() & 0o777, 0o600);
    }
}

fn send_sigterm_and_wait(fixture: &mut QuotaJournalFixture, restore_storage: bool) {
    assert!(Command::new("kill")
        .args(["-TERM", "--", &format!("-{}", fixture.child.0.id())])
        .status()
        .unwrap()
        .success());
    if restore_storage {
        thread::sleep(Duration::from_millis(200));
        assert!(
            fixture.child.0.try_wait().unwrap().is_none(),
            "SIGTERM discarded the unsaved API response"
        );
        fs::remove_dir(&fixture.database).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = fixture.child.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "SIGTERM must finish through normal cleanup: {status}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "SIGTERM did not wake the recorder"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fixture.calls(), 1);
    fixture.assert_saved(1);
}

#[test]
fn quota_resident_sigterm_drains_unsaved_response() {
    let mut fixture = QuotaJournalFixture::start_process(false, true, 2, Some(60));
    fixture.wait_for_calls(1);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fs::read_to_string(fixture._root.path().join("owner.log"))
        .unwrap()
        .contains("quota acquisition journal could not be committed")
    {
        assert!(
            Instant::now() < deadline,
            "fixture did not reach blocked persistence"
        );
        thread::sleep(Duration::from_millis(20));
    }
    send_sigterm_and_wait(&mut fixture, true);
}

#[test]
fn quota_resident_sigterm_wakes_long_interval() {
    let mut fixture = QuotaJournalFixture::start_process(false, false, 2, Some(3600));
    fixture.wait_for_calls(1);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let saved = Connection::open_with_flags(
            &fixture.database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|db| {
            db.query_row(
                "SELECT count(*) FROM quota_observations WHERE account_verified=1",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .unwrap_or(0);
        let log = fs::read_to_string(fixture._root.path().join("owner.log")).unwrap();
        if saved == 1
            && (log.contains("awaiting durable current-account quota")
                || log.contains("acknowledged generation="))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fixture recorder did not reach its scheduled wait"
        );
        thread::sleep(Duration::from_millis(20));
    }
    send_sigterm_and_wait(&mut fixture, false);
}

#[test]
#[ignore = "isolated child owns the quota poller and its environment"]
fn quota_journal_hold_poller() {
    let poller = QuotaPoller::start_with_interval(1);
    let owner_root = PathBuf::from(env::var_os("QUOTA_FIXTURE_OWNER_ROOT").unwrap());
    // Deliberately never drain the capacity-two result queue.
    loop {
        if owner_root.join("stop-owner").exists() {
            fs::write(owner_root.join("stopping-owner"), b"requested").unwrap();
            drop(poller);
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn quota_journal_normal_exit_drains_unsaved_response() {
    let mut fixture = QuotaJournalFixture::start(false, true, 2);
    fixture.wait_for_calls(1);
    // The blocked storage path keeps the first response owned in memory.
    thread::sleep(Duration::from_millis(1400));
    let root = fixture._root.path();
    fs::write(root.join("stop-owner"), b"stop").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !root.join("stopping-owner").exists() {
        assert!(Instant::now() < deadline, "owner did not begin shutdown");
        thread::sleep(Duration::from_millis(20));
    }
    thread::sleep(Duration::from_millis(200));
    assert!(
        fixture.child.0.try_wait().unwrap().is_none(),
        "normal exit discarded ownership of the unsaved API response"
    );
    fs::remove_dir(&fixture.database).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = fixture.child.0.try_wait().unwrap() {
            assert!(status.success(), "owner did not exit normally");
            break;
        }
        assert!(Instant::now() < deadline, "owner did not finish saving");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fixture.calls(), 1, "shutdown must not start another fetch");
    fixture.assert_saved(1);
}

#[test]
fn quota_journal_preserves_every_response_before_queue_delivery() {
    let mut fixture = QuotaJournalFixture::start(false, false, 3);
    fixture.wait_for_calls(4);
    fixture.assert_saved(3);
}

#[test]
fn quota_journal_preserves_response_before_quota_interpretation() {
    let mut fixture = QuotaJournalFixture::start(true, false, 1);
    fixture.wait_for_calls(2);
    fixture.assert_saved(1);
}

#[test]
fn quota_journal_retries_unsaved_response_before_fetching_another() {
    let mut fixture = QuotaJournalFixture::start(false, true, 2);
    fixture.wait_for_calls(1);
    thread::sleep(Duration::from_millis(1400));
    assert!(fixture.child.0.try_wait().unwrap().is_none());
    assert_eq!(
        fixture.calls(),
        1,
        "storage failure must retain the response, not replace it with a new fetch"
    );
    fs::remove_dir(&fixture.database).unwrap();
    fixture.wait_for_calls(3);
    fixture.assert_saved(2);
}

fn replay_partition(fixture: &QuotaJournalFixture) -> PathBuf {
    let root = fixture._root.path();
    let partition =
        codex_info_account_locator::ensure_partition(root.join("home"), root.join("data")).unwrap();
    if !partition.database_path.exists() {
        codex_info_db_writer::UsageStore::create_partitioned(
            &partition.database_path,
            &codex_info_db_writer::StoragePartitionIdentity {
                schema_version: "codex-info-account-db-v1".into(),
                profile_scope_id: partition.profile_scope_id.clone(),
                account_scope_id: partition.account_scope_id.clone(),
                storage_epoch: partition.storage_epoch,
                partition_id: partition.partition_id.clone(),
            },
        )
        .unwrap();
    }
    partition.database_path
}

fn run_quota_replay(fixture: &QuotaJournalFixture) -> String {
    let root = fixture._root.path();
    private_directory(&root.join("home/sessions"));
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_codex_info_recorder"))
            .arg("--once")
            .env("CODEX_HOME", root.join("home"))
            .env("CODEX_INFO_DATA_DIR", root.join("data"))
            .env("CODEX_INFO_CODEX_BIN", root.join("absent-app-server"))
            .env_remove("CODEX_INFO_RECORDER_DB")
            .env_remove("CODEX_INFO_ACCOUNT_DB")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(root.join("replay.log")).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    while child.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "recorder replay did not terminate"
        );
        thread::sleep(Duration::from_millis(20));
    }
    fs::read_to_string(root.join("replay.log")).unwrap()
}

fn accepted_quota_receipts(database: &Path) -> Vec<(String, i64, f64)> {
    let db =
        Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let present: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='quota_receipts')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(present, "durable quota replay has no account receipt store");
    let mut statement = db
        .prepare(
            "SELECT receipt_id, sequence, remaining_percent FROM quota_receipts ORDER BY sequence",
        )
        .unwrap();
    statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn quota_replay_restores_receipts_after_restart_without_new_api_response() {
    let mut fixture = QuotaJournalFixture::start(false, false, 3);
    fixture.wait_for_calls(4);
    fixture.child.0.kill().unwrap();
    fixture.child.0.wait().unwrap();
    let database = replay_partition(&fixture);
    run_quota_replay(&fixture);
    let first = accepted_quota_receipts(&database);
    assert_eq!(first.len(), 3);
    assert!(first.iter().all(|(_, _, remaining)| *remaining == 99.0));
    assert!(first.windows(2).all(|pair| pair[0].1 < pair[1].1));
    run_quota_replay(&fixture);
    assert_eq!(
        accepted_quota_receipts(&database),
        first,
        "restart must not apply a receipt twice"
    );
    fixture.assert_saved(3);
}

#[test]
fn quota_replay_restores_period_after_restart_without_new_api_response() {
    let mut fixture = QuotaJournalFixture::start(false, false, 3);
    fixture.wait_for_calls(4);
    fixture.child.0.kill().unwrap();
    fixture.child.0.wait().unwrap();
    let database = replay_partition(&fixture);
    let log = run_quota_replay(&fixture);
    assert_eq!(accepted_quota_receipts(&database).len(), 3);
    let db =
        Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let period: (i64, i64, i64) = db.query_row(
        "SELECT reset_at, latest_quota_reset_at, window_seconds FROM collection_generation WHERE singleton=1",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(period, (1_792_295_861, 1_792_295_861, 604_800), "{log}");
    let quota: (i64, f64) = db
        .query_row(
            "SELECT reset_at, remaining_percent FROM usage_history ORDER BY timestamp DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(quota, (1_792_295_861, 99.0));
    let state_path = fixture
        ._root
        .path()
        .join("data/history/recorder-state.json");
    assert!(
        state_path.is_file(),
        "durable current-account quota must publish after restart: {log}"
    );
    let state: Value = serde_json::from_slice(&fs::read(state_path).unwrap()).unwrap();
    let generation: String = db
        .query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        state["data_generation"].as_u64(),
        Some(generation.parse::<u64>().unwrap()),
        "{log}"
    );
    assert_eq!(
        state["write_state"], "degraded",
        "no fresh API success is claimed"
    );

    fixture.assert_saved(3);
}

#[test]
fn quota_replay_retains_unreadable_input_and_accepts_later_receipts() {
    let mut fixture = QuotaJournalFixture::start(false, false, 3);
    fixture.wait_for_calls(4);
    fixture.child.0.kill().unwrap();
    fixture.child.0.wait().unwrap();
    let database = replay_partition(&fixture);
    let raw = Connection::open(&fixture.database).unwrap();
    raw.execute("UPDATE quota_observations SET payload='{' WHERE sequence=(SELECT MIN(sequence) FROM quota_observations)", []).unwrap();
    let originals: Vec<(String, i64, String)> = raw
        .prepare("SELECT receipt_id, sequence, payload FROM quota_observations ORDER BY sequence")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    Connection::open(&database)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER quota_health_atomic BEFORE UPDATE ON collection_generation
         WHEN EXISTS(SELECT 1 FROM quota_receipts)
           AND NOT EXISTS(SELECT 1 FROM quota_acquisition_state WHERE singleton=1 AND degraded=1)
         BEGIN SELECT RAISE(ABORT, 'quota receipt published before unresolved state'); END;",
        )
        .unwrap();
    let log = run_quota_replay(&fixture);
    let expected: Vec<_> = originals
        .iter()
        .skip(1)
        .map(|(id, sequence, _)| (id.clone(), *sequence, 99.0))
        .collect();
    assert_eq!(accepted_quota_receipts(&database), expected, "{log}");
    assert!(log.contains("quota replay unresolved=1"), "{log}");
    let account = Connection::open(&database).unwrap();
    let has_health: bool = account
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='quota_acquisition_state')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        has_health,
        "quota replay failure must have durable status independent of threads"
    );
    let degraded: i64 = account
        .query_row(
            "SELECT degraded FROM quota_acquisition_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        degraded, 1,
        "unresolved raw input must remain publicly degraded"
    );

    let retained: Vec<(String, i64, String)> = raw
        .prepare("SELECT receipt_id, sequence, payload FROM quota_observations ORDER BY sequence")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(retained, originals, "replay must preserve unreadable input");
}

#[test]
fn quota_replay_reports_unverified_input_without_assigning_it() {
    let mut fixture = QuotaJournalFixture::start(false, false, 3);
    fixture.wait_for_calls(4);
    fixture.child.0.kill().unwrap();
    fixture.child.0.wait().unwrap();
    let database = replay_partition(&fixture);
    let raw = Connection::open(&fixture.database).unwrap();
    raw.execute("UPDATE quota_observations SET account_verified=0 WHERE sequence=(SELECT MIN(sequence) FROM quota_observations)", []).unwrap();
    let expected: Vec<(String, i64, f64)> = raw.prepare(
        "SELECT receipt_id, sequence, 99.0 FROM quota_observations WHERE account_verified=1 ORDER BY sequence"
    ).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap().collect::<Result<_, _>>().unwrap();
    let log = run_quota_replay(&fixture);
    assert_eq!(accepted_quota_receipts(&database), expected);
    assert!(
        log.contains("quota replay unresolved=1"),
        "unverified input must remain visible: {log}"
    );
    let unverified: i64 = raw
        .query_row(
            "SELECT count(*) FROM quota_observations WHERE account_verified=0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unverified, 1);
    fixture.assert_saved(3);
}

#[test]
fn quota_resident_keeps_replay_failure_until_raw_is_reprocessed() {
    let mut fixture = QuotaJournalFixture::start(false, false, 3);
    fixture.wait_for_calls(4);
    fixture.child.0.kill().unwrap();
    fixture.child.0.wait().unwrap();
    let database = replay_partition(&fixture);
    let raw = Connection::open(&fixture.database).unwrap();
    let (first_id, original): (String, String) = raw
        .query_row(
            "SELECT receipt_id,payload FROM quota_observations ORDER BY sequence LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    raw.execute(
        "UPDATE quota_observations SET payload='{}' WHERE receipt_id=?1",
        [&first_id],
    )
    .unwrap();
    let root = fixture._root.path();
    private_directory(&root.join("home/sessions"));
    let log_path = root.join("resident.log");
    let mut resident = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_codex_info_recorder"))
            .args(["--interval-secs", "1"])
            .env("CODEX_HOME", root.join("home"))
            .env("CODEX_INFO_DATA_DIR", root.join("data"))
            .env("CODEX_INFO_CODEX_BIN", root.join("server.py"))
            .env("QUOTA_FIXTURE_CALLS", &fixture.calls)
            .env("QUOTA_FIXTURE_PAYLOAD", root.join("quota.json"))
            .env("QUOTA_FIXTURE_RESPONSES", "6")
            .env_remove("CODEX_INFO_RECORDER_DB")
            .env_remove("CODEX_INFO_ACCOUNT_DB")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&log_path).unwrap())
            .spawn()
            .unwrap(),
    );
    let account = Connection::open(&database).unwrap();
    account.busy_timeout(Duration::from_secs(1)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        assert!(resident.0.try_wait().unwrap().is_none());
        if fixture.calls() >= 7 && accepted_quota_receipts(&database).len() == 4 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "resident did not persist new quota: {}",
            fs::read_to_string(&log_path).unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(fs::read_to_string(&log_path)
        .unwrap()
        .contains("quota lane ready"));
    let degraded: i64 = account
        .query_row(
            "SELECT degraded FROM quota_acquisition_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        degraded, 1,
        "fresh success must not erase unresolved replay"
    );
    let current = accepted_quota_receipts(&database).last().unwrap().clone();
    raw.execute(
        "UPDATE quota_observations SET payload=?1 WHERE receipt_id=?2",
        [&original, &first_id],
    )
    .unwrap();
    loop {
        assert!(resident.0.try_wait().unwrap().is_none());
        let degraded: i64 = account
            .query_row(
                "SELECT degraded FROM quota_acquisition_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if accepted_quota_receipts(&database).len() == 5 && degraded == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "resident did not recover from raw: {}",
            fs::read_to_string(&log_path).unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fixture.calls(),
        7,
        "recovery must not need another API response"
    );
    let restored = accepted_quota_receipts(&database);
    assert_eq!(restored.first().unwrap().0, first_id);
    assert_eq!(
        restored.last().unwrap(),
        &current,
        "old input must not replace current"
    );
    let raw_count: i64 = raw
        .query_row("SELECT count(*) FROM quota_observations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw_count, 5);
    resident.0.kill().unwrap();
    resident.0.wait().unwrap();
}

#[test]
fn quota_journal_continues_while_session_startup_is_unavailable() {
    let mut fixture = QuotaJournalFixture::start(false, false, 3);
    fixture.wait_for_calls(4);
    fixture.child.0.kill().unwrap();
    fixture.child.0.wait().unwrap();
    let database = replay_partition(&fixture);
    let account = Connection::open(&database).unwrap();
    account
        .execute_batch(
            "ALTER TABLE usage_model_history RENAME COLUMN total_tokens TO unreadable_tokens;",
        )
        .unwrap();
    let root = fixture._root.path();
    private_directory(&root.join("home/sessions"));
    let log_path = root.join("startup.log");
    let mut resident = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_codex_info_recorder"))
            .args(["--interval-secs", "1"])
            .env("CODEX_HOME", root.join("home"))
            .env("CODEX_INFO_DATA_DIR", root.join("data"))
            .env("CODEX_INFO_CODEX_BIN", root.join("server.py"))
            .env("QUOTA_FIXTURE_CALLS", &fixture.calls)
            .env("QUOTA_FIXTURE_PAYLOAD", root.join("quota.json"))
            .env("QUOTA_FIXTURE_RESPONSES", "6")
            .env_remove("CODEX_INFO_RECORDER_DB")
            .env_remove("CODEX_INFO_ACCOUNT_DB")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&log_path).unwrap())
            .spawn()
            .unwrap(),
    );
    let raw = Connection::open(&fixture.database).unwrap();
    raw.busy_timeout(Duration::from_secs(1)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        assert!(
            resident.0.try_wait().unwrap().is_none(),
            "Session startup must not terminate quota recording: {}",
            fs::read_to_string(&log_path).unwrap()
        );
        let saved: i64 = raw
            .query_row(
                "SELECT count(*) FROM quota_observations WHERE account_verified=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if saved == 5 && fixture.calls() >= 7 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "quota acquisition stopped with Session startup: {}",
            fs::read_to_string(&log_path).unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(accepted_quota_receipts(&database).is_empty());
    account
        .execute_batch(
            "ALTER TABLE usage_model_history RENAME COLUMN unreadable_tokens TO total_tokens;",
        )
        .unwrap();
    loop {
        assert!(resident.0.try_wait().unwrap().is_none());
        if accepted_quota_receipts(&database).len() == 5 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Session recovery did not replay quota: {}",
            fs::read_to_string(&log_path).unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fixture.calls(), 7);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM quota_observations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        5
    );
    resident.0.kill().unwrap();
    resident.0.wait().unwrap();
}

#[test]
fn quota_replay_rolls_back_receipts_with_generation_and_retries_from_raw() {
    let mut fixture = QuotaJournalFixture::start(false, false, 3);
    fixture.wait_for_calls(4);
    fixture.child.0.kill().unwrap();
    fixture.child.0.wait().unwrap();
    let database = replay_partition(&fixture);
    let db = Connection::open(&database).unwrap();
    db.execute_batch("CREATE TRIGGER quota_fixture_reject BEFORE UPDATE ON collection_generation WHEN EXISTS(SELECT 1 FROM quota_receipts) BEGIN SELECT RAISE(ABORT, 'fixture quota commit rejected'); END;").unwrap();
    let failure = run_quota_replay(&fixture);
    assert!(failure.contains("degraded quota replay"), "{failure}");
    assert!(
        failure.contains("fixture quota commit rejected"),
        "{failure}"
    );
    assert!(accepted_quota_receipts(&database).is_empty());
    fixture.assert_saved(3);
    db.execute_batch("DROP TRIGGER quota_fixture_reject")
        .unwrap();
    run_quota_replay(&fixture);
    assert_eq!(accepted_quota_receipts(&database).len(), 3);
}

struct Environment(Vec<(&'static str, Option<OsString>)>);

impl Environment {
    fn set(values: &[(&'static str, &Path)]) -> Self {
        let previous = values
            .iter()
            .map(|(key, value)| {
                let previous = env::var_os(key);
                env::set_var(key, value);
                (*key, previous)
            })
            .collect();
        Self(previous)
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (key, previous) in self.0.drain(..) {
            match previous {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl ChildGuard {
    fn id(&self) -> u32 {
        self.0.id()
    }
}

fn fixture_process_diagnostics(pid: u32, sessions: &Path, session: &Path) -> String {
    let process = PathBuf::from(format!("/proc/{pid}"));
    let process_alive = process.exists();
    let comm_is_codex = fs::read_to_string(process.join("comm"))
        .map(|value| value.trim_end() == "codex")
        .unwrap_or(false);
    let executable_is_codex = fs::read_link(process.join("exe"))
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_owned()))
        .as_deref()
        == Some(std::ffi::OsStr::new("codex"));
    let session_fd_matches = fs::read_dir(process.join("fd"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| fs::read_link(entry.path()).ok().as_deref() == Some(session))
        .count();
    let session_is_regular = fs::symlink_metadata(session)
        .map(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
        .unwrap_or(false);
    let under_sessions_root = match (fs::canonicalize(sessions), fs::canonicalize(session)) {
        (Ok(root), Ok(target)) => target
            .strip_prefix(root)
            .map(|relative| !relative.as_os_str().is_empty())
            .unwrap_or(false),
        _ => false,
    };
    let jsonl_extension = session.extension().and_then(|value| value.to_str()) == Some("jsonl");
    let fixture_candidate_count = if process_alive
        && comm_is_codex
        && executable_is_codex
        && session_fd_matches > 0
        && session_is_regular
        && under_sessions_root
        && jsonl_extension
    {
        1
    } else {
        0
    };
    format!(
        "process_alive={process_alive} comm_codex={comm_is_codex} exe_codex={executable_is_codex} session_fd_matches={session_fd_matches} regular_file={session_is_regular} under_sessions_root={under_sessions_root} jsonl={jsonl_extension} fixture_candidate_count={fixture_candidate_count}"
    )
}

fn fake_app_server_launch_count(calls: &Path, client: &str) -> usize {
    let Ok(records) = fs::read_to_string(calls) else {
        return 0;
    };
    records
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["client"].as_str() == Some(client))
        .count()
}

fn poll_result_kind(result: &ActiveThreadPollResult) -> &'static str {
    match result {
        ActiveThreadPollResult::Empty { .. } => "empty",
        ActiveThreadPollResult::Failed(_) | ActiveThreadPollResult::CheckpointMismatch => "failed",
        ActiveThreadPollResult::Snapshot { .. } => "snapshot",
    }
}

fn private_directory(path: &Path) {
    fs::create_dir_all(path).expect("create private fixture directory");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("protect fixture directory");
}

fn fake_app_server(path: &Path) {
    // The process models only Codex's SQLite location choice and the JSON-RPC
    // responses needed to exercise the three real recorder launch paths.
    let source = r#"#!/usr/bin/env python3
import json
import os
import sqlite3
import sys
import time

args = sys.argv[1:]
overrides = [
    args[index + 1][len('sqlite_home='):]
    for index in range(len(args) - 1)
    if args[index] == '-c' and args[index + 1].startswith('sqlite_home=')
]
sqlite_home = overrides[0] if overrides else os.environ['CODEX_HOME']
fail_private_initialize = os.environ.get('CODEX_INFO_FAKE_FAIL_PRIVATE_INITIALIZE') == '1'
fail_private_account_read = os.environ.get('CODEX_INFO_FAKE_FAIL_PRIVATE_ACCOUNT_READ') == '1'
thread_initialize_delay = float(os.environ.get('CODEX_INFO_FAKE_THREAD_INITIALIZE_DELAY_SECS', '0'))
thread_read_delay = float(os.environ.get('CODEX_INFO_FAKE_THREAD_READ_DELAY_SECS', '0'))
thread_read_marker = os.environ.get('CODEX_INFO_FAKE_THREAD_READ_MARKER')
thread_items_path = os.environ.get('CODEX_INFO_FAKE_THREAD_ITEMS')
with sqlite3.connect(os.path.join(sqlite_home, 'logs_2.sqlite')) as connection:
    connection.execute('CREATE TABLE IF NOT EXISTS fixture (value INTEGER)')

account = {
    'requiresOpenaiAuth': False,
    'account': {'type': 'chatgpt', 'email': 'fixture@example.com', 'planType': 'pro'},
}
quota = {'rateLimits': {'primary': {
    'usedPercent': 56,
    'resetsAt': int(time.time()) + 604800,
    'windowDurationMins': 10080,
}}}
for line in sys.stdin:
    request = json.loads(line)
    request_id = request.get('id')
    if not isinstance(request_id, int):
        continue
    method = request.get('method')
    if method == 'initialize':
        name = request.get('params', {}).get('clientInfo', {}).get('name')
        with open(os.environ['CODEX_INFO_FAKE_CALLS'], 'a', encoding='utf-8') as calls:
            calls.write(json.dumps({'client': name, 'args': args}) + '\n')
        if name == 'codex-info-recorder-thread-poller' and thread_initialize_delay:
            time.sleep(thread_initialize_delay)
        if overrides and fail_private_initialize:
            print(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'error': {'code': -32000, 'message': 'fixture failure'}}), flush=True)
            continue
        result = {}
    elif method == 'account/read':
        if overrides and fail_private_account_read:
            print(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'error': {'code': -32000, 'message': 'fixture failure'}}), flush=True)
            continue
        result = account
    elif method == 'account/rateLimits/read':
        result = quota
    elif method == 'thread/read':
        if thread_read_marker:
            with open(thread_read_marker, 'a', encoding='utf-8') as marker:
                marker.write('thread/read\n')
                marker.flush()
        if thread_read_delay:
            time.sleep(thread_read_delay)
        if thread_items_path:
            with open(thread_items_path, encoding='utf-8') as fixture:
                items = json.load(fixture)
            result = {'thread': items[request['params']['threadId']]}
        else:
            result = {'thread': {}}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'result': result}), flush=True)
"#;
    fs::write(path, source).expect("write fake app-server");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("make fake app-server executable");
}

fn live_session_process(root: &Path, session: &Path) -> ChildGuard {
    let executable = root.join("codex");
    fs::copy(
        // Copy this same-UID test binary to create a benign fixture process, not a security identity.
        // nosemgrep: rust.lang.security.current-exe.current-exe
        env::current_exe().expect("resolve current test executable"),
        &executable,
    )
    .expect("copy test executable under the required process name");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
        .expect("make fixture helper executable");
    let mut child = Command::new(&executable)
        .arg("--ignored")
        .arg("--exact")
        .arg("issue182_hold_session_file_open")
        .arg("--nocapture")
        .env("CODEX_INFO_ISSUE182_SESSION_PATH", session)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start benign codex process");
    let pid = child.id();
    let sessions = session.parent().expect("Session has a parent directory");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let process = PathBuf::from(format!("/proc/{pid}"));
        let comm = fs::read_to_string(process.join("comm")).unwrap_or_default();
        let executable_name = fs::read_link(process.join("exe"))
            .ok()
            .and_then(|path| path.file_name().map(|name| name.to_owned()));
        let session_open = fs::read_dir(process.join("fd"))
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| fs::read_link(entry.path()).ok().as_deref() == Some(session));
        if comm.trim_end() == "codex"
            && executable_name.as_deref() == Some(std::ffi::OsStr::new("codex"))
            && session_open
        {
            return ChildGuard(child);
        } else if Instant::now() >= deadline {
            let diagnostics = fixture_process_diagnostics(pid, sessions, session);
            eprintln!("issue182 fixture setup: {diagnostics}");
            let _ = child.kill();
            let _ = child.wait();
            panic!("FIXTURE_NOT_ADMITTED: live Session FD unavailable; {diagnostics}");
        } else {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn live_session_set_process(root: &Path, sessions: &[PathBuf]) -> ChildGuard {
    let executable = root.join("codex");
    fs::copy(
        // Copy this same-UID test binary to create a benign fixture process, not a security identity.
        // nosemgrep: rust.lang.security.current-exe.current-exe
        env::current_exe().expect("resolve current test executable"),
        &executable,
    )
    .expect("copy test executable under the required process name");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
        .expect("make fixture helper executable");
    let child = Command::new(&executable)
        .arg("--ignored")
        .arg("--exact")
        .arg("issue419_hold_session_files_open")
        .env(
            "CODEX_INFO_ISSUE419_SESSIONS_DIR",
            sessions[0].parent().expect("Session directory"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start fixture Codex process");
    let live = ChildGuard(child);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let process = PathBuf::from(format!("/proc/{}", live.id()));
        let comm = fs::read_to_string(process.join("comm")).unwrap_or_default();
        let executable_name = fs::read_link(process.join("exe"))
            .ok()
            .and_then(|path| path.file_name().map(|name| name.to_owned()));
        let open_paths = fs::read_dir(process.join("fd"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read_link(entry.path()).ok())
            .collect::<BTreeSet<_>>();
        if comm.trim_end() == "codex"
            && executable_name.as_deref() == Some(std::ffi::OsStr::new("codex"))
            && sessions.iter().all(|session| open_paths.contains(session))
        {
            return live;
        }
        assert!(
            Instant::now() < deadline,
            "FIXTURE_NOT_ADMITTED: Session FDs unavailable"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "spawned as the synthetic Codex process by the Issue #182 fixture"]
fn issue182_hold_session_file_open() {
    let session = env::var_os("CODEX_INFO_ISSUE182_SESSION_PATH")
        .map(PathBuf::from)
        .expect("fixture helper receives the Session path");
    let _session = File::open(session).expect("fixture helper opens the Session file");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

#[test]
#[ignore = "spawned as the synthetic Codex process by the Issue #419 fixture"]
fn issue419_hold_session_files_open() {
    let sessions = PathBuf::from(
        env::var_os("CODEX_INFO_ISSUE419_SESSIONS_DIR")
            .expect("fixture helper receives the Session directory"),
    );
    let _open_files = fs::read_dir(sessions)
        .expect("read Session directory")
        .map(|entry| File::open(entry.expect("Session entry").path()).expect("open Session"))
        .collect::<Vec<_>>();
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn checkpoint(sessions: &Path, session: &Path, committed_offset: u64) -> SessionCheckpoint {
    let root = fs::metadata(sessions).expect("stat Session root");
    let file = fs::metadata(session).expect("stat Session file");
    SessionCheckpoint {
        root_identity: format!("unix:{}:{}", root.dev(), root.ino()),
        relative_path: "one.jsonl".to_owned(),
        file_device: file.dev(),
        file_inode: file.ino(),
        committed_offset,
        discard_until_lf: false,
        collector_epoch: 7,
        cycle_seq: 3,
        prefix_generation: 9,
        prefix_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            .to_owned(),
        fully_attributed_from_zero: true,
        token_baseline_known: true,
        history_base_pending: false,
        last_model: Some("gpt-5.6-sol".to_owned()),
        last_task_running: Some(true),
        context_usage_tokens: None,
        context_window_tokens: None,
        previous_total: 42,
        previous_input: 40,
        previous_cached_input: 0,
        previous_output: 2,
        previous_cache_write_input: Some(0),
    }
}

#[test]
fn stopped_parent_and_descendants_are_absent_after_real_thread_read_cycle() {
    let _environment_lock = ENVIRONMENT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temporary = tempfile::tempdir().expect("create isolated fixture root");
    let root = temporary.path();
    let codex_home = root.join("codex-home");
    let data_root = root.join("data");
    let sessions = codex_home.join("sessions");
    private_directory(&codex_home);
    private_directory(&data_root);
    private_directory(&sessions);
    let auth = codex_home.join("auth.json");
    fs::write(&auth, br#"{"tokens":{"account_id":"fixture-account"}}"#)
        .expect("write account fixture");
    fs::set_permissions(&auth, fs::Permissions::from_mode(0o600)).expect("protect account fixture");
    let app_server = root.join("fake-app-server.py");
    fake_app_server(&app_server);
    let calls = root.join("app-server-calls.jsonl");
    let marker = root.join("thread-read-calls.txt");
    let items_path = root.join("thread-items.json");
    let cases = [
        (
            "stopped-child",
            "未設定",
            Some(("stopped-parent", 1)),
            false,
        ),
        ("valid-child", "未設定", Some(("valid-parent", 1)), true),
        (
            "stopped-grandchild",
            "Grandchild",
            Some(("stopped-child", 2)),
            true,
        ),
        ("independent-root", "Independent C", None, true),
        ("stopped-parent", "Stopped A", None, false),
        ("valid-parent", "Running B", None, true),
    ];
    let mut items = serde_json::Map::new();
    let mut session_paths = Vec::new();
    let mut checkpoints = Vec::new();
    for (id, title, parent, running) in cases {
        let session = sessions.join(format!("{id}.jsonl"));
        let mut session_meta = serde_json::to_vec(&json!({
            "type": "session_meta",
            "payload": {"id": id},
        }))
        .expect("serialize Session metadata");
        session_meta.push(b'\n');
        fs::write(&session, &session_meta).expect("write Session fixture");
        let mut checkpoint = checkpoint(&sessions, &session, session_meta.len() as u64);
        checkpoint.relative_path = format!("{id}.jsonl");
        checkpoints.push(checkpoint);
        let source = match parent {
            Some((parent_id, depth)) => json!({
                "subAgent": {"thread_spawn": {"parent_thread_id": parent_id, "depth": depth}}
            }),
            None => json!("cli"),
        };
        let item = json!({
            "cliVersion": "0.147.0",
            "createdAt": 1,
            "cwd": root,
            "ephemeral": false,
            "id": id,
            "modelProvider": "openai",
            "preview": "preview",
            "sessionId": format!("session-{id}"),
            "source": source,
            "status": if running {
                json!({"type": "active", "activeFlags": []})
            } else {
                json!({"type": "idle"})
            },
            "turns": [],
            "updatedAt": 1,
            "name": title,
            "path": session,
        });
        assert!(
            codex_info::thread_contract::validate_thread_item(&item).is_ok(),
            "FIXTURE_NOT_ADMITTED: invalid thread/read item for {id}"
        );
        items.insert(id.to_owned(), item);
        session_paths.push(session);
    }
    fs::write(
        &items_path,
        serde_json::to_vec(&items).expect("serialize thread/read fixture"),
    )
    .expect("write thread/read fixture");
    let _environment = Environment::set(&[
        ("CODEX_HOME", &codex_home),
        ("CODEX_INFO_DATA_DIR", &data_root),
        ("CODEX_INFO_CODEX_BIN", &app_server),
        ("CODEX_INFO_FAKE_CALLS", &calls),
        ("CODEX_INFO_FAKE_THREAD_ITEMS", &items_path),
        ("CODEX_INFO_FAKE_THREAD_READ_MARKER", &marker),
    ]);
    let _live_codex = live_session_set_process(root, &session_paths);
    let threads = ThreadPoller::start(sessions);
    assert!(threads.submit(&checkpoints));
    let result = threads
        .wait_for(Duration::from_secs(10))
        .expect("one complete thread cycle");
    let snapshot = match result {
        ActiveThreadPollResult::Snapshot { snapshot, .. } => snapshot,
        ActiveThreadPollResult::Failed(error) => {
            panic!("FIXTURE_NOT_ADMITTED: thread/read cycle failed: {error}")
        }
        ActiveThreadPollResult::CheckpointMismatch => {
            panic!("FIXTURE_NOT_ADMITTED: active session checkpoint mismatch")
        }
        ActiveThreadPollResult::Empty { .. } => {
            panic!("FIXTURE_NOT_ADMITTED: thread/read cycle was empty")
        }
    };
    let by_id = snapshot
        .threads
        .iter()
        .map(|thread| (thread.id.as_str(), thread))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        by_id.keys().copied().collect::<Vec<_>>(),
        ["independent-root", "valid-child", "valid-parent"]
    );
    assert_eq!(by_id["valid-child"].activity_status, "running");
    assert_eq!(
        by_id["valid-child"].parent_thread_id.as_deref(),
        Some("valid-parent")
    );
    assert_eq!(by_id["valid-child"].title, "未設定");
    assert_eq!(
        by_id
            .values()
            .filter(|thread| thread.activity_status == "running")
            .count(),
        3
    );
    assert_eq!(
        fs::read_to_string(marker)
            .expect("thread/read was called")
            .lines()
            .count(),
        6
    );
    assert_eq!(
        fake_app_server_launch_count(&calls, "codex-info-recorder-thread-poller"),
        1
    );
}

#[test]
fn thread_cycle_uses_one_deadline_across_initialize_and_read() {
    let _environment_lock = ENVIRONMENT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temporary = tempfile::tempdir().expect("create isolated slow-child fixture root");
    let root = temporary.path();
    let codex_home = root.join("codex-home");
    let data_root = root.join("data");
    let sessions = codex_home.join("sessions");
    private_directory(&codex_home);
    private_directory(&data_root);
    private_directory(&sessions);

    let auth = codex_home.join("auth.json");
    fs::write(&auth, br#"{"tokens":{"account_id":"fixture-account"}}"#)
        .expect("write account fixture");
    fs::set_permissions(&auth, fs::Permissions::from_mode(0o600)).expect("protect account fixture");
    let state = codex_home.join("state_5.sqlite");
    {
        let connection = Connection::open(&state).expect("create source SQLite fixture");
        connection
            .execute_batch("CREATE TABLE fixture (value INTEGER); INSERT INTO fixture VALUES (1);")
            .expect("seed source SQLite fixture");
    }
    fs::set_permissions(&state, fs::Permissions::from_mode(0o600))
        .expect("protect source SQLite fixture");

    let app_server = root.join("fake-app-server.py");
    fake_app_server(&app_server);
    let calls = root.join("app-server-calls.jsonl");
    let marker = root.join("thread-read-calls.txt");
    let initialize_delay = Path::new("9");
    let thread_read_delay = Path::new("14");
    let session = sessions.join("one.jsonl");
    let session_meta = b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\"}}\n";
    fs::write(&session, session_meta).expect("write active Session fixture");
    let _environment = Environment::set(&[
        ("CODEX_HOME", &codex_home),
        ("CODEX_INFO_DATA_DIR", &data_root),
        ("CODEX_INFO_CODEX_BIN", &app_server),
        ("CODEX_INFO_FAKE_CALLS", &calls),
        (
            "CODEX_INFO_FAKE_THREAD_INITIALIZE_DELAY_SECS",
            initialize_delay,
        ),
        ("CODEX_INFO_FAKE_THREAD_READ_DELAY_SECS", thread_read_delay),
        ("CODEX_INFO_FAKE_THREAD_READ_MARKER", &marker),
    ]);

    let _live_codex = live_session_process(root, &session);
    let threads = ThreadPoller::start(sessions.clone());
    let thread_checkpoint = checkpoint(&sessions, &session, session_meta.len() as u64);
    assert!(threads.submit(&[thread_checkpoint]));

    let cycle_start = Instant::now();
    let result = threads
        .wait_for(Duration::from_secs(20))
        .expect("one shared 15-second deadline must complete before the 20-second oracle");
    let elapsed = cycle_start.elapsed();
    assert!(
        matches!(result, ActiveThreadPollResult::Failed(_)),
        "slow thread/read response must fail at the cycle deadline"
    );
    assert!(
        elapsed < Duration::from_secs(20),
        "cycle exceeded the 20-second oracle: {elapsed:?}"
    );

    let marker_records = fs::read_to_string(&marker).expect("thread/read reached fake child");
    assert_eq!(
        marker_records.lines().collect::<Vec<_>>(),
        ["thread/read"],
        "the cycle must reach thread/read exactly once without retry"
    );
    assert_eq!(
        fake_app_server_launch_count(&calls, "codex-info-recorder-thread-poller"),
        1,
        "the slow cycle must launch exactly one app-server child"
    );
}

#[test]
fn recorder_app_server_paths_match_v197_launch() {
    let _environment_lock = ENVIRONMENT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temporary = tempfile::tempdir().expect("create isolated fixture root");
    let root = temporary.path();
    let codex_home = root.join("codex-home");
    let data_root = root.join("data");
    let sessions = codex_home.join("sessions");
    private_directory(&codex_home);
    private_directory(&data_root);
    private_directory(&sessions);

    let auth = codex_home.join("auth.json");
    fs::write(&auth, br#"{"tokens":{"account_id":"fixture-account"}}"#)
        .expect("write account fixture");
    fs::set_permissions(&auth, fs::Permissions::from_mode(0o600)).expect("protect account fixture");
    let state = codex_home.join("state_5.sqlite");
    {
        let connection = Connection::open(&state).expect("create source SQLite fixture");
        connection
            .execute_batch("CREATE TABLE fixture (value INTEGER); INSERT INTO fixture VALUES (1);")
            .expect("seed source SQLite fixture");
    }
    fs::set_permissions(&state, fs::Permissions::from_mode(0o600))
        .expect("protect source SQLite fixture");
    let state_before = fs::read(&state).expect("read source before probes");

    let app_server = root.join("fake-app-server.py");
    fake_app_server(&app_server);
    let calls = root.join("app-server-calls.jsonl");
    let _environment = Environment::set(&[
        ("CODEX_HOME", &codex_home),
        ("CODEX_INFO_DATA_DIR", &data_root),
        ("CODEX_INFO_CODEX_BIN", &app_server),
        ("CODEX_INFO_FAKE_CALLS", &calls),
    ]);

    assert_eq!(
        probe_codex_authentication_state().expect("auth probe must reach app-server"),
        CodexAuthenticationState::Authenticated
    );

    let mut quota = QuotaPoller::start_with_interval(3600);
    let quota_deadline = Instant::now() + Duration::from_secs(5);
    while quota.latest().is_none() {
        assert!(
            Instant::now() < quota_deadline,
            "fixture setup: quota path not reached"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let session = sessions.join("one.jsonl");
    let session_meta = b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\"}}\n";
    fs::write(&session, session_meta).expect("write Session fixture");
    let live_codex = live_session_process(root, &session);
    let threads = ThreadPoller::start(sessions.clone());
    let thread_checkpoint = checkpoint(&sessions, &session, session_meta.len() as u64);
    eprintln!(
        "issue182 before thread probe: {}; thread_app_server_launches={}",
        fixture_process_diagnostics(live_codex.id(), &sessions, &session),
        fake_app_server_launch_count(&calls, "codex-info-recorder-thread-poller")
    );
    assert!(threads.submit(&[thread_checkpoint]));
    let result = match threads.wait_for(Duration::from_secs(5)) {
        Some(result) => result,
        None => {
            let launches =
                fake_app_server_launch_count(&calls, "codex-info-recorder-thread-poller");
            eprintln!(
                "issue182 after thread probe: result=none thread_app_server_launches={launches}; {}",
                fixture_process_diagnostics(live_codex.id(), &sessions, &session)
            );
            if launches != 1 {
                panic!("FIXTURE_NOT_ADMITTED: Threads probe returned no result; thread_app_server_launches={launches}");
            }
            panic!("INCONCLUSIVE: Threads probe returned no result after app-server launch");
        }
    };
    let thread_app_server_launches =
        fake_app_server_launch_count(&calls, "codex-info-recorder-thread-poller");
    eprintln!(
        "issue182 after thread probe: result={} thread_app_server_launches={thread_app_server_launches}; {}",
        poll_result_kind(&result),
        fixture_process_diagnostics(live_codex.id(), &sessions, &session)
    );
    assert_eq!(
        thread_app_server_launches, 1,
        "FIXTURE_NOT_ADMITTED: thread fake app-server launch count={thread_app_server_launches}"
    );

    let records = fs::read_to_string(&calls).expect("all three app-server calls must be logged");
    let observed: Vec<Value> = records
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse fake app-server record"))
        .collect();
    let names = [
        "codex-info-recorder-auth-probe",
        "codex-info-recorder",
        "codex-info-recorder-thread-poller",
    ];
    for name in names {
        let count = observed
            .iter()
            .filter(|record| record["client"].as_str() == Some(name))
            .count();
        assert_eq!(count, 1, "fixture setup: {name} launch count must be one");
    }
    assert_eq!(
        observed.len(),
        3,
        "fixture setup: unexpected app-server launch"
    );

    for record in &observed {
        let args = record["args"]
            .as_array()
            .expect("recorded argv is an array");
        let arguments: Vec<_> = args.iter().map(|arg| arg.as_str().unwrap()).collect();
        assert_eq!(
            arguments,
            ["app-server", "--stdio"],
            "v1.0.97 launch argv for {}",
            record["client"]
        );
    }
    assert!(
        codex_home.join("logs_2.sqlite").exists(),
        "v1.0.97 app-server uses the shared Codex SQLite home"
    );
    assert_eq!(
        fs::read(state).expect("read source after probes"),
        state_before
    );
}

#![cfg(unix)]

use codex_info_db_writer::SessionCheckpoint;
use codex_info_recorder::{
    probe_codex_authentication_state, ActiveThreadPollResult, CodexAuthenticationState,
    QuotaPoller, ThreadPoller,
};
use rusqlite::{Connection, OptionalExtension};
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
        Self::start_process_with_account_change(
            malformed,
            storage_failure,
            responses,
            resident_interval,
            false,
        )
    }

    fn start_process_with_account_change(
        malformed: bool,
        storage_failure: bool,
        responses: usize,
        resident_interval: Option<u64>,
        account_change: bool,
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
        let reserved = codex_info_account_locator::ensure_partition(&home, &data).unwrap();
        if resident_interval.is_none() {
            // The library-only poller runs inside a previously confirmed
            // account. The resident branch exercises real startup instead.
            let at = chrono::Utc::now().timestamp();
            codex_info_account_locator::admit_partition(
                &home,
                &data,
                &reserved,
                codex_info_account_locator::AccountConfirmation::InitialRpc { began_at: at },
                || Some(at),
            )
            .unwrap();
        }
        let journal = data.join("history/quota");
        private_directory(&journal);
        let database = journal.join("observations.sqlite3");
        if storage_failure {
            fs::create_dir(&database).unwrap();
        }
        let expected = json!({"rateLimits":{"primary":{
            "usedPercent": if malformed { json!("future-format") } else { json!(1) },
            "resetsAt":chrono::Utc::now().timestamp() + 604800,"windowDurationMins":10080
        }},"futureField":{"retained":true}});
        let payload = root.path().join("quota.json");
        fs::write(&payload, expected.to_string()).unwrap();
        let calls = root.path().join("calls");
        let server = root.path().join("server.py");
        fs::write(&server, r#"#!/usr/bin/env python3
import json, os, sys, time
quota_read = False
for line in sys.stdin:
    req = json.loads(line)
    if 'id' not in req:
        continue
    result = {}
    if req['method'] == 'account/read':
        result = {'requiresOpenaiAuth':False,'account':{'type':'chatgpt','email':'fixture@example.invalid','planType':'pro'}}
        if quota_read and os.environ.get('QUOTA_FIXTURE_ACCOUNT_CHANGE') == '1':
            result['account']['email'] = 'other@example.invalid'
    if req['method'] == 'account/rateLimits/read':
        quota_read = True
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
                // Run a same-UID test helper; this path is not used as a security identity.
                // nosemgrep: rust.lang.security.current-exe.current-exe
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
                .env(
                    "QUOTA_FIXTURE_ACCOUNT_CHANGE",
                    if account_change { "1" } else { "0" },
                )
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
fn quota_account_change_preserves_unverified_input_without_applying_it() {
    let mut fixture =
        QuotaJournalFixture::start_process_with_account_change(false, false, 1, None, true);
    // The second fetch starts only after the first identity recheck finished.
    fixture.wait_for_calls(2);
    fixture.assert_saved(1);
    let journal = Connection::open_with_flags(
        &fixture.database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let verified: i64 = journal
        .query_row(
            "SELECT account_verified FROM quota_observations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        verified, 0,
        "a changed identity must not certify the response"
    );
    drop(journal);
    fixture.child.0.kill().unwrap();
    fixture.child.0.wait().unwrap();
    let database = replay_partition(&fixture);
    let _replay_log = run_quota_replay(&fixture);
    assert!(
        accepted_quota_receipts(&database).is_empty(),
        "restart must not assign the unverified response to an account"
    );
    fixture.assert_saved(1);
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

#[test]
fn real_startup_recovers_inactive_account_source_history() {
    assert_inactive_original_repair(false, false, false, false, None);
}

#[test]
fn issue604_historical_repairs_source_disproved_numbers_and_model() {
    assert_inactive_original_repair(true, false, false, false, None);
}

#[test]
fn issue604_current_repairs_source_disproved_numbers_at_the_original_cursor() {
    assert_inactive_original_repair(true, true, false, false, None);
}

#[test]
fn issue604_historical_repairs_initial_baseline_regression_and_double_add() {
    assert_inactive_original_repair(true, false, true, false, None);
}

#[test]
fn issue604_historical_reads_old_bytes_through_an_independently_accepted_container() {
    assert_inactive_original_repair(false, false, false, true, None);
}

#[test]
fn issue604_current_reads_old_bytes_through_an_independently_accepted_container() {
    assert_inactive_original_repair(false, true, false, true, None);
}

#[test]
fn issue604_source_equivalence_identical_prefix_alias_reaches_startup_and_recorded_index_once() {
    assert_inactive_original_repair(false, false, false, false, Some("identical-prefix-alias"));
}

#[test]
fn issue604_source_equivalence_event_bijection_reaches_startup_and_recorded_index_once() {
    assert_inactive_original_repair(false, false, false, false, Some("event-bijection"));
}

fn assert_inactive_original_repair(
    disproved_numbers: bool,
    selected_account: bool,
    stream_boundaries: bool,
    accepted_container: bool,
    equivalence: Option<&str>,
) {
    use codex_info_account_locator::{
        admit_partition, mark_partition_initialized, reserve_partition_at, AccountConfirmation,
    };
    use std::io::Write;
    fn confirmed_fixture(
        home: &Path,
        data: &Path,
        at: i64,
    ) -> codex_info_account_locator::AccountPartition {
        let partition = reserve_partition_at(home, data, at).unwrap();
        admit_partition(
            home,
            data,
            &partition,
            AccountConfirmation::InitialRpc { began_at: at },
            || Some(at),
        )
        .unwrap()
    }
    use codex_info_db_writer::{QuotaReceipt, StoragePartitionIdentity, UsageStore};
    use codex_info_recorder::{QuotaSnapshot, Recorder, RecorderConfig};
    use rusqlite::{params, OptionalExtension};
    let _environment = ENVIRONMENT_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let data = root.path().join("data");
    let sessions = root.path().join("custom-sessions");
    for path in [&home, &data, &sessions] {
        private_directory(path);
    }
    let auth = home.join("auth.json");
    fs::write(&auth, br#"{"tokens":{"account_id":"old-account"}}"#).unwrap();
    fs::set_permissions(&auth, fs::Permissions::from_mode(0o600)).unwrap();
    let start = chrono::Utc::now().timestamp().div_euclid(60) * 60 - 600;
    let reset = start + 3600;
    let old = confirmed_fixture(&home, &data, start);
    let identity = StoragePartitionIdentity {
        schema_version: "codex-info-account-db-v1".into(),
        profile_scope_id: old.profile_scope_id.clone(),
        account_scope_id: old.account_scope_id.clone(),
        storage_epoch: old.storage_epoch,
        partition_id: old.partition_id.clone(),
    };
    let mut body = format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":"old-session","session_id":"old-session"}}),
        json!({"type":"turn_context","timestamp":chrono::DateTime::from_timestamp(start,0).unwrap().to_rfc3339(),"payload":{"model":"gpt-6-astra"}})
    );
    let points = if equivalence.is_some() {
        vec![
            (0, start + 1, false),
            (10, start + 10, false),
            (15, start + 70, false),
            (15, start + 130, true),
        ]
    } else if stream_boundaries {
        vec![
            (10, start + 1, false),
            (20, start + 10, false),
            (5, start + 40, false),
            (25, start + 70, false),
            (25, start + 130, true),
        ]
    } else {
        vec![
            (0, start + 1, false),
            (10, start + 10, false),
            (20, start + 70, false),
            (20, start + 130, true),
        ]
    };
    for (total, at, with_quota) in points {
        let mut record = json!({"type":"event_msg","timestamp":chrono::DateTime::from_timestamp(at,0).unwrap().to_rfc3339(),
            "payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":total,"input_tokens":total,"cached_input_tokens":0,"output_tokens":0,"cache_write_input_tokens":0}}}});
        if equivalence.is_some() {
            record["payload"]["info"]["total_token_usage"]
                .as_object_mut()
                .unwrap()
                .remove("cache_write_input_tokens");
        }
        if with_quota {
            record["payload"]["rate_limits"] =
                json!({"secondary":{"used_percent":12,"resets_at":reset,"window_minutes":60}});
        }
        body.push_str(&format!("{record}\n"));
    }
    fs::write(sessions.join("old.jsonl"), body).unwrap();
    let mut recorder = Recorder::open_partitioned(
        RecorderConfig {
            sessions_root: sessions.clone(),
            chunk_bytes: 65536,
        },
        &old.database_path,
        &identity,
    )
    .unwrap();
    recorder
        .run_cycle_with_quota(Some(QuotaSnapshot {
            observed_at: start + 30,
            reset_at: reset,
            window_seconds: 3600,
            remaining_percent: Some(90.0),
        }))
        .unwrap()
        .unwrap();
    drop(recorder);
    let mut writer = UsageStore::open_partitioned(&old.database_path, &identity).unwrap();
    let receipts = [
        (1_i64, start + 1, 90.0),
        (2, start + 30, 90.0),
        (3, start + 130, 88.0),
    ]
    .map(|(sequence, observed_at, remaining_percent)| QuotaReceipt {
        receipt_id: format!("{sequence:064x}"),
        sequence,
        observed_at,
        reset_at: reset,
        window_seconds: 3600,
        remaining_percent,
    });
    writer.commit_quota_receipts(&receipts, false).unwrap();
    writer
        .stage_official_quota_inputs(
            &receipts
                .iter()
                .map(
                    |receipt| codex_info_db_writer::period_evidence::OfficialQuotaInput {
                        input_id: receipt.receipt_id.clone(),
                        account_scope: identity.account_scope_id.clone(),
                        sequence: u64::try_from(receipt.sequence).unwrap(),
                        observed_at: receipt.observed_at,
                        content_sha256: "f".repeat(64),
                        reset_at: Some(reset),
                        window_seconds: Some(3600),
                        reason: None,
                    },
                )
                .collect::<Vec<_>>(),
        )
        .unwrap();
    let expected_events = writer.load_session_events().unwrap();
    let original_state = writer.load_session_collection_state().unwrap();
    let mut checkpoints = original_state.checkpoints.clone();
    for checkpoint in &mut checkpoints {
        checkpoint.source_admission = None;
        checkpoint.last_model_context_kind = None;
        checkpoint.last_model_interval = None;
    }
    let expected_checkpoints = checkpoints.clone();
    drop(writer);
    mark_partition_initialized(&data, &old).unwrap();
    let db = Connection::open(&old.database_path).unwrap();
    db.execute("UPDATE session_checkpoints SET source_admission=NULL,last_model_context_kind=NULL,last_model_interval=NULL", []).unwrap();
    if disproved_numbers {
        db.execute(
            "UPDATE session_events SET total_tokens='42',input_tokens='42',model='gpt-6-luna'",
            [],
        )
        .unwrap();
        db.execute(
            "UPDATE session_checkpoints SET previous_total='999',previous_input='999'",
            [],
        )
        .unwrap();
        db.execute("UPDATE session_model_totals SET model='gpt-6-luna',total_tokens='84',input_tokens='84'",[]).unwrap();
        for checkpoint in &mut checkpoints {
            checkpoint.previous_total = Some(999);
            checkpoint.previous_input = Some(999);
        }
    }
    if stream_boundaries {
        for (index, at, total) in [
            (100, start + 1, 10),
            (101, start + 40, 5),
            (102, start + 130, 25),
        ] {
            db.execute("INSERT INTO session_events SELECT root_identity,relative_path,file_device,file_inode,prefix_generation,range_start,range_end,record_sha256,?1,?2,model,?3,?3,cached_input_tokens,output_tokens,cache_write_input_tokens,NULL,NULL,NULL FROM session_events LIMIT 1",params![index,at,total.to_string()]).unwrap();
        }
    }
    if let Some(mode) = equivalence {
        db.execute("UPDATE usage_model_history SET total_dollars=?1,price_version='preserved-fixture-price'", [0.123_456_789_012_345_66_f64]).unwrap();
        use sha2::{Digest, Sha256};
        // These are independent saved prefixes, not a boolean qualification.
        // The production startup must reread every hash and rebuild proof.
        let generation = format!("{:032x}", checkpoints[0].prefix_generation.wrapping_add(1));
        let new_identity = if mode == "event-bijection" {
            let path = sessions.join("old.jsonl");
            let original = fs::read(&path).unwrap();
            let replacement = sessions.join("accepted.tmp");
            let mut bytes = b"{\"type\":\"ignored_fixture_record\"}\n".to_vec();
            bytes.extend(original);
            fs::write(&replacement, &bytes).unwrap();
            let metadata = fs::metadata(&replacement).unwrap();
            fs::rename(&replacement, &path).unwrap();
            Some((
                metadata.dev(),
                metadata.ino(),
                bytes.len(),
                format!("{:x}", Sha256::digest(Sha256::digest(&bytes))),
            ))
        } else {
            None
        };
        for table in ["session_checkpoints", "session_ranges", "session_events"] {
            if new_identity.is_some() && table == "session_events" {
                continue;
            }
            let query = db
                .prepare(&format!("SELECT * FROM {table} LIMIT 0"))
                .unwrap();
            let columns = query
                .column_names()
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>();
            drop(query);
            let select = columns
                .iter()
                .map(|column| {
                    if column == "prefix_generation" {
                        return format!("'{generation}'");
                    }
                    if let Some((device, inode, end, hash)) = &new_identity {
                        match column.as_str() {
                            "file_device" => return format!("'{device}'"),
                            "file_inode" => return format!("'{inode}'"),
                            "committed_offset" | "end_offset" => return end.to_string(),
                            "record_sha256" | "prefix_sha256" => return format!("'{hash}'"),
                            _ => {}
                        }
                    }
                    column.clone()
                })
                .collect::<Vec<_>>()
                .join(",");
            db.execute(
                &format!("INSERT INTO {table} SELECT {select} FROM {table}"),
                [],
            )
            .unwrap();
        }
    }
    let container_inode = if accepted_container {
        let path = sessions.join("old.jsonl");
        let replacement = sessions.join("accepted.tmp");
        fs::write(&replacement, fs::read(&path).unwrap()).unwrap();
        let metadata = fs::metadata(&replacement).unwrap();
        assert_ne!(metadata.ino(), checkpoints[0].file_inode);
        fs::rename(replacement, path).unwrap();
        // This is a distinct previously accepted physical source. Both sets
        // of stored range hashes must be checked against the same full bytes.
        db.execute("INSERT INTO session_ranges SELECT root_identity,relative_path,?1,?2,start_offset,end_offset,collector_epoch,cycle_seq,prefix_generation,record_sha256 FROM session_ranges",
            params![metadata.dev().to_string(),metadata.ino().to_string()]).unwrap();
        Some(metadata.ino())
    } else {
        None
    };
    let raw_writer = UsageStore::open_read_only_partitioned(&old.database_path, &identity).unwrap();
    let events = raw_writer.load_session_events().unwrap();
    drop(raw_writer);
    if equivalence.is_none() {
        db.execute(
            "DELETE FROM usage_model_history WHERE timestamp=?1",
            [start],
        )
        .unwrap();
        db.execute(
            "DELETE FROM usage_model_history WHERE timestamp=?1",
            [start + 120],
        )
        .unwrap();
        db.execute(
            "DELETE FROM durable_state WHERE singleton>=2 AND data_generation=?1",
            [start + 120],
        )
        .unwrap();
        db.execute(
            "DELETE FROM usage_history WHERE timestamp=?1",
            [start + 120],
        )
        .unwrap();
        db.execute("DELETE FROM session_quota_windows", []).unwrap();
    }
    drop(db);
    // This belongs to the old account's time interval, but was never part of
    // its accepted prefix. Historical repair must stop at the saved N.
    if !selected_account && equivalence.is_none() {
        fs::OpenOptions::new().append(true).open(sessions.join("old.jsonl")).unwrap()
        .write_all(format!("{}\n",json!({"type":"event_msg","timestamp":chrono::DateTime::from_timestamp(start+140,0).unwrap().to_rfc3339(),
            "payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":999,"input_tokens":999,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":0}}}})).as_bytes()).unwrap();
    }
    let current = if selected_account {
        old.clone()
    } else {
        fs::write(&auth, br#"{"tokens":{"account_id":"current-account"}}"#).unwrap();
        confirmed_fixture(&home, &data, start + 180)
    };
    let run = |expected_rejection: Option<&str>| {
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_codex_info_recorder"))
                .args(["--once", "--sessions-root"])
                .arg(&sessions)
                .env("CODEX_HOME", &home)
                .env("CODEX_INFO_DATA_DIR", &data)
                .env(
                    "CODEX_INFO_CODEX_BIN",
                    root.path().join("absent-app-server"),
                )
                .env_remove("CODEX_INFO_RECORDER_DB")
                .env_remove("CODEX_INFO_ACCOUNT_DB")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(File::create(root.path().join("startup.log")).unwrap())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(8);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "real recorder startup did not terminate"
            );
            thread::sleep(Duration::from_millis(20));
        };
        let log = fs::read_to_string(root.path().join("startup.log")).unwrap();
        if let Some(rejection) = expected_rejection {
            assert!(
                log.contains(rejection),
                "expected injected SQL rejection: {log}"
            );
        }
        if selected_account && expected_rejection.is_some() {
            assert!(
                !status.success(),
                "an uncommitted one-shot repair must fail: {log}"
            );
            assert!(
                log.contains("one-shot historical repair remains uncommitted"),
                "{log}"
            );
        } else {
            assert!(status.success(), "{log}");
        }
    };
    if let Some(mode) = equivalence {
        let snapshot = || {
            let db = Connection::open(&old.database_path).unwrap();
            let mut q = db
                .prepare("SELECT * FROM session_checkpoints ORDER BY prefix_generation,file_inode")
                .unwrap();
            let count = q.column_count();
            let checkpoints = q
                .query_map([], |r| {
                    (0..count)
                        .map(|i| r.get::<_, rusqlite::types::Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let price: Vec<_> = db.prepare("SELECT reset_at,timestamp,model,total_dollars,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens FROM usage_model_history ORDER BY reset_at,timestamp,model").unwrap().query_map([], |r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<f64>>(3)?.map(f64::to_bits),(4..9).map(|column|r.get::<_,Option<String>>(column)).collect::<rusqlite::Result<Vec<_>>>()?))).unwrap().map(Result::unwrap).collect();
            let fixed:Vec<_>=db.prepare("SELECT timestamp,reset_at,sol_tokens,terra_tokens,luna_tokens,sol_dollars,terra_dollars,luna_dollars FROM usage_history ORDER BY reset_at,timestamp").unwrap().query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,Option<i64>>(4)?,r.get::<_,Option<f64>>(5)?.map(f64::to_bits),r.get::<_,Option<f64>>(6)?.map(f64::to_bits),r.get::<_,Option<f64>>(7)?.map(f64::to_bits)))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            let durable: Vec<(String,String)> = db.prepare("SELECT data_hash,snapshot_json FROM durable_state WHERE singleton>=2 AND json_extract(snapshot_json,'$.kind')='codex-info-usage-observation-v1' ORDER BY data_hash").unwrap().query_map([],|row|Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            (checkpoints, price, fixed, durable)
        };
        let original = snapshot();
        let original_raw = codex_info_db_writer::read_session_event_facts(
            &Connection::open(&old.database_path).unwrap(),
        )
        .unwrap();
        assert_eq!(original.0.len(), 2);
        assert!(original
            .1
            .iter()
            .all(|p| p.3 == Some(0.123_456_789_012_345_66_f64.to_bits())));
        // A missing whole-prefix hash and None-versus-zero are distinct
        // refusal cases. Neither may be repaired by retaining only a subset.
        let rejected = Connection::open(&old.database_path).unwrap();
        let ranges = UsageStore::open_read_only_partitioned(&old.database_path, &identity)
            .unwrap()
            .load_session_ranges()
            .unwrap();
        if mode == "identical-prefix-alias" {
            rejected.execute("UPDATE session_ranges SET record_sha256=?1 WHERE prefix_generation=(SELECT max(prefix_generation) FROM session_ranges)",["ab".repeat(32)]).unwrap();
        } else {
            rejected.execute("UPDATE session_events SET cache_write_input_tokens='0' WHERE event_index=(SELECT min(event_index) FROM session_events)",[]).unwrap();
        }
        drop(rejected);
        run(None);
        let unchanged = snapshot();
        assert_eq!(unchanged.0, original.0);
        for saved in &original.1 {
            assert!(
                unchanged.1.contains(saved),
                "incomplete prefix/None-versus-zero proof must preserve the last saved values"
            );
        }
        for saved in &original.2 {
            assert!(
                unchanged.2.contains(saved),
                "incomplete prefix/None-versus-zero proof must preserve the last saved values"
            );
        }
        let rejected = Connection::open(&old.database_path).unwrap();
        assert_eq!(
            rejected
                .query_row("SELECT count(*) FROM source_prefix_repairs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        for range in &ranges {
            rejected.execute("UPDATE session_ranges SET record_sha256=?1 WHERE prefix_generation=?2 AND file_inode=?3 AND start_offset=?4",params![range.record_sha256,format!("{:032x}",range.prefix_generation),range.file_inode.to_string(),range.start_offset]).unwrap();
        }
        if mode == "event-bijection" {
            rejected
                .execute(
                    "UPDATE session_events SET cache_write_input_tokens=NULL",
                    [],
                )
                .unwrap();
            rejected.execute("INSERT INTO session_events SELECT root_identity,relative_path,file_device,file_inode,prefix_generation,range_start,range_end,record_sha256,999,timestamp+500,model,'77','77',cached_input_tokens,output_tokens,cache_write_input_tokens,NULL,NULL,NULL FROM session_events LIMIT 1",[]).unwrap();
            drop(rejected);
            run(None);
            let rejected = Connection::open(&old.database_path).unwrap();
            assert_eq!(rejected.query_row("SELECT count(*) FROM source_prefix_repairs",[],|r|r.get::<_,i64>(0)).unwrap(),0,"bijection must include the entire old event inventory, including unmatched suffixes");
            rejected
                .execute("DELETE FROM session_events WHERE event_index=999", [])
                .unwrap();
        } else {
            drop(rejected);
        }
        let rejected = Connection::open(&old.database_path).unwrap();
        rejected.execute_batch("CREATE TRIGGER reject_equivalence BEFORE INSERT ON source_prefix_repairs BEGIN SELECT RAISE(ABORT,'equivalence rollback fixture'); END;").unwrap();
        drop(rejected);
        run(None);
        let unchanged = snapshot();
        assert_eq!(unchanged.0, original.0);
        for saved in &original.1 {
            assert!(
                unchanged.1.contains(saved),
                "failed sealed transaction must retain checkpoint and price bits"
            );
        }
        for saved in &original.2 {
            assert!(
                unchanged.2.contains(saved),
                "failed sealed transaction must retain checkpoint and price bits"
            );
        }
        let rejected = Connection::open(&old.database_path).unwrap();
        assert_eq!(
            rejected
                .query_row("SELECT count(*) FROM source_prefix_repairs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        rejected
            .execute_batch("DROP TRIGGER reject_equivalence")
            .unwrap();
        // Keep the same legal legacy producer shape, including the original
        // quota, USD, Luna values and optional source timestamp at each key.
        for timestamp in [start, start + 60] {
            let mut sample = rejected.query_row(
                "SELECT timestamp,reset_at,remaining_percent,sol_dollars,terra_dollars,luna_dollars,sol_tokens,terra_tokens,luna_tokens FROM usage_history WHERE reset_at=?1 AND timestamp=?2",
                params![reset,timestamp],
                |row| Ok(codex_info_db_writer::UsageHistorySample {
                    timestamp: row.get(0)?, reset_at: row.get(1)?, remaining_percent: row.get(2)?,
                    sol_dollars: row.get(3)?, terra_dollars: row.get(4)?, luna_dollars: row.get(5)?,
                    sol_tokens: row.get(6)?, terra_tokens: row.get(7)?, luna_tokens: row.get(8)?,
                }),
            ).unwrap();
            sample.sol_tokens = 66_123_101;
            sample.terra_tokens = 3_763_895;
            let observation =
                codex_info_db_writer::UsageHistoryObservation::legacy_unknown(&sample);
            let (singleton, previous): (i64,String) = rejected.query_row(
                "SELECT singleton,snapshot_json FROM durable_state WHERE singleton>=2 AND json_extract(snapshot_json,'$.kind')='codex-info-usage-observation-v1' AND json_extract(snapshot_json,'$.reset_at')=?1 AND json_extract(snapshot_json,'$.timestamp')=?2",
                params![reset,timestamp], |row| Ok((row.get(0)?,row.get(1)?)),
            ).unwrap();
            let previous: Value = serde_json::from_str(&previous).unwrap();
            let mut saved = json!({
                "kind":"codex-info-usage-observation-v1",
                "timestamp":observation.timestamp,"reset_at":observation.reset_at,
                "remaining_percent":observation.remaining_percent,
                "sol_dollars":observation.sol_dollars,"terra_dollars":observation.terra_dollars,"luna_dollars":observation.luna_dollars,
                "sol_tokens":observation.sol_tokens,"terra_tokens":observation.terra_tokens,"luna_tokens":observation.luna_tokens,
                "model_source":observation.model_source.as_str(),
            });
            if let Some(source_timestamp) = previous.get("source_timestamp") {
                saved["source_timestamp"] = source_timestamp.clone();
            }
            assert_eq!(rejected.execute(
                "UPDATE usage_history SET sol_tokens=?1,terra_tokens=?2 WHERE reset_at=?3 AND timestamp=?4",
                params![sample.sol_tokens,sample.terra_tokens,reset,timestamp],
            ).unwrap(),1);
            assert_eq!(rejected.execute(
                "UPDATE usage_model_history SET total_tokens='30',input_tokens='30',cached_input_tokens='0',output_tokens='0',cache_write_input_tokens=NULL,model_set_complete=0 WHERE reset_at=?1 AND timestamp=?2 AND model='gpt-6-astra'",
                params![reset,timestamp],
            ).unwrap(),1);
            assert_eq!(rejected.execute(
                "UPDATE durable_state SET snapshot_json=?1 WHERE singleton=?2 AND json_extract(snapshot_json,'$.reset_at')=?3 AND json_extract(snapshot_json,'$.timestamp')=?4",
                params![saved.to_string(),singleton,reset,timestamp],
            ).unwrap(),1);
        }
        let original = snapshot();
        drop(
            UsageStore::open_read_only_partitioned(&old.database_path, &identity)
                .expect("coherent saved legacy fixture retains the strict observation schema"),
        );

        drop(rejected);
        run(None);
        let db = Connection::open(&old.database_path).unwrap();
        let values = db.prepare("SELECT tokens_json FROM recorded_usage_facts WHERE eligible=1 ORDER BY timestamp,fact_id").unwrap().query_map([], |r|r.get::<_,String>(0)).unwrap().map(|r|serde_json::from_str::<Value>(&r.unwrap()).unwrap()).collect::<Vec<_>>();
        let total = values
            .iter()
            .map(|v| v["total"].as_u64().unwrap())
            .sum::<u64>();
        assert_eq!(
            total,
            15,
            "{mode}: qualified T10+T5 must reach the recorded index exactly once: {}",
            fs::read_to_string(root.path().join("startup.log")).unwrap()
        );
        assert_eq!(values.len(), 2);
        assert!(
            values.iter().all(|v| v["cache_write_input"].is_null()),
            "unknown W must remain unknown"
        );
        let reader_identity = codex_info_db_reader::StoragePartitionIdentity {
            schema_version: identity.schema_version.clone(),
            profile_scope_id: identity.profile_scope_id.clone(),
            account_scope_id: identity.account_scope_id.clone(),
            storage_epoch: identity.storage_epoch,
            partition_id: identity.partition_id.clone(),
        };
        let intervals = codex_info_db_reader::ReadIntervals::new(vec![
            codex_info_db_reader::ReadInterval::new(Some(start), Some(start + 180)).unwrap(),
        ])
        .unwrap();
        let public = codex_info_db_reader::DbReader::open_partitioned_with_intervals(
            &old.database_path,
            &reader_identity,
            intervals,
        )
        .unwrap()
        .read_snapshot()
        .unwrap();
        let archive = public
            .recordings
            .as_deref()
            .expect("production reader admits repaired recorded usage");
        let saved = archive
            .observations
            .iter()
            .filter(|observation| {
                observation.storage_kind == "source-event"
                    && [start + 10, start + 70].contains(&observation.timestamp)
                    && observation
                        .models
                        .iter()
                        .any(|model| model.stored_model_key == "gpt-6-astra")
            })
            .collect::<Vec<_>>();
        assert!(
            !saved.is_empty(),
            "repaired raw source observations remain visible"
        );
        for observation in saved {
            assert!(!observation.observation_id.is_empty());
            assert!(
                !observation
                    .unresolved_reasons
                    .iter()
                    .any(|reason| reason == "source-recovery-pending"),
                "{mode}: the independently proved raw observation is recovered: {:?}",
                observation.unresolved_reasons
            );
            let model = observation
                .models
                .iter()
                .find(|model| model.stored_model_key == "gpt-6-astra")
                .unwrap();
            assert_eq!(
                model.total_tokens,
                Some(if observation.timestamp == start + 10 {
                    10
                } else {
                    5
                })
            );
            assert_eq!(
                model.cache_write_input_tokens, None,
                "saved unknown W remains unknown after classification repair"
            );
        }
        let raw_count: i64 = db
            .query_row("SELECT count(*) FROM session_events", [], |r| r.get(0))
            .unwrap();
        assert!(raw_count >= 2, "old raw keys must remain present");
        let payload:Option<String>=db.query_row("SELECT payload_json FROM source_prefix_repairs WHERE json_extract(payload_json,'$.mode')=?1",[mode],|r|r.get(0)).optional().unwrap();
        assert!(payload.is_some(), "{mode}: startup must commit the full-prefix proof and old-to-canonical pointer relation");
        let payload: Value = serde_json::from_str(&payload.unwrap()).unwrap();
        assert_eq!(payload["version"], 2);
        assert_eq!(
            payload["mappings"].as_array().unwrap().len(),
            if mode == "identical-prefix-alias" {
                4
            } else {
                2
            }
        );
        let before: (String,String)=db.query_row("SELECT fact_revision,(SELECT group_concat(prefix_json,'|') FROM recorded_usage_prefixes) FROM recorded_usage_state",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        drop(db);
        let restored = snapshot();
        assert_eq!(
            restored.0, original.0,
            "repair must retain both complete checkpoints"
        );
        for saved in &original.1 {
            assert!(
                restored.1.contains(saved),
                "original key, saved numeric and USD REAL bits must remain exact: {saved:?}"
            );
        }
        for saved in &original.2 {
            assert!(
                restored.2.contains(saved),
                "past fixed legacy observation must remain exact: {saved:?}"
            );
        }
        for saved in &original.3 {
            assert!(
                restored.3.contains(saved),
                "past durable observation must remain exact: {saved:?}"
            );
        }
        let raw = codex_info_db_writer::read_session_event_facts(
            &Connection::open(&old.database_path).unwrap(),
        )
        .unwrap();
        for saved in &original_raw {
            let restored = raw
                .iter()
                .find(|fact| fact.source == saved.source)
                .expect("every original nine-field key remains stored");
            assert_eq!(
                (&restored.tokens, &restored.model, restored.timestamp),
                (&saved.tokens, &saved.model, saved.timestamp)
            );
            if mode == "event-bijection" {
                assert_eq!(
                    restored.admission, saved.admission,
                    "new-source admission cannot be written onto the old physical key"
                );
            }
        }
        assert!(
            payload
                .to_string()
                .contains(&format!("{:016x}", 0.123_456_789_012_345_66_f64.to_bits())),
            "immutable provenance retains original USD REAL bits"
        );
        let fixed_reader =
            UsageStore::open_read_only_partitioned(&old.database_path, &identity).unwrap();
        assert_eq!(
            fixed_reader
                .load_session_events()
                .unwrap()
                .iter()
                .map(|event| event.total_tokens)
                .sum::<u64>(),
            15
        );
        let retired = payload["mappings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|mapping| mapping["original"] != mapping["canonical"])
            .unwrap()["original"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let mutation = Connection::open(&old.database_path).unwrap();
        let predicate="root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 AND range_start=?6 AND range_end=?7 AND record_sha256=?8 AND event_index=?9";
        let previous: (String, Option<String>) = mutation
            .query_row(
                &format!("SELECT total_tokens,input_tokens FROM session_events WHERE {predicate}"),
                rusqlite::params_from_iter(&retired),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        mutation.execute(&format!("UPDATE session_events SET total_tokens='99',input_tokens='99' WHERE {predicate}"),rusqlite::params_from_iter(&retired)).unwrap();
        assert!(
            fixed_reader.load_session_events().is_err(),
            "a new batch cannot reuse a proof after its retired raw inventory changes"
        );
        let mut restore = retired;
        restore.push(previous.0);
        restore.push(previous.1.unwrap());
        mutation
            .execute(
                &format!(
                    "UPDATE session_events SET total_tokens=?10,input_tokens=?11 WHERE {predicate}"
                ),
                rusqlite::params_from_iter(&restore),
            )
            .unwrap();
        assert_eq!(
            fixed_reader
                .load_session_events()
                .unwrap()
                .iter()
                .map(|event| event.total_tokens)
                .sum::<u64>(),
            15,
            "failed batch scope drops its cache before a restored new batch"
        );
        drop(mutation);
        drop(fixed_reader);
        run(None);
        let db = Connection::open(&old.database_path).unwrap();
        let after:(String,String)=db.query_row("SELECT fact_revision,(SELECT group_concat(prefix_json,'|') FROM recorded_usage_prefixes) FROM recorded_usage_state",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(
            after, before,
            "same accepted proof replay must be SQL no-op for facts/prefixes"
        );
        let restored = snapshot();
        assert_eq!(restored.0, original.0);
        for saved in &original.1 {
            assert!(restored.1.contains(saved));
        }
        for saved in &original.3 {
            assert!(
                restored.3.contains(saved),
                "noop retains the same durable observation"
            );
        }
        drop(db);
        // Reactivate A through the real account/main path. The ordinary cycle
        // must consume the same canonical view as historical reconstruction.
        fs::write(&auth, br#"{"tokens":{"account_id":"old-account"}}"#).unwrap();
        let reactivated = confirmed_fixture(&home, &data, start + 200);
        assert_eq!(reactivated.database_path, old.database_path);
        run(None);
        let db = Connection::open(&old.database_path).unwrap();
        let normal:(String,String)=db.query_row("SELECT fact_revision,(SELECT group_concat(prefix_json,'|') FROM recorded_usage_prefixes) FROM recorded_usage_state",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(
            normal,
            before,
            "normal cycle must not recount either side of the proof: {}",
            fs::read_to_string(root.path().join("startup.log")).unwrap()
        );
        let total: String = db
            .query_row(
                "SELECT total_tokens FROM session_model_totals WHERE model='gpt-6-astra'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            total, "15",
            "normal model totals must use exactly the same canonical event stream"
        );
        let restored = snapshot();
        assert_eq!(
            restored.0.len(),
            2,
            "normal collection must retain both proof-referenced checkpoints"
        );
        for saved in &original.1 {
            assert!(
                restored.1.contains(saved),
                "normal current repair cannot rewrite a past saved model observation"
            );
        }
        for saved in &original.2 {
            assert!(
                restored.2.contains(saved),
                "normal current repair cannot rewrite a past fixed legacy observation"
            );
        }
        for saved in &original.3 {
            assert!(
                restored.3.contains(saved),
                "normal collection retains the same durable observation"
            );
        }
        return;
    }
    if let Some(inode) = container_inode {
        let ranges = UsageStore::open_read_only_partitioned(&old.database_path, &identity)
            .unwrap()
            .load_session_ranges()
            .unwrap();
        for fault in [
            "container-range-missing",
            "container-hash-wrong",
            "original-hash-wrong",
        ] {
            let db = Connection::open(&old.database_path).unwrap();
            match fault {
                "container-range-missing" => {
                    db.execute(
                        "DELETE FROM session_ranges WHERE file_inode=?1",
                        [inode.to_string()],
                    )
                    .unwrap();
                }
                "container-hash-wrong" => {
                    db.execute(
                        "UPDATE session_ranges SET record_sha256=?1 WHERE file_inode=?2",
                        params!["ab".repeat(32), inode.to_string()],
                    )
                    .unwrap();
                }
                _ => {
                    db.execute(
                        "UPDATE session_ranges SET record_sha256=?1 WHERE file_inode=?2",
                        params!["cd".repeat(32), checkpoints[0].file_inode.to_string()],
                    )
                    .unwrap();
                }
            }
            drop(db);
            run(None);
            let db = Connection::open(&old.database_path).unwrap();
            let qualified: i64 = db
                .query_row(
                    "SELECT count(*) FROM session_events WHERE admission_json IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                qualified, 0,
                "both independent sets of accepted evidence are required: {fault}"
            );
            assert_eq!(
                UsageStore::open_read_only_partitioned(&old.database_path, &identity)
                    .unwrap()
                    .load_session_events()
                    .unwrap(),
                events,
                "failed byte proof must retain the old event values: {fault}"
            );
            db.execute("DELETE FROM session_ranges", []).unwrap();
            for range in &ranges {
                db.execute(
                    "INSERT INTO session_ranges VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        range.root_identity,
                        range.relative_path,
                        range.file_device.to_string(),
                        range.file_inode.to_string(),
                        range.start_offset,
                        range.end_offset,
                        format!("{:032x}", range.collector_epoch),
                        range.cycle_seq.to_string(),
                        format!("{:032x}", range.prefix_generation),
                        range.record_sha256
                    ],
                )
                .unwrap();
            }
        }
    }
    let failed = Connection::open(&old.database_path).unwrap();
    failed.execute_batch("CREATE TRIGGER reject_parser_repair BEFORE INSERT ON source_prefix_repairs BEGIN SELECT RAISE(ABORT,'sealed parser test rejection'); END;").unwrap();
    drop(failed);
    run(if selected_account {
        Some("sealed parser test rejection")
    } else {
        None
    });
    let failed = Connection::open(&old.database_path).unwrap();
    let untouched: (i64,i64,i64) = failed.query_row("SELECT (SELECT count(*) FROM session_events WHERE admission_json IS NOT NULL),(SELECT count(*) FROM session_checkpoints WHERE source_admission IS NOT NULL),(SELECT count(*) FROM source_prefix_repairs)",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(
        untouched,
        (0, 0, 0),
        "failed parser repair cannot publish qualification or provenance"
    );
    let failed_writer =
        UsageStore::open_read_only_partitioned(&old.database_path, &identity).unwrap();
    let mut failed_checkpoints = failed_writer
        .load_session_collection_state()
        .unwrap()
        .checkpoints;
    if selected_account {
        for checkpoint in &mut failed_checkpoints {
            checkpoint.cycle_seq = checkpoints[0].cycle_seq;
        }
    }
    assert_eq!(failed_checkpoints, checkpoints);
    assert_eq!(failed_writer.load_session_events().unwrap(), events);
    drop(failed_writer);
    failed
        .execute_batch("DROP TRIGGER reject_parser_repair")
        .unwrap();
    drop(failed);
    run(None);
    assert!(
        current.database_path.is_file(),
        "main must initialize authenticated account B"
    );
    let db = Connection::open(&old.database_path).unwrap();
    let qualified: i64 = db.query_row("SELECT COUNT(*) FROM session_events WHERE reset_at=?1 AND window_seconds=3600 AND admission_json IS NOT NULL",[reset],|row|row.get(0)).unwrap();
    assert_eq!(
        qualified,
        2,
        "account must recover the accepted original events against its own official timeline: {}",
        fs::read_to_string(root.path().join("startup.log")).unwrap()
    );
    let tokens:Option<String>=db.query_row("SELECT total_tokens FROM usage_model_history WHERE timestamp=?1 AND model='gpt-6-astra'",[start],|r|r.get(0)).optional().unwrap();
    assert_eq!(
        tokens.as_deref(),
        Some("10"),
        "main must restore inactive account A's source prefix"
    );
    let quota: Option<f64> = db
        .query_row(
            "SELECT remaining_percent FROM usage_history WHERE timestamp=?1 AND reset_at=?2",
            params![start + 120, reset],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
        .flatten();
    assert_eq!(quota, Some(88.0));
    drop(db);
    let writer = UsageStore::open_read_only_partitioned(&old.database_path, &identity).unwrap();
    let mut restored_events = writer.load_session_events().unwrap();
    if disproved_numbers {
        let raw = Connection::open(&old.database_path).unwrap();
        let facts = codex_info_db_writer::read_session_event_facts(&raw).unwrap();
        for (restored, expected) in restored_events.iter_mut().zip(&expected_events) {
            let fact = facts
                .iter()
                .find(|fact| fact.source.event_index == restored.event_index)
                .unwrap();
            let proof = match &fact.admission.as_ref().unwrap().numeric {
                codex_info_db_writer::event_admission::NumericEventProof::LegacyCounter(proof) => {
                    proof
                }
                _ => panic!("expected a source counter proof"),
            };
            assert_eq!(
                restored.event_index, proof.after.pointer.start,
                "replacement key names the exact raw record"
            );
            restored.event_index = expected.event_index;
        }
    }
    assert_eq!(restored_events, expected_events);
    let mut repaired_checkpoints = writer.load_session_collection_state().unwrap().checkpoints;
    for checkpoint in &mut repaired_checkpoints {
        assert!(
            checkpoint
                .source_admission
                .as_ref()
                .and_then(|state| state.account_binding.as_ref())
                .is_some(),
            "legacy parser qualification must recover from the accepted original prefix"
        );
        assert!(checkpoint.last_model_context_kind.is_some());
        assert!(checkpoint.last_model_interval.is_some());
        checkpoint.source_admission = None;
        checkpoint.last_model_context_kind = None;
        checkpoint.last_model_interval = None;
        if selected_account {
            checkpoint.cycle_seq = expected_checkpoints[0].cycle_seq;
        }
    }
    assert_eq!(
        repaired_checkpoints, expected_checkpoints,
        "repair must keep the same cursor and the raw state proven at that position"
    );
    assert_eq!(
        writer.load_session_collection_state().unwrap().model_totals[0].total_tokens,
        if stream_boundaries { 15 } else { 20 }
    );
    let repaired_state = writer.load_session_collection_state().unwrap();
    assert_eq!(
        repaired_state.collector_epoch,
        original_state.collector_epoch
    );
    if selected_account {
        assert!(repaired_state.cycle_seq > original_state.cycle_seq);
    } else {
        assert_eq!(
            repaired_state.cycle_seq, original_state.cycle_seq,
            "historical repair is not a Session collection cycle"
        );
    }
    assert_eq!(
        writer.latest_quota_receipt().unwrap(),
        Some(receipts[2].clone())
    );
    let proof = Connection::open(&old.database_path).unwrap();
    let payload: String = proof
        .query_row(
            "SELECT payload_json FROM source_prefix_repairs",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert!(payload["original_parser"]["source_admission"].is_null());
    assert!(payload["original_parser"]["last_model_context_kind"].is_null());
    assert!(payload["original_parser"]["last_model_interval"].is_null());
    if let Some(inode) = container_inode {
        assert_eq!(payload["read_container"]["file_inode"], inode);
        assert_eq!(
            payload["read_container"]["accepted_end"],
            checkpoints[0].committed_offset
        );
        assert!(!payload["read_container"]["ranges"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            payload["checkpoint"]["file_inode"], checkpoints[0].file_inode,
            "the reading container must not replace the old physical identity"
        );
    }
    if disproved_numbers {
        assert_eq!(payload["original_raw"]["total"], 999);
        assert_eq!(
            payload["qualified_raw"]["total"],
            if stream_boundaries { 25 } else { 20 }
        );
        assert!(payload["original"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fact| fact["tokens"]["total"] == 42));
        assert_eq!(payload["original"][0]["model"], "gpt-6-luna");
        assert_eq!(
            payload["numeric_replay"]["records"]
                .as_array()
                .unwrap()
                .len(),
            if stream_boundaries { 5 } else { 4 }
        );
    }
    if stream_boundaries {
        let records = payload["numeric_replay"]["records"].as_array().unwrap();
        assert_eq!(records[0]["raw"]["total"], 10);
        assert_eq!(records[0]["outcome"], "account-counter-baseline");
        assert_eq!(records[2]["before"]["total"], 20);
        assert_eq!(records[2]["raw"]["total"], 5);
        assert_eq!(
            records[2]["outcome"],
            "raw-counter-regression-or-invalid-delta"
        );
        assert_eq!(records[3]["before"]["total"], 20);
        assert_eq!(records[3]["raw"]["total"], 25);
        let rows = Connection::open(&old.database_path).unwrap();
        let complete: i64 = rows
            .query_row(
                "SELECT max(model_set_complete) FROM usage_model_history WHERE reset_at=?1",
                [reset],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            complete, 0,
            "a positive initial baseline cannot become full coverage"
        );
    }
    assert_eq!(
        payload["checkpoint"]["accepted_end"].as_u64(),
        Some(checkpoints[0].committed_offset)
    );
    assert_eq!(
        payload["qualified_parser"]["source_admission"]["account_binding"]["partition"]
            ["account_scope_id"],
        identity.account_scope_id
    );
    drop(proof);
    let fixed_generation = repaired_state.data_generation;
    drop(writer);
    run(None);
    let writer = UsageStore::open_read_only_partitioned(&old.database_path, &identity).unwrap();
    if !selected_account {
        assert_eq!(
            writer
                .load_session_collection_state()
                .unwrap()
                .data_generation,
            fixed_generation,
            "same accepted prefix after restart is a no-op"
        );
    } else {
        assert_eq!(writer.load_session_events().unwrap().len(), 2);
    }
    assert!(old.database_path.with_extension("sqlite3.bak.1").is_file());
}

#[test]
fn issue604_startup_requires_initial_account_confirmation_before_database() {
    fn run(root: &Path, mode: &str) -> (bool, String) {
        let log = root.join(format!("startup-{mode}.log"));
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_codex_info_recorder"))
                .args(["--once", "--interval-secs", "1"])
                .env("CODEX_HOME", root.join("home"))
                .env("CODEX_INFO_DATA_DIR", root.join("data"))
                .env("CODEX_INFO_CODEX_BIN", root.join("server.py"))
                .env("CODEX_INFO_ACCOUNT_ACTIVATION_UNIX", "1")
                .env("CONFIRMATION_MODE", mode)
                .env("CONFIRMATION_ROOT", root)
                .env_remove("CODEX_INFO_RECORDER_DB")
                .env_remove("CODEX_INFO_ACCOUNT_DB")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(File::create(&log).unwrap())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(12);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "confirmation fixture did not terminate: {mode}"
            );
            thread::sleep(Duration::from_millis(20));
        };
        (status.success(), fs::read_to_string(log).unwrap())
    }
    fn setup(root: &Path) {
        private_directory(&root.join("home"));
        private_directory(&root.join("home/sessions"));
        private_directory(&root.join("data"));
        fs::write(
            root.join("home/auth.json"),
            br#"{"tokens":{"account_id":"confirmation-fixture-A"}}"#,
        )
        .unwrap();
        fs::set_permissions(
            root.join("home/auth.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        fs::write(root.join("server.py"), r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ['CONFIRMATION_ROOT'])
mode = os.environ['CONFIRMATION_MODE']
reads = 0
for line in sys.stdin:
    message = json.loads(line)
    method = message.get('method')
    if 'id' not in message:
        continue
    result = {}
    error = None
    if method == 'account/read':
        reads += 1
        if not (root / 'initial-confirmation.done').exists() and list((root / 'data/history/accounts').glob('**/usage_history.sqlite3')):
            (root / 'premature-database').touch()
        if mode == 'error':
            error = {'code': -32000, 'message': 'fixture account read unavailable'}
        else:
            if mode == 'raw-switch' and reads == 1:
                auth = root / 'home/auth.json'
                auth.write_text('{"tokens":{"account_id":"confirmation-fixture-B"}}')
                auth.chmod(0o600)
            if mode == 'lease-replace' and reads == 1:
                temporary = root / 'data/history/replacement-lease'
                temporary.write_text('{}')
                temporary.chmod(0o600)
                temporary.replace(root / 'data/history/usage_record_daemon.lock')
            email = 'fixture@example.test' if mode != 'account-mismatch' or reads == 1 else 'other@example.test'
            result = {'account': {'type': 'chatgpt', 'email': email, 'planType': 'pro'}, 'requiresOpenaiAuth': True}
            if reads == 2 and mode == 'success':
                (root / 'initial-confirmation.done').touch()
    elif method == 'account/rateLimits/read':
        result = {'rateLimits': {'primary': {'usedPercent': 0, 'resetsAt': int(time.time()) + 604800, 'windowDurationMins': 10080}}}
    response = {'id': message['id'], 'error': error} if error else {'id': message['id'], 'result': result}
    print(json.dumps(response), flush=True)
"#).unwrap();
        fs::set_permissions(root.join("server.py"), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut failures = Vec::new();
    for mode in ["error", "account-mismatch", "raw-switch", "lease-replace"] {
        let root = tempfile::tempdir().unwrap();
        setup(root.path());
        let (success, log) = run(root.path(), mode);
        let registry: Value = serde_json::from_slice(
            &fs::read(root.path().join("data/history/account_profile_v1.json")).unwrap(),
        )
        .unwrap();
        let entries: Vec<_> = registry["accounts"].as_object().unwrap().values().collect();
        let unconfirmed = entries.iter().all(|entry| {
            entry["canonical_confirmation"].is_null()
                && entry["lifecycle_intervals"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
                && entry["state"] == "allocated"
        });
        let any_db = entries.iter().any(|entry| {
            root.path()
                .join("data/history/accounts/v1")
                .join(
                    registry["accounts"]
                        .as_object()
                        .unwrap()
                        .iter()
                        .find(|(_, value)| *value == *entry)
                        .unwrap()
                        .0,
                )
                .join(format!(
                    "epoch-{}",
                    entry["storage_epoch"].as_u64().unwrap()
                ))
                .join("usage_history.sqlite3")
                .exists()
        });
        failures.push((
            mode,
            !success && unconfirmed && !any_db && !root.path().join("premature-database").exists(),
            log,
        ));
    }
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    let began = chrono::Utc::now().timestamp();
    let (success, log) = run(root.path(), "success");
    let ended = chrono::Utc::now().timestamp();
    let read_registry = || -> Value {
        serde_json::from_slice(
            &fs::read(root.path().join("data/history/account_profile_v1.json")).unwrap(),
        )
        .unwrap()
    };
    let registry = read_registry();
    let (scope, entry) = registry["accounts"]
        .as_object()
        .unwrap()
        .iter()
        .next()
        .unwrap();
    let proof = entry["canonical_confirmation"].clone();
    let interval = entry["lifecycle_intervals"].clone();
    let valid = success
        && entry["state"] == "initialized"
        && proof["rule_version"] == 1
        && proof["profile_scope_id"] == registry["profile_scope_id"]
        && proof["account_scope_id"] == scope.as_str()
        && proof["storage_epoch"] == entry["storage_epoch"]
        && proof["began_at"]
            .as_i64()
            .is_some_and(|t| t >= began && t <= ended)
        && proof["ended_at"]
            .as_i64()
            .is_some_and(|t| t >= proof["began_at"].as_i64().unwrap_or(i64::MAX) && t <= ended)
        && interval.as_array().is_some_and(|v| {
            v.len() == 1 && v[0]["start_at"] == proof["began_at"] && v[0]["end_at"].is_null()
        })
        && !root.path().join("premature-database").exists();
    let (resumed, resume_log) = run(root.path(), "error");
    let after = read_registry();
    let stable = resumed
        && after["accounts"][scope]["canonical_confirmation"] == proof
        && after["accounts"][scope]["lifecycle_intervals"] == interval
        && after["next_storage_epoch"] == 2;
    for (mode, qualified, log) in failures {
        assert!(
            qualified,
            "initial confirmation failure leaked storage/ownership: {mode}\n{log}"
        );
    }
    assert!(
        valid,
        "successful initial confirmation was not committed before DB initialization\n{log}"
    );
    assert!(
        stable,
        "saved confirmation did not permit RPC-independent restart\n{resume_log}"
    );
}

#[test]
fn issue604_collector_binds_sources_before_checkpoint_and_usage() {
    verify_bound_source_event_collection(false, false, false, false, false, false, false);
}

#[test]
fn issue604_collector_recovers_original_event_qualification_from_accepted_prefix() {
    verify_bound_source_event_collection(true, false, false, false, false, false, false);
}

#[test]
fn issue604_collector_retires_only_the_original_counter_matched_to_a_response() {
    verify_bound_source_event_collection(false, true, false, false, false, false, false);
}

#[test]
fn issue604_collector_requalifies_only_the_matching_saved_legacy_family_key() {
    verify_bound_source_event_collection(false, false, true, false, false, false, false);
}

#[test]
fn issue604_qualified_live_append_progresses_before_old_parser_reconstruction() {
    verify_bound_source_event_collection(false, false, false, true, false, false, false);
}

#[test]
fn issue604_qualified_live_append_does_not_spend_its_budget_requalifying_old_events() {
    verify_bound_source_event_collection(false, false, false, false, true, false, false);
}

#[test]
fn issue604_exact_period_history_includes_prior_owned_intervals_after_reactivation() {
    verify_bound_source_event_collection(false, false, false, false, false, true, false);
}

#[test]
fn issue604_response_before_rpc_binding_stays_pending_without_blocking_the_source() {
    verify_bound_source_event_collection(false, false, false, false, false, false, true);
}

#[test]
fn issue604_numerical_replay_keeps_an_already_qualified_legacy_family_key() {
    verify_bound_source_event_collection(true, false, true, false, false, false, false);
}

#[test]
fn issue604_numerical_replay_does_not_alias_different_exact_models_of_one_family() {
    verify_bound_source_event_collection(true, true, true, false, false, false, false);
}

#[test]
fn issue604_numerical_replay_qualifies_the_original_unattributed_model() {
    verify_bound_source_event_collection(true, false, true, false, false, false, true);
}

fn verify_bound_source_event_collection(
    restore_original: bool,
    retire_counter: bool,
    legacy_family: bool,
    live_before_reconstruction: bool,
    live_before_original_repair: bool,
    reactivated: bool,
    prior_rpc_response: bool,
) {
    fn counter(total: u64, at: i64) -> String {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(at, 0)
            .unwrap()
            .to_rfc3339();
        format!(
            "{}\n",
            json!({"type":"event_msg","timestamp":timestamp,
            "payload":{"type":"token_count","info":{"total_token_usage":{
                "total_tokens":total,"input_tokens":total,"cached_input_tokens":0,"output_tokens":0
            }}}})
        )
    }
    fn setup() -> (tempfile::TempDir, String) {
        let root = match env::var_os("CODEX_INFO_TEST_EXPORT_BINDING_FIXTURE") {
            Some(export) => tempfile::tempdir_in(PathBuf::from(export).parent().unwrap()).unwrap(),
            None => tempfile::tempdir().unwrap(),
        };
        let home = root.path().join("home");
        private_directory(&home);
        private_directory(&home.join("sessions"));
        private_directory(&root.path().join("data"));
        let auth = home.join("auth.json");
        fs::write(&auth, br#"{"tokens":{"account_id":"binding-fixture-A"}}"#).unwrap();
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o600)).unwrap();
        let old = chrono::Utc::now().timestamp() - 30;
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(old, 0)
            .unwrap()
            .to_rfc3339();
        let original = format!(
            "{}\n{}\n{}",
            json!({"type":"session_meta","timestamp":timestamp,
                "payload":{"id":"binding-thread","session_id":"binding-session"}}),
            json!({"type":"turn_context","timestamp":timestamp,
                "payload":{"model":"gpt-6.1-sol","turn_id":"turn","root_turn_id":"root-turn"}}),
            counter(15, old)
        );
        let source = home.join("sessions/one.jsonl");
        fs::write(&source, &original).unwrap();
        let item = json!({
            "cliVersion":"0.147.0", "createdAt":old, "updatedAt":old,
            "cwd":root.path(), "ephemeral":false, "id":"binding-thread",
            "modelProvider":"openai", "preview":"fixture", "name":"fixture",
            "sessionId":"binding-session", "source":"cli",
            "status":{"type":"idle"}, "turns":[], "path":source
        });
        assert!(codex_info::thread_contract::validate_thread_item(&item).is_ok());
        fs::write(root.path().join("thread.json"), item.to_string()).unwrap();
        fs::write(
            root.path().join("reset-at"),
            (chrono::Utc::now().timestamp() + 604800).to_string(),
        )
        .unwrap();
        fs::write(root.path().join("server.py"), r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ['BINDING_FIXTURE_ROOT'])
mode = os.environ['BINDING_FIXTURE_MODE']
thread_seen = False
for line in sys.stdin:
    req = json.loads(line)
    if 'id' not in req:
        continue
    method = req.get('method')
    result, error = {}, None
    if mode == 'offline' and method != 'initialize':
        error = {'code':-32000,'message':'fixture RPC unavailable'}
    elif method == 'account/read':
        email = 'other@example.invalid' if thread_seen and mode == 'account-mismatch' else 'fixture@example.invalid'
        result = {'requiresOpenaiAuth':False,'account':{'type':'chatgpt','email':email,'planType':'pro'}}
    elif method == 'thread/read':
        thread_seen = True
        with (root / 'thread-reads').open('a') as calls:
            calls.write('thread/read\n')
        item = json.loads((root / 'thread.json').read_text())
        if mode == 'thread-error':
            error = {'code':-32000,'message':'fixture thread unavailable'}
        if mode == 'id-mismatch':
            item['id'] = 'another-thread'
        if mode == 'path-mismatch':
            item['path'] = str(root / 'missing.jsonl')
        if mode == 'raw-switch':
            auth = root / 'home/auth.json'
            auth.write_text('{"tokens":{"account_id":"binding-fixture-B"}}')
            auth.chmod(0o600)
        result = {'thread':item}
    elif method == 'account/rateLimits/read':
        used = 0
        if mode == 'repair-live':
            state = root / 'repair-percent'
            used = int(state.read_text()) + 1 if state.exists() else 1
            state.write_text(str(used))
        result = {'rateLimits':{'primary':{'usedPercent':used,'resetsAt':int((root / 'reset-at').read_text()),'windowDurationMins':10080}}}
    response = {'id':req['id'],'error':error} if error else {'id':req['id'],'result':result}
    print(json.dumps(response), flush=True)
"#).unwrap();
        fs::set_permissions(
            root.path().join("server.py"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        (root, original)
    }
    fn run(root: &Path, mode: &str, label: &str, bounded_repair: bool) -> (bool, String) {
        let log = root.join(format!("binding-{label}.log"));
        let args = if bounded_repair {
            vec!["--interval-secs", "1", "--chunk-bytes", "700"]
        } else if label == "live-before-repair" {
            vec!["--once", "--interval-secs", "1", "--chunk-bytes", "700"]
        } else {
            vec!["--once", "--interval-secs", "1"]
        };
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_codex_info_recorder"))
                .args(args)
                .env("CODEX_HOME", root.join("home"))
                .env("CODEX_INFO_DATA_DIR", root.join("data"))
                .env("CODEX_INFO_CODEX_BIN", root.join("server.py"))
                .env("BINDING_FIXTURE_ROOT", root)
                .env("BINDING_FIXTURE_MODE", mode)
                .env_remove("CODEX_INFO_RECORDER_DB")
                .env_remove("CODEX_INFO_ACCOUNT_DB")
                .stdin(if bounded_repair {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::null())
                .stderr(File::create(&log).unwrap())
                .spawn()
                .unwrap(),
        );
        eprintln!(
            "issue604_fixture label={label} recorder_pid={} source_bytes={}",
            child.0.id(),
            fs::metadata(root.join("home/sessions/one.jsonl"))
                .unwrap()
                .len()
        );
        if bounded_repair {
            let deadline = Instant::now() + Duration::from_secs(12);
            let mut failed_at = None;
            let mut released = false;
            loop {
                let (db, _) = database(root);
                if mode == "repair-live" {
                    let receipts: i64 = db
                        .query_row("SELECT count(*) FROM quota_receipts", [], |r| r.get(0))
                        .unwrap();
                    let text = fs::read_to_string(&log).unwrap();
                    if failed_at.is_none() && text.contains("forced pending qualification rollback")
                    {
                        // The next scheduled quota cycle can commit before
                        // this poll sees the failed transaction. Freeze the
                        // actual candidate input, not a later DB maximum.
                        let failed = text.find("forced pending qualification rollback").unwrap();
                        let revision: i64 = text[..failed]
                            .lines()
                            .rev()
                            .find_map(|line| {
                                line.strip_prefix(
                                    "codex-info-recorder diagnostic history_fixed_input revision=",
                                )
                                .and_then(|revision| revision.parse().ok())
                            })
                            .expect("failed repair must identify its fixed official input");
                        failed_at = Some((receipts, revision));
                        if label == "restore-after-source-change" {
                            use std::io::Write;
                            // A legitimate non-usage append invalidates the
                            // prepared file fingerprint while quota keeps running.
                            let timestamp = chrono::Utc::now().to_rfc3339();
                            let marker = format!(
                                "{}\n",
                                json!({"type":"event_msg","timestamp":timestamp,
                                "payload":{"type":"context_compacted"}})
                            );
                            fs::OpenOptions::new()
                                .append(true)
                                .open(root.join("home/sessions/one.jsonl"))
                                .unwrap()
                                .write_all(marker.as_bytes())
                                .unwrap();
                        }
                    }
                    if let Some((count, revision)) =
                        failed_at.filter(|(count, _)| receipts >= count + 2)
                    {
                        if !released {
                            let writable = Connection::open(db.path().unwrap()).unwrap();
                            writable
                                .execute_batch("DROP TRIGGER reject_pending_qualification")
                                .unwrap();
                            fs::write(root.join("pending-repair-input.json"),json!({"failure_receipts":count,"released_receipts":receipts,"fixed_revision":revision}).to_string()).unwrap();
                            released = true;
                        }
                    }
                    assert!(
                        !released || !text.contains("source qualification repair input changed"),
                        "fresh quota poisoned fixed pending repair: {text}"
                    );
                }
                let qualified: i64 = db
                    .query_row(
                        "SELECT count(*) FROM session_events WHERE admission_json IS NOT NULL",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                if qualified == 4 {
                    break;
                }
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "bounded repair owner exited: {}",
                    fs::read_to_string(&log).unwrap()
                );
                assert!(
                    Instant::now() < deadline,
                    "bounded full-prefix replay did not complete: {}",
                    fs::read_to_string(&log).unwrap()
                );
                thread::sleep(Duration::from_millis(20));
            }
            assert!(Command::new("kill")
                .args(["-TERM", "--", &child.0.id().to_string()])
                .status()
                .unwrap()
                .success());
        }
        let deadline = Instant::now() + Duration::from_secs(12);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "source fixture did not terminate: {label}\n{}",
                fs::read_to_string(&log).unwrap()
            );
            thread::sleep(Duration::from_millis(20));
        };
        (status.success(), fs::read_to_string(log).unwrap())
    }
    fn database(root: &Path) -> (Connection, Value) {
        let registry: Value = serde_json::from_slice(
            &fs::read(root.join("data/history/account_profile_v1.json")).unwrap(),
        )
        .unwrap();
        let (scope, entry) = registry["accounts"]
            .as_object()
            .unwrap()
            .iter()
            .next()
            .unwrap();
        let path = root
            .join("data/history/accounts/v1")
            .join(scope)
            .join(format!(
                "epoch-{}",
                entry["storage_epoch"].as_u64().unwrap()
            ))
            .join("usage_history.sqlite3");
        let db =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        db.execute_batch("PRAGMA query_only=ON").unwrap();
        (db, entry["canonical_confirmation"].clone())
    }
    type JournalReceipt = (i64, String, i64, bool, String);
    fn read_journal_receipt(
        root: &Path,
        sql: &str,
        values: &[&dyn rusqlite::ToSql],
    ) -> Option<JournalReceipt> {
        let path = root.join("data/history/quota/observations.sqlite3");
        if !path.is_file() {
            return None;
        }
        let db =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        db.busy_timeout(Duration::from_secs(1)).unwrap();
        db.execute_batch("PRAGMA query_only=ON").unwrap();
        db.query_row(sql, values, |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .optional()
        .unwrap()
    }
    fn latest_journal_receipt(root: &Path) -> Option<JournalReceipt> {
        read_journal_receipt(
            root,
            "SELECT sequence,receipt_id,observed_at,account_verified,account_scope
             FROM quota_observations ORDER BY sequence DESC LIMIT 1",
            &[],
        )
    }
    fn first_verified_journal_receipt_after(
        root: &Path,
        previous_sequence: Option<i64>,
    ) -> Option<JournalReceipt> {
        read_journal_receipt(
            root,
            "SELECT sequence,receipt_id,observed_at,account_verified,account_scope
             FROM quota_observations
             WHERE sequence>?1 AND account_verified=1
             ORDER BY sequence LIMIT 1",
            &[&previous_sequence.unwrap_or(0)],
        )
    }
    fn checkpoint_state(db: &Connection) -> (i64, String, Value) {
        let (offset, generation, state): (i64, String, String) = db.query_row(
            "SELECT committed_offset,prefix_generation,source_admission FROM session_checkpoints",
            [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).unwrap();
        (offset, generation, serde_json::from_str(&state).unwrap())
    }
    fn ledger(db: &Connection) -> Vec<(String, String)> {
        db.prepare("SELECT model,total_tokens FROM session_events ORDER BY timestamp,event_index")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    for mode in [
        "thread-error",
        "id-mismatch",
        "path-mismatch",
        "account-mismatch",
        "raw-switch",
    ] {
        let (root, _) = setup();
        let observed_begin = chrono::Utc::now().timestamp();
        let (_, log) = run(root.path(), mode, mode, false);
        let observed_end = chrono::Utc::now().timestamp();
        let (db, _) = database(root.path());
        let count: i64 = db
            .query_row("SELECT count(*) FROM session_checkpoints", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            count, 0,
            "unconfirmed source advanced a checkpoint: {mode}\n{log}"
        );
        assert!(ledger(&db).is_empty());
        assert_eq!(
            fs::read_to_string(root.path().join("thread-reads"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        let registry: Value = serde_json::from_slice(
            &fs::read(root.path().join("data/history/account_profile_v1.json")).unwrap(),
        )
        .unwrap();
        let entry = registry["accounts"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap();
        let end = &entry["lifecycle_intervals"][0]["end_at"];
        if mode == "raw-switch" {
            assert!(end.as_i64().is_some_and(|end| end >= observed_begin && end <= observed_end),
                "a detected raw loss must close old ownership before exit, without waiting for restart");
        } else {
            assert!(
                end.is_null(),
                "RPC-only failure cannot close local ownership: {mode}"
            );
        }
    }
    let (root, mut original) = setup();
    if reactivated {
        let reset: i64 = fs::read_to_string(root.path().join("reset-at"))
            .unwrap()
            .parse()
            .unwrap();
        fs::write(root.path().join("reset-at"), (reset - 3600).to_string()).unwrap();
    }
    if live_before_original_repair {
        let end = original.find('\n').unwrap();
        let mut metadata: Value = serde_json::from_str(&original[..end]).unwrap();
        metadata["irrelevant_padding"] = json!("x".repeat(1400));
        original = format!("{metadata}{}", &original[end..]);
        fs::write(root.path().join("home/sessions/one.jsonl"), &original).unwrap();
    }
    let began = chrono::Utc::now().timestamp();
    let (success, log) = run(root.path(), "success", "success", false);
    assert!(success, "source binding startup failed\n{log}");
    let ended = chrono::Utc::now().timestamp();
    let (db, canonical) = database(root.path());
    let (offset, generation, state) = checkpoint_state(&db);
    let binding = state["account_binding"].clone();
    assert_eq!(offset, original.len() as i64);
    assert_eq!(binding["version"], 1);
    assert_eq!(binding["prefix_end"], original.len() as u64);
    assert_eq!(binding["prefix_generation"], generation);
    assert_eq!(
        binding["metadata"],
        json!({"thread_id":"binding-thread","session_id":"binding-session"})
    );
    let source = root.path().join("home/sessions/one.jsonl");
    let metadata = fs::metadata(&source).unwrap();
    assert_eq!(binding["file_device"], metadata.dev());
    assert_eq!(binding["file_inode"], metadata.ino());
    assert_eq!(binding["relative_path"], "one.jsonl");
    let partition: (String, String, String, String) = db.query_row(
        "SELECT profile_scope_id,account_scope_id,storage_epoch,partition_id FROM storage_partition",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    ).unwrap();
    assert_eq!(
        binding["partition"],
        json!({"schema_version":"codex-info-account-db-v1",
        "profile_scope_id":partition.0,"account_scope_id":partition.1,
        "storage_epoch":partition.2.parse::<u64>().unwrap(),"partition_id":partition.3})
    );
    let proof = &binding["proof"];
    assert_eq!(proof["kind"], "rpc");
    let proof_begin = proof["began_at"].as_i64().unwrap();
    let proof_end = proof["ended_at"].as_i64().unwrap();
    assert!(proof_begin >= began && proof_end >= proof_begin && proof_end <= ended);
    assert!(proof["process"]["pid"].as_i64().unwrap() > 0);
    assert!(
        ledger(&db).is_empty(),
        "the initial cumulative 15 is not account consumption"
    );
    let initial_history_tokens: Option<i64> = db
        .query_row(
            "SELECT sol_tokens FROM usage_history ORDER BY timestamp DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        initial_history_tokens, None,
        "an unproved initial source baseline cannot publish a confirmed empty period"
    );
    drop(db);
    let previous_journal_sequence = latest_journal_receipt(root.path()).map(|receipt| receipt.0);

    // Keep the production owner resident until its asynchronous quota fetch
    // has durably completed. A one-shot owner can finish its collection before
    // the poller thread begins, so process exit alone is not acquisition proof.
    let log = root.path().join("binding-period-evidence-resident.log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_codex_info_recorder"));
    command
        .args(["--interval-secs", "60"])
        .env("CODEX_HOME", root.path().join("home"))
        .env("CODEX_INFO_DATA_DIR", root.path().join("data"))
        .env("CODEX_INFO_CODEX_BIN", root.path().join("server.py"))
        .env("BINDING_FIXTURE_ROOT", root.path())
        .env("BINDING_FIXTURE_MODE", "success")
        .env_remove("CODEX_INFO_RECORDER_DB")
        .env_remove("CODEX_INFO_ACCOUNT_DB")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&log).unwrap());
    command.process_group(0);
    let mut resident = ChildGuard(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(12);
    let acquired = loop {
        assert!(
            resident.0.try_wait().unwrap().is_none(),
            "period-evidence resident exited before a verified receipt was saved\n{}",
            fs::read_to_string(&log).unwrap()
        );
        if let Some(receipt) =
            first_verified_journal_receipt_after(root.path(), previous_journal_sequence)
        {
            break receipt;
        }
        assert!(
            Instant::now() < deadline,
            "resident did not durably save a verified post-binding quota receipt\n{}",
            fs::read_to_string(&log).unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert!(
        previous_journal_sequence.is_none_or(|previous| acquired.0 > previous),
        "the post-binding acquisition must have a new durable journal sequence: previous={previous_journal_sequence:?}, acquired={acquired:?}"
    );
    assert!(
        acquired.3,
        "the post-binding receipt must be verified against the same account: {acquired:?}"
    );
    assert!(
        acquired.2 >= proof_begin,
        "the post-binding durable receipt must be observed at or after source binding: observed_at={}, proof_begin={proof_begin}",
        acquired.2
    );
    assert!(
        !acquired.4.is_empty(),
        "verified receipt has no account scope"
    );
    assert!(Command::new("kill")
        .args(["-TERM", "--", &format!("-{}", resident.id())])
        .status()
        .unwrap()
        .success());
    let deadline = Instant::now() + Duration::from_secs(12);
    let status = loop {
        if let Some(status) = resident.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "resident did not finish after the verified journal commit\n{}",
            fs::read_to_string(&log).unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "resident did not exit through normal shutdown: {status}\n{}",
        fs::read_to_string(&log).unwrap()
    );

    // Replay offline after the acquisition owner has stopped. This exercises
    // the journal-to-account-DB restart path without allowing another fetch.
    let (success, replay_log) = run(root.path(), "offline", "period-evidence-replay", false);
    assert!(success, "official evidence replay failed\n{replay_log}");
    let (db, _) = database(root.path());
    let (receipt_sequence, now): (i64, i64) = db
        .query_row(
            "SELECT sequence,observed_at FROM quota_receipts WHERE receipt_id=?1",
            [acquired.1.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let account_scope: String = db
        .query_row(
            "SELECT account_scope_id FROM storage_partition",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        receipt_sequence == acquired.0 && now == acquired.2,
        "account DB must replay the exact durable post-binding receipt: journal={acquired:?}, account_db=({receipt_sequence},{now})"
    );
    assert_eq!(
        account_scope, acquired.4,
        "account DB receipt must retain the journal's confirmed account ownership"
    );
    let expected_reset: i64 = fs::read_to_string(root.path().join("reset-at"))
        .unwrap()
        .parse()
        .unwrap();
    drop(db);
    let appended = format!("{}{}", counter(20, now), counter(25, now));
    fs::write(&source, format!("{original}{appended}")).unwrap();
    if live_before_reconstruction || live_before_original_repair {
        use sha2::{Digest, Sha256};
        let (db, _) = database(root.path());
        let next: String = db
            .query_row("SELECT cycle_seq FROM collection_generation", [], |r| {
                r.get(0)
            })
            .unwrap();
        // Startup's owner commit and the collection each advance the cycle.
        // Put the legacy source first in the existing rotating order.
        let backlog_name = if (next.parse::<u64>().unwrap() + 2) % 2 == 0 {
            "aaa.jsonl"
        } else {
            "zzz.jsonl"
        };
        let backlog = format!("{original}{}", counter(15, now - 30).repeat(20));
        assert!(backlog.len() > 1400 && appended.len() < 700);
        let path = root.path().join("home/sessions").join(backlog_name);
        fs::write(&path, &backlog).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let writable = Connection::open(db.path().unwrap()).unwrap();
        writable.execute("INSERT INTO session_checkpoints SELECT root_identity,?1,?2,?3,?4,discard_until_lf,collector_epoch,cycle_seq,prefix_generation,prefix_sha256,fully_attributed_from_zero,token_baseline_known,last_model,previous_total,previous_input,previous_cached_input,previous_output,last_task_running,previous_cache_write_input,history_base_pending,context_usage_tokens,context_window_tokens,NULL,NULL,NULL FROM session_checkpoints WHERE relative_path='one.jsonl'",
            rusqlite::params![backlog_name,metadata.dev().to_string(),metadata.ino().to_string(),backlog.len() as i64]).unwrap();
        let hash = format!("{:x}", Sha256::digest(Sha256::digest(backlog.as_bytes())));
        writable.execute("INSERT INTO session_ranges SELECT root_identity,?1,?2,?3,0,?4,collector_epoch,cycle_seq,prefix_generation,?5 FROM session_checkpoints WHERE relative_path=?1",
            rusqlite::params![backlog_name,metadata.dev().to_string(),metadata.ino().to_string(),backlog.len() as i64,hash]).unwrap();
        if live_before_original_repair {
            assert!(original.len() > 1400);
            writable.execute("INSERT INTO session_events SELECT root_identity,relative_path,file_device,file_inode,prefix_generation,start_offset,end_offset,record_sha256,0,?1,'gpt-6-luna','9','9','0','0','0',NULL,NULL,NULL FROM session_ranges WHERE relative_path='one.jsonl' LIMIT 1",[now-30]).unwrap();
        }
        drop(writable);
        drop(db);
        let (success, log) = run(root.path(), "offline", "live-before-repair", false);
        assert!(success, "{log}");
        let (db, _) = database(root.path());
        let expected = if live_before_original_repair {
            vec![
                ("gpt-6-luna".to_owned(), "9".to_owned()),
                ("gpt-6.1-sol".to_owned(), "5".to_owned()),
            ]
        } else {
            vec![("gpt-6.1-sol".to_owned(), "5".to_owned())]
        };
        assert_eq!(ledger(&db),expected,"an already qualified append must progress in one bounded cycle while old reconstruction remains pending: {log}");
        let live_end: i64 = db
            .query_row(
                "SELECT committed_offset FROM session_checkpoints WHERE relative_path='one.jsonl'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live_end, (original.len() + appended.len()) as i64);
        if live_before_original_repair {
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_pending_ranges WHERE relative_path='one.jsonl' AND reason='source-original-qualification-unresolved')",[],|r|r.get::<_,bool>(0)).unwrap(),"deferred old numerical qualification remains explicit, not confirmed/zero");
        }
        let old: Option<String> = db
            .query_row(
                "SELECT source_admission FROM session_checkpoints WHERE relative_path=?1",
                [backlog_name],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            old.is_none(),
            "a partly replayed old source cannot publish complete parser qualification"
        );
        return;
    }
    let (resumed, log) = run(root.path(), "offline", "resume", false);
    assert!(
        resumed,
        "saved source binding must survive RPC failure\n{log}"
    );
    let (db, after_canonical) = database(root.path());
    let saved = checkpoint_state(&db);
    assert_eq!(saved.0, (original.len() + appended.len()) as i64);
    assert_eq!(saved.2["account_binding"], binding);
    assert_eq!(after_canonical, canonical);
    assert_eq!(ledger(&db), [("gpt-6.1-sol".to_owned(), "5".to_owned())]);
    let (event_reset, event_window, admission): (Option<i64>, Option<i64>, Option<String>) = db
        .query_row(
            "SELECT reset_at,window_seconds,admission_json FROM session_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let admission: Value = serde_json::from_str(
        &admission.expect("accepted source consumption must retain its original qualification"),
    )
    .unwrap();
    assert_eq!(admission["version"], 1);
    assert_eq!(admission["source_binding"], binding);
    assert_eq!(admission["verified_prefix_end"], saved.0 as u64);
    let registry: Value = serde_json::from_slice(
        &fs::read(root.path().join("data/history/account_profile_v1.json")).unwrap(),
    )
    .unwrap();
    let entry = registry["accounts"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(admission["ownership"], entry["lifecycle_intervals"][0]);
    let numeric = &admission["numeric"];
    assert_eq!(numeric["kind"], "legacy-counter");
    use sha2::{Digest, Sha256};
    let before = counter(20, now);
    let after = counter(25, now);
    for (name, start, bytes, total) in [
        ("before", original.len(), before.as_bytes(), 20),
        ("after", original.len() + before.len(), after.as_bytes(), 25),
    ] {
        assert_eq!(
            numeric[name],
            json!({
                "pointer":{"start":start,"end":start+bytes.len(),
                    "sha256":format!("{:x}",Sha256::digest(bytes)),"timestamp":now},
                "total":total,"input":total,"cached_input":0,"cache_write_input":null,"output":0
            })
        );
    }
    let context_start = original.find('\n').unwrap() + 1;
    let context_end = context_start + original[context_start..].find('\n').unwrap() + 1;
    assert_eq!(
        numeric["model_context"],
        json!({
            "pointer":{"start":context_start,"end":context_end,
                "sha256":format!("{:x}",Sha256::digest(&original.as_bytes()[context_start..context_end]))},
        "kind":"turn","interval":"none"
        })
    );
    assert_eq!(
        event_reset,
        Some(expected_reset),
        "an accepted original event at a qualified official observation point must be attributed"
    );
    assert_eq!(event_window, Some(604800));
    assert!(!admission["period"].is_null());
    assert!(admission["period"]["lower_at"].as_i64().unwrap() <= now);
    assert!(admission["period"]["upper_at"].as_i64().unwrap() >= now);
    if restore_original && legacy_family {
        use codex_info_db_writer::{SessionLifecycleInterval, StoragePartitionIdentity};
        use codex_info_recorder::{AccountEpochProof, Recorder, RecorderConfig};
        let identity: StoragePartitionIdentity =
            serde_json::from_value(binding["partition"].clone()).unwrap();
        let mut family = admission.clone();
        let mut numeric = family["numeric"].clone();
        numeric.as_object_mut().unwrap().remove("kind");
        if prior_rpc_response {
            family["numeric"]["model_context"] = Value::Null;
            family["period"] = Value::Null;
        } else {
            family["numeric"] = json!({"kind":"legacy-family-counter","proof":numeric,"exact_model":if retire_counter { "gpt-5.6-sol" } else { "gpt-6.1-sol" }});
        }
        let pointer_start = admission["numeric"]["after"]["pointer"]["start"]
            .as_u64()
            .unwrap();
        let path = db.path().unwrap().to_owned();
        drop(db);
        let writable = Connection::open(&path).unwrap();
        writable
            .execute(
                "UPDATE session_events SET model=?3,event_index=?1,admission_json=?2,reset_at=?4,window_seconds=?5",
                rusqlite::params![pointer_start, family.to_string(), if prior_rpc_response { "UNATTRIBUTED" } else { "SOL" }, (!prior_rpc_response).then_some(expected_reset), (!prior_rpc_response).then_some(604800)],
            )
            .unwrap();
        // An unqualified initial baseline makes the original-prefix repair
        // necessary; it is not a second confirmed use of this response.
        writable.execute("INSERT INTO session_events SELECT root_identity,relative_path,file_device,file_inode,prefix_generation,0,?1,?2,999,timestamp,'gpt-6.1-sol','15','15','0','0',NULL,NULL,NULL,NULL FROM session_events LIMIT 1",rusqlite::params![original.len(),format!("{:x}",Sha256::digest(Sha256::digest(original.as_bytes())))]).unwrap();
        writable
            .execute(
                "UPDATE session_checkpoints SET previous_total='24',previous_input='24'",
                [],
            )
            .unwrap();
        let saved = codex_info_db_writer::read_session_event_facts(&writable)
            .unwrap()
            .into_iter()
            .find(|fact| fact.admission.is_some())
            .unwrap();
        drop(writable);
        let owners = entry["lifecycle_intervals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|interval| {
                SessionLifecycleInterval::new(
                    interval["start_at"].as_i64(),
                    interval["end_at"].as_i64(),
                )
            })
            .collect();
        let mut recorder = Recorder::open_partitioned_for_account_with_lifecycle(
            RecorderConfig {
                sessions_root: root.path().join("home/sessions"),
                chunk_bytes: 65536,
            },
            &path,
            &identity,
            entry["activation_timestamp"].as_i64(),
            None,
            owners,
        )
        .unwrap();
        let epoch = AccountEpochProof::capture(&root.path().join("home")).unwrap();
        let collected = recorder.run_account_cycle_with_stored_quota_guarded(
            &root.path().join("home"),
            &epoch,
            || true,
        );
        if retire_counter {
            let error = collected.expect_err(
                "a different exact model in the same family is not the qualified original",
            );
            assert!(error
                .to_string()
                .contains("qualification repair cannot replace numeric/source facts"));
            let (db, _) = database(root.path());
            let facts = codex_info_db_writer::read_session_event_facts(&db).unwrap();
            assert_eq!(
                facts.iter().find(|fact| fact.source == saved.source),
                Some(&saved)
            );
            assert_eq!(facts.len(), 2);
            assert_eq!(
                recorder.state().unwrap().checkpoints[0].previous_total,
                Some(24)
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_prefix_repairs", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                0
            );
            return;
        }
        collected.expect("raw checkpoint repair must preserve qualified labels and recover only missing original model context");
        let (db, _) = database(root.path());
        let facts = codex_info_db_writer::read_session_event_facts(&db).unwrap();
        let retained = facts
            .iter()
            .find(|fact| fact.source == saved.source)
            .expect("the saved family pointer must survive");
        assert_eq!(
            retained.model,
            if prior_rpc_response {
                "gpt-6.1-sol"
            } else {
                "SOL"
            }
        );
        assert_eq!(retained.tokens, saved.tokens);
        if prior_rpc_response {
            assert_eq!(retained.period.unwrap().reset_at, expected_reset);
            use codex_info_db_writer::event_admission::NumericEventProof;
            let (NumericEventProof::LegacyCounter(before), NumericEventProof::LegacyCounter(after)) = (
                &saved.admission.as_ref().unwrap().numeric,
                &retained.admission.as_ref().unwrap().numeric,
            ) else {
                panic!("original counter proof must survive model qualification")
            };
            assert_eq!(before.before, after.before);
            assert_eq!(before.after, after.after);
            assert!(before.model_context.is_none());
            assert!(after.model_context.is_some());
            let provenance: String = db.query_row("SELECT payload_json FROM source_prefix_repairs WHERE json_extract(payload_json,'$.original[0].model')='UNATTRIBUTED' OR json_extract(payload_json,'$.original[1].model')='UNATTRIBUTED'", [], |row| row.get(0)).unwrap();
            let provenance: Value = serde_json::from_str(&provenance).unwrap();
            let originals = provenance["original"].as_array().unwrap();
            let original = originals
                .iter()
                .find(|fact| fact["model"] == "UNATTRIBUTED")
                .unwrap();
            assert_eq!(original["tokens"]["total"], 5);
            assert_eq!(original["source"]["event_index"], pointer_start);
            assert_eq!(original["admission"], family);
        } else {
            assert_eq!(
                retained.admission.as_ref().unwrap().numeric,
                saved.admission.as_ref().unwrap().numeric
            );
        }
        assert_eq!(
            facts.iter().filter(|fact| fact.admission.is_some()).count(),
            1
        );
        assert_eq!(
            recorder.state().unwrap().checkpoints[0].previous_total,
            Some(25)
        );
        return;
    }
    if prior_rpc_response {
        // This source existed during the account's ownership, before this
        // physical source acquired its first RPC account binding.
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(now, 0)
            .unwrap()
            .to_rfc3339();
        let usage = json!({"total_tokens":7,"input_tokens":5,"cached_input_tokens":1,"cache_write_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":1});
        let bytes = format!(
            "{}\n{}\n{}\n{}\n",
            json!({"type":"session_meta","payload":{"id":"binding-thread","session_id":"binding-session"}}),
            json!({"type":"turn_context","timestamp":timestamp,"payload":{"model":"gpt-6.1-sol","turn_id":"turn","root_turn_id":"root-turn"}}),
            json!({"type":"token_usage_record","timestamp":timestamp,"payload":{"thread_id":"binding-thread","session_id":"binding-session","root_turn_id":"root-turn","turn_id":"turn","response_id":"older-root","usage":usage,"turn_token_usage":usage,"thread_token_usage":usage}}),
            json!({"type":"event_msg","timestamp":timestamp,"payload":{"type":"token_count","info":{"total_token_usage":usage,"last_token_usage":usage}}}),
        );
        let other = root.path().join("home/sessions/two.jsonl");
        fs::write(&other, &bytes).unwrap();
        let mut item: Value =
            serde_json::from_slice(&fs::read(root.path().join("thread.json")).unwrap()).unwrap();
        item["path"] = json!(other);
        fs::write(root.path().join("thread.json"), item.to_string()).unwrap();
        while chrono::Utc::now().timestamp() <= now {
            thread::sleep(Duration::from_millis(20));
        }
        drop(db);
        let (success, log) = run(root.path(), "success", "prior-rpc-response", false);
        assert!(success, "{log}");
        let (db, _) = database(root.path());
        let stored: Option<(i64,String)> = db.query_row("SELECT committed_offset,source_admission FROM session_checkpoints WHERE relative_path='two.jsonl'",[], |r|Ok((r.get(0)?,r.get(1)?))).optional().unwrap();
        assert!(
            stored.is_some(),
            "an unqualified historical response must not abort the source transaction: {log}"
        );
        let (end, state) = stored.unwrap();
        assert_eq!(end, bytes.len() as i64);
        let state: Value = serde_json::from_str(&state).unwrap();
        assert!(
            state["account_binding"]["proof"]["began_at"]
                .as_i64()
                .unwrap()
                > now
        );
        assert_eq!(state["responses"].as_array().unwrap().len(), 1);
        assert_eq!(state["responses"][0]["pointer"]["timestamp"], now);
        assert_eq!(ledger(&db), [("gpt-6.1-sol".to_owned(), "5".to_owned())]);
        assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_pending_ranges WHERE relative_path='two.jsonl' AND reason='response-source-qualification-unresolved')", [], |r|r.get::<_,bool>(0)).unwrap());
        return;
    }
    if reactivated {
        use codex_info_db_writer::{
            period_evidence::OfficialQuotaInput, SessionLifecycleInterval,
            StoragePartitionIdentity, UsageStore,
        };
        use codex_info_recorder::{AccountEpochProof, Recorder, RecorderConfig};
        let identity: StoragePartitionIdentity =
            serde_json::from_value(binding["partition"].clone()).unwrap();
        let next_start = now + 2;
        let owners = vec![
            SessionLifecycleInterval::new(
                Some(admission["ownership"]["start_at"].as_i64().unwrap()),
                Some(now + 1),
            ),
            SessionLifecycleInterval::new(Some(next_start), None),
        ];
        let path = db.path().unwrap().to_owned();
        drop(db);
        while chrono::Utc::now().timestamp() < next_start {
            thread::sleep(Duration::from_millis(20));
        }
        let mut store = UsageStore::open_partitioned(&path, &identity).unwrap();
        // A migrated raw aggregate may include unassigned/other-period usage.
        // It is retained evidence, not the prefix of this exact period.
        Connection::open(&path).unwrap().execute(
            "INSERT INTO session_model_totals(model,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens) VALUES('gpt-6-luna','70','70','0','0','0')", []
        ).unwrap();
        store
            .stage_official_quota_inputs(&[OfficialQuotaInput {
                input_id: "e".repeat(64),
                account_scope: identity.account_scope_id.clone(),
                sequence: 900000,
                observed_at: next_start,
                content_sha256: "f".repeat(64),
                reset_at: Some(expected_reset),
                window_seconds: Some(604800),
                reason: None,
            }])
            .unwrap();
        // Advance the independent observation clock across a minute; no
        // source timestamp or source byte is rewritten by this observation.
        store
            .commit_quota_receipts(
                &[codex_info_db_writer::QuotaReceipt {
                    receipt_id: "d".repeat(64),
                    sequence: 900001,
                    observed_at: next_start + 120,
                    reset_at: expected_reset,
                    window_seconds: 604800,
                    remaining_percent: 80.0,
                }],
                false,
            )
            .unwrap();
        // Retain a legacy offset whose earlier prefix is not covered by the
        // qualified source set. New collection must not guess or delete it.
        let saved_generation = store
            .load_session_collection_state()
            .unwrap()
            .data_generation;
        let raw = Connection::open(&path).unwrap();
        let range: Vec<String> = raw.query_row("SELECT root_identity,relative_path,file_device,file_inode,CAST(start_offset AS TEXT),CAST(end_offset AS TEXT),collector_epoch,cycle_seq,prefix_generation,record_sha256 FROM session_ranges WHERE relative_path='one.jsonl' ORDER BY end_offset DESC LIMIT 1", [], |row|(0..10).map(|index|row.get(index)).collect()).unwrap();
        let offset_end = (next_start + 59).div_euclid(60) * 60 + 60;
        let models = json!([["gpt-6.1-sol", 5, 5, 0, 0, null]]);
        let payload = serde_json::to_string(&json!([
            identity.partition_id,
            expected_reset,
            604800,
            saved_generation - 1,
            offset_end,
            [],
            [range],
            [[
                (now - 120).div_euclid(60) * 60,
                models,
                [0.000025, 0.0, 0.0]
            ]],
            models,
            [0.000025, 0.0, 0.0]
        ]))
        .unwrap();
        let offset_id = format!("{:x}", Sha256::digest(payload.as_bytes()));
        raw.execute(
            "INSERT INTO session_timeline_recoveries VALUES(?1,?2,?3)",
            rusqlite::params![offset_id, payload, saved_generation.to_string()],
        )
        .unwrap();
        let original_minute: i64 = raw.query_row("SELECT timestamp FROM usage_history WHERE sol_tokens=5 ORDER BY timestamp DESC LIMIT 1", [], |row|row.get(0)).unwrap();
        let preserved_row = |db: &Connection| -> Vec<rusqlite::types::Value> {
            db.query_row(
                "SELECT * FROM usage_history WHERE reset_at=?1 AND timestamp=?2",
                rusqlite::params![expected_reset, original_minute],
                |row| {
                    (0..9)
                        .map(|index| {
                            row.get::<_, rusqlite::types::Value>(index)
                                .map(|value| match value {
                                    rusqlite::types::Value::Real(value) => {
                                        rusqlite::types::Value::Text(format!(
                                            "REAL:{:016x}",
                                            value.to_bits()
                                        ))
                                    }
                                    other => other,
                                })
                        })
                        .collect()
                },
            )
            .unwrap()
        };
        let original_row = preserved_row(&raw);
        let observation_values = |db: &Connection| -> String {
            db.query_row("SELECT json_remove(snapshot_json,'$.model_source','$.empty_model_set_complete') FROM durable_state WHERE singleton>=2 AND data_generation=?1 AND json_extract(snapshot_json,'$.reset_at')=?2", rusqlite::params![original_minute,expected_reset], |row|row.get(0)).unwrap()
        };
        let original_observation_values = observation_values(&raw);
        drop(store);
        let bytes = format!(
            "{original}{appended}{}{}",
            counter(25, next_start),
            counter(30, next_start)
        );
        fs::write(&source, &bytes).unwrap();
        let mut recorder = Recorder::open_partitioned_for_account_with_lifecycle(
            RecorderConfig {
                sessions_root: root.path().join("home/sessions"),
                chunk_bytes: 65536,
            },
            &path,
            &identity,
            Some(next_start),
            None,
            owners,
        )
        .unwrap();
        let epoch = AccountEpochProof::capture(&root.path().join("home")).unwrap();
        recorder
            .run_account_cycle_with_stored_quota_guarded(&root.path().join("home"), &epoch, || true)
            .expect("the exact-period prefix includes both independently owned intervals");
        let state = recorder.state().unwrap();
        assert_eq!(state.model_totals[0].total_tokens, 10);
        assert_eq!(state.checkpoints[0].committed_offset, bytes.len() as u64);
        let (db, _) = database(root.path());
        let latest: String = db.query_row("SELECT total_tokens FROM usage_model_history WHERE reset_at=?1 AND model='gpt-6.1-sol' ORDER BY timestamp DESC LIMIT 1", [expected_reset], |r|r.get(0)).unwrap();
        assert_eq!(
            latest, "10",
            "old and current owned interval facts form one exact period"
        );
        assert_eq!(
            db.query_row(
                "SELECT payload_json FROM session_timeline_recoveries WHERE recovery_id=?1",
                [&offset_id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            payload
        );
        assert_eq!(preserved_row(&db), original_row);
        assert_eq!(observation_values(&db), original_observation_values);
        assert_eq!(db.query_row("SELECT json_extract(snapshot_json,'$.model_source') FROM durable_state WHERE singleton>=2 AND data_generation=?1 AND json_extract(snapshot_json,'$.reset_at')=?2", rusqlite::params![original_minute,expected_reset], |row|row.get::<_,String>(0)).unwrap(), "legacy-unknown");
        assert_eq!(db.query_row("SELECT model_set_complete FROM usage_model_history WHERE reset_at=?1 AND model='gpt-6.1-sol' ORDER BY timestamp DESC LIMIT 1", [expected_reset], |row|row.get::<_,i64>(0)).unwrap(), 0);
        assert_eq!(
            ledger(&db),
            vec![("gpt-6.1-sol".to_owned(), "5".to_owned()); 2]
        );
        return;
    }
    let writable = Connection::open(db.path().unwrap()).unwrap();
    writable.execute_batch(
        "CREATE TRIGGER reject_event_admission_insert BEFORE INSERT ON session_events
         WHEN NEW.admission_json IS NOT NULL BEGIN SELECT RAISE(ABORT,'fixture admission write'); END;
         CREATE TRIGGER reject_event_admission_update BEFORE UPDATE OF admission_json ON session_events
         BEGIN SELECT RAISE(ABORT,'fixture admission write'); END;",
    ).unwrap();
    drop(db);

    let more = counter(30, now);
    fs::write(&source, format!("{original}{appended}{more}")).unwrap();
    let (_, failed_log) = run(root.path(), "offline", "admission-rollback", false);
    assert!(
        failed_log.contains("fixture admission write"),
        "{failed_log}"
    );
    let (db, _) = database(root.path());
    assert_eq!(
        checkpoint_state(&db),
        saved,
        "qualification and cursor must commit together"
    );
    assert_eq!(ledger(&db), [("gpt-6.1-sol".to_owned(), "5".to_owned())]);
    writable.execute_batch("DROP TRIGGER reject_event_admission_insert; DROP TRIGGER reject_event_admission_update;").unwrap();
    drop(writable);
    drop(db);
    let (recovered, recovered_log) = run(root.path(), "offline", "admission-retry", false);
    assert!(recovered, "{recovered_log}");
    let (db, _) = database(root.path());
    let saved = checkpoint_state(&db);
    assert_eq!(
        saved.0,
        (original.len() + appended.len() + more.len()) as i64
    );
    assert_eq!(
        ledger(&db),
        vec![("gpt-6.1-sol".to_owned(), "5".to_owned()); 2]
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM session_events WHERE admission_json IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    let compatibility_dollars: Option<f64> = db
        .query_row(
            "SELECT sol_dollars FROM usage_history ORDER BY timestamp DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(compatibility_dollars.map(f64::to_bits),Some(0x3f0a36e2eb1c432d),"ten qualified SOL input tokens use the frozen compatibility tariff and operation order independently of exact-model prices");
    drop(db);

    // The next source point has no official observation on its right. Keep
    // its valid numeric event, but do not extrapolate the last receipt into a
    // period or let the current-period accumulator count it by timestamp.
    let unassigned = counter(35, now + 1);
    fs::write(&source, format!("{original}{appended}{more}{unassigned}")).unwrap();
    let (success, log) = run(root.path(), "offline", "unassigned-current", false);
    assert!(success, "{log}");
    let (db, _) = database(root.path());
    let saved = checkpoint_state(&db);
    assert_eq!(
        ledger(&db),
        vec![("gpt-6.1-sol".to_owned(), "5".to_owned()); 3]
    );
    let unassigned_period: Option<i64> = db
        .query_row(
            "SELECT reset_at FROM session_events ORDER BY timestamp DESC,event_index DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unassigned_period, None);
    let current_tokens: String = db
        .query_row(
            "SELECT total_tokens FROM session_model_totals WHERE model='gpt-6.1-sol'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        current_tokens, "10",
        "an unassigned event cannot enter the current period by its timestamp"
    );
    drop(db);

    let response = |total: u64, cumulative: u64| {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(now, 0)
            .unwrap()
            .to_rfc3339();
        let usage = json!({"total_tokens":total,"input_tokens":total-2,"cached_input_tokens":1,
            "cache_write_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":1});
        format!(
            "{}\n{}\n",
            json!({"type":"token_usage_record","timestamp":timestamp,
            "payload":{"thread_id":"binding-thread","session_id":"binding-session",
                "root_turn_id":"root-turn","turn_id":"turn","response_id":"normal-response",
                "usage":usage,"turn_token_usage":usage,"thread_token_usage":usage}}),
            json!({"type":"event_msg","timestamp":timestamp,"payload":{"type":"token_count","info":{
                "total_token_usage":{"total_tokens":cumulative,"input_tokens":cumulative-2,"cached_input_tokens":1,"cache_write_input_tokens":0,"output_tokens":2},
                "last_token_usage":usage}}})
        )
    };
    let root_response = response(7, 42);
    let (root_record, closing_counter) = root_response.split_once('\n').unwrap();
    let open_source = format!("{original}{appended}{more}{unassigned}{root_record}\n");
    fs::write(&source, &open_source).unwrap();
    let (success, log) = run(root.path(), "offline", "open-response", false);
    assert!(success, "{log}");
    let (db, _) = database(root.path());
    assert_eq!(db.query_row("SELECT count(*) FROM session_events WHERE json_extract(admission_json,'$.numeric.kind')='response-record'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_pending_ranges WHERE reason='response-qualification-pending')",[],|r|r.get::<_,bool>(0)).unwrap());
    let pending_state = checkpoint_state(&db);
    assert_eq!(pending_state.0, open_source.len() as i64);
    drop(db);
    assert!(!closing_counter.is_empty());
    let mut accepted_source = format!("{original}{appended}{more}{unassigned}{root_response}");
    fs::write(&source, &accepted_source).unwrap();
    let (success, log) = run(root.path(), "offline", "canonical-response", false);
    assert!(success, "{log}");
    let (db, _) = database(root.path());
    let response_rows:i64=db.query_row("SELECT count(*) FROM session_events WHERE json_extract(admission_json,'$.numeric.kind')='response-record'",[],|r|r.get(0)).unwrap();
    assert_eq!(
        response_rows, 1,
        "a classified qualified root must reach the canonical event ledger"
    );
    let root_values:(String,Option<i64>,i64,String)=db.query_row("SELECT total_tokens,reset_at,timestamp,admission_json FROM session_events WHERE json_extract(admission_json,'$.numeric.kind')='response-record'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(root_values.0, "7");
    assert_eq!(root_values.1, Some(expected_reset));
    assert_eq!(root_values.2, now);
    let response_admission: Value = serde_json::from_str(&root_values.3).unwrap();
    assert_eq!(
        response_admission["numeric"]["candidate"]["record"]["usage"],
        json!({"total":7,"input":5,"cached_input":1,"cache_write_input":0,"output":2,"reasoning_output":1})
    );
    let first_root_admission = root_values.3;
    let after_root = checkpoint_state(&db);
    assert!(after_root.0 > saved.0);
    drop(db);
    // Same ordered 5-ID key, timestamp and all six usage components is one
    // response even at another source byte position and after restart.
    accepted_source.push_str(&response(7, 49));
    fs::write(&source, &accepted_source).unwrap();
    let (success, log) = run(root.path(), "offline", "response-replay", false);
    assert!(success, "{log}");
    let (db, _) = database(root.path());
    assert_eq!(db.query_row("SELECT count(*) FROM session_events WHERE json_extract(admission_json,'$.numeric.kind')='response-record'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(
        db.query_row(
            "SELECT total_tokens FROM session_model_totals WHERE model='gpt-6.1-sol'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "17"
    );
    assert_eq!(db.query_row("SELECT admission_json FROM session_events WHERE json_extract(admission_json,'$.numeric.kind')='response-record'",[],|r|r.get::<_,String>(0)).unwrap(),first_root_admission);
    drop(db);
    // A differing usage for that same key is retained as a conflict and may
    // not replace the original seven-token event or add another consumption.
    accepted_source.push_str(&response(8, 57));
    fs::write(&source, &accepted_source).unwrap();
    let (success, log) = run(root.path(), "offline", "response-conflict", false);
    assert!(success, "{log}");
    let (db, _) = database(root.path());
    assert_eq!(
        db.query_row("SELECT count(*) FROM session_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        db.query_row(
            "SELECT total_tokens FROM session_model_totals WHERE model='gpt-6.1-sol'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "17"
    );
    assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM session_pending_ranges WHERE reason='response-record-conflict')",[],|r|r.get::<_,bool>(0)).unwrap(),
        "conflict candidate state={} log={log} reasons={:?}",checkpoint_state(&db).2,
        db.prepare("SELECT reason FROM session_pending_ranges").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap());
    let saved = checkpoint_state(&db);
    let saved_ledger = ledger(&db);
    drop(db);

    let changed = accepted_source.replacen("\"total_tokens\":15", "\"total_tokens\":16", 1);
    assert_ne!(changed, accepted_source);
    assert_eq!(changed.len(), accepted_source.len());
    fs::write(&source, format!("{changed}{}", counter(40, now + 1))).unwrap();
    let (_, _) = run(root.path(), "offline", "changed-prefix", false);
    let (db, _) = database(root.path());
    assert_eq!(
        checkpoint_state(&db),
        saved,
        "changed accepted bytes cannot advance the old source"
    );
    assert_eq!(ledger(&db), saved_ledger);
    let accepted_database_path = db.path().unwrap().to_owned();
    drop(db);
    if restore_original {
        fs::write(&source, &accepted_source).unwrap();
        let writable = Connection::open(&accepted_database_path).unwrap();
        let before_source: (String,String) = writable.query_row("SELECT admission_json,record_sha256 FROM session_events WHERE json_extract(admission_json,'$.numeric.kind')='legacy-counter' ORDER BY timestamp,event_index LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(writable.execute("UPDATE session_events SET reset_at=NULL,window_seconds=NULL,admission_json=NULL WHERE admission_json=?1",[&before_source.0]).unwrap(),1);
        writable.execute_batch("CREATE TRIGGER reject_pending_qualification BEFORE UPDATE OF admission_json ON session_events WHEN OLD.admission_json IS NULL AND NEW.admission_json IS NOT NULL BEGIN SELECT RAISE(ABORT,'forced pending qualification rollback'); END;").unwrap();
        drop(writable);
        let (success, log) = run(root.path(), "repair-live", "restore-original-event", true);
        assert!(success, "{log}");
        let (db, _) = database(root.path());
        assert_eq!(db.query_row("SELECT count(*) FROM session_events WHERE admission_json IS NOT NULL",[],|r|r.get::<_,i64>(0)).unwrap(),4,"a matching accepted original must recover its numeric/source qualification before period projection");
        let restored: (i64,i64,String,String) = db.query_row("SELECT reset_at,window_seconds,admission_json,record_sha256 FROM session_events WHERE json_extract(admission_json,'$.numeric.kind')='legacy-counter' ORDER BY timestamp,event_index LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        let restored_admission: Value = serde_json::from_str(&restored.2).unwrap();
        let original_admission: Value = serde_json::from_str(&before_source.0).unwrap();
        assert_eq!(restored.0, expected_reset);
        assert_eq!(restored.1, 604800);
        assert_eq!(restored.3, before_source.1);
        assert_eq!(restored_admission["numeric"], original_admission["numeric"]);
        let pending: Value = serde_json::from_slice(
            &fs::read(root.path().join("pending-repair-input.json")).unwrap(),
        )
        .unwrap();
        assert!(
            pending["released_receipts"].as_i64().unwrap()
                >= pending["failure_receipts"].as_i64().unwrap() + 2
        );
        let snapshot_generation = restored_admission["period"]["evidence_generation"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap();
        let fixed_revision:i64=db.query_row("SELECT json_extract(payload_json,'$.input_revision') FROM period_evidence_snapshots WHERE generation=?1",[snapshot_generation],|r|r.get(0)).unwrap();
        assert_eq!(
            fixed_revision,
            pending["fixed_revision"].as_i64().unwrap(),
            "new quota cannot alter a fixed pending evidence snapshot: {log}"
        );
        let newest_revision: i64 = db
            .query_row(
                "SELECT max(revision) FROM official_period_inputs",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(newest_revision > fixed_revision);
        let newest_remaining: f64 = db
            .query_row(
                "SELECT remaining_percent FROM quota_receipts ORDER BY sequence DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let (latest_sequence,latest_at,latest_reset,latest_window):(i64,i64,i64,i64)=db.query_row("SELECT sequence,observed_at,reset_at,window_seconds FROM quota_receipts ORDER BY sequence DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert!(latest_sequence >= pending["released_receipts"].as_i64().unwrap());
        assert_eq!(latest_reset, expected_reset);
        assert_eq!(latest_window, 604800);
        let remaining_values=db.prepare("SELECT remaining_percent FROM quota_receipts WHERE remaining_percent<100 ORDER BY sequence").unwrap().query_map([],|r|r.get::<_,f64>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert!(remaining_values.len() >= 3);
        for (index, remaining) in remaining_values.iter().enumerate() {
            assert_eq!(*remaining, 99.0 - index as f64);
        }
        assert_eq!(remaining_values.last(), Some(&newest_remaining));
        let registry: Value = serde_json::from_slice(
            &fs::read(root.path().join("data/history/account_profile_v1.json")).unwrap(),
        )
        .unwrap();
        let account = registry["accounts"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap();
        let intervals = codex_info_db_reader::ReadIntervals::new(
            account["lifecycle_intervals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|interval| {
                    codex_info_db_reader::ReadInterval::new(
                        interval["start_at"].as_i64(),
                        interval["end_at"].as_i64(),
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap();
        let identity = codex_info_db_reader::StoragePartitionIdentity {
            schema_version: binding["partition"]["schema_version"]
                .as_str()
                .unwrap()
                .into(),
            profile_scope_id: binding["partition"]["profile_scope_id"]
                .as_str()
                .unwrap()
                .into(),
            account_scope_id: binding["partition"]["account_scope_id"]
                .as_str()
                .unwrap()
                .into(),
            storage_epoch: binding["partition"]["storage_epoch"].as_u64().unwrap(),
            partition_id: binding["partition"]["partition_id"]
                .as_str()
                .unwrap()
                .into(),
        };
        let public = codex_info_db_reader::DbReader::open_partitioned_with_intervals(
            &accepted_database_path,
            &identity,
            intervals,
        )
        .unwrap()
        .read_snapshot()
        .unwrap();
        let quota = public
            .details
            .quota
            .as_ref()
            .expect("independent current quota remains public");
        assert_eq!(
            (
                quota.reset_at,
                quota.window_seconds,
                quota.remaining_percent.to_bits()
            ),
            (latest_reset, latest_window, newest_remaining.to_bits())
        );
        assert_eq!(public.details.observed_at, Some(latest_at));
        assert!(public.history_available);
        let minute = latest_at.div_euclid(60) * 60;
        let point = public
            .history_samples_v3
            .iter()
            .find(|point| point.reset_at == latest_reset && point.timestamp == minute)
            .expect("history projects the exact receipt key independently");
        assert_eq!(
            point.remaining_percent.map(f64::to_bits),
            Some(newest_remaining.to_bits())
        );
        // The SQL usage observation remains the fixed pending model batch.
        // Public quota history is owned by receipts, without a second write.
        let original_model_at:i64=db.query_row("SELECT json_extract(snapshot_json,'$.source_timestamp') FROM durable_state WHERE singleton>=2 AND json_extract(snapshot_json,'$.reset_at')=?1 ORDER BY data_generation DESC LIMIT 1",[expected_reset],|r|r.get::<_,Option<i64>>(0).map(|value|value.expect("qualified model preserves original observation time"))).unwrap();
        assert!(original_model_at <= latest_at);

        assert_eq!(
            ledger(&db),
            saved_ledger,
            "qualification recovery preserves every original numeric/source row"
        );
        assert_eq!(
            checkpoint_state(&db).0,
            saved.0,
            "recovery cannot advance the accepted source prefix"
        );
        drop(db);

        // A failed SQL transaction retains its fixed candidate. A later real
        // source append must retire that candidate and allow a fresh worker
        // preparation, without changing any of the original usage facts.
        let writable = Connection::open(&accepted_database_path).unwrap();
        assert_eq!(writable.execute("UPDATE session_events SET reset_at=NULL,window_seconds=NULL,admission_json=NULL WHERE record_sha256=?1",[&before_source.1]).unwrap(),1);
        writable.execute_batch("CREATE TRIGGER reject_pending_qualification BEFORE UPDATE OF admission_json ON session_events WHEN OLD.admission_json IS NULL AND NEW.admission_json IS NOT NULL BEGIN SELECT RAISE(ABORT,'forced pending qualification rollback'); END;").unwrap();
        drop(writable);
        let (success, log) = run(
            root.path(),
            "repair-live",
            "restore-after-source-change",
            true,
        );
        assert!(success, "{log}");
        let retired = log
            .find("historical repair candidate retired")
            .expect("changed source must retire its stale candidate");
        assert!(
            log[retired..].contains("history_prepare requested"),
            "fresh preparation must follow retirement: {log}"
        );
        assert!(
            log[retired..].contains("history_commit finish")
                && log[retired..].contains("success=true"),
            "a fresh candidate must actually commit: {log}"
        );
        let (db, _) = database(root.path());
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM session_events WHERE admission_json IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            4
        );
        assert_eq!(
            ledger(&db),
            saved_ledger,
            "fresh preparation preserves all original numeric/source facts"
        );
        assert_eq!(
            u64::try_from(checkpoint_state(&db).0).unwrap(),
            fs::metadata(&source).unwrap().len(),
            "the new accepted prefix contains the legitimate marker"
        );
        drop(db);
    }
    if legacy_family {
        fs::write(&source, &accepted_source).unwrap();
        let writable = Connection::open(&accepted_database_path).unwrap();
        let old_sha:String=writable.query_row("SELECT record_sha256 FROM session_events WHERE json_extract(admission_json,'$.numeric.kind')='legacy-counter' ORDER BY timestamp,event_index LIMIT 1",[],|r|r.get(0)).unwrap();
        assert_eq!(writable.execute("UPDATE session_events SET model='SOL',reset_at=NULL,window_seconds=NULL,admission_json=NULL WHERE record_sha256=?1",[&old_sha]).unwrap(),1);
        writable.execute_batch("UPDATE session_checkpoints SET source_admission=json_remove(source_admission,'$.account_binding')").unwrap();
        let cp = checkpoint_state(&writable);
        drop(writable);
        let (success, log) = run(root.path(), "offline", "requalify-old-family", false);
        assert!(success, "{log}");
        let (db, _) = database(root.path());
        let restored:(String,Option<i64>,Option<String>)=db.query_row("SELECT total_tokens,reset_at,admission_json FROM session_events WHERE record_sha256=?1 AND model='SOL'",[&old_sha],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(
            (restored.0, restored.1),
            ("5".into(), Some(expected_reset)),
            "the original family key must qualify only through its exact raw model: {log}"
        );
        let proof: Value = serde_json::from_str(restored.2.as_ref().unwrap()).unwrap();
        assert_eq!(proof["numeric"]["exact_model"], "gpt-6.1-sol");
        assert_eq!(
            db.query_row("SELECT count(*) FROM session_events", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert_eq!(
            db.query_row(
                "SELECT sum(CAST(total_tokens AS INTEGER)) FROM session_model_totals",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            17
        );
        assert_eq!(checkpoint_state(&db).0, cp.0);
        drop(db);
        let writable = Connection::open(&accepted_database_path).unwrap();
        let incompatible_sha:String=writable.query_row("SELECT record_sha256 FROM session_events WHERE model='gpt-6.1-sol' AND json_extract(admission_json,'$.numeric.kind')='legacy-counter' ORDER BY timestamp,event_index LIMIT 1",[],|r|r.get(0)).unwrap();
        assert_eq!(writable.execute("UPDATE session_events SET model='TERRA',reset_at=NULL,window_seconds=NULL,admission_json=NULL WHERE record_sha256=?1",[&incompatible_sha]).unwrap(),1);
        let original_ledger = ledger(&writable);
        drop(writable);
        let (success, log) = run(root.path(), "offline", "refuse-other-family", false);
        assert!(success, "{log}");
        let (db, _) = database(root.path());
        assert_eq!(ledger(&db), original_ledger);
        assert!(db.query_row("SELECT admission_json IS NULL AND reset_at IS NULL FROM session_events WHERE record_sha256=?1 AND model='TERRA'",[&incompatible_sha],|r|r.get::<_,bool>(0)).unwrap());
        assert_eq!(
            db.query_row("SELECT count(*) FROM source_prefix_repairs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(db);
    }
    if retire_counter {
        fs::write(&source, &accepted_source).unwrap();
        let root_at = chrono::DateTime::<chrono::Utc>::from_timestamp(now, 0)
            .unwrap()
            .to_rfc3339();
        let counter_at = chrono::DateTime::<chrono::Utc>::from_timestamp(now + 1, 0)
            .unwrap()
            .to_rfc3339();
        let usage = json!({"total_tokens":11,"input_tokens":9,"cached_input_tokens":1,"cache_write_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":1});
        accepted_source.push_str(&format!("{}\n{}\n",
            json!({"type":"token_usage_record","timestamp":root_at,"payload":{"thread_id":"binding-thread","session_id":"binding-session","root_turn_id":"root-turn","turn_id":"turn","response_id":"retired-response","usage":usage,"turn_token_usage":usage,"thread_token_usage":usage}}),
            json!({"type":"event_msg","timestamp":counter_at,"payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":68,"input_tokens":66,"cached_input_tokens":1,"cache_write_input_tokens":0,"output_tokens":2},"last_token_usage":usage}}})
        ));
        fs::write(&source, &accepted_source).unwrap();
        let (success, log) = run(root.path(), "offline", "new-root-before-old-counter", false);
        assert!(success, "{log}");
        let writable = Connection::open(&accepted_database_path).unwrap();
        let original_sha:String=writable.query_row("SELECT record_sha256 FROM session_events WHERE json_extract(admission_json,'$.numeric.candidate.record.ids.response_id')='retired-response'",[],|r|r.get(0)).unwrap();
        // Simulate the old counter-only representation in the same accepted
        // source range. Its numbers describe cumulative delta 57 -> 68,
        // whereas the immutable root records I9/C1/O2 and the earlier time.
        assert_eq!(writable.execute("UPDATE session_events SET model='SOL',event_index=0,timestamp=?1,input_tokens='11',cached_input_tokens='0',output_tokens='0',cache_write_input_tokens=NULL,reset_at=NULL,window_seconds=NULL,admission_json=NULL WHERE json_extract(admission_json,'$.numeric.candidate.record.ids.response_id')='retired-response'",[now+1]).unwrap(),1);
        let prefix_before = checkpoint_state(&writable);
        let ledger_before = ledger(&writable);
        let snapshots_before: i64 = writable
            .query_row("SELECT count(*) FROM period_evidence_snapshots", [], |r| {
                r.get(0)
            })
            .unwrap();
        writable.execute_batch("CREATE TRIGGER reject_counter_retirement BEFORE DELETE ON session_events WHEN OLD.total_tokens='11' AND OLD.admission_json IS NULL BEGIN SELECT RAISE(ABORT,'forced original retirement rollback'); END;").unwrap();
        drop(writable);
        let (_, failed_log) = run(root.path(), "offline", "rollback-original-counter", false);
        assert!(
            failed_log.contains("forced original retirement rollback"),
            "{failed_log}"
        );
        let writable = Connection::open(&accepted_database_path).unwrap();
        assert_eq!(checkpoint_state(&writable), prefix_before);
        assert_eq!(ledger(&writable), ledger_before);
        assert_eq!(
            writable
                .query_row("SELECT count(*) FROM source_prefix_repairs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            writable
                .query_row("SELECT count(*) FROM period_evidence_snapshots", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap(),
            snapshots_before
        );
        writable
            .execute_batch("DROP TRIGGER reject_counter_retirement")
            .unwrap();
        drop(writable);
        let (success, log) = run(root.path(), "offline", "replace-original-counter", false);
        assert!(success, "{log}");
        let (db, _) = database(root.path());
        let restored:(i64,String,String,String,String,String,Option<i64>,String)=db.query_row("SELECT timestamp,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens,reset_at,record_sha256 FROM session_events WHERE json_extract(admission_json,'$.numeric.candidate.record.ids.response_id')='retired-response'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional().unwrap().unwrap_or((0,String::new(),String::new(),String::new(),String::new(),String::new(),None,String::new()));
        assert_eq!(
            restored,
            (
                now,
                "11".into(),
                "9".into(),
                "1".into(),
                "2".into(),
                "0".into(),
                Some(expected_reset),
                original_sha
            ),
            "a proven old counter is retired in favor of the original response once: {log}"
        );
        assert_eq!(db.query_row("SELECT model FROM session_events WHERE json_extract(admission_json,'$.numeric.candidate.record.ids.response_id')='retired-response'",[],|r|r.get::<_,String>(0)).unwrap(),"SOL","retirement preserves an existing family key");
        assert_eq!(
            db.query_row("SELECT count(*) FROM session_events", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert_eq!(
            db.query_row(
                "SELECT CAST(sum(CAST(total_tokens AS INTEGER)) AS TEXT) FROM session_model_totals",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "28"
        );
        assert_eq!(checkpoint_state(&db).0, prefix_before.0);
        assert_eq!(checkpoint_state(&db).1, prefix_before.1);
        let (provenance, id, sha): (String, String, String) = db
            .query_row(
                "SELECT payload_json,repair_id,payload_sha256 FROM source_prefix_repairs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(id, sha);
        assert_eq!(id, format!("{:x}", Sha256::digest(provenance.as_bytes())));
        let proof: Value = serde_json::from_str(&provenance).unwrap();
        let retired = &proof["retirements"][0];
        assert_eq!(retired["original"]["timestamp"], now + 1);
        assert_eq!(retired["original"]["source"]["event_index"], 0);
        assert_eq!(
            retired["original"]["tokens"],
            json!({"total":11,"input":11,"cached_input":0,"cache_write_input":null,"output":0})
        );
        assert_eq!(retired["replacement"]["timestamp"], now);
        assert_eq!(
            retired["replacement"]["admission"]["numeric"]["candidate"]["counter_association"]
                ["state"],
            "matched"
        );
        assert!(PathBuf::from(format!("{accepted_database_path}.bak.1")).is_file());
        let before_replay = ledger(&db);
        drop(db);
        let (success, log) = run(root.path(), "offline", "replay-retired-counter", false);
        assert!(success, "{log}");
        let (db, _) = database(root.path());
        assert_eq!(ledger(&db), before_replay);
        assert_eq!(
            db.query_row("SELECT payload_json FROM source_prefix_repairs", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
            provenance
        );
        drop(db);
        // A differing reasoning component in the FIRST counter is enough
        // to refuse association. A later all-six match cannot rescue it.
        let usage = json!({"total_tokens":13,"input_tokens":11,"cached_input_tokens":1,"cache_write_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":1});
        let unmatched = json!({"total_tokens":13,"input_tokens":11,"cached_input_tokens":1,"cache_write_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":0});
        accepted_source.push_str(&format!("{}\n{}\n{}\n",
            json!({"type":"token_usage_record","timestamp":root_at,"payload":{"thread_id":"binding-thread","session_id":"binding-session","root_turn_id":"root-turn","turn_id":"turn","response_id":"unmatched-response","usage":usage,"turn_token_usage":usage,"thread_token_usage":usage}}),
            json!({"type":"event_msg","timestamp":counter_at,"payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":81,"input_tokens":79,"cached_input_tokens":1,"cache_write_input_tokens":0,"output_tokens":2},"last_token_usage":unmatched}}}),
            json!({"type":"event_msg","timestamp":counter_at,"payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":94,"input_tokens":92,"cached_input_tokens":1,"cache_write_input_tokens":0,"output_tokens":2},"last_token_usage":usage}}})
        ));
        fs::write(&source, &accepted_source).unwrap();
        let (success, log) = run(root.path(), "offline", "unmatched-root", false);
        assert!(success, "{log}");
        let writable = Connection::open(&accepted_database_path).unwrap();
        assert_eq!(writable.execute("UPDATE session_events SET event_index=0,timestamp=?1,input_tokens='13',cached_input_tokens='0',output_tokens='0',cache_write_input_tokens=NULL,reset_at=NULL,window_seconds=NULL,admission_json=NULL WHERE json_extract(admission_json,'$.numeric.candidate.record.ids.response_id')='unmatched-response'",[now+1]).unwrap(),1);
        let unmatched_prefix = checkpoint_state(&writable);
        let unmatched_ledger = ledger(&writable);
        drop(writable);
        let (success, log) = run(root.path(), "offline", "do-not-search-later-counter", false);
        assert!(success, "{log}");
        let (db, _) = database(root.path());
        assert_eq!(ledger(&db), unmatched_ledger);
        assert_eq!(checkpoint_state(&db).0, unmatched_prefix.0);
        assert_eq!(db.query_row("SELECT count(*) FROM session_events WHERE admission_json IS NULL AND total_tokens='13' AND input_tokens='13' AND cache_write_input_tokens IS NULL",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(
            db.query_row("SELECT count(*) FROM source_prefix_repairs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(db);
    }
    if let Some(export) = env::var_os("CODEX_INFO_TEST_EXPORT_BINDING_FIXTURE") {
        // A fake-account process fixture is retained only for the explicit
        // recorder-owner -> REST integration probe. Restore its accepted
        // original before handing it to that probe.
        fs::write(&source, &accepted_source).unwrap();
        let destination = PathBuf::from(export);
        assert!(!destination.exists(), "fixture export destination exists");
        let original_root = root.keep();
        fs::write(&destination, original_root.to_str().unwrap()).unwrap();
        eprintln!(
            "issue604 binding fixture retained at {} (locator {})",
            original_root.display(),
            destination.display()
        );
    }
}

#[test]
fn issue604_collector_reuses_only_owned_accepted_prefix_for_historical_binding() {
    use codex_info_account_locator::{
        admit_partition, mark_partition_initialized, reserve_partition_at, AccountConfirmation,
    };
    use codex_info_db_writer::StoragePartitionIdentity;
    use codex_info_recorder::{Recorder, RecorderConfig};
    use sha2::{Digest, Sha256};

    for owned in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let data = root.path().join("data");
        private_directory(&home);
        private_directory(&home.join("sessions"));
        private_directory(&data);
        let auth = home.join("auth.json");
        fs::write(
            &auth,
            br#"{"tokens":{"account_id":"historical-binding-fixture"}}"#,
        )
        .unwrap();
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o600)).unwrap();
        let now = chrono::Utc::now().timestamp();
        let observed = now - 30;
        let start = if owned { now - 3600 } else { now };
        let reserved = reserve_partition_at(&home, &data, start).unwrap();
        let partition = admit_partition(
            &home,
            &data,
            &reserved,
            AccountConfirmation::InitialRpc { began_at: start },
            || Some(start),
        )
        .unwrap();
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: partition.profile_scope_id.clone(),
            account_scope_id: partition.account_scope_id.clone(),
            storage_epoch: partition.storage_epoch,
            partition_id: partition.partition_id.clone(),
        };
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(observed, 0)
            .unwrap()
            .to_rfc3339();
        let counter = |total: u64, at: i64| {
            let at = chrono::DateTime::<chrono::Utc>::from_timestamp(at, 0)
                .unwrap()
                .to_rfc3339();
            format!(
                "{}\n",
                json!({"type":"event_msg","timestamp":at,
                "payload":{"type":"token_count","info":{"total_token_usage":{
                    "total_tokens":total,"input_tokens":total,"cached_input_tokens":0,"output_tokens":0
                }}}})
            )
        };
        let metadata = format!(
            "{}\n{}\n",
            json!({"type":"session_meta","timestamp":timestamp,
                "payload":{"id":"historical-thread","session_id":"historical-session"}}),
            json!({"type":"turn_context","timestamp":timestamp,
                "payload":{"model":"gpt-6.1-sol","turn_id":"turn","root_turn_id":"root-turn"}})
        );
        let first = counter(15, observed);
        let original = format!("{metadata}{first}");
        let source = home.join("sessions/one.jsonl");
        fs::write(&source, &original).unwrap();
        // This fixture represents an original accepted CP, not a new runtime
        // authentication claim. The real owner below must revalidate it.
        let mut seed = Recorder::open_partitioned(
            RecorderConfig {
                sessions_root: home.join("sessions"),
                chunk_bytes: 65536,
            },
            &partition.database_path,
            &identity,
        )
        .unwrap();
        seed.run_cycle().unwrap().unwrap();
        let before = seed.state().unwrap().checkpoints.remove(0);
        assert_eq!(before.committed_offset, original.len() as u64);
        assert!(before
            .source_admission
            .as_ref()
            .unwrap()
            .account_binding
            .is_none());
        drop(seed);
        mark_partition_initialized(&data, &partition).unwrap();
        let appended_at = chrono::Utc::now().timestamp();
        fs::write(
            &source,
            format!(
                "{original}{}{}",
                counter(20, appended_at),
                counter(25, appended_at)
            ),
        )
        .unwrap();
        let server = root.path().join("offline.py");
        fs::write(&server, r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    req = json.loads(line)
    if 'id' not in req:
        continue
    response = {'id':req['id'],'result':{}} if req.get('method') == 'initialize' else {'id':req['id'],'error':{'code':-32000,'message':'fixture RPC unavailable'}}
    print(json.dumps(response), flush=True)
"#).unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o700)).unwrap();
        let log = root.path().join("historical.log");
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_codex_info_recorder"))
                .args(["--once", "--interval-secs", "1"])
                .env("CODEX_HOME", &home)
                .env("CODEX_INFO_DATA_DIR", &data)
                .env("CODEX_INFO_CODEX_BIN", &server)
                .env_remove("CODEX_INFO_RECORDER_DB")
                .env_remove("CODEX_INFO_ACCOUNT_DB")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(File::create(&log).unwrap())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(12);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "historical binding fixture did not terminate"
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert!(
            status.success(),
            "saved canonical account could not restart\n{}",
            fs::read_to_string(log).unwrap()
        );
        let db = Connection::open_with_flags(
            &partition.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        db.execute_batch("PRAGMA query_only=ON").unwrap();
        let (offset,generation,state): (i64,String,String) = db.query_row(
            "SELECT committed_offset,prefix_generation,source_admission FROM session_checkpoints",
            [],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).unwrap();
        let state: Value = serde_json::from_str(&state).unwrap();
        let amounts: Vec<String> = db
            .prepare("SELECT total_tokens FROM session_events ORDER BY timestamp,event_index")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        if owned {
            let binding = &state["account_binding"];
            assert_eq!(
                binding["proof"]["kind"], "accepted-prefix",
                "an original owned point needs its saved qualification, not current RPC"
            );
            assert_eq!(binding["prefix_end"], original.len() as u64);
            assert_eq!(
                binding["prefix_generation"],
                format!("{:032x}", before.prefix_generation)
            );
            assert_eq!(
                binding["proof"]["anchor"],
                json!({
                    "start":metadata.len(),"end":original.len(),
                    "sha256":format!("{:x}",Sha256::digest(first.as_bytes())),"timestamp":observed
                })
            );
            assert_eq!(
                amounts,
                ["5", "5"],
                "the accepted owned 15 remains a comparison baseline"
            );
            assert_eq!(offset, fs::metadata(&source).unwrap().len() as i64);
            assert_eq!(generation, format!("{:032x}", before.prefix_generation));
        } else {
            assert_eq!(
                offset, before.committed_offset as i64,
                "an unowned legacy point plus failed initial RPC cannot advance source ownership"
            );
            let original_state: Value =
                serde_json::from_str(&before.source_admission.unwrap().to_storage().unwrap())
                    .unwrap();
            assert_eq!(state, original_state);
            assert!(
                amounts.is_empty(),
                "current authentication cannot assign the old point"
            );
        }
    }
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
    let expected_reset = fixture.expected["rateLimits"]["primary"]["resetsAt"]
        .as_i64()
        .unwrap();
    assert_eq!(period, (expected_reset, expected_reset, 604_800), "{log}");
    let quota: (i64, f64) = db
        .query_row(
            "SELECT reset_at, remaining_percent FROM usage_history ORDER BY timestamp DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(quota, (expected_reset, 99.0));
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
        "the absent app-server is a real transport failure, independent of retained historical input"
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
fn quota_resident_accepts_fresh_quota_while_retaining_old_unresolved_input() {
    let mut fixture = QuotaJournalFixture::start(false, false, 3);
    fixture.wait_for_calls(4);
    fixture.child.0.kill().unwrap();
    fixture.child.0.wait().unwrap();
    let database = replay_partition(&fixture);
    let raw = Connection::open(&fixture.database).unwrap();
    let first_id: String = raw
        .query_row(
            "SELECT receipt_id FROM quota_observations ORDER BY sequence LIMIT 1",
            [],
            |r| r.get(0),
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
            "SELECT COALESCE((SELECT degraded OR input_degraded FROM quota_acquisition_state WHERE singleton=1),0)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        degraded, 0,
        "fresh accepted quota recovers current acquisition independently of old unresolved input"
    );
    let accepted = accepted_quota_receipts(&database);
    assert_eq!(accepted.len(), 4);
    assert!(accepted.iter().all(|(id, _, _)| id != &first_id));
    let latest_raw: (String, i64) = raw
        .query_row(
            "SELECT receipt_id,sequence FROM quota_observations ORDER BY sequence DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        accepted.last().unwrap(),
        &(latest_raw.0, latest_raw.1, 99.0)
    );
    let retained: String = raw
        .query_row(
            "SELECT payload FROM quota_observations WHERE receipt_id=?1",
            [&first_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        retained, "{}",
        "recovery does not rewrite or remove unresolved original input"
    );
    assert_eq!(
        fixture.calls(),
        7,
        "recovery does not require another API response"
    );
    let raw_count: i64 = raw
        .query_row("SELECT count(*) FROM quota_observations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw_count, 5);
    resident.0.kill().unwrap();
    resident.0.wait().unwrap();
}

#[test]
fn quota_resident_separates_saved_uninterpretable_input_from_transport_failure() {
    let fixture = QuotaJournalFixture::start_process(true, false, 3, Some(1));
    let root = fixture._root.path();
    let deadline = Instant::now() + Duration::from_secs(12);
    let database = loop {
        if let Ok(partition) = codex_info_account_locator::locate_existing_partition(
            root.join("home"),
            root.join("data"),
        ) {
            let db = Connection::open(&partition.database_path).unwrap();
            if db
                .query_row(
                    "SELECT input_degraded FROM quota_acquisition_state WHERE singleton=1",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
                == Some(1)
            {
                break partition.database_path;
            }
        }
        assert!(
            Instant::now() < deadline,
            "resident did not retain uninterpretable input: {}",
            fs::read_to_string(root.join("owner.log")).unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    };
    let db = Connection::open(&database).unwrap();
    let health = || {
        db.query_row(
            "SELECT degraded,input_degraded FROM quota_acquisition_state WHERE singleton=1",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .unwrap()
    };
    assert_eq!(
        health(),
        (0, 1),
        "a verified and saved response is not a transport failure"
    );
    assert!(accepted_quota_receipts(&database).is_empty());
    let raw = Connection::open(&fixture.database).unwrap();
    let original: (String,String,i64) = raw.query_row("SELECT receipt_id,payload,account_verified FROM quota_observations ORDER BY sequence LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(original.2, 1);
    let mut valid = fixture.expected.clone();
    valid["rateLimits"]["primary"]["usedPercent"] = json!(1);
    fs::write(root.join("quota-next.json"), valid.to_string()).unwrap();
    fs::rename(root.join("quota-next.json"), root.join("quota.json")).unwrap();
    loop {
        let accepted = accepted_quota_receipts(&database);
        if health() == (0, 0) && accepted.last().is_some_and(|r| r.2 == 99.0) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "resident did not recover from a later valid saved response: {}",
            fs::read_to_string(root.join("owner.log")).unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    }
    let retained: (String, i64) = raw
        .query_row(
            "SELECT payload,account_verified FROM quota_observations WHERE receipt_id=?1",
            [&original.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(retained, (original.1, 1));
    let rows: Vec<(i64, String)> = raw
        .prepare("SELECT sequence,payload FROM quota_observations ORDER BY sequence")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(rows.windows(2).all(|pair| pair[0].0 < pair[1].0));
    for (index, (sequence, payload)) in rows.iter().enumerate() {
        assert!(*sequence > 0);
        assert_eq!(
            serde_json::from_str::<Value>(payload).unwrap()["fixtureSequence"],
            index
        );
    }
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
        ActiveThreadPollResult::Failed(_) | ActiveThreadPollResult::CheckpointMismatch { .. } => {
            "failed"
        }
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
        last_model_context_kind: None,
        last_model_interval: None,
        source_admission: None,
        last_task_running: Some(true),
        context_usage_tokens: None,
        context_window_tokens: None,
        previous_total: Some(42),
        previous_input: Some(40),
        previous_cached_input: Some(0),
        previous_output: Some(2),
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
        ActiveThreadPollResult::CheckpointMismatch { .. } => {
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

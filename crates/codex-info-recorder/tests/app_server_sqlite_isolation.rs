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
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

static ENVIRONMENT_LOCK: Mutex<()> = Mutex::new(());

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

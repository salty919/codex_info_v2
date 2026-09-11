//! Independent, local Session recorder.
//!
//! This crate is deliberately below the REST and root application layers.
//! Session JSONL is an append-only source; the recorder admits exact complete
//! byte ranges to the partitioned UsageStore and only advances a durable
//! checkpoint after the writer transaction has committed and been read back.

#![deny(unsafe_code)]

use chrono::{DateTime, Months, Utc};
use codex_info::{security, thread_contract};
use codex_info_db_writer::{
    canonical_reset_period, classify_quota_transition, finalize_session_timeline_recovery,
    ActiveThreadRecord, ActiveThreadSnapshot, QuotaTransition, RecordedSessionSource,
    SessionCheckpoint, SessionCollectionCommit, SessionCollectionState, SessionEvent,
    SessionModelTotal, SessionPendingRange, SessionRange, SessionTimelineRecovery,
    SessionTimelineRecoveryPoint, StoragePartitionIdentity, UsageHistoryObservation,
    UsageHistorySample, UsageStore, UsageStoreError,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

pub const PARSER_VERSION: &str = "codex-info-session-recorder-v3";
pub const DEFAULT_CHUNK_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_INTERVAL_SECS: u64 = 60;
const UNATTRIBUTED_MODEL: &str = "UNATTRIBUTED";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
// These are the existing app-server security contract values from the root
// collector.  The independent lane must apply the same bounded JSON-RPC
// framing and response timeout.
const APP_SERVER_MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
const APP_SERVER_MAX_IGNORED_MESSAGES: usize = 1_024;
const APP_SERVER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PROC_PROCESS_ENTRIES: usize = 65_536;
const MAX_CODEX_PROCESS_FDS: usize = 16_384;
const MAX_OPEN_SESSION_FILES: usize = 1_024;
const MAX_ACTIVE_THREAD_ROWS: usize = 256;
const MAX_THREAD_CHECKPOINTS: usize = 65_536;
const MAX_THREAD_ID_SCALARS: usize = 128;

#[derive(Debug)]
pub enum RecorderError {
    Io(io::Error),
    Writer(UsageStoreError),
    Invalid(String),
}

impl fmt::Display for RecorderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "recorder I/O error: {error}"),
            Self::Writer(error) => write!(formatter, "recorder writer error: {error}"),
            Self::Invalid(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for RecorderError {}

impl From<io::Error> for RecorderError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<UsageStoreError> for RecorderError {
    fn from(error: UsageStoreError) -> Self {
        Self::Writer(error)
    }
}

#[derive(Clone, Debug)]
pub struct RecorderConfig {
    pub sessions_root: PathBuf,
    pub chunk_bytes: u64,
}

impl RecorderConfig {
    pub fn validate(&self) -> Result<(), RecorderError> {
        if !self.sessions_root.is_absolute() {
            return Err(RecorderError::Invalid(
                "sessions root must be absolute".to_owned(),
            ));
        }
        if self.chunk_bytes == 0 {
            return Err(RecorderError::Invalid(
                "recorder chunk size must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CycleReport {
    pub generation: u64,
    pub accepted_ranges: usize,
    pub pending_ranges: usize,
    pub sources_seen: usize,
}

/// A successful quota observation from the independent app-server lane.
/// `remaining_percent` is optional for plans which expose no bounded quota;
/// such observations never become period authority without a valid value.
#[derive(Clone, Debug, PartialEq)]
pub struct QuotaSnapshot {
    pub observed_at: i64,
    pub reset_at: i64,
    pub window_seconds: i64,
    pub remaining_percent: Option<f64>,
}

type QuotaPollResult = Result<QuotaSnapshot, String>;

/// Health transitions from the isolated quota lane. The payload is retained
/// only for a successful observation; failures are intentionally categorical
/// so a malformed app-server response cannot leak into recorder diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub enum QuotaPollEvent {
    Ready(QuotaSnapshot),
    Failed,
}

/// Polls the Codex app-server in a lane independent of Session collection.
/// A failed poll is reported as degraded while the last good quota remains
/// available to the next recorder cycle.
pub struct QuotaPoller {
    receiver: Receiver<QuotaPollResult>,
    _worker: JoinHandle<()>,
    latest: Option<QuotaSnapshot>,
    events: Vec<QuotaPollEvent>,
}

impl QuotaPoller {
    pub fn start() -> Self {
        Self::start_with_interval(DEFAULT_INTERVAL_SECS)
    }

    /// Start the quota lane with the caller's polling cadence. The worker
    /// never waits on the Session recorder; a full result queue only drops a
    /// stale observation and does not stop the lane.
    pub fn start_with_interval(interval_secs: u64) -> Self {
        let (sender, receiver) = mpsc::sync_channel(2);
        let interval_secs = interval_secs.max(1);
        let worker = thread::spawn(move || loop {
            let result = fetch_quota_snapshot();
            match sender.try_send(result) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => break,
            }
            thread::sleep(Duration::from_secs(interval_secs));
        });
        Self {
            receiver,
            _worker: worker,
            latest: None,
            events: Vec::new(),
        }
    }

    pub fn latest(&mut self) -> Option<QuotaSnapshot> {
        while let Ok(result) = self.receiver.try_recv() {
            match result {
                Ok(snapshot) => {
                    self.latest = Some(snapshot.clone());
                    self.events.push(QuotaPollEvent::Ready(snapshot));
                }
                Err(error) => {
                    eprintln!("recorder degraded: quota poll failed: {error}");
                    self.events.push(QuotaPollEvent::Failed);
                }
            }
        }
        self.latest.clone()
    }

    /// Drain health transitions without waiting for a poll result. The main
    /// loop can forward these to the DB health API while continuing Session
    /// collection when the app-server lane is degraded.
    pub fn take_events(&mut self) -> Vec<QuotaPollEvent> {
        std::mem::take(&mut self.events)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActiveThreadPollResult {
    Snapshot(ActiveThreadSnapshot),
    Empty,
    Failed(String),
}

enum ActiveThreadPollCommand {
    Probe { checkpoints: Vec<SessionCheckpoint> },
}

/// Non-blocking bridge between the recorder cycle and live active-thread
/// discovery. The worker owns the short-lived app-server process and all
/// `/proc`/Session reads; the token recorder only submits checkpoints and
/// drains completed results with `try_*` operations.
pub struct ThreadPoller {
    sender: SyncSender<ActiveThreadPollCommand>,
    receiver: Receiver<ActiveThreadPollResult>,
    _worker: JoinHandle<()>,
}

impl ThreadPoller {
    pub fn start(sessions_root: PathBuf) -> Self {
        let (sender, commands) = mpsc::sync_channel(1);
        let (results, receiver) = mpsc::sync_channel(2);
        let worker = thread::spawn(move || {
            while let Ok(ActiveThreadPollCommand::Probe { checkpoints }) = commands.recv() {
                let result = collect_active_thread_snapshot(&sessions_root, &checkpoints);
                // Never let an app-server result block or back up the Session
                // loop. A later cycle will request a fresh snapshot.
                let _ = results.try_send(result);
            }
        });
        Self {
            sender,
            receiver,
            _worker: worker,
        }
    }

    /// Queue a probe only when the worker is idle. `false` means the caller
    /// should continue its Session cycle and retry on the next cycle.
    pub fn submit(&self, checkpoints: &[SessionCheckpoint]) -> bool {
        if checkpoints.len() > MAX_THREAD_CHECKPOINTS {
            return false;
        }
        self.sender
            .try_send(ActiveThreadPollCommand::Probe {
                checkpoints: checkpoints.to_vec(),
            })
            .is_ok()
    }

    pub fn drain(&self) -> Vec<ActiveThreadPollResult> {
        self.receiver.try_iter().collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AppServerAccount {
    email: String,
    plan_type: String,
}

fn fetch_quota_snapshot() -> QuotaPollResult {
    let executable = resolve_codex_executable()?;
    let mut child = Command::new(executable)
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Codex app-server could not be started".to_owned())?;
    let Some(mut input) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Codex app-server stdin is unavailable".to_owned());
    };
    let Some(stdout) = child.stdout.take() else {
        drop(input);
        let _ = child.kill();
        let _ = child.wait();
        return Err("Codex app-server stdout is unavailable".to_owned());
    };
    let output = app_server_reader(stdout);
    let result = (|| {
        request_app_server(
            &mut input,
            &output,
            1,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "codex-info-recorder",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {"experimentalApi": true}
            }),
        )?;
        let account_value = request_app_server(&mut input, &output, 2, "account/read", json!({}))?;
        let account = decode_app_server_account(&account_value)?;
        let rate_limits = request_app_server(
            &mut input,
            &output,
            3,
            "account/rateLimits/read",
            Value::Null,
        )?;
        let snapshot = parse_app_server_quota(&rate_limits, &account.plan_type)?;
        let account_recheck =
            request_app_server(&mut input, &output, 4, "account/read", json!({}))?;
        if decode_app_server_account(&account_recheck)? != account {
            return Err("Codex account identity changed during quota read".to_owned());
        }
        Ok(snapshot)
    })();
    drop(input);
    // The recorder owns this short-lived app-server connection. Reap it on
    // every path so a quota outage cannot leak one process per cycle.
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn resolve_codex_executable() -> Result<PathBuf, String> {
    let candidates = if let Some(path) = std::env::var_os("CODEX_INFO_CODEX_BIN") {
        if !Path::new(&path).is_absolute() {
            return Err("CODEX_INFO_CODEX_BIN must be an absolute path".to_owned());
        }
        vec![PathBuf::from(path)]
    } else {
        std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join("codex"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    for candidate in candidates {
        let canonical = match fs::canonicalize(candidate) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let metadata = match fs::symlink_metadata(&canonical) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        if metadata.mode() & 0o111 == 0 {
            continue;
        }
        return Ok(canonical);
    }
    Err("Codex app-server executable is unavailable".to_owned())
}

fn app_server_reader(stdout: ChildStdout) -> Receiver<Result<String, String>> {
    let (sender, receiver) = mpsc::sync_channel(16);
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_bounded_rpc_line(&mut reader) {
                Ok(Some(line)) => {
                    if sender.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = sender.send(Err(error));
                    break;
                }
            }
        }
    });
    receiver
}

fn read_bounded_rpc_line<R: BufRead>(reader: &mut R) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|_| "Codex app-server response could not be read".to_owned())?;
        if buffer.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Err("Codex app-server response ended with a partial line".to_owned());
        }
        if let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
            if bytes.len().saturating_add(position) > APP_SERVER_MAX_LINE_BYTES {
                reader.consume(position + 1);
                return Err("Codex app-server response exceeded its line limit".to_owned());
            }
            bytes.extend_from_slice(&buffer[..position]);
            reader.consume(position + 1);
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| "Codex app-server response was not UTF-8".to_owned());
        }
        if bytes.len().saturating_add(buffer.len()) > APP_SERVER_MAX_LINE_BYTES {
            let length = buffer.len();
            reader.consume(length);
            return Err("Codex app-server response exceeded its line limit".to_owned());
        }
        bytes.extend_from_slice(buffer);
        let length = buffer.len();
        reader.consume(length);
    }
}

fn request_app_server(
    input: &mut impl Write,
    output: &Receiver<Result<String, String>>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    request_app_server_before_deadline(
        input,
        output,
        id,
        method,
        params,
        Instant::now() + APP_SERVER_RESPONSE_TIMEOUT,
    )
}

fn request_app_server_before_deadline(
    input: &mut impl Write,
    output: &Receiver<Result<String, String>>,
    id: u64,
    method: &str,
    params: Value,
    deadline: Instant,
) -> Result<Value, String> {
    let message = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    writeln!(input, "{message}")
        .map_err(|_| "Codex app-server request could not be sent".to_owned())?;
    input
        .flush()
        .map_err(|_| "Codex app-server request could not be sent".to_owned())?;
    let mut ignored = 0usize;
    loop {
        let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
            return Err("Codex app-server response timed out".to_owned());
        };
        let line = match output.recv_timeout(wait) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => {
                return Err("Codex app-server response timed out".to_owned())
            }
            Err(RecvTimeoutError::Disconnected) => return Err("Codex app-server exited".to_owned()),
        };
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                ignored = ignored.saturating_add(1);
                if ignored > APP_SERVER_MAX_IGNORED_MESSAGES {
                    return Err("Codex app-server sent too many ignored messages".to_owned());
                }
                continue;
            }
        };
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            ignored = ignored.saturating_add(1);
            if ignored > APP_SERVER_MAX_IGNORED_MESSAGES {
                return Err("Codex app-server sent too many ignored messages".to_owned());
            }
            continue;
        }
        if value.get("error").is_some() {
            return Err("Codex app-server rejected the request".to_owned());
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

fn collect_active_thread_snapshot(
    sessions_root: &Path,
    checkpoints: &[SessionCheckpoint],
) -> ActiveThreadPollResult {
    let (sessions_root, active_paths) = match active_thread_paths(sessions_root) {
        Ok(value) => value,
        Err(error) => return ActiveThreadPollResult::Failed(error),
    };
    if active_paths.is_empty() {
        return ActiveThreadPollResult::Empty;
    }

    let root_metadata = match fs::metadata(&sessions_root) {
        Ok(metadata) => metadata,
        Err(_) => return ActiveThreadPollResult::Failed("session root stat failed".to_owned()),
    };
    let root_identity = root_identity(&sessions_root, &root_metadata);
    let mut candidates = Vec::with_capacity(active_paths.len());
    for path in active_paths {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                return ActiveThreadPollResult::Failed("active session disappeared".to_owned())
            }
        };
        let relative_path = match path.strip_prefix(&sessions_root) {
            Ok(relative) if !relative.as_os_str().is_empty() => {
                relative.to_string_lossy().replace('\\', "/")
            }
            _ => {
                return ActiveThreadPollResult::Failed(
                    "active session path escaped root".to_owned(),
                )
            }
        };
        let Some(checkpoint) = checkpoints.iter().find(|checkpoint| {
            checkpoint.root_identity == root_identity
                && checkpoint.relative_path == relative_path
                && checkpoint.file_device == file_device(&metadata)
                && checkpoint.file_inode == file_inode(&metadata)
        }) else {
            // A live Session without a read-back checkpoint is not a trusted
            // rollout state. Treat it as degraded rather than publishing a
            // false empty snapshot.
            return ActiveThreadPollResult::Failed("active session checkpoint mismatch".to_owned());
        };
        let thread_id = match read_session_meta_id(&sessions_root, &path, &metadata) {
            Ok(id) => id,
            Err(error) => return ActiveThreadPollResult::Failed(error),
        };
        candidates.push((path, metadata, thread_id, checkpoint.clone()));
    }

    let deadline = Instant::now() + APP_SERVER_RESPONSE_TIMEOUT;
    let executable = match resolve_codex_executable() {
        Ok(path) => path,
        Err(_) => {
            return ActiveThreadPollResult::Failed(
                "Codex app-server executable unavailable".to_owned(),
            )
        }
    };
    let mut child = match Command::new(executable)
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            return ActiveThreadPollResult::Failed("Codex app-server start failed".to_owned())
        }
    };
    let Some(mut input) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return ActiveThreadPollResult::Failed("Codex app-server stdin unavailable".to_owned());
    };
    let Some(stdout) = child.stdout.take() else {
        drop(input);
        let _ = child.kill();
        let _ = child.wait();
        return ActiveThreadPollResult::Failed("Codex app-server stdout unavailable".to_owned());
    };
    let output = app_server_reader(stdout);
    let result = (|| {
        request_app_server_before_deadline(
            &mut input,
            &output,
            1,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "codex-info-recorder-thread-poller",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {"experimentalApi": true}
            }),
            deadline,
        )?;
        let mut next_request_id = 2_u64;
        let mut seen_ids = BTreeSet::new();
        let mut rollouts = BTreeMap::new();
        let mut thread_items = Vec::with_capacity(candidates.len());
        for (path, metadata, thread_id, checkpoint) in &candidates {
            let rollout = read_active_rollout(&sessions_root, path, metadata, checkpoint)?;
            let request_id = next_request_id;
            next_request_id = next_request_id
                .checked_add(1)
                .ok_or_else(|| "thread request id exhausted".to_owned())?;
            let result = request_app_server_before_deadline(
                &mut input,
                &output,
                request_id,
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
                deadline,
            )?;
            let result_object = result
                .as_object()
                .filter(|object| object.len() == 1)
                .ok_or_else(|| "thread/read response envelope rejected".to_owned())?;
            let thread_item = result_object
                .get("thread")
                .ok_or_else(|| "thread/read response missing thread".to_owned())?;
            let candidate = thread_contract::validate_thread_item(thread_item)
                .map_err(|_| "thread/read response rejected".to_owned())?;
            let response_path = candidate.path().and_then(|value| {
                security::canonical_regular_file_under(&sessions_root, Path::new(value)).ok()
            });
            if candidate.id() != thread_id || response_path.as_ref() != Some(path) {
                return Err("thread/read identity mismatch".to_owned());
            }
            if !seen_ids.insert(thread_id.clone()) {
                return Err("duplicate active thread identity".to_owned());
            }
            if thread_items.len() >= MAX_ACTIVE_THREAD_ROWS {
                return Err("active thread row limit exceeded".to_owned());
            }
            rollouts.insert(thread_id.clone(), rollout);
            thread_items.push(thread_item.clone());
        }
        let mut accumulator = thread_contract::ThreadCycleAccumulator::new();
        accumulator
            .accept_page(&json!({"data": thread_items}))
            .map_err(|_| "thread/read cycle rejected".to_owned())?;
        let outcome = thread_contract::select_active_threads_parsed_where(
            accumulator,
            |candidate| {
                candidate
                    .path()
                    .and_then(|value| {
                        security::canonical_regular_file_under(&sessions_root, Path::new(value))
                            .ok()
                    })
                    .is_some_and(|path| {
                        candidates
                            .iter()
                            .any(|(candidate_path, _, _, _)| candidate_path == &path)
                    })
            },
            |candidate| rollouts.get(candidate.id()).cloned().ok_or(()),
        );
        let snapshots = match outcome {
            thread_contract::ThreadCycleOutcome::Snapshots(snapshots) => snapshots,
            thread_contract::ThreadCycleOutcome::NoThread => Vec::new(),
            thread_contract::ThreadCycleOutcome::CycleError => {
                return Err("active thread cycle rejected".to_owned())
            }
        };
        Ok(snapshots
            .into_iter()
            .map(|snapshot| ActiveThreadRecord {
                id: snapshot.thread_id,
                title: snapshot.title,
                model: snapshot.model,
                model_label: snapshot.model_label,
                created_at: Some(snapshot.created_at),
                updated_at: snapshot.updated_at,
                last_user_message_at: snapshot.last_user_message_at,
                total_tokens: snapshot.total_tokens,
                context_usage_tokens: snapshot.context_usage_tokens,
                context_window_tokens: snapshot.context_window_tokens,
                is_subagent: snapshot.is_subagent,
                parent_thread_id: snapshot.parent_thread_id,
                depth: snapshot.depth,
            })
            .collect::<Vec<_>>())
    })();
    drop(input);
    let _ = child.kill();
    let _ = child.wait();
    match result {
        Ok(threads) if threads.is_empty() => ActiveThreadPollResult::Empty,
        Ok(threads) => ActiveThreadPollResult::Snapshot(ActiveThreadSnapshot {
            observed_at: Utc::now().timestamp(),
            threads,
        }),
        Err(error) => ActiveThreadPollResult::Failed(error),
    }
}

fn read_active_rollout(
    sessions_root: &Path,
    expected_path: &Path,
    expected_metadata: &Metadata,
    checkpoint: &SessionCheckpoint,
) -> Result<thread_contract::ValidatedRollout, String> {
    let canonical = security::canonical_regular_file_under(sessions_root, expected_path)
        .map_err(|_| "active rollout path rejected".to_owned())?;
    if canonical != expected_path {
        return Err("active rollout path changed".to_owned());
    }
    let before_path = fs::symlink_metadata(&canonical)
        .map_err(|_| "active rollout path disappeared".to_owned())?;
    if before_path.file_type().is_symlink() || !before_path.is_file() {
        return Err("active rollout is not a regular file".to_owned());
    }
    let mut file = File::open(&canonical).map_err(|_| "active rollout open failed".to_owned())?;
    let before_file = file
        .metadata()
        .map_err(|_| "active rollout stat failed".to_owned())?;
    if !same_file_identity(&before_path, &before_file)
        || !same_file_identity(&before_file, expected_metadata)
        || before_file.len() > security::MAX_SESSION_FILE_BYTES
    {
        return Err("active rollout identity rejected".to_owned());
    }
    let snapshot_len = before_file.len();
    let checkpoint_offset = checkpoint.committed_offset;
    if checkpoint_offset > snapshot_len {
        return Err("active rollout checkpoint is ahead of file".to_owned());
    }
    let parse_start = if checkpoint.discard_until_lf {
        match first_rollout_newline_end(&mut file, checkpoint_offset, snapshot_len)? {
            Some(offset) => offset,
            None => checkpoint_offset,
        }
    } else {
        checkpoint_offset
    };
    let complete_len = if parse_start == checkpoint_offset && checkpoint.discard_until_lf {
        parse_start
    } else {
        complete_rollout_range_end(&mut file, parse_start, snapshot_len)?
    };
    let mut parser = thread_contract::RolloutAccumulator::seeded(
        checkpoint.last_model.clone(),
        checkpoint.previous_total,
        checkpoint
            .last_task_running
            .or_else(|| (complete_len > parse_start).then_some(true)),
    );
    if complete_len > parse_start {
        file.seek(SeekFrom::Start(parse_start))
            .map_err(|_| "active rollout seek failed".to_owned())?;
        let appended_len = complete_len
            .checked_sub(parse_start)
            .ok_or_else(|| "active rollout offset underflow".to_owned())?;
        let mut reader = BufReader::new((&mut file).take(appended_len));
        parser
            .apply_reader(&mut reader, appended_len)
            .map_err(|_| "active rollout parse rejected".to_owned())?;
    }
    let snapshot = parser
        .snapshot()
        .map_err(|_| "active rollout state rejected".to_owned())?;
    let after_file = file
        .metadata()
        .map_err(|_| "active rollout post-stat failed".to_owned())?;
    let after_path = fs::symlink_metadata(&canonical)
        .map_err(|_| "active rollout path post-stat failed".to_owned())?;
    if !same_file_identity(&before_file, &after_file)
        || !same_file_identity(&after_file, &after_path)
        || after_file.len() < before_file.len()
    {
        return Err("active rollout changed during read".to_owned());
    }
    Ok(snapshot)
}

fn first_rollout_newline_end(
    file: &mut File,
    start_offset: u64,
    snapshot_len: u64,
) -> Result<Option<u64>, String> {
    file.seek(SeekFrom::Start(start_offset))
        .map_err(|_| "active rollout seek failed".to_owned())?;
    let mut reader = BufReader::new(&mut *file);
    let mut observed = start_offset;
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|_| "active rollout read failed".to_owned())?;
        if buffer.is_empty() {
            break;
        }
        if let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
            let consumed = u64::try_from(position)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| "active rollout offset overflow".to_owned())?;
            observed = observed
                .checked_add(consumed)
                .ok_or_else(|| "active rollout offset overflow".to_owned())?;
            reader.consume(position + 1);
            return Ok((observed <= snapshot_len).then_some(observed));
        } else {
            let consumed = u64::try_from(buffer.len())
                .map_err(|_| "active rollout offset overflow".to_owned())?;
            let buffer_len = buffer.len();
            observed = observed
                .checked_add(consumed)
                .ok_or_else(|| "active rollout offset overflow".to_owned())?;
            reader.consume(buffer_len);
        }
        if observed >= snapshot_len {
            break;
        }
    }
    Ok(None)
}

fn complete_rollout_range_end(
    file: &mut File,
    start_offset: u64,
    snapshot_len: u64,
) -> Result<u64, String> {
    file.seek(SeekFrom::Start(start_offset))
        .map_err(|_| "active rollout seek failed".to_owned())?;
    let mut reader = BufReader::new(&mut *file);
    let mut observed = start_offset;
    let mut complete = start_offset;
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|_| "active rollout read failed".to_owned())?;
        if buffer.is_empty() {
            break;
        }
        if let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
            let consumed = u64::try_from(position)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| "active rollout offset overflow".to_owned())?;
            observed = observed
                .checked_add(consumed)
                .ok_or_else(|| "active rollout offset overflow".to_owned())?;
            reader.consume(position + 1);
            complete = observed;
        } else {
            let consumed = u64::try_from(buffer.len())
                .map_err(|_| "active rollout offset overflow".to_owned())?;
            let buffer_len = buffer.len();
            observed = observed
                .checked_add(consumed)
                .ok_or_else(|| "active rollout offset overflow".to_owned())?;
            reader.consume(buffer_len);
        }
        if observed >= snapshot_len {
            break;
        }
    }
    Ok(complete.min(snapshot_len))
}

fn read_session_meta_id(
    sessions_root: &Path,
    path: &Path,
    expected_metadata: &Metadata,
) -> Result<String, String> {
    let canonical = canonical_session_file(sessions_root, path)
        .ok_or_else(|| "session_meta path rejected".to_owned())?;
    let before_path =
        fs::symlink_metadata(&canonical).map_err(|_| "session_meta path disappeared".to_owned())?;
    let mut file = File::open(&canonical).map_err(|_| "session_meta open failed".to_owned())?;
    let before_file = file
        .metadata()
        .map_err(|_| "session_meta stat failed".to_owned())?;
    if !same_file_identity(&before_path, &before_file)
        || file_device(&before_file) != file_device(expected_metadata)
        || file_inode(&before_file) != file_inode(expected_metadata)
    {
        return Err("session_meta identity mismatch".to_owned());
    }
    let line = read_bounded_rpc_line(&mut BufReader::new(
        (&mut file).take(APP_SERVER_MAX_LINE_BYTES as u64 + 1),
    ))?
    .ok_or_else(|| "session_meta is empty".to_owned())?;
    let value: Value =
        serde_json::from_str(&line).map_err(|_| "session_meta JSON rejected".to_owned())?;
    let object = value
        .as_object()
        .ok_or_else(|| "session_meta envelope rejected".to_owned())?;
    if object.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Err("session_meta record rejected".to_owned());
    }
    let id = object
        .get("payload")
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("id"))
        .and_then(Value::as_str)
        .filter(|id| {
            (1..=MAX_THREAD_ID_SCALARS).contains(&id.chars().count())
                && !id.chars().any(char::is_control)
        })
        .ok_or_else(|| "session_meta id rejected".to_owned())?
        .to_owned();
    let after_file = file
        .metadata()
        .map_err(|_| "session_meta post-stat failed".to_owned())?;
    let after_path = fs::symlink_metadata(&canonical)
        .map_err(|_| "session_meta path post-stat failed".to_owned())?;
    if !same_file_identity(&before_file, &after_file)
        || !same_file_identity(&after_file, &after_path)
        || after_file.len() < before_file.len()
    {
        return Err("session_meta changed during read".to_owned());
    }
    Ok(id)
}

fn active_thread_paths(sessions_root: &Path) -> Result<(PathBuf, BTreeSet<PathBuf>), String> {
    #[cfg(not(unix))]
    {
        let _ = sessions_root;
        return Err("active session process inventory unavailable".to_owned());
    }
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(sessions_root)
            .map_err(|_| "sessions root unavailable".to_owned())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("sessions root is not a regular directory".to_owned());
        }
        let canonical_root = sessions_root
            .canonicalize()
            .map_err(|_| "sessions root could not be canonicalized".to_owned())?;
        let paths = open_codex_session_paths(Path::new("/proc"), &canonical_root)?;
        Ok((canonical_root, paths))
    }
}

#[cfg(unix)]
fn open_codex_session_paths(
    proc_root: &Path,
    sessions_root: &Path,
) -> Result<BTreeSet<PathBuf>, String> {
    let mut process_entries = 0usize;
    let mut open_files = BTreeSet::new();
    let processes = fs::read_dir(proc_root).map_err(|_| "process inventory failed".to_owned())?;
    for process in processes {
        process_entries = process_entries
            .checked_add(1)
            .ok_or_else(|| "process inventory limit exceeded".to_owned())?;
        if process_entries > MAX_PROC_PROCESS_ENTRIES {
            return Err("process inventory limit exceeded".to_owned());
        }
        let process = match process {
            Ok(process) => process,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err("process inventory entry failed".to_owned()),
        };
        let name = process.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let process_path = process.path();
        let mut comm = Vec::new();
        match File::open(process_path.join("comm")) {
            Ok(file) => {
                if file.take(64).read_to_end(&mut comm).is_err() {
                    return Err("process name read failed".to_owned());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err("process name open failed".to_owned()),
        }
        if comm.strip_suffix(b"\n") != Some(b"codex") && comm.as_slice() != b"codex" {
            continue;
        }
        let executable = match fs::read_link(process_path.join("exe")) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err("process executable read failed".to_owned()),
        };
        if executable.file_name().and_then(|value| value.to_str()) != Some("codex") {
            continue;
        }
        let descriptors = match fs::read_dir(process_path.join("fd")) {
            Ok(descriptors) => descriptors,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err("process descriptor inventory failed".to_owned()),
        };
        let mut descriptor_count = 0usize;
        for descriptor in descriptors {
            descriptor_count = descriptor_count
                .checked_add(1)
                .ok_or_else(|| "process descriptor limit exceeded".to_owned())?;
            if descriptor_count > MAX_CODEX_PROCESS_FDS {
                return Err("process descriptor limit exceeded".to_owned());
            }
            let descriptor = match descriptor {
                Ok(descriptor) => descriptor,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return Err("process descriptor entry failed".to_owned()),
            };
            let target = match fs::read_link(descriptor.path()) {
                Ok(path) => path,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return Err("process descriptor read failed".to_owned()),
            };
            let Some(canonical) = canonical_session_file(sessions_root, &target) else {
                continue;
            };
            open_files.insert(canonical);
            if open_files.len() > MAX_OPEN_SESSION_FILES {
                return Err("active session file limit exceeded".to_owned());
            }
        }
    }
    Ok(open_files)
}

fn canonical_session_file(root: &Path, candidate: &Path) -> Option<PathBuf> {
    let canonical = security::canonical_regular_file_under(root, candidate).ok()?;
    (canonical.extension().and_then(|value| value.to_str()) == Some("jsonl")).then_some(canonical)
}

fn same_file_identity(left: &Metadata, right: &Metadata) -> bool {
    file_device(left) == file_device(right) && file_inode(left) == file_inode(right)
}

#[cfg(unix)]
fn file_device(metadata: &Metadata) -> u64 {
    metadata.dev()
}

#[cfg(not(unix))]
fn file_device(_metadata: &Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn file_inode(metadata: &Metadata) -> u64 {
    metadata.ino()
}

#[cfg(not(unix))]
fn file_inode(_metadata: &Metadata) -> u64 {
    0
}

fn decode_app_server_account(value: &Value) -> Result<AppServerAccount, String> {
    let account = value
        .as_object()
        .and_then(|object| object.get("account"))
        .and_then(Value::as_object)
        .ok_or_else(|| "Codex account response is unavailable".to_owned())?;
    if account.get("type").and_then(Value::as_str) != Some("chatgpt") {
        return Err("Codex account is not an authenticated ChatGPT account".to_owned());
    }
    let email = account
        .get("email")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
        .ok_or_else(|| "Codex account email is unavailable".to_owned())?;
    let plan_type = account
        .get("planType")
        .and_then(Value::as_str)
        .filter(|value| is_supported_plan(value))
        .ok_or_else(|| "Codex account plan is unavailable".to_owned())?;
    Ok(AppServerAccount {
        email: email.to_owned(),
        plan_type: plan_type.to_owned(),
    })
}

fn is_supported_plan(value: &str) -> bool {
    matches!(
        value,
        "free"
            | "go"
            | "plus"
            | "pro"
            | "prolite"
            | "team"
            | "self_serve_business_prolite"
            | "self_serve_business_usage_based"
            | "business"
            | "ent26"
            | "enterprise_cbp_automation"
            | "enterprise_cbp_usage_based"
            | "enterprise"
            | "edu"
    )
}

fn monthly_quota_window_seconds(reset_at: i64) -> i64 {
    let Some(end) = DateTime::<Utc>::from_timestamp(reset_at, 0) else {
        return 31 * 86_400;
    };
    end.checked_sub_months(Months::new(1))
        .map(|start| (end - start).num_seconds().max(1))
        .unwrap_or(31 * 86_400)
}

fn parse_app_server_quota(value: &Value, plan_type: &str) -> Result<QuotaSnapshot, String> {
    let limits = value
        .as_object()
        .and_then(|object| object.get("rateLimits"))
        .and_then(Value::as_object)
        .ok_or_else(|| "Codex quota response is unavailable".to_owned())?;
    if plan_type.starts_with("enterprise") {
        if let Some(individual) = limits.get("individualLimit").and_then(Value::as_object) {
            let remaining = individual
                .get("remainingPercent")
                .and_then(Value::as_i64)
                .filter(|value| (0..=100).contains(value));
            let reset_at = individual
                .get("resetsAt")
                .and_then(Value::as_i64)
                .filter(|value| *value > 0);
            if let (Some(remaining), Some(reset_at)) = (remaining, reset_at) {
                return Ok(QuotaSnapshot {
                    observed_at: Utc::now().timestamp(),
                    reset_at,
                    window_seconds: monthly_quota_window_seconds(reset_at),
                    remaining_percent: Some(remaining as f64),
                });
            }
        }
    }
    let mut selected: Option<(i64, i64, i64, u8)> = None;
    for (key, priority) in [("primary", 0_u8), ("secondary", 1_u8)] {
        let Some(window) = limits.get(key).and_then(Value::as_object) else {
            continue;
        };
        let used = window
            .get("usedPercent")
            .and_then(Value::as_i64)
            .filter(|value| (0..=100).contains(value));
        let reset_at = window
            .get("resetsAt")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0);
        let duration_minutes = window
            .get("windowDurationMins")
            .and_then(Value::as_i64)
            .filter(|value| (1..=527_040).contains(value));
        let (Some(used), Some(reset_at), Some(duration_minutes)) =
            (used, reset_at, duration_minutes)
        else {
            return Err("Codex quota window is malformed".to_owned());
        };
        let window_seconds = duration_minutes
            .checked_mul(60)
            .ok_or_else(|| "Codex quota window is too large".to_owned())?;
        let candidate = (window_seconds, reset_at, 100 - used, priority);
        let better = selected.is_none_or(|current| {
            candidate.0 > current.0
                || (candidate.0 == current.0 && candidate.1 > current.1)
                || (candidate.0 == current.0 && candidate.1 == current.1 && candidate.3 < current.3)
        });
        if better {
            selected = Some(candidate);
        }
    }
    let Some((window_seconds, reset_at, remaining, _)) = selected else {
        return Err("Codex quota has no bounded window".to_owned());
    };
    Ok(QuotaSnapshot {
        observed_at: Utc::now().timestamp(),
        reset_at,
        window_seconds,
        remaining_percent: Some(remaining as f64),
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TokenSnapshot {
    pub total: u64,
    pub input: u64,
    pub cached_input: u64,
    pub output: u64,
    pub cache_write_input: Option<u64>,
}

impl TokenSnapshot {
    fn checked_delta_from(self, previous: Self) -> Option<Self> {
        let cache_write_input = match (self.cache_write_input, previous.cache_write_input) {
            (Some(current), Some(before)) => Some(current.checked_sub(before)?),
            (None, None) => None,
            _ => return None,
        };
        Some(Self {
            total: self.total.checked_sub(previous.total)?,
            input: self.input.checked_sub(previous.input)?,
            cached_input: self.cached_input.checked_sub(previous.cached_input)?,
            output: self.output.checked_sub(previous.output)?,
            cache_write_input,
        })
    }

    fn cache_write_delta_from(self, previous: Self) -> Option<u64> {
        match (self.cache_write_input, previous.cache_write_input) {
            (Some(current), Some(before)) => current.checked_sub(before),
            (Some(current), None) if previous.total == 0 => Some(current),
            (Some(0), None) => Some(0),
            _ => None,
        }
    }

    fn valid(self) -> bool {
        self.cached_input <= self.input
            && self.cache_write_input.is_none_or(|writes| {
                self.cached_input
                    .checked_add(writes)
                    .is_some_and(|value| value <= self.input)
            })
    }

    fn has_usage(self) -> bool {
        self.total > 0
            || self.input > 0
            || self.cached_input > 0
            || self.output > 0
            || self.cache_write_input.is_some_and(|value| value > 0)
    }
}

#[derive(Clone, Debug)]
struct ModelCounter {
    total: u64,
    input: u64,
    cached_input: u64,
    output: u64,
    cache_write_input: Option<u64>,
}

impl Default for ModelCounter {
    fn default() -> Self {
        Self {
            total: 0,
            input: 0,
            cached_input: 0,
            output: 0,
            // `Some(0)` is the additive identity for a known counter. Any
            // unknown delta still changes the accumulated value to `None`.
            cache_write_input: Some(0),
        }
    }
}

impl ModelCounter {
    fn from_total(total: &SessionModelTotal) -> Self {
        Self {
            total: total.total_tokens,
            input: total.input_tokens,
            cached_input: total.cached_input_tokens,
            output: total.output_tokens,
            cache_write_input: total.cache_write_input_tokens,
        }
    }

    fn add(&mut self, delta: TokenSnapshot) -> Result<(), RecorderError> {
        self.total = self
            .total
            .checked_add(delta.total)
            .ok_or_else(|| RecorderError::Invalid("session token total overflow".to_owned()))?;
        self.input = self
            .input
            .checked_add(delta.input)
            .ok_or_else(|| RecorderError::Invalid("session input total overflow".to_owned()))?;
        self.cached_input = self
            .cached_input
            .checked_add(delta.cached_input)
            .ok_or_else(|| {
                RecorderError::Invalid("session cached input total overflow".to_owned())
            })?;
        self.output = self
            .output
            .checked_add(delta.output)
            .ok_or_else(|| RecorderError::Invalid("session output total overflow".to_owned()))?;
        self.cache_write_input = match (self.cache_write_input, delta.cache_write_input) {
            (Some(left), Some(right)) => Some(left.checked_add(right).ok_or_else(|| {
                RecorderError::Invalid("session cache-write total overflow".to_owned())
            })?),
            _ => None,
        };
        if self.cached_input > self.input
            || self.cache_write_input.is_some_and(|writes| {
                self.cached_input
                    .checked_add(writes)
                    .is_none_or(|value| value > self.input)
            })
        {
            return Err(RecorderError::Invalid(
                "session token components are inconsistent".to_owned(),
            ));
        }
        Ok(())
    }

    fn snapshot(&self) -> TokenSnapshot {
        TokenSnapshot {
            total: self.total,
            input: self.input,
            cached_input: self.cached_input,
            output: self.output,
            cache_write_input: self.cache_write_input,
        }
    }

    fn to_total(&self, model: &str) -> SessionModelTotal {
        SessionModelTotal {
            model: model.to_owned(),
            total_tokens: self.total,
            input_tokens: self.input,
            cached_input_tokens: self.cached_input,
            output_tokens: self.output,
            cache_write_input_tokens: self.cache_write_input,
        }
    }

    fn dollars(&self, model: &str) -> f64 {
        let input = self.input.saturating_sub(self.cached_input) as f64;
        let cached = self.cached_input as f64;
        let output = self.output as f64;
        let (input_rate, cached_rate, output_rate) = match model {
            "SOL" => (5.0, 0.5, 30.0),
            "TERRA" => (2.0, 0.2, 12.0),
            "LUNA" => (0.2, 0.02, 1.2),
            "ASTRA" => {
                let Some(writes) = self.cache_write_input else {
                    return 0.0;
                };
                let ordinary = self
                    .input
                    .saturating_sub(self.cached_input)
                    .saturating_sub(writes);
                return ordinary as f64 * 10.0 / 1_000_000.0
                    + self.cached_input as f64 * 1.0 / 1_000_000.0
                    + writes as f64 * 12.5 / 1_000_000.0
                    + output * 50.0 / 1_000_000.0;
            }
            _ => return 0.0,
        };
        (input * input_rate + cached * cached_rate + output * output_rate) / 1_000_000.0
    }
}

#[derive(Clone, Debug, Default)]
struct ModelTotals {
    values: BTreeMap<String, ModelCounter>,
}

impl ModelTotals {
    fn from_state(state: &[SessionModelTotal]) -> Self {
        let mut values = BTreeMap::new();
        for total in state {
            values.insert(total.model.clone(), ModelCounter::from_total(total));
        }
        Self { values }
    }

    fn canonical_model(model: &str) -> Option<String> {
        let lowered = model.to_ascii_lowercase();
        let canonical = if lowered.contains("sol") {
            "SOL"
        } else if lowered.contains("terra") {
            "TERRA"
        } else if lowered.contains("luna") {
            "LUNA"
        } else if lowered.contains("astra") {
            "ASTRA"
        } else {
            let trimmed = model.trim();
            if trimmed.is_empty()
                || trimmed.len() > codex_info_db_writer::MAX_SESSION_MODEL_BYTES
                || trimmed.chars().any(char::is_control)
            {
                return None;
            }
            return Some(trimmed.to_owned());
        };
        Some(canonical.to_owned())
    }

    fn checkpoint_model(model: &str) -> Option<String> {
        let trimmed = model.trim();
        if !trimmed.is_empty()
            && trimmed.len() <= codex_info_db_writer::MAX_SESSION_MODEL_BYTES
            && !trimmed.chars().any(char::is_control)
        {
            Some(trimmed.to_owned())
        } else {
            Self::canonical_model(model)
        }
    }

    fn usage_model(model: Option<&str>) -> String {
        model
            .and_then(Self::canonical_model)
            .unwrap_or_else(|| UNATTRIBUTED_MODEL.to_owned())
    }

    fn add(&mut self, model: &str, delta: TokenSnapshot) -> Result<(), RecorderError> {
        if !delta.has_usage() {
            return Ok(());
        }
        let model = Self::canonical_model(model).unwrap_or_else(|| UNATTRIBUTED_MODEL.to_owned());
        self.values.entry(model).or_default().add(delta)
    }

    fn to_totals(&self) -> Vec<SessionModelTotal> {
        self.values
            .iter()
            .map(|(model, counter)| counter.to_total(model))
            .collect()
    }

    fn positive_totals(&self) -> Vec<SessionModelTotal> {
        self.values
            .iter()
            .filter(|(_, counter)| counter.snapshot().has_usage())
            .map(|(model, counter)| counter.to_total(model))
            .collect()
    }

    fn dollar_totals(&self) -> (f64, f64, f64) {
        let mut result = (0.0, 0.0, 0.0);
        for (model, counter) in &self.values {
            let dollars = counter.dollars(model);
            match model.as_str() {
                "SOL" => result.0 += dollars,
                "TERRA" => result.1 += dollars,
                "LUNA" => result.2 += dollars,
                _ => {}
            }
        }
        result
    }

    fn token_totals(&self) -> (u64, u64, u64) {
        (
            self.values.get("SOL").map_or(0, |row| row.total),
            self.values.get("TERRA").map_or(0, |row| row.total),
            self.values.get("LUNA").map_or(0, |row| row.total),
        )
    }

    fn history_sample(
        &self,
        timestamp: i64,
        reset_at: i64,
        remaining_percent: f64,
    ) -> UsageHistorySample {
        let dollars = self.dollar_totals();
        let tokens = self.token_totals();
        UsageHistorySample {
            timestamp,
            reset_at,
            remaining_percent: Some(remaining_percent),
            sol_dollars: dollars.0,
            terra_dollars: dollars.1,
            luna_dollars: dollars.2,
            sol_tokens: tokens.0,
            terra_tokens: tokens.1,
            luna_tokens: tokens.2,
        }
    }
}

#[derive(Clone)]
struct TimedModelUsage {
    timestamp: i64,
    model: String,
    delta: TokenSnapshot,
}

fn session_event_to_timed_usage(event: SessionEvent) -> Result<TimedModelUsage, RecorderError> {
    let delta = TokenSnapshot {
        total: event.total_tokens,
        input: event.input_tokens,
        cached_input: event.cached_input_tokens,
        output: event.output_tokens,
        cache_write_input: event.cache_write_input_tokens,
    };
    if !delta.valid() {
        return Err(RecorderError::Invalid(
            "durable session event has invalid token components".to_owned(),
        ));
    }
    Ok(TimedModelUsage {
        timestamp: event.timestamp,
        model: event.model,
        delta,
    })
}

#[derive(Clone)]
struct PendingBatch {
    reset_at: i64,
    window_seconds: i64,
    collector_epoch: u128,
    cycle_seq: u64,
    samples: Vec<UsageHistorySample>,
    observations: Vec<UsageHistoryObservation>,
    checkpoints: Vec<SessionCheckpoint>,
    ranges: Vec<SessionRange>,
    durable_events: Vec<SessionEvent>,
    pending_evidence: Vec<SessionPendingRange>,
    model_totals: Vec<SessionModelTotal>,
    timeline_recovery: Option<SessionTimelineRecovery>,
    accepted_ranges: usize,
    sources_seen: usize,
}

struct WriterLock {
    _file: File,
}

/// Profile-level process lease expected by the installer.  The JSON identity
/// is diagnostic evidence; ownership itself is the kernel advisory lock, so a
/// crash or SIGKILL releases it without deleting a path another process may
/// have opened.
pub struct ProfileLease {
    _file: File,
    pid: u32,
    starttime_ticks: i64,
    owner_nonce: String,
}

impl ProfileLease {
    pub fn acquire(data_root: impl AsRef<Path>) -> Result<Self, RecorderError> {
        let history = data_root.as_ref().join("history");
        let history_metadata = fs::symlink_metadata(&history)?;
        if history_metadata.file_type().is_symlink() || !history_metadata.is_dir() {
            return Err(RecorderError::Invalid(
                "recorder profile lease parent is not a regular directory".to_owned(),
            ));
        }
        let path = history.join("usage_record_daemon.lock");
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(RecorderError::Invalid(
                    "recorder profile lease is not a regular file".to_owned(),
                ));
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&path)?;
        #[cfg(unix)]
        {
            let result =
                rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive);
            if let Err(error) = result {
                if matches!(error.raw_os_error(), 11 | 35) {
                    return Err(RecorderError::Invalid(
                        "recorder profile lease is already owned".to_owned(),
                    ));
                }
                return Err(RecorderError::Io(error.into()));
            }
        }
        let pid = std::process::id();
        let starttime_ticks = process_starttime_ticks(pid)?;
        let (executable_device, executable_inode) = executable_identity()?;
        let mut nonce_bytes = [0_u8; 16];
        getrandom::fill(&mut nonce_bytes)
            .map_err(|error| RecorderError::Invalid(format!("lease nonce failed: {error}")))?;
        let owner_nonce = hex_bytes(&nonce_bytes);
        let document = json!({
            "pid": pid,
            "started_at": Utc::now().timestamp(),
            "starttime_ticks": starttime_ticks,
            "executable_device": executable_device,
            "executable_inode": executable_inode,
            "owner_nonce": owner_nonce,
        });
        let bytes = serde_json::to_vec(&document).map_err(|error| {
            RecorderError::Invalid(format!("lease state encode failed: {error}"))
        })?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(Self {
            _file: file,
            pid,
            starttime_ticks,
            owner_nonce,
        })
    }
}

/// Atomic installer-visible recorder state.  It deliberately contains only
/// the fixed v1 key set consumed by `recorder_identity_check`.
pub struct RecorderStateWriter {
    path: PathBuf,
    partition_id: String,
    pid: u32,
    starttime_ticks: i64,
    owner_nonce: String,
    last_commit_unix: Option<i64>,
    last_state: Option<SessionCollectionState>,
}

impl RecorderStateWriter {
    pub fn new(
        data_root: impl AsRef<Path>,
        identity: &StoragePartitionIdentity,
        lease: &ProfileLease,
    ) -> Result<Self, RecorderError> {
        let history = data_root.as_ref().join("history");
        let metadata = fs::symlink_metadata(&history)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(RecorderError::Invalid(
                "recorder state parent is not a regular directory".to_owned(),
            ));
        }
        Ok(Self {
            path: history.join("recorder-state.json"),
            partition_id: identity.partition_id.clone(),
            pid: lease.pid,
            starttime_ticks: lease.starttime_ticks,
            owner_nonce: lease.owner_nonce.clone(),
            last_commit_unix: None,
            last_state: None,
        })
    }

    pub fn write_degraded(
        &mut self,
        state: Option<&SessionCollectionState>,
    ) -> Result<(), RecorderError> {
        let state = state.or(self.last_state.as_ref());
        // A degraded write must retain the last successful commit identity.
        // On a fresh process there may be no durable commit yet; leave an
        // absent (or existing last-good) state file untouched rather than
        // replacing it with an installer-invalid null generation.
        if self.last_commit_unix.is_none() {
            if state.is_some_and(|state| state.data_generation == 0) {
                return self.write_document("degraded", state, None);
            }
            return Err(RecorderError::Invalid(
                "no successful recorder commit is available for degraded state".to_owned(),
            ));
        }
        if state.is_none() {
            return Err(RecorderError::Invalid(
                "no recorder state is available for degraded state".to_owned(),
            ));
        }
        self.write_document("degraded", state, self.last_commit_unix)
    }

    pub fn write_ready(&mut self, state: &SessionCollectionState) -> Result<(), RecorderError> {
        self.write_committed(state, false)
    }

    pub fn write_committed(
        &mut self,
        state: &SessionCollectionState,
        has_pending: bool,
    ) -> Result<(), RecorderError> {
        let now = Utc::now().timestamp();
        self.last_commit_unix = Some(now);
        self.last_state = Some(state.clone());
        self.write_document(
            if has_pending { "degraded" } else { "ready" },
            Some(state),
            self.last_commit_unix,
        )
    }

    fn write_document(
        &self,
        write_state: &str,
        state: Option<&SessionCollectionState>,
        last_commit_unix: Option<i64>,
    ) -> Result<(), RecorderError> {
        let generation = state
            .filter(|state| state.data_generation > 0)
            .map(|state| state.data_generation);
        let collector_epoch = state
            .filter(|state| state.data_generation > 0)
            .and_then(|state| state.collector_epoch)
            .map(|epoch| format!("{epoch:032x}"));
        let cycle_seq = state
            .filter(|state| state.data_generation > 0)
            .map(|state| state.cycle_seq)
            .filter(|cycle| *cycle > 0);
        let document = json!({
            "schema": "codex-info-recorder-state-v1",
            "pid": self.pid,
            "process_starttime": self.starttime_ticks,
            "owner_nonce": self.owner_nonce,
            "write_state": write_state,
            "partition_id_hash": self.partition_id,
            "data_generation": generation,
            "collector_epoch": collector_epoch,
            "cycle_seq": cycle_seq,
            "last_commit_unix": last_commit_unix,
            "updated_at_unix": Utc::now().timestamp(),
        });
        let bytes = serde_json::to_vec(&document).map_err(|error| {
            RecorderError::Invalid(format!("recorder state encode failed: {error}"))
        })?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| RecorderError::Invalid("recorder state parent is missing".to_owned()))?;
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|error| {
            RecorderError::Invalid(format!("recorder state nonce failed: {error}"))
        })?;
        let temporary = parent.join(format!(".recorder-state.json.tmp-{}", hex_bytes(&nonce)));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary)?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            if let Ok(existing) = fs::symlink_metadata(&self.path) {
                if existing.file_type().is_symlink() || !existing.is_file() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "recorder state is not a regular file",
                    ));
                }
            }
            fs::rename(&temporary, &self.path)?;
            File::open(parent).and_then(|directory| directory.sync_all())?;
            Ok::<(), io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(RecorderError::Io)
    }
}

impl WriterLock {
    fn acquire(database: &Path) -> Result<Self, RecorderError> {
        let parent = database.parent().ok_or_else(|| {
            RecorderError::Invalid("partition database parent is missing".to_owned())
        })?;
        if !parent.is_dir() {
            return Err(RecorderError::Invalid(
                "partition database parent is missing".to_owned(),
            ));
        }
        let path = parent.join("account-recorder.lock");
        if let Ok(existing) = fs::symlink_metadata(&path) {
            if existing.file_type().is_symlink() || !existing.is_file() {
                return Err(RecorderError::Invalid(
                    "account partition writer lock is not a regular file".to_owned(),
                ));
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(RecorderError::Invalid(
                "account partition writer lock is not a regular file".to_owned(),
            ));
        }
        #[cfg(unix)]
        {
            // The kernel owns this lock, so a SIGKILL cannot strand ownership
            // in the persistent diagnostic file.  Keep the path in place and
            // never unlink a file another process may have opened.
            let result =
                rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive);
            if let Err(error) = result {
                if matches!(error.raw_os_error(), 11 | 35) {
                    return Err(RecorderError::Invalid(
                        "account partition writer is already owned".to_owned(),
                    ));
                }
                return Err(RecorderError::Io(error.into()));
            }
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        let pid = std::process::id();
        writeln!(file, "pid={pid}")?;
        file.sync_all()?;
        Ok(Self { _file: file })
    }
}

pub struct Recorder {
    config: RecorderConfig,
    writer: UsageStore,
    database: Option<PathBuf>,
    identity: Option<StoragePartitionIdentity>,
    _writer_lock: Option<WriterLock>,
    pending: Option<PendingBatch>,
    last_ranges: Vec<SessionRange>,
}

impl Recorder {
    /// Open an already initialized partition. Allocation and profile metadata
    /// remain outside this crate; a missing or uninitialized partition fails
    /// closed instead of creating a second schema.
    pub fn open_partitioned(
        config: RecorderConfig,
        database: impl AsRef<Path>,
        identity: &StoragePartitionIdentity,
    ) -> Result<Self, RecorderError> {
        config.validate()?;
        let database = database.as_ref().to_owned();
        let lock = WriterLock::acquire(&database)?;
        let existing_database = match fs::symlink_metadata(&database) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(RecorderError::Invalid(
                        "partition database must be a regular file".to_owned(),
                    ));
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if existing_database {
            UsageStore::backup_generations_partitioned(&database, identity, 3)?;
        }
        let mut writer = if existing_database {
            UsageStore::open_partitioned(&database, identity)?
        } else {
            UsageStore::create_partitioned(&database, identity)?
        };
        writer.prune_older_than_three_months(Utc::now())?;
        Ok(Self {
            config,
            writer,
            database: Some(database),
            identity: Some(identity.clone()),
            _writer_lock: Some(lock),
            pending: None,
            last_ranges: Vec::new(),
        })
    }

    /// Test/embedding constructor for a caller that already owns a UsageStore.
    /// Production account partitions use open_partitioned so the sibling lock
    /// and startup backups cannot be accidentally omitted.
    pub fn open_with_writer(
        config: RecorderConfig,
        writer: UsageStore,
    ) -> Result<Self, RecorderError> {
        config.validate()?;
        Ok(Self {
            config,
            writer,
            database: None,
            identity: None,
            _writer_lock: None,
            pending: None,
            last_ranges: Vec::new(),
        })
    }

    pub fn generation(&self) -> Result<u64, RecorderError> {
        Ok(self.writer.load_session_collection_state()?.data_generation)
    }

    pub fn state(&self) -> Result<SessionCollectionState, RecorderError> {
        Ok(self.writer.load_session_collection_state()?)
    }

    /// Publish a complete active-thread candidate through the writer's
    /// atomic snapshot boundary. The caller owns the independent poll lane;
    /// this method only performs the bounded DB operation after a result has
    /// been drained and never runs on the Session RPC worker.
    pub fn commit_active_thread_snapshot(
        &mut self,
        snapshot: &ActiveThreadSnapshot,
        acquisition_degraded: bool,
    ) -> Result<u64, RecorderError> {
        Ok(self
            .writer
            .commit_active_thread_snapshot_with_health(snapshot, acquisition_degraded)?)
    }

    pub fn mark_active_thread_snapshot_degraded(&mut self) -> Result<u64, RecorderError> {
        Ok(self.writer.mark_active_thread_snapshot_degraded()?)
    }

    pub fn clear_active_thread_snapshot_degraded(&mut self) -> Result<u64, RecorderError> {
        Ok(self.writer.clear_active_thread_snapshot_degraded()?)
    }

    pub fn model_totals(&self) -> Result<BTreeMap<String, TokenSnapshot>, RecorderError> {
        let state = self.writer.load_session_collection_state()?;
        Ok(state
            .model_totals
            .into_iter()
            .map(|total| {
                (
                    total.model,
                    TokenSnapshot {
                        total: total.total_tokens,
                        input: total.input_tokens,
                        cached_input: total.cached_input_tokens,
                        output: total.output_tokens,
                        cache_write_input: total.cache_write_input_tokens,
                    },
                )
            })
            .collect())
    }

    pub fn ranges(&self) -> &[SessionRange] {
        &self.last_ranges
    }

    /// Collect one bounded cycle. If a prior commit failed, the exact
    /// candidate vectors are retried without rescanning or changing their
    /// generation identity.
    pub fn run_cycle(&mut self) -> Result<Option<CycleReport>, RecorderError> {
        self.run_cycle_with_quota(None)
    }

    /// Collect one cycle while optionally admitting a quota observation from
    /// the independent app-server lane. A rejected or failed quota candidate
    /// never blocks local Session progress; a successful boundary resets only
    /// the period's cumulative model authority while preserving source
    /// checkpoints for append-only replay.
    pub fn run_cycle_with_quota(
        &mut self,
        quota: Option<QuotaSnapshot>,
    ) -> Result<Option<CycleReport>, RecorderError> {
        if self.pending.is_some() {
            return self.commit_pending().map(Some);
        }

        let state = self.writer.load_session_collection_state()?;
        // Pending parser evidence survives process restarts. Keep it in the
        // cycle's degraded decision even when the current source scan starts
        // after a completed malformed record; only the writer transaction
        // may remove evidence after an accepted exact range.
        let durable_pending_count = self.writer.load_session_pending_ranges()?.len();
        let admitted_quota = quota
            .as_ref()
            .and_then(|candidate| admit_quota_period(&state, candidate));
        let (reset_at, window_seconds, reset_transition) = admitted_quota
            .as_ref()
            .map(|(transition, reset_at, window_seconds)| {
                (*reset_at, *window_seconds, Some(*transition))
            })
            .unwrap_or((state.reset_at, state.window_seconds, None));
        if reset_at < 0 || window_seconds < 0 || (reset_at == 0) != (window_seconds == 0) {
            return Err(RecorderError::Invalid(
                "durable session period is invalid".to_owned(),
            ));
        }
        let period_available = reset_at > 0 && window_seconds > 0;
        let period_restarted = matches!(
            reset_transition,
            Some(QuotaTransition::Initial | QuotaTransition::Boundary)
        ) || !period_available;
        let discovery = discover_sources(&self.config.sessions_root);
        let collector_epoch = state.collector_epoch.unwrap_or_else(|| {
            collector_epoch(&self.config.sessions_root, self.database.as_deref(), &state)
        });
        let cycle_seq = state
            .cycle_seq
            .checked_add(1)
            .ok_or_else(|| RecorderError::Invalid("session cycle sequence overflow".to_owned()))?;
        let (sources, inventory_root, inventory_failures) = match discovery {
            Ok(inventory) => (
                inventory.sources,
                inventory.root_identity,
                inventory.failures,
            ),
            Err(error) => (
                Vec::new(),
                fallback_root_identity(&self.config.sessions_root),
                vec![InventoryFailure {
                    relative_path: "__inventory__".to_owned(),
                    reason: format!("session-root-inventory-failed: {error}"),
                }],
            ),
        };
        let baseline_existing =
            state.checkpoints.is_empty() && state.collector_epoch != Some(collector_epoch);
        let mut totals = if period_restarted || state.data_generation == 0 {
            ModelTotals::default()
        } else {
            ModelTotals::from_state(&state.model_totals)
        };
        let mut events = Vec::new();
        let mut checkpoints = Vec::new();
        let mut ranges = Vec::new();
        let mut durable_events = Vec::new();
        let mut pending_evidence = inventory_failures
            .into_iter()
            .map(|failure| {
                pending_inventory_failure(
                    inventory_root.clone(),
                    failure,
                    collector_epoch,
                    cycle_seq,
                )
            })
            .collect::<Vec<_>>();
        let mut consumed_budget = 0_u64;
        let mut pending_ranges = pending_evidence.len();
        if durable_pending_count != 0 {
            pending_ranges = pending_ranges.max(1);
        }
        let window_start = reset_at.saturating_sub(window_seconds);
        let timeline_end = if period_available {
            quota
                .as_ref()
                .filter(|_| admitted_quota.is_some())
                .map_or_else(|| Utc::now().timestamp(), |candidate| candidate.observed_at)
                .min(reset_at)
        } else {
            0
        };
        let replayed_events = if period_restarted && period_available {
            self.writer
                .load_session_events()?
                .into_iter()
                .filter(|event| event.timestamp >= window_start && event.timestamp <= timeline_end)
                .map(session_event_to_timed_usage)
                .collect::<Result<Vec<_>, RecorderError>>()?
        } else {
            Vec::new()
        };
        for event in &replayed_events {
            totals.add(&event.model, event.delta)?;
        }
        let initial_totals = totals.clone();

        let source_count = sources.len();
        let first_source = if source_count == 0 {
            0
        } else {
            usize::try_from(cycle_seq % source_count as u64).unwrap_or(0)
        };
        for logical_index in 0..source_count {
            let source = &sources[(first_source + logical_index) % source_count];
            let prior = prior_checkpoint(&state.checkpoints, source);
            if consumed_budget >= self.config.chunk_bytes {
                if checkpoint_covers_observed_end(prior, source) {
                    continue;
                }
                pending_ranges = pending_ranges.saturating_add(1);
                pending_evidence.push(pending_source_issue(
                    source,
                    prior,
                    collector_epoch,
                    cycle_seq,
                    "cycle-budget-backlog",
                ));
                continue;
            }
            let result = scan_source(
                source,
                prior,
                baseline_existing,
                collector_epoch,
                cycle_seq,
                reset_at,
                window_start,
                self.config.chunk_bytes.saturating_sub(consumed_budget),
                &mut totals,
                &mut events,
            );
            let Some(result) = (match result {
                Ok(result) => result,
                Err(error) => {
                    // A source can disappear or be replaced while the
                    // inventory is being walked.  Keep the other sources
                    // collectable and retry this source on the next cycle.
                    eprintln!(
                        "recorder degraded: session source {} was not collected: {error}",
                        source.recorded.relative_path
                    );
                    pending_ranges = pending_ranges.saturating_add(1);
                    pending_evidence.push(pending_source_issue(
                        source,
                        prior,
                        collector_epoch,
                        cycle_seq,
                        "source-scan-failed",
                    ));
                    continue;
                }
            }) else {
                pending_ranges = pending_ranges.saturating_add(1);
                pending_evidence.push(pending_source_issue(
                    source,
                    prior,
                    collector_epoch,
                    cycle_seq,
                    "source-changed-during-scan",
                ));
                continue;
            };
            consumed_budget = consumed_budget.saturating_add(result.consumed_bytes);
            if result.unresolved {
                pending_ranges = pending_ranges.saturating_add(result.pending.len().max(1));
            }
            if result.changed {
                checkpoints.push(result.checkpoint);
            }
            if let Some(range) = result.range {
                ranges.push(range);
            }
            durable_events.extend(result.events);
            pending_evidence.extend(result.pending);
        }

        let timeline_recovery = if !period_restarted && period_available {
            if let Some(identity) = self.identity.as_ref() {
                build_timeline_recovery(
                    &events,
                    reset_at,
                    window_seconds,
                    timeline_end,
                    &state,
                    &ranges,
                    collector_epoch,
                    cycle_seq,
                    &totals,
                    identity,
                )?
            } else {
                None
            }
        } else {
            None
        };
        let mut history_events = replayed_events;
        history_events.extend(
            events
                .iter()
                .filter(|event| event.timestamp <= timeline_end)
                .cloned(),
        );
        let history_initial_totals = if period_restarted {
            ModelTotals::default()
        } else {
            initial_totals
        };
        let (samples, history_models) = if timeline_recovery.is_some() || !period_available {
            (Vec::new(), BTreeMap::new())
        } else {
            build_history(
                &history_events,
                reset_at,
                history_initial_totals,
                pending_ranges == 0,
            )
        };
        let quota_sample = quota.as_ref().and_then(|candidate| {
            admitted_quota.as_ref().and_then(|(_, canonical_reset, _)| {
                candidate.remaining_percent.map(|remaining| {
                    totals.history_sample(candidate.observed_at, *canonical_reset, remaining)
                })
            })
        });
        let mut samples = samples;
        if let Some(sample) = quota_sample {
            samples.push(sample);
        }
        let observations = samples
            .iter()
            .map(|sample| {
                let models = if quota
                    .as_ref()
                    .is_some_and(|candidate| candidate.observed_at == sample.timestamp)
                {
                    totals.to_totals()
                } else {
                    history_models
                        .get(&sample.timestamp)
                        .cloned()
                        .unwrap_or_default()
                };
                UsageHistoryObservation::confirmed_with_models(sample, models)
            })
            .collect::<Vec<_>>();
        let model_totals = totals.to_totals();
        self.pending = Some(PendingBatch {
            reset_at,
            window_seconds,
            collector_epoch,
            cycle_seq,
            accepted_ranges: ranges.len(),
            sources_seen: sources.len(),
            samples,
            observations,
            checkpoints,
            ranges,
            durable_events,
            pending_evidence,
            model_totals,
            timeline_recovery,
        });
        self.commit_pending().map(Some)
    }

    pub fn into_writer(self) -> UsageStore {
        self.writer
    }

    fn commit_pending(&mut self) -> Result<CycleReport, RecorderError> {
        let pending = self.pending.as_ref().ok_or_else(|| {
            RecorderError::Invalid("recorder pending batch is missing".to_owned())
        })?;
        let commit = SessionCollectionCommit {
            reset_at: pending.reset_at,
            window_seconds: pending.window_seconds,
            collector_epoch: pending.collector_epoch,
            cycle_seq: pending.cycle_seq,
            samples: &pending.samples,
            checkpoints: &pending.checkpoints,
            ranges: &pending.ranges,
            model_totals: &pending.model_totals,
            recorded_sessions: &[],
        };
        let result = if let Some(recovery) = pending.timeline_recovery.as_ref() {
            self.writer
                .commit_session_collection_with_timeline_recovery_events_and_pending_ranges(
                    commit,
                    &pending.observations,
                    recovery,
                    &pending.durable_events,
                    &pending.pending_evidence,
                )?
        } else {
            self.writer
                .commit_session_collection_with_events_and_pending_ranges(
                    commit,
                    &pending.observations,
                    &pending.durable_events,
                    &pending.pending_evidence,
                )?
        };
        let read_back = self.writer.load_session_collection_state()?;
        if read_back.data_generation != result.data_generation
            || read_back.collector_epoch != Some(pending.collector_epoch)
            || read_back.cycle_seq != pending.cycle_seq
            || read_back.model_totals != pending.model_totals
            || pending.checkpoints.iter().any(|expected| {
                !read_back
                    .checkpoints
                    .iter()
                    .any(|actual| actual == expected)
            })
            || !self.writer.verify_session_collection_batch(
                &pending.ranges,
                &pending.durable_events,
                &pending.pending_evidence,
            )?
        {
            return Err(RecorderError::Invalid(
                "session collection read-back did not match committed batch".to_owned(),
            ));
        }
        let durable_pending_count = self.writer.session_pending_range_count()?;
        let report = CycleReport {
            generation: result.data_generation,
            accepted_ranges: pending.accepted_ranges,
            pending_ranges: durable_pending_count,
            sources_seen: pending.sources_seen,
        };
        self.last_ranges = pending.ranges.clone();
        self.pending = None;
        Ok(report)
    }
}

fn admit_quota_period(
    state: &SessionCollectionState,
    candidate: &QuotaSnapshot,
) -> Option<(QuotaTransition, i64, i64)> {
    if candidate.reset_at <= 0 || candidate.window_seconds <= 0 || candidate.observed_at <= 0 {
        eprintln!(
            "recorder degraded: quota response has an invalid period reset_at={} window_seconds={} observed_at={}",
            candidate.reset_at, candidate.window_seconds, candidate.observed_at
        );
        return None;
    }
    let Some(remaining_percent) = candidate.remaining_percent else {
        eprintln!("recorder degraded: quota response has no bounded remaining percentage");
        return None;
    };
    if !remaining_percent.is_finite() || !(0.0..=100.0).contains(&remaining_percent) {
        eprintln!("recorder degraded: quota response has an invalid remaining percentage");
        return None;
    }
    let canonical_reset_at = if state.reset_at > 0 {
        canonical_reset_period(state.reset_at, candidate.reset_at).unwrap_or(candidate.reset_at)
    } else {
        candidate.reset_at
    };
    let previous_reset_at =
        (state.data_generation > 0 && state.reset_at > 0).then_some(state.reset_at);
    let previous_observed_at = state
        .last_quota_observation
        .as_ref()
        .map(|observation| observation.observed_at);
    let transition = classify_quota_transition(
        previous_reset_at,
        state.window_seconds,
        previous_observed_at,
        canonical_reset_at,
        candidate.window_seconds,
        Some(remaining_percent),
        candidate.observed_at,
    );
    if transition == QuotaTransition::Rejected {
        eprintln!(
            "recorder degraded: quota period candidate rejected reset_at={} observed_at={}",
            candidate.reset_at, candidate.observed_at
        );
        return None;
    }
    let (reset_at, window_seconds) = if transition == QuotaTransition::SamePeriod {
        (state.reset_at, state.window_seconds)
    } else {
        (canonical_reset_at, candidate.window_seconds)
    };
    Some((transition, reset_at, window_seconds))
}

struct Source {
    path: PathBuf,
    recorded: RecordedSessionSource,
}

struct InventoryFailure {
    relative_path: String,
    reason: String,
}

struct SourceInventory {
    root_identity: String,
    sources: Vec<Source>,
    failures: Vec<InventoryFailure>,
}

fn fallback_root_identity(root: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-info-session-root-fallback-v1\0");
    hasher.update(root.to_string_lossy().as_bytes());
    hex_digest(hasher.finalize().as_slice())
}

fn pending_inventory_failure(
    root_identity: String,
    failure: InventoryFailure,
    collector_epoch: u128,
    cycle_seq: u64,
) -> SessionPendingRange {
    let source = RecordedSessionSource {
        root_identity: root_identity.clone(),
        relative_path: failure.relative_path.clone(),
        file_bytes: 0,
        modified_nanos: 0,
        file_device: 0,
        file_inode: 0,
    };
    SessionPendingRange {
        root_identity,
        relative_path: failure.relative_path,
        file_device: 0,
        file_inode: 0,
        start_offset: 0,
        end_offset: 0,
        collector_epoch,
        cycle_seq,
        prefix_generation: prefix_generation(collector_epoch, &source, EMPTY_SHA256),
        record_sha256: EMPTY_SHA256.to_owned(),
        parser_version: PARSER_VERSION.to_owned(),
        reason: failure.reason,
        complete: false,
    }
}

fn pending_source_issue(
    source: &Source,
    prior: Option<&SessionCheckpoint>,
    collector_epoch: u128,
    cycle_seq: u64,
    reason: &'static str,
) -> SessionPendingRange {
    let resumable = prior.filter(|checkpoint| checkpoint_can_resume_offset(checkpoint, source));
    let start_offset = resumable.map_or(0, |checkpoint| checkpoint.committed_offset);
    let prefix_generation = resumable.map_or_else(
        || prefix_generation(collector_epoch, &source.recorded, EMPTY_SHA256),
        |checkpoint| checkpoint.prefix_generation,
    );
    SessionPendingRange {
        root_identity: source.recorded.root_identity.clone(),
        relative_path: source.recorded.relative_path.clone(),
        file_device: source.recorded.file_device,
        file_inode: source.recorded.file_inode,
        start_offset,
        end_offset: start_offset,
        collector_epoch,
        cycle_seq,
        prefix_generation,
        record_sha256: EMPTY_SHA256.to_owned(),
        parser_version: PARSER_VERSION.to_owned(),
        reason: reason.to_owned(),
        complete: false,
    }
}

struct SourceOutcome {
    checkpoint: SessionCheckpoint,
    range: Option<SessionRange>,
    events: Vec<SessionEvent>,
    pending: Vec<SessionPendingRange>,
    unresolved: bool,
    changed: bool,
    consumed_bytes: u64,
}

fn discover_sources(root: &Path) -> Result<SourceInventory, RecorderError> {
    let root = match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(RecorderError::Invalid(
                "sessions root must be a regular directory".to_owned(),
            ))
        }
        Ok(_) => root.canonicalize()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(RecorderError::Invalid(
                "sessions root does not exist".to_owned(),
            ))
        }
        Err(error) => return Err(error.into()),
    };
    let root_metadata = fs::metadata(&root)?;
    let root_identity = root_identity(&root, &root_metadata);
    let mut paths = Vec::new();
    let mut failures = Vec::new();
    collect_jsonl(&root, &mut paths, &mut failures)?;
    paths.sort();
    let mut sources = Vec::with_capacity(paths.len());
    for path in paths {
        let relative = path
            .strip_prefix(&root)
            .map_err(|_| RecorderError::Invalid("session path escaped root".to_owned()))?;
        let relative_path = relative.to_string_lossy().replace('\\', "/");
        if relative_path.is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            failures.push(InventoryFailure {
                relative_path: "__inventory__".to_owned(),
                reason: "invalid-session-relative-path".to_owned(),
            });
            continue;
        }
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                failures.push(InventoryFailure {
                    relative_path,
                    reason: format!("source-stat-failed: {error}"),
                });
                continue;
            }
        };
        sources.push(Source {
            path,
            recorded: recorded_source(root_identity.clone(), relative_path, metadata),
        });
    }
    Ok(SourceInventory {
        root_identity,
        sources,
        failures,
    })
}

fn collect_jsonl(
    path: &Path,
    output: &mut Vec<PathBuf>,
    failures: &mut Vec<InventoryFailure>,
) -> Result<(), RecorderError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            failures.push(InventoryFailure {
                relative_path: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .unwrap_or("__inventory__")
                    .to_owned(),
                reason: format!("inventory-stat-failed: {error}"),
            });
            return Ok(());
        }
    };
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            output.push(path.to_owned());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut entries = match fs::read_dir(path) {
        Ok(entries) => entries
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    failures.push(InventoryFailure {
                        relative_path: "__inventory__".to_owned(),
                        reason: format!("directory-entry-read-failed: {error}"),
                    });
                    None
                }
            })
            .collect::<Vec<_>>(),
        Err(error) => {
            failures.push(InventoryFailure {
                relative_path: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .unwrap_or("__inventory__")
                    .to_owned(),
                reason: format!("directory-read-failed: {error}"),
            });
            return Ok(());
        }
    };
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if let Err(error) = collect_jsonl(&entry.path(), output, failures) {
            eprintln!(
                "recorder degraded: session source {} could not be inventoried: {error}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn root_identity(root: &Path, metadata: &Metadata) -> String {
    #[cfg(unix)]
    {
        let _ = root;
        format!("unix:{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in root.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("path-fnv1a64:{hash:016x}")
    }
}

fn recorded_source(root: String, relative: String, metadata: Metadata) -> RecordedSessionSource {
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_nanos());
    #[cfg(unix)]
    let (file_device, file_inode) = (metadata.dev(), metadata.ino());
    #[cfg(not(unix))]
    let (file_device, file_inode) = (0, 0);
    RecordedSessionSource {
        root_identity: root,
        relative_path: relative,
        file_bytes: metadata.len(),
        modified_nanos,
        file_device,
        file_inode,
    }
}

fn prior_checkpoint<'a>(
    checkpoints: &'a [SessionCheckpoint],
    source: &Source,
) -> Option<&'a SessionCheckpoint> {
    let exact = checkpoints
        .iter()
        .filter(|checkpoint| {
            checkpoint.root_identity == source.recorded.root_identity
                && checkpoint.relative_path == source.recorded.relative_path
                && checkpoint.file_device == source.recorded.file_device
                && checkpoint.file_inode == source.recorded.file_inode
        })
        .max_by_key(|checkpoint| checkpoint.cycle_seq);
    exact.or_else(|| {
        checkpoints
            .iter()
            .filter(|checkpoint| {
                checkpoint.root_identity == source.recorded.root_identity
                    && checkpoint.relative_path == source.recorded.relative_path
            })
            .max_by_key(|checkpoint| checkpoint.cycle_seq)
    })
}

fn checkpoint_covers_observed_end(prior: Option<&SessionCheckpoint>, source: &Source) -> bool {
    prior.is_some_and(|checkpoint| {
        checkpoint_can_resume_offset(checkpoint, source)
            && checkpoint.committed_offset == source.recorded.file_bytes
    })
}

fn checkpoint_can_resume_offset(checkpoint: &SessionCheckpoint, source: &Source) -> bool {
    checkpoint.root_identity == source.recorded.root_identity
        && checkpoint.relative_path == source.recorded.relative_path
        && checkpoint.file_device == source.recorded.file_device
        && checkpoint.file_inode == source.recorded.file_inode
        && checkpoint.committed_offset <= source.recorded.file_bytes
}

#[allow(clippy::too_many_arguments)]
fn scan_source(
    source: &Source,
    prior: Option<&SessionCheckpoint>,
    baseline_existing: bool,
    collector_epoch: u128,
    cycle_seq: u64,
    reset_at: i64,
    window_start: i64,
    max_bytes: u64,
    totals: &mut ModelTotals,
    events: &mut Vec<TimedModelUsage>,
) -> Result<Option<SourceOutcome>, RecorderError> {
    let before_path = fs::symlink_metadata(&source.path)?;
    if before_path.file_type().is_symlink() || !before_path.is_file() {
        return Ok(None);
    }
    let before_file = fs::metadata(&source.path)?;
    if before_file.len() != source.recorded.file_bytes
        || file_identity(&before_file) != (source.recorded.file_device, source.recorded.file_inode)
    {
        return Ok(None);
    }
    // A byte offset is valid only for the same physical file. If a restore
    // changes inode, scan from zero and re-synchronize on the durable token
    // vector below instead of guessing that the old byte boundary survived.
    let continuous = prior.filter(|checkpoint| {
        checkpoint.root_identity == source.recorded.root_identity
            && checkpoint.relative_path == source.recorded.relative_path
            && checkpoint.file_device == source.recorded.file_device
            && checkpoint.file_inode == source.recorded.file_inode
            && checkpoint.committed_offset <= before_file.len()
    });
    let recovery_anchor = prior
        .filter(|_| continuous.is_none())
        .map(|checkpoint| TokenSnapshot {
            total: checkpoint.previous_total,
            input: checkpoint.previous_input,
            cached_input: checkpoint.previous_cached_input,
            output: checkpoint.previous_output,
            cache_write_input: checkpoint.previous_cache_write_input,
        });
    let mut recovery_anchor_found = recovery_anchor.is_none();
    let (
        start_offset,
        mut discard_until_lf,
        mut fully_attributed,
        mut baseline_known,
        mut last_model,
        mut last_task_running,
        mut previous,
        mut prefix_generation_value,
        mut prefix_sha256,
    ) = if let Some(checkpoint) = continuous {
        (
            checkpoint.committed_offset,
            checkpoint.discard_until_lf,
            checkpoint.fully_attributed_from_zero,
            checkpoint.token_baseline_known,
            checkpoint.last_model.clone(),
            checkpoint.last_task_running,
            TokenSnapshot {
                total: checkpoint.previous_total,
                input: checkpoint.previous_input,
                cached_input: checkpoint.previous_cached_input,
                output: checkpoint.previous_output,
                cache_write_input: checkpoint.previous_cache_write_input,
            },
            checkpoint.prefix_generation,
            checkpoint.prefix_sha256.clone(),
        )
    } else {
        (
            0,
            false,
            prior.is_none() && !baseline_existing,
            prior.is_none() && !baseline_existing,
            None,
            None,
            TokenSnapshot::default(),
            prefix_generation(collector_epoch, &source.recorded, EMPTY_SHA256),
            EMPTY_SHA256.to_owned(),
        )
    };
    if start_offset > before_file.len() {
        return Ok(None);
    }

    let resumed_partial = discard_until_lf;
    let mut reader = BufReader::new(File::open(&source.path)?);
    reader.seek(SeekFrom::Start(start_offset))?;
    let admitted_start = start_offset;
    let mut physical_offset = start_offset;
    let mut consumed_bytes = 0_u64;
    let mut unresolved = false;
    let mut unresolved_start = None;
    let mut pending_evidence = Vec::<(u64, u64, &'static str, bool)>::new();
    let mut candidate_totals = totals.clone();
    let mut candidate_events = Vec::new();
    // Keep every source-proven timestamped delta in the event ledger even
    // when the last quota authority cannot cover its period.  `candidate_events`
    // is only the currently materialized history projection; this second
    // vector is what lets a later reset boundary reconstruct outage-spanning
    // usage without rereading or double-adding checkpoints.
    let mut all_candidate_events = Vec::new();
    let mut recovery_stream_previous = None;
    let mut recovery_stream_events = Vec::new();
    let mut recovery_stream_proven = true;
    let mut recovery_last = None;
    let mut read_any = false;
    loop {
        if consumed_bytes >= max_bytes && read_any {
            break;
        }
        let record_start = physical_offset;
        let record = read_streaming_record(&mut reader)?;
        let (summary, bytes) = match record {
            RecordRead::End => break,
            RecordRead::Present(summary, bytes) => (summary, bytes),
            RecordRead::Invalid(bytes, terminated) => {
                consumed_bytes = consumed_bytes.saturating_add(bytes);
                unresolved = true;
                if terminated {
                    // A complete malformed line is exact source evidence.
                    // Keep its bytes in this contiguous range and continue
                    // so later records cannot be stranded behind a poison
                    // line forever.
                    physical_offset = physical_offset.saturating_add(bytes);
                    read_any = true;
                    discard_until_lf = false;
                    pending_evidence.push((
                        record_start,
                        physical_offset,
                        "malformed-json-record",
                        true,
                    ));
                    continue;
                }
                let pending_end = record_start.saturating_add(bytes);
                pending_evidence.push((
                    record_start,
                    pending_end,
                    "unterminated-malformed-json-record",
                    false,
                ));
                unresolved_start = Some(record_start);
                discard_until_lf = true;
                break;
            }
            RecordRead::Unterminated(bytes) => {
                consumed_bytes = consumed_bytes.saturating_add(bytes);
                unresolved = true;
                let pending_end = record_start.saturating_add(bytes);
                pending_evidence.push((record_start, pending_end, "partial-json-record", false));
                unresolved_start = Some(record_start);
                discard_until_lf = true;
                break;
            }
        };
        read_any = true;
        consumed_bytes = consumed_bytes.saturating_add(bytes);
        physical_offset = physical_offset.saturating_add(bytes);
        if summary.usage_unparsed() {
            unresolved = true;
            // This is a complete line with an untrusted usage shape. Admit
            // its exact bytes, but do not stop subsequent safe records.
            discard_until_lf = false;
            pending_evidence.push((
                record_start,
                physical_offset,
                "usage-payload-unparsed",
                true,
            ));
            continue;
        }
        if let Some(running) = summary.task_running() {
            last_task_running = Some(running);
        }
        if summary.event_type() != Some("thread_settings_applied") || last_model.is_none() {
            if let Some(model) = summary.event_model() {
                last_model = ModelTotals::checkpoint_model(model);
            }
        }
        let Some(current) = summary.token_snapshot() else {
            continue;
        };
        if !current.valid() {
            unresolved = true;
            // Invalid token components are retained as source evidence while
            // arithmetic remains fail-closed; later records are still read.
            pending_evidence.push((
                record_start,
                physical_offset,
                "invalid-token-components",
                true,
            ));
            continue;
        }
        if let Some(anchor) = recovery_anchor.filter(|_| !recovery_anchor_found) {
            baseline_known = true;
            previous = current;
            let timestamp = summary.event_timestamp();
            let model = ModelTotals::usage_model(last_model.as_deref());
            recovery_last = Some((current, timestamp));
            if current != anchor {
                if let Some(before) = recovery_stream_previous {
                    match current.checked_delta_from(before) {
                        Some(delta) if timestamp > 0 && delta.has_usage() => {
                            recovery_stream_events.push(TimedModelUsage {
                                timestamp,
                                model,
                                delta,
                            });
                        }
                        Some(_) => {}
                        None => {
                            recovery_stream_proven = false;
                            // Values before a reset or an incomparable token
                            // shape may overlap the old physical file. Only
                            // the new, internally monotonic segment after this
                            // boundary is safe to reconstruct.
                            recovery_stream_events.clear();
                        }
                    }
                }
                recovery_stream_previous = Some(current);
                continue;
            }
            recovery_anchor_found = true;
            recovery_stream_events.clear();
            continue;
        }
        if !baseline_known {
            previous = current;
            baseline_known = true;
            continue;
        }
        if current.total < previous.total
            || current.input < previous.input
            || current.cached_input < previous.cached_input
            || current.output < previous.output
            || matches!(
                (current.cache_write_input, previous.cache_write_input),
                (Some(current), Some(previous)) if current < previous
            )
        {
            previous = current;
            continue;
        }
        let delta = TokenSnapshot {
            total: current.total - previous.total,
            input: current.input - previous.input,
            cached_input: current.cached_input - previous.cached_input,
            output: current.output - previous.output,
            cache_write_input: current.cache_write_delta_from(previous),
        };
        previous = current;
        let timestamp = summary.event_timestamp();
        let model = ModelTotals::usage_model(last_model.as_deref());
        let timed_event = (timestamp > 0 && delta.has_usage()).then(|| TimedModelUsage {
            timestamp,
            model: model.clone(),
            delta,
        });
        if let Some(event) = timed_event.as_ref() {
            all_candidate_events.push(event.clone());
        }
        if reset_at == 0 {
            // Quota authority is not available yet.  Keep the source-derived
            // delta as an event, but do not invent a period or materialize
            // model totals/history until a valid reset/window is admitted.
            if let Some(event) = timed_event {
                candidate_events.push(event);
            }
            continue;
        }
        if timestamp < window_start || timestamp > reset_at {
            continue;
        }
        candidate_totals.add(&model, delta)?;
        // Session timestamps can be slightly ahead of the local cycle clock.
        // Keep every source-authoritative delta paired with the total applied
        // by this transaction; display projection excludes future points.
        if let Some(event) = timed_event {
            candidate_events.push(event);
        }
    }

    let end_offset = unresolved_start.unwrap_or(physical_offset);
    let after_file = reader.get_ref().metadata()?;
    let after_path = fs::symlink_metadata(&source.path)?;
    if after_path.file_type().is_symlink()
        || !after_path.is_file()
        || file_identity(&after_file) != file_identity(&before_file)
        || after_file.len() < end_offset
    {
        return Ok(None);
    }
    if let Some(anchor) = recovery_anchor.filter(|_| !recovery_anchor_found) {
        unresolved = true;
        pending_evidence.push((
            admitted_start,
            end_offset,
            "checkpoint-token-anchor-missing",
            true,
        ));
        let recovered_events = if recovery_stream_proven {
            recovery_last
                .and_then(|(last, timestamp)| {
                    last.checked_delta_from(anchor).and_then(|delta| {
                        (timestamp > 0 && delta.has_usage()).then(|| {
                            vec![TimedModelUsage {
                                timestamp,
                                model: UNATTRIBUTED_MODEL.to_owned(),
                                delta,
                            }]
                        })
                    })
                })
                // If the replacement starts after a proven counter reset,
                // its endpoint cannot be joined to the old anchor. Preserve
                // only deltas proven inside the new monotonic stream.
                .unwrap_or(recovery_stream_events)
        } else {
            recovery_stream_events
        };
        for event in recovered_events {
            all_candidate_events.push(event.clone());
            if reset_at == 0 {
                candidate_events.push(event);
            } else if event.timestamp >= window_start && event.timestamp <= reset_at {
                candidate_totals.add(&event.model, event.delta)?;
                candidate_events.push(event);
            }
        }
    }
    let accepted_digest = (end_offset > admitted_start)
        .then(|| sha256_file_range(&source.path, admitted_start, end_offset))
        .transpose()?;
    if admitted_start == 0 {
        prefix_sha256 = accepted_digest
            .clone()
            .unwrap_or_else(|| EMPTY_SHA256.to_owned());
        prefix_generation_value =
            prefix_generation(collector_epoch, &source.recorded, &prefix_sha256);
    }
    if !unresolved
        && !discard_until_lf
        && end_offset == after_file.len()
        && (fully_attributed || admitted_start == 0 || resumed_partial)
    {
        fully_attributed = true;
    }
    if unresolved {
        fully_attributed = false;
    }
    if !unresolved {
        discard_until_lf = false;
    }
    let checkpoint = SessionCheckpoint {
        root_identity: source.recorded.root_identity.clone(),
        relative_path: source.recorded.relative_path.clone(),
        file_device: source.recorded.file_device,
        file_inode: source.recorded.file_inode,
        committed_offset: end_offset,
        discard_until_lf,
        collector_epoch,
        cycle_seq,
        prefix_generation: prefix_generation_value,
        prefix_sha256,
        fully_attributed_from_zero: fully_attributed,
        token_baseline_known: baseline_known,
        last_model,
        last_task_running,
        previous_total: previous.total,
        previous_input: previous.input,
        previous_cached_input: previous.cached_input,
        previous_output: previous.output,
        previous_cache_write_input: previous.cache_write_input,
    };
    let range = (end_offset > admitted_start).then(|| SessionRange {
        root_identity: checkpoint.root_identity.clone(),
        relative_path: checkpoint.relative_path.clone(),
        file_device: checkpoint.file_device,
        file_inode: checkpoint.file_inode,
        start_offset: admitted_start,
        end_offset,
        collector_epoch,
        cycle_seq,
        prefix_generation: checkpoint.prefix_generation,
        record_sha256: accepted_digest.expect("accepted range has a digest"),
    });
    let durable_events = if let Some(range) = range.as_ref() {
        all_candidate_events
            .iter()
            .cloned()
            .enumerate()
            .map(|(event_index, event)| {
                Ok(SessionEvent {
                    root_identity: range.root_identity.clone(),
                    relative_path: range.relative_path.clone(),
                    file_device: range.file_device,
                    file_inode: range.file_inode,
                    prefix_generation: range.prefix_generation,
                    range_start: range.start_offset,
                    range_end: range.end_offset,
                    record_sha256: range.record_sha256.clone(),
                    event_index: u64::try_from(event_index).map_err(|_| {
                        RecorderError::Invalid("session event index overflow".to_owned())
                    })?,
                    timestamp: event.timestamp,
                    model: event.model,
                    total_tokens: event.delta.total,
                    input_tokens: event.delta.input,
                    cached_input_tokens: event.delta.cached_input,
                    output_tokens: event.delta.output,
                    cache_write_input_tokens: event.delta.cache_write_input,
                })
            })
            .collect::<Result<Vec<_>, RecorderError>>()?
    } else if candidate_events.is_empty() {
        Vec::new()
    } else {
        return Err(RecorderError::Invalid(
            "session events have no accepted source range".to_owned(),
        ));
    };
    let pending = pending_evidence
        .into_iter()
        .map(|(start_offset, end_offset, reason, complete)| {
            let record_sha256 = if start_offset == end_offset {
                EMPTY_SHA256.to_owned()
            } else {
                sha256_file_range(&source.path, start_offset, end_offset)?
            };
            Ok(SessionPendingRange {
                root_identity: checkpoint.root_identity.clone(),
                relative_path: checkpoint.relative_path.clone(),
                file_device: checkpoint.file_device,
                file_inode: checkpoint.file_inode,
                start_offset,
                end_offset,
                collector_epoch,
                cycle_seq,
                prefix_generation: checkpoint.prefix_generation,
                record_sha256,
                parser_version: PARSER_VERSION.to_owned(),
                reason: reason.to_owned(),
                complete,
            })
        })
        .collect::<Result<Vec<_>, RecorderError>>()?;
    *totals = candidate_totals;
    events.extend(candidate_events);
    let changed = prior.is_none_or(|old| !same_checkpoint_content(old, &checkpoint));
    Ok(Some(SourceOutcome {
        checkpoint,
        range,
        events: durable_events,
        pending,
        unresolved,
        changed,
        consumed_bytes,
    }))
}

fn file_identity(metadata: &Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        (metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        (0, 0)
    }
}

fn same_checkpoint_content(left: &SessionCheckpoint, right: &SessionCheckpoint) -> bool {
    left.root_identity == right.root_identity
        && left.relative_path == right.relative_path
        && left.file_device == right.file_device
        && left.file_inode == right.file_inode
        && left.committed_offset == right.committed_offset
        && left.discard_until_lf == right.discard_until_lf
        && left.prefix_generation == right.prefix_generation
        && left.prefix_sha256 == right.prefix_sha256
        && left.fully_attributed_from_zero == right.fully_attributed_from_zero
        && left.token_baseline_known == right.token_baseline_known
        && left.last_model == right.last_model
        && left.last_task_running == right.last_task_running
        && left.previous_total == right.previous_total
        && left.previous_input == right.previous_input
        && left.previous_cached_input == right.previous_cached_input
        && left.previous_output == right.previous_output
        && left.previous_cache_write_input == right.previous_cache_write_input
}

fn sha256_file_range(path: &Path, start: u64, end: u64) -> Result<String, RecorderError> {
    if start > end || end > i64::MAX as u64 {
        return Err(RecorderError::Invalid(
            "session range exceeds SQLite offset".to_owned(),
        ));
    }
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut remaining = end - start;
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = Sha256::new();
    while remaining > 0 {
        let size = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..size])?;
        if read == 0 {
            return Err(RecorderError::Invalid(
                "session range ended before the source file".to_owned(),
            ));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(hex_digest(hasher.finalize().as_slice()))
}

fn prefix_generation(
    collector_epoch: u128,
    source: &RecordedSessionSource,
    prefix_sha256: &str,
) -> u128 {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-info-session-prefix-v1\0");
    hasher.update(collector_epoch.to_be_bytes());
    hasher.update(source.root_identity.as_bytes());
    hasher.update([0]);
    hasher.update(source.relative_path.as_bytes());
    hasher.update(source.file_device.to_be_bytes());
    hasher.update(source.file_inode.to_be_bytes());
    hasher.update(prefix_sha256.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(bytes).max(1)
}

fn collector_epoch(root: &Path, database: Option<&Path>, state: &SessionCollectionState) -> u128 {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-info-session-collector-epoch-v1\0");
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update([0]);
    if let Some(database) = database {
        hasher.update(database.to_string_lossy().as_bytes());
    }
    hasher.update(state.data_generation.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(bytes).max(1)
}

fn build_history(
    events: &[TimedModelUsage],
    reset_at: i64,
    mut totals: ModelTotals,
    complete: bool,
) -> (
    Vec<UsageHistorySample>,
    BTreeMap<i64, Vec<SessionModelTotal>>,
) {
    let mut ordered = events.to_vec();
    ordered.sort_by_key(|event| event.timestamp);
    let mut by_minute = BTreeMap::<i64, (UsageHistorySample, Vec<SessionModelTotal>)>::new();
    for event in ordered {
        if totals.add(&event.model, event.delta).is_err() {
            continue;
        }
        let minute = event.timestamp.div_euclid(60) * 60;
        if minute <= 0 {
            continue;
        }
        let dollars = totals.dollar_totals();
        let tokens = totals.token_totals();
        let sample = UsageHistorySample {
            timestamp: minute,
            reset_at,
            remaining_percent: None,
            sol_dollars: dollars.0,
            terra_dollars: dollars.1,
            luna_dollars: dollars.2,
            sol_tokens: tokens.0,
            terra_tokens: tokens.1,
            luna_tokens: tokens.2,
        };
        by_minute.insert(
            minute,
            (
                sample,
                if complete {
                    totals.to_totals()
                } else {
                    totals.positive_totals()
                },
            ),
        );
    }
    let mut samples = Vec::with_capacity(by_minute.len());
    let mut models = BTreeMap::new();
    for (minute, (sample, totals)) in by_minute {
        samples.push(sample);
        models.insert(minute, totals);
    }
    (samples, models)
}

#[allow(clippy::too_many_arguments)]
fn build_timeline_recovery(
    events: &[TimedModelUsage],
    reset_at: i64,
    window_seconds: i64,
    timeline_end: i64,
    state: &SessionCollectionState,
    ranges: &[SessionRange],
    collector_epoch: u128,
    cycle_seq: u64,
    collected_totals: &ModelTotals,
    identity: &StoragePartitionIdentity,
) -> Result<Option<SessionTimelineRecovery>, RecorderError> {
    if state.data_generation == 0 || ranges.is_empty() {
        return Ok(None);
    }
    let projection_end = timeline_end.div_euclid(60) * 60;
    if projection_end <= 0
        || projection_end > reset_at
        || !events.iter().any(|event| {
            event.delta.has_usage() && event.timestamp.div_euclid(60) * 60 < projection_end
        })
    {
        return Ok(None);
    }
    let source = ModelTotals::from_state(&state.model_totals);
    let mut offsets = ModelTotals::default();
    let mut ordered = events.to_vec();
    ordered.sort_by_key(|event| event.timestamp);
    let mut points = BTreeMap::<i64, Vec<SessionModelTotal>>::new();
    for event in ordered {
        offsets.add(&event.model, event.delta)?;
        let minute = event.timestamp.div_euclid(60) * 60;
        if minute < projection_end {
            points.insert(minute, offsets.positive_totals());
        }
    }
    if points.is_empty() {
        return Ok(None);
    }
    let final_offsets = offsets.positive_totals();
    if final_offsets.is_empty() {
        return Ok(None);
    }
    let mut expected = source.clone();
    for (model, counter) in &offsets.values {
        expected.add(model, counter.snapshot())?;
    }
    if expected.to_totals() != collected_totals.to_totals() {
        return Err(RecorderError::Invalid(
            "timeline recovery totals do not match the source append".to_owned(),
        ));
    }
    let dollars = offsets.dollar_totals();
    let final_tokens = offsets.token_totals();
    let mut recovery_points = Vec::with_capacity(points.len());
    for (timestamp, model_totals) in points {
        let point_totals = ModelTotals::from_state(&model_totals).token_totals();
        recovery_points.push(SessionTimelineRecoveryPoint {
            timestamp,
            offset_model_totals: model_totals,
            offset_sol_dollars: weighted_dollars(point_totals.0, final_tokens.0, dollars.0),
            offset_terra_dollars: weighted_dollars(point_totals.1, final_tokens.1, dollars.1),
            offset_luna_dollars: weighted_dollars(point_totals.2, final_tokens.2, dollars.2),
        });
    }
    let mut canonical_ranges = ranges.to_vec();
    canonical_ranges.sort();
    canonical_ranges.dedup();
    if canonical_ranges.is_empty()
        || canonical_ranges
            .iter()
            .any(|range| range.collector_epoch != collector_epoch || range.cycle_seq != cycle_seq)
    {
        return Err(RecorderError::Invalid(
            "timeline recovery ranges are not canonical".to_owned(),
        ));
    }
    let recovery = SessionTimelineRecovery {
        recovery_id: String::new(),
        canonical_reset_at: reset_at,
        window_seconds,
        source_data_generation: state.data_generation,
        projection_end_exclusive: projection_end,
        source_model_totals: state.model_totals.clone(),
        ranges: canonical_ranges,
        points: recovery_points,
        final_offset_model_totals: final_offsets,
        final_offset_sol_dollars: dollars.0,
        final_offset_terra_dollars: dollars.1,
        final_offset_luna_dollars: dollars.2,
    };
    Ok(Some(finalize_session_timeline_recovery(
        &identity.partition_id,
        recovery,
    )?))
}

fn weighted_dollars(value: u64, final_value: u64, final_dollars: f64) -> f64 {
    if final_value == 0 {
        0.0
    } else if value == final_value {
        // Preserve the authoritative endpoint bit-for-bit. Multiplying by
        // the token count before dividing can round one ULP above it.
        final_dollars
    } else {
        final_dollars * (value as f64 / final_value as f64)
    }
}

enum RecordRead {
    End,
    Present(Box<SessionRecordSummary>, u64),
    Invalid(u64, bool),
    Unterminated(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionRecordParseError {
    Syntax,
    Io,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionJsonKey {
    Type,
    Timestamp,
    Model,
    Payload,
    ThreadSettings,
    Info,
    TotalTokenUsage,
    TotalTokens,
    CacheWriteInputTokens,
    InputTokens,
    CachedInputTokens,
    OutputTokens,
    Other,
}

const SESSION_JSON_KEYS: [(&[u8], SessionJsonKey); 12] = [
    (b"type", SessionJsonKey::Type),
    (b"timestamp", SessionJsonKey::Timestamp),
    (b"model", SessionJsonKey::Model),
    (b"payload", SessionJsonKey::Payload),
    (b"thread_settings", SessionJsonKey::ThreadSettings),
    (b"info", SessionJsonKey::Info),
    (b"total_token_usage", SessionJsonKey::TotalTokenUsage),
    (b"total_tokens", SessionJsonKey::TotalTokens),
    (
        b"cache_write_input_tokens",
        SessionJsonKey::CacheWriteInputTokens,
    ),
    (b"input_tokens", SessionJsonKey::InputTokens),
    (b"cached_input_tokens", SessionJsonKey::CachedInputTokens),
    (b"output_tokens", SessionJsonKey::OutputTokens),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionJsonObject {
    Root,
    Payload,
    ThreadSettings,
    Info,
    TokenUsage,
}

#[derive(Clone, Debug, Default)]
struct TokenUsageSummary {
    total: Option<u64>,
    cache_write_input: Option<u64>,
    input: Option<u64>,
    cached_input: Option<u64>,
    output: Option<u64>,
    total_seen: bool,
    malformed: bool,
}

impl TokenUsageSummary {
    fn snapshot(&self) -> Option<TokenSnapshot> {
        Some(TokenSnapshot {
            total: self.total?,
            cache_write_input: self.cache_write_input,
            input: self.input.unwrap_or(0),
            cached_input: self.cached_input.unwrap_or(0),
            output: self.output.unwrap_or(0),
        })
    }
}

#[derive(Clone, Debug, Default)]
struct PayloadSummary {
    event_type: Option<String>,
    model: Option<String>,
    model_seen: bool,
    model_malformed: bool,
    thread_settings_seen: bool,
    thread_settings_object: bool,
    thread_settings_model: Option<String>,
    thread_settings_model_seen: bool,
    thread_settings_model_malformed: bool,
    info_seen: bool,
    info_null: bool,
    info_object: bool,
    token_usage_seen: bool,
    token_usage_object: bool,
    token_usage: TokenUsageSummary,
}

#[derive(Clone, Debug, Default)]
struct SessionRecordSummary {
    outer_type: Option<String>,
    timestamp: Option<String>,
    timestamp_seen: bool,
    timestamp_malformed: bool,
    root_model: Option<String>,
    root_model_seen: bool,
    root_model_malformed: bool,
    payload_seen: bool,
    payload_object: bool,
    payload: PayloadSummary,
}

impl SessionRecordSummary {
    fn event_type(&self) -> Option<&str> {
        match self.outer_type.as_deref() {
            Some(
                "task_started"
                | "task_complete"
                | "task_completed"
                | "turn_aborted"
                | "token_count"
                | "turn_context"
                | "thread_context"
                | "thread_settings_applied",
            ) => self.outer_type.as_deref(),
            _ => self.payload.event_type.as_deref(),
        }
    }

    fn event_model(&self) -> Option<&str> {
        let (model, malformed) = match self.event_type() {
            Some("turn_context" | "thread_context") => {
                if self.payload.model.is_some() {
                    (self.payload.model.as_deref(), self.payload.model_malformed)
                } else {
                    (self.root_model.as_deref(), self.root_model_malformed)
                }
            }
            Some("thread_settings_applied") => {
                if self.payload.thread_settings_model.is_some() {
                    (
                        self.payload.thread_settings_model.as_deref(),
                        self.payload.thread_settings_model_malformed,
                    )
                } else if self.payload.model.is_some() {
                    (self.payload.model.as_deref(), self.payload.model_malformed)
                } else {
                    (self.root_model.as_deref(), self.root_model_malformed)
                }
            }
            _ => (None, false),
        };
        if malformed || model.is_some_and(|model| model.chars().any(char::is_control)) {
            return None;
        }
        let model = model?;
        (!model.trim().is_empty()).then_some(model)
    }

    fn token_snapshot(&self) -> Option<TokenSnapshot> {
        (self.event_type() == Some("token_count") && self.payload_object)
            .then(|| self.payload.token_usage.snapshot())
            .flatten()
    }

    fn event_timestamp(&self) -> i64 {
        self.timestamp
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp())
            .unwrap_or(0)
    }

    /// A known event with a field shape that cannot be attributed safely must
    /// not become the end of an incremental checkpoint.  `info: null` is the
    /// producer's valid no-sample token-count envelope and remains a no-op.
    fn usage_unparsed(&self) -> bool {
        match self.event_type() {
            Some("token_count") => {
                if !self.payload_object || !self.payload.info_seen {
                    return true;
                }
                if self.payload.info_null {
                    return false;
                }
                let timestamp_valid = self
                    .timestamp
                    .as_deref()
                    .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                    .is_some();
                !self.payload.info_object
                    || !self.payload.token_usage_seen
                    || !self.payload.token_usage_object
                    || !self.payload.token_usage.total_seen
                    || self.payload.token_usage.malformed
                    || self.timestamp_malformed
                    || !self.timestamp_seen
                    || !timestamp_valid
            }
            Some("turn_context" | "thread_context") => self.event_model().is_none(),
            Some("thread_settings_applied") => self.event_model().is_none(),
            _ => false,
        }
    }

    fn task_running(&self) -> Option<bool> {
        match self.event_type() {
            Some("task_started") => Some(true),
            Some("task_complete" | "task_completed" | "turn_aborted") => Some(false),
            _ => None,
        }
    }
}

struct SessionRecordInput<'a, R: BufRead> {
    reader: &'a mut R,
    consumed: u64,
    saw_bytes: bool,
    terminated: bool,
    eof: bool,
    depth: usize,
}

impl<'a, R: BufRead> SessionRecordInput<'a, R> {
    fn new(reader: &'a mut R) -> Self {
        Self {
            reader,
            consumed: 0,
            saw_bytes: false,
            terminated: false,
            eof: false,
            depth: 0,
        }
    }

    fn peek_byte(&mut self) -> Result<Option<u8>, SessionRecordParseError> {
        let buffer = self
            .reader
            .fill_buf()
            .map_err(|_| SessionRecordParseError::Io)?;
        if buffer.is_empty() {
            self.eof = true;
            return Ok(None);
        }
        if buffer[0] == b'\n' {
            return Ok(None);
        }
        Ok(Some(buffer[0]))
    }

    fn next_byte(&mut self) -> Result<Option<u8>, SessionRecordParseError> {
        let buffer = self
            .reader
            .fill_buf()
            .map_err(|_| SessionRecordParseError::Io)?;
        if buffer.is_empty() {
            self.eof = true;
            return Ok(None);
        }
        if buffer[0] == b'\n' {
            self.consume(1);
            self.terminated = true;
            return Ok(None);
        }
        let byte = buffer[0];
        self.consume(1);
        self.saw_bytes = true;
        Ok(Some(byte))
    }

    fn consume(&mut self, amount: usize) {
        self.reader.consume(amount);
        self.consumed = self.consumed.saturating_add(amount as u64);
    }

    fn next_required(&mut self) -> Result<u8, SessionRecordParseError> {
        self.next_byte()?.ok_or(SessionRecordParseError::Syntax)
    }

    fn consume_if(&mut self, expected: u8) -> Result<bool, SessionRecordParseError> {
        if self.peek_byte()? == Some(expected) {
            let _ = self.next_byte()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn skip_whitespace(&mut self) -> Result<(), SessionRecordParseError> {
        while matches!(self.peek_byte()?, Some(b' ' | b'\t' | b'\r')) {
            let _ = self.next_byte()?;
        }
        Ok(())
    }

    fn enter_container(&mut self) -> Result<(), SessionRecordParseError> {
        // serde_json's default recursion limit is 128. Keep the same
        // structural guard while allowing record size itself to remain
        // unbounded for streamed payloads.
        if self.depth >= 128 {
            return Err(SessionRecordParseError::Syntax);
        }
        self.depth += 1;
        Ok(())
    }

    fn leave_container(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn parse_record(&mut self) -> Result<SessionRecordSummary, SessionRecordParseError> {
        self.skip_whitespace()?;
        let mut summary = SessionRecordSummary::default();
        match self.peek_byte()? {
            Some(b'{') => self.parse_object(SessionJsonObject::Root, &mut summary)?,
            Some(b'[') => self.skip_array()?,
            Some(_) => self.skip_value()?,
            None => return Err(SessionRecordParseError::Syntax),
        }
        self.skip_whitespace()?;
        if self.peek_byte()?.is_some() {
            return Err(SessionRecordParseError::Syntax);
        }
        Ok(summary)
    }

    fn parse_object(
        &mut self,
        object: SessionJsonObject,
        summary: &mut SessionRecordSummary,
    ) -> Result<(), SessionRecordParseError> {
        self.enter_container()?;
        let result = (|| {
            if self.next_required()? != b'{' {
                return Err(SessionRecordParseError::Syntax);
            }
            self.skip_whitespace()?;
            if self.consume_if(b'}')? {
                return Ok(());
            }
            loop {
                let key = self.parse_key()?;
                self.skip_whitespace()?;
                if self.next_required()? != b':' {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_whitespace()?;
                self.parse_object_value(object, key, summary)?;
                self.skip_whitespace()?;
                if self.consume_if(b'}')? {
                    return Ok(());
                }
                if !self.consume_if(b',')? {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_whitespace()?;
                if self.peek_byte()? == Some(b'}') {
                    return Err(SessionRecordParseError::Syntax);
                }
            }
        })();
        self.leave_container();
        result
    }

    fn parse_object_value(
        &mut self,
        object: SessionJsonObject,
        key: SessionJsonKey,
        summary: &mut SessionRecordSummary,
    ) -> Result<(), SessionRecordParseError> {
        match object {
            SessionJsonObject::Root => match key {
                SessionJsonKey::Type => {
                    summary.outer_type = self.parse_text_value()?.0;
                }
                SessionJsonKey::Timestamp => {
                    summary.timestamp_seen = true;
                    let (value, malformed) = self.parse_text_value()?;
                    summary.timestamp = value;
                    summary.timestamp_malformed = malformed;
                }
                SessionJsonKey::Model => {
                    summary.root_model_seen = true;
                    let (value, malformed) = self.parse_text_value()?;
                    summary.root_model = value;
                    summary.root_model_malformed = malformed;
                }
                SessionJsonKey::Payload => {
                    summary.payload_seen = true;
                    summary.payload = PayloadSummary::default();
                    if self.peek_byte()? == Some(b'{') {
                        summary.payload_object = true;
                        self.parse_object(SessionJsonObject::Payload, summary)?;
                    } else {
                        summary.payload_object = false;
                        self.skip_value()?;
                    }
                }
                _ => self.skip_value()?,
            },
            SessionJsonObject::Payload => match key {
                SessionJsonKey::Type => {
                    summary.payload.event_type = self.parse_text_value()?.0;
                }
                SessionJsonKey::Model => {
                    summary.payload.model_seen = true;
                    let (value, malformed) = self.parse_text_value()?;
                    summary.payload.model = value;
                    summary.payload.model_malformed = malformed;
                }
                SessionJsonKey::ThreadSettings => {
                    summary.payload.thread_settings_seen = true;
                    summary.payload.thread_settings_model = None;
                    summary.payload.thread_settings_model_seen = false;
                    summary.payload.thread_settings_model_malformed = false;
                    if self.peek_byte()? == Some(b'{') {
                        summary.payload.thread_settings_object = true;
                        self.parse_object(SessionJsonObject::ThreadSettings, summary)?;
                    } else {
                        summary.payload.thread_settings_object = false;
                        self.skip_value()?;
                    }
                }
                SessionJsonKey::Info => {
                    summary.payload.info_seen = true;
                    summary.payload.info_null = self.peek_byte()? == Some(b'n');
                    summary.payload.info_object = false;
                    summary.payload.token_usage_seen = false;
                    summary.payload.token_usage_object = false;
                    summary.payload.token_usage = TokenUsageSummary::default();
                    if self.peek_byte()? == Some(b'{') {
                        summary.payload.info_object = true;
                        self.parse_object(SessionJsonObject::Info, summary)?;
                    } else {
                        self.skip_value()?;
                    }
                }
                _ => self.skip_value()?,
            },
            SessionJsonObject::ThreadSettings => {
                if key == SessionJsonKey::Model {
                    summary.payload.thread_settings_model_seen = true;
                    let (value, malformed) = self.parse_text_value()?;
                    summary.payload.thread_settings_model = value;
                    summary.payload.thread_settings_model_malformed = malformed;
                } else {
                    self.skip_value()?;
                }
            }
            SessionJsonObject::Info => {
                if key == SessionJsonKey::TotalTokenUsage {
                    summary.payload.token_usage_seen = true;
                    summary.payload.token_usage_object = false;
                    summary.payload.token_usage = TokenUsageSummary::default();
                    if self.peek_byte()? == Some(b'{') {
                        summary.payload.token_usage_object = true;
                        self.parse_object(SessionJsonObject::TokenUsage, summary)?;
                        if !summary.payload.token_usage.total_seen {
                            summary.payload.token_usage.malformed = true;
                        }
                    } else {
                        self.skip_value()?;
                        summary.payload.token_usage.malformed = true;
                    }
                } else {
                    self.skip_value()?;
                }
            }
            SessionJsonObject::TokenUsage => match key {
                SessionJsonKey::TotalTokens => {
                    summary.payload.token_usage.total_seen = true;
                    let (value, malformed) = self.parse_u64_value()?;
                    summary.payload.token_usage.total = value;
                    summary.payload.token_usage.malformed |= malformed || value.is_none();
                }
                SessionJsonKey::CacheWriteInputTokens => {
                    let (value, malformed) = self.parse_u64_value()?;
                    summary.payload.token_usage.cache_write_input = value;
                    summary.payload.token_usage.malformed |= malformed;
                }
                SessionJsonKey::InputTokens => {
                    let (value, malformed) = self.parse_u64_value()?;
                    summary.payload.token_usage.input = value;
                    summary.payload.token_usage.malformed |= malformed;
                }
                SessionJsonKey::CachedInputTokens => {
                    let (value, malformed) = self.parse_u64_value()?;
                    summary.payload.token_usage.cached_input = value;
                    summary.payload.token_usage.malformed |= malformed;
                }
                SessionJsonKey::OutputTokens => {
                    let (value, malformed) = self.parse_u64_value()?;
                    summary.payload.token_usage.output = value;
                    summary.payload.token_usage.malformed |= malformed;
                }
                _ => self.skip_value()?,
            },
        }
        Ok(())
    }

    fn parse_key(&mut self) -> Result<SessionJsonKey, SessionRecordParseError> {
        if self.next_required()? != b'"' {
            return Err(SessionRecordParseError::Syntax);
        }
        let mut matches = [true; SESSION_JSON_KEYS.len()];
        let mut length = 0usize;
        loop {
            let byte = self.next_required()?;
            match byte {
                b'"' => break,
                b'\\' => {
                    let character = self.parse_escape_character()?;
                    if character.is_ascii() {
                        let byte = character as u8;
                        for (index, (candidate, _)) in SESSION_JSON_KEYS.iter().enumerate() {
                            if matches[index] && candidate.get(length).copied() != Some(byte) {
                                matches[index] = false;
                            }
                        }
                    } else {
                        matches.fill(false);
                    }
                    length = length.saturating_add(1);
                }
                byte if byte < 0x20 => return Err(SessionRecordParseError::Syntax),
                byte if byte < 0x80 => {
                    for (index, (candidate, _)) in SESSION_JSON_KEYS.iter().enumerate() {
                        if matches[index] && candidate.get(length).copied() != Some(byte) {
                            matches[index] = false;
                        }
                    }
                    length = length.saturating_add(1);
                }
                byte => {
                    self.consume_utf8_character(byte)?;
                    matches.fill(false);
                    length = length.saturating_add(1);
                }
            }
        }
        Ok(SESSION_JSON_KEYS
            .iter()
            .enumerate()
            .find_map(|(index, (candidate, key))| {
                (matches[index] && candidate.len() == length).then_some(*key)
            })
            .unwrap_or(SessionJsonKey::Other))
    }

    fn parse_text_value(&mut self) -> Result<(Option<String>, bool), SessionRecordParseError> {
        if self.peek_byte()? == Some(b'"') {
            let (value, truncated) = self.parse_text()?;
            return Ok((Some(value), truncated));
        }
        let is_null = self.peek_byte()? == Some(b'n');
        self.skip_value()?;
        Ok((None, !is_null))
    }

    fn parse_text(&mut self) -> Result<(String, bool), SessionRecordParseError> {
        if self.next_required()? != b'"' {
            return Err(SessionRecordParseError::Syntax);
        }
        let mut text = String::with_capacity(codex_info_db_writer::MAX_SESSION_MODEL_BYTES);
        let mut truncated = false;
        loop {
            let byte = self.next_required()?;
            match byte {
                b'"' => return Ok((text, truncated)),
                b'\\' => {
                    let character = self.parse_escape_character()?;
                    if text.len().saturating_add(character.len_utf8())
                        <= codex_info_db_writer::MAX_SESSION_MODEL_BYTES
                    {
                        text.push(character);
                    } else {
                        truncated = true;
                    }
                }
                byte if byte < 0x20 => return Err(SessionRecordParseError::Syntax),
                byte if byte < 0x80 => {
                    if text.len() < codex_info_db_writer::MAX_SESSION_MODEL_BYTES {
                        text.push(byte as char);
                    } else {
                        truncated = true;
                    }
                }
                byte => {
                    let (bytes, length) = self.read_utf8_character(byte)?;
                    let value = std::str::from_utf8(&bytes[..length])
                        .map_err(|_| SessionRecordParseError::Syntax)?;
                    if text.len().saturating_add(value.len())
                        <= codex_info_db_writer::MAX_SESSION_MODEL_BYTES
                    {
                        text.push_str(value);
                    } else {
                        truncated = true;
                    }
                }
            }
        }
    }

    fn skip_string(&mut self) -> Result<(), SessionRecordParseError> {
        if self.next_required()? != b'"' {
            return Err(SessionRecordParseError::Syntax);
        }
        loop {
            let byte = self.next_required()?;
            match byte {
                b'"' => return Ok(()),
                b'\\' => {
                    self.parse_escape_character()?;
                }
                byte if byte < 0x20 => return Err(SessionRecordParseError::Syntax),
                byte if byte < 0x80 => {}
                byte => {
                    self.consume_utf8_character(byte)?;
                }
            }
        }
    }

    fn parse_escape_character(&mut self) -> Result<char, SessionRecordParseError> {
        match self.next_required()? {
            b'"' => Ok('"'),
            b'\\' => Ok('\\'),
            b'/' => Ok('/'),
            b'b' => Ok('\u{0008}'),
            b'f' => Ok('\u{000c}'),
            b'n' => Ok('\n'),
            b'r' => Ok('\r'),
            b't' => Ok('\t'),
            b'u' => {
                let first = self.parse_hex_quad()?;
                if (0xD800..=0xDBFF).contains(&first) {
                    if self.next_required()? != b'\\' || self.next_required()? != b'u' {
                        return Err(SessionRecordParseError::Syntax);
                    }
                    let second = self.parse_hex_quad()?;
                    if !(0xDC00..=0xDFFF).contains(&second) {
                        return Err(SessionRecordParseError::Syntax);
                    }
                    let codepoint = 0x1_0000
                        + ((u32::from(first) - 0xD800) << 10)
                        + (u32::from(second) - 0xDC00);
                    char::from_u32(codepoint).ok_or(SessionRecordParseError::Syntax)
                } else if (0xDC00..=0xDFFF).contains(&first) {
                    Err(SessionRecordParseError::Syntax)
                } else {
                    char::from_u32(u32::from(first)).ok_or(SessionRecordParseError::Syntax)
                }
            }
            _ => Err(SessionRecordParseError::Syntax),
        }
    }

    fn parse_hex_quad(&mut self) -> Result<u16, SessionRecordParseError> {
        let mut value = 0u16;
        for _ in 0..4 {
            value = (value << 4)
                | u16::from(
                    char::from(self.next_required()?)
                        .to_digit(16)
                        .ok_or(SessionRecordParseError::Syntax)? as u8,
                );
        }
        Ok(value)
    }

    fn read_utf8_character(
        &mut self,
        first: u8,
    ) -> Result<([u8; 4], usize), SessionRecordParseError> {
        let length = match first {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => return Err(SessionRecordParseError::Syntax),
        };
        let mut bytes = [0u8; 4];
        bytes[0] = first;
        for byte in &mut bytes[1..length] {
            *byte = self.next_required()?;
            if !(*byte >= 0x80 && *byte <= 0xBF) {
                return Err(SessionRecordParseError::Syntax);
            }
        }
        std::str::from_utf8(&bytes[..length]).map_err(|_| SessionRecordParseError::Syntax)?;
        Ok((bytes, length))
    }

    fn consume_utf8_character(&mut self, first: u8) -> Result<(), SessionRecordParseError> {
        let _ = self.read_utf8_character(first)?;
        Ok(())
    }

    fn parse_u64_value(&mut self) -> Result<(Option<u64>, bool), SessionRecordParseError> {
        match self.peek_byte()? {
            Some(b'-' | b'0'..=b'9') => {
                let value = self.parse_number()?;
                Ok((value, value.is_none()))
            }
            Some(b'n') => {
                self.skip_value()?;
                Ok((None, false))
            }
            Some(_) => {
                self.skip_value()?;
                Ok((None, true))
            }
            None => Err(SessionRecordParseError::Syntax),
        }
    }

    fn parse_number(&mut self) -> Result<Option<u64>, SessionRecordParseError> {
        let negative = self.consume_if(b'-')?;
        let mut integer = true;
        let mut overflow = false;
        let mut value = 0u64;
        match self.next_byte()? {
            Some(b'0') => {
                if matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                    return Err(SessionRecordParseError::Syntax);
                }
            }
            Some(byte @ b'1'..=b'9') => {
                value = u64::from(byte - b'0');
                while let Some(byte @ b'0'..=b'9') = self.peek_byte()? {
                    let _ = self.next_byte()?;
                    if let Some(next) = value
                        .checked_mul(10)
                        .and_then(|value| value.checked_add(u64::from(byte - b'0')))
                    {
                        value = next;
                    } else {
                        overflow = true;
                    }
                }
            }
            _ => return Err(SessionRecordParseError::Syntax),
        }
        if self.consume_if(b'.')? {
            integer = false;
            if !matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                return Err(SessionRecordParseError::Syntax);
            }
            while matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                let _ = self.next_byte()?;
            }
        }
        if matches!(self.peek_byte()?, Some(b'e' | b'E')) {
            integer = false;
            let _ = self.next_byte()?;
            if !self.consume_if(b'+')? {
                let _ = self.consume_if(b'-')?;
            }
            if !matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                return Err(SessionRecordParseError::Syntax);
            }
            while matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                let _ = self.next_byte()?;
            }
        }
        Ok((!negative && integer && !overflow).then_some(value))
    }

    fn parse_literal(&mut self, literal: &[u8]) -> Result<(), SessionRecordParseError> {
        for expected in literal {
            if self.next_required()? != *expected {
                return Err(SessionRecordParseError::Syntax);
            }
        }
        Ok(())
    }

    fn skip_value(&mut self) -> Result<(), SessionRecordParseError> {
        self.skip_whitespace()?;
        match self.peek_byte()? {
            Some(b'"') => self.skip_string(),
            Some(b'{') => self.skip_object(),
            Some(b'[') => self.skip_array(),
            Some(b'n') => self.parse_literal(b"null"),
            Some(b't') => self.parse_literal(b"true"),
            Some(b'f') => self.parse_literal(b"false"),
            Some(b'-' | b'0'..=b'9') => self.parse_number().map(|_| ()),
            Some(_) | None => Err(SessionRecordParseError::Syntax),
        }
    }

    fn skip_object(&mut self) -> Result<(), SessionRecordParseError> {
        self.enter_container()?;
        let result = (|| {
            if self.next_required()? != b'{' {
                return Err(SessionRecordParseError::Syntax);
            }
            self.skip_whitespace()?;
            if self.consume_if(b'}')? {
                return Ok(());
            }
            loop {
                self.skip_string()?;
                self.skip_whitespace()?;
                if self.next_required()? != b':' {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_value()?;
                self.skip_whitespace()?;
                if self.consume_if(b'}')? {
                    return Ok(());
                }
                if !self.consume_if(b',')? {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_whitespace()?;
                if self.peek_byte()? == Some(b'}') {
                    return Err(SessionRecordParseError::Syntax);
                }
            }
        })();
        self.leave_container();
        result
    }

    fn skip_array(&mut self) -> Result<(), SessionRecordParseError> {
        self.enter_container()?;
        let result = (|| {
            if self.next_required()? != b'[' {
                return Err(SessionRecordParseError::Syntax);
            }
            self.skip_whitespace()?;
            if self.consume_if(b']')? {
                return Ok(());
            }
            loop {
                self.skip_value()?;
                self.skip_whitespace()?;
                if self.consume_if(b']')? {
                    return Ok(());
                }
                if !self.consume_if(b',')? {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_whitespace()?;
                if self.peek_byte()? == Some(b']') {
                    return Err(SessionRecordParseError::Syntax);
                }
            }
        })();
        self.leave_container();
        result
    }

    fn finish_record(&mut self) -> Result<bool, SessionRecordParseError> {
        if self.terminated {
            return Ok(true);
        }
        let buffer = self
            .reader
            .fill_buf()
            .map_err(|_| SessionRecordParseError::Io)?;
        if buffer.is_empty() {
            self.eof = true;
            return Ok(false);
        }
        if buffer[0] == b'\n' {
            self.consume(1);
            self.terminated = true;
            return Ok(true);
        }
        Ok(false)
    }

    fn drain_record(&mut self) -> Result<bool, SessionRecordParseError> {
        if self.terminated {
            return Ok(true);
        }
        loop {
            let buffer = self
                .reader
                .fill_buf()
                .map_err(|_| SessionRecordParseError::Io)?;
            if buffer.is_empty() {
                self.eof = true;
                return Ok(false);
            }
            if let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
                self.saw_bytes |= position > 0;
                self.consume(position + 1);
                self.terminated = true;
                return Ok(true);
            }
            self.saw_bytes = true;
            let length = buffer.len();
            self.consume(length);
        }
    }
}

fn read_streaming_record(reader: &mut BufReader<File>) -> Result<RecordRead, RecorderError> {
    if reader.fill_buf()?.is_empty() {
        return Ok(RecordRead::End);
    }
    let mut input = SessionRecordInput::new(reader);
    let parsed = input.parse_record();
    if matches!(parsed.as_ref(), Err(SessionRecordParseError::Io)) {
        return Err(RecorderError::Invalid(
            "session source read failed while parsing".to_owned(),
        ));
    }
    if parsed.is_err() {
        if !input.saw_bytes && input.eof && !input.terminated {
            return Ok(RecordRead::End);
        }
        let terminated = if input.terminated {
            true
        } else {
            input.drain_record().map_err(|_| {
                RecorderError::Invalid("session source read failed while draining".to_owned())
            })?
        };
        if terminated {
            return Ok(RecordRead::Invalid(input.consumed, true));
        }
        return Ok(RecordRead::Unterminated(input.consumed));
    }
    if input.finish_record().map_err(|_| {
        RecorderError::Invalid("session source read failed while framing".to_owned())
    })? {
        return Ok(RecordRead::Present(
            Box::new(parsed.expect("parsed session record")),
            input.consumed,
        ));
    }
    Ok(RecordRead::Unterminated(input.consumed))
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        use fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_bytes(&digest)
}

#[cfg(unix)]
fn process_starttime_ticks(pid: u32) -> Result<i64, RecorderError> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields = text
        .rsplit_once(") ")
        .ok_or_else(|| RecorderError::Invalid("process stat is malformed".to_owned()))?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let value = fields
        .get(19)
        .ok_or_else(|| RecorderError::Invalid("process stat has no start time".to_owned()))?
        .parse::<i64>()
        .map_err(|_| RecorderError::Invalid("process start time is invalid".to_owned()))?;
    if value <= 0 {
        return Err(RecorderError::Invalid(
            "process start time is not positive".to_owned(),
        ));
    }
    Ok(value)
}

#[cfg(not(unix))]
fn process_starttime_ticks(_pid: u32) -> Result<i64, RecorderError> {
    Ok(Utc::now().timestamp().max(1))
}

#[cfg(unix)]
fn executable_identity() -> Result<(u64, u64), RecorderError> {
    let path = fs::read_link("/proc/self/exe")?;
    let metadata = fs::metadata(path)?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn executable_identity() -> Result<(u64, u64), RecorderError> {
    Ok((1, 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::io::Write;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("codex-info-recorder-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn identity() -> StoragePartitionIdentity {
        StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".to_owned(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 1,
            partition_id: "33".repeat(32),
        }
    }

    fn config(root: &Path, chunk_bytes: u64) -> RecorderConfig {
        RecorderConfig {
            sessions_root: root.join("sessions"),
            chunk_bytes,
        }
    }

    fn prepare(name: &str) -> (PathBuf, PathBuf) {
        let root = temp_root(name);
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let database = root.join("usage_history.sqlite3");
        UsageStore::create_partitioned(&database, &identity()).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE collection_generation
                 SET reset_at=?1, window_seconds=?2",
                (Utc::now().timestamp() + 3600, 3600_i64),
            )
            .unwrap();
        (root, database)
    }

    fn token(total: u64, timestamp: i64) -> String {
        format!(
            "{{\"type\":\"event_msg\",\"timestamp\":\"{}\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"total_tokens\":{},\"input_tokens\":{},\"cached_input_tokens\":0,\"output_tokens\":0}}}}}}}}\n",
            DateTime::<Utc>::from_timestamp(timestamp, 0)
                .unwrap()
                .to_rfc3339(),
            total,
            total
        )
    }

    #[cfg(unix)]
    #[test]
    fn session_root_identity_remains_legacy_checkpoint_compatible() {
        let root = temp_root("legacy-root-identity");
        let metadata = fs::metadata(&root).unwrap();

        assert_eq!(
            root_identity(&root, &metadata),
            format!("unix:{}:{}", metadata.dev(), metadata.ino())
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn two_fresh_durable_acks_reflect_tokens() {
        let (root, database) = prepare("two-acks");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        fs::write(&source, token(10, now)).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        assert_eq!(recorder.run_cycle().unwrap().unwrap().generation, 1);
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(token(15, now + 1).as_bytes())
            .unwrap();
        assert_eq!(recorder.run_cycle().unwrap().unwrap().generation, 2);
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        assert_eq!(recorder.run_cycle().unwrap().unwrap().generation, 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn source_timestamp_ahead_of_cycle_clock_does_not_block_durable_ack() {
        let (root, database) = prepare("future-source-timestamp");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        Connection::open(&database)
            .unwrap()
            .execute(
                "UPDATE collection_generation SET window_seconds=?1",
                [7_200_i64],
            )
            .unwrap();
        fs::write(&source, token(10, now - 180)).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        recorder.run_cycle().unwrap().unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(format!("{}{}", token(15, now - 120), token(20, now + 120)).as_bytes())
            .unwrap();

        let report = recorder.run_cycle().unwrap().unwrap();

        assert_eq!(report.generation, 2);
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            10
        );
        let future_samples: i64 = Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM usage_history WHERE timestamp > ?1",
                [now],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(future_samples, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unchanged_counter_does_not_create_a_phantom_model_total() {
        let mut totals = ModelTotals::default();

        totals.add("ASTRA", TokenSnapshot::default()).unwrap();

        assert!(totals.to_totals().is_empty());
    }

    #[test]
    fn known_cache_write_delta_is_preserved_from_zero() {
        let mut totals = ModelTotals::default();

        totals
            .add(
                "ASTRA",
                TokenSnapshot {
                    total: 10,
                    input: 8,
                    cached_input: 2,
                    output: 2,
                    cache_write_input: Some(3),
                },
            )
            .unwrap();

        assert_eq!(totals.to_totals()[0].cache_write_input_tokens, Some(3));
    }

    #[test]
    fn recovery_delta_requires_matching_cache_write_lineage() {
        let anchor = TokenSnapshot {
            total: 10,
            input: 8,
            cached_input: 2,
            output: 2,
            cache_write_input: Some(3),
        };
        let current = TokenSnapshot {
            total: 15,
            input: 12,
            cached_input: 3,
            output: 3,
            cache_write_input: Some(5),
        };

        assert_eq!(
            current.checked_delta_from(anchor),
            Some(TokenSnapshot {
                total: 5,
                input: 4,
                cached_input: 1,
                output: 1,
                cache_write_input: Some(2),
            })
        );
        assert_eq!(
            TokenSnapshot {
                cache_write_input: None,
                ..current
            }
            .checked_delta_from(anchor),
            None
        );
    }

    #[test]
    fn timeline_weighting_preserves_the_exact_final_dollar_endpoint() {
        let dollars = 54.696256_f64;

        assert_eq!(weighted_dollars(87_009_506, 87_009_506, dollars), dollars);
        assert!(weighted_dollars(87_009_505, 87_009_506, dollars) <= dollars);
    }

    #[test]
    fn exhausted_cycle_budget_does_not_mark_sources_already_at_eof_as_backlog() {
        let (root, database) = prepare("budget-eof");
        let first_source = root.join("sessions/a.jsonl");
        let second_source = root.join("sessions/b.jsonl");
        let now = Utc::now().timestamp();
        fs::write(&first_source, token(10, now)).unwrap();
        fs::write(&second_source, token(20, now)).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 4096), &database, &identity()).unwrap();
        assert_eq!(recorder.run_cycle().unwrap().unwrap().pending_ranges, 0);
        drop(recorder);
        fs::OpenOptions::new()
            .append(true)
            .open(&first_source)
            .unwrap()
            .write_all(token(15, now + 1).as_bytes())
            .unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1), &database, &identity()).unwrap();

        let report = recorder.run_cycle().unwrap().unwrap();

        assert_eq!(report.accepted_ranges, 1);
        assert_eq!(report.pending_ranges, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_nonusage_is_streamed_and_following_token_is_recovered() {
        let (root, database) = prepare("oversized");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        let huge = format!(
            "{{\"type\":\"tool_result\",\"output\":\"{}\"}}\n",
            "x".repeat(4 * 1024 * 1024 + 1)
        );
        fs::write(&source, format!("{}{}", huge, token(20, now))).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 128), &database, &identity()).unwrap();
        assert_eq!(recorder.run_cycle().unwrap().unwrap().accepted_ranges, 1);
        assert_eq!(recorder.run_cycle().unwrap().unwrap().accepted_ranges, 1);
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(token(25, now + 1).as_bytes())
            .unwrap();
        recorder.run_cycle().unwrap().unwrap();
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn terminated_malformed_record_does_not_poison_following_tokens() {
        let (root, database) = prepare("malformed-following-token");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        fs::write(
            &source,
            format!(
                "{{\"unterminated\":}}\n{}{}",
                token(10, now),
                token(15, now + 1)
            ),
        )
        .unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        let first = recorder.run_cycle().unwrap().unwrap();
        assert_eq!(first.pending_ranges, 1);
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        assert!(!recorder.state().unwrap().checkpoints[0].fully_attributed_from_zero);
        let pending: (i64, String, i64) = Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT COUNT(*), reason, complete FROM session_pending_ranges",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(pending, (1, "malformed-json-record".to_owned(), 1));
        assert_eq!(recorder.generation().unwrap(), 1);
        let heartbeat = recorder.run_cycle().unwrap().unwrap();
        assert_eq!(heartbeat.accepted_ranges, 0);
        assert_eq!(heartbeat.generation, 2);
        assert_eq!(recorder.generation().unwrap(), 2);
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        let pending_count: i64 = Connection::open(&database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM session_pending_ranges", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(pending_count, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fresh_quota_outage_commits_events_then_materializes_after_quota_recovery() {
        let (root, database) = prepare("fresh-quota-outage");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        fs::write(&source, token(10, now)).unwrap();
        Connection::open(&database)
            .unwrap()
            .execute(
                "UPDATE collection_generation SET reset_at=0, window_seconds=0",
                [],
            )
            .unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();

        let first = recorder.run_cycle_with_quota(None).unwrap().unwrap();
        assert_eq!(first.generation, 1);
        assert_eq!(first.accepted_ranges, 1);
        assert!(recorder.model_totals().unwrap().is_empty());
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(token(15, now + 1).as_bytes())
            .unwrap();
        let second = recorder.run_cycle_with_quota(None).unwrap().unwrap();
        assert_eq!(second.generation, 2);
        assert_eq!(second.accepted_ranges, 1);
        assert!(recorder.model_totals().unwrap().is_empty());
        let event_count: i64 = Connection::open(&database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(event_count, 1);

        let recovered = recorder
            .run_cycle_with_quota(Some(QuotaSnapshot {
                observed_at: now + 2,
                reset_at: now + 3600,
                window_seconds: 3600,
                remaining_percent: Some(82.0),
            }))
            .unwrap()
            .unwrap();
        assert_eq!(recovered.generation, 3);
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        assert_eq!(recorder.state().unwrap().reset_at, now + 3600);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quota_outage_after_boundary_replays_events_into_the_new_period() {
        let (root, database) = prepare("quota-boundary-outage");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        let previous_reset = now + 10;
        Connection::open(&database)
            .unwrap()
            .execute(
                "UPDATE collection_generation SET reset_at=?1, window_seconds=?2",
                (0_i64, 0_i64),
            )
            .unwrap();
        fs::write(&source, token(10, now)).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        recorder
            .run_cycle_with_quota(Some(QuotaSnapshot {
                observed_at: now,
                reset_at: previous_reset,
                window_seconds: 3600,
                remaining_percent: Some(90.0),
            }))
            .unwrap()
            .unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(token(15, now + 15).as_bytes())
            .unwrap();
        recorder.run_cycle_with_quota(None).unwrap().unwrap();
        assert!(recorder.model_totals().unwrap().is_empty());
        let next_reset = now + 3610;
        recorder
            .run_cycle_with_quota(Some(QuotaSnapshot {
                observed_at: now + 20,
                reset_at: next_reset,
                window_seconds: 3600,
                remaining_percent: Some(77.0),
            }))
            .unwrap()
            .unwrap();
        assert_eq!(recorder.state().unwrap().reset_at, next_reset);
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn existing_checkpoint_does_not_drop_first_token_of_new_source() {
        let (root, database) = prepare("new-source-baseline");
        let first_source = root.join("sessions/first.jsonl");
        let second_source = root.join("sessions/second.jsonl");
        let now = Utc::now().timestamp();
        fs::write(&first_source, token(10, now)).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        recorder.run_cycle().unwrap().unwrap();
        fs::write(&second_source, token(20, now + 1)).unwrap();
        recorder.run_cycle().unwrap().unwrap();
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            20
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn replaced_source_without_anchor_is_reconciled_once_and_reported() {
        let (root, database) = prepare("source-replacement");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        let original = (1..=8)
            .map(|index| token(index * 10, now + index as i64))
            .collect::<String>();
        fs::write(&source, original).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 4096), &database, &identity()).unwrap();
        recorder.run_cycle().unwrap().unwrap();
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            70
        );
        fs::write(
            &source,
            format!("{}{}", token(100, now + 20), token(105, now + 21)),
        )
        .unwrap();
        let replacement_report = recorder.run_cycle().unwrap().unwrap();
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            95
        );
        assert_eq!(replacement_report.pending_ranges, 1);
        let diagnostic: (String, i64) = Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT reason, complete FROM session_pending_ranges",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            diagnostic,
            ("checkpoint-token-anchor-missing".to_owned(), 1)
        );

        recorder.run_cycle().unwrap().unwrap();
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            95
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn replaced_source_recovers_only_the_segment_after_a_counter_reset() {
        for (name, replacement_values, recovered_total) in [
            ("reset-above-anchor", [90, 100, 20, 105], 155),
            ("reset-below-anchor", [90, 100, 20, 30], 80),
            ("reset-before-first-record", [20, 20, 20, 30], 80),
        ] {
            let (root, database) = prepare(name);
            let source = root.join("sessions/one.jsonl");
            let now = Utc::now().timestamp();
            let original = (1..=8)
                .map(|index| token(index * 10, now + index as i64))
                .collect::<String>();
            fs::write(&source, original).unwrap();
            let mut recorder =
                Recorder::open_partitioned(config(&root, 4096), &database, &identity()).unwrap();
            recorder.run_cycle().unwrap().unwrap();

            let replacement = replacement_values
                .into_iter()
                .enumerate()
                .map(|(index, total)| token(total, now + 20 + index as i64))
                .collect::<String>();
            fs::write(&source, replacement).unwrap();
            let report = recorder.run_cycle().unwrap().unwrap();

            assert_eq!(
                recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
                recovered_total
            );
            assert_eq!(report.pending_ranges, 1);
            recorder.run_cycle().unwrap().unwrap();
            assert_eq!(
                recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
                recovered_total
            );
            let _ = fs::remove_dir_all(root);
        }
    }

    #[cfg(unix)]
    #[test]
    fn restored_source_inode_reuses_the_logical_path_checkpoint() {
        let (root, database) = prepare("source-inode-replacement");
        let source = root.join("sessions/one.jsonl");
        let replacement = root.join("sessions/replacement.tmp");
        let now = Utc::now().timestamp();
        let committed = format!("{}{}", token(10, now), token(15, now + 1));
        fs::write(&source, &committed).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 4096), &database, &identity()).unwrap();
        recorder.run_cycle().unwrap().unwrap();
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        let original_inode = fs::metadata(&source).unwrap().ino();
        fs::write(&replacement, format!("{committed}{}", token(20, now + 2))).unwrap();
        fs::rename(&replacement, &source).unwrap();
        assert_ne!(fs::metadata(&source).unwrap().ino(), original_inode);

        recorder.run_cycle().unwrap().unwrap();

        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            10
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recorder_lock_is_released_after_owner_drop() {
        let (root, database) = prepare("lock-release");
        let first =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        let second = Recorder::open_partitioned(config(&root, 1024), &database, &identity());
        assert!(matches!(
            second,
            Err(RecorderError::Invalid(message)) if message.contains("already owned")
        ));
        drop(first);
        let reopened = Recorder::open_partitioned(config(&root, 1024), &database, &identity());
        assert!(reopened.is_ok());
        drop(reopened);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn partial_tail_keeps_start_and_replays_when_completed() {
        let (root, database) = prepare("partial");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        let first = token(10, now);
        let second = token(15, now + 1);
        fs::write(&source, format!("{}{}", first, second.trim_end())).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        let first_report = recorder.run_cycle().unwrap().unwrap();
        assert_eq!(first_report.generation, 1);
        assert!(recorder.state().unwrap().checkpoints[0].discard_until_lf);
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        assert_eq!(recorder.run_cycle().unwrap().unwrap().generation, 2);
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restart_replay_is_idempotent() {
        let (root, database) = prepare("restart");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        fs::write(&source, token(10, now)).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        recorder.run_cycle().unwrap().unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(token(15, now + 1).as_bytes())
            .unwrap();
        recorder.run_cycle().unwrap().unwrap();
        drop(recorder);
        let mut restarted =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        assert_eq!(restarted.run_cycle().unwrap().unwrap().generation, 3);
        assert_eq!(restarted.generation().unwrap(), 3);
        assert_eq!(
            restarted.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn busy_database_does_not_advance_checkpoint_and_retries_same_batch() {
        let (root, database) = prepare("busy");
        let source = root.join("sessions/one.jsonl");
        let now = Utc::now().timestamp();
        fs::write(&source, token(10, now)).unwrap();
        let mut recorder =
            Recorder::open_partitioned(config(&root, 1024), &database, &identity()).unwrap();
        recorder.run_cycle().unwrap().unwrap();
        let before = recorder.state().unwrap().checkpoints[0].committed_offset;
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(token(15, now + 1).as_bytes())
            .unwrap();
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(recorder.run_cycle().is_err());
        drop(connection);
        assert_eq!(
            recorder.state().unwrap().checkpoints[0].committed_offset,
            before
        );
        assert_eq!(recorder.run_cycle().unwrap().unwrap().generation, 2);
        assert_eq!(
            recorder.model_totals().unwrap()[UNATTRIBUTED_MODEL].total,
            5
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn app_server_quota_parser_uses_canonical_longest_window() {
        let value = json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 10,
                    "resetsAt": 1_800,
                    "windowDurationMins": 60
                },
                "secondary": {
                    "usedPercent": 20,
                    "resetsAt": 2_000,
                    "windowDurationMins": 120
                }
            }
        });
        let snapshot = parse_app_server_quota(&value, "pro").unwrap();
        assert_eq!(snapshot.reset_at, 2_000);
        assert_eq!(snapshot.window_seconds, 120 * 60);
        assert_eq!(snapshot.remaining_percent, Some(80.0));
        assert!(snapshot.observed_at > 0);
    }

    #[test]
    fn active_rollout_uses_checkpoint_offset_and_preserves_display_model() {
        let root = temp_root("active-thread-record");
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let path = sessions.join("one.jsonl");
        let session_meta = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\"}}\n";
        fs::write(&path, session_meta).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let root_metadata = fs::metadata(&sessions).unwrap();
        let checkpoint = SessionCheckpoint {
            root_identity: root_identity(&sessions, &root_metadata),
            relative_path: "one.jsonl".to_owned(),
            file_device: file_device(&metadata),
            file_inode: file_inode(&metadata),
            committed_offset: session_meta.len() as u64,
            discard_until_lf: false,
            collector_epoch: 7,
            cycle_seq: 3,
            prefix_generation: 9,
            prefix_sha256: EMPTY_SHA256.to_owned(),
            fully_attributed_from_zero: true,
            token_baseline_known: true,
            last_model: Some("gpt-5.6-sol".to_owned()),
            last_task_running: Some(true),
            previous_total: 42,
            previous_input: 40,
            previous_cached_input: 0,
            previous_output: 2,
            previous_cache_write_input: Some(0),
        };
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(token(43, Utc::now().timestamp()).as_bytes())
            .unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let snapshot = read_active_rollout(&sessions, &path, &metadata, &checkpoint).unwrap();
        assert!(snapshot.is_running());
        assert_eq!(snapshot.model(), "gpt-5.6-sol");
        assert_eq!(snapshot.model_label(), "gpt-5.6-sol");
        assert_eq!(snapshot.total_tokens(), Some(43));
        let _ = fs::remove_dir_all(root);
    }
}

// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! The resident service's serialized usage-history writer.
//!
//! Local-session collection belongs to the resident state producer.  This
//! module owns only the profile lock, startup database maintenance, and atomic
//! commits of complete generations supplied by that producer. SQLite's
//! transaction/upsert contract remains the authority for durable writes.

use crate::account_scope::{self, AccountPartition};
use crate::security;
use crate::usage_store::{
    HistoryContinuityModelRecovery, RecordedSessionSource, RecorderGap, SessionCheckpoint,
    SessionCollectionCommit, SessionModelTotal, SessionRange, StoragePartitionIdentity,
    UsageHistoryObservation, UsageHistorySample, UsageStore,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(test)]
pub(crate) const RESET_HINT_FILE_NAME: &str = "usage_reset_hint.json";
pub(crate) const DAEMON_LOCK_FILE_NAME: &str = "usage_record_daemon.lock";
pub(crate) const RECORDER_STATE_FILE_NAME: &str = "recorder-state.json";
pub(crate) const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) const MIN_INTERVAL: Duration = Duration::from_secs(5);
pub(crate) const MAX_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[cfg(test)]
const MAX_HINT_BYTES: u64 = 4 * 1024;
const MAX_LOCK_BYTES: u64 = 4 * 1024;
#[cfg(test)]
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[allow(dead_code)]
const MAX_RECORDER_STATE_BYTES: u64 = 8 * 1024;
const RECORDER_STATE_SCHEMA: &str = "codex-info-recorder-state-v1";
pub(crate) const RECORDER_LAST_COMMIT_MAX_AGE_SECS: i64 = 150;
const MAX_RECORDER_UNIX_TIMESTAMP: i64 = 253_402_300_799;
#[cfg(target_os = "linux")]
const MAX_PROC_UPTIME_BYTES: usize = 128;
const MAX_SOURCE_RESCAN_SAMPLES: usize = 31 * 24 * 60;

pub(crate) fn monotonic_now_ns() -> u64 {
    #[cfg(target_os = "linux")]
    {
        return read_proc_uptime_ns().unwrap_or(0);
    }
    #[cfg(not(target_os = "linux"))]
    {
        // A process-relative Instant cannot order persisted values after a
        // restart.  There is no supported boot-wide source on this target,
        // so callers receive the invalid zero sentinel and fail closed.
        0
    }
}

#[cfg(target_os = "linux")]
fn parse_proc_uptime_ns(contents: &[u8]) -> Option<u64> {
    let token = contents
        .split(|byte| byte.is_ascii_whitespace())
        .next()
        .filter(|token| !token.is_empty())?;
    let mut seconds = 0_u64;
    let mut fraction = 0_u64;
    let mut fraction_digits = 0_usize;
    let mut saw_decimal = false;
    for byte in token {
        match byte {
            b'0'..=b'9' if saw_decimal => {
                if fraction_digits >= 9 {
                    return None;
                }
                fraction = fraction
                    .checked_mul(10)?
                    .checked_add(u64::from(byte - b'0'))?;
                fraction_digits += 1;
            }
            b'0'..=b'9' => {
                seconds = seconds
                    .checked_mul(10)?
                    .checked_add(u64::from(byte - b'0'))?;
            }
            b'.' if !saw_decimal => saw_decimal = true,
            _ => return None,
        }
    }
    let scale = 10_u64.checked_pow(u32::try_from(9_usize.saturating_sub(fraction_digits)).ok()?)?;
    seconds
        .checked_mul(1_000_000_000)?
        .checked_add(fraction.checked_mul(scale)?)
        .filter(|value| *value > 0)
}

#[cfg(target_os = "linux")]
fn read_proc_uptime_ns() -> Option<u64> {
    let file = File::open("/proc/uptime").ok()?;
    let mut contents = Vec::with_capacity(MAX_PROC_UPTIME_BYTES);
    file.take(u64::try_from(MAX_PROC_UPTIME_BYTES + 1).ok()?)
        .read_to_end(&mut contents)
        .ok()?;
    if contents.len() > MAX_PROC_UPTIME_BYTES {
        return None;
    }
    parse_proc_uptime_ns(&contents)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecorderWriteState {
    IdleNoAccount,
    Ready,
    Degraded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecorderState {
    pub(crate) schema: String,
    pub(crate) pid: u32,
    pub(crate) process_starttime: u64,
    pub(crate) owner_nonce: String,
    pub(crate) write_state: RecorderWriteState,
    pub(crate) partition_id_hash: Option<String>,
    pub(crate) data_generation: Option<u64>,
    pub(crate) collector_epoch: Option<String>,
    pub(crate) cycle_seq: Option<u64>,
    pub(crate) last_commit_unix: Option<i64>,
    pub(crate) updated_at_unix: i64,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ResetHint {
    pub(crate) reset_at: i64,
    pub(crate) window_seconds: i64,
}

#[cfg(test)]
impl ResetHint {
    fn new(reset_at: i64, window_seconds: i64) -> Option<Self> {
        (reset_at > 0 && (1..=366 * 86_400).contains(&window_seconds)).then_some(Self {
            reset_at,
            window_seconds,
        })
    }

    fn is_valid(self) -> bool {
        self.reset_at > 0 && (1..=366 * 86_400).contains(&self.window_seconds)
    }
}

/// Resolve the metadata location from the same protected data root as the
/// history database.  The path is intentionally not configurable separately:
/// a daemon must never read a hint from a different account/data directory.
#[cfg(test)]
pub(crate) fn reset_hint_path() -> Option<PathBuf> {
    crate::usage_data_root().map(|root| root.join("history").join(RESET_HINT_FILE_NAME))
}

pub(crate) fn daemon_lock_path() -> Option<PathBuf> {
    crate::usage_data_root().map(|root| root.join("history").join(DAEMON_LOCK_FILE_NAME))
}

pub(crate) fn recorder_state_path() -> Option<PathBuf> {
    crate::usage_data_root().map(|root| root.join("history").join(RECORDER_STATE_FILE_NAME))
}

#[allow(dead_code)]
fn read_recorder_state_path() -> Option<PathBuf> {
    let root = std::env::var_os("CODEX_INFO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(crate::default_codex_root);
    if !root.is_absolute() {
        return None;
    }
    security::validate_absolute_root(&root)
        .ok()
        .map(|root| root.join("history").join(RECORDER_STATE_FILE_NAME))
}

fn valid_lower_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes.saturating_mul(2)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn partition_id_hash(partition_id: &str) -> String {
    let digest = Sha256::digest(partition_id.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl RecorderState {
    fn validate(&self) -> Result<(), String> {
        if self.schema != RECORDER_STATE_SCHEMA
            || self.pid == 0
            || self.process_starttime == 0
            || !valid_lower_hex(&self.owner_nonce, 16)
            || self.updated_at_unix <= 0
            || self.updated_at_unix > MAX_RECORDER_UNIX_TIMESTAMP
            || self.updated_at_unix
                > unix_now()
                    .max(1)
                    .saturating_add(RECORDER_LAST_COMMIT_MAX_AGE_SECS)
            || self.data_generation.is_some_and(|value| value == 0)
            || self
                .partition_id_hash
                .as_deref()
                .is_some_and(|value| !valid_lower_hex(value, 32))
            || self.collector_epoch.as_deref().is_some_and(|value| {
                !valid_lower_hex(value, 16) || value.bytes().all(|b| b == b'0')
            })
            || self.cycle_seq.is_some_and(|value| value == 0)
            || self.last_commit_unix.is_some_and(|value| {
                value <= 0
                    || value > MAX_RECORDER_UNIX_TIMESTAMP
                    || value > unix_now().max(1)
                    || value > self.updated_at_unix
            })
        {
            return Err("recorder state identity or bounds are invalid".into());
        }
        match self.write_state {
            RecorderWriteState::IdleNoAccount
                if self.partition_id_hash.is_none()
                    && self.data_generation.is_none()
                    && self.collector_epoch.is_none()
                    && self.cycle_seq.is_none()
                    && self.last_commit_unix.is_none() => {}
            RecorderWriteState::Ready
                if self.partition_id_hash.is_some()
                    && self.data_generation.is_some()
                    && self.collector_epoch.is_some()
                    && self.cycle_seq.is_some()
                    && self.last_commit_unix.is_some() => {}
            RecorderWriteState::Degraded if self.partition_id_hash.is_some() => {}
            _ => return Err("recorder state write_state fields are inconsistent".into()),
        }
        Ok(())
    }
}

fn recorder_state_for_lock(
    lock: &DaemonLock,
    write_state: RecorderWriteState,
    partition_id: Option<&str>,
    data_generation: Option<u64>,
    collector_epoch: Option<u128>,
    cycle_seq: Option<u64>,
    last_commit_unix: Option<i64>,
) -> RecorderState {
    let partition_id_hash = partition_id.map(partition_id_hash);
    RecorderState {
        schema: RECORDER_STATE_SCHEMA.to_owned(),
        pid: lock.record.pid,
        process_starttime: lock.record.starttime_ticks,
        owner_nonce: lock.record.owner_nonce.clone(),
        write_state,
        partition_id_hash,
        data_generation,
        collector_epoch: collector_epoch.map(|epoch| format!("{epoch:032x}")),
        cycle_seq,
        last_commit_unix,
        updated_at_unix: unix_now().max(1),
    }
}

/// Convert a prior owner-only state into an explicit pending stop/restart
/// interval once the matching account partition is admitted.  The state file
/// contains only the partition hash, so this proof is completed at the
/// account boundary; a different account can never inherit the old gap.
fn pending_gap_from_previous_state(
    previous: &RecorderState,
    partition: &AccountPartition,
) -> Option<RecorderGap> {
    if previous.write_state == RecorderWriteState::IdleNoAccount
        || previous.partition_id_hash.as_deref()
            != Some(partition_id_hash(&partition.partition_id).as_str())
    {
        return None;
    }
    let stopped_at_monotonic_ns = monotonic_now_ns();
    if stopped_at_monotonic_ns == 0 {
        return None;
    }
    let now = unix_now().max(1);
    let start_at = previous
        .last_commit_unix
        .filter(|timestamp| *timestamp > 0 && *timestamp <= now)
        .unwrap_or(now);
    let owner_collector_epoch = previous
        .collector_epoch
        .as_deref()
        .and_then(|value| u128::from_str_radix(value, 16).ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    let confirmation_cycle_seq = previous.cycle_seq.filter(|value| *value > 0).unwrap_or(1);
    let mut digest = Sha256::new();
    digest.update(b"codex-info-recorder-restart-gap-v1\0");
    digest.update(previous.owner_nonce.as_bytes());
    digest.update(previous.pid.to_be_bytes());
    digest.update(previous.process_starttime.to_be_bytes());
    digest.update(partition.partition_id.as_bytes());
    let digest = digest.finalize();
    Some(RecorderGap {
        gap_id: digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        partition_id: partition.partition_id.clone(),
        source_identity_before: format!("resident:{}", partition.partition_id),
        source_identity_after: "unresolved".into(),
        cursor_before: format!(
            "generation-{}",
            previous.data_generation.unwrap_or_default()
        ),
        cursor_after: "unresolved".into(),
        stopped_at_monotonic_ns,
        resumed_at_monotonic_ns: None,
        start_at,
        end_at: now,
        reset_at: None,
        reason: "daemon_stop_unrecoverable".into(),
        state: "pending".into(),
        owner_collector_epoch,
        confirmation_cycle_seq,
    })
}

fn lock_is_current(lock: &DaemonLock) -> bool {
    let Ok(path_metadata) = fs::symlink_metadata(&lock.path) else {
        return false;
    };
    let Ok(file_metadata) = lock.file.metadata() else {
        return false;
    };
    !path_metadata.file_type().is_symlink()
        && path_metadata.is_file()
        && lock_identity_from_metadata(&path_metadata) == lock.identity
        && lock_identity_from_metadata(&file_metadata) == lock.identity
}

fn persist_recorder_state(lock: &DaemonLock, state: &RecorderState) -> Result<(), String> {
    if !lock_is_current(lock) {
        return Err("recorder profile lock changed before state write".into());
    }
    state.validate()?;
    let path =
        recorder_state_path().ok_or_else(|| "recorder state root is unavailable".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "recorder state parent is unavailable".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    // `create_dir_all` succeeds for an existing symlink.  Validate the
    // complete parent chain before changing permissions or replacing the
    // state path so a hostile `history` link can never redirect this owner
    // record outside the configured data root.
    security::validate_absolute_root(parent)
        .map_err(|_| "recorder state parent is unsafe".to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("recorder state path is not a regular file".into());
        }
    }
    let bytes = serde_json::to_vec(state).map_err(|error| error.to_string())?;
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{RECORDER_STATE_FILE_NAME}.tmp-{}-{counter}",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let result = (|| -> Result<(), String> {
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        if !lock_is_current(lock) {
            return Err("recorder profile lock changed during state write".into());
        }
        fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Read and validate the owner-only recorder state without changing the data
/// root. Packaging and tests use this helper as the liveness oracle.
#[allow(dead_code)]
pub(crate) fn read_recorder_state() -> Result<Option<RecorderState>, String> {
    let Some(path) = read_recorder_state_path() else {
        return Ok(None);
    };
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_RECORDER_STATE_BYTES
    {
        return Err("recorder state file is invalid".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "recorder state parent is unavailable".to_owned())?;
    security::validate_absolute_root(parent)
        .map_err(|_| "recorder state parent is unsafe".to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err("recorder state file is not owner-private".into());
        }
    }
    let bytes = fs::read(&path).map_err(|error| error.to_string())?;
    let value = crate::decode_unique_json(&bytes)?;
    let object = value
        .as_object()
        .ok_or_else(|| "recorder state is not an object".to_owned())?;
    const KEYS: [&str; 11] = [
        "schema",
        "pid",
        "process_starttime",
        "owner_nonce",
        "write_state",
        "partition_id_hash",
        "data_generation",
        "collector_epoch",
        "cycle_seq",
        "last_commit_unix",
        "updated_at_unix",
    ];
    if object.len() != KEYS.len() || KEYS.iter().any(|key| !object.contains_key(*key)) {
        return Err("recorder state key set is invalid".into());
    }
    let state: RecorderState = serde_json::from_value(value).map_err(|error| error.to_string())?;
    state.validate()?;
    Ok(Some(state))
}

/// Resolve the profile lock for `--stop` without creating a missing data
/// directory. Service startup intentionally prepares its root, but an
/// idempotent stop of a profile that was never started must remain read-only.
fn stop_lock_path() -> Option<PathBuf> {
    let root = std::env::var_os("CODEX_INFO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(crate::default_codex_root);
    if !root.is_absolute() {
        return None;
    }
    if !root.exists() {
        return Some(root.join("history").join(DAEMON_LOCK_FILE_NAME));
    }
    security::validate_absolute_root(&root)
        .ok()
        .map(|root| root.join("history").join(DAEMON_LOCK_FILE_NAME))
}

/// Read a bounded, private reset hint.  Any malformed, replaced, symlinked,
/// or oversized metadata is ignored; the next authenticated quota response
/// can safely replace it.
#[cfg(test)]
pub(crate) fn load_reset_hint() -> Option<(i64, i64)> {
    let path = reset_hint_path()?;
    let metadata = fs::symlink_metadata(&path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_HINT_BYTES {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).ok()?;
    let hint = serde_json::from_slice::<ResetHint>(&bytes).ok()?;
    hint.is_valid()
        .then_some((hint.reset_at, hint.window_seconds))
}

/// Atomically replace the reset hint after a successful quota response.
/// Existing metadata is not opened for writing, and a failed temporary write
/// leaves the previous hint untouched.
#[cfg(test)]
pub(crate) fn persist_reset_hint(reset_at: i64, window_seconds: i64) -> Result<(), ()> {
    let hint = ResetHint::new(reset_at, window_seconds).ok_or(())?;
    let path = reset_hint_path().ok_or(())?;
    let parent = path.parent().ok_or(())?;
    fs::create_dir_all(parent).map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|_| ())?;
    }

    // Refuse to replace a symlink.  A regular target is replaced with rename,
    // which is atomic within this directory and never follows the old path.
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(());
        }
    }

    let bytes = serde_json::to_vec(&hint).map_err(|_| ())?;
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{RESET_HINT_FILE_NAME}.tmp-{}-{counter}",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|_| ())?;
    let result = (|| {
        file.write_all(&bytes).map_err(|_| ())?;
        file.sync_all().map_err(|_| ())?;
        drop(file);
        fs::rename(&temporary, &path).map_err(|_| ())?;
        #[cfg(unix)]
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn daemon_interval_from_environment() -> Duration {
    let raw = std::env::var("CODEX_INFO_DAEMON_INTERVAL_SECS")
        .ok()
        .or_else(|| std::env::var("CODEX_INFO_RECORD_INTERVAL_SECS").ok());
    let seconds = raw
        .as_deref()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_INTERVAL);
    seconds.clamp(MIN_INTERVAL, MAX_INTERVAL)
}

#[derive(Debug)]
enum DaemonError {
    DataRoot,
    Lock(std::io::Error),
    Runtime,
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DataRoot => "daemon data directory is unavailable",
            Self::Lock(error) => {
                let _ = error.kind();
                "daemon lock operation failed"
            }
            Self::Runtime => "daemon runtime could not start",
        })
    }
}

impl std::error::Error for DaemonError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct LockRecord {
    pid: u32,
    started_at: i64,
    starttime_ticks: u64,
    executable_device: u64,
    executable_inode: u64,
    owner_nonce: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProcessIdentity {
    pid: u32,
    starttime_ticks: u64,
    executable_device: u64,
    executable_inode: u64,
}

#[cfg(unix)]
fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    use std::os::unix::fs::MetadataExt;

    if pid == 0 {
        return None;
    }
    let process_root = Path::new("/proc").join(pid.to_string());
    // `/proc/<pid>/stat` encloses comm in parentheses and comm may contain
    // spaces. Split only after the final `) `; starttime is field 22, hence
    // index 19 in the remaining field-3-based slice.
    let stat = fs::read_to_string(process_root.join("stat")).ok()?;
    let (_, fields) = stat.rsplit_once(") ")?;
    let starttime_ticks = fields.split_whitespace().nth(19)?.parse().ok()?;
    let executable = fs::metadata(process_root.join("exe")).ok()?;
    Some(ProcessIdentity {
        pid,
        starttime_ticks,
        executable_device: executable.dev(),
        executable_inode: executable.ino(),
    })
}

#[cfg(not(unix))]
fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    (pid == std::process::id()).then_some(ProcessIdentity {
        pid,
        starttime_ticks: 0,
        executable_device: 0,
        executable_inode: 0,
    })
}

#[cfg(unix)]
fn owner_nonce() -> Result<String, DaemonError> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(DaemonError::Lock)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(not(unix))]
fn owner_nonce() -> Result<String, DaemonError> {
    // The recorder is a Linux/WSL service. Keep non-Unix builds compilable,
    // while never treating this fallback as a cross-process authority.
    Ok(format!(
        "{:016x}{:016x}",
        std::process::id(),
        unix_now().unsigned_abs()
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LockIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    length: u64,
    #[cfg(not(unix))]
    modified_nanos: u128,
}

fn lock_identity_from_metadata(metadata: &fs::Metadata) -> LockIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        LockIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        LockIdentity {
            length: metadata.len(),
            modified_nanos: modified_nanos(metadata),
        }
    }
}

fn lock_is_same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    lock_identity_from_metadata(left) == lock_identity_from_metadata(right)
}

impl LockRecord {
    fn is_complete(&self) -> bool {
        self.pid > 0
            && self.started_at > 0
            && self.starttime_ticks > 0
            && self.executable_device > 0
            && self.executable_inode > 0
            && valid_lower_hex(&self.owner_nonce, 16)
    }

    fn matches_process(&self, identity: &ProcessIdentity) -> bool {
        self.is_complete()
            && self.pid == identity.pid
            && self.starttime_ticks == identity.starttime_ticks
            && self.executable_device == identity.executable_device
            && self.executable_inode == identity.executable_inode
    }
}

/// Parse only the current, complete lock schema.  The exact key set matters:
/// accepting a partially populated legacy object would make a numeric PID an
/// authority again.  Unknown keys are rejected so a future schema cannot be
/// mistaken for this stop contract without an explicit parser update.
fn parse_lock_record(bytes: &[u8]) -> Option<LockRecord> {
    let value = crate::decode_unique_json(bytes).ok()?;
    let object = value.as_object()?;
    const REQUIRED_KEYS: [&str; 6] = [
        "pid",
        "started_at",
        "starttime_ticks",
        "executable_device",
        "executable_inode",
        "owner_nonce",
    ];
    if object.len() != REQUIRED_KEYS.len()
        || REQUIRED_KEYS.iter().any(|key| !object.contains_key(*key))
    {
        return None;
    }
    let record = serde_json::from_value::<LockRecord>(value).ok()?;
    record.is_complete().then_some(record)
}

fn lock_owner_is_current(record: &LockRecord) -> bool {
    process_identity(record.pid).is_some_and(|identity| record.matches_process(&identity))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LockSnapshot {
    identity: LockIdentity,
    record: LockRecord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StopPhase {
    Initial,
    PreSignal,
    PostSignal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StopError {
    LockUnavailable,
    LockInvalid,
    OwnerChanged,
    SignalFailed,
    Timeout,
    #[allow(dead_code)]
    Unsupported,
}

/// Read a lock while proving that the path and opened file refer to the same
/// regular file before and after the bounded payload read.  A present but
/// malformed, incomplete, symlinked, oversized, or replaced lock is never
/// converted into an authority or removed by the stop path.
fn read_lock_snapshot(path: &Path) -> Result<Option<LockSnapshot>, StopError> {
    read_lock_snapshot_for_phase_with_hook(path, StopPhase::Initial, None)
}

fn read_lock_snapshot_for_phase(
    path: &Path,
    phase: StopPhase,
) -> Result<Option<LockSnapshot>, StopError> {
    read_lock_snapshot_for_phase_with_hook(path, phase, None)
}

fn read_lock_snapshot_for_phase_with_hook(
    path: &Path,
    phase: StopPhase,
    after_read: Option<&dyn Fn()>,
) -> Result<Option<LockSnapshot>, StopError> {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return match phase {
                StopPhase::PreSignal => Err(StopError::OwnerChanged),
                StopPhase::Initial | StopPhase::PostSignal => Ok(None),
            }
        }
        Err(_) => return Err(StopError::LockUnavailable),
    };
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || path_metadata.len() > MAX_LOCK_BYTES
    {
        return Err(StopError::LockInvalid);
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return match phase {
                StopPhase::PreSignal => Err(StopError::OwnerChanged),
                StopPhase::Initial => Err(StopError::LockUnavailable),
                StopPhase::PostSignal => Ok(None),
            }
        }
        Err(_) => return Err(StopError::LockUnavailable),
    };
    let file_metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return match phase {
                StopPhase::PreSignal => Err(StopError::OwnerChanged),
                StopPhase::Initial => Err(StopError::LockUnavailable),
                StopPhase::PostSignal => Ok(None),
            }
        }
        Err(_) => return Err(StopError::LockUnavailable),
    };
    if !lock_is_same_file(&path_metadata, &file_metadata) {
        return Err(StopError::OwnerChanged);
    }
    let mut bytes = Vec::with_capacity(path_metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|_| StopError::LockUnavailable)?;
    if bytes.len() > MAX_LOCK_BYTES as usize {
        return Err(StopError::LockInvalid);
    }
    if let Some(after_read) = after_read {
        after_read();
    }
    let current_path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return match phase {
                StopPhase::PreSignal => Err(StopError::OwnerChanged),
                StopPhase::Initial => Err(StopError::OwnerChanged),
                StopPhase::PostSignal => Ok(None),
            }
        }
        Err(_) => return Err(StopError::LockUnavailable),
    };
    if current_path_metadata.file_type().is_symlink()
        || !lock_is_same_file(&current_path_metadata, &file_metadata)
    {
        return Err(StopError::OwnerChanged);
    }
    let record = parse_lock_record(&bytes).ok_or(StopError::LockInvalid)?;
    Ok(Some(LockSnapshot {
        identity: lock_identity_from_metadata(&file_metadata),
        record,
    }))
}

#[cfg(test)]
fn current_lock_owner_pid_at(path: &Path) -> Option<u32> {
    let snapshot = read_lock_snapshot(path).ok().flatten()?;
    lock_owner_is_current(&snapshot.record).then_some(snapshot.record.pid)
}

/// A process-instance-bound owner token for callers that need to pair a
/// listener response with the exact lock owner. PID alone is insufficient
/// across rapid exit/restart and PID reuse; retain the same starttime,
/// executable device/inode, and nonce that the stop contract validates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DaemonOwnerIdentity {
    pub(crate) pid: u32,
    pub(crate) starttime_ticks: u64,
    pub(crate) executable_device: u64,
    pub(crate) executable_inode: u64,
    pub(crate) owner_nonce: String,
}

pub(crate) fn current_daemon_owner_identity() -> Option<DaemonOwnerIdentity> {
    let snapshot = read_lock_snapshot(&daemon_lock_path()?).ok().flatten()?;
    let process = process_identity(snapshot.record.pid)?;
    snapshot
        .record
        .matches_process(&process)
        .then_some(DaemonOwnerIdentity {
            pid: snapshot.record.pid,
            starttime_ticks: snapshot.record.starttime_ticks,
            executable_device: snapshot.record.executable_device,
            executable_inode: snapshot.record.executable_inode,
            owner_nonce: snapshot.record.owner_nonce,
        })
}

/// Return only the PID from a complete, current recorder lock identity.
/// Callers use this to distinguish the service child they own from a
/// concurrently-started winner; malformed, stale, or replaced locks are not
/// treated as authority.
pub(crate) fn current_daemon_owner_pid() -> Option<u32> {
    current_daemon_owner_identity().map(|owner| owner.pid)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OwnerClassification {
    NoOwner,
    ManagedActive,
    KnownUnmanagedCodex,
    Stale,
    Malformed,
    Foreign,
}

#[cfg(target_os = "linux")]
fn process_has_managed_marker(pid: u32) -> bool {
    let path = Path::new("/proc").join(pid.to_string()).join("environ");
    fs::read(path)
        .ok()
        .map(|bytes| {
            bytes
                .split(|byte| *byte == 0)
                .any(|entry| entry == b"CODEX_INFO_SYSTEMD_MANAGED=1")
        })
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn process_has_managed_marker(_pid: u32) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn process_is_known_codex(identity: &ProcessIdentity) -> bool {
    let process_root = Path::new("/proc").join(identity.pid.to_string());
    let executable = fs::read_link(process_root.join("exe"))
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_owned()));
    let executable_name = executable.as_deref().and_then(|name| name.to_str());
    let executable_name = matches!(executable_name, Some("codex_info" | "codex-info"));
    if !executable_name {
        return false;
    }
    let command_line = fs::read(process_root.join("cmdline")).ok();
    let Some(command_line) = command_line else {
        return false;
    };
    let args = command_line
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .collect::<Vec<_>>();
    if args.len() != 3 || args[1] != b"--port" {
        return false;
    }
    let valid_port = args[2].iter().all(u8::is_ascii_digit) && !args[2].is_empty() && {
        let port = args[2].iter().fold(0_u32, |value, byte| {
            value
                .saturating_mul(10)
                .saturating_add(u32::from(*byte - b'0'))
        });
        (1..=u32::from(u16::MAX)).contains(&port)
    };
    valid_port
}

#[cfg(not(target_os = "linux"))]
fn process_is_known_codex(_identity: &ProcessIdentity) -> bool {
    false
}

pub(crate) fn classify_profile_owner() -> OwnerClassification {
    let Some(path) = daemon_lock_path() else {
        return OwnerClassification::Malformed;
    };
    let path_exists = fs::symlink_metadata(&path).is_ok();
    let snapshot = match read_lock_snapshot(&path) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return if path_exists {
                OwnerClassification::Malformed
            } else {
                OwnerClassification::NoOwner
            };
        }
        Err(_) => return OwnerClassification::Malformed,
    };
    let Some(identity) = process_identity(snapshot.record.pid) else {
        return OwnerClassification::Stale;
    };
    if !snapshot.record.matches_process(&identity) {
        return OwnerClassification::Stale;
    }
    if process_has_managed_marker(identity.pid) {
        OwnerClassification::ManagedActive
    } else if process_is_known_codex(&identity) {
        OwnerClassification::KnownUnmanagedCodex
    } else {
        OwnerClassification::Foreign
    }
}

#[cfg(target_os = "linux")]
fn pidfd_for_process(pid: u32) -> Result<rustix::fd::OwnedFd, StopError> {
    use rustix::process::{pidfd_open, Pid, PidfdFlags};

    let raw_pid = i32::try_from(pid).map_err(|_| StopError::SignalFailed)?;
    let pid = Pid::from_raw(raw_pid).ok_or(StopError::SignalFailed)?;
    pidfd_open(pid, PidfdFlags::empty()).map_err(|_| StopError::SignalFailed)
}

/// Send one TERM through a pidfd for a process we spawned ourselves.  This is
/// used only to reap a losing startup child; the public `--stop` path below
/// additionally revalidates the profile lock before invoking the same kernel
/// primitive.
#[cfg(target_os = "linux")]
pub(crate) fn send_term_to_owned_process(pid: u32) -> bool {
    use rustix::process::{pidfd_send_signal, Signal};

    let Ok(pidfd) = pidfd_for_process(pid) else {
        return false;
    };
    pidfd_send_signal(&pidfd, Signal::Term).is_ok()
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn send_term_to_owned_process(_pid: u32) -> bool {
    false
}

/// Stop the profile-owned service, if present, using a process-instance-bound
/// pidfd.  The lock is read and validated before the pidfd is acquired, then
/// the exact lock identity, record, and process identity are read again after
/// acquisition.  A single TERM is sent only after both observations agree.
pub(crate) fn stop_daemon() -> Result<(), StopError> {
    let path = stop_lock_path().ok_or(StopError::LockUnavailable)?;
    let Some(initial) = read_lock_snapshot(&path)? else {
        // An absent profile lock is the idempotent stopped state.
        return Ok(());
    };
    let initial_process = process_identity(initial.record.pid).ok_or(StopError::LockInvalid)?;
    if !initial.record.matches_process(&initial_process) {
        return Err(StopError::LockInvalid);
    }

    #[cfg(target_os = "linux")]
    {
        let pidfd = pidfd_for_process(initial.record.pid)?;
        let Some(revalidated) = read_lock_snapshot_for_phase(&path, StopPhase::PreSignal)? else {
            return Err(StopError::OwnerChanged);
        };
        let revalidated_process =
            process_identity(revalidated.record.pid).ok_or(StopError::OwnerChanged)?;
        if revalidated != initial || !revalidated.record.matches_process(&revalidated_process) {
            return Err(StopError::OwnerChanged);
        }

        // Deliberately do not retry this operation: one invocation owns at
        // most one TERM, and a failed syscall is reported to the caller.
        use rustix::process::{pidfd_send_signal, Signal};
        pidfd_send_signal(&pidfd, Signal::Term).map_err(|_| StopError::SignalFailed)?;

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let phase = StopPhase::PostSignal;
        loop {
            match read_lock_snapshot_for_phase(&path, phase) {
                Ok(None) => return Ok(()),
                Ok(Some(current)) if current != initial => return Err(StopError::OwnerChanged),
                Ok(Some(_)) => {}
                Err(error) => return Err(error),
            }
            if std::time::Instant::now() >= deadline {
                return Err(StopError::Timeout);
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = initial;
        Err(StopError::Unsupported)
    }
}

fn lock_is_stale(path: &Path) -> Result<bool, DaemonError> {
    let metadata = fs::symlink_metadata(path).map_err(DaemonError::Lock)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(false);
    }
    if metadata.len() > MAX_LOCK_BYTES {
        return Ok(false);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(DaemonError::Lock)?
        .read_to_end(&mut bytes)
        .map_err(DaemonError::Lock)?;
    if let Some(record) = parse_lock_record(&bytes) {
        return Ok(!lock_owner_is_current(&record));
    }
    // A malformed/unknown lock is not stale evidence. Preserve it so a
    // managed activation fails closed instead of silently deleting a foreign
    // owner's coordination record.
    Ok(false)
}

struct DaemonLock {
    path: PathBuf,
    file: File,
    identity: LockIdentity,
    record: LockRecord,
}

impl DaemonLock {
    fn acquire(path: PathBuf) -> Result<Option<Self>, DaemonError> {
        let parent = path.parent().ok_or(DaemonError::DataRoot)?;
        fs::create_dir_all(parent).map_err(DaemonError::Lock)?;
        // Do not follow an existing symlink at the profile-lock directory.
        // This check must precede chmod and create_new so lock authority
        // remains within the configured, owner-private data root.
        security::validate_absolute_root(parent).map_err(|_| DaemonError::DataRoot)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(DaemonError::Lock)?;
        }

        for attempt in 0..2 {
            let mut options = OpenOptions::new();
            options.write(true).read(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(mut file) => {
                    let process = process_identity(std::process::id()).ok_or_else(|| {
                        DaemonError::Lock(std::io::Error::other(
                            "current process identity is unavailable",
                        ))
                    })?;
                    let record = LockRecord {
                        pid: process.pid,
                        started_at: unix_now(),
                        starttime_ticks: process.starttime_ticks,
                        executable_device: process.executable_device,
                        executable_inode: process.executable_inode,
                        owner_nonce: owner_nonce()?,
                    };
                    let record = serde_json::to_vec(&record).map_err(|_| {
                        DaemonError::Lock(std::io::Error::other(
                            "lock identity serialization failed",
                        ))
                    })?;
                    let result = (|| -> Result<fs::Metadata, DaemonError> {
                        file.write_all(&record).map_err(DaemonError::Lock)?;
                        file.write_all(b"\n").map_err(DaemonError::Lock)?;
                        file.sync_all().map_err(DaemonError::Lock)?;
                        file.metadata().map_err(DaemonError::Lock)
                    })();
                    match result {
                        Ok(metadata) => {
                            return Ok(Some(Self {
                                path,
                                file,
                                identity: lock_identity_from_metadata(&metadata),
                                record: serde_json::from_slice(&record).map_err(|_| {
                                    DaemonError::Lock(std::io::Error::other(
                                        "lock identity serialization failed",
                                    ))
                                })?,
                            }));
                        }
                        Err(error) => {
                            // The lock was newly created by this attempt. If
                            // writing it fails, clean up only when the path
                            // still names this exact inode; never remove a
                            // racing replacement.
                            if let (Ok(path_metadata), Ok(file_metadata)) =
                                (fs::symlink_metadata(&path), file.metadata())
                            {
                                if lock_is_same_file(&path_metadata, &file_metadata) {
                                    let _ = fs::remove_file(&path);
                                }
                            }
                            return Err(error);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 1 {
                        return Ok(None);
                    }
                    if !lock_is_stale(&path)? {
                        return Ok(None);
                    }
                    let before = fs::symlink_metadata(&path).map_err(DaemonError::Lock)?;
                    if before.file_type().is_symlink() {
                        return Ok(None);
                    }
                    // Only remove the exact stale file we inspected.  A
                    // racing new owner leaves a different inode and is not
                    // disturbed.
                    let current = fs::symlink_metadata(&path).map_err(DaemonError::Lock)?;
                    if !lock_is_same_file(&before, &current) {
                        return Ok(None);
                    }
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(DaemonError::Lock(error)),
                    }
                }
                Err(error) => return Err(DaemonError::Lock(error)),
            }
        }
        Ok(None)
    }
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        let Ok(path_metadata) = fs::symlink_metadata(&self.path) else {
            return;
        };
        let Ok(file_metadata) = self.file.metadata() else {
            return;
        };
        if path_metadata.file_type().is_symlink()
            || !path_metadata.is_file()
            || lock_identity_from_metadata(&path_metadata) != self.identity
            || lock_identity_from_metadata(&file_metadata) != self.identity
        {
            return;
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_secs()).ok())
        .unwrap_or(0)
}

fn profile_lock_path() -> Result<PathBuf, DaemonError> {
    daemon_lock_path().ok_or(DaemonError::DataRoot)
}

pub(crate) struct RecorderGeneration {
    pub(crate) reset_at: i64,
    pub(crate) window_seconds: i64,
    pub(crate) collector_epoch: u128,
    pub(crate) cycle_seq: u64,
    pub(crate) samples: Vec<UsageHistorySample>,
    pub(crate) observations: Vec<UsageHistoryObservation>,
    pub(crate) recorded_sessions: Vec<RecordedSessionSource>,
    pub(crate) session_checkpoints: Vec<SessionCheckpoint>,
    pub(crate) session_ranges: Vec<SessionRange>,
    pub(crate) session_model_totals: Vec<SessionModelTotal>,
    pub(crate) history_continuity_recovery: Option<HistoryContinuityModelRecovery>,
    pub(crate) bounded_source_rescan_complete: bool,
}

/// A source closure is accepted only when the bounded collection result has at
/// least one actual quota observation for the admitted reset period. Session
/// backfill rows have a null quota and are excluded from this proof; an empty
/// quota result never proves that the source was closed.
fn quota_source_rescan_is_closed(
    samples: &[UsageHistorySample],
    reset_at: i64,
    bounded_source_rescan_complete: bool,
) -> bool {
    let quota_samples = samples
        .iter()
        .filter(|sample| {
            sample.reset_at == reset_at
                && sample.remaining_percent.is_some()
                && sample.timestamp > 0
                && sample.timestamp.rem_euclid(60) == 0
        })
        .collect::<Vec<_>>();
    bounded_source_rescan_complete
        && !quota_samples.is_empty()
        && quota_samples.len() <= MAX_SOURCE_RESCAN_SAMPLES
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecorderCommitAck {
    pub(crate) data_generation: u64,
    pub(crate) collector_epoch: u128,
    pub(crate) cycle_seq: u64,
    pub(crate) last_commit_unix: i64,
    pub(crate) canonical_samples: Vec<UsageHistorySample>,
    pub(crate) canonical_observations: Vec<UsageHistoryObservation>,
    pub(crate) fallback_model_totals: Option<Vec<SessionModelTotal>>,
    pub(crate) legacy_history_bridged: bool,
}

enum RecorderCommand {
    Activate {
        partition: AccountPartition,
        now: chrono::DateTime<chrono::Utc>,
        completed: mpsc::SyncSender<Result<(), String>>,
    },
    Deactivate {
        completed: mpsc::SyncSender<Result<(), String>>,
    },
    Store {
        partition_id: String,
        generation: RecorderGeneration,
        committed: mpsc::SyncSender<Result<RecorderCommitAck, String>>,
    },
    ForgetRecordedSessions {
        partition_id: String,
        recorded_sessions: Vec<RecordedSessionSource>,
        committed: mpsc::SyncSender<Result<(), String>>,
    },
    BeginGap {
        partition_id: String,
        gap: RecorderGap,
        completed: mpsc::SyncSender<Result<(), String>>,
    },
    Shutdown,
}

fn maintain_history_database(
    database: &Path,
    identity: &StoragePartitionIdentity,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<UsageStore, String> {
    UsageStore::backup_generations_partitioned(database, identity, 3)
        .map_err(|error| error.to_string())?;
    let mut store =
        UsageStore::open_partitioned(database, identity).map_err(|error| error.to_string())?;
    store
        .prune_older_than_three_months(now)
        .map_err(|error| error.to_string())?;
    Ok(store)
}

#[cfg(test)]
pub(crate) fn maintain_history_database_for_test(
    database: &Path,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    UsageStore::backup_generations(database, 3).map_err(|error| error.to_string())?;
    let mut store = UsageStore::open(database).map_err(|error| error.to_string())?;
    store
        .prune_older_than_three_months(now)
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) struct ActiveRecorderPartition {
    partition: AccountPartition,
    store: UsageStore,
    _writer_lock: DaemonLock,
}

fn private_partition_directory(partition: &AccountPartition) -> Result<PathBuf, String> {
    let directory = partition
        .database_path
        .parent()
        .ok_or_else(|| "account partition directory is missing".to_owned())?;
    if partition.candidate_path.parent() != Some(directory)
        || partition.writer_lock_path.parent() != Some(directory)
    {
        return Err("account partition paths disagree".into());
    }
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let account_directory = directory
            .parent()
            .ok_or_else(|| "account scope directory is missing".to_owned())?;
        for private_directory in [account_directory, directory] {
            fs::set_permissions(private_directory, fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
            security::validate_absolute_root(private_directory)
                .map_err(|error| error.to_string())?;
        }
    }
    security::validate_absolute_root(directory).map_err(|error| error.to_string())
}

fn path_exists_without_following(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("account partition artifact is not a regular file".into())
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn legacy_database_for_partition(partition: &AccountPartition) -> Option<PathBuf> {
    let epoch = partition.database_path.parent()?;
    let account = epoch.parent()?;
    let version = account.parent()?;
    let accounts = version.parent()?;
    let history = accounts.parent()?;
    (version.file_name()? == "v1" && accounts.file_name()? == "accounts")
        .then(|| history.join("usage_history.sqlite3"))
}

fn sync_file_and_parent(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "account partition directory is missing".to_owned())?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

pub(crate) fn activate_account_partition(
    partition: AccountPartition,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ActiveRecorderPartition, String> {
    let directory = private_partition_directory(&partition)?;
    let identity = partition.storage_identity();

    let final_exists = path_exists_without_following(&partition.database_path)?;
    let candidate_exists = path_exists_without_following(&partition.candidate_path)?;
    if final_exists && candidate_exists {
        return Err("account partition has both final and candidate databases".into());
    }
    let existing_database = final_exists;
    if !final_exists {
        if candidate_exists {
            let candidate = UsageStore::open_partitioned(&partition.candidate_path, &identity)
                .map_err(|error| error.to_string())?;
            drop(candidate);
        } else {
            let candidate = UsageStore::create_partitioned(&partition.candidate_path, &identity)
                .map_err(|error| error.to_string())?;
            drop(candidate);
        }
        sync_file_and_parent(&partition.candidate_path)?;
        fs::rename(&partition.candidate_path, &partition.database_path)
            .map_err(|error| error.to_string())?;
        File::open(&directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
    }

    let writer_lock = DaemonLock::acquire(partition.writer_lock_path.clone())
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "account partition writer is already owned".to_owned())?;
    let store = if existing_database {
        maintain_history_database(&partition.database_path, &identity, now)?
    } else {
        UsageStore::open_partitioned(&partition.database_path, &identity)
            .map_err(|error| error.to_string())?
    };
    account_scope::mark_partition_initialized(&partition).map_err(|error| error.to_string())?;
    Ok(ActiveRecorderPartition {
        partition,
        store,
        _writer_lock: writer_lock,
    })
}

/// Serialized history-writer ownership embedded in the combined service.
///
/// This worker never reads local sessions. The resident state producer sends
/// complete generations, and the worker acknowledges each only after its
/// SQLite transaction commits. The service process owns SIGINT/SIGTERM and
/// stops this worker before releasing the REST listener.
pub(crate) struct RecorderWorker {
    commands: Option<mpsc::Sender<RecorderCommand>>,
    worker: Option<JoinHandle<()>>,
    active: bool,
}

impl RecorderWorker {
    pub(crate) fn start() -> Result<Self, String> {
        let (commands, command_receiver) = mpsc::channel();
        let (started, started_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("codex-info-recorder".into())
            .spawn(move || {
                let result = (|| -> Result<Option<DaemonLock>, DaemonError> {
                    let lock_path = profile_lock_path()?;
                    let lock = DaemonLock::acquire(lock_path)?;
                    Ok(lock)
                })();
                let lock = match result {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = started.send(Err(error.to_string()));
                        return;
                    }
                };
                let Some(_lock) = lock else {
                    // An already-running owner is not an error and must never
                    // be killed or replaced by an X-only/combined launch.
                    let _ = started.send(Ok(false));
                    return;
                };

                // Read the prior owner state while the new profile lock is
                // held, before replacing it with this process's initial idle
                // state.  A valid state from a dead owner is the only durable
                // restart evidence; malformed state is preserved as a
                // fail-closed startup error rather than overwritten.
                let previous_state = match read_recorder_state() {
                    Ok(previous) => previous,
                    Err(error) => {
                        let _ = started.send(Err(error));
                        return;
                    }
                };

                if let Err(error) = persist_recorder_state(
                    &_lock,
                    &recorder_state_for_lock(
                        &_lock,
                        RecorderWriteState::IdleNoAccount,
                        None,
                        None,
                        None,
                        None,
                        None,
                    ),
                ) {
                    let _ = started.send(Err(error));
                    return;
                }

                // Readiness means this process owns the exact profile lock and
                // its serialized writer is waiting for producer commands.
                if started.send(Ok(true)).is_err() {
                    return;
                }

                let mut active: Option<ActiveRecorderPartition> = None;
                let mut previous_state = previous_state;
                let mut injected_failure_consumed = false;
                while let Ok(command) = command_receiver.recv() {
                    match command {
                        RecorderCommand::Shutdown => break,
                        RecorderCommand::Activate {
                            partition,
                            now,
                            completed,
                        } => {
                            if active.as_ref().is_some_and(|current| {
                                current.partition.partition_id == partition.partition_id
                            }) {
                                let _ = completed.send(Ok(()));
                                continue;
                            }
                            // Switching identity is a hard writer boundary:
                            // close the old SQLite handle and release its exact
                            // account lock before touching the new partition.
                            active = None;
                            let requested_partition_id = partition.partition_id.clone();
                            match activate_account_partition(partition, now) {
                                Ok(mut next) => {
                                    if let Some(previous) = previous_state.as_ref() {
                                        if let Some(restart_gap) =
                                            pending_gap_from_previous_state(previous, &next.partition)
                                        {
                                            let gap_result = (|| -> Result<(), String> {
                                                let store = &mut next.store;
                                                let mut restart_gap = restart_gap;
                                                let collection_state = store
                                                    .load_session_collection_state()
                                                    .map_err(|error| error.to_string())?;
                                                if collection_state.reset_at > 0 {
                                                    // The old state file deliberately
                                                    // contains no reset value. Use
                                                    // only the already-committed
                                                    // partition generation, never
                                                    // the wall clock, to bind the
                                                    // pending interval to a period.
                                                    restart_gap.reset_at =
                                                        Some(collection_state.reset_at);
                                                }
                                                let has_pending = store
                                                    .load_recorder_gaps()
                                                    .map_err(|error| error.to_string())?
                                                    .iter()
                                                    .any(|gap| gap.state == "pending");
                                                if has_pending {
                                                    Ok(())
                                                } else {
                                                    store
                                                        .begin_recorder_gap(&restart_gap)
                                                        .map_err(|error| error.to_string())
                                                }
                                            })();
                                            if let Err(error) = gap_result {
                                                let _ = completed.send(Err(error));
                                                continue;
                                            }
                                            // The prior owner evidence has
                                            // now been consumed for this
                                            // partition. Keep it only when a
                                            // different account is admitted
                                            // first, so A→B→A still gets the
                                            // correct hash-bound boundary.
                                            previous_state = None;
                                        }
                                    }
                                    if let Err(error) = persist_recorder_state(
                                        &_lock,
                                        &recorder_state_for_lock(
                                            &_lock,
                                            RecorderWriteState::Degraded,
                                            Some(&next.partition.partition_id),
                                            None,
                                            None,
                                            None,
                                            None,
                                        ),
                                    ) {
                                        let _ = completed.send(Err(error));
                                        continue;
                                    }
                                    active = Some(next);
                                    let _ = completed.send(Ok(()));
                                }
                                Err(error) => {
                                    // Do not leave the previous Ready record
                                    // looking authoritative after the account
                                    // switch has released its writer.  The
                                    // requested partition is retained only as
                                    // bounded identity context; no generation
                                    // or commit fields are claimed.
                                    let _ = persist_recorder_state(
                                        &_lock,
                                        &recorder_state_for_lock(
                                            &_lock,
                                            RecorderWriteState::Degraded,
                                            Some(&requested_partition_id),
                                            None,
                                            None,
                                            None,
                                            None,
                                        ),
                                    );
                                    let _ = completed.send(Err(error));
                                }
                            }
                        }
                        RecorderCommand::Deactivate { completed } => {
                            active = None;
                            let result = persist_recorder_state(
                                &_lock,
                                &recorder_state_for_lock(
                                    &_lock,
                                    RecorderWriteState::IdleNoAccount,
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                ),
                            );
                            let _ = completed.send(result);
                        }
                        RecorderCommand::Store {
                            partition_id,
                            generation,
                            committed,
                        } => {
                            let RecorderGeneration {
                                reset_at,
                                window_seconds,
                                collector_epoch,
                                cycle_seq,
                                samples,
                                observations,
                                recorded_sessions,
                                session_checkpoints,
                                session_ranges,
                                session_model_totals,
                                history_continuity_recovery,
                                bounded_source_rescan_complete,
                            } = generation;
                            if std::env::var("CODEX_INFO_RECORDER_FAILURE")
                                .ok()
                                .is_some_and(|mode| {
                                    !injected_failure_consumed && mode == "worker-death"
                                })
                            {
                                break;
                            }
                            let mut result = if std::env::var("CODEX_INFO_RECORDER_FAILURE")
                                .ok()
                                .is_some_and(|mode| {
                                    !injected_failure_consumed && mode == "busy"
                                }) {
                                injected_failure_consumed = true;
                                Err("database is locked (injected busy)".to_owned())
                            } else if let Some(mode) = std::env::var("CODEX_INFO_RECORDER_FAILURE")
                                .ok()
                                .filter(|mode| {
                                    !injected_failure_consumed
                                        && matches!(mode.as_str(), "fatal" | "readonly" | "full")
                                })
                            {
                                injected_failure_consumed = true;
                                Err(format!("database write failed (injected {mode})"))
                            } else {
                                active
                                    .as_mut()
                                    .filter(|current| current.partition.partition_id == partition_id)
                                    .ok_or_else(|| {
                                        "recorder account partition is not active".to_owned()
                                    })
                                    .and_then(|current| {
                                        let legacy =
                                            legacy_database_for_partition(&current.partition);
                                        let store = &mut current.store;
                                        let mut commit_samples = samples.as_slice();
                                        let mut commit_model_totals =
                                            session_model_totals.as_slice();
                                        let mut fallback_was_used = false;
                                        if let Some(recovery) = history_continuity_recovery.as_ref() {
                                            if let Err(error) = store
                                                .apply_history_continuity_model_totals(recovery)
                                            {
                                                // Recovery is optional; ordinary recording is
                                                // not. The producer supplied the exact same
                                                // generation before adding the recovery offset,
                                                // so no inferred subtraction is needed here.
                                                eprintln!(
                                                    "codex-info: continuity recovery deferred; committing ordinary generation: {error}"
                                                );
                                                commit_samples = &recovery.fallback_samples;
                                                commit_model_totals =
                                                    &recovery.fallback_model_totals;
                                                fallback_was_used = true;
                                            }
                                        }
                                        let commit_result = store
                                            .commit_session_collection_with_observations(SessionCollectionCommit {
                                                reset_at,
                                                window_seconds,
                                                collector_epoch,
                                                cycle_seq,
                                                samples: commit_samples,
                                                checkpoints: &session_checkpoints,
                                                ranges: &session_ranges,
                                                model_totals: commit_model_totals,
                                                recorded_sessions: &recorded_sessions,
                                            }, &observations)
                                            .map_err(|error| error.to_string())?;
                                        let mut data_generation = commit_result.data_generation;
                                        let canonical_samples = commit_result.canonical_samples;
                                        let canonical_observations =
                                            commit_result.canonical_observations;
                                        let mut legacy_history_bridged = false;
                                        if let Some(legacy) = legacy {
                                            if store
                                                .bridge_verified_legacy_history(&legacy)
                                                .map_err(|error| error.to_string())?
                                            {
                                                legacy_history_bridged = true;
                                                data_generation = store
                                                    .load_session_collection_state()
                                                    .map_err(|error| error.to_string())?
                                                    .data_generation;
                                            }
                                        }
                                        // The quota portion of this
                                        // acknowledged generation is the
                                        // only production source proof a
                                        // recorder restart may consume.
                                        // Session backfill rows carry a
                                        // null remaining value and are
                                        // intentionally excluded, so
                                        // token recovery can never
                                        // fabricate a quota gap repair.
                                        let mut source_minutes = samples
                                            .iter()
                                            .filter(|sample| {
                                                // A quota value from a
                                                // different reset period is
                                                // not evidence for this
                                                // gap, even when its minute
                                                // happens to overlap.
                                                sample.reset_at == reset_at
                                                    && sample.remaining_percent.is_some()
                                            })
                                            .map(|sample| sample.timestamp.div_euclid(60) * 60)
                                            .collect::<Vec<_>>();
                                        source_minutes.sort_unstable();
                                        source_minutes.dedup();
                                        let source_identity_after =
                                            format!("authenticated-quota:{partition_id}");
                                        let cursor_after = format!(
                                            "collector:{collector_epoch:032x}:cycle:{cycle_seq}"
                                        );
                                        let source_closed = quota_source_rescan_is_closed(
                                            &samples,
                                            reset_at,
                                            bounded_source_rescan_complete,
                                        );
                                        let resumed_at_monotonic_ns = monotonic_now_ns();
                                        if resumed_at_monotonic_ns == 0 {
                                            return Err(
                                                "boot-wide monotonic clock is unavailable"
                                                    .to_owned(),
                                            );
                                        }
                                        store
                                            .reconcile_pending_recorder_gaps(
                                                &source_identity_after,
                                                &cursor_after,
                                                resumed_at_monotonic_ns,
                                                reset_at,
                                                collector_epoch,
                                                cycle_seq,
                                                &source_minutes,
                                                source_closed,
                                            )
                                            .map_err(|error| error.to_string())?;
                                        // The transaction return value is
                                        // checked again through a fresh
                                        // SELECT before publishing state;
                                        // a successful SQL call alone is
                                        // not a recorder liveness claim.
                                        let committed_state = store
                                            .load_session_collection_state()
                                            .map_err(|error| error.to_string())?;
                                        if committed_state.data_generation != data_generation
                                            || committed_state.collector_epoch
                                                != Some(collector_epoch)
                                            || committed_state.cycle_seq != cycle_seq
                                        {
                                            return Err(
                                                "recorder generation read-back mismatch".to_owned(),
                                            );
                                        }
                                        Ok(RecorderCommitAck {
                                            data_generation,
                                            collector_epoch,
                                            cycle_seq,
                                            last_commit_unix: unix_now().max(1),
                                            canonical_samples,
                                            canonical_observations,
                                            fallback_model_totals: fallback_was_used
                                                .then(|| commit_model_totals.to_vec()),
                                            legacy_history_bridged,
                                        })
                                    })
                            };
                            if result.is_ok() {
                                if let (Some(current), Ok(ack)) =
                                    (active.as_ref(), result.as_ref())
                                {
                                    if let Err(error) = persist_recorder_state(
                                        &_lock,
                                        &recorder_state_for_lock(
                                            &_lock,
                                            RecorderWriteState::Ready,
                                            Some(&current.partition.partition_id),
                                            Some(ack.data_generation),
                                            Some(ack.collector_epoch),
                                            Some(ack.cycle_seq),
                                            Some(ack.last_commit_unix),
                                        ),
                                    ) {
                                        result = Err(error);
                                    } else {
                                        eprintln!(
                                            "codex-info: recorder committed {} samples and {} session markers",
                                            samples.len(),
                                            recorded_sessions.len()
                                        );
                                    }
                                }
                            } else if let Some(current) = active.as_ref() {
                                let _ = persist_recorder_state(
                                    &_lock,
                                    &recorder_state_for_lock(
                                        &_lock,
                                        RecorderWriteState::Degraded,
                                        Some(&current.partition.partition_id),
                                        None,
                                        None,
                                        None,
                                        None,
                                    ),
                                );
                            }
                            let _ = committed.send(result);
                        }
                        RecorderCommand::ForgetRecordedSessions {
                            partition_id,
                            recorded_sessions,
                            committed,
                        } => {
                            let result = active
                                .as_mut()
                                .filter(|current| current.partition.partition_id == partition_id)
                                .ok_or_else(|| {
                                    "recorder account partition is not active".to_owned()
                                })
                                .and_then(|current| {
                                    current
                                        .store
                                        .forget_recorded_sessions(&recorded_sessions)
                                        .map(|_| ())
                                        .map_err(|error| error.to_string())
                                });
                            let _ = committed.send(result);
                        }
                        RecorderCommand::BeginGap {
                            partition_id,
                            gap,
                            completed,
                        } => {
                            let result = active
                                .as_mut()
                                .filter(|current| current.partition.partition_id == partition_id)
                                .ok_or_else(|| {
                                    "recorder account partition is not active".to_owned()
                                })
                                .and_then(|current| {
                                    if gap.partition_id != current.partition.partition_id {
                                        return Err(
                                            "recorder gap account partition mismatch".to_owned(),
                                        );
                                    }
                                    current
                                        .store
                                        .begin_recorder_gap(&gap)
                                        .map_err(|error| error.to_string())
                                });
                            let _ = completed.send(result);
                        }
                    }
                }
            })
            .map_err(|_| DaemonError::Runtime.to_string())?;

        match started_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(active)) => Ok(Self {
                commands: Some(commands),
                worker: Some(worker),
                active,
            }),
            Ok(Err(error)) => {
                let _ = commands.send(RecorderCommand::Shutdown);
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = commands.send(RecorderCommand::Shutdown);
                let _ = worker.join();
                Err(DaemonError::Runtime.to_string())
            }
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    /// The worker owns the profile lock from a dedicated thread.  A command
    /// channel can be closed between two service ticks, so publication also
    /// checks the lock identity instead of relying on the startup boolean.
    pub(crate) fn owner_is_live(&self) -> bool {
        self.active && current_daemon_owner_pid() == Some(std::process::id())
    }

    /// Probe worker liveness at the one-second service cadence without adding
    /// a command behind a legitimate SQLite transaction.  A response timeout
    /// cannot distinguish a busy writer from a dead writer; the owned join
    /// handle can, without delaying or terminating the resident service.
    pub(crate) fn probe(&self) -> Result<(), String> {
        if self.commands.is_none()
            || self
                .worker
                .as_ref()
                .is_none_or(std::thread::JoinHandle::is_finished)
        {
            return Err(DaemonError::Runtime.to_string());
        }
        Ok(())
    }

    /// Quiesce any old account and activate exactly one confirmed storage
    /// partition. Candidate creation/recovery and bounded maintenance all run
    /// on the sole serialized writer thread.
    pub(crate) fn activate_partition(
        &self,
        partition: AccountPartition,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String> {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| DaemonError::Runtime.to_string())?;
        let (completed, receiver) = mpsc::sync_channel(1);
        commands
            .send(RecorderCommand::Activate {
                partition,
                now,
                completed,
            })
            .map_err(|_| DaemonError::Runtime.to_string())?;
        receiver
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| DaemonError::Runtime.to_string())?
    }

    pub(crate) fn deactivate_partition(&self) -> Result<(), String> {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| DaemonError::Runtime.to_string())?;
        let (completed, receiver) = mpsc::sync_channel(1);
        commands
            .send(RecorderCommand::Deactivate { completed })
            .map_err(|_| DaemonError::Runtime.to_string())?;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| DaemonError::Runtime.to_string())?
    }

    /// Commit usage rows and the exact source markers that authorize later
    /// cleanup in one SQLite transaction.
    pub(crate) fn store_generation(
        &self,
        partition_id: String,
        generation: RecorderGeneration,
    ) -> Result<RecorderCommitAck, String> {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| DaemonError::Runtime.to_string())?;
        let (committed, receiver) = mpsc::sync_channel(1);
        commands
            .send(RecorderCommand::Store {
                partition_id,
                generation,
                committed,
            })
            .map_err(|_| DaemonError::Runtime.to_string())?;
        // A real SQLite busy transaction is allowed to use its complete
        // two-second busy timeout.  A worker that closes while handling the
        // command must not make the resident wait beyond the same liveness
        // budget, so observe the owned thread between short receive windows.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err("recorder worker response timed out".to_owned());
            }
            match receiver.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(DaemonError::Runtime.to_string());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self
                        .worker
                        .as_ref()
                        .is_some_and(std::thread::JoinHandle::is_finished)
                    {
                        return Err("recorder worker stopped".to_owned());
                    }
                }
            }
        }
    }

    /// Remove exact markers after their source files were successfully
    /// unlinked. Failure is safe: the stale marker remains fingerprint-bound.
    pub(crate) fn forget_recorded_sessions(
        &self,
        partition_id: String,
        recorded_sessions: Vec<RecordedSessionSource>,
    ) -> Result<(), String> {
        if recorded_sessions.is_empty() {
            return Ok(());
        }
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| DaemonError::Runtime.to_string())?;
        let (committed, receiver) = mpsc::sync_channel(1);
        commands
            .send(RecorderCommand::ForgetRecordedSessions {
                partition_id,
                recorded_sessions,
                committed,
            })
            .map_err(|_| DaemonError::Runtime.to_string())?;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| DaemonError::Runtime.to_string())?
    }

    /// Persist a pending interval before releasing the resident owner.  The
    /// interval remains non-public until a later source-rescan caller proves
    /// recovery or an explicit source authority confirms it unrecoverable.
    pub(crate) fn begin_gap(&self, partition_id: String, gap: RecorderGap) -> Result<(), String> {
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| DaemonError::Runtime.to_string())?;
        let (completed, receiver) = mpsc::sync_channel(1);
        commands
            .send(RecorderCommand::BeginGap {
                partition_id,
                gap,
                completed,
            })
            .map_err(|_| DaemonError::Runtime.to_string())?;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| DaemonError::Runtime.to_string())?
    }

    pub(crate) fn shutdown(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(RecorderCommand::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RecorderWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("codex-info-daemon-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn boot_wide_monotonic_source_is_bounded_and_restart_ordered() {
        assert_eq!(
            parse_proc_uptime_ns(b"42.000000001 7.0\n"),
            Some(42_000_000_001)
        );
        assert_eq!(parse_proc_uptime_ns(b"42 7.0\n"), Some(42_000_000_000));
        assert!(parse_proc_uptime_ns(b"42.0000000001 7.0\n").is_none());
        assert!(parse_proc_uptime_ns(b"not-uptime 7.0\n").is_none());
        assert!(parse_proc_uptime_ns(&vec![b'1'; MAX_PROC_UPTIME_BYTES + 1]).is_none());

        // These fixtures model two different processes reading the same
        // boot-wide clock. A persisted stop value remains ordered after the
        // restart; the database transition test rejects any reversal.
        let stopped = parse_proc_uptime_ns(b"9000.100000001 1.0\n").unwrap();
        let resumed = parse_proc_uptime_ns(b"9000.100000002 1.0\n").unwrap();
        assert!(resumed >= stopped);
    }

    #[test]
    fn interval_is_bounded_even_for_invalid_environment_values() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old = std::env::var_os("CODEX_INFO_DAEMON_INTERVAL_SECS");
        std::env::set_var("CODEX_INFO_DAEMON_INTERVAL_SECS", "1");
        assert_eq!(daemon_interval_from_environment(), MIN_INTERVAL);
        std::env::set_var("CODEX_INFO_DAEMON_INTERVAL_SECS", "999999");
        assert_eq!(daemon_interval_from_environment(), MAX_INTERVAL);
        std::env::set_var("CODEX_INFO_DAEMON_INTERVAL_SECS", "not-a-number");
        assert_eq!(daemon_interval_from_environment(), DEFAULT_INTERVAL);
        match old {
            Some(value) => std::env::set_var("CODEX_INFO_DAEMON_INTERVAL_SECS", value),
            None => std::env::remove_var("CODEX_INFO_DAEMON_INTERVAL_SECS"),
        }
    }

    #[test]
    fn reset_hint_round_trip_is_atomic_and_private() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = temp_root("hint");
        let old = std::env::var_os("CODEX_INFO_DATA_DIR");
        std::env::set_var("CODEX_INFO_DATA_DIR", &root);
        assert_eq!(load_reset_hint(), None);
        persist_reset_hint(1_800_000_000, 604_800).unwrap();
        assert_eq!(load_reset_hint(), Some((1_800_000_000, 604_800)));
        #[cfg(unix)]
        {
            let metadata = fs::metadata(root.join("history").join(RESET_HINT_FILE_NAME)).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
        match old {
            Some(value) => std::env::set_var("CODEX_INFO_DATA_DIR", value),
            None => std::env::remove_var("CODEX_INFO_DATA_DIR"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_pid_lock_is_reclaimed_and_live_lock_is_singleton() {
        let root = temp_root("lock");
        let path = root.join(DAEMON_LOCK_FILE_NAME);
        let stale = LockRecord {
            pid: 4_294_967_294,
            started_at: 1,
            starttime_ticks: 1,
            executable_device: 1,
            executable_inode: 1,
            owner_nonce: "00".repeat(16),
        };
        fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
        let first = DaemonLock::acquire(path.clone()).unwrap().unwrap();
        assert_eq!(current_lock_owner_pid_at(&path), Some(std::process::id()));
        assert!(DaemonLock::acquire(path.clone()).unwrap().is_none());
        drop(first);
        assert_eq!(current_lock_owner_pid_at(&path), None);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stop_authority_requires_complete_lock_record() {
        let root = temp_root("strict-lock");
        let path = root.join(DAEMON_LOCK_FILE_NAME);
        let current = process_identity(std::process::id()).unwrap();
        let complete = LockRecord {
            pid: current.pid,
            started_at: unix_now(),
            starttime_ticks: current.starttime_ticks,
            executable_device: current.executable_device,
            executable_inode: current.executable_inode,
            owner_nonce: "ab".repeat(16),
        };
        fs::write(&path, serde_json::to_vec(&complete).unwrap()).unwrap();
        assert_eq!(current_lock_owner_pid_at(&path), Some(std::process::id()));

        for payload in [
            br#"{"pid":1,"started_at":1}"#.as_slice(),
            br#"{"pid":1,"started_at":1,"starttime_ticks":1,"executable_device":1,"executable_inode":1}"#.as_slice(),
            br#"{"pid":1,"started_at":1,"starttime_ticks":1,"executable_device":1,"executable_inode":1,"owner_nonce":"00"}"#.as_slice(),
            b"not-json".as_slice(),
        ] {
            fs::write(&path, payload).unwrap();
            assert_eq!(current_lock_owner_pid_at(&path), None);
            assert!(parse_lock_record(payload).is_none());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn managed_owner_classification_fails_closed_for_missing_malformed_and_foreign_lock() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = temp_root("owner-classification");
        let old_data = std::env::var_os("CODEX_INFO_DATA_DIR");
        let old_marker = std::env::var_os("CODEX_INFO_SYSTEMD_MANAGED");
        std::env::set_var("CODEX_INFO_DATA_DIR", &root);
        std::env::remove_var("CODEX_INFO_SYSTEMD_MANAGED");
        let path = daemon_lock_path().unwrap();
        assert_eq!(classify_profile_owner(), OwnerClassification::NoOwner);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br#"{"pid":1}"#).unwrap();
        assert_eq!(classify_profile_owner(), OwnerClassification::Malformed);

        let current = process_identity(std::process::id()).unwrap();
        let record = LockRecord {
            pid: current.pid,
            started_at: unix_now(),
            starttime_ticks: current.starttime_ticks,
            executable_device: current.executable_device,
            executable_inode: current.executable_inode,
            owner_nonce: "ef".repeat(16),
        };
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(classify_profile_owner(), OwnerClassification::Foreign);
        let _ = fs::remove_file(path);
        match old_marker {
            Some(value) => std::env::set_var("CODEX_INFO_SYSTEMD_MANAGED", value),
            None => std::env::remove_var("CODEX_INFO_SYSTEMD_MANAGED"),
        }
        match old_data {
            Some(value) => std::env::set_var("CODEX_INFO_DATA_DIR", value),
            None => std::env::remove_var("CODEX_INFO_DATA_DIR"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stop_of_missing_profile_is_success_without_creating_data_root() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "codex-info-daemon-stop-absent-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let old = std::env::var_os("CODEX_INFO_DATA_DIR");
        std::env::set_var("CODEX_INFO_DATA_DIR", &root);
        assert_eq!(stop_daemon(), Ok(()));
        assert!(!root.exists());
        match old {
            Some(value) => std::env::set_var("CODEX_INFO_DATA_DIR", value),
            None => std::env::remove_var("CODEX_INFO_DATA_DIR"),
        }
    }

    #[test]
    fn post_signal_lock_disappearance_after_payload_read_is_success() {
        let root = temp_root("post-signal-disappearance");
        let path = root.join(DAEMON_LOCK_FILE_NAME);
        let current = process_identity(std::process::id()).unwrap();
        let record = LockRecord {
            pid: current.pid,
            started_at: unix_now(),
            starttime_ticks: current.starttime_ticks,
            executable_device: current.executable_device,
            executable_inode: current.executable_inode,
            owner_nonce: "cd".repeat(16),
        };
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();

        let remove_lock = || fs::remove_file(&path).unwrap();
        let pre_signal =
            read_lock_snapshot_for_phase_with_hook(&path, StopPhase::PreSignal, Some(&remove_lock));
        assert_eq!(pre_signal, Err(StopError::OwnerChanged));
        assert!(!path.exists());

        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let post_signal = read_lock_snapshot_for_phase_with_hook(
            &path,
            StopPhase::PostSignal,
            Some(&remove_lock),
        );
        assert_eq!(post_signal, Ok(None));
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn pid_reuse_identity_mismatch_is_reclaimed_without_touching_live_owner() {
        let root = temp_root("pid-reuse");
        let path = root.join(DAEMON_LOCK_FILE_NAME);
        let current = process_identity(std::process::id()).unwrap();
        let reused = LockRecord {
            pid: current.pid,
            started_at: unix_now(),
            starttime_ticks: current.starttime_ticks.saturating_add(1),
            executable_device: current.executable_device,
            executable_inode: current.executable_inode,
            owner_nonce: "00".repeat(16),
        };
        fs::write(&path, serde_json::to_vec(&reused).unwrap()).unwrap();

        let acquired = DaemonLock::acquire(path.clone()).unwrap().unwrap();
        let stored: LockRecord = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored.pid, current.pid);
        assert_eq!(stored.starttime_ticks, current.starttime_ticks);
        assert_eq!(stored.executable_device, current.executable_device);
        assert_eq!(stored.executable_inode, current.executable_inode);
        assert_eq!(stored.owner_nonce.len(), 32);
        drop(acquired);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recorder_writer_persists_only_the_caller_generation_and_holds_the_profile_lock() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = temp_root("writer-generation");
        let codex_home = root.join("codex");
        let sessions = codex_home
            .join("sessions")
            .join("2026")
            .join("08")
            .join("22");
        fs::create_dir_all(&sessions).unwrap();
        let now = unix_now().max(1);
        let reset_at = now + 3_600;
        let session = sessions.join("must-not-be-collected.jsonl");
        let context = serde_json::json!({
            "timestamp": chrono::DateTime::<chrono::Utc>::from_timestamp(now, 0).unwrap().to_rfc3339(),
            "type": "turn_context",
            "model": "gpt-5.6-sol"
        });
        let tokens = serde_json::json!({
            "timestamp": chrono::DateTime::<chrono::Utc>::from_timestamp(now, 0).unwrap().to_rfc3339(),
            "type": "token_count",
            "payload": {"info": {"total_token_usage": {
                "total_tokens": 999, "input_tokens": 900,
                "cached_input_tokens": 800, "output_tokens": 99
            }}}
        });
        fs::write(&session, format!("{}\n{}\n", context, tokens)).unwrap();
        let data_dir = root.join("data");
        let old_home = std::env::var_os("CODEX_HOME");
        let old_data = std::env::var_os("CODEX_INFO_DATA_DIR");
        std::env::set_var("CODEX_HOME", &codex_home);
        std::env::set_var("CODEX_INFO_DATA_DIR", &data_dir);
        persist_reset_hint(reset_at, 604_800).unwrap();

        let mut writer = RecorderWorker::start().unwrap();
        assert!(writer.is_active());
        assert!(writer.owner_is_live());
        let idle_state = read_recorder_state().unwrap().unwrap();
        assert_eq!(idle_state.schema, RECORDER_STATE_SCHEMA);
        assert_eq!(idle_state.write_state, RecorderWriteState::IdleNoAccount);
        assert!(idle_state.partition_id_hash.is_none());
        writer.probe().unwrap();
        assert_eq!(
            read_recorder_state().unwrap().unwrap().updated_at_unix,
            idle_state.updated_at_unix,
            "liveness probe must not create a durable-state write"
        );
        let state_path = recorder_state_path().unwrap();
        let state_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        let mut state_keys = state_json
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        state_keys.sort();
        assert_eq!(
            state_keys,
            vec![
                "collector_epoch",
                "cycle_seq",
                "data_generation",
                "last_commit_unix",
                "owner_nonce",
                "partition_id_hash",
                "pid",
                "process_starttime",
                "schema",
                "updated_at_unix",
                "write_state",
            ]
        );
        assert!(state_json.get("updated_at").is_none());
        let mut duplicate = RecorderWorker::start().unwrap();
        assert!(!duplicate.is_active());
        let account_key = crate::account_scope::AccountKey::synthetic_preview("daemon-account");
        let partition = crate::account_scope::resolve_partition(&data_dir, &account_key).unwrap();
        writer
            .activate_partition(partition.clone(), chrono::Utc::now())
            .unwrap();

        let expected = crate::usage_store::UsageHistorySample {
            timestamp: now + 60,
            reset_at,
            remaining_percent: Some(41.0),
            sol_dollars: 323.674_247,
            terra_dollars: 2.5,
            luna_dollars: 1.25,
            sol_tokens: 12_345,
            terra_tokens: 6_789,
            luna_tokens: 321,
        };
        let marker = crate::usage_store::RecordedSessionSource {
            root_identity: "unix:10:20".into(),
            relative_path: "2026/09/session.jsonl".into(),
            file_bytes: 123,
            modified_nanos: 1_700_000_000_000_000_000,
            file_device: 10,
            file_inode: 30,
        };
        let commit_ack = writer
            .store_generation(
                partition.partition_id.clone(),
                RecorderGeneration {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: 1,
                    cycle_seq: 1,
                    samples: vec![expected.clone()],
                    observations: Vec::new(),
                    recorded_sessions: vec![marker.clone()],
                    session_checkpoints: vec![crate::usage_store::SessionCheckpoint {
                        root_identity: marker.root_identity.clone(),
                        relative_path: marker.relative_path.clone(),
                        file_device: marker.file_device,
                        file_inode: marker.file_inode,
                        committed_offset: marker.file_bytes,
                        discard_until_lf: false,
                        collector_epoch: 1,
                        cycle_seq: 1,
                        prefix_generation: 1,
                        prefix_sha256: "00".repeat(32),
                        fully_attributed_from_zero: true,
                        token_baseline_known: true,
                        last_task_running: Some(true),
                        last_model: Some("SOL".into()),
                        previous_total: 12_345,
                        previous_input: 10_000,
                        previous_cached_input: 2_000,
                        previous_output: 2_345,
                    }],
                    session_ranges: Vec::new(),
                    session_model_totals: vec![crate::usage_store::SessionModelTotal {
                        model: "SOL".into(),
                        total_tokens: 12_345,
                        input_tokens: 10_000,
                        cached_input_tokens: 2_000,
                        output_tokens: 2_345,
                    }],
                    history_continuity_recovery: None,
                    bounded_source_rescan_complete: true,
                },
            )
            .unwrap();
        let ready_state = read_recorder_state().unwrap().unwrap();
        assert_eq!(ready_state.write_state, RecorderWriteState::Ready);
        assert_eq!(
            ready_state.data_generation,
            Some(commit_ack.data_generation)
        );
        assert_eq!(ready_state.cycle_seq, Some(commit_ack.cycle_seq));
        assert!(ready_state.collector_epoch.is_some());

        let database = partition.database_path.clone();
        let lock = daemon_lock_path().unwrap();
        let identity = partition.storage_identity();
        let store = UsageStore::open_read_only_partitioned(&database, &identity).unwrap();
        let samples = store.load_all().unwrap();
        assert_eq!(samples, vec![expected]);
        assert!(store.recorded_session_matches(&marker).unwrap());
        drop(store);
        writer
            .forget_recorded_sessions(partition.partition_id.clone(), vec![marker.clone()])
            .unwrap();
        assert!(
            !UsageStore::open_read_only_partitioned(&database, &identity)
                .unwrap()
                .recorded_session_matches(&marker)
                .unwrap()
        );
        assert!(lock.exists());

        // A rejected optional continuity recovery must not block the exact
        // ordinary generation carried beside it. This fixture intentionally
        // supplies no durable continuity row, so applying the offset fails.
        let fallback_sample = crate::usage_store::UsageHistorySample {
            timestamp: now + 120,
            reset_at,
            remaining_percent: Some(40.0),
            sol_dollars: 324.0,
            terra_dollars: 2.5,
            luna_dollars: 1.25,
            sol_tokens: 13_000,
            terra_tokens: 6_789,
            luna_tokens: 321,
        };
        let fallback_totals = vec![crate::usage_store::SessionModelTotal {
            model: "SOL".into(),
            total_tokens: 13_000,
            input_tokens: 10_500,
            cached_input_tokens: 2_100,
            output_tokens: 2_500,
        }];
        let mut recovered_sample = fallback_sample.clone();
        recovered_sample.sol_tokens = 14_000;
        recovered_sample.sol_dollars = 332.65;
        let fallback_ack = writer
            .store_generation(
                partition.partition_id.clone(),
                RecorderGeneration {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: 1,
                    cycle_seq: 2,
                    samples: vec![recovered_sample],
                    observations: Vec::new(),
                    recorded_sessions: Vec::new(),
                    session_checkpoints: Vec::new(),
                    session_ranges: Vec::new(),
                    session_model_totals: vec![crate::usage_store::SessionModelTotal {
                        model: "SOL".into(),
                        total_tokens: 14_000,
                        input_tokens: 11_300,
                        cached_input_tokens: 2_400,
                        output_tokens: 2_700,
                    }],
                    history_continuity_recovery: Some(
                        crate::usage_store::HistoryContinuityModelRecovery {
                            authority: crate::usage_store::HistoryContinuityRecovery {
                                source_fingerprint: "aa".repeat(8),
                                source_rows: 1,
                                boundary_timestamp: now,
                                reset_at,
                                sol_dollars: 8.65,
                                terra_dollars: 0.0,
                                luna_dollars: 0.0,
                                sol_tokens: 1_000,
                                terra_tokens: 0,
                                luna_tokens: 0,
                            },
                            model_totals: vec![crate::usage_store::SessionModelTotal {
                                model: "SOL".into(),
                                total_tokens: 1_000,
                                input_tokens: 800,
                                cached_input_tokens: 300,
                                output_tokens: 200,
                            }],
                            fallback_samples: vec![fallback_sample.clone()],
                            fallback_model_totals: fallback_totals.clone(),
                        },
                    ),
                    bounded_source_rescan_complete: false,
                },
            )
            .unwrap();
        assert_eq!(fallback_ack.fallback_model_totals, Some(fallback_totals));
        assert!(UsageStore::open_read_only_partitioned(&database, &identity)
            .unwrap()
            .load_all()
            .unwrap()
            .contains(&fallback_sample));

        let account_b = crate::account_scope::AccountKey::synthetic_preview("daemon-account-b");
        let partition_b = crate::account_scope::resolve_partition(&data_dir, &account_b).unwrap();
        writer
            .activate_partition(partition_b.clone(), chrono::Utc::now())
            .unwrap();
        assert!(!partition.writer_lock_path.exists());
        assert!(partition_b.writer_lock_path.is_file());
        assert!(writer
            .store_generation(
                partition.partition_id.clone(),
                RecorderGeneration {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: 2,
                    cycle_seq: 1,
                    samples: Vec::new(),
                    observations: Vec::new(),
                    recorded_sessions: Vec::new(),
                    session_checkpoints: Vec::new(),
                    session_ranges: Vec::new(),
                    session_model_totals: Vec::new(),
                    history_continuity_recovery: None,
                    bounded_source_rescan_complete: false,
                },
            )
            .is_err());
        assert!(UsageStore::open_read_only_partitioned(
            &partition_b.database_path,
            &partition_b.storage_identity(),
        )
        .unwrap()
        .load_all()
        .unwrap()
        .is_empty());
        writer
            .activate_partition(partition.clone(), chrono::Utc::now())
            .unwrap();
        assert!(partition.writer_lock_path.is_file());
        assert!(!partition_b.writer_lock_path.exists());
        assert!(partition
            .database_path
            .with_extension("sqlite3.bak.1")
            .is_file());
        assert_eq!(
            crate::account_scope::resolve_partition(&data_dir, &account_b).unwrap(),
            partition_b
        );

        duplicate.shutdown();
        writer.shutdown();
        assert!(!lock.exists());
        assert!(!partition.writer_lock_path.exists());
        match old_home {
            Some(value) => std::env::set_var("CODEX_HOME", value),
            None => std::env::remove_var("CODEX_HOME"),
        }
        match old_data {
            Some(value) => std::env::set_var("CODEX_INFO_DATA_DIR", value),
            None => std::env::remove_var("CODEX_INFO_DATA_DIR"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recorder_production_source_result_reaches_all_gap_states_without_session_quota_proof() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = temp_root("production-gap-source");
        let data_dir = root.join("data");
        fs::create_dir_all(&data_dir).unwrap();
        let old_data = std::env::var_os("CODEX_INFO_DATA_DIR");
        std::env::set_var("CODEX_INFO_DATA_DIR", &data_dir);

        let account = crate::account_scope::AccountKey::synthetic_preview("production-gap");
        let partition = crate::account_scope::resolve_partition(&data_dir, &account).unwrap();
        let reset_at = 1_800_604_800;
        let stopped_at_monotonic_ns = monotonic_now_ns();
        assert!(stopped_at_monotonic_ns > 0);
        let pending_gap = |id: char, start_at: i64, end_at: i64, gap_reset_at: i64| RecorderGap {
            gap_id: id.to_string().repeat(32),
            partition_id: partition.partition_id.clone(),
            source_identity_before: "resident:production-gap".into(),
            source_identity_after: "unresolved".into(),
            cursor_before: "generation-1".into(),
            cursor_after: "unresolved".into(),
            stopped_at_monotonic_ns,
            resumed_at_monotonic_ns: None,
            start_at,
            end_at,
            reset_at: Some(gap_reset_at),
            reason: "daemon_stop_unrecoverable".into(),
            state: "pending".into(),
            owner_collector_epoch: 1,
            confirmation_cycle_seq: 1,
        };

        let mut writer = RecorderWorker::start().unwrap();
        writer
            .activate_partition(partition.clone(), chrono::Utc::now())
            .unwrap();
        let mut store =
            UsageStore::open_partitioned(&partition.database_path, &partition.storage_identity())
                .unwrap();
        for gap in [
            pending_gap('a', 1_800_000_000, 1_800_000_180, reset_at),
            pending_gap('b', 1_800_000_240, 1_800_000_300, reset_at),
            pending_gap('c', 1_800_000_360, 1_800_000_420, reset_at + 604_800),
        ] {
            store.begin_recorder_gap(&gap).unwrap();
        }
        drop(store);

        let quota_sample = |timestamp| UsageHistorySample {
            timestamp,
            reset_at,
            remaining_percent: Some(80.0),
            sol_dollars: 1.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: 10,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        assert!(quota_source_rescan_is_closed(
            &[
                quota_sample(1_800_000_060),
                quota_sample(1_800_000_120),
                quota_sample(1_800_000_180),
            ],
            reset_at,
            true,
        ));
        writer
            .store_generation(
                partition.partition_id.clone(),
                RecorderGeneration {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: 2,
                    cycle_seq: 2,
                    samples: vec![
                        quota_sample(1_800_000_060),
                        quota_sample(1_800_000_120),
                        quota_sample(1_800_000_180),
                    ],
                    observations: Vec::new(),
                    recorded_sessions: Vec::new(),
                    session_checkpoints: Vec::new(),
                    session_ranges: Vec::new(),
                    session_model_totals: Vec::new(),
                    history_continuity_recovery: None,
                    bounded_source_rescan_complete: true,
                },
            )
            .unwrap();
        let store = UsageStore::open_read_only_partitioned(
            &partition.database_path,
            &partition.storage_identity(),
        )
        .unwrap();
        let first_states = store
            .load_recorder_gaps()
            .unwrap()
            .into_iter()
            .map(|gap| (gap.gap_id.chars().next().unwrap(), gap.state))
            .collect::<std::collections::BTreeMap<_, _>>();
        let recovered_gap = store
            .load_recorder_gaps()
            .unwrap()
            .into_iter()
            .find(|gap| gap.gap_id == "a".repeat(32))
            .unwrap();
        assert_eq!(recovered_gap.start_at, 1_800_000_000);
        assert_eq!(recovered_gap.end_at, 1_800_000_180);
        assert_eq!(
            first_states.get(&'a').map(String::as_str),
            Some("recovered")
        );
        assert_eq!(
            first_states.get(&'b').map(String::as_str),
            Some("confirmed")
        );
        assert_eq!(first_states.get(&'c').map(String::as_str), Some("rejected"));
        assert_eq!(store.load_confirmed_recorder_gaps().unwrap().len(), 1);
        drop(store);

        let mut store =
            UsageStore::open_partitioned(&partition.database_path, &partition.storage_identity())
                .unwrap();
        store
            .begin_recorder_gap(&pending_gap('d', 1_800_000_480, 1_800_000_540, reset_at))
            .unwrap();
        drop(store);

        // A null-quota session backfill is ignored for source closure, while
        // the independently admitted quota observation still confirms the
        // interval. The durable backfill row remains quota-null.
        let mut session_backfill = quota_sample(1_800_001_080);
        session_backfill.remaining_percent = None;
        writer
            .store_generation(
                partition.partition_id.clone(),
                RecorderGeneration {
                    reset_at,
                    window_seconds: 604_800,
                    collector_epoch: 3,
                    cycle_seq: 1,
                    samples: vec![quota_sample(1_800_001_020), session_backfill],
                    observations: Vec::new(),
                    recorded_sessions: Vec::new(),
                    session_checkpoints: Vec::new(),
                    session_ranges: Vec::new(),
                    session_model_totals: Vec::new(),
                    history_continuity_recovery: None,
                    bounded_source_rescan_complete: true,
                },
            )
            .unwrap();
        let store = UsageStore::open_read_only_partitioned(
            &partition.database_path,
            &partition.storage_identity(),
        )
        .unwrap();
        let confirmed = store
            .load_recorder_gaps()
            .unwrap()
            .into_iter()
            .find(|gap| gap.gap_id == "d".repeat(32))
            .expect("session backfill gap");
        assert_eq!(confirmed.state, "confirmed");
        assert_eq!(store.load_confirmed_recorder_gaps().unwrap().len(), 2);
        assert!(store
            .load_all()
            .unwrap()
            .iter()
            .any(|sample| sample.timestamp == 1_800_001_080 && sample.remaining_percent.is_none()));
        drop(store);
        writer.shutdown();
        match old_data {
            Some(value) => std::env::set_var("CODEX_INFO_DATA_DIR", value),
            None => std::env::remove_var("CODEX_INFO_DATA_DIR"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn allocated_candidate_and_renamed_final_resume_without_a_stale_writer_lock() {
        for phase in ["candidate", "renamed-final"] {
            let root = temp_root(phase);
            let account = crate::account_scope::AccountKey::synthetic_preview("crash-account");
            let partition = crate::account_scope::resolve_partition(&root, &account).unwrap();
            let identity = partition.storage_identity();
            let candidate =
                UsageStore::create_partitioned(&partition.candidate_path, &identity).unwrap();
            drop(candidate);
            if phase == "renamed-final" {
                fs::rename(&partition.candidate_path, &partition.database_path).unwrap();
            }
            assert!(!partition.writer_lock_path.exists());

            let active = activate_account_partition(partition.clone(), chrono::Utc::now()).unwrap();
            assert!(partition.database_path.is_file());
            assert!(!partition.candidate_path.exists());
            assert!(partition.writer_lock_path.is_file());
            drop(active);
            assert!(!partition.writer_lock_path.exists());
            assert_eq!(
                crate::account_scope::resolve_partition(&root, &account).unwrap(),
                partition
            );
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn recorder_probe_does_not_misclassify_a_busy_writer_as_stopped() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_data = std::env::var_os("CODEX_INFO_DATA_DIR");
        let root = temp_root("probe-busy-writer");
        let data_dir = root.join("data");
        fs::create_dir_all(&data_dir).unwrap();
        std::env::set_var("CODEX_INFO_DATA_DIR", &data_dir);

        let account = crate::account_scope::AccountKey::synthetic_preview("probe-account");
        let partition = crate::account_scope::resolve_partition(&data_dir, &account).unwrap();
        let mut writer = RecorderWorker::start().unwrap();
        writer
            .activate_partition(partition.clone(), chrono::Utc::now())
            .unwrap();

        let blocker = rusqlite::Connection::open(&partition.database_path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let reset_at = unix_now().max(1) + 3_600;
        let generation = RecorderGeneration {
            reset_at,
            window_seconds: 604_800,
            collector_epoch: 1,
            cycle_seq: 1,
            samples: Vec::new(),
            observations: Vec::new(),
            recorded_sessions: Vec::new(),
            session_checkpoints: Vec::new(),
            session_ranges: Vec::new(),
            session_model_totals: Vec::new(),
            history_continuity_recovery: None,
            bounded_source_rescan_complete: false,
        };

        std::thread::scope(|scope| {
            let store =
                scope.spawn(|| writer.store_generation(partition.partition_id.clone(), generation));
            std::thread::sleep(Duration::from_millis(100));
            writer
                .probe()
                .expect("a live writer waiting on SQLite must not be reported dead");
            blocker.execute_batch("ROLLBACK").unwrap();
            store.join().unwrap().unwrap();
        });

        writer.shutdown();
        match old_data {
            Some(value) => std::env::set_var("CODEX_INFO_DATA_DIR", value),
            None => std::env::remove_var("CODEX_INFO_DATA_DIR"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recorder_failure_injections_are_finite_and_do_not_retry_in_one_callback() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_data = std::env::var_os("CODEX_INFO_DATA_DIR");
        for mode in ["busy", "fatal", "full", "readonly", "worker-death"] {
            let root = temp_root(&format!("failure-{mode}"));
            let data_dir = root.join("data");
            fs::create_dir_all(&data_dir).unwrap();
            std::env::set_var("CODEX_INFO_DATA_DIR", &data_dir);
            std::env::set_var("CODEX_INFO_RECORDER_FAILURE", mode);
            let account = crate::account_scope::AccountKey::synthetic_preview("failure-account");
            let partition = crate::account_scope::resolve_partition(&data_dir, &account).unwrap();
            let reset_at = unix_now().max(1) + 3_600;
            let generation = || RecorderGeneration {
                reset_at,
                window_seconds: 604_800,
                collector_epoch: 1,
                cycle_seq: 1,
                samples: Vec::new(),
                observations: Vec::new(),
                recorded_sessions: Vec::new(),
                session_checkpoints: Vec::new(),
                session_ranges: Vec::new(),
                session_model_totals: Vec::new(),
                history_continuity_recovery: None,
                bounded_source_rescan_complete: false,
            };
            let mut writer = RecorderWorker::start().unwrap();
            if mode != "worker-death" {
                writer
                    .activate_partition(partition.clone(), chrono::Utc::now())
                    .unwrap();
            }
            let first = writer.store_generation(partition.partition_id.clone(), generation());
            assert!(
                first.is_err(),
                "injection mode {mode} unexpectedly succeeded"
            );
            if mode != "worker-death" {
                let degraded_updated_at = read_recorder_state().unwrap().unwrap().updated_at_unix;
                assert_eq!(
                    read_recorder_state().unwrap().unwrap().write_state,
                    RecorderWriteState::Degraded
                );
                writer.probe().unwrap();
                assert_eq!(
                    read_recorder_state().unwrap().unwrap().updated_at_unix,
                    degraded_updated_at,
                    "liveness probe must not hide degraded state with a heartbeat write"
                );
            }
            if mode == "busy" {
                let degraded = read_recorder_state().unwrap().unwrap();
                assert_eq!(degraded.write_state, RecorderWriteState::Degraded);
                std::env::remove_var("CODEX_INFO_RECORDER_FAILURE");
                let ack = writer
                    .store_generation(partition.partition_id.clone(), generation())
                    .unwrap();
                assert!(ack.data_generation > 0);
                assert_eq!(
                    read_recorder_state().unwrap().unwrap().write_state,
                    RecorderWriteState::Ready
                );
            } else if mode == "worker-death" {
                // The command response disconnects before the worker's join
                // handle is guaranteed to report finished. Observe the same
                // bounded one-second window used by the resident probe tick.
                let deadline = std::time::Instant::now() + Duration::from_secs(1);
                while writer.probe().is_ok() {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "stopped recorder worker remained live past one probe tick"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
            } else {
                assert!(writer.probe().is_ok());
            }
            writer.shutdown();
            std::env::remove_var("CODEX_INFO_RECORDER_FAILURE");
            let _ = fs::remove_dir_all(root);
        }
        match old_data {
            Some(value) => std::env::set_var("CODEX_INFO_DATA_DIR", value),
            None => std::env::remove_var("CODEX_INFO_DATA_DIR"),
        }
    }
}

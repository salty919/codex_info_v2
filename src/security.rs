// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! Security boundaries shared by the Codex process, storage, and UI layers.
//!
//! The functions in this module deliberately return redacted, categorical
//! errors. Callers can log the category, but must not accidentally expose an
//! authentication URL, a JSON-RPC payload, or a filesystem path to the UI.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Component, Path, PathBuf};
use std::process::Child;
#[cfg(test)]
use std::process::ExitStatus;
use std::time::Duration;

use url::Url;

// External UI scalar limits (SECURITY.md control 7).
pub const MAX_EMAIL_SCALARS: usize = 254;
pub const MAX_PLAN_SCALARS: usize = 64;
pub const MAX_MODEL_SCALARS: usize = 128;
pub const MAX_THREAD_TITLE_SCALARS: usize = 512;
pub const MAX_LIMIT_NAME_SCALARS: usize = 96;
pub const MAX_STATUS_SCALARS: usize = 160;
pub const MAX_ACCOUNT_ACTIVITY_LABEL_SCALARS: usize = 24;
pub const MAX_AUTH_URL_SCALARS: usize = 2_048;

// Session traversal and JSON-RPC budgets (SECURITY.md controls 4 and 5).
pub const MAX_SESSION_DEPTH: usize = 8;
pub const MAX_SESSION_FILES: usize = 4_096;
// A selected prefix may consist of one long-running session. Do not reject
// that session at a smaller, independent limit before reading its delta.
pub const MAX_SESSION_FILE_BYTES: u64 = MAX_SESSION_TOTAL_BYTES;
pub const MAX_JSONL_LINE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SESSION_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_RPC_LINE_BYTES: usize = MAX_JSONL_LINE_BYTES;
pub const MAX_RPC_IGNORED_MESSAGES: usize = 1_024;
pub const RPC_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityErrorKind {
    InvalidInput,
    InvalidUrl,
    TooLong,
    UnsafePath,
    UnsafeExecutable,
    LimitExceeded,
    Unterminated,
    #[cfg(test)]
    InvalidNumber,
    #[cfg(test)]
    SecretValue,
    Io,
    Parse,
    Child,
}

/// A deliberately generic error. No untrusted value is retained in the
/// error, so `Display` is safe for a status label or log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecurityError {
    kind: SecurityErrorKind,
}

impl SecurityError {
    pub const fn new(kind: SecurityErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> SecurityErrorKind {
        self.kind
    }

    pub const fn message(self) -> &'static str {
        match self.kind {
            SecurityErrorKind::InvalidInput => "invalid input",
            SecurityErrorKind::InvalidUrl => "invalid authentication URL",
            SecurityErrorKind::TooLong => "input exceeds security limit",
            SecurityErrorKind::UnsafePath => "unsafe path",
            SecurityErrorKind::UnsafeExecutable => "unsafe executable",
            SecurityErrorKind::LimitExceeded => "security limit exceeded",
            SecurityErrorKind::Unterminated => "unterminated protected record",
            #[cfg(test)]
            SecurityErrorKind::InvalidNumber => "invalid numeric value",
            #[cfg(test)]
            SecurityErrorKind::SecretValue => "secret value rejected",
            SecurityErrorKind::Io => "protected file operation failed",
            SecurityErrorKind::Parse => "protected data could not be parsed",
            SecurityErrorKind::Child => "protected process operation failed",
        }
    }
}

impl fmt::Display for SecurityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for SecurityError {}

impl From<io::Error> for SecurityError {
    fn from(_: io::Error) -> Self {
        Self::new(SecurityErrorKind::Io)
    }
}

impl From<url::ParseError> for SecurityError {
    fn from(_: url::ParseError) -> Self {
        Self::new(SecurityErrorKind::InvalidUrl)
    }
}

/// Shorten by Unicode scalar values, never by bytes.
pub fn shorten_unicode(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    match limit {
        0 => String::new(),
        1 => "…".to_owned(),
        _ => value
            .chars()
            .take(limit - 1)
            .chain(std::iter::once('…'))
            .collect(),
    }
}

/// Return whether a scalar is forbidden in an external UI string.
fn is_forbidden_ui_scalar(value: char) -> bool {
    matches!(
        value,
        '\u{0000}'..='\u{001F}'
            | '\u{007F}'..='\u{009F}'
            | '\u{061C}'
            | '\u{200E}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// Normalize an external UI string to one line with collapsed ASCII spaces.
fn normalize_ui_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut previous_was_space = false;

    for scalar in value.chars() {
        let scalar = if is_forbidden_ui_scalar(scalar) {
            ' '
        } else {
            scalar
        };
        if scalar == ' ' {
            if previous_was_space {
                continue;
            }
            previous_was_space = true;
        } else {
            previous_was_space = false;
        }
        normalized.push(scalar);
    }

    normalized
}

/// Bound text before it is stored or rendered. NUL is normalized to a space.
pub fn bounded_ui_text(value: &str, limit: usize) -> Result<String, SecurityError> {
    Ok(shorten_unicode(&normalize_ui_text(value), limit))
}

pub fn bounded_email(value: &str) -> Result<String, SecurityError> {
    bounded_ui_text(value, MAX_EMAIL_SCALARS)
}

fn bounded_contract_text(value: &str, limit: usize) -> Result<String, SecurityError> {
    let normalized = normalize_ui_text(value);
    if normalized.chars().count() > limit {
        return Err(SecurityError::new(SecurityErrorKind::TooLong));
    }
    Ok(normalized)
}

pub fn bounded_plan(value: &str) -> Result<String, SecurityError> {
    bounded_contract_text(value, MAX_PLAN_SCALARS)
}

pub fn bounded_model(value: &str) -> Result<String, SecurityError> {
    bounded_contract_text(value, MAX_MODEL_SCALARS)
}

pub fn bounded_model_label(value: &str) -> Result<String, SecurityError> {
    bounded_ui_text(value, MAX_ACCOUNT_ACTIVITY_LABEL_SCALARS)
}

pub fn bounded_thread_title(value: &str) -> Result<String, SecurityError> {
    bounded_ui_text(value, MAX_THREAD_TITLE_SCALARS)
}

pub fn bounded_limit_name(value: &str) -> Result<String, SecurityError> {
    bounded_ui_text(value, MAX_LIMIT_NAME_SCALARS)
}

pub fn bounded_status(value: &str) -> Result<String, SecurityError> {
    bounded_ui_text(value, MAX_STATUS_SCALARS)
}

/// Reject a value at a secret boundary rather than trying to redact it after
/// it has reached a UI/logging layer. Empty optional fields are acceptable.
#[cfg(test)]
pub fn validate_secret_text(value: &str) -> Result<(), SecurityError> {
    if value.is_empty() {
        Ok(())
    } else {
        Err(SecurityError::new(SecurityErrorKind::SecretValue))
    }
}

#[cfg(test)]
pub fn validate_non_negative_i64(value: i64) -> Result<u64, SecurityError> {
    u64::try_from(value).map_err(|_| SecurityError::new(SecurityErrorKind::InvalidNumber))
}

#[cfg(test)]
pub fn parse_non_negative_u64(value: &str) -> Result<u64, SecurityError> {
    if value.is_empty() || value.starts_with(['+', '-']) {
        return Err(SecurityError::new(SecurityErrorKind::InvalidNumber));
    }
    value
        .parse::<u64>()
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidNumber))
}

#[cfg(test)]
pub fn validate_non_negative_f64(value: f64) -> Result<f64, SecurityError> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(SecurityError::new(SecurityErrorKind::InvalidNumber))
    }
}

fn valid_dns_label(label: &str) -> bool {
    if label.is_empty() || label.len() > 63 || !label.is_ascii() {
        return false;
    }
    if label.starts_with('-') || label.ends_with('-') {
        return false;
    }
    label
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn allowed_auth_host(host: &str) -> bool {
    if host.is_empty() || host.ends_with('.') || !host.is_ascii() {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.iter().any(|label| !valid_dns_label(label)) {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    ["openai.com", "chatgpt.com"]
        .iter()
        .any(|base| lower == *base || lower.ends_with(&format!(".{base}")))
}

/// Parse and validate an authentication URL without suffix-substring or
/// shell-based checks.
pub fn validate_auth_url(value: &str) -> Result<Url, SecurityError> {
    if value.chars().count() > MAX_AUTH_URL_SCALARS {
        return Err(SecurityError::new(SecurityErrorKind::TooLong));
    }
    let url = Url::parse(value).map_err(|_| SecurityError::new(SecurityErrorKind::InvalidUrl))?;
    if !url.scheme().eq_ignore_ascii_case("https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !allowed_auth_host(url.host_str().unwrap_or_default())
        || matches!(url.port(), Some(port) if port != 443)
    {
        return Err(SecurityError::new(SecurityErrorKind::InvalidUrl));
    }
    Ok(url)
}

fn reject_parent_components(path: &Path) -> Result<(), SecurityError> {
    if !path.is_absolute() {
        return Err(SecurityError::new(SecurityErrorKind::UnsafePath));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(SecurityError::new(SecurityErrorKind::UnsafePath));
    }
    Ok(())
}

/// Verify every existing component without following a symlink. Returning
/// the original path keeps this helper useful for both roots and candidates.
fn inspect_no_symlink_components(path: &Path) -> Result<(), SecurityError> {
    reject_parent_components(path)?;
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => return Err(SecurityError::new(SecurityErrorKind::UnsafePath)),
            Component::Normal(name) => {
                current.push(name);
                let metadata = fs::symlink_metadata(&current)
                    .map_err(|_| SecurityError::new(SecurityErrorKind::UnsafePath))?;
                if metadata.file_type().is_symlink() {
                    return Err(SecurityError::new(SecurityErrorKind::UnsafePath));
                }
            }
        }
    }
    Ok(())
}

/// Validate and canonicalize a configured absolute directory root.
pub fn validate_absolute_root(root: &Path) -> Result<PathBuf, SecurityError> {
    inspect_no_symlink_components(root)?;
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| SecurityError::new(SecurityErrorKind::UnsafePath))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(SecurityError::new(SecurityErrorKind::UnsafePath));
    }
    fs::canonicalize(root).map_err(|_| SecurityError::new(SecurityErrorKind::UnsafePath))
}

/// Return a canonical regular file only when it remains below `root`.
pub fn canonical_regular_file_under(
    root: &Path,
    candidate: &Path,
) -> Result<PathBuf, SecurityError> {
    let canonical_root = validate_absolute_root(root)?;
    inspect_no_symlink_components(candidate)?;
    let candidate_metadata = fs::symlink_metadata(candidate)
        .map_err(|_| SecurityError::new(SecurityErrorKind::UnsafePath))?;
    if candidate_metadata.file_type().is_symlink() || !candidate_metadata.is_file() {
        return Err(SecurityError::new(SecurityErrorKind::UnsafePath));
    }
    let canonical_candidate = fs::canonicalize(candidate)
        .map_err(|_| SecurityError::new(SecurityErrorKind::UnsafePath))?;
    if canonical_candidate == canonical_root
        || canonical_candidate.strip_prefix(&canonical_root).is_err()
    {
        return Err(SecurityError::new(SecurityErrorKind::UnsafePath));
    }
    let canonical_metadata = fs::symlink_metadata(&canonical_candidate)
        .map_err(|_| SecurityError::new(SecurityErrorKind::UnsafePath))?;
    if canonical_metadata.file_type().is_symlink() || !canonical_metadata.is_file() {
        return Err(SecurityError::new(SecurityErrorKind::UnsafePath));
    }
    Ok(canonical_candidate)
}

#[cfg(unix)]
fn has_unsafe_write_bits(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o022 != 0
}

#[cfg(unix)]
fn unescape_mount_path(value: &str) -> PathBuf {
    PathBuf::from(
        value
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\"),
    )
}

/// WSL DrvFS reports synthetic 0777 modes even when Windows ACLs restrict a
/// path. Treat such a component as same-UID trusted only on the actual DrvFS
/// mount; ordinary Unix world/group-writable paths remain rejected.
#[cfg(unix)]
fn is_same_uid_drvfs_path(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Ok(current_uid) = fs::metadata("/proc/self").map(|metadata| metadata.uid()) else {
        return false;
    };
    if !matches!(
        fs::metadata(path).map(|metadata| metadata.uid()),
        Ok(owner) if owner == current_uid
    ) {
        return false;
    }
    let Ok(mounts) = fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    mounts.lines().any(|line| {
        let Some((left, right)) = line.split_once(" - ") else {
            return false;
        };
        let Some(mount_point) = left.split_ascii_whitespace().nth(4) else {
            return false;
        };
        right.starts_with("9p ")
            && right.contains("aname=drvfs")
            && path.starts_with(unescape_mount_path(mount_point))
    })
}

#[cfg(not(unix))]
fn is_same_uid_drvfs_path(_path: &Path) -> bool {
    false
}

#[cfg(not(unix))]
fn has_unsafe_write_bits(metadata: &fs::Metadata) -> bool {
    !metadata.permissions().readonly()
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(metadata: &fs::Metadata) -> bool {
    !metadata.permissions().readonly()
}

fn validate_canonical_executable(path: &Path) -> Result<PathBuf, SecurityError> {
    let metadata =
        fs::metadata(path).map_err(|_| SecurityError::new(SecurityErrorKind::UnsafeExecutable))?;
    if !metadata.is_file()
        || !is_executable(&metadata)
        || (has_unsafe_write_bits(&metadata) && !is_same_uid_drvfs_path(path))
    {
        return Err(SecurityError::new(SecurityErrorKind::UnsafeExecutable));
    }

    let mut current = path;
    loop {
        let parent_metadata = fs::metadata(current)
            .map_err(|_| SecurityError::new(SecurityErrorKind::UnsafeExecutable))?;
        if !parent_metadata.is_dir() && current != path {
            return Err(SecurityError::new(SecurityErrorKind::UnsafeExecutable));
        }
        if has_unsafe_write_bits(&parent_metadata) && !is_same_uid_drvfs_path(current) {
            return Err(SecurityError::new(SecurityErrorKind::UnsafeExecutable));
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent;
    }
    Ok(path.to_owned())
}

/// Resolve an explicitly configured executable. The final path component
/// must not itself be a symlink; PATH resolution below has a compatibility
/// exception for launchers installed as symlinks.
pub fn resolve_executable_path(path: &Path) -> Result<PathBuf, SecurityError> {
    if !path.is_absolute() {
        return Err(SecurityError::new(SecurityErrorKind::UnsafeExecutable));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| SecurityError::new(SecurityErrorKind::UnsafeExecutable))?;
    if metadata.file_type().is_symlink() {
        return Err(SecurityError::new(SecurityErrorKind::UnsafeExecutable));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|_| SecurityError::new(SecurityErrorKind::UnsafeExecutable))?;
    validate_canonical_executable(&canonical)
}

/// Resolve a plain executable name using an explicit PATH value. Relative
/// PATH entries are skipped because their meaning depends on mutable process
/// state; an existing candidate is canonicalized before permission checks.
pub fn resolve_executable_from_path<P: AsRef<OsStr>>(
    name: &str,
    path_value: P,
) -> Result<PathBuf, SecurityError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(SecurityError::new(SecurityErrorKind::UnsafeExecutable));
    }
    for directory in std::env::split_paths(path_value.as_ref()) {
        if !directory.is_absolute() {
            continue;
        }
        let candidate = directory.join(name);
        let metadata = match fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err(SecurityError::new(SecurityErrorKind::UnsafeExecutable)),
        };
        if !metadata.is_file() && !metadata.file_type().is_symlink() {
            return Err(SecurityError::new(SecurityErrorKind::UnsafeExecutable));
        }
        let canonical = fs::canonicalize(&candidate)
            .map_err(|_| SecurityError::new(SecurityErrorKind::UnsafeExecutable))?;
        return validate_canonical_executable(&canonical);
    }
    Err(SecurityError::new(SecurityErrorKind::UnsafeExecutable))
}

fn drain_until_newline<R: BufRead>(reader: &mut R) -> Result<bool, SecurityError> {
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|_| SecurityError::new(SecurityErrorKind::Io))?;
        if buffer.is_empty() {
            return Ok(false);
        }
        if let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
            reader.consume(position + 1);
            return Ok(true);
        }
        let length = buffer.len();
        reader.consume(length);
    }
}

/// Read one UTF-8 JSONL payload. The newline is not part of the returned
/// string. An oversized line is fully drained before the error is returned,
/// allowing the caller to recover and read the next line.
pub fn read_bounded_jsonl_record<R: BufRead>(
    reader: &mut R,
) -> Result<Option<(String, bool)>, SecurityError> {
    let mut line = Vec::with_capacity(MAX_JSONL_LINE_BYTES);
    let mut terminated = false;
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|_| SecurityError::new(SecurityErrorKind::Io))?;
        if buffer.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
            if line.len().saturating_add(position) > MAX_JSONL_LINE_BYTES {
                reader.consume(position + 1);
                return Err(SecurityError::new(SecurityErrorKind::LimitExceeded));
            }
            line.extend_from_slice(&buffer[..position]);
            reader.consume(position + 1);
            terminated = true;
            break;
        }
        if line.len().saturating_add(buffer.len()) > MAX_JSONL_LINE_BYTES {
            let length = buffer.len();
            reader.consume(length);
            let terminated = drain_until_newline(reader)?;
            return Err(SecurityError::new(if terminated {
                SecurityErrorKind::LimitExceeded
            } else {
                SecurityErrorKind::Unterminated
            }));
        }
        line.extend_from_slice(buffer);
        let length = buffer.len();
        reader.consume(length);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(|line| Some((line, terminated)))
        .map_err(|_| {
            SecurityError::new(if terminated {
                SecurityErrorKind::Parse
            } else {
                SecurityErrorKind::Unterminated
            })
        })
}

pub fn read_bounded_jsonl_line<R: BufRead>(
    reader: &mut R,
) -> Result<Option<String>, SecurityError> {
    match read_bounded_jsonl_record(reader)? {
        Some((line, true)) => Ok(Some(line)),
        Some((_line, false)) => Err(SecurityError::new(SecurityErrorKind::Unterminated)),
        None => Ok(None),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcLine(String);

impl RpcLine {
    pub fn new(value: impl Into<String>) -> Result<Self, SecurityError> {
        let value = value.into();
        if value.len() > MAX_RPC_LINE_BYTES {
            return Err(SecurityError::new(SecurityErrorKind::LimitExceeded));
        }
        Ok(Self(value))
    }

    pub fn read<R: BufRead>(reader: &mut R) -> Result<Option<Self>, SecurityError> {
        read_bounded_jsonl_line(reader)?.map(Self::new).transpose()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RpcLine {
    type Error = SecurityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for RpcLine {
    type Error = SecurityError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value.to_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpcLimits {
    pub max_line_bytes: usize,
    pub max_ignored_messages: usize,
    pub response_timeout: Duration,
}

impl RpcLimits {
    pub const fn standard() -> Self {
        Self {
            max_line_bytes: MAX_RPC_LINE_BYTES,
            max_ignored_messages: MAX_RPC_IGNORED_MESSAGES,
            response_timeout: RPC_RESPONSE_TIMEOUT,
        }
    }

    pub fn record_ignored_message(&self, count: &mut usize) -> Result<(), SecurityError> {
        if *count >= self.max_ignored_messages {
            return Err(SecurityError::new(SecurityErrorKind::LimitExceeded));
        }
        *count += 1;
        Ok(())
    }
}

impl Default for RpcLimits {
    fn default() -> Self {
        Self::standard()
    }
}

#[cfg(test)]
pub fn check_ignored_message_count(count: usize) -> Result<(), SecurityError> {
    if count <= MAX_RPC_IGNORED_MESSAGES {
        Ok(())
    } else {
        Err(SecurityError::new(SecurityErrorKind::LimitExceeded))
    }
}

/// Own a child until the caller explicitly reaps it. Drop is fail-safe: it
/// kills and waits, suppressing only cleanup errors because Drop cannot
/// report them without panicking.
pub struct ChildGuard {
    child: Option<Child>,
}

impl fmt::Debug for ChildGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChildGuard")
            .field("pid", &self.child.as_ref().map(Child::id))
            .finish()
    }
}

impl ChildGuard {
    pub fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    #[cfg(test)]
    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    #[cfg(test)]
    pub fn is_reaped(&self) -> bool {
        self.child.is_none()
    }

    pub fn child_mut(&mut self) -> Result<&mut Child, SecurityError> {
        self.child
            .as_mut()
            .ok_or_else(|| SecurityError::new(SecurityErrorKind::Child))
    }

    #[cfg(test)]
    pub fn reap(&mut self) -> Result<ExitStatus, SecurityError> {
        let result = self
            .child_mut()?
            .wait()
            .map_err(|_| SecurityError::new(SecurityErrorKind::Child));
        if result.is_ok() {
            self.child.take();
        }
        result
    }

    pub fn kill_and_reap(&mut self) -> Result<(), SecurityError> {
        let child = self.child_mut()?;
        let _ = child.kill();
        child
            .wait()
            .map_err(|_| SecurityError::new(SecurityErrorKind::Child))?;
        self.child.take();
        Ok(())
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Cursor;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let base = std::env::current_dir()
                .expect("test working directory")
                .join("target")
                .join("security-fixtures");
            fs::create_dir_all(&base).expect("security fixture base");
            let path = base.join(format!(
                "codex-info-security-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("unique security fixture");
            Self { path }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }

        fn file(&self, name: &str, mode: u32) -> PathBuf {
            let path = self.path(name);
            File::create(&path).expect("fixture file");
            set_mode(&path, mode);
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).expect("mode metadata").permissions();
        permissions.set_mode(mode);
        fs::set_permissions(path, permissions).expect("set fixture mode");
    }

    #[cfg(not(unix))]
    fn set_mode(_path: &Path, _mode: u32) {}

    #[test]
    fn unicode_shortening_is_scalar_based_and_handles_small_limits() {
        assert_eq!(shorten_unicode("あ😀é", 0), "");
        assert_eq!(shorten_unicode("あ😀é", 1), "…");
        assert_eq!(shorten_unicode("あ😀é", 2), "あ…");
        assert_eq!(shorten_unicode("あ😀é", 3), "あ😀é");
        assert_eq!(shorten_unicode("abcdef", 4), "abc…");
    }

    #[test]
    fn bounded_text_shortens_and_rejects_nul_without_leaking_values() {
        assert_eq!(shorten_unicode("😀😀😀", 2), "😀…");
        assert_eq!(bounded_status("bad\0value").unwrap(), "bad value");
        let error = validate_secret_text("token=untrusted").expect_err("secret rejected");
        assert!(error.to_string().len() < 64);
        assert!(!error.to_string().contains("token"));
    }

    #[test]
    fn numeric_boundaries_are_rejected_without_overflow() {
        assert_eq!(validate_non_negative_i64(0).expect("zero"), 0);
        assert!(validate_non_negative_i64(-1).is_err());
        assert_eq!(
            parse_non_negative_u64("18446744073709551615").expect("u64 max"),
            u64::MAX
        );
        assert!(parse_non_negative_u64("18446744073709551616").is_err());
        assert!(parse_non_negative_u64("-1").is_err());
        assert!(validate_non_negative_f64(f64::NAN).is_err());
        assert!(validate_non_negative_f64(-0.1).is_err());
    }

    #[test]
    fn auth_urls_enforce_exact_hosts_credentials_ports_fragments_and_lengths() {
        for accepted in [
            "https://openai.com",
            "https://chatgpt.com/path?q=1",
            "https://api.openai.com/v1",
            "https://foo.bar.chatgpt.com:443/login",
        ] {
            assert!(validate_auth_url(accepted).is_ok(), "accepted: {accepted}");
        }
        for rejected in [
            "http://openai.com",
            "https://evilopenai.com",
            "https://openai.com.evil.example",
            "https://openai.com@evil.example",
            "https://user:pass@openai.com",
            "https://openai.com:444",
            "https://openai.com/#fragment",
            "https://foo..openai.com",
            "https://openаi.com", // Cyrillic а, a visual lookalike.
        ] {
            assert!(validate_auth_url(rejected).is_err(), "rejected: {rejected}");
        }

        let prefix = "https://openai.com/";
        let exact = format!(
            "{prefix}{}",
            "a".repeat(MAX_AUTH_URL_SCALARS - prefix.chars().count())
        );
        assert_eq!(exact.chars().count(), MAX_AUTH_URL_SCALARS);
        assert!(validate_auth_url(&exact).is_ok());
        let over = format!("{exact}a");
        assert_eq!(over.chars().count(), MAX_AUTH_URL_SCALARS + 1);
        assert!(validate_auth_url(&over).is_err());
    }

    #[test]
    fn roots_and_rollouts_are_canonical_regular_contained_files() {
        let fixture = Fixture::new();
        let root = fixture.path("root");
        fs::create_dir(&root).expect("root");
        let nested = root.join("nested");
        fs::create_dir(&nested).expect("nested");
        let file = nested.join("rollout.jsonl");
        fs::write(&file, b"{}\n").expect("rollout");
        let outside = fixture.file("outside", 0o644);
        assert!(validate_absolute_root(&root).is_ok());
        assert!(canonical_regular_file_under(&root, &file).is_ok());
        assert!(canonical_regular_file_under(&root, &outside).is_err());
        assert!(canonical_regular_file_under(&root, Path::new("relative.jsonl")).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let linked = fixture.path("linked-rollout.jsonl");
            symlink(&file, &linked).expect("rollout symlink");
            assert!(canonical_regular_file_under(&root, &linked).is_err());
            let linked_root = fixture.path("linked-root");
            symlink(&root, &linked_root).expect("root symlink");
            assert!(validate_absolute_root(&linked_root).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn executable_modes_and_parent_permissions_are_enforced() {
        let fixture = Fixture::new();
        let executable = fixture.file("codex", 0o755);
        assert_eq!(
            resolve_executable_path(&executable).expect("safe executable"),
            executable
        );

        set_mode(&executable, 0o775);
        assert!(resolve_executable_path(&executable).is_err());
        set_mode(&executable, 0o644);
        assert!(resolve_executable_path(&executable).is_err());
        set_mode(&executable, 0o755);
        set_mode(&fixture.path, 0o777);
        assert!(resolve_executable_path(&executable).is_err());

        set_mode(&fixture.path, 0o755);
        let link = fixture.path("codex-link");
        std::os::unix::fs::symlink(&executable, &link).expect("launcher symlink");
        assert!(resolve_executable_path(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn path_resolution_accepts_installed_launcher_symlink_by_canonical_target() {
        // The installed Codex launcher is a symlink. PATH resolution accepts
        // that compatibility form, then validates its canonical target and
        // every canonical parent; explicit overrides still reject symlinks.
        let fixture = Fixture::new();
        let target = fixture.file("codex-target", 0o755);
        let link = fixture.path("codex");
        std::os::unix::fs::symlink(&target, &link).expect("PATH launcher symlink");
        let resolved = resolve_executable_from_path("codex", fixture.path.as_os_str())
            .expect("symlink PATH candidate");
        assert_eq!(
            resolved,
            fs::canonicalize(target).expect("canonical target")
        );
    }

    #[test]
    fn jsonl_reader_enforces_limit_drains_and_recovers() {
        let exact = "x".repeat(MAX_JSONL_LINE_BYTES);
        let over = "y".repeat(MAX_JSONL_LINE_BYTES + 1);
        let input = format!("{over}\nnext\n");
        let mut reader = Cursor::new(input.into_bytes());
        assert!(matches!(
            read_bounded_jsonl_line(&mut reader),
            Err(error) if error.kind() == SecurityErrorKind::LimitExceeded
        ));
        assert_eq!(
            read_bounded_jsonl_line(&mut reader).expect("recovery"),
            Some("next".to_owned())
        );

        let mut exact_reader = Cursor::new(format!("{exact}\nnext\n").into_bytes());
        assert_eq!(
            read_bounded_jsonl_line(&mut exact_reader)
                .expect("exact line")
                .expect("line")
                .len(),
            MAX_JSONL_LINE_BYTES
        );
        assert_eq!(
            read_bounded_jsonl_line(&mut exact_reader).expect("next"),
            Some("next".to_owned())
        );
    }

    #[test]
    fn jsonl_reader_rejects_unterminated_invalid_and_oversized_records() {
        let mut invalid = Cursor::new(vec![b'{', 0xff, b'}']);
        assert!(matches!(
            read_bounded_jsonl_record(&mut invalid),
            Err(error) if error.kind() == SecurityErrorKind::Unterminated
        ));

        let mut oversized = Cursor::new(vec![b'x'; MAX_JSONL_LINE_BYTES + 1]);
        assert!(matches!(
            read_bounded_jsonl_record(&mut oversized),
            Err(error) if error.kind() == SecurityErrorKind::Unterminated
        ));

        let mut valid = Cursor::new(b"{}".to_vec());
        assert!(matches!(
            read_bounded_jsonl_line(&mut valid),
            Err(error) if error.kind() == SecurityErrorKind::Unterminated
        ));
        let mut rpc = Cursor::new(br#"{"jsonrpc":"2.0"}"#.to_vec());
        assert!(matches!(
            RpcLine::read(&mut rpc),
            Err(error) if error.kind() == SecurityErrorKind::Unterminated
        ));
    }

    #[test]
    fn rpc_line_and_ignored_message_limits_have_exact_boundaries() {
        assert!(RpcLine::new("x".repeat(MAX_RPC_LINE_BYTES)).is_ok());
        assert!(RpcLine::new("x".repeat(MAX_RPC_LINE_BYTES + 1)).is_err());
        assert!(check_ignored_message_count(MAX_RPC_IGNORED_MESSAGES).is_ok());
        assert!(check_ignored_message_count(MAX_RPC_IGNORED_MESSAGES + 1).is_err());
        let limits = RpcLimits::standard();
        let mut count = 0;
        for _ in 0..MAX_RPC_IGNORED_MESSAGES {
            limits
                .record_ignored_message(&mut count)
                .expect("within budget");
        }
        assert_eq!(count, MAX_RPC_IGNORED_MESSAGES);
        assert!(limits.record_ignored_message(&mut count).is_err());
        assert_eq!(limits.response_timeout, Duration::from_secs(15));
    }

    #[test]
    fn child_guard_reaps_explicitly_and_on_drop() {
        let mut short = ChildGuard::new(Command::new("true").spawn().expect("true child"));
        let status = short.reap().expect("short child reap");
        assert!(status.success());
        assert!(short.is_reaped());

        let long = ChildGuard::new(
            Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("sleep child"),
        );
        let pid = long.id().expect("child pid");
        drop(long);
        let pid_text = pid.to_string();
        let probe = Command::new("kill")
            .args(["-0", pid_text.as_str()])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("kill probe");
        assert!(!probe.success(), "guard must reap pid {pid}");
    }
    #[test]
    fn security_s0_sv03_forbidden_scalar_table_is_single_line() {
        let ranges = [
            (0x0000_u32, 0x001F_u32),
            (0x007F_u32, 0x009F_u32),
            (0x061C_u32, 0x061C_u32),
            (0x200E_u32, 0x200F_u32),
            (0x2028_u32, 0x202E_u32),
            (0x2066_u32, 0x2069_u32),
        ];
        let mut forbidden = Vec::new();
        for (start, end) in ranges {
            for code in start..=end {
                forbidden.push(char::from_u32(code).expect("valid Unicode scalar"));
            }
        }

        let is_forbidden = |value: char| {
            matches!(
                value,
                '\u{0000}'..='\u{001F}'
                    | '\u{007F}'..='\u{009F}'
                    | '\u{061C}'
                    | '\u{200E}'..='\u{200F}'
                    | '\u{2028}'..='\u{202E}'
                    | '\u{2066}'..='\u{2069}'
            )
        };

        for scalar in &forbidden {
            assert_eq!(
                bounded_ui_text(&scalar.to_string(), 1)
                    .expect("bounded_ui_text should normalize forbidden scalar"),
                " "
            );
        }

        let controls: String = forbidden.iter().copied().collect();
        let normalized = bounded_ui_text(&format!("A {} {} B", controls, controls), 256)
            .expect("bounded_ui_text should normalize composite");
        assert_eq!(normalized, "A B");
        assert!(!normalized
            .chars()
            .any(|value| { is_forbidden(value) || value == '\n' || value == '\r' }));
    }

    #[test]
    fn security_s0_sv01_sv02_normalize_then_scalar_boundaries() {
        let limit = 4;

        let below = "é\n😀";
        assert_eq!(below.chars().count(), 3);
        assert_eq!(normalize_ui_text(below), "é 😀");
        assert_eq!(bounded_ui_text(below, limit).unwrap(), "é 😀");

        let at_limit = "é\n😀A";
        assert_eq!(at_limit.chars().count(), 4);
        assert_eq!(normalize_ui_text(at_limit), "é 😀A");
        assert_eq!(bounded_ui_text(at_limit, limit).unwrap(), "é 😀A");

        let above = "é\n \t😀AB";
        assert_eq!(normalize_ui_text(above), "é 😀AB");
        assert_eq!(normalize_ui_text(above).chars().count(), 5);
        let shortened = bounded_ui_text(above, limit).unwrap();
        assert_eq!(shortened, "é 😀…");
        assert_eq!(shortened.matches('…').count(), 1);
        assert!(!shortened.contains('\n'));
    }

    #[test]
    fn security_s0_r17c7_per_field_overlimit_policy() {
        let email_254 = "e".repeat(254);
        let email_255 = "e".repeat(255);
        assert_eq!(bounded_email(&email_254).unwrap(), email_254);
        assert_eq!(bounded_email(&email_255).unwrap(), "e".repeat(253) + "…");

        let plan_64 = "p".repeat(64);
        let plan_65 = "p".repeat(65);
        assert_eq!(bounded_plan(&plan_64).unwrap(), plan_64);
        assert_eq!(
            bounded_plan(&plan_65).unwrap_err().kind(),
            SecurityErrorKind::TooLong
        );

        let model_128 = "m".repeat(128);
        let model_129 = "m".repeat(129);
        assert_eq!(bounded_model(&model_128).unwrap(), model_128);
        assert_eq!(
            bounded_model(&model_129).unwrap_err().kind(),
            SecurityErrorKind::TooLong
        );

        let label_24 = "abcdefghijklmnopqrstuvwx";
        let label_25 = "abcdefghijklmnopqrstuvwxy";
        assert_eq!(bounded_model_label(label_24).unwrap(), label_24);
        assert_eq!(
            bounded_model_label(label_25).unwrap(),
            "abcdefghijklmnopqrstuvw…"
        );

        let title_512 = "t".repeat(512);
        let title_513 = "t".repeat(513);
        assert_eq!(bounded_thread_title(&title_512).unwrap(), title_512);
        assert_eq!(
            bounded_thread_title(&title_513).unwrap(),
            "t".repeat(511) + "…"
        );

        let limit_name_96 = "l".repeat(96);
        let limit_name_97 = "l".repeat(97);
        assert_eq!(bounded_limit_name(&limit_name_96).unwrap(), limit_name_96);
        assert_eq!(
            bounded_limit_name(&limit_name_97).unwrap(),
            "l".repeat(95) + "…"
        );

        let status_160 = "s".repeat(160);
        let status_161 = "s".repeat(161);
        assert_eq!(bounded_status(&status_160).unwrap(), status_160);
        assert_eq!(bounded_status(&status_161).unwrap(), "s".repeat(159) + "…");
    }
}

// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! Private SQLite generations for short-lived Codex app-server children.
//!
//! This module copies only Codex's state index with SQLite's online-backup
//! API. Authentication, configuration, and session JSONL remain rooted in the
//! caller's unchanged `CODEX_HOME`.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};

use crate::security;

const GENERATION_PREFIX: &str = "generation-";
const ROOT_LOCK_FILE: &str = ".prepare.lock";
const MARKER_FILE: &str = ".codex-info-generation";
const LOCK_FILE: &str = ".owner.lock";
const STATE_DATABASE: &str = "state_5.sqlite";
const MARKER_VERSION: &str = "codex-info-app-server-sqlite-v1";

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);
static PREPARE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationErrorKind {
    UnsafeRoot,
    UnsafeSource,
    UnsafeGeneration,
    Busy,
    Database,
    Io,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationError {
    kind: GenerationErrorKind,
}

impl GenerationError {
    const fn new(kind: GenerationErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> GenerationErrorKind {
        self.kind
    }
}

impl fmt::Display for GenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            GenerationErrorKind::UnsafeRoot => "unsafe app-server cache root",
            GenerationErrorKind::UnsafeSource => "unsafe Codex state database",
            GenerationErrorKind::UnsafeGeneration => "unsafe app-server cache generation",
            GenerationErrorKind::Busy => "app-server cache generation is still in use",
            GenerationErrorKind::Database => "Codex state snapshot failed",
            GenerationErrorKind::Io => "app-server cache operation failed",
        })
    }
}

impl std::error::Error for GenerationError {}

/// A prepared generation whose lock stays live for the whole child lifetime.
///
/// Drop deliberately preserves the directory. Call [`Self::cleanup`] only
/// after the app-server child has been reaped. A dropped generation becomes a
/// stale candidate which a later prepare may recover safely.
pub struct PreparedGeneration {
    cache_root: PathBuf,
    path: PathBuf,
    directory: File,
    _lock: File,
}

impl fmt::Debug for PreparedGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedGeneration")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl PreparedGeneration {
    pub fn prepare(cache_root: &Path, codex_root: &Path) -> Result<Self, GenerationError> {
        let _serial = PREPARE_LOCK
            .lock()
            .map_err(|_| GenerationError::new(GenerationErrorKind::Io))?;
        let cache_root = prepare_private_root(cache_root)?;
        let _root_lock = acquire_root_lock(&cache_root)?;
        recover_stale_generations(&cache_root)?;

        let before = root_entry_names(&cache_root)?;
        let generation_name = next_generation_name();
        let generation_path = cache_root.join(&generation_name);
        create_private_directory(&generation_path)?;
        let (generation_directory, _) = open_generation_directory(&generation_path)?;
        let mut generation_directory = Some(File::from(generation_directory));

        let mut owned_lock = None;
        let created = (|| {
            let lock = create_private_file(&generation_path.join(LOCK_FILE))?;
            rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                .map_err(|_| GenerationError::new(GenerationErrorKind::Io))?;
            owned_lock = Some(lock);
            let marker = format!("{MARKER_VERSION}\n{generation_name}\n");
            let mut marker_file = create_private_file(&generation_path.join(MARKER_FILE))?;
            use std::io::Write;
            marker_file
                .write_all(marker.as_bytes())
                .and_then(|()| marker_file.sync_all())
                .map_err(|_| GenerationError::new(GenerationErrorKind::Io))?;
            snapshot_state_database(codex_root, &generation_path)?;
            validate_generation_at(
                &generation_path,
                generation_directory
                    .as_ref()
                    .expect("generation directory is retained until prepare succeeds"),
            )?;

            let mut expected = before;
            expected.insert(generation_name);
            if root_entry_names(&cache_root)? != expected {
                return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
            }
            Ok(Self {
                cache_root: cache_root.clone(),
                path: generation_path.clone(),
                directory: generation_directory
                    .take()
                    .expect("successful prepare retains the generation directory"),
                _lock: owned_lock
                    .take()
                    .expect("successful prepare retains the acquired generation lock"),
            })
        })();

        if created.is_err() && owned_lock.is_some() {
            if let Some(directory) = generation_directory.as_ref() {
                let _ = remove_generation_files(&generation_path, directory, false);
            }
        }
        created
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn sqlite_home_override(&self) -> OsString {
        let mut value = OsString::from("sqlite_home=");
        value.push(&self.path);
        value
    }

    /// Remove this generation after the associated child is known to be
    /// reaped. The live owner lock remains held throughout this operation.
    pub fn cleanup(self) -> Result<(), GenerationError> {
        let _serial = PREPARE_LOCK
            .lock()
            .map_err(|_| GenerationError::new(GenerationErrorKind::Io))?;
        let _root_lock = acquire_root_lock(&self.cache_root)?;
        remove_owned_generation(&self.path, &self.directory)
    }

    #[cfg(test)]
    fn lock_file(&self) -> &File {
        &self._lock
    }
}

fn next_generation_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    format!(
        "{GENERATION_PREFIX}{}-{nanos}-{sequence}",
        std::process::id()
    )
}

#[cfg(unix)]
fn validate_private_directory(path: &Path) -> Result<(), GenerationError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(path)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeRoot))?;
    if !path.is_absolute()
        || metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(GenerationError::new(GenerationErrorKind::UnsafeRoot));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_directory(_path: &Path) -> Result<(), GenerationError> {
    Err(GenerationError::new(GenerationErrorKind::UnsafeRoot))
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<(), GenerationError> {
    use std::os::unix::fs::DirBuilderExt;

    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|_| GenerationError::new(GenerationErrorKind::Io))?;
    validate_private_directory(path)
}

#[cfg(not(unix))]
fn create_private_directory(_path: &Path) -> Result<(), GenerationError> {
    Err(GenerationError::new(GenerationErrorKind::UnsafeRoot))
}

fn prepare_private_root(path: &Path) -> Result<PathBuf, GenerationError> {
    if path.exists() {
        validate_private_directory(path)?;
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| GenerationError::new(GenerationErrorKind::UnsafeRoot))?;
        security::validate_absolute_root(parent)
            .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeRoot))?;
        create_private_directory(path)?;
    }
    let canonical = fs::canonicalize(path)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeRoot))?;
    if canonical != path {
        return Err(GenerationError::new(GenerationErrorKind::UnsafeRoot));
    }
    Ok(canonical)
}

#[cfg(unix)]
fn acquire_root_lock(root: &Path) -> Result<File, GenerationError> {
    let root_fd = rustix::fs::open(
        root,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeRoot))?;
    let flags =
        rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW;
    let lock_fd =
        match rustix::fs::openat(&root_fd, ROOT_LOCK_FILE, flags, rustix::fs::Mode::empty()) {
            Ok(fd) => fd,
            Err(error) if error == rustix::io::Errno::NOENT => {
                if fs::read_dir(root)
                    .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeRoot))?
                    .next()
                    .is_some()
                {
                    return Err(GenerationError::new(GenerationErrorKind::UnsafeRoot));
                }
                rustix::fs::openat(
                    &root_fd,
                    ROOT_LOCK_FILE,
                    flags | rustix::fs::OFlags::CREATE | rustix::fs::OFlags::EXCL,
                    rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
                )
                .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeRoot))?
            }
            Err(_) => return Err(GenerationError::new(GenerationErrorKind::UnsafeRoot)),
        };
    let lock = File::from(lock_fd);
    validate_private_regular_fd(&lock)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeRoot))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
        |error| {
            if error == rustix::io::Errno::WOULDBLOCK {
                GenerationError::new(GenerationErrorKind::Busy)
            } else {
                GenerationError::new(GenerationErrorKind::Io)
            }
        },
    )?;
    Ok(lock)
}

#[cfg(not(unix))]
fn acquire_root_lock(_root: &Path) -> Result<File, GenerationError> {
    Err(GenerationError::new(GenerationErrorKind::UnsafeRoot))
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<File, GenerationError> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| GenerationError::new(GenerationErrorKind::Io))?;
    validate_private_regular_file(path)?;
    Ok(file)
}

#[cfg(not(unix))]
fn create_private_file(_path: &Path) -> Result<File, GenerationError> {
    Err(GenerationError::new(GenerationErrorKind::UnsafeRoot))
}

#[cfg(unix)]
fn validate_private_regular_file(path: &Path) -> Result<(), GenerationError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(path)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_regular_file(_path: &Path) -> Result<(), GenerationError> {
    Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration))
}

fn source_identity(path: &Path) -> Result<(u64, u64), GenerationError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeSource))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(GenerationError::new(GenerationErrorKind::UnsafeSource));
        }
        return Ok((metadata.dev(), metadata.ino()));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(GenerationError::new(GenerationErrorKind::UnsafeSource))
    }
}

fn optional_sidecar_identities(
    database: &Path,
) -> Result<Vec<(PathBuf, (u64, u64))>, GenerationError> {
    let mut sidecars = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let mut name = database.as_os_str().to_os_string();
        name.push(suffix);
        let path = PathBuf::from(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => sidecars.push((path.clone(), source_identity(&path)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(GenerationError::new(GenerationErrorKind::UnsafeSource)),
        }
    }
    Ok(sidecars)
}

fn snapshot_state_database(codex_root: &Path, generation: &Path) -> Result<(), GenerationError> {
    let canonical_root = security::validate_absolute_root(codex_root)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeSource))?;
    let source_path = canonical_root.join(STATE_DATABASE);
    let source_path = security::canonical_regular_file_under(&canonical_root, &source_path)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeSource))?;
    if source_path.parent() != Some(canonical_root.as_path())
        || source_path.file_name() != Some(OsStr::new(STATE_DATABASE))
    {
        return Err(GenerationError::new(GenerationErrorKind::UnsafeSource));
    }
    let source_before = source_identity(&source_path)?;
    let sidecars_before = optional_sidecar_identities(&source_path)?;

    let source = Connection::open_with_flags(
        &source_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| GenerationError::new(GenerationErrorKind::Database))?;
    source
        .pragma_update(None, "query_only", true)
        .and_then(|()| source.pragma_update(None, "trusted_schema", false))
        .map_err(|_| GenerationError::new(GenerationErrorKind::Database))?;

    let destination_path = generation.join(STATE_DATABASE);
    let destination_file = create_private_file(&destination_path)?;
    drop(destination_file);
    let mut destination = Connection::open_with_flags(
        &destination_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| GenerationError::new(GenerationErrorKind::Database))?;
    {
        let backup = Backup::new(&source, &mut destination)
            .map_err(|_| GenerationError::new(GenerationErrorKind::Database))?;
        match backup
            .step(-1)
            .map_err(|_| GenerationError::new(GenerationErrorKind::Database))?
        {
            StepResult::Done => {}
            StepResult::More | StepResult::Busy | StepResult::Locked => {
                return Err(GenerationError::new(GenerationErrorKind::Database));
            }
            _ => return Err(GenerationError::new(GenerationErrorKind::Database)),
        }
    }
    drop(destination);
    drop(source);
    validate_private_regular_file(&destination_path)?;

    if source_identity(&source_path)? != source_before
        || optional_sidecar_identities(&source_path)? != sidecars_before
    {
        return Err(GenerationError::new(GenerationErrorKind::UnsafeSource));
    }
    Ok(())
}

fn is_allowed_sqlite_name(name: &OsStr) -> bool {
    const BASES: [&str; 5] = [
        "state_5.sqlite",
        "logs_2.sqlite",
        "goals_1.sqlite",
        "memories_1.sqlite",
        "queue_1.sqlite",
    ];
    let Some(name) = name.to_str() else {
        return false;
    };
    BASES.iter().any(|base| {
        name == *base
            || name == format!("{base}-wal")
            || name == format!("{base}-shm")
            || name == format!("{base}-journal")
    })
}

fn expected_marker(generation: &Path) -> Result<String, GenerationError> {
    let name = generation
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| name.starts_with(GENERATION_PREFIX))
        .ok_or_else(|| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
    Ok(format!("{MARKER_VERSION}\n{name}\n"))
}

struct GenerationShape {
    entries: Vec<OsString>,
    saw_lock: bool,
}

fn validate_generation_shape_at(
    generation: &Path,
    directory: &File,
    require_state: bool,
) -> Result<GenerationShape, GenerationError> {
    use std::io::Read;
    use std::os::fd::AsRawFd;

    let directory_stat = rustix::fs::fstat(directory)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
    validate_private_directory_stat(&directory_stat)?;
    let marker_expected = expected_marker(generation)?;
    let anchored = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let mut entries = Vec::new();
    let mut saw_marker = false;
    let mut saw_lock = false;
    let mut saw_state = false;
    let mut saw_sqlite = false;
    for entry in fs::read_dir(anchored)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?
    {
        let entry =
            entry.map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
        let name = entry.file_name();
        if name != MARKER_FILE && name != LOCK_FILE && !is_allowed_sqlite_name(&name) {
            return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
        }
        let entry_fd = rustix::fs::openat(
            directory,
            &name,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
        let opened = rustix::fs::fstat(&entry_fd)
            .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
        validate_private_regular_stat(&opened)?;
        let current = rustix::fs::statat(directory, &name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
        if !same_stat_identity(&opened, &current) {
            return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
        }
        if name == MARKER_FILE {
            let mut marker = String::new();
            File::from(entry_fd)
                .take(4_097)
                .read_to_string(&mut marker)
                .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
            if marker != marker_expected {
                return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
            }
            saw_marker = true;
        } else if name == LOCK_FILE {
            saw_lock = true;
        } else {
            saw_sqlite = true;
            saw_state |= name == STATE_DATABASE;
        }
        entries.push(name);
    }
    if require_state {
        if !saw_marker || !saw_lock || !saw_state {
            return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
        }
    } else if !saw_marker && saw_sqlite {
        return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
    }
    Ok(GenerationShape { entries, saw_lock })
}

fn validate_generation_at(generation: &Path, directory: &File) -> Result<(), GenerationError> {
    validate_generation_shape_at(generation, directory, true).map(|_| ())
}

fn root_entry_names(root: &Path) -> Result<BTreeSet<String>, GenerationError> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(root).map_err(|_| GenerationError::new(GenerationErrorKind::Io))? {
        let entry = entry.map_err(|_| GenerationError::new(GenerationErrorKind::Io))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
        if name == ROOT_LOCK_FILE {
            validate_private_regular_file(&entry.path())?;
            continue;
        }
        if !name.starts_with(GENERATION_PREFIX) {
            return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
        }
        names.insert(name);
    }
    Ok(names)
}

fn recover_stale_generations(root: &Path) -> Result<(), GenerationError> {
    for name in root_entry_names(root)? {
        let generation = root.join(name);
        let (generation_fd, _) = open_generation_directory(&generation)?;
        let directory = File::from(generation_fd);
        let shape = validate_generation_shape_at(&generation, &directory, false)?;
        let _lock = if shape.saw_lock {
            let lock_fd = rustix::fs::openat(
                &directory,
                LOCK_FILE,
                rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::empty(),
            )
            .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
            let lock = File::from(lock_fd);
            validate_private_regular_fd(&lock)?;
            match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => Some(lock),
                Err(error) if error == rustix::io::Errno::WOULDBLOCK => continue,
                Err(_) => return Err(GenerationError::new(GenerationErrorKind::Io)),
            }
        } else {
            None
        };
        remove_generation_files(&generation, &directory, false)?;
    }
    Ok(())
}

fn remove_owned_generation(generation: &Path, directory: &File) -> Result<(), GenerationError> {
    remove_generation_files(generation, directory, true)
}

fn remove_generation_files(
    generation: &Path,
    directory: &File,
    require_state: bool,
) -> Result<(), GenerationError> {
    #[cfg(unix)]
    {
        let parent = generation
            .parent()
            .ok_or_else(|| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
        let name = generation
            .file_name()
            .ok_or_else(|| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
        let parent_fd = rustix::fs::open(
            parent,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
        let generation_stat = rustix::fs::fstat(directory)
            .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
        validate_private_directory_stat(&generation_stat)?;
        let shape = validate_generation_shape_at(generation, directory, require_state)?;
        ensure_generation_mapping(&parent_fd, name, &generation_stat)?;
        for entry_name in shape.entries {
            ensure_generation_mapping(&parent_fd, name, &generation_stat)?;
            let entry_fd = rustix::fs::openat(
                directory,
                &entry_name,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::empty(),
            )
            .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
            let opened = rustix::fs::fstat(&entry_fd)
                .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
            validate_private_regular_stat(&opened)?;
            let current = rustix::fs::statat(
                directory,
                &entry_name,
                rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
            if !same_stat_identity(&opened, &current) {
                return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
            }
            rustix::fs::unlinkat(directory, &entry_name, rustix::fs::AtFlags::empty())
                .map_err(|_| GenerationError::new(GenerationErrorKind::Io))?;
        }
        ensure_generation_mapping(&parent_fd, name, &generation_stat)?;
        rustix::fs::unlinkat(&parent_fd, name, rustix::fs::AtFlags::REMOVEDIR)
            .map_err(|_| GenerationError::new(GenerationErrorKind::Io))
    }
    #[cfg(not(unix))]
    {
        let _ = generation;
        Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration))
    }
}

#[cfg(unix)]
fn ensure_generation_mapping(
    parent: &impl rustix::fd::AsFd,
    name: &OsStr,
    expected: &rustix::fs::Stat,
) -> Result<(), GenerationError> {
    let current = rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
    if !same_stat_identity(expected, &current) {
        return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
    }
    Ok(())
}

#[cfg(unix)]
fn open_generation_directory(
    generation: &Path,
) -> Result<(rustix::fd::OwnedFd, rustix::fs::Stat), GenerationError> {
    let fd = rustix::fs::open(
        generation,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
    let stat = rustix::fs::fstat(&fd)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
    validate_private_directory_stat(&stat)?;
    Ok((fd, stat))
}

#[cfg(unix)]
fn validate_private_directory_stat(stat: &rustix::fs::Stat) -> Result<(), GenerationError> {
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o777 != 0o700
    {
        return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_private_regular_stat(stat: &rustix::fs::Stat) -> Result<(), GenerationError> {
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_nlink != 1
        || stat.st_mode & 0o777 != 0o600
    {
        return Err(GenerationError::new(GenerationErrorKind::UnsafeGeneration));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_private_regular_fd(file: &File) -> Result<(), GenerationError> {
    let stat = rustix::fs::fstat(file)
        .map_err(|_| GenerationError::new(GenerationErrorKind::UnsafeGeneration))?;
    validate_private_regular_stat(&stat)
}

#[cfg(unix)]
fn same_stat_identity(left: &rustix::fs::Stat, right: &rustix::fs::Stat) -> bool {
    left.st_dev == right.st_dev
        && left.st_ino == right.st_ino
        && left.st_uid == right.st_uid
        && left.st_mode == right.st_mode
        && left.st_nlink == right.st_nlink
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::process::{Command, Stdio};

    struct Fixture {
        root: PathBuf,
        codex: PathBuf,
        cache: PathBuf,
        _source: Connection,
    }

    impl Fixture {
        fn new() -> Self {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

            let root = std::env::current_dir()
                .unwrap()
                .join("target")
                .join("app-server-sqlite-fixtures")
                .join(next_generation_name());
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&root)
                .unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let codex = root.join("codex");
            fs::DirBuilder::new().mode(0o700).create(&codex).unwrap();
            let database = Connection::open(codex.join(STATE_DATABASE)).unwrap();
            database
                .execute_batch(
                    "PRAGMA journal_mode=WAL; CREATE TABLE threads(id TEXT PRIMARY KEY, value TEXT);\
                     INSERT INTO threads VALUES ('thread-1', 'before');",
                )
                .unwrap();
            for suffix in ["", "-wal", "-shm"] {
                let path = codex.join(format!("{STATE_DATABASE}{suffix}"));
                if path.exists() {
                    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
                }
            }
            let cache = root.join("cache");
            Self {
                root,
                codex,
                cache,
                _source: database,
            }
        }

        fn durable_source_hashes(&self) -> Vec<(PathBuf, u64, [u8; 32])> {
            ["", "-wal"]
                .into_iter()
                .filter_map(|suffix| {
                    let path = self.codex.join(format!("{STATE_DATABASE}{suffix}"));
                    path.exists().then(|| {
                        let bytes = fs::read(&path).unwrap();
                        let digest: [u8; 32] = Sha256::digest(&bytes).into();
                        (path, bytes.len() as u64, digest)
                    })
                })
                .collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn online_backup_is_private_and_source_is_unchanged() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new();
        let source_before = fixture.durable_source_hashes();
        let generation = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
        assert_eq!(
            generation.sqlite_home_override(),
            OsString::from(format!("sqlite_home={}", generation.path().display()))
        );
        let destination = Connection::open(generation.path().join(STATE_DATABASE)).unwrap();
        assert_eq!(
            destination
                .query_row("SELECT value FROM threads WHERE id='thread-1'", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "before"
        );
        drop(destination);
        assert_eq!(fixture.durable_source_hashes(), source_before);
        assert_eq!(
            fs::metadata(generation.path().join(STATE_DATABASE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        generation.cleanup().unwrap();
        assert!(root_entry_names(&fixture.cache).unwrap().is_empty());
    }

    #[test]
    fn live_generation_is_kept_and_dropped_generation_is_recovered() {
        let fixture = Fixture::new();
        let first = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
        rustix::fs::flock(
            first.lock_file(),
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .unwrap();
        let first_path = first.path().to_owned();
        let second = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
        assert!(first_path.exists());
        assert_eq!(root_entry_names(&fixture.cache).unwrap().len(), 2);
        second.cleanup().unwrap();
        drop(first);
        let third = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
        assert!(!first_path.exists());
        assert_eq!(root_entry_names(&fixture.cache).unwrap().len(), 1);
        third.cleanup().unwrap();
    }

    #[test]
    fn foreign_root_entry_blocks_prepare_without_removal() {
        let fixture = Fixture::new();
        create_private_directory(&fixture.cache).unwrap();
        drop(acquire_root_lock(&fixture.cache).unwrap());
        let foreign = fixture.cache.join("not-managed");
        fs::write(&foreign, b"keep").unwrap();
        let error = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap_err();
        assert_eq!(error.kind(), GenerationErrorKind::UnsafeGeneration);
        assert_eq!(fs::read(foreign).unwrap(), b"keep");
    }

    #[test]
    fn stale_incomplete_generation_is_recovered_without_growth() {
        let fixture = Fixture::new();
        create_private_directory(&fixture.cache).unwrap();
        drop(acquire_root_lock(&fixture.cache).unwrap());
        let generation = fixture.cache.join(next_generation_name());
        create_private_directory(&generation).unwrap();
        create_private_file(&generation.join(LOCK_FILE)).unwrap();
        let marker = format!(
            "{MARKER_VERSION}\n{}\n",
            generation.file_name().unwrap().to_string_lossy()
        );
        fs::write(generation.join(MARKER_FILE), marker).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            generation.join(MARKER_FILE),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let recovered = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
        assert!(!generation.exists());
        assert_eq!(root_entry_names(&fixture.cache).unwrap().len(), 1);
        recovered.cleanup().unwrap();
    }

    #[test]
    fn crash_before_marker_is_recovered_without_permanent_block() {
        for with_lock in [false, true] {
            let fixture = Fixture::new();
            create_private_directory(&fixture.cache).unwrap();
            drop(acquire_root_lock(&fixture.cache).unwrap());
            let generation = fixture.cache.join(next_generation_name());
            create_private_directory(&generation).unwrap();
            if with_lock {
                create_private_file(&generation.join(LOCK_FILE)).unwrap();
            }
            let recovered = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
            assert!(!generation.exists());
            assert_eq!(root_entry_names(&fixture.cache).unwrap().len(), 1);
            recovered.cleanup().unwrap();
        }
    }

    #[test]
    fn cleanup_rejects_replaced_generation_without_touching_replacement() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new();
        let generation = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
        let path = generation.path().to_owned();
        let moved = fixture.cache.join(next_generation_name());
        fs::rename(&path, &moved).unwrap();
        create_private_directory(&path).unwrap();
        let replacement = path.join("keep");
        fs::write(&replacement, b"replacement").unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();

        let error = generation.cleanup().unwrap_err();
        assert_eq!(error.kind(), GenerationErrorKind::UnsafeGeneration);
        assert_eq!(fs::read(replacement).unwrap(), b"replacement");
        assert!(moved.join(STATE_DATABASE).exists());
    }

    #[test]
    fn inherited_owner_lock_preserves_generation_until_child_exit() {
        let fixture = Fixture::new();
        let generation = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
        let path = generation.path().to_owned();
        let input = generation.lock_file().try_clone().unwrap();
        let mut child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::from(input))
            .spawn()
            .unwrap();
        drop(generation);
        let current = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
        assert!(path.exists());
        assert_eq!(root_entry_names(&fixture.cache).unwrap().len(), 2);
        current.cleanup().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let recovered = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap();
        assert!(!path.exists());
        recovered.cleanup().unwrap();
    }

    #[test]
    fn source_symlink_is_rejected_without_cache_growth() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let source = fixture.codex.join(STATE_DATABASE);
        let real = fixture.codex.join("real.sqlite");
        fs::rename(&source, &real).unwrap();
        symlink(&real, &source).unwrap();
        let error = PreparedGeneration::prepare(&fixture.cache, &fixture.codex).unwrap_err();
        assert_eq!(error.kind(), GenerationErrorKind::UnsafeSource);
        assert!(root_entry_names(&fixture.cache).unwrap().is_empty());
    }
}

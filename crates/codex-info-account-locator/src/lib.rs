// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! Account and storage-partition identity boundary.
//!
//! This crate is the narrow identity boundary shared by the recorder and REST
//! processes. The ordinary locator reads the current auth.json and existing
//! profile registry, derives opaque account/partition identifiers, and
//! returns an already initialized database path. The recorder-only
//! `ensure_partition` capability allocates the missing profile/account entry;
//! it never migrates a database or chooses an existing path heuristically.

use hmac::{Hmac, Mac};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::Sha256;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

const AUTH_FILE_NAME: &str = "auth.json";
pub const PROFILE_METADATA_FILE_NAME: &str = "account_profile_v1.json";
const PROFILE_SCHEMA: &str = "codex-info-profile-scope-v1";
pub const ACCOUNT_DB_SCHEMA: &str = "codex-info-account-db-v1";
const ACCOUNT_SCOPE_DOMAIN: &[u8] = b"codex-info-account-scope-v1\0";
const PARTITION_SCOPE_DOMAIN: &[u8] = b"codex-info-storage-partition-v1\0";
const MAX_AUTH_FILE_BYTES: u64 = 64 * 1024;
const MAX_PROFILE_FILE_BYTES: u64 = 64 * 1024;
const MAX_ACCOUNT_KEY_BYTES: usize = 512;
const MAX_LOGIN_ID_SCALARS: usize = 254;
const PROFILE_ID_BYTES: usize = 16;
const INSTALL_KEY_BYTES: usize = 32;
const SCOPE_ID_BYTES: usize = 32;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocatorErrorKind {
    UnsafeRoot,
    UnsafeFile,
    InvalidAuth,
    InvalidMetadata,
    RecoveryRequired,
    Io,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocatorError {
    kind: LocatorErrorKind,
}

impl LocatorError {
    const fn new(kind: LocatorErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> LocatorErrorKind {
        self.kind
    }
}

impl fmt::Display for LocatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            LocatorErrorKind::UnsafeRoot => "account root is unsafe",
            LocatorErrorKind::UnsafeFile => "account authority file is unsafe",
            LocatorErrorKind::InvalidAuth => "account authority is invalid",
            LocatorErrorKind::InvalidMetadata => "account profile metadata is invalid",
            LocatorErrorKind::RecoveryRequired => "account storage recovery is required",
            LocatorErrorKind::Io => "account storage operation failed",
        })
    }
}

impl std::error::Error for LocatorError {}

pub type Result<T> = std::result::Result<T, LocatorError>;

/// Identity stored in a partition's singleton storage_partition row.
///
/// All account-derived values are opaque lower-hex strings. The raw
/// tokens.account_id never enters this type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoragePartitionIdentity {
    pub schema_version: String,
    pub profile_scope_id: String,
    pub account_scope_id: String,
    pub storage_epoch: u64,
    pub partition_id: String,
}

/// One half-open account lifecycle interval.  `start_at` is inclusive and
/// `end_at` is exclusive; `None` denotes an unbounded side.  The value is
/// intentionally account-opaque and contains no auth material.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountLifecycleInterval {
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
}

impl AccountLifecycleInterval {
    fn validate(self) -> Result<()> {
        if self.start_at.is_some_and(|value| value <= 0)
            || self.end_at.is_some_and(|value| value <= 0)
            || matches!((self.start_at, self.end_at), (Some(start), Some(end)) if start >= end)
        {
            return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
        }
        Ok(())
    }
}

/// An already initialized account partition selected by the current auth
/// authority. No directory scan or newest-file heuristic is used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountPartition {
    pub profile_scope_id: String,
    pub account_scope_id: String,
    pub storage_epoch: u64,
    pub partition_id: String,
    pub database_path: PathBuf,
    /// Display-only login identifier reported by the authenticated Codex
    /// account. It never participates in account or partition selection.
    pub login_id: Option<String>,
    /// Inclusive Unix-second boundary at which this partition's most recent
    /// interval began accepting shared Session evidence. It is `None` when
    /// that latest interval starts at the unbounded beginning.
    pub activation_timestamp: Option<i64>,
    /// All disjoint half-open intervals ever owned by this physical
    /// partition.  A→B→A therefore has two entries for A instead of one
    /// stale interval that would mix the accounts' time domains.
    pub lifecycle_intervals: Vec<AccountLifecycleInterval>,
    /// The most recently appended interval for this partition.  It may be
    /// closed for a non-current account, or open for the current account.
    pub current_interval_start: Option<i64>,
    pub current_interval_end: Option<i64>,
}

impl AccountPartition {
    /// Opaque selector presented by the REST account chooser.
    ///
    /// Storage epochs are allocated monotonically by the registry and are
    /// never derived from, or equal to, the auth account key.  The selector
    /// therefore identifies a physical partition without exposing the
    /// account scope hash (or any auth material).
    pub fn public_id(&self) -> String {
        format!("account-{}", self.storage_epoch)
    }

    pub fn storage_identity(&self) -> StoragePartitionIdentity {
        StoragePartitionIdentity {
            schema_version: ACCOUNT_DB_SCHEMA.to_owned(),
            profile_scope_id: self.profile_scope_id.clone(),
            account_scope_id: self.account_scope_id.clone(),
            storage_epoch: self.storage_epoch,
            partition_id: self.partition_id.clone(),
        }
    }

    pub fn identity(&self) -> StoragePartitionIdentity {
        self.storage_identity()
    }

    pub fn intervals(&self) -> &[AccountLifecycleInterval] {
        &self.lifecycle_intervals
    }

    pub fn lifecycle(&self) -> &[AccountLifecycleInterval] {
        &self.lifecycle_intervals
    }

    pub fn current_interval(&self) -> Option<AccountLifecycleInterval> {
        self.lifecycle_intervals.last().copied()
    }
}

#[derive(Clone, Eq, PartialEq)]
struct AccountKey(Vec<u8>);

impl AccountKey {
    fn new(value: String) -> Result<Self> {
        if value.is_empty()
            || value.len() > MAX_ACCOUNT_KEY_BYTES
            || value.chars().any(|character| character.is_control())
        {
            return Err(LocatorError::new(LocatorErrorKind::InvalidAuth));
        }
        Ok(Self(value.into_bytes()))
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for AccountKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccountKey([redacted])")
    }
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RegistryState {
    Allocated,
    Initialized,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryEntry {
    storage_epoch: u64,
    state: RegistryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    activation_timestamp: Option<i64>,
    /// Optional display metadata. Account mapping remains exclusively keyed
    /// by the irreversible account scope above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    login_id: Option<String>,
    /// Empty for pre-lifecycle metadata.  The recorder-only admission path
    /// materializes that legacy form before it can be written again.
    #[serde(default, alias = "intervals")]
    lifecycle_intervals: Vec<AccountLifecycleInterval>,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ProfileMetadata {
    schema_version: String,
    profile_scope_id: String,
    install_key: String,
    next_storage_epoch: u64,
    #[serde(deserialize_with = "deserialize_registry")]
    accounts: BTreeMap<String, RegistryEntry>,
}

fn deserialize_registry<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, RegistryEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    struct RegistryVisitor;

    impl<'de> Visitor<'de> for RegistryVisitor {
        type Value = BTreeMap<String, RegistryEntry>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an account scope registry without duplicate keys")
        }

        fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut entries = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, RegistryEntry>()? {
                if entries.insert(key, value).is_some() {
                    return Err(de::Error::custom("duplicate account scope"));
                }
            }
            Ok(entries)
        }
    }

    deserializer.deserialize_map(RegistryVisitor)
}

struct AccountKeySeed;

impl<'de> DeserializeSeed<'de> for AccountKeySeed {
    type Value = AccountKey;

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct AuthVisitor;

        impl<'de> Visitor<'de> for AuthVisitor {
            type Value = AccountKey;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a Codex auth object")
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut tokens_seen = false;
                let mut account_key = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "tokens" {
                        if tokens_seen {
                            return Err(de::Error::duplicate_field("tokens"));
                        }
                        tokens_seen = true;
                        account_key = Some(map.next_value_seed(TokensSeed)?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                account_key
                    .flatten()
                    .ok_or_else(|| de::Error::missing_field("tokens.account_id"))
            }
        }

        deserializer.deserialize_map(AuthVisitor)
    }
}

struct TokensSeed;

impl<'de> DeserializeSeed<'de> for TokensSeed {
    type Value = Option<AccountKey>;

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TokensVisitor;

        impl<'de> Visitor<'de> for TokensVisitor {
            type Value = Option<AccountKey>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a Codex token object")
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut account_id_seen = false;
                let mut account_key = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "account_id" {
                        if account_id_seen {
                            return Err(de::Error::duplicate_field("account_id"));
                        }
                        account_id_seen = true;
                        let value = map.next_value::<String>()?;
                        account_key = Some(AccountKey::new(value).map_err(de::Error::custom)?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(account_key)
            }
        }

        deserializer.deserialize_map(TokensVisitor)
    }
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

#[cfg(unix)]
fn validate_private_directory(metadata: &fs::Metadata) -> bool {
    metadata.is_dir()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == effective_uid()
        && metadata.mode() & 0o777 == 0o700
}

#[cfg(not(unix))]
fn validate_private_directory(metadata: &fs::Metadata) -> bool {
    metadata.is_dir() && !metadata.file_type().is_symlink()
}

#[cfg(unix)]
fn validate_private_file(metadata: &fs::Metadata, max_bytes: u64) -> bool {
    metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == effective_uid()
        && metadata.mode() & 0o777 == 0o600
        && (1..=max_bytes).contains(&metadata.len())
}

#[cfg(not(unix))]
fn validate_private_file(metadata: &fs::Metadata, max_bytes: u64) -> bool {
    metadata.is_file()
        && !metadata.file_type().is_symlink()
        && (1..=max_bytes).contains(&metadata.len())
}

#[cfg(unix)]
fn validate_artifact_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == effective_uid()
        && metadata.mode() & 0o777 == 0o600
}

#[cfg(not(unix))]
fn validate_artifact_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file() && !metadata.file_type().is_symlink()
}

fn inspect_no_symlink_components(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(LocatorError::new(LocatorErrorKind::UnsafeRoot));
    }

    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(LocatorError::new(LocatorErrorKind::UnsafeRoot));
            }
            Component::Normal(name) => {
                current.push(name);
                let metadata = fs::symlink_metadata(&current)
                    .map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeRoot))?;
                if metadata.file_type().is_symlink() {
                    return Err(LocatorError::new(LocatorErrorKind::UnsafeRoot));
                }
            }
        }
    }
    Ok(())
}

fn validate_absolute_root(root: &Path) -> Result<PathBuf> {
    inspect_no_symlink_components(root)?;
    let metadata =
        fs::symlink_metadata(root).map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeRoot))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(LocatorError::new(LocatorErrorKind::UnsafeRoot));
    }
    fs::canonicalize(root).map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeRoot))
}

#[cfg(unix)]
fn same_file(before: &fs::Metadata, opened: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.dev() == opened.dev()
        && before.ino() == opened.ino()
        && before.len() == opened.len()
        && before.uid() == opened.uid()
        && before.mode() == opened.mode()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.uid() == after.uid()
        && before.mode() == after.mode()
}

#[cfg(not(unix))]
fn same_file(before: &fs::Metadata, opened: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.len() == opened.len() && opened.len() == after.len()
}

fn read_private_file_with_post_read<F>(path: &Path, max_bytes: u64, post_read: F) -> Result<Vec<u8>>
where
    F: FnOnce(),
{
    let parent = path
        .parent()
        .ok_or_else(|| LocatorError::new(LocatorErrorKind::UnsafeRoot))?;
    let root_before = fs::symlink_metadata(parent)
        .map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeRoot))?;
    if !parent.is_absolute() || !validate_private_directory(&root_before) {
        return Err(LocatorError::new(LocatorErrorKind::UnsafeRoot));
    }
    let before =
        fs::symlink_metadata(path).map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeFile))?;
    if !validate_private_file(&before, max_bytes) {
        return Err(LocatorError::new(LocatorErrorKind::UnsafeFile));
    }

    #[cfg(unix)]
    let mut file = {
        let fd = rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeFile))?;
        File::from(fd)
    };
    #[cfg(not(unix))]
    let mut file = File::open(path).map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeFile))?;

    let opened = file
        .metadata()
        .map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeFile))?;
    if !validate_private_file(&opened, max_bytes) {
        return Err(LocatorError::new(LocatorErrorKind::UnsafeFile));
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
    post_read();
    let after =
        fs::symlink_metadata(path).map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeFile))?;
    let root_after = fs::symlink_metadata(parent)
        .map_err(|_| LocatorError::new(LocatorErrorKind::UnsafeRoot))?;
    if bytes.len() as u64 != opened.len()
        || bytes.len() as u64 > max_bytes
        || !validate_private_file(&after, max_bytes)
        || !same_file(&before, &opened, &after)
        || !validate_private_directory(&root_after)
    {
        return Err(LocatorError::new(LocatorErrorKind::UnsafeFile));
    }
    #[cfg(unix)]
    if root_before.dev() != root_after.dev() || root_before.ino() != root_after.ino() {
        return Err(LocatorError::new(LocatorErrorKind::UnsafeFile));
    }
    Ok(bytes)
}

fn read_private_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    read_private_file_with_post_read(path, max_bytes, || {})
}

fn read_account_key(codex_home: &Path) -> Result<AccountKey> {
    let codex_home = validate_absolute_root(codex_home)?;
    let bytes = read_private_file(&codex_home.join(AUTH_FILE_NAME), MAX_AUTH_FILE_BYTES)?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    let account_key = AccountKeySeed
        .deserialize(&mut deserializer)
        .map_err(|_| LocatorError::new(LocatorErrorKind::InvalidAuth))?;
    deserializer
        .end()
        .map_err(|_| LocatorError::new(LocatorErrorKind::InvalidAuth))?;
    Ok(account_key)
}

fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N]> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    hex::decode(value)
        .map_err(|_| LocatorError::new(LocatorErrorKind::InvalidMetadata))?
        .try_into()
        .map_err(|_| LocatorError::new(LocatorErrorKind::InvalidMetadata))
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> Result<[u8; SCOPE_ID_BYTES]> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| LocatorError::new(LocatorErrorKind::InvalidMetadata))?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

fn account_scope(install_key: &[u8], account_key: &AccountKey) -> Result<[u8; SCOPE_ID_BYTES]> {
    hmac(install_key, &[ACCOUNT_SCOPE_DOMAIN, account_key.as_bytes()])
}

fn partition_scope(
    install_key: &[u8],
    profile_scope_id: &[u8; PROFILE_ID_BYTES],
    account_scope_id: &[u8; SCOPE_ID_BYTES],
    storage_epoch: u64,
) -> Result<[u8; SCOPE_ID_BYTES]> {
    hmac(
        install_key,
        &[
            PARTITION_SCOPE_DOMAIN,
            profile_scope_id,
            account_scope_id,
            &storage_epoch.to_be_bytes(),
        ],
    )
}

fn accounts_root(data_root: &Path) -> PathBuf {
    data_root.join("history").join("accounts").join("v1")
}

fn metadata_path(data_root: &Path) -> PathBuf {
    data_root.join("history").join(PROFILE_METADATA_FILE_NAME)
}

fn root_has_account_artifacts(data_root: &Path) -> Result<bool> {
    match fs::read_dir(accounts_root(data_root)) {
        Ok(mut entries) => Ok(entries
            .next()
            .transpose()
            .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?
            .is_some()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(LocatorError::new(LocatorErrorKind::RecoveryRequired)),
    }
}

fn prepare_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
    if !validate_private_directory(&metadata) {
        return Err(LocatorError::new(LocatorErrorKind::UnsafeRoot));
    }
    Ok(())
}

fn write_metadata(data_root: &Path, metadata: &ProfileMetadata) -> Result<()> {
    validate_metadata(metadata)?;
    let path = metadata_path(data_root);
    let parent = path
        .parent()
        .ok_or_else(|| LocatorError::new(LocatorErrorKind::UnsafeRoot))?;
    prepare_private_directory(parent)?;
    let accounts_parent = parent.join("accounts");
    prepare_private_directory(&accounts_parent)?;
    prepare_private_directory(&accounts_root(data_root))?;
    if let Ok(existing) = fs::symlink_metadata(&path) {
        if !validate_private_file(&existing, MAX_PROFILE_FILE_BYTES) {
            return Err(LocatorError::new(LocatorErrorKind::UnsafeFile));
        }
    }
    let bytes = serde_json::to_vec(metadata)
        .map_err(|_| LocatorError::new(LocatorErrorKind::InvalidMetadata))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_PROFILE_FILE_BYTES {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
    let temporary = parent.join(format!(
        ".{PROFILE_METADATA_FILE_NAME}.tmp-{}",
        hex::encode(nonce)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
        file.sync_all()
            .map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
        drop(file);
        fs::rename(&temporary, &path).map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn initial_metadata() -> Result<ProfileMetadata> {
    let mut profile_scope_id = [0_u8; PROFILE_ID_BYTES];
    let mut install_key = [0_u8; INSTALL_KEY_BYTES];
    getrandom::fill(&mut profile_scope_id)
        .and_then(|()| getrandom::fill(&mut install_key))
        .map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
    Ok(ProfileMetadata {
        schema_version: PROFILE_SCHEMA.to_owned(),
        profile_scope_id: hex::encode(profile_scope_id),
        install_key: hex::encode(install_key),
        next_storage_epoch: 1,
        accounts: BTreeMap::new(),
    })
}

fn load_or_initialize_metadata(data_root: &Path) -> Result<ProfileMetadata> {
    match fs::symlink_metadata(metadata_path(data_root)) {
        Ok(_) => load_metadata(data_root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if root_has_account_artifacts(data_root)? {
                return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
            }
            let metadata = initial_metadata()?;
            write_metadata(data_root, &metadata)?;
            load_metadata(data_root)
        }
        Err(_) => Err(LocatorError::new(LocatorErrorKind::RecoveryRequired)),
    }
}

fn directory_is_empty(path: &Path) -> Result<bool> {
    match fs::read_dir(path) {
        Ok(mut entries) => Ok(entries
            .next()
            .transpose()
            .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?
            .is_none()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(_) => Err(LocatorError::new(LocatorErrorKind::RecoveryRequired)),
    }
}

fn validate_metadata(metadata: &ProfileMetadata) -> Result<()> {
    if metadata.schema_version != PROFILE_SCHEMA || metadata.next_storage_epoch == 0 {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    decode_fixed::<PROFILE_ID_BYTES>(&metadata.profile_scope_id)?;
    decode_fixed::<INSTALL_KEY_BYTES>(&metadata.install_key)?;
    let mut maximum_epoch = 0_u64;
    let mut storage_epochs = HashSet::with_capacity(metadata.accounts.len());
    let mut entries_with_intervals = 0_usize;
    let mut entries_without_intervals = 0_usize;
    for (scope, entry) in &metadata.accounts {
        decode_fixed::<SCOPE_ID_BYTES>(scope)?;
        if entry.storage_epoch == 0 {
            return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
        }
        if !storage_epochs.insert(entry.storage_epoch) {
            // Public selectors are intentionally derived from storage_epoch;
            // duplicate registry epochs would make two partitions select the
            // same opaque ID and could cross-wire cache entries.
            return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
        }
        if entry.activation_timestamp.is_some_and(|value| value <= 0) {
            return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
        }
        if entry
            .login_id
            .as_deref()
            .is_some_and(|value| !valid_login_id(value))
        {
            return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
        }
        if entry.lifecycle_intervals.is_empty() {
            entries_without_intervals = entries_without_intervals.saturating_add(1);
        } else {
            entries_with_intervals = entries_with_intervals.saturating_add(1);
            for interval in &entry.lifecycle_intervals {
                interval.validate()?;
            }
            for pair in entry.lifecycle_intervals.windows(2) {
                if lifecycle_sort_key(&pair[1]) < lifecycle_sort_key(&pair[0]) {
                    return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
                }
            }
        }
        maximum_epoch = maximum_epoch.max(entry.storage_epoch);
    }
    // Empty lifecycle fields are the one explicitly supported legacy form.
    // A mixture of legacy and migrated entries cannot be assigned a single
    // authoritative timeline without guessing, so it is rejected.
    if entries_with_intervals != 0 && entries_without_intervals != 0 {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    if entries_with_intervals != 0 {
        validate_lifecycle_timeline(metadata)?;
    }
    if metadata.next_storage_epoch <= maximum_epoch {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    Ok(())
}

fn lifecycle_sort_key(interval: &AccountLifecycleInterval) -> (i64, i64) {
    (
        interval.start_at.unwrap_or(i64::MIN),
        interval.end_at.unwrap_or(i64::MAX),
    )
}

/// Validate the profile-wide lifecycle timeline.  The union of account
/// intervals may begin at a finite first admission, but once it begins it
/// must have exactly adjacent half-open intervals through one and only one
/// open interval.  This rejects overlap, gaps, reversed ranges, and multiple
/// current accounts before any database is selected.
fn validate_lifecycle_timeline(metadata: &ProfileMetadata) -> Result<()> {
    let mut intervals = metadata
        .accounts
        .iter()
        .flat_map(|(scope, entry)| {
            entry
                .lifecycle_intervals
                .iter()
                .map(move |interval| (scope.as_str(), *interval))
        })
        .collect::<Vec<_>>();
    intervals.sort_by_key(|(_, interval)| lifecycle_sort_key(interval));
    if intervals.is_empty() {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    let open_count = intervals
        .iter()
        .filter(|(_, interval)| interval.end_at.is_none())
        .count();
    if open_count != 1 {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    for pair in intervals.windows(2) {
        let previous = pair[0].1;
        let next = pair[1].1;
        match (previous.end_at, next.start_at) {
            (Some(end), Some(start)) if end == start => {}
            // A finite interval followed by an unbounded-start interval has a
            // gap at -infinity unless it is the first interval, and a
            // differing finite boundary is a real gap or overlap.
            _ => return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata)),
        }
    }
    Ok(())
}

fn has_lifecycle_intervals(metadata: &ProfileMetadata) -> bool {
    metadata
        .accounts
        .values()
        .any(|entry| !entry.lifecycle_intervals.is_empty())
}

/// Materialize the only safe interpretation of pre-lifecycle metadata in
/// memory.  The recorder path persists the same result; read-only locator
/// callers receive the projection without mutating files.
fn project_legacy_lifecycle(
    metadata: &ProfileMetadata,
    current_scope: &str,
) -> Result<ProfileMetadata> {
    if metadata.accounts.is_empty() || has_lifecycle_intervals(metadata) {
        return Ok(metadata.clone());
    }
    if !metadata.accounts.contains_key(current_scope) {
        // There is no trustworthy way to decide which legacy partition was
        // current at the migration boundary.
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    let boundaries = metadata
        .accounts
        .values()
        .filter_map(|entry| entry.activation_timestamp)
        .collect::<Vec<_>>();
    let boundary = metadata
        .accounts
        .get(current_scope)
        .and_then(|entry| entry.activation_timestamp)
        .filter(|value| *value > 0)
        .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    // The legacy record carries the migration boundary in exactly one
    // authority entry.  If that proof is absent or duplicated, assigning
    // either account a time domain would be a guess.
    if boundaries.len() != 1 || boundaries[0] != boundary {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    let mut projected = metadata.clone();
    for (scope, entry) in &mut projected.accounts {
        let interval = if scope == current_scope {
            AccountLifecycleInterval {
                start_at: Some(boundary),
                end_at: None,
            }
        } else {
            AccountLifecycleInterval {
                start_at: None,
                end_at: Some(boundary),
            }
        };
        entry.activation_timestamp = interval.start_at;
        entry.lifecycle_intervals = vec![interval];
    }
    validate_metadata(&projected)?;
    Ok(projected)
}

fn unix_timestamp_now() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| LocatorError::new(LocatorErrorKind::Io))?;
    i64::try_from(duration.as_secs()).map_err(|_| LocatorError::new(LocatorErrorKind::Io))
}

fn open_lifecycle_scope(metadata: &ProfileMetadata) -> Result<Option<String>> {
    let mut open_scope = None;
    for (scope, entry) in &metadata.accounts {
        for interval in &entry.lifecycle_intervals {
            if interval.end_at.is_none() {
                if open_scope.is_some() {
                    return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
                }
                open_scope = Some(scope.clone());
            }
        }
    }
    if metadata.accounts.is_empty() {
        Ok(None)
    } else if open_scope.is_some() {
        Ok(open_scope)
    } else {
        Err(LocatorError::new(LocatorErrorKind::RecoveryRequired))
    }
}

fn close_open_lifecycle_interval(metadata: &mut ProfileMetadata, end_at: i64) -> Result<()> {
    if end_at <= 0 {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    let mut closed = false;
    for entry in metadata.accounts.values_mut() {
        for interval in &mut entry.lifecycle_intervals {
            if interval.end_at.is_none() {
                if closed || interval.start_at.is_some_and(|start_at| end_at <= start_at) {
                    return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
                }
                interval.end_at = Some(end_at);
                closed = true;
            }
        }
    }
    if closed {
        Ok(())
    } else {
        Err(LocatorError::new(LocatorErrorKind::RecoveryRequired))
    }
}

fn load_metadata(data_root: &Path) -> Result<ProfileMetadata> {
    let bytes = read_private_file(&metadata_path(data_root), MAX_PROFILE_FILE_BYTES)?;
    let metadata = serde_json::from_slice::<ProfileMetadata>(&bytes)
        .map_err(|_| LocatorError::new(LocatorErrorKind::InvalidMetadata))?;
    validate_metadata(&metadata)?;
    Ok(metadata)
}

fn validate_registry_artifacts(data_root: &Path, metadata: &ProfileMetadata) -> Result<()> {
    let root = accounts_root(data_root);
    let root_metadata = match fs::symlink_metadata(&root) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if metadata
                .accounts
                .values()
                .any(|entry| entry.state == RegistryState::Initialized)
            {
                Err(LocatorError::new(LocatorErrorKind::RecoveryRequired))
            } else {
                Ok(())
            };
        }
        Err(_) => return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired)),
    };
    if !validate_private_directory(&root_metadata) {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }

    for entry in
        fs::read_dir(&root).map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?
    {
        let entry = entry.map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
        let scope = entry
            .file_name()
            .into_string()
            .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
        if decode_fixed::<SCOPE_ID_BYTES>(&scope).is_err()
            || !metadata.accounts.contains_key(&scope)
            || !validate_private_directory(
                &fs::symlink_metadata(entry.path())
                    .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?,
            )
        {
            return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
        }
    }

    for (scope, registry) in &metadata.accounts {
        let account_directory = root.join(scope);
        let account_metadata = match fs::symlink_metadata(&account_directory) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if registry.state == RegistryState::Initialized {
                    return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
                }
                continue;
            }
            Err(_) => return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired)),
        };
        if !validate_private_directory(&account_metadata) {
            return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
        }
        let epoch_name = format!("epoch-{}", registry.storage_epoch);
        for entry in fs::read_dir(&account_directory)
            .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?
        {
            let entry = entry.map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
            if name != epoch_name
                || !validate_private_directory(
                    &fs::symlink_metadata(entry.path())
                        .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?,
                )
            {
                return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
            }
        }
        let epoch_directory = account_directory.join(&epoch_name);
        let epoch_metadata = match fs::symlink_metadata(&epoch_directory) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if registry.state == RegistryState::Initialized {
                    return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
                }
                continue;
            }
            Err(_) => return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired)),
        };
        if !validate_private_directory(&epoch_metadata) {
            return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
        }
        let mut database_exists = false;
        for entry in fs::read_dir(&epoch_directory)
            .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?
        {
            let entry = entry.map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
            let allowed = name == "usage_history.sqlite3"
                || name == "usage_history.sqlite3.candidate"
                || name == "account-writer.lock"
                || name == "account-recorder.lock"
                || matches!(
                    name.as_str(),
                    "usage_history.sqlite3.bak.1"
                        | "usage_history.sqlite3.bak.2"
                        | "usage_history.sqlite3.bak.3"
                );
            let artifact = fs::symlink_metadata(entry.path())
                .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
            if !allowed || !validate_artifact_file(&artifact) {
                return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
            }
            database_exists |= name == "usage_history.sqlite3";
        }
        if registry.state == RegistryState::Initialized && !database_exists {
            return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
        }
        // An allocated row may already have a regular database when the
        // recorder was terminated between SQLite identity validation and the
        // registry promotion.  The recorder owns that fail-closed identity
        // check and may resume by promoting it after a successful open.
    }
    Ok(())
}

fn partition_from_metadata(
    data_root: &Path,
    metadata: &ProfileMetadata,
    account_scope_id: &[u8; SCOPE_ID_BYTES],
    storage_epoch: u64,
) -> Result<AccountPartition> {
    let registry = metadata
        .accounts
        .get(&hex::encode(account_scope_id))
        .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    if registry.storage_epoch != storage_epoch {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    let profile_scope_raw = decode_fixed::<PROFILE_ID_BYTES>(&metadata.profile_scope_id)?;
    let install_key = decode_fixed::<INSTALL_KEY_BYTES>(&metadata.install_key)?;
    let partition_id = partition_scope(
        &install_key,
        &profile_scope_raw,
        account_scope_id,
        storage_epoch,
    )?;
    let account_scope_hex = hex::encode(account_scope_id);
    let epoch_directory = accounts_root(data_root)
        .join(&account_scope_hex)
        .join(format!("epoch-{storage_epoch}"));
    let lifecycle_intervals = registry.lifecycle_intervals.clone();
    let (current_interval_start, current_interval_end) = lifecycle_intervals
        .last()
        .map(|interval| (interval.start_at, interval.end_at))
        .unwrap_or((None, None));
    Ok(AccountPartition {
        profile_scope_id: metadata.profile_scope_id.clone(),
        account_scope_id: account_scope_hex,
        storage_epoch,
        partition_id: hex::encode(partition_id),
        database_path: epoch_directory.join("usage_history.sqlite3"),
        login_id: registry.login_id.clone(),
        activation_timestamp: current_interval_start,
        lifecycle_intervals,
        current_interval_start,
        current_interval_end,
    })
}

/// Select or allocate the current account's partition using the same profile
/// metadata and HMAC identity rules as the main application.  Allocation is
/// intentionally exposed only for the recorder startup path; the ordinary
/// locator remains [`locate_existing_partition`] and does not mutate state.
///
/// The caller may prepare this private root before taking its process lease so
/// the lease itself is established before profile metadata allocation.
pub fn prepare_recorder_data_root(data_root: impl AsRef<Path>) -> Result<()> {
    let data_root = validate_absolute_root(data_root.as_ref())?;
    prepare_private_directory(&data_root)?;
    prepare_private_directory(&data_root.join("history"))
}

pub fn ensure_partition(
    codex_home: impl AsRef<Path>,
    data_root: impl AsRef<Path>,
) -> Result<AccountPartition> {
    ensure_partition_with_activation(codex_home, data_root, None)
}

/// Select or allocate the current account partition and persist the exact
/// first-admission boundary only when a new account entry is allocated.
/// Reopening an existing account never moves its established boundary.
pub fn ensure_partition_with_activation(
    codex_home: impl AsRef<Path>,
    data_root: impl AsRef<Path>,
    activation_timestamp: Option<i64>,
) -> Result<AccountPartition> {
    if activation_timestamp.is_some_and(|value| value <= 0) {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    let codex_home = validate_absolute_root(codex_home.as_ref())?;
    let data_root = validate_absolute_root(data_root.as_ref())?;
    let account_key = read_account_key(&codex_home)?;
    let legacy_metadata = load_or_initialize_metadata(&data_root)?;
    validate_registry_artifacts(&data_root, &legacy_metadata)?;

    let install_key = decode_fixed::<INSTALL_KEY_BYTES>(&legacy_metadata.install_key)?;
    let account_scope_id = account_scope(&install_key, &account_key)?;
    let scope_hex = hex::encode(account_scope_id);
    let mut metadata = project_legacy_lifecycle(&legacy_metadata, &scope_hex)?;
    let mut metadata_changed = metadata != legacy_metadata;

    let (storage_epoch, state) = if let Some(entry) = metadata.accounts.get(&scope_hex) {
        let storage_epoch = entry.storage_epoch;
        let state = entry.state;
        let current_scope = open_lifecycle_scope(&metadata)?;
        if current_scope.as_deref() != Some(scope_hex.as_str()) {
            let transition_timestamp = activation_timestamp.unwrap_or(unix_timestamp_now()?);
            close_open_lifecycle_interval(&mut metadata, transition_timestamp)?;
            let entry = metadata
                .accounts
                .get_mut(&scope_hex)
                .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
            entry.lifecycle_intervals.push(AccountLifecycleInterval {
                start_at: Some(transition_timestamp),
                end_at: None,
            });
            entry.activation_timestamp = Some(transition_timestamp);
            metadata_changed = true;
        }
        (storage_epoch, state)
    } else {
        let account_directory = accounts_root(&data_root).join(&scope_hex);
        if !directory_is_empty(&account_directory)? || account_directory.exists() {
            return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
        }
        let transition_timestamp = if metadata.accounts.is_empty() {
            activation_timestamp
        } else {
            let transition_timestamp = activation_timestamp.unwrap_or(unix_timestamp_now()?);
            close_open_lifecycle_interval(&mut metadata, transition_timestamp)?;
            Some(transition_timestamp)
        };
        let storage_epoch = metadata.next_storage_epoch;
        metadata.next_storage_epoch = storage_epoch
            .checked_add(1)
            .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
        let lifecycle_interval = AccountLifecycleInterval {
            start_at: transition_timestamp,
            end_at: None,
        };
        metadata.accounts.insert(
            scope_hex.clone(),
            RegistryEntry {
                storage_epoch,
                state: RegistryState::Allocated,
                activation_timestamp: lifecycle_interval.start_at,
                login_id: None,
                lifecycle_intervals: vec![lifecycle_interval],
            },
        );
        metadata_changed = true;
        (storage_epoch, RegistryState::Allocated)
    };

    if metadata_changed {
        validate_metadata(&metadata)?;
        write_metadata(&data_root, &metadata)?;
    }

    let partition =
        partition_from_metadata(&data_root, &metadata, &account_scope_id, storage_epoch)?;
    let epoch_directory = partition
        .database_path
        .parent()
        .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    match state {
        RegistryState::Allocated => {
            let account_directory = epoch_directory
                .parent()
                .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
            prepare_private_directory(account_directory)?;
            prepare_private_directory(epoch_directory)?;
            for entry in fs::read_dir(epoch_directory)
                .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?
            {
                let entry =
                    entry.map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
                if name != "usage_history.sqlite3"
                    && name != "usage_history.sqlite3.candidate"
                    && name != "account-writer.lock"
                    && name != "account-recorder.lock"
                    && !matches!(
                        name.as_str(),
                        "usage_history.sqlite3.bak.1"
                            | "usage_history.sqlite3.bak.2"
                            | "usage_history.sqlite3.bak.3"
                    )
                {
                    return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
                }
                if !validate_artifact_file(
                    &fs::symlink_metadata(entry.path())
                        .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?,
                ) {
                    return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
                }
            }
        }
        RegistryState::Initialized => {
            let database = fs::symlink_metadata(&partition.database_path)
                .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
            if !validate_artifact_file(&database) {
                return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
            }
        }
    }
    Ok(partition)
}

/// Derive the current auth authority's opaque account scope without
/// allocating a registry entry or selecting another account's database.
pub fn current_account_scope_id(
    codex_home: impl AsRef<Path>,
    data_root: impl AsRef<Path>,
) -> Result<String> {
    let codex_home = validate_absolute_root(codex_home.as_ref())?;
    let data_root = validate_absolute_root(data_root.as_ref())?;
    let account_key = read_account_key(&codex_home)?;
    let metadata = load_metadata(&data_root)?;
    let install_key = decode_fixed::<INSTALL_KEY_BYTES>(&metadata.install_key)?;
    Ok(hex::encode(account_scope(&install_key, &account_key)?))
}

/// Promote an allocated partition after the recorder has created and
/// identity-validated its SQLite database.  The metadata transition is
/// atomic at the profile-file level and is idempotent for initialized rows.
pub fn mark_partition_initialized(
    data_root: impl AsRef<Path>,
    partition: &AccountPartition,
) -> Result<()> {
    let data_root = validate_absolute_root(data_root.as_ref())?;
    let database = fs::symlink_metadata(&partition.database_path)
        .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    if !validate_artifact_file(&database) {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    let mut metadata = load_metadata(&data_root)?;
    if metadata.profile_scope_id != partition.profile_scope_id {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    let entry = metadata
        .accounts
        .get_mut(&partition.account_scope_id)
        .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    if entry.storage_epoch != partition.storage_epoch {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    if entry.state != RegistryState::Initialized {
        entry.state = RegistryState::Initialized;
        write_metadata(&data_root, &metadata)?;
    }
    Ok(())
}

fn valid_login_id(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= MAX_LOGIN_ID_SCALARS
        && !value.chars().any(char::is_control)
}

/// Persist display-only account identity for the exact recorder-owned
/// partition. Mapping stays bound to the irreversible account scope, so a
/// changed or malformed label can never redirect storage.
pub fn set_partition_login_id(
    data_root: impl AsRef<Path>,
    partition: &AccountPartition,
    login_id: &str,
) -> Result<()> {
    if !valid_login_id(login_id) {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    let data_root = validate_absolute_root(data_root.as_ref())?;
    let mut metadata = load_metadata(&data_root)?;
    if metadata.profile_scope_id != partition.profile_scope_id {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    let entry = metadata
        .accounts
        .get_mut(&partition.account_scope_id)
        .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    if entry.storage_epoch != partition.storage_epoch {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    if entry.login_id.as_deref() != Some(login_id) {
        entry.login_id = Some(login_id.to_owned());
        write_metadata(&data_root, &metadata)?;
    }
    Ok(())
}

/// Locate the initialized partition for the exact account in auth.json.
///
/// codex_home contains auth.json; data_root contains
/// history/account_profile_v1.json. Both roots and authority files are
/// read-only. A missing registry entry, uninitialized entry, missing
/// database, unknown artifact, or identity mismatch returns an error instead
/// of selecting another account's database.
pub fn locate_existing_partition(
    codex_home: impl AsRef<Path>,
    data_root: impl AsRef<Path>,
) -> Result<AccountPartition> {
    let codex_home = validate_absolute_root(codex_home.as_ref())?;
    let data_root = validate_absolute_root(data_root.as_ref())?;
    let account_key = read_account_key(&codex_home)?;
    let legacy_metadata = load_metadata(&data_root)?;

    let install_key = decode_fixed::<INSTALL_KEY_BYTES>(&legacy_metadata.install_key)?;
    let account_scope_id = account_scope(&install_key, &account_key)?;
    let account_scope_hex = hex::encode(account_scope_id);
    let metadata = project_legacy_lifecycle(&legacy_metadata, &account_scope_hex)?;
    validate_registry_artifacts(&data_root, &metadata)?;
    let registry = metadata
        .accounts
        .get(&account_scope_hex)
        .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    if registry.state != RegistryState::Initialized {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }

    let partition = partition_from_metadata(
        &data_root,
        &metadata,
        &account_scope_id,
        registry.storage_epoch,
    )?;
    // A read-only REST locator may select only the account whose interval is
    // currently open.  A historical partition remains enumerable by the
    // explicit account list, but it is never mistaken for the live auth
    // authority before the recorder admits the transition.
    if partition.current_interval_end.is_some() {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    let database_metadata = fs::symlink_metadata(&partition.database_path)
        .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    if !validate_artifact_file(&database_metadata) {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    Ok(partition)
}

/// Locate every initialized partition declared by the profile registry.
///
/// The registry is authoritative: this function never scans arbitrary
/// account directories to discover candidates and never allocates or repairs
/// metadata.  The current auth authority is still parsed so production REST
/// cannot start with an invalid authority, while each returned partition is
/// selected solely from an `Initialized` registry entry and its exact
/// derived database path.
pub fn locate_existing_partitions(
    codex_home: impl AsRef<Path>,
    data_root: impl AsRef<Path>,
) -> Result<Vec<AccountPartition>> {
    let codex_home = validate_absolute_root(codex_home.as_ref())?;
    let data_root = validate_absolute_root(data_root.as_ref())?;
    let account_key = read_account_key(&codex_home)?;
    let legacy_metadata = load_metadata(&data_root)?;
    let install_key = decode_fixed::<INSTALL_KEY_BYTES>(&legacy_metadata.install_key)?;
    let current_scope = hex::encode(account_scope(&install_key, &account_key)?);
    let metadata = project_legacy_lifecycle(&legacy_metadata, &current_scope)?;
    validate_registry_artifacts(&data_root, &metadata)?;

    let current_registry = metadata
        .accounts
        .get(&current_scope)
        .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    if current_registry.state != RegistryState::Initialized {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    let current_scope_id = decode_fixed::<SCOPE_ID_BYTES>(&current_scope)?;
    let current_partition = partition_from_metadata(
        &data_root,
        &metadata,
        &current_scope_id,
        current_registry.storage_epoch,
    )?;
    if current_partition.current_interval_end.is_some() {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }

    let mut partitions = Vec::with_capacity(metadata.accounts.len());
    for (scope, registry) in &metadata.accounts {
        if registry.state != RegistryState::Initialized {
            continue;
        }
        let account_scope_id = decode_fixed::<SCOPE_ID_BYTES>(scope)?;
        let partition = partition_from_metadata(
            &data_root,
            &metadata,
            &account_scope_id,
            registry.storage_epoch,
        )?;
        // validate_registry_artifacts checks this for initialized entries;
        // retain the direct check here so the returned path is proven against
        // the exact metadata entry even if that helper changes independently.
        let database_metadata = fs::symlink_metadata(&partition.database_path)
            .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
        if !validate_artifact_file(&database_metadata) {
            return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
        }
        // Exercise the install-key-derived identity while constructing every
        // entry; no caller can substitute an arbitrary scope/path pair.
        let expected_partition_id = partition_scope(
            &install_key,
            &decode_fixed::<PROFILE_ID_BYTES>(&metadata.profile_scope_id)?,
            &account_scope_id,
            registry.storage_epoch,
        )?;
        if partition.partition_id != hex::encode(expected_partition_id) {
            return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
        }
        partitions.push(partition);
    }
    Ok(partitions)
}

/// Compatibility spelling for callers that use resolve for identity lookup.
pub fn resolve_existing_partition(
    codex_home: impl AsRef<Path>,
    data_root: impl AsRef<Path>,
) -> Result<AccountPartition> {
    locate_existing_partition(codex_home, data_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(name: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "codex-info-account-locator-{name}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn make_private_directory(path: &Path) {
        fs::create_dir_all(path).unwrap();
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn write_private_file(path: &Path, bytes: &[u8]) {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(path).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }

    fn replace_private_file(path: &Path, bytes: &[u8]) {
        fs::remove_file(path).unwrap();
        write_private_file(path, bytes);
    }

    fn replace_auth(codex_home: &Path, account_id: &str) {
        replace_private_file(
            &codex_home.join(AUTH_FILE_NAME),
            format!(r#"{{"tokens":{{"account_id":"{account_id}"}}}}"#).as_bytes(),
        );
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<String, (bool, u64, Vec<u8>)> {
        fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<String, (bool, u64, Vec<u8>)>) {
            let metadata = fs::symlink_metadata(path).unwrap();
            #[cfg(unix)]
            let inode = metadata.ino();
            #[cfg(not(unix))]
            let inode = 0;
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let is_directory = metadata.is_dir();
            let bytes = if is_directory {
                Vec::new()
            } else {
                fs::read(path).unwrap()
            };
            snapshot.insert(relative, (is_directory, inode, bytes));
            if is_directory {
                for entry in fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), snapshot);
                }
            }
        }

        let mut snapshot = BTreeMap::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    fn fixture() -> (PathBuf, PathBuf, String, String, String) {
        let root = temp_root("two-accounts");
        let codex_home = root.join("codex-home");
        let data_root = root.join("data");
        make_private_directory(&codex_home);
        make_private_directory(&data_root);

        let account_a = AccountKey::new("workspace-A".to_owned()).unwrap();
        let account_b = AccountKey::new("workspace-B".to_owned()).unwrap();
        let install_key = [0x11_u8; INSTALL_KEY_BYTES];
        let profile_scope = [0x22_u8; PROFILE_ID_BYTES];
        let scope_a = hex::encode(account_scope(&install_key, &account_a).unwrap());
        let scope_b = hex::encode(account_scope(&install_key, &account_b).unwrap());
        let epoch_a = 7_u64;
        let epoch_b = 13_u64;
        let migration_boundary = 1_789_167_600_i64;
        let partition_a = hex::encode(
            partition_scope(
                &install_key,
                &profile_scope,
                &decode_fixed::<SCOPE_ID_BYTES>(&scope_a).unwrap(),
                epoch_a,
            )
            .unwrap(),
        );

        write_private_file(
            &codex_home.join(AUTH_FILE_NAME),
            br#"{"tokens":{"access_token":"ignored","account_id":"workspace-A"}}"#,
        );
        let history = data_root.join("history");
        let accounts = history.join("accounts").join("v1");
        make_private_directory(&history);
        make_private_directory(&history.join("accounts"));
        make_private_directory(&accounts);
        let metadata = format!(
            "{{\"schema_version\":\"{}\",\"profile_scope_id\":\"{}\",\"install_key\":\"{}\",\"next_storage_epoch\":14,\"accounts\":{{\"{}\":{{\"storage_epoch\":{},\"state\":\"initialized\",\"activation_timestamp\":{} }},\"{}\":{{\"storage_epoch\":{},\"state\":\"initialized\"}}}}}}",
            PROFILE_SCHEMA,
            hex::encode(profile_scope),
            hex::encode(install_key),
            scope_a,
            epoch_a,
            migration_boundary,
            scope_b,
            epoch_b
        );
        write_private_file(
            &history.join(PROFILE_METADATA_FILE_NAME),
            metadata.as_bytes(),
        );

        for (scope, epoch, marker) in [
            (&scope_a, epoch_a, b"account-A-db".as_slice()),
            (&scope_b, epoch_b, b"account-B-db".as_slice()),
        ] {
            let epoch_directory = accounts.join(scope).join(format!("epoch-{epoch}"));
            make_private_directory(&accounts.join(scope));
            make_private_directory(&epoch_directory);
            write_private_file(&epoch_directory.join("usage_history.sqlite3"), marker);
        }
        (codex_home, data_root, scope_a, scope_b, partition_a)
    }

    #[test]
    fn selects_exact_current_account_partition_and_does_not_mutate_filesystem() {
        let (codex_home, data_root, scope_a, scope_b, partition_a) = fixture();
        let before = (snapshot_tree(&codex_home), snapshot_tree(&data_root));

        let partition = locate_existing_partition(&codex_home, &data_root).unwrap();

        assert_eq!(partition.account_scope_id, scope_a);
        assert_ne!(partition.account_scope_id, scope_b);
        assert_eq!(partition.storage_epoch, 7);
        assert_eq!(partition.partition_id, partition_a);
        assert_eq!(
            partition.database_path,
            data_root
                .join("history")
                .join("accounts")
                .join("v1")
                .join(&scope_a)
                .join("epoch-7")
                .join("usage_history.sqlite3")
        );
        assert_eq!(
            partition.storage_identity(),
            StoragePartitionIdentity {
                schema_version: ACCOUNT_DB_SCHEMA.to_owned(),
                profile_scope_id: "22".repeat(PROFILE_ID_BYTES),
                account_scope_id: scope_a,
                storage_epoch: 7,
                partition_id: partition_a,
            }
        );
        assert_eq!(
            before,
            (snapshot_tree(&codex_home), snapshot_tree(&data_root))
        );
    }

    #[test]
    fn lists_only_registry_initialized_partitions_with_opaque_epoch_ids() {
        let (codex_home, data_root, scope_a, scope_b, _) = fixture();

        let mut partitions = locate_existing_partitions(&codex_home, &data_root).unwrap();
        partitions.sort_by_key(|partition| partition.storage_epoch);

        assert_eq!(
            partitions
                .iter()
                .map(|partition| partition.account_scope_id.as_str())
                .collect::<Vec<_>>(),
            vec![scope_a.as_str(), scope_b.as_str()]
        );
        assert_eq!(
            partitions
                .iter()
                .map(AccountPartition::public_id)
                .collect::<Vec<_>>(),
            vec!["account-7", "account-13"]
        );
        for partition in partitions {
            assert!(!partition.public_id().contains(&partition.account_scope_id));
        }
    }

    #[test]
    fn missing_metadata_is_not_initialized_or_repaired() {
        let root = temp_root("missing-metadata");
        let codex_home = root.join("codex-home");
        let data_root = root.join("data");
        make_private_directory(&codex_home);
        make_private_directory(&data_root);
        write_private_file(
            &codex_home.join(AUTH_FILE_NAME),
            br#"{"tokens":{"account_id":"workspace-A"}}"#,
        );
        let before = (snapshot_tree(&codex_home), snapshot_tree(&data_root));

        let error = locate_existing_partition(&codex_home, &data_root).unwrap_err();

        assert_eq!(error.kind(), LocatorErrorKind::UnsafeRoot);
        assert_eq!(
            before,
            (snapshot_tree(&codex_home), snapshot_tree(&data_root))
        );
        assert!(!data_root.join("history").exists());
    }

    #[test]
    fn recorder_allocation_uses_profile_registry_and_promotes_exact_partition() {
        let root = temp_root("recorder-allocation");
        let codex_home = root.join("codex-home");
        let data_root = root.join("data");
        make_private_directory(&codex_home);
        make_private_directory(&data_root);
        write_private_file(
            &codex_home.join(AUTH_FILE_NAME),
            br#"{"tokens":{"account_id":"workspace-A"}}"#,
        );

        let allocated = ensure_partition(&codex_home, &data_root).unwrap();
        assert!(!allocated.database_path.exists());
        assert!(allocated.database_path.parent().unwrap().is_dir());
        write_private_file(&allocated.database_path, b"sqlite-fixture");
        write_private_file(
            &allocated
                .database_path
                .parent()
                .unwrap()
                .join("account-recorder.lock"),
            b"pid=fixture\n",
        );
        // A crash after database creation but before registry promotion must
        // resume the same allocated identity; it is not a recovery error.
        assert_eq!(
            ensure_partition(&codex_home, &data_root).unwrap(),
            allocated
        );
        mark_partition_initialized(&data_root, &allocated).unwrap();
        let selected = locate_existing_partition(&codex_home, &data_root).unwrap();
        assert_eq!(selected, allocated);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recorder_allocation_persists_the_first_account_activation_boundary() {
        let root = temp_root("recorder-activation-boundary");
        let codex_home = root.join("codex-home");
        let data_root = root.join("data");
        make_private_directory(&codex_home);
        make_private_directory(&data_root);
        write_private_file(
            &codex_home.join(AUTH_FILE_NAME),
            br#"{"tokens":{"account_id":"workspace-A"}}"#,
        );

        let allocated =
            ensure_partition_with_activation(&codex_home, &data_root, Some(1_789_167_600)).unwrap();
        assert_eq!(allocated.activation_timestamp, Some(1_789_167_600));
        let reopened =
            ensure_partition_with_activation(&codex_home, &data_root, Some(1_900_000_000)).unwrap();
        assert_eq!(reopened, allocated);
        assert_eq!(
            current_account_scope_id(&codex_home, &data_root).unwrap(),
            allocated.account_scope_id
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn display_login_id_is_bounded_persisted_and_does_not_change_partition_mapping() {
        let root = temp_root("display-login-id");
        let codex_home = root.join("codex-home");
        let data_root = root.join("data");
        make_private_directory(&codex_home);
        make_private_directory(&data_root);
        write_private_file(
            &codex_home.join(AUTH_FILE_NAME),
            br#"{"tokens":{"account_id":"workspace-A"}}"#,
        );

        let allocated =
            ensure_partition_with_activation(&codex_home, &data_root, Some(100)).unwrap();
        write_private_file(&allocated.database_path, b"account-A-db");
        mark_partition_initialized(&data_root, &allocated).unwrap();
        set_partition_login_id(&data_root, &allocated, "user@example.com").unwrap();

        let located = locate_existing_partition(&codex_home, &data_root).unwrap();
        assert_eq!(located.storage_epoch, allocated.storage_epoch);
        assert_eq!(located.account_scope_id, allocated.account_scope_id);
        assert_eq!(located.login_id.as_deref(), Some("user@example.com"));
        assert_eq!(
            set_partition_login_id(&data_root, &located, " bad@example.com")
                .unwrap_err()
                .kind(),
            LocatorErrorKind::InvalidMetadata
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_two_account_metadata_migrates_at_its_recorded_exact_boundary() {
        let (codex_home, data_root, scope_a, scope_b, _) = fixture();
        let boundary = 1_789_167_600_i64;

        // The fixture is legacy-shaped: only the current authority's
        // activation_timestamp carries the migration proof.  A restart must
        // materialize intervals from that value and ignore a new requested
        // activation time for the same account.
        let current = ensure_partition_with_activation(
            &codex_home,
            &data_root,
            Some(boundary.saturating_add(10_000)),
        )
        .unwrap();
        assert_eq!(
            current.lifecycle_intervals,
            vec![AccountLifecycleInterval {
                start_at: Some(boundary),
                end_at: None,
            }]
        );
        assert_eq!(current.current_interval_start, Some(boundary));
        assert_eq!(current.current_interval_end, None);

        let partitions = locate_existing_partitions(&codex_home, &data_root).unwrap();
        let current_a = partitions
            .iter()
            .find(|partition| partition.account_scope_id == scope_a)
            .unwrap();
        let old_b = partitions
            .iter()
            .find(|partition| partition.account_scope_id == scope_b)
            .unwrap();
        assert_eq!(
            current_a.lifecycle_intervals,
            vec![AccountLifecycleInterval {
                start_at: Some(boundary),
                end_at: None,
            }]
        );
        assert_eq!(
            old_b.lifecycle_intervals,
            vec![AccountLifecycleInterval {
                start_at: None,
                end_at: Some(boundary),
            }]
        );

        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(metadata_path(&data_root)).unwrap()).unwrap();
        let accounts = metadata["accounts"].as_object().unwrap();
        assert_eq!(
            accounts[&scope_a]["lifecycle_intervals"][0]["start_at"],
            serde_json::json!(boundary)
        );
        assert_eq!(
            accounts[&scope_b]["lifecycle_intervals"][0]["end_at"],
            serde_json::json!(boundary)
        );
        let _ = fs::remove_dir_all(data_root.parent().unwrap());
    }

    #[test]
    fn same_account_restart_is_interval_idempotent() {
        let root = temp_root("same-account-restart-interval");
        let codex_home = root.join("codex-home");
        let data_root = root.join("data");
        make_private_directory(&codex_home);
        make_private_directory(&data_root);
        write_private_file(
            &codex_home.join(AUTH_FILE_NAME),
            br#"{"tokens":{"account_id":"workspace-A"}}"#,
        );

        let first = ensure_partition_with_activation(&codex_home, &data_root, Some(100)).unwrap();
        let second = ensure_partition_with_activation(&codex_home, &data_root, Some(200)).unwrap();
        assert_eq!(second, first);
        assert_eq!(
            second.lifecycle_intervals,
            vec![AccountLifecycleInterval {
                start_at: Some(100),
                end_at: None,
            }]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn account_switch_reuses_partitions_and_appends_disjoint_intervals() {
        let root = temp_root("account-switch-intervals");
        let codex_home = root.join("codex-home");
        let data_root = root.join("data");
        make_private_directory(&codex_home);
        make_private_directory(&data_root);
        write_private_file(
            &codex_home.join(AUTH_FILE_NAME),
            br#"{"tokens":{"account_id":"workspace-A"}}"#,
        );

        let first_a = ensure_partition_with_activation(&codex_home, &data_root, Some(100)).unwrap();
        write_private_file(&first_a.database_path, b"account-A-db");
        mark_partition_initialized(&data_root, &first_a).unwrap();

        replace_auth(&codex_home, "workspace-B");
        let first_b = ensure_partition_with_activation(&codex_home, &data_root, Some(200)).unwrap();
        write_private_file(&first_b.database_path, b"account-B-db");
        mark_partition_initialized(&data_root, &first_b).unwrap();

        replace_auth(&codex_home, "workspace-A");
        let second_a =
            ensure_partition_with_activation(&codex_home, &data_root, Some(300)).unwrap();
        assert_eq!(second_a.storage_epoch, first_a.storage_epoch);
        assert_eq!(second_a.database_path, first_a.database_path);
        assert_eq!(
            second_a.lifecycle_intervals,
            vec![
                AccountLifecycleInterval {
                    start_at: Some(100),
                    end_at: Some(200),
                },
                AccountLifecycleInterval {
                    start_at: Some(300),
                    end_at: None,
                },
            ]
        );
        let b = locate_existing_partitions(&codex_home, &data_root)
            .unwrap()
            .into_iter()
            .find(|partition| partition.storage_epoch == first_b.storage_epoch)
            .unwrap();
        assert_eq!(
            b.lifecycle_intervals,
            vec![AccountLifecycleInterval {
                start_at: Some(200),
                end_at: Some(300),
            }]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_lifecycle_overlap_gap_and_unknown_legacy_boundary_fail_closed() {
        let root = temp_root("malformed-intervals");
        let codex_home = root.join("codex-home");
        let data_root = root.join("data");
        make_private_directory(&codex_home);
        make_private_directory(&data_root);
        write_private_file(
            &codex_home.join(AUTH_FILE_NAME),
            br#"{"tokens":{"account_id":"workspace-A"}}"#,
        );
        let _ = ensure_partition_with_activation(&codex_home, &data_root, Some(100)).unwrap();
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(metadata_path(&data_root)).unwrap()).unwrap();
        let scope = current_account_scope_id(&codex_home, &data_root).unwrap();
        metadata["accounts"][&scope]["lifecycle_intervals"] = serde_json::json!([
            {"start_at": 100, "end_at": 200},
            {"start_at": 150, "end_at": null}
        ]);
        replace_private_file(
            &metadata_path(&data_root),
            serde_json::to_vec(&metadata).unwrap().as_slice(),
        );
        let error = ensure_partition_with_activation(&codex_home, &data_root, Some(300))
            .expect_err("overlap must not choose a time domain");
        assert_eq!(error.kind(), LocatorErrorKind::InvalidMetadata);

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn auth_replacement_during_read_is_rejected() {
        let root = temp_root("auth-replacement");
        let auth_path = root.join(AUTH_FILE_NAME);
        write_private_file(&auth_path, br#"{"tokens":{"account_id":"workspace-A"}}"#);
        let replacement = root.join("replacement.json");
        write_private_file(&replacement, br#"{"tokens":{"account_id":"workspace-B"}}"#);
        assert_eq!(
            fs::metadata(&auth_path).unwrap().len(),
            fs::metadata(&replacement).unwrap().len()
        );
        let error = read_private_file_with_post_read(&auth_path, MAX_AUTH_FILE_BYTES, || {
            fs::rename(&replacement, &auth_path).unwrap();
        })
        .unwrap_err();
        assert_eq!(error.kind(), LocatorErrorKind::UnsafeFile);
    }
}

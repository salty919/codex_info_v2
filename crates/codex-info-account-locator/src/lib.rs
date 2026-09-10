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
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

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

/// An already initialized account partition selected by the current auth
/// authority. No directory scan or newest-file heuristic is used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountPartition {
    pub profile_scope_id: String,
    pub account_scope_id: String,
    pub storage_epoch: u64,
    pub partition_id: String,
    pub database_path: PathBuf,
}

impl AccountPartition {
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
}

#[derive(Deserialize, Serialize)]
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
    for (scope, entry) in &metadata.accounts {
        decode_fixed::<SCOPE_ID_BYTES>(scope)?;
        if entry.storage_epoch == 0 {
            return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
        }
        maximum_epoch = maximum_epoch.max(entry.storage_epoch);
    }
    if metadata.next_storage_epoch <= maximum_epoch {
        return Err(LocatorError::new(LocatorErrorKind::InvalidMetadata));
    }
    Ok(())
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
    Ok(AccountPartition {
        profile_scope_id: metadata.profile_scope_id.clone(),
        account_scope_id: account_scope_hex,
        storage_epoch,
        partition_id: hex::encode(partition_id),
        database_path: epoch_directory.join("usage_history.sqlite3"),
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
    let codex_home = validate_absolute_root(codex_home.as_ref())?;
    let data_root = validate_absolute_root(data_root.as_ref())?;
    let account_key = read_account_key(&codex_home)?;
    let mut metadata = load_or_initialize_metadata(&data_root)?;
    validate_registry_artifacts(&data_root, &metadata)?;

    let install_key = decode_fixed::<INSTALL_KEY_BYTES>(&metadata.install_key)?;
    let account_scope_id = account_scope(&install_key, &account_key)?;
    let scope_hex = hex::encode(account_scope_id);
    let (storage_epoch, state) = if let Some(entry) = metadata.accounts.get(&scope_hex) {
        (entry.storage_epoch, entry.state)
    } else {
        let account_directory = accounts_root(&data_root).join(&scope_hex);
        if !directory_is_empty(&account_directory)? || account_directory.exists() {
            return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
        }
        let storage_epoch = metadata.next_storage_epoch;
        metadata.next_storage_epoch = storage_epoch
            .checked_add(1)
            .ok_or_else(|| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
        metadata.accounts.insert(
            scope_hex,
            RegistryEntry {
                storage_epoch,
                state: RegistryState::Allocated,
            },
        );
        write_metadata(&data_root, &metadata)?;
        (storage_epoch, RegistryState::Allocated)
    };

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
    let metadata = load_metadata(&data_root)?;
    validate_registry_artifacts(&data_root, &metadata)?;

    let install_key = decode_fixed::<INSTALL_KEY_BYTES>(&metadata.install_key)?;
    let account_scope_id = account_scope(&install_key, &account_key)?;
    let account_scope_hex = hex::encode(account_scope_id);
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
    let database_metadata = fs::symlink_metadata(&partition.database_path)
        .map_err(|_| LocatorError::new(LocatorErrorKind::RecoveryRequired))?;
    if !validate_artifact_file(&database_metadata) {
        return Err(LocatorError::new(LocatorErrorKind::RecoveryRequired));
    }
    Ok(partition)
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
            "{{\"schema_version\":\"{}\",\"profile_scope_id\":\"{}\",\"install_key\":\"{}\",\"next_storage_epoch\":14,\"accounts\":{{\"{}\":{{\"storage_epoch\":{},\"state\":\"initialized\"}},\"{}\":{{\"storage_epoch\":{},\"state\":\"initialized\"}}}}}}",
            PROFILE_SCHEMA,
            hex::encode(profile_scope),
            hex::encode(install_key),
            scope_a,
            epoch_a,
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

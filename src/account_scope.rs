// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! Account identity and physical storage partition authority.
//!
//! Raw account identifiers are deliberately confined to `AccountKey`.  The
//! type has a redacted `Debug` implementation and is never serializable. Only
//! domain-separated HMAC values cross into paths or durable metadata.

use codex_info::security;
use hmac::{Hmac, Mac};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

const AUTH_FILE_NAME: &str = "auth.json";
const PROFILE_METADATA_FILE_NAME: &str = "account_profile_v1.json";
const PROFILE_SCHEMA: &str = "codex-info-profile-scope-v1";
pub(crate) const ACCOUNT_DB_SCHEMA: &str = "codex-info-account-db-v1";
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
pub(crate) enum AccountScopeErrorKind {
    UnsafeRoot,
    UnsafeFile,
    InvalidAuth,
    InvalidMetadata,
    RecoveryRequired,
    Io,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AccountScopeError {
    kind: AccountScopeErrorKind,
}

impl AccountScopeError {
    const fn new(kind: AccountScopeErrorKind) -> Self {
        Self { kind }
    }

    #[cfg(test)]
    pub(crate) const fn kind(self) -> AccountScopeErrorKind {
        self.kind
    }
}

impl fmt::Display for AccountScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            AccountScopeErrorKind::UnsafeRoot => "account root is unsafe",
            AccountScopeErrorKind::UnsafeFile => "account authority file is unsafe",
            AccountScopeErrorKind::InvalidAuth => "account authority is invalid",
            AccountScopeErrorKind::InvalidMetadata => "account profile metadata is invalid",
            AccountScopeErrorKind::RecoveryRequired => "account storage recovery is required",
            AccountScopeErrorKind::Io => "account storage operation failed",
        })
    }
}

impl std::error::Error for AccountScopeError {}

pub(crate) type Result<T> = std::result::Result<T, AccountScopeError>;

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct AccountKey(Vec<u8>);

impl AccountKey {
    fn new(value: String) -> Result<Self> {
        if value.is_empty()
            || value.len() > MAX_ACCOUNT_KEY_BYTES
            || value.chars().any(|character| character.is_control())
        {
            return Err(AccountScopeError::new(AccountScopeErrorKind::InvalidAuth));
        }
        Ok(Self(value.into_bytes()))
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub(crate) fn same_account(&self, other: &Self) -> bool {
        self == other
    }

    pub(crate) fn synthetic_preview(value: &str) -> Self {
        Self::new(value.to_owned()).expect("test account key must be valid")
    }
}

impl fmt::Debug for AccountKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccountKey([redacted])")
    }
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct AccountScopeId([u8; SCOPE_ID_BYTES]);

impl AccountScopeId {
    pub(crate) fn as_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for AccountScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccountScopeId(")?;
        formatter.write_str(&self.as_hex())?;
        formatter.write_str(")")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct AccountPartition {
    pub(crate) profile_scope_id: String,
    pub(crate) account_scope_id: String,
    pub(crate) storage_epoch: u64,
    pub(crate) partition_id: String,
    pub(crate) database_path: PathBuf,
    pub(crate) candidate_path: PathBuf,
    pub(crate) writer_lock_path: PathBuf,
    metadata_path: PathBuf,
}

impl fmt::Debug for AccountPartition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountPartition")
            .field("profile_scope_id", &self.profile_scope_id)
            .field("account_scope_id", &self.account_scope_id)
            .field("storage_epoch", &self.storage_epoch)
            .field("partition_id", &self.partition_id)
            .field("database_path", &self.database_path)
            .finish_non_exhaustive()
    }
}

impl AccountPartition {
    pub(crate) fn storage_identity(&self) -> codex_info::usage_store::StoragePartitionIdentity {
        codex_info::usage_store::StoragePartitionIdentity {
            schema_version: ACCOUNT_DB_SCHEMA.to_owned(),
            profile_scope_id: self.profile_scope_id.clone(),
            account_scope_id: self.account_scope_id.clone(),
            storage_epoch: self.storage_epoch,
            partition_id: self.partition_id.clone(),
        }
    }

    pub(crate) fn synthetic_preview(account_key: &AccountKey) -> Self {
        let install_key = [0x42; INSTALL_KEY_BYTES];
        let profile = [0x24; PROFILE_ID_BYTES];
        let account_scope_id =
            account_scope(&install_key, account_key).expect("test account scope must be derivable");
        let storage_epoch = 1;
        let partition_id =
            partition_scope(&install_key, &profile, &account_scope_id.0, storage_epoch)
                .expect("test partition must be derivable");
        let scope = account_scope_id.as_hex();
        let directory = std::env::temp_dir()
            .join("codex-info-account-test")
            .join(&scope)
            .join("epoch-1");
        Self {
            profile_scope_id: hex::encode(profile),
            account_scope_id: scope,
            storage_epoch,
            partition_id: hex::encode(partition_id),
            database_path: directory.join("usage_history.sqlite3"),
            candidate_path: directory.join("usage_history.sqlite3.candidate"),
            writer_lock_path: directory.join("account-writer.lock"),
            metadata_path: std::env::temp_dir()
                .join("codex-info-account-test")
                .join(PROFILE_METADATA_FILE_NAME),
        }
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

#[cfg(unix)]
fn validate_private_file(metadata: &fs::Metadata, max_bytes: u64) -> bool {
    metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == effective_uid()
        && metadata.mode() & 0o777 == 0o600
        && (1..=max_bytes).contains(&metadata.len())
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

#[cfg(unix)]
fn read_private_file_with_post_read<F>(path: &Path, max_bytes: u64, post_read: F) -> Result<Vec<u8>>
where
    F: FnOnce(),
{
    let parent = path
        .parent()
        .ok_or_else(|| AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot))?;
    let root_before = fs::symlink_metadata(parent)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot))?;
    if !parent.is_absolute() || !validate_private_directory(&root_before) {
        return Err(AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot));
    }
    let before = fs::symlink_metadata(path)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::UnsafeFile))?;
    if !validate_private_file(&before, max_bytes) {
        return Err(AccountScopeError::new(AccountScopeErrorKind::UnsafeFile));
    }

    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::UnsafeFile))?;
    let mut file = File::from(fd);
    let opened = file
        .metadata()
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::UnsafeFile))?;
    if !validate_private_file(&opened, max_bytes) {
        return Err(AccountScopeError::new(AccountScopeErrorKind::UnsafeFile));
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
    post_read();
    let after = fs::symlink_metadata(path)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::UnsafeFile))?;
    let root_after = fs::symlink_metadata(parent)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot))?;
    if bytes.len() as u64 != opened.len()
        || bytes.len() as u64 > max_bytes
        || !validate_private_file(&after, max_bytes)
        || !same_file(&before, &opened, &after)
        || !validate_private_directory(&root_after)
        || root_before.dev() != root_after.dev()
        || root_before.ino() != root_after.ino()
    {
        return Err(AccountScopeError::new(AccountScopeErrorKind::UnsafeFile));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn read_private_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    read_private_file_with_post_read(path, max_bytes, || {})
}

#[cfg(not(unix))]
fn read_private_file(_path: &Path, _max_bytes: u64) -> Result<Vec<u8>> {
    Err(AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot))
}

pub(crate) fn read_account_key(codex_root: &Path) -> Result<AccountKey> {
    let codex_root = security::validate_absolute_root(codex_root)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot))?;
    let path = codex_root.join(AUTH_FILE_NAME);
    let bytes = read_private_file(&path, MAX_AUTH_FILE_BYTES)?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    let account_key = AccountKeySeed
        .deserialize(&mut deserializer)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::InvalidAuth))?;
    deserializer
        .end()
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::InvalidAuth))?;
    Ok(account_key)
}

fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N]> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AccountScopeError::new(
            AccountScopeErrorKind::InvalidMetadata,
        ));
    }
    let decoded = hex::decode(value)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::InvalidMetadata))?;
    decoded
        .try_into()
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::InvalidMetadata))
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> Result<[u8; SCOPE_ID_BYTES]> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::InvalidMetadata))?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

fn account_scope(install_key: &[u8], account_key: &AccountKey) -> Result<AccountScopeId> {
    hmac(install_key, &[ACCOUNT_SCOPE_DOMAIN, account_key.as_bytes()]).map(AccountScopeId)
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
    let root = accounts_root(data_root);
    match fs::read_dir(root) {
        Ok(mut entries) => Ok(entries
            .next()
            .transpose()
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?
            .is_some()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(AccountScopeError::new(
            AccountScopeErrorKind::RecoveryRequired,
        )),
    }
}

fn validate_metadata(metadata: &ProfileMetadata) -> Result<()> {
    if metadata.schema_version != PROFILE_SCHEMA || metadata.next_storage_epoch == 0 {
        return Err(AccountScopeError::new(
            AccountScopeErrorKind::InvalidMetadata,
        ));
    }
    decode_fixed::<PROFILE_ID_BYTES>(&metadata.profile_scope_id)?;
    decode_fixed::<INSTALL_KEY_BYTES>(&metadata.install_key)?;
    let mut maximum_epoch = 0u64;
    for (scope, entry) in &metadata.accounts {
        decode_fixed::<SCOPE_ID_BYTES>(scope)?;
        if entry.storage_epoch == 0 {
            return Err(AccountScopeError::new(
                AccountScopeErrorKind::InvalidMetadata,
            ));
        }
        maximum_epoch = maximum_epoch.max(entry.storage_epoch);
    }
    if metadata.next_storage_epoch <= maximum_epoch {
        return Err(AccountScopeError::new(
            AccountScopeErrorKind::InvalidMetadata,
        ));
    }
    Ok(())
}

fn load_metadata(data_root: &Path) -> Result<ProfileMetadata> {
    let bytes = read_private_file(&metadata_path(data_root), MAX_PROFILE_FILE_BYTES)?;
    let metadata = serde_json::from_slice::<ProfileMetadata>(&bytes)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::InvalidMetadata))?;
    validate_metadata(&metadata)?;
    Ok(metadata)
}

fn prepare_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
        if !validate_private_directory(&metadata) {
            return Err(AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot));
        }
    }
    Ok(())
}

fn write_metadata(data_root: &Path, metadata: &ProfileMetadata) -> Result<()> {
    validate_metadata(metadata)?;
    let path = metadata_path(data_root);
    let parent = path
        .parent()
        .ok_or_else(|| AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot))?;
    prepare_private_directory(parent)?;
    prepare_private_directory(&accounts_root(data_root))?;
    if let Ok(existing) = fs::symlink_metadata(&path) {
        if !validate_private_file(&existing, MAX_PROFILE_FILE_BYTES) {
            return Err(AccountScopeError::new(AccountScopeErrorKind::UnsafeFile));
        }
    }

    let bytes = serde_json::to_vec(metadata)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::InvalidMetadata))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_PROFILE_FILE_BYTES {
        return Err(AccountScopeError::new(
            AccountScopeErrorKind::InvalidMetadata,
        ));
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
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
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
        file.sync_all()
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
        drop(file);
        fs::rename(&temporary, &path)
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn initial_metadata() -> Result<ProfileMetadata> {
    let mut profile_scope_id = [0u8; PROFILE_ID_BYTES];
    let mut install_key = [0u8; INSTALL_KEY_BYTES];
    getrandom::fill(&mut profile_scope_id)
        .and_then(|()| getrandom::fill(&mut install_key))
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::Io))?;
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
                return Err(AccountScopeError::new(
                    AccountScopeErrorKind::RecoveryRequired,
                ));
            }
            let metadata = initial_metadata()?;
            write_metadata(data_root, &metadata)?;
            load_metadata(data_root)
        }
        Err(_) => Err(AccountScopeError::new(
            AccountScopeErrorKind::RecoveryRequired,
        )),
    }
}

fn partition_from_metadata(
    data_root: &Path,
    metadata: &ProfileMetadata,
    account_scope_id: &AccountScopeId,
    storage_epoch: u64,
) -> Result<AccountPartition> {
    let profile_scope_raw = decode_fixed::<PROFILE_ID_BYTES>(&metadata.profile_scope_id)?;
    let install_key = decode_fixed::<INSTALL_KEY_BYTES>(&metadata.install_key)?;
    let partition_id = partition_scope(
        &install_key,
        &profile_scope_raw,
        &account_scope_id.0,
        storage_epoch,
    )?;
    let account_scope_hex = account_scope_id.as_hex();
    let epoch_directory = accounts_root(data_root)
        .join(&account_scope_hex)
        .join(format!("epoch-{storage_epoch}"));
    Ok(AccountPartition {
        profile_scope_id: metadata.profile_scope_id.clone(),
        account_scope_id: account_scope_hex,
        storage_epoch,
        partition_id: hex::encode(partition_id),
        database_path: epoch_directory.join("usage_history.sqlite3"),
        candidate_path: epoch_directory.join("usage_history.sqlite3.candidate"),
        writer_lock_path: epoch_directory.join("account-writer.lock"),
        metadata_path: metadata_path(data_root),
    })
}

fn directory_is_empty(path: &Path) -> Result<bool> {
    match fs::read_dir(path) {
        Ok(mut entries) => Ok(entries
            .next()
            .transpose()
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?
            .is_none()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(_) => Err(AccountScopeError::new(
            AccountScopeErrorKind::RecoveryRequired,
        )),
    }
}

#[cfg(unix)]
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
                Err(AccountScopeError::new(
                    AccountScopeErrorKind::RecoveryRequired,
                ))
            } else {
                Ok(())
            };
        }
        Err(_) => {
            return Err(AccountScopeError::new(
                AccountScopeErrorKind::RecoveryRequired,
            ));
        }
    };
    if !validate_private_directory(&root_metadata) {
        return Err(AccountScopeError::new(
            AccountScopeErrorKind::RecoveryRequired,
        ));
    }

    for entry in fs::read_dir(&root)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?
    {
        let entry =
            entry.map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
        let scope = entry
            .file_name()
            .into_string()
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
        if decode_fixed::<SCOPE_ID_BYTES>(&scope).is_err()
            || !metadata.accounts.contains_key(&scope)
            || !validate_private_directory(
                &fs::symlink_metadata(entry.path())
                    .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?,
            )
        {
            return Err(AccountScopeError::new(
                AccountScopeErrorKind::RecoveryRequired,
            ));
        }
    }

    for (scope, registry) in &metadata.accounts {
        let account_directory = root.join(scope);
        let account_metadata = match fs::symlink_metadata(&account_directory) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if registry.state == RegistryState::Initialized {
                    return Err(AccountScopeError::new(
                        AccountScopeErrorKind::RecoveryRequired,
                    ));
                }
                continue;
            }
            Err(_) => {
                return Err(AccountScopeError::new(
                    AccountScopeErrorKind::RecoveryRequired,
                ));
            }
        };
        if !validate_private_directory(&account_metadata) {
            return Err(AccountScopeError::new(
                AccountScopeErrorKind::RecoveryRequired,
            ));
        }
        let epoch_name = format!("epoch-{}", registry.storage_epoch);
        for entry in fs::read_dir(&account_directory)
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?
        {
            let entry = entry
                .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
            if name != epoch_name
                || !validate_private_directory(
                    &fs::symlink_metadata(entry.path()).map_err(|_| {
                        AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired)
                    })?,
                )
            {
                return Err(AccountScopeError::new(
                    AccountScopeErrorKind::RecoveryRequired,
                ));
            }
        }

        let epoch_directory = account_directory.join(&epoch_name);
        let epoch_metadata = match fs::symlink_metadata(&epoch_directory) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if registry.state == RegistryState::Initialized {
                    return Err(AccountScopeError::new(
                        AccountScopeErrorKind::RecoveryRequired,
                    ));
                }
                continue;
            }
            Err(_) => {
                return Err(AccountScopeError::new(
                    AccountScopeErrorKind::RecoveryRequired,
                ));
            }
        };
        if !validate_private_directory(&epoch_metadata) {
            return Err(AccountScopeError::new(
                AccountScopeErrorKind::RecoveryRequired,
            ));
        }
        let mut database_exists = false;
        for entry in fs::read_dir(&epoch_directory)
            .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?
        {
            let entry = entry
                .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
            let allowed = name == "usage_history.sqlite3"
                || name == "usage_history.sqlite3.candidate"
                || name == "account-writer.lock"
                || matches!(
                    name.as_str(),
                    "usage_history.sqlite3.bak.1"
                        | "usage_history.sqlite3.bak.2"
                        | "usage_history.sqlite3.bak.3"
                );
            let artifact = fs::symlink_metadata(entry.path())
                .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
            if !allowed
                || artifact.file_type().is_symlink()
                || !artifact.is_file()
                || artifact.uid() != effective_uid()
                || artifact.mode() & 0o777 != 0o600
            {
                return Err(AccountScopeError::new(
                    AccountScopeErrorKind::RecoveryRequired,
                ));
            }
            database_exists |= name == "usage_history.sqlite3";
        }
        if registry.state == RegistryState::Initialized && !database_exists {
            return Err(AccountScopeError::new(
                AccountScopeErrorKind::RecoveryRequired,
            ));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_registry_artifacts(_data_root: &Path, _metadata: &ProfileMetadata) -> Result<()> {
    Err(AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot))
}

pub(crate) fn resolve_partition(
    data_root: &Path,
    account_key: &AccountKey,
) -> Result<AccountPartition> {
    let data_root = security::validate_absolute_root(data_root)
        .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::UnsafeRoot))?;
    let mut metadata = load_or_initialize_metadata(&data_root)?;
    validate_registry_artifacts(&data_root, &metadata)?;
    let install_key = decode_fixed::<INSTALL_KEY_BYTES>(&metadata.install_key)?;
    let account_scope_id = account_scope(&install_key, account_key)?;
    let scope_hex = account_scope_id.as_hex();

    let (storage_epoch, state) = if let Some(entry) = metadata.accounts.get(&scope_hex) {
        (entry.storage_epoch, entry.state)
    } else {
        let account_directory = accounts_root(&data_root).join(&scope_hex);
        if !directory_is_empty(&account_directory)? || account_directory.exists() {
            return Err(AccountScopeError::new(
                AccountScopeErrorKind::RecoveryRequired,
            ));
        }
        let storage_epoch = metadata.next_storage_epoch;
        metadata.next_storage_epoch = storage_epoch
            .checked_add(1)
            .ok_or_else(|| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
        metadata.accounts.insert(
            scope_hex.clone(),
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
    match state {
        RegistryState::Allocated => {
            let epoch_directory = partition
                .database_path
                .parent()
                .ok_or_else(|| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
            if epoch_directory.exists() {
                let allowed = [
                    partition.database_path.as_path(),
                    partition.candidate_path.as_path(),
                ];
                for entry in fs::read_dir(epoch_directory)
                    .map_err(|_| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?
                {
                    let path = entry
                        .map_err(|_| {
                            AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired)
                        })?
                        .path();
                    if !allowed.contains(&path.as_path()) {
                        return Err(AccountScopeError::new(
                            AccountScopeErrorKind::RecoveryRequired,
                        ));
                    }
                }
            }
        }
        RegistryState::Initialized => {
            if !partition.database_path.is_file() {
                return Err(AccountScopeError::new(
                    AccountScopeErrorKind::RecoveryRequired,
                ));
            }
        }
    }
    Ok(partition)
}

pub(crate) fn mark_partition_initialized(partition: &AccountPartition) -> Result<()> {
    if !partition.database_path.is_file() {
        return Err(AccountScopeError::new(
            AccountScopeErrorKind::RecoveryRequired,
        ));
    }
    let data_root = partition
        .metadata_path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
    let mut metadata = load_metadata(data_root)?;
    if metadata.profile_scope_id != partition.profile_scope_id {
        return Err(AccountScopeError::new(
            AccountScopeErrorKind::RecoveryRequired,
        ));
    }
    let entry = metadata
        .accounts
        .get_mut(&partition.account_scope_id)
        .ok_or_else(|| AccountScopeError::new(AccountScopeErrorKind::RecoveryRequired))?;
    if entry.storage_epoch != partition.storage_epoch {
        return Err(AccountScopeError::new(
            AccountScopeErrorKind::RecoveryRequired,
        ));
    }
    if entry.state != RegistryState::Initialized {
        entry.state = RegistryState::Initialized;
        write_metadata(data_root, &metadata)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(name: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "codex-info-account-scope-{name}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn write_auth(root: &Path, body: &str) {
        let path = root.join(AUTH_FILE_NAME);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        options
            .open(path)
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
    }

    #[test]
    fn account_key_parser_rejects_duplicate_and_unsafe_files() {
        let valid = temp_root("valid-auth");
        write_auth(
            &valid,
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"ignored","account_id":"workspace-A"}}"#,
        );
        assert_eq!(read_account_key(&valid).unwrap().as_bytes(), b"workspace-A");

        let duplicate = temp_root("duplicate-auth");
        write_auth(
            &duplicate,
            r#"{"tokens":{"account_id":"A","account_id":"B"}}"#,
        );
        assert_eq!(
            read_account_key(&duplicate).unwrap_err().kind(),
            AccountScopeErrorKind::InvalidAuth
        );

        let public = temp_root("public-auth");
        write_auth(&public, r#"{"tokens":{"account_id":"A"}}"#);
        #[cfg(unix)]
        fs::set_permissions(
            public.join(AUTH_FILE_NAME),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert_eq!(
            read_account_key(&public).unwrap_err().kind(),
            AccountScopeErrorKind::UnsafeFile
        );
    }

    #[test]
    fn hmac_scopes_are_stable_and_account_separated() {
        let key = [0x11; INSTALL_KEY_BYTES];
        let account_a = AccountKey::new("workspace-A".into()).unwrap();
        let account_b = AccountKey::new("workspace-B".into()).unwrap();
        let first = account_scope(&key, &account_a).unwrap();
        assert_eq!(first, account_scope(&key, &account_a).unwrap());
        assert_eq!(
            first.as_hex(),
            "4e768bd4c1b7b344c7b93d1b4d732c74f9084d5aceca4fe354df32c01fc636b6"
        );
        assert_ne!(first, account_scope(&key, &account_b).unwrap());
        assert_eq!(
            hex::encode(partition_scope(&key, &[0x24; PROFILE_ID_BYTES], &first.0, 1).unwrap()),
            "a1a6c80e33ecc9ce8f06b866f980151f6b6b4f86028c0fe4dffa6ae9da821409"
        );
        assert_ne!(
            first,
            account_scope(&key, &AccountKey::new(" workspace-A".into()).unwrap()).unwrap()
        );
        assert_ne!(
            first,
            account_scope(&key, &AccountKey::new("workspace-a".into()).unwrap()).unwrap()
        );
        assert_eq!(first.as_hex().len(), 64);
        assert!(first
            .as_hex()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }

    #[test]
    fn account_key_contract_rejects_missing_types_controls_limits_and_symlinks() {
        for (name, body) in [
            ("missing-tokens", r#"{}"#.to_owned()),
            ("tokens-type", r#"{"tokens":[]}"#.to_owned()),
            ("account-type", r#"{"tokens":{"account_id":7}}"#.to_owned()),
            ("empty", r#"{"tokens":{"account_id":""}}"#.to_owned()),
            (
                "control",
                r#"{"tokens":{"account_id":"workspace\u0000A"}}"#.to_owned(),
            ),
            (
                "too-long",
                format!(r#"{{"tokens":{{"account_id":"{}"}}}}"#, "x".repeat(513)),
            ),
        ] {
            let root = temp_root(name);
            write_auth(&root, &body);
            assert_eq!(
                read_account_key(&root).unwrap_err().kind(),
                AccountScopeErrorKind::InvalidAuth,
                "fixture {name}"
            );
        }

        let oversized = temp_root("oversized");
        write_auth(&oversized, &"x".repeat(MAX_AUTH_FILE_BYTES as usize + 1));
        assert_eq!(
            read_account_key(&oversized).unwrap_err().kind(),
            AccountScopeErrorKind::UnsafeFile
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = temp_root("symlink");
            let target = root.join("real-auth.json");
            let mut options = OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            options
                .open(&target)
                .unwrap()
                .write_all(br#"{"tokens":{"account_id":"workspace-A"}}"#)
                .unwrap();
            symlink(&target, root.join(AUTH_FILE_NAME)).unwrap();
            assert_eq!(
                read_account_key(&root).unwrap_err().kind(),
                AccountScopeErrorKind::UnsafeFile
            );

            let unsafe_root = temp_root("unsafe-root");
            write_auth(&unsafe_root, r#"{"tokens":{"account_id":"workspace-A"}}"#);
            fs::set_permissions(&unsafe_root, fs::Permissions::from_mode(0o750)).unwrap();
            assert_eq!(
                read_account_key(&unsafe_root).unwrap_err().kind(),
                AccountScopeErrorKind::UnsafeRoot
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn account_authority_rejects_a_same_size_mid_read_replacement() {
        let root = temp_root("mid-read-replacement");
        let original = root.join(AUTH_FILE_NAME);
        let replacement = root.join("replacement.json");
        write_auth(&root, r#"{"tokens":{"account_id":"workspace-A"}}"#);
        let original_len = fs::metadata(&original).unwrap().len();
        let replacement_body = r#"{"tokens":{"account_id":"workspace-B"}}"#;
        assert_eq!(replacement_body.len() as u64, original_len);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        options
            .open(&replacement)
            .unwrap()
            .write_all(replacement_body.as_bytes())
            .unwrap();

        let error = read_private_file_with_post_read(&original, MAX_AUTH_FILE_BYTES, || {
            fs::rename(&replacement, &original).unwrap();
        })
        .unwrap_err();
        assert_eq!(error.kind(), AccountScopeErrorKind::UnsafeFile);
    }

    #[test]
    fn profile_registry_reuses_epoch_and_separates_physical_paths() {
        let data_root = temp_root("registry");
        let account_a = AccountKey::new("workspace-A".into()).unwrap();
        let account_b = AccountKey::new("workspace-B".into()).unwrap();
        let first_a = resolve_partition(&data_root, &account_a).unwrap();
        let second_a = resolve_partition(&data_root, &account_a).unwrap();
        let first_b = resolve_partition(&data_root, &account_b).unwrap();

        assert_eq!(first_a, second_a);
        assert_ne!(first_a.account_scope_id, first_b.account_scope_id);
        assert_ne!(first_a.storage_epoch, first_b.storage_epoch);
        assert_ne!(first_a.database_path, first_b.database_path);
        assert_ne!(first_a.writer_lock_path, first_b.writer_lock_path);
        assert!(!first_a.database_path.exists());

        let metadata_bytes = fs::read(metadata_path(&data_root)).unwrap();
        let text = String::from_utf8(metadata_bytes).unwrap();
        assert!(!text.contains("workspace-A"));
        assert!(!text.contains("workspace-B"));
    }

    #[test]
    fn raw_identity_canaries_are_absent_from_partition_paths_metadata_and_candidate_db() {
        const RAW_ACCOUNT: &str = "raw-account-key-CANARY-129";
        const RAW_EMAIL: &str = "raw-email-CANARY-129@example.test";
        const RAW_TOKEN: &str = "raw-auth-token-CANARY-129";

        fn assert_tree_excludes(root: &Path, needles: &[&str]) {
            let mut pending = vec![root.to_path_buf()];
            while let Some(path) = pending.pop() {
                let rendered = path.to_string_lossy();
                for needle in needles {
                    assert!(!rendered.contains(needle), "secret canary entered a path");
                }
                let metadata = fs::symlink_metadata(&path).unwrap();
                if metadata.is_dir() {
                    pending.extend(
                        fs::read_dir(path)
                            .unwrap()
                            .map(|entry| entry.unwrap().path()),
                    );
                } else if metadata.is_file() {
                    let bytes = fs::read(path).unwrap();
                    for needle in needles {
                        assert!(
                            !bytes
                                .windows(needle.len())
                                .any(|window| window == needle.as_bytes()),
                            "secret canary entered a durable artifact"
                        );
                    }
                }
            }
        }

        let auth_root = temp_root("identity-canary-auth");
        write_auth(
            &auth_root,
            &format!(
                r#"{{"email":"{RAW_EMAIL}","tokens":{{"access_token":"{RAW_TOKEN}","account_id":"{RAW_ACCOUNT}"}}}}"#
            ),
        );
        let account = read_account_key(&auth_root).unwrap();
        let data_root = temp_root("identity-canary-scan");
        let partition = resolve_partition(&data_root, &account).unwrap();
        let candidate = codex_info::usage_store::UsageStore::create_partitioned(
            &partition.candidate_path,
            &partition.storage_identity(),
        )
        .unwrap();
        drop(candidate);

        assert_eq!(format!("{account:?}"), "AccountKey([redacted])");
        assert_tree_excludes(&data_root, &[RAW_ACCOUNT, RAW_EMAIL, RAW_TOKEN]);
    }

    #[test]
    fn missing_registry_with_account_artifacts_is_recovery_required() {
        let data_root = temp_root("missing-registry");
        let orphan = accounts_root(&data_root).join("orphan");
        fs::create_dir_all(&orphan).unwrap();
        let account = AccountKey::new("workspace-A".into()).unwrap();
        assert_eq!(
            resolve_partition(&data_root, &account).unwrap_err().kind(),
            AccountScopeErrorKind::RecoveryRequired
        );
    }

    #[cfg(unix)]
    #[test]
    fn registry_rejects_orphans_unknown_artifacts_and_missing_initialized_database() {
        let account = AccountKey::new("workspace-A".into()).unwrap();

        let orphan_root = temp_root("orphan-after-registry");
        let partition = resolve_partition(&orphan_root, &account).unwrap();
        let orphan = accounts_root(&orphan_root).join("ab".repeat(SCOPE_ID_BYTES));
        fs::create_dir(&orphan).unwrap();
        fs::set_permissions(&orphan, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            resolve_partition(&orphan_root, &account)
                .unwrap_err()
                .kind(),
            AccountScopeErrorKind::RecoveryRequired
        );
        assert!(!partition.database_path.exists());

        let unknown_root = temp_root("unknown-allocated-artifact");
        let partition = resolve_partition(&unknown_root, &account).unwrap();
        let epoch = partition.database_path.parent().unwrap();
        fs::create_dir_all(epoch).unwrap();
        fs::set_permissions(epoch.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(epoch, fs::Permissions::from_mode(0o700)).unwrap();
        let unknown = epoch.join("unknown");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(unknown)
            .unwrap();
        assert_eq!(
            resolve_partition(&unknown_root, &account)
                .unwrap_err()
                .kind(),
            AccountScopeErrorKind::RecoveryRequired
        );

        let missing_root = temp_root("missing-initialized-database");
        let partition = resolve_partition(&missing_root, &account).unwrap();
        let epoch = partition.database_path.parent().unwrap();
        fs::create_dir_all(epoch).unwrap();
        fs::set_permissions(epoch.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(epoch, fs::Permissions::from_mode(0o700)).unwrap();
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&partition.database_path)
            .unwrap();
        mark_partition_initialized(&partition).unwrap();
        fs::remove_file(&partition.database_path).unwrap();
        assert_eq!(
            resolve_partition(&missing_root, &account)
                .unwrap_err()
                .kind(),
            AccountScopeErrorKind::RecoveryRequired
        );
    }

    #[test]
    fn legacy_database_backups_and_sessions_remain_byte_identical() {
        let data_root = temp_root("legacy-sentinel");
        let history = data_root.join("history");
        let sessions = data_root.join("sessions");
        fs::create_dir_all(&history).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        let legacy = history.join("usage_history.sqlite3");
        let backup = history.join("usage_history.sqlite3.bak.1");
        let session = sessions.join("legacy.jsonl");
        fs::write(&legacy, b"legacy-database-sentinel").unwrap();
        fs::write(&backup, b"legacy-backup-sentinel").unwrap();
        fs::write(&session, b"legacy-session-sentinel\n").unwrap();
        let snapshot = |path: &Path| {
            #[cfg(unix)]
            let identity = fs::metadata(path).unwrap().ino();
            #[cfg(not(unix))]
            let identity = 0_u64;
            (identity, fs::read(path).unwrap())
        };
        let before = [&legacy, &backup, &session].map(|path| snapshot(path));

        let account = AccountKey::new("workspace-A".into()).unwrap();
        let partition = resolve_partition(&data_root, &account).unwrap();
        assert!(!partition.database_path.exists());

        let after = [&legacy, &backup, &session].map(|path| snapshot(path));
        assert_eq!(before, after);
        let metadata = String::from_utf8(fs::read(metadata_path(&data_root)).unwrap()).unwrap();
        assert!(!metadata.contains("workspace-A"));
    }
}

//! Durable receiver journal. An acknowledged receipt is never ahead of fsync.
use super::{
    sync::{Data, Receiver},
    WireError,
};
use std::path::{Path, PathBuf};
#[derive(Debug)]
pub enum CacheError {
    Io(std::io::Error),
    Invalid,
    Missing,
    Wire(WireError),
}
impl From<std::io::Error> for CacheError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<WireError> for CacheError {
    fn from(e: WireError) -> Self {
        Self::Wire(e)
    }
}
pub struct Cache {
    pub receiver: Receiver,
    pub consumer: Option<String>,
    pub seq: u64,
    dir: PathBuf,
    pub pending_cursors: BTreeMap<String, String>,
}
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, Write};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    consumer: Option<String>,
    seq: u64,
    root: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalRecord {
    #[serde(default)]
    target: Option<String>,
    view_keys: BTreeMap<String, serde_json::Value>,
    data: Option<Data>,
    drop_partials: Option<String>,
}
fn file(path: &Path, create: bool) -> Result<std::fs::File, std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)?.sync_all()?;
    }
    Ok(())
}
impl Cache {
    pub fn open(path: &Path) -> Result<Self, CacheError> {
        let anchor = path.with_extension("identity");
        if !path.exists() && anchor.exists() {
            return Err(CacheError::Missing);
        }
        let fresh = match std::fs::create_dir(path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(e.into()),
        };
        if fresh {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
            file(&path.join("receipts"), true)?.sync_all()?;
            let cache = Self {
                receiver: Receiver::default(),
                consumer: None,
                seq: 0,
                dir: path.into(),
                pending_cursors: BTreeMap::new(),
            };
            cache.save_anchor()?;
            cache.save_profile()?;
            sync_directory(path)?;
            return Ok(cache);
        }
        if path.symlink_metadata()?.file_type().is_symlink() {
            return Err(CacheError::Invalid);
        }
        let mut profile: Profile =
            serde_json::from_slice(&std::fs::read(path.join("profile")).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    CacheError::Missing
                } else {
                    e.into()
                }
            })?)
            .map_err(|_| CacheError::Invalid)?;
        if profile
            .consumer
            .as_deref()
            .is_some_and(|id| !super::request::opaque(id))
        {
            return Err(CacheError::Invalid);
        }
        let mut journal = file(&path.join("receipts"), false).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CacheError::Missing
            } else {
                e.into()
            }
        })?;
        let mut receiver = Receiver::default();
        let mut pending_cursors = BTreeMap::new();
        let mut qualified = profile.root == super::sync::hex(&receiver.root()?);
        let mut committed = 0;
        loop {
            let mut length = [0; 8];
            match journal.read_exact(&mut length) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
            let length = u64::from_be_bytes(length);
            if length > 33_570_816 {
                return Err(CacheError::Invalid);
            }
            let mut expected = [0; 32];
            let mut bytes = vec![0; length as usize];
            let mut seal = [0];
            if journal
                .read_exact(&mut expected)
                .and_then(|_| journal.read_exact(&mut bytes))
                .and_then(|_| journal.read_exact(&mut seal))
                .is_err()
            {
                break;
            }
            if seal != [0xa5] || super::hash(&bytes) != expected {
                return Err(CacheError::Invalid);
            }
            let record: JournalRecord =
                serde_json::from_slice(&bytes).map_err(|_| CacheError::Invalid)?;
            if let Some(key) = record.target.as_ref() {
                if let Some(cursor) = record
                    .data
                    .as_ref()
                    .and_then(|data| data.next_cursor.as_ref())
                {
                    pending_cursors.insert(key.clone(), cursor.clone());
                } else {
                    pending_cursors.remove(key);
                }
            }
            receiver.view_keys.extend(record.view_keys);
            if let Some(data) = record.data {
                receiver.apply(&data)?;
            }
            if let Some(view) = record.drop_partials {
                receiver.discard_partials(&view);
            }
            qualified |= profile.root == super::sync::hex(&receiver.root()?);
            committed = journal.stream_position()?;
        }
        if !qualified || profile.consumer.is_none() && !receiver.definitions.is_empty() {
            return Err(CacheError::Invalid);
        }
        // An unsealed append was never acknowledged. Preserve every sealed record;
        // only the incomplete transaction tail can be removed.
        if journal.metadata()?.len() != committed {
            journal.set_len(committed)?;
            journal.sync_all()?;
        }
        if anchor.exists() {
            let identity: Option<String> = serde_json::from_slice(&std::fs::read(&anchor)?)
                .map_err(|_| CacheError::Invalid)?;
            if let Some(identity) = identity {
                if profile.consumer.is_none() && receiver.definitions.is_empty() {
                    profile.consumer = Some(identity);
                } else if profile.consumer.as_ref() != Some(&identity) {
                    return Err(CacheError::Invalid);
                }
            }
        }
        let cache = Self {
            receiver,
            consumer: profile.consumer,
            seq: profile.seq,
            dir: path.into(),
            pending_cursors,
        };
        cache.save_anchor()?;
        cache.save_profile()?;
        Ok(cache)
    }
    fn save_anchor(&self) -> Result<(), CacheError> {
        let anchor = self.dir.with_extension("identity");
        let temporary = self.dir.with_extension("identity-next");
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        output.write_all(&serde_json::to_vec(&self.consumer).map_err(|_| CacheError::Invalid)?)?;
        output.sync_all()?;
        std::fs::rename(temporary, anchor)?;
        if let Some(parent) = self.dir.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }
    fn save_profile(&self) -> Result<(), CacheError> {
        let profile = Profile {
            consumer: self.consumer.clone(),
            seq: self.seq,
            root: super::sync::hex(&self.receiver.root()?),
        };
        let temporary = self.dir.join("profile-next");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(&temporary)?;
        output.write_all(&serde_json::to_vec(&profile).map_err(|_| CacheError::Invalid)?)?;
        output.sync_all()?;
        std::fs::rename(temporary, self.dir.join("profile"))?;
        sync_directory(&self.dir)?;
        Ok(())
    }
    pub fn issue(&mut self, id: &str) -> Result<(), CacheError> {
        if !super::request::opaque(id) || self.consumer.as_deref().is_some_and(|old| old != id) {
            return Err(CacheError::Invalid);
        }
        self.consumer = Some(id.into());
        self.save_anchor()?;
        self.save_profile()
    }
    pub fn commit(
        &mut self,
        target: &super::request::Target,
        data: &Data,
    ) -> Result<(), CacheError> {
        if self.consumer.is_none() {
            return Err(CacheError::Invalid);
        }
        let mut next = self.receiver.clone();
        next.register_target(target, data)?;
        next.apply(data)?;
        let view_keys = data
            .changes
            .iter()
            .filter_map(|change| {
                next.view_keys
                    .get(&change.view_id)
                    .map(|key| (change.view_id.clone(), key.clone()))
            })
            .collect();
        let bytes = serde_json::to_vec(&JournalRecord {
            target: Some(target.transfer_key()?),
            view_keys,
            data: Some(data.clone()),
            drop_partials: None,
        })
        .map_err(|_| CacheError::Invalid)?;
        let mut journal = file(&self.dir.join("receipts"), false)?;
        journal.seek(std::io::SeekFrom::End(0))?;
        journal.write_all(&(bytes.len() as u64).to_be_bytes())?;
        journal.write_all(&super::hash(&bytes))?;
        journal.write_all(&bytes)?;
        journal.write_all(&[0xa5])?;
        journal.sync_all()?;
        self.receiver = next;
        let key = target.transfer_key()?;
        if let Some(cursor) = data.next_cursor.as_ref() {
            self.pending_cursors.insert(key, cursor.clone());
        } else {
            self.pending_cursors.remove(&key);
        }
        self.save_profile()
    }
    pub fn discard_partials(&mut self, view_id: &str) -> Result<(), CacheError> {
        let record = JournalRecord {
            target: None,
            view_keys: BTreeMap::new(),
            data: None,
            drop_partials: Some(view_id.into()),
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| CacheError::Invalid)?;
        let mut journal = file(&self.dir.join("receipts"), false)?;
        journal.seek(std::io::SeekFrom::End(0))?;
        journal.write_all(&(bytes.len() as u64).to_be_bytes())?;
        journal.write_all(&super::hash(&bytes))?;
        journal.write_all(&bytes)?;
        journal.write_all(&[0xa5])?;
        journal.sync_all()?;
        self.receiver.discard_partials(view_id);
        self.save_profile()
    }
    pub fn clear_cursor(&mut self, target: &super::request::Target) -> Result<(), CacheError> {
        let key = target.transfer_key()?;
        let record = JournalRecord {
            target: Some(key.clone()),
            view_keys: BTreeMap::new(),
            data: None,
            drop_partials: None,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| CacheError::Invalid)?;
        let mut journal = file(&self.dir.join("receipts"), false)?;
        journal.seek(std::io::SeekFrom::End(0))?;
        journal.write_all(&(bytes.len() as u64).to_be_bytes())?;
        journal.write_all(&super::hash(&bytes))?;
        journal.write_all(&bytes)?;
        journal.write_all(&[0xa5])?;
        journal.sync_all()?;
        self.pending_cursors.remove(&key);
        self.save_profile()
    }
    pub fn next_seq(&mut self) -> Result<u64, CacheError> {
        self.seq = self.seq.checked_add(1).ok_or(CacheError::Invalid)?;
        self.save_profile()?;
        Ok(self.seq)
    }
}
impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "receiver cache I/O: {e}"),
            Self::Missing => f.write_str("cache_missing"),
            Self::Invalid => f.write_str("cache_invalid"),
            Self::Wire(e) => write!(f, "receiver cache: {e}"),
        }
    }
}
impl std::error::Error for CacheError {}
#[cfg(test)]
mod tests {
    use super::super::{
        request::Target,
        sync::{Catalog, Change, Ref, Set},
    };
    use super::*;
    use serde_json::json;
    #[test]
    fn committed_receiver_and_identity_survive_restart_and_corruption_is_not_bootstrap() {
        let dir = std::env::temp_dir().join(format!(
            "codex-info-receipt-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut cache = Cache::open(&dir).unwrap();
        assert!(cache.consumer.is_none());
        cache.issue("profile-1").unwrap();
        let mut catalog = Catalog::default();
        let id = catalog
            .intern(
                "main",
                &json!({"acquisition_state":"auth_required","quota":null}),
            )
            .unwrap();
        let data = Data {
            result: "data".into(),
            delivery_id: "delivery-1".into(),
            definitions: catalog.transfer(id, &Default::default()).unwrap(),
            patches: vec![],
            changes: vec![Change {
                view_id: "view-1".into(),
                account: None,
                base_image: None,
                target_image: "image-1".into(),
                sets: vec![Set {
                    key: json!(["main"]),
                    value: Ref { r#ref: id },
                }],
                removes: vec![],
            }],
            fragments: vec![],
            completed_views: vec!["view-1".into()],
            next_cursor: None,
        };
        let target = Target::parse("/state?parts=main").unwrap().unwrap();
        cache.commit(&target, &data).unwrap();
        let root = cache.receiver.root().unwrap();
        assert_eq!(cache.next_seq().unwrap(), 1);
        drop(cache);
        let restored = Cache::open(&dir).unwrap();
        assert_eq!(restored.consumer.as_deref(), Some("profile-1"));
        assert_eq!(restored.seq, 1);
        assert_eq!(restored.receiver.root().unwrap(), root);
        assert_eq!(
            restored.receiver.resolve(id).unwrap()["acquisition_state"],
            "auth_required"
        );
        drop(restored);
        std::fs::write(dir.join("receipts"), b"broken").unwrap();
        assert!(matches!(
            Cache::open(&dir),
            Err(CacheError::Invalid) | Err(CacheError::Missing)
        ));
        std::fs::remove_file(dir.join("receipts")).unwrap();
        std::fs::remove_file(dir.join("profile")).unwrap();
        std::fs::remove_file(dir.with_extension("identity")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn committed_large_receiver_small_update_does_not_walk_retained_definitions() {
        let dir = std::env::temp_dir().join(format!(
            "codex-690-cache-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut cache = Cache::open(&dir).unwrap();
        cache.issue("large-cache-consumer").unwrap();
        let mut catalog = Catalog::default();
        let mut definitions = Vec::new();
        for i in 0..10000u64 {
            let id = catalog
                .intern("text", &json!(format!("retained-{i}")))
                .unwrap();
            definitions.push(catalog.definitions[&id].clone());
        }
        let root = catalog
            .intern("main", &json!({"observed_at":100,"models":[],"quota":null}))
            .unwrap();
        let known = definitions.iter().map(|d| d.id).collect();
        definitions.extend(catalog.transfer(root, &known).unwrap());
        let target = Target::parse("/state?parts=main").unwrap().unwrap();
        let first = Data {
            result: "data".into(),
            delivery_id: "large-1".into(),
            definitions,
            patches: vec![],
            changes: vec![Change {
                view_id: "main-view".into(),
                account: None,
                base_image: None,
                target_image: "first".into(),
                sets: vec![Set {
                    key: json!(["main"]),
                    value: Ref { r#ref: root },
                }],
                removes: vec![],
            }],
            fragments: vec![],
            completed_views: vec!["main-view".into()],
            next_cursor: None,
        };
        cache.commit(&target, &first).unwrap();
        let known = cache.receiver.definitions.keys().copied().collect();
        let next = catalog
            .intern("main", &json!({"observed_at":101,"models":[],"quota":null}))
            .unwrap();
        let definitions = catalog.transfer(next, &known).unwrap();
        assert_eq!(definitions.len(), 2);
        let before = cache.receiver.definitions.full_walks();
        let old = cache.receiver.root().unwrap();
        let small = Data {
            result: "data".into(),
            delivery_id: "large-2".into(),
            definitions,
            patches: vec![],
            changes: vec![Change {
                view_id: "main-view".into(),
                account: None,
                base_image: Some("first".into()),
                target_image: "second".into(),
                sets: vec![Set {
                    key: json!(["main"]),
                    value: Ref { r#ref: next },
                }],
                removes: vec![],
            }],
            fragments: vec![],
            completed_views: vec!["main-view".into()],
            next_cursor: None,
        };
        cache.commit(&target, &small).unwrap();
        assert_eq!(
            cache.receiver.definitions.full_walks(),
            before,
            "commit must not enumerate retained definition cache"
        );
        assert_ne!(cache.receiver.root().unwrap(), old);
        assert_eq!(cache.receiver.resolve(next).unwrap()["observed_at"], 101);
        for _ in 0..1000 {
            cache.receiver.root().unwrap();
        }
        assert_eq!(cache.receiver.definitions.full_walks(), before);
        drop(cache);
        std::fs::remove_file(dir.join("receipts")).unwrap();
        std::fs::remove_file(dir.join("profile")).unwrap();
        std::fs::remove_file(dir.with_extension("identity")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn missing_entire_cache_directory_is_recovery_not_a_new_consumer() {
        let dir = std::env::temp_dir().join(format!(
            "codex-690-lost-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let held = dir.with_extension("held");
        let mut cache = Cache::open(&dir).unwrap();
        cache.issue("original-consumer").unwrap();
        drop(cache);
        std::fs::rename(&dir, &held).unwrap();
        assert!(matches!(Cache::open(&dir), Err(CacheError::Missing)));
        assert!(!dir.exists());
        std::fs::remove_file(held.join("profile")).unwrap();
        std::fs::remove_file(held.join("receipts")).unwrap();
        std::fs::remove_dir(held).unwrap();
        std::fs::remove_file(dir.with_extension("identity")).unwrap();
    }
}

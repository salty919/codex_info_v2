//! Durable raw response prefix, before JSON qualification. No received byte replay.
use super::cache::CacheError;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub target: String,
    pub delivery: String,
    pub consumer: String,
    pub length: u64,
    pub hash: String,
}
pub struct Incoming {
    pub header: Header,
    pub offset: u64,
    pub prefix: String,
    root: PathBuf,
}
fn key(target: &str) -> String {
    super::sync::hex(&super::hash(target.as_bytes()))
}
impl Incoming {
    pub fn targets(directory: &Path) -> Result<Vec<String>, CacheError> {
        let mut targets = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("incoming-") && name.ends_with(".json") {
                let header: Header = serde_json::from_slice(&std::fs::read(path)?)
                    .map_err(|_| CacheError::Invalid)?;
                targets.push(header.target);
            }
        }
        Ok(targets)
    }
    pub fn receipts(directory: &Path) -> Result<Vec<serde_json::Value>, CacheError> {
        let mut receipts = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("incoming-") || !name.ends_with(".json") {
                continue;
            }
            let header: Header =
                serde_json::from_slice(&std::fs::read(&path)?).map_err(|_| CacheError::Invalid)?;
            if let Some(incoming) = Self::load(directory, &header.target)? {
                receipts.push(serde_json::json!({"delivery_id":header.delivery,"offset":incoming.offset,"prefix_hash":incoming.prefix}));
            }
        }
        Ok(receipts)
    }

    pub fn load(directory: &Path, target: &str) -> Result<Option<Self>, CacheError> {
        let root = directory.join(format!("incoming-{}", key(target)));
        let meta = root.with_extension("json");
        let bytes = match std::fs::read(&meta) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let header: Header = serde_json::from_slice(&bytes).map_err(|_| CacheError::Invalid)?;
        if header.target != target || header.length > 33_554_432 {
            return Err(CacheError::Invalid);
        }
        let bytes = std::fs::read(&root)?;
        if bytes.len() as u64 > header.length {
            return Err(CacheError::Invalid);
        }
        Ok(Some(Self {
            header,
            offset: bytes.len() as u64,
            prefix: super::sync::hex(&super::hash(bytes)),
            root,
        }))
    }
    pub fn begin(directory: &Path, header: Header) -> Result<Self, CacheError> {
        if !super::request::opaque(&header.delivery)
            || !super::request::opaque(&header.consumer)
            || super::sync::parse_digest(&header.hash).is_err()
            || header.length > 33_554_432
        {
            return Err(CacheError::Invalid);
        }
        if let Some(existing) = Self::load(directory, &header.target)? {
            if existing.header.delivery == header.delivery && existing.header.hash == header.hash {
                return Ok(existing);
            }
            return Err(CacheError::Invalid);
        }
        let root = directory.join(format!("incoming-{}", key(&header.target)));
        let mut raw = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&root)?;
        raw.flush()?;
        raw.sync_all()?;
        let mut meta = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.with_extension("json"))?;
        meta.write_all(&serde_json::to_vec(&header).map_err(|_| CacheError::Invalid)?)?;
        meta.sync_all()?;
        std::fs::File::open(directory)?.sync_all()?;
        Ok(Self {
            header,
            offset: 0,
            prefix: super::sync::hex(&super::hash([])),
            root,
        })
    }
    pub fn append(&mut self, bytes: &[u8]) -> Result<(), CacheError> {
        if self
            .offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > self.header.length)
        {
            return Err(CacheError::Invalid);
        }
        let mut output = std::fs::OpenOptions::new().append(true).open(&self.root)?;
        if output.metadata()?.len() != self.offset {
            return Err(CacheError::Invalid);
        }
        output.write_all(bytes)?;
        output.sync_all()?;
        self.offset += bytes.len() as u64;
        Ok(())
    }
    pub fn bytes(&self) -> Result<Vec<u8>, CacheError> {
        let mut bytes = Vec::new();
        std::fs::File::open(&self.root)?.read_to_end(&mut bytes)?;
        if bytes.len() as u64 != self.header.length
            || super::sync::hex(&super::hash(&bytes)) != self.header.hash
        {
            return Err(CacheError::Invalid);
        }
        Ok(bytes)
    }
    pub fn finish(self) -> Result<(), CacheError> {
        std::fs::remove_file(self.root.with_extension("json"))?;
        std::fs::remove_file(&self.root)?;
        if let Some(parent) = self.root.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

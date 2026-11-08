//! Immutable issued response pages survive transport and producer restarts.
use super::*;
use std::{
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Meta {
    pub consumer: String,
    pub target: String,
    pub delivery: String,
    pub length: u64,
    pub hash: String,
    #[serde(default)]
    pub bases: Vec<wire::Definition>,
    #[serde(default)]
    pub offered: Vec<String>,
}
pub(super) struct Outbox {
    root: PathBuf,
}
impl Outbox {
    pub fn open() -> std::io::Result<Self> {
        #[cfg(test)]
        let root = std::env::temp_dir().join(format!(
            "codex-api-690-outbox-{}",
            opaque_id().map_err(|_| std::io::Error::other("entropy"))?
        ));
        #[cfg(not(test))]
        let root = std::env::var_os("CODEX_INFO_WIRE_OUTBOX_PATH")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_STATE_HOME")
                    .map(|p| PathBuf::from(p).join("codex-info/rest-outbox"))
            })
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|p| PathBuf::from(p).join(".local/state/codex-info/rest-outbox"))
            })
            .ok_or_else(|| std::io::Error::other("outbox path unavailable"))?;
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    fn paths(&self, consumer: &str, delivery: &str) -> Result<(PathBuf, PathBuf), ()> {
        if !wire::request::opaque(consumer) || !wire::request::opaque(delivery) {
            return Err(());
        }
        let key = hex(&Sha256::digest(format!("{consumer}/{delivery}")));
        let path = self.root.join(key);
        Ok((path.with_extension("json"), path.with_extension("body")))
    }
    pub fn save(
        &self,
        consumer: &str,
        target: &str,
        body: &[u8],
        catalog: &wire::sync::Catalog,
    ) -> Result<Option<Meta>, ()> {
        let data: Value = serde_json::from_slice(body).map_err(|_| ())?;
        if data["result"] != "data" {
            return Ok(None);
        }
        let delivery = data["delivery_id"].as_str().ok_or(())?;
        let (meta_path, body_path) = self.paths(consumer, delivery)?;
        let typed: wire::sync::Data = serde_json::from_slice(body).map_err(|_| ())?;
        let base_ids = typed
            .patches
            .iter()
            .map(|patch| patch.base.r#ref)
            .collect::<BTreeSet<_>>();
        let bases = base_ids
            .into_iter()
            .map(|id| catalog.definitions.get(&id).cloned().ok_or(()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut offered = BTreeSet::new();
        for id in typed
            .definitions
            .iter()
            .map(|d| d.id)
            .chain(typed.patches.iter().map(|p| p.id))
        {
            if let Some(hash) = catalog.digests.get(&id) {
                offered.insert(hex(hash));
            }
        }
        for fragment in &typed.fragments {
            if let Some(descriptor) = fragment.descriptor.as_ref() {
                offered.insert(descriptor.value_hash.clone());
            } else if let Some(hash) = catalog.digests.get(&fragment.id) {
                offered.insert(hex(hash));
            }
        }
        let meta = Meta {
            consumer: consumer.into(),
            target: target.into(),
            delivery: delivery.into(),
            length: body.len() as u64,
            hash: hex(&Sha256::digest(body)),
            bases,
            offered: offered.into_iter().collect(),
        };
        Self::write(&body_path, body)?;
        Self::write(&meta_path, &serde_json::to_vec(&meta).map_err(|_| ())?)?;
        std::fs::File::open(&self.root)
            .and_then(|file| file.sync_all())
            .map_err(|_| ())?;
        Ok(Some(meta))
    }
    fn write(path: &Path, bytes: &[u8]) -> Result<(), ()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| ())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| ())
    }
    pub fn resume(
        &self,
        consumer: &str,
        target: &str,
        delivery: &str,
        offset: u64,
        prefix: &str,
    ) -> Result<(Meta, Vec<u8>), ()> {
        let (meta_path, body_path) = self.paths(consumer, delivery)?;
        let meta: Meta =
            serde_json::from_slice(&std::fs::read(meta_path).map_err(|_| ())?).map_err(|_| ())?;
        if meta.consumer != consumer
            || meta.target != target
            || meta.delivery != delivery
            || meta.length > MAX_BODY_BYTES as u64
            || offset > meta.length
        {
            return Err(());
        }
        let body = std::fs::read(body_path).map_err(|_| ())?;
        if body.len() as u64 != meta.length
            || hex(&Sha256::digest(&body)) != meta.hash
            || hex(&Sha256::digest(&body[..offset as usize])) != prefix
        {
            return Err(());
        }
        Ok((meta, body))
    }
    pub fn recover(
        &self,
        consumer: &str,
        delivery: &str,
        offset: u64,
        prefix: &str,
    ) -> Result<(Meta, wire::sync::Data), ()> {
        let (meta_path, _) = self.paths(consumer, delivery)?;
        let meta: Meta =
            serde_json::from_slice(&std::fs::read(meta_path).map_err(|_| ())?).map_err(|_| ())?;
        let (meta, body) = self.resume(consumer, &meta.target, delivery, offset, prefix)?;
        Ok((meta, serde_json::from_slice(&body).map_err(|_| ())?))
    }
    pub fn retire(&self, consumer: &str, delivery: &str) {
        if let Ok((meta, body)) = self.paths(consumer, delivery) {
            let _ = std::fs::remove_file(meta);
            let _ = std::fs::remove_file(body);
        }
    }
}

//! Content-addressed definitions and atomic UI view receipt (Refs #690).
use super::persistent::Map;
use super::{Definition, Digest, WireError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Catalog {
    pub definitions: BTreeMap<u64, Definition>,
    pub digests: BTreeMap<u64, Digest>,
    by_content: BTreeMap<String, u64>,
    pub children: BTreeMap<u64, Vec<u64>>,
    next: u64,
}
impl Catalog {
    pub fn with_first_id(seed: u64) -> Self {
        Self {
            next: seed,
            ..Self::default()
        }
    }
    pub fn intern(&mut self, kind: &str, value: &Value) -> Result<u64, WireError> {
        self.intern_typed(Some(kind), value, 0)
    }
    fn intern_typed(
        &mut self,
        kind: Option<&str>,
        value: &Value,
        depth: usize,
    ) -> Result<u64, WireError> {
        if depth > 128 {
            return Err(WireError("definition depth exceeded"));
        }
        if super::reference(value)?.is_some() {
            return Err(WireError("source contains reference"));
        }
        let owned_kind = if value.is_null() {
            "null"
        } else if value.is_array() {
            "list"
        } else {
            kind.unwrap_or_else(|| match value {
                Value::Bool(_) => "bool",
                Value::String(_) => "text",
                Value::Number(n) if n.is_u64() => "u64",
                Value::Number(n) if n.is_i64() => "i64",
                Value::Number(_) => "f64",
                _ => "",
            })
        };
        if !super::KINDS.contains(&owned_kind) {
            return Err(WireError("unknown kind"));
        }
        let digest = super::value_digest(kind, value, &BTreeMap::new())?;
        let content_key = format!("{}:{}", owned_kind, hex(&digest));
        if let Some(&id) = self.by_content.get(&content_key) {
            return Ok(id);
        }
        let encoded = match value {
            Value::Object(object) => {
                let mut output = serde_json::Map::new();
                for (name, value) in object {
                    let id = self.intern_typed(super::field_kind(kind, name), value, depth + 1)?;
                    output.insert(name.clone(), serde_json::json!({"ref":id}));
                }
                Value::Object(output)
            }
            Value::Array(values) => {
                let element_kind = kind.and_then(|k| k.strip_suffix("[]"));
                let mut output = Vec::with_capacity(values.len());
                for value in values {
                    let id = self.intern_typed(element_kind, value, depth + 1)?;
                    output.push(serde_json::json!({"ref":id}));
                }
                Value::Array(output)
            }
            _ => value.clone(),
        };
        self.next = self
            .next
            .checked_add(1)
            .ok_or(WireError("definition id exhausted"))?;
        let id = self.next;
        let mut children = Vec::new();
        references(&encoded, &mut children)?;
        self.children.insert(id, children);
        self.definitions.insert(
            id,
            Definition {
                id,
                kind: owned_kind.into(),
                value: encoded,
            },
        );
        self.digests.insert(id, digest);
        self.by_content.insert(content_key, id);
        Ok(id)
    }
    pub fn transfer(
        &self,
        root: u64,
        known: &std::collections::BTreeSet<u64>,
    ) -> Result<Vec<Definition>, WireError> {
        fn walk(
            c: &Catalog,
            id: u64,
            known: &std::collections::BTreeSet<u64>,
            seen: &mut std::collections::BTreeSet<u64>,
            out: &mut Vec<Definition>,
            depth: usize,
        ) -> Result<(), WireError> {
            if known.contains(&id) || !seen.insert(id) {
                return Ok(());
            }
            if depth > 128 {
                return Err(WireError("reference depth exceeded"));
            }
            let d = c
                .definitions
                .get(&id)
                .ok_or(WireError("unknown definition"))?;
            let mut children = Vec::new();
            references(&d.value, &mut children)?;
            for child in children {
                walk(c, child, known, seen, out, depth + 1)?
            }
            out.push(d.clone());
            Ok(())
        }
        let mut out = Vec::new();
        walk(self, root, known, &mut Default::default(), &mut out, 0)?;
        Ok(out)
    }
}
fn references(value: &Value, out: &mut Vec<u64>) -> Result<(), WireError> {
    if let Some(id) = super::reference(value)? {
        out.push(id)
    } else {
        match value {
            Value::Array(values) => {
                for v in values {
                    references(v, out)?
                }
            }
            Value::Object(values) => {
                for v in values.values() {
                    references(v, out)?
                }
            }
            _ => {}
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Default)]
pub struct Receiver {
    pub by_digest: Map<Digest, u64>,
    pub last_by_kind: Map<String, u64>,
    tree: super::manifest::Tree,
    dirty_bindings: Map<String, Map<String, ()>>,
    pub definitions: Map<u64, Definition>,
    pub hashes: Map<u64, Digest>,
    pub external: Map<u64, Digest>,
    pub view_keys: Map<String, Value>,
    pub views: Map<String, View>,
    pub pending: Map<String, View>,
    pub partials: Map<u64, Partial>,
    pub remote_partials: Map<u64, PartialReceipt>,
}
impl Receiver {
    pub fn receive(&mut self, definitions: &[Definition]) -> Result<(), WireError> {
        let mut next = self.definitions.clone();
        for definition in definitions {
            if definition.id == 0
                || !super::KINDS.contains(&definition.kind.as_str())
                || self.external.contains_key(&definition.id)
                || next.insert(definition.id, definition.clone()).is_some()
            {
                return Err(WireError("duplicate or invalid definition"));
            }
        }
        let mut hashes = self.hashes.clone();
        for definition in definitions {
            let digest = super::value_digest_with_known(
                Some(&definition.kind),
                &definition.value,
                &next,
                &super::KnownPair(&hashes, &self.external),
            )?;
            hashes.insert(definition.id, digest);
        }
        for definition in definitions {
            self.record_definition(definition.id, hashes[&definition.id])?;
        }
        for definition in definitions {
            self.last_by_kind
                .insert(definition.kind.clone(), definition.id);
        }
        self.hashes = hashes;
        self.definitions = next;
        Ok(())
    }
    pub fn materialize_alias(&mut self, definition: &Definition) -> Result<(), WireError> {
        if let Some(old) = self.definitions.get(&definition.id) {
            return if old.kind == definition.kind && old.value == definition.value {
                Ok(())
            } else {
                Err(WireError("alias body changed"))
            };
        }
        let expected = *self
            .external
            .get(&definition.id)
            .ok_or(WireError("alias is not received"))?;
        let digest = super::value_digest_with_known(
            Some(&definition.kind),
            &definition.value,
            &self.definitions,
            &super::KnownPair(&self.hashes, &self.external),
        )?;
        if digest != expected {
            return Err(WireError("alias qualification mismatch"));
        }
        self.external.remove(&definition.id);
        self.hashes.insert(definition.id, digest);
        self.definitions.insert(definition.id, definition.clone());
        self.last_by_kind
            .insert(definition.kind.clone(), definition.id);
        Ok(())
    }
    pub fn resolve(&self, id: u64) -> Result<Value, WireError> {
        fn resolve(
            value: &Value,
            defs: &Map<u64, Definition>,
            visiting: &mut std::collections::BTreeSet<u64>,
            depth: usize,
        ) -> Result<Value, WireError> {
            if depth > 128 {
                return Err(WireError("reference depth exceeded"));
            }
            if let Some(id) = super::reference(value)? {
                if !visiting.insert(id) {
                    return Err(WireError("reference cycle"));
                }
                let result = resolve(
                    &defs
                        .get(&id)
                        .ok_or(WireError("unresolved reference"))?
                        .value,
                    defs,
                    visiting,
                    depth + 1,
                );
                visiting.remove(&id);
                return result;
            }
            match value {
                Value::Array(values) => values
                    .iter()
                    .map(|v| resolve(v, defs, visiting, depth + 1))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array),
                Value::Object(values) => values
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), resolve(v, defs, visiting, depth + 1)?)))
                    .collect::<Result<serde_json::Map<_, _>, WireError>>()
                    .map(Value::Object),
                _ => Ok(value.clone()),
            }
        }
        resolve(
            &serde_json::json!({"ref":id}),
            &self.definitions,
            &mut Default::default(),
            0,
        )
    }
    pub fn register_target(
        &mut self,
        target: &super::request::Target,
        data: &Data,
    ) -> Result<(), WireError> {
        for change in &data.changes {
            let part = if target.resource == super::request::Resource::State {
                let name = change
                    .sets
                    .first()
                    .and_then(|s| s.key.get(0))
                    .and_then(Value::as_str);
                target
                    .parts
                    .iter()
                    .copied()
                    .find(|p| Some(p.as_str()) == name)
            } else {
                None
            };
            if target.resource == super::request::Resource::State && part.is_none() {
                continue;
            }
            self.view_keys.insert(
                change.view_id.clone(),
                serde_json::from_slice(&target.view_key(part)?)
                    .map_err(|_| WireError("invalid view key"))?,
            );
        }
        Ok(())
    }
    pub fn manifest(&self) -> Result<Vec<(Value, Digest, Option<u64>)>, WireError> {
        let mut entries = Vec::new();
        let mut received = std::collections::BTreeSet::new();
        let mut digests = self.external.clone();
        for (&id, definition) in &self.definitions {
            let digest = if let Some(&hash) = self.hashes.get(&id) {
                hash
            } else {
                super::value_digest_with_known(
                    Some(&definition.kind),
                    &definition.value,
                    &self.definitions,
                    &self.external,
                )?
            };
            digests.insert(id, digest);
        }
        for (&id, &digest) in &digests {
            if received.insert(digest) {
                entries.push((serde_json::json!(["received", hex(&digest)]), digest, None));
            }
            entries.push((serde_json::json!(["alias", id]), digest, Some(id)));
        }
        for (id, view) in &self.views {
            if let Some(key) = self.view_keys.get(id) {
                for (logical, &definition) in &view.bindings {
                    let logical: Value = serde_json::from_str(logical)
                        .map_err(|_| WireError("invalid binding key"))?;
                    entries.push((
                        serde_json::json!(["binding", key, logical]),
                        *digests
                            .get(&definition)
                            .ok_or(WireError("missing binding digest"))?,
                        Some(definition),
                    ));
                }
            }
        }
        for (&id, p) in &self.partials {
            entries.push((
                serde_json::json!(["partial", p.delivery_id, id]),
                super::hash(&p.bytes),
                Some(id),
            ));
        }
        for (&id, p) in &self.remote_partials {
            entries.push((
                serde_json::json!(["partial", p.delivery_id, id]),
                parse_digest(&p.prefix_hash)?,
                Some(id),
            ));
        }
        Ok(entries)
    }
    pub fn root(&self) -> Result<Digest, WireError> {
        Ok(self.tree.root())
    }
    fn record_definition(&mut self, id: u64, digest: Digest) -> Result<(), WireError> {
        self.by_digest.entry(digest).or_insert_with(|| id);
        if let Some(definition) = self.definitions.get(&id) {
            self.last_by_kind.insert(definition.kind.clone(), id);
        }
        self.tree.insert(
            super::canonical_json(None, &serde_json::json!(["received", hex(&digest)]))?,
            digest,
        )?;
        self.tree.insert(
            super::canonical_json(None, &serde_json::json!(["alias", id]))?,
            digest,
        )
    }
    pub fn import_manifest(
        &mut self,
        entries: &[(Value, Digest, Option<u64>)],
    ) -> Result<(), WireError> {
        let mut next = Self {
            partials: self.partials.clone(),
            remote_partials: self.remote_partials.clone(),
            ..Self::default()
        };
        for (key, digest, id) in entries {
            match key.get(0).and_then(Value::as_str) {
                Some("alias") => {
                    let id = id.ok_or(WireError("missing alias id"))?;
                    if key.get(1).and_then(Value::as_u64) != Some(id)
                        || next.external.insert(id, *digest).is_some()
                    {
                        return Err(WireError("invalid alias manifest"));
                    }
                    next.by_digest.entry(*digest).or_insert_with(|| id);
                }
                Some("binding") => {
                    let id = id.ok_or(WireError("missing binding id"))?;
                    let canonical = super::canonical_json(None, &key[1])?;
                    let view_id = hex(&super::hash(canonical));
                    next.view_keys.insert(view_id.clone(), key[1].clone());
                    next.views.entry(view_id).or_default().bindings.insert(
                        String::from_utf8(super::canonical_json(None, &key[2])?).unwrap(),
                        id,
                    );
                }
                Some("received") => {}
                Some("partial") => {
                    let id = id.ok_or(WireError("missing partial id"))?;
                    let (receipt_hash, delivery) = if let Some(p) = next.partials.get(&id) {
                        (super::hash(&p.bytes), &p.delivery_id)
                    } else if let Some(p) = next.remote_partials.get(&id) {
                        (parse_digest(&p.prefix_hash)?, &p.delivery_id)
                    } else {
                        return Err(WireError("partial receipt needs prefix reconciliation"));
                    };
                    if key.get(1).and_then(Value::as_str) != Some(delivery.as_str())
                        || key.get(2).and_then(Value::as_u64) != Some(id)
                        || receipt_hash != *digest
                    {
                        return Err(WireError("partial manifest mismatch"));
                    }
                }
                _ => return Err(WireError("invalid manifest namespace")),
            }
        }
        let ids = next.views.keys().cloned().collect::<Vec<_>>();
        for id in ids {
            let view = next.views.get_mut(&id).unwrap();
            let entries = view
                .bindings
                .iter()
                .map(|(key, id)| (key.as_bytes().to_vec(), next.external[id]))
                .collect::<Vec<_>>();
            for (logical, value) in &entries {
                view.tree.insert(logical.clone(), *value)?;
                if let Some(key) = next.view_keys.get(&id) {
                    let logical: Value = serde_json::from_slice(logical)
                        .map_err(|_| WireError("invalid logical key"))?;
                    view.receipt_tree.insert(
                        super::canonical_json(None, &serde_json::json!(["binding", key, logical]))?,
                        *value,
                    )?;
                }
            }
            view.image = hex(&view.tree.root());
        }
        // The independently returned root must also include the generated received
        // set; fabricated/missing/duplicate namespace entries cannot be accepted.
        let expected = super::manifest_root(
            &entries
                .iter()
                .map(|(key, hash, _)| Ok((super::canonical_json(None, key)?, *hash)))
                .collect::<Result<Vec<_>, WireError>>()?,
        )?;
        let full = next
            .manifest()?
            .into_iter()
            .map(|(key, hash, _)| Ok((super::canonical_json(None, &key)?, hash)))
            .collect::<Result<Vec<_>, WireError>>()?;
        if super::manifest_root(&full)? != expected {
            return Err(WireError("manifest mismatch"));
        }
        for (key, hash) in full {
            next.tree.insert(key, hash)?;
        }
        *self = next;
        Ok(())
    }
}
pub fn hex(value: &[u8]) -> String {
    value.iter().map(|v| format!("{v:02x}")).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;
    #[test]
    fn shared_content_is_transferred_once_and_new_observation_reuses_values() {
        let mut catalog = Catalog::default();
        let mut receiver = Receiver::default();
        let first = json!({"observed_at":100,"models_status":"complete","models":[{"model":"codex","total_tokens":18446744073709551615u64}],"quota":null});
        let root = catalog.intern("main", &first).unwrap();
        let sent = catalog.transfer(root, &BTreeSet::new()).unwrap();
        assert!(!sent.is_empty());
        receiver.receive(&sent).unwrap();
        assert_eq!(receiver.resolve(root).unwrap(), first);
        let known = receiver
            .definitions
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        assert!(catalog.transfer(root, &known).unwrap().is_empty());
        let mut next = first.clone();
        next["observed_at"] = json!(101);
        let new_root = catalog.intern("main", &next).unwrap();
        let delta = catalog.transfer(new_root, &known).unwrap();
        // Only the new timestamp and its containing object need definitions.
        assert_eq!(delta.len(), 2);
        assert!(delta
            .iter()
            .all(|d| d.kind != "model_usage" && d.kind != "text" && d.kind != "u64"));
        receiver.receive(&delta).unwrap();
        assert_eq!(receiver.resolve(new_root).unwrap(), next);
        assert_ne!(
            receiver.root().unwrap(),
            super::super::manifest_root(&[]).unwrap()
        );
    }
    #[test]
    fn manifest_roundtrip_keeps_large_aliases_and_multiple_binding_namespaces() {
        let mut catalog = Catalog::with_first_id(1u64 << 60);
        let mut receiver = Receiver::default();
        for (part, value, kind) in [
            (
                "main",
                json!({"observed_at":1800000000,"models_status":"complete","quota":null,"models":[{"model":"SOL","total_tokens":11,"input_tokens":11,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":0,"estimated_cost":{"price_revision":"saved","ordinary_input_dollars":-0.0,"cache_write_input_dollars":0.0,"cached_input_dollars":1.0,"output_dollars":2.0,"total_dollars":3.0}}]}),
                "main",
            ),
            (
                "account_choices",
                json!({"default_account_id":"account-1","accounts":[{"id":"account-1","is_current":true,"login_id":null}]}),
                "account_choices",
            ),
            (
                "runtime",
                json!({"rest_product_version":"1.0.173","recorder_product_version":null,"recorder_status":"unavailable"}),
                "runtime",
            ),
        ] {
            let target = super::super::request::Target::parse(&format!("/state?parts={part}"))
                .unwrap()
                .unwrap();
            let key = serde_json::from_slice::<Value>(
                &target.view_key(target.parts.first().copied()).unwrap(),
            )
            .unwrap();
            let view_id = hex(&super::super::hash(
                super::super::canonical_json(None, &key).unwrap(),
            ));
            let id = catalog.intern(kind, &value).unwrap();
            let known = receiver.definitions.keys().copied().collect();
            let image = hex(&super::super::manifest_root(&[(
                super::super::canonical_json(None, &json!([part])).unwrap(),
                catalog.digests[&id],
            )])
            .unwrap());
            let data = Data {
                result: "data".into(),
                delivery_id: format!("delivery-{part}"),
                definitions: catalog.transfer(id, &known).unwrap(),
                patches: vec![],
                fragments: vec![],
                changes: vec![Change {
                    view_id: view_id.clone(),
                    account: None,
                    base_image: None,
                    target_image: image,
                    sets: vec![Set {
                        key: json!([part]),
                        value: Ref { r#ref: id },
                    }],
                    removes: vec![],
                }],
                completed_views: vec![view_id],
                next_cursor: None,
            };
            receiver.register_target(&target, &data).unwrap();
            receiver.apply(&data).unwrap();
        }
        let entries = receiver.manifest().unwrap();
        let expected = super::super::manifest_root(
            &entries
                .iter()
                .map(|(key, digest, _)| (super::super::canonical_json(None, key).unwrap(), *digest))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(
            receiver.root().unwrap(),
            expected,
            "cached root must equal a complete independent manifest walk"
        );
        let mut imported = Receiver::default();
        imported.import_manifest(&entries).unwrap();
        assert_eq!(receiver.root().unwrap(), imported.root().unwrap());
        for hash in receiver.hashes.values() {
            assert!(imported.by_digest.contains_key(hash));
        }
        assert_eq!(imported.views.len(), 3);
    }
    #[test]
    fn receive_is_atomic_and_preserves_null_missing_and_money_bits() {
        let mut catalog = Catalog::default();
        let mut receiver = Receiver::default();
        let cost = json!({"price_revision":"saved","ordinary_input_dollars":-0.0,"cached_input_dollars":0.0,"cache_write_input_dollars":1.0,"output_dollars":2.0,"total_dollars":3.0});
        let root = catalog.intern("model_cost", &cost).unwrap();
        let sent = catalog.transfer(root, &BTreeSet::new()).unwrap();
        let mut broken = sent.clone();
        broken.last_mut().unwrap().value = json!({"ref":999999});
        assert!(receiver.receive(&broken).is_err());
        assert!(receiver.definitions.is_empty());
        receiver.receive(&sent).unwrap();
        let result = receiver.resolve(root).unwrap();
        assert_eq!(
            result["ordinary_input_dollars"].as_f64().unwrap().to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            result["cached_input_dollars"].as_f64().unwrap().to_bits(),
            0.0f64.to_bits()
        );
        let null = catalog.intern("main", &json!({"quota":null})).unwrap();
        let missing = catalog.intern("main", &json!({})).unwrap();
        assert_ne!(null, missing);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ref {
    pub r#ref: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Set {
    pub key: Value,
    pub value: Ref,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub view_id: String,
    pub account: Option<String>,
    pub base_image: Option<String>,
    pub target_image: String,
    pub sets: Vec<Set>,
    pub removes: Vec<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub id: u64,
    pub kind: String,
    pub base: Ref,
    pub ops: Vec<FieldOp>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum FieldOp {
    Set { path: String, value: Value },
    Remove { path: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fragment {
    pub descriptor: Option<FragmentDescriptor>,
    pub id: u64,
    pub kind: String,
    pub total_bytes: u64,
    pub offset: u64,
    pub content_hash: String,
    pub data: String,
    pub last: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub result: String,
    pub delivery_id: String,
    pub definitions: Vec<Definition>,
    pub patches: Vec<Patch>,
    pub changes: Vec<Change>,
    pub fragments: Vec<Fragment>,
    pub completed_views: Vec<String>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct View {
    pub image: String,
    pub bindings: Map<String, u64>,
    pub tree: super::manifest::Tree,
    pub receipt_tree: super::manifest::Tree,
}

impl Receiver {
    pub fn apply(&mut self, data: &Data) -> Result<(), WireError> {
        if data.result != "data" || data.delivery_id.is_empty() {
            return Err(WireError("invalid delivery"));
        }
        let mut next = self.clone();
        let mut added = Vec::new();
        for definition in &data.definitions {
            if definition.id == 0
                || !super::KINDS.contains(&definition.kind.as_str())
                || next.external.contains_key(&definition.id)
                || next
                    .definitions
                    .insert(definition.id, definition.clone())
                    .is_some()
            {
                return Err(WireError("duplicate or invalid definition"));
            }
            added.push(definition.id);
        }
        for patch in &data.patches {
            if next.definitions.contains_key(&patch.id) {
                return Err(WireError("duplicate patch"));
            }
            let base = next
                .definitions
                .get(&patch.base.r#ref)
                .ok_or(WireError("missing patch base"))?;
            if base.kind != patch.kind {
                return Err(WireError("patch kind mismatch"));
            }
            let mut value = base.value.clone();
            for op in &patch.ops {
                match op {
                    FieldOp::Set { path, value: new } => {
                        let (parent, key) = pointer_parent(&mut value, path)?;
                        parent
                            .as_object_mut()
                            .ok_or(WireError("patch parent not object"))?
                            .insert(key, new.clone());
                    }
                    FieldOp::Remove { path } => {
                        let (parent, key) = pointer_parent(&mut value, path)?;
                        if parent
                            .as_object_mut()
                            .ok_or(WireError("patch parent not object"))?
                            .remove(&key)
                            .is_none()
                        {
                            return Err(WireError("missing remove"));
                        }
                    }
                }
            }
            next.definitions.insert(
                patch.id,
                Definition {
                    id: patch.id,
                    kind: patch.kind.clone(),
                    value,
                },
            );
            added.push(patch.id);
        }
        for fragment in &data.fragments {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&fragment.data)
                .map_err(|_| WireError("invalid fragment encoding"))?;
            if bytes.is_empty()
                || bytes.len() > 8192
                || (!fragment.last && bytes.len() != 8192)
                || fragment
                    .offset
                    .checked_add(bytes.len() as u64)
                    .is_none_or(|n| {
                        n > fragment.total_bytes || fragment.last != (n == fragment.total_bytes)
                    })
            {
                return Err(WireError("invalid fragment boundary"));
            }
            if !next.partials.contains_key(&fragment.id) && fragment.descriptor.is_none() {
                return Err(WireError("missing fragment descriptor"));
            }
            let partial = next.partials.entry(fragment.id).or_insert_with(|| Partial {
                descriptor: fragment.descriptor.clone().unwrap(),
                delivery_id: data.delivery_id.clone(),
                kind: fragment.kind.clone(),
                hash: fragment.content_hash.clone(),
                total: fragment.total_bytes,
                bytes: Vec::new(),
            });
            if fragment
                .descriptor
                .as_ref()
                .is_some_and(|d| d != &partial.descriptor)
            {
                return Err(WireError("fragment descriptor changed"));
            }
            if partial.kind != fragment.kind
                || partial.hash != fragment.content_hash
                || partial.total != fragment.total_bytes
                || partial.bytes.len() as u64 != fragment.offset
            {
                return Err(WireError("fragment prefix mismatch"));
            }
            partial.bytes.extend(bytes);
            if fragment.last {
                let complete = next.partials.remove(&fragment.id).unwrap();
                if hex(&super::hash(&complete.bytes)) != complete.hash {
                    return Err(WireError("fragment content hash"));
                }
                if complete.kind == "definition" {
                    let definition: Definition = serde_json::from_slice(&complete.bytes)
                        .map_err(|_| WireError("invalid fragmented definition"))?;
                    if definition.id != fragment.id {
                        return Err(WireError("fragment identity"));
                    }
                    if next
                        .definitions
                        .insert(definition.id, definition.clone())
                        .is_some()
                    {
                        return Err(WireError("duplicate fragment definition"));
                    }
                    added.push(definition.id);
                } else {
                    return Err(WireError("invalid fragmented kind"));
                }
            }
        }
        for id in added {
            let d = &next.definitions[&id];
            let hash = super::value_digest_with_known(
                Some(&d.kind),
                &d.value,
                &next.definitions,
                &super::KnownPair(&next.hashes, &next.external),
            )?;
            next.hashes.insert(id, hash);
            next.record_definition(id, hash)?;
        }
        let affected = data
            .fragments
            .iter()
            .map(|f| f.id)
            .collect::<std::collections::BTreeSet<_>>();
        for id in affected {
            if let Some(old) = self.partials.get(&id) {
                let key = super::canonical_json(
                    None,
                    &serde_json::json!(["partial", old.delivery_id, id]),
                )?;
                next.tree.remove(&key)?;
            }
            if let Some(partial) = next.partials.get(&id) {
                next.tree.insert(
                    super::canonical_json(
                        None,
                        &serde_json::json!(["partial", partial.delivery_id, id]),
                    )?,
                    super::hash(&partial.bytes),
                )?;
            }
        }
        for change in &data.changes {
            let view = next
                .pending
                .entry(change.view_id.clone())
                .or_insert_with(|| next.views.get(&change.view_id).cloned().unwrap_or_default());
            if view.image != change.target_image {
                if change.base_image.as_deref()
                    != next.views.get(&change.view_id).map(|v| v.image.as_str())
                {
                    return Err(WireError("view base mismatch"));
                }
                *view = next.views.get(&change.view_id).cloned().unwrap_or_default();
                view.image = change.target_image.clone();
                next.dirty_bindings.remove(&change.view_id);
            }
            for set in &change.sets {
                if !next.definitions.contains_key(&set.value.r#ref)
                    && !next.external.contains_key(&set.value.r#ref)
                {
                    return Err(WireError("unresolved binding"));
                }
                let key = String::from_utf8(super::canonical_json(None, &set.key)?).unwrap();
                next.dirty_bindings
                    .entry(change.view_id.clone())
                    .or_default()
                    .insert(key.clone(), ());
                let digest = next
                    .hashes
                    .get(&set.value.r#ref)
                    .or_else(|| next.external.get(&set.value.r#ref))
                    .ok_or(WireError("missing row digest"))?;
                view.tree.insert(key.as_bytes().to_vec(), *digest)?;
                if let Some(view_key) = next.view_keys.get(&change.view_id) {
                    view.receipt_tree.insert(
                        super::canonical_json(
                            None,
                            &serde_json::json!(["binding", view_key, set.key]),
                        )?,
                        *digest,
                    )?;
                }
                view.bindings.insert(key, set.value.r#ref);
            }
            for key in &change.removes {
                let key = String::from_utf8(super::canonical_json(None, key)?).unwrap();
                view.bindings
                    .remove(&key)
                    .ok_or(WireError("missing binding remove"))?;
                view.tree.remove(key.as_bytes())?;
                if let Some(view_key) = next.view_keys.get(&change.view_id) {
                    let logical: Value =
                        serde_json::from_str(&key).map_err(|_| WireError("invalid logical key"))?;
                    view.receipt_tree.remove(&super::canonical_json(
                        None,
                        &serde_json::json!(["binding", view_key, logical]),
                    )?)?;
                }
                next.dirty_bindings
                    .entry(change.view_id.clone())
                    .or_default()
                    .insert(key, ());
            }
        }
        for view in &data.completed_views {
            let complete = next
                .pending
                .remove(view)
                .ok_or(WireError("missing completed view"))?;
            if let Some(key) = next.view_keys.get(view) {
                let mut prefix = super::canonical_json(None, &serde_json::json!(["binding", key]))?;
                *prefix.last_mut().unwrap() = b',';
                next.tree.replace_prefix(&prefix, &complete.receipt_tree)?;
            }
            next.dirty_bindings.remove(view);
            next.views.insert(view.clone(), complete);
        }
        *self = next;
        Ok(())
    }
}
fn pointer_parent<'a>(
    value: &'a mut Value,
    path: &str,
) -> Result<(&'a mut Value, String), WireError> {
    let (parent, key) = path.rsplit_once('/').ok_or(WireError("invalid pointer"))?;
    let key = key.replace("~1", "/").replace("~0", "~");
    let parent = if parent.is_empty() {
        value
    } else {
        value
            .pointer_mut(parent)
            .ok_or(WireError("missing pointer parent"))?
    };
    Ok((parent, key))
}
impl Catalog {
    pub fn patch(&self, id: u64, known: &std::collections::BTreeSet<u64>) -> Option<Patch> {
        let target = self.definitions.get(&id)?;
        target.value.as_object()?;
        let base = known
            .iter()
            .rev()
            .filter_map(|id| self.definitions.get(id))
            .find(|d| d.kind == target.kind && d.value.is_object())?;
        self.patch_from(id, base.id)
    }
    pub fn patch_from(&self, id: u64, base: u64) -> Option<Patch> {
        let target = self.definitions.get(&id)?;
        let object = target.value.as_object()?;
        let base = self.definitions.get(&base)?;
        if base.kind != target.kind {
            return None;
        }
        let old = base.value.as_object()?;
        let mut ops = Vec::new();
        let pointer = |key: &str| format!("/{}", key.replace('~', "~0").replace('/', "~1"));
        for (key, value) in object {
            if old.get(key) != Some(value) {
                ops.push(FieldOp::Set {
                    path: pointer(key),
                    value: value.clone(),
                })
            }
        }
        for key in old.keys() {
            if !object.contains_key(key) {
                ops.push(FieldOp::Remove { path: pointer(key) })
            }
        }
        let patch = Patch {
            id,
            kind: target.kind.clone(),
            base: Ref { r#ref: base.id },
            ops,
        };
        (serde_json::to_vec(&patch).ok()?.len() < serde_json::to_vec(target).ok()?.len())
            .then_some(patch)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Partial {
    pub descriptor: FragmentDescriptor,
    pub delivery_id: String,
    pub kind: String,
    pub hash: String,
    pub total: u64,
    pub bytes: Vec<u8>,
}

/// Substitute server-local IDs with receiver-retained aliases, without body replay.
pub fn remap_refs(value: &mut Value, aliases: &BTreeMap<u64, u64>) {
    if let Ok(Some(id)) = super::reference(value) {
        if let Some(alias) = aliases.get(&id) {
            *value = serde_json::json!({"ref":alias});
        }
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                remap_refs(value, aliases)
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                remap_refs(value, aliases)
            }
        }
        _ => {}
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentDescriptor {
    pub definition_kind: String,
    pub value_hash: String,
    pub view_id: String,
    pub target_image: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialReceipt {
    pub delivery_id: String,
    pub id: u64,
    pub committed_offset: u64,
    pub prefix_hash: String,
    pub content_hash: String,
    pub total_bytes: u64,
    pub descriptor: FragmentDescriptor,
}
impl Receiver {
    pub fn partial_receipts(&self) -> Vec<PartialReceipt> {
        self.partials
            .iter()
            .map(|(&id, p)| PartialReceipt {
                delivery_id: p.delivery_id.clone(),
                id,
                committed_offset: p.bytes.len() as u64,
                prefix_hash: hex(&super::hash(&p.bytes)),
                content_hash: p.hash.clone(),
                total_bytes: p.total,
                descriptor: p.descriptor.clone(),
            })
            .chain(self.remote_partials.values().cloned())
            .collect()
    }
    pub fn discard_partials(&mut self, view_id: &str) {
        let keys = self
            .partials
            .iter()
            .filter(|(_, p)| p.descriptor.view_id == view_id)
            .map(|(&id, p)| serde_json::json!(["partial", p.delivery_id, id]))
            .chain(
                self.remote_partials
                    .iter()
                    .filter(|(_, p)| p.descriptor.view_id == view_id)
                    .map(|(&id, p)| serde_json::json!(["partial", p.delivery_id, id])),
            )
            .collect::<Vec<_>>();
        for key in keys {
            let _ = self
                .tree
                .remove(&super::canonical_json(None, &key).unwrap());
        }
        self.partials.retain(|_, p| p.descriptor.view_id != view_id);
        self.remote_partials
            .retain(|_, p| p.descriptor.view_id != view_id);
    }
}
pub fn parse_digest(value: &str) -> Result<Digest, WireError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(WireError("invalid digest"));
    }
    let mut digest = [0; 32];
    for (i, b) in digest.iter_mut().enumerate() {
        *b = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)
            .map_err(|_| WireError("invalid digest"))?
    }
    Ok(digest)
}
impl Data {
    pub fn record_count(&self) -> usize {
        self.definitions.len() + self.patches.len() + self.fragments.len() + self.changes.len()
    }
    pub fn ordinal_range(&self, start: usize, end: usize) -> Result<Self, WireError> {
        if start > end || end > self.record_count() {
            return Err(WireError("invalid receipt ordinal"));
        }
        let mut next = self.clone();
        let mut offset = 0;
        fn take<T: Clone>(values: &[T], offset: &mut usize, start: usize, end: usize) -> Vec<T> {
            let lower = start.saturating_sub(*offset).min(values.len());
            let upper = end.saturating_sub(*offset).min(values.len());
            *offset += values.len();
            values[lower..upper].to_vec()
        }
        next.definitions = take(&self.definitions, &mut offset, start, end);
        next.patches = take(&self.patches, &mut offset, start, end);
        next.fragments = take(&self.fragments, &mut offset, start, end);
        next.changes = take(&self.changes, &mut offset, start, end);
        if end < self.record_count() {
            next.completed_views.clear();
            next.next_cursor = None
        }
        Ok(next)
    }
}

#[cfg(test)]
mod interop_tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;
    #[test]
    fn receiver_fixture_retains_exact_main_money_and_changed_field() {
        let mut catalog = Catalog::default();
        let mut receiver = Receiver::default();
        let target = super::super::request::Target::parse("/state?parts=main")
            .unwrap()
            .unwrap();
        let mut value = json!({"acquisition_state":"ready","observed_at":100,"authenticated":true,"plan_label":"fixed","selection":null,"quota":null,"models_status":"complete","models":[{"model":"codex","total_tokens":18446744073709551615u64,"input_tokens":18446744073709551615u64,"cached_input_tokens":0,"cache_write_input_tokens":null,"output_tokens":0,"estimated_cost":{"price_revision":"stored","ordinary_input_dollars":-0.0,"cached_input_dollars":0.0,"cache_write_input_dollars":1.0,"output_dollars":2.0,"total_dollars":3.0}}],"thread_summary":null});
        let mut pages = Vec::new();
        let mut image = None;
        for observed in [100, 101] {
            value["observed_at"] = json!(observed);
            let id = catalog.intern("main", &value).unwrap();
            let target_image = hex(&super::super::manifest_root(&[(
                super::super::canonical_json(None, &json!(["main"])).unwrap(),
                catalog.digests[&id],
            )])
            .unwrap());
            let known = receiver
                .definitions
                .keys()
                .copied()
                .collect::<BTreeSet<_>>();
            let mut definitions = Vec::new();
            let mut patches = Vec::new();
            for d in catalog.transfer(id, &known).unwrap() {
                if let Some(patch) = catalog.patch(d.id, &known) {
                    patches.push(patch)
                } else {
                    definitions.push(d)
                }
            }
            let data = Data {
                result: "data".into(),
                delivery_id: format!("delivery-{observed}"),
                definitions,
                patches,
                changes: vec![Change {
                    view_id: "main-view".into(),
                    account: None,
                    base_image: image.clone(),
                    target_image: target_image.clone(),
                    sets: vec![Set {
                        key: json!(["main"]),
                        value: Ref { r#ref: id },
                    }],
                    removes: vec![],
                }],
                fragments: vec![],
                completed_views: vec!["main-view".into()],
                next_cursor: None,
            };
            receiver.register_target(&target, &data).unwrap();
            receiver.apply(&data).unwrap();
            let resolved = receiver.resolve(id).unwrap();
            assert_eq!(
                resolved["models"][0]["total_tokens"].as_u64(),
                Some(u64::MAX)
            );
            assert_eq!(
                resolved["models"][0]["estimated_cost"]["ordinary_input_dollars"]
                    .as_f64()
                    .unwrap()
                    .to_bits(),
                (-0.0f64).to_bits()
            );
            assert_eq!(resolved["observed_at"], observed);
            pages.push(json!({"data":data,"view_keys":receiver.view_keys,"root":hex(&receiver.root().unwrap()),"main_id":id}));
            image = Some(target_image);
        }
        assert_eq!(pages[1]["data"]["definitions"].as_array().unwrap().len(), 1);
        assert_eq!(pages[1]["data"]["patches"].as_array().unwrap().len(), 1);
        if let Ok(path) = std::env::var("CODEX_INFO_WIRE_INTEROP_FIXTURE") {
            std::fs::write(path,serde_json::to_vec_pretty(&json!({"source":"Rust/.NET receiver parity; fixed values anchored by independent Python byte vectors","pages":pages})).unwrap()).unwrap();
        }
    }
}

use super::*;
use std::collections::BTreeSet;
use wire::sync::{Change, Data, Receiver, Ref, Set};

#[derive(Default)]
pub(super) struct Peer {
    pub(super) receiver: Receiver,
    offered: BTreeSet<u64>,
    protected: BTreeSet<wire::Digest>,
    bases: BTreeMap<u64, wire::Definition>,
    pub(super) deliveries: BTreeMap<String, Data>,
    plans: BTreeMap<String, Plan>,
    seq: u64,
    manifest: Vec<(Value, wire::Digest, Option<u64>)>,
    expected_root: String,
    partial_receipts: Vec<wire::sync::PartialReceipt>,
    confirmed: BTreeMap<String, usize>,
    drop_suggested: BTreeSet<String>,
    active_probe: String,
}
#[derive(Clone)]
struct Plan {
    views: Vec<(Vec<u8>, Arc<View>)>,
    source: Arc<Source>,
    view: usize,
    difference: Option<wire::manifest::Differences>,
    current: Option<(Value, Option<u64>)>,
    stack: Vec<(u64, usize)>,
    definition: Option<u64>,
    fragment: usize,
    encoded: Option<Arc<Vec<u8>>>,
    encoded_hash: Option<String>,
}
impl Peer {
    fn root(&self) -> String {
        hex(&self.receiver.root().unwrap())
    }
    fn reconcile(&mut self, root: &str) -> bool {
        if self.root() == root {
            return true;
        }
        if !self.drop_suggested.is_empty() {
            let mut next = self.receiver.clone();
            for view in &self.drop_suggested {
                next.discard_partials(view);
            }
            if hex(&next.root().unwrap()) == root {
                self.receiver = next;
                self.drop_suggested.clear();
                return true;
            }
        }
        // Receipt acknowledgements can disappear. Verify issued immutable records
        // before accepting metadata; never resend an uncertain definition body.
        let mut candidate = self.receiver.clone();
        let mut completed = Vec::new();
        for (id, data) in &self.deliveries {
            if candidate
                .apply(
                    &data
                        .ordinal_range(*self.confirmed.get(id).unwrap_or(&0), data.record_count())
                        .unwrap(),
                )
                .is_err()
            {
                return false;
            }
            completed.push(id.clone());
            if hex(&candidate.root().unwrap()) == root {
                self.receiver = candidate;
                for id in completed {
                    self.deliveries.remove(&id);
                }
                return true;
            }
        }
        false
    }
    pub fn data(
        &mut self,
        target: &Target,
        source: &Arc<Source>,
        views: Vec<(Vec<u8>, Arc<View>)>,
        root: &str,
    ) -> Option<Vec<u8>> {
        let control = |reason: &str| {
            serde_json::to_vec(&json!({"result":"sync_required","view_id":hex(&Sha256::digest(target.view_key(None).unwrap_or_default())),"reason":reason,"target_image":null,"control_seq":1})).unwrap()
        };
        if !self.reconcile(root) {
            return Some(control("receipt_unknown"));
        }
        for partial in self.receiver.remote_partials.values() {
            let digest = wire::sync::parse_digest(&partial.descriptor.value_hash).ok()?;
            if !source.catalog.digests.values().any(|hash| hash == &digest) {
                let view = partial.descriptor.view_id.clone();
                self.drop_suggested.insert(view.clone());
                return Some(serde_json::to_vec(&json!({"result":"sync_required","view_id":view,"reason":"image_changed","target_image":null,"control_seq":1})).unwrap());
            }
        }
        let mut plan = if let Some(cursor) = &target.cursor {
            let Some(plan) = self.plans.remove(cursor) else {
                return Some(control("cursor_outdated"));
            };
            if plan
                .views
                .iter()
                .map(|(key, _)| key)
                .ne(views.iter().map(|(key, _)| key))
            {
                return Some(serde_json::to_vec(&json!({"error":"bad_request"})).unwrap());
            }
            if plan
                .views
                .iter()
                .any(|(key, view)| source.views.get(key).is_none_or(|v| v.image != view.image))
            {
                return Some(control("cursor_outdated"));
            }
            plan
        } else {
            Plan {
                views,
                source: Arc::clone(source),
                view: 0,
                difference: None,
                current: None,
                stack: Vec::new(),
                definition: None,
                fragment: 0,
                encoded: None,
                encoded_hash: None,
            }
        };
        if target.cursor.is_none()
            && plan.views.iter().all(|(key, view)| {
                self.receiver
                    .views
                    .get(&hex(&Sha256::digest(key)))
                    .is_some_and(|old| old.image == view.image)
            })
        {
            return Some(Vec::new());
        }
        let mut data = Data {
            result: "data".into(),
            delivery_id: opaque_id().ok()?,
            definitions: Vec::new(),
            patches: Vec::new(),
            changes: Vec::new(),
            fragments: Vec::new(),
            completed_views: Vec::new(),
            next_cursor: None,
        };
        let mut used = 8192usize;
        let (mut samples, mut threads, mut steps) = (0usize, 0usize, 0usize);
        let mut emitted = BTreeSet::new();
        let mut blocked = false;
        fn alias(receiver: &Receiver, catalog: &wire::sync::Catalog, id: u64) -> u64 {
            catalog
                .digests
                .get(&id)
                .and_then(|hash| {
                    receiver.by_digest.get(hash).copied().or_else(|| {
                        receiver
                            .partials
                            .iter()
                            .find(|(_, p)| p.descriptor.value_hash == hex(hash))
                            .map(|(old, _)| *old)
                    })
                })
                .unwrap_or(id)
        }
        fn remap(value: &mut Value, receiver: &Receiver, catalog: &wire::sync::Catalog) {
            if let Some(id) = value
                .as_object()
                .filter(|object| object.len() == 1)
                .and_then(|object| object.get("ref"))
                .and_then(Value::as_u64)
            {
                *value = json!({"ref":alias(receiver,catalog,id)});
                return;
            }
            match value {
                Value::Array(values) => {
                    for value in values {
                        remap(value, receiver, catalog)
                    }
                }
                Value::Object(values) => {
                    for value in values.values_mut() {
                        remap(value, receiver, catalog)
                    }
                }
                _ => {}
            }
        }
        fn change<'a>(
            data: &'a mut Data,
            id: &str,
            view: &View,
            base: Option<String>,
        ) -> &'a mut Change {
            if data
                .changes
                .last()
                .is_none_or(|change| change.view_id != id)
            {
                data.changes.push(Change {
                    view_id: id.into(),
                    account: view.account.clone(),
                    base_image: base,
                    target_image: view.image.clone(),
                    sets: Vec::new(),
                    removes: Vec::new(),
                });
            }
            data.changes.last_mut().unwrap()
        }
        'page: while plan.view < plan.views.len() {
            let (key, view) = plan.views[plan.view].clone();
            let view_id = hex(&Sha256::digest(&key));
            let previous = self.receiver.views.get(&view_id);
            let base = previous.map(|v| v.image.clone());
            if previous.is_some_and(|old| old.image == view.image) {
                plan.view += 1;
                plan.difference = None;
                continue;
            }
            if plan.difference.is_none() {
                let empty = wire::manifest::Tree::default();
                plan.difference = Some(
                    view.tree
                        .differences(previous.map(|old| &old.tree).unwrap_or(&empty)),
                );
            }
            if plan.current.is_none() {
                if let Some((key, value)) = plan.difference.as_mut().unwrap().next() {
                    let logical: Value = serde_json::from_slice(&key).ok()?;
                    let id = value.and_then(|_| view.row_ids.get(&key).copied());
                    if value.is_some() && id.is_none() {
                        return None;
                    }
                    if let Some(id) = id {
                        plan.stack.push((id, 0));
                    }
                    plan.current = Some((logical, id));
                } else {
                    change(&mut data, &view_id, &view, base);
                    data.completed_views.push(view_id);
                    plan.view += 1;
                    plan.difference = None;
                    continue;
                }
            }
            let (logical, id) = plan.current.clone().unwrap();
            if logical[0] == "sample" && samples >= 1024 || logical[0] == "thread" && threads >= 256
            {
                break;
            }
            loop {
                if plan.definition.is_none() {
                    while let Some((id, next)) = plan.stack.last_mut() {
                        if steps >= 8192 {
                            break 'page;
                        }
                        steps += 1;
                        if emitted.contains(id)
                            || plan
                                .source
                                .catalog
                                .digests
                                .get(id)
                                .is_some_and(|hash| self.receiver.by_digest.contains_key(hash))
                        {
                            plan.stack.pop();
                            continue;
                        }
                        let children = plan.source.catalog.children.get(id)?;
                        if *next < children.len() {
                            let child = children[*next];
                            *next += 1;
                            plan.stack.push((child, 0));
                        } else {
                            plan.definition = Some(*id);
                            plan.stack.pop();
                            break;
                        }
                    }
                }
                let Some(id) = plan.definition else { break };
                // A parked dependency can be received through another issued
                // resource while this cursor waits. Requalify it before the
                // offered/protected guard; replaying that body is forbidden.
                if plan
                    .source
                    .catalog
                    .digests
                    .get(&id)
                    .is_some_and(|hash| self.receiver.by_digest.contains_key(hash))
                {
                    plan.definition = None;
                    plan.encoded = None;
                    plan.encoded_hash = None;
                    plan.fragment = 0;
                    continue;
                }
                let partial = self.receiver.partials.iter().find(|(_, partial)| {
                    partial.descriptor.value_hash == hex(&plan.source.catalog.digests[&id])
                });
                if (self.offered.contains(&id)
                    || self.protected.contains(&plan.source.catalog.digests[&id]))
                    && !emitted.contains(&id)
                    && plan.fragment == 0
                    && partial.is_none()
                {
                    blocked = true;
                    break 'page;
                }
                let source_definition = plan.source.catalog.definitions.get(&id)?;
                let transferred_id = partial.map(|(&old, _)| old).unwrap_or(id);
                if plan.encoded.is_none() {
                    let mut definition = source_definition.clone();
                    definition.id = transferred_id;
                    remap(&mut definition.value, &self.receiver, &plan.source.catalog);
                    plan.encoded = Some(Arc::new(serde_json::to_vec(&definition).ok()?));
                    plan.encoded_hash = Some(hex(&Sha256::digest(
                        plan.encoded.as_ref().unwrap().as_slice(),
                    )));
                    if let Some((_, partial)) = partial {
                        plan.fragment = partial.bytes.len();
                    }
                }
                let bytes = plan.encoded.as_ref().unwrap();
                if bytes.len() > 8192 {
                    use base64::Engine as _;
                    let offset = plan.fragment;
                    let end = (offset + 8192).min(bytes.len());
                    let descriptor =
                        partial
                            .map(|(_, p)| p.descriptor.clone())
                            .unwrap_or_else(|| wire::sync::FragmentDescriptor {
                                definition_kind: source_definition.kind.clone(),
                                value_hash: hex(&plan.source.catalog.digests[&id]),
                                view_id: view_id.clone(),
                                target_image: view.image.clone(),
                            });
                    let fragment = wire::sync::Fragment {
                        descriptor: if offset == 0 || partial.is_some() {
                            Some(descriptor)
                        } else {
                            None
                        },
                        id: transferred_id,
                        kind: "definition".into(),
                        total_bytes: bytes.len() as u64,
                        offset: offset as u64,
                        content_hash: plan.encoded_hash.clone()?,
                        data: base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .encode(&bytes[offset..end]),
                        last: end == bytes.len(),
                    };
                    let size = serde_json::to_vec(&fragment).ok()?.len() + 1;
                    if used + size > MAX_BODY_BYTES {
                        break 'page;
                    }
                    used += size;
                    data.fragments.push(fragment);
                    self.offered.insert(id);
                    self.protected.insert(plan.source.catalog.digests[&id]);
                    plan.fragment = end;
                    if end < bytes.len() {
                        continue;
                    }
                } else {
                    let definition: wire::Definition = serde_json::from_slice(bytes).ok()?;
                    let patch = self
                        .receiver
                        .last_by_kind
                        .get(&definition.kind)
                        .and_then(|base| plan.source.catalog.patch_from(id, *base))
                        .filter(|_| {
                            alias(&self.receiver, &plan.source.catalog, id) == id
                                && self.receiver.external.is_empty()
                        });
                    let size = patch
                        .as_ref()
                        .and_then(|patch| serde_json::to_vec(patch).ok())
                        .map_or(bytes.len(), |bytes| bytes.len())
                        + 1;
                    if used + size > MAX_BODY_BYTES {
                        break 'page;
                    }
                    used += size;
                    if let Some(patch) = patch {
                        data.patches.push(patch)
                    } else {
                        data.definitions.push(definition)
                    }
                    self.offered.insert(id);
                    self.protected.insert(plan.source.catalog.digests[&id]);
                }
                emitted.insert(id);
                plan.definition = None;
                plan.encoded = None;
                plan.encoded_hash = None;
                plan.fragment = 0;
            }
            if let Some(id) = id {
                let set = Set {
                    key: logical.clone(),
                    value: Ref {
                        r#ref: alias(&self.receiver, &plan.source.catalog, id),
                    },
                };
                let size = serde_json::to_vec(&set).ok()?.len() + 1;
                if used + size > MAX_BODY_BYTES {
                    break;
                }
                used += size;
                change(&mut data, &view_id, &view, base).sets.push(set);
            } else {
                let size = serde_json::to_vec(&logical).ok()?.len() + 1;
                if used + size > MAX_BODY_BYTES {
                    break;
                }
                used += size;
                change(&mut data, &view_id, &view, base)
                    .removes
                    .push(logical.clone());
            }
            if logical[0] == "sample" {
                samples += 1
            }
            if logical[0] == "thread" {
                threads += 1
            }
            plan.current = None;
        }
        if blocked && data.record_count() == 0 {
            if let Some(cursor) = target.cursor.as_ref() {
                self.plans.insert(cursor.clone(), plan);
            }
            return Some(serde_json::to_vec(&json!({"result":"pending","reason":"receiving","view_id":hex(&Sha256::digest(target.view_key(None).unwrap_or_default())),"target_image":null,"control_seq":1})).unwrap());
        }
        if plan.view < plan.views.len() {
            let cursor = opaque_id().ok()?;
            data.next_cursor = Some(cursor.clone());
            self.plans.insert(cursor, plan);
        }
        let bytes = serde_json::to_vec(&data).ok()?;
        if bytes.len() > MAX_BODY_BYTES {
            return None;
        }
        self.receiver.register_target(target, &data).ok()?;
        self.deliveries.insert(data.delivery_id.clone(), data);
        Some(bytes)
    }
    pub(super) fn receipt(&mut self, id: &str, through: u64, root: &str) -> bool {
        let Some(data) = self.deliveries.get(id) else {
            return self.root() == root;
        };
        let Ok(through) = usize::try_from(through) else {
            return false;
        };
        let start = *self.confirmed.get(id).unwrap_or(&0);
        let Ok(prefix) = data.ordinal_range(start, through) else {
            return false;
        };
        let mut next = self.receiver.clone();
        if next.apply(&prefix).is_err() || hex(&next.root().unwrap()) != root {
            return false;
        }
        self.receiver = next;
        self.protected
            .retain(|hash| !self.receiver.by_digest.contains_key(hash));
        if through == data.record_count() {
            self.deliveries.remove(id);
            self.confirmed.remove(id);
        } else {
            self.confirmed.insert(id.into(), through);
        }
        true
    }
    pub(super) fn restore_delivery(
        &mut self,
        meta: &outbox::Meta,
        target: &Target,
        data: &Data,
    ) -> Result<(), wire::WireError> {
        for base in &meta.bases {
            self.bases.insert(base.id, base.clone());
            if self.receiver.definitions.contains_key(&base.id)
                || self.receiver.external.contains_key(&base.id)
            {
                self.receiver.materialize_alias(base)?;
            }
        }
        self.receiver.register_target(target, data)?;
        for hash in &meta.offered {
            self.protected.insert(wire::sync::parse_digest(hash)?);
        }
        self.deliveries
            .insert(data.delivery_id.clone(), data.clone());
        Ok(())
    }
    fn restore_prefixes(
        &self,
        source: &Source,
        next: &mut Receiver,
    ) -> Result<(), wire::WireError> {
        let mut aliases = BTreeMap::new();
        let mut received = BTreeMap::new();
        for (key, digest, id) in &self.manifest {
            if key[0] == "alias" {
                received.insert(*digest, id.ok_or(wire::WireError("missing alias id"))?);
            }
        }
        for (&id, digest) in &source.catalog.digests {
            if let Some(&old) = received.get(digest) {
                aliases.insert(id, old);
            }
        }
        for receipt in &self.partial_receipts {
            let digest = wire::sync::parse_digest(&receipt.descriptor.value_hash)?;
            let Some((&id, _)) = source
                .catalog
                .digests
                .iter()
                .find(|(_, hash)| **hash == digest)
            else {
                next.remote_partials.insert(receipt.id, receipt.clone());
                continue;
            };
            let mut definition = source.catalog.definitions[&id].clone();
            if definition.kind != receipt.descriptor.definition_kind {
                return Err(wire::WireError("partial kind mismatch"));
            }
            definition.id = receipt.id;
            wire::sync::remap_refs(&mut definition.value, &aliases);
            let bytes = serde_json::to_vec(&definition)
                .map_err(|_| wire::WireError("partial serialization"))?;
            let offset = usize::try_from(receipt.committed_offset)
                .map_err(|_| wire::WireError("partial offset"))?;
            if bytes.len() as u64 != receipt.total_bytes
                || offset >= bytes.len()
                || hex(&Sha256::digest(&bytes)) != receipt.content_hash
                || hex(&Sha256::digest(
                    bytes
                        .get(..offset)
                        .ok_or(wire::WireError("partial offset"))?,
                )) != receipt.prefix_hash
            {
                return Err(wire::WireError("partial prefix mismatch"));
            }
            next.partials.insert(
                receipt.id,
                wire::sync::Partial {
                    descriptor: receipt.descriptor.clone(),
                    delivery_id: receipt.delivery_id.clone(),
                    kind: "definition".into(),
                    hash: receipt.content_hash.clone(),
                    total: receipt.total_bytes,
                    bytes: bytes[..offset].to_vec(),
                },
            );
        }
        Ok(())
    }
}

async fn record(stream: &mut h2::SendStream<Bytes>, value: Value) -> Result<(), ()> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| ())?;
    bytes.push(b'\n');
    if bytes.len() > 8192 {
        return Err(());
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    tokio::time::timeout_at(deadline, async {
        stream.reserve_capacity(bytes.len());
        let mut offset = 0;
        while offset < bytes.len() {
            let capacity = poll_fn(|cx| stream.poll_capacity(cx))
                .await
                .ok_or(())?
                .map_err(|_| ())?;
            if capacity == 0 {
                continue;
            }
            let end = (offset + capacity).min(bytes.len());
            stream
                .send_data(Bytes::copy_from_slice(&bytes[offset..end]), false)
                .map_err(|_| ())?;
            offset = end;
        }
        Ok(())
    })
    .await
    .map_err(|_| ())?
}
fn targets(value: &Value) -> Result<Vec<Target>, ()> {
    let values = value.as_array().ok_or(())?;
    if values.len() > 7 {
        return Err(());
    }
    values
        .iter()
        .map(|value| {
            let path = value.get("path").and_then(Value::as_str).ok_or(())?;
            let query = value.get("query").and_then(Value::as_object).ok_or(())?;
            let pairs = query
                .iter()
                .map(|(key, value)| {
                    let text = value
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| value.as_i64().map(|n| n.to_string()))
                        .ok_or(())?;
                    Ok(format!("{key}={text}"))
                })
                .collect::<Result<Vec<_>, ()>>()?;
            let target = format!("{path}?{}", pairs.join("&"));
            let target = Target::parse(&target).map_err(|_| ())?.ok_or(())?;
            if target.resource == Resource::Subscription {
                return Err(());
            }
            Ok(target)
        })
        .collect()
}
fn target_views(hub: &Hub, targets: &[Target]) -> Vec<(Target, Vec<u8>, Option<String>)> {
    let source = hub.source.read().unwrap_or_else(|p| p.into_inner());
    let mut result = Vec::new();
    for target in targets {
        let keys = if target.resource == Resource::State {
            target
                .parts
                .iter()
                .filter_map(|part| target.view_key(Some(*part)).ok())
                .collect::<Vec<_>>()
        } else {
            target.view_key(None).into_iter().collect()
        };
        for key in keys {
            let image = source.views.get(&key).map(|view| view.image.clone());
            result.push((target.clone(), key, image));
        }
    }
    result
}
fn resolved(hub: &Hub, targets: &[Target]) -> Value {
    json!(target_views(hub,targets).into_iter().map(|(target,key,image)|json!({"path":target.resource.path(),"query":serde_json::from_slice::<Value>(&key).unwrap()[1],"view_id":hex(&Sha256::digest(&key)),"target_image":image})).collect::<Vec<_>>())
}
fn notification(
    hub: &Hub,
    targets: &[Target],
    sent: &mut BTreeMap<String, String>,
    seq: u64,
) -> Option<Value> {
    let mut views = Vec::new();
    for (_, key, image) in target_views(hub, targets) {
        let id = hex(&Sha256::digest(&key));
        let marker = image.clone().unwrap_or_default();
        if sent.get(&id) != Some(&marker) {
            sent.insert(id.clone(), marker);
            views.push(json!({"view_id":id,"target_image":image}));
        }
    }
    (!views.is_empty()).then(|| json!({"type":"changed","notify_seq":seq,"views":views}))
}
pub(super) async fn subscription(
    request: HttpRequest<h2::RecvStream>,
    mut respond: SendResponse<Bytes>,
    hub: Arc<Hub>,
    consumer: String,
    peer: Arc<Mutex<Peer>>,
) {
    if request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        != Some("application/x-ndjson; charset=utf-8")
    {
        send_error(&hub, "/subscription", respond, 400, "bad_request", None).await;
        return;
    }
    let response = Response::builder()
        .status(200)
        .header("content-type", "application/x-ndjson; charset=utf-8")
        .header("cache-control", "no-store")
        .header("codex-info-consumer", &consumer)
        .body(())
        .unwrap();
    let Ok(mut output) = respond.send_response(response, false) else {
        return;
    };
    let mut input = request.into_body();
    let mut buffer = Vec::new();
    let mut opened = false;
    let mut subscriptions = Vec::new();
    let mut changes = hub.changes.subscribe();
    let mut sent = BTreeMap::new();
    let mut outstanding = None;
    let mut notify_seq = 0u64;
    let first_deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        tokio::select! {
         _=tokio::time::sleep_until(first_deadline),if !opened=>break,
         changed=changes.changed(),if opened&&outstanding.is_none()=>{
          if changed.is_err(){break}notify_seq+=1;
          if let Some(value)=notification(&hub,&subscriptions,&mut sent,notify_seq){if record(&mut output,value).await.is_err(){break}outstanding=Some(notify_seq)}
         },
         chunk=input.data()=>{
          let Some(Ok(chunk))=chunk else{break};let size=chunk.len();
          for &byte in &chunk{buffer.push(byte);if buffer.len()>8192{return}
           if byte!=b'\n'{continue}
           let Ok(value)=serde_json::from_slice::<Value>(&buffer)else{return};buffer.clear();let Some(seq)=value["seq"].as_u64().filter(|n|*n>0)else{return};
           let kind=value["type"].as_str().unwrap_or("");let response={
            let mut peer=peer.lock().unwrap_or_else(|p|p.into_inner());if seq<=peer.seq{return}peer.seq=seq;
            match kind{
             "open" if !opened=>{if value["consumer"]!=consumer{return}let Ok(targets)=targets(&value["subscriptions"])else{return};subscriptions=targets;
              if let(Some(outbox),Some(incoming))=(hub.outbox.as_ref(),value["incoming_deliveries"].as_array()){for entry in incoming{let Some(delivery)=entry["delivery_id"].as_str()else{return};let Some(offset)=entry["offset"].as_u64()else{return};let Some(prefix)=entry["prefix_hash"].as_str()else{return};if let Ok((meta,data))=outbox.recover(&consumer,delivery,offset,prefix){if let Ok(Some(target))=Target::parse(&meta.target){if peer.restore_delivery(&meta,&target,&data).is_err(){return}}}else{return}}}

              peer.partial_receipts=serde_json::from_value(value["partial_receipts"].clone()).unwrap_or_default();peer.expected_root=value["cache_root"].as_str().unwrap_or("").into();let matched=peer.reconcile(value["cache_root"].as_str().unwrap_or(""));if matched{let incoming=value["incoming_deliveries"].as_array().map(|entries|entries.iter().filter_map(|entry|entry["delivery_id"].as_str()).collect::<BTreeSet<_>>()).unwrap_or_default();let keep=peer.deliveries.iter().filter(|(id,_)|incoming.contains(id.as_str())).map(|(id,data)|(id.clone(),data.clone())).collect::<BTreeMap<_,_>>();let retired=peer.deliveries.keys().filter(|id|!keep.contains_key(*id)).cloned().collect::<Vec<_>>();peer.offered.clear();for data in keep.values(){peer.offered.extend(data.definitions.iter().map(|d|d.id));peer.offered.extend(data.patches.iter().map(|p|p.id));peer.offered.extend(data.fragments.iter().map(|f|f.id));}peer.deliveries=keep;peer.confirmed.retain(|id,_|incoming.contains(id.as_str()));if let Some(outbox)=hub.outbox.as_ref(){for id in retired{outbox.retire(&consumer,&id);}}}opened=true;peer.active_probe=opaque_id().unwrap();
              for view in resolved(&hub,&subscriptions).as_array().unwrap(){sent.insert(view["view_id"].as_str().unwrap().into(),view["target_image"].as_str().unwrap_or("").into());}
              json!({"type":"ready","ack_seq":seq,"session":opaque_id().unwrap(),"cache_state":if matched{"matched"}else{"reconcile_required"},"views":resolved(&hub,&subscriptions)})},
             "subscribe" if opened=>{let Ok(targets)=targets(&value["targets"])else{return};hub.window(&consumer,targets.iter().find(|target|target.purpose==Some(wire::request::PeriodPurpose::Window)));subscriptions=targets;json!({"type":"ack","ack_seq":seq,"views":resolved(&hub,&subscriptions)})},
             "manifest" if opened=>{
              use base64::Engine as _;if value["probe"].as_str()!=Some(peer.active_probe.as_str()){return}let Some(nodes)=value["nodes"].as_array()else{return};
              for node in nodes{let Some(key)=node["key"].as_str()else{return};let Ok(bytes)=base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(key)else{return};let Ok(key)=serde_json::from_slice::<Value>(&bytes)else{return};
               let Some(hash)=node["hash"].as_str().and_then(parse_digest)else{return};peer.manifest.push((key,hash,node["id"].as_u64()));}
              if value["next"].is_null(){let mut next=Receiver::default();let restored=peer.restore_prefixes(&hub.source.read().unwrap(),&mut next).is_ok();let entries=std::mem::take(&mut peer.manifest);
               if !restored||next.import_manifest(&entries).is_err()||peer.bases.values().any(|base|next.materialize_alias(base).is_err())||hex(&next.root().unwrap())!=peer.expected_root{json!({"type":"recovery_required","ack_seq":seq,"reason":"cache_invalid"})}
               else{peer.receiver=next;json!({"type":"ack","ack_seq":seq})}
              }else{json!({"type":"ack","ack_seq":seq})}
             },
             "receipt" if opened=>{let id=value["delivery_id"].as_str().unwrap_or("");let through=value["complete_through"].as_u64().unwrap_or(u64::MAX);let complete=peer.deliveries.get(id).is_some_and(|data|through==data.record_count()as u64);let success=peer.receipt(id,through,value["cache_root"].as_str().unwrap_or(""));if !success{json!({"type":"recovery_required","ack_seq":seq,"reason":"cache_invalid"})}else{if complete{if let Some(outbox)=hub.outbox.as_ref(){outbox.retire(&consumer,id);}}json!({"type":"ack","ack_seq":seq})}},
             "notify_ack" if opened=>{if value["notify_seq"].as_u64()!=outstanding{return}outstanding=None;changes.mark_changed();json!({"type":"ack","ack_seq":seq})},
             "check" if opened=>{changes.mark_changed();json!({"type":"ack","ack_seq":seq})},
             "close" if opened=>{let _=output.send_data(Bytes::new(),true);return},
             _=>return,
            }
           };
           let probe=if response["type"]=="ready"&&response["cache_state"]=="reconcile_required"{let peer=peer.lock().unwrap();Some(json!({"type":"probe","probe":peer.active_probe,"keys":[],"expected_root":peer.expected_root}))}else{None};
           if record(&mut output,response).await.is_err(){return}
           if let Some(probe)=probe{if record(&mut output,probe).await.is_err(){return}}
          }
          if input.flow_control().release_capacity(size).is_err(){break}
         }
        }
    }
    output.send_reset(h2::Reason::CANCEL);
}

fn parse_digest(text: &str) -> Option<wire::Digest> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut digest = [0; 32];
    for (i, b) in digest.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?
    }
    Some(digest)
}

#[cfg(test)]
pub(super) fn notification_for_test(
    hub: &Hub,
    targets: &[Target],
    sent: &mut BTreeMap<String, String>,
    seq: u64,
) -> Option<Value> {
    notification(hub, targets, sent, seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use wire::sync::{hex, Catalog, Fragment, FragmentDescriptor};
    fn source(text: &str) -> (Arc<Source>, Target, u64) {
        let target = Target::parse("/state?parts=main").unwrap().unwrap();
        let key = target.view_key(Some(Projection::Main)).unwrap();
        let mut catalog = Catalog::with_first_id(10000);
        let root = catalog
            .intern("main", &json!({"authenticated":true,"plan_label":text}))
            .unwrap();
        let child = catalog.definitions[&root].value["plan_label"]["ref"]
            .as_u64()
            .unwrap();
        let image = hex(&wire::manifest_root(&[(
            wire::canonical_json(None, &json!(["main"])).unwrap(),
            catalog.digests[&root],
        )])
        .unwrap());
        let definitions = catalog
            .transfer(root, &Default::default())
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect();
        let row_key = wire::canonical_json(None, &json!(["main"])).unwrap();
        let mut tree = wire::manifest::Tree::default();
        tree.insert(row_key.clone(), catalog.digests[&root])
            .unwrap();
        let view = source::View {
            account: None,
            image,
            rows: vec![source::Row {
                key: json!(["main"]),
                id: root,
            }],
            definitions,
            tree,
            row_ids: [(row_key, root)].into_iter().collect(),
        };
        (
            Arc::new(Source {
                views: BTreeMap::from([(key, Arc::new(view))]),
                catalog,
            }),
            target,
            child,
        )
    }
    #[test]
    fn restart_resumes_only_committed_fragment_suffix_and_changed_source_is_normal_sync() {
        let text = "x".repeat(20000);
        let mut first = Catalog::with_first_id(20);
        let old_id = first.intern("text", &json!(text)).unwrap();
        let bytes = serde_json::to_vec(&first.definitions[&old_id]).unwrap();
        let (current, target, child) = source(&text);
        let key = target.view_key(Some(Projection::Main)).unwrap();
        let view_id = hex(&Sha256::digest(&key));
        let image = current.views[&key].image.clone();
        let partial = Data {
            result: "data".into(),
            delivery_id: "first-delivery".into(),
            definitions: vec![],
            patches: vec![],
            changes: vec![],
            fragments: vec![Fragment {
                descriptor: Some(FragmentDescriptor {
                    definition_kind: "text".into(),
                    value_hash: hex(&current.catalog.digests[&child]),
                    view_id: view_id.clone(),
                    target_image: image,
                }),
                id: old_id,
                kind: "definition".into(),
                total_bytes: bytes.len() as u64,
                offset: 0,
                content_hash: hex(&Sha256::digest(&bytes)),
                data: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes[..8192]),
                last: false,
            }],
            completed_views: vec![],
            next_cursor: Some("old-cursor".into()),
        };
        let mut receiver = Receiver::default();
        receiver.apply(&partial).unwrap();
        assert_eq!(receiver.partials[&old_id].bytes.len(), 8192);
        let manifest = receiver.manifest().unwrap();
        let mut peer = Peer {
            manifest: manifest.clone(),
            partial_receipts: receiver.partial_receipts(),
            ..Peer::default()
        };
        let mut reconstructed = Receiver::default();
        peer.restore_prefixes(&current, &mut reconstructed).unwrap();
        reconstructed.import_manifest(&manifest).unwrap();
        assert_eq!(reconstructed.root().unwrap(), receiver.root().unwrap());
        peer.receiver = reconstructed;
        let root = hex(&receiver.root().unwrap());
        let body = peer
            .data(&target, &current, current.selected(&target).unwrap(), &root)
            .unwrap();
        let data: Data = serde_json::from_slice(&body).unwrap();
        assert!(!data.fragments.is_empty());
        assert_eq!(data.fragments[0].id, old_id);
        assert_eq!(data.fragments[0].offset, 8192);
        assert!(data.fragments.iter().all(|f| f.offset >= 8192));
        receiver.register_target(&target, &data).unwrap();
        receiver.apply(&data).unwrap();
        let root_id = receiver.views[&view_id].bindings.values().next().unwrap();
        assert_eq!(receiver.resolve(*root_id).unwrap()["plan_label"], text);
        assert!(peer.receipt(
            &data.delivery_id,
            data.record_count() as u64,
            &hex(&receiver.root().unwrap())
        ));
        let (changed, target, _) = source(&"y".repeat(20000));
        let mut peer = Peer {
            manifest: manifest.clone(),
            partial_receipts: partial_receipts(&partial),
            ..Peer::default()
        };
        let mut reconstructed = Receiver::default();
        peer.restore_prefixes(&changed, &mut reconstructed).unwrap();
        reconstructed.import_manifest(&manifest).unwrap();
        peer.receiver = reconstructed;
        let root = hex(&peer.receiver.root().unwrap());
        let response: Value = serde_json::from_slice(
            &peer
                .data(&target, &changed, changed.selected(&target).unwrap(), &root)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(response["result"], "sync_required");
        assert_eq!(response["reason"], "image_changed");
        assert!(response.get("definitions").is_none());
        let mut client = Receiver::default();
        client.apply(&partial).unwrap();
        client.discard_partials(&view_id);
        let root = hex(&client.root().unwrap());
        let response: Value = serde_json::from_slice(
            &peer
                .data(&target, &changed, changed.selected(&target).unwrap(), &root)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(response["result"], "data");
    }
    #[test]
    fn parked_cursor_skips_dependency_received_from_another_resource() {
        let (source, mut target, shared) = source("shared body");
        let mut peer = Peer::default();
        let mut receiver = Receiver::default();
        peer.offered.insert(shared);
        peer.protected.insert(source.catalog.digests[&shared]);
        let first: Data = serde_json::from_slice(
            &peer
                .data(
                    &target,
                    &source,
                    source.selected(&target).unwrap(),
                    &hex(&receiver.root().unwrap()),
                )
                .unwrap(),
        )
        .unwrap();
        assert!(!first.definitions.is_empty());
        assert!(first.completed_views.is_empty());
        receiver.register_target(&target, &first).unwrap();
        receiver.apply(&first).unwrap();
        assert!(peer.receipt(
            &first.delivery_id,
            first.record_count() as u64,
            &hex(&receiver.root().unwrap())
        ));
        let received = Data {
            result: "data".into(),
            delivery_id: "other-issued-resource".into(),
            definitions: vec![source.catalog.definitions[&shared].clone()],
            patches: vec![],
            fragments: vec![],
            changes: vec![],
            completed_views: vec![],
            next_cursor: None,
        };
        peer.deliveries
            .insert(received.delivery_id.clone(), received.clone());
        receiver.apply(&received).unwrap();
        assert!(peer.receipt(
            &received.delivery_id,
            received.record_count() as u64,
            &hex(&receiver.root().unwrap())
        ));
        target.cursor = first.next_cursor;
        let next: Data = serde_json::from_slice(
            &peer
                .data(
                    &target,
                    &source,
                    source.selected(&target).unwrap(),
                    &hex(&receiver.root().unwrap()),
                )
                .unwrap(),
        )
        .unwrap();
        assert!(next.next_cursor.is_none());
        assert_eq!(next.completed_views.len(), 1);
        assert!(next
            .definitions
            .iter()
            .all(|definition| definition.id != shared));
        receiver.register_target(&target, &next).unwrap();
        receiver.apply(&next).unwrap();
        let view = receiver.views.values().next().unwrap();
        assert_eq!(
            receiver
                .resolve(*view.bindings.values().next().unwrap())
                .unwrap()["plan_label"],
            "shared body"
        );
    }
    fn partial_receipts(data: &Data) -> Vec<wire::sync::PartialReceipt> {
        let mut receiver = Receiver::default();
        receiver.apply(data).unwrap();
        receiver.partial_receipts()
    }
    #[test]
    fn receipt_ordinals_are_collections_not_json_property_order() {
        let mut catalog = Catalog::default();
        let id = catalog.intern("main", &json!({"quota":null})).unwrap();
        let data = Data {
            result: "data".into(),
            delivery_id: "ordered".into(),
            definitions: catalog.transfer(id, &Default::default()).unwrap(),
            patches: vec![],
            fragments: vec![],
            changes: vec![],
            completed_views: vec![],
            next_cursor: None,
        };
        let mut peer = Peer::default();
        peer.deliveries
            .insert(data.delivery_id.clone(), data.clone());
        let mut client = Receiver::default();
        client.apply(&data.ordinal_range(0, 1).unwrap()).unwrap();
        assert!(peer.receipt("ordered", 1, &hex(&client.root().unwrap())));
        assert_eq!(peer.receiver.definitions.len(), 1);
        client
            .apply(&data.ordinal_range(1, data.record_count()).unwrap())
            .unwrap();
        assert!(peer.receipt(
            "ordered",
            data.record_count() as u64,
            &hex(&client.root().unwrap())
        ));
        assert_eq!(peer.receiver.root().unwrap(), client.root().unwrap());
    }
    #[test]
    fn first_thread_page_does_not_transfer_bodies_for_later_rows() {
        let target = Target::parse("/threads?account=account-1")
            .unwrap()
            .unwrap();
        let mut catalog = wire::sync::Catalog::default();
        let mut rows = Vec::new();
        let mut definitions = Vec::new();
        let mut seen = BTreeSet::new();
        let mut tree = wire::manifest::Tree::default();
        let mut row_ids = BTreeMap::new();
        for i in 0..257u64 {
            let key = json!(["thread", format!("thread-{i:04}")]);
            let id=catalog.intern("thread",&json!({"title":format!("row {i}"),"activity_status":"running","parent_thread_id":null,"model":"gpt-6.1-sol","model_label":"6.1 Sol","total_tokens":i,"context_usage_tokens":1,"context_window_tokens":100,"created_at":1800000000,"last_user_message_at":1800000000,"is_subagent":false,"depth":0})).unwrap();
            let bytes = wire::canonical_json(None, &key).unwrap();
            tree.insert(bytes.clone(), catalog.digests[&id]).unwrap();
            row_ids.insert(bytes, id);
            for d in catalog.transfer(id, &seen).unwrap() {
                seen.insert(d.id);
                definitions.push(d.id);
            }
            rows.push(super::source::Row { key, id });
        }
        let key = target.view_key(None).unwrap();
        let view = Arc::new(View {
            account: Some("account-1".into()),
            image: hex(&tree.root()),
            rows,
            definitions,
            tree,
            row_ids,
        });
        let source = Arc::new(Source {
            views: [(key.clone(), view)].into_iter().collect(),
            catalog,
        });
        let mut peer = Peer::default();
        let mut receiver = Receiver::default();
        let first: Data = serde_json::from_slice(
            &peer
                .data(
                    &target,
                    &source,
                    source.selected(&target).unwrap(),
                    &hex(&receiver.root().unwrap()),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(first.changes[0].sets.len(), 256);
        assert_eq!(
            first
                .definitions
                .iter()
                .filter(|d| d.kind == "thread")
                .count(),
            256,
            "page one cannot transfer body of the 257th invisible row"
        );
        assert!(first.completed_views.is_empty());
        receiver.register_target(&target, &first).unwrap();
        receiver.apply(&first).unwrap();
        assert!(receiver.views.is_empty());
        assert!(peer.receipt(
            &first.delivery_id,
            first.record_count() as u64,
            &hex(&receiver.root().unwrap())
        ));
        let mut next = target.clone();
        next.cursor = first.next_cursor;
        assert!(next.cursor.is_some());
        let final_page: Data = serde_json::from_slice(
            &peer
                .data(
                    &next,
                    &source,
                    source.selected(&next).unwrap(),
                    &hex(&receiver.root().unwrap()),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(final_page.changes[0].sets.len(), 1);
        assert_eq!(
            final_page
                .definitions
                .iter()
                .filter(|d| d.kind == "thread")
                .count()
                + final_page
                    .patches
                    .iter()
                    .filter(|d| d.kind == "thread")
                    .count(),
            1
        );
        assert!(final_page.next_cursor.is_none());
        receiver.register_target(&target, &final_page).unwrap();
        receiver.apply(&final_page).unwrap();
        assert_eq!(receiver.views.values().next().unwrap().bindings.len(), 257);
    }
}

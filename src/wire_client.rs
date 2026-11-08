// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only
//! One HTTP/2 connection, shared durable receipts, and change-driven UI reads.
use bytes::Bytes;
use codex_info_rest_contract::wire::{
    self,
    cache::Cache,
    request::{Projection, Resource, Target},
    sync::{hex, Data},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, net::SocketAddr, path::Path, sync::Arc, time::Duration};
use tokio::sync::{watch, Mutex, Semaphore};

#[derive(Clone, Debug)]
pub enum Failure {
    Transport,
    Http(u16),
    Invalid,
    Pending,
    Sync,
    Cache(String),
}
pub type Result<T> = std::result::Result<T, Failure>;
#[derive(Clone, Debug)]
pub struct Page {
    pub data: Option<Data>,
    pub views: BTreeMap<String, (String, Vec<(Value, Value)>)>,
}
struct Subscription {
    output: h2::SendStream<Bytes>,
    ready: watch::Receiver<bool>,
    ready_sender: watch::Sender<bool>,
}
type InFlight = watch::Receiver<Option<Result<Page>>>;
pub struct Client {
    sender: Mutex<h2::client::SendRequest<Bytes>>,
    cache: Arc<std::sync::Mutex<Cache>>,
    address: SocketAddr,
    cache_path: std::path::PathBuf,
    initial_deadline: std::sync::Mutex<Option<tokio::time::Instant>>,
    terminal_failure: std::sync::Mutex<Option<Failure>>,
    subscription: Mutex<Option<Subscription>>,
    receipts: Mutex<()>,
    identity: Mutex<()>,
    control: Mutex<()>,
    targets: std::sync::Mutex<BTreeMap<String, Target>>,
    disconnected: std::sync::atomic::AtomicBool,
    changes: watch::Sender<u64>,
    dirty: std::sync::Mutex<BTreeMap<String, String>>,
    slots: Semaphore,
    flights: Mutex<BTreeMap<Vec<u8>, InFlight>>,
    acks: Mutex<BTreeMap<u64, tokio::sync::oneshot::Sender<()>>>,
}
impl Client {
    pub async fn connect(address: SocketAddr, path: &Path) -> Result<Arc<Self>> {
        if !address.ip().is_loopback() {
            return Err(Failure::Invalid);
        }
        let restore_started = std::time::Instant::now();
        let cache = Cache::open(path).map_err(|e| Failure::Cache(e.to_string()))?;
        if std::env::var_os("CODEX_INFO_DEBUG").is_some() {
            eprintln!(
                "[codex-info] wire cache restore elapsed_ms={}",
                restore_started.elapsed().as_millis()
            );
        }
        // Receiver qualification is local startup preparation, before any
        // HTTP operation. The UI owner reports preparing while it runs.
        // Connection, subscription and the first GET share this one budget.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let socket = tokio::time::timeout_at(deadline, tokio::net::TcpStream::connect(address))
            .await
            .map_err(|_| Failure::Transport)?
            .map_err(|_| Failure::Transport)?;
        let mut builder = h2::client::Builder::new();
        builder
            .initial_window_size(16384)
            .initial_connection_window_size(8 * 16384)
            .max_frame_size(16384)
            .max_header_list_size(8192);
        let (sender, connection) = tokio::time::timeout_at(deadline, builder.handshake(socket))
            .await
            .map_err(|_| Failure::Transport)?
            .map_err(|_| Failure::Transport)?;
        let (changes, _) = watch::channel(0);
        let client = Arc::new(Self {
            sender: Mutex::new(sender),
            cache: Arc::new(std::sync::Mutex::new(cache)),
            address,
            cache_path: path.into(),
            initial_deadline: std::sync::Mutex::new(Some(deadline)),
            terminal_failure: std::sync::Mutex::new(None),
            subscription: Mutex::new(None),
            receipts: Mutex::new(()),
            identity: Mutex::new(()),
            control: Mutex::new(()),
            targets: std::sync::Mutex::new(BTreeMap::new()),
            disconnected: std::sync::atomic::AtomicBool::new(false),
            changes,
            dirty: std::sync::Mutex::new(BTreeMap::new()),
            slots: Semaphore::new(7),
            flights: Mutex::new(BTreeMap::new()),
            acks: Mutex::new(BTreeMap::new()),
        });
        let weak = Arc::downgrade(&client);
        tokio::spawn(async move {
            let _ = connection.await;
            if let Some(client) = weak.upgrade() {
                client
                    .disconnected
                    .store(true, std::sync::atomic::Ordering::Release);
                client.wake();
            }
        });
        let established = client.cache.lock().unwrap().consumer.is_some();
        if established {
            client.ensure_subscription(deadline).await?;
            for target in wire::incoming::Incoming::targets(path)
                .map_err(|e| Failure::Cache(e.to_string()))?
            {
                let client = client.clone();
                tokio::spawn(async move {
                    client.resume_incoming(target).await;
                });
            }
        }
        Ok(client)
    }
    pub async fn check(self: &Arc<Self>) -> Result<()> {
        if self.cache.lock().unwrap().consumer.is_none() {
            return Ok(());
        }
        let deadline = self
            .initial_deadline
            .lock()
            .unwrap()
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3));
        self.ensure_subscription(deadline).await?;
        self.send_control(json!({"type":"check"}), deadline).await
    }
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }
    pub fn receiver(&self) -> wire::sync::Receiver {
        self.cache.lock().unwrap().receiver.clone()
    }
    fn wake(&self) {
        self.changes.send_modify(|n| *n += 1)
    }
    fn key(target: &Target, part: Option<Projection>) -> String {
        String::from_utf8(target.view_key(part).unwrap()).unwrap()
    }
    pub async fn set_targets(self: &Arc<Self>, raw: &[String]) -> Result<()> {
        let mut targets = BTreeMap::new();
        for raw in raw {
            let target = Target::parse(raw)
                .map_err(|_| Failure::Invalid)?
                .ok_or(Failure::Invalid)?;
            if target.resource == Resource::Subscription {
                return Err(Failure::Invalid);
            }
            if target.resource == Resource::State {
                for part in &target.parts {
                    let mut single = target.clone();
                    single.parts = [*part].into_iter().collect();
                    targets.insert(Self::key(&single, Some(*part)), single);
                }
            } else {
                targets.insert(Self::key(&target, None), target);
            }
        }
        if targets.len() > 7 {
            return Err(Failure::Invalid);
        }
        {
            let mut old = self.targets.lock().unwrap();
            if old.keys().eq(targets.keys()) {
                return Ok(());
            }
            *old = targets;
        }
        if self.subscription.lock().await.is_some() {
            let targets = self.subscription_targets();
            let deadline = self
                .initial_deadline
                .lock()
                .unwrap()
                .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3));
            self.send_control(json!({"type":"subscribe","targets":targets}), deadline)
                .await?;
            if std::env::var_os("CODEX_INFO_DEBUG").is_some() {
                eprintln!("[codex-info] wire subscriptions accepted: {}", raw.len());
            }
        }
        Ok(())
    }
    fn subscription_targets(&self) -> Vec<Value> {
        let cache = self.cache.lock().unwrap();
        self.targets.lock().unwrap().values().map(|target|{
   let part=target.parts.first().copied();let key=serde_json::from_slice::<Value>(&target.view_key(part).unwrap()).unwrap();let id=cache.receiver.view_keys.iter().find(|(_,v)|**v==key).map(|(id,_)|id);
   json!({"view_id":id,"path":target.resource.path(),"query":key[1].as_object().unwrap().iter().filter(|(_,v)|!v.is_null()).map(|(k,v)|(k.clone(),v.clone())).collect::<serde_json::Map<_,_>>(),"accepted_image":id.and_then(|id|cache.receiver.views.get(id)).map(|v|&v.image)})
  }).collect()
    }
    pub async fn get(self: &Arc<Self>, raw: &str) -> Result<Page> {
        let deadline = self
            .initial_deadline
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3));
        self.get_until(raw, deadline).await
    }
    async fn resume_incoming(self: Arc<Self>, target: String) {
        let mut changes = self.changes();
        loop {
            match self
                .get_until(
                    &target,
                    tokio::time::Instant::now() + Duration::from_secs(3),
                )
                .await
            {
                Ok(_) => return,
                Err(Failure::Pending | Failure::Sync) => {
                    if changes.changed().await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    if !matches!(error, Failure::Transport) {
                        *self.terminal_failure.lock().unwrap() = Some(error);
                    } else {
                        self.disconnected
                            .store(true, std::sync::atomic::Ordering::Release);
                    }
                    self.wake();
                    return;
                }
            }
        }
    }
    async fn get_until(
        self: &Arc<Self>,
        raw: &str,
        deadline: tokio::time::Instant,
    ) -> Result<Page> {
        let target = Target::parse(raw)
            .map_err(|_| Failure::Invalid)?
            .ok_or(Failure::Invalid)?;
        if target.resource == Resource::Subscription {
            return Err(Failure::Invalid);
        }
        let state = target.resource == Resource::State && target.cursor.is_none();
        let keys = if state {
            target
                .parts
                .iter()
                .map(|p| target.view_key(Some(*p)).unwrap())
                .collect::<Vec<_>>()
        } else {
            vec![raw.as_bytes().to_vec()]
        };
        let mut flights = self.flights.lock().await;
        let mut waits = Vec::new();
        let mut owned = Vec::new();
        let mut missing = std::collections::BTreeSet::new();
        let (sender, receiver) = watch::channel(None);
        for key in keys {
            if let Some(existing) = flights.get(&key) {
                waits.push(existing.clone())
            } else {
                if state {
                    let part = target
                        .parts
                        .iter()
                        .find(|p| target.view_key(Some(**p)).unwrap() == key)
                        .copied()
                        .ok_or(Failure::Invalid)?;
                    missing.insert(part);
                }
                flights.insert(key.clone(), receiver.clone());
                owned.push(key);
            }
        }
        if !owned.is_empty() {
            waits.push(receiver);
            let client = Arc::clone(self);
            let request = if state {
                let parts = missing
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                let account = target
                    .account
                    .as_ref()
                    .filter(|_| missing.iter().any(|p| p.account_scoped()))
                    .map(|id| format!("&account={id}"))
                    .unwrap_or_default();
                format!("/state?parts={parts}{account}")
            } else {
                raw.to_owned()
            };
            tokio::spawn(async move {
                let result = tokio::time::timeout_at(deadline, client.get_once(&request, deadline))
                    .await
                    .unwrap_or(Err(Failure::Transport));
                sender.send_replace(Some(result));
                let mut flights = client.flights.lock().await;
                for key in owned {
                    flights.remove(&key);
                }
            });
        }
        drop(flights);
        let mut output = None;
        for mut result in waits {
            if result.borrow().is_none() {
                tokio::time::timeout_at(deadline, result.changed())
                    .await
                    .map_err(|_| Failure::Transport)?
                    .map_err(|_| Failure::Transport)?;
            }
            output = Some(result.borrow().clone().ok_or(Failure::Transport)??);
        }
        if state {
            self.page(&target, output.and_then(|p| p.data))
        } else {
            output.ok_or(Failure::Transport)
        }
    }
    fn page(&self, target: &Target, data: Option<Data>) -> Result<Page> {
        let cache = self.cache.lock().unwrap();
        let mut views = BTreeMap::new();
        for (id, key) in &cache.receiver.view_keys {
            let target_keys = if target.resource == Resource::State {
                target
                    .parts
                    .iter()
                    .map(|p| target.view_key(Some(*p)).unwrap())
                    .collect::<Vec<_>>()
            } else {
                vec![target.view_key(None).unwrap()]
            };
            if !target_keys
                .iter()
                .any(|expected| wire::canonical_json(None, key).unwrap() == *expected)
            {
                continue;
            }
            if let Some(view) = cache.receiver.views.get(id) {
                let rows = view
                    .bindings
                    .iter()
                    .map(|(key, id)| {
                        Ok((
                            serde_json::from_str(key).map_err(|_| Failure::Invalid)?,
                            cache.receiver.resolve(*id).map_err(|_| Failure::Invalid)?,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                views.insert(id.clone(), (view.image.clone(), rows));
            }
        }
        Ok(Page { data, views })
    }
    async fn get_once(self: &Arc<Self>, raw: &str, deadline: tokio::time::Instant) -> Result<Page> {
        if let Some(error) = self.terminal_failure.lock().unwrap().clone() {
            return Err(error);
        }
        if self.disconnected.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Failure::Transport);
        }
        let original = Target::parse(raw)
            .map_err(|_| Failure::Invalid)?
            .ok_or(Failure::Invalid)?;
        let continued = if original.cursor.is_none() {
            let key = original.transfer_key().map_err(|_| Failure::Invalid)?;
            self.cache
                .lock()
                .unwrap()
                .pending_cursors
                .get(&key)
                .map(|cursor| format!("{raw}&cursor={cursor}"))
        } else {
            None
        };
        let raw = continued.as_deref().unwrap_or(raw);
        let target = Target::parse(raw)
            .map_err(|_| Failure::Invalid)?
            .ok_or(Failure::Invalid)?;
        let mut identity = if self.cache.lock().unwrap().consumer.is_none() {
            Some(
                tokio::time::timeout_at(deadline, self.identity.lock())
                    .await
                    .map_err(|_| Failure::Transport)?,
            )
        } else {
            None
        };
        let established = self.cache.lock().unwrap().consumer.is_some();
        if established {
            self.ensure_subscription(deadline).await?;
            let sub = self.subscription.lock().await;
            if sub.as_ref().is_some_and(|s| !*s.ready.borrow()) {
                if std::env::var_os("CODEX_INFO_DEBUG").is_some() {
                    eprintln!("[codex-info] wire pending: subscription_reconcile");
                }
                return Err(Failure::Pending);
            }
        }
        let cached = self.page(&target, None)?;
        let expected = if target.resource == Resource::State {
            target.parts.len()
        } else {
            1
        };
        if target.cursor.is_none()
            && cached.views.len() == expected
            && cached.views.iter().all(|(id, (image, _))| {
                self.dirty
                    .lock()
                    .unwrap()
                    .get(id)
                    .is_none_or(|target| target == image)
            })
        {
            return Ok(cached);
        }
        let _slot = tokio::time::timeout_at(deadline, self.slots.acquire())
            .await
            .map_err(|_| Failure::Transport)?
            .map_err(|_| Failure::Transport)?;
        let (consumer, root) = {
            let cache = self.cache.lock().unwrap();
            (
                cache.consumer.clone(),
                hex(&cache.receiver.root().map_err(|_| Failure::Invalid)?),
            )
        };
        let budget = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis()
            .min(3000) as u64;
        if budget == 0 {
            return Err(Failure::Transport);
        }
        let mut request = http::Request::builder()
            .uri(format!("http://{}{raw}", self.address))
            .header("codex-info-budget-ms", budget)
            .header("codex-info-cache-root", root);
        if let Some(id) = consumer {
            request = request.header("codex-info-consumer", id)
        }
        let incoming = wire::incoming::Incoming::load(&self.cache_path, raw)
            .map_err(|e| Failure::Cache(e.to_string()))?;
        if let Some(partial) = incoming.as_ref() {
            request = request
                .header("codex-info-delivery", &partial.header.delivery)
                .header("codex-info-offset", partial.offset)
                .header("codex-info-prefix-sha256", &partial.prefix);
        }
        let (response, _) = self
            .sender
            .lock()
            .await
            .send_request(request.body(()).map_err(|_| Failure::Invalid)?, true)
            .map_err(|_| Failure::Transport)?;
        let response = response.await.map_err(|_| Failure::Transport)?;
        if response.status() == 204 {
            return self.page(&target, None);
        }
        if response.status() != 200 {
            return Err(Failure::Http(response.status().as_u16()));
        }
        let consumer = response
            .headers()
            .get("codex-info-consumer")
            .and_then(|v| v.to_str().ok())
            .ok_or(Failure::Invalid)?
            .to_owned();
        self.cache
            .lock()
            .unwrap()
            .issue(&consumer)
            .map_err(|e| Failure::Cache(e.to_string()))?;
        let mut incoming = if let Some(delivery) = response
            .headers()
            .get("codex-info-delivery")
            .and_then(|v| v.to_str().ok())
        {
            let parse = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .ok_or(Failure::Invalid)
            };
            let offset = parse("codex-info-offset")?;
            let length = parse("codex-info-total-bytes")?;
            let hash = response
                .headers()
                .get("codex-info-body-sha256")
                .and_then(|v| v.to_str().ok())
                .ok_or(Failure::Invalid)?
                .into();
            let created = wire::incoming::Incoming::begin(
                &self.cache_path,
                wire::incoming::Header {
                    target: raw.into(),
                    delivery: delivery.into(),
                    consumer: consumer.clone(),
                    length,
                    hash,
                },
            )
            .map_err(|e| Failure::Cache(e.to_string()))?;
            if created.offset != offset {
                return Err(Failure::Invalid);
            }
            Some(created)
        } else {
            if incoming.is_some() {
                return Err(Failure::Cache("response_prefix_missing".into()));
            }
            None
        };
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.map_err(|_| Failure::Transport)?;
            if let Some(incoming) = incoming.as_mut() {
                incoming
                    .append(&chunk)
                    .map_err(|e| Failure::Cache(e.to_string()))?;
            } else {
                if bytes.len() + chunk.len() > 33_554_432 {
                    return Err(Failure::Invalid);
                }
                bytes.extend_from_slice(&chunk);
            }
            body.flow_control()
                .release_capacity(chunk.len())
                .map_err(|_| Failure::Transport)?;
        }
        if let Some(incoming) = incoming.as_ref() {
            bytes = incoming
                .bytes()
                .map_err(|e| Failure::Cache(e.to_string()))?;
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Invalid)?;
        match value["result"].as_str() {
            Some("pending") => {
                if std::env::var_os("CODEX_INFO_DEBUG").is_some() {
                    eprintln!("[codex-info] wire pending source: {}", value["reason"]);
                }
                self.ensure_subscription(deadline).await?;
                return Err(Failure::Pending);
            }
            Some("sync_required") => {
                if value["reason"] == "cursor_outdated" {
                    self.cache
                        .lock()
                        .unwrap()
                        .clear_cursor(&target)
                        .map_err(|e| Failure::Cache(e.to_string()))?;
                    self.wake();
                }
                if value["reason"] == "image_changed" {
                    if let Some(view) = value["view_id"].as_str() {
                        self.cache
                            .lock()
                            .unwrap()
                            .discard_partials(view)
                            .map_err(|e| Failure::Cache(e.to_string()))?;
                        self.wake();
                    }
                }
                return Err(Failure::Sync);
            }
            Some("cache_recovery_required") => {
                return Err(Failure::Cache(
                    value["reason"].as_str().unwrap_or("cache_invalid").into(),
                ))
            }
            Some("data") => {}
            _ => return Err(Failure::Invalid),
        }
        let data: Data = serde_json::from_value(value).map_err(|_| Failure::Invalid)?;
        let _receipt = tokio::time::timeout_at(deadline, self.receipts.lock())
            .await
            .map_err(|_| Failure::Transport)?;
        self.cache
            .lock()
            .unwrap()
            .commit(&target, &data)
            .map_err(|error| match error {
                wire::cache::CacheError::Wire(_) => Failure::Invalid,
                error => Failure::Cache(error.to_string()),
            })?;
        if let Some(incoming) = incoming {
            incoming
                .finish()
                .map_err(|e| Failure::Cache(e.to_string()))?;
        }
        identity.take();
        for id in &data.completed_views {
            let cache = self.cache.lock().unwrap();
            let mut dirty = self.dirty.lock().unwrap();
            if dirty.get(id).is_none_or(|wanted| {
                cache
                    .receiver
                    .views
                    .get(id)
                    .is_some_and(|view| &view.image == wanted)
            }) {
                dirty.remove(id);
            }
        }
        self.wake();
        let fresh = self.subscription.lock().await.is_none();
        self.ensure_subscription(deadline).await?;
        if !fresh {
            let root = hex(&self
                .cache
                .lock()
                .unwrap()
                .receiver
                .root()
                .map_err(|_| Failure::Invalid)?);
            self.send_control(json!({"type":"receipt","delivery_id":data.delivery_id,"complete_through":data.record_count(),"partial":null,"cache_root":root}),deadline).await?;
        }
        self.page(&target, Some(data))
    }
    async fn ensure_subscription(self: &Arc<Self>, deadline: tokio::time::Instant) -> Result<()> {
        let mut subscription = tokio::time::timeout_at(deadline, self.subscription.lock())
            .await
            .map_err(|_| Failure::Transport)?;
        if subscription.is_some() {
            return Ok(());
        }
        let (consumer, root, max_id, partials) = {
            let cache = self.cache.lock().unwrap();
            (
                cache.consumer.clone().ok_or(Failure::Invalid)?,
                hex(&cache.receiver.root().map_err(|_| Failure::Invalid)?),
                cache
                    .receiver
                    .definitions
                    .keys()
                    .max()
                    .copied()
                    .unwrap_or(0),
                cache.receiver.partial_receipts(),
            )
        };
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://{}/subscription", self.address))
            .header("content-type", "application/x-ndjson; charset=utf-8")
            .header("codex-info-consumer", &consumer)
            .header("codex-info-cache-root", &root)
            .header(
                "codex-info-budget-ms",
                deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_millis()
                    .clamp(1, 3000) as u64,
            )
            .body(())
            .unwrap();
        let (response, mut output) = self
            .sender
            .lock()
            .await
            .send_request(request, false)
            .map_err(|_| Failure::Transport)?;
        let seq = self
            .cache
            .lock()
            .unwrap()
            .next_seq()
            .map_err(|e| Failure::Cache(e.to_string()))?;
        send_record(&mut output,&json!({"type":"open","seq":seq,"consumer":consumer,"cache_root":root,"max_definition_id":max_id,"subscriptions":self.subscription_targets(),"partial_receipts":partials,"incoming_deliveries":wire::incoming::Incoming::receipts(&self.cache_path).map_err(|e|Failure::Cache(e.to_string()))?}),deadline).await?;
        let response = tokio::time::timeout_at(deadline, response)
            .await
            .map_err(|_| Failure::Transport)?
            .map_err(|_| Failure::Transport)?;
        if response.status() != 200 {
            return Err(Failure::Http(response.status().as_u16()));
        }
        let (ready, wait) = watch::channel(false);
        *subscription = Some(Subscription {
            output,
            ready: wait,
            ready_sender: ready.clone(),
        });
        drop(subscription);
        let client = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = client
                .clone()
                .read_notifications(response.into_body(), ready)
                .await
            {
                if !matches!(error, Failure::Transport) {
                    *client.terminal_failure.lock().unwrap() = Some(error);
                }
            }
            client
                .disconnected
                .store(true, std::sync::atomic::Ordering::Release);
            client.acks.lock().await.clear();
            client.wake();
        });
        Ok(())
    }
    async fn send_control(&self, value: Value, deadline: tokio::time::Instant) -> Result<()> {
        self.control_record(value, deadline, true).await
    }
    async fn control_record(
        &self,
        mut value: Value,
        deadline: tokio::time::Instant,
        wait: bool,
    ) -> Result<()> {
        let control = tokio::time::timeout_at(deadline, self.control.lock())
            .await
            .map_err(|_| Failure::Transport)?;
        let seq = self
            .cache
            .lock()
            .unwrap()
            .next_seq()
            .map_err(|e| Failure::Cache(e.to_string()))?;
        value["seq"] = json!(seq);
        let (tx, rx) = tokio::sync::oneshot::channel();
        if wait {
            self.acks.lock().await.insert(seq, tx);
        }
        {
            let mut sub = tokio::time::timeout_at(deadline, self.subscription.lock())
                .await
                .map_err(|_| Failure::Transport)?;
            let sub = sub.as_mut().ok_or(Failure::Transport)?;
            send_record(&mut sub.output, &value, deadline).await?;
        }
        drop(control);
        if wait {
            let outcome = tokio::time::timeout_at(deadline, rx).await;
            self.acks.lock().await.remove(&seq);
            outcome
                .map_err(|_| Failure::Transport)?
                .map_err(|_| Failure::Transport)?;
        }
        Ok(())
    }
    async fn read_notifications(
        self: Arc<Self>,
        mut input: h2::RecvStream,
        ready: watch::Sender<bool>,
    ) -> Result<()> {
        let mut buffer = Vec::new();
        while let Some(chunk) = input.data().await {
            let chunk = chunk.map_err(|_| Failure::Transport)?;
            let size = chunk.len();
            for &byte in &chunk {
                buffer.push(byte);
                if buffer.len() > 8192 {
                    return Err(Failure::Invalid);
                }
                if byte != b'\n' {
                    continue;
                }
                let value =
                    serde_json::from_slice::<Value>(&buffer).map_err(|_| Failure::Invalid)?;
                buffer.clear();
                match value["type"].as_str() {
                    Some("ready") => {
                        if let Some(views) = value["views"].as_array() {
                            let cache = self.cache.lock().unwrap();
                            let mut dirty = self.dirty.lock().unwrap();
                            for view in views {
                                if let (Some(id), Some(image)) = (
                                    view["view_id"].as_str(),
                                    view["target_image"].as_str().or(Some("")),
                                ) {
                                    if cache
                                        .receiver
                                        .views
                                        .get(id)
                                        .is_none_or(|v| v.image != image)
                                    {
                                        dirty.insert(id.into(), image.into());
                                    }
                                }
                            }
                        }
                        ready.send_replace(value["cache_state"] == "matched");
                        self.wake();
                    }
                    Some("changed") => {
                        if let Some(views) = value["views"].as_array() {
                            for view in views {
                                if let (Some(id), Some(image)) = (
                                    view["view_id"].as_str(),
                                    view["target_image"].as_str().or(Some("")),
                                ) {
                                    self.dirty.lock().unwrap().insert(id.into(), image.into());
                                }
                            }
                        }
                        let _ = self
                            .control_record(
                                json!({"type":"notify_ack","notify_seq":value["notify_seq"]}),
                                tokio::time::Instant::now() + Duration::from_secs(3),
                                false,
                            )
                            .await;
                        self.wake();
                    }
                    Some("probe") => {
                        let client = Arc::clone(&self);
                        let probe = value.clone();
                        tokio::spawn(async move {
                            if let Err(error) = client.send_manifest(probe).await {
                                *client.terminal_failure.lock().unwrap() = Some(error);
                                client.wake();
                            }
                        });
                    }
                    Some("ack") => {
                        if let Some(views) = value["views"].as_array() {
                            let cache = self.cache.lock().unwrap();
                            let mut dirty = self.dirty.lock().unwrap();
                            for view in views {
                                if let (Some(id), Some(image)) = (
                                    view["view_id"].as_str(),
                                    view["target_image"].as_str().or(Some("")),
                                ) {
                                    if cache
                                        .receiver
                                        .views
                                        .get(id)
                                        .is_none_or(|v| v.image != image)
                                    {
                                        dirty.insert(id.into(), image.into());
                                    }
                                }
                            }
                        }
                        if let Some(seq) = value["ack_seq"].as_u64() {
                            if let Some(sender) = self.acks.lock().await.remove(&seq) {
                                let _ = sender.send(());
                            }
                        }
                    }
                    Some("recovery_required") => {
                        return Err(Failure::Cache(
                            value["reason"].as_str().unwrap_or("cache_invalid").into(),
                        ))
                    }
                    _ => return Err(Failure::Invalid),
                }
            }
            if input.flow_control().release_capacity(size).is_err() {
                break;
            }
        }
        Err(Failure::Transport)
    }
    async fn send_manifest(&self, value: Value) -> Result<()> {
        let entries = self
            .cache
            .lock()
            .unwrap()
            .receiver
            .manifest()
            .map_err(|_| Failure::Invalid)?;
        let mut nodes = Vec::new();
        for (key, hash, id) in entries {
            use base64::Engine as _;
            let node = json!({"key":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(wire::canonical_json(None,&key).map_err(|_|Failure::Invalid)?),"hash":hex(&hash),"id":id});
            nodes.push(node);
            if serde_json::to_vec(&nodes)
                .map_err(|_| Failure::Invalid)?
                .len()
                > 6500
            {
                self.send_control(
                    json!({"type":"manifest","probe":value["probe"],"nodes":nodes,"next":"more"}),
                    tokio::time::Instant::now() + Duration::from_secs(3),
                )
                .await?;
                nodes = Vec::new();
            }
        }
        self.send_control(
            json!({"type":"manifest","probe":value["probe"],"nodes":nodes,"next":null}),
            tokio::time::Instant::now() + Duration::from_secs(3),
        )
        .await?;
        if let Some(sub) = self.subscription.lock().await.as_mut() {
            sub.ready_sender.send_replace(true);
        }
        self.wake();
        Ok(())
    }
}

async fn send_record(
    stream: &mut h2::SendStream<Bytes>,
    value: &Value,
    deadline: tokio::time::Instant,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| Failure::Invalid)?;
    bytes.push(b'\n');
    if bytes.len() > 8192 {
        return Err(Failure::Invalid);
    }
    tokio::time::timeout_at(deadline, async {
        let mut offset = 0;
        while offset < bytes.len() {
            stream.reserve_capacity(bytes.len() - offset);
            let capacity = std::future::poll_fn(|cx| stream.poll_capacity(cx))
                .await
                .ok_or(Failure::Transport)?
                .map_err(|_| Failure::Transport)?;
            if capacity == 0 {
                continue;
            }
            let end = (offset + capacity).min(bytes.len());
            stream
                .send_data(Bytes::copy_from_slice(&bytes[offset..end]), false)
                .map_err(|_| Failure::Transport)?;
            offset = end;
        }
        Ok(())
    })
    .await
    .map_err(|_| Failure::Transport)?
}

#[cfg(test)]
mod live_tests {
    use super::*;
    #[tokio::test(flavor = "current_thread")]
    async fn subscription_header_wait_ends_at_remaining_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut connection = h2::server::handshake(socket).await.unwrap();
            let (request, _withheld) = connection.accept().await.unwrap().unwrap();
            assert_eq!(request.method(), "POST");
            assert_eq!(request.uri().path(), "/subscription");
            std::future::pending::<()>().await;
        });
        let path = std::env::temp_dir().join(format!(
            "codex-690-header-deadline-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let client = Client::connect(address, &path).await.unwrap();
        client
            .cache
            .lock()
            .unwrap()
            .issue("existing-receiver")
            .unwrap();
        let start = tokio::time::Instant::now();
        let deadline = start + Duration::from_millis(50);
        let result = tokio::time::timeout_at(
            start + Duration::from_secs(1),
            client.ensure_subscription(deadline),
        )
        .await
        .expect("withheld response HEADERS must not leave initialization pending");
        assert!(matches!(result, Err(Failure::Transport)));
        assert!(start.elapsed() < Duration::from_millis(500));
        assert_eq!(
            client.cache.lock().unwrap().consumer.as_deref(),
            Some("existing-receiver")
        );
        server.abort();
        drop(client);
        for file in std::fs::read_dir(&path).unwrap() {
            std::fs::remove_file(file.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(&path).unwrap();
        std::fs::remove_file(path.with_extension("identity")).unwrap();
    }
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires explicit owned candidate REST fixture"]
    async fn main_then_new_runtime_projection_uses_shared_receiver_after_subscription_ack() {
        let port = std::env::var("CODEX_INFO_API_690_PORT").expect("explicit fixture port");
        let parent = std::env::var_os("CODEX_INFO_API_690_OUTPUT")
            .map(std::path::PathBuf::from)
            .expect("owned output");
        let path = parent.join(format!(
            "rust-live-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let address = format!("127.0.0.1:{port}").parse().unwrap();
        let client = Client::connect(address, &path).await.unwrap();
        let mut changes = client.changes();
        client
            .set_targets(&["/state?parts=main".into()])
            .await
            .unwrap();
        let main = client.get("/state?parts=main").await.unwrap();
        assert_eq!(
            main.views.values().next().unwrap().1[0].1["acquisition_state"],
            "ready"
        );
        assert_eq!(
            main.views.values().next().unwrap().1[0].1["models"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        client
            .set_targets(&["/state?parts=main".into(), "/state?parts=runtime".into()])
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        let page = loop {
            match client.get("/state?parts=runtime").await {
                Ok(page) => break page,
                Err(Failure::Pending) => {
                    tokio::time::timeout_at(deadline, changes.changed())
                        .await
                        .unwrap()
                        .unwrap();
                }
                Err(error) => panic!("runtime projection: {error:?}"),
            }
        };
        let runtime = &page.views.values().next().expect("runtime view").1[0].1;
        assert_eq!(runtime["rest_product_version"], "1.0.173");
        assert_eq!(runtime["recorder_status"], "unavailable");
    }
}

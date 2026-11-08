//! Bounded HTTP/2 UI transport. Source capture/encoding happens outside requests.

use super::*;
use bytes::Bytes;
use codex_info_rest_contract::{
    wire::{
        self,
        request::{Projection, Resource, Target},
    },
    PublicThread, PublicThreadActivityStatus,
};
use h2::server::SendResponse;
use http::{Request as HttpRequest, Response};
use std::future::poll_fn;
use tokio::sync::{watch, Semaphore};

const FRAME: usize = 16_384;
const STREAMS: u32 = 8;
mod outbox;
mod session;
mod source;
use session::Peer;
use source::{Source, View};

pub(super) struct Wake<'a>(pub &'a SnapshotStore);
impl Drop for Wake<'_> {
    fn drop(&mut self) {
        self.0.wire_signal.send_modify(|revision| *revision += 1);
    }
}

struct ConnectionOwner {
    id: String,
    consumer: Mutex<Option<String>>,
    close: watch::Sender<bool>,
}
type FailureEvent = (&'static str, &'static str, &'static str, Option<u16>);
struct Hub {
    source: RwLock<Arc<Source>>,
    changes: watch::Sender<u64>,
    consumers: Mutex<BTreeMap<String, Arc<Mutex<Peer>>>>,
    active: Mutex<BTreeMap<String, (String, watch::Sender<bool>)>>,
    demands: Mutex<BTreeMap<String, Target>>,
    demand_signal: watch::Sender<u64>,
    outbox: Option<Arc<outbox::Outbox>>,
    failures: Option<tokio::sync::mpsc::Sender<FailureEvent>>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn opaque_id() -> Result<String, ()> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| ())?;
    Ok(hex(&bytes))
}

fn visible_threads(threads: &[PublicThread]) -> Vec<&PublicThread> {
    let index = threads
        .iter()
        .map(|thread| (thread.id.as_str(), thread))
        .collect::<BTreeMap<_, _>>();
    threads
        .iter()
        .filter(|thread| {
            if thread.parent_thread_id.is_none()
                && thread.activity_status == Some(PublicThreadActivityStatus::Stopped)
            {
                return false;
            }
            let mut parent = thread.parent_thread_id.as_deref();
            for _ in 0..threads.len() {
                let Some(id) = parent else { return true };
                let Some(ancestor) = index.get(id) else {
                    return true;
                };
                if ancestor.activity_status == Some(PublicThreadActivityStatus::Stopped) {
                    return false;
                }
                parent = ancestor.parent_thread_id.as_deref();
            }
            true
        })
        .collect()
}
fn thread_value(thread: &PublicThread) -> Value {
    let mut value = serde_json::to_value(thread).unwrap();
    value.as_object_mut().unwrap().remove("id");
    value["activity_status"] = json!(thread.activity_status.unwrap_or_default());
    value
}

pub(super) fn main_value(
    snapshot: &PublishedSnapshot,
    account: Option<&PublicAccountV3>,
    read_failed: bool,
) -> Value {
    let details = &snapshot.details;
    let historical = account.is_some_and(|account| !account.is_current);
    let mut models = serde_json::to_value(&snapshot.models_v3).expect("validated model DTO");
    for model in models.as_array_mut().expect("model array") {
        if let Some(cost) = model
            .get_mut("estimated_cost")
            .and_then(Value::as_object_mut)
        {
            if let Some(revision) = cost.remove("price_version") {
                cost.insert("price_revision".into(), revision);
            }
        }
    }
    let state = if read_failed || snapshot.acquisition_degraded {
        PublicState::Error
    } else {
        snapshot.current.state
    };
    let summary = if snapshot.thread_snapshot_known
        && details.authenticated
        && !historical
        && state != PublicState::Error
    {
        let count = |threads: Vec<&PublicThread>| {
            let mut models = BTreeMap::<String, (u64, u64, u64)>::new();
            let (mut running, mut unknown) = (0u64, 0u64);
            for thread in &threads {
                let entry = models.entry(thread.model.clone()).or_default();
                entry.0 += 1;
                match thread.activity_status.unwrap_or_default() {
                    PublicThreadActivityStatus::Running => {
                        entry.1 += 1;
                        running += 1
                    }
                    PublicThreadActivityStatus::Unknown => {
                        entry.2 += 1;
                        unknown += 1
                    }
                    PublicThreadActivityStatus::Stopped => {}
                }
            }
            (threads.len(),running,unknown,models.into_iter().map(|(model,(open,running,unknown))|json!({"model":model,"open_count":open,"running_count":running,"unknown_activity_count":unknown})).collect::<Vec<_>>())
        };
        let (open, running, unknown, models) = count(visible_threads(&details.threads));
        Some(
            json!({"open_count":open,"running_count":running,"unknown_activity_count":unknown,"by_model":models}),
        )
    } else {
        None
    };
    json!({"acquisition_state":state,"observed_at":details.observed_at,
        "authenticated":details.authenticated,"plan_label":details.plan_label,
        "selection":account.map(|account| json!({"account_id":account.id,
            "login_id":account.login_id,"is_current":account.is_current})),
        "quota":details.quota,"models_status":snapshot.current.models_status,"models":models,
        "thread_summary":summary })
}

impl Hub {
    fn new(store: &SnapshotStore) -> Self {
        let (changes, _) = watch::channel(0);
        let (demand_signal, _) = watch::channel(0);
        let (failures, mut records) =
            tokio::sync::mpsc::channel::<(&'static str, &'static str, &'static str, Option<u16>)>(
                16 * 8,
            );
        let diagnostics = Arc::clone(&store.diagnostics);
        tokio::spawn(async move {
            while let Some((route, stage, reason, status)) = records.recv().await {
                let log = Arc::clone(&diagnostics);
                let _ =
                    tokio::task::spawn_blocking(move || log.record(route, stage, reason, status))
                        .await;
            }
        });
        let hub = Self {
            source: RwLock::new(Arc::new(Source::empty())),
            changes,
            consumers: Mutex::new(BTreeMap::new()),
            active: Mutex::new(BTreeMap::new()),
            demands: Mutex::new(BTreeMap::new()),
            demand_signal,
            outbox: outbox::Outbox::open().ok().map(Arc::new),
            failures: Some(failures),
        };
        hub.refresh(store);
        hub
    }
    fn failure(
        &self,
        route: &'static str,
        stage: &'static str,
        reason: &'static str,
        status: Option<u16>,
    ) {
        if let Some(queue) = &self.failures {
            let _ = queue.try_send((route, stage, reason, status));
        }
    }
    fn refresh(&self, store: &SnapshotStore) {
        let previous = Arc::clone(&self.source.read().unwrap_or_else(|p| p.into_inner()));
        let Ok(mut source) = Source::build(store, &previous) else {
            return;
        };
        let windows = self
            .demands
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for target in &windows {
            source.prepare_window(target);
        }
        let changed = previous.views.len() != source.views.len()
            || source.views.iter().any(|(key, v)| {
                previous
                    .views
                    .get(key)
                    .is_none_or(|old| old.image != v.image)
            });
        *self.source.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(source);
        if changed {
            self.changes.send_modify(|r| *r += 1)
        }
        #[cfg(test)]
        store
            .wire_prepared
            .send_replace(*store.wire_signal.borrow());
    }
    fn bind(&self, owner: &ConnectionOwner, consumer: &str) -> bool {
        let mut binding = owner.consumer.lock().unwrap_or_else(|p| p.into_inner());
        if binding.as_ref().is_some_and(|id| id != consumer) {
            return false;
        }
        let mut active = self.active.lock().unwrap_or_else(|p| p.into_inner());
        if binding.is_some() {
            return active.get(consumer).is_some_and(|(id, _)| id == &owner.id);
        }
        if let Some((_, previous)) =
            active.insert(consumer.into(), (owner.id.clone(), owner.close.clone()))
        {
            previous.send_replace(true);
        }
        *binding = Some(consumer.into());
        true
    }
    fn peer(
        &self,
        supplied: Option<&str>,
        root: Option<&str>,
    ) -> Result<(String, Arc<Mutex<Peer>>), ()> {
        if root.is_none_or(|root| wire::sync::parse_digest(root).is_err()) {
            return Err(());
        }
        let mut peers = self.consumers.try_lock().map_err(|_| ())?;
        if let Some(id) = supplied {
            if !wire::request::opaque(id) {
                return Err(());
            }
            let peer = peers
                .entry(id.into())
                .or_insert_with(|| Arc::new(Mutex::new(Peer::default())));
            return Ok((id.to_owned(), Arc::clone(peer)));
        }
        let empty = hex(&wire::manifest_root(&[]).unwrap());
        if root != Some(empty.as_str()) {
            return Err(());
        }
        let id = opaque_id()?;
        let peer = Arc::new(Mutex::new(Peer::default()));
        peers.insert(id.clone(), Arc::clone(&peer));
        Ok((id, peer))
    }
    fn window(&self, consumer: &str, target: Option<&Target>) {
        let mut windows = self.demands.lock().unwrap_or_else(|p| p.into_inner());
        let changed = if let Some(target) = target {
            if windows.get(consumer) == Some(target) {
                false
            } else {
                windows.insert(consumer.into(), target.clone());
                true
            }
        } else {
            windows.remove(consumer).is_some()
        };
        drop(windows);
        if changed {
            self.demand_signal.send_modify(|r| *r += 1);
        }
    }
    fn state_body(
        &self,
        target: &Target,
        peer: &Arc<Mutex<Peer>>,
        root: &str,
        consumer: &str,
    ) -> Option<Vec<u8>> {
        // This lock protects only an Arc swap, never source preparation or I/O.
        let source = Arc::clone(&self.source.read().unwrap_or_else(|p| p.into_inner()));
        let selected = source.selected(target);
        if selected.is_none() {
            if target.resource == Resource::Periods
                && target.purpose == Some(wire::request::PeriodPurpose::Window)
            {
                self.window(consumer, Some(target));
            }
            return Some(serde_json::to_vec(&json!({"result":"pending","view_id":hex(&Sha256::digest(target.view_key(None).ok()?)),"reason":"preparing","target_image":null,"control_seq":1})).unwrap());
        }
        peer.try_lock()
            .ok()?
            .data(target, &source, selected.unwrap(), root)
    }
}

async fn send(
    respond: SendResponse<Bytes>,
    status: u16,
    body: Vec<u8>,
    consumer: Option<&str>,
) -> Result<(), ()> {
    send_page(respond, status, body, consumer, None, 0).await
}
async fn send_error(
    hub: &Hub,
    route: &'static str,
    respond: SendResponse<Bytes>,
    status: u16,
    code: &'static str,
    consumer: Option<&str>,
) {
    let stage = match code {
        "bad_request" | "request_too_large" => "request_parse",
        "snapshot_unavailable" => "snapshot_read",
        "serialization_failed" => "response_generation",
        _ => "route_input",
    };
    hub.failure(route, stage, code, Some(status));
    if send(respond, status, error(code), consumer).await.is_err() {
        hub.failure(route, "response_write", "response_failed", Some(status));
    }
}
async fn send_page(
    mut respond: SendResponse<Bytes>,
    status: u16,
    body: Vec<u8>,
    consumer: Option<&str>,
    meta: Option<&outbox::Meta>,
    offset: u64,
) -> Result<(), ()> {
    let mut response = Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", "no-store")
        .header("content-length", body.len());
    if let Some(consumer) = consumer {
        response = response.header("codex-info-consumer", consumer);
    }
    if let Some(meta) = meta {
        response = response
            .header("codex-info-delivery", &meta.delivery)
            .header("codex-info-body-sha256", &meta.hash)
            .header("codex-info-total-bytes", meta.length)
            .header("codex-info-offset", offset);
    }
    let mut stream = respond
        .send_response(response.body(()).map_err(|_| ())?, body.is_empty())
        .map_err(|_| ())?;
    let mut offset = 0;
    while offset < body.len() {
        let requested = (body.len() - offset).min(FRAME);
        stream.reserve_capacity(requested);
        let capacity = poll_fn(|cx| stream.poll_capacity(cx))
            .await
            .ok_or(())?
            .map_err(|_| ())?;
        if capacity == 0 {
            continue;
        }
        let end = (offset + capacity.min(FRAME)).min(body.len());
        stream
            .send_data(
                Bytes::copy_from_slice(&body[offset..end]),
                end == body.len(),
            )
            .map_err(|_| ())?;
        offset = end;
    }
    Ok(())
}

fn error(code: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"error":code})).unwrap()
}

async fn handle(
    request: HttpRequest<h2::RecvStream>,
    respond: SendResponse<Bytes>,
    hub: Arc<Hub>,
    owner: Arc<ConnectionOwner>,
    received: tokio::time::Instant,
) {
    let route = match request.uri().path() {
        "/state" => "/state",
        "/history/periods" => "/history/periods",
        "/history" => "/history",
        "/threads" => "/threads",
        "/subscription" => "/subscription",
        _ => "unknown",
    };
    let budget = request
        .headers()
        .get("codex-info-budget-ms")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0 && *v <= 3000);
    let deadline = received + Duration::from_millis(budget.unwrap_or(1000).min(1000));
    let request_is_subscription = request.uri().path() == "/subscription";
    let operation = async {
        let raw = request
            .uri()
            .path_and_query()
            .map(|v| v.as_str())
            .unwrap_or("");
        let target = match Target::parse(raw) {
            Ok(Some(target)) => target,
            Ok(None) => {
                send_error(&hub, route, respond, 404, "bad_request", None).await;
                return;
            }
            Err(_) => {
                send_error(&hub, route, respond, 400, "bad_request", None).await;
                return;
            }
        };
        if request.method().as_str() != target.resource.method() {
            send_error(&hub, route, respond, 405, "bad_request", None).await;
            return;
        }
        if budget.is_none() {
            send_error(&hub, route, respond, 400, "bad_request", None).await;
            return;
        }
        if target.resource != Resource::Subscription && !request.body().is_end_stream() {
            send_error(&hub, route, respond, 413, "request_too_large", None).await;
            return;
        }
        let root = request
            .headers()
            .get("codex-info-cache-root")
            .and_then(|v| v.to_str().ok());
        let supplied = request
            .headers()
            .get("codex-info-consumer")
            .and_then(|v| v.to_str().ok());
        let (consumer, peer) = match hub.peer(supplied, root) {
            Ok(peer) => peer,
            Err(_) => {
                send_error(&hub, route, respond, 400, "bad_request", None).await;
                return;
            }
        };
        if !hub.bind(&owner, &consumer) {
            send_error(&hub, route, respond, 400, "bad_request", None).await;
            return;
        }
        if target.resource == Resource::Subscription {
            session::subscription(request, respond, Arc::clone(&hub), consumer, peer).await;
            return;
        }
        if let Some(delivery) = request
            .headers()
            .get("codex-info-delivery")
            .and_then(|h| h.to_str().ok())
        {
            let resume = request
                .headers()
                .get("codex-info-offset")
                .and_then(|h| h.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .zip(
                    request
                        .headers()
                        .get("codex-info-prefix-sha256")
                        .and_then(|h| h.to_str().ok()),
                );
            let Some((offset, prefix)) = resume else {
                send_error(&hub, route, respond, 400, "bad_request", Some(&consumer)).await;
                return;
            };
            let Some(outbox) = hub.outbox.clone() else {
                send_error(
                    &hub,
                    route,
                    respond,
                    503,
                    "snapshot_unavailable",
                    Some(&consumer),
                )
                .await;
                return;
            };
            let (owned_consumer, owned_raw, owned_delivery, owned_prefix) = (
                consumer.clone(),
                raw.to_owned(),
                delivery.to_owned(),
                prefix.to_owned(),
            );
            let restored = tokio::task::spawn_blocking(move || {
                outbox.resume(
                    &owned_consumer,
                    &owned_raw,
                    &owned_delivery,
                    offset,
                    &owned_prefix,
                )
            })
            .await;
            match restored {
                Ok(Ok((meta, body))) => {
                    if let Ok(data) = serde_json::from_slice::<wire::sync::Data>(&body) {
                        if let Ok(mut peer) = peer.try_lock() {
                            let _ = peer.restore_delivery(&meta, &target, &data);
                        }
                    }
                    let suffix = body[offset as usize..].to_vec();
                    let _ =
                        send_page(respond, 200, suffix, Some(&consumer), Some(&meta), offset).await;
                }
                _ => {
                    let body=serde_json::to_vec(&json!({"result":"cache_recovery_required","reason":"response_prefix_mismatch","view_id":hex(&Sha256::digest(target.view_key(None).unwrap_or_default())),"target_image":null,"control_seq":1})).unwrap();
                    let _ = send(respond, 200, body, Some(&consumer)).await;
                }
            }
            return;
        }
        if let Some(body) = hub.state_body(&target, &peer, root.unwrap_or(""), &consumer) {
            let status = if body.is_empty() {
                204
            } else if serde_json::from_slice::<Value>(&body)
                .ok()
                .is_some_and(|v| v.get("error").is_some())
            {
                400
            } else {
                200
            };
            if status == 200
                && serde_json::from_slice::<Value>(&body)
                    .ok()
                    .is_some_and(|v| v["result"] == "data")
            {
                let Some(outbox) = hub.outbox.clone() else {
                    send_error(
                        &hub,
                        route,
                        respond,
                        503,
                        "snapshot_unavailable",
                        Some(&consumer),
                    )
                    .await;
                    return;
                };
                let (owned_consumer, owned_raw) = (consumer.clone(), raw.to_owned());
                let source = hub.source.read().unwrap_or_else(|p| p.into_inner()).clone();
                let saved = tokio::task::spawn_blocking(move || {
                    let result = outbox.save(&owned_consumer, &owned_raw, &body, &source.catalog);
                    (result, body)
                })
                .await;
                if let Ok((Ok(meta), body)) = saved {
                    if send_page(respond, status, body, Some(&consumer), meta.as_ref(), 0)
                        .await
                        .is_err()
                    {
                        hub.failure(route, "response_write", "response_failed", Some(status));
                    }
                } else {
                    send_error(
                        &hub,
                        route,
                        respond,
                        503,
                        "snapshot_unavailable",
                        Some(&consumer),
                    )
                    .await;
                }
                return;
            }
            if status >= 400 {
                hub.failure(route, "route_input", "bad_request", Some(status));
            }
            if send(respond, status, body, Some(&consumer)).await.is_err() {
                hub.failure(route, "response_write", "response_failed", Some(status));
            }
            return;
        }
        let view = hex(&Sha256::digest(target.view_key(None).unwrap_or_default()));
        let body = serde_json::to_vec(
            &json!({"result":"pending","view_id":view,"reason":"preparing",
            "target_image":null,"control_seq":1}),
        )
        .unwrap();
        let _ = send(respond, 200, body, Some(&consumer)).await;
    };
    if request_is_subscription {
        operation.await;
    } else if tokio::time::timeout_at(deadline, operation).await.is_err() {
        hub.failure(route, "response_write", "deadline_exceeded", None);
    }
}

async fn connection(socket: tokio::net::TcpStream, hub: Arc<Hub>) {
    let mut builder = h2::server::Builder::new();
    builder
        .initial_window_size(FRAME as u32)
        .initial_connection_window_size((STREAMS as usize * FRAME) as u32)
        .max_frame_size(FRAME as u32)
        .max_concurrent_streams(STREAMS)
        .max_header_list_size(MAX_HEADER_BYTES as u32);
    let Ok(Ok(mut connection)) =
        tokio::time::timeout(REQUEST_TIMEOUT, builder.handshake::<_, Bytes>(socket)).await
    else {
        return;
    };
    let (close, mut evicted) = watch::channel(false);
    let owner = Arc::new(ConnectionOwner {
        id: opaque_id().unwrap(),
        consumer: Mutex::new(None),
        close,
    });
    loop {
        tokio::select! {
         _=evicted.changed()=>break,
         next=connection.accept()=>{let Some(Ok((request,respond)))=next else{break};let received=tokio::time::Instant::now();tokio::spawn(handle(request,respond,Arc::clone(&hub),Arc::clone(&owner),received));}
        }
    }
    let binding = owner.consumer.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(consumer) = binding.as_ref() {
        let mut active = hub.active.lock().unwrap_or_else(|p| p.into_inner());
        if active.get(consumer).is_some_and(|(id, _)| id == &owner.id) {
            active.remove(consumer);
            hub.window(consumer, None);
        }
    }
}

pub(super) fn serve(listener: TcpListener, store: Arc<SnapshotStore>, stop: Arc<AtomicBool>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("HTTP/2 runtime");
    runtime.block_on(async move {
        let Ok(listener)=tokio::net::TcpListener::from_std(listener) else{return};
        let hub=Arc::new(Hub::new(&store));
        let permits=Arc::new(Semaphore::new(16));
        let (publisher_store,publisher_hub,publisher_stop)=(Arc::clone(&store),Arc::clone(&hub),Arc::clone(&stop));
        let publisher=tokio::spawn(async move {
            let mut revision = publisher_store.wire_signal.subscribe();
            let mut demand = publisher_hub.demand_signal.subscribe();
            revision.mark_changed();
            loop {
                tokio::select!{r=revision.changed()=>{if r.is_err(){break}},r=demand.changed()=>{if r.is_err(){break}}}
                if publisher_stop.load(Ordering::Acquire){break}
                // Source I/O/canonical preparation belongs to this worker,
                // never to the request task.
                let (source,prepared)=(Arc::clone(&publisher_store),Arc::clone(&publisher_hub));
                let _=tokio::task::spawn_blocking(move||prepared.refresh(&source)).await;
            }
        });
        let mut stop_signal = store.wire_signal.subscribe();
        loop {
            if stop.load(Ordering::Acquire)||store.publication_failed(){break}
            tokio::select! {
                _=stop_signal.changed()=>{},
                accepted=listener.accept()=>{
                    let Ok((socket,_))=accepted else{break};
                    let Ok(permit)=Arc::clone(&permits).try_acquire_owned() else{drop(socket);continue};
                    let hub=Arc::clone(&hub);
                    tokio::spawn(async move{let _permit=permit;connection(socket,hub).await;});
                }
            }
        }
        publisher.abort();
    });
}

#[cfg(test)]
pub(super) fn assert_confirmed_main_thread_dependency_boundaries() {
    use wire::sync::{Catalog, Data, Receiver};
    fn prepared(snapshot: &PublishedSnapshot, account: &PublicAccountV3) -> Arc<Source> {
        let mut catalog = Catalog::default();
        let mut views = BTreeMap::new();
        for (path, part, rows) in [
            (
                "/state?parts=main&account=account-1",
                Some(Projection::Main),
                vec![(
                    json!(["main"]),
                    "main",
                    main_value(snapshot, Some(account), false),
                )],
            ),
            (
                "/threads?account=account-1",
                None,
                snapshot
                    .details
                    .threads
                    .iter()
                    .map(|thread| (json!(["thread", thread.id]), "thread", thread_value(thread)))
                    .collect(),
            ),
        ] {
            let target = Target::parse(path).unwrap().unwrap();
            let key = target.view_key(part).unwrap();
            let mut tree = wire::manifest::Tree::default();
            let mut row_ids = BTreeMap::new();
            let mut prepared = Vec::new();
            let mut definitions = Vec::new();
            let mut seen = BTreeSet::new();
            for (logical, kind, value) in rows {
                let id = catalog.intern(kind, &value).unwrap();
                let bytes = wire::canonical_json(None, &logical).unwrap();
                tree.insert(bytes.clone(), catalog.digests[&id]).unwrap();
                row_ids.insert(bytes, id);
                for definition in catalog.transfer(id, &seen).unwrap() {
                    seen.insert(definition.id);
                    definitions.push(definition.id);
                }
                prepared.push(source::Row { key: logical, id });
            }
            views.insert(
                key,
                Arc::new(View {
                    account: Some(account.id.clone()),
                    image: hex(&tree.root()),
                    rows: prepared,
                    definitions,
                    tree,
                    row_ids,
                }),
            );
        }
        Arc::new(Source { views, catalog })
    }
    let account = PublicAccountV3 {
        id: "account-1".into(),
        is_current: true,
        activation_at: Some(1799990000),
        deactivation_at: None,
        ownership_intervals: vec![],
        login_id: None,
    };
    let mut snapshot =
        PublishedSnapshot::account_boundary(PublicState::Initializing, 1, "source".into());
    snapshot.details.authenticated = true;
    snapshot.details.observed_at = Some(1800000000);
    snapshot.current.state = PublicState::Ready;
    snapshot.current.models_status = codex_info_db_reader::CurrentModelsStatus::Complete;
    snapshot.thread_snapshot_known = true;
    snapshot.details.quota = Some(codex_info_rest_contract::PublicQuota {
        remaining_percent: 50.0,
        reset_at: 1800003600,
        window_seconds: 3600,
        monthly: false,
    });
    snapshot.models_v3 = [("SOL", 11), ("TERRA", 22), ("LUNA", 33)]
        .into_iter()
        .map(
            |(model, total)| codex_info_rest_contract::PublicModelUsageV3 {
                model: model.into(),
                total_tokens: total,
                input_tokens: total,
                cached_input_tokens: 0,
                cache_write_input_tokens: Some(0),
                output_tokens: 0,
                estimated_cost: None,
            },
        )
        .collect();
    let thread:PublicThread=serde_json::from_value(json!({"id":"thread-a","title":"before","activity_status":"running","parent_thread_id":null,"model":"gpt-6.1-sol","model_label":"6.1 Sol","total_tokens":11,"context_usage_tokens":1,"context_window_tokens":100,"created_at":1800000000,"last_user_message_at":1800000000,"is_subagent":false,"depth":0})).unwrap();
    snapshot.details.threads.push(thread);
    let main = Target::parse("/state?parts=main&account=account-1")
        .unwrap()
        .unwrap();
    let threads = Target::parse("/threads?account=account-1")
        .unwrap()
        .unwrap();
    let key = main.view_key(Some(Projection::Main)).unwrap();
    let thread_key = threads.view_key(None).unwrap();
    let first = prepared(&snapshot, &account);
    let before = main_value(&snapshot, Some(&account), false);
    assert_eq!(before["quota"]["remaining_percent"], 50.0);
    assert_eq!(before["models"].as_array().unwrap().len(), 3);
    assert_eq!(before["thread_summary"]["open_count"], 1);
    let mut peer = Peer::default();
    let mut receiver = Receiver::default();
    let data: Data = serde_json::from_slice(
        &peer
            .data(
                &main,
                &first,
                first.selected(&main).unwrap(),
                &hex(&receiver.root().unwrap()),
            )
            .unwrap(),
    )
    .unwrap();
    receiver.register_target(&main, &data).unwrap();
    receiver.apply(&data).unwrap();
    assert!(peer.receipt(
        &data.delivery_id,
        data.record_count() as u64,
        &hex(&receiver.root().unwrap())
    ));
    snapshot.details.threads[0].title = "after".into();
    snapshot.details.threads[0].context_usage_tokens = Some(2);
    snapshot.details.threads[0].last_user_message_at = Some(1800000001);
    let title = prepared(&snapshot, &account);
    assert_ne!(
        first.views[&thread_key].image,
        title.views[&thread_key].image
    );
    assert_eq!(first.views[&key].image, title.views[&key].image);
    assert!(peer
        .data(
            &main,
            &title,
            title.selected(&main).unwrap(),
            &hex(&receiver.root().unwrap())
        )
        .unwrap()
        .is_empty());
    let (changes, _) = watch::channel(0);
    let (demand_signal, _) = watch::channel(0);
    let hub = Hub {
        source: RwLock::new(title),
        changes,
        consumers: Mutex::new(BTreeMap::new()),
        active: Mutex::new(BTreeMap::new()),
        demands: Mutex::new(BTreeMap::new()),
        demand_signal,
        outbox: None,
        failures: None,
    };
    let id = hex(&Sha256::digest(&key));
    let mut sent = BTreeMap::from([(id.clone(), first.views[&key].image.clone())]);
    assert!(
        session::notification_for_test(&hub, std::slice::from_ref(&main), &mut sent, 1).is_none(),
        "closed Threads title/context changes produce no Main notification"
    );
    let mut second = snapshot.details.threads[0].clone();
    second.id = "thread-b".into();
    snapshot.details.threads.push(second);
    let count = prepared(&snapshot, &account);
    assert_ne!(count.views[&key].image, first.views[&key].image);
    *hub.source.write().unwrap() = count;
    let changed =
        session::notification_for_test(&hub, std::slice::from_ref(&main), &mut sent, 2).unwrap();
    assert_eq!(changed["views"][0]["view_id"], id);
    let after = main_value(&snapshot, Some(&account), false);
    assert_eq!(after["thread_summary"]["open_count"], 2);
    assert_eq!(after["models"], before["models"]);
    assert_eq!(after["quota"], before["quota"]);
}

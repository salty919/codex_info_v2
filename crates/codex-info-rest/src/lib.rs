//! Standalone, loopback-only REST process.
//!
//! The process owns an in-memory publication cache and a read-only database
//! reader.  It never imports the recorder, writer, Session, Slint, or root
//! `codex_info` crates.  A failed candidate read leaves the last complete
//! generation in place and marks the store degraded for diagnostics.

use codex_info_db_reader::{DbReader, DbSnapshot};
use codex_info_rest_contract::{
    PublicAccountV3, PublicAccountsV3, PublicDetails, PublicDetailsV2, PublicDetailsV3,
    PublicHistoryGap, PublicState, API_VERSION, API_VERSION_V2, API_VERSION_V3,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_REQUEST_LINE_BYTES: usize = 2_048;
const MAX_HEADER_BYTES: usize = 8 * 1_024;
const MAX_HEADER_COUNT: usize = 32;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_HISTORY_PAGE: usize = 1_024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedSnapshot {
    pub generation: u64,
    pub data_hash: String,
    pub pair: String,
    pub has_pending_ranges: bool,
    pub details: PublicDetails,
    pub models_v3: Vec<codex_info_rest_contract::PublicModelUsageV3>,
    pub history_samples_v3: Vec<codex_info_rest_contract::PublicHistoryObservationV3>,
    pub history_samples_v2: Vec<codex_info_rest_contract::PublicHistoryObservation>,
}

impl PublishedSnapshot {
    fn from_db(snapshot: DbSnapshot) -> Self {
        let hash_prefix = snapshot.data_hash.get(..32).unwrap_or(&snapshot.data_hash);
        let pair = format!("v1:{:032x}{hash_prefix}", snapshot.generation);
        Self {
            generation: snapshot.generation,
            data_hash: snapshot.data_hash,
            pair,
            has_pending_ranges: snapshot.has_pending_ranges,
            details: snapshot.details,
            models_v3: snapshot.models_v3,
            history_samples_v3: snapshot.history_samples_v3,
            history_samples_v2: snapshot.history_samples_v2,
        }
    }

    fn from_db_for_account(snapshot: DbSnapshot, storage_epoch: u64) -> Self {
        let mut published = Self::from_db(snapshot);
        // Keep the established v1:<64 hex> wire shape while binding both the
        // stable account-partition namespace and the complete content
        // identity.  The fixed layout is epoch (64 bit), generation (64 bit),
        // and the first 128 bits of the reader's SHA-256 data hash.  In
        // particular, a restart after a same-generation content rewrite must
        // not turn an old conditional request into a false 304.
        let hash_prefix = published
            .data_hash
            .get(..32)
            .expect("DbReader data hashes are canonical 64-character hex");
        published.pair = format!(
            "v1:{storage_epoch:016x}{:016x}{hash_prefix}",
            published.generation
        );
        published
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshStatus {
    Updated { generation: u64 },
    Unchanged { generation: u64 },
    RetainedLastGood { generation: u64 },
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreStatus {
    pub generation: Option<u64>,
    pub degraded: bool,
}

#[derive(Debug)]
struct StoreInner {
    current: Option<Arc<PublishedSnapshot>>,
    degraded: bool,
}

struct AccountStore {
    reader: DbReader,
    storage_epoch: u64,
    refresh_lock: Mutex<()>,
    inner: RwLock<StoreInner>,
}

impl AccountStore {
    fn new(reader: DbReader, storage_epoch: u64) -> Self {
        Self {
            reader,
            storage_epoch,
            refresh_lock: Mutex::new(()),
            inner: RwLock::new(StoreInner {
                current: None,
                degraded: false,
            }),
        }
    }

    fn reader(&self) -> &DbReader {
        &self.reader
    }

    /// Read one candidate and atomically replace the current generation only
    /// after the reader has completed all schema/value/domain checks.
    fn refresh(&self) -> RefreshStatus {
        let _refresh_guard = self
            .refresh_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .current
            .as_ref()
            .map(|snapshot| (snapshot.generation, snapshot.has_pending_ranges));
        if let Some((current_generation, current_pending)) = current {
            match self.reader.read_change_marker() {
                Ok(Some(marker))
                    if marker.generation == current_generation
                        && marker.has_pending_ranges == current_pending =>
                {
                    self.inner
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .degraded = marker.has_pending_ranges;
                    return RefreshStatus::Unchanged {
                        generation: current_generation,
                    };
                }
                Ok(Some(marker)) if marker.generation < current_generation => {
                    self.inner
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .degraded = true;
                    return RefreshStatus::RetainedLastGood {
                        generation: current_generation,
                    };
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("codex-info-rest: snapshot marker read failed: {error}");
                    self.inner
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .degraded = true;
                    return RefreshStatus::RetainedLastGood {
                        generation: current_generation,
                    };
                }
            }
        }
        let candidate = self
            .reader
            .read_snapshot()
            .map(|snapshot| PublishedSnapshot::from_db_for_account(snapshot, self.storage_epoch));
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match candidate {
            Ok(candidate) => {
                let current = inner
                    .current
                    .as_ref()
                    .map(|snapshot| (snapshot.generation, snapshot.data_hash.clone()));
                match current {
                    None => {
                        let generation = candidate.generation;
                        let degraded = candidate.has_pending_ranges;
                        inner.current = Some(Arc::new(candidate));
                        inner.degraded = degraded;
                        RefreshStatus::Updated { generation }
                    }
                    Some((current_generation, _)) if candidate.generation < current_generation => {
                        inner.degraded = true;
                        RefreshStatus::RetainedLastGood {
                            generation: current_generation,
                        }
                    }
                    Some((current_generation, current_hash))
                        if candidate.generation == current_generation
                            && candidate.data_hash == current_hash =>
                    {
                        inner.degraded = candidate.has_pending_ranges;
                        RefreshStatus::Unchanged {
                            generation: current_generation,
                        }
                    }
                    Some((current_generation, _)) if candidate.generation == current_generation => {
                        // A writer must advance generation whenever content
                        // changes.  Refusing a same-generation rewrite prevents
                        // two incompatible rows from being mixed in the cache.
                        inner.degraded = true;
                        RefreshStatus::RetainedLastGood {
                            generation: current_generation,
                        }
                    }
                    Some(_) => {
                        let generation = candidate.generation;
                        let degraded = candidate.has_pending_ranges;
                        inner.current = Some(Arc::new(candidate));
                        inner.degraded = degraded;
                        RefreshStatus::Updated { generation }
                    }
                }
            }
            Err(error) => {
                eprintln!("codex-info-rest: snapshot refresh failed: {error}");
                if let Some(current_generation) =
                    inner.current.as_ref().map(|current| current.generation)
                {
                    inner.degraded = true;
                    RefreshStatus::RetainedLastGood {
                        generation: current_generation,
                    }
                } else {
                    inner.degraded = true;
                    RefreshStatus::Unavailable
                }
            }
        }
    }

    fn status(&self) -> StoreStatus {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        StoreStatus {
            generation: inner.current.as_ref().map(|snapshot| snapshot.generation),
            degraded: inner.degraded,
        }
    }

    fn snapshot(&self) -> Option<Arc<PublishedSnapshot>> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .current
            .clone()
    }

    /// Return the last-good snapshot and its publication health atomically.
    /// Keeping these values under one read lock prevents a concurrent refresh
    /// from pairing a new generation with the previous degraded flag.
    fn snapshot_with_status(&self) -> (Option<Arc<PublishedSnapshot>>, bool) {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (inner.current.clone(), inner.degraded)
    }
}

/// Reader metadata supplied by production account discovery.  The reader is
/// already opened read-only (and, in production, identity-validated) before
/// this value is handed to the REST cache.
#[derive(Debug)]
pub struct AccountReader {
    pub id: String,
    pub storage_epoch: u64,
    pub is_current: bool,
    pub activation_at: Option<i64>,
    pub deactivation_at: Option<i64>,
    pub login_id: Option<String>,
    pub reader: DbReader,
}

impl AccountReader {
    pub fn new(
        id: impl Into<String>,
        storage_epoch: u64,
        is_current: bool,
        activation_at: Option<i64>,
        deactivation_at: Option<i64>,
        reader: DbReader,
    ) -> Self {
        Self {
            id: id.into(),
            storage_epoch,
            is_current,
            activation_at,
            deactivation_at,
            login_id: None,
            reader,
        }
    }

    pub fn with_login_id(mut self, login_id: Option<String>) -> Self {
        self.login_id = login_id;
        self
    }
}

/// Reader/cache boundary.  The cache is an in-memory last-good snapshot per
/// account partition, not a second persistence authority and is never written
/// to disk.  `new(DbReader)` remains the fixture-compatible single-reader API.
pub struct SnapshotStore {
    default_account_id: String,
    accounts: BTreeMap<String, AccountStore>,
    account_descriptors: Vec<PublicAccountV3>,
}

impl SnapshotStore {
    pub fn new(reader: DbReader) -> Self {
        Self::new_with_accounts(
            vec![AccountReader::new("account-1", 1, true, None, None, reader)],
            "account-1",
        )
        .expect("single fixture account configuration is valid")
    }

    pub fn new_with_accounts(
        mut accounts: Vec<AccountReader>,
        default_account_id: impl Into<String>,
    ) -> Result<Self, RestServerError> {
        if accounts.is_empty() {
            return Err(RestServerError::InvalidAccounts(
                "at least one account reader is required".to_owned(),
            ));
        }
        let default_account_id = default_account_id.into();
        accounts.sort_by(|left, right| {
            right
                .is_current
                .cmp(&left.is_current)
                .then_with(|| right.storage_epoch.cmp(&left.storage_epoch))
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut descriptors = Vec::with_capacity(accounts.len());
        let mut stores = BTreeMap::new();
        for account in accounts {
            if account.id != format!("account-{}", account.storage_epoch)
                || account.storage_epoch == 0
            {
                return Err(RestServerError::InvalidAccounts(
                    "account selector does not match storage epoch".to_owned(),
                ));
            }
            let descriptor = PublicAccountV3 {
                id: account.id.clone(),
                is_current: account.is_current,
                activation_at: account.activation_at,
                deactivation_at: account.deactivation_at,
                login_id: account.login_id,
            };
            descriptors.push(descriptor);
            if stores
                .insert(
                    account.id,
                    AccountStore::new(account.reader, account.storage_epoch),
                )
                .is_some()
            {
                return Err(RestServerError::InvalidAccounts(
                    "duplicate account selector".to_owned(),
                ));
            }
        }
        let public = PublicAccountsV3 {
            default_account_id: default_account_id.clone(),
            accounts: descriptors.clone(),
        };
        public.validate().map_err(|error| {
            RestServerError::InvalidAccounts(format!("invalid account selector: {error}"))
        })?;
        Ok(Self {
            default_account_id,
            accounts: stores,
            account_descriptors: descriptors,
        })
    }

    pub fn reader(&self) -> &DbReader {
        self.accounts
            .get(&self.default_account_id)
            .expect("validated default account")
            .reader()
    }

    pub fn refresh(&self) -> RefreshStatus {
        self.refresh_account(&self.default_account_id)
    }

    pub fn refresh_account(&self, account_id: &str) -> RefreshStatus {
        self.accounts
            .get(account_id)
            .map(AccountStore::refresh)
            .unwrap_or(RefreshStatus::Unavailable)
    }

    pub fn status(&self) -> StoreStatus {
        self.status_account(&self.default_account_id)
    }

    pub fn status_account(&self, account_id: &str) -> StoreStatus {
        self.accounts
            .get(account_id)
            .map(AccountStore::status)
            .unwrap_or(StoreStatus {
                generation: None,
                degraded: true,
            })
    }

    pub fn snapshot(&self) -> Option<Arc<PublishedSnapshot>> {
        self.snapshot_account(&self.default_account_id)
    }

    pub fn snapshot_account(&self, account_id: &str) -> Option<Arc<PublishedSnapshot>> {
        self.accounts
            .get(account_id)
            .and_then(AccountStore::snapshot)
    }

    pub fn snapshot_with_status(&self) -> (Option<Arc<PublishedSnapshot>>, bool) {
        self.snapshot_with_status_account(&self.default_account_id)
    }

    pub fn snapshot_with_status_account(
        &self,
        account_id: &str,
    ) -> (Option<Arc<PublishedSnapshot>>, bool) {
        self.accounts
            .get(account_id)
            .map(AccountStore::snapshot_with_status)
            .unwrap_or((None, true))
    }

    pub fn has_account(&self, account_id: &str) -> bool {
        self.accounts.contains_key(account_id)
    }

    pub fn default_account_id(&self) -> &str {
        &self.default_account_id
    }

    pub fn account_descriptors(&self) -> &[PublicAccountV3] {
        &self.account_descriptors
    }
}

#[derive(Debug)]
pub enum RestServerError {
    NonLoopbackAddress,
    Bind(io::Error),
    Listener(io::Error),
    InvalidPort,
    InvalidAccounts(String),
}

impl fmt::Display for RestServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonLoopbackAddress => formatter.write_str("REST listener must use loopback"),
            Self::Bind(error) => write!(formatter, "REST listener bind failed: {error}"),
            Self::Listener(error) => write!(formatter, "REST listener failed: {error}"),
            Self::InvalidPort => formatter.write_str("REST port is invalid"),
            Self::InvalidAccounts(error) => write!(formatter, "REST accounts are invalid: {error}"),
        }
    }
}

impl std::error::Error for RestServerError {}

pub struct RestServer {
    local_addr: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    store: Arc<SnapshotStore>,
}

impl RestServer {
    pub fn start(reader: DbReader, listen_addr: SocketAddr) -> Result<Self, RestServerError> {
        Self::start_with_accounts(
            vec![AccountReader::new("account-1", 1, true, None, None, reader)],
            "account-1",
            listen_addr,
        )
    }

    /// Start the REST listener with every registry-authoritative initialized
    /// account partition.  The default is the one marked current by the
    /// production locator; all account caches remain physically independent.
    pub fn start_with_accounts(
        accounts: Vec<AccountReader>,
        default_account_id: impl Into<String>,
        listen_addr: SocketAddr,
    ) -> Result<Self, RestServerError> {
        if !listen_addr.ip().is_loopback() {
            return Err(RestServerError::NonLoopbackAddress);
        }
        let listener = TcpListener::bind(listen_addr).map_err(RestServerError::Bind)?;
        listener
            .set_nonblocking(true)
            .map_err(RestServerError::Listener)?;
        let local_addr = listener.local_addr().map_err(RestServerError::Listener)?;
        let store = Arc::new(SnapshotStore::new_with_accounts(
            accounts,
            default_account_id,
        )?);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_store = Arc::clone(&store);
        let worker = thread::Builder::new()
            .name("codex-info-rest".to_owned())
            .spawn(move || serve(listener, worker_store, worker_stop))
            .map_err(RestServerError::Listener)?;
        Ok(Self {
            local_addr,
            stop,
            worker: Some(worker),
            store,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn store(&self) -> Arc<SnapshotStore> {
        Arc::clone(&self.store)
    }

    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RestServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve(listener: TcpListener, store: Arc<SnapshotStore>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _peer)) => {
                let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT));
                let _ = stream.set_write_timeout(Some(REQUEST_TIMEOUT));
                handle_connection(&mut stream, &store);
                let _ = stream.shutdown(Shutdown::Both);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Route {
    Health,
    Details,
    DetailsV2,
    DetailsV3,
    AccountsV3,
    CurrentV3,
    HistoryPeriodsV3,
    HistoryV3,
    ThreadsV3,
}

impl Route {
    fn v3(self) -> bool {
        matches!(
            self,
            Self::DetailsV3
                | Self::AccountsV3
                | Self::CurrentV3
                | Self::HistoryPeriodsV3
                | Self::HistoryV3
                | Self::ThreadsV3
        )
    }
}

#[derive(Debug)]
struct Request {
    method: String,
    route: Option<Route>,
    account: Option<String>,
    period: Option<String>,
    cursor: Option<usize>,
    if_none_match: Option<String>,
    body_length: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParseError {
    BadRequest,
    HeadersTooLarge,
    BodyNotAllowed,
}

fn handle_connection(stream: &mut TcpStream, store: &SnapshotStore) {
    let request = match read_request(stream) {
        Ok(request) => request,
        Err(ParseError::HeadersTooLarge) => {
            write_json_response(
                stream,
                431,
                error_body("request_headers_too_large"),
                None,
                false,
            );
            return;
        }
        Err(ParseError::BodyNotAllowed) => {
            write_json_response(
                stream,
                413,
                error_body("request_body_not_allowed"),
                None,
                false,
            );
            return;
        }
        Err(ParseError::BadRequest) => {
            write_json_response(stream, 400, error_body("bad_request"), None, false);
            return;
        }
    };
    let Some(route) = request.route else {
        write_json_response(stream, 404, error_body("not_found"), None, false);
        return;
    };
    if request.method != "GET" {
        write_json_response(stream, 405, error_body("method_not_allowed"), None, false);
        return;
    }
    if request.body_length > 0 {
        write_json_response(
            stream,
            413,
            error_body("request_body_not_allowed"),
            None,
            false,
        );
        return;
    }
    if route == Route::Health {
        write_json_response(stream, 200, health_body(), None, false);
        return;
    }

    if route == Route::AccountsV3 {
        let accounts = PublicAccountsV3 {
            default_account_id: store.default_account_id().to_owned(),
            accounts: store.account_descriptors().to_vec(),
        };
        match flatten_with_version(API_VERSION_V3, &accounts) {
            Ok(body) => write_json_response(stream, 200, body, None, false),
            Err(RouteError::Serialization) => {
                write_json_response(stream, 500, error_body("serialization_failed"), None, false)
            }
            Err(_) => unreachable!("account descriptors are validated at startup"),
        }
        return;
    }

    let account_id = request
        .account
        .as_deref()
        .unwrap_or_else(|| store.default_account_id());
    if !store.has_account(account_id) {
        write_json_response(stream, 400, error_body("unknown_account"), None, false);
        return;
    }

    // A malformed request never reaches refresh, so parser errors cannot
    // influence published generation or trigger a database access.
    let refresh = store.refresh_account(account_id);
    let (snapshot, degraded) = store.snapshot_with_status_account(account_id);
    let Some(snapshot) = snapshot else {
        let _ = refresh;
        write_json_response(stream, 503, error_body("snapshot_unavailable"), None, false);
        return;
    };
    if !degraded
        && route.v3()
        && request
            .if_none_match
            .as_deref()
            .is_some_and(|pair| pair == snapshot.pair)
    {
        write_json_response(stream, 304, Vec::new(), Some(snapshot.pair.as_str()), true);
        return;
    }
    let response = serialize_route(
        &snapshot,
        route,
        request.period.as_deref(),
        request.cursor,
        degraded,
    );
    match response {
        Ok(body) => write_json_response(stream, 200, body, Some(snapshot.pair.as_str()), false),
        Err(RouteError::StaleCursor) => {
            write_json_response(stream, 400, error_body("stale_cursor"), None, false)
        }
        Err(RouteError::Serialization) => {
            write_json_response(stream, 500, error_body("serialization_failed"), None, false)
        }
        Err(RouteError::UnknownPeriod) => write_json_response(
            stream,
            400,
            error_body("invalid_history_query"),
            None,
            false,
        ),
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Request, ParseError> {
    let started = Instant::now();
    let mut data = Vec::with_capacity(4 * 1_024);
    let terminator;
    loop {
        if started.elapsed() >= REQUEST_TIMEOUT {
            return Err(ParseError::BadRequest);
        }
        let mut chunk = [0_u8; 1_024];
        match stream.read(&mut chunk) {
            Ok(0) => return Err(ParseError::BadRequest),
            Ok(count) => {
                data.extend_from_slice(&chunk[..count]);
                if data.len() > MAX_HEADER_BYTES {
                    return Err(ParseError::HeadersTooLarge);
                }
                if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                    terminator = position + 4;
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(_) => return Err(ParseError::BadRequest),
        }
    }
    let header_bytes = &data[..terminator];
    let trailing = &data[terminator..];
    let text = std::str::from_utf8(header_bytes).map_err(|_| ParseError::BadRequest)?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(ParseError::BadRequest)?;
    if request_line.len() > MAX_REQUEST_LINE_BYTES {
        return Err(ParseError::HeadersTooLarge);
    }
    let mut fields = request_line.split(' ');
    let method = fields.next().ok_or(ParseError::BadRequest)?.to_owned();
    let target = fields.next().ok_or(ParseError::BadRequest)?;
    if fields.next() != Some("HTTP/1.1") || fields.next().is_some() || method.is_empty() {
        return Err(ParseError::BadRequest);
    }
    let mut headers = Vec::<(String, String)>::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or(ParseError::BadRequest)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(ParseError::BadRequest);
        }
        if value.len() > 1_024 {
            return Err(ParseError::HeadersTooLarge);
        }
        let normalized = name.to_ascii_lowercase();
        if headers.iter().any(|(existing, _)| existing == &normalized) {
            return Err(ParseError::BadRequest);
        }
        headers.push((normalized, value.trim().to_owned()));
        if headers.len() > MAX_HEADER_COUNT {
            return Err(ParseError::HeadersTooLarge);
        }
    }
    let content_length = header(&headers, "content-length")
        .map(|value| value.parse::<usize>().map_err(|_| ParseError::BadRequest))
        .transpose()?
        .unwrap_or(0);
    if header(&headers, "transfer-encoding").is_some() {
        return Err(ParseError::BadRequest);
    }
    if content_length > MAX_BODY_BYTES || !trailing.is_empty() && content_length == 0 {
        return Err(ParseError::BodyNotAllowed);
    }
    if content_length > trailing.len() {
        return Err(ParseError::BodyNotAllowed);
    }
    let (route, account, period, cursor) = parse_target(target)?;
    let if_none_match = header(&headers, "if-none-match")
        .map(parse_etag)
        .transpose()?;
    Ok(Request {
        method,
        route,
        account,
        period,
        cursor,
        if_none_match,
        body_length: content_length,
    })
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header_name, _)| header_name == name)
        .map(|(_, value)| value.as_str())
}

fn parse_etag(value: &str) -> Result<String, ParseError> {
    let value = value.trim();
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err(ParseError::BadRequest);
    }
    let pair = &value[1..value.len() - 1];
    if pair.is_empty()
        || !pair
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
    {
        return Err(ParseError::BadRequest);
    }
    Ok(pair.to_owned())
}

type ParsedTarget = (Option<Route>, Option<String>, Option<String>, Option<usize>);

fn valid_account_selector(value: &str) -> bool {
    let Some(epoch) = value.strip_prefix("account-") else {
        return false;
    };
    let Ok(epoch) = epoch.parse::<u64>() else {
        return false;
    };
    epoch > 0 && value == format!("account-{epoch}")
}

fn parse_target(target: &str) -> Result<ParsedTarget, ParseError> {
    if target.is_empty() || target.contains('#') || !target.starts_with('/') {
        return Err(ParseError::BadRequest);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let route = match path {
        "/health" | "/v1/health" => Some(Route::Health),
        "/v1/details" => Some(Route::Details),
        "/v2/details" => Some(Route::DetailsV2),
        "/v3/details" => Some(Route::DetailsV3),
        "/v3/accounts" => Some(Route::AccountsV3),
        "/v3/current" => Some(Route::CurrentV3),
        "/v3/history/periods" => Some(Route::HistoryPeriodsV3),
        "/v3/history" => Some(Route::HistoryV3),
        "/v3/threads" => Some(Route::ThreadsV3),
        _ => None,
    };
    if route != Some(Route::HistoryV3) {
        if route.is_some_and(Route::v3) {
            if target.contains('?') && query.is_empty() {
                return Err(ParseError::BadRequest);
            }
            if route == Some(Route::AccountsV3) {
                if !query.is_empty() {
                    return Err(ParseError::BadRequest);
                }
                return Ok((route, None, None, None));
            }
            if query.is_empty() {
                return Ok((route, None, None, None));
            }
            if query.contains('%') {
                return Err(ParseError::BadRequest);
            }
            let mut account = None;
            for pair in query.split('&') {
                let (name, value) = pair.split_once('=').ok_or(ParseError::BadRequest)?;
                if name != "account"
                    || value.is_empty()
                    || value.contains('=')
                    || !valid_account_selector(value)
                    || account.replace(value.to_owned()).is_some()
                {
                    return Err(ParseError::BadRequest);
                }
            }
            return Ok((route, account, None, None));
        }
        if !query.is_empty() {
            return Ok((None, None, None, None));
        }
        return Ok((route, None, None, None));
    }
    if query.is_empty() || query.contains('%') {
        return Err(ParseError::BadRequest);
    }
    let mut account = None;
    let mut period = None;
    let mut cursor = None;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').ok_or(ParseError::BadRequest)?;
        if value.is_empty() || value.contains('=') {
            return Err(ParseError::BadRequest);
        }
        match name {
            "account" if account.is_none() && valid_account_selector(value) => {
                account = Some(value.to_owned())
            }
            "period" if period.is_none() => period = Some(value.to_owned()),
            "cursor" if cursor.is_none() => {
                cursor = Some(value.parse::<usize>().map_err(|_| ParseError::BadRequest)?);
            }
            _ => return Err(ParseError::BadRequest),
        }
    }
    Ok((Some(Route::HistoryV3), account, period, cursor))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RouteError {
    Serialization,
    UnknownPeriod,
    StaleCursor,
}

fn serialize_route(
    snapshot: &PublishedSnapshot,
    route: Route,
    period: Option<&str>,
    cursor: Option<usize>,
    degraded: bool,
) -> Result<Vec<u8>, RouteError> {
    let details = &snapshot.details;
    match route {
        Route::Health => Ok(health_body()),
        Route::Details => {
            let details = details_v1(snapshot, degraded);
            flatten_with_version(API_VERSION, &details)
        }
        Route::DetailsV2 => {
            let details = details_v2_for_wire(snapshot, degraded);
            flatten_with_version(API_VERSION_V2, &details)
        }
        Route::DetailsV3 => {
            let details = details_v3(snapshot, degraded);
            flatten_with_version(API_VERSION_V3, &details)
        }
        Route::AccountsV3 => Err(RouteError::Serialization),
        Route::CurrentV3 => {
            let state = if degraded {
                PublicState::Error
            } else {
                details.state
            };
            serialize_json(&json!({
                "api_version": API_VERSION_V3,
                "state": state,
                "observed_at": details.observed_at,
                "authenticated": details.authenticated,
                "plan_label": details.plan_label,
                "quota": details.quota,
                "models": snapshot.models_v3,
                "active_thread_count": details.active_thread_count,
            }))
        }
        Route::HistoryPeriodsV3 => serialize_json(&json!({
            "api_version": API_VERSION_V3,
            "history_periods": details.history_periods,
        })),
        Route::ThreadsV3 => serialize_json(&json!({
            "api_version": API_VERSION_V3,
            "threads": details.threads,
        })),
        Route::HistoryV3 => serialize_history(snapshot, period, cursor),
    }
}

fn details_v3(snapshot: &PublishedSnapshot, degraded: bool) -> PublicDetailsV3 {
    let details_v2 = details_v2(snapshot);
    let mut details = PublicDetailsV3::from_v2_with_models_and_history(
        &details_v2,
        &snapshot.models_v3,
        &snapshot.history_samples_v3,
    );
    if degraded {
        // The source read failed after a complete generation had already been
        // published.  Keep every last-good value, but expose the failure on
        // the existing v3 state field instead of silently claiming `ready`.
        details.state = PublicState::Error;
    }
    details
}

fn details_v1(snapshot: &PublishedSnapshot, degraded: bool) -> PublicDetails {
    let mut details = snapshot.details.clone();
    if degraded {
        details.state = PublicState::Error;
    }
    details
}

fn details_v2(snapshot: &PublishedSnapshot) -> PublicDetailsV2 {
    let mut details = PublicDetailsV2::from(&snapshot.details);
    details.history_samples = snapshot.history_samples_v2.clone();
    details
}

fn details_v2_for_wire(snapshot: &PublishedSnapshot, degraded: bool) -> PublicDetailsV2 {
    let mut details = details_v2(snapshot);
    if degraded {
        details.state = PublicState::Error;
    }
    details
}

fn flatten_with_version<T: serde::Serialize>(
    version: &str,
    value: &T,
) -> Result<Vec<u8>, RouteError> {
    let mut object = match serde_json::to_value(value).map_err(|_| RouteError::Serialization)? {
        Value::Object(object) => object,
        _ => return Err(RouteError::Serialization),
    };
    object.insert("api_version".to_owned(), Value::String(version.to_owned()));
    serialize_json(&Value::Object(object))
}

fn serialize_history(
    snapshot: &PublishedSnapshot,
    period: Option<&str>,
    cursor: Option<usize>,
) -> Result<Vec<u8>, RouteError> {
    let details = &snapshot.details;
    let period = period.ok_or(RouteError::UnknownPeriod)?;
    let Some(period_meta) = details
        .history_periods
        .iter()
        .find(|item| item.id == period)
    else {
        return Err(RouteError::UnknownPeriod);
    };
    // The paged history wire shape has no state field; its exact contract is
    // preserved while the v3 details/current resources expose degradation.
    let samples = snapshot
        .history_samples_v3
        .iter()
        .filter(|sample| {
            sample.reset_at >= period_meta.reset_at.saturating_sub(60)
                && sample.reset_at <= period_meta.reset_at
                && sample.timestamp >= period_meta.start_at
                && sample.timestamp <= period_meta.end_at
        })
        .collect::<Vec<_>>();
    let start = cursor.unwrap_or(0);
    if start > samples.len() {
        return Err(RouteError::StaleCursor);
    }
    let end = start.saturating_add(MAX_HISTORY_PAGE).min(samples.len());
    let next_cursor = (end < samples.len()).then(|| end.to_string());
    let history_gaps = if start == 0 {
        details
            .history_gaps
            .iter()
            .filter(|gap| {
                gap.reset_at >= period_meta.reset_at.saturating_sub(60)
                    && gap.reset_at <= period_meta.reset_at
            })
            .cloned()
            .collect::<Vec<PublicHistoryGap>>()
    } else {
        Vec::new()
    };
    serialize_json(&json!({
        "api_version": API_VERSION_V3,
        "history_samples": &samples[start..end],
        "history_gaps": history_gaps,
        "next_cursor": next_cursor,
        "resume_cursor": end.to_string(),
    }))
}

fn serialize_json(value: &Value) -> Result<Vec<u8>, RouteError> {
    serde_json::to_vec(value).map_err(|_| RouteError::Serialization)
}

fn health_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "api_version": API_VERSION,
        // Keep the health wire owner compatible with the Windows strict
        // parser.  The standalone process has its own package version, but
        // it serves the same codex-info loopback contract.
        "service": "codex-info",
        // The REST process has an independent package version, but health is
        // a distribution contract and therefore reports the root product
        // version emitted by build.rs.
        "product_version": env!("CODEX_INFO_PRODUCT_VERSION"),
    }))
    .expect("fixed health body")
}

fn error_body(error: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"api_version": API_VERSION, "error": error}))
        .expect("fixed error body")
}

fn write_json_response(
    stream: &mut TcpStream,
    status: u16,
    body: Vec<u8>,
    pair: Option<&str>,
    not_modified: bool,
) {
    let reason = match status {
        200 => "OK",
        304 => "Not Modified",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let pair_header = if status == 200 || status == 304 {
        pair.map(|pair| format!("Codex-Info-Published-Pair: {pair}\r\n"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let body = if not_modified { Vec::new() } else { body };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n{pair_header}Content-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

/// Parse a `--port` value without allowing non-loopback binds.
pub fn loopback_addr(port: &str) -> Result<SocketAddr, RestServerError> {
    let port = port
        .parse::<u16>()
        .map_err(|_| RestServerError::InvalidPort)?;
    Ok(SocketAddr::new(
        IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("codex-info-rest-{name}-{suffix}.sqlite3"))
    }

    fn fixture(path: &PathBuf, token: i64) {
        let connection = Connection::open(path).expect("fixture");
        connection
            .execute_batch(
                r#"CREATE TABLE usage_history(
                    timestamp INTEGER NOT NULL, reset_at INTEGER NOT NULL,
                    remaining_percent REAL, sol_dollars REAL NOT NULL,
                    terra_dollars REAL NOT NULL, luna_dollars REAL NOT NULL,
                    sol_tokens INTEGER NOT NULL, terra_tokens INTEGER NOT NULL,
                    luna_tokens INTEGER NOT NULL
                );
                CREATE TABLE collection_generation(
                    singleton INTEGER PRIMARY KEY, data_generation TEXT NOT NULL,
                    reset_at INTEGER NOT NULL, window_seconds INTEGER NOT NULL,
                    collector_epoch TEXT, cycle_seq TEXT NOT NULL
                );
                CREATE TABLE durable_state(
                    singleton INTEGER PRIMARY KEY, data_generation INTEGER NOT NULL,
                    data_hash TEXT NOT NULL, snapshot_json TEXT NOT NULL
                );
                INSERT INTO collection_generation VALUES(1,'1',1800000060,3600,NULL,'0');
                INSERT INTO durable_state VALUES
                    (2,1800000000,'fixture-legacy-observation',
                     '{"kind":"codex-info-usage-observation-v1","timestamp":1800000000,"reset_at":1800000060,"remaining_percent":50.0,"model_source":"legacy-unknown"}');"#,
            )
            .expect("schema");
        connection
            .execute(
                "INSERT INTO usage_history VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    1_800_000_000_i64,
                    1_800_000_060_i64,
                    50.0,
                    1.0,
                    2.0,
                    3.0,
                    token,
                    2,
                    3
                ],
            )
            .expect("row");
    }

    fn request(address: SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(address).expect("connect");
        stream.write_all(request.as_bytes()).expect("request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("response");
        response
    }

    fn body(response: &str) -> &str {
        response.split_once("\r\n\r\n").expect("HTTP body").1
    }

    fn published_pair(response: &str) -> &str {
        response
            .lines()
            .find_map(|line| line.strip_prefix("Codex-Info-Published-Pair: "))
            .expect("published pair")
    }

    #[test]
    fn v3_history_page_boundary_preserves_task_activity_field() {
        let reset_at = 1_800_000_600_i64;
        let start_at = 1_800_000_000_i64;
        let history_samples = (0..1_025)
            .map(
                |index| codex_info_rest_contract::PublicHistoryObservationV3 {
                    timestamp: start_at + index * 60,
                    reset_at,
                    remaining_percent: None,
                    task_active_since_previous: Some(index % 2 == 0),
                    models: None,
                    models_complete: false,
                    model_source: "legacy-unknown".to_owned(),
                },
            )
            .collect::<Vec<_>>();
        let mut details = codex_info_rest_contract::PublicDetails::default();
        details
            .history_periods
            .push(codex_info_rest_contract::PublicHistoryPeriod {
                id: reset_at.to_string(),
                start_at,
                end_at: start_at + 1_024 * 60,
                reset_at,
                label: "task page".to_owned(),
                current: true,
            });
        let snapshot = PublishedSnapshot {
            generation: 1,
            data_hash: "hash".to_owned(),
            pair: "pair".to_owned(),
            has_pending_ranges: false,
            details,
            models_v3: Vec::new(),
            history_samples_v2: Vec::new(),
            history_samples_v3: history_samples,
        };

        let first: serde_json::Value = serde_json::from_slice(
            &serialize_history(&snapshot, Some(&reset_at.to_string()), None)
                .expect("first history page"),
        )
        .expect("first page JSON");
        let second: serde_json::Value = serde_json::from_slice(
            &serialize_history(&snapshot, Some(&reset_at.to_string()), Some(1_024))
                .expect("second history page"),
        )
        .expect("second page JSON");

        assert_eq!(first["history_samples"].as_array().unwrap().len(), 1_024);
        assert_eq!(
            first["history_samples"][1023]["task_active_since_previous"],
            false
        );
        assert_eq!(first["next_cursor"], "1024");
        assert_eq!(second["history_samples"].as_array().unwrap().len(), 1);
        assert_eq!(
            second["history_samples"][0]["task_active_since_previous"],
            true
        );
        assert_eq!(second["resume_cursor"], "1025");
    }

    #[test]
    fn health_uses_the_strict_codex_info_contract_with_distribution_version() {
        let value: serde_json::Value = serde_json::from_slice(&health_body()).expect("health");
        let object = value.as_object().expect("health object");
        assert_eq!(object.len(), 3);
        assert_eq!(value["api_version"], "v1");
        assert_eq!(value["service"], "codex-info");
        assert_eq!(value["product_version"], env!("CODEX_INFO_PRODUCT_VERSION"));
    }

    #[test]
    fn malformed_http_isolated_from_snapshot_refresh() {
        let path = temp_db("malformed-http");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("reader");
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).expect("server");
        let malformed = request(
            server.local_addr(),
            "GET /v1/details?bad=1 HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(malformed.starts_with("HTTP/1.1 404"));
        assert_eq!(server.store().status().generation, None);
        let valid = request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(valid.starts_with("HTTP/1.1 200"));
        assert_eq!(server.store().status().generation, Some(1));
        server.shutdown();
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn v1_and_v3_json_keep_root_model_selection_and_cache_write_data() {
        let path = temp_db("model-v3-json");
        fixture(&path, 10);
        let connection = Connection::open(&path).expect("fixture");
        connection
            .execute_batch(
                "CREATE TABLE session_model_totals(
                    model TEXT PRIMARY KEY, total_tokens TEXT NOT NULL,
                    input_tokens TEXT NOT NULL, cached_input_tokens TEXT NOT NULL,
                    output_tokens TEXT NOT NULL, cache_write_input_tokens TEXT
                );
                INSERT INTO session_model_totals VALUES
                    ('ASTRA','1100000','1000000','200000','100000','100000'),
                    ('SOL','110','100','40','10','0'),
                    ('gpt-7-nova','30','20','5','10','3');",
            )
            .expect("model schema");
        connection
            .execute_batch(
                "UPDATE durable_state SET
                    data_hash='history-observation',
                    snapshot_json='{\"kind\":\"codex-info-usage-observation-v1\",\"timestamp\":1800000000,\"reset_at\":1800000060,\"remaining_percent\":50.0,\"model_source\":\"confirmed\"}'
                    WHERE singleton=2;
                CREATE TABLE usage_model_history(
                    reset_at INTEGER NOT NULL, timestamp INTEGER NOT NULL,
                    model TEXT NOT NULL, total_tokens TEXT NOT NULL,
                    input_tokens TEXT NOT NULL, cached_input_tokens TEXT NOT NULL,
                    output_tokens TEXT NOT NULL, cache_write_input_tokens TEXT,
                    model_set_complete INTEGER NOT NULL
                );
                INSERT INTO usage_model_history VALUES
                    (1800000060,1800000000,'ASTRA','1100000','1000000','200000','100000','100000',1),
                    (1800000060,1800000000,'SOL','110','100','40','10','0',1),
                    (1800000060,1800000000,'gpt-7-nova','30','20','5','10','3',1);",
            )
            .expect("history model schema");
        drop(connection);

        let reader = DbReader::open(&path).expect("reader");
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).expect("server");
        let v1_response = request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(v1_response.starts_with("HTTP/1.1 200"));
        let v1: serde_json::Value = serde_json::from_str(body(&v1_response)).expect("v1 JSON");
        let v1_models = v1["models"]
            .as_array()
            .expect("v1 models")
            .iter()
            .map(|model| model["name"].as_str().expect("v1 model name"))
            .collect::<Vec<_>>();
        assert_eq!(v1_models, vec!["SOL"]);
        assert_eq!(v1["models"][0]["input_tokens"], 60);
        assert_eq!(v1["models"][0]["cached_input_tokens"], 40);

        let v3_response = request(
            server.local_addr(),
            "GET /v3/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(v3_response.starts_with("HTTP/1.1 200"));
        let v3: serde_json::Value = serde_json::from_str(body(&v3_response)).expect("v3 JSON");
        let v3_models = v3["models"]
            .as_array()
            .expect("v3 models")
            .iter()
            .map(|model| model["model"].as_str().expect("v3 model name"))
            .collect::<Vec<_>>();
        assert_eq!(v3_models, vec!["SOL", "ASTRA", "gpt-7-nova"]);
        assert_eq!(v3["models"][1]["total_tokens"], 1_100_000);
        assert_eq!(v3["models"][1]["input_tokens"], 1_000_000);
        assert_eq!(v3["models"][1]["cached_input_tokens"], 200_000);
        assert_eq!(v3["models"][1]["cache_write_input_tokens"], 100_000);
        assert_eq!(v3["models"][1]["output_tokens"], 100_000);
        assert_eq!(
            v3["models"][1]["estimated_cost"]["price_version"],
            "ASTRA_USER_2026-09-05"
        );
        assert_eq!(v3["models"][1]["estimated_cost"]["total_dollars"], 13.45);
        assert!(v3["models"][2]["estimated_cost"].is_null());
        assert_eq!(v3["history_samples"][0]["model_source"], "confirmed");
        assert_eq!(v3["history_samples"][0]["models_complete"], true);
        assert!(v3["history_samples"][0]["task_active_since_previous"].is_null());
        assert_eq!(
            v3["history_samples"][0]["models"]
                .as_array()
                .expect("history models")
                .iter()
                .map(|model| model["model"].as_str().expect("history model name"))
                .collect::<Vec<_>>(),
            vec!["ASTRA", "SOL", "gpt-7-nova"]
        );
        assert_eq!(
            v3["history_samples"][0]["models"][0]["cache_write_input_tokens"],
            100_000
        );
        assert!(v3["history_samples"][0]["models"][0]["total_dollars"].is_null());
        server.shutdown();
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn last_good_generation_survives_transient_read_failure_and_recovers() {
        let path = temp_db("last-good");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("reader");
        let store = SnapshotStore::new(reader);
        assert!(matches!(
            store.refresh(),
            RefreshStatus::Updated { generation: 1 }
        ));
        let before = store.snapshot().expect("snapshot");
        let moved = path.with_extension("sqlite3.moved");
        fs::rename(&path, &moved).expect("induce read failure");
        assert!(matches!(
            store.refresh(),
            RefreshStatus::RetainedLastGood { generation: 1 }
        ));
        assert_eq!(store.snapshot().expect("last-good"), before);
        fs::rename(&moved, &path).expect("restore db");
        assert!(matches!(
            store.refresh(),
            RefreshStatus::Unchanged { generation: 1 }
        ));
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn committed_generation_is_the_cache_invalidation_boundary() {
        let path = temp_db("generation-cache");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("reader");
        let store = SnapshotStore::new(reader);
        assert!(matches!(
            store.refresh(),
            RefreshStatus::Updated { generation: 1 }
        ));
        let initial = store.snapshot().expect("initial snapshot");

        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute("UPDATE usage_history SET sol_tokens=99", [])
            .expect("uncommitted generation mutation");
        assert!(matches!(
            store.refresh(),
            RefreshStatus::Unchanged { generation: 1 }
        ));
        assert_eq!(
            store.snapshot().expect("cached snapshot").data_hash,
            initial.data_hash,
            "rows outside a committed generation must not replace the publication cache"
        );

        connection
            .execute(
                "UPDATE collection_generation SET data_generation='2' WHERE singleton=1",
                [],
            )
            .expect("commit generation");
        assert!(matches!(
            store.refresh(),
            RefreshStatus::Updated { generation: 2 }
        ));
        let updated = store.snapshot().expect("updated snapshot");
        assert_ne!(updated.data_hash, initial.data_hash);
        assert_eq!(updated.details.history_samples[0].sol_tokens, 99);
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn v1_and_v2_wire_mark_last_good_snapshot_degraded_without_clearing_data() {
        let path = temp_db("last-good-legacy-wire");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("reader");
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).expect("server");

        let initial_v1 = request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let initial_v2 = request(
            server.local_addr(),
            "GET /v2/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let initial_v1_json: serde_json::Value =
            serde_json::from_str(body(&initial_v1)).expect("initial v1 JSON");
        let initial_v2_json: serde_json::Value =
            serde_json::from_str(body(&initial_v2)).expect("initial v2 JSON");
        assert_eq!(initial_v1_json["state"], "ready");
        assert_eq!(initial_v2_json["state"], "ready");

        let moved = path.with_extension("sqlite3.moved");
        fs::rename(&path, &moved).expect("induce read failure");
        let degraded_v1 = request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let degraded_v2 = request(
            server.local_addr(),
            "GET /v2/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let degraded_v1_json: serde_json::Value =
            serde_json::from_str(body(&degraded_v1)).expect("degraded v1 JSON");
        let degraded_v2_json: serde_json::Value =
            serde_json::from_str(body(&degraded_v2)).expect("degraded v2 JSON");
        assert_eq!(degraded_v1_json["state"], "error");
        assert_eq!(degraded_v2_json["state"], "error");
        assert_eq!(
            degraded_v1_json["history_samples"],
            initial_v1_json["history_samples"]
        );
        assert_eq!(
            degraded_v2_json["history_samples"],
            initial_v2_json["history_samples"]
        );

        fs::rename(&moved, &path).expect("restore db");
        server.shutdown();
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn v3_wire_marks_last_good_snapshot_degraded_without_clearing_data() {
        let path = temp_db("last-good-v3-wire");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("reader");
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).expect("server");

        let initial = request(
            server.local_addr(),
            "GET /v3/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(initial.starts_with("HTTP/1.1 200"));
        let pair = published_pair(&initial).to_owned();
        let initial_json: serde_json::Value =
            serde_json::from_str(body(&initial)).expect("initial v3 JSON");
        assert_eq!(initial_json["state"], "ready");
        assert!(!initial_json["history_samples"]
            .as_array()
            .expect("initial history")
            .is_empty());

        let moved = path.with_extension("sqlite3.moved");
        fs::rename(&path, &moved).expect("induce read failure");
        let conditional_request =
            format!("GET /v3/details HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{pair}\"\r\n\r\n");
        let degraded = request(server.local_addr(), &conditional_request);
        assert!(degraded.starts_with("HTTP/1.1 200"));
        let degraded_json: serde_json::Value =
            serde_json::from_str(body(&degraded)).expect("degraded v3 JSON");
        assert_eq!(degraded_json["state"], "error");
        assert_eq!(
            degraded_json["history_samples"],
            initial_json["history_samples"]
        );

        fs::rename(&moved, &path).expect("restore db");
        let recovered = request(
            server.local_addr(),
            "GET /v3/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let recovered_json: serde_json::Value =
            serde_json::from_str(body(&recovered)).expect("recovered v3 JSON");
        assert_eq!(recovered_json["state"], "ready");
        server.shutdown();
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn v3_account_selector_reads_each_partition_and_scopes_etag() {
        let path_a = temp_db("accounts-a");
        let path_b = temp_db("accounts-b");
        fixture(&path_a, 10);
        fixture(&path_b, 20);
        let connection = Connection::open(&path_b).expect("account B fixture db");
        connection
            .execute_batch(
                "UPDATE usage_history
                    SET timestamp=1800000120, reset_at=1800000180;
                 UPDATE collection_generation SET reset_at=1800000180;
                 UPDATE durable_state SET
                    data_generation=1800000120,
                    snapshot_json='{\"kind\":\"codex-info-usage-observation-v1\",\"timestamp\":1800000120,\"reset_at\":1800000180,\"remaining_percent\":50.0,\"model_source\":\"legacy-unknown\"}'
                    WHERE singleton=2;",
            )
            .expect("account B timestamp");
        drop(connection);

        let reader_a = DbReader::open(&path_a).expect("account A reader");
        let reader_b = DbReader::open(&path_b).expect("account B reader");
        let mut server = RestServer::start_with_accounts(
            vec![
                AccountReader::new("account-7", 7, true, Some(1_800_000_000), None, reader_a),
                AccountReader::new("account-13", 13, false, None, None, reader_b),
            ],
            "account-7",
            "127.0.0.1:0".parse().unwrap(),
        )
        .expect("multi-account server");

        let accounts_response = request(
            server.local_addr(),
            "GET /v3/accounts HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(accounts_response.starts_with("HTTP/1.1 200"));
        let accounts: serde_json::Value =
            serde_json::from_str(body(&accounts_response)).expect("accounts JSON");
        assert_eq!(accounts["api_version"], "v3");
        assert_eq!(accounts["default_account_id"], "account-7");
        assert_eq!(accounts["accounts"][0]["id"], "account-7");
        assert_eq!(accounts["accounts"][0]["is_current"], true);
        assert_eq!(accounts["accounts"][0]["activation_at"], 1_800_000_000_i64);
        assert!(accounts["accounts"][0]["deactivation_at"].is_null());
        assert_eq!(accounts["accounts"][1]["id"], "account-13");
        assert_eq!(accounts["accounts"][1]["is_current"], false);
        assert!(accounts["accounts"][1]["activation_at"].is_null());
        assert!(accounts["accounts"][1]["deactivation_at"].is_null());

        let default_response = request(
            server.local_addr(),
            "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let default_json: serde_json::Value =
            serde_json::from_str(body(&default_response)).expect("default current JSON");
        assert_eq!(default_json["observed_at"], 1_800_000_000_i64);

        let selected_response = request(
            server.local_addr(),
            "GET /v3/current?account=account-13 HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let selected_json: serde_json::Value =
            serde_json::from_str(body(&selected_response)).expect("selected current JSON");
        assert_eq!(selected_json["observed_at"], 1_800_000_120_i64);

        for target in [
            "/v3/details?account=account-13",
            "/v3/history/periods?account=account-13",
            "/v3/threads?account=account-13",
            "/v3/history?account=account-13&period=1800000180",
        ] {
            let response = request(
                server.local_addr(),
                &format!("GET {target} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert!(response.starts_with("HTTP/1.1 200"), "{target}: {response}");
        }

        let default_details = request(
            server.local_addr(),
            "GET /v3/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let selected_details = request(
            server.local_addr(),
            "GET /v3/details?account=account-13 HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let default_pair = published_pair(&default_details).to_owned();
        let selected_pair = published_pair(&selected_details).to_owned();
        assert_ne!(default_pair, selected_pair);
        assert!(default_pair.starts_with("v1:"));
        assert!(selected_pair.starts_with("v1:"));
        assert_eq!(default_pair.len(), 67);
        assert_eq!(selected_pair.len(), 67);
        assert_eq!(&default_pair[3..19], format!("{:016x}", 7));
        assert_eq!(&selected_pair[3..19], format!("{:016x}", 13));
        assert_eq!(&default_pair[19..35], &selected_pair[19..35]);
        assert_ne!(&default_pair[35..], &selected_pair[35..]);
        let cross_account_conditional = request(
            server.local_addr(),
            &format!(
                "GET /v3/details?account=account-13 HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{default_pair}\"\r\n\r\n"
            ),
        );
        assert!(cross_account_conditional.starts_with("HTTP/1.1 200"));

        // A recorder rewrite that accidentally keeps the same generation is
        // still a new content identity.  Simulate the process boundary by
        // restarting REST after changing only the durable projection value,
        // then prove the old pair cannot produce a false 304.
        server.shutdown();
        let connection = Connection::open(&path_b).expect("account B rewrite db");
        connection
            .execute("UPDATE usage_history SET sol_dollars=9.0", [])
            .expect("same-generation content rewrite");
        drop(connection);
        let reader_a = DbReader::open(&path_a).expect("restarted account A reader");
        let reader_b = DbReader::open(&path_b).expect("restarted account B reader");
        let mut restarted = RestServer::start_with_accounts(
            vec![
                AccountReader::new("account-7", 7, true, Some(1_800_000_000), None, reader_a),
                AccountReader::new("account-13", 13, false, None, None, reader_b),
            ],
            "account-7",
            "127.0.0.1:0".parse().unwrap(),
        )
        .expect("restarted multi-account server");
        let same_account_changed = request(
            restarted.local_addr(),
            &format!(
                "GET /v3/details?account=account-13 HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{selected_pair}\"\r\n\r\n"
            ),
        );
        assert!(same_account_changed.starts_with("HTTP/1.1 200"));
        let rewritten_pair = published_pair(&same_account_changed);
        assert_ne!(rewritten_pair, selected_pair);
        assert_eq!(&rewritten_pair[3..19], &selected_pair[3..19]);
        assert_eq!(&rewritten_pair[19..35], &selected_pair[19..35]);
        assert_ne!(&rewritten_pair[35..], &selected_pair[35..]);

        for target in [
            "/v3/details?account=account-999",
            "/v3/details?account=account-7&account=account-13",
            "/v3/details?account=not-an-account",
            "/v3/details?unknown=1",
            "/v3/accounts?account=account-7",
        ] {
            let response = request(
                restarted.local_addr(),
                &format!("GET {target} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert!(response.starts_with("HTTP/1.1 400"), "{target}: {response}");
        }

        restarted.shutdown();
        fs::remove_file(path_a).expect("account A cleanup");
        fs::remove_file(path_b).expect("account B cleanup");
    }

    #[test]
    fn only_incomplete_ranges_mark_details_degraded_without_clearing_data() {
        let path = temp_db("pending-ranges-wire");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("reader");
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).expect("server");

        let initial = ["v1", "v2", "v3"].map(|version| {
            let wire_request = format!("GET /{version}/details HTTP/1.1\r\nHost:x\r\n\r\n");
            let response = request(server.local_addr(), &wire_request);
            assert!(response.starts_with("HTTP/1.1 200"));
            serde_json::from_str::<serde_json::Value>(body(&response)).expect("initial JSON")
        });
        for value in &initial {
            assert_eq!(value["state"], "ready");
        }

        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute_batch(
                "CREATE TABLE session_pending_ranges(
                    source_id TEXT NOT NULL, range_start INTEGER NOT NULL,
                    complete INTEGER NOT NULL
                );
                INSERT INTO session_pending_ranges VALUES('complete', 0, 1);",
            )
            .expect("pending fixture");
        for version in ["v1", "v2", "v3"] {
            let wire_request = format!("GET /{version}/details HTTP/1.1\r\nHost:x\r\n\r\n");
            let response = request(server.local_addr(), &wire_request);
            let value: serde_json::Value =
                serde_json::from_str(body(&response)).expect("complete diagnostic JSON");
            assert_eq!(value["state"], "ready");
        }
        connection
            .execute(
                "INSERT INTO session_pending_ranges VALUES('incomplete', 1, 0)",
                [],
            )
            .expect("incomplete fixture");
        for (version, previous) in ["v1", "v2", "v3"].into_iter().zip(initial.iter()) {
            let wire_request = format!("GET /{version}/details HTTP/1.1\r\nHost:x\r\n\r\n");
            let response = request(server.local_addr(), &wire_request);
            let value: serde_json::Value =
                serde_json::from_str(body(&response)).expect("pending JSON");
            assert_eq!(value["state"], "error");
            assert_eq!(value["history_samples"], previous["history_samples"]);
        }

        connection
            .execute("DELETE FROM session_pending_ranges", [])
            .expect("clear pending fixture");
        let recovered = request(
            server.local_addr(),
            "GET /v3/details HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let recovered: serde_json::Value =
            serde_json::from_str(body(&recovered)).expect("recovered JSON");
        assert_eq!(recovered["state"], "ready");
        server.shutdown();
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn non_loopback_bind_is_rejected() {
        let path = temp_db("bind");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("reader");
        let error = match RestServer::start(reader, "0.0.0.0:0".parse().unwrap()) {
            Ok(_) => panic!("public bind unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(matches!(error, RestServerError::NonLoopbackAddress));
        fs::remove_file(path).expect("cleanup");
    }
}

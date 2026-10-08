//! Standalone, loopback-only REST process.
//!
//! The process owns an in-memory publication cache and a read-only database
//! reader.  It never imports the recorder, writer, Session, Slint, or root
//! `codex_info` crates.  A failed candidate read leaves the last complete
//! generation in place and marks the store degraded for diagnostics.

mod diagnostics;
mod history;

use codex_info_db_reader::{DbReader, DbSnapshot, ReaderError};
use codex_info_rest_contract::{
    PublicAccountV3, PublicAccountsV3, PublicDetails, PublicDetailsV2, PublicDetailsV3,
    PublicRuntimeVersions, PublicState, API_VERSION, API_VERSION_V2, API_VERSION_V3,
};
use diagnostics::FailureLog;
use history::{HistoryIndex, PeriodIndex};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
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

#[cfg(test)]
thread_local! {
    static HISTORY_ROW_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_history_row_visit() {
    HISTORY_ROW_VISITS.with(|count| count.set(count.get() + 1));
}

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
    history_index: HistoryIndex,
}

impl PublishedSnapshot {
    fn response_pair(&self, degraded: bool) -> Cow<'_, str> {
        if !degraded {
            return Cow::Borrowed(&self.pair);
        }
        // Health is part of the published representation even when the DB
        // generation has not changed. A separate namespace lets existing
        // clients accept both the error and its recovery without comparing
        // their counters or retaining an error body behind a healthy 304.
        let mut hash = Sha256::new();
        hash.update(b"codex-info-rest-degraded-v1\0");
        hash.update(self.pair.as_bytes());
        Cow::Owned(format!("v1:{:x}", hash.finalize()))
    }

    fn from_db_for_account(snapshot: DbSnapshot, storage_epoch: u64) -> Self {
        let history_index = HistoryIndex::build(&snapshot, storage_epoch);
        // Preserve the pair shape while binding the stable account namespace,
        // generation and the complete reader-validated content identity.
        let hash_prefix = snapshot
            .data_hash
            .get(..32)
            .expect("DbReader data hashes are canonical 64-character hex");
        let pair = format!(
            "v1:{storage_epoch:016x}{:016x}{hash_prefix}",
            snapshot.generation
        );
        Self {
            generation: snapshot.generation,
            data_hash: snapshot.data_hash,
            pair,
            has_pending_ranges: snapshot.has_pending_ranges,
            details: snapshot.details,
            models_v3: snapshot.models_v3,
            history_samples_v3: snapshot.history_samples_v3,
            history_samples_v2: snapshot.history_samples_v2,
            history_index,
        }
    }

    fn account_boundary(state: PublicState, generation: u64) -> Self {
        debug_assert!(state != PublicState::Ready);
        let state_code = match state {
            PublicState::Initializing => 1_u128,
            PublicState::AuthRequired => 2_u128,
            PublicState::Error => 3_u128,
            PublicState::Ready => 0_u128,
        };
        let identity = (state_code << 64) | u128::from(generation);
        let data_hash = format!("{identity:064x}");
        let details = PublicDetails {
            state,
            ..PublicDetails::default()
        };
        Self {
            generation,
            // A state transition changes the opaque namespace, while repeated
            // publications of one state advance the counter. This keeps the
            // Linux freshness gate monotonic without borrowing any prior
            // account's partition identity.
            pair: format!("v1:{:016x}{state_code:016x}{generation:032x}", 0),
            data_hash,
            has_pending_ranges: false,
            details,
            models_v3: Vec::new(),
            history_samples_v3: Vec::new(),
            history_samples_v2: Vec::new(),
            history_index: HistoryIndex::default(),
        }
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
    fn refresh(&self, log: &FailureLog, route: &str) -> RefreshStatus {
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
                    log.record(
                        route,
                        "snapshot_marker",
                        &reader_failure_reason(&error),
                        None,
                    );
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
                log.record(route, "snapshot_read", &reader_failure_reason(&error), None);
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
    pub ownership_intervals: Vec<codex_info_rest_contract::PublicAccountOwnershipInterval>,
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
            ownership_intervals: Vec::new(),
            login_id: None,
            reader,
        }
    }

    pub fn with_ownership_intervals(
        mut self,
        intervals: Vec<codex_info_rest_contract::PublicAccountOwnershipInterval>,
    ) -> Self {
        self.ownership_intervals = intervals;
        self
    }

    pub fn with_login_id(mut self, login_id: Option<String>) -> Self {
        self.login_id = login_id;
        self
    }
}

/// Reader/cache boundary.  The cache is an in-memory last-good snapshot per
/// account partition, not a second persistence authority and is never written
/// to disk.  `new(DbReader)` remains the fixture-compatible single-reader API.
type RecorderVersionReader = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct SnapshotStore {
    default_account_id: Option<String>,
    accounts: BTreeMap<String, AccountStore>,
    account_descriptors: Vec<PublicAccountV3>,
    boundary: RwLock<BoundaryPublication>,
    recorder_version_reader: RwLock<Option<RecorderVersionReader>>,
    diagnostics: FailureLog,
}

#[derive(Debug)]
struct BoundaryPublication {
    generation: u64,
    snapshot: Option<Arc<PublishedSnapshot>>,
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
                ownership_intervals: account.ownership_intervals,
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
            default_account_id: Some(default_account_id.clone()),
            accounts: descriptors.clone(),
        };
        public.validate().map_err(|error| {
            RestServerError::InvalidAccounts(format!("invalid account selector: {error}"))
        })?;
        Ok(Self {
            default_account_id: Some(default_account_id),
            accounts: stores,
            account_descriptors: descriptors,
            boundary: RwLock::new(BoundaryPublication {
                generation: 0,
                snapshot: None,
            }),
            recorder_version_reader: RwLock::new(None),
            diagnostics: FailureLog::default(),
        })
    }

    pub fn new_without_account(state: PublicState) -> Self {
        debug_assert!(state != PublicState::Ready);
        Self {
            default_account_id: None,
            accounts: BTreeMap::new(),
            account_descriptors: Vec::new(),
            boundary: RwLock::new(BoundaryPublication {
                generation: 1,
                snapshot: Some(Arc::new(PublishedSnapshot::account_boundary(state, 1))),
            }),
            recorder_version_reader: RwLock::new(None),
            diagnostics: FailureLog::default(),
        }
    }

    pub fn set_log_data_root(&self, root: &std::path::Path) {
        self.diagnostics.set_data_root(root);
    }

    pub fn set_recorder_version_reader(
        &self,
        reader: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) {
        *self
            .recorder_version_reader
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(reader));
    }

    fn runtime_versions(&self) -> PublicRuntimeVersions {
        let reader = self
            .recorder_version_reader
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        PublicRuntimeVersions::new(
            env!("CODEX_INFO_PRODUCT_VERSION"),
            reader.and_then(|read| read()),
        )
    }

    pub fn reader(&self) -> &DbReader {
        let default_account_id = self
            .default_account_id
            .as_deref()
            .expect("fixture store has a default account");
        self.accounts
            .get(default_account_id)
            .expect("validated default account")
            .reader()
    }

    pub fn refresh(&self) -> RefreshStatus {
        self.default_account_id
            .as_deref()
            .map(|account_id| self.refresh_account(account_id))
            .unwrap_or(RefreshStatus::Unavailable)
    }

    pub fn refresh_account(&self, account_id: &str) -> RefreshStatus {
        self.refresh_account_for_route(account_id, "snapshot_refresh")
    }

    fn refresh_account_for_route(&self, account_id: &str, route: &str) -> RefreshStatus {
        self.accounts
            .get(account_id)
            .map(|account| account.refresh(&self.diagnostics, route))
            .unwrap_or(RefreshStatus::Unavailable)
    }

    pub fn status(&self) -> StoreStatus {
        self.default_account_id
            .as_deref()
            .map(|account_id| self.status_account(account_id))
            .unwrap_or(StoreStatus {
                generation: None,
                degraded: false,
            })
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
        self.boundary_snapshot().or_else(|| {
            self.default_account_id
                .as_deref()
                .and_then(|account_id| self.snapshot_account(account_id))
        })
    }

    pub fn snapshot_account(&self, account_id: &str) -> Option<Arc<PublishedSnapshot>> {
        self.accounts
            .get(account_id)
            .and_then(AccountStore::snapshot)
    }

    pub fn snapshot_with_status(&self) -> (Option<Arc<PublishedSnapshot>>, bool) {
        if let Some(snapshot) = self.boundary_snapshot() {
            return (Some(snapshot), false);
        }
        self.default_account_id
            .as_deref()
            .map(|account_id| self.snapshot_with_status_account(account_id))
            .unwrap_or((None, false))
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

    pub fn default_account_id(&self) -> Option<&str> {
        self.default_account_id.as_deref()
    }

    pub fn account_descriptors(&self) -> &[PublicAccountV3] {
        &self.account_descriptors
    }

    pub fn public_accounts(&self) -> PublicAccountsV3 {
        let boundary = self.boundary_snapshot().is_some();
        let mut accounts = self.account_descriptors.clone();
        if boundary {
            for account in &mut accounts {
                account.is_current = false;
            }
        }
        PublicAccountsV3 {
            default_account_id: (!boundary)
                .then(|| self.default_account_id.clone())
                .flatten(),
            accounts,
        }
    }

    pub fn publish_account_boundary(&self, state: PublicState) {
        debug_assert!(state != PublicState::Ready);
        let mut boundary = self
            .boundary
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if boundary
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.details.state == state)
        {
            return;
        }
        boundary.generation = boundary.generation.saturating_add(1).max(1);
        let generation = boundary.generation;
        boundary.snapshot = Some(Arc::new(PublishedSnapshot::account_boundary(
            state, generation,
        )));
    }

    pub fn clear_account_boundary(&self) {
        self.boundary
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot = None;
    }

    fn boundary_snapshot(&self) -> Option<Arc<PublishedSnapshot>> {
        self.boundary
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot
            .clone()
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

    /// Start a read-only listener while no Codex account is authenticated.
    /// The store contains no selectable current account and publishes only
    /// the strict empty boundary root until the production supervisor exits
    /// and restarts this process with a freshly admitted account catalog.
    pub fn start_without_account(
        state: PublicState,
        listen_addr: SocketAddr,
    ) -> Result<Self, RestServerError> {
        if !listen_addr.ip().is_loopback() {
            return Err(RestServerError::NonLoopbackAddress);
        }
        if state == PublicState::Ready {
            return Err(RestServerError::InvalidAccounts(
                "a ready REST store requires a current account".to_owned(),
            ));
        }
        let listener = TcpListener::bind(listen_addr).map_err(RestServerError::Bind)?;
        listener
            .set_nonblocking(true)
            .map_err(RestServerError::Listener)?;
        let local_addr = listener.local_addr().map_err(RestServerError::Listener)?;
        let store = Arc::new(SnapshotStore::new_without_account(state));
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
            Err(_) => {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Route {
    Health,
    Runtime,
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
    fn label(self) -> &'static str {
        match self {
            Self::Health => "/v1/health",
            Self::Runtime => "/v1/runtime",
            Self::Details => "/v1/details",
            Self::DetailsV2 => "/v2/details",
            Self::DetailsV3 => "/v3/details",
            Self::AccountsV3 => "/v3/accounts",
            Self::CurrentV3 => "/v3/current",
            Self::HistoryPeriodsV3 => "/v3/history/periods",
            Self::HistoryV3 => "/v3/history",
            Self::ThreadsV3 => "/v3/threads",
        }
    }
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
    cursor: Option<String>,
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
    let mut route_label = "unparsed";
    let request = match read_request(stream) {
        Ok(request) => request,
        Err(ParseError::HeadersTooLarge) => {
            write_json_response(
                stream,
                &store.diagnostics,
                route_label,
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
                &store.diagnostics,
                route_label,
                413,
                error_body("request_body_not_allowed"),
                None,
                false,
            );
            return;
        }
        Err(ParseError::BadRequest) => {
            write_json_response(
                stream,
                &store.diagnostics,
                route_label,
                400,
                error_body("bad_request"),
                None,
                false,
            );
            return;
        }
    };
    route_label = request.route.map(Route::label).unwrap_or("unknown_route");
    let Some(route) = request.route else {
        write_json_response(
            stream,
            &store.diagnostics,
            route_label,
            404,
            error_body("not_found"),
            None,
            false,
        );
        return;
    };
    if request.method != "GET" {
        write_json_response(
            stream,
            &store.diagnostics,
            route_label,
            405,
            error_body("method_not_allowed"),
            None,
            false,
        );
        return;
    }
    if request.body_length > 0 {
        write_json_response(
            stream,
            &store.diagnostics,
            route_label,
            413,
            error_body("request_body_not_allowed"),
            None,
            false,
        );
        return;
    }
    if route == Route::Health {
        write_json_response(
            stream,
            &store.diagnostics,
            route_label,
            200,
            health_body(),
            None,
            false,
        );
        return;
    }
    if route == Route::Runtime {
        match flatten_with_version(API_VERSION, &store.runtime_versions()) {
            Ok(body) => write_json_response(
                stream,
                &store.diagnostics,
                route_label,
                200,
                body,
                None,
                false,
            ),
            Err(error) => write_route_error(stream, &store.diagnostics, route_label, error),
        }
        return;
    }

    if route == Route::AccountsV3 {
        let accounts = store.public_accounts();
        match flatten_with_version(API_VERSION_V3, &accounts) {
            Ok(body) => write_json_response(
                stream,
                &store.diagnostics,
                route_label,
                200,
                body,
                None,
                false,
            ),
            Err(RouteError::Serialization) => write_json_response(
                stream,
                &store.diagnostics,
                route_label,
                500,
                error_body("serialization_failed"),
                None,
                false,
            ),
            Err(_) => unreachable!("account descriptors are validated at startup"),
        }
        return;
    }

    let (snapshot, degraded) = if request.account.is_none() {
        if let Some(snapshot) = store.boundary_snapshot() {
            (Some(snapshot), false)
        } else {
            let Some(account_id) = store.default_account_id() else {
                write_json_response(
                    stream,
                    &store.diagnostics,
                    route_label,
                    503,
                    error_body("snapshot_unavailable"),
                    None,
                    false,
                );
                return;
            };
            let _refresh = store.refresh_account_for_route(account_id, route_label);
            store.snapshot_with_status_account(account_id)
        }
    } else {
        let account_id = request.account.as_deref().expect("account checked above");
        if !store.has_account(account_id) {
            write_json_response(
                stream,
                &store.diagnostics,
                route_label,
                400,
                error_body("unknown_account"),
                None,
                false,
            );
            return;
        }
        let _refresh = store.refresh_account_for_route(account_id, route_label);
        store.snapshot_with_status_account(account_id)
    };
    let Some(snapshot) = snapshot else {
        write_json_response(
            stream,
            &store.diagnostics,
            route_label,
            503,
            error_body("snapshot_unavailable"),
            None,
            false,
        );
        return;
    };
    let response_pair = if route.v3() {
        snapshot.response_pair(degraded)
    } else {
        Cow::Borrowed(snapshot.pair.as_str())
    };
    if !degraded
        && route.v3()
        && request
            .if_none_match
            .as_deref()
            .is_some_and(|pair| pair == response_pair)
    {
        // A matching pair cannot suppress validation of a stale history cursor.
        if route == Route::HistoryV3 {
            if let Err(error) = history_selection(
                &snapshot,
                request.period.as_deref(),
                request.cursor.as_deref(),
            ) {
                write_route_error(stream, &store.diagnostics, route_label, error);
                return;
            }
        }
        write_json_response(
            stream,
            &store.diagnostics,
            route_label,
            304,
            Vec::new(),
            Some(response_pair.as_ref()),
            true,
        );
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
        Ok(body) => {
            if matches!(
                route,
                Route::Details | Route::DetailsV2 | Route::DetailsV3 | Route::CurrentV3
            ) && (degraded || snapshot.details.state == PublicState::Error)
            {
                let reason = if degraded && snapshot.has_pending_ranges {
                    "incomplete_source_or_acquisition"
                } else if degraded {
                    "last_good_after_refresh_failure"
                } else if snapshot.details.authenticated {
                    "incomplete_source_or_acquisition"
                } else {
                    "account_boundary_error"
                };
                store
                    .diagnostics
                    .record(route_label, "publication_state", reason, Some(200));
            }
            write_json_response(
                stream,
                &store.diagnostics,
                route_label,
                200,
                body,
                Some(response_pair.as_ref()),
                false,
            )
        }
        Err(error) => write_route_error(stream, &store.diagnostics, route_label, error),
    }
}

fn write_route_error(stream: &mut TcpStream, log: &FailureLog, route: &str, error: RouteError) {
    match error {
        RouteError::StaleCursor => write_json_response(
            stream,
            log,
            route,
            400,
            error_body("stale_cursor"),
            None,
            false,
        ),
        RouteError::Serialization => write_json_response(
            stream,
            log,
            route,
            500,
            error_body("serialization_failed"),
            None,
            false,
        ),
        RouteError::UnknownPeriod => write_json_response(
            stream,
            log,
            route,
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
    let authority = stream
        .local_addr()
        .map_err(|_| ParseError::BadRequest)?
        .to_string();
    if header(&headers, "host") != Some(authority.as_str()) {
        return Err(ParseError::BadRequest);
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

type ParsedTarget = (
    Option<Route>,
    Option<String>,
    Option<String>,
    Option<String>,
);

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
        "/v1/runtime" => Some(Route::Runtime),
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
                cursor = Some(value.to_owned());
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
    cursor: Option<String>,
    degraded: bool,
) -> Result<Vec<u8>, RouteError> {
    let details = &snapshot.details;
    match route {
        Route::Health => Ok(health_body()),
        Route::Runtime => unreachable!("runtime versions do not use a database snapshot"),
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
                "open_session_thread_count": details.threads.len(),
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
    details.threads = legacy_running_rows(&details.threads);
    details.active_thread_count = details.threads.len() as u64;
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
    details.threads = legacy_running_rows(&details.threads);
    details.active_thread_count = details.threads.len() as u64;
    if degraded {
        details.state = PublicState::Error;
    }
    details
}

fn legacy_running_rows(
    threads: &[codex_info_rest_contract::PublicThread],
) -> Vec<codex_info_rest_contract::PublicThread> {
    threads
        .iter()
        .filter(|thread| {
            matches!(
                thread.activity_status,
                None | Some(codex_info_rest_contract::PublicThreadActivityStatus::Running)
            )
        })
        .cloned()
        .map(|mut thread| {
            thread.activity_status = None;
            thread
        })
        .collect()
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

fn history_selection<'a>(
    snapshot: &'a PublishedSnapshot,
    period: Option<&str>,
    cursor: Option<&str>,
) -> Result<(&'a PeriodIndex, usize), RouteError> {
    let period = period.ok_or(RouteError::UnknownPeriod)?;
    let indexed = snapshot
        .history_index
        .period(period)
        .ok_or(if cursor.is_some() {
            RouteError::StaleCursor
        } else {
            RouteError::UnknownPeriod
        })?;
    let start = match cursor {
        Some(cursor) => indexed
            .start_after(&snapshot.history_samples_v3, cursor)
            .ok_or(RouteError::StaleCursor)?,
        None => 0,
    };
    Ok((indexed, start))
}

fn serialize_history(
    snapshot: &PublishedSnapshot,
    period: Option<&str>,
    cursor: Option<String>,
) -> Result<Vec<u8>, RouteError> {
    let (indexed, start) = history_selection(snapshot, period, cursor.as_deref())?;
    let end = start
        .saturating_add(MAX_HISTORY_PAGE)
        .min(indexed.samples.len());
    let samples = indexed.samples[start..end]
        .iter()
        .map(|&row| {
            #[cfg(test)]
            record_history_row_visit();
            &snapshot.history_samples_v3[row]
        })
        .collect::<Vec<_>>();
    let resume_cursor = if end > 0 {
        Some(indexed.resume(&snapshot.history_samples_v3, end - 1))
    } else {
        cursor.clone()
    };
    let next_cursor = if end < indexed.samples.len() {
        resume_cursor.clone()
    } else {
        None
    };
    // The first page carries the complete gap set. Continuations prove that
    // set is unchanged and leave the client's previously accepted gaps intact.
    let history_gaps = if cursor.is_none() {
        indexed
            .gaps
            .iter()
            .map(|&row| &snapshot.details.history_gaps[row])
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    serialize_json(&json!({
        "api_version": API_VERSION_V3,
        "history_samples": samples,
        "history_gaps": history_gaps,
        "next_cursor": next_cursor,
        "resume_cursor": resume_cursor,
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
    log: &FailureLog,
    route: &str,
    status: u16,
    body: Vec<u8>,
    pair: Option<&str>,
    not_modified: bool,
) {
    if status >= 400 {
        let value = serde_json::from_slice::<Value>(&body).ok();
        let failure = value
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("response_failed");
        let stage = match failure {
            "bad_request" | "request_headers_too_large" => "request_parse",
            "snapshot_unavailable" => "snapshot_read",
            "serialization_failed" => "response_generation",
            _ => "route_input",
        };
        log.record(route, stage, failure, Some(status));
    }
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
    if let Err(error) = stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .and_then(|_| stream.flush())
    {
        log.record(
            route,
            "response_write",
            &format!("{:?}", error.kind()),
            Some(status),
        );
    }
}

fn reader_failure_reason(error: &ReaderError) -> String {
    match error {
        ReaderError::Io(error) => format!("database_io_{:?}", error.kind()),
        ReaderError::Sqlite(error) => format!("database_sqlite_{:?}", error.sqlite_error_code()),
        ReaderError::Schema(_) => "database_schema_invalid".to_owned(),
        ReaderError::InvalidValue(_) => "database_value_invalid".to_owned(),
        ReaderError::Contract(_) => "public_projection_invalid".to_owned(),
        ReaderError::TooManyRows(_) => "history_projection_too_large".to_owned(),
    }
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

    #[test]
    fn issue_481_runtime_versions_are_available_without_an_account() {
        let server = RestServer::start_without_account(
            PublicState::AuthRequired,
            "127.0.0.1:0".parse().unwrap(),
        )
        .expect("listener");
        let response = request(
            server.local_addr(),
            "GET /v1/runtime HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let value: Value = serde_json::from_str(body(&response)).unwrap();
        assert_eq!(value["api_version"], "v1");
        assert_eq!(value["rest_version"], env!("CODEX_INFO_PRODUCT_VERSION"));
        assert!(value["recorder_version"].is_null());
        assert_eq!(value["recorder_status"], "unavailable");
    }

    #[test]
    fn issue_362_v3_open_rows_preserve_legacy_active_projection() {
        let make_thread = |id: &str, status| codex_info_rest_contract::PublicThread {
            id: id.to_owned(),
            title: id.to_owned(),
            activity_status: Some(status),
            parent_thread_id: None,
            model: "gpt-5".to_owned(),
            model_label: "SOL".to_owned(),
            total_tokens: None,
            context_usage_tokens: None,
            context_window_tokens: None,
            created_at: Some(1_800_000_000),
            last_user_message_at: None,
            is_subagent: false,
            depth: Some(0),
        };
        let details = PublicDetails {
            active_thread_count: 1,
            threads: vec![
                make_thread(
                    "running",
                    codex_info_rest_contract::PublicThreadActivityStatus::Running,
                ),
                make_thread(
                    "stopped",
                    codex_info_rest_contract::PublicThreadActivityStatus::Stopped,
                ),
            ],
            ..PublicDetails::default()
        };
        let snapshot = PublishedSnapshot {
            generation: 1,
            data_hash: "hash".to_owned(),
            pair: "pair".to_owned(),
            has_pending_ranges: false,
            details,
            models_v3: Vec::new(),
            history_samples_v2: Vec::new(),
            history_samples_v3: Vec::new(),
            history_index: HistoryIndex::default(),
        };
        let json = |route| -> serde_json::Value {
            serde_json::from_slice(&serialize_route(&snapshot, route, None, None, false).unwrap())
                .unwrap()
        };
        let current = json(Route::CurrentV3);
        assert_eq!(current["active_thread_count"], 1);
        assert_eq!(current["open_session_thread_count"], 2);
        let open_rows = json(Route::ThreadsV3);
        assert_eq!(open_rows["threads"].as_array().unwrap().len(), 2);
        assert_eq!(open_rows["threads"][1]["activity_status"], "stopped");
        let legacy = json(Route::Details);
        assert_eq!(legacy["active_thread_count"], 1);
        assert_eq!(legacy["threads"].as_array().unwrap().len(), 1);
        assert!(legacy["threads"][0].get("activity_status").is_none());
        let legacy_v2 = json(Route::DetailsV2);
        assert_eq!(legacy_v2["threads"].as_array().unwrap().len(), 1);
        assert!(legacy_v2["threads"][0].get("activity_status").is_none());
        let v3_details = json(Route::DetailsV3);
        assert_eq!(v3_details["open_session_thread_count"], 2);
        assert_eq!(v3_details["threads"][1]["activity_status"], "stopped");
    }

    #[test]
    fn issue_419_public_pair_counts_only_the_accepted_parent_subtrees() {
        let row = |id: &str, parent: Option<&str>, status| codex_info_rest_contract::PublicThread {
            id: id.to_owned(),
            title: if id == "valid-child" { "未設定" } else { id }.to_owned(),
            activity_status: Some(status),
            parent_thread_id: parent.map(str::to_owned),
            model: "gpt-5.6-sol".to_owned(),
            model_label: "SOL".to_owned(),
            total_tokens: None,
            context_usage_tokens: None,
            context_window_tokens: None,
            created_at: Some(1_800_000_000),
            last_user_message_at: None,
            is_subagent: parent.is_some(),
            depth: parent.map(|_| 1),
        };
        use codex_info_rest_contract::PublicThreadActivityStatus::Running;
        let details = PublicDetails {
            active_thread_count: 3,
            threads: vec![
                row("independent-root", None, Running),
                row("valid-child", Some("valid-parent"), Running),
                row("valid-parent", None, Running),
            ],
            ..PublicDetails::default()
        };
        let snapshot = PublishedSnapshot {
            generation: 1,
            data_hash: "hash".to_owned(),
            pair: "pair".to_owned(),
            has_pending_ranges: false,
            details,
            models_v3: Vec::new(),
            history_samples_v2: Vec::new(),
            history_samples_v3: Vec::new(),
            history_index: HistoryIndex::default(),
        };
        let json = |route| -> serde_json::Value {
            serde_json::from_slice(&serialize_route(&snapshot, route, None, None, false).unwrap())
                .unwrap()
        };
        let current = json(Route::CurrentV3);
        assert_eq!(current["open_session_thread_count"], 3);
        assert_eq!(current["active_thread_count"], 3);
        let threads = json(Route::ThreadsV3);
        assert_eq!(
            threads["threads"]
                .as_array()
                .unwrap()
                .iter()
                .map(|thread| thread["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["independent-root", "valid-child", "valid-parent"]
        );
        assert_eq!(threads["threads"][1]["parent_thread_id"], "valid-parent");
        let combined = json(Route::DetailsV3);
        assert_eq!(combined["open_session_thread_count"], 3);
        assert_eq!(combined["active_thread_count"], 3);
        assert_eq!(combined["threads"], threads["threads"]);
    }

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
        // Existing HTTP fixtures use x as an authority placeholder. Bind it
        // to the actual ephemeral listener instead of accepting a foreign Host.
        let request = request.replace("Host:x\r\n", &format!("Host:{address}\r\n"));
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

    fn history_test_fixture(
        epoch: u64,
        generation: u64,
        count: usize,
        corrected: bool,
        gap: bool,
        period_id: &str,
    ) -> PublishedSnapshot {
        let start = 1_800_000_000;
        let reset = 1_802_678_400;
        let end = start + count.saturating_sub(1) as i64 * 60;
        let mut details = PublicDetails {
            state: PublicState::Ready,
            authenticated: true,
            observed_at: Some(end),
            ..PublicDetails::default()
        };
        details
            .history_periods
            .push(codex_info_rest_contract::PublicHistoryPeriod {
                id: period_id.to_owned(),
                start_at: start - 600,
                end_at: end,
                reset_at: reset,
                label: "fixture".to_owned(),
                current: true,
            });
        if gap {
            details
                .history_gaps
                .push(codex_info_rest_contract::PublicHistoryGap {
                    gap_id: "fixture-gap".to_owned(),
                    reset_at: reset,
                    start_at: start - 120,
                    end_at: start - 60,
                    reason: "recorder-stopped".to_owned(),
                });
        }
        PublishedSnapshot::from_db_for_account(
            DbSnapshot {
                generation,
                data_hash: format!("{generation:064x}"),
                has_pending_ranges: false,
                details,
                models_v3: Vec::new(),
                history_samples_v2: Vec::new(),
                history_samples_v3: (0..count)
                    .map(|i| codex_info_rest_contract::PublicHistoryObservationV3 {
                        timestamp: start + i as i64 * 60,
                        reset_at: reset,
                        remaining_percent: Some(if corrected && i == 0 { 70.0 } else { 80.0 }),
                        task_active_since_previous: None,
                        models: None,
                        models_complete: false,
                        model_source: "legacy-unknown".to_owned(),
                    })
                    .collect(),
            },
            epoch,
        )
    }

    fn history_test_page(
        snapshot: &PublishedSnapshot,
        period: &str,
        cursor: Option<&str>,
    ) -> Result<Value, RouteError> {
        let target = match cursor {
            Some(cursor) => format!("/v3/history?period={period}&cursor={cursor}"),
            None => format!("/v3/history?period={period}"),
        };
        let (_, _, _, cursor) = parse_target(&target).map_err(|_| RouteError::StaleCursor)?;
        let bytes = serialize_route(snapshot, Route::HistoryV3, Some(period), cursor, false)?;
        serde_json::from_slice(&bytes).map_err(|_| RouteError::Serialization)
    }

    fn history_test_resume(snapshot: &PublishedSnapshot) -> String {
        let mut cursor = None;
        loop {
            let page = history_test_page(snapshot, "1802678400", cursor.as_deref()).unwrap();
            if page["next_cursor"].is_null() {
                return page["resume_cursor"].as_str().unwrap().to_owned();
            }
            cursor = Some(page["next_cursor"].as_str().unwrap().to_owned());
        }
    }

    #[test]
    fn history_cursor_rejects_changed_prefix_gap_account_and_period() {
        let original = history_test_fixture(10, 1, 2, false, false, "1802678400");
        let cursor = history_test_resume(&original);
        let cases = [
            (
                "sample correction",
                history_test_fixture(10, 2, 3, true, false, "1802678400"),
                "1802678400",
            ),
            (
                "gap changed",
                history_test_fixture(10, 2, 3, false, true, "1802678400"),
                "1802678400",
            ),
            (
                "different account",
                history_test_fixture(11, 2, 3, false, false, "1802678400"),
                "1802678400",
            ),
            (
                "different period",
                history_test_fixture(10, 2, 3, false, false, "other-period"),
                "other-period",
            ),
        ];
        let rejected = cases
            .iter()
            .map(|(name, snapshot, period)| {
                let result = history_test_page(snapshot, period, Some(&cursor));
                println!("{name}: {result:?}");
                matches!(result, Err(RouteError::StaleCursor))
            })
            .collect::<Vec<_>>();
        assert_eq!(rejected, vec![true, true, true, true]);
    }

    fn history_test_edit(
        snapshot: PublishedSnapshot,
        storage_epoch: u64,
        edit: impl FnOnce(&mut DbSnapshot),
    ) -> PublishedSnapshot {
        let generation = snapshot.generation + 1;
        let mut candidate = DbSnapshot {
            generation,
            data_hash: format!("{generation:064x}"),
            has_pending_ranges: snapshot.has_pending_ranges,
            details: snapshot.details,
            models_v3: snapshot.models_v3,
            history_samples_v2: snapshot.history_samples_v2,
            history_samples_v3: snapshot.history_samples_v3,
        };
        edit(&mut candidate);
        PublishedSnapshot::from_db_for_account(candidate, storage_epoch)
    }

    #[test]
    fn history_cursor_rejects_deleted_prefix_and_gap_recovery_or_correction() {
        let original = history_test_fixture(10, 1, 2, false, true, "1802678400");
        let cursor = history_test_resume(&original);
        let deleted_prefix = history_test_edit(original.clone(), 10, |snapshot| {
            snapshot.history_samples_v3.remove(0);
        });
        let recovered_gap = history_test_edit(original.clone(), 10, |snapshot| {
            snapshot.details.history_gaps.clear();
        });
        let corrected_gap = history_test_edit(original.clone(), 10, |snapshot| {
            snapshot.details.history_gaps[0].end_at -= 1;
        });
        let changed_reset = history_test_edit(original.clone(), 10, |snapshot| {
            snapshot.details.history_periods[0].reset_at += 60;
        });
        for snapshot in [
            &deleted_prefix,
            &recovered_gap,
            &corrected_gap,
            &changed_reset,
        ] {
            assert_eq!(
                history_test_page(snapshot, "1802678400", Some(&cursor)),
                Err(RouteError::StaleCursor)
            );
        }
        for malformed in [
            "2".to_owned(),
            "h1.invalid".to_owned(),
            cursor.replacen("h1.", "h2.", 1),
            format!("{cursor}.extra"),
            cursor.replacen("1800000060", "1800000061", 1),
        ] {
            assert_eq!(
                history_test_page(&original, "1802678400", Some(&malformed)),
                Err(RouteError::StaleCursor)
            );
        }
        let metadata_only = history_test_edit(original, 10, |snapshot| {
            snapshot.details.history_periods[0].label = "renamed".to_owned();
            snapshot.details.history_periods[0].current = false;
        });
        let empty_delta = history_test_page(&metadata_only, "1802678400", Some(&cursor)).unwrap();
        assert_eq!(empty_delta["history_samples"].as_array().unwrap().len(), 0);
        assert_eq!(empty_delta["resume_cursor"], cursor);
    }

    #[test]
    fn history_cursor_for_an_unknown_period_is_stale() {
        let original = history_test_fixture(10, 1, 2, false, false, "1802678400");
        let cursor = history_test_resume(&original);
        assert_eq!(
            history_test_page(&original, "unknown-period", Some(&cursor)),
            Err(RouteError::StaleCursor)
        );
        assert_eq!(
            history_test_page(&original, "unknown-period", None),
            Err(RouteError::UnknownPeriod)
        );
    }

    #[test]
    fn history_cursor_preserves_append_paging_and_empty_delta() {
        let original = history_test_fixture(10, 1, 2, false, true, "1802678400");
        let cursor = history_test_resume(&original);
        let extended = history_test_fixture(10, 2, 4, false, true, "1802678400");
        let delta = history_test_page(&extended, "1802678400", Some(&cursor)).unwrap();
        assert_eq!(delta["history_samples"].as_array().unwrap().len(), 2);
        assert_eq!(delta["history_samples"][0]["timestamp"], 1_800_000_120_i64);
        assert!(delta["history_gaps"].as_array().unwrap().is_empty());
        let resume = delta["resume_cursor"].as_str().unwrap();
        let empty = history_test_page(&extended, "1802678400", Some(resume)).unwrap();
        assert_eq!(empty["resume_cursor"], resume);
        assert!(empty["history_samples"].as_array().unwrap().is_empty());
        let initial_empty = history_test_fixture(10, 1, 0, false, false, "1802678400");
        assert!(
            history_test_page(&initial_empty, "1802678400", None).unwrap()["resume_cursor"]
                .is_null()
        );
    }

    #[test]
    fn history_stale_cursor_is_not_hidden_by_matching_etag() {
        let path = temp_db("history-stale-etag");
        fixture(&path, 10);
        let reader = DbReader::open(&path).unwrap();
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).unwrap();
        let address = server.local_addr();
        let head = request(
            address,
            "GET /v3/history?period=1800000060 HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(head.starts_with("HTTP/1.1 200"));
        let pair = published_pair(&head);
        let stale = request(address, &format!(
            "GET /v3/history?period=1800000060&cursor=999999 HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{pair}\"\r\n\r\n"
        ));
        assert!(stale.starts_with("HTTP/1.1 400"), "{stale}");
        assert_eq!(
            serde_json::from_str::<Value>(body(&stale)).unwrap(),
            json!({"api_version":"v1","error":"stale_cursor"})
        );
        assert!(!stale.contains("Codex-Info-Published-Pair:"));
        server.shutdown();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn history_delta_work_depends_on_suffix_not_retained_prefix() {
        let mut observed = Vec::new();
        for prefix in [1_024_usize, 32_768] {
            let original = history_test_fixture(10, 1, prefix, false, false, "1802678400");
            let cursor = history_test_resume(&original);
            let extended = history_test_fixture(10, 2, prefix + 2, false, false, "1802678400");
            HISTORY_ROW_VISITS.with(|count| count.set(0));
            let started = Instant::now();
            let delta = history_test_page(&extended, "1802678400", Some(&cursor)).unwrap();
            let elapsed = started.elapsed();
            let visits = HISTORY_ROW_VISITS.with(|count| count.get());
            assert_eq!(delta["history_samples"].as_array().unwrap().len(), 2);
            assert_eq!(
                delta["history_samples"][0]["timestamp"],
                1_800_000_000_i64 + prefix as i64 * 60
            );
            // Wire authority prescribes binary key lookup plus only the suffix.
            let bound = (usize::BITS - (prefix + 2).leading_zeros()) as usize + 3;
            println!("prefix={prefix} suffix=2 examined_rows={visits} binary_lookup_plus_suffix_bound={bound} debug_elapsed={elapsed:?}");
            observed.push((visits, bound));
        }
        assert!(
            observed.iter().all(|(visits, bound)| visits <= bound),
            "{observed:?}"
        );
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
        let snapshot = PublishedSnapshot::from_db_for_account(
            DbSnapshot {
                generation: 1,
                data_hash: format!("{:064x}", 1),
                has_pending_ranges: false,
                details,
                models_v3: Vec::new(),
                history_samples_v2: Vec::new(),
                history_samples_v3: history_samples,
            },
            1,
        );

        let first: serde_json::Value = serde_json::from_slice(
            &serialize_history(&snapshot, Some(&reset_at.to_string()), None)
                .expect("first history page"),
        )
        .expect("first page JSON");
        let (_, _, _, cursor) = parse_target(&format!(
            "/v3/history?period={reset_at}&cursor={}",
            first["next_cursor"].as_str().unwrap()
        ))
        .unwrap();
        let second: serde_json::Value = serde_json::from_slice(
            &serialize_history(&snapshot, Some(&reset_at.to_string()), cursor)
                .expect("second history page"),
        )
        .expect("second page JSON");

        assert_eq!(first["history_samples"].as_array().unwrap().len(), 1_024);
        assert_eq!(
            first["history_samples"][1023]["task_active_since_previous"],
            false
        );
        assert!(first["next_cursor"].is_string());
        assert_eq!(second["history_samples"].as_array().unwrap().len(), 1);
        assert_eq!(
            second["history_samples"][0]["task_active_since_previous"],
            true
        );
        assert!(second["resume_cursor"].is_string());
        assert_ne!(first["next_cursor"], second["resume_cursor"]);
    }

    #[test]
    fn idle_quota_rollover_commits_and_updates_rest_current() {
        use codex_info_db_writer::{StoragePartitionIdentity, UsageStore};
        use codex_info_recorder::{QuotaSnapshot, Recorder, RecorderConfig};

        // Fixed anonymous timestamps and percentages are an independent oracle.
        // No Session file, quota poller, account RPC, or production profile is used.
        const OBSERVED_AT: i64 = 2_000_000_040;
        const WINDOW: i64 = 7 * 24 * 60 * 60;
        const NEXT_OBSERVED_AT: i64 = OBSERVED_AT + 120;
        const NEXT_RESET_AT: i64 = OBSERVED_AT - 16 + WINDOW;
        let root = tempfile::tempdir().expect("anonymous idle fixture");
        let sessions = root.path().join("sessions");
        fs::create_dir(&sessions).expect("empty Session directory");
        let database = root.path().join("usage_history.sqlite3");
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".to_owned(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 1,
            partition_id: "33".repeat(32),
        };
        drop(UsageStore::create_partitioned(&database, &identity).expect("fixture DB"));
        let mut recorder = Recorder::open_partitioned(
            RecorderConfig {
                sessions_root: sessions.clone(),
                chunk_bytes: 1024,
            },
            &database,
            &identity,
        )
        .expect("fixture recorder");
        let first = recorder
            .run_cycle_with_quota(Some(QuotaSnapshot {
                observed_at: OBSERVED_AT,
                reset_at: OBSERVED_AT + 24 * 60 * 60,
                window_seconds: WINDOW,
                remaining_percent: Some(25.0),
            }))
            .expect("old quota cycle")
            .expect("quota-only commit");
        assert_eq!(first.accepted_ranges, 0);
        assert_eq!(first.pending_ranges, 0);
        assert_eq!(first.sources_seen, 0);

        let reader_identity = codex_info_db_reader::StoragePartitionIdentity {
            schema_version: identity.schema_version.clone(),
            profile_scope_id: identity.profile_scope_id.clone(),
            account_scope_id: identity.account_scope_id.clone(),
            storage_epoch: identity.storage_epoch,
            partition_id: identity.partition_id.clone(),
        };
        let reader =
            DbReader::open_partitioned(&database, &reader_identity).expect("fixture reader");
        let mut server =
            RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).expect("test REST");
        let before = request(
            server.local_addr(),
            "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(before.starts_with("HTTP/1.1 200"));
        let before_value: Value = serde_json::from_str(body(&before)).expect("old current JSON");
        assert_eq!(before_value["state"], "ready");
        assert_eq!(before_value["observed_at"], OBSERVED_AT);
        assert_eq!(before_value["quota"]["remaining_percent"], 25.0);
        assert_eq!(
            before_value["quota"]["reset_at"],
            OBSERVED_AT + 24 * 60 * 60
        );
        assert_eq!(before_value["quota"]["window_seconds"], WINDOW);

        let rolled = recorder
            .run_cycle_with_quota(Some(QuotaSnapshot {
                observed_at: NEXT_OBSERVED_AT,
                reset_at: NEXT_RESET_AT,
                window_seconds: WINDOW,
                remaining_percent: Some(95.0),
            }))
            .expect("idle rollover cycle")
            .expect("idle quota-only commit");
        assert!(rolled.generation > first.generation);
        assert_eq!(rolled.accepted_ranges, 0);
        assert_eq!(rolled.pending_ranges, 0);
        assert_eq!(rolled.sources_seen, 0);
        assert_eq!(fs::read_dir(&sessions).unwrap().count(), 0);

        let connection = Connection::open(&database).expect("fixture DB assertions");
        let generation: (String, i64, i64, i64) = connection
            .query_row(
                "SELECT data_generation, reset_at, latest_quota_reset_at, window_seconds
                 FROM collection_generation WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(generation.0, rolled.generation.to_string());
        assert_eq!(
            (generation.1, generation.2, generation.3),
            (NEXT_RESET_AT, NEXT_RESET_AT, WINDOW)
        );
        let quota_row: (i64, i64, f64) = connection
            .query_row(
                "SELECT timestamp, reset_at, remaining_percent FROM usage_history
                 ORDER BY timestamp DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(quota_row, (NEXT_OBSERVED_AT, NEXT_RESET_AT, 95.0));
        let event_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(event_count, 0);
        drop(connection);

        server.store().refresh();
        let after = request(
            server.local_addr(),
            "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(after.starts_with("HTTP/1.1 200"));
        let after_value: Value = serde_json::from_str(body(&after)).expect("new current JSON");
        assert_eq!(after_value["state"], "ready");
        assert_eq!(after_value["observed_at"], NEXT_OBSERVED_AT);
        assert_eq!(after_value["quota"]["remaining_percent"], 95.0);
        assert_eq!(after_value["quota"]["reset_at"], NEXT_RESET_AT);
        assert_eq!(after_value["quota"]["window_seconds"], WINDOW);
        assert_ne!(published_pair(&before), published_pair(&after));
        server.shutdown();
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
    fn v3_history_exposes_saved_legacy_totals_without_inventing_components() {
        let path = temp_db("legacy-history-wire");
        fixture(&path, 10);
        Connection::open(&path)
            .expect("fixture")
            .execute("DELETE FROM durable_state", [])
            .expect("remove post-legacy provenance");

        let reader = DbReader::open(&path).expect("reader");
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).expect("server");
        let response = request(
            server.local_addr(),
            "GET /v3/history?period=1800000060 HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 200"));
        let value: serde_json::Value =
            serde_json::from_str(body(&response)).expect("v3 history JSON");
        let sample = &value["history_samples"][0];
        assert_eq!(sample["model_source"], "legacy-unknown");
        assert_eq!(sample["models_complete"], false);
        let models = sample["models"].as_array().expect("legacy model totals");
        for (name, tokens, dollars) in [("SOL", 10_u64, 1.0), ("TERRA", 2, 2.0), ("LUNA", 3, 3.0)] {
            let model = models
                .iter()
                .find(|model| model["model"] == name)
                .expect("saved legacy model");
            assert_eq!(model["total_tokens"], tokens);
            assert_eq!(model["total_dollars"], dollars);
            assert!(model.get("input_tokens").is_none());
            assert!(model.get("cached_input_tokens").is_none());
            assert!(model.get("cache_write_input_tokens").is_none());
            assert!(model.get("output_tokens").is_none());
        }
        server.shutdown();
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn host_authority_is_required_before_snapshot_access() {
        let path = temp_db("host-authority");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("reader");
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).expect("server");
        let address = server.local_addr();
        let duplicate = format!("Host: {address}\r\nHost: {address}\r\n");
        for headers in ["", "Host: example.invalid\r\n", duplicate.as_str()] {
            let response = request(
                address,
                &format!("GET /v1/details HTTP/1.1\r\n{headers}\r\n"),
            );
            assert!(response.starts_with("HTTP/1.1 400"), "{response}");
            assert!(!response.contains("Codex-Info-Published-Pair:"));
            assert_eq!(server.store().status().generation, None);
        }
        let valid = request(
            address,
            &format!("GET /v1/details HTTP/1.1\r\nHost: {address}\r\n\r\n"),
        );
        assert!(valid.starts_with("HTTP/1.1 200"), "{valid}");
        assert_eq!(server.store().status().generation, Some(1));
        server.shutdown();
        fs::remove_file(path).expect("cleanup");
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
    fn issue_134_degraded_pair_recovers_without_database_change() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("usage.sqlite3");
        fixture(&path, 10);
        let mut server = RestServer::start(
            DbReader::open(&path).unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let routes = [
            "/v3/current",
            "/v3/details",
            "/v3/threads",
            "/v3/history/periods",
            "/v3/history?period=1800000060",
        ];
        let initial = request(
            server.local_addr(),
            "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let initial_json: Value = serde_json::from_str(body(&initial)).unwrap();
        let ready_pair = published_pair(&initial).to_owned();
        let moved = path.with_extension("moved");
        fs::rename(&path, &moved).unwrap();
        let mut error_pair = None;
        for route in routes {
            let response = request(
                server.local_addr(),
                &format!(
                    "GET {route} HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{ready_pair}\"\r\n\r\n"
                ),
            );
            assert!(response.starts_with("HTTP/1.1 200"), "{route}");
            let pair = published_pair(&response);
            if let Some(expected) = &error_pair {
                assert_eq!(pair, expected, "split resources must agree");
            } else {
                error_pair = Some(pair.to_owned());
            }
            if route == "/v3/current" {
                let error: Value = serde_json::from_str(body(&response)).unwrap();
                assert_eq!(error["state"], "error");
                assert_eq!(error["models"], initial_json["models"]);
            }
        }
        fs::rename(&moved, &path).unwrap();
        let error_pair = error_pair.unwrap();
        let recovered = request(
            server.local_addr(),
            &format!(
                "GET /v3/current HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{error_pair}\"\r\n\r\n"
            ),
        );
        assert!(
            recovered.starts_with("HTTP/1.1 200"),
            "recovery must replace the cached error, even with unchanged DB: {recovered}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(body(&recovered)).unwrap(),
            initial_json
        );
        assert_eq!(published_pair(&recovered), ready_pair);
        // Linux accepts namespace changes independently of its monotonic counter.
        assert_ne!(&error_pair[3..35], &ready_pair[3..35]);
        assert_eq!(error_pair.len(), ready_pair.len());
        assert!(error_pair[3..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        let unchanged = request(
            server.local_addr(),
            &format!(
                "GET /v3/current HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{ready_pair}\"\r\n\r\n"
            ),
        );
        assert!(unchanged.starts_with("HTTP/1.1 304"));
        server.shutdown();
    }

    #[test]
    fn issue_134_http_success_error_state_is_logged() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private-db-name.sqlite3");
        fixture(&path, 10);
        let mut server = RestServer::start(
            DbReader::open(&path).unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        server.store().set_log_data_root(root.path());
        let wire = "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n";
        let _ = request(server.local_addr(), wire);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE session_pending_ranges(source_id TEXT, range_start INTEGER, complete INTEGER); INSERT INTO session_pending_ranges VALUES('private-source-name',0,0); UPDATE collection_generation SET data_generation='2';").unwrap();
        let pending = request(server.local_addr(), wire);
        assert_eq!(
            serde_json::from_str::<Value>(body(&pending)).unwrap()["state"],
            "error"
        );
        connection
            .execute_batch("DELETE FROM session_pending_ranges; UPDATE collection_generation SET data_generation='3';")
            .unwrap();
        drop(connection);
        let _ = request(server.local_addr(), wire);
        let moved = path.with_extension("moved");
        fs::rename(&path, &moved).unwrap();
        let _ = request(server.local_addr(), wire);
        fs::rename(&moved, &path).unwrap();
        server.store().publish_account_boundary(PublicState::Error);
        let _ = request(server.local_addr(), wire);
        server
            .store()
            .publish_account_boundary(PublicState::Initializing);
        let _ = request(server.local_addr(), wire);
        let log = fs::read_dir(root.path().join("logs/rest"))
            .unwrap()
            .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect::<String>();
        let rows: Vec<Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let publication: Vec<_> = rows
            .iter()
            .filter(|row| row["stage"] == "publication_state")
            .collect();
        assert_eq!(
            publication.len(),
            3,
            "every HTTP200 error state must be diagnosed; ready/initializing are not errors: {log}"
        );
        for (row, reason) in publication.iter().zip([
            "incomplete_source_or_acquisition",
            "last_good_after_refresh_failure",
            "account_boundary_error",
        ]) {
            assert_eq!(row["reason"], reason);
            assert_eq!(row["route"], "/v3/current");
            assert_eq!(row["http_status"], 200);
        }
        assert!(!log.contains("private-db-name"));
        assert!(!log.contains("private-source-name"));
        server.shutdown();
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
                AccountReader::new("account-7", 7, true, Some(1_800_000_000), None, reader_a)
                    .with_ownership_intervals(vec![
                        codex_info_rest_contract::PublicAccountOwnershipInterval {
                            start_at: Some(1_799_999_000),
                            end_at: Some(1_799_999_600),
                        },
                        codex_info_rest_contract::PublicAccountOwnershipInterval {
                            start_at: Some(1_800_000_000),
                            end_at: None,
                        },
                    ]),
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
        assert_eq!(
            accounts["accounts"][0]["ownership_intervals"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            accounts["accounts"][0]["ownership_intervals"][0]["end_at"],
            1_799_999_600_i64
        );
        assert_eq!(
            accounts["accounts"][0]["ownership_intervals"][1]["start_at"],
            1_800_000_000_i64
        );
        assert!(accounts["accounts"][0]["ownership_intervals"][1]["end_at"].is_null());
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
    fn account_boundary_clears_current_account_and_unscoped_quota() {
        let path = temp_db("account-logout-boundary");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("account reader");
        let mut server = RestServer::start_with_accounts(
            vec![AccountReader::new(
                "account-7",
                7,
                true,
                Some(1_800_000_000),
                None,
                reader,
            )],
            "account-7",
            "127.0.0.1:0".parse().unwrap(),
        )
        .expect("account server");

        let ready = request(
            server.local_addr(),
            "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let ready: serde_json::Value =
            serde_json::from_str(body(&ready)).expect("ready current JSON");
        assert_eq!(ready["state"], "ready");
        assert!(ready["quota"].is_object());

        server
            .store()
            .publish_account_boundary(PublicState::AuthRequired);
        let accounts = request(
            server.local_addr(),
            "GET /v3/accounts HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let accounts: serde_json::Value =
            serde_json::from_str(body(&accounts)).expect("logged-out accounts JSON");
        assert!(accounts["default_account_id"].is_null());
        assert_eq!(accounts["accounts"][0]["is_current"], false);

        let logged_out = request(
            server.local_addr(),
            "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let logged_out: serde_json::Value =
            serde_json::from_str(body(&logged_out)).expect("logged-out current JSON");
        assert_eq!(logged_out["state"], "auth_required");
        assert_eq!(logged_out["authenticated"], false);
        assert!(logged_out["observed_at"].is_null());
        assert!(logged_out["quota"].is_null());
        assert!(logged_out["models"].as_array().is_some_and(Vec::is_empty));
        assert_eq!(logged_out["active_thread_count"], 0);

        server.shutdown();
        fs::remove_file(path).expect("account boundary cleanup");
    }

    #[test]
    fn issue_575_legacy_collector_recovery_republishes_ready_without_double_credit() {
        use codex_info_db_writer::{StoragePartitionIdentity, UsageStore};
        use codex_info_recorder::{QuotaSnapshot, Recorder, RecorderConfig};
        use std::io::Write;

        const NOW: i64 = 2_000_000_040;
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        fs::create_dir(&sessions).unwrap();
        let source = sessions.join("one.jsonl");
        let database = root.path().join("history.sqlite3");
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 1,
            partition_id: "33".repeat(32),
        };
        drop(UsageStore::create_partitioned(&database, &identity).unwrap());
        let token = |value: u64, time: i64| {
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "event_msg",
                    "timestamp": chrono::DateTime::<chrono::Utc>::from_timestamp(time, 0).unwrap().to_rfc3339(),
                    "payload": {"type": "token_count", "info": {"total_token_usage": {
                        "total_tokens": value, "input_tokens": value,
                        "cached_input_tokens": 0, "output_tokens": 0
                    }}}
                })
            )
        };
        let model = "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-6-astra\"}}\n";
        fs::write(
            &source,
            format!("{model}{}{}", token(10, NOW - 20), token(20, NOW - 10)),
        )
        .unwrap();
        let open = || {
            Recorder::open_partitioned(
                RecorderConfig {
                    sessions_root: sessions.clone(),
                    chunk_bytes: 4096,
                },
                &database,
                &identity,
            )
            .unwrap()
        };
        let quota = |observed_at| {
            Some(QuotaSnapshot {
                observed_at,
                reset_at: NOW + 3600,
                window_seconds: 7200,
                remaining_percent: Some(72.0),
            })
        };
        let mut recorder = open();
        recorder.run_cycle_with_quota(quota(NOW)).unwrap().unwrap();
        // A physical replacement lacks the old 20-token anchor. Preserve the
        // old inode checkpoint, just as the affected saved profiles do.
        fs::rename(&source, root.path().join("original.jsonl")).unwrap();
        fs::write(&source, format!("{model}{}", token(30, NOW + 30))).unwrap();
        assert_eq!(
            recorder
                .run_cycle_with_quota(quota(NOW + 60))
                .unwrap()
                .unwrap()
                .pending_ranges,
            1
        );
        drop(recorder);
        let connection = Connection::open(&database).unwrap();
        connection
            .execute("DELETE FROM session_token_anchor_recoveries", [])
            .unwrap();
        drop(connection);
        let reader_identity = codex_info_db_reader::StoragePartitionIdentity {
            schema_version: identity.schema_version.clone(),
            profile_scope_id: identity.profile_scope_id.clone(),
            account_scope_id: identity.account_scope_id.clone(),
            storage_epoch: identity.storage_epoch,
            partition_id: identity.partition_id.clone(),
        };
        let reader = DbReader::open_partitioned(&database, &reader_identity).unwrap();
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).unwrap();
        let get = |route: &str| {
            request(
                server.local_addr(),
                &format!("GET {route} HTTP/1.1\r\nHost:x\r\n\r\n"),
            )
        };
        let before = get("/v3/current");
        let initial: Value = serde_json::from_str(body(&before)).unwrap();
        assert_eq!(initial["state"], "error");
        assert_eq!(initial["models"][0]["total_tokens"], 10);
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(format!("{}{}", token(20, NOW + 80), token(25, NOW + 90)).as_bytes())
            .unwrap();
        let mut recorder = open();
        recorder
            .run_cycle_with_quota(quota(NOW + 120))
            .unwrap()
            .unwrap();
        recorder
            .run_cycle_with_quota(quota(NOW + 180))
            .unwrap()
            .unwrap();
        server.store().refresh();
        let after = get("/v3/current");
        let value: Value = serde_json::from_str(body(&after)).unwrap();
        // Independent arithmetic: initial 20-10 plus restored 25-20 = 15.
        assert_eq!(value["state"], "ready");
        assert_eq!(value["models"][0]["total_tokens"], 15);
        assert_eq!(value["quota"]["remaining_percent"], 72.0);
        assert_ne!(published_pair(&before), published_pair(&after));
        let history = get(&format!("/v3/history?period={}", NOW + 3600));
        assert_eq!(published_pair(&history), published_pair(&after));
        let history_value: Value = serde_json::from_str(body(&history)).unwrap();
        let tail = history_value["history_samples"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(tail["models_complete"], true);
        assert_eq!(tail["models"][0]["total_tokens"], 15);
        drop(recorder);
        let mut restarted = open();
        restarted
            .run_cycle_with_quota(quota(NOW + 240))
            .unwrap()
            .unwrap();
        server.store().refresh();
        let final_value: Value = serde_json::from_str(body(&get("/v3/current"))).unwrap();
        assert_eq!(final_value["state"], "ready");
        assert_eq!(final_value["models"][0]["total_tokens"], 15);
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM session_pending_ranges", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        server.shutdown();
    }

    #[test]
    fn issue_575_completed_inventory_publishes_ready_in_same_cycle() {
        use codex_info_db_writer::{StoragePartitionIdentity, UsageStore};
        use codex_info_recorder::{QuotaSnapshot, Recorder, RecorderConfig};
        use std::io::Write;

        const NOW: i64 = 2_000_000_040;
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        fs::create_dir(&sessions).unwrap();
        let source = sessions.join("one.jsonl");
        let database = root.path().join("history.sqlite3");
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 1,
            partition_id: "33".repeat(32),
        };
        drop(UsageStore::create_partitioned(&database, &identity).unwrap());
        let token = |value: u64, time: i64| {
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "event_msg",
                    "timestamp": chrono::DateTime::<chrono::Utc>::from_timestamp(time, 0).unwrap().to_rfc3339(),
                    "payload": {"type": "token_count", "info": {"total_token_usage": {
                        "total_tokens": value, "input_tokens": value,
                        "cached_input_tokens": 0, "output_tokens": 0
                    }}}
                })
            )
        };
        let model = "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-6-astra\"}}\n";
        fs::write(
            &source,
            format!("{model}{}{}", token(10, NOW - 120), token(20, NOW - 60)),
        )
        .unwrap();
        let initial_source_len = fs::metadata(&source).unwrap().len();
        let open = |chunk_bytes| {
            Recorder::open_partitioned(
                RecorderConfig {
                    sessions_root: sessions.clone(),
                    chunk_bytes,
                },
                &database,
                &identity,
            )
            .unwrap()
        };
        let quota = |observed_at| {
            Some(QuotaSnapshot {
                observed_at,
                reset_at: NOW + 3600,
                window_seconds: 7200,
                remaining_percent: Some(72.0),
            })
        };

        let mut initial = open(4096);
        initial.run_cycle_with_quota(quota(NOW)).unwrap().unwrap();
        drop(initial);

        let first_append = token(30, NOW + 60);
        let second_append = token(40, NOW + 120);
        fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(format!("{first_append}{second_append}").as_bytes())
            .unwrap();
        let mut bounded = open(first_append.len() as u64);
        let backlog = bounded
            .run_cycle_with_quota(quota(NOW + 180))
            .unwrap()
            .unwrap();
        assert!(
            backlog.pending_ranges > 0,
            "the small chunk must leave unread source bytes"
        );
        let bounded_checkpoint_offset: i64 = Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT MAX(committed_offset) FROM session_checkpoints WHERE relative_path='one.jsonl'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            bounded_checkpoint_offset as u64,
            initial_source_len + first_append.len() as u64
        );
        drop(bounded);

        let reader_identity = codex_info_db_reader::StoragePartitionIdentity {
            schema_version: identity.schema_version.clone(),
            profile_scope_id: identity.profile_scope_id.clone(),
            account_scope_id: identity.account_scope_id.clone(),
            storage_epoch: identity.storage_epoch,
            partition_id: identity.partition_id.clone(),
        };
        let reader = DbReader::open_partitioned(&database, &reader_identity).unwrap();
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).unwrap();
        let get = |route: &str| {
            request(
                server.local_addr(),
                &format!("GET {route} HTTP/1.1\r\nHost:x\r\n\r\n"),
            )
        };
        let incomplete_response = get("/v3/current");
        let incomplete: Value = serde_json::from_str(body(&incomplete_response)).unwrap();
        assert_eq!(incomplete["state"], "error");
        assert_eq!(incomplete["models"][0]["total_tokens"], 20);
        assert_eq!(incomplete["models"][0]["model"], "gpt-6-astra");

        let generation_before: u64 = Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT CAST(data_generation AS INTEGER) FROM collection_generation WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut recovered = open(4096);
        let caught_up = recovered
            .run_cycle_with_quota(quota(NOW + 240))
            .unwrap()
            .unwrap();
        assert_eq!(caught_up.pending_ranges, 0);
        assert_eq!(caught_up.generation, generation_before + 1);

        let connection = Connection::open(&database).unwrap();
        let published_generation: String = connection
            .query_row(
                "SELECT data_generation FROM collection_generation WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(published_generation, caught_up.generation.to_string());
        let checkpoint_offset: i64 = connection
            .query_row(
                "SELECT MAX(committed_offset) FROM session_checkpoints WHERE relative_path='one.jsonl'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            checkpoint_offset as u64,
            fs::metadata(&source).unwrap().len()
        );
        let pending_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM session_pending_ranges", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(pending_count, 0);
        let (astra_total, models_complete): (String, i64) = connection
            .query_row(
                "SELECT total_tokens, model_set_complete FROM usage_model_history
                 WHERE timestamp=(SELECT MAX(timestamp) FROM usage_model_history)
                   AND model='gpt-6-astra'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(astra_total, "30");
        assert_eq!(models_complete, 1);
        drop(connection);

        server.store().refresh();
        let mut ready_pair = None;
        for route in ["/v1/details", "/v2/details", "/v3/details", "/v3/current"] {
            let response = get(route);
            assert!(response.starts_with("HTTP/1.1 200"), "{route}: {response}");
            let value: Value = serde_json::from_str(body(&response)).unwrap();
            assert_eq!(value["state"], "ready", "{route}");
            let pair = published_pair(&response);
            if let Some(expected) = &ready_pair {
                assert_eq!(
                    pair, expected,
                    "{route} must publish the recovered generation"
                );
            } else {
                ready_pair = Some(pair.to_owned());
            }
            if route == "/v3/current" || route == "/v3/details" {
                assert_eq!(value["models"][0]["model"], "gpt-6-astra");
                assert_eq!(value["models"][0]["total_tokens"], 30);
            }
        }
        let history_response = get(&format!("/v3/history?period={}", NOW + 3600));
        assert_eq!(
            published_pair(&history_response),
            ready_pair.as_deref().unwrap()
        );
        let history: Value = serde_json::from_str(body(&history_response)).unwrap();
        let tail = history["history_samples"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(tail["models_complete"], true);
        assert_eq!(tail["models"][0]["model"], "gpt-6-astra");
        assert_eq!(tail["models"][0]["total_tokens"], 30);
        assert_eq!(caught_up.generation, backlog.generation + 1);

        drop(recovered);
        server.shutdown();
    }

    #[test]
    fn issue_575_latest_partial_models_and_recovery_share_one_publication() {
        let log_root = tempfile::tempdir().unwrap();
        let path = temp_db("current-model-integrity");
        fixture(&path, 10);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(
            "CREATE TABLE usage_model_history(
                reset_at INTEGER, timestamp INTEGER, model TEXT, total_tokens TEXT,
                input_tokens TEXT, cached_input_tokens TEXT, output_tokens TEXT,
                cache_write_input_tokens TEXT, model_set_complete INTEGER);
             INSERT INTO usage_model_history VALUES
                (1800000060,1800000000,'ASTRA','14534996','14493439','14201984','41557','0',1);
             UPDATE durable_state SET snapshot_json=replace(snapshot_json,'legacy-unknown','confirmed');"
        ).unwrap();
        // Exercise the production canonical-account reader: legacy aliases may
        // legitimately reassign a prior reset's boundary sample to a new window.
        connection
            .pragma_update(
                None,
                "user_version",
                codex_info_db_reader::HISTORY_CANONICAL_SCHEMA_VERSION,
            )
            .unwrap();
        connection.execute_batch(
            "CREATE UNIQUE INDEX usage_history_canonical_timestamp_idx ON usage_history(timestamp);
             CREATE UNIQUE INDEX usage_model_history_canonical_timestamp_model_idx ON usage_model_history(timestamp,model);
             CREATE TABLE storage_partition(singleton INTEGER, schema_version TEXT,
                 profile_scope_id TEXT, account_scope_id TEXT, storage_epoch TEXT, partition_id TEXT);"
        ).unwrap();
        for (name, table, operation) in [
            (
                "usage_history_canonical_insert_guard",
                "usage_history",
                "INSERT",
            ),
            (
                "usage_history_canonical_update_guard",
                "usage_history",
                "UPDATE",
            ),
            (
                "usage_model_history_canonical_insert_guard",
                "usage_model_history",
                "INSERT",
            ),
            (
                "usage_model_history_canonical_update_guard",
                "usage_model_history",
                "UPDATE",
            ),
            (
                "durable_history_observation_insert_guard",
                "durable_state",
                "INSERT",
            ),
            (
                "durable_history_observation_update_guard",
                "durable_state",
                "UPDATE",
            ),
            (
                "usage_history_sidecar_update_guard",
                "usage_history",
                "UPDATE",
            ),
            (
                "usage_history_sidecar_delete_guard",
                "usage_history",
                "DELETE",
            ),
        ] {
            connection
                .execute_batch(&format!(
                "CREATE TRIGGER {name} BEFORE {operation} ON {table} WHEN 0 BEGIN SELECT 1; END;"
            ))
                .unwrap();
        }
        let identity = codex_info_db_reader::StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".to_owned(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 2,
            partition_id: "33".repeat(32),
        };
        connection
            .execute(
                "INSERT INTO storage_partition VALUES(1,?1,?2,?3,'2',?4)",
                rusqlite::params![
                    identity.schema_version,
                    identity.profile_scope_id,
                    identity.account_scope_id,
                    identity.partition_id
                ],
            )
            .unwrap();
        let reader = DbReader::open_partitioned(&path, &identity).unwrap();
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).unwrap();
        server.store().set_log_data_root(log_root.path());
        let get = |route: &str| {
            request(
                server.local_addr(),
                &format!("GET {route} HTTP/1.1\r\nHost:x\r\n\r\n"),
            )
        };
        let initial: Value = serde_json::from_str(body(&get("/v3/current"))).unwrap();
        assert_eq!(initial["state"], "ready");
        assert_eq!(initial["models"][0]["total_tokens"], 14_534_996);
        connection.execute_batch(
            "INSERT INTO usage_history VALUES(1800000060,1800000060,5,0,0,0,0,0,0);
             INSERT INTO usage_model_history VALUES
                (1800000060,1800000060,'ASTRA','201416126','200878864','197512320','537262','0',0);
             INSERT INTO durable_state VALUES(3,1800000060,'partial',
                '{\"kind\":\"codex-info-usage-observation-v1\",\"timestamp\":1800000060,\"reset_at\":1800000060,\"remaining_percent\":5,\"model_source\":\"confirmed\"}');
             CREATE TABLE session_pending_ranges(source_id TEXT, range_start INTEGER, complete INTEGER);
             INSERT INTO session_pending_ranges VALUES('anchor-evidence',0,1);
             UPDATE collection_generation SET data_generation='2';"
        ).unwrap();
        let mut error_pair = String::new();
        for route in ["/v1/details", "/v2/details", "/v3/details", "/v3/current"] {
            let response = get(route);
            let value: Value = serde_json::from_str(body(&response)).unwrap();
            assert_eq!(value["state"], "error", "{route}");
            assert_eq!(value["observed_at"], 1_800_000_060);
            assert_eq!(value["quota"]["remaining_percent"], 5.0);
            if route.starts_with("/v3/") {
                assert_eq!(value["models"][0]["total_tokens"], 201_416_126);
                let dollars = value["models"][0]["estimated_cost"]["total_dollars"]
                    .as_f64()
                    .unwrap();
                assert!((dollars - 258.04086).abs() < 1e-9);
            }
            let pair = published_pair(&response);
            if error_pair.is_empty() {
                error_pair = pair.to_owned();
            }
            assert_eq!(pair, error_pair);
        }
        let history = get("/v3/history?period=1800000060");
        assert_eq!(published_pair(&history), error_pair);
        let history: Value = serde_json::from_str(body(&history)).unwrap();
        let tail = history["history_samples"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(tail["models_complete"], false);
        assert_eq!(tail["models"][0]["total_tokens"], 201_416_126);
        let log = fs::read_dir(log_root.path().join("logs/rest"))
            .unwrap()
            .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect::<String>();
        let publications: Vec<Value> = log
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|row| row["stage"] == "publication_state")
            .collect();
        assert_eq!(publications.len(), 4);
        for row in publications {
            assert_eq!(row["reason"], "incomplete_source_or_acquisition");
        }
        connection
            .execute_batch(
                "UPDATE usage_model_history SET model_set_complete=1 WHERE timestamp=1800000060;
             DELETE FROM session_pending_ranges;
             UPDATE collection_generation SET data_generation='3';",
            )
            .unwrap();
        let recovered = request(
            server.local_addr(),
            &format!(
                "GET /v3/current HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{error_pair}\"\r\n\r\n"
            ),
        );
        assert!(recovered.starts_with("HTTP/1.1 200"));
        assert_ne!(published_pair(&recovered), error_pair);
        let recovered: Value = serde_json::from_str(body(&recovered)).unwrap();
        assert_eq!(recovered["state"], "ready");
        assert_eq!(recovered["models"][0]["total_tokens"], 201_416_126);
        // The first new-period observation is already used: no zero-token or
        // 100% sample is required to switch authority away from the old week.
        connection.execute_batch(
            "UPDATE collection_generation SET data_generation='4', reset_at=1800003660;
             INSERT INTO usage_history VALUES(1800000120,1800003660,98,0,0,0,0,0,0);
             INSERT INTO usage_model_history VALUES
                (1800003660,1800000120,'ASTRA','440','400','0','40','0',1);
             INSERT INTO durable_state VALUES(4,1800000120,'new-period',
                '{\"kind\":\"codex-info-usage-observation-v1\",\"timestamp\":1800000120,\"reset_at\":1800003660,\"remaining_percent\":98,\"model_source\":\"confirmed\"}');"
        ).unwrap();
        let next = get("/v3/current");
        let next_value: Value = serde_json::from_str(body(&next)).unwrap();
        assert_eq!(next_value["state"], "ready");
        assert_eq!(next_value["quota"]["reset_at"], 1_800_003_660);
        assert_eq!(next_value["quota"]["remaining_percent"], 98.0);
        assert_eq!(next_value["models"][0]["total_tokens"], 440);
        let periods = get("/v3/history/periods");
        assert_eq!(published_pair(&periods), published_pair(&next));
        let periods: Value = serde_json::from_str(body(&periods)).unwrap();
        let current = periods["history_periods"]
            .as_array()
            .unwrap()
            .iter()
            .find(|period| period["current"] == true)
            .unwrap();
        assert_eq!(current["reset_at"], 1_800_003_660);
        let next_history = get("/v3/history?period=1800003660");
        assert_eq!(published_pair(&next_history), published_pair(&next));
        let next_history: Value = serde_json::from_str(body(&next_history)).unwrap();
        assert_eq!(next_history["history_samples"].as_array().unwrap().len(), 1);
        assert_eq!(
            next_history["history_samples"][0]["models"][0]["total_tokens"],
            440
        );
        server.shutdown();
        fs::remove_file(path).unwrap();
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

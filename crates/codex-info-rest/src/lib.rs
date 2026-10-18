//! Standalone, loopback-only REST process.
//!
//! The process owns an in-memory publication cache and a read-only database
//! reader.  It never imports the recorder, writer, Session, Slint, or root
//! `codex_info` crates.  A failed candidate read leaves the last complete
//! generation in place and marks the store degraded for diagnostics.

mod diagnostics;
mod history;
mod recordings;

use codex_info_db_reader::{
    CollectionCommit, CurrentPublication, DbReader, DbSnapshot, HistoryArchive, ReaderError,
};
use codex_info_rest_contract::{
    PublicAccountV3, PublicAccountsV3, PublicDetails, PublicDetailsV2, PublicDetailsV3,
    PublicRuntimeVersions, PublicState, PublishedPair, API_VERSION, API_VERSION_V2, API_VERSION_V3,
};
use diagnostics::FailureLog;
use history::{HistoryIndex, PeriodIndex};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_REQUEST_LINE_BYTES: usize = 2_048;
const MAX_HEADER_BYTES: usize = 8 * 1_024;
const MAX_HEADER_COUNT: usize = 32;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_HISTORY_PAGE: usize = 1_024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const READER_FRESHNESS: Duration = Duration::from_secs(60);

/// Already validated by the recorder-state/owner observer. Heartbeat wall
/// clocks are deliberately absent from the reconciliation identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecorderCommitNotice {
    pub pid: u64,
    pub process_starttime: u64,
    pub owner_nonce: String,
    pub partition_id: String,
    pub commit: CollectionCommit,
    pub degraded: bool,
}

/// Fixed diagnostic categories; source values and errors never enter the log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecorderSourceUnavailableReason {
    PublicationUnavailable,
    FileInvalid,
    FileNotPrivate,
    ReadFailed,
    JsonInvalid,
    IdentityInvalid,
    OwnerNotLive,
    PublicationStale,
    CommitStale,
    InconsistentField,
    PartitionMismatch,
    CommitMissing,
    BeforeActivation,
    Idle,
    Unspecified,
}

impl RecorderSourceUnavailableReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PublicationUnavailable => "publication_unavailable",
            Self::FileInvalid => "file_invalid",
            Self::FileNotPrivate => "file_not_private",
            Self::ReadFailed => "read_failed",
            Self::JsonInvalid => "json_invalid",
            Self::IdentityInvalid => "identity_invalid",
            Self::OwnerNotLive => "owner_not_live",
            Self::PublicationStale => "publication_stale",
            Self::CommitStale => "commit_stale",
            Self::InconsistentField => "inconsistent_field",
            Self::PartitionMismatch => "partition_mismatch",
            Self::CommitMissing => "commit_missing",
            Self::BeforeActivation => "before_activation",
            Self::Idle => "idle",
            Self::Unspecified => "unspecified",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ReaderIntent {
    Independent,
    Unconfirmed,
    Recorder(RecorderCommitNotice),
}

#[derive(Default)]
struct ReaderPending {
    latest: BTreeMap<String, Arc<ReaderIntent>>,
    accounts: BTreeSet<String>,
    history: BTreeMap<String, (Arc<ReaderIntent>, i64)>,
    recordings: BTreeMap<String, (Arc<ReaderIntent>, i64)>,
    freshness: bool,
    stopped: bool,
}

#[derive(Default)]
struct ReaderControl {
    pending: Mutex<ReaderPending>,
    changed: Condvar,
    history_changed: Condvar,
    recordings_changed: Condvar,
}

#[derive(Clone, Copy, PartialEq)]
enum ReaderLane {
    Current,
    History,
    Recordings,
    #[cfg(test)]
    Direct,
}

#[derive(Debug)]
struct PairIssuer {
    epoch: [u8; 16],
    counter: u128,
    failed: Arc<AtomicBool>,
}

impl PairIssuer {
    fn new() -> Result<Self, RestServerError> {
        let mut epoch = [0; 16];
        getrandom::fill(&mut epoch).map_err(|_| RestServerError::EpochUnavailable)?;
        Self::from_epoch(epoch)
    }

    fn from_epoch(epoch: [u8; 16]) -> Result<Self, RestServerError> {
        if epoch == [0; 16] {
            return Err(RestServerError::EpochUnavailable);
        }
        Ok(Self {
            epoch,
            counter: 0,
            failed: Arc::new(AtomicBool::new(false)),
        })
    }

    fn issue(&mut self) -> Option<String> {
        self.issue_batch::<1>().map(|[pair]| pair)
    }

    fn issue_batch<const N: usize>(&mut self) -> Option<[String; N]> {
        if self.failed.load(Ordering::Acquire) {
            return None;
        }
        let Some(counter) = self.counter.checked_add(N as u128) else {
            self.failed.store(true, Ordering::Release);
            return None;
        };
        let pairs = std::array::from_fn(|offset| {
            PublishedPair::from_epoch_counter(self.epoch, self.counter + offset as u128 + 1)
                .as_str()
                .to_owned()
        });
        self.counter = counter;
        Some(pairs)
    }
}

#[cfg(test)]
thread_local! {
    static HISTORY_ROW_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DIRECTORY_ROW_ENCODINGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_history_row_visit() {
    HISTORY_ROW_VISITS.with(|count| count.set(count.get() + 1));
}

#[derive(Debug, Default)]
struct DirectoryPages {
    sequences: Mutex<BTreeMap<(usize, usize), DirectorySequence>>,
}

// Encoded pages are derived, lazy state, never part of the published domain.
impl PartialEq for DirectoryPages {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

#[derive(Debug, Default)]
struct DirectorySequence {
    pages: BTreeMap<usize, Vec<u8>>,
    frontier: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishedSnapshot {
    pub generation: u64,
    pub data_hash: String,
    pub pair: String,
    pub history_available: bool,
    pub has_pending_ranges: bool,
    pub acquisition_degraded: bool,
    pub current: CurrentPublication,
    pub details: PublicDetails,
    pub models_v3: Vec<codex_info_rest_contract::PublicModelUsageV3>,
    pub history_samples_v3: Vec<codex_info_rest_contract::PublicHistoryObservationV3>,
    pub history_samples_v2: Vec<codex_info_rest_contract::PublicHistoryObservation>,
    pub retained_history: HistoryArchive,
    pub recordings: Option<Arc<codex_info_db_reader::RecordedArchive>>,
    recorded_scope_account: u64,
    history_index: HistoryIndex,
    directory_indices: [Vec<usize>; 2],
    directory_pages: Arc<DirectoryPages>,
    reprojection_witness: Option<codex_info_db_reader::ReprojectionWitness>,
    history_reprojection_witness: Option<codex_info_db_reader::ReprojectionWitness>,
}

impl PublishedSnapshot {
    fn with_history(mut self, history: &Self) -> Result<Self, ReaderError> {
        self.history_available = history.history_available;
        self.details
            .history_periods
            .clone_from(&history.details.history_periods);
        self.details
            .history_samples
            .clone_from(&history.details.history_samples);
        self.details
            .history_gaps
            .clone_from(&history.details.history_gaps);
        // Compatibility details combine Current and History. Their current
        // period metadata follows the independently qualified Current time;
        // no observation or model value is added to the history payload.
        if let Some(observed_at) = self.details.observed_at {
            for period in &mut self.details.history_periods {
                if period.current {
                    period.end_at = period.reset_at.min(observed_at);
                }
            }
        }
        self.details.validate_history_domain()?;
        self.history_samples_v3
            .clone_from(&history.history_samples_v3);
        self.history_samples_v2
            .clone_from(&history.history_samples_v2);
        self.retained_history.clone_from(&history.retained_history);
        self.history_index = history.history_index.clone();
        self.directory_indices
            .clone_from(&history.directory_indices);
        self.directory_pages = Arc::clone(&history.directory_pages);
        self.history_reprojection_witness
            .clone_from(&history.history_reprojection_witness);
        let mut hash = Sha256::new();
        hash.update(self.data_hash.as_bytes());
        hash.update(history.data_hash.as_bytes());
        self.data_hash = format!("{:x}", hash.finalize());
        Ok(self)
    }

    fn with_recordings(
        mut self,
        archive: Arc<codex_info_db_reader::RecordedArchive>,
    ) -> Result<Self, ReaderError> {
        let mut hash = Sha256::new();
        hash.update(self.data_hash.as_bytes());
        hash.update(serde_json::to_vec(archive.as_ref()).map_err(|_| {
            ReaderError::InvalidValue("recorded archive serialization failed".into())
        })?);
        self.data_hash = format!("{:x}", hash.finalize());
        self.recordings = Some(archive);
        Ok(self)
    }

    fn candidate_from_db_for_account(snapshot: DbSnapshot, storage_epoch: u64) -> Self {
        let history_index = HistoryIndex::build(&snapshot, storage_epoch);
        let directory_indices = directory_indices(&snapshot.retained_history, &history_index);
        let history_reprojection_witness = snapshot
            .history_available
            .then(|| snapshot.reprojection_witness.clone())
            .flatten();
        Self {
            generation: snapshot.generation,
            data_hash: snapshot.data_hash,
            // A candidate has no public identity until its admission succeeds.
            pair: String::new(),
            has_pending_ranges: snapshot.has_pending_ranges,
            acquisition_degraded: snapshot.acquisition_degraded,
            history_available: snapshot.history_available,
            current: snapshot.current,
            details: snapshot.details,
            models_v3: snapshot.models_v3,
            history_samples_v3: snapshot.history_samples_v3,
            history_samples_v2: snapshot.history_samples_v2,
            retained_history: snapshot.retained_history,
            recordings: snapshot.recordings,
            recorded_scope_account: storage_epoch,
            history_index,
            directory_indices,
            directory_pages: Arc::default(),
            reprojection_witness: snapshot.reprojection_witness,
            history_reprojection_witness,
        }
    }

    #[cfg(test)]
    fn from_db_for_account(snapshot: DbSnapshot, storage_epoch: u64) -> Self {
        use std::sync::atomic::AtomicU64;
        static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(1);
        let mut candidate = Self::candidate_from_db_for_account(snapshot, storage_epoch);
        // Direct serializer fixtures inject one non-secret epoch and distinct
        // successful-publication counters, independently of DB/account values.
        candidate.pair = PublishedPair::from_epoch_counter(
            [0xa5; 16],
            u128::from(FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed)),
        )
        .as_str()
        .to_owned();
        candidate
    }

    fn account_boundary(state: PublicState, generation: u64, pair: String) -> Self {
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
            pair,
            data_hash,
            has_pending_ranges: false,
            acquisition_degraded: false,
            history_available: true,
            current: CurrentPublication::unavailable(details.state),
            details,
            models_v3: Vec::new(),
            history_samples_v3: Vec::new(),
            history_samples_v2: Vec::new(),
            retained_history: HistoryArchive::default(),
            recordings: Some(Arc::default()),
            recorded_scope_account: 0,
            history_index: HistoryIndex::default(),
            directory_indices: Default::default(),
            directory_pages: Arc::default(),
            reprojection_witness: None,
            history_reprojection_witness: None,
        }
    }
}

fn exact_fact<T: serde::Serialize>(left: &T, right: &T) -> bool {
    // Compare every typed field, including optional knownness and binary64
    // signed zero, without reducing the evidence to a digest.
    serde_json::to_vec(left)
        .ok()
        .zip(serde_json::to_vec(right).ok())
        .is_some_and(|(left, right)| left == right)
}

fn common_facts_equal<T: serde::Serialize>(
    left: &[T],
    right: &[T],
    lower: i64,
    upper: i64,
    timestamp: impl Fn(&T) -> i64,
) -> bool {
    let mut left = left
        .iter()
        .filter(|row| timestamp(row) > lower && timestamp(row) <= upper);
    let mut right = right
        .iter()
        .filter(|row| timestamp(row) > lower && timestamp(row) <= upper);
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(left), Some(right)) if exact_fact(left, right) => {}
            _ => return false,
        }
    }
}

fn same_public_domain(previous: &PublishedSnapshot, candidate: &PublishedSnapshot) -> bool {
    if previous.recordings != candidate.recordings
        || previous.history_available != candidate.history_available
        || previous.current.models_status != candidate.current.models_status
        || (if previous.acquisition_degraded {
            PublicState::Error
        } else {
            previous.current.state
        }) != (if candidate.acquisition_degraded {
            PublicState::Error
        } else {
            candidate.current.state
        })
        || !exact_fact(
            &details_v1(previous, previous.acquisition_degraded),
            &details_v1(candidate, candidate.acquisition_degraded),
        )
        || !exact_fact(
            &details_v2_for_wire(previous, previous.acquisition_degraded),
            &details_v2_for_wire(candidate, candidate.acquisition_degraded),
        )
        || !exact_fact(
            &details_v3(previous, previous.acquisition_degraded),
            &details_v3(candidate, candidate.acquisition_degraded),
        )
    {
        return false;
    }
    if !previous.history_available {
        return true;
    }
    let directory = |snapshot: &PublishedSnapshot| {
        snapshot
            .retained_history
            .periods
            .iter()
            .filter_map(|period| {
                let indexed = snapshot.history_index.period(&period.id)?;
                (period.current || !indexed.samples.is_empty() || !indexed.gaps.is_empty()).then(
                    || {
                        (
                            period.clone(),
                            snapshot
                                .retained_history
                                .origins
                                .get(&period.reset_at)
                                .copied(),
                            period.current || indexed.has_token_usage,
                        )
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    if !exact_fact(&directory(previous), &directory(candidate)) {
        return false;
    }
    // An empty, unlisted period is still a known direct lookup. Internal
    // archive row positions and unexposed metadata are not wire identity.
    let periods = |snapshot: &PublishedSnapshot| {
        snapshot
            .retained_history
            .periods
            .iter()
            .map(|period| period.id.clone())
            .collect::<BTreeSet<_>>()
    };
    let previous_periods = periods(previous);
    if previous_periods != periods(candidate) {
        return false;
    }
    previous_periods.iter().all(|id| {
        let (Some(old), Some(new)) = (
            previous.history_index.period(id),
            candidate.history_index.period(id),
        ) else {
            return false;
        };
        old.samples.len() == new.samples.len()
            && old.gaps.len() == new.gaps.len()
            && old.same_cursor_identity(new)
            && old.samples.iter().zip(&new.samples).all(|(&left, &right)| {
                exact_fact(
                    &previous.retained_history.samples[left],
                    &candidate.retained_history.samples[right],
                )
            })
            && old.gaps.iter().zip(&new.gaps).all(|(&left, &right)| {
                exact_fact(
                    &previous.retained_history.gaps[left],
                    &candidate.retained_history.gaps[right],
                )
            })
    })
}

fn history_facts_preserved(previous: &PublishedSnapshot, candidate: &PublishedSnapshot) -> bool {
    let (Some(old), Some(new)) = (
        &previous.history_reprojection_witness,
        &candidate.history_reprojection_witness,
    ) else {
        return false;
    };
    if !old.same_commit_authority(new) || !old.same_common_quota_receipts(new) {
        return false;
    }
    let (lower, upper, recent_lower, recent_upper, gap_lower, gap_upper) =
        match (old.publication_window(), new.publication_window()) {
            (Some(old), Some(new)) => (
                (old.retained_start.div_euclid(60) * 60 - 1)
                    .max(new.retained_start.div_euclid(60) * 60 - 1),
                old.at.min(new.at),
                old.at
                    .saturating_sub(31 * 86_400)
                    .max(new.at.saturating_sub(31 * 86_400)),
                old.at.min(new.at),
                old.retained_start.max(new.retained_start),
                old.at.min(new.at),
            ),
            (None, None) => (i64::MIN, i64::MAX, i64::MIN, i64::MAX, i64::MIN, i64::MAX),
            _ => return false,
        };
    if !common_facts_equal(
        &previous.retained_history.samples,
        &candidate.retained_history.samples,
        lower,
        upper,
        |row| row.timestamp,
    ) || !common_facts_equal(
        &previous.details.history_samples,
        &candidate.details.history_samples,
        recent_lower,
        recent_upper,
        |row| row.timestamp,
    ) || !common_facts_equal(
        &previous.history_samples_v2,
        &candidate.history_samples_v2,
        recent_lower,
        recent_upper,
        |row| row.timestamp,
    ) {
        return false;
    }
    let gaps = |snapshot: &PublishedSnapshot| {
        snapshot
            .retained_history
            .gaps
            .iter()
            .filter_map(|gap| {
                let start = gap.start_at.max(gap_lower);
                let end = gap.end_at.min(gap_upper);
                (start <= end).then(|| {
                    (
                        gap.gap_id.clone(),
                        (gap.reset_at, start, end, gap.reason.clone()),
                    )
                })
            })
            .collect::<BTreeMap<_, _>>()
    };
    if gaps(previous) != gaps(candidate) {
        return false;
    }
    let periods_by_id = candidate
        .retained_history
        .periods
        .iter()
        .map(|period| (&period.id, period))
        .collect::<BTreeMap<_, _>>();
    fn source_facts(
        snapshot: &PublishedSnapshot,
    ) -> BTreeMap<
        i64,
        (
            Vec<&codex_info_rest_contract::PublicHistoryObservationV3>,
            Vec<&codex_info_rest_contract::PublicHistoryGap>,
        ),
    > {
        let mut facts = BTreeMap::<i64, (Vec<_>, Vec<_>)>::new();
        for row in &snapshot.retained_history.samples {
            facts.entry(row.reset_at).or_default().0.push(row);
        }
        for gap in &snapshot.retained_history.gaps {
            facts.entry(gap.reset_at).or_default().1.push(gap);
        }
        facts
    }
    let old_sources = source_facts(previous);
    let new_sources = source_facts(candidate);
    for period in &previous.retained_history.periods {
        if let Some(next) = periods_by_id.get(&period.id) {
            if period.reset_at != next.reset_at || period.current != next.current {
                return false;
            }
            if previous.retained_history.origins.get(&period.reset_at)
                != candidate.retained_history.origins.get(&period.reset_at)
            {
                // Origin qualification may change only with a changed source
                // extent of this exact period, never another period's clock.
                if exact_fact(
                    &old_sources.get(&period.reset_at),
                    &new_sources.get(&period.reset_at),
                ) {
                    return false;
                }
            }
        }
    }
    true
}

fn same_commit_reprojection(
    previous: &PublishedSnapshot,
    candidate: &PublishedSnapshot,
    known_history: Option<&PublishedSnapshot>,
) -> bool {
    let (Some(old), Some(new)) = (
        &previous.reprojection_witness,
        &candidate.reprojection_witness,
    ) else {
        return false;
    };
    if !old.same_commit_authority(new)
        || !new.preserves_previous_current_observation(old)
        || !old.same_common_quota_receipts(new)
        || previous.has_pending_ranges != candidate.has_pending_ranges
        || previous.acquisition_degraded != candidate.acquisition_degraded
        || previous.current.state != candidate.current.state
        || previous.details.authenticated != candidate.details.authenticated
        || previous.details.plan_label != candidate.details.plan_label
        || !exact_fact(&previous.details.quota, &candidate.details.quota)
        || previous.details.active_thread_count != candidate.details.active_thread_count
        || !exact_fact(&previous.details.threads, &candidate.details.threads)
    {
        return false;
    }
    let eligible = |key, window| {
        candidate
            .details
            .quota
            .as_ref()
            .is_some_and(|quota| new.current_observation_is_eligible_at(key, window, quota))
    };
    let model_selection_valid = match (old.current_observation(), new.current_observation()) {
        (left, right) if left == right => {
            exact_fact(&previous.models_v3, &candidate.models_v3)
                && exact_fact(&previous.details.models, &candidate.details.models)
                && previous.current.models_status == candidate.current.models_status
        }
        (None, new_key @ Some(_)) => {
            previous.models_v3.is_empty()
                && previous.details.models.is_empty()
                && previous.current.models_status
                    == codex_info_db_reader::CurrentModelsStatus::Unavailable
                && matches!((old.publication_window(), new.publication_window()),
                    (Some(old_window), Some(new_window))
                        if eligible(new_key, new_window) && !eligible(new_key, old_window))
        }
        (old_key, new_key) => match (old.publication_window(), new.publication_window()) {
            (Some(old_window), Some(new_window)) => {
                if new_key.is_none() {
                    candidate.models_v3.is_empty()
                        && candidate.details.models.is_empty()
                        && candidate.current.models_status
                            == codex_info_db_reader::CurrentModelsStatus::Unavailable
                        && !eligible(old_key, new_window)
                } else {
                    eligible(new_key, new_window)
                        && (!eligible(new_key, old_window) || !eligible(old_key, new_window))
                }
            }
            _ => false,
        },
    };
    if !model_selection_valid {
        return false;
    }
    !candidate.history_available
        || known_history
            .filter(|known| known.generation == candidate.generation)
            .is_none_or(|known| history_facts_preserved(known, candidate))
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
    last_verified_history: Option<Arc<PublishedSnapshot>>,
    degraded: bool,
    reader_unavailable: bool,
    admitted_source: Option<ReaderIntent>,
}

#[derive(Clone, Copy)]
struct HistoryFreshness {
    verified_at: i64,
    current: bool,
}

type ResourceCapture = (
    Option<Arc<PublishedSnapshot>>,
    bool,
    Option<HistoryFreshness>,
);

fn same_history_owner(left: Option<&ReaderIntent>, right: Option<&ReaderIntent>) -> bool {
    match (left, right) {
        (Some(ReaderIntent::Recorder(left)), Some(ReaderIntent::Recorder(right))) => {
            left.pid == right.pid
                && left.process_starttime == right.process_starttime
                && left.owner_nonce == right.owner_nonce
                && left.partition_id == right.partition_id
                && left.commit.collector_epoch == right.commit.collector_epoch
        }
        (Some(ReaderIntent::Independent), Some(ReaderIntent::Independent)) | (None, None) => true,
        _ => false,
    }
}

struct AccountStore {
    reader: DbReader,
    storage_epoch: u64,
    issuer: Arc<Mutex<PairIssuer>>,
    refresh_lock: Mutex<()>,
    inner: RwLock<StoreInner>,
}

impl AccountStore {
    fn new(reader: DbReader, storage_epoch: u64, issuer: Arc<Mutex<PairIssuer>>) -> Self {
        Self {
            reader,
            storage_epoch,
            issuer,
            refresh_lock: Mutex::new(()),
            inner: RwLock::new(StoreInner {
                current: None,
                last_verified_history: None,
                degraded: false,
                reader_unavailable: false,
                admitted_source: None,
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
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = inner.current.as_ref().map(|snapshot| {
            (
                snapshot.generation,
                snapshot.has_pending_ranges,
                snapshot.acquisition_degraded,
                inner.reader_unavailable,
            )
        });
        drop(inner);
        if let Some((
            current_generation,
            current_pending,
            current_acquisition_degraded,
            reader_unavailable,
        )) = current
        {
            match self.reader.read_change_marker() {
                Ok(Some(marker))
                    if marker.generation == current_generation
                        && marker.has_pending_ranges == current_pending
                        && marker.acquisition_degraded == current_acquisition_degraded
                        && !reader_unavailable =>
                {
                    self.inner
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .degraded = marker.acquisition_degraded;
                    return RefreshStatus::Unchanged {
                        generation: current_generation,
                    };
                }
                Ok(Some(marker)) if marker.generation < current_generation => {
                    log.record(route, "publication_guard", "generation_regressed", None);
                    let mut inner = self.inner.write().unwrap_or_else(|p| p.into_inner());
                    inner.degraded = true;
                    inner.reader_unavailable = true;
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
                    let mut inner = self.inner.write().unwrap_or_else(|p| p.into_inner());
                    inner.degraded = true;
                    inner.reader_unavailable = true;
                    return RefreshStatus::RetainedLastGood {
                        generation: current_generation,
                    };
                }
            }
        }
        let candidate = self.reader.read_snapshot().map(|snapshot| {
            PublishedSnapshot::candidate_from_db_for_account(snapshot, self.storage_epoch)
        });
        self.admit_candidate(candidate, log, route, None)
    }

    fn admit_candidate(
        &self,
        candidate: Result<PublishedSnapshot, ReaderError>,
        log: &FailureLog,
        route: &str,
        source: Option<&ReaderIntent>,
    ) -> RefreshStatus {
        let mut issuer = self.issuer.lock().unwrap_or_else(|p| p.into_inner());
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let status = match candidate {
            Ok(mut candidate) => {
                if let Some(previous) = inner
                    .current
                    .as_ref()
                    .filter(|previous| previous.generation == candidate.generation)
                {
                    let source_rewrite = match (
                        &previous.reprojection_witness,
                        &candidate.reprojection_witness,
                    ) {
                        (Some(old), Some(new)) => {
                            !old.same_authority_values(new)
                                || old.collection_commit() != new.collection_commit()
                                || !old.same_common_quota_receipts(new)
                                || !new.preserves_previous_current_observation(old)
                        }
                        _ => false,
                    };
                    if source_rewrite
                        || (previous.data_hash != candidate.data_hash
                            && !same_commit_reprojection(
                                previous,
                                &candidate,
                                inner.last_verified_history.as_deref(),
                            ))
                    {
                        log.record(
                            route,
                            "publication_guard",
                            if source_rewrite {
                                "same_generation_source_rewrite"
                            } else {
                                "same_commit_reprojection_rejected"
                            },
                            None,
                        );
                        let generation = previous.generation;
                        inner.degraded = true;
                        inner.reader_unavailable = true;
                        return RefreshStatus::RetainedLastGood { generation };
                    }
                }
                let current = inner
                    .current
                    .as_ref()
                    .map(|snapshot| (snapshot.generation, snapshot.data_hash.clone()));
                match current {
                    None => {
                        let Some(pair) = issuer.issue() else {
                            log.record(route, "publication_guard", "pair_issuance_failed", None);
                            inner.degraded = true;
                            inner.reader_unavailable = true;
                            return RefreshStatus::Unavailable;
                        };
                        candidate.pair = pair;
                        let generation = candidate.generation;
                        let degraded = candidate.acquisition_degraded;
                        inner.current = Some(Arc::new(candidate));
                        inner.degraded = degraded;
                        inner.reader_unavailable = false;
                        RefreshStatus::Updated { generation }
                    }
                    Some((current_generation, _)) if candidate.generation < current_generation => {
                        log.record(route, "publication_guard", "generation_regressed", None);
                        inner.degraded = true;
                        inner.reader_unavailable = true;
                        RefreshStatus::RetainedLastGood {
                            generation: current_generation,
                        }
                    }
                    Some((_, _))
                        if same_public_domain(inner.current.as_ref().unwrap(), &candidate) =>
                    {
                        let previous = inner.current.as_ref().unwrap();
                        candidate.pair = previous.pair.clone();
                        candidate.directory_pages = Arc::clone(&previous.directory_pages);
                        let generation = candidate.generation;
                        let frame = if candidate == **previous {
                            Arc::clone(previous)
                        } else {
                            Arc::new(candidate)
                        };
                        inner.degraded = frame.acquisition_degraded;
                        inner.reader_unavailable = false;
                        inner.current = Some(frame);
                        RefreshStatus::Unchanged { generation }
                    }
                    Some((current_generation, _)) => {
                        let Some(pair) = issuer.issue() else {
                            log.record(route, "publication_guard", "pair_issuance_failed", None);
                            inner.degraded = true;
                            inner.reader_unavailable = true;
                            return RefreshStatus::RetainedLastGood {
                                generation: current_generation,
                            };
                        };
                        candidate.pair = pair;
                        let generation = candidate.generation;
                        let degraded = candidate.acquisition_degraded;
                        inner.current = Some(Arc::new(candidate));
                        inner.degraded = degraded;
                        inner.reader_unavailable = false;
                        RefreshStatus::Updated { generation }
                    }
                }
            }
            Err(error) => {
                log.record(route, "snapshot_read", &reader_failure_reason(&error), None);
                inner.reader_unavailable = true;
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
        };
        if matches!(
            status,
            RefreshStatus::Updated { .. } | RefreshStatus::Unchanged { .. }
        ) {
            // A new commit is not a revocation of the previously verified
            // history. An owner/epoch boundary is: it cannot lend old facts
            // to a different recording authority, even in the same account.
            let epoch_changed = inner
                .last_verified_history
                .as_ref()
                .zip(inner.current.as_ref())
                .is_some_and(|(history, current)| {
                    let epoch = |frame: &PublishedSnapshot| {
                        frame
                            .reprojection_witness
                            .as_ref()
                            .and_then(|proof| proof.collection_commit())
                            .map(|commit| commit.collector_epoch)
                    };
                    epoch(history) != epoch(current)
                });
            if epoch_changed || !same_history_owner(inner.admitted_source.as_ref(), source) {
                inner.last_verified_history = None;
            }
            if let Some(current) = inner.current.as_ref() {
                if current.history_available {
                    inner.last_verified_history = Some(Arc::clone(current));
                }
            }
            inner.admitted_source = source.cloned();
        }
        status
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

    #[cfg(test)]
    fn capture(&self) -> (Option<Arc<PublishedSnapshot>>, bool) {
        let inner = self.inner.read().unwrap_or_else(|p| p.into_inner());
        (inner.current.clone(), inner.reader_unavailable)
    }

    fn capture_resource(&self, route: Route) -> ResourceCapture {
        let inner = self.inner.read().unwrap_or_else(|p| p.into_inner());
        let frame = if route.history_resource() {
            inner
                .last_verified_history
                .as_ref()
                .or(inner.current.as_ref())
        } else {
            inner.current.as_ref()
        };
        let freshness = route
            .history_resource()
            .then(|| {
                let history = frame?.history_reprojection_witness.as_ref()?;
                let window = history.publication_window()?;
                let current = inner.current.as_ref()?.reprojection_witness.as_ref()?;
                Some(HistoryFreshness {
                    verified_at: window.at,
                    current: history.collection_commit() == current.collection_commit()
                        && current
                            .publication_window()
                            .is_some_and(|latest| window.at >= latest.at),
                })
            })
            .flatten();
        (frame.cloned(), inner.reader_unavailable, freshness)
    }

    fn retire_history(&self) {
        let mut inner = self.inner.write().unwrap_or_else(|p| p.into_inner());
        inner.last_verified_history = None;
        inner.reader_unavailable = true;
    }

    fn history_owner_matches(&self, intent: &ReaderIntent) -> bool {
        let inner = self.inner.read().unwrap_or_else(|p| p.into_inner());
        same_history_owner(inner.admitted_source.as_ref(), Some(intent))
    }

    fn mark_unavailable(&self) {
        let mut inner = self.inner.write().unwrap_or_else(|p| p.into_inner());
        inner.reader_unavailable = true;
        inner.degraded = true;
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
    source: ReaderIntent,
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
            source: ReaderIntent::Independent,
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

    pub fn with_recorder_commit(mut self, notice: Option<RecorderCommitNotice>) -> Self {
        self.source = notice
            .map(ReaderIntent::Recorder)
            .unwrap_or(ReaderIntent::Unconfirmed);
        self
    }
}

/// Reader/cache boundary.  The cache is an in-memory last-good snapshot per
/// account partition, not a second persistence authority and is never written
/// to disk.  `new(DbReader)` remains the fixture-compatible single-reader API.
type RecorderVersionReader = Arc<dyn Fn() -> Option<String> + Send + Sync>;
type PublicationClock = Arc<dyn Fn() -> Option<i64> + Send + Sync>;

fn utc_unix_seconds() -> Option<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
}

pub struct SnapshotStore {
    issuer: Arc<Mutex<PairIssuer>>,
    publication_failure: Arc<AtomicBool>,
    reader_control: ReaderControl,
    boundary: RwLock<BoundaryPublication>,
    recorder_version_reader: RwLock<Option<RecorderVersionReader>>,
    publication_clock: PublicationClock,
    #[cfg(test)]
    history_candidate_ready: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    recording_candidate_ready: Option<Arc<dyn Fn() + Send + Sync>>,
    diagnostics: FailureLog,
}

struct BoundaryPublication {
    generation: u64,
    snapshot: Option<Arc<PublishedSnapshot>>,
    pair: String,
    accounts: PublicAccountsV3,
    configured_accounts: PublicAccountsV3,
    stores: BTreeMap<String, Arc<AccountStore>>,
    initializing: Arc<PublishedSnapshot>,
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
        let mut intents = BTreeMap::new();
        let pair_issuer = PairIssuer::new()?;
        let publication_failure = Arc::clone(&pair_issuer.failed);
        let issuer = Arc::new(Mutex::new(pair_issuer));
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
            intents.insert(account.id.clone(), Arc::new(account.source));
            if stores
                .insert(
                    account.id,
                    Arc::new(AccountStore::new(
                        account.reader,
                        account.storage_epoch,
                        Arc::clone(&issuer),
                    )),
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
        let initial_pair = issuer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .issue()
            .expect("a fresh publisher has counter space for its initializing frame");
        Ok(Self {
            issuer,
            publication_failure,
            reader_control: ReaderControl {
                pending: Mutex::new(ReaderPending {
                    latest: intents,
                    freshness: true,
                    ..ReaderPending::default()
                }),
                changed: Condvar::new(),
                history_changed: Condvar::new(),
                recordings_changed: Condvar::new(),
            },
            boundary: RwLock::new(BoundaryPublication {
                generation: 0,
                snapshot: None,
                initializing: Arc::new(PublishedSnapshot::account_boundary(
                    PublicState::Initializing,
                    0,
                    initial_pair.clone(),
                )),
                pair: initial_pair,
                configured_accounts: public.clone(),
                stores,
                accounts: public,
            }),
            recorder_version_reader: RwLock::new(None),
            publication_clock: Arc::new(utc_unix_seconds),
            #[cfg(test)]
            history_candidate_ready: None,
            #[cfg(test)]
            recording_candidate_ready: None,
            diagnostics: FailureLog::default(),
        })
    }

    pub fn new_without_account(state: PublicState) -> Self {
        Self::try_new_without_account(state).expect("boundary fixture publisher initialization")
    }

    fn try_new_without_account(state: PublicState) -> Result<Self, RestServerError> {
        debug_assert!(state != PublicState::Ready);
        let pair_issuer = PairIssuer::new()?;
        let publication_failure = Arc::clone(&pair_issuer.failed);
        let issuer = Arc::new(Mutex::new(pair_issuer));
        let pair = issuer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .issue()
            .expect("a fresh publisher has counter space for its initializing frame");
        Ok(Self {
            issuer,
            publication_failure,
            reader_control: ReaderControl::default(),
            boundary: RwLock::new(BoundaryPublication {
                generation: 1,
                initializing: Arc::new(PublishedSnapshot::account_boundary(
                    PublicState::Initializing,
                    0,
                    pair.clone(),
                )),
                snapshot: Some(Arc::new(PublishedSnapshot::account_boundary(
                    state,
                    1,
                    pair.clone(),
                ))),
                pair,
                accounts: PublicAccountsV3 {
                    default_account_id: None,
                    accounts: Vec::new(),
                },
                configured_accounts: PublicAccountsV3 {
                    default_account_id: None,
                    accounts: Vec::new(),
                },
                stores: BTreeMap::new(),
            }),
            recorder_version_reader: RwLock::new(None),
            publication_clock: Arc::new(utc_unix_seconds),
            #[cfg(test)]
            history_candidate_ready: None,
            #[cfg(test)]
            recording_candidate_ready: None,
            diagnostics: FailureLog::default(),
        })
    }

    pub fn set_log_data_root(&self, root: &std::path::Path) {
        self.diagnostics.set_data_root(root);
    }

    /// Reconcile a locator-validated catalog without replacing the listener,
    /// publisher identity, or unchanged account frames. Superseded reader
    /// intents cannot publish after this atomic routing exchange.
    pub fn reconcile_accounts(
        &self,
        mut accounts: Vec<AccountReader>,
        default_account_id: impl Into<String>,
    ) -> Result<bool, RestServerError> {
        let default_account_id = default_account_id.into();
        accounts.sort_by(|left, right| {
            right
                .is_current
                .cmp(&left.is_current)
                .then_with(|| right.storage_epoch.cmp(&left.storage_epoch))
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut seen = BTreeSet::new();
        let mut descriptors = Vec::with_capacity(accounts.len());
        for account in &accounts {
            if account.storage_epoch == 0
                || account.id != format!("account-{}", account.storage_epoch)
                || !seen.insert(account.id.clone())
            {
                return Err(RestServerError::InvalidAccounts(
                    "invalid or duplicate account selector".into(),
                ));
            }
            descriptors.push(PublicAccountV3 {
                id: account.id.clone(),
                is_current: account.is_current,
                activation_at: account.activation_at,
                deactivation_at: account.deactivation_at,
                ownership_intervals: account.ownership_intervals.clone(),
                login_id: account.login_id.clone(),
            });
        }
        let public = PublicAccountsV3 {
            default_account_id: Some(default_account_id),
            accounts: descriptors,
        };
        public
            .validate()
            .map_err(|error| RestServerError::InvalidAccounts(error.to_string()))?;
        let mut pending = self
            .reader_control
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if pending.stopped {
            return Err(RestServerError::InvalidAccounts("reader is stopped".into()));
        }
        // The same lock order is used by boundary publication. HTTP captures
        // only the routing frame and never takes the issuer or pending lock.
        let mut issuer = self.issuer.lock().unwrap_or_else(|p| p.into_inner());
        let mut global = self.boundary.write().unwrap_or_else(|p| p.into_inner());
        let mut stores = BTreeMap::new();
        let mut intents = BTreeMap::new();
        let mut changed = global.configured_accounts != public;
        let mut changed_default_reader = false;
        for account in accounts {
            let old = global.stores.get(&account.id).filter(|old| {
                old.reader == account.reader && old.storage_epoch == account.storage_epoch
            });
            let retained = old.is_some();
            changed |= !retained;
            changed_default_reader |=
                !retained && public.default_account_id.as_deref() == Some(account.id.as_str());
            let store = old.cloned().unwrap_or_else(|| {
                Arc::new(AccountStore::new(
                    account.reader,
                    account.storage_epoch,
                    Arc::clone(&self.issuer),
                ))
            });
            let source = pending
                .latest
                .get(&account.id)
                .filter(|old| retained && old.as_ref() == &account.source)
                .cloned()
                .unwrap_or_else(|| {
                    pending.accounts.insert(account.id.clone());
                    Arc::new(account.source)
                });
            if *source == ReaderIntent::Unconfirmed {
                store.mark_unavailable();
            }
            intents.insert(account.id.clone(), source);
            stores.insert(account.id, store);
        }
        if changed {
            let pair = issuer.issue().ok_or(RestServerError::EpochUnavailable)?;
            let changing_default =
                global.configured_accounts.default_account_id != public.default_account_id;
            global.generation = global.generation.saturating_add(1).max(1);
            if changing_default || changed_default_reader || global.snapshot.is_some() {
                global.snapshot = Some(Arc::new(PublishedSnapshot::account_boundary(
                    PublicState::Initializing,
                    global.generation,
                    pair.clone(),
                )));
            }
            global.pair = pair;
            global.accounts = public.clone();
            global.configured_accounts = public;
            global.stores = stores;
        }
        pending.latest = intents;
        let initialized = pending.latest.keys().cloned().collect::<BTreeSet<_>>();
        pending.accounts.retain(|id| initialized.contains(id));
        self.reader_control.changed.notify_one();
        Ok(changed)
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

    fn account_store(&self, id: &str) -> Option<Arc<AccountStore>> {
        self.boundary
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .stores
            .get(id)
            .cloned()
    }

    pub fn reader(&self) -> DbReader {
        self.account_store(&self.default_account_id().expect("fixture default account"))
            .expect("validated default account")
            .reader()
            .clone()
    }

    pub fn refresh(&self) -> RefreshStatus {
        self.default_account_id()
            .map(|id| self.refresh_account(&id))
            .unwrap_or(RefreshStatus::Unavailable)
    }

    pub fn refresh_account(&self, account_id: &str) -> RefreshStatus {
        self.refresh_account_for_route(account_id, "snapshot_refresh")
    }

    fn refresh_account_for_route(&self, account_id: &str, route: &str) -> RefreshStatus {
        self.account_store(account_id)
            .map(|account| account.refresh(&self.diagnostics, route))
            .unwrap_or(RefreshStatus::Unavailable)
    }

    #[cfg(test)]
    fn capture_account_frame(&self, account_id: &str) -> (Option<Arc<PublishedSnapshot>>, bool) {
        self.account_store(account_id)
            .map(|account| account.capture())
            .unwrap_or((None, true))
    }

    #[cfg(test)]
    fn capture_default_frame(&self) -> (Option<Arc<PublishedSnapshot>>, bool) {
        let global = self.boundary.read().unwrap_or_else(|p| p.into_inner());
        if let Some(boundary) = global.snapshot.as_ref() {
            return (Some(Arc::clone(boundary)), false);
        }
        let Some(account) = global
            .accounts
            .default_account_id
            .as_deref()
            .and_then(|id| global.stores.get(id))
        else {
            return (None, true);
        };
        let (frame, unavailable) = account.capture();
        if frame.is_none() && !unavailable {
            return (Some(Arc::clone(&global.initializing)), false);
        }
        (frame, unavailable)
    }

    fn capture_resource(&self, account_id: Option<&str>, route: Route) -> ResourceCapture {
        if let Some(account_id) = account_id {
            return self
                .account_store(account_id)
                .map(|account| account.capture_resource(route))
                .unwrap_or((None, true, None));
        }
        let global = self.boundary.read().unwrap_or_else(|p| p.into_inner());
        if let Some(boundary) = global.snapshot.as_ref() {
            return (Some(Arc::clone(boundary)), false, None);
        }
        let Some(account) = global
            .accounts
            .default_account_id
            .as_deref()
            .and_then(|id| global.stores.get(id))
        else {
            return (None, true, None);
        };
        let (frame, unavailable, history) = account.capture_resource(route);
        if frame.is_none() && !unavailable {
            (Some(Arc::clone(&global.initializing)), false, None)
        } else {
            (frame, unavailable, history)
        }
    }

    pub fn status(&self) -> StoreStatus {
        self.default_account_id()
            .map(|id| self.status_account(&id))
            .unwrap_or(StoreStatus {
                generation: None,
                degraded: false,
            })
    }

    pub fn status_account(&self, account_id: &str) -> StoreStatus {
        self.account_store(account_id)
            .map(|account| account.status())
            .unwrap_or(StoreStatus {
                generation: None,
                degraded: true,
            })
    }

    pub fn snapshot(&self) -> Option<Arc<PublishedSnapshot>> {
        let global = self.boundary.read().unwrap_or_else(|p| p.into_inner());
        global.snapshot.clone().or_else(|| {
            global
                .configured_accounts
                .default_account_id
                .as_deref()
                .and_then(|id| global.stores.get(id))
                .and_then(|account| account.snapshot())
        })
    }

    pub fn snapshot_account(&self, account_id: &str) -> Option<Arc<PublishedSnapshot>> {
        self.account_store(account_id)
            .and_then(|account| account.snapshot())
    }

    pub fn snapshot_with_status(&self) -> (Option<Arc<PublishedSnapshot>>, bool) {
        let global = self.boundary.read().unwrap_or_else(|p| p.into_inner());
        if let Some(snapshot) = &global.snapshot {
            return (Some(Arc::clone(snapshot)), false);
        }
        global
            .configured_accounts
            .default_account_id
            .as_deref()
            .and_then(|id| global.stores.get(id))
            .map(|account| account.snapshot_with_status())
            .unwrap_or((None, false))
    }

    pub fn snapshot_with_status_account(
        &self,
        account_id: &str,
    ) -> (Option<Arc<PublishedSnapshot>>, bool) {
        self.account_store(account_id)
            .map(|account| account.snapshot_with_status())
            .unwrap_or((None, true))
    }

    pub fn has_account(&self, account_id: &str) -> bool {
        self.boundary
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .stores
            .contains_key(account_id)
    }

    pub fn default_account_id(&self) -> Option<String> {
        self.boundary
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .configured_accounts
            .default_account_id
            .clone()
    }

    pub fn account_descriptors(&self) -> Vec<PublicAccountV3> {
        self.boundary
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .configured_accounts
            .accounts
            .clone()
    }

    pub fn public_accounts(&self) -> PublicAccountsV3 {
        self.capture_accounts().0
    }

    fn capture_accounts(&self) -> (PublicAccountsV3, String) {
        let global = self.boundary.read().unwrap_or_else(|p| p.into_inner());
        (global.accounts.clone(), global.pair.clone())
    }

    pub fn publication_failed(&self) -> bool {
        self.publication_failure.load(Ordering::Acquire)
    }

    pub fn notify_recorder_commit(&self, account_id: &str, notice: RecorderCommitNotice) -> bool {
        self.update_reader_intent(
            account_id,
            ReaderIntent::Recorder(notice),
            RecorderSourceUnavailableReason::Unspecified,
        )
    }

    pub fn mark_recorder_source_unavailable(&self, account_id: &str) -> bool {
        self.mark_recorder_source_unavailable_with_reason(
            account_id,
            RecorderSourceUnavailableReason::Unspecified,
        )
    }

    pub fn mark_recorder_source_unavailable_with_reason(
        &self,
        account_id: &str,
        reason: RecorderSourceUnavailableReason,
    ) -> bool {
        self.update_reader_intent(account_id, ReaderIntent::Unconfirmed, reason)
    }

    pub fn recorder_commit_is_published(
        &self,
        account_id: &str,
        notice: &RecorderCommitNotice,
    ) -> bool {
        self.account_store(account_id).is_some_and(|account| {
            let inner = account.inner.read().unwrap_or_else(|p| p.into_inner());
            !inner.reader_unavailable
                && inner.current.is_some()
                && inner.admitted_source.as_ref() == Some(&ReaderIntent::Recorder(notice.clone()))
        })
    }

    fn update_reader_intent(
        &self,
        account_id: &str,
        intent: ReaderIntent,
        reason: RecorderSourceUnavailableReason,
    ) -> bool {
        let mut pending = self
            .reader_control
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if pending.stopped || !pending.latest.contains_key(account_id) {
            return false;
        }
        if pending
            .latest
            .get(account_id)
            .is_some_and(|old| old.as_ref() == &intent)
        {
            return false;
        }
        if intent == ReaderIntent::Unconfirmed {
            self.diagnostics
                .record("reader_reconcile", "recorder_source", reason.as_str(), None);
            if let Some(account) = self.account_store(account_id) {
                account.mark_unavailable();
            }
        } else if pending.latest.get(account_id).is_some_and(|old| {
            old.as_ref() != &ReaderIntent::Unconfirmed
                && !same_history_owner(Some(old.as_ref()), Some(&intent))
        }) {
            if let Some(account) = self.account_store(account_id) {
                account.retire_history();
            }
        }
        pending
            .latest
            .insert(account_id.to_owned(), Arc::new(intent));
        pending.accounts.insert(account_id.to_owned());
        self.reader_control.changed.notify_one();
        true
    }

    fn prepare_reader_candidate_at(
        &self,
        account_id: &str,
        at: i64,
    ) -> Result<(PublishedSnapshot, Option<CollectionCommit>), ReaderError> {
        let Some(account) = self.account_store(account_id) else {
            return Err(ReaderError::InvalidValue("account catalog changed".into()));
        };
        let previous_key = account.snapshot().and_then(|frame| {
            frame
                .reprojection_witness
                .as_ref()
                .and_then(|proof| proof.current_observation())
        });
        let candidate = account
            .reader
            .read_committed_history_for_reprojection_at(at, previous_key)?;
        #[cfg(test)]
        if let Some(ready) = &self.history_candidate_ready {
            ready();
        }
        Ok((
            PublishedSnapshot::candidate_from_db_for_account(
                candidate.snapshot,
                account.storage_epoch,
            ),
            candidate.commit,
        ))
    }

    fn prepare_current_candidate_at(
        &self,
        account_id: &str,
        at: i64,
    ) -> Result<Option<(PublishedSnapshot, Option<CollectionCommit>)>, ReaderError> {
        let account = self
            .account_store(account_id)
            .ok_or_else(|| ReaderError::InvalidValue("account catalog changed".into()))?;
        let previous_key = account.snapshot().and_then(|frame| {
            frame
                .reprojection_witness
                .as_ref()
                .and_then(|proof| proof.current_observation())
        });
        Ok(account
            .reader
            .read_committed_current_for_reprojection_at(at, previous_key)?
            .map(|candidate| {
                (
                    PublishedSnapshot::candidate_from_db_for_account(
                        candidate.snapshot,
                        account.storage_epoch,
                    ),
                    candidate.commit,
                )
            }))
    }

    fn prepare_recordings_candidate_at(
        &self,
        account_id: &str,
        at: i64,
    ) -> Result<(PublishedSnapshot, Option<CollectionCommit>), ReaderError> {
        let account = self
            .account_store(account_id)
            .ok_or_else(|| ReaderError::InvalidValue("account catalog changed".into()))?;
        let previous_key = account.snapshot().and_then(|frame| {
            frame
                .reprojection_witness
                .as_ref()
                .and_then(|proof| proof.current_observation())
        });
        let candidate = account
            .reader
            .read_committed_recordings_for_reprojection_at(at, previous_key)?;
        #[cfg(test)]
        if let Some(ready) = &self.recording_candidate_ready {
            ready();
        }
        Ok((
            PublishedSnapshot::candidate_from_db_for_account(
                candidate.snapshot,
                account.storage_epoch,
            ),
            candidate.commit,
        ))
    }

    #[cfg(test)]
    fn complete_reader_candidate(
        &self,
        account_id: &str,
        intent: &Arc<ReaderIntent>,
        candidate: Result<(PublishedSnapshot, Option<CollectionCommit>), ReaderError>,
    ) {
        self.complete_reader_lane(account_id, intent, candidate, ReaderLane::Direct);
    }

    fn complete_reader_lane(
        &self,
        account_id: &str,
        intent: &Arc<ReaderIntent>,
        mut candidate: Result<(PublishedSnapshot, Option<CollectionCommit>), ReaderError>,
        lane: ReaderLane,
    ) {
        // Keep this guard through root exchange. Accepting a newer intent and
        // publishing its predecessor cannot cross each other at this boundary.
        let pending = self
            .reader_control
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if pending.stopped {
            return;
        }
        if !pending
            .latest
            .get(account_id)
            .is_some_and(|latest| Arc::ptr_eq(latest, intent))
        {
            if lane == ReaderLane::History {
                self.diagnostics.record(
                    "reader_reconcile",
                    "history_admission",
                    "intent_superseded",
                    None,
                );
            }
            return;
        }
        let Some(account) = self.account_store(account_id) else {
            return;
        };
        if **intent == ReaderIntent::Unconfirmed {
            account.mark_unavailable();
            return;
        }
        if let (ReaderIntent::Recorder(notice), Ok((_, commit))) = (intent.as_ref(), &candidate) {
            if *commit != Some(notice.commit) {
                // A DB commit may precede publication of its state file. This
                // candidate is unconfirmed, rather than a failed DB read.
                if lane == ReaderLane::History {
                    self.diagnostics.record(
                        "reader_reconcile",
                        "history_admission",
                        "commit_not_acknowledged",
                        None,
                    );
                }
                return;
            }
        }
        if lane == ReaderLane::Recordings {
            let Ok((next, commit)) = candidate else {
                self.diagnostics.record(
                    "reader_reconcile",
                    "recordings_read",
                    "archive_unavailable",
                    None,
                );
                return;
            };
            let Some(current) = account.snapshot() else {
                return;
            };
            let Some(archive) = next.recordings else {
                return;
            };
            if next.generation != current.generation
                || !matches!((&current.reprojection_witness, &next.reprojection_witness),
                    (Some(old), Some(new)) if old.collection_commit() == commit
                        && old.same_commit_authority(new)
                        && old.same_common_quota_receipts(new)
                        && new.preserves_previous_current_observation(old))
            {
                return;
            }
            if let Some(known) = &current.recordings {
                if known.as_ref() != archive.as_ref() {
                    self.diagnostics.record(
                        "reader_reconcile",
                        "recordings_read",
                        "same_generation_source_rewrite",
                        None,
                    );
                }
                return;
            }
            // The read-only archive adds one independently qualified resource
            // to this exact commit. Keep every current/history field from the
            // latest frame, including its newer publication-time projection.
            candidate = (*current)
                .clone()
                .with_recordings(archive)
                .map(|frame| (frame, commit));
        } else if matches!(lane, ReaderLane::Current | ReaderLane::History) {
            if let Ok((next, _)) = &mut candidate {
                if lane == ReaderLane::Current
                    && !next.history_available
                    && account.history_owner_matches(intent)
                {
                    if let Some(current) = account.snapshot().filter(|current| {
                        current.history_available
                            && current.generation == next.generation
                            && same_commit_reprojection(current, next, None)
                    }) {
                        // A newer Current projection of this same commit does
                        // not invalidate the independently qualified history.
                        match next.clone().with_history(&current) {
                            Ok(merged) => *next = merged,
                            Err(_) => self.diagnostics.record(
                                "reader_reconcile",
                                "history_admission",
                                "history_reprojection_invalid",
                                None,
                            ),
                        }
                    }
                }
                if next.recordings.is_none() {
                    if let Some(current) = account.snapshot().filter(|current| {
                        current.generation == next.generation
                            && same_commit_reprojection(current, next, None)
                    }) {
                        if let Some(archive) = &current.recordings {
                            match next.clone().with_recordings(Arc::clone(archive)) {
                                Ok(merged) => *next = merged,
                                Err(_) => return,
                            }
                        }
                    }
                }
            }
        }
        if lane == ReaderLane::History {
            match &mut candidate {
                Err(error) => {
                    self.diagnostics.record(
                        "reader_reconcile",
                        "history_read",
                        &reader_failure_reason(error),
                        None,
                    );
                    return;
                }
                Ok((next, _)) => {
                    if let Some(current) = account.snapshot() {
                        let reason = if next.generation < current.generation {
                            Some("generation_superseded")
                        } else if next.generation == current.generation
                            && current.history_available
                            && matches!((&current.history_reprojection_witness, &next.history_reprojection_witness),
                                (Some(old), Some(new)) if matches!((old.publication_window(), new.publication_window()),
                                    (Some(old), Some(new)) if new.at < old.at))
                        {
                            Some("publication_window_superseded")
                        } else {
                            None
                        };
                        if let Some(reason) = reason {
                            self.diagnostics.record(
                                "reader_reconcile",
                                "history_admission",
                                reason,
                                None,
                            );
                            return;
                        }
                        if next.generation == current.generation
                            && matches!((&current.reprojection_witness, &next.reprojection_witness),
                                (Some(old), Some(new)) if matches!((old.publication_window(), new.publication_window()),
                                    (Some(old), Some(new)) if new.at < old.at))
                        {
                            if !next.history_available
                                || !same_commit_reprojection(next, &current, None)
                            {
                                self.diagnostics.record(
                                    "reader_reconcile",
                                    "history_admission",
                                    "history_authority_changed",
                                    None,
                                );
                                return;
                            }
                            // Keep the latest Current fields and its witness.
                            // History retains the window of its own transaction.
                            match (*current).clone().with_history(next) {
                                Ok(merged) => *next = merged,
                                Err(_) => {
                                    self.diagnostics.record(
                                        "reader_reconcile",
                                        "history_admission",
                                        "history_reprojection_invalid",
                                        None,
                                    );
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
        if lane == ReaderLane::Current && account.history_owner_matches(intent) {
            if let Ok((next, _)) = &candidate {
                let mut inner = account.inner.write().unwrap_or_else(|p| p.into_inner());
                if inner.current.as_ref().is_some_and(|old| {
                    old.history_available
                        && old.generation == next.generation
                        && same_commit_reprojection(
                            old,
                            next,
                            inner.last_verified_history.as_deref(),
                        )
                        && old.current == next.current
                        && old.details.observed_at == next.details.observed_at
                        && exact_fact(&old.models_v3, &next.models_v3)
                }) {
                    // A cheap confirmation cannot withdraw already qualified
                    // history for this same commit and unchanged current view.
                    inner.admitted_source = Some((**intent).clone());
                    inner.reader_unavailable = false;
                    inner.degraded = next.acquisition_degraded;
                    return;
                }
            }
        }
        let history_became_available = lane == ReaderLane::History
            && candidate
                .as_ref()
                .is_ok_and(|(next, _)| next.history_available)
            && account
                .snapshot()
                .is_none_or(|current| !current.history_available);
        let status = account.admit_candidate(
            candidate.map(|(snapshot, _)| snapshot),
            &self.diagnostics,
            "reader_reconcile",
            Some(intent),
        );
        if history_became_available
            && matches!(
                status,
                RefreshStatus::Updated { .. } | RefreshStatus::Unchanged { .. }
            )
        {
            let reason = if self.default_account_id().as_deref() == Some(account_id) {
                "current_history_available"
            } else {
                "archived_history_available"
            };
            self.diagnostics
                .record("reader_reconcile", "history_admission", reason, None);
        }
    }

    fn stop_reader(&self) {
        let mut pending = self
            .reader_control
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        pending.stopped = true;
        pending.accounts.clear();
        pending.history.clear();
        pending.recordings.clear();
        pending.freshness = false;
        self.reader_control.changed.notify_all();
        self.reader_control.history_changed.notify_all();
        self.reader_control.recordings_changed.notify_all();
    }

    pub fn publish_account_boundary(&self, state: PublicState) {
        debug_assert!(state != PublicState::Ready);
        let mut pending = self
            .reader_control
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(id) = self.default_account_id().as_ref() {
            if pending
                .latest
                .get(id)
                .is_some_and(|intent| **intent != ReaderIntent::Independent)
            {
                pending
                    .latest
                    .insert(id.clone(), Arc::new(ReaderIntent::Unconfirmed));
                pending.accounts.insert(id.clone());
                if let Some(account) = self.account_store(id) {
                    account.mark_unavailable();
                }
                self.reader_control.changed.notify_one();
            }
        }
        let mut issuer = self.issuer.lock().unwrap_or_else(|p| p.into_inner());
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
        let Some(pair) = issuer.issue() else {
            return;
        };
        boundary.generation = boundary.generation.saturating_add(1).max(1);
        let generation = boundary.generation;
        boundary.snapshot = Some(Arc::new(PublishedSnapshot::account_boundary(
            state,
            generation,
            pair.clone(),
        )));
        boundary.pair = pair;
        boundary.accounts.default_account_id = None;
        for account in &mut boundary.accounts.accounts {
            account.is_current = false;
        }
    }

    pub fn clear_account_boundary(&self) {
        let mut issuer = self.issuer.lock().unwrap_or_else(|p| p.into_inner());
        let mut global = self.boundary.write().unwrap_or_else(|p| p.into_inner());
        if global.snapshot.is_none() {
            return;
        }
        let Some(pair) = issuer.issue() else {
            return;
        };
        global.snapshot = None;
        global.pair = pair;
        global.accounts = global.configured_accounts.clone();
    }
}

#[derive(Debug)]
pub enum RestServerError {
    EpochUnavailable,
    NonLoopbackAddress,
    Bind(io::Error),
    Listener(io::Error),
    InvalidPort,
    InvalidAccounts(String),
}

impl fmt::Display for RestServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EpochUnavailable => formatter.write_str("REST publisher epoch is unavailable"),
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
    reader_worker: Option<JoinHandle<()>>,
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
        let store = SnapshotStore::new_with_accounts(accounts, default_account_id)?;
        Self::start_with_store(store, listen_addr)
    }

    fn start_with_store(
        store: SnapshotStore,
        listen_addr: SocketAddr,
    ) -> Result<Self, RestServerError> {
        let store = Arc::new(store);
        let listener = TcpListener::bind(listen_addr).map_err(RestServerError::Bind)?;
        listener
            .set_nonblocking(true)
            .map_err(RestServerError::Listener)?;
        let local_addr = listener.local_addr().map_err(RestServerError::Listener)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_store = Arc::clone(&store);
        let worker = thread::Builder::new()
            .name("codex-info-rest".to_owned())
            .spawn(move || serve(listener, worker_store, worker_stop))
            .map_err(RestServerError::Listener)?;
        let reader_worker = match start_reader_worker(Arc::clone(&store)) {
            Ok(reader_worker) => reader_worker,
            Err(error) => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                return Err(error);
            }
        };
        Ok(Self {
            local_addr,
            stop,
            worker: Some(worker),
            reader_worker: Some(reader_worker),
            store,
        })
    }

    /// Start a read-only listener while no Codex account is authenticated.
    /// The store contains no selectable current account and publishes only
    /// the strict empty boundary root until catalog reconciliation admits
    /// an initialized account, using the same listener and pair issuer.
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
        Self::start_with_store(SnapshotStore::try_new_without_account(state)?, listen_addr)
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn store(&self) -> Arc<SnapshotStore> {
        Arc::clone(&self.store)
    }

    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.store.stop_reader();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(reader_worker) = self.reader_worker.take() {
            let _ = reader_worker.join();
        }
    }
}

impl Drop for RestServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve(listener: TcpListener, store: Arc<SnapshotStore>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) && !store.publication_failed() {
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

fn start_reader_worker(store: Arc<SnapshotStore>) -> Result<JoinHandle<()>, RestServerError> {
    let history_store = Arc::clone(&store);
    let history_worker = thread::Builder::new()
        .name("codex-info-rest-history".to_owned())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_auxiliary_reader_worker(&history_store, ReaderLane::History);
            }));
            if outcome.is_err() {
                history_store.diagnostics.record(
                    "reader_reconcile",
                    "history_read",
                    "worker_terminated",
                    None,
                );
            }
        });
    if history_worker.is_err() {
        store.diagnostics.record(
            "reader_reconcile",
            "history_read",
            "worker_unavailable",
            None,
        );
    }
    let recordings_store = Arc::clone(&store);
    let recordings_worker = thread::Builder::new()
        .name("codex-info-rest-recordings".to_owned())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_auxiliary_reader_worker(&recordings_store, ReaderLane::Recordings);
            }));
            if outcome.is_err() {
                recordings_store.diagnostics.record(
                    "reader_reconcile",
                    "recordings_read",
                    "worker_terminated",
                    None,
                );
            }
        });
    if recordings_worker.is_err() {
        store.diagnostics.record(
            "reader_reconcile",
            "recordings_read",
            "worker_unavailable",
            None,
        );
    }
    let cleanup = Arc::clone(&store);
    let started = thread::Builder::new()
        .name("codex-info-rest-reader".to_owned())
        .spawn(move || {
            // The auxiliary worker is read-only. Shutdown revokes its result
            // authority; current publication never joins a long archive read.
            let _history_worker = history_worker;
            let _recordings_worker = recordings_worker;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_reader_worker(&store);
            }));
            if outcome.is_err() {
                store.publication_failure.store(true, Ordering::Release);
                store.stop_reader();
            }
        })
        .map_err(RestServerError::Listener);
    if started.is_err() {
        cleanup.stop_reader();
    }
    started
}

fn run_auxiliary_reader_worker(store: &SnapshotStore, lane: ReaderLane) {
    let mut last_account: Option<String> = None;
    loop {
        let (id, intent, at) = {
            let mut pending = store
                .reader_control
                .pending
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            while !pending.stopped
                && match lane {
                    ReaderLane::History => pending.history.is_empty(),
                    ReaderLane::Recordings => pending.recordings.is_empty(),
                    _ => unreachable!("auxiliary reader lane"),
                }
            {
                let changed = if lane == ReaderLane::History {
                    &store.reader_control.history_changed
                } else {
                    &store.reader_control.recordings_changed
                };
                pending = changed.wait(pending).unwrap_or_else(|p| p.into_inner());
            }
            if pending.stopped || store.publication_failed() {
                return;
            }
            // Round robin keeps an active account from starving another
            // account's history. Each queued account holds only its newest job.
            let queue = if lane == ReaderLane::History {
                &mut pending.history
            } else {
                &mut pending.recordings
            };
            let id = queue
                .keys()
                .find(|id| last_account.as_ref().is_none_or(|last| *id > last))
                .or_else(|| queue.keys().next())
                .cloned()
                .unwrap();
            let (intent, at) = queue.remove(&id).unwrap();
            last_account = Some(id.clone());
            if !pending
                .latest
                .get(&id)
                .is_some_and(|latest| Arc::ptr_eq(latest, &intent))
            {
                continue;
            }
            (id, intent, at)
        };
        let Some(account) = store.account_store(&id) else {
            continue;
        };
        let started = Instant::now();
        let candidate = if lane == ReaderLane::History {
            let _refresh_guard = account
                .refresh_lock
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            store.prepare_reader_candidate_at(&id, at)
        } else {
            store.prepare_recordings_candidate_at(&id, at)
        };
        let elapsed = started.elapsed();
        if elapsed >= Duration::from_secs(1) {
            // Fixed stage and duration only; no account, SQL, source or value.
            eprintln!(
                "{}",
                json!({
                    "schema": "codex-info-reader-duration-v1",
                    "stage": if lane == ReaderLane::History { "period_history" } else { "recorded_archive" },
                    "elapsed_ms": elapsed.as_millis(),
                })
            );
        }
        store.complete_reader_lane(&id, &intent, candidate, lane);
    }
}

fn run_reader_worker(store: &SnapshotStore) {
    let mut deadline = Instant::now() + READER_FRESHNESS;
    loop {
        let jobs = {
            let mut pending = store
                .reader_control
                .pending
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            loop {
                if pending.stopped || store.publication_failed() {
                    return;
                }
                let now = Instant::now();
                if now >= deadline {
                    pending.freshness = true;
                    let ticks =
                        now.duration_since(deadline).as_secs() / READER_FRESHNESS.as_secs() + 1;
                    deadline += Duration::from_secs(ticks * READER_FRESHNESS.as_secs());
                }
                if pending.freshness || !pending.accounts.is_empty() {
                    break;
                }
                let (next, _) = store
                    .reader_control
                    .changed
                    .wait_timeout(pending, deadline.saturating_duration_since(now))
                    .unwrap_or_else(|p| p.into_inner());
                pending = next;
            }
            let accounts = if pending.freshness {
                pending.latest.keys().cloned().collect()
            } else {
                std::mem::take(&mut pending.accounts)
            };
            pending.accounts.clear();
            pending.freshness = false;
            accounts
                .into_iter()
                .filter_map(|id| pending.latest.get(&id).cloned().map(|intent| (id, intent)))
                .collect::<Vec<_>>()
        };
        let at = (store.publication_clock)();
        for (id, intent) in jobs {
            let Some(account) = store.account_store(&id) else {
                continue;
            };
            if *intent == ReaderIntent::Unconfirmed {
                let pending = store
                    .reader_control
                    .pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if !pending.stopped
                    && pending
                        .latest
                        .get(&id)
                        .is_some_and(|latest| Arc::ptr_eq(latest, &intent))
                {
                    account.mark_unavailable();
                }
                continue;
            }
            let candidate = at
                .ok_or_else(|| {
                    ReaderError::InvalidValue(
                        "publication clock is outside the supported range".to_owned(),
                    )
                })
                .and_then(|at| store.prepare_current_candidate_at(&id, at));
            match candidate {
                Ok(Some(candidate)) => {
                    store.complete_reader_lane(&id, &intent, Ok(candidate), ReaderLane::Current)
                }
                Ok(None) => {} // Legacy current authority remains in the full reader.
                Err(error) => {
                    store.complete_reader_lane(&id, &intent, Err(error), ReaderLane::Current)
                }
            }
            if let Some(at) = at {
                let mut pending = store
                    .reader_control
                    .pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if !pending.stopped
                    && pending
                        .latest
                        .get(&id)
                        .is_some_and(|latest| Arc::ptr_eq(latest, &intent))
                {
                    pending
                        .history
                        .insert(id.clone(), (Arc::clone(&intent), at));
                    store.reader_control.history_changed.notify_one();
                    if account
                        .snapshot()
                        .is_none_or(|frame| frame.recordings.is_none())
                    {
                        pending.recordings.insert(id, (intent, at));
                        store.reader_control.recordings_changed.notify_one();
                    }
                }
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
    CurrentV4,
    CurrentV5,
    HistoryScopesV6,
    HistoryRecordingsV4,
    HistoryObservationsV4,
    HistoryPeriodsV3,
    SelectableHistoryPeriodsV3,
    HistoryPeriodsV4,
    SelectableHistoryPeriodsV4,
    HistoryPeriodsV5,
    SelectableHistoryPeriodsV5,
    HistoryV3,
    ThreadsV3,
}

impl Route {
    fn history_resource(self) -> bool {
        matches!(
            self,
            Self::Details
                | Self::DetailsV2
                | Self::DetailsV3
                | Self::HistoryPeriodsV3
                | Self::SelectableHistoryPeriodsV3
                | Self::HistoryPeriodsV4
                | Self::SelectableHistoryPeriodsV4
                | Self::HistoryPeriodsV5
                | Self::SelectableHistoryPeriodsV5
                | Self::HistoryV3
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Health => "/v1/health",
            Self::Runtime => "/v1/runtime",
            Self::Details => "/v1/details",
            Self::DetailsV2 => "/v2/details",
            Self::DetailsV3 => "/v3/details",
            Self::AccountsV3 => "/v3/accounts",
            Self::CurrentV3 => "/v3/current",
            Self::CurrentV4 => "/v4/current",
            Self::CurrentV5 => "/v5/current",
            Self::HistoryScopesV6 => "/v6/history/scopes",
            Self::HistoryRecordingsV4 => "/v4/history/recordings",
            Self::HistoryObservationsV4 => "/v4/history/observations",
            Self::HistoryPeriodsV3 => "/v3/history/periods",
            Self::SelectableHistoryPeriodsV3 => "/v3/history/periods/selectable",
            Self::HistoryPeriodsV4 => "/v4/history/periods",
            Self::SelectableHistoryPeriodsV4 => "/v4/history/periods/selectable",
            Self::HistoryPeriodsV5 => "/v5/history/periods",
            Self::SelectableHistoryPeriodsV5 => "/v5/history/periods/selectable",
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
                | Self::CurrentV4
                | Self::CurrentV5
                | Self::HistoryScopesV6
                | Self::HistoryRecordingsV4
                | Self::HistoryObservationsV4
                | Self::HistoryPeriodsV3
                | Self::SelectableHistoryPeriodsV3
                | Self::HistoryPeriodsV4
                | Self::SelectableHistoryPeriodsV4
                | Self::HistoryPeriodsV5
                | Self::SelectableHistoryPeriodsV5
                | Self::HistoryV3
                | Self::ThreadsV3
        )
    }

    fn directory_v5(self) -> bool {
        matches!(
            self,
            Self::HistoryPeriodsV5 | Self::SelectableHistoryPeriodsV5
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
        let (accounts, pair) = store.capture_accounts();
        if request.if_none_match.as_deref() == Some(pair.as_str()) {
            write_json_response(
                stream,
                &store.diagnostics,
                route_label,
                304,
                Vec::new(),
                Some(&pair),
                true,
            );
            return;
        }
        match flatten_with_version(API_VERSION_V3, &accounts) {
            Ok(body) => write_json_response(
                stream,
                &store.diagnostics,
                route_label,
                200,
                body,
                Some(&pair),
                false,
            ),
            Err(RouteError::Serialization) => write_json_response(
                stream,
                &store.diagnostics,
                route_label,
                500,
                error_body("internal_error"),
                None,
                false,
            ),
            Err(_) => unreachable!("account descriptors are validated at startup"),
        }
        return;
    }

    if let Some(account_id) = request.account.as_deref() {
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
    }
    let (snapshot, reader_unavailable, history_freshness) =
        store.capture_resource(request.account.as_deref(), route);
    if reader_unavailable || store.publication_failed() {
        if !reader_unavailable {
            store.diagnostics.record(
                route_label,
                "publication_guard",
                "pair_issuance_failed",
                None,
            );
        }
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
    }
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
    let degraded = snapshot.acquisition_degraded;
    let response_pair = snapshot.pair.as_str();
    // Qualification belongs to this captured route/query even when the pair
    // matches. A conditional response cannot hide a failed history resource,
    // an invalid cursor or a transport boundary. This reads no database.
    let body = match serialize_route_with_diagnostics(
        &snapshot,
        route,
        request.period.as_deref(),
        request.cursor,
        degraded,
        &store.diagnostics,
        route_label,
    ) {
        Ok(body) => body,
        Err(error) => {
            write_route_error(stream, &store.diagnostics, route_label, error);
            return;
        }
    };
    if route.v3()
        && request
            .if_none_match
            .as_deref()
            .is_some_and(|pair| pair == response_pair)
    {
        write_json_response_with_history(
            stream,
            &store.diagnostics,
            route_label,
            304,
            Vec::new(),
            (Some(response_pair), history_freshness),
            true,
        );
        return;
    }
    {
        if matches!(
            route,
            Route::Details
                | Route::DetailsV2
                | Route::DetailsV3
                | Route::CurrentV3
                | Route::CurrentV4
        ) && (degraded
            || if route == Route::CurrentV4 {
                snapshot.current.state == PublicState::Error
            } else {
                snapshot.details.state == PublicState::Error
            })
        {
            let reason = if degraded && snapshot.acquisition_degraded {
                "acquisition_failed"
            } else if degraded {
                "last_good_after_refresh_failure"
            } else if snapshot.details.authenticated {
                "incomplete_current_projection"
            } else {
                "account_boundary_error"
            };
            store
                .diagnostics
                .record(route_label, "publication_state", reason, Some(200));
        }
        write_json_response_with_history(
            stream,
            &store.diagnostics,
            route_label,
            200,
            body,
            (Some(response_pair), history_freshness),
            false,
        )
    }
}

fn write_route_error(stream: &mut TcpStream, log: &FailureLog, route: &str, error: RouteError) {
    match error {
        RouteError::SnapshotUnavailable => write_json_response(
            stream,
            log,
            route,
            503,
            error_body("snapshot_unavailable"),
            None,
            false,
        ),
        RouteError::StaleCursor => write_json_response(
            stream,
            log,
            route,
            400,
            error_body("stale_cursor"),
            None,
            false,
        ),
        RouteError::HistoryUnavailable => write_json_response(
            stream,
            log,
            route,
            503,
            error_body("history_unavailable"),
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
        RouteError::ResourceTooLarge => write_json_response(
            stream,
            log,
            route,
            413,
            error_body("resource_too_large"),
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
                continue;
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
        "/v4/current" => Some(Route::CurrentV4),
        "/v5/current" => Some(Route::CurrentV5),
        "/v6/history/scopes" => Some(Route::HistoryScopesV6),
        "/v4/history/recordings" => Some(Route::HistoryRecordingsV4),
        "/v4/history/observations" => Some(Route::HistoryObservationsV4),
        "/v3/history/periods" => Some(Route::HistoryPeriodsV3),
        "/v3/history/periods/selectable" => Some(Route::SelectableHistoryPeriodsV3),
        "/v4/history/periods" => Some(Route::HistoryPeriodsV4),
        "/v4/history/periods/selectable" => Some(Route::SelectableHistoryPeriodsV4),
        "/v5/history/periods" => Some(Route::HistoryPeriodsV5),
        "/v5/history/periods/selectable" => Some(Route::SelectableHistoryPeriodsV5),
        "/v3/history" => Some(Route::HistoryV3),
        "/v3/threads" => Some(Route::ThreadsV3),
        _ => None,
    };
    if route.is_some_and(Route::directory_v5) || route == Some(Route::HistoryScopesV6) {
        if (target.contains('?') && query.is_empty()) || query.contains('%') {
            return Err(ParseError::BadRequest);
        }
        let mut account = None;
        let mut cursor = None;
        let mut unknown = false;
        for pair in query.split('&').filter(|_| !query.is_empty()) {
            let (name, value) = pair.split_once('=').ok_or(ParseError::BadRequest)?;
            if value.is_empty() || value.contains('=') {
                return Err(ParseError::BadRequest);
            }
            match name {
                "account" if account.is_none() && valid_account_selector(value) => {
                    account = Some(value.to_owned());
                }
                "cursor" if cursor.is_none() => cursor = Some(value.to_owned()),
                "account" | "cursor" => return Err(ParseError::BadRequest),
                _ => unknown = true,
            }
        }
        if unknown && route == Some(Route::HistoryScopesV6) {
            return Err(ParseError::BadRequest);
        }
        return Ok((if unknown { None } else { route }, account, None, cursor));
    }
    if matches!(
        route,
        Some(Route::HistoryRecordingsV4 | Route::HistoryObservationsV4)
    ) {
        if query.is_empty() || query.contains('%') {
            return Err(ParseError::BadRequest);
        }
        let mut account = None;
        let mut cursor = None;
        let mut start = None;
        let mut end = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or(ParseError::BadRequest)?;
            if value.is_empty() || value.contains('=') {
                return Err(ParseError::BadRequest);
            }
            let time = || {
                value.parse::<i64>().ok().filter(|v| {
                    *v > 0
                        && v.to_string() == value
                        && *v <= codex_info_rest_contract::recorded_usage::MAX_END_EXCLUSIVE
                })
            };
            match key {
                "account" if account.is_none() && valid_account_selector(value) => {
                    account = Some(value.to_owned())
                }
                "cursor" if cursor.is_none() => cursor = Some(value.to_owned()),
                "start" if start.is_none() => start = Some(time().ok_or(ParseError::BadRequest)?),
                "end" if end.is_none() => end = Some(time().ok_or(ParseError::BadRequest)?),
                _ => return Err(ParseError::BadRequest),
            }
        }
        let (start, end) = start
            .zip(end)
            .filter(|(start, end)| start < end)
            .ok_or(ParseError::BadRequest)?;
        return Ok((route, account, Some(format!("{start}:{end}")), cursor));
    }
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
    SnapshotUnavailable,
    HistoryUnavailable,
    UnknownPeriod,
    StaleCursor,
    ResourceTooLarge,
}

fn serialize_route(
    snapshot: &PublishedSnapshot,
    route: Route,
    period: Option<&str>,
    cursor: Option<String>,
    degraded: bool,
) -> Result<Vec<u8>, RouteError> {
    serialize_route_with_budget(snapshot, route, period, cursor, degraded, MAX_BODY_BYTES)
}

fn serialize_route_with_diagnostics(
    snapshot: &PublishedSnapshot,
    route: Route,
    period: Option<&str>,
    cursor: Option<String>,
    degraded: bool,
    log: &FailureLog,
    route_label: &str,
) -> Result<Vec<u8>, RouteError> {
    let result = serialize_route(snapshot, route, period, cursor, degraded);
    if matches!(&result, Err(RouteError::SnapshotUnavailable)) {
        log.record(
            route_label,
            "recorded_projection",
            "recorded_archive_unavailable",
            None,
        );
    }
    result
}

fn serialize_route_with_budget(
    snapshot: &PublishedSnapshot,
    route: Route,
    period: Option<&str>,
    cursor: Option<String>,
    degraded: bool,
    directory_body_budget: usize,
) -> Result<Vec<u8>, RouteError> {
    if !snapshot.history_available
        && matches!(
            route,
            Route::HistoryPeriodsV3
                | Route::SelectableHistoryPeriodsV3
                | Route::HistoryV3
                | Route::HistoryPeriodsV4
                | Route::SelectableHistoryPeriodsV4
                | Route::HistoryPeriodsV5
                | Route::SelectableHistoryPeriodsV5
        )
    {
        return Err(RouteError::HistoryUnavailable);
    }
    let details = &snapshot.details;
    match route {
        Route::CurrentV5
        | Route::HistoryScopesV6
        | Route::HistoryRecordingsV4
        | Route::HistoryObservationsV4 => recordings::serialize(
            snapshot,
            route,
            period,
            cursor.as_deref(),
            degraded,
            directory_body_budget,
        ),
        Route::Health => Ok(health_body()),
        Route::Runtime => unreachable!("runtime versions do not use a database snapshot"),
        Route::Details => {
            if snapshot
                .history_samples_v2
                .iter()
                .any(|row| row.as_legacy_sample().is_none())
            {
                return Err(RouteError::HistoryUnavailable);
            }
            validate_compatibility_history_capacity(snapshot)?;
            let details = details_v1(snapshot, degraded);
            flatten_with_version(API_VERSION, &details)
        }
        Route::DetailsV2 => {
            validate_compatibility_history_capacity(snapshot)?;
            let details = details_v2_for_wire(snapshot, degraded);
            flatten_with_version(API_VERSION_V2, &details)
        }
        Route::DetailsV3 => {
            validate_compatibility_history_capacity(snapshot)?;
            let details = details_v3(snapshot, degraded);
            flatten_with_version(API_VERSION_V3, &details)
        }
        Route::AccountsV3 => Err(RouteError::Serialization),
        Route::CurrentV3 | Route::CurrentV4 => {
            let state = if degraded {
                PublicState::Error
            } else if route == Route::CurrentV4 {
                snapshot.current.state
            } else {
                details.state
            };
            let mut current = json!({
                "api_version": API_VERSION_V3,
                "state": state,
                "observed_at": details.observed_at,
                "authenticated": details.authenticated,
                "plan_label": details.plan_label,
                "quota": details.quota,
                "models": snapshot.models_v3,
                "active_thread_count": details.active_thread_count,
                "open_session_thread_count": details.threads.len(),
            });
            if route == Route::CurrentV4 {
                current["api_version"] = json!("v4");
                current["models_status"] = json!(snapshot.current.models_status);
            }
            serialize_json(&current)
        }
        Route::HistoryPeriodsV4 | Route::SelectableHistoryPeriodsV4 => {
            let old = if route == Route::HistoryPeriodsV4 {
                Route::HistoryPeriodsV3
            } else {
                Route::SelectableHistoryPeriodsV3
            };
            let mut document: Value =
                serde_json::from_slice(&serialize_route(snapshot, old, None, None, degraded)?)
                    .map_err(|_| RouteError::Serialization)?;
            document["api_version"] = json!("v4");
            for period in document["history_periods"]
                .as_array_mut()
                .expect("period directory array")
            {
                let origin = period["reset_at"]
                    .as_i64()
                    .and_then(|reset| snapshot.retained_history.origins.get(&reset));
                period["origin_at"] = json!(origin);
            }
            serialize_json(&document)
        }
        Route::HistoryPeriodsV3 => {
            // Keep the exact archive and direct history lookup intact. Only
            // omit past selector entries with no in-bounds observations or gaps.
            let periods = snapshot
                .retained_history
                .periods
                .iter()
                .filter(|period| {
                    period.current
                        || !snapshot
                            .history_index
                            .period(&period.id)
                            .is_some_and(|index| index.samples.is_empty() && index.gaps.is_empty())
                })
                .collect::<Vec<_>>();
            serialize_json(&json!({
                "api_version": API_VERSION_V3,
                "history_periods": periods,
            }))
        }
        Route::SelectableHistoryPeriodsV3 => {
            // Selection requires recorded token usage, not a claim that
            // omitted periods were unused. The full directory and pages remain.
            let periods = snapshot
                .retained_history
                .periods
                .iter()
                .filter(|period| {
                    period.current
                        || snapshot
                            .history_index
                            .period(&period.id)
                            .is_some_and(|index| index.has_token_usage)
                })
                .collect::<Vec<_>>();
            serialize_json(&json!({
                "api_version": API_VERSION_V3,
                "history_periods": periods,
            }))
        }
        Route::HistoryPeriodsV5 | Route::SelectableHistoryPeriodsV5 => {
            serialize_directory(snapshot, route, cursor.as_deref(), directory_body_budget)
        }
        Route::ThreadsV3 => serialize_json(&json!({
            "api_version": API_VERSION_V3,
            "threads": details.threads,
        })),
        Route::HistoryV3 => serialize_history(snapshot, period, cursor),
    }
}

fn validate_compatibility_history_capacity(snapshot: &PublishedSnapshot) -> Result<(), RouteError> {
    let limit = codex_info_rest_contract::MAX_PUBLIC_HISTORY_SAMPLES;
    if snapshot.details.history_samples.len() > limit
        || snapshot.history_samples_v2.len() > limit
        || snapshot.history_samples_v3.len() > limit
    {
        return Err(RouteError::ResourceTooLarge);
    }
    Ok(())
}

fn directory_indices(archive: &HistoryArchive, history: &HistoryIndex) -> [Vec<usize>; 2] {
    let mut all = (0..archive.periods.len())
        .filter(|&index| {
            let period = &archive.periods[index];
            period.current
                || history
                    .period(&period.id)
                    .is_some_and(|entry| !entry.samples.is_empty() || !entry.gaps.is_empty())
        })
        .collect::<Vec<_>>();
    all.sort_by(|&left, &right| {
        let left = &archive.periods[left];
        let right = &archive.periods[right];
        (left.reset_at, left.id.as_bytes()).cmp(&(right.reset_at, right.id.as_bytes()))
    });
    let selectable = all
        .iter()
        .copied()
        .filter(|&index| {
            let period = &archive.periods[index];
            period.current
                || history
                    .period(&period.id)
                    .is_some_and(|entry| entry.has_token_usage)
        })
        .collect();
    [all, selectable]
}

fn directory_kind(route: Route) -> (usize, char) {
    match route {
        Route::HistoryPeriodsV5 => (0, 'a'),
        Route::SelectableHistoryPeriodsV5 => (1, 's'),
        _ => unreachable!("only v5 directories have a directory cursor"),
    }
}

fn directory_cursor(snapshot: &PublishedSnapshot, route: Route, offset: usize) -> String {
    let (_, kind) = directory_kind(route);
    format!("d1.{}.{kind}.{offset}", &snapshot.pair[3..])
}

fn directory_start(
    snapshot: &PublishedSnapshot,
    route: Route,
    cursor: Option<&str>,
) -> Result<usize, RouteError> {
    if !snapshot.history_available {
        return Err(RouteError::HistoryUnavailable);
    }
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let (index, kind) = directory_kind(route);
    let prefix = format!("d1.{}.{kind}.", &snapshot.pair[3..]);
    let raw_offset = cursor
        .strip_prefix(&prefix)
        .ok_or(RouteError::StaleCursor)?;
    let offset = raw_offset
        .parse::<usize>()
        .map_err(|_| RouteError::StaleCursor)?;
    if offset == 0
        || offset >= snapshot.directory_indices[index].len()
        || offset.to_string() != raw_offset
    {
        return Err(RouteError::StaleCursor);
    }
    Ok(offset)
}

#[derive(serde::Serialize)]
struct DirectoryPeriodV5<'a> {
    id: &'a str,
    start_at: i64,
    end_at: i64,
    reset_at: i64,
    label: &'a str,
    current: bool,
    origin_at: Option<i64>,
}

fn serialize_directory(
    snapshot: &PublishedSnapshot,
    route: Route,
    cursor: Option<&str>,
    budget: usize,
) -> Result<Vec<u8>, RouteError> {
    let start = directory_start(snapshot, route, cursor)?;
    let (index, _) = directory_kind(route);
    let indices = &snapshot.directory_indices[index];
    let mut cached = snapshot
        .directory_pages
        .sequences
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let sequence = cached.entry((index, budget)).or_default();
    if let Some(body) = sequence.pages.get(&start) {
        return Ok(body.clone());
    }
    let mut body = br#"{"api_version":"v5","history_periods":["#.to_vec();
    let mut end = start;
    const SUFFIX: &[u8] = b"],\"next_cursor\":";
    while end < indices.len() {
        #[cfg(test)]
        DIRECTORY_ROW_ENCODINGS.with(|count| count.set(count.get() + 1));
        let period = &snapshot.retained_history.periods[indices[end]];
        let row = serde_json::to_vec(&DirectoryPeriodV5 {
            id: &period.id,
            start_at: period.start_at,
            end_at: period.end_at,
            reset_at: period.reset_at,
            label: &period.label,
            current: period.current,
            origin_at: snapshot
                .retained_history
                .origins
                .get(&period.reset_at)
                .copied(),
        })
        .map_err(|_| RouteError::Serialization)?;
        let next = (end + 1 < indices.len()).then(|| directory_cursor(snapshot, route, end + 1));
        let encoded_next = serde_json::to_vec(&next).map_err(|_| RouteError::Serialization)?;
        let comma = usize::from(end > start);
        let length = body
            .len()
            .checked_add(comma)
            .and_then(|length| length.checked_add(row.len()))
            .and_then(|length| length.checked_add(SUFFIX.len()))
            .and_then(|length| length.checked_add(encoded_next.len()))
            .and_then(|length| length.checked_add(1));
        if length.is_none_or(|length| length > budget) {
            if end == start {
                return Err(RouteError::ResourceTooLarge);
            }
            break;
        }
        if comma != 0 {
            body.push(b',');
        }
        body.extend_from_slice(&row);
        end += 1;
    }
    let next = (end < indices.len()).then(|| directory_cursor(snapshot, route, end));
    body.extend_from_slice(SUFFIX);
    serde_json::to_writer(&mut body, &next).map_err(|_| RouteError::Serialization)?;
    body.push(b'}');
    if body.len() > budget {
        return Err(RouteError::ResourceTooLarge);
    }
    // Retain only non-overlapping pages along the server-issued sequence.
    // Arbitrary valid offsets still work, without creating overlapping copies
    // of the complete directory in memory. The lock also coalesces concurrent
    // connections requesting the same not-yet-encoded canonical page.
    if start == sequence.frontier {
        sequence.pages.insert(start, body.clone());
        sequence.frontier = end;
    }
    Ok(body)
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
            .start_after(&snapshot.retained_history.samples, cursor)
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
            &snapshot.retained_history.samples[row]
        })
        .collect::<Vec<_>>();
    let resume_cursor = if end > 0 {
        Some(indexed.resume(&snapshot.retained_history.samples, end - 1))
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
            .map(|&row| &snapshot.retained_history.gaps[row])
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
    write_json_response_with_history(stream, log, route, status, body, (pair, None), not_modified);
}

fn write_json_response_with_history(
    stream: &mut TcpStream,
    log: &FailureLog,
    route: &str,
    status: u16,
    body: Vec<u8>,
    publication: (Option<&str>, Option<HistoryFreshness>),
    not_modified: bool,
) {
    let (pair, history) = publication;
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
    let history_header = history
        .filter(|_| status == 200 || status == 304)
        .map(|history| {
            format!(
                "Codex-Info-History-State: {}\r\nCodex-Info-History-Verified-At: {}\r\n",
                if history.current {
                    "verified"
                } else {
                    "refreshing"
                },
                history.verified_at,
            )
        })
        .unwrap_or_default();
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n{pair_header}{history_header}Content-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
    fn issue604_recorded_scope_resources_preserve_unknown_period_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recorded-scope.sqlite3");
        fixture(&path, 1);
        let store = Arc::new(SnapshotStore::new(DbReader::open(&path).unwrap()));
        let snapshot = history_test_fixture(1, 1, 1, false, false, "1802678400");
        store
            .account_store("account-1")
            .unwrap()
            .inner
            .write()
            .unwrap()
            .current = Some(Arc::new(snapshot));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::clone(&store);
        let worker = thread::spawn(move || {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                handle_connection(&mut stream, &captured);
            }
        });
        let routes = [
            "/v5/current",
            "/v6/history/scopes",
            "/v4/history/recordings?start=1791623262&end=1792228062",
            "/v4/history/observations?start=1791623262&end=1792228062",
        ];
        let responses = routes
            .map(|route| request(address, &format!("GET {route} HTTP/1.1\r\nHost:x\r\n\r\n")));
        worker.join().unwrap();
        let mut pair = None;
        for (route, response) in routes.into_iter().zip(responses) {
            assert!(
                response.starts_with("HTTP/1.1 200"),
                "{route} must expose the scope contract: {}",
                response.lines().next().unwrap()
            );
            let actual_pair = published_pair(&response).to_owned();
            assert!(pair
                .as_ref()
                .is_none_or(|previous| previous == &actual_pair));
            pair = Some(actual_pair);
            let value: Value = serde_json::from_str(body(&response)).unwrap();
            assert_eq!(value["period_identity"]["state"], "unknown");
            assert_eq!(
                value["period_identity"]["reason"],
                "provider-period-identity-unavailable"
            );
            if route == "/v5/current" {
                assert!(value["models"].as_array().unwrap().is_empty());
                assert_eq!(value["models_status"], "unavailable");
            }
        }
    }

    fn recorded_literal_snapshot() -> (PublishedSnapshot, Value) {
        use codex_info_rest_contract::recorded_usage::*;
        let literal: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/recorded_scope_contract.json"
        ))
        .unwrap();
        let mut archive = codex_info_db_reader::RecordedArchive {
            coverage: RecordingCoverage::Partial,
            ..Default::default()
        };
        let mut prefix = RecordedTokenPrefix::default();
        // Independent source A and B. Their original R values differ. The
        // literal replay is checked by the writer identity consumer, not
        // promoted into another prefix event here.
        for input in literal["independent_inputs"]
            .as_array()
            .unwrap()
            .iter()
            .take(2)
        {
            prefix
                .add(codex_info_rest_contract::token_facts::TokenFacts {
                    total: input["total"].as_u64().unwrap(),
                    input: input["input"].as_u64(),
                    cached_input: input["cached_input"].as_u64(),
                    cache_write_input: input["cache_write_input"].as_u64(),
                    output: input["output"].as_u64(),
                })
                .unwrap();
            archive
                .prefixes
                .entry("gpt-6.1-sol".into())
                .or_default()
                .push((input["timestamp"].as_i64().unwrap(), prefix.clone()));
        }
        archive.quota_observations =
            serde_json::from_value(literal["recordings"]["quota_observations"].clone()).unwrap();
        archive.observations =
            serde_json::from_value(literal["observations"]["observations"].clone()).unwrap();
        let mut snapshot = history_test_fixture(1, 1, 1, false, false, "1802678400");
        snapshot.recordings = Some(Arc::new(archive));
        snapshot.details.authenticated = true;
        snapshot.details.plan_label = Some("Plus".into());
        snapshot.details.quota =
            Some(serde_json::from_value(literal["current"]["quota"].clone()).unwrap());
        snapshot.acquisition_degraded = false;
        snapshot.models_v3 = vec![codex_info_rest_contract::PublicModelUsageV3 {
            model: "gpt-6.1-sol".into(),
            total_tokens: 66123101,
            input_tokens: 65862746,
            cached_input_tokens: 65275264,
            cache_write_input_tokens: Some(0),
            output_tokens: 260355,
            estimated_cost: None,
        }];
        (snapshot, literal)
    }

    #[test]
    fn issue604_recorded_literal_survives_reset_switch_and_page_boundaries() {
        let (snapshot, literal) = recorded_literal_snapshot();
        let current: Value = serde_json::from_slice(
            &serialize_route(&snapshot, Route::CurrentV5, None, None, false).unwrap(),
        )
        .unwrap();
        assert_eq!(current["models"], literal["current"]["models"]);
        assert_eq!(current["usage_scope"], literal["current"]["usage_scope"]);
        assert_eq!(current["state"], "ready");
        let mut alternate = snapshot.clone();
        alternate.details.quota.as_mut().unwrap().reset_at = 1792823931;
        let after: Value = serde_json::from_slice(
            &serialize_route(&alternate, Route::CurrentV5, None, None, false).unwrap(),
        )
        .unwrap();
        assert_eq!(after["models"], current["models"]);
        assert_eq!(after["usage_scope"], current["usage_scope"]);
        let directory: Value = serde_json::from_slice(
            &serialize_route(&snapshot, Route::HistoryScopesV6, None, None, false).unwrap(),
        )
        .unwrap();
        let other: Value = serde_json::from_slice(
            &serialize_route(&alternate, Route::HistoryScopesV6, None, None, false).unwrap(),
        )
        .unwrap();
        assert_eq!(directory["scopes"], other["scopes"]);
        assert_eq!(
            directory["scopes"].as_array().unwrap().len(),
            2,
            "R-only change cannot add an actual-week selector"
        );
        let range = "1791623262:1792228062";
        let mut cursor = None;
        let mut samples = Vec::new();
        let mut quota = Vec::new();
        let mut cursors = Vec::new();
        loop {
            let page: Value = serde_json::from_slice(
                &serialize_route_with_budget(
                    &snapshot,
                    Route::HistoryRecordingsV4,
                    Some(range),
                    cursor.clone(),
                    false,
                    800,
                )
                .unwrap(),
            )
            .unwrap();
            samples.extend(page["samples"].as_array().unwrap().iter().cloned());
            quota.extend(
                page["quota_observations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned(),
            );
            assert_eq!(page["scope"], literal["recordings"]["scope"]);
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            if let Some(cursor) = &cursor {
                cursors.push(cursor.clone());
            } else {
                break;
            }
            assert!(cursors.len() < 10);
        }
        assert!(
            !cursors.is_empty(),
            "finite budget must exercise a real page boundary"
        );
        assert_eq!(
            samples,
            literal["recordings"]["samples"]
                .as_array()
                .unwrap()
                .as_slice()
        );
        assert_eq!(
            quota,
            literal["recordings"]["quota_observations"]
                .as_array()
                .unwrap()
                .as_slice()
        );
        let stale = &cursors[0];
        assert_eq!(
            serialize_route(
                &snapshot,
                Route::HistoryObservationsV4,
                Some(range),
                Some(stale.clone()),
                false
            ),
            Err(RouteError::StaleCursor)
        );
        assert_eq!(
            serialize_route(
                &snapshot,
                Route::HistoryRecordingsV4,
                Some("1791623263:1792228062"),
                Some(stale.clone()),
                false
            ),
            Err(RouteError::StaleCursor)
        );
        let mut peer = snapshot.clone();
        peer.recorded_scope_account += 1;
        assert_eq!(
            serialize_route(
                &peer,
                Route::HistoryRecordingsV4,
                Some(range),
                Some(stale.clone()),
                false
            ),
            Err(RouteError::StaleCursor)
        );
        let saved: Value = serde_json::from_slice(
            &serialize_route(
                &snapshot,
                Route::HistoryObservationsV4,
                Some(range),
                None,
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved["observations"],
            literal["observations"]["observations"]
        );
        for bad in [
            "/v4/history/recordings?start=01&end=2",
            "/v4/history/recordings?start=1&end=1",
            "/v4/history/observations?start=1&end=2&limit=1",
            "/v4/history/recordings?start=1&start=2&end=3",
        ] {
            assert_eq!(parse_target(bad), Err(ParseError::BadRequest));
        }
    }

    #[test]
    fn issue604_recorded_projection_failure_never_becomes_successful_empty_current() {
        let (mut snapshot, _) = recorded_literal_snapshot();
        snapshot.recordings = None;
        assert!(serialize_route(&snapshot,Route::CurrentV5,None,None,false).is_err(),"failed recorded projection must retain client last good through HTTP failure, never publish empty success");
    }

    #[test]
    fn issue604_existing_ui_routes_keep_saved_models_and_periods() {
        let (mut snapshot, _) = recorded_literal_snapshot();
        snapshot.current.models_status = codex_info_db_reader::CurrentModelsStatus::Partial;
        for auxiliary_archive in [true, false] {
            if !auxiliary_archive {
                snapshot.recordings = None;
            }
            let current: Value = serde_json::from_slice(
                &serialize_route(&snapshot, Route::CurrentV4, None, None, false).unwrap(),
            )
            .unwrap();
            assert_eq!(current["models"][0]["total_tokens"], 66123101);
            assert_eq!(current["models_status"], "partial");
            assert!(current["quota"].is_object());
            for route in [
                Route::HistoryPeriodsV3,
                Route::SelectableHistoryPeriodsV3,
                Route::HistoryPeriodsV4,
                Route::SelectableHistoryPeriodsV4,
                Route::HistoryPeriodsV5,
                Route::SelectableHistoryPeriodsV5,
            ] {
                let directory: Value = serde_json::from_slice(
                    &serialize_route(&snapshot, route, None, None, false).unwrap(),
                )
                .unwrap();
                assert_eq!(directory["history_periods"][0]["id"], "1802678400");
            }
            let history: Value = serde_json::from_slice(
                &serialize_route(&snapshot, Route::HistoryV3, Some("1802678400"), None, false)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(history["history_samples"].as_array().unwrap().len(), 1);
            assert_eq!(history["history_samples"][0]["timestamp"], 1800000000);
        }
    }

    #[test]
    fn issue604_compatibility_sample_overflow_is_isolated_from_split_resources() {
        let initial = history_test_fixture(10, 1, 44_640, false, false, "1802678400");
        let mut samples = initial.history_samples_v3;
        let mut peer = initial.details.history_periods[0].clone();
        peer.id = "1802678401".into();
        peer.reset_at = 1_802_678_401;
        peer.start_at = 1_800_000_000;
        peer.end_at = peer.start_at;
        peer.current = false;
        samples.push(codex_info_rest_contract::PublicHistoryObservationV3 {
            timestamp: peer.start_at,
            reset_at: peer.reset_at,
            ..samples[0].clone()
        });
        let observations = samples
            .iter()
            .map(|point| codex_info_rest_contract::PublicHistoryObservation {
                timestamp: point.timestamp,
                reset_at: point.reset_at,
                remaining_percent: point.remaining_percent,
                sol_dollars: Some(0.0),
                terra_dollars: Some(0.0),
                luna_dollars: Some(0.0),
                sol_tokens: Some(0),
                terra_tokens: Some(0),
                luna_tokens: Some(0),
                model_source: "legacy-unknown".into(),
            })
            .collect::<Vec<_>>();
        let mut details = initial.details;
        details.history_periods.push(peer);
        details.history_samples = observations
            .iter()
            .map(|row| row.as_legacy_sample().unwrap())
            .collect();
        let snapshot = PublishedSnapshot::from_db_for_account(
            DbSnapshot {
                recordings: Some(Arc::default()),
                generation: 1,
                data_hash: "a".repeat(64),
                has_pending_ranges: false,
                acquisition_degraded: false,
                history_available: true,
                current: CurrentPublication::unavailable(PublicState::Ready),
                retained_history: HistoryArchive {
                    periods: details.history_periods.clone(),
                    samples: samples.clone(),
                    gaps: vec![],
                    origins: BTreeMap::new(),
                },
                reprojection_witness: None,
                details,
                models_v3: vec![],
                history_samples_v2: observations,
                history_samples_v3: samples,
            },
            10,
        );
        for route in [Route::Details, Route::DetailsV2, Route::DetailsV3] {
            assert!(
                matches!(
                    serialize_route(&snapshot, route, None, None, false),
                    Err(RouteError::ResourceTooLarge)
                ),
                "only the old details resource exceeds its sample limit"
            );
        }
        let current: Value = serde_json::from_slice(
            &serialize_route(&snapshot, Route::CurrentV4, None, None, false).unwrap(),
        )
        .unwrap();
        assert_eq!(current["state"], "ready");
        let directory: Value = serde_json::from_slice(
            &serialize_route(&snapshot, Route::HistoryPeriodsV5, None, None, false).unwrap(),
        )
        .unwrap();
        assert_eq!(directory["history_periods"].as_array().unwrap().len(), 2);
        let mut total = 0;
        for period in ["1802678400", "1802678401"] {
            let mut cursor = None;
            loop {
                let page = history_test_page(&snapshot, period, cursor.as_deref()).unwrap();
                total += page["history_samples"].as_array().unwrap().len();
                cursor = page["next_cursor"].as_str().map(str::to_owned);
                if cursor.is_none() {
                    break;
                }
            }
        }
        assert_eq!(total, 44_641, "split history keeps every exact period key");
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("issue604-conditional-capacity.sqlite3");
        fixture(&path, 1);
        let store = Arc::new(SnapshotStore::new(DbReader::open(&path).unwrap()));
        store
            .account_store("account-1")
            .unwrap()
            .inner
            .write()
            .unwrap()
            .current = Some(Arc::new(snapshot.clone()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let captured_store = Arc::clone(&store);
        let worker = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                handle_connection(&mut stream, &captured_store);
            }
        });
        let conditional = request(
            address,
            &format!(
                "GET /v3/details HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{}\"\r\n\r\n",
                snapshot.pair
            ),
        );
        let current = request(address, "GET /v4/current HTTP/1.1\r\nHost:x\r\n\r\n");
        worker.join().unwrap();
        assert!(
            conditional.starts_with("HTTP/1.1 413"),
            "a matching pair cannot conceal an unavailable resource: {}",
            conditional.lines().next().unwrap()
        );
        assert!(current.starts_with("HTTP/1.1 200"));
        assert_eq!(published_pair(&current), snapshot.pair);
        assert_eq!(store.snapshot().unwrap().pair, snapshot.pair);
        let mut unknown = snapshot;
        unknown.history_samples_v2[0].sol_tokens = None;
        assert_eq!(
            serialize_route(&unknown, Route::Details, None, None, false),
            Err(RouteError::HistoryUnavailable),
            "v1 representability is checked before transport capacity"
        );
    }

    #[test]
    fn issue604_unknown_legacy_history_cannot_be_dropped_from_a_v1_success() {
        let mut snapshot =
            PublishedSnapshot::account_boundary(PublicState::Error, 1, "fixture".into());
        snapshot.details.state = PublicState::Ready;
        snapshot.details.authenticated = true;
        snapshot.details.observed_at = Some(1_800_000_000);
        snapshot.details.quota = Some(codex_info_rest_contract::PublicQuota {
            reset_at: 1_800_000_600,
            window_seconds: 600,
            monthly: false,
            remaining_percent: 80.0,
        });
        snapshot.current = CurrentPublication::unavailable(PublicState::Ready);
        snapshot
            .history_samples_v2
            .push(codex_info_rest_contract::PublicHistoryObservation {
                timestamp: 1_800_000_000,
                reset_at: 1_800_000_600,
                remaining_percent: Some(80.0),
                sol_dollars: None,
                terra_dollars: None,
                luna_dollars: None,
                sol_tokens: None,
                terra_tokens: None,
                luna_tokens: None,
                model_source: "unavailable".into(),
            });
        snapshot
            .history_samples_v3
            .push(codex_info_rest_contract::PublicHistoryObservationV3 {
                timestamp: 1_800_000_000,
                reset_at: 1_800_000_600,
                remaining_percent: Some(80.0),
                task_active_since_previous: None,
                model_source: "confirmed".into(),
                models_complete: true,
                models: Some(vec![codex_info_rest_contract::PublicHistoryModelUsageV3 {
                    model: "gpt-6.1-sol".into(),
                    total_tokens: 100,
                    input_tokens: None,
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                    output_tokens: None,
                    total_dollars: None,
                }]),
            });
        assert_eq!(
            serialize_route(&snapshot, Route::Details, None, None, false),
            Err(RouteError::HistoryUnavailable),
            "v1 cannot encode this retained observation"
        );
        let json = |route| -> serde_json::Value {
            serde_json::from_slice(&serialize_route(&snapshot, route, None, None, false).unwrap())
                .unwrap()
        };
        let v2 = json(Route::DetailsV2);
        assert_eq!(v2["history_samples"].as_array().unwrap().len(), 1);
        assert_eq!(v2["history_samples"][0]["model_source"], "unavailable");
        assert!(v2["history_samples"][0]["sol_tokens"].is_null());
        let v3 = json(Route::DetailsV3);
        assert_eq!(v3["history_samples"][0]["models"][0]["total_tokens"], 100);
        assert!(v3["history_samples"][0]["models"][0]
            .get("input_tokens")
            .is_none());
        let current = json(Route::CurrentV4);
        assert_eq!(current["state"], "ready");
        assert_eq!(current["quota"]["remaining_percent"], 80.0);
    }

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
            recordings: Some(Arc::default()),
            recorded_scope_account: 0,
            generation: 1,
            data_hash: "hash".to_owned(),
            pair: "pair".to_owned(),
            has_pending_ranges: false,
            acquisition_degraded: false,
            history_available: true,
            current: CurrentPublication::unavailable(details.state),
            details,
            models_v3: Vec::new(),
            history_samples_v2: Vec::new(),
            history_samples_v3: Vec::new(),
            retained_history: HistoryArchive::default(),
            history_index: HistoryIndex::default(),
            directory_indices: Default::default(),
            directory_pages: Arc::default(),
            reprojection_witness: None,
            history_reprojection_witness: None,
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
            recordings: Some(Arc::default()),
            recorded_scope_account: 0,
            generation: 1,
            data_hash: "hash".to_owned(),
            pair: "pair".to_owned(),
            has_pending_ranges: false,
            acquisition_degraded: false,
            history_available: true,
            current: CurrentPublication::unavailable(details.state),
            details,
            models_v3: Vec::new(),
            history_samples_v2: Vec::new(),
            history_samples_v3: Vec::new(),
            retained_history: HistoryArchive::default(),
            history_index: HistoryIndex::default(),
            directory_indices: Default::default(),
            directory_pages: Arc::default(),
            reprojection_witness: None,
            history_reprojection_witness: None,
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

    #[test]
    fn issue604_standalone_listener_uses_a_new_epoch_for_each_publisher() {
        let first = RestServer::start_without_account(
            PublicState::Initializing,
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let second = RestServer::start_without_account(
            PublicState::Initializing,
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let first_response = request(
            first.local_addr(),
            "GET /v4/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let second_response = request(
            second.local_addr(),
            "GET /v4/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(first_response.starts_with("HTTP/1.1 200"));
        assert!(second_response.starts_with("HTTP/1.1 200"));
        let first_pair = published_pair(&first_response);
        let second_pair = published_pair(&second_response);
        assert_eq!(first_pair.len(), 67);
        assert_eq!(second_pair.len(), 67);
        assert_eq!(&first_pair[35..], "00000000000000000000000000000001");
        assert_eq!(&second_pair[35..], "00000000000000000000000000000001");
        assert_ne!(
            &first_pair[3..35],
            &second_pair[3..35],
            "independent publishers must not derive the epoch from the same boundary state"
        );
    }

    #[test]
    fn issue604_standalone_account_frames_share_epoch_and_allocate_distinct_counters() {
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("first.sqlite3");
        let second_path = directory.path().join("second.sqlite3");
        fixture(&first_path, 10);
        fixture(&second_path, 20);
        let store = SnapshotStore::new_with_accounts(
            vec![
                AccountReader::new(
                    "account-7",
                    7,
                    true,
                    None,
                    None,
                    DbReader::open(&first_path).unwrap(),
                ),
                AccountReader::new(
                    "account-13",
                    13,
                    false,
                    None,
                    None,
                    DbReader::open(&second_path).unwrap(),
                ),
            ],
            "account-7",
        )
        .unwrap();
        assert!(matches!(
            store.refresh_account("account-7"),
            RefreshStatus::Updated { .. }
        ));
        assert!(matches!(
            store.refresh_account("account-13"),
            RefreshStatus::Updated { .. }
        ));
        let first = store.snapshot_account("account-7").unwrap();
        let second = store.snapshot_account("account-13").unwrap();
        assert_eq!(
            &first.pair[3..35],
            &second.pair[3..35],
            "account storage identity and data generation are not a server epoch"
        );
        let first_counter = u128::from_str_radix(&first.pair[35..], 16).unwrap();
        let second_counter = u128::from_str_radix(&second.pair[35..], 16).unwrap();
        assert!(first_counter > 1);
        assert!(second_counter > first_counter);
        assert_ne!(first.pair, second.pair);
    }

    #[test]
    fn issue604_standalone_http_reads_the_captured_frame_without_refreshing_the_db() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("captured.sqlite3");
        fixture(&path, 10);
        let store = Arc::new(SnapshotStore::new(DbReader::open(&path).unwrap()));
        assert!(matches!(store.refresh(), RefreshStatus::Updated { .. }));
        let before = store.snapshot().unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "BEGIN IMMEDIATE; UPDATE usage_history SET sol_tokens=20; \
                 UPDATE collection_generation SET data_generation='2'; COMMIT;",
            )
            .unwrap();
        drop(connection);

        // Exercise the production HTTP handler with an explicitly published
        // frame. No reader worker runs in this harness, so only an HTTP-time
        // DB refresh can replace it with the newly committed generation.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handler_store = Arc::clone(&store);
        let handler = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handle_connection(&mut stream, &handler_store);
        });
        let response = request(address, "GET /v4/current HTTP/1.1\r\nHost:x\r\n\r\n");
        handler.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert_eq!(
            published_pair(&response),
            before.pair,
            "HTTP must return the captured frame rather than publish a DB change"
        );
        assert!(Arc::ptr_eq(&before, &store.snapshot().unwrap()));
    }

    #[test]
    fn issue604_health_does_not_wait_for_snapshot_publication_lock() {
        let store = Arc::new(SnapshotStore::new_without_account(PublicState::Initializing));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        // A publisher may still be validating a large historical frame. It
        // has not failed and must not borrow the readiness listener's budget.
        let publication = store.issuer.lock().unwrap();
        let server_store = Arc::clone(&store);
        let server_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || serve(listener, server_store, server_stop));
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(150)))
            .unwrap();
        stream
            .write_all(format!("GET /v1/health HTTP/1.1\r\nHost:{address}\r\n\r\n").as_bytes())
            .unwrap();
        let mut response = String::new();
        let received = stream.read_to_string(&mut response);
        stop.store(true, Ordering::Release);
        drop(publication);
        worker.join().unwrap();
        assert!(
            received.is_ok() && response.starts_with("HTTP/1.1 200"),
            "healthy listener must answer while publication is locked: {received:?}"
        );
        let document: Value = serde_json::from_str(body(&response)).unwrap();
        assert_eq!(document["service"], "codex-info");
        assert_eq!(document["api_version"], "v1");
        assert!(!store.publication_failed());
    }

    #[test]
    fn issue604_publication_counter_batch_is_atomic_and_exhaustion_is_permanent() {
        let mut issuer = PairIssuer::from_epoch([0x11; 16]).unwrap();
        let [first, second] = issuer.issue_batch::<2>().unwrap();
        assert_eq!(
            first,
            "v1:1111111111111111111111111111111100000000000000000000000000000001"
        );
        assert_eq!(
            second,
            "v1:1111111111111111111111111111111100000000000000000000000000000002"
        );
        issuer.counter = u128::MAX - 1;
        assert!(issuer.issue_batch::<2>().is_none());
        assert_eq!(
            issuer.counter,
            u128::MAX - 1,
            "insufficient space cannot partially reserve the final counter"
        );
        assert!(issuer.failed.load(Ordering::Acquire));
        assert!(
            issuer.issue().is_none(),
            "exhaustion cannot be recovered by requesting a smaller batch"
        );
        assert_eq!(issuer.counter, u128::MAX - 1);
        assert!(PairIssuer::from_epoch([0; 16]).is_err());
    }

    #[test]
    fn issue604_catalog_reconciliation_retains_other_frames_and_rejects_old_intents() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("catalog-a.sqlite3");
        let b = root.path().join("catalog-b.sqlite3");
        fixture(&a, 10);
        fixture(&b, 20);
        let readers = |current: &str| {
            vec![
                AccountReader::new(
                    "account-7",
                    7,
                    current == "account-7",
                    None,
                    None,
                    DbReader::open(&a).unwrap(),
                ),
                AccountReader::new(
                    "account-13",
                    13,
                    current == "account-13",
                    None,
                    None,
                    DbReader::open(&b).unwrap(),
                ),
            ]
        };
        let mut initial = readers("account-7");
        initial[0].source = ReaderIntent::Recorder(issue604_commit_notice(1));
        let store = SnapshotStore::new_with_accounts(initial, "account-7").unwrap();
        store.refresh_account("account-7");
        store.refresh_account("account-13");
        let first = store.snapshot_account("account-7").unwrap();
        let second = store.snapshot_account("account-13").unwrap();
        let old_intent =
            Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-7"]);
        let old_candidate = store.prepare_reader_candidate_at("account-7", 1_800_000_600);
        let (_, old_catalog_pair) = store.capture_accounts();
        assert!(store
            .reconcile_accounts(readers("account-13"), "account-13")
            .unwrap());
        let (catalog, pair) = store.capture_accounts();
        assert_eq!(catalog.default_account_id.as_deref(), Some("account-13"));
        assert_eq!(
            catalog
                .accounts
                .iter()
                .filter(|a| a.is_current)
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            ["account-13"]
        );
        assert_ne!(pair, old_catalog_pair);
        assert_eq!(
            &pair[3..35],
            &old_catalog_pair[3..35],
            "a catalog exchange keeps the process publisher epoch"
        );
        assert_eq!(
            store.capture_default_frame().0.unwrap().details.state,
            PublicState::Initializing
        );
        for (id, old) in [("account-7", &first), ("account-13", &second)] {
            assert!(
                Arc::ptr_eq(old, &store.snapshot_account(id).unwrap()),
                "unchanged account read scopes retain their exact last-good frames"
            );
        }
        let counter = store.issuer.lock().unwrap().counter;
        store.complete_reader_candidate("account-7", &old_intent, old_candidate);
        assert!(Arc::ptr_eq(
            &first,
            &store.snapshot_account("account-7").unwrap()
        ));
        assert_eq!(store.issuer.lock().unwrap().counter, counter);
        assert!(!store
            .reconcile_accounts(readers("account-13"), "account-13")
            .unwrap());
        assert_eq!(
            store.capture_accounts().1,
            pair,
            "same catalog has no new publication"
        );
        let mut invalid = readers("account-13");
        invalid[1].id = "account-7".into();
        assert!(store.reconcile_accounts(invalid, "account-13").is_err());
        assert_eq!(
            store.capture_accounts().1,
            pair,
            "invalid catalogs never partially change routing"
        );
        assert!(Arc::ptr_eq(
            &second,
            &store.snapshot_account("account-13").unwrap()
        ));
        let mut narrowed = readers("account-13");
        narrowed[0].reader = DbReader::open_with_intervals(
            &a,
            codex_info_db_reader::ReadIntervals::new(vec![
                codex_info_db_reader::ReadInterval::new(Some(1_800_000_000), None).unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(store.reconcile_accounts(narrowed, "account-13").unwrap());
        assert!(
            store.snapshot_account("account-7").is_none(),
            "a changed ownership scope must be reread before its old frame is reusable"
        );
        assert!(Arc::ptr_eq(
            &second,
            &store.snapshot_account("account-13").unwrap()
        ));
    }

    #[test]
    fn issue604_reader_discards_superseded_and_shutdown_candidates() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reader-admission.sqlite3");
        fixture(&path, 10);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("UPDATE collection_generation SET collector_epoch='11111111111111111111111111111111',cycle_seq='1'").unwrap();
        let store = SnapshotStore::new(DbReader::open(&path).unwrap());
        let old_intent =
            Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-1"]);
        let old_candidate = store.prepare_reader_candidate_at("account-1", 1_800_000_600);
        let notice = issue604_commit_notice(1);
        assert!(store.notify_recorder_commit("account-1", notice.clone()));
        store.complete_reader_candidate("account-1", &old_intent, old_candidate);
        assert!(store.snapshot_account("account-1").is_none());
        assert_eq!(
            store.issuer.lock().unwrap().counter,
            1,
            "a superseded candidate cannot spend a publication counter"
        );

        let latest = Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-1"]);
        let candidate = store.prepare_reader_candidate_at("account-1", 1_800_000_600);
        store.complete_reader_candidate("account-1", &latest, candidate);
        let before = store.snapshot_account("account-1").unwrap();
        assert!(store.recorder_commit_is_published("account-1", &notice));
        assert_eq!(&before.pair[35..], "00000000000000000000000000000002");

        connection.execute_batch("UPDATE collection_generation SET data_generation='2',cycle_seq='2'; UPDATE usage_history SET sol_tokens=20").unwrap();
        assert!(store.notify_recorder_commit("account-1", issue604_commit_notice(2)));
        let latest = Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-1"]);
        let candidate = store.prepare_reader_candidate_at("account-1", 1_800_000_600);
        store.stop_reader();
        store.complete_reader_candidate("account-1", &latest, candidate);
        assert!(Arc::ptr_eq(
            &before,
            &store.snapshot_account("account-1").unwrap()
        ));
        assert_eq!(
            store.issuer.lock().unwrap().counter,
            2,
            "shutdown cannot admit a late read"
        );
    }

    #[test]
    fn issue604_reader_waits_for_the_matching_acknowledged_db_commit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reader-commit-match.sqlite3");
        fixture(&path, 10);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("UPDATE collection_generation SET data_generation='2',collector_epoch='11111111111111111111111111111111',cycle_seq='2'").unwrap();
        let store = SnapshotStore::new(DbReader::open(&path).unwrap());
        assert!(store.notify_recorder_commit("account-1", issue604_commit_notice(1)));
        let intent = Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-1"]);
        store.complete_reader_candidate(
            "account-1",
            &intent,
            store.prepare_reader_candidate_at("account-1", 1_800_000_600),
        );
        assert!(
            store.snapshot_account("account-1").is_none(),
            "a DB commit ahead of its state file is not yet acknowledged"
        );
        assert_eq!(store.issuer.lock().unwrap().counter, 1);
        assert!(
            !store.status_account("account-1").degraded,
            "an unconfirmed candidate is not a DB failure"
        );
        let notice = issue604_commit_notice(2);
        assert!(store.notify_recorder_commit("account-1", notice.clone()));
        assert!(
            !store.notify_recorder_commit("account-1", notice.clone()),
            "the same accepted tuple cannot enqueue a second cycle"
        );
        let intent = Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-1"]);
        store.complete_reader_candidate(
            "account-1",
            &intent,
            store.prepare_reader_candidate_at("account-1", 1_800_000_600),
        );
        assert_eq!(store.snapshot_account("account-1").unwrap().generation, 2);
        assert_eq!(store.issuer.lock().unwrap().counter, 2);
        assert!(store.recorder_commit_is_published("account-1", &notice));
    }

    #[test]
    fn issue604_reader_source_failure_and_same_domain_recovery_preserve_the_pair() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source-availability.sqlite3");
        fixture(&path, 10);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("UPDATE collection_generation SET collector_epoch='11111111111111111111111111111111',cycle_seq='1'").unwrap();
        let store = SnapshotStore::new(DbReader::open(&path).unwrap());
        let notice = issue604_commit_notice(1);
        store.notify_recorder_commit("account-1", notice.clone());
        let intent = Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-1"]);
        store.complete_reader_candidate(
            "account-1",
            &intent,
            store.prepare_reader_candidate_at("account-1", 1_800_000_600),
        );
        let before = store.snapshot_account("account-1").unwrap();
        assert!(store.mark_recorder_source_unavailable("account-1"));
        let (retained, unavailable) = store.capture_account_frame("account-1");
        assert!(unavailable);
        assert!(Arc::ptr_eq(&before, &retained.unwrap()));
        assert_eq!(store.issuer.lock().unwrap().counter, 2);

        store.notify_recorder_commit("account-1", notice);
        let intent = Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-1"]);
        store.complete_reader_candidate(
            "account-1",
            &intent,
            store.prepare_reader_candidate_at("account-1", 1_800_000_600),
        );
        let (after, unavailable) = store.capture_account_frame("account-1");
        assert!(!unavailable);
        assert!(Arc::ptr_eq(&before, &after.unwrap()));
        assert_eq!(
            store.issuer.lock().unwrap().counter,
            2,
            "availability recovery alone is not a publication"
        );
    }

    #[test]
    fn issue604_history_cancellation_reports_its_exact_boundary() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history-admission.sqlite3");
        let reader = issue604_time_projection_fixture(&path);
        let store = SnapshotStore::new_with_accounts(
            vec![AccountReader::new("account-7", 7, true, None, None, reader)
                .with_recorder_commit(Some(issue604_commit_notice(1)))],
            "account-7",
        )
        .unwrap();
        store.diagnostics.set_data_root(root.path());
        let initial = Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-7"]);
        // This boundary protects an already published history window, not
        // the independently newer Current window.
        store.complete_reader_lane(
            "account-7",
            &initial,
            store.prepare_reader_candidate_at("account-7", 1_800_000_360),
            ReaderLane::Direct,
        );
        let current = store.snapshot_account("account-7").unwrap();
        assert_eq!(current.models_v3[0].total_tokens, 20);
        let counter = store.issuer.lock().unwrap().counter;

        // A successful history read can still be discarded at three distinct
        // boundaries. Diagnose that boundary without changing its authority.
        let history = store
            .prepare_reader_candidate_at("account-7", 1_800_000_300)
            .unwrap();
        assert!(history.0.history_available);
        store.complete_reader_lane(
            "account-7",
            &initial,
            Ok(history.clone()),
            ReaderLane::History,
        );
        assert!(Arc::ptr_eq(
            &current,
            &store.snapshot_account("account-7").unwrap()
        ));

        assert!(store.notify_recorder_commit("account-7", issue604_commit_notice(2)));
        store.complete_reader_lane(
            "account-7",
            &initial,
            Ok(history.clone()),
            ReaderLane::History,
        );
        let latest = Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-7"]);
        store.complete_reader_lane("account-7", &latest, Ok(history), ReaderLane::History);
        assert!(Arc::ptr_eq(
            &current,
            &store.snapshot_account("account-7").unwrap()
        ));
        assert_eq!(store.issuer.lock().unwrap().counter, counter);
        assert!(
            !store.capture_account_frame("account-7").1,
            "cancellation is not failure of the already published current frame"
        );

        Connection::open(&path)
            .unwrap()
            .execute_batch("UPDATE collection_generation SET data_generation='2',cycle_seq='2';")
            .unwrap();
        store.complete_reader_lane(
            "account-7",
            &latest,
            Ok(store
                .prepare_current_candidate_at("account-7", 1_800_000_360)
                .unwrap()
                .unwrap()),
            ReaderLane::Current,
        );
        assert!(
            !store
                .snapshot_account("account-7")
                .unwrap()
                .history_available
        );
        store.complete_reader_lane(
            "account-7",
            &latest,
            store.prepare_reader_candidate_at("account-7", 1_800_000_360),
            ReaderLane::History,
        );
        let recovered = store.snapshot_account("account-7").unwrap();
        assert!(recovered.history_available);
        assert_eq!(recovered.generation, 2);
        assert_eq!(recovered.models_v3[0].total_tokens, 20);
        assert_eq!(recovered.retained_history.samples.len(), 2);

        let directory = root.path().join("logs/rest");
        let mut rows = Vec::<Value>::new();
        if directory.exists() {
            for entry in fs::read_dir(directory).unwrap() {
                for line in fs::read_to_string(entry.unwrap().path()).unwrap().lines() {
                    rows.push(serde_json::from_str(line).unwrap());
                }
            }
        }
        assert_eq!(
            rows.iter()
                .map(|row| row["reason"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "publication_window_superseded",
                "intent_superseded",
                "commit_not_acknowledged",
                "current_history_available"
            ],
            "successful history reads must report the boundary that prevents publication"
        );
        for row in &rows {
            assert_eq!(row["route"], "reader_reconcile");
            assert_eq!(row["stage"], "history_admission");
            assert_eq!(
                row.as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["timestamp", "route", "stage", "reason", "http_status"])
            );
        }
        let logged = serde_json::to_string(&rows).unwrap();
        for private in [
            "account-7",
            "history-admission.sqlite3",
            "SOL",
            &current.pair,
            &current.data_hash,
        ] {
            assert!(
                !logged.contains(private),
                "diagnostics must contain only fixed categories"
            );
        }
    }

    #[test]
    fn issue604_unavailable_diagnostics_preserve_last_good() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("diagnostic-fixture.sqlite3");
        let reader = issue604_time_projection_fixture(&path);
        let store = SnapshotStore::new_with_accounts(
            vec![AccountReader::new(
                "account-7",
                7,
                true,
                None,
                None,
                reader.clone(),
            )],
            "account-7",
        )
        .unwrap();
        store.diagnostics.set_data_root(root.path());
        let account = store.account_store("account-7").unwrap();
        let initial = issue604_candidate_for_store_at(&account, &reader, 1_800_000_300);
        assert_eq!(initial.generation, 1);
        assert_eq!(initial.models_v3[0].total_tokens, 10);
        assert!(matches!(
            account.admit_candidate(Ok(initial), &store.diagnostics, "reader_reconcile", None),
            RefreshStatus::Updated { generation: 1 }
        ));
        let before = account.snapshot().unwrap();
        let pair = before.pair.clone();
        let counter = store.issuer.lock().unwrap().counter;
        Connection::open(&path)
            .unwrap()
            .execute_batch("UPDATE usage_model_history SET total_tokens='99',input_tokens='99' WHERE timestamp=1800000000;")
            .unwrap();
        let changed = issue604_candidate_for_store_at(&account, &reader, 1_800_000_300);
        assert_eq!(changed.generation, 1);
        assert_eq!(changed.models_v3[0].total_tokens, 99);
        assert_ne!(changed.data_hash, before.data_hash);
        assert!(matches!(
            account.admit_candidate(Ok(changed), &store.diagnostics, "reader_reconcile", None),
            RefreshStatus::RetainedLastGood { generation: 1 }
        ));
        let (retained, unavailable) = account.capture();
        assert!(unavailable);
        assert!(Arc::ptr_eq(&before, &retained.unwrap()));
        assert_eq!(account.snapshot().unwrap().pair, pair);
        assert_eq!(store.issuer.lock().unwrap().counter, counter);
        assert!(store.mark_recorder_source_unavailable_with_reason(
            "account-7",
            RecorderSourceUnavailableReason::PublicationStale
        ));
        assert!(!store.mark_recorder_source_unavailable_with_reason(
            "account-7",
            RecorderSourceUnavailableReason::ReadFailed
        ));
        assert!(Arc::ptr_eq(&before, &account.snapshot().unwrap()));
        assert_eq!(account.snapshot().unwrap().pair, pair);
        assert_eq!(store.issuer.lock().unwrap().counter, counter);

        let log_directory = root.path().join("logs/rest");
        let mut rows = Vec::new();
        if log_directory.exists() {
            for entry in fs::read_dir(log_directory).unwrap() {
                for line in fs::read_to_string(entry.unwrap().path()).unwrap().lines() {
                    rows.push(serde_json::from_str::<Value>(line).unwrap());
                }
            }
        }
        assert_eq!(
            rows.len(),
            2,
            "one publication guard and one new source intent"
        );
        assert_eq!(rows[0]["stage"], "publication_guard");
        assert_eq!(rows[0]["reason"], "same_generation_source_rewrite");
        assert_eq!(rows[1]["stage"], "recorder_source");
        assert_eq!(rows[1]["reason"], "publication_stale");
        for row in &rows {
            assert_eq!(
                row.as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["timestamp", "route", "stage", "reason", "http_status"])
            );
            assert!(row["timestamp"].as_i64().is_some());
            assert_eq!(row["route"], "reader_reconcile");
            assert_eq!(row["http_status"], Value::Null);
        }
        let logged = serde_json::to_string(&rows).unwrap();
        for private in [
            "account-7",
            "diagnostic-fixture.sqlite3",
            "SOL",
            &before.data_hash,
            &pair,
        ] {
            assert!(
                !logged.contains(private),
                "source values must not enter diagnostics"
            );
        }

        // This serialization failure is later than source/cache admission.
        // Preserve its result while identifying the missing recorded archive.
        let projection_root = tempfile::tempdir().unwrap();
        let projection_log = FailureLog::default();
        projection_log.set_data_root(projection_root.path());
        let mut without_archive = (*before).clone();
        without_archive.recordings = None;
        assert_eq!(
            serialize_route_with_diagnostics(
                &without_archive,
                Route::CurrentV5,
                None,
                None,
                false,
                &projection_log,
                "/v5/current"
            ),
            Err(RouteError::SnapshotUnavailable)
        );
        let directory = projection_root.path().join("logs/rest");
        let mut projection_rows = Vec::new();
        if directory.exists() {
            for entry in fs::read_dir(directory).unwrap() {
                for line in fs::read_to_string(entry.unwrap().path()).unwrap().lines() {
                    projection_rows.push(serde_json::from_str::<Value>(line).unwrap());
                }
            }
        }
        assert_eq!(
            projection_rows.len(),
            1,
            "one archive serialization diagnostic"
        );
        let row = &projection_rows[0];
        assert_eq!(row["route"], "/v5/current");
        assert_eq!(row["stage"], "recorded_projection");
        assert_eq!(row["reason"], "recorded_archive_unavailable");
        assert_eq!(row["http_status"], Value::Null);
        assert!(row["timestamp"].as_i64().is_some());
        assert_eq!(
            row.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["timestamp", "route", "stage", "reason", "http_status"])
        );
        let logged = row.to_string();
        for private in ["account-7", "SOL", &before.data_hash, &pair] {
            assert!(!logged.contains(private));
        }
    }

    fn issue604_commit_notice(generation: u64) -> RecorderCommitNotice {
        RecorderCommitNotice {
            pid: 1,
            process_starttime: 1,
            owner_nonce: "22222222222222222222222222222222".to_owned(),
            partition_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_owned(),
            commit: CollectionCommit {
                generation,
                collector_epoch: Some(0x11111111111111111111111111111111),
                cycle_seq: generation,
            },
            degraded: false,
        }
    }

    #[test]
    fn issue604_standalone_startup_reads_all_accounts_without_an_http_request() {
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("first.sqlite3");
        let second_path = directory.path().join("second.sqlite3");
        fixture(&first_path, 10);
        fixture(&second_path, 20);
        let server = RestServer::start_with_accounts(
            vec![
                AccountReader::new(
                    "account-7",
                    7,
                    true,
                    None,
                    None,
                    DbReader::open(&first_path).unwrap(),
                ),
                AccountReader::new(
                    "account-13",
                    13,
                    false,
                    None,
                    None,
                    DbReader::open(&second_path).unwrap(),
                ),
            ],
            "account-7",
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let store = server.store();
        // Bound fixture synchronization, not production startup latency. These
        // observations touch only the captured cache and issue no HTTP or DB
        // request that could initialize an account on the reader's behalf.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && (store.snapshot_account("account-7").is_none()
                || store.snapshot_account("account-13").is_none())
        {
            thread::sleep(Duration::from_millis(5));
        }
        let first = store
            .snapshot_account("account-7")
            .expect("the startup reader must publish the current account without HTTP");
        let second = store
            .snapshot_account("account-13")
            .expect("the startup reader must also publish the historical account without HTTP");
        assert_eq!(&first.pair[3..35], &second.pair[3..35]);
        // The worker uses account-selector byte order, independently of the
        // catalog's current-first presentation order: account-13 precedes -7.
        assert_eq!(u128::from_str_radix(&second.pair[35..], 16).unwrap(), 2);
        assert_eq!(u128::from_str_radix(&first.pair[35..], 16).unwrap(), 3);
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

    fn ready_fixture_server_at(reader: DbReader, at: i64) -> RestServer {
        let mut store = SnapshotStore::new_with_accounts(
            vec![AccountReader::new("account-1", 1, true, None, None, reader)],
            "account-1",
        )
        .unwrap();
        store.publication_clock = Arc::new(move || Some(at));
        let server = RestServer::start_with_store(store, "127.0.0.1:0".parse().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while server.store.snapshot_account("account-1").is_none() {
            assert!(Instant::now() < deadline, "fixture account did not publish");
            assert!(!server.store.publication_failed());
            thread::sleep(Duration::from_millis(1));
        }
        server
    }

    fn issue604_time_projection_fixture(path: &PathBuf) -> DbReader {
        fixture(path, 10);
        let db = Connection::open(path).unwrap();
        db.execute_batch(r#"
            DELETE FROM usage_history; DELETE FROM durable_state;
            UPDATE collection_generation SET reset_at=1800604800,window_seconds=604800,
                collector_epoch='11111111111111111111111111111111',cycle_seq='1';
            CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,sequence INTEGER UNIQUE,
                observed_at INTEGER,reset_at INTEGER,window_seconds INTEGER,
                remaining_percent REAL,applied_generation TEXT);
            INSERT INTO quota_receipts VALUES('original',1,1800000000,1800604800,604800,50,'1');
            CREATE TABLE usage_model_history(timestamp INTEGER,reset_at INTEGER,model TEXT,
                total_tokens TEXT,input_tokens TEXT,cached_input_tokens TEXT,output_tokens TEXT,
                cache_write_input_tokens TEXT,model_set_complete INTEGER,price_version TEXT);
            INSERT INTO usage_history VALUES
                (1800000000,1800604800,50,0.00005,0,0,10,0,0),
                (1800000360,1800604800,NULL,0.00010,0,0,20,0,0);
            INSERT INTO usage_model_history VALUES
                (1800000000,1800604800,'SOL','10','10','0','0','0',1,'stored-price'),
                (1800000360,1800604800,'SOL','20','20','0','0','0',1,'stored-price');
            INSERT INTO durable_state VALUES
                (2,1800000000,'observation-1','{"kind":"codex-info-usage-observation-v1","timestamp":1800000000,"reset_at":1800604800,"remaining_percent":50,"model_source":"confirmed"}'),
                (3,1800000360,'observation-2','{"kind":"codex-info-usage-observation-v1","timestamp":1800000360,"reset_at":1800604800,"remaining_percent":null,"model_source":"confirmed"}');
        "#).unwrap();
        DbReader::open(path).unwrap()
    }

    #[test]
    fn issue604_history_keeps_its_verified_revision_across_current_commits() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("independent-history.sqlite3");
        let reader = issue604_time_projection_fixture(&path);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "ALTER TABLE usage_model_history ADD COLUMN total_dollars REAL;
            UPDATE usage_model_history SET total_dollars=CASE total_tokens
                WHEN '10' THEN 0.00005 ELSE 0.00010 END;",
        )
        .unwrap();
        let store = Arc::new(
            SnapshotStore::new_with_accounts(
                vec![AccountReader::new("account-7", 7, true, None, None, reader)
                    .with_recorder_commit(Some(issue604_commit_notice(1)))],
                "account-7",
            )
            .unwrap(),
        );
        let intent =
            || Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-7"]);
        store.complete_reader_lane(
            "account-7",
            &intent(),
            store.prepare_reader_candidate_at("account-7", 1_800_000_300),
            ReaderLane::History,
        );
        let first = store.snapshot_account("account-7").unwrap();
        assert_eq!(first.retained_history.samples.len(), 1);
        assert_eq!(first.models_v3[0].total_tokens, 10);
        let get = |route: &str, conditional: Option<&str>| -> String {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let captured = Arc::clone(&store);
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                handle_connection(&mut stream, &captured);
            });
            let condition = conditional
                .map(|pair| format!("If-None-Match: \"{pair}\"\r\n"))
                .unwrap_or_default();
            let response = request(
                address,
                &format!("GET {route} HTTP/1.1\r\nHost:x\r\n{condition}\r\n"),
            );
            worker.join().unwrap();
            response
        };
        let old_history = get("/v3/history?period=1800604800", None);
        assert!(old_history.starts_with("HTTP/1.1 200"));
        let old_body: Value = serde_json::from_str(body(&old_history)).unwrap();

        // Source commits and fixed expected values are independent of the
        // publisher. History has not read commit 2 at this point.
        db.execute_batch(
            "BEGIN;
            INSERT INTO quota_receipts VALUES('next',2,1800000360,1800604800,604800,49,'2');
            UPDATE collection_generation SET data_generation='2',cycle_seq='2';
            COMMIT;",
        )
        .unwrap();
        assert!(store.notify_recorder_commit("account-7", issue604_commit_notice(2)));
        store.complete_reader_lane(
            "account-7",
            &intent(),
            Ok(store
                .prepare_current_candidate_at("account-7", 1_800_000_360)
                .unwrap()
                .unwrap()),
            ReaderLane::Current,
        );
        let current = get("/v4/current", None);
        let current_body: Value = serde_json::from_str(body(&current)).unwrap();
        assert_eq!(current_body["models"][0]["total_tokens"], 20);
        assert_eq!(current_body["quota"]["remaining_percent"], 49.0);
        assert_ne!(published_pair(&current), first.pair);
        assert_eq!(
            published_pair(&get("/v3/threads", None)),
            published_pair(&current)
        );

        let retained = get("/v3/history?period=1800604800", None);
        assert!(
            retained.starts_with("HTTP/1.1 200"),
            "a newer Current commit must not withdraw the verified history revision: {retained}"
        );
        assert_eq!(published_pair(&retained), first.pair);
        assert_eq!(
            serde_json::from_str::<Value>(body(&retained)).unwrap(),
            old_body
        );
        assert!(retained.contains("Codex-Info-History-State: refreshing\r\n"));
        assert!(retained.contains("Codex-Info-History-Verified-At: 1800000300\r\n"));
        for route in [
            "/v3/history/periods",
            "/v4/history/periods",
            "/v5/history/periods",
            "/v3/history/periods/selectable",
            "/v3/details",
        ] {
            let response = get(route, None);
            assert!(response.starts_with("HTTP/1.1 200"), "{route}: {response}");
            assert_eq!(published_pair(&response), first.pair);
        }
        let compatibility: Value = serde_json::from_str(body(&get("/v3/details", None))).unwrap();
        assert_eq!(compatibility["models"][0]["total_tokens"], 10);
        assert_eq!(compatibility["quota"]["remaining_percent"], 50.0);
        let counter = store.issuer.lock().unwrap().counter;
        let unchanged = get("/v3/history?period=1800604800", Some(&first.pair));
        assert!(unchanged.starts_with("HTTP/1.1 304"));
        assert!(unchanged.contains("Codex-Info-History-State: refreshing\r\n"));
        assert_eq!(published_pair(&unchanged), first.pair);
        assert_eq!(
            store.issuer.lock().unwrap().counter,
            counter,
            "HTTP freshness metadata does not create a data publication"
        );

        // The exact acknowledged History commit atomically replaces the
        // old revision. No point is extrapolated during the wait.
        store.complete_reader_lane(
            "account-7",
            &intent(),
            store.prepare_reader_candidate_at("account-7", 1_800_000_360),
            ReaderLane::History,
        );
        let recovered = get("/v3/history?period=1800604800", None);
        assert!(recovered.starts_with("HTTP/1.1 200"));
        assert!(recovered.contains("Codex-Info-History-State: verified\r\n"));
        assert!(recovered.contains("Codex-Info-History-Verified-At: 1800000360\r\n"));
        let recovered_body: Value = serde_json::from_str(body(&recovered)).unwrap();
        let samples = recovered_body["history_samples"].as_array().unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0]["models"][0]["total_tokens"], 10);
        assert_eq!(samples[1]["models"][0]["total_tokens"], 20);
        assert_eq!(samples[0]["models"][0]["total_dollars"], 0.00005);
        let recovered_pair = published_pair(&recovered).to_owned();
        assert_ne!(recovered_pair, first.pair);

        // This legacy DB fixture permits window-based alias normalization.
        // Advance a full week with its receipt after the old deadline: the
        // old model is outside the new window, not a same-week legacy alias.
        // The old History must remain intact, without a new-period sample.
        db.execute_batch(
            "BEGIN;
            INSERT INTO quota_receipts VALUES('new-period',3,1800604860,1801209600,604800,77,'3');
            UPDATE collection_generation SET data_generation='3',cycle_seq='3',reset_at=1801209600;
            COMMIT;",
        )
        .unwrap();
        assert!(store.notify_recorder_commit("account-7", issue604_commit_notice(3)));
        store.complete_reader_lane(
            "account-7",
            &intent(),
            Ok(store
                .prepare_current_candidate_at("account-7", 1_800_604_860)
                .unwrap()
                .unwrap()),
            ReaderLane::Current,
        );
        let new_current: Value = serde_json::from_str(body(&get("/v4/current", None))).unwrap();
        assert_eq!(new_current["quota"]["reset_at"], 1_801_209_600_i64);
        assert_eq!(new_current["quota"]["remaining_percent"], 77.0);
        assert_eq!(new_current["models"], json!([]));
        let periods = get("/v3/history/periods", None);
        assert_eq!(published_pair(&periods), recovered_pair);
        let periods: Value = serde_json::from_str(body(&periods)).unwrap();
        assert_eq!(periods["history_periods"][0]["reset_at"], 1_800_604_800_i64);
        let retained = get("/v3/history?period=1800604800", None);
        assert_eq!(
            serde_json::from_str::<Value>(body(&retained)).unwrap(),
            recovered_body
        );
        assert!(retained.contains("Codex-Info-History-State: refreshing\r\n"));
        assert!(get("/v3/history?period=1801209600", None).starts_with("HTTP/1.1 400"));

        let mut new_owner = issue604_commit_notice(3);
        new_owner.owner_nonce = "33333333333333333333333333333333".into();
        assert!(store.notify_recorder_commit("account-7", new_owner));
        store.complete_reader_lane(
            "account-7",
            &intent(),
            Ok(store
                .prepare_current_candidate_at("account-7", 1_800_604_860)
                .unwrap()
                .unwrap()),
            ReaderLane::Current,
        );
        assert!(
            get("/v3/history/periods", None).starts_with("HTTP/1.1 503"),
            "an owner boundary cannot reuse a previous owner's History"
        );
    }

    fn issue604_candidate_at(reader: &DbReader, at: i64) -> PublishedSnapshot {
        PublishedSnapshot::candidate_from_db_for_account(
            reader.read_committed_snapshot_at(at).unwrap().snapshot,
            7,
        )
    }

    fn issue604_candidate_for_store_at(
        store: &AccountStore,
        reader: &DbReader,
        at: i64,
    ) -> PublishedSnapshot {
        let previous_key = store.snapshot().and_then(|frame| {
            frame
                .reprojection_witness
                .as_ref()
                .and_then(|proof| proof.current_observation())
        });
        PublishedSnapshot::candidate_from_db_for_account(
            reader
                .read_committed_snapshot_for_reprojection_at(at, previous_key)
                .unwrap()
                .snapshot,
            7,
        )
    }

    #[test]
    fn issue604_delayed_history_preserves_current_and_history_windows() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("delayed-history.sqlite3");
        let reader = issue604_time_projection_fixture(&path);
        Connection::open(&path).unwrap().execute_batch(r#"
            INSERT INTO usage_history VALUES
                (1800000420,1800604800,NULL,0.00015,0,0,30,0,0);
            INSERT INTO usage_model_history VALUES
                (1800000420,1800604800,'SOL','30','30','0','0','0',1,'stored-price');
            INSERT INTO durable_state VALUES
                (4,1800000420,'observation-3','{"kind":"codex-info-usage-observation-v1","timestamp":1800000420,"reset_at":1800604800,"remaining_percent":null,"model_source":"confirmed"}');
        "#).unwrap();
        let store = SnapshotStore::new_with_accounts(
            vec![AccountReader::new("account-7", 7, true, None, None, reader)],
            "account-7",
        )
        .unwrap();
        let intent = Arc::clone(&store.reader_control.pending.lock().unwrap().latest["account-7"]);
        store.complete_reader_lane(
            "account-7",
            &intent,
            Ok(store
                .prepare_current_candidate_at("account-7", 1_800_000_300)
                .unwrap()
                .unwrap()),
            ReaderLane::Current,
        );
        let delayed = store
            .prepare_reader_candidate_at("account-7", 1_800_000_300)
            .unwrap();
        assert!(delayed.0.history_available);
        assert_eq!(delayed.0.retained_history.samples.len(), 1);
        assert_eq!(
            delayed.0.retained_history.samples[0]
                .models
                .as_ref()
                .unwrap()[0]
                .total_tokens,
            10_u64
        );

        store.complete_reader_lane(
            "account-7",
            &intent,
            Ok(store
                .prepare_current_candidate_at("account-7", 1_800_000_360)
                .unwrap()
                .unwrap()),
            ReaderLane::Current,
        );
        let current = store.snapshot_account("account-7").unwrap();
        assert_eq!(current.models_v3[0].total_tokens, 20);
        assert_eq!(current.details.observed_at, Some(1_800_000_360));
        store.complete_reader_lane(
            "account-7",
            &intent,
            Ok(delayed.clone()),
            ReaderLane::History,
        );
        let combined = store.snapshot_account("account-7").unwrap();
        assert!(
            combined.history_available,
            "qualified history must publish without rolling back the newer Current window"
        );
        assert_eq!(combined.generation, 1);
        assert_eq!(combined.current, current.current);
        assert_eq!(combined.models_v3, current.models_v3);
        assert_eq!(combined.details.observed_at, Some(1_800_000_360));
        assert_eq!(
            combined
                .reprojection_witness
                .as_ref()
                .unwrap()
                .publication_window()
                .unwrap()
                .at,
            1_800_000_360
        );
        assert_eq!(
            combined.retained_history.samples.len(),
            1,
            "the older history read must not invent the later eligible observation"
        );
        assert_eq!(
            combined.retained_history.samples[0].timestamp,
            1_800_000_000
        );
        assert_eq!(
            combined.retained_history.samples[0]
                .models
                .as_ref()
                .unwrap()[0]
                .total_tokens,
            10_u64
        );
        assert_ne!(combined.pair, current.pair);
        combined.details.validate_history_domain().unwrap();

        store.complete_reader_lane(
            "account-7",
            &intent,
            Ok(store
                .prepare_current_candidate_at("account-7", 1_800_000_420)
                .unwrap()
                .unwrap()),
            ReaderLane::Current,
        );
        let advanced = store.snapshot_account("account-7").unwrap();
        assert!(
            advanced.history_available,
            "a same-commit Current refresh cannot withdraw known History"
        );
        assert_eq!(advanced.models_v3[0].total_tokens, 30);
        assert_eq!(advanced.details.observed_at, Some(1_800_000_420));
        assert_eq!(advanced.retained_history.samples.len(), 1);
        advanced.details.validate_history_domain().unwrap();
        let current_wire: Value = serde_json::from_slice(
            &serialize_route(&advanced, Route::CurrentV4, None, None, false).unwrap(),
        )
        .unwrap();
        assert_eq!(current_wire["models"][0]["total_tokens"], 30);
        let period = &advanced
            .retained_history
            .periods
            .iter()
            .find(|period| period.current)
            .unwrap()
            .id;
        let history_wire: Value = serde_json::from_slice(
            &serialize_route(&advanced, Route::HistoryV3, Some(period), None, false).unwrap(),
        )
        .unwrap();
        assert_eq!(history_wire["history_samples"].as_array().unwrap().len(), 1);
        assert_eq!(
            history_wire["history_samples"][0]["models"][0]["total_tokens"],
            10
        );

        store.complete_reader_lane(
            "account-7",
            &intent,
            store.prepare_reader_candidate_at("account-7", 1_800_000_360),
            ReaderLane::History,
        );
        let full = store.snapshot_account("account-7").unwrap();
        assert_eq!(full.retained_history.samples.len(), 2);
        assert_eq!(full.retained_history.samples[1].timestamp, 1_800_000_360);
        assert_eq!(
            full.retained_history.samples[1].models.as_ref().unwrap()[0].total_tokens,
            20_u64
        );
        assert_eq!(full.models_v3[0].total_tokens, 30);
        assert_eq!(full.details.observed_at, Some(1_800_000_420));
        full.details.validate_history_domain().unwrap();
        store.complete_reader_lane("account-7", &intent, Ok(delayed), ReaderLane::History);
        assert!(
            Arc::ptr_eq(&full, &store.snapshot_account("account-7").unwrap()),
            "the already published history extent cannot regress"
        );

        Connection::open(&path).unwrap().execute_batch(
            "UPDATE usage_model_history SET total_tokens='99',input_tokens='99' WHERE timestamp=1800000000;"
        ).unwrap();
        store.complete_reader_lane(
            "account-7",
            &intent,
            store.prepare_reader_candidate_at("account-7", 1_800_000_360),
            ReaderLane::History,
        );
        let (retained, unavailable) = store.capture_account_frame("account-7");
        assert!(
            unavailable,
            "same-commit history source rewrites must still be rejected"
        );
        assert!(Arc::ptr_eq(&full, &retained.unwrap()));
    }

    #[test]
    fn issue604_current_publication_advances_while_history_is_blocked() {
        use std::sync::atomic::{AtomicI64, AtomicUsize};
        use std::sync::mpsc;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("independent-current.sqlite3");
        let reader = issue604_time_projection_fixture(&path);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(r#"
            CREATE TABLE active_thread_snapshot(singleton INTEGER PRIMARY KEY,
                observed_at INTEGER NOT NULL, threads_json TEXT NOT NULL,
                acquisition_degraded INTEGER NOT NULL);
            INSERT INTO active_thread_snapshot VALUES(1,1800000000,
                '[{"id":"independent-thread","updated_at":1800000000,"title":"saved thread","activity_status":"running","parent_thread_id":null,"model":"gpt-5","model_label":"SOL","total_tokens":12,"context_usage_tokens":8,"context_window_tokens":128,"created_at":1799999940,"last_user_message_at":1800000000,"is_subagent":false,"depth":0}]',0);
        "#).unwrap();
        let mut store = SnapshotStore::new_with_accounts(
            vec![AccountReader::new("account-7", 7, true, None, None, reader)],
            "account-7",
        )
        .unwrap();
        let clock = Arc::new(AtomicI64::new(1_800_000_300));
        let time = Arc::clone(&clock);
        store.publication_clock = Arc::new(move || Some(time.load(Ordering::SeqCst)));
        let calls = Arc::new(AtomicUsize::new(0));
        let history_calls = Arc::clone(&calls);
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        store.history_candidate_ready = Some(Arc::new(move || {
            if history_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                entered_tx.send(()).unwrap();
                let _ = release_rx.lock().unwrap().recv();
            }
        }));
        let mut server =
            RestServer::start_with_store(store, "127.0.0.1:0".parse().unwrap()).unwrap();
        let store = server.store();
        let wait_frame = |generation: u64, history: bool| {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(frame) = store.snapshot_account("account-7") {
                    if frame.generation == generation && (!history || frame.history_available) {
                        return frame;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "current commit {generation} must publish independently of blocked history"
                );
                thread::sleep(Duration::from_millis(1));
            }
        };
        let http = |route| {
            request(
                server.local_addr(),
                &format!("GET {route} HTTP/1.1\r\nHost:x\r\n\r\n"),
            )
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let first = wait_frame(1, false);
            assert!(
                !first.history_available,
                "unread history is unavailable, never empty"
            );
            let current = http("/v4/current");
            assert!(current.starts_with("HTTP/1.1 200"), "{current}");
            let value: Value = serde_json::from_str(body(&current)).unwrap();
            assert_eq!(value["state"], "ready");
            assert_eq!(value["quota"]["remaining_percent"], 50.0);
            assert_eq!(value["models"][0]["total_tokens"], 10);
            let threads = http("/v3/threads");
            assert!(threads.starts_with("HTTP/1.1 200"), "{threads}");
            assert_eq!(published_pair(&current), published_pair(&threads));
            let threads: Value = serde_json::from_str(body(&threads)).unwrap();
            assert_eq!(threads["threads"][0]["id"], "independent-thread");
            assert!(http("/v3/history/periods").starts_with("HTTP/1.1 503"));

            db.execute_batch(
                "BEGIN;
                INSERT INTO quota_receipts VALUES('next',2,1800000360,1800604800,604800,49,'2');
                UPDATE collection_generation SET data_generation='2',cycle_seq='2';
                COMMIT;",
            )
            .unwrap();
            clock.store(1_800_000_360, Ordering::SeqCst);
            store.reader_control.pending.lock().unwrap().freshness = true;
            store.reader_control.changed.notify_all();
            let second = wait_frame(2, false);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "only one history read may be in flight"
            );
            assert_eq!(second.models_v3[0].total_tokens, 20);
            assert_eq!(
                second.details.quota.as_ref().unwrap().remaining_percent,
                49.0
            );
            assert_ne!(first.pair, second.pair);
            assert!(!second.history_available);
            release_tx.send(()).unwrap();
            let full = wait_frame(2, true);
            assert_eq!(full.models_v3[0].total_tokens, 20);
            assert_eq!(full.details.quota.as_ref().unwrap().remaining_percent, 49.0);
            assert!(
                !store.account_store("account-7").unwrap().status().degraded,
                "an obsolete history result cannot poison the newer current commit"
            );
            let current = http("/v4/current");
            let history = http("/v3/history/periods");
            assert!(history.starts_with("HTTP/1.1 200"), "{history}");
            assert_eq!(published_pair(&current), published_pair(&history));
        }));
        // The intended RED also releases and joins the blocked reader.
        let _ = release_tx.try_send(());
        server.shutdown();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn issue604_period_history_does_not_wait_for_recorded_archive() {
        use std::sync::atomic::{AtomicI64, AtomicUsize};
        use std::sync::mpsc;

        let root = tempfile::tempdir().unwrap();
        let first_path = root.path().join("current.sqlite3");
        let second_path = root.path().join("archived.sqlite3");
        let first_reader = issue604_time_projection_fixture(&first_path);
        let second_reader = issue604_time_projection_fixture(&second_path);
        for path in [&first_path, &second_path] {
            Connection::open(path)
                .unwrap()
                .execute_batch(
                    "ALTER TABLE usage_model_history ADD COLUMN total_dollars REAL;
                 UPDATE usage_model_history SET total_dollars=CASE total_tokens
                     WHEN '10' THEN 0.00005 ELSE 0.00010 END;",
                )
                .unwrap();
        }
        let mut store = SnapshotStore::new_with_accounts(
            vec![
                AccountReader::new("account-7", 7, true, None, None, first_reader),
                AccountReader::new("account-8", 8, false, None, None, second_reader),
            ],
            "account-7",
        )
        .unwrap();
        let clock = Arc::new(AtomicI64::new(1_800_000_300));
        let time = Arc::clone(&clock);
        store.publication_clock = Arc::new(move || Some(time.load(Ordering::SeqCst)));
        let calls = Arc::new(AtomicUsize::new(0));
        let observed_calls = Arc::clone(&calls);
        let (entered_tx, entered_rx) = mpsc::sync_channel(2);
        let (release_tx, release_rx) = mpsc::sync_channel(2);
        let release_rx = Mutex::new(release_rx);
        store.recording_candidate_ready = Some(Arc::new(move || {
            let call = observed_calls.fetch_add(1, Ordering::SeqCst);
            if call < 2 {
                entered_tx.send(call).unwrap();
                let _ = release_rx.lock().unwrap().recv();
            }
        }));
        let mut server =
            RestServer::start_with_store(store, "127.0.0.1:0".parse().unwrap()).unwrap();
        let store = server.store();
        let wait_frame = |account: &str, generation: u64, recordings: bool| {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(frame) = store.snapshot_account(account) {
                    if frame.generation == generation
                        && frame.history_available
                        && (!recordings || frame.recordings.is_some())
                    {
                        return frame;
                    }
                }
                assert!(Instant::now() < deadline,
                    "period history for {account} commit {generation} must publish while recorded archive is blocked");
                thread::sleep(Duration::from_millis(1));
            }
        };
        let http = |route: &str| {
            request(
                server.local_addr(),
                &format!("GET {route} HTTP/1.1\r\nHost:x\r\n\r\n"),
            )
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_eq!(entered_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 0);
            for account in ["account-7", "account-8"] {
                let frame = wait_frame(account, 1, false);
                assert!(
                    frame.recordings.is_none(),
                    "unread archive must remain unavailable"
                );
                assert_eq!(frame.models_v3[0].total_tokens, 10);
                assert_eq!(
                    frame.models_v3[0]
                        .estimated_cost
                        .as_ref()
                        .unwrap()
                        .total_dollars,
                    0.00005
                );
                let current = http(&format!("/v4/current?account={account}"));
                let periods = http(&format!("/v3/history/periods?account={account}"));
                let history = http(&format!("/v3/history?account={account}&period=1800604800"));
                for response in [&current, &periods, &history] {
                    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
                    assert_eq!(published_pair(response), frame.pair);
                }
                let history: Value = serde_json::from_str(body(&history)).unwrap();
                assert_eq!(
                    history["history_samples"][0]["models"][0]["total_tokens"],
                    10
                );
                assert!(http(&format!(
                    "/v4/history/observations?account={account}&start=1800000000&end=1800000301"
                ))
                .starts_with("HTTP/1.1 503"));
            }
            Connection::open(&first_path)
                .unwrap()
                .execute_batch(
                    "BEGIN;
                INSERT INTO quota_receipts VALUES('next',2,1800000360,1800604800,604800,49,'2');
                UPDATE collection_generation SET data_generation='2',cycle_seq='2';
                COMMIT;",
                )
                .unwrap();
            clock.store(1_800_000_360, Ordering::SeqCst);
            store.reader_control.pending.lock().unwrap().freshness = true;
            store.reader_control.changed.notify_all();
            let second = wait_frame("account-7", 2, false);
            assert_eq!(second.models_v3[0].total_tokens, 20);
            assert_eq!(
                second.details.quota.as_ref().unwrap().remaining_percent,
                49.0
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "one auxiliary read in flight"
            );
            release_tx.send(()).unwrap();
            assert_eq!(entered_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
            assert!(
                store
                    .snapshot_account("account-7")
                    .unwrap()
                    .recordings
                    .is_none(),
                "an archive read from commit 1 cannot attach to commit 2"
            );
            release_tx.send(()).unwrap();
            let complete = wait_frame("account-7", 2, true);
            assert_eq!(complete.models_v3[0].total_tokens, 20);
            assert_eq!(
                complete
                    .recordings
                    .as_ref()
                    .unwrap()
                    .quota_observations
                    .last()
                    .unwrap()
                    .remaining_percent,
                49.0
            );
            let current = http("/v4/current?account=account-7");
            let history = http("/v3/history/periods?account=account-7");
            let recordings =
                http("/v4/history/observations?account=account-7&start=1800000000&end=1800000361");
            for response in [&current, &history, &recordings] {
                assert!(response.starts_with("HTTP/1.1 200"), "{response}");
                assert_eq!(published_pair(response), complete.pair);
            }
        }));
        let _ = release_tx.try_send(());
        let _ = release_tx.try_send(());
        server.shutdown();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn issue604_reader_worker_panic_preserves_the_last_frame_and_closes_the_listener() {
        let root = tempfile::tempdir().unwrap();
        let reader = issue604_time_projection_fixture(&root.path().join("reader-panic.sqlite3"));
        let mut store = SnapshotStore::new_with_accounts(
            vec![AccountReader::new("account-7", 7, true, None, None, reader)],
            "account-7",
        )
        .unwrap();
        let fail = Arc::new(AtomicBool::new(false));
        let worker_fail = Arc::clone(&fail);
        store.publication_clock = Arc::new(move || {
            assert!(
                !worker_fail.load(Ordering::Acquire),
                "injected reader worker panic"
            );
            Some(1_800_000_300)
        });
        let mut server =
            RestServer::start_with_store(store, "127.0.0.1:0".parse().unwrap()).unwrap();
        let store = server.store();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while store.snapshot_account("account-7").is_none() {
                assert!(Instant::now() < deadline, "initial frame did not publish");
                thread::sleep(Duration::from_millis(1));
            }
            let old = store.snapshot_account("account-7").unwrap();
            let global_pair = store.capture_accounts().1;
            let counter = store.issuer.lock().unwrap().counter;
            fail.store(true, Ordering::Release);
            store.reader_control.pending.lock().unwrap().freshness = true;
            store.reader_control.changed.notify_one();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !server.reader_worker.as_ref().unwrap().is_finished() {
                assert!(Instant::now() < deadline, "reader did not finish");
                thread::sleep(Duration::from_millis(1));
            }
            assert!(
                store.publication_failed(),
                "reader exit must be visible to the process supervisor"
            );
            assert!(Arc::ptr_eq(
                &old,
                &store.snapshot_account("account-7").unwrap()
            ));
            assert_eq!(store.capture_accounts().1, global_pair);
            assert_eq!(store.issuer.lock().unwrap().counter, counter);
            let pending = store.reader_control.pending.lock().unwrap();
            assert!(pending.stopped);
            assert!(pending.accounts.is_empty());
            drop(pending);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !server.worker.as_ref().unwrap().is_finished() {
                assert!(
                    Instant::now() < deadline,
                    "failed publisher left the listener running"
                );
                thread::sleep(Duration::from_millis(1));
            }
            assert!(
                TcpStream::connect_timeout(&server.local_addr(), Duration::from_millis(100))
                    .is_err()
            );
        }));
        server.shutdown();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn issue604_worker_fixes_one_time_per_cycle_and_reprojects_without_a_new_commit() {
        use std::sync::atomic::{AtomicI64, AtomicUsize};
        let root = tempfile::tempdir().unwrap();
        let readers = [7, 13].map(|epoch| {
            let reader = issue604_time_projection_fixture(
                &root.path().join(format!("time-{epoch}.sqlite3")),
            );
            AccountReader::new(
                format!("account-{epoch}"),
                epoch,
                epoch == 7,
                None,
                None,
                reader,
            )
        });
        let mut store = SnapshotStore::new_with_accounts(readers.into(), "account-7").unwrap();
        let clock = Arc::new(AtomicI64::new(1_800_000_300));
        let clock_calls = Arc::new(AtomicUsize::new(0));
        let time = Arc::clone(&clock);
        let calls = Arc::clone(&clock_calls);
        store.publication_clock = Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Some(time.load(Ordering::SeqCst))
        });
        let store = Arc::new(store);
        let worker = start_reader_worker(Arc::clone(&store)).unwrap();
        let wait = |at| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let frames = ["account-7", "account-13"].map(|id| store.snapshot_account(id));
                if frames.iter().all(|frame| {
                    frame.as_ref().is_some_and(|frame| {
                        frame.history_available
                            && frame
                                .reprojection_witness
                                .as_ref()
                                .and_then(|proof| proof.publication_window())
                                .is_some_and(|window| window.at == at)
                    })
                }) {
                    return frames.map(Option::unwrap);
                }
                assert!(
                    Instant::now() < deadline,
                    "both accounts must consume the captured cycle time"
                );
                thread::sleep(Duration::from_millis(5));
            }
        };
        // Always join on a failed assertion as well as normal test exit.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let first = wait(1_800_000_300);
            assert_eq!(clock_calls.load(Ordering::SeqCst), 1);
            for frame in &first {
                assert_eq!(frame.models_v3[0].total_tokens, 10);
                assert_eq!(frame.retained_history.samples.len(), 1);
            }
            clock.store(1_800_000_360, Ordering::SeqCst);
            {
                let mut pending = store.reader_control.pending.lock().unwrap();
                pending.freshness = true;
                store.reader_control.changed.notify_one();
            }
            let second = wait(1_800_000_360);
            assert_eq!(clock_calls.load(Ordering::SeqCst), 2);
            for (old, new) in first.iter().zip(&second) {
                assert_eq!(new.generation, 1);
                assert_eq!(new.models_v3[0].total_tokens, 20);
                assert_eq!(new.retained_history.samples.len(), 2);
                assert_ne!(new.pair, old.pair);
            }
        }));
        store.stop_reader();
        worker.join().unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn issue604_time_reprojection_cannot_add_a_previously_eligible_current_observation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("same-commit-current-addition.sqlite3");
        let reader = issue604_time_projection_fixture(&path);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "DELETE FROM usage_model_history; DELETE FROM durable_state;
             ALTER TABLE usage_history RENAME COLUMN sol_dollars TO broken_dollars;",
        )
        .unwrap();
        let store = AccountStore::new(
            reader.clone(),
            7,
            Arc::new(Mutex::new(PairIssuer::from_epoch([0x34; 16]).unwrap())),
        );
        let log = FailureLog::default();
        let initial = issue604_candidate_for_store_at(&store, &reader, 1_800_000_300);
        assert!(!initial.history_available);
        assert!(initial.models_v3.is_empty());
        assert!(matches!(
            store.admit_candidate(Ok(initial), &log, "test", None),
            RefreshStatus::Updated { generation: 1 }
        ));
        let old = store.snapshot().unwrap();
        // This timestamp was already eligible at the original T. Adding its
        // stored source without a commit is not a time-only qualification.
        db.execute_batch(r#"
            INSERT INTO usage_model_history VALUES
                (1800000060,1800604800,'SOL','10','10','0','0','0',1,'stored-price');
            INSERT INTO durable_state VALUES
                (2,1800000060,'added-current','{"kind":"codex-info-usage-observation-v1","timestamp":1800000060,"reset_at":1800604800,"remaining_percent":null,"model_source":"confirmed"}');
        "#).unwrap();
        let added = issue604_candidate_for_store_at(&store, &reader, 1_800_000_301);
        assert!(!added.history_available);
        assert_eq!(added.models_v3[0].total_tokens, 10);
        assert!(matches!(
            store.admit_candidate(Ok(added), &log, "test", None),
            RefreshStatus::RetainedLastGood { generation: 1 }
        ));
        assert!(Arc::ptr_eq(&old, &store.snapshot().unwrap()));
        assert!(store.inner.read().unwrap().reader_unavailable);
    }

    #[test]
    fn issue604_current_model_retention_expiry_is_an_admissible_same_commit_reprojection() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("current-retention.sqlite3");
        let reader = issue604_time_projection_fixture(&path);
        Connection::open(&path).unwrap().execute_batch(r#"
            DELETE FROM usage_history; DELETE FROM usage_model_history; DELETE FROM durable_state;
            UPDATE collection_generation SET reset_at=1794787200,window_seconds=13219200;
            UPDATE quota_receipts SET observed_at=1792108800,reset_at=1794787200,window_seconds=13219200;
            INSERT INTO usage_history VALUES(1784160000,1794787200,NULL,0.00005,0,0,10,0,0);
            INSERT INTO usage_model_history VALUES
                (1784160000,1794787200,'SOL','10','10','0','0','0',1,'stored-price');
            INSERT INTO durable_state VALUES
                (2,1784160000,'retained-model','{"kind":"codex-info-usage-observation-v1","timestamp":1784160000,"reset_at":1794787200,"remaining_percent":null,"model_source":"confirmed"}');
        "#).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let store = AccountStore::new(
            reader.clone(),
            7,
            Arc::new(Mutex::new(PairIssuer::from_epoch([0x33; 16]).unwrap())),
        );
        let log = FailureLog::default();
        let initial = issue604_candidate_for_store_at(&store, &reader, 1_792_108_800);
        assert_eq!(initial.models_v3[0].total_tokens, 10);
        assert!(matches!(
            store.admit_candidate(Ok(initial), &log, "test", None),
            RefreshStatus::Updated { generation: 1 }
        ));
        let old = store.snapshot().unwrap();
        // This unpartitioned legacy fixture keeps exact event seconds. Its
        // observation expires one second later, not at the next minute.
        let expired = issue604_candidate_for_store_at(&store, &reader, 1_792_108_801);
        assert!(
            expired.models_v3.is_empty(),
            "the reader must qualify current models by the retention window"
        );
        assert!(
            matches!(
                store.admit_candidate(Ok(expired), &log, "test", None),
                RefreshStatus::Updated { generation: 1 }
            ),
            "verified time expiry must not become a reader failure"
        );
        let new = store.snapshot().unwrap();
        assert_ne!(new.pair, old.pair);
        assert_eq!(new.details.quota, old.details.quota);
        assert_eq!(
            new.current.models_status,
            codex_info_db_reader::CurrentModelsStatus::Unavailable
        );
        assert!(!store.inner.read().unwrap().reader_unavailable);
        let restored = issue604_candidate_for_store_at(&store, &reader, 1_792_108_800);
        assert_eq!(restored.models_v3[0].total_tokens, 10);
        assert!(matches!(
            store.admit_candidate(Ok(restored), &log, "test", None),
            RefreshStatus::Updated { generation: 1 }
        ));
        assert_eq!(store.snapshot().unwrap().details.quota, old.details.quota);
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
    }

    #[test]
    fn issue604_same_commit_time_qualification_advances_and_rolls_back_without_rewriting_facts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("same-commit-time.sqlite3");
        let reader = issue604_time_projection_fixture(&path);
        let store = AccountStore::new(
            reader.clone(),
            7,
            Arc::new(Mutex::new(PairIssuer::from_epoch([0x31; 16]).unwrap())),
        );
        let log = FailureLog::default();
        let old = issue604_candidate_for_store_at(&store, &reader, 1_800_000_300);
        assert_eq!(old.models_v3[0].total_tokens, 10);
        assert!(matches!(
            store.admit_candidate(Ok(old), &log, "test", None),
            RefreshStatus::Updated { generation: 1 }
        ));
        let original = store.snapshot().unwrap();
        let later = issue604_candidate_for_store_at(&store, &reader, 1_800_000_360);
        assert_eq!(later.models_v3[0].total_tokens, 20);
        assert_eq!(later.retained_history.samples.len(), 2);
        assert!(
            matches!(
                store.admit_candidate(Ok(later), &log, "test", None),
                RefreshStatus::Updated { generation: 1 }
            ),
            "a newly time-eligible original observation must not be rejected as a DB rewrite"
        );
        let advanced = store.snapshot().unwrap();
        assert_ne!(advanced.pair, original.pair);
        assert_eq!(advanced.details.observed_at, Some(1_800_000_360));
        assert_eq!(advanced.details.quota, original.details.quota);
        assert_eq!(
            advanced.retained_history.samples[0],
            original.retained_history.samples[0]
        );
        Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE collection_generation SET data_generation='2', cycle_seq='2'",
                [],
            )
            .unwrap();
        let same = issue604_candidate_for_store_at(&store, &reader, 1_800_000_361);
        assert!(matches!(
            store.admit_candidate(Ok(same), &log, "test", None),
            RefreshStatus::Unchanged { generation: 2 }
        ));
        assert_eq!(store.snapshot().unwrap().pair, advanced.pair);
        let earlier = issue604_candidate_for_store_at(&store, &reader, 1_800_000_300);
        assert!(matches!(
            store.admit_candidate(Ok(earlier), &log, "test", None),
            RefreshStatus::Updated { generation: 2 }
        ));
        let rolled_back = store.snapshot().unwrap();
        assert_eq!(rolled_back.models_v3[0].total_tokens, 10);
        assert_ne!(rolled_back.pair, advanced.pair);
        assert!(!store.inner.read().unwrap().reader_unavailable);
    }

    #[test]
    fn issue604_same_commit_time_projection_rejects_changes_to_known_facts_and_keeps_the_pair() {
        for defect in [
            "model",
            "usd",
            "addition",
            "missing",
            "quota-identity",
            "source-price",
            "past-quota-identity",
            "collection-sequence",
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("same-commit-rewrite.sqlite3");
            let reader = issue604_time_projection_fixture(&path);
            if defect == "past-quota-identity" {
                Connection::open(&path).unwrap().execute_batch(
                    "UPDATE quota_receipts SET sequence=2;
                    INSERT INTO quota_receipts VALUES('past',1,1799999940,1800604740,604800,100,'1');"
                ).unwrap();
            }
            let store = AccountStore::new(
                reader.clone(),
                7,
                Arc::new(Mutex::new(PairIssuer::from_epoch([0x32; 16]).unwrap())),
            );
            let log = FailureLog::default();
            assert!(matches!(
                store.admit_candidate(
                    Ok(issue604_candidate_at(&reader, 1_800_000_300)),
                    &log,
                    "test",
                    None
                ),
                RefreshStatus::Updated { .. }
            ));
            let original = store.snapshot().unwrap();
            let db = Connection::open(&path).unwrap();
            db.execute_batch(match defect {
                "model" => "UPDATE usage_model_history SET total_tokens='99',input_tokens='99' WHERE timestamp=1800000000;",
                "usd" => "UPDATE usage_history SET sol_dollars=0.00999 WHERE timestamp=1800000000;",
                "addition" => "INSERT INTO usage_history VALUES(1800000060,1800604800,NULL,0.00006,0,0,12,0,0);",
                "missing" => "DELETE FROM usage_history WHERE timestamp=1800000000; DELETE FROM usage_model_history WHERE timestamp=1800000000; DELETE FROM durable_state WHERE data_generation=1800000000;",
                "quota-identity" => "UPDATE quota_receipts SET receipt_id='uncommitted-rewrite';",
                "past-quota-identity" => "UPDATE quota_receipts SET receipt_id='uncommitted-past-rewrite' WHERE sequence=1;",
                "collection-sequence" => "UPDATE collection_generation SET cycle_seq='2';",
                _ => "UPDATE usage_model_history SET price_version='uncommitted-rewrite' WHERE timestamp=1800000000;",
            }).unwrap();
            // A stored source-price edit has no public value change. It must
            // still be detected before an unchanged-publication fast path.
            let at = if matches!(defect, "source-price" | "past-quota-identity") {
                1_800_000_300
            } else {
                1_800_000_360
            };
            let candidate = issue604_candidate_for_store_at(&store, &reader, at);
            assert!(
                matches!(
                    store.admit_candidate(Ok(candidate), &log, "test", None),
                    RefreshStatus::RetainedLastGood { generation: 1 }
                ),
                "same-commit rewrite accepted: {defect}"
            );
            assert!(
                Arc::ptr_eq(&store.snapshot().unwrap(), &original),
                "old frame changed: {defect}"
            );
            assert!(
                store.inner.read().unwrap().reader_unavailable,
                "failed proof must make the route unavailable: {defect}"
            );
        }
    }

    #[test]
    fn issue604_history_failure_keeps_current_and_retries_cannot_erase_known_history() {
        for tamper in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("history-lane-recovery.sqlite3");
            let reader = issue604_time_projection_fixture(&path);
            let store = AccountStore::new(
                reader.clone(),
                7,
                Arc::new(Mutex::new(PairIssuer::from_epoch([0x33; 16]).unwrap())),
            );
            let log = FailureLog::default();
            assert!(matches!(
                store.admit_candidate(
                    Ok(issue604_candidate_for_store_at(
                        &store,
                        &reader,
                        1_800_000_300
                    )),
                    &log,
                    "test",
                    None
                ),
                RefreshStatus::Updated { generation: 1 }
            ));
            let original = store.snapshot().unwrap();
            let db = Connection::open(&path).unwrap();
            db.execute_batch(
                "ALTER TABLE usage_history RENAME COLUMN sol_dollars TO unavailable_dollars;",
            )
            .unwrap();
            assert!(matches!(
                store.admit_candidate(
                    Ok(issue604_candidate_for_store_at(
                        &store,
                        &reader,
                        1_800_000_301
                    )),
                    &log,
                    "test",
                    None
                ),
                RefreshStatus::Updated { generation: 1 }
            ));
            let failed_history = store.snapshot().unwrap();
            assert!(!store.inner.read().unwrap().reader_unavailable);
            let current: Value = serde_json::from_slice(
                &serialize_route(&failed_history, Route::CurrentV4, None, None, false).unwrap(),
            )
            .unwrap();
            assert_eq!(current["state"], "ready");
            assert_eq!(current["models_status"], "complete");
            assert_eq!(current["models"][0]["total_tokens"], 10);
            assert_eq!(failed_history.details.quota, original.details.quota);
            assert_eq!(
                serialize_route(&failed_history, Route::HistoryPeriodsV5, None, None, false),
                Err(RouteError::HistoryUnavailable)
            );
            assert!(matches!(
                store.admit_candidate(
                    Ok(issue604_candidate_for_store_at(
                        &store,
                        &reader,
                        1_800_000_302
                    )),
                    &log,
                    "test",
                    None
                ),
                RefreshStatus::Unchanged { generation: 1 }
            ));
            let held = store.snapshot().unwrap();
            assert_eq!(held.pair, failed_history.pair);
            if tamper {
                db.execute("UPDATE usage_history SET unavailable_dollars=0.00006 WHERE timestamp=1800000000", []).unwrap();
            }
            db.execute_batch(
                "ALTER TABLE usage_history RENAME COLUMN unavailable_dollars TO sol_dollars;",
            )
            .unwrap();
            let result = store.admit_candidate(
                Ok(issue604_candidate_for_store_at(
                    &store,
                    &reader,
                    1_800_000_303,
                )),
                &log,
                "test",
                None,
            );
            if tamper {
                assert!(
                    matches!(result, RefreshStatus::RetainedLastGood { generation: 1 }),
                    "a retry must not erase the last verified history's known USD"
                );
                assert_eq!(store.snapshot().unwrap().pair, held.pair);
                assert!(store.inner.read().unwrap().reader_unavailable);
            } else {
                assert!(matches!(result, RefreshStatus::Updated { generation: 1 }));
                let recovered = store.snapshot().unwrap();
                assert!(recovered.history_available);
                assert_ne!(recovered.pair, held.pair);
                assert_eq!(recovered.retained_history, original.retained_history);
            }
        }
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
        let samples: Vec<_> = (0..count)
            .map(|i| codex_info_rest_contract::PublicHistoryObservationV3 {
                timestamp: start + i as i64 * 60,
                reset_at: reset,
                remaining_percent: Some(if corrected && i == 0 { 70.0 } else { 80.0 }),
                task_active_since_previous: None,
                models: None,
                models_complete: false,
                model_source: "legacy-unknown".to_owned(),
            })
            .collect();
        PublishedSnapshot::from_db_for_account(
            DbSnapshot {
                recordings: Some(Arc::default()),
                generation,
                data_hash: format!("{generation:064x}"),
                has_pending_ranges: false,
                acquisition_degraded: false,
                history_available: true,
                current: CurrentPublication::unavailable(details.state),
                retained_history: HistoryArchive {
                    periods: details.history_periods.clone(),
                    gaps: details.history_gaps.clone(),
                    samples: samples.clone(),
                    origins: BTreeMap::new(),
                },
                reprojection_witness: None,
                details,
                models_v3: Vec::new(),
                history_samples_v2: Vec::new(),
                history_samples_v3: samples,
            },
            epoch,
        )
    }

    // Private serialization utilities below preserve saved bytes/cursors. They
    // do not establish that legacy provider-period resources may return 200.
    fn refresh_serialization_fixture(server: &RestServer) {
        assert!(matches!(
            server.store().refresh(),
            RefreshStatus::Updated { .. } | RefreshStatus::Unchanged { .. }
        ));
    }

    fn publish_serialization_fixture_at(store: &SnapshotStore, at: i64) {
        let id = store.default_account_id().expect("fixture account");
        let intent = store.reader_control.pending.lock().unwrap().latest[&id].clone();
        let candidate = store.prepare_reader_candidate_at(&id, at);
        assert!(
            candidate.is_ok(),
            "fixture candidate failed: {:?}",
            candidate.as_ref().err()
        );
        store.complete_reader_candidate(&id, &intent, candidate);
        let (snapshot, reader_unavailable) = store.capture_account_frame(&id);
        assert!(snapshot.is_some(), "fixture publication must exist");
        assert!(
            !reader_unavailable,
            "fixed-clock fixture reader must be available"
        );
    }

    fn fixed_time_serialization_fixture(reader: DbReader, at: i64) -> RestServer {
        let mut store = SnapshotStore::new(reader);
        store.publication_clock = Arc::new(move || Some(at));
        publish_serialization_fixture_at(&store, at);
        RestServer::start_with_store(store, "127.0.0.1:0".parse().unwrap()).unwrap()
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
        let bytes = serialize_history(snapshot, Some(period), cursor)?;
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
            recordings: Some(Arc::default()),
            generation,
            data_hash: format!("{generation:064x}"),
            has_pending_ranges: snapshot.has_pending_ranges,
            acquisition_degraded: snapshot.acquisition_degraded,
            history_available: snapshot.history_available,
            current: snapshot.current,
            details: snapshot.details,
            models_v3: snapshot.models_v3,
            history_samples_v2: snapshot.history_samples_v2,
            history_samples_v3: snapshot.history_samples_v3,
            retained_history: snapshot.retained_history,
            reprojection_witness: snapshot.reprojection_witness,
        };
        edit(&mut candidate);
        candidate.retained_history = HistoryArchive {
            periods: candidate.details.history_periods.clone(),
            gaps: candidate.details.history_gaps.clone(),
            samples: candidate.history_samples_v3.clone(),
            origins: BTreeMap::new(),
        };
        PublishedSnapshot::from_db_for_account(candidate, storage_epoch)
    }

    #[test]
    fn issue604_v5_directory_pages_are_lazy_reused_and_expire_with_the_pair() {
        // Directory serialization is also exercised through its public HTTP route.
        let snapshot = history_test_fixture(10, 1, 1, false, false, "1802678400");
        DIRECTORY_ROW_ENCODINGS.with(|count| count.set(0));
        let first =
            serialize_directory(&snapshot, Route::HistoryPeriodsV5, None, MAX_BODY_BYTES).unwrap();
        assert_eq!(DIRECTORY_ROW_ENCODINGS.with(|count| count.replace(0)), 1);
        let shared_frame = snapshot.clone();
        let repeated =
            serialize_directory(&shared_frame, Route::HistoryPeriodsV5, None, MAX_BODY_BYTES)
                .unwrap();
        assert_eq!(first, repeated);
        assert_eq!(
            DIRECTORY_ROW_ENCODINGS.with(|count| count.replace(0)),
            0,
            "another connection to the same immutable frame must reuse the encoded page"
        );
        let next = history_test_edit(snapshot, 10, |candidate| {
            candidate.details.history_periods[0].label =
                "2027/01/15 16:50:00 +09:00 ～ 2027/01/15 17:00:00 +09:00（現在）".into();
        });
        let refreshed =
            serialize_directory(&next, Route::HistoryPeriodsV5, None, MAX_BODY_BYTES).unwrap();
        assert_ne!(first, refreshed);
        assert_eq!(DIRECTORY_ROW_ENCODINGS.with(|count| count.replace(0)), 1);
    }

    #[test]
    fn issue604_v5_directory_query_classification() {
        for path in ["/v5/history/periods", "/v5/history/periods/selectable"] {
            let (route, account, period, cursor) =
                parse_target(&format!("{path}?account=account-10&cursor=opaque")).unwrap();
            assert!(route.is_some(), "v5 directory route must be registered");
            assert_eq!(account.as_deref(), Some("account-10"));
            assert!(period.is_none());
            assert_eq!(cursor.as_deref(), Some("opaque"));
            for suffix in [
                "?cursor=",
                "?cursor=a&cursor=b",
                "?account=bad",
                "?account=account-10&account=account-10",
                "?cursor=%31",
                "#fragment",
            ] {
                assert!(matches!(
                    parse_target(&format!("{path}{suffix}")),
                    Err(ParseError::BadRequest)
                ));
            }
            for suffix in ["?unknown=value", "?period=1802678400"] {
                assert!(parse_target(&format!("{path}{suffix}"))
                    .unwrap()
                    .0
                    .is_none());
            }
        }
    }

    #[test]
    fn issue604_v5_directory_empty_and_current_sets_preserve_schema() {
        // Directory serialization is also exercised through its public HTTP route.
        let original = history_test_fixture(10, 1, 0, false, false, "1802678400");
        for current in [false, true] {
            let snapshot = history_test_edit(original.clone(), 10, |candidate| {
                candidate.details.history_periods[0].current = current;
            });
            for path in ["/v5/history/periods", "/v5/history/periods/selectable"] {
                let route = parse_target(path).unwrap().0.expect("v5 directory route");
                let bytes = serialize_directory(&snapshot, route, None, MAX_BODY_BYTES).unwrap();
                let page: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(page.as_object().unwrap().len(), 3);
                assert_eq!(page["api_version"], "v5");
                assert_eq!(
                    page["history_periods"].as_array().unwrap().len(),
                    usize::from(current)
                );
                assert!(page["next_cursor"].is_null());
            }
        }
    }

    #[test]
    fn issue604_v5_directory_pages_preserve_rows_bytes_and_cursor_domains() {
        // Directory serialization is also exercised through its public HTTP route.
        let (route, _, _, _) = parse_target("/v5/history/periods").unwrap();
        let route = route.expect("v5 acquisition directory must be registered");
        let (selectable, _, _, _) = parse_target("/v5/history/periods/selectable").unwrap();
        let selectable = selectable.expect("v5 selectable directory must be registered");
        // Independent compact-JSON byte oracle: the first 3 rows and actual
        // cursor occupy 683 UTF-8 bytes; 4 rows occupy 868 bytes (860 Unicode
        // scalars). An injected 861-byte budget therefore admits exactly 3.
        // Production always supplies the unchanged 32 MiB constant.
        const BUDGET: usize = 861;
        let labels = [
            "2027/01/15 16:50:00 +09:00 ～ 2027/01/15 17:00:00 +09:00",
            "2027/01/15 16:51:00 +09:00 ～ 2027/01/15 17:01:00 +09:00",
            "2027/01/15 16:52:00 +09:00 ～ 2027/01/15 17:02:00 +09:00",
            "2027/01/15 16:53:00 +09:00 ～ 2027/01/15 17:03:00 +09:00",
            "2027/01/15 16:54:00 +09:00 ～ 2027/02/15 17:04:00 +09:00（現在）",
        ];
        let reset = 1_802_678_400_i64;
        let snapshot = history_test_edit(
            history_test_fixture(10, 1, 1, false, false, &reset.to_string()),
            10,
            |candidate| {
                candidate.details.history_periods = (0..5)
                    .rev()
                    .map(|index| codex_info_rest_contract::PublicHistoryPeriod {
                        id: (reset + index * 60).to_string(),
                        start_at: 1_799_999_400 + index * 60,
                        end_at: 1_800_000_000 + index * 60,
                        reset_at: reset + index * 60,
                        label: labels[index as usize].to_owned(),
                        current: index == 4,
                    })
                    .collect();
                candidate.history_samples_v3 = (0..5)
                    .map(
                        |index| codex_info_rest_contract::PublicHistoryObservationV3 {
                            timestamp: 1_800_000_000 + index * 60,
                            reset_at: reset + index * 60,
                            remaining_percent: Some(90.0),
                            task_active_since_previous: None,
                            models: Some(vec![
                                codex_info_rest_contract::PublicHistoryModelUsageV3 {
                                    model: "SOL".into(),
                                    total_tokens: 1,
                                    input_tokens: None,
                                    cached_input_tokens: None,
                                    cache_write_input_tokens: None,
                                    output_tokens: None,
                                    total_dollars: None,
                                },
                            ]),
                            models_complete: true,
                            model_source: "confirmed".into(),
                        },
                    )
                    .collect();
            },
        );
        let first = serialize_directory(&snapshot, route, None, BUDGET).unwrap();
        assert_eq!(first.len(), 683);
        assert!(first.starts_with(
            b"{\"api_version\":\"v5\",\"history_periods\":[{\"id\":\"1802678400\",\"start_at\":"
        ));
        let page: Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(page.as_object().unwrap().len(), 3);
        assert_eq!(page["api_version"], "v5");
        assert_eq!(page["history_periods"].as_array().unwrap().len(), 3);
        for (index, period) in page["history_periods"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            assert_eq!(period.as_object().unwrap().len(), 7);
            assert_eq!(period["reset_at"].as_i64(), Some(reset + index as i64 * 60));
            assert_eq!(
                period["id"].as_str(),
                Some((reset + index as i64 * 60).to_string().as_str())
            );
            assert_eq!(period["label"].as_str(), Some(labels[index]));
            assert!(period["origin_at"].is_null());
        }
        let cursor = page["next_cursor"].as_str().unwrap();
        assert_eq!(cursor, format!("d1.{}.a.3", &snapshot.pair[3..]));
        let (parsed_route, account, _, parsed_cursor) = parse_target(&format!(
            "/v5/history/periods?account=account-10&cursor={cursor}"
        ))
        .unwrap();
        assert_eq!(parsed_route, Some(route));
        assert_eq!(account.as_deref(), Some("account-10"));
        assert_eq!(parsed_cursor.as_deref(), Some(cursor));
        let second = serialize_directory(&snapshot, route, Some(cursor), BUDGET).unwrap();
        assert_eq!(second.len(), 440);
        let second: Value = serde_json::from_slice(&second).unwrap();
        assert_eq!(second["history_periods"].as_array().unwrap().len(), 2);
        assert_eq!(
            second["history_periods"][0]["reset_at"].as_i64(),
            Some(reset + 180)
        );
        assert!(second["next_cursor"].is_null());
        assert!(matches!(
            serialize_directory(&snapshot, selectable, Some(cursor), BUDGET),
            Err(RouteError::StaleCursor)
        ));
        for offset in ["0", "03", "5", "6", "-1", "18446744073709551616"] {
            let invalid = format!("d1.{}.a.{offset}", &snapshot.pair[3..]);
            assert!(matches!(
                serialize_directory(&snapshot, route, Some(&invalid), BUDGET),
                Err(RouteError::StaleCursor)
            ));
        }
        for epoch in [10, 11] {
            let changed = history_test_edit(snapshot.clone(), epoch, |_| {});
            assert!(matches!(
                serialize_directory(&changed, route, Some(cursor), BUDGET),
                Err(RouteError::StaleCursor)
            ));
        }
    }

    #[test]
    fn issue604_v5_directory_rejects_a_single_row_above_injected_page_budget() {
        // Directory serialization is also exercised through its public HTTP route.
        let (route, _, _, _) = parse_target("/v5/history/periods").unwrap();
        let route = route.expect("v5 directory must be registered");
        let snapshot = history_test_edit(
            history_test_fixture(10, 1, 1, false, false, "1802678400"),
            10,
            |candidate| {
                candidate.details.history_periods[0].current = false;
                candidate.details.history_periods[0].label =
                    "2027/01/15 16:50:00 +09:00 ～ 2027/01/15 17:00:00 +09:00".to_owned();
            },
        );
        // A valid single row fits the public limit. This private budget tests
        // the paginator's refusal to emit a nonterminal empty page.
        let error = serialize_directory(&snapshot, route, None, 64).err();
        assert!(error.is_some_and(|error| format!("{error:?}") == "ResourceTooLarge"));
    }

    #[test]
    fn issue604_v5_directory_validates_cursor_before_matching_etag() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("issue604-v5-etag.sqlite3");
        fixture(&path, 1);
        let mut server = ready_fixture_server_at(DbReader::open(&path).unwrap(), 1_800_000_060);
        refresh_serialization_fixture(&server);
        // A matching ETag cannot bypass an invalid directory cursor.
        let first = request(
            server.local_addr(),
            "GET /v5/history/periods HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(first.starts_with("HTTP/1.1 200"), "{first}");
        let snapshot = server.store().snapshot().unwrap();
        let pair = snapshot.pair.clone();
        let cursor = format!("d1.{}.a.1", &pair[3..]);
        let stale = request(
            server.local_addr(),
            &format!(
                "GET /v5/history/periods?cursor={cursor} HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{pair}\"\r\n\r\n"
            ),
        );
        assert!(stale.starts_with("HTTP/1.1 400"), "{stale}");
        assert!(matches!(
            directory_start(&snapshot, Route::HistoryPeriodsV5, Some(&cursor)),
            Err(RouteError::StaleCursor)
        ));
        let cached = request(
            server.local_addr(),
            &format!(
                "GET /v5/history/periods HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{pair}\"\r\n\r\n"
            ),
        );
        assert!(cached.starts_with("HTTP/1.1 304"), "{cached}");
        let repeated = request(
            server.local_addr(),
            "GET /v5/history/periods HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(repeated.starts_with("HTTP/1.1 200"), "{repeated}");
        server.shutdown();
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
    fn history_nearby_reset_keeps_gap_in_its_exact_period() {
        let original = history_test_fixture(10, 1, 2, false, true, "1802678400");
        let snapshot = history_test_edit(original, 10, |candidate| {
            let mut nearby = candidate.details.history_periods[0].clone();
            nearby.id = "nearby-reset".into();
            nearby.reset_at += 1;
            nearby.start_at = candidate.history_samples_v3[0].timestamp;
            nearby.current = false;
            candidate.details.history_periods.push(nearby);
        });
        let page = history_test_page(&snapshot, "nearby-reset", None).unwrap();
        assert!(page["history_gaps"].as_array().unwrap().is_empty());
        let original = history_test_page(&snapshot, "1802678400", None).unwrap();
        assert_eq!(original["history_gaps"].as_array().unwrap().len(), 1);
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
        let mut server = ready_fixture_server_at(reader, 1_800_000_060);
        refresh_serialization_fixture(&server);
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
            println!(
                "prefix={prefix} suffix=2 examined_rows={visits} binary_lookup_plus_suffix_bound={bound} debug_elapsed={elapsed:?}"
            );
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
                recordings: Some(Arc::default()),
                generation: 1,
                data_hash: format!("{:064x}", 1),
                has_pending_ranges: false,
                acquisition_degraded: false,
                history_available: true,
                current: CurrentPublication::unavailable(details.state),
                retained_history: HistoryArchive {
                    periods: details.history_periods.clone(),
                    gaps: details.history_gaps.clone(),
                    samples: history_samples.clone(),
                    origins: BTreeMap::new(),
                },
                reprojection_witness: None,
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
    fn official_reset_datetime_rollover_commits_and_updates_rest_current() {
        const OBSERVED: i64 = 2_000_000_040;
        const OLD_RESET: i64 = OBSERVED + 24 * 60 * 60;
        for (observed, reset, remaining) in [
            (OBSERVED, OBSERVED - 79 + 604_800, 100.0),
            (OBSERVED, OLD_RESET + 1, 9.0),
            (OBSERVED, OLD_RESET - 1, 5.0),
            (OBSERVED - 60, OLD_RESET + 60, 100.0),
        ] {
            assert_official_reset_reaches_rest(observed, reset, remaining, false);
        }
    }

    #[test]
    fn issue_637_official_weekly_reset_reaches_current_with_partial_session() {
        assert_official_reset_reaches_rest(2_000_000_040, 2_000_604_761, 100.0, true);
    }

    fn assert_official_reset_reaches_rest(
        next_observed_at: i64,
        next_reset_at: i64,
        remaining: f64,
        pending_history: bool,
    ) {
        use codex_info_db_writer::{StoragePartitionIdentity, UsageStore};
        use codex_info_recorder::{QuotaSnapshot, Recorder, RecorderConfig};

        // Fixed anonymous timestamps and percentages are an independent oracle.
        // The partial-session case uses real recorder collection; no production profile is used.
        const OBSERVED_AT: i64 = 2_000_000_040;
        const WINDOW: i64 = 7 * 24 * 60 * 60;
        const REPEAT_OBSERVED_AT: i64 = OBSERVED_AT + 120;
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
        recorder
            .run_cycle_with_quota(Some(QuotaSnapshot {
                observed_at: next_observed_at.min(OBSERVED_AT) - 60,
                reset_at: OBSERVED_AT + 24 * 60 * 60,
                window_seconds: WINDOW,
                remaining_percent: Some(10.0),
            }))
            .expect("previous minute")
            .expect("previous minute commit");
        let first = recorder
            .run_cycle_with_quota(Some(QuotaSnapshot {
                observed_at: OBSERVED_AT,
                reset_at: OBSERVED_AT + 24 * 60 * 60,
                window_seconds: WINDOW,
                remaining_percent: Some(10.0),
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
        // Async and explicit fixture publication use the same production
        // candidate entry and the same fixed anonymous observation horizon.
        let mut store = SnapshotStore::new(reader);
        store.publication_clock = Arc::new(|| Some(REPEAT_OBSERVED_AT));
        let publish_fixture = |store: &SnapshotStore| {
            let id = store.default_account_id().expect("fixture account");
            let intent = store.reader_control.pending.lock().unwrap().latest[&id].clone();
            let candidate = store.prepare_reader_candidate_at(&id, REPEAT_OBSERVED_AT);
            assert!(
                candidate.is_ok(),
                "fixture candidate failed: {:?}",
                candidate.as_ref().err()
            );
            store.complete_reader_candidate(&id, &intent, candidate);
            let (snapshot, reader_unavailable) = store.capture_account_frame(&id);
            assert!(snapshot.is_some(), "fixture publication must exist");
            assert!(
                !reader_unavailable,
                "fixed-clock fixture reader must be available"
            );
        };
        publish_fixture(&store);
        let mut server =
            RestServer::start_with_store(store, "127.0.0.1:0".parse().unwrap()).expect("test REST");
        let current_request = format!(
            "GET /{}/current HTTP/1.1\r\nHost:x\r\n\r\n",
            if pending_history { "v4" } else { "v3" }
        );
        let before = request(server.local_addr(), &current_request);
        assert!(before.starts_with("HTTP/1.1 200"));
        let before_value: Value = serde_json::from_str(body(&before)).expect("old current JSON");
        assert_eq!(before_value["state"], "ready");
        assert_eq!(before_value["observed_at"], OBSERVED_AT);
        assert_eq!(before_value["quota"]["remaining_percent"], 10.0);
        assert_eq!(
            before_value["quota"]["reset_at"],
            OBSERVED_AT + 24 * 60 * 60
        );
        assert_eq!(before_value["quota"]["window_seconds"], WINDOW);

        let partial = sessions.join("incomplete.jsonl");
        const PARTIAL_BYTES: &[u8] = br#"{"type":"response_item","payload":"#;
        if pending_history {
            fs::write(&partial, PARTIAL_BYTES).unwrap();
        }
        let rolled = recorder
            .run_cycle_with_quota(Some(QuotaSnapshot {
                observed_at: next_observed_at,
                reset_at: next_reset_at,
                window_seconds: WINDOW,
                remaining_percent: Some(remaining),
            }))
            .expect("idle rollover cycle")
            .expect("idle quota-only commit");
        assert!(rolled.generation > first.generation);
        assert_eq!(rolled.accepted_ranges, 0);
        assert_eq!(rolled.pending_ranges, usize::from(pending_history));
        assert_eq!(rolled.sources_seen, usize::from(pending_history));
        assert_eq!(
            fs::read_dir(&sessions).unwrap().count(),
            usize::from(pending_history)
        );

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
            (next_reset_at, next_reset_at, WINDOW)
        );
        let quota_row: (i64, i64, f64) = connection
            .query_row(
                "SELECT timestamp, reset_at, remaining_percent FROM usage_history
                 WHERE reset_at=?1 ORDER BY timestamp DESC LIMIT 1",
                [next_reset_at],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(quota_row, (next_observed_at, next_reset_at, remaining));
        let event_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(event_count, 0);
        drop(connection);

        DbReader::open_partitioned(&database, &reader_identity)
            .unwrap()
            .read_snapshot()
            .unwrap_or_else(|error| {
                panic!("reset={next_reset_at} observed={next_observed_at}: {error:?}")
            });
        publish_fixture(&server.store());
        let after = request(server.local_addr(), &current_request);
        assert!(after.starts_with("HTTP/1.1 200"));
        let after_value: Value = serde_json::from_str(body(&after)).expect("new current JSON");
        assert_eq!(
            after_value["state"], "ready",
            "an incomplete Session record is not an acquisition or publication failure"
        );
        if pending_history {
            assert_eq!(after_value["api_version"], "v4");
            assert_eq!(before_value["models_status"], "unavailable");
            assert_eq!(before_value["models"], json!([]));
            assert_eq!(after_value["models"], json!([]));
            assert_eq!(after_value["models_status"], "unavailable");
        }
        assert_eq!(
            server.store().snapshot().unwrap().has_pending_ranges,
            pending_history
        );
        assert_eq!(
            after_value["observed_at"],
            next_observed_at.max(OBSERVED_AT)
        );
        assert_eq!(after_value["quota"]["remaining_percent"], remaining);
        assert_eq!(after_value["quota"]["reset_at"], next_reset_at);
        assert_eq!(after_value["quota"]["window_seconds"], WINDOW);
        assert_ne!(published_pair(&before), published_pair(&after));

        recorder
            .run_cycle_with_quota(Some(QuotaSnapshot {
                observed_at: REPEAT_OBSERVED_AT,
                reset_at: next_reset_at,
                window_seconds: WINDOW,
                remaining_percent: Some(99.0),
            }))
            .expect("repeat official reset identity")
            .expect("repeat quota commit");
        let state = recorder.state().expect("repeated reset state");
        assert_eq!(state.reset_at, next_reset_at);
        assert_eq!(state.latest_quota_reset_at, next_reset_at);
        let period_count: i64 = Connection::open(&database)
            .expect("fixture DB after repeated reset")
            .query_row(
                "SELECT COUNT(DISTINCT reset_at) FROM usage_history",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            period_count, 2,
            "the distinct raw reset storage key is retained"
        );
        publish_fixture(&server.store());
        let repeated = request(server.local_addr(), &current_request);
        assert!(repeated.starts_with("HTTP/1.1 200"));
        let repeated_value: Value =
            serde_json::from_str(body(&repeated)).expect("repeated current JSON");
        assert_eq!(repeated_value["observed_at"], REPEAT_OBSERVED_AT);
        assert_eq!(repeated_value["quota"]["remaining_percent"], 99.0);
        assert_eq!(repeated_value["quota"]["reset_at"], next_reset_at);
        if pending_history {
            assert_eq!(repeated_value["state"], "ready");
            assert_eq!(repeated_value["models_status"], "unavailable");
            assert_eq!(fs::read(&partial).unwrap(), PARTIAL_BYTES);
            assert_eq!(
                Connection::open(&database)
                    .unwrap()
                    .query_row(
                        "SELECT count(*) FROM session_pending_ranges WHERE complete=0",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
            assert!(
                !server.store().snapshot().unwrap().acquisition_degraded,
                "source incompleteness is not an acquisition failure"
            );
        }
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
        refresh_serialization_fixture(&server);
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
        refresh_serialization_fixture(&server);

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
        assert!(matches!(
            server.store().refresh(),
            RefreshStatus::RetainedLastGood { .. }
        ));
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
        refresh_serialization_fixture(&server);
        server.shutdown();
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn v3_wire_marks_last_good_snapshot_degraded_without_clearing_data() {
        let path = temp_db("last-good-v3-wire");
        fixture(&path, 10);
        let reader = DbReader::open(&path).expect("reader");
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).expect("server");
        refresh_serialization_fixture(&server);

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
        assert!(matches!(
            server.store().refresh(),
            RefreshStatus::RetainedLastGood { .. }
        ));
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
        refresh_serialization_fixture(&server);
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
    fn current_v4_initializing_and_account_boundary_are_strictly_empty() {
        let path = temp_db("current-v4-empty-states");
        fixture(&path, 10);
        Connection::open(&path)
            .unwrap()
            .execute_batch("DELETE FROM usage_history; DELETE FROM durable_state;")
            .unwrap();
        let reader = DbReader::open(&path).unwrap();
        let mut server = RestServer::start(reader, "127.0.0.1:0".parse().unwrap()).unwrap();
        for state in ["initializing", "auth_required"] {
            if state == "auth_required" {
                server
                    .store()
                    .publish_account_boundary(PublicState::AuthRequired);
            }
            let response = request(
                server.local_addr(),
                "GET /v4/current HTTP/1.1\r\nHost:x\r\n\r\n",
            );
            assert!(response.starts_with("HTTP/1.1 200"));
            assert!(!published_pair(&response).is_empty());
            let value: serde_json::Value = serde_json::from_str(body(&response)).unwrap();
            assert_eq!(value["api_version"], "v4");
            assert_eq!(value["state"], state);
            assert_eq!(value["models_status"], "unavailable");
            assert_eq!(value["authenticated"], false);
            assert!(value["observed_at"].is_null());
            assert!(value["quota"].is_null());
            assert!(value["plan_label"].is_null());
            assert_eq!(value["models"], serde_json::json!([]));
            assert_eq!(value["active_thread_count"], 0);
            assert_eq!(value["open_session_thread_count"], 0);
        }
        server.shutdown();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn issue_637_v4_cache_exposes_read_failure_and_recovery_without_losing_quota() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("v4-current.sqlite3");
        fixture(&path, 10);
        let mut server = RestServer::start(
            DbReader::open(&path).unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let get = |pair: Option<&str>| {
            let conditional = pair.map_or(String::new(), |pair| {
                format!("If-None-Match: \"{pair}\"\r\n")
            });
            request(
                server.local_addr(),
                &format!("GET /v4/current HTTP/1.1\r\nHost:x\r\n{conditional}\r\n"),
            )
        };
        let initial = get(None);
        assert!(initial.starts_with("HTTP/1.1 200"));
        let initial_json: Value = serde_json::from_str(body(&initial)).unwrap();
        assert_eq!(initial_json["state"], "ready");
        assert_eq!(initial_json["api_version"], "v4");
        let pair = published_pair(&initial);
        assert!(get(Some(pair)).starts_with("HTTP/1.1 304"));
        let moved = path.with_extension("unavailable");
        fs::rename(&path, &moved).unwrap();
        let failure = get(Some(pair));
        assert!(failure.starts_with("HTTP/1.1 200"));
        let error_json: Value = serde_json::from_str(body(&failure)).unwrap();
        assert_eq!(error_json["state"], "error");
        assert_eq!(error_json["quota"], initial_json["quota"]);
        assert_eq!(error_json["models"], initial_json["models"]);
        assert_eq!(error_json["models_status"], initial_json["models_status"]);
        let failed_pair = published_pair(&failure);
        assert_ne!(failed_pair, pair);
        fs::rename(moved, path).unwrap();
        let recovered = get(Some(failed_pair));
        assert!(recovered.starts_with("HTTP/1.1 200"));
        assert_eq!(
            serde_json::from_str::<Value>(body(&recovered)).unwrap(),
            initial_json
        );
        assert_eq!(published_pair(&recovered), pair);
        server.shutdown();
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
    fn issue604_normal_reset_count_keeps_current_models_and_history() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("normal-reset-count.sqlite3");
        fixture(&path, 10);
        let db = Connection::open(&path).unwrap();
        db.execute_batch("DELETE FROM usage_history; DELETE FROM durable_state;
            CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,sequence INTEGER,
                observed_at INTEGER,reset_at INTEGER,window_seconds INTEGER,
                remaining_percent REAL,applied_generation TEXT);
            CREATE TABLE session_model_totals(model TEXT PRIMARY KEY,total_tokens TEXT,
                input_tokens TEXT,cached_input_tokens TEXT,output_tokens TEXT,cache_write_input_tokens TEXT);
            INSERT INTO session_model_totals VALUES('SOL','755374','755000','0','374','0');
            CREATE TABLE usage_model_history(reset_at INTEGER,timestamp INTEGER,model TEXT,
                total_tokens TEXT,input_tokens TEXT,cached_input_tokens TEXT,output_tokens TEXT,
                cache_write_input_tokens TEXT,model_set_complete INTEGER);").unwrap();
        const NOW: i64 = 1_800_011_280;
        let base = 1_800_000_000;
        for count in [128_i64, 129, 188] {
            db.execute_batch(
                "DELETE FROM quota_receipts; DELETE FROM usage_history;
                DELETE FROM durable_state; DELETE FROM usage_model_history;",
            )
            .unwrap();
            for index in 0..count {
                let at = base + index * 60;
                db.execute(
                    "INSERT INTO quota_receipts VALUES(?1,?2,?3,?4,604800,100,'1')",
                    params![format!("receipt-{index}"), index + 1, at, at + 604800],
                )
                .unwrap();
            }
            let last = base + (count - 1) * 60;
            let reset = last + 604800;
            db.execute(
                "INSERT INTO usage_history VALUES(?1,?2,100,2.46325,0,0,755374,0,0)",
                params![last, reset],
            )
            .unwrap();
            db.execute(
                "INSERT INTO durable_state VALUES(2,?1,'source',?2)",
                params![
                    last,
                    json!({"kind":"codex-info-usage-observation-v1","timestamp":last,
                    "reset_at":reset,"remaining_percent":100.0,"model_source":"confirmed"})
                    .to_string()
                ],
            )
            .unwrap();
            db.execute("INSERT INTO usage_model_history VALUES(?1,?2,'SOL','755374','755000','0','374','0',1)",
                params![reset,last]).unwrap();
            let reader = DbReader::open(&path).unwrap();
            let snapshot = reader.read_snapshot().unwrap();
            assert_eq!(
                snapshot.current.state,
                PublicState::Ready,
                "normal official reset count {count} must preserve current data"
            );
            assert!(snapshot.history_available);
            assert_eq!(snapshot.retained_history.periods.len(), count as usize);
            assert!(snapshot
                .models_v3
                .iter()
                .any(|m| m.model == "SOL" && m.total_tokens == 755374));
            let mut server = ready_fixture_server_at(reader, NOW);
            refresh_serialization_fixture(&server);
            let response = request(
                server.local_addr(),
                "GET /v4/current HTTP/1.1\r\nHost:x\r\n\r\n",
            );
            assert!(response.starts_with("HTTP/1.1 200"));
            let current: Value = serde_json::from_str(body(&response)).unwrap();
            assert_eq!(current["state"], "ready");
            assert!(current["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["model"] == "SOL" && model["total_tokens"] == 755374));
            assert_ne!(current["models_status"], "unavailable");
            let published = server.store().snapshot().unwrap();
            assert_eq!(
                current["active_thread_count"],
                published.details.active_thread_count
            );
            assert_eq!(
                current["quota"],
                serde_json::to_value(&published.details.quota).unwrap()
            );
            assert!(published
                .models_v3
                .iter()
                .any(|m| m.model == "SOL" && m.total_tokens == 755374));
            for version in ["v3", "v4"] {
                let response = request(
                    server.local_addr(),
                    &format!("GET /{version}/history/periods HTTP/1.1\r\nHost:x\r\n\r\n"),
                );
                assert!(response.starts_with("HTTP/1.1 200"), "{response}");
                let directory: Value = serde_json::from_str(body(&response)).unwrap();
                assert_eq!(
                    directory["history_periods"].as_array().unwrap().len(),
                    count as usize
                );
                assert_eq!(published.retained_history.periods.len(), count as usize);
            }
            assert_eq!(
                db.query_row("SELECT count(*) FROM quota_receipts", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                count
            );
            server.shutdown();
        }
    }

    #[test]
    fn quota_current_is_independent_of_history_publication_failure() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("quota-history-failure.sqlite3");
        fixture(&path, 10);
        const FIXTURE_AT: i64 = 1_800_007_680;
        let mut server =
            fixed_time_serialization_fixture(DbReader::open(&path).unwrap(), FIXTURE_AT);
        let current_request = "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n";
        let previous = request(server.local_addr(), current_request);
        assert_eq!(
            serde_json::from_str::<Value>(body(&previous)).unwrap()["quota"]["remaining_percent"],
            50.0
        );
        // A corrupt historical percentage, not legitimate reset cardinality,
        // is the failure authority. The latest receipt remains valid.
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,
             sequence INTEGER, observed_at INTEGER, reset_at INTEGER,
             window_seconds INTEGER, remaining_percent REAL, applied_generation TEXT);
             WITH RECURSIVE receipts(n) AS (SELECT 0 UNION ALL SELECT n+1 FROM receipts WHERE n<128)
             INSERT INTO quota_receipts SELECT printf('%064x', n+1), n+1,
                 1800000000+n*60,1800604800+n*60,604800,CASE WHEN n=0 THEN -1.0 ELSE 99.0 END,'2' FROM receipts;
             UPDATE collection_generation SET data_generation='2';",
        )
        .unwrap();
        publish_serialization_fixture_at(&server.store(), FIXTURE_AT);
        let response = request(server.local_addr(), current_request);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let current: Value = serde_json::from_str(body(&response)).unwrap();
        assert_eq!(
            current["quota"]["remaining_percent"], 99.0,
            "history validation must not retain the old current quota"
        );
        assert_eq!(current["quota"]["reset_at"], 1_800_612_480_i64);
        assert_eq!(current["observed_at"], 1_800_007_680_i64);
        assert_eq!(current["state"], "error");
        assert!(current["authenticated"].as_bool().unwrap());
        assert_ne!(published_pair(&previous), published_pair(&response));
        for route in ["/v3/history/periods", "/v3/history?period=1800612480"] {
            let response = request(
                server.local_addr(),
                &format!("GET {route} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert!(response.starts_with("HTTP/1.1 503"), "{response}");
            assert_eq!(
                serde_json::from_str::<Value>(body(&response)).unwrap()["error"],
                "history_unavailable"
            );
        }
        assert_eq!(
            db.query_row("SELECT count(*) FROM quota_receipts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            129
        );
        server.shutdown();
    }

    #[test]
    fn quota_boundary_minute_reaches_http() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("boundary.sqlite3");
        fixture(&path, 10);
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "DELETE FROM usage_history; DELETE FROM durable_state;
             INSERT INTO usage_history VALUES(1800000000,1800604841,100,1,0,0,30,0,0);
             CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,
             sequence INTEGER, observed_at INTEGER, reset_at INTEGER,
             window_seconds INTEGER, remaining_percent REAL, applied_generation TEXT);
             INSERT INTO quota_receipts VALUES
             ('first',1,1800000041,1800604841,604800,100,'10'),
             ('second',2,1800000080,1800604841,604800,99,'10');
             UPDATE collection_generation SET data_generation='10';",
            )
            .unwrap();
        let mut server = RestServer::start(
            DbReader::open(&path).unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        refresh_serialization_fixture(&server);
        let response = request(
            server.local_addr(),
            "GET /v3/history?period=1800604841 HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let page: Value = serde_json::from_str(body(&response)).unwrap();
        let samples = page["history_samples"].as_array().unwrap();
        assert_eq!(
            samples.len(),
            2,
            "the boundary minute must survive reader and HTTP indexing"
        );
        assert_eq!(samples[0]["timestamp"], 1_800_000_000_i64);
        assert_eq!(samples[0]["remaining_percent"], 100.0);
        let sol = samples[0]["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["model"] == "SOL")
            .unwrap();
        assert_eq!(sol["total_tokens"], 30);
        assert_eq!(samples[1]["remaining_percent"], 99.0);
        let response = request(
            server.local_addr(),
            "GET /v3/history/periods HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let periods: Value = serde_json::from_str(body(&response)).unwrap();
        assert_eq!(periods["history_periods"][0]["start_at"], 1_800_000_041_i64);
        server.shutdown();
    }

    #[test]
    fn history_origin_v4_keeps_v3_compatible_and_unknown_origins_unknown() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("origins.sqlite3");
        fixture(&path, 10);
        Connection::open(&path).unwrap().execute_batch(
            "INSERT INTO usage_history VALUES(1799900000,1799900060,89,1,2,3,10,20,30);
             CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,sequence INTEGER,observed_at INTEGER,reset_at INTEGER,window_seconds INTEGER,remaining_percent REAL,applied_generation TEXT);
             INSERT INTO quota_receipts VALUES('receipt',1,1800000000,1800000060,3600,50,'1');"
        ).unwrap();
        let mut server = RestServer::start(
            DbReader::open(&path).unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        refresh_serialization_fixture(&server);
        let snapshot = server.store().snapshot().unwrap();
        assert_eq!(snapshot.retained_history.periods.len(), 2);
        assert_eq!(
            snapshot.retained_history.origins.get(&1_800_000_060),
            Some(&1_799_996_460)
        );
        assert!(
            !snapshot
                .retained_history
                .origins
                .contains_key(&1_799_900_060),
            "first recorded sample is not reset authority"
        );
        for suffix in ["", "/selectable"] {
            let old = request(
                server.local_addr(),
                &format!("GET /v3/history/periods{suffix} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert!(old.starts_with("HTTP/1.1 200"), "{old}");
            let old: Value = serde_json::from_str(body(&old)).unwrap();
            assert!(old["history_periods"].as_array().unwrap().iter().all(|p| p
                .as_object()
                .unwrap()
                .len()
                == 6));
            let new = request(
                server.local_addr(),
                &format!("GET /v4/history/periods{suffix} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert!(new.starts_with("HTTP/1.1 200"), "{new}");
            let new: Value = serde_json::from_str(body(&new)).unwrap();
            assert_eq!(new["api_version"], "v4");
            let periods = new["history_periods"].as_array().unwrap();
            assert_eq!(periods.len(), 2);
            assert_eq!(
                periods
                    .iter()
                    .find(|p| p["reset_at"] == 1800000060_i64)
                    .unwrap()["origin_at"],
                1799996460_i64
            );
            let unknown = periods
                .iter()
                .find(|p| p["reset_at"] == 1799900060_i64)
                .unwrap();
            assert!(unknown.as_object().unwrap().contains_key("origin_at"));
            assert!(
                unknown["origin_at"].is_null(),
                "first recorded sample is not reset authority"
            );
        }
        server.shutdown();
    }

    #[test]
    fn quota_receipt_history_reaches_http_without_models() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("quota-history.sqlite3");
        fixture(&path, 10);
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "DELETE FROM usage_history;
             CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,
             sequence INTEGER, observed_at INTEGER, reset_at INTEGER,
             window_seconds INTEGER, remaining_percent REAL, applied_generation TEXT);
             INSERT INTO quota_receipts VALUES
             ('first',1,1800000000,1800604800,604800,10,'10'),
             ('second',2,1800000060,1800604801,604800,100,'10'),
             ('third',3,1800000080,1800604801,604800,99,'10'),
             ('fourth',4,1800000120,1800604801,604800,98,'10');
             UPDATE collection_generation SET data_generation='10';",
            )
            .unwrap();
        let mut server =
            fixed_time_serialization_fixture(DbReader::open(&path).unwrap(), 1_800_000_180);
        let response = request(
            server.local_addr(),
            "GET /v3/history/periods HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let periods: Value = serde_json::from_str(body(&response)).unwrap();
        let periods = periods["history_periods"].as_array().unwrap();
        assert_eq!(periods.len(), 2);
        let new = periods.iter().find(|p| p["id"] == "1800604801").unwrap();
        let old = periods.iter().find(|p| p["id"] == "1800604800").unwrap();
        assert_eq!(new["start_at"], 1_800_000_001_i64);
        assert!(old["end_at"].as_i64().unwrap() < new["start_at"].as_i64().unwrap());
        for (reset, expected) in [("1800604800", vec![10.0]), ("1800604801", vec![99.0, 98.0])] {
            let response = request(
                server.local_addr(),
                &format!("GET /v3/history?period={reset} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            let page: Value = serde_json::from_str(body(&response)).unwrap();
            let samples = page["history_samples"].as_array().unwrap();
            assert_eq!(
                samples
                    .iter()
                    .map(|p| p["remaining_percent"].as_f64().unwrap())
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(samples.iter().all(|p| p["models"].is_null()));
        }
        server.shutdown();
    }

    #[test]
    fn issue646_selectable_periods_require_tokens_but_preserve_the_archive() {
        let (route, _, _, _) = parse_target("/v3/history/periods/selectable").unwrap();
        let route = route.expect("the selectable resource must exist");
        for (id, current, count, gap, model, expected) in [
            ("past-empty", false, 0, false, None, false),
            ("current-empty", true, 0, false, None, true),
            ("quota-only", false, 1, false, None, false),
            ("complete-zero", false, 1, false, Some((0, true)), false),
            ("partial-zero", false, 1, false, Some((0, false)), false),
            (
                "partial-positive",
                false,
                1,
                false,
                Some((28_924, false)),
                true,
            ),
            ("complete-positive", false, 1, false, Some((1, true)), true),
            ("gap-only", false, 0, true, None, false),
        ] {
            let original = history_test_fixture(10, 1, count, false, gap, id);
            let snapshot = history_test_edit(original, 10, |candidate| {
                candidate.details.history_periods[0].current = current;
                for point in &mut candidate.history_samples_v3 {
                    point.remaining_percent = Some(90.0);
                    point.models = model.map(|(total_tokens, _)| {
                        vec![codex_info_rest_contract::PublicHistoryModelUsageV3 {
                            model: "SOL".into(),
                            total_tokens,
                            input_tokens: None,
                            cached_input_tokens: None,
                            cache_write_input_tokens: None,
                            output_tokens: None,
                            total_dollars: None,
                        }]
                    });
                    point.models_complete = model.is_some_and(|(_, complete)| complete);
                    point.model_source = if model.is_some() {
                        "legacy-unknown"
                    } else {
                        "unavailable"
                    }
                    .into();
                }
            });
            let selected: Value = serde_json::from_slice(
                &serialize_route(&snapshot, route, None, None, false).unwrap(),
            )
            .unwrap();
            assert_eq!(
                !selected["history_periods"].as_array().unwrap().is_empty(),
                expected,
                "{id}"
            );
            // This is selection eligibility, never a mutation of recorded facts.
            let page = history_test_page(&snapshot, id, None).unwrap();
            assert_eq!(page["history_samples"].as_array().unwrap().len(), count);
            assert_eq!(
                page["history_gaps"].as_array().unwrap().len(),
                usize::from(gap)
            );
            let directory: Value = serde_json::from_slice(
                &serialize_route(&snapshot, Route::HistoryPeriodsV3, None, None, false).unwrap(),
            )
            .unwrap();
            assert_eq!(
                !directory["history_periods"].as_array().unwrap().is_empty(),
                current || count > 0 || gap
            );
            if count > 0 && !expected {
                let updated = history_test_edit(snapshot, 11, |candidate| {
                    candidate.history_samples_v3[0].models =
                        Some(vec![codex_info_rest_contract::PublicHistoryModelUsageV3 {
                            model: "SOL".into(),
                            total_tokens: 1,
                            input_tokens: None,
                            cached_input_tokens: None,
                            cache_write_input_tokens: None,
                            output_tokens: None,
                            total_dollars: None,
                        }]);
                });
                let visible: Value = serde_json::from_slice(
                    &serialize_route(&updated, route, None, None, false).unwrap(),
                )
                .unwrap();
                assert_eq!(
                    visible["history_periods"][0]["id"], id,
                    "late token evidence republishes the exact ID"
                );
            }
        }
    }

    #[test]
    fn issue646_selectable_keeps_one_second_apart_token_ids_exact() {
        let (route, _, _, _) = parse_target("/v3/history/periods/selectable").unwrap();
        let route = route.expect("the selectable resource must exist");
        let original = history_test_fixture(10, 1, 2, false, false, "first");
        let snapshot = history_test_edit(original, 10, |candidate| {
            candidate.details.history_periods[0].current = false;
            let mut second = candidate.details.history_periods[0].clone();
            second.id = "second".into();
            second.reset_at += 1;
            second.start_at += 1;
            candidate.details.history_periods.push(second);
            candidate.history_samples_v3[1].reset_at += 1;
            for (index, point) in candidate.history_samples_v3.iter_mut().enumerate() {
                point.models = Some(vec![codex_info_rest_contract::PublicHistoryModelUsageV3 {
                    model: "SOL".into(),
                    total_tokens: (index + 1) as u64,
                    input_tokens: None,
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                    output_tokens: None,
                    total_dollars: None,
                }]);
            }
        });
        let selected: Value =
            serde_json::from_slice(&serialize_route(&snapshot, route, None, None, false).unwrap())
                .unwrap();
        let ids = selected["history_periods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["first", "second"]);
        for (id, total) in [("first", 1), ("second", 2)] {
            let page = history_test_page(&snapshot, id, None).unwrap();
            assert_eq!(page["history_samples"].as_array().unwrap().len(), 1);
            assert_eq!(
                page["history_samples"][0]["models"][0]["total_tokens"],
                total
            );
        }
    }

    #[test]
    fn issue646_selectable_http_keeps_token_period_and_all_direct_pages() {
        const START: i64 = 1_800_000_001;
        const WINDOW: i64 = 604_800;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("token-periods.sqlite3");
        fixture(&path, 10);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "DELETE FROM usage_history; DELETE FROM durable_state;
            CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,sequence INTEGER,
            observed_at INTEGER,reset_at INTEGER,window_seconds INTEGER,
            remaining_percent REAL,applied_generation TEXT);
            CREATE TABLE usage_model_history(reset_at INTEGER,timestamp INTEGER,model TEXT,
            total_tokens TEXT,input_tokens TEXT,cached_input_tokens TEXT,output_tokens TEXT,
            cache_write_input_tokens TEXT,model_set_complete INTEGER);",
        )
        .unwrap();
        let starts = [START, START + 128, START + 190, START + 600];
        for (index, start) in starts.into_iter().enumerate() {
            let observed = start + if index == 2 { 120 } else { 60 };
            let timestamp = observed - observed.rem_euclid(60);
            db.execute(
                "INSERT INTO quota_receipts VALUES(?1,?2,?3,?4,?5,100,'10')",
                params![
                    index.to_string(),
                    index as i64 + 1,
                    observed,
                    start + WINDOW,
                    WINDOW
                ],
            )
            .unwrap();
            db.execute(
                "INSERT INTO usage_history VALUES(?1,?2,100,0,0,0,?3,0,0)",
                params![
                    timestamp,
                    start + WINDOW,
                    if index == 2 { 28_924 } else { 0 }
                ],
            )
            .unwrap();
            db.execute(
                "INSERT INTO usage_model_history VALUES(?1,?2,'SOL',?3,?3,'0','0','0',0)",
                params![
                    start + WINDOW,
                    timestamp,
                    if index == 2 { "28924" } else { "0" }
                ],
            )
            .unwrap();
        }
        db.execute(
            "UPDATE collection_generation SET data_generation='10',reset_at=?1,window_seconds=?2",
            params![starts[3] + WINDOW, WINDOW],
        )
        .unwrap();
        // Match the live account's canonical loader. The unpartitioned legacy
        // loader can reassign nearby reset aliases before quota projection.
        db.pragma_update(
            None,
            "user_version",
            codex_info_db_reader::HISTORY_CANONICAL_SCHEMA_VERSION,
        )
        .unwrap();
        db.execute_batch("CREATE UNIQUE INDEX usage_history_canonical_timestamp_idx ON usage_history(timestamp);
            CREATE UNIQUE INDEX usage_model_history_canonical_timestamp_model_idx ON usage_model_history(timestamp,model);
            CREATE TABLE storage_partition(singleton INTEGER,schema_version TEXT,profile_scope_id TEXT,
            account_scope_id TEXT,storage_epoch TEXT,partition_id TEXT);").unwrap();
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
            db.execute_batch(&format!(
                "CREATE TRIGGER {name} BEFORE {operation} ON {table} WHEN 0 BEGIN SELECT 1; END;"
            ))
            .unwrap();
        }
        let identity = codex_info_db_reader::StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 2,
            partition_id: "33".repeat(32),
        };
        db.execute(
            "INSERT INTO storage_partition VALUES(1,?1,?2,?3,'2',?4)",
            params![
                identity.schema_version,
                identity.profile_scope_id,
                identity.account_scope_id,
                identity.partition_id
            ],
        )
        .unwrap();
        let reader = DbReader::open_partitioned(&path, &identity).unwrap();
        let before = reader.read_snapshot().unwrap();
        for (index, start) in starts.into_iter().enumerate() {
            let points = before
                .retained_history
                .samples
                .iter()
                .filter(|point| point.reset_at == start + WINDOW)
                .collect::<Vec<_>>();
            assert_eq!(
                points.len(),
                1,
                "the canonical fixture has one in-bounds observation per exact reset"
            );
            assert_eq!(points[0].model_source, "legacy-unknown");
            assert!(!points[0].models_complete);
            assert_eq!(
                points[0]
                    .models
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|model| model.model == "SOL")
                    .unwrap()
                    .total_tokens,
                if index == 2 { 28_924 } else { 0 }
            );
        }
        let mut server = RestServer::start(reader.clone(), "127.0.0.1:0".parse().unwrap()).unwrap();
        refresh_serialization_fixture(&server);
        let directory = request(
            server.local_addr(),
            "GET /v3/history/periods HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(directory.starts_with("HTTP/1.1 200"));
        let all: Value = serde_json::from_str(body(&directory)).unwrap();
        assert_eq!(all["history_periods"].as_array().unwrap().len(), 4);
        let selected = request(
            server.local_addr(),
            "GET /v3/history/periods/selectable HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(selected.starts_with("HTTP/1.1 200"), "{selected}");
        assert_eq!(published_pair(&directory), published_pair(&selected));
        let selected: Value = serde_json::from_str(body(&selected)).unwrap();
        let ids = selected["history_periods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                (starts[3] + WINDOW).to_string(),
                (starts[2] + WINDOW).to_string()
            ]
        );
        for (index, start) in starts.into_iter().enumerate() {
            let reset = start + WINDOW;
            let response = request(
                server.local_addr(),
                &format!("GET /v3/history?period={reset} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert!(response.starts_with("HTTP/1.1 200"));
            assert_eq!(published_pair(&directory), published_pair(&response));
            let page: Value = serde_json::from_str(body(&response)).unwrap();
            let samples = page["history_samples"].as_array().unwrap();
            assert_eq!(samples.len(), 1);
            assert_eq!(samples[0]["models_complete"], false);
            assert_eq!(
                samples[0]["models"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|model| model["model"] == "SOL")
                    .unwrap()["total_tokens"],
                if index == 2 { 28_924 } else { 0 }
            );
        }
        assert_eq!(reader.read_snapshot().unwrap().data_hash, before.data_hash);
        assert_eq!(
            db.query_row("SELECT count(*) FROM usage_history", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            4
        );
        server.shutdown();
    }

    #[test]
    fn issue646_empty_period_catalog_keeps_current_observations_and_gaps() {
        for (id, current, count, gap, model, expected) in [
            ("past-empty", false, 0, false, None, false),
            ("current-empty", true, 0, false, None, true),
            ("quota-only", false, 1, false, None, true),
            ("zero-model", false, 1, false, Some((0, true)), true),
            ("partial-model", false, 1, false, Some((17, false)), true),
            ("gap-only", false, 0, true, None, true),
        ] {
            let original = history_test_fixture(10, 1, count, false, gap, id);
            let snapshot = history_test_edit(original, 10, |candidate| {
                candidate.details.history_periods[0].current = current;
                for point in &mut candidate.history_samples_v3 {
                    point.remaining_percent = Some(100.0);
                    point.model_source = "unavailable".into();
                    if let Some((total_tokens, complete)) = model {
                        point.models =
                            Some(vec![codex_info_rest_contract::PublicHistoryModelUsageV3 {
                                model: "gpt-6.1-sol".into(),
                                total_tokens,
                                input_tokens: None,
                                cached_input_tokens: None,
                                cache_write_input_tokens: None,
                                output_tokens: None,
                                total_dollars: None,
                            }]);
                        point.models_complete = complete;
                        point.model_source = "confirmed".into();
                    }
                }
            });
            let body =
                serialize_route(&snapshot, Route::HistoryPeriodsV3, None, None, false).unwrap();
            let catalog: Value = serde_json::from_slice(&body).unwrap();
            let periods = catalog["history_periods"].as_array().unwrap();
            assert_eq!(!periods.is_empty(), expected, "{id}");
            if expected {
                assert_eq!(periods[0]["id"], id);
            }
            // Selection visibility never revokes the original exact ID.
            let page = history_test_page(&snapshot, id, None).unwrap();
            assert_eq!(page["history_samples"].as_array().unwrap().len(), count);
            assert_eq!(
                page["history_gaps"].as_array().unwrap().len(),
                usize::from(gap)
            );
        }
    }

    #[test]
    fn issue646_empty_period_catalog_omits_moving_reset_placeholders() {
        // All nine raw reset storage keys and quota facts survive. Unknown
        // provider-period identity cannot classify earlier facts as unused.
        const START: i64 = 1_800_000_012;
        const WINDOW: i64 = 604_800;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("moving-reset.sqlite3");
        fixture(&path, 10);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "DELETE FROM usage_history; DELETE FROM durable_state;
             CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,
             sequence INTEGER, observed_at INTEGER, reset_at INTEGER,
             window_seconds INTEGER, remaining_percent REAL, applied_generation TEXT);",
        )
        .unwrap();
        for minute in 0..9_i64 {
            let start = START + minute * 60;
            let observed = start + 109;
            let timestamp = observed - observed.rem_euclid(60);
            db.execute(
                "INSERT INTO quota_receipts VALUES(?1,?2,?3,?4,?5,100,'10')",
                params![
                    minute.to_string(),
                    minute + 1,
                    observed,
                    start + WINDOW,
                    WINDOW
                ],
            )
            .unwrap();
            db.execute(
                "INSERT INTO usage_history VALUES(?1,?2,100,0,0,0,0,0,0)",
                params![timestamp, start + WINDOW],
            )
            .unwrap();
        }
        let current_reset = START + 8 * 60 + WINDOW;
        db.execute(
            "UPDATE collection_generation SET data_generation='10',reset_at=?1,window_seconds=?2",
            params![current_reset, WINDOW],
        )
        .unwrap();
        let reader = DbReader::open(&path).unwrap();
        let before = reader.read_snapshot().unwrap();
        assert_eq!(before.retained_history.periods.len(), 9);
        let mut server = RestServer::start(reader.clone(), "127.0.0.1:0".parse().unwrap()).unwrap();
        refresh_serialization_fixture(&server);
        let response = request(
            server.local_addr(),
            "GET /v3/history/periods HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let snapshot = server.store().snapshot().unwrap();
        let periods = &snapshot.directory_indices[0];
        // Full directory preserves raw keys; selectable applies its own eligibility.
        assert_eq!(periods.len(), 9);
        let ids = periods
            .iter()
            .map(|&i| snapshot.retained_history.periods[i].id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), 9, "every saved key is unique");
        assert_eq!(
            ids,
            (0..9_i64)
                .map(|minute| (START + minute * 60 + WINDOW).to_string())
                .collect::<std::collections::BTreeSet<_>>()
        );
        let current = periods
            .iter()
            .map(|&i| &snapshot.retained_history.periods[i])
            .filter(|p| p.current)
            .map(|p| p.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(current, vec![current_reset.to_string()]);
        let selectable = request(
            server.local_addr(),
            "GET /v3/history/periods/selectable HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(selectable.starts_with("HTTP/1.1 200"), "{selectable}");
        let selected: Value = serde_json::from_str(body(&selectable)).unwrap();
        assert_eq!(selected["history_periods"].as_array().unwrap().len(), 1);
        assert_eq!(
            selected["history_periods"][0]["id"],
            current_reset.to_string()
        );
        for minute in 0..9_i64 {
            let reset = START + minute * 60 + WINDOW;
            let page_response = request(
                server.local_addr(),
                &format!("GET /v3/history?period={reset} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert!(page_response.starts_with("HTTP/1.1 200"), "{page_response}");
            assert_eq!(published_pair(&page_response), published_pair(&response));
            let page: Value = serde_json::from_str(body(&page_response)).unwrap();
            let samples = page["history_samples"].as_array().unwrap();
            let observed = START + minute * 60 + 109;
            let original_timestamp = observed - observed.rem_euclid(60);
            assert_eq!(
                samples
                    .iter()
                    .filter(|point| point["timestamp"] == original_timestamp
                        && point["reset_at"] == reset
                        && point["remaining_percent"] == 100.0)
                    .count(),
                1,
                "the exact original quota tuple survives once"
            );
            // Saved compatibility observations do not create additional
            // consumption events or a guaranteed provider-period identity.
            let expected_minutes = if minute == 8 {
                vec![6, 7, 8]
            } else {
                vec![minute]
            };
            assert_eq!(samples.len(), expected_minutes.len());
            let timestamps = samples
                .iter()
                .map(|point| point["timestamp"].as_i64().unwrap())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(timestamps.len(), expected_minutes.len());
            assert_eq!(
                timestamps,
                expected_minutes
                    .into_iter()
                    .map(|m| {
                        let observed = START + m * 60 + 109;
                        observed - observed.rem_euclid(60)
                    })
                    .collect::<std::collections::BTreeSet<_>>()
            );
            assert!(samples
                .iter()
                .all(|point| point["reset_at"] == reset && point["remaining_percent"] == 100.0));
            assert!(page["history_gaps"].as_array().unwrap().is_empty());
            assert!(page["next_cursor"].is_null());
        }
        assert_eq!(reader.read_snapshot().unwrap().data_hash, before.data_hash);
        for table in ["quota_receipts", "usage_history"] {
            let count: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 9, "{table} retains every original row");
        }
        server.shutdown();
    }

    #[test]
    fn issue646_retained_history_is_paged_without_expanding_legacy_details() {
        const NOW: i64 = 1_800_000_000;
        const OLD: i64 = NOW - 40 * 86_400;
        const OLD_RESET: i64 = OLD + 604_800;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("retained.sqlite3");
        fixture(&path, 10);
        let db = Connection::open(&path).unwrap();
        db.execute_batch("DELETE FROM usage_history; DELETE FROM durable_state;
            CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,sequence INTEGER,observed_at INTEGER,
            reset_at INTEGER,window_seconds INTEGER,remaining_percent REAL,applied_generation TEXT);").unwrap();
        db.execute(
            "UPDATE collection_generation SET reset_at=?1,window_seconds=604800",
            [NOW + 604_800],
        )
        .unwrap();
        for (id, sequence, time, reset) in [
            ("old", 1, OLD + 1099 * 60, OLD_RESET),
            ("new", 2, NOW, NOW + 604_800),
        ] {
            db.execute(
                "INSERT INTO quota_receipts VALUES(?1,?2,?3,?4,604800,90,'1')",
                params![id, sequence, time, reset],
            )
            .unwrap();
        }
        for index in 0..1100i64 {
            db.execute(
                "INSERT INTO usage_history VALUES(?1,?2,90,1,0,0,?3,0,0)",
                params![OLD + index * 60, OLD_RESET, index + 1],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO usage_history VALUES(?1,?2,90,2,0,0,10,0,0)",
            params![NOW, NOW + 604_800],
        )
        .unwrap();
        let reader = DbReader::open(&path).unwrap();
        let initial_hash = reader.read_snapshot().unwrap().data_hash;
        let mut server = RestServer::start(reader.clone(), "127.0.0.1:0".parse().unwrap()).unwrap();
        refresh_serialization_fixture(&server);
        let catalog = request(
            server.local_addr(),
            "GET /v3/history/periods HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        assert!(catalog.starts_with("HTTP/1.1 200"), "{catalog}");
        let value: Value = serde_json::from_str(body(&catalog)).unwrap();
        let old_period_id = OLD_RESET.to_string();
        assert!(value["history_periods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"].as_str() == Some(old_period_id.as_str())));
        let mut cursor = None;
        let mut tokens = Vec::new();
        for _ in 0..2 {
            let suffix = cursor
                .as_ref()
                .map(|c| format!("&cursor={c}"))
                .unwrap_or_default();
            let page = request(
                server.local_addr(),
                &format!("GET /v3/history?period={OLD_RESET}{suffix} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert!(page.starts_with("HTTP/1.1 200"), "{page}");
            assert_eq!(published_pair(&page), published_pair(&catalog));
            let page: Value = serde_json::from_str(body(&page)).unwrap();
            let rows = page["history_samples"].as_array().unwrap();
            assert!(rows.len() <= 1024);
            tokens.extend(rows.iter().map(|p| {
                p["models"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|model| model["model"] == "SOL")
                    .unwrap()["total_tokens"]
                    .as_u64()
                    .unwrap()
            }));
            cursor = page["next_cursor"].as_str().map(str::to_owned);
        }
        assert!(cursor.is_none());
        assert_eq!(tokens, (1..=1100u64).collect::<Vec<_>>());
        for route in ["/v1/details", "/v2/details", "/v3/details"] {
            let response = request(
                server.local_addr(),
                &format!("GET {route} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            let details: Value = serde_json::from_str(body(&response)).unwrap();
            assert_eq!(details["history_samples"].as_array().unwrap().len(), 1);
            assert_eq!(details["history_periods"].as_array().unwrap().len(), 1);
        }
        db.execute(
            "UPDATE usage_history SET sol_tokens=999 WHERE timestamp=?1",
            [OLD],
        )
        .unwrap();
        assert_ne!(
            initial_hash,
            reader.read_snapshot().unwrap().data_hash,
            "archived facts participate in the immutable pair identity"
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM usage_history", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1101
        );
        server.shutdown();
    }

    #[test]
    fn issue646_archive_failure_preserves_recent_details_and_models() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("archive-failure.sqlite3");
        fixture(&path, 10);
        // Keep the archive failure genuine after removing the old count cap.
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,sequence INTEGER,
            observed_at INTEGER,reset_at INTEGER,window_seconds INTEGER,remaining_percent REAL,applied_generation TEXT);
            WITH RECURSIVE receipts(n) AS (SELECT 0 UNION ALL SELECT n+1 FROM receipts WHERE n<128)
            INSERT INTO quota_receipts SELECT printf('%064x', n+1),n+1,1800000000-40*86400+n*60,
                1800000000-40*86400+604800+n*60,604800,CASE WHEN n=0 THEN -1 ELSE 90 END,'1' FROM receipts;
            INSERT INTO quota_receipts VALUES('current',130,1800000000,1800000060,3600,50,'1');").unwrap();
        // This lane needs explicit confirmed model evidence; legacy columns
        // alone are history display values, not current model authority.
        db.execute_batch("CREATE TABLE session_model_totals(model TEXT PRIMARY KEY,total_tokens TEXT,
            input_tokens TEXT,cached_input_tokens TEXT,output_tokens TEXT,cache_write_input_tokens TEXT);
            INSERT INTO session_model_totals VALUES('SOL','10','7','0','3','0');
            CREATE TABLE usage_model_history(reset_at INTEGER,timestamp INTEGER,model TEXT,total_tokens TEXT,
            input_tokens TEXT,cached_input_tokens TEXT,output_tokens TEXT,cache_write_input_tokens TEXT,model_set_complete INTEGER);
            INSERT INTO usage_model_history VALUES(1800000060,1800000000,'SOL','10','7','0','3','0',1);
            UPDATE durable_state SET snapshot_json='{\"kind\":\"codex-info-usage-observation-v1\",\"timestamp\":1800000000,\"reset_at\":1800000060,\"remaining_percent\":50.0,\"model_source\":\"confirmed\"}' WHERE singleton=2;
        ").unwrap();
        let snapshot = DbReader::open(&path).unwrap().read_snapshot().unwrap();
        assert_eq!(snapshot.details.history_samples.len(), 1);
        assert_eq!(snapshot.details.history_samples[0].sol_tokens, 10);
        assert!(snapshot
            .models_v3
            .iter()
            .any(|model| model.model == "SOL" && model.total_tokens == 10));
        assert!(!snapshot.history_available);
        let published = PublishedSnapshot::from_db_for_account(snapshot, 1);
        assert!(matches!(
            serialize_route(&published, Route::HistoryPeriodsV3, None, None, false),
            Err(RouteError::HistoryUnavailable)
        ));
    }

    #[test]
    fn quota_receipt_current_reaches_http_despite_model_failure() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("quota-current.sqlite3");
        fixture(&path, 10);
        const FIXTURE_AT: i64 = 1_800_000_020;
        let mut server =
            fixed_time_serialization_fixture(DbReader::open(&path).unwrap(), FIXTURE_AT);
        let wire = "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n";
        let previous = request(server.local_addr(), wire);
        assert_eq!(
            serde_json::from_str::<Value>(body(&previous)).unwrap()["quota"]["remaining_percent"],
            50.0
        );

        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE quota_receipts(receipt_id TEXT PRIMARY KEY,
            sequence INTEGER, observed_at INTEGER, reset_at INTEGER,
            window_seconds INTEGER, remaining_percent REAL, applied_generation TEXT);
            INSERT INTO quota_receipts VALUES('official',1,1800000020,1800604800,604800,99.0,'2');
            CREATE TABLE usage_model_history(broken_column INTEGER);
            UPDATE collection_generation SET data_generation='2';",
        )
        .unwrap();
        publish_serialization_fixture_at(&server.store(), FIXTURE_AT);
        let response = request(server.local_addr(), wire);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let current: Value = serde_json::from_str(body(&response)).unwrap();
        assert_eq!(current["state"], "error");
        assert_eq!(current["authenticated"], true);
        assert_eq!(current["observed_at"], 1_800_000_020_i64);
        assert_eq!(current["quota"]["remaining_percent"], 99.0);
        assert_eq!(current["quota"]["reset_at"], 1_800_604_800_i64);
        assert_eq!(current["models"], serde_json::json!([]));
        assert_ne!(published_pair(&previous), published_pair(&response));
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
        connection.execute_batch("CREATE TABLE session_pending_ranges(source_id TEXT, range_start INTEGER, complete INTEGER); INSERT INTO session_pending_ranges VALUES('private-source-name',0,0); CREATE TABLE quota_acquisition_state(singleton INTEGER PRIMARY KEY, degraded INTEGER); INSERT INTO quota_acquisition_state VALUES(1,1); UPDATE collection_generation SET data_generation='2';").unwrap();
        let pending = request(server.local_addr(), wire);
        assert_eq!(
            serde_json::from_str::<Value>(body(&pending)).unwrap()["state"],
            "error"
        );
        connection
            .execute_batch("UPDATE quota_acquisition_state SET degraded=0; UPDATE collection_generation SET data_generation='3';")
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
            "acquisition_failed",
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
        fn published_server(accounts: Vec<AccountReader>) -> RestServer {
            const AT: i64 = 1_800_000_120;
            let mut store = SnapshotStore::new_with_accounts(accounts, "account-7")
                .expect("multi-account fixture store");
            store.publication_clock = Arc::new(|| Some(AT));
            for id in ["account-7", "account-13"] {
                let intent = store.reader_control.pending.lock().unwrap().latest[id].clone();
                let candidate = store.prepare_reader_candidate_at(id, AT);
                store.complete_reader_candidate(id, &intent, candidate);
                assert!(
                    store.snapshot_account(id).is_some(),
                    "fixture account {id} must publish before HTTP assertions"
                );
            }
            RestServer::start_with_store(store, "127.0.0.1:0".parse().unwrap())
                .expect("published multi-account fixture server")
        }

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
        let mut server = published_server(vec![
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
        ]);

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
        assert_eq!(&default_pair[3..35], &selected_pair[3..35]);
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
        let mut restarted = published_server(vec![
            AccountReader::new("account-7", 7, true, Some(1_800_000_000), None, reader_a),
            AccountReader::new("account-13", 13, false, None, None, reader_b),
        ]);
        let same_account_changed = request(
            restarted.local_addr(),
            &format!(
                "GET /v3/details?account=account-13 HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{selected_pair}\"\r\n\r\n"
            ),
        );
        assert!(same_account_changed.starts_with("HTTP/1.1 200"));
        let rewritten_pair = published_pair(&same_account_changed);
        assert_ne!(rewritten_pair, selected_pair);
        assert_ne!(&rewritten_pair[3..35], &selected_pair[3..35]);

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
        let mut store = SnapshotStore::new_with_accounts(
            vec![AccountReader::new(
                "account-7",
                7,
                true,
                Some(1_800_000_000),
                None,
                reader,
            )],
            "account-7",
        )
        .expect("account fixture store");
        store.publication_clock = Arc::new(|| Some(1_800_000_120));
        let mut server = RestServer::start_with_store(store, "127.0.0.1:0".parse().unwrap())
            .expect("account server");

        let deadline = Instant::now() + Duration::from_secs(5);
        while server.store().snapshot_account("account-7").is_none() {
            assert!(
                Instant::now() < deadline,
                "startup worker did not publish the account fixture"
            );
            thread::sleep(Duration::from_millis(1));
        }

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

        let v4 = request(
            server.local_addr(),
            "GET /v4/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let mut v4: Value = serde_json::from_str(body(&v4)).unwrap();
        assert_eq!(v4["models_status"], "unavailable");
        v4.as_object_mut().unwrap().remove("models_status");
        v4["api_version"] = json!("v3");
        assert_eq!(v4, logged_out, "v4 must clear the same account-owned data");

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
        let initial_v4: Value = serde_json::from_str(body(&get("/v4/current"))).unwrap();
        assert_eq!(initial_v4["state"], "ready");
        assert_eq!(initial_v4["models_status"], "complete");
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
        let partial_v4 = get("/v4/current");
        assert_eq!(published_pair(&partial_v4), error_pair);
        let partial_v4: Value = serde_json::from_str(body(&partial_v4)).unwrap();
        assert_eq!(partial_v4["state"], "ready");
        assert_eq!(partial_v4["models_status"], "partial");
        assert_eq!(partial_v4["models"][0]["total_tokens"], 201_416_126);
        assert_eq!(partial_v4["quota"]["remaining_percent"], 5.0);
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
            assert_eq!(row["reason"], "incomplete_current_projection");
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
        connection
            .execute_batch(
                "UPDATE usage_model_history SET total_tokens='invalid' WHERE timestamp=1800000120;
             UPDATE collection_generation SET data_generation='5';",
            )
            .unwrap();
        let invalid: Value = serde_json::from_str(body(&get("/v4/current"))).unwrap();
        assert_eq!(invalid["state"], "error");
        assert_eq!(invalid["models_status"], "unavailable");
        assert_eq!(invalid["quota"]["remaining_percent"], 98.0);
        assert_eq!(invalid["quota"]["reset_at"], 1_800_003_660);
        connection
            .execute_batch(
                "UPDATE usage_model_history SET total_tokens='440' WHERE timestamp=1800000120;
             UPDATE collection_generation SET data_generation='6';",
            )
            .unwrap();
        let repaired: Value = serde_json::from_str(body(&get("/v4/current"))).unwrap();
        assert_eq!(repaired["state"], "ready");
        assert_eq!(repaired["models_status"], "complete");
        server.shutdown();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn issue_637_acquisition_failure_and_recovery_are_independent_of_pending_history() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("usage.sqlite3");
        fixture(&path, 10);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(
            "CREATE TABLE session_pending_ranges(source_id TEXT, range_start INTEGER, complete INTEGER);
             INSERT INTO session_pending_ranges VALUES('incomplete',0,0);
             CREATE TABLE quota_acquisition_state(singleton INTEGER PRIMARY KEY, degraded INTEGER);
             INSERT INTO quota_acquisition_state VALUES(1,1);").unwrap();
        let mut server = RestServer::start(
            DbReader::open(&path).unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let routes = ["/v1/details", "/v2/details", "/v3/details", "/v3/current"];
        let failures = routes.map(|route| {
            let response = request(
                server.local_addr(),
                &format!("GET {route} HTTP/1.1\r\nHost:x\r\n\r\n"),
            );
            assert_eq!(
                serde_json::from_str::<Value>(body(&response)).unwrap()["state"],
                "error"
            );
            response
        });
        // Health can recover without a new usage generation. The error ETag
        // must not trap a client in its previous error state.
        connection
            .execute("UPDATE quota_acquisition_state SET degraded=0", [])
            .unwrap();
        for (route, failure) in routes.into_iter().zip(failures) {
            let response = request(
                server.local_addr(),
                &format!(
                    "GET {route} HTTP/1.1\r\nHost:x\r\nIf-None-Match: \"{}\"\r\n\r\n",
                    published_pair(&failure)
                ),
            );
            assert!(response.starts_with("HTTP/1.1 200"));
            let recovered: Value = serde_json::from_str(body(&response)).unwrap();
            let failed: Value = serde_json::from_str(body(&failure)).unwrap();
            assert_eq!(recovered["state"], "ready");
            assert_eq!(recovered["quota"], failed["quota"]);
            assert_eq!(recovered["models"], failed["models"]);
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM session_pending_ranges WHERE complete=0",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        server.shutdown();
    }

    #[test]
    fn issue_637_pending_history_does_not_revoke_successful_acquisition() {
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
            assert_eq!(value["state"], "ready");
            assert_eq!(value["history_samples"], previous["history_samples"]);
        }

        // The current endpoint used by the Windows footer must agree with
        // all details versions. Pending source evidence must remain durable.
        let current = request(
            server.local_addr(),
            "GET /v3/current HTTP/1.1\r\nHost:x\r\n\r\n",
        );
        let current: Value = serde_json::from_str(body(&current)).unwrap();
        assert_eq!(current["state"], "ready");
        assert_eq!(current["quota"], initial[2]["quota"]);
        assert!(server.store().snapshot().unwrap().has_pending_ranges);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM session_pending_ranges WHERE complete=0",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );

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

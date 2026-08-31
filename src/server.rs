// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! Loopback-only, read-only REST API for an already running Codex Info UI.
//!
//! This module deliberately knows nothing about Slint, Codex app-server,
//! SQLite, or local session files. The UI thread copies a whitelisted immutable
//! snapshot into [`ApiSnapshotPublisher`]; HTTP handlers only read that copy.

use crate::security;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::oneshot;

/// Environment variable that opt-ins to the local REST listener.
pub const API_LISTEN_ENV: &str = "CODEX_INFO_API_LISTEN";
pub const API_VERSION: &str = "v1";
/// Maximum number of model rows accepted at the public boundary.
pub const MAX_PUBLIC_MODELS: usize = 3;
/// SQLite retains three calendar months, while one REST details snapshot is
/// limited to one calendar month of minute buckets.
pub const MAX_PUBLIC_HISTORY_PERIODS: usize = 128;
pub const MAX_PUBLIC_HISTORY_SAMPLES: usize = 31 * 24 * 60;
pub const MAX_PUBLIC_HISTORY_GAPS: usize = 4_096;
pub const MAX_PUBLIC_THREADS: usize = 256;
const API_START_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_PUBLIC_UNIX_SECONDS: i64 = 253_402_300_799; // 9999-12-31T23:59:59Z
const MAX_PUBLIC_ID_SCALARS: usize = 512;
const MAX_PUBLIC_HISTORY_LABEL_SCALARS: usize = 512;

// These limits are the finite wire boundary from docs/REST_API_V1.md. The
// parser closes every connection after one request, so no unbounded body or
// pipeline is ever handed to the snapshot handlers.
const MAX_REQUEST_LINE_BYTES: usize = 2_048;
const MAX_METHOD_BYTES: usize = 8;
const MAX_REQUEST_HEADERS: usize = 32;
const MAX_HEADER_NAME_BYTES: usize = 64;
const MAX_HEADER_VALUE_BYTES: usize = 1_024;
const MAX_HEADER_AGGREGATE_BYTES: usize = 8 * 1_024;
const MAX_ACTIVE_CONNECTIONS: usize = 16;
const REQUEST_HEADER_DEADLINE: Duration = Duration::from_secs(3);
const REQUEST_READ_POLL: Duration = Duration::from_millis(100);

/// The public availability of the monitor data. No error detail is exported.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicState {
    #[default]
    Initializing,
    Ready,
    AuthRequired,
    Error,
}

/// Quota values safe for the intranet monitoring client.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicQuota {
    pub remaining_percent: f64,
    pub reset_at: i64,
    pub window_seconds: i64,
    pub monthly: bool,
}

/// Per-model usage values published by `/v1/details`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicDetailedModelUsage {
    pub name: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub input_dollars: f64,
    pub cached_input_dollars: f64,
    pub output_dollars: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicHistoryPeriod {
    pub id: String,
    pub start_at: i64,
    pub end_at: i64,
    /// Canonical quota-reset boundary. `end_at` may be clipped when a newer
    /// reset period begins before this boundary, so clients must not infer
    /// sample ownership from the graph end.
    pub reset_at: i64,
    pub label: String,
    pub current: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicHistorySample {
    pub timestamp: i64,
    pub reset_at: i64,
    /// `null` means the local session backfill had no quota observation.
    pub remaining_percent: Option<f64>,
    pub sol_dollars: f64,
    pub terra_dollars: f64,
    pub luna_dollars: f64,
    pub sol_tokens: u64,
    pub terra_tokens: u64,
    pub luna_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicHistoryGap {
    pub gap_id: String,
    pub reset_at: i64,
    pub start_at: i64,
    pub end_at: i64,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicThread {
    pub id: String,
    pub title: String,
    pub parent_thread_id: Option<String>,
    pub model: String,
    pub model_label: String,
    pub total_tokens: Option<u64>,
    pub context_usage_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub created_at: Option<i64>,
    pub last_user_message_at: Option<i64>,
    pub is_subagent: bool,
    pub depth: Option<i32>,
}

/// The sole immutable data document that may cross the REST trust boundary.
/// Do not add account email, authentication URLs, filesystem locations, raw
/// backend errors, or secrets to this type.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicDetails {
    pub state: PublicState,
    pub observed_at: Option<i64>,
    pub authenticated: bool,
    pub plan_label: Option<String>,
    pub quota: Option<PublicQuota>,
    pub models: Vec<PublicDetailedModelUsage>,
    pub active_thread_count: u64,
    pub history_periods: Vec<PublicHistoryPeriod>,
    pub history_samples: Vec<PublicHistorySample>,
    pub history_gaps: Vec<PublicHistoryGap>,
    pub threads: Vec<PublicThread>,
    pub estimated_cost_label: String,
}

impl Default for PublicDetails {
    fn default() -> Self {
        Self {
            state: PublicState::Initializing,
            observed_at: None,
            authenticated: false,
            plan_label: None,
            quota: None,
            models: Vec::new(),
            active_thread_count: 0,
            history_periods: Vec::new(),
            history_samples: Vec::new(),
            history_gaps: Vec::new(),
            threads: Vec::new(),
            estimated_cost_label: "概算 —".to_owned(),
        }
    }
}

impl PublicDetails {
    pub fn validate(&self) -> Result<(), ApiSnapshotError> {
        if self
            .observed_at
            .is_some_and(|timestamp| !valid_timestamp(timestamp))
        {
            return Err(ApiSnapshotError::InvalidObservedAt);
        }
        if let Some(quota) = self.quota.as_ref() {
            if !quota.remaining_percent.is_finite()
                || !(0.0..=100.0).contains(&quota.remaining_percent)
                || !valid_timestamp(quota.reset_at)
                || quota.window_seconds <= 0
            {
                return Err(ApiSnapshotError::InvalidQuota);
            }
        }
        if self
            .plan_label
            .as_deref()
            .is_some_and(|label| !valid_text(label, security::MAX_PLAN_SCALARS))
        {
            return Err(ApiSnapshotError::InvalidLabel);
        }
        if self.history_periods.len() > MAX_PUBLIC_HISTORY_PERIODS {
            return Err(ApiSnapshotError::ListTooLong);
        }
        if self.history_samples.len() > MAX_PUBLIC_HISTORY_SAMPLES {
            return Err(ApiSnapshotError::ListTooLong);
        }
        if self.history_gaps.len() > MAX_PUBLIC_HISTORY_GAPS {
            return Err(ApiSnapshotError::ListTooLong);
        }
        if self.models.len() > MAX_PUBLIC_MODELS {
            return Err(ApiSnapshotError::ListTooLong);
        }

        let mut model_names = HashSet::with_capacity(self.models.len());
        for model in &self.models {
            if !matches!(model.name.as_str(), "SOL" | "TERRA" | "LUNA")
                || !model_names.insert(model.name.as_str())
                || !valid_non_negative_rate(model.input_dollars)
                || !valid_non_negative_rate(model.cached_input_dollars)
                || !valid_non_negative_rate(model.output_dollars)
            {
                return Err(ApiSnapshotError::InvalidModel);
            }
        }

        let mut period_ids = HashSet::with_capacity(self.history_periods.len());
        let mut period_resets = HashSet::with_capacity(self.history_periods.len());
        let mut current_periods = 0usize;
        for period in &self.history_periods {
            if !valid_text(&period.id, MAX_PUBLIC_ID_SCALARS)
                || !period_ids.insert(period.id.as_str())
                || !period_resets.insert(period.reset_at)
                || !valid_timestamp(period.start_at)
                || !valid_timestamp(period.end_at)
                || !valid_timestamp(period.reset_at)
                || period.end_at < period.start_at
                || period.reset_at < period.end_at
                || !valid_text(&period.label, MAX_PUBLIC_HISTORY_LABEL_SCALARS)
                || period.label.is_empty()
            {
                return Err(ApiSnapshotError::InvalidHistoryPeriod);
            }
            if period.current {
                current_periods = current_periods.saturating_add(1);
            }
        }
        if current_periods > 1 {
            return Err(ApiSnapshotError::InvalidHistoryPeriod);
        }

        for period in &self.history_periods {
            if period.current {
                let Some(observed_at) = self.observed_at else {
                    return Err(ApiSnapshotError::InvalidHistoryPeriod);
                };
                if period.end_at != period.reset_at.min(observed_at) {
                    return Err(ApiSnapshotError::InvalidHistoryPeriod);
                }
            }
        }

        let mut sample_ids = HashSet::with_capacity(self.history_samples.len());
        let mut canonical_sample_ids = HashSet::with_capacity(self.history_samples.len());
        let mut previous_sample_key = None;
        for sample in &self.history_samples {
            if !valid_timestamp(sample.timestamp)
                || sample.timestamp.rem_euclid(60) != 0
                || !valid_timestamp(sample.reset_at)
                || self
                    .observed_at
                    .is_none_or(|observed_at| sample.timestamp > observed_at)
                || sample
                    .remaining_percent
                    .is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
                || !valid_non_negative_rate(sample.sol_dollars)
                || !valid_non_negative_rate(sample.terra_dollars)
                || !valid_non_negative_rate(sample.luna_dollars)
            {
                return Err(ApiSnapshotError::InvalidHistorySample);
            }
            if !sample_ids.insert((sample.reset_at, sample.timestamp)) {
                return Err(ApiSnapshotError::InvalidHistorySample);
            }
            let matching_periods = self
                .history_periods
                .iter()
                .filter(|period| {
                    sample.reset_at >= period.reset_at.saturating_sub(60)
                        && sample.reset_at <= period.reset_at
                        && sample.timestamp >= period.start_at
                        && sample.timestamp <= period.end_at
                })
                .collect::<Vec<_>>();
            if matching_periods.len() != 1 {
                return Err(ApiSnapshotError::InvalidHistorySample);
            }
            let period = matching_periods[0];
            if !canonical_sample_ids.insert((period.id.as_str(), sample.timestamp)) {
                return Err(ApiSnapshotError::InvalidHistorySample);
            }
            let sample_key = (sample.reset_at, sample.timestamp);
            if previous_sample_key.is_some_and(|previous| previous > sample_key) {
                return Err(ApiSnapshotError::InvalidHistorySample);
            }
            previous_sample_key = Some(sample_key);
        }

        let mut gap_ids = HashSet::with_capacity(self.history_gaps.len());
        let mut previous_gap_key = None;
        let mut gap_ranges = Vec::with_capacity(self.history_gaps.len());
        for gap in &self.history_gaps {
            if !valid_lower_hex32(&gap.gap_id)
                || !gap_ids.insert(gap.gap_id.as_str())
                || !valid_timestamp(gap.reset_at)
                || !valid_timestamp(gap.start_at)
                || !valid_timestamp(gap.end_at)
                || gap.start_at > gap.end_at
                || !matches!(
                    gap.reason.as_str(),
                    "daemon_stop_unrecoverable" | "reset_hint_expired" | "auth_epoch_tombstoned"
                )
            {
                return Err(ApiSnapshotError::InvalidHistoryGap);
            }
            let matching_periods = self
                .history_periods
                .iter()
                .filter(|period| {
                    gap.reset_at >= period.reset_at.saturating_sub(60)
                        && gap.reset_at <= period.reset_at
                        && gap.start_at >= period.start_at
                        && gap.end_at <= period.end_at
                })
                .collect::<Vec<_>>();
            if matching_periods.len() != 1 {
                return Err(ApiSnapshotError::InvalidHistoryGap);
            }
            let period = matching_periods[0];
            let gap_key = (gap.reset_at, gap.start_at, gap.end_at, gap.gap_id.as_str());
            if previous_gap_key.is_some_and(|previous| previous > gap_key) {
                return Err(ApiSnapshotError::InvalidHistoryGap);
            }
            previous_gap_key = Some(gap_key);
            if gap_ranges.iter().any(|(period_id, start_at, end_at)| {
                period_id == &period.id && gap.start_at <= *end_at && *start_at <= gap.end_at
            }) {
                return Err(ApiSnapshotError::InvalidHistoryGap);
            }
            gap_ranges.push((period.id.clone(), gap.start_at, gap.end_at));
        }

        validate_public_threads(&self.threads)?;
        if !valid_text(&self.estimated_cost_label, security::MAX_STATUS_SCALARS)
            || self.estimated_cost_label.is_empty()
        {
            return Err(ApiSnapshotError::InvalidLabel);
        }
        Ok(())
    }
}

/// Validate the complete bounded thread slice before it can cross any public
/// or local presentation boundary.  This is the single owner for the REST
/// thread schema/domain, capacity, and duplicate rules; callers must not
/// reproduce these checks.
/// Public because the binary target owns the local state/admission path while
/// this module belongs to the library target; the implementation remains the
/// sole server-side thread validation owner.
pub fn validate_public_threads(threads: &[PublicThread]) -> Result<(), ApiSnapshotError> {
    if threads.len() > MAX_PUBLIC_THREADS {
        return Err(ApiSnapshotError::ListTooLong);
    }

    let mut thread_ids = HashSet::with_capacity(threads.len());
    for thread in threads {
        if !valid_text(&thread.id, MAX_PUBLIC_ID_SCALARS)
            || !thread_ids.insert(thread.id.as_str())
            || !valid_text(&thread.title, security::MAX_THREAD_TITLE_SCALARS)
            || thread.title.is_empty()
            || !valid_text(&thread.model, security::MAX_MODEL_SCALARS)
            || thread.model.is_empty()
            || !valid_text(
                &thread.model_label,
                security::MAX_ACCOUNT_ACTIVITY_LABEL_SCALARS,
            )
            || thread.model_label.is_empty()
            || !thread
                .parent_thread_id
                .as_deref()
                .is_none_or(|id| valid_text(id, MAX_PUBLIC_ID_SCALARS) && !id.is_empty())
            || !thread.created_at.is_none_or(valid_timestamp)
            || !thread.last_user_message_at.is_none_or(valid_timestamp)
            || !thread.depth.is_none_or(|depth| (0..=1024).contains(&depth))
        {
            return Err(ApiSnapshotError::InvalidThread);
        }
    }
    Ok(())
}

fn valid_timestamp(value: i64) -> bool {
    (1..=MAX_PUBLIC_UNIX_SECONDS).contains(&value)
}

fn valid_non_negative_rate(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn valid_text(value: &str, max_scalars: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= max_scalars
        && !value
            .chars()
            .any(|character| character.is_control() || is_bidi_formatting(character))
}

fn is_bidi_formatting(value: char) -> bool {
    matches!(
        value,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn valid_lower_hex32(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A redacted validation error for data that would leave the process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiSnapshotError {
    InvalidObservedAt,
    InvalidQuota,
    InvalidModel,
    InvalidLabel,
    InvalidHistoryPeriod,
    InvalidHistorySample,
    InvalidHistoryGap,
    InvalidThread,
    ListTooLong,
    Serialization,
    PublishedPairGenerationFailed,
}

impl fmt::Display for ApiSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidObservedAt => "public snapshot has an invalid observation time",
            Self::InvalidQuota => "public snapshot has an invalid quota",
            Self::InvalidModel => "public snapshot has an invalid model",
            Self::InvalidLabel => "public snapshot has an invalid label",
            Self::InvalidHistoryPeriod => "public snapshot has an invalid history period",
            Self::InvalidHistorySample => "public snapshot has an invalid history sample",
            Self::InvalidHistoryGap => "public snapshot has an invalid history gap",
            Self::InvalidThread => "public snapshot has an invalid thread",
            Self::ListTooLong => "public snapshot has too many rows",
            Self::Serialization => "public snapshot could not be serialized",
            Self::PublishedPairGenerationFailed => {
                "published pair generation is permanently unavailable"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ApiSnapshotError {}

#[derive(Serialize)]
struct DetailsResponse<'a> {
    api_version: &'static str,
    #[serde(flatten)]
    details: &'a PublicDetails,
}

fn serialize_details(details: &PublicDetails) -> Result<Vec<u8>, ApiSnapshotError> {
    serde_json::to_vec(&DetailsResponse {
        api_version: API_VERSION,
        details,
    })
    .map_err(|_| ApiSnapshotError::Serialization)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedPair {
    identity: String,
}

impl PublishedPair {
    pub fn as_str(&self) -> &str {
        &self.identity
    }

    pub fn identity(&self) -> &str {
        self.as_str()
    }

    fn from_epoch_counter(epoch: [u8; 16], counter: u128) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut identity = String::with_capacity(67);
        identity.push_str("v1:");
        for byte in epoch {
            identity.push(HEX[(byte >> 4) as usize] as char);
            identity.push(HEX[(byte & 0x0f) as usize] as char);
        }
        for shift in (0..32).rev() {
            let nibble = ((counter >> (shift * 4)) & 0x0f) as usize;
            identity.push(HEX[nibble] as char);
        }
        Self { identity }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PublishedPairGenerationState {
    Uninitialized,
    Active { epoch: [u8; 16], counter: u128 },
    PermanentFailed,
}

#[derive(Clone, Debug, PartialEq)]
struct PublishedSnapshot {
    details_body: Vec<u8>,
    pair: Option<PublishedPair>,
    generation: PublishedPairGenerationState,
}

impl PublishedSnapshot {
    fn with_unpublished_epoch(epoch: [u8; 16]) -> Self {
        Self {
            generation: PublishedPairGenerationState::Active { epoch, counter: 0 },
            ..Self::default()
        }
    }

    fn with_initial_pair(epoch: [u8; 16]) -> Self {
        let mut snapshot = Self::with_unpublished_epoch(epoch);
        snapshot.pair = Some(PublishedPair::from_epoch_counter(epoch, 1));
        snapshot.generation = PublishedPairGenerationState::Active { epoch, counter: 1 };
        snapshot
    }
}

impl Default for PublishedSnapshot {
    fn default() -> Self {
        let details = PublicDetails::default();
        details
            .validate()
            .expect("default details must validate before publication");
        Self {
            details_body: serialize_details(&details).expect("default details must serialize"),
            pair: None,
            generation: PublishedPairGenerationState::Uninitialized,
        }
    }
}

type SharedSnapshot = Arc<RwLock<PublishedSnapshot>>;

/// Cloneable one-way publication handle held by the UI thread.
#[derive(Clone)]
pub struct ApiSnapshotPublisher {
    snapshot: SharedSnapshot,
}

impl ApiSnapshotPublisher {
    #[cfg(test)]
    fn with_unpublished_epoch_for_test(epoch: [u8; 16]) -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(PublishedSnapshot::with_unpublished_epoch(
                epoch,
            ))),
        }
    }

    pub fn published_pair(&self) -> Option<PublishedPair> {
        let current = self
            .snapshot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        current.pair.clone()
    }

    /// Atomically publishes one completely validated details generation.
    pub fn publish_details(&self, details: PublicDetails) -> Result<(), ApiSnapshotError> {
        details.validate()?;
        let details_body = serialize_details(&details)?;
        self.publish_serialized(details_body).map(|_| ())
    }

    #[cfg(test)]
    fn publish_for_test(&self, details: PublicDetails) -> Result<PublishedPair, ApiSnapshotError> {
        details.validate()?;
        let details_body = serialize_details(&details)?;
        self.publish_serialized(details_body)
    }

    fn publish_serialized(&self, details_body: Vec<u8>) -> Result<PublishedPair, ApiSnapshotError> {
        let mut current = self
            .snapshot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (epoch, next_counter) = match &current.generation {
            PublishedPairGenerationState::Active { epoch, counter } => {
                let Some(next_counter) = counter.checked_add(1) else {
                    current.generation = PublishedPairGenerationState::PermanentFailed;
                    return Err(ApiSnapshotError::PublishedPairGenerationFailed);
                };
                (*epoch, next_counter)
            }
            PublishedPairGenerationState::Uninitialized
            | PublishedPairGenerationState::PermanentFailed => {
                return Err(ApiSnapshotError::PublishedPairGenerationFailed);
            }
        };
        let pair = PublishedPair::from_epoch_counter(epoch, next_counter);
        *current = PublishedSnapshot {
            details_body,
            pair: Some(pair.clone()),
            generation: PublishedPairGenerationState::Active {
                epoch,
                counter: next_counter,
            },
        };
        Ok(pair)
    }
}

/// A listener configuration that can never represent a LAN or public bind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiServerConfig {
    listen_addr: SocketAddr,
}

impl ApiServerConfig {
    pub fn new(listen_addr: SocketAddr) -> Result<Self, ApiServerError> {
        if !is_loopback(listen_addr.ip()) {
            return Err(ApiServerError::NonLoopbackAddress);
        }
        Ok(Self { listen_addr })
    }

    /// Parses the opt-in listener. An unset variable keeps the API disabled.
    pub fn from_environment() -> Result<Option<Self>, ApiServerError> {
        let Some(value) = env::var_os(API_LISTEN_ENV) else {
            return Ok(None);
        };
        let value = value
            .to_str()
            .ok_or(ApiServerError::InvalidListenConfiguration)?;
        let address = value
            .parse::<SocketAddr>()
            .map_err(|_| ApiServerError::InvalidListenConfiguration)?;
        Self::new(address).map(Some)
    }

    pub const fn listen_addr(self) -> SocketAddr {
        self.listen_addr
    }
}

fn is_loopback(address: IpAddr) -> bool {
    address.is_loopback()
}

fn server_epoch_from_source<F>(source: F) -> Result<[u8; 16], ApiServerError>
where
    F: FnOnce(&mut [u8; 16]) -> Result<(), ()>,
{
    let mut epoch = [0_u8; 16];
    source(&mut epoch).map_err(|_| ApiServerError::EntropyUnavailable)?;
    if epoch == [0_u8; 16] {
        return Err(ApiServerError::EntropyUnavailable);
    }
    Ok(epoch)
}

fn random_server_epoch() -> Result<[u8; 16], ApiServerError> {
    server_epoch_from_source(|epoch| getrandom::fill(epoch).map_err(|_| ()))
}

/// Redacted errors for starting the optional API listener.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiServerError {
    InvalidListenConfiguration,
    NonLoopbackAddress,
    EntropyUnavailable,
    BindFailed,
    RuntimeFailed,
    WorkerStartFailed,
}

impl fmt::Display for ApiServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidListenConfiguration => "API listen configuration is invalid",
            Self::NonLoopbackAddress => "API listener must use a loopback address",
            Self::EntropyUnavailable => "API listener entropy source unavailable",
            Self::BindFailed => "API listener could not bind safely",
            Self::RuntimeFailed => "API runtime could not start",
            Self::WorkerStartFailed => "API worker could not start",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ApiServerError {}

/// An optional API worker. Dropping it closes the listener and joins its
/// thread; it owns no Codex child process and never accesses UI state.
pub struct ApiServer {
    publisher: ApiSnapshotPublisher,
    local_addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl ApiServer {
    pub fn from_environment() -> Result<Option<Self>, ApiServerError> {
        ApiServerConfig::from_environment()?
            .map(Self::start)
            .transpose()
    }

    pub fn start(config: ApiServerConfig) -> Result<Self, ApiServerError> {
        let epoch = random_server_epoch()?;
        Self::start_with_epoch(config, epoch)
    }

    fn start_with_epoch(config: ApiServerConfig, epoch: [u8; 16]) -> Result<Self, ApiServerError> {
        let listener =
            TcpListener::bind(config.listen_addr).map_err(|_| ApiServerError::BindFailed)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| ApiServerError::BindFailed)?;
        let local_addr = listener
            .local_addr()
            .map_err(|_| ApiServerError::BindFailed)?;
        let publisher = ApiSnapshotPublisher {
            snapshot: Arc::new(RwLock::new(PublishedSnapshot::with_initial_pair(epoch))),
        };
        let snapshot = Arc::clone(&publisher.snapshot);
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let (started, started_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("codex-info-api".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let _ = started.send(Err(ApiServerError::RuntimeFailed));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match TokioTcpListener::from_std(listener) {
                        Ok(listener) => listener,
                        Err(_) => {
                            let _ = started.send(Err(ApiServerError::RuntimeFailed));
                            return;
                        }
                    };
                    if started.send(Ok(())).is_err() {
                        return;
                    }
                    serve_listener(
                        listener,
                        snapshot,
                        authority_for(local_addr),
                        shutdown_receiver,
                    )
                    .await;
                });
            })
            .map_err(|_| ApiServerError::WorkerStartFailed)?;

        match started_receiver.recv_timeout(API_START_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                publisher,
                local_addr,
                shutdown: Some(shutdown),
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = shutdown.send(());
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = shutdown.send(());
                let _ = worker.join();
                Err(ApiServerError::WorkerStartFailed)
            }
        }
    }

    #[cfg(test)]
    fn start_with_epoch_source<F>(
        config: ApiServerConfig,
        source: F,
    ) -> Result<Self, ApiServerError>
    where
        F: FnOnce(&mut [u8; 16]) -> Result<(), ()>,
    {
        let epoch = server_epoch_from_source(source)?;
        Self::start_with_epoch(config, epoch)
    }

    pub fn publisher(&self) -> ApiSnapshotPublisher {
        self.publisher.clone()
    }

    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stops the worker and releases the loopback port. Calling it more than
    /// once is harmless.
    pub fn shutdown(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Serialize)]
struct HealthResponse {
    api_version: &'static str,
    service: &'static str,
}

#[derive(Serialize)]
struct ErrorResponse {
    api_version: &'static str,
    error: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApiRoute {
    Health,
    Details,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParseFailure {
    BadRequest,
    HeadersTooLarge,
}

#[derive(Debug)]
enum HeaderRead {
    Complete {
        data: Vec<u8>,
        line_end: usize,
        terminator: usize,
        trailing_data: bool,
    },
    Timeout,
    BadRequest,
    HeadersTooLarge,
    Closed,
}

#[derive(Debug)]
struct ParsedRequest {
    route: Option<ApiRoute>,
    is_get: bool,
    body_not_allowed: bool,
}

fn authority_for(address: SocketAddr) -> String {
    match address {
        SocketAddr::V4(address) => format!("{}:{}", address.ip(), address.port()),
        SocketAddr::V6(address) => format!("[{}]:{}", address.ip(), address.port()),
    }
}

/// Accept at most one bounded HTTP/1.1 request on each socket. A small
/// blocking worker per admitted socket keeps the parser independent from
/// axum/hyper's permissive connection lifecycle while the listener itself
/// remains asynchronous and shutdown-aware.
async fn serve_listener(
    listener: TokioTcpListener,
    snapshot: SharedSnapshot,
    authority: String,
    mut shutdown_receiver: oneshot::Receiver<()>,
) {
    let shutdown = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let mut workers: Vec<JoinHandle<()>> = Vec::new();

    loop {
        tokio::select! {
            _ = &mut shutdown_receiver => break,
            accepted = listener.accept() => {
                let Ok((stream, _peer)) = accepted else {
                    continue;
                };
                let Ok(stream) = stream.into_std() else {
                    continue;
                };
                let _ = stream.set_nonblocking(false);

                if !try_admit_connection(&active) {
                    let mut stream = stream;
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    write_error_response(&mut stream, 429, "too_many_requests");
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }

                // Completed joins are reaped before retaining another handle,
                // preventing a sequential client from growing this vector.
                let mut worker_index = 0;
                while worker_index < workers.len() {
                    if workers[worker_index].is_finished() {
                        let worker = workers.swap_remove(worker_index);
                        let _ = worker.join();
                    } else {
                        worker_index += 1;
                    }
                }

                let worker_shutdown = Arc::clone(&shutdown);
                let worker_active = Arc::clone(&active);
                let worker_snapshot = Arc::clone(&snapshot);
                let worker_authority = authority.clone();
                workers.push(thread::spawn(move || {
                    handle_connection(
                        stream,
                        worker_snapshot,
                        worker_authority,
                        worker_shutdown,
                    );
                    worker_active.fetch_sub(1, Ordering::AcqRel);
                }));
            }
        }
    }

    // Stop new admissions first, then give already accepted sockets the
    // documented finite drain window. Their read deadline is also bounded by
    // three seconds, so joining does not leave an orphaned listener worker.
    shutdown.store(true, Ordering::Release);
    for worker in workers {
        let _ = worker.join();
    }
}

fn try_admit_connection(active: &AtomicUsize) -> bool {
    let mut current = active.load(Ordering::Acquire);
    loop {
        if current >= MAX_ACTIVE_CONNECTIONS {
            return false;
        }
        match active.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(next) => current = next,
        }
    }
}

type HttpResponse = (u16, Vec<u8>, Option<PublishedPair>);

fn snapshot_response(snapshot: &SharedSnapshot, route: ApiRoute) -> HttpResponse {
    let current = snapshot
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(pair) = current.pair.clone() else {
        return (503, error_body("snapshot_unavailable"), None);
    };
    if !matches!(
        current.generation,
        PublishedPairGenerationState::Active { .. }
    ) {
        return (503, error_body("snapshot_unavailable"), None);
    }
    let body = match route {
        ApiRoute::Details => current.details_body.clone(),
        ApiRoute::Health => return (503, error_body("snapshot_unavailable"), None),
    };
    (200, body, Some(pair))
}

fn handle_connection(
    mut stream: TcpStream,
    snapshot: SharedSnapshot,
    authority: String,
    shutdown: Arc<AtomicBool>,
) {
    let _ = stream.set_read_timeout(Some(REQUEST_READ_POLL));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));

    if shutdown.load(Ordering::Acquire) {
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }

    let response = match read_request_headers(&mut stream, &shutdown) {
        HeaderRead::Complete {
            data,
            line_end,
            terminator,
            trailing_data,
        } => match parse_request(&data, line_end, terminator, trailing_data, &authority) {
            Ok(request) => match request.route {
                None => (404, error_body("not_found"), None),
                Some(_) if !request.is_get => (405, error_body("method_not_allowed"), None),
                Some(_) if request.body_not_allowed => {
                    (413, error_body("request_body_not_allowed"), None)
                }
                Some(ApiRoute::Health) => (200, health_body(), None),
                Some(ApiRoute::Details) => snapshot_response(&snapshot, ApiRoute::Details),
            },
            Err(ParseFailure::BadRequest) => (400, error_body("bad_request"), None),
            Err(ParseFailure::HeadersTooLarge) => {
                (431, error_body("request_headers_too_large"), None)
            }
        },
        HeaderRead::Timeout => (408, error_body("request_timeout"), None),
        HeaderRead::BadRequest => (400, error_body("bad_request"), None),
        HeaderRead::HeadersTooLarge => (431, error_body("request_headers_too_large"), None),
        HeaderRead::Closed => {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    };

    write_json_response(&mut stream, response.0, &response.1, response.2.as_ref());
    let _ = stream.shutdown(Shutdown::Both);
}

fn read_request_headers(stream: &mut TcpStream, shutdown: &AtomicBool) -> HeaderRead {
    let started = Instant::now();
    let mut data = Vec::with_capacity(4 * 1_024);

    loop {
        if shutdown.load(Ordering::Acquire) {
            return HeaderRead::Closed;
        }
        if started.elapsed() >= REQUEST_HEADER_DEADLINE {
            return HeaderRead::Timeout;
        }

        let mut chunk = [0_u8; 1_024];
        match stream.read(&mut chunk) {
            Ok(0) => return HeaderRead::Closed,
            Ok(count) => {
                data.extend_from_slice(&chunk[..count]);
                if let Some(terminator) = find_bytes(&data, b"\r\n\r\n") {
                    let Some(line_end) = find_bytes(&data, b"\r\n") else {
                        return HeaderRead::BadRequest;
                    };
                    if line_end.saturating_add(2) > MAX_REQUEST_LINE_BYTES {
                        return HeaderRead::BadRequest;
                    }
                    let header_start = line_end + 2;
                    let header_aggregate =
                        terminator.saturating_add(2).saturating_sub(header_start);
                    if header_aggregate > MAX_HEADER_AGGREGATE_BYTES {
                        return HeaderRead::HeadersTooLarge;
                    }
                    let header_end = terminator + 4;
                    if invalid_header_line_endings(&data[..header_end]) {
                        return HeaderRead::BadRequest;
                    }
                    return HeaderRead::Complete {
                        trailing_data: data.len() > header_end,
                        data,
                        line_end,
                        terminator,
                    };
                }

                if find_bytes(&data, b"\r\n").is_none() {
                    if data.len() > MAX_REQUEST_LINE_BYTES {
                        return HeaderRead::BadRequest;
                    }
                } else if data.len() > MAX_REQUEST_LINE_BYTES + MAX_HEADER_AGGREGATE_BYTES + 4 {
                    return HeaderRead::HeadersTooLarge;
                }
                if data
                    .windows(2)
                    .any(|window| window[1] == b'\n' && window[0] != b'\r')
                    || data.first() == Some(&b'\n')
                {
                    return HeaderRead::BadRequest;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return HeaderRead::Closed,
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn invalid_header_line_endings(data: &[u8]) -> bool {
    data.iter().enumerate().any(|(index, byte)| {
        (*byte == b'\n' && (index == 0 || data[index - 1] != b'\r'))
            || (*byte == b'\r' && data.get(index + 1) != Some(&b'\n'))
    })
}

fn parse_request(
    data: &[u8],
    line_end: usize,
    terminator: usize,
    trailing_data: bool,
    authority: &str,
) -> Result<ParsedRequest, ParseFailure> {
    let request_line = &data[..line_end];
    let mut fields = request_line.split(|byte| *byte == b' ');
    let method = fields.next().ok_or(ParseFailure::BadRequest)?;
    let target = fields.next().ok_or(ParseFailure::BadRequest)?;
    let version = fields.next().ok_or(ParseFailure::BadRequest)?;
    if fields.next().is_some()
        || method.is_empty()
        || method.len() > MAX_METHOD_BYTES
        || !is_http_token(method)
        || version != b"HTTP/1.1"
        || target.is_empty()
        || target.iter().any(|byte| !(0x21..=0x7e).contains(byte))
    {
        return Err(ParseFailure::BadRequest);
    }

    let route = classify_target(target)?;
    let mut seen = HashSet::new();
    let mut host = None;
    let mut content_length = None;
    let mut transfer_encoding = false;
    let mut disallowed_header = false;
    let header_section = &data[line_end + 2..terminator];
    let mut cursor = 0;
    let mut header_count = 0usize;
    while cursor < header_section.len() {
        let line = match find_bytes(&header_section[cursor..], b"\r\n") {
            Some(offset) => {
                let line = &header_section[cursor..cursor + offset];
                cursor += offset + 2;
                line
            }
            None => {
                let line = &header_section[cursor..];
                cursor = header_section.len();
                line
            }
        };
        if line.is_empty() || matches!(line.first(), Some(b' ' | b'\t')) {
            return Err(ParseFailure::BadRequest);
        }
        header_count += 1;
        if header_count > MAX_REQUEST_HEADERS {
            return Err(ParseFailure::HeadersTooLarge);
        }
        let colon = line
            .iter()
            .position(|byte| *byte == b':')
            .ok_or(ParseFailure::BadRequest)?;
        let name = &line[..colon];
        let raw_value = &line[colon + 1..];
        if name.len() > MAX_HEADER_NAME_BYTES || raw_value.len() > MAX_HEADER_VALUE_BYTES {
            return Err(ParseFailure::HeadersTooLarge);
        }
        if !is_http_token(name) {
            return Err(ParseFailure::BadRequest);
        }
        let name = std::str::from_utf8(name)
            .map_err(|_| ParseFailure::BadRequest)?
            .to_ascii_lowercase();
        if !seen.insert(name.clone()) {
            return Err(ParseFailure::BadRequest);
        }
        let value = trim_ows(raw_value);
        let value = std::str::from_utf8(value).map_err(|_| ParseFailure::BadRequest)?;
        if value.chars().any(char::is_control) {
            return Err(ParseFailure::BadRequest);
        }

        match name.as_str() {
            "host" => host = Some(value.to_owned()),
            "accept" | "user-agent" => {}
            "connection" => {
                if value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
                {
                    return Err(ParseFailure::BadRequest);
                }
            }
            "content-length" => {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(ParseFailure::BadRequest);
                }
                let length = value.parse::<u64>().map_err(|_| ParseFailure::BadRequest)?;
                content_length = Some(length);
            }
            "transfer-encoding" => transfer_encoding = true,
            _ => disallowed_header = true,
        }
    }

    if host.as_deref() != Some(authority) {
        return Err(ParseFailure::BadRequest);
    }
    if disallowed_header {
        return Err(ParseFailure::BadRequest);
    }

    Ok(ParsedRequest {
        route,
        is_get: method == b"GET",
        body_not_allowed: trailing_data
            || transfer_encoding
            || content_length.is_some_and(|length| length != 0),
    })
}

fn classify_target(target: &[u8]) -> Result<Option<ApiRoute>, ParseFailure> {
    if !target.starts_with(b"/") || target.starts_with(b"//") {
        return Err(ParseFailure::BadRequest);
    }
    Ok(match target {
        b"/v1/health" => Some(ApiRoute::Health),
        b"/v1/details" => Some(ApiRoute::Details),
        _ => None,
    })
}

fn is_http_token(value: &[u8]) -> bool {
    !value.is_empty()
        && value.iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn trim_ows(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(start, |index| index + 1);
    &value[start..end]
}

fn health_body() -> Vec<u8> {
    serde_json::to_vec(&HealthResponse {
        api_version: API_VERSION,
        service: "codex-info",
    })
    .expect("fixed health response must serialize")
}

fn error_body(error: &'static str) -> Vec<u8> {
    serde_json::to_vec(&ErrorResponse {
        api_version: API_VERSION,
        error,
    })
    .expect("fixed error response must serialize")
}

fn write_error_response(stream: &mut TcpStream, status: u16, error: &'static str) {
    let body = error_body(error);
    write_json_response(stream, status, &body, None);
}

fn write_json_response(
    stream: &mut TcpStream,
    status: u16,
    body: &[u8],
    pair: Option<&PublishedPair>,
) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let pair_header = if status == 200 {
        pair.map_or_else(String::new, |pair| {
            format!("Codex-Info-Published-Pair: {}\r\n", pair.as_str())
        })
    } else {
        String::new()
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n{pair_header}content-type: application/json; charset=utf-8\r\ncache-control: no-store\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    fn environment_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn api_server_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn loopback_config() -> ApiServerConfig {
        ApiServerConfig::new(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap()
    }

    #[test]
    fn production_server_starts_with_initial_pair_before_first_publish() {
        let epoch = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let mut server = ApiServer::start_with_epoch_source(loopback_config(), |target| {
            target.copy_from_slice(&epoch);
            Ok(())
        })
        .unwrap();
        let publisher = server.publisher();
        assert_eq!(
            publisher.published_pair().unwrap().as_str(),
            "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001"
        );
        publisher.publish_details(PublicDetails::default()).unwrap();
        assert_eq!(
            publisher.published_pair().unwrap().as_str(),
            "v1:00112233445566778899aabbccddeeff00000000000000000000000000000002"
        );
        server.shutdown();
    }

    #[test]
    fn published_pair_generation_matches_fixed_vectors() {
        let epoch = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let publisher = ApiSnapshotPublisher::with_unpublished_epoch_for_test(epoch);
        publisher.publish_details(PublicDetails::default()).unwrap();
        assert_eq!(
            publisher.published_pair().unwrap().as_str(),
            "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001"
        );
        publisher.publish_details(PublicDetails::default()).unwrap();
        assert_eq!(
            publisher.published_pair().unwrap().as_str(),
            "v1:00112233445566778899aabbccddeeff00000000000000000000000000000002"
        );

        let epoch = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xf0,
        ];
        let publisher = ApiSnapshotPublisher::with_unpublished_epoch_for_test(epoch);
        publisher.publish_details(PublicDetails::default()).unwrap();
        assert_eq!(
            publisher.published_pair().unwrap().as_str(),
            "v1:00112233445566778899aabbccddeef000000000000000000000000000000001"
        );
    }

    #[test]
    fn same_body_successful_publish_allocates_a_new_pair() {
        let publisher = ApiSnapshotPublisher::with_unpublished_epoch_for_test([0x11; 16]);
        publisher.publish_details(PublicDetails::default()).unwrap();
        let first = publisher.published_pair().unwrap();
        publisher.publish_details(PublicDetails::default()).unwrap();
        let second = publisher.published_pair().unwrap();
        assert_ne!(first, second);
        assert!(second.as_str().ends_with("2"));
    }

    #[test]
    fn invalid_publish_and_reads_leave_pair_unchanged() {
        let publisher = ApiSnapshotPublisher::with_unpublished_epoch_for_test([0x22; 16]);
        publisher.publish_details(PublicDetails::default()).unwrap();
        let before = publisher.published_pair();

        let invalid = PublicDetails {
            observed_at: Some(-1),
            ..PublicDetails::default()
        };
        assert_eq!(
            publisher.publish_details(invalid),
            Err(ApiSnapshotError::InvalidObservedAt)
        );
        assert_eq!(publisher.published_pair(), before);
        assert_eq!(publisher.published_pair(), before);
    }

    #[test]
    fn concurrent_successful_publishes_have_unique_pairs() {
        let publisher = std::sync::Arc::new(ApiSnapshotPublisher::with_unpublished_epoch_for_test(
            [0x33; 16],
        ));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
        let handles = (0..16)
            .map(|_| {
                let publisher = std::sync::Arc::clone(&publisher);
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    publisher
                        .publish_for_test(PublicDetails::default())
                        .unwrap()
                        .as_str()
                        .to_owned()
                })
            })
            .collect::<Vec<_>>();
        let identities = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(identities.len(), 16);
    }

    #[test]
    fn counter_overflow_permanently_fails_without_replacing_old_pair() {
        let epoch = [0x44; 16];
        let publisher = ApiSnapshotPublisher::with_unpublished_epoch_for_test(epoch);
        publisher.publish_details(PublicDetails::default()).unwrap();
        let before = publisher.published_pair();
        {
            let mut state = publisher
                .snapshot
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.generation = PublishedPairGenerationState::Active {
                epoch,
                counter: u128::MAX,
            };
        }

        let expected = Err(ApiSnapshotError::PublishedPairGenerationFailed);
        assert_eq!(
            publisher.publish_details(PublicDetails::default()),
            expected
        );
        assert_eq!(publisher.published_pair(), before);
        assert_eq!(
            publisher.publish_details(PublicDetails::default()),
            expected
        );
        let state = publisher
            .snapshot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            state.generation,
            PublishedPairGenerationState::PermanentFailed
        );
    }

    #[test]
    fn entropy_failure_and_all_zero_epoch_fail_before_bind() {
        let occupied = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
        let config = ApiServerConfig::new(occupied.local_addr().unwrap()).unwrap();
        assert!(matches!(
            ApiServer::start_with_epoch_source(config, |_| Err(())),
            Err(ApiServerError::EntropyUnavailable)
        ));
        assert!(matches!(
            ApiServer::start_with_epoch_source(config, |epoch| {
                epoch.fill(0);
                Ok(())
            }),
            Err(ApiServerError::EntropyUnavailable)
        ));
    }

    fn wire_request(address: SocketAddr, request: &str) -> String {
        // Existing fixtures use a human-readable localhost Host. Wire it to
        // the ephemeral listener authority so production parsing remains
        // exact while the tests exercise the same origin-form contract.
        let authority = authority_for(address);
        let host_rewritten = request.replace("Host: localhost", &format!("Host: {authority}"));
        wire_request_raw(address, host_rewritten.as_bytes())
    }

    fn wire_request_raw(address: SocketAddr, request: &[u8]) -> String {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        // A deliberately oversized fixture can be rejected while the client
        // is still writing. Treat that peer close as a malformed-request
        // response boundary; valid fixtures still fail below if no response
        // can be parsed, so this does not hide a healthy-path regression.
        if stream.write_all(request).is_err() {
            return String::new();
        }
        if stream.flush().is_err() {
            return String::new();
        }
        let mut response = String::new();
        if stream.read_to_string(&mut response).is_err() {
            return String::new();
        }
        response
    }

    fn body(response: &str) -> Value {
        let (_, body) = response.split_once("\r\n\r\n").unwrap();
        serde_json::from_str(body).unwrap()
    }

    fn published_pair_headers(response: &str) -> Vec<String> {
        response
            .split("\r\n\r\n")
            .next()
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("Codex-Info-Published-Pair")
                    .then(|| value.trim().to_owned())
            })
            .collect()
    }

    #[test]
    fn details_is_the_sole_snapshot_route_and_owns_one_generation_header() {
        let epoch = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let mut server = ApiServer::start_with_epoch_source(loopback_config(), |target| {
            target.copy_from_slice(&epoch);
            Ok(())
        })
        .unwrap();
        let expected_initial =
            "v1:00112233445566778899aabbccddeeff00000000000000000000000000000001";
        let status = wire_request(
            server.local_addr(),
            "GET /v1/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        let details = wire_request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(status.starts_with("HTTP/1.1 404"));
        assert!(published_pair_headers(&status).is_empty());
        assert_eq!(published_pair_headers(&details), vec![expected_initial]);

        server
            .publisher()
            .publish_details(PublicDetails::default())
            .unwrap();
        let expected_next = "v1:00112233445566778899aabbccddeeff00000000000000000000000000000002";
        let status = wire_request(
            server.local_addr(),
            "GET /v1/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        let details = wire_request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(status.starts_with("HTTP/1.1 404"));
        assert!(published_pair_headers(&status).is_empty());
        assert_eq!(published_pair_headers(&details), vec![expected_next]);
        server.shutdown();
    }

    #[test]
    fn non_success_and_unavailable_responses_have_no_pair_header() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let address = server.local_addr();
        let responses = [
            wire_request(
                address,
                "GET /v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            ),
            wire_request(
                address,
                "GET /v1/missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            ),
            wire_request(
                address,
                "DELETE /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            ),
            wire_request(
                address,
                "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nmalformed\r\n\r\n",
            ),
            wire_request(
                address,
                "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nConnection: close\r\n\r\n",
            ),
        ];
        for response in responses {
            assert!(published_pair_headers(&response).is_empty(), "{response:?}");
        }

        let authority = authority_for(address);
        let oversized = format!(
            "GET /v1/health HTTP/1.1\r\nHost: {authority}\r\nX-Fill: {}\r\n\r\n",
            "x".repeat(MAX_HEADER_AGGREGATE_BYTES)
        );
        let response = wire_request_raw(address, oversized.as_bytes());
        assert!(response.starts_with("HTTP/1.1 431"), "{response:?}");
        assert!(published_pair_headers(&response).is_empty());
        assert!(response.split_once("\r\n\r\n").unwrap().0.len() < MAX_HEADER_AGGREGATE_BYTES);

        {
            let publisher = server.publisher();
            let mut state = publisher
                .snapshot
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.generation = PublishedPairGenerationState::PermanentFailed;
        }
        let response = wire_request(
            address,
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(response.starts_with("HTTP/1.1 503"), "{response:?}");
        assert_eq!(body(&response)["error"], "snapshot_unavailable");
        assert!(published_pair_headers(&response).is_empty());
        server.shutdown();
    }

    #[test]
    fn too_many_active_connections_have_no_pair_header() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let address = server.local_addr();
        let authority = authority_for(address);
        let partial = format!("GET /v1/health HTTP/1.1\r\nHost: {authority}\r\n");
        let mut blockers = Vec::with_capacity(MAX_ACTIVE_CONNECTIONS);
        for _ in 0..MAX_ACTIVE_CONNECTIONS {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(partial.as_bytes()).unwrap();
            blockers.push(stream);
        }
        // Connecting and writing the partial requests does not prove that the
        // listener threads have accepted all blockers yet.  Wait for the
        // externally observable saturation response instead of racing a fixed
        // sleep against a loaded test host.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let response = loop {
            let response = wire_request(
                address,
                "GET /v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            );
            if response.starts_with("HTTP/1.1 429") || std::time::Instant::now() >= deadline {
                break response;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(response.starts_with("HTTP/1.1 429"), "{response:?}");
        assert!(published_pair_headers(&response).is_empty());
        drop(blockers);
        server.shutdown();
    }

    fn detailed_fixture() -> PublicDetails {
        PublicDetails {
            state: PublicState::Ready,
            observed_at: Some(1_780_000_020),
            authenticated: true,
            plan_label: Some("Pro".into()),
            quota: Some(PublicQuota {
                remaining_percent: 98.0,
                reset_at: 1_780_400_000,
                window_seconds: 604_800,
                monthly: false,
            }),
            models: vec![PublicDetailedModelUsage {
                name: "SOL".into(),
                input_tokens: 900,
                cached_input_tokens: 300,
                output_tokens: 400,
                input_dollars: 0.0045,
                cached_input_dollars: 0.00015,
                output_dollars: 0.012,
            }],
            active_thread_count: 1,
            history_periods: vec![PublicHistoryPeriod {
                id: "1780400000".into(),
                start_at: 1_779_395_200,
                end_at: 1_780_000_020,
                reset_at: 1_780_400_000,
                label: "2026/06/01 — 2026/06/08".into(),
                current: true,
            }],
            history_samples: vec![PublicHistorySample {
                timestamp: 1_780_000_020,
                reset_at: 1_780_400_000,
                remaining_percent: None,
                sol_dollars: 0.01665,
                terra_dollars: 0.0,
                luna_dollars: 0.0,
                sol_tokens: 1_600,
                terra_tokens: 0,
                luna_tokens: 0,
            }],
            history_gaps: Vec::new(),
            threads: vec![PublicThread {
                id: "thread-1".into(),
                title: "安全な読み取り確認".into(),
                parent_thread_id: None,
                model: "gpt-5.6-sol".into(),
                model_label: "gpt-5.6-sol".into(),
                total_tokens: Some(1_600),
                context_usage_tokens: Some(1_200),
                context_window_tokens: Some(258_400),
                created_at: Some(1_779_999_000),
                last_user_message_at: Some(1_779_999_900),
                is_subagent: false,
                depth: None,
            }],
            estimated_cost_label: "概算 $1".into(),
        }
    }

    fn history_fixture(sample_count: usize) -> PublicDetails {
        let mut details = detailed_fixture();
        let end = 1_800_000_000_i64;
        let start = end - sample_count as i64 * 60;
        details.observed_at = Some(end);
        details.quota = Some(PublicQuota {
            remaining_percent: 48.0,
            reset_at: end + 604_800,
            window_seconds: 604_800,
            monthly: false,
        });
        details.history_periods = vec![PublicHistoryPeriod {
            id: "slo-period".into(),
            start_at: start,
            end_at: end,
            reset_at: end + 604_800,
            label: "SLO fixture".into(),
            current: true,
        }];
        details.history_samples = (0..sample_count)
            .map(|index| {
                let fraction = index as f64 / sample_count.max(1) as f64;
                PublicHistorySample {
                    timestamp: start + index as i64 * 60,
                    reset_at: end + 604_800,
                    remaining_percent: Some(100.0 - 52.0 * fraction),
                    sol_dollars: 8.75 * fraction,
                    terra_dollars: 4.5 * fraction,
                    luna_dollars: 2.75 * fraction,
                    sol_tokens: (8_400.0 * fraction) as u64,
                    terra_tokens: (4_200.0 * fraction) as u64,
                    luna_tokens: (2_100.0 * fraction) as u64,
                }
            })
            .collect();
        details
    }

    fn latency_percentiles(address: SocketAddr, route: &str, runs: usize) -> (f64, f64, f64) {
        let request =
            format!("GET {route} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        for _ in 0..5 {
            let _ = wire_request(address, &request);
        }
        let mut elapsed = Vec::with_capacity(runs);
        for _ in 0..runs {
            let started = Instant::now();
            let response = wire_request(address, &request);
            assert!(response.starts_with("HTTP/1.1"));
            elapsed.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        elapsed.sort_by(f64::total_cmp);
        let percentile = |percent: usize| elapsed[(runs * percent).div_ceil(100) - 1];
        (percentile(90), percentile(95), elapsed[runs - 1])
    }

    #[test]
    #[ignore = "explicit host loopback latency SLO gate"]
    fn all_rest_routes_meet_latency_slo_at_supported_capacity() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let address = server.local_addr();

        for route in ["/v1/health", "/v1/missing"] {
            let (p90, p95, maximum) = latency_percentiles(address, route, 100);
            eprintln!("SLO route={route} n=100 p90={p90:.3}ms p95={p95:.3}ms max={maximum:.3}ms");
            assert!(p90 <= 25.0, "{route} p90 {p90:.3}ms exceeds 25ms");
            assert!(p95 <= 50.0, "{route} p95 {p95:.3}ms exceeds 50ms");
        }

        for (sample_count, p90_limit, p95_limit) in [
            (10_080, 50.0, 100.0),
            (MAX_PUBLIC_HISTORY_SAMPLES, 100.0, 150.0),
        ] {
            server
                .publisher()
                .publish_details(history_fixture(sample_count))
                .unwrap();
            let (p90, p95, maximum) = latency_percentiles(address, "/v1/details", 30);
            eprintln!(
                "SLO route=/v1/details samples={sample_count} n=30 p90={p90:.3}ms p95={p95:.3}ms max={maximum:.3}ms"
            );
            assert!(
                p90 <= p90_limit,
                "details({sample_count}) p90 {p90:.3}ms exceeds {p90_limit}ms"
            );
            assert!(
                p95 <= p95_limit,
                "details({sample_count}) p95 {p95:.3}ms exceeds {p95_limit}ms"
            );
        }
        server.shutdown();
    }

    #[test]
    fn environment_is_disabled_when_listen_is_unset() {
        let _guard = environment_lock().lock().unwrap();
        let previous = env::var_os(API_LISTEN_ENV);
        env::remove_var(API_LISTEN_ENV);
        assert_eq!(ApiServerConfig::from_environment().unwrap(), None);
        match previous {
            Some(value) => env::set_var(API_LISTEN_ENV, value),
            None => env::remove_var(API_LISTEN_ENV),
        }
    }

    #[test]
    fn configuration_rejects_non_loopback_or_non_numeric_addresses() {
        assert_eq!(
            ApiServerConfig::new("0.0.0.0:8787".parse().unwrap()),
            Err(ApiServerError::NonLoopbackAddress)
        );
        assert_eq!(
            ApiServerConfig::new("192.168.1.7:8787".parse().unwrap()),
            Err(ApiServerError::NonLoopbackAddress)
        );
        assert_eq!(
            ApiServerConfig::new("[::]:8787".parse().unwrap()),
            Err(ApiServerError::NonLoopbackAddress)
        );
        assert!(ApiServerConfig::new("[::1]:8787".parse().unwrap()).is_ok());
        let _guard = environment_lock().lock().unwrap();
        let previous = env::var_os(API_LISTEN_ENV);
        env::set_var(API_LISTEN_ENV, "localhost:8787");
        assert_eq!(
            ApiServerConfig::from_environment(),
            Err(ApiServerError::InvalidListenConfiguration)
        );
        match previous {
            Some(value) => env::set_var(API_LISTEN_ENV, value),
            None => env::remove_var(API_LISTEN_ENV),
        }
    }

    #[test]
    fn health_details_errors_and_snapshot_are_json_no_store() {
        // All API tests use an ephemeral loopback port. Serialize their server
        // lifetimes so another test cannot claim this test's just-released
        // port between shutdown and the explicit rebind assertion.
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let publisher = server.publisher();
        publisher.publish_details(detailed_fixture()).unwrap();

        let health = wire_request(
            server.local_addr(),
            "GET /v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(health.starts_with("HTTP/1.1 200"), "response: {health:?}");
        assert!(health.contains("cache-control: no-store"));
        assert_eq!(body(&health)["api_version"], "v1");

        let details = wire_request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(details.starts_with("HTTP/1.1 200"));
        assert!(details.contains("cache-control: no-store"));
        assert_eq!(body(&details)["state"], "ready");

        let missing = wire_request(
            server.local_addr(),
            "GET /v1/missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(missing.starts_with("HTTP/1.1 404"));
        assert_eq!(body(&missing)["error"], "not_found");

        let wrong_method = wire_request(
            server.local_addr(),
            "POST /v1/details HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        assert!(wrong_method.starts_with("HTTP/1.1 405"));
        assert_eq!(body(&wrong_method)["error"], "method_not_allowed");

        let wrong_details_method = wire_request(
            server.local_addr(),
            "POST /v1/details HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        assert!(wrong_details_method.starts_with("HTTP/1.1 405"));
        server.shutdown();
    }

    #[test]
    fn request_wire_contract_has_bounded_read_only_mapping() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let address = server.local_addr();
        let authority = authority_for(address);
        let request = |start: &str, headers: &str, body: &str| {
            format!("{start}\r\n{headers}\r\n\r\n{body}").into_bytes()
        };
        let host = format!("Host: {authority}\r\nConnection: close");

        let mut cases: Vec<(Vec<u8>, u16, &str)> = vec![
            (
                request("GET /v1/details?query=1 HTTP/1.1", &host, ""),
                404,
                "not_found",
            ),
            (
                request("GET http://127.0.0.1:8787/v1/details HTTP/1.1", &host, ""),
                400,
                "bad_request",
            ),
            (
                request("GET //127.0.0.1/v1/details HTTP/1.1", &host, ""),
                400,
                "bad_request",
            ),
            (
                request("GET /v1/details HTTP/1.1", "Connection: close", ""),
                400,
                "bad_request",
            ),
            (
                request(
                    "GET /v1/details HTTP/1.1",
                    &format!("Host: {authority}\r\nhost: {authority}\r\nConnection: close"),
                    "",
                ),
                400,
                "bad_request",
            ),
            (
                request(
                    "GET /v1/details HTTP/1.1",
                    "Host: 127.0.0.1:1\r\nConnection: close",
                    "",
                ),
                400,
                "bad_request",
            ),
            (
                request(
                    "GET /v1/details HTTP/1.1",
                    &format!("{host}\r\nContent-Length: 1"),
                    "x",
                ),
                413,
                "request_body_not_allowed",
            ),
            (
                request(
                    "GET /v1/details HTTP/1.1",
                    &format!("{host}\r\nTransfer-Encoding: chunked"),
                    "",
                ),
                413,
                "request_body_not_allowed",
            ),
            (
                request(
                    "GET /v1/details HTTP/1.1",
                    &format!("{host}\r\nContent-Length: nope"),
                    "",
                ),
                400,
                "bad_request",
            ),
            (
                request(
                    "GET /v1/details HTTP/1.1",
                    &format!("{host}\r\nAuthorization: Bearer redacted"),
                    "",
                ),
                400,
                "bad_request",
            ),
            (
                request(
                    "GET /v1/details HTTP/1.1",
                    &format!("{host}\r\nAccept: */*\r\nAccept: application/json"),
                    "",
                ),
                400,
                "bad_request",
            ),
            (
                request(
                    "POST /v1/details HTTP/1.1",
                    &format!("{host}\r\nContent-Length: 0"),
                    "",
                ),
                405,
                "method_not_allowed",
            ),
            (
                request(
                    "POST /v1/unknown HTTP/1.1",
                    &format!("{host}\r\nContent-Length: 0"),
                    "",
                ),
                404,
                "not_found",
            ),
            (
                request("GET /v1/details HTTP/1.1\n", &host, ""),
                400,
                "bad_request",
            ),
        ];

        let oversized = format!("{host}\r\nX-Oversized: {}", "x".repeat(1_025));
        cases.push((
            request("GET /v1/details HTTP/1.1", &oversized, ""),
            431,
            "request_headers_too_large",
        ));
        let aggregate = (0..9)
            .map(|index| format!("X-{index}: {}", "x".repeat(1_000)))
            .collect::<Vec<_>>()
            .join("\r\n");
        cases.push((
            request(
                "GET /v1/details HTTP/1.1",
                &format!("{host}\r\n{aggregate}"),
                "",
            ),
            431,
            "request_headers_too_large",
        ));
        let count = (0..33)
            .map(|index| format!("X-{index}: x"))
            .collect::<Vec<_>>()
            .join("\r\n");
        cases.push((
            request(
                "GET /v1/details HTTP/1.1",
                &format!("{host}\r\n{count}"),
                "",
            ),
            431,
            "request_headers_too_large",
        ));

        for (raw_request, status, error) in cases {
            let response = wire_request_raw(address, &raw_request);
            assert!(
                response.starts_with(&format!("HTTP/1.1 {status}")),
                "{response:?}"
            );
            assert_eq!(body(&response)["error"], error, "request={raw_request:?}");
            assert!(response.contains("content-type: application/json; charset=utf-8"));
            assert!(response.contains("cache-control: no-store"));
            assert!(response.contains("connection: close"));
        }

        let canonical = wire_request_raw(
            address,
            format!(
                "GET /v1/health HTTP/1.1\r\nHost: {authority}\r\nAccept: */*\r\nUser-Agent: curl/8\r\n\r\n"
            )
            .as_bytes(),
        );
        assert!(canonical.starts_with("HTTP/1.1 200"), "{canonical:?}");
        assert_eq!(body(&canonical)["service"], "codex-info");

        server.shutdown();
    }

    #[test]
    fn details_endpoint_publishes_the_complete_whitelisted_document() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        server
            .publisher()
            .publish_details(detailed_fixture())
            .unwrap();

        let details = wire_request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(details.starts_with("HTTP/1.1 200"));
        assert!(details.contains("cache-control: no-store"));
        let details_body = body(&details);
        assert_eq!(details_body["api_version"], "v1");
        let mut top_level_keys = details_body
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        top_level_keys.sort();
        assert_eq!(
            top_level_keys,
            [
                "active_thread_count",
                "api_version",
                "authenticated",
                "estimated_cost_label",
                "history_gaps",
                "history_periods",
                "history_samples",
                "models",
                "observed_at",
                "plan_label",
                "quota",
                "state",
                "threads",
            ]
        );
        assert_eq!(details_body["history_gaps"], Value::Array(Vec::new()));
        assert_eq!(details_body["models"][0]["input_dollars"], 0.0045);
        assert!(details_body["history_samples"][0]["remaining_percent"].is_null());
        assert_eq!(details_body["history_periods"][0]["id"], "1780400000");
        assert_eq!(details_body["threads"][0]["context_window_tokens"], 258400);
        assert!(details_body.get("email").is_none());
        assert!(details_body.get("auth_url").is_none());
        assert!(details_body.get("error_detail").is_none());

        server.shutdown();
    }

    #[test]
    fn rejected_requests_cannot_mutate_the_published_pair() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        server
            .publisher()
            .publish_details(detailed_fixture())
            .unwrap();
        let before_details = body(&wire_request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        ));

        let cases = [
            "DELETE /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                .to_owned(),
            "GET /v1/unknown HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                .to_owned(),
            "GET /v1/details HTTP/1.1\r\nmalformed-header\r\nConnection: close\r\n\r\n"
                .to_owned(),
            format!(
                "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nX-Oversize: {}\r\nConnection: close\r\n\r\n",
                "x".repeat(128 * 1024)
            ),
        ];
        for request in cases {
            let _ = wire_request(server.local_addr(), &request);
            assert_eq!(
                body(&wire_request(
                    server.local_addr(),
                    "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )),
                before_details
            );
        }
        server.shutdown();
    }

    #[test]
    fn invalid_publication_keeps_the_last_snapshot() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let publisher = server.publisher();
        let initial = PublicDetails {
            state: PublicState::AuthRequired,
            ..PublicDetails::default()
        };
        publisher.publish_details(initial).unwrap();
        let invalid = PublicDetails {
            quota: Some(PublicQuota {
                remaining_percent: 101.0,
                reset_at: 1,
                window_seconds: 1,
                monthly: false,
            }),
            ..PublicDetails::default()
        };
        assert_eq!(
            publisher.publish_details(invalid),
            Err(ApiSnapshotError::InvalidQuota)
        );
        let details = wire_request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(body(&details)["state"], "auth_required");
        server.shutdown();
    }

    #[test]
    fn invalid_details_keep_the_last_atomic_generation() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let publisher = server.publisher();
        publisher.publish_details(detailed_fixture()).unwrap();
        let mut invalid = detailed_fixture();
        invalid.models[0].output_dollars = f64::NAN;
        assert_eq!(
            publisher.publish_details(invalid),
            Err(ApiSnapshotError::InvalidModel)
        );
        let details = wire_request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(body(&details)["estimated_cost_label"], "概算 $1");
        server.shutdown();
    }

    #[test]
    fn duplicate_thread_details_keep_the_last_atomic_generation() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let publisher = server.publisher();
        let known_good = detailed_fixture();
        publisher.publish_details(known_good.clone()).unwrap();
        let before_pair = publisher.published_pair();
        let before_details = wire_request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );

        let mut duplicate = known_good;
        duplicate.threads.push(duplicate.threads[0].clone());
        assert_eq!(
            publisher.publish_details(duplicate),
            Err(ApiSnapshotError::InvalidThread)
        );
        assert_eq!(publisher.published_pair(), before_pair);
        assert_eq!(
            wire_request(
                server.local_addr(),
                "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            ),
            before_details
        );
        server.shutdown();
    }

    #[test]
    fn one_month_history_overflow_rejects_candidate_and_keeps_last_generation() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let publisher = server.publisher();
        publisher.publish_details(detailed_fixture()).unwrap();

        let oversized = history_fixture(MAX_PUBLIC_HISTORY_SAMPLES + 1);
        assert_eq!(
            publisher.publish_details(oversized),
            Err(ApiSnapshotError::ListTooLong)
        );

        let details = wire_request(
            server.local_addr(),
            "GET /v1/details HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        let details = body(&details);
        assert_eq!(details["estimated_cost_label"], "概算 $1");
        assert_eq!(details["history_samples"].as_array().unwrap().len(), 1);
        server.shutdown();
    }

    #[test]
    fn detail_validation_is_bounded_for_times_rates_lists_models_and_text() {
        let mut invalid = detailed_fixture();
        invalid.history_samples[0].timestamp = 0;
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistorySample)
        );

        let mut invalid = detailed_fixture();
        let observed_at = invalid.observed_at.unwrap();
        invalid.history_periods[0].current = false;
        invalid.history_periods[0].end_at = observed_at + 60;
        invalid.history_samples[0].timestamp = observed_at + 60;
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistorySample)
        );

        let mut invalid = detailed_fixture();
        invalid.history_samples[0].sol_dollars = f64::INFINITY;
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistorySample)
        );

        let mut invalid = detailed_fixture();
        invalid.history_samples[0].timestamp += 1;
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistorySample)
        );

        let mut invalid = detailed_fixture();
        invalid.history_samples[0].timestamp = invalid.history_periods[0].end_at + 60;
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistorySample)
        );

        let mut invalid = detailed_fixture();
        invalid
            .history_samples
            .push(invalid.history_samples[0].clone());
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistorySample)
        );

        let mut invalid = detailed_fixture();
        let mut canonical_collision = invalid.history_samples[0].clone();
        canonical_collision.reset_at -= 60;
        invalid.history_samples.push(canonical_collision);
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistorySample)
        );

        let mut invalid = detailed_fixture();
        invalid.history_periods[0].label = "x".repeat(513);
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistoryPeriod)
        );

        let mut invalid = detailed_fixture();
        invalid.history_periods[0].label.push('\u{202e}');
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistoryPeriod)
        );

        let mut invalid = detailed_fixture();
        invalid.history_samples.clear();
        invalid.history_periods.push(PublicHistoryPeriod {
            id: "duplicate-reset".into(),
            start_at: 1_779_395_200,
            end_at: 1_780_000_020,
            reset_at: 1_780_400_000,
            label: "duplicate reset".into(),
            current: false,
        });
        assert_eq!(
            invalid.validate(),
            Err(ApiSnapshotError::InvalidHistoryPeriod)
        );

        let mut invalid = detailed_fixture();
        invalid.threads[0].title = "x".repeat(513);
        assert_eq!(invalid.validate(), Err(ApiSnapshotError::InvalidThread));

        let mut invalid = detailed_fixture();
        invalid.models = vec![invalid.models[0].clone(); MAX_PUBLIC_MODELS + 1];
        assert_eq!(invalid.validate(), Err(ApiSnapshotError::ListTooLong));

        let mut invalid = detailed_fixture();
        invalid.models[0].name = "OTHER".into();
        assert_eq!(invalid.validate(), Err(ApiSnapshotError::InvalidModel));
    }

    #[test]
    fn public_thread_slice_validation_keeps_exact_capacity_errors() {
        let template = detailed_fixture()
            .threads
            .into_iter()
            .next()
            .expect("detailed fixture has one thread");
        let make_threads = |count: usize| {
            (0..count)
                .map(|index| {
                    let mut thread = template.clone();
                    thread.id = format!("thread-{index}");
                    thread.title = format!("title-{index}");
                    thread
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(
            validate_public_threads(&make_threads(MAX_PUBLIC_THREADS)),
            Ok(())
        );
        assert_eq!(
            validate_public_threads(&make_threads(MAX_PUBLIC_THREADS + 1)),
            Err(ApiSnapshotError::ListTooLong)
        );

        let duplicate = vec![template.clone(), template.clone()];
        assert_eq!(
            validate_public_threads(&duplicate),
            Err(ApiSnapshotError::InvalidThread)
        );

        let mut invalid_value = template;
        invalid_value.title = "x".repeat(513);
        assert_eq!(
            validate_public_threads(&[invalid_value]),
            Err(ApiSnapshotError::InvalidThread)
        );
    }

    #[test]
    fn history_gap_validation_is_exact_and_period_bounded() {
        let mut details = detailed_fixture();
        details.history_gaps = vec![PublicHistoryGap {
            gap_id: "0123456789abcdef0123456789abcdef".into(),
            reset_at: 1_780_400_000,
            start_at: 1_779_500_000,
            end_at: 1_779_500_060,
            reason: "daemon_stop_unrecoverable".into(),
        }];
        details.validate().unwrap();

        let serialized = serde_json::to_value(&details).unwrap();
        let gap = serialized["history_gaps"][0].as_object().unwrap();
        assert_eq!(gap.len(), 5);
        assert_eq!(
            gap.keys().cloned().collect::<Vec<_>>(),
            vec!["end_at", "gap_id", "reason", "reset_at", "start_at"]
        );

        let mut invalid = details.clone();
        invalid.history_gaps[0].gap_id = "0123456789ABCDEF0123456789abcdef".into();
        assert_eq!(invalid.validate(), Err(ApiSnapshotError::InvalidHistoryGap));

        let mut invalid = details.clone();
        invalid.history_gaps[0].reason = "pending".into();
        assert_eq!(invalid.validate(), Err(ApiSnapshotError::InvalidHistoryGap));

        let mut invalid = details.clone();
        invalid.history_gaps[0].start_at = 1_779_500_061;
        assert_eq!(invalid.validate(), Err(ApiSnapshotError::InvalidHistoryGap));

        let mut invalid = details.clone();
        invalid.history_gaps.push(PublicHistoryGap {
            gap_id: "fedcba9876543210fedcba9876543210".into(),
            reset_at: 1_780_400_000,
            start_at: 1_779_500_060,
            end_at: 1_779_500_120,
            reason: "reset_hint_expired".into(),
        });
        assert_eq!(invalid.validate(), Err(ApiSnapshotError::InvalidHistoryGap));

        let mut invalid = details.clone();
        invalid.history_gaps[0].end_at = 1_780_000_080;
        assert_eq!(invalid.validate(), Err(ApiSnapshotError::InvalidHistoryGap));
    }

    #[test]
    fn shutdown_releases_the_bound_port() {
        let _guard = api_server_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut server = ApiServer::start(loopback_config()).unwrap();
        let address = server.local_addr();
        let conflicting = ApiServerConfig::new(address).unwrap();
        assert_eq!(
            ApiServer::start(conflicting).err(),
            Some(ApiServerError::BindFailed)
        );
        server.shutdown();
        server.shutdown();
        let rebound = TcpListener::bind(address);
        assert!(rebound.is_ok());
    }
}

//! Public, immutable REST DTOs shared by the recorder-independent reader and
//! the REST executable.
//!
//! This crate intentionally contains no filesystem, SQLite, session, writer,
//! UI, or HTTP-server dependency.  Keeping the wire values here makes the
//! process boundary visible to Cargo as well as to the runtime.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const API_VERSION: &str = "v1";
pub const API_VERSION_V2: &str = "v2";
pub const API_VERSION_V3: &str = "v3";
pub const MAX_PUBLIC_MODELS: usize = 3;
pub const MAX_PUBLIC_MODELS_V3: usize = 1_024;
pub const MAX_PUBLIC_HISTORY_PERIODS: usize = 128;
pub const MAX_PUBLIC_HISTORY_SAMPLES: usize = 31 * 24 * 60;
pub const MAX_PUBLIC_HISTORY_GAPS: usize = 4_096;
pub const MAX_PUBLIC_THREADS: usize = 256;
pub const MAX_PUBLIC_ID_SCALARS: usize = 512;
const MAX_PUBLIC_UNIX_SECONDS: i64 = 253_402_300_799;
const MAX_PUBLIC_HISTORY_LABEL_SCALARS: usize = 512;
const MAX_PUBLIC_STATUS_SCALARS: usize = 160;
const MAX_PUBLIC_PLAN_SCALARS: usize = 64;
pub const MAX_PUBLIC_MODEL_SCALARS: usize = 128;
const MAX_PUBLIC_MODEL_LABEL_SCALARS: usize = 24;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicState {
    #[default]
    Initializing,
    Ready,
    AuthRequired,
    Error,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicQuota {
    pub remaining_percent: f64,
    pub reset_at: i64,
    pub window_seconds: i64,
    pub monthly: bool,
}

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
    pub reset_at: i64,
    pub label: String,
    pub current: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicHistorySample {
    pub timestamp: i64,
    pub reset_at: i64,
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
pub struct PublicHistoryObservation {
    pub timestamp: i64,
    pub reset_at: i64,
    pub remaining_percent: Option<f64>,
    pub sol_dollars: Option<f64>,
    pub terra_dollars: Option<f64>,
    pub luna_dollars: Option<f64>,
    pub sol_tokens: Option<u64>,
    pub terra_tokens: Option<u64>,
    pub luna_tokens: Option<u64>,
    pub model_source: String,
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

/// The exact v1 details root.  The `api_version` field is supplied by the
/// response envelope so this type can also be used as an internal snapshot.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractError {
    InvalidTimestamp,
    InvalidNumber,
    InvalidPeriod,
    InvalidSample,
    InvalidGap,
    InvalidModel,
    TooManyItems,
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTimestamp => "invalid timestamp",
            Self::InvalidNumber => "invalid numeric value",
            Self::InvalidPeriod => "invalid history period",
            Self::InvalidSample => "invalid history sample",
            Self::InvalidGap => "invalid history gap",
            Self::InvalidModel => "invalid model",
            Self::TooManyItems => "too many public items",
        })
    }
}

impl std::error::Error for ContractError {}

fn valid_timestamp(value: i64) -> bool {
    (1..=MAX_PUBLIC_UNIX_SECONDS).contains(&value)
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= limit
        && !value
            .chars()
            .any(|character| character.is_control() || is_bidi_formatting(character))
}

/// Validate a model identifier at the v3 wire boundary.  Reader-side
/// projection uses the same predicate so malformed names are isolated before
/// they can make an otherwise valid snapshot fail strict validation.
pub fn is_valid_public_model_name(value: &str) -> bool {
    valid_text(value, MAX_PUBLIC_MODEL_SCALARS)
}

fn valid_rate(value: f64) -> bool {
    value.is_finite() && value >= 0.0
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

impl PublicDetails {
    /// Validate values before they cross the REST process boundary.
    ///
    /// This is deliberately a pure bounded check.  It never reads the
    /// filesystem or tries to repair a malformed database row.
    pub fn validate(&self) -> Result<(), ContractError> {
        if self
            .observed_at
            .is_some_and(|value| !valid_timestamp(value))
        {
            return Err(ContractError::InvalidTimestamp);
        }
        if let Some(quota) = &self.quota {
            if !valid_rate(quota.remaining_percent)
                || quota.remaining_percent > 100.0
                || !valid_timestamp(quota.reset_at)
                || quota.window_seconds <= 0
            {
                return Err(ContractError::InvalidNumber);
            }
        }
        if self.models.len() > MAX_PUBLIC_MODELS
            || self.history_periods.len() > MAX_PUBLIC_HISTORY_PERIODS
            || self.history_samples.len() > MAX_PUBLIC_HISTORY_SAMPLES
            || self.history_gaps.len() > MAX_PUBLIC_HISTORY_GAPS
            || self.threads.len() > MAX_PUBLIC_THREADS
        {
            return Err(ContractError::TooManyItems);
        }
        if self
            .plan_label
            .as_deref()
            .is_some_and(|label| !valid_text(label, MAX_PUBLIC_PLAN_SCALARS))
            || !valid_text(&self.estimated_cost_label, MAX_PUBLIC_STATUS_SCALARS)
        {
            return Err(ContractError::InvalidModel);
        }

        let mut names = HashSet::new();
        for model in &self.models {
            if !matches!(model.name.as_str(), "SOL" | "TERRA" | "LUNA")
                || !names.insert(model.name.as_str())
                || !valid_rate(model.input_dollars)
                || !valid_rate(model.cached_input_dollars)
                || !valid_rate(model.output_dollars)
            {
                return Err(ContractError::InvalidModel);
            }
        }

        let mut period_ids = HashSet::new();
        let mut period_resets = HashSet::new();
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
            {
                return Err(ContractError::InvalidPeriod);
            }
            if period.current {
                current_periods = current_periods.saturating_add(1);
            }
        }
        if current_periods > 1 {
            return Err(ContractError::InvalidPeriod);
        }
        for period in &self.history_periods {
            if period.current {
                let Some(observed_at) = self.observed_at else {
                    return Err(ContractError::InvalidPeriod);
                };
                if period.end_at != period.reset_at.min(observed_at) {
                    return Err(ContractError::InvalidPeriod);
                }
            }
        }

        let mut sample_ids = HashSet::with_capacity(self.history_samples.len());
        let mut canonical_sample_ids = HashSet::with_capacity(self.history_samples.len());
        let mut previous_sample_key = None;
        for sample in &self.history_samples {
            if !valid_timestamp(sample.timestamp)
                || !valid_timestamp(sample.reset_at)
                || sample.timestamp.rem_euclid(60) != 0
                || self
                    .observed_at
                    .is_some_and(|observed| sample.timestamp > observed)
                || sample
                    .remaining_percent
                    .is_some_and(|value| !valid_rate(value) || value > 100.0)
                || !valid_rate(sample.sol_dollars)
                || !valid_rate(sample.terra_dollars)
                || !valid_rate(sample.luna_dollars)
            {
                return Err(ContractError::InvalidSample);
            }
            if !sample_ids.insert((sample.reset_at, sample.timestamp)) {
                return Err(ContractError::InvalidSample);
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
                return Err(ContractError::InvalidSample);
            }
            let period = matching_periods[0];
            if !canonical_sample_ids.insert((period.id.as_str(), sample.timestamp)) {
                return Err(ContractError::InvalidSample);
            }
            let sample_key = (sample.reset_at, sample.timestamp);
            if previous_sample_key.is_some_and(|previous| previous > sample_key) {
                return Err(ContractError::InvalidSample);
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
                return Err(ContractError::InvalidGap);
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
                return Err(ContractError::InvalidGap);
            }
            let period = matching_periods[0];
            let gap_key = (gap.reset_at, gap.start_at, gap.end_at, gap.gap_id.as_str());
            if previous_gap_key.is_some_and(|previous| previous > gap_key) {
                return Err(ContractError::InvalidGap);
            }
            previous_gap_key = Some(gap_key);
            if gap_ranges.iter().any(|(period_id, start_at, end_at)| {
                *period_id == period.id.as_str()
                    && gap.start_at <= *end_at
                    && *start_at <= gap.end_at
            }) {
                return Err(ContractError::InvalidGap);
            }
            gap_ranges.push((period.id.as_str(), gap.start_at, gap.end_at));
        }
        self.validate_threads()?;
        Ok(())
    }

    fn validate_threads(&self) -> Result<(), ContractError> {
        if self.threads.len() > MAX_PUBLIC_THREADS {
            return Err(ContractError::TooManyItems);
        }
        let mut thread_ids = HashSet::with_capacity(self.threads.len());
        for thread in &self.threads {
            if !valid_text(&thread.id, MAX_PUBLIC_ID_SCALARS)
                || !thread_ids.insert(thread.id.as_str())
                || !valid_text(&thread.title, MAX_PUBLIC_ID_SCALARS)
                || !valid_text(&thread.model, MAX_PUBLIC_MODEL_SCALARS)
                || !valid_text(&thread.model_label, MAX_PUBLIC_MODEL_LABEL_SCALARS)
                || !thread
                    .parent_thread_id
                    .as_deref()
                    .is_none_or(|id| valid_text(id, MAX_PUBLIC_ID_SCALARS))
                || !thread.created_at.is_none_or(valid_timestamp)
                || !thread.last_user_message_at.is_none_or(valid_timestamp)
                || !thread.depth.is_none_or(|depth| (0..=1024).contains(&depth))
            {
                return Err(ContractError::InvalidModel);
            }
        }
        Ok(())
    }
}

/// V2 adds source classification while retaining the v1 field names.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicDetailsV2 {
    pub state: PublicState,
    pub observed_at: Option<i64>,
    pub authenticated: bool,
    pub plan_label: Option<String>,
    pub quota: Option<PublicQuota>,
    pub models: Vec<PublicDetailedModelUsage>,
    pub active_thread_count: u64,
    pub history_periods: Vec<PublicHistoryPeriod>,
    pub history_samples: Vec<PublicHistoryObservation>,
    pub history_gaps: Vec<PublicHistoryGap>,
    pub threads: Vec<PublicThread>,
    pub estimated_cost_label: String,
}

impl From<&PublicDetails> for PublicDetailsV2 {
    fn from(details: &PublicDetails) -> Self {
        Self {
            state: details.state,
            observed_at: details.observed_at,
            authenticated: details.authenticated,
            plan_label: details.plan_label.clone(),
            quota: details.quota.clone(),
            models: details.models.clone(),
            active_thread_count: details.active_thread_count,
            history_periods: details.history_periods.clone(),
            history_samples: details
                .history_samples
                .iter()
                .map(|sample| PublicHistoryObservation {
                    timestamp: sample.timestamp,
                    reset_at: sample.reset_at,
                    remaining_percent: sample.remaining_percent,
                    sol_dollars: Some(sample.sol_dollars),
                    terra_dollars: Some(sample.terra_dollars),
                    luna_dollars: Some(sample.luna_dollars),
                    sol_tokens: Some(sample.sol_tokens),
                    terra_tokens: Some(sample.terra_tokens),
                    luna_tokens: Some(sample.luna_tokens),
                    model_source: "legacy-unknown".to_owned(),
                })
                .collect(),
            history_gaps: details.history_gaps.clone(),
            threads: details.threads.clone(),
            estimated_cost_label: details.estimated_cost_label.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicModelUsageV3 {
    pub model: String,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub estimated_cost: Option<PublicModelCostV3>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicModelCostV3 {
    pub price_version: String,
    pub ordinary_input_dollars: f64,
    pub cached_input_dollars: f64,
    pub cache_write_input_dollars: f64,
    pub output_dollars: f64,
    pub total_dollars: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicHistoryModelUsageV3 {
    pub model: String,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_dollars: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicHistoryObservationV3 {
    pub timestamp: i64,
    pub reset_at: i64,
    pub remaining_percent: Option<f64>,
    pub models: Option<Vec<PublicHistoryModelUsageV3>>,
    pub models_complete: bool,
    pub model_source: String,
}

/// Project the legacy SOL/TERRA/LUNA columns without turning an unavailable
/// observation into zero-valued models.  This helper mirrors the root
/// consumer contract and is intentionally pure.
pub fn legacy_history_models_v3(
    sample: &PublicHistoryObservation,
) -> Option<Vec<PublicHistoryModelUsageV3>> {
    if sample.model_source == "unavailable" {
        return None;
    }
    Some(vec![
        legacy_model("SOL", sample.sol_tokens?, sample.sol_dollars?),
        legacy_model("TERRA", sample.terra_tokens?, sample.terra_dollars?),
        legacy_model("LUNA", sample.luna_tokens?, sample.luna_dollars?),
    ])
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicDetailsV3 {
    pub state: PublicState,
    pub observed_at: Option<i64>,
    pub authenticated: bool,
    pub plan_label: Option<String>,
    pub quota: Option<PublicQuota>,
    pub models: Vec<PublicModelUsageV3>,
    pub active_thread_count: u64,
    pub history_periods: Vec<PublicHistoryPeriod>,
    pub history_samples: Vec<PublicHistoryObservationV3>,
    pub history_gaps: Vec<PublicHistoryGap>,
    pub threads: Vec<PublicThread>,
}

impl From<&PublicDetailsV2> for PublicDetailsV3 {
    fn from(details: &PublicDetailsV2) -> Self {
        let models: Vec<PublicModelUsageV3> = details
            .models
            .iter()
            .map(|model| PublicModelUsageV3 {
                model: model.name.clone(),
                total_tokens: model
                    .input_tokens
                    .saturating_add(model.cached_input_tokens)
                    .saturating_add(model.output_tokens),
                // v1 exposes ordinary input separately, while the v3
                // component contract defines cached input as part of the
                // input total.  Preserve the root server's compatibility
                // mapping so cached <= input remains true for strict clients.
                input_tokens: model.input_tokens.saturating_add(model.cached_input_tokens),
                cached_input_tokens: model.cached_input_tokens,
                cache_write_input_tokens: None,
                output_tokens: model.output_tokens,
                // The v1 dollars are observations, not a v3 price-calculated
                // estimate.  Keep the compatibility projection nullable just
                // like the root server; the reader supplies a priced v3 row
                // when its durable token source is authoritative.
                estimated_cost: None,
            })
            .collect();
        Self::from_v2_with_models(details, &models)
    }
}

impl PublicDetailsV3 {
    /// Build a v3 envelope while retaining the generic model rows from the
    /// authoritative token projection.  The v1/v2 DTOs intentionally admit
    /// only the three legacy model columns, so converting through either one
    /// would otherwise silently discard ASTRA and additional model names.
    pub fn from_v2_with_models(details: &PublicDetailsV2, models: &[PublicModelUsageV3]) -> Self {
        let history_samples = details
            .history_samples
            .iter()
            .map(|sample| {
                let models = match (
                    sample.sol_tokens,
                    sample.terra_tokens,
                    sample.luna_tokens,
                    sample.sol_dollars,
                    sample.terra_dollars,
                    sample.luna_dollars,
                ) {
                    (
                        Some(sol_tokens),
                        Some(terra_tokens),
                        Some(luna_tokens),
                        Some(sol_dollars),
                        Some(terra_dollars),
                        Some(luna_dollars),
                    ) => Some(vec![
                        legacy_model("SOL", sol_tokens, sol_dollars),
                        legacy_model("TERRA", terra_tokens, terra_dollars),
                        legacy_model("LUNA", luna_tokens, luna_dollars),
                    ]),
                    _ => None,
                };
                PublicHistoryObservationV3 {
                    timestamp: sample.timestamp,
                    reset_at: sample.reset_at,
                    remaining_percent: sample.remaining_percent,
                    models,
                    models_complete: false,
                    model_source: sample.model_source.clone(),
                }
            })
            .collect();
        Self {
            state: details.state,
            observed_at: details.observed_at,
            authenticated: details.authenticated,
            plan_label: details.plan_label.clone(),
            quota: details.quota.clone(),
            models: models.to_vec(),
            active_thread_count: details.active_thread_count,
            history_periods: details.history_periods.clone(),
            history_samples,
            history_gaps: details.history_gaps.clone(),
            threads: details.threads.clone(),
        }
    }

    /// Build a v3 envelope with both the current generic model rows and the
    /// source-aware history graph supplied by the read-only database reader.
    /// This keeps sidecar model totals/provenance from being erased by the
    /// legacy v1/v2 adapter.
    pub fn from_v2_with_models_and_history(
        details: &PublicDetailsV2,
        models: &[PublicModelUsageV3],
        history_samples: &[PublicHistoryObservationV3],
    ) -> Self {
        let mut projected = Self::from_v2_with_models(details, models);
        projected.history_samples = history_samples.to_vec();
        projected
    }
}

fn legacy_model(model: &str, total_tokens: u64, total_dollars: f64) -> PublicHistoryModelUsageV3 {
    PublicHistoryModelUsageV3 {
        model: model.to_owned(),
        total_tokens,
        input_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        output_tokens: None,
        total_dollars: Some(total_dollars),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_details_are_bounded_and_serializable() {
        let details = PublicDetails::default();
        details.validate().expect("default contract");
        let value = serde_json::to_value(details).expect("serialize contract");
        assert!(value.get("state").is_some());
        assert!(value.get("api_version").is_none());
    }

    #[test]
    fn v2_and_v3_keep_legacy_unknown_explicit() {
        let mut details = PublicDetails {
            observed_at: Some(1_800_000_000),
            ..PublicDetails::default()
        };
        details.history_periods.push(PublicHistoryPeriod {
            id: "1800000060".to_owned(),
            start_at: 1_799_999_940,
            end_at: 1_800_000_000,
            reset_at: 1_800_000_060,
            label: "period".to_owned(),
            current: true,
        });
        details.history_samples.push(PublicHistorySample {
            timestamp: 1_800_000_000,
            reset_at: 1_800_000_060,
            remaining_percent: Some(50.0),
            sol_dollars: 1.0,
            terra_dollars: 2.0,
            luna_dollars: 3.0,
            sol_tokens: 10,
            terra_tokens: 20,
            luna_tokens: 30,
        });
        details.models.push(PublicDetailedModelUsage {
            name: "SOL".to_owned(),
            input_tokens: 60,
            cached_input_tokens: 40,
            output_tokens: 10,
            input_dollars: 1.0,
            cached_input_dollars: 2.0,
            output_dollars: 3.0,
        });
        details.validate().expect("fixture contract");
        let v2 = PublicDetailsV2::from(&details);
        assert_eq!(v2.history_samples[0].model_source, "legacy-unknown");
        let v3 = PublicDetailsV3::from(&v2);
        assert!(!v3.history_samples[0].models_complete);
        assert_eq!(v3.models[0].input_tokens, 100);
        assert_eq!(v3.models[0].cached_input_tokens, 40);
        assert_eq!(v3.models[0].total_tokens, 110);
    }
}

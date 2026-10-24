//! Qualified local consumption and saved observations, independent of a
//! provider's unproved usage-period identity. PRODUCT owns RECORDED-USAGE-SCOPE-604.

use crate::token_facts::TokenFacts;
use crate::{
    is_valid_public_model_name, ContractError, PublicModelCostV3, PublicQuota, PublicState,
};
use serde::{Deserialize, Serialize};

pub const MAX_END_EXCLUSIVE: i64 = 253_402_300_800;

/// Opt-in evidence about this application's Codex logs, never account quota.
pub const LOCAL_USAGE_HEADER: &str = "Codex-Info-History-Extensions";
pub const LOCAL_USAGE_EXTENSION: &str = "local-usage-v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalUsageState {
    Used,
    Unused,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalUsageInterval {
    pub start_at: i64,
    pub end_at: i64,
    pub state: LocalUsageState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalUsage {
    pub version: u8,
    pub start_at: i64,
    pub end_at: i64,
    pub intervals: Vec<LocalUsageInterval>,
}

impl LocalUsage {
    pub fn unavailable(start_at: i64, end_at: i64) -> Self {
        Self {
            version: 1,
            start_at,
            end_at,
            intervals: vec![LocalUsageInterval {
                start_at,
                end_at,
                state: LocalUsageState::Unavailable,
            }],
        }
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        if self.version != 1
            || self.start_at < 1
            || self.start_at >= self.end_at
            || self.end_at > MAX_END_EXCLUSIVE
        {
            return Err(ContractError::InvalidPeriod);
        }
        let mut cursor = self.start_at;
        for interval in &self.intervals {
            if interval.start_at != cursor
                || interval.end_at <= cursor
                || interval.end_at > self.end_at
            {
                return Err(ContractError::InvalidPeriod);
            }
            cursor = interval.end_at;
        }
        if cursor != self.end_at {
            return Err(ContractError::InvalidPeriod);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodIdentity {
    pub state: String,
    pub reason: String,
}

impl Default for PeriodIdentity {
    fn default() -> Self {
        Self {
            state: "unknown".into(),
            reason: "provider-period-identity-unavailable".into(),
        }
    }
}

impl PeriodIdentity {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self != &Self::default() {
            return Err(ContractError::InvalidPeriod);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordingCoverage {
    Complete,
    Partial,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingScope {
    pub kind: String,
    pub start_at: i64,
    pub end_at: i64,
    pub coverage: RecordingCoverage,
}

impl RecordingScope {
    pub fn new(
        start_at: i64,
        end_at: i64,
        coverage: RecordingCoverage,
    ) -> Result<Self, ContractError> {
        let scope = Self {
            kind: "recorded-time".into(),
            start_at,
            end_at,
            coverage,
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        if self.kind != "recorded-time"
            || self.start_at < 1
            || self.start_at >= self.end_at
            || self.end_at > MAX_END_EXCLUSIVE
        {
            return Err(ContractError::InvalidTimestamp);
        }
        Ok(())
    }

    pub fn contains(&self, timestamp: i64) -> bool {
        self.start_at <= timestamp && timestamp < self.end_at
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedModelUsage {
    pub model: String,
    pub total_tokens: u64,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub estimated_cost: Option<PublicModelCostV3>,
}

impl RecordedModelUsage {
    pub fn from_tokens(model: String, tokens: TokenFacts) -> Result<Self, ContractError> {
        let value = Self {
            model,
            total_tokens: tokens.total,
            input_tokens: tokens.input,
            cached_input_tokens: tokens.cached_input,
            cache_write_input_tokens: tokens.cache_write_input,
            output_tokens: tokens.output,
            estimated_cost: None,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn tokens(&self) -> TokenFacts {
        TokenFacts {
            total: self.total_tokens,
            input: self.input_tokens,
            cached_input: self.cached_input_tokens,
            cache_write_input: self.cache_write_input_tokens,
            output: self.output_tokens,
        }
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        if !is_valid_public_model_name(&self.model) {
            return Err(ContractError::InvalidModel);
        }
        self.tokens()
            .validate_fact()
            .map_err(|_| ContractError::InvalidNumber)?;
        if let Some(cost) = &self.estimated_cost {
            if self.input_tokens.is_none()
                || self.cached_input_tokens.is_none()
                || self.cache_write_input_tokens.is_none()
                || self.output_tokens.is_none()
                || cost.price_version.is_empty()
                || [
                    cost.ordinary_input_dollars,
                    cost.cached_input_dollars,
                    cost.cache_write_input_dollars,
                    cost.output_dollars,
                    cost.total_dollars,
                ]
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
            {
                return Err(ContractError::InvalidNumber);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedCurrent {
    pub api_version: String,
    pub state: PublicState,
    pub observed_at: Option<i64>,
    pub authenticated: bool,
    pub plan_label: Option<String>,
    pub quota: Option<PublicQuota>,
    pub period_identity: PeriodIdentity,
    pub usage_scope: Option<RecordingScope>,
    pub models_status: String,
    pub models: Vec<RecordedModelUsage>,
    pub active_thread_count: u64,
    pub open_session_thread_count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedSample {
    pub timestamp: i64,
    pub models: Vec<RecordedModelUsage>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageInterval {
    pub start_at: i64,
    pub end_at: i64,
    pub coverage: RecordingCoverage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaObservation {
    pub sequence: String,
    pub observed_at: i64,
    pub reset_at: i64,
    pub window_seconds: i64,
    pub remaining_percent: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredModelObservation {
    pub stored_model_key: String,
    pub total_tokens: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_dollars: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredObservation {
    pub observation_id: String,
    pub timestamp: i64,
    pub raw_reset_at: Option<i64>,
    pub storage_kind: String,
    pub models: Vec<StoredModelObservation>,
    pub unresolved_reasons: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingScopeEntry {
    pub id: String,
    pub kind: String,
    pub start_at: i64,
    pub end_at: i64,
    pub current: bool,
    pub recordings_available: bool,
    pub saved_observations_available: bool,
    pub coverage: RecordingCoverage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingScopeDirectory {
    pub api_version: String,
    pub observed_at: Option<i64>,
    pub period_identity: PeriodIdentity,
    pub scopes: Vec<RecordingScopeEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingHistoryPage {
    pub api_version: String,
    pub scope: RecordingScope,
    pub period_identity: PeriodIdentity,
    pub coverage_intervals: Vec<CoverageInterval>,
    pub samples: Vec<RecordedSample>,
    pub quota_observations: Vec<QuotaObservation>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredObservationsPage {
    pub api_version: String,
    pub scope: RecordingScope,
    pub period_identity: PeriodIdentity,
    pub observations: Vec<StoredObservation>,
    pub next_cursor: Option<String>,
}

/// Internal persisted prefix: decimal strings keep sums lossless beyond one
/// public UInt64. Unknown counts allow a range to exclude an earlier unknown
/// component without converting that unknown into zero.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixComponent {
    pub known_sum: String,
    pub unknown_count: u64,
}

impl Default for PrefixComponent {
    fn default() -> Self {
        Self {
            known_sum: "0".into(),
            unknown_count: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedTokenPrefix {
    pub event_count: u64,
    pub total: String,
    pub input: PrefixComponent,
    pub cached_input: PrefixComponent,
    pub cache_write_input: PrefixComponent,
    pub output: PrefixComponent,
}

impl Default for RecordedTokenPrefix {
    fn default() -> Self {
        Self {
            event_count: 0,
            total: "0".into(),
            input: PrefixComponent::default(),
            cached_input: PrefixComponent::default(),
            cache_write_input: PrefixComponent::default(),
            output: PrefixComponent::default(),
        }
    }
}

fn quantity(value: &str) -> Result<u128, ContractError> {
    let parsed = value
        .parse::<u128>()
        .map_err(|_| ContractError::InvalidNumber)?;
    if parsed.to_string() != value {
        return Err(ContractError::InvalidNumber);
    }
    Ok(parsed)
}

impl RecordedTokenPrefix {
    pub fn validate(&self) -> Result<(), ContractError> {
        let total = quantity(&self.total)?;
        for component in [
            &self.input,
            &self.cached_input,
            &self.cache_write_input,
            &self.output,
        ] {
            if quantity(&component.known_sum)? > total || component.unknown_count > self.event_count
            {
                return Err(ContractError::InvalidNumber);
            }
        }
        let input = quantity(&self.input.known_sum)?;
        let output = quantity(&self.output.known_sum)?;
        let components = input
            .checked_add(output)
            .ok_or(ContractError::InvalidNumber)?;
        let cached = quantity(&self.cached_input.known_sum)?
            .checked_add(quantity(&self.cache_write_input.known_sum)?)
            .ok_or(ContractError::InvalidNumber)?;
        if components > total
            || (self.input.unknown_count == 0
                && self.output.unknown_count == 0
                && components != total)
            || cached
                > if self.input.unknown_count == 0 {
                    input
                } else {
                    total
                }
            || (self.event_count == 0 && self != &Self::default())
        {
            return Err(ContractError::InvalidNumber);
        }
        Ok(())
    }

    pub fn add(&mut self, tokens: TokenFacts) -> Result<(), ContractError> {
        tokens
            .validate_fact()
            .map_err(|_| ContractError::InvalidNumber)?;
        self.validate()?;
        let mut candidate = self.clone();
        candidate.event_count = candidate
            .event_count
            .checked_add(1)
            .ok_or(ContractError::InvalidNumber)?;
        candidate.total = quantity(&candidate.total)?
            .checked_add(u128::from(tokens.total))
            .ok_or(ContractError::InvalidNumber)?
            .to_string();
        for (component, value) in [
            (&mut candidate.input, tokens.input),
            (&mut candidate.cached_input, tokens.cached_input),
            (&mut candidate.cache_write_input, tokens.cache_write_input),
            (&mut candidate.output, tokens.output),
        ] {
            if let Some(value) = value {
                component.known_sum = quantity(&component.known_sum)?
                    .checked_add(u128::from(value))
                    .ok_or(ContractError::InvalidNumber)?
                    .to_string();
            } else {
                component.unknown_count = component
                    .unknown_count
                    .checked_add(1)
                    .ok_or(ContractError::InvalidNumber)?;
            }
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub fn difference(&self, before: &Self) -> Result<Option<TokenFacts>, ContractError> {
        self.validate()?;
        before.validate()?;
        let count = self
            .event_count
            .checked_sub(before.event_count)
            .ok_or(ContractError::InvalidNumber)?;
        let difference = |after: &str, before: &str| -> Result<u64, ContractError> {
            let value = quantity(after)?
                .checked_sub(quantity(before)?)
                .ok_or(ContractError::InvalidNumber)?;
            u64::try_from(value).map_err(|_| ContractError::InvalidNumber)
        };
        let component = |after: &PrefixComponent,
                         before: &PrefixComponent|
         -> Result<Option<u64>, ContractError> {
            let unknown = after
                .unknown_count
                .checked_sub(before.unknown_count)
                .ok_or(ContractError::InvalidNumber)?;
            let known = quantity(&after.known_sum)?
                .checked_sub(quantity(&before.known_sum)?)
                .ok_or(ContractError::InvalidNumber)?;
            if unknown == 0 {
                u64::try_from(known)
                    .map(Some)
                    .map_err(|_| ContractError::InvalidNumber)
            } else {
                Ok(None)
            }
        };
        let tokens = TokenFacts {
            total: difference(&self.total, &before.total)?,
            input: component(&self.input, &before.input)?,
            cached_input: component(&self.cached_input, &before.cached_input)?,
            cache_write_input: component(&self.cache_write_input, &before.cache_write_input)?,
            output: component(&self.output, &before.output)?,
        };
        if count == 0 {
            if self != before {
                return Err(ContractError::InvalidNumber);
            }
            return Ok(None);
        }
        tokens
            .validate_fact()
            .map_err(|_| ContractError::InvalidNumber)?;
        Ok(Some(tokens))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_prefix_retains_nullable_range_facts_and_first_record() {
        let a = TokenFacts {
            total: 10,
            input: None,
            cached_input: Some(2),
            cache_write_input: Some(0),
            output: Some(2),
        };
        let b = TokenFacts {
            total: 5,
            input: Some(4),
            cached_input: Some(1),
            cache_write_input: Some(0),
            output: Some(1),
        };
        let mut prefix = RecordedTokenPrefix::default();
        prefix.add(a).unwrap();
        let before = prefix.clone();
        prefix.add(b).unwrap();
        assert_eq!(
            prefix
                .difference(&RecordedTokenPrefix::default())
                .unwrap()
                .unwrap(),
            TokenFacts {
                total: 15,
                input: None,
                cached_input: Some(3),
                cache_write_input: Some(0),
                output: Some(3)
            }
        );
        assert_eq!(prefix.difference(&before).unwrap(), Some(b));
        assert_eq!(
            before.difference(&RecordedTokenPrefix::default()).unwrap(),
            Some(a)
        );
        assert_eq!(before.difference(&before).unwrap(), None);
        let mut invalid = prefix.clone();
        invalid.total = "14".into();
        invalid.input.known_sum = "14".into();
        assert!(invalid.validate().is_err());
        assert!(prefix.difference(&invalid).is_err());
    }

    #[test]
    fn scope_prefix_can_exclude_old_global_overflow_without_clamping() {
        let huge = TokenFacts {
            total: u64::MAX,
            input: Some(u64::MAX),
            cached_input: Some(0),
            cache_write_input: Some(0),
            output: Some(0),
        };
        let small = TokenFacts {
            total: 5,
            input: Some(5),
            cached_input: Some(0),
            cache_write_input: Some(0),
            output: Some(0),
        };
        let mut after = RecordedTokenPrefix::default();
        after.add(huge).unwrap();
        let before = after.clone();
        after.add(small).unwrap();
        assert_eq!(after.difference(&before).unwrap(), Some(small));
        assert!(after.difference(&RecordedTokenPrefix::default()).is_err());
    }
}

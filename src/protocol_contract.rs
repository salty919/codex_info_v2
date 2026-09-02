// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! Schema-first account and quota decoding for the vendored app-server v2
//! protocol.
//!
//! The hashes below are deliberately constants rather than a runtime fallback:
//! changing the generated protocol is a replacement-gate event, not something
//! this decoder silently follows.

use std::fmt;

use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use crate::security;

pub const SCHEMA_MANIFEST: &str = "CODEX_INFO_PROTOCOL_SCHEMA_MANIFEST_V1";
pub const CODEX_CLI_VERSION: &str = "0.147.0";
pub const GENERATED_UTC_DATE: &str = "2026-08-14";
pub const SCHEMA_BUNDLE_SHA256: &str =
    "f3dec1e031d99a420b137b903f02196d4325eece57620c925bb7130b25f168d2";

pub const GET_ACCOUNT_RESPONSE_VENDORED_SHA256: &str =
    "9274e7c57af620183f775f56dfa2f9061329f46420ee8f40df7c536676a47280";
pub const GET_ACCOUNT_RATE_LIMITS_RESPONSE_VENDORED_SHA256: &str =
    "92e7bc01aee38d2f0c483a2d3812620aa99703c752cc0bafea0ae30096ea6390";
pub const PLAN_TYPE_VENDORED_SHA256: &str =
    "585f8053090425683e665307b22b7f2793e17f93340361bb1c00b95320d9643c";

const DEFAULT_LIMIT_NAME: &str = "Codex";
const WEEK_SECONDS: i64 = 7 * 86_400;
const MAX_WINDOW_DURATION_MINS: i64 = 527_040;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractError {
    AccountSchema,
    AccountUpdatedSchema,
    QuotaSchema,
    InvalidPlanType,
    AccountQuotaPlanMismatch,
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::AccountSchema => "account response schema contract violation",
            Self::AccountUpdatedSchema => "account updated notification schema contract violation",
            Self::QuotaSchema => "quota response schema contract violation",
            Self::InvalidPlanType => "plan type contract violation",
            Self::AccountQuotaPlanMismatch => "account and quota plan contract mismatch",
        };
        formatter.write_str(message)
    }
}

/// Validates the exact app-server v2 `account/updated` notification used as
/// the local account-generation boundary. Unknown or duplicate JSON members
/// are rejected by the caller's JSON parser/object cardinality check rather
/// than being treated as a harmless notification.
pub fn validate_account_updated_notification(value: &Value) -> Result<(), ContractError> {
    const AUTH_MODES: [&str; 7] = [
        "apikey",
        "chatgpt",
        "chatgptAuthTokens",
        "headers",
        "agentIdentity",
        "personalAccessToken",
        "bedrockApiKey",
    ];
    let message = value
        .as_object()
        .ok_or(ContractError::AccountUpdatedSchema)?;
    if message.len() != 3
        || message.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || message.get("method").and_then(Value::as_str) != Some("account/updated")
        || message.contains_key("id")
    {
        return Err(ContractError::AccountUpdatedSchema);
    }
    let params = message
        .get("params")
        .and_then(Value::as_object)
        .ok_or(ContractError::AccountUpdatedSchema)?;
    if params
        .keys()
        .any(|key| key != "authMode" && key != "planType")
    {
        return Err(ContractError::AccountUpdatedSchema);
    }
    if let Some(auth_mode) = params.get("authMode") {
        if !auth_mode.is_null()
            && !auth_mode
                .as_str()
                .is_some_and(|mode| AUTH_MODES.contains(&mode))
        {
            return Err(ContractError::AccountUpdatedSchema);
        }
    }
    if let Some(plan_type) = params.get("planType") {
        if !plan_type.is_null() && !plan_type.as_str().and_then(PlanType::parse).is_some() {
            return Err(ContractError::AccountUpdatedSchema);
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAccountUpdatedEnvelope {
    jsonrpc: String,
    method: String,
    params: StrictAccountUpdatedParams,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictAccountUpdatedParams {
    auth_mode: Option<String>,
    plan_type: Option<String>,
}

pub fn is_account_updated_notification_json(raw: &str) -> Result<bool, ContractError> {
    struct MethodVisitor;

    impl<'de> Visitor<'de> for MethodVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a JSON-RPC object")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut method_seen = false;
            let mut account_updated = false;
            while let Some(key) = map.next_key::<String>()? {
                if key == "method" {
                    let method = map.next_value::<Value>()?;
                    let current = method.as_str() == Some("account/updated");
                    if method_seen && (account_updated || current) {
                        return Err(serde::de::Error::duplicate_field("method"));
                    }
                    method_seen = true;
                    account_updated |= current;
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(account_updated)
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let result = deserializer
        .deserialize_map(MethodVisitor)
        .map_err(|_| ContractError::AccountUpdatedSchema)?;
    deserializer
        .end()
        .map_err(|_| ContractError::AccountUpdatedSchema)?;
    Ok(result)
}

pub fn validate_account_updated_notification_json(raw: &str) -> Result<(), ContractError> {
    let envelope = serde_json::from_str::<StrictAccountUpdatedEnvelope>(raw)
        .map_err(|_| ContractError::AccountUpdatedSchema)?;
    let _ = (&envelope.params.auth_mode, &envelope.params.plan_type);
    if envelope.jsonrpc != "2.0" || envelope.method != "account/updated" {
        return Err(ContractError::AccountUpdatedSchema);
    }
    let value =
        serde_json::from_str::<Value>(raw).map_err(|_| ContractError::AccountUpdatedSchema)?;
    validate_account_updated_notification(&value)
}

impl std::error::Error for ContractError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanFamily {
    Free,
    Go,
    Plus,
    Pro,
    ProLite,
    Team,
    Business,
    Enterprise,
    Edu,
    Unset,
}

impl PlanFamily {
    pub const fn is_known(self) -> bool {
        !matches!(self, Self::Unset)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanType {
    Free,
    Go,
    Plus,
    Pro,
    ProLite,
    Team,
    SelfServeBusinessProLite,
    SelfServeBusinessUsageBased,
    Business,
    Ent26,
    EnterpriseCbpAutomation,
    EnterpriseCbpUsageBased,
    Enterprise,
    Edu,
    Unknown,
}

impl PlanType {
    pub const VALUES: [&'static str; 15] = [
        "free",
        "go",
        "plus",
        "pro",
        "prolite",
        "team",
        "self_serve_business_prolite",
        "self_serve_business_usage_based",
        "business",
        "ent26",
        "enterprise_cbp_automation",
        "enterprise_cbp_usage_based",
        "enterprise",
        "edu",
        "unknown",
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "free" => Self::Free,
            "go" => Self::Go,
            "plus" => Self::Plus,
            "pro" => Self::Pro,
            "prolite" => Self::ProLite,
            "team" => Self::Team,
            "self_serve_business_prolite" => Self::SelfServeBusinessProLite,
            "self_serve_business_usage_based" => Self::SelfServeBusinessUsageBased,
            "business" => Self::Business,
            "ent26" => Self::Ent26,
            "enterprise_cbp_automation" => Self::EnterpriseCbpAutomation,
            "enterprise_cbp_usage_based" => Self::EnterpriseCbpUsageBased,
            "enterprise" => Self::Enterprise,
            "edu" => Self::Edu,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Go => "go",
            Self::Plus => "plus",
            Self::Pro => "pro",
            Self::ProLite => "prolite",
            Self::Team => "team",
            Self::SelfServeBusinessProLite => "self_serve_business_prolite",
            Self::SelfServeBusinessUsageBased => "self_serve_business_usage_based",
            Self::Business => "business",
            Self::Ent26 => "ent26",
            Self::EnterpriseCbpAutomation => "enterprise_cbp_automation",
            Self::EnterpriseCbpUsageBased => "enterprise_cbp_usage_based",
            Self::Enterprise => "enterprise",
            Self::Edu => "edu",
            Self::Unknown => "unknown",
        }
    }

    pub const fn family(self) -> PlanFamily {
        match self {
            Self::Free => PlanFamily::Free,
            Self::Go => PlanFamily::Go,
            Self::Plus => PlanFamily::Plus,
            Self::Pro => PlanFamily::Pro,
            Self::ProLite => PlanFamily::ProLite,
            Self::Team => PlanFamily::Team,
            Self::SelfServeBusinessProLite | Self::SelfServeBusinessUsageBased | Self::Business => {
                PlanFamily::Business
            }
            Self::Ent26
            | Self::EnterpriseCbpAutomation
            | Self::EnterpriseCbpUsageBased
            | Self::Enterprise => PlanFamily::Enterprise,
            Self::Edu => PlanFamily::Edu,
            Self::Unknown => PlanFamily::Unset,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Free => "無料",
            Self::Go => "Go",
            Self::Plus => "Plus",
            Self::Pro => "Pro",
            Self::ProLite => "Pro Lite",
            Self::Team => "Team",
            Self::SelfServeBusinessProLite | Self::SelfServeBusinessUsageBased | Self::Business => {
                "Business"
            }
            Self::Ent26
            | Self::EnterpriseCbpAutomation
            | Self::EnterpriseCbpUsageBased
            | Self::Enterprise => "エンタープライズ",
            Self::Edu => "教育",
            Self::Unknown => "プラン未設定",
        }
    }
}

pub fn plan_type_from_wire(value: Option<&str>) -> Option<PlanType> {
    value.and_then(PlanType::parse)
}

pub fn plan_family_from_wire(value: Option<&str>) -> Option<PlanFamily> {
    plan_type_from_wire(value).map(PlanType::family)
}

pub fn plan_label(value: Option<&str>) -> String {
    value
        .and_then(PlanType::parse)
        .map_or_else(|| "プラン未設定".to_owned(), |plan| plan.label().to_owned())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountOutcome {
    AuthRequired,
    Supported { email: String, plan_type: PlanType },
    UnsupportedNoData,
}

impl AccountOutcome {
    pub fn plan_family(&self) -> Option<PlanFamily> {
        match self {
            Self::Supported { plan_type, .. } => Some(plan_type.family()),
            Self::AuthRequired | Self::UnsupportedNoData => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaSnapshot {
    pub fixed_used_percent: Option<i32>,
    pub remaining_percent: Option<i32>,
    pub reset_at: i64,
    pub window_seconds: i64,
    pub limit_name: String,
    pub monthly: bool,
    pub unlimited: bool,
}

#[derive(Clone, Copy)]
enum Endpoint {
    Account,
    Quota,
}

impl Endpoint {
    const fn error(self) -> ContractError {
        match self {
            Self::Account => ContractError::AccountSchema,
            Self::Quota => ContractError::QuotaSchema,
        }
    }
}

fn object(value: &Value, endpoint: Endpoint) -> Result<&Map<String, Value>, ContractError> {
    value.as_object().ok_or_else(|| endpoint.error())
}

fn required<'a>(
    values: &'a Map<String, Value>,
    key: &str,
    endpoint: Endpoint,
) -> Result<&'a Value, ContractError> {
    values.get(key).ok_or_else(|| endpoint.error())
}

fn is_integer(value: &Value) -> bool {
    value.as_i64().is_some()
}

fn is_i32(value: &Value) -> bool {
    value
        .as_i64()
        .and_then(|number| i32::try_from(number).ok())
        .is_some()
}

fn is_nullable_string(value: &Value) -> bool {
    value.is_null() || value.is_string()
}

fn is_nullable_i64(value: &Value) -> bool {
    value.is_null() || is_integer(value)
}

fn is_nullable_bool(value: &Value) -> bool {
    value.is_null() || value.is_boolean()
}

fn validate_plan(value: &Value, endpoint: Endpoint) -> Result<(), ContractError> {
    if value.as_str().and_then(PlanType::parse).is_some() {
        Ok(())
    } else {
        Err(endpoint.error())
    }
}

fn validate_account_variant(value: &Value) -> Result<(), ContractError> {
    let endpoint = Endpoint::Account;
    let values = object(value, endpoint)?;
    let account_type = required(values, "type", endpoint)?
        .as_str()
        .ok_or_else(|| endpoint.error())?;

    match account_type {
        "apiKey" => Ok(()),
        "chatgpt" => {
            let email = required(values, "email", endpoint)?;
            if !is_nullable_string(email) {
                return Err(endpoint.error());
            }
            validate_plan(required(values, "planType", endpoint)?, endpoint)
        }
        "amazonBedrock" => {
            if let Some(value) = values.get("usesCodexManagedCredentials") {
                if !value.is_boolean() {
                    return Err(endpoint.error());
                }
            }
            Ok(())
        }
        _ => Err(endpoint.error()),
    }
}

pub fn validate_account_response(value: &Value) -> Result<(), ContractError> {
    let endpoint = Endpoint::Account;
    let values = object(value, endpoint)?;
    if !required(values, "requiresOpenaiAuth", endpoint)?.is_boolean() {
        return Err(endpoint.error());
    }
    if let Some(account) = values.get("account") {
        if !account.is_null() {
            validate_account_variant(account)?;
        }
    }
    Ok(())
}

pub fn decode_account(value: &Value) -> Result<AccountOutcome, ContractError> {
    validate_account_response(value)?;
    let values = object(value, Endpoint::Account)?;
    let requires_openai_auth =
        required(values, "requiresOpenaiAuth", Endpoint::Account)?.as_bool() == Some(true);

    if let Some(account) = values.get("account").and_then(Value::as_object) {
        if account.get("type").and_then(Value::as_str) == Some("chatgpt") {
            let email = account
                .get("email")
                .and_then(Value::as_str)
                .and_then(|email| security::bounded_email(email).ok())
                .filter(|email| !email.trim().is_empty());
            let plan_type = account
                .get("planType")
                .and_then(Value::as_str)
                .and_then(PlanType::parse)
                .ok_or(ContractError::AccountSchema)?;
            if let Some(email) = email {
                // In the real 0.147.0 response this capability flag remains true
                // for an authenticated ChatGPT account.  Account presence is the
                // authentication state; the flag alone is not a logout marker.
                return Ok(AccountOutcome::Supported { email, plan_type });
            }
        }
    }

    if requires_openai_auth {
        Ok(AccountOutcome::AuthRequired)
    } else {
        Ok(AccountOutcome::UnsupportedNoData)
    }
}

fn validate_credits(value: &Value) -> Result<(), ContractError> {
    let endpoint = Endpoint::Quota;
    if value.is_null() {
        return Ok(());
    }
    let values = object(value, endpoint)?;
    if !required(values, "hasCredits", endpoint)?.is_boolean()
        || !required(values, "unlimited", endpoint)?.is_boolean()
    {
        return Err(endpoint.error());
    }
    if let Some(balance) = values.get("balance") {
        if !is_nullable_string(balance) {
            return Err(endpoint.error());
        }
    }
    Ok(())
}

fn validate_spend_control(value: &Value) -> Result<(), ContractError> {
    let endpoint = Endpoint::Quota;
    if value.is_null() {
        return Ok(());
    }
    let values = object(value, endpoint)?;
    if !required(values, "limit", endpoint)?.is_string()
        || !is_i32(required(values, "remainingPercent", endpoint)?)
        || !is_integer(required(values, "resetsAt", endpoint)?)
        || !required(values, "used", endpoint)?.is_string()
    {
        return Err(endpoint.error());
    }
    Ok(())
}

fn validate_window(value: &Value) -> Result<(), ContractError> {
    let endpoint = Endpoint::Quota;
    if value.is_null() {
        return Ok(());
    }
    let values = object(value, endpoint)?;
    if !is_i32(required(values, "usedPercent", endpoint)?) {
        return Err(endpoint.error());
    }
    if let Some(resets_at) = values.get("resetsAt") {
        if !is_nullable_i64(resets_at) {
            return Err(endpoint.error());
        }
    }
    if let Some(duration) = values.get("windowDurationMins") {
        if !is_nullable_i64(duration) {
            return Err(endpoint.error());
        }
    }
    Ok(())
}

fn validate_rate_limit_snapshot(value: &Value) -> Result<(), ContractError> {
    let endpoint = Endpoint::Quota;
    let values = object(value, endpoint)?;
    if let Some(credits) = values.get("credits") {
        validate_credits(credits)?;
    }
    if let Some(individual) = values.get("individualLimit") {
        validate_spend_control(individual)?;
    }
    if let Some(limit_id) = values.get("limitId") {
        if !is_nullable_string(limit_id) {
            return Err(endpoint.error());
        }
    }
    if let Some(limit_name) = values.get("limitName") {
        if !is_nullable_string(limit_name) {
            return Err(endpoint.error());
        }
    }
    if let Some(plan_type) = values.get("planType") {
        if plan_type.is_null() {
            // nullable union
        } else {
            validate_plan(plan_type, endpoint)?;
        }
    }
    if let Some(primary) = values.get("primary") {
        validate_window(primary)?;
    }
    if let Some(secondary) = values.get("secondary") {
        validate_window(secondary)?;
    }
    if let Some(reached_type) = values.get("rateLimitReachedType") {
        if !reached_type.is_null()
            && !matches!(
                reached_type.as_str(),
                Some(
                    "rate_limit_reached"
                        | "workspace_owner_credits_depleted"
                        | "workspace_member_credits_depleted"
                        | "workspace_owner_usage_limit_reached"
                        | "workspace_member_usage_limit_reached"
                )
            )
        {
            return Err(endpoint.error());
        }
    }
    if let Some(spend_control_reached) = values.get("spendControlReached") {
        if !is_nullable_bool(spend_control_reached) {
            return Err(endpoint.error());
        }
    }
    Ok(())
}

fn validate_reset_credit(value: &Value) -> Result<(), ContractError> {
    let endpoint = Endpoint::Quota;
    let values = object(value, endpoint)?;
    if !is_integer(required(values, "grantedAt", endpoint)?)
        || !required(values, "id", endpoint)?.is_string()
        || !matches!(
            required(values, "resetType", endpoint)?.as_str(),
            Some("codexRateLimits" | "unknown")
        )
        || !matches!(
            required(values, "status", endpoint)?.as_str(),
            Some("available" | "redeeming" | "redeemed" | "unknown")
        )
    {
        return Err(endpoint.error());
    }
    for key in ["description", "title"] {
        if let Some(value) = values.get(key) {
            if !is_nullable_string(value) {
                return Err(endpoint.error());
            }
        }
    }
    if let Some(expires_at) = values.get("expiresAt") {
        if !is_nullable_i64(expires_at) {
            return Err(endpoint.error());
        }
    }
    Ok(())
}

fn validate_reset_credits(value: &Value) -> Result<(), ContractError> {
    let endpoint = Endpoint::Quota;
    if value.is_null() {
        return Ok(());
    }
    let values = object(value, endpoint)?;
    if !is_integer(required(values, "availableCount", endpoint)?) {
        return Err(endpoint.error());
    }
    if let Some(credits) = values.get("credits") {
        if credits.is_null() {
            return Ok(());
        }
        let credits = credits.as_array().ok_or_else(|| endpoint.error())?;
        for credit in credits {
            validate_reset_credit(credit)?;
        }
    }
    Ok(())
}

pub fn validate_rate_limits_response(value: &Value) -> Result<(), ContractError> {
    let endpoint = Endpoint::Quota;
    let values = object(value, endpoint)?;
    validate_rate_limit_snapshot(required(values, "rateLimits", endpoint)?)?;

    if let Some(by_id) = values.get("rateLimitsByLimitId") {
        if !by_id.is_null() {
            let by_id = by_id.as_object().ok_or_else(|| endpoint.error())?;
            for snapshot in by_id.values() {
                validate_rate_limit_snapshot(snapshot)?;
            }
        }
    }
    if let Some(reset_credits) = values.get("rateLimitResetCredits") {
        validate_reset_credits(reset_credits)?;
    }
    Ok(())
}

fn bounded_limit_name(value: Option<&Value>) -> Result<String, ContractError> {
    let Some(value) = value else {
        return Ok(DEFAULT_LIMIT_NAME.to_owned());
    };
    if value.is_null() {
        return Ok(DEFAULT_LIMIT_NAME.to_owned());
    }
    let value = value.as_str().ok_or(ContractError::QuotaSchema)?;
    let value = security::bounded_limit_name(value).map_err(|_| ContractError::QuotaSchema)?;
    if value.trim().is_empty() {
        Ok(DEFAULT_LIMIT_NAME.to_owned())
    } else {
        Ok(value)
    }
}

fn floor_div(value: i128, divisor: i128) -> i128 {
    let quotient = value / divisor;
    if value % divisor < 0 {
        quotient - 1
    } else {
        quotient
    }
}

fn civil_date_from_unix_days(days: i128) -> (i128, i128, i128) {
    let z = days + 719_468;
    let era = floor_div(z, 146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

fn unix_days_from_civil_date(year: i128, month: i128, day: i128) -> i128 {
    let adjusted_year = year - if month <= 2 { 1 } else { 0 };
    let era = floor_div(adjusted_year, 400);
    let year_of_era = adjusted_year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn is_leap_year(year: i128) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i128, month: i128) -> i128 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => unreachable!("month must be in the range 1..=12"),
    }
}

fn monthly_window_seconds(reset_at: i64) -> i64 {
    let timestamp = i128::from(reset_at);
    let unix_days = timestamp.div_euclid(86_400);
    let seconds_of_day = timestamp.rem_euclid(86_400);
    let (year, month, day) = civil_date_from_unix_days(unix_days);
    let (previous_year, previous_month) = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    let previous_day = day.min(days_in_month(previous_year, previous_month));
    let previous_timestamp = unix_days_from_civil_date(previous_year, previous_month, previous_day)
        * 86_400
        + seconds_of_day;
    let difference = timestamp - previous_timestamp;
    debug_assert!(difference > 0 && difference <= 31 * 86_400);
    i64::try_from(difference).expect("calendar month duration must fit in i64")
}

fn valid_individual(value: &Map<String, Value>) -> Option<(i32, i64)> {
    let individual = value.get("individualLimit")?.as_object()?;
    let remaining = individual
        .get("remainingPercent")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| (0..=100).contains(value))?;
    let reset_at = individual
        .get("resetsAt")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 1)?;
    Some((remaining, reset_at))
}

#[derive(Clone, Copy)]
struct FixedCandidate {
    used: i32,
    remaining: i32,
    reset_at: i64,
    window_seconds: i64,
    priority: u8,
}

fn valid_fixed(value: &Map<String, Value>, key: &str, priority: u8) -> Option<FixedCandidate> {
    let window = value.get(key)?.as_object()?;
    let used = window
        .get("usedPercent")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| (0..=100).contains(value))?;
    let reset_at = window
        .get("resetsAt")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 1)?;
    let duration_mins = window
        .get("windowDurationMins")
        .and_then(Value::as_i64)
        .filter(|value| (1..=MAX_WINDOW_DURATION_MINS).contains(value))?;
    let window_seconds = duration_mins.checked_mul(60)?;
    Some(FixedCandidate {
        used,
        remaining: 100 - used,
        reset_at,
        window_seconds,
        priority,
    })
}

fn fixed_is_better(candidate: FixedCandidate, current: FixedCandidate) -> bool {
    candidate
        .window_seconds
        .cmp(&current.window_seconds)
        .then_with(|| candidate.reset_at.cmp(&current.reset_at))
        .then_with(|| current.priority.cmp(&candidate.priority))
        .is_gt()
}

fn canonical_plan_family(values: &Map<String, Value>) -> Result<Option<PlanFamily>, ContractError> {
    let Some(plan_type) = values.get("planType") else {
        return Ok(None);
    };
    if plan_type.is_null() {
        return Ok(None);
    }
    Ok(plan_type
        .as_str()
        .and_then(PlanType::parse)
        .map(PlanType::family))
}

pub fn decode_quota(
    value: &Value,
    account_family: Option<PlanFamily>,
) -> Result<Option<QuotaSnapshot>, ContractError> {
    validate_rate_limits_response(value)?;
    let values = object(value, Endpoint::Quota)?;
    let canonical = required(values, "rateLimits", Endpoint::Quota)?
        .as_object()
        .ok_or(ContractError::QuotaSchema)?;
    let quota_family = canonical_plan_family(canonical)?;

    if let (Some(account_family), Some(quota_family)) = (account_family, quota_family) {
        if account_family.is_known() && quota_family.is_known() && account_family != quota_family {
            return Err(ContractError::AccountQuotaPlanMismatch);
        }
    }
    let effective_family = account_family
        .filter(|family| family.is_known())
        .or_else(|| quota_family.filter(|family| family.is_known()))
        .unwrap_or(PlanFamily::Unset);
    let limit_name = bounded_limit_name(canonical.get("limitName"))?;

    if effective_family == PlanFamily::Enterprise {
        if let Some((remaining, reset_at)) = valid_individual(canonical) {
            return Ok(Some(QuotaSnapshot {
                fixed_used_percent: None,
                remaining_percent: Some(remaining),
                reset_at,
                window_seconds: monthly_window_seconds(reset_at),
                limit_name,
                monthly: true,
                unlimited: false,
            }));
        }
    }

    let mut selected = None;
    for (key, priority) in [("primary", 0), ("secondary", 1)] {
        if let Some(candidate) = valid_fixed(canonical, key, priority) {
            if selected.is_none_or(|current| fixed_is_better(candidate, current)) {
                selected = Some(candidate);
            }
        }
    }
    if let Some(candidate) = selected {
        return Ok(Some(QuotaSnapshot {
            fixed_used_percent: Some(candidate.used),
            remaining_percent: Some(candidate.remaining),
            reset_at: candidate.reset_at,
            window_seconds: candidate.window_seconds,
            limit_name,
            monthly: false,
            unlimited: false,
        }));
    }

    let unlimited = canonical
        .get("credits")
        .and_then(Value::as_object)
        .and_then(|credits| credits.get("unlimited"))
        .and_then(Value::as_bool)
        == Some(true);
    if unlimited {
        return Ok(Some(QuotaSnapshot {
            fixed_used_percent: None,
            remaining_percent: None,
            reset_at: 0,
            window_seconds: WEEK_SECONDS,
            limit_name,
            monthly: false,
            unlimited: true,
        }));
    }
    Ok(None)
}

pub fn decode_quota_for_plan(
    value: &Value,
    account_plan: Option<&str>,
) -> Result<Option<QuotaSnapshot>, ContractError> {
    validate_rate_limits_response(value)?;
    let account_family = match account_plan {
        Some(plan) => Some(
            PlanType::parse(plan)
                .ok_or(ContractError::InvalidPlanType)?
                .family(),
        ),
        None => None,
    };
    decode_quota(value, account_family)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Map, Value};

    use super::*;

    fn fixed_snapshot(used: i32, reset_at: i64, minutes: i64) -> Value {
        json!({
            "primary": {
                "usedPercent": used,
                "resetsAt": reset_at,
                "windowDurationMins": minutes
            }
        })
    }

    fn response(rate_limits: Value) -> Value {
        json!({"rateLimits": rate_limits})
    }

    fn account_response(account: Value) -> Value {
        json!({"requiresOpenaiAuth": false, "account": account})
    }

    fn valid_window() -> Value {
        json!({"usedPercent": 31, "resetsAt": 100, "windowDurationMins": 60})
    }

    fn valid_individual() -> Value {
        json!({
            "limit": "100",
            "remainingPercent": 73,
            "resetsAt": 100,
            "used": "27"
        })
    }

    fn valid_credits() -> Value {
        json!({"hasCredits": false, "unlimited": false, "balance": "0"})
    }

    fn valid_reset_credit() -> Value {
        json!({
            "grantedAt": 1,
            "id": "credit-1",
            "resetType": "codexRateLimits",
            "status": "available",
            "description": "description",
            "title": "title",
            "expiresAt": 2
        })
    }

    fn wrong_json_types() -> Vec<Value> {
        vec![
            Value::Null,
            json!(true),
            json!("wrong"),
            json!([]),
            json!({}),
            json!(1.25),
            Value::from(i64::MIN),
            Value::from(u64::MAX),
        ]
    }

    fn wrong_non_null_types() -> Vec<Value> {
        vec![
            json!(true),
            json!("wrong"),
            json!([]),
            json!({}),
            json!(1.25),
            Value::from(u64::MAX),
        ]
    }

    fn wrong_i32_values() -> Vec<Value> {
        vec![
            Value::Null,
            json!(true),
            json!("wrong"),
            json!([]),
            json!({}),
            json!(1.25),
            Value::from(i64::MIN),
            Value::from(i64::MAX),
            Value::from(u64::MAX),
        ]
    }

    fn wrong_i64_values() -> Vec<Value> {
        vec![
            Value::Null,
            json!(true),
            json!("wrong"),
            json!([]),
            json!({}),
            json!(1.25),
            Value::from(u64::MAX),
        ]
    }

    fn snapshot_with_field(key: &str, value: Value) -> Value {
        let mut snapshot = Map::new();
        snapshot.insert(key.to_owned(), value);
        response(Value::Object(snapshot))
    }

    fn assert_quota_schema(value: Value) {
        assert_eq!(
            validate_rate_limits_response(&value),
            Err(ContractError::QuotaSchema),
            "quota fixture unexpectedly accepted: {value:?}"
        );
    }

    fn sorted_object_keys(value: &Value) -> Vec<String> {
        let mut keys: Vec<_> = value
            .as_object()
            .expect("schema object")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    fn definition<'a>(schema: &'a Value, name: &str) -> &'a Value {
        &schema["definitions"][name]
    }

    fn enum_values<'a>(schema: &'a Value, definition_name: &str) -> Vec<&'a str> {
        definition(schema, definition_name)["enum"]
            .as_array()
            .expect("enum array")
            .iter()
            .map(|value| value.as_str().expect("enum string"))
            .collect()
    }

    fn required_names(schema: &Value, definition_name: &str) -> Vec<String> {
        let Some(required) = definition(schema, definition_name)["required"].as_array() else {
            return Vec::new();
        };
        let mut names: Vec<_> = required
            .iter()
            .map(|value| value.as_str().expect("required string").to_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn account_updated_notification_is_strict_and_duplicate_safe() {
        for valid in [
            r#"{"jsonrpc":"2.0","method":"account/updated","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"account/updated","params":{"authMode":null,"planType":null}}"#,
            r#"{"jsonrpc":"2.0","method":"account/updated","params":{"authMode":"chatgpt","planType":"pro"}}"#,
        ] {
            assert_eq!(is_account_updated_notification_json(valid), Ok(true));
            assert_eq!(validate_account_updated_notification_json(valid), Ok(()));
        }
        assert_eq!(
            is_account_updated_notification_json(
                r#"{"jsonrpc":"2.0","id":1,"result":{"method":"account/updated"}}"#
            ),
            Ok(false)
        );
        assert_eq!(
            is_account_updated_notification_json(
                r#"{"method":"account/updated","method":"ordinary/event","params":{}}"#
            ),
            Err(ContractError::AccountUpdatedSchema)
        );

        for invalid in [
            r#"{"jsonrpc":"2.0","method":"account/updated","params":{},"extra":true}"#,
            r#"{"jsonrpc":"2.0","method":"account/updated","params":{"extra":true}}"#,
            r#"{"jsonrpc":"2.0","method":"account/updated","params":{"authMode":"chatgpt","authMode":"apikey"}}"#,
            r#"{"jsonrpc":"2.0","method":"account/updated","params":{"planType":"pro","planType":"free"}}"#,
            r#"{"jsonrpc":"2.0","method":"account/updated","params":{"authMode":"unknown"}}"#,
            r#"{"jsonrpc":"2.0","method":"account/updated","params":{"planType":"PLUS"}}"#,
        ] {
            assert_eq!(
                validate_account_updated_notification_json(invalid),
                Err(ContractError::AccountUpdatedSchema),
                "unexpectedly accepted {invalid}"
            );
        }
    }

    fn plan_family_compatibility() -> [[bool; 15]; 15] {
        [
            [
                true, false, false, false, false, false, false, false, false, false, false, false,
                false, false, true,
            ],
            [
                false, true, false, false, false, false, false, false, false, false, false, false,
                false, false, true,
            ],
            [
                false, false, true, false, false, false, false, false, false, false, false, false,
                false, false, true,
            ],
            [
                false, false, false, true, false, false, false, false, false, false, false, false,
                false, false, true,
            ],
            [
                false, false, false, false, true, false, false, false, false, false, false, false,
                false, false, true,
            ],
            [
                false, false, false, false, false, true, false, false, false, false, false, false,
                false, false, true,
            ],
            [
                false, false, false, false, false, false, true, true, true, false, false, false,
                false, false, true,
            ],
            [
                false, false, false, false, false, false, true, true, true, false, false, false,
                false, false, true,
            ],
            [
                false, false, false, false, false, false, true, true, true, false, false, false,
                false, false, true,
            ],
            [
                false, false, false, false, false, false, false, false, false, true, true, true,
                true, false, true,
            ],
            [
                false, false, false, false, false, false, false, false, false, true, true, true,
                true, false, true,
            ],
            [
                false, false, false, false, false, false, false, false, false, true, true, true,
                true, false, true,
            ],
            [
                false, false, false, false, false, false, false, false, false, true, true, true,
                true, false, true,
            ],
            [
                false, false, false, false, false, false, false, false, false, false, false, false,
                false, true, true,
            ],
            [
                true, true, true, true, true, true, true, true, true, true, true, true, true, true,
                true,
            ],
        ]
    }

    #[test]
    fn schema_identity_is_pinned_to_the_vendored_manifest() {
        let manifest: Value =
            serde_json::from_str(include_str!("../protocol/SCHEMA_MANIFEST.json")).unwrap();
        let account_schema: Value =
            serde_json::from_str(include_str!("../protocol/v2/GetAccountResponse.json")).unwrap();
        let quota_schema: Value = serde_json::from_str(include_str!(
            "../protocol/v2/GetAccountRateLimitsResponse.json"
        ))
        .unwrap();
        let plan_schema: Value =
            serde_json::from_str(include_str!("../protocol/v2/PlanType.canonical.json")).unwrap();

        assert_eq!(manifest["schema"].as_str(), Some(SCHEMA_MANIFEST));
        assert_eq!(
            manifest["codex_cli_version"].as_str(),
            Some(CODEX_CLI_VERSION)
        );
        assert_eq!(
            manifest["generated_utc_date"].as_str(),
            Some(GENERATED_UTC_DATE)
        );
        assert_eq!(manifest["experimental"], json!(false));
        assert_eq!(
            manifest["generation_command"],
            json!([
                "codex",
                "app-server",
                "generate-json-schema",
                "--out",
                "<empty-output-directory>"
            ])
        );
        assert_eq!(
            manifest["bundle"]["sha256"].as_str(),
            Some(SCHEMA_BUNDLE_SHA256)
        );

        let expected_artifacts = [
            (
                "GetAccountResponse",
                "protocol/v2/GetAccountResponse.json",
                Some("v2/GetAccountResponse.json"),
                "dc770d8377dfe9b93b67e857e22ba7f99726265c8ca5cae63c5ad06e78b70b9d",
                "03949a001fd055f6ad8bb10ab7b7a2a4d3850b3b46de9f2131195c61ce9c2df5",
                GET_ACCOUNT_RESPONSE_VENDORED_SHA256,
            ),
            (
                "GetAccountRateLimitsResponse",
                "protocol/v2/GetAccountRateLimitsResponse.json",
                Some("v2/GetAccountRateLimitsResponse.json"),
                "c67762e486f5a9b081ec42650fce657781669e6b1261c09c47bc8085d8cf2fc1",
                "e313f2918e069b0c3380890ea92986197f3c74a9e1a163e80b51235c1f0aa79d",
                GET_ACCOUNT_RATE_LIMITS_RESPONSE_VENDORED_SHA256,
            ),
            (
                "PlanType",
                "protocol/v2/PlanType.canonical.json",
                None,
                "585f8053090425683e665307b22b7f2793e17f93340361bb1c00b95320d9643c",
                "585f8053090425683e665307b22b7f2793e17f93340361bb1c00b95320d9643c",
                PLAN_TYPE_VENDORED_SHA256,
            ),
        ];
        let artifacts = manifest["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), expected_artifacts.len());
        for (artifact, expected) in artifacts.iter().zip(expected_artifacts) {
            assert_eq!(artifact["id"].as_str(), Some(expected.0));
            assert_eq!(artifact["path"].as_str(), Some(expected.1));
            match expected.2 {
                Some(path) => assert_eq!(artifact["generated_path"].as_str(), Some(path)),
                None => assert!(artifact.get("generated_path").is_none()),
            }
            assert_eq!(artifact["generated_raw_sha256"].as_str(), Some(expected.3));
            assert_eq!(
                artifact["canonical_jq_cS_sha256"].as_str(),
                Some(expected.4)
            );
            assert_eq!(artifact["vendored_sha256"].as_str(), Some(expected.5));
        }

        assert_eq!(
            enum_values(&account_schema, "PlanType"),
            PlanType::VALUES.to_vec()
        );
        assert_eq!(
            enum_values(&quota_schema, "PlanType"),
            PlanType::VALUES.to_vec()
        );
        assert_eq!(
            plan_schema["enum"].as_array().unwrap(),
            &PlanType::VALUES
                .iter()
                .map(|value| Value::String((*value).to_owned()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            plan_schema["enum"].as_array().unwrap().len(),
            PlanType::VALUES.len()
        );
    }

    #[test]
    fn schema_validator_surface_matches_all_actual_account_and_quota_inventories() {
        let account: Value =
            serde_json::from_str(include_str!("../protocol/v2/GetAccountResponse.json")).unwrap();
        let quota: Value = serde_json::from_str(include_str!(
            "../protocol/v2/GetAccountRateLimitsResponse.json"
        ))
        .unwrap();

        assert_eq!(
            sorted_object_keys(&account["properties"]),
            ["account", "requiresOpenaiAuth"]
        );
        assert_eq!(account["required"], json!(["requiresOpenaiAuth"]));
        let account_variants = account["definitions"]["Account"]["oneOf"]
            .as_array()
            .unwrap();
        assert_eq!(account_variants.len(), 3);
        let variant = |title: &str| {
            account_variants
                .iter()
                .find(|value| value["title"].as_str() == Some(title))
                .expect("account variant")
        };
        assert_eq!(
            sorted_object_keys(&variant("ApiKeyAccount")["properties"]),
            ["type"]
        );
        assert_eq!(required_names(&account, "Account"), Vec::<String>::new());
        assert_eq!(variant("ApiKeyAccount")["required"], json!(["type"]));
        assert_eq!(
            variant("ChatgptAccount")["required"],
            json!(["email", "planType", "type"])
        );
        assert_eq!(
            sorted_object_keys(&variant("ChatgptAccount")["properties"]),
            ["email", "planType", "type"]
        );
        assert_eq!(variant("AmazonBedrockAccount")["required"], json!(["type"]));
        assert_eq!(
            sorted_object_keys(&variant("AmazonBedrockAccount")["properties"]),
            ["type", "usesCodexManagedCredentials"]
        );
        assert_eq!(
            variant("ChatgptAccount")["properties"]["email"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            variant("ChatgptAccount")["properties"]["planType"]["$ref"],
            json!("#/definitions/PlanType")
        );
        assert_eq!(
            variant("AmazonBedrockAccount")["properties"]["usesCodexManagedCredentials"]["type"],
            json!("boolean")
        );

        assert_eq!(
            sorted_object_keys(&quota["properties"]),
            ["rateLimitResetCredits", "rateLimits", "rateLimitsByLimitId"]
        );
        assert_eq!(quota["required"], json!(["rateLimits"]));
        assert_eq!(
            quota["properties"]["rateLimitsByLimitId"]["type"],
            json!(["object", "null"])
        );
        assert_eq!(
            quota["properties"]["rateLimitResetCredits"]["anyOf"],
            json!([{"$ref": "#/definitions/RateLimitResetCreditsSummary"}, {"type": "null"}])
        );

        assert_eq!(
            sorted_object_keys(&definition(&quota, "CreditsSnapshot")["properties"]),
            ["balance", "hasCredits", "unlimited"]
        );
        assert_eq!(
            required_names(&quota, "CreditsSnapshot"),
            ["hasCredits", "unlimited"]
        );
        assert_eq!(
            definition(&quota, "CreditsSnapshot")["properties"]["balance"]["type"],
            json!(["string", "null"])
        );

        assert_eq!(
            sorted_object_keys(&definition(&quota, "SpendControlLimitSnapshot")["properties"]),
            ["limit", "remainingPercent", "resetsAt", "used"]
        );
        assert_eq!(
            required_names(&quota, "SpendControlLimitSnapshot"),
            ["limit", "remainingPercent", "resetsAt", "used"]
        );
        assert_eq!(
            sorted_object_keys(&definition(&quota, "RateLimitWindow")["properties"]),
            ["resetsAt", "usedPercent", "windowDurationMins"]
        );
        assert_eq!(required_names(&quota, "RateLimitWindow"), ["usedPercent"]);
        assert_eq!(
            definition(&quota, "RateLimitWindow")["properties"]["resetsAt"]["type"],
            json!(["integer", "null"])
        );
        assert_eq!(
            definition(&quota, "RateLimitWindow")["properties"]["windowDurationMins"]["type"],
            json!(["integer", "null"])
        );

        let snapshot = definition(&quota, "RateLimitSnapshot");
        assert_eq!(
            sorted_object_keys(&snapshot["properties"]),
            [
                "credits",
                "individualLimit",
                "limitId",
                "limitName",
                "planType",
                "primary",
                "rateLimitReachedType",
                "secondary",
                "spendControlReached"
            ]
        );
        for (field, reference) in [
            ("credits", "#/definitions/CreditsSnapshot"),
            ("individualLimit", "#/definitions/SpendControlLimitSnapshot"),
            ("primary", "#/definitions/RateLimitWindow"),
            ("secondary", "#/definitions/RateLimitWindow"),
        ] {
            assert_eq!(
                snapshot["properties"][field]["anyOf"],
                json!([{"$ref": reference}, {"type": "null"}]),
                "union inventory changed for {field}"
            );
        }
        assert_eq!(
            snapshot["properties"]["planType"]["anyOf"],
            json!([{"$ref": "#/definitions/PlanType"}, {"type": "null"}])
        );
        assert_eq!(
            snapshot["properties"]["rateLimitReachedType"]["anyOf"],
            json!([{"$ref": "#/definitions/RateLimitReachedType"}, {"type": "null"}])
        );
        assert_eq!(
            snapshot["properties"]["limitId"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            snapshot["properties"]["limitName"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            snapshot["properties"]["spendControlReached"]["type"],
            json!(["boolean", "null"])
        );

        assert_eq!(
            definition(&quota, "RateLimitResetCreditsSummary")["properties"],
            json!({
                "availableCount": {"format": "int64", "type": "integer"},
                "credits": {
                    "type": ["array", "null"],
                    "items": {"$ref": "#/definitions/RateLimitResetCredit"},
                    "description": "Detail rows for available reset credits, when the backend provides them.\n\n`null` means only `availableCount` is known, while an empty array means details were fetched and no available credits were returned. The backend may cap this list, so its length can be less than `availableCount`."
                }
            })
        );
        assert_eq!(
            required_names(&quota, "RateLimitResetCreditsSummary"),
            ["availableCount"]
        );
        assert_eq!(
            sorted_object_keys(&definition(&quota, "RateLimitResetCredit")["properties"]),
            [
                "description",
                "expiresAt",
                "grantedAt",
                "id",
                "resetType",
                "status",
                "title"
            ]
        );
        assert_eq!(
            required_names(&quota, "RateLimitResetCredit"),
            ["grantedAt", "id", "resetType", "status"]
        );
        assert_eq!(
            definition(&quota, "RateLimitResetCredit")["properties"]["description"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            definition(&quota, "RateLimitResetCredit")["properties"]["title"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            definition(&quota, "RateLimitResetCredit")["properties"]["expiresAt"]["type"],
            json!(["integer", "null"])
        );
        assert_eq!(
            definition(&quota, "RateLimitResetCredit")["properties"]["resetType"]["$ref"],
            json!("#/definitions/RateLimitResetType")
        );
        assert_eq!(
            definition(&quota, "RateLimitResetCredit")["properties"]["status"]["$ref"],
            json!("#/definitions/RateLimitResetCreditStatus")
        );
        assert_eq!(
            enum_values(&quota, "RateLimitReachedType"),
            vec![
                "rate_limit_reached",
                "workspace_owner_credits_depleted",
                "workspace_member_credits_depleted",
                "workspace_owner_usage_limit_reached",
                "workspace_member_usage_limit_reached"
            ]
        );
        assert_eq!(
            enum_values(&quota, "RateLimitResetType"),
            vec!["codexRateLimits", "unknown"]
        );
        assert_eq!(
            enum_values(&quota, "RateLimitResetCreditStatus"),
            vec!["available", "redeeming", "redeemed", "unknown"]
        );
    }

    #[test]
    fn plan_values_are_exact_and_aliases_are_rejected() {
        assert_eq!(
            PlanType::VALUES,
            [
                "free",
                "go",
                "plus",
                "pro",
                "prolite",
                "team",
                "self_serve_business_prolite",
                "self_serve_business_usage_based",
                "business",
                "ent26",
                "enterprise_cbp_automation",
                "enterprise_cbp_usage_based",
                "enterprise",
                "edu",
                "unknown",
            ]
        );
        for alias in [" Free", "FREE", "pro-lite", "chatgptenterprise", "1", ""] {
            assert!(PlanType::parse(alias).is_none(), "alias accepted: {alias}");
        }
        assert_eq!(PlanType::Ent26.family(), PlanFamily::Enterprise);
        assert_eq!(
            PlanType::EnterpriseCbpAutomation.family(),
            PlanFamily::Enterprise
        );
        assert_eq!(
            PlanType::EnterpriseCbpUsageBased.family(),
            PlanFamily::Enterprise
        );
        assert_eq!(PlanType::Enterprise.family(), PlanFamily::Enterprise);
        assert_eq!(PlanType::Business.family(), PlanFamily::Business);
        assert_eq!(PlanType::Unknown.family(), PlanFamily::Unset);
        assert_eq!(plan_label(Some("unknown")), "プラン未設定");
    }

    #[test]
    fn plan_labels_cover_every_wire_value_and_invalid_inputs() {
        let cases = [
            (Some("free"), "無料"),
            (Some("go"), "Go"),
            (Some("plus"), "Plus"),
            (Some("pro"), "Pro"),
            (Some("prolite"), "Pro Lite"),
            (Some("team"), "Team"),
            (Some("self_serve_business_prolite"), "Business"),
            (Some("self_serve_business_usage_based"), "Business"),
            (Some("business"), "Business"),
            (Some("ent26"), "エンタープライズ"),
            (Some("enterprise_cbp_automation"), "エンタープライズ"),
            (Some("enterprise_cbp_usage_based"), "エンタープライズ"),
            (Some("enterprise"), "エンタープライズ"),
            (Some("edu"), "教育"),
            (Some("unknown"), "プラン未設定"),
            (Some("invalid"), "プラン未設定"),
            (None, "プラン未設定"),
        ];
        for (wire, expected) in cases {
            assert_eq!(plan_label(wire), expected, "wire value: {wire:?}");
        }
    }

    #[test]
    fn account_outcomes_validate_before_meaning() {
        let auth = json!({"requiresOpenaiAuth": true, "account": {"type": "apiKey"}});
        assert_eq!(decode_account(&auth).unwrap(), AccountOutcome::AuthRequired);

        let supported = json!({
            "requiresOpenaiAuth": false,
            "account": {"type": "chatgpt", "email": "a@example.com", "planType": "unknown"}
        });
        assert!(matches!(
            decode_account(&supported).unwrap(),
            AccountOutcome::Supported { .. }
        ));

        for account in [
            Value::Null,
            json!({"type": "apiKey"}),
            json!({"type": "amazonBedrock"}),
            json!({"type": "chatgpt", "email": null, "planType": "free"}),
            json!({"type": "chatgpt", "email": "", "planType": "free"}),
        ] {
            let response = json!({"requiresOpenaiAuth": false, "account": account});
            assert_eq!(
                decode_account(&response).unwrap(),
                AccountOutcome::UnsupportedNoData
            );
        }
        assert_eq!(
            decode_account(&json!({"requiresOpenaiAuth": "false"})),
            Err(ContractError::AccountSchema)
        );
        assert_eq!(
            decode_account(&json!({
                "requiresOpenaiAuth": false,
                "account": {"type": "chatgpt", "email": "a@example.com", "planType": "FREE"}
            })),
            Err(ContractError::AccountSchema)
        );
    }

    #[test]
    fn authenticated_chatgpt_account_is_not_cleared_by_openai_auth_capability_flag() {
        let observed_shape = json!({
            "requiresOpenaiAuth": true,
            "account": {
                "type": "chatgpt",
                "email": "authenticated@example.com",
                "planType": "prolite"
            }
        });
        assert_eq!(
            decode_account(&observed_shape),
            Ok(AccountOutcome::Supported {
                email: "authenticated@example.com".to_owned(),
                plan_type: PlanType::ProLite,
            })
        );
        assert_eq!(
            decode_account(&json!({
                "requiresOpenaiAuth": true,
                "account": null
            })),
            Ok(AccountOutcome::AuthRequired)
        );
    }

    #[test]
    fn fixed_31_uses_top_level_only_and_returns_69() {
        let value = json!({
            "rateLimits": {
                "limitName": "Codex",
                "primary": {"usedPercent": 31, "resetsAt": 100, "windowDurationMins": 60},
                "model": {"usedPercent": 0}
            },
            "rateLimitsByLimitId": {
                "codex": {"primary": {"usedPercent": 0, "resetsAt": 200, "windowDurationMins": 100}}
            }
        });
        let snapshot = decode_quota(&value, None).unwrap().unwrap();
        assert_eq!(snapshot.fixed_used_percent, Some(31));
        assert_eq!(snapshot.remaining_percent, Some(69));
    }

    #[test]
    fn by_id_order_value_and_count_are_output_invariant() {
        let base = response(fixed_snapshot(31, 100, 60));
        let mut first = base.clone();
        first["rateLimitsByLimitId"] = json!({
            "codex": {"primary": {"usedPercent": 0, "resetsAt": 200, "windowDurationMins": 120}},
            "other": {}
        });
        let mut second = base;
        second["rateLimitsByLimitId"] = json!({
            "other": {},
            "codex": {"primary": {"usedPercent": 100, "resetsAt": 1, "windowDurationMins": 1}},
            "third": {"credits": {"hasCredits": false, "unlimited": true}}
        });
        assert_eq!(decode_quota(&first, None), decode_quota(&second, None));
    }

    #[test]
    fn invalid_top_level_is_unavailable_even_with_a_valid_by_id_candidate() {
        let value = json!({
            "rateLimits": {},
            "rateLimitsByLimitId": {
                "codex": {"primary": {"usedPercent": 31, "resetsAt": 100, "windowDurationMins": 60}}
            }
        });
        assert_eq!(decode_quota(&value, None).unwrap(), None);
    }

    #[test]
    fn invalid_auxiliary_schema_invalidates_the_whole_response() {
        let by_id_invalid = json!({
            "rateLimits": {},
            "rateLimitsByLimitId": {"codex": {"primary": {"usedPercent": "31"}}}
        });
        assert_eq!(
            decode_quota(&by_id_invalid, None),
            Err(ContractError::QuotaSchema)
        );

        let reset_credit_invalid = json!({
            "rateLimits": {},
            "rateLimitResetCredits": {
                "availableCount": 1,
                "credits": [{"grantedAt": 1, "id": "x", "resetType": "codexRateLimits"}]
            }
        });
        assert_eq!(
            decode_quota(&reset_credit_invalid, None),
            Err(ContractError::QuotaSchema)
        );
    }

    #[test]
    fn fixed_boundaries_are_direct_remaining_values() {
        for (used, remaining) in [(0, 100), (31, 69), (100, 0)] {
            let snapshot = decode_quota(&response(fixed_snapshot(used, 100, 1)), None)
                .unwrap()
                .unwrap();
            assert_eq!(snapshot.fixed_used_percent, Some(used));
            assert_eq!(snapshot.remaining_percent, Some(remaining));
        }
    }

    #[test]
    fn enterprise_individual_is_direct_and_precedes_fixed() {
        let value = response(json!({
            "planType": "enterprise",
            "individualLimit": {"limit": "ignored", "remainingPercent": 73, "resetsAt": 100, "used": "ignored"},
            "primary": {"usedPercent": 99, "resetsAt": 300, "windowDurationMins": 100}
        }));
        let snapshot = decode_quota(&value, Some(PlanFamily::Enterprise))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.fixed_used_percent, None);
        assert_eq!(snapshot.remaining_percent, Some(73));
        assert!(snapshot.monthly);
    }

    #[test]
    fn unknown_account_family_falls_back_to_known_canonical_family() {
        let value = response(json!({
            "planType": "enterprise",
            "individualLimit": {
                "limit": "ignored",
                "remainingPercent": 73,
                "resetsAt": 100,
                "used": "ignored"
            },
            "primary": {"usedPercent": 99, "resetsAt": 300, "windowDurationMins": 100}
        }));
        let snapshot = decode_quota_for_plan(&value, Some("unknown"))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.fixed_used_percent, None);
        assert_eq!(snapshot.remaining_percent, Some(73));
        assert!(snapshot.monthly);
    }

    #[test]
    fn enterprise_variants_match_each_other_but_business_mismatches() {
        let enterprise = [
            "ent26",
            "enterprise_cbp_automation",
            "enterprise_cbp_usage_based",
            "enterprise",
        ];
        for account_plan in enterprise {
            for quota_plan in enterprise {
                let value = response(json!({"planType": quota_plan}));
                assert!(!matches!(
                    decode_quota_for_plan(&value, Some(account_plan)),
                    Err(ContractError::AccountQuotaPlanMismatch)
                ));
            }
        }

        for business_plan in [
            "self_serve_business_prolite",
            "self_serve_business_usage_based",
            "business",
        ] {
            let value = response(json!({"planType": "enterprise"}));
            assert_eq!(
                decode_quota_for_plan(&value, Some(business_plan)),
                Err(ContractError::AccountQuotaPlanMismatch)
            );
        }
    }

    #[test]
    fn protocol_p2d_qv07_calendar_boundaries_use_public_decoder() {
        let cases: [(Value, QuotaSnapshot); 11] = [
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 1_709_251_200_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1_709_251_200,
                    window_seconds: 2_505_600,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 1_677_628_800_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1_677_628_800,
                    window_seconds: 2_419_200,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 1_714_521_600_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1_714_521_600,
                    window_seconds: 2_592_000,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 1_706_745_600_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1_706_745_600,
                    window_seconds: 2_678_400,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 1_705_322_096_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1_705_322_096,
                    window_seconds: 2_678_400,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 951_868_800_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 951_868_800,
                    window_seconds: 2_505_600,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 4_107_542_400_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 4_107_542_400,
                    window_seconds: 2_419_200,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 13_574_649_600_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 13_574_649_600,
                    window_seconds: 2_505_600,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 1_711_843_200_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1_711_843_200,
                    window_seconds: 2_678_400,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": 1_680_220_800_i64,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1_680_220_800,
                    window_seconds: 2_678_400,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "individualLimit": {
                            "limit": "100",
                            "remainingPercent": 73,
                            "resetsAt": i64::MAX,
                            "used": "27"
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: i64::MAX,
                    window_seconds: 2_592_000,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
        ];

        let mut visited = 0;
        for (value, expected) in cases {
            assert_eq!(
                decode_quota_for_plan(&value, Some("enterprise")),
                Ok(Some(expected))
            );
            visited += 1;
        }
        assert_eq!(visited, 11);
    }

    #[test]
    fn fixed_ties_use_duration_then_reset_then_primary() {
        let duration = response(json!({
            "primary": {"usedPercent": 10, "resetsAt": 900, "windowDurationMins": 60},
            "secondary": {"usedPercent": 20, "resetsAt": 100, "windowDurationMins": 120}
        }));
        assert_eq!(
            decode_quota(&duration, None)
                .unwrap()
                .unwrap()
                .fixed_used_percent,
            Some(20)
        );

        let reset = response(json!({
            "primary": {"usedPercent": 10, "resetsAt": 100, "windowDurationMins": 60},
            "secondary": {"usedPercent": 20, "resetsAt": 200, "windowDurationMins": 60}
        }));
        assert_eq!(
            decode_quota(&reset, None)
                .unwrap()
                .unwrap()
                .fixed_used_percent,
            Some(20)
        );

        let primary = response(json!({
            "primary": {"usedPercent": 10, "resetsAt": 100, "windowDurationMins": 60},
            "secondary": {"usedPercent": 20, "resetsAt": 100, "windowDurationMins": 60}
        }));
        assert_eq!(
            decode_quota(&primary, None)
                .unwrap()
                .unwrap()
                .fixed_used_percent,
            Some(10)
        );
    }

    #[test]
    fn unlimited_has_no_percentage_and_plan_mismatch_is_an_error() {
        let unlimited = response(json!({
            "credits": {"hasCredits": false, "balance": "ignored", "unlimited": true}
        }));
        let snapshot = decode_quota(&unlimited, None).unwrap().unwrap();
        assert!(snapshot.unlimited);
        assert_eq!(snapshot.remaining_percent, None);
        assert_eq!(snapshot.fixed_used_percent, None);

        let mismatch = response(json!({"planType": "business"}));
        assert_eq!(
            decode_quota(&mismatch, Some(PlanFamily::Enterprise)),
            Err(ContractError::AccountQuotaPlanMismatch)
        );
    }

    #[test]
    fn decode_quota_for_plan_validates_quota_before_account_plan() {
        let invalid_quota = json!({
            "rateLimits": {"primary": {"usedPercent": "invalid"}}
        });
        assert_eq!(
            decode_quota_for_plan(&invalid_quota, Some("not-a-plan")),
            Err(ContractError::QuotaSchema)
        );

        let valid_quota = response(json!({
            "credits": {"hasCredits": false, "unlimited": true}
        }));
        assert_eq!(
            decode_quota_for_plan(&valid_quota, Some("not-a-plan")),
            Err(ContractError::InvalidPlanType)
        );
    }

    #[test]
    fn account_schema_partition_matrix_is_exhaustive() {
        let mut cases: Vec<(Value, Result<AccountOutcome, ContractError>)> = Vec::new();

        for root in wrong_json_types() {
            cases.push((root, Err(ContractError::AccountSchema)));
        }

        cases.push((json!({}), Err(ContractError::AccountSchema)));
        for flag in [Value::Null, json!("false"), json!(1), json!([]), json!({})] {
            cases.push((
                json!({"requiresOpenaiAuth": flag}),
                Err(ContractError::AccountSchema),
            ));
        }

        cases.push((
            json!({"requiresOpenaiAuth": false}),
            Ok(AccountOutcome::UnsupportedNoData),
        ));
        cases.push((
            json!({"requiresOpenaiAuth": false, "account": null}),
            Ok(AccountOutcome::UnsupportedNoData),
        ));

        for account in wrong_non_null_types() {
            cases.push((account_response(account), Err(ContractError::AccountSchema)));
        }

        cases.push((
            account_response(json!({"type": "apiKey"})),
            Ok(AccountOutcome::UnsupportedNoData),
        ));
        cases.push((
            account_response(json!({
                "type": "apiKey",
                "apiKey": "ignored",
                "metadata": {"source": "fixture"},
                "extra": true
            })),
            Ok(AccountOutcome::UnsupportedNoData),
        ));

        for uses_codex_managed_credentials in [None, Some(true), Some(false)] {
            let account = match uses_codex_managed_credentials {
                None => json!({"type": "amazonBedrock"}),
                Some(value) => json!({
                    "type": "amazonBedrock",
                    "usesCodexManagedCredentials": value
                }),
            };
            cases.push((
                account_response(account),
                Ok(AccountOutcome::UnsupportedNoData),
            ));
        }
        for uses_codex_managed_credentials in
            [Value::Null, json!("true"), json!(1), json!([]), json!({})]
        {
            cases.push((
                account_response(json!({
                    "type": "amazonBedrock",
                    "usesCodexManagedCredentials": uses_codex_managed_credentials
                })),
                Err(ContractError::AccountSchema),
            ));
        }

        for account in [
            json!({"type": "chatgpt"}),
            json!({"type": "chatgpt", "email": "a@example.com"}),
            json!({"type": "chatgpt", "planType": "free"}),
        ] {
            cases.push((account_response(account), Err(ContractError::AccountSchema)));
        }
        for email in [Value::Null, json!(""), json!(" \t\n ")] {
            cases.push((
                account_response(json!({
                    "type": "chatgpt",
                    "email": email,
                    "planType": "free"
                })),
                Ok(AccountOutcome::UnsupportedNoData),
            ));
        }
        for email in [json!(true), json!(1), json!([]), json!({})] {
            cases.push((
                account_response(json!({
                    "type": "chatgpt",
                    "email": email,
                    "planType": "free"
                })),
                Err(ContractError::AccountSchema),
            ));
        }

        let plan_cases = [
            ("free", PlanType::Free),
            ("go", PlanType::Go),
            ("plus", PlanType::Plus),
            ("pro", PlanType::Pro),
            ("prolite", PlanType::ProLite),
            ("team", PlanType::Team),
            (
                "self_serve_business_prolite",
                PlanType::SelfServeBusinessProLite,
            ),
            (
                "self_serve_business_usage_based",
                PlanType::SelfServeBusinessUsageBased,
            ),
            ("business", PlanType::Business),
            ("ent26", PlanType::Ent26),
            (
                "enterprise_cbp_automation",
                PlanType::EnterpriseCbpAutomation,
            ),
            (
                "enterprise_cbp_usage_based",
                PlanType::EnterpriseCbpUsageBased,
            ),
            ("enterprise", PlanType::Enterprise),
            ("edu", PlanType::Edu),
            ("unknown", PlanType::Unknown),
        ];
        for (wire, plan_type) in plan_cases {
            cases.push((
                account_response(json!({
                    "type": "chatgpt",
                    "email": "a@example.com",
                    "planType": wire
                })),
                Ok(AccountOutcome::Supported {
                    email: "a@example.com".to_owned(),
                    plan_type,
                }),
            ));
        }

        for plan_type in [
            json!(" free"),
            json!("free "),
            json!("FREE"),
            json!("pro-lite"),
            json!("xpro"),
            json!("prox"),
            json!("フリー"),
            json!(""),
            Value::Null,
            json!(true),
            json!(1),
            json!([]),
            json!({}),
        ] {
            cases.push((
                account_response(json!({
                    "type": "chatgpt",
                    "email": "a@example.com",
                    "planType": plan_type
                })),
                Err(ContractError::AccountSchema),
            ));
        }

        cases.push((
            json!({
                "requiresOpenaiAuth": false,
                "account": {"type": "unknown", "email": "a@example.com", "planType": "free"}
            }),
            Err(ContractError::AccountSchema),
        ));
        cases.push((
            json!({
                "requiresOpenaiAuth": false,
                "account": {"email": "a@example.com", "planType": "free"}
            }),
            Err(ContractError::AccountSchema),
        ));
        for account_type in [Value::Null, json!(true), json!(1), json!([]), json!({})] {
            cases.push((
                json!({
                    "requiresOpenaiAuth": false,
                    "account": {
                        "type": account_type,
                        "email": "a@example.com",
                        "planType": "free"
                    }
                }),
                Err(ContractError::AccountSchema),
            ));
        }
        cases.push((
            json!({
                "requiresOpenaiAuth": true,
                "account": {
                    "type": "chatgpt",
                    "email": false,
                    "planType": "free"
                }
            }),
            Err(ContractError::AccountSchema),
        ));
        cases.push((
            json!({
                "requiresOpenaiAuth": true,
                "account": {"type": "not-an-account-type"}
            }),
            Err(ContractError::AccountSchema),
        ));

        cases.push((
            json!({
                "requiresOpenaiAuth": true,
                "account": {"type": "chatgpt", "email": "a@example.com", "planType": "free"}
            }),
            Ok(AccountOutcome::Supported {
                email: "a@example.com".to_owned(),
                plan_type: PlanType::Free,
            }),
        ));

        let expected_supported = Ok(AccountOutcome::Supported {
            email: "a@example.com".to_owned(),
            plan_type: PlanType::Free,
        });
        cases.push((
            json!({
                "requiresOpenaiAuth": false,
                "rootExtra": {"kept": true},
                "account": {"type": "chatgpt", "email": "a@example.com", "planType": "free"}
            }),
            expected_supported.clone(),
        ));
        cases.push((
            json!({
                "requiresOpenaiAuth": false,
                "account": {
                    "type": "chatgpt",
                    "email": "a@example.com",
                    "planType": "free",
                    "accountExtra": [1, 2, 3]
                }
            }),
            expected_supported,
        ));

        for (value, expected) in cases {
            assert_eq!(decode_account(&value), expected, "fixture: {value:?}");
        }
    }

    #[test]
    fn quota_schema_root_and_snapshot_matrix() {
        let assert_ok = |value: Value| {
            assert_eq!(
                validate_rate_limits_response(&value),
                Ok(()),
                "quota fixture unexpectedly rejected: {value:?}"
            );
        };

        for value in wrong_json_types()
            .into_iter()
            .filter(|value| !value.is_object())
        {
            assert_quota_schema(value);
        }
        assert_quota_schema(json!({}));
        for value in wrong_json_types()
            .into_iter()
            .filter(|value| !value.is_object())
        {
            assert_quota_schema(response(value));
        }

        assert_ok(response(json!({})));
        assert_ok(json!({
            "rateLimits": {"snapshotExtra": {"kept": true}},
            "rootExtra": {"kept": true}
        }));

        let valid_values = vec![
            ("credits", valid_credits()),
            ("individualLimit", valid_individual()),
            ("limitId", json!("limit-id")),
            ("limitName", json!("Limit name")),
            ("primary", valid_window()),
            ("secondary", valid_window()),
        ];
        for (key, value) in valid_values {
            assert_ok(snapshot_with_field(key, value));
        }
        for key in [
            "credits",
            "individualLimit",
            "limitId",
            "limitName",
            "planType",
            "primary",
            "secondary",
            "rateLimitReachedType",
            "spendControlReached",
        ] {
            assert_ok(snapshot_with_field(key, Value::Null));
            for wrong in wrong_non_null_types() {
                if ((key == "limitId" || key == "limitName") && wrong.is_string())
                    || (key == "spendControlReached" && wrong.is_boolean())
                {
                    continue;
                }
                assert_quota_schema(snapshot_with_field(key, wrong));
            }
        }

        for plan in [
            "free",
            "go",
            "plus",
            "pro",
            "prolite",
            "team",
            "self_serve_business_prolite",
            "self_serve_business_usage_based",
            "business",
            "ent26",
            "enterprise_cbp_automation",
            "enterprise_cbp_usage_based",
            "enterprise",
            "edu",
            "unknown",
        ] {
            assert_ok(snapshot_with_field("planType", json!(plan)));
        }
        for reached_type in [
            "rate_limit_reached",
            "workspace_owner_credits_depleted",
            "workspace_member_credits_depleted",
            "workspace_owner_usage_limit_reached",
            "workspace_member_usage_limit_reached",
        ] {
            assert_ok(snapshot_with_field(
                "rateLimitReachedType",
                json!(reached_type),
            ));
        }
        for value in [json!(true), json!(false)] {
            assert_ok(snapshot_with_field("spendControlReached", value));
        }

        for key in ["primary", "secondary"] {
            assert_ok(snapshot_with_field(key, valid_window()));
            assert_ok(snapshot_with_field(key, Value::Null));
            for wrong in wrong_non_null_types() {
                assert_quota_schema(snapshot_with_field(key, wrong));
            }

            for used_percent in [i32::MIN, i32::MAX] {
                let mut window = valid_window();
                window
                    .as_object_mut()
                    .expect("window object")
                    .insert("usedPercent".to_owned(), json!(used_percent));
                assert_ok(snapshot_with_field(key, window));
            }
            for wrong in wrong_i32_values() {
                let mut window = valid_window();
                window
                    .as_object_mut()
                    .expect("window object")
                    .insert("usedPercent".to_owned(), wrong);
                assert_quota_schema(snapshot_with_field(key, window));
            }
            for outside in [
                Value::from(i64::from(i32::MIN) - 1),
                Value::from(i64::from(i32::MAX) + 1),
            ] {
                let mut window = valid_window();
                window
                    .as_object_mut()
                    .expect("window object")
                    .insert("usedPercent".to_owned(), outside);
                assert_quota_schema(snapshot_with_field(key, window));
            }

            for optional_key in ["resetsAt", "windowDurationMins"] {
                let mut absent = valid_window();
                absent
                    .as_object_mut()
                    .expect("window object")
                    .remove(optional_key);
                assert_ok(snapshot_with_field(key, absent));

                let mut null = valid_window();
                null.as_object_mut()
                    .expect("window object")
                    .insert(optional_key.to_owned(), Value::Null);
                assert_ok(snapshot_with_field(key, null));

                for boundary in [Value::from(i64::MIN), Value::from(i64::MAX)] {
                    let mut window = valid_window();
                    window
                        .as_object_mut()
                        .expect("window object")
                        .insert(optional_key.to_owned(), boundary);
                    assert_ok(snapshot_with_field(key, window));
                }
                for wrong in wrong_i64_values()
                    .into_iter()
                    .filter(|value| !value.is_null())
                {
                    let mut window = valid_window();
                    window
                        .as_object_mut()
                        .expect("window object")
                        .insert(optional_key.to_owned(), wrong);
                    assert_quota_schema(snapshot_with_field(key, window));
                }
            }
        }
    }

    #[test]
    fn quota_schema_nested_objects_matrix() {
        let assert_ok = |value: Value| {
            assert_eq!(
                validate_rate_limits_response(&value),
                Ok(()),
                "quota fixture unexpectedly rejected: {value:?}"
            );
        };

        for key in ["hasCredits", "unlimited"] {
            let mut missing = valid_credits();
            missing.as_object_mut().expect("credits object").remove(key);
            assert_quota_schema(snapshot_with_field("credits", missing));

            let mut null = valid_credits();
            null.as_object_mut()
                .expect("credits object")
                .insert(key.to_owned(), Value::Null);
            assert_quota_schema(snapshot_with_field("credits", null));

            for wrong in wrong_non_null_types() {
                if wrong.is_boolean() {
                    continue;
                }
                let mut invalid = valid_credits();
                invalid
                    .as_object_mut()
                    .expect("credits object")
                    .insert(key.to_owned(), wrong);
                assert_quota_schema(snapshot_with_field("credits", invalid));
            }
        }
        let mut balance_missing = valid_credits();
        balance_missing
            .as_object_mut()
            .expect("credits object")
            .remove("balance");
        assert_ok(snapshot_with_field("credits", balance_missing));
        let mut balance_null = valid_credits();
        balance_null
            .as_object_mut()
            .expect("credits object")
            .insert("balance".to_owned(), Value::Null);
        assert_ok(snapshot_with_field("credits", balance_null));
        for wrong in wrong_non_null_types() {
            if wrong.is_string() {
                continue;
            }
            let mut invalid = valid_credits();
            invalid
                .as_object_mut()
                .expect("credits object")
                .insert("balance".to_owned(), wrong);
            assert_quota_schema(snapshot_with_field("credits", invalid));
        }
        let mut credits_unknown = valid_credits();
        credits_unknown
            .as_object_mut()
            .expect("credits object")
            .insert("creditsExtra".to_owned(), json!({"kept": true}));
        assert_ok(snapshot_with_field("credits", credits_unknown));

        for key in ["limit", "used"] {
            let mut missing = valid_individual();
            missing
                .as_object_mut()
                .expect("individual object")
                .remove(key);
            assert_quota_schema(snapshot_with_field("individualLimit", missing));

            let mut null = valid_individual();
            null.as_object_mut()
                .expect("individual object")
                .insert(key.to_owned(), Value::Null);
            assert_quota_schema(snapshot_with_field("individualLimit", null));

            for wrong in wrong_non_null_types() {
                if wrong.is_string() {
                    continue;
                }
                let mut invalid = valid_individual();
                invalid
                    .as_object_mut()
                    .expect("individual object")
                    .insert(key.to_owned(), wrong);
                assert_quota_schema(snapshot_with_field("individualLimit", invalid));
            }
        }

        let mut missing_remaining = valid_individual();
        missing_remaining
            .as_object_mut()
            .expect("individual object")
            .remove("remainingPercent");
        assert_quota_schema(snapshot_with_field("individualLimit", missing_remaining));
        for wrong in wrong_i32_values() {
            let mut invalid = valid_individual();
            invalid
                .as_object_mut()
                .expect("individual object")
                .insert("remainingPercent".to_owned(), wrong);
            assert_quota_schema(snapshot_with_field("individualLimit", invalid));
        }
        for boundary in [i32::MIN, i32::MAX] {
            let mut valid = valid_individual();
            valid
                .as_object_mut()
                .expect("individual object")
                .insert("remainingPercent".to_owned(), json!(boundary));
            assert_ok(snapshot_with_field("individualLimit", valid));
        }
        for outside in [
            Value::from(i64::from(i32::MIN) - 1),
            Value::from(i64::from(i32::MAX) + 1),
        ] {
            let mut invalid = valid_individual();
            invalid
                .as_object_mut()
                .expect("individual object")
                .insert("remainingPercent".to_owned(), outside);
            assert_quota_schema(snapshot_with_field("individualLimit", invalid));
        }

        let mut missing_resets_at = valid_individual();
        missing_resets_at
            .as_object_mut()
            .expect("individual object")
            .remove("resetsAt");
        assert_quota_schema(snapshot_with_field("individualLimit", missing_resets_at));
        for wrong in wrong_i64_values() {
            let mut invalid = valid_individual();
            invalid
                .as_object_mut()
                .expect("individual object")
                .insert("resetsAt".to_owned(), wrong);
            assert_quota_schema(snapshot_with_field("individualLimit", invalid));
        }
        for boundary in [Value::from(i64::MIN), Value::from(i64::MAX)] {
            let mut valid = valid_individual();
            valid
                .as_object_mut()
                .expect("individual object")
                .insert("resetsAt".to_owned(), boundary);
            assert_ok(snapshot_with_field("individualLimit", valid));
        }

        let mut individual_unknown = valid_individual();
        individual_unknown
            .as_object_mut()
            .expect("individual object")
            .insert("individualExtra".to_owned(), json!("kept"));
        assert_ok(snapshot_with_field("individualLimit", individual_unknown));
    }

    #[test]
    fn quota_schema_by_id_and_reset_credit_matrix() {
        let assert_ok = |value: Value| {
            assert_eq!(
                validate_rate_limits_response(&value),
                Ok(()),
                "quota fixture unexpectedly rejected: {value:?}"
            );
        };
        let summary_response =
            |summary: Value| json!({"rateLimits": {}, "rateLimitResetCredits": summary});
        let item_response =
            |item: Value| summary_response(json!({"availableCount": 1, "credits": [item]}));

        assert_ok(json!({"rateLimits": {}}));
        assert_ok(json!({"rateLimits": {}, "rateLimitsByLimitId": Value::Null}));
        assert_ok(json!({"rateLimits": {}, "rateLimitsByLimitId": {}}));
        assert_ok(json!({
            "rateLimits": {},
            "rateLimitsByLimitId": {
                "first": {},
                "arbitrary-second-key": {
                    "primary": valid_window(),
                    "snapshotExtra": true
                }
            }
        }));
        for wrong in wrong_json_types()
            .into_iter()
            .filter(|value| !value.is_null() && !value.is_object())
        {
            assert_quota_schema(json!({
                "rateLimits": {},
                "rateLimitsByLimitId": wrong
            }));
        }
        for invalid_snapshot in [
            json!({
                "credits": {"hasCredits": true, "unlimited": "wrong", "balance": "0"}
            }),
            json!({
                "individualLimit": {
                    "limit": "100",
                    "remainingPercent": 73,
                    "resetsAt": 100,
                    "used": null
                }
            }),
            json!({"primary": {"usedPercent": 2_147_483_648_i64}}),
            json!({"limitId": false}),
            json!({"planType": "not-a-plan"}),
            json!({"rateLimitReachedType": "not-reached"}),
            json!({"spendControlReached": "wrong"}),
        ] {
            assert_quota_schema(json!({
                "rateLimits": {},
                "rateLimitsByLimitId": {"invalid": invalid_snapshot}
            }));
        }

        assert_ok(summary_response(Value::Null));
        for wrong in wrong_json_types()
            .into_iter()
            .filter(|value| !value.is_null() && !value.is_object())
        {
            assert_quota_schema(summary_response(wrong));
        }
        assert_quota_schema(summary_response(json!({})));
        for wrong in wrong_i64_values() {
            let mut summary = json!({"availableCount": 1});
            summary
                .as_object_mut()
                .expect("summary object")
                .insert("availableCount".to_owned(), wrong);
            assert_quota_schema(summary_response(summary));
        }
        for boundary in [Value::from(i64::MIN), Value::from(i64::MAX)] {
            let mut summary = json!({"availableCount": 1});
            summary
                .as_object_mut()
                .expect("summary object")
                .insert("availableCount".to_owned(), boundary);
            assert_ok(summary_response(summary));
        }
        assert_ok(summary_response(json!({"availableCount": 1})));
        assert_ok(summary_response(
            json!({"availableCount": 1, "credits": Value::Null}),
        ));
        assert_ok(summary_response(
            json!({"availableCount": 1, "credits": []}),
        ));
        assert_ok(summary_response(json!({
            "availableCount": 1,
            "credits": [valid_reset_credit()]
        })));
        for wrong in wrong_json_types()
            .into_iter()
            .filter(|value| !value.is_null() && !value.is_array())
        {
            assert_quota_schema(summary_response(json!({
                "availableCount": 1,
                "credits": wrong
            })));
        }
        assert_ok(summary_response(json!({
            "availableCount": 1,
            "summaryExtra": {"kept": true}
        })));

        for key in ["grantedAt", "id", "resetType", "status"] {
            let mut missing = valid_reset_credit();
            missing
                .as_object_mut()
                .expect("reset credit object")
                .remove(key);
            assert_quota_schema(item_response(missing));

            let mut null = valid_reset_credit();
            null.as_object_mut()
                .expect("reset credit object")
                .insert(key.to_owned(), Value::Null);
            assert_quota_schema(item_response(null));
        }
        for wrong in wrong_i64_values() {
            let mut invalid = valid_reset_credit();
            invalid
                .as_object_mut()
                .expect("reset credit object")
                .insert("grantedAt".to_owned(), wrong);
            assert_quota_schema(item_response(invalid));
        }
        for key in ["id", "resetType", "status"] {
            for wrong in wrong_non_null_types() {
                if key == "id" && wrong.is_string() {
                    continue;
                }
                let mut invalid = valid_reset_credit();
                invalid
                    .as_object_mut()
                    .expect("reset credit object")
                    .insert(key.to_owned(), wrong);
                assert_quota_schema(item_response(invalid));
            }
        }
        for granted_at in [Value::from(i64::MIN), Value::from(i64::MAX)] {
            let mut valid = valid_reset_credit();
            valid
                .as_object_mut()
                .expect("reset credit object")
                .insert("grantedAt".to_owned(), granted_at);
            assert_ok(item_response(valid));
        }
        for reset_type in ["codexRateLimits", "unknown"] {
            let mut valid = valid_reset_credit();
            valid
                .as_object_mut()
                .expect("reset credit object")
                .insert("resetType".to_owned(), json!(reset_type));
            assert_ok(item_response(valid));
        }
        for status in ["available", "redeeming", "redeemed", "unknown"] {
            let mut valid = valid_reset_credit();
            valid
                .as_object_mut()
                .expect("reset credit object")
                .insert("status".to_owned(), json!(status));
            assert_ok(item_response(valid));
        }

        for key in ["description", "title"] {
            let mut absent = valid_reset_credit();
            absent
                .as_object_mut()
                .expect("reset credit object")
                .remove(key);
            assert_ok(item_response(absent));

            let mut null = valid_reset_credit();
            null.as_object_mut()
                .expect("reset credit object")
                .insert(key.to_owned(), Value::Null);
            assert_ok(item_response(null));

            let mut string = valid_reset_credit();
            string
                .as_object_mut()
                .expect("reset credit object")
                .insert(key.to_owned(), json!("replacement"));
            assert_ok(item_response(string));

            for wrong in wrong_non_null_types() {
                if wrong.is_string() {
                    continue;
                }
                let mut invalid = valid_reset_credit();
                invalid
                    .as_object_mut()
                    .expect("reset credit object")
                    .insert(key.to_owned(), wrong);
                assert_quota_schema(item_response(invalid));
            }
        }

        let mut expires_absent = valid_reset_credit();
        expires_absent
            .as_object_mut()
            .expect("reset credit object")
            .remove("expiresAt");
        assert_ok(item_response(expires_absent));
        let mut expires_null = valid_reset_credit();
        expires_null
            .as_object_mut()
            .expect("reset credit object")
            .insert("expiresAt".to_owned(), Value::Null);
        assert_ok(item_response(expires_null));
        for boundary in [Value::from(i64::MIN), Value::from(i64::MAX)] {
            let mut valid = valid_reset_credit();
            valid
                .as_object_mut()
                .expect("reset credit object")
                .insert("expiresAt".to_owned(), boundary);
            assert_ok(item_response(valid));
        }
        for wrong in wrong_i64_values()
            .into_iter()
            .filter(|value| !value.is_null())
        {
            let mut invalid = valid_reset_credit();
            invalid
                .as_object_mut()
                .expect("reset credit object")
                .insert("expiresAt".to_owned(), wrong);
            assert_quota_schema(item_response(invalid));
        }
        let mut item_unknown = valid_reset_credit();
        item_unknown
            .as_object_mut()
            .expect("reset credit object")
            .insert("itemExtra".to_owned(), json!(42));
        assert_ok(item_response(item_unknown));
    }

    #[test]
    fn quota_semantics_p2c_fixed_boundaries_and_semantics() {
        let fixed = |used: i64, reset_at: i64, window_duration_mins: i64| {
            json!({
                "usedPercent": used,
                "resetsAt": reset_at,
                "windowDurationMins": window_duration_mins
            })
        };
        let response = |window: Value| json!({"rateLimits": {"primary": window}});
        let expected =
            |used: i32, remaining: i32, reset_at: i64, window_seconds: i64| QuotaSnapshot {
                fixed_used_percent: Some(used),
                remaining_percent: Some(remaining),
                reset_at,
                window_seconds,
                limit_name: "Codex".to_owned(),
                monthly: false,
                unlimited: false,
            };

        assert_eq!(
            decode_quota(&response(fixed(0, 100, 60)), None)
                .unwrap()
                .unwrap()
                .remaining_percent,
            Some(100)
        );
        assert_eq!(
            decode_quota(&response(fixed(31, 100, 60)), None)
                .unwrap()
                .unwrap()
                .remaining_percent,
            Some(69)
        );
        assert_eq!(
            decode_quota(&response(fixed(100, 100, 60)), None)
                .unwrap()
                .unwrap()
                .remaining_percent,
            Some(0)
        );

        for used in [-1_i64, 101] {
            let value = response(fixed(used, 100, 60));
            assert_eq!(validate_rate_limits_response(&value), Ok(()));
            assert_eq!(decode_quota(&value, None), Ok(None));
        }

        for reset_at in [1_i64, i64::MAX] {
            let value = response(fixed(31, reset_at, 60));
            assert_eq!(
                decode_quota(&value, None).unwrap().unwrap(),
                expected(31, 69, reset_at, 3_600)
            );
        }
        for reset_at in [0_i64, -1] {
            let value = response(fixed(31, reset_at, 60));
            assert_eq!(validate_rate_limits_response(&value), Ok(()));
            assert_eq!(decode_quota(&value, None), Ok(None));
        }

        assert_eq!(
            decode_quota(&response(fixed(31, 100, 1)), None)
                .unwrap()
                .unwrap(),
            expected(31, 69, 100, 60)
        );
        assert_eq!(
            decode_quota(&response(fixed(31, 100, 527_040)), None)
                .unwrap()
                .unwrap(),
            expected(31, 69, 100, 31_622_400)
        );
        for duration_mins in [0_i64, -1, 527_041, i64::MAX] {
            let value = response(fixed(31, 100, duration_mins));
            assert_eq!(validate_rate_limits_response(&value), Ok(()));
            assert_eq!(decode_quota(&value, None), Ok(None));
        }

        let assert_no_candidate = |window: Value| {
            let value = json!({
                "rateLimits": {"primary": window},
                "rateLimitsByLimitId": {
                    "fallback": {"primary": valid_window()}
                },
                "rateLimitResetCredits": {
                    "availableCount": 1,
                    "credits": [valid_reset_credit()]
                }
            });
            assert_eq!(validate_rate_limits_response(&value), Ok(()));
            assert_eq!(decode_quota(&value, None), Ok(None));
        };

        let mut missing_reset = fixed(31, 100, 60);
        missing_reset
            .as_object_mut()
            .expect("fixed window object")
            .remove("resetsAt");
        assert_no_candidate(missing_reset);

        let mut null_reset = fixed(31, 100, 60);
        null_reset
            .as_object_mut()
            .expect("fixed window object")
            .insert("resetsAt".to_owned(), Value::Null);
        assert_no_candidate(null_reset);

        let mut missing_duration = fixed(31, 100, 60);
        missing_duration
            .as_object_mut()
            .expect("fixed window object")
            .remove("windowDurationMins");
        assert_no_candidate(missing_duration);

        let mut null_duration = fixed(31, 100, 60);
        null_duration
            .as_object_mut()
            .expect("fixed window object")
            .insert("windowDurationMins".to_owned(), Value::Null);
        assert_no_candidate(null_duration);
    }

    #[test]
    fn quota_semantics_p2c_individual_precedence_and_fallback() {
        let individual = |remaining_percent: i64, reset_at: i64, limit: &str, used: &str| {
            json!({
                "limit": limit,
                "remainingPercent": remaining_percent,
                "resetsAt": reset_at,
                "used": used
            })
        };
        let fixed = |used_percent: i64, reset_at: i64, window_duration_mins: i64| {
            json!({
                "usedPercent": used_percent,
                "resetsAt": reset_at,
                "windowDurationMins": window_duration_mins
            })
        };
        let response = |individual_value: Option<Value>,
                        fixed_value: Option<Value>,
                        credits: Option<Value>,
                        plan: Option<&str>| {
            let mut snapshot = Map::new();
            if let Some(value) = individual_value {
                snapshot.insert("individualLimit".to_owned(), value);
            }
            if let Some(value) = fixed_value {
                snapshot.insert("primary".to_owned(), value);
            }
            if let Some(value) = credits {
                snapshot.insert("credits".to_owned(), value);
            }
            if let Some(value) = plan {
                snapshot.insert("planType".to_owned(), json!(value));
            }
            json!({"rateLimits": Value::Object(snapshot)})
        };

        for remaining_percent in [0_i64, 73, 100] {
            let value = response(
                Some(individual(remaining_percent, 1_709_251_200, "100", "27")),
                None,
                None,
                None,
            );
            assert_eq!(
                decode_quota_for_plan(&value, Some("enterprise"))
                    .unwrap()
                    .unwrap(),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(remaining_percent as i32),
                    reset_at: 1_709_251_200,
                    window_seconds: 2_505_600,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                }
            );
        }

        for remaining_percent in [-1_i64, 101] {
            let value = response(
                Some(individual(remaining_percent, 1_709_251_200, "100", "27")),
                None,
                None,
                None,
            );
            assert_eq!(validate_rate_limits_response(&value), Ok(()));
            assert_eq!(decode_quota_for_plan(&value, Some("enterprise")), Ok(None));
        }
        for reset_at in [0_i64, -1] {
            let value = response(
                Some(individual(73, reset_at, "100", "27")),
                None,
                None,
                None,
            );
            assert_eq!(validate_rate_limits_response(&value), Ok(()));
            assert_eq!(decode_quota_for_plan(&value, Some("enterprise")), Ok(None));
        }
        for reset_at in [1_i64, i64::MAX] {
            let value = response(
                Some(individual(73, reset_at, "100", "27")),
                None,
                None,
                None,
            );
            let snapshot = decode_quota_for_plan(&value, Some("enterprise"))
                .unwrap()
                .unwrap();
            assert_eq!(snapshot.fixed_used_percent, None);
            assert_eq!(snapshot.remaining_percent, Some(73));
            assert_eq!(snapshot.reset_at, reset_at);
            assert!(snapshot.monthly);
            assert!(!snapshot.unlimited);
        }

        let first = response(
            Some(individual(73, 1_709_251_200, "100", "27")),
            None,
            None,
            None,
        );
        let second = response(
            Some(individual(73, 1_709_251_200, "200", "0")),
            None,
            None,
            None,
        );
        assert_eq!(
            decode_quota_for_plan(&first, Some("enterprise")),
            decode_quota_for_plan(&second, Some("enterprise"))
        );

        let precedence = response(
            Some(individual(73, 1_709_251_200, "100", "27")),
            Some(fixed(31, 999, 60)),
            Some(json!({
                "hasCredits": true,
                "unlimited": true,
                "balance": "0"
            })),
            Some("enterprise"),
        );
        assert_eq!(
            decode_quota_for_plan(&precedence, Some("enterprise"))
                .unwrap()
                .unwrap(),
            QuotaSnapshot {
                fixed_used_percent: None,
                remaining_percent: Some(73),
                reset_at: 1_709_251_200,
                window_seconds: 2_505_600,
                limit_name: "Codex".to_owned(),
                monthly: true,
                unlimited: false,
            }
        );

        let invalid_individual_falls_through = response(
            Some(individual(-1, 1_709_251_200, "100", "27")),
            Some(fixed(31, 999, 60)),
            Some(valid_credits()),
            Some("enterprise"),
        );
        assert_eq!(
            decode_quota_for_plan(&invalid_individual_falls_through, Some("enterprise"))
                .unwrap()
                .unwrap(),
            QuotaSnapshot {
                fixed_used_percent: Some(31),
                remaining_percent: Some(69),
                reset_at: 999,
                window_seconds: 3_600,
                limit_name: "Codex".to_owned(),
                monthly: false,
                unlimited: false,
            }
        );

        let invalid_fixed_falls_through = response(
            None,
            Some(fixed(-1, 999, 60)),
            Some(json!({
                "hasCredits": false,
                "unlimited": true,
                "balance": "0"
            })),
            Some("enterprise"),
        );
        let unlimited = decode_quota_for_plan(&invalid_fixed_falls_through, Some("enterprise"))
            .unwrap()
            .unwrap();
        assert_eq!(unlimited.fixed_used_percent, None);
        assert_eq!(unlimited.remaining_percent, None);
        assert!(unlimited.unlimited);

        let no_candidate = response(None, None, Some(valid_credits()), None);
        assert_eq!(
            decode_quota_for_plan(&no_candidate, Some("enterprise")),
            Ok(None)
        );

        let non_enterprise = response(
            Some(individual(73, 1_709_251_200, "100", "27")),
            None,
            Some(valid_credits()),
            None,
        );
        assert_eq!(
            decode_quota_for_plan(&non_enterprise, Some("pro")),
            Ok(None)
        );
    }

    #[test]
    fn quota_semantics_p2c_fixed_selection_tiebreaks() {
        let fixed = |used_percent: i64, reset_at: i64, window_duration_mins: i64| {
            json!({
                "usedPercent": used_percent,
                "resetsAt": reset_at,
                "windowDurationMins": window_duration_mins
            })
        };
        let response = |primary: Value, secondary: Value| json!({"rateLimits": {"primary": primary, "secondary": secondary}});
        let expected =
            |used: i32, remaining: i32, reset_at: i64, window_seconds: i64| QuotaSnapshot {
                fixed_used_percent: Some(used),
                remaining_percent: Some(remaining),
                reset_at,
                window_seconds,
                limit_name: "Codex".to_owned(),
                monthly: false,
                unlimited: false,
            };

        let longer_duration = response(fixed(11, 100, 100), fixed(22, 200, 200));
        assert_eq!(
            decode_quota(&longer_duration, None).unwrap().unwrap(),
            expected(22, 78, 200, 12_000)
        );

        let later_reset = response(fixed(33, 300, 200), fixed(44, 200, 200));
        assert_eq!(
            decode_quota(&later_reset, None).unwrap().unwrap(),
            expected(33, 67, 300, 12_000)
        );

        let primary_tie = response(fixed(55, 300, 200), fixed(66, 300, 200));
        assert_eq!(
            decode_quota(&primary_tie, None).unwrap().unwrap(),
            expected(55, 45, 300, 12_000)
        );
    }

    #[test]
    fn quota_semantics_p2c_plan_family_matrix_and_enterprise_variants() {
        let compatibility = plan_family_compatibility();
        for (account_index, &account_plan) in PlanType::VALUES.iter().enumerate() {
            for (quota_index, &quota_plan) in PlanType::VALUES.iter().enumerate() {
                let value = json!({"rateLimits": {"planType": quota_plan}});
                let result = decode_quota_for_plan(&value, Some(account_plan));
                if compatibility[account_index][quota_index] {
                    assert_eq!(
                        result,
                        Ok(None),
                        "compatible pair returned unexpected quota result"
                    );
                } else {
                    assert_eq!(
                        result,
                        Err(ContractError::AccountQuotaPlanMismatch),
                        "incompatible pair returned an unexpected result"
                    );
                }
            }
        }

        let enterprise = [
            "ent26",
            "enterprise_cbp_automation",
            "enterprise_cbp_usage_based",
            "enterprise",
        ];
        for account_plan in enterprise {
            for quota_plan in enterprise {
                let mut individual = valid_individual();
                individual
                    .as_object_mut()
                    .expect("individual object")
                    .insert("resetsAt".to_owned(), json!(1_709_251_200_i64));
                let value = json!({
                    "rateLimits": {
                        "planType": quota_plan,
                        "individualLimit": individual
                    }
                });
                let snapshot = decode_quota_for_plan(&value, Some(account_plan))
                    .unwrap()
                    .unwrap();
                assert!(snapshot.monthly);
                assert_eq!(snapshot.remaining_percent, Some(73));
                assert_eq!(snapshot.fixed_used_percent, None);
            }
        }
    }

    #[test]
    fn quota_semantics_p2c_calendar_month_literals() {
        let response = |reset_at: i64| {
            json!({
                "rateLimits": {
                    "individualLimit": {
                        "limit": "100",
                        "remainingPercent": 73,
                        "resetsAt": reset_at,
                        "used": "27"
                    }
                }
            })
        };
        let cases = [
            (1_709_251_200_i64, 2_505_600_i64),
            (1_677_628_800_i64, 2_419_200_i64),
            (1_714_521_600_i64, 2_592_000_i64),
            (1_706_745_600_i64, 2_678_400_i64),
            (1_705_322_096_i64, 2_678_400_i64),
            (951_868_800_i64, 2_505_600_i64),
            (4_107_542_400_i64, 2_419_200_i64),
            (13_574_649_600_i64, 2_505_600_i64),
            (1_711_843_200_i64, 2_678_400_i64),
            (1_680_220_800_i64, 2_678_400_i64),
            (i64::MAX, 2_592_000_i64),
        ];
        for (reset_at, expected_window_seconds) in cases {
            let snapshot = decode_quota(&response(reset_at), Some(PlanFamily::Enterprise))
                .unwrap()
                .unwrap();
            assert_eq!(snapshot.reset_at, reset_at);
            assert_eq!(snapshot.window_seconds, expected_window_seconds);
            assert!(snapshot.monthly);
            assert_eq!(snapshot.remaining_percent, Some(73));
            assert_eq!(snapshot.fixed_used_percent, None);
            assert!(!snapshot.unlimited);
        }
    }

    #[test]
    fn quota_semantics_p2c_metamorphic_ignored_fields() {
        let canonical = || {
            json!({
                "rateLimits": {
                    "primary": {
                        "usedPercent": 31,
                        "resetsAt": 1_709_251_200_i64,
                        "windowDurationMins": 10_080
                    },
                    "credits": {
                        "hasCredits": false,
                        "unlimited": false,
                        "balance": "0"
                    },
                    "limitId": "base-id",
                    "rateLimitReachedType": "rate_limit_reached",
                    "spendControlReached": false
                }
            })
        };
        let baseline = decode_quota(&canonical(), Some(PlanFamily::Pro))
            .unwrap()
            .unwrap();
        assert_eq!(
            baseline,
            QuotaSnapshot {
                fixed_used_percent: Some(31),
                remaining_percent: Some(69),
                reset_at: 1_709_251_200,
                window_seconds: 604_800,
                limit_name: "Codex".to_owned(),
                monthly: false,
                unlimited: false,
            }
        );
        let assert_same = |value: Value| {
            assert_eq!(
                decode_quota(&value, Some(PlanFamily::Pro)).unwrap(),
                Some(baseline.clone())
            );
        };

        let mut by_id_value = Map::new();
        by_id_value.insert(
            "model-a".to_owned(),
            json!({"primary": {"usedPercent": 99, "resetsAt": 2, "windowDurationMins": 1}}),
        );
        let mut value = canonical();
        value
            .as_object_mut()
            .expect("root object")
            .insert("rateLimitsByLimitId".to_owned(), Value::Object(by_id_value));
        assert_same(value);

        let mut by_id_order = Map::new();
        by_id_order.insert("model-z".to_owned(), json!({}));
        by_id_order.insert(
            "model-a".to_owned(),
            json!({"primary": {"usedPercent": 99, "resetsAt": 2, "windowDurationMins": 1}}),
        );
        let mut value = canonical();
        value
            .as_object_mut()
            .expect("root object")
            .insert("rateLimitsByLimitId".to_owned(), Value::Object(by_id_order));
        assert_same(value);

        let mut by_id_count = Map::new();
        by_id_count.insert("model-a".to_owned(), json!({}));
        by_id_count.insert("model-b".to_owned(), json!({}));
        by_id_count.insert("model-c".to_owned(), json!({}));
        let mut value = canonical();
        value
            .as_object_mut()
            .expect("root object")
            .insert("rateLimitsByLimitId".to_owned(), Value::Object(by_id_count));
        assert_same(value);

        let mut value = canonical();
        value.as_object_mut().expect("root object").insert(
            "rateLimitResetCredits".to_owned(),
            json!({"availableCount": 0, "credits": []}),
        );
        assert_same(value);

        let mut value = canonical();
        value.as_object_mut().expect("root object").insert(
            "rateLimitResetCredits".to_owned(),
            json!({"availableCount": 1, "credits": [valid_reset_credit()]}),
        );
        assert_same(value);

        let mut second_credit = valid_reset_credit();
        second_credit
            .as_object_mut()
            .expect("reset credit object")
            .insert("id".to_owned(), json!("credit-2"));
        second_credit
            .as_object_mut()
            .expect("reset credit object")
            .insert("grantedAt".to_owned(), json!(2_i64));
        let mut value = canonical();
        value.as_object_mut().expect("root object").insert(
            "rateLimitResetCredits".to_owned(),
            json!({
                "availableCount": 2,
                "credits": [second_credit, valid_reset_credit()]
            }),
        );
        assert_same(value);

        let mut value = canonical();
        value
            .get_mut("rateLimits")
            .expect("rate limits")
            .as_object_mut()
            .expect("snapshot object")
            .insert("limitId".to_owned(), json!("other-id"));
        assert_same(value);

        let mut value = canonical();
        value
            .get_mut("rateLimits")
            .expect("rate limits")
            .as_object_mut()
            .expect("snapshot object")
            .insert(
                "rateLimitReachedType".to_owned(),
                json!("workspace_member_usage_limit_reached"),
            );
        assert_same(value);

        let mut value = canonical();
        value
            .get_mut("rateLimits")
            .expect("rate limits")
            .as_object_mut()
            .expect("snapshot object")
            .insert("spendControlReached".to_owned(), json!(true));
        assert_same(value);

        let mut value = canonical();
        value
            .as_object_mut()
            .expect("root object")
            .insert("rootExtra".to_owned(), json!({"kept": true}));
        assert_same(value);

        let mut value = canonical();
        value
            .get_mut("rateLimits")
            .expect("rate limits")
            .as_object_mut()
            .expect("snapshot object")
            .insert("snapshotExtra".to_owned(), json!({"kept": true}));
        assert_same(value);

        let mut value = canonical();
        value
            .get_mut("rateLimits")
            .expect("rate limits")
            .get_mut("primary")
            .expect("primary window")
            .as_object_mut()
            .expect("primary object")
            .insert("windowExtra".to_owned(), json!({"kept": true}));
        assert_same(value);

        let mut value = canonical();
        value
            .get_mut("rateLimits")
            .expect("rate limits")
            .get_mut("credits")
            .expect("credits object")
            .as_object_mut()
            .expect("credits map")
            .insert("creditsExtra".to_owned(), json!({"kept": true}));
        assert_same(value);

        let mut value = canonical();
        let credits = value
            .get_mut("rateLimits")
            .expect("rate limits")
            .get_mut("credits")
            .expect("credits object")
            .as_object_mut()
            .expect("credits map");
        credits.insert("balance".to_owned(), json!("999"));
        credits.insert("hasCredits".to_owned(), json!(true));
        assert_same(value);

        let mut invalid_by_id = canonical();
        invalid_by_id.as_object_mut().expect("root object").insert(
            "rateLimitsByLimitId".to_owned(),
            json!({"invalid": {"primary": {"usedPercent": "invalid"}}}),
        );
        assert_eq!(
            decode_quota(&invalid_by_id, Some(PlanFamily::Pro)),
            Err(ContractError::QuotaSchema)
        );

        let mut invalid_reset_credits = canonical();
        invalid_reset_credits
            .as_object_mut()
            .expect("root object")
            .insert(
                "rateLimitResetCredits".to_owned(),
                json!({"availableCount": 1, "credits": [{}]}),
            );
        assert_eq!(
            decode_quota(&invalid_reset_credits, Some(PlanFamily::Pro)),
            Err(ContractError::QuotaSchema)
        );

        let no_candidate = json!({
            "rateLimits": {"planType": "pro"},
            "rateLimitsByLimitId": {
                "model-a": {"primary": valid_window()},
                "model-b": {"individualLimit": valid_individual()}
            },
            "rateLimitResetCredits": {
                "availableCount": 1,
                "credits": [valid_reset_credit()]
            }
        });
        assert_eq!(decode_quota(&no_candidate, Some(PlanFamily::Pro)), Ok(None));
    }

    #[test]
    fn protocol_p2d_qv13_limit_name_default_partition() {
        let values = vec![
            json!({
                "rateLimits": {
                    "primary": {
                        "usedPercent": 31,
                        "resetsAt": 1709251200,
                        "windowDurationMins": 10080
                    }
                }
            }),
            json!({
                "rateLimits": {
                    "primary": {
                        "usedPercent": 31,
                        "resetsAt": 1709251200,
                        "windowDurationMins": 10080
                    },
                    "limitName": null
                }
            }),
            json!({
                "rateLimits": {
                    "primary": {
                        "usedPercent": 31,
                        "resetsAt": 1709251200,
                        "windowDurationMins": 10080
                    },
                    "limitName": ""
                }
            }),
            json!({
                "rateLimits": {
                    "primary": {
                        "usedPercent": 31,
                        "resetsAt": 1709251200,
                        "windowDurationMins": 10080
                    },
                    "limitName": "   "
                }
            }),
            json!({
                "rateLimits": {
                    "primary": {
                        "usedPercent": 31,
                        "resetsAt": 1709251200,
                        "windowDurationMins": 10080
                    },
                    "limitName": "\u{0000}\u{001f}\u{007f}"
                }
            }),
        ];

        for value in values {
            let expected = QuotaSnapshot {
                fixed_used_percent: Some(31),
                remaining_percent: Some(69),
                reset_at: 1709251200,
                window_seconds: 604800,
                limit_name: "Codex".to_owned(),
                monthly: false,
                unlimited: false,
            };
            assert_eq!(decode_quota(&value, None), Ok(Some(expected.clone())));
            assert_eq!(
                decode_quota_for_plan(&value, Some("pro")),
                Ok(Some(expected))
            );
        }
    }

    #[test]
    fn protocol_p2d_qv13_limit_name_normalization_and_scalar_boundaries() {
        let scalar_96 = "界".repeat(96);
        let scalar_97 = "界".repeat(97);
        let first_95 = "界".repeat(95);
        assert_eq!(scalar_96.chars().count(), 96);
        assert_eq!(scalar_97.chars().count(), 97);
        assert!(scalar_96.len() > scalar_96.chars().count());

        let values = vec![
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 31,
                            "resetsAt": 1709251200,
                            "windowDurationMins": 10080
                        },
                        "limitName": "Custom"
                    }
                }),
                "Custom".to_owned(),
            ),
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 31,
                            "resetsAt": 1709251200,
                            "windowDurationMins": 10080
                        },
                        "limitName": "Custom\u{0000}\u{0001}\u{007f}\u{009f}\u{061c}\u{200e}\u{200f}\u{2028}\u{202e}\u{2066}\u{2069}Name"
                    }
                }),
                "Custom Name".to_owned(),
            ),
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 31,
                            "resetsAt": 1709251200,
                            "windowDurationMins": 10080
                        },
                        "limitName": scalar_96.clone()
                    }
                }),
                scalar_96.clone(),
            ),
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 31,
                            "resetsAt": 1709251200,
                            "windowDurationMins": 10080
                        },
                        "limitName": scalar_97.clone()
                    }
                }),
                format!("{}…", first_95),
            ),
        ];

        for (value, expected_name) in values {
            let expected = QuotaSnapshot {
                fixed_used_percent: Some(31),
                remaining_percent: Some(69),
                reset_at: 1709251200,
                window_seconds: 604800,
                limit_name: expected_name,
                monthly: false,
                unlimited: false,
            };
            assert_eq!(decode_quota(&value, None), Ok(Some(expected.clone())));
            assert_eq!(
                decode_quota_for_plan(&value, Some("pro")),
                Ok(Some(expected))
            );
        }
    }

    #[test]
    fn protocol_p2d_pv03_pv04_email_public_boundaries() {
        let account = |email: Value| {
            json!({
                "requiresOpenaiAuth": false,
                "account": {
                    "type": "chatgpt",
                    "email": email,
                    "planType": "pro"
                }
            })
        };

        assert_eq!(
            decode_account(&json!({"requiresOpenaiAuth": false})),
            Ok(AccountOutcome::UnsupportedNoData)
        );
        assert_eq!(
            decode_account(&json!({"requiresOpenaiAuth": false, "account": null})),
            Ok(AccountOutcome::UnsupportedNoData)
        );
        assert_eq!(
            decode_account(&account(Value::Null)),
            Ok(AccountOutcome::UnsupportedNoData)
        );
        assert_eq!(
            decode_account(&json!({
                "requiresOpenaiAuth": false,
                "account": {"type": "chatgpt", "planType": "pro"}
            })),
            Err(ContractError::AccountSchema)
        );

        let ranges = [
            (0x0000_u32, 0x001f_u32),
            (0x007f_u32, 0x009f_u32),
            (0x061c_u32, 0x061c_u32),
            (0x200e_u32, 0x200f_u32),
            (0x2028_u32, 0x202e_u32),
            (0x2066_u32, 0x2069_u32),
        ];
        for &(start, end) in &ranges {
            for code in start..=end {
                let scalar = char::from_u32(code).expect("valid Unicode scalar");
                assert_eq!(
                    decode_account(&account(json!(scalar.to_string()))),
                    Ok(AccountOutcome::UnsupportedNoData),
                    "forbidden scalar U+{code:04X} was not treated as empty"
                );
            }
        }

        for whitespace in ["", " ", "\t", "\n", "\r", "\u{000b}", "\u{000c}"] {
            assert_eq!(
                decode_account(&account(json!(whitespace))),
                Ok(AccountOutcome::UnsupportedNoData),
                "ASCII whitespace {whitespace:?} was not treated as empty"
            );
        }

        assert_eq!(
            decode_account(&account(json!("user@example.com"))),
            Ok(AccountOutcome::Supported {
                email: "user@example.com".to_owned(),
                plan_type: PlanType::Pro,
            })
        );
        assert_eq!(
            decode_account(&account(json!("user\u{0000}\u{0001}@example.com"))),
            Ok(AccountOutcome::Supported {
                email: "user @example.com".to_owned(),
                plan_type: PlanType::Pro,
            })
        );

        let email_254 = "界".repeat(254);
        let email_255 = "界".repeat(255);
        let expected_email_255 = format!("{}…", "界".repeat(253));
        assert_eq!(email_254.chars().count(), 254);
        assert_eq!(email_255.chars().count(), 255);
        assert_eq!(expected_email_255.chars().count(), 254);
        assert_eq!(
            decode_account(&account(json!(email_254.clone()))),
            Ok(AccountOutcome::Supported {
                email: email_254,
                plan_type: PlanType::Pro,
            })
        );
        assert_eq!(
            decode_account(&account(json!(email_255))),
            Ok(AccountOutcome::Supported {
                email: expected_email_255,
                plan_type: PlanType::Pro,
            })
        );
    }

    #[test]
    fn protocol_p2d_qv13_no_invalid_default_public_and_static() {
        for invalid in [json!(true), json!(1), json!([]), json!({})] {
            let value = json!({
                "rateLimits": {
                    "primary": {
                        "usedPercent": 31,
                        "resetsAt": 1709251200,
                        "windowDurationMins": 10080
                    },
                    "limitName": invalid
                }
            });
            assert_eq!(
                validate_rate_limits_response(&value),
                Err(ContractError::QuotaSchema)
            );
            assert_eq!(decode_quota(&value, None), Err(ContractError::QuotaSchema));
            assert_eq!(
                decode_quota_for_plan(&value, Some("pro")),
                Err(ContractError::QuotaSchema)
            );
        }

        let source = include_str!("protocol_contract.rs");
        let function_start = source
            .find("fn bounded_limit_name")
            .expect("bounded_limit_name definition");
        let body_start = function_start
            + source[function_start..]
                .find('{')
                .expect("bounded_limit_name body");
        let signature = &source[function_start..body_start];
        assert!(signature.contains("-> Result<String, ContractError>"));

        let mut depth = 0usize;
        let mut body_end = None;
        for (offset, byte) in source.as_bytes()[body_start..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        body_end = Some(body_start + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &source[body_start..=body_end.expect("bounded_limit_name body end")];
        assert!(!body.contains("unwrap_or"));
        assert!(!body.contains("unwrap_or_else"));
    }

    #[test]
    fn protocol_p2d_qv15_by_id_item_schema_rejection_public() {
        for invalid in [
            Value::Null,
            json!(true),
            json!(1),
            json!("invalid"),
            json!([]),
        ] {
            let mut value = json!({"rateLimits": {}});
            value.as_object_mut().expect("quota root object").insert(
                "rateLimitsByLimitId".to_owned(),
                json!({"invalid": invalid}),
            );
            assert_eq!(
                validate_rate_limits_response(&value),
                Err(ContractError::QuotaSchema)
            );
            assert_eq!(decode_quota(&value, None), Err(ContractError::QuotaSchema));
        }

        let valid = json!({
            "rateLimits": {},
            "rateLimitsByLimitId": {"valid": {}}
        });
        assert_eq!(validate_rate_limits_response(&valid), Ok(()));
        assert_eq!(decode_quota(&valid, None), Ok(None));
    }

    #[test]
    fn protocol_p2d_qv19_reset_credit_item_schema_rejection_public() {
        for invalid in [
            Value::Null,
            json!(true),
            json!(1),
            json!("invalid"),
            json!([]),
        ] {
            let value = json!({
                "rateLimits": {},
                "rateLimitResetCredits": {
                    "availableCount": 1,
                    "credits": [invalid]
                }
            });
            assert_eq!(
                validate_rate_limits_response(&value),
                Err(ContractError::QuotaSchema)
            );
            assert_eq!(decode_quota(&value, None), Err(ContractError::QuotaSchema));
        }

        let valid = json!({
            "rateLimits": {},
            "rateLimitResetCredits": {
                "availableCount": 1,
                "credits": [{
                    "grantedAt": 1709251200,
                    "id": "credit-id",
                    "resetType": "codexRateLimits",
                    "status": "available",
                    "description": "description",
                    "title": "title",
                    "expiresAt": 1709856000
                }]
            }
        });
        assert_eq!(validate_rate_limits_response(&valid), Ok(()));
        assert_eq!(decode_quota(&valid, None), Ok(None));
    }

    #[test]
    fn protocol_p2d_pv01_pv11_public_validator_precedence() {
        let auth_required = json!({
            "requiresOpenaiAuth": true,
            "account": null
        });
        assert_eq!(validate_account_response(&auth_required), Ok(()));
        assert_eq!(
            decode_account(&auth_required),
            Ok(AccountOutcome::AuthRequired)
        );

        let chatgpt = json!({
            "requiresOpenaiAuth": false,
            "account": {
                "type": "chatgpt",
                "email": "a@example.com",
                "planType": "pro"
            }
        });
        assert_eq!(validate_account_response(&chatgpt), Ok(()));
        assert_eq!(
            decode_account(&chatgpt),
            Ok(AccountOutcome::Supported {
                email: "a@example.com".to_owned(),
                plan_type: PlanType::Pro
            })
        );

        let api_key = json!({
            "requiresOpenaiAuth": false,
            "account": {"type": "apiKey"}
        });
        assert_eq!(validate_account_response(&api_key), Ok(()));
        assert_eq!(
            decode_account(&api_key),
            Ok(AccountOutcome::UnsupportedNoData)
        );

        let amazon_bedrock = json!({
            "requiresOpenaiAuth": false,
            "account": {
                "type": "amazonBedrock",
                "usesCodexManagedCredentials": true
            }
        });
        assert_eq!(validate_account_response(&amazon_bedrock), Ok(()));
        assert_eq!(
            decode_account(&amazon_bedrock),
            Ok(AccountOutcome::UnsupportedNoData)
        );

        for account in [
            json!(true),
            json!({"type": "unknown"}),
            json!({"type": "chatgpt", "planType": "pro"}),
            json!({
                "type": "chatgpt",
                "email": "a@example.com",
                "planType": "invalid"
            }),
            json!({
                "type": "amazonBedrock",
                "usesCodexManagedCredentials": "true"
            }),
        ] {
            let value = json!({
                "requiresOpenaiAuth": true,
                "account": account
            });
            assert_eq!(
                validate_account_response(&value),
                Err(ContractError::AccountSchema)
            );
            assert_eq!(decode_account(&value), Err(ContractError::AccountSchema));
        }
    }

    #[test]
    fn protocol_p2d_qd05_wrapperless_root_rejects_public() {
        for value in [
            json!({
                "primary": {
                    "usedPercent": 31,
                    "resetsAt": 1709251200,
                    "windowDurationMins": 10080
                }
            }),
            json!({
                "individualLimit": {
                    "limit": "100",
                    "remainingPercent": 73,
                    "resetsAt": 1709251200,
                    "used": "27"
                }
            }),
            json!({
                "credits": {
                    "hasCredits": true,
                    "unlimited": false,
                    "balance": "0"
                }
            }),
            json!({"planType": "pro"}),
        ] {
            assert_eq!(
                validate_rate_limits_response(&value),
                Err(ContractError::QuotaSchema)
            );
            assert_eq!(decode_quota(&value, None), Err(ContractError::QuotaSchema));
        }
    }

    fn literal_alias_mutations(wire: &str) -> [String; 8] {
        [
            format!(" {wire}"),
            format!("{wire} "),
            wire.to_ascii_uppercase(),
            if wire.contains('_') {
                wire.replace('_', "-")
            } else {
                format!("{wire}-separator")
            },
            if wire.contains('_') {
                wire.replace('_', "")
            } else {
                format!("{wire}separator")
            },
            format!("prefix_{wire}"),
            format!("{wire}_suffix"),
            format!("{wire}界"),
        ]
    }

    #[test]
    fn protocol_p2d_pv05_public_plan_type_literal_table() {
        let cases = [
            ("free", PlanType::Free),
            ("go", PlanType::Go),
            ("plus", PlanType::Plus),
            ("pro", PlanType::Pro),
            ("prolite", PlanType::ProLite),
            ("team", PlanType::Team),
            (
                "self_serve_business_prolite",
                PlanType::SelfServeBusinessProLite,
            ),
            (
                "self_serve_business_usage_based",
                PlanType::SelfServeBusinessUsageBased,
            ),
            ("business", PlanType::Business),
            ("ent26", PlanType::Ent26),
            (
                "enterprise_cbp_automation",
                PlanType::EnterpriseCbpAutomation,
            ),
            (
                "enterprise_cbp_usage_based",
                PlanType::EnterpriseCbpUsageBased,
            ),
            ("enterprise", PlanType::Enterprise),
            ("edu", PlanType::Edu),
            ("unknown", PlanType::Unknown),
        ];
        for (wire, expected) in cases {
            assert_eq!(plan_type_from_wire(Some(wire)), Some(expected));
        }
        assert_eq!(plan_type_from_wire(None), None);
        assert_eq!(plan_type_from_wire(Some("")), None);
        assert_eq!(plan_type_from_wire(Some("pro_plan")), None);
        assert_eq!(plan_type_from_wire(Some("not-listed")), None);
    }

    #[test]
    fn protocol_p2d_pv05_public_plan_family_literal_table() {
        let cases = [
            ("free", PlanFamily::Free),
            ("go", PlanFamily::Go),
            ("plus", PlanFamily::Plus),
            ("pro", PlanFamily::Pro),
            ("prolite", PlanFamily::ProLite),
            ("team", PlanFamily::Team),
            ("self_serve_business_prolite", PlanFamily::Business),
            ("self_serve_business_usage_based", PlanFamily::Business),
            ("business", PlanFamily::Business),
            ("ent26", PlanFamily::Enterprise),
            ("enterprise_cbp_automation", PlanFamily::Enterprise),
            ("enterprise_cbp_usage_based", PlanFamily::Enterprise),
            ("enterprise", PlanFamily::Enterprise),
            ("edu", PlanFamily::Edu),
            ("unknown", PlanFamily::Unset),
        ];
        for (wire, expected) in cases {
            assert_eq!(plan_family_from_wire(Some(wire)), Some(expected));
        }
        assert_eq!(plan_family_from_wire(None), None);
        assert_eq!(plan_family_from_wire(Some("")), None);
        assert_eq!(plan_family_from_wire(Some("pro_plan")), None);
        assert_eq!(plan_family_from_wire(Some("not-listed")), None);
    }

    #[test]
    fn protocol_p2d_pv05_all_alias_mutations_reject_public_account() {
        let wires = [
            "free",
            "go",
            "plus",
            "pro",
            "prolite",
            "team",
            "self_serve_business_prolite",
            "self_serve_business_usage_based",
            "business",
            "ent26",
            "enterprise_cbp_automation",
            "enterprise_cbp_usage_based",
            "enterprise",
            "edu",
            "unknown",
        ];
        let mut visited = 0;
        for wire in wires {
            let aliases = literal_alias_mutations(wire);
            assert_eq!(aliases.len(), 8);
            for (index, alias) in aliases.iter().enumerate() {
                assert_ne!(alias, wire);
                assert!(aliases
                    .iter()
                    .enumerate()
                    .all(|(other_index, other)| index == other_index || alias != other));
                let value = json!({
                    "requiresOpenaiAuth": false,
                    "account": {
                        "type": "chatgpt",
                        "email": "user@example.com",
                        "planType": alias
                    }
                });
                assert_eq!(
                    validate_account_response(&value),
                    Err(ContractError::AccountSchema)
                );
                assert_eq!(decode_account(&value), Err(ContractError::AccountSchema));
                visited += 1;
            }
        }
        assert_eq!(visited, 15 * 8);
    }

    #[test]
    fn protocol_p2d_pv05_account_outcome_family_literal_table() {
        let cases = [
            (PlanType::Free, PlanFamily::Free),
            (PlanType::Go, PlanFamily::Go),
            (PlanType::Plus, PlanFamily::Plus),
            (PlanType::Pro, PlanFamily::Pro),
            (PlanType::ProLite, PlanFamily::ProLite),
            (PlanType::Team, PlanFamily::Team),
            (PlanType::SelfServeBusinessProLite, PlanFamily::Business),
            (PlanType::SelfServeBusinessUsageBased, PlanFamily::Business),
            (PlanType::Business, PlanFamily::Business),
            (PlanType::Ent26, PlanFamily::Enterprise),
            (PlanType::EnterpriseCbpAutomation, PlanFamily::Enterprise),
            (PlanType::EnterpriseCbpUsageBased, PlanFamily::Enterprise),
            (PlanType::Enterprise, PlanFamily::Enterprise),
            (PlanType::Edu, PlanFamily::Edu),
            (PlanType::Unknown, PlanFamily::Unset),
        ];
        for (plan_type, expected) in cases {
            let outcome = AccountOutcome::Supported {
                email: "user@example.com".to_owned(),
                plan_type,
            };
            assert_eq!(outcome.plan_family(), Some(expected));
        }
        assert_eq!(AccountOutcome::AuthRequired.plan_family(), None);
        assert_eq!(AccountOutcome::UnsupportedNoData.plan_family(), None);
    }

    #[test]
    fn protocol_p2d_qv16_all_alias_mutations_reject_public_quota() {
        let wires = [
            "free",
            "go",
            "plus",
            "pro",
            "prolite",
            "team",
            "self_serve_business_prolite",
            "self_serve_business_usage_based",
            "business",
            "ent26",
            "enterprise_cbp_automation",
            "enterprise_cbp_usage_based",
            "enterprise",
            "edu",
            "unknown",
        ];
        let mut account_argument_visited = 0;
        let mut canonical_plan_visited = 0;
        for wire in wires {
            let aliases = literal_alias_mutations(wire);
            assert_eq!(aliases.len(), 8);
            for (index, alias) in aliases.iter().enumerate() {
                assert_ne!(alias, wire);
                assert!(aliases
                    .iter()
                    .enumerate()
                    .all(|(other_index, other)| index == other_index || alias != other));
                let value = json!({"rateLimits": {}});
                assert_eq!(
                    decode_quota_for_plan(&value, Some(alias)),
                    Err(ContractError::InvalidPlanType)
                );
                account_argument_visited += 1;

                let canonical_value = json!({
                    "rateLimits": {
                        "planType": alias
                    }
                });
                assert_eq!(
                    validate_rate_limits_response(&canonical_value),
                    Err(ContractError::QuotaSchema)
                );
                assert_eq!(
                    decode_quota(&canonical_value, None),
                    Err(ContractError::QuotaSchema)
                );
                assert_eq!(
                    decode_quota_for_plan(&canonical_value, Some("pro")),
                    Err(ContractError::QuotaSchema)
                );
                canonical_plan_visited += 1;
            }
        }
        assert_eq!(account_argument_visited, 15 * 8);
        assert_eq!(canonical_plan_visited, 15 * 8);
    }

    #[test]
    fn protocol_p2d_qv17_qv18_exact_15x15_public_matrix() {
        let wires = [
            "free",
            "go",
            "plus",
            "pro",
            "prolite",
            "team",
            "self_serve_business_prolite",
            "self_serve_business_usage_based",
            "business",
            "ent26",
            "enterprise_cbp_automation",
            "enterprise_cbp_usage_based",
            "enterprise",
            "edu",
            "unknown",
        ];
        let compatibility = plan_family_compatibility();
        let mut visited = 0;
        for (account_index, &account_plan) in wires.iter().enumerate() {
            for (quota_index, &quota_plan) in wires.iter().enumerate() {
                let value = json!({"rateLimits": {"planType": quota_plan}});
                let result = decode_quota_for_plan(&value, Some(account_plan));
                if compatibility[account_index][quota_index] {
                    assert_eq!(
                        result,
                        Ok(None),
                        "compatible pair returned unexpected quota result"
                    );
                } else {
                    assert_eq!(
                        result,
                        Err(ContractError::AccountQuotaPlanMismatch),
                        "incompatible pair returned an unexpected result"
                    );
                }
                visited += 1;
            }
        }
        assert_eq!(visited, 15 * 15);
    }

    #[test]
    fn protocol_p2d_qv16_unknown_unset_complete_public_table() {
        type PlanCase<'a> = (Option<&'a str>, Option<Option<&'a str>>, QuotaSnapshot);
        let cases: [PlanCase<'_>; 14] = [
            (
                None,
                None,
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                Some("unknown"),
                None,
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                None,
                Some(None),
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                Some("unknown"),
                Some(None),
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                None,
                Some(Some("unknown")),
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                Some("unknown"),
                Some(Some("unknown")),
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                None,
                Some(Some("enterprise")),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1709251200,
                    window_seconds: 2505600,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                Some("unknown"),
                Some(Some("enterprise")),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1709251200,
                    window_seconds: 2505600,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                Some("enterprise"),
                None,
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1709251200,
                    window_seconds: 2505600,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                Some("enterprise"),
                Some(None),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1709251200,
                    window_seconds: 2505600,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                Some("enterprise"),
                Some(Some("unknown")),
                QuotaSnapshot {
                    fixed_used_percent: None,
                    remaining_percent: Some(73),
                    reset_at: 1709251200,
                    window_seconds: 2505600,
                    limit_name: "Codex".to_owned(),
                    monthly: true,
                    unlimited: false,
                },
            ),
            (
                Some("pro"),
                None,
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                Some("pro"),
                Some(None),
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                Some("pro"),
                Some(Some("unknown")),
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
        ];
        let payload = |quota_plan: Option<Option<&str>>| {
            let mut rate_limits = json!({
                "individualLimit": {
                    "limit": "100",
                    "remainingPercent": 73,
                    "resetsAt": 1709251200,
                    "used": "27"
                },
                "primary": {
                    "usedPercent": 31,
                    "resetsAt": 999,
                    "windowDurationMins": 60
                }
            });
            if let Some(plan_type) = quota_plan {
                rate_limits
                    .as_object_mut()
                    .expect("rateLimits object")
                    .insert(
                        "planType".to_owned(),
                        plan_type.map_or(Value::Null, |wire| json!(wire)),
                    );
            }
            json!({"rateLimits": rate_limits})
        };

        let mut visited = 0;
        for (account_plan, quota_plan, expected) in cases {
            let value = payload(quota_plan);
            let result = match account_plan {
                None => decode_quota(&value, None),
                Some(account_plan) => decode_quota_for_plan(&value, Some(account_plan)),
            };
            assert_eq!(result, Ok(Some(expected)));
            visited += 1;
        }
        assert_eq!(visited, 14);
    }

    #[test]
    fn protocol_p2d_qd09_nonenterprise_fixed_beats_other_candidates() {
        let wires = [
            "free",
            "go",
            "plus",
            "pro",
            "prolite",
            "team",
            "self_serve_business_prolite",
            "self_serve_business_usage_based",
            "business",
            "edu",
            "unknown",
        ];
        let mut visited = 0;
        for wire in wires {
            let value = json!({
                "rateLimits": {
                    "individualLimit": {
                        "limit": "100",
                        "remainingPercent": 73,
                        "resetsAt": 1709251200,
                        "used": "27"
                    },
                    "primary": {
                        "usedPercent": 31,
                        "resetsAt": 999,
                        "windowDurationMins": 60
                    },
                    "credits": {
                        "hasCredits": true,
                        "unlimited": true,
                        "balance": "0"
                    }
                }
            });
            assert_eq!(
                decode_quota_for_plan(&value, Some(wire)),
                Ok(Some(QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                }))
            );
            visited += 1;
        }
        assert_eq!(visited, 11);
    }

    #[test]
    fn protocol_p2d_qd10_qd11_fixed_oracles_are_literal_snapshots() {
        let fixed_cases: [(Value, QuotaSnapshot); 4] = [
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 0,
                            "resetsAt": 999,
                            "windowDurationMins": 60
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: Some(0),
                    remaining_percent: Some(100),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 31,
                            "resetsAt": 999,
                            "windowDurationMins": 60
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: Some(31),
                    remaining_percent: Some(69),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 55,
                            "resetsAt": 999,
                            "windowDurationMins": 60
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: Some(55),
                    remaining_percent: Some(45),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 100,
                            "resetsAt": 999,
                            "windowDurationMins": 60
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: Some(100),
                    remaining_percent: Some(0),
                    reset_at: 999,
                    window_seconds: 3600,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
        ];

        let mut fixed_visited = 0;
        for (value, expected) in fixed_cases {
            assert_eq!(decode_quota(&value, None), Ok(Some(expected.clone())));
            assert_eq!(
                decode_quota_for_plan(&value, Some("pro")),
                Ok(Some(expected))
            );
            fixed_visited += 1;
        }
        assert_eq!(fixed_visited, 4);

        let tie_cases: [(Value, QuotaSnapshot); 3] = [
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 11,
                            "resetsAt": 100,
                            "windowDurationMins": 100
                        },
                        "secondary": {
                            "usedPercent": 22,
                            "resetsAt": 200,
                            "windowDurationMins": 200
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: Some(22),
                    remaining_percent: Some(78),
                    reset_at: 200,
                    window_seconds: 12000,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 33,
                            "resetsAt": 300,
                            "windowDurationMins": 200
                        },
                        "secondary": {
                            "usedPercent": 44,
                            "resetsAt": 200,
                            "windowDurationMins": 200
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: Some(33),
                    remaining_percent: Some(67),
                    reset_at: 300,
                    window_seconds: 12000,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
            (
                json!({
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 55,
                            "resetsAt": 300,
                            "windowDurationMins": 200
                        },
                        "secondary": {
                            "usedPercent": 66,
                            "resetsAt": 300,
                            "windowDurationMins": 200
                        }
                    }
                }),
                QuotaSnapshot {
                    fixed_used_percent: Some(55),
                    remaining_percent: Some(45),
                    reset_at: 300,
                    window_seconds: 12000,
                    limit_name: "Codex".to_owned(),
                    monthly: false,
                    unlimited: false,
                },
            ),
        ];

        let mut tie_visited = 0;
        for (value, expected) in tie_cases {
            assert_eq!(decode_quota(&value, None), Ok(Some(expected.clone())));
            assert_eq!(
                decode_quota_for_plan(&value, Some("pro")),
                Ok(Some(expected))
            );
            tie_visited += 1;
        }
        assert_eq!(tie_visited, 3);
    }

    #[test]
    fn verify_protocol_p2d_static_no_dead_or_circular_evidence() {
        let source = include_str!("protocol_contract.rs");
        let (production, test_region) = source
            .split_once("#[cfg(test)]")
            .expect("test module marker");

        let fixed_calculation = ["100", " - ", "used"].concat();
        assert_eq!(production.matches(&fixed_calculation).count(), 1);
        assert_eq!(test_region.matches(&fixed_calculation).count(), 0);

        let account_schema_helper = ["fn ", "assert_account_schema"].concat();
        assert!(!test_region.contains(&account_schema_helper));

        let expected_plan_parse = ["plan_type:", " PlanType::parse", "("].concat();
        assert!(!test_region.contains(&expected_plan_parse));

        let private_month_call = ["monthly_window_seconds", "("].concat();
        assert!(!test_region.contains(&private_month_call));

        let function_slice = |name: &str| {
            let marker = format!("fn {name}");
            let start = test_region.find(&marker).expect("function definition");
            let tail = &test_region[start..];
            let end = tail
                .find("\n    #[test]")
                .map_or(test_region.len(), |offset| start + offset);
            &test_region[start..end]
        };
        for name in [
            "quota_semantics_p2c_plan_family_matrix_and_enterprise_variants",
            "protocol_p2d_qv17_qv18_exact_15x15_public_matrix",
        ] {
            assert!(
                !function_slice(name).contains("assert_ne"),
                "forbidden assert_ne in {name}"
            );
        }

        let ignore_attribute = ["#[", "ignore"].concat();
        let should_panic_attribute = ["#[", "should_panic"].concat();
        let std_env_gate = ["std::", "env"].concat();
        assert!(!test_region.contains(&ignore_attribute));
        assert!(!test_region.contains(&should_panic_attribute));
        assert!(!test_region.contains(&std_env_gate));

        let required_names = [
            [
                "protocol_p2d_qd10_qd11_fixed_oracles_are_",
                "literal_snapshots",
            ]
            .concat(),
            [
                "protocol_p2d_qv07_calendar_boundaries_use_",
                "public_decoder",
            ]
            .concat(),
            [
                "verify_protocol_p2d_static_no_dead_",
                "or_circular_evidence",
            ]
            .concat(),
        ];
        for name in required_names {
            let marker = format!("fn {name}");
            assert_eq!(test_region.matches(&marker).count(), 1, "{name}");
        }
    }
}

//! Public time scopes. Reported reset deadlines never partition consumption.
use super::{PublishedSnapshot, Route, RouteError};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use codex_info_rest_contract::{recorded_usage::*, PublicState};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(super) fn current(
    snapshot: &PublishedSnapshot,
    degraded: bool,
) -> Result<RecordedCurrent, RouteError> {
    let details = &snapshot.details;
    let archive = snapshot.recordings.as_deref();
    let quota_at = archive
        .and_then(|a| a.quota_observations.last())
        .map(|v| v.observed_at);
    let observed_at = quota_at
        .into_iter()
        .chain(archive.and_then(|a| a.latest_fact_at()))
        .max();
    let usage_scope = observed_at
        .zip(details.quota.as_ref())
        .and_then(|(at, quota)| {
            let end = at.checked_add(1)?;
            let start = end.checked_sub(quota.window_seconds)?;
            RecordingScope::new(
                start,
                end,
                archive.map_or(RecordingCoverage::Unknown, |a| a.coverage),
            )
            .ok()
        });
    let models = match (archive, &usage_scope) {
        (Some(archive), Some(scope)) => archive
            .models(scope.start_at, scope.end_at)
            .map_err(|_| RouteError::HistoryUnavailable)?,
        _ => Vec::new(),
    };
    let models_status = if usage_scope
        .as_ref()
        .is_some_and(|v| v.coverage == RecordingCoverage::Complete)
    {
        "complete"
    } else if models.is_empty() {
        "unavailable"
    } else {
        "partial"
    };
    let state = if !details.authenticated {
        details.state
    } else if degraded || snapshot.acquisition_degraded || archive.is_none() {
        PublicState::Error
    } else if observed_at.is_some() {
        PublicState::Ready
    } else {
        PublicState::Initializing
    };
    let mut value = RecordedCurrent {
        api_version: "v5".into(),
        state,
        observed_at,
        authenticated: details.authenticated,
        plan_label: details.plan_label.clone(),
        quota: details.quota.clone(),
        period_identity: PeriodIdentity::default(),
        usage_scope,
        models_status: models_status.into(),
        models,
        active_thread_count: details.active_thread_count,
        open_session_thread_count: details.threads.len() as u64,
    };
    if !details.authenticated || value.state == PublicState::Initializing {
        value.observed_at = None;
        value.plan_label = None;
        value.quota = None;
        value.usage_scope = None;
        value.models.clear();
        value.models_status = "unavailable".into();
        value.active_thread_count = 0;
        value.open_session_thread_count = 0;
    }
    Ok(value)
}

fn month(at: i64) -> Option<(i64, i64)> {
    let date = DateTime::from_timestamp(at, 0)?;
    let start = Utc
        .with_ymd_and_hms(date.year(), date.month(), 1, 0, 0, 0)
        .single()?;
    let (year, month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    let end = Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).single()?;
    Some((start.timestamp(), end.timestamp()))
}

fn scopes(
    snapshot: &PublishedSnapshot,
    degraded: bool,
) -> Result<RecordingScopeDirectory, RouteError> {
    let archive = snapshot
        .recordings
        .as_ref()
        .ok_or(RouteError::HistoryUnavailable)?;
    let current = current(snapshot, degraded)?;
    let mut scopes = Vec::new();
    let available = |start, end| {
        (
            !archive
                .models(start, end)
                .map(|v| v.is_empty())
                .unwrap_or(true),
            archive
                .observations
                .iter()
                .any(|v| v.timestamp >= start && v.timestamp < end),
        )
    };
    if let Some(scope) = current.usage_scope {
        let (recordings_available, saved_observations_available) =
            available(scope.start_at, scope.end_at);
        scopes.push(RecordingScopeEntry {
            id: "recorded.current".into(),
            kind: "rolling".into(),
            start_at: scope.start_at,
            end_at: scope.end_at,
            current: true,
            recordings_available,
            saved_observations_available,
            coverage: scope.coverage,
        });
    }
    let months = archive
        .prefixes
        .values()
        .flat_map(|v| v.iter().map(|v| v.0))
        .chain(archive.observations.iter().map(|v| v.timestamp))
        .chain(archive.quota_observations.iter().map(|v| v.observed_at))
        .chain(
            snapshot
                .retained_history
                .gaps
                .iter()
                .flat_map(|v| [v.start_at, v.end_at.saturating_sub(1)]),
        )
        .filter_map(month)
        .collect::<BTreeSet<_>>();
    for (start_at, end_at) in months.into_iter().rev() {
        let date = DateTime::from_timestamp(start_at, 0).ok_or(RouteError::Serialization)?;
        let (recordings_available, saved_observations_available) = available(start_at, end_at);
        scopes.push(RecordingScopeEntry {
            id: format!("recorded.month.{:04}-{:02}", date.year(), date.month()),
            kind: "utc-month".into(),
            start_at,
            end_at,
            current: false,
            recordings_available,
            saved_observations_available,
            coverage: archive.coverage,
        });
    }
    Ok(RecordingScopeDirectory {
        api_version: "v6".into(),
        observed_at: current.observed_at,
        period_identity: PeriodIdentity::default(),
        scopes,
        next_cursor: None,
    })
}

fn cursor_prefix(
    snapshot: &PublishedSnapshot,
    resource: char,
    range: Option<(i64, i64)>,
) -> String {
    let (start, end) = range.unwrap_or((0, 0));
    format!(
        "r1.{}.{}.{}.{}.{}.",
        &snapshot.pair[3..],
        snapshot.recorded_scope_account,
        resource,
        start,
        end
    )
}
fn offset(
    snapshot: &PublishedSnapshot,
    resource: char,
    range: Option<(i64, i64)>,
    cursor: Option<&str>,
    total: usize,
) -> Result<usize, RouteError> {
    let Some(cursor) = cursor else { return Ok(0) };
    let text = cursor
        .strip_prefix(&cursor_prefix(snapshot, resource, range))
        .ok_or(RouteError::StaleCursor)?;
    let value = text.parse::<usize>().map_err(|_| RouteError::StaleCursor)?;
    if value.to_string() != text || value == 0 || value >= total {
        return Err(RouteError::StaleCursor);
    }
    Ok(value)
}

/// One serialized prefix; linear byte accounting includes the exact outer
/// shape and the exact next cursor before choosing any item.
fn page(
    mut document: Value,
    arrays: &[(&str, Vec<Value>)],
    start: usize,
    prefix: &str,
    budget: usize,
) -> Result<Vec<u8>, RouteError> {
    let total = arrays.iter().map(|(_, v)| v.len()).sum::<usize>();
    document["next_cursor"] = Value::Null;
    for (key, _) in arrays {
        document[*key] = json!([]);
    }
    let mut size = serde_json::to_vec(&document)
        .map_err(|_| RouteError::Serialization)?
        .len();
    if size > budget {
        return Err(RouteError::ResourceTooLarge);
    }
    let mut position = 0;
    let mut end = start;
    let mut full = false;
    for (key, values) in arrays {
        for value in values {
            if position < start {
                position += 1;
                continue;
            }
            if full {
                break;
            }
            let encoded = serde_json::to_vec(value).map_err(|_| RouteError::Serialization)?;
            let target = document[*key]
                .as_array_mut()
                .ok_or(RouteError::Serialization)?;
            let additional = encoded.len() + usize::from(!target.is_empty());
            let candidate_end = end + 1;
            let cursor_bytes = if candidate_end < total {
                serde_json::to_vec(&format!("{prefix}{candidate_end}"))
                    .map_err(|_| RouteError::Serialization)?
                    .len()
                    - 4
            } else {
                0
            };
            if size + additional + cursor_bytes > budget {
                full = true;
                break;
            }
            size += additional;
            target.push(value.clone());
            end += 1;
            position += 1;
        }
        if full {
            break;
        }
    }
    if end == start && start < total {
        return Err(RouteError::ResourceTooLarge);
    }
    document["next_cursor"] = if end < total {
        json!(format!("{prefix}{end}"))
    } else {
        Value::Null
    };
    let body = serde_json::to_vec(&document).map_err(|_| RouteError::Serialization)?;
    if body.len() > budget {
        return Err(RouteError::ResourceTooLarge);
    }
    Ok(body)
}

pub(super) fn serialize(
    snapshot: &PublishedSnapshot,
    route: Route,
    range: Option<&str>,
    cursor: Option<&str>,
    degraded: bool,
    budget: usize,
) -> Result<Vec<u8>, RouteError> {
    if route == Route::CurrentV5 {
        if snapshot.recordings.is_none() {
            return Err(RouteError::SnapshotUnavailable);
        }
        return serde_json::to_vec(&current(snapshot, degraded)?)
            .map_err(|_| RouteError::Serialization);
    }
    let archive = snapshot
        .recordings
        .as_ref()
        .ok_or(RouteError::HistoryUnavailable)?;
    if route == Route::HistoryScopesV6 {
        let directory = scopes(snapshot, degraded)?;
        let values = directory
            .scopes
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| RouteError::Serialization)?;
        let start = offset(snapshot, 's', None, cursor, values.len())?;
        return page(
            serde_json::to_value(directory).map_err(|_| RouteError::Serialization)?,
            &[("scopes", values)],
            start,
            &cursor_prefix(snapshot, 's', None),
            budget,
        );
    }
    let (start, end) = range
        .and_then(|v| v.split_once(':'))
        .and_then(|(a, b)| a.parse().ok().zip(b.parse().ok()))
        .ok_or(RouteError::Serialization)?;
    let scope =
        RecordingScope::new(start, end, archive.coverage).map_err(|_| RouteError::Serialization)?;
    if route == Route::HistoryRecordingsV4 {
        let samples = archive
            .samples(start, end)
            .map_err(|_| RouteError::HistoryUnavailable)?;
        let samples = samples
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| RouteError::Serialization)?;
        let quota = archive
            .quota_observations
            .iter()
            .filter(|v| scope.contains(v.observed_at))
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| RouteError::Serialization)?;
        let offset = offset(
            snapshot,
            'r',
            Some((start, end)),
            cursor,
            samples.len() + quota.len(),
        )?;
        let body = json!({"api_version":"v4","scope":scope,"period_identity":PeriodIdentity::default(),"coverage_intervals":[{"start_at":start,"end_at":end,"coverage":archive.coverage}],"samples":[],"quota_observations":[],"next_cursor":null});
        return page(
            body,
            &[("samples", samples), ("quota_observations", quota)],
            offset,
            &cursor_prefix(snapshot, 'r', Some((start, end))),
            budget,
        );
    }
    let values = archive
        .observations
        .iter()
        .filter(|v| scope.contains(v.timestamp))
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RouteError::Serialization)?;
    let offset = offset(snapshot, 'o', Some((start, end)), cursor, values.len())?;
    page(
        json!({"api_version":"v4","scope":scope,"period_identity":PeriodIdentity::default(),"observations":[],"next_cursor":null}),
        &[("observations", values)],
        offset,
        &cursor_prefix(snapshot, 'o', Some((start, end))),
        budget,
    )
}

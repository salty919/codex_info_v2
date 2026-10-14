//! Quota history is a read projection of accepted acquisition records.
//! Session history contributes only model facts at the same exact key.

use super::*;

struct Receipt {
    sequence: i64,
    observed_at: i64,
    reset_at: i64,
    window_seconds: i64,
    remaining_percent: f64,
}

/// Restore only historical origin authority. Current acquisition records and
/// canonical minute/model values remain their separate, unchanged sources.
pub(super) fn project_recovered_windows(
    connection: &Connection,
    intervals: &ReadIntervals,
    details: &mut PublicDetails,
    origins: &mut BTreeMap<i64, i64>,
) -> Result<(), ReaderError> {
    if !table_exists(connection, "session_quota_windows")?
        || !table_exists(connection, "session_events")?
    {
        return Ok(());
    }
    let mut statement = connection.prepare(
        "SELECT w.reset_at,w.window_seconds,w.observed_at FROM session_quota_windows w
         WHERE EXISTS(SELECT 1 FROM session_events e WHERE e.root_identity=w.root_identity
             AND e.relative_path=w.relative_path AND e.range_start=w.range_start
             AND e.range_end=w.range_end AND e.record_sha256=w.record_sha256
             AND e.timestamp=w.observed_at)",
    )?;
    let mut windows = BTreeMap::<i64, BTreeSet<i64>>::new();
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })? {
        let (reset, seconds, observed_at) = row?;
        let Some(start) = current_period_start_at(reset, seconds) else {
            continue;
        };
        if !intervals.contains(observed_at)
            || observed_at < start
            || observed_at > reset
            || details
                .observed_at
                .is_none_or(|cutoff| observed_at > cutoff)
        {
            continue;
        }
        windows.entry(reset).or_default().insert(start);
    }
    for period in &mut details.history_periods {
        if origins.contains_key(&period.reset_at) {
            continue;
        }
        let Some(starts) = windows
            .get(&period.reset_at)
            .filter(|values| values.len() == 1)
        else {
            continue;
        };
        let start = *starts.first().expect("one verified window");
        // A canonical bucket may begin before the exact second of its
        // official origin. Earlier complete minutes are a conflicting source.
        if period.start_at.div_euclid(60) < start.div_euclid(60) || start > period.end_at {
            continue;
        }
        period.start_at = start;
        origins.insert(period.reset_at, start);
    }
    clip_history_periods(&mut details.history_periods, intervals);
    assign_history_period_labels(
        &mut details.history_periods,
        details.quota.as_ref().map(|quota| quota.reset_at),
    );
    Ok(())
}

pub(super) fn project(
    connection: &Connection,
    intervals: &ReadIntervals,
    details: &mut PublicDetails,
    history_v2: &mut Vec<PublicHistoryObservation>,
    history_v3: &mut Vec<PublicHistoryObservationV3>,
    history_window: HistoryWindow,
) -> Result<BTreeMap<i64, i64>, ReaderError> {
    let Some(current) = details.quota.as_ref() else {
        return Ok(BTreeMap::new());
    };
    let observed_at = details.observed_at.unwrap_or(0);
    let mut statement = connection.prepare(
        "SELECT sequence, observed_at, reset_at, window_seconds, remaining_percent
         FROM quota_receipts WHERE observed_at > ?1 AND observed_at <= ?2 ORDER BY sequence",
    )?;
    let rows = statement.query_map(
        params![history_window.cutoff(observed_at), observed_at],
        |row| {
            Ok(Receipt {
                sequence: row.get(0)?,
                observed_at: row.get(1)?,
                reset_at: row.get(2)?,
                window_seconds: row.get(3)?,
                remaining_percent: row.get(4)?,
            })
        },
    )?;
    let mut minutes = BTreeMap::new();
    // This order is the acquisition order, not deadline or local-clock order.
    let mut periods_by_receipt = BTreeMap::new();
    for row in rows {
        let receipt = row?;
        if !intervals.contains(receipt.observed_at) {
            continue;
        }
        periods_by_receipt.insert(receipt.reset_at, (receipt.sequence, receipt.window_seconds));
        minutes.insert(
            receipt.observed_at - receipt.observed_at.rem_euclid(60),
            receipt,
        );
    }
    let mut history = std::mem::take(history_v3)
        .into_iter()
        .map(|point| ((point.reset_at, point.timestamp), point))
        .collect::<BTreeMap<_, _>>();
    for (timestamp, receipt) in minutes {
        // A later official reset owns this minute. A model vector recorded
        // under another reset is never moved into the new period.
        history.retain(|(reset, time), _| *time != timestamp || *reset == receipt.reset_at);
        let point = history
            .entry((receipt.reset_at, timestamp))
            .or_insert_with(|| PublicHistoryObservationV3 {
                timestamp,
                reset_at: receipt.reset_at,
                remaining_percent: None,
                task_active_since_previous: None,
                models: None,
                models_complete: false,
                model_source: "unavailable".into(),
            });
        point.remaining_percent = Some(receipt.remaining_percent);
    }

    let mut periods = std::mem::take(&mut details.history_periods)
        .into_iter()
        .map(|period| (period.reset_at, period))
        .collect::<BTreeMap<_, _>>();
    let mut acquisition_periods = periods_by_receipt
        .iter()
        .filter_map(|(reset, (sequence, window))| {
            current_period_start_at(*reset, *window).map(|start| (*sequence, *reset, start))
        })
        .collect::<Vec<_>>();
    acquisition_periods.sort_by_key(|(sequence, _, _)| *sequence);
    for (_, reset, start) in &acquisition_periods {
        let end = history
            .values()
            .filter(|point| point.reset_at == *reset)
            .map(|point| point.timestamp)
            .max()
            .unwrap_or(*start);
        let period = periods
            .entry(*reset)
            .or_insert_with(|| PublicHistoryPeriod {
                id: reset.to_string(),
                reset_at: *reset,
                start_at: *start,
                end_at: end,
                label: String::new(),
                current: false,
            });
        period.start_at = *start;
        period.end_at = period.end_at.max(end).min(*reset).min(observed_at);
    }
    // The current window exists even when the Session lane has no rows.
    if let Some((start, end)) =
        current_period_bounds(current.reset_at, current.window_seconds, observed_at)
    {
        periods.insert(
            current.reset_at,
            PublicHistoryPeriod {
                id: current.reset_at.to_string(),
                reset_at: current.reset_at,
                start_at: start,
                end_at: end,
                label: String::new(),
                current: true,
            },
        );
    }
    for period in periods.values_mut() {
        period.current = period.reset_at == current.reset_at;
        if period.current {
            continue;
        }
        let successor = acquisition_periods
            .iter()
            .position(|(_, reset, _)| *reset == period.reset_at)
            .and_then(|index| acquisition_periods.get(index + 1))
            .or_else(|| {
                (!periods_by_receipt.contains_key(&period.reset_at))
                    .then(|| acquisition_periods.first())
                    .flatten()
            });
        if let Some((_, _, start)) = successor {
            period.end_at = period.end_at.min(start.saturating_sub(1));
        }
    }
    periods.retain(|_, period| period.start_at <= period.end_at);
    let belongs = |reset: i64, timestamp: i64| {
        periods.get(&reset).is_some_and(|period| {
            codex_info_rest_contract::history_minute_in_period(
                timestamp,
                period.start_at,
                period.end_at,
            )
        })
    };
    history.retain(|(reset, timestamp), _| belongs(*reset, *timestamp));
    details.history_gaps.retain(|gap| {
        periods
            .get(&gap.reset_at)
            .is_some_and(|period| gap.start_at >= period.start_at && gap.end_at <= period.end_at)
    });
    *history_v3 = history.into_values().collect();
    *history_v2 = history_observations_v2(&details.history_samples, history_v3);
    details.history_samples = history_samples_v1(history_v2);
    details.history_periods = periods.into_values().collect();
    clip_history_periods(&mut details.history_periods, intervals);
    assign_history_period_labels(&mut details.history_periods, Some(current.reset_at));
    details.history_periods.sort_by(|left, right| {
        right
            .start_at
            .cmp(&left.start_at)
            .then_with(|| right.reset_at.cmp(&left.reset_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    // Keep authority separate from the observed/public bounds. A legacy
    // first-owned sample is never evidence of a reset origin.
    let mut origins = periods_by_receipt
        .iter()
        .filter_map(|(reset, (_, window))| {
            current_period_start_at(*reset, *window).map(|start| (*reset, start))
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(start) = current_period_start_at(current.reset_at, current.window_seconds) {
        origins.insert(current.reset_at, start);
    }
    origins.retain(|reset, start| {
        details
            .history_periods
            .iter()
            .any(|period| period.reset_at == *reset && *start <= period.start_at)
    });
    Ok(origins)
}

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

pub(super) fn project(
    connection: &Connection,
    intervals: &ReadIntervals,
    details: &mut PublicDetails,
    history_v2: &mut Vec<PublicHistoryObservation>,
    history_v3: &mut Vec<PublicHistoryObservationV3>,
) -> Result<(), ReaderError> {
    let Some(current) = details.quota.as_ref() else {
        return Ok(());
    };
    let observed_at = details.observed_at.unwrap_or(0);
    let mut statement = connection.prepare(
        "SELECT sequence, observed_at, reset_at, window_seconds, remaining_percent
         FROM quota_receipts WHERE observed_at > ?1 AND observed_at <= ?2 ORDER BY sequence",
    )?;
    let rows = statement.query_map(
        params![
            observed_at.saturating_sub(HISTORY_WINDOW_SECONDS),
            observed_at
        ],
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
    Ok(())
}

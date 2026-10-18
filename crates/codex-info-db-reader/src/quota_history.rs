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

#[derive(Default)]
pub(super) struct OriginProjection {
    pub(super) known: BTreeMap<i64, i64>,
    pub(super) conflicting: BTreeSet<i64>,
}

/// Restore only historical origin authority. Current acquisition records and
/// canonical minute/model values remain their separate, unchanged sources.
pub(super) fn project_recovered_windows(
    connection: &Connection,
    intervals: &ReadIntervals,
    details: &mut PublicDetails,
    origins: &mut BTreeMap<i64, i64>,
    conflicting_origins: &BTreeSet<i64>,
    publication: Option<PublicationWindow>,
) -> Result<(), ReaderError> {
    if !table_exists(connection, "session_quota_windows")?
        || !table_exists(connection, "session_events")?
    {
        return Ok(());
    }
    let range_authority = if table_exists(connection, "session_ranges")? {
        " OR EXISTS(SELECT 1 FROM session_ranges r WHERE r.root_identity=w.root_identity
             AND r.relative_path=w.relative_path AND r.start_offset=w.range_start
             AND r.end_offset=w.range_end AND r.record_sha256=w.record_sha256)"
    } else {
        ""
    };
    let mut statement = connection.prepare(&format!(
        "SELECT w.reset_at,w.window_seconds,w.observed_at FROM session_quota_windows w
         WHERE EXISTS(SELECT 1 FROM session_events e WHERE e.root_identity=w.root_identity
             AND e.relative_path=w.relative_path AND e.range_start=w.range_start
             AND e.range_end=w.range_end AND e.record_sha256=w.record_sha256
             AND e.timestamp=w.observed_at){range_authority}",
    ))?;
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
            || observed_at >= reset
            || publication.is_some_and(|window| observed_at > window.raw_upper())
        {
            continue;
        }
        windows.entry(reset).or_default().insert(start);
    }
    for period in &mut details.history_periods {
        if origins.contains_key(&period.reset_at) || conflicting_origins.contains(&period.reset_at)
        {
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
    publication: Option<PublicationWindow>,
) -> Result<OriginProjection, ReaderError> {
    let Some(current) = details.quota.as_ref() else {
        return Ok(OriginProjection::default());
    };
    let observed_at = details.observed_at.unwrap_or(0);
    let (cutoff, history_upper) = publication
        .map(|window| (window.cutoff(history_window), window.raw_upper()))
        .unwrap_or((history_window.cutoff(observed_at), observed_at));
    let mut statement = connection.prepare(
        "SELECT sequence, observed_at, reset_at, window_seconds, remaining_percent
         FROM quota_receipts WHERE observed_at > ?1 AND observed_at <= ?2 ORDER BY sequence",
    )?;
    let rows = statement.query_map(params![cutoff, history_upper], |row| {
        Ok(Receipt {
            sequence: row.get(0)?,
            observed_at: row.get(1)?,
            reset_at: row.get(2)?,
            window_seconds: row.get(3)?,
            remaining_percent: row.get(4)?,
        })
    })?;
    let mut minutes = BTreeMap::new();
    // This order is the acquisition order, not deadline or local-clock order.
    let mut periods_by_receipt = BTreeMap::new();
    let mut receipt_ends = BTreeMap::<i64, i64>::new();
    let mut receipt_origins = BTreeMap::<i64, BTreeSet<i64>>::new();
    for row in rows {
        let receipt = row?;
        if !intervals.contains(receipt.observed_at) {
            continue;
        }
        if !receipt.remaining_percent.is_finite()
            || !(0.0..=100.0).contains(&receipt.remaining_percent)
        {
            return Err(ReaderError::InvalidValue(
                "quota history receipt has an invalid remaining percentage".to_owned(),
            ));
        }
        periods_by_receipt.insert(receipt.reset_at, (receipt.sequence, receipt.window_seconds));
        if let Some(start) = current_period_start_at(receipt.reset_at, receipt.window_seconds) {
            receipt_origins
                .entry(receipt.reset_at)
                .or_default()
                .insert(start);
        }
        receipt_ends
            .entry(receipt.reset_at)
            .and_modify(|end| *end = (*end).max(receipt.observed_at))
            .or_insert(receipt.observed_at);
        minutes.insert(
            (
                receipt.reset_at,
                receipt.observed_at - receipt.observed_at.rem_euclid(60),
            ),
            receipt,
        );
    }
    if let Some(window) = publication {
        minutes.retain(|(_, timestamp), _| *timestamp > cutoff && *timestamp <= window.at);
    }
    let mut history = std::mem::take(history_v3)
        .into_iter()
        .map(|point| ((point.reset_at, point.timestamp), point))
        .collect::<BTreeMap<_, _>>();
    for ((_, timestamp), receipt) in minutes {
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
    let mut model_ends = BTreeMap::<i64, i64>::new();
    let mut model_starts = BTreeMap::<i64, i64>::new();
    for point in history.values() {
        model_starts
            .entry(point.reset_at)
            .and_modify(|start| *start = (*start).min(point.timestamp))
            .or_insert(point.timestamp);
        model_ends
            .entry(point.reset_at)
            .and_modify(|end| *end = (*end).max(point.timestamp))
            .or_insert(point.timestamp);
    }
    for (_, reset, start) in &acquisition_periods {
        let known_origin = *reset == current.reset_at
            || receipt_origins
                .get(reset)
                .is_some_and(|starts| starts.len() == 1);
        let public_start = if known_origin {
            *start
        } else {
            model_starts.get(reset).copied().unwrap_or(*start)
        };
        let end = model_ends
            .get(reset)
            .into_iter()
            .chain(receipt_ends.get(reset))
            .copied()
            .max()
            .unwrap_or(*start);
        let period = periods
            .entry(*reset)
            .or_insert_with(|| PublicHistoryPeriod {
                id: reset.to_string(),
                reset_at: *reset,
                start_at: public_start,
                end_at: end,
                label: String::new(),
                current: false,
            });
        if known_origin {
            period.start_at = *start;
        } else {
            period.start_at = period.start_at.min(public_start);
        }
        period.end_at = period.end_at.max(end).min(*reset).min(history_upper);
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
    let conflicting = receipt_origins
        .iter()
        .filter_map(|(&reset, starts)| {
            (reset != current.reset_at && starts.len() > 1).then_some(reset)
        })
        .collect::<BTreeSet<_>>();
    let mut origins = periods_by_receipt
        .iter()
        .filter(|(reset, _)| !conflicting.contains(reset))
        .filter_map(|(reset, (_, window))| {
            current_period_start_at(*reset, *window).map(|start| (*reset, start))
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(start) = current_period_start_at(current.reset_at, current.window_seconds) {
        origins.insert(current.reset_at, start);
    }
    let published_starts = details
        .history_periods
        .iter()
        .map(|period| (period.reset_at, period.start_at))
        .collect::<BTreeMap<_, _>>();
    origins.retain(|reset, start| {
        published_starts
            .get(reset)
            .is_some_and(|published| *start <= *published)
    });
    Ok(OriginProjection {
        known: origins,
        conflicting,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue604_recovered_window_end_is_exclusive() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session_events(root_identity TEXT,relative_path TEXT,
                range_start INTEGER,range_end INTEGER,record_sha256 TEXT,timestamp INTEGER);
             CREATE TABLE session_quota_windows(reset_at INTEGER,window_seconds INTEGER,
                observed_at INTEGER,root_identity TEXT,relative_path TEXT,
                range_start INTEGER,range_end INTEGER,record_sha256 TEXT);",
            )
            .unwrap();
        for (observed_at, expected) in [(1_800_000_599, Some(1_800_000_000)), (1_800_000_600, None)]
        {
            connection
                .execute_batch("DELETE FROM session_events; DELETE FROM session_quota_windows;")
                .unwrap();
            connection
                .execute(
                    "INSERT INTO session_events VALUES('root','source.jsonl',0,100,'proof',?1)",
                    [observed_at],
                )
                .unwrap();
            connection.execute("INSERT INTO session_quota_windows VALUES(1800000600,600,?1,'root','source.jsonl',0,100,'proof')", [observed_at]).unwrap();
            let mut details = PublicDetails {
                history_periods: vec![PublicHistoryPeriod {
                    id: "1800000600".to_owned(),
                    reset_at: 1_800_000_600,
                    start_at: 1_800_000_060,
                    end_at: 1_800_000_600,
                    label: "historical".to_owned(),
                    current: false,
                }],
                ..PublicDetails::default()
            };
            let mut origins = BTreeMap::new();
            project_recovered_windows(
                &connection,
                &ReadIntervals::unbounded(),
                &mut details,
                &mut origins,
                &BTreeSet::new(),
                None,
            )
            .unwrap();
            assert_eq!(
                origins.get(&1_800_000_600).copied(),
                expected,
                "only an observation strictly before reset can prove that window"
            );
        }
    }

    #[test]
    fn issue604_same_minute_quota_receipts_are_ordered_within_each_exact_period() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE quota_receipts(sequence INTEGER,observed_at INTEGER,
                reset_at INTEGER,window_seconds INTEGER,remaining_percent REAL);
             INSERT INTO quota_receipts VALUES
                (1,1800000001,1800604800,604800,80),
                (2,1800000002,1800604801,604800,90),
                (3,1800000003,1800604800,604800,70);",
            )
            .unwrap();
        for history_window in [HistoryWindow::Recent, HistoryWindow::Retained] {
            let mut details = PublicDetails {
                observed_at: Some(1_800_000_060),
                quota: Some(PublicQuota {
                    remaining_percent: 70.0,
                    reset_at: 1_800_604_800,
                    window_seconds: 604_800,
                    monthly: false,
                }),
                ..PublicDetails::default()
            };
            let mut v2 = Vec::new();
            let mut v3 = Vec::new();
            project(
                &connection,
                &ReadIntervals::unbounded(),
                &mut details,
                &mut v2,
                &mut v3,
                history_window,
                None,
            )
            .unwrap();
            assert_eq!(
                v3.iter()
                    .map(|point| (point.reset_at, point.timestamp, point.remaining_percent))
                    .collect::<Vec<_>>(),
                vec![
                    (1_800_604_800, 1_800_000_000, Some(70.0)),
                    (1_800_604_801, 1_800_000_000, Some(90.0)),
                ]
            );
            assert!(v3.iter().all(|point| point.models.is_none()));
            assert_eq!(v2.len(), 2);
            assert!(details.history_samples.is_empty());
        }
    }
}

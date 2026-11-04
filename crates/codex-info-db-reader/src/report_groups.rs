//! Read-only projection of overlapping reports; stored keys and prices stay intact.
use super::*;
use codex_info_pricing::{
    estimate_at, estimate_legacy, estimate_with_revision, Usage as PricingUsage,
};

#[derive(Clone)]
struct Fact {
    id: String,
    at: i64,
    model: String,
    raw_model: String,
    tokens: TokenFacts,
    reset: i64,
    window: i64,
}

#[derive(Clone)]
struct Saved {
    reset: i64,
    at: i64,
    model: String,
    tokens: TokenFacts,
    dollars: Option<f64>,
    revision: Option<String>,
}

type SavedTokenKey = (u64, Option<u64>, Option<u64>, Option<u64>, Option<u64>);
type SavedExactIndex<'a> =
    BTreeMap<(String, SavedTokenKey), BTreeMap<Option<String>, Vec<&'a Saved>>>;

struct SavedIndex<'a> {
    exact: SavedExactIndex<'a>,
    context: BTreeMap<String, Vec<&'a Saved>>,
}

#[derive(Clone, Copy)]
struct Receipt {
    sequence: i64,
    at: i64,
    reset: i64,
    window: i64,
    minute: i64,
    remaining: f64,
}

fn invalid() -> ReaderError {
    ReaderError::InvalidValue("invalid report-group numeric fact".into())
}

fn load_facts(db: &Connection) -> Result<Vec<Fact>, ReaderError> {
    let mut query = db.prepare(
        "SELECT f.fact_id,f.timestamp,f.model,f.tokens_json,e.model,e.reset_at,e.window_seconds
         FROM recorded_usage_facts f JOIN session_events e ON f.source_key=json_array(
         e.root_identity,e.relative_path,e.file_device,e.file_inode,e.prefix_generation,
         CAST(e.range_start AS TEXT),CAST(e.range_end AS TEXT),e.record_sha256,CAST(e.event_index AS TEXT))
         WHERE f.eligible=1 AND e.reset_at IS NOT NULL AND e.window_seconds IS NOT NULL",
    )?;
    let mut result = Vec::new();
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let json: String = row.get(3)?;
        let tokens: TokenFacts = serde_json::from_str(&json).map_err(|_| invalid())?;
        tokens.validate_fact().map_err(|_| invalid())?;
        result.push(Fact {
            id: row.get(0)?,
            at: row.get(1)?,
            model: row.get(2)?,
            tokens,
            raw_model: row.get(4)?,
            reset: row.get(5)?,
            window: row.get(6)?,
        });
    }
    Ok(result)
}

fn load_saved(db: &Connection) -> Result<Vec<Saved>, ReaderError> {
    let has_total_dollars = table_has_column(db, "usage_model_history", "total_dollars")?;
    let has_price_version = table_has_column(db, "usage_model_history", "price_version")?;
    let total_dollars_column = if has_total_dollars {
        "total_dollars"
    } else {
        "NULL"
    };
    let price_version_column = if has_price_version {
        "price_version"
    } else {
        "NULL"
    };
    let query_sql = format!(
        "SELECT reset_at,timestamp,model,total_tokens,input_tokens,cached_input_tokens,cache_write_input_tokens,output_tokens,{total_dollars_column} AS total_dollars,{price_version_column} AS price_version FROM usage_model_history ORDER BY timestamp,reset_at,model"
    );
    let mut query = db.prepare(&query_sql)?;
    let mut rows = query.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let number = |index| -> Option<u64> {
            sql_text(row, index).and_then(|v| canonical_timeline_u64(&v))
        };
        let Some(total) = number(3) else { continue };
        let tokens = TokenFacts {
            total,
            input: number(4),
            cached_input: number(5),
            cache_write_input: number(6),
            output: number(7),
        };
        if tokens.validate_fact().is_err() {
            continue;
        }
        result.push(Saved {
            reset: row.get(0)?,
            at: row.get(1)?,
            model: row.get(2)?,
            tokens,
            dollars: row.get(8)?,
            revision: row.get(9)?,
        });
    }
    Ok(result)
}

fn saved_token_key(tokens: TokenFacts) -> SavedTokenKey {
    (
        tokens.total,
        tokens.input,
        tokens.cached_input,
        tokens.cache_write_input,
        tokens.output,
    )
}

fn latest_saved<'a>(rows: &[&'a Saved], at: i64) -> Option<&'a Saved> {
    let mut lower = 0usize;
    let mut upper = rows.len();
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        if rows[middle].at <= at {
            lower = middle + 1;
        } else {
            upper = middle;
        }
    }
    lower
        .checked_sub(1)
        .and_then(|index| rows.get(index).copied())
}

fn build_saved_index<'a>(rows: &[&'a Saved]) -> SavedIndex<'a> {
    let mut index = SavedIndex {
        exact: BTreeMap::new(),
        context: BTreeMap::new(),
    };
    for row in rows {
        index
            .exact
            .entry((row.model.clone(), saved_token_key(row.tokens)))
            .or_default()
            .entry(row.revision.clone())
            .or_default()
            .push(*row);
        index
            .context
            .entry(row.model.clone())
            .or_default()
            .push(*row);
    }
    for by_revision in index.exact.values_mut() {
        for values in by_revision.values_mut() {
            values.sort_by_key(|row| (row.at, row.reset, row.model.clone()));
        }
    }
    for values in index.context.values_mut() {
        values.sort_by_key(|row| (row.at, row.reset, row.model.clone()));
    }
    index
}

fn load_receipts(db: &Connection, intervals: &ReadIntervals) -> Result<Vec<Receipt>, ReaderError> {
    if !table_exists(db, "quota_receipts")? {
        return Ok(Vec::new());
    }
    let mut query = db.prepare(
        "SELECT sequence,observed_at,reset_at,window_seconds,remaining_percent
         FROM quota_receipts ORDER BY sequence",
    )?;
    let mut rows = query.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let (sequence, at, reset, window, remaining): (i64, i64, i64, i64, f64) = (
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        );
        let Some(origin) = current_period_start_at(reset, window) else {
            continue;
        };
        if !remaining.is_finite()
            || !(0.0..=100.0).contains(&remaining)
            || at < origin
            || at >= reset
            || !intervals.contains(at)
        {
            continue;
        }
        result.push(Receipt {
            sequence,
            at,
            reset,
            window,
            minute: at - at.rem_euclid(60),
            remaining,
        });
    }
    Ok(result)
}

// A name correspondence comes from the complete source PK, never a family guess.
fn names_from_index(
    model: &str,
    forward: &BTreeMap<String, BTreeSet<String>>,
    reverse: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut names = BTreeSet::from([model.to_owned()]);
    if let Some(raw) = reverse.get(model).filter(|values| values.len() == 1) {
        let name = raw.first().expect("one raw name");
        if forward.get(name).is_some_and(|values| values.len() == 1) {
            names.insert(name.clone());
        }
    }
    names
}

fn estimate_saved_revision(
    model: &str,
    usage: PricingUsage,
    revision: &str,
) -> Option<codex_info_pricing::Cost> {
    estimate_with_revision(model, usage, revision)
        .or_else(|| estimate_legacy(model, usage).filter(|cost| cost.price_version == revision))
}

fn dollars(
    model: &str,
    tokens: TokenFacts,
    exact_rows: &[&Saved],
    context_rows: &[&Saved],
    at: i64,
) -> Option<f64> {
    let usage = PricingUsage {
        input_tokens: tokens.input?,
        cached_input_tokens: tokens.cached_input?,
        cache_write_input_tokens: tokens.cache_write_input,
        output_tokens: tokens.output?,
    };
    if let Some(row) = exact_rows
        .iter()
        .filter(|row| row.dollars.is_some())
        .max_by_key(|row| row.at)
    {
        let revision = row.revision.as_deref()?;
        let expected = estimate_saved_revision(model, usage, revision)?.total_dollars;
        return Some(
            row.dollars
                .filter(|saved| saved.to_bits() == expected.to_bits())
                .unwrap_or(expected),
        );
    }
    let latest = context_rows.iter().max_by_key(|row| row.at);
    if let Some(revision) = latest.and_then(|row| row.revision.as_deref()) {
        estimate_saved_revision(model, usage, revision).map(|cost| cost.total_dollars)
    } else {
        estimate_at(model, usage, at).map(|cost| cost.total_dollars)
    }
}

fn minute(timestamp: i64) -> i64 {
    timestamp - timestamp.rem_euclid(60)
}

fn group_id(origin: i64, window: i64) -> String {
    format!("rg1-{}-{}", minute(origin), window)
}

fn empty_point(reset_at: i64, timestamp: i64) -> PublicHistoryObservationV3 {
    PublicHistoryObservationV3 {
        timestamp,
        reset_at,
        remaining_percent: None,
        task_active_since_previous: None,
        models: None,
        models_complete: false,
        model_source: "unavailable".into(),
    }
}

pub(super) fn project(
    db: &Connection,
    archive: &mut HistoryArchive,
    evidence: &TaskActivityEvidence,
    intervals: &ReadIntervals,
) -> Result<(), ReaderError> {
    let source_periods = archive.periods.clone();
    let source_samples = archive.samples.clone();
    let source_gaps = archive.gaps.clone();
    let source_current_reset_at = source_periods
        .iter()
        .find_map(|period| period.current.then_some(period.reset_at));
    let mut grouped = BTreeMap::<(i64, i64), Vec<PublicHistoryPeriod>>::new();
    for period in &source_periods {
        let Some(&origin) = archive.origins.get(&period.reset_at) else {
            continue;
        };
        let Some(window) = period
            .reset_at
            .checked_sub(origin)
            .filter(|value| *value > 0)
        else {
            continue;
        };
        grouped
            .entry((minute(origin), window))
            .or_default()
            .push(period.clone());
    }
    if grouped.is_empty() {
        return Ok(());
    }

    let facts = if table_exists(db, "recorded_usage_facts")? && table_exists(db, "session_events")?
    {
        load_facts(db)?
    } else {
        Vec::new()
    };
    let saved = if table_exists(db, "usage_model_history")? {
        load_saved(db)?
    } else {
        Vec::new()
    };
    let receipts = load_receipts(db, intervals)?;
    let mut grouped_resets = BTreeSet::new();
    let mut canonical_periods = Vec::new();
    let mut canonical_samples = Vec::new();
    let mut canonical_gaps = Vec::new();
    let mut canonical_origins = archive.origins.clone();
    let mut aliases = archive.aliases.clone();

    for ((origin_minute, window), members) in grouped {
        let member_resets = members
            .iter()
            .map(|period| period.reset_at)
            .collect::<BTreeSet<_>>();
        let Some(origin) = member_resets
            .iter()
            .filter_map(|reset| archive.origins.get(reset).copied())
            .min()
        else {
            continue;
        };
        let Some(&transport_reset) = member_resets.iter().max() else {
            continue;
        };
        let Some(start_at) = members.iter().map(|period| period.start_at).min() else {
            continue;
        };
        let Some(mut end_at) = members.iter().map(|period| period.end_at).max() else {
            continue;
        };
        let mut seen_ids = BTreeSet::<&str>::new();
        let mut selected = facts
            .iter()
            .filter(|fact| {
                if !member_resets.contains(&fact.reset)
                    || fact.window != window
                    || !intervals.contains(fact.at)
                    || fact.at >= fact.reset
                {
                    return false;
                }
                let member_origin = archive.origins.get(&fact.reset).copied().unwrap_or(origin);
                fact.at >= member_origin && seen_ids.insert(fact.id.as_str())
            })
            .collect::<Vec<_>>();
        selected.sort_by_key(|fact| (fact.at, fact.id.clone()));
        if let Some(fact_end) = selected.iter().map(|fact| fact.at).max() {
            end_at = end_at.max(fact_end);
        }
        if let Some(receipt_end) = receipts
            .iter()
            .filter(|receipt| member_resets.contains(&receipt.reset) && receipt.window == window)
            .map(|receipt| receipt.at)
            .max()
        {
            end_at = end_at.max(receipt_end);
        }
        if let Some(gap_end) = source_gaps
            .iter()
            .filter(|gap| member_resets.contains(&gap.reset_at))
            .map(|gap| gap.end_at)
            .max()
        {
            end_at = end_at.max(gap_end);
        }
        end_at = end_at.min(transport_reset);
        if start_at > end_at {
            continue;
        }

        grouped_resets.extend(member_resets.iter().copied());
        for member in member_resets.iter().copied() {
            canonical_origins.remove(&member);
            if member != transport_reset {
                aliases.insert(member, transport_reset);
            }
        }
        canonical_origins.insert(transport_reset, origin);

        let canonical_period = PublicHistoryPeriod {
            id: group_id(origin_minute, window),
            start_at,
            end_at,
            reset_at: transport_reset,
            label: String::new(),
            current: members.iter().any(|period| period.current),
        };

        let mut points = BTreeMap::<(i64, i64), PublicHistoryObservationV3>::new();
        for sample in source_samples
            .iter()
            .filter(|sample| member_resets.contains(&sample.reset_at))
        {
            if codex_info_rest_contract::history_minute_in_period(
                sample.timestamp,
                start_at,
                end_at,
            ) {
                let mut point = sample.clone();
                point.remaining_percent = None;
                point.reset_at = transport_reset;
                point.models = None;
                point.models_complete = false;
                point.model_source = "unavailable".into();
                points
                    .entry((transport_reset, point.timestamp))
                    .or_insert(point);
            }
        }
        for fact in &selected {
            let timestamp = minute(fact.at);
            if codex_info_rest_contract::history_minute_in_period(timestamp, start_at, end_at) {
                points
                    .entry((transport_reset, timestamp))
                    .or_insert_with(|| empty_point(transport_reset, timestamp));
            }
        }
        let mut receipt_points = BTreeMap::<i64, Receipt>::new();
        for receipt in receipts
            .iter()
            .filter(|receipt| member_resets.contains(&receipt.reset) && receipt.window == window)
        {
            if !codex_info_rest_contract::history_minute_in_period(receipt.minute, start_at, end_at)
            {
                continue;
            }
            receipt_points
                .entry(receipt.minute)
                .and_modify(|current| {
                    if receipt.sequence > current.sequence {
                        *current = *receipt;
                    }
                })
                .or_insert(*receipt);
            points
                .entry((transport_reset, receipt.minute))
                .or_insert_with(|| empty_point(transport_reset, receipt.minute));
        }
        for (timestamp, receipt) in &receipt_points {
            if let Some(point) = points.get_mut(&(transport_reset, *timestamp)) {
                point.remaining_percent = Some(receipt.remaining);
            }
        }

        assign_task_activity_for_intervals(
            &mut points,
            std::slice::from_ref(&canonical_period),
            evidence,
            intervals,
        );
        let mut group_saved = saved
            .iter()
            .filter(|row| member_resets.contains(&row.reset))
            .collect::<Vec<_>>();
        group_saved.sort_by_key(|row| (row.at, row.reset, row.model.clone()));
        let saved_index = build_saved_index(&group_saved);
        let mut fact_cursor = 0usize;
        let mut prefix_totals = BTreeMap::<String, TokenFacts>::new();
        let mut forward_names = BTreeMap::<String, BTreeSet<String>>::new();
        let mut reverse_names = BTreeMap::<String, BTreeSet<String>>::new();
        for point in points.values_mut() {
            let cutoff = point.timestamp.saturating_add(60);
            while selected
                .get(fact_cursor)
                .is_some_and(|fact| fact.at < cutoff)
            {
                let fact = selected[fact_cursor];
                let total = prefix_totals
                    .get(&fact.model)
                    .copied()
                    .map(|previous| previous.checked_sum(fact.tokens))
                    .transpose()
                    .map_err(|_| invalid())?
                    .unwrap_or(fact.tokens);
                prefix_totals.insert(fact.model.clone(), total);
                forward_names
                    .entry(fact.raw_model.clone())
                    .or_default()
                    .insert(fact.model.clone());
                reverse_names
                    .entry(fact.model.clone())
                    .or_default()
                    .insert(fact.raw_model.clone());
                fact_cursor += 1;
            }
            let names = prefix_totals
                .keys()
                .map(|model| {
                    (
                        model.clone(),
                        names_from_index(model, &forward_names, &reverse_names),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let models = prefix_totals
                .clone()
                .into_iter()
                .map(|(model, tokens)| {
                    let exact_key = saved_token_key(tokens);
                    let exact_rows = names[&model]
                        .iter()
                        .flat_map(|name| {
                            saved_index
                                .exact
                                .get(&(name.clone(), exact_key))
                                .into_iter()
                                .flat_map(|by_revision| by_revision.values())
                                .filter_map(|rows| latest_saved(rows, point.timestamp))
                        })
                        .collect::<Vec<_>>();
                    let context_rows = names[&model]
                        .iter()
                        .filter_map(|name| {
                            saved_index
                                .context
                                .get(name)
                                .and_then(|rows| latest_saved(rows, point.timestamp))
                        })
                        .collect::<Vec<_>>();
                    let total_dollars =
                        dollars(&model, tokens, &exact_rows, &context_rows, point.timestamp);
                    PublicHistoryModelUsageV3 {
                        model,
                        total_tokens: tokens.total,
                        input_tokens: tokens.input,
                        cached_input_tokens: tokens.cached_input,
                        cache_write_input_tokens: tokens.cache_write_input,
                        output_tokens: tokens.output,
                        total_dollars,
                    }
                })
                .collect::<Vec<_>>();
            point.models = (!models.is_empty()).then_some(models);
            point.models_complete = false;
            point.model_source = if point.models.is_some() {
                "legacy-unknown".into()
            } else {
                "unavailable".into()
            };
        }
        canonical_periods.push(canonical_period);
        canonical_samples.extend(points.into_values());
        canonical_gaps.extend(
            source_gaps
                .iter()
                .filter(|gap| member_resets.contains(&gap.reset_at))
                .map(|gap| {
                    let mut canonical = gap.clone();
                    canonical.reset_at = transport_reset;
                    canonical
                }),
        );
    }

    archive.periods = source_periods
        .into_iter()
        .filter(|period| !grouped_resets.contains(&period.reset_at))
        .chain(canonical_periods)
        .collect();
    archive.samples = source_samples
        .into_iter()
        .filter(|sample| !grouped_resets.contains(&sample.reset_at))
        .chain(canonical_samples)
        .collect();
    archive.gaps = source_gaps
        .into_iter()
        .filter(|gap| !grouped_resets.contains(&gap.reset_at))
        .chain(canonical_gaps)
        .collect();
    archive.origins = canonical_origins;
    archive.aliases = aliases;
    archive
        .samples
        .sort_by_key(|sample| (sample.reset_at, sample.timestamp));
    archive.periods.sort_by(|left, right| {
        right
            .start_at
            .cmp(&left.start_at)
            .then_with(|| right.reset_at.cmp(&left.reset_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    assign_history_period_labels(&mut archive.periods, source_current_reset_at);
    archive
        .gaps
        .sort_by_key(|gap| (gap.reset_at, gap.start_at, gap.end_at));
    archive.gaps.dedup();
    Ok(())
}

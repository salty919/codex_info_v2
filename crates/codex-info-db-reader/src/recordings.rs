//! One read-transaction projection of local recorded facts and saved values.
//! No Session files, price recalculation, reset grouping, or write connection.
use super::{recorded_usage::*, table_exists, ReaderError};
use codex_info_rest_contract::token_facts::TokenFacts;
use rusqlite::{Connection, Row};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RecordedArchive {
    pub prefixes: BTreeMap<String, Vec<(i64, RecordedTokenPrefix)>>,
    pub quota_observations: Vec<QuotaObservation>,
    pub observations: Vec<StoredObservation>,
    pub coverage: RecordingCoverage,
    pub fact_revision: u64,
    /// A valid auxiliary archive may predate initialization of its local index.
    /// This is a read-transaction fact, not new persisted state or coverage.
    pub local_index_initialized: bool,
    /// Timed numeric candidates whose local consumption is not yet qualified.
    pub pending_numeric_times: BTreeSet<i64>,
}

fn invalid(message: &str) -> ReaderError {
    ReaderError::InvalidValue(message.into())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl RecordedArchive {
    /// Presence of local consumption, at the graph's existing minute granularity.
    /// Neither quota observations nor all-device recording coverage participate.
    pub fn local_usage(&self, start: i64, end: i64) -> Result<LocalUsage, ReaderError> {
        if !self.local_index_initialized {
            return Err(invalid("local usage index uninitialized"));
        }
        let mut minutes = BTreeMap::<i64, LocalUsageState>::new();
        for values in self.prefixes.values() {
            let first = values.partition_point(|(at, _)| *at < start);
            let last = values.partition_point(|(at, _)| *at < end);
            for index in first..last {
                let zero = RecordedTokenPrefix::default();
                let before = index.checked_sub(1).map_or(&zero, |i| &values[i].1);
                let state = match values[index].1.difference(before)? {
                    Some(tokens) if tokens.total > 0 => LocalUsageState::Used,
                    Some(_) => continue,
                    None => LocalUsageState::Unavailable,
                };
                let minute = values[index].0.div_euclid(60) * 60;
                let entry = minutes.entry(minute).or_insert(state);
                if state == LocalUsageState::Used {
                    *entry = state;
                }
            }
        }
        for &at in self.pending_numeric_times.range(start..end) {
            minutes
                .entry(at.div_euclid(60) * 60)
                .or_insert(LocalUsageState::Unavailable);
        }
        let mut intervals = Vec::<LocalUsageInterval>::new();
        let mut cursor = start;
        for (minute, state) in minutes {
            let left = minute.max(start);
            let right = (minute + 60).min(end);
            if cursor < left {
                push_local_interval(&mut intervals, cursor, left, LocalUsageState::Unused);
            }
            push_local_interval(&mut intervals, left, right, state);
            cursor = right;
        }
        if cursor < end {
            push_local_interval(&mut intervals, cursor, end, LocalUsageState::Unused);
        }
        let evidence = LocalUsage {
            version: 1,
            start_at: start,
            end_at: end,
            intervals,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    pub fn latest_fact_at(&self) -> Option<i64> {
        self.prefixes
            .values()
            .filter_map(|v| v.last().map(|v| v.0))
            .max()
    }
    pub fn models(&self, start: i64, end: i64) -> Result<Vec<RecordedModelUsage>, ReaderError> {
        let mut result = Vec::new();
        for (model, values) in &self.prefixes {
            let before = values.partition_point(|(t, _)| *t < start);
            let after = values.partition_point(|(t, _)| *t < end);
            if before == after {
                continue;
            }
            let zero = RecordedTokenPrefix::default();
            let lower = before.checked_sub(1).map_or(&zero, |i| &values[i].1);
            let upper = &values[after - 1].1;
            if let Some(tokens) = upper.difference(lower)? {
                result.push(RecordedModelUsage::from_tokens(model.clone(), tokens)?);
            }
        }
        Ok(result)
    }

    pub fn samples(&self, start: i64, end: i64) -> Result<Vec<RecordedSample>, ReaderError> {
        let timestamps = self
            .prefixes
            .values()
            .flat_map(|v| {
                v.iter()
                    .filter_map(|(at, _)| (*at >= start && *at < end).then_some(*at))
            })
            .collect::<BTreeSet<_>>();
        timestamps
            .into_iter()
            .map(|timestamp| {
                Ok(RecordedSample {
                    timestamp,
                    models: self.models(start, timestamp + 1)?,
                })
            })
            .collect()
    }
}

fn push_local_interval(
    intervals: &mut Vec<LocalUsageInterval>,
    start_at: i64,
    end_at: i64,
    state: LocalUsageState,
) {
    if start_at >= end_at {
        return;
    }
    if let Some(previous) = intervals
        .last_mut()
        .filter(|v| v.end_at == start_at && v.state == state)
    {
        previous.end_at = end_at;
    } else {
        intervals.push(LocalUsageInterval {
            start_at,
            end_at,
            state,
        });
    }
}

/// The history lane needs only local numeric evidence, not the auxiliary
/// archive's full copy of quota and saved model observations.
pub(super) fn read_local(
    connection: &Connection,
    includes: impl Fn(i64) -> bool,
) -> Result<RecordedArchive, ReaderError> {
    let mut archive = read_prefixes(connection, true)?;
    read_pending_numeric_times(connection, &mut archive, includes)?;
    Ok(archive)
}

fn read_pending_numeric_times(
    connection: &Connection,
    archive: &mut RecordedArchive,
    includes: impl Fn(i64) -> bool,
) -> Result<(), ReaderError> {
    if table_exists(connection, "session_events")?
        && table_exists(connection, "session_pending_ranges")?
    {
        let mut query = connection.prepare(
            "SELECT DISTINCT e.timestamp FROM session_pending_ranges p JOIN session_events e
               ON p.root_identity=e.root_identity AND p.relative_path=e.relative_path
                 AND p.file_device=e.file_device AND p.file_inode=e.file_inode
                 AND p.prefix_generation=e.prefix_generation
                 AND p.start_offset<=e.range_start AND p.end_offset>=e.range_end
             WHERE e.admission_json IS NULL AND e.total_tokens <> '0'",
        )?;
        for value in query.query_map([], |row| row.get::<_, i64>(0))? {
            let at = value?;
            if !(1..MAX_END_EXCLUSIVE).contains(&at) {
                return Err(invalid("invalid pending usage timestamp"));
            }
            if includes(at) {
                archive.pending_numeric_times.insert(at);
            }
        }
    }
    if table_exists(connection, "session_checkpoints")?
        && table_exists(connection, "session_pending_ranges")?
    {
        let mut query = connection.prepare(
            "SELECT c.root_identity,c.relative_path,c.file_device,c.file_inode,c.prefix_generation,c.source_admission
             FROM session_checkpoints c WHERE c.source_admission IS NOT NULL AND EXISTS (
               SELECT 1 FROM session_pending_ranges p WHERE p.root_identity=c.root_identity
                 AND p.relative_path=c.relative_path AND p.file_device=c.file_device
                 AND p.file_inode=c.file_inode AND p.prefix_generation=c.prefix_generation)",
        )?;
        let mut ranges = connection.prepare(
            "SELECT start_offset,end_offset FROM session_pending_ranges
             WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4
               AND prefix_generation=?5 ORDER BY start_offset,end_offset",
        )?;
        for row in query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })? {
            let (root, path, device, inode, generation, payload) = row?;
            let spans = ranges
                .query_map(
                    rusqlite::params![root, path, device, inode, generation],
                    |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let mut merged = Vec::<(u64, u64)>::new();
            for (start, end) in spans {
                if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            // Responses also retain already-qualified proof. Only physical
            // ranges still marked pending can contribute uncertainty here.
            let document: serde_json::Value = serde_json::from_str(&payload)
                .map_err(|_| invalid("invalid pending response metadata"))?;
            let Some(responses) = document.get("responses").and_then(|v| v.as_array()) else {
                continue;
            };
            for response in responses {
                let positive = response
                    .pointer("/record/usage/total")
                    .and_then(|v| v.as_u64())
                    .is_some_and(|v| v > 0);
                if !positive {
                    continue;
                }
                let at = response
                    .pointer("/pointer/timestamp")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| invalid("invalid pending response timestamp"))?;
                let start = response
                    .pointer("/pointer/start")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| invalid("invalid pending response range"))?;
                let end = response
                    .pointer("/pointer/end")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| invalid("invalid pending response range"))?;
                if !(1..MAX_END_EXCLUSIVE).contains(&at) || start >= end {
                    return Err(invalid("invalid pending response metadata"));
                }
                let index = merged.partition_point(|span| span.0 <= start);
                let still_pending = index
                    .checked_sub(1)
                    .is_some_and(|index| merged[index].1 >= end);
                if still_pending && includes(at) {
                    archive.pending_numeric_times.insert(at);
                }
            }
        }
    }
    Ok(())
}

pub(super) fn read_all(
    connection: &Connection,
    includes: impl Fn(i64) -> bool,
) -> Result<RecordedArchive, ReaderError> {
    let mut archive = read(connection)?;
    if archive.local_index_initialized {
        read_pending_numeric_times(connection, &mut archive, includes)?;
    }
    Ok(archive)
}

pub(super) fn read(connection: &Connection) -> Result<RecordedArchive, ReaderError> {
    read_with_diagnostics(connection, |reason| {
        eprintln!(
            "{}",
            serde_json::json!({
                "schema": "codex-info-recorded-projection-diagnostic-v1",
                "stage": "recorded_projection",
                "reason": reason,
            })
        );
    })
}

fn read_with_diagnostics(
    connection: &Connection,
    mut sink: impl FnMut(&'static str),
) -> Result<RecordedArchive, ReaderError> {
    let result = read_inner(connection);
    if let Err(error) = &result {
        sink(recorded_projection_failure_reason(error));
    }
    result
}

fn recorded_projection_failure_reason(error: &ReaderError) -> &'static str {
    match error {
        ReaderError::Io(_) => "database_io_failure",
        ReaderError::Sqlite(_) => "database_sqlite_failure",
        ReaderError::Schema(_) => "database_schema_failure",
        ReaderError::Contract(_) => "recorded_contract_failure",
        ReaderError::TooManyRows(_) => "recorded_row_limit",
        ReaderError::InvalidValue(message) => match message.as_str() {
            "partial recorded usage schema" => "partial_recorded_usage_schema",
            "invalid recorded usage row storage" => "invalid_recorded_usage_row_storage",
            "invalid recorded revision" => "invalid_recorded_revision",
            "uncommitted recorded projection" => "uncommitted_recorded_projection",
            "invalid recording coverage" => "invalid_recording_coverage",
            "invalid recorded model head" => "invalid_recorded_model_head",
            "unordered recorded model head" => "unordered_recorded_model_head",
            "invalid recorded prefix evidence" => "invalid_recorded_prefix_evidence",
            "invalid recorded prefix payload" => "invalid_recorded_prefix_payload",
            "recorded prefix/head mismatch" => "recorded_prefix_head_mismatch",
            "invalid stored official quota observation" => {
                "invalid_stored_official_quota_observation"
            }
            "saved observation timestamp is invalid" => "saved_observation_timestamp_is_invalid",
            "invalid recovery pointer" => "invalid_recovery_pointer",
            "invalid recovery pointer fields" => "invalid_recovery_pointer_fields",
            "invalid recovery pointer encoding" => "invalid_recovery_pointer_encoding",
            "invalid recovered fact tokens" => "invalid_recovered_fact_tokens",
            "invalid recovered numeric fact" => "invalid_recovered_numeric_fact",
            "recovery relation digest mismatch" => "recovery_relation_digest_mismatch",
            "invalid recovery relation" => "invalid_recovery_relation",
            "invalid recovery relation mode" => "invalid_recovery_relation_mode",
            "missing recovery prefixes" => "missing_recovery_prefixes",
            "invalid recovery parser version" => "invalid_recovery_parser_version",
            "missing recovery original inventory" => "missing_recovery_original_inventory",
            "invalid recovery original source" => "invalid_recovery_original_source",
            "invalid recovery original number" => "invalid_recovery_original_number",
            "invalid recovery original encoding" => "invalid_recovery_original_encoding",
            "missing recovery mappings" => "missing_recovery_mappings",
            "recovery mapping is outside original inventory" => {
                "recovery_mapping_is_outside_original_inventory"
            }
            "invalid recovery canonical relation" => "invalid_recovery_canonical_relation",
            "invalid recovery mapping tokens" => "invalid_recovery_mapping_tokens",
            "recovery relation differs from admitted fact" => {
                "recovery_relation_differs_from_admitted_fact"
            }
            _ => "recorded_value_invalid",
        },
    }
}

fn read_prefixes(
    connection: &Connection,
    require_initialized: bool,
) -> Result<RecordedArchive, ReaderError> {
    let names = [
        "recorded_usage_facts",
        "recorded_usage_prefixes",
        "recorded_usage_state",
    ];
    let present = names
        .iter()
        .map(|name| table_exists(connection, name))
        .collect::<Result<Vec<_>, _>>()?;
    if present.iter().any(|v| *v) && present.iter().any(|v| !*v) {
        return Err(invalid("partial recorded usage schema"));
    }
    if require_initialized && !present[0] {
        return Err(invalid("local usage index unavailable"));
    }
    let mut archive = RecordedArchive::default();
    if present[0] {
        for table in names {
            let wr: bool = connection.query_row(
                "SELECT wr FROM pragma_table_list WHERE name=?1",
                [table],
                |r| r.get(0),
            )?;
            if !wr {
                return Err(invalid("invalid recorded usage row storage"));
            }
        }
        let (version,initialized,revision,models,coverage,dirty):(i64,bool,String,String,String,String)=connection.query_row("SELECT format_version,initialized,fact_revision,models_json,coverage,dirty_json FROM recorded_usage_state WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        if require_initialized && !initialized {
            return Err(invalid("local usage index uninitialized"));
        }
        archive.local_index_initialized = initialized;
        archive.fact_revision = revision
            .parse::<u64>()
            .map_err(|_| invalid("invalid recorded revision"))?;
        if version != 1 || archive.fact_revision.to_string() != revision || dirty != "{}" {
            return Err(invalid("uncommitted recorded projection"));
        }
        archive.coverage = match coverage.as_str() {
            "complete" => RecordingCoverage::Complete,
            "partial" => RecordingCoverage::Partial,
            "unknown" => RecordingCoverage::Unknown,
            _ => return Err(invalid("invalid recording coverage")),
        };
        let expected: Vec<String> =
            serde_json::from_str(&models).map_err(|_| invalid("invalid recorded model head"))?;
        if expected.windows(2).any(|v| v[0] >= v[1]) {
            return Err(invalid("unordered recorded model head"));
        }
        let mut q=connection.prepare("SELECT model,timestamp,prefix_json,prefix_sha256 FROM recorded_usage_prefixes ORDER BY model,timestamp")?;
        let mut rows = q.query([])?;
        while let Some(row) = rows.next()? {
            let model: String = row.get(0)?;
            let at: i64 = row.get(1)?;
            let payload: String = row.get(2)?;
            let sha: String = row.get(3)?;
            if !(1..MAX_END_EXCLUSIVE).contains(&at) || digest(payload.as_bytes()) != sha {
                return Err(invalid("invalid recorded prefix evidence"));
            }
            let prefix: RecordedTokenPrefix = serde_json::from_str(&payload)
                .map_err(|_| invalid("invalid recorded prefix payload"))?;
            prefix.validate()?;
            let values = archive.prefixes.entry(model.clone()).or_default();
            if let Some((_, previous)) = values.last() {
                prefix.difference(previous)?;
            }
            RecordedModelUsage::from_tokens(
                model,
                TokenFacts {
                    total: 0,
                    input: Some(0),
                    cached_input: Some(0),
                    cache_write_input: Some(0),
                    output: Some(0),
                },
            )?;
            values.push((at, prefix));
        }
        if archive.prefixes.keys().cloned().collect::<Vec<_>>() != expected
            || (!initialized && !archive.prefixes.is_empty())
        {
            return Err(invalid("recorded prefix/head mismatch"));
        }
    }
    Ok(archive)
}

fn read_inner(connection: &Connection) -> Result<RecordedArchive, ReaderError> {
    let mut archive = read_prefixes(connection, false)?;
    if table_exists(connection, "quota_receipts")? {
        let mut q=connection.prepare("SELECT sequence,observed_at,reset_at,window_seconds,remaining_percent FROM quota_receipts ORDER BY sequence")?;
        let values = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, f64>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (sequence, observed_at, reset_at, window_seconds, remaining_percent) in values {
            if sequence < 1
                || !(1..MAX_END_EXCLUSIVE).contains(&observed_at)
                || !(1..MAX_END_EXCLUSIVE).contains(&reset_at)
                || window_seconds <= 0
                || !remaining_percent.is_finite()
                || !(0.0..=100.0).contains(&remaining_percent)
            {
                return Err(invalid("invalid stored official quota observation"));
            }
            archive.quota_observations.push(QuotaObservation {
                sequence: sequence.to_string(),
                observed_at,
                reset_at,
                window_seconds,
                remaining_percent,
            });
        }
    }
    read_observations(
        connection,
        &mut archive,
        table_exists(connection, "recorded_usage_facts")?,
    )?;
    archive.observations.sort_by(|a, b| {
        (
            a.timestamp,
            a.raw_reset_at,
            &a.storage_kind,
            &a.observation_id,
        )
            .cmp(&(
                b.timestamp,
                b.raw_reset_at,
                &b.storage_kind,
                &b.observation_id,
            ))
    });
    Ok(archive)
}

fn unsigned(
    row: &Row<'_>,
    index: usize,
    invalid_value: &mut bool,
) -> rusqlite::Result<Option<u64>> {
    use rusqlite::types::ValueRef;
    Ok(match row.get_ref(index)? {
        ValueRef::Null => None,
        ValueRef::Integer(v) if v >= 0 => Some(v as u64),
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes)
            .ok()
            .and_then(|v| v.parse::<u64>().ok().filter(|n| n.to_string() == v))
        {
            Some(v) => Some(v),
            None => {
                *invalid_value = true;
                None
            }
        },
        _ => {
            *invalid_value = true;
            None
        }
    })
}
fn dollars(row: &Row<'_>, index: usize, invalid_value: &mut bool) -> rusqlite::Result<Option<f64>> {
    use rusqlite::types::ValueRef;
    let value = match row.get_ref(index)? {
        ValueRef::Null => return Ok(None),
        ValueRef::Real(value) => value,
        ValueRef::Integer(value) => value as f64,
        _ => {
            *invalid_value = true;
            return Ok(None);
        }
    };
    Ok(if value.is_finite() && value >= 0.0 {
        Some(value)
    } else {
        *invalid_value = true;
        None
    })
}
fn reasons(invalid_value: bool, extra: Option<&str>) -> Vec<String> {
    let mut values = vec!["period-identity-unknown".to_owned()];
    if invalid_value {
        values.push("invalid-saved-value".into());
    }
    if let Some(extra) = extra {
        values.push(extra.into());
    }
    values.sort();
    values
}
fn append(
    archive: &mut RecordedArchive,
    at: i64,
    reset: Option<i64>,
    kind: &str,
    id: String,
    models: Vec<StoredModelObservation>,
    reasons: Vec<String>,
) -> Result<(), ReaderError> {
    if !(1..MAX_END_EXCLUSIVE).contains(&at)
        || reset.is_some_and(|v| !(1..MAX_END_EXCLUSIVE).contains(&v))
    {
        return Err(invalid("saved observation timestamp is invalid"));
    }
    archive.observations.push(StoredObservation {
        observation_id: id,
        timestamp: at,
        raw_reset_at: reset,
        storage_kind: kind.into(),
        models,
        unresolved_reasons: reasons,
    });
    Ok(())
}
fn read_observations(
    connection: &Connection,
    archive: &mut RecordedArchive,
    index_present: bool,
) -> Result<(), ReaderError> {
    let recovered = recovered_source_observations(connection, index_present)?;
    let mut q=connection.prepare("SELECT timestamp,reset_at,sol_tokens,terra_tokens,luna_tokens,sol_dollars,terra_dollars,luna_dollars FROM usage_history ORDER BY timestamp,reset_at")?;
    let mut rows = q.query([])?;
    while let Some(row) = rows.next()? {
        let at: i64 = row.get(0)?;
        let reset: i64 = row.get(1)?;
        let mut invalid_value = false;
        let mut models = Vec::new();
        for (index, model) in ["SOL", "TERRA", "LUNA"].iter().enumerate() {
            models.push(StoredModelObservation {
                stored_model_key: (*model).into(),
                total_tokens: unsigned(row, 2 + index, &mut invalid_value)?,
                input_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                output_tokens: None,
                total_dollars: dollars(row, 5 + index, &mut invalid_value)?,
            });
        }
        append(
            archive,
            at,
            Some(reset),
            "legacy-fixed",
            format!("f:{reset}:{at}"),
            models,
            reasons(invalid_value, None),
        )?;
    }
    if table_exists(connection, "usage_model_history")? {
        let mut q=connection.prepare("SELECT timestamp,reset_at,model,total_tokens,input_tokens,cached_input_tokens,cache_write_input_tokens,output_tokens,total_dollars FROM usage_model_history ORDER BY timestamp,reset_at,model")?;
        let mut rows = q.query([])?;
        while let Some(row) = rows.next()? {
            let at: i64 = row.get(0)?;
            let reset: i64 = row.get(1)?;
            let model: String = row.get(2)?;
            let mut invalid_value = false;
            let item = StoredModelObservation {
                stored_model_key: model.clone(),
                total_tokens: unsigned(row, 3, &mut invalid_value)?,
                input_tokens: unsigned(row, 4, &mut invalid_value)?,
                cached_input_tokens: unsigned(row, 5, &mut invalid_value)?,
                cache_write_input_tokens: unsigned(row, 6, &mut invalid_value)?,
                output_tokens: unsigned(row, 7, &mut invalid_value)?,
                total_dollars: dollars(row, 8, &mut invalid_value)?,
            };
            append(
                archive,
                at,
                Some(reset),
                "model-sidecar",
                format!("m:{}", digest(format!("{reset}:{at}:{model}").as_bytes())),
                vec![item],
                reasons(invalid_value, None),
            )?;
        }
    }
    if table_exists(connection, "session_events")? {
        let admission:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_events') WHERE name='admission_json')",[],|r|r.get(0))?;
        let qualification = if admission { "e.reset_at" } else { "NULL" };
        let filter = if !admission {
            ""
        } else if index_present {
            "WHERE e.admission_json IS NULL OR e.model='UNATTRIBUTED' OR EXISTS(SELECT 1 FROM recorded_usage_facts f WHERE f.source_key=json_array(e.root_identity,e.relative_path,e.file_device,e.file_inode,e.prefix_generation,CAST(e.range_start AS TEXT),CAST(e.range_end AS TEXT),e.record_sha256,CAST(e.event_index AS TEXT)) AND f.eligible=0)"
        } else {
            "WHERE e.admission_json IS NULL OR e.model='UNATTRIBUTED'"
        };
        let mut q=connection.prepare(&format!("SELECT e.timestamp,{qualification},e.model,e.total_tokens,e.input_tokens,e.cached_input_tokens,e.cache_write_input_tokens,e.output_tokens,json_array(e.root_identity,e.relative_path,e.file_device,e.file_inode,e.prefix_generation,CAST(e.range_start AS TEXT),CAST(e.range_end AS TEXT),e.record_sha256,CAST(e.event_index AS TEXT)) FROM session_events e {filter} ORDER BY e.timestamp"))?;
        let mut rows = q.query([])?;
        while let Some(row) = rows.next()? {
            let at: i64 = row.get(0)?;
            let reset: Option<i64> = row.get(1)?;
            let model: String = row.get(2)?;
            let key: String = row.get(8)?;
            let mut invalid_value = false;
            let item = StoredModelObservation {
                stored_model_key: model,
                total_tokens: unsigned(row, 3, &mut invalid_value)?,
                input_tokens: unsigned(row, 4, &mut invalid_value)?,
                cached_input_tokens: unsigned(row, 5, &mut invalid_value)?,
                cache_write_input_tokens: unsigned(row, 6, &mut invalid_value)?,
                output_tokens: unsigned(row, 7, &mut invalid_value)?,
                total_dollars: None,
            };
            append(
                archive,
                at,
                reset,
                "source-event",
                format!("e:{}", digest(key.as_bytes())),
                vec![item],
                reasons(
                    invalid_value,
                    (!recovered.contains(&key)).then_some("source-recovery-pending"),
                ),
            )?;
        }
    }
    Ok(())
}

// Only test builds count owned raw UTF-8 payload bytes. This is a lower
// bound on payload residency, not total allocation/RSS or admission evidence.
#[cfg(test)]
thread_local! {
    static PROOF_PAYLOAD_RESIDENCY: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
    static CANDIDATE_PAYLOAD_OWNERS: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
}

#[cfg(test)]
struct RecoveryPayload(String);

#[cfg(test)]
impl RecoveryPayload {
    fn new(payload: String) -> Self {
        PROOF_PAYLOAD_RESIDENCY.with(|counter| {
            let (live, peak) = counter.get();
            let live = live
                .checked_add(payload.len())
                .expect("payload byte count overflow");
            counter.set((live, peak.max(live)));
        });
        Self(payload)
    }
}

#[cfg(test)]
impl std::ops::Deref for RecoveryPayload {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl Drop for RecoveryPayload {
    fn drop(&mut self) {
        PROOF_PAYLOAD_RESIDENCY.with(|counter| {
            let (live, peak) = counter.get();
            counter.set((
                live.checked_sub(self.0.len())
                    .expect("payload byte count underflow"),
                peak,
            ));
        });
    }
}

#[cfg(not(test))]
type RecoveryPayload = String;

fn recovery_payload(row: &Row<'_>) -> rusqlite::Result<RecoveryPayload> {
    let payload: String = row.get(1)?;
    #[cfg(test)]
    CANDIDATE_PAYLOAD_OWNERS.with(|counter| {
        let (v1, v2) = counter.get();
        counter.set((
            v1 + usize::from(payload.as_bytes().ends_with(b",\"version\":1}")),
            v2 + usize::from(payload.as_bytes().ends_with(b",\"version\":2}")),
        ));
    });
    #[cfg(test)]
    let payload = RecoveryPayload::new(payload);
    Ok(payload)
}

/// Exclude canonical v1 and recognize canonical v2 without a header scan.
/// Other forms retain strict header classification, never proof admission.
pub fn source_repair_is_v2_candidate(
    payload: &str,
) -> std::result::Result<bool, serde_json::Error> {
    if payload.as_bytes().ends_with(b",\"version\":1}") {
        return Ok(false);
    }
    if payload.as_bytes().ends_with(b",\"version\":2}") {
        return Ok(true);
    }
    use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};

    struct VersionKey;
    impl<'de> DeserializeSeed<'de> for VersionKey {
        type Value = bool;

        fn deserialize<D: serde::Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> Result<bool, D::Error> {
            struct KeyVisitor;
            impl Visitor<'_> for KeyVisitor {
                type Value = bool;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("an object key")
                }

                fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<bool, E> {
                    Ok(value == "version")
                }
            }
            deserializer.deserialize_str(KeyVisitor)
        }
    }

    struct HeaderVisitor {
        root: bool,
    }
    impl<'de> DeserializeSeed<'de> for HeaderVisitor {
        type Value = bool;

        fn deserialize<D: serde::Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> Result<bool, D::Error> {
            deserializer.deserialize_any(self)
        }
    }
    impl<'de> Visitor<'de> for HeaderVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON value")
        }

        fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<bool, E> {
            Ok(!self.root && value == 2)
        }

        fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<bool, E> {
            Ok(!self.root && value == 2)
        }

        fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<bool, E> {
            Ok(!self.root && value == 2.0)
        }

        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<bool, E> {
            Ok(false)
        }

        fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<bool, E> {
            Ok(false)
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<bool, E> {
            Ok(false)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<bool, A::Error> {
            while sequence.next_element::<IgnoredAny>()?.is_some() {}
            Ok(false)
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
            if !self.root {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                return Ok(false);
            }
            let mut first_version = None;
            while let Some(is_version) = map.next_key_seed(VersionKey)? {
                if is_version && first_version.is_none() {
                    first_version = Some(map.next_value_seed(HeaderVisitor { root: false })?);
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(first_version.unwrap_or(false))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(payload);
    let candidate = HeaderVisitor { root: true }.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(candidate)
}

/// Decode the saved payload for the full v2 qualification consumer.
/// Candidate selection alone does not grant recovery authority.
pub fn decode_source_repair_v2(
    payload: &str,
) -> std::result::Result<serde_json::Value, serde_json::Error> {
    use serde::de::{MapAccess, Visitor};

    struct VersionObject;
    impl<'de> Visitor<'de> for VersionObject {
        type Value = serde_json::Value;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a source repair object with one integer version 2")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut object = serde_json::Map::new();
            let mut version_count = 0;
            let mut integer_two = false;
            while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
                if key == "version" {
                    version_count += 1;
                    integer_two = value.as_u64() == Some(2);
                }
                object.insert(key, value);
            }
            if version_count != 1 || !integer_two {
                return Err(serde::de::Error::custom("invalid source repair version"));
            }
            Ok(serde_json::Value::Object(object))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(payload);
    let value = serde::Deserializer::deserialize_map(&mut deserializer, VersionObject)?;
    deserializer.end()?;
    Ok(value)
}

/// Shared finite SQLite selection; excluded v1 rows never own Rust strings.
pub const SOURCE_REPAIR_V2_CANDIDATE_QUERY: &str = "SELECT repair_id,payload_json,payload_sha256 FROM source_prefix_repairs WHERE substr(CAST(payload_json AS BLOB),-13) <> CAST(',\"version\":1}' AS BLOB) ORDER BY repair_id";

const SOURCE_REPAIR_CANDIDATE_SQL: &str = SOURCE_REPAIR_V2_CANDIDATE_QUERY;
const SOURCE_REPAIR_HAS_PROOF_SQL: &str = "SELECT EXISTS(SELECT 1 FROM source_prefix_repairs)";

struct RecoveryCandidate {
    id: String,
    payload: RecoveryPayload,
    sha: String,
}

fn next_recovery_candidate(
    rows: &mut rusqlite::Rows<'_>,
) -> Result<Option<RecoveryCandidate>, ReaderError> {
    while let Some(row) = rows.next()? {
        let payload = recovery_payload(row)?;
        if !source_repair_is_v2_candidate(&payload)
            .map_err(|_| invalid("invalid recovery relation"))?
        {
            continue;
        }
        return Ok(Some(RecoveryCandidate {
            id: row.get(0)?,
            payload,
            sha: row.get(2)?,
        }));
    }
    Ok(None)
}

/// A completed relation changes the recovery label of the saved observation,
/// never its numbers or period identity. The already admitted fact index is
/// checked in this same read transaction; no Session file is opened here.
fn recovered_source_observations(
    connection: &Connection,
    index_present: bool,
) -> Result<BTreeSet<String>, ReaderError> {
    use serde_json::Value;
    let mut recovered = BTreeSet::new();
    if !index_present || !table_exists(connection, "source_prefix_repairs")? {
        return Ok(recovered);
    }
    let has_proof: bool =
        connection.query_row(SOURCE_REPAIR_HAS_PROOF_SQL, [], |row| row.get(0))?;
    if !has_proof {
        return Ok(recovered);
    }
    let mut query = connection.prepare(SOURCE_REPAIR_CANDIDATE_SQL)?;
    let mut rows = query.query([])?;
    let Some(first) = next_recovery_candidate(&mut rows)? else {
        return Ok(recovered);
    };
    let pointer = |value: &Value| -> Result<String, ReaderError> {
        let fields = value
            .as_array()
            .ok_or_else(|| invalid("invalid recovery pointer"))?;
        if fields.len() != 9 || fields.iter().any(|field| field.as_str().is_none()) {
            return Err(invalid("invalid recovery pointer fields"));
        }
        serde_json::to_string(fields).map_err(|_| invalid("invalid recovery pointer encoding"))
    };
    let mut facts = BTreeMap::new();
    let mut source_ids = BTreeMap::new();
    {
        let mut query = connection.prepare("SELECT fact_id,source_key,timestamp,model,tokens_json FROM recorded_usage_facts WHERE eligible=1 ORDER BY fact_id")?;
        let mut rows = query.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            source_ids.insert(row.get::<_, String>(1)?, id.clone());
            let tokens: TokenFacts = serde_json::from_str(&row.get::<_, String>(4)?)
                .map_err(|_| invalid("invalid recovered fact tokens"))?;
            tokens
                .validate_fact()
                .map_err(|_| invalid("invalid recovered numeric fact"))?;
            facts.insert(
                id,
                (row.get::<_, i64>(2)?, row.get::<_, String>(3)?, tokens),
            );
        }
    }
    let candidates = std::iter::once(Ok(first)).chain(std::iter::from_fn(|| {
        next_recovery_candidate(&mut rows).transpose()
    }));
    for candidate in candidates {
        let RecoveryCandidate { id, payload, sha } = candidate?;
        if id != sha || digest(payload.as_bytes()) != sha {
            return Err(invalid("recovery relation digest mismatch"));
        }
        let value: Value =
            decode_source_repair_v2(&payload).map_err(|_| invalid("invalid recovery relation"))?;
        if !matches!(
            value["mode"].as_str(),
            Some("identical-prefix-alias" | "event-bijection")
        ) {
            return Err(invalid("invalid recovery relation mode"));
        }
        let prefixes = value["prefixes"]
            .as_array()
            .ok_or_else(|| invalid("missing recovery prefixes"))?;
        if prefixes.is_empty()
            || prefixes
                .iter()
                .any(|prefix| prefix["parser_version"] != "codex-info-session-recorder-v3")
        {
            return Err(invalid("invalid recovery parser version"));
        }
        let mut original_keys = BTreeSet::new();
        for original in value["original"]
            .as_array()
            .ok_or_else(|| invalid("missing recovery original inventory"))?
        {
            let source = &original["source"];
            let text = |name: &str| {
                source[name]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("invalid recovery original source"))
            };
            let number = |name: &str| {
                source[name]
                    .as_u64()
                    .map(|n| n.to_string())
                    .ok_or_else(|| invalid("invalid recovery original number"))
            };
            original_keys.insert(
                serde_json::to_string(&[
                    text("root_identity")?,
                    text("relative_path")?,
                    number("file_device")?,
                    number("file_inode")?,
                    text("prefix_generation")?,
                    number("range_start")?,
                    number("range_end")?,
                    text("record_sha256")?,
                    number("event_index")?,
                ])
                .map_err(|_| invalid("invalid recovery original encoding"))?,
            );
        }
        let mappings = value["mappings"]
            .as_array()
            .ok_or_else(|| invalid("missing recovery mappings"))?;
        for mapping in mappings {
            let original = pointer(&mapping["original"])?;
            if !original_keys.contains(&original) {
                return Err(invalid("recovery mapping is outside original inventory"));
            }
            let canonical = pointer(&mapping["canonical"])?;
            let qualified = &mapping["qualified"];
            match mapping["canonical_kind"].as_str() {
                Some("qualified-source") if pointer(&qualified["source"])? == canonical => {}
                Some("retained-response")
                    if original == canonical
                        && qualified["admission"]["numeric"]["kind"] == "response-record" => {}
                _ => return Err(invalid("invalid recovery canonical relation")),
            }
            let response_id = qualified["admission"]["numeric"]["candidate"]["key"]
                .as_str()
                .map(|key| format!("r:{key}"));
            let fact = source_ids
                .get(&canonical)
                .or(response_id.as_ref())
                .and_then(|id| facts.get(id));
            // A later independent ambiguity can make a fact ineligible. Keep
            // the observation pending rather than inventing current authority.
            let Some((timestamp, model, tokens)) = fact else {
                continue;
            };
            let qualified_tokens: TokenFacts = serde_json::from_value(qualified["tokens"].clone())
                .map_err(|_| invalid("invalid recovery mapping tokens"))?;
            let numeric = &qualified["admission"]["numeric"];
            let exact_model = numeric["legacy_family_exact_model"]
                .as_str()
                .or_else(|| numeric["exact_model"].as_str())
                .or_else(|| qualified["model"].as_str());
            if qualified["timestamp"].as_i64() != Some(*timestamp)
                || exact_model != Some(model.as_str())
                || qualified_tokens != *tokens
            {
                return Err(invalid("recovery relation differs from admitted fact"));
            }
            recovered.insert(original);
        }
    }
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue673_local_usage_empty_positive_and_timed_pending_are_independent_of_quota() {
        let mut archive = RecordedArchive {
            local_index_initialized: true,
            ..RecordedArchive::default()
        };
        assert_eq!(
            archive.local_usage(60, 660).unwrap().intervals,
            [LocalUsageInterval {
                start_at: 60,
                end_at: 660,
                state: LocalUsageState::Unused
            }]
        );
        let mut prefix = RecordedTokenPrefix::default();
        prefix
            .add(TokenFacts {
                total: 10,
                input: Some(8),
                cached_input: Some(2),
                cache_write_input: Some(0),
                output: Some(2),
            })
            .unwrap();
        archive.prefixes.insert("SOL".into(), vec![(120, prefix)]);
        archive.pending_numeric_times.extend([120, 180]);
        let actual = archive.local_usage(60, 660).unwrap();
        let expected = [
            (60, 120, LocalUsageState::Unused),
            (120, 180, LocalUsageState::Used),
            (180, 240, LocalUsageState::Unavailable),
            (240, 660, LocalUsageState::Unused),
        ];
        assert_eq!(
            actual.intervals,
            expected.map(|(start_at, end_at, state)| LocalUsageInterval {
                start_at,
                end_at,
                state
            })
        );
        assert!(archive.quota_observations.is_empty());
        assert_eq!(archive.coverage, RecordingCoverage::Unknown);
        assert!(
            read_local(&Connection::open_in_memory().unwrap(), |_| true).is_err(),
            "a missing index is not a processed empty log set"
        );
    }

    #[test]
    fn issue673_pending_candidates_are_localized_without_an_offset_only_window_veto() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE session_pending_ranges(root_identity TEXT,relative_path TEXT,
            file_device TEXT,file_inode TEXT,prefix_generation TEXT,start_offset INTEGER,end_offset INTEGER);
            CREATE TABLE session_events(root_identity TEXT,relative_path TEXT,file_device TEXT,file_inode TEXT,
            prefix_generation TEXT,range_start INTEGER,range_end INTEGER,timestamp INTEGER,total_tokens TEXT,admission_json TEXT);
            CREATE TABLE session_checkpoints(root_identity TEXT,relative_path TEXT,file_device TEXT,
                file_inode TEXT,prefix_generation TEXT,source_admission TEXT);
            INSERT INTO session_pending_ranges VALUES('r','p','1','2','3',0,100),('r','offset-only','1','4','3',0,200);
            INSERT INTO session_events VALUES('r','p','1','2','3',10,20,180,'5',NULL);").unwrap();
        let pending = serde_json::json!({"responses":[
            {"pointer":{"timestamp":300,"start":30,"end":40},"record":{"usage":{"total":2}}},
            {"pointer":{"timestamp":360,"start":200,"end":220},"record":{"usage":{"total":2}}},
            {"pointer":{"timestamp":420,"start":40,"end":50},"record":{"usage":{"total":0}}},
            {"pointer":{"timestamp":900,"start":50,"end":60},"record":{"usage":{"total":2}}}]});
        connection
            .execute(
                "INSERT INTO session_checkpoints VALUES('r','p','1','2','3',?1)",
                [pending.to_string()],
            )
            .unwrap();
        let mut archive = RecordedArchive {
            local_index_initialized: true,
            ..RecordedArchive::default()
        };
        read_pending_numeric_times(&connection, &mut archive, |at| at < 600).unwrap();
        assert_eq!(archive.pending_numeric_times, BTreeSet::from([180, 300]));
        assert_eq!(
            archive.local_usage(360, 960).unwrap().intervals,
            [LocalUsageInterval {
                start_at: 360,
                end_at: 960,
                state: LocalUsageState::Unused
            }]
        );
    }

    #[test]
    fn issue604_recorded_projection_diagnostic_preserves_result() {
        let connection = Connection::open_in_memory().unwrap();
        // Existing storage definitions, with no fabricated fact or proof.
        connection
            .execute_batch(
                "CREATE TABLE recorded_usage_facts (
                fact_id TEXT NOT NULL PRIMARY KEY,source_key TEXT NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL CHECK(timestamp>0),model TEXT NOT NULL,
                tokens_json TEXT NOT NULL,evidence_sha256 TEXT NOT NULL,
                source_identity TEXT NOT NULL,ambiguity_key TEXT,
                eligible INTEGER NOT NULL CHECK(eligible IN (0,1))
             ) WITHOUT ROWID;
             CREATE TABLE recorded_usage_prefixes (
                model TEXT NOT NULL,timestamp INTEGER NOT NULL CHECK(timestamp>0),
                prefix_json TEXT NOT NULL,prefix_sha256 TEXT NOT NULL,
                PRIMARY KEY(model,timestamp)
             ) WITHOUT ROWID;
             CREATE TABLE recorded_usage_state (
                singleton INTEGER NOT NULL PRIMARY KEY CHECK(singleton=1),
                format_version INTEGER NOT NULL CHECK(format_version=1),
                initialized INTEGER NOT NULL CHECK(initialized IN (0,1)),
                fact_revision TEXT NOT NULL,models_json TEXT NOT NULL,
                coverage TEXT NOT NULL,dirty_json TEXT NOT NULL
             ) WITHOUT ROWID;
             INSERT INTO recorded_usage_state VALUES(1,1,0,'0','[]','unknown','{}');
             CREATE TABLE usage_history (
                timestamp INTEGER,reset_at INTEGER,sol_tokens INTEGER,
                terra_tokens INTEGER,luna_tokens INTEGER,sol_dollars REAL,
                terra_dollars REAL,luna_dollars REAL
             );",
            )
            .unwrap();
        let mut notifications = Vec::new();
        let archive = read_with_diagnostics(&connection, |reason| notifications.push(reason))
            .expect("a valid empty archive remains available");
        assert_eq!(archive, RecordedArchive::default());
        assert!(
            archive.local_usage(60, 660).is_err(),
            "a valid auxiliary archive is not proof of an initialized local index"
        );
        assert!(notifications.is_empty());
        connection
            .execute("UPDATE recorded_usage_state SET dirty_json='[]'", [])
            .unwrap();
        let result = read_with_diagnostics(&connection, |reason| notifications.push(reason));
        assert!(matches!(result, Err(ReaderError::InvalidValue(ref message))
            if message == "uncommitted recorded projection"));
        assert_eq!(notifications, ["uncommitted_recorded_projection"]);
        let reason = recorded_projection_failure_reason(&invalid(
            "UNKNOWN_PRIVATE_SENTINEL model=secret path=/private/source account=private",
        ));
        assert_eq!(reason, "recorded_value_invalid");
        assert!(!reason.contains("PRIVATE"));
        assert!(!reason.contains("secret"));
    }

    #[test]
    fn issue604_proof_candidates_preserve_versions_without_sql_json_reparse() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE source_prefix_repairs (repair_id TEXT PRIMARY KEY,payload_json TEXT NOT NULL,payload_sha256 TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE recorded_usage_facts (
                fact_id TEXT NOT NULL PRIMARY KEY,source_key TEXT NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL CHECK(timestamp>0),model TEXT NOT NULL,
                tokens_json TEXT NOT NULL,evidence_sha256 TEXT NOT NULL,
                source_identity TEXT NOT NULL,ambiguity_key TEXT,
                eligible INTEGER NOT NULL CHECK(eligible IN (0,1))
             ) WITHOUT ROWID;"
        ).unwrap();
        let large_v1 = format!(
            r#"{{"body":"{}","version":1}}"#,
            r#"é漢字\n\"\\literal"#.repeat(4096)
        );
        // Selection literals are not admitted recovery proofs.
        let cases = [
            ("canonical-v1-small", r#"{"body":"literal","version":1}"#),
            ("canonical-v1-large", large_v1.as_str()),
            (
                "canonical-v1-nested",
                r#"{"body":{"text":",\"version\":1}"},"version":1}"#,
            ),
            (
                "canonical-v2",
                r#"{"body":{"text":",\"version\":1}"},"mappings":[],"mode":"event-bijection","original":[],"prefixes":[],"version":2}"#,
            ),
            ("plain2", r#"{"version":2,"backup":{"version":1}}"#),
            ("decimal2", r#"{"version":2.0}"#),
            ("exponent2", r#"{"version":2e0}"#),
            ("spaced2", " { \"version\" : 2 } "),
            ("noncanonical-v1", r#"{"version":1,"body":"tail"}"#),
            ("string2", r#"{"version":"2"}"#),
            ("boolean", r#"{"version":true}"#),
            ("null", r#"{"version":null}"#),
            ("nested-only", r#"{"backup":{"version":2}}"#),
            ("root-array", r#"[{"version":2}]"#),
            ("root-number", "2"),
            ("version-array", r#"{"version":[2]}"#),
            ("version-object", r#"{"version":{"version":2}}"#),
        ];
        for (id, payload) in cases {
            connection
                .execute(
                    "INSERT INTO source_prefix_repairs VALUES(?1,?2,?1)",
                    rusqlite::params![id, payload],
                )
                .unwrap();
        }
        CANDIDATE_PAYLOAD_OWNERS.with(|counter| counter.set((0, 0)));
        let mut selected = BTreeSet::new();
        {
            let mut query = connection.prepare(SOURCE_REPAIR_CANDIDATE_SQL).unwrap();
            let mut rows = query.query([]).unwrap();
            while let Some(candidate) = next_recovery_candidate(&mut rows).unwrap() {
                assert!(selected.insert(candidate.id));
            }
        }
        let owner_counts = CANDIDATE_PAYLOAD_OWNERS.with(std::cell::Cell::get);
        let expected = ["canonical-v2", "plain2", "decimal2", "exponent2", "spaced2"]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            selected, expected,
            "canonical candidates and conservative fallback"
        );
        assert_eq!(
            owner_counts.1, 1,
            "the canonical v2 row reaches its Rust owner"
        );

        for (payload, sha, expected_error) in [
            (
                r#"{"body":,"version":2}"#,
                "6eed95eb637410c09e83d34a9fac2839c6bfee82e1873d2211133f7c1950aeac",
                "invalid recovery relation",
            ),
            (
                r#"{"mappings":[],"mode":"event-bijection","original":[],"prefixes":[],"version":2}"#,
                "0000000000000000000000000000000000000000000000000000000000000000",
                "recovery relation digest mismatch",
            ),
        ] {
            connection
                .execute("DELETE FROM source_prefix_repairs", [])
                .unwrap();
            connection
                .execute(
                    "INSERT INTO source_prefix_repairs VALUES(?1,?2,?1)",
                    rusqlite::params![sha, payload],
                )
                .unwrap();
            let result = recovered_source_observations(&connection, true);
            assert!(
                matches!(result, Err(ReaderError::InvalidValue(ref message)) if message == expected_error),
                "malformed or mismatched v2 remains rejected: {result:?}"
            );
        }
        // This shared full-JSON decoder is connected to both qualification
        // consumers; these literals test its version boundary, not a proof.
        let invalid_versions = [
            ("decimal", r#"{"version":2.0}"#),
            ("exponent", r#"{"version":2e0}"#),
            ("string", r#"{"version":"2"}"#),
            ("boolean", r#"{"version":true}"#),
            ("null", r#"{"version":null}"#),
            ("missing", r#"{"body":{"version":2}}"#),
            ("root-array", r#"[{"version":2}]"#),
            ("duplicate-two", r#"{"version":2,"version":2}"#),
            ("duplicate-one-two", r#"{"version":1,"version":2}"#),
        ];
        let version_rejections = invalid_versions
            .map(|(name, payload)| (name, decode_source_repair_v2(payload).is_err()));
        for payload in [
            r#"{"version":2,"body":{"version":1}}"#,
            " { \"body\" : [true], \"version\" : 2 } ",
        ] {
            let value = decode_source_repair_v2(payload).expect("one integer root version 2");
            assert_eq!(value["version"].as_u64(), Some(2));
        }
        for sql in [SOURCE_REPAIR_CANDIDATE_SQL, SOURCE_REPAIR_HAS_PROOF_SQL] {
            let json_functions = connection
                .prepare(&format!("EXPLAIN {sql}"))
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, Option<String>>(5)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .into_iter()
                .filter_map(|(opcode, p4)| {
                    p4.filter(|name| {
                        matches!(opcode.as_str(), "Function" | "PureFunc")
                            && name.starts_with("json_")
                    })
                })
                .collect::<Vec<_>>();
            assert!(
                json_functions.is_empty(),
                "no SQLite JSON proof parse: {json_functions:?}"
            );
        }
        assert_eq!(
            owner_counts.0, 0,
            "excluded canonical v1 rows never become owned Rust payloads"
        );
        assert!(
            version_rejections.iter().all(|(_, rejected)| *rejected),
            "full qualification requires one integer root version 2: {version_rejections:?}"
        );
    }

    #[test]
    fn issue604_recovery_empty_proof_catalog() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE source_prefix_repairs (repair_id TEXT PRIMARY KEY,payload_json TEXT NOT NULL,payload_sha256 TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE recorded_usage_facts (
                fact_id TEXT NOT NULL PRIMARY KEY,
                source_key TEXT NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL CHECK(timestamp>0),
                model TEXT NOT NULL,
                tokens_json TEXT NOT NULL,
                evidence_sha256 TEXT NOT NULL,
                source_identity TEXT NOT NULL,
                ambiguity_key TEXT,
                eligible INTEGER NOT NULL CHECK(eligible IN (0,1))
             ) WITHOUT ROWID;
             INSERT INTO recorded_usage_facts VALUES(
                'literal-fact','literal-source',100,'literal-model',
                'not-json','literal-evidence','literal-identity',NULL,1);"
        ).unwrap();
        // With no proof, this helper must not qualify or reject unrelated
        // facts. This does not claim that the invalid fact is admissible.
        let recovered = recovered_source_observations(&connection, true)
            .expect("an empty proof catalog does not inspect fact tokens");
        assert!(recovered.is_empty());
    }

    #[test]
    fn issue604_proof_payload_residency() {
        let connection = Connection::open_in_memory().unwrap();
        // Canonical existing table definitions; no valid proof or fact is
        // fabricated. Both independent version-2 JSON rows have invalid modes.
        connection.execute_batch(
            "CREATE TABLE source_prefix_repairs (repair_id TEXT PRIMARY KEY,payload_json TEXT NOT NULL,payload_sha256 TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE recorded_usage_facts (
                fact_id TEXT NOT NULL PRIMARY KEY,
                source_key TEXT NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL CHECK(timestamp>0),
                model TEXT NOT NULL,
                tokens_json TEXT NOT NULL,
                evidence_sha256 TEXT NOT NULL,
                source_identity TEXT NOT NULL,
                ambiguity_key TEXT,
                eligible INTEGER NOT NULL CHECK(eligible IN (0,1))
             ) WITHOUT ROWID;"
        ).unwrap();
        for payload in [
            r#"{"version":2,"mode":"invalid-first","literal":"日本語"}"#,
            r#"{"version":2,"mode":"invalid-second","literal":"é-literal"}"#,
        ] {
            let sha = digest(payload.as_bytes());
            connection
                .execute(
                    "INSERT INTO source_prefix_repairs VALUES(?1,?2,?1)",
                    rusqlite::params![sha, payload],
                )
                .unwrap();
        }
        let (rows, max_payload_bytes): (i64, i64) = connection.query_row(
            "SELECT count(*),max(length(CAST(payload_json AS BLOB))) FROM source_prefix_repairs",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(rows, 2);
        PROOF_PAYLOAD_RESIDENCY.with(|counter| {
            assert_eq!(counter.get().0, 0);
            counter.set((0, 0));
        });
        let result = recovered_source_observations(&connection, true);
        assert!(
            matches!(result, Err(ReaderError::InvalidValue(ref message))
            if message == "invalid recovery relation mode"),
            "{result:?}"
        );
        let (live, peak_raw_payload_bytes) = PROOF_PAYLOAD_RESIDENCY.with(|counter| counter.get());
        assert_eq!(live, 0, "reject must release every owned payload");
        assert!(
            peak_raw_payload_bytes > 0,
            "production query must own the tested payload"
        );
        assert!(usize::try_from(max_payload_bytes).unwrap() >= peak_raw_payload_bytes,
            "owned raw UTF-8 peak {peak_raw_payload_bytes} exceeds independent single-row max {max_payload_bytes}");
    }
}

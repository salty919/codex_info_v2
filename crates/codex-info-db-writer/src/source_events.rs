//! Lossless reads of saved source events. A recorded model label and token
//! values are not, by themselves, new source/model/official-period authority.

use crate::event_admission::{EventAdmission, SessionEventPeriod};
use crate::{Result, SessionEvent, TokenFacts, UsageStoreError};
use rusqlite::{Connection, Row};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionEventSource {
    pub root_identity: String,
    pub relative_path: String,
    pub file_device: u64,
    pub file_inode: u64,
    pub prefix_generation: u128,
    pub range_start: u64,
    pub range_end: u64,
    pub record_sha256: String,
    pub event_index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LosslessSessionEvent {
    pub source: SessionEventSource,
    pub timestamp: i64,
    pub model: String,
    pub tokens: TokenFacts,
    pub period: Option<SessionEventPeriod>,
    pub admission: Option<EventAdmission>,
}

impl From<SessionEvent> for LosslessSessionEvent {
    fn from(event: SessionEvent) -> Self {
        Self {
            source: SessionEventSource {
                root_identity: event.root_identity,
                relative_path: event.relative_path,
                file_device: event.file_device,
                file_inode: event.file_inode,
                prefix_generation: event.prefix_generation,
                range_start: event.range_start,
                range_end: event.range_end,
                record_sha256: event.record_sha256,
                event_index: event.event_index,
            },
            timestamp: event.timestamp,
            model: event.model,
            tokens: TokenFacts {
                total: event.total_tokens,
                input: event.input_tokens,
                cached_input: event.cached_input_tokens,
                cache_write_input: event.cache_write_input_tokens,
                output: event.output_tokens,
            },
            period: None,
            admission: None,
        }
    }
}

/// The caller owns partition identity and snapshot admission. This performs
/// SELECT only and never repairs a row or interprets missing fields as zero.
pub fn read_session_event_facts(connection: &Connection) -> Result<Vec<LosslessSessionEvent>> {
    read_matching_raw(connection, "", [])
}

pub(super) fn response_by_key(
    connection: &Connection,
    key: &str,
) -> Result<Option<LosslessSessionEvent>> {
    let mut values=read_matching(connection,
        "WHERE json_extract(admission_json,'$.numeric.kind')='response-record' AND json_extract(admission_json,'$.numeric.candidate.key')=?1",[key])?;
    if values.len() > 1 {
        return Err(UsageStoreError::InvalidImport(
            "duplicate canonical response identity".into(),
        ));
    }
    Ok(values.pop())
}

pub fn same_response(existing: &LosslessSessionEvent, candidate: &LosslessSessionEvent) -> bool {
    use crate::event_admission::NumericEventProof;
    let response = |fact: &LosslessSessionEvent| {
        fact.admission
            .as_ref()
            .and_then(|admission| match &admission.numeric {
                NumericEventProof::ResponseRecord { candidate, .. } => Some((
                    candidate.record.ids.clone(),
                    candidate.record.usage,
                    candidate.session_scope_qualified,
                    candidate.qualified_model().map(str::to_owned),
                )),
                _ => None,
            })
    };
    existing.timestamp == candidate.timestamp
        && response(existing).is_some()
        && response(existing) == response(candidate)
}

pub(super) fn read_matching<P: rusqlite::Params>(
    connection: &Connection,
    filter: &str,
    parameters: P,
) -> Result<Vec<LosslessSessionEvent>> {
    let _scope = crate::source_qualification_repair::CanonicalSourceScope::begin(connection)?;
    let table = crate::source_qualification_repair::canonical_table(connection)?;
    read_from_table(connection, filter, parameters, &table)
}

pub(super) fn read_matching_raw<P: rusqlite::Params>(
    connection: &Connection,
    filter: &str,
    parameters: P,
) -> Result<Vec<LosslessSessionEvent>> {
    read_from_table(connection, filter, parameters, "session_events")
}

fn read_from_table<P: rusqlite::Params>(
    connection: &Connection,
    filter: &str,
    parameters: P,
    table: &str,
) -> Result<Vec<LosslessSessionEvent>> {
    let qualified_columns: i64 = connection.query_row(
        "SELECT count(*) FROM pragma_table_info('session_events')
         WHERE name IN ('reset_at','window_seconds','admission_json')",
        [],
        |row| row.get(0),
    )?;
    let qualification = match qualified_columns {
        0 => "NULL,NULL,NULL",
        3 => "reset_at,window_seconds,admission_json",
        _ => {
            return Err(UsageStoreError::InvalidImport(
                "partial event qualification schema".into(),
            ))
        }
    };
    let mut statement = connection.prepare(&format!(
        "SELECT root_identity, relative_path, file_device, file_inode,
                prefix_generation, range_start, range_end, record_sha256,
                event_index, timestamp, model, total_tokens, input_tokens,
                cached_input_tokens, output_tokens, cache_write_input_tokens, {qualification}
         FROM {table} {filter}
         ORDER BY timestamp, root_identity, relative_path, range_start, event_index,
                  file_device, file_inode, prefix_generation, range_end, record_sha256",
    ))?;
    let rows = statement
        .query_map(parameters, |row| {
            Ok(LosslessSessionEvent {
                source: SessionEventSource {
                    root_identity: row.get(0)?,
                    relative_path: row.get(1)?,
                    file_device: required_unsigned(row, 2)?,
                    file_inode: required_unsigned(row, 3)?,
                    prefix_generation: crate::canonical_u128_hex(
                        &row.get::<_, String>(4)?,
                        "source event generation",
                    )
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    range_start: unsigned_position(row, 5)?,
                    range_end: unsigned_position(row, 6)?,
                    record_sha256: row.get(7)?,
                    event_index: unsigned_position(row, 8)?,
                },
                timestamp: row.get(9)?,
                model: row.get(10)?,
                tokens: TokenFacts {
                    total: required_unsigned(row, 11)?,
                    input: optional_unsigned(row, 12)?,
                    cached_input: optional_unsigned(row, 13)?,
                    output: optional_unsigned(row, 14)?,
                    cache_write_input: optional_unsigned(row, 15)?,
                },
                period: match (
                    row.get::<_, Option<i64>>(16)?,
                    row.get::<_, Option<i64>>(17)?,
                ) {
                    (None, None) => None,
                    (Some(reset_at), Some(window_seconds)) => Some(SessionEventPeriod {
                        reset_at,
                        window_seconds,
                    }),
                    _ => return Err(rusqlite::Error::InvalidQuery),
                },
                admission: row
                    .get::<_, Option<String>>(18)?
                    .map(|value| {
                        serde_json::from_str::<EventAdmission>(&value)
                            .map_err(|_| rusqlite::Error::InvalidQuery)
                    })
                    .transpose()?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut partition = None;
    for event in &rows {
        validate(event)?;
        if let Some(admission) = event.admission.as_ref() {
            let identity = &admission.source_binding.partition;
            if let Some(previous) = partition {
                if previous != identity {
                    return Err(UsageStoreError::InvalidImport(
                        "event admission partition mismatch".into(),
                    ));
                }
            } else {
                crate::validate_storage_partition_identity(connection, identity)?;
                partition = Some(identity);
            }
        }
    }
    Ok(rows)
}

fn unsigned_position(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    u64::try_from(row.get::<_, i64>(index)?).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn required_unsigned(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    optional_unsigned(row, index)?.ok_or(rusqlite::Error::InvalidQuery)
}

fn optional_unsigned(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    row.get::<_, Option<String>>(index)?
        .map(|value| {
            crate::canonical_u64_text(&value, "source event unsigned value")
                .map_err(|_| rusqlite::Error::InvalidQuery)
        })
        .transpose()
}

fn validate(event: &LosslessSessionEvent) -> Result<()> {
    let source = &event.source;
    crate::validate_session_key(&source.root_identity, &source.relative_path)?;
    if source.range_start >= source.range_end
        || source.range_end > i64::MAX as u64
        || source.event_index > i64::MAX as u64
        || source.prefix_generation == 0
        || event.timestamp <= 0
        || event.timestamp > crate::MAX_PUBLIC_UNIX_SECONDS
        || !crate::valid_session_model(&event.model)
        || event.tokens.validate_fact().is_err()
    {
        return Err(UsageStoreError::InvalidImport(
            "lossless session event is invalid".into(),
        ));
    }
    if match &event.admission {
        Some(admission) => !admission.valid_for(event, event.period),
        None => event.period.is_some(),
    } {
        return Err(UsageStoreError::InvalidImport(
            "source event admission is invalid".into(),
        ));
    }
    crate::validate_sha256(&source.record_sha256, "session event record")
}

impl TryFrom<LosslessSessionEvent> for SessionEvent {
    type Error = UsageStoreError;

    fn try_from(event: LosslessSessionEvent) -> Result<Self> {
        validate(&event)?;
        let source = event.source;
        let legacy = Self {
            root_identity: source.root_identity,
            relative_path: source.relative_path,
            file_device: source.file_device,
            file_inode: source.file_inode,
            prefix_generation: source.prefix_generation,
            range_start: source.range_start,
            range_end: source.range_end,
            record_sha256: source.record_sha256,
            event_index: source.event_index,
            timestamp: event.timestamp,
            model: event.model,
            total_tokens: event.tokens.total,
            input_tokens: event.tokens.input,
            cached_input_tokens: event.tokens.cached_input,
            output_tokens: event.tokens.output,
            cache_write_input_tokens: event.tokens.cache_write_input,
        };
        crate::validate_session_event(&legacy)?;
        Ok(legacy)
    }
}

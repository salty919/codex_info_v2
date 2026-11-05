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
    let mut values=read_canonical_evidence(connection,
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
    read_from_table(connection, filter, parameters, &table, validate)
}

/// Canonical original evidence for comparison. Retirement exclusion and all
/// known source/owner/pointer/numeric/period contradictions remain enforced.
pub(super) fn read_canonical_evidence<P: rusqlite::Params>(
    connection: &Connection,
    filter: &str,
    parameters: P,
) -> Result<Vec<LosslessSessionEvent>> {
    let _scope = crate::source_qualification_repair::CanonicalSourceScope::begin(connection)?;
    let table = crate::source_qualification_repair::canonical_table(connection)?;
    read_from_table(connection, filter, parameters, &table, validate_raw)
}

/// Canonical numeric projection keeps retirement exclusion, but an original
/// whose model is not yet qualified contributes only incomplete coverage.
/// The raw-role validator still rejects every known evidence contradiction.
pub(super) fn read_canonical_projection<P: rusqlite::Params>(
    connection: &Connection,
    filter: &str,
    parameters: P,
) -> Result<(Vec<LosslessSessionEvent>, Option<i64>)> {
    let mut facts = read_canonical_evidence(connection, filter, parameters)?;
    let mut first_unconfirmed: Option<i64> = None;
    facts.retain(|event| {
        let confirmed =
            event
                .admission
                .as_ref()
                .is_some_and(|admission| match &admission.numeric {
                    crate::event_admission::NumericEventProof::ResponseRecord {
                        candidate, ..
                    } => candidate.qualified_model().is_some(),
                    _ => true,
                });
        if !confirmed {
            first_unconfirmed =
                Some(first_unconfirmed.map_or(event.timestamp, |old| old.min(event.timestamp)));
        }
        confirmed
    });
    Ok((facts, first_unconfirmed))
}

pub(super) fn read_matching_raw<P: rusqlite::Params>(
    connection: &Connection,
    filter: &str,
    parameters: P,
) -> Result<Vec<LosslessSessionEvent>> {
    read_from_table(
        connection,
        filter,
        parameters,
        "session_events",
        validate_raw,
    )
}

fn read_from_table<P: rusqlite::Params>(
    connection: &Connection,
    filter: &str,
    parameters: P,
    table: &str,
    validate_row: fn(&LosslessSessionEvent) -> Result<()>,
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
        validate_row(event)?;
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
    validate_admission_role(event, true)
}

// Original values/proof are input to repair and equivalence comparison, not
// evidence that the supplied model has already qualified for aggregation.
fn validate_raw(event: &LosslessSessionEvent) -> Result<()> {
    validate_admission_role(event, false)
}

fn validate_admission_role(event: &LosslessSessionEvent, qualified: bool) -> Result<()> {
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
    let invalid_admission = match &event.admission {
        Some(admission) => {
            if qualified {
                !admission.valid_for(event, event.period)
            } else {
                !admission.valid_for_requalification(event, event.period)
            }
        }
        None => event.period.is_some(),
    };
    if invalid_admission {
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

#[cfg(test)]
pub(crate) mod issue659_raw_evidence_tests {
    use super::*;
    use crate::event_admission::{EventOwnership, NumericEventProof, PeriodAttributionProof};
    use crate::source_admission::{
        ClassificationPointer, ClassificationProof, PendingResponse, RecordPointer,
        ResponseContext, ResponseCounterAssociation, ResponseIds, ResponseModelClassification,
        ResponseModelFailure, ResponseRecord, ResponseUsage, SessionMetadata, SourceAccountBinding,
        SourceAdmission, SourceBindingProcess, SourceBindingProof, StrongContext,
    };

    const AT: i64 = 1_791_871_200;

    fn fixture(unassigned: bool) -> (Connection, LosslessSessionEvent, String) {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::SCHEMA).unwrap();
        connection.execute_batch(crate::PARTITION_SCHEMA).unwrap();
        let tx = connection.transaction().unwrap();
        crate::ensure_session_event_schema(&tx).unwrap();
        crate::stamp_current_account_db_schema(&tx).unwrap();
        tx.commit().unwrap();
        let partition = crate::StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "a".repeat(64),
            storage_epoch: 7,
            partition_id: "a".repeat(64),
        };
        connection.execute("INSERT INTO storage_partition(singleton,schema_version,profile_scope_id,account_scope_id,storage_epoch,partition_id) VALUES(1,?1,?2,?3,'7',?4)", rusqlite::params![partition.schema_version,partition.profile_scope_id,partition.account_scope_id,partition.partition_id]).unwrap();
        let usage = ResponseUsage {
            total: 5,
            input: 4,
            cached_input: 1,
            cache_write_input: 0,
            output: 1,
            reasoning_output: 0,
        };
        let record = ResponseRecord {
            ids: ResponseIds {
                thread_id: "thread".into(),
                session_id: "session".into(),
                root_turn_id: "root-turn".into(),
                turn_id: "turn".into(),
                response_id: "response".into(),
            },
            usage,
            turn_usage: usage,
            thread_usage: usage,
        };
        let boundary = ClassificationProof::Record {
            pointer: ClassificationPointer {
                start: 50,
                end: 60,
                sha256: "00".repeat(32),
            },
        };
        let candidate = PendingResponse {
            key: record.ids.key(),
            record,
            pointer: RecordPointer {
                start: 20,
                end: 40,
                sha256: "00".repeat(32),
                timestamp: AT,
            },
            session_scope_qualified: true,
            root_context: Some(ResponseContext {
                model: "gpt-6.1-sol".into(),
                context: StrongContext {
                    turn_id: Some("turn".into()),
                    root_turn_id: Some("root-turn".into()),
                    pointer: RecordPointer {
                        start: 0,
                        end: 10,
                        sha256: "00".repeat(32),
                        timestamp: AT - 1,
                    },
                },
            }),
            classification: ResponseModelClassification::Normal {
                boundary: boundary.clone(),
            },
            counter_association: ResponseCounterAssociation::Awaiting,
        };
        let mut event = LosslessSessionEvent {
            source: SessionEventSource {
                root_identity: "unix:10:20".into(),
                relative_path: "one.jsonl".into(),
                file_device: 10,
                file_inode: 42,
                prefix_generation: 0x5678,
                range_start: 0,
                range_end: 100,
                record_sha256: "00".repeat(32),
                event_index: 20,
            },
            timestamp: AT,
            model: "gpt-6.1-sol".into(),
            tokens: TokenFacts {
                total: 5,
                input: Some(4),
                cached_input: Some(1),
                cache_write_input: Some(0),
                output: Some(1),
            },
            period: Some(SessionEventPeriod {
                reset_at: AT + 100,
                window_seconds: 1000,
            }),
            admission: Some(EventAdmission {
                version: 1,
                source_binding: SourceAccountBinding {
                    version: 1,
                    partition,
                    root_identity: "unix:10:20".into(),
                    relative_path: "one.jsonl".into(),
                    file_device: 10,
                    file_inode: 42,
                    prefix_generation: format!("{:032x}", 0x5678u128),
                    prefix_end: 100,
                    metadata: SessionMetadata {
                        thread_id: "thread".into(),
                        session_id: Some("session".into()),
                    },
                    proof: SourceBindingProof::Rpc {
                        began_at: AT - 10,
                        ended_at: AT - 9,
                        process: SourceBindingProcess {
                            pid: 1234,
                            starttime_ticks: 17,
                            executable_device: 10,
                            executable_inode: 20,
                        },
                    },
                    creator_origin: None,
                },
                verified_prefix_end: 100,
                ownership: EventOwnership {
                    start_at: AT - 1000,
                    end_at: None,
                },
                numeric: NumericEventProof::ResponseRecord {
                    candidate: Box::new(candidate),
                    legacy_family_exact_model: None,
                },
                period: Some(PeriodAttributionProof {
                    rule_version: 1,
                    lower_at: AT,
                    upper_at: AT,
                    evidence_generation: "1".into(),
                    evidence_sha256: "00".repeat(32),
                }),
            }),
        };
        assert!(
            event
                .admission
                .as_ref()
                .unwrap()
                .valid_for(&event, event.period),
            "qualified control must be valid before changing only classification"
        );
        let a = event.admission.as_mut().unwrap();
        let NumericEventProof::ResponseRecord { candidate, .. } = &mut a.numeric else {
            unreachable!()
        };
        if unassigned {
            event.model = "UNATTRIBUTED".into();
            candidate.classification = ResponseModelClassification::Unassigned {
                reason: ResponseModelFailure::UnrecognizedControl,
                boundary: boundary.clone(),
            };
            candidate.counter_association = ResponseCounterAssociation::Unmatched { boundary };
            candidate.root_context.as_mut().unwrap().context.turn_id = Some("other-turn".into());
            candidate
                .root_context
                .as_mut()
                .unwrap()
                .context
                .root_turn_id = Some("other-root-turn".into());
        } else {
            candidate.classification = ResponseModelClassification::AwaitingBoundary;
        }
        let mut source = SourceAdmission::from_response_boundary();
        source.metadata = Some(a.source_binding.metadata.clone());
        source.metadata_qualified = true;
        source.push_response((**candidate).clone());
        assert!(
            source.valid(100),
            "pending/unassigned proof is structurally valid"
        );
        assert!(candidate.qualified_model().is_none());
        assert!(
            !event
                .admission
                .as_ref()
                .unwrap()
                .valid_for(&event, event.period),
            "raw success must not become qualified model/period acceptance"
        );
        let original = serde_json::to_string(event.admission.as_ref().unwrap()).unwrap();
        connection.execute("INSERT INTO session_events(root_identity,relative_path,file_device,file_inode,prefix_generation,range_start,range_end,record_sha256,event_index,timestamp,model,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens,reset_at,window_seconds,admission_json) VALUES('unix:10:20','one.jsonl','10','42',?1,0,100,?2,20,?3,?4,'5','4','1','1','0',?5,1000,?6)",rusqlite::params![format!("{:032x}",0x5678u128),"00".repeat(32),AT,event.model,AT+100,original]).unwrap();
        (connection, event, original)
    }

    fn assert_raw_preserved(unassigned: bool) {
        let (connection, event, original) = fixture(unassigned);
        let before = connection.total_changes();
        let raw=read_matching_raw(&connection,"WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5",rusqlite::params!["unix:10:20","one.jsonl","10","42",format!("{:032x}",0x5678u128)])
            .expect("RAW_REQUALIFICATION_EVIDENCE_REJECTED");
        assert_eq!(raw.as_slice(), std::slice::from_ref(&event));
        assert!(!raw[0]
            .admission
            .as_ref()
            .unwrap()
            .valid_for(&raw[0], raw[0].period));
        let qualified = read_matching(&connection, "", []);
        assert!(
            matches!(qualified, Err(UsageStoreError::InvalidImport(ref reason))
                if reason == "source event admission is invalid"),
            "actual qualified reader must reject unresolved model proof: {qualified:?}"
        );
        let (projection, pending_at) = read_canonical_projection(&connection, "", []).unwrap();
        assert!(
            projection.is_empty(),
            "pending raw evidence became qualified consumption"
        );
        assert_eq!(pending_at, Some(event.timestamp));
        let retained: String = connection
            .query_row("SELECT admission_json FROM session_events", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(retained, original);
        assert_eq!(connection.total_changes(), before);
    }

    #[test]
    fn awaiting_model_proof_remains_readable_for_requalification() {
        assert_raw_preserved(false);
    }

    #[test]
    fn unattributed_unassigned_proof_is_retained_without_model_promotion() {
        assert_raw_preserved(true);
    }

    fn replace_proof(connection: &Connection, value: &serde_json::Value) {
        connection
            .execute(
                "UPDATE session_events SET admission_json=?1",
                [serde_json::to_string(value).unwrap()],
            )
            .unwrap();
    }

    fn actual_raw(connection: &Connection) -> Result<Vec<LosslessSessionEvent>> {
        read_matching_raw(connection, "", [])
    }

    #[test]
    fn raw_rejects_known_contradictions_instead_of_bypassing_qualification() {
        let mut accepted = Vec::new();
        for kind in [
            "source",
            "owner",
            "numeric",
            "pointer",
            "period",
            "known-model-conflict",
            "normal-id-mismatch",
            "normal-context-missing",
            "compaction-id-missing",
            "session-scope-missing",
        ] {
            let (connection, _, original) = fixture(false);
            let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
            let boundary = serde_json::json!({"kind":"record","pointer":{"start":50,"end":60,"sha256":"00".repeat(32)}});
            match kind {
                "source" => {
                    value["source_binding"]["root_identity"] = serde_json::json!("unix:10:21")
                }
                "owner" => value["ownership"]["start_at"] = serde_json::json!(AT + 1),
                "numeric" => {
                    value["numeric"]["candidate"]["record"]["usage"]["total"] = serde_json::json!(6)
                }
                "pointer" => {
                    value["numeric"]["candidate"]["pointer"]["end"] = serde_json::json!(101)
                }
                "period" => value["period"]["evidence_sha256"] = serde_json::json!("bad"),
                "known-model-conflict" => {
                    value["numeric"]["candidate"]["classification"] =
                        serde_json::json!({"state":"normal","boundary":boundary});
                    value["numeric"]["candidate"]["root_context"]["model"] =
                        serde_json::json!("gpt-6.1-terra");
                }
                "normal-id-mismatch" => {
                    value["numeric"]["candidate"]["classification"] =
                        serde_json::json!({"state":"normal","boundary":boundary});
                    value["numeric"]["candidate"]["root_context"]["context"]["turn_id"] =
                        serde_json::json!("different");
                }
                "normal-context-missing" => {
                    value["numeric"]["candidate"]["classification"] =
                        serde_json::json!({"state":"normal","boundary":boundary});
                    value["numeric"]["candidate"]["root_context"] = serde_json::Value::Null;
                }
                "compaction-id-missing" => {
                    let replacement = value["numeric"]["candidate"]["root_context"].clone();
                    value["numeric"]["candidate"]["classification"] = serde_json::json!({"state":"compaction","snapshot":{"start":50,"end":60,"sha256":"00".repeat(32)},"replacement":replacement});
                    value["numeric"]["candidate"]["classification"]["replacement"]["context"]
                        ["pointer"] = serde_json::json!({"start":60,"end":70,"timestamp":AT+1,"sha256":"00".repeat(32)});
                    value["numeric"]["candidate"]["root_context"]["context"]["turn_id"] =
                        serde_json::Value::Null;
                }
                "session-scope-missing" => {
                    value["numeric"]["candidate"]["session_scope_qualified"] =
                        serde_json::json!(false)
                }
                _ => unreachable!(),
            }
            replace_proof(&connection, &value);
            let before = connection.total_changes();
            if actual_raw(&connection).is_ok() {
                accepted.push(kind);
            }
            assert!(
                read_matching(&connection, "", []).is_err(),
                "qualified reader accepted {kind}"
            );
            assert!(
                read_canonical_projection(&connection, "", []).is_err(),
                "projection accepted {kind}"
            );
            assert!(
                read_canonical_evidence(&connection, "", []).is_err(),
                "comparison inventory accepted a known contradiction: {kind}"
            );
            assert!(
                crate::load_session_event_replay_identities_from_connection(&connection).is_err(),
                "opaque comparison inventory accepted a known contradiction: {kind}"
            );
            let key = value["numeric"]["candidate"]["key"].as_str().unwrap();
            assert!(
                response_by_key(&connection, key).is_err(),
                "response comparison accepted a known contradiction: {kind}"
            );
            assert_eq!(connection.total_changes(), before);
        }
        assert!(
            accepted.is_empty(),
            "RAW_CONTRADICTORY_PROOF_ACCEPTED {accepted:?}"
        );
    }

    #[test]
    fn only_explicit_unresolved_classifications_are_raw_evidence() {
        for state in [
            "legacy-unknown",
            "awaiting-boundary",
            "awaiting-replacement",
        ] {
            let (connection, _, original) = fixture(false);
            let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
            value["numeric"]["candidate"]["classification"] = if state == "awaiting-replacement" {
                serde_json::json!({"state":state,"snapshot":{"start":50,"end":60,"sha256":"00".repeat(32)}})
            } else {
                serde_json::json!({"state":state})
            };
            if state != "awaiting-replacement" {
                value["numeric"]["candidate"]["root_context"] = serde_json::Value::Null;
            }
            replace_proof(&connection, &value);
            let before = connection.total_changes();
            assert_eq!(actual_raw(&connection).unwrap().len(), 1);
            let (projection, pending_at) = read_canonical_projection(&connection, "", []).unwrap();
            assert!(projection.is_empty());
            assert_eq!(pending_at, Some(AT));
            assert!(read_matching(&connection, "", []).is_err());
            assert_eq!(connection.total_changes(), before);
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SavedProjectionCells {
        family_bits: Option<u64>,
        family_unknown: bool,
        family_tokens: Option<i64>,
        unknown_tokens: bool,
        total: String,
        cache_write: Option<String>,
        dollar_bits: Option<u64>,
        price: Option<String>,
    }

    fn saved_projection_cells(connection: &Connection) -> SavedProjectionCells {
        let (dollar,unknown,tokens,unknown_tokens):(Option<f64>,Option<f64>,Option<i64>,Option<i64>)=connection.query_row(
            "SELECT sol_dollars,terra_dollars,sol_tokens,terra_tokens FROM usage_history WHERE reset_at=?1 AND timestamp=?2",
            rusqlite::params![AT+100,AT],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        let (total,cache_write,usd,price):(String,Option<String>,Option<f64>,Option<String>)=connection.query_row(
            "SELECT total_tokens,cache_write_input_tokens,total_dollars,price_version FROM usage_model_history WHERE reset_at=?1 AND timestamp=?2 AND model='gpt-6.1-sol'",
            rusqlite::params![AT+100,AT],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        SavedProjectionCells {
            family_bits: dollar.map(f64::to_bits),
            family_unknown: unknown.is_none(),
            family_tokens: tokens,
            unknown_tokens: unknown_tokens.is_none(),
            total,
            cache_write,
            dollar_bits: usd.map(f64::to_bits),
            price,
        }
    }

    pub(crate) fn pending_model_projection_store(
        unassigned: bool,
    ) -> (crate::UsageStore, crate::StoragePartitionIdentity, String) {
        let (mut connection, old, original) = fixture(unassigned);
        // Reuse the real create_partitioned initialization statements for all
        // remaining in-memory tables; no guessed shortened schema.
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(crate::quota_receipts::SCHEMA)
            .unwrap();
        transaction
            .execute_batch(crate::period_evidence::SCHEMA)
            .unwrap();
        transaction
            .execute_batch(crate::source_qualification_repair::SCHEMA)
            .unwrap();
        transaction
            .execute_batch(crate::source_qualification_repair::CANDIDATE_INDEX_SCHEMA)
            .unwrap();
        transaction
            .execute_batch(crate::recorded_usage::SCHEMA)
            .unwrap();
        transaction
            .execute_batch(crate::session_quota_recovery::WINDOW_SCHEMA)
            .unwrap();
        crate::quota_receipts::ensure_health_schema(&transaction).unwrap();
        crate::ensure_collection_generation_live_quota_schema(&transaction, false).unwrap();
        crate::ensure_storage_partition_login_id_schema(&transaction).unwrap();
        crate::ensure_durable_state_schema(&transaction).unwrap();
        crate::ensure_recorder_gap_ledger_schema(&transaction).unwrap();
        crate::ensure_session_checkpoint_schema(&transaction).unwrap();
        crate::ensure_history_continuity_schema(&transaction).unwrap();
        crate::ensure_usage_model_history_schema(&transaction).unwrap();
        crate::ensure_session_cumulative_recovery_schema(&transaction).unwrap();
        crate::ensure_session_timeline_recovery_schema(&transaction).unwrap();
        crate::ensure_session_pending_range_schema(&transaction).unwrap();
        crate::ensure_session_token_anchor_recovery_schema(&transaction).unwrap();
        crate::ensure_session_event_schema(&transaction).unwrap();
        crate::ensure_session_task_event_schema(&transaction).unwrap();
        crate::ensure_active_thread_snapshot_schema(&transaction).unwrap();
        crate::migrate_nullable_history_schema(&transaction).unwrap();
        crate::stamp_current_account_db_schema(&transaction).unwrap();
        transaction.commit().unwrap();
        let saved_usd = 1.234_567_890_123_456_f64;
        connection.execute("INSERT INTO usage_history(timestamp,reset_at,remaining_percent,sol_dollars,terra_dollars,luna_dollars,sol_tokens,terra_tokens,luna_tokens) VALUES(?1,?2,50.0,?3,NULL,0.0,100,NULL,0)",rusqlite::params![AT,AT+100,saved_usd]).unwrap();
        connection.execute("INSERT INTO usage_model_history(reset_at,timestamp,model,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens,model_set_complete,total_dollars,price_version) VALUES(?1,?2,'gpt-6.1-sol','100','90','10','10',NULL,0,?3,'synthetic-saved-v1')",rusqlite::params![AT+100,AT,saved_usd]).unwrap();
        let mut new = old.clone();
        new.timestamp = AT + 1;
        new.model = "gpt-6.1-terra".into();
        new.source.event_index = 60;
        new.tokens = TokenFacts {
            total: 7,
            input: Some(6),
            cached_input: Some(1),
            cache_write_input: Some(0),
            output: Some(1),
        };
        let a = new.admission.as_mut().unwrap();
        a.period.as_mut().unwrap().lower_at = AT + 1;
        a.period.as_mut().unwrap().upper_at = AT + 1;
        let NumericEventProof::ResponseRecord { candidate, .. } = &mut a.numeric else {
            unreachable!()
        };
        candidate.record.ids.response_id = "qualified-new-response".into();
        candidate.key = candidate.record.ids.key();
        candidate.record.usage.total = 7;
        candidate.record.usage.input = 6;
        candidate.record.turn_usage = candidate.record.usage;
        candidate.record.thread_usage = candidate.record.usage;
        candidate.pointer.start = 60;
        candidate.pointer.end = 70;
        candidate.pointer.timestamp = AT + 1;
        candidate.root_context.as_mut().unwrap().model = "gpt-6.1-terra".into();
        candidate.root_context.as_mut().unwrap().context.turn_id =
            Some(candidate.record.ids.turn_id.clone());
        candidate
            .root_context
            .as_mut()
            .unwrap()
            .context
            .root_turn_id = Some(candidate.record.ids.root_turn_id.clone());
        candidate.counter_association = ResponseCounterAssociation::Awaiting;
        candidate.classification = ResponseModelClassification::Normal {
            boundary: ClassificationProof::Record {
                pointer: ClassificationPointer {
                    start: 80,
                    end: 90,
                    sha256: "00".repeat(32),
                },
            },
        };
        assert!(new.admission.as_ref().unwrap().valid_for(&new, new.period));
        let new_json = serde_json::to_string(new.admission.as_ref().unwrap()).unwrap();
        connection.execute("INSERT INTO session_events SELECT root_identity,relative_path,file_device,file_inode,prefix_generation,range_start,range_end,record_sha256,60,?1,'gpt-6.1-terra','7','6','1','1','0',reset_at,window_seconds,?2 FROM session_events WHERE event_index=20",rusqlite::params![AT+1,new_json]).unwrap();
        let partition = old
            .admission
            .as_ref()
            .unwrap()
            .source_binding
            .partition
            .clone();
        let mut store = crate::UsageStore {
            connection,
            period_evidence_cache: Default::default(),
        };
        let input = |sequence, observed_at| crate::period_evidence::OfficialQuotaInput {
            input_id: format!("{sequence:064x}"),
            account_scope: partition.account_scope_id.clone(),
            sequence,
            observed_at,
            content_sha256: format!("{sequence:064x}"),
            reset_at: Some(AT + 100),
            window_seconds: Some(1000),
            reason: None,
        };
        store
            .stage_official_quota_inputs(&[input(1, AT - 5), input(2, AT + 5)])
            .unwrap();
        (store, partition, original)
    }

    fn assert_pending_model_projection(unassigned: bool) {
        let (store, partition, original) = pending_model_projection_store(unassigned);
        let before = saved_projection_cells(&store.connection);
        let sql_before = store.connection.total_changes();
        let prepared = store
            .prepare_period_attribution(
                &partition,
                &[crate::SessionLifecycleInterval::new(Some(AT - 1000), None)],
            )
            .expect("PENDING_MODEL_PERIOD_PREPARE_REJECTED");
        let (events, incomplete) = store
            .session_events_for_prepared_period(&prepared, AT + 100, 1000)
            .expect("PENDING_MODEL_PERIOD_PROJECTION_REJECTED");
        assert!(
            incomplete,
            "pending model must not become a complete subset"
        );
        assert_eq!(
            events.len(),
            1,
            "new qualified consumption must remain in the projection"
        );
        assert_eq!(events[0].model, "gpt-6.1-terra");
        assert_eq!(events[0].total_tokens, 7);
        assert_eq!(
            saved_projection_cells(&store.connection),
            before,
            "saved values/price bits/NULL changed during projection"
        );
        let retained: String = store
            .connection
            .query_row(
                "SELECT admission_json FROM session_events WHERE event_index=20",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(retained, original);
        assert_eq!(store.connection.total_changes(), sql_before);
    }

    #[test]
    fn historical_partial_models_preserve_saved_prices_and_new_qualified_consumption() {
        for unassigned in [false, true] {
            let (mut store, partition, original) = pending_model_projection_store(unassigned);
            // The numeric/model-qualified new response initially has no
            // official period; the prepared official observations supply it.
            let mut newly_qualified = read_matching(&store.connection, "WHERE event_index=60", [])
                .unwrap()
                .pop()
                .unwrap();
            newly_qualified.period = None;
            newly_qualified.admission.as_mut().unwrap().period = None;
            assert!(newly_qualified
                .admission
                .as_ref()
                .unwrap()
                .valid_for(&newly_qualified, None));
            let new_json =
                serde_json::to_string(newly_qualified.admission.as_ref().unwrap()).unwrap();
            store.connection.execute("UPDATE session_events SET reset_at=NULL,window_seconds=NULL,admission_json=?1 WHERE event_index=60",[new_json]).unwrap();
            store.connection.execute("UPDATE collection_generation SET data_generation='1',reset_at=?1,latest_quota_reset_at=?1,window_seconds=1000,collector_epoch=?2,cycle_seq='1' WHERE singleton=1",rusqlite::params![AT+100,format!("{:032x}",0x5678u128)]).unwrap();
            store.connection.execute("INSERT INTO session_model_totals(model,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens) VALUES('gpt-6.1-sol','100','90','10','10',NULL),('gpt-6.1-terra','7','6','1','1','0')",[]).unwrap();
            let before = store.load_session_collection_state().unwrap().model_totals;
            let prices = saved_projection_cells(&store.connection);
            let prepared = store
                .prepare_period_attribution(
                    &partition,
                    &[crate::SessionLifecycleInterval::new(Some(AT - 1000), None)],
                )
                .unwrap();
            store
                .commit_historical_source_repairs(&prepared, &[])
                .expect("HISTORICAL_PENDING_MODEL_PROJECTION_REJECTED");
            assert_eq!(
                store.load_session_collection_state().unwrap().model_totals,
                before,
                "a qualified subset removed saved consumption or the new seven tokens"
            );
            assert_eq!(
                saved_projection_cells(&store.connection),
                prices,
                "historical partial projection changed saved numeric/price bits/NULL"
            );
            let retained: String = store
                .connection
                .query_row(
                    "SELECT admission_json FROM session_events WHERE event_index=20",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(retained, original);
            let count: i64 = store
                .connection
                .query_row("SELECT count(*) FROM session_events", [], |row| row.get(0))
                .unwrap();
            assert_eq!(
                count, 2,
                "history repair duplicated or deleted source evidence"
            );
            let fact_count: i64 = store
                .connection
                .query_row("SELECT count(*) FROM recorded_usage_facts", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(
                fact_count, 1,
                "pending saved model label became a recorded fact or new consumption disappeared"
            );
            let (model, tokens): (String, String) = store
                .connection
                .query_row(
                    "SELECT model,tokens_json FROM recorded_usage_facts",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(model, "gpt-6.1-terra");
            let tokens: TokenFacts = serde_json::from_str(&tokens).unwrap();
            assert_eq!(
                tokens,
                TokenFacts {
                    total: 7,
                    input: Some(6),
                    cached_input: Some(1),
                    cache_write_input: Some(0),
                    output: Some(1)
                }
            );
            let (projection, incomplete) = store
                .session_events_for_prepared_period(&prepared, AT + 100, 1000)
                .unwrap();
            assert!(incomplete);
            assert_eq!(projection.len(), 1);
            assert_eq!(projection[0].total_tokens, 7);
            assert_eq!(projection[0].model, "gpt-6.1-terra");
        }
        println!("HISTORICAL_PENDING_MODEL_PRESERVATION exact_and_unassigned=true saved_prices_and_null=true new_qualified_seven=true");
    }

    #[test]
    fn pending_model_keeps_projection_incomplete_and_preserves_saved_values() {
        assert_pending_model_projection(false);
    }

    #[test]
    fn unassigned_model_keeps_projection_incomplete_without_promotion() {
        assert_pending_model_projection(true);
    }
    #[test]
    fn normal_collection_commit_does_not_require_model_qualification_for_replay_inventory() {
        for unassigned in [false, true] {
            let (mut store, _, original) = pending_model_projection_store(unassigned);
            let totals = vec![
                crate::SessionModelTotal {
                    model: "gpt-6.1-sol".into(),
                    total_tokens: 100,
                    input_tokens: Some(90),
                    cached_input_tokens: Some(10),
                    output_tokens: Some(10),
                    cache_write_input_tokens: None,
                },
                crate::SessionModelTotal {
                    model: "gpt-6.1-terra".into(),
                    total_tokens: 7,
                    input_tokens: Some(6),
                    cached_input_tokens: Some(1),
                    output_tokens: Some(1),
                    cache_write_input_tokens: Some(0),
                },
            ];
            let prices = saved_projection_cells(&store.connection);
            store
                .commit_session_collection(crate::SessionCollectionCommit {
                    reset_at: AT + 100,
                    window_seconds: 1000,
                    collector_epoch: 0x5678,
                    cycle_seq: 1,
                    samples: &[],
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &totals,
                    recorded_sessions: &[],
                })
                .expect("NORMAL_COMMIT_REPLAY_INVENTORY_REJECTED");
            assert_eq!(
                store.load_session_collection_state().unwrap().model_totals,
                totals
            );
            assert_eq!(saved_projection_cells(&store.connection), prices);
            let retained: String = store
                .connection
                .query_row(
                    "SELECT admission_json FROM session_events WHERE event_index=20",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(retained, original);
            assert!(
                read_matching(&store.connection, "WHERE event_index=20", []).is_err(),
                "comparison inventory must not become qualified adoption"
            );
        }
    }

    #[test]
    fn response_identity_lookup_retains_pending_model_for_existing_conflict_rules() {
        for unassigned in [false, true] {
            let (connection, event, original) = fixture(unassigned);
            let store = crate::UsageStore {
                connection,
                period_evidence_cache: Default::default(),
            };
            let NumericEventProof::ResponseRecord { candidate, .. } =
                &event.admission.as_ref().unwrap().numeric
            else {
                unreachable!()
            };
            let before = store.connection.total_changes();
            let saved = store
                .response_event(&candidate.key)
                .expect("RESPONSE_COMPARISON_INVENTORY_REJECTED")
                .expect("existing raw response must not be omitted");
            assert_eq!(saved, event);
            assert!(!saved
                .admission
                .as_ref()
                .unwrap()
                .valid_for(&saved, saved.period));
            let mut qualified = event.clone();
            let NumericEventProof::ResponseRecord { candidate, .. } =
                &mut qualified.admission.as_mut().unwrap().numeric
            else {
                unreachable!()
            };
            candidate.classification = ResponseModelClassification::Normal {
                boundary: ClassificationProof::Record {
                    pointer: ClassificationPointer {
                        start: 50,
                        end: 60,
                        sha256: "00".repeat(32),
                    },
                },
            };
            candidate.root_context.as_mut().unwrap().context.turn_id =
                Some(candidate.record.ids.turn_id.clone());
            candidate
                .root_context
                .as_mut()
                .unwrap()
                .context
                .root_turn_id = Some(candidate.record.ids.root_turn_id.clone());
            candidate.counter_association = ResponseCounterAssociation::Awaiting;
            qualified.model = "gpt-6.1-sol".into();
            assert!(qualified
                .admission
                .as_ref()
                .unwrap()
                .valid_for(&qualified, qualified.period));
            assert!(
                !crate::same_response(&saved, &qualified),
                "an unconfirmed saved model cannot be inferred from a new response"
            );
            let retained: String = store
                .connection
                .query_row("SELECT admission_json FROM session_events", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(retained, original);
            assert_eq!(store.connection.total_changes(), before);

            let mut duplicate = event.clone();
            duplicate.source.relative_path = "second.jsonl".into();
            duplicate
                .admission
                .as_mut()
                .unwrap()
                .source_binding
                .relative_path = "second.jsonl".into();
            assert!(duplicate.admission.as_ref().unwrap().valid_for_requalification(&duplicate,duplicate.period),
                    "the second canonical response must be structurally valid before duplicate detection");
            let duplicate_json =
                serde_json::to_string(duplicate.admission.as_ref().unwrap()).unwrap();
            let duplicate_insert = store.connection.execute(
                "INSERT INTO session_events(root_identity,relative_path,file_device,file_inode,prefix_generation,range_start,range_end,record_sha256,event_index,timestamp,model,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens,reset_at,window_seconds,admission_json)
                 SELECT root_identity,'second.jsonl',file_device,file_inode,prefix_generation,range_start,range_end,record_sha256,event_index,timestamp,model,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens,reset_at,window_seconds,?1 FROM session_events",
                [duplicate_json],
            );
            assert!(
                matches!(duplicate_insert,Err(rusqlite::Error::SqliteFailure(error,_))
                            if error.code == rusqlite::ErrorCode::ConstraintViolation),
                "the existing response identity UNIQUE constraint must reject the duplicate"
            );
            let NumericEventProof::ResponseRecord { candidate, .. } =
                &event.admission.as_ref().unwrap().numeric
            else {
                unreachable!()
            };
            assert_eq!(store.response_event(&candidate.key).unwrap(), Some(event));
            let rows: i64 = store
                .connection
                .query_row("SELECT count(*) FROM session_events", [], |row| row.get(0))
                .unwrap();
            let retained: String = store
                .connection
                .query_row("SELECT admission_json FROM session_events", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 1);
            assert_eq!(retained, original);
            assert_eq!(
                store.connection.total_changes(),
                before,
                "rejected duplicate must leave the original row and values unchanged"
            );
        }
    }
    #[test]
    fn exact_numeric_commit_does_not_adopt_comparison_only_pending_evidence() {
        for unassigned in [false, true] {
            let (mut store, partition, original) = pending_model_projection_store(unassigned);
            let before_state = store.load_session_collection_state().unwrap();
            let before_price = saved_projection_cells(&store.connection);
            let before_rows: Vec<(String, String)> = store.connection.prepare(
                "SELECT CAST(event_index AS TEXT),admission_json FROM session_events ORDER BY event_index"
            ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
                .collect::<rusqlite::Result<_>>().unwrap();
            assert_eq!(store.session_event_replay_identities().unwrap().len(), 2);
            assert!(
                store.load_session_events().is_err(),
                "public numeric reader lost its full qualification requirement"
            );
            assert_eq!(
                read_matching(&store.connection, "WHERE event_index=60", [])
                    .unwrap()
                    .len(),
                1,
                "the independently qualified seven-token control must remain usable"
            );
            let prepared = store
                .prepare_period_attribution(
                    &partition,
                    &[crate::SessionLifecycleInterval::new(Some(AT - 1000), None)],
                )
                .unwrap();
            let totals = vec![
                crate::SessionModelTotal {
                    model: "gpt-6.1-sol".into(),
                    total_tokens: 100,
                    input_tokens: Some(90),
                    cached_input_tokens: Some(10),
                    output_tokens: Some(10),
                    cache_write_input_tokens: None,
                },
                crate::SessionModelTotal {
                    model: "gpt-6.1-terra".into(),
                    total_tokens: 7,
                    input_tokens: Some(6),
                    cached_input_tokens: Some(1),
                    output_tokens: Some(1),
                    cache_write_input_tokens: Some(0),
                },
            ];
            let sample = crate::UsageHistorySample {
                timestamp: AT,
                reset_at: AT + 100,
                remaining_percent: Some(50.0),
                sol_dollars: 1.234_567_890_123_456,
                terra_dollars: 0.0,
                luna_dollars: 0.0,
                sol_tokens: 100,
                terra_tokens: 7,
                luna_tokens: 0,
            };
            let result = store.commit_session_collection_with_observations_inner(
                crate::SessionCollectionCommit {
                    reset_at: AT + 100,
                    window_seconds: 1000,
                    collector_epoch: 0x5678,
                    cycle_seq: 1,
                    samples: std::slice::from_ref(&sample),
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &totals,
                    recorded_sessions: &[],
                },
                &[],
                Some(AT + 100),
                crate::CollectionEvidence {
                    source_repairs: &[],
                    response_replays: &[],
                    period_attribution: Some(&prepared),
                    qualified_events: Some(&[]),
                    source_quota: None,
                    exact_history_span: Some((AT, AT + 59)),
                    cumulative_recovery: None,
                    timeline_recovery: None,
                    pending_ranges: None,
                    token_anchor_recoveries: None,
                    token_anchor_resolutions: None,
                    events: None,
                    task_events: None,
                    task_indexed_ranges: None,
                },
            );
            match result {
                Err(crate::UsageStoreError::InvalidImport(message)) => assert_eq!(
                    message,"source event admission is invalid",
                    "the exact numeric boundary must reject the unqualified model, not a fixture/setup input"
                ),
                _ => panic!("exact numeric history accepted comparison evidence or failed for an unrelated reason"),
            }
            assert_eq!(
                store.load_session_collection_state().unwrap(),
                before_state,
                "rejected exact reconstruction changed the accepted collection state"
            );
            assert_eq!(
                saved_projection_cells(&store.connection),
                before_price,
                "rejected exact reconstruction changed saved quantities, price bits or NULL"
            );
            let after_rows: Vec<(String,String)> = store.connection.prepare(
                "SELECT CAST(event_index AS TEXT),admission_json FROM session_events ORDER BY event_index"
            ).unwrap().query_map([],|row|Ok((row.get(0)?,row.get(1)?))).unwrap()
                .collect::<rusqlite::Result<_>>().unwrap();
            assert_eq!(after_rows, before_rows);
            assert_eq!(after_rows[0].1, original);
        }
    }
    #[test]
    fn source_preparation_reuses_verified_view_and_commit_starts_fresh() {
        for unassigned in [false, true] {
            let (mut store, _, original) = pending_model_projection_store(unassigned);
            let prices = saved_projection_cells(&store.connection);
            let original_admission: EventAdmission = serde_json::from_str(&original).unwrap();
            let NumericEventProof::ResponseRecord { candidate, .. } = &original_admission.numeric
            else {
                unreachable!()
            };
            let key = candidate.key.clone();
            crate::source_qualification_repair::CanonicalSourceScope::reset_test_snapshot_load_count();
            let before = store.connection.total_changes();
            // Preparation shares the proof view; the writer must load a fresh one.
            let scope = store.source_preparation_scope().unwrap();
            let _canonical = store.canonical_source_snapshot().unwrap();
            assert_eq!(store.session_event_replay_identities().unwrap().len(), 2);
            for _ in 0..2 {
                let saved = store.response_event(&key).unwrap().unwrap();
                assert_eq!(
                    serde_json::to_string(saved.admission.as_ref().unwrap()).unwrap(),
                    original
                );
                assert!(!saved
                    .admission
                    .as_ref()
                    .unwrap()
                    .valid_for(&saved, saved.period));
            }
            assert_eq!(store.connection.total_changes(), before);
            assert_eq!(
                crate::source_qualification_repair::CanonicalSourceScope::test_snapshot_load_count(
                ),
                1,
                "SOURCE_PREPARATION_RELOADED_IMMUTABLE_PROOF"
            );
            drop(scope);
            let totals = vec![
                crate::SessionModelTotal {
                    model: "gpt-6.1-sol".into(),
                    total_tokens: 100,
                    input_tokens: Some(90),
                    cached_input_tokens: Some(10),
                    output_tokens: Some(10),
                    cache_write_input_tokens: None,
                },
                crate::SessionModelTotal {
                    model: "gpt-6.1-terra".into(),
                    total_tokens: 7,
                    input_tokens: Some(6),
                    cached_input_tokens: Some(1),
                    output_tokens: Some(1),
                    cache_write_input_tokens: Some(0),
                },
            ];
            store
                .commit_session_collection(crate::SessionCollectionCommit {
                    reset_at: AT + 100,
                    window_seconds: 1000,
                    collector_epoch: 0x5678,
                    cycle_seq: 1,
                    samples: &[],
                    checkpoints: &[],
                    ranges: &[],
                    model_totals: &totals,
                    recorded_sessions: &[],
                })
                .unwrap();
            assert_eq!(
                crate::source_qualification_repair::CanonicalSourceScope::test_snapshot_load_count(
                ),
                2,
                "write transaction reused the preparation proof view"
            );
            assert_eq!(
                store.load_session_collection_state().unwrap().model_totals,
                totals
            );
            assert_eq!(saved_projection_cells(&store.connection), prices);
            let saved: String = store
                .connection
                .query_row(
                    "SELECT admission_json FROM session_events WHERE event_index=20",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(saved, original);
        }
    }
}

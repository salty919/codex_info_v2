//! Immutable official-input interpretations and usage-owned evidence snapshots.
//! Acquisition and current quota can advance while a usage batch remains pending.

use crate::event_admission::{EventOwnership, PeriodAttributionProof, SessionEventPeriod};
use crate::period_attribution::{AttributionTimeline, OfficialObservation};
use crate::{
    LosslessSessionEvent, Result, SessionLifecycleInterval, StoragePartitionIdentity, UsageStore,
    UsageStoreError,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS official_period_inputs (
 revision INTEGER PRIMARY KEY AUTOINCREMENT,
 input_id TEXT NOT NULL,
 interpretation_json TEXT NOT NULL,
 interpretation_sha256 TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS official_period_input_identity
 ON official_period_inputs(input_id,revision DESC);
CREATE TABLE IF NOT EXISTS period_evidence_snapshots (
 generation INTEGER PRIMARY KEY CHECK(generation>0),
 sha256 TEXT NOT NULL,
 payload_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS period_evidence_head (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 generation INTEGER NOT NULL,
 sha256 TEXT NOT NULL,
 input_revision INTEGER NOT NULL
);";

pub(super) fn validate_schema(connection: &Connection) -> Result<()> {
    let tables = [
        (
            "official_period_inputs",
            vec![
                ("revision", "INTEGER", 1),
                ("input_id", "TEXT", 0),
                ("interpretation_json", "TEXT", 0),
                ("interpretation_sha256", "TEXT", 0),
            ],
        ),
        (
            "period_evidence_snapshots",
            vec![
                ("generation", "INTEGER", 1),
                ("sha256", "TEXT", 0),
                ("payload_json", "TEXT", 0),
            ],
        ),
        (
            "period_evidence_head",
            vec![
                ("singleton", "INTEGER", 1),
                ("generation", "INTEGER", 0),
                ("sha256", "TEXT", 0),
                ("input_revision", "INTEGER", 0),
            ],
        ),
    ];
    for (table, expected) in tables {
        let mut statement = connection.prepare(&format!(
            "SELECT name,type,pk FROM pragma_table_info('{table}') ORDER BY cid"
        ))?;
        let actual = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if actual
            != expected
                .into_iter()
                .map(|(n, t, p)| (n.to_owned(), t.to_owned(), p))
                .collect::<Vec<_>>()
        {
            return Err(invalid("period evidence table set or shape mismatch"));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfficialQuotaInput {
    pub input_id: String,
    pub account_scope: String,
    pub sequence: u64,
    pub observed_at: i64,
    pub content_sha256: String,
    pub reset_at: Option<i64>,
    pub window_seconds: Option<i64>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRef {
    generation: u64,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPayload {
    rule_version: u8,
    partition: StoragePartitionIdentity,
    parent: Option<SnapshotRef>,
    input_revision: i64,
    /// Only interpretations changed since parent. Old bytes remain immutable.
    inputs: Vec<OfficialQuotaInput>,
    ownership: Vec<EventOwnership>,
    previous: Vec<PreviousAttribution>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousAttribution {
    source: crate::SessionEventSource,
    period: Option<SessionEventPeriod>,
    admission: crate::event_admission::EventAdmission,
}

#[derive(Clone, Default)]
pub(super) struct EvidenceCache {
    revision: i64,
    inputs: BTreeMap<String, OfficialQuotaInput>,
    prepared: Option<PreparedPeriodAttribution>,
}

#[derive(Clone)]
pub struct PreparedPeriodAttribution {
    reference: SnapshotRef,
    payload: SnapshotPayload,
    bytes: String,
    timeline: AttributionTimeline,
    changed: bool,
}

fn invalid(message: &str) -> UsageStoreError {
    UsageStoreError::InvalidImport(message.to_owned())
}

fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|_| invalid("period evidence cannot be encoded"))
}

fn digest(bytes: &str) -> String {
    format!("{:x}", Sha256::digest(bytes.as_bytes()))
}

impl PreparedPeriodAttribution {
    pub(crate) fn candidate_period_at(&self, timestamp: i64) -> Option<SessionEventPeriod> {
        let owners = self
            .payload
            .ownership
            .iter()
            .enumerate()
            .filter(|(_, owner)| {
                owner.start_at <= timestamp && owner.end_at.is_none_or(|end| timestamp < end)
            })
            .collect::<Vec<_>>();
        if owners.len() != 1 {
            return None;
        }
        self.timeline
            .attribution_at(timestamp, owners[0].0 as u64)
            .map(|(period, _, _)| SessionEventPeriod {
                reset_at: period.reset_at,
                window_seconds: period.window_seconds,
            })
    }
    pub(crate) fn partition(&self) -> &StoragePartitionIdentity {
        &self.payload.partition
    }
    /// A replay may carry the current official derivation of the same
    /// immutable numeric/source fact. Only this fixed snapshot can establish
    /// that equivalence; callers retain the original admission and let the
    /// attribution owner update any changed membership and its provenance.
    pub(crate) fn is_reassignment_of(
        &self,
        old: &LosslessSessionEvent,
        new: &LosslessSessionEvent,
    ) -> Result<bool> {
        let (Some(before), Some(after)) = (&old.admission, &new.admission) else {
            return Ok(false);
        };
        if old.source != new.source
            || old.timestamp != new.timestamp
            || old.model != new.model
            || old.tokens != new.tokens
        {
            return Ok(false);
        }
        let mut same_source = before.clone();
        same_source.period = after.period.clone();
        if &same_source != after {
            return Ok(false);
        }
        let mut expected = old.clone();
        self.assign(&mut expected)?;
        Ok(expected == *new)
    }
    fn changes_assignment(&self, event: &LosslessSessionEvent) -> Result<bool> {
        let mut proposed = event.clone();
        self.assign(&mut proposed)?;
        let span = |fact: &LosslessSessionEvent| {
            fact.admission
                .as_ref()
                .and_then(|a| a.period.as_ref())
                .map(|p| (p.lower_at, p.upper_at))
        };
        let old_span_survives = match (span(event), span(&proposed)) {
            (None, None) => true,
            (Some((old_lower, old_upper)), Some((new_lower, new_upper))) => {
                new_lower <= old_lower && old_upper <= new_upper
            }
            _ => false,
        };
        Ok(event.period != proposed.period
            || !old_span_survives
            || event.admission.as_ref().map(|a| &a.ownership)
                != proposed.admission.as_ref().map(|a| &a.ownership))
    }
    pub fn assign(&self, event: &mut LosslessSessionEvent) -> Result<()> {
        let admission = event
            .admission
            .as_mut()
            .ok_or_else(|| invalid("event has no source qualification"))?;
        if admission.source_binding.partition != self.payload.partition {
            return Err(invalid("period evidence belongs to another partition"));
        }
        let owners = self
            .payload
            .ownership
            .iter()
            .enumerate()
            .filter(|(_, owner)| {
                owner.start_at == admission.ownership.start_at
                    && match (owner.end_at, admission.ownership.end_at) {
                        (None, Some(_)) => false,
                        (Some(next), Some(old)) => next <= old,
                        _ => true,
                    }
            })
            .collect::<Vec<_>>();
        if owners.len() != 1 {
            return Err(invalid("period evidence ownership is not unique"));
        }
        // Registry closure narrows this same continuous owner. Preserve the
        // old admission in snapshot.previous; never extend a closed proof.
        admission.ownership = owners[0].1.clone();
        let attribution = (event.model != "UNATTRIBUTED")
            .then(|| {
                self.timeline
                    .attribution_at(event.timestamp, owners[0].0 as u64)
            })
            .flatten();
        event.period = attribution.map(|(period, _, _)| SessionEventPeriod {
            reset_at: period.reset_at,
            window_seconds: period.window_seconds,
        });
        admission.period = attribution.map(|(_, lower_at, upper_at)| PeriodAttributionProof {
            rule_version: 1,
            lower_at,
            upper_at,
            evidence_generation: self.reference.generation.to_string(),
            evidence_sha256: self.reference.sha256.clone(),
        });
        if !event
            .admission
            .as_ref()
            .expect("admission retained")
            .valid_for(event, event.period)
        {
            return Err(invalid("attributed event qualification is invalid"));
        }
        Ok(())
    }
}

fn head(connection: &Connection) -> Result<Option<(SnapshotRef, i64)>> {
    Ok(connection
        .query_row(
            "SELECT generation,sha256,input_revision FROM period_evidence_head WHERE singleton=1",
            [],
            |r| {
                Ok((
                    SnapshotRef {
                        generation: r.get(0)?,
                        sha256: r.get(1)?,
                    },
                    r.get(2)?,
                ))
            },
        )
        .optional()?)
}

fn read_payload(
    connection: &Connection,
    reference: &SnapshotRef,
) -> Result<(SnapshotPayload, String)> {
    let (sha, bytes): (String, String) = connection.query_row(
        "SELECT sha256,payload_json FROM period_evidence_snapshots WHERE generation=?1",
        [reference.generation],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if sha != reference.sha256 || digest(&bytes) != sha {
        return Err(invalid("period evidence snapshot hash mismatch"));
    }
    let payload: SnapshotPayload =
        serde_json::from_str(&bytes).map_err(|_| invalid("invalid period evidence snapshot"))?;
    if payload.rule_version != 1 || encode(&payload)? != bytes {
        return Err(invalid("noncanonical period evidence snapshot"));
    }
    Ok((payload, bytes))
}

fn restored_timeline(
    connection: &Connection,
    reference: &SnapshotRef,
) -> Result<AttributionTimeline> {
    let (latest, _) = read_payload(connection, reference)?;
    let mut chain = Vec::new();
    let mut cursor = Some(reference.clone());
    while let Some(current) = cursor {
        let (payload, _) = read_payload(connection, &current)?;
        if payload.partition != latest.partition
            || payload
                .parent
                .as_ref()
                .is_some_and(|parent| parent.generation >= current.generation)
        {
            return Err(invalid("period evidence parent chain is invalid"));
        }
        cursor = payload.parent.clone();
        chain.push(payload);
    }
    let mut inputs = BTreeMap::new();
    for payload in chain.into_iter().rev() {
        for input in payload.inputs {
            inputs.insert(input.input_id.clone(), input);
        }
    }
    build_timeline(inputs.into_values(), &latest)
}

fn build_timeline(
    inputs: impl Iterator<Item = OfficialQuotaInput>,
    payload: &SnapshotPayload,
) -> Result<AttributionTimeline> {
    let mut observations = Vec::new();
    for input in inputs {
        if input.account_scope != payload.partition.account_scope_id {
            return Err(invalid("official input account scope mismatch"));
        }
        let owners = payload
            .ownership
            .iter()
            .enumerate()
            .filter(|(_, owner)| {
                input.observed_at >= owner.start_at
                    && owner.end_at.is_none_or(|end| input.observed_at < end)
            })
            .collect::<Vec<_>>();
        if owners.len() != 1 {
            // Unknown ownership cannot position a barrier or support a period.
            continue;
        }
        observations.push(OfficialObservation {
            lower_at: input.observed_at,
            upper_at: input.observed_at,
            reset_at: input.reset_at,
            window_seconds: input.window_seconds,
            acquisition_sequence: Some(input.sequence),
            ownership_interval: owners[0].0 as u64,
        });
    }
    AttributionTimeline::from_observations(&observations)
        .map_err(|_| invalid("official period evidence is inconsistent"))
}

impl UsageStore {
    /// Read one exact period through the prepared snapshot's virtual repairs.
    /// A pending batch does not mutate saved membership while building totals.
    pub fn session_events_for_prepared_period(
        &self,
        prepared: &PreparedPeriodAttribution,
        reset: i64,
        window: i64,
    ) -> Result<(Vec<crate::SessionEvent>, bool)> {
        let origin = reset
            .checked_sub(window)
            .ok_or_else(|| invalid("invalid exact period"))?;
        let _scope =
            crate::source_qualification_repair::CanonicalSourceScope::begin(&self.connection)?;
        let mut facts = crate::source_events::read_matching(
            &self.connection,
            "WHERE reset_at=?1 AND window_seconds=?2 AND timestamp>=?3 AND timestamp<?1",
            params![reset, window, origin],
        )?;
        for previous in &prepared.payload.previous {
            if let Some(fact) = facts.iter_mut().find(|fact| fact.source == previous.source) {
                prepared.assign(fact)?;
            } else {
                let source = &previous.source;
                let mut changed=crate::source_events::read_matching(&self.connection,
                    "WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 AND range_start=?6 AND range_end=?7 AND record_sha256=?8 AND event_index=?9",
                    params![source.root_identity,source.relative_path,source.file_device.to_string(),source.file_inode.to_string(),format!("{:032x}",source.prefix_generation),source.range_start,source.range_end,source.record_sha256,source.event_index])?;
                if let Some(mut fact) = changed.pop() {
                    prepared.assign(&mut fact)?;
                    facts.push(fact);
                }
            }
        }
        let mut incomplete=self.connection.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {} WHERE timestamp>=?1 AND timestamp<?2 AND admission_json IS NULL)",crate::source_qualification_repair::canonical_table(&self.connection)?),
            params![origin,reset],|r|r.get::<_,bool>(0))?;
        if !incomplete {
            for mut fact in crate::source_events::read_matching(&self.connection,
                "WHERE reset_at IS NULL AND admission_json IS NOT NULL AND timestamp>=?1 AND timestamp<?2",params![origin,reset])? {
                prepared.assign(&mut fact)?;
                if fact.period.is_none() { incomplete=true; break; }
            }
        }
        incomplete |= facts.iter().any(|fact| {
            fact.period.is_none() && fact.timestamp >= origin && fact.timestamp < reset
        });
        let events = facts
            .into_iter()
            .filter(|fact| {
                fact.period
                    == Some(SessionEventPeriod {
                        reset_at: reset,
                        window_seconds: window,
                    })
            })
            .map(crate::SessionEvent::try_from)
            .collect::<Result<Vec<_>>>()?;
        let (events, duplicate) = crate::canonicalize_session_events(events);
        Ok((events, incomplete || duplicate))
    }

    /// Acquisition-owned staging. No usage snapshot, event or public model root
    /// changes here. An interpretation replacement retains the preceding row.
    pub fn stage_official_quota_inputs(&mut self, inputs: &[OfficialQuotaInput]) -> Result<()> {
        if inputs.is_empty() {
            return Ok(());
        }
        let tx = self.connection.transaction()?;
        for input in inputs {
            crate::validate_sha256(&input.input_id, "official input identity")?;
            crate::validate_sha256(&input.content_sha256, "official input content")?;
            if input.sequence == 0
                || input.sequence > i64::MAX as u64
                || !(1..=crate::MAX_PUBLIC_UNIX_SECONDS).contains(&input.observed_at)
                || input.account_scope.is_empty()
                || input.reset_at.is_some() != input.window_seconds.is_some()
                || (input.reset_at.is_some() != input.reason.is_none())
                || input
                    .reset_at
                    .is_some_and(|reset| !(1..=crate::MAX_PUBLIC_UNIX_SECONDS).contains(&reset))
                || input.window_seconds.is_some_and(|window| window <= 0)
            {
                return Err(invalid("invalid official input interpretation"));
            }
            let bytes = encode(input)?;
            let sha = digest(&bytes);
            let previous: Option<(String, String)> = tx.query_row(
                "SELECT interpretation_sha256,interpretation_json FROM official_period_inputs WHERE input_id=?1 ORDER BY revision DESC LIMIT 1",
                [&input.input_id], |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?;
            if previous == Some((sha.clone(), bytes.clone())) {
                continue;
            }
            if let Some((old_sha, old_bytes)) = previous {
                if digest(&old_bytes) != old_sha {
                    return Err(invalid("official input hash mismatch"));
                }
                let old: OfficialQuotaInput = serde_json::from_str(&old_bytes)
                    .map_err(|_| invalid("invalid saved official input"))?;
                if old.input_id != input.input_id
                    || old.account_scope != input.account_scope
                    || old.sequence != input.sequence
                    || old.observed_at != input.observed_at
                    || old.content_sha256 != input.content_sha256
                {
                    return Err(invalid("immutable official input changed"));
                }
            }
            tx.execute("INSERT INTO official_period_inputs(input_id,interpretation_json,interpretation_sha256) VALUES(?1,?2,?3)",
                params![input.input_id, bytes, sha])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Freeze the exact staged input revisions, account ownership and parent.
    /// After activation, only newly staged revisions are read from the ledger.
    pub fn prepare_period_attribution(
        &self,
        partition: &StoragePartitionIdentity,
        intervals: &[SessionLifecycleInterval],
    ) -> Result<PreparedPeriodAttribution> {
        crate::validate_storage_partition_identity(&self.connection, partition)?;
        let ownership = intervals
            .iter()
            .filter_map(|interval| {
                interval.start_at.map(|start_at| EventOwnership {
                    start_at,
                    end_at: interval.end_at,
                })
            })
            .collect::<Vec<_>>();
        let current = head(&self.connection)?;
        let mut cache = self.period_evidence_cache.borrow_mut();
        let mut statement = self.connection.prepare("SELECT revision,interpretation_json,interpretation_sha256 FROM official_period_inputs WHERE revision>?1 ORDER BY revision")?;
        let mut rows = statement.query([cache.revision])?;
        let mut delta = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let revision: i64 = row.get(0)?;
            let bytes: String = row.get(1)?;
            let sha: String = row.get(2)?;
            if digest(&bytes) != sha {
                return Err(invalid("official input hash mismatch"));
            }
            let input: OfficialQuotaInput = serde_json::from_str(&bytes)
                .map_err(|_| invalid("invalid saved official input"))?;
            cache.inputs.insert(input.input_id.clone(), input.clone());
            if current
                .as_ref()
                .is_none_or(|(_, applied)| revision > *applied)
            {
                delta.insert(input.input_id.clone(), input);
            }
            cache.revision = revision;
        }
        // A failed prepare may already have filled the cache. Re-read only the
        // unapplied revision suffix, never the complete acquisition ledger.
        if delta.is_empty()
            && current
                .as_ref()
                .is_none_or(|(_, revision)| *revision < cache.revision)
        {
            let mut statement = self.connection.prepare("SELECT interpretation_json FROM official_period_inputs WHERE revision>?1 ORDER BY revision")?;
            for value in statement.query_map([current.as_ref().map_or(0, |(_, r)| *r)], |r| {
                r.get::<_, String>(0)
            })? {
                let input: OfficialQuotaInput = serde_json::from_str(&value?)
                    .map_err(|_| invalid("invalid saved official input"))?;
                delta.insert(input.input_id.clone(), input);
            }
        }
        let previous = current
            .as_ref()
            .map(|(reference, _)| read_payload(&self.connection, reference))
            .transpose()?;
        let changed = previous
            .as_ref()
            .is_none_or(|(payload, _)| !delta.is_empty() || payload.ownership != ownership);
        let (reference, payload, bytes) = if !changed {
            let (payload, bytes) = previous.expect("unchanged snapshot exists");
            (current.expect("unchanged head exists").0, payload, bytes)
        } else {
            let generation = current.as_ref().map_or(Ok(1), |(r, _)| {
                r.generation
                    .checked_add(1)
                    .ok_or(UsageStoreError::GenerationOverflow)
            })?;
            let mut payload = SnapshotPayload {
                rule_version: 1,
                partition: partition.clone(),
                parent: current.as_ref().map(|(r, _)| r.clone()),
                input_revision: cache.revision,
                inputs: delta.into_values().collect(),
                ownership,
                previous: Vec::new(),
            };
            let timeline = build_timeline(cache.inputs.values().cloned(), &payload)?;
            let old = match current.as_ref() {
                Some((reference, _)) => match cache
                    .prepared
                    .as_ref()
                    .filter(|last| &last.reference == reference)
                {
                    Some(last) => last.timeline.clone(),
                    None => restored_timeline(&self.connection, reference)?,
                },
                None => {
                    AttributionTimeline::from_observations(&[]).expect("empty timeline is valid")
                }
            };
            let provisional = PreparedPeriodAttribution {
                reference: SnapshotRef {
                    generation,
                    sha256: "0".repeat(64),
                },
                payload: payload.clone(),
                bytes: String::new(),
                timeline,
                changed: true,
            };
            let mut changed_ranges = provisional.timeline.changed_ranges(&old);
            if let Some((previous, _)) = previous.as_ref() {
                for owner in &previous.ownership {
                    if !provisional.payload.ownership.contains(owner) {
                        changed_ranges.push((
                            owner.start_at,
                            owner.end_at.map_or(crate::MAX_PUBLIC_UNIX_SECONDS, |end| {
                                end.saturating_sub(1)
                            }),
                        ));
                    }
                }
            }
            let mut candidates = BTreeMap::new();
            if !changed_ranges.is_empty() {
                let _scope = crate::source_qualification_repair::CanonicalSourceScope::begin(
                    &self.connection,
                )?;
                for (lower, upper) in changed_ranges {
                    for fact in crate::source_events::read_matching(
                        &self.connection,
                        "WHERE timestamp>=?1 AND timestamp<=?2 AND admission_json IS NOT NULL",
                        params![lower, upper],
                    )? {
                        candidates.insert(encode(&fact.source)?, fact);
                    }
                }
            }
            for fact in candidates.into_values() {
                if provisional.changes_assignment(&fact)? {
                    payload.previous.push(PreviousAttribution {
                        source: fact.source,
                        period: fact.period,
                        admission: fact.admission.expect("qualified selection"),
                    });
                }
            }
            let bytes = encode(&payload)?;
            (
                SnapshotRef {
                    generation,
                    sha256: digest(&bytes),
                },
                payload,
                bytes,
            )
        };
        let timeline = if let Some(cached) = cache
            .prepared
            .as_ref()
            .filter(|last| last.reference == reference)
        {
            cached.timeline.clone()
        } else {
            build_timeline(cache.inputs.values().cloned(), &payload)?
        };
        let prepared = PreparedPeriodAttribution {
            reference,
            payload,
            bytes,
            timeline,
            changed,
        };
        cache.prepared = Some(prepared.clone());
        Ok(prepared)
    }
}

pub(super) fn commit_snapshot(
    tx: &Transaction<'_>,
    prepared: &PreparedPeriodAttribution,
) -> Result<()> {
    crate::validate_storage_partition_identity(tx, &prepared.payload.partition)?;
    let current = head(tx)?.map(|(r, _)| r);
    if current.as_ref() == Some(&prepared.reference) {
        if read_payload(tx, &prepared.reference)?.1 != prepared.bytes {
            return Err(invalid("period evidence replay changed"));
        }
        return Ok(());
    }
    if !prepared.changed
        || current != prepared.payload.parent
        || digest(&prepared.bytes) != prepared.reference.sha256
    {
        return Err(invalid("period evidence parent changed"));
    }
    tx.execute(
        "INSERT INTO period_evidence_snapshots(generation,sha256,payload_json) VALUES(?1,?2,?3)",
        params![
            prepared.reference.generation,
            prepared.reference.sha256,
            prepared.bytes
        ],
    )?;
    tx.execute("INSERT INTO period_evidence_head(singleton,generation,sha256,input_revision) VALUES(1,?1,?2,?3) ON CONFLICT(singleton) DO UPDATE SET generation=excluded.generation,sha256=excluded.sha256,input_revision=excluded.input_revision",
        params![prepared.reference.generation, prepared.reference.sha256, prepared.payload.input_revision])?;
    Ok(())
}

/// Reassign only the indexed event set fixed by prepare. The old admission is
/// retained in the immutable snapshot, while original numeric rows keep their
/// source key, raw usage, model and timestamp.
pub(super) fn apply_reassignments(
    tx: &Transaction<'_>,
    prepared: &PreparedPeriodAttribution,
) -> Result<BTreeMap<(i64, i64), i64>> {
    let mut periods = BTreeMap::new();
    for previous in &prepared.payload.previous {
        let source = &previous.source;
        let values = crate::source_events::read_matching(tx,
            "WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 AND range_start=?6 AND range_end=?7 AND record_sha256=?8 AND event_index=?9",
            params![source.root_identity,source.relative_path,source.file_device.to_string(),source.file_inode.to_string(),format!("{:032x}",source.prefix_generation),source.range_start,source.range_end,source.record_sha256,source.event_index])?;
        let fact = values
            .into_iter()
            .next()
            .ok_or_else(|| invalid("reassigned event disappeared"))?;
        let mut candidate = fact.clone();
        prepared.assign(&mut candidate)?;
        if fact == candidate {
            continue;
        }
        if fact.period != previous.period || fact.admission.as_ref() != Some(&previous.admission) {
            return Err(invalid("saved event changed before attribution commit"));
        }
        tx.execute("UPDATE session_events SET reset_at=?10,window_seconds=?11,admission_json=?12
             WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 AND range_start=?6 AND range_end=?7 AND record_sha256=?8 AND event_index=?9",
            params![source.root_identity,source.relative_path,source.file_device.to_string(),source.file_inode.to_string(),format!("{:032x}",source.prefix_generation),source.range_start,source.range_end,source.record_sha256,source.event_index,
                candidate.period.map(|p|p.reset_at),candidate.period.map(|p|p.window_seconds),encode(candidate.admission.as_ref().expect("admission retained"))?])?;
        for period in [fact.period, candidate.period].into_iter().flatten() {
            periods
                .entry((period.reset_at, period.window_seconds))
                .and_modify(|since: &mut i64| *since = (*since).min(fact.timestamp))
                .or_insert(fact.timestamp);
        }
    }
    if prepared.changed {
        let selected = prepared
            .payload
            .inputs
            .iter()
            .filter_map(|input| input.reset_at.zip(input.window_seconds))
            .collect::<std::collections::BTreeSet<_>>();
        for (reset, window) in selected {
            let Some(origin) = reset.checked_sub(window) else {
                continue;
            };
            let unknown:Option<i64>=tx.query_row(&format!("SELECT min(timestamp) FROM {} WHERE timestamp>=?1 AND timestamp<?2 AND admission_json IS NULL",crate::source_qualification_repair::canonical_table(tx)?),params![origin,reset],|row|row.get(0))?;
            let Some(unknown) = unknown else {
                continue;
            };
            // A still-valid event proof need not change merely because the
            // run grew. Its period can nevertheless contain an OLD complete
            // coverage claim contradicted by a retained unqualified source.
            // Repair that quality once, without revisiting old numeric proofs.
            let unsafe_complete:Option<i64>=tx.query_row("SELECT min(timestamp) FROM usage_model_history WHERE reset_at=?1 AND timestamp>=?2 AND model_set_complete=1",params![reset,unknown.div_euclid(60)*60],|row|row.get(0))?;
            if let Some(since) = unsafe_complete {
                periods
                    .entry((reset, window))
                    .and_modify(|old| *old = (*old).min(since))
                    .or_insert(since);
            }
        }
    }
    Ok(periods)
}

/// Rebuild both old and new suffixes from exact persisted membership. No
/// negative token event or transfer of a different period's saved price occurs.
pub(super) struct OriginalProjectionPrice {
    models: BTreeMap<String, crate::SavedModelPrice>,
    fixed: [Option<f64>; 3],
    has_fixed_tokens: bool,
}

pub(super) type OriginalProjectionPrices =
    BTreeMap<(i64, i64), BTreeMap<i64, OriginalProjectionPrice>>;

pub(super) fn capture_projection_prices(
    tx: &Transaction<'_>,
    periods: &BTreeMap<(i64, i64), i64>,
) -> Result<OriginalProjectionPrices> {
    let mut saved = BTreeMap::new();
    for (&period, &since) in periods {
        let mut originals = BTreeMap::new();
        let mut statement=tx.prepare("SELECT timestamp,sol_dollars,terra_dollars,luna_dollars,(sol_tokens IS NOT NULL OR terra_tokens IS NOT NULL OR luna_tokens IS NOT NULL) FROM usage_history WHERE reset_at=?1 AND timestamp>=?2 ORDER BY timestamp")?;
        let rows = statement
            .query_map(params![period.0, since.div_euclid(60) * 60], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    [r.get(1)?, r.get(2)?, r.get(3)?],
                    r.get::<_, bool>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (timestamp, fixed, has_fixed_tokens) in rows {
            originals.insert(
                timestamp,
                OriginalProjectionPrice {
                    models: crate::load_saved_model_prices(tx, period.0, timestamp)?,
                    fixed,
                    has_fixed_tokens,
                },
            );
        }
        saved.insert(period, originals);
    }
    Ok(saved)
}

fn same_saved_usage(total: &crate::SessionModelTotal, saved: &crate::SavedModelPrice) -> bool {
    saved.total_tokens == total.total_tokens.to_string()
        && saved.input_tokens == total.input_tokens.map(|value| value.to_string())
        && saved.cached_input_tokens == total.cached_input_tokens.map(|value| value.to_string())
        && saved.cache_write_input_tokens
            == total
                .cache_write_input_tokens
                .map(|value| value.to_string())
        && saved.output_tokens == total.output_tokens.map(|value| value.to_string())
}

pub(super) fn reproject_periods(
    tx: &Transaction<'_>,
    periods: &BTreeMap<(i64, i64), i64>,
    original_prices: &OriginalProjectionPrices,
) -> Result<()> {
    use crate::{ModelSource, SessionEvent, UsageHistoryObservation};
    use std::collections::BTreeSet;
    let canonical_scope = crate::source_qualification_repair::CanonicalSourceScope::begin(tx)?;
    let retained_offsets = crate::load_session_timeline_recoveries_from_connection(tx)?;
    for (&(reset, window), &since) in periods {
        let origin = reset
            .checked_sub(window)
            .ok_or_else(|| invalid("invalid attributed period"))?;
        let retained_end = retained_offsets
            .iter()
            .filter(|(offset, _, _)| {
                offset.canonical_reset_at == reset && offset.window_seconds == window
            })
            .map(|(offset, _, _)| offset.projection_end_exclusive)
            .max();
        if let Some(end) = retained_end {
            crate::mark_retained_history_models_unknown(tx, reset, end)?;
        }
        let events = crate::source_events::read_matching(
            tx,
            "WHERE reset_at=?1 AND window_seconds=?2 AND timestamp>=?3 AND timestamp<?1",
            params![reset, window, origin],
        )?
        .into_iter()
        .map(SessionEvent::try_from)
        .collect::<Result<Vec<_>>>()?;
        let mut targets = BTreeSet::new();
        let mut statement=tx.prepare("SELECT timestamp FROM usage_history WHERE reset_at=?1 AND timestamp>=?2 AND timestamp<?3 ORDER BY timestamp")?;
        for value in statement.query_map(params![reset, since.div_euclid(60) * 60, reset], |r| {
            r.get::<_, i64>(0)
        })? {
            targets.insert(value?);
        }
        targets.extend(
            events
                .iter()
                .filter(|event| event.timestamp >= since)
                .map(|event| event.timestamp.div_euclid(60) * 60),
        );
        let mut receipt_query=tx.prepare("SELECT observed_at,remaining_percent FROM quota_receipts WHERE reset_at=?1 AND window_seconds=?2 AND observed_at>=?3 AND observed_at<?1 ORDER BY sequence")?;
        let receipts = receipt_query
            .query_map(params![reset, window, origin], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut receipt_minutes = BTreeMap::new();
        for (observed, remaining) in receipts {
            let minute = observed.div_euclid(60) * 60;
            if minute >= since.div_euclid(60) * 60 {
                targets.insert(minute);
                receipt_minutes.insert(minute, (observed, remaining));
            }
        }
        let has_unassigned:bool=tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {} WHERE timestamp>=?1 AND timestamp<?2 AND reset_at IS NULL)",crate::source_qualification_repair::canonical_table(tx)?),params![origin,reset],|r|r.get(0))?;
        let first_unqualified:Option<i64>=tx.query_row(&format!("SELECT min(timestamp) FROM {} WHERE timestamp>=?1 AND timestamp<?2 AND admission_json IS NULL",crate::source_qualification_repair::canonical_table(tx)?),params![origin,reset],|row|row.get(0))?;
        let mut prefix = Vec::new();
        let mut cursor = 0;
        for timestamp in targets {
            // This prefix still has an unretired legacy offset. An exact
            // proof for a later suffix cannot replace its saved numerics or
            // create earlier observations from the partial source set.
            if retained_end.is_some_and(|end| timestamp < end) {
                continue;
            }
            while cursor < events.len()
                && events[cursor].timestamp <= timestamp.saturating_add(59).min(reset - 1)
            {
                prefix = crate::checked_add_model_totals(
                    &prefix,
                    &[crate::event_model_total(&events[cursor])],
                )
                .ok_or(UsageStoreError::GenerationOverflow)?;
                cursor += 1;
            }
            if canonical_scope.preserves_observation(reset, timestamp) {
                continue;
            }
            let previous:Option<(i64,String,String)>=tx.query_row("SELECT data_generation,data_hash,snapshot_json FROM durable_state WHERE singleton>=2 AND data_hash=?1",
                [crate::observation_data_hash(reset,timestamp)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let source_timestamp = previous
                .as_ref()
                .map(|(_, _, json)| crate::observation_source_timestamp(json))
                .transpose()?
                .or_else(|| receipt_minutes.get(&timestamp).map(|(at, _)| *at))
                .or_else(|| {
                    events[..cursor]
                        .last()
                        .filter(|event| event.timestamp >= timestamp)
                        .map(|event| event.timestamp)
                });
            let preserved_source_timestamp = previous
                .as_ref()
                .map(|(_, _, json)| {
                    serde_json::from_str::<serde_json::Value>(json)
                        .map(|value| {
                            value
                                .get(crate::OBSERVATION_SOURCE_TIMESTAMP_KEY)
                                .and_then(serde_json::Value::as_i64)
                        })
                        .map_err(|_| invalid("saved observation metadata is invalid"))
                })
                .transpose()?
                .flatten();
            let previous = previous
                .map(|(at, hash, json)| crate::observation_from_sql(at, hash, json))
                .transpose()?;
            let remaining = crate::quota_receipts::remaining_for_exact_minute(tx, reset, timestamp)?
                .or(tx.query_row("SELECT remaining_percent FROM usage_history WHERE reset_at=?1 AND timestamp=?2",params![reset,timestamp],|r|r.get::<_,Option<f64>>(0)).optional()?.flatten());
            let current_saved = crate::load_saved_model_prices(tx, reset, timestamp)?;
            let original_period = original_prices.get(&(reset, window));
            let original = original_period.and_then(|period| period.get(&timestamp));
            if (original.is_some_and(|old| {
                !old.models.is_empty()
                    || old.has_fixed_tokens
                    || old.fixed.iter().any(Option::is_some)
            }) || !current_saved.is_empty())
                && first_unqualified
                    .is_some_and(|at| at <= timestamp.saturating_add(59).min(reset - 1))
            {
                // A proof for one source cannot disprove saved contributions
                // from another source whose original qualification is absent.
                // This includes an independently retained absolute value just
                // written by this transaction, after the original capture.
                // Preserve every numeric/price cell until that prefix closes.
                tx.execute("UPDATE usage_model_history SET model_set_complete=0 WHERE reset_at=?1 AND timestamp=?2 AND model_set_complete<>0",params![reset,timestamp])?;
                if let Some(mut old) = previous {
                    if old.model_source != ModelSource::LegacyUnknown || old.model_totals_complete {
                        old.model_source = ModelSource::LegacyUnknown;
                        old.model_totals_complete = false;
                        let json = crate::observation_json_with_source_timestamp(
                            tx,
                            &old,
                            preserved_source_timestamp,
                        )?;
                        tx.execute("UPDATE durable_state SET snapshot_json=?1 WHERE singleton>=2 AND data_hash=?2",params![json,crate::observation_data_hash(reset,timestamp)])?;
                    }
                }
                continue;
            }
            let no_prices = BTreeMap::new();
            let saved = match original_period {
                Some(_) => original.map_or(&no_prices, |row| &row.models),
                None => &current_saved,
            };
            let prices = prefix
                .iter()
                .map(|total| {
                    let price = crate::model_history_price_for_lifecycle_repair(
                        total,
                        timestamp,
                        saved.get(&total.model),
                    );
                    (total.model.clone(), price)
                })
                .collect::<BTreeMap<_, _>>();
            let complete = previous
                .as_ref()
                .is_some_and(|old| old.model_totals_complete)
                && retained_end.is_none()
                && !has_unassigned
                && !prefix.is_empty();
            let family_value = |name| -> Result<(Option<u64>, Option<f64>)> {
                let models = prefix
                    .iter()
                    .filter(|total| codex_info_pricing::family(&total.model) == Some(name))
                    .collect::<Vec<_>>();
                if models.is_empty() {
                    return Ok(if complete {
                        (Some(0), Some(0.0))
                    } else {
                        (None, None)
                    });
                }
                let total = models
                    .iter()
                    .try_fold(0_u64, |sum, value| sum.checked_add(value.total_tokens))
                    .ok_or(UsageStoreError::GenerationOverflow)?;
                if let Some(original) = original {
                    let old = original
                        .models
                        .iter()
                        .filter(|(model, _)| codex_info_pricing::family(model) == Some(name))
                        .collect::<Vec<_>>();
                    if old.len() == models.len()
                        && models.iter().all(|model| {
                            original
                                .models
                                .get(&model.model)
                                .is_some_and(|saved| same_saved_usage(model, saved))
                        })
                    {
                        let index = match name {
                            "SOL" => 0,
                            "TERRA" => 1,
                            _ => 2,
                        };
                        return Ok((Some(total), original.fixed[index]));
                    }
                }
                // Fixed family columns use the compatibility tariff, after
                // summing the integer family components. Exact model prices
                // (and their independent unknownness) remain in the sidecar.
                let component = |get: fn(&crate::SessionModelTotal) -> Option<u64>| {
                    models
                        .iter()
                        .try_fold(0_u64, |sum, value| sum.checked_add(get(value)?))
                };
                let dollars = component(|value| value.input_tokens)
                    .zip(component(|value| value.cached_input_tokens))
                    .zip(component(|value| value.output_tokens))
                    .and_then(|((input, cached), output)| {
                        codex_info_pricing::estimate_legacy(
                            name,
                            codex_info_pricing::Usage {
                                input_tokens: input,
                                cached_input_tokens: cached,
                                cache_write_input_tokens: Some(0),
                                output_tokens: output,
                            },
                        )
                    })
                    .map(|cost| cost.total_dollars);
                Ok((Some(total), dollars))
            };
            let (sol_tokens, sol_dollars) = family_value("SOL")?;
            let (terra_tokens, terra_dollars) = family_value("TERRA")?;
            let (luna_tokens, luna_dollars) = family_value("LUNA")?;
            let observation = UsageHistoryObservation {
                timestamp,
                reset_at: reset,
                remaining_percent: remaining,
                sol_dollars,
                terra_dollars,
                luna_dollars,
                sol_tokens,
                terra_tokens,
                luna_tokens,
                model_source: ModelSource::Confirmed,
                model_totals: Some(prefix.clone()),
                model_totals_complete: complete,
            };
            tx.execute(
                crate::UPSERT_SAMPLE,
                params![
                    timestamp,
                    reset,
                    remaining,
                    sol_dollars,
                    terra_dollars,
                    luna_dollars,
                    sol_tokens,
                    terra_tokens,
                    luna_tokens
                ],
            )?;
            // The sidecar helper preserves same-prefix prices; replace its
            // computed values with the same-period saved-price qualification.
            crate::upsert_observation_model_totals(
                tx,
                std::slice::from_ref(&observation),
                false,
                None,
            )?;
            let protected_models = canonical_scope.preserved_models(reset, timestamp);
            for (model, (dollars, version)) in prices {
                if protected_models.contains(&model) {
                    continue;
                }
                tx.execute("UPDATE usage_model_history SET total_dollars=?1,price_version=?2 WHERE reset_at=?3 AND timestamp=?4 AND model=?5",params![dollars,version,reset,timestamp,model])?;
            }
            let timestamps = source_timestamp
                .map(|at| BTreeMap::from([((reset, timestamp), at)]))
                .unwrap_or_default();
            crate::upsert_observations_with_sources(tx, &[observation], &timestamps)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_fixes_pending_inputs_replays_without_writes_and_rejects_parent_tampering() {
        let root =
            std::env::temp_dir().join(format!("codex-info-period-snapshot-{}", std::process::id()));
        let path = root.join("usage.sqlite3");
        let partition = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "a".repeat(64),
            storage_epoch: 7,
            partition_id: "a".repeat(64),
        };
        let mut store = UsageStore::create_partitioned(&path, &partition).unwrap();
        let input = |sequence, observed_at, reset_at| OfficialQuotaInput {
            input_id: format!("{sequence:064x}"),
            account_scope: partition.account_scope_id.clone(),
            sequence,
            observed_at,
            content_sha256: format!("{sequence:064x}"),
            reset_at,
            window_seconds: reset_at.map(|_| 500),
            reason: reset_at.is_none().then(|| "unrecognized-input".into()),
        };
        let ownership = [SessionLifecycleInterval::new(Some(500), None)];
        store
            .stage_official_quota_inputs(&[
                input(1, 600, Some(1000)),
                input(3, 700, Some(1000)),
                input(4, 750, Some(1000)),
            ])
            .unwrap();
        let pending = store
            .prepare_period_attribution(&partition, &ownership)
            .unwrap();
        let original_bytes = pending.bytes.clone();
        let expected = crate::period_attribution::OfficialPeriod {
            reset_at: 1000,
            window_seconds: 500,
        };
        assert_eq!(pending.timeline.period_at(650, 0), Some(expected));
        // Acquisition advances while the usage candidate stays fixed.
        store
            .stage_official_quota_inputs(&[input(2, 650, None)])
            .unwrap();
        let tx = store.connection.transaction().unwrap();
        commit_snapshot(&tx, &pending).unwrap();
        tx.commit().unwrap();
        assert_eq!(head(&store.connection).unwrap().unwrap().1, 3);
        assert_eq!(
            read_payload(&store.connection, &pending.reference)
                .unwrap()
                .1,
            original_bytes
        );
        let next = store
            .prepare_period_attribution(&partition, &ownership)
            .unwrap();
        for (at, period) in [
            (599, None),
            (600, Some(expected)),
            (625, None),
            (650, None),
            (675, None),
            (700, Some(expected)),
            (725, Some(expected)),
            (750, Some(expected)),
            (751, None),
        ] {
            assert_eq!(
                next.timeline.period_at(at, 0),
                period,
                "unknown barrier at {at}"
            );
        }
        {
            let tx = store.connection.transaction().unwrap();
            commit_snapshot(&tx, &next).unwrap();
            // Rollback leaves the pending parent and original bytes intact.
        }
        assert_eq!(
            head(&store.connection).unwrap().unwrap().0,
            pending.reference
        );
        let tx = store.connection.transaction().unwrap();
        commit_snapshot(&tx, &next).unwrap();
        tx.commit().unwrap();
        let before = store.connection.total_changes();
        let unchanged = store
            .prepare_period_attribution(&partition, &ownership)
            .unwrap();
        let tx = store.connection.transaction().unwrap();
        commit_snapshot(&tx, &unchanged).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            store.connection.total_changes(),
            before,
            "same evidence is a SQL no-op"
        );
        assert_eq!(
            read_payload(&store.connection, &pending.reference)
                .unwrap()
                .1,
            original_bytes
        );
        let tx = store.connection.transaction().unwrap();
        assert!(
            commit_snapshot(&tx, &pending).is_err(),
            "a stale parent cannot become current"
        );
        drop(tx);
        store.connection.execute("UPDATE period_evidence_snapshots SET payload_json=payload_json||' ' WHERE generation=1",[]).unwrap();
        assert!(
            restored_timeline(&store.connection, &next.reference).is_err(),
            "old bytes in the parent chain must still hash exactly"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}

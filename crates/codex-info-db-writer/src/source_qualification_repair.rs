//! Sealed, source-verified accepted-prefix repairs committed with usage.

use super::*;
#[path = "source_equivalence.rs"]
mod equivalence;
pub(super) use equivalence::{canonical_table, is_retired, CanonicalSourceScope};
pub use equivalence::{CanonicalSourceSnapshot, SourceEquivalenceMode, SourceEquivalencePrefix};

#[derive(Clone)]
pub struct PreparedSourceQualificationRepair {
    checkpoint: SessionCheckpoint,
    verification_checkpoint: SessionCheckpoint,
    qualified_events: Vec<LosslessSessionEvent>,
    original_events: Vec<LosslessSessionEvent>,
    input_fingerprint: String,
    backup: VerifiedPartitionBackup,
    retirements: Vec<SourceCounterRetirement>,
    provenance: String,
    repair_id: String,
    calculation_inputs: std::sync::Arc<CalculationInputs>,
    numerical_replay: bool,
    replay_pending: Vec<SessionPendingRange>,
    read_container: Option<AcceptedSourceReadContainer>,
    equivalence: Option<equivalence::PreparedEquivalence>,
}

/// A different accepted physical file used only to verify the old bytes.
/// Its identity never replaces the original checkpoint or event key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedSourceReadContainer {
    pub root_identity: String,
    pub relative_path: String,
    pub file_device: u64,
    pub file_inode: u64,
    pub accepted_end: u64,
    pub ranges: Vec<SessionRange>,
}

impl AcceptedSourceReadContainer {
    pub fn covers(&self, checkpoint: &SessionCheckpoint) -> bool {
        if self.root_identity != checkpoint.root_identity
            || self.relative_path != checkpoint.relative_path
            || (self.file_device, self.file_inode)
                == (checkpoint.file_device, checkpoint.file_inode)
            || self.accepted_end != checkpoint.committed_offset
            || self.ranges.is_empty()
            || self.accepted_end == 0
        {
            return false;
        }
        let generation = self.ranges[0].prefix_generation;
        let mut end = 0;
        for range in &self.ranges {
            if range.root_identity != self.root_identity
                || range.relative_path != self.relative_path
                || (range.file_device, range.file_inode) != (self.file_device, self.file_inode)
                || range.prefix_generation != generation
                || range.start_offset > end
                || range.start_offset >= range.end_offset
                || range.start_offset >= self.accepted_end
            {
                return false;
            }
            end = end.max(range.end_offset);
        }
        end >= self.accepted_end
    }
}

#[derive(Clone)]
pub struct SourceNumericalReplay {
    pub checkpoint: SessionCheckpoint,
    pub records: Vec<serde_json::Value>,
    pub pending: Vec<SessionPendingRange>,
}

/// One fixed calculation snapshot and verified backup shared by all source
/// repairs prepared before the same usage transaction.
#[derive(Clone)]
pub struct PreparedSourceRepairInputs {
    backup: VerifiedPartitionBackup,
    calculation_inputs: std::sync::Arc<CalculationInputs>,
}

impl PreparedSourceQualificationRepair {
    pub fn verification_checkpoints(&self) -> Vec<&SessionCheckpoint> {
        match &self.equivalence {
            Some(proof) => proof
                .prefixes
                .iter()
                .map(|prefix| &prefix.verification_checkpoint)
                .collect(),
            None => vec![&self.verification_checkpoint],
        }
    }
    pub fn checkpoint(&self) -> &SessionCheckpoint {
        &self.checkpoint
    }
    pub fn verification_checkpoint(&self) -> &SessionCheckpoint {
        &self.verification_checkpoint
    }
    pub fn qualified_events(&self) -> &[LosslessSessionEvent] {
        &self.qualified_events
    }
    pub fn read_container(&self) -> Option<&AcceptedSourceReadContainer> {
        self.read_container.as_ref()
    }
}

fn container_ranges(
    connection: &Connection,
    container: &AcceptedSourceReadContainer,
) -> Result<Vec<SessionRange>> {
    let generation = container
        .ranges
        .first()
        .ok_or_else(|| UsageStoreError::InvalidImport("empty reading container".into()))?
        .prefix_generation;
    let mut query = connection.prepare("SELECT root_identity,relative_path,file_device,file_inode,start_offset,end_offset,collector_epoch,cycle_seq,prefix_generation,record_sha256 FROM session_ranges WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 AND start_offset<?6 ORDER BY start_offset,end_offset,record_sha256")?;
    let rows = query
        .query_map(
            params![
                container.root_identity,
                container.relative_path,
                container.file_device.to_string(),
                container.file_inode.to_string(),
                format!("{generation:032x}"),
                container.accepted_end
            ],
            SessionRange::from_sql_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for range in &rows {
        validate_session_range(range)?;
    }
    Ok(rows)
}

impl UsageStore {
    pub fn accepted_source_read_containers(
        &self,
        checkpoint: &SessionCheckpoint,
        device: u64,
        inode: u64,
    ) -> Result<Vec<AcceptedSourceReadContainer>> {
        validate_session_checkpoint(checkpoint)?;
        let mut query = self.connection.prepare("SELECT root_identity,relative_path,file_device,file_inode,start_offset,end_offset,collector_epoch,cycle_seq,prefix_generation,record_sha256 FROM session_ranges WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND start_offset<?5 ORDER BY prefix_generation,start_offset,end_offset,record_sha256")?;
        let rows = query
            .query_map(
                params![
                    checkpoint.root_identity,
                    checkpoint.relative_path,
                    device.to_string(),
                    inode.to_string(),
                    checkpoint.committed_offset
                ],
                SessionRange::from_sql_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut groups = BTreeMap::<u128, Vec<SessionRange>>::new();
        for range in rows {
            validate_session_range(&range)?;
            groups
                .entry(range.prefix_generation)
                .or_default()
                .push(range);
        }
        Ok(groups
            .into_values()
            .filter_map(|ranges| {
                let value = AcceptedSourceReadContainer {
                    root_identity: checkpoint.root_identity.clone(),
                    relative_path: checkpoint.relative_path.clone(),
                    file_device: device,
                    file_inode: inode,
                    accepted_end: checkpoint.committed_offset,
                    ranges,
                };
                value.covers(checkpoint).then_some(value)
            })
            .collect())
    }

    pub fn attach_source_read_container(
        &self,
        mut repair: PreparedSourceQualificationRepair,
        container: AcceptedSourceReadContainer,
    ) -> Result<PreparedSourceQualificationRepair> {
        if !container.covers(&repair.checkpoint)
            || container_ranges(&self.connection, &container)? != container.ranges
        {
            return Err(UsageStoreError::InvalidImport(
                "reading container accepted ranges changed".into(),
            ));
        }
        let mut payload: serde_json::Value =
            serde_json::from_str(&repair.provenance).map_err(|_| {
                UsageStoreError::InvalidImport("invalid source repair provenance".into())
            })?;
        payload["read_container"] = serde_json::json!({"root_identity":container.root_identity,"relative_path":container.relative_path,"file_device":container.file_device,"file_inode":container.file_inode,"accepted_end":container.accepted_end,"ranges":container.ranges.iter().map(|range|serde_json::json!({"start":range.start_offset,"end":range.end_offset,"record_sha256":range.record_sha256,"prefix_generation":format!("{:032x}",range.prefix_generation),"collector_epoch":format!("{:032x}",range.collector_epoch),"cycle_seq":range.cycle_seq})).collect::<Vec<_>>()});
        repair.provenance = serde_json::to_string(&payload).map_err(|_| {
            UsageStoreError::InvalidImport("reading container cannot be encoded".into())
        })?;
        repair.repair_id = format!("{:x}", Sha256::digest(repair.provenance.as_bytes()));
        repair.read_container = Some(container);
        Ok(repair)
    }
}

/// Only pre-existing model/price cells are fixed. Independent quota writes
/// may change percentages or append new rows while the usage batch is pending.
#[derive(Clone)]
struct CalculationInputs {
    fixed: Vec<(i64, i64, Vec<Value>)>,
    models: Vec<(i64, i64, String, Vec<Value>)>,
}

fn same_values(left: &[Value], right: &[Value]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| match (left, right) {
                (Value::Real(a), Value::Real(b)) => a.to_bits() == b.to_bits(),
                _ => left == right,
            })
}

const FIXED_COLUMNS: &str =
    "sol_dollars,terra_dollars,luna_dollars,sol_tokens,terra_tokens,luna_tokens";

impl CalculationInputs {
    fn capture(connection: &Connection) -> Result<Self> {
        let fixed=connection.prepare(&format!("SELECT reset_at,timestamp,{FIXED_COLUMNS} FROM usage_history ORDER BY reset_at,timestamp"))?.query_map([],|row|Ok((row.get(0)?,row.get(1)?,(2..8).map(|i|row.get(i)).collect::<rusqlite::Result<Vec<Value>>>()?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut query = connection
            .prepare("SELECT * FROM usage_model_history ORDER BY reset_at,timestamp,model")?;
        let count = query.column_count();
        let models = query
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    (0..count)
                        .map(|i| row.get(i))
                        .collect::<rusqlite::Result<Vec<Value>>>()?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Self { fixed, models })
    }
    fn matches(&self, connection: &Connection) -> Result<bool> {
        for (reset, time, values) in &self.fixed {
            let actual=connection.query_row(&format!("SELECT {FIXED_COLUMNS} FROM usage_history WHERE reset_at=?1 AND timestamp=?2"),params![reset,time],|row|(0..6).map(|i|row.get(i)).collect::<rusqlite::Result<Vec<Value>>>()).optional()?;
            if !actual
                .as_ref()
                .is_some_and(|actual| same_values(actual, values))
            {
                return Ok(false);
            }
        }
        let mut query = connection.prepare(
            "SELECT * FROM usage_model_history WHERE reset_at=?1 AND timestamp=?2 AND model=?3",
        )?;
        let count = query.column_count();
        for (reset, time, model, values) in &self.models {
            let actual = query
                .query_row(params![reset, time, model], |row| {
                    (0..count)
                        .map(|i| row.get(i))
                        .collect::<rusqlite::Result<Vec<Value>>>()
                })
                .optional()?;
            if !actual
                .as_ref()
                .is_some_and(|actual| same_values(actual, values))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Clone)]
pub struct SourceCounterRetirement {
    pub original: LosslessSessionEvent,
    pub replacement: LosslessSessionEvent,
    pub counter: event_admission::LegacyCounterProof,
}

pub(super) const SCHEMA:&str="CREATE TABLE IF NOT EXISTS source_prefix_repairs (repair_id TEXT PRIMARY KEY,payload_json TEXT NOT NULL,payload_sha256 TEXT NOT NULL) WITHOUT ROWID;";

pub(super) const CANDIDATE_INDEX_SCHEMA: &str = "CREATE INDEX IF NOT EXISTS source_prefix_repairs_v2_candidates ON source_prefix_repairs(repair_id) WHERE substr(CAST(payload_json AS BLOB),-13) <> CAST(',\"version\":1}' AS BLOB)";

pub(super) fn validate_schema(connection: &Connection) -> Result<()> {
    let actual = connection
        .prepare(
            "SELECT name,type,pk,\"notnull\" FROM pragma_table_info('source_prefix_repairs') ORDER BY cid",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let expected = vec![
        ("repair_id".into(), "TEXT".into(), 1, 1),
        ("payload_json".into(), "TEXT".into(), 0, 1),
        ("payload_sha256".into(), "TEXT".into(), 0, 1),
    ];
    let without_rowid: i64 = connection.query_row(
        "SELECT wr FROM pragma_table_list WHERE schema='main' AND name='source_prefix_repairs'",
        [],
        |row| row.get(0),
    )?;
    if actual != expected || without_rowid != 1 {
        return Err(UsageStoreError::InvalidImport(
            "source repair provenance shape mismatch".into(),
        ));
    }
    Ok(())
}

fn fact_json(fact: &LosslessSessionEvent) -> serde_json::Value {
    serde_json::json!({"source":{"root_identity":fact.source.root_identity,"relative_path":fact.source.relative_path,"file_device":fact.source.file_device,"file_inode":fact.source.file_inode,"prefix_generation":format!("{:032x}",fact.source.prefix_generation),"range_start":fact.source.range_start,"range_end":fact.source.range_end,"record_sha256":fact.source.record_sha256,"event_index":fact.source.event_index},"timestamp":fact.timestamp,"model":fact.model,
        "tokens":{"total":fact.tokens.total,"input":fact.tokens.input,"cached_input":fact.tokens.cached_input,"cache_write_input":fact.tokens.cache_write_input,"output":fact.tokens.output},"period":fact.period,"admission":fact.admission})
}

fn valid_retirement(retirement: &SourceCounterRetirement) -> bool {
    let old = &retirement.original;
    let new = &retirement.replacement;
    let Some(event_admission::EventAdmission {
        numeric:
            event_admission::NumericEventProof::ResponseRecord {
                candidate,
                legacy_family_exact_model,
            },
        ..
    }) = &new.admission
    else {
        return false;
    };
    let counter = &retirement.counter;
    let facts = |point: &event_admission::CounterWitness| TokenFacts {
        total: point.total,
        input: point.input,
        cached_input: point.cached_input,
        cache_write_input: point.cache_write_input,
        output: point.output,
    };
    let delta = facts(&counter.after).checked_delta(facts(&counter.before));
    let compatible = |actual: TokenFacts| {
        old.tokens.total == actual.total
            && old.tokens.input.is_none_or(|n| actual.input == Some(n))
            && old
                .tokens
                .cached_input
                .is_none_or(|n| actual.cached_input == Some(n))
            && old
                .tokens
                .cache_write_input
                .is_none_or(|n| actual.cache_write_input == Some(n))
            && old.tokens.output.is_none_or(|n| actual.output == Some(n))
    };
    old.admission.is_none()
        && old.period.is_none()
        && candidate.matched_counter() == Some(&counter.after.pointer)
        && (candidate.qualified_model() == Some(old.model.as_str())
            || (matches!(old.model.as_str(), "SOL" | "TERRA" | "LUNA")
                && new.model == old.model
                && legacy_family_exact_model.as_deref() == candidate.qualified_model()
                && candidate
                    .qualified_model()
                    .and_then(codex_info_pricing::family)
                    == Some(old.model.as_str())))
        && old.timestamp == counter.after.pointer.timestamp
        && delta.ok().is_some_and(compatible)
        && old.source.root_identity == new.source.root_identity
        && old.source.relative_path == new.source.relative_path
        && old.source.file_device == new.source.file_device
        && old.source.file_inode == new.source.file_inode
        && old.source.prefix_generation == new.source.prefix_generation
        && old.source.range_start <= counter.after.pointer.start
        && counter.after.pointer.end <= old.source.range_end
        && counter.before.pointer.end <= counter.after.pointer.start
}

fn events(
    connection: &Connection,
    checkpoint: &SessionCheckpoint,
) -> Result<Vec<LosslessSessionEvent>> {
    source_events::read_matching_raw(connection,
        "WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5",
        params![checkpoint.root_identity,checkpoint.relative_path,checkpoint.file_device.to_string(),checkpoint.file_inode.to_string(),format!("{:032x}",checkpoint.prefix_generation)])
}

fn fingerprint(connection: &Connection, checkpoint: &SessionCheckpoint) -> Result<String> {
    fingerprint_with_owner(connection, checkpoint, true)
}

fn fingerprint_with_owner(
    connection: &Connection,
    checkpoint: &SessionCheckpoint,
    include_collection_owner: bool,
) -> Result<String> {
    let mut digest = Sha256::new();
    for table in [
        "session_checkpoints",
        "session_ranges",
        "session_events",
        "session_pending_ranges",
    ] {
        digest.update(table.as_bytes());
        let order = match table {
            "session_ranges" | "session_pending_ranges" => "start_offset,end_offset,record_sha256",
            "session_events" => "range_start,range_end,record_sha256,event_index",
            _ => "root_identity",
        };
        let mut query = connection.prepare(&format!("SELECT * FROM {table} WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 ORDER BY {order}"))?;
        let columns = query
            .column_names()
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let mut rows = query.query(params![
            checkpoint.root_identity,
            checkpoint.relative_path,
            checkpoint.file_device.to_string(),
            checkpoint.file_inode.to_string(),
            format!("{:032x}", checkpoint.prefix_generation)
        ])?;
        while let Some(row) = rows.next()? {
            digest.update([0xff]);
            for (column, name) in columns.iter().enumerate() {
                // Scheduled collection can acknowledge the same accepted bytes
                // again. These stamps are not source/parser/numeric evidence.
                if !include_collection_owner
                    && table != "session_events"
                    && matches!(name.as_str(), "collector_epoch" | "cycle_seq")
                {
                    continue;
                }
                match row.get::<_, Value>(column)? {
                    Value::Null => digest.update([0]),
                    Value::Integer(value) => {
                        digest.update([1]);
                        digest.update(value.to_be_bytes());
                    }
                    Value::Real(value) => {
                        digest.update([2]);
                        digest.update(value.to_bits().to_be_bytes());
                    }
                    Value::Text(value) => {
                        digest.update([3]);
                        digest.update((value.len() as u64).to_be_bytes());
                        digest.update(value.as_bytes());
                    }
                    Value::Blob(value) => {
                        digest.update([4]);
                        digest.update((value.len() as u64).to_be_bytes());
                        digest.update(value);
                    }
                }
            }
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

impl UsageStore {
    /// Reseal an already verified source intent after an ordinary collection
    /// acknowledgement. Every source/parser/range/event/pending value remains
    /// identical; only the collection stamps may advance. The fixed period
    /// evidence and all proposed facts are retained. `None` means the source
    /// facts changed and the caller must prepare a fresh candidate.
    pub fn rebase_source_qualification_repair(
        &self,
        repair: &PreparedSourceQualificationRepair,
    ) -> Result<Option<PreparedSourceQualificationRepair>> {
        if repair.equivalence.is_some()
            || self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM source_prefix_repairs WHERE repair_id=?1)",
                [&repair.repair_id],
                |row| row.get::<_, bool>(0),
            )?
        {
            // The transactional acknowledgement path verifies the saved proof
            // and exact qualified facts, including retries after success.
            return Ok(Some(repair.clone()));
        }
        let current = fingerprint(&self.connection, &repair.checkpoint)?;
        if current == repair.input_fingerprint {
            return Ok(Some(repair.clone()));
        }
        let provenance: serde_json::Value =
            serde_json::from_str(&repair.provenance).map_err(|_| {
                UsageStoreError::InvalidImport("invalid source repair provenance".into())
            })?;
        if provenance["input_facts_fingerprint"].as_str()
            != Some(fingerprint_with_owner(&self.connection, &repair.checkpoint, false)?.as_str())
            || events(&self.connection, &repair.checkpoint)? != repair.original_events
        {
            return Ok(None);
        }
        let mut rebased = repair.clone();
        rebased.input_fingerprint = current;
        // Collection stamps do not change the verified source intent or its
        // acknowledgement identity. Its original provenance stays immutable.
        Ok(Some(rebased))
    }

    pub fn load_unqualified_session_event_facts(&self) -> Result<Vec<LosslessSessionEvent>> {
        source_events::read_matching(&self.connection, "WHERE admission_json IS NULL", [])
    }
    pub fn source_events_for_qualification_repair(
        &self,
        checkpoint: &SessionCheckpoint,
    ) -> Result<Vec<LosslessSessionEvent>> {
        validate_session_checkpoint(checkpoint)?;
        events(&self.connection, checkpoint)
    }
    pub fn prepare_source_qualification_repair(
        &self,
        checkpoint: &SessionCheckpoint,
        original_events: Vec<LosslessSessionEvent>,
        qualified_events: Vec<LosslessSessionEvent>,
        backup: VerifiedPartitionBackup,
    ) -> Result<PreparedSourceQualificationRepair> {
        self.prepare_source_prefix_repair(
            checkpoint,
            original_events,
            qualified_events,
            Vec::new(),
            backup,
        )
    }
    pub fn prepare_source_prefix_repair(
        &self,
        checkpoint: &SessionCheckpoint,
        original_events: Vec<LosslessSessionEvent>,
        qualified_events: Vec<LosslessSessionEvent>,
        retirements: Vec<SourceCounterRetirement>,
        backup: VerifiedPartitionBackup,
    ) -> Result<PreparedSourceQualificationRepair> {
        let inputs = self.prepare_source_repair_inputs(backup)?;
        self.prepare_source_prefix_repair_with_inputs(
            checkpoint,
            original_events,
            qualified_events,
            retirements,
            inputs,
        )
    }
    pub fn prepare_source_repair_inputs(
        &self,
        backup: VerifiedPartitionBackup,
    ) -> Result<PreparedSourceRepairInputs> {
        if legacy_raw_evidence(&self.connection)?
            != (backup.raw_rows, backup.raw_fingerprint.clone())
        {
            return Err(UsageStoreError::InvalidImport(
                "source repair backup no longer matches its original".into(),
            ));
        }
        Ok(PreparedSourceRepairInputs {
            backup,
            calculation_inputs: std::sync::Arc::new(CalculationInputs::capture(&self.connection)?),
        })
    }
    pub fn prepare_source_prefix_repair_with_inputs(
        &self,
        checkpoint: &SessionCheckpoint,
        original_events: Vec<LosslessSessionEvent>,
        qualified_events: Vec<LosslessSessionEvent>,
        retirements: Vec<SourceCounterRetirement>,
        inputs: PreparedSourceRepairInputs,
    ) -> Result<PreparedSourceQualificationRepair> {
        self.prepare_source_prefix_repair_with_parser_inputs(
            checkpoint,
            checkpoint,
            original_events,
            qualified_events,
            retirements,
            inputs,
        )
    }

    pub fn prepare_source_prefix_repair_with_parser_inputs(
        &self,
        checkpoint: &SessionCheckpoint,
        reconstructed: &SessionCheckpoint,
        original_events: Vec<LosslessSessionEvent>,
        qualified_events: Vec<LosslessSessionEvent>,
        retirements: Vec<SourceCounterRetirement>,
        inputs: PreparedSourceRepairInputs,
    ) -> Result<PreparedSourceQualificationRepair> {
        self.prepare_source_prefix_repair_inner(
            checkpoint,
            reconstructed,
            original_events,
            qualified_events,
            retirements,
            inputs,
            None,
        )
    }

    pub fn prepare_source_numerical_repair(
        &self,
        checkpoint: &SessionCheckpoint,
        original_events: Vec<LosslessSessionEvent>,
        qualified_events: Vec<LosslessSessionEvent>,
        inputs: PreparedSourceRepairInputs,
        replay: SourceNumericalReplay,
    ) -> Result<PreparedSourceQualificationRepair> {
        self.prepare_source_prefix_repair_inner(
            checkpoint,
            &replay.checkpoint,
            original_events,
            qualified_events,
            Vec::new(),
            inputs,
            Some(&replay),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_source_prefix_repair_inner(
        &self,
        checkpoint: &SessionCheckpoint,
        reconstructed: &SessionCheckpoint,
        original_events: Vec<LosslessSessionEvent>,
        qualified_events: Vec<LosslessSessionEvent>,
        retirements: Vec<SourceCounterRetirement>,
        inputs: PreparedSourceRepairInputs,
        numerical_replay: Option<&SourceNumericalReplay>,
    ) -> Result<PreparedSourceQualificationRepair> {
        validate_session_checkpoint(checkpoint)?;
        // Parser restoration cannot move a cursor or change raw counters.
        // Only the qualification derived from the fixed accepted N is new.
        let mut same_facts = reconstructed.clone();
        same_facts.last_model = checkpoint.last_model.clone();
        same_facts.last_model_context_kind = checkpoint.last_model_context_kind;
        same_facts.last_model_interval = checkpoint.last_model_interval.clone();
        same_facts.source_admission = checkpoint.source_admission.clone();
        if numerical_replay.is_some() {
            same_facts.token_baseline_known = checkpoint.token_baseline_known;
            same_facts.previous_total = checkpoint.previous_total;
            same_facts.previous_input = checkpoint.previous_input;
            same_facts.previous_cached_input = checkpoint.previous_cached_input;
            same_facts.previous_cache_write_input = checkpoint.previous_cache_write_input;
            same_facts.previous_output = checkpoint.previous_output;
        }
        if same_facts != *checkpoint {
            return Err(UsageStoreError::InvalidImport(
                "source parser repair changed original facts".into(),
            ));
        }
        let PreparedSourceRepairInputs {
            backup,
            calculation_inputs,
        } = inputs;
        if retirements.iter().any(|retirement| {
            !valid_retirement(retirement)
                || !original_events.contains(&retirement.original)
                || !qualified_events.contains(&retirement.replacement)
        }) {
            return Err(UsageStoreError::InvalidImport(
                "source counter replacement lacks exact original proof".into(),
            ));
        }
        if events(&self.connection, checkpoint)? != original_events {
            return Err(UsageStoreError::InvalidImport(
                "source repair facts changed while replaying original".into(),
            ));
        }
        let binding = qualified_events
            .iter()
            .filter_map(|fact| fact.admission.as_ref())
            .map(|admission| &admission.source_binding)
            .find(|binding| binding.matches_checkpoint(checkpoint))
            .or_else(|| {
                reconstructed
                    .source_admission
                    .as_ref()
                    .and_then(|state| state.account_binding.as_ref())
                    .filter(|binding| binding.matches_checkpoint(checkpoint))
            })
            .ok_or_else(|| {
                UsageStoreError::InvalidImport("source repair has no qualified source fence".into())
            })?;
        let mut verification_checkpoint = reconstructed.clone();
        let state = verification_checkpoint
            .source_admission
            .as_mut()
            .ok_or_else(|| {
                UsageStoreError::InvalidImport(
                    "source repair has no reconstructed parser qualification".into(),
                )
            })?;
        if !state.metadata_qualified || state.metadata.as_ref() != Some(&binding.metadata) {
            return Err(UsageStoreError::InvalidImport(
                "source repair fence metadata changed".into(),
            ));
        }
        state.account_binding = Some(binding.clone());
        validate_session_checkpoint(&verification_checkpoint)?;
        let input_fingerprint = fingerprint(&self.connection, checkpoint)?;
        let original_pending=self.load_session_pending_ranges()?.into_iter().filter(|pending|pending.root_identity==checkpoint.root_identity && pending.relative_path==checkpoint.relative_path && pending.file_device==checkpoint.file_device && pending.file_inode==checkpoint.file_inode && pending.prefix_generation==checkpoint.prefix_generation)
            .map(|pending|serde_json::json!({"start":pending.start_offset,"end":pending.end_offset,"record_sha256":pending.record_sha256,"reason":pending.reason,"parser_version":pending.parser_version,"complete":pending.complete,"collector_epoch":format!("{:032x}",pending.collector_epoch),"cycle_seq":pending.cycle_seq})).collect::<Vec<_>>();
        let provenance=serde_json::to_string(&serde_json::json!({"version":1,"checkpoint":{"root_identity":checkpoint.root_identity,"relative_path":checkpoint.relative_path,"file_device":checkpoint.file_device,"file_inode":checkpoint.file_inode,"prefix_generation":format!("{:032x}",checkpoint.prefix_generation),"accepted_end":checkpoint.committed_offset,"prefix_sha256":checkpoint.prefix_sha256},"input_fingerprint":input_fingerprint,"input_facts_fingerprint":fingerprint_with_owner(&self.connection,checkpoint,false)?,
            "original_pending":original_pending,
            "original_parser":{"last_model":checkpoint.last_model,"last_model_context_kind":checkpoint.last_model_context_kind.map(SessionModelContextKind::as_str),"last_model_interval":checkpoint.last_model_interval.as_ref().map(SessionModelInterval::to_storage),"source_admission":checkpoint.source_admission},
            "qualified_parser":{"last_model":verification_checkpoint.last_model,"last_model_context_kind":verification_checkpoint.last_model_context_kind.map(SessionModelContextKind::as_str),"last_model_interval":verification_checkpoint.last_model_interval.as_ref().map(SessionModelInterval::to_storage),"source_admission":verification_checkpoint.source_admission},
            "original_raw":{"baseline_known":checkpoint.token_baseline_known,"total":checkpoint.previous_total,"input":checkpoint.previous_input,"cached_input":checkpoint.previous_cached_input,"cache_write_input":checkpoint.previous_cache_write_input,"output":checkpoint.previous_output},
            "qualified_raw":{"baseline_known":verification_checkpoint.token_baseline_known,"total":verification_checkpoint.previous_total,"input":verification_checkpoint.previous_input,"cached_input":verification_checkpoint.previous_cached_input,"cache_write_input":verification_checkpoint.previous_cache_write_input,"output":verification_checkpoint.previous_output},
            "numeric_replay":numerical_replay.map(|replay|serde_json::json!({"records":replay.records,"pending":replay.pending.iter().map(|pending|serde_json::json!({"start":pending.start_offset,"end":pending.end_offset,"reason":pending.reason})).collect::<Vec<_>>()})),
            "backup":{"partition_id":backup.partition_id,"raw_rows":backup.raw_rows,"raw_fingerprint":backup.raw_fingerprint},
            "original":original_events.iter().map(fact_json).collect::<Vec<_>>(),"qualified":qualified_events.iter().map(fact_json).collect::<Vec<_>>(),
            "retirements":retirements.iter().map(|retirement|serde_json::json!({"original":fact_json(&retirement.original),"replacement":fact_json(&retirement.replacement),"counter":retirement.counter})).collect::<Vec<_>>()
        })).map_err(|_|UsageStoreError::InvalidImport("source repair provenance cannot be encoded".into()))?;
        let repair_id = format!("{:x}", Sha256::digest(provenance.as_bytes()));
        Ok(PreparedSourceQualificationRepair {
            checkpoint: checkpoint.clone(),
            verification_checkpoint,
            qualified_events,
            original_events,
            input_fingerprint,
            backup,
            retirements,
            provenance,
            repair_id,
            calculation_inputs,
            numerical_replay: numerical_replay.is_some(),
            replay_pending: numerical_replay.map_or_else(Vec::new, |replay| replay.pending.clone()),
            read_container: None,
            equivalence: None,
        })
    }
}

impl UsageStore {
    /// A real scheduled commit of an existing admitted partition, before
    /// bounded source work. No source position or observation is synthesized.
    pub fn acknowledge_existing_session_owner(
        &mut self,
        identity: &StoragePartitionIdentity,
    ) -> Result<Option<SessionCollectionState>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_storage_partition_identity(&tx, identity)?;
        let (generation,epoch,cycle):(String,Option<String>,String)=tx.query_row(
            "SELECT data_generation,collector_epoch,cycle_seq FROM collection_generation WHERE singleton=1",[],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        let generation = canonical_u64_text(&generation, "startup generation")?;
        let cycle = canonical_u64_text(&cycle, "startup cycle")?;
        let has_quota: bool =
            tx.query_row("SELECT EXISTS(SELECT 1 FROM quota_receipts)", [], |row| {
                row.get(0)
            })?;
        if generation == 0 || cycle == 0 || epoch.is_none() || !has_quota {
            return Ok(None);
        }
        canonical_u128_hex(
            epoch.as_deref().expect("checked above"),
            "startup collector epoch",
        )?;
        let next = generation
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        let next_cycle = cycle
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        tx.execute(
            "UPDATE collection_generation SET data_generation=?1,cycle_seq=?2 WHERE singleton=1",
            params![next.to_string(), next_cycle.to_string()],
        )?;
        tx.commit()?;
        let state = self.load_session_collection_state()?;
        if state.data_generation != next || state.cycle_seq != next_cycle {
            return Err(UsageStoreError::InvalidImport(
                "startup owner commit readback changed".into(),
            ));
        }
        Ok(Some(state))
    }

    /// Apply only already accepted historical evidence. This never advances a
    /// Session cursor, CollectorEpoch, cycle, or the independent quota receipt.
    pub fn commit_historical_source_repairs(
        &mut self,
        prepared: &period_evidence::PreparedPeriodAttribution,
        repairs: &[PreparedSourceQualificationRepair],
    ) -> Result<Option<u64>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let _canonical_scope = CanonicalSourceScope::begin(&tx)?;
        let before = tx.total_changes();
        period_evidence::commit_snapshot(&tx, prepared)?;
        let mut periods = apply(&tx, repairs, Some(prepared))?;
        for (period, since) in period_evidence::apply_reassignments(&tx, prepared)? {
            periods
                .entry(period)
                .and_modify(|old| *old = (*old).min(since))
                .or_insert(since);
        }
        let prices = period_evidence::capture_projection_prices(&tx, &periods)?;
        period_evidence::reproject_periods(&tx, &periods, &prices)?;
        let current: (i64, i64) = tx.query_row(
            "SELECT reset_at,window_seconds FROM collection_generation WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let incomplete: bool = tx.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM {} WHERE timestamp>=?1 AND timestamp<?2
                 AND (admission_json IS NULL OR reset_at IS NULL))",
                canonical_table(&tx)?
            ),
            params![current.0.saturating_sub(current.1), current.0],
            |row| row.get(0),
        )?;
        if !incomplete
            && (periods.contains_key(&current)
                || repairs.iter().any(|repair| repair.equivalence.is_some()))
        {
            // A source alias/rebind still updates current canonical totals,
            // without recalculating past saved model/family observations.
            // An incomplete qualified subset cannot replace the already
            // saved absolute current value while other sources are pending.
            let facts = source_events::read_matching(
                &tx,
                "WHERE reset_at=?1 AND window_seconds=?2 AND admission_json IS NOT NULL",
                params![current.0, current.1],
            )?
            .into_iter()
            .map(SessionEvent::try_from)
            .collect::<Result<Vec<_>>>()?;
            let totals = sum_session_events(facts.iter())?;
            if totals != session_model_totals_from_transaction(&tx)? {
                tx.execute("DELETE FROM session_model_totals", [])?;
                for total in totals {
                    tx.execute("INSERT INTO session_model_totals(model,total_tokens,input_tokens,cached_input_tokens,output_tokens,cache_write_input_tokens) VALUES(?1,?2,?3,?4,?5,?6)",params![total.model,total.total_tokens.to_string(),total.input_tokens.map(|n|n.to_string()),total.cached_input_tokens.map(|n|n.to_string()),total.output_tokens.map(|n|n.to_string()),total.cache_write_input_tokens.map(|n|n.to_string())])?;
                }
            }
        }
        crate::recorded_usage::finish(&tx)?;
        if tx.total_changes() == before {
            return Ok(None);
        }
        let generation: String = tx.query_row(
            "SELECT data_generation FROM collection_generation WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let next = canonical_u64_text(&generation, "historical repair generation")?
            .checked_add(1)
            .ok_or(UsageStoreError::GenerationOverflow)?;
        tx.execute(
            "UPDATE collection_generation SET data_generation=?1 WHERE singleton=1",
            [next.to_string()],
        )?;
        tx.commit()?;
        Ok(Some(next))
    }
}

// A source-qualified unknown model is not a qualified model label. Only a
// sealed full-prefix numerical replay may add its missing original context;
// the raw witnesses, knownness, source, owner, and observed time stay exact.
fn original_model_refinement(old: &LosslessSessionEvent, new: &LosslessSessionEvent) -> bool {
    let (Some(before), Some(after)) = (&old.admission, &new.admission) else {
        return false;
    };
    let (
        event_admission::NumericEventProof::LegacyCounter(saved),
        event_admission::NumericEventProof::LegacyCounter(replayed),
    ) = (&before.numeric, &after.numeric)
    else {
        return false;
    };
    old.model == "UNATTRIBUTED"
        && new.model != "UNATTRIBUTED"
        && model_names::normalize_model_name(&new.model) == Some(new.model.as_str())
        && old.period.is_none()
        && before.period.is_none()
        && old.source == new.source
        && old.timestamp == new.timestamp
        && old.tokens == new.tokens
        && before.version == after.version
        && before.source_binding == after.source_binding
        && before.verified_prefix_end == after.verified_prefix_end
        && before.ownership == after.ownership
        && saved.model_context.is_none()
        && replayed.model_context.is_some()
        && saved.before == replayed.before
        && saved.after == replayed.after
        && before.valid_for(old, old.period)
        && after.valid_for(new, new.period)
}

pub(super) fn apply(
    tx: &rusqlite::Transaction<'_>,
    repairs: &[PreparedSourceQualificationRepair],
    prepared: Option<&period_evidence::PreparedPeriodAttribution>,
) -> Result<BTreeMap<(i64, i64), i64>> {
    let canonical_scope = CanonicalSourceScope::begin(tx)?;
    let mut periods = BTreeMap::new();
    let mut checked_inputs = BTreeSet::new();
    for repair in repairs {
        let prepared = prepared.ok_or_else(|| {
            UsageStoreError::InvalidImport("source repair has no fixed official evidence".into())
        })?;
        if repair.equivalence.is_some() {
            for (period, since) in equivalence::apply_equivalence(tx, repair, prepared)? {
                periods
                    .entry(period)
                    .and_modify(|at: &mut i64| *at = (*at).min(since))
                    .or_insert(since);
            }
            continue;
        }
        let partition = prepared.partition();
        if let Some(container) = &repair.read_container {
            if !container.covers(&repair.checkpoint)
                || container_ranges(tx, container)? != container.ranges
            {
                return Err(UsageStoreError::InvalidImport(
                    "reading container changed before commit".into(),
                ));
            }
        }
        validate_source_binding_partitions(tx, std::iter::once(&repair.verification_checkpoint))?;
        if repair
            .verification_checkpoint
            .source_admission
            .as_ref()
            .and_then(|state| state.account_binding.as_ref())
            .is_none_or(|binding| &binding.partition != partition)
        {
            return Err(UsageStoreError::InvalidImport(
                "source repair partition fence changed".into(),
            ));
        }
        if let Some((payload, sha)) = tx
            .query_row(
                "SELECT payload_json,payload_sha256 FROM source_prefix_repairs WHERE repair_id=?1",
                [&repair.repair_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        {
            let actual = events(tx, &repair.checkpoint)?;
            let mut acknowledged = actual.len() == repair.qualified_events.len();
            for proposed in &repair.qualified_events {
                let Some(saved) = actual.iter().find(|saved| saved.source == proposed.source)
                else {
                    acknowledged = false;
                    break;
                };
                if saved != proposed
                    && !prepared.is_reassignment_of(proposed, saved)?
                    && !(repair.original_events.contains(saved)
                        && prepared.is_reassignment_of(saved, proposed)?)
                {
                    acknowledged = false;
                    break;
                }
            }
            if payload != repair.provenance
                || sha != repair.repair_id
                || format!("{:x}", Sha256::digest(payload.as_bytes())) != sha
                || !acknowledged
            {
                return Err(UsageStoreError::InvalidImport(
                    "source repair acknowledgement changed".into(),
                ));
            }
            continue;
        }
        let calculation_input_key = std::sync::Arc::as_ptr(&repair.calculation_inputs);
        if checked_inputs.insert(calculation_input_key) && !repair.calculation_inputs.matches(tx)? {
            return Err(UsageStoreError::InvalidImport(
                "source repair calculation input changed".into(),
            ));
        }
        if repair.backup.partition_id != partition.partition_id
            || fingerprint(tx, &repair.checkpoint)? != repair.input_fingerprint
            || events(tx, &repair.checkpoint)? != repair.original_events
            || (!repair.numerical_replay
                && repair.original_events.len() != repair.qualified_events.len())
        {
            return Err(UsageStoreError::InvalidImport(
                "source qualification repair input changed".into(),
            ));
        }
        let mut numerical_periods = BTreeMap::new();
        let mut changed = Vec::new();
        let mut inserted = Vec::new();
        for old in &repair.original_events {
            let model_refinement = repair.numerical_replay
                && old.source.range_end <= repair.checkpoint.committed_offset
                && repair
                    .qualified_events
                    .iter()
                    .any(|new| original_model_refinement(old, new));
            if repair.numerical_replay && (old.admission.is_none() || model_refinement) {
                if repair.qualified_events.contains(old) {
                    continue;
                }
                if let Some(period) = prepared.candidate_period_at(old.timestamp) {
                    numerical_periods
                        .entry((period.reset_at, period.window_seconds))
                        .and_modify(|since: &mut i64| *since = (*since).min(old.timestamp))
                        .or_insert(old.timestamp);
                    periods
                        .entry((period.reset_at, period.window_seconds))
                        .and_modify(|since: &mut i64| *since = (*since).min(old.timestamp))
                        .or_insert(old.timestamp);
                }
                crate::recorded_usage::remove(tx, &old.source)?;
                let source = &old.source;
                if tx.execute("DELETE FROM session_events WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 AND range_start=?6 AND range_end=?7 AND record_sha256=?8 AND event_index=?9",params![source.root_identity,source.relative_path,source.file_device.to_string(),source.file_inode.to_string(),format!("{:032x}",source.prefix_generation),source.range_start,source.range_end,source.record_sha256,source.event_index])?!=1 {
                    return Err(UsageStoreError::InvalidImport("source numerical original changed".into()));
                }
                continue;
            }
            if let Some(retirement) = repair
                .retirements
                .iter()
                .find(|retirement| retirement.original == *old)
            {
                if !valid_retirement(retirement) {
                    return Err(UsageStoreError::InvalidImport(
                        "invalid source retirement".into(),
                    ));
                }
                let mut expected = retirement.replacement.clone();
                prepared.assign(&mut expected)?;
                if expected != retirement.replacement {
                    return Err(UsageStoreError::InvalidImport(
                        "source replacement period changed".into(),
                    ));
                }
                crate::recorded_usage::remove(tx, &old.source)?;
                let source = &old.source;
                if tx.execute("DELETE FROM session_events WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 AND range_start=?6 AND range_end=?7 AND record_sha256=?8 AND event_index=?9",params![source.root_identity,source.relative_path,source.file_device.to_string(),source.file_inode.to_string(),format!("{:032x}",source.prefix_generation),source.range_start,source.range_end,source.record_sha256,source.event_index])?!=1 {return Err(UsageStoreError::InvalidImport("source retirement row changed".into()));}
                inserted.push(retirement.replacement.clone());
                changed.push(retirement.replacement.clone());
                continue;
            }
            let new = repair
                .qualified_events
                .iter()
                .find(|new| new.source == old.source)
                .ok_or_else(|| {
                    UsageStoreError::InvalidImport("source repair lost an original".into())
                })?;
            if old == new || prepared.is_reassignment_of(old, new)? {
                // Retaining an old fact grants no qualification from this
                // bounded parser replay. Its saved range may extend beyond
                // this checkpoint and must remain untouched in that case.
                continue;
            }
            let changed_fact_failure = if old.source.range_end > repair.checkpoint.committed_offset
            {
                Some("saved range exceeds the repair prefix")
            } else if SessionEvent::try_from(old.clone())? != SessionEvent::try_from(new.clone())? {
                Some("numeric/source values differ")
            } else if old.admission.is_some() {
                Some("previous admission differs")
            } else {
                None
            };
            if let Some(reason) = changed_fact_failure {
                let admission_changes = match (&old.admission, &new.admission) {
                    (Some(before), Some(after)) => format!(
                        "binding={},prefix={},ownership={},numeric={},period={}",
                        before.source_binding != after.source_binding,
                        before.verified_prefix_end != after.verified_prefix_end,
                        before.ownership != after.ownership,
                        before.numeric != after.numeric,
                        before.period != after.period,
                    ),
                    _ => "presence".to_owned(),
                };
                return Err(UsageStoreError::InvalidImport(format!(
                    "qualification repair cannot replace numeric/source facts: {reason}; timestamp_changed={} model_changed={} tokens_changed={} admission_changed=[{admission_changes}]",
                    old.timestamp != new.timestamp,
                    old.model != new.model,
                    old.tokens != new.tokens,
                )));
            }
            if old.admission.is_none() && old != new {
                let mut expected = new.clone();
                prepared.assign(&mut expected)?;
                if &expected != new {
                    return Err(UsageStoreError::InvalidImport(
                        "qualification repair period changed".into(),
                    ));
                }
                changed.push(new.clone());
            }
        }
        if repair.numerical_replay {
            for fact in &repair.qualified_events {
                if repair.original_events.contains(fact) {
                    continue;
                }
                if let Some(old) = repair
                    .original_events
                    .iter()
                    .find(|old| old.source == fact.source)
                {
                    if prepared.is_reassignment_of(old, fact)? {
                        continue;
                    }
                }
                let mut expected = fact.clone();
                prepared.assign(&mut expected)?;
                if expected != *fact || fact.source.range_end > repair.checkpoint.committed_offset {
                    return Err(UsageStoreError::InvalidImport(
                        "numerical replacement lacks fixed original qualification".into(),
                    ));
                }
                inserted.push(fact.clone());
                changed.push(fact.clone());
            }
        }
        let mut raw = BTreeMap::new();
        for fact in inserted {
            let event = SessionEvent::try_from(fact)?;
            raw.insert(
                (
                    event.root_identity.clone(),
                    event.relative_path.clone(),
                    event.file_device,
                    event.file_inode,
                    event.prefix_generation,
                    event.range_start,
                    event.range_end,
                    event.record_sha256.clone(),
                    event.event_index,
                ),
                event,
            );
        }
        upsert_session_events(tx, &raw)?;
        upsert_session_event_qualifications(tx, &changed, false, "source-repair", Some(prepared))?;
        if repair.checkpoint.source_admission != repair.verification_checkpoint.source_admission
            || repair.checkpoint.last_model_context_kind
                != repair.verification_checkpoint.last_model_context_kind
            || repair.checkpoint.last_model_interval
                != repair.verification_checkpoint.last_model_interval
            || repair.numerical_replay
        {
            let checkpoint = &repair.verification_checkpoint;
            let admission = checkpoint
                .source_admission
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| {
                    UsageStoreError::InvalidImport("source parser repair encoding failed".into())
                })?;
            if tx.execute("UPDATE session_checkpoints SET last_model=?6,last_model_context_kind=?7,last_model_interval=?8,source_admission=?9,token_baseline_known=?10,previous_total=?11,previous_input=?12,previous_cached_input=?13,previous_cache_write_input=?14,previous_output=?15 WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5",
                params![checkpoint.root_identity,checkpoint.relative_path,checkpoint.file_device.to_string(),checkpoint.file_inode.to_string(),format!("{:032x}",checkpoint.prefix_generation),checkpoint.last_model,checkpoint.last_model_context_kind.map(SessionModelContextKind::as_str),checkpoint.last_model_interval.as_ref().map(SessionModelInterval::to_storage),admission,i64::from(checkpoint.token_baseline_known),checkpoint.previous_total.map(|n|n.to_string()),checkpoint.previous_input.map(|n|n.to_string()),checkpoint.previous_cached_input.map(|n|n.to_string()),checkpoint.previous_cache_write_input.map(|n|n.to_string()),checkpoint.previous_output.map(|n|n.to_string())])? != 1 {
                return Err(UsageStoreError::InvalidImport("source parser repair checkpoint changed".into()));
            }
        }
        if repair.numerical_replay {
            let checkpoint = &repair.checkpoint;
            tx.execute("DELETE FROM session_pending_ranges WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 AND end_offset<=?6",params![checkpoint.root_identity,checkpoint.relative_path,checkpoint.file_device.to_string(),checkpoint.file_inode.to_string(),format!("{:032x}",checkpoint.prefix_generation),checkpoint.committed_offset])?;
        }
        if !repair.replay_pending.is_empty() {
            let mut pending = BTreeMap::new();
            for item in &repair.replay_pending {
                validate_session_pending_range(item)?;
                pending.insert(
                    (
                        item.root_identity.clone(),
                        item.relative_path.clone(),
                        item.file_device,
                        item.file_inode,
                        item.prefix_generation,
                        item.start_offset,
                    ),
                    item.clone(),
                );
            }
            replace_session_pending_ranges(tx, &pending, &BTreeMap::new(), false)?;
        }
        tx.execute("INSERT INTO source_prefix_repairs (repair_id,payload_json,payload_sha256) VALUES (?1,?2,?1)",params![repair.repair_id,repair.provenance])?;
        for fact in &changed {
            if let Some(period) = fact.period {
                if repair.numerical_replay {
                    numerical_periods
                        .entry((period.reset_at, period.window_seconds))
                        .and_modify(|since: &mut i64| *since = (*since).min(fact.timestamp))
                        .or_insert(fact.timestamp);
                }
                periods
                    .entry((period.reset_at, period.window_seconds))
                    .and_modify(|since: &mut i64| *since = (*since).min(fact.timestamp))
                    .or_insert(fact.timestamp);
            }
        }
        canonical_scope.permit_numeric_corrections(&numerical_periods);
    }
    Ok(periods)
}

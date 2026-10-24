//! Full accepted-prefix equivalence. Raw source rows remain an immutable
//! inventory; only a sealed, transactionally installed relation retires them
//! from the canonical consumption stream.
use super::*;
use crate::event_admission::NumericEventProof;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEquivalenceMode {
    IdenticalPrefixAlias,
    EventBijection,
}
impl SourceEquivalenceMode {
    fn name(self) -> &'static str {
        match self {
            Self::IdenticalPrefixAlias => "identical-prefix-alias",
            Self::EventBijection => "event-bijection",
        }
    }
}

/// The recorder supplies the fixed, fully replayed prefix, never a trusted
/// boolean. Its FD/hash/owner fence is repeated immediately before commit.
#[derive(Clone)]
pub struct SourceEquivalencePrefix {
    pub checkpoint: SessionCheckpoint,
    pub verification_checkpoint: SessionCheckpoint,
    pub ranges: Vec<SessionRange>,
    pub qualified_events: Vec<LosslessSessionEvent>,
    pub numeric_records: Vec<serde_json::Value>,
    pub parser_version: String,
}

#[derive(Clone)]
pub(super) struct PreparedEquivalence {
    pub(super) prefixes: Vec<SourceEquivalencePrefix>,
    inventory: Vec<LosslessSessionEvent>,
    fingerprints: Vec<(SessionCheckpoint, String)>,
    mappings: Vec<serde_json::Value>,
    canonical: Vec<LosslessSessionEvent>,
}

fn invalid(message: &str) -> UsageStoreError {
    UsageStoreError::InvalidImport(message.into())
}
fn pointer(source: &SessionEventSource) -> Result<serde_json::Value> {
    serde_json::from_str(&crate::recorded_usage::source_key(source)?)
        .map_err(|_| invalid("invalid equivalence source key"))
}
fn qualified_json(fact: &LosslessSessionEvent) -> Result<serde_json::Value> {
    Ok(
        serde_json::json!({"source":pointer(&fact.source)?,"timestamp":fact.timestamp,
        "model":fact.model,"tokens":fact.tokens,"period":fact.period,"admission":fact.admission}),
    )
}
fn source_order(source: &SessionEventSource) -> SessionEventSourceKey {
    (
        source.root_identity.clone(),
        source.relative_path.clone(),
        source.file_device,
        source.file_inode,
        source.prefix_generation,
        source.range_start,
        source.range_end,
        source.record_sha256.clone(),
        source.event_index,
    )
}

fn exact_model(fact: &LosslessSessionEvent) -> &str {
    match fact.admission.as_ref().map(|a| &a.numeric) {
        Some(NumericEventProof::LegacyFamilyCounter { exact_model, .. }) => exact_model,
        Some(NumericEventProof::ResponseRecord {
            legacy_family_exact_model: Some(model),
            ..
        }) => model,
        _ => &fact.model,
    }
}
fn same_value(old: &LosslessSessionEvent, new: &LosslessSessionEvent) -> bool {
    let keeps_known_response = match old.admission.as_ref().map(|a| &a.numeric) {
        Some(NumericEventProof::ResponseRecord {
            candidate: saved, ..
        }) => {
            matches!(new.admission.as_ref().map(|a| &a.numeric),
                Some(NumericEventProof::ResponseRecord { candidate, .. })
                    if saved.record.ids == candidate.record.ids
                        && saved.record.usage == candidate.record.usage)
        }
        _ => true,
    };
    keeps_known_response
        && old.timestamp == new.timestamp
        && old.tokens == new.tokens
        && (exact_model(old) == exact_model(new)
            || (old.admission.is_none()
                && matches!(old.model.as_str(), "SOL" | "TERRA" | "LUNA")
                && codex_info_pricing::family(exact_model(new)) == Some(old.model.as_str())))
}

/// An unqualified old counter can name the same consumption at the matched
/// counter's original second. Only this full-prefix rebind may retire it in
/// favour of the independently qualified response's original second.
fn same_bijection_value(
    old: &LosslessSessionEvent,
    new: &LosslessSessionEvent,
    responses: &[source_admission::PendingResponse],
    numeric_records: &[serde_json::Value],
) -> bool {
    if same_value(old, new) {
        return true;
    }
    if old.admission.is_some()
        || old.period.is_some()
        || old.tokens != new.tokens
        || old.source.root_identity != new.source.root_identity
        || old.source.relative_path != new.source.relative_path
        || (old.source.file_device, old.source.file_inode)
            == (new.source.file_device, new.source.file_inode)
        || !(exact_model(old) == exact_model(new)
            || (matches!(old.model.as_str(), "SOL" | "TERRA" | "LUNA")
                && codex_info_pricing::family(exact_model(new)) == Some(old.model.as_str())))
    {
        return false;
    }
    let Some(admission) = &new.admission else {
        return false;
    };
    let NumericEventProof::ResponseRecord { candidate, .. } = &admission.numeric else {
        return false;
    };
    let Some(counter) = candidate.matched_counter() else {
        return false;
    };
    if !admission.valid_for(new, new.period)
        || !responses.iter().any(|saved| saved == candidate.as_ref())
        || counter.timestamp != old.timestamp
        || counter.timestamp < admission.ownership.start_at
        || admission
            .ownership
            .end_at
            .is_some_and(|end| counter.timestamp >= end)
    {
        return false;
    }
    let mut matching = numeric_records.iter().filter(|record| {
        record["pointer"]["start"].as_u64() == Some(counter.start)
            && record["pointer"]["end"].as_u64() == Some(counter.end)
            && record["pointer"]["sha256"].as_str() == Some(counter.sha256.as_str())
            && record["timestamp"].as_i64() == Some(counter.timestamp)
    });
    let Some(record) = matching.next() else {
        return false;
    };
    if matching.next().is_some() || record["outcome"] != "checked-counter-delta" {
        return false;
    }
    let (Ok(before), Ok(after)) = (
        serde_json::from_value::<TokenFacts>(record["before"].clone()),
        serde_json::from_value::<TokenFacts>(record["raw"].clone()),
    ) else {
        return false;
    };
    after.checked_delta(before) == Ok(old.tokens)
}

fn numeric_pointer(fact: &LosslessSessionEvent) -> Option<&crate::source_admission::RecordPointer> {
    match &fact.admission.as_ref()?.numeric {
        NumericEventProof::LegacyCounter(proof)
        | NumericEventProof::LegacyFamilyCounter { proof, .. } => Some(&proof.after.pointer),
        NumericEventProof::ResponseRecord { candidate, .. } => Some(&candidate.pointer),
    }
}
fn same_record(a: &LosslessSessionEvent, b: &LosslessSessionEvent) -> bool {
    same_value(a, b) && numeric_pointer(a) == numeric_pointer(b)
}
fn physical_events(
    connection: &Connection,
    checkpoint: &SessionCheckpoint,
) -> Result<Vec<LosslessSessionEvent>> {
    crate::source_events::read_matching_raw(
        connection,
        "WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4",
        params![
            checkpoint.root_identity,
            checkpoint.relative_path,
            checkpoint.file_device.to_string(),
            checkpoint.file_inode.to_string()
        ],
    )
}
fn checkpoint_json(checkpoint: &SessionCheckpoint) -> serde_json::Value {
    serde_json::json!({"root_identity":checkpoint.root_identity,"relative_path":checkpoint.relative_path,
        "file_device":checkpoint.file_device.to_string(),"file_inode":checkpoint.file_inode.to_string(),
        "prefix_generation":format!("{:032x}",checkpoint.prefix_generation),"accepted_end":checkpoint.committed_offset,
        "prefix_sha256":checkpoint.prefix_sha256})
}

impl UsageStore {
    pub fn canonical_source_snapshot(&self) -> Result<CanonicalSourceSnapshot> {
        load_snapshot(&self.connection)
    }
    /// Includes every generation and every saved suffix of this physical
    /// source. It deliberately bypasses the canonical retirement relation.
    pub fn source_physical_event_inventory(
        &self,
        checkpoint: &SessionCheckpoint,
    ) -> Result<Vec<LosslessSessionEvent>> {
        validate_session_checkpoint(checkpoint)?;
        physical_events(&self.connection, checkpoint)
    }
    pub fn prepare_source_equivalence(
        &self,
        mode: SourceEquivalenceMode,
        old_checkpoint: &SessionCheckpoint,
        prefixes: Vec<SourceEquivalencePrefix>,
        inputs: PreparedSourceRepairInputs,
    ) -> Result<PreparedSourceQualificationRepair> {
        if prefixes.is_empty()
            || (mode == SourceEquivalenceMode::IdenticalPrefixAlias && prefixes.len() < 2)
        {
            return Err(invalid(
                "equivalence requires complete independent prefixes",
            ));
        }
        let inventory = physical_events(&self.connection, old_checkpoint)?;
        if inventory.is_empty() {
            return Err(invalid("equivalence has no original event inventory"));
        }
        let checkpoints = {
            let transaction = rusqlite::Transaction::new_unchecked(
                &self.connection,
                TransactionBehavior::Deferred,
            )?;
            let mut checkpoints = Vec::new();
            let mut physical_sources = BTreeSet::new();
            for checkpoint in std::iter::once(old_checkpoint)
                .chain(prefixes.iter().map(|prefix| &prefix.checkpoint))
            {
                let physical = (
                    checkpoint.root_identity.clone(),
                    checkpoint.relative_path.clone(),
                    checkpoint.file_device,
                    checkpoint.file_inode,
                );
                if physical_sources.insert(physical) {
                    checkpoints.extend(load_session_checkpoints_matching(
                        &transaction,
                        "WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4",
                        params![checkpoint.root_identity, checkpoint.relative_path,
                            checkpoint.file_device.to_string(), checkpoint.file_inode.to_string()],
                    )?);
                }
            }
            transaction.commit()?;
            checkpoints
        };
        let mut fingerprints = Vec::new();
        for prefix in &prefixes {
            let checkpoint = &prefix.checkpoint;
            validate_session_checkpoint(checkpoint)?;
            validate_session_checkpoint(&prefix.verification_checkpoint)?;
            if prefix.parser_version != "codex-info-session-recorder-v3" {
                return Err(invalid("unsupported equivalence parser input version"));
            }
            if !checkpoints.contains(checkpoint)
                || self.load_session_ranges_for_checkpoint(checkpoint)? != prefix.ranges
            {
                return Err(invalid("equivalence prefix inventory changed"));
            }
            if checkpoint.root_identity != old_checkpoint.root_identity
                || checkpoint.relative_path != old_checkpoint.relative_path
                || checkpoint.committed_offset == 0
            {
                return Err(invalid("equivalence source namespace mismatch"));
            }
            let mut end = 0;
            for range in &prefix.ranges {
                validate_session_range(range)?;
                if range.start_offset > end
                    || range.start_offset >= range.end_offset
                    || range.end_offset > checkpoint.committed_offset
                    || range.root_identity != checkpoint.root_identity
                    || range.relative_path != checkpoint.relative_path
                    || range.file_device != checkpoint.file_device
                    || range.file_inode != checkpoint.file_inode
                    || range.prefix_generation != checkpoint.prefix_generation
                {
                    return Err(invalid("equivalence accepted prefix has a hole"));
                }
                end = end.max(range.end_offset);
            }
            if end != checkpoint.committed_offset {
                return Err(invalid("equivalence prefix is incomplete"));
            }
            let verified = &prefix.verification_checkpoint;
            if source_verification_identity(checkpoint) != source_verification_identity(verified)
                || verified.committed_offset != checkpoint.committed_offset
            {
                return Err(invalid("equivalence parser moved the source cursor"));
            }
            let binding = verified
                .source_admission
                .as_ref()
                .and_then(|s| s.account_binding.as_ref())
                .ok_or_else(|| invalid("equivalence has no source account binding"))?;
            validate_storage_partition_identity(&self.connection, &binding.partition)?;
            if binding.partition.partition_id != inputs.backup.partition_id
                || !binding.matches_checkpoint(verified)
            {
                return Err(invalid("equivalence partition fence mismatch"));
            }
            for fact in &prefix.qualified_events {
                let admission = fact
                    .admission
                    .as_ref()
                    .ok_or_else(|| invalid("equivalence has unqualified facts"))?;
                if !admission.valid_for(fact, fact.period)
                    || admission.source_binding != *binding
                    || fact.source.prefix_generation != checkpoint.prefix_generation
                    || fact.source.file_device != checkpoint.file_device
                    || fact.source.file_inode != checkpoint.file_inode
                    || !prefix.ranges.iter().any(|r| {
                        r.start_offset == fact.source.range_start
                            && r.end_offset == fact.source.range_end
                            && r.record_sha256 == fact.source.record_sha256
                    })
                {
                    return Err(invalid(
                        "equivalence fact is outside the independently accepted prefix",
                    ));
                }
            }
            fingerprints.push((
                checkpoint.clone(),
                fingerprint(&self.connection, checkpoint)?,
            ));
        }
        // Fence every generation of the complete old physical inventory,
        // including checkpoints whose original FD no longer exists.
        for checkpoint in checkpoints.iter().filter(|cp| {
            cp.root_identity == old_checkpoint.root_identity
                && cp.relative_path == old_checkpoint.relative_path
                && cp.file_device == old_checkpoint.file_device
                && cp.file_inode == old_checkpoint.file_inode
        }) {
            if !fingerprints.iter().any(|(cp, _)| cp == checkpoint) {
                fingerprints.push((
                    checkpoint.clone(),
                    fingerprint(&self.connection, checkpoint)?,
                ));
            }
        }
        let mut mappings = Vec::new();
        let mut canonical = Vec::new();
        match mode {
            SourceEquivalenceMode::IdenticalPrefixAlias => {
                let first = &prefixes[0];
                let metadata = first
                    .verification_checkpoint
                    .source_admission
                    .as_ref()
                    .and_then(|s| s.metadata.as_ref());
                for prefix in &prefixes {
                    let cp = &prefix.checkpoint;
                    if cp.file_device != old_checkpoint.file_device
                        || cp.file_inode != old_checkpoint.file_inode
                        || cp.committed_offset != first.checkpoint.committed_offset
                        || prefix
                            .verification_checkpoint
                            .source_admission
                            .as_ref()
                            .and_then(|s| s.metadata.as_ref())
                            != metadata
                        || prefix.qualified_events.len() != first.qualified_events.len()
                        || first.qualified_events.iter().any(|fact| {
                            prefix
                                .qualified_events
                                .iter()
                                .filter(|other| same_record(fact, other))
                                .count()
                                != 1
                        })
                    {
                        return Err(invalid(
                            "generation prefixes do not prove identical physical records",
                        ));
                    }
                }
                if inventory.iter().any(|old| {
                    !prefixes.iter().any(|p| {
                        p.checkpoint.prefix_generation == old.source.prefix_generation
                            && old.source.range_end <= p.checkpoint.committed_offset
                    })
                }) {
                    return Err(invalid(
                        "generation alias omits an original event or suffix",
                    ));
                }
                for fact in &first.qualified_events {
                    let originals = inventory
                        .iter()
                        .filter(|old| same_value(old, fact))
                        .collect::<Vec<_>>();
                    if originals.len() != prefixes.len()
                        || prefixes.iter().any(|p| {
                            originals
                                .iter()
                                .filter(|old| {
                                    old.source.prefix_generation == p.checkpoint.prefix_generation
                                })
                                .count()
                                != 1
                        })
                    {
                        return Err(invalid("generation alias original record is not unique"));
                    }
                    let chosen = *originals
                        .iter()
                        .min_by_key(|old| source_order(&old.source))
                        .expect("nonempty");
                    let prefix = prefixes
                        .iter()
                        .find(|p| p.checkpoint.prefix_generation == chosen.source.prefix_generation)
                        .expect("validated");
                    let mut qualified = prefix
                        .qualified_events
                        .iter()
                        .find(|new| same_record(new, fact))
                        .expect("validated")
                        .clone();
                    qualified.source = chosen.source.clone();
                    if let Some(admission) = &chosen.admission {
                        if !admission.valid_for(chosen, chosen.period) {
                            return Err(invalid("saved alias admission is invalid"));
                        }
                        qualified = chosen.clone();
                    }
                    if !qualified
                        .admission
                        .as_ref()
                        .is_some_and(|a| a.valid_for(&qualified, qualified.period))
                    {
                        return Err(invalid(
                            "canonical pointer does not contain the physical record",
                        ));
                    }
                    for old in originals {
                        mappings.push(serde_json::json!({"original":pointer(&old.source)?,"qualified":qualified_json(&qualified)?,"canonical":pointer(&qualified.source)?,"canonical_kind":"qualified-source"}));
                    }
                    canonical.push(qualified);
                }
                if mappings.len() != inventory.len() {
                    return Err(invalid("generation alias is not a full inventory mapping"));
                }
            }
            SourceEquivalenceMode::EventBijection => {
                if prefixes.len() != 1
                    || (
                        prefixes[0].checkpoint.file_device,
                        prefixes[0].checkpoint.file_inode,
                    ) == (old_checkpoint.file_device, old_checkpoint.file_inode)
                {
                    return Err(invalid(
                        "event rebind requires an independent physical prefix",
                    ));
                }
                let mut used = BTreeSet::new();
                let mut retained = BTreeMap::new();
                for old in &inventory {
                    let prefix = &prefixes[0];
                    let candidates = prefixes[0]
                        .qualified_events
                        .iter()
                        .filter(|new| {
                            same_bijection_value(
                                old,
                                new,
                                &prefix
                                    .verification_checkpoint
                                    .source_admission
                                    .as_ref()
                                    .expect("validated source qualification")
                                    .responses,
                                &prefix.numeric_records,
                            )
                        })
                        .collect::<Vec<_>>();
                    if candidates.len() != 1 {
                        return Err(invalid(
                            "old event does not have exactly one accepted counterpart",
                        ));
                    }
                    let new = candidates[0];
                    if !used.insert(crate::recorded_usage::source_key(&new.source)?) {
                        return Err(invalid("event rebind is not a bijection"));
                    }
                    if matches!(
                        old.admission.as_ref().map(|a| &a.numeric),
                        Some(NumericEventProof::ResponseRecord { .. })
                    ) {
                        // A known global response keeps its original unique
                        // row. The new physical pointer is additional evidence.
                        retained
                            .insert(crate::recorded_usage::source_key(&new.source)?, old.clone());
                        mappings.push(serde_json::json!({"original":pointer(&old.source)?,"qualified":qualified_json(new)?,"canonical":pointer(&old.source)?,"canonical_kind":"retained-response"}));
                    } else {
                        mappings.push(serde_json::json!({"original":pointer(&old.source)?,"qualified":qualified_json(new)?,"canonical":pointer(&new.source)?,"canonical_kind":"qualified-source"}));
                    }
                }
                for new in &prefixes[0].qualified_events {
                    canonical.push(
                        retained
                            .get(&crate::recorded_usage::source_key(&new.source)?)
                            .unwrap_or(new)
                            .clone(),
                    );
                }
            }
        }
        let partition = prefixes[0]
            .verification_checkpoint
            .source_admission
            .as_ref()
            .and_then(|s| s.account_binding.as_ref())
            .expect("validated")
            .partition
            .clone();
        let input_fingerprint = fingerprint(&self.connection, old_checkpoint)?;
        let provenance=serde_json::to_string(&serde_json::json!({"version":2,"mode":mode.name(),"partition":partition,
            "prefixes":prefixes.iter().map(|p|serde_json::json!({"checkpoint":checkpoint_json(&p.checkpoint),"ranges":p.ranges.iter().map(|r|serde_json::json!({"start":r.start_offset.to_string(),"end":r.end_offset.to_string(),"hash":r.record_sha256,"generation":format!("{:032x}",r.prefix_generation)})).collect::<Vec<_>>(),"numeric_records":p.numeric_records,"parser_version":p.parser_version,"source_admission":p.verification_checkpoint.source_admission})).collect::<Vec<_>>(),
            "original":inventory.iter().map(fact_json).collect::<Vec<_>>(),"mappings":mappings,
            "price_inputs":{"fixed":inputs.calculation_inputs.fixed.iter().map(|(r,t,v)|serde_json::json!({"reset_at":r,"timestamp":t,"values":v.iter().map(sql_value_json).collect::<Vec<_>>()})).collect::<Vec<_>>(),"models":inputs.calculation_inputs.models.iter().map(|(r,t,m,v)|serde_json::json!({"reset_at":r,"timestamp":t,"model":m,"values":v.iter().map(sql_value_json).collect::<Vec<_>>()})).collect::<Vec<_>>()},
            "backup":{"partition_id":inputs.backup.partition_id,"raw_rows":inputs.backup.raw_rows,"raw_fingerprint":inputs.backup.raw_fingerprint}}))
            .map_err(|_|invalid("equivalence provenance cannot be encoded"))?;
        let repair_id = format!("{:x}", Sha256::digest(provenance.as_bytes()));
        Ok(PreparedSourceQualificationRepair {
            checkpoint: old_checkpoint.clone(),
            verification_checkpoint: prefixes[0].verification_checkpoint.clone(),
            qualified_events: canonical.clone(),
            original_events: inventory.clone(),
            input_fingerprint,
            backup: inputs.backup,
            retirements: Vec::new(),
            provenance,
            repair_id,
            calculation_inputs: inputs.calculation_inputs,
            numerical_replay: false,
            replay_pending: Vec::new(),
            read_container: None,
            equivalence: Some(PreparedEquivalence {
                prefixes,
                inventory,
                fingerprints,
                mappings,
                canonical,
            }),
        })
    }
}
fn sql_value_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Integer(v) => serde_json::json!({"integer":v.to_string()}),
        Value::Real(v) => serde_json::json!({"real_bits":format!("{:016x}",v.to_bits())}),
        Value::Text(v) => serde_json::json!({"text":v}),
        Value::Blob(v) => serde_json::json!({"blob":v}),
    }
}
fn source_verification_identity(cp: &SessionCheckpoint) -> (&str, &str, u64, u64, u128) {
    (
        &cp.root_identity,
        &cp.relative_path,
        cp.file_device,
        cp.file_inode,
        cp.prefix_generation,
    )
}

fn same_admission_without_period(
    actual: Option<&event_admission::EventAdmission>,
    saved: &serde_json::Value,
) -> bool {
    let Some(mut actual) = actual.cloned() else {
        return false;
    };
    let Ok(mut saved) = serde_json::from_value::<event_admission::EventAdmission>(saved.clone())
    else {
        return false;
    };
    actual.period = None;
    saved.period = None;
    actual == saved
}
/// One verified view of immutable repairs for a fixed producer batch. It is
/// never used as the database commit fence; the transaction validates again.
#[derive(Clone, Default)]
pub struct CanonicalSourceSnapshot {
    retired: BTreeSet<String>,
    replays: BTreeMap<String, (i64, String, TokenFacts)>,
    prefixes: BTreeSet<String>,
    saved_observations: BTreeSet<(i64, i64)>,
    saved_models: BTreeMap<(i64, i64), BTreeSet<String>>,
    checkpoint_references: BTreeSet<[String; 5]>,
    numeric_corrections: BTreeMap<(i64, i64), i64>,
    proofs: usize,
}
impl CanonicalSourceSnapshot {
    pub fn is_retired(&self, source: &SessionEventSource) -> Result<bool> {
        Ok(self
            .retired
            .contains(&crate::recorded_usage::source_key(source)?))
    }
    pub fn is_proved_replay(&self, event: &SessionEvent) -> Result<bool> {
        let fact = LosslessSessionEvent::from(event.clone());
        Ok(self
            .replays
            .get(&crate::recorded_usage::source_key(&fact.source)?)
            .is_some_and(|(at, model, tokens)| {
                *at == fact.timestamp && *model == fact.model && *tokens == fact.tokens
            }))
    }
    pub fn prefix_recorded(&self, checkpoint: &SessionCheckpoint) -> Result<bool> {
        Ok(self.prefixes.contains(
            &serde_json::to_string(&checkpoint_json(checkpoint))
                .map_err(|_| invalid("invalid recorded prefix key"))?,
        ))
    }
    fn correction_covers(&self, reset: i64, timestamp: i64) -> bool {
        self.numeric_corrections.iter().any(|(&(end, _), &since)| {
            end == reset && timestamp >= since.div_euclid(60) * 60 && timestamp < end
        })
    }
}

type SharedSnapshot = std::rc::Rc<std::cell::RefCell<CanonicalSourceSnapshot>>;
thread_local! {
    static CANONICAL_SCOPES:std::cell::RefCell<Vec<(usize,SharedSnapshot)>>=const {std::cell::RefCell::new(Vec::new())};
}
#[cfg(test)]
thread_local! {
    static SNAPSHOT_LOAD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
/// Private synchronous scope: its owning batch/transaction drops the cache on
/// success and rollback alike. No cache crosses a connection or transaction.
pub(crate) struct CanonicalSourceScope {
    key: usize,
    snapshot: SharedSnapshot,
    installed: bool,
}
impl CanonicalSourceScope {
    #[cfg(test)]
    pub(crate) fn reset_test_snapshot_load_count() {
        SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
    }

    #[cfg(test)]
    pub(crate) fn test_snapshot_load_count() -> usize {
        SNAPSHOT_LOAD_COUNT.with(|count| count.get())
    }

    pub(crate) fn begin(connection: &Connection) -> Result<Self> {
        let key = connection as *const Connection as usize;
        if let Some(snapshot) = CANONICAL_SCOPES.with(|scopes| {
            scopes
                .borrow()
                .iter()
                .rev()
                .find(|(candidate, _)| *candidate == key)
                .map(|(_, snapshot)| snapshot.clone())
        }) {
            return Ok(Self {
                key,
                snapshot,
                installed: false,
            });
        }
        let snapshot = std::rc::Rc::new(std::cell::RefCell::new(load_snapshot(connection)?));
        CANONICAL_SCOPES.with(|scopes| scopes.borrow_mut().push((key, snapshot.clone())));
        Ok(Self {
            key,
            snapshot,
            installed: true,
        })
    }
    pub(crate) fn preserves_observation(&self, reset: i64, timestamp: i64) -> bool {
        let snapshot = self.snapshot.borrow();
        snapshot.saved_observations.contains(&(reset, timestamp))
            && !snapshot.correction_covers(reset, timestamp)
    }
    pub(crate) fn preserved_models(&self, reset: i64, timestamp: i64) -> BTreeSet<String> {
        let snapshot = self.snapshot.borrow();
        if snapshot.correction_covers(reset, timestamp) {
            return BTreeSet::new();
        }
        snapshot
            .saved_models
            .get(&(reset, timestamp))
            .cloned()
            .unwrap_or_default()
    }
    pub(crate) fn checkpoint_references_json(&self) -> Result<String> {
        serde_json::to_string(&self.snapshot.borrow().checkpoint_references)
            .map_err(|_| invalid("invalid retained checkpoint references"))
    }
    pub(crate) fn permit_numeric_corrections(&self, periods: &BTreeMap<(i64, i64), i64>) {
        let mut snapshot = self.snapshot.borrow_mut();
        for (&period, &since) in periods {
            snapshot
                .numeric_corrections
                .entry(period)
                .and_modify(|old| *old = (*old).min(since))
                .or_insert(since);
        }
    }
}
impl Drop for CanonicalSourceScope {
    fn drop(&mut self) {
        if self.installed {
            CANONICAL_SCOPES.with(|scopes| {
                let mut scopes = scopes.borrow_mut();
                if let Some(index) = scopes.iter().rposition(|(key, snapshot)| {
                    *key == self.key && std::rc::Rc::ptr_eq(snapshot, &self.snapshot)
                }) {
                    scopes.remove(index);
                }
            });
        }
    }
}
fn refresh_scope(connection: &Connection) -> Result<()> {
    let key = connection as *const Connection as usize;
    let updated = load_snapshot(connection)?;
    CANONICAL_SCOPES.with(|scopes| {
        if let Some((_, snapshot)) = scopes
            .borrow()
            .iter()
            .rev()
            .find(|(candidate, _)| *candidate == key)
        {
            let mut updated = updated;
            updated.numeric_corrections = snapshot.borrow().numeric_corrections.clone();
            *snapshot.borrow_mut() = updated;
        }
    });
    Ok(())
}
fn source_pointer_key(source: &serde_json::Value) -> Result<String> {
    let key = serde_json::json!([
        source["root_identity"],
        source["relative_path"],
        source["file_device"]
            .as_u64()
            .ok_or_else(|| invalid("invalid original device"))?
            .to_string(),
        source["file_inode"]
            .as_u64()
            .ok_or_else(|| invalid("invalid original inode"))?
            .to_string(),
        source["prefix_generation"],
        source["range_start"]
            .as_u64()
            .ok_or_else(|| invalid("invalid original start"))?
            .to_string(),
        source["range_end"]
            .as_u64()
            .ok_or_else(|| invalid("invalid original end"))?
            .to_string(),
        source["record_sha256"],
        source["event_index"]
            .as_u64()
            .ok_or_else(|| invalid("invalid original index"))?
            .to_string()
    ]);
    serde_json::to_string(&key).map_err(|_| invalid("invalid original pointer"))
}
fn mapping_key(value: &serde_json::Value) -> Result<String> {
    if !value
        .as_array()
        .is_some_and(|values| values.len() == 9 && values.iter().all(|v| v.is_string()))
    {
        return Err(invalid("equivalence pointer shape changed"));
    }
    serde_json::to_string(value).map_err(|_| invalid("invalid equivalence pointer"))
}
fn qualified_fact(value: &serde_json::Value) -> Result<LosslessSessionEvent> {
    let key: Vec<String> = serde_json::from_str(&mapping_key(&value["source"])?)
        .map_err(|_| invalid("invalid evidence pointer"))?;
    let number =
        |index: usize| crate::canonical_u64_text(&key[index], "equivalence evidence pointer");
    let fact = LosslessSessionEvent {
        source: SessionEventSource {
            root_identity: key[0].clone(),
            relative_path: key[1].clone(),
            file_device: number(2)?,
            file_inode: number(3)?,
            prefix_generation: crate::canonical_u128_hex(
                &key[4],
                "equivalence evidence generation",
            )?,
            range_start: number(5)?,
            range_end: number(6)?,
            record_sha256: key[7].clone(),
            event_index: number(8)?,
        },
        timestamp: value["timestamp"]
            .as_i64()
            .ok_or_else(|| invalid("invalid evidence timestamp"))?,
        model: value["model"]
            .as_str()
            .ok_or_else(|| invalid("invalid evidence model"))?
            .into(),
        tokens: serde_json::from_value(value["tokens"].clone())
            .map_err(|_| invalid("invalid evidence tokens"))?,
        period: serde_json::from_value(value["period"].clone())
            .map_err(|_| invalid("invalid evidence period"))?,
        admission: serde_json::from_value(value["admission"].clone())
            .map_err(|_| invalid("invalid evidence admission"))?,
    };
    let _ = SessionEvent::try_from(fact.clone())?;
    Ok(fact)
}
struct PrefixEvidence {
    binding: source_admission::SourceAccountBinding,
    end: u64,
    ranges: BTreeSet<String>,
    responses: Vec<source_admission::PendingResponse>,
    numeric_records: Vec<serde_json::Value>,
}
fn range_key(source: &SessionEventSource) -> Result<String> {
    let mut values: Vec<String> = serde_json::from_str(&crate::recorded_usage::source_key(source)?)
        .map_err(|_| invalid("invalid range key"))?;
    values.pop();
    serde_json::to_string(&values).map_err(|_| invalid("invalid range key"))
}
fn prefix_evidence(
    value: &serde_json::Value,
    accepted: &BTreeSet<String>,
) -> Result<PrefixEvidence> {
    let cp = &value["checkpoint"];
    let end = cp["accepted_end"]
        .as_u64()
        .ok_or_else(|| invalid("invalid evidence prefix end"))?;
    let state = source_admission::SourceAdmission::from_storage(
        &serde_json::to_string(&value["source_admission"])
            .map_err(|_| invalid("invalid evidence parser"))?,
        end,
    )
    .ok_or_else(|| invalid("invalid evidence parser qualification"))?;
    let binding = state
        .account_binding
        .ok_or_else(|| invalid("missing evidence source binding"))?;
    let device = canonical_u64_text(
        cp["file_device"]
            .as_str()
            .ok_or_else(|| invalid("invalid evidence device"))?,
        "evidence device",
    )?;
    let inode = canonical_u64_text(
        cp["file_inode"]
            .as_str()
            .ok_or_else(|| invalid("invalid evidence inode"))?,
        "evidence inode",
    )?;
    if value["parser_version"] != "codex-info-session-recorder-v3"
        || !binding.valid(end)
        || cp["root_identity"] != binding.root_identity
        || cp["relative_path"] != binding.relative_path
        || device != binding.file_device
        || inode != binding.file_inode
        || cp["prefix_generation"] != binding.prefix_generation
    {
        return Err(invalid("evidence source identity changed"));
    }
    let mut ranges = BTreeSet::new();
    let mut cursor = 0;
    for range in value["ranges"]
        .as_array()
        .ok_or_else(|| invalid("missing evidence ranges"))?
    {
        let start = crate::canonical_u64_text(
            range["start"]
                .as_str()
                .ok_or_else(|| invalid("invalid evidence start"))?,
            "evidence start",
        )?;
        let finish = crate::canonical_u64_text(
            range["end"]
                .as_str()
                .ok_or_else(|| invalid("invalid evidence end"))?,
            "evidence end",
        )?;
        if start > cursor
            || finish <= start
            || finish > end
            || range["generation"] != binding.prefix_generation
        {
            return Err(invalid("evidence prefix has a hole"));
        }
        cursor = cursor.max(finish);
        let key = serde_json::to_string(&serde_json::json!([
            binding.root_identity,
            binding.relative_path,
            binding.file_device.to_string(),
            binding.file_inode.to_string(),
            binding.prefix_generation,
            start.to_string(),
            finish.to_string(),
            range["hash"]
        ]))
        .map_err(|_| invalid("invalid evidence range"))?;
        if !accepted.contains(&key) {
            return Err(invalid("accepted evidence range changed"));
        }
        ranges.insert(key);
    }
    if cursor != end {
        return Err(invalid("evidence prefix is incomplete"));
    }
    Ok(PrefixEvidence {
        binding,
        end,
        ranges,
        responses: state.responses,
        numeric_records: value["numeric_records"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
    })
}

#[cfg(not(test))]
type RetainedProofPayload = String;

#[cfg(not(test))]
fn retained_proof_payload(value: String) -> RetainedProofPayload {
    value
}

// Count actual owned input strings in tests, independently of the JSON
// decoder's layout. Release builds contain neither this owner nor statistics.
#[cfg(test)]
thread_local! {
    static PROOF_PAYLOAD_BYTES: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
    static CANDIDATE_PAYLOAD_OWNERS: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
}

#[cfg(test)]
struct RetainedProofPayload(String);

#[cfg(test)]
fn retained_proof_payload(value: String) -> RetainedProofPayload {
    CANDIDATE_PAYLOAD_OWNERS.with(|counter| {
        let (v1, v2) = counter.get();
        counter.set((
            v1 + usize::from(value.as_bytes().ends_with(b",\"version\":1}")),
            v2 + usize::from(value.as_bytes().ends_with(b",\"version\":2}")),
        ));
    });
    PROOF_PAYLOAD_BYTES.with(|counter| {
        let (live, peak) = counter.get();
        let live = live
            .checked_add(value.len())
            .expect("test payload byte count");
        counter.set((live, peak.max(live)));
    });
    RetainedProofPayload(value)
}

#[cfg(test)]
impl std::ops::Deref for RetainedProofPayload {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl Drop for RetainedProofPayload {
    fn drop(&mut self) {
        PROOF_PAYLOAD_BYTES.with(|counter| {
            let (live, peak) = counter.get();
            counter.set((
                live.checked_sub(self.0.len()).expect("test payload owner"),
                peak,
            ));
        });
    }
}

fn validated_equivalence_payload(id: &str, payload: &str, sha: &str) -> Result<serde_json::Value> {
    if id != sha || format!("{:x}", Sha256::digest(payload.as_bytes())) != sha {
        return Err(invalid("equivalence payload hash changed"));
    }
    let value: serde_json::Value = codex_info_db_reader::decode_source_repair_v2(payload)
        .map_err(|_| invalid("invalid equivalence payload"))?;
    if !matches!(
        value["mode"].as_str(),
        Some("identical-prefix-alias" | "event-bijection")
    ) || !value["mappings"].is_array()
        || !value["original"].is_array()
        || !value["prefixes"].is_array()
    {
        return Err(invalid("invalid equivalence payload shape"));
    }
    Ok(value)
}

const EQUIVALENCE_PROOF_QUERY: &str = codex_info_db_reader::SOURCE_REPAIR_V2_CANDIDATE_QUERY;

type EquivalenceProofRow = (String, RetainedProofPayload, String);

fn next_equivalence_proof(rows: &mut rusqlite::Rows<'_>) -> Result<Option<EquivalenceProofRow>> {
    while let Some(row) = rows.next()? {
        let payload = retained_proof_payload(row.get::<_, String>(1)?);
        if codex_info_db_reader::source_repair_is_v2_candidate(&payload)
            .map_err(|_| invalid("invalid equivalence payload"))?
        {
            return Ok(Some((row.get(0)?, payload, row.get(2)?)));
        }
    }
    Ok(None)
}

fn load_snapshot(connection: &Connection) -> Result<CanonicalSourceSnapshot> {
    #[cfg(test)]
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(count.get() + 1));
    let mut snapshot = CanonicalSourceSnapshot::default();
    let exists:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='source_prefix_repairs')",[],|r|r.get(0))?;
    if !exists {
        return Ok(snapshot);
    }
    let mut proof_inputs = Vec::new();
    let mut referenced = BTreeSet::<[String; 4]>::new();
    // Retain only small references between the two passes. Both the raw
    // string and decoded proof die before the cursor advances to another row.
    {
        let mut query = connection.prepare(EQUIVALENCE_PROOF_QUERY)?;
        let mut rows = query.query([])?;
        while let Some((id, payload, sha)) = next_equivalence_proof(&mut rows)? {
            let value = validated_equivalence_payload(&id, &payload, &sha)?;
            for original in value["original"].as_array().expect("validated") {
                let key: Vec<String> =
                    serde_json::from_str(&source_pointer_key(&original["source"])?)
                        .map_err(|_| invalid("invalid original key"))?;
                referenced.insert([
                    key[0].clone(),
                    key[1].clone(),
                    key[2].clone(),
                    key[3].clone(),
                ]);
            }
            for mapping in value["mappings"].as_array().expect("validated") {
                let key: Vec<String> = serde_json::from_str(&mapping_key(&mapping["canonical"])?)
                    .map_err(|_| invalid("invalid canonical key"))?;
                referenced.insert([
                    key[0].clone(),
                    key[1].clone(),
                    key[2].clone(),
                    key[3].clone(),
                ]);
            }
            for prefix in value["prefixes"].as_array().expect("validated") {
                let cp = &prefix["checkpoint"];
                referenced.insert([
                    cp["root_identity"]
                        .as_str()
                        .ok_or_else(|| invalid("invalid prefix root"))?
                        .into(),
                    cp["relative_path"]
                        .as_str()
                        .ok_or_else(|| invalid("invalid prefix path"))?
                        .into(),
                    cp["file_device"]
                        .as_str()
                        .ok_or_else(|| invalid("invalid prefix device"))?
                        .into(),
                    cp["file_inode"]
                        .as_str()
                        .ok_or_else(|| invalid("invalid prefix inode"))?
                        .into(),
                ]);
            }
            proof_inputs.push((id, sha));
        }
    }
    if proof_inputs.is_empty() {
        return Ok(snapshot);
    }
    // One indexed inventory query for the referenced physical sources; no
    // SELECT, JSON parse or provenance verification occurs per fact below.
    let references = serde_json::to_string(&referenced)
        .map_err(|_| invalid("invalid equivalence inventory"))?
        .replace('\'', "''");
    let filter=format!("WHERE (root_identity,relative_path,file_device,file_inode) IN (SELECT json_extract(value,'$[0]'),json_extract(value,'$[1]'),json_extract(value,'$[2]'),json_extract(value,'$[3]') FROM json_each('{references}'))");
    let mut accepted = BTreeSet::new();
    let range_filter=format!("SELECT root_identity,relative_path,file_device,file_inode,start_offset,end_offset,collector_epoch,cycle_seq,prefix_generation,record_sha256 FROM session_ranges {filter}");
    let mut range_query = connection.prepare(&range_filter)?;
    for range in range_query
        .query_map([], SessionRange::from_sql_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?
    {
        validate_session_range(&range)?;
        let source = SessionEventSource {
            root_identity: range.root_identity,
            relative_path: range.relative_path,
            file_device: range.file_device,
            file_inode: range.file_inode,
            prefix_generation: range.prefix_generation,
            range_start: range.start_offset,
            range_end: range.end_offset,
            record_sha256: range.record_sha256,
            event_index: 0,
        };
        accepted.insert(range_key(&source)?);
    }
    snapshot.proofs = proof_inputs.len();
    let mut raw = BTreeMap::new();
    for fact in crate::source_events::read_matching_raw(connection, &filter, [])? {
        raw.insert(crate::recorded_usage::source_key(&fact.source)?, fact);
    }
    let mut query = connection.prepare(EQUIVALENCE_PROOF_QUERY)?;
    let mut rows = query.query([])?;
    let mut processed = 0;
    while let Some((id, payload, sha)) = next_equivalence_proof(&mut rows)? {
        let value = validated_equivalence_payload(&id, &payload, &sha)?;
        if !proof_inputs
            .get(processed)
            .is_some_and(|(expected_id, expected_sha)| *expected_id == id && *expected_sha == sha)
        {
            return Err(invalid("equivalence payload set changed between passes"));
        }
        processed += 1;
        let evidence = value["prefixes"]
            .as_array()
            .expect("validated")
            .iter()
            .map(|prefix| prefix_evidence(prefix, &accepted))
            .collect::<Result<Vec<_>>>()?;
        let mut original_keys = BTreeSet::new();
        for original in value["original"].as_array().expect("validated") {
            let key = source_pointer_key(&original["source"])?;
            let fact = raw
                .get(&key)
                .ok_or_else(|| invalid("equivalence original pointer disappeared"))?;
            let saved = fact_json(fact);
            if saved["timestamp"] != original["timestamp"]
                || saved["model"] != original["model"]
                || saved["tokens"] != original["tokens"]
            {
                return Err(invalid("equivalence original inventory changed"));
            }
            original_keys.insert(key);
        }
        for mapping in value["mappings"].as_array().expect("validated") {
            let original = mapping_key(&mapping["original"])?;
            let canonical = mapping_key(&mapping["canonical"])?;
            let fact = raw
                .get(&canonical)
                .ok_or_else(|| invalid("equivalence canonical pointer disappeared"))?;
            if !original_keys.contains(&original) {
                return Err(invalid("equivalence mapping lost its original inventory"));
            }
            let qualified = qualified_fact(&mapping["qualified"])?;
            match mapping["canonical_kind"].as_str() {
                Some("retained-response") => {
                    if original != canonical
                        || !matches!(
                            fact.admission.as_ref().map(|a| &a.numeric),
                            Some(NumericEventProof::ResponseRecord { .. })
                        )
                        || !same_value(fact, &qualified)
                        || !evidence.iter().any(|prefix| {
                            qualified.admission.as_ref().is_some_and(|a| {
                                a.source_binding == prefix.binding
                                    && a.verified_prefix_end == prefix.end
                            }) && range_key(&qualified.source)
                                .is_ok_and(|key| prefix.ranges.contains(&key))
                        })
                    {
                        return Err(invalid("retained response evidence changed"));
                    }
                    let saved = value["original"]
                        .as_array()
                        .expect("validated")
                        .iter()
                        .find(|saved| {
                            source_pointer_key(&saved["source"]).ok().as_ref() == Some(&original)
                        })
                        .ok_or_else(|| invalid("missing retained response admission"))?;
                    if !same_admission_without_period(fact.admission.as_ref(), &saved["admission"])
                    {
                        return Err(invalid("retained original response admission changed"));
                    }
                }
                Some("qualified-source") => {
                    if mapping["qualified"]["source"] != mapping["canonical"]
                        || fact.timestamp != qualified.timestamp
                        || fact.model != qualified.model
                        || fact.tokens != qualified.tokens
                        || !same_admission_without_period(
                            fact.admission.as_ref(),
                            &mapping["qualified"]["admission"],
                        )
                    {
                        return Err(invalid("equivalence canonical row changed"));
                    }
                    if value["mode"] == "event-bijection"
                        && !evidence.iter().any(|prefix| {
                            qualified.admission.as_ref().is_some_and(|admission| {
                                admission.source_binding == prefix.binding
                                    && admission.verified_prefix_end == prefix.end
                            }) && range_key(&qualified.source)
                                .is_ok_and(|key| prefix.ranges.contains(&key))
                                && same_bijection_value(
                                    raw.get(&original).expect("original validated"),
                                    &qualified,
                                    &prefix.responses,
                                    &prefix.numeric_records,
                                )
                        })
                    {
                        return Err(invalid("event rebind counterpart evidence changed"));
                    }
                }
                _ => return Err(invalid("invalid equivalence canonical kind")),
            }
            if original != canonical {
                let old = raw.get(&original).expect("original validated");
                snapshot.retired.insert(original.clone());
                snapshot
                    .replays
                    .insert(original, (old.timestamp, old.model.clone(), old.tokens));
            }
        }
        for (field, models) in [("fixed", false), ("models", true)] {
            for row in value["price_inputs"][field]
                .as_array()
                .ok_or_else(|| invalid("missing saved observation inventory"))?
            {
                let reset = row["reset_at"]
                    .as_i64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| invalid("invalid saved observation reset"))?;
                let timestamp = row["timestamp"]
                    .as_i64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| invalid("invalid saved observation time"))?;
                if models {
                    let model = row["model"]
                        .as_str()
                        .ok_or_else(|| invalid("invalid saved model observation"))?;
                    snapshot
                        .saved_models
                        .entry((reset, timestamp))
                        .or_default()
                        .insert(model.to_owned());
                } else {
                    snapshot.saved_observations.insert((reset, timestamp));
                }
            }
        }
        for original in value["original"].as_array().expect("validated") {
            let key: Vec<String> = serde_json::from_str(&source_pointer_key(&original["source"])?)
                .map_err(|_| invalid("invalid retained checkpoint source"))?;
            snapshot.checkpoint_references.insert([
                key[0].clone(),
                key[1].clone(),
                key[2].clone(),
                key[3].clone(),
                key[4].clone(),
            ]);
        }
        for prefix in value["prefixes"].as_array().expect("validated") {
            let cp = &prefix["checkpoint"];
            snapshot.checkpoint_references.insert([
                cp["root_identity"]
                    .as_str()
                    .ok_or_else(|| invalid("invalid retained checkpoint root"))?
                    .to_owned(),
                cp["relative_path"]
                    .as_str()
                    .ok_or_else(|| invalid("invalid retained checkpoint path"))?
                    .to_owned(),
                cp["file_device"]
                    .as_str()
                    .ok_or_else(|| invalid("invalid retained checkpoint device"))?
                    .to_owned(),
                cp["file_inode"]
                    .as_str()
                    .ok_or_else(|| invalid("invalid retained checkpoint inode"))?
                    .to_owned(),
                cp["prefix_generation"]
                    .as_str()
                    .ok_or_else(|| invalid("invalid retained checkpoint generation"))?
                    .to_owned(),
            ]);
            snapshot.prefixes.insert(
                serde_json::to_string(&prefix["checkpoint"])
                    .map_err(|_| invalid("invalid recorded prefix"))?,
            );
        }
    }
    if processed != proof_inputs.len() {
        return Err(invalid("equivalence payload set changed between passes"));
    }
    Ok(snapshot)
}

/// The right-hand 9-column list is uncorrelated and materialized once by
/// SQLite as a membership index, rather than expanding mappings per raw row.
pub(crate) fn canonical_table(connection: &Connection) -> Result<String> {
    let scope = CanonicalSourceScope::begin(connection)?;
    let snapshot = scope.snapshot.borrow();
    if snapshot.proofs == 0 {
        return Ok("session_events".into());
    }
    // Each key is already a validated compact nine-string JSON array. Build
    // the small membership list from that view, without reopening proof JSON.
    let retired = format!(
        "[{}]",
        snapshot
            .retired
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",")
    )
    .replace('\'', "''");
    let columns = [
        "root_identity",
        "relative_path",
        "file_device",
        "file_inode",
        "prefix_generation",
        "range_start",
        "range_end",
        "record_sha256",
        "event_index",
    ];
    let lhs = columns
        .iter()
        .map(|c| format!("e.{c}"))
        .collect::<Vec<_>>()
        .join(",");
    let rhs = (0..9)
        .map(|i| {
            let value = format!("json_extract(m.value,'$[{i}]')");
            if matches!(i, 5 | 6 | 8) {
                format!("CAST({value} AS INTEGER)")
            } else {
                value
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!("(SELECT e.* FROM session_events e WHERE ({lhs}) NOT IN (SELECT {rhs} FROM json_each('{retired}') m))"))
}
pub(crate) fn is_retired(connection: &Connection, source: &SessionEventSource) -> Result<bool> {
    let scope = CanonicalSourceScope::begin(connection)?;
    let retired = scope.snapshot.borrow().is_retired(source)?;
    Ok(retired)
}

pub(super) fn apply_equivalence(
    tx: &rusqlite::Transaction<'_>,
    repair: &PreparedSourceQualificationRepair,
    prepared: &period_evidence::PreparedPeriodAttribution,
) -> Result<BTreeMap<(i64, i64), i64>> {
    let _scope = CanonicalSourceScope::begin(tx)?;
    let proof = repair.equivalence.as_ref().expect("equivalence mode");
    if let Some((payload, sha)) = tx
        .query_row(
            "SELECT payload_json,payload_sha256 FROM source_prefix_repairs WHERE repair_id=?1",
            [&repair.repair_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if payload != repair.provenance || sha != repair.repair_id {
            return Err(invalid("equivalence acknowledgement changed"));
        }
        return Ok(BTreeMap::new());
    }
    if repair.backup.partition_id != prepared.partition().partition_id
        || !repair.calculation_inputs.matches(tx)?
        || physical_events(tx, &repair.checkpoint)? != proof.inventory
    {
        return Err(invalid(
            "equivalence physical inventory changed before commit",
        ));
    }
    for (checkpoint, hash) in &proof.fingerprints {
        if fingerprint(tx, checkpoint)? != *hash {
            return Err(invalid("equivalence accepted prefix changed before commit"));
        }
    }
    validate_source_binding_partitions(
        tx,
        proof.prefixes.iter().map(|p| &p.verification_checkpoint),
    )?;
    let mut raw = BTreeMap::new();
    let mut qualified = Vec::new();
    for fact in &proof.canonical {
        let existing=crate::source_events::read_matching_raw(tx,"WHERE root_identity=?1 AND relative_path=?2 AND file_device=?3 AND file_inode=?4 AND prefix_generation=?5 AND range_start=?6 AND range_end=?7 AND record_sha256=?8 AND event_index=?9",params![fact.source.root_identity,fact.source.relative_path,fact.source.file_device.to_string(),fact.source.file_inode.to_string(),format!("{:032x}",fact.source.prefix_generation),fact.source.range_start,fact.source.range_end,fact.source.record_sha256,fact.source.event_index])?;
        if let Some(old) = existing.first() {
            if !same_value(old, fact) {
                return Err(invalid("canonical equivalence row conflicts"));
            }
            if old.admission.is_some() {
                continue;
            }
        }
        let event = SessionEvent::try_from(fact.clone())?;
        raw.insert(session_event_primary_key(&event), event);
        qualified.push(fact.clone());
    }
    upsert_session_events(tx, &raw)?;
    upsert_session_event_qualifications(
        tx,
        &qualified,
        false,
        "source-equivalence",
        Some(prepared),
    )?;
    tx.execute(
        "INSERT INTO source_prefix_repairs(repair_id,payload_json,payload_sha256) VALUES(?1,?2,?1)",
        params![repair.repair_id, repair.provenance],
    )?;
    refresh_scope(tx)?;
    for mapping in &proof.mappings {
        if mapping["original"] != mapping["canonical"] {
            let old = proof
                .inventory
                .iter()
                .find(|old| pointer(&old.source).ok().as_ref() == Some(&mapping["original"]))
                .ok_or_else(|| invalid("equivalence lost old pointer"))?;
            crate::recorded_usage::remove(tx, &old.source)?;
        }
    }
    for fact in &proof.canonical {
        crate::recorded_usage::observe(tx, fact)?;
    }
    // Equivalence changes the canonical fact view, not the authority of
    // already saved legacy observations. Those cells remain raw history;
    // unlike a v1 numerical correction, this proof grants no rewrite of them.
    Ok(BTreeMap::new())
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn issue604_proof_candidates_preserve_versions_without_sql_json_reparse() {
        let connection = Connection::open_in_memory().expect("memory database");
        connection
            .execute_batch(super::super::SCHEMA)
            .expect("repair schema");
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
                    params![id, payload],
                )
                .expect("literal candidate metadata");
        }
        CANDIDATE_PAYLOAD_OWNERS.with(|counter| counter.set((0, 0)));
        let mut selected = BTreeSet::new();
        {
            let mut statement = connection
                .prepare(EQUIVALENCE_PROOF_QUERY)
                .expect("candidate statement");
            let mut rows = statement.query([]).expect("candidate cursor");
            while let Some((id, _payload, _sha)) =
                next_equivalence_proof(&mut rows).expect("candidate selection")
            {
                assert!(selected.insert(id));
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
                "invalid equivalence payload",
            ),
            (
                r#"{"mappings":[],"mode":"event-bijection","original":[],"prefixes":[],"version":2}"#,
                "0000000000000000000000000000000000000000000000000000000000000000",
                "equivalence payload hash changed",
            ),
        ] {
            connection
                .execute("DELETE FROM source_prefix_repairs", [])
                .expect("clear literal candidates");
            connection
                .execute(
                    "INSERT INTO source_prefix_repairs VALUES(?1,?2,?1)",
                    params![sha, payload],
                )
                .expect("negative v2 row");
            let result = load_snapshot(&connection);
            assert!(
                matches!(result, Err(UsageStoreError::InvalidImport(ref message)) if message == expected_error),
                "malformed or mismatched v2 remains rejected"
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
        let version_rejections = invalid_versions.map(|(name, payload)| {
            (
                name,
                codex_info_db_reader::decode_source_repair_v2(payload).is_err(),
            )
        });
        for payload in [
            r#"{"version":2,"body":{"version":1}}"#,
            " { \"body\" : [true], \"version\" : 2 } ",
        ] {
            let value = codex_info_db_reader::decode_source_repair_v2(payload)
                .expect("one integer root version 2");
            assert_eq!(value["version"].as_u64(), Some(2));
        }
        let json_functions = connection
            .prepare(&format!("EXPLAIN {EQUIVALENCE_PROOF_QUERY}"))
            .expect("bytecode")
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, Option<String>>(5)?))
            })
            .expect("bytecode rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("bytecode values")
            .into_iter()
            .filter_map(|(opcode, p4)| {
                p4.filter(|name| {
                    matches!(opcode.as_str(), "Function" | "PureFunc") && name.starts_with("json_")
                })
            })
            .collect::<Vec<_>>();
        assert!(
            json_functions.is_empty(),
            "no SQLite JSON proof parse: {json_functions:?}"
        );
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
    fn issue604_proof_payload_residency() {
        let connection = Connection::open_in_memory().expect("memory database");
        connection
            .execute_batch(super::super::SCHEMA)
            .expect("repair schema");
        for mode in ["invalid-first", "invalid-second"] {
            // Valid JSON and matching content identity, deliberately invalid
            // proof mode. These rows must never authorize a source repair.
            let payload = serde_json::to_string(&serde_json::json!({
                "version":2,"mode":mode,"prefixes":[],"original":[],"mappings":[]
            }))
            .expect("negative fixture JSON");
            let id = format!("{:x}", Sha256::digest(payload.as_bytes()));
            connection.execute(
                "INSERT INTO source_prefix_repairs(repair_id,payload_json,payload_sha256) VALUES (?1,?2,?1)",
                params![id,payload],
            ).expect("negative saved row");
        }
        let maximum: i64 = connection
            .query_row(
                "SELECT max(length(CAST(payload_json AS BLOB))) FROM source_prefix_repairs",
                [],
                |row| row.get(0),
            )
            .expect("independent stored byte size");
        PROOF_PAYLOAD_BYTES.with(|counter| {
            assert_eq!(counter.get().0, 0);
            counter.set((0, 0));
        });
        assert!(
            load_snapshot(&connection).is_err(),
            "invalid mode must be rejected"
        );
        let (live, peak) = PROOF_PAYLOAD_BYTES.with(std::cell::Cell::get);
        assert_eq!(live, 0, "rejected payload ownership must be released");
        assert!(
            peak <= usize::try_from(maximum).expect("stored byte size"),
            "payload UTF8 residency {peak} exceeds one stored row {maximum}"
        );
    }
}

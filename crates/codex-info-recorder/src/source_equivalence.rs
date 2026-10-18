//! Startup/normal recovery of fully proved source equivalence. This module
//! reads only recorder-owned accepted prefixes; it never substitutes a live
//! account RPC for historical source/owner evidence.
use super::*;
use codex_info_db_writer::{SourceEquivalenceMode, SourceEquivalencePrefix};

type EquivalencePreparation = (
    Vec<codex_info_db_writer::PreparedSourceQualificationRepair>,
    BTreeSet<SourceVerificationKey>,
    u64,
);

#[derive(PartialEq)]
pub(super) struct RejectedSourceEquivalence {
    mode: SourceEquivalenceMode,
    fingerprint: SourceFingerprint,
    checkpoints: Vec<SessionCheckpoint>,
    ranges: Vec<Vec<SessionRange>>,
    inventory_sha256: [u8; 32],
    account_boundary_epoch: Option<u128>,
    lifecycle_intervals: Vec<SessionLifecycleInterval>,
}

fn inventory_sha256(events: &[LosslessSessionEvent]) -> Result<[u8; 32], RecorderError> {
    let mut digest = Sha256::new();
    for event in events {
        let encoded = serde_json::to_vec(&(
            &event.source,
            event.timestamp,
            &event.model,
            &event.tokens,
            &event.period,
            &event.admission,
        ))
        .map_err(|_| RecorderError::Invalid("equivalence inventory cannot be encoded".into()))?;
        digest.update((encoded.len() as u64).to_be_bytes());
        digest.update(encoded);
    }
    Ok(digest.finalize().into())
}

impl Recorder {
    pub(super) fn prepare_source_equivalences(
        &mut self,
        state: &SessionCollectionState,
        period: &codex_info_db_writer::period_evidence::PreparedPeriodAttribution,
        inputs: &mut Option<codex_info_db_writer::PreparedSourceRepairInputs>,
    ) -> Result<EquivalencePreparation, RecorderError> {
        let identity = self
            .identity
            .as_ref()
            .ok_or_else(|| {
                RecorderError::Invalid("source equivalence requires a partition".into())
            })?
            .clone();
        let database = self
            .database
            .as_ref()
            .ok_or_else(|| RecorderError::Invalid("source equivalence has no database".into()))?
            .clone();
        let root = self.config.sessions_root.canonicalize()?;
        let root_key = root_identity(&root, &fs::symlink_metadata(&root)?);
        let mut groups = BTreeMap::<(String, String), Vec<&SessionCheckpoint>>::new();
        for cp in &state.checkpoints {
            if cp.root_identity == root_key && cp.committed_offset > 0 {
                groups
                    .entry((cp.root_identity.clone(), cp.relative_path.clone()))
                    .or_default()
                    .push(cp);
            }
        }
        let canonical_sources = self.writer.canonical_source_snapshot()?;
        let mut repairs = Vec::new();
        let mut handled = BTreeSet::new();
        let mut consumed = 0;
        let mut resumed_inputs = 0_u64;
        let mut restarted_inputs = 0_u64;
        let mut replayed_bytes = 0_u64;
        for ((_, relative), checkpoints) in groups {
            let namespace = (root_key.clone(), relative.clone());
            let path = root.join(relative);
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || path.canonicalize().ok().as_ref() != Some(&path)
            {
                continue;
            }
            let source = Source {
                path,
                recorded: recorded_source(
                    root_key.clone(),
                    checkpoints[0].relative_path.clone(),
                    metadata,
                ),
            };
            let current = checkpoints
                .iter()
                .copied()
                .filter(|cp| {
                    (cp.file_device, cp.file_inode)
                        == (source.recorded.file_device, source.recorded.file_inode)
                })
                .collect::<Vec<_>>();
            if current.is_empty() {
                continue;
            }
            let old_physical = checkpoints
                .iter()
                .copied()
                .filter(|cp| {
                    (cp.file_device, cp.file_inode)
                        != (source.recorded.file_device, source.recorded.file_inode)
                })
                .collect::<Vec<_>>();
            let (mode, old, prefix_checkpoints) = if current.len() > 1 {
                (
                    SourceEquivalenceMode::IdenticalPrefixAlias,
                    current[0],
                    current.clone(),
                )
            } else if let Some(old) = old_physical.first() {
                (SourceEquivalenceMode::EventBijection, *old, current.clone())
            } else {
                continue;
            };
            // Every targeted generation remains pending until this whole proof
            // succeeds. Generic single-prefix repair cannot silently replace it.
            for cp in &checkpoints {
                handled.insert(source_verification_key(cp));
            }
            let mut recorded = false;
            for cp in &current {
                recorded |= canonical_sources.prefix_recorded(cp)?;
            }
            if recorded {
                continue;
            }
            // Keep later groups pending without opening their full accepted
            // prefixes after this round's reconstruction budget is consumed.
            if consumed >= self.config.chunk_bytes {
                continue;
            }
            let Some(fingerprint) = source_fingerprint(&source)? else {
                continue;
            };
            let rejected_input = RejectedSourceEquivalence {
                mode,
                fingerprint,
                checkpoints: checkpoints.iter().map(|cp| (*cp).clone()).collect(),
                ranges: checkpoints
                    .iter()
                    .map(|cp| self.writer.load_session_ranges_for_checkpoint(cp))
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                inventory_sha256: inventory_sha256(
                    &self.writer.source_physical_event_inventory(old)?,
                )?,
                account_boundary_epoch: self.account_boundary_epoch,
                lifecycle_intervals: self.lifecycle_intervals.clone(),
            };
            // A complete counterpart mismatch cannot change with the same
            // bytes, accepted prefixes, old values and owner. Keep it pending
            // without letting an unchanged historical proof block collection.
            if self.rejected_source_equivalences.get(&namespace) == Some(&rejected_input) {
                continue;
            }
            self.rejected_source_equivalences.remove(&namespace);
            let mut prefixes = Vec::new();
            for cp in prefix_checkpoints {
                let accepted = self.writer.load_session_ranges_for_checkpoint(cp)?;
                if saved_source_prefix_matches(&source, cp, &accepted)? != Some(true) {
                    break;
                }
                let key = source_verification_key(cp);
                let fingerprint = source_fingerprint(&source)?;
                let reusable = self
                    .source_original_reconstructions
                    .get(&key)
                    .is_some_and(|r| {
                        r.matches(
                            cp,
                            &accepted,
                            self.account_boundary_epoch,
                            &self.lifecycle_intervals,
                        ) && r.read_container.is_none()
                    });
                if !reusable {
                    if self
                        .source_original_reconstructions
                        .get(&key)
                        .is_some_and(|old| {
                            old.replayed
                                .as_ref()
                                .is_some_and(|cp| cp.committed_offset > 0)
                        })
                    {
                        restarted_inputs += 1;
                    }
                    self.source_original_reconstructions.insert(
                        key.clone(),
                        SourceParserReconstruction {
                            original: cp.clone(),
                            ranges: accepted.clone(),
                            fingerprint: fingerprint.clone(),
                            account_boundary_epoch: self.account_boundary_epoch,
                            lifecycle_intervals: self.lifecycle_intervals.clone(),
                            replayed: None,
                            pending: Vec::new(),
                            raw_conflict: false,
                            qualification_attempted: false,
                            read_container: None,
                            numeric_events: Vec::new(),
                            counter_proofs: Vec::new(),
                            numeric_records: Vec::new(),
                        },
                    );
                }
                let eligible_from = cp
                    .source_admission
                    .as_ref()
                    .and_then(|s| s.account_binding.as_ref())
                    .and_then(|binding| match binding.proof {
                        SourceBindingProof::Rpc { began_at, .. } => Some(began_at),
                        _ => None,
                    });
                let ownership = SourceOwnership {
                    intervals: &self.lifecycle_intervals,
                    eligible_from,
                };
                let reconstruction = self
                    .source_original_reconstructions
                    .get_mut(&key)
                    .expect("inserted above");
                // The complete accepted [0,N) was just verified above. A
                // later append changes the file fingerprint, but not this
                // fixed parser input. Retain its bounded replay position;
                // checkpoint/range/owner changes still create a new replay.
                reconstruction.fingerprint = fingerprint.clone();
                let replayed_before = reconstruction
                    .replayed
                    .as_ref()
                    .map_or(0, |checkpoint| checkpoint.committed_offset);
                if reusable && replayed_before > 0 {
                    resumed_inputs += 1;
                }
                let budget = self.config.chunk_bytes.saturating_sub(consumed);
                if budget == 0 {
                    break;
                }
                consumed +=
                    reconstruction.advance(&source, budget, cp.cycle_seq, Some(&ownership))?;
                let replayed_after = reconstruction
                    .replayed
                    .as_ref()
                    .map_or(0, |checkpoint| checkpoint.committed_offset);
                replayed_bytes =
                    replayed_bytes.saturating_add(replayed_after.saturating_sub(replayed_before));
                let Some(mut verified) = reconstruction.qualified_checkpoint(cp.cycle_seq) else {
                    break;
                };
                let Some(binding) = historical_source_binding(
                    &identity,
                    &source,
                    &verified,
                    &self.lifecycle_intervals,
                )?
                else {
                    break;
                };
                verified
                    .source_admission
                    .as_mut()
                    .expect("qualified above")
                    .account_binding = Some(binding.clone());
                let facts = qualified_replayed_facts(
                    reconstruction,
                    &accepted,
                    &binding,
                    &ownership,
                    period,
                )?;
                if source_fingerprint(&source)? != fingerprint
                    || saved_source_prefix_matches(&source, cp, &accepted)? != Some(true)
                {
                    break;
                }
                prefixes.push(SourceEquivalencePrefix {
                    checkpoint: cp.clone(),
                    verification_checkpoint: verified,
                    ranges: accepted,
                    qualified_events: facts,
                    numeric_records: reconstruction.numeric_records.clone(),
                    parser_version: PARSER_VERSION.to_owned(),
                });
            }
            let expected = if mode == SourceEquivalenceMode::IdenticalPrefixAlias {
                current.len()
            } else {
                1
            };
            if prefixes.len() != expected {
                continue;
            }
            if inputs.is_none() {
                let backup =
                    UsageStore::backup_generations_partitioned_verified(&database, &identity, 3)?;
                *inputs = Some(self.writer.prepare_source_repair_inputs(backup)?);
            }
            match self.writer.prepare_source_equivalence(
                mode,
                old,
                prefixes,
                inputs.as_ref().expect("prepared").clone(),
            ) {
                Ok(repair) => repairs.push(repair),
                Err(error) => {
                    if matches!(&error, UsageStoreError::InvalidImport(message)
                        if message == "old event does not have exactly one accepted counterpart")
                        && source_fingerprint(&source)?.as_ref()
                            == Some(&rejected_input.fingerprint)
                    {
                        self.rejected_source_equivalences
                            .insert(namespace, rejected_input);
                    }
                    eprintln!("recorder accepted source equivalence remains pending: {error}")
                }
            }
        }
        let mut unfinished_inputs = 0_u64;
        let mut unfinished_bytes = 0_u64;
        for reconstruction in self.source_original_reconstructions.values() {
            let at = reconstruction
                .replayed
                .as_ref()
                .map_or(0, |cp| cp.committed_offset);
            if at < reconstruction.original.committed_offset {
                unfinished_inputs += 1;
                unfinished_bytes =
                    unfinished_bytes.saturating_add(reconstruction.original.committed_offset - at);
            }
        }
        eprintln!("codex-info-recorder diagnostic source_equivalence_progress resumed_inputs={resumed_inputs} restarted_inputs={restarted_inputs} replayed_bytes={replayed_bytes} unfinished_inputs={unfinished_inputs} unfinished_bytes={unfinished_bytes}");
        Ok((repairs, handled, consumed))
    }
}

fn qualified_replayed_facts(
    reconstruction: &SourceParserReconstruction,
    ranges: &[SessionRange],
    binding: &SourceAccountBinding,
    ownership: &SourceOwnership<'_>,
    period: &codex_info_db_writer::period_evidence::PreparedPeriodAttribution,
) -> Result<Vec<LosslessSessionEvent>, RecorderError> {
    let checkpoint = &reconstruction.original;
    let replay = reconstruction
        .replayed
        .as_ref()
        .ok_or_else(|| RecorderError::Invalid("source replay is incomplete".into()))?;
    let mut facts = Vec::new();
    for (raw, proof) in &reconstruction.numeric_events {
        if replay.source_admission.as_ref().is_some_and(|state| {
            state.responses.iter().any(|response| {
                response.qualified_model().is_some()
                    && response.matched_counter() == Some(&proof.after.pointer)
            })
        }) {
            continue;
        }
        let owners = ownership
            .intervals
            .iter()
            .filter(|owner| {
                owner.start_at.is_some_and(|start| {
                    proof.before.pointer.timestamp >= start
                        && proof.after.pointer.timestamp >= start
                }) && owner.end_at.is_none_or(|end| {
                    proof.before.pointer.timestamp < end && proof.after.pointer.timestamp < end
                })
            })
            .collect::<Vec<_>>();
        if owners.len() != 1
            || !ownership.contains(proof.before.pointer.timestamp)
            || !ownership.contains(proof.after.pointer.timestamp)
        {
            continue;
        }
        let range = ranges
            .iter()
            .find(|r| {
                r.start_offset <= proof.after.pointer.start
                    && proof.after.pointer.end <= r.end_offset
            })
            .ok_or_else(|| RecorderError::Invalid("replayed event has no accepted range".into()))?;
        let mut fact = LosslessSessionEvent::from(raw.clone());
        fact.source = codex_info_db_writer::SessionEventSource {
            root_identity: range.root_identity.clone(),
            relative_path: range.relative_path.clone(),
            file_device: range.file_device,
            file_inode: range.file_inode,
            prefix_generation: range.prefix_generation,
            range_start: range.start_offset,
            range_end: range.end_offset,
            record_sha256: range.record_sha256.clone(),
            event_index: proof.after.pointer.start,
        };
        fact.admission = Some(EventAdmission {
            version: 1,
            source_binding: binding.clone(),
            verified_prefix_end: checkpoint.committed_offset,
            ownership: EventOwnership {
                start_at: owners[0].start_at.expect("finite owner"),
                end_at: owners[0].end_at,
            },
            numeric: NumericEventProof::LegacyCounter(Box::new(proof.clone())),
            period: None,
        });
        period.assign(&mut fact)?;
        facts.push(fact);
    }
    for candidate in replay
        .source_admission
        .as_ref()
        .into_iter()
        .flat_map(|state| &state.responses)
    {
        let Some(model) = candidate.qualified_model() else {
            continue;
        };
        let owners = ownership
            .intervals
            .iter()
            .filter(|owner| {
                owner
                    .start_at
                    .is_some_and(|start| candidate.pointer.timestamp >= start)
                    && owner
                        .end_at
                        .is_none_or(|end| candidate.pointer.timestamp < end)
            })
            .collect::<Vec<_>>();
        if owners.len() != 1 || !ownership.contains(candidate.pointer.timestamp) {
            continue;
        }
        let range = ranges
            .iter()
            .find(|r| {
                r.start_offset <= candidate.pointer.start && candidate.pointer.end <= r.end_offset
            })
            .ok_or_else(|| RecorderError::Invalid("response has no accepted range".into()))?;
        let usage = candidate.record.usage;
        let mut fact = LosslessSessionEvent::from(SessionEvent {
            root_identity: range.root_identity.clone(),
            relative_path: range.relative_path.clone(),
            file_device: range.file_device,
            file_inode: range.file_inode,
            prefix_generation: range.prefix_generation,
            range_start: range.start_offset,
            range_end: range.end_offset,
            record_sha256: range.record_sha256.clone(),
            event_index: candidate.pointer.start,
            timestamp: candidate.pointer.timestamp,
            model: model.into(),
            total_tokens: usage.total,
            input_tokens: Some(usage.input),
            cached_input_tokens: Some(usage.cached_input),
            cache_write_input_tokens: Some(usage.cache_write_input),
            output_tokens: Some(usage.output),
        });
        fact.admission = Some(EventAdmission {
            version: 1,
            source_binding: binding.clone(),
            verified_prefix_end: checkpoint.committed_offset,
            ownership: EventOwnership {
                start_at: owners[0].start_at.expect("finite owner"),
                end_at: owners[0].end_at,
            },
            numeric: NumericEventProof::ResponseRecord {
                candidate: Box::new(candidate.clone()),
                legacy_family_exact_model: None,
            },
            period: None,
        });
        period.assign(&mut fact)?;
        facts.push(fact);
    }
    Ok(facts)
}

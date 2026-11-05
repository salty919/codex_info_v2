//! Persisted evidence attached to the original numeric event, not another
//! numeric ledger. Shape validation does not replace producer source checks.

use crate::source_admission::{
    ClassificationPointer, PendingResponse, RecordPointer, SourceAccountBinding, SourceAdmission,
    SourceBindingProof,
};
use crate::{SessionModelInterval, TokenFacts};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionEventPeriod {
    pub reset_at: i64,
    pub window_seconds: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventOwnership {
    pub start_at: i64,
    pub end_at: Option<i64>,
}

impl EventOwnership {
    fn valid(&self) -> bool {
        valid_time(self.start_at)
            && self
                .end_at
                .is_none_or(|end| valid_time(end) && self.start_at <= end)
    }

    fn contains(&self, timestamp: i64) -> bool {
        timestamp >= self.start_at && self.end_at.is_none_or(|end| timestamp < end)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CounterWitness {
    pub pointer: RecordPointer,
    pub total: u64,
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub cache_write_input: Option<u64>,
    pub output: Option<u64>,
}

impl CounterWitness {
    fn facts(&self) -> TokenFacts {
        TokenFacts {
            total: self.total,
            input: self.input,
            cached_input: self.cached_input,
            cache_write_input: self.cache_write_input,
            output: self.output,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyModelProof {
    pub pointer: ClassificationPointer,
    pub kind: String,
    pub interval: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyCounterProof {
    pub before: CounterWitness,
    pub after: CounterWitness,
    pub model_context: Option<LegacyModelProof>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NumericEventProof {
    LegacyCounter(Box<LegacyCounterProof>),
    /// Existing family keys only; a sealed full-prefix repair retains the
    /// original key and records the independently matched exact raw model.
    LegacyFamilyCounter {
        proof: Box<LegacyCounterProof>,
        exact_model: String,
    },
    ResponseRecord {
        candidate: Box<PendingResponse>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        legacy_family_exact_model: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodAttributionProof {
    pub rule_version: u8,
    pub lower_at: i64,
    pub upper_at: i64,
    pub evidence_generation: String,
    pub evidence_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventAdmission {
    pub version: u8,
    pub source_binding: SourceAccountBinding,
    pub verified_prefix_end: u64,
    pub ownership: EventOwnership,
    pub numeric: NumericEventProof,
    pub period: Option<PeriodAttributionProof>,
}

impl EventAdmission {
    pub fn valid_for(
        &self,
        event: &crate::LosslessSessionEvent,
        period: Option<SessionEventPeriod>,
    ) -> bool {
        let source = &event.source;
        let binding = &self.source_binding;
        if self.version != 1
            || !binding.valid(self.verified_prefix_end)
            || self.verified_prefix_end > i64::MAX as u64
            || source.range_end > self.verified_prefix_end
            || source.root_identity != binding.root_identity
            || source.relative_path != binding.relative_path
            || source.file_device != binding.file_device
            || source.file_inode != binding.file_inode
            || format!("{:032x}", source.prefix_generation) != binding.prefix_generation
            || !self.ownership.valid()
            || !self.ownership.contains(event.timestamp)
        {
            return false;
        }
        let owns = |timestamp| {
            self.ownership.contains(timestamp)
                && match binding.proof {
                    SourceBindingProof::Rpc {
                        began_at, ended_at, ..
                    } => {
                        timestamp >= began_at
                            || binding.creator_origin.as_ref().is_some_and(|origin| {
                                timestamp >= origin.pointer.timestamp
                                    && self.ownership.start_at == origin.ownership_start_at
                                    && self.ownership.contains(ended_at)
                            })
                    }
                    SourceBindingProof::AcceptedPrefix { .. } => true,
                }
        };
        let primary = match &self.numeric {
            NumericEventProof::LegacyCounter(proof)
            | NumericEventProof::LegacyFamilyCounter { proof, .. } => {
                let proof_model =
                    if let NumericEventProof::LegacyFamilyCounter { exact_model, .. } =
                        &self.numeric
                    {
                        if !matches!(event.model.as_str(), "SOL" | "TERRA" | "LUNA")
                            || crate::model_names::normalize_model_name(exact_model)
                                != Some(exact_model.as_str())
                            || matches!(exact_model.as_str(), "SOL" | "TERRA" | "LUNA" | "ASTRA")
                            || codex_info_pricing::family(exact_model) != Some(event.model.as_str())
                        {
                            return false;
                        }
                        exact_model.as_str()
                    } else {
                        event.model.as_str()
                    };
                let LegacyCounterProof {
                    before,
                    after,
                    model_context,
                } = proof.as_ref();
                if !before.pointer.valid(self.verified_prefix_end)
                    || !after.pointer.valid(self.verified_prefix_end)
                    || before.pointer.end > after.pointer.start
                    || !owns(before.pointer.timestamp)
                    || !owns(after.pointer.timestamp)
                    || after.pointer.timestamp != event.timestamp
                    || after.facts().checked_delta(before.facts()) != Ok(event.tokens)
                {
                    return false;
                }
                if model_context.as_ref().is_some_and(|context| {
                    !(matches!(context.kind.as_str(), "initial-settings" | "turn")
                        && context.pointer.start < context.pointer.end
                        && context.pointer.end <= after.pointer.start
                        && crate::validate_sha256(&context.pointer.sha256, "model context").is_ok()
                        && SessionModelInterval::parse(&context.interval).is_some())
                }) {
                    return false;
                }
                let model_valid = model_context.as_ref().is_some_and(|context| {
                    SessionModelInterval::parse(&context.interval)
                        == Some(SessionModelInterval::None)
                        || SessionModelInterval::parse(&context.interval)
                            == Some(SessionModelInterval::Model(proof_model.to_owned()))
                });
                if event.model != "UNATTRIBUTED" && !model_valid {
                    return false;
                }
                &after.pointer
            }
            NumericEventProof::ResponseRecord {
                candidate,
                legacy_family_exact_model,
            } => {
                let proof_model = match legacy_family_exact_model {
                    Some(exact)
                        if matches!(event.model.as_str(), "SOL" | "TERRA" | "LUNA")
                            && crate::model_names::normalize_model_name(exact)
                                == Some(exact.as_str())
                            && !matches!(exact.as_str(), "SOL" | "TERRA" | "LUNA" | "ASTRA")
                            && codex_info_pricing::family(exact) == Some(event.model.as_str()) =>
                    {
                        exact.as_str()
                    }
                    Some(_) => return false,
                    None => event.model.as_str(),
                };
                let mut admission = SourceAdmission::from_response_boundary();
                admission.metadata = Some(binding.metadata.clone());
                admission.metadata_qualified = true;
                admission.push_response((**candidate).clone());
                let usage = candidate.record.usage;
                if !candidate.session_scope_qualified
                    || !admission.valid(self.verified_prefix_end)
                    || candidate.qualified_model() != Some(proof_model)
                    || candidate.pointer.timestamp != event.timestamp
                    || !owns(candidate.pointer.timestamp)
                    || event.tokens
                        != (TokenFacts {
                            total: usage.total,
                            input: Some(usage.input),
                            cached_input: Some(usage.cached_input),
                            cache_write_input: Some(usage.cache_write_input),
                            output: Some(usage.output),
                        })
                {
                    return false;
                }
                &candidate.pointer
            }
        };
        if primary.start < source.range_start || primary.end > source.range_end {
            return false;
        }
        match (&self.period, period) {
            (None, None) => true,
            (Some(proof), Some(period)) => {
                let Some(origin) = period.reset_at.checked_sub(period.window_seconds) else {
                    return false;
                };
                proof.rule_version == 1
                    && period.window_seconds > 0
                    && valid_time(period.reset_at)
                    && valid_time(origin)
                    && proof.lower_at >= origin
                    && proof.upper_at < period.reset_at
                    && proof.lower_at <= event.timestamp
                    && event.timestamp <= proof.upper_at
                    && self.ownership.contains(proof.lower_at)
                    && self.ownership.contains(proof.upper_at)
                    && crate::canonical_u64_text(&proof.evidence_generation, "period evidence")
                        .is_ok_and(|value| value > 0)
                    && crate::validate_sha256(&proof.evidence_sha256, "period evidence").is_ok()
            }
            _ => false,
        }
    }
}

fn valid_time(value: i64) -> bool {
    (1..=crate::MAX_PUBLIC_UNIX_SECONDS).contains(&value)
}

#[cfg(test)]
mod issue659_creator_consumers {
    use crate::{
        event_admission, source_admission, LosslessSessionEvent, SessionEventSource, TokenFacts,
    };
    fn issue604_source_binding_json() -> serde_json::Value {
        serde_json::json!({
            "version": 2,
            "stream": "legacy-counter",
            "metadata": {"thread_id":"binding-thread", "session_id":"binding-session"},
            "metadata_qualified": true,
            "strong_context": null,
            "previous_raw": null,
            "response_waiting_for_counter": false,
            "responses": [],
            "account_binding": {
                "version": 1,
                "partition": {
                    "schema_version":"codex-info-account-db-v1",
                    "profile_scope_id":"11".repeat(16),
                    "account_scope_id":"a".repeat(64),
                    "storage_epoch":7,
                    "partition_id":"a".repeat(64)
                },
                "root_identity":"unix:10:20",
                "relative_path":"one.jsonl",
                "file_device":10,
                "file_inode":42,
                "prefix_generation":"00000000000000000000000000005678",
                "prefix_end":100,
                "metadata":{"thread_id":"binding-thread", "session_id":"binding-session"},
                "proof": {
                    "kind":"rpc",
                    "began_at":1791871200,
                    "ended_at":1791871201,
                    "process": {
                        "pid":1234,
                        "starttime_ticks":17,
                        "executable_device":10,
                        "executable_inode":20
                    }
                }
            }
        })
    }

    fn issue604_creator_origin_event(with_origin: bool) -> LosslessSessionEvent {
        const AT: i64 = 1_791_871_200;
        let mut binding = issue604_source_binding_json()["account_binding"].clone();
        binding["proof"]["began_at"] = serde_json::json!(AT + 100);
        binding["proof"]["ended_at"] = serde_json::json!(AT + 101);
        if with_origin {
            binding["creator_origin"] = serde_json::json!({
                "pointer":{"start":0,"end":10,"sha256":"11".repeat(32),"timestamp":AT},
                "ownership_start_at":AT - 10,"ownership_end_at":null
            });
        }
        let ids = source_admission::ResponseIds {
            thread_id: "binding-thread".into(),
            session_id: "binding-session".into(),
            root_turn_id: "root-turn".into(),
            turn_id: "turn".into(),
            response_id: "response".into(),
        };
        let pointer = |start, end, timestamp| serde_json::json!({"start":start,"end":end,"timestamp":timestamp,"sha256":"22".repeat(32)});
        let usage = serde_json::json!({"total":5,"input":4,"cached_input":1,"cache_write_input":0,"output":1,"reasoning_output":1});
        let admission = serde_json::json!({
            "version":1,"source_binding":binding,"verified_prefix_end":100,
            "ownership":{"start_at":AT-10,"end_at":null},"period":null,
            "numeric":{"kind":"response-record","candidate":{
                "key":ids.key(),"record":{"ids":ids,"usage":usage,"turn_usage":usage,"thread_usage":usage},
                "pointer":pointer(20,30,AT+3),"session_scope_qualified":true,
                "root_context":{"model":"gpt-6.1-sol","context":{"turn_id":"turn","root_turn_id":"root-turn","pointer":pointer(10,20,AT+1)}},
                "classification":{"state":"normal","boundary":{"kind":"record","pointer":{"start":30,"end":40,"sha256":"55".repeat(32)}}}
            }}
        });
        LosslessSessionEvent {
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
            timestamp: AT + 3,
            model: "gpt-6.1-sol".into(),
            tokens: TokenFacts {
                total: 5,
                input: Some(4),
                cached_input: Some(1),
                cache_write_input: Some(0),
                output: Some(1),
            },
            period: None,
            admission: Some(
                serde_json::from_value(admission)
                    .expect("verified creator origin must be a supported durable proof"),
            ),
        }
    }

    #[test]
    fn issue604_creator_origin_restores_only_owned_response_before_rpc() {
        let before = issue604_creator_origin_event(false);
        assert!(!before.admission.as_ref().unwrap().valid_for(&before, None));
        let event = issue604_creator_origin_event(true);
        assert!(event.admission.as_ref().unwrap().valid_for(&event, None));
        let stored = serde_json::to_string(&event.admission).unwrap();
        let restored: Option<event_admission::EventAdmission> =
            serde_json::from_str(&stored).unwrap();
        assert_eq!(restored, event.admission);
        assert_eq!(event.tokens.total, 5);
        assert_eq!(
            event.period, None,
            "source ownership cannot invent an official period"
        );
        let mut changed = event.clone();
        changed.admission.as_mut().unwrap().ownership.start_at += 1;
        assert!(!changed
            .admission
            .as_ref()
            .unwrap()
            .valid_for(&changed, None));
        let mut changed = event.clone();
        changed.admission.as_mut().unwrap().ownership.end_at = Some(event.timestamp + 1);
        assert!(!changed
            .admission
            .as_ref()
            .unwrap()
            .valid_for(&changed, None));
        let mut inherited = event.clone();
        if let event_admission::NumericEventProof::ResponseRecord { candidate, .. } =
            &mut inherited.admission.as_mut().unwrap().numeric
        {
            candidate.session_scope_qualified = false;
        }
        assert!(!inherited
            .admission
            .as_ref()
            .unwrap()
            .valid_for(&inherited, None));
    }

    #[test]
    fn issue604_creator_origin_rejects_broken_continuity_and_pointer() {
        let event = issue604_creator_origin_event(true);
        let original = serde_json::to_value(event.admission.as_ref().unwrap()).unwrap();
        for (pointer, replacement) in [
            (
                "/source_binding/creator_origin/pointer/start",
                serde_json::json!(1),
            ),
            (
                "/source_binding/creator_origin/pointer/end",
                serde_json::json!(101),
            ),
            (
                "/source_binding/creator_origin/pointer/sha256",
                serde_json::json!("invalid"),
            ),
            (
                "/source_binding/creator_origin/pointer/timestamp",
                serde_json::json!(1_791_871_204),
            ),
            (
                "/source_binding/creator_origin/ownership_start_at",
                serde_json::json!(1_791_871_201),
            ),
            (
                "/source_binding/creator_origin/ownership_end_at",
                serde_json::json!(1_791_871_301),
            ),
            (
                "/source_binding/proof/ended_at",
                serde_json::json!(1_791_871_199),
            ),
        ] {
            let mut value = original.clone();
            *value.pointer_mut(pointer).unwrap() = replacement;
            let changed: event_admission::EventAdmission = serde_json::from_value(value).unwrap();
            assert!(!changed.valid_for(&event, None), "{pointer}");
        }
    }
}

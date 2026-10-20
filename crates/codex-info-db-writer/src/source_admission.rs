//! Durable, unqualified source facts. These are not public model totals or
//! official period evidence. DATA RECORDER-MODEL-01 owns their admission.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn valid_identifier(value: &str) -> bool {
    super::active_thread_text_valid(value, 512)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum UsageStream {
    #[serde(rename = "legacy-counter")]
    LegacyCounter,
    #[serde(rename = "response-record")]
    ResponseRecord,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMetadata {
    pub thread_id: String,
    pub session_id: Option<String>,
}

/// A successful qualification of this local source in one account database.
/// Accepted range bytes and per-event ownership still require independent
/// verification; this record is neither provider billing evidence nor a live
/// process identity that can be reused after restart.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAccountBinding {
    pub version: u8,
    pub partition: super::StoragePartitionIdentity,
    pub root_identity: String,
    pub relative_path: String,
    pub file_device: u64,
    pub file_inode: u64,
    pub prefix_generation: String,
    pub prefix_end: u64,
    pub metadata: SessionMetadata,
    pub proof: SourceBindingProof,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceBindingProof {
    Rpc {
        began_at: i64,
        ended_at: i64,
        process: SourceBindingProcess,
    },
    AcceptedPrefix {
        anchor: RecordPointer,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceBindingProcess {
    pub pid: i32,
    pub starttime_ticks: u64,
    pub executable_device: u64,
    pub executable_inode: u64,
}

impl SourceAccountBinding {
    pub(crate) fn valid(&self, end: u64) -> bool {
        self.version == 1
            && self.partition.validate().is_ok()
            && super::validate_session_key(&self.root_identity, &self.relative_path).is_ok()
            && self.prefix_generation.len() == 32
            && self
                .prefix_generation
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && u128::from_str_radix(&self.prefix_generation, 16).is_ok_and(|value| value != 0)
            && self.prefix_end > 0
            && self.prefix_end <= end
            && self.prefix_end <= i64::MAX as u64
            && valid_identifier(&self.metadata.thread_id)
            && self
                .metadata
                .session_id
                .as_deref()
                .is_none_or(valid_identifier)
            && match &self.proof {
                SourceBindingProof::Rpc {
                    began_at,
                    ended_at,
                    process,
                } => {
                    (1..=super::MAX_PUBLIC_UNIX_SECONDS).contains(began_at)
                        && (1..=super::MAX_PUBLIC_UNIX_SECONDS).contains(ended_at)
                        && began_at <= ended_at
                        && process.pid > 0
                }
                SourceBindingProof::AcceptedPrefix { anchor } => anchor.valid(self.prefix_end),
            }
    }

    pub fn matches_checkpoint(&self, checkpoint: &super::SessionCheckpoint) -> bool {
        self.valid(checkpoint.committed_offset)
            && self.root_identity == checkpoint.root_identity
            && self.relative_path == checkpoint.relative_path
            && self.file_device == checkpoint.file_device
            && self.file_inode == checkpoint.file_inode
            && self.prefix_generation == format!("{:032x}", checkpoint.prefix_generation)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordPointer {
    pub start: u64,
    pub end: u64,
    pub sha256: String,
    pub timestamp: i64,
}

impl RecordPointer {
    pub(crate) fn valid(&self, end: u64) -> bool {
        self.start < self.end
            && self.end <= end
            && (1..=super::MAX_PUBLIC_UNIX_SECONDS).contains(&self.timestamp)
            && super::validate_sha256(&self.sha256, "source record").is_ok()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseIds {
    pub thread_id: String,
    pub session_id: String,
    pub root_turn_id: String,
    pub turn_id: String,
    pub response_id: String,
}

impl ResponseIds {
    fn ordered(&self) -> [&str; 5] {
        [
            &self.thread_id,
            &self.session_id,
            &self.root_turn_id,
            &self.turn_id,
            &self.response_id,
        ]
    }

    pub fn key(&self) -> String {
        let mut digest = Sha256::new();
        for id in self.ordered() {
            digest.update((id.len() as u64).to_be_bytes());
            digest.update(id.as_bytes());
        }
        digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn valid(&self) -> bool {
        self.ordered().into_iter().all(valid_identifier)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseUsage {
    pub total: u64,
    pub input: u64,
    pub cached_input: u64,
    pub cache_write_input: u64,
    pub output: u64,
    pub reasoning_output: u64,
}

impl ResponseUsage {
    pub fn valid(&self) -> bool {
        [
            self.total,
            self.input,
            self.cached_input,
            self.cache_write_input,
            self.output,
            self.reasoning_output,
        ]
        .into_iter()
        .all(|v| v <= i64::MAX as u64)
            && self.input.checked_add(self.output) == Some(self.total)
            && self
                .cached_input
                .checked_add(self.cache_write_input)
                .is_some_and(|v| v <= self.input)
            && self.reasoning_output <= self.output
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseRecord {
    pub ids: ResponseIds,
    pub usage: ResponseUsage,
    pub turn_usage: ResponseUsage,
    pub thread_usage: ResponseUsage,
}

impl ResponseRecord {
    pub fn valid(&self) -> bool {
        self.ids.valid()
            && self.usage.valid()
            && self.turn_usage.valid()
            && self.thread_usage.valid()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrongContext {
    pub turn_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub pointer: RecordPointer,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseContext {
    pub model: String,
    pub context: StrongContext,
}

/// Byte evidence for a classification boundary; its event time may be invalid.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassificationPointer {
    pub start: u64,
    pub end: u64,
    pub sha256: String,
}

impl ClassificationPointer {
    fn valid_after(&self, start: u64, end: u64) -> bool {
        start <= self.start
            && self.start < self.end
            && self.end <= end
            && super::validate_sha256(&self.sha256, "classification record").is_ok()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ClassificationProof {
    Record { pointer: ClassificationPointer },
    SourceEnd { offset: u64 },
}

impl ClassificationProof {
    fn valid_after(&self, start: u64, end: u64) -> bool {
        match self {
            Self::Record { pointer } => pointer.valid_after(start, end),
            Self::SourceEnd { offset } => start <= *offset && *offset <= end,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResponseModelFailure {
    UnrecognizedControl,
    SnapshotConflict,
    ReplacementMissing,
    ReplacementMismatch,
    HistoryBoundary,
    SourceBoundary,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ResponseModelClassification {
    #[default]
    LegacyUnknown,
    AwaitingBoundary,
    AwaitingReplacement {
        snapshot: ClassificationPointer,
    },
    Normal {
        boundary: ClassificationProof,
    },
    Compaction {
        snapshot: ClassificationPointer,
        replacement: ResponseContext,
    },
    Unassigned {
        reason: ResponseModelFailure,
        boundary: ClassificationProof,
    },
}

impl ResponseModelClassification {
    fn is_legacy_unknown(&self) -> bool {
        matches!(self, Self::LegacyUnknown)
    }
    fn is_open(&self) -> bool {
        matches!(
            self,
            Self::AwaitingBoundary | Self::AwaitingReplacement { .. }
        )
    }
}

/// Already recognized input. Numeric counter validity is a separate lane.
pub enum ResponseClassificationInput<'a> {
    NoOp,
    Counter,
    Task,
    NextRoot,
    Strong {
        context: &'a ResponseContext,
        is_turn: bool,
    },
    Snapshot(&'a ResponseRecord),
    Unknown,
    HistoryBoundary,
}

/// Association is independent of response numerical/model qualification.
/// Old checkpoints omit it; only a scan from the original prefix can create it.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ResponseCounterAssociation {
    #[default]
    LegacyUnknown,
    Awaiting,
    Unmatched {
        boundary: ClassificationProof,
    },
    Matched {
        pointer: RecordPointer,
        last: Option<ResponseUsage>,
        snapshot: Option<ClassificationPointer>,
    },
}

impl ResponseCounterAssociation {
    fn is_legacy_unknown(&self) -> bool {
        matches!(self, Self::LegacyUnknown)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingResponse {
    pub key: String,
    pub record: ResponseRecord,
    pub pointer: RecordPointer,
    pub session_scope_qualified: bool,
    pub root_context: Option<ResponseContext>,
    #[serde(
        default,
        skip_serializing_if = "ResponseModelClassification::is_legacy_unknown"
    )]
    pub classification: ResponseModelClassification,
    #[serde(
        default,
        skip_serializing_if = "ResponseCounterAssociation::is_legacy_unknown"
    )]
    pub counter_association: ResponseCounterAssociation,
}

impl PendingResponse {
    pub fn matched_counter(&self) -> Option<&RecordPointer> {
        match &self.counter_association {
            ResponseCounterAssociation::Matched { pointer, .. } => Some(pointer),
            _ => None,
        }
    }

    pub fn observe_counter_association(
        &mut self,
        input: &ResponseClassificationInput<'_>,
        proof: &ClassificationPointer,
        pointer: Option<&RecordPointer>,
        last: Option<ResponseUsage>,
    ) {
        use ResponseClassificationInput as Input;
        use ResponseCounterAssociation as State;
        if !matches!(self.counter_association, State::Awaiting) {
            return;
        }
        match input {
            Input::NoOp => return,
            Input::Snapshot(record) if *record == &self.record => return,
            Input::Strong { context, .. }
                if self.root_context.as_ref().is_some_and(|root| {
                    root.model == context.model
                        && root.context.turn_id == context.context.turn_id
                        && root.context.root_turn_id == context.context.root_turn_id
                }) || matches!(&self.classification,ResponseModelClassification::Compaction {replacement,..} if replacement==*context) =>
            {
                return
            }
            Input::Counter => {
                if let Some(pointer) = pointer {
                    let snapshot = match &self.classification {
                        ResponseModelClassification::Compaction { snapshot, .. } => {
                            Some(snapshot.clone())
                        }
                        _ => None,
                    };
                    if last == Some(self.record.usage) || snapshot.is_some() {
                        self.counter_association = State::Matched {
                            pointer: pointer.clone(),
                            last: if snapshot.is_none() { last } else { None },
                            snapshot,
                        };
                        return;
                    }
                }
            }
            _ => {}
        }
        self.counter_association = State::Unmatched {
            boundary: ClassificationProof::Record {
                pointer: proof.clone(),
            },
        };
    }

    fn counter_association_valid(&self, end: u64) -> bool {
        match &self.counter_association {
            ResponseCounterAssociation::LegacyUnknown | ResponseCounterAssociation::Awaiting => {
                true
            }
            ResponseCounterAssociation::Unmatched { boundary } => {
                boundary.valid_after(self.pointer.end, end)
            }
            ResponseCounterAssociation::Matched {
                pointer,
                last,
                snapshot,
            } => {
                pointer.valid(end)
                    && pointer.start >= self.pointer.end
                    && match (last, snapshot) {
                        (Some(last), None) => *last == self.record.usage,
                        (None, Some(snapshot)) => {
                            matches!(&self.classification,ResponseModelClassification::Compaction {snapshot:original,..} if original==snapshot)
                                && snapshot.end <= pointer.start
                        }
                        _ => false,
                    }
            }
        }
    }
    /// Request model only. This grants neither account nor period admission.
    pub fn qualified_model(&self) -> Option<&str> {
        if !self.session_scope_qualified {
            return None;
        }
        let context = self.root_context.as_ref()?;
        let same_turn = context.context.turn_id.as_deref() == Some(&self.record.ids.turn_id)
            && context.context.root_turn_id.as_deref() == Some(&self.record.ids.root_turn_id);
        match &self.classification {
            ResponseModelClassification::Normal { .. } if same_turn => Some(context.model.as_str()),
            ResponseModelClassification::Compaction { .. }
                if context.context.turn_id.is_some() && context.context.root_turn_id.is_some() =>
            {
                Some(context.model.as_str())
            }
            _ => None,
        }
    }

    pub fn observe_classification(
        &mut self,
        input: &ResponseClassificationInput<'_>,
        pointer: &ClassificationPointer,
    ) {
        use ResponseClassificationInput as Input;
        use ResponseModelClassification as State;
        let proof = || ClassificationProof::Record {
            pointer: pointer.clone(),
        };
        let unassigned = |reason| State::Unassigned {
            reason,
            boundary: proof(),
        };
        let matches_root = |context: &ResponseContext| {
            self.root_context
                .as_ref()
                .is_some_and(|root| root.model == context.model)
                && context.context.turn_id.as_deref() == Some(&self.record.ids.turn_id)
                && context.context.root_turn_id.as_deref() == Some(&self.record.ids.root_turn_id)
        };
        let reaffirms_original = |context: &ResponseContext| {
            self.root_context.as_ref().is_some_and(|root| {
                root.model == context.model
                    && root.context.turn_id == context.context.turn_id
                    && root.context.root_turn_id == context.context.root_turn_id
            })
        };
        self.classification = match &self.classification {
            State::AwaitingBoundary => match input {
                Input::NoOp => return,
                Input::Strong { context, .. } if reaffirms_original(context) => return,
                Input::Counter | Input::Task | Input::NextRoot | Input::Strong { .. } => {
                    State::Normal { boundary: proof() }
                }
                Input::Snapshot(snapshot) if *snapshot == &self.record => {
                    State::AwaitingReplacement {
                        snapshot: pointer.clone(),
                    }
                }
                Input::Snapshot(_) => unassigned(ResponseModelFailure::SnapshotConflict),
                Input::Unknown => unassigned(ResponseModelFailure::UnrecognizedControl),
                Input::HistoryBoundary => unassigned(ResponseModelFailure::HistoryBoundary),
            },
            State::AwaitingReplacement { snapshot } => match input {
                Input::NoOp => return,
                Input::Snapshot(record) if *record == &self.record => return,
                Input::Snapshot(_) => unassigned(ResponseModelFailure::SnapshotConflict),
                Input::Strong {
                    context,
                    is_turn: true,
                } if self.session_scope_qualified && matches_root(context) => State::Compaction {
                    snapshot: snapshot.clone(),
                    replacement: (*context).clone(),
                },
                Input::Strong { .. } => unassigned(ResponseModelFailure::ReplacementMismatch),
                Input::Unknown => unassigned(ResponseModelFailure::UnrecognizedControl),
                Input::HistoryBoundary => unassigned(ResponseModelFailure::HistoryBoundary),
                Input::Counter | Input::Task | Input::NextRoot => {
                    unassigned(ResponseModelFailure::ReplacementMissing)
                }
            },
            State::LegacyUnknown
            | State::Normal { .. }
            | State::Compaction { .. }
            | State::Unassigned { .. } => return,
        };
    }

    pub fn close_source_classification(&mut self, offset: u64) {
        if matches!(
            self.counter_association,
            ResponseCounterAssociation::Awaiting
        ) {
            self.counter_association = ResponseCounterAssociation::Unmatched {
                boundary: ClassificationProof::SourceEnd { offset },
            };
        }
        if matches!(
            self.classification,
            ResponseModelClassification::AwaitingBoundary
                | ResponseModelClassification::AwaitingReplacement { .. }
        ) {
            self.classification = ResponseModelClassification::Unassigned {
                reason: ResponseModelFailure::SourceBoundary,
                boundary: ClassificationProof::SourceEnd { offset },
            };
        }
    }

    fn classification_valid(&self, end: u64) -> bool {
        use ResponseModelClassification as State;
        match &self.classification {
            State::LegacyUnknown | State::AwaitingBoundary => true,
            State::AwaitingReplacement { snapshot } => snapshot.valid_after(self.pointer.end, end),
            State::Normal { boundary } => {
                matches!(boundary, ClassificationProof::Record { .. })
                    && boundary.valid_after(self.pointer.end, end)
            }
            State::Unassigned { reason, boundary } => {
                matches!(boundary, ClassificationProof::SourceEnd { .. })
                    == (*reason == ResponseModelFailure::SourceBoundary)
                    && boundary.valid_after(self.pointer.end, end)
            }
            State::Compaction {
                snapshot,
                replacement,
            } => {
                self.session_scope_qualified
                    && snapshot.valid_after(self.pointer.end, end)
                    && replacement.context.pointer.valid(end)
                    && snapshot.end <= replacement.context.pointer.start
                    && self
                        .root_context
                        .as_ref()
                        .is_some_and(|root| root.model == replacement.model)
                    && replacement.context.turn_id.as_deref() == Some(&self.record.ids.turn_id)
                    && replacement.context.root_turn_id.as_deref()
                        == Some(&self.record.ids.root_turn_id)
            }
        }
    }
}

/// SQL NULL is old/unknown state. Only a verified scan beginning at byte zero
/// may construct a new known legacy stream. Appends must restore this state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAdmission {
    version: u8,
    pub stream: UsageStream,
    pub metadata: Option<SessionMetadata>,
    pub metadata_qualified: bool,
    pub strong_context: Option<StrongContext>,
    /// Original adopted legacy model context; model/kind remain checkpoint facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_context: Option<ClassificationPointer>,
    pub previous_raw: Option<RecordPointer>,
    pub response_waiting_for_counter: bool,
    pub responses: Vec<PendingResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_binding: Option<SourceAccountBinding>,
}

impl SourceAdmission {
    pub fn requires_prefix_reconstruction(&self) -> bool {
        self.version != 2
            || self
                .responses
                .iter()
                .any(|response| response.classification.is_legacy_unknown())
    }

    pub fn from_response_boundary() -> Self {
        let mut state = Self::from_prefix_start();
        state.stream = UsageStream::ResponseRecord;
        state
    }

    pub fn from_prefix_start() -> Self {
        Self {
            version: 2,
            stream: UsageStream::LegacyCounter,
            metadata: None,
            metadata_qualified: false,
            strong_context: None,
            adopted_context: None,
            previous_raw: None,
            response_waiting_for_counter: false,
            responses: Vec::new(),
            account_binding: None,
        }
    }

    pub fn push_response(&mut self, response: PendingResponse) {
        self.version = 2;
        self.responses.push(response);
    }

    pub fn observe_response_classification(
        &mut self,
        input: &ResponseClassificationInput<'_>,
        pointer: &ClassificationPointer,
    ) {
        if let Some(response) = self.responses.last_mut() {
            response.observe_classification(input, pointer);
        }
    }

    pub fn valid(&self, end: u64) -> bool {
        let context_valid = |context: &StrongContext| {
            context.turn_id.as_deref().is_none_or(valid_identifier)
                && context.root_turn_id.as_deref().is_none_or(valid_identifier)
                && context.pointer.valid(end)
        };
        matches!(self.version, 1 | 2)
            && self.metadata.as_ref().is_none_or(|meta| {
                valid_identifier(&meta.thread_id)
                    && meta.session_id.as_deref().is_none_or(valid_identifier)
            })
            && (!self.metadata_qualified || self.metadata.is_some())
            && self.account_binding.as_ref().is_none_or(|binding| {
                self.version == 2
                    && self.metadata_qualified
                    && self.metadata.as_ref().is_some_and(|current| {
                        current.thread_id == binding.metadata.thread_id
                            && binding
                                .metadata
                                .session_id
                                .as_deref()
                                .is_none_or(|saved| current.session_id.as_deref() == Some(saved))
                    })
                    && binding.valid(end)
            })
            && self.strong_context.as_ref().is_none_or(context_valid)
            && self
                .adopted_context
                .as_ref()
                .is_none_or(|pointer| self.version == 2 && pointer.valid_after(0, end))
            && self.previous_raw.as_ref().is_none_or(|p| p.valid(end))
            && (self.stream == UsageStream::ResponseRecord
                || (!self.response_waiting_for_counter && self.responses.is_empty()))
            && self
                .responses
                .iter()
                .rev()
                .skip(1)
                .all(|response| !response.classification.is_open())
            && self.responses.iter().all(|response| {
                response.record.valid()
                    && (self.version == 2 || response.classification.is_legacy_unknown())
                    && response.classification_valid(end)
                    && response.counter_association_valid(end)
                    && response.key == response.record.ids.key()
                    && response.pointer.valid(end)
                    && (!response.session_scope_qualified
                        || self.metadata.as_ref().is_some_and(|meta| {
                            meta.thread_id == response.record.ids.thread_id
                                && meta.session_id.as_deref()
                                    == Some(&response.record.ids.session_id)
                        }))
                    && response.root_context.as_ref().is_none_or(|root| {
                        super::model_names::normalize_model_name(&root.model)
                            == Some(root.model.as_str())
                            && context_valid(&root.context)
                            && root.context.pointer.end <= response.pointer.start
                            && root
                                .context
                                .turn_id
                                .as_deref()
                                .is_some_and(valid_identifier)
                            && root
                                .context
                                .root_turn_id
                                .as_deref()
                                .is_some_and(valid_identifier)
                    })
            })
    }

    pub fn from_storage(value: &str, end: u64) -> Option<Self> {
        serde_json::from_str::<Self>(value)
            .ok()
            .filter(|state| state.valid(end))
    }

    pub fn to_storage(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

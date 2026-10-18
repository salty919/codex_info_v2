// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! Shared bounded recognition of the documented Session JSONL formats.
//! Callers own source/account admission and local versus live failure policy.

use chrono::DateTime;
use codex_info_db_writer::source_admission::{self, ResponseIds, ResponseRecord, ResponseUsage};
use codex_info_db_writer::{SessionModelContextKind, SessionModelInterval, TokenFacts};
use std::io::{self, BufRead};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TokenSnapshot {
    pub total: u64,
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub output: Option<u64>,
    pub cache_write_input: Option<u64>,
}

impl TokenSnapshot {
    pub fn facts(self) -> TokenFacts {
        TokenFacts {
            total: self.total,
            input: self.input,
            cached_input: self.cached_input,
            cache_write_input: self.cache_write_input,
            output: self.output,
        }
    }

    pub fn from_facts(tokens: TokenFacts) -> Self {
        Self {
            total: tokens.total,
            input: tokens.input,
            cached_input: tokens.cached_input,
            output: tokens.output,
            cache_write_input: tokens.cache_write_input,
        }
    }

    pub fn is_zero(self) -> bool {
        self.total == 0
            && [
                self.input,
                self.cached_input,
                self.output,
                self.cache_write_input,
            ]
            .into_iter()
            .all(|value| value == Some(0))
    }

    pub fn valid(self) -> bool {
        self.facts().validate_fact().is_ok()
    }

    pub fn valid_raw(self) -> bool {
        self.facts().validate_raw_counter().is_ok()
    }

    pub fn has_usage(self) -> bool {
        self.total > 0
    }
}

#[derive(Clone, Debug)]
pub struct ModelAttributionState {
    pub model: Option<String>,
    pub kind: SessionModelContextKind,
    pub interval: SessionModelInterval,
}

impl ModelAttributionState {
    pub fn new() -> Self {
        Self {
            model: None,
            kind: SessionModelContextKind::None,
            interval: SessionModelInterval::None,
        }
    }

    pub fn invalidate(&mut self) {
        self.kind = SessionModelContextKind::Unknown;
        self.interval = SessionModelInterval::Unknown;
    }

    /// Returns whether a recognized context was malformed. A valid weak
    /// setting that loses priority is neither an adoption nor an error.
    pub fn observe_context(&mut self, record: &SessionRecordSummary) -> bool {
        let weak = match record.event_type() {
            Some("thread_settings_applied") => true,
            Some("turn_context" | "thread_context") => false,
            _ => return false,
        };
        let model = record.qualified_context_model().and_then(checkpoint_model);
        let Some(model) = model else {
            if !weak || self.kind != SessionModelContextKind::Turn {
                self.invalidate();
            }
            return true;
        };
        if weak && self.kind != SessionModelContextKind::None {
            return false;
        }
        self.kind = if weak {
            SessionModelContextKind::InitialSettings
        } else {
            SessionModelContextKind::Turn
        };
        self.interval = match &self.interval {
            SessionModelInterval::None => SessionModelInterval::Model(model.clone()),
            SessionModelInterval::Model(previous) if previous == &model => {
                SessionModelInterval::Model(model.clone())
            }
            SessionModelInterval::Model(_) | SessionModelInterval::Mixed => {
                SessionModelInterval::Mixed
            }
            SessionModelInterval::Unknown => SessionModelInterval::Unknown,
        };
        self.model = Some(model);
        false
    }

    pub fn interval_model(&self) -> Option<&str> {
        match &self.interval {
            SessionModelInterval::None
                if matches!(
                    self.kind,
                    SessionModelContextKind::InitialSettings | SessionModelContextKind::Turn
                ) =>
            {
                self.model.as_deref()
            }
            SessionModelInterval::Model(model) => Some(model),
            _ => None,
        }
    }

    pub fn start_interval(&mut self) {
        self.interval = SessionModelInterval::None;
    }
}

impl Default for ModelAttributionState {
    fn default() -> Self {
        Self::new()
    }
}

fn checkpoint_model(model: &str) -> Option<String> {
    codex_info_db_writer::model_names::normalize_model_name(model)
        .filter(|model| *model != "UNATTRIBUTED")
        .map(str::to_owned)
}

pub enum RecordRead {
    End,
    Present(Box<SessionRecordSummary>, u64),
    Invalid(u64, bool),
    Unterminated(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionRecordParseError {
    Syntax,
    Io,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionJsonKey {
    Type,
    Timestamp,
    Model,
    Payload,
    ThreadSettings,
    HistoryBase,
    Info,
    TotalTokenUsage,
    LastTokenUsage,
    ModelContextWindow,
    TotalTokens,
    CacheWriteInputTokens,
    InputTokens,
    CachedInputTokens,
    OutputTokens,
    ReasoningOutputTokens,
    Id,
    ThreadId,
    SessionId,
    TurnId,
    RootTurnId,
    ResponseId,
    Usage,
    TurnTokenUsage,
    ThreadTokenUsage,
    LatestTokenUsageRecord,
    Message,
    Text,
    Full,
    State,
    TriggerTurn,
    ReplacementHistory,
    ReplacementHistoryMetadata,
    WindowNumber,
    WindowId,
    FirstWindowId,
    PreviousWindowId,
    ThreadIdCamel,
    TurnIdCamel,
    Goal,
    Item,
    StartedAtMs,
    CompletedAtMs,
    ItemId,
    SummaryIndex,
    Objective,
    Status,
    TokensUsed,
    TimeUsedSeconds,
    CreatedAt,
    UpdatedAt,
    TokenBudget,
    Role,
    Ordinal,
    Metadata,
    Other,
}

impl SessionJsonKey {
    pub fn bit(self) -> u64 {
        1_u64 << self as u64
    }

    pub fn response_field(self) -> bool {
        matches!(
            self,
            Self::ThreadId
                | Self::SessionId
                | Self::TurnId
                | Self::RootTurnId
                | Self::ResponseId
                | Self::Usage
                | Self::TurnTokenUsage
                | Self::ThreadTokenUsage
        )
    }

    pub fn token_component(self) -> bool {
        matches!(
            self,
            Self::TotalTokens
                | Self::InputTokens
                | Self::CachedInputTokens
                | Self::CacheWriteInputTokens
                | Self::OutputTokens
        )
    }
}

const SESSION_JSON_KEYS: [(&[u8], SessionJsonKey); 55] = [
    (b"type", SessionJsonKey::Type),
    (b"timestamp", SessionJsonKey::Timestamp),
    (b"model", SessionJsonKey::Model),
    (b"payload", SessionJsonKey::Payload),
    (b"thread_settings", SessionJsonKey::ThreadSettings),
    (b"history_base", SessionJsonKey::HistoryBase),
    (b"info", SessionJsonKey::Info),
    (b"total_token_usage", SessionJsonKey::TotalTokenUsage),
    (b"last_token_usage", SessionJsonKey::LastTokenUsage),
    (b"model_context_window", SessionJsonKey::ModelContextWindow),
    (b"total_tokens", SessionJsonKey::TotalTokens),
    (
        b"cache_write_input_tokens",
        SessionJsonKey::CacheWriteInputTokens,
    ),
    (b"input_tokens", SessionJsonKey::InputTokens),
    (b"cached_input_tokens", SessionJsonKey::CachedInputTokens),
    (b"output_tokens", SessionJsonKey::OutputTokens),
    (
        b"reasoning_output_tokens",
        SessionJsonKey::ReasoningOutputTokens,
    ),
    (b"id", SessionJsonKey::Id),
    (b"thread_id", SessionJsonKey::ThreadId),
    (b"session_id", SessionJsonKey::SessionId),
    (b"turn_id", SessionJsonKey::TurnId),
    (b"root_turn_id", SessionJsonKey::RootTurnId),
    (b"response_id", SessionJsonKey::ResponseId),
    (b"usage", SessionJsonKey::Usage),
    (b"turn_token_usage", SessionJsonKey::TurnTokenUsage),
    (b"thread_token_usage", SessionJsonKey::ThreadTokenUsage),
    (
        b"latest_token_usage_record",
        SessionJsonKey::LatestTokenUsageRecord,
    ),
    (b"message", SessionJsonKey::Message),
    (b"text", SessionJsonKey::Text),
    (b"full", SessionJsonKey::Full),
    (b"state", SessionJsonKey::State),
    (b"trigger_turn", SessionJsonKey::TriggerTurn),
    (b"replacement_history", SessionJsonKey::ReplacementHistory),
    (
        b"replacement_history_metadata",
        SessionJsonKey::ReplacementHistoryMetadata,
    ),
    (b"window_number", SessionJsonKey::WindowNumber),
    (b"window_id", SessionJsonKey::WindowId),
    (b"first_window_id", SessionJsonKey::FirstWindowId),
    (b"previous_window_id", SessionJsonKey::PreviousWindowId),
    (b"threadId", SessionJsonKey::ThreadIdCamel),
    (b"turnId", SessionJsonKey::TurnIdCamel),
    (b"goal", SessionJsonKey::Goal),
    (b"item", SessionJsonKey::Item),
    (b"started_at_ms", SessionJsonKey::StartedAtMs),
    (b"completed_at_ms", SessionJsonKey::CompletedAtMs),
    (b"item_id", SessionJsonKey::ItemId),
    (b"summary_index", SessionJsonKey::SummaryIndex),
    (b"objective", SessionJsonKey::Objective),
    (b"status", SessionJsonKey::Status),
    (b"tokensUsed", SessionJsonKey::TokensUsed),
    (b"timeUsedSeconds", SessionJsonKey::TimeUsedSeconds),
    (b"createdAt", SessionJsonKey::CreatedAt),
    (b"updatedAt", SessionJsonKey::UpdatedAt),
    (b"tokenBudget", SessionJsonKey::TokenBudget),
    (b"role", SessionJsonKey::Role),
    (b"ordinal", SessionJsonKey::Ordinal),
    (b"metadata", SessionJsonKey::Metadata),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionJsonObject {
    Root,
    Payload,
    ThreadSettings,
    Info,
    TokenUsage,
    LastTokenUsage,
    ResponseSnapshot,
    ResponseUsage(bool),
    ResponseTurnUsage(bool),
    ResponseThreadUsage(bool),
    Goal,
}

#[derive(Clone, Debug, Default)]
pub struct TokenUsageSummary {
    pub total: Option<u64>,
    pub cache_write_input: Option<u64>,
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub output: Option<u64>,
    pub reasoning_output: Option<u64>,
    pub reasoning_malformed: bool,
    pub total_seen: bool,
    pub malformed: bool,
}

impl TokenUsageSummary {
    pub fn response_usage(&self) -> Option<ResponseUsage> {
        let usage = ResponseUsage {
            total: self.total?,
            input: self.input?,
            cached_input: self.cached_input?,
            cache_write_input: self.cache_write_input?,
            output: self.output?,
            reasoning_output: self.reasoning_output?,
        };
        (!self.malformed && !self.reasoning_malformed && usage.valid()).then_some(usage)
    }
    pub fn snapshot(&self) -> Option<TokenSnapshot> {
        Some(TokenSnapshot {
            total: self.total?,
            cache_write_input: self.cache_write_input,
            input: self.input,
            cached_input: self.cached_input,
            output: self.output,
        })
    }

    pub fn valid_snapshot(&self) -> Option<TokenSnapshot> {
        (!self.malformed).then(|| self.snapshot()).flatten()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ResponsePayloadSummary {
    pub thread_id: Option<String>,
    pub session_id: Option<String>,
    pub session_id_seen: bool,
    pub turn_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub response_id: Option<String>,
    pub malformed_identifiers: u64,
    pub usage: TokenUsageSummary,
    pub turn_usage: TokenUsageSummary,
    pub thread_usage: TokenUsageSummary,
}

impl ResponsePayloadSummary {
    pub fn record(&self) -> Option<ResponseRecord> {
        if self.malformed_identifiers != 0 {
            return None;
        }
        let record = ResponseRecord {
            ids: ResponseIds {
                thread_id: self.thread_id.clone()?,
                session_id: self.session_id.clone()?,
                root_turn_id: self.root_turn_id.clone()?,
                turn_id: self.turn_id.clone()?,
                response_id: self.response_id.clone()?,
            },
            usage: self.usage.response_usage()?,
            turn_usage: self.turn_usage.response_usage()?,
            thread_usage: self.thread_usage.response_usage()?,
        };
        record.valid().then_some(record)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SessionNoOp {
    ResponseItem,
    ActivityNotice,
    WorldState,
    InterAgentMetadata,
    Compacted,
    ThreadGoal,
    ItemCompleted,
    Message,
    Reasoning,
    ReasoningSection,
}

impl SessionNoOp {
    pub fn keys(self) -> &'static [SessionJsonKey] {
        use SessionJsonKey::*;
        match self {
            Self::ResponseItem | Self::ActivityNotice => &[],
            Self::WorldState => &[Full, State],
            Self::InterAgentMetadata => &[TriggerTurn],
            Self::Compacted => &[
                Message,
                ReplacementHistory,
                ReplacementHistoryMetadata,
                WindowNumber,
                WindowId,
                FirstWindowId,
                PreviousWindowId,
                LatestTokenUsageRecord,
            ],
            Self::ThreadGoal => &[ThreadIdCamel, TurnIdCamel, Goal],
            Self::ItemCompleted => &[ThreadId, TurnId, Item, StartedAtMs, CompletedAtMs],
            Self::Message => &[Message],
            Self::Reasoning => &[Text],
            Self::ReasoningSection => &[ItemId, SummaryIndex],
        }
    }

    pub fn required(self) -> &'static [SessionJsonKey] {
        use SessionJsonKey::*;
        match self {
            Self::Compacted => &[Message],
            Self::ThreadGoal => &[ThreadIdCamel, Goal],
            Self::ItemCompleted => &[Item],
            Self::ReasoningSection => &[],
            _ => self.keys(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GoalShape {
    pub seen: u64,
    pub malformed: u64,
    pub duplicates: bool,
    pub thread_id: Option<String>,
}

impl GoalShape {
    pub fn keys() -> &'static [SessionJsonKey] {
        use SessionJsonKey::*;
        &[
            ThreadIdCamel,
            Objective,
            Status,
            TokensUsed,
            TimeUsedSeconds,
            CreatedAt,
            UpdatedAt,
            TokenBudget,
        ]
    }

    pub fn valid(&self, thread_id: Option<&str>) -> bool {
        !self.duplicates
            && self.malformed == 0
            && self.thread_id.as_deref() == thread_id
            && Self::keys()
                .iter()
                .filter(|key| **key != SessionJsonKey::TokenBudget)
                .all(|key| self.seen & key.bit() != 0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct NoOpShape {
    pub seen: u64,
    pub malformed: u64,
    pub thread_id: Option<String>,
    pub goal: GoalShape,
    pub replacement_history: Option<u64>,
    pub replacement_metadata: Option<u64>,
    pub role_seen: bool,
    pub role_is_string: bool,
    pub role_is_user: bool,
}

impl NoOpShape {
    pub fn field(&mut self, key: SessionJsonKey, valid: bool) {
        self.seen |= key.bit();
        if !valid {
            self.malformed |= key.bit();
        }
    }

    pub fn valid(&self, kind: SessionNoOp, payload: &PayloadSummary) -> bool {
        if kind
            .keys()
            .iter()
            .any(|key| self.malformed & key.bit() != 0)
            || kind.required().iter().any(|key| self.seen & key.bit() == 0)
        {
            return false;
        }
        match kind {
            SessionNoOp::ThreadGoal => {
                self.thread_id.is_some() && self.goal.valid(self.thread_id.as_deref())
            }
            SessionNoOp::ItemCompleted => {
                payload.response.thread_id.is_some()
                    && payload.response.turn_id.is_some()
                    && payload.response.malformed_identifiers
                        & (SessionJsonKey::ThreadId.bit() | SessionJsonKey::TurnId.bit())
                        == 0
            }
            SessionNoOp::Compacted => {
                self.replacement_metadata
                    .is_none_or(|count| self.replacement_history == Some(count))
                    && (!payload.snapshot_non_null || payload.snapshot.record().is_some())
            }
            _ => true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PayloadSummary {
    pub noop: NoOpShape,
    pub metadata_id: Option<String>,
    pub metadata_id_malformed: bool,
    pub response: ResponsePayloadSummary,
    pub snapshot_non_null: bool,
    pub snapshot: ResponsePayloadSummary,
    pub event_type: Option<String>,
    pub event_type_seen: bool,
    pub event_type_malformed: bool,
    pub model: Option<String>,
    pub model_seen: bool,
    pub model_malformed: bool,
    pub thread_settings_seen: bool,
    pub thread_settings_object: bool,
    pub thread_settings_model: Option<String>,
    pub thread_settings_model_seen: bool,
    pub thread_settings_model_malformed: bool,
    pub history_base_seen: bool,
    pub history_base_non_null: bool,
    pub info_seen: bool,
    pub info_null: bool,
    pub info_object: bool,
    pub token_usage_seen: bool,
    pub token_usage_object: bool,
    pub token_usage: TokenUsageSummary,
    pub last_token_usage_seen: bool,
    pub last_token_usage_null: bool,
    pub last_token_usage_object: bool,
    pub last_token_usage: TokenUsageSummary,
    pub context_window: Option<u64>,
    pub context_window_seen: bool,
    pub context_window_malformed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SessionRecordSummary {
    pub unknown_envelope_field: bool,
    pub ordinal_seen: bool,
    pub ordinal_malformed: bool,
    pub response_metadata_seen: bool,
    pub response_metadata_object: bool,
    pub response_metadata_null: bool,
    pub outer_type: Option<String>,
    pub timestamp: Option<String>,
    pub timestamp_seen: bool,
    pub timestamp_malformed: bool,
    pub root_model: Option<String>,
    pub root_model_seen: bool,
    pub root_model_malformed: bool,
    pub payload_seen: bool,
    pub payload_object: bool,
    pub payload: PayloadSummary,
    pub duplicate_fields: bool,
    pub root_duplicates: u64,
    pub payload_duplicates: u64,
    pub info_duplicates: bool,
    pub counter_duplicates: bool,
    pub thread_settings_duplicates: bool,
    pub response_duplicates: bool,
    pub snapshot_duplicates: bool,
}

impl SessionRecordSummary {
    pub fn response_stream_marker(&self) -> bool {
        self.outer_type.as_deref() == Some("token_usage_record")
            || (self.outer_type.as_deref() == Some("compacted") && self.payload.snapshot_non_null)
    }

    pub fn known_state_input(&self) -> bool {
        matches!(
            self.outer_type.as_deref(),
            Some("session_meta" | "token_usage_record")
        ) || matches!(
            self.event_type(),
            Some(
                "token_count"
                    | "turn_context"
                    | "thread_context"
                    | "thread_settings_applied"
                    | "task_started"
                    | "task_complete"
                    | "task_completed"
                    | "turn_aborted"
            )
        )
    }

    pub fn metadata_shape_valid(&self) -> bool {
        !self.duplicate_fields
            && self.payload_object
            && self.payload.metadata_id.is_some()
            && !self.payload.metadata_id_malformed
            && self.payload.response.malformed_identifiers & SessionJsonKey::SessionId.bit() == 0
            && (!self.payload.response.session_id_seen
                || self.payload.response.session_id.is_some())
    }

    pub fn task_shape_valid(&self) -> bool {
        self.task_running().is_some() && self.context_envelope_valid()
    }

    pub fn live_shape_valid(&self) -> bool {
        self.input_shape_valid_for_coverage()
            && !self.unknown_envelope_field
            && (self.outer_type.as_deref() != Some("response_item")
                || ((!self.payload.event_type_seen || self.payload.event_type.is_some())
                    && (!self.payload.noop.role_seen || self.payload.noop.role_is_string)
                    && self.payload_duplicates & SessionJsonKey::Role.bit() == 0))
    }

    pub fn user_message_timestamp(&self) -> Option<i64> {
        (self.live_shape_valid()
            && (self.event_type() == Some("user_message")
                || (self.outer_type.as_deref() == Some("response_item")
                    && self.payload.event_type.as_deref() == Some("message")
                    && self.payload.noop.role_is_user)))
            .then(|| self.event_timestamp())
    }

    /// Complete input recognition is a prerequisite for a coverage proof.
    /// Independent valid numeric/context facts can still be admitted by the
    /// main scanner when this stronger property is false.
    pub fn ordinal_shape_valid(&self) -> bool {
        !self.ordinal_malformed && self.root_duplicates & SessionJsonKey::Ordinal.bit() == 0
    }

    pub fn response_metadata_shape_valid(&self) -> bool {
        !self.response_metadata_seen
            || (self.outer_type.as_deref() == Some("response_item")
                && (self.response_metadata_null || self.response_metadata_object)
                && self.root_duplicates & SessionJsonKey::Metadata.bit() == 0)
    }

    pub fn envelope_metadata_shape_valid(&self) -> bool {
        self.ordinal_shape_valid() && self.response_metadata_shape_valid()
    }

    pub fn input_shape_valid_for_coverage(&self) -> bool {
        if !self.envelope_metadata_shape_valid() {
            return false;
        }
        if let Some(valid) = self.noop_valid() {
            return valid;
        }
        if self.task_running().is_some() {
            return self.task_shape_valid();
        }
        match self.outer_type.as_deref() {
            Some("session_meta") => return self.metadata_shape_valid(),
            Some("token_usage_record") => {
                return !self.duplicate_fields
                    && self.payload_object
                    && self.timestamp_seen
                    && !self.timestamp_malformed
                    && (1..=253_402_300_799).contains(&self.event_timestamp())
                    && self.payload.response.record().is_some()
            }
            _ => {}
        }
        if !self.known_state_input() || self.usage_unparsed() || self.context_unparsed() {
            return false;
        }
        if self.event_type() == Some("token_count") {
            self.payload.info_null
                || self
                    .token_snapshot()
                    .is_some_and(|snapshot| snapshot.valid_raw())
        } else {
            true
        }
    }

    pub fn observe_duplicate(&mut self, object: SessionJsonObject, key: SessionJsonKey) {
        match object {
            SessionJsonObject::Root => self.root_duplicates |= key.bit(),
            SessionJsonObject::Payload => self.payload_duplicates |= key.bit(),
            SessionJsonObject::Info => {
                self.info_duplicates |= matches!(
                    key,
                    SessionJsonKey::TotalTokenUsage
                        | SessionJsonKey::LastTokenUsage
                        | SessionJsonKey::ModelContextWindow
                )
            }
            SessionJsonObject::ThreadSettings => {
                self.thread_settings_duplicates |= key == SessionJsonKey::Model
            }
            SessionJsonObject::TokenUsage | SessionJsonObject::LastTokenUsage => {
                self.counter_duplicates |= key.token_component();
                if key == SessionJsonKey::ReasoningOutputTokens {
                    match object {
                        SessionJsonObject::TokenUsage => {
                            self.payload.token_usage.reasoning_malformed = true
                        }
                        SessionJsonObject::LastTokenUsage => {
                            self.payload.last_token_usage.reasoning_malformed = true
                        }
                        _ => unreachable!(),
                    }
                }
            }
            SessionJsonObject::ResponseSnapshot => self.snapshot_duplicates |= key.response_field(),
            SessionJsonObject::Goal => {
                self.payload.noop.goal.duplicates |= GoalShape::keys().contains(&key);
            }
            SessionJsonObject::ResponseUsage(snapshot)
            | SessionJsonObject::ResponseTurnUsage(snapshot)
            | SessionJsonObject::ResponseThreadUsage(snapshot) => {
                if key.token_component() || key == SessionJsonKey::ReasoningOutputTokens {
                    if snapshot {
                        self.snapshot_duplicates = true;
                    } else {
                        self.response_duplicates = true;
                    }
                }
            }
        }
    }

    pub fn important_duplicates(&self) -> bool {
        let root_has =
            |keys: &[SessionJsonKey]| keys.iter().any(|key| self.root_duplicates & key.bit() != 0);
        let payload_has = |keys: &[SessionJsonKey]| {
            keys.iter()
                .any(|key| self.payload_duplicates & key.bit() != 0)
        };
        if root_has(&[
            SessionJsonKey::Type,
            SessionJsonKey::Timestamp,
            SessionJsonKey::Payload,
            SessionJsonKey::Ordinal,
            SessionJsonKey::Metadata,
        ]) || payload_has(&[SessionJsonKey::Type])
        {
            return true;
        }
        if let Some(kind) = self.noop_kind() {
            return kind
                .keys()
                .iter()
                .any(|key| self.payload_duplicates & key.bit() != 0)
                || (matches!(kind, SessionNoOp::Compacted) && self.snapshot_duplicates)
                || (matches!(kind, SessionNoOp::ThreadGoal) && self.payload.noop.goal.duplicates);
        }
        match self.outer_type.as_deref() {
            Some("session_meta") => {
                return payload_has(&[
                    SessionJsonKey::Id,
                    SessionJsonKey::SessionId,
                    SessionJsonKey::HistoryBase,
                ])
            }
            Some("token_usage_record") => {
                return self.response_duplicates
                    || (self.payload_duplicates != 0
                        && SESSION_JSON_KEYS.iter().any(|(_, key)| {
                            key.response_field() && self.payload_duplicates & key.bit() != 0
                        }))
            }
            Some("compacted") => {
                return self.snapshot_duplicates
                    || payload_has(&[SessionJsonKey::LatestTokenUsageRecord])
            }
            _ => {}
        }
        match self.event_type() {
            Some("token_count") => {
                payload_has(&[SessionJsonKey::Info])
                    || self.info_duplicates
                    || self.counter_duplicates
            }
            Some("turn_context" | "thread_context") => {
                root_has(&[SessionJsonKey::Model])
                    || payload_has(&[
                        SessionJsonKey::Model,
                        SessionJsonKey::TurnId,
                        SessionJsonKey::RootTurnId,
                    ])
            }
            Some("thread_settings_applied") => {
                root_has(&[SessionJsonKey::Model])
                    || payload_has(&[SessionJsonKey::Model, SessionJsonKey::ThreadSettings])
                    || self.thread_settings_duplicates
            }
            _ => false,
        }
    }

    pub fn noop_kind(&self) -> Option<SessionNoOp> {
        match self.outer_type.as_deref()? {
            "response_item" => Some(SessionNoOp::ResponseItem),
            "world_state" => Some(SessionNoOp::WorldState),
            "inter_agent_communication_metadata" => Some(SessionNoOp::InterAgentMetadata),
            "compacted" => Some(SessionNoOp::Compacted),
            "event_msg" => match self.payload.event_type.as_deref()? {
                "mcp_tool_call_begin"
                | "mcp_tool_call_end"
                | "sub_agent_activity"
                | "web_search_end"
                | "patch_apply_end"
                | "context_compacted" => Some(SessionNoOp::ActivityNotice),
                "thread_goal_updated" => Some(SessionNoOp::ThreadGoal),
                "item_completed" => Some(SessionNoOp::ItemCompleted),
                "agent_message" | "user_message" => Some(SessionNoOp::Message),
                "agent_reasoning" | "agent_reasoning_raw_content" => Some(SessionNoOp::Reasoning),
                "agent_reasoning_section_break" => Some(SessionNoOp::ReasoningSection),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn noop_valid(&self) -> Option<bool> {
        let kind = self.noop_kind()?;
        Some(
            !self.duplicate_fields
                && self.payload_object
                && !self.timestamp_malformed
                && self.timestamp_seen
                && (1..=253_402_300_799).contains(&self.event_timestamp())
                && self.payload.noop.valid(kind, &self.payload),
        )
    }

    pub fn event_type(&self) -> Option<&str> {
        match self.outer_type.as_deref() {
            Some(
                "task_started"
                | "task_complete"
                | "task_completed"
                | "turn_aborted"
                | "token_count"
                | "turn_context"
                | "thread_context"
                | "thread_settings_applied",
            ) => self.outer_type.as_deref(),
            Some("event_msg") => self.payload.event_type.as_deref(),
            _ => None,
        }
    }

    pub fn event_model(&self) -> Option<&str> {
        let weak = match self.event_type() {
            Some("turn_context" | "thread_context") => false,
            Some("thread_settings_applied") => true,
            _ => return None,
        };
        if weak && self.payload.thread_settings_seen && !self.payload.thread_settings_object {
            return None;
        }
        let positions = [
            (
                self.root_model_seen,
                self.root_model.as_deref(),
                self.root_model_malformed,
            ),
            (
                self.payload.model_seen,
                self.payload.model.as_deref(),
                self.payload.model_malformed,
            ),
            (
                weak && self.payload.thread_settings_model_seen,
                self.payload.thread_settings_model.as_deref(),
                self.payload.thread_settings_model_malformed,
            ),
        ];
        let mut chosen: Option<&str> = None;
        for (seen, value, malformed) in positions {
            if !seen {
                continue;
            }
            let value = value?;
            if malformed
                || codex_info_db_writer::model_names::normalize_model_name(value).is_none()
                || chosen.is_some_and(|prior| prior.trim() != value.trim())
            {
                return None;
            }
            chosen = Some(value);
        }
        chosen
    }

    pub fn qualified_context_model(&self) -> Option<&str> {
        if !self.context_envelope_valid()
            || (matches!(self.event_type(), Some("turn_context" | "thread_context"))
                && self.payload.response.malformed_identifiers
                    & (SessionJsonKey::TurnId.bit() | SessionJsonKey::RootTurnId.bit())
                    != 0)
        {
            return None;
        }
        self.event_model()
    }

    pub fn context_envelope_valid(&self) -> bool {
        let payload_valid = if self.payload_seen {
            self.payload_object
                && !self.payload.event_type_malformed
                && if self.outer_type.as_deref() == Some("event_msg") {
                    self.payload.event_type_seen && self.payload.event_type.is_some()
                } else {
                    !self.payload.event_type_seen
                        || self.payload.event_type.as_deref() == self.outer_type.as_deref()
                }
        } else {
            matches!(
                self.outer_type.as_deref(),
                Some(
                    "turn_context"
                        | "thread_context"
                        | "thread_settings_applied"
                        | "task_started"
                        | "task_complete"
                        | "task_completed"
                        | "turn_aborted"
                )
            )
        };
        !self.duplicate_fields
            && !self.timestamp_malformed
            && self.timestamp_seen
            && (1..=253_402_300_799).contains(&self.event_timestamp())
            && payload_valid
    }

    pub fn token_snapshot(&self) -> Option<TokenSnapshot> {
        (self.event_type() == Some("token_count") && self.payload_object)
            .then(|| self.payload.token_usage.valid_snapshot())
            .flatten()
    }

    pub fn history_base_continuation(&self) -> bool {
        self.outer_type.as_deref() == Some("session_meta")
            && self.payload_object
            && self.payload.history_base_seen
            && self.payload.history_base_non_null
    }

    pub fn last_token_snapshot(&self) -> Option<TokenSnapshot> {
        (self.event_type() == Some("token_count")
            && self.payload_object
            && self.payload.info_object
            && self.payload.last_token_usage_seen
            && self.payload.last_token_usage_object)
            .then(|| self.payload.last_token_usage.valid_snapshot())
            .flatten()
            .filter(|snapshot| snapshot.valid_raw())
    }

    pub fn context_usage_tokens(&self) -> Option<u64> {
        self.last_token_snapshot().map(|value| value.total)
    }

    pub fn context_window_tokens(&self) -> Option<u64> {
        (self.event_type() == Some("token_count")
            && self.payload_object
            && self.payload.info_object
            && self.payload.context_window_seen
            && !self.payload.context_window_malformed)
            .then_some(self.payload.context_window)
            .flatten()
            .filter(|window| *window > 0)
    }

    pub fn context_pair(&self) -> Option<(u64, u64)> {
        if !self.context_envelope_valid() {
            return None;
        }
        self.context_usage_tokens()
            .zip(self.context_window_tokens())
    }

    pub fn context_unparsed(&self) -> bool {
        if self.event_type() != Some("token_count")
            || !self.context_envelope_valid()
            || !self.payload.info_object
        {
            return false;
        }
        self.payload.context_window_malformed
            || self.payload.context_window == Some(0)
            || (self.payload.last_token_usage_seen
                && !self.payload.last_token_usage_null
                && self.last_token_snapshot().is_none())
    }

    pub fn event_timestamp(&self) -> i64 {
        self.timestamp
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp())
            .unwrap_or(0)
    }

    /// A known event with a field shape that cannot be attributed safely must
    /// not become the end of an incremental checkpoint.  `info: null` is the
    /// producer's valid no-sample token-count envelope and remains a no-op.
    pub fn usage_unparsed(&self) -> bool {
        match self.event_type() {
            Some("token_count") => {
                if !self.context_envelope_valid() || !self.payload.info_seen {
                    return true;
                }
                if self.payload.info_null {
                    return false;
                }
                !self.payload.info_object
                    || !self.payload.token_usage_seen
                    || !self.payload.token_usage_object
                    || !self.payload.token_usage.total_seen
                    || self.payload.token_usage.malformed
            }
            Some("turn_context" | "thread_context" | "thread_settings_applied") => {
                self.qualified_context_model().is_none()
            }
            _ => false,
        }
    }

    pub fn task_running(&self) -> Option<bool> {
        task_running_for_event_type(self.event_type()?)
    }
}

/// Closed task lifecycle mapping shared by local and live consumers.
pub fn task_running_for_event_type(event_type: &str) -> Option<bool> {
    match event_type {
        "task_started" => Some(true),
        "task_complete" | "task_completed" | "turn_aborted" => Some(false),
        _ => None,
    }
}

pub struct SessionRecordInput<'a, R: BufRead> {
    pub reader: &'a mut R,
    pub consumed: u64,
    pub saw_bytes: bool,
    pub terminated: bool,
    pub eof: bool,
    pub depth: usize,
}

impl<'a, R: BufRead> SessionRecordInput<'a, R> {
    pub fn new(reader: &'a mut R) -> Self {
        Self {
            reader,
            consumed: 0,
            saw_bytes: false,
            terminated: false,
            eof: false,
            depth: 0,
        }
    }

    pub fn peek_byte(&mut self) -> Result<Option<u8>, SessionRecordParseError> {
        let buffer = self
            .reader
            .fill_buf()
            .map_err(|_| SessionRecordParseError::Io)?;
        if buffer.is_empty() {
            self.eof = true;
            return Ok(None);
        }
        if buffer[0] == b'\n' {
            return Ok(None);
        }
        Ok(Some(buffer[0]))
    }

    pub fn next_byte(&mut self) -> Result<Option<u8>, SessionRecordParseError> {
        let buffer = self
            .reader
            .fill_buf()
            .map_err(|_| SessionRecordParseError::Io)?;
        if buffer.is_empty() {
            self.eof = true;
            return Ok(None);
        }
        if buffer[0] == b'\n' {
            self.consume(1);
            self.terminated = true;
            return Ok(None);
        }
        let byte = buffer[0];
        self.consume(1);
        self.saw_bytes = true;
        Ok(Some(byte))
    }

    pub fn consume(&mut self, amount: usize) {
        self.reader.consume(amount);
        self.consumed = self.consumed.saturating_add(amount as u64);
    }

    pub fn next_required(&mut self) -> Result<u8, SessionRecordParseError> {
        self.next_byte()?.ok_or(SessionRecordParseError::Syntax)
    }

    pub fn consume_if(&mut self, expected: u8) -> Result<bool, SessionRecordParseError> {
        if self.peek_byte()? == Some(expected) {
            let _ = self.next_byte()?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn skip_whitespace(&mut self) -> Result<(), SessionRecordParseError> {
        while matches!(self.peek_byte()?, Some(b' ' | b'\t' | b'\r')) {
            let _ = self.next_byte()?;
        }
        Ok(())
    }

    pub fn enter_container(&mut self) -> Result<(), SessionRecordParseError> {
        // serde_json's default recursion limit is 128. Keep the same
        // structural guard while allowing record size itself to remain
        // unbounded for streamed payloads.
        if self.depth >= 128 {
            return Err(SessionRecordParseError::Syntax);
        }
        self.depth += 1;
        Ok(())
    }

    pub fn leave_container(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub fn parse_record(&mut self) -> Result<SessionRecordSummary, SessionRecordParseError> {
        self.skip_whitespace()?;
        let mut summary = SessionRecordSummary::default();
        match self.peek_byte()? {
            Some(b'{') => self.parse_object(SessionJsonObject::Root, &mut summary)?,
            Some(b'[') => self.skip_array()?,
            Some(_) => self.skip_value()?,
            None => return Err(SessionRecordParseError::Syntax),
        }
        self.skip_whitespace()?;
        if self.peek_byte()?.is_some() {
            return Err(SessionRecordParseError::Syntax);
        }
        summary.duplicate_fields = summary.important_duplicates();
        if summary.response_metadata_seen && summary.outer_type.as_deref() != Some("response_item")
        {
            summary.unknown_envelope_field = true;
        }
        Ok(summary)
    }

    pub fn parse_object(
        &mut self,
        object: SessionJsonObject,
        summary: &mut SessionRecordSummary,
    ) -> Result<(), SessionRecordParseError> {
        self.enter_container()?;
        let result = (|| {
            if self.next_required()? != b'{' {
                return Err(SessionRecordParseError::Syntax);
            }
            self.skip_whitespace()?;
            if self.consume_if(b'}')? {
                return Ok(());
            }
            let mut seen_keys = 0_u64;
            loop {
                let key = self.parse_key()?;
                if key != SessionJsonKey::Other {
                    let bit = key.bit();
                    if seen_keys & bit != 0 {
                        summary.observe_duplicate(object, key);
                    }
                    seen_keys |= bit;
                }
                self.skip_whitespace()?;
                if self.next_required()? != b':' {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_whitespace()?;
                self.parse_object_value(object, key, summary)?;
                self.skip_whitespace()?;
                if self.consume_if(b'}')? {
                    return Ok(());
                }
                if !self.consume_if(b',')? {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_whitespace()?;
                if self.peek_byte()? == Some(b'}') {
                    return Err(SessionRecordParseError::Syntax);
                }
            }
        })();
        self.leave_container();
        result
    }

    pub fn parse_object_value(
        &mut self,
        object: SessionJsonObject,
        key: SessionJsonKey,
        summary: &mut SessionRecordSummary,
    ) -> Result<(), SessionRecordParseError> {
        match object {
            SessionJsonObject::Root => match key {
                SessionJsonKey::Type => {
                    summary.outer_type = self.parse_text_value()?.0;
                }
                SessionJsonKey::Metadata => {
                    summary.response_metadata_seen = true;
                    summary.response_metadata_null = self.peek_byte()? == Some(b'n');
                    summary.response_metadata_object = self.peek_byte()? == Some(b'{');
                    // This root compartment belongs only to ResponseItem.
                    // Consume its entire JSON without reinterpreting nested
                    // keys as root/payload state or retaining private content.
                    self.skip_value()?;
                }
                SessionJsonKey::Ordinal => {
                    // Position metadata never becomes a counter, timestamp,
                    // identity, or live state. Null and absent remain unknown.
                    summary.ordinal_seen = true;
                    let (_, malformed) = self.parse_u64_value()?;
                    summary.ordinal_malformed |= malformed;
                }
                SessionJsonKey::Timestamp => {
                    summary.timestamp_seen = true;
                    let (value, malformed) = self.parse_text_value()?;
                    summary.timestamp = value;
                    summary.timestamp_malformed = malformed;
                }
                SessionJsonKey::Model => {
                    summary.root_model_seen = true;
                    let (value, malformed) = self.parse_model_value()?;
                    summary.root_model = value;
                    summary.root_model_malformed = malformed;
                }
                SessionJsonKey::Payload => {
                    summary.payload_seen = true;
                    summary.payload = PayloadSummary::default();
                    if self.peek_byte()? == Some(b'{') {
                        summary.payload_object = true;
                        self.parse_object(SessionJsonObject::Payload, summary)?;
                    } else {
                        summary.payload_object = false;
                        self.skip_value()?;
                    }
                }
                _ => {
                    summary.unknown_envelope_field = true;
                    self.skip_value()?;
                }
            },
            SessionJsonObject::Payload => match key {
                SessionJsonKey::Role => {
                    let (value, truncated) = self.parse_text_value()?;
                    summary.payload.noop.role_seen = true;
                    summary.payload.noop.role_is_string = value.is_some();
                    summary.payload.noop.role_is_user =
                        !truncated && value.as_deref() == Some("user");
                }
                SessionJsonKey::Message
                | SessionJsonKey::Text
                | SessionJsonKey::Full
                | SessionJsonKey::State
                | SessionJsonKey::TriggerTurn
                | SessionJsonKey::ReplacementHistory
                | SessionJsonKey::ReplacementHistoryMetadata
                | SessionJsonKey::WindowNumber
                | SessionJsonKey::WindowId
                | SessionJsonKey::FirstWindowId
                | SessionJsonKey::PreviousWindowId
                | SessionJsonKey::ThreadIdCamel
                | SessionJsonKey::TurnIdCamel
                | SessionJsonKey::Goal
                | SessionJsonKey::Item
                | SessionJsonKey::StartedAtMs
                | SessionJsonKey::CompletedAtMs
                | SessionJsonKey::ItemId
                | SessionJsonKey::SummaryIndex => {
                    self.parse_noop_field(key, summary)?;
                }
                SessionJsonKey::Id => {
                    let (value, malformed) = self.parse_identifier_value()?;
                    summary.payload.metadata_id = value;
                    summary.payload.metadata_id_malformed = malformed;
                }
                SessionJsonKey::ThreadId
                | SessionJsonKey::SessionId
                | SessionJsonKey::TurnId
                | SessionJsonKey::RootTurnId
                | SessionJsonKey::ResponseId
                | SessionJsonKey::Usage
                | SessionJsonKey::TurnTokenUsage
                | SessionJsonKey::ThreadTokenUsage => {
                    self.parse_response_field(key, false, summary)?;
                }
                SessionJsonKey::LatestTokenUsageRecord => {
                    summary.payload.snapshot_non_null = self.peek_byte()? != Some(b'n');
                    summary.payload.snapshot = ResponsePayloadSummary::default();
                    if self.peek_byte()? == Some(b'{') {
                        self.parse_object(SessionJsonObject::ResponseSnapshot, summary)?;
                    } else {
                        self.skip_value()?;
                    }
                }
                SessionJsonKey::Type => {
                    summary.payload.event_type_seen = true;
                    let (value, malformed) = self.parse_text_value()?;
                    summary.payload.event_type = value;
                    summary.payload.event_type_malformed = malformed;
                }
                SessionJsonKey::Model => {
                    summary.payload.model_seen = true;
                    let (value, malformed) = self.parse_model_value()?;
                    summary.payload.model = value;
                    summary.payload.model_malformed = malformed;
                }
                SessionJsonKey::ThreadSettings => {
                    summary.payload.thread_settings_seen = true;
                    summary.payload.thread_settings_model = None;
                    summary.payload.thread_settings_model_seen = false;
                    summary.payload.thread_settings_model_malformed = false;
                    if self.peek_byte()? == Some(b'{') {
                        summary.payload.thread_settings_object = true;
                        self.parse_object(SessionJsonObject::ThreadSettings, summary)?;
                    } else {
                        summary.payload.thread_settings_object = false;
                        self.skip_value()?;
                    }
                }
                SessionJsonKey::HistoryBase => {
                    summary.payload.history_base_seen = true;
                    summary.payload.history_base_non_null = self.peek_byte()? != Some(b'n');
                    self.skip_value()?;
                }
                SessionJsonKey::Info => {
                    summary.payload.info_seen = true;
                    summary.payload.info_null = self.peek_byte()? == Some(b'n');
                    summary.payload.info_object = false;
                    summary.payload.token_usage_seen = false;
                    summary.payload.token_usage_object = false;
                    summary.payload.token_usage = TokenUsageSummary::default();
                    if self.peek_byte()? == Some(b'{') {
                        summary.payload.info_object = true;
                        self.parse_object(SessionJsonObject::Info, summary)?;
                    } else {
                        self.skip_value()?;
                    }
                }
                _ => self.skip_value()?,
            },
            SessionJsonObject::ThreadSettings => {
                if key == SessionJsonKey::Model {
                    summary.payload.thread_settings_model_seen = true;
                    let (value, malformed) = self.parse_model_value()?;
                    summary.payload.thread_settings_model = value;
                    summary.payload.thread_settings_model_malformed = malformed;
                } else {
                    self.skip_value()?;
                }
            }
            SessionJsonObject::Info => {
                if key == SessionJsonKey::TotalTokenUsage {
                    summary.payload.token_usage_seen = true;
                    summary.payload.token_usage_object = false;
                    summary.payload.token_usage = TokenUsageSummary::default();
                    if self.peek_byte()? == Some(b'{') {
                        summary.payload.token_usage_object = true;
                        self.parse_object(SessionJsonObject::TokenUsage, summary)?;
                        if !summary.payload.token_usage.total_seen {
                            summary.payload.token_usage.malformed = true;
                        }
                    } else {
                        self.skip_value()?;
                        summary.payload.token_usage.malformed = true;
                    }
                } else if key == SessionJsonKey::LastTokenUsage {
                    summary.payload.last_token_usage_seen = true;
                    summary.payload.last_token_usage_null = self.peek_byte()? == Some(b'n');
                    summary.payload.last_token_usage_object = false;
                    summary.payload.last_token_usage = TokenUsageSummary::default();
                    if self.peek_byte()? == Some(b'{') {
                        summary.payload.last_token_usage_object = true;
                        self.parse_object(SessionJsonObject::LastTokenUsage, summary)?;
                        if !summary.payload.last_token_usage.total_seen {
                            summary.payload.last_token_usage.malformed = true;
                        }
                    } else {
                        self.skip_value()?;
                        summary.payload.last_token_usage.malformed = true;
                    }
                } else if key == SessionJsonKey::ModelContextWindow {
                    summary.payload.context_window_seen = true;
                    let (value, malformed) = self.parse_u64_value()?;
                    summary.payload.context_window = value;
                    summary.payload.context_window_malformed = malformed;
                } else {
                    self.skip_value()?;
                }
            }
            SessionJsonObject::Goal => self.parse_goal_field(key, summary)?,
            SessionJsonObject::ResponseSnapshot => self.parse_response_field(key, true, summary)?,
            SessionJsonObject::TokenUsage
            | SessionJsonObject::LastTokenUsage
            | SessionJsonObject::ResponseUsage(_)
            | SessionJsonObject::ResponseTurnUsage(_)
            | SessionJsonObject::ResponseThreadUsage(_) => {
                let token_usage = match object {
                    SessionJsonObject::TokenUsage => &mut summary.payload.token_usage,
                    SessionJsonObject::LastTokenUsage => &mut summary.payload.last_token_usage,
                    SessionJsonObject::ResponseUsage(snapshot) => {
                        if snapshot {
                            &mut summary.payload.snapshot.usage
                        } else {
                            &mut summary.payload.response.usage
                        }
                    }
                    SessionJsonObject::ResponseTurnUsage(snapshot) => {
                        if snapshot {
                            &mut summary.payload.snapshot.turn_usage
                        } else {
                            &mut summary.payload.response.turn_usage
                        }
                    }
                    SessionJsonObject::ResponseThreadUsage(snapshot) => {
                        if snapshot {
                            &mut summary.payload.snapshot.thread_usage
                        } else {
                            &mut summary.payload.response.thread_usage
                        }
                    }
                    _ => unreachable!("token usage parser object is exhaustive"),
                };
                match key {
                    SessionJsonKey::TotalTokens => {
                        token_usage.total_seen = true;
                        let (value, malformed) = self.parse_u64_value()?;
                        token_usage.total = value;
                        token_usage.malformed |= malformed || value.is_none();
                    }
                    SessionJsonKey::CacheWriteInputTokens => {
                        let (value, malformed) = self.parse_u64_value()?;
                        token_usage.cache_write_input = value;
                        token_usage.malformed |= malformed;
                    }
                    SessionJsonKey::InputTokens => {
                        let (value, malformed) = self.parse_u64_value()?;
                        token_usage.input = value;
                        token_usage.malformed |= malformed;
                    }
                    SessionJsonKey::CachedInputTokens => {
                        let (value, malformed) = self.parse_u64_value()?;
                        token_usage.cached_input = value;
                        token_usage.malformed |= malformed;
                    }
                    SessionJsonKey::OutputTokens => {
                        let (value, malformed) = self.parse_u64_value()?;
                        token_usage.output = value;
                        token_usage.malformed |= malformed;
                    }
                    SessionJsonKey::ReasoningOutputTokens => {
                        let (value, malformed) = self.parse_u64_value()?;
                        token_usage.reasoning_output = value;
                        token_usage.reasoning_malformed |= malformed;
                    }
                    _ => self.skip_value()?,
                }
            }
        }
        Ok(())
    }

    pub fn parse_response_field(
        &mut self,
        key: SessionJsonKey,
        snapshot: bool,
        summary: &mut SessionRecordSummary,
    ) -> Result<(), SessionRecordParseError> {
        match key {
            SessionJsonKey::ThreadId
            | SessionJsonKey::SessionId
            | SessionJsonKey::TurnId
            | SessionJsonKey::RootTurnId
            | SessionJsonKey::ResponseId => {
                let (value, malformed) = self.parse_identifier_value()?;
                let target = if snapshot {
                    &mut summary.payload.snapshot
                } else {
                    &mut summary.payload.response
                };
                if malformed {
                    target.malformed_identifiers |= key.bit();
                }
                match key {
                    SessionJsonKey::ThreadId => target.thread_id = value,
                    SessionJsonKey::SessionId => {
                        target.session_id_seen = true;
                        target.session_id = value;
                    }
                    SessionJsonKey::TurnId => target.turn_id = value,
                    SessionJsonKey::RootTurnId => target.root_turn_id = value,
                    SessionJsonKey::ResponseId => target.response_id = value,
                    _ => unreachable!("only response ID keys enter this arm"),
                }
            }
            SessionJsonKey::Usage
            | SessionJsonKey::TurnTokenUsage
            | SessionJsonKey::ThreadTokenUsage => {
                let target = if snapshot {
                    &mut summary.payload.snapshot
                } else {
                    &mut summary.payload.response
                };
                let usage = match key {
                    SessionJsonKey::Usage => &mut target.usage,
                    SessionJsonKey::TurnTokenUsage => &mut target.turn_usage,
                    SessionJsonKey::ThreadTokenUsage => &mut target.thread_usage,
                    _ => unreachable!("only response usage keys enter this arm"),
                };
                *usage = TokenUsageSummary::default();
                if self.peek_byte()? == Some(b'{') {
                    let object = match key {
                        SessionJsonKey::Usage => SessionJsonObject::ResponseUsage(snapshot),
                        SessionJsonKey::TurnTokenUsage => {
                            SessionJsonObject::ResponseTurnUsage(snapshot)
                        }
                        SessionJsonKey::ThreadTokenUsage => {
                            SessionJsonObject::ResponseThreadUsage(snapshot)
                        }
                        _ => unreachable!("only response usage keys enter this arm"),
                    };
                    self.parse_object(object, summary)?;
                } else {
                    usage.malformed = true;
                    self.skip_value()?;
                }
            }
            _ => self.skip_value()?,
        }
        Ok(())
    }

    pub fn parse_noop_field(
        &mut self,
        key: SessionJsonKey,
        summary: &mut SessionRecordSummary,
    ) -> Result<(), SessionRecordParseError> {
        use SessionJsonKey::*;
        let valid = match key {
            Message | Text | ItemId => {
                let valid = self.peek_byte()? == Some(b'"');
                self.skip_value()?;
                valid
            }
            Full | TriggerTurn => {
                let valid = matches!(self.peek_byte()?, Some(b't' | b'f'));
                self.skip_value()?;
                valid
            }
            State => {
                self.skip_value()?;
                true
            }
            Item => {
                let valid = self.peek_byte()? == Some(b'{');
                self.skip_value()?;
                valid
            }
            ThreadIdCamel | TurnIdCamel => {
                let (value, malformed) = self.parse_identifier_value()?;
                let valid = !malformed && (key == TurnIdCamel || value.is_some());
                if key == ThreadIdCamel {
                    summary.payload.noop.thread_id = value;
                }
                valid
            }
            Goal => {
                let valid = self.peek_byte()? == Some(b'{');
                summary.payload.noop.goal = GoalShape::default();
                if valid {
                    self.parse_object(SessionJsonObject::Goal, summary)?;
                } else {
                    self.skip_value()?;
                }
                valid
            }
            SummaryIndex => self.parse_signed_shape(false)?,
            StartedAtMs | CompletedAtMs => {
                let (value, malformed) = self.parse_u64_value()?;
                !malformed
                    && value.is_none_or(|v| v <= i64::MAX as u64)
                    && (key == StartedAtMs || value.is_some())
            }
            WindowNumber => {
                let (_, malformed) = self.parse_u64_value()?;
                !malformed
            }
            FirstWindowId | PreviousWindowId => {
                let (_, malformed) = self.parse_identifier_value()?;
                !malformed
            }
            WindowId => {
                if self.peek_byte()? == Some(b'"') {
                    let (_, malformed) = self.parse_identifier_value()?;
                    !malformed
                } else {
                    let (_, malformed) = self.parse_u64_value()?;
                    !malformed
                }
            }
            ReplacementHistory | ReplacementHistoryMetadata => {
                let (count, valid) = self.parse_object_array_shape()?;
                if key == ReplacementHistory {
                    summary.payload.noop.replacement_history = count;
                } else {
                    summary.payload.noop.replacement_metadata = count;
                }
                valid
            }
            _ => {
                self.skip_value()?;
                return Ok(());
            }
        };
        summary.payload.noop.field(key, valid);
        Ok(())
    }

    pub fn parse_goal_field(
        &mut self,
        key: SessionJsonKey,
        summary: &mut SessionRecordSummary,
    ) -> Result<(), SessionRecordParseError> {
        use SessionJsonKey::*;
        if !GoalShape::keys().contains(&key) {
            return self.skip_value();
        }
        let valid = match key {
            ThreadIdCamel => {
                let (value, malformed) = self.parse_identifier_value()?;
                let valid = !malformed && value.is_some();
                summary.payload.noop.goal.thread_id = value;
                valid
            }
            Objective => self.parse_string_scalar_shape(1, 4000)?,
            Status => {
                let (value, malformed) = self.parse_text_value()?;
                !malformed
                    && matches!(
                        value.as_deref(),
                        Some(
                            "active"
                                | "paused"
                                | "blocked"
                                | "usageLimited"
                                | "budgetLimited"
                                | "complete"
                        )
                    )
            }
            TokenBudget => self.parse_signed_shape(true)?,
            TokensUsed | TimeUsedSeconds | CreatedAt | UpdatedAt => {
                self.parse_signed_shape(false)?
            }
            _ => unreachable!("goal keys checked above"),
        };
        summary.payload.noop.goal.seen |= key.bit();
        if !valid {
            summary.payload.noop.goal.malformed |= key.bit();
        }
        Ok(())
    }

    pub fn parse_signed_shape(&mut self, nullable: bool) -> Result<bool, SessionRecordParseError> {
        if self.peek_byte()? == Some(b'n') {
            self.skip_value()?;
            return Ok(nullable);
        }
        if !matches!(self.peek_byte()?, Some(b'-' | b'0'..=b'9')) {
            self.skip_value()?;
            return Ok(false);
        }
        let negative = self.consume_if(b'-')?;
        if !matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
            return Err(SessionRecordParseError::Syntax);
        }
        let magnitude = self.parse_number()?;
        Ok(magnitude.is_some_and(|v| v <= i64::MAX as u64 + u64::from(negative)))
    }

    pub fn parse_string_scalar_shape(
        &mut self,
        min: usize,
        max: usize,
    ) -> Result<bool, SessionRecordParseError> {
        if self.peek_byte()? != Some(b'"') {
            self.skip_value()?;
            return Ok(false);
        }
        self.next_required()?;
        let mut count = 0usize;
        loop {
            match self.next_required()? {
                b'"' => return Ok((min..=max).contains(&count)),
                b'\\' => {
                    self.parse_escape_character()?;
                }
                byte if byte < 0x20 => return Err(SessionRecordParseError::Syntax),
                byte if byte >= 0x80 => {
                    self.consume_utf8_character(byte)?;
                }
                _ => {}
            }
            count = count.saturating_add(1).min(max + 1);
        }
    }

    pub fn parse_object_array_shape(
        &mut self,
    ) -> Result<(Option<u64>, bool), SessionRecordParseError> {
        if self.peek_byte()? == Some(b'n') {
            self.skip_value()?;
            return Ok((None, true));
        }
        if self.peek_byte()? != Some(b'[') {
            self.skip_value()?;
            return Ok((None, false));
        }
        self.enter_container()?;
        let result = (|| {
            self.next_required()?;
            self.skip_whitespace()?;
            let mut count = 0u64;
            let mut valid = true;
            if self.consume_if(b']')? {
                return Ok((Some(count), valid));
            }
            loop {
                valid &= self.peek_byte()? == Some(b'{');
                self.skip_value()?;
                count = count
                    .checked_add(1)
                    .ok_or(SessionRecordParseError::Syntax)?;
                self.skip_whitespace()?;
                if self.consume_if(b']')? {
                    return Ok((Some(count), valid));
                }
                if !self.consume_if(b',')? {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_whitespace()?;
                if self.peek_byte()? == Some(b']') {
                    return Err(SessionRecordParseError::Syntax);
                }
            }
        })();
        self.leave_container();
        result
    }

    pub fn parse_key(&mut self) -> Result<SessionJsonKey, SessionRecordParseError> {
        if self.next_required()? != b'"' {
            return Err(SessionRecordParseError::Syntax);
        }
        let mut matches = [true; SESSION_JSON_KEYS.len()];
        let mut length = 0usize;
        loop {
            let byte = self.next_required()?;
            match byte {
                b'"' => break,
                b'\\' => {
                    let character = self.parse_escape_character()?;
                    if character.is_ascii() {
                        let byte = character as u8;
                        for (index, (candidate, _)) in SESSION_JSON_KEYS.iter().enumerate() {
                            if matches[index] && candidate.get(length).copied() != Some(byte) {
                                matches[index] = false;
                            }
                        }
                    } else {
                        matches.fill(false);
                    }
                    length = length.saturating_add(1);
                }
                byte if byte < 0x20 => return Err(SessionRecordParseError::Syntax),
                byte if byte < 0x80 => {
                    for (index, (candidate, _)) in SESSION_JSON_KEYS.iter().enumerate() {
                        if matches[index] && candidate.get(length).copied() != Some(byte) {
                            matches[index] = false;
                        }
                    }
                    length = length.saturating_add(1);
                }
                byte => {
                    self.consume_utf8_character(byte)?;
                    matches.fill(false);
                    length = length.saturating_add(1);
                }
            }
        }
        Ok(SESSION_JSON_KEYS
            .iter()
            .enumerate()
            .find_map(|(index, (candidate, key))| {
                (matches[index] && candidate.len() == length).then_some(*key)
            })
            .unwrap_or(SessionJsonKey::Other))
    }

    pub fn parse_text_value(&mut self) -> Result<(Option<String>, bool), SessionRecordParseError> {
        if self.peek_byte()? == Some(b'"') {
            let (value, truncated) = self.parse_text()?;
            return Ok((Some(value), truncated));
        }
        let is_null = self.peek_byte()? == Some(b'n');
        self.skip_value()?;
        Ok((None, !is_null))
    }

    pub fn parse_identifier_value(
        &mut self,
    ) -> Result<(Option<String>, bool), SessionRecordParseError> {
        if self.peek_byte()? != Some(b'"') {
            return self.parse_text_value();
        }
        // 512 Unicode scalars occupy at most 2048 UTF-8 bytes. Scan the
        // entire string even after overflow; a truncated ID is never admitted.
        let (value, truncated) = self.parse_text_limit(512 * 4)?;
        let valid = !truncated && source_admission::valid_identifier(&value);
        Ok((valid.then_some(value), !valid))
    }

    pub fn parse_model_value(&mut self) -> Result<(Option<String>, bool), SessionRecordParseError> {
        if self.peek_byte()? != Some(b'"') {
            return self.parse_text_value();
        }
        self.next_required()?;
        let mut builder = codex_info_db_writer::model_names::ModelNameBuilder::default();
        loop {
            let character = match self.next_required()? {
                b'"' => {
                    let model = builder.finish();
                    let malformed = model.is_none();
                    return Ok((model, malformed));
                }
                b'\\' => self.parse_escape_character()?,
                byte if byte < 0x20 => return Err(SessionRecordParseError::Syntax),
                byte if byte < 0x80 => byte as char,
                byte => {
                    let (bytes, length) = self.read_utf8_character(byte)?;
                    std::str::from_utf8(&bytes[..length])
                        .map_err(|_| SessionRecordParseError::Syntax)?
                        .chars()
                        .next()
                        .ok_or(SessionRecordParseError::Syntax)?
                }
            };
            builder.push(character);
        }
    }

    pub fn parse_text(&mut self) -> Result<(String, bool), SessionRecordParseError> {
        self.parse_text_limit(codex_info_db_writer::MAX_SESSION_MODEL_BYTES)
    }

    pub fn parse_text_limit(
        &mut self,
        limit: usize,
    ) -> Result<(String, bool), SessionRecordParseError> {
        if self.next_required()? != b'"' {
            return Err(SessionRecordParseError::Syntax);
        }
        let mut text = String::with_capacity(limit);
        let mut truncated = false;
        loop {
            let byte = self.next_required()?;
            match byte {
                b'"' => return Ok((text, truncated)),
                b'\\' => {
                    let character = self.parse_escape_character()?;
                    if text.len().saturating_add(character.len_utf8()) <= limit {
                        text.push(character);
                    } else {
                        truncated = true;
                    }
                }
                byte if byte < 0x20 => return Err(SessionRecordParseError::Syntax),
                byte if byte < 0x80 => {
                    if text.len() < limit {
                        text.push(byte as char);
                    } else {
                        truncated = true;
                    }
                }
                byte => {
                    let (bytes, length) = self.read_utf8_character(byte)?;
                    let value = std::str::from_utf8(&bytes[..length])
                        .map_err(|_| SessionRecordParseError::Syntax)?;
                    if text.len().saturating_add(value.len()) <= limit {
                        text.push_str(value);
                    } else {
                        truncated = true;
                    }
                }
            }
        }
    }

    pub fn skip_string(&mut self) -> Result<(), SessionRecordParseError> {
        if self.next_required()? != b'"' {
            return Err(SessionRecordParseError::Syntax);
        }
        loop {
            let byte = self.next_required()?;
            match byte {
                b'"' => return Ok(()),
                b'\\' => {
                    self.parse_escape_character()?;
                }
                byte if byte < 0x20 => return Err(SessionRecordParseError::Syntax),
                byte if byte < 0x80 => {}
                byte => {
                    self.consume_utf8_character(byte)?;
                }
            }
        }
    }

    pub fn parse_escape_character(&mut self) -> Result<char, SessionRecordParseError> {
        match self.next_required()? {
            b'"' => Ok('"'),
            b'\\' => Ok('\\'),
            b'/' => Ok('/'),
            b'b' => Ok('\u{0008}'),
            b'f' => Ok('\u{000c}'),
            b'n' => Ok('\n'),
            b'r' => Ok('\r'),
            b't' => Ok('\t'),
            b'u' => {
                let first = self.parse_hex_quad()?;
                if (0xD800..=0xDBFF).contains(&first) {
                    if self.next_required()? != b'\\' || self.next_required()? != b'u' {
                        return Err(SessionRecordParseError::Syntax);
                    }
                    let second = self.parse_hex_quad()?;
                    if !(0xDC00..=0xDFFF).contains(&second) {
                        return Err(SessionRecordParseError::Syntax);
                    }
                    let codepoint = 0x1_0000
                        + ((u32::from(first) - 0xD800) << 10)
                        + (u32::from(second) - 0xDC00);
                    char::from_u32(codepoint).ok_or(SessionRecordParseError::Syntax)
                } else if (0xDC00..=0xDFFF).contains(&first) {
                    Err(SessionRecordParseError::Syntax)
                } else {
                    char::from_u32(u32::from(first)).ok_or(SessionRecordParseError::Syntax)
                }
            }
            _ => Err(SessionRecordParseError::Syntax),
        }
    }

    pub fn parse_hex_quad(&mut self) -> Result<u16, SessionRecordParseError> {
        let mut value = 0u16;
        for _ in 0..4 {
            value = (value << 4)
                | u16::from(
                    char::from(self.next_required()?)
                        .to_digit(16)
                        .ok_or(SessionRecordParseError::Syntax)? as u8,
                );
        }
        Ok(value)
    }

    pub fn read_utf8_character(
        &mut self,
        first: u8,
    ) -> Result<([u8; 4], usize), SessionRecordParseError> {
        let length = match first {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => return Err(SessionRecordParseError::Syntax),
        };
        let mut bytes = [0u8; 4];
        bytes[0] = first;
        for byte in &mut bytes[1..length] {
            *byte = self.next_required()?;
            if !(*byte >= 0x80 && *byte <= 0xBF) {
                return Err(SessionRecordParseError::Syntax);
            }
        }
        std::str::from_utf8(&bytes[..length]).map_err(|_| SessionRecordParseError::Syntax)?;
        Ok((bytes, length))
    }

    pub fn consume_utf8_character(&mut self, first: u8) -> Result<(), SessionRecordParseError> {
        let _ = self.read_utf8_character(first)?;
        Ok(())
    }

    pub fn parse_u64_value(&mut self) -> Result<(Option<u64>, bool), SessionRecordParseError> {
        match self.peek_byte()? {
            Some(b'-' | b'0'..=b'9') => {
                let value = self.parse_number()?;
                Ok((value, value.is_none()))
            }
            Some(b'n') => {
                self.skip_value()?;
                Ok((None, false))
            }
            Some(_) => {
                self.skip_value()?;
                Ok((None, true))
            }
            None => Err(SessionRecordParseError::Syntax),
        }
    }

    pub fn parse_number(&mut self) -> Result<Option<u64>, SessionRecordParseError> {
        let negative = self.consume_if(b'-')?;
        let mut integer = true;
        let mut overflow = false;
        let mut value = 0u64;
        match self.next_byte()? {
            Some(b'0') => {
                if matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                    return Err(SessionRecordParseError::Syntax);
                }
            }
            Some(byte @ b'1'..=b'9') => {
                value = u64::from(byte - b'0');
                while let Some(byte @ b'0'..=b'9') = self.peek_byte()? {
                    let _ = self.next_byte()?;
                    if let Some(next) = value
                        .checked_mul(10)
                        .and_then(|value| value.checked_add(u64::from(byte - b'0')))
                    {
                        value = next;
                    } else {
                        overflow = true;
                    }
                }
            }
            _ => return Err(SessionRecordParseError::Syntax),
        }
        if self.consume_if(b'.')? {
            integer = false;
            if !matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                return Err(SessionRecordParseError::Syntax);
            }
            while matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                let _ = self.next_byte()?;
            }
        }
        if matches!(self.peek_byte()?, Some(b'e' | b'E')) {
            integer = false;
            let _ = self.next_byte()?;
            if !self.consume_if(b'+')? {
                let _ = self.consume_if(b'-')?;
            }
            if !matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                return Err(SessionRecordParseError::Syntax);
            }
            while matches!(self.peek_byte()?, Some(b'0'..=b'9')) {
                let _ = self.next_byte()?;
            }
        }
        Ok((!negative && integer && !overflow).then_some(value))
    }

    pub fn parse_literal(&mut self, literal: &[u8]) -> Result<(), SessionRecordParseError> {
        for expected in literal {
            if self.next_required()? != *expected {
                return Err(SessionRecordParseError::Syntax);
            }
        }
        Ok(())
    }

    pub fn skip_value(&mut self) -> Result<(), SessionRecordParseError> {
        self.skip_whitespace()?;
        match self.peek_byte()? {
            Some(b'"') => self.skip_string(),
            Some(b'{') => self.skip_object(),
            Some(b'[') => self.skip_array(),
            Some(b'n') => self.parse_literal(b"null"),
            Some(b't') => self.parse_literal(b"true"),
            Some(b'f') => self.parse_literal(b"false"),
            Some(b'-' | b'0'..=b'9') => self.parse_number().map(|_| ()),
            Some(_) | None => Err(SessionRecordParseError::Syntax),
        }
    }

    pub fn skip_object(&mut self) -> Result<(), SessionRecordParseError> {
        self.enter_container()?;
        let result = (|| {
            if self.next_required()? != b'{' {
                return Err(SessionRecordParseError::Syntax);
            }
            self.skip_whitespace()?;
            if self.consume_if(b'}')? {
                return Ok(());
            }
            loop {
                self.skip_string()?;
                self.skip_whitespace()?;
                if self.next_required()? != b':' {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_value()?;
                self.skip_whitespace()?;
                if self.consume_if(b'}')? {
                    return Ok(());
                }
                if !self.consume_if(b',')? {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_whitespace()?;
                if self.peek_byte()? == Some(b'}') {
                    return Err(SessionRecordParseError::Syntax);
                }
            }
        })();
        self.leave_container();
        result
    }

    pub fn skip_array(&mut self) -> Result<(), SessionRecordParseError> {
        self.enter_container()?;
        let result = (|| {
            if self.next_required()? != b'[' {
                return Err(SessionRecordParseError::Syntax);
            }
            self.skip_whitespace()?;
            if self.consume_if(b']')? {
                return Ok(());
            }
            loop {
                self.skip_value()?;
                self.skip_whitespace()?;
                if self.consume_if(b']')? {
                    return Ok(());
                }
                if !self.consume_if(b',')? {
                    return Err(SessionRecordParseError::Syntax);
                }
                self.skip_whitespace()?;
                if self.peek_byte()? == Some(b']') {
                    return Err(SessionRecordParseError::Syntax);
                }
            }
        })();
        self.leave_container();
        result
    }

    pub fn finish_record(&mut self) -> Result<bool, SessionRecordParseError> {
        if self.terminated {
            return Ok(true);
        }
        let buffer = self
            .reader
            .fill_buf()
            .map_err(|_| SessionRecordParseError::Io)?;
        if buffer.is_empty() {
            self.eof = true;
            return Ok(false);
        }
        if buffer[0] == b'\n' {
            self.consume(1);
            self.terminated = true;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn drain_record(&mut self) -> Result<bool, SessionRecordParseError> {
        if self.terminated {
            return Ok(true);
        }
        loop {
            let buffer = self
                .reader
                .fill_buf()
                .map_err(|_| SessionRecordParseError::Io)?;
            if buffer.is_empty() {
                self.eof = true;
                return Ok(false);
            }
            if let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
                self.saw_bytes |= position > 0;
                self.consume(position + 1);
                self.terminated = true;
                return Ok(true);
            }
            self.saw_bytes = true;
            let length = buffer.len();
            self.consume(length);
        }
    }
}

pub fn read_streaming_record<R: BufRead, E: From<io::Error>>(
    reader: &mut R,
    invalid: impl Fn(&'static str) -> E,
) -> Result<RecordRead, E> {
    if reader.fill_buf()?.is_empty() {
        return Ok(RecordRead::End);
    }
    let mut input = SessionRecordInput::new(reader);
    let parsed = input.parse_record();
    if matches!(parsed.as_ref(), Err(SessionRecordParseError::Io)) {
        return Err(invalid("session source read failed while parsing"));
    }
    if parsed.is_err() {
        if !input.saw_bytes && input.eof && !input.terminated {
            return Ok(RecordRead::End);
        }
        let terminated = if input.terminated {
            true
        } else {
            input
                .drain_record()
                .map_err(|_| invalid("session source read failed while draining"))?
        };
        if terminated {
            return Ok(RecordRead::Invalid(input.consumed, true));
        }
        return Ok(RecordRead::Unterminated(input.consumed));
    }
    if input
        .finish_record()
        .map_err(|_| invalid("session source read failed while framing"))?
    {
        return Ok(RecordRead::Present(
            Box::new(parsed.expect("parsed session record")),
            input.consumed,
        ));
    }
    Ok(RecordRead::Unterminated(input.consumed))
}

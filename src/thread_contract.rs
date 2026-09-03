// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

//! Pure contracts for the pinned `thread/list` response.
//!
//! This module deliberately keeps the wire schema and the validation errors
//! categorical. It does not perform RPC, filesystem, or rollout work;
//! pagination state is kept private in the cycle accumulator.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{BufRead, Cursor};
use std::path::Path;
use std::sync::OnceLock;

use chrono::DateTime;
use serde_json::{json, Map, Value};

use crate::security;

const RESPONSE_SCHEMA_TEXT: &str =
    include_str!("../protocol/thread/ThreadListResponse.canonical.json");
const PARAMS_SCHEMA_TEXT: &str = include_str!("../protocol/thread/ThreadListParams.canonical.json");
const MANIFEST_TEXT: &str = include_str!("../protocol/thread/THREAD_SCHEMA_MANIFEST.json");

/// The generated schema bundle identity is intentionally public and fixed.
pub const THREAD_SCHEMA_MANIFEST_ID: &str = "CODEX_INFO_THREAD_SCHEMA_MANIFEST_V1";
pub const SCHEMA_MANIFEST_ID: &str = THREAD_SCHEMA_MANIFEST_ID;
pub const THREAD_SCHEMA_CLI_VERSION: &str = "0.147.0";
pub const PINNED_CLI_VERSION: &str = THREAD_SCHEMA_CLI_VERSION;
pub const THREAD_SCHEMA_UTC_DATE: &str = "2026-08-14";
pub const PINNED_SCHEMA_DATE_UTC: &str = THREAD_SCHEMA_UTC_DATE;
pub const THREAD_SCHEMA_BUNDLE_RAW_SHA256: &str =
    "f3dec1e031d99a420b137b903f02196d4325eece57620c925bb7130b25f168d2";
pub const THREAD_SCHEMA_PARAMS_RAW_SHA256: &str =
    "b227bb78acf9b91060d03c56d3f2072cdd9f1bd08290c11e8869f1a663b16da2";
pub const THREAD_SCHEMA_PARAMS_CANONICAL_SHA256: &str =
    "6a63582e96c9092edcdc19935484cadcd72a1ae128762f6d666fc2017596d310";
pub const THREAD_SCHEMA_RESPONSE_RAW_SHA256: &str =
    "08d5ffb0a799cf0d1c42c11b12c8bc4b04d6e515f96c6789bbec532eba1b2612";
pub const THREAD_SCHEMA_RESPONSE_CANONICAL_SHA256: &str =
    "f3d94a229732a0756eb8c6698e325c05a1105fb2b6fe668814b8e3277a21f130";

/// Exact public source-kind order pinned by the generated request schema.
pub const SOURCE_KINDS: [&str; 10] = [
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];
pub const THREAD_SOURCE_KINDS: [&str; 10] = SOURCE_KINDS;

const MAX_CURSOR_SCALARS: usize = 1024;
const MAX_VALIDATION_DEPTH: usize = 64;
const MAX_VALIDATION_WORK: usize = 100_000;
const MAX_THREAD_PAGES: usize = 32;
const MAX_UNIQUE_THREAD_IDS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadContractError {
    InvalidCursor,
    InvalidRequest,
    InvalidEnvelope,
    InvalidItem,
    InvalidSchema,
    ValidationBudgetExceeded,
    InvalidManifest,
}

impl ThreadContractError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidCursor => "thread cursor rejected",
            Self::InvalidRequest => "thread request rejected",
            Self::InvalidEnvelope => "thread page envelope rejected",
            Self::InvalidItem => "thread item rejected",
            Self::InvalidSchema => "thread schema rejected",
            Self::ValidationBudgetExceeded => "thread validation budget exceeded",
            Self::InvalidManifest => "thread schema manifest rejected",
        }
    }
}

impl fmt::Display for ThreadContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ThreadContractError {}

/// The six fixed fields of the first `thread/list` request, with an optional
/// opaque cursor added only for a follow-up request.
pub fn thread_list_request(cursor: Option<&str>) -> Result<Value, ThreadContractError> {
    if let Some(cursor) = cursor {
        validate_cursor(cursor)?;
    }

    let mut request = Map::new();
    request.insert("archived".to_owned(), Value::Bool(false));
    request.insert("limit".to_owned(), Value::from(100_u64));
    request.insert("sortKey".to_owned(), Value::String("updated_at".to_owned()));
    request.insert("sortDirection".to_owned(), Value::String("desc".to_owned()));
    request.insert(
        "sourceKinds".to_owned(),
        Value::Array(
            SOURCE_KINDS
                .iter()
                .map(|value| Value::String((*value).to_owned()))
                .collect(),
        ),
    );
    request.insert("useStateDbOnly".to_owned(), Value::Bool(false));
    if let Some(cursor) = cursor {
        request.insert("cursor".to_owned(), Value::String(cursor.to_owned()));
    }
    Ok(Value::Object(request))
}

fn validate_cursor(cursor: &str) -> Result<(), ThreadContractError> {
    let length = cursor.chars().count();
    if (1..=MAX_CURSOR_SCALARS).contains(&length) {
        Ok(())
    } else {
        Err(ThreadContractError::InvalidCursor)
    }
}

fn schema_document() -> Result<&'static Value, ThreadContractError> {
    static SCHEMA: OnceLock<Result<Value, ThreadContractError>> = OnceLock::new();
    match SCHEMA.get_or_init(|| {
        serde_json::from_str(RESPONSE_SCHEMA_TEXT).map_err(|_| ThreadContractError::InvalidSchema)
    }) {
        Ok(schema) => Ok(schema),
        Err(error) => Err(*error),
    }
}

fn params_schema_document() -> Result<&'static Value, ThreadContractError> {
    static SCHEMA: OnceLock<Result<Value, ThreadContractError>> = OnceLock::new();
    match SCHEMA.get_or_init(|| {
        serde_json::from_str(PARAMS_SCHEMA_TEXT).map_err(|_| ThreadContractError::InvalidSchema)
    }) {
        Ok(schema) => Ok(schema),
        Err(error) => Err(*error),
    }
}

/// Return the compile-time vendored response schema.  The returned value is
/// immutable and contains the complete generated artifact, not a display-only
/// subset.
pub fn thread_list_response_schema() -> Result<&'static Value, ThreadContractError> {
    schema_document()
}

/// Return the compile-time vendored request schema.
pub fn thread_list_params_schema() -> Result<&'static Value, ThreadContractError> {
    params_schema_document()
}

/// Validate an instance against any local Draft-07 schema using the exact
/// assertion subset used by the generated artifact.  Errors are categorical;
/// neither a JSON path nor an input value is retained.
pub fn validate_instance_against_schema(
    instance: &Value,
    schema: &Value,
) -> Result<(), ThreadContractError> {
    validate_instance_with_root(instance, schema, schema)
}

fn validate_instance_with_root(
    instance: &Value,
    subschema: &Value,
    root_schema: &Value,
) -> Result<(), ThreadContractError> {
    let mut state = ValidationState::default();
    validate_node(instance, subschema, root_schema, &mut state, 0)
        .map_err(|failure| failure.error())
}

/// Short alias useful to independent contract tests.
pub fn validate_schema(instance: &Value, schema: &Value) -> Result<(), ThreadContractError> {
    validate_instance_against_schema(instance, schema)
}

#[derive(Default)]
struct ValidationState {
    work: usize,
}

#[derive(Clone, Copy)]
struct ValidationFailure(ThreadContractError);

impl ValidationFailure {
    const fn schema() -> Self {
        Self(ThreadContractError::InvalidSchema)
    }

    const fn instance() -> Self {
        Self(ThreadContractError::InvalidItem)
    }

    const fn budget() -> Self {
        Self(ThreadContractError::ValidationBudgetExceeded)
    }

    const fn error(self) -> ThreadContractError {
        self.0
    }
}

fn validate_node(
    instance: &Value,
    schema: &Value,
    root_schema: &Value,
    state: &mut ValidationState,
    depth: usize,
) -> Result<(), ValidationFailure> {
    if depth > MAX_VALIDATION_DEPTH {
        return Err(ValidationFailure::budget());
    }
    state.work = state
        .work
        .checked_add(1)
        .ok_or_else(ValidationFailure::budget)?;
    if state.work > MAX_VALIDATION_WORK {
        return Err(ValidationFailure::budget());
    }

    let schema_object = match schema {
        Value::Bool(true) => return Ok(()),
        Value::Bool(false) => return Err(ValidationFailure::instance()),
        Value::Object(schema_object) => schema_object,
        _ => return Err(ValidationFailure::schema()),
    };

    if let Some(reference_value) = schema_object.get("$ref") {
        let reference = reference_value
            .as_str()
            .ok_or_else(ValidationFailure::schema)?;
        let target = resolve_local_reference(root_schema, reference)
            .ok_or_else(ValidationFailure::schema)?;
        return validate_node(instance, target, root_schema, state, depth + 1);
    }

    validate_schema_keyword_shapes(schema_object)?;

    if let Some(type_value) = schema_object.get("type") {
        if !matches_type(instance, type_value)? {
            return Err(ValidationFailure::instance());
        }
    }

    if let Some(enum_value) = schema_object.get("enum") {
        let values = enum_value
            .as_array()
            .ok_or_else(ValidationFailure::schema)?;
        if !values.iter().any(|candidate| candidate == instance) {
            return Err(ValidationFailure::instance());
        }
    }

    if let Some(object) = instance.as_object() {
        if let Some(required) = schema_object.get("required") {
            let required = required.as_array().ok_or_else(ValidationFailure::schema)?;
            for name in required {
                let name = name.as_str().ok_or_else(ValidationFailure::schema)?;
                if !object.contains_key(name) {
                    return Err(ValidationFailure::instance());
                }
            }
        }

        if let Some(properties) = schema_object.get("properties") {
            let properties = properties
                .as_object()
                .ok_or_else(ValidationFailure::schema)?;
            for (name, property_schema) in properties {
                if let Some(property) = object.get(name) {
                    validate_node(property, property_schema, root_schema, state, depth + 1)?;
                }
            }

            if let Some(additional) = schema_object.get("additionalProperties") {
                for (name, property) in object {
                    if properties.contains_key(name) {
                        continue;
                    }
                    match additional {
                        Value::Bool(true) => {}
                        Value::Bool(false) => return Err(ValidationFailure::instance()),
                        Value::Object(_) => {
                            validate_node(property, additional, root_schema, state, depth + 1)?;
                        }
                        _ => return Err(ValidationFailure::schema()),
                    }
                }
            }
        } else if let Some(additional) = schema_object.get("additionalProperties") {
            for property in object.values() {
                match additional {
                    Value::Bool(true) => {}
                    Value::Bool(false) => return Err(ValidationFailure::instance()),
                    Value::Object(_) => {
                        validate_node(property, additional, root_schema, state, depth + 1)?;
                    }
                    _ => return Err(ValidationFailure::schema()),
                }
            }
        }
    }

    if let Some(array) = instance.as_array() {
        if let Some(items) = schema_object.get("items") {
            match items {
                Value::Object(_) | Value::Bool(_) => {
                    for item in array {
                        validate_node(item, items, root_schema, state, depth + 1)?;
                    }
                }
                Value::Array(tuple_schemas) => {
                    for (index, item) in array.iter().enumerate() {
                        if let Some(item_schema) = tuple_schemas.get(index) {
                            validate_node(item, item_schema, root_schema, state, depth + 1)?;
                        }
                    }
                }
                _ => return Err(ValidationFailure::schema()),
            }
        }
    }

    if let Some(one_of) = schema_object.get("oneOf") {
        let branches = one_of.as_array().ok_or_else(ValidationFailure::schema)?;
        let mut matches = 0usize;
        for branch in branches {
            match validate_node(instance, branch, root_schema, state, depth + 1) {
                Ok(()) => matches += 1,
                Err(failure) if failure.error() == ThreadContractError::InvalidItem => {}
                Err(failure) => return Err(failure),
            }
        }
        if matches != 1 {
            return Err(ValidationFailure::instance());
        }
    }

    if let Some(any_of) = schema_object.get("anyOf") {
        let branches = any_of.as_array().ok_or_else(ValidationFailure::schema)?;
        let mut matches = 0usize;
        for branch in branches {
            match validate_node(instance, branch, root_schema, state, depth + 1) {
                Ok(()) => matches += 1,
                Err(failure) if failure.error() == ThreadContractError::InvalidItem => {}
                Err(failure) => return Err(failure),
            }
        }
        if matches == 0 {
            return Err(ValidationFailure::instance());
        }
    }

    if let Some(all_of) = schema_object.get("allOf") {
        let branches = all_of.as_array().ok_or_else(ValidationFailure::schema)?;
        for branch in branches {
            validate_node(instance, branch, root_schema, state, depth + 1)?;
        }
    }

    if let Some(minimum) = schema_object.get("minimum") {
        if instance.is_number() && !number_at_least(instance, minimum)? {
            return Err(ValidationFailure::instance());
        }
    }

    if let Some(min_length) = schema_object.get("minLength") {
        if let Some(string) = instance.as_str() {
            let minimum = min_length.as_u64().ok_or_else(ValidationFailure::schema)?;
            if u64::try_from(string.chars().count())
                .map(|length| length < minimum)
                .unwrap_or(false)
            {
                return Err(ValidationFailure::instance());
            }
        }
    }

    if let Some(format) = schema_object.get("format") {
        let format = format.as_str().ok_or_else(ValidationFailure::schema)?;
        if is_integer_instance(instance)
            && matches!(
                format,
                "int32" | "int64" | "uint" | "uint16" | "uint32" | "uint64"
            )
            && !matches_integer_format(instance, format)
        {
            return Err(ValidationFailure::instance());
        }
    }

    Ok(())
}

fn validate_schema_keyword_shapes(
    schema_object: &Map<String, Value>,
) -> Result<(), ValidationFailure> {
    if let Some(type_value) = schema_object.get("type") {
        match type_value {
            Value::String(type_name) => {
                if !is_known_type_name(type_name) {
                    return Err(ValidationFailure::schema());
                }
            }
            Value::Array(type_names) if !type_names.is_empty() => {
                for type_name in type_names {
                    let type_name = type_name.as_str().ok_or_else(ValidationFailure::schema)?;
                    if !is_known_type_name(type_name) {
                        return Err(ValidationFailure::schema());
                    }
                }
            }
            Value::Array(_) => return Err(ValidationFailure::schema()),
            _ => return Err(ValidationFailure::schema()),
        }
    }

    if let Some(enum_value) = schema_object.get("enum") {
        if !enum_value.is_array() {
            return Err(ValidationFailure::schema());
        }
    }

    if let Some(required) = schema_object.get("required") {
        let required = required.as_array().ok_or_else(ValidationFailure::schema)?;
        for name in required {
            if !name.is_string() {
                return Err(ValidationFailure::schema());
            }
        }
    }

    if let Some(properties) = schema_object.get("properties") {
        let properties = properties
            .as_object()
            .ok_or_else(ValidationFailure::schema)?;
        for property_schema in properties.values() {
            ensure_schema_value(property_schema)?;
        }
    }

    if let Some(additional) = schema_object.get("additionalProperties") {
        ensure_schema_value(additional)?;
    }

    if let Some(items) = schema_object.get("items") {
        match items {
            Value::Bool(_) | Value::Object(_) => {}
            Value::Array(tuple_schemas) => {
                for item_schema in tuple_schemas {
                    ensure_schema_value(item_schema)?;
                }
            }
            _ => return Err(ValidationFailure::schema()),
        }
    }

    for keyword in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = schema_object.get(keyword) {
            let branches = branches.as_array().ok_or_else(ValidationFailure::schema)?;
            for branch in branches {
                ensure_schema_value(branch)?;
            }
        }
    }

    if let Some(minimum) = schema_object.get("minimum") {
        if !minimum.is_number() {
            return Err(ValidationFailure::schema());
        }
    }

    if let Some(min_length) = schema_object.get("minLength") {
        if min_length.as_u64().is_none() {
            return Err(ValidationFailure::schema());
        }
    }

    if let Some(format) = schema_object.get("format") {
        if !format.is_string() {
            return Err(ValidationFailure::schema());
        }
    }

    Ok(())
}

fn ensure_schema_value(value: &Value) -> Result<(), ValidationFailure> {
    if matches!(value, Value::Bool(_) | Value::Object(_)) {
        Ok(())
    } else {
        Err(ValidationFailure::schema())
    }
}

fn resolve_local_reference<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    if reference == "#" {
        return Some(root);
    }
    let pointer = reference.strip_prefix("#/")?;
    let mut current = root;
    for encoded in pointer.split('/') {
        let token = encoded.replace("~1", "/").replace("~0", "~");
        current = current.get(token)?;
    }
    Some(current)
}

fn matches_type(instance: &Value, type_value: &Value) -> Result<bool, ValidationFailure> {
    match type_value {
        Value::String(type_name) => matches_single_type(instance, type_name),
        Value::Array(type_names) => {
            let mut matched = false;
            for type_name in type_names {
                let type_name = type_name.as_str().ok_or_else(ValidationFailure::schema)?;
                matched |= matches_single_type(instance, type_name)?;
            }
            Ok(matched)
        }
        _ => Err(ValidationFailure::schema()),
    }
}

fn is_known_type_name(type_name: &str) -> bool {
    matches!(
        type_name,
        "null" | "boolean" | "object" | "array" | "string" | "number" | "integer"
    )
}

fn matches_single_type(instance: &Value, type_name: &str) -> Result<bool, ValidationFailure> {
    match type_name {
        "null" => Ok(instance.is_null()),
        "boolean" => Ok(instance.is_boolean()),
        "object" => Ok(instance.is_object()),
        "array" => Ok(instance.is_array()),
        "string" => Ok(instance.is_string()),
        "number" => Ok(instance.is_number()),
        "integer" => Ok(is_integer_instance(instance)),
        _ => Err(ValidationFailure::schema()),
    }
}

fn is_integer_instance(instance: &Value) -> bool {
    instance.as_i64().is_some() || instance.as_u64().is_some()
}

fn matches_integer_format(instance: &Value, format: &str) -> bool {
    let Some(number) = instance.as_number() else {
        return false;
    };
    if !(number.is_i64() || number.is_u64()) {
        return false;
    }
    let signed = number.as_i64();
    let unsigned = number.as_u64();
    match format {
        "int32" => signed
            .map(|value| (-2_147_483_648..=2_147_483_647).contains(&value))
            .unwrap_or(false),
        "int64" => signed.is_some(),
        "uint" | "uint64" => unsigned.is_some(),
        "uint16" => unsigned
            .map(|value| value <= u16::MAX as u64)
            .unwrap_or(false),
        "uint32" => unsigned
            .map(|value| value <= u32::MAX as u64)
            .unwrap_or(false),
        _ => true,
    }
}

fn number_at_least(instance: &Value, minimum: &Value) -> Result<bool, ValidationFailure> {
    let minimum = minimum.as_f64().ok_or_else(ValidationFailure::schema)?;
    let number = instance
        .as_number()
        .ok_or_else(ValidationFailure::instance)?;
    if let Some(value) = number.as_i64() {
        return Ok((value as f64) >= minimum);
    }
    if let Some(value) = number.as_u64() {
        return Ok((value as f64) >= minimum);
    }
    number
        .as_f64()
        .map(|value| value >= minimum)
        .ok_or_else(ValidationFailure::instance)
}

/*
 * The validator above deliberately keeps the keyword-specific assertions
 * local.  This marker separates it from the immutable candidate contracts
 * below.
 */

/// Immutable, schema-first representation of a Thread item.  The complete
/// validated JSON is retained privately for exact deduplication in C2; only
/// bounded identity, timing, relation, and title fields are exposed here.
#[derive(Clone, Debug)]
pub struct ValidatedThreadCandidate {
    raw: Value,
    id: String,
    created_at: i64,
    updated_at: i64,
    path: Option<String>,
    title: String,
    active: bool,
    is_subagent: bool,
    parent_thread_id: Option<String>,
    depth: Option<i32>,
}

impl ValidatedThreadCandidate {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn updated_at(&self) -> i64 {
        self.updated_at
    }

    pub fn created_at(&self) -> i64 {
        self.created_at
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_subagent(&self) -> bool {
        self.is_subagent
    }

    pub fn parent_thread_id(&self) -> Option<&str> {
        self.parent_thread_id.as_deref()
    }

    pub fn depth(&self) -> Option<i32> {
        self.depth
    }

    pub fn raw_json(&self) -> &Value {
        &self.raw
    }

    pub fn thread(&self) -> &Value {
        &self.raw
    }

    pub fn derived_eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.created_at == other.created_at
            && self.updated_at == other.updated_at
            && self.path == other.path
            && self.title == other.title
            && self.active == other.active
            && self.is_subagent == other.is_subagent
            && self.parent_thread_id == other.parent_thread_id
            && self.depth == other.depth
    }

    pub fn exact_json_eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl PartialEq for ValidatedThreadCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
            && self.id == other.id
            && self.created_at == other.created_at
            && self.updated_at == other.updated_at
            && self.path == other.path
            && self.title == other.title
            && self.is_subagent == other.is_subagent
            && self.parent_thread_id == other.parent_thread_id
            && self.depth == other.depth
    }
}

impl Eq for ValidatedThreadCandidate {}

/// The result of accepting a page.  The accumulator does not follow the
/// cursor; it merely reports whether the caller would need another request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageAcceptance {
    NeedNextPage { cursor: String },
    Terminal,
}

impl PageAcceptance {
    pub fn next_cursor(&self) -> Option<&str> {
        match self {
            Self::NeedNextPage { cursor } => Some(cursor),
            Self::Terminal => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AccumulatorPhase {
    #[default]
    Collecting,
    Terminal,
    Failed,
}

/// Cycle-private page accumulator. Candidate data is exposed only by consuming
/// a successfully terminal accumulator through `ordered_candidates`.
#[derive(Clone, Debug, Default)]
pub struct ThreadCycleAccumulator {
    candidates: HashMap<String, ValidatedThreadCandidate>,
    seen_ids: HashSet<String>,
    rejected_ids: HashSet<String>,
    seen_cursors: HashSet<String>,
    page_count: usize,
    rejected_count: usize,
    phase: AccumulatorPhase,
}

impl ThreadCycleAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_terminal(&self) -> bool {
        self.phase == AccumulatorPhase::Terminal
    }

    pub fn is_failed(&self) -> bool {
        self.phase == AccumulatorPhase::Failed
    }

    pub fn accept_page(&mut self, page: &Value) -> Result<PageAcceptance, ThreadContractError> {
        if self.phase != AccumulatorPhase::Collecting {
            return Err(ThreadContractError::InvalidRequest);
        }

        let mut prospective = self.clone();
        match prospective.accept_page_transaction(page) {
            Ok(acceptance) => {
                *self = prospective;
                Ok(acceptance)
            }
            Err(error) => {
                self.phase = AccumulatorPhase::Failed;
                Err(error)
            }
        }
    }

    fn accept_page_transaction(
        &mut self,
        page: &Value,
    ) -> Result<PageAcceptance, ThreadContractError> {
        if self.page_count >= MAX_THREAD_PAGES {
            return Err(ThreadContractError::ValidationBudgetExceeded);
        }

        let object = page
            .as_object()
            .ok_or(ThreadContractError::InvalidEnvelope)?;
        let data = object
            .get("data")
            .ok_or(ThreadContractError::InvalidEnvelope)?;
        let data_array = data
            .as_array()
            .ok_or(ThreadContractError::InvalidEnvelope)?;

        // Replace data only after checking that it is an array.  This keeps
        // item failures independent while preserving envelope ownership for
        // root/data/cursor failures.
        let mut envelope = page.clone();
        if let Some(envelope_object) = envelope.as_object_mut() {
            envelope_object.insert("data".to_owned(), Value::Array(Vec::new()));
        } else {
            return Err(ThreadContractError::InvalidEnvelope);
        }
        let schema = schema_document().map_err(|_| ThreadContractError::InvalidEnvelope)?;
        if validate_instance_against_schema(&envelope, schema).is_err() {
            return Err(ThreadContractError::InvalidEnvelope);
        }

        let next_cursor = match object.get("nextCursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(cursor)) => {
                validate_cursor(cursor).map_err(|_| ThreadContractError::InvalidEnvelope)?;
                Some(cursor.clone())
            }
            Some(_) => return Err(ThreadContractError::InvalidEnvelope),
        };

        if let Some(cursor) = &next_cursor {
            if self.page_count + 1 >= MAX_THREAD_PAGES {
                return Err(ThreadContractError::ValidationBudgetExceeded);
            }
            if !self.seen_cursors.insert(cursor.clone()) {
                return Err(ThreadContractError::InvalidEnvelope);
            }
        }

        for item in data_array {
            match validate_thread_item(item) {
                Ok(candidate) => self.accept_candidate(candidate)?,
                Err(ThreadContractError::InvalidItem) => {
                    self.rejected_count = self
                        .rejected_count
                        .checked_add(1)
                        .ok_or(ThreadContractError::ValidationBudgetExceeded)?;
                }
                Err(error) => return Err(error),
            }
        }

        self.page_count = self
            .page_count
            .checked_add(1)
            .ok_or(ThreadContractError::ValidationBudgetExceeded)?;
        let acceptance = match next_cursor {
            Some(cursor) => PageAcceptance::NeedNextPage { cursor },
            None => {
                self.phase = AccumulatorPhase::Terminal;
                PageAcceptance::Terminal
            }
        };
        Ok(acceptance)
    }

    fn accept_candidate(
        &mut self,
        candidate: ValidatedThreadCandidate,
    ) -> Result<(), ThreadContractError> {
        let id = candidate.id.clone();
        if !self.seen_ids.contains(&id) {
            if self.seen_ids.len() >= MAX_UNIQUE_THREAD_IDS {
                return Err(ThreadContractError::ValidationBudgetExceeded);
            }
            self.seen_ids.insert(id.clone());
            self.candidates.insert(id, candidate);
            return Ok(());
        }

        if self.rejected_ids.contains(&id) {
            self.rejected_count = self
                .rejected_count
                .checked_add(1)
                .ok_or(ThreadContractError::ValidationBudgetExceeded)?;
            return Ok(());
        }

        let existing = self
            .candidates
            .get(&id)
            .ok_or(ThreadContractError::InvalidItem)?;
        if candidate.updated_at > existing.updated_at {
            self.candidates.insert(id, candidate);
        } else if candidate.updated_at == existing.updated_at && candidate != *existing {
            self.candidates.remove(&id);
            self.rejected_ids.insert(id);
            self.rejected_count = self
                .rejected_count
                .checked_add(1)
                .ok_or(ThreadContractError::ValidationBudgetExceeded)?;
        }
        Ok(())
    }

    /// Consume a completed cycle and return its unique candidates in the only
    /// selection order allowed by the contract.
    pub fn ordered_candidates(self) -> Result<Vec<ValidatedThreadCandidate>, ThreadContractError> {
        if self.phase != AccumulatorPhase::Terminal {
            return Err(ThreadContractError::InvalidRequest);
        }
        let mut candidates: Vec<_> = self.candidates.into_values().collect();
        candidates.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(candidates)
    }

    /// Explicitly abandon the private cycle.  Consuming `self` makes reuse or
    /// publication after an adapter-level abort impossible.
    pub fn abort(self) {}

    #[cfg(test)]
    fn debug_candidates(&self) -> Vec<&ValidatedThreadCandidate> {
        let mut values: Vec<_> = self.candidates.values().collect();
        values.sort_by(|left, right| left.id.cmp(&right.id));
        values
    }
}

/// Validate a complete vendored `#/definitions/Thread` first, then apply the
/// small semantic boundary needed by candidate selection.
pub fn validate_thread_item(item: &Value) -> Result<ValidatedThreadCandidate, ThreadContractError> {
    let schema = schema_document()?;
    let definitions = schema
        .get("definitions")
        .and_then(Value::as_object)
        .ok_or(ThreadContractError::InvalidSchema)?;
    let thread_schema = definitions
        .get("Thread")
        .ok_or(ThreadContractError::InvalidSchema)?;
    validate_instance_with_root(item, thread_schema, schema)?;

    let object = item.as_object().ok_or(ThreadContractError::InvalidItem)?;
    let updated_at = object
        .get("updatedAt")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or(ThreadContractError::InvalidItem)?;
    let created_at = object
        .get("createdAt")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or(ThreadContractError::InvalidItem)?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| {
            let length = value.chars().count();
            (1..=128).contains(&length)
        })
        .ok_or(ThreadContractError::InvalidItem)?
        .to_owned();
    let cwd = object
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && Path::new(value).is_absolute())
        .ok_or(ThreadContractError::InvalidItem)?;
    let _ = cwd;

    let path = match object.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => return Err(ThreadContractError::InvalidItem),
    };

    let name = object.get("name").and_then(Value::as_str);
    let preview = object
        .get("preview")
        .and_then(Value::as_str)
        .ok_or(ThreadContractError::InvalidItem)?;
    let normalized_name = name
        .map(security::bounded_thread_title)
        .transpose()
        .map_err(|_| ThreadContractError::InvalidItem)?
        .unwrap_or_default();
    let normalized_preview =
        security::bounded_thread_title(preview).map_err(|_| ThreadContractError::InvalidItem)?;
    let title = if !normalized_name.is_empty() {
        normalized_name
    } else if !normalized_preview.is_empty() {
        normalized_preview
    } else {
        "アクティブなスレッド".to_owned()
    };
    let active = object
        .get("status")
        .and_then(Value::as_object)
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
        == Some("active");
    let (is_subagent, parent_thread_id, depth) = thread_relation(object);

    Ok(ValidatedThreadCandidate {
        raw: item.clone(),
        id,
        created_at,
        updated_at,
        path,
        title,
        active,
        is_subagent,
        parent_thread_id,
        depth,
    })
}

fn thread_relation(object: &Map<String, Value>) -> (bool, Option<String>, Option<i32>) {
    let Some(subagent) = object
        .get("source")
        .and_then(Value::as_object)
        .and_then(|source| source.get("subAgent"))
    else {
        return (false, None, None);
    };
    let Some(spawn) = subagent
        .as_object()
        .and_then(|kind| kind.get("thread_spawn"))
        .and_then(Value::as_object)
    else {
        return (true, None, None);
    };
    let parent_thread_id = spawn
        .get("parent_thread_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let depth = spawn
        .get("depth")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    (true, parent_thread_id, depth)
}

/// A read-only snapshot of the pinned identity fields.  Hash strings are
/// obtained from the compile-time manifest, never discovered from the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedSchemaIdentity {
    pub manifest_id: String,
    pub cli_version: String,
    pub utc_date: String,
    pub bundle_raw_sha256: String,
    pub params_raw_sha256: String,
    pub params_canonical_sha256: String,
    pub response_raw_sha256: String,
    pub response_canonical_sha256: String,
}

pub fn schema_manifest_json() -> Result<&'static Value, ThreadContractError> {
    static MANIFEST: OnceLock<Result<Value, ThreadContractError>> = OnceLock::new();
    match MANIFEST.get_or_init(|| {
        serde_json::from_str(MANIFEST_TEXT).map_err(|_| ThreadContractError::InvalidManifest)
    }) {
        Ok(manifest) => Ok(manifest),
        Err(error) => Err(*error),
    }
}

pub fn pinned_schema_identity() -> Result<PinnedSchemaIdentity, ThreadContractError> {
    let manifest = schema_manifest_json()?;
    let manifest_object = manifest
        .as_object()
        .ok_or(ThreadContractError::InvalidManifest)?;
    if manifest_object.get("schema") != Some(&json!("CODEX_INFO_THREAD_SCHEMA_MANIFEST_V1"))
        || manifest_object.get("codex_cli_version") != Some(&json!("0.147.0"))
        || manifest_object.get("generated_utc_date") != Some(&json!("2026-08-14"))
        || manifest_object.get("generation_command")
            != Some(&json!([
                "codex",
                "app-server",
                "generate-json-schema",
                "--out",
                "<empty-output-directory>"
            ]))
        || manifest_object.get("experimental") != Some(&json!(false))
        || manifest_object.get("bundle")
            != Some(&json!({
                "generated_path": "codex_app_server_protocol.v2.schemas.json",
                "raw_sha256": "f3dec1e031d99a420b137b903f02196d4325eece57620c925bb7130b25f168d2"
            }))
        || manifest_object.get("artifacts")
            != Some(&json!([
                {
                    "id": "ThreadListParams",
                    "path": "protocol/thread/ThreadListParams.canonical.json",
                    "generated_path": "v2/ThreadListParams.json",
                    "generated_raw_sha256": "b227bb78acf9b91060d03c56d3f2072cdd9f1bd08290c11e8869f1a663b16da2",
                    "canonical_jq_cS_sha256": "6a63582e96c9092edcdc19935484cadcd72a1ae128762f6d666fc2017596d310",
                    "vendored_sha256": "6a63582e96c9092edcdc19935484cadcd72a1ae128762f6d666fc2017596d310"
                },
                {
                    "id": "ThreadListResponse",
                    "path": "protocol/thread/ThreadListResponse.canonical.json",
                    "generated_path": "v2/ThreadListResponse.json",
                    "generated_raw_sha256": "08d5ffb0a799cf0d1c42c11b12c8bc4b04d6e515f96c6789bbec532eba1b2612",
                    "canonical_jq_cS_sha256": "f3d94a229732a0756eb8c6698e325c05a1105fb2b6fe668814b8e3277a21f130",
                    "vendored_sha256": "f3d94a229732a0756eb8c6698e325c05a1105fb2b6fe668814b8e3277a21f130"
                }
            ]))
        || manifest_object.get("source_kinds")
            != Some(&json!([
                "cli",
                "vscode",
                "exec",
                "appServer",
                "subAgent",
                "subAgentReview",
                "subAgentCompact",
                "subAgentThreadSpawn",
                "subAgentOther",
                "unknown"
            ]))
    {
        return Err(ThreadContractError::InvalidManifest);
    }
    Ok(PinnedSchemaIdentity {
        manifest_id: THREAD_SCHEMA_MANIFEST_ID.to_owned(),
        cli_version: THREAD_SCHEMA_CLI_VERSION.to_owned(),
        utc_date: THREAD_SCHEMA_UTC_DATE.to_owned(),
        bundle_raw_sha256: THREAD_SCHEMA_BUNDLE_RAW_SHA256.to_owned(),
        params_raw_sha256: THREAD_SCHEMA_PARAMS_RAW_SHA256.to_owned(),
        params_canonical_sha256: THREAD_SCHEMA_PARAMS_CANONICAL_SHA256.to_owned(),
        response_raw_sha256: THREAD_SCHEMA_RESPONSE_RAW_SHA256.to_owned(),
        response_canonical_sha256: THREAD_SCHEMA_RESPONSE_CANONICAL_SHA256.to_owned(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RolloutError {
    FileTooLarge,
    InvalidUtf8,
    UnterminatedLine,
    LineTooLarge,
    InvalidJson,
    InvalidKnownEvent,
}

impl RolloutError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::FileTooLarge => "thread rollout exceeds file limit",
            Self::InvalidUtf8 => "thread rollout is not UTF-8",
            Self::UnterminatedLine => "thread rollout has an unterminated line",
            Self::LineTooLarge => "thread rollout line exceeds limit",
            Self::InvalidJson => "thread rollout JSON is invalid",
            Self::InvalidKnownEvent => "thread rollout known event is invalid",
        }
    }
}

impl fmt::Display for RolloutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for RolloutError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedRollout {
    running: bool,
    model: String,
    model_label: String,
    total_tokens: Option<u64>,
    context_usage_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    last_user_message_at: Option<i64>,
}

impl ValidatedRollout {
    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn with_running_override(mut self, running: bool) -> Self {
        self.running = running;
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn model_label(&self) -> &str {
        &self.model_label
    }

    pub fn total_tokens(&self) -> Option<u64> {
        self.total_tokens
    }

    pub fn context_usage_tokens(&self) -> Option<u64> {
        self.context_usage_tokens
    }

    pub fn context_window_tokens(&self) -> Option<u64> {
        self.context_window_tokens
    }

    pub fn last_user_message_at(&self) -> Option<i64> {
        self.last_user_message_at
    }
}

#[derive(Clone, Debug, Default)]
struct RolloutState {
    last_task_running: Option<bool>,
    last_model: Option<String>,
    last_total_tokens: Option<u64>,
    last_context_usage_tokens: Option<u64>,
    last_context_window_tokens: Option<u64>,
    last_user_message_at: Option<i64>,
}

/// Stateful parser used by the live rollout reader.
///
/// Rollout files are append-only.  The first cycle applies the complete
/// bounded prefix and later cycles apply only complete records appended after
/// the cached offset.  Keeping this state here makes the incremental path use
/// exactly the same known-event validation as the full reader.
#[derive(Clone, Debug, Default)]
pub struct RolloutAccumulator {
    state: RolloutState,
}

impl RolloutAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seeded(
        last_model: Option<String>,
        previous_total: u64,
        last_task_running: Option<bool>,
    ) -> Self {
        Self {
            state: RolloutState {
                last_task_running,
                last_model,
                last_total_tokens: Some(previous_total),
                ..RolloutState::default()
            },
        }
    }

    /// Apply complete records from `reader` to this accumulator.
    ///
    /// The caller supplies the bounded byte count for this chunk.  A complete
    /// chunk is expected in normal operation; an unterminated record remains a
    /// hard error so the caller cannot publish a partial known event.
    pub fn apply_reader<R: BufRead>(
        &mut self,
        reader: &mut R,
        snapshot_bytes: u64,
    ) -> Result<(), RolloutError> {
        if snapshot_bytes > security::MAX_SESSION_FILE_BYTES {
            return Err(RolloutError::FileTooLarge);
        }
        loop {
            let record = match security::read_bounded_jsonl_record(reader) {
                Ok(record) => record,
                Err(error)
                    if matches!(
                        error.kind(),
                        security::SecurityErrorKind::LimitExceeded
                            | security::SecurityErrorKind::Parse
                    ) =>
                {
                    // Tool output and malformed token-count/response-item
                    // records are non-liveness data.  The bounded reader has
                    // consumed the complete bad record before returning, so
                    // skipping it preserves the existing live-reader
                    // recovery contract without weakening lifecycle/model
                    // validation below.
                    continue;
                }
                Err(error) => {
                    return Err(match error.kind() {
                        security::SecurityErrorKind::LimitExceeded => RolloutError::LineTooLarge,
                        security::SecurityErrorKind::Parse => RolloutError::InvalidUtf8,
                        security::SecurityErrorKind::Unterminated => RolloutError::UnterminatedLine,
                        _ => RolloutError::InvalidJson,
                    });
                }
            };
            let Some((line, terminated)) = record else {
                break;
            };
            if !terminated {
                return Err(RolloutError::UnterminatedLine);
            }
            let value: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_)
                    if is_malformed_token_count_record(&line)
                        || is_malformed_response_item_record(&line) =>
                {
                    continue;
                }
                Err(_) => return Err(RolloutError::InvalidJson),
            };
            apply_rollout_event(&value, &mut self.state)?;
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<ValidatedRollout, RolloutError> {
        finish_rollout(self.state.clone())
    }
}

fn finish_rollout(state: RolloutState) -> Result<ValidatedRollout, RolloutError> {
    let model = state
        .last_model
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "不明".to_owned());
    let model_label =
        security::bounded_model_label(&model).map_err(|_| RolloutError::InvalidKnownEvent)?;
    Ok(ValidatedRollout {
        running: state.last_task_running == Some(true),
        model,
        model_label,
        total_tokens: state.last_total_tokens,
        context_usage_tokens: state.last_context_usage_tokens,
        context_window_tokens: state.last_context_window_tokens,
        last_user_message_at: state.last_user_message_at,
    })
}

fn event_timestamp(value: &Value) -> Option<i64> {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.timestamp())
        .filter(|timestamp| *timestamp > 0)
}

/// Validate a bounded JSONL stream before returning any task, model, or token
/// field. Non-empty streams must end in a newline. The caller supplies the
/// secure-open snapshot length so the file-size bound is checked before any
/// record is accepted.
pub fn parse_rollout_reader<R: BufRead>(
    reader: &mut R,
    snapshot_bytes: u64,
) -> Result<ValidatedRollout, RolloutError> {
    if snapshot_bytes > security::MAX_SESSION_FILE_BYTES {
        return Err(RolloutError::FileTooLarge);
    }
    let mut state = RolloutState::default();
    loop {
        let record =
            security::read_bounded_jsonl_record(reader).map_err(|error| match error.kind() {
                security::SecurityErrorKind::LimitExceeded => RolloutError::LineTooLarge,
                security::SecurityErrorKind::Parse => RolloutError::InvalidUtf8,
                security::SecurityErrorKind::Unterminated => RolloutError::UnterminatedLine,
                _ => RolloutError::InvalidJson,
            })?;
        let Some((line, terminated)) = record else {
            break;
        };
        if !terminated {
            return Err(RolloutError::UnterminatedLine);
        }
        let value: Value = serde_json::from_str(&line).map_err(|_| RolloutError::InvalidJson)?;
        apply_rollout_event(&value, &mut state)?;
    }
    finish_rollout(state)
}

/// Parse a live rollout while isolating oversized or invalid-UTF8 records.
///
/// Tool output can legitimately exceed the bounded record size.  Those
/// records are not needed to determine the running/model/token state, and the
/// bounded reader consumes each complete bad line before returning its error.
/// A malformed `token_count` envelope is likewise non-liveness data: some
/// live writers can split a large rate-limit payload at a physical newline.
/// Skip only that explicitly identified event family; lifecycle and model
/// records remain strict so an invalid state cannot be presented as running.
pub fn parse_rollout_reader_recoverable<R: BufRead>(
    reader: &mut R,
    snapshot_bytes: u64,
) -> Result<ValidatedRollout, RolloutError> {
    let mut accumulator = RolloutAccumulator::new();
    accumulator.apply_reader(reader, snapshot_bytes)?;
    accumulator.snapshot()
}

fn is_malformed_token_count_record(line: &str) -> bool {
    let Some(payload_start) = line.find("\"payload\"") else {
        return false;
    };
    // Require the root event family before the payload and the token-count
    // family inside the payload. This keeps whitespace variants recoverable
    // while preventing malformed lifecycle/model records that merely mention
    // token_count from being skipped.
    contains_string_field(&line[..payload_start], "type", "event_msg")
        && contains_string_field(&line[payload_start..], "type", "token_count")
}

fn is_malformed_response_item_record(line: &str) -> bool {
    let Some(payload_start) = line.find("\"payload\"") else {
        return false;
    };
    // response_item records carry transcript content, not task lifecycle,
    // model, or token state. An interrupted append must not invalidate later
    // complete state events in the same live rollout.
    contains_string_field(&line[..payload_start], "type", "response_item")
}

fn contains_string_field(input: &str, key: &str, expected: &str) -> bool {
    let marker = format!("\"{key}\"");
    let expected_marker = format!("\"{expected}\"");
    let mut offset = 0;
    while let Some(key_start) = find_json_key(input, &marker, offset) {
        let start = key_start + marker.len();
        let bytes = input.as_bytes();
        let mut cursor = start;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b':') {
            offset = key_start + 1;
            continue;
        }
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if input[cursor..].starts_with(&expected_marker) {
            return true;
        }
        offset = key_start + 1;
    }
    false
}

/// Find a key token outside quoted JSON strings. The surrounding record may
/// be truncated, so this intentionally performs only bounded lexical state
/// tracking rather than attempting to deserialize an incomplete object.
fn find_json_key(input: &str, marker: &str, from: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut in_string = false;
    let mut escaped = false;
    for index in from..bytes.len() {
        let byte = bytes[index];
        if byte == b'"' && !escaped {
            if !in_string && input[index..].starts_with(marker) {
                return Some(index);
            }
            in_string = !in_string;
        }
        escaped = in_string && byte == b'\\' && !escaped;
        if byte != b'\\' {
            escaped = false;
        }
    }
    None
}

/// Validate an in-memory rollout with the same streaming parser used by live
/// files. Keeping one parser prevents fixture and production semantics from
/// drifting.
pub fn parse_rollout(bytes: &[u8]) -> Result<ValidatedRollout, RolloutError> {
    let snapshot_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    parse_rollout_reader(&mut Cursor::new(bytes), snapshot_bytes)
}

fn apply_rollout_event(value: &Value, state: &mut RolloutState) -> Result<(), RolloutError> {
    let Some(root) = value.as_object() else {
        return Ok(());
    };
    let Some(root_type) = root.get("type").and_then(Value::as_str) else {
        return Ok(());
    };

    if root_type == "event_msg" {
        let payload = root
            .get("payload")
            .and_then(Value::as_object)
            .ok_or(RolloutError::InvalidKnownEvent)?;
        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .ok_or(RolloutError::InvalidKnownEvent)?;
        if event_type == "user_message" {
            if let Some(timestamp) = event_timestamp(value) {
                state.last_user_message_at = Some(timestamp);
            }
        }
        return apply_known_rollout_event(event_type, payload, true, state);
    }

    if root_type == "response_item"
        && value
            .get("payload")
            .and_then(Value::as_object)
            .is_some_and(|payload| {
                payload.get("type").and_then(Value::as_str) == Some("message")
                    && payload.get("role").and_then(Value::as_str) == Some("user")
            })
    {
        if let Some(timestamp) = event_timestamp(value) {
            state.last_user_message_at = Some(timestamp);
        }
        return Ok(());
    }

    let event = match root.get("payload") {
        Some(Value::Object(payload)) => payload,
        Some(_) if is_known_rollout_event(root_type) => {
            return Err(RolloutError::InvalidKnownEvent)
        }
        _ => root,
    };
    apply_known_rollout_event(root_type, event, false, state)
}

fn is_known_rollout_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "task_started"
            | "task_complete"
            | "task_completed"
            | "turn_aborted"
            | "turn_context"
            | "thread_context"
            | "thread_settings_applied"
            | "token_count"
    )
}

fn apply_known_rollout_event(
    event_type: &str,
    event: &Map<String, Value>,
    nested: bool,
    state: &mut RolloutState,
) -> Result<(), RolloutError> {
    if let Some(running) = task_running_for_event_type(event_type) {
        state.last_task_running = Some(running);
        return Ok(());
    }
    match event_type {
        "turn_context" | "thread_context" | "thread_settings_applied" => {
            state.last_model = Some(extract_known_model(event_type, event)?);
        }
        "token_count" if nested => {
            // Codex emits an initial token_count envelope with `info: null`
            // while the first usage sample is not available yet.  It carries
            // no token fields to trust, so treat it as a no-op; once `info`
            // is an object, keep strict validation for every numeric field.
            let info = event.get("info").ok_or(RolloutError::InvalidKnownEvent)?;
            let Some(info) = info.as_object() else {
                if info.is_null() {
                    return Ok(());
                }
                return Err(RolloutError::InvalidKnownEvent);
            };
            let total_tokens = info
                .get("total_token_usage")
                .and_then(Value::as_object)
                .and_then(|usage| usage.get("total_tokens"))
                .and_then(Value::as_u64)
                .ok_or(RolloutError::InvalidKnownEvent)?;
            state.last_total_tokens = Some(total_tokens);
            state.last_context_usage_tokens = match info.get("last_token_usage") {
                Some(last_usage) => Some(
                    last_usage
                        .as_object()
                        .and_then(|usage| usage.get("total_tokens"))
                        .and_then(Value::as_u64)
                        .ok_or(RolloutError::InvalidKnownEvent)?,
                ),
                None => None,
            };
            if let Some(context_window) = info.get("model_context_window") {
                state.last_context_window_tokens = Some(
                    context_window
                        .as_u64()
                        .ok_or(RolloutError::InvalidKnownEvent)?,
                );
            } else {
                state.last_context_window_tokens = None;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Canonical task-lifecycle projection shared by the rollout reader and the
/// durable session checkpoint writer.
pub fn task_running_for_event_type(event_type: &str) -> Option<bool> {
    match event_type {
        "task_started" => Some(true),
        "task_complete" | "task_completed" | "turn_aborted" => Some(false),
        _ => None,
    }
}

fn extract_known_model(
    event_type: &str,
    event: &Map<String, Value>,
) -> Result<String, RolloutError> {
    let model_value = if event_type == "thread_settings_applied" {
        match event.get("thread_settings") {
            Some(settings) => settings
                .as_object()
                .and_then(|settings| settings.get("model"))
                .ok_or(RolloutError::InvalidKnownEvent)?,
            None => event.get("model").ok_or(RolloutError::InvalidKnownEvent)?,
        }
    } else {
        event.get("model").ok_or(RolloutError::InvalidKnownEvent)?
    };
    let model = model_value
        .as_str()
        .ok_or(RolloutError::InvalidKnownEvent)?;
    security::bounded_model(model).map_err(|_| RolloutError::InvalidKnownEvent)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveThreadSnapshot {
    pub thread_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub title: String,
    pub model: String,
    pub model_label: String,
    pub total_tokens: Option<u64>,
    pub context_usage_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub last_user_message_at: Option<i64>,
    pub is_subagent: bool,
    pub parent_thread_id: Option<String>,
    pub depth: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadCycleOutcome {
    Snapshots(Vec<ActiveThreadSnapshot>),
    NoThread,
    CycleError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadTopologyNode<'a> {
    pub id: &'a str,
    pub parent_thread_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadTopologyError {
    Cycle,
}

/// Validate only parent topology for an already schema-validated, unique
/// thread slice. Missing parents are valid orphan edges. The caller owns
/// capacity and duplicate validation.
pub fn validate_selected_thread_topology(
    nodes: &[ThreadTopologyNode<'_>],
) -> Result<(), ThreadTopologyError> {
    let mut indices = HashMap::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        debug_assert!(
            indices.insert(node.id, index).is_none(),
            "topology input must already have unique IDs"
        );
    }

    let parent_indices = nodes
        .iter()
        .map(|node| {
            node.parent_thread_id
                .and_then(|parent_id| indices.get(parent_id).copied())
        })
        .collect::<Vec<_>>();
    let mut colors = vec![0u8; nodes.len()];

    for start in 0..nodes.len() {
        if colors[start] == 2 {
            continue;
        }
        let mut path = Vec::new();
        let mut current = Some(start);
        while let Some(index) = current {
            match colors[index] {
                0 => {
                    colors[index] = 1;
                    path.push(index);
                    current = parent_indices[index];
                }
                1 => return Err(ThreadTopologyError::Cycle),
                2 => break,
                _ => unreachable!("topology color has only three states"),
            }
        }
        for index in path {
            colors[index] = 2;
        }
    }
    Ok(())
}

/// Consume a terminal page cycle and collect every running candidate.
/// The reader adapter owns secure-open and metadata checks; any adapter or
/// rollout failure rejects that candidate without publishing partial fields.
pub fn select_active_threads<F, E>(
    accumulator: ThreadCycleAccumulator,
    read_rollout: F,
) -> ThreadCycleOutcome
where
    F: FnMut(&ValidatedThreadCandidate) -> Result<Vec<u8>, E>,
{
    select_active_threads_where(accumulator, |_| true, read_rollout)
}

/// As [`select_active_threads`], but candidates outside the current-process
/// set are ignored rather than treated as broken rollout files.
pub fn select_active_threads_where<P, F, E>(
    accumulator: ThreadCycleAccumulator,
    is_current: P,
    mut read_rollout: F,
) -> ThreadCycleOutcome
where
    P: FnMut(&ValidatedThreadCandidate) -> bool,
    F: FnMut(&ValidatedThreadCandidate) -> Result<Vec<u8>, E>,
{
    select_active_threads_parsed_where(accumulator, is_current, |candidate| {
        let bytes = read_rollout(candidate).map_err(|_| ())?;
        parse_rollout(&bytes).map_err(|_| ())
    })
}

/// Select current threads when secure filesystem code has already parsed each
/// rollout as a bounded stream. The complete terminal cycle is the atomic
/// publication unit: any admitted candidate failure rejects the whole cycle
/// instead of exposing a partial snapshot.
pub fn select_active_threads_parsed_where<P, F, E>(
    accumulator: ThreadCycleAccumulator,
    mut is_current: P,
    mut read_rollout: F,
) -> ThreadCycleOutcome
where
    P: FnMut(&ValidatedThreadCandidate) -> bool,
    F: FnMut(&ValidatedThreadCandidate) -> Result<ValidatedRollout, E>,
{
    let mut saw_candidate_failure = accumulator.rejected_count > 0;
    let candidates = match accumulator.ordered_candidates() {
        Ok(candidates) => candidates,
        Err(_) => return ThreadCycleOutcome::CycleError,
    };

    let mut snapshots = Vec::new();
    for candidate in candidates {
        if !is_current(&candidate) {
            continue;
        }
        if candidate.path().is_none() {
            saw_candidate_failure = true;
            continue;
        }
        let rollout = match read_rollout(&candidate) {
            Ok(rollout) => rollout,
            Err(_) => {
                saw_candidate_failure = true;
                continue;
            }
        };
        // The schema-validated thread/read status is the live app-server
        // authority. A restart can leave the durable rollout prefix without
        // a task_started event, so an explicitly active candidate promotes
        // the parsed rollout while preserving the existing rollout signal
        // for older app-server status variants.
        let running = rollout.is_running() || candidate.is_active();
        let rollout = rollout.with_running_override(running);
        if !rollout.is_running() {
            continue;
        }
        snapshots.push(ActiveThreadSnapshot {
            thread_id: candidate.id().to_owned(),
            created_at: candidate.created_at(),
            updated_at: candidate.updated_at(),
            title: candidate.title().to_owned(),
            model: rollout.model().to_owned(),
            model_label: rollout.model_label().to_owned(),
            total_tokens: rollout.total_tokens(),
            context_usage_tokens: rollout.context_usage_tokens(),
            context_window_tokens: rollout.context_window_tokens(),
            last_user_message_at: rollout.last_user_message_at(),
            is_subagent: candidate.is_subagent(),
            parent_thread_id: candidate.parent_thread_id().map(ToOwned::to_owned),
            depth: candidate.depth(),
        });
    }

    // A cycle is the unit of publication.  If any candidate that was
    // admitted to the cycle cannot be parsed/read, publishing the other
    // candidates would create a partial snapshot and hide data loss.  Keep
    // the previous complete snapshot at the caller instead.
    if saw_candidate_failure {
        ThreadCycleOutcome::CycleError
    } else if !snapshots.is_empty() {
        ThreadCycleOutcome::Snapshots(snapshots)
    } else {
        ThreadCycleOutcome::NoThread
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn full_thread() -> Value {
        json!({
            "cliVersion": "0.147.0",
            "createdAt": 1,
            "cwd": "/tmp/codex",
            "ephemeral": false,
            "id": "thread-1",
            "modelProvider": "openai",
            "preview": "preview",
            "sessionId": "session-1",
            "source": "cli",
            "status": {"type": "idle"},
            "turns": [],
            "updatedAt": 1,
            "name": "name",
            "path": "/tmp/codex/thread.json"
        })
    }

    fn thread_fixture(id: &str, updated_at: i64, name: &str) -> Value {
        let mut thread = full_thread();
        thread["id"] = json!(id);
        thread["sessionId"] = json!(format!("session-{id}"));
        thread["updatedAt"] = json!(updated_at);
        thread["name"] = json!(name);
        thread["path"] = json!(format!("/tmp/codex/{id}.jsonl"));
        thread
    }

    fn first_page_request_literal() -> Value {
        json!({
            "archived": false,
            "limit": 100,
            "sortKey": "updated_at",
            "sortDirection": "desc",
            "sourceKinds": [
                "cli",
                "vscode",
                "exec",
                "appServer",
                "subAgent",
                "subAgentReview",
                "subAgentCompact",
                "subAgentThreadSpawn",
                "subAgentOther",
                "unknown"
            ],
            "useStateDbOnly": false
        })
    }

    fn page(data: Value, next: Option<Value>) -> Value {
        let mut object = Map::new();
        object.insert("data".to_owned(), data);
        if let Some(next) = next {
            object.insert("nextCursor".to_owned(), next);
        }
        Value::Object(object)
    }

    fn page_with_backwards(data: Value, next: Option<Value>, backwards: Option<Value>) -> Value {
        let mut object = Map::new();
        object.insert("data".to_owned(), data);
        if let Some(next) = next {
            object.insert("nextCursor".to_owned(), next);
        }
        if let Some(backwards) = backwards {
            object.insert("backwardsCursor".to_owned(), backwards);
        }
        Value::Object(object)
    }

    fn rollout_bytes(events: &[Value]) -> Vec<u8> {
        if events.is_empty() {
            return Vec::new();
        }
        let mut text = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        text.push('\n');
        text.into_bytes()
    }

    fn rollout_raw_lines(lines: &[&str]) -> Vec<u8> {
        if lines.is_empty() {
            return Vec::new();
        }
        let mut text = lines.join("\n");
        text.push('\n');
        text.into_bytes()
    }

    fn terminal_cycle(rows: Vec<Value>) -> ThreadCycleAccumulator {
        let mut accumulator = ThreadCycleAccumulator::new();
        assert_eq!(
            accumulator.accept_page(&page(Value::Array(rows), None)),
            Ok(PageAcceptance::Terminal)
        );
        accumulator
    }

    fn assert_invalid_envelope(accumulator: &mut ThreadCycleAccumulator, envelope: Value) {
        assert!(matches!(
            accumulator.accept_page(&envelope),
            Err(ThreadContractError::InvalidEnvelope)
        ));
    }

    #[test]
    fn thread_c_schema_manifest_matches_pinned_cli_0147() {
        let manifest: Value = serde_json::from_str(include_str!(
            "../protocol/thread/THREAD_SCHEMA_MANIFEST.json"
        ))
        .expect("manifest JSON");
        let expected_manifest = json!({
            "schema": "CODEX_INFO_THREAD_SCHEMA_MANIFEST_V1",
            "codex_cli_version": "0.147.0",
            "generated_utc_date": "2026-08-14",
            "generation_command": [
                "codex",
                "app-server",
                "generate-json-schema",
                "--out",
                "<empty-output-directory>"
            ],
            "experimental": false,
            "bundle": {
                "generated_path": "codex_app_server_protocol.v2.schemas.json",
                "raw_sha256": "f3dec1e031d99a420b137b903f02196d4325eece57620c925bb7130b25f168d2"
            },
            "artifacts": [
                {
                    "id": "ThreadListParams",
                    "path": "protocol/thread/ThreadListParams.canonical.json",
                    "generated_path": "v2/ThreadListParams.json",
                    "generated_raw_sha256": "b227bb78acf9b91060d03c56d3f2072cdd9f1bd08290c11e8869f1a663b16da2",
                    "canonical_jq_cS_sha256": "6a63582e96c9092edcdc19935484cadcd72a1ae128762f6d666fc2017596d310",
                    "vendored_sha256": "6a63582e96c9092edcdc19935484cadcd72a1ae128762f6d666fc2017596d310"
                },
                {
                    "id": "ThreadListResponse",
                    "path": "protocol/thread/ThreadListResponse.canonical.json",
                    "generated_path": "v2/ThreadListResponse.json",
                    "generated_raw_sha256": "08d5ffb0a799cf0d1c42c11b12c8bc4b04d6e515f96c6789bbec532eba1b2612",
                    "canonical_jq_cS_sha256": "f3d94a229732a0756eb8c6698e325c05a1105fb2b6fe668814b8e3277a21f130",
                    "vendored_sha256": "f3d94a229732a0756eb8c6698e325c05a1105fb2b6fe668814b8e3277a21f130"
                }
            ],
            "source_kinds": [
                "cli",
                "vscode",
                "exec",
                "appServer",
                "subAgent",
                "subAgentReview",
                "subAgentCompact",
                "subAgentThreadSpawn",
                "subAgentOther",
                "unknown"
            ],
            "replacement_gate": {
                "implicit_runtime_follow": false,
                "fail_gate": "G2",
                "changed_inputs": [
                    "codex_cli_version",
                    "generated_utc_date",
                    "generation_command",
                    "experimental",
                    "bundle.raw_sha256",
                    "artifact generated_raw_sha256",
                    "artifact canonical_jq_cS_sha256",
                    "artifact vendored_sha256",
                    "source_kinds",
                    "generated schema definitions"
                ],
                "required_before_replacement": [
                    "update protocol/thread artifacts and manifest",
                    "update REQUIREMENTS.md, DESIGN.md, SECURITY.md, VERIFICATION_PLAN.md",
                    "update positive, rejection, boundary, conflict, E2E and mutation fixtures",
                    "rerun G1, G2, G3, G4 and preimplementation integration gate"
                ]
            }
        });
        assert_eq!(manifest, expected_manifest);

        let identity = pinned_schema_identity().expect("manifest is pinned");
        assert_eq!(identity.manifest_id, "CODEX_INFO_THREAD_SCHEMA_MANIFEST_V1");
        assert_eq!(identity.cli_version, "0.147.0");
        assert_eq!(identity.utc_date, "2026-08-14");
        assert_eq!(
            identity.bundle_raw_sha256,
            "f3dec1e031d99a420b137b903f02196d4325eece57620c925bb7130b25f168d2"
        );
        assert_eq!(
            identity.params_raw_sha256,
            "b227bb78acf9b91060d03c56d3f2072cdd9f1bd08290c11e8869f1a663b16da2"
        );
        assert_eq!(
            identity.params_canonical_sha256,
            "6a63582e96c9092edcdc19935484cadcd72a1ae128762f6d666fc2017596d310"
        );
        assert_eq!(
            identity.response_raw_sha256,
            "08d5ffb0a799cf0d1c42c11b12c8bc4b04d6e515f96c6789bbec532eba1b2612"
        );
        assert_eq!(
            identity.response_canonical_sha256,
            "f3d94a229732a0756eb8c6698e325c05a1105fb2b6fe668814b8e3277a21f130"
        );

        let params: Value = serde_json::from_str(include_str!(
            "../protocol/thread/ThreadListParams.canonical.json"
        ))
        .expect("params schema JSON");
        assert_eq!(
            params["definitions"]["ThreadSourceKind"],
            json!({
                "enum": [
                    "cli",
                    "vscode",
                    "exec",
                    "appServer",
                    "subAgent",
                    "subAgentReview",
                    "subAgentCompact",
                    "subAgentThreadSpawn",
                    "subAgentOther",
                    "unknown"
                ],
                "type": "string"
            })
        );

        let response: Value = serde_json::from_str(include_str!(
            "../protocol/thread/ThreadListResponse.canonical.json"
        ))
        .expect("response schema JSON");
        assert_eq!(
            response["definitions"]["SessionSource"],
            json!({
                "oneOf": [
                    {
                        "enum": ["cli", "vscode", "exec", "appServer", "unknown"],
                        "type": "string"
                    },
                    {
                        "additionalProperties": false,
                        "properties": {"custom": {"type": "string"}},
                        "required": ["custom"],
                        "title": "CustomSessionSource",
                        "type": "object"
                    },
                    {
                        "additionalProperties": false,
                        "properties": {
                            "subAgent": {"$ref": "#/definitions/SubAgentSource"}
                        },
                        "required": ["subAgent"],
                        "title": "SubAgentSessionSource",
                        "type": "object"
                    }
                ]
            })
        );
        assert_eq!(
            response["definitions"]["SubAgentSource"],
            json!({
                "oneOf": [
                    {
                        "enum": ["review", "compact", "memory_consolidation"],
                        "type": "string"
                    },
                    {
                        "additionalProperties": false,
                        "properties": {
                            "thread_spawn": {
                                "properties": {
                                    "agent_nickname": {
                                        "default": null,
                                        "type": ["string", "null"]
                                    },
                                    "agent_path": {
                                        "anyOf": [
                                            {"$ref": "#/definitions/AgentPath"},
                                            {"type": "null"}
                                        ],
                                        "default": null
                                    },
                                    "agent_role": {
                                        "default": null,
                                        "type": ["string", "null"]
                                    },
                                    "depth": {"format": "int32", "type": "integer"},
                                    "parent_thread_id": {"$ref": "#/definitions/ThreadId"}
                                },
                                "required": ["depth", "parent_thread_id"],
                                "type": "object"
                            }
                        },
                        "required": ["thread_spawn"],
                        "title": "ThreadSpawnSubAgentSource",
                        "type": "object"
                    },
                    {
                        "additionalProperties": false,
                        "properties": {"other": {"type": "string"}},
                        "required": ["other"],
                        "title": "OtherSubAgentSource",
                        "type": "object"
                    }
                ]
            })
        );
    }

    #[test]
    fn thread_c_request_first_page_exact_literal() {
        let actual = thread_list_request(None).expect("request");
        let expected = first_page_request_literal();
        assert_eq!(actual, expected);
        let object = actual.as_object().expect("request object");
        assert_eq!(object.len(), 6);
        for forbidden in ["cursor", "cwd", "modelProviders", "searchTerm", "sectionId"] {
            assert!(
                !object.contains_key(forbidden),
                "unexpected key {forbidden}"
            );
        }
    }

    #[test]
    fn thread_c_request_followup_cursor_boundaries_and_omissions() {
        let one = "x";
        let thousand_twenty_four = "界".repeat(1024);
        assert_eq!(one.chars().count(), 1);
        assert_eq!(thousand_twenty_four.chars().count(), 1024);
        assert_eq!(
            thread_list_request(Some(one)).unwrap()["cursor"],
            json!(one)
        );
        assert_eq!(
            thread_list_request(Some(&thousand_twenty_four)).unwrap()["cursor"],
            json!(thousand_twenty_four)
        );
        assert!(matches!(
            thread_list_request(Some("")),
            Err(ThreadContractError::InvalidCursor)
        ));
        let overlong = "x".repeat(1025);
        assert!(matches!(
            thread_list_request(Some(&overlong)),
            Err(ThreadContractError::InvalidCursor)
        ));
        let follow = thread_list_request(Some("opaque")).unwrap();
        let object = follow.as_object().expect("follow-up request object");
        assert_eq!(object.len(), 7);
        let mut without_cursor = follow.clone();
        without_cursor
            .as_object_mut()
            .expect("follow-up object")
            .remove("cursor");
        assert_eq!(without_cursor, first_page_request_literal());
        for forbidden in ["cwd", "modelProviders", "searchTerm", "sectionId"] {
            assert!(!object.contains_key(forbidden));
        }
    }

    #[test]
    fn thread_c_page_envelope_schema_matrix_is_atomic() {
        for envelope in [page(json!([]), None), page(json!([]), Some(json!(null)))] {
            let mut accumulator = ThreadCycleAccumulator::new();
            assert_eq!(
                accumulator.accept_page(&envelope),
                Ok(PageAcceptance::Terminal)
            );
        }

        let mut one_cursor = ThreadCycleAccumulator::new();
        assert_eq!(
            one_cursor.accept_page(&page(json!([]), Some(json!("x")))),
            Ok(PageAcceptance::NeedNextPage {
                cursor: "x".to_owned()
            })
        );
        let cursor_1024 = "界".repeat(1024);
        assert_eq!(cursor_1024.chars().count(), 1024);
        let mut max_cursor = ThreadCycleAccumulator::new();
        assert_eq!(
            max_cursor.accept_page(&page(json!([]), Some(json!(cursor_1024.clone())))),
            Ok(PageAcceptance::NeedNextPage {
                cursor: cursor_1024
            })
        );

        let mut invalid_cursor = ThreadCycleAccumulator::new();
        assert_invalid_envelope(&mut invalid_cursor, page(json!([]), Some(json!(""))));
        let mut invalid_cursor = ThreadCycleAccumulator::new();
        let overlong = "x".repeat(1025);
        assert_invalid_envelope(&mut invalid_cursor, page(json!([]), Some(json!(overlong))));
        for wrong_cursor in [json!(0), json!({})] {
            let mut invalid_cursor = ThreadCycleAccumulator::new();
            assert_invalid_envelope(&mut invalid_cursor, page(json!([]), Some(wrong_cursor)));
        }

        let mut accumulator = ThreadCycleAccumulator::new();
        let mut invalid_item = full_thread();
        invalid_item["id"] = json!(42);
        assert_eq!(
            accumulator.accept_page(&page(
                json!([full_thread(), invalid_item]),
                Some(json!("continue")),
            )),
            Ok(PageAcceptance::NeedNextPage {
                cursor: "continue".to_owned()
            })
        );
        assert_eq!(accumulator.candidates.len(), 1);
        assert_eq!(accumulator.rejected_count, 1);
        let before = accumulator.clone();
        let mut fatal_envelopes = vec![
            json!(null),
            json!([]),
            json!("root"),
            json!({}),
            json!({"nextCursor": "next"}),
            json!({"data": null}),
            json!({"data": {}}),
            json!({"data": "not-an-array"}),
            json!({"data": [], "backwardsCursor": 0}),
            json!({"data": [], "backwardsCursor": {}}),
            json!({"data": [], "nextCursor": 0}),
            json!({"data": [], "nextCursor": {}}),
            json!({"data": [], "nextCursor": ""}),
        ];
        let overlong = "x".repeat(1025);
        fatal_envelopes.push(json!({"data": [], "nextCursor": overlong}));
        for invalid in fatal_envelopes {
            let mut failed = before.clone();
            assert_invalid_envelope(&mut failed, invalid);
            assert!(failed.is_failed());
            assert_eq!(failed.candidates, before.candidates);
            assert_eq!(failed.rejected_count, before.rejected_count);
            assert_eq!(failed.candidates.len(), before.candidates.len());
            for (candidate, expected) in failed
                .debug_candidates()
                .iter()
                .zip(before.debug_candidates().iter())
            {
                assert_eq!(candidate.raw_json(), expected.raw_json());
                assert_eq!(candidate.id(), expected.id());
                assert_eq!(candidate.updated_at(), expected.updated_at());
                assert_eq!(candidate.path(), expected.path());
                assert_eq!(candidate.title(), expected.title());
            }
        }

        let mut valid_second = full_thread();
        valid_second["id"] = json!("thread-2");
        valid_second["name"] = json!("second");
        let mut invalid_later = full_thread();
        invalid_later["turns"] = json!({});
        let mut valid_third = full_thread();
        valid_third["id"] = json!("thread-3");
        valid_third["name"] = json!("third");
        let mut siblings = accumulator.clone();
        assert_eq!(
            siblings.accept_page(&page(
                json!([valid_second.clone(), invalid_later, valid_third.clone()]),
                None
            )),
            Ok(PageAcceptance::Terminal)
        );
        assert_eq!(siblings.candidates.len(), 3);
        assert_eq!(siblings.rejected_count, 2);
        let mut expected = ThreadCycleAccumulator::new();
        assert_eq!(
            expected.accept_page(&page(
                json!([full_thread(), valid_second, valid_third]),
                None
            )),
            Ok(PageAcceptance::Terminal)
        );
        assert_eq!(
            siblings.clone().ordered_candidates().unwrap(),
            expected.ordered_candidates().unwrap()
        );

        for backwards in [json!(""), json!("x".repeat(1025))] {
            let mut terminal = accumulator.clone();
            assert_eq!(
                terminal.accept_page(&page_with_backwards(
                    json!([]),
                    None,
                    Some(backwards.clone()),
                )),
                Ok(PageAcceptance::Terminal)
            );
            assert_eq!(terminal.candidates, before.candidates);
            assert_eq!(terminal.rejected_count, before.rejected_count);

            let mut follow_up = before.clone();
            assert_eq!(
                follow_up.accept_page(&page_with_backwards(
                    json!([]),
                    Some(json!("next")),
                    Some(backwards),
                )),
                Ok(PageAcceptance::NeedNextPage {
                    cursor: "next".to_owned()
                })
            );
            assert_eq!(follow_up.candidates, before.candidates);
            assert_eq!(follow_up.rejected_count, before.rejected_count);
        }
    }

    #[test]
    fn thread_c_thread_schema_required_and_type_matrix() {
        let valid = full_thread();
        assert!(validate_thread_item(&valid).is_ok());
        for field in [
            "cliVersion",
            "createdAt",
            "cwd",
            "ephemeral",
            "id",
            "modelProvider",
            "preview",
            "sessionId",
            "source",
            "status",
            "turns",
            "updatedAt",
        ] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                matches!(
                    validate_thread_item(&missing),
                    Err(ThreadContractError::InvalidItem)
                ),
                "missing {field}"
            );
        }
        for (field, wrong_value) in [
            ("cliVersion", json!(1)),
            ("createdAt", json!("1")),
            ("cwd", json!(1)),
            ("ephemeral", json!("false")),
            ("id", json!(1)),
            ("modelProvider", json!(1)),
            ("preview", json!(1)),
            ("sessionId", json!(1)),
            ("source", json!(1)),
            ("status", json!("idle")),
            ("turns", json!({})),
            ("updatedAt", json!("1")),
        ] {
            let mut wrong = valid.clone();
            wrong[field] = wrong_value;
            assert!(
                matches!(
                    validate_thread_item(&wrong),
                    Err(ThreadContractError::InvalidItem)
                ),
                "wrong type {field}"
            );
        }
    }

    #[test]
    fn thread_c_session_source_all_schema_valid_forms() {
        let cases = [
            ("cli", json!("cli")),
            ("vscode", json!("vscode")),
            ("exec", json!("exec")),
            ("appServer", json!("appServer")),
            ("unknown", json!("unknown")),
            ("custom literal", json!({"custom": "literal"})),
            ("subAgent review", json!({"subAgent": "review"})),
            ("subAgent compact", json!({"subAgent": "compact"})),
            (
                "subAgent memory_consolidation",
                json!({"subAgent": "memory_consolidation"}),
            ),
            (
                "thread_spawn base",
                json!({
                    "subAgent": {
                        "thread_spawn": {"depth": 0, "parent_thread_id": "parent"}
                    }
                }),
            ),
            (
                "thread_spawn agent_nickname null",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_nickname": null
                        }
                    }
                }),
            ),
            (
                "thread_spawn agent_nickname string",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_nickname": "nick"
                        }
                    }
                }),
            ),
            (
                "thread_spawn agent_path null",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_path": null
                        }
                    }
                }),
            ),
            (
                "thread_spawn agent_path string",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_path": "agent/path"
                        }
                    }
                }),
            ),
            (
                "thread_spawn agent_role null",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_role": null
                        }
                    }
                }),
            ),
            (
                "thread_spawn agent_role string",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_role": "role"
                        }
                    }
                }),
            ),
            (
                "thread_spawn all optional strings",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_nickname": "nick",
                            "agent_path": "agent/path",
                            "agent_role": "role"
                        }
                    }
                }),
            ),
            (
                "thread_spawn unknown field",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "unknown": "literal"
                        }
                    }
                }),
            ),
            (
                "subAgent other literal",
                json!({"subAgent": {"other": "literal"}}),
            ),
        ];

        for (label, source) in cases {
            let mut item = full_thread();
            item["source"] = json!(source);
            assert!(validate_thread_item(&item).is_ok(), "{label}: expected Ok");
        }
    }

    #[test]
    fn thread_c_session_source_invalid_union_matrix_rejects_item() {
        let cases = [
            ("null", json!(null)),
            ("bool", json!(true)),
            ("number", json!(42)),
            ("array", json!(["cli"])),
            ("empty object", json!({})),
            ("unknown outer", json!({"unknown": "literal"})),
            (
                "multiple union keys",
                json!({"custom": "literal", "subAgent": "review"}),
            ),
            (
                "outer extra beside custom",
                json!({"custom": "literal", "extra": "literal"}),
            ),
            (
                "outer extra beside subAgent",
                json!({"subAgent": "review", "extra": "literal"}),
            ),
            ("custom wrong type", json!({"custom": 42})),
            ("subAgent empty", json!({"subAgent": {}})),
            ("subAgent invalid string", json!({"subAgent": "invalid"})),
            (
                "thread_spawn null",
                json!({"subAgent": {"thread_spawn": null}}),
            ),
            (
                "thread_spawn string",
                json!({"subAgent": {"thread_spawn": "literal"}}),
            ),
            (
                "thread_spawn missing depth",
                json!({
                    "subAgent": {"thread_spawn": {"parent_thread_id": "parent"}}
                }),
            ),
            (
                "thread_spawn missing parent_thread_id",
                json!({"subAgent": {"thread_spawn": {"depth": 0}}}),
            ),
            (
                "thread_spawn depth string",
                json!({
                    "subAgent": {"thread_spawn": {"depth": "0", "parent_thread_id": "parent"}}
                }),
            ),
            (
                "thread_spawn depth fraction",
                json!({
                    "subAgent": {"thread_spawn": {"depth": 0.5, "parent_thread_id": "parent"}}
                }),
            ),
            (
                "thread_spawn depth bool",
                json!({
                    "subAgent": {"thread_spawn": {"depth": true, "parent_thread_id": "parent"}}
                }),
            ),
            (
                "thread_spawn depth i32::MIN-1",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": (i32::MIN as i64) - 1,
                            "parent_thread_id": "parent"
                        }
                    }
                }),
            ),
            (
                "thread_spawn depth i32::MAX+1",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": (i32::MAX as i64) + 1,
                            "parent_thread_id": "parent"
                        }
                    }
                }),
            ),
            (
                "thread_spawn parent_thread_id null",
                json!({
                    "subAgent": {"thread_spawn": {"depth": 0, "parent_thread_id": null}}
                }),
            ),
            (
                "thread_spawn parent_thread_id number",
                json!({
                    "subAgent": {"thread_spawn": {"depth": 0, "parent_thread_id": 42}}
                }),
            ),
            (
                "thread_spawn agent_nickname wrong type",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_nickname": 42
                        }
                    }
                }),
            ),
            (
                "thread_spawn agent_path wrong type",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_path": 42
                        }
                    }
                }),
            ),
            (
                "thread_spawn agent_role wrong type",
                json!({
                    "subAgent": {
                        "thread_spawn": {
                            "depth": 0,
                            "parent_thread_id": "parent",
                            "agent_role": 42
                        }
                    }
                }),
            ),
            (
                "extra beside thread_spawn at SubAgentSource wrapper",
                json!({
                    "subAgent": {
                        "thread_spawn": {"depth": 0, "parent_thread_id": "parent"},
                        "extra": "literal"
                    }
                }),
            ),
            ("other wrong type", json!({"subAgent": {"other": 42}})),
            ("other empty object", json!({"subAgent": {"other": {}}})),
            ("other missing", json!({"subAgent": {}})),
            (
                "extra beside other",
                json!({"subAgent": {"other": "literal", "extra": "literal"}}),
            ),
        ];

        for (label, source) in cases {
            let mut item = full_thread();
            item["source"] = json!(source);
            let result = validate_thread_item(&item);
            assert!(
                matches!(result, Err(ThreadContractError::InvalidItem)),
                "{label}: expected Err(InvalidItem)"
            );
        }
    }

    #[test]
    fn thread_c_schema_valid_auxiliary_fields_are_ignored_and_invalid_rejected() {
        let baseline = validate_thread_item(&full_thread()).expect("validated baseline");
        let assert_valid = |label: &str, item: Value| {
            let raw_changed = &item != baseline.raw_json();
            let candidate = match validate_thread_item(&item) {
                Ok(candidate) => candidate,
                Err(_) => panic!("{label}"),
            };
            assert_eq!(candidate.id(), baseline.id(), "{label}: id");
            assert_eq!(
                candidate.updated_at(),
                baseline.updated_at(),
                "{label}: updated_at"
            );
            assert_eq!(candidate.path(), baseline.path(), "{label}: path");
            assert_eq!(candidate.title(), baseline.title(), "{label}: title");
            if raw_changed {
                assert_ne!(
                    candidate.raw_json(),
                    baseline.raw_json(),
                    "{label}: raw_json"
                );
            }
        };
        let assert_invalid = |label: &str, item: Value| {
            assert!(
                matches!(
                    validate_thread_item(&item),
                    Err(ThreadContractError::InvalidItem)
                ),
                "{label}"
            );
        };

        {
            let mut item = full_thread();
            item["status"] = json!({"type":"notLoaded"});
            assert_valid("status.notLoaded", item);
        }
        {
            let mut item = full_thread();
            item["status"] = json!({"type":"idle"});
            assert_valid("status.idle", item);
        }
        {
            let mut item = full_thread();
            item["status"] = json!({"type":"systemError"});
            assert_valid("status.systemError", item);
        }
        {
            let mut item = full_thread();
            item["status"] = json!({
                "type":"active",
                "activeFlags":["waitingOnApproval", "waitingOnUserInput"]
            });
            assert_valid("status.active.both_flags", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!([]);
            assert_valid("turns.empty", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!([{"id":"turn", "items":[], "status":"completed"}]);
            assert_valid("turns.one_minimal", item);
        }
        {
            let mut item = full_thread();
            item.as_object_mut()
                .expect("Thread object")
                .remove("threadSource");
            assert_valid("threadSource.absent", item);
        }
        {
            let mut item = full_thread();
            item["threadSource"] = Value::Null;
            assert_valid("threadSource.null", item);
        }
        {
            let mut item = full_thread();
            item["threadSource"] = json!("cli");
            assert_valid("threadSource.string", item);
        }
        {
            let mut item = full_thread();
            item["unrelatedUnknown"] = json!({"allowed": true});
            assert_valid("unknown_thread_field", item);
        }
        {
            let mut item = full_thread();
            item["backwardsCursor"] = json!("opaque");
            assert_valid("item.backwardsCursor.string", item);
        }
        {
            let mut item = full_thread();
            item["backwardsCursor"] = json!(42);
            assert_valid("item.backwardsCursor.number", item);
        }
        {
            let mut item = full_thread();
            item["backwardsCursor"] = json!({"opaque": true});
            assert_valid("item.backwardsCursor.object", item);
        }

        {
            let mut item = full_thread();
            item["status"] = json!("active");
            assert_invalid("status.string", item);
        }
        {
            let mut item = full_thread();
            item["status"] = json!({});
            assert_invalid("status.empty_object", item);
        }
        {
            let mut item = full_thread();
            item["status"] = json!({"type":"unknown"});
            assert_invalid("status.unknown_discriminator", item);
        }
        {
            let mut item = full_thread();
            item["status"] = json!({"type":"active"});
            assert_invalid("status.active_missing_flags", item);
        }
        {
            let mut item = full_thread();
            item["status"] = json!({"type":"active", "activeFlags":"waitingOnApproval"});
            assert_invalid("status.active_flags_wrong_type", item);
        }
        {
            let mut item = full_thread();
            item["status"] = json!({"type":"active", "activeFlags":["invalidFlag"]});
            assert_invalid("status.active_invalid_flag", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!({});
            assert_invalid("turns.object", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!([{"items":[], "status":"completed"}]);
            assert_invalid("turn.missing_id", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!([{"id":"turn", "status":"completed"}]);
            assert_invalid("turn.missing_items", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!([{"id":"turn", "items":[]}]);
            assert_invalid("turn.missing_status", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!([{"id":1, "items":[], "status":"completed"}]);
            assert_invalid("turn.id_wrong_type", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!([{"id":"turn", "items":{}, "status":"completed"}]);
            assert_invalid("turn.items_wrong_type", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!([{"id":"turn", "items":[], "status":1}]);
            assert_invalid("turn.status_wrong_type", item);
        }
        {
            let mut item = full_thread();
            item["turns"] = json!([{"id":"turn", "items":[], "status":"unknown"}]);
            assert_invalid("turn.status_invalid_value", item);
        }
        {
            let mut item = full_thread();
            item["threadSource"] = json!(true);
            assert_invalid("threadSource.boolean", item);
        }
        {
            let mut item = full_thread();
            item["threadSource"] = json!(42);
            assert_invalid("threadSource.number", item);
        }
        {
            let mut item = full_thread();
            item["threadSource"] = json!({"kind":"cli"});
            assert_invalid("threadSource.object", item);
        }

        let mut accumulator = ThreadCycleAccumulator::new();
        let envelope = page_with_backwards(json!([]), None, Some(json!(1)));
        assert!(
            matches!(
                accumulator.accept_page(&envelope),
                Err(ThreadContractError::InvalidEnvelope)
            ),
            "page.backwardsCursor.number"
        );
    }

    #[test]
    fn thread_c_candidate_semantic_id_updated_path_boundaries() {
        let assert_valid = |label: &str, item: Value| {
            assert!(validate_thread_item(&item).is_ok(), "{label}");
        };
        let assert_invalid = |label: &str, item: Value| {
            assert!(
                matches!(
                    validate_thread_item(&item),
                    Err(ThreadContractError::InvalidItem)
                ),
                "{label}"
            );
        };

        {
            let mut item = full_thread();
            item["updatedAt"] = json!(1);
            assert_valid("updatedAt.one", item);
        }
        {
            let mut item = full_thread();
            item["updatedAt"] = json!(i64::MAX);
            assert_valid("updatedAt.i64_max", item);
        }
        {
            let mut item = full_thread();
            item["updatedAt"] = json!(0);
            assert_invalid("updatedAt.zero", item);
        }
        {
            let mut item = full_thread();
            item["updatedAt"] = json!(-1);
            assert_invalid("updatedAt.negative_one", item);
        }
        {
            let mut item = full_thread();
            item["updatedAt"] = json!(1.5);
            assert_invalid("updatedAt.fractional", item);
        }
        {
            let mut item = full_thread();
            item["updatedAt"] = json!("1");
            assert_invalid("updatedAt.string", item);
        }
        {
            let mut item = full_thread();
            item["updatedAt"] = json!(true);
            assert_invalid("updatedAt.boolean", item);
        }
        {
            let mut item = full_thread();
            item["updatedAt"] = json!(u64::MAX);
            assert_invalid("updatedAt.u64_max", item);
        }

        {
            let mut item = full_thread();
            item["id"] = json!("界");
            assert_valid("id.one_unicode_scalar", item);
        }
        {
            let mut item = full_thread();
            item["id"] = json!("界".repeat(128));
            assert_valid("id.128_unicode_scalars", item);
        }
        {
            let mut item = full_thread();
            item["id"] = json!("");
            assert_invalid("id.zero_unicode_scalars", item);
        }
        {
            let mut item = full_thread();
            item["id"] = json!("界".repeat(129));
            assert_invalid("id.129_unicode_scalars", item);
        }
        {
            let mut item = full_thread();
            item["id"] = json!(1);
            assert_invalid("id.non_string", item);
        }

        {
            let mut item = full_thread();
            item.as_object_mut().expect("Thread object").remove("path");
            assert_valid("path.absent", item);
        }
        {
            let mut item = full_thread();
            item["path"] = Value::Null;
            assert_valid("path.null", item);
        }
        {
            let mut item = full_thread();
            item["path"] = json!("/tmp/thread");
            assert_valid("path.absolute_nonempty", item);
        }
        {
            let mut item = full_thread();
            item["path"] = json!("relative/thread");
            assert_valid("path.relative_nonempty", item);
        }
        {
            let mut item = full_thread();
            item["path"] = json!("");
            assert_invalid("path.empty", item);
        }
        {
            let mut item = full_thread();
            item["path"] = json!(42);
            assert_invalid("path.non_string", item);
        }

        {
            let mut item = full_thread();
            item["cwd"] = json!("/");
            assert_valid("cwd.root", item);
        }
        {
            let mut item = full_thread();
            item["cwd"] = json!("/tmp/codex");
            assert_valid("cwd.absolute_tmp", item);
        }
        {
            let mut item = full_thread();
            item["cwd"] = json!("");
            assert_invalid("cwd.empty", item);
        }
        {
            let mut item = full_thread();
            item["cwd"] = json!("relative");
            assert_invalid("cwd.relative", item);
        }
        {
            let mut item = full_thread();
            item["cwd"] = json!(42);
            assert_invalid("cwd.non_string", item);
        }
    }

    #[test]
    fn thread_c_candidate_order_updated_desc_then_id_desc() {
        let mut accumulator = ThreadCycleAccumulator::new();
        let rows = json!([
            thread_fixture("a", 10, "a"),
            thread_fixture("old", 1, "old"),
            thread_fixture("z", 10, "z"),
            thread_fixture("new", 20, "new"),
        ]);
        assert_eq!(
            accumulator.accept_page(&page(rows, None)),
            Ok(PageAcceptance::Terminal)
        );
        let ids: Vec<_> = accumulator
            .ordered_candidates()
            .unwrap()
            .into_iter()
            .map(|candidate| candidate.id().to_owned())
            .collect();
        assert_eq!(ids, ["new", "z", "a", "old"]);
    }

    #[test]
    fn thread_c_pagination_terminal_and_empty_page_matrix() {
        for terminal in [page(json!([]), None), page(json!([]), Some(Value::Null))] {
            let mut accumulator = ThreadCycleAccumulator::new();
            assert_eq!(
                accumulator.accept_page(&terminal),
                Ok(PageAcceptance::Terminal)
            );
            assert!(accumulator.is_terminal());
            assert!(accumulator.ordered_candidates().unwrap().is_empty());
        }

        let mut accumulator = ThreadCycleAccumulator::new();
        assert_eq!(
            accumulator.accept_page(&page(json!([]), Some(json!("page-2")))),
            Ok(PageAcceptance::NeedNextPage {
                cursor: "page-2".to_owned()
            })
        );
        assert!(!accumulator.is_terminal());
        assert_eq!(
            accumulator.accept_page(&page(json!([thread_fixture("later", 2, "later")]), None,)),
            Ok(PageAcceptance::Terminal)
        );
        let selected = accumulator.ordered_candidates().unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id(), "later");
    }

    #[test]
    fn thread_c_cursor_cycle_and_32_page_budget_fail_closed() {
        let mut thirty_two = ThreadCycleAccumulator::new();
        for page_number in 1..32 {
            let cursor = format!("cursor-{page_number}");
            assert_eq!(
                thirty_two.accept_page(&page(json!([]), Some(json!(cursor.clone())))),
                Ok(PageAcceptance::NeedNextPage { cursor })
            );
        }
        assert_eq!(
            thirty_two.accept_page(&page(json!([]), None)),
            Ok(PageAcceptance::Terminal)
        );
        assert!(thirty_two.ordered_candidates().unwrap().is_empty());

        let mut request_thirty_three = ThreadCycleAccumulator::new();
        for page_number in 1..32 {
            let cursor = format!("cursor-{page_number}");
            assert!(matches!(
                request_thirty_three.accept_page(&page(json!([]), Some(json!(cursor)))),
                Ok(PageAcceptance::NeedNextPage { .. })
            ));
        }
        assert_eq!(
            request_thirty_three.accept_page(&page(json!([]), Some(json!("cursor-32")))),
            Err(ThreadContractError::ValidationBudgetExceeded)
        );
        assert!(request_thirty_three.is_failed());
        assert_eq!(
            request_thirty_three.ordered_candidates(),
            Err(ThreadContractError::InvalidRequest)
        );

        for cursors in [["A", "A", "unused"], ["A", "B", "A"]] {
            let mut accumulator = ThreadCycleAccumulator::new();
            assert!(matches!(
                accumulator.accept_page(&page(json!([]), Some(json!(cursors[0])))),
                Ok(PageAcceptance::NeedNextPage { .. })
            ));
            if cursors[1] != cursors[0] {
                assert!(matches!(
                    accumulator.accept_page(&page(json!([]), Some(json!(cursors[1])))),
                    Ok(PageAcceptance::NeedNextPage { .. })
                ));
            }
            let repeated = if cursors[1] == cursors[0] {
                cursors[1]
            } else {
                cursors[2]
            };
            assert_eq!(
                accumulator.accept_page(&page(json!([]), Some(json!(repeated)))),
                Err(ThreadContractError::InvalidEnvelope)
            );
            assert!(accumulator.is_failed());
        }
    }

    #[test]
    fn thread_c_1024_unique_item_budget_exact_boundary() {
        let rows: Vec<_> = (0..1024)
            .map(|index| {
                thread_fixture(
                    &format!("thread-{index:04}"),
                    i64::from(index) + 1,
                    "candidate",
                )
            })
            .collect();

        let mut exact = ThreadCycleAccumulator::new();
        let mut exact_with_duplicate = rows.clone();
        exact_with_duplicate.push(rows[0].clone());
        assert_eq!(
            exact.accept_page(&page(Value::Array(exact_with_duplicate), None)),
            Ok(PageAcceptance::Terminal)
        );
        assert_eq!(exact.ordered_candidates().unwrap().len(), 1024);

        let mut overflow = ThreadCycleAccumulator::new();
        assert_eq!(
            overflow.accept_page(&page(Value::Array(rows), Some(json!("next")))),
            Ok(PageAcceptance::NeedNextPage {
                cursor: "next".to_owned()
            })
        );
        assert_eq!(
            overflow.accept_page(&page(
                json!([thread_fixture("thread-1024", 1025, "overflow")]),
                None,
            )),
            Err(ThreadContractError::ValidationBudgetExceeded)
        );
        assert!(overflow.is_failed());
        assert_eq!(
            overflow.ordered_candidates(),
            Err(ThreadContractError::InvalidRequest)
        );
    }

    #[test]
    fn thread_c_identical_duplicate_deduplicates_only_once() {
        let duplicate = thread_fixture("same", 7, "same");
        let mut accumulator = ThreadCycleAccumulator::new();
        assert!(matches!(
            accumulator.accept_page(&page(json!([duplicate.clone()]), Some(json!("next")),)),
            Ok(PageAcceptance::NeedNextPage { .. })
        ));
        assert_eq!(
            accumulator.accept_page(&page(json!([duplicate.clone(), duplicate.clone()]), None)),
            Ok(PageAcceptance::Terminal)
        );
        let candidates = accumulator.ordered_candidates().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].raw_json(), &duplicate);
    }

    #[test]
    fn thread_c_same_id_latest_and_equal_timestamp_conflict_rules() {
        let old = thread_fixture("same", 10, "old");
        let new = thread_fixture("same", 20, "new");
        for rows in [
            json!([old.clone(), new.clone()]),
            json!([new.clone(), old.clone()]),
        ] {
            let mut accumulator = ThreadCycleAccumulator::new();
            assert_eq!(
                accumulator.accept_page(&page(rows, None)),
                Ok(PageAcceptance::Terminal)
            );
            let candidates = accumulator.ordered_candidates().unwrap();
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].updated_at(), 20);
            assert_eq!(candidates[0].title(), "new");
        }

        let equal_a = thread_fixture("conflict", 30, "A");
        let equal_b = thread_fixture("conflict", 30, "B");
        for rows in [
            json!([equal_a.clone(), equal_b.clone()]),
            json!([equal_b.clone(), equal_a.clone()]),
            json!([
                equal_a.clone(),
                equal_b.clone(),
                thread_fixture("conflict", 40, "newer")
            ]),
        ] {
            let mut accumulator = ThreadCycleAccumulator::new();
            assert_eq!(
                accumulator.accept_page(&page(rows, None)),
                Ok(PageAcceptance::Terminal)
            );
            assert!(accumulator.ordered_candidates().unwrap().is_empty());
        }
    }

    #[test]
    fn thread_c_private_accumulator_abort_never_yields_partial_snapshot() {
        let mut after_page_one = ThreadCycleAccumulator::new();
        assert!(matches!(
            after_page_one.accept_page(&page(
                json!([thread_fixture("private", 1, "private")]),
                Some(json!("next")),
            )),
            Ok(PageAcceptance::NeedNextPage { .. })
        ));
        assert_eq!(
            after_page_one.clone().ordered_candidates(),
            Err(ThreadContractError::InvalidRequest)
        );

        let mut after_page_n = after_page_one.clone();
        assert!(matches!(
            after_page_n.accept_page(&page(json!([]), Some(json!("next-2")))),
            Ok(PageAcceptance::NeedNextPage { .. })
        ));
        assert_eq!(
            after_page_n.clone().ordered_candidates(),
            Err(ThreadContractError::InvalidRequest)
        );

        after_page_one.abort();
        after_page_n.abort();

        let mut terminal = ThreadCycleAccumulator::new();
        assert_eq!(
            terminal.accept_page(&page(
                json!([thread_fixture("visible", 2, "visible")]),
                None,
            )),
            Ok(PageAcceptance::Terminal)
        );
        assert_eq!(terminal.ordered_candidates().unwrap().len(), 1);

        let source = include_str!("thread_contract.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("pub fn candidates("));
        assert!(!production.contains("pub fn raw_candidate_count("));
        assert!(production.contains("pub fn abort(self)"));
    }

    #[test]
    fn thread_c_rollout_last_valid_task_event_controls_running() {
        let cases = [
            ("no-task", vec![], false),
            ("top-started", vec![json!({"type":"task_started"})], true),
            (
                "nested-started",
                vec![json!({"type":"event_msg","payload":{"type":"task_started"}})],
                true,
            ),
            (
                "complete",
                vec![
                    json!({"type":"task_started"}),
                    json!({"type":"task_complete"}),
                ],
                false,
            ),
            (
                "completed",
                vec![
                    json!({"type":"task_started"}),
                    json!({"type":"event_msg","payload":{"type":"task_completed"}}),
                ],
                false,
            ),
            (
                "aborted",
                vec![
                    json!({"type":"task_started"}),
                    json!({"type":"turn_aborted"}),
                ],
                false,
            ),
            (
                "last-started-wins",
                vec![
                    json!({"type":"task_complete"}),
                    json!({"type":"event_msg","payload":{"type":"task_started"}}),
                ],
                true,
            ),
        ];
        for (label, events, expected) in cases {
            let parsed = parse_rollout(&rollout_bytes(&events)).unwrap();
            assert_eq!(parsed.is_running(), expected, "{label}");
        }
    }

    #[test]
    fn thread_c_schema_active_status_promotes_rollout_running_state() {
        let mut item = thread_fixture("active-status", 20, "active-status");
        item["status"] = json!({
            "type": "active",
            "activeFlags": ["waitingOnApproval"]
        });
        let candidate = validate_thread_item(&item).expect("active status validates");
        assert!(candidate.is_active());
        let outcome =
            select_active_threads(terminal_cycle(vec![item]), |_| -> Result<Vec<u8>, ()> {
                Ok(rollout_bytes(&[json!({"type":"task_complete"})]))
            });
        assert!(matches!(
            outcome,
            ThreadCycleOutcome::Snapshots(rows)
                if rows.len() == 1 && rows[0].thread_id == "active-status"
        ));
    }

    #[test]
    fn thread_c_model_scalar_and_label_complete_matrix() {
        let started = json!({"type":"task_started"});
        let no_model = parse_rollout(&rollout_bytes(std::slice::from_ref(&started))).unwrap();
        assert_eq!(no_model.model(), "不明");
        assert_eq!(no_model.model_label(), "不明");

        for (length, expected_label) in [
            (0, "不明".to_owned()),
            (1, "界".to_owned()),
            (24, "界".repeat(24)),
            (25, format!("{}…", "界".repeat(23))),
            (128, format!("{}…", "界".repeat(23))),
        ] {
            let model = "界".repeat(length);
            let events = [
                json!({"type":"thread_context","model":model}),
                started.clone(),
            ];
            let parsed = parse_rollout(&rollout_bytes(&events)).unwrap();
            assert_eq!(
                parsed.model(),
                if length == 0 {
                    "不明"
                } else {
                    model.as_str()
                },
                "length={length}"
            );
            assert_eq!(parsed.model_label(), expected_label, "length={length}");
        }

        for model_event in [
            json!({"type":"event_msg","payload":{"type":"turn_context","model":"nested"}}),
            json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"settings"}}}),
        ] {
            let expected = if model_event.pointer("/payload/type") == Some(&json!("turn_context")) {
                "nested"
            } else {
                "settings"
            };
            let parsed = parse_rollout(&rollout_bytes(&[model_event, started.clone()])).unwrap();
            assert_eq!(parsed.model(), expected);
        }

        let overlong = "界".repeat(129);
        assert_eq!(
            parse_rollout(&rollout_bytes(&[
                json!({"type":"thread_context","model":overlong}),
                started,
            ])),
            Err(RolloutError::InvalidKnownEvent)
        );
    }

    #[test]
    fn thread_c_cumulative_total_token_literal_matrix() {
        let events = [
            json!({"type":"task_started"}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"total_tokens":0,"input_tokens":900,"reasoning_output_tokens":800},
                "last_token_usage":{"total_tokens":700}
            }}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"total_tokens":999,"input_tokens":1,"cached_input_tokens":2,"output_tokens":3},
                "last_token_usage":{"total_tokens":999999}
            }}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"total_tokens":1000,"reasoning_output_tokens":u64::MAX},
                "last_token_usage":{"total_tokens":1}
            }}}),
            json!({"type":"token_count","info":{"total_token_usage":{"total_tokens":7}}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"total_tokens":u64::MAX,"input_tokens":1,"output_tokens":1},
                "last_token_usage":{"total_tokens":2}
            }}}),
        ];
        let parsed = parse_rollout(&rollout_bytes(&events)).unwrap();
        assert_eq!(parsed.total_tokens(), Some(u64::MAX));
    }

    #[test]
    fn thread_c_context_window_is_taken_from_the_latest_token_count() {
        let events = [
            json!({"type":"task_started"}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"total_tokens":10},
                "last_token_usage":{"total_tokens":8},
                "model_context_window":128000
            }}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"total_tokens":20},
                "last_token_usage":{"total_tokens":18},
                "model_context_window":258400
            }}}),
        ];
        let parsed = parse_rollout(&rollout_bytes(&events)).unwrap();
        assert_eq!(parsed.total_tokens(), Some(20));
        assert_eq!(parsed.context_usage_tokens(), Some(18));
        assert_eq!(parsed.context_window_tokens(), Some(258_400));

        let without_field = parse_rollout(&rollout_bytes(&[
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"total_tokens":25},
                "model_context_window":128000
            }}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"total_tokens":30}
            }}}),
        ]))
        .unwrap();
        assert_eq!(without_field.total_tokens(), Some(30));
        assert_eq!(without_field.context_usage_tokens(), None);
        assert_eq!(without_field.context_window_tokens(), None);
    }

    #[test]
    fn thread_c_known_token_invalid_event_rejects_entire_rollout() {
        let started = r#"{"type":"task_started"}"#;
        let valid = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":55}}}}"#;
        let invalid = [
            r#"{"type":"event_msg","payload":{"type":"token_count"}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":null}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":null}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":-1}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1.5}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":"1"}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":true}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":[]}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":{}}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":18446744073709551616}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1},"model_context_window":null}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1},"model_context_window":-1}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1},"model_context_window":"258400"}}}"#,
        ];
        for line in invalid {
            assert_eq!(
                parse_rollout(&rollout_raw_lines(&[started, valid, line])),
                Err(RolloutError::InvalidKnownEvent),
                "{line}"
            );
        }
    }

    #[test]
    fn thread_c_initial_null_token_count_is_a_safe_noop() {
        let rollout = rollout_raw_lines(&[
            r#"{"type":"task_started"}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":17}}}}"#,
        ]);
        let parsed = parse_rollout(&rollout).expect("null initial token sample is ignored");
        assert!(parsed.is_running());
        assert_eq!(parsed.total_tokens(), Some(17));
    }

    #[test]
    fn thread_c_known_task_and_model_required_field_matrix_rejects_file() {
        let invalid_values = [
            json!({"type":"event_msg"}),
            json!({"type":"event_msg","payload":null}),
            json!({"type":"event_msg","payload":"task_started"}),
            json!({"type":"event_msg","payload":[]}),
            json!({"type":"event_msg","payload":{}}),
            json!({"type":"event_msg","payload":{"type":1}}),
            json!({"type":"turn_context"}),
            json!({"type":"turn_context","model":null}),
            json!({"type":"turn_context","model":1}),
            json!({"type":"event_msg","payload":{"type":"thread_context"}}),
            json!({"type":"event_msg","payload":{"type":"thread_context","model":false}}),
            json!({"type":"thread_settings_applied","thread_settings":null}),
            json!({"type":"thread_settings_applied","thread_settings":{}}),
            json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":[]}}}),
            json!({"type":"thread_context","model":"界".repeat(129)}),
        ];
        for (index, invalid) in invalid_values.into_iter().enumerate() {
            let events = [
                json!({"type":"task_started"}),
                json!({"type":"thread_context","model":"valid-before"}),
                invalid,
            ];
            assert_eq!(
                parse_rollout(&rollout_bytes(&events)),
                Err(RolloutError::InvalidKnownEvent),
                "index={index}"
            );
        }
    }

    #[test]
    fn thread_c_rollout_bytes_utf8_jsonl_and_size_boundaries() {
        assert_eq!(
            parse_rollout(b""),
            Ok(ValidatedRollout {
                running: false,
                model: "不明".to_owned(),
                model_label: "不明".to_owned(),
                total_tokens: None,
                context_usage_tokens: None,
                context_window_tokens: None,
                last_user_message_at: None,
            })
        );
        assert!(parse_rollout(b"{}\n").is_ok());

        let prefix = r#"{"type":"future","padding":""#;
        let suffix = r#""}"#;
        let padding = security::MAX_JSONL_LINE_BYTES - prefix.len() - suffix.len();
        let mut exact_line = String::with_capacity(security::MAX_JSONL_LINE_BYTES + 1);
        exact_line.push_str(prefix);
        exact_line.push_str(&"x".repeat(padding));
        exact_line.push_str(suffix);
        assert_eq!(exact_line.len(), security::MAX_JSONL_LINE_BYTES);
        exact_line.push('\n');
        assert!(parse_rollout(exact_line.as_bytes()).is_ok());

        assert_eq!(
            parse_rollout(&[0xff, b'\n']),
            Err(RolloutError::InvalidUtf8)
        );
        assert_eq!(parse_rollout(b"{\n"), Err(RolloutError::InvalidJson));
        assert_eq!(
            parse_rollout(b"{}\n{}"),
            Err(RolloutError::UnterminatedLine)
        );
        let mut overlong_line = vec![b' '; security::MAX_JSONL_LINE_BYTES + 1];
        overlong_line.push(b'\n');
        assert_eq!(
            parse_rollout(&overlong_line),
            Err(RolloutError::LineTooLarge)
        );
        let mut oversized_file = Cursor::new(b"\n".as_slice());
        assert_eq!(
            parse_rollout_reader(&mut oversized_file, security::MAX_SESSION_FILE_BYTES + 1),
            Err(RolloutError::FileTooLarge)
        );
    }

    #[test]
    fn recoverable_rollout_parser_keeps_running_state_around_large_tool_output() {
        let prefix = rollout_bytes(&[
            json!({"type":"thread_context","model":"gpt-5.6-luna"}),
            json!({"type":"task_started"}),
        ]);
        let oversized = format!(
            "{{\"type\":\"response_item\",\"payload\":\"{}\"}}\n",
            "x".repeat(security::MAX_JSONL_LINE_BYTES + 128)
        );
        let suffix = rollout_bytes(&[
            json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":321}}}}),
        ]);
        let mut bytes = prefix;
        bytes.extend_from_slice(oversized.as_bytes());
        bytes.extend_from_slice(&suffix);
        let snapshot_bytes = bytes.len() as u64;
        let parsed = parse_rollout_reader_recoverable(&mut Cursor::new(bytes), snapshot_bytes)
            .expect("large tool output is an isolated record");
        assert!(parsed.is_running());
        assert_eq!(parsed.model(), "gpt-5.6-luna");
        assert_eq!(parsed.total_tokens(), Some(321));
    }

    #[test]
    fn recoverable_rollout_parser_skips_malformed_non_state_records_only() {
        let bytes = b"{\"type\":\"thread_context\",\"model\":\"gpt-5.6-luna\"}\n{\"type\":\"task_started\"}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"rate_limits\":{\"primary\":{\"use\n{\"type\":\"task_complete\"}\n";
        let parsed = parse_rollout_reader_recoverable(&mut Cursor::new(bytes), bytes.len() as u64)
            .expect("malformed token_count is non-liveness data");
        assert!(!parsed.is_running());
        assert_eq!(parsed.model(), "gpt-5.6-luna");

        let lifecycle = b"{\"type\":\"task_started\"}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"\n";
        assert_eq!(
            parse_rollout_reader_recoverable(&mut Cursor::new(lifecycle), lifecycle.len() as u64),
            Err(RolloutError::InvalidJson)
        );

        let spaced = b"{ \"type\" : \"event_msg\", \"payload\" : { \"type\" : \"token_count\", \"info\" : {\n{\"type\":\"task_complete\"}\n";
        let parsed =
            parse_rollout_reader_recoverable(&mut Cursor::new(spaced), spaced.len() as u64)
                .expect("whitespace-only token_count envelope is non-liveness data");
        assert!(!parsed.is_running());

        let interrupted_reasoning = b"{\"type\":\"response_item\",\"payload\":{\"type\":\"reasoning\",\"summary\":[{\"text\":\"interrupted\n{\"type\":\"task_complete\"}\n";
        let parsed = parse_rollout_reader_recoverable(
            &mut Cursor::new(interrupted_reasoning),
            interrupted_reasoning.len() as u64,
        )
        .expect("interrupted transcript content is non-state data");
        assert!(!parsed.is_running());

        let lifecycle_with_token_word =
            b"{\"type\":\"task_started\",\"payload\":{\"type\":\"token_count\"\n";
        assert_eq!(
            parse_rollout_reader_recoverable(
                &mut Cursor::new(lifecycle_with_token_word),
                lifecycle_with_token_word.len() as u64
            ),
            Err(RolloutError::InvalidJson)
        );

        let quoted_words = b"{\"note\":\"\\\"type\\\":\\\"event_msg\\\" and \\\"type\\\":\\\"token_count\\\"\",\"payload\":{\n";
        assert_eq!(
            parse_rollout_reader_recoverable(
                &mut Cursor::new(quoted_words),
                quoted_words.len() as u64
            ),
            Err(RolloutError::InvalidJson)
        );
    }

    #[test]
    fn thread_c_unknown_well_formed_events_do_not_change_snapshot() {
        let base = [
            json!({"type":"thread_context","model":"model-a"}),
            json!({"type":"task_started"}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":321}}}}),
        ];
        let expected = parse_rollout(&rollout_bytes(&base)).unwrap();
        let with_unknown = [
            json!({"type":"future_top","model":"wrong"}),
            base[0].clone(),
            json!({"type":"event_msg","payload":{"type":"future_nested","model":"wrong","total_tokens":999}}),
            base[1].clone(),
            json!({"type":"future_top","payload":{"type":"task_complete"}}),
            base[2].clone(),
            json!({"unknown":"object"}),
        ];
        assert_eq!(parse_rollout(&rollout_bytes(&with_unknown)), Ok(expected));
    }

    #[test]
    fn thread_c_accepts_live_rollout_envelopes_with_top_level_type_and_payload() {
        let rollout = rollout_bytes(&[
            json!({
                "timestamp":"2026-08-15T00:00:00Z",
                "type":"turn_context",
                "payload":{"model":"gpt-5.6-sol","cwd":"/tmp"}
            }),
            json!({
                "timestamp":"2026-08-15T00:00:01Z",
                "type":"task_started",
                "payload":{}
            }),
            json!({
                "type":"event_msg",
                "payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":777}}}
            }),
        ]);
        let parsed = parse_rollout(&rollout).unwrap();
        assert!(parsed.is_running());
        assert_eq!(parsed.model(), "gpt-5.6-sol");
        assert_eq!(parsed.model_label(), "gpt-5.6-sol");
        assert_eq!(parsed.total_tokens(), Some(777));

        assert_eq!(
            parse_rollout(&rollout_bytes(&[json!({
                "type":"turn_context",
                "payload":false
            })])),
            Err(RolloutError::InvalidKnownEvent)
        );
    }

    #[test]
    fn thread_c_tracks_latest_user_instruction_timestamp_for_duration_display() {
        let rollout = rollout_bytes(&[
            json!({
                "timestamp":"2026-08-15T00:00:01Z",
                "type":"event_msg",
                "payload":{"type":"user_message","message":{"text":"first"}}
            }),
            json!({
                "timestamp":"2026-08-15T00:00:02Z",
                "type":"response_item",
                "payload":{"type":"message","role":"user","content":[]}
            }),
            json!({
                "timestamp":"2026-08-15T00:00:03Z",
                "type":"task_started"
            }),
            json!({
                "type":"event_msg",
                "payload":{"type":"user_message","message":{"text":"no timestamp"}}
            }),
        ]);
        let parsed = parse_rollout(&rollout).expect("user events are valid rollout records");
        assert_eq!(
            parsed.last_user_message_at(),
            Some(
                DateTime::parse_from_rfc3339("2026-08-15T00:00:02Z")
                    .expect("fixture timestamp")
                    .timestamp()
            )
        );
    }

    #[test]
    fn thread_c_candidate_failure_rejects_the_complete_cycle() {
        let cycle = terminal_cycle(vec![
            thread_fixture("inactive", 400, "inactive"),
            thread_fixture("parse-error", 300, "parse-error"),
            thread_fixture("model-error", 200, "model-error"),
            thread_fixture("winner", 100, "winner-title"),
        ]);
        let outcome = select_active_threads(cycle, |candidate| -> Result<Vec<u8>, ()> {
            Ok(match candidate.id() {
                "inactive" => rollout_bytes(&[json!({"type":"task_complete"})]),
                "parse-error" => b"{\n".to_vec(),
                "model-error" => rollout_bytes(&[
                    json!({"type":"thread_context","model":"界".repeat(129)}),
                    json!({"type":"task_started"}),
                ]),
                "winner" => rollout_bytes(&[
                    json!({"type":"thread_context","model":"winner-model"}),
                    json!({"type":"task_started"}),
                    json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":42}}}}),
                ]),
                _ => return Err(()),
            })
        });
        assert_eq!(outcome, ThreadCycleOutcome::CycleError);
    }

    #[test]
    fn thread_c_snapshot_rejects_partial_candidate_reads() {
        let cycle = terminal_cycle(vec![
            thread_fixture("candidate-a", 20, "title-a"),
            thread_fixture("candidate-b", 10, "title-b"),
        ]);
        let mut reads = Vec::new();
        let outcome = select_active_threads(cycle, |candidate| -> Result<Vec<u8>, ()> {
            reads.push(candidate.id().to_owned());
            if candidate.id() != "candidate-a" {
                return Err(());
            }
            Ok(rollout_bytes(&[
                json!({"type":"thread_context","model":"model-a"}),
                json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":111}}}}),
                json!({"type":"task_started"}),
            ]))
        });
        assert_eq!(reads, ["candidate-a", "candidate-b"]);
        assert_eq!(outcome, ThreadCycleOutcome::CycleError);
    }

    #[test]
    fn thread_c_valid_running_without_token_keeps_thread_with_none() {
        let cycle = terminal_cycle(vec![
            thread_fixture("without-token", 20, "without-token"),
            thread_fixture("older-with-token", 10, "older-with-token"),
        ]);
        let outcome = select_active_threads(cycle, |candidate| -> Result<Vec<u8>, ()> {
            Ok(if candidate.id() == "without-token" {
                rollout_bytes(&[
                    json!({"type":"thread_context","model":"model-only"}),
                    json!({"type":"task_started"}),
                ])
            } else {
                rollout_bytes(&[json!({"type":"task_complete"})])
            })
        });
        assert_eq!(
            outcome,
            ThreadCycleOutcome::Snapshots(vec![ActiveThreadSnapshot {
                thread_id: "without-token".to_owned(),
                created_at: 1,
                updated_at: 20,
                title: "without-token".to_owned(),
                model: "model-only".to_owned(),
                model_label: "model-only".to_owned(),
                total_tokens: None,
                context_usage_tokens: None,
                context_window_tokens: None,
                last_user_message_at: None,
                is_subagent: false,
                parent_thread_id: None,
                depth: None,
            }])
        );
    }

    #[test]
    fn thread_c_no_thread_and_all_candidate_failure_are_distinct() {
        let empty = terminal_cycle(vec![]);
        assert_eq!(
            select_active_threads(empty, |_| -> Result<Vec<u8>, ()> { unreachable!() }),
            ThreadCycleOutcome::NoThread
        );

        let inactive = terminal_cycle(vec![thread_fixture("inactive", 1, "inactive")]);
        assert_eq!(
            select_active_threads(inactive, |_| -> Result<Vec<u8>, ()> {
                Ok(rollout_bytes(&[json!({"type":"task_complete"})]))
            }),
            ThreadCycleOutcome::NoThread
        );

        let mut invalid_item = full_thread();
        invalid_item["id"] = json!(null);
        let rejected = terminal_cycle(vec![invalid_item]);
        assert_eq!(
            select_active_threads(rejected, |_| -> Result<Vec<u8>, ()> { unreachable!() }),
            ThreadCycleOutcome::CycleError
        );

        let unreadable = terminal_cycle(vec![thread_fixture("unreadable", 1, "unreadable")]);
        assert_eq!(
            select_active_threads(unreadable, |_| -> Result<Vec<u8>, ()> { Err(()) }),
            ThreadCycleOutcome::CycleError
        );
    }

    #[test]
    fn thread_c_parent_and_child_are_all_published_with_relation_metadata() {
        let mut parent = thread_fixture("parent", 10, "parent");
        parent["source"] = json!("cli");
        let mut child = thread_fixture("child", 20, "child");
        child["source"] = json!({"subAgent":{"thread_spawn":{
            "parent_thread_id":"parent","depth":1
        }}});
        let rows = vec![parent, child];

        let both_running = select_active_threads(
            terminal_cycle(rows.clone()),
            |candidate| -> Result<Vec<u8>, ()> {
                Ok(rollout_bytes(&[
                    json!({"type":"thread_context","model":candidate.id()}),
                    json!({"type":"task_started"}),
                ]))
            },
        );
        let ThreadCycleOutcome::Snapshots(both) = both_running else {
            panic!("both running threads must be published");
        };
        assert_eq!(
            both.iter()
                .map(|thread| thread.thread_id.as_str())
                .collect::<Vec<_>>(),
            ["child", "parent"]
        );
        assert!(both[0].is_subagent);
        assert_eq!(both[0].parent_thread_id.as_deref(), Some("parent"));
        assert_eq!(both[0].depth, Some(1));
        assert!(!both[1].is_subagent);

        let child_complete = select_active_threads(
            terminal_cycle(rows.clone()),
            |candidate| -> Result<Vec<u8>, ()> {
                Ok(if candidate.id() == "child" {
                    rollout_bytes(&[json!({"type":"task_complete"})])
                } else {
                    rollout_bytes(&[json!({"type":"task_started"})])
                })
            },
        );
        assert!(matches!(
            child_complete,
            ThreadCycleOutcome::Snapshots(threads)
                if threads.len() == 1 && threads[0].thread_id == "parent"
        ));

        let all_complete =
            select_active_threads(terminal_cycle(rows), |_| -> Result<Vec<u8>, ()> {
                Ok(rollout_bytes(&[json!({"type":"turn_aborted"})]))
            });
        assert_eq!(all_complete, ThreadCycleOutcome::NoThread);
    }

    #[test]
    fn thread_c_current_process_filter_excludes_stale_sessions_without_failure() {
        let rows = vec![
            thread_fixture("current", 20, "current"),
            thread_fixture("stale", 10, "stale"),
        ];
        let outcome = select_active_threads_where(
            terminal_cycle(rows.clone()),
            |candidate| candidate.id() == "current",
            |candidate| -> Result<Vec<u8>, ()> {
                assert_eq!(candidate.id(), "current");
                Ok(rollout_bytes(&[json!({"type":"task_started"})]))
            },
        );
        assert!(matches!(
            outcome,
            ThreadCycleOutcome::Snapshots(threads)
                if threads.len() == 1 && threads[0].thread_id == "current"
        ));

        assert_eq!(
            select_active_threads_where(
                terminal_cycle(rows),
                |_| false,
                |_| -> Result<Vec<u8>, ()> { panic!("excluded rollout must not be read") },
            ),
            ThreadCycleOutcome::NoThread
        );
    }

    #[test]
    fn thread_c_all_current_cycle_failure_classes_return_no_partial_snapshot() {
        let collecting = ThreadCycleAccumulator::new();
        assert_eq!(
            select_active_threads(collecting, |_| -> Result<Vec<u8>, ()> { unreachable!() }),
            ThreadCycleOutcome::CycleError
        );

        let mut bad_envelope = ThreadCycleAccumulator::new();
        assert_eq!(
            bad_envelope.accept_page(&json!({"data":null})),
            Err(ThreadContractError::InvalidEnvelope)
        );
        assert_eq!(
            select_active_threads(bad_envelope, |_| -> Result<Vec<u8>, ()> { unreachable!() }),
            ThreadCycleOutcome::CycleError
        );

        let mut cursor_cycle = ThreadCycleAccumulator::new();
        assert!(matches!(
            cursor_cycle.accept_page(&page(json!([]), Some(json!("A")))),
            Ok(PageAcceptance::NeedNextPage { .. })
        ));
        assert_eq!(
            cursor_cycle.accept_page(&page(json!([]), Some(json!("A")))),
            Err(ThreadContractError::InvalidEnvelope)
        );
        assert_eq!(
            select_active_threads(cursor_cycle, |_| -> Result<Vec<u8>, ()> { unreachable!() }),
            ThreadCycleOutcome::CycleError
        );

        for bytes in [b"{}".to_vec(), b"{\n".to_vec()] {
            let cycle = terminal_cycle(vec![thread_fixture("bad-rollout", 1, "bad-rollout")]);
            assert_eq!(
                select_active_threads(cycle, |_| -> Result<Vec<u8>, ()> { Ok(bytes.clone()) }),
                ThreadCycleOutcome::CycleError
            );
        }

        let cycle = terminal_cycle(vec![thread_fixture("rpc-failure", 1, "rpc-failure")]);
        assert_eq!(
            select_active_threads(cycle, |_| -> Result<Vec<u8>, ()> { Err(()) }),
            ThreadCycleOutcome::CycleError
        );
    }

    #[test]
    fn thread_c_title_name_preview_fixed_literal_matrix() {
        include!("thread_contract_slice3_title.inc.rs");
        include!("thread_contract_slice3_schema.inc.rs");
        include!("thread_contract_slice3_numeric.inc.rs");
    }

    #[test]
    fn thread_topology_accepts_null_root_and_missing_parent_orphan() {
        assert_eq!(
            validate_selected_thread_topology(&[ThreadTopologyNode {
                id: "root",
                parent_thread_id: None,
            }]),
            Ok(())
        );
        assert_eq!(
            validate_selected_thread_topology(&[ThreadTopologyNode {
                id: "orphan",
                parent_thread_id: Some("missing"),
            }]),
            Ok(())
        );
    }

    #[test]
    fn thread_topology_accepts_parent_and_siblings() {
        assert_eq!(
            validate_selected_thread_topology(&[
                ThreadTopologyNode {
                    id: "root",
                    parent_thread_id: None,
                },
                ThreadTopologyNode {
                    id: "child-a",
                    parent_thread_id: Some("root"),
                },
                ThreadTopologyNode {
                    id: "child-b",
                    parent_thread_id: Some("root"),
                },
            ]),
            Ok(())
        );
    }

    #[test]
    fn thread_topology_rejects_self_two_node_and_long_cycles() {
        assert_eq!(
            validate_selected_thread_topology(&[ThreadTopologyNode {
                id: "self",
                parent_thread_id: Some("self"),
            }]),
            Err(ThreadTopologyError::Cycle)
        );
        assert_eq!(
            validate_selected_thread_topology(&[
                ThreadTopologyNode {
                    id: "a",
                    parent_thread_id: Some("b"),
                },
                ThreadTopologyNode {
                    id: "b",
                    parent_thread_id: Some("a"),
                },
            ]),
            Err(ThreadTopologyError::Cycle)
        );
        assert_eq!(
            validate_selected_thread_topology(&[
                ThreadTopologyNode {
                    id: "a",
                    parent_thread_id: Some("b"),
                },
                ThreadTopologyNode {
                    id: "b",
                    parent_thread_id: Some("c"),
                },
                ThreadTopologyNode {
                    id: "c",
                    parent_thread_id: Some("a"),
                },
            ]),
            Err(ThreadTopologyError::Cycle)
        );
    }

    #[test]
    fn thread_topology_accepts_fixed_256_node_chain() {
        let ids = (0..256)
            .map(|index| format!("n{index:03}"))
            .collect::<Vec<_>>();
        let nodes = ids
            .iter()
            .enumerate()
            .map(|(index, id)| ThreadTopologyNode {
                id: id.as_str(),
                parent_thread_id: (index > 0).then(|| ids[index - 1].as_str()),
            })
            .collect::<Vec<_>>();
        assert_eq!(validate_selected_thread_topology(&nodes), Ok(()));
    }
}

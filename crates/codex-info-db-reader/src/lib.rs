//! Read-only account-history projection for the standalone REST process.
//!
//! The reader deliberately owns no writer types and has no path to Session
//! files, recorder locks, migrations, backups, or the root `codex_info` crate.
//! Every connection is opened with `SQLITE_OPEN_READ_ONLY`, then `query_only`
//! is enabled and read back before any product query is issued.

use codex_info_rest_contract::{
    is_valid_public_model_name, ContractError, PublicDetailedModelUsage, PublicDetails,
    PublicHistoryGap, PublicHistoryModelUsageV3, PublicHistoryObservation,
    PublicHistoryObservationV3, PublicHistoryPeriod, PublicHistorySample, PublicModelCostV3,
    PublicModelUsageV3, PublicQuota, PublicState, PublicThread, MAX_PUBLIC_MODELS_V3,
};
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_HISTORY_ROWS: usize = 31 * 24 * 60;
const HISTORY_WINDOW_SECONDS: i64 = 31 * 24 * 60 * 60;
pub const HISTORY_CANONICAL_SCHEMA_VERSION: i64 = 10;
const RESET_AT_TOLERANCE_SECONDS: i64 = 60;
const MOVING_RESET_GROUP_MAX_DRIFT_SECONDS: i64 = 5 * 60;
const MOVING_RESET_STEP_TOLERANCE_SECONDS: i64 = 180;
const MOVING_RESET_MIN_HORIZON_SECONDS: i64 = 86_400;
const MAX_ACTIVE_THREADS: usize = 256;
const MAX_ACTIVE_THREAD_JSON_BYTES: usize = 1024 * 1024;
const MAX_PUBLIC_UNIX_SECONDS: i64 = 253_402_300_799;
const MAX_LOGIN_ID_SCALARS: usize = 254;
// These are the distribution's established local estimate rates.  Keep the
// REST projection numerically identical to the root UI: durable history
// dollars are cumulative totals, while the public model fields are split into
// ordinary input, cached input, and output components from token totals.
const SOL_PRICE_PER_MILLION: (f64, f64, f64) = (5.0, 0.5, 30.0);
const TERRA_PRICE_PER_MILLION: (f64, f64, f64) = (2.0, 0.2, 12.0);
const LUNA_PRICE_PER_MILLION: (f64, f64, f64) = (0.2, 0.02, 1.2);
const ASTRA_PRICE_PER_MILLION: (f64, f64, f64, f64) = (10.0, 1.0, 12.5, 50.0);
const LOCAL_ESTIMATE_PRICE_VERSION: &str = "LOCAL_ESTIMATE_V1_2026-08-14";
const ASTRA_PRICE_VERSION: &str = "ASTRA_USER_2026-09-05";

#[derive(Clone, Debug, PartialEq)]
pub struct DbSnapshot {
    pub generation: u64,
    pub data_hash: String,
    /// A pending recorder range means the durable snapshot is usable but not
    /// complete.  REST exposes this as `state:error` while retaining values.
    pub has_pending_ranges: bool,
    pub details: PublicDetails,
    /// The v3 model projection is kept beside the legacy details DTO.  v1/v2
    /// intentionally expose only SOL/TERRA/LUNA, while v3 must retain ASTRA
    /// and additional durable model names.
    pub models_v3: Vec<PublicModelUsageV3>,
    /// The v3 history graph carries sidecar model totals and source quality;
    /// v1/v2 history remains the legacy nine-column projection.
    pub history_samples_v3: Vec<PublicHistoryObservationV3>,
    /// v2 keeps the same nullable legacy fields while exposing source
    /// provenance and unavailable observations.
    pub history_samples_v2: Vec<PublicHistoryObservation>,
}

/// Cheap, transactionally consistent publication marker used by REST to
/// decide whether its immutable in-memory snapshot needs rebuilding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DbChangeMarker {
    pub generation: u64,
    pub has_pending_ranges: bool,
}

/// Durable identity expected for one physical account database.
///
/// This deliberately mirrors the writer/locator identity fields without
/// depending on either crate, keeping the read-only reader boundary acyclic.
/// Every production reader is opened with this identity so a registry path
/// cannot be paired with another account's SQLite file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoragePartitionIdentity {
    pub schema_version: String,
    pub profile_scope_id: String,
    pub account_scope_id: String,
    pub storage_epoch: u64,
    pub partition_id: String,
}

impl StoragePartitionIdentity {
    fn validate(&self) -> Result<(), ReaderError> {
        fn lower_hex(value: &str, bytes: usize) -> bool {
            value.len() == bytes * 2
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
        if self.schema_version != "codex-info-account-db-v1"
            || !lower_hex(&self.profile_scope_id, 16)
            || !lower_hex(&self.account_scope_id, 32)
            || self.storage_epoch == 0
            || !lower_hex(&self.partition_id, 32)
        {
            return Err(ReaderError::InvalidValue(
                "storage partition identity is invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

/// One half-open lifecycle interval owned by a partition.  `start_at` is
/// inclusive and `end_at` is exclusive; an omitted bound is unbounded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadInterval {
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
}

impl ReadInterval {
    pub fn new(start_at: Option<i64>, end_at: Option<i64>) -> Result<Self, ReaderError> {
        if start_at.is_some_and(|value| value <= 0)
            || end_at.is_some_and(|value| value <= 0)
            || matches!((start_at, end_at), (Some(start), Some(end)) if start >= end)
        {
            return Err(ReaderError::InvalidValue(
                "read interval is not a non-empty positive half-open range".to_owned(),
            ));
        }
        Ok(Self { start_at, end_at })
    }

    fn contains(self, timestamp: i64) -> bool {
        self.start_at.is_none_or(|start| timestamp >= start)
            && self.end_at.is_none_or(|end| timestamp < end)
    }
}

/// A disjoint union of lifecycle intervals for one physical partition.
///
/// Multiple intervals are required because a partition may be selected again
/// after another account was active (A→B→A).  The set is sorted and overlap
/// checked at construction, so no row can be attributed twice within one
/// reader and no later activation is hidden by a single stale upper bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadIntervals(Vec<ReadInterval>);

impl Default for ReadIntervals {
    fn default() -> Self {
        Self(vec![ReadInterval {
            start_at: None,
            end_at: None,
        }])
    }
}

impl ReadIntervals {
    pub fn new(mut intervals: Vec<ReadInterval>) -> Result<Self, ReaderError> {
        if intervals.is_empty()
            || intervals.iter().any(|interval| {
                interval.start_at.is_some_and(|value| value <= 0)
                    || interval.end_at.is_some_and(|value| value <= 0)
                    || matches!(
                        (interval.start_at, interval.end_at),
                        (Some(start), Some(end)) if start >= end
                    )
            })
        {
            return Err(ReaderError::InvalidValue(
                "read intervals must contain non-empty positive ranges".to_owned(),
            ));
        }
        intervals.sort_by_key(|interval| {
            (
                interval.start_at.unwrap_or(i64::MIN),
                interval.end_at.unwrap_or(i64::MAX),
            )
        });
        for pair in intervals.windows(2) {
            let previous_end = pair[0].end_at.unwrap_or(i64::MAX);
            let next_start = pair[1].start_at.unwrap_or(i64::MIN);
            if next_start < previous_end {
                return Err(ReaderError::InvalidValue(
                    "read intervals overlap".to_owned(),
                ));
            }
        }
        Ok(Self(intervals))
    }

    pub fn unbounded() -> Self {
        Self::default()
    }

    pub fn intervals(&self) -> &[ReadInterval] {
        &self.0
    }

    fn contains(&self, timestamp: i64) -> bool {
        self.0
            .iter()
            .copied()
            .any(|interval| interval.contains(timestamp))
    }

    /// Canonical history timestamps identify a complete minute bucket rather
    /// than the exact second at which its source observation was accepted.
    /// Keep that bucket when any of its seconds belong to this account. Raw
    /// Session/task/thread timestamps continue to use exact `contains`.
    fn intersects_canonical_minute(&self, timestamp: i64) -> bool {
        let minute_start = timestamp.saturating_sub(timestamp.rem_euclid(60));
        let minute_end = minute_start.saturating_add(60);
        self.0.iter().any(|interval| {
            interval.end_at.is_none_or(|end| end > minute_start)
                && interval.start_at.is_none_or(|start| start < minute_end)
        })
    }

    fn interval_containing(&self, timestamp: i64) -> Option<ReadInterval> {
        self.0
            .iter()
            .copied()
            .find(|interval| interval.contains(timestamp))
    }
}

#[derive(Debug)]
pub enum ReaderError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Schema(String),
    InvalidValue(String),
    Contract(ContractError),
    TooManyRows(usize),
}

impl fmt::Display for ReaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "database read failed: {error}"),
            Self::Sqlite(error) => write!(formatter, "database read failed: {error}"),
            Self::Schema(error) => write!(formatter, "database schema is not readable: {error}"),
            Self::InvalidValue(error) => write!(formatter, "database value is invalid: {error}"),
            Self::Contract(error) => write!(formatter, "public projection is invalid: {error}"),
            Self::TooManyRows(count) => {
                write!(formatter, "history projection is too large: {count}")
            }
        }
    }
}

impl std::error::Error for ReaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Contract(error) => Some(error),
            Self::Schema(_) | Self::InvalidValue(_) | Self::TooManyRows(_) => None,
        }
    }
}

impl From<std::io::Error> for ReaderError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for ReaderError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<ContractError> for ReaderError {
    fn from(error: ContractError) -> Self {
        Self::Contract(error)
    }
}

#[derive(Clone, Debug)]
pub struct DbReader {
    path: PathBuf,
    expected_partition_identity: Option<StoragePartitionIdentity>,
    read_intervals: ReadIntervals,
}

impl DbReader {
    /// Construct a reader without creating, migrating, or repairing a file.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, ReaderError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(ReaderError::InvalidValue(
                "database path must be absolute".to_owned(),
            ));
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(ReaderError::InvalidValue(
                    "database path must be a regular file".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A recorder may create the database after the independent
                // REST unit starts.  Keep this read-only locator alive; the
                // first read reports unavailable and never creates the file.
            }
            Err(error) => return Err(error.into()),
        }
        Ok(Self {
            path: path.to_owned(),
            expected_partition_identity: None,
            read_intervals: ReadIntervals::unbounded(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Open one independent read-only connection and prove its connection
    /// mode before running any projection query.
    fn connection(&self) -> Result<Connection, ReaderError> {
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.execute_batch("PRAGMA query_only = ON;")?;
        let query_only: i64 = connection.query_row("PRAGMA query_only", [], |row| row.get(0))?;
        if query_only != 1 {
            return Err(ReaderError::Schema(
                "SQLite query_only read-back was not enabled".to_owned(),
            ));
        }
        if let Some(expected) = &self.expected_partition_identity {
            validate_storage_partition_identity(&connection, expected)?;
            validate_canonical_history_tables(&connection)?;
        }
        Ok(connection)
    }

    /// Exposed for the focused read-only gate and diagnostics.
    pub fn query_only_enabled(&self) -> Result<bool, ReaderError> {
        let connection = self.connection()?;
        Ok(connection.query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))? == 1)
    }

    /// Open an initialized account partition only after its durable
    /// `storage_partition` singleton exactly matches the registry identity.
    /// No writer-side schema migration or repair is attempted.
    pub fn open_partitioned<P: AsRef<Path>>(
        path: P,
        identity: &StoragePartitionIdentity,
    ) -> Result<Self, ReaderError> {
        Self::open_partitioned_with_intervals(path, identity, ReadIntervals::unbounded())
    }

    /// Open an initialized account partition with all lifecycle intervals
    /// owned by that physical partition.  Identity is checked on this probe
    /// and again for every later read-only connection.
    pub fn open_partitioned_with_intervals<P: AsRef<Path>>(
        path: P,
        identity: &StoragePartitionIdentity,
        read_intervals: ReadIntervals,
    ) -> Result<Self, ReaderError> {
        let mut reader = Self::open_with_intervals(path, read_intervals)?;
        reader.expected_partition_identity = Some(identity.clone());
        let _connection = reader.connection()?;
        Ok(reader)
    }

    /// Construct a fixture-compatible reader with an explicit disjoint union
    /// of lifecycle intervals.  `DbReader::open` remains the unbounded
    /// single-reader compatibility API.
    pub fn open_with_intervals<P: AsRef<Path>>(
        path: P,
        read_intervals: ReadIntervals,
    ) -> Result<Self, ReaderError> {
        let mut reader = Self::open(path)?;
        reader.read_intervals = read_intervals;
        Ok(reader)
    }

    pub fn read_intervals(&self) -> &ReadIntervals {
        &self.read_intervals
    }

    /// Read the display-only login ID from the identity-validated account
    /// partition. It is never used to select the database or authorize a
    /// read; the opaque storage identity remains the sole authority.
    pub fn partition_login_id(&self) -> Result<Option<String>, ReaderError> {
        let connection = self.connection()?;
        let login_id: Option<String> = connection.query_row(
            "SELECT login_id FROM storage_partition WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if login_id.as_deref().is_some_and(|value| {
            value.is_empty()
                || value.trim() != value
                || value.chars().count() > MAX_LOGIN_ID_SCALARS
                || value.chars().any(char::is_control)
        }) {
            return Err(ReaderError::InvalidValue(
                "storage partition login id is invalid".to_owned(),
            ));
        }
        Ok(login_id)
    }

    /// Read only the recorder's commit marker and unresolved-work state.
    /// Legacy databases without an explicit generation return `None`, which
    /// directs REST to rebuild and validate the complete snapshot.
    pub fn read_change_marker(&self) -> Result<Option<DbChangeMarker>, ReaderError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let Some(generation) = read_explicit_generation(&transaction)? else {
            transaction.commit()?;
            return Ok(None);
        };
        let (_, acquisition_degraded) = read_active_thread_snapshot_or_degraded_for_intervals(
            &transaction,
            &self.read_intervals,
        );
        let has_pending_ranges = read_pending_ranges(&transaction)? || acquisition_degraded;
        transaction.commit()?;
        Ok(Some(DbChangeMarker {
            generation,
            has_pending_ranges,
        }))
    }

    /// Build a completely new snapshot.  Domain-invalid rows are omitted from
    /// the projection; schema and transient SQLite errors still reject the
    /// candidate so the REST layer can retain the preceding generation.
    pub fn read_snapshot(&self) -> Result<DbSnapshot, ReaderError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let (threads, acquisition_degraded) = read_active_thread_snapshot_or_degraded_for_intervals(
            &transaction,
            &self.read_intervals,
        );
        let has_pending_ranges = read_pending_ranges(&transaction)? || acquisition_degraded;
        let history_is_canonical = self.expected_partition_identity.is_some();
        let raw = read_history(&transaction, history_is_canonical)?
            .into_iter()
            .filter(|row| {
                if history_is_canonical {
                    self.read_intervals
                        .intersects_canonical_minute(row.timestamp)
                } else {
                    self.read_intervals.contains(row.timestamp)
                }
            })
            .collect::<Vec<_>>();
        let generation = read_generation(&transaction, raw.iter().map(|row| row.timestamp))?;
        let task_evidence = read_task_activity_evidence_for_intervals(
            &transaction,
            !raw.is_empty(),
            &self.read_intervals,
        );
        let (details, models_v3, history_samples_v2, history_samples_v3) =
            build_details_for_intervals(
                &transaction,
                &raw,
                &threads,
                &task_evidence,
                &self.read_intervals,
                history_is_canonical,
            )?;
        details.validate()?;
        let mut hasher = Sha256::new();
        hasher.update(generation.to_be_bytes());
        hasher.update(
            serde_json::to_vec(&(
                &details,
                &models_v3,
                &history_samples_v2,
                &history_samples_v3,
            ))
            .map_err(|error| {
                ReaderError::InvalidValue(format!("snapshot serialization failed: {error}"))
            })?,
        );
        let data_hash = hex_lower(&hasher.finalize());
        transaction.commit()?;
        Ok(DbSnapshot {
            generation,
            data_hash,
            has_pending_ranges,
            details,
            models_v3,
            history_samples_v2,
            history_samples_v3,
        })
    }
}

fn validate_storage_partition_identity(
    connection: &Connection,
    expected: &StoragePartitionIdentity,
) -> Result<(), ReaderError> {
    expected.validate()?;
    if !table_exists(connection, "storage_partition")? {
        return Err(ReaderError::Schema(
            "storage_partition table is missing".to_owned(),
        ));
    }
    let count: i64 = connection.query_row("SELECT COUNT(*) FROM storage_partition", [], |row| {
        row.get(0)
    })?;
    if count != 1 {
        return Err(ReaderError::Schema(format!(
            "storage_partition singleton cardinality is {count}"
        )));
    }
    let actual: (i64, String, String, String, String, String) = connection.query_row(
        "SELECT singleton, schema_version, profile_scope_id, account_scope_id,
                storage_epoch, partition_id
         FROM storage_partition",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;
    let actual_epoch = actual.4.parse::<u64>().map_err(|_| {
        ReaderError::InvalidValue("storage_partition.storage_epoch is not a u64".to_owned())
    })?;
    if actual.0 != 1
        || actual.1 != expected.schema_version
        || actual.2 != expected.profile_scope_id
        || actual.3 != expected.account_scope_id
        || actual.5 != expected.partition_id
        || actual.4 != expected.storage_epoch.to_string()
        || actual_epoch != expected.storage_epoch
    {
        return Err(ReaderError::InvalidValue(
            "storage partition identity mismatch".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawSample {
    pub timestamp: i64,
    pub reset_at: i64,
    pub remaining_percent: Option<f64>,
    pub sol_dollars: f64,
    pub terra_dollars: f64,
    pub luna_dollars: f64,
    pub sol_tokens: u64,
    pub terra_tokens: u64,
    pub luna_tokens: u64,
}

/// One storage-canonical row together with the exact legacy row selected as
/// its source. The source key lets a one-time migration move only sidecars
/// that were attached to the retained observation instead of guessing from
/// reset aliases after canonicalization.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalStorageSample {
    pub sample: PublicHistorySample,
    pub source_timestamp: i64,
    pub source_reset_at: i64,
}

#[derive(Clone, Debug)]
struct StoredActiveThread {
    id: String,
    updated_at: i64,
    title: String,
    parent_thread_id: Option<String>,
    model: String,
    model_label: String,
    total_tokens: Option<u64>,
    context_usage_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    created_at: Option<i64>,
    last_user_message_at: Option<i64>,
    is_subagent: bool,
    depth: Option<i32>,
}

fn read_active_thread_snapshot_or_degraded_for_intervals(
    connection: &Connection,
    intervals: &ReadIntervals,
) -> (Vec<PublicThread>, bool) {
    read_active_thread_snapshot_for_intervals(connection, intervals)
        .unwrap_or_else(|_| (Vec::new(), true))
}

impl<'de> Deserialize<'de> for StoredActiveThread {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "id",
            "updated_at",
            "title",
            "parent_thread_id",
            "model",
            "model_label",
            "total_tokens",
            "context_usage_tokens",
            "context_window_tokens",
            "created_at",
            "last_user_message_at",
            "is_subagent",
            "depth",
        ];
        struct StoredActiveThreadVisitor;
        impl<'de> Visitor<'de> for StoredActiveThreadVisitor {
            type Value = StoredActiveThread;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("one active thread object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut id = None;
                let mut updated_at = None;
                let mut title = None;
                let mut parent_thread_id = None;
                let mut model = None;
                let mut model_label = None;
                let mut total_tokens = None;
                let mut context_usage_tokens = None;
                let mut context_window_tokens = None;
                let mut created_at = None;
                let mut last_user_message_at = None;
                let mut is_subagent = None;
                let mut depth = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "id" => {
                            if id.is_some() {
                                return Err(de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        "updated_at" => {
                            if updated_at.is_some() {
                                return Err(de::Error::duplicate_field("updated_at"));
                            }
                            updated_at = Some(map.next_value()?);
                        }
                        "title" => {
                            if title.is_some() {
                                return Err(de::Error::duplicate_field("title"));
                            }
                            title = Some(map.next_value()?);
                        }
                        "parent_thread_id" => {
                            if parent_thread_id.is_some() {
                                return Err(de::Error::duplicate_field("parent_thread_id"));
                            }
                            parent_thread_id = Some(map.next_value()?);
                        }
                        "model" => {
                            if model.is_some() {
                                return Err(de::Error::duplicate_field("model"));
                            }
                            model = Some(map.next_value()?);
                        }
                        "model_label" => {
                            if model_label.is_some() {
                                return Err(de::Error::duplicate_field("model_label"));
                            }
                            model_label = Some(map.next_value()?);
                        }
                        "total_tokens" => {
                            if total_tokens.is_some() {
                                return Err(de::Error::duplicate_field("total_tokens"));
                            }
                            total_tokens = Some(map.next_value()?);
                        }
                        "context_usage_tokens" => {
                            if context_usage_tokens.is_some() {
                                return Err(de::Error::duplicate_field("context_usage_tokens"));
                            }
                            context_usage_tokens = Some(map.next_value()?);
                        }
                        "context_window_tokens" => {
                            if context_window_tokens.is_some() {
                                return Err(de::Error::duplicate_field("context_window_tokens"));
                            }
                            context_window_tokens = Some(map.next_value()?);
                        }
                        "created_at" => {
                            if created_at.is_some() {
                                return Err(de::Error::duplicate_field("created_at"));
                            }
                            created_at = Some(map.next_value()?);
                        }
                        "last_user_message_at" => {
                            if last_user_message_at.is_some() {
                                return Err(de::Error::duplicate_field("last_user_message_at"));
                            }
                            last_user_message_at = Some(map.next_value()?);
                        }
                        "is_subagent" => {
                            if is_subagent.is_some() {
                                return Err(de::Error::duplicate_field("is_subagent"));
                            }
                            is_subagent = Some(map.next_value()?);
                        }
                        "depth" => {
                            if depth.is_some() {
                                return Err(de::Error::duplicate_field("depth"));
                            }
                            depth = Some(map.next_value()?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, FIELDS)),
                    }
                }
                Ok(StoredActiveThread {
                    id: id.ok_or_else(|| de::Error::missing_field("id"))?,
                    updated_at: updated_at.ok_or_else(|| de::Error::missing_field("updated_at"))?,
                    title: title.ok_or_else(|| de::Error::missing_field("title"))?,
                    parent_thread_id: parent_thread_id
                        .ok_or_else(|| de::Error::missing_field("parent_thread_id"))?,
                    model: model.ok_or_else(|| de::Error::missing_field("model"))?,
                    model_label: model_label
                        .ok_or_else(|| de::Error::missing_field("model_label"))?,
                    total_tokens: total_tokens
                        .ok_or_else(|| de::Error::missing_field("total_tokens"))?,
                    context_usage_tokens: context_usage_tokens
                        .ok_or_else(|| de::Error::missing_field("context_usage_tokens"))?,
                    context_window_tokens: context_window_tokens
                        .ok_or_else(|| de::Error::missing_field("context_window_tokens"))?,
                    created_at: created_at.ok_or_else(|| de::Error::missing_field("created_at"))?,
                    last_user_message_at: last_user_message_at
                        .ok_or_else(|| de::Error::missing_field("last_user_message_at"))?,
                    is_subagent: is_subagent
                        .ok_or_else(|| de::Error::missing_field("is_subagent"))?,
                    depth: depth.ok_or_else(|| de::Error::missing_field("depth"))?,
                })
            }
        }
        deserializer.deserialize_map(StoredActiveThreadVisitor)
    }
}

fn active_thread_table_shape(connection: &Connection) -> Result<bool, ReaderError> {
    let mut statement = connection.prepare(
        "SELECT name, type, pk FROM pragma_table_info('active_thread_snapshot') ORDER BY cid",
    )?;
    let actual = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let current = vec![
        ("singleton".to_owned(), "INTEGER".to_owned(), 1),
        ("observed_at".to_owned(), "INTEGER".to_owned(), 0),
        ("threads_json".to_owned(), "TEXT".to_owned(), 0),
        ("acquisition_degraded".to_owned(), "INTEGER".to_owned(), 0),
    ];
    let legacy = current[..3].to_vec();
    if actual == current {
        Ok(true)
    } else if actual == legacy {
        Ok(false)
    } else {
        Err(ReaderError::Schema(
            "active_thread_snapshot table schema is invalid".to_owned(),
        ))
    }
}

/// Reads the optional singleton publication row.  Missing tables and rows are
/// the legacy empty state; a present but malformed candidate rejects the
/// whole read so REST can retain its last-good pair.
fn read_active_thread_snapshot_for_intervals(
    connection: &Connection,
    intervals: &ReadIntervals,
) -> Result<(Vec<PublicThread>, bool), ReaderError> {
    if !table_exists(connection, "active_thread_snapshot")? {
        return Ok((Vec::new(), false));
    }
    let has_degraded = active_thread_table_shape(connection)?;
    let count: i64 =
        connection.query_row("SELECT COUNT(*) FROM active_thread_snapshot", [], |row| {
            row.get(0)
        })?;
    if count > 1 {
        return Err(ReaderError::Schema(
            "active_thread_snapshot singleton cardinality is invalid".to_owned(),
        ));
    }
    let query = if has_degraded {
        "SELECT singleton, observed_at, threads_json, acquisition_degraded
         FROM active_thread_snapshot WHERE singleton = 1"
    } else {
        "SELECT singleton, observed_at, threads_json, 0
         FROM active_thread_snapshot WHERE singleton = 1"
    };
    let Some((singleton, observed_at, threads_json, acquisition_degraded)) = connection
        .query_row(query, [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .optional()?
    else {
        return Ok((Vec::new(), false));
    };
    if singleton != 1 {
        return Err(ReaderError::Schema(
            "active_thread_snapshot singleton key is invalid".to_owned(),
        ));
    }
    if !(1..=MAX_PUBLIC_UNIX_SECONDS).contains(&observed_at) {
        return Err(ReaderError::InvalidValue(
            "active thread snapshot observed_at is invalid".to_owned(),
        ));
    }
    // The singleton's observation time is the authority for the entire
    // active-thread set.  A snapshot captured outside this partition's owned
    // lifecycle intervals must not be reused merely because an individual
    // row's updated_at happens to fall inside an older interval.
    if !intervals.contains(observed_at) {
        return Ok((Vec::new(), false));
    }
    if !matches!(acquisition_degraded, 0 | 1) {
        return Err(ReaderError::InvalidValue(
            "active thread snapshot acquisition state is invalid".to_owned(),
        ));
    }
    if threads_json.len() > MAX_ACTIVE_THREAD_JSON_BYTES || threads_json.len() < 2 {
        return Err(ReaderError::InvalidValue(
            "active thread snapshot JSON is outside its size bound".to_owned(),
        ));
    }
    let mut stored: Vec<StoredActiveThread> =
        serde_json::from_str(&threads_json).map_err(|error| {
            ReaderError::InvalidValue(format!("active thread JSON is invalid: {error}"))
        })?;
    if stored.len() > MAX_ACTIVE_THREADS {
        return Err(ReaderError::TooManyRows(stored.len()));
    }
    let mut ids = HashSet::with_capacity(stored.len());
    for thread in &stored {
        if !ids.insert(thread.id.as_str()) {
            return Err(ReaderError::InvalidValue(
                "active thread snapshot contains duplicate ids".to_owned(),
            ));
        }
        if !(1..=MAX_PUBLIC_UNIX_SECONDS).contains(&thread.updated_at) {
            return Err(ReaderError::InvalidValue(
                "active thread updated_at is invalid".to_owned(),
            ));
        }
    }
    stored.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    let threads = stored
        .into_iter()
        .map(|thread| PublicThread {
            id: thread.id,
            title: thread.title,
            parent_thread_id: thread.parent_thread_id,
            model: thread.model,
            model_label: thread.model_label,
            total_tokens: thread.total_tokens,
            context_usage_tokens: thread.context_usage_tokens,
            context_window_tokens: thread.context_window_tokens,
            created_at: thread.created_at,
            last_user_message_at: thread.last_user_message_at,
            is_subagent: thread.is_subagent,
            depth: thread.depth,
        })
        .collect::<Vec<_>>();
    let validation = PublicDetails {
        active_thread_count: threads.len() as u64,
        threads: threads.clone(),
        ..PublicDetails::default()
    };
    validation.validate()?;
    Ok((threads, acquisition_degraded == 1))
}

#[derive(Clone, Debug)]
struct ResetGroup {
    canonical_reset_at: i64,
    start: i64,
    rows: Vec<SourcedRawSample>,
}

#[derive(Clone, Debug)]
struct SourcedRawSample {
    row: RawSample,
    source_timestamp: i64,
    source_reset_at: i64,
}

impl std::ops::Deref for SourcedRawSample {
    type Target = RawSample;

    fn deref(&self) -> &Self::Target {
        &self.row
    }
}

impl std::ops::DerefMut for SourcedRawSample {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.row
    }
}

fn read_history(
    connection: &Connection,
    _history_is_canonical: bool,
) -> Result<Vec<RawSample>, ReaderError> {
    let table = "usage_history";
    if !table_exists(connection, table)? {
        return Err(ReaderError::Schema(format!("{table} table is missing")));
    }
    let query = format!(
        "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
         FROM {table} ORDER BY reset_at ASC, timestamp ASC"
    );
    let mut statement = connection.prepare(&query)?;
    let rows = statement.query_map([], raw_sample_from_row)?;
    let mut values = Vec::new();
    for row in rows {
        let Some(value) = row? else {
            // Domain-invalid content is isolated to its own row. SQLite and
            // schema errors still propagate through `row?`, but one malformed
            // observation must not hide unrelated durable observations.
            continue;
        };
        values.push(value);
    }
    Ok(values)
}

fn validate_canonical_history_tables(connection: &Connection) -> Result<(), ReaderError> {
    let schema_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema_version != HISTORY_CANONICAL_SCHEMA_VERSION {
        return Err(ReaderError::Schema(
            "account history canonical migration is incomplete".to_owned(),
        ));
    }
    if !table_exists(connection, "usage_history")?
        || !table_exists(connection, "usage_model_history")?
    {
        return Err(ReaderError::Schema(
            "account canonical history tables are missing".to_owned(),
        ));
    }
    let unique_indexes: i64 = connection.query_row(
        "SELECT
             EXISTS(
                 SELECT 1 FROM pragma_index_list('usage_history')
                 WHERE name='usage_history_canonical_timestamp_idx' AND \"unique\"=1
             )
             + EXISTS(
                 SELECT 1 FROM pragma_index_list('usage_model_history')
                 WHERE name='usage_model_history_canonical_timestamp_model_idx' AND \"unique\"=1
             )",
        [],
        |row| row.get(0),
    )?;
    if unique_indexes != 2 {
        return Err(ReaderError::Schema(
            "account canonical history uniqueness constraints are missing".to_owned(),
        ));
    }
    let trigger_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE type='trigger' AND name IN (
             'usage_history_canonical_insert_guard',
             'usage_history_canonical_update_guard',
             'usage_model_history_canonical_insert_guard',
             'usage_model_history_canonical_update_guard',
             'durable_history_observation_insert_guard',
             'durable_history_observation_update_guard',
             'usage_history_sidecar_update_guard',
             'usage_history_sidecar_delete_guard'
         )",
        [],
        |row| row.get(0),
    )?;
    if trigger_count != 8 {
        return Err(ReaderError::Schema(
            "account canonical history guard set is incomplete".to_owned(),
        ));
    }
    Ok(())
}

fn raw_sample_from_row(row: &Row<'_>) -> rusqlite::Result<Option<RawSample>> {
    let timestamp: i64 = row.get(0)?;
    let reset_at: i64 = row.get(1)?;
    let remaining_percent: Option<f64> = row.get(2)?;
    let sol_dollars: f64 = row.get(3)?;
    let terra_dollars: f64 = row.get(4)?;
    let luna_dollars: f64 = row.get(5)?;
    let Some(sol_tokens) = row.get::<_, i64>(6)?.try_into().ok() else {
        return Ok(None);
    };
    let Some(terra_tokens) = row.get::<_, i64>(7)?.try_into().ok() else {
        return Ok(None);
    };
    let Some(luna_tokens) = row.get::<_, i64>(8)?.try_into().ok() else {
        return Ok(None);
    };
    if !valid_public_timestamp(timestamp)
        || !valid_public_timestamp(reset_at)
        || timestamp > reset_at
    {
        return Ok(None);
    }
    for value in [sol_dollars, terra_dollars, luna_dollars] {
        if !value.is_finite() || value < 0.0 {
            return Ok(None);
        }
    }
    if remaining_percent.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
    {
        return Ok(None);
    }
    Ok(Some(RawSample {
        timestamp,
        reset_at,
        remaining_percent,
        sol_dollars,
        terra_dollars,
        luna_dollars,
        sol_tokens,
        terra_tokens,
        luna_tokens,
    }))
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, ReaderError> {
    Ok(connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
         )",
        [table],
        |row| row.get(0),
    )?)
}

fn read_pending_ranges(connection: &Connection) -> Result<bool, ReaderError> {
    if !table_exists(connection, "session_pending_ranges")? {
        return Ok(false);
    }
    let query = if table_has_column(connection, "session_pending_ranges", "complete")? {
        "SELECT EXISTS(SELECT 1 FROM session_pending_ranges WHERE complete=0)"
    } else {
        // A legacy table has no durable completion proof, so its rows remain
        // conservatively pending while the complete snapshot is still read.
        "SELECT EXISTS(SELECT 1 FROM session_pending_ranges)"
    };
    Ok(connection.query_row(query, [], |row| row.get(0))?)
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, ReaderError> {
    let query = match table {
        "session_model_totals" => "PRAGMA table_info(session_model_totals)",
        "usage_model_history" => "PRAGMA table_info(usage_model_history)",
        "session_pending_ranges" => "PRAGMA table_info(session_pending_ranges)",
        _ => return Ok(false),
    };
    let mut statement = connection.prepare(query)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_generation<I>(connection: &Connection, fallback_timestamps: I) -> Result<u64, ReaderError>
where
    I: Iterator<Item = i64>,
{
    if let Some(generation) = read_explicit_generation(connection)? {
        return Ok(generation);
    }
    Ok(fallback_timestamps
        .max()
        .unwrap_or(0)
        .try_into()
        .unwrap_or(0))
}

fn read_explicit_generation(connection: &Connection) -> Result<Option<u64>, ReaderError> {
    if table_exists(connection, "collection_generation")? {
        let value: Option<String> = connection
            .query_row(
                "SELECT data_generation FROM collection_generation WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(value) = value {
            return parse_u64(&value, "collection_generation.data_generation").map(Some);
        }
    }
    if table_exists(connection, "durable_state")? {
        let value: Option<i64> = connection
            .query_row(
                "SELECT data_generation FROM durable_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(value) = value {
            return u64::try_from(value).map(Some).map_err(|_| {
                ReaderError::InvalidValue("durable_state generation is negative".to_owned())
            });
        }
    }
    Ok(None)
}

fn parse_u64(value: &str, field: &str) -> Result<u64, ReaderError> {
    value
        .parse::<u64>()
        .map_err(|_| ReaderError::InvalidValue(format!("{field} is not a canonical u64")))
}

type DetailsBuild = (
    PublicDetails,
    Vec<PublicModelUsageV3>,
    Vec<PublicHistoryObservation>,
    Vec<PublicHistoryObservationV3>,
);

fn build_details_for_intervals(
    connection: &Connection,
    raw: &[RawSample],
    threads: &[PublicThread],
    task_evidence: &TaskActivityEvidence,
    intervals: &ReadIntervals,
    history_is_canonical: bool,
) -> Result<DetailsBuild, ReaderError> {
    if raw.is_empty() {
        let details = PublicDetails {
            active_thread_count: threads.len() as u64,
            threads: threads.to_vec(),
            ..PublicDetails::default()
        };
        return Ok((details, Vec::new(), Vec::new(), Vec::new()));
    }
    let observed_at = raw.iter().map(|row| row.timestamp).max().unwrap_or(0);
    let (current_reset_at, window_seconds) = read_collection_config(connection)?;
    let cutoff = observed_at.saturating_sub(HISTORY_WINDOW_SECONDS);
    let samples = if history_is_canonical {
        if raw.len() > MAX_HISTORY_ROWS {
            return Err(ReaderError::TooManyRows(raw.len()));
        }
        raw.iter()
            .filter(|row| row.timestamp > cutoff && row.timestamp <= observed_at)
            .map(public_sample_from_raw)
            .collect::<Vec<_>>()
    } else {
        canonicalize_history_for_public_window(raw, current_reset_at, window_seconds)?
    };
    let mut periods = history_periods(&samples, observed_at, current_reset_at, window_seconds);
    clip_history_periods(&mut periods, intervals);
    let quota = latest_quota_row(raw, current_reset_at)
        .and_then(|row| {
            row.remaining_percent.map(|remaining_percent| PublicQuota {
                remaining_percent,
                reset_at: row.reset_at,
                window_seconds,
                monthly: false,
            })
        })
        .filter(|quota| quota.window_seconds > 0);
    let model_projection = read_model_projection_for_intervals(connection, intervals)?;
    let models = model_projection.v1;
    let history_samples_v3 = read_history_projection_for_intervals(
        connection,
        &samples,
        &mut periods,
        observed_at,
        cutoff,
        task_evidence,
        intervals,
    )?;
    clip_history_periods(&mut periods, intervals);
    assign_history_period_labels(&mut periods);
    let history_samples_v2 = history_observations_v2(&samples, &history_samples_v3);
    let history_samples_v1 = history_samples_v1(&history_samples_v2);
    let estimated_cost_label = format_estimated_cost(&models);
    let mut gaps = read_confirmed_gaps_for_intervals(connection, intervals)?
        .into_iter()
        .filter_map(|gap| {
            let period = periods.iter().find(|period| {
                gap.reset_at.abs_diff(period.reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
                    && gap.start_at >= period.start_at
                    && gap.end_at <= period.end_at
            })?;
            Some(PublicHistoryGap {
                reset_at: period.reset_at,
                ..gap
            })
        })
        .collect::<Vec<_>>();
    gaps.sort_by_key(|gap| (gap.reset_at, gap.start_at, gap.end_at, gap.gap_id.clone()));
    let mut non_overlapping_gaps = Vec::with_capacity(gaps.len());
    for gap in gaps {
        if non_overlapping_gaps
            .last()
            .is_some_and(|previous: &PublicHistoryGap| {
                previous.reset_at == gap.reset_at && gap.start_at <= previous.end_at
            })
        {
            continue;
        }
        non_overlapping_gaps.push(gap);
    }
    let gaps = non_overlapping_gaps;
    let state = if samples.is_empty() {
        PublicState::Initializing
    } else {
        PublicState::Ready
    };
    Ok((
        PublicDetails {
            state,
            observed_at: Some(observed_at),
            authenticated: !samples.is_empty(),
            plan_label: None,
            quota,
            models,
            active_thread_count: threads.len() as u64,
            history_periods: periods,
            // v1 has no nullable model fields or provenance field, so it
            // cannot truthfully carry metadata-only rows. Publish only exact
            // model vectors admitted by the source-aware v2/v3 projection.
            history_samples: history_samples_v1,
            history_gaps: gaps,
            threads: threads.to_vec(),
            estimated_cost_label,
        },
        model_projection.v3,
        history_samples_v2,
        history_samples_v3,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryModelSource {
    Confirmed,
    ReconstructedFromSession,
    Unavailable,
    LegacyUnknown,
}

impl HistoryModelSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::ReconstructedFromSession => "reconstructed-from-session",
            Self::Unavailable => "unavailable",
            Self::LegacyUnknown => "legacy-unknown",
        }
    }
}

#[derive(Clone, Debug)]
struct StoredHistoryObservation {
    timestamp: i64,
    reset_at: i64,
    remaining_percent: Option<f64>,
    model_source: HistoryModelSource,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TaskSourceKey {
    root_identity: String,
    relative_path: String,
    file_device: String,
    file_inode: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TaskRangeKey {
    source: TaskSourceKey,
    prefix_generation: String,
    start_offset: u64,
    end_offset: u64,
    record_sha256: String,
}

#[derive(Clone, Debug)]
struct TaskTransition {
    event_index: u64,
    timestamp: i64,
    running: bool,
}

#[derive(Clone, Debug, Default)]
struct TaskActivityEvidence {
    complete: bool,
    transitions: BTreeMap<TaskSourceKey, Vec<TaskTransition>>,
    source_observed_through: BTreeMap<TaskSourceKey, i64>,
    source_complete: BTreeMap<TaskSourceKey, bool>,
}

fn read_task_activity_evidence(connection: &Connection, has_history: bool) -> TaskActivityEvidence {
    read_task_activity_evidence_inner(connection, has_history).unwrap_or_else(|_| {
        TaskActivityEvidence {
            complete: !has_history,
            ..TaskActivityEvidence::default()
        }
    })
}

fn read_task_activity_evidence_for_intervals(
    connection: &Connection,
    has_history: bool,
    intervals: &ReadIntervals,
) -> TaskActivityEvidence {
    let mut evidence = read_task_activity_evidence(connection, has_history);
    evidence.transitions.retain(|_, events| {
        events.retain(|event| intervals.contains(event.timestamp));
        !events.is_empty()
    });
    evidence
        .source_observed_through
        .retain(|_, timestamp| intervals.contains(*timestamp));
    evidence
}

fn read_task_activity_evidence_inner(
    connection: &Connection,
    has_history: bool,
) -> Result<TaskActivityEvidence, ReaderError> {
    for table in [
        "session_task_events",
        "session_task_indexed_ranges",
        "session_ranges",
        "session_checkpoints",
        "session_pending_ranges",
    ] {
        if !table_exists(connection, table)? {
            return Ok(TaskActivityEvidence {
                complete: !has_history,
                ..TaskActivityEvidence::default()
            });
        }
    }

    let mut complete = true;
    let mut source_complete = BTreeMap::<TaskSourceKey, bool>::new();
    let mut checkpoint_sources = BTreeSet::new();
    let mut indexed_ranges = BTreeSet::new();
    let mut indexed_spans = BTreeMap::<TaskSourceKey, BTreeMap<String, Vec<(u64, u64)>>>::new();
    let mut statement = connection.prepare(
        "SELECT root_identity, relative_path, file_device, file_inode,
                prefix_generation, start_offset, end_offset, record_sha256
         FROM session_task_indexed_ranges",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let range = task_range_from_row(row)?;
        source_complete.entry(range.source.clone()).or_insert(true);
        if !indexed_ranges.insert(range.clone()) {
            return Err(ReaderError::InvalidValue(
                "duplicate indexed task range".to_owned(),
            ));
        }
        indexed_spans
            .entry(range.source)
            .or_default()
            .entry(range.prefix_generation)
            .or_default()
            .push((range.start_offset, range.end_offset));
    }
    for spans_by_prefix in indexed_spans.values_mut() {
        for spans in spans_by_prefix.values_mut() {
            spans.sort_unstable();
        }
    }

    let mut persisted_ranges = BTreeSet::new();
    let mut statement = connection.prepare(
        "SELECT root_identity, relative_path, file_device, file_inode,
                prefix_generation, start_offset, end_offset, record_sha256
         FROM session_ranges",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let range = task_range_from_row(row)?;
        if !persisted_ranges.insert(range.clone()) {
            return Err(ReaderError::InvalidValue(
                "duplicate persisted session range".to_owned(),
            ));
        }
        // An indexed super-range proves that every byte of the persisted
        // sub-range was parsed. Each range authenticates its own byte span,
        // so their SHA-256 values are intentionally not expected to be equal.
        let covered = indexed_spans
            .get(&range.source)
            .and_then(|spans| spans.get(&range.prefix_generation))
            .is_some_and(|spans| {
                spans
                    .iter()
                    .take_while(|(start, _)| *start <= range.start_offset)
                    .any(|(_, end)| *end >= range.end_offset)
            });
        if !covered {
            complete = false;
            source_complete.insert(range.source.clone(), false);
        } else {
            source_complete.entry(range.source.clone()).or_insert(true);
        }
    }

    let mut checkpoint_count = 0usize;
    let mut statement = connection.prepare(
        "SELECT root_identity, relative_path, file_device, file_inode,
                committed_offset, discard_until_lf, prefix_generation
         FROM session_checkpoints",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        checkpoint_count = checkpoint_count.saturating_add(1);
        let source = task_source_from_row(row, 0, 1, 2, 3)?;
        source_complete.entry(source.clone()).or_insert(true);
        let committed_offset = u64::try_from(row.get::<_, i64>(4)?).map_err(|_| {
            ReaderError::InvalidValue("negative session checkpoint offset".to_owned())
        })?;
        if row.get::<_, i64>(5)? != 0 {
            complete = false;
            source_complete.insert(source.clone(), false);
            continue;
        }
        let prefix_generation = task_hex_text(row, 6, 32, "prefix_generation")?;
        let mut cursor = 0u64;
        let mut checkpoint_complete = true;
        if let Some(spans) = indexed_spans
            .get(&source)
            .and_then(|spans| spans.get(&prefix_generation))
        {
            for &(start_offset, end_offset) in spans {
                if start_offset >= committed_offset {
                    break;
                }
                if start_offset > cursor || end_offset > committed_offset {
                    complete = false;
                    checkpoint_complete = false;
                    break;
                }
                cursor = end_offset;
            }
        }
        if cursor != committed_offset {
            complete = false;
            checkpoint_complete = false;
        }
        if !checkpoint_complete {
            source_complete.insert(source.clone(), false);
        } else {
            checkpoint_sources.insert(source);
        }
    }

    let mut statement = connection.prepare(
        "SELECT root_identity, relative_path, file_device, file_inode, complete
         FROM session_pending_ranges",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let source = task_source_from_row(row, 0, 1, 2, 3)?;
        source_complete.entry(source.clone()).or_insert(true);
        if row.get::<_, i64>(4)? != 1 {
            complete = false;
            source_complete.insert(source, false);
        }
    }

    let mut transitions = BTreeMap::<TaskSourceKey, Vec<TaskTransition>>::new();
    let mut source_observed_through = BTreeMap::<TaskSourceKey, i64>::new();
    let mut event_keys = BTreeSet::new();
    let mut statement = connection.prepare(
        "SELECT root_identity, relative_path, file_device, file_inode,
                prefix_generation, start_offset, end_offset, record_sha256,
                event_index, timestamp, running
         FROM session_task_events
         ORDER BY timestamp, event_index",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let source = task_source_from_row(row, 0, 1, 2, 3)?;
        source_complete.entry(source.clone()).or_insert(true);
        let range = TaskRangeKey {
            source: source.clone(),
            prefix_generation: task_hex_text(row, 4, 32, "prefix_generation")?,
            start_offset: task_non_negative_u64(row, 5, "start_offset")?,
            end_offset: task_positive_u64(row, 6, "end_offset")?,
            record_sha256: task_hex_text(row, 7, 64, "record_sha256")?,
        };
        if range.end_offset <= range.start_offset || !indexed_ranges.contains(&range) {
            complete = false;
            source_complete.insert(source, false);
            continue;
        }
        let event_index = task_non_negative_u64(row, 8, "event_index")?;
        if !event_keys.insert((range.clone(), event_index)) {
            return Err(ReaderError::InvalidValue(
                "duplicate task event identity".to_owned(),
            ));
        }
        let timestamp = row.get::<_, i64>(9)?;
        if !valid_public_timestamp(timestamp) {
            return Err(ReaderError::InvalidValue(
                "task event timestamp is outside the public domain".to_owned(),
            ));
        }
        let running = match row.get::<_, i64>(10)? {
            0 => false,
            1 => true,
            _ => {
                return Err(ReaderError::InvalidValue(
                    "task event running flag is not boolean".to_owned(),
                ));
            }
        };
        transitions
            .entry(source.clone())
            .or_default()
            .push(TaskTransition {
                event_index,
                timestamp,
                running,
            });
        source_observed_through
            .entry(source)
            .and_modify(|observed| *observed = (*observed).max(timestamp))
            .or_insert(timestamp);
    }

    // A task_started record without a matching completion proves activity
    // only as far as this same source was independently observed.  Token
    // snapshots provide that finite source-local horizon; they must never
    // turn one abandoned Session into an activity veto for every later
    // history period.
    if table_exists(connection, "session_events")? {
        let mut statement = connection.prepare(
            "SELECT root_identity, relative_path, file_device, file_inode,
                    MAX(timestamp)
             FROM session_events
             GROUP BY root_identity, relative_path, file_device, file_inode",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let source = task_source_from_row(row, 0, 1, 2, 3)?;
            let timestamp = row.get::<_, i64>(4)?;
            if !valid_public_timestamp(timestamp) {
                return Err(ReaderError::InvalidValue(
                    "task source observation horizon is outside the public domain".to_owned(),
                ));
            }
            source_observed_through
                .entry(source)
                .and_modify(|observed| *observed = (*observed).max(timestamp))
                .or_insert(timestamp);
        }
    }

    for source in source_complete.keys().cloned().collect::<Vec<_>>() {
        if !checkpoint_sources.contains(&source) {
            complete = false;
            source_complete.insert(source, false);
        }
    }

    if has_history && checkpoint_count == 0 && persisted_ranges.is_empty() && transitions.is_empty()
    {
        complete = false;
    }
    for source_events in transitions.values_mut() {
        source_events.sort_by_key(|event| (event.timestamp, event.event_index));
        let mut active = false;
        for event in source_events.iter() {
            if event.running {
                if active {
                    complete = false;
                }
                active = true;
            } else {
                if !active {
                    complete = false;
                }
                active = false;
            }
        }
        if active {
            complete = false;
        }
    }
    Ok(TaskActivityEvidence {
        complete,
        transitions,
        source_observed_through,
        source_complete,
    })
}

fn task_source_from_row(
    row: &Row<'_>,
    root_index: usize,
    path_index: usize,
    device_index: usize,
    inode_index: usize,
) -> Result<TaskSourceKey, ReaderError> {
    let root_identity = sql_text(row, root_index)
        .filter(|value| !value.is_empty() && value.len() <= 256 && value.is_ascii())
        .ok_or_else(|| ReaderError::InvalidValue("invalid task root_identity".to_owned()))?;
    let relative_path = sql_text(row, path_index).ok_or_else(|| {
        ReaderError::InvalidValue("task source relative_path is missing".to_owned())
    })?;
    let relative = Path::new(&relative_path);
    if relative_path.is_empty()
        || relative_path.len() > 4096
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(ReaderError::InvalidValue(
            "task source relative_path is invalid".to_owned(),
        ));
    }
    let file_device = task_decimal_text(row, device_index, "file_device")?;
    let file_inode = task_decimal_text(row, inode_index, "file_inode")?;
    Ok(TaskSourceKey {
        root_identity,
        relative_path,
        file_device,
        file_inode,
    })
}

fn task_range_from_row(row: &Row<'_>) -> Result<TaskRangeKey, ReaderError> {
    let source = task_source_from_row(row, 0, 1, 2, 3)?;
    let start_offset = task_non_negative_u64(row, 5, "start_offset")?;
    let end_offset = task_positive_u64(row, 6, "end_offset")?;
    if end_offset <= start_offset {
        return Err(ReaderError::InvalidValue(
            "task range end does not follow start".to_owned(),
        ));
    }
    Ok(TaskRangeKey {
        source,
        prefix_generation: task_hex_text(row, 4, 32, "prefix_generation")?,
        start_offset,
        end_offset,
        record_sha256: task_hex_text(row, 7, 64, "record_sha256")?,
    })
}

fn task_hex_text(
    row: &Row<'_>,
    index: usize,
    length: usize,
    field: &str,
) -> Result<String, ReaderError> {
    let value = sql_text(row, index)
        .filter(|value| valid_lower_hex(value, length))
        .ok_or_else(|| ReaderError::InvalidValue(format!("invalid task {field}")))?;
    Ok(value)
}

fn task_decimal_text(row: &Row<'_>, index: usize, field: &str) -> Result<String, ReaderError> {
    let value = sql_text(row, index)
        .and_then(|value| canonical_timeline_u64(&value).map(|_| value))
        .ok_or_else(|| ReaderError::InvalidValue(format!("invalid task {field}")))?;
    Ok(value)
}

fn task_non_negative_u64(row: &Row<'_>, index: usize, field: &str) -> Result<u64, ReaderError> {
    u64::try_from(row.get::<_, i64>(index)?)
        .map_err(|_| ReaderError::InvalidValue(format!("negative task {field}")))
}

fn task_positive_u64(row: &Row<'_>, index: usize, field: &str) -> Result<u64, ReaderError> {
    let value = task_non_negative_u64(row, index, field)?;
    (value > 0)
        .then_some(value)
        .ok_or_else(|| ReaderError::InvalidValue(format!("zero task {field}")))
}

#[derive(Clone, Debug)]
struct HistoryModelGroup {
    totals: BTreeMap<String, RawModelTotal>,
    complete: bool,
    complete_known: bool,
    valid: bool,
}

impl Default for HistoryModelGroup {
    fn default() -> Self {
        Self {
            totals: BTreeMap::new(),
            complete: false,
            complete_known: false,
            valid: true,
        }
    }
}

impl HistoryModelGroup {
    fn model_set_complete(&self) -> bool {
        self.valid && self.complete_known && self.complete
    }
}

fn valid_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_timeline_u64(value: &str) -> Option<u64> {
    let parsed = value.parse::<u64>().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

/// Build the v3 graph rows from the durable observation JSON and model-history
/// sidecar.  The legacy `usage_history` row remains the v1 source of truth for
/// period ownership and the three displayed dollar columns; sidecar faults
/// are isolated to their one timestamp/model group.
fn read_history_projection_for_intervals(
    connection: &Connection,
    samples: &[PublicHistorySample],
    periods: &mut [PublicHistoryPeriod],
    observed_at: i64,
    cutoff: i64,
    task_evidence: &TaskActivityEvidence,
    intervals: &ReadIntervals,
) -> Result<Vec<PublicHistoryObservationV3>, ReaderError> {
    let observations =
        read_stored_history_observations_for_intervals(connection, cutoff, observed_at, intervals)?;
    let model_groups =
        read_history_model_groups_for_intervals(connection, cutoff, observed_at, intervals)?;

    // Durable observation provenance and model totals are authoritative only
    // for their exact (reset_at, timestamp) key. A timestamp-only or
    // reset-tolerance join can attach a sidecar from another quota period.
    let mut observations_by_key = BTreeMap::<(i64, i64), &StoredHistoryObservation>::new();
    for observation in &observations {
        observations_by_key.insert((observation.reset_at, observation.timestamp), observation);
    }

    // Unavailable quota observations are not present in usage_history. The
    // root v2/v3 projection extends the containing period to those minutes so
    // the graph can retain the observed quota/time metadata without
    // fabricating model values or classifying the point as idle.
    for observation in observations
        .iter()
        .filter(|observation| observation.model_source == HistoryModelSource::Unavailable)
    {
        if let Some(period) = periods.iter_mut().find(|period| {
            observation.reset_at >= period.reset_at.saturating_sub(RESET_AT_TOLERANCE_SECONDS)
                && observation.reset_at <= period.reset_at
                && observation.timestamp <= period.end_at
        }) {
            period.start_at = period.start_at.min(observation.timestamp);
        }
    }

    let mut history = BTreeMap::<(i64, i64), PublicHistoryObservationV3>::new();
    for sample in samples {
        let stored = observations_by_key
            .get(&(sample.reset_at, sample.timestamp))
            .copied();
        let group = model_groups.get(&(sample.reset_at, sample.timestamp));
        let model_source = stored
            .map(|observation| observation.model_source)
            // Missing or malformed provenance is not evidence that a row was
            // explicitly saved as legacy. Keep its quota/time metadata while
            // failing closed for every model value.
            .unwrap_or(HistoryModelSource::Unavailable);
        let group_models_complete = group.is_some_and(HistoryModelGroup::model_set_complete);
        let source = if model_source == HistoryModelSource::Unavailable {
            HistoryModelSource::Unavailable
        } else if model_source == HistoryModelSource::ReconstructedFromSession {
            HistoryModelSource::ReconstructedFromSession
        } else if model_source == HistoryModelSource::Confirmed && group_models_complete {
            HistoryModelSource::Confirmed
        } else {
            HistoryModelSource::LegacyUnknown
        };
        // model_set_complete describes the stored sidecar group. The wire
        // claim is stronger: the same observation must also have confirmed
        // provenance. Derive both wire fields from this single decision so a
        // downgraded or reconstructed row cannot advertise a complete model
        // universe.
        let models_complete = source == HistoryModelSource::Confirmed && group_models_complete;
        let models = if matches!(
            source,
            HistoryModelSource::ReconstructedFromSession | HistoryModelSource::Unavailable
        ) {
            None
        } else {
            history_models_v3(group, sample)
        };
        history.insert(
            (sample.reset_at, sample.timestamp),
            PublicHistoryObservationV3 {
                timestamp: sample.timestamp,
                reset_at: sample.reset_at,
                remaining_percent: sample.remaining_percent,
                task_active_since_previous: None,
                models,
                models_complete,
                model_source: source.as_str().to_owned(),
            },
        );
    }

    // Keep sidecar-only unavailable points visible in v3. They are admitted
    // only when they belong to an already projected period and do not collide
    // with a valid legacy sample.
    for observation in observations
        .iter()
        .filter(|observation| observation.model_source == HistoryModelSource::Unavailable)
    {
        let Some(period) = periods.iter().find(|period| {
            observation.reset_at >= period.reset_at.saturating_sub(RESET_AT_TOLERANCE_SECONDS)
                && observation.reset_at <= period.reset_at
                && observation.timestamp >= period.start_at
                && observation.timestamp <= period.end_at
        }) else {
            continue;
        };
        let key = (period.reset_at, observation.timestamp);
        history
            .entry(key)
            .or_insert_with(|| PublicHistoryObservationV3 {
                timestamp: observation.timestamp,
                reset_at: period.reset_at,
                remaining_percent: observation.remaining_percent,
                task_active_since_previous: None,
                models: None,
                models_complete: false,
                model_source: "unavailable".to_owned(),
            });
    }
    suppress_regressing_history_models(&mut history);
    assign_task_activity_for_intervals(&mut history, periods, task_evidence, intervals);
    assign_history_period_labels(periods);
    Ok(history.into_values().collect())
}

#[cfg(test)]
fn assign_task_activity(
    history: &mut BTreeMap<(i64, i64), PublicHistoryObservationV3>,
    periods: &[PublicHistoryPeriod],
    evidence: &TaskActivityEvidence,
) {
    assign_task_activity_for_intervals(history, periods, evidence, &ReadIntervals::unbounded());
}

fn assign_task_activity_for_intervals(
    history: &mut BTreeMap<(i64, i64), PublicHistoryObservationV3>,
    periods: &[PublicHistoryPeriod],
    evidence: &TaskActivityEvidence,
    intervals: &ReadIntervals,
) {
    let activity = TaskActivityIntervals::from_evidence_for_intervals(evidence, intervals);
    // Sparse numeric history can have a long interval between two direct
    // observations even though the fully indexed task lifecycle supplies
    // exact activity boundaries. Publish model-less minute cadence rows so
    // clients do not smear the next completed token event backwards across
    // the gap. These rows claim neither quota nor model values and therefore
    // use the unavailable model source, never legacy-observed provenance.
    for period in periods {
        let mut timestamps = history
            .keys()
            .filter(|(reset_at, timestamp)| {
                reset_at.abs_diff(period.reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
                    && *timestamp >= period.start_at
                    && *timestamp <= period.end_at
            })
            .map(|(_, timestamp)| *timestamp)
            .collect::<Vec<_>>();
        timestamps.sort_unstable();
        timestamps.dedup();
        for pair in timestamps.windows(2) {
            let [left, right] = pair else {
                continue;
            };
            if intervals.interval_containing(*left) != intervals.interval_containing(*right) {
                continue;
            }
            let publish_full_cadence =
                evidence.complete || task_interval_overlaps_verified_idle(&activity, *left, *right);
            let mut cadence = if publish_full_cadence {
                let mut cadence = Vec::new();
                let mut timestamp = left.saturating_add(60);
                while timestamp < *right {
                    cadence.push(timestamp);
                    timestamp = timestamp.saturating_add(60);
                }
                cadence
            } else {
                // Even when coverage is only source-local, publish the minute
                // boundaries surrounding each verified lifecycle transition.
                // Otherwise a few seconds of work before a stop can make one
                // sparse multi-hour observation interval look wholly active.
                let start = activity
                    .verified_transition_minutes
                    .partition_point(|timestamp| *timestamp <= *left);
                let end = activity
                    .verified_transition_minutes
                    .partition_point(|timestamp| *timestamp < *right);
                activity.verified_transition_minutes[start..end].to_vec()
            };
            cadence.sort_unstable();
            cadence.dedup();
            for timestamp in cadence {
                history
                    .entry((period.reset_at, timestamp))
                    .or_insert_with(|| PublicHistoryObservationV3 {
                        timestamp,
                        reset_at: period.reset_at,
                        remaining_percent: None,
                        task_active_since_previous: None,
                        models: None,
                        models_complete: false,
                        model_source: "unavailable".to_owned(),
                    });
            }
        }
    }

    for observation in history.values_mut() {
        observation.task_active_since_previous = None;
    }

    for period in periods {
        let mut keys = history
            .keys()
            .filter(|(reset_at, timestamp)| {
                reset_at.abs_diff(period.reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
                    && *timestamp >= period.start_at
                    && *timestamp <= period.end_at
            })
            .copied()
            .collect::<Vec<_>>();
        keys.sort_by_key(|(_, timestamp)| *timestamp);
        let mut previous_timestamp = None;
        for key @ (_, timestamp) in keys {
            let current_interval = intervals.interval_containing(timestamp);
            let same_interval = previous_timestamp.is_some_and(|previous| {
                intervals.interval_containing(previous) == current_interval
            });
            let interval_start = if same_interval {
                previous_timestamp.unwrap_or(period.start_at)
            } else {
                current_interval
                    .and_then(|interval| interval.start_at)
                    .unwrap_or(period.start_at)
            };
            let active =
                task_active_in_interval(&activity, interval_start.min(timestamp), timestamp);
            let verified_idle = !active
                && task_interval_is_verified_idle(
                    &activity,
                    interval_start.min(timestamp),
                    timestamp,
                );
            history
                .get_mut(&key)
                .expect("task activity key collected from history")
                .task_active_since_previous = if evidence.complete || verified_idle {
                Some(active)
            } else {
                active.then_some(true)
            };
            previous_timestamp = Some(timestamp);
        }
    }
}

#[derive(Debug, Default)]
struct TaskActivityIntervals {
    active: Vec<(i64, i64)>,
    verified_idle: Vec<(i64, i64)>,
    verified_idle_prefix_end: Vec<i64>,
    verified_idle_union: Vec<(i64, i64)>,
    verified_transition_minutes: Vec<i64>,
}

impl TaskActivityIntervals {
    fn from_evidence(evidence: &TaskActivityEvidence) -> Self {
        let mut active = Vec::new();
        let mut verified_idle = Vec::new();
        let mut verified_transition_minutes = Vec::new();
        for (source, events) in &evidence.transitions {
            let observed_through = evidence
                .source_observed_through
                .get(source)
                .copied()
                .or_else(|| events.last().map(|event| event.timestamp));
            let mut active_since = None;
            for event in events {
                if event.running {
                    active_since.get_or_insert(event.timestamp);
                } else if let Some(start) = active_since.take() {
                    active.push((start, event.timestamp));
                }
            }
            if let (Some(start), Some(end)) = (active_since, observed_through) {
                if end >= start {
                    active.push((start, end));
                }
            }

            if evidence.source_complete.get(source) != Some(&true) {
                continue;
            }
            verified_idle.extend(events.windows(2).filter_map(|pair| {
                (!pair[0].running && pair[1].running)
                    .then_some((pair[0].timestamp, pair[1].timestamp))
            }));
            for event in events {
                let minute = event.timestamp.div_euclid(60).saturating_mul(60);
                verified_transition_minutes.push(minute);
                verified_transition_minutes.push(minute.saturating_add(60));
            }
        }
        active = merge_closed_intervals(active);
        verified_idle.sort_unstable();
        let mut furthest_end = i64::MIN;
        let verified_idle_prefix_end = verified_idle
            .iter()
            .map(|(_, end)| {
                furthest_end = furthest_end.max(*end);
                furthest_end
            })
            .collect();
        let verified_idle_union = merge_closed_intervals(verified_idle.clone());
        verified_transition_minutes.sort_unstable();
        verified_transition_minutes.dedup();
        Self {
            active,
            verified_idle,
            verified_idle_prefix_end,
            verified_idle_union,
            verified_transition_minutes,
        }
    }

    fn from_evidence_for_intervals(
        evidence: &TaskActivityEvidence,
        intervals: &ReadIntervals,
    ) -> Self {
        if intervals.is_unbounded() {
            return Self::from_evidence(evidence);
        }
        let mut activity = Self::from_evidence(evidence);
        activity.active = clip_closed_intervals(activity.active, intervals);
        activity.verified_idle = clip_closed_intervals(activity.verified_idle, intervals);
        let mut furthest_end = i64::MIN;
        activity.verified_idle_prefix_end = activity
            .verified_idle
            .iter()
            .map(|(_, end)| {
                furthest_end = furthest_end.max(*end);
                furthest_end
            })
            .collect();
        activity.verified_idle_union = merge_closed_intervals(activity.verified_idle.clone());
        activity
            .verified_transition_minutes
            .retain(|timestamp| intervals.intersects_canonical_minute(*timestamp));
        activity
    }
}

fn merge_closed_intervals(mut intervals: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    intervals.sort_unstable();
    let mut merged = Vec::<(i64, i64)>::with_capacity(intervals.len());
    for (start, end) in intervals {
        if let Some((_, previous_end)) = merged.last_mut() {
            if start <= *previous_end {
                *previous_end = (*previous_end).max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn clip_closed_intervals(ranges: Vec<(i64, i64)>, intervals: &ReadIntervals) -> Vec<(i64, i64)> {
    let mut clipped = Vec::new();
    for (start, end) in ranges {
        for interval in intervals.intervals() {
            let interval_start = interval.start_at.unwrap_or(i64::MIN);
            let interval_end = interval
                .end_at
                .and_then(|end| end.checked_sub(1))
                .unwrap_or(i64::MAX);
            let start = start.max(interval_start);
            let end = end.min(interval_end);
            if start <= end {
                clipped.push((start, end));
            }
        }
    }
    merge_closed_intervals(clipped)
}

fn task_interval_overlaps_verified_idle(
    activity: &TaskActivityIntervals,
    start: i64,
    end: i64,
) -> bool {
    end > start
        && activity
            .verified_idle_union
            .partition_point(|(idle_start, _)| *idle_start < end)
            .checked_sub(1)
            .is_some_and(|index| activity.verified_idle_union[index].1 > start)
}

fn task_interval_is_verified_idle(activity: &TaskActivityIntervals, start: i64, end: i64) -> bool {
    if end <= start {
        return false;
    }
    let Some(index) = activity
        .verified_idle
        .partition_point(|(idle_start, _)| *idle_start <= start)
        .checked_sub(1)
    else {
        return false;
    };
    activity.verified_idle_prefix_end[index] >= end
}

fn task_active_in_interval(activity: &TaskActivityIntervals, start: i64, end: i64) -> bool {
    if start > end {
        return false;
    }
    activity
        .active
        .partition_point(|(active_start, _)| *active_start <= end)
        .checked_sub(1)
        .is_some_and(|index| activity.active[index].1 >= start)
}

fn suppress_regressing_history_models(
    history: &mut BTreeMap<(i64, i64), PublicHistoryObservationV3>,
) {
    let mut watermarks = BTreeMap::<(i64, String), PublicHistoryModelUsageV3>::new();
    for ((reset_at, _), observation) in history.iter_mut() {
        // Only a complete direct observation is numeric authority. Legacy
        // values remain available for display, but cannot move this watermark
        // or cause a later direct observation to be rejected.
        if observation.model_source != "confirmed" || !observation.models_complete {
            continue;
        }
        let Some(models) = observation.models.as_mut() else {
            continue;
        };
        let suppressed = models
            .iter()
            .filter_map(|candidate| {
                let key = (*reset_at, candidate.model.clone());
                watermarks
                    .get(&key)
                    .is_some_and(|watermark| !history_model_dominates(candidate, watermark))
                    .then_some(candidate.model.clone())
            })
            .collect::<BTreeSet<_>>();
        if suppressed.is_empty() {
            for candidate in models.iter() {
                let key = (*reset_at, candidate.model.clone());
                watermarks
                    .entry(key)
                    .and_modify(|watermark| advance_history_model_watermark(watermark, candidate))
                    .or_insert_with(|| candidate.clone());
            }
        } else {
            models.retain(|candidate| !suppressed.contains(&candidate.model));
            observation.models_complete = false;
            observation.model_source = "legacy-unknown".to_owned();
        }
        models.sort_by(|left, right| left.model.cmp(&right.model));
        if models.is_empty() {
            observation.models = None;
        }
    }
}

fn history_model_dominates(
    candidate: &PublicHistoryModelUsageV3,
    watermark: &PublicHistoryModelUsageV3,
) -> bool {
    candidate.total_tokens >= watermark.total_tokens
        && optional_u64_does_not_regress(candidate.input_tokens, watermark.input_tokens)
        && optional_u64_does_not_regress(
            candidate.cached_input_tokens,
            watermark.cached_input_tokens,
        )
        && optional_u64_does_not_regress(
            candidate.cache_write_input_tokens,
            watermark.cache_write_input_tokens,
        )
        && optional_u64_does_not_regress(candidate.output_tokens, watermark.output_tokens)
        && optional_f64_does_not_regress(candidate.total_dollars, watermark.total_dollars)
}

fn optional_u64_does_not_regress(candidate: Option<u64>, watermark: Option<u64>) -> bool {
    match (candidate, watermark) {
        (Some(candidate), Some(watermark)) => candidate >= watermark,
        _ => true,
    }
}

fn optional_f64_does_not_regress(candidate: Option<f64>, watermark: Option<f64>) -> bool {
    match (candidate, watermark) {
        (Some(candidate), Some(watermark)) => candidate >= watermark,
        _ => true,
    }
}

fn advance_history_model_watermark(
    watermark: &mut PublicHistoryModelUsageV3,
    candidate: &PublicHistoryModelUsageV3,
) {
    watermark.total_tokens = candidate.total_tokens;
    if candidate.input_tokens.is_some() {
        watermark.input_tokens = candidate.input_tokens;
    }
    if candidate.cached_input_tokens.is_some() {
        watermark.cached_input_tokens = candidate.cached_input_tokens;
    }
    if candidate.cache_write_input_tokens.is_some() {
        watermark.cache_write_input_tokens = candidate.cache_write_input_tokens;
    }
    if candidate.output_tokens.is_some() {
        watermark.output_tokens = candidate.output_tokens;
    }
    if candidate.total_dollars.is_some() {
        watermark.total_dollars = candidate.total_dollars;
    }
}

fn history_observations_v2(
    samples: &[PublicHistorySample],
    history_v3: &[PublicHistoryObservationV3],
) -> Vec<PublicHistoryObservation> {
    history_v3
        .iter()
        .map(|sample| {
            let legacy = (!matches!(
                sample.model_source.as_str(),
                "reconstructed-from-session" | "unavailable"
            ))
            .then(|| {
                samples.iter().find(|legacy| {
                    legacy.timestamp == sample.timestamp && legacy.reset_at == sample.reset_at
                })
            })
            .flatten();
            PublicHistoryObservation {
                timestamp: sample.timestamp,
                reset_at: sample.reset_at,
                remaining_percent: sample.remaining_percent,
                sol_dollars: legacy.map(|legacy| legacy.sol_dollars),
                terra_dollars: legacy.map(|legacy| legacy.terra_dollars),
                luna_dollars: legacy.map(|legacy| legacy.luna_dollars),
                sol_tokens: legacy.map(|legacy| legacy.sol_tokens),
                terra_tokens: legacy.map(|legacy| legacy.terra_tokens),
                luna_tokens: legacy.map(|legacy| legacy.luna_tokens),
                model_source: sample.model_source.clone(),
            }
        })
        .collect()
}

fn history_samples_v1(samples: &[PublicHistoryObservation]) -> Vec<PublicHistorySample> {
    samples
        .iter()
        .filter(|sample| matches!(sample.model_source.as_str(), "confirmed" | "legacy-unknown"))
        .filter_map(|sample| {
            Some(PublicHistorySample {
                timestamp: sample.timestamp,
                reset_at: sample.reset_at,
                remaining_percent: sample.remaining_percent,
                sol_dollars: sample.sol_dollars?,
                terra_dollars: sample.terra_dollars?,
                luna_dollars: sample.luna_dollars?,
                sol_tokens: sample.sol_tokens?,
                terra_tokens: sample.terra_tokens?,
                luna_tokens: sample.luna_tokens?,
            })
        })
        .collect()
}

fn read_stored_history_observations_for_intervals(
    connection: &Connection,
    cutoff: i64,
    observed_at: i64,
    intervals: &ReadIntervals,
) -> Result<Vec<StoredHistoryObservation>, ReaderError> {
    if !table_exists(connection, "durable_state")? {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT singleton, data_generation, snapshot_json
         FROM durable_state
         WHERE singleton >= 2 AND data_generation > ?1 AND data_generation <= ?2
         ORDER BY data_generation ASC, singleton ASC",
    )?;
    let mut rows = statement.query(params![cutoff, observed_at])?;
    let mut observations = BTreeMap::<(i64, i64), StoredHistoryObservation>::new();
    while let Some(row) = rows.next()? {
        let Some(data_generation) = sql_i64(row, 1) else {
            continue;
        };
        let Some(snapshot_json) = sql_text(row, 2) else {
            continue;
        };
        let Some(observation) = parse_stored_history_observation(data_generation, &snapshot_json)
        else {
            // A malformed durable observation must not hide valid legacy
            // history or unrelated sidecar rows.
            continue;
        };
        if intervals.intersects_canonical_minute(observation.timestamp) {
            observations.insert((observation.reset_at, observation.timestamp), observation);
        }
    }
    Ok(observations.into_values().collect())
}

fn parse_stored_history_observation(
    data_generation: i64,
    snapshot_json: &str,
) -> Option<StoredHistoryObservation> {
    let value: serde_json::Value = serde_json::from_str(snapshot_json).ok()?;
    let object = value.as_object()?;
    if object.get("kind").and_then(serde_json::Value::as_str)
        != Some("codex-info-usage-observation-v1")
    {
        return None;
    }
    let timestamp = object
        .get("timestamp")
        .and_then(serde_json::Value::as_i64)?;
    let reset_at = object.get("reset_at").and_then(serde_json::Value::as_i64)?;
    if timestamp != data_generation
        || !valid_public_timestamp(timestamp)
        || !valid_public_timestamp(reset_at)
    {
        return None;
    }
    let remaining_percent = match object.get("remaining_percent")? {
        value if value.is_null() => None,
        value => {
            let value = value.as_f64()?;
            if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                return None;
            }
            Some(value)
        }
    };
    let model_source = match object
        .get("model_source")
        .and_then(serde_json::Value::as_str)?
    {
        "confirmed" => HistoryModelSource::Confirmed,
        "reconstructed-from-session" => HistoryModelSource::ReconstructedFromSession,
        "unavailable" => HistoryModelSource::Unavailable,
        "legacy-unknown" => HistoryModelSource::LegacyUnknown,
        _ => return None,
    };
    Some(StoredHistoryObservation {
        timestamp,
        reset_at,
        remaining_percent,
        model_source,
    })
}

fn read_history_model_groups_for_intervals(
    connection: &Connection,
    cutoff: i64,
    observed_at: i64,
    intervals: &ReadIntervals,
) -> Result<BTreeMap<(i64, i64), HistoryModelGroup>, ReaderError> {
    if !table_exists(connection, "usage_model_history")? {
        return Ok(BTreeMap::new());
    }
    let has_cache_write = table_has_column(
        connection,
        "usage_model_history",
        "cache_write_input_tokens",
    )?;
    let query = if has_cache_write {
        "SELECT reset_at, timestamp, model, total_tokens, input_tokens,
                cached_input_tokens, output_tokens, cache_write_input_tokens,
                model_set_complete
         FROM usage_model_history
         WHERE timestamp > ?1 AND timestamp <= ?2
         ORDER BY reset_at, timestamp, model"
    } else {
        "SELECT reset_at, timestamp, model, total_tokens, input_tokens,
                cached_input_tokens, output_tokens, model_set_complete
         FROM usage_model_history
         WHERE timestamp > ?1 AND timestamp <= ?2
         ORDER BY reset_at, timestamp, model"
    };
    let mut statement = connection.prepare(query)?;
    let mut rows = statement.query(params![cutoff, observed_at])?;
    let mut groups = BTreeMap::<(i64, i64), HistoryModelGroup>::new();
    while let Some(row) = rows.next()? {
        let Some(reset_at) = sql_i64(row, 0) else {
            continue;
        };
        let Some(timestamp) = sql_i64(row, 1) else {
            continue;
        };
        if !intervals.intersects_canonical_minute(timestamp) {
            continue;
        }
        let complete_index = if has_cache_write { 8 } else { 7 };
        let group = groups.entry((reset_at, timestamp)).or_default();
        let Some(complete) = sql_i64(row, complete_index) else {
            group.valid = false;
            continue;
        };
        let complete = match complete {
            0 => false,
            1 => true,
            _ => {
                group.valid = false;
                continue;
            }
        };
        if group.complete_known {
            if group.complete != complete {
                group.valid = false;
            }
        } else {
            group.complete = complete;
            group.complete_known = true;
        }
        let Some(model) = sql_text(row, 2) else {
            group.valid = false;
            continue;
        };
        let Some(total_tokens) = sql_text(row, 3).and_then(|value| value.parse::<u64>().ok())
        else {
            group.valid = false;
            continue;
        };
        let Some(input_tokens) = sql_text(row, 4).and_then(|value| value.parse::<u64>().ok())
        else {
            group.valid = false;
            continue;
        };
        let Some(cached_input_tokens) =
            sql_text(row, 5).and_then(|value| value.parse::<u64>().ok())
        else {
            group.valid = false;
            continue;
        };
        let Some(output_tokens) = sql_text(row, 6).and_then(|value| value.parse::<u64>().ok())
        else {
            group.valid = false;
            continue;
        };
        let cache_write_input_tokens = if has_cache_write {
            match sql_text_option(row, 7) {
                Some(None) => None,
                Some(Some(value)) => match value.parse::<u64>() {
                    Ok(value) => Some(value),
                    Err(_) => {
                        group.valid = false;
                        continue;
                    }
                },
                None => {
                    group.valid = false;
                    continue;
                }
            }
        } else {
            None
        };
        let row = RawModelTotal {
            model,
            total_tokens,
            input_tokens,
            cached_input_tokens,
            output_tokens,
            cache_write_input_tokens,
        };
        if !valid_public_timestamp(reset_at)
            || !valid_public_timestamp(timestamp)
            || !is_valid_public_model_name(&row.model)
            || row.cached_input_tokens > row.input_tokens
            || row.cache_write_input_tokens.is_some_and(|writes| {
                row.cached_input_tokens
                    .checked_add(writes)
                    .is_none_or(|discounted| discounted > row.input_tokens)
            })
            || group.totals.contains_key(&row.model)
        {
            group.valid = false;
            continue;
        }
        group.totals.insert(row.model.clone(), row);
    }
    Ok(groups)
}

fn sql_text(row: &Row<'_>, index: usize) -> Option<String> {
    match row.get_ref(index).ok()? {
        ValueRef::Text(value) => std::str::from_utf8(value).ok().map(str::to_owned),
        ValueRef::Integer(value) => Some(value.to_string()),
        ValueRef::Null | ValueRef::Real(_) | ValueRef::Blob(_) => None,
    }
}

fn sql_text_option(row: &Row<'_>, index: usize) -> Option<Option<String>> {
    match row.get_ref(index).ok()? {
        ValueRef::Null => Some(None),
        ValueRef::Text(value) => std::str::from_utf8(value)
            .ok()
            .map(|value| Some(value.to_owned())),
        ValueRef::Integer(value) => Some(Some(value.to_string())),
        ValueRef::Real(_) | ValueRef::Blob(_) => None,
    }
}

fn sql_i64(row: &Row<'_>, index: usize) -> Option<i64> {
    match row.get_ref(index).ok()? {
        ValueRef::Integer(value) => Some(value),
        ValueRef::Text(value) => std::str::from_utf8(value).ok()?.parse().ok(),
        ValueRef::Null | ValueRef::Real(_) | ValueRef::Blob(_) => None,
    }
}

fn history_models_v3(
    group: Option<&HistoryModelGroup>,
    sample: &PublicHistorySample,
) -> Option<Vec<PublicHistoryModelUsageV3>> {
    let mut models = group
        .map(|group| {
            group
                .totals
                .values()
                .map(|total| history_model_usage_v3(total, sample))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    models.sort_by(|left, right| left.model.cmp(&right.model));
    (!models.is_empty()).then_some(models)
}

fn history_model_usage_v3(
    total: &RawModelTotal,
    sample: &PublicHistorySample,
) -> PublicHistoryModelUsageV3 {
    let total_dollars = match total.model.as_str() {
        "SOL" => Some(sample.sol_dollars),
        "TERRA" => Some(sample.terra_dollars),
        "LUNA" => Some(sample.luna_dollars),
        // `usage_history` has no dollar column for arbitrary model names.
        // Do not turn a pricing estimate into a historical direct value.
        _ => None,
    };
    PublicHistoryModelUsageV3 {
        model: total.model.clone(),
        total_tokens: total.total_tokens,
        input_tokens: Some(total.input_tokens),
        cached_input_tokens: Some(total.cached_input_tokens),
        cache_write_input_tokens: total.cache_write_input_tokens,
        output_tokens: Some(total.output_tokens),
        total_dollars,
    }
}

fn history_periods(
    samples: &[PublicHistorySample],
    observed_at: i64,
    current_reset_at: Option<i64>,
    _window_seconds: i64,
) -> Vec<PublicHistoryPeriod> {
    let mut ranges = BTreeMap::<i64, (i64, i64)>::new();
    for sample in samples {
        ranges
            .entry(sample.reset_at)
            .and_modify(|range| {
                range.0 = range.0.min(sample.timestamp);
                range.1 = range.1.max(sample.timestamp);
            })
            .or_insert((sample.timestamp, sample.timestamp));
    }
    let current = current_reset_at.and_then(|authority| {
        ranges
            .keys()
            .copied()
            .filter(|reset_at| reset_at.abs_diff(authority) <= RESET_AT_TOLERANCE_SECONDS as u64)
            .min_by_key(|reset_at| reset_at.abs_diff(authority))
    });
    let fallback_current = current.or_else(|| {
        samples
            .iter()
            .max_by_key(|sample| (sample.timestamp, sample.reset_at))
            .map(|sample| sample.reset_at)
    });
    let mut periods = ranges
        .into_iter()
        .map(|(reset_at, (observed_start, observed_end))| {
            let is_current = Some(reset_at) == current;
            let start_at = observed_start;
            let mut end_at = observed_end.min(observed_at);
            if is_current {
                // `canonicalize_history` is the single authority for period
                // membership. A corrected rolling reset deadline can move
                // `reset_at - window` forward after the observed quota
                // recovery; re-clipping here would orphan those canonical
                // samples and split one physical cycle between two starts.
                // Samples are canonicalized to minute starts, while the
                // recorder's observed_at may retain event-level seconds.
                // The contract's current-period boundary is the exact
                // authority/observation minimum, not the rounded sample end.
                end_at = reset_at.min(observed_at);
            } else {
                end_at = end_at.min(reset_at);
            }
            PublicHistoryPeriod {
                // The root UI uses the canonical reset instant as its period
                // ID. Keep this stable numeric identity across REST and
                // Windows selections; labels remain presentation text.
                id: reset_at.to_string(),
                start_at,
                end_at: end_at.max(start_at),
                reset_at,
                label: String::new(),
                current: is_current || Some(reset_at) == fallback_current,
            }
        })
        .collect::<Vec<_>>();
    assign_history_period_labels(&mut periods);
    // The Windows client treats period order as part of the wire contract:
    // newest starts must precede older starts.  Keep reset/id as deterministic
    // tie-breakers for clipped or same-minute periods.
    periods.sort_by(|left, right| {
        right
            .start_at
            .cmp(&left.start_at)
            .then_with(|| right.reset_at.cmp(&left.reset_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    periods
}

fn clip_history_periods(periods: &mut Vec<PublicHistoryPeriod>, intervals: &ReadIntervals) {
    periods.retain_mut(|period| {
        let intersections = intervals
            .intervals()
            .iter()
            .filter_map(|interval| {
                // History points are minute buckets selected from the exact
                // account interval before this projection is built.  Keep
                // that exact row authority, but align the visible period
                // boundary to the same minute coordinate as its first point;
                // otherwise an activation at HH:MM:SS makes that valid point
                // precede the period and rejects the complete REST root.
                let visible_start = interval
                    .start_at
                    .map(|start| start - start.rem_euclid(60))
                    .unwrap_or(i64::MIN);
                let start = visible_start.max(period.start_at);
                let end = interval
                    .end_at
                    .and_then(|end| end.checked_sub(1))
                    .unwrap_or(i64::MAX)
                    .min(period.end_at);
                (start <= end).then_some((start, end))
            })
            .collect::<Vec<_>>();
        let Some((first_start, _)) = intersections.first().copied() else {
            return false;
        };
        let (_, last_end) = intersections
            .last()
            .copied()
            .unwrap_or((first_start, first_start));
        period.start_at = first_start;
        period.end_at = last_end;
        if !intervals.contains(period.reset_at) {
            period.current = false;
        }
        period.start_at <= period.end_at
    });
}

fn assign_history_period_labels(periods: &mut [PublicHistoryPeriod]) {
    for period in periods.iter_mut() {
        period.label = format_jst_period_label(
            period.start_at,
            if period.current {
                period.reset_at
            } else {
                period.end_at
            },
            period.current,
        );
    }
    let base_labels = periods
        .iter()
        .map(|period| period.label.clone())
        .collect::<Vec<_>>();
    let mut label_counts = BTreeMap::<String, usize>::new();
    for label in &base_labels {
        *label_counts.entry(label.clone()).or_default() += 1;
    }
    for (index, period) in periods.iter_mut().enumerate() {
        if label_counts
            .get(&base_labels[index])
            .is_some_and(|count| *count > 1)
        {
            let reset_label =
                format_jst_timestamp(period.reset_at).unwrap_or_else(|| "時刻不明".to_owned());
            period.label.push_str(&format!("（期限 {reset_label}）"));
        }
    }
}

fn format_jst_period_label(start_at: i64, end_at: i64, current: bool) -> String {
    let Some(start) = format_jst_timestamp(start_at) else {
        return "期間不明".to_owned();
    };
    let Some(end) = format_jst_timestamp(end_at) else {
        return "期間不明".to_owned();
    };
    let current_suffix = if current { "（現在）" } else { "" };
    format!("{start} ～ {end}{current_suffix}")
}

fn format_jst_timestamp(timestamp: i64) -> Option<String> {
    let shifted = timestamp.checked_add(9 * 60 * 60)?;
    let days = shifted.div_euclid(86_400);
    let seconds = shifted.rem_euclid(86_400);
    let (year, month, day) = civil_date_from_days(days);
    let hour = seconds / 3_600;
    let minute = seconds.rem_euclid(3_600) / 60;
    let second = seconds.rem_euclid(60);
    Some(format!(
        "{year:04}/{month:02}/{day:02} {hour:02}:{minute:02}:{second:02} +09:00"
    ))
}

// Proleptic Gregorian conversion for valid public Unix timestamps. JST is a
// fixed +09:00 reference label shared by the split REST clients.
fn civil_date_from_days(days_since_unix_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524
        - day_of_era / 146_096)
        .div_euclid(365);
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2).div_euclid(153);
    let day = day_of_year - (153 * month_part + 2).div_euclid(5) + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

fn latest_quota_row(rows: &[RawSample], current_reset_at: Option<i64>) -> Option<&RawSample> {
    if let Some(authority) = current_reset_at {
        if let Some(row) = rows
            .iter()
            .filter(|row| {
                row.remaining_percent.is_some()
                    && row.reset_at.abs_diff(authority) <= RESET_AT_TOLERANCE_SECONDS as u64
            })
            .max_by_key(|row| (row.timestamp, row.reset_at))
        {
            return Some(row);
        }
    }
    rows.iter()
        .filter(|row| row.remaining_percent.is_some())
        .max_by_key(|row| (row.timestamp, row.reset_at))
}

fn read_collection_config(connection: &Connection) -> Result<(Option<i64>, i64), ReaderError> {
    if !table_exists(connection, "collection_generation")? {
        return Ok((None, 0));
    }
    let value = connection
        .query_row(
            "SELECT reset_at, window_seconds FROM collection_generation WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    Ok(value
        .map(|(reset_at, window_seconds)| {
            (
                (reset_at > 0).then_some(reset_at),
                if window_seconds > 0 {
                    window_seconds
                } else {
                    0
                },
            )
        })
        .unwrap_or((None, 0)))
}

/// Canonicalizes the same bounded public history window used by every
/// account reader and by the durable projection writer. Keeping this as one
/// pure function prevents migration-time and read-time period identities
/// from drifting apart.
pub fn canonicalize_history_for_public_window(
    rows: &[RawSample],
    current_reset_at: Option<i64>,
    window_seconds: i64,
) -> Result<Vec<PublicHistorySample>, ReaderError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let observed_at = rows.iter().map(|row| row.timestamp).max().unwrap_or(0);
    let cutoff = observed_at.saturating_sub(HISTORY_WINDOW_SECONDS);
    let recent = rows
        .iter()
        .filter(|row| row.timestamp > cutoff && row.timestamp <= observed_at)
        .cloned()
        .collect::<Vec<_>>();
    let mut samples = canonicalize_history(&recent, current_reset_at, window_seconds, observed_at)?;
    if samples.is_empty() && !recent.is_empty() {
        // A stale/missing collection-generation window must not turn an
        // existing history database into a fabricated empty root. Retain the
        // structurally canonical rows while the explicit state reports that
        // there is no authoritative current window.
        samples = canonicalize_history(&recent, None, 0, observed_at)?;
    }
    Ok(samples)
}

/// Canonicalizes every retained row for the one-time account database
/// migration. Unlike the public-window entry point, this does not apply the
/// 31-day response-size limit; retention is owned by the writer.
pub fn canonicalize_history_for_storage(
    rows: &[RawSample],
    current_reset_at: Option<i64>,
    window_seconds: i64,
) -> Result<Vec<PublicHistorySample>, ReaderError> {
    canonicalize_history_for_storage_with_sources(rows, current_reset_at, window_seconds)
        .map(|samples| samples.into_iter().map(|sample| sample.sample).collect())
}

/// Storage canonicalization with the exact retained legacy source key.
pub fn canonicalize_history_for_storage_with_sources(
    rows: &[RawSample],
    current_reset_at: Option<i64>,
    window_seconds: i64,
) -> Result<Vec<CanonicalStorageSample>, ReaderError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let observed_at = rows.iter().map(|row| row.timestamp).max().unwrap_or(0);
    canonicalize_history_with_sources_and_limit(
        rows,
        current_reset_at,
        window_seconds,
        observed_at,
        None,
        true,
    )
}

fn public_sample_from_raw(row: &RawSample) -> PublicHistorySample {
    PublicHistorySample {
        timestamp: row.timestamp,
        reset_at: row.reset_at,
        remaining_percent: row.remaining_percent,
        sol_dollars: row.sol_dollars,
        terra_dollars: row.terra_dollars,
        luna_dollars: row.luna_dollars,
        sol_tokens: row.sol_tokens,
        terra_tokens: row.terra_tokens,
        luna_tokens: row.luna_tokens,
    }
}

fn canonicalize_history(
    rows: &[RawSample],
    current_reset_at: Option<i64>,
    window_seconds: i64,
    observed_at: i64,
) -> Result<Vec<PublicHistorySample>, ReaderError> {
    canonicalize_history_with_limit(
        rows,
        current_reset_at,
        window_seconds,
        observed_at,
        Some(MAX_HISTORY_ROWS),
    )
}

fn canonicalize_history_with_limit(
    rows: &[RawSample],
    current_reset_at: Option<i64>,
    window_seconds: i64,
    observed_at: i64,
    max_rows: Option<usize>,
) -> Result<Vec<PublicHistorySample>, ReaderError> {
    canonicalize_history_with_sources_and_limit(
        rows,
        current_reset_at,
        window_seconds,
        observed_at,
        max_rows,
        false,
    )
    .map(|samples| samples.into_iter().map(|sample| sample.sample).collect())
}

fn canonicalize_history_with_sources_and_limit(
    rows: &[RawSample],
    current_reset_at: Option<i64>,
    window_seconds: i64,
    observed_at: i64,
    max_rows: Option<usize>,
    reject_ambiguous: bool,
) -> Result<Vec<CanonicalStorageSample>, ReaderError> {
    let mut sorted = rows
        .iter()
        .cloned()
        .map(|row| SourcedRawSample {
            source_timestamp: row.timestamp,
            source_reset_at: row.reset_at,
            row,
        })
        .collect::<Vec<_>>();
    sorted.sort_by_key(|row| (row.timestamp, row.reset_at));
    let mut groups = Vec::<ResetGroup>::new();
    for mut row in sorted {
        if let Some(authority) = current_reset_at {
            if row.reset_at.abs_diff(authority) <= RESET_AT_TOLERANCE_SECONDS as u64 {
                row.reset_at = authority;
            }
        }
        let mut belongs = None;
        for (index, group) in groups.iter().enumerate().rev() {
            let anchor = group.rows.last().unwrap_or(&row);
            if reset_belongs_to_group(anchor, &row) {
                belongs = Some(index);
                break;
            }
            if row.timestamp.saturating_sub(anchor.timestamp) > MOVING_RESET_GROUP_MAX_DRIFT_SECONDS
            {
                break;
            }
        }
        if let Some(index) = belongs {
            let group = &mut groups[index];
            group.canonical_reset_at = group.canonical_reset_at.max(row.reset_at);
            group.rows.push(row);
        } else {
            groups.push(ResetGroup {
                canonical_reset_at: row.reset_at,
                start: row.timestamp,
                rows: vec![row],
            });
        }
    }

    let mut exact_merged = Vec::<ResetGroup>::with_capacity(groups.len());
    let mut exact_indexes = BTreeMap::<i64, usize>::new();
    for mut group in groups {
        if let Some(&index) = exact_indexes.get(&group.canonical_reset_at) {
            let existing = &mut exact_merged[index];
            existing.start = existing.start.min(group.start);
            existing.rows.append(&mut group.rows);
        } else {
            exact_indexes.insert(group.canonical_reset_at, exact_merged.len());
            exact_merged.push(group);
        }
    }
    for group in &mut exact_merged {
        group.rows.sort_by_key(|row| (row.timestamp, row.reset_at));
    }
    exact_merged.sort_by_key(|group| (group.start, group.canonical_reset_at));
    let mut family_merged = Vec::<ResetGroup>::with_capacity(exact_merged.len());
    for mut group in exact_merged {
        let family_index = family_merged.iter().position(|existing| {
            existing
                .canonical_reset_at
                .abs_diff(group.canonical_reset_at)
                <= RESET_AT_TOLERANCE_SECONDS as u64
        });
        if let Some(index) = family_index {
            let existing = &mut family_merged[index];
            existing.canonical_reset_at = existing.canonical_reset_at.max(group.canonical_reset_at);
            existing.start = existing.start.min(group.start);
            existing.rows.append(&mut group.rows);
            existing
                .rows
                .sort_by_key(|row| (row.timestamp, row.reset_at));
        } else {
            family_merged.push(group);
        }
    }
    family_merged.sort_by_key(|group| (group.start, group.canonical_reset_at));
    let mut groups = family_merged;
    clip_stale_reset_group_tails(&mut groups);
    groups.retain(|group| !group.rows.is_empty());
    remove_shadowed_moving_reset_groups(&mut groups);

    // An authoritative reset aliases one group.  Retain only the observed
    // current window for that group; all older groups remain readable.
    let current_group = current_reset_at.and_then(|authority| {
        groups
            .iter()
            .enumerate()
            .filter(|(_, group)| {
                group.canonical_reset_at.abs_diff(authority) <= RESET_AT_TOLERANCE_SECONDS as u64
            })
            .min_by_key(|(_, group)| group.canonical_reset_at.abs_diff(authority))
            .map(|(index, _)| index)
    });
    let theoretical_current_start = current_reset_at
        .zip((window_seconds > 0).then_some(window_seconds))
        .and_then(|(reset, window)| reset.checked_sub(window))
        .map(|start| start - start.rem_euclid(60));
    let current_start = current_group.map(|index| {
        let observed_group_start = groups[index].start - groups[index].start.rem_euclid(60);
        theoretical_current_start
            .map(|theoretical| theoretical.min(observed_group_start))
            .unwrap_or(observed_group_start)
    });

    let mut minute_owners = BTreeMap::<i64, Vec<usize>>::new();
    for (group_index, group) in groups.iter().enumerate() {
        for row in &group.rows {
            minute_owners
                .entry(row.timestamp - row.timestamp.rem_euclid(60))
                .or_default()
                .push(group_index);
        }
    }
    for owners in minute_owners.values_mut() {
        owners.sort_unstable();
        owners.dedup();
    }
    let group_facts = groups
        .iter()
        .map(|group| {
            (
                group.rows.iter().any(|row| row.remaining_percent.is_some()),
                group
                    .rows
                    .iter()
                    .map(|row| row.timestamp.div_euclid(60) * 60)
                    .min()
                    .unwrap_or(group.start.div_euclid(60) * 60),
                group
                    .rows
                    .iter()
                    .map(|row| row.timestamp.div_euclid(60) * 60)
                    .max()
                    .unwrap_or(group.start.div_euclid(60) * 60),
            )
        })
        .collect::<Vec<_>>();
    let mut boundary_minute_owners = BTreeMap::<i64, usize>::new();
    let mut ambiguous_minutes = BTreeSet::<i64>::new();
    for (minute, group_indexes) in minute_owners
        .iter()
        .filter(|(_, group_indexes)| group_indexes.len() > 1)
    {
        let bracketing = group_indexes
            .iter()
            .copied()
            .filter(|group_index| {
                group_facts[*group_index].1 < *minute && group_facts[*group_index].2 > *minute
            })
            .collect::<Vec<_>>();
        if let [owner] = bracketing.as_slice() {
            let competing_rows_are_isolated = group_indexes.iter().copied().all(|group_index| {
                group_index == *owner
                    || (group_facts[group_index].1 == *minute
                        && group_facts[group_index].2 == *minute)
            });
            if competing_rows_are_isolated {
                boundary_minute_owners.insert(*minute, *owner);
                continue;
            }
        }
        let quota_owners = group_indexes
            .iter()
            .copied()
            .filter(|group_index| group_facts[*group_index].0)
            .collect::<Vec<_>>();
        if let [owner] = quota_owners.as_slice() {
            boundary_minute_owners.insert(*minute, *owner);
            continue;
        }
        let continuing = group_indexes
            .iter()
            .copied()
            .filter(|group_index| {
                let group = &groups[*group_index];
                group.start.div_euclid(60) * 60 == *minute && group_facts[*group_index].2 > *minute
            })
            .collect::<Vec<_>>();
        let [owner] = continuing.as_slice() else {
            ambiguous_minutes.insert(*minute);
            continue;
        };
        if group_indexes
            .iter()
            .copied()
            .any(|group_index| group_index != *owner && group_facts[group_index].2 > *minute)
        {
            ambiguous_minutes.insert(*minute);
            continue;
        }
        boundary_minute_owners.insert(*minute, *owner);
    }
    if reject_ambiguous {
        if let Some(minute) = ambiguous_minutes.iter().next() {
            return Err(ReaderError::InvalidValue(format!(
                "history period ownership is ambiguous at timestamp {minute}"
            )));
        }
    }

    let mut canonical = Vec::new();
    for (group_index, group) in groups.into_iter().enumerate() {
        let mut by_minute = BTreeMap::<i64, Vec<SourcedRawSample>>::new();
        for row in group.rows {
            let minute = row.timestamp - row.timestamp.rem_euclid(60);
            if Some(group_index) == current_group
                && current_start.is_some_and(|start| row.timestamp < start)
            {
                continue;
            }
            if ambiguous_minutes.contains(&minute)
                || boundary_minute_owners
                    .get(&minute)
                    .is_some_and(|owner| *owner != group_index)
            {
                continue;
            }
            by_minute.entry(minute).or_default().push(row);
        }
        for (minute, mut minute_rows) in by_minute {
            minute_rows.sort_by_key(|row| (row.source_timestamp, row.source_reset_at));
            let mut final_quota = None;
            let mut quota_conflicted = false;
            for value in minute_rows.iter().filter_map(|row| row.remaining_percent) {
                if final_quota.is_some_and(|previous| value != previous) {
                    quota_conflicted = true;
                    break;
                }
                final_quota = Some(value);
            }
            if quota_conflicted {
                continue;
            }
            let maximums = minute_rows.iter().fold(
                (0.0_f64, 0.0_f64, 0.0_f64, 0_u64, 0_u64, 0_u64),
                |mut max, row| {
                    max.0 = max.0.max(row.sol_dollars);
                    max.1 = max.1.max(row.terra_dollars);
                    max.2 = max.2.max(row.luna_dollars);
                    max.3 = max.3.max(row.sol_tokens);
                    max.4 = max.4.max(row.terra_tokens);
                    max.5 = max.5.max(row.luna_tokens);
                    max
                },
            );
            let dominant = minute_rows.into_iter().rev().find(|row| {
                row.sol_dollars >= maximums.0
                    && row.terra_dollars >= maximums.1
                    && row.luna_dollars >= maximums.2
                    && row.sol_tokens >= maximums.3
                    && row.terra_tokens >= maximums.4
                    && row.luna_tokens >= maximums.5
                    && final_quota.is_none_or(|remaining| row.remaining_percent == Some(remaining))
            });
            let Some(dominant) = dominant else {
                continue;
            };
            let mut sample = public_sample_from_raw(&dominant.row);
            sample.timestamp = minute;
            sample.reset_at = group.canonical_reset_at;
            canonical.push(CanonicalStorageSample {
                sample,
                source_timestamp: dominant.source_timestamp,
                source_reset_at: dominant.source_reset_at,
            });
        }
    }
    canonical.sort_by_key(|row| (row.sample.reset_at, row.sample.timestamp));
    if max_rows.is_some_and(|limit| canonical.len() > limit) {
        return Err(ReaderError::TooManyRows(canonical.len()));
    }
    let _ = observed_at;
    Ok(canonical)
}

fn clip_stale_reset_group_tails(groups: &mut [ResetGroup]) {
    let fixed_suffix_starts = groups
        .iter()
        .map(fixed_reset_transition_start)
        .collect::<Vec<_>>();
    let shadowed_moving = shadowed_moving_reset_groups(groups);
    for candidate_index in 0..groups.len() {
        if shadowed_moving[candidate_index] {
            continue;
        }
        let Some(_fixed_suffix_start) = fixed_suffix_starts[candidate_index] else {
            continue;
        };
        let candidate_reset = groups[candidate_index].canonical_reset_at;
        // A rolling deadline followed by a stable reset confirms one quota
        // cycle. Keep its complete observed prelude: the quota recovery at
        // the first row is the public boundary, while the fixed suffix is
        // confirmation rather than a later artificial boundary.
        let transition_start = groups[candidate_index].start;
        for prior in groups.iter_mut().take(candidate_index) {
            if prior
                .canonical_reset_at
                .saturating_add(RESET_AT_TOLERANCE_SECONDS)
                >= candidate_reset
            {
                continue;
            }
            prior.rows.retain(|row| row.timestamp < transition_start);
        }
    }
    for group in groups {
        if let Some(start) = group.rows.iter().map(|row| row.timestamp).min() {
            group.start = start;
        }
    }
}

fn fixed_reset_transition_start(group: &ResetGroup) -> Option<i64> {
    let mut previous = None::<&RawSample>;
    for row in group.rows.iter().filter(|row| {
        row.reset_at.abs_diff(group.canonical_reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
    }) {
        if let Some(anchor) = previous {
            let different_minute = anchor.timestamp.div_euclid(60) != row.timestamp.div_euclid(60);
            let same_reset_family =
                anchor.reset_at.abs_diff(row.reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64;
            if different_minute
                && same_reset_family
                && (anchor.remaining_percent.is_some() || row.remaining_percent.is_some())
                && !reset_coordinates_form_moving_step(
                    anchor.timestamp,
                    anchor.reset_at,
                    row.timestamp,
                    row.reset_at,
                )
            {
                return Some(anchor.timestamp);
            }
        }
        previous = Some(row);
    }
    None
}

fn remove_shadowed_moving_reset_groups(groups: &mut Vec<ResetGroup>) {
    let shadowed = shadowed_moving_reset_groups(groups);
    let mut index = 0;
    groups.retain(|_| {
        let keep = !shadowed[index];
        index += 1;
        keep
    });
}

fn shadowed_moving_reset_groups(groups: &[ResetGroup]) -> Vec<bool> {
    let mut shadowed = vec![false; groups.len()];
    let mut prior_quota_max_end = None::<i64>;
    let mut batch_start = 0;
    while batch_start < groups.len() {
        let start = groups[batch_start].start;
        let mut batch_end = batch_start + 1;
        while batch_end < groups.len() && groups[batch_end].start == start {
            batch_end += 1;
        }
        for index in batch_start..batch_end {
            let end = groups[index]
                .rows
                .iter()
                .map(|row| row.timestamp)
                .max()
                .unwrap_or(groups[index].start);
            let moving = groups[index].rows.windows(2).any(|pair| {
                reset_coordinates_form_moving_step(
                    pair[0].timestamp,
                    pair[0].reset_at,
                    pair[1].timestamp,
                    pair[1].reset_at,
                )
            });
            shadowed[index] = moving && prior_quota_max_end.is_some_and(|prior| prior > end);
        }
        for group in &groups[batch_start..batch_end] {
            if group.rows.iter().any(|row| row.remaining_percent.is_some()) {
                let end = group
                    .rows
                    .iter()
                    .map(|row| row.timestamp)
                    .max()
                    .unwrap_or(group.start);
                prior_quota_max_end = Some(prior_quota_max_end.map_or(end, |prior| prior.max(end)));
            }
        }
        batch_start = batch_end;
    }
    shadowed
}

fn reset_coordinates_form_moving_step(
    anchor_timestamp: i64,
    anchor_reset_at: i64,
    candidate_timestamp: i64,
    candidate_reset_at: i64,
) -> bool {
    if candidate_timestamp <= anchor_timestamp || candidate_reset_at <= anchor_reset_at {
        return false;
    }
    let timestamp_delta = candidate_timestamp - anchor_timestamp;
    let reset_delta = candidate_reset_at - anchor_reset_at;
    let anchor_horizon = anchor_reset_at.saturating_sub(anchor_timestamp);
    let candidate_horizon = candidate_reset_at.saturating_sub(candidate_timestamp);
    reset_delta <= MOVING_RESET_GROUP_MAX_DRIFT_SECONDS
        && anchor_horizon >= MOVING_RESET_MIN_HORIZON_SECONDS
        && candidate_horizon >= MOVING_RESET_MIN_HORIZON_SECONDS
        && anchor_horizon.abs_diff(candidate_horizon) <= MOVING_RESET_STEP_TOLERANCE_SECONDS as u64
        && reset_delta.abs_diff(timestamp_delta) <= MOVING_RESET_STEP_TOLERANCE_SECONDS as u64
}

fn reset_belongs_to_group(anchor: &RawSample, candidate: &RawSample) -> bool {
    if candidate.reset_at.abs_diff(anchor.reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64 {
        return true;
    }
    if candidate.timestamp == anchor.timestamp {
        return candidate.reset_at > anchor.reset_at
            && candidate.reset_at - anchor.reset_at <= MOVING_RESET_GROUP_MAX_DRIFT_SECONDS
            && quota_observations_agree(anchor, candidate)
            && (cumulative_vector_dominates(candidate, anchor)
                || cumulative_vector_dominates(anchor, candidate));
    }
    if candidate.timestamp <= anchor.timestamp || candidate.reset_at <= anchor.reset_at {
        return false;
    }
    let timestamp_delta = candidate.timestamp - anchor.timestamp;
    let reset_delta = candidate.reset_at - anchor.reset_at;
    let anchor_horizon = anchor.reset_at.saturating_sub(anchor.timestamp);
    let candidate_horizon = candidate.reset_at.saturating_sub(candidate.timestamp);
    reset_delta <= MOVING_RESET_GROUP_MAX_DRIFT_SECONDS
        && anchor_horizon >= MOVING_RESET_MIN_HORIZON_SECONDS
        && candidate_horizon >= MOVING_RESET_MIN_HORIZON_SECONDS
        && anchor_horizon.abs_diff(candidate_horizon) <= MOVING_RESET_STEP_TOLERANCE_SECONDS as u64
        && reset_delta.abs_diff(timestamp_delta) <= MOVING_RESET_STEP_TOLERANCE_SECONDS as u64
}

fn cumulative_vector_dominates(candidate: &RawSample, required: &RawSample) -> bool {
    candidate.sol_dollars >= required.sol_dollars
        && candidate.terra_dollars >= required.terra_dollars
        && candidate.luna_dollars >= required.luna_dollars
        && candidate.sol_tokens >= required.sol_tokens
        && candidate.terra_tokens >= required.terra_tokens
        && candidate.luna_tokens >= required.luna_tokens
}

fn quota_observations_agree(anchor: &RawSample, candidate: &RawSample) -> bool {
    match (anchor.remaining_percent, candidate.remaining_percent) {
        (Some(anchor), Some(candidate)) => anchor == candidate,
        _ => true,
    }
}

#[derive(Clone, Debug)]
struct RawModelTotal {
    model: String,
    total_tokens: u64,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    cache_write_input_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default)]
struct ModelProjection {
    v1: Vec<PublicDetailedModelUsage>,
    v3: Vec<PublicModelUsageV3>,
}

#[cfg(test)]
fn read_models(connection: &Connection) -> Result<Vec<PublicDetailedModelUsage>, ReaderError> {
    Ok(read_model_projection(connection)?.v1)
}

/// Read the newest durable model totals once, retaining both wire views.  The
/// v1 view is intentionally limited to the three legacy columns, whereas v3
/// carries every valid named model and preserves cache-write availability.
fn read_model_projection(connection: &Connection) -> Result<ModelProjection, ReaderError> {
    let (table, query) = if table_exists(connection, "session_model_totals")? {
        (
            "session_model_totals",
            "SELECT model, total_tokens, input_tokens, cached_input_tokens, output_tokens,
                    cache_write_input_tokens
             FROM session_model_totals ORDER BY model",
        )
    } else if table_exists(connection, "usage_model_history")? {
        (
            "usage_model_history",
            "SELECT history.model, history.total_tokens, history.input_tokens,
                    history.cached_input_tokens, history.output_tokens,
                    history.cache_write_input_tokens
             FROM usage_model_history AS history
             WHERE history.timestamp = (
                 SELECT MAX(candidate.timestamp)
                 FROM usage_model_history AS candidate
                 WHERE candidate.model = history.model
             )
             ORDER BY history.model",
        )
    } else {
        return Ok(ModelProjection::default());
    };
    let has_cache_write = table_has_column(connection, table, "cache_write_input_tokens")?;
    let query = if has_cache_write {
        query
    } else if table == "session_model_totals" {
        "SELECT model, total_tokens, input_tokens, cached_input_tokens, output_tokens
         FROM session_model_totals ORDER BY model"
    } else {
        "SELECT history.model, history.total_tokens, history.input_tokens,
                history.cached_input_tokens, history.output_tokens
         FROM usage_model_history AS history
         WHERE history.timestamp = (
             SELECT MAX(candidate.timestamp)
             FROM usage_model_history AS candidate
             WHERE candidate.model = history.model
         )
         ORDER BY history.model"
    };
    let mut statement = connection.prepare(query)?;
    let rows = statement.query_map([], |row| {
        let Some(model) = sql_text(row, 0) else {
            return Ok(None);
        };
        let Some(total_tokens) = sql_text(row, 1).and_then(|value| value.parse::<u64>().ok())
        else {
            return Ok(None);
        };
        let Some(input_tokens) = sql_text(row, 2).and_then(|value| value.parse::<u64>().ok())
        else {
            return Ok(None);
        };
        let Some(cached_input_tokens) =
            sql_text(row, 3).and_then(|value| value.parse::<u64>().ok())
        else {
            return Ok(None);
        };
        let Some(output_tokens) = sql_text(row, 4).and_then(|value| value.parse::<u64>().ok())
        else {
            return Ok(None);
        };
        let cache_write_input_tokens = if has_cache_write {
            let parsed = match sql_text_option(row, 5) {
                Some(None) => Some(None),
                Some(Some(value)) => value.parse::<u64>().ok().map(Some),
                None => None,
            };
            let Some(parsed) = parsed else {
                return Ok(None);
            };
            parsed
        } else {
            None
        };
        Ok(Some(RawModelTotal {
            model,
            total_tokens,
            input_tokens,
            cached_input_tokens,
            output_tokens,
            cache_write_input_tokens,
        }))
    })?;

    let mut totals = Vec::new();
    for row in rows {
        let Some(row) = row? else { continue };
        totals.push(row);
    }
    Ok(project_model_totals(totals))
}

/// Read only timestamped model totals for a bounded lifecycle domain.
///
/// `session_model_totals` is intentionally not used here: it has no timestamp
/// and therefore cannot prove that its current values belong to the selected
/// account interval.  The history sidecar is the only source that can bind the
/// model projection to the same time domain as usage samples.  For each model,
/// the newest valid row inside the union is selected, so A→B→A keeps the later
/// A interval visible while excluding the B interval in between.
fn read_model_projection_for_intervals(
    connection: &Connection,
    intervals: &ReadIntervals,
) -> Result<ModelProjection, ReaderError> {
    if intervals.is_unbounded() {
        return read_model_projection(connection);
    }
    if !table_exists(connection, "usage_model_history")? {
        return Ok(ModelProjection::default());
    }
    let has_cache_write = table_has_column(
        connection,
        "usage_model_history",
        "cache_write_input_tokens",
    )?;
    let query = if has_cache_write {
        "SELECT reset_at, timestamp, model, total_tokens, input_tokens,
                cached_input_tokens, output_tokens, cache_write_input_tokens
         FROM usage_model_history ORDER BY timestamp DESC, reset_at DESC, model ASC"
    } else {
        "SELECT reset_at, timestamp, model, total_tokens, input_tokens,
                cached_input_tokens, output_tokens
         FROM usage_model_history ORDER BY timestamp DESC, reset_at DESC, model ASC"
    };
    let mut statement = connection.prepare(query)?;
    let mut rows = statement.query([])?;
    let mut selected = BTreeMap::<String, RawModelTotal>::new();
    while let Some(row) = rows.next()? {
        let Some(timestamp) = sql_i64(row, 1) else {
            continue;
        };
        if !intervals.intersects_canonical_minute(timestamp) {
            continue;
        }
        let Some(model) = sql_text(row, 2) else {
            continue;
        };
        if selected.contains_key(&model) {
            continue;
        }
        let Some(total_tokens) = sql_text(row, 3).and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let Some(input_tokens) = sql_text(row, 4).and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let Some(cached_input_tokens) =
            sql_text(row, 5).and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let Some(output_tokens) = sql_text(row, 6).and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let cache_write_input_tokens = if has_cache_write {
            let parsed = match sql_text_option(row, 7) {
                Some(None) => Some(None),
                Some(Some(value)) => value.parse::<u64>().ok().map(Some),
                None => None,
            };
            let Some(parsed) = parsed else {
                continue;
            };
            parsed
        } else {
            None
        };
        selected.insert(
            model.clone(),
            RawModelTotal {
                model,
                total_tokens,
                input_tokens,
                cached_input_tokens,
                output_tokens,
                cache_write_input_tokens,
            },
        );
    }
    Ok(project_model_totals(selected.into_values()))
}

impl ReadIntervals {
    fn is_unbounded(&self) -> bool {
        self.0.len() == 1 && self.0[0].start_at.is_none() && self.0[0].end_at.is_none()
    }
}

fn project_model_totals(rows: impl IntoIterator<Item = RawModelTotal>) -> ModelProjection {
    let mut projection = ModelProjection::default();
    let mut names = std::collections::HashSet::new();
    for row in rows {
        if !is_valid_public_model_name(&row.model)
            || row.total_tokens == 0
            || row.cached_input_tokens > row.input_tokens
            || row.cache_write_input_tokens.is_some_and(|writes| {
                row.cached_input_tokens
                    .checked_add(writes)
                    .is_none_or(|discounted| discounted > row.input_tokens)
            })
            || !names.insert(row.model.clone())
        {
            // A malformed model row is local to that model.  Continue with
            // all other durable totals so one bad/unknown row cannot cause a
            // complete details 503.
            continue;
        }
        let (input_dollars, cached_input_dollars, output_dollars) = model_dollar_costs(
            &row.model,
            row.input_tokens,
            row.cached_input_tokens,
            row.output_tokens,
        );
        if matches!(row.model.as_str(), "SOL" | "TERRA" | "LUNA") {
            projection.v1.push(PublicDetailedModelUsage {
                name: row.model.clone(),
                // Cached input is already included in the durable input
                // total.  v1 exposes its ordinary component separately.
                input_tokens: row.input_tokens.saturating_sub(row.cached_input_tokens),
                cached_input_tokens: row.cached_input_tokens,
                output_tokens: row.output_tokens,
                input_dollars,
                cached_input_dollars,
                output_dollars,
            });
        }
        projection.v3.push(PublicModelUsageV3 {
            model: row.model.clone(),
            total_tokens: row.total_tokens,
            // Unlike v1, v3 carries the durable input total including cached
            // and cache-write components exactly as recorded.
            input_tokens: row.input_tokens,
            cached_input_tokens: row.cached_input_tokens,
            cache_write_input_tokens: row.cache_write_input_tokens,
            output_tokens: row.output_tokens,
            estimated_cost: model_v3_cost(&row),
        });
    }
    projection
        .v1
        .sort_by_key(|model| match model.name.as_str() {
            "SOL" => 0_u8,
            "TERRA" => 1,
            "LUNA" => 2,
            _ => 3,
        });
    projection
        .v1
        .truncate(codex_info_rest_contract::MAX_PUBLIC_MODELS);
    projection.v3.sort_by(|left, right| {
        let left_rank = public_model_order(&left.model);
        let right_rank = public_model_order(&right.model);
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.model.cmp(&right.model))
    });
    projection.v3.truncate(MAX_PUBLIC_MODELS_V3);
    projection
}

fn public_model_order(model: &str) -> u8 {
    match model {
        "SOL" => 0,
        "TERRA" => 1,
        "LUNA" => 2,
        "ASTRA" => 3,
        _ => 4,
    }
}

fn model_v3_cost(row: &RawModelTotal) -> Option<PublicModelCostV3> {
    if row.model == "ASTRA" {
        let writes = row.cache_write_input_tokens?;
        let ordinary_input_tokens = row
            .input_tokens
            .checked_sub(row.cached_input_tokens)?
            .checked_sub(writes)?;
        let (input_rate, cached_rate, write_rate, output_rate) = ASTRA_PRICE_PER_MILLION;
        let ordinary_input = ordinary_input_tokens as f64 * input_rate / 1_000_000.0;
        let cached_input = row.cached_input_tokens as f64 * cached_rate / 1_000_000.0;
        let cache_write_input = writes as f64 * write_rate / 1_000_000.0;
        let output = row.output_tokens as f64 * output_rate / 1_000_000.0;
        return Some(PublicModelCostV3 {
            price_version: ASTRA_PRICE_VERSION.to_owned(),
            ordinary_input_dollars: ordinary_input,
            cached_input_dollars: cached_input,
            cache_write_input_dollars: cache_write_input,
            output_dollars: output,
            total_dollars: ordinary_input + cached_input + cache_write_input + output,
        });
    }
    if !matches!(row.model.as_str(), "SOL" | "TERRA" | "LUNA")
        || row.cache_write_input_tokens != Some(0)
    {
        return None;
    }
    let (ordinary_input, cached_input, output) = model_dollar_costs(
        &row.model,
        row.input_tokens,
        row.cached_input_tokens,
        row.output_tokens,
    );
    Some(PublicModelCostV3 {
        price_version: LOCAL_ESTIMATE_PRICE_VERSION.to_owned(),
        ordinary_input_dollars: ordinary_input,
        cached_input_dollars: cached_input,
        cache_write_input_dollars: 0.0,
        output_dollars: output,
        total_dollars: ordinary_input + cached_input + output,
    })
}

fn model_dollar_costs(
    model: &str,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
) -> (f64, f64, f64) {
    let (input_rate, cached_rate, output_rate) = match model {
        "SOL" => SOL_PRICE_PER_MILLION,
        "TERRA" => TERRA_PRICE_PER_MILLION,
        "LUNA" => LUNA_PRICE_PER_MILLION,
        _ => return (0.0, 0.0, 0.0),
    };
    let ordinary_input_tokens = input_tokens.saturating_sub(cached_input_tokens);
    (
        ordinary_input_tokens as f64 * input_rate / 1_000_000.0,
        cached_input_tokens as f64 * cached_rate / 1_000_000.0,
        output_tokens as f64 * output_rate / 1_000_000.0,
    )
}

fn format_estimated_cost(models: &[PublicDetailedModelUsage]) -> String {
    let total = models
        .iter()
        .map(|model| model.input_dollars + model.cached_input_dollars + model.output_dollars)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .sum::<f64>();
    if !total.is_finite() || total < 0.0 {
        return "概算 —".to_owned();
    }
    let cents = (total * 100.0).round();
    if !cents.is_finite() || cents < 0.0 || cents > u128::MAX as f64 {
        return "概算 —".to_owned();
    }
    let cents = cents as u128;
    format!(
        "概算 ${}.{:02}",
        format_unsigned_count(cents / 100),
        cents % 100
    )
}

fn format_unsigned_count(value: u128) -> String {
    let mut reversed = String::new();
    for (index, character) in value.to_string().chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            reversed.push(',');
        }
        reversed.push(character);
    }
    reversed.chars().rev().collect()
}

fn read_confirmed_gaps(connection: &Connection) -> Result<Vec<PublicHistoryGap>, ReaderError> {
    if !table_exists(connection, "recorder_gap_ledger")? {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT gap_id, reset_at, start_at, end_at, reason
         FROM recorder_gap_ledger
         WHERE state = 'confirmed' AND reset_at IS NOT NULL
         ORDER BY reset_at, start_at, end_at, gap_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(PublicHistoryGap {
            gap_id: row.get(0)?,
            reset_at: row.get(1)?,
            start_at: row.get(2)?,
            end_at: row.get(3)?,
            reason: row.get(4)?,
        })
    })?;
    let mut gaps = Vec::new();
    let mut gap_ids = std::collections::HashSet::new();
    for row in rows {
        let gap = row?;
        if !valid_gap_id(&gap.gap_id)
            || !valid_gap_reason(&gap.reason)
            || !valid_public_timestamp(gap.reset_at)
            || !valid_public_timestamp(gap.start_at)
            || !valid_public_timestamp(gap.end_at)
            || gap.end_at < gap.start_at
            || !gap_ids.insert(gap.gap_id.clone())
        {
            // A single malformed ledger row must not discard otherwise
            // durable history.  The period projection below additionally
            // drops gaps that are outside their canonical period.
            continue;
        }
        gaps.push(gap);
    }
    Ok(gaps)
}

fn read_confirmed_gaps_for_intervals(
    connection: &Connection,
    intervals: &ReadIntervals,
) -> Result<Vec<PublicHistoryGap>, ReaderError> {
    Ok(read_confirmed_gaps(connection)?
        .into_iter()
        .filter(|gap| intervals.contains(gap.start_at) && intervals.contains(gap.end_at))
        .collect())
}

fn valid_public_timestamp(value: i64) -> bool {
    value > 0 && value <= 253_402_300_799
}

fn valid_gap_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_gap_reason(value: &str) -> bool {
    matches!(
        value,
        "daemon_stop_unrecoverable" | "reset_hint_expired" | "auth_epoch_tombstoned"
    )
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 0x0f) as usize] as char);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("codex-info-reader-{name}-{suffix}.sqlite3"))
    }

    fn make_db(path: &Path) {
        let connection = Connection::open(path).expect("fixture db");
        connection
            .execute_batch(
                "CREATE TABLE usage_history(
                    timestamp INTEGER NOT NULL, reset_at INTEGER NOT NULL,
                    remaining_percent REAL, sol_dollars REAL NOT NULL,
                    terra_dollars REAL NOT NULL, luna_dollars REAL NOT NULL,
                    sol_tokens INTEGER NOT NULL, terra_tokens INTEGER NOT NULL,
                    luna_tokens INTEGER NOT NULL
                );
                CREATE TABLE collection_generation(
                    singleton INTEGER PRIMARY KEY, data_generation TEXT NOT NULL,
                    reset_at INTEGER NOT NULL, window_seconds INTEGER NOT NULL,
                    collector_epoch TEXT, cycle_seq TEXT NOT NULL
                );
                INSERT INTO collection_generation VALUES(1,'7',1800000060,3600,NULL,'0');",
            )
            .expect("fixture schema");
        connection
            .execute(
                "INSERT INTO usage_history VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    1_800_000_000_i64,
                    1_800_000_060_i64,
                    50.0,
                    1.0,
                    2.0,
                    3.0,
                    10,
                    20,
                    30
                ],
            )
            .expect("fixture row");
    }

    fn active_thread_json(id: &str, updated_at: i64) -> String {
        serde_json::json!([active_thread_value(id, updated_at)]).to_string()
    }

    fn active_thread_value(id: &str, updated_at: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "updated_at": updated_at,
            "title": "one SOL thread",
            "parent_thread_id": null,
            "model": "gpt-5",
            "model_label": "SOL",
            "total_tokens": 12,
            "context_usage_tokens": 8,
            "context_window_tokens": 128,
            "created_at": updated_at - 60,
            "last_user_message_at": updated_at,
            "is_subagent": false,
            "depth": 0
        })
    }

    fn make_boundary_db(path: &Path, boundary: i64) {
        let reset_at = boundary + 3_600;
        let connection = Connection::open(path).expect("boundary fixture db");
        connection
            .execute_batch(
                "CREATE TABLE usage_history(
                    timestamp INTEGER NOT NULL, reset_at INTEGER NOT NULL,
                    remaining_percent REAL, sol_dollars REAL NOT NULL,
                    terra_dollars REAL NOT NULL, luna_dollars REAL NOT NULL,
                    sol_tokens INTEGER NOT NULL, terra_tokens INTEGER NOT NULL,
                    luna_tokens INTEGER NOT NULL
                );
                CREATE TABLE collection_generation(
                    singleton INTEGER PRIMARY KEY, data_generation TEXT NOT NULL,
                    reset_at INTEGER NOT NULL, window_seconds INTEGER NOT NULL,
                    collector_epoch TEXT, cycle_seq TEXT NOT NULL
                );
                CREATE TABLE usage_model_history(
                    reset_at INTEGER NOT NULL, timestamp INTEGER NOT NULL,
                    model TEXT NOT NULL, total_tokens TEXT NOT NULL,
                    input_tokens TEXT NOT NULL, cached_input_tokens TEXT NOT NULL,
                    output_tokens TEXT NOT NULL, cache_write_input_tokens TEXT,
                    model_set_complete INTEGER NOT NULL
                );",
            )
            .expect("boundary fixture schema");
        connection
            .execute(
                "INSERT INTO collection_generation
                 VALUES(1,'7',?1,86400,NULL,'0')",
                params![reset_at],
            )
            .expect("boundary generation");
        for (timestamp, remaining_percent, token_total) in [
            (boundary - 60, 80.0, 10_i64),
            (boundary, 70.0, 20_i64),
            (boundary + 60, 60.0, 30_i64),
        ] {
            connection
                .execute(
                    "INSERT INTO usage_history VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        timestamp,
                        reset_at,
                        remaining_percent,
                        token_total as f64 / 10.0,
                        token_total as f64 / 10.0,
                        token_total as f64 / 10.0,
                        token_total,
                        token_total,
                        token_total,
                    ],
                )
                .expect("boundary usage row");
            connection
                .execute(
                    "INSERT INTO usage_model_history VALUES(?1,?2,'SOL',?3,?3,'0','0',NULL,1)",
                    params![reset_at, timestamp, token_total.to_string()],
                )
                .expect("boundary model row");
        }
    }

    fn add_active_thread_table(path: &Path, json: &str, degraded: i64) {
        let connection = Connection::open(path).expect("active thread fixture db");
        connection
            .execute_batch(
                "CREATE TABLE active_thread_snapshot(
                    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                    observed_at INTEGER NOT NULL CHECK(observed_at>0),
                    threads_json TEXT NOT NULL,
                    acquisition_degraded INTEGER NOT NULL DEFAULT 0
                        CHECK(acquisition_degraded IN (0,1))
                );",
            )
            .expect("active thread schema");
        connection
            .execute(
                "INSERT INTO active_thread_snapshot(
                    singleton, observed_at, threads_json, acquisition_degraded
                 ) VALUES(1,?1,?2,?3)",
                params![1_800_000_060_i64, json, degraded],
            )
            .expect("active thread row");
    }

    fn add_partition_identity(path: &Path, identity: &StoragePartitionIdentity) {
        let connection = Connection::open(path).expect("partition fixture db");
        connection
            .pragma_update(None, "user_version", HISTORY_CANONICAL_SCHEMA_VERSION)
            .expect("canonical fixture schema version");
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS usage_model_history(
                    reset_at INTEGER NOT NULL,
                    timestamp INTEGER NOT NULL,
                    model TEXT NOT NULL
                );
                CREATE TABLE durable_state(
                    singleton INTEGER PRIMARY KEY,
                    data_generation INTEGER NOT NULL,
                    snapshot_json TEXT NOT NULL
                );
                CREATE UNIQUE INDEX usage_history_canonical_timestamp_idx
                    ON usage_history(timestamp);
                CREATE UNIQUE INDEX usage_model_history_canonical_timestamp_model_idx
                    ON usage_model_history(timestamp, model);
                CREATE TRIGGER usage_history_canonical_insert_guard
                    BEFORE INSERT ON usage_history WHEN 0 BEGIN SELECT 1; END;
                CREATE TRIGGER usage_history_canonical_update_guard
                    BEFORE UPDATE ON usage_history WHEN 0 BEGIN SELECT 1; END;
                CREATE TRIGGER usage_model_history_canonical_insert_guard
                    BEFORE INSERT ON usage_model_history WHEN 0 BEGIN SELECT 1; END;
                CREATE TRIGGER usage_model_history_canonical_update_guard
                    BEFORE UPDATE ON usage_model_history WHEN 0 BEGIN SELECT 1; END;
                CREATE TRIGGER durable_history_observation_insert_guard
                    BEFORE INSERT ON durable_state WHEN 0 BEGIN SELECT 1; END;
                CREATE TRIGGER durable_history_observation_update_guard
                    BEFORE UPDATE ON durable_state WHEN 0 BEGIN SELECT 1; END;
                CREATE TRIGGER usage_history_sidecar_update_guard
                    BEFORE UPDATE ON usage_history WHEN 0 BEGIN SELECT 1; END;
                CREATE TRIGGER usage_history_sidecar_delete_guard
                    BEFORE DELETE ON usage_history WHEN 0 BEGIN SELECT 1; END;
                CREATE TABLE storage_partition(
                    singleton INTEGER,
                    schema_version TEXT,
                    profile_scope_id TEXT,
                    account_scope_id TEXT,
                    storage_epoch TEXT,
                    partition_id TEXT,
                    login_id TEXT
                );",
            )
            .expect("partition identity schema");
        connection
            .execute(
                "INSERT INTO storage_partition(
                    singleton, schema_version, profile_scope_id, account_scope_id,
                    storage_epoch, partition_id
                 ) VALUES(1,?1,?2,?3,?4,?5)",
                params![
                    identity.schema_version,
                    identity.profile_scope_id,
                    identity.account_scope_id,
                    identity.storage_epoch.to_string(),
                    identity.partition_id,
                ],
            )
            .expect("partition identity row");
    }

    #[test]
    fn one_sol_active_thread_is_projected_from_the_singleton_row() {
        let path = temp_db("active-thread-one-sol");
        make_db(&path);
        let json = active_thread_json("thread-sol", 1_800_000_000);
        add_active_thread_table(&path, &json, 0);
        let snapshot = DbReader::open(&path)
            .expect("reader")
            .read_snapshot()
            .expect("active thread snapshot");
        assert!(!snapshot.has_pending_ranges);
        assert_eq!(snapshot.details.active_thread_count, 1);
        assert_eq!(snapshot.details.threads.len(), 1);
        assert_eq!(snapshot.details.threads[0].id, "thread-sol");
        assert_eq!(snapshot.details.threads[0].model_label, "SOL");
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn legacy_database_without_active_thread_table_is_empty() {
        let path = temp_db("active-thread-legacy-empty");
        make_db(&path);
        let snapshot = DbReader::open(&path)
            .expect("reader")
            .read_snapshot()
            .expect("legacy snapshot");
        assert_eq!(snapshot.details.active_thread_count, 0);
        assert!(snapshot.details.threads.is_empty());
        assert!(!snapshot.has_pending_ranges);
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn malformed_active_thread_row_is_local_to_threads() {
        let path = temp_db("active-thread-malformed");
        make_db(&path);
        let malformed = serde_json::json!([{
            "id": "duplicate",
            "updated_at": 1_800_000_000,
            "title": "valid",
            "parent_thread_id": null,
            "model": "gpt-5",
            "model_label": "SOL",
            "total_tokens": 1,
            "context_usage_tokens": 1,
            "context_window_tokens": 1,
            "created_at": 1_800_000_000,
            "last_user_message_at": 1_800_000_000,
            "is_subagent": false,
            "depth": 1025
        }])
        .to_string();
        add_active_thread_table(&path, &malformed, 0);
        let snapshot = DbReader::open(&path)
            .expect("reader")
            .read_snapshot()
            .expect("usage history remains readable");
        assert!(snapshot.has_pending_ranges);
        assert!(snapshot.details.history_samples.is_empty());
        assert_eq!(snapshot.history_samples_v3.len(), 1);
        assert_eq!(snapshot.history_samples_v3[0].model_source, "unavailable");
        assert_eq!(snapshot.details.active_thread_count, 0);
        assert!(snapshot.details.threads.is_empty());
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn duplicate_active_thread_json_key_is_local_to_threads() {
        let path = temp_db("active-thread-duplicate-key");
        make_db(&path);
        let duplicate = r#"[{"id":"one","id":"two","updated_at":1800000000,"title":"valid","parent_thread_id":null,"model":"gpt-5","model_label":"SOL","total_tokens":1,"context_usage_tokens":1,"context_window_tokens":1,"created_at":1800000000,"last_user_message_at":1800000000,"is_subagent":false,"depth":0}]"#;
        add_active_thread_table(&path, duplicate, 0);
        let snapshot = DbReader::open(&path)
            .expect("reader")
            .read_snapshot()
            .expect("usage history remains readable");
        assert!(snapshot.has_pending_ranges);
        assert!(snapshot.details.history_samples.is_empty());
        assert_eq!(snapshot.history_samples_v3.len(), 1);
        assert_eq!(snapshot.history_samples_v3[0].model_source, "unavailable");
        assert!(snapshot.details.threads.is_empty());
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn acquisition_degraded_retains_threads_and_marks_the_change_marker() {
        let path = temp_db("active-thread-degraded");
        make_db(&path);
        let json = active_thread_json("thread-sol", 1_800_000_000);
        add_active_thread_table(&path, &json, 1);
        let reader = DbReader::open(&path).expect("reader");
        let snapshot = reader.read_snapshot().expect("degraded snapshot");
        assert!(snapshot.has_pending_ranges);
        assert_eq!(snapshot.details.active_thread_count, 1);
        assert_eq!(snapshot.details.threads[0].id, "thread-sol");
        assert_eq!(
            reader.read_change_marker().expect("change marker"),
            Some(DbChangeMarker {
                generation: 7,
                has_pending_ranges: true,
            })
        );
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn opens_query_only_and_reads_back_the_connection_setting() {
        let path = temp_db("query-only");
        make_db(&path);
        let reader = DbReader::open(&path).expect("reader");
        assert!(reader.query_only_enabled().expect("query_only"));
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn partitioned_reader_requires_exact_storage_partition_identity() {
        let path = temp_db("partition-identity");
        make_db(&path);
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".to_owned(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 13,
            partition_id: "33".repeat(32),
        };
        add_partition_identity(&path, &identity);
        assert!(DbReader::open_partitioned(&path, &identity).is_ok());

        let mut wrong_account = identity.clone();
        wrong_account.account_scope_id = "44".repeat(32);
        assert!(DbReader::open_partitioned(&path, &wrong_account).is_err());

        let connection = Connection::open(&path).expect("partition fixture db");
        connection
            .execute(
                "INSERT INTO storage_partition(
                    singleton, schema_version, profile_scope_id, account_scope_id,
                    storage_epoch, partition_id
                 ) VALUES(1,?1,?2,?3,?4,?5)",
                params![
                    identity.schema_version,
                    identity.profile_scope_id,
                    identity.account_scope_id,
                    identity.storage_epoch.to_string(),
                    identity.partition_id,
                ],
            )
            .expect("duplicate fixture row");
        assert!(DbReader::open_partitioned(&path, &identity).is_err());
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn partitioned_reader_returns_only_a_valid_display_login_id() {
        let path = temp_db("partition-login-id");
        make_db(&path);
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".to_owned(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 13,
            partition_id: "33".repeat(32),
        };
        add_partition_identity(&path, &identity);
        let connection = Connection::open(&path).expect("partition fixture db");
        connection
            .execute(
                "UPDATE storage_partition SET login_id=?1 WHERE singleton=1",
                ["user@example.com"],
            )
            .expect("login id fixture");
        drop(connection);

        let reader = DbReader::open_partitioned(&path, &identity).expect("partitioned reader");
        assert_eq!(
            reader.partition_login_id().expect("login id").as_deref(),
            Some("user@example.com")
        );

        let connection = Connection::open(&path).expect("partition fixture db");
        connection
            .execute(
                "UPDATE storage_partition SET login_id=?1 WHERE singleton=1",
                ["invalid\nvalue"],
            )
            .expect("invalid login id fixture");
        drop(connection);
        assert!(reader.partition_login_id().is_err());
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn partitioned_reader_rechecks_identity_after_path_replacement() {
        let path = temp_db("partition-identity-replacement");
        let replacement = temp_db("partition-identity-replacement-other");
        make_db(&path);
        make_db(&replacement);
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".to_owned(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 13,
            partition_id: "33".repeat(32),
        };
        add_partition_identity(&path, &identity);
        let mut replacement_identity = identity.clone();
        replacement_identity.account_scope_id = "44".repeat(32);
        add_partition_identity(&replacement, &replacement_identity);

        let reader = DbReader::open_partitioned(&path, &identity).expect("partitioned reader");
        let moved = path.with_extension("sqlite3.original");
        fs::rename(&path, &moved).expect("move original partition");
        fs::copy(&replacement, &path).expect("replace partition path");
        assert!(reader.read_change_marker().is_err());
        assert!(reader.read_snapshot().is_err());

        fs::remove_file(path).expect("cleanup replacement path");
        fs::remove_file(moved).expect("cleanup original partition");
        fs::remove_file(replacement).expect("cleanup replacement source");
    }

    #[test]
    fn read_intervals_require_a_non_overlapping_non_empty_union() {
        assert!(ReadIntervals::new(Vec::new()).is_err());
        assert!(ReadIntervals::new(vec![
            ReadInterval::new(None, Some(100)).expect("first interval"),
            ReadInterval::new(Some(50), None).expect("second interval"),
        ])
        .is_err());

        let intervals = ReadIntervals::new(vec![
            ReadInterval::new(Some(200), None).expect("later interval"),
            ReadInterval::new(None, Some(100)).expect("earlier interval"),
        ])
        .expect("disjoint intervals");
        assert_eq!(
            intervals.intervals(),
            &[
                ReadInterval {
                    start_at: None,
                    end_at: Some(100),
                },
                ReadInterval {
                    start_at: Some(200),
                    end_at: None,
                },
            ]
        );
        assert!(intervals.contains(99));
        assert!(!intervals.contains(100));
        assert!(!intervals.contains(199));
        assert!(intervals.contains(200));
    }

    #[test]
    fn bounded_reader_owns_boundary_rows_and_all_projection_surfaces() {
        let path = temp_db("lifecycle-boundary");
        let boundary = 1_800_000_000_i64;
        make_boundary_db(&path, boundary);
        let threads_json = serde_json::json!([
            active_thread_value("old-thread", boundary - 60),
            active_thread_value("current-thread", boundary + 60),
        ])
        .to_string();
        add_active_thread_table(&path, &threads_json, 0);

        let old_intervals = ReadIntervals::new(vec![
            ReadInterval::new(None, Some(boundary)).expect("old interval")
        ])
        .expect("old lifecycle domain");
        let current_intervals = ReadIntervals::new(vec![
            ReadInterval::new(Some(boundary), None).expect("current interval")
        ])
        .expect("current lifecycle domain");
        let old = DbReader::open_with_intervals(&path, old_intervals)
            .expect("old reader")
            .read_snapshot()
            .expect("old snapshot");
        let current = DbReader::open_with_intervals(&path, current_intervals)
            .expect("current reader")
            .read_snapshot()
            .expect("current snapshot");

        assert_eq!(old.details.observed_at, Some(boundary - 60));
        assert_eq!(
            old.details
                .quota
                .as_ref()
                .map(|quota| quota.remaining_percent),
            Some(80.0)
        );
        assert_eq!(old.details.active_thread_count, 0);
        assert!(old.details.threads.is_empty());
        assert!(old.details.history_samples.is_empty());
        assert_eq!(old.history_samples_v3.len(), 1);
        assert!(old
            .history_samples_v3
            .iter()
            .all(|sample| sample.timestamp < boundary));
        assert!(old
            .details
            .history_periods
            .iter()
            .all(|period| { period.start_at < boundary && period.end_at < boundary }));
        assert_eq!(old.models_v3[0].total_tokens, 10);

        assert_eq!(current.details.observed_at, Some(boundary + 60));
        assert_eq!(
            current
                .details
                .quota
                .as_ref()
                .map(|quota| quota.remaining_percent),
            Some(60.0)
        );
        assert_eq!(current.details.active_thread_count, 2);
        assert!(current
            .details
            .threads
            .iter()
            .any(|thread| thread.id == "current-thread"));
        assert!(current
            .details
            .threads
            .iter()
            .any(|thread| thread.id == "old-thread"));
        assert!(current.details.history_samples.is_empty());
        assert_eq!(current.history_samples_v3.len(), 2);
        assert!(current
            .history_samples_v3
            .iter()
            .all(|sample| sample.timestamp >= boundary));
        assert!(current
            .details
            .history_periods
            .iter()
            .all(|period| { period.start_at >= boundary && period.end_at >= boundary }));
        assert_eq!(current.models_v3[0].total_tokens, 30);
        assert_ne!(old.data_hash, current.data_hash);

        // A→B→A is represented as a union, not as one stale upper-bound
        // clip.  The exact-boundary row is B-owned here, while the later A
        // row remains readable.
        let reactivated_intervals = ReadIntervals::new(vec![
            ReadInterval::new(None, Some(boundary)).expect("first A interval"),
            ReadInterval::new(Some(boundary + 60), None).expect("second A interval"),
        ])
        .expect("reactivated lifecycle domain");
        let reactivated = DbReader::open_with_intervals(&path, reactivated_intervals)
            .expect("reactivated reader")
            .read_snapshot()
            .expect("reactivated snapshot");
        assert_eq!(reactivated.details.observed_at, Some(boundary + 60));
        assert!(reactivated.details.history_samples.is_empty());
        assert_eq!(reactivated.history_samples_v3.len(), 2);
        assert!(reactivated
            .history_samples_v3
            .iter()
            .any(|sample| sample.timestamp == boundary - 60));
        assert!(reactivated
            .history_samples_v3
            .iter()
            .any(|sample| sample.timestamp == boundary + 60));
        assert!(!reactivated
            .history_samples_v3
            .iter()
            .any(|sample| sample.timestamp == boundary));
        assert_eq!(reactivated.models_v3[0].total_tokens, 30);

        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn subminute_account_activation_keeps_exact_rows_in_a_valid_minute_projection() {
        let path = temp_db("subminute-lifecycle-boundary");
        let boundary = 1_800_000_017_i64;
        let visible_boundary = boundary - boundary.rem_euclid(60);
        make_boundary_db(&path, boundary);
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".to_owned(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 13,
            partition_id: "33".repeat(32),
        };
        let connection = Connection::open(&path).expect("subminute fixture db");
        connection
            .execute(
                "DELETE FROM usage_history WHERE timestamp != ?1",
                [boundary],
            )
            .expect("retain first owned history observation");
        connection
            .execute(
                "UPDATE usage_history SET timestamp=?1 WHERE timestamp=?2",
                params![visible_boundary, boundary],
            )
            .expect("canonicalize history minute");
        connection
            .execute(
                "DELETE FROM usage_model_history WHERE timestamp != ?1",
                [boundary],
            )
            .expect("retain first owned model observation");
        connection
            .execute(
                "UPDATE usage_model_history SET timestamp=?1 WHERE timestamp=?2",
                params![visible_boundary, boundary],
            )
            .expect("canonicalize model minute");
        drop(connection);
        add_partition_identity(&path, &identity);
        let intervals = ReadIntervals::new(vec![
            ReadInterval::new(Some(boundary), None).expect("current interval")
        ])
        .expect("current lifecycle domain");

        let snapshot = DbReader::open_partitioned_with_intervals(&path, &identity, intervals)
            .expect("bounded reader")
            .read_snapshot()
            .expect("subminute activation snapshot");

        snapshot
            .details
            .validate()
            .expect("valid public projection");
        assert_eq!(snapshot.details.observed_at, Some(visible_boundary));
        assert!(snapshot.details.authenticated);
        assert!(snapshot
            .details
            .history_samples
            .iter()
            .all(|sample| sample.timestamp >= visible_boundary));
        assert!(snapshot
            .details
            .history_periods
            .iter()
            .all(|period| period.start_at >= visible_boundary));
        assert_eq!(snapshot.models_v3[0].total_tokens, 20);

        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn read_does_not_create_wal_or_change_file_bytes() {
        let path = temp_db("no-side-effects");
        make_db(&path);
        let before = fs::read(&path).expect("before");
        let sidecar_before = [
            path.with_extension("sqlite3-wal"),
            path.with_extension("sqlite3-shm"),
            path.with_extension("sqlite3-journal"),
        ]
        .map(|path| (path.clone(), path.exists()));
        let snapshot = DbReader::open(&path)
            .expect("reader")
            .read_snapshot()
            .expect("snapshot");
        assert_eq!(snapshot.generation, 7);
        assert_eq!(before, fs::read(&path).expect("after"));
        for (path, existed) in sidecar_before {
            assert_eq!(
                path.exists(),
                existed,
                "sidecar changed: {}",
                path.display()
            );
        }
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn snapshot_reports_only_incomplete_ranges_as_pending() {
        let path = temp_db("pending-ranges");
        make_db(&path);
        let reader = DbReader::open(&path).expect("reader");
        assert!(
            !reader
                .read_snapshot()
                .expect("clean snapshot")
                .has_pending_ranges
        );

        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute_batch(
                "CREATE TABLE session_pending_ranges(
                    source_id TEXT NOT NULL, range_start INTEGER NOT NULL,
                    complete INTEGER NOT NULL
                );
                INSERT INTO session_pending_ranges VALUES('complete', 0, 1);",
            )
            .expect("pending fixture");
        let complete = reader
            .read_snapshot()
            .expect("complete diagnostic snapshot");
        assert!(!complete.has_pending_ranges);
        connection
            .execute(
                "INSERT INTO session_pending_ranges VALUES('incomplete', 1, 0)",
                [],
            )
            .expect("incomplete fixture");
        let pending = reader.read_snapshot().expect("pending snapshot");
        assert!(pending.has_pending_ranges);
        assert!(pending.details.history_samples.is_empty());
        assert_eq!(pending.history_samples_v3.len(), 1);
        assert_eq!(pending.history_samples_v3[0].model_source, "unavailable");

        connection
            .execute("DELETE FROM session_pending_ranges", [])
            .expect("clear pending fixture");
        assert!(
            !reader
                .read_snapshot()
                .expect("recovered snapshot")
                .has_pending_ranges
        );
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn change_marker_tracks_committed_generation_and_incomplete_work() {
        let path = temp_db("change-marker");
        make_db(&path);
        let reader = DbReader::open(&path).expect("reader");
        assert_eq!(
            reader.read_change_marker().expect("initial marker"),
            Some(DbChangeMarker {
                generation: 7,
                has_pending_ranges: false,
            })
        );

        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute_batch(
                "CREATE TABLE session_pending_ranges(
                    source_id TEXT NOT NULL, range_start INTEGER NOT NULL,
                    complete INTEGER NOT NULL
                );
                INSERT INTO session_pending_ranges VALUES('complete', 0, 1);",
            )
            .expect("complete diagnostic");
        assert_eq!(
            reader.read_change_marker().expect("complete marker"),
            Some(DbChangeMarker {
                generation: 7,
                has_pending_ranges: false,
            })
        );

        connection
            .execute(
                "INSERT INTO session_pending_ranges VALUES('incomplete', 1, 0)",
                [],
            )
            .expect("incomplete work");
        assert_eq!(
            reader.read_change_marker().expect("pending marker"),
            Some(DbChangeMarker {
                generation: 7,
                has_pending_ranges: true,
            })
        );
        connection
            .execute(
                "UPDATE collection_generation SET data_generation='8' WHERE singleton=1",
                [],
            )
            .expect("advance generation");
        assert_eq!(
            reader.read_change_marker().expect("advanced marker"),
            Some(DbChangeMarker {
                generation: 8,
                has_pending_ranges: true,
            })
        );
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn malformed_row_is_skipped_without_rejecting_valid_history() {
        let path = temp_db("malformed");
        make_db(&path);
        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute(
                "INSERT INTO usage_history VALUES(1,2,NULL,-1,0,0,0,0,0)",
                [],
            )
            .expect("malformed fixture");
        let snapshot = DbReader::open(&path)
            .expect("reader")
            .read_snapshot()
            .expect("one malformed row must not reject the candidate");
        assert_eq!(snapshot.details.state, PublicState::Ready);
        assert!(snapshot.details.history_samples.is_empty());
        assert_eq!(snapshot.history_samples_v3.len(), 1);
        assert_eq!(snapshot.history_samples_v3[0].model_source, "unavailable");
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn malformed_history_model_row_is_local_to_its_timestamp_group() {
        let path = temp_db("malformed-model-history");
        make_db(&path);
        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute_batch(
                "CREATE TABLE usage_model_history(
                    reset_at INTEGER NOT NULL, timestamp INTEGER NOT NULL,
                    model TEXT NOT NULL, total_tokens TEXT NOT NULL,
                    input_tokens TEXT NOT NULL, cached_input_tokens TEXT NOT NULL,
                    output_tokens TEXT NOT NULL, cache_write_input_tokens TEXT,
                    model_set_complete INTEGER NOT NULL
                );
                CREATE TABLE durable_state(
                    singleton INTEGER PRIMARY KEY, data_generation INTEGER NOT NULL,
                    snapshot_json TEXT NOT NULL
                );
                INSERT INTO usage_model_history VALUES
                    (1800000060,1800000000,'SOL','110','100','40','10','0',1),
                    (1800000060,1800000000,'BROKEN','1','1','99','0','0',1);",
            )
            .expect("history model schema");
        let provenance = serde_json::json!({
            "kind": "codex-info-usage-observation-v1",
            "timestamp": 1_800_000_000_i64,
            "reset_at": 1_800_000_060_i64,
            "remaining_percent": 50.0,
            "model_source": "legacy-unknown",
        })
        .to_string();
        connection
            .execute(
                "INSERT INTO durable_state(singleton,data_generation,snapshot_json)
                 VALUES(2,1800000000,?1)",
                params![provenance],
            )
            .expect("explicit legacy provenance");
        let snapshot = DbReader::open(&path)
            .expect("reader")
            .read_snapshot()
            .expect("one malformed model row must not reject valid history");
        assert_eq!(snapshot.details.state, PublicState::Ready);
        let history = &snapshot.history_samples_v3[0];
        assert_eq!(history.model_source, "legacy-unknown");
        assert!(!history.models_complete);
        assert_eq!(
            history
                .models
                .as_ref()
                .unwrap()
                .iter()
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>(),
            vec!["SOL"]
        );
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn overlapping_reset_ranges_keep_samples_valid_and_periods_strictly_ordered() {
        let samples = vec![
            PublicHistorySample {
                timestamp: 1_800_000_120,
                reset_at: 1_800_001_000,
                remaining_percent: Some(49.0),
                sol_dollars: 2.0,
                terra_dollars: 3.0,
                luna_dollars: 4.0,
                sol_tokens: 11,
                terra_tokens: 21,
                luna_tokens: 31,
            },
            PublicHistorySample {
                timestamp: 1_800_000_060,
                reset_at: 1_800_001_200,
                remaining_percent: Some(50.0),
                sol_dollars: 1.0,
                terra_dollars: 2.0,
                luna_dollars: 3.0,
                sol_tokens: 10,
                terra_tokens: 20,
                luna_tokens: 30,
            },
        ];
        let periods = history_periods(&samples, 1_800_000_120, None, 0);
        assert_eq!(periods[0].reset_at, 1_800_001_000);
        assert_eq!(periods[1].reset_at, 1_800_001_200);
        assert!(periods[0].start_at > periods[1].start_at);
        assert_eq!(periods[1].end_at, 1_800_000_060);

        let details = PublicDetails {
            state: PublicState::Ready,
            observed_at: Some(1_800_000_120),
            authenticated: true,
            history_periods: periods,
            history_samples: samples,
            ..PublicDetails::default()
        };
        details
            .validate()
            .expect("overlap must not make a valid sample unowned");
    }

    #[test]
    fn current_period_end_tracks_unrounded_observation_boundary() {
        let samples = vec![PublicHistorySample {
            timestamp: 1_800_000_120,
            reset_at: 1_800_001_000,
            remaining_percent: Some(49.0),
            sol_dollars: 2.0,
            terra_dollars: 3.0,
            luna_dollars: 4.0,
            sol_tokens: 11,
            terra_tokens: 21,
            luna_tokens: 31,
        }];
        let observed_at = 1_800_000_123;
        let periods = history_periods(&samples, observed_at, Some(1_800_001_000), 604_800);
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].end_at, observed_at);

        let details = PublicDetails {
            state: PublicState::Ready,
            observed_at: Some(observed_at),
            authenticated: true,
            history_periods: periods,
            history_samples: samples,
            ..PublicDetails::default()
        };
        details
            .validate()
            .expect("unrounded recorder observation must remain contract-valid");
    }

    #[test]
    fn model_input_excludes_cached_component_and_uses_established_pricing() {
        let path = temp_db("model-components");
        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute_batch(
                "CREATE TABLE session_model_totals(
                    model TEXT NOT NULL, total_tokens TEXT NOT NULL,
                    input_tokens TEXT NOT NULL, cached_input_tokens TEXT NOT NULL,
                    output_tokens TEXT NOT NULL
                );
                INSERT INTO session_model_totals VALUES('SOL','110','100','40','10');
                INSERT INTO session_model_totals VALUES('UNKNOWN','1','1','0','0');",
            )
            .expect("model schema");
        let models = read_models(&connection).expect("models");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].input_tokens, 60);
        assert_eq!(models[0].cached_input_tokens, 40);
        assert_eq!(models[0].output_tokens, 10);
        assert_eq!(models[0].input_dollars, 0.0003);
        assert_eq!(models[0].cached_input_dollars, 0.00002);
        assert_eq!(models[0].output_dollars, 0.0003);
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn v1_and_v3_model_views_match_root_selection_and_cache_write_semantics() {
        let path = temp_db("model-v3-components");
        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute_batch(
                "CREATE TABLE session_model_totals(
                    model TEXT PRIMARY KEY, total_tokens TEXT NOT NULL,
                    input_tokens TEXT NOT NULL, cached_input_tokens TEXT NOT NULL,
                    output_tokens TEXT NOT NULL, cache_write_input_tokens TEXT
                );
                INSERT INTO session_model_totals VALUES
                    ('ASTRA','1100000','1000000','200000','100000','100000'),
                    ('SOL','110','100','40','10','0'),
                    ('gpt-7-nova','30','20','5','10','3');",
            )
            .expect("model schema");
        let projection = read_model_projection(&connection).expect("model projection");

        // The root v1 projection remains the three legacy models only.
        assert_eq!(
            projection
                .v1
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["SOL"]
        );
        assert_eq!(projection.v1[0].input_tokens, 60);
        assert_eq!(projection.v1[0].cached_input_tokens, 40);

        // The root v3 projection retains every positive, valid model in its
        // canonical known-model-then-additional order and carries durable
        // input/cache-write facts
        // without recomputing total_tokens from the components.
        assert_eq!(
            projection
                .v3
                .iter()
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>(),
            vec!["SOL", "ASTRA", "gpt-7-nova"]
        );
        let astra = &projection.v3[1];
        assert_eq!(astra.total_tokens, 1_100_000);
        assert_eq!(astra.input_tokens, 1_000_000);
        assert_eq!(astra.cached_input_tokens, 200_000);
        assert_eq!(astra.cache_write_input_tokens, Some(100_000));
        assert_eq!(astra.output_tokens, 100_000);
        let astra_cost = astra.estimated_cost.as_ref().expect("ASTRA pricing");
        assert_eq!(astra_cost.price_version, "ASTRA_USER_2026-09-05");
        assert_eq!(astra_cost.ordinary_input_dollars, 7.0);
        assert_eq!(astra_cost.cached_input_dollars, 0.2);
        assert_eq!(astra_cost.cache_write_input_dollars, 1.25);
        assert_eq!(astra_cost.output_dollars, 5.0);
        assert_eq!(astra_cost.total_dollars, 13.45);
        assert_eq!(
            projection.v3[0]
                .estimated_cost
                .as_ref()
                .unwrap()
                .price_version,
            "LOCAL_ESTIMATE_V1_2026-08-14"
        );
        assert!(projection.v3[2].estimated_cost.is_none());
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn reset_jitter_is_canonicalized_without_inventing_model_provenance() {
        let path = temp_db("jittered-current");
        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute_batch(
                "CREATE TABLE usage_history(
                    timestamp INTEGER NOT NULL, reset_at INTEGER NOT NULL,
                    remaining_percent REAL, sol_dollars REAL NOT NULL,
                    terra_dollars REAL NOT NULL, luna_dollars REAL NOT NULL,
                    sol_tokens INTEGER NOT NULL, terra_tokens INTEGER NOT NULL,
                    luna_tokens INTEGER NOT NULL
                );
                CREATE TABLE collection_generation(
                    singleton INTEGER PRIMARY KEY, data_generation TEXT NOT NULL,
                    reset_at INTEGER NOT NULL, window_seconds INTEGER NOT NULL,
                    collector_epoch TEXT, cycle_seq TEXT NOT NULL
                );
                CREATE TABLE usage_model_history(
                    reset_at INTEGER NOT NULL, timestamp INTEGER NOT NULL,
                    model TEXT NOT NULL, total_tokens TEXT NOT NULL,
                    input_tokens TEXT NOT NULL, cached_input_tokens TEXT NOT NULL,
                    output_tokens TEXT NOT NULL, cache_write_input_tokens TEXT,
                    model_set_complete INTEGER NOT NULL
                );
                INSERT INTO collection_generation VALUES(1,'42',1800000604,86400,NULL,'0');",
            )
            .expect("fixture schema");
        for (timestamp, reset_at, token) in [
            (1_800_000_000_i64, 1_800_000_600_i64, 10_i64),
            (1_800_000_060_i64, 1_800_000_602_i64, 20_i64),
            (1_800_000_120_i64, 1_800_000_604_i64, 30_i64),
        ] {
            connection
                .execute(
                    "INSERT INTO usage_history VALUES(?1,?2,50.0,1.0,2.0,3.0,?3,2,3)",
                    params![timestamp, reset_at, token],
                )
                .expect("usage row");
        }
        connection
            .execute(
                "INSERT INTO usage_model_history VALUES(1800000604,1800000120,'SOL','30','20','5','5',NULL,1)",
                [],
            )
            .expect("model row");
        let snapshot = DbReader::open(&path)
            .expect("reader")
            .read_snapshot()
            .expect("jittered clone must be readable");
        assert_eq!(snapshot.details.state, PublicState::Ready);
        assert!(!snapshot.details.models.is_empty());
        assert_eq!(snapshot.details.history_periods.len(), 1);
        assert!(snapshot.details.history_samples.is_empty());
        assert_eq!(snapshot.history_samples_v3.len(), 3);
        assert!(snapshot
            .history_samples_v3
            .iter()
            .all(|sample| sample.model_source == "unavailable"));
        assert!(snapshot
            .history_samples_v3
            .iter()
            .all(|sample| !sample.models_complete));
        assert!(snapshot
            .history_samples_v3
            .iter()
            .all(|sample| sample.models.is_none()));
        assert!(snapshot
            .history_samples_v2
            .iter()
            .all(|sample| sample.model_source == "unavailable"));
        assert!(snapshot
            .history_samples_v3
            .iter()
            .all(|sample| sample.reset_at == 1_800_000_604));
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn public_history_uses_exact_saved_keys_and_never_reconstructs_models() {
        let path = temp_db("public-history-boundary");
        make_db(&path);
        let reset_at = 1_800_000_600_i64;
        let timestamps = [
            1_800_000_000_i64,
            1_800_000_060,
            1_800_000_120,
            1_800_000_180,
            1_800_000_300,
        ];
        let connection = Connection::open(&path).expect("fixture db");
        connection
            .execute("DELETE FROM usage_history", [])
            .expect("clear seed row");
        connection
            .execute(
                "UPDATE collection_generation SET reset_at = ?1, window_seconds = 86400
                 WHERE singleton = 1",
                params![reset_at],
            )
            .expect("set history period");
        connection
            .execute_batch(
                "CREATE TABLE durable_state(
                    singleton INTEGER PRIMARY KEY, data_generation INTEGER NOT NULL,
                    data_hash TEXT NOT NULL, snapshot_json TEXT NOT NULL
                );
                CREATE TABLE usage_model_history(
                    reset_at INTEGER NOT NULL, timestamp INTEGER NOT NULL,
                    model TEXT NOT NULL, total_tokens TEXT NOT NULL,
                    input_tokens TEXT NOT NULL, cached_input_tokens TEXT NOT NULL,
                    output_tokens TEXT NOT NULL, cache_write_input_tokens TEXT,
                    model_set_complete INTEGER NOT NULL
                );
                CREATE TABLE session_timeline_recoveries(
                    recovery_id TEXT PRIMARY KEY, payload_json TEXT NOT NULL,
                    applied_generation TEXT NOT NULL
                );",
            )
            .expect("history sidecars");
        for (index, timestamp) in timestamps.into_iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO usage_history VALUES(?1,?2,?3,1.0,0.0,0.0,10,0,0)",
                    params![timestamp, reset_at, 90.0 - index as f64],
                )
                .expect("usage row");
        }
        let observation_json = |timestamp: i64, observation_reset: i64, source: &str| {
            serde_json::json!({
                "kind": "codex-info-usage-observation-v1",
                "timestamp": timestamp,
                "reset_at": observation_reset,
                "remaining_percent": 90.0,
                "model_source": source,
            })
            .to_string()
        };
        // This source is deliberately one second off the raw reset key.  The
        // old tolerance join attached it; the public reader must not.
        let mismatched = observation_json(timestamps[0], reset_at + 1, "confirmed");
        let reconstructed = observation_json(timestamps[1], reset_at, "reconstructed-from-session");
        let unavailable = observation_json(timestamps[2], reset_at, "unavailable");
        let legacy = observation_json(timestamps[3], reset_at, "legacy-unknown");
        let unknown = observation_json(timestamps[4], reset_at, "mystery");
        for (singleton, timestamp, json) in [
            (2_i64, timestamps[0], mismatched),
            (3_i64, timestamps[1], reconstructed),
            (4_i64, timestamps[2], unavailable),
            (5_i64, timestamps[3], legacy),
            (6_i64, timestamps[4], unknown),
        ] {
            connection
                .execute(
                    "INSERT INTO durable_state VALUES(?1,?2,?3,?4)",
                    params![singleton, timestamp, format!("hash-{singleton}"), json],
                )
                .expect("durable observation");
        }
        connection
            .execute_batch(&format!(
                "INSERT INTO usage_model_history VALUES
                    ({mismatch_reset},{t0},'SOL','10','10','0','0',NULL,1),
                    ({reset_at},{t1},'SOL','20','20','0','0',NULL,1),
                    ({reset_at},{t2},'SOL','30','30','0','0',NULL,1),
                    ({reset_at},{t3},'gpt-custom','40','40','0','0',NULL,1),
                    ({reset_at},{t4},'SOL','50','50','0','0',NULL,1);",
                mismatch_reset = reset_at + 1,
                t0 = timestamps[0],
                t1 = timestamps[1],
                t2 = timestamps[2],
                t3 = timestamps[3],
                t4 = timestamps[4],
            ))
            .expect("model groups");
        let timeline_range = (
            "test-root".to_owned(),
            "session.jsonl".to_owned(),
            "1".to_owned(),
            "2".to_owned(),
            "0".to_owned(),
            "1".to_owned(),
            "00000000000000000000000000000001".to_owned(),
            "1".to_owned(),
            "00000000000000000000000000000001".to_owned(),
            "b".repeat(64),
        );
        let timeline_model = ("SOL".to_owned(), 1_u64, 1_u64, 0_u64, 0_u64, Some(0_u64));
        let timeline_payload = (
            "c".repeat(64),
            reset_at,
            86_400,
            6,
            timestamps[3] + 120,
            vec![("SOL".to_owned(), 10, 10, 0, 0, Some(0))],
            vec![timeline_range],
            vec![(
                timestamps[3] + 60,
                vec![timeline_model.clone()],
                (1.0, 0.0, 0.0),
            )],
            vec![timeline_model],
            (1.0, 0.0, 0.0),
        );
        let timeline_json = serde_json::to_string(&timeline_payload).expect("timeline payload");
        let timeline_id = hex_lower(Sha256::digest(timeline_json.as_bytes()).as_ref());
        connection
            .execute(
                "INSERT INTO session_timeline_recoveries VALUES(?1,?2,'7')",
                params![timeline_id, timeline_json],
            )
            .expect("timeline sidecar");

        let snapshot = DbReader::open(&path)
            .expect("reader")
            .read_snapshot()
            .expect("public history");
        assert_eq!(snapshot.history_samples_v3.len(), timestamps.len());
        assert!(!snapshot
            .history_samples_v3
            .iter()
            .any(|sample| sample.timestamp == timestamps[3] + 60));
        let mismatched = &snapshot.history_samples_v3[0];
        assert_eq!(mismatched.model_source, "unavailable");
        assert!(mismatched.models.is_none());
        assert!(!mismatched.models_complete);

        let reconstructed = &snapshot.history_samples_v3[1];
        assert_eq!(reconstructed.model_source, "reconstructed-from-session");
        assert!(reconstructed.models.is_none());
        assert!(!reconstructed.models_complete);
        assert_eq!(reconstructed.remaining_percent, Some(89.0));
        assert!(snapshot.history_samples_v2[..3].iter().all(|sample| {
            sample.sol_dollars.is_none()
                && sample.terra_dollars.is_none()
                && sample.luna_dollars.is_none()
                && sample.sol_tokens.is_none()
                && sample.terra_tokens.is_none()
                && sample.luna_tokens.is_none()
        }));

        let unavailable = &snapshot.history_samples_v3[2];
        assert_eq!(unavailable.model_source, "unavailable");
        assert!(unavailable.models.is_none());
        assert!(!unavailable.models_complete);
        assert_eq!(unavailable.remaining_percent, Some(88.0));

        let legacy = &snapshot.history_samples_v3[3];
        assert_eq!(legacy.model_source, "legacy-unknown");
        assert!(!legacy.models_complete);
        let custom = legacy
            .models
            .as_ref()
            .and_then(|models| models.iter().find(|model| model.model == "gpt-custom"))
            .expect("same-key legacy model group");
        assert_eq!(custom.total_tokens, 40);
        assert_eq!(custom.total_dollars, None);
        let unknown = &snapshot.history_samples_v3[4];
        assert_eq!(unknown.model_source, "unavailable");
        assert!(unknown.models.is_none());
        assert!(snapshot.history_samples_v2[4].sol_tokens.is_none());
        assert_eq!(snapshot.details.history_samples.len(), 1);
        assert_eq!(snapshot.details.history_samples[0].timestamp, timestamps[3]);

        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn retained_reset_tail_is_clipped_before_real_boundary_and_current_label_uses_reset() {
        let stale_reset = 1_789_300_251;
        let next_period_reset = 1_789_437_490;
        let real_reset = 1_789_623_591;
        let next_period_fixed_start = 1_788_832_680;
        let next_period_start = next_period_fixed_start - 2_100;
        let transition_timestamp = 1_789_018_800;
        let observed_at = transition_timestamp + 120;
        let row = |timestamp: i64,
                   reset_at: i64,
                   remaining_percent: f64,
                   sol_dollars: f64,
                   luna_dollars: f64| {
            let stale_family = reset_at.abs_diff(stale_reset) <= 2;
            let next_period_family = reset_at.abs_diff(next_period_reset) <= 2;
            RawSample {
                timestamp,
                reset_at,
                remaining_percent: Some(remaining_percent),
                sol_dollars,
                terra_dollars: 0.0,
                luna_dollars,
                sol_tokens: if stale_family { 555_312_427 } else { 0 },
                terra_tokens: 0,
                luna_tokens: if stale_family {
                    22_816_483
                } else if next_period_family {
                    91_512
                } else {
                    0
                },
            }
        };
        let mut samples = vec![
            row(1_788_695_460, stale_reset, 100.0, 0.0, 0.0),
            row(
                next_period_start - 60,
                stale_reset,
                17.0,
                370.814_975,
                1.423_482_24,
            ),
        ];
        // The API reports a deadline that moves with each observation before
        // settling on the next fixed reset. The simultaneous quota recovery
        // makes this complete prelude part of the new quota period.
        samples.extend((0..35).map(|minute| {
            let timestamp = next_period_start + minute * 60;
            row(
                timestamp,
                next_period_reset - (next_period_fixed_start - timestamp),
                100.0,
                0.0,
                0.0,
            )
        }));
        let alias_timestamp = next_period_start + 60;
        samples.push(row(
            alias_timestamp,
            next_period_reset - (next_period_fixed_start - alias_timestamp) + 61,
            100.0,
            0.0,
            0.0,
        ));
        samples.extend([
            row(
                1_788_975_540,
                stale_reset + 1,
                17.0,
                370.814_975,
                1.423_482_24,
            ),
            // These are retained stale-tail aliases at the false 02:40 boundary.
            row(
                1_788_975_480,
                stale_reset + 2,
                17.0,
                370.814_975,
                1.423_482_24,
            ),
            row(1_788_975_540, stale_reset, 17.0, 370.814_975, 1.423_482_24),
            row(next_period_fixed_start, next_period_reset, 100.0, 0.0, 0.0),
            row(
                1_788_975_600,
                next_period_reset + 1,
                29.0,
                0.0,
                0.012_395_44,
            ),
            row(
                transition_timestamp - 60,
                next_period_reset + 2,
                29.0,
                0.0,
                0.187_152_6,
            ),
            // The only real reset boundary is retained exactly once.
            row(transition_timestamp, real_reset, 100.0, 0.0, 0.0),
            row(transition_timestamp + 60, real_reset + 1, 100.0, 0.0, 0.0),
            row(transition_timestamp + 120, real_reset + 2, 100.0, 0.0, 0.0),
        ]);

        let canonical = canonicalize_history(&samples, None, 0, observed_at)
            .expect("literal retained history is structurally valid");
        assert!(canonical.iter().any(|sample| {
            sample.reset_at.abs_diff(stale_reset) <= 60 && sample.timestamp < next_period_start
        }));
        assert!(canonical.iter().all(|sample| {
            sample.reset_at.abs_diff(stale_reset) > 60 || sample.timestamp < next_period_start
        }));
        assert!(canonical.iter().any(|sample| {
            sample.reset_at.abs_diff(next_period_reset) <= 60
                && sample.timestamp >= next_period_start
        }));

        let periods = history_periods(&canonical, observed_at, None, 0);
        assert_eq!(periods.len(), 3, "{periods:#?}");
        assert!(periods
            .iter()
            .all(|period| period.start_at != 1_788_975_600));
        let next_period = periods
            .iter()
            .find(|period| period.reset_at.abs_diff(next_period_reset) <= 60)
            .expect("next fixed period");
        assert_eq!(next_period.start_at, next_period_start);
        assert_eq!(
            periods
                .iter()
                .filter(|period| period.reset_at.abs_diff(stale_reset) <= 60)
                .count(),
            1
        );
        let stale_period = periods
            .iter()
            .find(|period| period.reset_at.abs_diff(stale_reset) <= 60)
            .expect("stale prefix remains diagnostic history");
        assert_eq!(stale_period.end_at, next_period_start - 60);
        assert_eq!(
            periods
                .iter()
                .filter(|period| period.reset_at.abs_diff(real_reset) <= 60)
                .count(),
            1,
            "the explicit reset must remain one public boundary"
        );

        let current_samples = canonicalize_history(
            &[row(real_reset - 120, real_reset, 100.0, 0.0, 0.0)],
            None,
            0,
            real_reset - 30,
        )
        .expect("current label fixture");
        let current_periods =
            history_periods(&current_samples, real_reset - 30, Some(real_reset), 3_600);
        assert_eq!(current_periods.len(), 1);
        assert_eq!(current_periods[0].end_at, real_reset - 30);
        assert!(current_periods[0]
            .label
            .contains(&format_jst_timestamp(real_reset).expect("reset label")));
        assert!(!current_periods[0]
            .label
            .contains(&format_jst_timestamp(real_reset - 30).expect("observed label")));
    }

    #[test]
    fn conflicting_cross_period_minute_is_not_published() {
        let timestamp = 1_800_000_000;
        let row = |reset_at, remaining_percent, sol_tokens| RawSample {
            timestamp,
            reset_at,
            remaining_percent: Some(remaining_percent),
            sol_dollars: sol_tokens as f64,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        let rows = vec![row(1_800_000_600, 17.0, 17), row(1_800_001_200, 29.0, 29)];
        let canonical = canonicalize_history(&rows, None, 0, timestamp)
            .expect("cross-period fixture is structurally valid");
        assert!(
            canonical.is_empty(),
            "conflicting owners must not cross the wire"
        );
        assert!(canonicalize_history_for_storage_with_sources(&rows, None, 0).is_err());
    }

    #[test]
    fn bracketed_period_owns_collision_with_isolated_observation() {
        let timestamp = 1_800_000_060;
        let continuous_reset = 1_800_600_000;
        let isolated_reset = 1_800_686_400;
        let row = |timestamp, reset_at, remaining_percent, sol_tokens| RawSample {
            timestamp,
            reset_at,
            remaining_percent: Some(remaining_percent),
            sol_dollars: sol_tokens as f64,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        let rows = vec![
            row(timestamp - 60, continuous_reset, 88.0, 100),
            row(timestamp, continuous_reset, 88.0, 110),
            row(timestamp, isolated_reset, 14.0, 0),
            row(timestamp + 60, continuous_reset, 88.0, 120),
        ];

        let canonical = canonicalize_history_for_storage_with_sources(&rows, None, 0)
            .expect("the unique bracketing period owns the collision minute");
        assert_eq!(canonical.len(), 3);
        let collision = canonical
            .iter()
            .find(|sample| sample.sample.timestamp == timestamp)
            .expect("collision minute remains in its continuous period");
        assert_eq!(collision.sample.reset_at, continuous_reset);
        assert_eq!(collision.sample.remaining_percent, Some(88.0));
        assert_eq!(collision.sample.sol_tokens, 110);
        assert_eq!(collision.source_reset_at, continuous_reset);
    }

    #[test]
    fn corrected_reset_for_one_observation_stays_in_one_history_period() {
        let period_start = 1_789_200_649;
        let duplicate_timestamp = 1_789_200_780;
        let original_reset = 1_789_805_415;
        let corrected_reset = 1_789_805_549;
        let row = |timestamp,
                   reset_at,
                   remaining_percent,
                   sol_dollars,
                   luna_dollars,
                   sol_tokens,
                   luna_tokens| RawSample {
            timestamp,
            reset_at,
            remaining_percent,
            sol_dollars,
            terra_dollars: 0.0,
            luna_dollars,
            sol_tokens,
            terra_tokens: 0,
            luna_tokens,
        };
        let rows = vec![
            row(period_start, original_reset, Some(100.0), 0.0, 0.0, 0, 0),
            row(
                duplicate_timestamp,
                original_reset,
                None,
                0.08,
                0.12,
                747_717,
                2_089_196,
            ),
            row(
                duplicate_timestamp,
                corrected_reset,
                None,
                0.06,
                0.10,
                565_760,
                1_907_239,
            ),
            row(
                duplicate_timestamp + 60,
                corrected_reset,
                None,
                0.09,
                0.13,
                800_000,
                2_100_000,
            ),
        ];
        let raw_before = rows
            .iter()
            .map(|row| {
                (
                    row.timestamp,
                    row.reset_at,
                    row.remaining_percent,
                    row.sol_dollars,
                    row.terra_dollars,
                    row.luna_dollars,
                    row.sol_tokens,
                    row.terra_tokens,
                    row.luna_tokens,
                )
            })
            .collect::<Vec<_>>();

        assert!(reset_belongs_to_group(&rows[1], &rows[2]));
        let canonical = canonicalize_history(
            &rows,
            Some(corrected_reset),
            7 * 24 * 60 * 60,
            duplicate_timestamp + 60,
        )
        .expect("same physical observation is structurally valid");
        assert_eq!(canonical.len(), 3);
        assert!(canonical
            .iter()
            .all(|sample| sample.reset_at == corrected_reset));
        assert_eq!(
            canonical[0].timestamp,
            period_start - period_start.rem_euclid(60)
        );
        assert_eq!(canonical[1].sol_tokens, 747_717);
        assert_eq!(canonical[1].luna_tokens, 2_089_196);
        let canonical_with_sources = canonicalize_history_for_storage_with_sources(
            &rows,
            Some(corrected_reset),
            7 * 24 * 60 * 60,
        )
        .expect("storage canonicalization retains its source keys");
        assert_eq!(
            canonical_with_sources
                .iter()
                .map(|row| row.sample.clone())
                .collect::<Vec<_>>(),
            canonical
        );
        let corrected_minute = duplicate_timestamp - duplicate_timestamp.rem_euclid(60);
        let selected = canonical_with_sources
            .iter()
            .find(|row| row.sample.timestamp == corrected_minute)
            .expect("corrected minute is retained");
        assert_eq!(selected.source_timestamp, duplicate_timestamp);
        assert_eq!(selected.source_reset_at, original_reset);
        let periods = history_periods(
            &canonical,
            duplicate_timestamp + 60,
            Some(corrected_reset),
            7 * 24 * 60 * 60,
        );
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].start_at, canonical[0].timestamp);
        assert!(canonical.iter().all(|sample| {
            periods
                .iter()
                .filter(|period| {
                    sample.reset_at >= period.reset_at.saturating_sub(60)
                        && sample.reset_at <= period.reset_at
                        && sample.timestamp >= period.start_at
                        && sample.timestamp <= period.end_at
                })
                .count()
                == 1
        }));
        assert_eq!(
            raw_before,
            rows.iter()
                .map(|row| {
                    (
                        row.timestamp,
                        row.reset_at,
                        row.remaining_percent,
                        row.sol_dollars,
                        row.terra_dollars,
                        row.luna_dollars,
                        row.sol_tokens,
                        row.terra_tokens,
                        row.luna_tokens,
                    )
                })
                .collect::<Vec<_>>()
        );

        let mut quota_conflict = rows[2].clone();
        quota_conflict.remaining_percent = Some(99.0);
        let mut quota_anchor = rows[1].clone();
        quota_anchor.remaining_percent = Some(100.0);
        assert!(!reset_belongs_to_group(&quota_anchor, &quota_conflict));
        assert!(canonicalize_history(
            &[quota_anchor, quota_conflict],
            None,
            0,
            duplicate_timestamp,
        )
        .expect("conflicting quota fixture")
        .is_empty());

        let mut incomparable = rows[2].clone();
        incomparable.sol_tokens = rows[1].sol_tokens - 1;
        incomparable.luna_tokens = rows[1].luna_tokens + 1;
        incomparable.sol_dollars = rows[1].sol_dollars - 0.01;
        incomparable.luna_dollars = rows[1].luna_dollars + 0.01;
        assert!(!reset_belongs_to_group(&rows[1], &incomparable));
        assert!(canonicalize_history(
            &[rows[1].clone(), incomparable],
            None,
            0,
            duplicate_timestamp,
        )
        .expect("incomparable vector fixture")
        .is_empty());
    }

    #[test]
    fn same_period_quota_conflict_excludes_only_its_minute() {
        let minute = 1_800_000_000;
        let reset_at = 1_800_604_800;
        let row = |timestamp, remaining_percent, sol_tokens| RawSample {
            timestamp,
            reset_at,
            remaining_percent,
            sol_dollars: sol_tokens as f64 / 100.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        let rows = vec![
            row(minute + 3, Some(100.0), 10),
            row(minute + 31, Some(100.0), 20),
            row(minute + 58, Some(99.0), 30),
        ];

        let canonical =
            canonicalize_history_for_storage_with_sources(&rows, Some(reset_at), 604_800)
                .expect("one conflicted minute must not reject the complete candidate");

        assert!(canonical.is_empty());
    }

    #[test]
    fn ambiguous_owned_minute_is_excluded_from_public_and_storage() {
        let minute = 1_800_000_000;
        let reset_at = 1_800_604_800;
        let row = |timestamp, remaining_percent, sol_tokens, luna_tokens| -> RawSample {
            RawSample {
                timestamp,
                reset_at,
                remaining_percent,
                sol_dollars: sol_tokens as f64 / 100.0,
                terra_dollars: 0.0,
                luna_dollars: luna_tokens as f64 / 100.0,
                sol_tokens,
                terra_tokens: 0,
                luna_tokens,
            }
        };

        let quota_increase = vec![
            row(minute + 5, Some(99.0), 10, 0),
            row(minute + 45, Some(100.0), 20, 0),
        ];
        assert!(
            canonicalize_history(&quota_increase, Some(reset_at), 604_800, minute + 45)
                .expect("public canonicalization rejects only the ambiguous minute")
                .is_empty()
        );
        assert!(canonicalize_history_for_storage_with_sources(
            &quota_increase,
            Some(reset_at),
            604_800,
        )
        .expect("one quota-conflicted minute must not reject the complete candidate")
        .is_empty());

        let incomparable = vec![
            row(minute + 5, Some(100.0), 20, 10),
            row(minute + 45, Some(99.0), 10, 20),
        ];
        assert!(
            canonicalize_history(&incomparable, Some(reset_at), 604_800, minute + 45)
                .expect("public canonicalization rejects only the ambiguous minute")
                .is_empty()
        );
        assert!(canonicalize_history_for_storage_with_sources(
            &incomparable,
            Some(reset_at),
            604_800,
        )
        .expect("one incomparable minute must not reject the complete candidate")
        .is_empty());
    }

    #[test]
    fn task_activity_unions_sources_but_bounds_an_unclosed_task_to_observed_evidence() {
        let reset_at = 1_800_000_600;
        let period = PublicHistoryPeriod {
            id: reset_at.to_string(),
            start_at: 1_800_000_000,
            end_at: 1_800_000_180,
            reset_at,
            label: "task activity".to_owned(),
            current: true,
        };
        let observation = |timestamp| PublicHistoryObservationV3 {
            timestamp,
            reset_at,
            remaining_percent: None,
            task_active_since_previous: None,
            models: None,
            models_complete: false,
            model_source: "legacy-unknown".to_owned(),
        };
        let mut history = BTreeMap::from([
            ((reset_at, 1_800_000_000), observation(1_800_000_000)),
            ((reset_at, 1_800_000_060), observation(1_800_000_060)),
            ((reset_at, 1_800_000_120), observation(1_800_000_120)),
            ((reset_at, 1_800_000_180), observation(1_800_000_180)),
        ]);
        let source_a = TaskSourceKey {
            root_identity: "a".to_owned(),
            relative_path: "a.jsonl".to_owned(),
            file_device: "1".to_owned(),
            file_inode: "1".to_owned(),
        };
        let source_b = TaskSourceKey {
            root_identity: "b".to_owned(),
            relative_path: "b.jsonl".to_owned(),
            file_device: "1".to_owned(),
            file_inode: "2".to_owned(),
        };
        let event = |_source: &TaskSourceKey, timestamp, event_index, running| TaskTransition {
            event_index,
            timestamp,
            running,
        };
        let mut transitions = BTreeMap::new();
        // Source A has no closing lifecycle event, but its last independently
        // observed source event is at :75. It may veto idle only through that
        // evidence horizon, never through every later history period. Source B
        // starts and stops inside the first interval, proving that the result
        // remains a union over source streams.
        transitions.insert(
            source_a.clone(),
            vec![event(&source_a, 1_800_000_010, 0, true)],
        );
        transitions.insert(
            source_b.clone(),
            vec![
                event(&source_b, 1_800_000_020, 0, true),
                event(&source_b, 1_800_000_030, 1, false),
            ],
        );
        let evidence = TaskActivityEvidence {
            complete: false,
            transitions,
            source_observed_through: BTreeMap::from([
                (source_a, 1_800_000_075),
                (source_b, 1_800_000_030),
            ]),
            source_complete: BTreeMap::new(),
        };

        assign_task_activity(&mut history, &[period], &evidence);

        assert_eq!(
            history[&(reset_at, 1_800_000_000)].task_active_since_previous,
            None
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_060)].task_active_since_previous,
            Some(true)
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_120)].task_active_since_previous,
            Some(true)
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_180)].task_active_since_previous,
            None,
            "an unclosed start must not manufacture activity after its source evidence ends"
        );
    }

    #[test]
    fn complete_task_lifecycle_publishes_minute_cadence_across_sparse_history() {
        let reset_at = 1_800_000_600;
        let observation = |timestamp| PublicHistoryObservationV3 {
            timestamp,
            reset_at,
            remaining_percent: None,
            task_active_since_previous: None,
            models: None,
            models_complete: false,
            model_source: "legacy-unknown".to_owned(),
        };
        let mut history = BTreeMap::from([
            ((reset_at, 1_800_000_000), observation(1_800_000_000)),
            ((reset_at, 1_800_000_240), observation(1_800_000_240)),
        ]);
        let period = PublicHistoryPeriod {
            id: reset_at.to_string(),
            start_at: 1_800_000_000,
            end_at: 1_800_000_240,
            reset_at,
            label: "sparse lifecycle".to_owned(),
            current: true,
        };
        let source = TaskSourceKey {
            root_identity: "verified".to_owned(),
            relative_path: "verified.jsonl".to_owned(),
            file_device: "1".to_owned(),
            file_inode: "1".to_owned(),
        };
        let evidence = TaskActivityEvidence {
            complete: false,
            transitions: BTreeMap::from([(
                source.clone(),
                vec![
                    TaskTransition {
                        event_index: 0,
                        timestamp: 1_800_000_001,
                        running: true,
                    },
                    TaskTransition {
                        event_index: 1,
                        timestamp: 1_800_000_010,
                        running: false,
                    },
                    TaskTransition {
                        event_index: 2,
                        timestamp: 1_800_000_230,
                        running: true,
                    },
                ],
            )]),
            source_observed_through: BTreeMap::from([(source.clone(), 1_800_000_240)]),
            source_complete: BTreeMap::from([(source, true)]),
        };

        assign_task_activity(&mut history, &[period], &evidence);

        assert_eq!(history.len(), 5);
        assert!(history
            .iter()
            .filter(|((_, timestamp), _)| {
                matches!(*timestamp, 1_800_000_060 | 1_800_000_120 | 1_800_000_180)
            })
            .all(|(_, sample)| {
                sample.model_source == "unavailable"
                    && sample.models.is_none()
                    && !sample.models_complete
            }));
        assert_eq!(
            history[&(reset_at, 1_800_000_060)].task_active_since_previous,
            Some(true)
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_120)].task_active_since_previous,
            Some(false)
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_180)].task_active_since_previous,
            Some(false)
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_240)].task_active_since_previous,
            Some(true)
        );
        let v2 = history_observations_v2(&[], &history.into_values().collect::<Vec<_>>());
        assert!(v2
            .iter()
            .filter(|sample| {
                matches!(
                    sample.timestamp,
                    1_800_000_060 | 1_800_000_120 | 1_800_000_180
                )
            })
            .all(|sample| {
                sample.model_source == "unavailable"
                    && sample.sol_dollars.is_none()
                    && sample.terra_dollars.is_none()
                    && sample.luna_dollars.is_none()
                    && sample.sol_tokens.is_none()
                    && sample.terra_tokens.is_none()
                    && sample.luna_tokens.is_none()
            }));
        assert_eq!(v2.len(), 5);
    }

    #[test]
    fn verified_stop_boundary_does_not_smear_activity_across_sparse_tail() {
        let reset_at = 1_800_000_600;
        let observation = |timestamp| PublicHistoryObservationV3 {
            timestamp,
            reset_at,
            remaining_percent: None,
            task_active_since_previous: None,
            models: None,
            models_complete: false,
            model_source: "legacy-unknown".to_owned(),
        };
        let mut history = BTreeMap::from([
            ((reset_at, 1_800_000_000), observation(1_800_000_000)),
            ((reset_at, 1_800_000_240), observation(1_800_000_240)),
        ]);
        let period = PublicHistoryPeriod {
            id: reset_at.to_string(),
            start_at: 1_800_000_000,
            end_at: 1_800_000_240,
            reset_at,
            label: "sparse stopped task".to_owned(),
            current: true,
        };
        let source = TaskSourceKey {
            root_identity: "verified".to_owned(),
            relative_path: "verified.jsonl".to_owned(),
            file_device: "1".to_owned(),
            file_inode: "1".to_owned(),
        };
        let evidence = TaskActivityEvidence {
            complete: false,
            transitions: BTreeMap::from([(
                source.clone(),
                vec![
                    TaskTransition {
                        event_index: 0,
                        timestamp: 1_800_000_001,
                        running: true,
                    },
                    TaskTransition {
                        event_index: 1,
                        timestamp: 1_800_000_070,
                        running: false,
                    },
                ],
            )]),
            source_observed_through: BTreeMap::from([(source.clone(), 1_800_000_240)]),
            source_complete: BTreeMap::from([(source, true)]),
        };

        assign_task_activity(&mut history, &[period], &evidence);

        assert_eq!(
            history
                .keys()
                .map(|(_, timestamp)| *timestamp)
                .collect::<Vec<_>>(),
            [1_800_000_000, 1_800_000_060, 1_800_000_120, 1_800_000_240,]
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_120)].task_active_since_previous,
            Some(true)
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_240)].task_active_since_previous,
            None
        );
    }

    #[test]
    fn task_activity_requires_verified_indexed_coverage() {
        let connection = Connection::open_in_memory().expect("task fixture");
        connection
            .execute_batch(
                "CREATE TABLE session_ranges(
                    root_identity TEXT, relative_path TEXT, file_device TEXT,
                    file_inode TEXT, prefix_generation TEXT, start_offset INTEGER,
                    end_offset INTEGER, record_sha256 TEXT
                );
                CREATE TABLE session_task_indexed_ranges(
                    root_identity TEXT, relative_path TEXT, file_device TEXT,
                    file_inode TEXT, prefix_generation TEXT, start_offset INTEGER,
                    end_offset INTEGER, record_sha256 TEXT
                );
                CREATE TABLE session_checkpoints(
                    root_identity TEXT, relative_path TEXT, file_device TEXT,
                    file_inode TEXT, committed_offset INTEGER,
                    discard_until_lf INTEGER, prefix_generation TEXT
                );
                CREATE TABLE session_pending_ranges(
                    root_identity TEXT, relative_path TEXT, file_device TEXT,
                    file_inode TEXT, start_offset INTEGER, end_offset INTEGER,
                    collector_epoch INTEGER, cycle_seq INTEGER,
                    prefix_generation TEXT, record_sha256 TEXT,
                    parser_version TEXT, reason TEXT, complete INTEGER
                );
                CREATE TABLE session_task_events(
                    root_identity TEXT, relative_path TEXT, file_device TEXT,
                    file_inode TEXT, prefix_generation TEXT, start_offset INTEGER,
                    end_offset INTEGER, record_sha256 TEXT, event_index INTEGER,
                    timestamp INTEGER, running INTEGER
                );
                INSERT INTO session_ranges VALUES
                    ('unix:1:2',
                     'session.jsonl','1','2','00000000000000000000000000000000',0,10,
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb');
                INSERT INTO session_checkpoints VALUES
                    ('unix:1:2',
                     'session.jsonl','1','2',10,0,'00000000000000000000000000000000');
                INSERT INTO session_task_events VALUES
                    ('unix:1:2',
                     'session.jsonl','1','2','00000000000000000000000000000000',0,10,
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                     0,1800000010,1),
                    ('unix:1:2',
                     'session.jsonl','1','2','00000000000000000000000000000000',0,10,
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                     1,1800000020,0);",
            )
            .expect("task schema");

        assert!(!read_task_activity_evidence(&connection, true).complete);
        connection
            .execute(
                "INSERT INTO session_task_indexed_ranges VALUES
                 (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    "unix:1:2",
                    "session.jsonl",
                    "1",
                    "2",
                    "00000000000000000000000000000000",
                    0_i64,
                    10_i64,
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                ],
            )
            .expect("indexed range");
        connection
            .execute(
                "INSERT INTO session_task_indexed_ranges VALUES
                 (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    "unix:1:2",
                    "session.jsonl",
                    "1",
                    "2",
                    "00000000000000000000000000000000",
                    0_i64,
                    5_i64,
                    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                ],
            )
            .expect("overlapping indexed range");
        let evidence = read_task_activity_evidence(&connection, true);
        assert!(evidence.complete);
        assert_eq!(
            evidence.transitions.values().map(Vec::len).sum::<usize>(),
            2
        );

        connection
            .execute_batch(
                "INSERT INTO session_ranges VALUES
                    ('unix:3:4',
                     'unindexed.jsonl','3','4','00000000000000000000000000000000',0,10,
                     'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd');
                 INSERT INTO session_checkpoints VALUES
                    ('unix:3:4',
                     'unindexed.jsonl','3','4',10,0,'00000000000000000000000000000000');",
            )
            .expect("unindexed source range");
        let partial = read_task_activity_evidence(&connection, true);
        assert!(!partial.complete);
        assert_eq!(
            partial.transitions.values().map(Vec::len).sum::<usize>(),
            2,
            "an unrelated unindexed range must not erase a verified active transition"
        );
    }

    #[test]
    fn missing_task_activity_schema_is_incomplete_for_history() {
        let connection = Connection::open_in_memory().expect("missing task schema fixture");
        assert!(!read_task_activity_evidence(&connection, true).complete);
        assert!(read_task_activity_evidence(&connection, false).complete);
    }

    #[test]
    fn incomplete_task_evidence_publishes_active_veto_without_claiming_inactive() {
        let reset_at = 1_800_000_600;
        let observation = |timestamp| PublicHistoryObservationV3 {
            timestamp,
            reset_at,
            remaining_percent: None,
            task_active_since_previous: Some(true),
            models: None,
            models_complete: false,
            model_source: "legacy-unknown".to_owned(),
        };
        let mut history = [1_800_000_000, 1_800_000_060, 1_800_000_120, 1_800_000_180]
            .into_iter()
            .map(|timestamp| ((reset_at, timestamp), observation(timestamp)))
            .collect::<BTreeMap<_, _>>();
        let period = PublicHistoryPeriod {
            id: reset_at.to_string(),
            start_at: 1_800_000_000,
            end_at: 1_800_000_180,
            reset_at,
            label: "partial activity".to_owned(),
            current: true,
        };
        let source = TaskSourceKey {
            root_identity: "verified".to_owned(),
            relative_path: "verified.jsonl".to_owned(),
            file_device: "1".to_owned(),
            file_inode: "1".to_owned(),
        };
        let incomplete_source = TaskSourceKey {
            root_identity: "incomplete".to_owned(),
            relative_path: "incomplete.jsonl".to_owned(),
            file_device: "1".to_owned(),
            file_inode: "2".to_owned(),
        };
        let evidence = TaskActivityEvidence {
            complete: false,
            transitions: BTreeMap::from([
                (
                    source,
                    vec![
                        TaskTransition {
                            event_index: 0,
                            timestamp: 1_800_000_010,
                            running: true,
                        },
                        TaskTransition {
                            event_index: 1,
                            timestamp: 1_800_000_070,
                            running: false,
                        },
                    ],
                ),
                (
                    incomplete_source.clone(),
                    vec![TaskTransition {
                        event_index: 0,
                        timestamp: 1_800_000_130,
                        running: true,
                    }],
                ),
            ]),
            source_observed_through: BTreeMap::from([(incomplete_source, 1_800_000_150)]),
            source_complete: BTreeMap::new(),
        };
        assign_task_activity(&mut history, &[period], &evidence);
        assert_eq!(
            history[&(reset_at, 1_800_000_000)].task_active_since_previous,
            None
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_060)].task_active_since_previous,
            Some(true)
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_120)].task_active_since_previous,
            Some(true)
        );
        assert_eq!(
            history[&(reset_at, 1_800_000_180)].task_active_since_previous,
            Some(true),
            "an exact indexed start remains positive evidence even when later source coverage is incomplete"
        );
    }

    #[test]
    fn regressing_model_is_missing_until_recovery_without_hiding_other_models() {
        let model = |name: &str, total_tokens: u64, total_dollars: f64| PublicHistoryModelUsageV3 {
            model: name.to_owned(),
            total_tokens,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            output_tokens: None,
            total_dollars: Some(total_dollars),
        };
        let observation =
            |timestamp: i64, reset_at: i64, sol_tokens: u64, sol_dollars: f64, luna_tokens: u64| {
                PublicHistoryObservationV3 {
                    timestamp,
                    reset_at,
                    remaining_percent: Some(50.0),
                    task_active_since_previous: None,
                    models: Some(vec![
                        model("LUNA", luna_tokens, luna_tokens as f64),
                        model("SOL", sol_tokens, sol_dollars),
                    ]),
                    models_complete: true,
                    model_source: "confirmed".to_owned(),
                }
            };
        let first_reset = 1_800_000_600;
        let second_reset = 1_800_001_200;
        let mut history = BTreeMap::from([
            (
                (first_reset, 1_800_000_000),
                observation(1_800_000_000, first_reset, 100, 10.0, 50),
            ),
            (
                (first_reset, 1_800_000_060),
                observation(1_800_000_060, first_reset, 90, 11.0, 60),
            ),
            (
                (first_reset, 1_800_000_120),
                observation(1_800_000_120, first_reset, 100, 9.0, 70),
            ),
            (
                (first_reset, 1_800_000_180),
                observation(1_800_000_180, first_reset, 101, 11.0, 80),
            ),
            (
                (second_reset, 1_800_000_240),
                observation(1_800_000_240, second_reset, 1, 0.1, 1),
            ),
        ]);

        suppress_regressing_history_models(&mut history);

        let names = |timestamp| {
            history
                .values()
                .find(|sample| sample.timestamp == timestamp)
                .and_then(|sample| sample.models.as_ref())
                .map(|models| {
                    models
                        .iter()
                        .map(|model| model.model.as_str())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        assert_eq!(names(1_800_000_060), vec!["LUNA"]);
        assert_eq!(names(1_800_000_120), vec!["LUNA"]);
        assert_eq!(names(1_800_000_180), vec!["LUNA", "SOL"]);
        assert_eq!(names(1_800_000_240), vec!["LUNA", "SOL"]);
        assert!(!history[&(first_reset, 1_800_000_060)].models_complete);
        assert!(!history[&(first_reset, 1_800_000_120)].models_complete);
        assert_eq!(
            history[&(first_reset, 1_800_000_060)].model_source,
            "legacy-unknown"
        );
        assert_eq!(
            history[&(first_reset, 1_800_000_120)].model_source,
            "legacy-unknown"
        );
    }

    #[test]
    fn legacy_model_values_cannot_reject_a_later_direct_observation() {
        let model = |total_tokens: u64| PublicHistoryModelUsageV3 {
            model: "SOL".to_owned(),
            total_tokens,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            output_tokens: None,
            total_dollars: Some(total_tokens as f64),
        };
        let reset_at = 1_800_000_600;
        let observation = |timestamp: i64, total_tokens: u64, source: &str, complete: bool| {
            PublicHistoryObservationV3 {
                timestamp,
                reset_at,
                remaining_percent: Some(50.0),
                task_active_since_previous: None,
                models: Some(vec![model(total_tokens)]),
                models_complete: complete,
                model_source: source.to_owned(),
            }
        };
        let mut history = BTreeMap::from([
            (
                (reset_at, 1_800_000_000),
                observation(1_800_000_000, 100, "confirmed", true),
            ),
            (
                (reset_at, 1_800_000_060),
                observation(1_800_000_060, 1_000, "legacy-unknown", false),
            ),
            (
                (reset_at, 1_800_000_120),
                observation(1_800_000_120, 110, "confirmed", true),
            ),
        ]);

        suppress_regressing_history_models(&mut history);

        let later = &history[&(reset_at, 1_800_000_120)];
        assert_eq!(later.model_source, "confirmed");
        assert!(later.models_complete);
        assert_eq!(later.models.as_ref().unwrap()[0].total_tokens, 110);
        assert_eq!(
            history[&(reset_at, 1_800_000_060)].models.as_ref().unwrap()[0].total_tokens,
            1_000,
            "legacy data is retained as display-only history"
        );
    }
}

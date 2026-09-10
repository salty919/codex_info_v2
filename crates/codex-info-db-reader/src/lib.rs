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
    PublicModelUsageV3, PublicQuota, PublicState, MAX_PUBLIC_MODELS_V3,
};
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_HISTORY_ROWS: usize = 31 * 24 * 60;
const HISTORY_WINDOW_SECONDS: i64 = 31 * 24 * 60 * 60;
const RESET_AT_TOLERANCE_SECONDS: i64 = 60;
const MOVING_RESET_GROUP_MAX_DRIFT_SECONDS: i64 = 5 * 60;
const MOVING_RESET_STEP_TOLERANCE_SECONDS: i64 = 180;
const MOVING_RESET_MIN_HORIZON_SECONDS: i64 = 86_400;
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
        Ok(connection)
    }

    /// Exposed for the focused read-only gate and diagnostics.
    pub fn query_only_enabled(&self) -> Result<bool, ReaderError> {
        let connection = self.connection()?;
        Ok(connection.query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))? == 1)
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
        let has_pending_ranges = read_pending_ranges(&transaction)?;
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
        let has_pending_ranges = read_pending_ranges(&transaction)?;
        let raw = read_history(&transaction)?;
        let generation = read_generation(&transaction, raw.iter().map(|row| row.timestamp))?;
        let (details, models_v3, history_samples_v2, history_samples_v3) =
            build_details(&transaction, &raw)?;
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

#[derive(Clone, Debug)]
struct RawSample {
    timestamp: i64,
    reset_at: i64,
    remaining_percent: Option<f64>,
    sol_dollars: f64,
    terra_dollars: f64,
    luna_dollars: f64,
    sol_tokens: u64,
    terra_tokens: u64,
    luna_tokens: u64,
}

#[derive(Clone, Debug)]
struct ResetGroup {
    canonical_reset_at: i64,
    start: i64,
    rows: Vec<RawSample>,
}

fn read_history(connection: &Connection) -> Result<Vec<RawSample>, ReaderError> {
    if !table_exists(connection, "usage_history")? {
        return Err(ReaderError::Schema(
            "usage_history table is missing".to_owned(),
        ));
    }
    let mut statement = connection.prepare(
        "SELECT timestamp, reset_at, remaining_percent, sol_dollars,
                terra_dollars, luna_dollars, sol_tokens, terra_tokens, luna_tokens
         FROM usage_history ORDER BY reset_at ASC, timestamp ASC",
    )?;
    let rows = statement.query_map([], raw_sample_from_row)?;
    let mut values = Vec::new();
    for row in rows {
        if let Some(value) = row? {
            values.push(value);
        }
    }
    Ok(values)
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
    // Quota observations are supplied by an external service and historical
    // databases can contain isolated out-of-domain values.  That makes only
    // this nullable field unavailable; it must not hide otherwise durable
    // token/cost history or make the complete REST snapshot unavailable.
    let remaining_percent =
        remaining_percent.filter(|value| value.is_finite() && (0.0..=100.0).contains(value));
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

fn build_details(connection: &Connection, raw: &[RawSample]) -> Result<DetailsBuild, ReaderError> {
    if raw.is_empty() {
        return Ok((PublicDetails::default(), Vec::new(), Vec::new(), Vec::new()));
    }
    let observed_at = raw.iter().map(|row| row.timestamp).max().unwrap_or(0);
    let (current_reset_at, window_seconds) = read_collection_config(connection)?;
    let mut recent = Vec::new();
    let cutoff = observed_at.saturating_sub(HISTORY_WINDOW_SECONDS);
    for row in raw {
        if row.timestamp <= cutoff || row.timestamp > observed_at {
            continue;
        }
        recent.push(row.clone());
    }
    let mut samples = canonicalize_history(&recent, current_reset_at, window_seconds, observed_at)?;
    if samples.is_empty() && !recent.is_empty() {
        // A stale/missing collection-generation window must not turn an
        // existing history database into a fabricated empty root.  Retain
        // the structurally canonical rows and let the explicit state expose
        // the absence of an authoritative current window.
        samples = canonicalize_history(&recent, None, 0, observed_at)?;
    }
    let mut periods = history_periods(&samples, observed_at, current_reset_at, window_seconds);
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
    let model_projection = read_model_projection(connection)?;
    let models = model_projection.v1;
    let history_samples_v3 =
        read_history_projection(connection, &samples, &mut periods, observed_at, cutoff)?;
    let history_samples_v2 = history_observations_v2(&samples, &history_samples_v3);
    let estimated_cost_label = format_estimated_cost(&models);
    let mut gaps = read_confirmed_gaps(connection)?
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
            active_thread_count: 0,
            history_periods: periods,
            history_samples: samples,
            history_gaps: gaps,
            threads: Vec::new(),
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

/// Build the v3 graph rows from the durable observation JSON and model-history
/// sidecar.  The legacy `usage_history` row remains the v1 source of truth for
/// period ownership and the three displayed dollar columns; sidecar faults
/// are isolated to their one timestamp/model group.
fn read_history_projection(
    connection: &Connection,
    samples: &[PublicHistorySample],
    periods: &mut [PublicHistoryPeriod],
    observed_at: i64,
    cutoff: i64,
) -> Result<Vec<PublicHistoryObservationV3>, ReaderError> {
    let observations = read_stored_history_observations(connection, cutoff, observed_at)?;
    let model_groups = read_history_model_groups(connection, cutoff, observed_at)?;
    let mut observations_by_timestamp = BTreeMap::<i64, Vec<&StoredHistoryObservation>>::new();
    for observation in &observations {
        observations_by_timestamp
            .entry(observation.timestamp)
            .or_default()
            .push(observation);
    }
    let mut groups_by_timestamp = BTreeMap::<i64, Vec<(i64, &HistoryModelGroup)>>::new();
    for ((reset_at, timestamp), group) in &model_groups {
        groups_by_timestamp
            .entry(*timestamp)
            .or_default()
            .push((*reset_at, group));
    }

    // Unavailable quota observations are not present in usage_history.  The
    // root v2/v3 projection extends the containing period to those minutes so
    // the graph can render an explicit idle/dashed point without fabricating
    // model values.
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
        let stored = observations_by_timestamp
            .get(&sample.timestamp)
            .into_iter()
            .flatten()
            .filter(|observation| {
                observation.reset_at.abs_diff(sample.reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64
            })
            .max_by_key(|observation| {
                let has_models =
                    groups_by_timestamp
                        .get(&observation.timestamp)
                        .is_some_and(|groups| {
                            groups.iter().any(|(reset_at, _)| {
                                reset_at.abs_diff(observation.reset_at)
                                    <= RESET_AT_TOLERANCE_SECONDS as u64
                            })
                        });
                (
                    has_models,
                    history_model_source_rank(observation.model_source),
                )
            });
        // Legacy rows are synthetic `legacy-unknown` observations in the
        // root loader; model-history sidecars still attach to them even when
        // no durable_state JSON record exists.
        let group_reset_at = stored.map_or(sample.reset_at, |observation| observation.reset_at);
        let group = groups_by_timestamp
            .get(&sample.timestamp)
            .and_then(|groups| {
                groups.iter().find_map(|(reset_at, group)| {
                    (reset_at.abs_diff(group_reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64)
                        .then_some(*group)
                })
            });
        let model_source = stored
            .map(|observation| observation.model_source)
            .unwrap_or(HistoryModelSource::LegacyUnknown);
        let models_complete = group.is_some_and(HistoryModelGroup::model_set_complete);
        let models = history_models_v3(group, sample, models_complete);
        let source = if model_source == HistoryModelSource::Unavailable {
            HistoryModelSource::Unavailable
        } else if model_source == HistoryModelSource::ReconstructedFromSession {
            HistoryModelSource::ReconstructedFromSession
        } else if model_source == HistoryModelSource::Confirmed
            && models_complete
            && models.is_some()
        {
            HistoryModelSource::Confirmed
        } else {
            HistoryModelSource::LegacyUnknown
        };
        history.insert(
            (sample.reset_at, sample.timestamp),
            PublicHistoryObservationV3 {
                timestamp: sample.timestamp,
                reset_at: sample.reset_at,
                remaining_percent: sample.remaining_percent,
                models,
                models_complete,
                model_source: source.as_str().to_owned(),
            },
        );
    }

    // Keep sidecar-only unavailable points visible in v3.  They are admitted
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
                models: None,
                models_complete: false,
                model_source: "unavailable".to_owned(),
            });
    }
    Ok(history.into_values().collect())
}

fn history_observations_v2(
    samples: &[PublicHistorySample],
    history_v3: &[PublicHistoryObservationV3],
) -> Vec<PublicHistoryObservation> {
    history_v3
        .iter()
        .map(|sample| {
            let legacy = samples.iter().find(|legacy| {
                legacy.timestamp == sample.timestamp && legacy.reset_at == sample.reset_at
            });
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

fn read_stored_history_observations(
    connection: &Connection,
    cutoff: i64,
    observed_at: i64,
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
        observations.insert((observation.reset_at, observation.timestamp), observation);
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

fn read_history_model_groups(
    connection: &Connection,
    cutoff: i64,
    observed_at: i64,
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

fn history_model_source_rank(source: HistoryModelSource) -> u8 {
    match source {
        HistoryModelSource::Unavailable => 0,
        HistoryModelSource::LegacyUnknown => 1,
        HistoryModelSource::ReconstructedFromSession => 2,
        HistoryModelSource::Confirmed => 3,
    }
}

fn history_models_v3(
    group: Option<&HistoryModelGroup>,
    sample: &PublicHistorySample,
    models_complete: bool,
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
    if !models_complete {
        for legacy in legacy_history_models_v3(sample) {
            if !models.iter().any(|model| model.model == legacy.model) {
                models.push(legacy);
            }
        }
    }
    models.sort_by(|left, right| left.model.cmp(&right.model));
    (!models.is_empty()).then_some(models)
}

fn legacy_history_models_v3(sample: &PublicHistorySample) -> Vec<PublicHistoryModelUsageV3> {
    [
        ("SOL", sample.sol_tokens, sample.sol_dollars),
        ("TERRA", sample.terra_tokens, sample.terra_dollars),
        ("LUNA", sample.luna_tokens, sample.luna_dollars),
    ]
    .into_iter()
    .map(
        |(model, total_tokens, total_dollars)| PublicHistoryModelUsageV3 {
            model: model.to_owned(),
            total_tokens,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            output_tokens: None,
            total_dollars: Some(total_dollars),
        },
    )
    .collect()
}

fn history_model_usage_v3(
    total: &RawModelTotal,
    sample: &PublicHistorySample,
) -> PublicHistoryModelUsageV3 {
    let total_dollars = match total.model.as_str() {
        "SOL" => Some(sample.sol_dollars),
        "TERRA" => Some(sample.terra_dollars),
        "LUNA" => Some(sample.luna_dollars),
        _ => model_v3_cost(total).map(|cost| cost.total_dollars),
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
    window_seconds: i64,
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
            let mut start_at = observed_start;
            let mut end_at = observed_end.min(observed_at);
            if is_current && window_seconds > 0 {
                if let Some(authoritative_start) = reset_at.checked_sub(window_seconds) {
                    start_at =
                        start_at.max(authoritative_start - authoritative_start.rem_euclid(60));
                }
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
                label: format!("reset-{reset_at}"),
                current: is_current || Some(reset_at) == fallback_current,
            }
        })
        .collect::<Vec<_>>();
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

fn canonicalize_history(
    rows: &[RawSample],
    current_reset_at: Option<i64>,
    window_seconds: i64,
    observed_at: i64,
) -> Result<Vec<PublicHistorySample>, ReaderError> {
    let mut sorted = rows.to_vec();
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
            if row.timestamp.saturating_sub(group.start) > MOVING_RESET_GROUP_MAX_DRIFT_SECONDS {
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
    let current_start = current_reset_at
        .zip((window_seconds > 0).then_some(window_seconds))
        .and_then(|(reset, window)| reset.checked_sub(window))
        .map(|start| start - start.rem_euclid(60));

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

    let mut canonical = Vec::new();
    for (group_index, group) in groups.into_iter().enumerate() {
        let mut by_minute = BTreeMap::<i64, Vec<RawSample>>::new();
        for row in group.rows {
            let minute = row.timestamp - row.timestamp.rem_euclid(60);
            if Some(group_index) == current_group
                && current_start.is_some_and(|start| row.timestamp < start)
            {
                continue;
            }
            by_minute.entry(minute).or_default().push(row);
        }
        for (minute, mut minute_rows) in by_minute {
            let owners = minute_owners.get(&minute).map_or(0, Vec::len);
            if owners > 1 {
                // A quota value is the authority when reset aliases overlap.
                // More than one distinct quota owner is ambiguous for only
                // this minute; unrelated periods stay publishable.
                let mut quota_values = Vec::new();
                for value in minute_rows.iter().filter_map(|row| row.remaining_percent) {
                    if !quota_values.contains(&value) {
                        quota_values.push(value);
                    }
                }
                if quota_values.len() > 1 {
                    continue;
                }
            }
            minute_rows.sort_by_key(|row| (row.timestamp, row.reset_at));
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
            let Some(dominant) = minute_rows.into_iter().rev().find(|row| {
                row.sol_dollars >= maximums.0
                    && row.terra_dollars >= maximums.1
                    && row.luna_dollars >= maximums.2
                    && row.sol_tokens >= maximums.3
                    && row.terra_tokens >= maximums.4
                    && row.luna_tokens >= maximums.5
            }) else {
                continue;
            };
            canonical.push(PublicHistorySample {
                timestamp: minute,
                reset_at: group.canonical_reset_at,
                remaining_percent: dominant.remaining_percent,
                sol_dollars: dominant.sol_dollars,
                terra_dollars: dominant.terra_dollars,
                luna_dollars: dominant.luna_dollars,
                sol_tokens: dominant.sol_tokens,
                terra_tokens: dominant.terra_tokens,
                luna_tokens: dominant.luna_tokens,
            });
        }
    }
    canonical.sort_by_key(|sample| (sample.reset_at, sample.timestamp));
    if canonical.len() > MAX_HISTORY_ROWS {
        return Err(ReaderError::TooManyRows(canonical.len()));
    }
    let _ = observed_at;
    Ok(canonical)
}

fn reset_belongs_to_group(anchor: &RawSample, candidate: &RawSample) -> bool {
    if candidate.reset_at.abs_diff(anchor.reset_at) <= RESET_AT_TOLERANCE_SECONDS as u64 {
        return true;
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

    let mut projection = ModelProjection::default();
    let mut names = std::collections::HashSet::new();
    for row in rows {
        let Some(row) = row? else { continue };
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
    Ok(projection)
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

    #[test]
    fn opens_query_only_and_reads_back_the_connection_setting() {
        let path = temp_db("query-only");
        make_db(&path);
        let reader = DbReader::open(&path).expect("reader");
        assert!(reader.query_only_enabled().expect("query_only"));
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
        assert_eq!(pending.details.history_samples.len(), 1);

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
        assert_eq!(snapshot.details.history_samples.len(), 1);
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
                INSERT INTO usage_model_history VALUES
                    (1800000060,1800000000,'SOL','110','100','40','10','0',1),
                    (1800000060,1800000000,'BROKEN','1','1','99','0','0',1);",
            )
            .expect("history model schema");
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
            vec!["LUNA", "SOL", "TERRA"]
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
    fn reset_jitter_is_canonicalized_into_one_current_period_with_models() {
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
        assert_eq!(snapshot.details.history_samples.len(), 3);
        assert_eq!(snapshot.history_samples_v3.len(), 3);
        assert!(snapshot
            .history_samples_v3
            .iter()
            .all(|sample| sample.model_source == "legacy-unknown"));
        assert_eq!(
            snapshot
                .history_samples_v3
                .iter()
                .filter(|sample| sample.models_complete)
                .count(),
            1
        );
        let complete_history = snapshot
            .history_samples_v3
            .iter()
            .find(|sample| sample.models_complete)
            .expect("the sidecar-complete timestamp is projected");
        assert_eq!(complete_history.models.as_ref().unwrap()[0].model, "SOL");
        assert_eq!(
            snapshot.history_samples_v3[0]
                .models
                .as_ref()
                .unwrap()
                .first()
                .unwrap()
                .model,
            "LUNA"
        );
        assert_eq!(
            snapshot.history_samples_v2[0].model_source,
            "legacy-unknown"
        );
        assert!(snapshot
            .details
            .history_samples
            .iter()
            .all(|sample| { sample.reset_at == 1_800_000_604 }));
        fs::remove_file(path).expect("cleanup");
    }
}

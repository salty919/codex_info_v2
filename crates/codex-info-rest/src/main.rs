use codex_info_account_locator::{
    current_account_scope_id, locate_existing_partition, locate_existing_partitions,
    AccountPartition, LocatorError, LocatorErrorKind,
};
use codex_info_db_reader::{
    CollectionCommit, DbReader, ReadInterval, ReadIntervals, StoragePartitionIdentity,
};
use codex_info_rest::{
    loopback_addr, AccountReader, RecorderCommitNotice, RecorderSourceUnavailableReason, RestServer,
};
use codex_info_rest_contract::PublicState;
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const REST_INTERNAL_VERSION: &str = env!("CARGO_PKG_VERSION");
const RECORDER_STATE_MAX_BYTES: u64 = 4 * 1_024;
const RECORDER_STATE_FRESHNESS_SECS: i64 = 150;

#[derive(Clone, Copy)]
enum LocatorDiagnosticStage {
    CurrentPartition,
    CatalogCurrentPartition,
}

impl LocatorDiagnosticStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CurrentPartition => "current_partition",
            Self::CatalogCurrentPartition => "catalog_current_partition",
        }
    }
}

fn locator_failure_reason(kind: LocatorErrorKind) -> &'static str {
    match kind {
        LocatorErrorKind::UnsafeRoot => "unsafe_root",
        LocatorErrorKind::UnsafeFile => "unsafe_file",
        LocatorErrorKind::InvalidAuth => "invalid_auth",
        LocatorErrorKind::InvalidMetadata => "invalid_metadata",
        LocatorErrorKind::RecoveryRequired => "recovery_required",
        LocatorErrorKind::Io => "io",
    }
}

fn observe_locator_result<T>(
    result: Result<T, LocatorError>,
    stage: LocatorDiagnosticStage,
    mut sink: impl FnMut(&'static str, &'static str),
) -> Result<T, LocatorError> {
    if let Err(error) = &result {
        sink(stage.as_str(), locator_failure_reason(error.kind()));
    }
    result
}

fn log_locator_failure(stage: &'static str, reason: &'static str) {
    eprintln!(
        "{}",
        serde_json::json!({
            "schema": "codex-info-locator-diagnostic-v1",
            "stage": stage,
            "reason": reason,
        })
    );
}

struct AccountCatalog {
    readers: Vec<AccountReader>,
    default_account_id: String,
    current_scope_id: String,
    current_partition_id: String,
    current_activation_at: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RecorderPublication {
    Idle {
        owner: RecorderOwner,
    },
    Active {
        owner: RecorderOwner,
        partition_id: String,
        commit: Option<CollectionCommit>,
        last_commit_unix: Option<i64>,
        degraded: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecorderOwner {
    pid: u64,
    process_starttime: u64,
    nonce: String,
}

impl RecorderPublication {
    fn commit_notice(&self) -> Option<RecorderCommitNotice> {
        match self {
            Self::Active {
                owner,
                partition_id,
                commit: Some(commit),
                degraded,
                ..
            } => Some(RecorderCommitNotice {
                pid: owner.pid,
                process_starttime: owner.process_starttime,
                owner_nonce: owner.nonce.clone(),
                partition_id: partition_id.clone(),
                commit: *commit,
                degraded: *degraded,
            }),
            _ => None,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RecorderStateWire {
    schema: Value,
    pid: Value,
    process_starttime: Value,
    owner_nonce: Value,
    write_state: Value,
    partition_id_hash: Value,
    data_generation: Value,
    collector_epoch: Value,
    cycle_seq: Value,
    last_commit_unix: Value,
    updated_at_unix: Value,
    #[serde(default)]
    recorder_version: Option<Value>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("codex_info_rest: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut port = "8787".to_owned();
    let mut database = None;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--port" => {
                port = args
                    .next()
                    .ok_or_else(|| "--port requires a value".to_owned())?;
            }
            "--db" | "--database" => {
                database = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--db requires a path".to_owned())?,
                ));
            }
            "--help" | "-h" => {
                println!("usage: codex_info_rest [--port PORT]\n       codex_info_rest --version");
                return Ok(());
            }
            "--version" | "-V" => {
                println!("{REST_INTERNAL_VERSION}");
                return Ok(());
            }
            value if value.strip_prefix("--port=").is_some() => {
                port = value
                    .strip_prefix("--port=")
                    .expect("prefix checked")
                    .to_owned();
            }
            value if value.strip_prefix("--db=").is_some() => {
                database = Some(PathBuf::from(
                    value.strip_prefix("--db=").expect("prefix checked"),
                ));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    // Production REST must always use the partition selected by the account
    // locator.  A direct path is intentionally available only to explicit
    // debug fixtures; release builds and systemd-managed processes can never
    // opt into that escape hatch.
    let environment_database = env::var_os("CODEX_INFO_DB_PATH").map(PathBuf::from);
    let requested_database = database.or(environment_database);
    let fixture_mode = fixture_database_mode();
    validate_database_override(requested_database.as_deref(), fixture_mode)?;
    let address = loopback_addr(&port).map_err(|error| error.to_string())?;
    if let Some(path) = requested_database {
        if !fixture_mode {
            unreachable!("non-fixture database path rejected above");
        }
        let database = absolutize(path)?;
        let reader = DbReader::open(&database).map_err(|error| error.to_string())?;
        let server = RestServer::start_with_accounts(
            vec![AccountReader::new("account-1", 1, true, None, None, reader)],
            "account-1",
            address,
        )
        .map_err(|error| error.to_string())?;
        eprintln!(
            "codex_info_rest: read-only fixture listener={} accounts=1 default=account-1",
            server.local_addr(),
        );
        loop {
            thread::sleep(Duration::from_secs(1));
            if server.store().publication_failed() {
                return Err("REST publisher permanently failed".to_owned());
            }
        }
    }

    let (codex_home, data_root) = account_roots()?;
    let recorder_publication = read_recorder_publication(&data_root).ok();
    let (catalog, startup_catalog_failure) =
        match default_account_readers(&codex_home, &data_root, recorder_publication.as_ref()) {
            Ok(catalog) => (Some(catalog), None),
            Err(stage) => (None, Some(stage)),
        };
    let initial_state = catalog.as_ref().and_then(|catalog| {
        catalog_is_fresh(catalog, recorder_publication.as_ref()).then_some(PublicState::Ready)
    });
    let mut boundary_seen = initial_state != Some(PublicState::Ready);
    let (server, mut startup_scope, mut startup_partition) = match catalog {
        Some(catalog) => {
            let server = RestServer::start_with_accounts(
                catalog.readers,
                &catalog.default_account_id,
                address,
            )
            .map_err(|error| error.to_string())?;
            if boundary_seen {
                server
                    .store()
                    .publish_account_boundary(PublicState::Initializing);
            }
            (
                server,
                Some(catalog.current_scope_id),
                Some(catalog.current_partition_id),
            )
        }
        None => {
            let auth_scope_available = current_account_scope_id(&codex_home, &data_root).is_ok();
            let state = boundary_state_without_catalog(
                &codex_home,
                recorder_publication.as_ref(),
                auth_scope_available,
            );
            (
                RestServer::start_without_account(state, address)
                    .map_err(|error| error.to_string())?,
                None,
                None,
            )
        }
    };
    server.store().set_log_data_root(&data_root);
    let runtime_data_root = data_root.clone();
    server.store().set_recorder_version_reader(move || {
        read_recorder_publication_with_version(&runtime_data_root)
            .ok()
            .and_then(|(_, version)| version)
    });
    eprintln!(
        "codex_info_rest: read-only listener={} accounts={} default={}",
        server.local_addr(),
        server.store().account_descriptors().len(),
        server
            .store()
            .default_account_id()
            .as_deref()
            .unwrap_or("none"),
    );

    if let Some(stage) = startup_catalog_failure {
        eprintln!("codex_info_rest: catalog unavailable stage={stage}");
    }
    let mut catalog_deadline = Instant::now() + Duration::from_secs(60);
    loop {
        thread::sleep(Duration::from_secs(1));
        if server.store().publication_failed() {
            return Err("REST publisher permanently failed".to_owned());
        }
        let publication = read_recorder_publication(&data_root);
        let current_scope = current_account_scope_id(&codex_home, &data_root).ok();
        let current_partition = current_scope.as_deref().and_then(|scope| {
            observe_locator_result(
                locate_existing_partition(&codex_home, &data_root),
                LocatorDiagnosticStage::CurrentPartition,
                log_locator_failure,
            )
            .ok()
            .filter(|partition| partition.account_scope_id == scope)
        });
        let source_unavailable_reason = current_partition.as_ref().map(|partition| {
            recorder_source_unavailable_reason(
                publication.as_ref().map_err(String::as_str),
                &partition.partition_id,
                partition.current_interval_start,
            )
        });
        let publication = publication.ok();
        match (current_scope.as_deref(), current_partition.as_ref()) {
            (Some(scope), Some(partition))
                if recorder_matches_partition(partition, publication.as_ref()) =>
            {
                if startup_scope.as_deref() != Some(scope)
                    || startup_partition.as_deref() != Some(partition.partition_id.as_str())
                    || Instant::now() >= catalog_deadline
                {
                    match default_account_readers_with_locator_diagnostics(
                        &codex_home,
                        &data_root,
                        publication.as_ref(),
                        log_locator_failure,
                    ) {
                        Ok(catalog) => {
                            let changed = server
                                .store()
                                .reconcile_accounts(catalog.readers, catalog.default_account_id)
                                .map_err(|error| error.to_string())?;
                            startup_scope = Some(catalog.current_scope_id);
                            startup_partition = Some(catalog.current_partition_id);
                            boundary_seen |= changed;
                            catalog_deadline = Instant::now() + Duration::from_secs(60);
                        }
                        Err(stage) => {
                            eprintln!("codex_info_rest: catalog unavailable stage={stage}");
                            server.store().publish_account_boundary(PublicState::Error);
                            boundary_seen = true;
                            continue;
                        }
                    }
                }
                if let Some(notice) = publication
                    .as_ref()
                    .and_then(RecorderPublication::commit_notice)
                {
                    let id = partition.public_id();
                    server.store().notify_recorder_commit(&id, notice.clone());
                    if boundary_seen && server.store().recorder_commit_is_published(&id, &notice) {
                        server.store().clear_account_boundary();
                        boundary_seen = false;
                    }
                }
            }
            (Some(scope), partition) => {
                if startup_scope.as_deref() == Some(scope)
                    && partition.is_some_and(|partition| {
                        startup_partition.as_deref() == Some(partition.partition_id.as_str())
                    })
                {
                    if let Some(id) = server.store().default_account_id() {
                        server.store().mark_recorder_source_unavailable_with_reason(
                            &id,
                            source_unavailable_reason
                                .unwrap_or(RecorderSourceUnavailableReason::Unspecified),
                        );
                    }
                } else {
                    server
                        .store()
                        .publish_account_boundary(PublicState::Initializing);
                    boundary_seen = true;
                }
            }
            (None, _) => {
                server
                    .store()
                    .publish_account_boundary(boundary_state_without_catalog(
                        &codex_home,
                        publication.as_ref(),
                        false,
                    ));
                boundary_seen = true;
            }
        }
    }
}

fn fixture_database_mode() -> bool {
    cfg!(debug_assertions)
        && env::var_os("CODEX_INFO_REST_TEST_MODE").is_some_and(|value| value == "1")
        && env::var_os("CODEX_INFO_SYSTEMD_MANAGED").is_none()
}

fn validate_database_override(
    requested_database: Option<&std::path::Path>,
    fixture_mode: bool,
) -> Result<(), String> {
    if requested_database.is_some() && !fixture_mode {
        return Err(
            "explicit REST database paths are test-only; use the account locator in production"
                .to_owned(),
        );
    }
    Ok(())
}

fn absolutize(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| format!("resolve database path: {error}"))
    }
}

fn account_roots() -> Result<(PathBuf, PathBuf), String> {
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or_else(|| {
            "CODEX_HOME or HOME is required to locate the account database".to_owned()
        })?;
    let codex_home = absolutize(codex_home)?;
    let data_root = env::var_os("CODEX_INFO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| codex_home.clone());
    let data_root = absolutize(data_root)?;
    Ok((codex_home, data_root))
}

fn default_account_readers(
    codex_home: &Path,
    data_root: &Path,
    publication: Option<&RecorderPublication>,
) -> Result<AccountCatalog, &'static str> {
    default_account_readers_with_locator_diagnostics(
        codex_home,
        data_root,
        publication,
        |_stage, _reason| {},
    )
}

fn default_account_readers_with_locator_diagnostics(
    codex_home: &Path,
    data_root: &Path,
    publication: Option<&RecorderPublication>,
    sink: impl FnMut(&'static str, &'static str),
) -> Result<AccountCatalog, &'static str> {
    let current = observe_locator_result(
        locate_existing_partition(codex_home, data_root),
        LocatorDiagnosticStage::CatalogCurrentPartition,
        sink,
    )
    .map_err(|_| "current-partition")?;
    let partitions =
        locate_existing_partitions(codex_home, data_root).map_err(|_| "account-partitions")?;
    let default_account_id = current.public_id();
    let mut readers = Vec::with_capacity(partitions.len());
    for partition in partitions {
        let identity = partition_identity(&partition);
        let read_intervals =
            partition_read_intervals(&partition).map_err(|_| "ownership-intervals")?;
        let reader = DbReader::open_partitioned_with_intervals(
            &partition.database_path,
            &identity,
            read_intervals,
        )
        .map_err(|_| "partition-reader")?;
        let database_login_id = reader
            .partition_login_id()
            .map_err(|_| "partition-display-identity")?;
        let is_current = partition.account_scope_id == current.account_scope_id;
        let account_reader = AccountReader::new(
            partition.public_id(),
            partition.storage_epoch,
            is_current,
            partition.current_interval_start,
            partition.current_interval_end,
            reader,
        )
        .with_login_id(database_login_id.or(partition.login_id.clone()))
        .with_ownership_intervals(
            partition
                .lifecycle_intervals
                .iter()
                .map(
                    |interval| codex_info_rest_contract::PublicAccountOwnershipInterval {
                        start_at: interval.start_at,
                        end_at: interval.end_at,
                    },
                )
                .collect(),
        );
        readers.push(if is_current {
            let notice = recorder_matches_partition(&partition, publication)
                .then(|| publication.and_then(RecorderPublication::commit_notice))
                .flatten();
            account_reader.with_recorder_commit(notice)
        } else {
            account_reader
        });
    }
    Ok(AccountCatalog {
        readers,
        default_account_id,
        current_scope_id: current.account_scope_id.clone(),
        current_partition_id: current.partition_id.clone(),
        current_activation_at: current.current_interval_start,
    })
}

fn catalog_is_fresh(catalog: &AccountCatalog, publication: Option<&RecorderPublication>) -> bool {
    matches!(
        publication,
        Some(RecorderPublication::Active {
            partition_id,
            commit: Some(_),
            last_commit_unix: Some(last_commit_unix),
            ..
        }) if partition_id == &catalog.current_partition_id
            && catalog
                .current_activation_at
                .is_none_or(|activation| *last_commit_unix >= activation)
    )
}

fn recorder_matches_partition(
    partition: &AccountPartition,
    publication: Option<&RecorderPublication>,
) -> bool {
    matches!(
        publication,
        Some(RecorderPublication::Active {
            partition_id,
            commit: Some(_),
            last_commit_unix: Some(last_commit_unix),
            ..
        }) if partition_id == &partition.partition_id
            && partition
                .current_interval_start
                .is_none_or(|activation| *last_commit_unix >= activation)
    )
}

fn recorder_source_unavailable_reason(
    publication: Result<&RecorderPublication, &str>,
    partition_id: &str,
    activation_at: Option<i64>,
) -> RecorderSourceUnavailableReason {
    use RecorderSourceUnavailableReason as Reason;
    match publication {
        Err(error) => match error {
            "recorder publication is unavailable" => Reason::PublicationUnavailable,
            "recorder publication file is invalid" => Reason::FileInvalid,
            "recorder publication file is not owner-private" => Reason::FileNotPrivate,
            "recorder publication read failed" => Reason::ReadFailed,
            "recorder publication JSON is invalid" => Reason::JsonInvalid,
            "recorder publication identity is invalid"
            | "recorder partition identity is invalid"
            | "recorder generation is invalid" => Reason::IdentityInvalid,
            "recorder publication owner is not live" => Reason::OwnerNotLive,
            "recorder publication is stale" => Reason::PublicationStale,
            "recorder commit is stale" => Reason::CommitStale,
            "idle recorder publication is inconsistent" | "recorder write state is invalid" => {
                Reason::InconsistentField
            }
            _ => Reason::Unspecified,
        },
        Ok(RecorderPublication::Idle { .. }) => Reason::Idle,
        Ok(RecorderPublication::Active {
            partition_id: published_partition,
            commit,
            last_commit_unix,
            ..
        }) => {
            if published_partition != partition_id {
                Reason::PartitionMismatch
            } else if commit.is_none() || last_commit_unix.is_none() {
                Reason::CommitMissing
            } else if activation_at
                .zip(*last_commit_unix)
                .is_some_and(|(activation, commit_at)| commit_at < activation)
            {
                Reason::BeforeActivation
            } else {
                Reason::Unspecified
            }
        }
    }
}

fn boundary_state_without_catalog(
    codex_home: &Path,
    publication: Option<&RecorderPublication>,
    auth_scope_available: bool,
) -> PublicState {
    let auth_missing = fs::symlink_metadata(codex_home.join("auth.json"))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    if auth_missing && matches!(publication, Some(RecorderPublication::Idle { .. })) {
        PublicState::AuthRequired
    } else if auth_scope_available {
        PublicState::Initializing
    } else {
        PublicState::Error
    }
}

fn read_recorder_publication(data_root: &Path) -> Result<RecorderPublication, String> {
    read_recorder_publication_with_version(data_root).map(|(publication, _)| publication)
}

fn read_recorder_publication_with_version(
    data_root: &Path,
) -> Result<(RecorderPublication, Option<String>), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is invalid".to_owned())?
        .as_secs();
    let now = i64::try_from(now).map_err(|_| "system clock is invalid".to_owned())?;
    read_recorder_publication_at(data_root, now)
}

fn read_recorder_publication_at(
    data_root: &Path,
    now: i64,
) -> Result<(RecorderPublication, Option<String>), String> {
    let path = data_root.join("history/recorder-state.json");
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "recorder publication is unavailable".to_owned())?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || !(1..=RECORDER_STATE_MAX_BYTES).contains(&metadata.len())
    {
        return Err("recorder publication file is invalid".to_owned());
    }
    #[cfg(unix)]
    if metadata.mode() & 0o777 != 0o600 {
        return Err("recorder publication file is not owner-private".to_owned());
    }
    let bytes = fs::read(&path).map_err(|_| "recorder publication read failed".to_owned())?;
    let parsed: RecorderStateWire = serde_json::from_slice(&bytes)
        .map_err(|_| "recorder publication JSON is invalid".to_owned())?;
    let value = serde_json::to_value(parsed)
        .map_err(|_| "recorder publication JSON is invalid".to_owned())?;
    let object = value
        .as_object()
        .ok_or_else(|| "recorder publication is not an object".to_owned())?;
    let mut expected = BTreeSet::from([
        "schema",
        "pid",
        "process_starttime",
        "owner_nonce",
        "write_state",
        "partition_id_hash",
        "data_generation",
        "collector_epoch",
        "cycle_seq",
        "last_commit_unix",
        "updated_at_unix",
    ]);
    if object.contains_key("recorder_version") {
        expected.insert("recorder_version");
    }
    if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected
        || object.get("schema").and_then(Value::as_str) != Some("codex-info-recorder-state-v1")
        || !object
            .get("pid")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0)
        || !object
            .get("process_starttime")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0)
        || !object
            .get("owner_nonce")
            .and_then(Value::as_str)
            .is_some_and(|value| valid_lower_hex(value, 32))
    {
        return Err("recorder publication identity is invalid".to_owned());
    }
    if !recorder_process_is_live(object) {
        return Err("recorder publication owner is not live".to_owned());
    }
    let owner = RecorderOwner {
        pid: object["pid"].as_u64().expect("validated PID"),
        process_starttime: object["process_starttime"]
            .as_u64()
            .expect("validated process identity"),
        nonce: object["owner_nonce"]
            .as_str()
            .expect("validated owner nonce")
            .to_owned(),
    };
    let updated_at = object
        .get("updated_at_unix")
        .and_then(Value::as_i64)
        .filter(|value| {
            *value > 0 && *value <= now && now - *value <= RECORDER_STATE_FRESHNESS_SECS
        })
        .ok_or_else(|| "recorder publication is stale".to_owned())?;
    let _ = updated_at;
    let publication = match object.get("write_state").and_then(Value::as_str) {
        Some("idle_no_account") => {
            for key in [
                "partition_id_hash",
                "data_generation",
                "collector_epoch",
                "cycle_seq",
                "last_commit_unix",
            ] {
                if object.get(key) != Some(&Value::Null) {
                    return Err("idle recorder publication is inconsistent".to_owned());
                }
            }
            Ok(RecorderPublication::Idle { owner })
        }
        Some(write_state @ ("ready" | "degraded")) => {
            let partition_id = object
                .get("partition_id_hash")
                .and_then(Value::as_str)
                .filter(|value| valid_lower_hex(value, 64))
                .ok_or_else(|| "recorder partition identity is invalid".to_owned())?;
            if write_state == "degraded"
                && [
                    "data_generation",
                    "collector_epoch",
                    "cycle_seq",
                    "last_commit_unix",
                ]
                .into_iter()
                .all(|key| object.get(key) == Some(&Value::Null))
            {
                return Ok((
                    RecorderPublication::Active {
                        owner,
                        partition_id: partition_id.to_owned(),
                        commit: None,
                        last_commit_unix: None,
                        degraded: true,
                    },
                    object
                        .get("recorder_version")
                        .and_then(Value::as_str)
                        .filter(|version| codex_info_rest_contract::valid_runtime_version(version))
                        .map(str::to_owned),
                ));
            }
            let last_commit_unix = object
                .get("last_commit_unix")
                .and_then(Value::as_i64)
                .filter(|value| {
                    *value > 0 && *value <= now && now - *value <= RECORDER_STATE_FRESHNESS_SECS
                })
                .ok_or_else(|| "recorder commit is stale".to_owned())?;
            if !object
                .get("data_generation")
                .and_then(Value::as_u64)
                .is_some_and(|value| value > 0)
                || !object
                    .get("collector_epoch")
                    .and_then(Value::as_str)
                    .is_some_and(|value| valid_lower_hex(value, 32))
                || !object
                    .get("cycle_seq")
                    .and_then(Value::as_u64)
                    .is_some_and(|value| value > 0)
            {
                return Err("recorder generation is invalid".to_owned());
            }
            Ok(RecorderPublication::Active {
                owner,
                partition_id: partition_id.to_owned(),
                commit: Some(CollectionCommit {
                    generation: object["data_generation"]
                        .as_u64()
                        .expect("validated generation"),
                    collector_epoch: Some(
                        u128::from_str_radix(
                            object["collector_epoch"].as_str().expect("validated epoch"),
                            16,
                        )
                        .expect("validated epoch"),
                    ),
                    cycle_seq: object["cycle_seq"].as_u64().expect("validated sequence"),
                }),
                last_commit_unix: Some(last_commit_unix),
                degraded: write_state == "degraded",
            })
        }
        _ => Err("recorder write state is invalid".to_owned()),
    }?;
    let version = object
        .get("recorder_version")
        .and_then(Value::as_str)
        .filter(|version| codex_info_rest_contract::valid_runtime_version(version))
        .filter(|_| recorder_process_is_live(object))
        .map(str::to_owned);
    Ok((publication, version))
}

fn recorder_process_is_live(object: &serde_json::Map<String, Value>) -> bool {
    let Some(pid) = object.get("pid").and_then(Value::as_u64) else {
        return false;
    };
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, tail)) = stat.rsplit_once(')') else {
        return false;
    };
    let fields = tail.split_whitespace().collect::<Vec<_>>();
    !matches!(fields.first().copied(), None | Some("Z" | "X"))
        && fields.get(19).and_then(|value| value.parse::<u64>().ok())
            == object.get("process_starttime").and_then(Value::as_u64)
}

fn valid_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value.bytes().any(|byte| byte != b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn partition_read_intervals(partition: &AccountPartition) -> Result<ReadIntervals, String> {
    let intervals = partition
        .lifecycle_intervals
        .iter()
        .map(|interval| ReadInterval::new(interval.start_at, interval.end_at))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "account partition {} has an invalid lifecycle interval: {error}",
                partition.public_id()
            )
        })?;
    ReadIntervals::new(intervals).map_err(|error| {
        format!(
            "account partition {} has invalid lifecycle ownership: {error}",
            partition.public_id()
        )
    })
}

fn partition_identity(partition: &AccountPartition) -> StoragePartitionIdentity {
    let identity = partition.storage_identity();
    StoragePartitionIdentity {
        schema_version: identity.schema_version,
        profile_scope_id: identity.profile_scope_id,
        account_scope_id: identity.account_scope_id,
        storage_epoch: identity.storage_epoch,
        partition_id: identity.partition_id,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        boundary_state_without_catalog, catalog_is_fresh, fixture_database_mode,
        read_recorder_publication, validate_database_override, AccountCatalog, RecorderPublication,
        REST_INTERNAL_VERSION,
    };
    use codex_info_rest_contract::PublicState;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn issue604_locator_diagnostic_preserves_result() {
        use super::{
            locator_failure_reason, observe_locator_result, LocatorDiagnosticStage,
            LocatorErrorKind,
        };

        let kinds = [
            LocatorErrorKind::UnsafeRoot,
            LocatorErrorKind::UnsafeFile,
            LocatorErrorKind::InvalidAuth,
            LocatorErrorKind::InvalidMetadata,
            LocatorErrorKind::RecoveryRequired,
            LocatorErrorKind::Io,
        ];
        let reasons = kinds.map(locator_failure_reason);
        let mut notifications = Vec::new();
        let value = observe_locator_result(
            Ok(7),
            LocatorDiagnosticStage::CurrentPartition,
            |stage, reason| notifications.push((stage, reason)),
        );
        assert_eq!(value, Ok(7));
        assert!(notifications.is_empty(), "success has no diagnostic");

        // Relative roots are rejected before filesystem/database access.
        let original = super::locate_existing_partition(
            Path::new("literal-relative-root"),
            Path::new("literal-relative-data"),
        )
        .unwrap_err();
        assert_eq!(original.kind(), LocatorErrorKind::UnsafeRoot);
        for stage in [
            LocatorDiagnosticStage::CurrentPartition,
            LocatorDiagnosticStage::CatalogCurrentPartition,
        ] {
            let returned = observe_locator_result::<u8>(Err(original), stage, |stage, reason| {
                notifications.push((stage, reason));
            });
            assert_eq!(
                returned,
                Err(original),
                "the original typed error is retained"
            );
        }
        assert_eq!(
            notifications,
            [
                ("current_partition", "unsafe_root"),
                ("catalog_current_partition", "unsafe_root"),
            ],
            "exactly one fixed diagnostic for each failed locator result"
        );
        assert_eq!(
            reasons,
            [
                "unsafe_root",
                "unsafe_file",
                "invalid_auth",
                "invalid_metadata",
                "recovery_required",
                "io",
            ]
        );
        assert_eq!(
            LocatorDiagnosticStage::CurrentPartition.as_str(),
            "current_partition"
        );
        assert_eq!(
            LocatorDiagnosticStage::CatalogCurrentPartition.as_str(),
            "catalog_current_partition"
        );
    }

    #[test]
    fn internal_version_is_independent_from_distribution_product_version() {
        assert_eq!(REST_INTERNAL_VERSION, "0.1.0");
        assert_ne!(REST_INTERNAL_VERSION, env!("CODEX_INFO_PRODUCT_VERSION"));
    }

    #[test]
    fn fixture_database_mode_is_disabled_without_explicit_test_mode() {
        // The test process does not opt into the fixture escape hatch.  In
        // particular, debug assertions alone must not make a path override
        // usable by an ordinary local invocation.
        if std::env::var_os("CODEX_INFO_REST_TEST_MODE").is_none() {
            assert!(!fixture_database_mode());
        }
    }

    #[test]
    fn explicit_database_override_is_rejected_outside_fixture_mode() {
        let error = validate_database_override(Some(Path::new("/tmp/fixture.sqlite3")), false)
            .expect_err("production must reject a direct database path");
        assert!(error.contains("test-only"));
        validate_database_override(Some(Path::new("/tmp/fixture.sqlite3")), true)
            .expect("explicit fixture mode permits isolated test databases");
    }

    #[test]
    fn issue604_recorder_state_freshness_has_exact_inclusive_boundaries() {
        const NOW: i64 = 1_800_000_120;
        let root = tempfile::tempdir().unwrap();
        let mut outcomes = Vec::new();
        for field in ["updated_at_unix", "last_commit_unix"] {
            for offset in [-151, -150, 0, 1, 5] {
                let mut value = issue604_live_recorder_state();
                value["updated_at_unix"] = serde_json::json!(NOW);
                value["last_commit_unix"] = serde_json::json!(NOW);
                value[field] = serde_json::json!(NOW + offset);
                let bytes = serde_json::to_vec(&value).unwrap();
                write_issue604_recorder_state(root.path(), &bytes);
                outcomes.push((
                    field,
                    offset,
                    super::read_recorder_publication_at(root.path(), NOW).is_ok(),
                ));
                assert_eq!(
                    fs::read(root.path().join("history/recorder-state.json")).unwrap(),
                    bytes,
                    "state validation must not clamp or rewrite source timestamps"
                );
            }
        }
        assert_eq!(
            outcomes,
            [
                ("updated_at_unix", -151, false),
                ("updated_at_unix", -150, true),
                ("updated_at_unix", 0, true),
                ("updated_at_unix", 1, false),
                ("updated_at_unix", 5, false),
                ("last_commit_unix", -151, false),
                ("last_commit_unix", -150, true),
                ("last_commit_unix", 0, true),
                ("last_commit_unix", 1, false),
                ("last_commit_unix", 5, false),
            ],
            "only positive timestamps in the exact closed interval [now-150,now] are fresh"
        );
    }

    #[test]
    fn issue604_unavailable_diagnostics_source_reason() {
        let mut actual = Vec::new();
        for (error, expected) in [
            (
                "recorder publication is unavailable",
                "publication_unavailable",
            ),
            ("recorder publication file is invalid", "file_invalid"),
            (
                "recorder publication file is not owner-private",
                "file_not_private",
            ),
            ("recorder publication read failed", "read_failed"),
            ("recorder publication JSON is invalid", "json_invalid"),
            (
                "recorder publication identity is invalid",
                "identity_invalid",
            ),
            ("recorder partition identity is invalid", "identity_invalid"),
            ("recorder generation is invalid", "identity_invalid"),
            ("recorder publication owner is not live", "owner_not_live"),
            ("recorder publication is stale", "publication_stale"),
            ("recorder commit is stale", "commit_stale"),
            (
                "idle recorder publication is inconsistent",
                "inconsistent_field",
            ),
            ("recorder write state is invalid", "inconsistent_field"),
            ("system clock is invalid", "unspecified"),
            (
                "UNKNOWN_PRIVATE_SENTINEL token=/private/path account=secret",
                "unspecified",
            ),
        ] {
            let reason =
                super::recorder_source_unavailable_reason(Err(error), "partition-a", Some(100));
            actual.push(reason.as_str());
            assert_eq!(reason.as_str(), expected, "fixed error category");
        }
        let owner = super::RecorderOwner {
            pid: 987654,
            process_starttime: 123456,
            nonce: "PRIVATE_OWNER_SENTINEL".to_owned(),
        };
        let mut publication = RecorderPublication::Active {
            owner: owner.clone(),
            partition_id: "partition-a".to_owned(),
            commit: Some(super::CollectionCommit {
                generation: 42,
                collector_epoch: Some(17),
                cycle_seq: 19,
            }),
            last_commit_unix: Some(100),
            degraded: false,
        };
        for (partition, activation, expected) in [
            ("partition-b", Some(100), "partition_mismatch"),
            ("partition-a", Some(101), "before_activation"),
            ("partition-a", Some(100), "unspecified"),
            ("partition-a", None, "unspecified"),
        ] {
            let reason =
                super::recorder_source_unavailable_reason(Ok(&publication), partition, activation);
            actual.push(reason.as_str());
            assert_eq!(reason.as_str(), expected);
        }
        if let RecorderPublication::Active { commit, .. } = &mut publication {
            *commit = None;
        }
        let reason =
            super::recorder_source_unavailable_reason(Ok(&publication), "partition-a", Some(100));
        actual.push(reason.as_str());
        assert_eq!(reason.as_str(), "commit_missing");
        let reason = super::recorder_source_unavailable_reason(
            Ok(&RecorderPublication::Idle { owner }),
            "partition-a",
            Some(100),
        );
        actual.push(reason.as_str());
        assert_eq!(reason.as_str(), "idle");
        let output = actual.join(" ");
        for private in [
            "PRIVATE",
            "987654",
            "123456",
            "partition-a",
            "partition-b",
            "secret",
            "/private/path",
        ] {
            assert!(!output.contains(private));
        }
    }

    #[test]
    fn issue604_recorder_commit_identity_changes_without_heartbeat_changes() {
        let root = tempfile::tempdir().unwrap();
        let mut value = issue604_live_recorder_state();
        write_issue604_recorder_state(root.path(), &serde_json::to_vec(&value).unwrap());
        let first = read_recorder_publication(root.path()).unwrap();
        value["updated_at_unix"] =
            serde_json::json!(value["updated_at_unix"].as_i64().unwrap() - 1);
        write_issue604_recorder_state(root.path(), &serde_json::to_vec(&value).unwrap());
        assert_eq!(
            first,
            read_recorder_publication(root.path()).unwrap(),
            "heartbeat metadata cannot create a new commit identity"
        );
        for key in ["data_generation", "cycle_seq"] {
            let mut changed = value.clone();
            changed[key] = serde_json::json!(changed[key].as_u64().unwrap() + 1);
            write_issue604_recorder_state(root.path(), &serde_json::to_vec(&changed).unwrap());
            assert_ne!(
                first,
                read_recorder_publication(root.path()).unwrap(),
                "{key} must remain in the accepted commit identity"
            );
        }
    }

    #[test]
    fn issue604_degraded_without_a_commit_is_distinct_from_partial_commit_fields() {
        let root = tempfile::tempdir().unwrap();
        let mut value = issue604_live_recorder_state();
        value["write_state"] = serde_json::json!("degraded");
        for key in [
            "data_generation",
            "collector_epoch",
            "cycle_seq",
            "last_commit_unix",
        ] {
            value[key] = serde_json::Value::Null;
        }
        write_issue604_recorder_state(root.path(), &serde_json::to_vec(&value).unwrap());
        assert!(
            read_recorder_publication(root.path()).is_ok(),
            "an explicit unconfirmed commit is a valid degraded source state"
        );
        value["data_generation"] = serde_json::json!(7);
        write_issue604_recorder_state(root.path(), &serde_json::to_vec(&value).unwrap());
        assert!(
            read_recorder_publication(root.path()).is_err(),
            "partial commit fields cannot be completed from DB state"
        );
    }

    #[test]
    fn issue604_recorder_state_rejects_dead_owner_and_duplicate_fields() {
        let root = tempfile::tempdir().unwrap();
        let mut value = issue604_live_recorder_state();
        value["process_starttime"] =
            serde_json::json!(value["process_starttime"].as_u64().unwrap() + 1);
        write_issue604_recorder_state(root.path(), &serde_json::to_vec(&value).unwrap());
        assert!(
            read_recorder_publication(root.path()).is_err(),
            "a live PID with another start time is not the recorder owner"
        );
        let value = issue604_live_recorder_state();
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.pop();
        bytes.extend_from_slice(b",\"data_generation\":7}");
        write_issue604_recorder_state(root.path(), &bytes);
        assert!(
            read_recorder_publication(root.path()).is_err(),
            "duplicate source fields must not select an arbitrary value"
        );
    }

    fn issue604_live_recorder_state() -> serde_json::Value {
        let stat = fs::read_to_string("/proc/self/stat").unwrap();
        let (_, tail) = stat.rsplit_once(')').unwrap();
        let starttime = tail
            .split_whitespace()
            .nth(19)
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        serde_json::json!({
            "schema":"codex-info-recorder-state-v1", "pid":std::process::id(),
            "process_starttime":starttime, "owner_nonce":"11111111111111111111111111111111",
            "write_state":"ready", "partition_id_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "data_generation":7, "collector_epoch":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "cycle_seq":3,
            "last_commit_unix":now, "updated_at_unix":now
        })
    }

    fn write_issue604_recorder_state(root: &Path, bytes: &[u8]) {
        let history = root.join("history");
        fs::create_dir_all(&history).unwrap();
        let path = history.join("recorder-state.json");
        fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn issue604_partition_reader_accepts_same_second_empty_interval() {
        use super::partition_read_intervals;
        use codex_info_account_locator::AccountPartition;
        use std::path::PathBuf;
        let partition = AccountPartition {
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 1,
            partition_id: "33".repeat(32),
            database_path: PathBuf::from("/unused-audit-database"),
            login_id: None,
            activation_timestamp: Some(200),
            lifecycle_intervals: vec![
                codex_info_account_locator::AccountLifecycleInterval {
                    start_at: Some(100),
                    end_at: Some(100),
                },
                codex_info_account_locator::AccountLifecycleInterval {
                    start_at: Some(200),
                    end_at: None,
                },
            ],
            current_interval_start: Some(200),
            current_interval_end: None,
        };
        let intervals = partition_read_intervals(&partition)
            .expect("same-second transition is a valid read scope");
        assert_eq!(intervals.intervals().len(), 2);
        assert_eq!(intervals.intervals()[0].start_at, Some(100));
        assert_eq!(intervals.intervals()[0].end_at, Some(100));
    }

    #[test]
    fn recorder_publication_admits_logout_and_only_fresh_current_partition() {
        let root = tempfile::Builder::new()
            .prefix("codex-info-rest-account-boundary-")
            .tempdir()
            .expect("private account boundary root");
        let codex_home = root.path().join("codex-home");
        let history = root.path().join("history");
        fs::create_dir_all(&codex_home).expect("Codex home");
        fs::create_dir_all(&history).expect("history root");
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_secs(),
        )
        .expect("timestamp");
        let path = history.join("recorder-state.json");
        let live_owner = issue604_live_recorder_state();
        let write_state = |mut value: serde_json::Value| {
            value["pid"] = live_owner["pid"].clone();
            value["process_starttime"] = live_owner["process_starttime"].clone();
            fs::write(&path, serde_json::to_vec(&value).expect("state JSON"))
                .expect("recorder state");
            #[cfg(unix)]
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .expect("private recorder state");
        };

        write_state(serde_json::json!({
            "schema": "codex-info-recorder-state-v1",
            "recorder_version": "9.8.7",
            "pid": 1,
            "process_starttime": 1,
            "owner_nonce": "11111111111111111111111111111111",
            "write_state": "idle_no_account",
            "partition_id_hash": null,
            "data_generation": null,
            "collector_epoch": null,
            "cycle_seq": null,
            "last_commit_unix": null,
            "updated_at_unix": now
        }));
        let idle = read_recorder_publication(root.path()).expect("fresh idle publication");
        assert!(matches!(idle, RecorderPublication::Idle { .. }));
        assert_eq!(
            boundary_state_without_catalog(&codex_home, Some(&idle), false),
            PublicState::AuthRequired
        );
        fs::write(codex_home.join("auth.json"), b"{}").expect("invalid auth authority");
        assert_eq!(
            boundary_state_without_catalog(&codex_home, Some(&idle), false),
            PublicState::Error
        );

        write_state(serde_json::json!({
            "schema": "codex-info-recorder-state-v1",
            "pid": 1,
            "process_starttime": 1,
            "owner_nonce": "11111111111111111111111111111111",
            "write_state": "ready",
            "partition_id_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "data_generation": 7,
            "collector_epoch": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "cycle_seq": 3,
            "last_commit_unix": now,
            "updated_at_unix": now
        }));
        let active = read_recorder_publication(root.path()).expect("fresh active publication");
        let catalog = AccountCatalog {
            readers: Vec::new(),
            default_account_id: "account-7".to_owned(),
            current_scope_id: "scope".to_owned(),
            current_partition_id:
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            current_activation_at: Some(now),
        };
        assert!(catalog_is_fresh(&catalog, Some(&active)));
        let stale_activation = AccountCatalog {
            current_activation_at: Some(now + 1),
            ..catalog
        };
        assert!(!catalog_is_fresh(&stale_activation, Some(&active)));
    }
}

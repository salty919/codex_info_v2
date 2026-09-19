use codex_info_account_locator::{
    current_account_scope_id, locate_existing_partition, locate_existing_partitions,
    AccountPartition,
};
use codex_info_db_reader::{DbReader, ReadInterval, ReadIntervals, StoragePartitionIdentity};
use codex_info_rest::{loopback_addr, AccountReader, RestServer};
use codex_info_rest_contract::PublicState;
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const REST_INTERNAL_VERSION: &str = env!("CARGO_PKG_VERSION");
const RECORDER_STATE_MAX_BYTES: u64 = 4 * 1_024;
const RECORDER_STATE_FRESHNESS_SECS: i64 = 150;

struct AccountCatalog {
    readers: Vec<AccountReader>,
    default_account_id: String,
    current_scope_id: String,
    current_partition_id: String,
    current_activation_at: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RecorderPublication {
    Idle,
    Active {
        partition_id: String,
        last_commit_unix: i64,
    },
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
            thread::sleep(Duration::from_secs(3_600));
        }
    }

    let (codex_home, data_root) = account_roots()?;
    let recorder_publication = read_recorder_publication(&data_root).ok();
    let catalog = default_account_readers(&codex_home, &data_root).ok();
    let initial_state = catalog.as_ref().and_then(|catalog| {
        catalog_is_fresh(catalog, recorder_publication.as_ref()).then_some(PublicState::Ready)
    });
    let mut boundary_seen = initial_state != Some(PublicState::Ready);
    let (server, startup_scope, startup_partition) = match catalog {
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
    eprintln!(
        "codex_info_rest: read-only listener={} accounts={} default={}",
        server.local_addr(),
        server.store().account_descriptors().len(),
        server.store().default_account_id().unwrap_or("none"),
    );

    loop {
        thread::sleep(Duration::from_secs(1));
        let publication = read_recorder_publication(&data_root).ok();
        let current_scope = current_account_scope_id(&codex_home, &data_root).ok();
        let current_partition = current_scope.as_deref().and_then(|scope| {
            locate_existing_partition(&codex_home, &data_root)
                .ok()
                .filter(|partition| partition.account_scope_id == scope)
        });
        match (current_scope.as_deref(), current_partition.as_ref()) {
            (Some(scope), Some(partition))
                if recorder_matches_partition(partition, publication.as_ref()) =>
            {
                if startup_scope.as_deref() == Some(scope)
                    && startup_partition.as_deref() == Some(partition.partition_id.as_str())
                {
                    if boundary_seen {
                        server.store().clear_account_boundary();
                        boundary_seen = false;
                    }
                } else {
                    // systemd Restart=always starts a new process whose static
                    // read-only catalog is built from this admitted account.
                    return Ok(());
                }
            }
            (Some(_), _) => {
                server
                    .store()
                    .publish_account_boundary(PublicState::Initializing);
                boundary_seen = true;
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

fn default_account_readers(codex_home: &Path, data_root: &Path) -> Result<AccountCatalog, String> {
    let current = locate_existing_partition(codex_home, data_root)
        .map_err(|error| format!("locate current account database: {error}"))?;
    let partitions = locate_existing_partitions(codex_home, data_root)
        .map_err(|error| format!("locate account databases: {error}"))?;
    let default_account_id = current.public_id();
    let mut readers = Vec::with_capacity(partitions.len());
    for partition in partitions {
        let identity = partition_identity(&partition);
        let read_intervals = partition_read_intervals(&partition)?;
        let reader = DbReader::open_partitioned_with_intervals(
            &partition.database_path,
            &identity,
            read_intervals,
        )
        .map_err(|error| {
            format!(
                "open account partition {} ({}) read-only: {error}",
                partition.public_id(),
                partition.database_path.display()
            )
        })?;
        let database_login_id = reader.partition_login_id().map_err(|error| {
            format!(
                "read account partition {} display identity: {error}",
                partition.public_id()
            )
        })?;
        readers.push(
            AccountReader::new(
                partition.public_id(),
                partition.storage_epoch,
                partition.account_scope_id == current.account_scope_id,
                partition.current_interval_start,
                partition.current_interval_end,
                reader,
            )
            .with_login_id(database_login_id.or(partition.login_id.clone())),
        );
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
            last_commit_unix,
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
            last_commit_unix,
        }) if partition_id == &partition.partition_id
            && partition
                .current_interval_start
                .is_none_or(|activation| *last_commit_unix >= activation)
    )
}

fn boundary_state_without_catalog(
    codex_home: &Path,
    publication: Option<&RecorderPublication>,
    auth_scope_available: bool,
) -> PublicState {
    let auth_missing = fs::symlink_metadata(codex_home.join("auth.json"))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    if auth_missing && publication == Some(&RecorderPublication::Idle) {
        PublicState::AuthRequired
    } else if auth_scope_available {
        PublicState::Initializing
    } else {
        PublicState::Error
    }
}

fn read_recorder_publication(data_root: &Path) -> Result<RecorderPublication, String> {
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
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "recorder publication JSON is invalid".to_owned())?;
    let object = value
        .as_object()
        .ok_or_else(|| "recorder publication is not an object".to_owned())?;
    let expected = BTreeSet::from([
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
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is invalid".to_owned())?
        .as_secs();
    let now = i64::try_from(now).map_err(|_| "system clock is invalid".to_owned())?;
    let updated_at = object
        .get("updated_at_unix")
        .and_then(Value::as_i64)
        .filter(|value| {
            *value > 0 && *value <= now + 5 && now - *value <= RECORDER_STATE_FRESHNESS_SECS
        })
        .ok_or_else(|| "recorder publication is stale".to_owned())?;
    let _ = updated_at;
    match object.get("write_state").and_then(Value::as_str) {
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
            Ok(RecorderPublication::Idle)
        }
        Some("ready" | "degraded") => {
            let partition_id = object
                .get("partition_id_hash")
                .and_then(Value::as_str)
                .filter(|value| valid_lower_hex(value, 64))
                .ok_or_else(|| "recorder partition identity is invalid".to_owned())?;
            let last_commit_unix = object
                .get("last_commit_unix")
                .and_then(Value::as_i64)
                .filter(|value| {
                    *value > 0 && *value <= now + 5 && now - *value <= RECORDER_STATE_FRESHNESS_SECS
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
                partition_id: partition_id.to_owned(),
                last_commit_unix,
            })
        }
        _ => Err("recorder write state is invalid".to_owned()),
    }
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
    fn recorder_publication_admits_logout_and_only_fresh_current_partition() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-info-rest-account-boundary-{}-{suffix}",
            std::process::id()
        ));
        let codex_home = root.join("codex-home");
        let history = root.join("history");
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
        let write_state = |value: serde_json::Value| {
            fs::write(&path, serde_json::to_vec(&value).expect("state JSON"))
                .expect("recorder state");
            #[cfg(unix)]
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .expect("private recorder state");
        };

        write_state(serde_json::json!({
            "schema": "codex-info-recorder-state-v1",
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
        let idle = read_recorder_publication(&root).expect("fresh idle publication");
        assert_eq!(idle, RecorderPublication::Idle);
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
        let active = read_recorder_publication(&root).expect("fresh active publication");
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

        fs::remove_dir_all(root).expect("account boundary cleanup");
    }
}

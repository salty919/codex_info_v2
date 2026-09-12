use codex_info_account_locator::{
    locate_existing_partition, locate_existing_partitions, AccountPartition,
};
use codex_info_db_reader::{DbReader, ReadInterval, ReadIntervals, StoragePartitionIdentity};
use codex_info_rest::{loopback_addr, AccountReader, RestServer};
use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

const REST_INTERNAL_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    let (accounts, default_account_id) = match requested_database {
        Some(path) if fixture_mode => {
            let database = absolutize(path)?;
            let reader = DbReader::open(&database).map_err(|error| error.to_string())?;
            (
                vec![AccountReader::new("account-1", 1, true, None, None, reader)],
                "account-1".to_owned(),
            )
        }
        Some(_) => unreachable!("non-fixture database path rejected above"),
        None => default_account_readers()?,
    };
    let address = loopback_addr(&port).map_err(|error| error.to_string())?;
    let server = RestServer::start_with_accounts(accounts, &default_account_id, address)
        .map_err(|error| error.to_string())?;
    eprintln!(
        "codex_info_rest: read-only listener={} accounts={} default={}",
        server.local_addr(),
        server.store().account_descriptors().len(),
        default_account_id,
    );
    // systemd owns lifecycle and sends SIGTERM.  No recorder or writer is
    // started here; this binary only owns the read-only REST listener.
    loop {
        thread::sleep(Duration::from_secs(3_600));
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

fn default_account_readers() -> Result<(Vec<AccountReader>, String), String> {
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
    let current = locate_existing_partition(&codex_home, &data_root)
        .map_err(|error| format!("locate current account database: {error}"))?;
    let partitions = locate_existing_partitions(&codex_home, &data_root)
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
    Ok((readers, default_account_id))
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
    use super::{fixture_database_mode, validate_database_override, REST_INTERNAL_VERSION};
    use std::path::Path;

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
}

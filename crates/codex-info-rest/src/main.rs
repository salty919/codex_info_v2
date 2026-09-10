use codex_info_account_locator::locate_existing_partition;
use codex_info_db_reader::DbReader;
use codex_info_rest::{loopback_addr, RestServer};
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
    let database = match requested_database {
        Some(path) if fixture_mode => path,
        Some(_) => unreachable!("non-fixture database path rejected above"),
        None => default_database_path()?,
    };
    let database = absolutize(database)?;
    let reader = DbReader::open(&database).map_err(|error| error.to_string())?;
    let address = loopback_addr(&port).map_err(|error| error.to_string())?;
    let server = RestServer::start(reader, address).map_err(|error| error.to_string())?;
    eprintln!(
        "codex_info_rest: read-only listener={} database={}",
        server.local_addr(),
        database.display()
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

fn default_database_path() -> Result<PathBuf, String> {
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or_else(|| {
            "CODEX_HOME or HOME is required to locate the account database".to_owned()
        })?;
    let data_root = env::var_os("CODEX_INFO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| codex_home.clone());
    locate_existing_partition(&codex_home, &data_root)
        .map(|partition| partition.database_path)
        .map_err(|error| format!("locate existing account database: {error}"))
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

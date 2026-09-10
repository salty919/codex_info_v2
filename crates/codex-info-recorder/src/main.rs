use codex_info_account_locator::{
    ensure_partition, mark_partition_initialized, prepare_recorder_data_root, AccountPartition,
};
use codex_info_db_writer::StoragePartitionIdentity;
use codex_info_recorder::{
    ProfileLease, QuotaPoller, Recorder, RecorderConfig, RecorderStateWriter, DEFAULT_CHUNK_BYTES,
    DEFAULT_INTERVAL_SECS,
};
use std::path::PathBuf;
use std::time::Duration;

const RECORDER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
struct Options {
    codex_home: PathBuf,
    sessions_root: PathBuf,
    data_root: PathBuf,
    database: PathBuf,
    identity: StoragePartitionIdentity,
    partition: AccountPartition,
    chunk_bytes: u64,
    interval_secs: u64,
    once: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.iter().any(|argument| argument == "--version") {
        println!("{RECORDER_VERSION}");
        return Ok(());
    }
    let mut options = Options::parse(arguments)?;
    prepare_recorder_data_root(&options.data_root)
        .map_err(|error| format!("prepare recorder data root: {error}"))?;
    let _profile_lease = ProfileLease::acquire(&options.data_root)?;
    let partition = ensure_partition(&options.codex_home, &options.data_root)
        .map_err(|error| format!("locate or allocate account database: {error}"))?;
    if options.database.as_os_str().is_empty() {
        options.database = partition.database_path.clone();
    } else if options.database != partition.database_path {
        return Err("--database must be the locator-selected account partition".into());
    }
    options.identity = StoragePartitionIdentity {
        schema_version: "codex-info-account-db-v1".to_owned(),
        profile_scope_id: partition.profile_scope_id.clone(),
        account_scope_id: partition.account_scope_id.clone(),
        storage_epoch: partition.storage_epoch,
        partition_id: partition.partition_id.clone(),
    };
    options.partition = partition;
    let mut recorder = Recorder::open_partitioned(
        RecorderConfig {
            sessions_root: options.sessions_root,
            chunk_bytes: options.chunk_bytes,
        },
        &options.database,
        &options.identity,
    )?;
    mark_partition_initialized(&options.data_root, &options.partition)
        .map_err(|error| format!("mark account partition initialized: {error}"))?;
    let mut state_writer =
        RecorderStateWriter::new(&options.data_root, &options.identity, &_profile_lease)?;
    let mut quota_poller = QuotaPoller::start();
    loop {
        match recorder.run_cycle_with_quota(quota_poller.latest()) {
            Ok(Some(report)) => {
                eprintln!(
                    "codex-info-recorder generation={} accepted={} pending={} sources={}",
                    report.generation,
                    report.accepted_ranges,
                    report.pending_ranges,
                    report.sources_seen
                );
                match recorder.state() {
                    Ok(state) => {
                        let result =
                            state_writer.write_committed(&state, report.pending_ranges != 0);
                        if let Err(error) = result {
                            eprintln!("codex-info-recorder degraded state write failed: {error}");
                        }
                    }
                    Err(error) => {
                        eprintln!("codex-info-recorder degraded state read-back failed: {error}")
                    }
                }
            }
            Ok(None) => eprintln!("codex-info-recorder no new session ranges"),
            Err(error) => {
                // A DB or source failure is a degraded cycle. Keep the exact
                // next scheduled attempt independent of the REST process.
                eprintln!("codex-info-recorder degraded cycle: {error}");
                let state = recorder.state().ok();
                if let Err(state_error) = state_writer.write_degraded(state.as_ref()) {
                    eprintln!("codex-info-recorder degraded state write failed: {state_error}");
                }
            }
        }
        if options.once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(options.interval_secs));
    }
}

impl Options {
    fn parse<I>(arguments: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = String>,
    {
        let codex_home = default_codex_home();
        let sessions_root = codex_home.join("sessions");
        let database = std::env::var_os("CODEX_INFO_RECORDER_DB")
            .or_else(|| std::env::var_os("CODEX_INFO_ACCOUNT_DB"))
            .map(PathBuf::from);
        let mut options = Self {
            codex_home: codex_home.clone(),
            sessions_root,
            data_root: PathBuf::new(),
            database: database.unwrap_or_default(),
            identity: StoragePartitionIdentity {
                schema_version: String::new(),
                profile_scope_id: String::new(),
                account_scope_id: String::new(),
                storage_epoch: 0,
                partition_id: String::new(),
            },
            partition: AccountPartition {
                profile_scope_id: String::new(),
                account_scope_id: String::new(),
                storage_epoch: 0,
                partition_id: String::new(),
                database_path: PathBuf::new(),
            },
            chunk_bytes: DEFAULT_CHUNK_BYTES,
            interval_secs: std::env::var("CODEX_INFO_DAEMON_INTERVAL_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_INTERVAL_SECS),
            once: false,
        };
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--once" => options.once = true,
                "--help" | "-h" => {
                    println!(
                        "codex_info_recorder [--once] [--sessions-root PATH] [--database PATH] \
                         [--chunk-bytes N] [--interval-secs N]"
                    );
                    std::process::exit(0);
                }
                "--sessions-root" => options.sessions_root = next_path(&mut arguments, argument)?,
                "--database" => options.database = next_path(&mut arguments, argument)?,
                "--chunk-bytes" => {
                    options.chunk_bytes = next_u64(&mut arguments, argument)?;
                    if options.chunk_bytes == 0 {
                        return Err("--chunk-bytes must be positive".into());
                    }
                }
                "--interval-secs" => {
                    options.interval_secs = next_u64(&mut arguments, argument)?;
                    if options.interval_secs == 0 {
                        return Err("--interval-secs must be positive".into());
                    }
                }
                unknown => return Err(format!("unknown argument: {unknown}")),
            }
        }
        let data_root = std::env::var_os("CODEX_INFO_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| codex_home.clone());
        options.data_root = data_root;
        Ok(options)
    }
}

fn next_path<I>(arguments: &mut I, argument: String) -> Result<PathBuf, String>
where
    I: Iterator<Item = String>,
{
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{argument} requires a path"))
}

fn next_u64<I>(arguments: &mut I, argument: String) -> Result<u64, String>
where
    I: Iterator<Item = String>,
{
    arguments
        .next()
        .ok_or_else(|| format!("{argument} requires a number"))?
        .parse()
        .map_err(|_| format!("{argument} requires an unsigned integer"))
}

fn default_codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_the_recorder_package_version() {
        assert!(!super::RECORDER_VERSION.is_empty());
        assert!(super::RECORDER_VERSION
            .split('.')
            .all(|component| !component.is_empty()));
    }
}

use chrono::Utc;
use codex_info_account_locator::{
    ensure_partition, mark_partition_initialized, prepare_recorder_data_root, AccountPartition,
};
use codex_info_db_writer::{ActiveThreadSnapshot, StoragePartitionIdentity};
use codex_info_recorder::{
    ActiveThreadPollResult, ProfileLease, QuotaPollEvent, QuotaPoller, Recorder, RecorderConfig,
    RecorderStateWriter, ThreadPoller, DEFAULT_CHUNK_BYTES, DEFAULT_INTERVAL_SECS,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LaneHealth {
    Unknown,
    Ready,
    Failed,
}

fn desired_acquisition_degraded(quota: LaneHealth, threads: LaneHealth) -> Option<bool> {
    if quota == LaneHealth::Failed || threads == LaneHealth::Failed {
        Some(true)
    } else if quota == LaneHealth::Ready && threads == LaneHealth::Ready {
        Some(false)
    } else {
        None
    }
}

fn sync_acquisition_health(recorder: &mut Recorder, quota: LaneHealth, threads: LaneHealth) {
    let result = match desired_acquisition_degraded(quota, threads) {
        Some(true) => recorder.mark_active_thread_snapshot_degraded(),
        Some(false) => recorder.clear_active_thread_snapshot_degraded(),
        None => return,
    };
    if let Err(error) = result {
        eprintln!("codex-info-recorder acquisition health commit failed: {error}");
    }
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
            sessions_root: options.sessions_root.clone(),
            chunk_bytes: options.chunk_bytes,
        },
        &options.database,
        &options.identity,
    )?;
    mark_partition_initialized(&options.data_root, &options.partition)
        .map_err(|error| format!("mark account partition initialized: {error}"))?;
    let mut state_writer =
        RecorderStateWriter::new(&options.data_root, &options.identity, &_profile_lease)?;
    let mut quota_poller = QuotaPoller::start_with_interval(options.interval_secs);
    let thread_poller = ThreadPoller::start(options.sessions_root.clone());
    let mut quota_health = LaneHealth::Unknown;
    let mut thread_health = LaneHealth::Unknown;
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
        // Both lanes are deliberately submitted and drained with nonblocking
        // operations. The DB integration consumes these typed outcomes; an
        // app-server timeout or malformed response must not delay the next
        // Session token cycle.
        if let Some(event) = quota_poller.take_events().into_iter().last() {
            match event {
                QuotaPollEvent::Ready(_) => {
                    quota_health = LaneHealth::Ready;
                    eprintln!("codex-info-recorder quota lane ready");
                }
                QuotaPollEvent::Failed => {
                    quota_health = LaneHealth::Failed;
                    eprintln!("codex-info-recorder quota lane degraded");
                }
            }
            sync_acquisition_health(&mut recorder, quota_health, thread_health);
        }
        if let Ok(state) = recorder.state() {
            let _ = thread_poller.submit(&state.checkpoints);
        }
        if let Some(result) = thread_poller.drain().into_iter().last() {
            let snapshot = match result {
                ActiveThreadPollResult::Snapshot(snapshot) => Some(snapshot),
                ActiveThreadPollResult::Empty => Some(ActiveThreadSnapshot {
                    observed_at: Utc::now().timestamp(),
                    threads: Vec::new(),
                }),
                ActiveThreadPollResult::Failed(reason) => {
                    thread_health = LaneHealth::Failed;
                    eprintln!("codex-info-recorder active-thread lane degraded: {reason}");
                    sync_acquisition_health(&mut recorder, quota_health, thread_health);
                    None
                }
            };
            if let Some(snapshot) = snapshot {
                // Unknown quota health must not clear a degraded marker left by
                // a previous process. The snapshot remains readable while the
                // marker records that both acquisition lanes are not yet ready.
                let acquisition_degraded = quota_health != LaneHealth::Ready;
                match recorder.commit_active_thread_snapshot(&snapshot, acquisition_degraded) {
                    Ok(generation) => {
                        thread_health = LaneHealth::Ready;
                        eprintln!(
                            "codex-info-recorder active-thread snapshot rows={} observed_at={} generation={generation}",
                            snapshot.threads.len(),
                            snapshot.observed_at
                        );
                    }
                    Err(error) => {
                        thread_health = LaneHealth::Failed;
                        eprintln!(
                            "codex-info-recorder active-thread snapshot commit failed: {error}"
                        );
                        sync_acquisition_health(&mut recorder, quota_health, thread_health);
                    }
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
    use super::{desired_acquisition_degraded, LaneHealth};

    #[test]
    fn version_is_the_recorder_package_version() {
        assert!(!super::RECORDER_VERSION.is_empty());
        assert!(super::RECORDER_VERSION
            .split('.')
            .all(|component| !component.is_empty()));
    }

    #[test]
    fn acquisition_health_only_recovers_after_both_lanes_are_ready() {
        assert_eq!(
            desired_acquisition_degraded(LaneHealth::Ready, LaneHealth::Ready),
            Some(false)
        );
        assert_eq!(
            desired_acquisition_degraded(LaneHealth::Failed, LaneHealth::Ready),
            Some(true)
        );
        assert_eq!(
            desired_acquisition_degraded(LaneHealth::Ready, LaneHealth::Failed),
            Some(true)
        );
        assert_eq!(
            desired_acquisition_degraded(LaneHealth::Failed, LaneHealth::Failed),
            Some(true)
        );
        assert_eq!(
            desired_acquisition_degraded(LaneHealth::Unknown, LaneHealth::Ready),
            None
        );
        assert_eq!(
            desired_acquisition_degraded(LaneHealth::Ready, LaneHealth::Unknown),
            None
        );
        assert_eq!(
            desired_acquisition_degraded(LaneHealth::Unknown, LaneHealth::Unknown),
            None
        );
    }
}

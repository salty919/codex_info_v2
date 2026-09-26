use chrono::Utc;
use codex_info_account_locator::{
    current_account_scope_id, ensure_partition_with_activation, locate_existing_partition,
    locate_existing_partitions, mark_partition_initialized, prepare_recorder_data_root,
    set_partition_login_id as set_registry_login_id, AccountPartition,
};
use codex_info_db_writer::{
    ActiveThreadSnapshot, SessionCheckpoint, SessionLifecycleInterval, StoragePartitionIdentity,
    UsageStore,
};
use codex_info_recorder::{
    probe_codex_authentication_state, synchronize_inactive_partition, AccountEpochProof,
    ActiveThreadPollResult, CodexAuthenticationState, FixedRateSchedule, ProfileLease,
    QuotaPollEvent, QuotaPoller, Recorder, RecorderConfig, RecorderError, RecorderStateWriter,
    ThreadPoller, DEFAULT_CHUNK_BYTES, DEFAULT_INTERVAL_SECS,
};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
    activation_timestamp: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LaneHealth {
    Unknown,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CyclePublication {
    Committed { has_pending: bool },
    NoCommit,
    Degraded,
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

fn commit_thread_poll_result<EpochCheck>(
    result: ActiveThreadPollResult,
    cycle_epoch: &AccountEpochProof,
    recorder: &mut Recorder,
    quota_health: LaneHealth,
    thread_health: &mut LaneHealth,
    epoch_matches: EpochCheck,
) -> Result<bool, Box<dyn std::error::Error>>
where
    EpochCheck: FnMut(&AccountEpochProof) -> bool,
{
    let mut epoch_matches = epoch_matches;
    if !epoch_matches(cycle_epoch) {
        return Err(RecorderError::AccountBoundaryChanged.into());
    }
    let snapshot = match result {
        ActiveThreadPollResult::Snapshot { snapshot, epoch } => {
            if !epoch_matches(&epoch) {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            Some((snapshot, epoch))
        }
        ActiveThreadPollResult::Empty { epoch } => {
            if !epoch_matches(&epoch) {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            Some((
                ActiveThreadSnapshot {
                    observed_at: Utc::now().timestamp(),
                    threads: Vec::new(),
                },
                epoch,
            ))
        }
        ActiveThreadPollResult::Failed(reason) => {
            *thread_health = LaneHealth::Failed;
            eprintln!("codex-info-recorder active-thread lane degraded: {reason}");
            sync_acquisition_health(recorder, quota_health, *thread_health);
            None
        }
    };
    if let Some((snapshot, epoch)) = snapshot {
        // Unknown quota health must not clear a degraded marker left by a
        // previous process. The snapshot remains readable while the marker
        // records that both acquisition lanes are not yet ready.
        let acquisition_degraded = quota_health != LaneHealth::Ready;
        if !epoch_matches(&epoch) {
            return Err(RecorderError::AccountBoundaryChanged.into());
        }
        match recorder.commit_active_thread_snapshot(&snapshot, acquisition_degraded) {
            Ok(generation) => {
                if !epoch_matches(cycle_epoch) {
                    return Err(RecorderError::AccountBoundaryChanged.into());
                }
                *thread_health = LaneHealth::Ready;
                eprintln!(
                    "codex-info-recorder active-thread snapshot rows={} observed_at={} generation={generation}",
                    snapshot.threads.len(),
                    snapshot.observed_at
                );
                return Ok(true);
            }
            Err(error) => {
                *thread_health = LaneHealth::Failed;
                eprintln!("codex-info-recorder active-thread snapshot commit failed: {error}");
                sync_acquisition_health(recorder, quota_health, *thread_health);
            }
        }
    }
    Ok(false)
}

fn acknowledge_cycle_publication<EpochCheck>(
    publication: CyclePublication,
    cycle_epoch: &AccountEpochProof,
    recorder: &mut Recorder,
    state_writer: &mut RecorderStateWriter,
    quota_health: LaneHealth,
    epoch_matches: EpochCheck,
) -> Result<(), Box<dyn std::error::Error>>
where
    EpochCheck: FnMut(&AccountEpochProof) -> bool,
{
    let mut epoch_matches = epoch_matches;
    if !epoch_matches(cycle_epoch) {
        return Err(RecorderError::AccountBoundaryChanged.into());
    }
    match publication {
        CyclePublication::Committed { .. } if quota_health != LaneHealth::Ready => {
            // A newly admitted account is not public-current until its own
            // app-server quota has been committed. Leaving the prior recorder
            // authority untouched keeps REST in `initializing` instead of
            // exposing the previous account's reset time.
            eprintln!("codex-info-recorder awaiting fresh current-account quota");
        }
        CyclePublication::Committed { has_pending } => match recorder.state() {
            Ok(state) => {
                if !epoch_matches(cycle_epoch) {
                    return Err(RecorderError::AccountBoundaryChanged.into());
                }
                match state_writer.write_committed(&state, has_pending) {
                    Ok(()) => eprintln!(
                        "codex-info-recorder acknowledged generation={} cycle={}",
                        state.data_generation, state.cycle_seq
                    ),
                    Err(error) => {
                        eprintln!("codex-info-recorder degraded state write failed: {error}")
                    }
                }
            }
            Err(error) => {
                eprintln!("codex-info-recorder degraded state read-back failed: {error}");
                if let Err(state_error) = state_writer.write_degraded(None) {
                    eprintln!("codex-info-recorder degraded state write failed: {state_error}");
                }
            }
        },
        CyclePublication::Degraded => {
            let state = recorder.state().ok();
            if !epoch_matches(cycle_epoch) {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            if let Err(state_error) = state_writer.write_degraded(state.as_ref()) {
                eprintln!("codex-info-recorder degraded state write failed: {state_error}");
            }
        }
        CyclePublication::NoCommit => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn complete_cycle_publication_and_wait<Submit, Drain, WaitFor, Sleep, Clock, EpochCheck>(
    publication: CyclePublication,
    options: &Options,
    cycle_epoch: &AccountEpochProof,
    recorder: &mut Recorder,
    state_writer: &mut RecorderStateWriter,
    quota_health: LaneHealth,
    thread_health: &mut LaneHealth,
    schedule: &mut FixedRateSchedule,
    submit: &mut Submit,
    drain: &mut Drain,
    wait_for: &mut WaitFor,
    sleep: &mut Sleep,
    clock: &mut Clock,
    epoch_matches: &mut EpochCheck,
) -> Result<bool, Box<dyn std::error::Error>>
where
    Submit: FnMut(&[SessionCheckpoint]) -> bool,
    Drain: FnMut() -> Vec<ActiveThreadPollResult>,
    WaitFor: FnMut(Duration) -> Option<ActiveThreadPollResult>,
    Sleep: FnMut(Duration),
    Clock: FnMut() -> Instant,
    EpochCheck: FnMut(&AccountEpochProof) -> bool,
{
    if let Ok(state) = recorder.state() {
        let _ = submit(&state.checkpoints);
    }
    if let Some(result) = drain().into_iter().last() {
        commit_thread_poll_result(
            result,
            cycle_epoch,
            recorder,
            quota_health,
            thread_health,
            &mut *epoch_matches,
        )?;
    }
    // Publish the installer-visible acknowledgement only after every DB
    // writer in this cycle has drained. Active-thread and acquisition
    // health commits share collection_generation with Session data, so
    // publishing earlier leaves recorder-state one generation behind the
    // durable SQLite authority for the entire sleep interval.
    acknowledge_cycle_publication(
        publication,
        cycle_epoch,
        recorder,
        state_writer,
        quota_health,
        &mut *epoch_matches,
    )?;
    if options.once {
        return Ok(false);
    }
    let wait = schedule.complete_cycle(clock());
    if wait.missed_deadlines != 0 {
        eprintln!(
            "codex-info-recorder sampling overrun: missed_deadlines={}",
            wait.missed_deadlines
        );
    }
    if !wait.sleep_for.is_zero() {
        let deadline = clock() + wait.sleep_for;
        if let Some(result) = wait_for(wait.sleep_for) {
            let committed = commit_thread_poll_result(
                result,
                cycle_epoch,
                recorder,
                quota_health,
                thread_health,
                &mut *epoch_matches,
            )?;
            if committed {
                acknowledge_cycle_publication(
                    publication,
                    cycle_epoch,
                    recorder,
                    state_writer,
                    quota_health,
                    &mut *epoch_matches,
                )?;
            }
        }
        let remaining = deadline.saturating_duration_since(clock());
        if !remaining.is_zero() {
            sleep(remaining);
        }
    }
    Ok(true)
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
    let previous_authority = RecorderStateWriter::read_previous_authority(&options.data_root)?;
    if AccountEpochProof::capture(&options.codex_home).is_err() {
        let auth_path = options.codex_home.join("auth.json");
        match fs::symlink_metadata(&auth_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return run_without_account(&options, &_profile_lease)
            }
            _ => {
                return Err(RecorderError::Invalid(
                    "Codex account authority is present but invalid".to_owned(),
                )
                .into())
            }
        }
    }
    // A configured activation is useful for deterministic restart fixtures,
    // but it must never become the boundary of a future account switch.  If
    // the currently-authenticated partition is not the registry's open
    // interval (or cannot yet be located), admit the switch at wall-clock
    // time.  Existing-account admission itself remains idempotent in the
    // locator, so a restart ignores either value.
    let requested_activation = match options.activation_timestamp {
        Some(configured)
            if locate_existing_partition(&options.codex_home, &options.data_root)
                .is_ok_and(|partition| partition.current_interval_end.is_none()) =>
        {
            Some(configured)
        }
        Some(_) | None => Some(Utc::now().timestamp()),
    };
    let partition = ensure_partition_with_activation(
        &options.codex_home,
        &options.data_root,
        requested_activation,
    )
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
    let transition_fingerprint = previous_authority
        .as_ref()
        .filter(|previous| {
            previous.partition_id.as_deref() != Some(options.identity.partition_id.as_str())
        })
        .map(|previous| previous.transition_fingerprint.as_str());
    let lifecycle_intervals = options
        .partition
        .lifecycle_intervals
        .iter()
        .map(|interval| SessionLifecycleInterval::new(interval.start_at, interval.end_at))
        .collect();
    let mut recorder = Recorder::open_partitioned_for_account_with_lifecycle(
        RecorderConfig {
            sessions_root: options.sessions_root.clone(),
            chunk_bytes: options.chunk_bytes,
        },
        &options.database,
        &options.identity,
        options.partition.activation_timestamp,
        transition_fingerprint,
        lifecycle_intervals,
    )?;
    let startup_epoch = AccountEpochProof::capture(&options.codex_home)
        .map_err(|_| RecorderError::AccountBoundaryChanged)?;
    let startup_backup = UsageStore::backup_generations_partitioned_verified(
        &options.database,
        &options.identity,
        3,
    )?;
    if recorder.reconcile_session_lifecycle_guarded(&startup_backup, || {
        recorder_epoch_matches(&options, &startup_epoch)
    })? {
        eprintln!("codex-info-recorder repaired session totals across account lifecycles");
    }
    mark_partition_initialized(&options.data_root, &options.partition)
        .map_err(|error| format!("mark account partition initialized: {error}"))?;
    // Existing inactive partitions have no live writer. Bring every
    // initialized partition to the same durable history schema now, while
    // this process owns the profile lease. Login metadata is optional and
    // never decides whether historical data is canonicalized.
    match locate_existing_partitions(&options.codex_home, &options.data_root) {
        Ok(partitions) => {
            for partition in partitions {
                if partition.account_scope_id == options.partition.account_scope_id {
                    continue;
                }
                let identity = StoragePartitionIdentity {
                    schema_version: "codex-info-account-db-v1".to_owned(),
                    profile_scope_id: partition.profile_scope_id.clone(),
                    account_scope_id: partition.account_scope_id.clone(),
                    storage_epoch: partition.storage_epoch,
                    partition_id: partition.partition_id.clone(),
                };
                if let Err(error) = synchronize_inactive_partition(
                    &partition.database_path,
                    &identity,
                    partition.login_id.as_deref(),
                ) {
                    eprintln!(
                        "codex-info-recorder degraded inactive account history synchronization failed: {error}"
                    );
                }
            }
        }
        Err(error) => {
            eprintln!("codex-info-recorder degraded inactive account catalog failed: {error}")
        }
    }
    let mut state_writer =
        RecorderStateWriter::new(&options.data_root, &options.identity, &_profile_lease)?;
    let mut quota_poller = QuotaPoller::start_with_interval(options.interval_secs);
    let thread_poller = ThreadPoller::start(options.sessions_root.clone());
    let mut schedule = FixedRateSchedule::new(Duration::from_secs(options.interval_secs));
    let mut quota_health = LaneHealth::Unknown;
    let mut thread_health = LaneHealth::Unknown;
    loop {
        let cycle_epoch = AccountEpochProof::capture(&options.codex_home)
            .map_err(|_| RecorderError::AccountBoundaryChanged)?;
        if !recorder_epoch_matches(&options, &cycle_epoch) {
            return Err(RecorderError::AccountBoundaryChanged.into());
        }
        let quota_candidate = quota_poller.latest();
        if quota_candidate
            .as_ref()
            .is_some_and(|candidate| !candidate.validate_current(&options.codex_home))
        {
            return Err(RecorderError::AccountBoundaryChanged.into());
        }
        let quota = quota_candidate
            .as_ref()
            .map(|candidate| candidate.snapshot().clone());
        let publication = match recorder.run_cycle_with_quota_guarded(quota, || {
            recorder_epoch_matches(&options, &cycle_epoch)
                && quota_candidate
                    .as_ref()
                    .is_none_or(|candidate| candidate.validate_current(&options.codex_home))
        }) {
            Ok(Some(report)) => {
                eprintln!(
                    "codex-info-recorder generation={} accepted={} pending={} sources={}",
                    report.generation,
                    report.accepted_ranges,
                    report.pending_ranges,
                    report.sources_seen
                );
                CyclePublication::Committed {
                    has_pending: report.pending_ranges != 0,
                }
            }
            Ok(None) => {
                eprintln!("codex-info-recorder no new session ranges");
                CyclePublication::NoCommit
            }
            Err(RecorderError::AccountBoundaryChanged) => {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            Err(error) => {
                // A DB or source failure is a degraded cycle. Keep the exact
                // next scheduled attempt independent of the REST process.
                eprintln!("codex-info-recorder degraded cycle: {error}");
                CyclePublication::Degraded
            }
        };
        // Both lanes are deliberately submitted and drained with nonblocking
        // operations. The DB integration consumes these typed outcomes; an
        // app-server timeout or malformed response must not delay the next
        // Session token cycle.
        if let Some(event) = quota_poller.take_events().into_iter().last() {
            if !recorder_epoch_matches(&options, &cycle_epoch) {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            match event {
                QuotaPollEvent::Ready {
                    login_id, epoch, ..
                } => {
                    if !recorder_epoch_matches(&options, &epoch) {
                        return Err(RecorderError::AccountBoundaryChanged.into());
                    }
                    quota_health = LaneHealth::Ready;
                    // A display-label failure must never stop or delay the
                    // recorder's authoritative Session/DB path.
                    if let Err(error) = recorder.set_partition_login_id(&login_id) {
                        eprintln!(
                            "codex-info-recorder degraded account DB label persistence failed: {error}"
                        );
                    }
                    if !recorder_epoch_matches(&options, &epoch) {
                        return Err(RecorderError::AccountBoundaryChanged.into());
                    }
                    match epoch.run_if_current(&options.codex_home, || {
                        set_registry_login_id(
                            &options.data_root,
                            &options.partition,
                            &login_id,
                        )
                    }) {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => eprintln!(
                            "codex-info-recorder degraded account registry label persistence failed: {error}"
                        ),
                        Err(error) => return Err(error.into()),
                    }
                    eprintln!("codex-info-recorder quota lane ready");
                }
                QuotaPollEvent::Failed => {
                    quota_health = LaneHealth::Failed;
                    eprintln!("codex-info-recorder quota lane degraded");
                }
            }
            if !recorder_epoch_matches(&options, &cycle_epoch) {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            sync_acquisition_health(&mut recorder, quota_health, thread_health);
        }
        let mut submit = |checkpoints: &[SessionCheckpoint]| thread_poller.submit(checkpoints);
        let mut drain = || thread_poller.drain();
        let mut wait_for = |duration: Duration| thread_poller.wait_for(duration);
        let mut sleep = |duration: Duration| std::thread::sleep(duration);
        let mut clock = || Instant::now();
        let mut epoch_matches = |epoch: &AccountEpochProof| recorder_epoch_matches(&options, epoch);
        if !complete_cycle_publication_and_wait(
            publication,
            &options,
            &cycle_epoch,
            &mut recorder,
            &mut state_writer,
            quota_health,
            &mut thread_health,
            &mut schedule,
            &mut submit,
            &mut drain,
            &mut wait_for,
            &mut sleep,
            &mut clock,
            &mut epoch_matches,
        )? {
            return Ok(());
        }
    }
}

fn run_without_account(
    options: &Options,
    profile_lease: &ProfileLease,
) -> Result<(), Box<dyn std::error::Error>> {
    match probe_codex_authentication_state()
        .map_err(|error| RecorderError::Invalid(format!("confirm Codex logout: {error}")))?
    {
        CodexAuthenticationState::AuthRequired => {}
        CodexAuthenticationState::Authenticated => {
            return Err(RecorderError::Invalid(
                "Codex is authenticated but the local account authority is unavailable".to_owned(),
            )
            .into())
        }
    }
    let mut state_writer = RecorderStateWriter::new_idle(&options.data_root, profile_lease)?;
    state_writer.write_idle_no_account()?;
    if options.once {
        return Ok(());
    }
    let heartbeat = Duration::from_secs(options.interval_secs.clamp(1, 5));
    loop {
        std::thread::sleep(heartbeat);
        let auth_path = options.codex_home.join("auth.json");
        match fs::symlink_metadata(&auth_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                state_writer.write_idle_no_account()?;
            }
            Ok(_) if AccountEpochProof::capture(&options.codex_home).is_ok() => {
                return Err(RecorderError::AccountBoundaryChanged.into())
            }
            _ => {
                return Err(RecorderError::Invalid(
                    "Codex account authority appeared but is invalid".to_owned(),
                )
                .into())
            }
        }
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
                login_id: None,
                activation_timestamp: None,
                lifecycle_intervals: Vec::new(),
                current_interval_start: None,
                current_interval_end: None,
            },
            chunk_bytes: DEFAULT_CHUNK_BYTES,
            interval_secs: std::env::var("CODEX_INFO_DAEMON_INTERVAL_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_INTERVAL_SECS),
            once: false,
            activation_timestamp: std::env::var("CODEX_INFO_ACCOUNT_ACTIVATION_UNIX")
                .ok()
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|value| *value > 0),
        };
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--once" => options.once = true,
                "--help" | "-h" => {
                    println!(
                        "codex_info_recorder [--once] [--sessions-root PATH] [--database PATH] \
                         [--chunk-bytes N] [--interval-secs N] \
                         [--account-activation-unix N]"
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
                "--account-activation-unix" => {
                    let value = next_u64(&mut arguments, argument)?;
                    options.activation_timestamp = Some(
                        i64::try_from(value)
                            .ok()
                            .filter(|value| *value > 0)
                            .ok_or_else(|| {
                                "--account-activation-unix must be a positive Unix second"
                                    .to_owned()
                            })?,
                    );
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

fn account_authority_matches(options: &Options) -> bool {
    current_account_scope_id(&options.codex_home, &options.data_root)
        .is_ok_and(|scope| scope == options.partition.account_scope_id)
}

fn recorder_epoch_matches(options: &Options, epoch: &AccountEpochProof) -> bool {
    epoch.validate_current(&options.codex_home) && account_authority_matches(options)
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
    use super::*;
    use codex_info_db_writer::ActiveThreadRecord;
    use serde_json::Value;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;

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

    struct Issue362AsyncFixture {
        options: Options,
        epoch: AccountEpochProof,
        recorder: Recorder,
        state_writer: RecorderStateWriter,
        _lease: ProfileLease,
        _temp_dir: tempfile::TempDir,
    }

    impl Issue362AsyncFixture {
        fn state_path(&self) -> PathBuf {
            self.options.data_root.join("history/recorder-state.json")
        }

        fn state_bytes(&self) -> Vec<u8> {
            fs::read(self.state_path()).expect("recorder state")
        }

        fn state_value(&self) -> Value {
            serde_json::from_slice(&self.state_bytes()).expect("recorder state JSON")
        }

        fn seed_state(&mut self, has_pending: bool, acquisition_degraded: bool) -> Vec<u8> {
            let snapshot = issue_362_snapshot("seed", 1);
            self.recorder
                .commit_active_thread_snapshot(&snapshot, acquisition_degraded)
                .expect("seed active-thread snapshot");
            let state = self.recorder.state().expect("seed recorder state");
            self.state_writer
                .write_committed(&state, has_pending)
                .expect("seed recorder publication");
            self.state_bytes()
        }

        fn poll_result(&self, title: &str, observed_at: i64) -> ActiveThreadPollResult {
            ActiveThreadPollResult::Snapshot {
                snapshot: issue_362_snapshot(title, observed_at),
                epoch: self.epoch.clone(),
            }
        }

        fn cleanup(self) {
            let Self {
                recorder,
                state_writer,
                _lease,
                _temp_dir,
                ..
            } = self;
            drop(recorder);
            drop(state_writer);
            drop(_lease);
            _temp_dir.close().expect("Issue 362 fixture cleanup");
        }
    }

    fn issue_362_fixture(name: &str) -> Issue362AsyncFixture {
        let temp_dir = tempfile::Builder::new()
            .prefix(&format!("codex-info-recorder-{name}-"))
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("Issue 362 fixture directory");
        let root = temp_dir.path();
        let codex_home = root.join("codex-home");
        let sessions_root = root.join("sessions");
        let data_root = root.join("data");
        fs::create_dir_all(&codex_home).expect("Codex home");
        fs::create_dir_all(&sessions_root).expect("sessions root");
        fs::create_dir_all(&data_root).expect("data root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&codex_home, fs::Permissions::from_mode(0o700))
                .expect("Codex home permissions");
        }
        let auth_path = codex_home.join("auth.json");
        fs::write(
            &auth_path,
            br#"{"tokens":{"account_id":"issue-362-account","access_token":"secret"}}"#,
        )
        .expect("account authority");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&auth_path, fs::Permissions::from_mode(0o600))
                .expect("account authority permissions");
        }

        prepare_recorder_data_root(&data_root).expect("recorder data root");
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".to_owned(),
            profile_scope_id: "11".repeat(16),
            account_scope_id: "22".repeat(32),
            storage_epoch: 1,
            partition_id: "33".repeat(32),
        };
        let database = data_root.join("usage.sqlite3");
        UsageStore::create_partitioned(&database, &identity).expect("partition database");
        let partition = AccountPartition {
            profile_scope_id: identity.profile_scope_id.clone(),
            account_scope_id: identity.account_scope_id.clone(),
            storage_epoch: identity.storage_epoch,
            partition_id: identity.partition_id.clone(),
            database_path: database.clone(),
            login_id: None,
            activation_timestamp: None,
            lifecycle_intervals: Vec::new(),
            current_interval_start: None,
            current_interval_end: None,
        };
        let options = Options {
            codex_home: codex_home.clone(),
            sessions_root: sessions_root.clone(),
            data_root: data_root.clone(),
            database: database.clone(),
            identity: identity.clone(),
            partition,
            chunk_bytes: 4096,
            interval_secs: 60,
            once: false,
            activation_timestamp: None,
        };
        let lease = ProfileLease::acquire(&data_root).expect("profile lease");
        let epoch = AccountEpochProof::capture(&codex_home).expect("account epoch");
        let recorder = Recorder::open_partitioned(
            RecorderConfig {
                sessions_root: sessions_root.clone(),
                chunk_bytes: 4096,
            },
            &database,
            &identity,
        )
        .expect("partitioned recorder");
        let state_writer =
            RecorderStateWriter::new(&data_root, &identity, &lease).expect("state writer");
        Issue362AsyncFixture {
            options,
            epoch,
            recorder,
            state_writer,
            _lease: lease,
            _temp_dir: temp_dir,
        }
    }

    fn issue_362_snapshot(title: &str, observed_at: i64) -> ActiveThreadSnapshot {
        ActiveThreadSnapshot {
            observed_at,
            threads: vec![ActiveThreadRecord {
                id: "issue-362-thread".to_owned(),
                updated_at: observed_at,
                title: title.to_owned(),
                parent_thread_id: None,
                model: "gpt-5".to_owned(),
                model_label: "gpt-5".to_owned(),
                total_tokens: Some(1),
                context_usage_tokens: None,
                context_window_tokens: None,
                created_at: Some(observed_at),
                last_user_message_at: Some(observed_at),
                is_subagent: false,
                depth: Some(0),
            }],
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn issue_362_drive_cycle(
        fixture: &mut Issue362AsyncFixture,
        publication: CyclePublication,
        quota_health: LaneHealth,
        schedule: &mut FixedRateSchedule,
        now: &Cell<Instant>,
        wait_result: Option<ActiveThreadPollResult>,
        epoch_values: Vec<bool>,
        pre_sleep_observation: Option<&Cell<Option<(Instant, Value)>>>,
        submit_calls: &mut usize,
        drain_calls: &mut usize,
        wait_calls: &mut usize,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let mut submit = |_checkpoints: &[SessionCheckpoint]| {
            *submit_calls += 1;
            true
        };
        let mut drain = || {
            *drain_calls += 1;
            Vec::new()
        };
        let mut pending_result = wait_result;
        let mut wait_for = |duration: Duration| {
            assert_eq!(duration, Duration::from_secs(60));
            *wait_calls += 1;
            now.set(now.get() + Duration::from_secs(1));
            pending_result.take()
        };
        let state_path = fixture.state_path();
        let mut sleep = |duration: Duration| {
            if let Some(observation) = pre_sleep_observation {
                let state: Value =
                    serde_json::from_slice(&fs::read(&state_path).expect("pre-sleep state"))
                        .expect("pre-sleep state JSON");
                observation.set(Some((now.get(), state)));
            }
            now.set(now.get() + duration);
        };
        let mut clock = || now.get();
        let mut epoch_values = epoch_values.into_iter();
        let mut epoch_matches =
            move |_epoch: &AccountEpochProof| epoch_values.next().unwrap_or(true);
        let mut thread_health = LaneHealth::Unknown;
        complete_cycle_publication_and_wait(
            publication,
            &fixture.options,
            &fixture.epoch,
            &mut fixture.recorder,
            &mut fixture.state_writer,
            quota_health,
            &mut thread_health,
            schedule,
            &mut submit,
            &mut drain,
            &mut wait_for,
            &mut sleep,
            &mut clock,
            &mut epoch_matches,
        )
    }

    #[test]
    fn issue_362_async_ack_preserves_cycle_publication_state() {
        let cases = [
            (
                "ready",
                CyclePublication::Committed { has_pending: false },
                LaneHealth::Ready,
                false,
                false,
                "ready",
            ),
            (
                "pending",
                CyclePublication::Committed { has_pending: true },
                LaneHealth::Ready,
                true,
                false,
                "degraded",
            ),
            (
                "quota-pending",
                CyclePublication::Committed { has_pending: false },
                LaneHealth::Unknown,
                true,
                true,
                "degraded",
            ),
            (
                "degraded",
                CyclePublication::Degraded,
                LaneHealth::Ready,
                true,
                true,
                "degraded",
            ),
            (
                "no-commit",
                CyclePublication::NoCommit,
                LaneHealth::Ready,
                true,
                false,
                "degraded",
            ),
        ];
        for (name, publication, quota_health, has_pending, acquisition_degraded, expected) in cases
        {
            let mut fixture = issue_362_fixture(name);
            fixture.seed_state(has_pending, acquisition_degraded);
            let anchor = Instant::now();
            let now = Cell::new(anchor);
            let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
            let mut submit_calls = 0;
            let mut drain_calls = 0;
            let mut wait_calls = 0;
            let wait_result = fixture.poll_result("async", 2);
            let result = issue_362_drive_cycle(
                &mut fixture,
                publication,
                quota_health,
                &mut schedule,
                &now,
                Some(wait_result),
                vec![true; 8],
                None,
                &mut submit_calls,
                &mut drain_calls,
                &mut wait_calls,
            )
            .expect("shared cycle path");
            assert!(result);
            assert_eq!(fixture.state_value()["write_state"], expected);
            assert_eq!(now.get(), anchor + Duration::from_secs(60));
            assert_eq!(submit_calls, 1);
            assert_eq!(drain_calls, 1);
            assert_eq!(wait_calls, 1);
            fixture.cleanup();
        }
    }

    #[test]
    fn issue_362_async_result_commits_before_next_scheduled_cycle() {
        let mut fixture = issue_362_fixture("commit-before-next-cycle");
        let root_metadata = fs::symlink_metadata(fixture._temp_dir.path())
            .expect("Issue 362 fixture root metadata");
        assert_eq!(root_metadata.permissions().mode() & 0o777, 0o700);
        fixture.seed_state(false, false);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let pre_sleep_observation = Cell::new(None);
        let mut submit_calls = 0;
        let mut drain_calls = 0;
        let mut wait_calls = 0;
        let wait_result = fixture.poll_result("async", 2);
        assert!(issue_362_drive_cycle(
            &mut fixture,
            CyclePublication::Committed { has_pending: false },
            LaneHealth::Ready,
            &mut schedule,
            &now,
            Some(wait_result),
            vec![true; 8],
            Some(&pre_sleep_observation),
            &mut submit_calls,
            &mut drain_calls,
            &mut wait_calls,
        )
        .expect("shared cycle path"));
        let (before_sleep, sleep_state) = pre_sleep_observation
            .take()
            .expect("publication before fake sleep");
        assert_eq!(before_sleep, anchor + Duration::from_secs(1));
        assert_eq!(sleep_state["write_state"], "ready");
        assert_eq!(sleep_state["data_generation"], 2);
        assert_eq!(
            fixture
                .recorder
                .state()
                .expect("committed state")
                .data_generation,
            2
        );
        assert_eq!(fixture.state_value()["write_state"], "ready");
        assert_eq!(now.get(), anchor + Duration::from_secs(60));
        assert_eq!(
            schedule.complete_cycle(now.get()).sleep_for,
            Duration::from_secs(60)
        );
        assert_eq!(submit_calls, 1);
        assert_eq!(drain_calls, 1);
        assert_eq!(wait_calls, 1);
        fixture.cleanup();
    }

    #[test]
    fn issue_362_async_result_does_not_add_collection_or_probe() {
        let mut fixture = issue_362_fixture("no-extra-collection-or-probe");
        fixture.seed_state(false, false);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut collection_calls = 0;
        let mut submit_calls = 0;
        let mut drain_calls = 0;
        let mut wait_calls = 0;
        let first_wait_result = fixture.poll_result("async", 2);

        collection_calls += 1;
        fixture.recorder.run_cycle().expect("first collection");
        assert!(issue_362_drive_cycle(
            &mut fixture,
            CyclePublication::NoCommit,
            LaneHealth::Ready,
            &mut schedule,
            &now,
            Some(first_wait_result),
            vec![true; 8],
            None,
            &mut submit_calls,
            &mut drain_calls,
            &mut wait_calls,
        )
        .expect("first shared cycle path"));

        collection_calls += 1;
        fixture.recorder.run_cycle().expect("second collection");
        assert!(issue_362_drive_cycle(
            &mut fixture,
            CyclePublication::NoCommit,
            LaneHealth::Ready,
            &mut schedule,
            &now,
            None,
            vec![true; 8],
            None,
            &mut submit_calls,
            &mut drain_calls,
            &mut wait_calls,
        )
        .expect("second shared cycle path"));

        assert_eq!(collection_calls, 2);
        assert_eq!(submit_calls, 2);
        assert_eq!(drain_calls, 2);
        assert_eq!(wait_calls, 2);
        assert_eq!(now.get(), anchor + Duration::from_secs(120));
        fixture.cleanup();
    }

    #[test]
    fn issue_362_async_result_rejects_changed_account_epoch() {
        let mut before_commit = issue_362_fixture("epoch-before-commit");
        let before_state = before_commit.seed_state(false, false);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut submit_calls = 0;
        let mut drain_calls = 0;
        let mut wait_calls = 0;
        let before_commit_result = before_commit.poll_result("async", 2);
        let error = issue_362_drive_cycle(
            &mut before_commit,
            CyclePublication::NoCommit,
            LaneHealth::Ready,
            &mut schedule,
            &now,
            Some(before_commit_result),
            vec![true, true, false],
            None,
            &mut submit_calls,
            &mut drain_calls,
            &mut wait_calls,
        )
        .expect_err("changed epoch before commit");
        assert_eq!(
            before_commit
                .recorder
                .state()
                .expect("prior state")
                .data_generation,
            1
        );
        assert_eq!(before_commit.state_bytes(), before_state);
        assert!(matches!(
            error.downcast_ref::<RecorderError>(),
            Some(RecorderError::AccountBoundaryChanged)
        ));
        before_commit.cleanup();

        let mut after_commit = issue_362_fixture("epoch-after-commit");
        let before_state = after_commit.seed_state(false, false);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut submit_calls = 0;
        let mut drain_calls = 0;
        let mut wait_calls = 0;
        let after_commit_result = after_commit.poll_result("async", 2);
        let error = issue_362_drive_cycle(
            &mut after_commit,
            CyclePublication::NoCommit,
            LaneHealth::Ready,
            &mut schedule,
            &now,
            Some(after_commit_result),
            vec![true, true, true, true, false],
            None,
            &mut submit_calls,
            &mut drain_calls,
            &mut wait_calls,
        )
        .expect_err("changed epoch before acknowledgement");
        assert_eq!(
            after_commit
                .recorder
                .state()
                .expect("committed DB state")
                .data_generation,
            2
        );
        assert_eq!(after_commit.state_bytes(), before_state);
        assert!(matches!(
            error.downcast_ref::<RecorderError>(),
            Some(RecorderError::AccountBoundaryChanged)
        ));
        after_commit.cleanup();
    }
}

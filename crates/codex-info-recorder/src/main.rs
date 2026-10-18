use chrono::Utc;
use codex_info_account_locator::{
    admit_partition, close_partition_ownership_at, current_account_scope_id,
    locate_existing_partitions, mark_partition_initialized, partition_requires_confirmation,
    prepare_recorder_data_root, reserve_partition_at,
    set_partition_login_id as set_registry_login_id, AccountConfirmation, AccountPartition,
};
use codex_info_db_writer::{
    ActiveThreadSnapshot, SessionCheckpoint, SessionLifecycleInterval, StoragePartitionIdentity,
    UsageStore,
};
use codex_info_recorder::{
    confirm_codex_account, probe_codex_authentication_state, synchronize_inactive_partition,
    AccountEpochProof, ActiveThreadPollResult, BackgroundWork, CodexAuthenticationState,
    FixedRateSchedule, PreparedHistoricalRepair, ProfileLease, QuotaPollEvent, QuotaPoller,
    Recorder, RecorderConfig, RecorderError, RecorderStateWriter, ThreadPoller,
    DEFAULT_CHUNK_BYTES, DEFAULT_INTERVAL_SECS,
};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const RECORDER_VERSION: &str = env!("CARGO_PKG_VERSION");
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Acquire)
}

fn shutdown_sleep(duration: Duration) {
    let deadline = Instant::now() + duration;
    while !shutdown_requested() {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        std::thread::park_timeout(remaining);
    }
}

fn run_inactive_history<I, S, F, E>(
    recollection: bool,
    mut stopped: S,
    jobs: I,
    mut run: F,
) -> Result<bool, E>
where
    I: IntoIterator,
    S: FnMut() -> bool,
    F: FnMut(I::Item) -> Result<(), E>,
{
    if stopped() {
        return Ok(false);
    }
    if recollection {
        return Ok(true);
    }
    for job in jobs {
        if stopped() {
            return Ok(false);
        }
        run(job)?;
        if stopped() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn install_shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut terminate = {
            let _entered = runtime.enter();
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
        };
        let owner = std::thread::current();
        std::thread::Builder::new()
            .name("recorder-stop".into())
            .spawn(move || {
                runtime.block_on(async move {
                    if terminate.recv().await.is_some() {
                        SHUTDOWN_REQUESTED.store(true, Ordering::Release);
                        eprintln!("codex-info-recorder diagnostic shutdown requested");
                        owner.unpark();
                    }
                });
            })?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
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

fn start_inactive_history(
    options: Options,
    epoch: AccountEpochProof,
    lease: Arc<ProfileLease>,
) -> Result<BackgroundWork<Result<(), RecorderError>>, RecorderError> {
    let mut recorders = None;
    BackgroundWork::start("codex-info-inactive-history", move || {
        if shutdown_requested() {
            return Ok(());
        }
        if !recorder_epoch_matches(&options, &epoch, &lease) {
            return Err(RecorderError::AccountBoundaryChanged);
        }
        if recorders.is_none() {
            let partitions = locate_existing_partitions(&options.codex_home, &options.data_root)
                .map_err(|error| {
                    RecorderError::Invalid(format!("inactive catalog unavailable: {error}"))
                })?;
            let mut opened = Vec::new();
            for partition in partitions {
                if shutdown_requested() {
                    return Ok(());
                }
                if partition.account_scope_id == options.partition.account_scope_id {
                    continue;
                }
                let identity = StoragePartitionIdentity {
                    schema_version: "codex-info-account-db-v1".into(),
                    profile_scope_id: partition.profile_scope_id.clone(),
                    account_scope_id: partition.account_scope_id.clone(),
                    storage_epoch: partition.storage_epoch,
                    partition_id: partition.partition_id.clone(),
                };
                match synchronize_inactive_partition(
                    &partition.database_path,
                    &identity,
                    partition.login_id.as_deref(),
                )
                .and_then(|()| {
                    Recorder::open_partitioned_for_account_with_lifecycle(
                        RecorderConfig {
                            sessions_root: options.sessions_root.clone(),
                            chunk_bytes: options.chunk_bytes,
                        },
                        &partition.database_path,
                        &identity,
                        partition.activation_timestamp,
                        None,
                        partition
                            .lifecycle_intervals
                            .iter()
                            .map(|interval| {
                                SessionLifecycleInterval::new(interval.start_at, interval.end_at)
                            })
                            .collect(),
                    )
                }) {
                    Ok(recorder) => opened.push((partition.account_scope_id, recorder)),
                    Err(error) => eprintln!(
                        "codex-info-recorder inactive history initialization unavailable: {error}"
                    ),
                }
            }
            recorders = Some(opened);
        }
        run_inactive_history(
            false,
            shutdown_requested,
            recorders.as_mut().expect("initialized"),
            |(scope, historical)| {
                let guard = || recorder_epoch_matches(&options, &epoch, &lease);
                let result = historical
                    .replay_quota_journal(&options.data_root, scope, guard)
                    .and_then(|_| {
                        if shutdown_requested() {
                            Ok(None)
                        } else {
                            historical.repair_historical_sources_guarded(guard)
                        }
                    });
                match result {
                    Ok(Some(generation)) => {
                        eprintln!("codex-info-recorder inactive historical generation={generation}")
                    }
                    Ok(None) => {}
                    Err(RecorderError::AccountBoundaryChanged) => {
                        return Err(RecorderError::AccountBoundaryChanged)
                    }
                    Err(error) => eprintln!(
                        "codex-info-recorder inactive accepted-prefix repair unavailable: {error}"
                    ),
                }
                Ok(())
            },
        )?;
        Ok(())
    })
}

type CurrentHistoryWorker = BackgroundWork<Result<Option<PreparedHistoricalRepair>, RecorderError>>;

fn service_current_history<F>(
    current_history: &mut Option<CurrentHistoryWorker>,
    recorder: &mut Recorder,
    state_writer: &mut RecorderStateWriter,
    mut epoch_matches: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut() -> bool,
{
    let Some(worker) = current_history.as_mut() else {
        return Ok(());
    };
    let completed = worker.try_result();
    if worker.is_unavailable() {
        eprintln!("codex-info-recorder historical preparation worker disconnected; recording continues independently");
        *current_history = None;
        return Ok(());
    }
    let result = match completed {
        Some(Ok(Some(prepared))) => {
            let began = Instant::now();
            eprintln!("codex-info-recorder diagnostic history_commit start");
            let result = recorder.commit_prepared_historical_repair(prepared, &mut epoch_matches);
            eprintln!(
                "codex-info-recorder diagnostic history_commit finish elapsed_ms={} success={}",
                began.elapsed().as_millis(),
                result.is_ok()
            );
            result
        }
        Some(Err(error)) => Err(error),
        Some(Ok(None)) | None => Ok(None),
    };
    match result {
        Ok(Some(generation)) => {
            if !epoch_matches() {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            state_writer.write_committed(&recorder.state()?, true)?;
            eprintln!("codex-info-recorder current accepted-prefix generation={generation}");
        }
        Ok(None) => {}
        Err(RecorderError::AccountBoundaryChanged) => {
            return Err(RecorderError::AccountBoundaryChanged.into())
        }
        Err(error) => {
            eprintln!("codex-info-recorder current accepted-prefix repair unavailable: {error}")
        }
    }
    if worker.request() {
        eprintln!("codex-info-recorder diagnostic history_prepare requested");
    }
    Ok(())
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

const ACTIVE_THREAD_INTERVAL: Duration = Duration::from_secs(5);
const ACTIVE_THREAD_STABLE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Default)]
struct CheckpointRecovery {
    epoch: Option<AccountEpochProof>,
    attempted: bool,
    pending: bool,
}

impl CheckpointRecovery {
    fn request(&mut self, epoch: &AccountEpochProof) -> bool {
        if self.epoch.as_ref() != Some(epoch) {
            *self = Self {
                epoch: Some(epoch.clone()),
                ..Self::default()
            };
        }
        if self.attempted {
            return false;
        }
        self.attempted = true;
        self.pending = true;
        true
    }

    fn begin(&mut self, epoch: &AccountEpochProof) -> bool {
        if self.epoch.as_ref() != Some(epoch) {
            *self = Self::default();
            return false;
        }
        std::mem::take(&mut self.pending)
    }
}

#[derive(Default)]
struct AdaptiveThreadCadence {
    epoch: Option<AccountEpochProof>,
    totals: BTreeMap<String, Option<u64>>,
    last_delta: Option<Instant>,
    next_probe: Option<Instant>,
    in_flight: bool,
    checkpoint_recovery: CheckpointRecovery,
}

impl AdaptiveThreadCadence {
    // Only the complete, epoch-validated snapshot committed to the recorder
    // reaches this method. Unknown totals establish a fresh comparison
    // baseline; they neither mean zero nor prove unchanged usage.
    fn accept(&mut self, epoch: &AccountEpochProof, snapshot: &ActiveThreadSnapshot, now: Instant) {
        self.in_flight = false;
        self.checkpoint_recovery = CheckpointRecovery::default();
        let totals: BTreeMap<_, _> = snapshot
            .threads
            .iter()
            .map(|thread| (thread.id.clone(), thread.total_tokens))
            .collect();
        if self.epoch.as_ref() != Some(epoch) {
            self.last_delta = None;
            self.next_probe = None;
        } else {
            let changed = totals.iter().any(|(id, current)| {
                matches!((self.totals.get(id), current), (Some(Some(previous)), Some(current)) if previous != current)
            });
            if changed {
                if self.last_delta.is_none() {
                    self.next_probe = Some(now + ACTIVE_THREAD_INTERVAL);
                }
                self.last_delta = Some(now);
            } else if self.last_delta.is_some_and(|last_delta| {
                now.saturating_duration_since(last_delta) >= ACTIVE_THREAD_STABLE_WINDOW
            }) && self.totals == totals
                && (totals.is_empty() || totals.values().any(Option::is_some))
            {
                self.last_delta = None;
                self.next_probe = None;
            }
        }
        self.epoch = Some(epoch.clone());
        self.totals = totals;
    }

    fn failed(&mut self) {
        self.in_flight = false;
    }

    fn begin_checkpoint_recovery(&mut self, epoch: &AccountEpochProof) -> bool {
        if !self.checkpoint_recovery.begin(epoch) {
            return false;
        }
        // Re-probe the newly admitted inventory even if an adaptive deadline
        // was scheduled before the reconciliation request.
        self.next_probe = None;
        true
    }

    fn submit<Submit>(
        &mut self,
        now: Instant,
        checkpoints: &[SessionCheckpoint],
        submit: &mut Submit,
    ) -> bool
    where
        Submit: FnMut(&[SessionCheckpoint]) -> bool,
    {
        if self.in_flight || self.next_probe.is_some_and(|deadline| now < deadline) {
            return false;
        }
        // Advance from this attempt instead of catching up missed slots. A
        // slow worker is allowed to finish before another probe starts.
        if self.last_delta.is_some() {
            self.next_probe = Some(now + ACTIVE_THREAD_INTERVAL);
        }
        self.in_flight = submit(checkpoints);
        self.in_flight
    }
}

fn sync_quota_poll_health(recorder: &mut Recorder, quota: LaneHealth) {
    if quota != LaneHealth::Unknown {
        if let Err(error) = recorder.set_quota_acquisition_degraded(quota == LaneHealth::Failed) {
            eprintln!("codex-info-recorder quota health commit failed: {error}");
        }
    }
}

fn sync_acquisition_health(recorder: &mut Recorder, _quota: LaneHealth, threads: LaneHealth) {
    // A thread failure and a quota failure have independent durable owners.
    // Duplicating a quota failure here would keep it latched after replay.
    let result = match threads {
        LaneHealth::Failed => recorder.mark_active_thread_snapshot_degraded(),
        LaneHealth::Ready => recorder.clear_active_thread_snapshot_degraded(),
        LaneHealth::Unknown => return,
    };
    if let Err(error) = result {
        eprintln!("codex-info-recorder acquisition health commit failed: {error}");
    }
}

fn reject_checkpoint_recollection(
    recorder: &mut Recorder,
    quota_health: LaneHealth,
    thread_health: &mut LaneHealth,
    cadence: &mut AdaptiveThreadCadence,
    next_collection: Instant,
) {
    *thread_health = LaneHealth::Failed;
    sync_acquisition_health(recorder, quota_health, *thread_health);
    // A failed collection did not establish trusted checkpoints. Do not let
    // a probe of the old inventory clear the failure before the normal cycle.
    cadence.next_probe = Some(next_collection);
}

#[allow(clippy::too_many_arguments)]
fn commit_thread_poll_result<EpochCheck>(
    result: ActiveThreadPollResult,
    cycle_epoch: &AccountEpochProof,
    recorder: &mut Recorder,
    quota_health: LaneHealth,
    thread_health: &mut LaneHealth,
    cadence: &mut AdaptiveThreadCadence,
    now: Instant,
    epoch_matches: EpochCheck,
) -> Result<bool, Box<dyn std::error::Error>>
where
    EpochCheck: FnMut(&AccountEpochProof) -> bool,
{
    cadence.failed();
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
        ActiveThreadPollResult::CheckpointMismatch { sources } => {
            if !sources.is_empty() {
                let state = recorder.state()?;
                if !epoch_matches(cycle_epoch) {
                    return Err(RecorderError::AccountBoundaryChanged.into());
                }
                if sources.iter().all(|source| {
                    state.checkpoints.iter().any(|checkpoint| {
                        checkpoint.root_identity == source.root_identity
                            && checkpoint.relative_path == source.relative_path
                            && checkpoint.file_device == source.file_device
                            && checkpoint.file_inode == source.file_inode
                    })
                }) {
                    // The normal writer admitted these sources while the
                    // asynchronous probe still held its older inventory.
                    // Keep last-good health until a complete probe succeeds.
                    return Ok(false);
                }
            }
            recorder.prioritize_active_thread_sources(&sources);
            if cadence.checkpoint_recovery.request(cycle_epoch) {
                // No new candidate has been admitted yet. Preserve both the
                // last complete snapshot and its health while the owner runs
                // one guarded collection, before classifying this as failure.
                eprintln!("codex-info-recorder active-thread checkpoint reconciliation requested");
            } else {
                *thread_health = LaneHealth::Failed;
                eprintln!("codex-info-recorder active-thread lane degraded: active session checkpoint mismatch after reconciliation");
                sync_acquisition_health(recorder, quota_health, *thread_health);
            }
            None
        }
        ActiveThreadPollResult::Failed(reason) => {
            *thread_health = LaneHealth::Failed;
            eprintln!("codex-info-recorder active-thread lane degraded: {reason}");
            sync_acquisition_health(recorder, quota_health, *thread_health);
            None
        }
    };
    if let Some((snapshot, epoch)) = snapshot {
        // A same-epoch Thread observation is independent of quota acquisition.
        // Commit its facts and health even when quota has not completed yet.
        if !epoch_matches(&epoch) {
            return Err(RecorderError::AccountBoundaryChanged.into());
        }
        match recorder.commit_active_thread_snapshot(&snapshot, false) {
            Ok(generation) => {
                if !epoch_matches(cycle_epoch) {
                    return Err(RecorderError::AccountBoundaryChanged.into());
                }
                *thread_health = LaneHealth::Ready;
                cadence.accept(&epoch, &snapshot, now);
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
        CyclePublication::Committed { has_pending } => match recorder.state() {
            Ok(state) => {
                if !epoch_matches(cycle_epoch) {
                    return Err(RecorderError::AccountBoundaryChanged.into());
                }
                match state_writer
                    .write_committed(&state, has_pending || quota_health != LaneHealth::Ready)
                {
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
fn complete_cycle_publication_and_wait<
    Submit,
    Drain,
    WaitFor,
    Sleep,
    Clock,
    EpochCheck,
    AfterPublication,
>(
    publication: CyclePublication,
    options: &Options,
    cycle_epoch: &AccountEpochProof,
    recorder: &mut Recorder,
    state_writer: &mut RecorderStateWriter,
    quota_health: LaneHealth,
    thread_health: &mut LaneHealth,
    schedule: &mut FixedRateSchedule,
    cadence: &mut AdaptiveThreadCadence,
    submit: &mut Submit,
    drain: &mut Drain,
    wait_for: &mut WaitFor,
    sleep: &mut Sleep,
    clock: &mut Clock,
    epoch_matches: &mut EpochCheck,
    after_publication: &mut AfterPublication,
) -> Result<bool, Box<dyn std::error::Error>>
where
    Submit: FnMut(&[SessionCheckpoint]) -> bool,
    Drain: FnMut() -> Vec<ActiveThreadPollResult>,
    WaitFor: FnMut(Duration) -> Option<ActiveThreadPollResult>,
    Sleep: FnMut(Duration),
    Clock: FnMut() -> Instant,
    EpochCheck: FnMut(&AccountEpochProof) -> bool,
    AfterPublication:
        FnMut(&mut Recorder, &mut RecorderStateWriter) -> Result<(), Box<dyn std::error::Error>>,
{
    if let Ok(state) = recorder.state() {
        let _ = cadence.submit(clock(), &state.checkpoints, submit);
    }
    if let Some(result) = drain().into_iter().last() {
        commit_thread_poll_result(
            result,
            cycle_epoch,
            recorder,
            quota_health,
            thread_health,
            cadence,
            clock(),
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
    if options.once || shutdown_requested() {
        if cadence.checkpoint_recovery.pending {
            *thread_health = LaneHealth::Failed;
            sync_acquisition_health(recorder, quota_health, *thread_health);
        }
        if !shutdown_requested() && !cadence.checkpoint_recovery.pending {
            if !epoch_matches(cycle_epoch) {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            after_publication(recorder, state_writer)?;
        }
        return Ok(false);
    }
    if cadence.checkpoint_recovery.pending {
        return Ok(true);
    }
    let now = clock();
    let remaining = schedule.time_until_deadline(now);
    if !remaining.is_zero() {
        let deadline = now + remaining;
        loop {
            if shutdown_requested() {
                return Ok(false);
            }
            let now = clock();
            if now >= deadline {
                break;
            }
            // Thread deadlines are independent of the Session schedule. A
            // busy worker keeps its single outstanding request even when a
            // five-second slot or the next Session boundary passes.
            if !cadence.in_flight && cadence.next_probe.is_some_and(|due| now >= due) {
                if !epoch_matches(cycle_epoch) {
                    return Err(RecorderError::AccountBoundaryChanged.into());
                }
                if let Ok(state) = recorder.state() {
                    let _ = cadence.submit(now, &state.checkpoints, submit);
                } else {
                    cadence.next_probe = Some(now + ACTIVE_THREAD_INTERVAL);
                }
            }
            let wake_at = if cadence.in_flight {
                deadline
            } else {
                cadence.next_probe.map_or(deadline, |due| due.min(deadline))
            };
            let timeout = wake_at.saturating_duration_since(clock());
            if !cadence.in_flight {
                if !timeout.is_zero() {
                    sleep(timeout);
                }
                continue;
            }
            if let Some(result) = wait_for(timeout) {
                let committed = commit_thread_poll_result(
                    result,
                    cycle_epoch,
                    recorder,
                    quota_health,
                    thread_health,
                    cadence,
                    clock(),
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
                if cadence.checkpoint_recovery.pending {
                    return Ok(true);
                }
            } else {
                // A timeout never means an unchanged or completed probe. A
                // disconnected worker also must not cause a busy wait.
                let remaining = wake_at.saturating_duration_since(clock());
                if !remaining.is_zero() {
                    sleep(remaining);
                }
            }
        }
    }
    // Advance only after a normal wait, but account for work using its finish
    // time rather than the wake-up time (which can be just past the deadline).
    let wait = schedule.complete_cycle(now);
    if wait.missed_deadlines != 0 {
        eprintln!(
            "codex-info-recorder sampling overrun: missed_deadlines={}",
            wait.missed_deadlines
        );
    }
    if !shutdown_requested() && !cadence.checkpoint_recovery.pending {
        if !epoch_matches(cycle_epoch) {
            return Err(RecorderError::AccountBoundaryChanged.into());
        }
        after_publication(recorder, state_writer)?;
    }
    Ok(true)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.iter().any(|argument| argument == "--version") {
        println!("{RECORDER_VERSION}");
        return Ok(());
    }
    install_shutdown_signal()?;
    let mut options = Options::parse(arguments)?;
    prepare_recorder_data_root(&options.data_root)
        .map_err(|error| format!("prepare recorder data root: {error}"))?;
    let _profile_lease = Arc::new(ProfileLease::acquire(&options.data_root)?);
    let previous_authority = RecorderStateWriter::read_previous_authority(&options.data_root)?;
    if AccountEpochProof::capture(&options.codex_home).is_err() {
        let auth_path = options.codex_home.join("auth.json");
        match fs::symlink_metadata(&auth_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return run_without_account(&options, &_profile_lease)
            }
            _ => {
                if _profile_lease.validate_current(&options.data_root).is_ok() {
                    let _ =
                        close_partition_ownership_at(&options.data_root, Utc::now().timestamp());
                }
                return Err(RecorderError::Invalid(
                    "Codex account authority is present but invalid".to_owned(),
                )
                .into());
            }
        }
    }
    let partition = reserve_partition_at(
        &options.codex_home,
        &options.data_root,
        Utc::now().timestamp(),
    )
    .map_err(|error| format!("reserve account database: {error}"))?;
    let began_at = Utc::now().timestamp();
    let startup_epoch = AccountEpochProof::capture(&options.codex_home)
        .map_err(|_| RecorderError::AccountBoundaryChanged)?;
    _profile_lease.validate_current(&options.data_root)?;
    let initial = partition_requires_confirmation(&options.data_root, &partition)?;
    if initial {
        if let Err(error) = confirm_codex_account(&startup_epoch, &options.codex_home) {
            if !startup_epoch.validate_current(&options.codex_home) {
                let _ = close_partition_ownership_at(&options.data_root, Utc::now().timestamp());
            }
            return Err(RecorderError::Invalid(error).into());
        }
    }
    _profile_lease.validate_current(&options.data_root)?;
    let confirmation = if initial {
        AccountConfirmation::InitialRpc { began_at }
    } else {
        AccountConfirmation::Saved { began_at }
    };
    let partition = startup_epoch.run_if_current(&options.codex_home, || {
        admit_partition(
            &options.codex_home,
            &options.data_root,
            &partition,
            confirmation,
            || {
                (_profile_lease.validate_current(&options.data_root).is_ok()
                    && startup_epoch.validate_current(&options.codex_home))
                .then(|| Utc::now().timestamp())
            },
        )
    })??;
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
    // The raw input owner starts before Session storage admission. A broken
    // derived table or a temporarily unavailable writer must not stop quota
    // acquisition; the accepted journal can be replayed after storage recovers.
    let mut quota_poller = QuotaPoller::start_with_interval(options.interval_secs);
    let lifecycle_intervals: Vec<_> = options
        .partition
        .lifecycle_intervals
        .iter()
        .map(|interval| SessionLifecycleInterval::new(interval.start_at, interval.end_at))
        .collect();
    let mut recorder = loop {
        if shutdown_requested() {
            return Ok(());
        }
        if !recorder_epoch_matches(&options, &startup_epoch, &_profile_lease) {
            return Err(RecorderError::AccountBoundaryChanged.into());
        }
        let initialized = (|| -> Result<Recorder, Box<dyn std::error::Error>> {
            let mut recorder = Recorder::open_partitioned_for_account_with_lifecycle(
                RecorderConfig {
                    sessions_root: options.sessions_root.clone(),
                    chunk_bytes: options.chunk_bytes,
                },
                &options.database,
                &options.identity,
                options.partition.activation_timestamp,
                transition_fingerprint,
                lifecycle_intervals.clone(),
            )?;
            let startup_backup = UsageStore::backup_generations_partitioned_verified(
                &options.database,
                &options.identity,
                3,
            )?;
            if recorder.reconcile_session_lifecycle_guarded(&startup_backup, || {
                recorder_epoch_matches(&options, &startup_epoch, &_profile_lease)
            })? {
                eprintln!("codex-info-recorder repaired session totals across account lifecycles");
            }
            mark_partition_initialized(&options.data_root, &options.partition)
                .map_err(|error| format!("mark account partition initialized: {error}"))?;
            Ok(recorder)
        })();
        match initialized {
            Ok(recorder) => break recorder,
            Err(error) => {
                if !recorder_epoch_matches(&options, &startup_epoch, &_profile_lease) {
                    return Err(RecorderError::AccountBoundaryChanged.into());
                }
                eprintln!("codex-info-recorder degraded Session startup: {error}");
                if options.once {
                    return Err(error);
                }
                shutdown_sleep(Duration::from_secs(options.interval_secs));
            }
        }
    };
    if shutdown_requested() {
        return Ok(());
    }
    let mut state_writer =
        RecorderStateWriter::new(&options.data_root, &options.identity, &_profile_lease)?;
    if !recorder_epoch_matches(&options, &startup_epoch, &_profile_lease) {
        return Err(RecorderError::AccountBoundaryChanged.into());
    }
    if let Some(state) = recorder.acknowledge_existing_owner_guarded(|| {
        recorder_epoch_matches(&options, &startup_epoch, &_profile_lease)
    })? {
        state_writer.write_committed(&state, true)?;
    }
    // Historical file reads and inactive DB work have separate owners. Their
    // pending or failed result cannot postpone a quota/Session collection.
    let mut current_history = recorder
        .start_historical_repair_worker()
        .map_err(|error| {
            eprintln!("codex-info-recorder historical preparation unavailable: {error}");
        })
        .ok();
    let mut inactive_history = start_inactive_history(
        options.clone(),
        startup_epoch.clone(),
        Arc::clone(&_profile_lease),
    )
    .map_err(|error| {
        eprintln!("codex-info-recorder inactive history worker unavailable: {error}");
    })
    .ok();
    let thread_poller = ThreadPoller::start(options.sessions_root.clone());
    let mut schedule = FixedRateSchedule::new(Duration::from_secs(options.interval_secs));
    let mut thread_cadence = AdaptiveThreadCadence::default();
    let mut quota_poll_health = LaneHealth::Unknown;
    let mut thread_health = LaneHealth::Unknown;
    loop {
        if shutdown_requested() {
            return Ok(());
        }
        let cycle_epoch = AccountEpochProof::capture(&options.codex_home)
            .map_err(|_| RecorderError::AccountBoundaryChanged)?;
        if !recorder_epoch_matches(&options, &cycle_epoch, &_profile_lease) {
            return Err(RecorderError::AccountBoundaryChanged.into());
        }
        let early_recollection = if thread_cadence.checkpoint_recovery.pending {
            if !thread_cadence.begin_checkpoint_recovery(&cycle_epoch) {
                shutdown_sleep(schedule.time_until_deadline(Instant::now()));
                schedule.complete_cycle(Instant::now());
                continue;
            }
            true
        } else {
            false
        };
        let replay_began = Instant::now();
        eprintln!("codex-info-recorder diagnostic quota_replay start");
        let replay_failed = match recorder.replay_quota_journal(
            &options.data_root,
            &options.partition.account_scope_id,
            || recorder_epoch_matches(&options, &cycle_epoch, &_profile_lease),
        ) {
            Ok(outcome) => {
                if outcome.unresolved != 0 {
                    eprintln!(
                        "codex-info-recorder quota replay unresolved={}",
                        outcome.unresolved
                    );
                }
                outcome.latest_input_unresolved
            }
            Err(RecorderError::AccountBoundaryChanged) => {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            Err(error) => {
                eprintln!("codex-info-recorder degraded quota replay: {error}");
                true
            }
        };
        eprintln!(
            "codex-info-recorder diagnostic quota_replay finish elapsed_ms={}",
            replay_began.elapsed().as_millis()
        );
        let mut quota_health = if replay_failed {
            LaneHealth::Failed
        } else {
            quota_poll_health
        };
        sync_quota_poll_health(&mut recorder, quota_poll_health);
        sync_acquisition_health(&mut recorder, quota_health, thread_health);
        // Receipt/health commits are independent of a potentially long
        // bounded source reconstruction. Acknowledge their real DB generation
        // before starting that work; never refresh an unchanged generation.
        if recorder.has_accepted_quota()? {
            let state = recorder.state()?;
            if state.data_generation > state_writer.last_published_generation().unwrap_or(0)
                && state.collector_epoch.is_some()
                && state.cycle_seq > 0
            {
                if !recorder_epoch_matches(&options, &cycle_epoch, &_profile_lease) {
                    return Err(RecorderError::AccountBoundaryChanged.into());
                }
                state_writer.write_committed(&state, true)?;
            }
        }
        // Drain notifications for health reporting only. The durable receipt
        // store, including replay after restart, owns the actual quota input.
        let _ = quota_poller.latest();
        if shutdown_requested() {
            return Ok(());
        }
        let collection_began = Instant::now();
        eprintln!("codex-info-recorder diagnostic session_collection start");
        let publication = match recorder.run_account_cycle_with_stored_quota_guarded(
            &options.codex_home,
            &cycle_epoch,
            || recorder_epoch_matches(&options, &cycle_epoch, &_profile_lease),
        ) {
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
        eprintln!(
            "codex-info-recorder diagnostic session_collection finish elapsed_ms={}",
            collection_began.elapsed().as_millis()
        );
        if early_recollection && matches!(publication, CyclePublication::Degraded) {
            let now = Instant::now();
            reject_checkpoint_recollection(
                &mut recorder,
                quota_health,
                &mut thread_health,
                &mut thread_cadence,
                now + schedule.time_until_deadline(now),
            );
        }
        // Both lanes are deliberately submitted and drained with nonblocking
        // operations. The DB integration consumes these typed outcomes; an
        // app-server timeout or malformed response must not delay the next
        // Session token cycle.
        if let Some(event) = quota_poller.take_events().into_iter().last() {
            if !recorder_epoch_matches(&options, &cycle_epoch, &_profile_lease) {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            let input_unrecognized = matches!(event, QuotaPollEvent::InputStored { .. });
            match event {
                QuotaPollEvent::Ready {
                    login_id, epoch, ..
                }
                | QuotaPollEvent::InputStored { login_id, epoch } => {
                    if !recorder_epoch_matches(&options, &epoch, &_profile_lease) {
                        return Err(RecorderError::AccountBoundaryChanged.into());
                    }
                    quota_poll_health = LaneHealth::Ready;
                    quota_health = if replay_failed || input_unrecognized {
                        LaneHealth::Failed
                    } else {
                        LaneHealth::Ready
                    };
                    if input_unrecognized {
                        if let Err(error) = recorder.mark_quota_input_degraded() {
                            eprintln!("codex-info-recorder input health commit failed: {error}");
                        }
                    }
                    // A display-label failure must never stop or delay the
                    // recorder's authoritative Session/DB path.
                    if let Err(error) = recorder.set_partition_login_id(&login_id) {
                        eprintln!(
                            "codex-info-recorder degraded account DB label persistence failed: {error}"
                        );
                    }
                    if !recorder_epoch_matches(&options, &epoch, &_profile_lease) {
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
                    quota_poll_health = LaneHealth::Failed;
                    quota_health = LaneHealth::Failed;
                    eprintln!("codex-info-recorder quota lane degraded");
                }
            }
            if !recorder_epoch_matches(&options, &cycle_epoch, &_profile_lease) {
                return Err(RecorderError::AccountBoundaryChanged.into());
            }
            sync_quota_poll_health(&mut recorder, quota_poll_health);
            sync_acquisition_health(&mut recorder, quota_health, thread_health);
        }
        let mut submit = |checkpoints: &[SessionCheckpoint]| thread_poller.submit(checkpoints);
        let mut drain = || thread_poller.drain();
        let mut wait_for =
            |duration: Duration| thread_poller.wait_for_interruptible(duration, shutdown_requested);
        let mut sleep = shutdown_sleep;
        let mut clock = || Instant::now();
        let mut epoch_matches =
            |epoch: &AccountEpochProof| recorder_epoch_matches(&options, epoch, &_profile_lease);
        let mut after_publication = |recorder: &mut Recorder,
                                     state_writer: &mut RecorderStateWriter|
         -> Result<(), Box<dyn std::error::Error>> {
            if shutdown_requested() {
                return Ok(());
            }
            if let Some(worker) = inactive_history.as_mut() {
                if let Some(Err(error)) = worker.try_result() {
                    if matches!(error, RecorderError::AccountBoundaryChanged) {
                        return Err(error.into());
                    }
                    eprintln!("codex-info-recorder inactive history unavailable: {error}");
                }
                if worker.is_unavailable() {
                    eprintln!("codex-info-recorder inactive history worker disconnected; recording continues independently");
                    inactive_history = None;
                } else {
                    worker.request();
                }
            }
            service_current_history(&mut current_history, recorder, state_writer, || {
                recorder_epoch_matches(&options, &cycle_epoch, &_profile_lease)
            })
        };
        if !complete_cycle_publication_and_wait(
            publication,
            &options,
            &cycle_epoch,
            &mut recorder,
            &mut state_writer,
            quota_health,
            &mut thread_health,
            &mut schedule,
            &mut thread_cadence,
            &mut submit,
            &mut drain,
            &mut wait_for,
            &mut sleep,
            &mut clock,
            &mut epoch_matches,
            &mut after_publication,
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
    profile_lease.validate_current(&options.data_root)?;
    close_partition_ownership_at(&options.data_root, Utc::now().timestamp())?;
    let mut state_writer = RecorderStateWriter::new_idle(&options.data_root, profile_lease)?;
    state_writer.write_idle_no_account()?;
    if options.once {
        return Ok(());
    }
    let heartbeat = Duration::from_secs(options.interval_secs.clamp(1, 5));
    loop {
        shutdown_sleep(heartbeat);
        if shutdown_requested() {
            return Ok(());
        }
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

fn account_authority_matches(options: &Options) -> bool {
    current_account_scope_id(&options.codex_home, &options.data_root)
        .is_ok_and(|scope| scope == options.partition.account_scope_id)
}

fn recorder_epoch_matches(
    options: &Options,
    epoch: &AccountEpochProof,
    lease: &ProfileLease,
) -> bool {
    let matches = epoch.validate_current(&options.codex_home) && account_authority_matches(options);
    let lease_current = lease.validate_current(&options.data_root).is_ok();
    if !matches && lease_current {
        // Record observed identity loss; RPC-only failure does not reach it.
        let _ = close_partition_ownership_at(&options.data_root, Utc::now().timestamp());
    }
    matches && lease_current
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
    fn issue604_checkpoint_recollection_and_stop_do_not_wait_for_inactive_history() {
        let recollection_calls = Cell::new(0);
        let recollection = run_inactive_history(
            true,
            || false,
            [0, 1],
            |_| {
                recollection_calls.set(recollection_calls.get() + 1);
                Ok::<(), ()>(())
            },
        );

        let stopped_calls = Cell::new(0);
        let stopped = run_inactive_history(
            false,
            || true,
            [0, 1],
            |_| {
                stopped_calls.set(stopped_calls.get() + 1);
                Ok::<(), ()>(())
            },
        );

        let between_calls = Cell::new(0);
        let stop_between = Cell::new(false);
        let between = run_inactive_history(
            false,
            || stop_between.get(),
            [0, 1],
            |_| {
                between_calls.set(between_calls.get() + 1);
                stop_between.set(true);
                Ok::<(), ()>(())
            },
        );

        let normal_calls = Cell::new(0);
        let normal = run_inactive_history(
            false,
            || false,
            [0, 1],
            |_| {
                normal_calls.set(normal_calls.get() + 1);
                Ok::<(), ()>(())
            },
        );

        assert_eq!(
            recollection_calls.get(),
            0,
            "recollection starts no inactive job"
        );
        assert_eq!(
            recollection,
            Ok(true),
            "recollection continues to the active phase"
        );
        assert_eq!(stopped_calls.get(), 0, "an existing stop starts no job");
        assert_eq!(stopped, Ok(false), "an existing stop does not continue");
        assert_eq!(
            between_calls.get(),
            1,
            "stop prevents the next independent job"
        );
        assert_eq!(between, Ok(false), "a stop between jobs does not continue");
        assert_eq!(normal_calls.get(), 2, "ordinary history runs both jobs");
        assert_eq!(normal, Ok(true), "ordinary history continues");
    }

    #[test]
    fn version_is_the_recorder_package_version() {
        assert!(!super::RECORDER_VERSION.is_empty());
        assert!(super::RECORDER_VERSION
            .split('.')
            .all(|component| !component.is_empty()));
    }

    #[test]
    fn issue604_normal_publication_precedes_optional_history() {
        let mut outcomes = Vec::new();
        for case in [
            "normal",
            "once",
            "no-commit",
            "degraded",
            "recollection",
            "boundary",
            "stop",
        ] {
            SHUTDOWN_REQUESTED.store(false, Ordering::Release);
            let mut fixture = issue_362_fixture(&format!("history-after-publication-{case}"));
            fixture.seed_state(false, false);
            fixture
                .recorder
                .run_cycle_with_quota(None)
                .unwrap()
                .unwrap();
            let normal_generation = fixture.recorder.state().unwrap().data_generation;
            assert!(normal_generation > fixture.state_writer.last_published_generation().unwrap());
            fixture.options.once = case == "once";
            let publication = match case {
                "no-commit" => CyclePublication::NoCommit,
                "degraded" => CyclePublication::Degraded,
                _ => CyclePublication::Committed { has_pending: false },
            };
            let anchor = Instant::now();
            let now = Cell::new(anchor);
            let trace = std::cell::RefCell::new(Vec::new());
            let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
            let mut cadence = AdaptiveThreadCadence::default();
            let mut thread_health = LaneHealth::Ready;
            let mut submit = |_: &[SessionCheckpoint]| {
                trace.borrow_mut().push("submit");
                true
            };
            let mut polled = Some(if case == "recollection" {
                ActiveThreadPollResult::CheckpointMismatch {
                    sources: Vec::new(),
                }
            } else {
                fixture.poll_result("after-normal", 2)
            });
            let mut drain = || {
                trace.borrow_mut().push("drain");
                polled.take().into_iter().collect()
            };
            let mut wait_for = |duration| {
                trace.borrow_mut().push("wait");
                now.set(now.get() + duration);
                None
            };
            let mut sleep = |duration| {
                trace.borrow_mut().push("sleep");
                now.set(now.get() + duration);
            };
            let mut clock = || now.get();
            let mut epoch_matches = |_: &AccountEpochProof| case != "boundary";
            let calls = Cell::new(0);
            let ordered = Cell::new(false);
            let acknowledged = Cell::new(false);
            let after_wait = Cell::new(false);
            let mut history = |recorder: &mut Recorder,
                               writer: &mut RecorderStateWriter|
             -> Result<(), Box<dyn std::error::Error>> {
                calls.set(calls.get() + 1);
                ordered.set(trace.borrow().starts_with(&["submit", "drain"]));
                let state = recorder.state()?;
                acknowledged.set(
                    state.data_generation >= normal_generation
                        && writer.last_published_generation() == Some(state.data_generation),
                );
                after_wait.set(
                    now.get()
                        == if case == "once" {
                            anchor
                        } else {
                            anchor + Duration::from_secs(60)
                        },
                );
                trace.borrow_mut().push("history");
                Ok(())
            };
            SHUTDOWN_REQUESTED.store(case == "stop", Ordering::Release);
            let result = complete_cycle_publication_and_wait(
                publication,
                &fixture.options,
                &fixture.epoch,
                &mut fixture.recorder,
                &mut fixture.state_writer,
                LaneHealth::Ready,
                &mut thread_health,
                &mut schedule,
                &mut cadence,
                &mut submit,
                &mut drain,
                &mut wait_for,
                &mut sleep,
                &mut clock,
                &mut epoch_matches,
                &mut history,
            );
            SHUTDOWN_REQUESTED.store(false, Ordering::Release);
            if case == "boundary" {
                assert!(matches!(
                    result.unwrap_err().downcast_ref::<RecorderError>(),
                    Some(RecorderError::AccountBoundaryChanged)
                ));
            } else {
                assert_eq!(result.unwrap(), !matches!(case, "once" | "stop"), "{case}");
            }
            if case == "recollection" {
                assert!(cadence.checkpoint_recovery.pending);
            }
            let should_run = !matches!(case, "recollection" | "boundary" | "stop");
            outcomes.push((
                case,
                calls.get(),
                !should_run || (ordered.get() && after_wait.get()),
                !matches!(case, "normal" | "once") || acknowledged.get(),
            ));
            fixture.cleanup();
        }
        assert_eq!(outcomes,[("normal",1,true,true),("once",1,true,true),
            ("no-commit",1,true,true),("degraded",1,true,true),("recollection",0,true,true),
            ("boundary",0,true,true),("stop",0,true,true)],
            "normal commit, Thread drain and publication precede optional history; recovery and stop take priority");
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
                activity_status: "running".to_owned(),
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

    fn adaptive_thread_snapshot(total_tokens: Option<u64>) -> ActiveThreadSnapshot {
        let mut snapshot = issue_362_snapshot("adaptive", 1);
        snapshot.threads[0].total_tokens = total_tokens;
        snapshot
    }

    #[test]
    fn adaptive_thread_cadence_baseline_does_not_activate() {
        let fixture = issue_362_fixture("adaptive-baseline");
        let mut cadence = AdaptiveThreadCadence::default();
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(10)),
            Instant::now(),
        );
        assert_eq!(cadence.last_delta, None);
        assert_eq!(cadence.next_probe, None);
        fixture.cleanup();
    }

    #[test]
    fn adaptive_thread_cadence_increase_and_decrease_activate_five_seconds() {
        let fixture = issue_362_fixture("adaptive-delta");
        let anchor = Instant::now();
        for next in [11, 9] {
            let mut cadence = AdaptiveThreadCadence::default();
            cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(10)), anchor);
            cadence.accept(
                &fixture.epoch,
                &adaptive_thread_snapshot(Some(next)),
                anchor + Duration::from_secs(60),
            );
            assert_eq!(cadence.last_delta, Some(anchor + Duration::from_secs(60)));
            assert_eq!(cadence.next_probe, Some(anchor + Duration::from_secs(65)));
        }
        fixture.cleanup();
    }

    #[test]
    fn adaptive_thread_cadence_null_is_unknown_and_zero_is_known() {
        let fixture = issue_362_fixture("adaptive-null");
        let anchor = Instant::now();
        let mut cadence = AdaptiveThreadCadence::default();
        for (at, value) in [(0, None), (60, Some(0)), (120, None), (180, Some(0))] {
            cadence.accept(
                &fixture.epoch,
                &adaptive_thread_snapshot(value),
                anchor + Duration::from_secs(at),
            );
            assert_eq!(cadence.last_delta, None);
        }
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(1)),
            anchor + Duration::from_secs(240),
        );
        assert_eq!(cadence.last_delta, Some(anchor + Duration::from_secs(240)));
        fixture.cleanup();
    }

    #[test]
    fn adaptive_thread_cadence_new_thread_and_restart_establish_baselines() {
        let fixture = issue_362_fixture("adaptive-identity");
        let anchor = Instant::now();
        let mut cadence = AdaptiveThreadCadence::default();
        cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(10)), anchor);
        let mut replacement = adaptive_thread_snapshot(Some(20));
        replacement.threads[0].id = "replacement-thread".to_owned();
        cadence.accept(
            &fixture.epoch,
            &replacement,
            anchor + Duration::from_secs(60),
        );
        assert_eq!(cadence.last_delta, None);
        replacement.threads[0].total_tokens = Some(21);
        cadence.accept(
            &fixture.epoch,
            &replacement,
            anchor + Duration::from_secs(120),
        );
        assert!(cadence.last_delta.is_some());
        let mut restarted = AdaptiveThreadCadence::default();
        restarted.accept(
            &fixture.epoch,
            &replacement,
            anchor + Duration::from_secs(120),
        );
        assert_eq!(restarted.last_delta, None);
        fixture.cleanup();
    }

    #[test]
    fn adaptive_thread_cadence_account_epoch_change_resets_activity() {
        let fixture = issue_362_fixture("adaptive-epoch");
        let anchor = Instant::now();
        let mut cadence = AdaptiveThreadCadence::default();
        cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(10)), anchor);
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(11)),
            anchor + Duration::from_secs(60),
        );
        assert!(cadence.last_delta.is_some());
        fs::write(
            fixture.options.codex_home.join("auth.json"),
            br#"{"tokens":{"account_id":"adaptive-other-account","access_token":"secret"}}"#,
        )
        .expect("new test account authority");
        let next_epoch =
            AccountEpochProof::capture(&fixture.options.codex_home).expect("new account epoch");
        assert_ne!(next_epoch, fixture.epoch);
        cadence.accept(
            &next_epoch,
            &adaptive_thread_snapshot(Some(12)),
            anchor + Duration::from_secs(65),
        );
        assert_eq!(cadence.last_delta, None);
        assert_eq!(cadence.next_probe, None);
        fixture.cleanup();
    }

    #[test]
    fn adaptive_thread_cadence_failure_retains_last_good_and_activity() {
        let fixture = issue_362_fixture("adaptive-failure");
        let anchor = Instant::now();
        let mut cadence = AdaptiveThreadCadence::default();
        cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(10)), anchor);
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(11)),
            anchor + Duration::from_secs(60),
        );
        let accepted = cadence.totals.clone();
        cadence.in_flight = true;
        cadence.failed();
        assert!(!cadence.in_flight);
        assert_eq!(cadence.totals, accepted);
        assert_eq!(cadence.last_delta, Some(anchor + Duration::from_secs(60)));
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(12)),
            anchor + Duration::from_secs(125),
        );
        assert_eq!(cadence.last_delta, Some(anchor + Duration::from_secs(125)));
        fixture.cleanup();
    }

    #[test]
    fn adaptive_thread_cadence_does_not_queue_an_overlapping_probe() {
        let mut cadence = AdaptiveThreadCadence::default();
        let anchor = Instant::now();
        let mut submissions = 0;
        let mut submit = |_: &[SessionCheckpoint]| {
            submissions += 1;
            true
        };
        assert!(cadence.submit(anchor, &[], &mut submit));
        assert!(!cadence.submit(anchor + ACTIVE_THREAD_INTERVAL, &[], &mut submit));
        cadence.failed();
        assert!(cadence.submit(anchor + ACTIVE_THREAD_INTERVAL, &[], &mut submit));
        assert_eq!(submissions, 2);
    }

    #[test]
    fn adaptive_thread_cadence_only_exits_after_successful_stable_sixty_seconds() {
        let fixture = issue_362_fixture("adaptive-stable");
        let anchor = Instant::now();
        let mut cadence = AdaptiveThreadCadence::default();
        cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(10)), anchor);
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(11)),
            anchor + Duration::from_secs(60),
        );
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(11)),
            anchor + Duration::from_secs(119),
        );
        assert!(cadence.last_delta.is_some());
        cadence.failed();
        assert!(cadence.last_delta.is_some());
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(11)),
            anchor + Duration::from_secs(120),
        );
        assert_eq!(cadence.last_delta, None);
        assert_eq!(cadence.next_probe, None);
        fixture.cleanup();
    }

    #[test]
    fn adaptive_thread_cadence_unknown_values_do_not_prove_stability() {
        let fixture = issue_362_fixture("adaptive-unknown-stable");
        let anchor = Instant::now();
        let mut cadence = AdaptiveThreadCadence::default();
        cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(10)), anchor);
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(11)),
            anchor + Duration::from_secs(60),
        );
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(None),
            anchor + Duration::from_secs(120),
        );
        assert!(cadence.last_delta.is_some());
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(0)),
            anchor + Duration::from_secs(125),
        );
        assert!(cadence.last_delta.is_some());
        cadence.accept(
            &fixture.epoch,
            &adaptive_thread_snapshot(Some(0)),
            anchor + Duration::from_secs(130),
        );
        assert_eq!(cadence.last_delta, None);
        fixture.cleanup();
    }

    #[test]
    fn adaptive_thread_cadence_wait_drives_five_second_probes_and_returns_to_configured_cycle() {
        let mut fixture = issue_362_fixture("adaptive-wait");
        fixture.options.interval_secs = 120;
        fixture.seed_state(false, false);
        let initial_publication = fixture.state_bytes();
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(120));
        let mut cadence = AdaptiveThreadCadence::default();
        cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(1)), anchor);
        cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(2)), anchor);
        let probe_pending = Cell::new(false);
        let mut starts = Vec::new();
        let mut submit = |_: &[SessionCheckpoint]| {
            assert!(!probe_pending.replace(true), "probe work must not overlap");
            starts.push(now.get().duration_since(anchor).as_secs());
            true
        };
        let mut drain = || Vec::new();
        let result = ActiveThreadPollResult::Snapshot {
            snapshot: adaptive_thread_snapshot(Some(2)),
            epoch: fixture.epoch.clone(),
        };
        let mut wait_for = |duration: Duration| {
            assert!(probe_pending.get(), "wait only for an existing probe");
            let elapsed = duration.min(Duration::from_secs(1));
            now.set(now.get() + elapsed);
            if elapsed == Duration::from_secs(1) {
                probe_pending.set(false);
                Some(result.clone())
            } else {
                None
            }
        };
        let mut sleep = |duration: Duration| now.set(now.get() + duration);
        let mut clock = || now.get();
        let mut epoch_matches = |_: &AccountEpochProof| true;
        let mut health = LaneHealth::Ready;
        complete_cycle_publication_and_wait(
            CyclePublication::NoCommit,
            &fixture.options,
            &fixture.epoch,
            &mut fixture.recorder,
            &mut fixture.state_writer,
            LaneHealth::Ready,
            &mut health,
            &mut schedule,
            &mut cadence,
            &mut submit,
            &mut drain,
            &mut wait_for,
            &mut sleep,
            &mut clock,
            &mut epoch_matches,
            &mut |_, _| Ok(()),
        )
        .expect("adaptive wait");
        assert_eq!(starts, (1..=12).map(|slot| slot * 5).collect::<Vec<_>>());
        assert_eq!(now.get(), anchor + Duration::from_secs(120));
        assert_eq!(cadence.last_delta, None);
        assert!(!cadence.in_flight);
        assert_eq!(
            fixture.state_bytes(),
            initial_publication,
            "NoCommit must not acknowledge adaptive commits"
        );
        assert_eq!(
            schedule.complete_cycle(now.get()).sleep_for,
            Duration::from_secs(120)
        );
        fixture.cleanup();
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
        let mut cadence = AdaptiveThreadCadence::default();
        complete_cycle_publication_and_wait(
            publication,
            &fixture.options,
            &fixture.epoch,
            &mut fixture.recorder,
            &mut fixture.state_writer,
            quota_health,
            &mut thread_health,
            schedule,
            &mut cadence,
            &mut submit,
            &mut drain,
            &mut wait_for,
            &mut sleep,
            &mut clock,
            &mut epoch_matches,
            &mut |_, _| Ok(()),
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
    fn issue604_main_cycles_publish_two_receipts_while_history_is_blocked() {
        use codex_info_db_writer::QuotaReceipt;
        use std::io::Write;
        let mut fixture = issue_362_fixture("main-history-isolation");
        fixture.seed_state(false, false);
        let at = Utc::now().timestamp().div_euclid(60) * 60 - 180;
        let reset = at + 3600;
        let source = fixture.options.sessions_root.join("active.jsonl");
        let token = |total, timestamp| {
            format!(
                "{}\n",
                serde_json::json!({"timestamp":chrono::DateTime::<Utc>::from_timestamp(timestamp,0).unwrap().to_rfc3339(),
                "type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{
                    "total_tokens":total,"input_tokens":total,"cached_input_tokens":0,"output_tokens":0,"cache_write_input_tokens":0}}}})
            )
        };
        fs::write(&source, format!("{}\n{}\n{}",
            serde_json::json!({"type":"session_meta","timestamp":chrono::DateTime::<Utc>::from_timestamp(at,0).unwrap().to_rfc3339(),"payload":{"id":"thread","session_id":"thread"}}),
            serde_json::json!({"type":"turn_context","timestamp":chrono::DateTime::<Utc>::from_timestamp(at,0).unwrap().to_rfc3339(),"payload":{"model":"gpt-6.1-sol"}}), token(0,at))).unwrap();
        let (began, started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let mut worker: Option<CurrentHistoryWorker> = Some(
            BackgroundWork::start("blocked-main-history", move || {
                began.send(()).unwrap();
                released.recv().unwrap();
                Ok(None)
            })
            .unwrap(),
        );
        assert!(worker.as_mut().unwrap().request());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut cadence = AdaptiveThreadCadence::default();
        let mut health = LaneHealth::Ready;
        let before = fixture.recorder.state().unwrap().cycle_seq;
        for index in 0..2_u64 {
            let observed = at + index as i64 * 60 + 10;
            fs::OpenOptions::new()
                .append(true)
                .open(&source)
                .unwrap()
                .write_all(token((index + 1) * 5, observed).as_bytes())
                .unwrap();
            // Inject an already independently accepted official receipt at
            // the production writer boundary; its raw-input proof is owned
            // by the quota journal's separate direct tests.
            let mut writer =
                UsageStore::open_partitioned(&fixture.options.database, &fixture.options.identity)
                    .unwrap();
            writer
                .commit_quota_receipts(
                    &[QuotaReceipt {
                        receipt_id: format!("{:064x}", index + 1),
                        sequence: index as i64 + 1,
                        observed_at: observed,
                        reset_at: reset,
                        window_seconds: 3600,
                        remaining_percent: 80.0 - index as f64,
                    }],
                    false,
                )
                .unwrap();
            drop(writer);
            fixture
                .recorder
                .run_cycle_with_stored_quota_guarded(|| true)
                .unwrap()
                .unwrap();
            complete_cycle_publication_and_wait(
                CyclePublication::Committed { has_pending: false },
                &fixture.options,
                &fixture.epoch,
                &mut fixture.recorder,
                &mut fixture.state_writer,
                LaneHealth::Ready,
                &mut health,
                &mut schedule,
                &mut cadence,
                &mut |_| false,
                &mut || Vec::new(),
                &mut |duration| {
                    now.set(now.get() + duration);
                    None
                },
                &mut |duration| now.set(now.get() + duration),
                &mut || now.get(),
                &mut |_| true,
                &mut |recorder, state_writer| {
                    service_current_history(&mut worker, recorder, state_writer, || true)
                },
            )
            .unwrap();
            let state = fixture.recorder.state().unwrap();
            assert_eq!(state.cycle_seq, before + index + 1);
            assert_eq!(
                fixture.state_writer.last_published_generation().unwrap(),
                state.data_generation
            );
            assert_eq!(
                fixture.recorder.model_totals().unwrap()["gpt-6.1-sol"].total,
                (index + 1) * 5
            );
            assert!(
                worker.as_mut().unwrap().try_result().is_none(),
                "in-flight work cannot be a committed historical batch"
            );
        }
        assert_eq!(now.get(), anchor + Duration::from_secs(120));
        let db = rusqlite::Connection::open(&fixture.options.database).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM quota_receipts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(db);
        release.send(()).unwrap();
        drop(worker);
        fixture.cleanup();
    }

    #[test]
    fn issue604_pending_quota_keeps_independent_thread_commit_and_publication() {
        for degraded in [false, true] {
            let mut fixture = issue_362_fixture("pending-quota-retention");
            let before_ack = fixture.seed_state(false, degraded);
            let before_generation = fixture.recorder.state().unwrap().data_generation;
            let anchor = Instant::now();
            let now = Cell::new(anchor);
            let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
            let candidate = fixture.poll_result("independent-ready-thread", 2);
            let (mut submits, mut drains, mut waits) = (0, 0, 0);
            issue_362_drive_cycle(
                &mut fixture,
                CyclePublication::Committed { has_pending: false },
                LaneHealth::Unknown,
                &mut schedule,
                &now,
                Some(candidate),
                vec![true; 8],
                None,
                &mut submits,
                &mut drains,
                &mut waits,
            )
            .unwrap();
            let (payload, thread_degraded) = issue_134_thread_row(&fixture);
            let threads: Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(
                threads[0]["title"], "independent-ready-thread",
                "quota acquisition has no veto over a valid same-epoch Thread snapshot"
            );
            assert_eq!(thread_degraded, 0);
            let committed = fixture.recorder.state().unwrap().data_generation;
            assert!(committed > before_generation);
            assert_eq!(
                fixture.state_writer.last_published_generation().unwrap(),
                committed,
                "missing quota cannot hide independently committed data"
            );
            assert_ne!(fixture.state_bytes(), before_ack);
            assert_eq!(now.get(), anchor + Duration::from_secs(60));
            assert_eq!((submits, drains, waits), (1, 1, 1));
            fixture.cleanup();
        }
    }

    #[test]
    fn issue604_pending_quota_does_not_discard_first_thread_observation() {
        let mut fixture = issue_362_fixture("pending-quota-first-root");
        let mut health = LaneHealth::Unknown;
        let mut cadence = AdaptiveThreadCadence::default();
        let before_generation = fixture.recorder.state().unwrap().data_generation;
        let epoch = fixture.epoch.clone();
        for quota in [LaneHealth::Unknown, LaneHealth::Ready] {
            let committed = commit_thread_poll_result(
                ActiveThreadPollResult::Empty {
                    epoch: epoch.clone(),
                },
                &epoch,
                &mut fixture.recorder,
                quota,
                &mut health,
                &mut cadence,
                Instant::now(),
                |_| true,
            )
            .unwrap();
            let rows: i64 = rusqlite::Connection::open(&fixture.options.database)
                .unwrap()
                .query_row("SELECT count(*) FROM active_thread_snapshot", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(rows, 1, "quota cannot discard an observed empty Thread set");
            assert!(committed);
            assert!(fixture.recorder.state().unwrap().data_generation > before_generation);
            assert_eq!(issue_134_thread_row(&fixture), ("[]".to_owned(), 0));
            assert_eq!(health, LaneHealth::Ready);
        }
        fixture.cleanup();
    }

    #[test]
    fn issue_134_pending_quota_still_reports_actual_failures() {
        let mut fixture = issue_362_fixture("pending-quota-real-failure");
        fixture.seed_state(false, false);
        let before_payload = issue_134_thread_row(&fixture).0;
        let mut health = LaneHealth::Unknown;
        let mut cadence = AdaptiveThreadCadence::default();
        let epoch = fixture.epoch.clone();
        let cases = [
            (
                LaneHealth::Unknown,
                ActiveThreadPollResult::Failed("test failure".to_owned()),
                1,
                0,
                1,
            ),
            (
                LaneHealth::Failed,
                fixture.poll_result("quota-failed", 2),
                0,
                1,
                1,
            ),
            (
                LaneHealth::Ready,
                fixture.poll_result("both-ready", 3),
                0,
                0,
                0,
            ),
        ];
        for (quota, result, expected_thread_degraded, expected_quota_degraded, expected_combined) in
            cases
        {
            // The real owner loop persists quota health independently before
            // consuming a thread result. A healthy thread cannot clear it.
            sync_quota_poll_health(&mut fixture.recorder, quota);
            commit_thread_poll_result(
                result,
                &epoch,
                &mut fixture.recorder,
                quota,
                &mut health,
                &mut cadence,
                Instant::now(),
                |_| true,
            )
            .unwrap();
            let (payload, thread_degraded) = issue_134_thread_row(&fixture);
            let (quota_degraded, input_degraded): (i64, i64) =
                rusqlite::Connection::open(&fixture.options.database).unwrap()
                    .query_row(
                        "SELECT COALESCE((SELECT degraded FROM quota_acquisition_state WHERE singleton=1),0),
                                COALESCE((SELECT input_degraded FROM quota_acquisition_state WHERE singleton=1),0)",
                        [], |row| Ok((row.get(0)?, row.get(1)?)),
                    ).unwrap();
            let combined =
                i64::from(thread_degraded != 0 || quota_degraded != 0 || input_degraded != 0);
            assert_eq!(
                (thread_degraded, quota_degraded, input_degraded, combined),
                (
                    expected_thread_degraded,
                    expected_quota_degraded,
                    0,
                    expected_combined
                ),
                "each lane retains its failure until its own confirmed recovery"
            );
            if quota == LaneHealth::Unknown {
                assert_eq!(
                    payload, before_payload,
                    "actual failure retains last-good data"
                );
                assert_eq!(health, LaneHealth::Failed);
            }
        }
        fixture.cleanup();
    }

    fn issue_134_thread_row(fixture: &Issue362AsyncFixture) -> (String, i64) {
        rusqlite::Connection::open(&fixture.options.database).unwrap()
            .query_row("SELECT threads_json, acquisition_degraded FROM active_thread_snapshot WHERE singleton=1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn issue_134_drive_checkpoint_cycle(
        fixture: &mut Issue362AsyncFixture,
        cadence: &mut AdaptiveThreadCadence,
        health: &mut LaneHealth,
        schedule: &mut FixedRateSchedule,
        now: &Cell<Instant>,
        result: Option<ActiveThreadPollResult>,
        initial_drain: bool,
    ) {
        let mut submit = |_: &[SessionCheckpoint]| true;
        let mut initial = if initial_drain { result.clone() } else { None };
        let mut pending = if initial_drain { None } else { result };
        let mut drain = || initial.take().into_iter().collect();
        let mut wait_for = |duration: Duration| {
            let result = pending.take();
            now.set(
                now.get()
                    + if result.is_some() {
                        Duration::from_secs(1)
                    } else {
                        duration
                    },
            );
            result
        };
        let mut sleep = |duration| now.set(now.get() + duration);
        complete_cycle_publication_and_wait(
            CyclePublication::Committed { has_pending: false },
            &fixture.options,
            &fixture.epoch,
            &mut fixture.recorder,
            &mut fixture.state_writer,
            LaneHealth::Ready,
            health,
            schedule,
            cadence,
            &mut submit,
            &mut drain,
            &mut wait_for,
            &mut sleep,
            &mut || now.get(),
            &mut |_| true,
            &mut |_, _| Ok(()),
        )
        .expect("checkpoint cycle");
    }

    #[test]
    fn issue604_delayed_checkpoint_mismatch_uses_current_committed_inventory() {
        use std::os::unix::fs::MetadataExt;
        let mut fixture = issue_362_fixture("delayed-checkpoint-mismatch");
        fixture.seed_state(false, false);
        let root = &fixture.options.sessions_root;
        let noop = b"{\"type\":\"response_item\",\"timestamp\":\"2026-10-17T00:00:00Z\",\"payload\":{\"type\":\"message\"}}\n";
        let active = root.join("active.jsonl");
        fs::write(&active, noop).unwrap();
        let root_metadata = fs::metadata(root).unwrap();
        let metadata = fs::metadata(&active).unwrap();
        let source = codex_info_db_writer::RecordedSessionSource {
            root_identity: format!("unix:{}:{}", root_metadata.dev(), root_metadata.ino()),
            relative_path: "active.jsonl".to_owned(),
            file_device: metadata.dev(),
            file_inode: metadata.ino(),
            file_bytes: metadata.len(),
            modified_nanos: metadata
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        };
        let delayed = ActiveThreadPollResult::CheckpointMismatch {
            sources: vec![source.clone()],
        };
        assert!(fixture.recorder.state().unwrap().checkpoints.is_empty());
        fixture.recorder.run_cycle_with_quota(None).unwrap();
        let committed = fixture.recorder.state().unwrap();
        assert!(committed.checkpoints.iter().any(|checkpoint| {
            checkpoint.root_identity == source.root_identity
                && checkpoint.relative_path == source.relative_path
                && checkpoint.file_device == source.file_device
                && checkpoint.file_inode == source.file_inode
                && checkpoint.committed_offset == noop.len() as u64
        }));
        let before = issue_134_thread_row(&fixture);
        let generation = fixture.recorder.generation().unwrap();
        let publication = fixture.state_bytes();
        for attempted in [true, false] {
            let mut cadence = AdaptiveThreadCadence::default();
            if attempted {
                assert!(cadence.checkpoint_recovery.request(&fixture.epoch));
                assert!(cadence.begin_checkpoint_recovery(&fixture.epoch));
            }
            let mut health = LaneHealth::Ready;
            assert!(
                !commit_thread_poll_result(
                    delayed.clone(),
                    &fixture.epoch,
                    &mut fixture.recorder,
                    LaneHealth::Ready,
                    &mut health,
                    &mut cadence,
                    Instant::now(),
                    |_| true,
                )
                .unwrap(),
                "checkpoint presence does not establish Thread success"
            );
            assert_eq!(
                health,
                LaneHealth::Ready,
                "resolved old mismatch is not failure"
            );
            assert_eq!(issue_134_thread_row(&fixture), before);
            assert_eq!(fixture.recorder.generation().unwrap(), generation);
            assert_eq!(fixture.state_bytes(), publication);
            assert!(!cadence.checkpoint_recovery.pending);
            assert!(!cadence.begin_checkpoint_recovery(&fixture.epoch));
        }
        fixture.seed_state(false, true);
        let failed_before = issue_134_thread_row(&fixture);
        let failed_generation = fixture.recorder.generation().unwrap();
        let mut cadence = AdaptiveThreadCadence::default();
        let mut health = LaneHealth::Failed;
        assert!(!commit_thread_poll_result(
            delayed,
            &fixture.epoch,
            &mut fixture.recorder,
            LaneHealth::Ready,
            &mut health,
            &mut cadence,
            Instant::now(),
            |_| true,
        )
        .unwrap());
        assert_eq!(
            health,
            LaneHealth::Failed,
            "only a complete snapshot can recover"
        );
        assert_eq!(issue_134_thread_row(&fixture), failed_before);
        assert_eq!(fixture.recorder.generation().unwrap(), failed_generation);
        assert!(!cadence.checkpoint_recovery.pending);

        for physical_key in ["root", "path", "device", "inode", "mixed"] {
            fixture.seed_state(false, false);
            let payload = issue_134_thread_row(&fixture).0;
            let mut missing = source.clone();
            match physical_key {
                "root" => missing.root_identity.push_str("-different"),
                "path" | "mixed" => missing.relative_path = "missing.jsonl".to_owned(),
                "device" => missing.file_device += 1,
                "inode" => missing.file_inode += 1,
                _ => unreachable!(),
            }
            let sources = if physical_key == "mixed" {
                vec![source.clone(), missing]
            } else {
                vec![missing]
            };
            let mut cadence = AdaptiveThreadCadence::default();
            assert!(cadence.checkpoint_recovery.request(&fixture.epoch));
            assert!(cadence.begin_checkpoint_recovery(&fixture.epoch));
            let mut health = LaneHealth::Ready;
            assert!(!commit_thread_poll_result(
                ActiveThreadPollResult::CheckpointMismatch { sources },
                &fixture.epoch,
                &mut fixture.recorder,
                LaneHealth::Ready,
                &mut health,
                &mut cadence,
                Instant::now(),
                |_| true,
            )
            .unwrap());
            assert_eq!(health, LaneHealth::Failed, "{physical_key}");
            assert_eq!(
                issue_134_thread_row(&fixture),
                (payload, 1),
                "{physical_key}"
            );
            assert!(!cadence.checkpoint_recovery.pending);
            assert!(!cadence.begin_checkpoint_recovery(&fixture.epoch));
        }
        fixture.cleanup();
    }

    #[test]
    fn issue_134_checkpoint_reconciliation_recovers_on_original_deadline() {
        use std::os::unix::fs::MetadataExt;
        let mut fixture = issue_362_fixture("checkpoint-reconcile-success");
        fixture.seed_state(false, false);
        fixture.recorder.run_cycle_with_quota(None).unwrap();
        assert_eq!(fixture.recorder.state().unwrap().cycle_seq, 1);
        let root = &fixture.options.sessions_root;
        let noop = b"{\"type\":\"response_item\",\"timestamp\":\"2026-10-17T00:00:00Z\",\"payload\":{\"type\":\"message\"}}\n";
        fs::write(root.join("a-backlog.jsonl"), noop.repeat(3000)).unwrap();
        let active = root.join("b-active.jsonl");
        fs::write(&active, noop).unwrap();
        let root_metadata = fs::metadata(root).unwrap();
        let metadata = fs::metadata(&active).unwrap();
        let missing = codex_info_db_writer::RecordedSessionSource {
            root_identity: format!("unix:{}:{}", root_metadata.dev(), root_metadata.ino()),
            relative_path: "b-active.jsonl".to_owned(),
            file_device: metadata.dev(),
            file_inode: metadata.ino(),
            file_bytes: metadata.len(),
            modified_nanos: metadata
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        };
        let before = issue_134_thread_row(&fixture);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut cadence = AdaptiveThreadCadence::default();
        let mut health = LaneHealth::Ready;
        issue_134_drive_checkpoint_cycle(
            &mut fixture,
            &mut cadence,
            &mut health,
            &mut schedule,
            &now,
            Some(ActiveThreadPollResult::CheckpointMismatch {
                sources: vec![missing.clone()],
            }),
            true,
        );
        assert_eq!(now.get(), anchor);
        assert_eq!(
            issue_134_thread_row(&fixture),
            before,
            "pending admission must not publish error or partial threads"
        );
        assert!(cadence.begin_checkpoint_recovery(&fixture.epoch));
        // The production loop runs replaced-prefix repair before collection.
        // A missing active source owns this same cycle budget first. No old
        // prefix input may be touched until that guarded collection runs.
        let held_root = fixture.options.sessions_root.with_extension("held");
        fs::rename(&fixture.options.sessions_root, &held_root).unwrap();
        let deferred = fixture.recorder.repair_replaced_sources_guarded(|| true);
        fs::rename(&held_root, &fixture.options.sessions_root).unwrap();
        assert!(
            matches!(deferred, Ok(None)),
            "unrelated prefix repair must defer without scanning or spending the recovery budget"
        );
        fixture
            .recorder
            .run_cycle_with_quota_guarded(None, || true)
            .expect("one guarded collection");
        assert!(
            fixture
                .recorder
                .state()
                .unwrap()
                .checkpoints
                .iter()
                .any(|checkpoint| {
                    checkpoint.relative_path == missing.relative_path
                        && checkpoint.file_device == missing.file_device
                        && checkpoint.file_inode == missing.file_inode
                        && checkpoint.committed_offset == noop.len() as u64
                }),
            "main must pass the exact missing source into its one existing guarded recollection"
        );
        assert_eq!(fixture.options.chunk_bytes, 4096);
        let complete = fixture.poll_result("verified new Session", 2);
        issue_134_drive_checkpoint_cycle(
            &mut fixture,
            &mut cadence,
            &mut health,
            &mut schedule,
            &now,
            Some(complete),
            false,
        );
        let after = issue_134_thread_row(&fixture);
        assert_ne!(after.0, before.0);
        assert_eq!(after.1, 0);
        assert_eq!(health, LaneHealth::Ready);
        assert_eq!(now.get(), anchor + Duration::from_secs(60));
        assert_eq!(
            schedule.time_until_deadline(now.get()),
            Duration::from_secs(60)
        );
        assert!(!cadence.checkpoint_recovery.attempted);
        fixture.cleanup();
    }

    #[test]
    fn issue_134_repeated_checkpoint_mismatch_marks_error_without_second_recollection() {
        let mut fixture = issue_362_fixture("checkpoint-reconcile-repeated");
        fixture.seed_state(false, false);
        let before = issue_134_thread_row(&fixture).0;
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut cadence = AdaptiveThreadCadence::default();
        let mut health = LaneHealth::Ready;
        issue_134_drive_checkpoint_cycle(
            &mut fixture,
            &mut cadence,
            &mut health,
            &mut schedule,
            &now,
            Some(ActiveThreadPollResult::CheckpointMismatch {
                sources: Vec::new(),
            }),
            true,
        );
        assert!(cadence.begin_checkpoint_recovery(&fixture.epoch));
        issue_134_drive_checkpoint_cycle(
            &mut fixture,
            &mut cadence,
            &mut health,
            &mut schedule,
            &now,
            Some(ActiveThreadPollResult::CheckpointMismatch {
                sources: Vec::new(),
            }),
            true,
        );
        assert_eq!(now.get(), anchor + Duration::from_secs(60));
        assert_eq!(issue_134_thread_row(&fixture), (before, 1));
        assert_eq!(health, LaneHealth::Failed);
        assert!(!cadence.checkpoint_recovery.pending);
        assert!(!cadence.begin_checkpoint_recovery(&fixture.epoch));
        fixture.cleanup();
    }

    #[test]
    fn issue_134_adaptive_checkpoint_reconciliation_retains_activity_and_deadline() {
        let mut fixture = issue_362_fixture("checkpoint-adaptive-reconcile");
        fixture.seed_state(false, false);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut cadence = AdaptiveThreadCadence::default();
        cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(1)), anchor);
        cadence.accept(&fixture.epoch, &adaptive_thread_snapshot(Some(2)), anchor);
        let mut health = LaneHealth::Ready;
        issue_134_drive_checkpoint_cycle(
            &mut fixture,
            &mut cadence,
            &mut health,
            &mut schedule,
            &now,
            Some(ActiveThreadPollResult::CheckpointMismatch {
                sources: Vec::new(),
            }),
            false,
        );
        assert_eq!(now.get(), anchor + Duration::from_secs(6));
        assert_eq!(cadence.last_delta, Some(anchor));
        assert!(cadence.begin_checkpoint_recovery(&fixture.epoch));
        assert_eq!(
            cadence.next_probe, None,
            "recollection must immediately probe the fresh checkpoint set"
        );
        assert_eq!(cadence.last_delta, Some(anchor));
        let complete = ActiveThreadPollResult::Snapshot {
            snapshot: adaptive_thread_snapshot(Some(2)),
            epoch: fixture.epoch.clone(),
        };
        issue_134_drive_checkpoint_cycle(
            &mut fixture,
            &mut cadence,
            &mut health,
            &mut schedule,
            &now,
            Some(complete),
            false,
        );
        assert_eq!(now.get(), anchor + Duration::from_secs(60));
        assert_eq!(issue_134_thread_row(&fixture).1, 0);
        fixture.cleanup();
    }

    #[test]
    fn issue_134_checkpoint_reconciliation_does_not_clear_existing_error() {
        let mut fixture = issue_362_fixture("checkpoint-reconcile-existing-error");
        fixture.seed_state(false, true);
        let before = issue_134_thread_row(&fixture);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut cadence = AdaptiveThreadCadence::default();
        let mut health = LaneHealth::Failed;
        issue_134_drive_checkpoint_cycle(
            &mut fixture,
            &mut cadence,
            &mut health,
            &mut schedule,
            &now,
            Some(ActiveThreadPollResult::CheckpointMismatch {
                sources: Vec::new(),
            }),
            true,
        );
        assert_eq!(issue_134_thread_row(&fixture), before);
        assert_eq!(health, LaneHealth::Failed);
        fixture.cleanup();
    }

    #[test]
    fn issue_134_checkpoint_recollection_failure_waits_for_normal_collection() {
        let mut fixture = issue_362_fixture("checkpoint-recollect-failed");
        fixture.seed_state(false, false);
        let before = issue_134_thread_row(&fixture).0;
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut cadence = AdaptiveThreadCadence::default();
        let mut health = LaneHealth::Ready;
        issue_134_drive_checkpoint_cycle(
            &mut fixture,
            &mut cadence,
            &mut health,
            &mut schedule,
            &now,
            Some(ActiveThreadPollResult::CheckpointMismatch {
                sources: Vec::new(),
            }),
            true,
        );
        assert!(cadence.begin_checkpoint_recovery(&fixture.epoch));
        reject_checkpoint_recollection(
            &mut fixture.recorder,
            LaneHealth::Ready,
            &mut health,
            &mut cadence,
            anchor + Duration::from_secs(60),
        );
        assert_eq!(issue_134_thread_row(&fixture), (before, 1));
        assert!(!cadence.submit(anchor, &[], &mut |_| panic!(
            "failed inventory must not be probed before normal collection"
        )));
        assert!(!cadence.checkpoint_recovery.pending);
        fixture.cleanup();
    }

    #[test]
    fn issue_134_checkpoint_reconciliation_rejects_changed_epoch() {
        let fixture = issue_362_fixture("checkpoint-reconcile-epoch");
        let mut cadence = AdaptiveThreadCadence::default();
        assert!(cadence.checkpoint_recovery.request(&fixture.epoch));
        fs::write(
            fixture.options.codex_home.join("auth.json"),
            br#"{"tokens":{"account_id":"changed-account","access_token":"fixture"}}"#,
        )
        .unwrap();
        let changed = AccountEpochProof::capture(&fixture.options.codex_home).unwrap();
        assert_ne!(fixture.epoch, changed);
        assert!(!cadence.begin_checkpoint_recovery(&changed));
        assert!(!cadence.checkpoint_recovery.pending);
        fixture.cleanup();
    }

    #[test]
    fn issue_134_once_mode_cannot_leave_checkpoint_reconciliation_pending() {
        let mut fixture = issue_362_fixture("checkpoint-once");
        fixture.options.once = true;
        fixture.seed_state(false, false);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        issue_134_drive_checkpoint_cycle(
            &mut fixture,
            &mut AdaptiveThreadCadence::default(),
            &mut LaneHealth::Ready,
            &mut FixedRateSchedule::anchored(anchor, Duration::from_secs(60)),
            &now,
            Some(ActiveThreadPollResult::CheckpointMismatch {
                sources: Vec::new(),
            }),
            true,
        );
        assert_eq!(issue_134_thread_row(&fixture).1, 1);
        fixture.cleanup();
    }

    #[test]
    fn issue_134_checkpoint_mismatch_requests_collection_before_deadline() {
        let mut fixture = issue_362_fixture("checkpoint-recovery-deadline");
        fixture.seed_state(false, false);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        let mut submits = 0;
        let mut drains = 0;
        let mut waits = 0;
        assert!(issue_362_drive_cycle(
            &mut fixture,
            CyclePublication::Committed { has_pending: false },
            LaneHealth::Ready,
            &mut schedule,
            &now,
            Some(ActiveThreadPollResult::CheckpointMismatch {
                sources: Vec::new()
            }),
            vec![true; 8],
            None,
            &mut submits,
            &mut drains,
            &mut waits,
        )
        .expect("checkpoint reconciliation requested"));
        assert_eq!(
            now.get(), anchor + Duration::from_secs(1),
            "a newly active Session must trigger guarded collection without waiting for the periodic deadline"
        );
        fixture.cleanup();
    }

    #[test]
    fn issue_134_initial_checkpoint_reconciliation_keeps_last_good_health() {
        let mut fixture = issue_362_fixture("checkpoint-pending-health");
        fixture.seed_state(false, false);
        let anchor = Instant::now();
        let now = Cell::new(anchor);
        let mut schedule = FixedRateSchedule::anchored(anchor, Duration::from_secs(60));
        issue_362_drive_cycle(
            &mut fixture,
            CyclePublication::Committed { has_pending: false },
            LaneHealth::Ready,
            &mut schedule,
            &now,
            Some(ActiveThreadPollResult::CheckpointMismatch {
                sources: Vec::new(),
            }),
            vec![true; 8],
            None,
            &mut 0,
            &mut 0,
            &mut 0,
        )
        .expect("pending checkpoint reconciliation");
        let health: i64 = rusqlite::Connection::open(&fixture.options.database)
            .unwrap()
            .query_row(
                "SELECT acquisition_degraded FROM active_thread_snapshot WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(health, 0, "first checkpoint discovery must retain the verified health until guarded reconciliation finishes");
        fixture.cleanup();
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

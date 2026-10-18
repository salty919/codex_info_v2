//! A single outstanding historical job; recording never waits for this lane.

use super::*;

pub struct BackgroundWork<T> {
    requests: SyncSender<()>,
    results: Receiver<T>,
    in_flight: bool,
    unavailable: bool,
    _thread: JoinHandle<()>,
}

impl<T: Send + 'static> BackgroundWork<T> {
    pub fn start<F>(name: &str, mut work: F) -> Result<Self, RecorderError>
    where
        F: FnMut() -> T + Send + 'static,
    {
        let (requests, receiver) = mpsc::sync_channel(1);
        let (sender, results) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                while receiver.recv().is_ok() {
                    if sender.send(work()).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            results,
            in_flight: false,
            unavailable: false,
            _thread: thread,
        })
    }

    pub fn request(&mut self) -> bool {
        if self.in_flight || self.unavailable {
            return false;
        }
        if self.requests.try_send(()).is_err() {
            self.unavailable = true;
            return false;
        }
        self.in_flight = true;
        true
    }

    pub fn try_result(&mut self) -> Option<T> {
        match self.results.try_recv() {
            Ok(result) => {
                self.in_flight = false;
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.in_flight = false;
                self.unavailable = true;
                None
            }
        }
    }

    pub fn is_in_flight(&self) -> bool {
        self.in_flight
    }

    pub fn is_unavailable(&self) -> bool {
        self.unavailable
    }
}

/// Source bytes are read and hashed in the background. The live writer
/// receives an opaque completed candidate with the exact file fingerprints.
pub struct PreparedHistoricalRepair {
    pub(super) batch: HistoricalRepairBatch,
    pub(super) sources: Vec<(Source, SourceFingerprint)>,
}

impl Recorder {
    pub fn start_historical_repair_worker(
        &mut self,
    ) -> Result<
        BackgroundWork<Result<Option<PreparedHistoricalRepair>, RecorderError>>,
        RecorderError,
    > {
        // A failed auxiliary start must not silently restore expensive
        // historical scans to the foreground recording path.
        self.historical_preparation_owned = true;
        let config = self.config.clone();
        let database = self.database.clone().ok_or_else(|| {
            RecorderError::Invalid("historical worker requires a partition".into())
        })?;
        let identity = self.identity.clone().ok_or_else(|| {
            RecorderError::Invalid("historical worker requires an identity".into())
        })?;
        let activation = self.activation_timestamp;
        let intervals = self.lifecycle_intervals.clone();
        let boundary = self.account_boundary_epoch;
        let mut historical = None;
        BackgroundWork::start("codex-info-history-prepare", move || {
            let began = Instant::now();
            eprintln!("codex-info-recorder diagnostic history_prepare start");
            let result = (|| {
                if account_boundary_changed() {
                    return Err(RecorderError::AccountBoundaryChanged);
                }
                if historical.is_none() {
                    // This peer has no write connection and never acquires or
                    // releases the live recorder's exclusive writer lease.
                    let reader = UsageStore::open_read_only_partitioned(&database, &identity)?;
                    let mut prepared = Recorder::open_with_writer(config.clone(), reader)?;
                    prepared.database = Some(database.clone());
                    prepared.identity = Some(identity.clone());
                    prepared.activation_timestamp = activation;
                    prepared.lifecycle_intervals = intervals.clone();
                    prepared.account_boundary_epoch = boundary;
                    historical = Some(prepared);
                }
                historical
                    .as_mut()
                    .expect("initialized")
                    .prepare_saved_sources(true)
                    .map(|prepared| prepared.filter(|value| !value.batch.repairs.is_empty()))
            })();
            let (status, repairs) = match &result {
                Ok(Some(prepared)) => ("prepared", prepared.batch.repairs.len()),
                Ok(None) => ("unchanged", 0),
                Err(_) => ("error", 0),
            };
            eprintln!("codex-info-recorder diagnostic history_prepare finish elapsed_ms={} status={status} repairs={repairs}", began.elapsed().as_millis());
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn issue604_disconnected_history_worker_reports_unavailable() {
        let mut worker = BackgroundWork::<()>::start("test-history-disconnect", || {
            panic!("controlled historical worker failure");
        })
        .unwrap();
        assert!(worker.request());
        assert!(matches!(
            worker.results.recv_timeout(Duration::from_secs(5)),
            Err(RecvTimeoutError::Disconnected)
        ));
        assert!(worker.try_result().is_none());
        assert!(
            worker.is_unavailable(),
            "terminated historical work must be explicit, not permanently pending"
        );
        assert!(!worker.in_flight);
        assert!(!worker.request());
    }

    #[test]
    fn issue604_blocked_historical_work_does_not_hold_usage_or_quota_recording() {
        let (root, database, source, mut recorder, at) =
            crate::tests::issue604_parser_state_fixture("async-historical-recording");
        let owner = std::thread::current().id();
        let (began, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let mut worker = BackgroundWork::start("test-historical", move || {
            began.send(std::thread::current().id()).unwrap();
            released.recv().unwrap();
            7_u64
        })
        .expect("historical work needs a separate nonblocking owner");
        assert!(worker.request());
        assert_ne!(started.recv_timeout(Duration::from_secs(5)).unwrap(), owner);
        assert!(
            !worker.request(),
            "one unfinished historical job must not accumulate retries"
        );
        assert!(worker.try_result().is_none());
        OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(crate::tests::token(15, at + 2).as_bytes())
            .unwrap();
        let before = recorder.generation().unwrap();
        recorder
            .run_cycle()
            .unwrap()
            .expect("usage must commit while historical work is suspended");
        assert!(recorder.generation().unwrap() > before);
        assert_eq!(recorder.model_totals().unwrap()["gpt-6.1-sol"].total, 15);
        let state = recorder.state().unwrap();
        let receipt = codex_info_db_writer::QuotaReceipt {
            receipt_id: "f".repeat(64),
            sequence: 1,
            observed_at: at + 2,
            reset_at: state.reset_at,
            window_seconds: state.window_seconds,
            remaining_percent: 80.0,
        };
        assert!(recorder
            .writer
            .commit_quota_receipts(&[receipt], false)
            .unwrap()
            .is_some());
        let db = Connection::open(&database).unwrap();
        assert_eq!(
            db.query_row("SELECT remaining_percent FROM quota_receipts", [], |r| r
                .get::<_, f64>(0))
                .unwrap(),
            80.0
        );
        assert!(
            worker.try_result().is_none(),
            "recording did not require historical completion"
        );
        release.send(()).unwrap();
        drop(db);
        drop(recorder);
        drop(worker);
        fs::remove_dir_all(root).unwrap();
    }
}

//! Recover historical quota observations from already attributed Session bytes.

use super::*;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

type SourceKey = (String, i64, i64, String);
type ObservationKey = (i64, i64);

impl UsageStore {
    /// Fill missing historical percentages from exact, account-attributed
    /// Session records. This does not create current-quota receipts or change
    /// model facts. Unavailable originals remain missing and can be retried.
    pub fn recover_session_quota_history(
        &mut self,
        sessions_root: &Path,
        since: i64,
    ) -> Result<usize> {
        let mut sources = BTreeMap::<SourceKey, BTreeSet<ObservationKey>>::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT e.relative_path,e.range_start,e.range_end,e.record_sha256,e.timestamp,h.reset_at
                 FROM session_events e JOIN usage_history h
                   ON h.timestamp=e.timestamp-e.timestamp%60
                 WHERE h.timestamp>=?1 AND h.remaining_percent IS NULL",
            )?;
            let mut rows = statement.query([since])?;
            while let Some(row) = rows.next()? {
                sources
                    .entry((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    .or_default()
                    .insert((row.get(4)?, row.get(5)?));
            }
        }
        if sources.is_empty() {
            return Ok(0);
        }
        let root = fs::canonicalize(sessions_root)?;
        let mut observations = BTreeMap::<ObservationKey, (i64, Option<f64>)>::new();
        for ((relative_path, start, end, expected_hash), admitted) in sources {
            let Ok(path) = fs::canonicalize(root.join(relative_path)) else {
                continue;
            };
            if !path.starts_with(&root) {
                continue;
            }
            let read = || -> std::io::Result<_> {
                let mut file = fs::File::open(path)?;
                file.seek(SeekFrom::Start(start as u64))?;
                let mut reader = BufReader::new(file.take((end - start) as u64));
                let mut digest = Sha256::new();
                let mut line = Vec::new();
                let mut candidates = Vec::new();
                while reader.read_until(b'\n', &mut line)? != 0 {
                    digest.update(&line);
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) {
                        if let Some(candidate) = session_quota(&value, &admitted) {
                            candidates.push(candidate);
                        }
                    }
                    line.clear();
                }
                // Recorder's sha256_file_range uses hex_digest on the first
                // digest; that helper hashes once more before hex encoding.
                let actual_hash = format!("{:x}", Sha256::digest(digest.finalize()));
                Ok((actual_hash, candidates))
            };
            let Ok((actual_hash, candidates)) = read() else {
                continue;
            };
            if actual_hash != expected_hash {
                continue;
            }
            for (key, observed_at, remaining) in candidates {
                let candidate = observations
                    .entry(key)
                    .or_insert((observed_at, Some(remaining)));
                if observed_at > candidate.0 {
                    *candidate = (observed_at, Some(remaining));
                } else if observed_at == candidate.0 && candidate.1 != Some(remaining) {
                    candidate.1 = None;
                }
            }
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut restored = 0;
        for ((reset_at, timestamp), (_, remaining)) in observations {
            let Some(remaining) = remaining else {
                continue;
            };
            let changed = tx.execute(
                "UPDATE usage_history SET remaining_percent=?1
                 WHERE reset_at=?2 AND timestamp=?3 AND remaining_percent IS NULL",
                params![remaining, reset_at, timestamp],
            )?;
            if changed == 0 {
                continue;
            }
            tx.execute(
                "UPDATE durable_state SET snapshot_json=json_set(snapshot_json,'$.remaining_percent',?1)
                 WHERE singleton>=2 AND data_generation=?3
                   AND json_extract(snapshot_json,'$.reset_at')=?2
                   AND json_extract(snapshot_json,'$.remaining_percent') IS NULL",
                params![remaining, reset_at, timestamp],
            )?;
            restored += changed;
        }
        if restored > 0 {
            let generation: String = tx.query_row(
                "SELECT data_generation FROM collection_generation WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            let next = canonical_u64_text(&generation, "collection generation")?
                .checked_add(1)
                .ok_or(UsageStoreError::GenerationOverflow)?;
            tx.execute(
                "UPDATE collection_generation SET data_generation=?1 WHERE singleton=1",
                [next.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(restored)
    }
}

fn session_quota(
    value: &serde_json::Value,
    admitted: &BTreeSet<ObservationKey>,
) -> Option<(ObservationKey, i64, f64)> {
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let observed_at = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?).ok()?;
    let limits = payload.get("rate_limits")?;
    for name in ["secondary", "primary"] {
        let Some(window) = limits.get(name) else {
            continue;
        };
        let Some(reset_at) = window.get("resets_at").and_then(serde_json::Value::as_i64) else {
            continue;
        };
        if !admitted.contains(&(observed_at.timestamp(), reset_at)) {
            continue;
        }
        let used = window.get("used_percent")?.as_f64()?;
        if !used.is_finite() || !(0.0..=100.0).contains(&used) {
            return None;
        }
        return Some((
            (
                reset_at,
                observed_at.timestamp() - observed_at.timestamp().rem_euclid(60),
            ),
            observed_at.timestamp_millis(),
            100.0 - used,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    const MINUTE: i64 = 1_700_000_040;
    const RESET: i64 = 1_700_604_800;

    fn fixture() -> (PathBuf, UsageStore) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "session-quota-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "a".repeat(32),
            account_scope_id: "b".repeat(64),
            storage_epoch: 1,
            partition_id: "c".repeat(64),
        };
        let store =
            UsageStore::create_partitioned(root.join("history.sqlite3"), &identity).unwrap();
        (root, store)
    }

    fn add_history(store: &mut UsageStore, minute: i64, remaining: Option<f64>) {
        let sample = UsageHistorySample {
            timestamp: minute,
            reset_at: RESET,
            remaining_percent: remaining,
            sol_dollars: 2.0,
            terra_dollars: 0.0,
            luna_dollars: 0.0,
            sol_tokens: 100,
            terra_tokens: 0,
            luna_tokens: 0,
        };
        store.upsert_sample(&sample).unwrap();
        let tx = store.connection.transaction().unwrap();
        upsert_observations(&tx, &[UsageHistoryObservation::confirmed(&sample)]).unwrap();
        tx.commit().unwrap();
    }

    fn add_source(store: &UsageStore, root: &Path, name: &str, time: i64, reset: i64, used: f64) {
        let body = format!(
            "{}\n",
            json!({
                "type": "event_msg", "timestamp": DateTime::from_timestamp(time, 0).unwrap().to_rfc3339(),
                "payload": {"type":"token_count", "rate_limits": {
                    "secondary": {"used_percent": used, "resets_at": reset, "window_minutes": 10080}
                }}
            })
        );
        fs::write(root.join(name), &body).unwrap();
        store.connection.execute(
            "INSERT INTO session_events(root_identity,relative_path,file_device,file_inode,prefix_generation,
             range_start,range_end,record_sha256,event_index,timestamp,model,total_tokens,input_tokens,cached_input_tokens,output_tokens)
             VALUES('root',?1,'1','2',?2,0,?3,?4,0,?5,'SOL','1','1','0','0')",
            params![name, "a".repeat(32), body.len(), format!("{:x}", Sha256::digest(Sha256::digest(body.as_bytes()))), time],
        ).unwrap();
    }

    #[test]
    fn session_quota_recovery_fills_only_missing_exact_minutes_and_survives_replay() {
        let (root, mut store) = fixture();
        add_history(&mut store, MINUTE, None);
        add_history(&mut store, MINUTE + 60, Some(70.0));
        add_history(&mut store, MINUTE + 120, None);
        add_source(&store, &root, "first.jsonl", MINUTE + 10, RESET, 10.0);
        add_source(&store, &root, "last.jsonl", MINUTE + 50, RESET, 12.0);
        add_source(&store, &root, "existing.jsonl", MINUTE + 70, RESET, 20.0);
        add_source(
            &store,
            &root,
            "other-period.jsonl",
            MINUTE + 130,
            RESET + 600,
            1.0,
        );
        let before = store.load_all().unwrap();
        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            1
        );
        let mut expected = before;
        expected[0].remaining_percent = Some(88.0);
        assert_eq!(store.load_all().unwrap(), expected);
        assert_eq!(
            store
                .load_recent_observations(Utc.timestamp_opt(MINUTE + 180, 0).unwrap())
                .unwrap()[0]
                .remaining_percent,
            Some(88.0)
        );
        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            0
        );
        let mut replay = expected[0].clone();
        replay.remaining_percent = None;
        store.upsert_sample(&replay).unwrap();
        assert_eq!(store.load_all().unwrap(), expected);
        assert!(store.latest_quota_receipt().unwrap().is_none());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_quota_recovery_requires_original_bytes_and_rolls_back_on_save_failure() {
        let (root, mut store) = fixture();
        add_history(&mut store, MINUTE, None);
        add_source(&store, &root, "source.jsonl", MINUTE + 10, RESET, 10.0);
        let original = fs::read(root.join("source.jsonl")).unwrap();
        fs::write(root.join("source.jsonl"), b"changed source\n").unwrap();
        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            0
        );
        fs::write(root.join("source.jsonl"), original).unwrap();
        store.connection.execute_batch("CREATE TRIGGER fail_recovery BEFORE UPDATE ON durable_state BEGIN SELECT RAISE(ABORT,'save failure'); END;").unwrap();
        assert!(store.recover_session_quota_history(&root, MINUTE).is_err());
        assert_eq!(store.load_all().unwrap()[0].remaining_percent, None);
        store
            .connection
            .execute_batch("DROP TRIGGER fail_recovery;")
            .unwrap();
        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            1
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

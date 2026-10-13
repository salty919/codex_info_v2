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
        let known_resets: BTreeSet<i64> = self.connection.prepare(
            "SELECT reset_at FROM usage_history
             UNION SELECT reset_at FROM collection_generation WHERE reset_at>0
             UNION SELECT latest_quota_reset_at FROM collection_generation WHERE latest_quota_reset_at>0
             UNION SELECT reset_at FROM quota_receipts
             UNION SELECT json_extract(snapshot_json,'$.reset_at') FROM durable_state WHERE singleton>=2",
        )?.query_map([], |row| row.get(0))?.collect::<std::result::Result<_, _>>()?;
        let mut sources = BTreeMap::<SourceKey, BTreeMap<i64, Option<i64>>>::new();
        {
            let mut statement = self.connection.prepare(
                "SELECT e.relative_path,e.range_start,e.range_end,e.record_sha256,e.timestamp,COALESCE(h.reset_at,json_extract(d.snapshot_json,'$.reset_at'))
                 FROM session_events e LEFT JOIN usage_history h
                   ON h.timestamp=e.timestamp-e.timestamp%60
                 LEFT JOIN durable_state d ON d.singleton>=2 AND d.data_generation=e.timestamp-e.timestamp%60
                 WHERE e.timestamp-e.timestamp%60>=?1 AND h.remaining_percent IS NULL
                   AND json_extract(d.snapshot_json,'$.remaining_percent') IS NULL",
            )?;
            let mut rows = statement.query([since])?;
            while let Some(row) = rows.next()? {
                sources
                    .entry((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    .or_default()
                    .insert(row.get(4)?, row.get(5)?);
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
                        if let Some(candidate) = session_quota(&value, &admitted, &known_resets) {
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
        // Different exact resets in one missing minute cannot both own its
        // canonical row. Leave that minute unknown instead of choosing an ID.
        let mut minute_counts = BTreeMap::<i64, usize>::new();
        for (_, timestamp) in observations.keys() {
            *minute_counts.entry(*timestamp).or_default() += 1;
        }
        for ((reset_at, timestamp), (_, remaining)) in observations {
            let Some(remaining) = remaining else {
                continue;
            };
            if minute_counts[&timestamp] > 1 {
                continue;
            }
            let mut changed = tx.execute(
                "UPDATE usage_history SET remaining_percent=?1
                 WHERE reset_at=?2 AND timestamp=?3 AND remaining_percent IS NULL",
                params![remaining, reset_at, timestamp],
            )?;
            if changed == 0 {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM usage_history WHERE timestamp=?1)",
                    [timestamp],
                    |row| row.get(0),
                )?;
                if exists {
                    continue;
                }
                changed = 1;
                upsert_observations_with_sources(
                    &tx,
                    &[UsageHistoryObservation::unavailable(
                        timestamp,
                        reset_at,
                        Some(remaining),
                    )],
                    &BTreeMap::new(),
                )?;
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
    admitted: &BTreeMap<i64, Option<i64>>,
    known_resets: &BTreeSet<i64>,
) -> Option<(ObservationKey, i64, f64)> {
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let observed_at = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?).ok()?;
    let limits = payload.get("rate_limits")?;
    let mut selected = None;
    for name in ["secondary", "primary"] {
        let Some(window) = limits.get(name) else {
            continue;
        };
        let Some(reset_at) = window.get("resets_at").and_then(serde_json::Value::as_i64) else {
            continue;
        };
        let Some(existing_reset) = admitted.get(&observed_at.timestamp()) else {
            continue;
        };
        if reset_at < observed_at.timestamp()
            || existing_reset.is_some_and(|existing| existing != reset_at)
            || (existing_reset.is_none() && !known_resets.contains(&reset_at))
        {
            continue;
        }
        let used = window.get("used_percent")?.as_f64()?;
        if !used.is_finite() || !(0.0..=100.0).contains(&used) {
            return None;
        }
        let candidate = (
            (
                reset_at,
                observed_at.timestamp() - observed_at.timestamp().rem_euclid(60),
            ),
            observed_at.timestamp_millis(),
            100.0 - used,
        );
        if selected.is_some_and(|previous| previous != candidate) {
            return None;
        }
        selected = Some(candidate);
    }
    selected
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
    fn session_quota_recovery_restores_missing_rows_only_for_proven_exact_periods() {
        let (root, mut store) = fixture();
        add_history(&mut store, MINUTE, Some(90.0));
        add_source(&store, &root, "missing.jsonl", MINUTE + 70, RESET, 12.0);
        add_source(&store, &root, "existing.jsonl", MINUTE + 10, RESET, 99.0);
        add_source(
            &store,
            &root,
            "different.jsonl",
            MINUTE + 130,
            RESET + 1,
            1.0,
        );
        add_source(&store, &root, "changed.jsonl", MINUTE + 190, RESET, 14.0);
        fs::write(root.join("changed.jsonl"), b"changed original\n").unwrap();
        let original = store.load_all().unwrap();

        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            1
        );
        let rows = store.load_all().unwrap();
        assert_eq!(rows, original); // Quota-only observations never fabricate model rows.
        let observations = store
            .load_recent_observations(Utc.timestamp_opt(MINUTE + 240, 0).unwrap())
            .unwrap();
        let restored = observations
            .iter()
            .find(|row| row.timestamp == MINUTE + 60)
            .unwrap();
        assert_eq!(restored.model_source, ModelSource::Unavailable);
        assert_eq!(restored.model_totals, None);
        assert!(!restored.model_totals_complete);
        assert_eq!(restored.sol_tokens, None);
        assert_eq!(
            (restored.reset_at, restored.remaining_percent),
            (RESET, Some(88.0))
        );
        assert_eq!(observations.len(), 2);
        assert!(store.latest_quota_receipt().unwrap().is_none());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_quota_recovery_accepts_receipt_and_durable_only_known_resets() {
        for durable_only in [false, true] {
            let (root, mut store) = fixture();
            add_history(&mut store, MINUTE, Some(90.0));
            let reset = RESET + 600;
            if durable_only {
                let tx = store.connection.transaction().unwrap();
                upsert_observations(
                    &tx,
                    &[UsageHistoryObservation::unavailable(
                        MINUTE + 180,
                        reset,
                        Some(70.0),
                    )],
                )
                .unwrap();
                tx.commit().unwrap();
            } else {
                store
                    .connection
                    .execute(
                        "INSERT INTO quota_receipts VALUES('known',1,?1,?2,604800,70,'0')",
                        params![MINUTE + 180, reset],
                    )
                    .unwrap();
            }
            let receipt = store.latest_quota_receipt().unwrap();
            add_source(&store, &root, "known.jsonl", MINUTE + 70, reset, 12.0);
            assert_eq!(
                store.recover_session_quota_history(&root, MINUTE).unwrap(),
                1,
                "durable_only={durable_only}"
            );
            let observations = store
                .load_recent_observations(Utc.timestamp_opt(MINUTE + 240, 0).unwrap())
                .unwrap();
            let restored = observations
                .iter()
                .find(|row| row.timestamp == MINUTE + 60)
                .unwrap();
            assert_eq!(
                (restored.reset_at, restored.remaining_percent),
                (reset, Some(88.0))
            );
            assert_eq!(restored.model_source, ModelSource::Unavailable);
            assert_eq!(store.latest_quota_receipt().unwrap(), receipt);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn session_quota_recovery_rejects_two_known_resets_in_one_event() {
        let (root, mut store) = fixture();
        add_history(&mut store, MINUTE, Some(90.0));
        store
            .connection
            .execute(
                "UPDATE collection_generation SET reset_at=?1",
                [RESET + 600],
            )
            .unwrap();
        add_source(&store, &root, "ambiguous.jsonl", MINUTE + 70, RESET, 12.0);
        let path = root.join("ambiguous.jsonl");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["payload"]["rate_limits"]["primary"] =
            json!({"used_percent":20,"resets_at":RESET+600,"window_minutes":10080});
        let body = format!("{value}\n");
        fs::write(path, &body).unwrap();
        store
            .connection
            .execute(
                "UPDATE session_events SET range_end=?1,record_sha256=?2",
                params![
                    body.len(),
                    format!("{:x}", Sha256::digest(Sha256::digest(body.as_bytes())))
                ],
            )
            .unwrap();
        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            0
        );
        assert_eq!(
            store
                .load_recent_observations(Utc.timestamp_opt(MINUTE + 240, 0).unwrap())
                .unwrap()
                .len(),
            1
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_quota_recovery_missing_rows_are_atomic_and_idempotent() {
        let (root, mut store) = fixture();
        add_history(&mut store, MINUTE, Some(90.0));
        add_source(&store, &root, "missing.jsonl", MINUTE + 70, RESET, 12.0);
        let before = store.load_all().unwrap();
        store.connection.execute_batch("CREATE TRIGGER fail_missing_recovery BEFORE INSERT ON durable_state BEGIN SELECT RAISE(ABORT,'save failure'); END;").unwrap();
        assert!(store.recover_session_quota_history(&root, MINUTE).is_err());
        assert_eq!(store.load_all().unwrap(), before);
        store
            .connection
            .execute_batch("DROP TRIGGER fail_missing_recovery;")
            .unwrap();
        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            1
        );
        let restored = store.load_all().unwrap();
        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            0
        );
        assert_eq!(store.load_all().unwrap(), restored);
        drop(store);
        fs::remove_dir_all(root).unwrap();
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

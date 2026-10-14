//! Recover historical quota observations from already attributed Session bytes.

use super::*;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

type SourceKey = (String, String, i64, i64, String);
type ObservationKey = (i64, i64);
type QuotaCandidates = BTreeMap<ObservationKey, (i64, Option<f64>)>;
type EventSourceRanges = BTreeMap<(String, String), BTreeMap<i64, BTreeSet<(i64, i64, String)>>>;

pub(super) const WINDOW_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_quota_windows (
    reset_at INTEGER NOT NULL CHECK (reset_at > 0),
    window_seconds INTEGER NOT NULL CHECK (window_seconds > 0),
    observed_at INTEGER NOT NULL CHECK (observed_at > 0),
    root_identity TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    range_start INTEGER NOT NULL CHECK (range_start >= 0),
    range_end INTEGER NOT NULL CHECK (range_end > range_start),
    record_sha256 TEXT NOT NULL CHECK (
        length(record_sha256) = 64 AND record_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY (reset_at, window_seconds)
) WITHOUT ROWID;";

#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct RelocatedQuotaEvent {
    timestamp: i64,
    model: String,
    tokens: [u64; 4],
    cache_write_input_tokens: Option<u64>,
}

impl UsageStore {
    /// Fill missing historical percentages from exact, account-attributed
    /// Session records. This does not create current-quota receipts or change
    /// model facts. Unavailable originals remain missing and can be retried.
    pub fn recover_session_quota_history(
        &mut self,
        sessions_root: &Path,
        since: i64,
    ) -> Result<usize> {
        self.recover_session_quota_history_for_intervals(
            sessions_root,
            since,
            &[SessionLifecycleInterval::new(None, None)],
        )
    }

    pub fn recover_session_quota_history_for_intervals(
        &mut self,
        sessions_root: &Path,
        since: i64,
        lifecycle_intervals: &[SessionLifecycleInterval],
    ) -> Result<usize> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (restored, restored_windows) = recover_session_quota_history_in_transaction(
            &tx,
            sessions_root,
            since,
            lifecycle_intervals,
            None,
        )?;
        if restored > 0 || restored_windows > 0 {
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

pub(super) fn recover_session_quota_history_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    sessions_root: &Path,
    since: i64,
    lifecycle_intervals: &[SessionLifecycleInterval],
    committed_ranges: Option<&[SessionRange]>,
) -> Result<(usize, usize)> {
    let allowed_ranges = committed_ranges.map(|ranges| {
        ranges
            .iter()
            .map(|range| {
                (
                    range.root_identity.clone(),
                    range.relative_path.clone(),
                    range.start_offset as i64,
                    range.end_offset as i64,
                    range.record_sha256.clone(),
                )
            })
            .collect::<BTreeSet<SourceKey>>()
    });
    let intervals = canonical_lifecycle_intervals(lifecycle_intervals)?;
    let lifecycle_json = serde_json::Value::Array(intervals.iter().map(|interval|
            serde_json::json!({"start_at":interval.start_at,"end_at":interval.end_at})).collect()).to_string();
    let known_resets: BTreeSet<i64> = tx.prepare(
            "SELECT reset_at FROM usage_history
             UNION SELECT reset_at FROM collection_generation WHERE reset_at>0
             UNION SELECT latest_quota_reset_at FROM collection_generation WHERE latest_quota_reset_at>0
             UNION SELECT reset_at FROM quota_receipts
             UNION SELECT json_extract(snapshot_json,'$.reset_at') FROM durable_state WHERE singleton>=2",
        )?.query_map([], |row| row.get(0))?.collect::<std::result::Result<_, _>>()?;
    let mut known_windows = BTreeMap::<i64, BTreeSet<i64>>::new();
    let mut statement = tx.prepare(
            "SELECT reset_at,window_seconds FROM quota_receipts WHERE window_seconds>0
             UNION SELECT reset_at,window_seconds FROM collection_generation WHERE reset_at>0 AND window_seconds>0
             UNION SELECT latest_quota_reset_at,window_seconds FROM collection_generation WHERE latest_quota_reset_at>0 AND window_seconds>0",
        )?;
    for row in statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))? {
        let (reset, seconds) = row?;
        known_windows.entry(reset).or_default().insert(seconds);
    }
    drop(statement);
    let mut sources = BTreeMap::<SourceKey, BTreeMap<i64, BTreeSet<Option<i64>>>>::new();
    let mut accepted_ranges = BTreeSet::<SourceKey>::new();
    let mut minute_resets = BTreeMap::<i64, BTreeSet<Option<i64>>>::new();
    let mut event_sources = EventSourceRanges::new();
    {
        let mut statement = tx.prepare(
            "SELECT timestamp,reset_at FROM usage_history WHERE timestamp>=?1
             UNION SELECT data_generation,json_extract(snapshot_json,'$.reset_at')
             FROM durable_state WHERE singleton>=2 AND data_generation>=?1",
        )?;
        for row in statement.query_map([since], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })? {
            let (minute, reset) = row?;
            minute_resets.entry(minute).or_default().insert(reset);
        }
        let mut statement = tx.prepare(
            "SELECT root_identity,relative_path,timestamp,range_start,range_end,record_sha256
             FROM session_events WHERE timestamp-timestamp%60>=?1",
        )?;
        for row in statement.query_map([since], |row| {
            Ok((
                (row.get(0)?, row.get(1)?),
                row.get(2)?,
                (row.get(3)?, row.get(4)?, row.get(5)?),
            ))
        })? {
            let (path, at, range) = row?;
            event_sources
                .entry(path)
                .or_default()
                .entry(at)
                .or_default()
                .insert(range);
        }
        let mut statement = tx.prepare(
            "SELECT root_identity,relative_path,start_offset,end_offset,record_sha256 FROM session_ranges",
        )?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })? {
            let key: SourceKey = row?;
            if allowed_ranges
                .as_ref()
                .is_none_or(|allowed| allowed.contains(&key))
            {
                sources.entry(key.clone()).or_default();
                accepted_ranges.insert(key);
            }
        }
    }
    {
        let mut statement = tx.prepare(
                "SELECT e.relative_path,e.range_start,e.range_end,e.record_sha256,e.timestamp,COALESCE(h.reset_at,json_extract(d.snapshot_json,'$.reset_at')),e.root_identity
                 FROM session_events e LEFT JOIN usage_history h
                   ON h.timestamp=e.timestamp-e.timestamp%60
                 LEFT JOIN durable_state d ON d.singleton>=2 AND d.data_generation=e.timestamp-e.timestamp%60
                 WHERE e.timestamp-e.timestamp%60>=?1
                   AND EXISTS(SELECT 1 FROM json_each(?2) owned
                       WHERE (json_extract(owned.value,'$.start_at') IS NULL OR e.timestamp>=json_extract(owned.value,'$.start_at'))
                         AND (json_extract(owned.value,'$.end_at') IS NULL OR e.timestamp<json_extract(owned.value,'$.end_at')))
                   AND (
                   (h.remaining_percent IS NULL AND json_extract(d.snapshot_json,'$.remaining_percent') IS NULL)
                   OR (h.reset_at IS NOT NULL
                       AND NOT EXISTS(SELECT 1 FROM quota_receipts q WHERE q.reset_at=h.reset_at AND q.window_seconds>0)
                       AND NOT EXISTS(SELECT 1 FROM session_quota_windows w WHERE w.reset_at=h.reset_at
                           AND EXISTS(SELECT 1 FROM json_each(?2) owned
                               WHERE (json_extract(owned.value,'$.start_at') IS NULL OR w.observed_at>=json_extract(owned.value,'$.start_at'))
                                 AND (json_extract(owned.value,'$.end_at') IS NULL OR w.observed_at<json_extract(owned.value,'$.end_at'))))))",
            )?;
        let mut rows = statement.query(params![since, lifecycle_json])?;
        while let Some(row) = rows.next()? {
            let key: SourceKey = (
                row.get(6)?,
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
            );
            if allowed_ranges
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(&key))
            {
                continue;
            }
            sources
                .entry(key)
                .or_default()
                .entry(row.get(4)?)
                .or_default()
                .insert(row.get(5)?);
        }
    }
    if sources.is_empty() {
        return Ok((0, 0));
    }
    let root = fs::canonicalize(sessions_root)?;
    let mut observations = BTreeMap::<ObservationKey, (i64, Option<f64>)>::new();
    let mut windows = BTreeMap::new();
    let mut relocated_paths = BTreeSet::new();
    for (key, mut admitted) in sources {
        let accepted_range = accepted_ranges.contains(&key);
        let (root_identity, relative_path, start, end, expected_hash) = key;
        let previous_events = event_sources.get(&(root_identity.clone(), relative_path.clone()));
        let event_range = (start, end, expected_hash.clone());
        let Ok(path) = fs::canonicalize(root.join(&relative_path)) else {
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
                    // A token_count can report quota without changing any
                    // counters. Its accepted bytes, rather than a fabricated
                    // zero-usage event, bind this observation to its source.
                    if accepted_range {
                        if let Some(at) = value
                            .get("timestamp")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                            .map(|at| at.timestamp())
                            .filter(|at| {
                                *at > 0
                                    && at.div_euclid(60) * 60 >= since
                                    && lifecycle_contains(&intervals, *at)
                                    // A rewritten record already attributed to
                                    // another range still requires the existing
                                    // normalized-event relocation proof.
                                    && previous_events.and_then(|events| events.get(at))
                                        .is_none_or(|ranges| ranges.contains(&event_range))
                            })
                        {
                            let authorities = minute_resets
                                .get(&(at.div_euclid(60) * 60))
                                .cloned()
                                .unwrap_or_else(|| BTreeSet::from([None]));
                            admitted.entry(at).or_default().extend(authorities);
                        }
                    }
                    if let Some(candidate) =
                        session_quota(&value, &admitted, &known_resets, &known_windows)
                    {
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
            relocated_paths.insert(relative_path);
            continue;
        }
        for (key, observed_at, remaining, seconds) in candidates {
            retain_quota_candidate(&mut observations, key, observed_at, remaining);
            windows.entry((key.0, seconds)).or_insert((
                observed_at.div_euclid(1000),
                root_identity.clone(),
                relative_path.clone(),
                start,
                end,
                expected_hash.clone(),
            ));
        }
    }
    if committed_ranges.is_none() {
        recover_relocated_quota(
            tx,
            &root,
            since,
            &relocated_paths,
            &known_resets,
            &known_windows,
            &mut observations,
        )?;
    }
    observations.retain(|_, (at, _)| lifecycle_contains(&intervals, at.div_euclid(1000)));
    let mut restored = 0;
    let mut restored_windows = 0;
    let invalid_windows = {
        let mut statement =
            tx.prepare("SELECT reset_at,window_seconds,observed_at FROM session_quota_windows")?;
        let values = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        values
    };
    for (reset, seconds, at) in invalid_windows
        .into_iter()
        .filter(|_| committed_ranges.is_none())
    {
        if !lifecycle_contains(&intervals, at) {
            restored_windows += tx.execute(
                "DELETE FROM session_quota_windows WHERE reset_at=?1 AND window_seconds=?2",
                params![reset, seconds],
            )?;
        }
    }
    for ((reset, seconds), (at, root_identity, path, start, end, hash)) in windows {
        restored_windows += tx.execute(
            "INSERT OR IGNORE INTO session_quota_windows VALUES (?1,?2,?7,?8,?3,?4,?5,?6)",
            params![reset, seconds, path, start, end, hash, at, root_identity],
        )?;
    }
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
        let durable_remaining: Option<f64> = tx.query_row(
            "SELECT json_extract(snapshot_json,'$.remaining_percent') FROM durable_state
             WHERE singleton>=2 AND data_generation=?1 AND json_extract(snapshot_json,'$.reset_at')=?2",
            params![timestamp, reset_at], |row| row.get(0),
        ).optional()?.flatten();
        if durable_remaining.is_some() {
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
                tx,
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
    Ok((restored, restored_windows))
}

fn retain_quota_candidate(
    observations: &mut QuotaCandidates,
    key: ObservationKey,
    at: i64,
    remaining: f64,
) {
    let candidate = observations.entry(key).or_insert((at, Some(remaining)));
    if at > candidate.0 {
        *candidate = (at, Some(remaining));
    } else if at == candidate.0 && candidate.1 != Some(remaining) {
        candidate.1 = None;
    }
}

fn recover_relocated_quota(
    connection: &Connection,
    root: &Path,
    since: i64,
    paths: &BTreeSet<String>,
    known_resets: &BTreeSet<i64>,
    known_windows: &BTreeMap<i64, BTreeSet<i64>>,
    observations: &mut QuotaCandidates,
) -> Result<()> {
    for relative_path in paths {
        let mut events = BTreeSet::new();
        let mut authorities = BTreeMap::<i64, BTreeSet<Option<i64>>>::new();
        let mut roots = BTreeSet::new();
        let mut statement = connection.prepare(
            "SELECT e.root_identity,e.timestamp,e.model,e.total_tokens,e.input_tokens,e.cached_input_tokens,e.output_tokens,e.cache_write_input_tokens,
             COALESCE(h.reset_at,json_extract(d.snapshot_json,'$.reset_at'))
             FROM session_events e LEFT JOIN usage_history h ON h.timestamp=e.timestamp-e.timestamp%60
             LEFT JOIN durable_state d ON d.singleton>=2 AND d.data_generation=e.timestamp-e.timestamp%60
             WHERE e.relative_path=?1 AND e.timestamp-e.timestamp%60>=?2
               AND h.remaining_percent IS NULL AND json_extract(d.snapshot_json,'$.remaining_percent') IS NULL",
        )?;
        let mut rows = statement.query(params![relative_path, since])?;
        while let Some(row) = rows.next()? {
            let timestamp = row.get(1)?;
            roots.insert(row.get::<_, String>(0)?);
            authorities
                .entry(timestamp)
                .or_default()
                .insert(row.get(8)?);
            events.insert(RelocatedQuotaEvent {
                timestamp,
                model: row.get(2)?,
                tokens: [
                    canonical_u64_text(&row.get::<_, String>(3)?, "total_tokens")?,
                    canonical_u64_text(&row.get::<_, String>(4)?, "input_tokens")?,
                    canonical_u64_text(&row.get::<_, String>(5)?, "cached_input_tokens")?,
                    canonical_u64_text(&row.get::<_, String>(6)?, "output_tokens")?,
                ],
                cache_write_input_tokens: row
                    .get::<_, Option<String>>(7)?
                    .map(|value| canonical_u64_text(&value, "cache_write_input_tokens"))
                    .transpose()?,
            });
        }
        drop(rows);
        drop(statement);
        let Ok(path) = fs::canonicalize(root.join(relative_path)) else {
            continue;
        };
        if !path.starts_with(root) {
            continue;
        }
        for root_identity in roots {
            let mut statement = connection.prepare(
                "SELECT file_device,file_inode,prefix_generation,start_offset,end_offset,record_sha256
                 FROM session_ranges WHERE root_identity=?1 AND relative_path=?2
                 ORDER BY file_device,file_inode,prefix_generation,start_offset,end_offset",
            )?;
            let ranges = statement
                .query_map(params![root_identity, relative_path], |row| {
                    Ok((
                        (
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ),
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut previous = None;
            let mut model = None;
            for (identity, start, end, hash) in ranges {
                if start < 0 || end <= start {
                    continue;
                }
                if previous.as_ref() != Some(&(identity.clone(), start)) {
                    model = None;
                }
                let read = || -> std::io::Result<_> {
                    let mut file = fs::File::open(&path)?;
                    file.seek(SeekFrom::Start(start as u64))?;
                    let mut reader = BufReader::new(file.take((end - start) as u64));
                    let mut digest = Sha256::new();
                    let mut line = Vec::new();
                    let mut current_model = model.clone();
                    let mut candidates = Vec::new();
                    while reader.read_until(b'\n', &mut line)? != 0 {
                        digest.update(&line);
                        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) {
                            if value.get("type").and_then(serde_json::Value::as_str)
                                == Some("turn_context")
                            {
                                current_model = value
                                    .pointer("/payload/model")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned);
                            }
                            if let Some(event) =
                                relocated_quota_event(&value, current_model.as_deref())
                            {
                                if events.contains(&event) {
                                    let admitted = BTreeMap::from([(
                                        event.timestamp,
                                        authorities[&event.timestamp].clone(),
                                    )]);
                                    if let Some(candidate) = session_quota(
                                        &value,
                                        &admitted,
                                        known_resets,
                                        known_windows,
                                    ) {
                                        candidates.push(candidate);
                                    }
                                }
                            }
                        }
                        line.clear();
                    }
                    Ok((
                        format!("{:x}", Sha256::digest(digest.finalize())),
                        candidates,
                        current_model,
                    ))
                };
                match read() {
                    Ok((actual, candidates, current_model)) if actual == hash => {
                        model = current_model;
                        previous = Some((identity, end));
                        for (key, at, remaining, _) in candidates {
                            retain_quota_candidate(observations, key, at, remaining);
                        }
                    }
                    _ => {
                        model = None;
                        previous = None;
                    }
                }
            }
        }
    }
    Ok(())
}

fn relocated_quota_event(
    value: &serde_json::Value,
    model: Option<&str>,
) -> Option<RelocatedQuotaEvent> {
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let usage = payload.pointer("/info/last_token_usage")?;
    Some(RelocatedQuotaEvent {
        timestamp: DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
            .ok()?
            .timestamp(),
        model: model?.to_owned(),
        tokens: [
            usage.get("total_tokens")?.as_u64()?,
            usage.get("input_tokens")?.as_u64()?,
            usage.get("cached_input_tokens")?.as_u64()?,
            usage.get("output_tokens")?.as_u64()?,
        ],
        cache_write_input_tokens: usage
            .get("cache_write_input_tokens")
            .and_then(serde_json::Value::as_u64),
    })
}

fn session_quota(
    value: &serde_json::Value,
    admitted: &BTreeMap<i64, BTreeSet<Option<i64>>>,
    known_resets: &BTreeSet<i64>,
    known_windows: &BTreeMap<i64, BTreeSet<i64>>,
) -> Option<(ObservationKey, i64, f64, i64)> {
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let observed_at = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?).ok()?;
    let limits = payload.get("rate_limits")?;
    let longest_window = ["secondary", "primary"]
        .into_iter()
        .filter_map(|name| limits.get(name)?.get("window_minutes")?.as_i64())
        .filter(|minutes| *minutes > 0)
        .max()?;
    let mut selected = None;
    for name in ["secondary", "primary"] {
        let Some(window) = limits.get(name) else {
            continue;
        };
        let Some(minutes) = window
            .get("window_minutes")
            .and_then(serde_json::Value::as_i64)
        else {
            continue;
        };
        if minutes != longest_window {
            continue;
        }
        let seconds = minutes.checked_mul(60)?;
        let Some(reset_at) = window.get("resets_at").and_then(serde_json::Value::as_i64) else {
            continue;
        };
        let Some(authorities) = admitted.get(&observed_at.timestamp()) else {
            continue;
        };
        if authorities.len() != 1 {
            continue;
        }
        let existing_reset = authorities.iter().next().copied().flatten();
        if known_windows
            .get(&reset_at)
            .is_some_and(|windows| windows.len() != 1 || !windows.contains(&seconds))
        {
            continue;
        }
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
            seconds,
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
    fn zero_delta_range_quota_requires_hash_and_owned_unambiguous_authority() {
        for fault in ["none", "hash", "unowned", "ambiguous"] {
            let (root, mut store) = fixture();
            add_history(&mut store, MINUTE, Some(90.0));
            let at = MINUTE + 70;
            let body = format!(
                "{}\n",
                json!({"type":"event_msg","timestamp":DateTime::from_timestamp(at,0).unwrap().to_rfc3339(),
                "payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":0,"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"cache_write_input_tokens":0}},
                "rate_limits":{"secondary":{"used_percent":12,"resets_at":RESET,"window_minutes":10080}}}})
            );
            fs::write(root.join("zero.jsonl"), &body).unwrap();
            let hash = if fault == "hash" {
                "0".repeat(64)
            } else {
                format!("{:x}", Sha256::digest(Sha256::digest(body.as_bytes())))
            };
            store.connection.execute("INSERT INTO session_ranges VALUES('root','zero.jsonl','1','2',0,?1,?2,'1',?2,?3)",params![body.len() as i64,"a".repeat(32),hash]).unwrap();
            if fault == "ambiguous" {
                let tx = store.connection.transaction().unwrap();
                upsert_observations(
                    &tx,
                    &[
                        UsageHistoryObservation::unavailable(MINUTE + 60, RESET, None),
                        UsageHistoryObservation::unavailable(MINUTE + 60, RESET + 1, None),
                    ],
                )
                .unwrap();
                tx.commit().unwrap();
            }
            let intervals = [SessionLifecycleInterval::new(
                if fault == "unowned" {
                    Some(at + 1)
                } else {
                    Some(MINUTE)
                },
                None,
            )];
            let restored = store
                .recover_session_quota_history_for_intervals(&root, MINUTE, &intervals)
                .unwrap();
            assert_eq!(
                restored,
                usize::from(fault == "none"),
                "range-only case={fault}"
            );
            let remaining:Option<f64>=store.connection.query_row("SELECT json_extract(snapshot_json,'$.remaining_percent') FROM durable_state WHERE singleton>=2 AND data_generation=?1 AND json_extract(snapshot_json,'$.reset_at')=?2",params![MINUTE+60,RESET],|r|r.get(0)).optional().unwrap().flatten();
            assert_eq!(remaining, (fault == "none").then_some(88.0));
            assert!(store.load_session_events().unwrap().is_empty());
            if fault == "none" {
                assert_eq!(
                    store
                        .connection
                        .query_row("SELECT observed_at FROM session_quota_windows", [], |r| r
                            .get::<_, i64>(
                            0
                        ))
                        .unwrap(),
                    at
                );
                assert_eq!(
                    store
                        .recover_session_quota_history_for_intervals(&root, MINUTE, &intervals)
                        .unwrap(),
                    0
                );
            }
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn recovered_quota_window_upgrade_keeps_v14_history_and_reopens() {
        let (root, mut store) = fixture();
        add_history(&mut store, MINUTE, Some(90.0));
        let original = store.load_all().unwrap();
        store
            .connection
            .execute_batch("DROP TABLE IF EXISTS session_quota_windows; PRAGMA user_version=14;")
            .unwrap();
        drop(store);
        let identity = StoragePartitionIdentity {
            schema_version: "codex-info-account-db-v1".into(),
            profile_scope_id: "a".repeat(32),
            account_scope_id: "b".repeat(64),
            storage_epoch: 1,
            partition_id: "c".repeat(64),
        };
        let database = root.join("history.sqlite3");
        let backup =
            UsageStore::backup_generations_partitioned_verified(&database, &identity, 3).unwrap();
        assert!(
            UsageStore::migrate_partition_history_after_verified_backup(
                &database, &identity, &backup
            )
            .unwrap(),
            "v14 needs a metadata-only upgrade for verified historical windows"
        );
        let reopened = UsageStore::open_partitioned(&database, &identity).unwrap();
        assert_eq!(reopened.load_all().unwrap(), original);
        let evidence_rows: i64 = reopened
            .connection
            .query_row("SELECT count(*) FROM session_quota_windows", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(evidence_rows, 0);
        drop(reopened);
        assert!(
            !UsageStore::migrate_partition_history_after_verified_backup(
                &database, &identity, &backup
            )
            .unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovered_quota_window_is_saved_without_replacing_known_history_or_current_receipts() {
        let (root, mut store) = fixture();
        add_history(&mut store, MINUTE, Some(90.0));
        add_source(&store, &root, "known-quota.jsonl", MINUTE + 10, RESET, 12.0);
        let before = store.load_all().unwrap();
        let receipt = store.latest_quota_receipt().unwrap();

        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            0
        );
        let exists: bool = store.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='session_quota_windows')",
            [], |row| row.get(0)).unwrap();
        assert!(
            exists,
            "verified Session window evidence must survive independently of current receipts"
        );
        let saved: (i64, i64, i64) = store
            .connection
            .query_row(
                "SELECT reset_at,window_seconds,observed_at FROM session_quota_windows",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(saved, (RESET, 604_800, MINUTE + 10));
        assert_eq!(store.load_all().unwrap(), before);
        assert_eq!(store.latest_quota_receipt().unwrap(), receipt);
        assert_eq!(
            store.recover_session_quota_history(&root, MINUTE).unwrap(),
            0
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_quota_recovery_rejects_ambiguous_durable_minute_authority() {
        for source_reset in [RESET, RESET + 1] {
            let (root, mut store) = fixture();
            let tx = store.connection.transaction().unwrap();
            upsert_observations(
                &tx,
                &[
                    UsageHistoryObservation::unavailable(MINUTE + 60, RESET, None),
                    UsageHistoryObservation::unavailable(MINUTE + 60, RESET + 1, None),
                ],
            )
            .unwrap();
            tx.commit().unwrap();
            add_source(
                &store,
                &root,
                "ambiguous.jsonl",
                MINUTE + 70,
                source_reset,
                12.0,
            );
            assert_eq!(
                store.recover_session_quota_history(&root, MINUTE).unwrap(),
                0,
                "two exact durable period authorities cannot be resolved by SQL row order"
            );
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn relocated_quota_requires_a_verified_accepted_range_and_the_same_normalized_event() {
        for (accepted, hash_matches, model_matches, tokens_match, expected) in [
            (true, true, true, true, 1),
            (false, true, true, true, 0),
            (true, false, true, true, 0),
            (true, true, false, true, 0),
            (true, true, true, false, 0),
        ] {
            let (root, mut store) = fixture();
            add_history(&mut store, MINUTE, Some(90.0));
            add_history(&mut store, MINUTE + 60, None);
            add_source(&store, &root, "rewritten.jsonl", MINUTE + 70, RESET, 12.0);
            store
                .connection
                .execute("UPDATE session_events SET cache_write_input_tokens='0'", [])
                .unwrap();
            let original_events = store.load_session_events().unwrap();
            let body = format!(
                "{}\n{}\n",
                json!({"type":"turn_context","payload":{"model":if model_matches {"SOL"} else {"LUNA"}}}),
                json!({
                    "type":"event_msg","timestamp":DateTime::from_timestamp(MINUTE+70,0).unwrap().to_rfc3339(),
                    "payload":{"type":"token_count","info":{"last_token_usage":{
                        "total_tokens":if tokens_match {1} else {2},"input_tokens":1,
                        "cached_input_tokens":0,"output_tokens":0,"cache_write_input_tokens":0
                    }},"rate_limits":{"secondary":{"used_percent":12,"resets_at":RESET,"window_minutes":10080}}}
                })
            );
            fs::write(root.join("rewritten.jsonl"), &body).unwrap();
            if accepted {
                let hash = if hash_matches {
                    format!("{:x}", Sha256::digest(Sha256::digest(body.as_bytes())))
                } else {
                    "0".repeat(64)
                };
                store.connection.execute(
                    "INSERT INTO session_ranges(root_identity,relative_path,file_device,file_inode,start_offset,end_offset,collector_epoch,cycle_seq,prefix_generation,record_sha256)
                     VALUES('root','rewritten.jsonl','1','3',0,?1,?2,'1',?3,?4)",
                    params![body.len(),"c".repeat(32),"b".repeat(32),hash],
                ).unwrap();
            }
            assert_eq!(
                store.recover_session_quota_history(&root, MINUTE).unwrap(),
                expected
            );
            assert_eq!(store.load_session_events().unwrap(), original_events);
            let rows = store.load_all().unwrap();
            let row = rows
                .iter()
                .find(|row| row.timestamp == MINUTE + 60)
                .unwrap();
            assert_eq!(
                row.remaining_percent,
                if expected == 1 { Some(88.0) } else { None }
            );
            assert_eq!(
                store.recover_session_quota_history(&root, MINUTE).unwrap(),
                0
            );
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn session_quota_recovery_requires_the_recorded_window() {
        for (seconds, source_minutes, expected) in [
            (604_800_i64, Some(300_i64), 0),
            (604_800, None, 0),
            (604_800, Some(10_080), 1),
            (18_000, Some(300), 1),
        ] {
            let (root, mut store) = fixture();
            let reset = MINUTE + seconds - 60;
            store
                .connection
                .execute(
                    "INSERT INTO quota_receipts VALUES('known-window',1,?1,?2,?3,90,'0')",
                    params![MINUTE, reset, seconds],
                )
                .unwrap();
            let receipt = store.latest_quota_receipt().unwrap();
            add_source(&store, &root, "window.jsonl", MINUTE + 70, reset, 12.0);
            let path = root.join("window.jsonl");
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            value["payload"]["rate_limits"]["secondary"]["window_minutes"] = json!(source_minutes);
            let body = format!("{value}\n");
            fs::write(path, &body).unwrap();
            store.connection.execute(
                "UPDATE session_events SET range_end=?1,record_sha256=?2 WHERE relative_path='window.jsonl'",
                params![body.len(), format!("{:x}", Sha256::digest(Sha256::digest(body.as_bytes())))],
            ).unwrap();
            assert_eq!(
                store.recover_session_quota_history(&root, MINUTE).unwrap(),
                expected,
                "recorded_seconds={seconds}, source_minutes={source_minutes:?}"
            );
            assert_eq!(store.latest_quota_receipt().unwrap(), receipt);
            let observations = store
                .load_recent_observations(Utc.timestamp_opt(MINUTE + 240, 0).unwrap())
                .unwrap();
            assert_eq!(observations.len(), expected);
            if expected == 1 {
                assert_eq!(
                    (observations[0].reset_at, observations[0].remaining_percent),
                    (reset, Some(88.0))
                );
                assert_eq!(observations[0].model_source, ModelSource::Unavailable);
                assert_eq!(observations[0].model_totals, None);
            }
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
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

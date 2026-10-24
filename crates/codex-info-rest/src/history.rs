//! Immutable history query indexes, built only when publishing a snapshot.

use codex_info_db_reader::DbSnapshot;
use codex_info_rest_contract::PublicHistoryObservationV3;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct HistoryIndex {
    periods: BTreeMap<String, PeriodIndex>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PeriodIndex {
    pub(crate) samples: Vec<usize>,
    pub(crate) gaps: Vec<usize>,
    pub(crate) has_token_usage: bool,
    fingerprints: Vec<[u8; 32]>,
}

impl HistoryIndex {
    pub(crate) fn build(snapshot: &DbSnapshot, storage_epoch: u64) -> Self {
        let mut index = Self::default();
        let mut periods_by_reset = BTreeMap::<i64, Vec<_>>::new();
        for period in &snapshot.retained_history.periods {
            periods_by_reset
                .entry(period.reset_at)
                .or_default()
                .push(period);
            index
                .periods
                .insert(period.id.clone(), PeriodIndex::default());
        }
        // Preserve the reader's ordered keys and the established period filter,
        // without scanning all retained samples separately for every period.
        for (row, sample) in snapshot.retained_history.samples.iter().enumerate() {
            #[cfg(test)]
            crate::record_history_row_visit();
            for (_, periods) in
                periods_by_reset.range(sample.reset_at..=sample.reset_at.saturating_add(60))
            {
                for period in periods {
                    if (sample.reset_at == period.reset_at
                        || !periods_by_reset.contains_key(&sample.reset_at))
                        && codex_info_rest_contract::history_minute_in_period(
                            sample.timestamp,
                            period.start_at,
                            period.end_at,
                        )
                    {
                        let indexed = index.periods.get_mut(&period.id).expect("indexed period");
                        indexed.samples.push(row);
                        indexed.has_token_usage |= sample.models.as_ref().is_some_and(|models| {
                            models.iter().any(|model| model.total_tokens > 0)
                        });
                    }
                }
            }
        }
        for (row, gap) in snapshot.retained_history.gaps.iter().enumerate() {
            for (_, periods) in
                periods_by_reset.range(gap.reset_at..=gap.reset_at.saturating_add(60))
            {
                for period in periods {
                    if (gap.reset_at != period.reset_at
                        && periods_by_reset.contains_key(&gap.reset_at))
                        || gap.start_at < period.start_at
                        || gap.end_at > period.end_at
                    {
                        continue;
                    }
                    index
                        .periods
                        .get_mut(&period.id)
                        .expect("indexed period")
                        .gaps
                        .push(row);
                }
            }
        }
        for period in &snapshot.retained_history.periods {
            let indexed = index.periods.get_mut(&period.id).expect("indexed period");
            let gaps = indexed
                .gaps
                .iter()
                .map(|&row| &snapshot.retained_history.gaps[row])
                .collect::<Vec<_>>();
            let mut hash = Sha256::new();
            hash.update(b"codex-info-rest-history-prefix-v1\n");
            // Generation, label, current flag and a growing end_at are not
            // prefix identity: ordinary appends must preserve an issued cursor.
            hash.update(
                serde_json::to_vec(&(storage_epoch, &period.id, period.reset_at, gaps))
                    .expect("validated history identity is serializable"),
            );
            hash.update(b"\n");
            indexed.fingerprints.reserve(indexed.samples.len());
            for (position, &row) in indexed.samples.iter().enumerate() {
                #[cfg(test)]
                crate::record_history_row_visit();
                hash.update(
                    serde_json::to_vec(&snapshot.retained_history.samples[row])
                        .expect("validated history observation is serializable"),
                );
                hash.update(b"\n");
                // Prove the local evidence in the same prefix as the samples.
                // Empty unused tails may grow without invalidating a cursor;
                // new/corrected activity in an old interval must reject it.
                let start = if position == 0 {
                    period.start_at
                } else {
                    snapshot.retained_history.samples[row].timestamp
                };
                let end = indexed
                    .samples
                    .get(position + 1)
                    .map(|&next| snapshot.retained_history.samples[next].timestamp)
                    .unwrap_or_else(|| period.end_at.saturating_add(1));
                if let Some(archive) = &snapshot.recordings {
                    if let Ok(local) = archive.local_usage(start, end) {
                        for interval in local.intervals.iter().filter(|v| v.state != codex_info_rest_contract::recorded_usage::LocalUsageState::Unused) {
                            let mut minute = interval.start_at.div_euclid(60) * 60;
                            while minute < interval.end_at {
                                hash.update(minute.to_le_bytes());
                                hash.update([u8::from(interval.state == codex_info_rest_contract::recorded_usage::LocalUsageState::Used)]);
                                minute += 60;
                            }
                        }
                    } else {
                        hash.update(b"local-invalid\n");
                    }
                } else {
                    hash.update(b"local-unavailable\n");
                }
                indexed.fingerprints.push(hash.clone().finalize().into());
            }
        }
        index
    }

    pub(crate) fn period(&self, id: &str) -> Option<&PeriodIndex> {
        self.periods.get(id)
    }
}

impl PeriodIndex {
    pub(crate) fn same_cursor_identity(&self, other: &Self) -> bool {
        self.fingerprints == other.fingerprints
    }

    pub(crate) fn resume(&self, rows: &[PublicHistoryObservationV3], position: usize) -> String {
        let sample = &rows[self.samples[position]];
        format!(
            "h1.{}.{}.{}",
            sample.reset_at,
            sample.timestamp,
            hex(&self.fingerprints[position])
        )
    }

    pub(crate) fn start_after(
        &self,
        rows: &[PublicHistoryObservationV3],
        cursor: &str,
    ) -> Option<usize> {
        let mut fields = cursor.split('.');
        if fields.next()? != "h1" {
            return None;
        }
        let reset = fields.next()?;
        let timestamp = fields.next()?;
        let fingerprint = fields.next()?;
        if fields.next().is_some()
            || fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        let key = (reset.parse::<i64>().ok()?, timestamp.parse::<i64>().ok()?);
        if reset != key.0.to_string() || timestamp != key.1.to_string() {
            return None;
        }
        // Each comparison touches one retained row; no prefix is rehashed on
        // a request, including requests issued against an older generation.
        let mut low = 0;
        let mut high = self.samples.len();
        while low < high {
            let mid = low + (high - low) / 2;
            #[cfg(test)]
            crate::record_history_row_visit();
            let sample = &rows[self.samples[mid]];
            match (sample.reset_at, sample.timestamp).cmp(&key) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => {
                    return (fingerprint == hex(&self.fingerprints[mid])).then_some(mid + 1);
                }
            }
        }
        None
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(64);
    for &byte in bytes {
        value.push(DIGITS[usize::from(byte >> 4)] as char);
        value.push(DIGITS[usize::from(byte & 15)] as char);
    }
    value
}

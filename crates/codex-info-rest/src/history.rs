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
    fingerprints: Vec<[u8; 32]>,
}

impl HistoryIndex {
    pub(crate) fn build(snapshot: &DbSnapshot, storage_epoch: u64) -> Self {
        let mut index = Self::default();
        let mut periods_by_reset = BTreeMap::<i64, Vec<_>>::new();
        for period in &snapshot.details.history_periods {
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
        for (row, sample) in snapshot.history_samples_v3.iter().enumerate() {
            #[cfg(test)]
            crate::record_history_row_visit();
            for (_, periods) in
                periods_by_reset.range(sample.reset_at..=sample.reset_at.saturating_add(60))
            {
                for period in periods {
                    if sample.timestamp >= period.start_at && sample.timestamp <= period.end_at {
                        index
                            .periods
                            .get_mut(&period.id)
                            .expect("indexed period")
                            .samples
                            .push(row);
                    }
                }
            }
        }
        for (row, gap) in snapshot.details.history_gaps.iter().enumerate() {
            for (_, periods) in
                periods_by_reset.range(gap.reset_at..=gap.reset_at.saturating_add(60))
            {
                for period in periods {
                    index
                        .periods
                        .get_mut(&period.id)
                        .expect("indexed period")
                        .gaps
                        .push(row);
                }
            }
        }
        for period in &snapshot.details.history_periods {
            let indexed = index.periods.get_mut(&period.id).expect("indexed period");
            let gaps = indexed
                .gaps
                .iter()
                .map(|&row| &snapshot.details.history_gaps[row])
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
            for &row in &indexed.samples {
                #[cfg(test)]
                crate::record_history_row_visit();
                hash.update(
                    serde_json::to_vec(&snapshot.history_samples_v3[row])
                        .expect("validated history observation is serializable"),
                );
                hash.update(b"\n");
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

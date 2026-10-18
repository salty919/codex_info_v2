//! Associate consumption observation times with qualified official quota facts.
//!
//! DATA_PROTECTION_POLICY's "数値eventの公式期間所属" owns these rules.
//! Source/account qualification precedes this module. Session quota snapshots
//! are not observations accepted by this API. Each input belongs to one account
//! partition; ownership_interval distinguishes its continuous ownership spans.

use std::collections::{BTreeMap, BTreeSet};

const MAX_UNIX_SECONDS: i64 = 253_402_300_799;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OfficialPeriod {
    pub reset_at: i64,
    pub window_seconds: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OfficialObservation {
    pub lower_at: i64,
    pub upper_at: i64,
    pub reset_at: Option<i64>,
    pub window_seconds: Option<i64>,
    pub acquisition_sequence: Option<u64>,
    pub ownership_interval: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributionError {
    InvalidBounds,
    InvalidReset,
    InvalidWindow,
    InvalidReceiptSequence,
    ConflictingReceiptSequence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Bounds {
    lower: i64,
    upper: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Component {
    ownership_interval: u64,
    bounds: Bounds,
    period: Option<OfficialPeriod>,
}

#[derive(Clone, Copy, Debug)]
struct Run {
    ownership_interval: u64,
    first: Bounds,
    last: Bounds,
    period: Option<OfficialPeriod>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AttributedSpan {
    ownership_interval: u64,
    bounds: Bounds,
    period: OfficialPeriod,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributionTimeline {
    spans: Vec<AttributedSpan>,
}

impl AttributionTimeline {
    /// Inputs are the complete qualified evidence for the reconstruction.
    /// A bounded/incremental caller must retain the same clock/window/run facts.
    /// Bounds have already been intersected with one proved ownership interval.
    pub fn from_observations(
        observations: &[OfficialObservation],
    ) -> Result<Self, AttributionError> {
        let mut receipts = BTreeMap::new();
        let mut unique = BTreeSet::new();
        let mut windows: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
        for &observation in observations {
            if !(1..=MAX_UNIX_SECONDS).contains(&observation.lower_at)
                || !(observation.lower_at..=MAX_UNIX_SECONDS).contains(&observation.upper_at)
            {
                return Err(AttributionError::InvalidBounds);
            }
            if observation
                .reset_at
                .is_some_and(|reset| !(1..=MAX_UNIX_SECONDS).contains(&reset))
                || (observation.reset_at.is_none() && observation.window_seconds.is_some())
            {
                return Err(AttributionError::InvalidReset);
            }
            if let Some(window) = observation.window_seconds {
                if window <= 0 {
                    return Err(AttributionError::InvalidWindow);
                }
                windows
                    .entry(observation.reset_at.expect("window requires reset"))
                    .or_default()
                    .insert(window);
            }
            if let Some(sequence) = observation.acquisition_sequence {
                if sequence == 0 || observation.lower_at != observation.upper_at {
                    return Err(AttributionError::InvalidReceiptSequence);
                }
                if let Some(previous) = receipts.insert(sequence, observation) {
                    if previous != observation {
                        return Err(AttributionError::ConflictingReceiptSequence);
                    }
                }
            }
            unique.insert(observation);
        }

        let mut components: Vec<Component> = unique
            .into_iter()
            .map(|observation| {
                let period = observation.window_seconds.and_then(|window| {
                    let reset_at = observation.reset_at?;
                    let origin = reset_at.checked_sub(window)?;
                    (origin >= 1 && windows[&reset_at].len() == 1).then_some(OfficialPeriod {
                        reset_at,
                        window_seconds: window,
                    })
                });
                let original = Bounds {
                    lower: observation.lower_at,
                    upper: observation.upper_at,
                };
                let (bounds, period) = match period {
                    Some(period) => {
                        let clipped = Bounds {
                            lower: original.lower.max(period.reset_at - period.window_seconds),
                            upper: original.upper.min(period.reset_at - 1),
                        };
                        if clipped.lower <= clipped.upper {
                            (clipped, Some(period))
                        } else {
                            (original, None)
                        }
                    }
                    None => (original, None),
                };
                Component {
                    ownership_interval: observation.ownership_interval,
                    bounds,
                    period,
                }
            })
            .collect();

        // Acquisition order proves clock reversals; time sorting must not hide
        // them. Separate ownership intervals never establish continuity.
        let mut previous_time = BTreeMap::new();
        for observation in receipts.values() {
            if let Some(previous) =
                previous_time.insert(observation.ownership_interval, observation.lower_at)
            {
                if observation.lower_at < previous {
                    components.push(Component {
                        ownership_interval: observation.ownership_interval,
                        bounds: Bounds {
                            lower: observation.lower_at,
                            upper: previous,
                        },
                        period: None,
                    });
                }
            }
        }

        components.sort_by_key(|component| {
            (
                component.ownership_interval,
                component.bounds.lower,
                component.bounds.upper,
            )
        });
        let mut merged: Vec<Component> = Vec::new();
        for component in components {
            if let Some(previous) = merged.last_mut() {
                if previous.ownership_interval == component.ownership_interval
                    && component.bounds.lower <= previous.bounds.upper
                {
                    previous.bounds.upper = previous.bounds.upper.max(component.bounds.upper);
                    if previous.period != component.period {
                        previous.period = None;
                    }
                    continue;
                }
            }
            merged.push(component);
        }

        let mut runs: Vec<Run> = Vec::new();
        for component in merged {
            if let Some(previous) = runs.last_mut() {
                if component.period.is_some()
                    && previous.ownership_interval == component.ownership_interval
                    && previous.period == component.period
                {
                    previous.last = component.bounds;
                    continue;
                }
            }
            runs.push(Run {
                ownership_interval: component.ownership_interval,
                first: component.bounds,
                last: component.bounds,
                period: component.period,
            });
        }

        let mut limits: Vec<Bounds> = runs
            .iter()
            .map(|run| Bounds {
                lower: run.first.upper,
                upper: run.last.lower,
            })
            .collect();
        for (index, adjacent) in runs.windows(2).enumerate() {
            let [previous, current] = adjacent else {
                unreachable!()
            };
            if let (Some(old), Some(new)) = (previous.period, current.period) {
                let origin = new.reset_at - new.window_seconds;
                if previous.ownership_interval == current.ownership_interval
                    && old != new
                    && previous.last.upper < origin
                    && origin <= current.first.lower
                {
                    limits[index].upper = origin - 1;
                    limits[index + 1].lower = origin;
                }
            }
        }

        let spans = runs
            .iter()
            .zip(limits)
            .filter_map(|(run, bounds)| {
                let period = run.period?;
                let clipped = Bounds {
                    lower: bounds.lower.max(period.reset_at - period.window_seconds),
                    upper: bounds.upper.min(period.reset_at - 1),
                };
                (clipped.lower <= clipped.upper).then_some(AttributedSpan {
                    ownership_interval: run.ownership_interval,
                    bounds: clipped,
                    period,
                })
            })
            .collect();
        Ok(Self { spans })
    }

    pub fn period_at(&self, observed_at: i64, ownership_interval: u64) -> Option<OfficialPeriod> {
        self.attribution_at(observed_at, ownership_interval)
            .map(|(period, _, _)| period)
    }

    /// Keep an old proof whose entire span is still supported by the same
    /// owner and period. Revisit a shortened/split proof in full; a simple
    /// extension only admits the newly covered seconds.
    pub fn changed_ranges(&self, previous: &Self) -> Vec<(i64, i64)> {
        let contains = |new: &AttributedSpan, old: &AttributedSpan| {
            new.ownership_interval == old.ownership_interval
                && new.period == old.period
                && new.bounds.lower <= old.bounds.lower
                && old.bounds.upper <= new.bounds.upper
        };
        let mut ranges = previous
            .spans
            .iter()
            .filter(|old| !self.spans.iter().any(|new| contains(new, old)))
            .map(|old| (old.bounds.lower, old.bounds.upper))
            .collect::<Vec<_>>();
        for new in &self.spans {
            let mut lower = new.bounds.lower;
            for old in previous.spans.iter().filter(|old| contains(new, old)) {
                if lower < old.bounds.lower {
                    ranges.push((lower, old.bounds.lower - 1));
                }
                lower = lower.max(old.bounds.upper.saturating_add(1));
            }
            if lower <= new.bounds.upper {
                ranges.push((lower, new.bounds.upper));
            }
        }
        ranges.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (lower, upper) in ranges {
            if let Some(last) = merged.last_mut() {
                if lower <= last.1.saturating_add(1) {
                    last.1 = last.1.max(upper);
                    continue;
                }
            }
            merged.push((lower, upper));
        }
        merged
    }

    /// The period and proved closed range come from the same timeline span.
    pub fn attribution_at(
        &self,
        observed_at: i64,
        ownership_interval: u64,
    ) -> Option<(OfficialPeriod, i64, i64)> {
        let index = self.spans.partition_point(|span| {
            (span.ownership_interval, span.bounds.lower) <= (ownership_interval, observed_at)
        });
        let span = self.spans.get(index.checked_sub(1)?)?;
        (span.ownership_interval == ownership_interval && observed_at <= span.bounds.upper)
            .then_some((span.period, span.bounds.lower, span.bounds.upper))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(time: i64, reset: i64, window: Option<i64>, seq: u64) -> OfficialObservation {
        OfficialObservation {
            lower_at: time,
            upper_at: time,
            reset_at: Some(reset),
            window_seconds: window,
            acquisition_sequence: Some(seq),
            ownership_interval: 1,
        }
    }

    fn bounded(lower: i64, upper: i64, reset: i64, window: i64) -> OfficialObservation {
        OfficialObservation {
            lower_at: lower,
            upper_at: upper,
            reset_at: Some(reset),
            window_seconds: Some(window),
            acquisition_sequence: None,
            ownership_interval: 1,
        }
    }

    fn period(reset_at: i64, window_seconds: i64) -> OfficialPeriod {
        OfficialPeriod {
            reset_at,
            window_seconds,
        }
    }

    #[test]
    fn official_timeline_preserves_observed_periods_and_uncertainty() {
        let a = period(1000, 500);
        let b = period(1001, 500);
        // Independent fixed expectations: observation precision, exact reset,
        // clock ambiguity, official origin, ownership and no extrapolation.
        let cases = vec![
            (
                "stable",
                vec![
                    point(600, 1000, Some(500), 1),
                    point(700, 1000, Some(500), 2),
                ],
                vec![
                    (599, None),
                    (600, Some(a)),
                    (650, Some(a)),
                    (700, Some(a)),
                    (701, None),
                ],
            ),
            (
                "single point",
                vec![point(600, 1000, Some(500), 1)],
                vec![(599, None), (600, Some(a)), (601, None)],
            ),
            (
                "single minute",
                vec![bounded(600, 659, 1000, 500)],
                vec![(600, None), (630, None), (659, None)],
            ),
            (
                "minute precision",
                vec![bounded(600, 659, 1000, 500), bounded(720, 779, 1000, 500)],
                vec![
                    (658, None),
                    (659, Some(a)),
                    (700, Some(a)),
                    (720, Some(a)),
                    (721, None),
                ],
            ),
            (
                "overlap conflict",
                vec![bounded(600, 659, 1000, 500), bounded(620, 679, 1001, 500)],
                vec![(600, None), (650, None), (679, None)],
            ),
            (
                "same second conflict",
                vec![
                    point(600, 1000, Some(500), 1),
                    point(600, 1001, Some(500), 2),
                    point(700, 1000, Some(500), 3),
                    point(800, 1000, Some(500), 4),
                ],
                vec![(600, None), (650, None), (700, Some(a)), (750, Some(a))],
            ),
            (
                "shifted reset",
                vec![
                    point(600, 1000, Some(500), 1),
                    point(700, 1001, Some(500), 2),
                ],
                vec![(600, Some(a)), (601, None), (699, None), (700, Some(b))],
            ),
            (
                "official origin",
                vec![
                    point(600, 1000, Some(500), 1),
                    point(700, 1050, Some(400), 2),
                ],
                vec![
                    (600, Some(a)),
                    (649, Some(a)),
                    (650, Some(period(1050, 400))),
                    (699, Some(period(1050, 400))),
                    (700, Some(period(1050, 400))),
                    (701, None),
                ],
            ),
            (
                "clock regression",
                vec![
                    point(700, 1000, Some(500), 1),
                    point(650, 1000, Some(500), 2),
                    point(750, 1000, Some(500), 3),
                    point(800, 1000, Some(500), 4),
                ],
                vec![
                    (650, None),
                    (680, None),
                    (700, None),
                    (749, None),
                    (750, Some(a)),
                    (775, Some(a)),
                ],
            ),
            (
                "sequence gaps",
                vec![
                    point(600, 1000, Some(500), 1),
                    point(700, 1000, Some(500), 10),
                ],
                vec![(650, Some(a))],
            ),
            (
                "same ID returns",
                vec![
                    point(600, 1000, Some(500), 1),
                    point(650, 1001, Some(500), 2),
                    point(700, 1000, Some(500), 3),
                ],
                vec![
                    (600, Some(a)),
                    (625, None),
                    (650, Some(b)),
                    (675, None),
                    (700, Some(a)),
                ],
            ),
            (
                "window conflict",
                vec![
                    point(600, 1000, Some(500), 1),
                    point(700, 1000, Some(400), 2),
                ],
                vec![(600, None), (650, None), (700, None)],
            ),
            (
                "window clips minute",
                vec![bounded(480, 539, 1000, 500), point(600, 1000, Some(500), 1)],
                vec![
                    (499, None),
                    (500, None),
                    (539, Some(a)),
                    (550, Some(a)),
                    (600, Some(a)),
                ],
            ),
            (
                "exclusive reset",
                vec![
                    point(990, 1000, Some(500), 1),
                    point(999, 1000, Some(500), 2),
                ],
                vec![(990, Some(a)), (999, Some(a)), (1000, None)],
            ),
            (
                "missing window barrier",
                vec![
                    point(600, 1000, Some(500), 1),
                    point(650, 1100, None, 2),
                    point(700, 1000, Some(500), 3),
                ],
                vec![
                    (600, Some(a)),
                    (625, None),
                    (650, None),
                    (675, None),
                    (700, Some(a)),
                ],
            ),
        ];
        for (name, observations, expectations) in cases {
            let timeline = AttributionTimeline::from_observations(&observations).unwrap();
            for (time, expected) in expectations {
                assert_eq!(timeline.period_at(time, 1), expected, "{name} at {time}");
            }
        }
        let mut later = point(750, 1000, Some(500), 2);
        later.ownership_interval = 2;
        let timeline =
            AttributionTimeline::from_observations(&[point(600, 1000, Some(500), 1), later])
                .unwrap();
        assert_eq!(timeline.period_at(600, 1), Some(a));
        assert_eq!(timeline.period_at(625, 1), None);
        assert_eq!(timeline.period_at(725, 2), None);
        assert_eq!(timeline.period_at(750, 2), Some(a));
        assert_eq!(timeline.period_at(750, 1), None);
        let mut different_window = point(750, 1000, Some(400), 2);
        different_window.ownership_interval = 2;
        let timeline = AttributionTimeline::from_observations(&[
            point(600, 1000, Some(500), 1),
            different_window,
        ])
        .unwrap();
        assert_eq!(timeline.period_at(600, 1), None);
        assert_eq!(timeline.period_at(750, 2), None);
        assert_eq!(
            AttributionTimeline::from_observations(&[])
                .unwrap()
                .period_at(650, 1),
            None
        );
    }

    #[test]
    fn official_timeline_requalification_uses_the_original_acquisition_sequence() {
        let first = point(600, 1000, Some(500), 1);
        let last = point(700, 1000, Some(500), 3);
        let recovered = point(650, 1001, Some(500), 2);
        let before = AttributionTimeline::from_observations(&[first, last]).unwrap();
        assert_eq!(before.period_at(650, 1), Some(period(1000, 500)));
        let after = AttributionTimeline::from_observations(&[last, recovered, first]).unwrap();
        assert_eq!(after.period_at(650, 1), Some(period(1001, 500)));
        assert_eq!(after.period_at(625, 1), None);
        assert_eq!(after.period_at(675, 1), None);
        assert_eq!(
            after,
            AttributionTimeline::from_observations(&[first, recovered, last, recovered]).unwrap()
        );
    }

    #[test]
    fn official_timeline_rejects_invalid_qualified_observation_shapes() {
        let valid = point(600, 1000, Some(500), 1);
        for invalid in [
            OfficialObservation {
                lower_at: 0,
                ..valid
            },
            OfficialObservation {
                upper_at: 253_402_300_800,
                ..valid
            },
            OfficialObservation {
                lower_at: 601,
                ..valid
            },
            OfficialObservation {
                reset_at: Some(0),
                ..valid
            },
            OfficialObservation {
                window_seconds: Some(0),
                ..valid
            },
            OfficialObservation {
                window_seconds: Some(-1),
                ..valid
            },
            OfficialObservation {
                acquisition_sequence: Some(0),
                ..valid
            },
            OfficialObservation {
                upper_at: 601,
                ..valid
            },
        ] {
            assert!(
                AttributionTimeline::from_observations(&[invalid]).is_err(),
                "{invalid:?}"
            );
        }
        let conflicting = OfficialObservation {
            reset_at: Some(1001),
            ..valid
        };
        assert_eq!(
            AttributionTimeline::from_observations(&[valid, conflicting]),
            Err(AttributionError::ConflictingReceiptSequence)
        );
    }
}

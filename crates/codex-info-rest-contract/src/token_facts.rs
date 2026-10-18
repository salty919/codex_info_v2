//! Lossless token arithmetic shared by source admission and public projection.
//! DATA_PROTECTION_POLICY's RECORDER-MODEL-01 owns the arithmetic, including
//! the distinction between raw comparison counters and consumption facts.

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenFacts {
    pub total: u64,
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub cache_write_input: Option<u64>,
    pub output: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenFactError {
    ComponentExceedsTotal,
    TotalDoesNotMatchComponents,
    DiscountedInputExceedsInput,
    ArithmeticOverflow,
    CounterRegression,
}

impl TokenFacts {
    pub fn validate_fact(self) -> Result<(), TokenFactError> {
        self.validate(true)
    }

    pub fn validate_raw_counter(self) -> Result<(), TokenFactError> {
        self.validate(false)
    }

    fn validate(self, exact_total: bool) -> Result<(), TokenFactError> {
        if self.input.is_some_and(|value| value > self.total)
            || self.output.is_some_and(|value| value > self.total)
        {
            return Err(TokenFactError::ComponentExceedsTotal);
        }
        if let Some((input, output)) = self.input.zip(self.output) {
            let sum = input
                .checked_add(output)
                .ok_or(TokenFactError::ArithmeticOverflow)?;
            if sum > self.total || (exact_total && sum != self.total) {
                return Err(TokenFactError::TotalDoesNotMatchComponents);
            }
        }
        // The sum of known discounted components is a lower bound. This
        // check does not assign a value to any unknown stored component.
        let lower_bound = self
            .cached_input
            .into_iter()
            .chain(self.cache_write_input)
            .try_fold(0_u64, |sum, value| sum.checked_add(value))
            .ok_or(TokenFactError::ArithmeticOverflow)?;
        let input_bound = match (self.input, self.output) {
            (Some(input), _) => input,
            (None, Some(output)) => self.total - output,
            (None, None) => self.total,
        };
        if lower_bound > input_bound {
            return Err(TokenFactError::DiscountedInputExceedsInput);
        }
        Ok(())
    }

    /// A failed observation is not returned as a replacement baseline.
    /// Source continuity/model/ownership and context adjustment qualification
    /// precede this arithmetic; none can be inferred from these numbers.
    pub fn checked_delta(self, previous: Self) -> Result<Self, TokenFactError> {
        self.validate_raw_counter()?;
        previous.validate_raw_counter()?;
        let delta = Self {
            total: self
                .total
                .checked_sub(previous.total)
                .ok_or(TokenFactError::CounterRegression)?,
            input: subtract(self.input, previous.input)?,
            cached_input: subtract(self.cached_input, previous.cached_input)?,
            cache_write_input: subtract(self.cache_write_input, previous.cache_write_input)?,
            output: subtract(self.output, previous.output)?,
        };
        delta.validate_fact()?;
        Ok(delta)
    }

    pub fn checked_sum(self, other: Self) -> Result<Self, TokenFactError> {
        self.validate_fact()?;
        other.validate_fact()?;
        let sum = Self {
            total: self
                .total
                .checked_add(other.total)
                .ok_or(TokenFactError::ArithmeticOverflow)?,
            input: add(self.input, other.input)?,
            cached_input: add(self.cached_input, other.cached_input)?,
            cache_write_input: add(self.cache_write_input, other.cache_write_input)?,
            output: add(self.output, other.output)?,
        };
        sum.validate_fact()?;
        Ok(sum)
    }
}

fn subtract(current: Option<u64>, previous: Option<u64>) -> Result<Option<u64>, TokenFactError> {
    match current.zip(previous) {
        Some((current, previous)) => current
            .checked_sub(previous)
            .map(Some)
            .ok_or(TokenFactError::CounterRegression),
        None => Ok(None),
    }
}

fn add(left: Option<u64>, right: Option<u64>) -> Result<Option<u64>, TokenFactError> {
    match left.zip(right) {
        Some((left, right)) => left
            .checked_add(right)
            .map(Some)
            .ok_or(TokenFactError::ArithmeticOverflow),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(
        total: u64,
        input: Option<u64>,
        cached_input: Option<u64>,
        cache_write_input: Option<u64>,
        output: Option<u64>,
    ) -> TokenFacts {
        TokenFacts {
            total,
            input,
            cached_input,
            cache_write_input,
            output,
        }
    }

    #[test]
    fn numeric_facts_and_raw_counters_follow_the_distinct_owner_equations() {
        // Fixed expected results from the owner equations, including cases
        // where a missing component cannot conceal a known contradiction.
        let cases = [
            (
                "known zero",
                facts(0, Some(0), Some(0), Some(0), Some(0)),
                true,
                true,
            ),
            (
                "unknown details",
                facts(10, None, None, None, None),
                true,
                true,
            ),
            (
                "missing writes",
                facts(10, Some(8), Some(2), None, Some(2)),
                true,
                true,
            ),
            (
                "context adjustment",
                facts(10, Some(0), Some(0), None, Some(0)),
                false,
                true,
            ),
            (
                "raw context gap",
                facts(10, Some(3), None, None, Some(2)),
                false,
                true,
            ),
            (
                "input exceeds total",
                facts(10, Some(11), None, None, None),
                false,
                false,
            ),
            (
                "output exceeds total",
                facts(10, None, None, None, Some(11)),
                false,
                false,
            ),
            (
                "known sum exceeds total",
                facts(10, Some(8), None, None, Some(3)),
                false,
                false,
            ),
            (
                "discount lower bound",
                facts(10, None, Some(2), None, Some(9)),
                false,
                false,
            ),
            (
                "writes lower bound",
                facts(10, None, None, Some(2), Some(9)),
                false,
                false,
            ),
            (
                "discount exceeds input",
                facts(10, Some(8), Some(7), Some(2), Some(2)),
                false,
                false,
            ),
            (
                "discount exceeds total",
                facts(10, None, Some(11), None, None),
                false,
                false,
            ),
            (
                "unknown can be satisfied",
                facts(10, None, Some(7), Some(1), Some(2)),
                true,
                true,
            ),
            (
                "full unsigned total",
                facts(u64::MAX, None, None, None, None),
                true,
                true,
            ),
            (
                "full unsigned input",
                facts(u64::MAX, Some(u64::MAX), None, None, Some(0)),
                true,
                true,
            ),
            (
                "sum overflow",
                facts(u64::MAX, Some(u64::MAX), None, None, Some(1)),
                false,
                false,
            ),
            (
                "discount overflow",
                facts(u64::MAX, None, Some(u64::MAX), Some(1), None),
                false,
                false,
            ),
            (
                "lower bound overflow",
                facts(u64::MAX, None, Some(u64::MAX), None, Some(1)),
                false,
                false,
            ),
            (
                "zero cannot hide input",
                facts(0, None, Some(1), None, None),
                false,
                false,
            ),
        ];
        for (name, value, fact_valid, raw_valid) in cases {
            assert_eq!(value.validate_fact().is_ok(), fact_valid, "fact: {name}");
            assert_eq!(
                value.validate_raw_counter().is_ok(),
                raw_valid,
                "raw: {name}"
            );
        }
    }

    #[test]
    fn counter_delta_preserves_unknowns_and_does_not_rebaseline_regressions() {
        let previous = facts(20, Some(12), None, None, Some(8));
        let next = facts(25, Some(15), Some(5), None, Some(10));
        let delta = facts(5, Some(3), None, None, Some(2));
        assert_eq!(next.checked_delta(previous), Ok(delta));
        let later = facts(30, Some(18), Some(7), None, Some(12));
        assert_eq!(
            later.checked_delta(next),
            Ok(facts(5, Some(3), Some(2), None, Some(2)))
        );
        let regression = facts(10, Some(6), None, None, Some(4));
        assert_eq!(
            regression.checked_delta(previous),
            Err(TokenFactError::CounterRegression)
        );
        assert_eq!(
            next.checked_delta(previous),
            Ok(delta),
            "failed raw is never a new anchor"
        );
        assert_eq!(
            facts(25, Some(15), Some(5), None, Some(10)).checked_delta(facts(
                20,
                Some(12),
                Some(7),
                None,
                Some(8)
            )),
            Err(TokenFactError::CounterRegression)
        );
        assert_eq!(
            facts(25, Some(3), Some(1), None, Some(2)).checked_delta(facts(
                20,
                Some(0),
                Some(0),
                None,
                Some(0)
            )),
            Ok(facts(5, Some(3), Some(1), None, Some(2)))
        );
        assert!(
            facts(25, Some(2), None, None, Some(1))
                .checked_delta(facts(20, Some(0), None, None, Some(0)))
                .is_err(),
            "a raw context gap is not an extra consumption component"
        );
        assert_eq!(
            facts(20, Some(12), None, None, Some(8)).checked_delta(previous),
            Ok(facts(0, Some(0), None, None, Some(0)))
        );
    }

    #[test]
    fn numeric_sum_preserves_unknowns_and_rejects_overflow() {
        let left = facts(5, Some(3), Some(1), Some(0), Some(2));
        let right = facts(7, Some(5), None, Some(0), Some(2));
        let sum = facts(12, Some(8), None, Some(0), Some(4));
        assert_eq!(left.checked_sum(right), Ok(sum));
        assert_eq!(right.checked_sum(left), Ok(sum));
        assert_eq!(
            facts(u64::MAX, None, None, None, None).checked_sum(facts(1, None, None, None, None)),
            Err(TokenFactError::ArithmeticOverflow)
        );
        assert!(left
            .checked_sum(facts(1, Some(2), None, None, None))
            .is_err());
        assert_eq!(
            facts(0, None, None, None, None).checked_sum(left),
            Ok(facts(5, None, None, None, None)),
            "an unknown zero observation is not a complete zero identity"
        );
    }
}

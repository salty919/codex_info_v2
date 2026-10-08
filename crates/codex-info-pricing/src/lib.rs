//! Versioned Standard short-context API reference prices. No runtime network access.

#[derive(Clone, Copy, Debug)]
pub struct Usage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: u64,
}

#[derive(Clone, Debug)]
pub struct Cost {
    pub price_version: String,
    pub ordinary_input_dollars: f64,
    pub cached_input_dollars: f64,
    pub cache_write_input_dollars: f64,
    pub output_dollars: f64,
    pub total_dollars: f64,
}

#[derive(serde::Deserialize)]
struct Catalog {
    revisions: Vec<Revision>,
}

#[derive(serde::Deserialize)]
struct Revision {
    id: String,
    observed_at: i64,
    models: std::collections::BTreeMap<String, Rates>,
}

#[derive(serde::Deserialize)]
struct Rates {
    input: f64,
    cached_input: f64,
    cache_write_input: f64,
    output: f64,
}

fn catalog() -> Option<&'static Catalog> {
    static CATALOG: std::sync::OnceLock<Option<Catalog>> = std::sync::OnceLock::new();
    CATALOG
        .get_or_init(|| serde_json::from_str(include_str!("../data/standard-short.json")).ok())
        .as_ref()
}

fn price(revision: &Revision, model: &str, usage: Usage) -> Option<Cost> {
    let rates = revision.models.get(&model.trim().to_ascii_lowercase())?;
    calculate(rates, &revision.id, usage)
}

pub fn estimate(model: &str, usage: Usage) -> Option<Cost> {
    price(catalog()?.revisions.last()?, model, usage)
}

pub fn estimate_at(model: &str, usage: Usage, observed_at: i64) -> Option<Cost> {
    let revision = catalog()?
        .revisions
        .iter()
        .rev()
        .find(|r| r.observed_at <= observed_at)?;
    price(revision, model, usage)
}

pub fn estimate_with_revision(model: &str, usage: Usage, revision: &str) -> Option<Cost> {
    price(
        catalog()?.revisions.iter().find(|r| r.id == revision)?,
        model,
        usage,
    )
}

pub fn family(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "sol" | "gpt-6.1-sol" | "gpt-6-sol" | "gpt-5.6-sol" => Some("SOL"),
        "terra" | "gpt-5.6-terra" => Some("TERRA"),
        "luna" | "gpt-6-luna" | "gpt-5.6-luna" => Some("LUNA"),
        "astra" | "gpt-6-astra" => Some("ASTRA"),
        _ => None,
    }
}

pub fn estimate_legacy(model: &str, usage: Usage) -> Option<Cost> {
    let (rates, version) = match model.trim().to_ascii_uppercase().as_str() {
        "ASTRA" => ([10.0, 1.0, 12.5, 50.0], "ASTRA_USER_2026-09-05"),
        family if usage.cache_write_input_tokens == Some(0) => {
            let rates = match family {
                "SOL" => [5.0, 0.5, 0.0, 30.0],
                "TERRA" => [2.0, 0.2, 0.0, 12.0],
                "LUNA" => [0.2, 0.02, 0.0, 1.2],
                _ => return None,
            };
            (rates, "LOCAL_ESTIMATE_V1_2026-08-14")
        }
        _ => return None,
    };
    calculate(
        &Rates {
            input: rates[0],
            cached_input: rates[1],
            cache_write_input: rates[2],
            output: rates[3],
        },
        version,
        usage,
    )
}

fn calculate(rates: &Rates, version: &str, usage: Usage) -> Option<Cost> {
    let writes = usage.cache_write_input_tokens?;
    let ordinary = usage
        .input_tokens
        .checked_sub(usage.cached_input_tokens)?
        .checked_sub(writes)?;
    let ordinary_input_dollars = ordinary as f64 / 1_000_000.0 * rates.input;
    let cached_input_dollars = usage.cached_input_tokens as f64 / 1_000_000.0 * rates.cached_input;
    let cache_write_input_dollars = writes as f64 / 1_000_000.0 * rates.cache_write_input;
    let output_dollars = usage.output_tokens as f64 / 1_000_000.0 * rates.output;
    let total_dollars =
        ordinary_input_dollars + cached_input_dollars + cache_write_input_dollars + output_dollars;
    total_dollars.is_finite().then(|| Cost {
        price_version: version.to_owned(),
        ordinary_input_dollars,
        cached_input_dollars,
        cache_write_input_dollars,
        output_dollars,
        total_dollars,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage() -> Usage {
        Usage {
            input_tokens: 3_000_000,
            cached_input_tokens: 1_000_000,
            cache_write_input_tokens: Some(1_000_000),
            output_tokens: 1_000_000,
        }
    }

    #[test]
    fn persisted_revision_survives_catalog_updates() {
        let first = "1791430564-183968cc652f6276";
        let cost = estimate_with_revision("gpt-6-sol", usage(), first).unwrap();
        assert!((cost.total_dollars - 14.7).abs() < 1e-10);
        assert!(estimate_at("gpt-6-sol", usage(), 1791430563).is_none());
        assert_eq!(
            estimate_at("gpt-6-sol", usage(), 1791430564)
                .unwrap()
                .price_version,
            first
        );
    }

    #[test]
    fn official_standard_short_rates_and_component_partition() {
        for (model, expected) in [
            ("gpt-6-astra", [10.0, 1.0, 12.5, 50.0]),
            ("gpt-6.1-sol", [2.0, 0.1, 2.5, 10.0]),
            ("gpt-6-luna", [0.1, 0.01, 0.125, 0.5]),
            ("gpt-6-sol", [2.0, 0.2, 2.5, 10.0]),
            ("gpt-5.6-sol", [4.0, 0.4, 5.0, 20.0]),
            ("gpt-5.6-terra", [2.0, 0.2, 2.5, 12.0]),
            ("gpt-5.6-luna", [0.2, 0.02, 0.25, 1.2]),
        ] {
            let c =
                estimate_with_revision(model, usage(), "1791430564-183968cc652f6276").expect(model);
            assert_eq!(
                [
                    c.ordinary_input_dollars,
                    c.cached_input_dollars,
                    c.cache_write_input_dollars,
                    c.output_dollars
                ],
                expected
            );
            assert!((c.total_dollars - expected.iter().sum::<f64>()).abs() < 1e-10);
            assert!(!c.price_version.is_empty());
        }
    }

    #[test]
    fn missing_components_unknown_versions_and_invalid_partitions_are_unpriced() {
        assert!(estimate("SOL", usage()).is_none());
        assert!(estimate("gpt-7-sol", usage()).is_none());
        assert_eq!(family("gpt-6.1-sol"), Some("SOL"));
        assert_eq!(family("SOL"), Some("SOL"));
        assert_eq!(family("gpt-7-sol"), None);
        let mut u = usage();
        u.cache_write_input_tokens = None;
        assert!(estimate("gpt-6-sol", u).is_none());
        u.cache_write_input_tokens = Some(u64::MAX);
        assert!(estimate("gpt-6-sol", u).is_none());
        assert!(estimate_at("gpt-6-sol", usage(), 0).is_none());
        assert!(estimate_with_revision("gpt-6-sol", usage(), "missing").is_none());
    }

    #[test]
    fn legacy_rates_are_frozen_and_not_model_aliases() {
        let u = Usage {
            cache_write_input_tokens: Some(0),
            ..usage()
        };
        for (model, total, revision) in [
            ("SOL", 40.5, "LOCAL_ESTIMATE_V1_2026-08-14"),
            ("TERRA", 16.2, "LOCAL_ESTIMATE_V1_2026-08-14"),
            ("LUNA", 1.62, "LOCAL_ESTIMATE_V1_2026-08-14"),
            ("ASTRA", 71.0, "ASTRA_USER_2026-09-05"),
        ] {
            let c = estimate_legacy(model, u).unwrap();
            assert!((c.total_dollars - total).abs() < 1e-10);
            assert_eq!(c.price_version, revision);
        }
        assert!(estimate_legacy("SOL", usage()).is_none());
        assert!(estimate_legacy("gpt-6-sol", u).is_none());
    }
}

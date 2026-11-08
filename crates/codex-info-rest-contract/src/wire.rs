//! The single UI wire contract and receiver representation (Refs #690).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub mod cache;
pub mod manifest;
pub mod persistent;
pub mod request;
pub mod sync;

pub type Digest = [u8; 32];

pub const KINDS: &[&str] = &[
    "main",
    "selection",
    "quota",
    "model_usage",
    "model_cost",
    "thread_summary",
    "thread_model_count",
    "account_choices",
    "account_choice",
    "account_context",
    "ownership_interval",
    "runtime",
    "period",
    "sample",
    "history_model",
    "gap",
    "local_usage",
    "local_interval",
    "thread",
    "text",
    "u64",
    "i64",
    "f64",
    "bool",
    "null",
    "list",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub id: u64,
    pub kind: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireError(pub &'static str);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for WireError {}

pub trait DefinitionStore {
    fn definition(&self, id: &u64) -> Option<&Definition>;
}
pub trait DigestStore {
    fn cached(&self, id: &u64) -> Option<Digest>;
}
impl DefinitionStore for BTreeMap<u64, Definition> {
    fn definition(&self, id: &u64) -> Option<&Definition> {
        self.get(id)
    }
}
impl DefinitionStore for persistent::Map<u64, Definition> {
    fn definition(&self, id: &u64) -> Option<&Definition> {
        self.get(id)
    }
}
impl DigestStore for BTreeMap<u64, Digest> {
    fn cached(&self, id: &u64) -> Option<Digest> {
        self.get(id).copied()
    }
}
impl DigestStore for persistent::Map<u64, Digest> {
    fn cached(&self, id: &u64) -> Option<Digest> {
        self.get(id).copied()
    }
}
pub struct KnownPair<'a>(
    pub &'a persistent::Map<u64, Digest>,
    pub &'a persistent::Map<u64, Digest>,
);
impl DigestStore for KnownPair<'_> {
    fn cached(&self, id: &u64) -> Option<Digest> {
        self.0.get(id).or_else(|| self.1.get(id)).copied()
    }
}

/// Kind/field context is mandatory for schema numbers such as model costs.
/// A JSON integer token in an f64 field remains a binary64 value.
pub fn value_digest(
    kind: Option<&str>,
    value: &Value,
    definitions: &impl DefinitionStore,
) -> Result<Digest, WireError> {
    digest_value(
        kind,
        value,
        definitions,
        &BTreeMap::<u64, Digest>::new(),
        &mut BTreeSet::new(),
        0,
    )
}

pub fn value_digest_with_known(
    kind: Option<&str>,
    value: &Value,
    definitions: &impl DefinitionStore,
    known: &impl DigestStore,
) -> Result<Digest, WireError> {
    digest_value(kind, value, definitions, known, &mut BTreeSet::new(), 0)
}

fn hash(bytes: impl AsRef<[u8]>) -> Digest {
    Sha256::digest(bytes.as_ref()).into()
}

fn reference(value: &Value) -> Result<Option<u64>, WireError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if !object.contains_key("ref") {
        return Ok(None);
    }
    if object.len() != 1 {
        return Err(WireError("reserved reference key"));
    }
    object["ref"]
        .as_u64()
        .filter(|id| *id > 0)
        .map(Some)
        .ok_or(WireError("invalid reference"))
}

fn field_kind(kind: Option<&str>, field: &str) -> Option<&'static str> {
    match (kind, field) {
        (Some("model_cost"), "price_revision") => Some("text"),
        (Some("model_cost"), _) => Some("f64"),
        (Some("quota" | "sample"), "remaining_percent") => Some("f64"),
        (Some("history_model"), "total_dollars") => Some("f64"),
        (Some("main"), "selection") => Some("selection"),
        (Some("main"), "quota") => Some("quota"),
        (Some("main"), "models") => Some("model_usage[]"),
        (Some("main"), "thread_summary") => Some("thread_summary"),
        (Some("model_usage"), "estimated_cost") => Some("model_cost"),
        (Some("sample"), "models") => Some("history_model[]"),
        (Some("account_choices"), "accounts") => Some("account_choice[]"),
        (Some("account_context"), "ownership_intervals") => Some("ownership_interval[]"),
        (Some("local_usage"), "intervals") => Some("local_interval[]"),
        (Some("thread_summary"), "by_model") => Some("thread_model_count[]"),
        (
            _,
            "observed_at"
            | "reset_at"
            | "start_at"
            | "end_at"
            | "origin_at"
            | "activation_at"
            | "deactivation_at"
            | "created_at"
            | "last_user_message_at"
            | "window_seconds"
            | "depth",
        ) => Some("i64"),
        _ => None,
    }
}

fn digest_value(
    kind: Option<&str>,
    value: &Value,
    definitions: &impl DefinitionStore,
    known: &impl DigestStore,
    visiting: &mut BTreeSet<u64>,
    depth: usize,
) -> Result<Digest, WireError> {
    if depth > 128 {
        return Err(WireError("reference depth exceeded"));
    }
    if let Some(id) = reference(value)? {
        if !visiting.insert(id) {
            return Err(WireError("reference cycle"));
        }
        let definition = match definitions.definition(&id) {
            Some(definition) => definition,
            None => {
                visiting.remove(&id);
                return known.cached(&id).ok_or(WireError("unresolved reference"));
            }
        };
        if let Some(expected) = kind {
            if definition.kind != "null" && !expected.ends_with("[]") && expected != definition.kind
            {
                return Err(WireError("reference kind mismatch"));
            }
        }
        if let Some(digest) = known.cached(&id) {
            visiting.remove(&id);
            return Ok(digest);
        }
        let result = digest_value(
            Some(&definition.kind),
            &definition.value,
            definitions,
            known,
            visiting,
            depth + 1,
        );
        visiting.remove(&id);
        return result;
    }
    if value.is_null() {
        return Ok(hash([0x10]));
    }
    let mut bytes = Vec::new();
    match (kind, value) {
        (Some("f64"), _) => {
            let number = value
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or(WireError("invalid f64"))?;
            bytes.push(0x15);
            bytes.extend(number.to_bits().to_be_bytes());
        }
        (Some("u64"), _) => {
            bytes.push(0x13);
            bytes.extend(
                value
                    .as_u64()
                    .ok_or(WireError("invalid u64"))?
                    .to_be_bytes(),
            );
        }
        (Some("i64"), _) => {
            bytes.push(0x14);
            bytes.extend(
                value
                    .as_i64()
                    .ok_or(WireError("invalid i64"))?
                    .to_be_bytes(),
            );
        }
        (_, Value::Bool(value)) => bytes.push(if *value { 0x12 } else { 0x11 }),
        (_, Value::Number(value)) => {
            if let Some(number) = value.as_u64() {
                bytes.push(0x13);
                bytes.extend(number.to_be_bytes());
            } else if let Some(number) = value.as_i64() {
                bytes.push(0x14);
                bytes.extend(number.to_be_bytes());
            } else {
                let number = value
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or(WireError("invalid number"))?;
                bytes.push(0x15);
                bytes.extend(number.to_bits().to_be_bytes());
            }
        }
        (_, Value::String(value)) => {
            bytes.push(0x16);
            bytes.extend((value.len() as u64).to_be_bytes());
            bytes.extend(value.as_bytes());
        }
        (_, Value::Array(values)) => {
            bytes.push(0x17);
            bytes.extend((values.len() as u64).to_be_bytes());
            let element_kind = kind.and_then(|kind| kind.strip_suffix("[]"));
            for value in values {
                bytes.extend(digest_value(
                    element_kind,
                    value,
                    definitions,
                    known,
                    visiting,
                    depth + 1,
                )?);
            }
        }
        (_, Value::Object(values)) => {
            bytes.push(0x18);
            bytes.extend((values.len() as u64).to_be_bytes());
            let sorted = values.iter().collect::<BTreeMap<_, _>>();
            for (key, value) in sorted {
                bytes.extend((key.len() as u64).to_be_bytes());
                bytes.extend(key.as_bytes());
                bytes.extend(digest_value(
                    field_kind(kind, key),
                    value,
                    definitions,
                    known,
                    visiting,
                    depth + 1,
                )?);
            }
        }
        (_, Value::Null) => unreachable!(),
    }
    Ok(hash(bytes))
}

pub fn manifest_root(entries: &[(Vec<u8>, Digest)]) -> Result<Digest, WireError> {
    let mut tree = manifest::Tree::default();
    let mut keys = BTreeSet::new();
    for (key, digest) in entries {
        if !keys.insert(key) {
            return Err(WireError("duplicate manifest key"));
        }
        tree.insert(key.clone(), *digest)?;
    }
    Ok(tree.root())
}

fn canonical_float(value: f64) -> Result<String, WireError> {
    if !value.is_finite() {
        return Err(WireError("nonfinite f64"));
    }
    if value == 0.0 {
        return Ok(if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .into());
    }
    // serde_json's shortest round-trip digits are normalized independently
    // of its presentation thresholds, which differ from .NET's formatter.
    let encoded = serde_json::to_string(&value.abs()).map_err(|_| WireError("f64 encoding"))?;
    let (mantissa, exponent) = encoded
        .split_once(['e', 'E'])
        .map_or((encoded.as_str(), 0), |(m, e)| {
            (m, e.parse::<i32>().unwrap())
        });
    let fractional = mantissa.split_once('.').map_or(0, |(_, f)| f.len() as i32);
    let mut digits = mantissa.replace('.', "").trim_start_matches('0').to_owned();
    let mut power = exponent - fractional;
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
        power += 1;
    }
    let point = digits.len() as i32 + power;
    let plain = if point <= 0 {
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else if point >= digits.len() as i32 {
        format!(
            "{}{}",
            digits,
            "0".repeat((point - digits.len() as i32) as usize)
        )
    } else {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    };
    let scientific = if digits.len() == 1 {
        format!("{}e{}", digits, point - 1)
    } else {
        format!("{}.{}e{}", &digits[..1], &digits[1..], point - 1)
    };
    let result = if plain.len() <= scientific.len() {
        plain
    } else {
        scientific
    };
    Ok(if value.is_sign_negative() {
        format!("-{result}")
    } else {
        result
    })
}

pub fn canonical_json(kind: Option<&str>, value: &Value) -> Result<Vec<u8>, WireError> {
    fn string(value: &str, output: &mut String) {
        output.push('"');
        for ch in value.chars() {
            match ch {
                '"' => output.push_str("\\\""),
                '\\' => output.push_str("\\\\"),
                ch if ch < ' ' => {
                    use std::fmt::Write;
                    write!(output, "\\u{:04x}", ch as u32).unwrap();
                }
                ch => output.push(ch),
            }
        }
        output.push('"');
    }
    fn encode(kind: Option<&str>, value: &Value, output: &mut String) -> Result<(), WireError> {
        if reference(value)?.is_some() {
            output.push_str(&serde_json::to_string(value).unwrap());
            return Ok(());
        }
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) if kind == Some("f64") || value.is_f64() => {
                output.push_str(&canonical_float(
                    value.as_f64().ok_or(WireError("invalid f64"))?,
                )?);
            }
            Value::Number(value) => output.push_str(&value.to_string()),
            Value::String(value) => string(value, output),
            Value::Array(values) => {
                output.push('[');
                let element_kind = kind.and_then(|kind| kind.strip_suffix("[]"));
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    encode(element_kind, value, output)?;
                }
                output.push(']');
            }
            Value::Object(values) => {
                output.push('{');
                for (index, (key, value)) in
                    values.iter().collect::<BTreeMap<_, _>>().iter().enumerate()
                {
                    if index > 0 {
                        output.push(',');
                    }
                    string(key, output);
                    output.push(':');
                    encode(field_kind(kind, key), value, output)?;
                }
                output.push('}');
            }
        }
        Ok(())
    }
    let mut result = String::new();
    encode(kind, value, &mut result)?;
    Ok(result.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/wire_vectors.json")).unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn digest(text: &str) -> Digest {
        std::array::from_fn(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
    }

    fn input(case: &Value) -> Value {
        if case["kind"] == "f64" {
            let bits = u64::from_str_radix(case["value"].as_str().unwrap(), 16).unwrap();
            Value::from(f64::from_bits(bits))
        } else {
            case["value"].clone()
        }
    }

    #[test]
    fn typed_value_hashes_match_independent_byte_vectors() {
        let definitions = BTreeMap::from([(
            7,
            Definition {
                id: 7,
                kind: "f64".into(),
                value: Value::from(1.0),
            },
        )]);
        for case in vectors()["values"].as_array().unwrap() {
            let kind = case["kind"].as_str().unwrap();
            let context = (!matches!(kind, "object" | "ref")).then_some(kind);
            let actual = value_digest(context, &input(case), &definitions).unwrap();
            assert_eq!(
                hex(&actual),
                case["digest"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn manifest_roots_match_independent_byte_vectors() {
        let fixture = vectors();
        let manifest = &fixture["manifest"];
        let entries = manifest["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["key"].as_str().unwrap().as_bytes().to_vec(),
                    digest(entry["value_digest"].as_str().unwrap()),
                )
            })
            .collect::<Vec<_>>();
        for (count, name) in [(0, "empty"), (1, "one_leaf"), (2, "two_leaves")] {
            assert_eq!(
                hex(&manifest_root(&entries[..count]).unwrap()),
                manifest[name].as_str().unwrap(),
                "{name}"
            );
        }
    }

    #[test]
    fn canonical_f64_preserves_typed_one_and_signed_zero() {
        for case in vectors()["values"].as_array().unwrap() {
            if case["kind"] != "f64" {
                continue;
            }
            assert_eq!(
                String::from_utf8(canonical_json(Some("f64"), &input(case)).unwrap()).unwrap(),
                case["canonical"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
}

pub mod incoming;

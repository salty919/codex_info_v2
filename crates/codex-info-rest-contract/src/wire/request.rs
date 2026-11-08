//! Exact UI resources and view selectors. There are no API version routes.

use super::{canonical_json, WireError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    Main,
    AccountChoices,
    AccountContext,
    Runtime,
}

impl Projection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::AccountChoices => "account_choices",
            Self::AccountContext => "account_context",
            Self::Runtime => "runtime",
        }
    }

    pub fn account_scoped(self) -> bool {
        matches!(self, Self::Main | Self::AccountContext)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resource {
    State,
    Periods,
    History,
    Threads,
    Subscription,
}

impl Resource {
    pub fn path(self) -> &'static str {
        match self {
            Self::State => "/state",
            Self::Periods => "/history/periods",
            Self::History => "/history",
            Self::Threads => "/threads",
            Self::Subscription => "/subscription",
        }
    }

    pub fn method(self) -> &'static str {
        if self == Self::Subscription {
            "POST"
        } else {
            "GET"
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodPurpose {
    Selectable,
    Window,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub resource: Resource,
    pub parts: BTreeSet<Projection>,
    pub account: Option<String>,
    pub period: Option<String>,
    pub purpose: Option<PeriodPurpose>,
    pub from: Option<i64>,
    pub until: Option<i64>,
    pub image: Option<String>,
    pub cursor: Option<String>,
}

pub fn opaque(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
}

impl Target {
    pub fn parse(target: &str) -> Result<Option<Self>, WireError> {
        if target.len() > 2048 || !target.starts_with('/') || target.contains(['#', '%']) {
            return Err(WireError("invalid target"));
        }
        let (path, raw) = target.split_once('?').unwrap_or((target, ""));
        let resource = match path {
            "/state" => Resource::State,
            "/history/periods" => Resource::Periods,
            "/history" => Resource::History,
            "/threads" => Resource::Threads,
            "/subscription" => Resource::Subscription,
            _ => return Ok(None),
        };
        if target.contains('?') && raw.is_empty() {
            return Err(WireError("empty query"));
        }
        let mut query = BTreeMap::new();
        for pair in raw.split('&').filter(|_| !raw.is_empty()) {
            let (name, value) = pair.split_once('=').ok_or(WireError("invalid query"))?;
            if name.is_empty()
                || value.is_empty()
                || value.len() > 1024
                || value.contains('=')
                || query.insert(name, value).is_some()
            {
                return Err(WireError("duplicate or empty query"));
            }
        }
        let allowed: &[&str] = match resource {
            Resource::State => &["parts", "account", "image", "cursor"],
            Resource::Periods => &["account", "purpose", "from", "until", "image", "cursor"],
            Resource::History => &["account", "period", "image", "cursor"],
            Resource::Threads => &["account", "image", "cursor"],
            Resource::Subscription => &[],
        };
        if query.keys().any(|name| !allowed.contains(name)) {
            return Err(WireError("unknown query"));
        }
        let text = |name: &str| -> Result<Option<String>, WireError> {
            query
                .get(name)
                .map(|value| {
                    if opaque(value) {
                        Ok((*value).to_owned())
                    } else {
                        Err(WireError("invalid selector"))
                    }
                })
                .transpose()
        };
        let mut result = Self {
            resource,
            parts: BTreeSet::new(),
            account: text("account")?,
            period: text("period")?,
            purpose: None,
            from: None,
            until: None,
            image: text("image")?,
            cursor: text("cursor")?,
        };
        if resource == Resource::State {
            for part in query
                .get("parts")
                .ok_or(WireError("parts required"))?
                .split(',')
            {
                let part = match part {
                    "main" => Projection::Main,
                    "account_choices" => Projection::AccountChoices,
                    "account_context" => Projection::AccountContext,
                    "runtime" => Projection::Runtime,
                    _ => return Err(WireError("unknown projection")),
                };
                if !result.parts.insert(part) {
                    return Err(WireError("duplicate projection"));
                }
            }
            if result.account.is_some() && !result.parts.iter().any(|part| part.account_scoped()) {
                return Err(WireError("account is irrelevant to projection"));
            }
        } else if resource != Resource::Subscription && result.account.is_none() {
            return Err(WireError("account required"));
        }
        if resource == Resource::History && result.period.is_none() {
            return Err(WireError("period required"));
        }
        if resource == Resource::Periods {
            result.purpose = Some(match query.get("purpose").copied() {
                Some("selectable") => PeriodPurpose::Selectable,
                Some("window") => PeriodPurpose::Window,
                _ => return Err(WireError("purpose required")),
            });
            if result.purpose == Some(PeriodPurpose::Window) {
                let time = |name| -> Result<i64, WireError> {
                    let text = query.get(name).ok_or(WireError("window bounds required"))?;
                    let value: i64 = text.parse().map_err(|_| WireError("invalid time"))?;
                    if value <= 0 || value > 253_402_300_800 || value.to_string() != *text {
                        return Err(WireError("invalid time"));
                    }
                    Ok(value)
                };
                result.from = Some(time("from")?);
                result.until = Some(time("until")?);
                if result.from >= result.until {
                    return Err(WireError("invalid window"));
                }
            } else if query.contains_key("from") || query.contains_key("until") {
                return Err(WireError("bounds are irrelevant to selector"));
            }
        }
        Ok(Some(result))
    }

    /// Only the meaning of a view participates in its identity, never its
    /// current image, page cursor, peer ID, or remaining request budget.
    pub fn view_key(&self, projection: Option<Projection>) -> Result<Vec<u8>, WireError> {
        let mut query = serde_json::Map::new();
        if let Some(part) = projection {
            query.insert("parts".into(), json!(part.as_str()));
            if part.account_scoped() {
                query.insert("account".into(), json!(self.account));
            }
        } else {
            query.insert("account".into(), json!(self.account));
        }
        if let Some(period) = &self.period {
            query.insert("period".into(), json!(period));
        }
        if let Some(purpose) = &self.purpose {
            query.insert("purpose".into(), json!(purpose));
        }
        if let Some(from) = self.from {
            query.insert("from".into(), json!(from));
        }
        if let Some(until) = self.until {
            query.insert("until".into(), json!(until));
        }
        canonical_json(None, &json!([self.resource.path(), Value::Object(query)]))
    }
}

impl Target {
    pub fn transfer_key(&self) -> Result<String, super::WireError> {
        let keys = if self.resource == Resource::State {
            self.parts
                .iter()
                .map(|part| self.view_key(Some(*part)))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![self.view_key(None)?]
        };
        let mut keys = keys
            .into_iter()
            .map(|key| String::from_utf8(key).map_err(|_| super::WireError("invalid target key")))
            .collect::<Result<Vec<_>, _>>()?;
        keys.sort();
        Ok(keys.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_projections_and_period_filters_have_exact_inputs() {
        let target = Target::parse("/state?parts=runtime,main&account=account-7")
            .unwrap()
            .unwrap();
        assert_eq!(
            target.parts.into_iter().collect::<Vec<_>>(),
            [Projection::Main, Projection::Runtime]
        );
        for bad in [
            "/state",
            "/state?parts=main,main",
            "/state?parts=unused",
            "/state?parts=runtime&account=account-7",
            "/history?account=account-7",
            "/threads",
            "/history/periods?account=account-7&purpose=selectable&from=1",
            "/history/periods?account=account-7&purpose=window&from=2&until=1",
            "/state?parts=main&parts=runtime",
            "/state?parts=main&unused=1",
        ] {
            assert!(Target::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cursor_and_image_do_not_change_view_identity() {
        let left = Target::parse("/history?account=account-7&period=123")
            .unwrap()
            .unwrap();
        let right = Target::parse("/history?period=123&account=account-7&cursor=a&image=b")
            .unwrap()
            .unwrap();
        assert_eq!(left.view_key(None).unwrap(), right.view_key(None).unwrap());
    }
}

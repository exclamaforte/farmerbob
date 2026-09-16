//! The source registry: every way of getting an agent to do work.
//!
//! Parsed from `sources.toml`, which is the single place an arm's model, launcher, price and
//! eligibility live. Eligibility matters more than it looks: the registry holds several arms
//! that are the *same model* reached by different routes, one free on a plan and one metered.
//! Dispatching both as independent candidates is how this project spent real money on a model
//! it already had for free, so [`Source::eligible`] encodes the rule rather than leaving it to
//! whoever writes the next task matrix.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// Whether an arm may be dispatched, and why not when it may not.
#[derive(Debug, Clone, PartialEq)]
pub enum Eligibility {
    Ok,
    /// Explicitly turned off in the registry.
    Disabled(String),
    /// Not in a dispatchable state (`limited`, unknown status, …).
    NotDispatchable(String),
    /// A paid route to a model that is also available free and healthy.
    RedundantPaidRoute { free_arm: String, price_in: f64, price_out: f64 },
}

impl Eligibility {
    pub fn is_ok(&self) -> bool {
        matches!(self, Eligibility::Ok)
    }

    /// One line explaining a refusal, suitable for printing to a user.
    pub fn reason(&self) -> Option<String> {
        match self {
            Eligibility::Ok => None,
            Eligibility::Disabled(why) => Some(format!("disabled: {why}")),
            Eligibility::NotDispatchable(st) => Some(format!("status is `{st}`, not dispatchable")),
            Eligibility::RedundantPaidRoute { free_arm, price_in, price_out } => Some(format!(
                "costs ${price_in}/${price_out} per 1M but `{free_arm}` is the same model, \
                 free and verified"
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub launcher: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub quota: Option<String>,
    /// The bucket that actually rate-limits. For OpenRouter arms this is the *upstream*
    /// vendor, not `openrouter` — OpenRouter is a router, and it was Poolside that
    /// rate-limited us.
    #[serde(default)]
    pub quota_bucket: Option<String>,
    #[serde(default)]
    pub price_in: Option<f64>,
    #[serde(default)]
    pub price_out: Option<f64>,
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub redundant_with: Option<String>,
    #[serde(default)]
    pub disabled_reason: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

impl Source {
    /// Free on a plan, rather than metered per token.
    pub fn is_free(&self) -> bool {
        self.quota.as_deref() == Some("plan")
            || (self.price_in.unwrap_or(0.0) == 0.0 && self.price_out.unwrap_or(0.0) == 0.0)
    }

    /// The bucket to count against a per-provider concurrency cap.
    pub fn bucket(&self) -> &str {
        self.quota_bucket
            .as_deref()
            .or(self.provider.as_deref())
            .unwrap_or("unknown")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Registry {
    #[serde(rename = "source")]
    pub sources: BTreeMap<String, Source>,
}

impl Registry {
    pub fn load(path: &Path) -> Result<Registry, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
    }

    pub fn get(&self, arm: &str) -> Option<&Source> {
        self.sources.get(arm)
    }

    /// Whether `arm` may be dispatched right now.
    pub fn eligible(&self, arm: &str) -> Eligibility {
        let Some(s) = self.sources.get(arm) else {
            return Eligibility::NotDispatchable("absent from the registry".into());
        };
        if s.status == "disabled" {
            return Eligibility::Disabled(
                s.disabled_reason.clone().unwrap_or_else(|| "no reason recorded".into()),
            );
        }
        if s.status != "verified" && s.status != "untested" {
            return Eligibility::NotDispatchable(s.status.clone());
        }
        // A paid route must not run while the same model is available free and healthy.
        if let Some(free) = &s.redundant_with {
            let paid = s.price_in.unwrap_or(0.0) > 0.0;
            let free_ok = self.sources.get(free).map(|f| f.status == "verified").unwrap_or(false);
            if paid && free_ok {
                return Eligibility::RedundantPaidRoute {
                    free_arm: free.clone(),
                    price_in: s.price_in.unwrap_or(0.0),
                    price_out: s.price_out.unwrap_or(0.0),
                };
            }
        }
        Eligibility::Ok
    }

    /// Every arm that may be dispatched, cheapest first so that a caller taking the first N
    /// gets the cheapest N rather than an arbitrary N.
    pub fn dispatchable(&self) -> Vec<(&str, &Source)> {
        let mut v: Vec<(&str, &Source)> = self
            .sources
            .iter()
            .filter(|(k, _)| self.eligible(k).is_ok())
            .map(|(k, s)| (k.as_str(), s))
            .collect();
        v.sort_by(|a, b| {
            let cost = |s: &Source| s.price_in.unwrap_or(0.0) + s.price_out.unwrap_or(0.0);
            cost(a.1).partial_cmp(&cost(b.1)).unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(toml_src: &str) -> Registry {
        toml::from_str(toml_src).expect("test fixture should parse")
    }

    const FIXTURE: &str = r#"
[source.free-arm]
status = "verified"
kind = "cli"
model = "some-model"
quota = "plan"

[source.paid-twin]
status = "verified"
kind = "opencode"
model = "vendor/some-model"
quota = "metered"
price_in = 0.75
price_out = 3.75
redundant_with = "free-arm"

[source.solo-paid]
status = "verified"
kind = "opencode"
model = "vendor/other"
quota = "metered"
price_in = 0.04
price_out = 0.10

[source.switched-off]
status = "disabled"
model = "vendor/x"
disabled_reason = "superseded"

[source.half-working]
status = "limited"
model = "vendor/y"
"#;

    #[test]
    fn a_paid_route_yields_to_its_free_twin() {
        let r = reg(FIXTURE);
        match r.eligible("paid-twin") {
            Eligibility::RedundantPaidRoute { free_arm, .. } => assert_eq!(free_arm, "free-arm"),
            other => panic!("expected the paid twin to be refused, got {other:?}"),
        }
        assert!(r.eligible("free-arm").is_ok());
    }

    #[test]
    fn a_paid_arm_with_no_free_twin_is_fine() {
        assert!(reg(FIXTURE).eligible("solo-paid").is_ok());
    }

    #[test]
    fn disabled_and_limited_are_refused_with_a_reason() {
        let r = reg(FIXTURE);
        assert!(r.eligible("switched-off").reason().is_some());
        assert!(r.eligible("half-working").reason().is_some());
    }

    #[test]
    fn an_unknown_arm_is_refused_rather_than_silently_skipped() {
        assert!(!reg(FIXTURE).eligible("no-such-arm").is_ok());
    }

    #[test]
    fn a_paid_twin_becomes_eligible_when_its_free_route_is_unhealthy() {
        // the guard is about *availability*, not about price alone
        let r = reg(&FIXTURE.replace(
            "[source.free-arm]\nstatus = \"verified\"",
            "[source.free-arm]\nstatus = \"limited\"",
        ));
        assert!(r.eligible("paid-twin").is_ok(), "no free route is healthy, so paying is correct");
    }

    #[test]
    fn dispatchable_is_ordered_cheapest_first() {
        let r = reg(FIXTURE);
        let d = r.dispatchable();
        let names: Vec<&str> = d.iter().map(|(k, _)| *k).collect();
        assert_eq!(names.first(), Some(&"free-arm"), "free arms come first");
        assert!(!names.contains(&"paid-twin"));
        assert!(!names.contains(&"switched-off"));
    }

    #[test]
    fn bucket_prefers_the_upstream_vendor_over_the_router() {
        let r = reg(r#"
[source.a]
status = "verified"
model = "openrouter/poolside/laguna"
provider = "openrouter"
quota_bucket = "poolside"
"#);
        assert_eq!(r.get("a").map(|s| s.bucket()), Some("poolside"));
    }

    #[test]
    fn plan_arms_and_zero_priced_arms_both_count_as_free() {
        let r = reg(FIXTURE);
        assert!(r.get("free-arm").map(|s| s.is_free()).unwrap_or(false));
        assert!(!r.get("solo-paid").map(|s| s.is_free()).unwrap_or(true));
    }
}

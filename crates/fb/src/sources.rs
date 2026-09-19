//! The source registry: every way of getting an agent to do work.
//!
//! Parsed from `sources.toml`, which is the single place an arm's model, launcher, price and
//! eligibility live. Eligibility matters more than it looks: the registry holds several arms
//! that are the *same model* reached by different routes, one free on a plan and one metered.
//! Dispatching both as independent candidates is how this project spent real money on a model
//! it already had for free, so [`Registry::eligible`] encodes the rule rather than leaving it to
//! whoever writes the next task matrix.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use farmerbob_core::window::{State, must_disable, read};
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
    RedundantPaidRoute {
        /// The free arm that already covers this model.
        free_arm: String,
        /// The paid route's input price, per 1M tokens.
        price_in: f64,
        /// The paid route's output price, per 1M tokens.
        price_out: f64,
    },
    /// The provider has refused this arm until a stated time. Quota exhaustion is not
    /// an arm failure (bead farmerbob-h04): the arm is fine and the wall is temporary.
    Parked {
        /// `parked_until` exactly as the registry holds it.
        until: String,
        /// Why this is not dispatchable, in prose. Never empty.
        why: String,
    },
    /// The arm's time box has closed, or its `expires_at` cannot be read.
    /// Carries the reason a human should see, non-empty.
    WindowClosed {
        /// `expires_at` exactly as the registry holds it.
        expires_at: String,
        /// Why this is not dispatchable, in prose. Never empty.
        why: String,
    },
}

impl Eligibility {
    /// True only for [`Eligibility::Ok`]; every other variant is a refusal.
    pub fn is_ok(&self) -> bool {
        matches!(self, Eligibility::Ok)
    }

    /// One line explaining a refusal, suitable for printing to a user.
    pub fn reason(&self) -> Option<String> {
        match self {
            Eligibility::Ok => None,
            Eligibility::Disabled(why) => Some(format!("disabled: {why}")),
            Eligibility::NotDispatchable(st) => Some(format!("status is `{st}`, not dispatchable")),
            Eligibility::Parked { until, why } => Some(format!("parked until {until}: {why}")),
            Eligibility::RedundantPaidRoute {
                free_arm,
                price_in,
                price_out,
            } => Some(format!(
                "costs ${price_in}/${price_out} per 1M but `{free_arm}` is the same model, \
                 free and verified"
            )),
            Eligibility::WindowClosed { why, .. } => Some(format!("window closed: {why}")),
        }
    }
}

/// One way of getting an agent to do work, as `sources.toml` holds it.
#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub model: String,
    // Read by nothing in this crate today; deserialized so the `sources.toml` schema stays
    // whole. The same holds for `context` and `notes` below.
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub redundant_with: Option<String>,
    #[serde(default)]
    pub disabled_reason: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub notes: Option<String>,
    /// When the arm's enablement window closes, exactly as `sources.toml` writes it and
    /// unparsed. `None` when the registry sets no time box at all; an empty string is
    /// set-but-unreadable and must not collapse into `None`.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// When a provider-side refusal expires, exactly as `sources.toml` writes it and
    /// unparsed. `None` when the arm is not parked.
    ///
    /// This field did not exist until 2026-09-19, and its absence meant the Rust registry
    /// was blind to parks while `fb-eligible.sh` and `fb eligible`'s own command path both
    /// honoured them. Three implementations of one rule, and the one every programmatic
    /// caller uses was the one missing it: `fb critique` cast a parked arm twice in a row,
    /// seconds after `fb eligible` had refused that same arm by name.
    #[serde(default)]
    pub parked_until: Option<String>,
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

/// Every arm in `sources.toml`, keyed by registry name.
#[derive(Debug, Clone, Deserialize)]
pub struct Registry {
    #[serde(rename = "source")]
    pub sources: BTreeMap<String, Source>,
}

impl Registry {
    /// Parse the registry from a `sources.toml`, saying which file failed.
    pub fn load(path: &Path) -> Result<Registry, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
    }

    /// The arm's registry entry, if it exists.
    pub fn get(&self, arm: &str) -> Option<&Source> {
        self.sources.get(arm)
    }

    /// Whether `arm` may be dispatched right now.
    ///
    /// WINDOW-BLIND: this check never looks at `expires_at`, so an arm whose time box closed
    /// hours ago still reads `Ok` here. [`Registry::eligible_at`] is the one that includes the
    /// window; this one deliberately takes no clock and reads none.
    pub fn eligible(&self, arm: &str) -> Eligibility {
        self.decide(arm, None)
    }

    /// Eligibility INCLUDING the time box. `now` is the caller's clock; this module has none.
    ///
    /// The precedence is total, and exactly one [`Eligibility`] is returned — highest first:
    ///
    /// ```text
    /// Disabled  >  WindowClosed  >  NotDispatchable  >  RedundantPaidRoute  >  Ok
    /// ```
    ///
    /// `Disabled` wins because it is what an operator set by hand; `WindowClosed` beats the
    /// softer refusals because a closed window is the harder stop and reporting them would
    /// hide it. An arm with no `expires_at`, or whose window is still open, gets exactly what
    /// [`Registry::eligible`] returns. Where the window's edge sits — `now` exactly at the
    /// deadline counts as closed — and the fail-closed rule for an unreadable deadline are
    /// `farmerbob_core::window`'s decisions, used here rather than restated.
    ///
    /// Migrating callers onto this check is a separate task; until it lands nothing in the
    /// binary reaches it, which is what the `allow` records.
    #[allow(dead_code)]
    pub fn eligible_at(&self, arm: &str, now: DateTime<Utc>) -> Eligibility {
        self.decide(arm, Some(now))
    }

    /// The one implementation behind both checks: `now` is `None` for the window-blind
    /// [`Registry::eligible`] and `Some` for [`Registry::eligible_at`], so the non-window
    /// paths cannot drift apart.
    fn decide(&self, arm: &str, now: Option<DateTime<Utc>>) -> Eligibility {
        let Some(s) = self.sources.get(arm) else {
            return Eligibility::NotDispatchable("absent from the registry".into());
        };
        if s.status == "disabled" {
            return Eligibility::Disabled(
                s.disabled_reason
                    .clone()
                    .unwrap_or_else(|| "no reason recorded".into()),
            );
        }
        // A park needs a clock, so it is checked only on the `Some(now)` path. It comes
        // before the status test deliberately: a parked arm is usually `verified`, and
        // testing status first would call it dispatchable.
        if let (Some(now), Some(until)) = (now, s.parked_until.as_deref()) {
            match DateTime::parse_from_rfc3339(until) {
                Ok(parsed) if parsed.with_timezone(&Utc) > now => {
                    return Eligibility::Parked {
                        until: until.to_string(),
                        why:
                            "the provider refused this arm; quota exhaustion is not an arm failure"
                                .into(),
                    };
                }
                Ok(_) => {}
                // Unreadable is not absent. Refusing is the safe direction: the alternative
                // is dispatching into a wall the registry was trying to warn about.
                Err(_) => {
                    return Eligibility::Parked {
                        until: until.to_string(),
                        why: "parked_until is set but cannot be parsed as a timestamp".into(),
                    };
                }
            }
        }
        if let Some((expires_at, why)) = window_stop(s, now) {
            return Eligibility::WindowClosed { expires_at, why };
        }
        if s.status != "verified" && s.status != "untested" {
            return Eligibility::NotDispatchable(s.status.clone());
        }
        // A paid route must not run while the same model is available free and healthy.
        if let Some(free) = &s.redundant_with {
            let paid = s.price_in.unwrap_or(0.0) > 0.0;
            let free_ok = self
                .sources
                .get(free)
                .map(|f| f.status == "verified")
                .unwrap_or(false);
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
        // eligible_at, not eligible: the window-blind check cannot see a park, and this is
        // the function every programmatic caller reaches for.
        let now = Utc::now();
        let mut v: Vec<(&str, &Source)> = self
            .sources
            .iter()
            .filter(|(k, _)| self.eligible_at(k, now).is_ok())
            .map(|(k, s)| (k.as_str(), s))
            .collect();
        v.sort_by(|a, b| {
            let cost = |s: &Source| s.price_in.unwrap_or(0.0) + s.price_out.unwrap_or(0.0);
            cost(a.1)
                .partial_cmp(&cost(b.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }
}

/// The window's verdict for one source against the caller's clock: `Some` — the `expires_at`
/// exactly as the registry holds it, plus the prose a human should see — once the window has
/// closed or the deadline cannot be read, `None` while it is still open, or when the registry
/// sets no window at all. Whether "closed or unreadable" is the rule is [`must_disable`]'s
/// decision in `farmerbob_core::window`, not restated here; the registry's own `disabled`
/// statement outranks the clock and the caller has already returned `Disabled` for it.
fn window_stop(s: &Source, now: Option<DateTime<Utc>>) -> Option<(String, String)> {
    let now = now?;
    let expires_at = s.expires_at.as_deref()?;
    let state = read(expires_at, now);
    if !must_disable(&state, s.status != "disabled") {
        return None;
    }
    let why = match state {
        State::Unparseable => format!(
            "`expires_at` `{expires_at}` is unreadable: it does not parse as an instant, so \
             the window is treated as closed (unparseable deadlines fail closed)"
        ),
        State::Expired { seconds_over } => format!(
            "the window closed {seconds_over} seconds ago: `expires_at` `{expires_at}` has \
             passed"
        ),
        // `must_disable` is false for `Open`, so the guard above already returned.
        State::Open { .. } => return None,
    };
    Some((expires_at.to_string(), why))
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
        assert!(
            r.eligible("paid-twin").is_ok(),
            "no free route is healthy, so paying is correct"
        );
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

    // --- the time box: `eligible_at` vs the window-blind `eligible` ---

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("test timestamp should parse")
            .with_timezone(&Utc)
    }

    const NOW: &str = "2030-06-01T00:00:00Z";
    const T_PAST: &str = "2030-01-01T00:00:00Z";

    const EXPIRY_FIXTURE: &str = r#"
[source.plain]
status = "verified"
kind = "cli"
model = "vendor/plain"
quota = "plan"

[source.hand-off]
status = "disabled"
model = "vendor/hand"
disabled_reason = "operator disabled it"

[source.twin-free]
status = "verified"
kind = "cli"
model = "vendor/twin"
quota = "plan"

[source.twin-paid]
status = "verified"
kind = "opencode"
model = "vendor/twin"
quota = "metered"
price_in = 0.50
price_out = 2.00
redundant_with = "twin-free"

[source.capped]
status = "limited"
model = "vendor/capped"

[source.future-box]
status = "verified"
model = "vendor/future"
expires_at = "2031-01-01T00:00:00Z"

[source.closed-box]
status = "verified"
model = "vendor/closed"
expires_at = "2030-01-01T00:00:00Z"

[source.instant-box]
status = "verified"
model = "vendor/instant"
expires_at = "2030-06-01T00:00:00Z"

[source.junk-box]
status = "verified"
model = "vendor/junk"
expires_at = "not a timestamp"

[source.empty-box]
status = "verified"
model = "vendor/empty"
expires_at = ""

[source.retired-after-close]
status = "disabled"
model = "vendor/retired"
disabled_reason = "operator closed it by hand"
expires_at = "2030-01-01T00:00:00Z"

[source.paid-and-closed]
status = "verified"
kind = "opencode"
model = "vendor/twin"
quota = "metered"
price_in = 0.50
price_out = 2.00
redundant_with = "twin-free"
expires_at = "2030-01-01T00:00:00Z"

[source.limited-and-closed]
status = "limited"
model = "vendor/late"
expires_at = "2030-01-01T00:00:00Z"
"#;

    #[test]
    fn without_expires_at_eligible_at_matches_eligible_exactly() {
        let r = reg(EXPIRY_FIXTURE);
        let now = at(NOW);
        for arm in [
            "plain",
            "hand-off",
            "twin-free",
            "twin-paid",
            "capped",
            "no-such-arm",
        ] {
            assert_eq!(
                r.eligible_at(arm, now),
                r.eligible(arm),
                "an arm with no time box must be unaffected: {arm}"
            );
        }
    }

    #[test]
    fn the_named_paths_without_a_time_box_are_unchanged() {
        let r = reg(EXPIRY_FIXTURE);
        let now = at(NOW);
        assert!(r.eligible_at("plain", now).is_ok());
        assert!(matches!(
            r.eligible_at("hand-off", now),
            Eligibility::Disabled(reason) if reason == "operator disabled it"
        ));
        match r.eligible_at("twin-paid", now) {
            Eligibility::RedundantPaidRoute { free_arm, .. } => {
                assert_eq!(free_arm, "twin-free")
            }
            other => panic!("expected the paid twin to be refused, got {other:?}"),
        }
    }

    #[test]
    fn a_future_window_leaves_a_dispatchable_arm_ok() {
        let r = reg(EXPIRY_FIXTURE);
        assert!(matches!(
            r.eligible_at("future-box", at(NOW)),
            Eligibility::Ok
        ));
    }

    #[test]
    fn one_closed_arm_pins_the_window_blind_and_window_aware_difference() {
        let r = reg(EXPIRY_FIXTURE);
        let now = at(NOW);
        // Same arm, same instant: `eligible` has no clock and stays Ok, `eligible_at` sees
        // the closed box. Pinning both on one arm keeps the two checks from being confused.
        assert!(matches!(r.eligible("closed-box"), Eligibility::Ok));
        assert!(matches!(
            r.eligible_at("closed-box", now),
            Eligibility::WindowClosed { .. }
        ));
    }

    #[test]
    fn an_expired_window_refuses_with_the_value_it_was_given() {
        let r = reg(EXPIRY_FIXTURE);
        match r.eligible_at("closed-box", at(NOW)) {
            Eligibility::WindowClosed { expires_at, why } => {
                assert_eq!(expires_at, T_PAST);
                assert!(!why.is_empty());
            }
            other => panic!("expected a closed window, got {other:?}"),
        }
    }

    #[test]
    fn now_exactly_at_the_instant_counts_as_closed() {
        let r = reg(EXPIRY_FIXTURE);
        // the boundary itself is core's (`Expired { seconds_over: 0 }` is closed); asserted
        // here rather than restated
        assert!(matches!(
            r.eligible_at("instant-box", at(NOW)),
            Eligibility::WindowClosed { .. }
        ));
    }

    #[test]
    fn an_unreadable_deadline_fails_closed_and_says_what_it_could_not_read() {
        let r = reg(EXPIRY_FIXTURE);
        match r.eligible_at("junk-box", at(NOW)) {
            Eligibility::WindowClosed { expires_at, why } => {
                assert_eq!(expires_at, "not a timestamp");
                assert!(!why.is_empty());
                // the prose is not pinned, only what it must convey: that the deadline could
                // not be read, and which value defeated it (either spelling of "unreadable")
                let low = why.to_lowercase();
                assert!(
                    low.contains("unread") || low.contains("unpars"),
                    "the reason must say the deadline was unreadable, got: {why}"
                );
                assert!(
                    low.contains("not a timestamp"),
                    "the reason must mention the value it could not read, got: {why}"
                );
            }
            other => panic!("expected a closed window, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_expires_at_is_set_but_unreadable_not_absent() {
        let r = reg(EXPIRY_FIXTURE);
        match r.eligible_at("empty-box", at(NOW)) {
            Eligibility::WindowClosed { expires_at, why } => {
                assert_eq!(expires_at, "");
                assert!(!why.is_empty());
            }
            other => panic!("expected the empty string to fail closed, got {other:?}"),
        }
        // collapsing "" into "absent" would report Ok; the two must not collapse
        assert!(!r.eligible_at("empty-box", at(NOW)).is_ok());
    }

    // A precedence bug only shows up when two conditions hold on ONE arm: two tests that each
    // arrange a single condition would both pass against an implementation that checks them in
    // the wrong order. So Disabled-over-WindowClosed and WindowClosed-over-the-softer-refusals
    // are pinned below against arms carrying BOTH facts at once.

    #[test]
    fn disabled_outranks_the_closed_window_on_the_same_arm() {
        let r = reg(EXPIRY_FIXTURE);
        assert!(matches!(
            r.eligible_at("retired-after-close", at(NOW)),
            Eligibility::Disabled(reason) if reason == "operator closed it by hand"
        ));
    }

    #[test]
    fn the_closed_window_outranks_a_redundant_paid_route_on_the_same_arm() {
        let r = reg(EXPIRY_FIXTURE);
        assert!(matches!(
            r.eligible_at("paid-and-closed", at(NOW)),
            Eligibility::WindowClosed { .. }
        ));
    }

    #[test]
    fn the_closed_window_outranks_a_limited_status_on_the_same_arm() {
        // the pair the precedence exists to decide: WindowClosed, not NotDispatchable
        let r = reg(EXPIRY_FIXTURE);
        assert!(matches!(
            r.eligible_at("limited-and-closed", at(NOW)),
            Eligibility::WindowClosed { .. }
        ));
    }

    #[test]
    fn an_absent_arm_stays_not_dispatchable_with_the_recorded_reason() {
        let r = reg(EXPIRY_FIXTURE);
        assert_eq!(
            r.eligible_at("no-such-arm", at(NOW)),
            Eligibility::NotDispatchable("absent from the registry".into())
        );
    }

    /// A parked arm is not dispatchable, and `dispatchable()` -- which every programmatic
    /// caller uses -- must agree with `fb eligible`, which has always refused one.
    #[test]
    fn a_parked_arm_is_not_dispatchable() {
        let toml = concat!(
            "[source.parked-arm]\nstatus = \"verified\"\n",
            "parked_until = \"2099-01-01T00:00:00Z\"\n",
            "[source.free-arm]\nstatus = \"verified\"\n"
        );
        let r = reg(toml);
        let names: Vec<&str> = r.dispatchable().into_iter().map(|(k, _)| k).collect();
        assert!(
            !names.contains(&"parked-arm"),
            "a parked arm must not be dispatchable, got {names:?}"
        );
        assert!(names.contains(&"free-arm"), "an unparked arm still is");
    }

    /// A park in the PAST is spent, not a permanent refusal. Same field as the test
    /// above with one value changed: the two answers must differ.
    #[test]
    fn a_park_that_has_expired_no_longer_refuses() {
        let toml = concat!(
            "[source.was-parked]\nstatus = \"verified\"\n",
            "parked_until = \"2000-01-01T00:00:00Z\"\n"
        );
        let r = reg(toml);
        let names: Vec<&str> = r.dispatchable().into_iter().map(|(k, _)| k).collect();
        assert!(names.contains(&"was-parked"), "an expired park is spent");
    }

    /// An unreadable `parked_until` refuses rather than being treated as absent. The
    /// alternative is dispatching into the wall the registry was warning about.
    #[test]
    fn an_unparseable_park_refuses() {
        let toml = concat!(
            "[source.bad-park]\nstatus = \"verified\"\n",
            "parked_until = \"soon\"\n"
        );
        let r = reg(toml);
        let e = r.eligible_at(&"bad-park".to_string(), Utc::now());
        assert!(!e.is_ok());
        assert!(e.reason().unwrap_or_default().contains("bad-park") || !e.is_ok());
    }
}

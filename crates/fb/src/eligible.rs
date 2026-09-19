//! The dispatch guard formerly implemented by `fb-eligible.sh`.

use farmerbob_core::gate::{Observation, Verdict, judge};
use farmerbob_core::measurement::Measurement;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Why an arm may or may not be dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    /// Dispatchable, and proven: it has completed a scored run.
    Verified,
    /// Dispatchable, but never run here. Carries nothing -- the absence IS
    /// the fact.
    Untested,
    /// Refused permanently. Carries the registry's stated reason.
    Disabled(String),
    /// Refused until a stated time. Carries that time as written.
    Parked(String),
    /// Not in the registry at all. This is a TYPO, not a policy decision,
    /// and callers treat it differently.
    Unregistered,
}

/// Classify one arm from its registry entry.
///
/// `status` and `parked_until` are the registry's values; `None` for either
/// means the field was absent.
pub fn classify(
    status: Option<&str>,
    parked_until: Option<&str>,
    reason: Option<&str>,
) -> Eligibility {
    let Some(st) = status else {
        return Eligibility::Unregistered;
    };

    match st {
        "disabled" => {
            let reason = reason.unwrap_or("no reason recorded");
            Eligibility::Disabled(reason.to_string())
        }
        "verified" => {
            if let Some(parked) = parked_until {
                Eligibility::Parked(parked.to_string())
            } else {
                Eligibility::Verified
            }
        }
        "untested" => {
            if let Some(parked) = parked_until {
                Eligibility::Parked(parked.to_string())
            } else {
                Eligibility::Untested
            }
        }
        other => Eligibility::Disabled(format!("status is `{other}`, not dispatchable")),
    }
}

/// Whether this arm may be dispatched now.
pub fn dispatchable(e: &Eligibility) -> bool {
    matches!(e, Eligibility::Verified | Eligibility::Untested)
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryFile {
    source: std::collections::BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Deserialize)]
struct Entry {
    status: Option<String>,
    disabled_reason: Option<String>,
    parked_until: Option<String>,
    redundant_with: Option<String>,
    price_in: Option<f64>,
    price_out: Option<f64>,
}

fn read_registry(path: &Path) -> Measurement<RegistryFile> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            return Measurement::instrument_failed(&format!("cannot read registry: {error}"));
        }
    };
    match toml::from_str(&text) {
        Ok(registry) => Measurement::observed(registry),
        Err(error) => Measurement::instrument_failed(&format!("cannot parse registry: {error}")),
    }
}

fn price(value: Option<f64>, field: &str) -> Measurement<f64> {
    match value {
        Some(value) => Measurement::observed(value),
        None => Measurement::instrument_failed(&format!("{field} is absent")),
    }
}

fn timestamp(value: &str) -> Measurement<i64> {
    let (date, time) = match value.split_once('T').or_else(|| value.split_once(' ')) {
        Some(parts) => parts,
        None => return Measurement::instrument_failed("parked_until is not a valid timestamp"),
    };
    let mut date_parts = date.split('-');
    let (year, month, day) = match (date_parts.next(), date_parts.next(), date_parts.next()) {
        (Some(y), Some(m), Some(d)) => match (y.parse::<i64>(), m.parse::<i64>(), d.parse::<i64>())
        {
            (Ok(y), Ok(m), Ok(d)) => (y, m, d),
            _ => return Measurement::instrument_failed("parked_until is not a valid timestamp"),
        },
        _ => return Measurement::instrument_failed("parked_until is not a valid timestamp"),
    };
    let (clock, offset) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, 0_i64)
    } else {
        let split = time.rfind(['+', '-']);
        let Some(index) = split else {
            return Measurement::instrument_failed("parked_until is not a valid timestamp");
        };
        let (clock, zone) = time.split_at(index);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let zone = &zone[1..];
        let mut parts = zone.split(':');
        let (hours, minutes) = match (parts.next(), parts.next()) {
            (Some(h), Some(m)) => match (h.parse::<i64>(), m.parse::<i64>()) {
                (Ok(h), Ok(m)) => (h, m),
                _ => {
                    return Measurement::instrument_failed("parked_until is not a valid timestamp");
                }
            },
            _ => return Measurement::instrument_failed("parked_until is not a valid timestamp"),
        };
        (clock, sign * (hours * 3600 + minutes * 60))
    };
    let mut clock_parts = clock.split(':');
    let (hour, minute, second) = match (clock_parts.next(), clock_parts.next(), clock_parts.next())
    {
        (Some(h), Some(m), Some(s)) => {
            let second = match s.split('.').next() {
                Some(second) => second,
                None => {
                    return Measurement::instrument_failed("parked_until is not a valid timestamp");
                }
            };
            match (h.parse::<i64>(), m.parse::<i64>(), second.parse::<i64>()) {
                (Ok(h), Ok(m), Ok(s)) => (h, m, s),
                _ => {
                    return Measurement::instrument_failed("parked_until is not a valid timestamp");
                }
            }
        }
        _ => return Measurement::instrument_failed("parked_until is not a valid timestamp"),
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    if day < 1 || day > days_in_month || hour > 23 || minute > 59 || second > 59 {
        return Measurement::instrument_failed("parked_until is not a valid timestamp");
    }
    // Days from the civil calendar, valid for the modern dates in the registry.
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let month_adjusted = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_adjusted + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146097 + day_of_era - 719468;
    Measurement::observed(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

fn now_seconds() -> Measurement<i64> {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => Measurement::observed(duration.as_secs() as i64),
        Err(error) => Measurement::instrument_failed(&format!(
            "system clock is before the Unix epoch: {error}"
        )),
    }
}

fn decide(arm: &str, path: &Path) -> Measurement<bool> {
    let registry = match read_registry(path) {
        Measurement::Observed(registry) => registry,
        Measurement::Missing(reason) => return Measurement::Missing(reason),
    };
    let Some(entry) = registry.source.get(arm) else {
        let e = classify(None, None, None);
        eprintln!("{arm}: not in the registry");
        return Measurement::observed(dispatchable(&e));
    };

    let parked_str = entry.parked_until.as_deref();
    let eligibility = classify(
        entry.status.as_deref(),
        parked_str,
        entry.disabled_reason.as_deref(),
    );

    match &eligibility {
        Eligibility::Disabled(reason) => {
            eprintln!("{arm}: disabled -- {reason}");
            return Measurement::observed(dispatchable(&eligibility));
        }
        Eligibility::Unregistered => {
            eprintln!("{arm}: not in the registry");
            return Measurement::observed(dispatchable(&eligibility));
        }
        Eligibility::Parked(parked) => {
            let until = timestamp(parked);
            let now = now_seconds();
            match (&until, &now) {
                (Measurement::Observed(until), Measurement::Observed(now)) if until > now => {
                    let left = until - now;
                    eprintln!(
                        "{arm}: parked until {parked} ({}d{}h left) -- quota exhausted, not an arm failure",
                        left / 86_400,
                        (left % 86_400) / 3600
                    );
                    return Measurement::observed(dispatchable(&eligibility));
                }
                (Measurement::Missing(_), _) => {
                    eprintln!("{arm}: parked_until is not a valid timestamp: {parked:?}");
                    return Measurement::observed(dispatchable(&eligibility));
                }
                _ => {
                    // Park has expired: arm is not currently parked.
                }
            }
        }
        Eligibility::Verified | Eligibility::Untested => {}
    }

    let paid = price(entry.price_in, "price_in");
    if let (Some(free), Measurement::Observed(price_in)) = (entry.redundant_with.as_deref(), &paid)
        && *price_in > 0.0
        && let Some(free_entry) = registry.source.get(free)
        && classify(free_entry.status.as_deref(), None, None) == Eligibility::Verified
    {
        let price_out = match price(entry.price_out, "price_out") {
            Measurement::Observed(value) => value,
            Measurement::Missing(reason) => return Measurement::Missing(reason),
        };
        eprintln!(
            "{arm}: ${price_in}/{price_out} but {free} is the same model, free and verified -- use that (set FB_ALLOW_PAID_DUPES=1 to compare harnesses deliberately)"
        );
        return Measurement::observed(false);
    }

    // If it was parked but the park has expired, evaluate without park.
    let final_eligibility = if matches!(eligibility, Eligibility::Parked(_)) {
        classify(
            entry.status.as_deref(),
            None,
            entry.disabled_reason.as_deref(),
        )
    } else {
        eligibility
    };

    Measurement::observed(dispatchable(&final_eligibility))
}

/// Run the eligibility predicate using the repository's registry.
pub fn run_cmd(arm: &str) -> i32 {
    // The repo root, not a relative path. The script reads "$repo/sources.toml" with $repo
    // absolute, and this port shipped `Path::new("sources.toml")` -- which works from the
    // repo and from nowhere else. Run from any other directory it answered "cannot read
    // registry" for EVERY arm, so a dispatcher whose cwd differed would have benched the
    // whole fleet at once.
    //
    // It fails closed, which is the one mercy: an unreadable registry refuses everything
    // rather than permitting everything, so this could never have caused a dispatch that
    // should not have happened. Found by extending the differential past its declared cases,
    // which is also where the missing `$price_in` came from.
    run_cmd_with_path(arm, &registry_path())
}

/// Locate `sources.toml` the way the script does -- by an absolute path that does not depend
/// on where the process was started.
///
/// `paths::repo()` is NOT enough on its own: it falls back to the current directory, so it
/// reproduces the very bug this resolves. `$FB_REPO` wins when set, otherwise walk up from
/// the working directory until a `sources.toml` appears, which finds the repo from any
/// subdirectory of it. The final fallback is the plain relative path, so the failure mode when
/// nothing is found is the one we already understand: refuse every arm, loudly.
fn registry_path() -> PathBuf {
    if let Some(root) = std::env::var_os("FB_REPO") {
        return PathBuf::from(root).join("sources.toml");
    }
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let candidate = dir.join("sources.toml");
        if candidate.is_file() {
            return candidate;
        }
        if !dir.pop() {
            return PathBuf::from("sources.toml");
        }
    }
}

fn run_cmd_with_path(arm: &str, path: &Path) -> i32 {
    let measured = decide(arm, path);
    let verdict = match measured {
        Measurement::Observed(eligible) => judge(&Observation {
            built: Some(true),
            tests_passed: Some(eligible),
            tests_run: Some(1),
            lines_added: Some(1),
            declared_targets_present: Some(true),
            // Synthetic observation: this call site uses `judge` as a boolean combinator,
            // not to score a run, so there is no worktree and no scope to depart from.
            // Some(0) rather than None DELIBERATELY -- None means "scope was not assessed"
            // and yields Indeterminate, which here would turn a correct answer into a
            // refusal to answer. That these sites exist at all is the N-of-N finding from
            // port-eligible: my spec told every arm to route pass/fail through gate::judge,
            // and eligibility is not a run verdict.
            scope_departures: Some(0),
        }),
        Measurement::Missing(reason) => {
            eprintln!(
                "{arm}: {}",
                match reason {
                    farmerbob_core::measurement::Absent::InstrumentFailed { reason } => reason,
                    _ => "eligibility could not be measured".to_string(),
                }
            );
            Verdict::Indeterminate
        }
    };
    if verdict.is_pass() { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clause_1_verified_no_park() {
        let e = classify(Some("verified"), None, None);
        assert_eq!(e, Eligibility::Verified);
        assert!(dispatchable(&e));
    }

    #[test]
    fn clause_2_untested_no_park() {
        let e = classify(Some("untested"), None, None);
        assert_eq!(e, Eligibility::Untested);
        assert!(dispatchable(&e));
    }

    #[test]
    fn clause_3_disabled_carries_reason() {
        let e = classify(Some("disabled"), None, Some("sunset"));
        assert_eq!(e, Eligibility::Disabled("sunset".to_string()));
        assert!(!dispatchable(&e));
    }

    #[test]
    fn clause_4_park_outranks_verified() {
        let e = classify(Some("verified"), Some("2026-09-20T00:00:00Z"), None);
        assert_eq!(e, Eligibility::Parked("2026-09-20T00:00:00Z".to_string()));
        assert!(!dispatchable(&e));
    }

    #[test]
    fn clause_5_status_none_is_unregistered() {
        let e = classify(None, None, None);
        assert_eq!(e, Eligibility::Unregistered);
        assert!(!dispatchable(&e));
        assert_ne!(e, Eligibility::Disabled("reason".to_string()));
    }

    #[test]
    fn clause_6_unrecognised_status_refused() {
        let e = classify(Some("retired"), None, None);
        assert_ne!(e, Eligibility::Verified);
        assert!(!dispatchable(&e));
    }

    #[test]
    fn clause_7_disabled_none_reason_yields_non_empty() {
        let e = classify(Some("disabled"), None, None);
        match &e {
            Eligibility::Disabled(reason) => assert!(!reason.is_empty()),
            _ => panic!("expected Disabled variant"),
        }
        assert!(!dispatchable(&e));
    }

    #[test]
    fn boundaries_empty_status_and_disabled_park_collision() {
        let e_empty = classify(Some(""), None, None);
        assert_ne!(e_empty, Eligibility::Verified);
        assert!(!dispatchable(&e_empty));

        let e_collision = classify(
            Some("disabled"),
            Some("2026-09-20T00:00:00Z"),
            Some("provider down"),
        );
        assert_eq!(
            e_collision,
            Eligibility::Disabled("provider down".to_string())
        );
        assert!(!dispatchable(&e_collision));
    }

    #[test]
    fn clause_8_run_cmd_distinguishes_verified_and_disabled() {
        let verified_code = run_cmd("oc-muse-spark");
        let disabled_code = run_cmd("oc-deepseek-v4-pro");
        assert_eq!(verified_code, 0);
        assert_eq!(disabled_code, 1);
        assert_ne!(verified_code, disabled_code);
    }
}

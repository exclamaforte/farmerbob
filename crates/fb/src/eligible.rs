//! The dispatch guard formerly implemented by `fb-eligible.sh`.

use farmerbob_core::gate::{Observation, Verdict, judge};
use farmerbob_core::measurement::Measurement;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    Disabled,
    Verified,
    Untested,
    Other(Option<String>),
}

impl Status {
    fn parse(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("disabled") => Self::Disabled,
            Some("verified") => Self::Verified,
            Some("untested") => Self::Untested,
            _ => Self::Other(value),
        }
    }

    fn display(&self) -> &str {
        match self {
            Self::Disabled => "disabled",
            Self::Verified => "verified",
            Self::Untested => "untested",
            Self::Other(Some(value)) => value,
            Self::Other(None) => "None",
        }
    }
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
        eprintln!("{arm}: not in the registry");
        return Measurement::observed(false);
    };
    let status = Status::parse(entry.status.clone());
    if status == Status::Disabled {
        let reason = entry
            .disabled_reason
            .as_deref()
            .unwrap_or("no reason recorded");
        eprintln!("{arm}: disabled -- {reason}");
        return Measurement::observed(false);
    }
    if !matches!(status, Status::Verified | Status::Untested) {
        eprintln!("{arm}: status={}, not dispatchable", status.display());
        return Measurement::observed(false);
    }
    if let Some(parked) = entry.parked_until.as_deref() {
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
                return Measurement::observed(false);
            }
            (Measurement::Missing(_), _) => {
                eprintln!("{arm}: parked_until is not a valid timestamp: {parked:?}");
                return Measurement::observed(false);
            }
            _ => {}
        }
    }
    let paid = price(entry.price_in, "price_in");
    if let (Some(free), Measurement::Observed(price_in)) = (entry.redundant_with.as_deref(), &paid)
        && *price_in > 0.0
        && let Some(free_entry) = registry.source.get(free)
        && Status::parse(free_entry.status.clone()) == Status::Verified
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
    Measurement::observed(true)
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
    fn missing_and_disabled_have_distinct_decisions() {
        let path =
            std::env::temp_dir().join(format!("fb-eligible-{}-{}.toml", std::process::id(), 1));
        std::fs::write(&path, "[source.off]\nstatus=\"disabled\"\n").unwrap();
        assert_eq!(run_cmd_with_path("absent", &path), 1);
        assert_eq!(run_cmd_with_path("off", &path), 1);
        std::fs::remove_file(path).unwrap();
    }
}

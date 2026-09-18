//! Availability and neglected arm detection.
//!
//! An arm in the registry can be ready, parked, unverified, or disabled. When an arm's
//! park expires, the registry condition has passed, but until the arm is dispatched or
//! revisited, the registry row still asserts an expired park. This module distinguishes
//! [`Availability::ParkExpired`] from [`Availability::Ready`]: both are dispatchable, but
//! an expired park signifies that the provider claim outlived its condition and was never
//! checked.
//!
//! Furthermore, an arm that is dispatchable but has not been dispatched recently is
//! considered [`Neglected`]. An arm that has never run carries [`LastSeen::Never`],
//! which is distinct from an ancient timestamp and must never be reported as one.

use std::collections::{BTreeMap, HashMap};

/// One registry row, reduced to what availability depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arm {
    /// The arm's id, e.g. `"or-nemotron-ultra"`.
    pub name: String,
    /// The registry's `status` field verbatim: `"verified"`, `"disabled"`, `"untested"`, ...
    pub status: String,
    /// `parked_until` as epoch milliseconds, when the row carries one.
    pub parked_until_ms: Option<u64>,
}

/// Whether an arm can be dispatched, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// Dispatchable now.
    Ready,
    /// `status` is `"disabled"`. A decision, not a condition; it does not expire.
    Disabled,
    /// `status` is neither `"verified"` nor `"disabled"`.
    Unverified,
    /// `parked_until` is still in the future.
    Parked,
    /// `parked_until` has PASSED. Dispatchable, and distinct from [`Availability::Ready`]:
    /// the row still asserts a park, so the claim outlived the condition and
    /// nothing has revisited it.
    ParkExpired,
}

/// Classify one arm.
///
/// Status strings are an open set: only `"verified"` and `"disabled"` carry meaning here,
/// and every other value (including new statuses or variations like `"Verified"`) is
/// [`Availability::Unverified`]. Falling back to `Unverified` rather than `Ready` is
/// deliberate: an unrecognised status must not become dispatchable by default.
///
/// Precedence rules:
/// 1. `status == "disabled"` yields [`Availability::Disabled`] regardless of `parked_until_ms`.
///    A disabled arm is a decision and a park is a condition; the decision wins and does not expire.
/// 2. `status` neither `"verified"` nor `"disabled"` yields [`Availability::Unverified`],
///    again regardless of `parked_until_ms`. Comparison is exact (`"Verified"` is not `"verified"`).
/// 3. For a `"verified"` arm:
///    - `parked_until_ms == None` yields [`Availability::Ready`].
///    - `parked_until_ms > now_ms` yields [`Availability::Parked`].
///    - `parked_until_ms <= now_ms` yields [`Availability::ParkExpired`] (the park has elapsed).
pub fn availability(arm: &Arm, now_ms: u64) -> Availability {
    if arm.status == "disabled" {
        Availability::Disabled
    } else if arm.status != "verified" {
        Availability::Unverified
    } else {
        match arm.parked_until_ms {
            None => Availability::Ready,
            Some(until) if until > now_ms => Availability::Parked,
            Some(_) => Availability::ParkExpired,
        }
    }
}

/// Whether this availability permits dispatch. `Ready` and `ParkExpired` do.
pub fn dispatchable(a: Availability) -> bool {
    matches!(a, Availability::Ready | Availability::ParkExpired)
}

/// How long since an arm was last dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastSeen {
    /// Dispatched this many milliseconds ago.
    Ago(u64),
    /// Present in the registry and never dispatched at all. NOT the same as a
    /// very old timestamp, and must never be reported as one.
    Never,
}

/// An arm that could run and has not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Neglected {
    /// The arm's id.
    pub name: String,
    /// Why it is dispatchable: [`Availability::Ready`] or [`Availability::ParkExpired`].
    pub availability: Availability,
    /// When it last ran.
    pub last_seen: LastSeen,
}

/// Every dispatchable arm that has not run within `stale_after_ms`.
///
/// `last_dispatch` maps an arm name to the epoch-millisecond time it last ran.
/// A name absent from it has never run.
///
/// # Duplicates and Boundaries
///
/// - Only arms for which [`dispatchable`] is true can be neglected. Parked, disabled,
///   and unverified arms are never returned.
/// - If `arms` contains duplicate rows with the same `name`, the first occurrence in `arms`
///   is described and subsequent duplicates are ignored.
/// - Returns entries sorted by `name` in ascending byte order, with no duplicate names.
/// - An arm absent from `last_dispatch` is neglected and carries [`LastSeen::Never`].
/// - If the same name appears multiple times in `last_dispatch`, the maximum timestamp
///   (the latest dispatch time) wins.
/// - A timestamp in `last_dispatch` that is in the future (`t > now_ms`) indicates recent
///   activity; the arm is not neglected, avoiding underflow in `LastSeen`.
/// - An arm dispatched at least `stale_after_ms` ago (`now_ms - t >= stale_after_ms`) is
///   neglected and carries [`LastSeen::Ago(now_ms - t)`].
/// - When `stale_after_ms == 0`, every dispatchable arm is neglected, including one dispatched
///   at exactly `now_ms` with [`LastSeen::Ago`]`(0)`.
/// - An arm dispatched at exactly `now_ms - stale_after_ms` is neglected.
/// - An arm dispatched more recently than `stale_after_ms` (`now_ms - t < stale_after_ms`)
///   is not neglected.
/// - Names in `last_dispatch` that do not match any arm in `arms` are ignored.
/// - An empty `arms` slice returns an empty vector.
pub fn neglected(
    arms: &[Arm],
    last_dispatch: &[(&str, u64)],
    now_ms: u64,
    stale_after_ms: u64,
) -> Vec<Neglected> {
    if arms.is_empty() {
        return Vec::new();
    }

    let mut unique_arms: BTreeMap<&str, &Arm> = BTreeMap::new();
    for arm in arms {
        unique_arms.entry(arm.name.as_str()).or_insert(arm);
    }

    let mut dispatches: HashMap<&str, u64> = HashMap::new();
    for &(name, timestamp) in last_dispatch {
        dispatches
            .entry(name)
            .and_modify(|t| *t = (*t).max(timestamp))
            .or_insert(timestamp);
    }

    let mut result = Vec::new();

    for (name, arm) in unique_arms {
        let avail = availability(arm, now_ms);
        if !dispatchable(avail) {
            continue;
        }

        match dispatches.get(name) {
            None => {
                result.push(Neglected {
                    name: arm.name.clone(),
                    availability: avail,
                    last_seen: LastSeen::Never,
                });
            }
            Some(&last_t) => {
                if last_t > now_ms {
                    continue;
                }
                let elapsed = now_ms - last_t;
                if elapsed >= stale_after_ms {
                    result.push(Neglected {
                        name: arm.name.clone(),
                        availability: avail,
                        last_seen: LastSeen::Ago(elapsed),
                    });
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(name: &str, status: &str, parked_until_ms: Option<u64>) -> Arm {
        Arm {
            name: name.to_string(),
            status: status.to_string(),
            parked_until_ms,
        }
    }

    // Clause 1: status == "disabled" yields Disabled whatever parked_until_ms says.
    #[test]
    fn clause1_disabled_without_park_is_disabled() {
        let a = arm("a1", "disabled", None);
        assert_eq!(availability(&a, 1000), Availability::Disabled);
    }

    #[test]
    fn clause1_disabled_with_future_park_is_disabled() {
        let a = arm("a1", "disabled", Some(2000));
        assert_eq!(availability(&a, 1000), Availability::Disabled);
    }

    #[test]
    fn clause1_disabled_with_past_park_is_disabled() {
        let a = arm("a1", "disabled", Some(500));
        assert_eq!(availability(&a, 1000), Availability::Disabled);
    }

    #[test]
    fn clause1_disabled_with_boundary_now_park_is_disabled() {
        let a = arm("a1", "disabled", Some(1000));
        assert_eq!(availability(&a, 1000), Availability::Disabled);
    }

    // Clause 2: status neither "verified" nor "disabled" yields Unverified, whatever the park says.
    #[test]
    fn clause2_untested_status_is_unverified_regardless_of_park() {
        let a_none = arm("a1", "untested", None);
        let a_future = arm("a1", "untested", Some(2000));
        let a_past = arm("a1", "untested", Some(500));
        assert_eq!(availability(&a_none, 1000), Availability::Unverified);
        assert_eq!(availability(&a_future, 1000), Availability::Unverified);
        assert_eq!(availability(&a_past, 1000), Availability::Unverified);
    }

    #[test]
    fn clause2_status_comparison_is_exact_titlecase_is_unverified() {
        let a = arm("a1", "Verified", None);
        assert_eq!(availability(&a, 1000), Availability::Unverified);
    }

    #[test]
    fn clause2_status_comparison_is_exact_uppercase_is_unverified() {
        let a = arm("a1", "DISABLED", None);
        assert_eq!(availability(&a, 1000), Availability::Unverified);
    }

    #[test]
    fn clause2_unknown_status_is_unverified() {
        let a = arm("a1", "provisional_status_future", Some(500));
        assert_eq!(availability(&a, 1000), Availability::Unverified);
    }

    #[test]
    fn clause2_empty_status_is_unverified() {
        let a = arm("a1", "", None);
        assert_eq!(availability(&a, 1000), Availability::Unverified);
    }

    // Clause 3: A verified arm with parked_until_ms in the future yields Parked.
    #[test]
    fn clause3_verified_with_future_park_is_parked() {
        let a = arm("a1", "verified", Some(1500));
        assert_eq!(availability(&a, 1000), Availability::Parked);
    }

    // Clause 4: A verified arm with parked_until_ms in the past yields ParkExpired, not Ready.
    #[test]
    fn clause4_verified_with_past_park_is_park_expired_not_ready() {
        let a = arm("a1", "verified", Some(800));
        assert_eq!(availability(&a, 1000), Availability::ParkExpired);
    }

    // Boundary: parked_until_ms EXACTLY equal to now_ms yields ParkExpired.
    #[test]
    fn boundary_park_exactly_at_now_is_park_expired() {
        let a = arm("a1", "verified", Some(1000));
        assert_eq!(availability(&a, 1000), Availability::ParkExpired);
    }

    // Clause 5: A verified arm with parked_until_ms: None yields Ready.
    #[test]
    fn clause5_verified_without_park_is_ready() {
        let a = arm("a1", "verified", None);
        assert_eq!(availability(&a, 1000), Availability::Ready);
    }

    // Clause 6: dispatchable is true for Ready and ParkExpired, false for the other three. Pin all five.
    #[test]
    fn clause6_dispatchable_pins_all_five_variants() {
        assert!(dispatchable(Availability::Ready));
        assert!(dispatchable(Availability::ParkExpired));
        assert!(!dispatchable(Availability::Disabled));
        assert!(!dispatchable(Availability::Unverified));
        assert!(!dispatchable(Availability::Parked));
    }

    // Boundary: An EMPTY arms slice: neglected returns an empty vector.
    #[test]
    fn boundary_empty_arms_returns_empty_vector() {
        let result = neglected(&[], &[("a1", 500)], 1000, 100);
        assert!(result.is_empty());
    }

    // Clause 7: neglected returns only arms for which dispatchable is true.
    // Parked, disabled, and unverified arms are never neglected.
    #[test]
    fn clause7_parked_arm_is_never_neglected() {
        let arms = [arm("parked_arm", "verified", Some(2000))];
        let result = neglected(&arms, &[], 1000, 100);
        assert!(result.is_empty());
    }

    #[test]
    fn clause7_disabled_arm_is_never_neglected() {
        let arms = [arm("disabled_arm", "disabled", None)];
        let result = neglected(&arms, &[], 1000, 100);
        assert!(result.is_empty());
    }

    #[test]
    fn clause7_unverified_arm_is_never_neglected() {
        let arms = [arm("unverified_arm", "untested", None)];
        let result = neglected(&arms, &[], 1000, 100);
        assert!(result.is_empty());
    }

    // Clause 8: An arm in last_dispatch whose timestamp is at least stale_after_ms old
    // is neglected and carries LastSeen::Ago(now_ms - t).
    #[test]
    fn clause8_stale_dispatch_is_neglected_with_ago() {
        let arms = [arm("or-nemotron-ultra", "verified", None)];
        let dispatches = [("or-nemotron-ultra", 700)];
        let result = neglected(&arms, &dispatches, 1000, 200);
        assert_eq!(
            result,
            vec![Neglected {
                name: "or-nemotron-ultra".to_string(),
                availability: Availability::Ready,
                last_seen: LastSeen::Ago(300),
            }]
        );
    }

    // Clause 9: An arm absent from last_dispatch is neglected and carries LastSeen::Never,
    // not Ago(now_ms) and not Ago(u64::MAX).
    #[test]
    fn clause9_absent_arm_is_neglected_with_never() {
        let arms = [arm("fresh-arm", "verified", None)];
        let result = neglected(&arms, &[], 1000, 100);
        assert_eq!(
            result,
            vec![Neglected {
                name: "fresh-arm".to_string(),
                availability: Availability::Ready,
                last_seen: LastSeen::Never,
            }]
        );
    }

    // Clause 10: An arm dispatched more recently than stale_after_ms is NOT neglected.
    #[test]
    fn clause10_recent_dispatch_is_not_neglected() {
        let arms = [arm("active-arm", "verified", None)];
        let dispatches = [("active-arm", 950)];
        let result = neglected(&arms, &dispatches, 1000, 100);
        assert!(result.is_empty());
    }

    // Boundary: stale_after_ms == 0: every dispatchable arm is neglected, including one
    // dispatched at exactly now_ms.
    #[test]
    fn boundary_stale_after_zero_neglects_dispatch_at_now() {
        let arms = [arm("now-arm", "verified", None)];
        let dispatches = [("now-arm", 1000)];
        let result = neglected(&arms, &dispatches, 1000, 0);
        assert_eq!(
            result,
            vec![Neglected {
                name: "now-arm".to_string(),
                availability: Availability::Ready,
                last_seen: LastSeen::Ago(0),
            }]
        );
    }

    #[test]
    fn boundary_stale_after_zero_neglects_all_dispatchable_arms() {
        let arms = [
            arm("arm-never", "verified", None),
            arm("arm-expired", "verified", Some(500)),
        ];
        let dispatches = [("arm-expired", 1000)];
        let result = neglected(&arms, &dispatches, 1000, 0);
        assert_eq!(
            result,
            vec![
                Neglected {
                    name: "arm-expired".to_string(),
                    availability: Availability::ParkExpired,
                    last_seen: LastSeen::Ago(0),
                },
                Neglected {
                    name: "arm-never".to_string(),
                    availability: Availability::Ready,
                    last_seen: LastSeen::Never,
                },
            ]
        );
    }

    // Boundary: An arm dispatched at EXACTLY now_ms - stale_after_ms: neglected.
    #[test]
    fn boundary_dispatch_exactly_at_stale_threshold_is_neglected() {
        let arms = [arm("edge-arm", "verified", None)];
        let dispatches = [("edge-arm", 800)]; // now_ms (1000) - stale (200) = 800
        let result = neglected(&arms, &dispatches, 1000, 200);
        assert_eq!(
            result,
            vec![Neglected {
                name: "edge-arm".to_string(),
                availability: Availability::Ready,
                last_seen: LastSeen::Ago(200),
            }]
        );
    }

    #[test]
    fn boundary_dispatch_one_ms_after_stale_threshold_is_not_neglected() {
        let arms = [arm("edge-arm", "verified", None)];
        let dispatches = [("edge-arm", 801)]; // elapsed = 199 < 200
        let result = neglected(&arms, &dispatches, 1000, 200);
        assert!(result.is_empty());
    }

    // Boundary: A timestamp in last_dispatch that is in the FUTURE (greater than now_ms):
    // the arm is not neglected, and LastSeen must not underflow.
    #[test]
    fn boundary_future_timestamp_is_not_neglected_and_does_not_underflow() {
        let arms = [arm("future-arm", "verified", None)];
        let dispatches = [("future-arm", 1500)];
        let result = neglected(&arms, &dispatches, 1000, 100);
        assert!(result.is_empty());
    }

    // Boundary: A name in last_dispatch matching no arm: ignored, not an error.
    #[test]
    fn boundary_non_matching_dispatch_name_ignored() {
        let arms = [arm("real-arm", "verified", None)];
        let dispatches = [("ghost-arm", 100), ("real-arm", 950)];
        let result = neglected(&arms, &dispatches, 1000, 100);
        assert!(result.is_empty());
    }

    // Boundary: The same name twice in last_dispatch: latest timestamp wins.
    #[test]
    fn boundary_duplicate_dispatch_name_latest_wins() {
        let arms = [arm("multi-dispatch", "verified", None)];
        // Ancient dispatch at 100, recent dispatch at 950.
        let dispatches = [("multi-dispatch", 100), ("multi-dispatch", 950)];
        let result = neglected(&arms, &dispatches, 1000, 100);
        assert!(result.is_empty());
    }

    // Composition: neglected returns entries sorted by name, ascending byte order.
    #[test]
    fn composition_sorted_by_name_ascending_byte_order() {
        let arms = [
            arm("gamma", "verified", None),
            arm("alpha", "verified", None),
            arm("beta", "verified", None),
        ];
        let result = neglected(&arms, &[], 1000, 100);
        let names: Vec<&str> = result.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    // Composition: ParkExpired arm is recognized as dispatchable and neglected.
    #[test]
    fn composition_park_expired_arm_is_dispatchable_and_neglected() {
        let arms = [arm("expired-park", "verified", Some(500))];
        let dispatches = [("expired-park", 600)];
        let result = neglected(&arms, &dispatches, 1000, 200);
        assert_eq!(
            result,
            vec![Neglected {
                name: "expired-park".to_string(),
                availability: Availability::ParkExpired,
                last_seen: LastSeen::Ago(400),
            }]
        );
    }

    // Composition: Duplicate rows in arms collapse to one entry with no duplicates.
    #[test]
    fn composition_duplicate_arms_deduplicated_to_single_entry() {
        let arms = [
            arm("same-arm", "verified", None),
            arm("same-arm", "verified", None),
        ];
        let result = neglected(&arms, &[], 1000, 100);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "same-arm");
    }

    // Degenerate boundary: now_ms == 0.
    #[test]
    fn boundary_now_ms_zero_handled_cleanly() {
        let arms = [
            arm("arm-ready", "verified", None),
            arm("arm-parked", "verified", Some(10)),
            arm("arm-expired", "verified", Some(0)),
        ];
        let dispatches = [("arm-ready", 0)];
        let result = neglected(&arms, &dispatches, 0, 0);
        assert_eq!(
            result,
            vec![
                Neglected {
                    name: "arm-expired".to_string(),
                    availability: Availability::ParkExpired,
                    last_seen: LastSeen::Never,
                },
                Neglected {
                    name: "arm-ready".to_string(),
                    availability: Availability::Ready,
                    last_seen: LastSeen::Ago(0),
                },
            ]
        );
    }

    // Degenerate boundary: u64::MAX timestamps.
    #[test]
    fn boundary_large_timestamps_do_not_overflow() {
        let arms = [arm("arm1", "verified", None)];
        let dispatches = [("arm1", u64::MAX - 500)];
        let result = neglected(&arms, &dispatches, u64::MAX, 400);
        assert_eq!(
            result,
            vec![Neglected {
                name: "arm1".to_string(),
                availability: Availability::Ready,
                last_seen: LastSeen::Ago(500),
            }]
        );
    }
}

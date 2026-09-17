//! A measurement that cannot silently claim it happened.
//!
//! Seven instruments in this project have made the same mistake, each written
//! by someone who knew about the previous one: a check that could not run
//! reported the same value as a check that ran and found nothing wrong. An
//! empty test suite read as "passed"; an uncompilable suite read as "no
//! signal"; a veto with no reference read as "not vetoed". Documentation did
//! not stop any of them; only a type can.
//!
//! [`Measurement`] is that type. A quantity is never a bare value: it is
//! either [`Measurement::Observed`] — actually seen — or
//! [`Measurement::Missing`], carrying a stated [`Absent`] reason, and nothing
//! lets the second read as the first by accident. There is deliberately no
//! `unwrap`, no `unwrap_or`, no `Default`, and no `From<Option<T>>`: those
//! are precisely the paths the seven recurrences travelled. The short exit is
//! the safe one — [`Measurement::value`] is an [`Option`] — and the only
//! route to a bare `T` is [`Measurement::or_default_because`], which makes
//! the caller state, in prose, why a default is defensible at that call site.
//!
//! The module is pure: no I/O.

/// The reason text stored when a constructor is handed a blank reason, so a
/// log line always says something.
const UNSPECIFIED: &str = "unspecified";

/// Normalises a stated reason: blank input becomes [`UNSPECIFIED`], anything
/// else is stored exactly as given.
fn stated(reason: &str) -> String {
    if reason.trim().is_empty() {
        UNSPECIFIED.to_string()
    } else {
        reason.to_string()
    }
}

/// Why a measurement is absent. Exactly these variants and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Absent {
    /// The instrument was never run.
    NotAttempted,
    /// The instrument ran but could not produce a value, e.g. the suite did
    /// not compile.
    InstrumentFailed {
        /// What went wrong, in prose. Never empty.
        reason: String,
    },
    /// There was nothing to measure, e.g. a veto with no reference
    /// implementation.
    NothingToMeasure {
        /// Why there was nothing, in prose. Never empty.
        reason: String,
    },
    /// A value was produced but is not trustworthy, e.g. a known-broken
    /// suite.
    Untrusted {
        /// Why the value cannot be trusted, in prose. Never empty.
        reason: String,
    },
}

/// An observation, or a stated reason there is none.
///
/// Deliberately not `Default` and not `From<Option<T>>`: either would let an
/// absence become a value — or a value an absence — without a stated reason,
/// which is the coercion this type exists to close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Measurement<T> {
    /// The quantity was actually observed.
    Observed(T),
    /// The quantity is absent, for the stated reason.
    Missing(Absent),
}

impl<T> Measurement<T> {
    /// A value that was actually observed.
    pub fn observed(v: T) -> Self {
        Measurement::Observed(v)
    }

    /// No measurement: the instrument was never run.
    pub fn not_attempted() -> Self {
        Measurement::Missing(Absent::NotAttempted)
    }

    /// The instrument ran but could not produce a value.
    pub fn instrument_failed(reason: &str) -> Self {
        Measurement::Missing(Absent::InstrumentFailed {
            reason: stated(reason),
        })
    }

    /// There was nothing to measure.
    pub fn nothing_to_measure(reason: &str) -> Self {
        Measurement::Missing(Absent::NothingToMeasure {
            reason: stated(reason),
        })
    }

    /// A value was produced but is not trustworthy.
    pub fn untrusted(reason: &str) -> Self {
        Measurement::Missing(Absent::Untrusted {
            reason: stated(reason),
        })
    }

    /// True when, and only when, a value was observed.
    pub fn is_observed(&self) -> bool {
        matches!(self, Measurement::Observed(_))
    }

    /// The observed value, if there is one. The short, safe read.
    pub fn value(&self) -> Option<&T> {
        match self {
            Measurement::Observed(v) => Some(v),
            Measurement::Missing(_) => None,
        }
    }

    /// The stated reason there is no value, if there is no value.
    pub fn absent(&self) -> Option<&Absent> {
        match self {
            Measurement::Observed(_) => None,
            Measurement::Missing(absent) => Some(absent),
        }
    }

    /// Map over an observed value, preserving absence unchanged: the same
    /// [`Absent`] variant and reason text, whatever constructed them.
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Measurement<U> {
        match self {
            Measurement::Observed(v) => Measurement::Observed(f(v)),
            Measurement::Missing(absent) => Measurement::Missing(absent),
        }
    }

    /// The ONLY way to get a bare value out.
    ///
    /// Named to be conspicuous at the call site and to require the caller to
    /// state, in prose, why a default is defensible here. An observed value
    /// comes back as-is; a missing one is replaced by `default`. The
    /// justification is never read at run time — its job is to appear in the
    /// diff and the review, not in the computation.
    pub fn or_default_because(self, default: T, justification: &str) -> T {
        let _ = justification;
        match self {
            Measurement::Observed(v) => v,
            Measurement::Missing(_) => default,
        }
    }
}

/// True when every measurement in the slice was observed.
///
/// True for an empty slice: vacuous truth, nothing is missing.
pub fn all_observed<T>(ms: &[Measurement<T>]) -> bool {
    ms.iter().all(Measurement::is_observed)
}

/// The reasons, in slice order, for every measurement that is missing.
///
/// Observed entries contribute nothing; an empty slice yields an empty vector.
pub fn missing_reasons<T>(ms: &[Measurement<T>]) -> Vec<&Absent> {
    ms.iter().filter_map(Measurement::absent).collect()
}

/// Combine two measurements of the same quantity from independent instruments.
///
/// Two observations that agree are the value. Two observations that DISAGREE
/// are [`Absent::Untrusted`]: independent instruments contradicting each
/// other means at least one is broken, and picking either is guessing. When
/// exactly one side observed, that observation stands; when neither
/// observed, the first argument's absence is returned unchanged.
pub fn corroborate<T: PartialEq + Clone>(a: Measurement<T>, b: Measurement<T>) -> Measurement<T> {
    match (a, b) {
        (Measurement::Observed(x), Measurement::Observed(y)) => {
            if x == y {
                Measurement::Observed(x)
            } else {
                Measurement::Missing(Absent::Untrusted {
                    reason: "independent instruments disagree".to_string(),
                })
            }
        }
        (observed @ Measurement::Observed(_), Measurement::Missing(_)) => observed,
        (Measurement::Missing(_), observed @ Measurement::Observed(_)) => observed,
        (Measurement::Missing(absent), Measurement::Missing(_)) => Measurement::Missing(absent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A type with no `Clone` and no `PartialEq`, pinning that the read-side
    /// API never requires either.
    #[derive(Debug)]
    struct Unclonable(i32);

    #[test]
    fn observed_exposes_value_and_no_absence() {
        let m = Measurement::observed(7u32);
        assert!(m.is_observed());
        assert_eq!(m.value(), Some(&7));
        assert_eq!(m.absent(), None);
    }

    #[test]
    fn missing_exposes_absence_and_no_value() {
        let m = Measurement::<u32>::not_attempted();
        assert!(!m.is_observed());
        assert_eq!(m.value(), None);
        assert_eq!(m.absent(), Some(&Absent::NotAttempted));
    }

    #[test]
    fn constructors_record_the_stated_absence_verbatim() {
        assert_eq!(
            Measurement::<u32>::instrument_failed("did not compile"),
            Measurement::Missing(Absent::InstrumentFailed {
                reason: "did not compile".to_string(),
            })
        );
        assert_eq!(
            Measurement::<u32>::nothing_to_measure("no reference implementation"),
            Measurement::Missing(Absent::NothingToMeasure {
                reason: "no reference implementation".to_string(),
            })
        );
        assert_eq!(
            Measurement::<u32>::untrusted("known-broken suite"),
            Measurement::Missing(Absent::Untrusted {
                reason: "known-broken suite".to_string(),
            })
        );
    }

    #[test]
    fn blank_reason_becomes_unspecified() {
        assert_eq!(
            Measurement::<u32>::instrument_failed(""),
            Measurement::Missing(Absent::InstrumentFailed {
                reason: "unspecified".to_string(),
            })
        );
        assert_eq!(
            Measurement::<u32>::nothing_to_measure(""),
            Measurement::Missing(Absent::NothingToMeasure {
                reason: "unspecified".to_string(),
            })
        );
        assert_eq!(
            Measurement::<u32>::untrusted("   "),
            Measurement::Missing(Absent::Untrusted {
                reason: "unspecified".to_string(),
            })
        );
    }

    #[test]
    fn map_transforms_the_observed_value() {
        let doubled = Measurement::observed(21u32).map(|v| v * 2);
        assert_eq!(doubled, Measurement::Observed(42));

        let typed = Measurement::observed(7u32).map(|v| v.to_string());
        assert_eq!(typed, Measurement::Observed("7".to_string()));
    }

    #[test]
    fn map_preserves_the_absent_variant_and_reason_text() {
        let cases = [
            Measurement::<u32>::not_attempted(),
            Measurement::<u32>::instrument_failed("did not compile"),
            Measurement::<u32>::nothing_to_measure("no reference implementation"),
            Measurement::<u32>::untrusted("known-broken suite"),
        ];
        for m in cases {
            let mapped = m.clone().map(|v: u32| v.to_string());
            assert!(!mapped.is_observed());
            assert_eq!(mapped.absent(), m.absent());
        }
    }

    #[test]
    fn or_default_because_returns_the_observed_value() {
        let m = Measurement::observed(String::from("measured"));
        let v = m.or_default_because(String::from("default"), "value is observed above");
        assert_eq!(v, "measured");
    }

    #[test]
    fn or_default_because_returns_default_only_when_missing() {
        let m = Measurement::<u32>::nothing_to_measure("no reference implementation");
        let v = m.or_default_because(0, "zero means unmeasured in this table");
        assert_eq!(v, 0);

        let m = Measurement::<u32>::not_attempted();
        let v = m.or_default_because(0, "instrument never ran; zero is a placeholder");
        assert_eq!(v, 0);
    }

    #[test]
    fn all_observed_true_for_empty_slice() {
        assert!(all_observed::<u32>(&[]));
        assert!(missing_reasons::<u32>(&[]).is_empty());
    }

    #[test]
    fn all_observed_false_when_any_measurement_is_missing() {
        let all = [Measurement::observed(1u32), Measurement::observed(2u32)];
        assert!(all_observed(&all));

        let one_missing = [
            Measurement::observed(1u32),
            Measurement::<u32>::not_attempted(),
            Measurement::observed(3u32),
        ];
        assert!(!all_observed(&one_missing));
    }

    #[test]
    fn missing_reasons_keeps_slice_order() {
        let ms = [
            Measurement::<u32>::not_attempted(),
            Measurement::observed(1),
            Measurement::<u32>::instrument_failed("did not compile"),
            Measurement::observed(2),
            Measurement::<u32>::nothing_to_measure("no reference implementation"),
        ];
        let reasons = missing_reasons(&ms);
        assert_eq!(
            reasons,
            vec![
                &Absent::NotAttempted,
                &Absent::InstrumentFailed {
                    reason: "did not compile".to_string(),
                },
                &Absent::NothingToMeasure {
                    reason: "no reference implementation".to_string(),
                },
            ]
        );
    }

    #[test]
    fn missing_reasons_omits_observed_entries() {
        let ms = [Measurement::observed(1u32), Measurement::observed(2u32)];
        assert!(missing_reasons(&ms).is_empty());
    }

    #[test]
    fn disagreeing_observations_corroborate_to_untrusted() {
        let m = corroborate(Measurement::observed(1u32), Measurement::observed(2u32));
        let reason = match m {
            Measurement::Missing(Absent::Untrusted { reason }) => reason,
            other => panic!("expected Untrusted, got {other:?}"),
        };
        assert!(!reason.is_empty());
    }

    #[test]
    fn agreeing_observations_corroborate_to_the_value() {
        let m = corroborate(Measurement::observed(5u32), Measurement::observed(5u32));
        assert_eq!(m, Measurement::Observed(5));
    }

    #[test]
    fn one_observed_one_missing_yields_the_observed() {
        let m = corroborate(
            Measurement::observed(3u32),
            Measurement::<u32>::not_attempted(),
        );
        assert_eq!(m, Measurement::Observed(3));

        let m = corroborate(
            Measurement::<u32>::instrument_failed("did not compile"),
            Measurement::observed(4u32),
        );
        assert_eq!(m, Measurement::Observed(4));

        // Any absence counts as missing, Untrusted included.
        let m = corroborate(
            Measurement::<u32>::untrusted("known-broken suite"),
            Measurement::observed(5u32),
        );
        assert_eq!(m, Measurement::Observed(5));
    }

    #[test]
    fn neither_observed_yields_the_first_absence() {
        let m = corroborate(
            Measurement::<u32>::not_attempted(),
            Measurement::<u32>::instrument_failed("did not compile"),
        );
        assert_eq!(m, Measurement::Missing(Absent::NotAttempted));

        let m = corroborate(
            Measurement::<u32>::untrusted("known-broken suite"),
            Measurement::<u32>::nothing_to_measure("no reference implementation"),
        );
        assert_eq!(
            m,
            Measurement::Missing(Absent::Untrusted {
                reason: "known-broken suite".to_string(),
            })
        );
    }

    #[test]
    fn measurement_and_absent_are_debug_clone_and_equality_comparable() {
        fn assert_traits<T: std::fmt::Debug + Clone + PartialEq + Eq>() {}
        assert_traits::<Measurement<i32>>();
        assert_traits::<Absent>();

        let a = Measurement::observed(1u32);
        assert_eq!(a, a.clone());
        assert_ne!(a, Measurement::observed(2u32));
        assert_ne!(
            Measurement::<u32>::observed(1),
            Measurement::<u32>::not_attempted()
        );
        assert_ne!(
            Measurement::<u32>::instrument_failed("one"),
            Measurement::<u32>::instrument_failed("two")
        );
    }

    #[test]
    fn read_side_works_for_types_that_are_neither_clone_nor_eq() {
        let observed = Measurement::observed(Unclonable(7));
        assert!(observed.is_observed());
        assert_eq!(observed.value().map(|u| u.0), Some(7));

        let doubled = observed.map(|u| Unclonable(u.0 * 2));
        let v = doubled.or_default_because(Unclonable(0), "observed above");
        assert_eq!(v.0, 14);

        let missing = Measurement::<Unclonable>::instrument_failed("tool not installed");
        assert!(missing.value().is_none());
        assert!(missing.absent().is_some());
        let v = missing.or_default_because(Unclonable(0), "zero means unmeasured here");
        assert_eq!(v.0, 0);
    }
}

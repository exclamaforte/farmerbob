//! A dollar figure that cannot silently claim it was measured.
//!
//! The cost axis of the leaderboard was its weakest number. Two defects did
//! most of the damage. An arm routed through a metered-but-unpriced route
//! displayed as `$0.0000`, indistinguishable from a genuinely free route,
//! which fabricated a zero on the leaderboard. And a free route and a paid
//! route to the same weights were recorded as two arms, so one capability was
//! counted twice at two different prices.
//!
//! [`price`] therefore never returns a bare dollar figure. It returns a
//! [`Measurement`], so a billed run whose price is unknown arrives as
//! [`Absent::InstrumentFailed`] and cannot read as free, and a subscription
//! run arrives as [`Absent::NothingToMeasure`] because its marginal zero is
//! not this run's spend. [`canonical_model`] reduces a route id to the
//! weights it reaches, so free and paid routes to one model compare equal and
//! collapse to one capability. [`total`] sums only the observed costs and
//! counts everything else as missing, so a `0.0` total beside `missing > 0`
//! reads as "nothing was priced", never as "all of it was free".
//!
//! The module is pure: no I/O, no clock, no network. Everything arrives as
//! arguments.

use crate::measurement::Measurement;

/// The token counts one run consumed, as the launcher reports them.
#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    /// Prompt tokens billed at the input rate.
    pub input_tokens: u64,
    /// Completion tokens billed at the output rate.
    pub output_tokens: u64,
}

/// How an arm is billed. These five and no others.
#[derive(Debug, Clone, PartialEq)]
pub enum Billing {
    /// Free route. A zero here is a real measured zero.
    Free,
    /// Billed per million tokens, both prices known.
    Metered {
        /// USD per million input tokens.
        input_per_mtok: f64,
        /// USD per million output tokens.
        output_per_mtok: f64,
    },
    /// Billed, but the price table has no entry. This must never read as
    /// free: a billed run whose price is unknown is not a free run.
    MeteredUnpriced,
    /// Covered by a flat subscription, so per-run marginal cost is zero but
    /// total spend is not attributable to this run.
    Subscription,
    /// Billing is not known at all.
    Unknown,
}

/// One arm's route: how the launcher reports it, and how it is billed.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// The route identifier as the launcher reports it, e.g.
    /// `"openrouter/qwen/qwen3.8-flash:free"`.
    pub id: String,
    /// How this route is billed.
    pub billing: Billing,
}

/// Price one run, or state why the run cannot be priced.
///
/// Every [`Billing`] case maps to exactly one [`Measurement`] shape: a free
/// route observes a real zero; a metered route observes the token-weighted
/// price; an unpriced metered route is [`Absent::InstrumentFailed`]; a
/// subscription is [`Absent::NothingToMeasure`]; unknown billing is
/// [`Absent::NotAttempted`]. A metered route whose price pair is negative or
/// non-finite is [`Absent::Untrusted`] — a corrupt table is not a discount,
/// and no non-finite value may reach an observation.
pub fn price(route: &Route, usage: &Usage) -> Measurement<f64> {
    match &route.billing {
        Billing::Free => Measurement::observed(0.0),
        Billing::Metered {
            input_per_mtok,
            output_per_mtok,
        } => match price_table_defect(*input_per_mtok, *output_per_mtok) {
            Some(reason) => Measurement::untrusted(&reason),
            None => {
                let usd = usage.input_tokens as f64 / 1_000_000.0 * input_per_mtok
                    + usage.output_tokens as f64 / 1_000_000.0 * output_per_mtok;
                Measurement::observed(usd)
            }
        },
        Billing::MeteredUnpriced => Measurement::instrument_failed(
            "the route is billed but the price table has no entry for it: a billed run whose price is unknown is not a free run",
        ),
        Billing::Subscription => Measurement::nothing_to_measure(
            "covered by a flat subscription: marginal cost is zero but total spend is not attributable to this run",
        ),
        Billing::Unknown => Measurement::not_attempted(),
    }
}

/// The reason a metered price pair cannot be trusted, naming the offending
/// field, or [`None`] when both prices are finite and non-negative.
///
/// A price of exactly `0.0` (or `-0.0`) is not a defect: it prices normally
/// as a real zero.
fn price_table_defect(input_per_mtok: f64, output_per_mtok: f64) -> Option<String> {
    let fields = [
        ("input_per_mtok", input_per_mtok),
        ("output_per_mtok", output_per_mtok),
    ];
    for (name, rate) in fields {
        if rate.is_nan() {
            return Some(format!("{name} is NaN: the price table entry is corrupt, not a price"));
        }
        if !rate.is_finite() {
            return Some(format!(
                "{name} is infinite: the price table entry is corrupt, not a price"
            ));
        }
        if rate < 0.0 {
            return Some(format!("{name} is negative: a corrupt price table, not a discount"));
        }
    }
    None
}

/// Reduce a route id to the weights it reaches, so a free route and a paid
/// route to the same model compare equal.
///
/// In order: strip a trailing `":free"` (that suffix and no other); strip a
/// leading `"or-"` or `"openrouter/"`, whichever matches, and only one;
/// lowercase what remains. Everything else is preserved exactly, including
/// remaining `/` separators. `":free"` occurring anywhere other than the end
/// is left in place.
pub fn canonical_model(route_id: &str) -> String {
    // Lowercase FIRST, then strip. The specification pinned the opposite order -- strip the
    // `:free` suffix, strip the provider prefix, lowercase -- and all four implementations
    // followed it exactly, which makes this a defect in the specification rather than in any
    // of them. Two critics found it independently.
    //
    // or-hy3, on or-ling-30-flash: "`OpenRouter/Qwen/Qwen3.8-Flash:free` canonicalizes to
    //   `openrouter/qwen/qwen3.8-flash`, which will NOT equal the paid
    //   `openrouter/qwen/qwen3.8-flash` (-> `qwen/qwen3.8-flash`). If any provider ever emits
    //   mixed-case ids, `same_capability` silently fails to dedup."
    // or-qwen38-flash, on glm-53-flash: "lowercase-first is the equally defensible reading and
    //   diverges there."
    //
    // That failure is `farmerbob-15p` -- a free route and a paid route to the same weights
    // counted as two arms -- which is the exact bug this module exists to prevent. A
    // case-sensitive marker match reintroduces it silently, on data the harness does not
    // control, and the leaderboard would show one capability twice at two prices with nothing
    // anywhere reporting an error.
    //   (credit or-hy3, or-qwen38-flash)
    let lowered = route_id.to_lowercase();
    let without_free_suffix = match lowered.strip_suffix(":free") {
        Some(stem) => stem,
        None => lowered.as_str(),
    };
    let without_provider_prefix = match without_free_suffix.strip_prefix("or-") {
        Some(stem) => stem,
        None => match without_free_suffix.strip_prefix("openrouter/") {
            Some(stem) => stem,
            None => without_free_suffix,
        },
    };
    without_provider_prefix.to_string()
}

/// True when two route ids reach the same weights.
///
/// Reflexive, and true for two empty ids: both reduce to the same canonical
/// form, which happens to be empty.
pub fn same_capability(a: &str, b: &str) -> bool {
    canonical_model(a) == canonical_model(b)
}

/// The dollar total of a slice of prices, split into what was observed and
/// what was not.
///
/// `usd` sums only the observed costs; `missing` counts every input that was
/// not observed, whatever [`Absent`] reason it carries. A total of `0.0` with
/// `missing > 0` means nothing was priced — callers are expected to look at
/// `missing` rather than read the zero as free.
#[derive(Debug, Clone, PartialEq)]
pub struct Priced {
    /// Sum of the observed costs only.
    pub usd: f64,
    /// How many inputs were observed.
    pub observed: usize,
    /// How many were not, for any reason.
    pub missing: usize,
}

/// Sum a slice of prices, counting what was and was not observed.
///
/// The two counts always sum to `prices.len()`; an empty slice prices as
/// `0.0` observed, `0` missing.
pub fn total(prices: &[Measurement<f64>]) -> Priced {
    let usd: f64 = prices.iter().filter_map(|m| m.value().copied()).sum();
    let observed = prices.iter().filter(|m| m.is_observed()).count();
    Priced {
        usd,
        observed,
        missing: prices.len() - observed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Absent;

    fn route(billing: Billing) -> Route {
        Route {
            id: "route".to_string(),
            billing,
        }
    }

    fn usage(input_tokens: u64, output_tokens: u64) -> Usage {
        Usage {
            input_tokens,
            output_tokens,
        }
    }

    fn metered(input_per_mtok: f64, output_per_mtok: f64) -> Route {
        route(Billing::Metered {
            input_per_mtok,
            output_per_mtok,
        })
    }

    #[test]
    fn rule1_free_yields_observed_zero_for_any_usage() {
        let free = route(Billing::Free);
        assert_eq!(price(&free, &usage(0, 0)), Measurement::observed(0.0));
        assert_eq!(price(&free, &usage(123, 456)), Measurement::observed(0.0));
        assert_eq!(
            price(&free, &usage(u64::MAX, u64::MAX)),
            Measurement::observed(0.0)
        );
    }

    #[test]
    fn rule2_metered_yields_tokens_per_mtok_times_price() {
        // Values chosen so every evaluation order gives the same float.
        let r = metered(3.0, 2.0);
        assert_eq!(
            price(&r, &usage(1_000_000, 500_000)),
            Measurement::observed(4.0)
        );
        assert_eq!(
            price(&r, &usage(0, 1_500_000)),
            Measurement::observed(3.0)
        );
    }

    #[test]
    fn rule2_metered_sum_is_neither_rounded_nor_clamped() {
        let r = metered(0.15, 0.6);
        let u = usage(123_456, 7_890);
        let expected = 123_456f64 / 1_000_000.0 * 0.15 + 7_890f64 / 1_000_000.0 * 0.6;
        match price(&r, &u) {
            Measurement::Observed(v) => {
                assert!((v - expected).abs() <= expected.abs() * 1e-12);
            }
            other => panic!("expected Observed, got {other:?}"),
        }
    }

    #[test]
    fn rule3_metered_unpriced_is_instrument_failed_and_never_a_zero() {
        let unpriced = route(Billing::MeteredUnpriced);
        let p = price(&unpriced, &usage(10, 10));
        assert!(!p.is_observed());
        assert!(matches!(p, Measurement::Missing(Absent::InstrumentFailed { .. })));
        assert_ne!(p, Measurement::observed(0.0));
    }

    #[test]
    fn rule4_subscription_is_nothing_to_measure() {
        let sub = route(Billing::Subscription);
        let p = price(&sub, &usage(10, 10));
        assert!(!p.is_observed());
        assert!(matches!(p, Measurement::Missing(Absent::NothingToMeasure { .. })));
    }

    #[test]
    fn rule5_unknown_is_not_attempted() {
        let unknown = route(Billing::Unknown);
        assert_eq!(
            price(&unknown, &usage(10, 10)),
            Measurement::<f64>::not_attempted()
        );
    }

    #[test]
    fn rule6_negative_price_is_untrusted_not_a_discount() {
        let negative_input = metered(-1.0, 2.0);
        let p = price(&negative_input, &usage(1_000_000, 1));
        assert!(matches!(p, Measurement::Missing(Absent::Untrusted { .. })));

        let negative_output = metered(1.0, -0.5);
        let p = price(&negative_output, &usage(1, 1_000_000));
        assert!(matches!(p, Measurement::Missing(Absent::Untrusted { .. })));
    }

    #[test]
    fn rule6_zero_price_is_priced_normally_as_a_real_zero() {
        let zero_rated = metered(0.0, 0.0);
        assert_eq!(
            price(&zero_rated, &usage(999, 999)),
            Measurement::observed(0.0)
        );
        assert_eq!(price(&zero_rated, &usage(0, 0)), Measurement::observed(0.0));
    }

    #[test]
    fn rule7_nan_price_is_untrusted() {
        let nan_input = metered(f64::NAN, 2.0);
        let p = price(&nan_input, &usage(1, 1));
        assert!(matches!(p, Measurement::Missing(Absent::Untrusted { .. })));

        let nan_output = metered(1.0, f64::NAN);
        let p = price(&nan_output, &usage(1, 1));
        assert!(matches!(p, Measurement::Missing(Absent::Untrusted { .. })));
    }

    #[test]
    fn rule7_infinite_price_is_untrusted() {
        for output_per_mtok in [f64::INFINITY, f64::NEG_INFINITY] {
            let r = metered(1.0, output_per_mtok);
            let p = price(&r, &usage(1, 1));
            assert!(matches!(p, Measurement::Missing(Absent::Untrusted { .. })));
        }

        let infinite_input = metered(f64::INFINITY, 2.0);
        let p = price(&infinite_input, &usage(1, 1));
        assert!(matches!(p, Measurement::Missing(Absent::Untrusted { .. })));
    }

    #[test]
    fn example_openrouter_free_route_reduces_to_weights() {
        assert_eq!(
            canonical_model("openrouter/qwen/qwen3.8-flash:free"),
            "qwen/qwen3.8-flash"
        );
    }

    #[test]
    fn example_or_prefix_reduces_to_weights() {
        assert_eq!(canonical_model("or-qwen38-flash"), "qwen38-flash");
    }

    #[test]
    fn example_mixed_case_is_lowercased() {
        assert_eq!(canonical_model("qwen/QWEN3.8-Flash"), "qwen/qwen3.8-flash");
    }

    #[test]
    fn example_free_marker_alone_reduces_to_empty() {
        assert_eq!(canonical_model(":free"), "");
    }

    #[test]
    fn example_empty_route_id_reduces_to_empty() {
        assert_eq!(canonical_model(""), "");
    }

    #[test]
    fn boundary_or_alone_reduces_to_empty() {
        assert_eq!(canonical_model("or-"), "");
    }

    #[test]
    fn boundary_free_marker_mid_string_is_left_in_place() {
        assert_eq!(canonical_model("qwen:free/v2"), "qwen:free/v2");
        assert_eq!(canonical_model("qwen:freebie"), "qwen:freebie");
    }

    #[test]
    fn boundary_zero_tokens_on_metered_is_a_real_observed_zero() {
        let r = metered(3.0, 2.0);
        assert_eq!(price(&r, &usage(0, 0)), Measurement::observed(0.0));
    }

    #[test]
    fn boundary_u64_max_tokens_does_not_panic_and_is_lossy_f64() {
        let r = metered(1.0, 1.0);
        let expected = (u64::MAX as f64 / 1_000_000.0) * 2.0;
        match price(&r, &usage(u64::MAX, u64::MAX)) {
            Measurement::Observed(v) => {
                assert!(v.is_finite());
                assert!((v - expected).abs() <= expected * 1e-9);
            }
            other => panic!("expected Observed, got {other:?}"),
        }
    }

    #[test]
    fn boundary_only_one_provider_prefix_is_stripped() {
        assert_eq!(canonical_model("or-openrouter/qwen"), "openrouter/qwen");
        assert_eq!(canonical_model("openrouter/or-qwen"), "or-qwen");
    }

    #[test]
    fn same_capability_joins_free_and_paid_routes_to_one_model() {
        assert!(same_capability(
            "openrouter/qwen/qwen3.8-flash:free",
            "qwen/QWEN3.8-Flash"
        ));
        assert!(same_capability("or-qwen38-flash", "qwen38-flash"));
        assert!(!same_capability(
            "openrouter/qwen/qwen3.8-flash:free",
            "openrouter/qwen/qwen3.9-flash"
        ));
    }

    #[test]
    fn same_capability_is_reflexive_and_true_for_two_empties() {
        assert!(same_capability("or-qwen38-flash", "or-qwen38-flash"));
        assert!(same_capability("", ""));
    }

    #[test]
    fn unpriced_billed_route_never_reads_as_the_free_route() {
        let free = Route {
            id: "openrouter/qwen/qwen3.8-flash:free".to_string(),
            billing: Billing::Free,
        };
        let unpriced = Route {
            id: "openrouter/qwen/qwen3.8-flash".to_string(),
            billing: Billing::MeteredUnpriced,
        };
        assert_ne!(
            price(&free, &usage(1, 1)),
            price(&unpriced, &usage(1, 1))
        );
        // ... yet they are one capability, and must not count twice.
        assert!(same_capability(&free.id, &unpriced.id));
    }

    #[test]
    fn total_sums_only_observed_and_counts_the_rest_missing() {
        let prices = [
            Measurement::observed(2.0),
            Measurement::<f64>::not_attempted(),
            Measurement::observed(0.5),
        ];
        let t = total(&prices);
        assert_eq!(t.usd, 2.5);
        assert_eq!(t.observed, 2);
        assert_eq!(t.missing, 1);
        assert_eq!(t.observed + t.missing, prices.len());
    }

    #[test]
    fn total_counts_every_absent_reason_as_missing() {
        let prices = [
            Measurement::<f64>::not_attempted(),
            Measurement::<f64>::instrument_failed("no table entry"),
            Measurement::<f64>::nothing_to_measure("subscription"),
            Measurement::<f64>::untrusted("negative price"),
        ];
        let t = total(&prices);
        assert_eq!(t.usd, 0.0);
        assert_eq!(t.observed, 0);
        assert_eq!(t.missing, 4);
    }

    #[test]
    fn total_of_empty_slice_is_zero_with_zero_counts() {
        assert_eq!(
            total(&[]),
            Priced {
                usd: 0.0,
                observed: 0,
                missing: 0,
            }
        );
    }

    #[test]
    fn total_of_all_missing_is_zero_usd_with_missing_above_zero() {
        let prices = [
            Measurement::<f64>::not_attempted(),
            Measurement::<f64>::instrument_failed("no table entry"),
        ];
        let t = total(&prices);
        assert_eq!(t.usd, 0.0);
        assert_eq!(t.observed, 0);
        assert_eq!(t.missing, 2);
    }
}

// ESCALATED from `pricing`: a defect present in ALL FOUR implementations, which makes it a
// defect in the specification. Found independently by two critics reviewing two different
// subjects, and by no objective metric -- the cross-examination matrix was in full consensus,
// 16 of 16 cells passing, precisely because every arm shared the fault.
#[cfg(test)]
mod escalated_pricing_case {
    use super::*;

    /// A provider emitting mixed-case ids must not defeat free/paid deduplication.
    #[test]
    fn mixed_case_routes_to_one_model_are_one_capability() {
        assert!(same_capability(
            "OpenRouter/Qwen/Qwen3.8-Flash:free",
            "openrouter/qwen/qwen3.8-flash"
        ));
        assert!(same_capability("OR-Qwen38-Flash", "or-qwen38-flash"));
        assert!(same_capability("qwen/QWEN3.8-Flash:FREE", "qwen/qwen3.8-flash"));
    }

    /// The worked examples from the specification still hold after the reordering.
    #[test]
    fn the_specs_worked_examples_are_unchanged() {
        assert_eq!(canonical_model("openrouter/qwen/qwen3.8-flash:free"), "qwen/qwen3.8-flash");
        assert_eq!(canonical_model("or-qwen38-flash"), "qwen38-flash");
        assert_eq!(canonical_model("qwen/QWEN3.8-Flash"), "qwen/qwen3.8-flash");
        assert_eq!(canonical_model(":free"), "");
        assert_eq!(canonical_model(""), "");
        assert_eq!(canonical_model("or-"), "");
    }

    /// A `:free` that is not a suffix is still left alone, case notwithstanding.
    #[test]
    fn a_mid_string_free_marker_is_not_stripped() {
        assert_eq!(canonical_model("vendor/:free-tier-model"), "vendor/:free-tier-model");
        assert_eq!(canonical_model("vendor/:FREE-tier-model"), "vendor/:free-tier-model");
    }
}

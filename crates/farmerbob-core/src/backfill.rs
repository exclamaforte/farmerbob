//! Pure planning for tasks that may need a subjective-tier backfill.

/// What the caller found on disk for one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The task name.
    pub name: String,
    /// A spec exists at `.fb/prompts/<name>.md`.
    pub has_spec: bool,
    /// A non-empty `<name>.claims.json` exists.
    pub has_claims: bool,
    /// The declared target, resolved from the spec. `None` when the spec
    /// declares none or the declaration could not be read.
    pub target: Option<String>,
}

/// Why a task will not be backfilled, or that it will. Exactly these and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Run the pipeline. Carries the crate parsed from the target.
    Run {
        /// The crate the target lives in, e.g. `farmerbob-core`.
        krate: String,
    },
    /// No spec, so there is nothing to run against.
    NoSpec,
    /// Claims already exist; the subjective tier has run.
    AlreadyDone,
    /// The spec declares no target, or one this module cannot parse a crate
    /// from. Never silently skipped: the reason is the value.
    NoTarget {
        /// What the spec gave, if anything, so a reader can fix it.
        declared: Option<String>,
    },
}

/// Decide one task.
pub fn plan(c: &Candidate) -> Plan {
    if !c.has_spec {
        return Plan::NoSpec;
    }

    if c.has_claims {
        return Plan::AlreadyDone;
    }

    let Some(target) = c.target.as_deref() else {
        return Plan::NoTarget { declared: None };
    };

    let components: Vec<&str> = target.split('/').collect();
    if components.len() < 3 || components[1].is_empty() {
        return Plan::NoTarget {
            declared: Some(target.to_owned()),
        };
    }

    Plan::Run {
        krate: components[1].to_owned(),
    }
}

/// Every task that will actually run, in the order given.
pub fn runnable(cs: &[Candidate]) -> Vec<(String, String)> {
    cs.iter()
        .filter_map(|candidate| match plan(candidate) {
            Plan::Run { krate } => Some((candidate.name.clone(), krate)),
            Plan::NoSpec | Plan::AlreadyDone | Plan::NoTarget { .. } => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Candidate, Plan, plan, runnable};

    fn candidate(has_spec: bool, has_claims: bool, target: Option<&str>) -> Candidate {
        Candidate {
            name: String::from("task"),
            has_spec,
            has_claims,
            target: target.map(String::from),
        }
    }

    #[test]
    fn no_spec_has_precedence_over_claims_and_a_valid_target() {
        let candidate = candidate(false, true, Some("crates/farmerbob-core/src/lib.rs"));

        assert_eq!(plan(&candidate), Plan::NoSpec);
    }

    #[test]
    fn claims_have_precedence_over_a_valid_target() {
        let candidate = candidate(true, true, Some("crates/farmerbob-core/src/lib.rs"));

        assert_eq!(plan(&candidate), Plan::AlreadyDone);
    }

    #[test]
    fn missing_target_is_reported() {
        let candidate = candidate(true, false, None);

        assert_eq!(plan(&candidate), Plan::NoTarget { declared: None });
    }

    #[test]
    fn valid_target_carries_the_second_path_component() {
        let candidate = candidate(true, false, Some("crates/farmerbob-core/src/lib.rs"));

        assert_eq!(
            plan(&candidate),
            Plan::Run {
                krate: String::from("farmerbob-core"),
            }
        );
    }

    #[test]
    fn extra_target_components_are_allowed() {
        let candidate = candidate(true, false, Some("crates/farmerbob-core/src/a/b.rs"));

        assert_eq!(
            plan(&candidate),
            Plan::Run {
                krate: String::from("farmerbob-core"),
            }
        );
    }

    #[test]
    fn malformed_targets_keep_the_declared_value() {
        let cases = ["src/lib.rs", "crates//src/x.rs", ""];

        for declared in cases {
            let candidate = candidate(true, false, Some(declared));

            assert_eq!(
                plan(&candidate),
                Plan::NoTarget {
                    declared: Some(String::from(declared)),
                }
            );
        }
    }

    #[test]
    fn empty_name_is_planned_normally() {
        let candidate = Candidate {
            name: String::new(),
            has_spec: true,
            has_claims: false,
            target: Some(String::from("crates/x/src/y.rs")),
        };

        assert_eq!(
            plan(&candidate),
            Plan::Run {
                krate: String::from("x"),
            }
        );
    }

    #[test]
    fn runnable_keeps_only_runs_in_input_order_and_keeps_duplicates() {
        let candidates = vec![
            Candidate {
                name: String::from("first"),
                has_spec: true,
                has_claims: false,
                target: Some(String::from("crates/one/src/a.rs")),
            },
            Candidate {
                name: String::from("skip-no-spec"),
                has_spec: false,
                has_claims: false,
                target: Some(String::from("crates/ignored/src/a.rs")),
            },
            Candidate {
                name: String::from("second"),
                has_spec: true,
                has_claims: false,
                target: Some(String::from("crates/two/src/b.rs")),
            },
            Candidate {
                name: String::from("first"),
                has_spec: true,
                has_claims: false,
                target: Some(String::from("crates/one/src/c.rs")),
            },
        ];

        assert_eq!(
            runnable(&candidates),
            vec![
                (String::from("first"), String::from("one")),
                (String::from("second"), String::from("two")),
                (String::from("first"), String::from("one")),
            ]
        );
    }

    #[test]
    fn runnable_is_empty_for_empty_input() {
        assert!(runnable(&[]).is_empty());
    }
}

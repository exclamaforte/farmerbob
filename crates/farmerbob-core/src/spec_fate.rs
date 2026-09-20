//! Dispatchability of queued specs against a given base.

use crate::target_decl::{self, Declaration, NoDeclaration};

/// Whether a queued spec can still be dispatched against a given base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// At least one declared deliverable still has work in it. `settled` is a
    /// warning listing already-completed paths in declaration order.
    Runnable { settled: Vec<String> },
    /// Every declared deliverable's precondition already holds, with grounds
    /// for each settled path in declaration order.
    Stale { settled: Vec<(String, String)> },
    /// The spec declares nothing, or contains a declaration that cannot be read.
    Undecidable(NoDeclaration),
}

/// The base a spec is judged against: the set of paths that exist on it.
pub trait Base {
    /// Whether `path` exists on this base.
    fn exists(&self, path: &str) -> bool;
}

/// Returns the fate of one spec against one base.
pub fn fate(spec: &str, base: &impl Base) -> Fate {
    let declarations = match target_decl::declared_all(spec) {
        Ok(declarations) => declarations,
        Err(reason) => return Fate::Undecidable(reason),
    };
    let declaration_count = declarations.len();
    let mut settled = Vec::new();

    for declaration in declarations {
        let path = target_decl::path(&declaration).to_string();
        let already_done = match &declaration {
            Declaration::Creates(_) => base.exists(&path),
            Declaration::Modifies(_) => !base.exists(&path),
        };
        if already_done {
            settled.push((path.clone(), grounds(&declaration, &path)));
        }
    }

    // ZERO DECLARATIONS IS NOT STALE. `settled.len() == declaration_count` is `0 == 0` for a
    // spec that declares nothing, which would mark it dead -- and the boundary clause says
    // the opposite: a spec that asks for nothing has not been shown to have nothing left to
    // do. `declared_all` returns `Err(Absent)` for an empty declaration list today, so this
    // is unreachable through it; the guard is here because the rule must not depend on a
    // distant invariant in another module, and `Stale` is the dangerous direction.
    if declaration_count == 0 {
        return Fate::Undecidable(NoDeclaration::Absent);
    }
    if settled.len() == declaration_count {
        Fate::Stale { settled }
    } else {
        Fate::Runnable {
            settled: settled.into_iter().map(|(path, _)| path).collect(),
        }
    }
}

fn grounds(declaration: &Declaration, path: &str) -> String {
    match declaration {
        Declaration::Creates(_) => {
            format!(
                "creates-task is already satisfied because '{}' exists",
                path
            )
        }
        Declaration::Modifies(_) => {
            format!(
                "modifies-task cannot proceed because '{}' does not exist",
                path
            )
        }
    }
}

/// Returns one fate per input, in the order given, without dropping or merging entries.
pub fn survey<'a>(specs: &[(&'a str, &'a str)], base: &impl Base) -> Vec<(&'a str, Fate)> {
    specs
        .iter()
        .map(|(name, spec)| (*name, fate(spec, base)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SetBase<'a> {
        paths: &'a [&'a str],
    }

    impl Base for SetBase<'_> {
        fn exists(&self, path: &str) -> bool {
            self.paths.contains(&path)
        }
    }

    struct PanicBase;

    impl Base for PanicBase {
        fn exists(&self, _path: &str) -> bool {
            panic!("Base::exists must not be called for an Undecidable spec");
        }
    }

    fn base<'a>(paths: &'a [&'a str]) -> SetBase<'a> {
        SetBase { paths }
    }

    #[test]
    fn creates_absent_is_runnable_with_nothing_settled() {
        assert_eq!(
            fate("<!-- fb:creates a.rs -->", &base(&[])),
            Fate::Runnable { settled: vec![] }
        );
    }

    #[test]
    fn creates_present_is_stale_with_path_and_grounds() {
        match fate("<!-- fb:creates a.rs -->", &base(&["a.rs"])) {
            Fate::Stale { settled } => {
                assert_eq!(settled.len(), 1);
                assert_eq!(settled[0].0, "a.rs");
                assert!(!settled[0].1.is_empty());
                assert!(settled[0].1.contains("a.rs"));
            }
            other => panic!("expected stale, got {other:?}"),
        }
    }

    #[test]
    fn modifies_absent_is_stale() {
        match fate("<!-- fb:modifies a.rs -->", &base(&[])) {
            Fate::Stale { settled } => assert_eq!(settled.len(), 1),
            other => panic!("expected stale, got {other:?}"),
        }
    }

    #[test]
    fn modifies_present_is_runnable_with_nothing_settled() {
        assert_eq!(
            fate("<!-- fb:modifies a.rs -->", &base(&["a.rs"])),
            Fate::Runnable { settled: vec![] }
        );
    }

    #[test]
    fn one_settled_created_deliverable_does_not_stale_spec_in_both_orders() {
        for spec in [
            "<!-- fb:creates a.rs -->\n<!-- fb:creates b.rs -->",
            "<!-- fb:creates b.rs -->\n<!-- fb:creates a.rs -->",
        ] {
            assert_eq!(
                fate(spec, &base(&["a.rs"])),
                Fate::Runnable {
                    settled: vec!["a.rs".to_string()]
                }
            );
        }
    }

    #[test]
    fn all_created_deliverables_settled_are_stale_in_declared_order() {
        match fate(
            "<!-- fb:creates a.rs -->\n<!-- fb:creates b.rs -->",
            &base(&["a.rs", "b.rs"]),
        ) {
            Fate::Stale { settled } => {
                assert_eq!(settled.len(), 2);
                assert_eq!(settled[0].0, "a.rs");
                assert_eq!(settled[1].0, "b.rs");
                assert!(settled.iter().all(|(_, grounds)| !grounds.is_empty()));
            }
            other => panic!("expected stale, got {other:?}"),
        }
    }

    #[test]
    fn mixed_verbs_only_settle_the_stale_create() {
        assert_eq!(
            fate(
                "<!-- fb:creates a.rs -->\n<!-- fb:modifies b.rs -->",
                &base(&["a.rs", "b.rs"])
            ),
            Fate::Runnable {
                settled: vec!["a.rs".to_string()]
            }
        );
    }

    #[test]
    fn duplicate_paths_are_judged_and_returned_twice() {
        match fate(
            "<!-- fb:creates a.rs -->\n<!-- fb:creates a.rs -->",
            &base(&["a.rs"]),
        ) {
            Fate::Stale { settled } => {
                assert_eq!(settled.len(), 2);
                assert_eq!(settled[0].0, "a.rs");
                assert_eq!(settled[1].0, "a.rs");
            }
            other => panic!("expected stale, got {other:?}"),
        }
    }

    #[test]
    fn absent_spec_is_undecidable_without_consulting_base() {
        assert_eq!(
            fate("no declaration", &PanicBase),
            Fate::Undecidable(NoDeclaration::Absent)
        );
    }

    #[test]
    fn refused_declaration_is_undecidable_and_absorbs_other_declarations() {
        let result = fate(
            "<!-- fb:creates a.rs -->\n<!-- fb:deletes b.rs -->",
            &PanicBase,
        );
        assert!(matches!(
            result,
            Fate::Undecidable(NoDeclaration::Refused { .. })
        ));
    }

    #[test]
    fn two_declarations_are_decidable() {
        assert!(!matches!(
            fate(
                "<!-- fb:creates a.rs -->\n<!-- fb:creates b.rs -->",
                &base(&[])
            ),
            Fate::Undecidable(NoDeclaration::Ambiguous(_))
        ));
    }

    #[test]
    fn zero_survey_entries_returns_empty() {
        assert!(survey(&[], &PanicBase).is_empty());
    }

    #[test]
    fn survey_preserves_entries_and_order() {
        let result = survey(
            &[
                ("first", "<!-- fb:creates a.rs -->"),
                ("second", "no marker"),
            ],
            &base(&[]),
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "first");
        assert_eq!(result[1].0, "second");
    }

    /// ZERO DECLARATIONS IS NOT STALE. `settled.len() == declaration_count` is `0 == 0` for
    /// a spec declaring nothing, which marks it dead -- and dead is the dangerous
    /// direction, because a stale spec is never dispatched again. The boundary clause says
    /// the opposite: a spec that asks for nothing has not been shown to have nothing left
    /// in it to do.
    ///
    /// `declared_all` returns `Err(Absent)` for an empty list, so this cannot be reached
    /// through it today. The guard exists because the rule must not rest on an invariant
    /// held in another module.
    #[test]
    fn a_spec_declaring_nothing_is_undecidable_and_never_stale() {
        struct Nothing;
        impl Base for Nothing {
            fn exists(&self, _: &str) -> bool {
                false
            }
        }
        assert_eq!(
            fate("# Task\nNo markers here at all.\n", &Nothing),
            Fate::Undecidable(NoDeclaration::Absent)
        );
    }
}

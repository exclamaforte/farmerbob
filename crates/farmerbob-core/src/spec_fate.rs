//! Dispatchability of queued specs against a given base.
//!
//! A spec that declares `fb:creates path` is stale once `path` exists on the
//! base — every correct implementation of it would be a no-op. A spec that
//! declares `fb:modifies path` is stale once `path` is gone. This module
//! encodes that check as a pure function of the spec text and a [`Base`]
//! answering existence queries, so any caller (dispatcher, queue UI, tests)
//! can ask without touching the filesystem.

use crate::target_decl::{self, Declaration, NoDeclaration};

/// Whether a queued spec can still be dispatched against a given base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// The spec's precondition holds on this base. Dispatch it.
    Runnable,
    /// The spec's precondition cannot hold on this base, so every correct
    /// implementation of it is a no-op. Carries the declared path and the
    /// grounds, for a human reading a queue listing.
    Stale { path: String, grounds: String },
    /// The spec declares no single deliverable, so nothing can be decided
    /// about it. Carries the reason from `target_decl`.
    Undecidable(NoDeclaration),
}

/// The base a spec is judged against: the set of paths that exist on it.
///
/// A caller builds this from a worktree, a git tree, or a test fixture. This
/// module never touches the filesystem.
pub trait Base {
    /// Whether `path` exists on this base.
    fn exists(&self, path: &str) -> bool;
}

/// The fate of one spec against one base.
///
/// Returns [`Fate::Runnable`] when the spec's declared precondition holds,
/// [`Fate::Stale`] when it cannot, and [`Fate::Undecidable`] when the spec
/// has no single unambiguous declaration.
///
/// This is a pure function: calling it twice with the same inputs gives the
/// same answer.
pub fn fate(spec: &str, base: &impl Base) -> Fate {
    match target_decl::declared(spec) {
        Ok(Declaration::Creates(path)) => {
            if base.exists(&path) {
                let grounds = format!(
                    "creates-task for '{}' but that path already exists on this base",
                    path
                );
                Fate::Stale { path, grounds }
            } else {
                Fate::Runnable
            }
        }
        Ok(Declaration::Modifies(path)) => {
            if base.exists(&path) {
                Fate::Runnable
            } else {
                let grounds = format!(
                    "modifies-task for '{}' but that path does not exist on this base",
                    path
                );
                Fate::Stale { path, grounds }
            }
        }
        Err(no_decl) => Fate::Undecidable(no_decl),
    }
}

/// The fate of many specs against one base, in the order given.
///
/// Returns one entry per input, paired with the name it was given. No entry
/// is dropped, merged or reordered. Two specs with the same name produce two
/// separate entries.
pub fn survey<'a>(specs: &[(&'a str, &'a str)], base: &impl Base) -> Vec<(&'a str, Fate)> {
    specs
        .iter()
        .map(|(name, spec)| (*name, fate(spec, base)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test fixture for [`Base`]: backed by a fixed set of paths.
    struct SetBase<'a> {
        paths: &'a [&'a str],
    }

    impl Base for SetBase<'_> {
        fn exists(&self, path: &str) -> bool {
            self.paths.contains(&path)
        }
    }

    /// A [`Base`] that panics on any call to `exists`. Used to verify that
    /// the base is never consulted for an Undecidable spec.
    struct PanicBase;

    impl Base for PanicBase {
        fn exists(&self, _path: &str) -> bool {
            panic!("Base::exists must not be called for an Undecidable spec");
        }
    }

    fn present<'a>(paths: &'a [&'a str]) -> SetBase<'a> {
        SetBase { paths }
    }

    // ── Clause 1: creates + path exists → Stale ───────────────────────────

    #[test]
    fn clause_1_creates_path_exists_is_stale() {
        let spec = "<!-- fb:creates a/b.rs -->";
        let base = present(&["a/b.rs"]);
        match fate(spec, &base) {
            Fate::Stale { path, grounds } => {
                assert_eq!(path, "a/b.rs");
                assert!(!grounds.is_empty(), "grounds must not be empty");
                assert!(grounds.contains("a/b.rs"), "grounds must mention the path");
            }
            other => panic!("expected Stale, got {:?}", other),
        }
    }

    // ── Clause 2: creates + path absent → Runnable ────────────────────────

    #[test]
    fn clause_2_creates_path_absent_is_runnable() {
        let spec = "<!-- fb:creates a/b.rs -->";
        let base = present(&[]);
        assert_eq!(fate(spec, &base), Fate::Runnable);
    }

    // ── Clause 3: modifies + path exists → Runnable; absent → Stale ───────

    #[test]
    fn clause_3a_modifies_path_exists_is_runnable() {
        let spec = "<!-- fb:modifies a/b.rs -->";
        let base = present(&["a/b.rs"]);
        assert_eq!(fate(spec, &base), Fate::Runnable);
    }

    #[test]
    fn clause_3b_modifies_path_absent_is_stale() {
        let spec = "<!-- fb:modifies a/b.rs -->";
        let base = present(&[]);
        match fate(spec, &base) {
            Fate::Stale { path, grounds } => {
                assert_eq!(path, "a/b.rs");
                assert!(!grounds.is_empty(), "grounds must not be empty");
                assert!(grounds.contains("a/b.rs"), "grounds must mention the path");
            }
            other => panic!("expected Stale, got {:?}", other),
        }
    }

    // ── Clause 5: no fb: marker → Undecidable(Absent) ─────────────────────

    #[test]
    fn clause_5_no_marker_is_undecidable_absent() {
        let spec = "# A task with no deliverable marker";
        assert_eq!(
            fate(spec, &PanicBase),
            Fate::Undecidable(NoDeclaration::Absent)
        );
    }

    // ── Clause 6: two markers → Undecidable(Ambiguous), base not called ───

    #[test]
    fn clause_6_two_markers_undecidable_ambiguous_base_not_called() {
        let spec = "<!-- fb:creates a/b.rs -->\n<!-- fb:modifies c/d.rs -->\n";
        match fate(spec, &PanicBase) {
            Fate::Undecidable(NoDeclaration::Ambiguous(paths)) => {
                assert!(paths.len() >= 2, "expected at least two paths in Ambiguous");
            }
            other => panic!("expected Undecidable(Ambiguous(..)), got {:?}", other),
        }
    }

    // ── Clause 7: fate is pure ─────────────────────────────────────────────

    #[test]
    fn clause_7_fate_is_pure() {
        let spec = "<!-- fb:creates x.rs -->";
        let base = present(&["x.rs"]);
        assert_eq!(fate(spec, &base), fate(spec, &base));
    }

    // ── Boundary: survey(&[], base) is empty ──────────────────────────────

    #[test]
    fn boundary_survey_empty_input() {
        let result = survey(&[], &PanicBase);
        assert!(result.is_empty());
    }

    // ── Boundary: survey over one spec equals fate on that spec ───────────

    #[test]
    fn boundary_survey_single_agrees_with_fate() {
        let spec = "<!-- fb:creates foo.rs -->";
        let base = present(&[]);
        let surveyed = survey(&[("task-a", spec)], &base);
        assert_eq!(surveyed.len(), 1);
        assert_eq!(surveyed[0].0, "task-a");
        assert_eq!(surveyed[0].1, fate(spec, &base));
    }

    // ── Boundary: two specs with same name → two entries (no dedup) ───────

    #[test]
    fn boundary_survey_same_name_two_entries() {
        let spec = "<!-- fb:creates foo.rs -->";
        let base = present(&[]);
        let input = [("same", spec), ("same", spec)];
        let result = survey(&input, &base);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "same");
        assert_eq!(result[1].0, "same");
    }

    // ── Boundary: survey preserves input order ────────────────────────────

    #[test]
    fn boundary_survey_preserves_order() {
        let spec_a = "<!-- fb:creates alpha.rs -->";
        let spec_b = "<!-- fb:creates beta.rs -->";
        let base = present(&[]);
        let input = [("a", spec_a), ("b", spec_b)];
        let result = survey(&input, &base);
        assert_eq!(result[0].0, "a");
        assert_eq!(result[1].0, "b");
    }
}

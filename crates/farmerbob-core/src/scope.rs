//! Scope: whether a run stayed inside its declared deliverable.
//!
//! A task declares its deliverable as a SET of file paths. This module
//! decides, from paths alone and without any I/O, whether a run's changes
//! stayed inside that declaration: the declared files themselves, the module
//! declaration a new file needs in order to compile, and nothing else.
//!
//! The set was a single path until 2026-09-19, and the single path was never
//! a decision. It was inferred from an incident: one arm changed 51, 52 and
//! 51 files across three runs and was disqualified for wandering. It had not
//! wandered. It had run `cargo fmt` on a rustfmt-dirty repository, and 51 is
//! exactly the number of dirty files its changes intersected -- as
//! rustfmt.toml's own comment records. The gate was tightened to one file on
//! the strength of a formatter artefact.
//!
//! The cost of that was not theoretical. scope-blame/glm-53-flash added a
//! field to `Change` correctly and could not pass: two constructions of
//! `Change` live in reap_exec.rs, in the same crate and a different file, so
//! fixing them was OutOfScope and leaving them was TESTS-FAIL. No legal move.
//!
//! What the gate is actually for survives intact: an arm that touches a file
//! outside what its task declared is out of scope, and that is still measured
//! by exact string comparison with no guessing.
//!
//! Paths are compared as exact strings. Two spellings of the same file are
//! two different paths here, by design: guessing at path equivalence is how
//! a scope check silently permits something.

/// The files the task told the arm to produce, repo-relative, forward
/// slashes, no leading `"./"`.
///
/// Example: `["crates/farmerbob-core/src/scope.rs"]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// Every file the task declared. One entry is the common case and was
    /// the only case until 2026-09-19; a task spanning several files lists
    /// them all.
    pub targets: Vec<String>,
}

impl Declared {
    /// A declaration of a single file -- the common case, and what every
    /// caller wrote before the set existed.
    pub fn one(target: impl Into<String>) -> Declared {
        Declared {
            targets: vec![target.into()],
        }
    }

    /// Whether `path` is one of the declared files. Exact string comparison,
    /// like everything else here.
    pub fn covers(&self, path: &str) -> bool {
        self.targets.iter().any(|t| t == path)
    }
}

/// The handoff every spec REQUIRES the arm to write.
///
/// `_handoff.md` is appended to every dispatched spec and says, under the heading
/// "Handoff (required)": *write `.fb/handoff.md` in the repository root*.
///
/// The harness does not provision this file -- the arm writes it -- so it is correctly not
/// `harness_owned`. But being the arm's own work is not the same as being OUT OF SCOPE: the
/// harness commanded it. Until 2026-09-19 the scope gate only looked inside `crates/`, so
/// it never saw the handoff; widening the universe (bead farmerbob-0s8x) made it visible
/// and every obedient arm was immediately marked OUT-OF-SCOPE for obeying.
pub const REQUIRED_OUTPUT: &str = ".fb/handoff.md";

/// Whether a path is output the spec REQUIRED, so producing it is never a departure.
pub fn required_output(path: &str) -> bool {
    path == REQUIRED_OUTPUT
}

/// A path that is permitted even though it is not the target.
///
/// This enum names the grants [`assess`] may apply. The list is CLOSED: a third grant would
/// be a change to this enum, not an extension of it. No function returns an `Allowance`;
/// the enum exists so each grant has a name in the type system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allowance {
    /// The handoff `_handoff.md` requires of every arm. Penalising an arm for writing a
    /// file the harness told it to write measures obedience as a fault.
    RequiredHandoff,
    /// The `lib.rs` in the same directory as the target. Adding
    /// `pub mod y;` to the crate's own `lib.rs` is required to make a new
    /// module compile, so it is not a violation.
    ModuleDeclaration,
}

/// Whether a departure changed behaviour, or only layout.
///
/// A formatting-only departure is still a departure -- it is still a rule
/// broken, and it still costs an adjudicator the trouble of stripping it --
/// but it cannot do the thing the scope gate exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The change alters what the file means.
    Semantic,
    /// The change alters only whitespace and line breaks.
    FormattingOnly,
}

/// A change to a file that is neither the target nor allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Departure {
    /// Changed a file that is neither the target nor allowed.
    Foreign {
        /// The path exactly as it appeared in the change record.
        path: String,
        /// Whether this change altered what the file means, or only its
        /// layout, as [`Change::formatting_only`] reported it.
        kind: Kind,
    },
    /// Deleted a file the run was not asked to touch.
    Deleted {
        /// The path exactly as it appeared in the change record.
        path: String,
        /// Always [`Kind::Semantic`]: deleting a file is not a formatting
        /// change, whatever [`Change::formatting_only`] said.
        kind: Kind,
    },
}

/// The scope verdict for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// True when the target itself was changed. Deleting the target does
    /// not set this.
    pub target_changed: bool,
    /// Permitted non-target paths that were changed, sorted ascending, no
    /// duplicates.
    pub allowed: Vec<String>,
    /// Violations, sorted ascending by the contained path, no duplicates.
    pub departures: Vec<Departure>,
}

/// One observed change to a path in the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Repo-relative path, exactly as observed.
    pub path: String,
    /// True when the change is a deletion.
    pub deleted: bool,
    /// True when this change is whitespace-only -- what
    /// `git diff --ignore-all-space --ignore-blank-lines` reports as empty.
    /// The caller measures it; this module performs no I/O and runs no
    /// formatter.
    pub formatting_only: bool,
}

/// Returns the `lib.rs` sitting in the same directory as `target` — the file
/// a new module must be declared in to compile.
///
/// Returns `None` when `target` is itself a `lib.rs`, when it has no `/`,
/// or when it is empty. The file is not looked up: this module performs no
/// I/O, so existence is not checked.
///
/// # Examples
///
/// ```
/// use farmerbob_core::scope::module_declaration_for;
///
/// assert_eq!(
///     module_declaration_for("crates/x/src/y.rs"),
///     Some(String::from("crates/x/src/lib.rs"))
/// );
/// assert_eq!(module_declaration_for("crates/x/src/lib.rs"), None);
/// assert_eq!(module_declaration_for("main.rs"), None);
/// assert_eq!(module_declaration_for(""), None);
/// ```
pub fn module_declaration_for(target: &str) -> Option<String> {
    let (dir, file) = target.rsplit_once('/')?;
    if file == "lib.rs" {
        return None;
    }
    Some(format!("{dir}/lib.rs"))
}

/// Every file in the target's directory that could legitimately declare it as a module.
///
/// A library crate declares modules from `lib.rs`; a BINARY crate declares them from
/// `main.rs`, and this module originally knew only the first. `fb` is a binary crate, so a
/// port task whose spec says "declare it from main.rs with `mod critique;`" had that very
/// file reported as a scope departure -- the harness flagging an arm for doing exactly what
/// it was told.
///
/// Both roots are returned rather than guessing which kind of crate this is: nothing here
/// reads the filesystem or Cargo.toml, and an arm that edits the one its crate does not use
/// has still only touched an empty or nonexistent file. Returns an empty `Vec` when `target`
/// is itself a crate root or has no directory.
pub fn module_declarations_for(target: &str) -> Vec<String> {
    let Some((dir, file)) = target.rsplit_once('/') else {
        return Vec::new();
    };
    if file == "lib.rs" || file == "main.rs" {
        return Vec::new();
    }
    vec![format!("{dir}/lib.rs"), format!("{dir}/main.rs")]
}

/// Classifies every change against the declared target.
///
/// A change to the target sets [`Scope::target_changed`] (unless it deletes
/// the target, which is a [`Departure::Deleted`]). A change to the target's
/// module declaration is [`Allowance::ModuleDeclaration`] and lands in
/// [`Scope::allowed`] — unless it deletes that file, which is again a
/// departure. Every other change is a departure, [`Departure::Foreign`] or
/// [`Departure::Deleted`] according to [`Change::deleted`], carrying
/// [`Kind::FormattingOnly`] when [`Change::formatting_only`] says the diff
/// was whitespace-only -- except that a deletion is always
/// [`Kind::Semantic`], whatever the flag says, because deleting a file is
/// not a formatting change.
///
/// Paths are compared as exact strings; nothing is normalised.
pub fn assess(declared: &Declared, changes: &[Change]) -> Scope {
    // One module-declaration allowance per declared file: a task creating three
    // modules needs three `pub mod` lines, and granting only the first one's would
    // reproduce the no-legal-move trap this set was introduced to remove.
    let mut declarations: Vec<String> = declared
        .targets
        .iter()
        .flat_map(|t| module_declarations_for(t))
        .collect();
    declarations.sort();
    declarations.dedup();

    let mut scope = Scope {
        target_changed: false,
        allowed: Vec::new(),
        departures: Vec::new(),
    };

    for change in changes {
        let kind = if change.formatting_only && !change.deleted {
            Kind::FormattingOnly
        } else {
            Kind::Semantic
        };

        // The harness told the arm to write this. Producing it cannot be a departure, and
        // a DELETION of it is not either -- there is nothing there to protect.
        if required_output(&change.path) {
            scope.allowed.push(change.path.clone());
            continue;
        }

        if declared.covers(&change.path) {
            // A task asks for a file to exist; deleting it is not
            // producing it.
            if change.deleted {
                scope.departures.push(Departure::Deleted {
                    path: change.path.clone(),
                    kind,
                });
            } else {
                scope.target_changed = true;
            }
        } else if allowance_for(&change.path, &declarations).is_some() {
            // Deleting the declaration is a violation like any other
            // deletion, not an exercised allowance.
            if change.deleted {
                scope.departures.push(Departure::Deleted {
                    path: change.path.clone(),
                    kind,
                });
            } else {
                scope.allowed.push(change.path.clone());
            }
        } else if change.deleted {
            scope.departures.push(Departure::Deleted {
                path: change.path.clone(),
                kind,
            });
        } else {
            scope.departures.push(Departure::Foreign {
                path: change.path.clone(),
                kind,
            });
        }
    }

    scope.allowed.sort();
    scope.allowed.dedup();
    sort_and_dedup_departures(&mut scope.departures);

    scope
}

/// True when the run strayed nowhere.
///
/// This does **not** require [`Scope::target_changed`]: a run that produced
/// nothing at all is outside this module's question, so an empty change set
/// is clean here.
pub fn is_clean(scope: &Scope) -> bool {
    scope.departures.is_empty()
}

/// Departures the agent is answerable for: those of kind [`Kind::Semantic`].
///
/// This is the figure a verdict keys on. [`Scope::departures`] keeps every
/// departure, formatting-only ones included -- `scope.departures.len()` is
/// the figure to REPORT -- and they are different numbers on purpose: a run
/// whose every departure is formatting-only has departures and no semantic
/// ones at all.
pub fn semantic_departures(scope: &Scope) -> usize {
    scope
        .departures
        .iter()
        .filter(|departure| {
            matches!(
                departure,
                Departure::Foreign {
                    kind: Kind::Semantic,
                    ..
                } | Departure::Deleted {
                    kind: Kind::Semantic,
                    ..
                }
            )
        })
        .count()
}

/// Whether the run departed in a way that could affect the measurement.
///
/// False for a run whose every departure is [`Kind::FormattingOnly`], true
/// as soon as one is [`Kind::Semantic`]. A run with no departures at all is
/// semantically clean too, so this agrees with [`is_clean`] there and
/// disagrees exactly when a formatting-only departure is being discounted.
pub fn is_semantically_clean(scope: &Scope) -> bool {
    semantic_departures(scope) == 0
}

/// The one allowance this module grants, if the path earns one.
fn allowance_for(path: &str, declarations: &[String]) -> Option<Allowance> {
    declarations
        .iter()
        .any(|d| d == path)
        .then_some(Allowance::ModuleDeclaration)
}

impl Departure {
    /// The path this departure is about, for ordering and de-duplication.
    fn path(&self) -> &str {
        match self {
            Departure::Foreign { path, .. } => path,
            Departure::Deleted { path, .. } => path,
        }
    }
}

/// Orders `Deleted` ahead of `Foreign` within an equal path, so the first
/// of each run after the sort is the one the contract keeps.
fn deletion_rank(departure: &Departure) -> u8 {
    match departure {
        Departure::Deleted { .. } => 0,
        Departure::Foreign { .. } => 1,
    }
}

/// Sorts departures ascending by path across both variants and collapses
/// equal paths to one entry, preferring `Deleted`. The entry kept is the
/// first of its run after the sort, and its [`Kind`] rides along with it.
fn sort_and_dedup_departures(departures: &mut Vec<Departure>) {
    departures.sort_by(|a, b| {
        a.path()
            .cmp(b.path())
            .then_with(|| deletion_rank(a).cmp(&deletion_rank(b)))
    });
    departures.dedup_by(|a, b| a.path() == b.path());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(target: &str) -> Declared {
        Declared::one(target.to_string())
    }

    fn change(path: &str) -> Change {
        Change {
            path: path.to_string(),
            deleted: false,
            formatting_only: false,
        }
    }

    fn deleted(path: &str) -> Change {
        Change {
            path: path.to_string(),
            deleted: true,
            formatting_only: false,
        }
    }

    fn formatting_only(path: &str) -> Change {
        Change {
            path: path.to_string(),
            deleted: false,
            formatting_only: true,
        }
    }

    // Rule 1: the declaration is the sibling lib.rs, and None for a lib.rs
    // target, a slash-less path, or an empty target.
    #[test]
    fn rule1_module_declaration_is_the_sibling_lib_rs() {
        assert_eq!(
            module_declaration_for("crates/x/src/y.rs"),
            Some(String::from("crates/x/src/lib.rs"))
        );
        assert_eq!(
            module_declaration_for("a/b/c/deeper.txt"),
            Some(String::from("a/b/c/lib.rs"))
        );
    }

    #[test]
    fn rule1_module_declaration_is_none_for_lib_rs_target() {
        assert_eq!(module_declaration_for("crates/x/src/lib.rs"), None);
        assert_eq!(module_declaration_for("lib.rs"), None);
    }

    #[test]
    fn rule1_module_declaration_is_none_without_a_slash() {
        assert_eq!(module_declaration_for("main.rs"), None);
    }

    #[test]
    fn rule1_module_declaration_is_none_for_an_empty_target() {
        assert_eq!(module_declaration_for(""), None);
    }

    // Rule 2: a change to the target sets target_changed and appears
    // nowhere else.
    #[test]
    fn rule2_a_change_to_the_target_sets_target_changed_alone() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[change("crates/x/src/y.rs")],
        );
        assert!(scope.target_changed);
        assert!(scope.allowed.is_empty());
        assert!(scope.departures.is_empty());
        assert!(is_clean(&scope));
    }

    // Rule 3: the module declaration is allowed, not a departure.
    #[test]
    fn rule3_the_module_declaration_is_allowed_not_a_departure() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[change("crates/x/src/lib.rs")],
        );
        assert!(!scope.target_changed);
        assert_eq!(scope.allowed, vec![String::from("crates/x/src/lib.rs")]);
        assert!(scope.departures.is_empty());
        assert!(is_clean(&scope));
    }

    #[test]
    fn rule3_target_and_declaration_change_together_cleanly() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[change("crates/x/src/lib.rs"), change("crates/x/src/y.rs")],
        );
        assert!(scope.target_changed);
        assert_eq!(scope.allowed, vec![String::from("crates/x/src/lib.rs")]);
        assert!(scope.departures.is_empty());
    }

    // Rule 4: every other path is a departure, Foreign or Deleted by kind.
    #[test]
    fn rule4_other_changes_are_departures_of_the_matching_kind() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[
                change("crates/x/src/other.rs"),
                deleted("crates/y/src/gone.rs"),
            ],
        );
        assert!(!scope.target_changed);
        assert!(scope.allowed.is_empty());
        assert_eq!(
            scope.departures,
            vec![
                Departure::Foreign {
                    path: String::from("crates/x/src/other.rs"),
                    kind: Kind::Semantic,
                },
                Departure::Deleted {
                    path: String::from("crates/y/src/gone.rs"),
                    kind: Kind::Semantic,
                },
            ]
        );
        assert!(!is_clean(&scope));
    }

    // Rule 4: even the target's own lib.rs, deleted, is a violation.
    #[test]
    fn rule4_deleting_the_allowed_declaration_is_a_violation() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[deleted("crates/x/src/lib.rs")],
        );
        assert!(scope.allowed.is_empty());
        assert_eq!(
            scope.departures,
            vec![Departure::Deleted {
                path: String::from("crates/x/src/lib.rs"),
                kind: Kind::Semantic,
            }]
        );
    }

    // Rule 5: deleting the target is a departure and leaves target_changed
    // false.
    #[test]
    fn rule5_deleting_the_target_is_a_departure_not_a_change() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[deleted("crates/x/src/y.rs")],
        );
        assert!(!scope.target_changed);
        assert!(scope.allowed.is_empty());
        assert_eq!(
            scope.departures,
            vec![Departure::Deleted {
                path: String::from("crates/x/src/y.rs"),
                kind: Kind::Semantic,
            }]
        );
        assert!(!is_clean(&scope));
    }

    // Rule 6: is_clean depends only on departures, never on target_changed.
    #[test]
    fn rule6_is_clean_ignores_target_changed() {
        let untouched = assess(&declared("crates/x/src/y.rs"), &[]);
        assert!(!untouched.target_changed);
        assert!(is_clean(&untouched));

        let produced = assess(
            &declared("crates/x/src/y.rs"),
            &[change("crates/x/src/y.rs")],
        );
        assert!(produced.target_changed);
        assert!(is_clean(&produced));

        let strayed = assess(&declared("crates/x/src/y.rs"), &[change("elsewhere.rs")]);
        assert!(!is_clean(&strayed));
    }

    // Boundary: empty changes give an empty scope, which is clean.
    #[test]
    fn boundary_empty_changes_yield_an_empty_clean_scope() {
        let scope = assess(&declared("crates/x/src/y.rs"), &[]);
        assert!(!scope.target_changed);
        assert!(scope.allowed.is_empty());
        assert!(scope.departures.is_empty());
        assert!(is_clean(&scope));
    }

    // Boundary: an empty target means nothing can equal it and there is no
    // declaration, so every change is a departure.
    #[test]
    fn boundary_empty_target_makes_every_change_a_departure() {
        let scope = assess(
            &declared(""),
            &[
                change("crates/x/src/y.rs"),
                change("lib.rs"),
                deleted("other.rs"),
            ],
        );
        assert!(!scope.target_changed);
        assert!(scope.allowed.is_empty());
        assert_eq!(scope.departures.len(), 3);
    }

    // Boundary: a path appearing twice with the same kind contributes one
    // entry.
    #[test]
    fn boundary_identical_duplicates_collapse_to_one_entry() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[
                change("crates/a.rs"),
                change("crates/a.rs"),
                deleted("crates/b.rs"),
                deleted("crates/b.rs"),
                change("crates/x/src/lib.rs"),
                change("crates/x/src/lib.rs"),
            ],
        );
        assert_eq!(scope.allowed, vec![String::from("crates/x/src/lib.rs")]);
        assert_eq!(
            scope.departures,
            vec![
                Departure::Foreign {
                    path: String::from("crates/a.rs"),
                    kind: Kind::Semantic,
                },
                Departure::Deleted {
                    path: String::from("crates/b.rs"),
                    kind: Kind::Semantic,
                },
            ]
        );

        let target_twice = assess(
            &declared("crates/x/src/y.rs"),
            &[change("crates/x/src/y.rs"), change("crates/x/src/y.rs")],
        );
        assert!(target_twice.target_changed);
        assert!(target_twice.departures.is_empty());
    }

    // Boundary: one path both changed and deleted keeps the Deleted entry.
    #[test]
    fn boundary_conflicting_duplicates_keep_the_deleted_one() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[
                change("crates/b.rs"),
                deleted("crates/b.rs"),
                change("crates/a.rs"),
            ],
        );
        assert_eq!(
            scope.departures,
            vec![
                Departure::Foreign {
                    path: String::from("crates/a.rs"),
                    kind: Kind::Semantic,
                },
                Departure::Deleted {
                    path: String::from("crates/b.rs"),
                    kind: Kind::Semantic,
                },
            ]
        );
    }

    // Boundary: departures sort by path over the whole list, Foreign and
    // Deleted interleaving.
    #[test]
    fn boundary_departures_interleave_by_path_across_variants() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[
                deleted("crates/d.rs"),
                change("crates/a.rs"),
                deleted("crates/b.rs"),
                change("crates/c.rs"),
            ],
        );
        assert_eq!(
            scope.departures,
            vec![
                Departure::Foreign {
                    path: String::from("crates/a.rs"),
                    kind: Kind::Semantic,
                },
                Departure::Deleted {
                    path: String::from("crates/b.rs"),
                    kind: Kind::Semantic,
                },
                Departure::Foreign {
                    path: String::from("crates/c.rs"),
                    kind: Kind::Semantic,
                },
                Departure::Deleted {
                    path: String::from("crates/d.rs"),
                    kind: Kind::Semantic,
                },
            ]
        );
    }

    // Boundary: paths are compared as exact strings, never normalised.
    #[test]
    fn boundary_path_spellings_are_not_normalised() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[
                change("./crates/x/src/y.rs"),
                change("crates/x/src/../src/y.rs"),
            ],
        );
        assert!(!scope.target_changed);
        assert!(scope.allowed.is_empty());
        assert_eq!(scope.departures.len(), 2);
    }

    // Clause 1: the formatting_only flag decides a foreign departure's
    // kind -- both ways, over one input containing both.
    #[test]
    fn clause1_formatting_only_flag_decides_a_foreign_departures_kind() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[change("crates/a.rs"), formatting_only("crates/b.rs")],
        );
        assert_eq!(
            scope.departures,
            vec![
                Departure::Foreign {
                    path: String::from("crates/a.rs"),
                    kind: Kind::Semantic,
                },
                Departure::Foreign {
                    path: String::from("crates/b.rs"),
                    kind: Kind::FormattingOnly,
                },
            ]
        );
    }

    // Clause 2, the headline: departures without semantic ones. All four
    // figures pinned on the one value.
    #[test]
    fn clause2_all_formatting_only_departures_are_semantically_clean_but_not_clean() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[
                formatting_only("crates/a.rs"),
                formatting_only("crates/b.rs"),
            ],
        );
        assert_eq!(scope.departures.len(), 2);
        assert_eq!(semantic_departures(&scope), 0);
        assert!(is_semantically_clean(&scope));
        assert!(!is_clean(&scope));
    }

    // Clause 3: one Semantic departure among formatting-only ones makes the
    // run not semantically clean. The semantic change is LAST in the input
    // (and sorts last) so an implementation that stops early is caught.
    #[test]
    fn clause3_one_semantic_departure_makes_the_run_not_semantically_clean() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[
                formatting_only("crates/a.rs"),
                formatting_only("crates/b.rs"),
                change("crates/z.rs"),
            ],
        );
        assert_eq!(semantic_departures(&scope), 1);
        assert!(!is_semantically_clean(&scope));
        assert!(!is_clean(&scope));
    }

    // Clause 4: a deletion is Semantic even when the caller flags it
    // formatting-only; the flag cannot make a deletion a layout change.
    #[test]
    fn clause4_a_deleted_change_is_semantic_even_when_flagged_formatting_only() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[Change {
                path: String::from("crates/gone.rs"),
                deleted: true,
                formatting_only: true,
            }],
        );
        assert_eq!(
            scope.departures,
            vec![Departure::Deleted {
                path: String::from("crates/gone.rs"),
                kind: Kind::Semantic,
            }]
        );
        assert_eq!(semantic_departures(&scope), 1);
        assert!(!is_semantically_clean(&scope));
    }

    // Clause 5: is_clean keeps its meaning -- false whenever departures
    // exist, whatever their kinds. The formatting-only field is what
    // separates it from is_semantically_clean (clause 2 pins that side).
    #[test]
    fn clause5_is_clean_counts_formatting_only_departures_too() {
        let mixed = assess(
            &declared("crates/x/src/y.rs"),
            &[
                formatting_only("crates/a.rs"),
                change("crates/z.rs"),
                deleted("crates/d.rs"),
            ],
        );
        assert_eq!(mixed.departures.len(), 3);
        assert!(!is_clean(&mixed));
        assert!(!is_semantically_clean(&mixed));

        let all_formatting = assess(
            &declared("crates/x/src/y.rs"),
            &[formatting_only("crates/a.rs")],
        );
        assert!(!is_clean(&all_formatting));
        assert!(is_semantically_clean(&all_formatting));
    }

    // Clause 6: target_changed and allowed are untouched. A non-deleted
    // change to the target is not a departure, and formatting_only has no
    // bearing on that.
    #[test]
    fn clause6_a_target_change_is_not_a_departure_whatever_the_flag_says() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[Change {
                path: String::from("crates/x/src/y.rs"),
                deleted: false,
                formatting_only: true,
            }],
        );
        assert!(scope.target_changed);
        assert!(scope.allowed.is_empty());
        assert!(scope.departures.is_empty());
        assert!(is_clean(&scope));
        assert!(is_semantically_clean(&scope));
        assert_eq!(semantic_departures(&scope), 0);
    }

    // Clause 6: an allowed path with formatting_only true is still allowed,
    // still not a departure.
    #[test]
    fn clause6_an_allowed_path_with_formatting_only_stays_allowed() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[Change {
                path: String::from("crates/x/src/lib.rs"),
                deleted: false,
                formatting_only: true,
            }],
        );
        assert!(!scope.target_changed);
        assert_eq!(scope.allowed, vec![String::from("crates/x/src/lib.rs")]);
        assert!(scope.departures.is_empty());
        assert!(is_clean(&scope));
        assert!(is_semantically_clean(&scope));
    }

    // Boundary at zero: no departures at all, and all three figures agree.
    // That agreement is what makes clause 2's disagreement meaningful.
    #[test]
    fn boundary_with_no_departures_all_three_figures_agree() {
        let scope = assess(&declared("crates/x/src/y.rs"), &[]);
        assert!(is_clean(&scope));
        assert!(is_semantically_clean(&scope));
        assert_eq!(semantic_departures(&scope), 0);

        let target_only = assess(
            &declared("crates/x/src/y.rs"),
            &[change("crates/x/src/y.rs")],
        );
        assert!(target_only.departures.is_empty());
        assert!(is_clean(&target_only));
        assert!(is_semantically_clean(&target_only));
        assert_eq!(semantic_departures(&target_only), 0);
    }

    // Boundary at one: a single formatting-only departure already separates
    // is_clean from is_semantically_clean. The boundary is at one, not at
    // some threshold.
    #[test]
    fn boundary_exactly_one_formatting_only_departure_still_counts_zero_semantic() {
        let scope = assess(
            &declared("crates/x/src/y.rs"),
            &[formatting_only("crates/a.rs")],
        );
        assert_eq!(scope.departures.len(), 1);
        assert_eq!(semantic_departures(&scope), 0);
        assert!(is_semantically_clean(&scope));
        assert!(!is_clean(&scope));
    }
}

#[cfg(test)]
mod binary_crate_roots {
    use super::*;

    fn changed(paths: &[&str]) -> Vec<Change> {
        paths
            .iter()
            .map(|p| Change {
                path: (*p).to_string(),
                deleted: false,
                formatting_only: false,
            })
            .collect()
    }

    /// `fb` is a BINARY crate: it declares modules from main.rs, not lib.rs. The first
    /// version knew only lib.rs, so a port task whose spec says "declare it from main.rs"
    /// had that exact file reported as a scope departure -- the harness flagging an arm for
    /// doing what it was told.
    #[test]
    fn declaring_a_module_from_main_rs_is_not_a_departure() {
        let declared = Declared::one("crates/fb/src/critique.rs");
        let sc = assess(
            &declared,
            &changed(&["crates/fb/src/critique.rs", "crates/fb/src/main.rs"]),
        );
        assert!(sc.target_changed);
        assert_eq!(sc.allowed, vec!["crates/fb/src/main.rs".to_string()]);
        assert!(is_clean(&sc), "main.rs is a crate root, not a stray edit");
    }

    /// A library crate's root still works exactly as before.
    #[test]
    fn declaring_a_module_from_lib_rs_is_still_not_a_departure() {
        let declared = Declared::one("crates/farmerbob-core/src/scope.rs");
        let sc = assess(
            &declared,
            &changed(&[
                "crates/farmerbob-core/src/scope.rs",
                "crates/farmerbob-core/src/lib.rs",
            ]),
        );
        assert_eq!(
            sc.allowed,
            vec!["crates/farmerbob-core/src/lib.rs".to_string()]
        );
        assert!(is_clean(&sc));
    }

    /// The allowance covers the roots and nothing else: a real stray edit still departs.
    #[test]
    fn a_sibling_module_is_still_a_departure() {
        let declared = Declared::one("crates/fb/src/critique.rs");
        let sc = assess(
            &declared,
            &changed(&[
                "crates/fb/src/critique.rs",
                "crates/fb/src/main.rs",
                "crates/fb/src/score.rs",
            ]),
        );
        assert_eq!(
            sc.departures.len(),
            1,
            "score.rs is not a crate root: {:?}",
            sc.departures
        );
        assert!(!is_clean(&sc));
    }

    /// Deleting a crate root is a violation, exactly as deleting lib.rs always was.
    #[test]
    fn deleting_a_crate_root_is_still_a_departure() {
        let declared = Declared::one("crates/fb/src/critique.rs");
        let sc = assess(
            &declared,
            &[
                Change {
                    path: "crates/fb/src/critique.rs".into(),
                    deleted: false,
                    formatting_only: false,
                },
                Change {
                    path: "crates/fb/src/main.rs".into(),
                    deleted: true,
                    formatting_only: false,
                },
            ],
        );
        assert!(sc.allowed.is_empty());
        assert_eq!(sc.departures.len(), 1);
    }

    #[test]
    fn a_crate_root_target_has_no_declaration_of_its_own() {
        assert!(module_declarations_for("crates/fb/src/main.rs").is_empty());
        assert!(module_declarations_for("crates/x/src/lib.rs").is_empty());
        assert!(module_declarations_for("no-directory.rs").is_empty());
        assert!(module_declarations_for("").is_empty());
    }

    /// A task may declare SEVERAL files, and touching any of them is not a
    /// departure. This is the case that was unrepresentable until 2026-09-19,
    /// and its absence failed an arm that had done nothing wrong.
    #[test]
    fn a_task_may_declare_several_files() {
        let declared = Declared {
            targets: vec![
                "crates/farmerbob-core/src/scope.rs".to_string(),
                "crates/farmerbob-core/src/reap_exec.rs".to_string(),
            ],
        };
        let sc = assess(
            &declared,
            &[
                Change {
                    path: "crates/farmerbob-core/src/scope.rs".to_string(),
                    deleted: false,
                    formatting_only: false,
                },
                Change {
                    path: "crates/farmerbob-core/src/reap_exec.rs".to_string(),
                    deleted: false,
                    formatting_only: false,
                },
            ],
        );
        assert!(sc.target_changed);
        assert!(is_clean(&sc), "both declared files are in scope: {sc:?}");
    }

    /// The gate still does its job: a file OUTSIDE the declared set is a
    /// departure, however many files the set holds. Same input as the test
    /// above with one path added -- the two answers must differ.
    #[test]
    fn a_file_outside_a_multi_file_declaration_is_still_a_departure() {
        let declared = Declared {
            targets: vec![
                "crates/farmerbob-core/src/scope.rs".to_string(),
                "crates/farmerbob-core/src/reap_exec.rs".to_string(),
            ],
        };
        let sc = assess(
            &declared,
            &[
                Change {
                    path: "crates/farmerbob-core/src/scope.rs".to_string(),
                    deleted: false,
                    formatting_only: false,
                },
                Change {
                    path: "crates/fb/src/pareto.rs".to_string(),
                    deleted: false,
                    formatting_only: false,
                },
            ],
        );
        assert!(!is_clean(&sc));
        assert_eq!(sc.departures.len(), 1);
    }

    /// Each declared file earns its own module-declaration allowance. A task
    /// creating three modules needs three `pub mod` lines, and granting only
    /// the first file's would rebuild the no-legal-move trap.
    #[test]
    fn every_declared_file_earns_its_module_declaration() {
        let declared = Declared {
            targets: vec![
                "crates/a/src/one.rs".to_string(),
                "crates/b/src/two.rs".to_string(),
            ],
        };
        let sc = assess(
            &declared,
            &[
                Change {
                    path: "crates/a/src/lib.rs".to_string(),
                    deleted: false,
                    formatting_only: false,
                },
                Change {
                    path: "crates/b/src/lib.rs".to_string(),
                    deleted: false,
                    formatting_only: false,
                },
            ],
        );
        assert!(is_clean(&sc), "both lib.rs files are allowed: {sc:?}");
        assert_eq!(sc.allowed.len(), 2);
    }

    /// An empty declaration declares nothing, so every change is a departure.
    /// Not an error, and not a free pass.
    #[test]
    fn an_empty_declaration_permits_nothing() {
        let declared = Declared { targets: vec![] };
        let sc = assess(
            &declared,
            &[Change {
                path: "crates/a/src/one.rs".to_string(),
                deleted: false,
                formatting_only: false,
            }],
        );
        assert!(!sc.target_changed);
        assert_eq!(sc.departures.len(), 1);
    }

    /// THE HARNESS COMMANDED THIS FILE. `_handoff.md` is appended to every dispatched spec
    /// and says, under "Handoff (required)", to write `.fb/handoff.md`. Marking an arm
    /// OUT-OF-SCOPE for doing so measures obedience as a fault, and the verdict feeds the
    /// bandit.
    ///
    /// It went unnoticed because the gate only looked inside `crates/` until 2026-09-19.
    /// Widening the universe (farmerbob-0s8x) made the handoff visible, and the very next
    /// run -- novelty/codex-luna, which wrote a 7-line handoff exactly as instructed --
    /// came back OUT-OF-SCOPE with that as its only departure.
    #[test]
    fn writing_the_required_handoff_is_never_a_departure() {
        let declared = Declared::one("crates/farmerbob-core/src/novelty.rs");
        let changes = vec![
            Change {
                path: "crates/farmerbob-core/src/novelty.rs".to_string(),
                formatting_only: false,
                deleted: false,
            },
            Change {
                path: ".fb/handoff.md".to_string(),
                formatting_only: false,
                deleted: false,
            },
        ];
        let scope = assess(&declared, &changes);
        assert!(
            scope.departures.is_empty(),
            "the handoff is required output: {:?}",
            scope.departures
        );
        assert!(scope.allowed.contains(&".fb/handoff.md".to_string()));
        assert!(scope.target_changed);
    }

    /// Only THAT path. A different file under `.fb/` is not required output, and letting
    /// the allowance be a prefix would hand every arm the whole harness directory.
    #[test]
    fn the_allowance_is_the_one_path_and_not_a_prefix() {
        assert!(required_output(".fb/handoff.md"));
        assert!(!required_output(".fb/handoff.md.bak"));
        assert!(!required_output(".fb/prompts/x.md"));
        assert!(!required_output(".fb"));
        assert!(!required_output(""));
    }

    /// A handoff alone is not the task. The allowance stops it counting AGAINST the arm; it
    /// must not make an untouched deliverable look changed.
    #[test]
    fn a_handoff_alone_does_not_count_as_touching_the_target() {
        let declared = Declared::one("crates/x/src/y.rs");
        let scope = assess(
            &declared,
            &[Change {
                path: ".fb/handoff.md".to_string(),
                formatting_only: false,
                deleted: false,
            }],
        );
        assert!(!scope.target_changed, "the deliverable was never written");
        assert!(scope.departures.is_empty());
    }
}

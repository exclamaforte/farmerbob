//! Scope: whether a run stayed inside its declared deliverable.
//!
//! Every task in the harness declares exactly one deliverable — one file
//! path. This module decides, from paths alone and without any I/O, whether
//! a run's changes stayed inside that declaration: the target itself, the
//! one module declaration a new file needs in order to compile, and nothing
//! else.
//!
//! Paths are compared as exact strings. Two spellings of the same file are
//! two different paths here, by design: guessing at path equivalence is how
//! a scope check silently permits something.

/// The one file the task told the arm to produce, repo-relative,
/// forward slashes, no leading `"./"`.
///
/// Example: `"crates/farmerbob-core/src/scope.rs"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// The one file the task told the arm to produce.
    pub target: String,
}

/// A path that is permitted even though it is not the target.
///
/// This enum names the grants [`assess`] may apply. There is exactly one:
/// [`Allowance::ModuleDeclaration`]. No function returns an `Allowance`;
/// the enum exists so the grant has a name in the type system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allowance {
    /// The `lib.rs` in the same directory as the target. Adding
    /// `pub mod y;` to the crate's own `lib.rs` is required to make a new
    /// module compile, so it is not a violation. This is the only allowance.
    ModuleDeclaration,
}

/// A change to a file that is neither the target nor allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Departure {
    /// Changed a file that is neither the target nor allowed.
    Foreign {
        /// The path exactly as it appeared in the change record.
        path: String,
    },
    /// Deleted a file the run was not asked to touch.
    Deleted {
        /// The path exactly as it appeared in the change record.
        path: String,
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
/// [`Departure::Deleted`] according to [`Change::deleted`].
///
/// Paths are compared as exact strings; nothing is normalised.
pub fn assess(declared: &Declared, changes: &[Change]) -> Scope {
    let declarations = module_declarations_for(&declared.target);

    let mut scope = Scope {
        target_changed: false,
        allowed: Vec::new(),
        departures: Vec::new(),
    };

    for change in changes {
        if change.path == declared.target {
            // A task asks for a file to exist; deleting it is not
            // producing it.
            if change.deleted {
                scope
                    .departures
                    .push(Departure::Deleted { path: change.path.clone() });
            } else {
                scope.target_changed = true;
            }
        } else if allowance_for(&change.path, &declarations).is_some() {
            // Deleting the declaration is a violation like any other
            // deletion, not an exercised allowance.
            if change.deleted {
                scope
                    .departures
                    .push(Departure::Deleted { path: change.path.clone() });
            } else {
                scope.allowed.push(change.path.clone());
            }
        } else if change.deleted {
            scope
                .departures
                .push(Departure::Deleted { path: change.path.clone() });
        } else {
            scope
                .departures
                .push(Departure::Foreign { path: change.path.clone() });
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
            Departure::Foreign { path } => path,
            Departure::Deleted { path } => path,
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
/// equal paths to one entry, preferring `Deleted`.
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
        Declared {
            target: target.to_string(),
        }
    }

    fn change(path: &str) -> Change {
        Change {
            path: path.to_string(),
            deleted: false,
        }
    }

    fn deleted(path: &str) -> Change {
        Change {
            path: path.to_string(),
            deleted: true,
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
        let scope = assess(&declared("crates/x/src/y.rs"), &[change("crates/x/src/y.rs")]);
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
            &[
                change("crates/x/src/lib.rs"),
                change("crates/x/src/y.rs"),
            ],
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
                    path: String::from("crates/x/src/other.rs")
                },
                Departure::Deleted {
                    path: String::from("crates/y/src/gone.rs")
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
                path: String::from("crates/x/src/lib.rs")
            }]
        );
    }

    // Rule 5: deleting the target is a departure and leaves target_changed
    // false.
    #[test]
    fn rule5_deleting_the_target_is_a_departure_not_a_change() {
        let scope = assess(&declared("crates/x/src/y.rs"), &[deleted("crates/x/src/y.rs")]);
        assert!(!scope.target_changed);
        assert!(scope.allowed.is_empty());
        assert_eq!(
            scope.departures,
            vec![Departure::Deleted {
                path: String::from("crates/x/src/y.rs")
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

        let produced = assess(&declared("crates/x/src/y.rs"), &[change("crates/x/src/y.rs")]);
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
                    path: String::from("crates/a.rs")
                },
                Departure::Deleted {
                    path: String::from("crates/b.rs")
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
                    path: String::from("crates/a.rs")
                },
                Departure::Deleted {
                    path: String::from("crates/b.rs")
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
                    path: String::from("crates/a.rs")
                },
                Departure::Deleted {
                    path: String::from("crates/b.rs")
                },
                Departure::Foreign {
                    path: String::from("crates/c.rs")
                },
                Departure::Deleted {
                    path: String::from("crates/d.rs")
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
}



#[cfg(test)]
mod binary_crate_roots {
    use super::*;

    fn changed(paths: &[&str]) -> Vec<Change> {
        paths.iter().map(|p| Change { path: (*p).to_string(), deleted: false }).collect()
    }

    /// `fb` is a BINARY crate: it declares modules from main.rs, not lib.rs. The first
    /// version knew only lib.rs, so a port task whose spec says "declare it from main.rs"
    /// had that exact file reported as a scope departure -- the harness flagging an arm for
    /// doing what it was told.
    #[test]
    fn declaring_a_module_from_main_rs_is_not_a_departure() {
        let declared = Declared { target: "crates/fb/src/critique.rs".into() };
        let sc = assess(&declared, &changed(&["crates/fb/src/critique.rs", "crates/fb/src/main.rs"]));
        assert!(sc.target_changed);
        assert_eq!(sc.allowed, vec!["crates/fb/src/main.rs".to_string()]);
        assert!(is_clean(&sc), "main.rs is a crate root, not a stray edit");
    }

    /// A library crate's root still works exactly as before.
    #[test]
    fn declaring_a_module_from_lib_rs_is_still_not_a_departure() {
        let declared = Declared { target: "crates/farmerbob-core/src/scope.rs".into() };
        let sc = assess(
            &declared,
            &changed(&["crates/farmerbob-core/src/scope.rs", "crates/farmerbob-core/src/lib.rs"]),
        );
        assert_eq!(sc.allowed, vec!["crates/farmerbob-core/src/lib.rs".to_string()]);
        assert!(is_clean(&sc));
    }

    /// The allowance covers the roots and nothing else: a real stray edit still departs.
    #[test]
    fn a_sibling_module_is_still_a_departure() {
        let declared = Declared { target: "crates/fb/src/critique.rs".into() };
        let sc = assess(
            &declared,
            &changed(&["crates/fb/src/critique.rs", "crates/fb/src/main.rs", "crates/fb/src/score.rs"]),
        );
        assert_eq!(sc.departures.len(), 1, "score.rs is not a crate root: {:?}", sc.departures);
        assert!(!is_clean(&sc));
    }

    /// Deleting a crate root is a violation, exactly as deleting lib.rs always was.
    #[test]
    fn deleting_a_crate_root_is_still_a_departure() {
        let declared = Declared { target: "crates/fb/src/critique.rs".into() };
        let sc = assess(
            &declared,
            &[
                Change { path: "crates/fb/src/critique.rs".into(), deleted: false },
                Change { path: "crates/fb/src/main.rs".into(), deleted: true },
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
}

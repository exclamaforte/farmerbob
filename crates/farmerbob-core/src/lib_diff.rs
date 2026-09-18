//! `lib_diff`: what a diff of `lib.rs` did, judged line by line.
//!
//! [`crate::scope::assess`] compares paths, and its one allowance —
//! [`crate::scope::Allowance::ModuleDeclaration`] — permits the `lib.rs`
//! beside the target as a path, because adding `pub mod y;` there is
//! required to make a new module compile. The rubric says the gate
//! "permits exactly two things: the declared target, and adding
//! `pub mod y;` to the `lib.rs` beside it". It cannot check the second
//! half of that sentence: once `lib.rs` is permitted as a path, anything
//! done inside it passes.
//!
//! On `store-batch` an arm used exactly that hole. It changed three
//! existing declarations in `lib.rs` to `pub(crate)` — a widening of what
//! the crate exposes that had nothing to do with declaring a module — and
//! scored clean, and had to be ruled out of scope by hand. This module
//! classifies a `lib.rs` diff so the gate makes that judgement itself. It
//! is read alongside `scope.rs`; replacing `Allowance` is a separate
//! decision that wants this evidence first.
//!
//! The permitted line shapes are a CLOSED set of exactly three:
//!
//! 1. an exact `pub mod y;`, the form the rubric quotes — one space, no
//!    body, no leading indentation, no trailing comment or code;
//! 2. a whitespace-only line, including an empty one;
//! 3. a comment-only line: the first non-whitespace content begins a `//`
//!    comment.
//!
//! Anything else is [`LibChange::Beyond`]. A rejected shape is reported as
//! `Beyond`, never silently permitted. The set is closed deliberately: an
//! open one would let the next unanticipated edit through, which is the
//! defect being fixed. A shape the rubric does not quote — a block comment
//! (`/* .. */`), a raw identifier (`r#`), an indented declaration — is a
//! shape the gate does not guess at, so none of them is in the set.

/// One line of a unified diff of `lib.rs`, already stripped of its leading
/// `+` or `-`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// The line's text, without the diff marker and without a trailing
    /// newline.
    pub text: String,
    /// True for an added line (`+`), false for a removed line (`-`).
    pub added: bool,
}

/// What a `lib.rs` diff did, beyond what the gate permits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibChange {
    /// Only module declarations were added, and nothing was removed.
    ///
    /// `modules` is sorted ascending and deduplicated, and MAY be empty: a
    /// diff that adds only blank or comment lines is a real edit to
    /// `lib.rs` that declares nothing.
    DeclarationsOnly {
        /// The modules declared, sorted ascending, without duplicates.
        modules: Vec<String>,
    },
    /// Something else happened. Every offending line, in diff order.
    ///
    /// `lines` carries only the offending lines — a permitted line in the
    /// same diff is not evidence of a violation and is not carried. It is
    /// never empty, and no guard makes it so: this variant is constructed
    /// only after at least one offending line has been collected, because
    /// a diff with no offending lines takes the
    /// [`LibChange::DeclarationsOnly`] branch instead.
    Beyond {
        /// Lines that are not permitted, in the order the diff supplied.
        lines: Vec<DiffLine>,
    },
    /// The diff is empty: `lib.rs` was not touched.
    ///
    /// Distinct from [`LibChange::DeclarationsOnly`] with an empty
    /// `modules`: that is a diff which WAS supplied — lines were added to
    /// `lib.rs` — and happened to declare nothing. `Untouched` means no
    /// diff arrived at all.
    Untouched,
}

/// Classify a `lib.rs` diff.
///
/// An empty diff is [`LibChange::Untouched`]. Any REMOVED line makes the
/// result [`LibChange::Beyond`], whatever the line is — including a
/// removed `pub mod y;` and including a removed blank line. The allowance
/// is for ADDING a declaration; deleting one is a change to what the crate
/// exposes. One added declaration beside one removed blank line is the
/// case a reader expects to be forgiven, and it is not forgiven: the
/// removed blank line is an offending entry like any other. Every
/// offending line is carried, in diff order, not just the first.
///
/// Otherwise the result is [`LibChange::DeclarationsOnly`], carrying the
/// declared module names sorted ascending and deduplicated.
///
/// # Examples
///
/// ```
/// use farmerbob_core::lib_diff::{classify, DiffLine, LibChange};
///
/// assert_eq!(classify(&[]), LibChange::Untouched);
///
/// let clean = classify(&[DiffLine {
///     text: String::from("pub mod y;"),
///     added: true,
/// }]);
/// assert!(matches!(clean, LibChange::DeclarationsOnly { .. }));
///
/// let edit = classify(&[DiffLine {
///     text: String::from("pub(crate) fn run_state_name() {}"),
///     added: true,
/// }]);
/// assert!(matches!(edit, LibChange::Beyond { .. }));
/// ```
pub fn classify(diff: &[DiffLine]) -> LibChange {
    if diff.is_empty() {
        return LibChange::Untouched;
    }

    let mut offenders: Vec<DiffLine> = Vec::new();
    let mut modules: Vec<String> = Vec::new();

    for line in diff {
        if !line.added {
            // The allowance is for adding a declaration. Any removal —
            // even of a blank line — is a change to what the crate
            // exposes, so it offends whatever its text is.
            offenders.push(line.clone());
        } else if let Some(name) = declared_module(&line.text) {
            modules.push(name.to_owned());
        } else if !is_blank(&line.text) && !is_comment_only(&line.text) {
            offenders.push(line.clone());
        }
    }

    if offenders.is_empty() {
        modules.sort();
        modules.dedup();
        LibChange::DeclarationsOnly { modules }
    } else {
        LibChange::Beyond { lines: offenders }
    }
}

/// Whether this change is within what the gate permits.
///
/// True for [`LibChange::Untouched`] and [`LibChange::DeclarationsOnly`];
/// false for [`LibChange::Beyond`].
pub fn permitted(c: &LibChange) -> bool {
    !matches!(c, LibChange::Beyond { .. })
}

/// The module name a `pub mod y;` line declares, if the line is exactly
/// that.
///
/// The match is exact because the rubric quotes the exact form: `pub mod `
/// with one space, then a plain identifier, then `;` and nothing more —
/// no leading indentation, no body, no trailing comment or code. Returns
/// `None` for anything else, including `mod y;` without `pub`,
/// `pub mod y {` with a body, and a line with trailing code. A rejected
/// shape is never silently permitted: fed to [`classify`] as an added
/// line, it reports [`LibChange::Beyond`].
///
/// # Examples
///
/// ```
/// use farmerbob_core::lib_diff::declared_module;
///
/// assert_eq!(declared_module("pub mod y;"), Some("y"));
/// assert_eq!(declared_module("mod y;"), None);
/// assert_eq!(declared_module("pub mod y {"), None);
/// assert_eq!(declared_module("pub mod y;  // note"), None);
/// assert_eq!(declared_module("pub  mod y;"), None);
/// assert_eq!(declared_module("pub mod y ;"), None);
/// assert_eq!(declared_module("pub mod ;"), None);
/// ```
pub fn declared_module(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("pub mod ")?;
    let (name, after) = rest.split_once(';')?;
    if !after.is_empty() || !is_identifier(name) {
        return None;
    }
    Some(name)
}

/// True when `name` is a plain identifier: a letter or `_` first, then
/// letters, digits, or `_`. Raw identifiers (`r#`) are not accepted.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_') && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// True when the line is empty or nothing but whitespace.
fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// True when the line's first non-whitespace content begins a `//`
/// comment, so the line carries no code. A line with code before a
/// comment — `pub mod y; // note` — does not start with `//` and is not
/// comment-only.
fn is_comment_only(text: &str) -> bool {
    text.trim_start().starts_with("//")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added(text: &str) -> DiffLine {
        DiffLine {
            text: text.to_string(),
            added: true,
        }
    }

    fn removed(text: &str) -> DiffLine {
        DiffLine {
            text: text.to_string(),
            added: false,
        }
    }

    // Clause 1: an empty diff is Untouched, and permitted.
    #[test]
    fn an_empty_diff_is_untouched_and_permitted() {
        assert_eq!(classify(&[]), LibChange::Untouched);
        assert!(permitted(&classify(&[])));
    }

    // Clause 2: only added declarations, sorted and deduplicated.
    #[test]
    fn added_declarations_are_declarations_only_sorted_and_deduped() {
        let change = classify(&[
            added("pub mod zeta;"),
            added("pub mod alpha;"),
            added("pub mod zeta;"),
        ]);
        assert_eq!(
            change,
            LibChange::DeclarationsOnly {
                modules: vec![String::from("alpha"), String::from("zeta")]
            }
        );
        assert!(permitted(&change));
    }

    // Boundary: one added declaration carries one module.
    #[test]
    fn one_added_declaration_carries_one_module() {
        assert_eq!(
            classify(&[added("pub mod lib_diff;")]),
            LibChange::DeclarationsOnly {
                modules: vec![String::from("lib_diff")]
            }
        );
    }

    // Clause 3: any removed line is Beyond, even a removed declaration.
    #[test]
    fn a_removed_declaration_is_beyond() {
        let change = classify(&[removed("pub mod y;")]);
        assert_eq!(
            change,
            LibChange::Beyond {
                lines: vec![removed("pub mod y;")]
            }
        );
        assert!(!permitted(&change));
    }

    // Clause 4: the three store-batch shapes, each Beyond carrying it.
    #[test]
    fn the_store_batch_visibility_edits_are_beyond_one_by_one() {
        let edits = [
            "pub(crate) conn: Connection,",
            "pub(crate) fn run_state_data_json(state: &RunState, conn: &Connection) -> String {",
            "pub(crate) fn run_state_name(state: &RunState) -> String {",
        ];
        for edit in edits {
            let change = classify(&[added(edit)]);
            assert_eq!(
                change,
                LibChange::Beyond {
                    lines: vec![added(edit)]
                }
            );
            assert!(!permitted(&change));
        }
    }

    // Clause 4: the whole store-batch diff — three removals, three
    // visibility widenings — is Beyond, and every line offended.
    #[test]
    fn the_store_batch_diff_is_beyond_with_all_six_lines() {
        let diff = vec![
            removed("    conn: Connection,"),
            added("    pub(crate) conn: Connection,"),
            removed("fn run_state_data_json(state: &RunState, conn: &Connection) -> String {"),
            added(
                "pub(crate) fn run_state_data_json(state: &RunState, conn: &Connection) -> String {",
            ),
            removed("fn run_state_name(state: &RunState) -> String {"),
            added("pub(crate) fn run_state_name(state: &RunState) -> String {"),
        ];
        assert_eq!(
            classify(&diff),
            LibChange::Beyond {
                lines: diff.clone()
            }
        );
    }

    // Clause 5: Beyond carries EVERY offending line, in diff order, and
    // none of the permitted lines beside them.
    #[test]
    fn beyond_carries_every_offending_line_in_diff_order() {
        let diff = vec![
            added("pub mod y;"),
            added("pub(crate) conn: Connection,"),
            added("// declare the module"),
            added("use std::collections::HashMap;"),
            removed("fn run_state_name(state: &RunState) -> String {"),
            added("pub mod y {"),
        ];
        let change = classify(&diff);
        assert_eq!(
            change,
            LibChange::Beyond {
                lines: vec![
                    diff[1].clone(),
                    diff[3].clone(),
                    diff[4].clone(),
                    diff[5].clone(),
                ]
            }
        );
        assert!(!permitted(&change));
    }

    // Clause 6: declared_module accepts exactly the rubric's form.
    #[test]
    fn declared_module_accepts_exactly_the_rubrics_form() {
        assert_eq!(declared_module("pub mod y;"), Some("y"));
        assert_eq!(declared_module("mod y;"), None);
        assert_eq!(declared_module("pub mod y {"), None);
        assert_eq!(declared_module("pub mod y;  // note"), None);
        assert_eq!(declared_module("pub  mod y;"), None);
        assert_eq!(declared_module("pub mod y ;"), None);
    }

    // Clause 6: a rejected declaration shape surfaces as Beyond, never
    // silently permitted.
    #[test]
    fn a_rejected_declaration_shape_surfaces_as_beyond() {
        let change = classify(&[added("pub mod y;  // note")]);
        assert_eq!(
            change,
            LibChange::Beyond {
                lines: vec![added("pub mod y;  // note")]
            }
        );
        assert!(!permitted(&change));
    }

    // Clause 7: blank and whitespace-only added lines are permitted and
    // never named as modules.
    #[test]
    fn blank_added_lines_are_permitted_and_never_named_modules() {
        let change = classify(&[added(""), added("   "), added("\t"), added("pub mod y;")]);
        assert_eq!(
            change,
            LibChange::DeclarationsOnly {
                modules: vec![String::from("y")]
            }
        );
        assert!(permitted(&change));
    }

    // Clause 8: a comment-only added line is permitted.
    #[test]
    fn a_comment_only_added_line_is_permitted() {
        let change = classify(&[added("// declare the module"), added("pub mod y;")]);
        assert_eq!(
            change,
            LibChange::DeclarationsOnly {
                modules: vec![String::from("y")]
            }
        );
        assert!(permitted(&change));
    }

    // Clause 8: code before a comment is not comment-only.
    #[test]
    fn code_before_a_comment_is_beyond() {
        let change = classify(&[added("pub mod y; // note")]);
        assert_eq!(
            change,
            LibChange::Beyond {
                lines: vec![added("pub mod y; // note")]
            }
        );
    }

    // A realistic clean edit: blank lines and a comment beside the
    // declaration.
    #[test]
    fn a_realistic_declaration_edit_is_clean() {
        let diff = vec![
            added(""),
            added("// wave77: the scope gate permits this file"),
            added("pub mod lib_diff;"),
            added(""),
        ];
        let change = classify(&diff);
        assert_eq!(
            change,
            LibChange::DeclarationsOnly {
                modules: vec![String::from("lib_diff")]
            }
        );
        assert!(permitted(&change));
    }
}

#[cfg(test)]
mod boundaries {
    use super::*;

    fn added(text: &str) -> DiffLine {
        DiffLine {
            text: text.to_string(),
            added: true,
        }
    }

    fn removed(text: &str) -> DiffLine {
        DiffLine {
            text: text.to_string(),
            added: false,
        }
    }

    // One added declaration and one removed blank line: Beyond, and the
    // removed blank line is the offending entry. This is the case a reader
    // expects to be forgiven; clause 3 does not forgive it.
    #[test]
    fn a_removed_blank_line_is_beyond_and_is_the_offending_entry() {
        let change = classify(&[added("pub mod y;"), removed("")]);
        assert_eq!(
            change,
            LibChange::Beyond {
                lines: vec![removed("")]
            }
        );
        assert!(!permitted(&change));
    }

    // A diff of only blank added lines is DeclarationsOnly with an EMPTY
    // modules vector, not Untouched: lines were added to lib.rs, so the
    // file was touched; nothing declarable came of it.
    #[test]
    fn a_blank_only_diff_is_declarations_only_with_no_modules() {
        let change = classify(&[added(""), added("   ")]);
        assert_eq!(
            change,
            LibChange::DeclarationsOnly {
                modules: Vec::new()
            }
        );
        assert!(permitted(&change));
        assert_ne!(change, LibChange::Untouched);
    }

    // The SAME module declared twice in one diff is carried once.
    #[test]
    fn the_same_module_declared_twice_is_carried_once() {
        let change = classify(&[added("pub mod y;"), added("pub mod y;")]);
        assert_eq!(
            change,
            LibChange::DeclarationsOnly {
                modules: vec![String::from("y")]
            }
        );
    }

    // `pub mod ;` names no module: not a declaration, so Beyond.
    #[test]
    fn an_empty_module_name_is_not_a_declaration() {
        assert_eq!(declared_module("pub mod ;"), None);
        let change = classify(&[added("pub mod ;")]);
        assert_eq!(
            change,
            LibChange::Beyond {
                lines: vec![added("pub mod ;")]
            }
        );
    }

    // A removal is Beyond even when a declaration is added in the same
    // diff; the added declaration is permitted and so is not carried.
    #[test]
    fn an_added_declaration_does_not_absorb_a_removal() {
        let change = classify(&[added("pub mod z;"), removed("pub mod y;")]);
        assert_eq!(
            change,
            LibChange::Beyond {
                lines: vec![removed("pub mod y;")]
            }
        );
    }
}

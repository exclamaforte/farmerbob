//! Precondition checking for queued task specs.
//!
//! A spec declares the paths it will touch in HTML comments in its prompt. A
//! declaration is a whole line of the form
//!
//! ```text
//! <!-- fb:creates crates/thing/src/new.rs -->
//! <!-- fb:modifies crates/thing/src/quota.rs -->
//! ```
//!
//! with optional leading whitespace. Specs are written days before they run
//! and sit in a queue while other waves merge; a merge moves the base under
//! everything still queued, so each declaration is a requirement on which
//! side of the base a path must be at launch time.
//!
//! Three rules keep prose *about* the syntax from being mistaken for a use
//! of it. The line-matching shape that these rules guard has already
//! produced three separate bugs in this project by matching a pattern
//! inside a quotation of itself:
//!
//! - The comment must start the line, after optional leading whitespace. A
//!   marker written mid-sentence, after other words on the same line, is
//!   discussed syntax, not a declaration.
//! - A marker inside a fenced code block is documentation of the syntax,
//!   not a use of it. A fence is a line whose trimmed text starts with
//!   three or more backticks; fences toggle, so a marker after the closing
//!   fence is recognised again.
//! - The marker word is one of a closed set of exactly two: `fb:creates`
//!   and `fb:modifies`. Any other word after the comment opener and `fb:`
//!   -- a third marker introduced by a newer prompt, say -- is ignored
//!   rather than refused, so an older binary meeting a newer prompt still
//!   runs it; and it is never silently treated as one of the two known
//!   markers.
//!
//! The path is the text between the marker word and the closing `-->`,
//! trimmed. A marker whose path is empty after trimming declares nothing
//! and is not an error. [`check`] takes the base as the set of paths that
//! exist on it, so a base that does not exist yet -- the normal situation
//! for a queued spec -- is as testable as one that does.

/// Which side of the base a declared path must be on.
///
/// Closed: exactly these two variants. A future marker word expands this
/// enum only when the scanner is taught to refuse prompts that use it;
/// until then unknown words are ignored (see [`declarations`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// `fb:creates` -- the path must NOT exist on the base.
    Absent,
    /// `fb:modifies` -- the path MUST exist on the base.
    Present,
}

/// One declared path and what the spec requires of it.
///
/// Named `Declared` here as the spec fixes; the pre-existing
/// [`crate::scope::Declared`] is a different concept (the single
/// deliverable target of a run, with no requirement attached).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// Repo-relative path, exactly as written in the prompt.
    pub path: String,
    /// What the marker requires.
    pub requirement: Requirement,
}

/// Whether a queued spec can still run against a base.
///
/// Closed: exactly these three variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// Every declaration holds.
    Satisfied,
    /// At least one declaration does not hold. Every violation, in
    /// declaration order, duplicates preserved -- a spec that is wrong in
    /// two ways says so once. Never empty: [`check`] never returns
    /// `Violated(vec![])`; an empty violation list is `Satisfied`.
    Violated(Vec<Violation>),
    /// The prompt declares nothing, so nothing can be checked. NOT
    /// [`Precondition::Satisfied`]: an unchecked spec and a checked one are
    /// different states, and collapsing them would make a check whose
    /// failure is indistinguishable from its success.
    Undeclared,
}

/// One declaration that does not hold against the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The declared path.
    pub path: String,
    /// What the spec required.
    pub required: Requirement,
    /// Whether the path is actually on the base.
    pub exists: bool,
}

/// Every declaration in a prompt, in the order they appear.
///
/// The result is a transcript of the prompt, not a set: document order, with
/// duplicates preserved. A prompt that declares the same path twice is
/// reported as what it literally says, twice.
pub fn declarations(prompt: &str) -> Vec<Declared> {
    let mut declared = Vec::new();
    let mut fenced = false;
    for line in prompt.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some(d) = parse_declaration(trimmed) {
            declared.push(d);
        }
    }
    declared
}

/// Parse one already-trimmed line into a [`Declared`], or `None` if the
/// line is not a declaration.
fn parse_declaration(line: &str) -> Option<Declared> {
    let rest = line.strip_prefix("<!--")?.trim_start();
    let (requirement, rest) = match rest.strip_prefix("fb:creates") {
        Some(rest) => (Requirement::Absent, rest),
        None => {
            let modifies = rest.strip_prefix("fb:modifies")?;
            (Requirement::Present, modifies)
        }
    };
    // The marker word ends at the whitespace that separates it from the
    // path. A word that merely starts with a marker (`fb:createsx`) is a
    // different word, not a marker.
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let body = rest.strip_suffix("-->")?.trim();
    if body.is_empty() {
        // A marker with no path declares nothing and is not an error.
        return None;
    }
    Some(Declared {
        path: body.to_string(),
        requirement,
    })
}

/// Check a prompt's declarations against the paths present on the base.
///
/// `present` is the set of repo-relative paths that exist. Membership is an
/// EXACT string match: no normalisation, no prefix matching, no case
/// folding. Two spellings of one file are two different paths.
///
/// A prompt that declares nothing cannot be checked, so the answer is
/// [`Precondition::Undeclared`]. Otherwise every declaration is evaluated,
/// and the answer is [`Precondition::Satisfied`] only when all of them
/// hold. [`Precondition::Violated`] carries every failing declaration in
/// declaration order and is never empty.
pub fn check(prompt: &str, present: &[&str]) -> Precondition {
    let declared = declarations(prompt);
    if declared.is_empty() {
        return Precondition::Undeclared;
    }
    let violations: Vec<Violation> = declared
        .iter()
        .filter_map(|d| {
            let exists = present.contains(&d.path.as_str());
            let holds = match d.requirement {
                Requirement::Absent => !exists,
                Requirement::Present => exists,
            };
            if holds {
                None
            } else {
                Some(Violation {
                    path: d.path.clone(),
                    required: d.requirement,
                    exists,
                })
            }
        })
        .collect();
    if violations.is_empty() {
        Precondition::Satisfied
    } else {
        Precondition::Violated(violations)
    }
}

#[cfg(test)]
mod tests {
    use super::{Declared, Precondition, Requirement, Violation, check, declarations};

    fn absent(path: &str) -> Declared {
        Declared {
            path: path.to_string(),
            requirement: Requirement::Absent,
        }
    }

    fn present(path: &str) -> Declared {
        Declared {
            path: path.to_string(),
            requirement: Requirement::Present,
        }
    }

    fn violated(path: &str, required: Requirement, exists: bool) -> Violation {
        Violation {
            path: path.to_string(),
            required,
            exists,
        }
    }

    #[test]
    fn creates_marker_declares_absent() {
        assert_eq!(
            declarations("<!-- fb:creates a/b.rs -->"),
            vec![absent("a/b.rs")],
        );
    }

    #[test]
    fn modifies_marker_declares_present() {
        assert_eq!(
            declarations("<!-- fb:modifies a/b.rs -->"),
            vec![present("a/b.rs")],
        );
    }

    #[test]
    fn declarations_keep_text_order_not_requirement_order() {
        let prompt = [
            "<!-- fb:modifies exists.rs -->",
            "some prose between declarations",
            "<!-- fb:creates fresh.rs -->",
        ]
        .join("\n");
        assert_eq!(
            declarations(&prompt),
            vec![present("exists.rs"), absent("fresh.rs")],
        );
    }

    #[test]
    fn creates_with_path_on_base_is_violated() {
        assert_eq!(
            check("<!-- fb:creates a/b.rs -->", &["a/b.rs"]),
            Precondition::Violated(vec![violated("a/b.rs", Requirement::Absent, true)]),
        );
    }

    #[test]
    fn modifies_with_path_missing_from_base_is_violated() {
        assert_eq!(
            check("<!-- fb:modifies a/b.rs -->", &[]),
            Precondition::Violated(vec![violated("a/b.rs", Requirement::Present, false)]),
        );
    }

    #[test]
    fn markerless_prompt_is_undeclared_not_satisfied() {
        let prompt = "a prompt that never declares anything";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(check(prompt, &["a/b.rs"]), Precondition::Undeclared);
    }

    #[test]
    fn marker_mid_sentence_declares_nothing() {
        let prompt = "the rubric says fb:creates x.rs means the path must not exist";
        assert_eq!(declarations(prompt), Vec::new());
        // The mentioned path exists; if it were parsed as a declaration this
        // would be Violated rather than Undeclared.
        assert_eq!(check(prompt, &["x.rs"]), Precondition::Undeclared);
    }

    #[test]
    fn marker_in_fence_ignored_but_recognised_after_close() {
        let prompt = [
            "<!-- fb:creates before.rs -->",
            "```",
            "<!-- fb:creates in/fence.rs -->",
            "```",
            "<!-- fb:creates after/fence.rs -->",
        ]
        .join("\n");
        assert_eq!(
            declarations(&prompt),
            vec![absent("before.rs"), absent("after/fence.rs")],
        );
        assert_eq!(
            check(&prompt, &["in/fence.rs", "after/fence.rs"]),
            Precondition::Violated(vec![violated("after/fence.rs", Requirement::Absent, true)]),
        );
    }

    #[test]
    fn fences_toggle_repeatedly() {
        let prompt = [
            "```",
            "<!-- fb:creates one.rs -->",
            "```",
            "```",
            "<!-- fb:creates two.rs -->",
            "```",
            "<!-- fb:creates three.rs -->",
        ]
        .join("\n");
        assert_eq!(declarations(&prompt), vec![absent("three.rs")]);
    }

    #[test]
    fn fence_recognised_after_leading_whitespace() {
        let prompt = [
            "   ```",
            "<!-- fb:creates in/fence.rs -->",
            "```",
            "<!-- fb:creates out.rs -->",
        ]
        .join("\n");
        assert_eq!(declarations(&prompt), vec![absent("out.rs")]);
    }

    #[test]
    fn four_backticks_open_a_fence() {
        let prompt = [
            "````",
            "<!-- fb:creates in/fence.rs -->",
            "````",
            "<!-- fb:creates out.rs -->",
        ]
        .join("\n");
        assert_eq!(declarations(&prompt), vec![absent("out.rs")]);
    }

    #[test]
    fn marker_without_path_declares_nothing_not_an_error() {
        let prompt = [
            "<!-- fb:creates -->",
            "<!-- fb:creates   -->",
            "<!-- fb:modifies -->",
        ]
        .join("\n");
        assert_eq!(declarations(&prompt), Vec::new());
        assert_eq!(check(&prompt, &[]), Precondition::Undeclared);
    }

    #[test]
    fn path_is_trimmed_of_surrounding_whitespace() {
        assert_eq!(
            declarations("<!-- fb:creates   a/b.rs  -->"),
            vec![absent("a/b.rs")]
        );
    }

    #[test]
    fn every_violation_reported_in_declaration_order() {
        let prompt = [
            "<!-- fb:creates taken.rs -->",
            "<!-- fb:modifies missing.rs -->",
            "<!-- fb:creates also-taken.rs -->",
            "<!-- fb:modifies present.rs -->",
        ]
        .join("\n");
        let base = ["taken.rs", "also-taken.rs", "present.rs"];
        assert_eq!(
            check(&prompt, &base),
            Precondition::Violated(vec![
                violated("taken.rs", Requirement::Absent, true),
                violated("missing.rs", Requirement::Present, false),
                violated("also-taken.rs", Requirement::Absent, true),
            ]),
        );
    }

    #[test]
    fn empty_prompt_is_undeclared() {
        assert_eq!(declarations(""), Vec::new());
        assert_eq!(check("", &["a.rs"]), Precondition::Undeclared);
    }

    #[test]
    fn empty_base_satisfies_creates() {
        assert_eq!(
            check("<!-- fb:creates a.rs -->", &[]),
            Precondition::Satisfied
        );
    }

    #[test]
    fn empty_base_violates_modifies() {
        assert_eq!(
            check("<!-- fb:modifies a.rs -->", &[]),
            Precondition::Violated(vec![violated("a.rs", Requirement::Present, false)]),
        );
    }

    #[test]
    fn duplicate_declarations_transcribed_and_violated_twice() {
        let prompt = ["<!-- fb:creates dup.rs -->", "<!-- fb:creates dup.rs -->"].join("\n");
        assert_eq!(
            declarations(&prompt),
            vec![absent("dup.rs"), absent("dup.rs")]
        );
        assert_eq!(
            check(&prompt, &["dup.rs"]),
            Precondition::Violated(vec![
                violated("dup.rs", Requirement::Absent, true),
                violated("dup.rs", Requirement::Absent, true),
            ]),
        );
    }

    #[test]
    fn contradictory_markers_violated_against_every_base() {
        let prompt = ["<!-- fb:creates x.rs -->", "<!-- fb:modifies x.rs -->"].join("\n");
        assert_eq!(declarations(&prompt), vec![absent("x.rs"), present("x.rs")]);
        for base in [&[] as &[&str], &["x.rs"], &["other.rs", "x.rs"]] {
            match check(&prompt, base) {
                Precondition::Violated(v) => assert_eq!(v.len(), 1, "base {base:?}"),
                other => panic!("expected Violated against base {base:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_marker_word_ignored_not_refused() {
        let prompt = "<!-- fb:deletes gone.rs -->";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(check(prompt, &[]), Precondition::Undeclared);
    }

    #[test]
    fn marker_word_not_matched_by_prefix_of_longer_word() {
        let prompt = ["<!-- fb:createsx -->", "<!-- fb:modifiesy -->"].join("\n");
        assert_eq!(declarations(&prompt), Vec::new());
    }

    #[test]
    fn leading_whitespace_before_comment_allowed() {
        assert_eq!(
            declarations("  <!-- fb:creates a.rs -->"),
            vec![absent("a.rs")]
        );
        assert_eq!(
            declarations("\t<!-- fb:modifies a.rs -->"),
            vec![present("a.rs")]
        );
    }

    #[test]
    fn membership_is_exact_string_match() {
        let prompt = "<!-- fb:modifies a/b.rs -->";
        // A parent directory, a suffix, a case difference and an extension
        // of the name are all different paths, none of them the declared
        // one.
        let near_misses = ["a", "a/b.rs.orig", "A/b.rs", "a/b.rsx"];
        assert_eq!(
            check(prompt, &near_misses),
            Precondition::Violated(vec![violated("a/b.rs", Requirement::Present, false)]),
        );
    }

    #[test]
    fn all_holding_prompt_is_satisfied() {
        let prompt = ["<!-- fb:creates new.rs -->", "<!-- fb:modifies old.rs -->"].join("\n");
        assert_eq!(
            check(&prompt, &["old.rs", "unrelated.rs"]),
            Precondition::Satisfied
        );
    }
}

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
//! # The Grammar
//!
//! A line declares if and only if, after removing leading whitespace, it
//! matches exactly:
//!
//! ```text
//! OPEN , one or more spaces , "fb:" , WORD , one or more spaces , PATH , one or more spaces , CLOSE
//! ```
//!
//! and nothing follows `CLOSE` except whitespace.
//!
//! - `OPEN` is `<!--` and `CLOSE` is `-->`.
//! - `WORD` is `creates` or `modifies` and nothing else.
//! - `PATH` is one or more characters containing no whitespace and not containing `CLOSE`.
//! - Every `one or more spaces` is spaces only (`' '`). A tab is not a space here.
//!
//! # Refusal and Silence
//!
//! A line that looks like a declaration but violates the grammar is refused
//! and reported by [`rejections`]. Five rules define refusals:
//!
//! - [`Rejected::Tab`]: A tab appeared where the grammar requires spaces (after
//!   `OPEN`, after `fb:WORD`, or before `CLOSE`).
//! - [`Rejected::TrailingText`]: Text other than whitespace followed `CLOSE`.
//! - [`Rejected::UnknownWord`]: The marker word was neither `creates` nor `modifies`.
//! - [`Rejected::NoSpaceAfterOpener`]: No whitespace between `OPEN` and `fb:`.
//! - [`Rejected::MultipleMarkers`]: A line carries more than one marker. The
//!   path is delimited by `CLOSE`, so on a two-marker line every reading of
//!   "the path" is a guess, and a guessed path is exactly what this module
//!   exists to refuse.
//!
//! When a line exhibits multiple independent faults, the first reason in
//! [`Rejected`]'s declaration order is reported.
//!
//! A line with no `OPEN` at all, or prose that mentions a marker mid-sentence,
//! or text inside a fenced code block (delimited by three or more backticks
//! or tildes), is ignored: silence is for text that never looked
//! like a live declaration.

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

/// Marker words this module does not answer for, and must not refuse.
///
/// Each is read by another part of the harness, and each was checked: `fb:reads` by
/// `dispatch_cmd::reads_of` (dispatch_cmd.rs:85), `fb:case` and `fb:differential` by
/// `differential` (differential.rs:96 and :86).
///
/// `fb:deletes` is deliberately NOT here. It has no reader anywhere in this tree -- every
/// occurrence is this module's own test fixture for an unknown word -- so it stays
/// `UnknownWord`, which is what a typo should be.
///
/// A SUPERSET, never a closed world -- a family added here without being added there, or
/// the reverse, is how `fb:reads` came to make fifteen specs undispatchable.
/// (bead farmerbob-2ve)
pub const OTHER_MARKER_FAMILIES: [&str; 3] = ["reads", "case", "differential"];

/// Whether `word` is one of the two deliverable verbs this module answers for.
fn is_deliverable_word(word: &str) -> bool {
    word == "creates" || word == "modifies"
}

/// Why a line that looks like a declaration is not one.
///
/// Reported so a spec author sees the difference between "you wrote no marker" and
/// "you wrote a marker I refused", which are otherwise indistinguishable from
/// [`Precondition::Undeclared`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]

pub enum Rejected {
    /// A tab appeared where the grammar requires spaces.
    Tab,
    /// Text other than whitespace followed the closing delimiter.
    TrailingText,
    /// The marker word was neither `creates` nor `modifies`.
    UnknownWord,
    /// No whitespace between the comment opener and `fb:`.
    NoSpaceAfterOpener,
    /// The line carries more than one marker.
    ///
    /// The path is delimited by the closing delimiter, so on a two-marker
    /// line every reading of "the path" is a guess, and a guessed path is
    /// exactly what this module exists to refuse.
    MultipleMarkers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fence {
    Backtick,
    Tilde,
}

enum LineClassification {
    Declared(Declared),
    Rejected(Rejected),
    Ignored,
}

struct Comment<'a> {
    raw: &'a str,
    open_idx: usize,
    close_idx: usize,
    is_marker: bool,
    word: String,
    no_space_after_opener: bool,
    contains_tab: bool,
}

/// Parses an already-trimmed-at-start line if it matches the declaration grammar exactly.
fn try_parse_declaration(trimmed_start: &str) -> Option<Declared> {
    let rest = trimmed_start.strip_prefix("<!--")?;

    // 1. One or more spaces after OPEN (SPACES ONLY, ' ')
    let spaces1 = rest.bytes().take_while(|&b| b == b' ').count();
    if spaces1 == 0 {
        return None;
    }
    let rest = &rest[spaces1..];

    // 2. "fb:"
    let rest = rest.strip_prefix("fb:")?;

    // 3. WORD: "creates" or "modifies"
    let (requirement, rest) = if let Some(r) = rest.strip_prefix("creates") {
        (Requirement::Absent, r)
    } else {
        let r = rest.strip_prefix("modifies")?;
        (Requirement::Present, r)
    };

    // 4. One or more spaces after WORD (SPACES ONLY, ' ')
    let spaces2 = rest.bytes().take_while(|&b| b == b' ').count();
    if spaces2 == 0 {
        return None;
    }
    let rest = &rest[spaces2..];

    // 5. PATH: one or more characters containing no whitespace and not containing "-->"
    let path_byte_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
    if path_byte_len == 0 {
        return None;
    }
    let path = &rest[..path_byte_len];
    if path.contains("-->") {
        return None;
    }
    let rest = &rest[path_byte_len..];

    // 6. One or more spaces before CLOSE (SPACES ONLY, ' ')
    let spaces3 = rest.bytes().take_while(|&b| b == b' ').count();
    if spaces3 == 0 {
        return None;
    }
    let rest = &rest[spaces3..];

    // 7. CLOSE ("-->")
    let rest = rest.strip_prefix("-->")?;

    // 8. Nothing follows CLOSE except whitespace
    if !rest.chars().all(|c| c.is_whitespace()) {
        return None;
    }

    Some(Declared {
        path: path.to_string(),
        requirement,
    })
}

/// Scans all HTML comments starting with `<!--` on `trimmed_start`.
fn scan_comments(trimmed_start: &str) -> Vec<Comment<'_>> {
    let mut comments = Vec::new();
    let mut pos = 0;
    while pos < trimmed_start.len() {
        let Some(open_rel) = trimmed_start[pos..].find("<!--") else {
            break;
        };
        let open_idx = pos + open_rel;
        let after_open = open_idx + 4;
        let (close_idx, raw) = match trimmed_start[after_open..].find("-->") {
            Some(close_rel) => {
                let close_end = after_open + close_rel + 3;
                (close_end, &trimmed_start[open_idx..close_end])
            }
            None => (trimmed_start.len(), &trimmed_start[open_idx..]),
        };

        let is_marker = raw.contains("fb:");
        let no_space_after_opener = raw
            .strip_prefix("<!--")
            .is_some_and(|r| r.starts_with("fb:"));
        let contains_tab = raw.contains('\t');

        let mut word = String::new();
        if let Some(fb_pos) = raw.find("fb:") {
            let after_fb = &raw[fb_pos + 3..];
            for (i, c) in after_fb.char_indices() {
                if c.is_whitespace() || after_fb[i..].starts_with("-->") {
                    break;
                }
                word.push(c);
            }
        }

        comments.push(Comment {
            raw,
            open_idx,
            close_idx,
            is_marker,
            word,
            no_space_after_opener,
            contains_tab,
        });

        pos = close_idx;
    }
    comments
}

/// Determines the rejection reason for a line that started with `<!--` and contained `fb:`,
/// returning the first reason in [`Rejected`]'s declaration order.
fn determine_rejection(trimmed_start: &str) -> Option<Rejected> {
    let comments = scan_comments(trimmed_start);
    let markers: Vec<&Comment<'_>> = comments.iter().filter(|c| c.is_marker).collect();
    if markers.is_empty() {
        return None;
    }

    let marker_count: usize = comments
        .iter()
        .map(|c| c.raw.match_indices("fb:").count())
        .sum();

    // 1. Tab: A tab appeared where the grammar requires spaces.
    let has_tab = markers.iter().any(|m| m.contains_tab);
    if has_tab {
        return Some(Rejected::Tab);
    }

    // 2. TrailingText: Text other than whitespace followed the closing delimiter.
    let mut has_trailing_text = false;
    for i in 0..markers.len().saturating_sub(1) {
        let between = &trimmed_start[markers[i].close_idx..markers[i + 1].open_idx];
        if !between.chars().all(|c| c.is_whitespace()) {
            has_trailing_text = true;
            break;
        }
    }
    if !has_trailing_text {
        let last_marker = markers[markers.len() - 1];
        let after_last = &trimmed_start[last_marker.close_idx..];
        if !after_last.chars().all(|c| c.is_whitespace()) {
            has_trailing_text = true;
        }
    }
    if has_trailing_text {
        return Some(Rejected::TrailingText);
    }

    // 3. UnknownWord: the marker word names no family this project uses.
    //
    // A WORD FROM ANOTHER FAMILY IS NOT A MALFORMED DELIVERABLE MARKER. This refused every
    // `fb:` word that was not `creates` or `modifies`, and the harness emits several others:
    // `fb:reads` (28 uses, consumed by `dispatch_cmd::reads_of`), `fb:case` and
    // `fb:differential` (19 and 12, consumed by `differential`), `fb:deletes` (9).
    //
    // So a spec carrying `<!-- fb:reads sources.toml -->` was REFUSED outright, and because
    // the refusal is silent it simply could not be dispatched, pipelined, or have its fate
    // decided, with nothing saying why. Fifteen specs in `.fb/prompts` carry `fb:reads`;
    // five were checked and all five were silently refused.  (bead farmerbob-9mh, t8a5)
    //
    // This module answers ONE question -- what does this spec declare as its deliverable --
    // and a marker addressed to a different reader is not its business. It is skipped, not
    // refused. A word belonging to NO family is still `UnknownWord`, because that is a typo
    // and `fb:createsx` appears once in this tree.
    let has_unknown_word = markers.iter().any(|m| {
        !is_deliverable_word(&m.word) && !OTHER_MARKER_FAMILIES.contains(&m.word.as_str())
    });
    if has_unknown_word {
        return Some(Rejected::UnknownWord);
    }

    // 4. NoSpaceAfterOpener: No whitespace between the comment opener and `fb:`.
    let has_no_space_after_opener = markers.iter().any(|m| m.no_space_after_opener);
    if has_no_space_after_opener {
        return Some(Rejected::NoSpaceAfterOpener);
    }

    // 5. MultipleMarkers: The line carries more than one marker.
    if marker_count > 1 {
        return Some(Rejected::MultipleMarkers);
    }

    None
}

/// Classifies a non-fenced line into a declaration, a rejection, or silence.
fn classify_line(line: &str) -> LineClassification {
    let trimmed_start = line.trim_start();
    if !trimmed_start.starts_with("<!--") {
        return LineClassification::Ignored;
    }
    if !trimmed_start.contains("fb:") {
        return LineClassification::Ignored;
    }

    if let Some(decl) = try_parse_declaration(trimmed_start) {
        return LineClassification::Declared(decl);
    }

    if let Some(rejected) = determine_rejection(trimmed_start) {
        return LineClassification::Rejected(rejected);
    }

    LineClassification::Ignored
}

/// Iterates over non-fenced lines in `prompt`, tracking fence blocks and 1-based line numbers.
fn classified_lines(prompt: &str) -> impl Iterator<Item = (u32, LineClassification)> + '_ {
    let mut current_fence = None;
    prompt.lines().enumerate().filter_map(move |(idx, line)| {
        let trimmed = line.trim();
        match current_fence {
            None => {
                if trimmed.starts_with("```") {
                    current_fence = Some(Fence::Backtick);
                    return None;
                } else if trimmed.starts_with("~~~") {
                    current_fence = Some(Fence::Tilde);
                    return None;
                }
            }
            Some(Fence::Backtick) => {
                if trimmed.starts_with("```") {
                    current_fence = None;
                }
                return None;
            }
            Some(Fence::Tilde) => {
                if trimmed.starts_with("~~~") {
                    current_fence = None;
                }
                return None;
            }
        }
        let line_number = (idx + 1) as u32;
        Some((line_number, classify_line(line)))
    })
}

/// Every declaration in a prompt, in the order they appear.
///
/// The result is a transcript of the prompt, not a set: document order, with
/// duplicates preserved. A prompt that declares the same path twice is
/// reported as what it literally says, twice.
pub fn declarations(prompt: &str) -> Vec<Declared> {
    classified_lines(prompt)
        .filter_map(|(_, class)| match class {
            LineClassification::Declared(d) => Some(d),
            _ => None,
        })
        .collect()
}

/// Lines that look like declarations and were refused, with the line number
/// (1-based) and the reason. In line order.
///
/// A line with two independent faults reports the first reason in the order
/// [`Rejected`] declares.
pub fn rejections(prompt: &str) -> Vec<(u32, Rejected)> {
    classified_lines(prompt)
        .filter_map(|(line_num, class)| match class {
            LineClassification::Rejected(r) => Some((line_num, r)),
            _ => None,
        })
        .collect()
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
    use super::{
        Declared, Precondition, Rejected, Requirement, Violation, check, declarations, rejections,
    };

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
        assert_eq!(rejections("<!-- fb:creates a/b.rs -->"), Vec::new());
    }

    #[test]
    fn modifies_marker_declares_present() {
        assert_eq!(
            declarations("<!-- fb:modifies a/b.rs -->"),
            vec![present("a/b.rs")],
        );
        assert_eq!(rejections("<!-- fb:modifies a/b.rs -->"), Vec::new());
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
        assert_eq!(rejections(prompt), Vec::new());
        assert_eq!(check(prompt, &["a/b.rs"]), Precondition::Undeclared);
    }

    #[test]
    fn marker_mid_sentence_declares_nothing() {
        let prompt = "the rubric says fb:creates x.rs means the path must not exist";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), Vec::new());
        assert_eq!(check(prompt, &["x.rs"]), Precondition::Undeclared);
    }

    #[test]
    fn marker_mid_sentence_with_comment_brackets_declares_nothing() {
        let prompt = "the rubric says <!-- fb:creates x.rs --> means something";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), Vec::new());
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
        assert_eq!(rejections(&prompt), Vec::new());
        assert_eq!(
            check(&prompt, &["in/fence.rs", "after/fence.rs"]),
            Precondition::Violated(vec![violated("after/fence.rs", Requirement::Absent, true)]),
        );
    }

    #[test]
    fn rejected_looking_marker_in_fence_is_not_reported() {
        let prompt = [
            "```",
            "<!-- fb:creates bad.rs --> trailing text",
            "<!--\tfb:creates tab.rs -->",
            "<!--fb:creates tight.rs-->",
            "```",
            "<!-- fb:creates good.rs -->",
        ]
        .join("\n");
        assert_eq!(declarations(&prompt), vec![absent("good.rs")]);
        assert_eq!(rejections(&prompt), Vec::new());
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
        assert_eq!(rejections(&prompt), Vec::new());
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
    fn tilde_fence_opens_and_closes() {
        let prompt = [
            "<!-- fb:creates before.rs -->",
            "~~~",
            "<!-- fb:creates inside.rs -->",
            "<!-- fb:creates bad.rs --> trailing",
            "~~~",
            "<!-- fb:creates after.rs -->",
        ]
        .join("\n");
        assert_eq!(
            declarations(&prompt),
            vec![absent("before.rs"), absent("after.rs")]
        );
        assert_eq!(rejections(&prompt), Vec::new());
    }

    #[test]
    fn backtick_fence_closed_only_by_backticks() {
        let prompt = [
            "```",
            "~~~",
            "<!-- fb:creates inside.rs -->",
            "~~~",
            "<!-- fb:creates still_inside.rs -->",
            "```",
            "<!-- fb:creates after.rs -->",
        ]
        .join("\n");
        assert_eq!(declarations(&prompt), vec![absent("after.rs")]);
        assert_eq!(rejections(&prompt), Vec::new());
    }

    #[test]
    fn tilde_fence_closed_only_by_tildes() {
        let prompt = [
            "~~~",
            "```",
            "<!-- fb:creates inside.rs -->",
            "```",
            "<!-- fb:creates still_inside.rs -->",
            "~~~",
            "<!-- fb:creates after.rs -->",
        ]
        .join("\n");
        assert_eq!(declarations(&prompt), vec![absent("after.rs")]);
        assert_eq!(rejections(&prompt), Vec::new());
    }

    #[test]
    fn fence_opened_and_never_closed_fences_to_eof() {
        let prompt = [
            "<!-- fb:creates before.rs -->",
            "```",
            "<!-- fb:creates inside1.rs -->",
            "<!-- fb:creates inside2.rs -->",
        ]
        .join("\n");
        assert_eq!(declarations(&prompt), vec![absent("before.rs")]);
        assert_eq!(rejections(&prompt), Vec::new());
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
        assert_eq!(rejections(&prompt), Vec::new());
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
    fn exactly_one_space_minimum_and_multiple_spaces_valid() {
        // One space minimum in all three positions
        let prompt1 = "<!-- fb:creates one.rs -->";
        assert_eq!(declarations(prompt1), vec![absent("one.rs")]);
        assert_eq!(rejections(prompt1), Vec::new());

        // Two or more spaces in all three positions
        let prompt2 = "<!--  fb:creates   two.rs    -->";
        assert_eq!(declarations(prompt2), vec![absent("two.rs")]);
        assert_eq!(rejections(prompt2), Vec::new());
    }

    #[test]
    fn trailing_whitespace_declares_normally() {
        let prompt = "<!-- fb:creates normal.rs -->   \t  ";
        assert_eq!(declarations(prompt), vec![absent("normal.rs")]);
        assert_eq!(rejections(prompt), Vec::new());
    }

    #[test]
    fn trailing_text_after_close_declares_nothing_and_rejects() {
        let prompt = "<!-- fb:creates foo.rs --> trailing text";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), vec![(1, Rejected::TrailingText)]);
    }

    #[test]
    fn tab_after_opener_rejects() {
        let prompt = "<!--\tfb:creates foo.rs -->";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), vec![(1, Rejected::Tab)]);

        let prompt_multi = "<!-- \t fb:creates foo.rs -->";
        assert_eq!(declarations(prompt_multi), Vec::new());
        assert_eq!(rejections(prompt_multi), vec![(1, Rejected::Tab)]);
    }

    #[test]
    fn tab_after_word_rejects() {
        let prompt = "<!-- fb:creates\tfoo.rs -->";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), vec![(1, Rejected::Tab)]);

        let prompt_multi = "<!-- fb:creates \t foo.rs -->";
        assert_eq!(declarations(prompt_multi), Vec::new());
        assert_eq!(rejections(prompt_multi), vec![(1, Rejected::Tab)]);
    }

    #[test]
    fn tab_before_close_rejects() {
        let prompt = "<!-- fb:creates foo.rs\t-->";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), vec![(1, Rejected::Tab)]);

        let prompt_multi = "<!-- fb:creates foo.rs \t -->";
        assert_eq!(declarations(prompt_multi), Vec::new());
        assert_eq!(rejections(prompt_multi), vec![(1, Rejected::Tab)]);
    }

    #[test]
    fn no_space_after_opener_rejects() {
        let prompt1 = "<!--fb:creates foo.rs -->";
        assert_eq!(declarations(prompt1), Vec::new());
        assert_eq!(rejections(prompt1), vec![(1, Rejected::NoSpaceAfterOpener)]);

        let prompt2 = "<!--fb:modifies foo.rs -->";
        assert_eq!(declarations(prompt2), Vec::new());
        assert_eq!(rejections(prompt2), vec![(1, Rejected::NoSpaceAfterOpener)]);

        let prompt3 = "<!--fb:creates foo.rs-->";
        assert_eq!(declarations(prompt3), Vec::new());
        assert_eq!(rejections(prompt3), vec![(1, Rejected::NoSpaceAfterOpener)]);
    }

    #[test]
    fn two_markers_on_one_line_declare_nothing_and_reject_multiple_markers() {
        let prompt = "<!-- fb:creates a.rs --> <!-- fb:creates b.rs -->";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), vec![(1, Rejected::MultipleMarkers)]);

        let prompt_tight = "<!-- fb:creates a.rs --><!-- fb:creates b.rs -->";
        assert_eq!(declarations(prompt_tight), Vec::new());
        assert_eq!(
            rejections(prompt_tight),
            vec![(1, Rejected::MultipleMarkers)]
        );
    }

    #[test]
    fn three_markers_on_one_line_reports_multiple_markers_once() {
        let prompt = "<!-- fb:creates a.rs --> <!-- fb:creates b.rs --> <!-- fb:creates c.rs -->";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), vec![(1, Rejected::MultipleMarkers)]);
    }

    #[test]
    fn unknown_marker_word_declares_nothing_and_rejects() {
        let prompt = "<!-- fb:deletes gone.rs -->";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), vec![(1, Rejected::UnknownWord)]);
        assert_eq!(check(prompt, &[]), Precondition::Undeclared);
    }

    #[test]
    fn marker_word_longer_variant_rejects_unknown_word() {
        let prompt = "<!-- fb:createsx gone.rs -->";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), vec![(1, Rejected::UnknownWord)]);
    }

    #[test]
    fn tie_breaking_order_among_rejections() {
        // Tab (1) beats TrailingText (2)
        let p_tab_trailing = "<!--\tfb:creates a.rs --> prose";
        assert_eq!(rejections(p_tab_trailing), vec![(1, Rejected::Tab)]);

        // Tab (1) beats UnknownWord (3)
        let p_tab_unknown = "<!--\tfb:unknown a.rs -->";
        assert_eq!(rejections(p_tab_unknown), vec![(1, Rejected::Tab)]);

        // Tab (1) beats MultipleMarkers (5)
        let p_tab_multi = "<!--\tfb:creates a.rs --> <!-- fb:creates b.rs -->";
        assert_eq!(rejections(p_tab_multi), vec![(1, Rejected::Tab)]);

        // TrailingText (2) beats UnknownWord (3)
        let p_trailing_unknown = "<!-- fb:unknown a.rs --> prose";
        assert_eq!(
            rejections(p_trailing_unknown),
            vec![(1, Rejected::TrailingText)]
        );

        // TrailingText (2) beats NoSpaceAfterOpener (4)
        let p_trailing_opener = "<!--fb:creates a.rs --> prose";
        assert_eq!(
            rejections(p_trailing_opener),
            vec![(1, Rejected::TrailingText)]
        );

        // TrailingText (2) beats MultipleMarkers (5)
        let p_trailing_multi = "<!-- fb:creates a.rs --> <!-- fb:creates b.rs --> prose";
        assert_eq!(
            rejections(p_trailing_multi),
            vec![(1, Rejected::TrailingText)]
        );

        let p_between_multi = "<!-- fb:creates a.rs --> and <!-- fb:creates b.rs -->";
        assert_eq!(
            rejections(p_between_multi),
            vec![(1, Rejected::TrailingText)]
        );

        // UnknownWord (3) beats NoSpaceAfterOpener (4)
        let p_unknown_opener = "<!--fb:unknown a.rs -->";
        assert_eq!(
            rejections(p_unknown_opener),
            vec![(1, Rejected::UnknownWord)]
        );

        // UnknownWord (3) beats MultipleMarkers (5)
        let p_unknown_multi = "<!-- fb:unknown a.rs --> <!-- fb:creates b.rs -->";
        assert_eq!(
            rejections(p_unknown_multi),
            vec![(1, Rejected::UnknownWord)]
        );

        // NoSpaceAfterOpener (4) beats MultipleMarkers (5)
        let p_opener_multi = "<!--fb:creates a.rs --> <!-- fb:creates b.rs -->";
        assert_eq!(
            rejections(p_opener_multi),
            vec![(1, Rejected::NoSpaceAfterOpener)]
        );
    }

    #[test]
    fn rejections_and_declarations_never_name_the_same_line() {
        let prompt = [
            "<!-- fb:creates valid1.rs -->",
            "<!--\tfb:creates tab.rs -->",
            "<!-- fb:creates valid2.rs -->",
            "<!-- fb:creates trailing.rs --> trailing text",
            "some prose that declares nothing",
            "<!-- fb:unknown bad_word.rs -->",
            "<!--fb:creates tight.rs -->",
            "<!-- fb:creates two_a.rs --> <!-- fb:creates two_b.rs -->",
            "<!-- fb:modifies valid3.rs -->",
        ]
        .join("\n");

        let decls = declarations(&prompt);
        let rejs = rejections(&prompt);

        assert_eq!(
            decls,
            vec![
                absent("valid1.rs"),
                absent("valid2.rs"),
                present("valid3.rs"),
            ]
        );

        assert_eq!(
            rejs,
            vec![
                (2, Rejected::Tab),
                (4, Rejected::TrailingText),
                (6, Rejected::UnknownWord),
                (7, Rejected::NoSpaceAfterOpener),
                (8, Rejected::MultipleMarkers),
            ]
        );
    }

    #[test]
    fn empty_prompt_boundaries() {
        assert_eq!(declarations(""), Vec::new());
        assert_eq!(rejections(""), Vec::new());
        assert_eq!(check("", &["a.rs"]), Precondition::Undeclared);
    }

    #[test]
    fn only_rejected_markers_prompt_is_undeclared() {
        let prompt = [
            "<!--\tfb:creates tab.rs -->",
            "<!-- fb:creates a.rs --> trailing",
            "<!--fb:creates opener.rs -->",
        ]
        .join("\n");
        assert_eq!(declarations(&prompt), Vec::new());
        assert_eq!(
            rejections(&prompt),
            vec![
                (1, Rejected::Tab),
                (2, Rejected::TrailingText),
                (3, Rejected::NoSpaceAfterOpener),
            ]
        );
        assert_eq!(
            check(&prompt, &["tab.rs", "a.rs"]),
            Precondition::Undeclared
        );
    }

    #[test]
    fn empty_comment_is_not_a_marker_or_rejection() {
        let prompt = ["<!-- -->", "<!---->", "<!--    -->"].join("\n");
        assert_eq!(declarations(&prompt), Vec::new());
        assert_eq!(rejections(&prompt), Vec::new());
        assert_eq!(check(&prompt, &[]), Precondition::Undeclared);
    }

    #[test]
    fn path_containing_close_rejects_rather_than_truncating() {
        let prompt = "<!-- fb:creates a/b-->c.rs -->";
        assert_eq!(declarations(prompt), Vec::new());
        assert_eq!(rejections(prompt), vec![(1, Rejected::TrailingText)]);
    }

    #[test]
    fn leading_whitespace_allowed() {
        assert_eq!(
            declarations("  <!-- fb:creates a.rs -->"),
            vec![absent("a.rs")]
        );
        assert_eq!(
            declarations("\t<!-- fb:modifies a.rs -->"),
            vec![present("a.rs")]
        );
        assert_eq!(rejections("  <!-- fb:creates a.rs -->"), Vec::new());
        assert_eq!(rejections("\t<!-- fb:modifies a.rs -->"), Vec::new());
    }

    #[test]
    fn membership_is_exact_string_match() {
        let prompt = "<!-- fb:modifies a/b.rs -->";
        let near_misses = ["a", "a/b.rs.orig", "A/b.rs", "a/b.rsx"];
        assert_eq!(
            check(prompt, &near_misses),
            Precondition::Violated(vec![violated("a/b.rs", Requirement::Present, false)]),
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
    fn all_holding_prompt_is_satisfied() {
        let prompt = ["<!-- fb:creates new.rs -->", "<!-- fb:modifies old.rs -->"].join("\n");
        assert_eq!(
            check(&prompt, &["old.rs", "unrelated.rs"]),
            Precondition::Satisfied
        );
    }
}

// ESCALATED: 2 confirmed finding(s) by critic gemini-38-flash, found on glm-53-flash.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_marker_grammar_glm_53_flash {
    use super::*;

    #[test]
    fn claim_1() {
        // "A line containing an unknown marker word followed by trailing text after the closer reports Rejected::UnknownWord instead of higher-priority Rejected::TrailingText."
        assert_eq!(
            rejections("<!-- fb:deletes a.rs --> trailing text"),
            vec![(1, Rejected::TrailingText)],
        );
    }

    #[test]
    fn claim_2() {
        // "A line containing an unknown marker word and a tab in the post-word separator reports Rejected::UnknownWord instead of higher-priority Rejected::Tab."
        assert_eq!(
            rejections("<!-- fb:deletes\ta.rs -->"),
            vec![(1, Rejected::Tab)],
        );
    }
}

// ESCALATED: 3 confirmed finding(s) by critic glm-53-flash, found on or-inkling.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_marker_grammar_or_inkling {
    use super::*;

    #[test]
    #[ignore = "the claimed Rejected and rejections API does not exist"]
    fn claim_1() {
        // "The Exact API additions — `Rejected` and `rejections(&str) -> Vec<(u32, Rejected)>` — do not exist."
        let prompt = "<!--fb:creates a.rs -->";
        let _ = prompt;
        // No assertion can be compiled because the claimed reporting API is absent.
    }

    #[test]
    fn claim_2() {
        // "A tab in any of the three separator positions yields a live declaration."
        let prompt = "<!-- fb:creates\ta.rs -->";
        assert!(declarations(prompt).is_empty());
        assert_eq!(check(prompt, &[]), Precondition::Undeclared);
    }

    #[test]
    fn claim_3() {
        // "The tight form — no space after the opener — is accepted."
        let prompt = "<!--fb:creates a.rs -->";
        assert!(declarations(prompt).is_empty());
        assert_eq!(check(prompt, &[]), Precondition::Undeclared);
    }

    #[test]
    fn claim_4() {
        // "Tilde fences are unrecognised; markers inside them are live."
        let prompt = "~~~\n<!-- fb:creates x.rs -->\n~~~";
        assert!(declarations(prompt).is_empty());
        assert_eq!(check(prompt, &[]), Precondition::Undeclared);
    }

    /// A WORD FROM ANOTHER FAMILY IS NOT A MALFORMED DELIVERABLE MARKER. This module refused
    /// every `fb:` word that was not `creates` or `modifies`, and the harness emits several
    /// others -- `fb:reads` is consumed by `dispatch_cmd::reads_of` and appears in fifteen
    /// specs. Each of those specs was REFUSED outright, and silently, so it could not be
    /// dispatched, pipelined or have its fate decided, with nothing saying why.
    #[test]
    fn a_marker_from_another_family_is_skipped_not_refused() {
        let prompt = "<!-- fb:reads sources.toml -->\n<!-- fb:creates a.rs -->\n";
        assert_eq!(
            rejections(prompt),
            Vec::new(),
            "fb:reads is not this module's business"
        );
        assert_eq!(
            declarations(prompt)
                .iter()
                .map(|d| d.path.clone())
                .collect::<Vec<_>>(),
            vec!["a.rs".to_string()]
        );
    }

    /// Every family in the exempt list is skipped, checked by ITERATING the list rather than
    /// restating it, so a family added without a reader is still caught by the doc's claim.
    #[test]
    fn every_exempt_family_is_skipped() {
        for word in OTHER_MARKER_FAMILIES {
            let prompt = format!("<!-- fb:{word} x -->\n<!-- fb:creates a.rs -->\n");
            assert_eq!(
                rejections(&prompt),
                Vec::new(),
                "fb:{word} should be skipped"
            );
        }
    }

    /// A word belonging to NO family is still a typo, and `fb:createsx` appears once in this
    /// tree. Exempting the unknown would turn a misspelling into a spec that declares
    /// nothing, silently.
    #[test]
    fn a_word_in_no_family_is_still_an_unknown_word() {
        assert_eq!(
            rejections("<!-- fb:createsx a.rs -->"),
            vec![(1, Rejected::UnknownWord)]
        );
    }
}

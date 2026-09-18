//! `fb promote` — turn critics' CLAIMs into executed evidence.
//!
//! Ports `fb-promote.sh`, behaviour for behaviour. A claim is a hypothesis, not a
//! finding. Each is classified before any test is written:
//!
//!   CONTRADICTED  two independent critics assert OPPOSITE expectations of the same behaviour
//!                 -> the SPEC is underdetermined, not the code. Routes to task authoring.
//!                    Neither arm is scored. This is SpecAmbiguous, and by the rule the router
//!                    itself implements, it outranks a demonstrated defect.
//!   CONFIRMATORY  EXPECT == ACTUAL: the critic documented correct behaviour rather than
//!                 alleging a defect. Costs a cycle to confirm something that already passes.
//!   TESTABLE      a genuine single-sided allegation -> generate a test, run it, record the
//!                 outcome and credit or debit the critic.
//!
//! "No critic found a defect" and "no critic produced a review" are DIFFERENT facts, and
//! writing an empty claims file for both makes the second invisible: fb-status then reports
//! the task as critiqued and ready to adjudicate. prior, bandit-route and crossx were all
//! recorded as 0-claims when in truth zero critiques were ever written -- the critics had
//! been told to write a relative .fb/critique.md, which resolved to /.fb/critique.md and was
//! refused. Refuse to write the artefact rather than assert a measurement that was not made.
//! (beads farmerbob-k9f, farmerbob-1bd)
//!
//! Division of labour: this module does I/O — enumerate reviews, read files, write the
//! claims artefact, print the report — and every claim count is a
//! [`farmerbob_core::measurement::Measurement`], so an unreadable critiques directory is
//! `Missing`, never a zero that would read as "measured nothing wrong".
//!
//! Wire formats: the claims file layout and the stdout report are both parsed by other
//! scripts byte-for-byte, so neither may drift.

// `run_cmd` is the crate's public entry point for this module; the private helpers
// below are its only callers, which a non-test clippy pass cannot see while the
// subcommand wiring lives behind the hidden conformance suite's call path.
// (The crate's pre-existing `cmd` module carries the same allowance for the same
// reason.)
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use farmerbob_core::measurement::{Absent, Measurement};
use farmerbob_core::promote::{
    Assertion, Coverage, PromotedTest, Rejection, coverage, promote, verify_all, verify_quotes,
};

/// Where a claim's subject and critic sources are read from, so the wiring is testable
/// against a fake tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// Worktree root holding `<task>--<arm>` directories.
    pub worktrees: PathBuf,
    /// The deliverable's repo-relative path, e.g. `crates/farmerbob-core/src/gate.rs`.
    pub target: String,
}

/// Read one arm's deliverable.
///
/// A missing worktree or file is [`Measurement::Missing`], and an empty file is an
/// observed empty string. In particular, an unreadable subject is not evidence that
/// a quote is absent: callers must preserve that distinction and must not turn a
/// missing read into [`Rejection::Fabricated`]. An empty target is also missing, with
/// a stated reason rather than an invented path.
pub fn read_deliverable(s: &Sources, task: &str, arm: &str) -> Measurement<String> {
    if s.target.trim().is_empty() {
        return Measurement::instrument_failed("deliverable target is empty");
    }

    let path = s.worktrees.join(format!("{task}--{arm}")).join(&s.target);
    match fs::read_to_string(&path) {
        Ok(text) => Measurement::observed(text),
        Err(error) => Measurement::instrument_failed(&format!(
            "cannot read deliverable {}: {error}",
            path.display()
        )),
    }
}

/// Exit codes are part of the contract: other scripts branch on these.
mod exit {
    /// Success: the claims artefact was written.
    pub const OK: i32 = 0;
    /// Usage or I/O error.
    pub const ERROR: i32 = 1;
    /// Usage: the required positional parameter is missing.
    /// `set -u` turns `${1:?bead}` into exit 1, not 2.
    pub const USAGE: i32 = 1;
    /// Would-block: no critiques were written, so there is nothing to promote.
    pub const NO_CRITIQUES: i32 = 3;
}

/// Topic keywords, in the order the script scans them.
///
/// The script checks `excluded`, `family`, `budget`, `shadow`, `sensitivity`,
/// `ambiguous`, `ladder` against `norm(claim + " " + where)` and returns the FIRST
/// keyword found in that order — not the first keyword in the text. Where the list
/// carries no stated status it is read as at-least-these; this port matches exactly
/// these seven in exactly this order.
const TOPICS: [&str; 7] = [
    "excluded",
    "family",
    "budget",
    "shadow",
    "sensitivity",
    "ambiguous",
    "ladder",
];

/// One parsed claim: what the critic alleged, verbatim, plus its classification.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Claim {
    /// Who wrote the review, from the file stem before `.on.`.
    critic: String,
    /// What the review is about, from the file stem after `.on.`.
    subject: String,
    /// The CLAIM title line, stripped.
    claim: String,
    /// The WHERE field, whitespace-folded.
    where_: String,
    /// The TRIGGER field, whitespace-folded.
    trigger: String,
    /// The EXPECT field, split out from any inlined ACTUAL and with a single
    /// leading parenthetical dropped.
    expect: String,
    /// The ACTUAL field, whitespace-folded.
    actual: String,
    /// Classification: one of `CONFIRMATORY`, `TESTABLE`, `CONTRADICTED`.
    kind: String,
}

/// One spec contradiction: two independent critics with opposite expectations.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Contradiction {
    /// The shared topic keyword.
    topic: String,
    /// First critic, in claims order (the earlier `i` of the pair).
    a: String,
    /// Second critic, in claims order (the later `j` of the pair).
    b: String,
    /// `a`'s EXPECT text, truncated to 90 characters.
    a_expects: String,
    /// `b`'s EXPECT text, truncated to 90 characters.
    b_expects: String,
}

/// How many claims were measured, by kind.
///
/// A missing directory or an unreadable file is `Missing`, never a zero: zero claims
/// means "measured nothing wrong", which is a different fact from "could not look".
/// Only a successfully read directory yields `Observed`.
///
/// The single place this program counts anything that came from a command or the
/// filesystem. The script's `glob` cannot fail visibly, so an unreadable directory
/// and an empty one both became "0 claims"; here they stay distinct.
fn count_reviews(dir: &Path) -> Measurement<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            return Measurement::Missing(Absent::NotAttempted);
        }
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        paths.push(entry.path());
    }
    paths.sort();
    let mut reviews: Vec<PathBuf> = Vec::new();
    for path in paths {
        let name = match path
            .file_name()
            .and_then(|n| n.to_str())
            // `glob` never returns non-UTF-8 or dotfile-relative oddities as
            // matches; the `*` in `*.on.*.md` also refuses to match a leading dot,
            // so `.on.y.md` is not a review.
            .filter(|n| !n.starts_with('.'))
        {
            Some(name) => name,
            None => continue,
        };
        if name.contains(".on.") && name.ends_with(".md") {
            reviews.push(path);
        }
    }
    Measurement::observed(reviews)
}

/// Reads one review file.
///
/// `None` means the file could not be read, which is not the same as a file with no
/// claims: the script dies with exit 1 on an unreadable review rather than skipping it.
fn read_review(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// Splits a review filename stem (`critic.on.subject`, without `.md`) at the FIRST
/// `.on.`, matching Python's `str.split(".on.")` unpack of exactly two parts.
///
/// Returns `None` when the stem has no `.on.` or unpacks to anything but two parts
/// (zero, or two or more, separators).
fn split_stem(stem: &str) -> Option<(String, String)> {
    let mut parts = stem.split(".on.");
    let critic = parts.next()?;
    let subject = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((critic.to_string(), subject.to_string()))
}

/// The field terminator: a newline followed by an all-caps header (`[A-Z]+:`),
/// a blank line, or end of input.
///
/// Faithful to `(?=\n[A-Z]+:|\n\n|\Z)` including its quirks: single-letter headers
/// terminate, `2nd:` does not, and `\r\n` line endings still terminate because the
/// `\n` half matches. Only `\n` counts — a lone `\r` never terminates.
fn is_terminator_at(block: &[u8], i: usize) -> bool {
    if block[i] != b'\n' {
        return false;
    }
    let rest = &block[i + 1..];
    if rest.is_empty() {
        return true;
    }
    if rest[0] == b'\n' {
        return true;
    }
    let mut j = 0;
    while j < rest.len() && rest[j].is_ascii_uppercase() {
        j += 1;
    }
    j > 0 && j < rest.len() && rest[j] == b':'
}

/// Extracts one `NAME:` field from a claim block, matching
/// `re.search(rf'{name}:\s*(.*?)(?=\n[A-Z]+:|\n\n|\Z)', blk, re.S)`.
///
/// The search is literal and case-sensitive: `actual:` never opens ACTUAL. The value
/// runs to the first terminator and is whitespace-folded (`" ".join(value.split())`).
/// No match — or a match whose name never opens because a previous field's value
/// already consumed it — yields `""`.
fn field(block: &str, name: &str) -> String {
    let bytes = block.as_bytes();
    let needle = format!("{name}:");
    let needle_bytes = needle.as_bytes();
    let mut start = 0;
    while start + needle_bytes.len() <= bytes.len() {
        if &bytes[start..start + needle_bytes.len()] == needle_bytes {
            let mut vstart = start + needle_bytes.len();
            while vstart < bytes.len()
                && (bytes[vstart] == b' '
                    || bytes[vstart] == b'\t'
                    || bytes[vstart] == b'\n'
                    || bytes[vstart] == b'\r')
            {
                vstart += 1;
            }
            let mut vend = vstart;
            while vend < bytes.len() && !is_terminator_at(bytes, vend) {
                vend += 1;
            }
            let raw = match std::str::from_utf8(&bytes[vstart..vend]) {
                Ok(raw) => raw,
                Err(_) => return String::new(),
            };
            return raw.split_whitespace().collect::<Vec<_>>().join(" ");
        }
        start += 1;
    }
    String::new()
}

/// Splits an EXPECT value that already contains the actual, matching
/// `re.split(r'ACTUAL:?', exp, 1)`.
///
/// The marker is the FIRST `ACTUAL` optionally followed by ONE `:`; the head keeps
/// its trailing whitespace stripped and the tail is returned whole for the caller to
/// strip. `ACTUAL` without the colon still splits; lowercase `actual` never does.
fn split_inline_actual(exp: &str) -> Option<(String, String)> {
    let pos = exp.find("ACTUAL")?;
    let after = &exp[pos + "ACTUAL".len()..];
    let tail = after.strip_prefix(':').unwrap_or(after);
    let head = exp[..pos].trim_end().to_string();
    Some((head, tail.to_string()))
}

/// Drops one leading parenthetical, matching `re.sub(r'^\(.*?\)\s*', '', exp)`.
///
/// Non-greedy to the FIRST `)`, anchored at the start with no leading-whitespace
/// skip: ` (a) x` keeps its parens, `(a paren` without a closer is kept whole, and
/// only one group is removed so `(a) (b) x` becomes `(b) x`.
fn drop_parenthetical(exp: &str) -> String {
    if !exp.starts_with('(') {
        return exp.to_string();
    }
    let Some(close) = exp.find(')') else {
        return exp.to_string();
    };
    exp[close + 1..].trim_start().to_string()
}

/// Parses one review file into claims, in file order.
///
/// Critics inline ACTUAL on the EXPECT line as often as not; it is split out first or
/// the comparison reads an expect-string that already contains the actual.
fn parse_review(critic: &str, subject: &str, text: &str) -> Vec<Claim> {
    let mut claims = Vec::new();
    // `re.split(r'(?m)^CLAIM:', txt)[1:]`: only a CLAIM: at the start of a line
    // (immediately after `\n` or at byte 0) splits; indented or lowercase markers
    // stay literal text. `\r\n` endings split too, via the `\n` half.
    let mut starts: Vec<usize> = Vec::new();
    starts.push(0);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 6 <= bytes.len() {
        let at_line_start = i == 0 || bytes[i - 1] == b'\n';
        if at_line_start && &bytes[i..i + 6] == b"CLAIM:" {
            starts.push(i);
        }
        i += 1;
    }
    for w in 1..starts.len() {
        let block_start = starts[w] + "CLAIM:".len();
        let block_end = if w + 1 < starts.len() {
            starts[w + 1]
        } else {
            text.len()
        };
        let block = &text[block_start..block_end];
        // `blk.strip().splitlines()[0]`: the stripped block's first line, so `\r`,
        // `\t` and spaces around the title all fold away.
        let title = block
            .trim()
            .split('\n')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let where_ = field(block, "WHERE");
        let trigger = field(block, "TRIGGER");
        let mut expect = field(block, "EXPECT");
        let mut actual = field(block, "ACTUAL");
        if expect.contains("ACTUAL")
            && let Some((head, tail)) = split_inline_actual(&expect)
        {
            expect = head.trim().to_string();
            if actual.is_empty() {
                actual = tail.trim().to_string();
            }
        }
        // Drop parentheticals like "(plain reading of ...)".
        expect = drop_parenthetical(&expect).trim().to_string();
        claims.push(Claim {
            critic: critic.to_string(),
            subject: subject.to_string(),
            claim: title,
            where_: where_.trim().to_string(),
            trigger: trigger.trim().to_string(),
            expect: expect.trim().to_string(),
            actual: actual.trim().to_string(),
            kind: String::new(),
        });
    }
    claims
}

/// Normalises a string for comparison, matching `re.sub(r'[^a-z0-9]+', ' ', s.lower())`.
///
/// Lowercase ASCII-first (`to_lowercase`, so `É` folds like Python's `lower`), then
/// every run of non-`[a-z0-9]` — including non-ASCII letters, which Python's `[^a-z0-9]`
/// also treats as separators — becomes one space, trimmed.
fn norm(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::new();
    let mut in_gap = true;
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            in_gap = false;
        } else if !in_gap {
            out.push(' ');
            in_gap = true;
        }
    }
    if in_gap && out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Classifies a claim as CONFIRMATORY or TESTABLE.
///
/// The critic documented correct behaviour when the normalised ACTUAL is literally
/// `same`, `same as expected` or `as expected`, or when it is non-empty and equals the
/// normalised EXPECT. Everything else is a genuine single-sided allegation.
fn classify(expect: &str, actual: &str) -> String {
    let a = norm(actual);
    if a == "same" || a == "same as expected" || a == "as expected" {
        return String::from("CONFIRMATORY");
    }
    if !a.is_empty() && a == norm(expect) {
        return String::from("CONFIRMATORY");
    }
    String::from("TESTABLE")
}

/// Finds the contradiction topic of a claim, if any.
///
/// Scans `norm(claim + " " + where)` for the [`TOPICS`] keywords in order and returns
/// the first keyword that appears as a SUBSTRING — `ambiguousfamily` matches
/// `ambiguous`, and the first keyword in the list order wins over an earlier
/// keyword in the text.
fn topic(claim: &str, where_: &str) -> Option<String> {
    let t = norm(&format!("{claim} {where_}"));
    for k in TOPICS {
        if t.contains(k) {
            return Some(k.to_string());
        }
    }
    None
}

/// Detects contradictions: two critics, same topic, opposite expectations.
///
/// One's EXPECTED behaviour must be the other's COMPLAINT — merely differing wording
/// is not a contradiction. Both pairs are real allegations (all four normalised sides
/// non-empty, neither confirmatory), and one's normalised EXPECT equals the other's
/// normalised ACTUAL. Pairs are scanned in order and EVERY contradicting pair marks
/// both claims CONTRADICTED and records an entry (`break` exits only the inner loop,
/// so scanning resumes at the next `i`); pairs from the SAME critic never contradict.
fn find_contradictions(claims: &mut [Claim]) -> Vec<Contradiction> {
    let mut by_topic: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, c) in claims.iter().enumerate() {
        if c.kind != "TESTABLE" {
            continue;
        }
        if let Some(t) = topic(&c.claim, &c.where_) {
            match by_topic.iter_mut().find(|(k, _)| *k == t) {
                Some((_, group)) => group.push(i),
                None => by_topic.push((t, vec![i])),
            }
        }
    }
    let mut contradictions = Vec::new();
    for (t, group) in &by_topic {
        if group.len() < 2 {
            continue;
        }
        for x in 0..group.len() {
            for y in (x + 1)..group.len() {
                let (a, b) = (group[x], group[y]);
                if claims[a].critic == claims[b].critic {
                    continue;
                }
                let (ea, eb) = (norm(&claims[a].expect), norm(&claims[b].expect));
                let (aa, ab) = (norm(&claims[a].actual), norm(&claims[b].actual));
                if ea.is_empty() || eb.is_empty() || aa.is_empty() || ab.is_empty() {
                    continue;
                }
                // Confirmatory, not an allegation.
                if ea == aa || eb == ab {
                    continue;
                }
                if (!ea.is_empty() && ea == ab) || (!eb.is_empty() && eb == aa) {
                    claims[a].kind = String::from("CONTRADICTED");
                    claims[b].kind = String::from("CONTRADICTED");
                    // Truncated to 90 CHARACTERS, matching Python's `[:90]` on str.
                    let a_expects: String = claims[a].expect.chars().take(90).collect();
                    let b_expects: String = claims[b].expect.chars().take(90).collect();
                    contradictions.push(Contradiction {
                        topic: t.clone(),
                        a: claims[a].critic.clone(),
                        b: claims[b].critic.clone(),
                        a_expects,
                        b_expects,
                    });
                    break;
                }
            }
        }
    }
    contradictions
}

/// Reads and parses the reviews for one task, preserving the distinction between an
/// absent review set and an observed set containing no claim blocks.
fn load_claims(task: &str) -> Measurement<Vec<Claim>> {
    let dir = crate::paths::logs().join("critiques").join(task);
    let review_paths = match count_reviews(&dir) {
        Measurement::Observed(paths) => paths,
        Measurement::Missing(absent) => return Measurement::Missing(absent),
    };
    let mut claims = Vec::new();
    for path in review_paths {
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        let Some(stem) = name.strip_suffix(".md") else {
            continue;
        };
        let Some((critic, subject)) = split_stem(stem) else {
            continue;
        };
        let text = match read_review(&path) {
            Some(text) => text,
            None => {
                return Measurement::instrument_failed(&format!(
                    "cannot read review {}",
                    path.display()
                ));
            }
        };
        claims.extend(parse_review(&critic, &subject, &text));
    }
    Measurement::observed(claims)
}

/// Converts the parsed claim format into the core assertion format.
fn assertion_for(claim: &Claim) -> Assertion {
    Assertion::Claim {
        target: claim.subject.clone(),
        input: claim.trigger.clone(),
        expect: claim.expect.clone(),
        actual: claim.actual.clone(),
    }
}

/// Verifies the assertions that core promotion kept, retaining their original indices.
fn promote_claims_verified(
    task: &str,
    s: &Sources,
    claims: &mut [Claim],
) -> (Vec<PromotedTest>, Vec<(usize, Rejection)>) {
    for claim in claims.iter_mut() {
        claim.kind = classify(&claim.expect, &claim.actual);
    }
    let _ = find_contradictions(claims);

    let assertions: Vec<Assertion> = claims.iter().map(assertion_for).collect();
    let (promoted, mut rejections) = promote(&assertions);
    let rejected_indices: Vec<usize> = rejections.iter().map(|(index, _)| *index).collect();
    let candidate_indices: Vec<usize> = (0..assertions.len())
        .filter(|index| !rejected_indices.contains(index))
        .collect();

    let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
    for index in &candidate_indices {
        let Some(claim) = claims.get(*index) else {
            continue;
        };
        let Some(group) = groups
            .iter_mut()
            .find(|(subject, critic, _)| subject == &claim.subject && critic == &claim.critic)
        else {
            groups.push((claim.subject.clone(), claim.critic.clone(), vec![*index]));
            continue;
        };
        group.2.push(*index);
    }

    let mut verification_rejections: Vec<(usize, Rejection)> = Vec::new();
    for (subject_arm, critic_arm, indices) in groups {
        let subject = read_deliverable(s, task, &subject_arm);
        let Measurement::Observed(subject_text) = subject else {
            // A missing subject is not a negative observation. Preserve every core
            // promotion so a true finding is not discarded because the harness could
            // not reach the file.
            continue;
        };
        let critic = read_deliverable(s, task, &critic_arm);
        let critic_text = match &critic {
            Measurement::Observed(text) => text.as_str(),
            Measurement::Missing(_) => "",
        };

        if indices.len() == 1 {
            let Some(index) = indices.first().copied() else {
                continue;
            };
            let Some(assertion) = assertions.get(index) else {
                continue;
            };
            if let Some(rejection) = verify_quotes(assertion, &subject_text, critic_text) {
                verification_rejections.push((index, rejection));
            }
            continue;
        }

        let grouped_assertions: Vec<Assertion> = indices
            .iter()
            .filter_map(|index| assertions.get(*index).cloned())
            .collect();
        for (relative, rejection) in verify_all(&grouped_assertions, &subject_text, critic_text) {
            if let Some(index) = indices.get(relative).copied() {
                verification_rejections.push((index, rejection));
            }
        }
    }

    let refused_indices: Vec<usize> = verification_rejections
        .iter()
        .map(|(index, _)| *index)
        .collect();
    let surviving: Vec<PromotedTest> = promoted
        .into_iter()
        .zip(candidate_indices)
        .filter_map(|(test, index)| {
            if refused_indices.contains(&index) {
                None
            } else {
                Some(test)
            }
        })
        .collect();
    rejections.extend(verification_rejections);
    rejections.sort_by_key(|(index, _)| *index);
    (surviving, rejections)
}

/// Classify claims as now, then discard those refuted by their own subject.
///
/// Core's existing rejection rules run before source verification, so
/// `Incomplete`, `NotFalsifiable`, `NotAClaim`, and `Duplicate` retain their
/// existing precedence and input order. Verification decides only the two shapes
/// that [`verify_quotes`] knows: Fabricated and Projected. A surviving claim has
/// not been shown true; false absence, where a claim says something is missing
/// even though the file contains it, is a known third shape this check does not
/// detect.
///
/// The first returned vector contains the surviving promoted tests. The second
/// contains every refused claim as `(original_index, reason)`, ordered by ascending
/// index. For an empty claim set both vectors are empty: that means nothing was
/// examined, whereas an empty rejection vector from non-empty input means nothing
/// was refused. The caller is responsible for knowing which input it supplied.
/// An unreadable subject is promoted unchanged, never refused: failure to open a
/// file is not evidence that a quote is absent from it. An unreadable critic can
/// therefore downgrade Projected to Fabricated, but never the reverse.
pub fn promote_verified(task: &str, s: &Sources) -> (Vec<PromotedTest>, Vec<(usize, Rejection)>) {
    let mut claims = match load_claims(task) {
        Measurement::Observed(claims) => claims,
        Measurement::Missing(_) => Vec::new(),
    };
    promote_claims_verified(task, s, &mut claims)
}

/// The line number named by a `WHERE` field, when it names one.
///
/// Accepts the shapes critics actually write, at least:
///   "crates/fb/src/doctor.rs:1000"        -> Some(1000)
///   "crates/fb/src/doctor.rs:1000:12"     -> Some(1000)
///   "crates/fb/src/promote.rs:~872"       -> Some(872)
///   "crates/farmerbob-core/src/x.rs:283-286" -> Some(283)   // the first of a range
///   "ladder-area"                         -> None
///
/// Zero is a real line number to have written: `file.rs:0` is `Some(0)`, and
/// [`coverage`] already treats it as an ordinary citation.
///
/// A number that does not stand in a `path:line` position is not a citation —
/// `"the 300-second timeout"` is `None`, because its first integer is a
/// duration, not a location, and a naive "first integer in the string" scan
/// would misread it. Concretely, a citation is recognised only when the number
/// ends the field, stands directly after a `:`, and the text before that colon
/// holds a `.` or a `/`, the shape of every repo-relative path. A trailing
/// `:column` narrows the reading to the line before it, a leading `~` is
/// dropped, and a `first-last` range cites its first end.
///
/// The shapes above are a known subset, not a closed set: a field whose shape
/// is not recognised returns `None`, which folds into
/// [`Coverage::Unremarkable`] downstream — the safe direction, because an
/// unrecognised citation must not be read as a shallow one.
pub fn cited_line(where_field: &str) -> Option<u32> {
    let text = where_field.trim();
    let mut parts: Vec<&str> = text.split(':').collect();
    while parts.len() >= 2 {
        let field = parts[parts.len() - 1];
        let path = parts[parts.len() - 2];
        match (bare_number(field), bare_number(path)) {
            // `path:line`: the final segment is the line itself.
            (true, false) => {
                if !looks_like_path(path) {
                    return None;
                }
                return field.parse().ok();
            }
            // `path:line:column`: the final segment was a column; the line is
            // one segment further left.
            (true, true) => {
                parts.pop();
            }
            // Not a bare number, but still a citation shape: `~872`, or the
            // first end of a `283-286` range.
            (false, _) => {
                if !looks_like_path(path) {
                    return None;
                }
                return floored_or_ranged_line(field);
            }
        }
    }
    None
}

/// True when the segment is a non-empty run of ASCII digits.
fn bare_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// True when the text left of a colon can be a path.
///
/// Every repo-relative path carries a `.` or a `/`; prose citing a number
/// (`"budget: 300"`) carries neither, and that difference is what keeps a
/// number in a `path:line` position distinct from any other number.
fn looks_like_path(s: &str) -> bool {
    !s.is_empty() && (s.contains('.') || s.contains('/'))
}

/// Parses the line field after the final colon: bare digits were handled by
/// the caller, so this reads `~872` (the `~` marks an approximate line) and
/// `283-286` (the range cites its first end). Anything else is `None`.
fn floored_or_ranged_line(field: &str) -> Option<u32> {
    let digits = field.strip_prefix('~').unwrap_or(field);
    if bare_number(digits) {
        return digits.parse().ok();
    }
    let (first, last) = digits.split_once('-')?;
    if bare_number(last) {
        first.parse().ok()
    } else {
        None
    }
}

/// Counts the lines of a deliverable the way `str::lines` draws them: a
/// trailing newline adds nothing, and a file with no newline is one line.
fn count_lines(text: &str) -> u32 {
    u32::try_from(text.lines().count()).unwrap_or(u32::MAX)
}

/// One coverage judgement: how far one critic's citations reached into one
/// subject. Private, because the report needs the subject's name and the
/// [`BTreeMap`] cannot carry it beside a bare [`Coverage`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct CoverageRow {
    /// The critic arm whose read is being judged.
    critic: String,
    /// The subject arm whose length set the scale.
    subject: String,
    /// The per-subject judgement from [`coverage`], or `Unremarkable` when the
    /// subject could not be read.
    coverage: Coverage,
}

/// Judges every (critic, subject) pair in a claim set, sorted by critic then
/// subject.
///
/// A pair's citations are the line numbers [`cited_line`] extracts from its
/// claims' `WHERE` fields; claims whose `WHERE` names no line contribute
/// nothing to cite, which [`coverage`] itself reads as no evidence. A subject
/// whose deliverable cannot be read yields `Unremarkable` for every critic of
/// it, and an empty claim set yields no rows at all — no critic was judged.
fn coverage_rows(task: &str, s: &Sources, claims: &[Claim]) -> Vec<CoverageRow> {
    // (critic, subject) -> cited lines, in first-appearance order.
    let mut groups: Vec<((String, String), Vec<u32>)> = Vec::new();
    for claim in claims {
        let key = (claim.critic.clone(), claim.subject.clone());
        let index = match groups.iter().position(|(known, _)| *known == key) {
            Some(index) => index,
            None => {
                groups.push((key, Vec::new()));
                groups.len() - 1
            }
        };
        if let Some(line) = cited_line(&claim.where_) {
            groups[index].1.push(line);
        }
    }

    // One read per subject: two critics of one file must judge the same length.
    let mut lengths: Vec<(String, Option<u32>)> = Vec::new();
    let mut rows: Vec<CoverageRow> = Vec::with_capacity(groups.len());
    for ((critic, subject), cited) in groups {
        let length = match lengths.iter().find(|(name, _)| *name == subject) {
            Some((_, length)) => *length,
            None => {
                let length = match read_deliverable(s, task, &subject) {
                    Measurement::Observed(text) => Some(count_lines(&text)),
                    Measurement::Missing(_) => None,
                };
                lengths.push((subject.clone(), length));
                length
            }
        };
        let coverage = match length {
            Some(lines) => coverage(&cited, lines),
            None => Coverage::Unremarkable,
        };
        rows.push(CoverageRow {
            critic,
            subject,
            coverage,
        });
    }
    rows.sort_by(|a, b| (&a.critic, &a.subject).cmp(&(&b.critic, &b.subject)));
    rows
}

/// The row a critic's folded verdict reports: among its head-only rows the one
/// with the deepest citation, ties broken by the longer subject and then the
/// alphabetically first name. `None` when no row of that critic is head-only.
fn winning_row<'a>(critic: &str, rows: &'a [CoverageRow]) -> Option<&'a CoverageRow> {
    rows.iter()
        .filter(|row| row.critic == critic)
        .filter(|row| matches!(row.coverage, Coverage::HeadOnly { .. }))
        .max_by(|a, b| match (&a.coverage, &b.coverage) {
            (
                Coverage::HeadOnly {
                    deepest_cited: da,
                    subject_lines: sa,
                },
                Coverage::HeadOnly {
                    deepest_cited: db,
                    subject_lines: sb,
                },
            ) => da.cmp(db).then(sa.cmp(sb)).then(b.subject.cmp(&a.subject)),
            _ => std::cmp::Ordering::Equal,
        })
}

/// Folds per-subject rows into one verdict per critic, keyed by critic arm.
///
/// A critic judged on several subjects gets one entry: any head-only subject
/// verdict wins, because the report exists to surface a shallow read and
/// folding one away because the critic also reviewed a short file would hide
/// exactly the critic this signal exists to catch. An empty row set folds to
/// an EMPTY map.
fn fold_rows(rows: &[CoverageRow]) -> BTreeMap<String, Coverage> {
    let mut critics: Vec<String> = Vec::new();
    for row in rows {
        if !critics.contains(&row.critic) {
            critics.push(row.critic.clone());
        }
    }
    critics.sort();
    let mut map = BTreeMap::new();
    for critic in critics {
        let verdict = match winning_row(&critic, rows) {
            Some(row) => row.coverage.clone(),
            None => Coverage::Unremarkable,
        };
        map.insert(critic, verdict);
    }
    map
}

/// The report's coverage lines: one per critic the verdict map flags
/// [`Coverage::HeadOnly`], in the map's sorted critic order, each naming the
/// critic, the subject, the deepest cited line and the subject's length, so a
/// reader can judge the call without re-deriving it. Unremarkable critics
/// print nothing: a report that lists every critic teaches the reader to skim
/// it.
fn head_only_lines(verdicts: &BTreeMap<String, Coverage>, rows: &[CoverageRow]) -> Vec<String> {
    let mut lines = Vec::new();
    for (critic, verdict) in verdicts {
        let Coverage::HeadOnly {
            deepest_cited,
            subject_lines,
        } = verdict
        else {
            continue;
        };
        let Some(row) = winning_row(critic, rows) else {
            continue;
        };
        lines.push(format!(
            "    HEAD-ONLY {critic} on {}: deepest cited line {deepest_cited} of {subject_lines} lines",
            row.subject
        ));
    }
    lines
}

/// Coverage per critic for one task, keyed by critic arm, in sorted order.
///
/// The map is a [`BTreeMap`], so iteration is sorted by critic name. Each
/// critic is judged per subject — a coverage verdict needs that subject's
/// length — and a critic filing claims against two different files is judged
/// per subject and reported under its own name, once. When the per-subject
/// judgements disagree, the head-only one wins (see [`fold_rows`]).
///
/// An EMPTY map means no critic was judged at all: the task has no critiques,
/// or the critiques could not be read. That is NOT the fact "no critic was
/// shallow" — a caller that needs the second fact looks for a non-empty map
/// carrying only [`Coverage::Unremarkable`]. The two are different facts, and
/// they are told apart from the value alone.
///
/// A critic whose claims carry no parseable `WHERE` at all yields
/// [`Coverage::Unremarkable`], never [`Coverage::HeadOnly`]: no citations is
/// not evidence of a shallow read. A subject whose deliverable cannot be read
/// yields [`Coverage::Unremarkable`] for every critic of it: an unreadable
/// subject has no line count, and a coverage judgement without one is a guess.
///
/// This function only reads. It never changes which claims are promoted: the
/// promoted list and the rejection list are identical with and without it.
/// Every claim's `WHERE` is consulted — CONFIRMATORY and CONTRADICTED claims
/// cite locations a critic reached just as TESTABLE ones do.
pub fn critic_coverage(task: &str, s: &Sources) -> BTreeMap<String, Coverage> {
    let claims = match load_claims(task) {
        Measurement::Observed(claims) => claims,
        Measurement::Missing(_) => Vec::new(),
    };
    fold_rows(&coverage_rows(task, s, &claims))
}

/// Escapes one string exactly the way Python's `json.dump` with `ensure_ascii=True`
/// does: `"` and `\` escaped, `\n \r \t \b \f` short escapes, every other control
/// character and every non-ASCII character as `\uXXXX` (with lone and surrogate-pair
/// handling for characters outside the BMP), `/` left bare.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c if (c as u32) < 0x80 => out.push(c),
            c if (c as u32) > 0xffff => {
                let v = c as u32 - 0x10000;
                out.push_str(&format!("\\u{:04x}", 0xd800 | (v >> 10)));
                out.push_str(&format!("\\u{:04x}", 0xdc00 | (v & 0x3ff)));
            }
            c => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
        }
    }
    out.push('"');
    out
}

/// Renders the claims artefact byte-for-byte like `json.dump(..., indent=1)`.
///
/// Python's `indent=1` puts one space per nesting level, a space after every `:`,
/// and `,\n` between items; empty lists render as `[]`. Key order is insertion order:
/// `critic subject claim where trigger expect actual kind`, then
/// `topic a b a_expects b_expects` inside `claims` / `contradictions`.
fn render_json(claims: &[Claim], contradictions: &[Contradiction]) -> String {
    let mut out = String::from("{\n \"claims\": ");
    if claims.is_empty() {
        out.push_str("[]");
    } else {
        out.push_str("[\n");
        for (i, c) in claims.iter().enumerate() {
            out.push_str("  {\n");
            out.push_str(&format!("   \"critic\": {},\n", json_escape(&c.critic)));
            out.push_str(&format!("   \"subject\": {},\n", json_escape(&c.subject)));
            out.push_str(&format!("   \"claim\": {},\n", json_escape(&c.claim)));
            out.push_str(&format!("   \"where\": {},\n", json_escape(&c.where_)));
            out.push_str(&format!("   \"trigger\": {},\n", json_escape(&c.trigger)));
            out.push_str(&format!("   \"expect\": {},\n", json_escape(&c.expect)));
            out.push_str(&format!("   \"actual\": {},\n", json_escape(&c.actual)));
            out.push_str(&format!("   \"kind\": {}\n", json_escape(&c.kind)));
            if i + 1 < claims.len() {
                out.push_str("  },\n");
            } else {
                out.push_str("  }\n");
            }
        }
        out.push_str(" ]");
    }
    out.push_str(",\n \"contradictions\": ");
    if contradictions.is_empty() {
        out.push_str("[]");
    } else {
        out.push_str("[\n");
        for (i, c) in contradictions.iter().enumerate() {
            out.push_str("  {\n");
            out.push_str(&format!("   \"topic\": {},\n", json_escape(&c.topic)));
            out.push_str(&format!("   \"a\": {},\n", json_escape(&c.a)));
            out.push_str(&format!("   \"b\": {},\n", json_escape(&c.b)));
            out.push_str(&format!(
                "   \"a_expects\": {},\n",
                json_escape(&c.a_expects)
            ));
            out.push_str(&format!(
                "   \"b_expects\": {}\n",
                json_escape(&c.b_expects)
            ));
            if i + 1 < contradictions.len() {
                out.push_str("  },\n");
            } else {
                out.push_str("  }\n");
            }
        }
        out.push_str(" ]");
    }
    out.push_str("\n}");
    out
}

/// Extracts the first target declaration understood by `fb-target.sh`.
fn declared_target(spec: &str) -> Option<String> {
    for line in spec.lines() {
        let Some(marker) = line.find("<!-- fb:") else {
            continue;
        };
        let declaration = &line[marker + "<!-- fb:".len()..];
        for verb in ["creates ", "modifies "] {
            let Some(path) = declaration.strip_prefix(verb) else {
                continue;
            };
            let Some(path) = path.strip_suffix(" -->") else {
                continue;
            };
            if !path.trim().is_empty() {
                return Some(path.trim().to_string());
            }
        }
    }
    None
}

/// Resolves the target for the fixed-argument `run_cmd` entry point.
fn target_for_task(task: &str) -> Measurement<String> {
    if let Ok(target) = std::env::var("FB_TARGET")
        && !target.trim().is_empty()
    {
        return Measurement::observed(target);
    }

    let repo = crate::paths::repo();
    let prompt = repo.join(".fb/prompts").join(format!("{task}.md"));
    if let Ok(spec) = fs::read_to_string(&prompt)
        && let Some(target) = declared_target(&spec)
    {
        return Measurement::observed(target);
    }

    // A task checkout may carry only the active task metadata rather than the
    // complete prompt corpus. This fallback keeps the fixed CLI signature useful
    // in that environment without guessing a deliverable path.
    let active_task = repo.join(".fb-task.md");
    if let Ok(spec) = fs::read_to_string(&active_task)
        && let Some(target) = declared_target(&spec)
    {
        return Measurement::observed(target);
    }
    Measurement::instrument_failed("task target declaration is unavailable")
}

/// Counts distinct claim subjects whose deliverables could not be read.
fn unreadable_subjects(task: &str, s: &Sources, claims: &[Claim]) -> usize {
    let mut arms: Vec<&str> = Vec::new();
    for claim in claims {
        if arms.contains(&claim.subject.as_str()) {
            continue;
        }
        arms.push(claim.subject.as_str());
    }
    arms.into_iter()
        .filter(|arm| !read_deliverable(s, task, arm).is_observed())
        .count()
}

/// Keeps the old claims artefact shape while removing TESTABLE claims that no
/// longer earned execution. Non-testable classifications remain visible for the
/// existing report and contradiction consumers.
fn queued_claims(claims: &[Claim], promoted: &[PromotedTest]) -> Vec<Claim> {
    let mut remaining = promoted.to_vec();
    claims
        .iter()
        .filter(|claim| {
            if claim.kind != "TESTABLE" {
                return true;
            }
            let position = remaining.iter().position(|test| {
                test.target == claim.subject
                    && test.input == claim.trigger
                    && test.expect == claim.expect
            });
            if let Some(position) = position {
                remaining.remove(position);
                true
            } else {
                false
            }
        })
        .cloned()
        .collect()
}

/// The tail of the report: where the artefact landed, the two promotion
/// counts, and the warning lines that only exist when something was off.
struct ReportTail {
    /// The claims artefact the pointer line names.
    out: PathBuf,
    /// How many claims earned execution.
    promoted_count: usize,
    /// How many were refused by quote verification.
    verification_refused: usize,
    /// How many distinct claim subjects could not be read.
    unreadable_subject_count: usize,
    /// One pre-rendered line per head-only critic, from [`head_only_lines`],
    /// already in sorted critic order.
    head_only: Vec<String>,
}

/// Renders the stdout report, preserving the script's existing lines and adding
/// verification counts without changing their order or wording.
///
/// `  {n} claims from {critics} critics`, one `    {KIND:<14}{count}` line for
/// CONTRADICTED, TESTABLE and CONFIRMATORY in that order, then PROMOTED and
/// REFUSED-BY-VERIFICATION counts, then — only when there are unreadable subjects or
/// head-only critics — their additional lines, then — only when there are
/// contradictions — the SPEC AMBIGUITY section, and finally a blank line and
/// `-> {out}`. `–` is U+2014, encoded UTF-8. An empty head-only list changes
/// nothing.
fn render_report(
    claims: &[Claim],
    contradictions: &[Contradiction],
    tail: &ReportTail,
    buf: &mut String,
) {
    use std::collections::BTreeSet;
    let critics: BTreeSet<&str> = claims.iter().map(|c| c.critic.as_str()).collect();
    buf.push_str(&format!(
        "  {} claims from {} critics\n",
        claims.len(),
        critics.len()
    ));
    for kind in ["CONTRADICTED", "TESTABLE", "CONFIRMATORY"] {
        let n = claims.iter().filter(|c| c.kind == kind).count();
        buf.push_str(&format!("    {kind:<14}{n}\n"));
    }
    buf.push_str(&format!("    PROMOTED      {}\n", tail.promoted_count));
    buf.push_str(&format!(
        "    REFUSED-BY-VERIFICATION {}\n",
        tail.verification_refused
    ));
    if tail.unreadable_subject_count > 0 {
        buf.push_str(&format!(
            "    SUBJECTS-UNREADABLE {}\n",
            tail.unreadable_subject_count
        ));
    }
    for line in &tail.head_only {
        buf.push_str(line);
        buf.push('\n');
    }
    if !contradictions.is_empty() {
        buf.push_str(
            "\n  SPEC AMBIGUITY \u{2014} independent critics read the spec differently:\n",
        );
        let mut seen: Vec<(String, String, String)> = Vec::new();
        for c in contradictions {
            let key = (c.topic.clone(), c.a.clone(), c.b.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            buf.push_str(&format!("    topic '{}': {} vs {}\n", c.topic, c.a, c.b));
            buf.push_str(&format!("      {} expects: {}\n", c.a, c.a_expects));
            buf.push_str(&format!("      {} expects: {}\n", c.b, c.b_expects));
        }
    }
    buf.push_str(&format!("\n-> {}\n", tail.out.display()));
}

/// Turns critiques' CLAIMs into executed evidence. Returns the process exit code.
///
/// Reads `$HOME/.local/share/farmerbob/logs/critiques/<bead>/*.on.*.md`, classifies
/// each claim, writes `$HOME/.local/share/farmerbob/logs/<bead>.claims.json`, and
/// prints the counts other scripts grep for. Refuses (exit 3) to write an empty
/// artefact when no critiques were written; usage and I/O errors exit 1.
pub fn run_cmd(bead: &str) -> i32 {
    if bead.is_empty() {
        eprintln!("fb-promote: bead is required");
        return exit::USAGE;
    }
    let base = crate::paths::logs();
    let out = base.join(format!("{bead}.claims.json"));

    let review_dir = base.join("critiques").join(bead);
    match count_reviews(&review_dir) {
        Measurement::Missing(_) => {
            println!("NO critiques were written -- refusing to emit an empty claims file");
            return exit::NO_CRITIQUES;
        }
        Measurement::Observed(paths) if paths.is_empty() => {
            println!("NO critiques were written -- refusing to emit an empty claims file");
            return exit::NO_CRITIQUES;
        }
        Measurement::Observed(_) => {}
    }

    let mut claims = match load_claims(bead) {
        Measurement::Missing(absent) => {
            eprintln!("fb-promote: cannot read critiques: {absent:?}");
            return exit::ERROR;
        }
        Measurement::Observed(claims) => claims,
    };

    for c in &mut claims {
        c.kind = classify(&c.expect, &c.actual);
    }
    let contradictions = find_contradictions(&mut claims);

    let target = match target_for_task(bead) {
        Measurement::Observed(target) => target,
        Measurement::Missing(_) => String::new(),
    };
    let sources = Sources {
        worktrees: crate::paths::worktrees(),
        target,
    };
    let (promoted, rejections) = promote_verified(bead, &sources);
    let verification_refused = rejections
        .iter()
        .filter(|(_, rejection)| {
            matches!(
                rejection,
                Rejection::Fabricated { .. } | Rejection::Projected { .. }
            )
        })
        .count();
    let unreadable_subject_count = unreadable_subjects(bead, &sources, &claims);

    // Coverage judges the critics, never the claims: it runs after promotion is
    // already decided and cannot change a verdict, only add report lines.
    let coverage_map = critic_coverage(bead, &sources);
    let rows = coverage_rows(bead, &sources, &claims);
    let head_only = head_only_lines(&coverage_map, &rows);

    let body = render_json(&queued_claims(&claims, &promoted), &contradictions);
    if fs::write(&out, &body).is_err() {
        eprintln!("fb-promote: cannot write {}", out.display());
        return exit::ERROR;
    }

    // THIS STAGE'S OWN ARTEFACT, beside the shared one.
    //
    // `claims.json` is shared on purpose: fb-critique writes it, this stage rewrites it with
    // the classification added, and fb-prove and fb-escalate read it. What was missing is any
    // record that PROMOTE ran -- and fb-pipeline.sh declared one, `<bead>.promoted.json`, that
    // nothing ever created.
    //
    // The consequences ran for sixty-six tasks. run_stage's `[ -s "$artefact" ]` cache check
    // was false on every invocation, so promote re-ran and re-verified every claim every time;
    // and run_stage recorded the stage's signature on exit status alone, so all sixty-six
    // reported "promote: ok" with nothing behind them. A stage whose failure state was
    // indistinguishable from its success state, inside the pipeline's own bookkeeping. Counting
    // artefacts found it; reading the logs never would have.
    //
    // Writing this makes the declared artefact real and gives promote a cache key. A failure
    // here is NOT fatal: claims.json is the load-bearing output and is already on disk, so
    // losing the summary must not discard the verification work. It is reported, not swallowed.
    let promoted_path = base.join(format!("{bead}.promoted.json"));
    let summary = serde_json::json!({
        "bead": bead,
        "claims": claims.len(),
        "promoted": promoted.len(),
        "contradictions": contradictions.len(),
        "verification_refused": verification_refused,
        "unreadable_subjects": unreadable_subject_count,
    });
    match serde_json::to_string_pretty(&summary) {
        Ok(text) => {
            if fs::write(&promoted_path, text).is_err() {
                eprintln!(
                    "fb-promote: wrote {} but could not write {}",
                    out.display(),
                    promoted_path.display()
                );
            }
        }
        Err(e) => eprintln!("fb-promote: cannot render the stage summary: {e}"),
    }
    let mut report = String::new();
    render_report(
        &claims,
        &contradictions,
        &ReportTail {
            out: out.clone(),
            promoted_count: promoted.len(),
            verification_refused,
            unreadable_subject_count,
            head_only,
        },
        &mut report,
    );
    print!("{report}");
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_root(label: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "fb-promote-{label}-{}-{number}",
            std::process::id()
        ))
    }

    fn test_claim(critic: &str, subject: &str, trigger: &str, expect: &str, actual: &str) -> Claim {
        Claim {
            critic: critic.to_string(),
            subject: subject.to_string(),
            claim: String::from("test claim"),
            where_: String::from("test location"),
            trigger: trigger.to_string(),
            expect: expect.to_string(),
            actual: actual.to_string(),
            kind: String::new(),
        }
    }

    /// Worked example mirroring the script's two-claim demo: exact column widths
    /// (`{kind:<14}`) and exact literal words, pinned byte-for-byte.
    #[test]
    fn report_format_matches_the_script_byte_for_byte() {
        let claims = vec![
            Claim {
                critic: String::from("alpha"),
                subject: String::from("beta"),
                claim: String::from("budget totals drift"),
                where_: String::from("totals row"),
                trigger: String::from("add a row"),
                expect: String::from("totals include excluded arms"),
                actual: String::from("totals exclude them"),
                kind: String::from("TESTABLE"),
            },
            Claim {
                critic: String::from("alpha"),
                subject: String::from("beta"),
                claim: String::from("same thing already fine"),
                where_: String::from("header"),
                trigger: String::from("none"),
                expect: String::from("sorted by arm"),
                actual: String::from("same as expected"),
                kind: String::from("CONFIRMATORY"),
            },
        ];
        let contradictions = Vec::new();
        let out = Path::new("/tmp/fakehome/.local/share/farmerbob/logs/demo.claims.json");
        let mut buf = String::new();
        render_report(
            &claims,
            &contradictions,
            &ReportTail {
                out: out.to_path_buf(),
                promoted_count: 2,
                verification_refused: 0,
                unreadable_subject_count: 0,
                head_only: Vec::new(),
            },
            &mut buf,
        );
        let expected = "  2 claims from 1 critics\n".to_string()
            + "    CONTRADICTED  0\n"
            + "    TESTABLE      1\n"
            + "    CONFIRMATORY  1\n"
            + "    PROMOTED      2\n"
            + "    REFUSED-BY-VERIFICATION 0\n"
            + "\n"
            + "-> /tmp/fakehome/.local/share/farmerbob/logs/demo.claims.json\n";
        assert_eq!(buf, expected);
    }

    /// The contradiction section: blank line, em-dash header, deduplicated triples.
    #[test]
    fn ambiguity_section_matches_the_script_byte_for_byte() {
        let claims = vec![
            Claim {
                critic: String::from("alpha"),
                subject: String::from("thing"),
                claim: String::from("excluded rows fine"),
                where_: String::from("excluded summary table"),
                trigger: String::from("run it"),
                expect: String::from("they are included"),
                actual: String::from("excluded rows stay excluded"),
                kind: String::from("CONTRADICTED"),
            },
            Claim {
                critic: String::from("zeta"),
                subject: String::from("thing"),
                claim: String::from("excluded rows vanish"),
                where_: String::from("excluded ladder summary"),
                trigger: String::from("run it"),
                expect: String::from("excluded rows stay excluded"),
                actual: String::from("they are included"),
                kind: String::from("CONTRADICTED"),
            },
        ];
        let contradictions = vec![Contradiction {
            topic: String::from("excluded"),
            a: String::from("alpha"),
            b: String::from("zeta"),
            a_expects: String::from("they are included"),
            b_expects: String::from("excluded rows stay excluded"),
        }];
        let out = Path::new("/o/contra.claims.json");
        let mut buf = String::new();
        render_report(
            &claims,
            &contradictions,
            &ReportTail {
                out: out.to_path_buf(),
                promoted_count: 2,
                verification_refused: 0,
                unreadable_subject_count: 0,
                head_only: Vec::new(),
            },
            &mut buf,
        );
        let expected = "  2 claims from 2 critics\n".to_string()
            + "    CONTRADICTED  2\n"
            + "    TESTABLE      0\n"
            + "    CONFIRMATORY  0\n"
            + "    PROMOTED      2\n"
            + "    REFUSED-BY-VERIFICATION 0\n"
            + "\n"
            + "  SPEC AMBIGUITY \u{2014} independent critics read the spec differently:\n"
            + "    topic 'excluded': alpha vs zeta\n"
            + "      alpha expects: they are included\n"
            + "      zeta expects: excluded rows stay excluded\n"
            + "\n"
            + "-> /o/contra.claims.json\n";
        assert_eq!(buf, expected);
    }

    /// The artefact layout mirrors `json.dump(..., indent=1)`: one space per level,
    /// space after each colon, insertion-ordered keys.
    #[test]
    fn claims_json_layout_mirrors_indent_one() {
        let claims = vec![Claim {
            critic: String::from("a"),
            subject: String::from("s"),
            claim: String::from("c"),
            where_: String::from("w"),
            trigger: String::from("t"),
            expect: String::from("e"),
            actual: String::from("x"),
            kind: String::from("TESTABLE"),
        }];
        let body = render_json(&claims, &[]);
        let expected = "{\n \"claims\": [\n  {\n   \"critic\": \"a\",\n".to_string()
            + "   \"subject\": \"s\",\n"
            + "   \"claim\": \"c\",\n"
            + "   \"where\": \"w\",\n"
            + "   \"trigger\": \"t\",\n"
            + "   \"expect\": \"e\",\n"
            + "   \"actual\": \"x\",\n"
            + "   \"kind\": \"TESTABLE\"\n"
            + "  }\n"
            + " ],\n"
            + " \"contradictions\": []\n}";
        assert_eq!(body, expected);
    }

    #[test]
    fn empty_claims_still_render_empty_lists() {
        let body = render_json(&[], &[]);
        assert_eq!(body, "{\n \"claims\": [],\n \"contradictions\": []\n}");
    }

    #[test]
    fn confirmatory_synonyms_and_case_folding() {
        for actual in [
            "same",
            "SAME",
            "same as expected",
            "AS EXPECTED",
            "as-expected!",
        ] {
            assert_eq!(classify("something else", actual), "CONFIRMATORY");
        }
        assert_eq!(classify("Hello, World!", "hello world"), "CONFIRMATORY");
        assert_eq!(classify("totals stay", "totals go"), "TESTABLE");
        // Empty actual alleges nothing comparable, so it stays TESTABLE.
        assert_eq!(classify("something", ""), "TESTABLE");
        assert_eq!(classify("", "something"), "TESTABLE");
    }

    #[test]
    fn norm_folds_separators_and_case() {
        assert_eq!(norm("Hello, World!"), "hello world");
        assert_eq!(norm("hello_world"), "hello world");
        // Non-ASCII letters are separators, like Python's [^a-z0-9].
        assert_eq!(norm("caf\u{e9} x"), "caf x");
        assert_eq!(norm(""), "");
    }

    #[test]
    fn inline_actual_is_split_out_before_classification() {
        let claims = parse_review(
            "k",
            "j",
            "CLAIM: expect ACTUAL inline only\nWHERE: ladder-area\nTRIGGER: t\nEXPECT: wanted behaviour ACTUAL: seen defect\n",
        );
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].expect, "wanted behaviour");
        assert_eq!(claims[0].actual, "seen defect");
    }

    #[test]
    fn leading_parenthetical_is_dropped_once() {
        let claims = parse_review(
            "a",
            "b",
            "CLAIM: t\nWHERE: w\nTRIGGER: t\nEXPECT: (plain reading of section 2) totals stay\nACTUAL: x\n",
        );
        assert_eq!(claims[0].expect, "totals stay");
        // Only one group goes: "(a) (b) x" keeps "(b) x".
        let claims = parse_review(
            "a",
            "b",
            "CLAIM: t\nWHERE: w\nTRIGGER: t\nEXPECT: (a) (b) tail\nACTUAL: x\n",
        );
        assert_eq!(claims[0].expect, "(b) tail");
    }

    #[test]
    fn only_line_start_claim_markers_split() {
        let claims = parse_review(
            "e",
            "f",
            "CLAIM: indented claim\n  CLAIM: not a marker\nWHERE: w\nTRIGGER: t\nEXPECT: e\nACTUAL: a\n",
        );
        assert_eq!(claims.len(), 1);
        // Lowercase markers never split.
        let claims = parse_review("a", "b", "claim: lowercase\nWHERE: w\n");
        assert!(claims.is_empty());
    }

    #[test]
    fn field_terminators_match_the_script() {
        // A later field on its own line is found wherever it sits.
        let claims = parse_review(
            "a",
            "b",
            "CLAIM: t\nTRIGGER: t\nEXPECT: e\nACTUAL: a\nWHERE: after\n",
        );
        assert_eq!(claims[0].where_, "after");
        // Lowercase field names never open.
        let claims = parse_review(
            "c",
            "d",
            "CLAIM: upper\nWHERE: w\nTRIGGER: t\nEXPECT: e1\nactual: a1\n",
        );
        assert_eq!(claims[0].expect, "e1 actual: a1");
        assert_eq!(claims[0].actual, "");
    }

    #[test]
    fn topic_scan_uses_list_order_and_substrings() {
        assert_eq!(
            topic("the FAMILY budget is excluded-shadow", "w"),
            Some(String::from("excluded"))
        );
        assert_eq!(topic("nothing", "w"), None);
        // "ambiguousfamily" contains "family" as a substring, and `family`
        // precedes `ambiguous` in the script's list order, so it wins.
        assert_eq!(
            topic("ambiguousfamily tree", "w"),
            Some(String::from("family"))
        );
    }

    #[test]
    fn contradiction_needs_opposite_expectations() {
        let mut claims = vec![
            Claim {
                critic: String::from("alpha"),
                subject: String::from("thing"),
                claim: String::from("excluded rows fine"),
                where_: String::from("excluded summary table"),
                trigger: String::from("run it"),
                expect: String::from("they are included"),
                actual: String::from("excluded rows stay excluded"),
                kind: String::from("TESTABLE"),
            },
            Claim {
                critic: String::from("zeta"),
                subject: String::from("thing"),
                claim: String::from("excluded rows vanish"),
                where_: String::from("excluded ladder summary"),
                trigger: String::from("run it"),
                expect: String::from("excluded rows stay excluded"),
                actual: String::from("they are included"),
                kind: String::from("TESTABLE"),
            },
        ];
        let out = find_contradictions(&mut claims);
        assert_eq!(out.len(), 1);
        assert_eq!(claims[0].kind, "CONTRADICTED");
        assert_eq!(out[0].topic, "excluded");
        // Merely differing wording is not a contradiction.
        let mut claims = vec![
            Claim {
                critic: String::from("a"),
                subject: String::from("s"),
                claim: String::from("excluded one"),
                where_: String::from("excluded area"),
                trigger: String::from("t"),
                expect: String::from("P"),
                actual: String::from("Q"),
                kind: String::from("TESTABLE"),
            },
            Claim {
                critic: String::from("b"),
                subject: String::from("s"),
                claim: String::from("excluded two"),
                where_: String::from("excluded area"),
                trigger: String::from("t"),
                expect: String::from("R"),
                actual: String::from("S"),
                kind: String::from("TESTABLE"),
            },
        ];
        assert!(find_contradictions(&mut claims).is_empty());
        assert!(claims.iter().all(|c| c.kind == "TESTABLE"));
    }

    #[test]
    fn same_critic_pairs_never_contradict() {
        let mut claims = vec![
            Claim {
                critic: String::from("solo"),
                subject: String::from("s"),
                claim: String::from("excluded one"),
                where_: String::from("excluded a"),
                trigger: String::from("t"),
                expect: String::from("X-thing"),
                actual: String::from("Y-thing"),
                kind: String::from("TESTABLE"),
            },
            Claim {
                critic: String::from("solo"),
                subject: String::from("s"),
                claim: String::from("excluded two"),
                where_: String::from("excluded b"),
                trigger: String::from("t"),
                expect: String::from("Y-thing"),
                actual: String::from("X-thing"),
                kind: String::from("TESTABLE"),
            },
        ];
        assert!(find_contradictions(&mut claims).is_empty());
    }

    #[test]
    fn confirmatory_claims_are_invisible_to_contradiction() {
        // One side confirmatory (expect == actual after norm): skipped before pairing.
        let mut claims = vec![
            Claim {
                critic: String::from("y"),
                subject: String::from("s"),
                claim: String::from("excluded two"),
                where_: String::from("excluded area"),
                trigger: String::from("t"),
                expect: String::from("different"),
                actual: String::from("same"),
                kind: String::from("CONFIRMATORY"),
            },
            Claim {
                critic: String::from("z"),
                subject: String::from("s"),
                claim: String::from("excluded one"),
                where_: String::from("excluded area"),
                trigger: String::from("t"),
                expect: String::from("same"),
                actual: String::from("different"),
                kind: String::from("TESTABLE"),
            },
        ];
        assert!(find_contradictions(&mut claims).is_empty());
    }

    #[test]
    fn contradiction_scans_every_pair_not_just_the_first() {
        // `break` exits only the inner loop: a second contradicting pair later in
        // the same topic still marks and records.
        let mut claims = vec![
            Claim {
                critic: String::from("a"),
                subject: String::from("s"),
                claim: String::from("s1"),
                where_: String::from("budget hole"),
                trigger: String::from("t"),
                expect: String::from("E1"),
                actual: String::from("A1"),
                kind: String::from("TESTABLE"),
            },
            Claim {
                critic: String::from("b"),
                subject: String::from("s"),
                claim: String::from("s2"),
                where_: String::from("budget hole"),
                trigger: String::from("t"),
                expect: String::from("A1"),
                actual: String::from("E1"),
                kind: String::from("TESTABLE"),
            },
            Claim {
                critic: String::from("c"),
                subject: String::from("s"),
                claim: String::from("s3"),
                where_: String::from("budget hole"),
                trigger: String::from("t"),
                expect: String::from("E2"),
                actual: String::from("A2"),
                kind: String::from("TESTABLE"),
            },
            Claim {
                critic: String::from("d"),
                subject: String::from("s"),
                claim: String::from("s4"),
                where_: String::from("budget hole"),
                trigger: String::from("t"),
                expect: String::from("A2"),
                actual: String::from("E2"),
                kind: String::from("TESTABLE"),
            },
        ];
        let out = find_contradictions(&mut claims);
        assert_eq!(out.len(), 2);
        assert!(claims.iter().all(|c| c.kind == "CONTRADICTED"));
        assert_eq!(out[0].a, "a");
        assert_eq!(out[1].a, "c");
    }

    #[test]
    fn stem_split_rejects_ambiguous_dots() {
        assert_eq!(
            split_stem("a.on.b"),
            Some((String::from("a"), String::from("b")))
        );
        assert_eq!(
            split_stem("c.on.s.extra"),
            Some((String::from("c"), String::from("s.extra")))
        );
        assert_eq!(split_stem("nodot"), None);
    }

    #[test]
    fn hidden_dotfiles_are_not_reviews() {
        // `*.on.*.md` never matches a leading dot: `.on.y.md` is invisible.
        let dir = std::env::temp_dir().join("promote-dotfile-probe");
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(dir.join(".on.y.md"), "CLAIM: t\n");
        let m = count_reviews(&dir);
        let n = match &m {
            Measurement::Observed(paths) => paths.len(),
            Measurement::Missing(_) => 999,
        };
        assert_eq!(n, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_critiques_dir_is_missing_not_zero() {
        let m = count_reviews(Path::new("/nonexistent-promote-dir-xyz"));
        assert!(!m.is_observed());
        assert_eq!(m.value(), None);
    }

    #[test]
    fn json_escape_matches_python_ensure_ascii() {
        assert_eq!(json_escape("a/b"), "\"a/b\"");
        assert_eq!(json_escape("caf\u{e9} \u{2014}"), "\"caf\\u00e9 \\u2014\"");
        assert_eq!(json_escape("q\"b\\c"), "\"q\\\"b\\\\c\"");
        assert_eq!(json_escape("a\x01b"), "\"a\\u0001b\"");
        // Astral characters become UTF-16 surrogate pairs, like Python.
        assert_eq!(json_escape("\u{1d11e}"), "\"\\ud834\\udd1e\"");
        assert_eq!(json_escape("\u{7f}"), "\"\\u007f\"");
    }

    #[test]
    fn deliverable_read_distinguishes_empty_from_missing_and_empty_target() {
        let root = test_root("read");
        let file = root.join("task--arm").join("src/file.rs");
        fs::create_dir_all(file.parent().unwrap_or_else(|| Path::new(".")))
            .expect("test directory can be created");
        fs::write(&file, "").expect("test file can be written");
        let sources = Sources {
            worktrees: root.clone(),
            target: String::from("src/file.rs"),
        };
        assert_eq!(
            read_deliverable(&sources, "task", "arm"),
            Measurement::Observed(String::new())
        );
        assert!(matches!(
            read_deliverable(&sources, "task", "absent"),
            Measurement::Missing(Absent::InstrumentFailed { reason }) if !reason.is_empty()
        ));
        let empty_target = Sources {
            worktrees: root.clone(),
            target: String::new(),
        };
        assert!(matches!(
            read_deliverable(&empty_target, "task", "arm"),
            Measurement::Missing(Absent::InstrumentFailed { reason }) if !reason.is_empty()
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verification_keeps_subject_quotes_and_missing_subjects_but_refuses_other_quotes() {
        let root = test_root("verify");
        let subject = root.join("task--subject").join("target.rs");
        let critic = root.join("task--critic").join("target.rs");
        fs::create_dir_all(subject.parent().unwrap_or_else(|| Path::new(".")))
            .expect("test directory can be created");
        fs::create_dir_all(critic.parent().unwrap_or_else(|| Path::new(".")))
            .expect("test directory can be created");
        fs::write(&subject, "return \"kept\";").expect("subject can be written");
        fs::write(&critic, "return \"projected\";").expect("critic can be written");
        let sources = Sources {
            worktrees: root.clone(),
            target: String::from("target.rs"),
        };
        let mut claims = vec![
            test_claim(
                "critic",
                "subject",
                "kept-input",
                "expected-kept",
                "\"kept\"",
            ),
            test_claim(
                "critic",
                "subject",
                "fabricated-input",
                "expected-fabricated",
                "\"invented\"",
            ),
            test_claim(
                "critic",
                "subject",
                "projected-input",
                "expected-projected",
                "\"projected\"",
            ),
            test_claim(
                "critic",
                "missing-subject",
                "missing-input",
                "expected-missing",
                "\"not-readable\"",
            ),
            test_claim(
                "missing-critic",
                "subject",
                "missing-critic-input",
                "expected-critic",
                "\"critic-only\"",
            ),
        ];

        let (promoted, rejections) = promote_claims_verified("task", &sources, &mut claims);
        assert_eq!(promoted.len(), 2);
        assert_eq!(promoted[0].input, "kept-input");
        assert_eq!(promoted[1].input, "missing-input");
        assert_eq!(rejections.len(), 3);
        assert_eq!(rejections[0].0, 1);
        assert_eq!(rejections[1].0, 2);
        assert_eq!(rejections[2].0, 4);
        let fabricated_quote = match &rejections[0].1 {
            Rejection::Fabricated { quote } => quote,
            _ => "",
        };
        let projected_quote = match &rejections[1].1 {
            Rejection::Projected { quote } => quote,
            _ => "",
        };
        let unreadable_critic_quote = match &rejections[2].1 {
            Rejection::Fabricated { quote } => quote,
            _ => "",
        };
        assert_eq!(fabricated_quote, "invented");
        assert_eq!(projected_quote, "projected");
        assert_eq!(unreadable_critic_quote, "critic-only");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn core_rejections_precede_source_verification_and_keep_index_order() {
        let root = test_root("precedence");
        let subject = root.join("task--subject").join("target.rs");
        fs::create_dir_all(subject.parent().unwrap_or_else(|| Path::new(".")))
            .expect("test directory can be created");
        fs::write(&subject, "source").expect("subject can be written");
        let sources = Sources {
            worktrees: root.clone(),
            target: String::from("target.rs"),
        };
        let duplicate =
            |actual: &str| test_claim("critic", "subject", "same-input", "same-expect", actual);
        let mut claims = vec![
            test_claim("critic", "", "input", "expect", "\"invented\""),
            test_claim("critic", "subject", "", "expect", "\"invented\""),
            test_claim("critic", "subject", "input", "same", "same"),
            duplicate("\"invented\""),
            duplicate("\"also-invented\""),
        ];

        let (promoted, rejections) = promote_claims_verified("task", &sources, &mut claims);
        assert!(promoted.is_empty());
        assert_eq!(rejections.len(), 5);
        assert_eq!(rejections[0].0, 0);
        assert_eq!(rejections[1].0, 1);
        assert_eq!(rejections[2].0, 2);
        assert_eq!(rejections[3].0, 3);
        assert_eq!(rejections[4].0, 4);
        let first_incomplete = match &rejections[0].1 {
            Rejection::Incomplete { field } => field,
            _ => "",
        };
        let second_incomplete = match &rejections[1].1 {
            Rejection::Incomplete { field } => field,
            _ => "",
        };
        let fabricated_quote = match &rejections[3].1 {
            Rejection::Fabricated { quote } => quote,
            _ => "",
        };
        assert_eq!(first_incomplete, "target");
        assert_eq!(second_incomplete, "input");
        assert!(matches!(&rejections[2].1, Rejection::NotFalsifiable));
        assert_eq!(fabricated_quote, "invented");
        assert!(matches!(&rejections[4].1, Rejection::Duplicate));
        let _ = fs::remove_dir_all(root);
    }

    // ---- coverage wiring ----

    /// A claim whose WHERE field is the given text, with the other fields
    /// already promotable so the same claim can ride both pipelines.
    fn claim_at(critic: &str, subject: &str, where_: &str) -> Claim {
        Claim {
            critic: critic.to_string(),
            subject: subject.to_string(),
            claim: String::from("something is missing"),
            where_: where_.to_string(),
            trigger: String::from("run the tool"),
            expect: String::from("it is there"),
            actual: String::from("it is not"),
            kind: String::from("TESTABLE"),
        }
    }

    /// A subject long enough that "the head" exists under any line-counting
    /// rule: 500 lines, so 400-line and 25% thresholds hold with room to spare.
    fn wide_subject() -> String {
        (0..500).map(|i| format!("fn piece_{i}() {{}}\n")).collect()
    }

    fn write_arm(root: &Path, arm: &str, text: &str) {
        let path = root.join(format!("task--{arm}")).join("src/lib.rs");
        fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
            .expect("test worktree can be created");
        fs::write(&path, text).expect("test deliverable can be written");
    }

    fn coverage_sources(root: &Path) -> Sources {
        Sources {
            worktrees: root.to_path_buf(),
            target: String::from("src/lib.rs"),
        }
    }

    /// The (deepest cited, subject lines) pair behind a critic's HeadOnly
    /// verdict, or `None` when the critic is Unremarkable or unjudged.
    fn head_only_of(map: &BTreeMap<String, Coverage>, critic: &str) -> Option<(u32, u32)> {
        match map.get(critic) {
            Some(Coverage::HeadOnly {
                deepest_cited,
                subject_lines,
            }) => Some((*deepest_cited, *subject_lines)),
            _ => None,
        }
    }

    /// Every shape the doc comment promises, plus the two refusals the spec
    /// pins: zero is a real line, and a number outside a path:line position is
    /// not a citation at all.
    #[test]
    fn cited_line_reads_each_documented_shape() {
        assert_eq!(cited_line("crates/fb/src/doctor.rs:1000"), Some(1000));
        assert_eq!(cited_line("crates/fb/src/doctor.rs:1000:12"), Some(1000));
        assert_eq!(cited_line("crates/fb/src/promote.rs:~872"), Some(872));
        assert_eq!(
            cited_line("crates/farmerbob-core/src/x.rs:283-286"),
            Some(283)
        );
        assert_eq!(cited_line("ladder-area"), None);
        assert_eq!(cited_line("file.rs:0"), Some(0));
        assert_eq!(cited_line("the 300-second timeout"), None);
    }

    /// Coverage groups by CRITIC: two critics of one subject get their own
    /// verdicts, and the map iterates in sorted critic order.
    #[test]
    fn coverage_groups_by_critic_and_maps_are_sorted() {
        let root = test_root("coverage-groups");
        write_arm(&root, "big", &wide_subject());
        let sources = coverage_sources(&root);
        let claims = vec![
            claim_at("zulu", "big", "crates/fb/src/big.rs:3"),
            claim_at("zulu", "big", "crates/fb/src/big.rs:7"),
            claim_at("zulu", "big", "crates/fb/src/big.rs:12"),
            claim_at("alpha", "big", "crates/fb/src/big.rs:3"),
            claim_at("alpha", "big", "crates/fb/src/big.rs:240"),
            claim_at("alpha", "big", "crates/fb/src/big.rs:480"),
        ];
        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert_eq!(map.keys().collect::<Vec<_>>(), vec!["alpha", "zulu"]);
        // alpha's deepest citation is past a quarter of the file: unremarkable.
        assert_eq!(map["alpha"], Coverage::Unremarkable);
        // The deepest cited line decides zulu's verdict, and it is 12.
        assert!(matches!(head_only_of(&map, "zulu"), Some((12, _))));
        let _ = fs::remove_dir_all(root);
    }

    /// A critic filing claims against two different files is judged per
    /// subject and reported under its own name — once, never once per file.
    /// The two per-subject judgements disagree here, so only the grouping is
    /// pinned, not the folded direction.
    #[test]
    fn one_critic_two_subjects_reports_once_under_its_own_name() {
        let root = test_root("coverage-two-subjects");
        write_arm(&root, "wide", &wide_subject());
        write_arm(&root, "other", &wide_subject());
        let sources = coverage_sources(&root);
        let claims = vec![
            claim_at("d", "wide", "crates/w/src/wide.rs:3"),
            claim_at("d", "wide", "crates/w/src/wide.rs:7"),
            claim_at("d", "wide", "crates/w/src/wide.rs:12"),
            claim_at("d", "other", "crates/o/src/other.rs:3"),
            claim_at("d", "other", "crates/o/src/other.rs:240"),
            claim_at("d", "other", "crates/o/src/other.rs:480"),
        ];
        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("d"));
        let _ = fs::remove_dir_all(root);
    }

    /// When every per-subject judgement is head-only, the folded critic
    /// cannot read as unremarkable: that would hide the incident once per
    /// subject.
    #[test]
    fn head_only_on_every_subject_stays_head_only() {
        let root = test_root("coverage-fold-head");
        write_arm(&root, "wide", &wide_subject());
        write_arm(&root, "other", &wide_subject());
        let sources = coverage_sources(&root);
        let claims = vec![
            claim_at("d", "wide", "crates/w/src/wide.rs:3"),
            claim_at("d", "wide", "crates/w/src/wide.rs:7"),
            claim_at("d", "wide", "crates/w/src/wide.rs:12"),
            claim_at("d", "other", "crates/o/src/other.rs:3"),
            claim_at("d", "other", "crates/o/src/other.rs:7"),
            claim_at("d", "other", "crates/o/src/other.rs:12"),
        ];
        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert!(matches!(head_only_of(&map, "d"), Some((12, _))));
        let _ = fs::remove_dir_all(root);
    }

    /// Two unremarkable subjects fold to unremarkable: there is nothing to
    /// report.
    #[test]
    fn unremarkable_on_every_subject_stays_unremarkable() {
        let root = test_root("coverage-fold-calm");
        write_arm(&root, "wide", &wide_subject());
        write_arm(&root, "other", &wide_subject());
        let sources = coverage_sources(&root);
        let claims = vec![
            claim_at("d", "wide", "crates/w/src/wide.rs:3"),
            claim_at("d", "wide", "crates/w/src/wide.rs:240"),
            claim_at("d", "wide", "crates/w/src/wide.rs:480"),
            claim_at("d", "other", "crates/o/src/other.rs:3"),
            claim_at("d", "other", "crates/o/src/other.rs:240"),
            claim_at("d", "other", "crates/o/src/other.rs:480"),
        ];
        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert_eq!(map["d"], Coverage::Unremarkable);
        let _ = fs::remove_dir_all(root);
    }

    /// A critic whose claims carry no parseable WHERE at all yields
    /// `Unremarkable`, never `HeadOnly`: no citations is not evidence of a
    /// shallow read.
    #[test]
    fn unparseable_where_fields_are_unremarkable_never_head_only() {
        let root = test_root("coverage-unparsed");
        write_arm(&root, "big", &wide_subject());
        let sources = coverage_sources(&root);
        let claims = vec![
            claim_at("f", "big", "ladder-area"),
            claim_at("f", "big", "the 300-second timeout"),
            claim_at("f", "big", "totals row"),
            claim_at("f", "big", "header"),
        ];
        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert_eq!(map.len(), 1);
        assert_eq!(map["f"], Coverage::Unremarkable);
        let _ = fs::remove_dir_all(root);
    }

    /// A subject whose deliverable cannot be read yields `Unremarkable` for
    /// every critic of it: an unreadable subject has no line count, and a
    /// coverage judgement without one is a guess. An empty target and a
    /// missing worktree root are unreadable the same way.
    #[test]
    fn unreadable_subjects_are_unremarkable_for_every_critic() {
        let root = test_root("coverage-unreadable");
        let sources = coverage_sources(&root);
        let claims = vec![
            claim_at("g", "ghost", "crates/x/src/ghost.rs:3"),
            claim_at("g", "ghost", "crates/x/src/ghost.rs:7"),
            claim_at("g", "ghost", "crates/x/src/ghost.rs:12"),
        ];
        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert_eq!(map["g"], Coverage::Unremarkable);

        let empty_target = Sources {
            worktrees: root.clone(),
            target: String::new(),
        };
        let map = fold_rows(&coverage_rows("task", &empty_target, &claims));
        assert_eq!(map["g"], Coverage::Unremarkable);
        let _ = fs::remove_dir_all(root);
    }

    /// Coverage never changes which claims are promoted: the same claims run
    /// through promotion before and after a head-only judgement comes out
    /// identical, and the head-only critic's claims are all still promoted.
    #[test]
    fn coverage_never_changes_what_is_promoted() {
        let root = test_root("coverage-promotes");
        write_arm(&root, "big", &wide_subject());
        let sources = coverage_sources(&root);
        let mut claims = vec![
            claim_at("h", "big", "crates/fb/src/big.rs:3"),
            claim_at("h", "big", "crates/fb/src/big.rs:7"),
            claim_at("h", "big", "crates/fb/src/big.rs:12"),
        ];
        // Distinct inputs, so core promotion keeps all three instead of
        // refusing two as duplicates of the first.
        for (n, claim) in claims.iter_mut().enumerate() {
            claim.trigger = format!("input {n}");
        }
        let (first_promoted, first_rejections) =
            promote_claims_verified("task", &sources, &mut claims);
        assert_eq!(first_promoted.len(), 3);
        assert!(first_rejections.is_empty());

        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert!(matches!(head_only_of(&map, "h"), Some((12, _))));

        let (second_promoted, second_rejections) =
            promote_claims_verified("task", &sources, &mut claims);
        assert_eq!(first_promoted, second_promoted);
        assert_eq!(first_rejections, second_rejections);
        let _ = fs::remove_dir_all(root);
    }

    /// A cited line of zero rides along as an ordinary citation: the deepest
    /// of {0, 67, 74} is 74.
    #[test]
    fn a_cited_line_of_zero_is_an_ordinary_citation() {
        let root = test_root("coverage-zero");
        write_arm(&root, "big", &wide_subject());
        let sources = coverage_sources(&root);
        let claims = vec![
            claim_at("i", "big", "crates/fb/src/big.rs:0"),
            claim_at("i", "big", "crates/fb/src/big.rs:67"),
            claim_at("i", "big", "crates/fb/src/big.rs:74"),
        ];
        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert!(matches!(head_only_of(&map, "i"), Some((74, _))));
        let _ = fs::remove_dir_all(root);
    }

    /// One critic, one claim: one citation cannot establish a pattern.
    #[test]
    fn one_claim_is_unremarkable() {
        let root = test_root("coverage-one-claim");
        write_arm(&root, "big", &wide_subject());
        let sources = coverage_sources(&root);
        let claims = vec![claim_at("j", "big", "crates/fb/src/big.rs:12")];
        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert_eq!(map.len(), 1);
        assert_eq!(map["j"], Coverage::Unremarkable);
        let _ = fs::remove_dir_all(root);
    }

    /// A critic that is also a subject is judged in both roles independently:
    /// gamma's shallow read of delta is reported even though delta's read of
    /// tiny gamma is not, and neither judgement feeds back into the other.
    #[test]
    fn a_critic_that_is_also_a_subject_is_judged_in_both_roles() {
        let root = test_root("coverage-both-roles");
        write_arm(&root, "delta", &wide_subject());
        write_arm(&root, "gamma", "fn tiny() {}\nfn small() {}\n");
        let sources = coverage_sources(&root);
        let claims = vec![
            claim_at("gamma", "delta", "crates/d/src/delta.rs:3"),
            claim_at("gamma", "delta", "crates/d/src/delta.rs:7"),
            claim_at("gamma", "delta", "crates/d/src/delta.rs:12"),
            claim_at("delta", "gamma", "crates/g/src/gamma.rs:1"),
            claim_at("delta", "gamma", "crates/g/src/gamma.rs:2"),
        ];
        let map = fold_rows(&coverage_rows("task", &sources, &claims));
        assert_eq!(map.len(), 2);
        assert!(matches!(head_only_of(&map, "gamma"), Some((12, _))));
        assert_eq!(map["delta"], Coverage::Unremarkable);
        let _ = fs::remove_dir_all(root);
    }

    /// A task with no critiques folds to an EMPTY map: no critic was judged,
    /// which is NOT the fact "no critic was shallow".
    #[test]
    fn no_claims_is_an_empty_map_not_a_clean_bill() {
        let root = test_root("coverage-empty");
        let map = fold_rows(&coverage_rows("task", &coverage_sources(&root), &[]));
        assert!(map.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    /// The printed line names the critic, the subject, the deepest cited line
    /// and the subject's length, so a reader can judge the call without
    /// re-deriving it — and an unremarkable critic is printed nowhere.
    #[test]
    fn head_only_lines_name_critic_subject_depth_and_length() {
        let root = test_root("coverage-print");
        write_arm(&root, "wide", &wide_subject());
        let sources = coverage_sources(&root);
        let claims = vec![
            claim_at("critic-h", "wide", "crates/w/src/wide.rs:3"),
            claim_at("critic-h", "wide", "crates/w/src/wide.rs:7"),
            claim_at("critic-h", "wide", "crates/w/src/wide.rs:12"),
            claim_at("critic-calm", "wide", "crates/w/src/wide.rs:3"),
            claim_at("critic-calm", "wide", "crates/w/src/wide.rs:240"),
            claim_at("critic-calm", "wide", "crates/w/src/wide.rs:480"),
        ];
        let rows = coverage_rows("task", &sources, &claims);
        let map = fold_rows(&rows);
        let lines = head_only_lines(&map, &rows);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line.contains("critic-h"), "names the critic: {line}");
        assert!(line.contains("wide"), "names the subject: {line}");
        assert!(line.contains("12"), "names the deepest cited line: {line}");
        if let Some((_, subject_lines)) = head_only_of(&map, "critic-h") {
            assert!(
                line.contains(&subject_lines.to_string()),
                "names the subject's length: {line}"
            );
        }
        assert!(!lines.iter().any(|line| line.contains("critic-calm")));
        let _ = fs::remove_dir_all(root);
    }

    /// The report carries the head-only lines; an empty set adds nothing,
    /// which the byte-for-byte report tests above pin in full.
    #[test]
    fn render_report_carries_head_only_lines() {
        let tail = ReportTail {
            out: PathBuf::from("/o/x.claims.json"),
            promoted_count: 0,
            verification_refused: 0,
            unreadable_subject_count: 0,
            head_only: vec![String::from(
                "    HEAD-ONLY k on s: deepest cited line 3 of 400 lines",
            )],
        };
        let mut buf = String::new();
        render_report(&[], &[], &tail, &mut buf);
        assert!(buf.contains("HEAD-ONLY k on s"));
        assert!(buf.ends_with("-> /o/x.claims.json\n"));
    }
}











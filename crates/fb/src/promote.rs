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

use std::fs;
use std::path::{Path, PathBuf};

use farmerbob_core::measurement::{Absent, Measurement};

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

/// Renders the stdout report byte-for-byte like the script's final prints.
///
/// `  {n} claims from {critics} critics`, one `    {KIND:<14}{count}` line for
/// CONTRADICTED, TESTABLE and CONFIRMATORY in that order, then — only when there are
/// contradictions — a blank line, `  SPEC AMBIGUITY — independent critics read the spec
/// differently:`, and one deduplicated triple per `(topic, a, b)` in first-seen order,
/// then a blank line and `-> {out}`. `–` is U+2014, encoded UTF-8.
fn render_report(claims: &[Claim], contradictions: &[Contradiction], out: &Path, buf: &mut String) {
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
    buf.push_str(&format!("\n-> {}\n", out.display()));
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
    let home = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => home,
        _ => {
            eprintln!("fb-promote: HOME is not set");
            return exit::ERROR;
        }
    };
    let base = PathBuf::from(home).join(".local/share/farmerbob/logs");
    let dir = base.join("critiques").join(bead);
    let out = base.join(format!("{bead}.claims.json"));

    let reviews = count_reviews(&dir);
    if !reviews.is_observed() {
        println!("NO critiques were written -- refusing to emit an empty claims file");
        return exit::NO_CRITIQUES;
    }
    let review_paths: Vec<PathBuf> = match &reviews {
        Measurement::Observed(paths) => paths.clone(),
        Measurement::Missing(_) => Vec::new(),
    };
    if review_paths.is_empty() {
        println!("NO critiques were written -- refusing to emit an empty claims file");
        return exit::NO_CRITIQUES;
    }

    let mut claims: Vec<Claim> = Vec::new();
    for path in &review_paths {
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        let stem = match name.strip_suffix(".md") {
            Some(stem) => stem,
            None => continue,
        };
        let Some((critic, subject)) = split_stem(stem) else {
            continue;
        };
        let Some(text) = read_review(path) else {
            eprintln!("fb-promote: cannot read {}", path.display());
            return exit::ERROR;
        };
        claims.extend(parse_review(&critic, &subject, &text));
    }
    for c in &mut claims {
        c.kind = classify(&c.expect, &c.actual);
    }
    let contradictions = find_contradictions(&mut claims);

    let body = render_json(&claims, &contradictions);
    if fs::write(&out, &body).is_err() {
        eprintln!("fb-promote: cannot write {}", out.display());
        return exit::ERROR;
    }
    let mut report = String::new();
    render_report(&claims, &contradictions, &out, &mut report);
    print!("{report}");
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

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
        render_report(&claims, &contradictions, out, &mut buf);
        let expected = "  2 claims from 1 critics\n".to_string()
            + "    CONTRADICTED  0\n"
            + "    TESTABLE      1\n"
            + "    CONFIRMATORY  1\n"
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
        render_report(&claims, &contradictions, out, &mut buf);
        let expected = "  2 claims from 2 critics\n".to_string()
            + "    CONTRADICTED  2\n"
            + "    TESTABLE      0\n"
            + "    CONFIRMATORY  0\n"
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
}

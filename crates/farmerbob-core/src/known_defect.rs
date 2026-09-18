//! The known-defect record: confirmed findings that could not be escalated.
//!
//! When a confirmed finding's test fails against the merged code it cannot
//! join the suite, so it is recorded in `.fb/known-defects/<task>.md`
//! instead of being discarded. This module renders one entry for that file
//! and reads the entries already in it, so a caller can tell a new finding
//! from one already recorded. It performs no I/O and keeps no clock: the
//! file arrives as a `&str` and the timestamp as an argument.
//!
//! An entry is a `## ` heading of the exact shape
//! `## <stamp> -- <critic> on <subject>`, then the claim on its own line as
//! `<critic> on <subject>: <claim>`, then one `<LABEL>: <text>` line per
//! piece of evidence. [`render`] and [`parse`] are only correct with
//! respect to each other and are tested together: a matched pair of bugs,
//! one in each direction, would pass two independent one-way tests.

#![forbid(unsafe_code)]

/// The evidence labels the critique contract fixes. The set is closed --
/// exactly these four, in this canonical order. `parse` gathers evidence in
/// this order and ignores a line carrying any other label rather than
/// guessing, because a mis-parsed label would attribute one field's text to
/// another.
const EVIDENCE_LABELS: [&str; 4] = ["WHERE", "TRIGGER", "EXPECT", "ACTUAL"];

/// The indent [`render`] puts on a continuation line: every chunk after the
/// first of a text that carried an embedded newline. Headings and evidence
/// labels are recognised only at column zero, so a `## ` or a `TRIGGER: `
/// inside a claim lands indented and cannot be read back as the start of an
/// entry or as evidence.
const CONTINUATION_PREFIX: &str = "  ";

/// One confirmed finding that could not be escalated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownDefect {
    /// The arm that found it.
    pub critic: String,
    /// The arm it is about -- the one that was merged.
    pub subject: String,
    /// The claim's one-line statement, as the critic wrote it.
    pub claim: String,
    /// The critic's own WHERE / TRIGGER / EXPECT / ACTUAL lines, in that
    /// order, each without its label. A line the critic did not write is
    /// absent from the vector rather than present and empty.
    pub evidence: Vec<String>,
}

/// The prefix of the claim line: who found it, on which arm.
fn claim_prefix(critic: &str, subject: &str) -> String {
    format!("{critic} on {subject}: ")
}

/// Write `text` onto the line already open in `out`: the first chunk
/// continues that line, and every further chunk of an embedded newline
/// becomes its own line under [`CONTINUATION_PREFIX`], so nothing inside
/// `text` can be mistaken for entry structure.
fn push_multiline(out: &mut String, text: &str) {
    let mut chunks = text.split('\n');
    if let Some(first) = chunks.next() {
        out.push_str(first);
    }
    out.push('\n');
    for chunk in chunks {
        out.push_str(CONTINUATION_PREFIX);
        out.push_str(chunk);
        out.push('\n');
    }
}

/// Render one entry for `.fb/known-defects/<task>.md`.
///
/// The entry is a `## ` heading carrying the stamp, the critic and the
/// subject in the shape `## <stamp> -- <critic> on <subject>`, then the
/// claim, then each evidence line as `<LABEL>: <text>`. Every line is
/// newline-terminated, so entries concatenate into a file without extra
/// separators. `stamp` is the caller's timestamp; this module has no clock.
///
/// `task` names the file the entry is destined for; the pinned entry shape
/// gives it no line of its own, so nothing in the output depends on it.
///
/// `evidence` is positional: element `i` renders under the `i`th label of
/// WHERE, TRIGGER, EXPECT, ACTUAL. Only the first four elements have a
/// label to carry, so anything beyond the closed set of four is not
/// rendered.
pub fn render(task: &str, stamp: &str, d: &KnownDefect) -> String {
    let _ = task;
    let mut out = String::new();
    out.push_str("## ");
    out.push_str(stamp);
    out.push_str(" -- ");
    out.push_str(&d.critic);
    out.push_str(" on ");
    out.push_str(&d.subject);
    out.push('\n');
    out.push_str(&claim_prefix(&d.critic, &d.subject));
    push_multiline(&mut out, &d.claim);
    for (label, text) in EVIDENCE_LABELS.iter().zip(d.evidence.iter()) {
        out.push_str(label);
        out.push_str(": ");
        push_multiline(&mut out, text);
    }
    out
}

/// Where an indented continuation line of an entry goes next.
enum Target {
    /// Onto the claim.
    Claim,
    /// Onto the evidence slot for the `i`th canonical label.
    Evidence(usize),
    /// Nowhere yet; the entry has no claim line and no evidence so far.
    Nothing,
}

/// One `## `-headed region of the file while it is being read.
struct Entry {
    critic: String,
    subject: String,
    /// The claim so far, grown by continuation lines with embedded `\n`.
    claim: String,
    /// Per-label evidence text; `None` for a label never written.
    evidence: [Option<String>; 4],
    /// Where the next indented continuation line goes.
    writing: Target,
    /// `<critic> on <subject>: `, the marker of the claim line.
    prefix: String,
}

impl Entry {
    /// Start an entry from the text after `## ` on the heading line.
    ///
    /// The heading reads `<stamp> -- <critic> on <subject>`. The stamp is
    /// everything before the first ` -- ` -- a separator an RFC3339 stamp
    /// cannot carry, so a stamp that carries `--` still parses. Critic and
    /// subject split at the last ` on `.
    fn heading(head: &str) -> Self {
        let rest = match head.split_once(" -- ") {
            Some((_, rest)) => rest,
            None => head,
        };
        let (critic, subject) = match rest.rsplit_once(" on ") {
            Some((critic, subject)) => (critic.to_string(), subject.to_string()),
            None => (rest.to_string(), String::new()),
        };
        let prefix = claim_prefix(&critic, &subject);
        Entry {
            critic,
            subject,
            claim: String::new(),
            evidence: [None, None, None, None],
            writing: Target::Nothing,
            prefix,
        }
    }

    /// Absorb one body line of this entry.
    ///
    /// The claim line opens the claim; a `<LABEL>:` line opens evidence for
    /// that label; an indented continuation line appends to whatever opened
    /// last. Any other line -- blank, prose, or a label outside the closed
    /// set -- is ignored.
    fn absorb(&mut self, line: &str) {
        if let Some(first) = line.strip_prefix(self.prefix.as_str()) {
            self.claim = first.to_string();
            self.writing = Target::Claim;
            return;
        }
        for (slot, label) in EVIDENCE_LABELS.iter().enumerate() {
            if let Some(text) = line
                .strip_prefix(*label)
                .and_then(|after| after.strip_prefix(':'))
            {
                let text = text.strip_prefix(' ').unwrap_or(text);
                self.evidence[slot] = Some(text.to_string());
                self.writing = Target::Evidence(slot);
                return;
            }
        }
        if let Some(rest) = line.strip_prefix(CONTINUATION_PREFIX) {
            match &mut self.writing {
                Target::Claim => {
                    self.claim.push('\n');
                    self.claim.push_str(rest);
                }
                Target::Evidence(slot) => {
                    if let Some(Some(text)) = self.evidence.get_mut(*slot) {
                        text.push('\n');
                        text.push_str(rest);
                    }
                }
                Target::Nothing => {}
            }
        }
    }

    /// The finished finding: evidence only for labels actually written, in
    /// the fixed WHERE / TRIGGER / EXPECT / ACTUAL order, never padded.
    fn finish(self) -> KnownDefect {
        KnownDefect {
            critic: self.critic,
            subject: self.subject,
            claim: self.claim,
            evidence: self.evidence.iter().flatten().cloned().collect(),
        }
    }
}

/// Parse the entries already in such a file, so a caller can tell a new
/// finding from one already recorded.
///
/// An entry begins at a line starting with `## ` and runs to the next such
/// line or the end of the file; entries come back in file order. A file
/// with no `## ` heading -- including an empty one -- has no entries and
/// yields an empty vector rather than an error. Within an entry, the claim
/// is the text after the first `<critic> on <subject>: ` line, continued
/// across [`render`]'s indented continuation lines; evidence is the
/// `WHERE:` / `TRIGGER:` / `EXPECT:` / `ACTUAL:` lines, gathered in that
/// fixed order, only for labels the critic actually wrote. Any other label,
/// and any other unprefixed line, is ignored.
pub fn parse(file: &str) -> Vec<KnownDefect> {
    let mut entries = Vec::new();
    let mut current: Option<Entry> = None;
    for line in file.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            if let Some(entry) = current.take() {
                entries.push(entry.finish());
            }
            current = Some(Entry::heading(head));
        } else if let Some(entry) = current.as_mut() {
            entry.absorb(line);
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry.finish());
    }
    entries
}

/// Whether this defect is already present in `existing`.
///
/// Two entries are the same when critic, subject and claim all match
/// exactly. The evidence is not compared: a re-run that gathers more of it
/// is the same finding, and treating it as new would fill the file with
/// duplicates.
pub fn already_recorded(existing: &[KnownDefect], d: &KnownDefect) -> bool {
    existing
        .iter()
        .any(|e| e.critic == d.critic && e.subject == d.subject && e.claim == d.claim)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: &str = "2026-09-18T11:08:04-07:00";

    fn defect(claim: &str, evidence: &[&str]) -> KnownDefect {
        KnownDefect {
            critic: "or-luna-pro".to_string(),
            subject: "or-muse-spark".to_string(),
            claim: claim.to_string(),
            evidence: evidence.iter().map(|e| e.to_string()).collect(),
        }
    }

    fn rendered(d: &KnownDefect) -> String {
        render("wave85", STAMP, d)
    }

    #[test]
    fn heading_carries_stamp_critic_subject_in_pinned_shape() {
        let d = defect(
            "FILE-AS-KNOWN-DEFECT -- test fails against merged code",
            &[],
        );
        let text = rendered(&d);
        let first = text.lines().next().unwrap_or_default();
        assert_eq!(
            first,
            "## 2026-09-18T11:08:04-07:00 -- or-luna-pro on or-muse-spark"
        );
    }

    #[test]
    fn claim_line_follows_the_heading_in_pinned_shape() {
        let d = defect(
            "FILE-AS-KNOWN-DEFECT -- test fails against merged code",
            &[],
        );
        let text = rendered(&d);
        let second = text.lines().nth(1).unwrap_or_default();
        assert_eq!(
            second,
            "or-luna-pro on or-muse-spark: FILE-AS-KNOWN-DEFECT -- test fails against merged code"
        );
    }

    #[test]
    fn evidence_lines_carry_their_labels_in_canonical_order() {
        let d = defect("claim", &["crates/x/src/a.rs:42", "cargo test -p x"]);
        let text = rendered(&d);
        let lines: Vec<&str> = text.lines().collect();
        let where_at = lines
            .iter()
            .position(|l| *l == "WHERE: crates/x/src/a.rs:42");
        let trigger_at = lines.iter().position(|l| *l == "TRIGGER: cargo test -p x");
        assert!(where_at.is_some(), "first evidence renders under WHERE");
        assert!(
            trigger_at.is_some(),
            "second evidence renders under TRIGGER"
        );
        assert!(where_at.unwrap_or(0) < trigger_at.unwrap_or(1));
        assert!(!text.contains("EXPECT:"));
        assert!(!text.contains("ACTUAL:"));
    }

    #[test]
    fn round_trip_with_full_evidence() {
        let d = defect(
            "the arm sorts `both` arms before it merges",
            &[
                "crates/x/src/a.rs:42",
                "cargo test -p x -- sort",
                "returns Ok",
                "returns Err(Nope)",
            ],
        );
        assert_eq!(parse(&rendered(&d)), vec![d]);
    }

    #[test]
    fn round_trip_without_evidence_keeps_it_empty() {
        let d = defect("test fails against merged code", &[]);
        let parsed = parse(&rendered(&d));
        assert_eq!(parsed, vec![d]);
        assert_eq!(parsed.first().map(|p| p.evidence.len()), Some(0));
    }

    #[test]
    fn empty_evidence_renders_no_section_at_all() {
        let d = defect("a plain claim", &[]);
        let text = rendered(&d);
        for label in EVIDENCE_LABELS {
            assert!(!text.contains(label), "no {label} line without evidence");
        }
    }

    #[test]
    fn partial_evidence_round_trips_and_is_never_padded() {
        let d = defect("claim", &["what ran", "what came out"]);
        let parsed = parse(&rendered(&d));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].evidence.len(), 2);
        assert_eq!(parsed, vec![d]);
    }

    #[test]
    fn parse_gathers_evidence_in_canonical_order_not_file_order() {
        let file = r#"## 2026-09-18T11:08:04-07:00 -- or-luna-pro on or-muse-spark
or-luna-pro on or-muse-spark: claim
TRIGGER: trigger text
WHERE: where text
"#;
        let parsed = parse(file);
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].evidence,
            vec!["where text".to_string(), "trigger text".to_string()]
        );
    }

    #[test]
    fn parse_ignores_a_label_outside_the_closed_set() {
        let file = r#"## 2026-09-18T11:08:04-07:00 -- or-luna-pro on or-muse-spark
or-luna-pro on or-muse-spark: claim
NOTE: an aside the critic also wrote
WHERE: where text
"#;
        let parsed = parse(file);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].evidence, vec!["where text".to_string()]);
    }

    #[test]
    fn parse_returns_several_entries_in_file_order() {
        let mut entries = Vec::new();
        for (critic, subject, claim) in [
            ("or-luna-pro", "or-muse-spark", "first finding"),
            ("or-vesta-2", "or-atlas-9", "second finding"),
            ("or-nova-1", "or-echo-4", "third finding"),
        ] {
            entries.push(KnownDefect {
                critic: critic.to_string(),
                subject: subject.to_string(),
                claim: claim.to_string(),
                evidence: Vec::new(),
            });
        }
        let file: String = entries.iter().map(rendered).collect();
        let parsed = parse(&file);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed, entries);
    }

    #[test]
    fn file_without_headings_has_no_entries() {
        let file = "some prose\nthat is not the format\nat all\n";
        assert!(parse(file).is_empty());
    }

    #[test]
    fn empty_file_has_no_entries() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn legacy_prose_paragraph_is_not_part_of_the_claim() {
        let file = r#"## 2026-09-18T11:08:04-07:00 -- or-luna-pro on or-muse-spark
or-luna-pro on or-muse-spark: FILE-AS-KNOWN-DEFECT -- test fails against merged code

The escalated test failed against the merged reference, so it was not
added to the suite. The finding was confirmed and is recorded here so
it survives the retraction.
"#;
        let parsed = parse(file);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].critic, "or-luna-pro");
        assert_eq!(parsed[0].subject, "or-muse-spark");
        assert_eq!(
            parsed[0].claim,
            "FILE-AS-KNOWN-DEFECT -- test fails against merged code"
        );
        assert!(parsed[0].evidence.is_empty());
    }

    #[test]
    fn claim_with_backticks_and_quotes_round_trips() {
        let d = defect("uses `sort_unstable` on \"both\" arms", &[]);
        assert_eq!(parse(&rendered(&d)), vec![d]);
    }

    #[test]
    fn embedded_heading_stays_inside_the_claim() {
        let d = defect("line one\n## Rogue Heading\nline three", &[]);
        let parsed = parse(&rendered(&d));
        assert_eq!(
            parsed.len(),
            1,
            "a ## inside the claim must not open a second entry"
        );
        assert_eq!(parsed, vec![d]);
    }

    #[test]
    fn embedded_label_stays_inside_the_claim() {
        let d = defect("line one\nTRIGGER: forged evidence\nline three", &[]);
        let parsed = parse(&rendered(&d));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed, vec![d], "the forged label is claim text");
        assert!(parsed[0].evidence.is_empty());
    }

    #[test]
    fn stamp_carrying_a_double_dash_still_parses() {
        let d = defect("claim", &["evidence"]);
        let text = render("wave85", "2026-09-18T11:08:04-07:00--rerun2", &d);
        let parsed = parse(&text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].critic, "or-luna-pro");
        assert_eq!(parsed[0].subject, "or-muse-spark");
        assert_eq!(parsed[0].claim, "claim");
    }

    #[test]
    fn empty_claim_is_degenerate_but_round_trips() {
        let d = defect("", &[]);
        let parsed = parse(&rendered(&d));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed, vec![d]);
        assert_eq!(parsed[0].claim, "");
    }

    #[test]
    fn already_recorded_matches_exactly_and_ignores_evidence() {
        let recorded = defect("same claim", &["old where"]);
        let candidate = defect(
            "same claim",
            &["new where", "new trigger", "new expect", "new actual"],
        );
        assert!(already_recorded(&[recorded], &candidate));
    }

    #[test]
    fn differing_claims_are_distinct_findings_and_both_get_recorded() {
        let a = defect("claim one", &[]);
        let b = defect("claim two", &[]);
        assert!(!already_recorded(std::slice::from_ref(&a), &b));
        let file = format!("{}{}", rendered(&a), rendered(&b));
        let parsed = parse(&file);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].claim, "claim one");
        assert_eq!(parsed[1].claim, "claim two");
    }

    #[test]
    fn already_recorded_against_an_empty_slice_is_false() {
        assert!(!already_recorded(&[], &defect("claim", &[])));
    }
}

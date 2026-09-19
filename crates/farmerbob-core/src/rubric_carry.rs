/// How a spec stands against the canonical rubric.
pub enum Carry {
    /// The spec ends with the canonical rubric, byte for byte after normalisation.
    Current,
    /// The spec carries an OLDER rubric: every line it has is a line the canonical
    /// rubric still has, in order, but the canonical rubric has lines it does not.
    /// Carries the canonical lines that are missing, in canonical order.
    Stale(Vec<String>),
    /// The spec carries text where the rubric belongs, and it is not a prefix of
    /// the canonical rubric. Re-appending will not fix it.
    Foreign,
    /// The spec carries no recognisable rubric at all.
    Absent,
}

fn normalize(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.trim_end().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Compare one spec against the canonical rubric.
///
/// `anchor` is a line that begins the rubric in both documents; text before the first
/// occurrence of `anchor` in `spec` is the spec's own body and is not compared.
pub fn carry(spec: &str, rubric: &str, anchor: &str) -> Carry {
    if anchor.is_empty() {
        // Empty anchor is not pinned by the spec; return Absent as the safe default.
        return Carry::Absent;
    }

    let rubric_norm = normalize(rubric);

    // Find first line in spec that equals the anchor exactly.
    let spec_lines: Vec<&str> = spec.lines().collect();
    let anchor_idx = spec_lines.iter().position(|&line| line == anchor);

    match anchor_idx {
        None => Carry::Absent,
        Some(idx) => {
            // Empty canonical rubric: every spec containing the anchor is Current.
            if rubric_norm.is_empty() {
                return Carry::Current;
            }

            let spec_portion_str = spec_lines[idx..].join("\n");
            let spec_norm = normalize(&spec_portion_str);

            if spec_norm == rubric_norm {
                Carry::Current
            } else if rubric_norm.len() >= spec_norm.len() {
                let is_prefix = spec_norm.iter().enumerate().all(|(i, line)| {
                    rubric_norm.get(i) == Some(line)
                });
                if is_prefix {
                    let missing = rubric_norm[spec_norm.len()..].to_vec();
                    Carry::Stale(missing)
                } else {
                    Carry::Foreign
                }
            } else {
                // Spec portion is longer than rubric: check if rubric is a prefix of spec.
                // If the rubric is a prefix of the spec, the spec has extra lines.
                // The spec carries MORE lines than the rubric has. Whether or not the
                // rubric is a prefix of it, re-appending cannot fix it: there is text
                // where the rubric belongs that the rubric does not account for.
                Carry::Foreign
            }
        }
    }
}

/// The lines of `rubric` that `spec` does not carry, in canonical order.
///
/// Empty when the spec is `Current`. Defined for every `Carry`.
pub fn missing_lines(spec: &str, rubric: &str, anchor: &str) -> Vec<String> {
    match carry(spec, rubric, anchor) {
        Carry::Current => Vec::new(),
        Carry::Stale(lines) => lines,
        Carry::Foreign => Vec::new(),
        Carry::Absent => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Clause 1: spec text after anchor equals rubric -> Current.
    #[test]
    fn clause_1_current_exact() {
        assert!(matches!(carry("body\nrubric\nline1", "rubric\nline1", "rubric"), Carry::Current));
    }

    // Clause 2: normalisation (trailing spaces, blank lines) -> Current.
    #[test]
    fn clause_2_normalisation_trailing_spaces() {
        assert!(matches!(
            carry("body\nrubric\nline1 ", "rubric\nline1", "rubric"),
            Carry::Current
        ));
    }

    #[test]
    fn clause_2_normalisation_blank_lines_inserted() {
        assert!(matches!(
            carry("body\nrubric\nline1\n\nline2", "rubric\nline1\nline2", "rubric"),
            Carry::Current
        ));
    }

    // Clause 3: first K lines of rubric -> Stale with canonical missing lines.
    #[test]
    fn clause_3_stale_first_k_lines() {
        let result = carry("body\nrubric\nline1", "rubric\nline1\nline2\nline3", "rubric");
        assert!(matches!(result, Carry::Stale(ref lines) if lines == &["line2", "line3"]));
    }

    // Clause 4: changed line in middle -> Foreign (not Stale).
    #[test]
    fn clause_4_changed_line_is_foreign() {
        let result = carry("body\nrubric\nline1\nlineX\nline2", "rubric\nline1\nline2", "rubric");
        assert!(matches!(result, Carry::Foreign));
    }

    // Clause 5: anchor followed by unrelated prose -> Foreign.
    #[test]
    fn clause_5_unrelated_prose_foreign() {
        let result = carry("body\nrubric\nunrelated prose", "rubric\nline1", "rubric");
        assert!(matches!(result, Carry::Foreign));
    }

    // Clause 6: spec not containing anchor -> Absent.
    #[test]
    fn clause_6_no_anchor_absent() {
        assert!(matches!(carry("body only", "rubric\nline1", "rubric"), Carry::Absent));
    }

    // Clause 7: missing_lines agrees with carry.
    #[test]
    fn clause_7_missing_lines_agreement_current() {
        assert!(missing_lines("body\nrubric\nline1", "rubric\nline1", "rubric").is_empty());
    }

    #[test]
    fn clause_7_missing_lines_agreement_stale() {
        assert_eq!(
            missing_lines("body\nrubric\nline1", "rubric\nline1\nline2", "rubric"),
            vec!["line2"]
        );
    }

    // Clause 8: first occurrence of anchor. Spec quotes anchor in body, real rubric below.
    // The comparison begins at the quoted line, so body text between occurrences is treated as rubric.
    #[test]
    fn clause_8_first_anchor_quoted_body() {
        // The body contains "rubric" as a quote; the real rubric starts at the second occurrence.
        // But with first-occurrence rule, comparison starts at the first "rubric" line.
        // The text between the two occurrences ("fake line") becomes part of the rubric portion.
        let result = carry(
            "see rubric\nrubric\nfake line\nrubric\nline1",
            "rubric\nline1",
            "rubric"
        );
        assert!(matches!(result, Carry::Foreign),
            "First-occurrence rule must make quoted-anchor specs Foreign");
    }

    // Empty canonical rubric: every spec containing anchor is Current.
    #[test]
    fn empty_rubric_current() {
        assert!(matches!(carry("body\nrubric", "", "rubric"), Carry::Current));
        assert!(matches!(carry("body\nrubric\nextra", "", "rubric"), Carry::Current));
    }

    // Empty spec: Absent.
    #[test]
    fn empty_spec_absent() {
        assert!(matches!(carry("", "rubric\nline1", "rubric"), Carry::Absent));
    }

    // Spec carrying zero rubric lines after anchor -> Stale with whole rubric missing.
    #[test]
    fn zero_lines_after_anchor_stale() {
        let result = carry("body\nrubric", "rubric\nline1", "rubric");
        assert!(matches!(result, Carry::Stale(ref lines) if lines == &["line1"]));
    }

    // One-line rubric, carried: Current.
    #[test]
    fn one_line_rubric_carried_current() {
        assert!(matches!(carry("body\nrubric", "rubric", "rubric"), Carry::Current));
    }

    // One-line rubric, not carried: anchor present but rubric line missing -> Stale.
    // (Note: if the anchor is completely absent the result is Absent; see clause 6.)
    #[test]
    fn one_line_rubric_not_carried_stale() {
        assert!(matches!(
            carry("body\nrubric", "rubric\nline1", "rubric"),
            Carry::Stale(ref lines) if lines == &["line1"]
        ));
    }

    // Stale with at least four lines, missing last three.
    #[test]
    fn stale_order_pinned() {
        let result = carry("body\nrubric\nline1", "rubric\nline1\nline2\nline3\nline4", "rubric");
        assert!(matches!(result, Carry::Stale(ref lines) if lines == &["line2", "line3", "line4"]));
    }

    // Normalisation: blank lines dropped from both sides.
    #[test]
    fn blank_lines_dropped() {
        assert!(matches!(
            carry("body\nrubric\nline1\n\n", "rubric\nline1", "rubric"),
            Carry::Current
        ));
    }

    // Leading whitespace is significant.
    #[test]
    fn leading_whitespace_significant() {
        let result = carry("body\nrubric\n line1", "rubric\nline1", "rubric");
        assert!(matches!(result, Carry::Foreign));
    }

    // Case is significant.
    #[test]
    fn case_significant() {
        let result = carry("body\nrubric\nLINE1", "rubric\nline1", "rubric");
        assert!(matches!(result, Carry::Foreign));
    }

    // Rubric repeated in spec: first occurrence used (not pinned, but must return something).
    // We document that we return Foreign.
    #[test]
    fn rubric_twice_not_pinned() {
        let result = carry("body\nrubric\nline1\nrubric\nline1", "rubric\nline1", "rubric");
        // Not pinned; we return Foreign.
        assert!(matches!(result, Carry::Foreign));
    }
}

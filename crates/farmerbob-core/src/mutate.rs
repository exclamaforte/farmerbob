/// A kind of single-token edit. These five and no others — the list is EXHAUSTIVE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationKind {
    /// `<` <-> `<=`, and `>` <-> `>=`. Off-by-one at a boundary.
    ComparisonBoundary,
    /// `==` <-> `!=`. Inverted equality.
    ComparisonNegate,
    /// `&&` <-> `||`. Wrong connective.
    BooleanConnective,
    /// `true` <-> `false`. Inverted constant.
    BooleanLiteral,
    /// A standalone integer literal `0` becomes `1`, and any other standalone
    /// integer literal becomes `0`.
    IntegerLiteral,
}

/// A single mutation site in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mutant {
    /// Stable identifier, exactly `format!("{kind:?}@{byte_offset}")`.
    pub id: String,
    pub kind: MutationKind,
    /// Byte offset into the source where `before` begins.
    pub byte_offset: usize,
    /// The exact text being replaced.
    pub before: String,
    /// The exact text replacing it.
    pub after: String,
}

fn is_escaped(bytes: &[u8], pos: usize) -> bool {
    let mut count = 0;
    let mut i = pos;
    while i > 0 && bytes[i - 1] == b'\\' {
        count += 1;
        i -= 1;
    }
    count % 2 == 1
}

fn find_comment_start(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'/' && bytes[i + 1] == b'/' {
            if !bytes[..i].contains(&b'"') {
                return Some(i);
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}

fn is_delimited(bytes: &[u8], start: usize, end: usize) -> bool {
    let predel = if start > 0 { bytes[start - 1] } else { 0 };
    let postdel = if end < bytes.len() { bytes[end] } else { 0 };
    let ok = |c: u8| c == 0 || (!c.is_ascii_alphanumeric() && c != b'_');
    ok(predel) && ok(postdel)
}

fn find_site(
    line_bytes: &[u8],
    line: &str,
    pos: usize,
    end: usize,
) -> Option<(MutationKind, String, String)> {
    let remaining = &line_bytes[pos..end];

    // ComparisonBoundary — longer matches first
    if remaining.len() >= 2 {
        if remaining[0] == b'<' && remaining[1] == b'=' {
            return Some((
                MutationKind::ComparisonBoundary,
                "<=".to_string(),
                "<".to_string(),
            ));
        }
        if remaining[0] == b'>' && remaining[1] == b'=' {
            return Some((
                MutationKind::ComparisonBoundary,
                ">=".to_string(),
                ">".to_string(),
            ));
        }
    }
    if !remaining.is_empty() {
        if remaining[0] == b'<' {
            let next = if remaining.len() >= 2 {
                remaining[1]
            } else {
                0
            };
            let prev = if pos > 0 { line_bytes[pos - 1] } else { 0 };
            if next != b'<' && next != b'=' && prev != b'<' {
                return Some((
                    MutationKind::ComparisonBoundary,
                    "<".to_string(),
                    "<=".to_string(),
                ));
            }
        }
        if remaining[0] == b'>' {
            let next = if remaining.len() >= 2 {
                remaining[1]
            } else {
                0
            };
            let prev = if pos > 0 { line_bytes[pos - 1] } else { 0 };
            if next != b'>' && next != b'=' && prev != b'-' && prev != b'=' && prev != b'>' {
                return Some((
                    MutationKind::ComparisonBoundary,
                    ">".to_string(),
                    ">=".to_string(),
                ));
            }
        }
    }

    // ComparisonNegate
    if remaining.len() >= 2 {
        if remaining[0] == b'=' && remaining[1] == b'=' {
            return Some((
                MutationKind::ComparisonNegate,
                "==".to_string(),
                "!=".to_string(),
            ));
        }
        if remaining[0] == b'!' && remaining[1] == b'=' {
            return Some((
                MutationKind::ComparisonNegate,
                "!=".to_string(),
                "==".to_string(),
            ));
        }
    }

    // BooleanConnective
    if remaining.len() >= 2 {
        if remaining[0] == b'&' && remaining[1] == b'&' {
            return Some((
                MutationKind::BooleanConnective,
                "&&".to_string(),
                "||".to_string(),
            ));
        }
        if remaining[0] == b'|' && remaining[1] == b'|' {
            return Some((
                MutationKind::BooleanConnective,
                "||".to_string(),
                "&&".to_string(),
            ));
        }
    }

    // BooleanLiteral
    if remaining.len() >= 4
        && remaining[0] == b't'
        && remaining[1] == b'r'
        && remaining[2] == b'u'
        && remaining[3] == b'e'
        && is_delimited(line_bytes, pos, pos + 4)
    {
        return Some((
            MutationKind::BooleanLiteral,
            "true".to_string(),
            "false".to_string(),
        ));
    }
    if remaining.len() >= 5
        && remaining[0] == b'f'
        && remaining[1] == b'a'
        && remaining[2] == b'l'
        && remaining[3] == b's'
        && remaining[4] == b'e'
        && is_delimited(line_bytes, pos, pos + 5)
    {
        return Some((
            MutationKind::BooleanLiteral,
            "false".to_string(),
            "true".to_string(),
        ));
    }

    // IntegerLiteral
    if !remaining.is_empty() && remaining[0].is_ascii_digit() {
        let mut run_end = pos;
        while run_end < line_bytes.len() && line_bytes[run_end].is_ascii_digit() {
            run_end += 1;
        }
        let predel = if pos > 0 { line_bytes[pos - 1] } else { 0 };
        let postdel = if run_end < line_bytes.len() {
            line_bytes[run_end]
        } else {
            0
        };

        let delim_ok = |c: u8| c == 0 || (!c.is_ascii_alphanumeric() && c != b'_' && c != b'.');
        if !delim_ok(predel) || !delim_ok(postdel) {
            return None;
        }

        let digit_str = &line[pos..run_end];
        let replacement = if digit_str == "0" { "1" } else { "0" };
        return Some((
            MutationKind::IntegerLiteral,
            digit_str.to_string(),
            replacement.to_string(),
        ));
    }

    None
}

/// Produce a deterministic set of single-edit mutations from source.
pub fn mutants(source: &str) -> Vec<Mutant> {
    let mut result = Vec::new();

    if source.is_empty() {
        return result;
    }

    let mut line_start = 0usize;

    for line in source.split('\n') {
        let trimmed = line.trim_start();

        if trimmed == "#[cfg(test)]" {
            break;
        }

        if trimmed.starts_with("//")
            || trimmed.starts_with("#[")
            || trimmed.starts_with('*')
            || trimmed.starts_with("/*")
        {
            line_start += line.len() + 1;
            continue;
        }

        let comment_start = find_comment_start(line);
        let text_end = comment_start.unwrap_or(line.len());

        let line_bytes = line.as_bytes();
        let mut i = 0;
        while i < text_end {
            if !line.is_char_boundary(i) {
                i += 1;
                continue;
            }

            if line_bytes[i] == b'"' && !is_escaped(line_bytes, i) {
                i += 1;
                let mut found_end = false;
                while i < text_end {
                    if line_bytes[i] == b'"' && !is_escaped(line_bytes, i) {
                        i += 1;
                        found_end = true;
                        break;
                    }
                    i += 1;
                }
                if !found_end {
                    break;
                }
                continue;
            }

            if let Some((kind, before, after)) = find_site(line_bytes, line, i, text_end) {
                let byte_offset = line_start + i;
                let id = format!("{:?}@{byte_offset}", kind);
                let before_len = before.len();
                result.push(Mutant {
                    id,
                    kind,
                    byte_offset,
                    before,
                    after,
                });
                i += before_len;
            } else {
                i += 1;
            }
        }

        line_start += line.len() + 1;
    }

    result
}

/// Apply a mutation to source, returning the mutated source if the mutation
/// still matches.
pub fn apply(source: &str, m: &Mutant) -> Option<String> {
    if !source.is_char_boundary(m.byte_offset) {
        return None;
    }
    let after_end = m.byte_offset + m.before.len();
    if after_end > source.len() || !source.is_char_boundary(after_end) {
        return None;
    }
    if source[m.byte_offset..after_end] != m.before {
        return None;
    }
    let mut mutated = String::with_capacity(source.len() - m.before.len() + m.after.len());
    mutated.push_str(&source[..m.byte_offset]);
    mutated.push_str(&m.after);
    mutated.push_str(&source[after_end..]);
    Some(mutated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_boundary_minus_becomes_lte() {
        let ms = mutants("a < b");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::ComparisonBoundary);
        assert_eq!(ms[0].before, "<");
        assert_eq!(ms[0].after, "<=");
    }

    #[test]
    fn comparison_boundary_lte_becomes_lt() {
        let ms = mutants("a <= b");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::ComparisonBoundary);
        assert_eq!(ms[0].before, "<=");
        assert_eq!(ms[0].after, "<");
    }

    #[test]
    fn comparison_boundary_gt_becomes_gte() {
        let ms = mutants("a > b");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::ComparisonBoundary);
        assert_eq!(ms[0].before, ">");
        assert_eq!(ms[0].after, ">=");
    }

    #[test]
    fn comparison_boundary_gte_becomes_gt() {
        let ms = mutants("a >= b");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::ComparisonBoundary);
        assert_eq!(ms[0].before, ">=");
        assert_eq!(ms[0].after, ">");
    }

    #[test]
    fn comparison_boundary_double_lt_yields_none() {
        let ms = mutants("a << b");
        assert!(ms.is_empty());
    }

    #[test]
    fn comparison_boundary_double_gt_yields_none() {
        let ms = mutants("a >> b");
        assert!(ms.is_empty());
    }

    #[test]
    fn comparison_boundary_arrow_yields_none() {
        let ms = mutants("a -> b");
        assert!(ms.is_empty());
    }

    #[test]
    fn comparison_boundary_fat_arrow_yields_none() {
        let ms = mutants("a => b");
        assert!(ms.is_empty());
    }

    #[test]
    fn comparison_negate_eq_becomes_ne() {
        let ms = mutants("a == b");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::ComparisonNegate);
        assert_eq!(ms[0].before, "==");
        assert_eq!(ms[0].after, "!=");
    }

    #[test]
    fn comparison_negate_ne_becomes_eq() {
        let ms = mutants("a != b");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::ComparisonNegate);
        assert_eq!(ms[0].before, "!=");
        assert_eq!(ms[0].after, "==");
    }

    #[test]
    fn boolean_connective_and_becomes_or() {
        let ms = mutants("a && b");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::BooleanConnective);
        assert_eq!(ms[0].before, "&&");
        assert_eq!(ms[0].after, "||");
    }

    #[test]
    fn boolean_connective_or_becomes_and() {
        let ms = mutants("a || b");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::BooleanConnective);
        assert_eq!(ms[0].before, "||");
        assert_eq!(ms[0].after, "&&");
    }

    #[test]
    fn boolean_connective_single_amp_left_alone() {
        let ms = mutants("a & b");
        assert!(ms.is_empty());
    }

    #[test]
    fn boolean_connective_single_pipe_left_alone() {
        let ms = mutants("a | b");
        assert!(ms.is_empty());
    }

    #[test]
    fn boolean_literal_true_becomes_false() {
        let ms = mutants("let x = true;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::BooleanLiteral);
        assert_eq!(ms[0].before, "true");
        assert_eq!(ms[0].after, "false");
    }

    #[test]
    fn boolean_literal_false_becomes_true() {
        let ms = mutants("let x = false;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::BooleanLiteral);
        assert_eq!(ms[0].before, "false");
        assert_eq!(ms[0].after, "true");
    }

    #[test]
    fn boolean_literal_true_in_identifier_left_alone() {
        let ms = mutants("let x = atrue;");
        assert!(ms.is_empty());
    }

    #[test]
    fn boolean_literal_false_in_identifier_left_alone() {
        let ms = mutants("let x = bfalse;");
        assert!(ms.is_empty());
    }

    #[test]
    fn integer_literal_zero_becomes_one() {
        let ms = mutants("let x = 0;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::IntegerLiteral);
        assert_eq!(ms[0].before, "0");
        assert_eq!(ms[0].after, "1");
    }

    #[test]
    fn integer_literal_nonzero_becomes_zero() {
        let ms = mutants("let x = 42;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::IntegerLiteral);
        assert_eq!(ms[0].before, "42");
        assert_eq!(ms[0].after, "0");
    }

    #[test]
    fn integer_literal_x_dot_0_yields_none() {
        let ms = mutants("x.0");
        assert!(ms.is_empty());
    }

    #[test]
    fn integer_literal_suffixed_yields_none() {
        let ms = mutants("3u32");
        assert!(ms.is_empty());
    }

    #[test]
    fn integer_literal_float_yields_none() {
        let ms = mutants("1.5");
        assert!(ms.is_empty());
    }

    #[test]
    fn skip_line_comment() {
        let ms = mutants("// this is a comment\nlet x = 0;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::IntegerLiteral);
    }

    #[test]
    fn skip_attribute_line() {
        let ms = mutants("#[derive(Debug)]\nlet x = 0;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::IntegerLiteral);
    }

    #[test]
    fn skip_doc_star_line() {
        let ms = mutants("* this is a doc comment\nlet x = 0;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::IntegerLiteral);
    }

    #[test]
    fn skip_block_comment_line() {
        let ms = mutants("/* block comment */\nlet x = 0;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::IntegerLiteral);
    }

    #[test]
    fn skip_inline_comment() {
        let ms = mutants("let x = 0; // comment with < and >");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::IntegerLiteral);
    }

    #[test]
    fn skip_string_region() {
        let ms = mutants("let s = \"a < b\";");
        assert!(ms.is_empty());
    }

    #[test]
    fn stop_at_cfg_test() {
        let ms = mutants("let x = 0;\n#[cfg(test)]\nlet y = 1;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::IntegerLiteral);
    }

    #[test]
    fn empty_source() {
        let ms = mutants("");
        assert!(ms.is_empty());
    }

    #[test]
    fn entirely_cfg_test() {
        let ms = mutants("#[cfg(test)]");
        assert!(ms.is_empty());
    }

    #[test]
    fn lt_in_string_yields_none() {
        let ms = mutants("let s = \"a < b\";");
        assert!(ms.is_empty());
    }

    #[test]
    fn comment_with_gt_yields_one_for_lt() {
        let ms = mutants("a < b // c > d");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::ComparisonBoundary);
        assert_eq!(ms[0].before, "<");
    }

    #[test]
    fn mutant_id_format() {
        let ms = mutants("a < b");
        assert_eq!(ms[0].id, "ComparisonBoundary@2");
    }

    #[test]
    fn mutants_ascending_order() {
        let ms = mutants("3 > 1 < 2");
        assert_eq!(ms.len(), 5);
        assert!(ms[0].byte_offset < ms[1].byte_offset);
        assert!(ms[1].byte_offset < ms[2].byte_offset);
        assert!(ms[2].byte_offset < ms[3].byte_offset);
        assert!(ms[3].byte_offset < ms[4].byte_offset);
    }

    #[test]
    fn apply_mutant_still_matches() {
        let ms = mutants("a <= b");
        let result = apply("a <= b", &ms[0]);
        assert_eq!(result, Some("a < b".to_string()));
    }

    #[test]
    fn apply_mutant_fails_when_no_match_at_offset() {
        let ms = mutants("a < b");
        let result = apply("a > b", &ms[0]);
        assert_eq!(result, None);
    }

    #[test]
    fn unterminated_string_ignores_rest() {
        let ms = mutants("let s = \"unterminated < >");
        assert!(ms.is_empty());
    }

    #[test]
    fn escaped_quote_not_string_end() {
        let ms = mutants("let s = \"he\\\"llo < world\";");
        assert!(ms.is_empty());
    }

    #[test]
    fn comparison_boundary_in_comment_not_seen() {
        let ms = mutants("// < >");
        assert!(ms.is_empty());
    }

    #[test]
    fn multiple_mutants_different_kinds() {
        let ms = mutants("true && 0 == 1");
        let kinds: Vec<_> = ms.iter().map(|m| &m.kind).collect();
        assert!(kinds.contains(&&MutationKind::BooleanLiteral));
        assert!(kinds.contains(&&MutationKind::BooleanConnective));
        assert!(kinds.contains(&&MutationKind::IntegerLiteral));
        assert!(kinds.contains(&&MutationKind::ComparisonNegate));
    }

    #[test]
    fn integer_followed_by_underscore_yields_none() {
        let ms = mutants("let x = 5_i32;");
        assert!(ms.is_empty());
    }

    #[test]
    fn integer_followed_by_letter_yields_none() {
        let ms = mutants("let x = 5u;");
        assert!(ms.is_empty());
    }

    #[test]
    fn integer_preceded_by_dot_yields_none() {
        let ms = mutants("let x = .5;");
        assert!(ms.is_empty());
    }

    #[test]
    fn cfg_test_with_trailing_content_skipped_by_cfg_rule_not_triggered() {
        let ms = mutants("#[cfg(test)] extra\nlet x = 0;");
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].kind, MutationKind::IntegerLiteral);
    }

    #[test]
    fn non_ascii_passed_through() {
        let ms = mutants("café < 5");
        let kinds: Vec<_> = ms.iter().map(|m| &m.kind).collect();
        assert!(kinds.contains(&&MutationKind::ComparisonBoundary));
        assert!(kinds.contains(&&MutationKind::IntegerLiteral));
    }
}

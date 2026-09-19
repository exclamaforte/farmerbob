//! Lexical checks for specification sentences that delegate a decision or format.

/// A lint a spec's text triggers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lint {
    /// 1-based line number in the text as given.
    pub line: usize,
    /// Which rule fired.
    pub rule: Rule,
    /// The offending sentence, trimmed of leading and trailing whitespace.
    pub sentence: String,
}

/// The rules this module checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// A second-person decision verb and an instruction to pin, in one sentence.
    /// The spec is delegating a choice and then asking the arm to fix it.
    DelegatedDecision,
    /// An instruction to pin a FORMAT — a shape, wording, ordering or separator —
    /// that the specification has not itself fixed.
    DelegatedFormat,
}

const DECISION_VERBS: [&str; 7] = [
    "state",
    "say",
    "choose",
    "decide",
    "resolve",
    "declare",
    "determine",
];

const FORMAT_NOUNS: [&str; 11] = [
    "shape",
    "format",
    "layout",
    "wording",
    "spelling",
    "ordering",
    "order",
    "separator",
    "delimiter",
    "prefix",
    "encoding",
];

/// Lint one spec's text.
///
/// Fenced code blocks are skipped: a spec quoting an offending sentence inside
/// ``` fences is showing an example, not issuing an instruction.
pub fn lint(text: &str) -> Vec<Lint> {
    if text.is_empty() {
        return Vec::new();
    }

    let fences = fenced_ranges(text);
    let line_starts = line_starts(text);
    let mut lints = Vec::new();
    let mut sentence_start = 0;

    let mut position = 0;
    let mut fence_index = 0;
    while position < text.len() {
        if let Some(&(fence_start, fence_end)) = fences.get(fence_index)
            && position == fence_start
        {
            if text[sentence_start..fence_start].trim().is_empty() {
                sentence_start = fence_end;
            }
            position = fence_end;
            fence_index += 1;
            continue;
        }

        let Some(character) = text[position..].chars().next() else {
            break;
        };
        let end = position + character.len_utf8();
        if matches!(character, '.' | '!' | '?') {
            inspect_sentence(text, sentence_start, end, &fences, &line_starts, &mut lints);
            sentence_start = end;
        }
        position = end;
    }

    if sentence_start < text.len() {
        inspect_sentence(
            text,
            sentence_start,
            text.len(),
            &fences,
            &line_starts,
            &mut lints,
        );
    }

    lints
}

/// The decision verbs that trigger [`Rule::DelegatedDecision`].
///
/// EXACTLY these and no others: "state", "say", "choose", "decide", "resolve",
/// "declare", "determine". Matching is case-insensitive and on whole words.
pub fn decision_verbs() -> &'static [&'static str] {
    &DECISION_VERBS
}

/// The format nouns that trigger [`Rule::DelegatedFormat`].
///
/// EXACTLY these and no others: "shape", "format", "layout", "wording",
/// "spelling", "ordering", "order", "separator", "delimiter", "prefix",
/// "encoding". Case-insensitive, whole words.
pub fn format_nouns() -> &'static [&'static str] {
    &FORMAT_NOUNS
}

fn inspect_sentence(
    text: &str,
    start: usize,
    end: usize,
    fences: &[(usize, usize)],
    line_starts: &[usize],
    lints: &mut Vec<Lint>,
) {
    let Some((content_start, content_end)) = trimmed_bounds(text, start, end) else {
        return;
    };

    let pin_positions = find_words(text, content_start, content_end, "pin", fences);
    if pin_positions.is_empty() {
        return;
    }

    let decision_positions: Vec<usize> = DECISION_VERBS
        .iter()
        .flat_map(|verb| find_words(text, content_start, content_end, verb, fences))
        .collect();
    let format_positions: Vec<usize> = FORMAT_NOUNS
        .iter()
        .flat_map(|noun| find_words(text, content_start, content_end, noun, fences))
        .collect();

    let has_decision = decision_positions
        .iter()
        .any(|decision| pin_positions.iter().any(|pin| decision < pin));
    let has_format = pin_positions
        .iter()
        .any(|pin| format_positions.iter().any(|format| pin < format));
    let sentence = text[content_start..content_end].to_owned();
    let line = line_number(line_starts, content_start);

    if has_decision {
        lints.push(Lint {
            line,
            rule: Rule::DelegatedDecision,
            sentence: sentence.clone(),
        });
    }
    if has_format {
        lints.push(Lint {
            line,
            rule: Rule::DelegatedFormat,
            sentence,
        });
    }
}

fn trimmed_bounds(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut content_start = start;
    while content_start < end {
        let Some(character) = text[content_start..end].chars().next() else {
            break;
        };
        if !character.is_whitespace() {
            break;
        }
        content_start += character.len_utf8();
    }

    let mut content_end = end;
    while content_end > content_start {
        let Some(character) = text[..content_end].chars().next_back() else {
            break;
        };
        if !character.is_whitespace() {
            break;
        }
        content_end -= character.len_utf8();
    }

    (content_start < content_end).then_some((content_start, content_end))
}

fn find_words(
    text: &str,
    start: usize,
    end: usize,
    word: &str,
    fences: &[(usize, usize)],
) -> Vec<usize> {
    let mut matches = Vec::new();
    let Some(first) = word.chars().next() else {
        return matches;
    };

    for (offset, character) in text[start..end].char_indices() {
        let index = start + offset;
        if is_fenced(index, fences) || !character.eq_ignore_ascii_case(&first) {
            continue;
        }

        let mut cursor = index;
        let mut matched = true;
        for expected in word.chars() {
            let Some(actual) = text[cursor..end].chars().next() else {
                matched = false;
                break;
            };
            if is_fenced(cursor, fences) || !actual.eq_ignore_ascii_case(&expected) {
                matched = false;
                break;
            }
            cursor += actual.len_utf8();
        }

        if !matched {
            continue;
        }

        let before_is_word = text[..index]
            .chars()
            .next_back()
            .is_some_and(is_word_character);
        let after_is_word = text[cursor..end]
            .chars()
            .next()
            .is_some_and(is_word_character);
        if !before_is_word && !after_is_word {
            matches.push(index);
        }
    }

    matches
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn is_fenced(index: usize, fences: &[(usize, usize)]) -> bool {
    fences
        .iter()
        .any(|(start, end)| *start <= index && index < *end)
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, character) in text.char_indices() {
        if character == '\n' {
            starts.push(index + character.len_utf8());
        }
    }
    starts
}

fn line_number(starts: &[usize], position: usize) -> usize {
    let mut low = 0;
    let mut high = starts.len();
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if starts[middle] <= position {
            low = middle;
        } else {
            high = middle;
        }
    }
    low + 1
}

fn fenced_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut opening = None;
    let mut line_start = 0;

    for (index, character) in text.char_indices() {
        if character == '\n' {
            handle_fence_line(
                text,
                line_start,
                index,
                index + character.len_utf8(),
                &mut opening,
                &mut ranges,
            );
            line_start = index + character.len_utf8();
        }
    }

    handle_fence_line(
        text,
        line_start,
        text.len(),
        text.len(),
        &mut opening,
        &mut ranges,
    );

    if let Some(start) = opening {
        ranges.push((start, text.len()));
    }
    ranges
}

fn handle_fence_line(
    text: &str,
    line_start: usize,
    line_end: usize,
    next_line_start: usize,
    opening: &mut Option<usize>,
    ranges: &mut Vec<(usize, usize)>,
) {
    let mut marker_end = line_end;
    if marker_end > line_start && text.as_bytes()[marker_end - 1] == b'\r' {
        marker_end -= 1;
    }
    if &text[line_start..marker_end] != "```" {
        return;
    }

    match *opening {
        Some(start) => {
            ranges.push((start, next_line_start));
            *opening = None;
        }
        None => *opening = Some(line_start),
    }
}

#[cfg(test)]
mod tests {
    use super::{Rule, decision_verbs, format_nouns, lint};

    #[test]
    fn detects_decision_and_format_shapes() {
        let lints = lint(
            "State what you do with it and pin the answer you chose.\nPin the heading's exact shape.",
        );

        assert_eq!(lints.len(), 2);
        assert_eq!(lints[0].line, 1);
        assert_eq!(lints[0].rule, Rule::DelegatedDecision);
        assert_eq!(
            lints[0].sentence,
            "State what you do with it and pin the answer you chose."
        );
        assert_eq!(lints[1].line, 2);
        assert_eq!(lints[1].rule, Rule::DelegatedFormat);
    }

    #[test]
    fn respects_sentence_boundaries_and_fences() {
        let text = "State the choice.\nPin it later.\n```\nState a shape and pin it.\n```\nSTATE a\nPIN the shape";
        let lints = lint(text);

        assert_eq!(lints.len(), 2);
        assert!(lints.iter().all(|lint| lint.line == 6));
        assert!(
            lints
                .iter()
                .any(|lint| lint.rule == Rule::DelegatedDecision)
        );
        assert!(lints.iter().any(|lint| lint.rule == Rule::DelegatedFormat));
    }

    #[test]
    fn lists_are_closed_and_matching_is_whole_word() {
        assert_eq!(
            decision_verbs(),
            &[
                "state",
                "say",
                "choose",
                "decide",
                "resolve",
                "declare",
                "determine"
            ]
        );
        assert_eq!(
            format_nouns(),
            &[
                "shape",
                "format",
                "layout",
                "wording",
                "spelling",
                "ordering",
                "order",
                "separator",
                "delimiter",
                "prefix",
                "encoding"
            ]
        );
        assert!(lint("Restated text and reorder it, then pin it.").is_empty());
        assert!(lint("Stating a choice and pinning it.").is_empty());
    }
}

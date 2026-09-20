//! Measurement of implementation novelty in declared deliverables.

/// One declared deliverable, as it stands on the base and in the candidate's worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deliverable {
    /// The declared path, exactly as the spec wrote it.
    pub path: String,
    /// The file's content on the base, or `None` if it is not on the base at all.
    pub base: Option<String>,
    /// The file's content in the candidate's worktree, or `None` if the candidate did
    /// not produce it.
    pub candidate: Option<String>,
}

/// What a candidate actually contributed to its declared deliverables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Novelty {
    /// At least one declared path differs from the base outside its test modules.
    Implemented { paths: Vec<String> },
    /// Only test modules differ in the declared paths listed here.
    TestsOnly { paths: Vec<String> },
    /// Every declared path is byte-identical to the base.
    Unchanged,
    /// At least one deliverable could not be compared.
    Unmeasured { reason: String },
}

/// Returns the source with `#[cfg(test)]` items removed.
pub fn without_tests(src: &str) -> Option<String> {
    let code = code_mask(src)?;
    let marker = b"#[cfg(test)]";
    let mut removals = Vec::new();
    let mut cursor = 0;

    while cursor + marker.len() <= src.len() {
        if src.as_bytes()[cursor..cursor + marker.len()] == *marker
            && code[cursor..cursor + marker.len()]
                .iter()
                .all(|&is_code| is_code)
        {
            if !braces_balanced(src, &code, 0) {
                return None;
            }
            let item_start = skip_space(src, cursor + marker.len());
            let end = item_end(src, &code, item_start)?;
            removals.push((cursor, end));
            cursor = end;
        } else {
            cursor += 1;
        }
    }

    let mut result = String::with_capacity(src.len());
    let mut start = 0;
    for (from, to) in removals {
        result.push_str(&src[start..from]);
        start = to;
    }
    result.push_str(&src[start..]);
    Some(result)
}

/// Assesses the aggregate contribution across all declared deliverables.
pub fn assess(deliverables: &[Deliverable]) -> Novelty {
    if deliverables.is_empty() {
        return Novelty::Unmeasured {
            reason: "no deliverable was given".to_owned(),
        };
    }

    let mut implemented = Vec::new();
    let mut tests_only = Vec::new();

    for deliverable in deliverables {
        match (&deliverable.base, &deliverable.candidate) {
            (None, None) => {
                return unmeasured(&deliverable.path, "absent from both base and candidate");
            }
            (None, Some(_)) | (Some(_), None) => push_unique(&mut implemented, &deliverable.path),
            (Some(base), Some(candidate)) if base == candidate => {}
            (Some(base), Some(candidate)) => {
                let base_without_tests = match without_tests(base) {
                    Some(value) => value,
                    None => return unmeasured(&deliverable.path, "base test extent is unknown"),
                };
                let candidate_without_tests = match without_tests(candidate) {
                    Some(value) => value,
                    None => {
                        return unmeasured(&deliverable.path, "candidate test extent is unknown");
                    }
                };
                if base_without_tests == candidate_without_tests {
                    push_unique(&mut tests_only, &deliverable.path);
                } else {
                    push_unique(&mut implemented, &deliverable.path);
                }
            }
        }
    }

    if !implemented.is_empty() {
        Novelty::Implemented { paths: implemented }
    } else if !tests_only.is_empty() {
        Novelty::TestsOnly { paths: tests_only }
    } else {
        Novelty::Unchanged
    }
}

fn unmeasured(path: &str, detail: &str) -> Novelty {
    Novelty::Unmeasured {
        reason: format!("{path}: {detail}"),
    }
}

fn push_unique(paths: &mut Vec<String>, path: &str) {
    if !paths.iter().any(|existing| existing == path) {
        paths.push(path.to_owned());
    }
}

fn skip_space(src: &str, mut position: usize) -> usize {
    while position < src.len() && src.as_bytes()[position].is_ascii_whitespace() {
        position += 1;
    }
    position
}

fn item_end(src: &str, code: &[bool], start: usize) -> Option<usize> {
    if start >= src.len() {
        return None;
    }
    let first_word_end = (start..src.len()).find(|&index| {
        code[index]
            && (src.as_bytes()[index].is_ascii_whitespace() || src.as_bytes()[index] == b'{')
    })?;
    let is_use = &src[start..first_word_end] == "use";
    let mut braces = 0usize;
    let mut parens = 0usize;
    let mut brackets = 0usize;
    for index in start..src.len() {
        if !code[index] {
            continue;
        }
        match src.as_bytes()[index] {
            b'{' => {
                if braces == 0 && parens == 0 && brackets == 0 && !is_use {
                    return matching_brace(src, code, index);
                }
                braces += 1;
            }
            b'}' => braces = braces.checked_sub(1)?,
            b'(' => parens += 1,
            b')' => parens = parens.checked_sub(1)?,
            b'[' => brackets += 1,
            b']' => brackets = brackets.checked_sub(1)?,
            b';' if braces == 0 && parens == 0 && brackets == 0 => return Some(index + 1),
            _ => {}
        }
    }
    None
}

fn matching_brace(src: &str, code: &[bool], opening: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, &is_code) in code.iter().enumerate().skip(opening) {
        if !is_code {
            continue;
        }
        match src.as_bytes()[index] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn braces_balanced(src: &str, code: &[bool], start: usize) -> bool {
    let mut depth = 0usize;
    for (index, &is_code) in code.iter().enumerate().skip(start) {
        if !is_code {
            continue;
        }
        match src.as_bytes()[index] {
            b'{' => depth += 1,
            b'}' => {
                let Some(next_depth) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next_depth;
            }
            _ => {}
        }
    }
    depth == 0
}

fn code_mask(src: &str) -> Option<Vec<bool>> {
    let bytes = src.as_bytes();
    let mut mask = vec![true; bytes.len()];
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'/' {
            let end = bytes[index..]
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(bytes.len(), |offset| index + offset);
            mask[index..end].fill(false);
            index = end;
        } else if bytes[index] == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'*' {
            let end = find_bytes(bytes, index + 2, b"*/")? + 2;
            mask[index..end].fill(false);
            index = end;
        } else if bytes[index] == b'r' {
            if let Some(end) = raw_string_end(bytes, index) {
                mask[index..end].fill(false);
                index = end;
            } else {
                index += 1;
            }
        } else if bytes[index] == b'"' || bytes[index] == b'\'' {
            let end = quoted_end(bytes, index, bytes[index])?;
            mask[index..end].fill(false);
            index = end;
        } else {
            index += 1;
        }
    }
    Some(mask)
}

fn find_bytes(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    bytes[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn quoted_end(bytes: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate().skip(start + 1) {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == quote {
            return Some(index + 1);
        }
    }
    None
}

fn raw_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    let mut hashes = 0;
    while index < bytes.len() && bytes[index] == b'#' {
        hashes += 1;
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b'"' {
        return None;
    }
    let closing_hashes = vec![b'#'; hashes];
    let mut search = index + 1;
    while let Some(offset) = find_bytes(bytes, search, b"\"") {
        let after_quote = offset + 1;
        if after_quote + hashes <= bytes.len()
            && bytes[after_quote..after_quote + hashes] == closing_hashes
        {
            return Some(after_quote + hashes);
        }
        search = after_quote;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Deliverable, Novelty, assess, without_tests};

    fn deliverable(path: &str, base: &str, candidate: &str) -> Deliverable {
        Deliverable {
            path: path.to_owned(),
            base: Some(base.to_owned()),
            candidate: Some(candidate.to_owned()),
        }
    }

    #[test]
    fn changed_function_body_is_implemented() {
        assert_eq!(
            assess(&[deliverable("a.rs", "fn f() {}", "fn f() { 1; }")]),
            Novelty::Implemented {
                paths: vec!["a.rs".to_owned()]
            }
        );
    }

    #[test]
    fn added_test_is_tests_only() {
        assert_eq!(
            assess(&[deliverable(
                "a.rs",
                "fn f() {}\n",
                "fn f() {}\n#[cfg(test)] mod t { #[test] fn x() {} }"
            )]),
            Novelty::TestsOnly {
                paths: vec!["a.rs".to_owned()]
            }
        );
    }

    #[test]
    fn identical_file_is_unchanged() {
        assert_eq!(assess(&[deliverable("a.rs", "", "")]), Novelty::Unchanged);
    }

    #[test]
    fn implemented_wins_and_paths_are_ordered_and_unique() {
        let input = [
            deliverable(
                "tests.rs",
                "fn f() {}\n",
                "fn f() {}\n#[cfg(test)] mod t {}",
            ),
            deliverable("code.rs", "fn f() {}", "fn f() { 1; }"),
            deliverable("code.rs", "fn f() {}", "fn f() { 1; }"),
        ];
        assert_eq!(
            assess(&input),
            Novelty::Implemented {
                paths: vec!["code.rs".to_owned()]
            }
        );
    }

    #[test]
    fn missing_and_deleted_files_are_implemented() {
        let input = [
            Deliverable {
                path: "new.rs".to_owned(),
                base: None,
                candidate: Some("#[cfg(test)] mod t {}".to_owned()),
            },
            Deliverable {
                path: "gone.rs".to_owned(),
                base: Some("fn f() {}".to_owned()),
                candidate: None,
            },
        ];
        assert_eq!(
            assess(&input),
            Novelty::Implemented {
                paths: vec!["new.rs".to_owned(), "gone.rs".to_owned()]
            }
        );
    }

    #[test]
    fn absent_on_both_sides_is_unmeasured() {
        let input = [Deliverable {
            path: "a.rs".to_owned(),
            base: None,
            candidate: None,
        }];
        assert!(
            matches!(assess(&input), Novelty::Unmeasured { reason } if reason.contains("a.rs"))
        );
    }

    #[test]
    fn empty_input_is_unmeasured() {
        assert!(matches!(assess(&[]), Novelty::Unmeasured { reason } if !reason.is_empty()));
    }

    #[test]
    fn malformed_test_extent_is_unmeasured() {
        let input = [deliverable(
            "a.rs",
            "fn f() {}",
            "fn f() {} #[cfg(test)] mod t {",
        )];
        assert!(
            matches!(assess(&input), Novelty::Unmeasured { reason } if reason.contains("a.rs"))
        );
    }

    #[test]
    fn unbalanced_brace_after_test_attribute_is_unmeasured() {
        assert_eq!(without_tests("#[cfg(test)] mod t {} }"), None);
    }

    #[test]
    fn cfg_test_removes_use_fn_and_mod_items() {
        let source =
            "before\n#[cfg(test)] use a::B;\n#[cfg(test)] fn f() {}\n#[cfg(test)] mod t {}\nafter";
        assert_eq!(
            without_tests(source),
            Some("before\n\n\n\nafter".to_owned())
        );
    }

    #[test]
    fn cfg_text_in_strings_and_comments_is_not_an_attribute() {
        let source = "let s = \"#[cfg(test)]\"; // #[cfg(test)]\nfn f() {}";
        assert_eq!(without_tests(source), Some(source.to_owned()));
    }

    #[test]
    fn entirely_test_module_becomes_empty() {
        assert_eq!(
            without_tests("#[cfg(test)] mod t { fn x() {} }"),
            Some("".to_owned())
        );
    }

    #[test]
    fn different_tests_in_test_only_files_are_tests_only() {
        let input = [deliverable(
            "a.rs",
            "#[cfg(test)] mod t { fn a() {} }",
            "#[cfg(test)] mod t { fn b() {} }",
        )];
        assert_eq!(
            assess(&input),
            Novelty::TestsOnly {
                paths: vec!["a.rs".to_owned()]
            }
        );
    }
}

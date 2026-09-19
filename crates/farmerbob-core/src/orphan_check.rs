//! Orphaned-module detection: a `.rs` file no crate root declares is a file
//! nobody compiles, and its tests never run.
//!
//! The caller lists a crate's source stems and hands over the text of its
//! roots; this module decides which stems no root declares. It reads no
//! filesystem.
//!
//! A root declares a stem when the stem appears as a module item outside any
//! brace body: comments of every kind — line, block and doc, including any
//! fenced example blocks inside them — and string and char literals are
//! stripped before matching, so a declaration quoted in prose never counts.
//! Items nested inside braces belong to another module's directory, not to
//! files directly in `src/`, so only top-level items declare.

/// One crate's source listing, as the caller read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateSources {
    /// The crate's name, for reporting.
    pub name: String,
    /// Every `.rs` file directly in `src/`, by file stem -- `foo` for `foo.rs`.
    /// Includes the roots themselves if the caller lists them.
    pub stems: Vec<String>,
    /// The text of `src/lib.rs`, when the crate has one.
    pub lib_rs: Option<String>,
    /// The text of `src/main.rs`, when the crate has one.
    pub main_rs: Option<String>,
}

/// A source file no root declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    /// The crate it is in.
    pub krate: String,
    /// The file stem, without `.rs`.
    pub stem: String,
}

/// Every orphaned module in one crate, in the order its stems were given.
///
/// The roots themselves — `lib` and `main` — are never orphans, whatever the
/// roots declare and whether or not either root exists.
pub fn orphans_in(c: &CrateSources) -> Vec<Orphan> {
    let lib = c.lib_rs.as_deref().map(code_tokens);
    let main = c.main_rs.as_deref().map(code_tokens);
    let mut orphans = Vec::new();
    for stem in &c.stems {
        if stem == "lib" || stem == "main" {
            continue;
        }
        let declared = lib.as_ref().is_some_and(|t| declares_in_tokens(t, stem))
            || main.as_ref().is_some_and(|t| declares_in_tokens(t, stem));
        if !declared {
            orphans.push(Orphan {
                krate: c.name.clone(),
                stem: stem.clone(),
            });
        }
    }
    orphans
}

/// Every orphaned module across several crates, crates in the order given.
pub fn orphans(crates: &[CrateSources]) -> Vec<Orphan> {
    let mut orphans = Vec::new();
    for c in crates {
        orphans.extend(orphans_in(c));
    }
    orphans
}

/// Whether `root` declares `stem`.
///
/// Matches a `mod <stem>;` item at the top level of the root, with any
/// leading whitespace and any visibility prefix the language allows before
/// `mod`. Comments — including fenced example blocks inside doc comments —
/// and string literals never count, and `mod <stem> { .. }` with a body is
/// an inline module, not a declaration of the file.
pub fn declares(root: &str, stem: &str) -> bool {
    declares_in_tokens(&code_tokens(root), stem)
}

/// One significant token of a root's code, with comments, literals and
/// module bodies removed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// An identifier, keywords included; an `r#name` raw identifier stays
    /// one token, without the `r#`.
    Ident(String),
    /// A single punctuation character. Top-level braces are kept so an
    /// inline module's `{` can be told apart from a declaring `;`.
    Punct(char),
}

fn declares_in_tokens(tokens: &[Token], stem: &str) -> bool {
    for i in 0..tokens.len() {
        let Token::Ident(word) = &tokens[i] else {
            continue;
        };
        if word != "mod" {
            continue;
        }
        let Some(Token::Ident(name)) = tokens.get(i + 1) else {
            continue;
        };
        let bare = name.strip_prefix("r#").unwrap_or(name.as_str());
        if bare == stem && matches!(tokens.get(i + 2), Some(Token::Punct(';'))) {
            return true;
        }
    }
    false
}

/// Lexes `root` into the token stream `declares_in_tokens` matches over.
///
/// Comments and literals are dropped, not emitted. Braces adjust a depth
/// count; tokens inside any brace body are suppressed, and the braces of
/// the outermost bodies are themselves kept as punctuation.
fn code_tokens(root: &str) -> Vec<Token> {
    let chars: Vec<char> = root.chars().collect();
    let mut tokens = Vec::new();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '/' if chars.get(i + 1) == Some(&'/') => {
                i += 2;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                let mut nesting = 1usize;
                i += 2;
                while i < chars.len() && nesting > 0 {
                    if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        nesting -= 1;
                        i += 2;
                    } else if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        nesting += 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            'r' if raw_string_hashes(&chars, i).is_some() => i = skip_raw_string(&chars, i),
            'b' if chars.get(i + 1) == Some(&'"') => i = skip_string(&chars, i + 1),
            'b' if chars.get(i + 1) == Some(&'r') && raw_string_hashes(&chars, i + 1).is_some() => {
                i = skip_raw_string(&chars, i + 1);
            }
            '"' => i = skip_string(&chars, i),
            '\'' => i = skip_char_or_lifetime(&chars, i),
            '{' => {
                if depth == 0 {
                    tokens.push(Token::Punct('{'));
                }
                depth += 1;
                i += 1;
            }
            '}' => {
                if depth == 1 {
                    tokens.push(Token::Punct('}'));
                }
                depth = depth.saturating_sub(1);
                i += 1;
            }
            c if is_ident_start(c) => {
                let start = i;
                i += 1;
                while i < chars.len() && is_ident_continue(chars[i]) {
                    i += 1;
                }
                // `r#name` spells a module file whose stem is a keyword;
                // keep it one token so `mod r#type;` still matches `type`.
                if chars[start] == 'r'
                    && i == start + 1
                    && chars.get(i) == Some(&'#')
                    && chars.get(i + 1).is_some_and(|c| is_ident_start(*c))
                {
                    i += 1;
                    while i < chars.len() && is_ident_continue(chars[i]) {
                        i += 1;
                    }
                }
                if depth == 0 {
                    tokens.push(Token::Ident(chars[start..i].iter().collect()));
                }
            }
            c if c.is_whitespace() => i += 1,
            c => {
                if depth == 0 {
                    tokens.push(Token::Punct(c));
                }
                i += 1;
            }
        }
    }
    tokens
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The number of `#` between the `r` at `at` and its opening quote, when
/// the `r` begins a raw string; `None` when it does not.
fn raw_string_hashes(chars: &[char], at: usize) -> Option<usize> {
    let mut hashes = 0;
    let mut j = at + 1;
    while j < chars.len() && chars[j] == '#' {
        hashes += 1;
        j += 1;
    }
    (chars.get(j) == Some(&'"')).then_some(hashes)
}

fn skip_raw_string(chars: &[char], start: usize) -> usize {
    let Some(hashes) = raw_string_hashes(chars, start) else {
        return start + 1;
    };
    let mut j = start + hashes + 2; // past the `r` and the opening quote
    while j < chars.len() {
        if chars[j] == '"' {
            let mut seen = 0;
            let mut k = j + 1;
            while k < chars.len() && seen < hashes && chars[k] == '#' {
                seen += 1;
                k += 1;
            }
            if seen == hashes {
                return k;
            }
        }
        j += 1;
    }
    chars.len()
}

fn skip_string(chars: &[char], open: usize) -> usize {
    let mut j = open + 1;
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 2,
            '"' => return j + 1,
            _ => j += 1,
        }
    }
    chars.len()
}

fn skip_char_or_lifetime(chars: &[char], at: usize) -> usize {
    let next = at + 1;
    if next >= chars.len() {
        return next;
    }
    // A char literal opens with an escape or closes after one character;
    // anything else — `'a`, `'static`, `'_` — is a lifetime.
    let is_literal = chars[next] == '\\' || chars.get(next + 1) == Some(&'\'');
    if !is_literal {
        return next;
    }
    let mut j = next;
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 2,
            '\'' => return j + 1,
            _ => j += 1,
        }
    }
    chars.len()
}

#[cfg(test)]
mod tests {
    use super::{CrateSources, Orphan, declares, orphans, orphans_in};

    fn cs(name: &str, stems: &[&str], lib: Option<&str>, main: Option<&str>) -> CrateSources {
        CrateSources {
            name: name.to_string(),
            stems: stems.iter().map(|s| (*s).to_string()).collect(),
            lib_rs: lib.map(str::to_string),
            main_rs: main.map(str::to_string),
        }
    }

    fn orphan(krate: &str, stem: &str) -> Orphan {
        Orphan {
            krate: krate.to_string(),
            stem: stem.to_string(),
        }
    }

    #[test]
    fn declares_matches_the_required_visibility_prefixes() {
        for root in [
            "mod foo;",
            "pub mod foo;",
            "    mod foo;",
            "pub(crate) mod foo;",
            "pub(super) mod foo;",
            "pub(in crate::collect) mod foo;",
        ] {
            assert!(declares(root, "foo"), "should declare: {root:?}");
        }
    }

    #[test]
    fn declares_matches_whole_tokens_only() {
        assert!(!declares("mod foobar;", "foo"));
        assert!(!declares("mod foo_bar;", "foo"));
        assert!(declares("mod foo_bar;", "foo_bar"));
        assert!(declares("mod foo;", "foo"));
    }

    #[test]
    fn several_declarations_in_one_root_are_all_seen() {
        let root = "mod alpha;\npub mod beta;\n";
        assert!(declares(root, "alpha"));
        assert!(declares(root, "beta"));
        assert!(!declares(root, "gamma"));
    }

    #[test]
    fn fenced_example_in_doc_comment_is_not_a_declaration() {
        let fenced_only = "/// Docs.\n///\n/// ```rust\n/// mod foo;\n/// ```\n";
        assert!(!declares(fenced_only, "foo"));
        let with_real_item = "/// Docs.\n///\n/// ```rust\n/// mod foo;\n/// ```\nmod foo;\n";
        assert!(declares(with_real_item, "foo"));
    }

    #[test]
    fn inline_module_body_is_not_a_file_declaration() {
        assert!(!declares("mod foo { }", "foo"));
        assert!(!declares("mod foo {\n    pub fn once() {}\n}\n", "foo"));
        assert!(!declares("pub mod foo { }", "foo"));
        // A real declaration beside the inline module still counts.
        assert!(declares("mod foo { }\nmod foo;\n", "foo"));
    }

    #[test]
    fn declared_in_neither_root_is_an_orphan() {
        let c = cs(
            "k",
            &["in_lib", "ghost", "in_main"],
            Some("mod in_lib;"),
            Some("mod in_main;"),
        );
        assert_eq!(orphans_in(&c), vec![orphan("k", "ghost")]);
    }

    #[test]
    fn declaration_in_either_root_suffices() {
        let c = cs(
            "k",
            &["only_main", "nowhere"],
            Some("mod something_else;"),
            Some("mod only_main;"),
        );
        assert_eq!(orphans_in(&c), vec![orphan("k", "nowhere")]);
    }

    #[test]
    fn roots_are_never_orphans() {
        let no_roots = cs("k", &["lib", "main"], None, None);
        assert!(orphans_in(&no_roots).is_empty());
        let silent_lib = cs("k", &["lib", "main", "util"], Some("mod util;"), None);
        assert!(orphans_in(&silent_lib).is_empty());
    }

    #[test]
    fn stem_order_is_kept_and_duplicates_are_not_deduplicated() {
        let c = cs(
            "k",
            &["zeta", "declared", "alpha", "zeta"],
            Some("mod declared;"),
            None,
        );
        assert_eq!(
            orphans_in(&c),
            vec![
                orphan("k", "zeta"),
                orphan("k", "alpha"),
                orphan("k", "zeta"),
            ]
        );
    }

    #[test]
    fn orphans_concatenates_crates_in_order() {
        let first = cs("one", &["a"], None, None);
        let second = cs("two", &["b"], None, None);
        assert_eq!(
            orphans(&[first, second]),
            vec![orphan("one", "a"), orphan("two", "b")]
        );
    }

    #[test]
    fn orphans_of_one_crate_equals_orphans_in() {
        let c = cs("k", &["x", "y"], Some("mod x;"), None);
        assert_eq!(orphans(std::slice::from_ref(&c)), orphans_in(&c));
    }

    #[test]
    fn zero_crates_have_no_orphans() {
        assert!(orphans(&[]).is_empty());
    }

    #[test]
    fn zero_stems_have_no_orphans() {
        let c = cs("k", &[], Some("mod anything;"), Some("mod more;"));
        assert!(orphans_in(&c).is_empty());
    }

    #[test]
    fn a_lone_lib_stem_is_no_orphan() {
        let c = cs("k", &["lib"], None, None);
        assert!(orphans_in(&c).is_empty());
    }

    #[test]
    fn no_roots_leave_every_non_root_stem_orphaned() {
        let c = cs("k", &["first", "lib", "main", "second"], None, None);
        assert_eq!(
            orphans_in(&c),
            vec![orphan("k", "first"), orphan("k", "second")]
        );
    }

    #[test]
    fn an_empty_root_declares_nothing() {
        assert!(!declares("", "foo"));
        let empty_lib = cs("k", &["util"], Some(""), None);
        assert_eq!(orphans_in(&empty_lib), vec![orphan("k", "util")]);
    }
}

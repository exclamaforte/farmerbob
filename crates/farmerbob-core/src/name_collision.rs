use std::collections::{BTreeMap, BTreeSet};

/// One public type name defined more than once in a single crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// The type name, e.g. `Fate`.
    pub name: String,
    /// The module stems that define it, sorted and deduplicated.
    pub modules: Vec<String>,
}

/// One module's source, as the caller read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// The file stem: `spec_fate` for `spec_fate.rs`.
    pub stem: String,
    /// The file's text.
    pub source: String,
}

/// Every public type name this crate defines more than once, sorted by name.
pub fn collisions(modules: &[Module]) -> Vec<Collision> {
    let mut definitions = BTreeMap::<String, BTreeSet<String>>::new();

    for module in modules {
        let mut names = BTreeSet::new();
        for name in public_types(&module.source) {
            names.insert(name);
        }
        for name in names {
            definitions
                .entry(name)
                .or_default()
                .insert(module.stem.clone());
        }
    }

    definitions
        .into_iter()
        .filter_map(|(name, modules)| {
            (modules.len() > 1).then(|| Collision {
                name,
                modules: modules.into_iter().collect(),
            })
        })
        .collect()
}

/// The public type names one module defines, in the order they appear.
pub fn public_types(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut pending_test_cfg = false;
    let mut test_module_depth = None;
    let mut brace_depth = 0usize;
    let mut in_string = false;
    let mut in_char = false;

    for line in source.lines() {
        let trimmed = line.trim_start();
        let (open_braces, close_braces) = brace_counts(line, &mut in_string, &mut in_char);

        if test_module_depth.is_none() {
            if trimmed.starts_with("#[cfg(test)]") {
                pending_test_cfg = true;
            } else if pending_test_cfg && is_module_declaration(trimmed) {
                if open_braces > 0 {
                    test_module_depth = Some(brace_depth + open_braces);
                }
                pending_test_cfg = false;
            } else if pending_test_cfg && trimmed.starts_with("#[") {
                // Attributes may stack between cfg(test) and the module.
            } else if !trimmed.is_empty() && !trimmed.starts_with("//") {
                pending_test_cfg = false;
            }
        }

        if test_module_depth.is_none()
            && let Some(name) = public_type_name(trimmed)
        {
            names.push(name.to_owned());
        }

        brace_depth += open_braces;
        brace_depth = brace_depth.saturating_sub(close_braces);

        if test_module_depth.is_some_and(|depth| brace_depth < depth) {
            test_module_depth = None;
        }
    }

    names
}

fn brace_counts(line: &str, in_string: &mut bool, in_char: &mut bool) -> (usize, usize) {
    let mut opens = 0;
    let mut closes = 0;
    let mut escaped = false;
    let mut characters = line.chars().peekable();

    while let Some(character) = characters.next() {
        if *in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                *in_string = false;
            }
            continue;
        }
        if *in_char {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '\'' {
                *in_char = false;
            }
            continue;
        }

        match character {
            '/' if characters.peek() == Some(&'/') => break,
            '"' => *in_string = true,
            '\'' if starts_char_literal(&characters) => *in_char = true,
            '{' => opens += 1,
            '}' => closes += 1,
            _ => {}
        }
    }

    (opens, closes)
}

fn starts_char_literal(characters: &std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let mut lookahead = characters.clone();
    match lookahead.next() {
        Some('\\') => {
            while let Some(character) = lookahead.next() {
                if character == '\'' {
                    return true;
                }
                if character == '\\' {
                    lookahead.next();
                }
            }
            false
        }
        Some(_) => lookahead.next() == Some('\''),
        None => false,
    }
}

fn is_module_declaration(line: &str) -> bool {
    line.split_whitespace().next() == Some("mod")
}

fn public_type_name(line: &str) -> Option<&str> {
    let mut tokens = line.split_whitespace();
    if tokens.next() == Some("pub")
        && matches!(
            tokens.next(),
            Some("enum" | "struct" | "trait" | "type" | "union")
        )
    {
        let name = tokens.next()?;
        let end = name
            .char_indices()
            .find(|&(_, character)| matches!(character, '<' | '(' | '{' | ';' | '=' | ':'))
            .map_or(name.len(), |(index, _)| index);
        Some(&name[..end])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{Collision, Module, collisions, public_types};

    #[test]
    fn public_types_preserves_order_and_supported_kinds() {
        let source = "pub struct Alpha;\npub enum Beta {}\npub trait Gamma {}\npub type Delta = u8;\npub union Epsilon { value: u8 }";
        assert_eq!(
            public_types(source),
            ["Alpha", "Beta", "Gamma", "Delta", "Epsilon"]
        );
    }

    #[test]
    fn tuple_struct_name_excludes_parameters() {
        assert_eq!(public_types("pub struct Foo(u32);"), ["Foo"]);
        let modules = [
            Module {
                stem: "tuple".into(),
                source: "pub struct Foo(u32);".into(),
            },
            Module {
                stem: "enum".into(),
                source: "pub enum Foo {}".into(),
            },
        ];
        assert_eq!(collisions(&modules)[0].modules, ["enum", "tuple"]);
    }

    #[test]
    fn clause_one_reports_two_public_enum_definitions() {
        let modules = [
            Module {
                stem: "first".into(),
                source: "pub enum Fate {}".into(),
            },
            Module {
                stem: "second".into(),
                source: "pub enum Fate {}".into(),
            },
        ];
        assert_eq!(
            collisions(&modules),
            [Collision {
                name: "Fate".into(),
                modules: ["first", "second"].into_iter().map(String::from).collect()
            }]
        );
    }

    #[test]
    fn clause_two_mixes_public_item_kinds() {
        let modules = [
            Module {
                stem: "one".into(),
                source: "pub struct Plan;".into(),
            },
            Module {
                stem: "two".into(),
                source: "pub enum Plan {}".into(),
            },
        ];
        assert_eq!(collisions(&modules)[0].name, "Plan");
    }

    #[test]
    fn clause_three_ignores_mentions() {
        let modules = [
            Module {
                stem: "definition".into(),
                source: "pub enum Fate {}".into(),
            },
            Module {
                stem: "use".into(),
                source: "fn check(_: Fate) {}\n/// Fate".into(),
            },
        ];
        assert!(collisions(&modules).is_empty());
    }

    #[test]
    fn clause_four_deduplicates_a_repeated_module() {
        let modules = [
            Module {
                stem: "same".into(),
                source: "pub enum Fate {}\npub enum Fate {}".into(),
            },
            Module {
                stem: "same".into(),
                source: "pub enum Fate {}".into(),
            },
        ];
        assert!(collisions(&modules).is_empty());
    }

    #[test]
    fn clause_five_ignores_private_items() {
        let modules = [
            Module {
                stem: "private".into(),
                source: "enum Fate {}".into(),
            },
            Module {
                stem: "public".into(),
                source: "pub enum Fate {}".into(),
            },
        ];
        assert!(collisions(&modules).is_empty());
    }

    #[test]
    fn clause_six_ignores_cfg_test_modules() {
        let modules = [
            Module {
                stem: "api".into(),
                source: "pub enum Fate {}".into(),
            },
            Module {
                stem: "tests".into(),
                source: "#[cfg(test)]\nmod tests {\n    pub enum Fate {}\n}".into(),
            },
        ];
        assert!(collisions(&modules).is_empty());
    }

    #[test]
    fn cfg_test_attributes_stack() {
        let modules = [
            Module {
                stem: "api".into(),
                source: "pub enum Fate {}".into(),
            },
            Module {
                stem: "tests".into(),
                source: "#[cfg(test)]\n#[allow(dead_code)]\nmod tests {\n    pub enum Fate {}\n}"
                    .into(),
            },
        ];
        assert!(collisions(&modules).is_empty());
    }

    #[test]
    fn braces_in_strings_and_comments_do_not_escape_cfg_test_modules() {
        let modules = [
            Module {
                stem: "api".into(),
                source: "pub enum Fate {}".into(),
            },
            Module {
                stem: "tests".into(),
                source: "#[cfg(test)]\nmod tests {\n    let _ = \"{\\\"\";\n    let _ = '}'; // }\n    pub enum Fate {}\n}\npub enum Fate {}".into(),
            },
        ];
        assert_eq!(collisions(&modules)[0].modules, ["api", "tests"]);
    }

    #[test]
    fn clause_seven_sorts_names_and_modules() {
        let modules = [
            Module {
                stem: "zeta".into(),
                source: "pub enum Beta {}\npub enum Alpha {}".into(),
            },
            Module {
                stem: "alpha".into(),
                source: "pub enum Beta {}\npub enum Alpha {}".into(),
            },
        ];
        assert_eq!(
            collisions(&modules),
            [
                Collision {
                    name: "Alpha".into(),
                    modules: ["alpha", "zeta"].into_iter().map(String::from).collect()
                },
                Collision {
                    name: "Beta".into(),
                    modules: ["alpha", "zeta"].into_iter().map(String::from).collect()
                },
            ]
        );
    }

    #[test]
    fn boundaries_zero_one_empty_and_three_modules() {
        assert!(collisions(&[]).is_empty());
        assert!(
            collisions(&[Module {
                stem: "one".into(),
                source: "".into()
            }])
            .is_empty()
        );
        let modules = [
            Module {
                stem: "a".into(),
                source: "pub enum Fate {}".into(),
            },
            Module {
                stem: "b".into(),
                source: "pub enum Fate {}".into(),
            },
            Module {
                stem: "c".into(),
                source: "pub enum Fate {}".into(),
            },
        ];
        assert_eq!(collisions(&modules)[0].modules.len(), 3);
    }
}

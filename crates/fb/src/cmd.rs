//! Pure parsing of the `fb` command surface.
#![allow(dead_code)]
// The items in this module are `fb`'s public command-API surface. They are
// declared here but not yet wired into `main.rs`'s control flow, so a non-test
// `clippy`/`rustc` pass cannot see their callers. The unit tests in this file
// are their only current users; remove this once `main.rs` consumes the API.
//!
//! This module maps already-tokenised arguments to a decision. It performs no
//! I/O, spawns no processes, and uses no argument-parsing library. Keeping it
//! pure is what makes the whole command surface testable without running
//! anything, and is the contract that lets a planning agent script against the
//! CLI: branch on the exit code, parse the JSON, never crash on bad input.

/// The subcommands `fb` understands.
///
/// Each variant is the intent behind a single invocation. The parser never
/// touches the outside world to build one; it only records what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show farmerbob's current status.
    Status,
    /// Dispatch `task` to the given implementation `arms`.
    Dispatch { task: String, arms: Vec<String> },
    /// Score a single `task` against recorded runs.
    Score { task: String },
    /// Select a single `arm` as the answer for `task`.
    Select { task: String, arm: String },
    /// Show the per-arm leaderboard.
    Leaderboard,
    /// Show help, optionally for a specific `topic`.
    Help { topic: Option<String> },
    /// Print the version and exit.
    Version,
}

/// Global flags that may appear before or after the subcommand.
///
/// Exactly `--json`, `--quiet` and `--dry-run` are recognised; this struct
/// exposes no other state, so an unknown flag can never silently be swallowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    /// Emit machine-readable JSON instead of prose.
    pub json: bool,
    /// Suppress non-essential output.
    pub quiet: bool,
    /// Compute the action but perform no side effects.
    pub dry_run: bool,
}

/// A fully parsed invocation: intent plus the global flags that governed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// What the user asked for.
    pub command: Command,
    /// Global flags that applied to the whole invocation.
    pub flags: Flags,
}

/// Why a tokenised argument list could not be turned into an [`Invocation`].
///
/// Every variant is a *usage* error and therefore exits `2`. The variants are
/// deliberately structured so an agent can branch on the cause: an unknown
/// command names what was expected (and offers a suggestion via [`suggest`]),
/// while missing or unknown input names exactly what was wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A subcommand was given that does not exist. `known` is the full, sorted
    /// list of valid subcommands; see [`known_commands`].
    UnknownCommand { given: String, known: Vec<String> },
    /// A required positional argument was absent.
    MissingArgument { command: String, argument: String },
    /// A token beginning with `-` was not one of the recognised flags.
    UnknownFlag { given: String },
    /// No subcommand was supplied at all.
    NoCommand,
}

impl ParseError {
    /// The process exit code for this error.
    ///
    /// Every parse error is a usage error and always exits `2`, never `1`. This
    /// lets an agent distinguish "I called this wrong" from "the operation
    /// failed" without reading any prose.
    pub fn exit_code(&self) -> i32 {
        2
    }
}

/// The canonical subcommand names.
///
/// This is the single source of truth for both the help text and the `known`
/// list reported by [`ParseError::UnknownCommand`]. The returned vector is
/// sorted so help and error output are stable and deterministic.
const COMMAND_NAMES: &[&str] = &[
    "dispatch",
    "help",
    "leaderboard",
    "score",
    "select",
    "status",
    "version",
];

/// Parse already-tokenised arguments, NOT including `argv[0]`.
///
/// Flags (`--json`, `--quiet`, `--dry-run`) may appear before or after the
/// subcommand and in any order. A bare `--` ends flag parsing: every later
/// token is positional even if it begins with `-`. The function is total — no
/// input whatsoever may panic.
///
/// # Errors
///
/// Returns a [`ParseError`] describing exactly what was wrong: an unknown
/// subcommand, a missing positional, an unrecognised flag, or no subcommand.
pub fn parse(args: &[String]) -> Result<Invocation, ParseError> {
    let mut flags = Flags::default();
    let mut positionals: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            while i < args.len() {
                positionals.push(args[i].clone());
                i += 1;
            }
            break;
        }
        if arg.starts_with('-') {
            if arg == "--json" {
                flags.json = true;
            } else if arg == "--quiet" {
                flags.quiet = true;
            } else if arg == "--dry-run" {
                flags.dry_run = true;
            } else {
                return Err(ParseError::UnknownFlag { given: arg.clone() });
            }
            i += 1;
        } else {
            positionals.push(arg.clone());
            i += 1;
        }
    }

    let command = match positionals.first() {
        None => return Err(ParseError::NoCommand),
        Some(name) => match name.as_str() {
            "status" => Command::Status,
            "dispatch" => {
                let task = positionals.get(1).ok_or_else(missing_argument_fn(
                    "dispatch",
                    "task",
                ))?;
                let arms = positionals.get(2..).ok_or_else(missing_argument_fn(
                    "dispatch",
                    "arm",
                ))?;
                if arms.is_empty() {
                    return Err(ParseError::MissingArgument {
                        command: "dispatch".to_string(),
                        argument: "arm".to_string(),
                    });
                }
                Command::Dispatch {
                    task: task.clone(),
                    arms: arms.to_vec(),
                }
            }
            "score" => {
                let task = positionals.get(1).ok_or_else(missing_argument_fn(
                    "score",
                    "task",
                ))?;
                Command::Score { task: task.clone() }
            }
            "select" => {
                let task = positionals.get(1).ok_or_else(missing_argument_fn(
                    "select",
                    "task",
                ))?;
                let arm = positionals.get(2).ok_or_else(missing_argument_fn(
                    "select",
                    "arm",
                ))?;
                Command::Select {
                    task: task.clone(),
                    arm: arm.clone(),
                }
            }
            "leaderboard" => Command::Leaderboard,
            "help" => Command::Help {
                topic: positionals.get(1).cloned(),
            },
            "version" => Command::Version,
            other => {
                return Err(ParseError::UnknownCommand {
                    given: other.to_string(),
                    known: known_commands().iter().map(|s| s.to_string()).collect(),
                });
            }
        },
    };

    Ok(Invocation { command, flags })
}

/// Build the closure that produces a [`ParseError::MissingArgument`].
fn missing_argument_fn(
    command: &'static str,
    argument: &'static str,
) -> impl Fn() -> ParseError {
    move || ParseError::MissingArgument {
        command: command.to_string(),
        argument: argument.to_string(),
    }
}

/// The sorted list of known subcommand names.
///
/// This is the single source of truth for help text and for the `known` field
/// of [`ParseError::UnknownCommand`]; do not hand-list subcommands anywhere
/// else.
pub fn known_commands() -> Vec<&'static str> {
    let mut v: Vec<&str> = COMMAND_NAMES.to_vec();
    v.sort_unstable();
    v
}

/// The closest known command to a misspelling, by case-insensitive Levenshtein
/// distance, when that distance is at most `2`.
///
/// Returns `None` otherwise. A wrong guess is worse than none, so a distance
/// greater than `2` is deliberately not reported.
pub fn suggest(given: &str) -> Option<&'static str> {
    let g = given.to_lowercase();
    let mut best: Option<(&'static str, usize)> = None;
    for name in COMMAND_NAMES {
        let d = levenshtein(&g, &name.to_lowercase());
        match best {
            Some((_, bd)) if d >= bd => {}
            _ => best = Some((name, d)),
        }
    }
    match best {
        Some((name, d)) if d <= 2 => Some(name),
        _ => None,
    }
}

/// Case-insensitive-insensitive here only via the caller lowercasing inputs:
/// classic Levenshtein edit distance over `char`s.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = a.len();
    let m = b.len();
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flags_before_and_after_subcommand_same_invocation() {
        let before = parse(&args(&["--json", "--quiet", "status"])).unwrap();
        let after = parse(&args(&["status", "--quiet", "--json"])).unwrap();
        assert_eq!(before, after);

        let mixed1 = parse(&args(&["--dry-run", "score", "t1", "--json"])).unwrap();
        let mixed2 = parse(&args(&["score", "--json", "t1", "--dry-run"])).unwrap();
        assert_eq!(mixed1, mixed2);
    }

    #[test]
    fn double_dash_makes_later_json_positional() {
        // The `--json` after `--` is an arm, not the global flag.
        let inv = parse(&args(&["dispatch", "mytask", "--", "--json"])).unwrap();
        match inv.command {
            Command::Dispatch { task, arms } => {
                assert_eq!(task, "mytask");
                assert_eq!(arms, vec!["--json".to_string()]);
            }
            other => panic!("expected dispatch, got {other:?}"),
        }
        assert!(!inv.flags.json);
    }

    #[test]
    fn empty_args_is_no_command() {
        assert_eq!(parse(&args(&[])), Err(ParseError::NoCommand));
    }

    #[test]
    fn dispatch_without_arms_names_missing_argument() {
        // Task present, arms absent -> the missing argument is "arm".
        assert_eq!(
            parse(&args(&["dispatch", "mytask"])),
            Err(ParseError::MissingArgument {
                command: "dispatch".to_string(),
                argument: "arm".to_string(),
            })
        );
        // No task at all -> missing argument is "task".
        assert_eq!(
            parse(&args(&["dispatch"])),
            Err(ParseError::MissingArgument {
                command: "dispatch".to_string(),
                argument: "task".to_string(),
            })
        );
    }

    #[test]
    fn unknown_flag_distinct_from_unknown_command() {
        let flag_err = parse(&args(&["--nope"])).unwrap_err();
        let cmd_err = parse(&args(&["bogus"])).unwrap_err();
        assert!(matches!(flag_err, ParseError::UnknownFlag { .. }));
        assert!(matches!(cmd_err, ParseError::UnknownCommand { .. }));
        assert_ne!(flag_err, cmd_err);
    }

    #[test]
    fn every_error_exits_two() {
        let cases: Vec<Vec<String>> = vec![
            args(&[]),
            args(&["--bogus"]),
            args(&["frobnicate"]),
            args(&["dispatch"]),
            args(&["dispatch", "only-task"]),
            args(&["score"]),
            args(&["select", "t"]),
        ];
        for c in &cases {
            match parse(c) {
                Ok(_) => panic!("expected an error for {c:?}"),
                Err(e) => assert_eq!(e.exit_code(), 2, "case {c:?}"),
            }
        }
    }

    #[test]
    fn suggest_finds_one_edit_typo_and_refuses_distant() {
        assert_eq!(suggest("stauts"), Some("status"));
        assert_eq!(suggest("leadrboard"), Some("leaderboard"));
        assert_eq!(suggest("vereion"), Some("version"));
        assert_eq!(suggest("DISPATCH"), Some("dispatch"));
        assert_eq!(suggest("xyzzy"), None);
        assert_eq!(suggest(""), None);
    }

    #[test]
    fn known_commands_sorted_and_complete() {
        let k = known_commands();
        let mut sorted = k.clone();
        sorted.sort_unstable();
        assert_eq!(k, sorted);
        assert!(k.contains(&"dispatch"));
        assert!(k.contains(&"status"));
    }

    #[test]
    fn help_topic_parses_even_if_unknown() {
        // Reporting an unknown topic is the help command's job, not the parser's.
        let inv = parse(&args(&["help", "nonexistent-topic"])).unwrap();
        assert_eq!(
            inv.command,
            Command::Help {
                topic: Some("nonexistent-topic".to_string())
            }
        );
    }

    #[test]
    fn malformed_inputs_never_panic() {
        let table: Vec<Vec<String>> = vec![
            args(&[]),
            args(&["--"]),
            args(&["--", "--"]),
            args(&["-"]),
            args(&["--json=yes"]),
            args(&["--json", "--", "--json"]),
            args(&["dispatch", "--json", "task"]),
            args(&["select"]),
            args(&["select", "t", "a", "extra"]),
            args(&["version", "extra", "stuff"]),
            args(&["leaderboard", "--quiet", "unexpected"]),
            args(&["---"]),
            args(&["status", "--dry-run", "dispatch"]),
        ];
        for c in &table {
            let _ = parse(c);
        }
    }

    #[test]
    fn dispatch_with_one_arm_parses() {
        let inv = parse(&args(&["dispatch", "task", "arm1"])).unwrap();
        assert_eq!(
            inv.command,
            Command::Dispatch {
                task: "task".to_string(),
                arms: vec!["arm1".to_string()],
            }
        );
    }
}

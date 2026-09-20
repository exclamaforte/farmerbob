//! Print what a task spec declares, for the shell to consume.
//!
//! One reader, one answer. [`run`] reads the spec file and asks
//! `farmerbob_core::target_decl::declared` what it declares, then renders
//! that answer and maps it to an exit code. This module parses nothing
//! itself: a regex or a scan here would be the fourth reader this
//! subcommand exists to retire.
#![allow(dead_code)]
// `run` and `Format` are the subcommand's API surface, declared here but not
// yet wired into `main.rs`'s argument parser -- wiring is a separate task, and
// adding the flags now would be a scope violation. The unit tests in this
// file are their only current users; remove this once `main.rs` consumes the
// API.

use std::io::Write;
use std::path::Path;

use farmerbob_core::target_decl::{self, Declaration, NoDeclaration};

/// How the answer is rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    /// `<verb> <path>`, one line, no trailing spaces. The shell splits on the
    /// first space.
    Line,
    /// The path alone, one line.
    PathOnly,
    /// The verb alone, one line: `creates` or `modifies`.
    VerbOnly,
}

/// Print what a spec declares, for the shell to consume.
///
/// Reads the file at `spec_path` and writes to `out`. Returns the process
/// exit code the caller should use:
///
/// - `0` — exactly one declaration; the answer is the one line written to
///   `out` in the requested format.
/// - `2` — the spec declares none.
/// - `3` — the spec declares more than one, or a marker was refused.
/// - `4` — the file could not be read at all.
///
/// Nothing is written to `out` for any non-zero code, so a caller that
/// captures the output and one that checks the exit code get the same
/// answer.
pub fn run(spec_path: &Path, format: Format, out: &mut dyn Write) -> i32 {
    run_explained(spec_path, format, out, &mut std::io::sink())
}

/// Print what a spec declares, for a machine to consume.
///
/// `out` receives the answer and nothing else. `why` receives a human-readable
/// diagnosis only when a marker was refused. Returns the same process exit
/// codes as [`run`].
pub fn run_explained(
    spec_path: &Path,
    format: Format,
    out: &mut dyn Write,
    why: &mut dyn Write,
) -> i32 {
    let spec = match std::fs::read_to_string(spec_path) {
        Ok(spec) => spec,
        Err(_) => return 4,
    };
    // declared_all, not declared: a task may declare several files, and `declared` reports
    // that as Ambiguous. It returned 3 and printed NOTHING for the first two-marker spec
    // written, and `fb_target` turned that into an empty string with exit 0 -- a silent
    // failure that would have dispatched a task whose target nothing could resolve.
    let declarations = match target_decl::declared_all(&spec) {
        Ok(declarations) => declarations,
        Err(NoDeclaration::Absent) => return 2,
        Err(NoDeclaration::Ambiguous(_)) => return 3,
        Err(reason @ NoDeclaration::Refused { .. }) => {
            if let Some(message) = diagnose(&reason) {
                let _ = writeln!(why, "{message}");
            }
            return 3;
        }
    };
    // One line per declaration, in the order written. A single-deliverable spec still
    // prints exactly one line, so every existing caller reads what it always did.
    let line = declarations
        .iter()
        .map(|declaration| {
            let verb = match declaration {
                Declaration::Creates(_) => "creates",
                Declaration::Modifies(_) => "modifies",
            };
            let path = target_decl::path(declaration);
            match format {
                Format::Line => format!("{verb} {path}"),
                Format::PathOnly => path.to_string(),
                Format::VerbOnly => verb.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    // Every non-zero return above happens before anything is written, so the
    // caller sees either one line and 0, or nothing and non-zero. A failed
    // write (a closed pipe, say) is the caller's condition to observe, not a
    // different answer about the spec; the four codes are about the spec, and
    // inventing a fifth for I/O would break the shell's contract.
    let _ = writeln!(out, "{line}");
    0
}

/// Explain one refused declaration, including its 1-based line and a correct
/// marker shape. Non-refusal outcomes intentionally have no diagnosis.
pub fn diagnose(reason: &NoDeclaration) -> Option<String> {
    let NoDeclaration::Refused { line, reason } = reason else {
        return None;
    };

    let lower = reason.to_ascii_lowercase();
    let problem = if lower.contains("no whitespace") || lower.contains("opener") {
        "put a space after `<!--`"
    } else if lower.contains("tab") {
        "replace the tab with spaces"
    } else if lower.contains("trailing text") || lower.contains("after marker closer") {
        "remove the text after `-->`"
    } else if lower.contains("unknown marker word") || lower.contains("unknown word") {
        "use `creates` or `modifies` as the marker word"
    } else if lower.contains("multiple markers") {
        "put one marker per line"
    } else {
        let detail = if reason.trim().is_empty() {
            "the marker was refused"
        } else {
            reason.as_str()
        };
        return Some(format!(
            "line {line}: {detail}; write a marker like `<!-- fb:creates PATH -->`"
        ));
    };

    Some(format!(
        "line {line}: {problem}; write a marker like `<!-- fb:creates PATH -->`"
    ))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Format, diagnose, run, run_explained};
    use farmerbob_core::target_decl::{self, NoDeclaration};

    /// A spec file on disk that removes itself when the test ends.
    struct TempSpec {
        path: PathBuf,
    }

    impl TempSpec {
        fn new(name: &str, contents: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("fb_decl_cmd_{}_{}", std::process::id(), name));
            std::fs::write(&path, contents).unwrap();
            TempSpec { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempSpec {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// The exit code the pinned contract assigns to what `declared` reports:
    /// `Ok` is 0, `Absent` is 2, `Ambiguous` or `Refused` is 3. Clause 10's
    /// oracle, so the agreement is asserted without re-asserting the parse.
    // declared_all, because that is what `run` calls now. Agreement with `declared` would
    // reassert the one-file contract this command no longer has.
    fn code_of(contents: &str) -> i32 {
        match target_decl::declared_all(contents) {
            Ok(_) => 0,
            Err(target_decl::NoDeclaration::Absent) => 2,
            Err(_) => 3,
        }
    }

    #[test]
    fn clause_1_one_marker_line_format() {
        let spec = TempSpec::new("clause1", "<!-- fb:creates crates/a/b.rs -->\n");
        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 0);
        assert_eq!(out, b"creates crates/a/b.rs\n");
    }

    #[test]
    fn clause_2_path_only_and_verb_only() {
        let spec = TempSpec::new("clause2", "<!-- fb:creates crates/a/b.rs -->\n");

        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::PathOnly, &mut out), 0);
        assert_eq!(out, b"crates/a/b.rs\n");

        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::VerbOnly, &mut out), 0);
        assert_eq!(out, b"creates\n");
    }

    #[test]
    fn clause_3_modifies_marker_yields_modifies() {
        let spec = TempSpec::new("clause3", "<!-- fb:modifies crates/a/b.rs -->\n");

        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 0);
        assert_eq!(out, b"modifies crates/a/b.rs\n");

        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::VerbOnly, &mut out), 0);
        assert_eq!(out, b"modifies\n");
    }

    #[test]
    fn clause_4_only_marker_inside_a_fence() {
        let spec = TempSpec::new("clause4", "```\n<!-- fb:creates crates/a/b.rs -->\n```\n");
        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 2);
        assert!(out.is_empty());
    }

    #[test]
    fn clause_5_no_marker_at_all() {
        let spec = TempSpec::new("clause5", "# Task\n\nProse, but no marker anywhere.\n");
        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 2);
        assert!(out.is_empty());
    }

    /// Clause 7: this differs from `clause_1_one_marker_line_format` only in
    /// the number of markers.
    #[test]
    /// Two markers are two deliverables, in the order written, and each keeps its own verb.
    /// This asserted 3-and-print-nothing while a task could declare exactly one file. It
    /// cannot any more, and the silent empty it produced would have dispatched a task whose
    /// target nothing could resolve.
    fn clause_6_two_markers_outside_fences() {
        let spec = TempSpec::new(
            "clause6",
            "<!-- fb:creates crates/a/b.rs -->\n<!-- fb:modifies crates/c/d.rs -->\n",
        );
        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 0);
        assert_eq!(
            String::from_utf8_lossy(&out).trim(),
            "creates crates/a/b.rs\nmodifies crates/c/d.rs"
        );
    }

    /// Clause 8: the same call shape must give different codes for a missing
    /// file and for a spec that declares nothing.
    #[test]
    fn clause_8_missing_file_is_4_not_2() {
        let missing =
            std::env::temp_dir().join(format!("fb_decl_cmd_{}_missing.md", std::process::id()));
        let markerless = TempSpec::new("clause8", "no marker anywhere\n");

        let mut out_missing = Vec::new();
        let missing_code = run(&missing, Format::Line, &mut out_missing);
        let mut out_none = Vec::new();
        let none_code = run(markerless.path(), Format::Line, &mut out_none);

        assert_eq!(missing_code, 4);
        assert_eq!(none_code, 2);
        assert_ne!(missing_code, none_code);
        assert!(out_missing.is_empty());
        assert!(out_none.is_empty());
    }

    /// Clause 9, as one test over all three non-zero codes.
    #[test]
    fn clause_9_every_non_zero_code_writes_nothing() {
        let none = TempSpec::new("clause9_none", "no marker\n");
        // A REFUSED marker is the remaining 3: two markers are now two deliverables, so a
        // malformed one is what is left that cannot be placed.
        let ambiguous = TempSpec::new("clause9_two", "<!-- fb:deletes gone.rs -->\n");
        let missing =
            std::env::temp_dir().join(format!("fb_decl_cmd_{}_clause9.md", std::process::id()));

        for (expected, spec_path) in [
            (2, none.path()),
            (3, ambiguous.path()),
            (4, missing.as_path()),
        ] {
            let mut out = Vec::new();
            assert_eq!(run(spec_path, Format::Line, &mut out), expected);
            assert!(out.is_empty(), "exit {expected} must leave out empty");
        }
    }

    /// Clause 10: agreement with `target_decl::declared` on inputs spanning
    /// every answer it can give, rather than a re-statement of the parse.
    #[test]
    fn clause_10_agrees_with_target_decl_on_every_input() {
        let inputs = [
            "",
            "# no marker\n",
            "<!-- fb:creates crates/a/b.rs -->\n",
            "<!-- fb:modifies crates/a/b.rs -->",
            "```\n<!-- fb:creates fenced.rs -->\n```\nprose\n",
            "<!-- fb:creates one.rs -->\n<!-- fb:modifies two.rs -->\n",
            "<!--\tfb:creates tab.rs -->\n",
            "<!-- fb:deletes gone.rs -->\n",
            "<!-- fb:creates -->\n",
            "<!-- fb:creates only.rs -->",
            "```text\n<!-- fb:creates example.rs -->\n```\n<!-- fb:creates real.rs -->\n",
        ];
        for (i, contents) in inputs.iter().enumerate() {
            let spec = TempSpec::new(&format!("clause10_{i}"), contents);
            let mut out = Vec::new();
            let code = run(spec.path(), Format::Line, &mut out);
            assert_eq!(code, code_of(contents), "input {i}: {contents:?}");
            if code == 0 {
                let text = std::str::from_utf8(&out).unwrap();
                assert!(text.ends_with('\n'), "input {i}: one trailing newline");
                // One line PER DECLARATION. A task may declare several files, so a fixed
                // count of one would reassert the contract this command no longer has.
                assert_eq!(
                    text.lines().count(),
                    target_decl::declared_all(contents)
                        .map(|d| d.len())
                        .unwrap_or(0),
                    "input {i}: one line per declaration"
                );
            } else {
                assert!(out.is_empty(), "input {i}: exit {code} writes nothing");
            }
        }
    }

    #[test]
    fn boundary_empty_file_declares_none_but_was_read() {
        let spec = TempSpec::new("boundary_empty", "");
        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 2);
        assert!(out.is_empty());
    }

    #[test]
    fn boundary_file_is_exactly_one_marker() {
        let spec = TempSpec::new("boundary_one_marker", "<!-- fb:creates crates/a/b.rs -->");
        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 0);
        assert_eq!(out, b"creates crates/a/b.rs\n");
    }

    #[test]
    fn three_markers_are_three_deliverables() {
        let spec = TempSpec::new(
            "boundary_three",
            "<!-- fb:creates a.rs -->\n<!-- fb:modifies b.rs -->\n<!-- fb:creates c.rs -->\n",
        );
        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 0);
        assert_eq!(
            String::from_utf8_lossy(&out).trim(),
            "creates a.rs\nmodifies b.rs\ncreates c.rs"
        );
    }

    /// Confirmed against `target_decl`: a marker with no path is silence
    /// (`Absent`), not a refusal, so the matching code is 2.
    #[test]
    fn boundary_marker_without_path_matches_target_decl() {
        let contents = "<!-- fb:creates -->\n";
        let spec = TempSpec::new("boundary_no_path", contents);
        let mut out = Vec::new();
        let code = run(spec.path(), Format::Line, &mut out);
        assert_eq!(code, code_of(contents));
        assert_eq!(code, 2);
        assert!(out.is_empty());
    }

    #[test]
    fn boundary_directory_returns_4() {
        let mut out = Vec::new();
        assert_eq!(run(&std::env::temp_dir(), Format::Line, &mut out), 4);
        assert!(out.is_empty());
    }

    #[test]
    fn refused_marker_is_explained_on_its_line_and_not_in_out() {
        let spec = TempSpec::new("explained_refusal", "# heading\n<!--fb:creates a.rs -->\n");
        let mut out = Vec::new();
        let mut why = Vec::new();
        assert_eq!(
            run_explained(spec.path(), Format::Line, &mut out, &mut why),
            3
        );
        let message = String::from_utf8_lossy(&why).to_ascii_lowercase();
        assert!(out.is_empty());
        assert!(message.contains("line 2"));
        assert!(message.contains("space after `<!--`"));
        assert!(message.contains("<!-- fb:creates path -->"));
    }

    #[test]
    fn diagnosis_maps_known_reasons_and_has_a_fallback() {
        let cases = [
            ("tab in separator where spaces required", "tab"),
            ("trailing text after marker closer", "text after `-->`"),
            ("unknown marker word", "creates"),
            ("multiple markers on a single line", "one marker per line"),
            ("no whitespace between opener and fb:", "space after `<!--`"),
            ("new parser reason", "new parser reason"),
        ];
        for (reason, expected) in cases {
            let refusal = NoDeclaration::Refused {
                line: 17,
                reason: reason.to_string(),
            };
            let message = diagnose(&refusal).expect("refusals have diagnoses");
            assert!(message.to_ascii_lowercase().contains(expected));
            assert!(message.contains("17"));
            assert!(message.contains("<!-- fb:creates PATH -->"));
        }
    }

    #[test]
    fn only_refusals_are_diagnosed() {
        assert_eq!(diagnose(&NoDeclaration::Absent), None);
        assert_eq!(
            diagnose(&NoDeclaration::Ambiguous(vec!["a.rs".into()])),
            None
        );
    }

    #[test]
    fn explained_exit_codes_and_stream_composition() {
        let absent = TempSpec::new("explained_absent", "");
        let valid = TempSpec::new("explained_valid", "<!-- fb:creates a.rs -->\n");
        let missing = std::env::temp_dir().join(format!(
            "fb_decl_cmd_{}_explained_missing.md",
            std::process::id()
        ));

        let mut out = Vec::new();
        let mut why = Vec::new();
        assert_eq!(
            run_explained(absent.path(), Format::Line, &mut out, &mut why),
            2
        );
        assert!(out.is_empty() && why.is_empty());

        let mut out = Vec::new();
        let mut why = Vec::new();
        assert_eq!(
            run_explained(valid.path(), Format::Line, &mut out, &mut why),
            0
        );
        assert!(!out.is_empty() && why.is_empty());

        let mut out = Vec::new();
        let mut why = Vec::new();
        assert_eq!(run_explained(&missing, Format::Line, &mut out, &mut why), 4);
        assert!(out.is_empty() && why.is_empty());
    }
}

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
    let spec = match std::fs::read_to_string(spec_path) {
        Ok(spec) => spec,
        Err(_) => return 4,
    };
    let declaration = match target_decl::declared(&spec) {
        Ok(declaration) => declaration,
        Err(NoDeclaration::Absent) => return 2,
        Err(NoDeclaration::Ambiguous(_)) | Err(NoDeclaration::Refused { .. }) => return 3,
    };
    let verb = match &declaration {
        Declaration::Creates(_) => "creates",
        Declaration::Modifies(_) => "modifies",
    };
    let path = target_decl::path(&declaration);
    let line = match format {
        Format::Line => format!("{verb} {path}"),
        Format::PathOnly => path.to_string(),
        Format::VerbOnly => verb.to_string(),
    };
    // Every non-zero return above happens before anything is written, so the
    // caller sees either one line and 0, or nothing and non-zero. A failed
    // write (a closed pipe, say) is the caller's condition to observe, not a
    // different answer about the spec; the four codes are about the spec, and
    // inventing a fifth for I/O would break the shell's contract.
    let _ = writeln!(out, "{line}");
    0
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Format, run};
    use farmerbob_core::target_decl;

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
    fn code_of(contents: &str) -> i32 {
        match target_decl::declared(contents) {
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
    fn clause_6_two_markers_outside_fences() {
        let spec = TempSpec::new(
            "clause6",
            "<!-- fb:creates crates/a/b.rs -->\n<!-- fb:modifies crates/c/d.rs -->\n",
        );
        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 3);
        assert!(out.is_empty());
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
        let ambiguous = TempSpec::new(
            "clause9_two",
            "<!-- fb:creates a.rs -->\n<!-- fb:modifies b.rs -->\n",
        );
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
                assert_eq!(text.lines().count(), 1, "input {i}: exactly one line");
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
    fn boundary_three_markers_return_3_like_two() {
        let spec = TempSpec::new(
            "boundary_three",
            "<!-- fb:creates a.rs -->\n<!-- fb:modifies b.rs -->\n<!-- fb:creates c.rs -->\n",
        );
        let mut out = Vec::new();
        assert_eq!(run(spec.path(), Format::Line, &mut out), 3);
        assert!(out.is_empty());
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
}

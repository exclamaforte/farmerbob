//! Timing table rendering from run file timestamps.
//!
//! Ports `fb-timing.sh` to use `farmerbob_core::run_timing::derive` for all
//! time and throughput arithmetic, ensuring the shell and core agree.
#![allow(dead_code)]
// Uncalled pub fn in a binary crate trips dead_code under -D warnings;
// wiring into main.rs's argument parser is a separate task.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use farmerbob_core::measurement::Measurement;
use farmerbob_core::run_timing::{self, Wrote};

/// One run to report on.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingRow {
    /// Arm name.
    pub arm: String,
    /// When the run began, seconds since the epoch.
    pub started_s: i64,
    /// The files it wrote, with their mtimes, in any order.
    pub wrote: Vec<Wrote>,
    /// Lines added across those files, as the caller counted them.
    pub lines: u32,
}

/// The marker an absent figure renders as. Exactly this string.
pub const ABSENT: &str = "--";

/// Read one task's candidate worktrees into rows.
///
/// `fb timing` was wired to `run(&[], ..)` -- it discarded its TASK argument and printed a
/// header over an empty table, for as long as the command has existed. fb-timing.sh did the
/// real work in awk and stat. This is that work, in Rust, and the script is retired with it.
///
/// `wt_root` is a parameter so this is testable against a scratch directory rather than the
/// live worktree store.
pub fn gather(task: &str, wt_root: &Path) -> Vec<TimingRow> {
    let prefix = format!("{task}--");
    let Ok(entries) = fs::read_dir(wt_root) else {
        return Vec::new();
    };
    let mut rows: Vec<TimingRow> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(arm) = name.strip_prefix(&prefix) else {
            continue;
        };
        // A critic's scratch checkout is not a candidate. fb-timing.sh listed
        // `critic--gemini-38-flash` beside the real arm with dashes for every column,
        // because it matched the same prefix.
        if arm.starts_with("critic--") {
            continue;
        }
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let started_s = fs::metadata(dir.join(".git"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        let mut wrote: Vec<Wrote> = Vec::new();
        for path in changed_paths(&dir) {
            let Ok(meta) = fs::metadata(dir.join(&path)) else {
                continue;
            };
            let mtime_s = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            wrote.push(Wrote { path, mtime_s });
        }
        let lines = added_lines(&dir);
        rows.push(TimingRow {
            arm: arm.to_string(),
            started_s,
            wrote,
            lines,
        });
    }
    rows.sort_by(|a, b| a.arm.cmp(&b.arm));
    rows
}

/// Tracked edits plus untracked additions, deduplicated, in sorted order.
fn changed_paths(dir: &Path) -> Vec<String> {
    let mut paths: Vec<String> = git_lines(dir, &["diff", "--name-only", "HEAD"]);
    paths.extend(git_lines(
        dir,
        &["ls-files", "--others", "--exclude-standard"],
    ));
    paths.sort();
    paths.dedup();
    paths
}

/// Lines the run added: the diff's insertions plus every line of a created file, which the
/// diff cannot see because the file is untracked.
fn added_lines(dir: &Path) -> u32 {
    let mut total: u32 = git_lines(dir, &["diff", "--numstat", "HEAD"])
        .iter()
        .filter_map(|l| l.split_whitespace().next()?.parse::<u32>().ok())
        .sum();
    for path in git_lines(dir, &["ls-files", "--others", "--exclude-standard"]) {
        if let Ok(text) = fs::read_to_string(dir.join(&path)) {
            total = total.saturating_add(text.lines().count() as u32);
        }
    }
    total
}

fn git_lines(dir: &Path, args: &[&str]) -> Vec<String> {
    let Ok(out) = Command::new("git").current_dir(dir).args(args).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Render a timing table.
///
/// Columns are arm, ttfw, span, lines, rate, in that order. A `Missing`
/// figure renders as `ABSENT`, never as a number and never blank.
pub fn table(rows: &[TimingRow]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<22} {:>7} {:>7} {:>7} {:>9}\n",
        "ARM", "TTFW", "SPAN", "LINES", "RATE"
    ));

    for row in rows {
        let measurement = run_timing::derive(row.started_s, &row.wrote, row.lines);
        let (ttfw, span, lines, rate) = match measurement {
            Measurement::Observed(timing) => {
                let rate_str = match timing.lines_per_min {
                    Measurement::Observed(r) => format!("{r}"),
                    Measurement::Missing(_) => ABSENT.to_string(),
                };
                (
                    timing.ttfw_s.to_string(),
                    timing.span_s.to_string(),
                    timing.lines.to_string(),
                    rate_str,
                )
            }
            Measurement::Missing(_) => (
                ABSENT.to_string(),
                ABSENT.to_string(),
                ABSENT.to_string(),
                ABSENT.to_string(),
            ),
        };
        out.push_str(&format!(
            "{:<22} {:>7} {:>7} {:>7} {:>9}\n",
            row.arm, ttfw, span, lines, rate
        ));
    }

    out
}

/// Render and write. Returns the exit code the caller should use.
pub fn run(rows: &[TimingRow], out: &mut dyn Write) -> i32 {
    let rendered = table(rows);
    let _ = out.write_all(rendered.as_bytes());
    if rows.is_empty() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use farmerbob_core::run_timing::{Wrote, derive};

    #[test]
    fn clause_1_non_empty_row_matches_derive_output() {
        let files = [
            Wrote {
                path: "src/a.rs".to_string(),
                mtime_s: 120,
            },
            Wrote {
                path: "src/b.rs".to_string(),
                mtime_s: 200,
            },
        ];
        let row = TimingRow {
            arm: "test-arm-1".to_string(),
            started_s: 100,
            wrote: files.to_vec(),
            lines: 40,
        };

        let expected_meas = derive(row.started_s, &row.wrote, row.lines);
        let expected = match expected_meas {
            Measurement::Observed(t) => t,
            Measurement::Missing(_) => unreachable!(),
        };

        let output = table(&[row]);
        assert!(output.contains("test-arm-1"));
        assert!(output.contains(&expected.ttfw_s.to_string()));
        assert!(output.contains(&expected.span_s.to_string()));
        assert!(output.contains(&expected.lines.to_string()));
        if let Measurement::Observed(rate) = expected.lines_per_min {
            assert!(output.contains(&rate.to_string()));
        }
    }

    #[test]
    fn clause_2_and_3_pinned_as_pair_contrast_absent_levels() {
        // Clause 2: empty wrote renders with ABSENT figures
        let empty_row = TimingRow {
            arm: "empty-arm".to_string(),
            started_s: 100,
            wrote: Vec::new(),
            lines: 0,
        };
        let out_empty = table(&[empty_row]);
        assert!(out_empty.contains("empty-arm"));
        assert!(out_empty.contains(ABSENT));

        // Clause 3: single file has observed ttfw and span (0), but ABSENT rate
        let single_file = [Wrote {
            path: "src/one.rs".to_string(),
            mtime_s: 150,
        }];
        let single_row = TimingRow {
            arm: "single-arm".to_string(),
            started_s: 100,
            wrote: single_file.to_vec(),
            lines: 15,
        };
        let out_single = table(&[single_row]);
        assert!(out_single.contains("single-arm"));
        assert!(out_single.contains("50")); // ttfw: 150 - 100
        assert!(out_single.contains(ABSENT)); // rate is absent

        // Clause 4: verify that they render differently as a pair
        assert_ne!(out_empty, out_single);
    }

    #[test]
    fn clause_5_input_order_preserved_exactly() {
        let rows = [
            TimingRow {
                arm: "arm-first".to_string(),
                started_s: 100,
                wrote: Vec::new(),
                lines: 0,
            },
            TimingRow {
                arm: "arm-second".to_string(),
                started_s: 100,
                wrote: Vec::new(),
                lines: 0,
            },
            TimingRow {
                arm: "arm-third".to_string(),
                started_s: 100,
                wrote: Vec::new(),
                lines: 0,
            },
        ];

        let out = table(&rows);
        let pos1 = out.find("arm-first").unwrap();
        let pos2 = out.find("arm-second").unwrap();
        let pos3 = out.find("arm-third").unwrap();

        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
    }

    #[test]
    fn clause_6_heading_mentions_all_five_columns_case_insensitively() {
        let out = table(&[]);
        let first_line = out.lines().next().unwrap_or("").to_lowercase();

        assert!(first_line.contains("arm"));
        assert!(first_line.contains("ttfw"));
        assert!(first_line.contains("span"));
        assert!(first_line.contains("lines"));
        assert!(first_line.contains("rate"));
    }

    #[test]
    fn clause_7_run_writes_table_output_and_returns_correct_exit_codes() {
        let rows = [TimingRow {
            arm: "arm-test".to_string(),
            started_s: 100,
            wrote: Vec::new(),
            lines: 0,
        }];

        // Non-empty rows: returns 0 and writes table output
        let mut out_non_empty = Vec::new();
        let code_non_empty = run(&rows, &mut out_non_empty);
        assert_eq!(code_non_empty, 0);
        assert_eq!(String::from_utf8(out_non_empty).unwrap(), table(&rows));

        // Empty rows: returns 1 and writes heading only
        let mut out_empty = Vec::new();
        let code_empty = run(&[], &mut out_empty);
        assert_eq!(code_empty, 1);
        assert_eq!(String::from_utf8(out_empty).unwrap(), table(&[]));
    }

    #[test]
    fn clause_8_absent_constant_is_verbatim() {
        assert_eq!(ABSENT, "--");
    }

    #[test]
    fn boundary_zero_rows_heading_only() {
        let out = table(&[]);
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn boundary_lines_zero_with_files_written() {
        let files = [
            Wrote {
                path: "src/a.rs".to_string(),
                mtime_s: 100,
            },
            Wrote {
                path: "src/b.rs".to_string(),
                mtime_s: 160,
            },
        ];
        let row = TimingRow {
            arm: "zero-lines-arm".to_string(),
            started_s: 100,
            wrote: files.to_vec(),
            lines: 0,
        };

        let out = table(&[row]);
        assert!(out.contains("zero-lines-arm"));
        assert!(out.contains("0"));
    }

    #[test]
    fn boundary_mtime_before_started_renders_derive_output() {
        let files = [Wrote {
            path: "src/clock_skew.rs".to_string(),
            mtime_s: 970,
        }];
        let row = TimingRow {
            arm: "skew-arm".to_string(),
            started_s: 1000,
            wrote: files.to_vec(),
            lines: 10,
        };

        let expected_meas = derive(row.started_s, &row.wrote, row.lines);
        let expected = match expected_meas {
            Measurement::Observed(t) => t,
            Measurement::Missing(_) => unreachable!(),
        };

        let out = table(&[row]);
        assert!(out.contains("skew-arm"));
        assert!(out.contains(&expected.ttfw_s.to_string()));
    }

    #[test]
    fn boundary_duplicate_arm_names_both_rendered() {
        let rows = [
            TimingRow {
                arm: "same-arm".to_string(),
                started_s: 100,
                wrote: Vec::new(),
                lines: 0,
            },
            TimingRow {
                arm: "same-arm".to_string(),
                started_s: 200,
                wrote: Vec::new(),
                lines: 0,
            },
        ];

        let out = table(&rows);
        let matches = out.matches("same-arm").count();
        assert_eq!(matches, 2);
    }
}

#[cfg(test)]
mod gather_tests {
    use super::*;

    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let d = std::env::temp_dir().join(format!("fb-timing-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            let _ = fs::create_dir_all(&d);
            Scratch(d)
        }
        /// A worktree that is a real git repo with one committed file and one edit.
        fn arm(&self, name: &str, edited: &str, created: Option<&str>) {
            let wt = self.0.join(name);
            let _ = fs::create_dir_all(wt.join("crates"));
            let run = |args: &[&str]| {
                let _ = Command::new("git").current_dir(&wt).args(args).output();
            };
            run(&["init", "-q"]);
            run(&["config", "user.email", "t@t"]);
            run(&["config", "user.name", "t"]);
            let _ = fs::write(wt.join(edited), "one\n");
            run(&["add", "-A"]);
            run(&["commit", "-qm", "base"]);
            let _ = fs::write(wt.join(edited), "one\ntwo\n");
            if let Some(c) = created {
                let _ = fs::write(wt.join(c), "a\nb\nc\n");
            }
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The headline: a task's candidate worktrees become rows. `fb timing` was wired to
    /// `run(&[], ..)` and printed a header over nothing for as long as it existed, while
    /// fb-timing.sh did the work in stat and awk.
    #[test]
    fn candidate_worktrees_become_rows() {
        let s = Scratch::new("rows");
        s.arm("t--alpha", "crates/a.rs", None);
        s.arm("t--beta", "crates/b.rs", None);
        let rows = gather("t", &s.0);
        let arms: Vec<&str> = rows.iter().map(|r| r.arm.as_str()).collect();
        assert_eq!(arms, vec!["alpha", "beta"], "sorted, one per worktree");
    }

    /// A critic's scratch checkout matches the same prefix and is NOT a candidate.
    /// fb-timing.sh listed `critic--gemini-38-flash` beside the real arm with dashes in
    /// every column.
    #[test]
    fn a_critics_worktree_is_not_a_candidate() {
        let s = Scratch::new("critic");
        s.arm("t--alpha", "crates/a.rs", None);
        s.arm("t--critic--beta", "crates/b.rs", None);
        let rows = gather("t", &s.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].arm, "alpha");
    }

    /// Lines counted are the diff's insertions PLUS every line of a created file, which the
    /// diff cannot see because the file is untracked. One edit adding a line, one new file
    /// of three: four.
    #[test]
    fn created_files_count_even_though_the_diff_cannot_see_them() {
        let s = Scratch::new("lines");
        s.arm("t--alpha", "crates/a.rs", Some("crates/new.rs"));
        let rows = gather("t", &s.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].lines, 4, "1 added + 3 in the untracked file");
        let paths: Vec<&str> = rows[0].wrote.iter().map(|w| w.path.as_str()).collect();
        assert!(paths.contains(&"crates/a.rs") && paths.contains(&"crates/new.rs"));
    }

    /// A task with no worktrees is an empty table, not an error. Same call, different task
    /// name -- the two answers must differ.
    #[test]
    fn a_task_with_no_worktrees_gathers_nothing() {
        let s = Scratch::new("none");
        s.arm("t--alpha", "crates/a.rs", None);
        assert_eq!(gather("t", &s.0).len(), 1);
        assert!(gather("other", &s.0).is_empty());
    }

    /// A root that does not exist is empty, and does not panic.
    #[test]
    fn a_missing_worktree_root_is_empty() {
        assert!(gather("t", Path::new("/nonexistent/fb-timing")).is_empty());
    }
}

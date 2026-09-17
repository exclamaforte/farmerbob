//! `fb differential` -- run a ported candidate against the script it replaces, and diff.
//!
//! # Why this exists
//!
//! Every other gate in this harness is a gate on FORM. `fb score` asks whether a candidate
//! builds, whether its own tests pass, whether it touched only its deliverable, whether it is
//! clippy-clean. A CONSTANT SATISFIES ALL OF THEM.
//!
//! That is not hypothetical. On `port-defects`, one arm submitted 311 lines that never invoked
//! cargo at all. It emitted the script's exact wire format with the numbers hardcoded --
//! `caught: "0"`, `sensitivity: "n/a"`, `arms: {}` -- and it passed verdict, scope, build,
//! lint and its own three tests. It was caught by exactly one instrument: another model read
//! it. (farmerbob-h70d)
//!
//! A port is the one kind of task in this project with a real oracle -- the script it replaces
//! -- and until now that oracle was consulted by hand, by me, when I remembered to. This makes
//! it a stage.
//!
//! # What it refuses to do
//!
//! It never reports PASS for an arm it could not run. "The candidate agreed with the script"
//! and "I was unable to ask" are different facts and they get different values, because the
//! bug this whole project keeps having is the one where they get the same one.

use farmerbob_core::measurement::{Absent, Measurement};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// A candidate that outlives this is not slow, it is hung. The shell side gets the same
/// bound, so a script that hangs cannot be scored as a candidate divergence.
const RUN_TIMEOUT_SECS: u64 = 300;

/// One invocation to compare: the arguments handed to both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub args: Vec<String>,
}

/// What both sides produced for one case. Compared field by field, because a port that gets
/// the exit code right and the message wrong has still broken the contract: the dispatcher
/// branches on the code and a human reads the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// How one arm compared against the script, over every case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmVerdict {
    /// Every case matched the script on code, stdout and stderr.
    Agrees,
    /// At least one case differed. Carries the first divergence, rendered.
    Diverges { case: usize, detail: String },
}

/// The per-arm result. `Missing` is a first-class answer: an arm whose binary would not build
/// has NOT been shown to agree, and must never be recorded as though it had.
pub type ArmResult = Measurement<ArmVerdict>;

/// Parse the differential declaration out of a task spec.
///
/// The spec declares it the same way it declares its deliverable:
///
/// ```text
/// <!-- fb:differential fb-eligible.sh eligible -->
/// <!-- fb:case or-hy3 -->
/// <!-- fb:case claude-sonnet -->
/// <!-- fb:case no-such-arm -->
/// ```
///
/// The first names the script and the `fb` subcommand that replaces it. Each `fb:case` is one
/// argument vector given to both sides verbatim.
///
/// Returns `NothingToMeasure` rather than an error when the spec declares no differential:
/// most tasks are not ports, and "this task has no oracle" is a fact about the task, not a
/// failure of this instrument.
pub fn parse_spec(spec: &str) -> Measurement<(String, String, Vec<Case>)> {
    let mut decl: Option<(String, String)> = None;
    let mut cases = Vec::new();
    for line in spec.lines() {
        let line = line.trim();
        if let Some(rest) = tag(line, "fb:differential") {
            let mut it = rest.split_whitespace();
            match (it.next(), it.next()) {
                (Some(script), Some(sub)) => decl = Some((script.to_string(), sub.to_string())),
                _ => {
                    return Measurement::instrument_failed(
                        "fb:differential needs both a script and a subcommand",
                    )
                }
            }
        } else if let Some(rest) = tag(line, "fb:case") {
            cases.push(Case {
                args: rest.split_whitespace().map(str::to_string).collect(),
            });
        }
    }
    match decl {
        None => Measurement::nothing_to_measure("the spec declares no fb:differential"),
        Some((script, sub)) if cases.is_empty() => {
            let _ = (&script, &sub);
            Measurement::instrument_failed(
                "fb:differential is declared but no fb:case gives it an input to run on",
            )
        }
        Some((script, sub)) => Measurement::observed((script, sub, cases)),
    }
}

/// Extract the body of an `<!-- name ... -->` comment, if this line is one.
fn tag<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    // `strip_prefix`, never `trim_start_matches`: `fb:case` is a prefix of nothing here, but
    // the last time this file used `trim_start_matches` to drop a marker it ate every leading
    // character in the set and rendered "SPEC" as "PEC".
    rest.strip_prefix(name).map(str::trim_start)
}

/// Compare two outcomes, rendering the FIRST field that differs.
///
/// Rendered rather than returned as a bool because the whole value of this gate is the
/// sentence a human reads when it fires.
pub fn compare(case: usize, args: &[String], want: &Outcome, got: &Outcome) -> Option<String> {
    let argv = args.join(" ");
    if want.code != got.code {
        // Carry the candidate's stderr with the code. Without it this line reads "script
        // exits 0, candidate exits 2" and a reader cannot tell a wrong ANSWER from a wrong
        // CLI SHAPE -- on the first real run of this gate, three arms exited 2 because clap
        // rejected the argument vector before their code ran at all, and the message gave no
        // way to know that. A divergence a human cannot act on is barely better than none.
        let mut m = format!(
            "`{argv}`: script exits {}, candidate exits {}",
            want.code, got.code
        );
        if !got.stderr.trim().is_empty() {
            m.push_str(&format!("\n    candidate stderr: {}", render(&got.stderr)));
        }
        return Some(m);
    }
    for (what, w, g) in [
        ("stdout", &want.stdout, &got.stdout),
        ("stderr", &want.stderr, &got.stderr),
    ] {
        if w != g {
            return Some(format!(
                "`{argv}`: {what} differs at case {case}\n    script:    {}\n    candidate: {}",
                render(w),
                render(g)
            ));
        }
    }
    None
}

/// One-line rendering of a captured stream, so a divergence fits on a terminal.
fn render(s: &str) -> String {
    let flat = s.replace('\n', "\\n");
    if flat.chars().count() > 160 {
        let cut: String = flat.chars().take(157).collect();
        format!("{cut}...")
    } else if flat.is_empty() {
        "<empty>".to_string()
    } else {
        flat
    }
}

/// Run one command under the timeout, capturing all three outputs.
fn run(cmd: &mut Command, timeout: Duration) -> Measurement<Outcome> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = match cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Measurement::instrument_failed(&format!("could not spawn: {e}")),
    };
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Measurement::instrument_failed(&format!("wait failed: {e}")),
        }
    };
    let Some(status) = status else {
        return Measurement::instrument_failed("timed out");
    };
    let mut so = String::new();
    let mut se = String::new();
    if let Some(h) = out.as_mut() {
        let _ = h.read_to_string(&mut so);
    }
    if let Some(h) = err.as_mut() {
        let _ = h.read_to_string(&mut se);
    }
    // A process killed by a signal has no exit code. It is not "exit 0" and it is not any
    // other number we could invent, so it is not observed at all.
    match status.code() {
        Some(code) => Measurement::observed(Outcome {
            code,
            stdout: so,
            stderr: se,
        }),
        None => Measurement::instrument_failed("killed by a signal, no exit code"),
    }
}

/// Build one arm's worktree and return the path to its `fb` binary.
///
/// A build failure is `InstrumentFailed`, not a divergence. The arm may be perfectly correct
/// and the workspace broken for an unrelated reason; this gate does not get to say which.
fn build_arm(wt: &Path) -> Measurement<PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(wt).args(["build", "-p", "fb"]);
    match run(&mut cmd, Duration::from_secs(RUN_TIMEOUT_SECS)) {
        Measurement::Observed(o) if o.code == 0 => {
            let bin = wt.join("target/debug/fb");
            if bin.exists() {
                Measurement::observed(bin)
            } else {
                Measurement::instrument_failed("cargo build succeeded but target/debug/fb is absent")
            }
        }
        Measurement::Observed(o) => Measurement::instrument_failed(&format!(
            "cargo build exited {}: {}",
            o.code,
            render(&o.stderr)
        )),
        Measurement::Missing(a) => Measurement::Missing(a),
    }
}

/// The whole report, per arm, in a stable order.
pub type Report = BTreeMap<String, ArmResult>;

/// Compare every arm against the script over every case.
///
/// `reference` runs the SCRIPT, now. Never an artefact on disk: an artefact records the last
/// time something ran and is not a statement of what it does. That mistake voided a correct
/// arm on `port-objective` and the correction is load-bearing enough to be repeated here.
pub fn measure(
    repo: &Path,
    script: &str,
    subcommand: &str,
    cases: &[Case],
    arms: &[(String, PathBuf)],
) -> (Report, Vec<Measurement<Outcome>>) {
    let timeout = Duration::from_secs(RUN_TIMEOUT_SECS);

    let reference: Vec<Measurement<Outcome>> = cases
        .iter()
        .map(|c| {
            let mut cmd = Command::new("bash");
            cmd.current_dir(repo).arg(repo.join(script)).args(&c.args);
            run(&mut cmd, timeout)
        })
        .collect();

    let mut report = Report::new();
    for (name, wt) in arms {
        let bin = match build_arm(wt) {
            Measurement::Observed(b) => b,
            Measurement::Missing(a) => {
                report.insert(name.clone(), Measurement::Missing(a));
                continue;
            }
        };
        let mut verdict = Measurement::observed(ArmVerdict::Agrees);
        for (i, c) in cases.iter().enumerate() {
            let Measurement::Observed(want) = &reference[i] else {
                // The ORACLE failed, not the candidate. Saying "diverges" here would blame the
                // arm for the instrument's fault, which is precisely what `Indeterminate`
                // exists to prevent elsewhere in this crate.
                verdict = Measurement::instrument_failed(&format!(
                    "the script itself did not produce a comparable result on case {i}"
                ));
                break;
            };
            let mut cmd = Command::new(&bin);
            cmd.current_dir(repo).arg(subcommand).args(&c.args);
            match run(&mut cmd, timeout) {
                Measurement::Observed(got) => {
                    if let Some(detail) = compare(i, &c.args, want, &got) {
                        verdict = Measurement::observed(ArmVerdict::Diverges { case: i, detail });
                        break;
                    }
                }
                Measurement::Missing(a) => {
                    verdict = Measurement::Missing(a);
                    break;
                }
            }
        }
        report.insert(name.clone(), verdict);
    }
    (report, reference)
}

/// Render the report as the table the pipeline prints.
pub fn render_report(report: &Report) -> String {
    let mut s = String::new();
    s.push_str(&format!("\n{:<24}{}\n", "ARM", "DIFFERENTIAL"));
    for (name, r) in report {
        let cell = match r {
            Measurement::Observed(ArmVerdict::Agrees) => "agrees with the script".to_string(),
            Measurement::Observed(ArmVerdict::Diverges { case, .. }) => {
                format!("DIVERGES at case {case}")
            }
            Measurement::Missing(a) => format!("NOT MEASURED -- {}", why(a)),
        };
        s.push_str(&format!("{name:<24}{cell}\n"));
    }
    for (name, r) in report {
        if let Measurement::Observed(ArmVerdict::Diverges { detail, .. }) = r {
            s.push_str(&format!("\n{name}:\n  {detail}\n"));
        }
    }
    s
}

/// Plain-language reason, so the table never prints a bare enum at a human.
fn why(a: &Absent) -> String {
    match a {
        Absent::NotAttempted => "not attempted".to_string(),
        Absent::InstrumentFailed { reason } => reason.clone(),
        Absent::NothingToMeasure { reason } => reason.clone(),
        Absent::Untrusted { reason } => reason.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(code: i32, so: &str, se: &str) -> Outcome {
        Outcome {
            code,
            stdout: so.to_string(),
            stderr: se.to_string(),
        }
    }

    #[test]
    fn a_spec_without_a_differential_is_nothing_to_measure_not_a_failure() {
        let m = parse_spec("# Task: do a thing\n<!-- fb:creates src/x.rs -->\n");
        assert!(matches!(
            m,
            Measurement::Missing(Absent::NothingToMeasure { .. })
        ));
    }

    #[test]
    fn a_differential_with_no_cases_is_an_instrument_failure() {
        // Declared but unrunnable. If this returned `Agrees` it would be the exact bug the
        // whole gate exists to catch, committed by the gate itself.
        let m = parse_spec("<!-- fb:differential fb-eligible.sh eligible -->");
        assert!(matches!(m, Measurement::Missing(Absent::InstrumentFailed { .. })));
    }

    #[test]
    fn parses_the_script_subcommand_and_every_case() {
        let m = parse_spec(
            "<!-- fb:differential fb-eligible.sh eligible -->\n\
             <!-- fb:case or-hy3 -->\n\
             <!-- fb:case no-such-arm -->\n",
        );
        let (script, sub, cases) = m.value().expect("declared").clone();
        assert_eq!(script, "fb-eligible.sh");
        assert_eq!(sub, "eligible");
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[1].args, vec!["no-such-arm".to_string()]);
    }

    #[test]
    fn identical_outcomes_do_not_diverge() {
        let a = out(1, "", "or-x: disabled\n");
        assert_eq!(compare(0, &["or-x".into()], &a, &a), None);
    }

    #[test]
    fn a_matching_exit_code_with_a_different_message_still_diverges() {
        // The case that matters most for a predicate port. Both sides say "no", so anything
        // comparing exit codes alone reports agreement -- while the reason a human reads has
        // been silently replaced.
        let want = out(1, "", "or-x: disabled -- paid duplicate\n");
        let got = out(1, "", "or-x: not dispatchable\n");
        let d = compare(0, &["or-x".into()], &want, &got).expect("must diverge");
        assert!(d.contains("stderr differs"), "{d}");
    }

    #[test]
    fn the_exit_code_is_reported_before_the_streams() {
        let want = out(0, "", "");
        let got = out(1, "", "boom");
        let d = compare(3, &["a".into()], &want, &got).expect("must diverge");
        assert!(d.contains("script exits 0, candidate exits 1"), "{d}");
    }

    #[test]
    fn a_code_divergence_carries_the_candidate_stderr() {
        // Regression for the first real run: three arms exited 2 because clap rejected the
        // argument vector, and the table said only "candidate exits 2".
        let want = out(0, "rows\n", "");
        let got = out(2, "", "error: the following required arguments were not provided:\n  <WTROOT>");
        let d = compare(0, &["port-defects".into()], &want, &got).expect("must diverge");
        assert!(d.contains("candidate stderr"), "{d}");
        assert!(d.contains("required arguments"), "{d}");
    }

    #[test]
    fn a_code_divergence_with_a_silent_candidate_says_only_the_codes() {
        let want = out(0, "", "");
        let got = out(2, "", "   \n");
        let d = compare(0, &["a".into()], &want, &got).expect("must diverge");
        assert!(!d.contains("candidate stderr"), "{d}");
    }

    #[test]
    fn an_empty_stream_renders_as_a_word_not_as_nothing() {
        // `<empty>` rather than "": a blank on either side of a diff is unreadable, and this
        // gate's entire output is a sentence a human acts on.
        let want = out(0, "something\n", "");
        let got = out(0, "", "");
        let d = compare(0, &["a".into()], &want, &got).expect("must diverge");
        assert!(d.contains("<empty>"), "{d}");
    }

    #[test]
    fn an_unbuildable_arm_is_missing_and_never_agrees() {
        let report: Report = [(
            "stub".to_string(),
            Measurement::instrument_failed("cargo build exited 101"),
        )]
        .into_iter()
        .collect();
        let t = render_report(&report);
        assert!(t.contains("NOT MEASURED"), "{t}");
        assert!(!t.contains("agrees"), "{t}");
    }

    #[test]
    fn a_divergence_prints_its_detail_under_the_table() {
        let report: Report = [(
            "arm".to_string(),
            Measurement::observed(ArmVerdict::Diverges {
                case: 2,
                detail: "`x`: script exits 1, candidate exits 0".to_string(),
            }),
        )]
        .into_iter()
        .collect();
        let t = render_report(&report);
        assert!(t.contains("DIVERGES at case 2"), "{t}");
        assert!(t.contains("script exits 1, candidate exits 0"), "{t}");
    }
}

// ---------------------------------------------------------------------------------------
// Command entry point
// ---------------------------------------------------------------------------------------

/// `fb differential <task>`.
///
/// Exit codes, which the pipeline branches on:
///   0  every arm that could be measured agrees with the script
///   1  at least one arm diverges -- a real finding about a candidate
///   2  this task HAS an oracle and it could not be consulted -- a gap, and a problem
///   3  this task has no oracle: it is not a port, and there is nothing here to do
///
/// All four are deliberately distinct, and 2 against 3 is the one that matters. An earlier
/// draft of this function returned 2 for both "the spec declares no differential" and "the
/// differential is declared but I could not run it" -- which would have made a task that was
/// never checked look exactly like a task with nothing to check. That is the bug this file
/// exists to end, committed inside the file itself, and it survived until the stage was
/// wired into the pipeline and the two cases needed different handling.
pub fn run_cmd(task: &str) -> i32 {
    let repo = crate::paths::repo();
    let spec_path = repo.join(".fb/prompts").join(format!("{task}.md"));
    let spec = match std::fs::read_to_string(&spec_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("differential: cannot read {}: {e}", spec_path.display());
            return 2;
        }
    };

    let (script, sub, cases) = match parse_spec(&spec) {
        Measurement::Observed(v) => v,
        Measurement::Missing(a) => {
            // NOT a failure. Most tasks are not ports, and a task with no oracle is a fact
            // about the task. Only a DECLARED differential that could not run is a gap.
            println!("  differential: {}", why(&a));
            return match a {
                Absent::NothingToMeasure { .. } => 3,
                _ => 2,
            };
        }
    };

    if !repo.join(&script).exists() {
        eprintln!("differential: {script} does not exist; nothing to compare against");
        return 2;
    }

    let wt_root = crate::paths::worktrees();
    let mut arms: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&wt_root) {
        let prefix = format!("{task}--");
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(arm) = name.strip_prefix(&prefix).filter(|_| e.path().is_dir()) {
                arms.push((arm.to_string(), e.path()));
            }
        }
    }
    arms.sort();

    if arms.is_empty() {
        eprintln!("differential: no worktrees named {task}--*");
        return 2;
    }

    println!(
        "  differential: {} arms against {script}, {} cases each",
        arms.len(),
        cases.len()
    );

    let (report, reference) = measure(&repo, &script, &sub, &cases, &arms);

    // Report the ORACLE's own health first. A reference that could not run makes every cell
    // below it uninterpretable, and a reader must not have to infer that from the cells.
    let broken = reference.iter().filter(|m| !m.is_observed()).count();
    if broken > 0 {
        println!(
            "  differential: WARNING -- the script failed to produce a result on {broken} of {} cases",
            cases.len()
        );
    }

    print!("{}", render_report(&report));

    let out = crate::paths::logs().join(format!("{task}.differential.json"));
    if let Some(p) = out.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let body = serialize(&report);
    match std::fs::write(&out, body) {
        Ok(()) => println!("-> {}", out.display()),
        Err(e) => eprintln!("differential: could not write {}: {e}", out.display()),
    }

    let diverged = report
        .values()
        .any(|r| matches!(r, Measurement::Observed(ArmVerdict::Diverges { .. })));
    let measured = report.values().any(Measurement::is_observed);

    if diverged {
        1
    } else if measured {
        0
    } else {
        eprintln!("differential: no arm could be measured; this is not a pass");
        2
    }
}

/// Hand-rolled so the three states survive the round trip as three distinct shapes. A
/// serializer that wrote `null` for Missing would hand the next reader the same ambiguity
/// this gate exists to remove.
fn serialize(report: &Report) -> String {
    let mut rows = Vec::new();
    for (name, r) in report {
        let cell = match r {
            Measurement::Observed(ArmVerdict::Agrees) => "{\"state\":\"agrees\"}".to_string(),
            Measurement::Observed(ArmVerdict::Diverges { case, detail }) => format!(
                "{{\"state\":\"diverges\",\"case\":{case},\"detail\":{}}}",
                json_str(detail)
            ),
            Measurement::Missing(a) => format!(
                "{{\"state\":\"not_measured\",\"reason\":{}}}",
                json_str(&why(a))
            ),
        };
        rows.push(format!("  {}: {cell}", json_str(name)));
    }
    format!("{{\n{}\n}}\n", rows.join(",\n"))
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod serialize_tests {
    use super::*;

    #[test]
    fn the_three_states_serialize_as_three_distinct_shapes() {
        let report: Report = [
            ("a".to_string(), Measurement::observed(ArmVerdict::Agrees)),
            (
                "b".to_string(),
                Measurement::observed(ArmVerdict::Diverges {
                    case: 1,
                    detail: "x".to_string(),
                }),
            ),
            ("c".to_string(), Measurement::not_attempted()),
        ]
        .into_iter()
        .collect();
        let s = serialize(&report);
        assert!(s.contains("\"state\":\"agrees\""), "{s}");
        assert!(s.contains("\"state\":\"diverges\""), "{s}");
        assert!(s.contains("\"state\":\"not_measured\""), "{s}");
        // The one thing that must never appear: a null standing in for an unmeasured arm.
        assert!(!s.contains("null"), "{s}");
    }

    #[test]
    fn a_detail_containing_quotes_and_newlines_survives() {
        let report: Report = [(
            "a".to_string(),
            Measurement::observed(ArmVerdict::Diverges {
                case: 0,
                detail: "said \"no\"\nthen exited".to_string(),
            }),
        )]
        .into_iter()
        .collect();
        let s = serialize(&report);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert_eq!(v["a"]["detail"], "said \"no\"\nthen exited");
    }
}

#[cfg(test)]
mod exit_code_tests {
    use super::*;

    /// The distinction the pipeline branches on, pinned as a test because it is the one
    /// thing here that is easy to collapse and fatal to collapse.
    #[test]
    fn no_oracle_and_an_unusable_oracle_are_different_codes() {
        let none = parse_spec("<!-- fb:creates x.rs -->");
        let broken = parse_spec("<!-- fb:differential fb-x.sh x -->");

        let code = |m: &Measurement<(String, String, Vec<Case>)>| match m {
            Measurement::Observed(_) => 0,
            Measurement::Missing(Absent::NothingToMeasure { .. }) => 3,
            Measurement::Missing(_) => 2,
        };
        assert_eq!(code(&none), 3, "a task that is not a port has nothing to check");
        assert_eq!(code(&broken), 2, "a declared oracle that cannot run is a gap");
        assert_ne!(code(&none), code(&broken));
    }
}

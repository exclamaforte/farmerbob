//! `fb fate <task> --veto <passed|failed|did-not-run>` — the bridge between the
//! escalation shell and `farmerbob_core::finding_fate`.
//!
//! `fb-escalate.sh` escalates a finding, runs it against the merged reference, and
//! on failure prints `VETOED against the merged reference -- retracting`. The veto
//! is right to refuse a failing test into a green suite. But the finding was
//! ACCEPTED by the adjudicator, is real, and then existed nowhere but a bead
//! somebody wrote by hand. The better the critic, the more likely its finding is
//! about the arm that won, and the more certainly it was discarded.
//!
//! The decision itself has lived in core since finding_fate merged; what was
//! missing was a way for the shell to ask. This command is that: the shell asks,
//! core decides via [`finding_fate::decide`], the shell acts. Writing the decision
//! a second time here is how this harness ended up with four copies of its verdict
//! logic, so there is no second table — the only things this module owns are
//! reading the two input files and rendering what core already decided.
//!
//! Inputs, both overridable through `FB_REPO` / `FB_LOGS` like every other port:
//!
//! * `.fb/adjudicated/<task>` — first line `MERGED <arm>` names the arm that won.
//!   Records beginning anything else (`VOID`, `SUPERSEDED`) name no arm, and a
//!   fate decided without one would be a guess.
//! * `<logs>/<task>.claims.json` — `{"claims": [...]}`, each claim carrying at
//!   least `critic` and `subject`. A claim may carry more; those fields are a
//!   known subset and extra ones are not an error.

use std::fs;
use std::io::Write;
use std::path::Path;

use farmerbob_core::finding_fate::{self, Fate, VetoRun};

/// Exit codes are part of the contract: the shell branches on these.
mod exit {
    /// Every claim in the file got its line.
    pub const OK: i32 = 0;
    /// An I/O failure the input checks do not classify as nothing-to-decide.
    pub const ERROR: i32 = 1;
    /// Nothing was decided: unrecognised `--veto`, no adjudication record, no
    /// merged arm, a missing or unparseable claims file, or no claims to decide.
    /// "Decided nothing" must not read as "everything was fine".
    pub const NOTHING_DECIDED: i32 = 2;
}

/// Decide the fate of a task's findings and print one line per finding.
///
/// `veto` is what happened when the escalated test ran against the merged
/// reference: `passed`, `failed`, or `did-not-run`.
///
/// Returns a process exit code: 0 when every finding was decided, 2 when the
/// task has no adjudication record or no claims to decide, 1 on an I/O or
/// parse failure.
pub fn run_cmd(task: &str, veto: &str) -> i32 {
    let Some(parsed) = parse_veto(veto) else {
        eprintln!("fb fate: --veto must be one of passed, failed, did-not-run, not `{veto}`");
        return exit::NOTHING_DECIDED;
    };
    if task.is_empty() {
        eprintln!("fb fate: task is required");
        return exit::NOTHING_DECIDED;
    }
    let outcome = decide_all(&crate::paths::repo(), &crate::paths::logs(), task, parsed);
    if let Some(complaint) = &outcome.complaint {
        eprintln!("{complaint}");
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in &outcome.lines {
        if writeln!(out, "{line}").is_err() {
            eprintln!("fb fate: cannot write findings to stdout");
            return exit::ERROR;
        }
    }
    if out.flush().is_err() {
        eprintln!("fb fate: cannot flush stdout");
        return exit::ERROR;
    }
    outcome.code
}

/// Parses the `--veto` value. The set is CLOSED at exactly three: anything else
/// is refused, because guessing which of three states the caller meant is how a
/// harness reports a fault it never observed.
fn parse_veto(veto: &str) -> Option<VetoRun> {
    match veto {
        "passed" => Some(VetoRun::Passed),
        "failed" => Some(VetoRun::Failed),
        "did-not-run" => Some(VetoRun::DidNotRun),
        _ => None,
    }
}

/// One claim as the decision needs it: who raised the finding, and which arm the
/// finding is about. Claims carry more fields; those are ignored, not an error.
struct Claim {
    critic: String,
    subject: String,
}

/// What one invocation decided, before it is printed.
struct Outcome {
    /// One line per claim in the file, in file order: a fate line, or, for a
    /// claim missing `critic` or `subject`, the note that it was skipped and why.
    lines: Vec<String>,
    /// Why nothing could be decided, for stderr. Empty when the command decided.
    complaint: Option<String>,
    /// The process exit code.
    code: i32,
}

impl Outcome {
    /// Nothing was decided, with the reason.
    fn undecided(code: i32, complaint: String) -> Self {
        Outcome {
            lines: Vec::new(),
            complaint: Some(complaint),
            code,
        }
    }
}

/// Decides every claim of one task against inputs rooted at `repo` and `logs`.
///
/// Takes the roots as arguments rather than reading the environment so a test
/// can point it at a fake tree: the environment is process-global and cargo runs
/// tests in parallel, so a test that redirects `FB_REPO` redirects every other
/// test's reads too.
fn decide_all(repo: &Path, logs: &Path, task: &str, veto: VetoRun) -> Outcome {
    let record = repo.join(".fb/adjudicated").join(task);
    let text = match fs::read_to_string(&record) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Outcome::undecided(
                exit::NOTHING_DECIDED,
                format!("fb fate: no adjudication record: {}", record.display()),
            );
        }
        Err(e) => {
            return Outcome::undecided(
                exit::ERROR,
                format!("fb fate: cannot read {}: {e}", record.display()),
            );
        }
    };
    let first = text.lines().next().unwrap_or_default();
    let Some(merged) = merged_arm(first) else {
        return Outcome::undecided(
            exit::NOTHING_DECIDED,
            format!(
                "fb fate: {} does not name a merged arm (first line: {first:?})",
                record.display()
            ),
        );
    };

    let claims_path = logs.join(format!("{task}.claims.json"));
    let text = match fs::read_to_string(&claims_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Outcome::undecided(
                exit::NOTHING_DECIDED,
                format!("fb fate: no claims file: {}", claims_path.display()),
            );
        }
        Err(e) => {
            return Outcome::undecided(
                exit::ERROR,
                format!("fb fate: cannot read {}: {e}", claims_path.display()),
            );
        }
    };
    let entries = match claims_array(&text) {
        Ok(entries) => entries,
        Err(why) => {
            return Outcome::undecided(
                exit::NOTHING_DECIDED,
                format!(
                    "fb fate: {} is not a claims file: {why}",
                    claims_path.display()
                ),
            );
        }
    };
    if entries.is_empty() {
        return Outcome::undecided(
            exit::NOTHING_DECIDED,
            format!("fb fate: {}: no claims to decide", claims_path.display()),
        );
    }

    let mut lines = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        match claim_fields(entry) {
            Ok(claim) => {
                let subject_merged = claim.subject == merged;
                let fate = finding_fate::decide(subject_merged, veto);
                lines.push(fate_line(&claim, &fate));
            }
            Err(why) => lines.push(format!("skipped claim {}: {why}", index + 1)),
        }
    }
    Outcome {
        lines,
        complaint: None,
        code: exit::OK,
    }
}

/// The arm a record's first line names when it begins `MERGED <arm>`.
///
/// `MERGED` with nothing after it names no arm, and neither does `VOID` or
/// `SUPERSEDED`: a task without a named winner has no merged subject, and every
/// fate against one would be a guess.
fn merged_arm(first_line: &str) -> Option<String> {
    let arm = first_line.strip_prefix("MERGED ")?.trim();
    if arm.is_empty() {
        return None;
    }
    Some(arm.to_string())
}

/// Extracts the claims array from the `{"claims": [...]}` envelope.
fn claims_array(text: &str) -> Result<Vec<serde_json::Value>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let claims = value.get("claims").ok_or("no `claims` key")?;
    claims
        .as_array()
        .cloned()
        .ok_or_else(|| "`claims` is not an array".to_string())
}

/// The two fields the decision reads, rejecting a claim that cannot name them.
///
/// Absent, null or non-string `critic` / `subject` counts as missing. A fate
/// decided for a claim whose subject nobody named would be a guess wearing a
/// verdict, so the claim is skipped and the skip reported instead.
fn claim_fields(entry: &serde_json::Value) -> Result<Claim, String> {
    let obj = entry.as_object().ok_or("claim is not a JSON object")?;
    Ok(Claim {
        critic: str_field(obj, "critic")?,
        subject: str_field(obj, "subject")?,
    })
}

/// One string field of a claim object.
fn str_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, String> {
    match obj.get(key) {
        None | Some(serde_json::Value::Null) => Err(format!("missing {key}")),
        Some(value) => value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{key} is not a string")),
    }
}

/// Renders one finding's line: the fate token the clauses name, the reason
/// `finding_fate` supplies when it supplies one, and the (critic, subject)
/// pair that says which finding the line is about.
fn fate_line(claim: &Claim, fate: &Fate) -> String {
    let head = format!("{} on {}", claim.critic, claim.subject);
    match fate {
        Fate::Escalate => format!("{head}: ESCALATE"),
        Fate::FileAsKnownDefect { reason } => {
            format!("{head}: FILE-AS-KNOWN-DEFECT -- {reason}")
        }
        Fate::Unverifiable { reason } => format!("{head}: UNVERIFIABLE -- {reason}"),
        Fate::NoSubject => format!("{head}: NO-SUBJECT"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tempdir(label: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("fb-fate-{label}-{}-{n}", std::process::id()))
    }

    /// Builds both input files for a task adjudicated as `merged_arm`.
    fn write_inputs(repo: &Path, logs: &Path, task: &str, merged_arm: &str, claims: &str) {
        fs::create_dir_all(repo.join(".fb/adjudicated")).unwrap();
        fs::write(
            repo.join(".fb/adjudicated").join(task),
            format!("MERGED {merged_arm}\n"),
        )
        .unwrap();
        fs::create_dir_all(logs).unwrap();
        fs::write(logs.join(format!("{task}.claims.json")), claims).unwrap();
    }

    fn one_claim(critic: &str, subject: &str) -> String {
        format!(r#"{{"critic": "{critic}", "subject": "{subject}"}}"#)
    }

    fn envelope(claims: &[String]) -> String {
        format!(r#"{{"claims": [{}]}}"#, claims.join(", "))
    }

    /// The fate token the clauses name, derived from core, not from a string
    /// table of this module's own.
    fn token(fate: &Fate) -> String {
        match fate {
            Fate::Escalate => "ESCALATE".to_string(),
            Fate::FileAsKnownDefect { .. } => "FILE-AS-KNOWN-DEFECT".to_string(),
            Fate::Unverifiable { .. } => "UNVERIFIABLE".to_string(),
            Fate::NoSubject => "NO-SUBJECT".to_string(),
        }
    }

    fn complaint_of(outcome: &Outcome) -> String {
        outcome
            .complaint
            .clone()
            .unwrap_or_else(|| String::from("<no complaint>"))
    }

    // -- the veto value is a closed set (clause 6) --------------------------------------

    #[test]
    fn veto_parses_exactly_the_closed_set() {
        assert_eq!(parse_veto("passed"), Some(VetoRun::Passed));
        assert_eq!(parse_veto("failed"), Some(VetoRun::Failed));
        assert_eq!(parse_veto("did-not-run"), Some(VetoRun::DidNotRun));
        // Anything else is clause 6, including near-misses that a case-fold or
        // an underscore-for-dash substitution would happily accept.
        for wrong in [
            "PASSED",
            "Failed",
            "DID-NOT-RUN",
            "didnotrun",
            "did_not_run",
            "",
        ] {
            assert_eq!(parse_veto(wrong), None, "`{wrong}` must not parse");
        }
    }

    #[test]
    fn an_unrecognised_veto_returns_2_before_reading_anything() {
        // A wrong `--veto` is refused before the command looks at the task, so
        // this holds no matter what the filesystem does or does not contain.
        for wrong in ["sideways", "PASSED", ""] {
            assert_eq!(run_cmd("fate-cmd-no-such-task", wrong), 2, "veto `{wrong}`");
        }
    }

    // -- one fate per (subject_merged, veto) cell, decided by core (clauses 1-5) --------

    #[test]
    fn merged_subject_with_passed_veto_prints_escalate() {
        let (repo, logs) = (tempdir("escalate"), tempdir("escalate-logs"));
        write_inputs(
            &repo,
            &logs,
            "t",
            "winner",
            &envelope(&[one_claim("critic-a", "winner")]),
        );
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Passed);
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.lines.len(), 1);
        assert!(outcome.lines[0].contains("ESCALATE"), "{:?}", outcome.lines);
        assert!(!outcome.lines[0].contains("NO-SUBJECT"));
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn merged_subject_with_failed_veto_files_as_known_defect_with_the_reason_core_supplies() {
        let (repo, logs) = (tempdir("defect"), tempdir("defect-logs"));
        write_inputs(
            &repo,
            &logs,
            "t",
            "winner",
            &envelope(&[one_claim("critic-a", "winner")]),
        );
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Failed);
        // Deciding that the finding must be filed as a known defect is the
        // command working: the exit code is 0, not a failure.
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.lines.len(), 1);
        let Fate::FileAsKnownDefect { reason } = finding_fate::decide(true, VetoRun::Failed) else {
            panic!("core changed: merged+failed is no longer FileAsKnownDefect");
        };
        assert!(
            outcome.lines[0].contains("FILE-AS-KNOWN-DEFECT"),
            "{:?}",
            outcome.lines
        );
        assert!(
            outcome.lines[0].contains(&reason),
            "line must carry the reason finding_fate supplies: {:?}",
            outcome.lines
        );
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn merged_subject_with_did_not_run_prints_unverifiable_and_never_the_others() {
        let (repo, logs) = (tempdir("unverifiable"), tempdir("unverifiable-logs"));
        write_inputs(
            &repo,
            &logs,
            "t",
            "winner",
            &envelope(&[one_claim("critic-a", "winner")]),
        );
        let outcome = decide_all(&repo, &logs, "t", VetoRun::DidNotRun);
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.lines.len(), 1);
        let line = &outcome.lines[0];
        assert!(line.contains("UNVERIFIABLE"), "{line}");
        assert!(!line.contains("ESCALATE"));
        assert!(!line.contains("FILE-AS-KNOWN-DEFECT"));
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn an_unmerged_subject_prints_no_subject_for_every_veto_value() {
        for veto in [VetoRun::Passed, VetoRun::Failed, VetoRun::DidNotRun] {
            let (repo, logs) = (tempdir("nosubject"), tempdir("nosubject-logs"));
            write_inputs(
                &repo,
                &logs,
                "t",
                "winner",
                &envelope(&[one_claim("critic-a", "loser")]),
            );
            let outcome = decide_all(&repo, &logs, "t", veto);
            assert_eq!(outcome.code, 0, "{veto:?}");
            assert_eq!(outcome.lines.len(), 1, "{veto:?}");
            assert!(
                outcome.lines[0].contains("NO-SUBJECT"),
                "{veto:?}: {:?}",
                outcome.lines
            );
            fs::remove_dir_all(&repo).ok();
            fs::remove_dir_all(&logs).ok();
        }
    }

    /// Clause 5: the command's fate is `finding_fate::decide`'s. Every cell of
    /// the 2x3 input space must agree with core called directly on the same
    /// inputs, so a local reimplementation of the table cannot hide.
    #[test]
    fn all_six_input_combinations_agree_with_decide_called_directly() {
        for (merged, subject) in [(true, "winner"), (false, "loser")] {
            for veto in [VetoRun::Passed, VetoRun::Failed, VetoRun::DidNotRun] {
                let (repo, logs) = (tempdir("matrix"), tempdir("matrix-logs"));
                write_inputs(
                    &repo,
                    &logs,
                    "t",
                    "winner",
                    &envelope(&[one_claim("c", subject)]),
                );
                let outcome = decide_all(&repo, &logs, "t", veto);
                let fate = finding_fate::decide(merged, veto);
                assert_eq!(outcome.code, 0);
                assert_eq!(outcome.lines.len(), 1);
                let want = token(&fate);
                assert!(
                    outcome.lines[0].contains(&want),
                    "subject_merged={merged} veto={veto:?}: want {want}, got {:?}",
                    outcome.lines
                );
                // A fate that carries a reason must carry the reason core wrote.
                match &fate {
                    Fate::FileAsKnownDefect { reason } | Fate::Unverifiable { reason } => {
                        assert!(
                            outcome.lines[0].contains(reason),
                            "{merged}/{veto:?}: reason missing: {:?}",
                            outcome.lines
                        );
                    }
                    _ => {}
                }
                fs::remove_dir_all(&repo).ok();
                fs::remove_dir_all(&logs).ok();
            }
        }
    }

    // -- missing and malformed inputs (clauses 7 and 8, and the record boundary) --------

    #[test]
    fn a_missing_adjudication_record_returns_2_and_names_the_file() {
        let (repo, logs) = (tempdir("no-record"), tempdir("no-record-logs"));
        fs::create_dir_all(repo.join(".fb/adjudicated")).unwrap();
        fs::create_dir_all(&logs).unwrap();
        let outcome = decide_all(&repo, &logs, "unadjudicated", VetoRun::Passed);
        assert_eq!(outcome.code, 2);
        assert!(
            outcome.lines.is_empty(),
            "decided nothing: {:?}",
            outcome.lines
        );
        let want = repo.join(".fb/adjudicated/unadjudicated");
        assert!(complaint_of(&outcome).contains(&want.display().to_string()));
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn a_missing_claims_file_returns_2_and_names_the_file() {
        let (repo, logs) = (tempdir("no-claims"), tempdir("no-claims-logs"));
        write_inputs(&repo, &logs, "t", "winner", "PLACEHOLDER");
        fs::remove_file(logs.join("t.claims.json")).unwrap();
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Passed);
        assert_eq!(outcome.code, 2);
        assert!(outcome.lines.is_empty());
        let want = logs.join("t.claims.json");
        assert!(complaint_of(&outcome).contains(&want.display().to_string()));
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn an_unparseable_claims_file_returns_2_and_names_the_file() {
        let (repo, logs) = (tempdir("bad-claims"), tempdir("bad-claims-logs"));
        write_inputs(&repo, &logs, "t", "winner", "{\"claims\": [ not json }");
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Passed);
        assert_eq!(outcome.code, 2);
        assert!(outcome.lines.is_empty());
        let want = logs.join("t.claims.json");
        assert!(complaint_of(&outcome).contains(&want.display().to_string()));
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn a_claims_file_without_a_claims_array_returns_2() {
        let (repo, logs) = (tempdir("no-array"), tempdir("no-array-logs"));
        write_inputs(&repo, &logs, "t", "winner", "{}");
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Passed);
        assert_eq!(outcome.code, 2);
        assert!(outcome.lines.is_empty());
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn an_empty_claims_array_returns_2_with_nothing_to_decide() {
        // Zero findings decided is not success.
        let (repo, logs) = (tempdir("empty"), tempdir("empty-logs"));
        write_inputs(&repo, &logs, "t", "winner", r#"{"claims": []}"#);
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Failed);
        assert_eq!(outcome.code, 2);
        assert!(outcome.lines.is_empty());
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn a_record_not_beginning_merged_has_no_arm_and_returns_2() {
        for first_line in [
            "VOID (evidence destroyed)",
            "SUPERSEDED by farmerbob-abc",
            "MERGED",
            "MERGED   ",
            "",
        ] {
            let (repo, logs) = (tempdir("void"), tempdir("void-logs"));
            fs::create_dir_all(repo.join(".fb/adjudicated")).unwrap();
            fs::create_dir_all(&logs).unwrap();
            fs::write(
                repo.join(".fb/adjudicated/t"),
                format!("{first_line}\nlonger explanation on later lines\n"),
            )
            .unwrap();
            let outcome = decide_all(&repo, &logs, "t", VetoRun::Passed);
            assert_eq!(outcome.code, 2, "first line {first_line:?}");
            assert!(outcome.lines.is_empty(), "first line {first_line:?}");
            assert!(
                complaint_of(&outcome)
                    .contains(&repo.join(".fb/adjudicated/t").display().to_string()),
                "first line {first_line:?}: {}",
                complaint_of(&outcome)
            );
            fs::remove_dir_all(&repo).ok();
            fs::remove_dir_all(&logs).ok();
        }
    }

    // -- skipped claims, and the shapes that must not change them ------------------------

    #[test]
    fn a_claim_missing_subject_is_skipped_and_does_not_change_the_others() {
        let (repo, logs) = (tempdir("skip-subject"), tempdir("skip-subject-logs"));
        write_inputs(
            &repo,
            &logs,
            "t",
            "winner",
            &envelope(&[
                one_claim("critic-a", "winner"),
                r#"{"critic": "critic-b"}"#.to_string(),
            ]),
        );
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Passed);
        // The decidable claim was decided: the skip must not drag the exit code.
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.lines.len(), 2);
        assert!(outcome.lines[0].contains("ESCALATE"), "{:?}", outcome.lines);
        assert!(outcome.lines[0].contains("critic-a"));
        assert!(outcome.lines[1].contains("skipped"), "{:?}", outcome.lines);
        assert!(outcome.lines[1].contains("subject"), "{:?}", outcome.lines);
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn a_claim_missing_critic_is_skipped_with_the_reason() {
        let (repo, logs) = (tempdir("skip-critic"), tempdir("skip-critic-logs"));
        write_inputs(
            &repo,
            &logs,
            "t",
            "winner",
            &envelope(&[r#"{"subject": "winner"}"#.to_string()]),
        );
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Failed);
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.lines.len(), 1);
        assert!(outcome.lines[0].contains("skipped"), "{:?}", outcome.lines);
        assert!(outcome.lines[0].contains("critic"), "{:?}", outcome.lines);
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn n_claims_print_n_lines_in_the_order_the_file_lists_them() {
        let (repo, logs) = (tempdir("order"), tempdir("order-logs"));
        write_inputs(
            &repo,
            &logs,
            "t",
            "winner",
            &envelope(&[
                one_claim("critic-a", "winner"),
                one_claim("critic-b", "loser"),
                one_claim("critic-c", "winner"),
            ]),
        );
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Failed);
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.lines.len(), 3);
        assert!(outcome.lines[0].contains("FILE-AS-KNOWN-DEFECT"));
        assert!(outcome.lines[0].contains("critic-a"));
        assert!(outcome.lines[1].contains("NO-SUBJECT"));
        assert!(outcome.lines[1].contains("critic-b"));
        assert!(outcome.lines[2].contains("FILE-AS-KNOWN-DEFECT"));
        assert!(outcome.lines[2].contains("critic-c"));
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn the_same_critic_twice_prints_two_lines() {
        // This reports findings, not critics.
        let (repo, logs) = (tempdir("twice"), tempdir("twice-logs"));
        write_inputs(
            &repo,
            &logs,
            "t",
            "winner",
            &envelope(&[
                one_claim("critic-a", "winner"),
                one_claim("critic-a", "winner"),
            ]),
        );
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Passed);
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.lines.len(), 2);
        for line in &outcome.lines {
            assert!(line.contains("ESCALATE"), "{line}");
            assert!(line.contains("critic-a"), "{line}");
        }
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn extra_fields_on_a_claim_are_not_an_error() {
        // The fields read are a known subset: a claim written by a richer
        // pipeline carries `kind`, `where`, `expect` and more, and every one of
        // them must be ignored rather than refused.
        let (repo, logs) = (tempdir("extra"), tempdir("extra-logs"));
        write_inputs(
            &repo,
            &logs,
            "t",
            "winner",
            &envelope(&[r#"{"critic": "critic-a", "subject": "winner", "kind": "TESTABLE", "claim": "budget totals drift", "where": "totals row", "trigger": "add a row", "expect": "totals include excluded arms", "actual": "they exclude them"}"#.to_string()]),
        );
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Passed);
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.lines.len(), 1);
        assert!(outcome.lines[0].contains("ESCALATE"), "{:?}", outcome.lines);
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }

    /// A claim that is not an object at all cannot name a critic or a subject,
    /// so it is skipped like any other unnameable claim.
    #[test]
    fn a_claim_that_is_not_an_object_is_skipped() {
        let (repo, logs) = (tempdir("not-object"), tempdir("not-object-logs"));
        write_inputs(
            &repo,
            &logs,
            "t",
            "winner",
            &envelope(&[
                "\"just a string\"".to_string(),
                one_claim("critic-a", "winner"),
            ]),
        );
        let outcome = decide_all(&repo, &logs, "t", VetoRun::Passed);
        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.lines.len(), 2);
        assert!(outcome.lines[0].contains("skipped"), "{:?}", outcome.lines);
        assert!(outcome.lines[1].contains("ESCALATE"), "{:?}", outcome.lines);
        fs::remove_dir_all(&repo).ok();
        fs::remove_dir_all(&logs).ok();
    }
}

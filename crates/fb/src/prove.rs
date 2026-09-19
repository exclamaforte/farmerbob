//! Turn TESTABLE claims into executed evidence.
//!
//! This is a behaviour-for-behaviour port of `fb-prove.sh`. A test is written to
//! assert the *reviewer's* expectation, so it fails when the reviewer was right
//! (a confirmed claim) and passes when they were wrong (a refuted claim). Every
//! such outcome is recorded, and a reference veto discards failures that also
//! occur against the merged implementation — a test that also fails on HEAD is
//! testing the claim's misreading of the spec, not a defect in this candidate.
//!
//! The shell script collapsed several "the command could not produce a number"
//! cases into a silent `0` (`grep` finding nothing, `2>/dev/null` swallowing a
//! compile error, `${x:-0}` defaulting an empty count). Each of those is now a
//! [`farmerbob_core::measurement::Measurement::Missing`] with a stated reason,
//! never a defaulted zero: a quantity is either
//! [`farmerbob_core::measurement::Measurement::Observed`] or it carries a reason
//! it is absent, and the public core below never lets the second read as the
//! first by accident.
//!
//! The public surface is [`run_cmd`]; everything below it is split into a pure,
//! input-independent core (count parsing, the veto arithmetic, the per-subject
//! result, the format, and the critic-credit aggregation) and an orchestration
//! layer that does the filesystem, `cargo` and prover work and feeds the core.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use farmerbob_core::measurement::Measurement;
use serde::Serialize;

/// The repo root: hardcoded to match the script's `$REPO`.
fn repo_root() -> PathBuf {
    PathBuf::from("/home/gabe/Documents/farmerbob")
}

/// The worktree root: `$HOME/.local/share/farmerbob/worktrees`.
fn wt_root() -> PathBuf {
    crate::paths::worktrees()
}

/// `$HOME/.local/share/farmerbob/logs`.
fn logs_dir() -> PathBuf {
    crate::paths::logs()
}

/// Whether the reference veto could be performed against the merged implementation.
///
/// The script distinguishes `ok` (the merged implementation built and ran) from
/// `unavailable` (HEAD has no reference for the module, or the reference did not
/// compile). The latter must NOT become a `0` in the failure count: it is a
/// genuinely different fact, recorded as [`Measurement::Missing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VetoState {
    /// The reference implementation was measured.
    Ok,
    /// No reference could be measured; confirmations are provisional.
    Unavailable,
}

impl VetoState {
    /// The literal string the script records in the per-subject JSON (`veto`).
    fn as_str(self) -> &'static str {
        match self {
            VetoState::Ok => "ok",
            VetoState::Unavailable => "unavailable",
        }
    }

    /// Whether the script would mark the subject `provisional`.
    fn is_unavailable(self) -> bool {
        matches!(self, VetoState::Unavailable)
    }
}

/// One subject's proof outcome, with every count a [`Measurement`] rather than a
/// bare value with a sentinel default.
///
/// `confirmed` is the number of candidate test *failures* that are unique to the
/// candidate (a reviewer was right); `refuted` is the number of candidate test
/// *passes* (a reviewer was wrong); `vetoed` is the number of candidate failures
/// that also occur on the merged implementation and so are not confirmations.
#[derive(Debug, Clone)]
pub struct SubjectResult {
    /// The implementation (worktree suffix) this result is about.
    pub subject: String,
    /// Candidate failures not also seen on the reference: the confirmations.
    pub confirmed: Measurement<u32>,
    /// Candidate passes: the refutations.
    pub refuted: Measurement<u32>,
    /// Candidate failures also seen on the reference: vetoed, not confirmations.
    pub vetoed: Measurement<u32>,
    /// Whether the reference veto was measured.
    pub veto: VetoState,
}

/// Extract a `N <word>` count from `cargo test` output, or
/// [`Measurement::Missing`] when the summary line is absent.
///
/// This is exactly the script's `grep -oE '[0-9]+ <word>' | head -1` except that
/// a missing line becomes [`Measurement::Missing`] (the suite did not compile or
/// produced no summary) instead of a defaulted `0`. An observed `0` — a real
/// "nothing failed" — stays [`Measurement::Observed`], which is a different fact
/// from "we could not see the result at all".
pub fn parse_test_count(output: &str, word: &str) -> Measurement<u32> {
    let bytes = output.as_bytes();
    let w = word.as_bytes();
    let mut i = 0;
    while i + w.len() <= bytes.len() {
        if &bytes[i..i + w.len()] == w {
            // The word must be preceded by a single space and then a run of digits:
            // `<digits> <word>` (cargo's `7 passed`), so a bare occurrence of the
            // word (e.g. "the test passed") does not match.
            if i > 0 && bytes[i - 1] == b' ' {
                let mut j = i - 1;
                while j > 0 && bytes[j - 1].is_ascii_digit() {
                    j -= 1;
                }
                if j < i - 1
                    && let Ok(n) = output[j..i - 1].parse::<u32>()
                {
                    return Measurement::observed(n);
                }
            }
        }
        i += 1;
    }
    Measurement::instrument_failed(&format!(
        "no `{word}` summary in test output; the suite may not have compiled or run"
    ))
}

/// The smaller of two counts, or [`Measurement::Missing`] unless both are observed.
///
/// Used for the reference veto: a failure is only "also on the reference" when the
/// reference was actually measured. If either side is absent, the minimum is
/// absent too — we will not invent a veto count.
fn min_meas(a: &Measurement<u32>, b: &Measurement<u32>) -> Measurement<u32> {
    match (a, b) {
        (Measurement::Observed(x), Measurement::Observed(y)) => Measurement::observed(*x.min(y)),
        (Measurement::Missing(m), _) | (_, Measurement::Missing(m)) => {
            Measurement::Missing(m.clone())
        }
    }
}

/// `a - b`, or [`Measurement::Missing`] unless both are observed.
///
/// The only arithmetic the veto needs: confirmed = candidate failures − vetoed.
/// When the vetoed amount is itself absent we cannot subtract it, so the
/// confirmation is absent rather than a guessed number.
fn sub_meas(a: &Measurement<u32>, b: &Measurement<u32>) -> Measurement<u32> {
    match (a, b) {
        (Measurement::Observed(x), Measurement::Observed(y)) => {
            Measurement::observed(x.saturating_sub(*y))
        }
        (Measurement::Missing(m), _) | (_, Measurement::Missing(m)) => {
            Measurement::Missing(m.clone())
        }
    }
}

/// The sum of three counts, or [`Measurement::Missing`] unless all three are
/// observed — the "proved" total must not be stated when part of it is unmeasured.
fn combine_total(
    confirmed: &Measurement<u32>,
    refuted: &Measurement<u32>,
    vetoed: &Measurement<u32>,
) -> Measurement<u32> {
    match (confirmed.value(), refuted.value(), vetoed.value()) {
        (Some(c), Some(r), Some(v)) => Measurement::observed(c + r + v),
        _ => Measurement::nothing_to_measure(
            "not all counts were observed; the proved total is withheld",
        ),
    }
}

/// Compute one subject's outcome from the three measured counts and the veto state.
///
/// `candidate_pass` and `candidate_fail` come from the candidate's own test run;
/// `reference_fail` comes from the merged implementation's run, or is
/// [`Measurement::Missing`] when the reference veto could not be performed
/// ([`VetoState::Unavailable`]). The veto arithmetic is:
///
/// * `vetoed = min(reference_fail, candidate_fail)` — failures also on HEAD.
/// * `confirmed = candidate_fail − vetoed` — candidate-unique failures.
/// * `refuted = candidate_pass` — candidate passes.
///
/// When the reference is unavailable, `reference_fail` is missing, which propagates
/// through `min` and `sub` so `vetoed` and `confirmed` are missing too: the
/// confirmations are explicitly *not* reported as a number, and [`VetoState`] marks
/// them provisional. The script instead reported a full confirmation with a
/// `provisional` flag — the silent lie this port closes.
pub fn prove_subject(
    subject: &str,
    candidate_pass: Measurement<u32>,
    candidate_fail: Measurement<u32>,
    reference_fail: Measurement<u32>,
    veto: VetoState,
) -> SubjectResult {
    let vetoed = min_meas(&reference_fail, &candidate_fail);
    let confirmed = sub_meas(&candidate_fail, &vetoed);
    SubjectResult {
        subject: subject.to_string(),
        confirmed,
        refuted: candidate_pass,
        vetoed,
        veto,
    }
}

/// Render a count for the printed line: the observed number, or `n/a` when the
/// count is absent. The script printed a `0` in this position; `n/a` keeps the
/// column shape other scripts grep while refusing to assert a measured zero that
/// was never measured.
fn shown(m: &Measurement<u32>) -> String {
    match m.value() {
        Some(n) => n.to_string(),
        None => "n/a".to_string(),
    }
}

/// Render the per-subject summary line, byte-for-byte with the script's `printf`.
///
/// `  %-22s proved %s: %s CONFIRMED, %s refuted, %s VETOED (also fail on the reference)`.
/// The `%-22s` left-justifies the subject to width 22 (no truncation, matching the
/// shell `%s` width); the counts are the observed numbers, or `n/a` when a count
/// is [`Measurement::Missing`].
pub fn format_subject_line(r: &SubjectResult) -> String {
    let total = combine_total(&r.confirmed, &r.refuted, &r.vetoed);
    format!(
        "  {:<22} proved {}: {} CONFIRMED, {} refuted, {} VETOED (also fail on the reference)\n",
        r.subject,
        shown(&total),
        shown(&r.confirmed),
        shown(&r.refuted),
        shown(&r.vetoed),
    )
}

/// A single critic's credit, exactly the script's `credit[critic]` dict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CriticCredit {
    /// TESTABLE claims by this critic that had a matching subject row.
    pub claims: u32,
    /// Of those, how many sat on a subject whose confirmation count was > 0.
    pub on_confirmed_subjects: u32,
}

/// The aggregated report.
pub struct Aggregate {
    /// Per-subject results, in the order they were processed.
    pub subjects: Vec<SubjectResult>,
    /// Critic credit, keyed by critic name.
    pub critics: BTreeMap<String, CriticCredit>,
    /// Sum of observed confirmed counts across subjects.
    pub confirmed_total: u32,
    /// Sum of observed refuted counts across subjects.
    pub refuted_total: u32,
}

/// A claim as read from the task's `claims.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ClaimRec {
    /// The claim classification, e.g. `TESTABLE`.
    #[serde(default)]
    kind: String,
    /// The implementation (worktree suffix) the claim is about.
    #[serde(default)]
    subject: String,
    /// The critic who raised the claim.
    #[serde(default)]
    critic: String,
}

/// The `claims.json` envelope.
#[derive(Debug, serde::Deserialize)]
struct ClaimsFile {
    #[serde(default)]
    claims: Vec<ClaimRec>,
}

/// Aggregate per-subject results into critic credit and the confirmed/refuted totals.
///
/// Mirrors the script's final python: for every TESTABLE claim, if a row exists for
/// its subject, the critic is credited with one claim; if that row's confirmation
/// count is truthy (observed and `> 0`), they are also credited with an
/// on-confirmed-subject. The confirmed/refuted totals sum only the *observed*
/// counts — an absent confirmation contributes nothing, never a silent `0`.
pub fn aggregate(rows: &[SubjectResult], claims: &[ClaimRec]) -> Aggregate {
    let mut by_subject: BTreeMap<&str, &SubjectResult> = BTreeMap::new();
    for r in rows {
        by_subject.insert(r.subject.as_str(), r);
    }

    let mut critics: BTreeMap<String, CriticCredit> = BTreeMap::new();
    for c in claims {
        if c.kind != "TESTABLE" {
            continue;
        }
        let Some(row) = by_subject.get(c.subject.as_str()) else {
            continue;
        };
        let entry = critics
            .entry(c.critic.clone())
            .or_insert_with(|| CriticCredit {
                claims: 0,
                on_confirmed_subjects: 0,
            });
        entry.claims += 1;
        if row.confirmed.value().map(|n| *n > 0).unwrap_or(false) {
            entry.on_confirmed_subjects += 1;
        }
    }

    let confirmed_total: u32 = rows
        .iter()
        .filter_map(|r| r.confirmed.value().copied())
        .sum();
    let refuted_total: u32 = rows.iter().filter_map(|r| r.refuted.value().copied()).sum();

    Aggregate {
        subjects: rows.to_vec(),
        critics,
        confirmed_total,
        refuted_total,
    }
}

/// Render the final summary lines, byte-for-byte with the script's closing python.
///
/// `  {tc} claims CONFIRMED by execution, {tr} refuted` then `-> {out}`. The script
/// prints these after `wait`; the leading blank line is preserved.
pub fn summary_text(confirmed_total: u32, refuted_total: u32, out: &Path) -> String {
    format!(
        "\n  {} claims CONFIRMED by execution, {} refuted\n-> {}\n",
        confirmed_total,
        refuted_total,
        out.display(),
    )
}

/// The on-disk shape of a per-subject record, matching the script's `json.dump`.
#[derive(Debug, Serialize)]
struct SubjectRecord {
    subject: String,
    confirmed: Option<u32>,
    refuted: Option<u32>,
    vetoed: Option<u32>,
    veto: String,
    provisional: bool,
}

impl SubjectResult {
    /// Project this core result into the JSON record shape.
    ///
    /// Confirmed/refuted/vetoed are `null` when [`Measurement::Missing`] — the honest
    /// replacement for the script's defaulted `0`. `veto` and `provisional` carry the
    /// veto state exactly as the script's `veto`/`provisional` fields.
    fn record(&self) -> SubjectRecord {
        SubjectRecord {
            subject: self.subject.clone(),
            confirmed: self.confirmed.value().copied(),
            refuted: self.refuted.value().copied(),
            vetoed: self.vetoed.value().copied(),
            veto: self.veto.as_str().to_string(),
            provisional: self.veto.is_unavailable(),
        }
    }
}

/// The on-disk shape of the final report, matching the script's `json.dump`.
#[derive(Serialize)]
struct FinalReport<'a> {
    subjects: Vec<SubjectRecord>,
    critics: &'a BTreeMap<String, CriticCredit>,
}

/// Serialise `value` as JSON with one-space indentation (Python `indent=1`), to match
/// the script's `json.dump(..., indent=1)`.
fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(
        &mut buf,
        serde_json::ser::PrettyFormatter::with_indent(b" "),
    );
    value
        .serialize(&mut ser)
        .with_context(|| format!("serialising report for {}", path.display()))?;
    std::fs::write(path, buf).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Public entry point, mirroring the script's positional parameters in order:
/// `bead` (`$1`), `krate` (`$2`), `target` (`$3`, repo-relative), `prover` (`$4`).
///
/// Returns the process exit code:
/// - `0` on a complete run that wrote `<logs>/<bead>.proved.json`
/// - `1` on a usage or I/O error
/// - `4` when there is no claims file (not applicable; matching `fb-prove.sh`)
pub fn run_cmd(bead: &str, krate: &str, target: &str, prover: &str) -> i32 {
    match run(bead, krate, target, prover) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// The real body of [`run_cmd`], returning its exit code or a structured error.
fn run(bead: &str, krate: &str, target: &str, prover: &str) -> Result<i32> {
    let claims_path = logs_dir().join(format!("{bead}.claims.json"));
    if !is_non_empty_file(&claims_path) {
        println!("prove: n/a, no claims file -- nothing was promoted, so there is nothing to prove");
        return Ok(4);
    }
    let claims = load_claims(&claims_path)?;
    let subjects = subjects_with_testable(&claims);

    // `echo "${#SUBJECTS[@]} implementations have claims against them"`
    println!(
        "{} implementations have claims against them",
        subjects.len()
    );

    let proofs = logs_dir().join("proofs").join(bead);
    std::fs::create_dir_all(&proofs).with_context(|| format!("creating {}", proofs.display()))?;

    let mut rows: Vec<SubjectResult> = Vec::new();
    for subject in &subjects {
        match process_subject(bead, krate, target, prover, subject, &proofs)? {
            Some((result, line)) => {
                print!("{line}");
                let rec = result.record();
                let json = serde_json::to_string(&rec)
                    .with_context(|| format!("serialising subject {subject}"))?;
                let subj_path = proofs.join(format!("{subject}.json"));
                std::fs::write(&subj_path, json)
                    .with_context(|| format!("writing {}", subj_path.display()))?;
                rows.push(result);
            }
            None => continue,
        }
    }

    let agg = aggregate(&rows, &claims);
    let out = logs_dir().join(format!("{bead}.proved.json"));
    let report = FinalReport {
        subjects: agg.subjects.iter().map(SubjectResult::record).collect(),
        critics: &agg.critics,
    };
    write_json_pretty(&out, &report)?;
    print!(
        "{}",
        summary_text(agg.confirmed_total, agg.refuted_total, &out)
    );
    Ok(0)
}

/// Read the task's `claims.json`.
fn load_claims(path: &Path) -> Result<Vec<ClaimRec>> {
    let txt = std::fs::read_to_string(path)
        .with_context(|| format!("reading claims file {}", path.display()))?;
    let file: ClaimsFile = serde_json::from_str(&txt)
        .with_context(|| format!("parsing claims file {}", path.display()))?;
    Ok(file.claims)
}

/// The subjects (implementation labels) that have at least one TESTABLE claim,
/// sorted and de-duplicated, matching the script's `mapfile`-fed `SUBJECTS`.
fn subjects_with_testable(claims: &[ClaimRec]) -> Vec<String> {
    let mut subs: Vec<String> = claims
        .iter()
        .filter(|c| c.kind == "TESTABLE")
        .map(|c| c.subject.clone())
        .collect();
    subs.sort();
    subs.dedup();
    subs
}

/// Process one subject end-to-end: build the prompt, run the prover and the
/// candidate suite, run the reference veto, and return the [`SubjectResult`] and
/// its printed line. Returns `Ok(None)` when the subject's worktree is absent or
/// it has no claims (the script's `[ -d "$sw" ] || exit 0` / empty-claims skip).
#[allow(clippy::too_many_arguments)]
fn process_subject(
    bead: &str,
    krate: &str,
    target: &str,
    prover: &str,
    subject: &str,
    proofs: &Path,
) -> Result<Option<(SubjectResult, String)>> {
    let sw = wt_root().join(format!("{bead}--{subject}"));
    if !sw.is_dir() {
        return Ok(None);
    }

    let repo = repo_root();
    let cand_tree = proofs.join(format!("{subject}.tree"));
    std::fs::remove_dir_all(&cand_tree).ok();
    std::fs::create_dir_all(&cand_tree)
        .with_context(|| format!("creating candidate tree {}", cand_tree.display()))?;
    copy_worktree(&sw, &cand_tree)?;

    // Build the prompt and dispatch the prover (best-effort; the script runs this
    // in a subshell that does not abort the run when the prover fails).
    if let Err(e) = run_prover(bead, krate, target, prover, subject, &repo, &cand_tree) {
        eprintln!("warn: prover for {subject} did not run: {e}");
    }

    // Run the candidate's own suite against the candidate tree.
    let cand_out = run_cargo_test(&cand_tree, krate);
    let candidate_pass = parse_test_count(&cand_out, "passed");
    let candidate_fail = parse_test_count(&cand_out, "failed");

    // The reference veto.
    let (reference_fail, veto) = reference_veto(&repo, krate, target, &cand_tree, proofs, subject);

    let result = prove_subject(
        subject,
        candidate_pass,
        candidate_fail,
        reference_fail,
        veto,
    );
    let line = format_subject_line(&result);
    Ok(Some((result, line)))
}

/// The farmerbob state directory root: `$HOME/.local/share/farmerbob/state`.
pub fn state_root() -> Measurement<PathBuf> {
    match std::env::var_os("HOME") {
        Some(home) => {
            Measurement::observed(PathBuf::from(home).join(".local/share/farmerbob/state"))
        }
        None => Measurement::instrument_failed("HOME is not set"),
    }
}

/// Compute the XDG data, state, and cache paths for a run.
pub fn environment_paths(state_root: &Path, run: &str) -> [PathBuf; 3] {
    let run_root = state_root.join(run);
    [
        run_root.join("data"),
        run_root.join("state"),
        run_root.join("cache"),
    ]
}

/// Run the prover (the script's `fb_launch`) best-effort, generating tests in the
/// candidate tree. Any failure is non-fatal: the candidate suite is simply empty
/// and `cargo test` reports no results, which [`parse_test_count`] records as
/// [`Measurement::Missing`] rather than a zero.
fn run_prover(
    bead: &str,
    krate: &str,
    target: &str,
    prover: &str,
    subject: &str,
    repo: &Path,
    cand_tree: &Path,
) -> Result<()> {
    let prompt_tpl = repo.join(".fb/prompts/_prove.md");
    let claims_path = logs_dir().join(format!("{bead}.claims.json"));
    let prompt_path = cand_tree.join(format!("{subject}.prompt.md"));
    let claims = std::fs::read_to_string(&claims_path)
        .with_context(|| format!("reading claims {}", claims_path.display()))?;
    let template = std::fs::read_to_string(&prompt_tpl)
        .with_context(|| format!("reading prover template {}", prompt_tpl.display()))?;

    let prompt = template
        .replace("{TARGET}", target)
        .replace("{CRATE}", krate)
        .replace("{CLAIMS}", &claims);
    std::fs::write(&prompt_path, prompt)
        .with_context(|| format!("writing prompt {}", prompt_path.display()))?;

    let state_root = match state_root() {
        Measurement::Observed(root) => root,
        Measurement::Missing(reason) => {
            anyhow::bail!("cannot resolve state root: {reason:?}");
        }
    };
    let run = format!("{bead}--{subject}");
    let [data_home, state_home, cache_home] = environment_paths(&state_root, &run);
    for directory in [&data_home, &state_home, &cache_home] {
        std::fs::create_dir_all(directory)
            .with_context(|| format!("creating directory {}", directory.display()))?;
    }

    let prompt_str = std::fs::read_to_string(&prompt_path).unwrap_or_default();
    let launch = format!(
        "set -e; . {}/fb-launch.sh; fb_launch {} {} {}",
        repo.display(),
        sh_quote(prover),
        sh_quote(&prompt_str),
        sh_quote(cand_tree.to_string_lossy().as_ref()),
    );
    let log = cand_tree.join(format!("{subject}.log"));
    let out = Command::new("bash")
        .args(["-c", &launch])
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_STATE_HOME", &state_home)
        .env("XDG_CACHE_HOME", &cache_home)
        .output()
        .context("spawning prover")?;
    std::fs::write(&log, out.stdout).ok();
    std::fs::write(log.with_extension("err"), out.stderr).ok();
    Ok(())
}

/// Perform the reference veto and return `(reference_fail, veto_state)`.
///
/// When HEAD has no reference for the module under test, or the reference does not
/// compile, the reference could not be measured: `reference_fail` is
/// [`Measurement::Missing`] and the veto is [`VetoState::Unavailable`]. The script
/// reported `reffail=0` in that case (and a `provisional` flag); here the absence is
/// the type, so a missing reference never reads as "checked and clean".
fn reference_veto(
    repo: &Path,
    krate: &str,
    target: &str,
    cand_tree: &Path,
    proofs: &Path,
    subject: &str,
) -> (Measurement<u32>, VetoState) {
    let head_target = repo.join(target);
    if !is_non_empty_file(&head_target) {
        println!(
            "  {subject}: VETO UNAVAILABLE -- HEAD has no reference for {target}; confirmations are PROVISIONAL"
        );
        return (
            Measurement::nothing_to_measure(
                "HEAD has no reference implementation for the module under test",
            ),
            VetoState::Unavailable,
        );
    }

    let ref_tree = proofs.join(format!("{subject}.ref"));
    std::fs::remove_dir_all(&ref_tree).ok();
    std::fs::create_dir_all(&ref_tree).ok();
    copy_worktree(repo, &ref_tree).ok();

    if let Err(e) = graft_proved_module(&cand_tree.join(target), &ref_tree.join(target)) {
        eprintln!("warn: reference graft failed for {subject}: {e}");
        return (
            Measurement::instrument_failed("reference graft failed"),
            VetoState::Unavailable,
        );
    }

    let out = run_cargo_test(&ref_tree, krate);
    if is_compile_error(&out) {
        return (
            Measurement::instrument_failed("reference implementation did not compile"),
            VetoState::Unavailable,
        );
    }
    (parse_test_count(&out, "failed"), VetoState::Ok)
}

/// True when `path` exists, is a file, and is non-empty.
fn is_non_empty_file(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(m) => m.is_file() && m.len() > 0,
        Err(_) => false,
    }
}

/// Append the candidate's `#[cfg(test)] mod proved { ... }` block to the reference
/// target, mirroring the script's python graft. A missing block is not an error —
/// the reference simply has no proved tests and reports `0` failures.
fn graft_proved_module(cand_target: &Path, ref_target: &Path) -> Result<()> {
    let cand_src = std::fs::read_to_string(cand_target)
        .with_context(|| format!("reading candidate target {}", cand_target.display()))?;
    let block = match extract_proved_block(&cand_src) {
        Some(b) => b,
        None => return Ok(()),
    };
    let mut ref_src = std::fs::read_to_string(ref_target)
        .with_context(|| format!("reading reference target {}", ref_target.display()))?;
    ref_src.push('\n');
    ref_src.push_str(&block);
    std::fs::write(ref_target, ref_src)
        .with_context(|| format!("writing grafted reference {}", ref_target.display()))?;
    Ok(())
}

/// Find the `#[cfg(test)] mod proved { ... }` block (greedy to end of file),
/// mirroring the script's `re.search(r'#\[cfg\(test\)\]\s*mod proved\s*\{.*', …)`.
fn extract_proved_block(src: &str) -> Option<String> {
    let marker = "#[cfg(test)]";
    let mut from = 0;
    while let Some(pos) = src[from..].find(marker) {
        let abs = from + pos;
        let tail = &src[abs + marker.len()..];
        if tail.trim_start().starts_with("mod proved") {
            return Some(src[abs..].to_string());
        }
        from = abs + marker.len();
    }
    None
}

/// Copy `Cargo.toml`, `Cargo.lock`, `rustfmt.toml` and `crates/` from `from` to `to`,
/// mirroring the script's `tar cf - Cargo.toml Cargo.lock crates rustfmt.toml`.
fn copy_worktree(from: &Path, to: &Path) -> Result<()> {
    for name in ["Cargo.toml", "Cargo.lock", "rustfmt.toml"] {
        let s = from.join(name);
        if s.exists() {
            std::fs::copy(&s, to.join(name)).with_context(|| format!("copying {name}"))?;
        }
    }
    let from_crates = from.join("crates");
    let to_crates = to.join("crates");
    if from_crates.is_dir() {
        copy_dir(&from_crates, &to_crates)?;
    }
    Ok(())
}

/// Recursively copy a directory tree.
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).with_context(|| format!("copying {}", path.display()))?;
        }
    }
    Ok(())
}

/// Run `cargo test -p <krate>` in `dir` and return the combined stdout+stderr.
///
/// A compile failure or timeout yields an empty/error string; [`parse_test_count`]
/// turns the absence of a summary line into [`Measurement::Missing`], so an
/// unbuildable suite is never silently counted as zero failures.
fn run_cargo_test(dir: &Path, krate: &str) -> String {
    let mut cmd = Command::new("cargo");
    cmd.args(["test", "-p", krate])
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let pid = child.id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(400);
    let killer = std::thread::Builder::new()
        .name("prove-cargo-timeout".to_string())
        .spawn(move || {
            while std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-9", &format!("-{pid}")])
                    .status();
            }
        })
        .ok();
    let output = child.wait_with_output();
    if let Some(k) = killer {
        let _ = k.join();
    }
    match output {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        }
        Err(_) => String::new(),
    }
}

/// True when `output` shows a compile failure: `could not compile` or a line
/// beginning `error[E<digits>:`.
fn is_compile_error(out: &str) -> bool {
    if out.contains("could not compile") {
        return true;
    }
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("error[E") {
            let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && rest.chars().nth(digits) == Some(':') {
                return true;
            }
        }
    }
    false
}

/// Single-quote a string for safe interpolation into a `bash -c` script.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A worked example pinned byte-for-byte against the script's `printf`.
    ///
    /// Candidate: 7 passed, 3 failed; reference: 1 failed. So `vetoed = min(1,3) = 1`,
    /// `confirmed = 3 - 1 = 2`, `refuted = 7`, total proved = 2 + 7 + 1 = 10.
    fn worked_example() -> SubjectResult {
        prove_subject(
            "sensitivity",
            Measurement::observed(7),
            Measurement::observed(3),
            Measurement::observed(1),
            VetoState::Ok,
        )
    }

    #[test]
    fn parse_test_count_observes_a_real_number() {
        let out = "test result: ok. 7 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out";
        assert_eq!(parse_test_count(out, "passed"), Measurement::observed(7));
        assert_eq!(parse_test_count(out, "failed"), Measurement::observed(2));
    }

    #[test]
    fn parse_test_count_observes_zero_not_missing() {
        let out = "test result: ok. 0 passed; 0 failed";
        assert_eq!(parse_test_count(out, "passed"), Measurement::observed(0));
        assert_eq!(parse_test_count(out, "failed"), Measurement::observed(0));
    }

    #[test]
    fn parse_test_count_is_missing_when_summary_absent() {
        let out = "error: could not compile\n";
        assert!(parse_test_count(out, "passed").absent().is_some());
        assert!(parse_test_count(out, "failed").absent().is_some());
        // A bare occurrence of the word with no preceding count must not match.
        assert!(
            parse_test_count("the test passed cleanly", "passed")
                .absent()
                .is_some()
        );
    }

    #[test]
    fn veto_min_and_confirm_subtract_are_correct() {
        let r = worked_example();
        assert_eq!(r.vetoed, Measurement::observed(1));
        assert_eq!(r.confirmed, Measurement::observed(2));
        assert_eq!(r.refuted, Measurement::observed(7));
        // Total proved = confirmed + refuted + vetoed.
        assert_eq!(
            combine_total(&r.confirmed, &r.refuted, &r.vetoed),
            Measurement::observed(10)
        );
    }

    #[test]
    fn veto_unavailable_propagates_missing_confirmation() {
        // Reference could not be measured: vetoed and confirmed are Missing, not 0.
        let r = prove_subject(
            "s",
            Measurement::observed(5),
            Measurement::observed(3),
            Measurement::nothing_to_measure("no reference"),
            VetoState::Unavailable,
        );
        assert_eq!(
            r.vetoed,
            Measurement::Missing(farmerbob_core::measurement::Absent::NothingToMeasure {
                reason: "no reference".to_string()
            })
        );
        assert!(r.confirmed.absent().is_some());
        assert!(r.veto.is_unavailable());
        assert_eq!(r.refuted, Measurement::observed(5));
    }

    #[test]
    fn candidate_unmeasured_makes_everything_missing() {
        let r = prove_subject(
            "s",
            Measurement::instrument_failed("no candidate output"),
            Measurement::instrument_failed("no candidate output"),
            Measurement::observed(0),
            VetoState::Ok,
        );
        assert!(r.confirmed.absent().is_some());
        assert!(r.refuted.absent().is_some());
        assert!(r.vetoed.absent().is_some());
    }

    #[test]
    fn subject_line_format_is_byte_for_byte() {
        let r = worked_example();
        let got = format_subject_line(&r);
        // `%-22s` left-justifies "sensitivity" (11 chars) to width 22.
        let want = format!(
            "  {:<22} proved 10: 2 CONFIRMED, 7 refuted, 1 VETOED (also fail on the reference)\n",
            "sensitivity"
        );
        assert_eq!(
            got, want,
            "per-subject line must match the script's printf exactly"
        );
    }

    #[test]
    fn summary_format_is_byte_for_byte() {
        let got = summary_text(2, 7, Path::new("/logs/t.proved.json"));
        let want = "\n  2 claims CONFIRMED by execution, 7 refuted\n-> /logs/t.proved.json\n";
        assert_eq!(
            got, want,
            "summary lines must match the script's closing python"
        );
    }

    #[test]
    fn aggregate_credits_critics_and_totals() {
        // Two subjects: one confirmed (count 2), one with no confirmation.
        let rows = vec![
            SubjectResult {
                subject: "alpha".to_string(),
                confirmed: Measurement::observed(2),
                refuted: Measurement::observed(4),
                vetoed: Measurement::observed(0),
                veto: VetoState::Ok,
            },
            SubjectResult {
                subject: "beta".to_string(),
                confirmed: Measurement::observed(0),
                refuted: Measurement::observed(1),
                vetoed: Measurement::observed(0),
                veto: VetoState::Ok,
            },
        ];
        let claims = vec![
            ClaimRec {
                kind: "TESTABLE".to_string(),
                subject: "alpha".to_string(),
                critic: "c1".to_string(),
            },
            ClaimRec {
                kind: "TESTABLE".to_string(),
                subject: "alpha".to_string(),
                critic: "c1".to_string(),
            },
            ClaimRec {
                kind: "TESTABLE".to_string(),
                subject: "beta".to_string(),
                critic: "c2".to_string(),
            },
            ClaimRec {
                kind: "EDITORIAL".to_string(),
                subject: "alpha".to_string(),
                critic: "c3".to_string(),
            },
            ClaimRec {
                kind: "TESTABLE".to_string(),
                subject: "ghost".to_string(),
                critic: "c4".to_string(),
            },
        ];
        let agg = aggregate(&rows, &claims);
        // c1 has two TESTABLE claims on "alpha", both on a confirmed subject.
        let c1 = agg.critics.get("c1").expect("c1 credited");
        assert_eq!(c1.claims, 2);
        assert_eq!(c1.on_confirmed_subjects, 2);
        // c2 credited one claim, but "beta" confirmed count is 0.
        let c2 = agg.critics.get("c2").expect("c2 credited");
        assert_eq!(c2.claims, 1);
        assert_eq!(c2.on_confirmed_subjects, 0);
        // Non-TESTABLE and subject-with-no-row claims are skipped.
        assert!(!agg.critics.contains_key("c3"));
        assert!(!agg.critics.contains_key("c4"));
        // Totals sum observed confirmed/refuted across rows.
        assert_eq!(agg.confirmed_total, 2);
        assert_eq!(agg.refuted_total, 5);
    }

    #[test]
    fn final_report_json_round_trips_with_indent_one() {
        let rows = [worked_example()];
        let mut critics = BTreeMap::new();
        critics.insert(
            "c1".to_string(),
            CriticCredit {
                claims: 1,
                on_confirmed_subjects: 1,
            },
        );
        let report = FinalReport {
            subjects: rows.iter().map(SubjectResult::record).collect(),
            critics: &critics,
        };
        let mut buf = Vec::new();
        let mut ser = serde_json::Serializer::with_formatter(
            &mut buf,
            serde_json::ser::PrettyFormatter::with_indent(b" "),
        );
        report.serialize(&mut ser).unwrap();
        let txt = String::from_utf8(buf).unwrap();
        assert!(txt.starts_with("{\n \""), "top level indented by one space");
        assert!(txt.contains("\"subjects\""));
        assert!(txt.contains("\"critics\""));
        assert!(txt.contains("\"confirmed\": 2"));
        assert!(txt.contains("\"refuted\": 7"));
        assert!(txt.contains("\"veto\": \"ok\""));
        assert!(txt.contains("\"provisional\": false"));
        let _: serde_json::Value = serde_json::from_str(&txt).unwrap();
    }

    #[test]
    fn subject_record_nulls_when_missing() {
        let r = prove_subject(
            "s",
            Measurement::observed(5),
            Measurement::observed(3),
            Measurement::nothing_to_measure("no reference"),
            VetoState::Unavailable,
        );
        let rec = r.record();
        assert!(rec.confirmed.is_none());
        assert!(rec.vetoed.is_none());
        assert_eq!(rec.refuted, Some(5));
        assert_eq!(rec.veto, "unavailable");
        assert!(rec.provisional);
    }

    #[test]
    fn environment_paths_are_isolated_and_stable() {
        let root = PathBuf::from("/tmp/fb-prove-state");
        let first = environment_paths(&root, "task--subject1");
        let second = environment_paths(&root, "task--subject2");
        let repeat = environment_paths(&root, "task--subject1");

        // Clause 1: Different run values receive different XDG_DATA_HOME values.
        assert_ne!(first[0], second[0]);
        assert_ne!(first, second);

        // Clause 2: Same run receives the same value.
        assert_eq!(first, repeat);

        // Clause 3: All three variables are set, and all three point inside that run's directory.
        let run_root = root.join("task--subject1");
        assert!(first.iter().all(|path| path.starts_with(&run_root)));
        assert_eq!(first[0], run_root.join("data"));
        assert_eq!(first[1], run_root.join("state"));
        assert_eq!(first[2], run_root.join("cache"));
    }

    #[test]
    fn different_subjects_get_different_directories() {
        let root = PathBuf::from("/tmp/fb-prove-state");
        let task = "sample-task";
        let run_a = format!("{task}--subject-a");
        let run_b = format!("{task}--subject-b");
        let env_a = environment_paths(&root, &run_a);
        let env_b = environment_paths(&root, &run_b);

        // Clause 5: Two different subjects of the same task get different directories.
        assert_ne!(env_a, env_b);
        assert_ne!(env_a[0], env_b[0]);
        assert_ne!(env_a[1], env_b[1]);
        assert_ne!(env_a[2], env_b[2]);
    }

    #[test]
    fn missing_claims_file_returns_not_applicable_and_creates_no_directory() {
        let bead = format!("fb-prove-test-missing-{}", std::process::id());
        let claims_file = logs_dir().join(format!("{bead}.claims.json"));
        let proofs_dir = logs_dir().join("proofs").join(&bead);
        let _ = std::fs::remove_file(&claims_file);
        let _ = std::fs::remove_dir_all(&proofs_dir);

        // Clause 6: Exit code 4 when there is no claims file.
        // Boundary: ZERO subjects: nothing is spawned and nothing is created.
        let code = run_cmd(&bead, "fb", "crates/fb/src/prove.rs", "dummy-prover");
        assert_eq!(code, 4, "missing claims file must return exit code 4");
        assert!(
            !proofs_dir.exists(),
            "no directory must appear for a task with no claims"
        );
    }

    #[test]
    fn empty_claims_file_returns_not_applicable() {
        let bead = format!("fb-prove-test-empty-{}", std::process::id());
        let claims_file = logs_dir().join(format!("{bead}.claims.json"));
        let proofs_dir = logs_dir().join("proofs").join(&bead);
        let _ = std::fs::create_dir_all(logs_dir());
        let _ = std::fs::write(&claims_file, "");
        let _ = std::fs::remove_dir_all(&proofs_dir);

        let code = run_cmd(&bead, "fb", "crates/fb/src/prove.rs", "dummy-prover");
        let _ = std::fs::remove_file(&claims_file);
        assert_eq!(code, 4, "empty claims file must return exit code 4");
        assert!(
            !proofs_dir.exists(),
            "no directory must appear for a task with no claims"
        );
    }

    #[test]
    fn doc_comment_names_all_three_codes() {
        let text = std::fs::read_to_string("crates/fb/src/prove.rs")
            .or_else(|_| std::fs::read_to_string("src/prove.rs"))
            .expect("prove.rs source text");
        let doc_start = text.find("pub fn run_cmd").expect("run_cmd definition");
        let doc_prefix = &text[..doc_start];
        let doc_comment = doc_prefix
            .lines()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            doc_comment.contains('0'),
            "doc comment must name exit code 0"
        );
        assert!(
            doc_comment.contains('1'),
            "doc comment must name exit code 1"
        );
        assert!(
            doc_comment.contains('4'),
            "doc comment must name exit code 4"
        );
    }
}

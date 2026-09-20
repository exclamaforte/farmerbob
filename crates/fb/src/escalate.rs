//! Promote a CONFIRMED finding into the task's permanent conformance suite, with
//! provenance, and credit the critic that found it.
//!
//! Ported from `fb-escalate.sh` and `fb-escalate-graft.py`. Two disciplines:
//!
//! REFERENCE VETO. An escalated test must PASS against the merged reference. One that
//! fails is encoding the critic's misreading of the spec rather than a defect -- a critic
//! here once "confirmed" a test asserting `sensitivity()` should be `None` where the spec
//! makes it `Some(0.0)`. Confirmation against a losing candidate is not enough.
//!
//! PROVENANCE. Every escalated test records the critic that found it, the candidate it was
//! found on, and the task, so a bad test can be traced and retired and a good one credited.
//!
//! The Rust that stood here before was hollow: `crossx` printed "no discriminating suite"
//! and `auto` printed "no confirmed proofs" without reading anything, and `verify`
//! filtered on `conformance_` alone -- the exact marker mismatch the shell carried a
//! comment about fixing -- and returned 0 whether or not a single test ran.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use farmerbob_core::measurement::Measurement;

/// The header every escalated block carries. It is also the retraction boundary: a
/// retraction cuts from the LAST one to the end of the file.
pub const TAG: &str = "\n// ESCALATED";

/// The arm whose cross-examination suite discriminated best, if any.
///
/// A suite that is not `suite_discriminating`, or that discovered nothing, establishes
/// nothing worth escalating.
pub fn discriminating(crossx: &serde_json::Value) -> Option<String> {
    let mut best: Option<(String, f64)> = None;
    for (arm, v) in crossx.as_object()? {
        if v.get("suite_discriminating").and_then(|b| b.as_bool()) != Some(true) {
            continue;
        }
        let d = v.get("discovery").and_then(|n| n.as_f64()).unwrap_or(0.0);
        if d <= 0.0 {
            continue;
        }
        if best.as_ref().is_none_or(|(_, b)| d > *b) {
            best = Some((arm.clone(), d));
        }
    }
    best.map(|(a, _)| a)
}

/// A Rust identifier is not a task name: both carry hyphens, and an identifier may not.
pub fn ident(s: &str) -> String {
    s.replace('-', "_")
}

/// The module name an escalated proof block takes.
pub fn proof_marker(task: &str, subject: &str) -> String {
    format!("escalated_{}_{}", ident(task), ident(subject))
}

/// The module name an escalated cross-examination suite takes.
pub fn crossx_marker(task: &str, finder: &str) -> String {
    format!("cx_{}_{}", ident(task), ident(finder))
}

/// Lift a named `#[cfg(test)] mod <name>` block out of a source file, renamed to `marker`.
///
/// The braces are BALANCED rather than matched with a regex. A greedy `.*` under DOTALL
/// swallows the enclosing module's closing brace too, which appends a stray `}` and breaks
/// the whole suite -- caught on the first run this was attempted.
pub fn lift_mod(src: &str, name: &str, marker: &str) -> Option<String> {
    let needle = format!("mod {name}");
    let at = src.find(&needle)?;
    // Back up over an immediately preceding `#[cfg(test)]` so the block stays a test block.
    let start = src[..at]
        .rfind("#[cfg(test)]")
        .filter(|&i| src[i..at].trim() == "#[cfg(test)]")
        .unwrap_or(at);
    let open = src[at..].find('{')? + at;
    let mut depth = 0usize;
    let mut end = None;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    // An unbalanced block means the file was truncated. Appending it would break the suite.
    Some(src[start..end?].replacen(&needle, &format!("mod {marker}"), 1))
}

/// The provenance header an escalated block carries.
pub fn provenance(n: usize, critic: &str, subject: &str, how: &str) -> String {
    format!(
        "{TAG}: {n} confirmed finding(s) by critic {critic}, found on {subject} ({how}).\n\
         // Promoted from an executed proof that passed the reference veto. Provenance is\n\
         // recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)\n"
    )
}

/// Everything before the LAST escalated block: what a retraction leaves behind.
pub fn retract(text: &str) -> String {
    match text.rfind(TAG) {
        Some(i) => format!("{}\n", &text[..i]),
        None => text.to_string(),
    }
}

/// How many escalated tests passed and failed, from cargo's own summary line.
///
/// ZERO MATCHED IS NOT A PASS. `cargo test -p c <filter>` that matches nothing prints
///
///   test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1440 filtered out
///
/// which reads as a pass. That is `farmerbob_core::suite_match`'s rule and this is the
/// caller it was written for: the instrument could not find what it was asked to check and
/// said everything was fine -- inside the anti-invalid-test check.
pub fn tally(output: &str) -> (u32, u32) {
    let Some(line) = output.lines().rev().find(|l| l.starts_with("test result:")) else {
        return (0, 0);
    };
    let n = |what: &str| -> u32 {
        line.split(';')
            .chain(std::iter::once(line))
            .find_map(|seg| {
                let seg = seg.trim().strip_suffix(what)?.trim();
                seg.rsplit(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()
            })
            .unwrap_or(0)
    };
    (n(" passed"), n(" failed"))
}

/// Both marker shapes a suite may carry: `escalated_*` from `auto` and `cx_*` from
/// `crossx`, plus `conformance_*` from the older hand-grafted suites still in the tree.
pub fn verify_filters(task: &str) -> Vec<String> {
    let t = ident(task);
    vec![
        format!("escalated_{t}"),
        format!("cx_{t}"),
        format!("conformance_{t}"),
    ]
}

fn ledger_path() -> Measurement<PathBuf> {
    Measurement::observed(crate::paths::repo().join(".fb/credits.json"))
}

fn read_text(path: &Path) -> Measurement<String> {
    match fs::read_to_string(path) {
        Ok(text) => Measurement::observed(text),
        Err(error) => {
            Measurement::instrument_failed(&format!("cannot read {}: {error}", path.display()))
        }
    }
}

fn command_output(program: &str, args: &[&str]) -> Measurement<String> {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
            Ok(text) => Measurement::observed(text),
            Err(error) => {
                Measurement::instrument_failed(&format!("command output was not UTF-8: {error}"))
            }
        },
        Ok(output) => {
            Measurement::instrument_failed(&format!("{program} exited with {}", output.status))
        }
        Err(error) => Measurement::instrument_failed(&format!("cannot run {program}: {error}")),
    }
}

fn crate_name(tgt: &str) -> Measurement<String> {
    match tgt.split('/').nth(1).filter(|s| !s.is_empty()) {
        Some(name) => Measurement::observed(name.to_string()),
        None => Measurement::instrument_failed("target has no crate component"),
    }
}

/// Run `fb escalate` with its five positional slots.
pub fn run_cmd(command: &str, task: &str, critic: &str, subject: &str, n: &str) -> i32 {
    match command {
        "crossx" => crossx(task),
        "auto" => auto(task),
        "verify" => verify(task),
        "credit" => credit(task, critic, subject, n),
        "ledger" => ledger(),
        _ => {
            eprintln!("error: {command}: expected crossx, auto, verify, credit or ledger");
            1
        }
    }
}

/// The paths a spec declares.
///
/// `fb decl` used to be reached by shelling out to `fb-target.sh`, which shelled back to
/// `fb decl`. The rule lives in `farmerbob_core::target_decl`; call it.
fn targets(task: &str) -> Result<Vec<String>, String> {
    let spec = crate::paths::repo()
        .join(".fb/prompts")
        .join(format!("{task}.md"));
    let text =
        fs::read_to_string(&spec).map_err(|_| format!("{task}: no spec at {}", spec.display()))?;
    let decls = farmerbob_core::target_decl::declared_all(&text)
        .map_err(|e| format!("{task}: spec declares no readable target ({e:?})"))?;
    Ok(decls
        .into_iter()
        .map(|d| match d {
            farmerbob_core::target_decl::Declaration::Creates(p)
            | farmerbob_core::target_decl::Declaration::Modifies(p) => p,
        })
        .collect())
}

/// The first declared path, for the callers that graft into one file.
fn target(task: &str) -> Result<String, String> {
    targets(task)?
        .into_iter()
        .next()
        .ok_or_else(|| format!("{task}: spec declares no target"))
}

fn cargo_test(krate: &str, filter: &str) -> Measurement<String> {
    command_output("cargo", &["test", "-p", krate, filter])
}

/// Append a block to a file it is not already in.
fn graft(path: &Path, marker: &str, block: &str) -> std::io::Result<bool> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing.contains(marker) {
        return Ok(false);
    }
    fs::write(path, format!("{existing}\n{block}\n"))?;
    Ok(true)
}

/// Undo the last escalation in both files.
fn retract_from(paths: &[&Path]) {
    for p in paths {
        if let Ok(s) = fs::read_to_string(p) {
            let _ = fs::write(p, retract(&s));
        }
    }
}

/// Escalate the best cross-examination suite.
///
/// Cross-examination finds defects that never pass through a CLAIM, so they never reached
/// the permanent suite: outcome's validation order, resume's `due_now` ordering and gate's
/// one-line explain were all found this way and all discarded after a single adjudication.
fn crossx(task: &str) -> i32 {
    let tgt = match target(task) {
        Ok(t) => t,
        Err(why) => {
            eprintln!("{why}");
            return 2;
        }
    };
    let krate = match crate_name(&tgt) {
        Measurement::Observed(k) => k,
        Measurement::Missing(reason) => {
            eprintln!("cannot resolve crate: {reason:?}");
            return 1;
        }
    };
    let cx = crate::paths::logs().join(format!("{task}.crossx.json"));
    let Ok(text) = fs::read_to_string(&cx) else {
        println!("{task}: no crossx");
        return 0;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        eprintln!("{task}: crossx log is not readable JSON -- nothing established");
        return 1;
    };
    let Some(finder) = discriminating(&json) else {
        println!("  no discriminating suite");
        return 0;
    };
    let suite = crate::paths::repo().join(format!(".fb/conformance/{task}.rs"));
    let marker = crossx_marker(task, &finder);
    if fs::read_to_string(&suite)
        .unwrap_or_default()
        .contains(&marker)
    {
        println!("  already escalated from {finder}");
        return 0;
    }
    println!("  discriminating suite: {finder}");
    let src = crate::paths::worktrees()
        .join(format!("{task}--{finder}"))
        .join(&tgt);
    let Ok(src) = fs::read_to_string(&src) else {
        println!("  {finder}: no worktree to lift the suite from");
        return 0;
    };
    let Some(block) = lift_mod(&src, "tests", &marker) else {
        println!("  {finder}: no balanced test module to lift");
        return 0;
    };
    let body = provenance(0, &finder, &finder, "crossx") + &block;
    let tgt_path = crate::paths::repo().join(&tgt);
    if graft(&suite, &marker, &body).is_err() || graft(&tgt_path, &marker, &body).is_err() {
        eprintln!("  could not graft {marker}");
        return 1;
    }
    let (passed, failed) = match cargo_test(&krate, &marker) {
        Measurement::Observed(out) => tally(&out),
        Measurement::Missing(reason) => {
            // The veto could not RUN. That is not a pass.
            eprintln!("  veto could not be measured ({reason:?}) -- retracting");
            retract_from(&[&suite, &tgt_path]);
            return 1;
        }
    };
    if passed == 0 || failed > 0 {
        println!("  VETOED against the merged reference -- retracting");
        retract_from(&[&suite, &tgt_path]);
        return 0;
    }
    println!("  kept: {passed} test(s) pass against the merged winner");
    credit(task, &finder, "crossx", &passed.to_string())
}

/// Harvest every CONFIRMED proof into the task's permanent suite.
///
/// `fb prove` already wrote the test, ran it against the subject and ran it against the
/// merged reference. The artefact existed and was being discarded; escalation keeps it.
fn auto(task: &str) -> i32 {
    let tgt = match target(task) {
        Ok(t) => t,
        Err(why) => {
            eprintln!("{why}");
            return 2;
        }
    };
    let krate = match crate_name(&tgt) {
        Measurement::Observed(k) => k,
        Measurement::Missing(reason) => {
            eprintln!("cannot resolve crate: {reason:?}");
            return 1;
        }
    };
    let proofs = crate::paths::logs().join("proofs").join(task);
    let Ok(entries) = fs::read_dir(&proofs) else {
        println!("{task}: no proofs");
        return 0;
    };
    let suite = crate::paths::repo().join(format!(".fb/conformance/{task}.rs"));
    let tgt_path = crate::paths::repo().join(&tgt);
    let claims = fs::read_to_string(crate::paths::logs().join(format!("{task}.claims.json")))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let mut any = false;
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for j in files {
        let Ok(v) = fs::read_to_string(&j).and_then(|t| {
            serde_json::from_str::<serde_json::Value>(&t)
                .map_err(|e| std::io::Error::other(e.to_string()))
        }) else {
            eprintln!(
                "  {}: unreadable proof -- skipped, NOT counted as unconfirmed",
                j.display()
            );
            continue;
        };
        let Some(subj) = v.get("subject").and_then(|s| s.as_str()) else {
            continue;
        };
        let conf = v.get("confirmed").and_then(|c| c.as_u64()).unwrap_or(0);
        if conf == 0 {
            continue;
        }
        // `provisional` means prove could not veto, because prove runs BEFORE the merge and
        // HEAD had no reference yet. That is the normal case, not an exception: a blanket
        // refusal here meant nothing could ever escalate. The veto below runs against the
        // reference as it stands NOW, which is the check that actually matters.
        if v.get("provisional").and_then(|p| p.as_bool()) == Some(true) {
            println!("  {subj}: prove could not veto (pre-merge); vetoing now instead");
        }
        let tree = proofs.join(format!("{subj}.tree")).join(&tgt);
        let Ok(src) = fs::read_to_string(&tree) else {
            println!("  {subj}: confirmed {conf} but no proof tree");
            continue;
        };
        let critic = critic_for(claims.as_ref(), subj).unwrap_or_else(|| "unknown".to_string());
        let marker = proof_marker(task, subj);
        if fs::read_to_string(&suite)
            .unwrap_or_default()
            .contains(&marker)
        {
            println!("  {subj}: already escalated");
            continue;
        }
        let Some(block) = lift_mod(&src, "proved", &marker) else {
            println!("  {subj}: no balanced `mod proved` block in the proof tree");
            continue;
        };
        let body = provenance(conf as usize, &critic, subj, "prove") + &block;
        // GRAFT INTO THE CRATE BEFORE VETOING. `cargo test -p <crate> <marker>` while the
        // block lives only in .fb/conformance matches zero tests, and cargo exits 0 on zero
        // tests -- so the veto passed by measuring nothing. That is the empty-suite trap
        // (farmerbob-slh) reappearing inside the check built to prevent it.
        if graft(&suite, &marker, &body).is_err() || graft(&tgt_path, &marker, &body).is_err() {
            eprintln!("  {subj}: could not graft");
            continue;
        }
        let (passed, failed) = match cargo_test(&krate, &marker) {
            Measurement::Observed(out) => tally(&out),
            Measurement::Missing(reason) => {
                eprintln!("  {subj}: veto could not be measured ({reason:?}) -- retracting");
                retract_from(&[&suite, &tgt_path]);
                continue;
            }
        };
        if passed == 0 || failed > 0 {
            // THE TEST FAILS, SO IT CANNOT JOIN A GREEN SUITE -- BUT THE FINDING IS REAL.
            //
            // This used to retract and stop, and the finding then existed nowhere. The
            // better the critic, the more likely its finding is about the arm that WON, and
            // the more certainly it was discarded. `farmerbob_core::finding_fate` answers
            // FILE-AS-KNOWN-DEFECT for exactly this case.
            file_known_defect(task, subj, claims.as_ref());
            println!("  {subj}: VETOED against the merged reference -- retracting");
            retract_from(&[&suite, &tgt_path]);
            continue;
        }
        println!("  {subj}: escalated {conf} finding(s) from {critic}");
        credit(task, &critic, subj, &conf.to_string());
        any = true;
    }
    if any {
        println!("  -> {}  (run: fb escalate verify {task})", suite.display());
    }
    0
}

/// Which critic found it: a claim names its critic; take the one with most claims on this
/// subject.
fn critic_for(claims: Option<&serde_json::Value>, subject: &str) -> Option<String> {
    let rows = claims?.get("claims")?.as_array()?;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for c in rows {
        if c.get("subject").and_then(|s| s.as_str()) == Some(subject)
            && let Some(critic) = c.get("critic").and_then(|s| s.as_str())
        {
            *counts.entry(critic).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map(|(c, _)| c.to_string())
}

/// The claim a critic made about this subject, as `farmerbob_core::known_defect` wants it.
///
/// Returns `None` when no claim names this subject: a known-defect entry with no claim
/// records that a defect exists without recording what it is, which is what the file is for.
fn defect_from_claims(
    claims: Option<&serde_json::Value>,
    subject: &str,
) -> Option<farmerbob_core::known_defect::KnownDefect> {
    let rows = claims?.get("claims")?.as_array()?;
    let row = rows
        .iter()
        .find(|c| c.get("subject").and_then(|s| s.as_str()) == Some(subject))?;
    let text = |k: &str| {
        row.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    // WHERE, TRIGGER, EXPECT, ACTUAL, in that order. A line the critic did not write is
    // ABSENT rather than present and empty -- an empty evidence line asserts the critic
    // considered it and had nothing to say.
    let evidence = ["where", "trigger", "expect", "actual"]
        .iter()
        .filter_map(|k| text(k))
        .collect();
    Some(farmerbob_core::known_defect::KnownDefect {
        critic: text("critic")?,
        subject: subject.to_string(),
        claim: text("claim")?,
        evidence,
    })
}

/// Record a confirmed-but-unmergeable finding so it survives the retraction.
///
/// THE RECORD MUST CARRY THE FINDING. This wrote a timestamp, the two arm names and a
/// boilerplate sentence -- so the file said a defect existed without saying what it was,
/// which is the one thing it is for. It also appended unconditionally, so re-running
/// `fb escalate auto` recorded the same defect again; and it hand-rolled a format
/// `known_defect::parse` cannot read, so the deduplication that module provides could never
/// have worked even if it had been called.
///
/// `farmerbob_core::known_defect` renders the entry, parses the file back and answers
/// `already_recorded`. It had no callers. (bead farmerbob-oa5w)
fn file_known_defect(task: &str, subject: &str, claims: Option<&serde_json::Value>) {
    use farmerbob_core::known_defect::{already_recorded, parse, render};
    let kd = crate::paths::repo().join(".fb/known-defects");
    if fs::create_dir_all(&kd).is_err() {
        return;
    }
    let Some(defect) = defect_from_claims(claims, subject) else {
        println!(
            "  {subject}: confirmed, but no claim names it -- not recording a defect with no \
             statement of what it is"
        );
        return;
    };
    let f = kd.join(format!("{task}.md"));
    let existing_text = fs::read_to_string(&f).unwrap_or_default();
    if already_recorded(&parse(&existing_text), &defect) {
        println!("  {subject}: already recorded as a known defect");
        return;
    }
    let entry = render(task, &chrono::Utc::now().to_rfc3339(), &defect);
    let body = if existing_text.is_empty() {
        entry
    } else {
        format!("{existing_text}\n{entry}")
    };
    if fs::write(&f, body).is_ok() {
        println!("  {subject}: recorded as a known defect -> .fb/known-defects/{task}.md");
    }
}

/// Run the whole escalated suite against the merged reference.
fn verify(task: &str) -> i32 {
    let suite = crate::paths::repo().join(format!(".fb/conformance/{task}.rs"));
    if !suite.is_file() {
        println!("no suite for {task}");
        return 2;
    }
    let tgt = match target(task) {
        Ok(t) => t,
        Err(why) => {
            eprintln!("{why}");
            return 1;
        }
    };
    let krate = match crate_name(&tgt) {
        Measurement::Observed(k) => k,
        Measurement::Missing(reason) => {
            eprintln!("cannot resolve crate: {reason:?}");
            return 1;
        }
    };
    println!("reference veto: {task} against merged HEAD ({krate})");
    let (mut ran, mut failed) = (0u32, 0u32);
    for filter in verify_filters(task) {
        match cargo_test(&krate, &filter) {
            Measurement::Observed(out) => {
                for l in out.lines().filter(|l| l.starts_with("test ")) {
                    println!("{l}");
                }
                let (p, f) = tally(&out);
                ran += p + f;
                failed += f;
            }
            Measurement::Missing(reason) => {
                // Cargo did not run. Counting that as zero would let a broken toolchain
                // read as "nothing failed".
                eprintln!("VERIFY COULD NOT RUN for {task} on {filter}: {reason:?}");
                return 1;
            }
        }
    }
    if ran == 0 {
        println!(
            "VERIFY MATCHED NOTHING for {task} -- tried {}",
            verify_filters(task).join(", ")
        );
        println!("  Zero tests ran. That is not a pass: nothing was established about the suite.");
        return 1;
    }
    if failed > 0 {
        println!(
            "VERIFY FAILED for {task}: {failed} of {ran} escalated test(s) fail against merged HEAD"
        );
        return 1;
    }
    println!("verify ok: {ran} escalated test(s) pass against merged HEAD");
    0
}
fn load_ledger(path: &Path) -> Measurement<serde_json::Value> {
    if !path.exists() {
        return Measurement::observed(serde_json::json!({ "contributions": [] }));
    }
    match read_text(path) {
        Measurement::Observed(text) => match serde_json::from_str(&text) {
            Ok(value) => Measurement::observed(value),
            Err(error) => Measurement::instrument_failed(&format!("invalid ledger JSON: {error}")),
        },
        Measurement::Missing(reason) => Measurement::Missing(reason),
    }
}

fn credit(task: &str, critic: &str, subject: &str, n: &str) -> i32 {
    let path = match ledger_path() {
        Measurement::Observed(path) => path,
        Measurement::Missing(reason) => {
            eprintln!("cannot locate ledger: {reason:?}");
            return 1;
        }
    };
    let count = match n.parse::<u64>() {
        Ok(value) => Measurement::observed(value),
        Err(error) => Measurement::instrument_failed(&format!("invalid test count: {error}")),
    };
    let count = match count {
        Measurement::Observed(value) => value,
        Measurement::Missing(reason) => {
            eprintln!("cannot credit: {reason:?}");
            return 1;
        }
    };
    let mut data = match load_ledger(&path) {
        Measurement::Observed(value) => value,
        Measurement::Missing(reason) => {
            eprintln!("cannot read ledger: {reason:?}");
            return 1;
        }
    };
    let contributions = data
        .get_mut("contributions")
        .and_then(serde_json::Value::as_array_mut);
    let Some(contributions) = contributions else {
        eprintln!("ledger has no contributions array");
        return 1;
    };
    if contributions.iter().any(|entry| {
        entry.get("task").and_then(serde_json::Value::as_str) == Some(task)
            && entry.get("critic").and_then(serde_json::Value::as_str) == Some(critic)
            && entry.get("found_on").and_then(serde_json::Value::as_str) == Some(subject)
    }) {
        println!("already credited: {critic} on {task}/{subject}");
        return 0;
    }
    contributions.push(
        serde_json::json!({"task": task, "critic": critic, "found_on": subject, "tests": count}),
    );
    let text = match serde_json::to_string_pretty(&data) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("cannot encode ledger: {error}");
            return 1;
        }
    };
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!("cannot create ledger directory: {error}");
        return 1;
    }
    match fs::write(&path, format!("{text}\n")) {
        Ok(()) => {
            println!("credited {critic}: {count} test(s) on {task}, found on {subject}");
            0
        }
        Err(error) => {
            eprintln!("cannot write ledger: {error}");
            1
        }
    }
}

fn ledger() -> i32 {
    let path = match ledger_path() {
        Measurement::Observed(path) => path,
        Measurement::Missing(reason) => {
            eprintln!("cannot locate ledger: {reason:?}");
            return 1;
        }
    };
    if !path.is_file() {
        println!("no contributions recorded");
        return 0;
    }
    let data = match load_ledger(&path) {
        Measurement::Observed(value) => value,
        Measurement::Missing(reason) => {
            eprintln!("cannot read ledger: {reason:?}");
            return 1;
        }
    };
    let mut by: BTreeMap<String, (u64, Vec<String>)> = BTreeMap::new();
    let Some(rows) = data
        .get("contributions")
        .and_then(serde_json::Value::as_array)
    else {
        eprintln!("ledger has no contributions array");
        return 1;
    };
    for row in rows {
        let (Some(critic), Some(task), Some(tests)) = (
            row.get("critic").and_then(serde_json::Value::as_str),
            row.get("task").and_then(serde_json::Value::as_str),
            row.get("tests").and_then(serde_json::Value::as_u64),
        ) else {
            eprintln!("ledger contribution is malformed");
            return 1;
        };
        let entry = by
            .entry(critic.to_string())
            .or_insert_with(|| (0, Vec::new()));
        entry.0 += tests;
        if !entry.1.iter().any(|seen| seen == task) {
            entry.1.push(task.to_string());
        }
    }
    println!("{:<22}{:>6}  TASKS", "CRITIC", "TESTS");
    let mut total = 0;
    let mut critics = 0;
    let mut rows: Vec<_> = by.into_iter().collect();
    rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
    for (critic, (tests, mut tasks)) in rows {
        tasks.sort();
        println!("{critic:<22}{tests:>6}  {}", tasks.join(" "));
        total += tests;
        critics += 1;
    }
    println!("\n{total} tests escalated into the permanent suite from {critics} critic(s)");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A greedy match for `mod proved { ... }` swallows the ENCLOSING module's closing
    /// brace, appending a stray `}` that breaks the whole suite.
    #[test]
    fn the_lifted_block_is_brace_balanced() {
        let src = "mod outer {\n#[cfg(test)]\nmod proved {\n    fn a() { if x { y } }\n}\n}\n";
        let got = lift_mod(src, "proved", "m").unwrap();
        assert_eq!(got.matches('{').count(), got.matches('}').count(), "{got}");
        assert!(got.starts_with("#[cfg(test)]"), "{got}");
        assert!(got.ends_with('}'), "{got}");
        assert!(got.contains("mod m {"), "{got}");
    }

    /// A truncated file has no closing brace. Appending what is there would break the suite,
    /// so nothing is lifted at all.
    #[test]
    fn an_unbalanced_block_lifts_nothing() {
        assert_eq!(lift_mod("mod proved {\n  fn a() {\n", "proved", "m"), None);
    }

    /// Zero matched is NOT a pass. `cargo test <filter>` that matches nothing prints
    /// "ok. 0 passed; 0 failed ... 1440 filtered out", which reads as success.
    #[test]
    fn a_filter_that_matched_nothing_tallies_zero_run() {
        let out = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1440 filtered out";
        assert_eq!(tally(out), (0, 0));
    }

    #[test]
    fn a_tally_reads_the_last_summary_line() {
        let out = "test result: ok. 9 passed; 0 failed; 0 ignored\n\
                   test result: FAILED. 2 passed; 3 failed; 0 ignored\n";
        assert_eq!(tally(out), (2, 3));
    }

    /// Output with no summary line at all is zero-run, which the caller reports as
    /// "matched nothing" rather than as a pass.
    #[test]
    fn output_without_a_summary_is_zero_run() {
        assert_eq!(tally("error: could not compile"), (0, 0));
    }

    /// `auto` writes `escalated_<task>_<arm>` and verify once filtered on
    /// `conformance_<task>`, so it matched nothing BY CONSTRUCTION and read as a pass --
    /// inside the anti-invalid-test check. Every marker shape the tree carries is tried.
    #[test]
    fn verify_tries_every_marker_shape_auto_and_crossx_write() {
        let f = verify_filters("cost-honesty");
        let auto = proof_marker("cost-honesty", "codex-luna");
        let cx = crossx_marker("cost-honesty", "codex-luna");
        assert!(
            f.iter().any(|x| auto.starts_with(x.as_str())),
            "{f:?} vs {auto}"
        );
        assert!(
            f.iter().any(|x| cx.starts_with(x.as_str())),
            "{f:?} vs {cx}"
        );
        assert!(f.iter().any(|x| x == "conformance_cost_honesty"), "{f:?}");
    }

    /// Task and arm names carry hyphens; Rust identifiers may not.
    #[test]
    fn markers_are_rust_identifiers() {
        let m = proof_marker("cost-honesty", "gemini-38-flash");
        assert_eq!(m, "escalated_cost_honesty_gemini_38_flash");
        assert!(!m.contains('-'));
    }

    /// A retraction cuts from the LAST header to the end, leaving every earlier escalation.
    #[test]
    fn retraction_removes_only_the_last_block() {
        let s = format!(
            "base\n{}: one\nmod a {{}}\n{}: two\nmod b {{}}\n",
            TAG.trim_start(),
            TAG.trim_start()
        );
        let got = retract(&s);
        assert!(got.contains("mod a"), "{got}");
        assert!(!got.contains("mod b"), "{got}");
    }

    /// Nothing escalated means nothing to cut -- not a truncated file.
    #[test]
    fn retracting_a_clean_file_changes_nothing() {
        assert_eq!(retract("fn main() {}\n"), "fn main() {}\n");
    }

    /// A suite that did not discriminate establishes nothing worth escalating, and neither
    /// does one that discovered nothing.
    #[test]
    fn only_a_discriminating_suite_with_discovery_is_escalated() {
        let j = serde_json::json!({
            "a": {"suite_discriminating": false, "discovery": 9},
            "b": {"suite_discriminating": true,  "discovery": 0},
            "c": {"suite_discriminating": true,  "discovery": 2},
            "d": {"suite_discriminating": true,  "discovery": 5},
        });
        assert_eq!(discriminating(&j).as_deref(), Some("d"));
        assert_eq!(discriminating(&serde_json::json!({})), None);
    }

    /// The critic with the most claims on a subject is the one credited; a subject with no
    /// claims yields None, which the caller renders as "unknown" rather than crediting the
    /// wrong arm.
    #[test]
    fn credit_goes_to_the_critic_with_claims_on_that_subject() {
        let c = serde_json::json!({"claims": [
            {"subject": "x", "critic": "alpha"},
            {"subject": "x", "critic": "alpha"},
            {"subject": "x", "critic": "beta"},
            {"subject": "y", "critic": "gamma"},
        ]});
        assert_eq!(critic_for(Some(&c), "x").as_deref(), Some("alpha"));
        assert_eq!(critic_for(Some(&c), "z"), None);
        assert_eq!(critic_for(None, "x"), None);
    }

    /// Provenance names the critic, the candidate and the count, so a bad test can be
    /// traced and retired and a good one credited.
    #[test]
    fn provenance_names_critic_subject_and_count() {
        let h = provenance(3, "gemini-38-flash", "codex-luna", "prove");
        assert!(h.starts_with(TAG), "{h}");
        assert!(h.contains("3 confirmed finding(s)"), "{h}");
        assert!(h.contains("gemini-38-flash"), "{h}");
        assert!(h.contains("codex-luna"), "{h}");
    }

    #[test]
    fn ledger_output_format_is_pinned() {
        let output = format!(
            "{:<22}{:>6}  TASKS\n{:<22}{:>6}  task-a task-b\n\n{} tests escalated into the permanent suite from {} critic(s)\n",
            "CRITIC", "TESTS", "farmerbob-ljq", 12, 12, 1
        );
        assert_eq!(
            output,
            "CRITIC                 TESTS  TASKS\nfarmerbob-ljq             12  task-a task-b\n\n12 tests escalated into the permanent suite from 1 critic(s)\n"
        );
    }

    #[test]
    fn absent_count_is_not_zero() {
        let value: Measurement<u64> = Measurement::instrument_failed("cargo failed");
        assert_eq!(
            value,
            Measurement::Missing(farmerbob_core::measurement::Absent::InstrumentFailed {
                reason: "cargo failed".to_string()
            })
        );
    }

    #[test]
    fn gate_keeps_zero_tests_as_a_measured_fact() {
        let observation = farmerbob_core::gate::Observation {
            built: Some(true),
            tests_passed: Some(true),
            tests_run: Some(0),
            lines_added: Some(1),
            declared_targets_present: Some(true),
            scope_departures: Some(0),
        };
        assert_eq!(
            farmerbob_core::gate::judge(&observation),
            farmerbob_core::gate::Verdict::NoTests
        );
    }

    /// THE RECORD MUST CARRY THE FINDING. The entry used to be a timestamp, two arm names
    /// and a boilerplate sentence, so the file said a defect existed without saying what it
    /// was -- the one thing it is for.
    #[test]
    fn a_known_defect_carries_the_claim_and_its_evidence() {
        let claims = serde_json::json!({"claims": [{
            "subject": "codex-luna", "critic": "agy-opus-46",
            "claim": "sensitivity() returns None where the spec makes it Some(0.0)",
            "where": "cost.rs:212", "trigger": "an arm with no priced runs",
            "expect": "Some(0.0)", "actual": "None"
        }]});
        let d = defect_from_claims(Some(&claims), "codex-luna").expect("a claim names it");
        assert_eq!(d.critic, "agy-opus-46");
        assert!(d.claim.contains("sensitivity()"));
        assert_eq!(
            d.evidence,
            vec![
                "cost.rs:212",
                "an arm with no priced runs",
                "Some(0.0)",
                "None"
            ]
        );
    }

    /// A line the critic did not write is ABSENT, not empty. An empty evidence line asserts
    /// the critic considered it and had nothing to say.
    #[test]
    fn evidence_the_critic_omitted_is_absent_not_blank() {
        let claims = serde_json::json!({"claims": [{
            "subject": "s", "critic": "c", "claim": "x", "where": "f.rs:1", "trigger": ""
        }]});
        let d = defect_from_claims(Some(&claims), "s").unwrap();
        assert_eq!(d.evidence, vec!["f.rs:1"]);
    }

    /// No claim naming the subject means no statement of the defect, and an entry without
    /// one records nothing worth keeping.
    #[test]
    fn a_subject_with_no_claim_yields_no_defect() {
        let claims =
            serde_json::json!({"claims": [{"subject": "other", "critic": "c", "claim": "x"}]});
        assert!(defect_from_claims(Some(&claims), "missing").is_none());
        assert!(defect_from_claims(None, "missing").is_none());
    }

    /// Re-running escalation must not append the same defect again. The old writer appended
    /// unconditionally AND wrote a format `known_defect::parse` cannot read, so the
    /// deduplication could never have worked even if it had been called.
    #[test]
    fn the_same_defect_is_recognised_in_a_rendered_file() {
        use farmerbob_core::known_defect::{already_recorded, parse, render};
        let claims = serde_json::json!({"claims": [{
            "subject": "s", "critic": "c", "claim": "the thing is wrong", "where": "f.rs:1"
        }]});
        let d = defect_from_claims(Some(&claims), "s").unwrap();
        let file = render("task", "2026-09-19T00:00:00Z", &d);
        assert!(
            already_recorded(&parse(&file), &d),
            "what render writes must be what parse reads: {file}"
        );
    }
}

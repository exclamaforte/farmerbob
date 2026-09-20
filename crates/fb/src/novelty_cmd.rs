//! `fb novelty` — exploration credit on its own axis.
//!
//! Bead farmerbob-x81s.9: a verified-correct candidate under an approach
//! no one has verified before is a novelty win, recorded alongside the
//! existing outcome rather than folded into it. Speedup is written down
//! next to the win and never summed with it. The pure ledger lives in
//! [`farmerbob_core::approach`]; this module is file I/O plus the CLI:
//! one JSON ledger per task under `<state>/novelty/`, read back by `log`
//! and, later, injected into specs by the negative-results bead.
//!
//! Exit codes: 0 whenever the ledger is read or written; 2 for an unknown
//! verdict, an unreadable candidate, or an unwritable store. A missing
//! ledger on `log` is exit 1: the task name is probably wrong, and that
//! should read as an error rather than an empty research programme.

use std::path::{Path, PathBuf};

/// Ledger file for a task: sanitised like an artifact pin name, so a
/// task can never escape its directory.
pub fn ledger_path(root: &Path, task: &str) -> Result<PathBuf, String> {
    if task.is_empty()
        || task.contains("..")
        || !task
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
    {
        return Err(format!("invalid task name: {task:?}"));
    }
    let safe: String = task
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(root.join("novelty").join(format!("{safe}.json")))
}

fn load_ledger(root: &Path, task: &str) -> Result<farmerbob_core::approach::Ledger, String> {
    let path = ledger_path(root, task)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("unreadable ledger at {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(farmerbob_core::approach::Ledger::new(task))
        }
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn save_ledger(
    root: &Path,
    task: &str,
    ledger: &farmerbob_core::approach::Ledger,
) -> Result<(), String> {
    let path = ledger_path(root, task)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text =
        serde_json::to_string_pretty(ledger).map_err(|e| format!("cannot encode ledger: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Record one attempt at a task. Returns the exit code.
#[allow(clippy::too_many_arguments)] // Eight CLI flags in, one struct out would only rename the call sites.
pub fn record_at(
    root: &Path,
    task: &str,
    arm: &str,
    file: &str,
    verdict: &str,
    detail: &str,
    speedup: Option<f64>,
    asked: Option<&str>,
    out: &mut dyn std::io::Write,
) -> i32 {
    if !farmerbob_core::approach::VERDICTS.contains(&verdict) {
        let _ = writeln!(
            out,
            "error: unknown verdict {verdict:?}, want one of {}",
            farmerbob_core::approach::VERDICTS.join("|")
        );
        return 2;
    }
    if let Some(strategy) = asked
        && farmerbob_core::approach::STRATEGIES
            .iter()
            .all(|(name, _)| *name != strategy)
    {
        let names: Vec<&str> = farmerbob_core::approach::STRATEGIES
            .iter()
            .map(|(name, _)| *name)
            .collect();
        let _ = writeln!(
            out,
            "error: unknown strategy {strategy:?}, want one of {}",
            names.join("|")
        );
        return 2;
    }
    let src = match std::fs::read_to_string(Path::new(file)) {
        Ok(src) => src,
        Err(e) => {
            let _ = writeln!(out, "error: cannot read {file}: {e}");
            return 2;
        }
    };
    let mut ledger = match load_ledger(root, task) {
        Ok(ledger) => ledger,
        Err(why) => {
            let _ = writeln!(out, "error: {why}");
            return 2;
        }
    };
    let signature = farmerbob_core::approach::signature(&src);
    let key = signature.key();
    let novel = ledger.record(arm, &signature, verdict, detail, speedup, asked);
    if let Err(why) = save_ledger(root, task, &ledger) {
        let _ = writeln!(out, "error: {why}");
        return 2;
    }
    if novel {
        let _ = writeln!(out, "novelty: NEW APPROACH {key} by {arm}");
    } else {
        let _ = writeln!(out, "novelty: seen {key} by {arm} ({verdict})");
    }
    0
}

/// Print a task's ledger. Returns the exit code.
pub fn log_at(root: &Path, task: &str, out: &mut dyn std::io::Write) -> i32 {
    let path = match ledger_path(root, task) {
        Ok(path) => path,
        Err(why) => {
            let _ = writeln!(out, "error: {why}");
            return 2;
        }
    };
    if !path.is_file() {
        let _ = writeln!(out, "no novelty ledger for {task}");
        return 1;
    }
    match load_ledger(root, task) {
        Ok(ledger) => {
            let _ = writeln!(out, "{}", ledger.render());
            0
        }
        Err(why) => {
            let _ = writeln!(out, "error: {why}");
            2
        }
    }
}

/// Print a task's spec-ready digest: verified approaches with the bar to
/// beat, dead ends with the harness's measured reasons (bead
/// farmerbob-x81s.11). Written for paste-injection into later specs.
/// A missing ledger briefs to the first-wave line rather than erroring:
/// a spec carrying that line is honest about being wave one.
pub fn brief_at(root: &Path, task: &str, out: &mut dyn std::io::Write) -> i32 {
    let ledger = match load_ledger(root, task) {
        Ok(ledger) => ledger,
        Err(why) => {
            let _ = writeln!(out, "error: {why}");
            return 2;
        }
    };
    let _ = writeln!(out, "{}", ledger.brief());
    0
}

/// Resolve the state root: `$FB_STATE`, else the real state dir.
fn root() -> PathBuf {
    crate::paths::state()
}

#[allow(clippy::too_many_arguments)] // Eight CLI flags in, one struct out would only rename the call sites.
pub fn run_record(
    task: &str,
    arm: &str,
    file: &str,
    verdict: &str,
    detail: &str,
    speedup: Option<f64>,
    asked: Option<&str>,
    out: &mut dyn std::io::Write,
) -> i32 {
    record_at(
        &root(),
        task,
        arm,
        file,
        verdict,
        detail,
        speedup,
        asked,
        out,
    )
}

pub fn run_log(task: &str, out: &mut dyn std::io::Write) -> i32 {
    log_at(&root(), task, out)
}

pub fn run_brief(task: &str, out: &mut dyn std::io::Write) -> i32 {
    brief_at(&root(), task, out)
}

pub fn run_strategy(task: &str, out: &mut dyn std::io::Write) -> i32 {
    strategy_at(&root(), task, out)
}

/// Print a task's strategy report: per asked strategy, attempts with
/// asked-versus-produced match (bead farmerbob-x81s.10). A missing
/// ledger reports no assignments, exit 1 like `log`.
pub fn strategy_at(root: &Path, task: &str, out: &mut dyn std::io::Write) -> i32 {
    let path = match ledger_path(root, task) {
        Ok(path) => path,
        Err(why) => {
            let _ = writeln!(out, "error: {why}");
            return 2;
        }
    };
    if !path.is_file() {
        let _ = writeln!(out, "no novelty ledger for {task}");
        return 1;
    }
    match load_ledger(root, task) {
        Ok(ledger) => {
            let _ = writeln!(out, "{}", ledger.strategy_report());
            0
        }
        Err(why) => {
            let _ = writeln!(out, "error: {why}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fb-novelty-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    fn candidate(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("candidate");
        path.to_string_lossy().into_owned()
    }

    /// Record then log round-trips through real files: a novelty win
    /// first, seen after, adverse never novel.
    #[test]
    fn record_log_round_trip() {
        let root = sandbox("roundtrip");
        let dir = root.join("cands");
        std::fs::create_dir_all(&dir).expect("cands");
        let eager = candidate(&dir, "e.py", "import torch\ndef f(x):\n    return x\n");
        let tf32 = candidate(
            &dir,
            "t.py",
            "import torch\ntorch.backends.cuda.matmul.allow_tf32 = True\ndef f(x):\n    return x\n",
        );
        let mut out = Vec::new();
        assert_eq!(
            record_at(
                &root,
                "task:1",
                "a",
                &eager,
                "correct",
                "ok",
                Some(1.0),
                None,
                &mut out
            ),
            0
        );
        assert!(
            String::from_utf8_lossy(&out).contains("NEW APPROACH"),
            "{out:?}"
        );
        let mut out = Vec::new();
        assert_eq!(
            record_at(
                &root,
                "task:1",
                "b",
                &eager,
                "correct",
                "ok",
                Some(1.7),
                None,
                &mut out
            ),
            0
        );
        assert!(String::from_utf8_lossy(&out).contains("seen"), "{out:?}");
        let mut out = Vec::new();
        assert_eq!(
            record_at(
                &root,
                "task:1",
                "c",
                &tf32,
                "incorrect",
                "values",
                None,
                None,
                &mut out
            ),
            0
        );
        assert!(String::from_utf8_lossy(&out).contains("seen"), "{out:?}");
        let mut out = Vec::new();
        assert_eq!(log_at(&root, "task:1", &mut out), 0);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("1 approaches"), "{text}");
        assert!(text.contains("3 attempts"), "{text}");
        assert!(text.contains('a'), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Unknown verdicts and missing files are usage errors, and a
    /// missing ledger on log names the task.
    #[test]
    fn bad_inputs_are_loud() {
        let root = sandbox("bad");
        let mut out = Vec::new();
        assert_eq!(
            record_at(
                &root,
                "t",
                "a",
                "/tmp/x.py",
                "brilliant",
                "",
                None,
                None,
                &mut out
            ),
            2
        );
        assert_eq!(
            record_at(
                &root,
                "t",
                "a",
                "/tmp/fb-novelty-no-such-file.py",
                "correct",
                "",
                None,
                None,
                &mut out
            ),
            2
        );
        assert_eq!(log_at(&root, "never-seen", &mut out), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task names sanitise to their directory: slashes and colons cannot
    /// escape it.
    #[test]
    fn task_names_cannot_escape() {
        let root = PathBuf::from("/s");
        let path = ledger_path(&root, "kernelbench:level1/97_X").expect("path");
        assert_eq!(
            path,
            PathBuf::from("/s/novelty/kernelbench_level1_97_X.json")
        );
        assert!(ledger_path(&root, "").is_err());
        assert!(ledger_path(&root, "../x").is_err());
    }

    /// Brief renders the digest: tried approaches with the bar, dead ends
    /// with harness reasons; missing ledgers brief first-wave, exit 0.
    #[test]
    fn brief_renders_digest_and_first_wave() {
        let root = sandbox("brief");
        let dir = root.join("cands");
        std::fs::create_dir_all(&dir).expect("cands");
        let eager = candidate(&dir, "e.py", "import torch\ndef f(x):\n    return x\n");
        let mut out = Vec::new();
        assert_eq!(
            record_at(
                &root,
                "t",
                "a",
                &eager,
                "correct",
                "ok",
                Some(1.7),
                None,
                &mut out
            ),
            0
        );
        assert_eq!(
            record_at(
                &root,
                "t",
                "b",
                &eager,
                "incorrect",
                "values",
                None,
                None,
                &mut out
            ),
            0
        );
        let mut out = Vec::new();
        assert_eq!(brief_at(&root, "t", &mut out), 0);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("TRIED"), "{text}");
        assert!(text.contains("1.700x"), "{text}");
        assert!(text.contains("DEAD END"), "{text}");
        assert!(text.contains("incorrect (b)"), "{text}");
        let mut out = Vec::new();
        assert_eq!(brief_at(&root, "unseen-task", &mut out), 0);
        assert!(
            String::from_utf8_lossy(&out).contains("first wave"),
            "{out:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Strategies record with the attempt and report both directions: a
    /// typo'd strategy is refused, and asked-versus-produced judges.
    #[test]
    fn strategy_records_and_reports() {
        let root = sandbox("strategy");
        let dir = root.join("cands");
        std::fs::create_dir_all(&dir).expect("cands");
        let eager = candidate(&dir, "e.py", "import torch\ndef f(x):\n    return x\n");
        let mut out = Vec::new();
        assert_eq!(
            record_at(
                &root,
                "t",
                "a",
                &eager,
                "correct",
                "ok",
                Some(1.0),
                Some("eager-baseline"),
                &mut out
            ),
            0
        );
        assert_eq!(
            record_at(
                &root,
                "t",
                "b",
                &eager,
                "correct",
                "ok",
                Some(1.0),
                Some("triton-tiled"),
                &mut out
            ),
            0
        );
        // Unknown strategies are refused, not recorded.
        let mut out = Vec::new();
        assert_eq!(
            record_at(
                &root,
                "t",
                "c",
                &eager,
                "correct",
                "ok",
                None,
                Some("nope"),
                &mut out
            ),
            2
        );
        let mut out = Vec::new();
        assert_eq!(strategy_at(&root, "t", &mut out), 0);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("strategy eager-baseline"), "{text}");
        assert!(text.contains("a correct 1.000x [MATCH]"), "{text}");
        assert!(text.contains("strategy triton-tiled"), "{text}");
        assert!(text.contains("b correct 1.000x [MISS]"), "{text}");
        assert_eq!(strategy_at(&root, "unseen-task", &mut out), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}

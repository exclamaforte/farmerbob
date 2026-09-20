//! `fb diversity` — cluster one wave's candidates by approach.
//!
//! Bead farmerbob-x81s.8: a wave that collapsed to one approach must read
//! as one approach, not N successes. The pure work lives in
//! [`farmerbob_core::approach`]; this module is the CLI skin: parse
//! `arm=path` specs, read the files, print the report. Exit 0 whenever
//! the report prints; exit 2 for unusable specs or unreadable files.
//! Telling the harness there is nothing to cluster is not an error: zero
//! candidates is an empty report, and a single candidate is one approach.

use std::path::Path;

/// One parsed `--candidate arm=path` spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// Arm name, for the report.
    pub arm: String,
    /// Candidate file path.
    pub path: String,
}

/// Parse one `arm=path` spec. Both sides must be non-empty; the `=` is
/// required. A missing separator or an empty arm names a typo about what
/// is being clustered, and clustering the wrong set silently is how a
/// collapsed wave keeps reading as N successes.
pub fn parse_spec(spec: &str) -> Result<Spec, String> {
    let Some((arm, path)) = spec.split_once('=') else {
        return Err(format!("unparseable --candidate {spec:?}, want arm=path"));
    };
    let (arm, path) = (arm.trim(), path.trim());
    if arm.is_empty() || path.is_empty() {
        return Err(format!("unparseable --candidate {spec:?}, want arm=path"));
    }
    Ok(Spec {
        arm: arm.to_string(),
        path: path.to_string(),
    })
}

/// Cluster already-read sources and render the report.
pub fn report(candidates: &[(String, String)]) -> String {
    let items: Vec<farmerbob_core::approach::Candidate> = candidates
        .iter()
        .map(|(arm, src)| farmerbob_core::approach::Candidate { arm, src })
        .collect();
    let clusters = farmerbob_core::approach::cluster(&items);
    farmerbob_core::approach::render(candidates.len(), &clusters)
}

/// Read every spec's file and report. Returns the exit code.
pub fn run(specs: &[String], out: &mut dyn std::io::Write) -> i32 {
    let mut parsed = Vec::new();
    for spec in specs {
        match parse_spec(spec) {
            Ok(s) => parsed.push(s),
            Err(why) => {
                let _ = writeln!(out, "error: {why}");
                return 2;
            }
        }
    }
    let mut sources = Vec::new();
    for spec in &parsed {
        match std::fs::read_to_string(Path::new(&spec.path)) {
            Ok(src) => sources.push((spec.arm.clone(), src)),
            Err(e) => {
                let _ = writeln!(out, "error: cannot read {}: {e}", spec.path);
                return 2;
            }
        }
    }
    let _ = writeln!(out, "{}", report(&sources));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Specs need both sides: a bare path or a bare arm is a typo about
    /// what is being clustered.
    #[test]
    fn specs_require_arm_and_path() {
        assert_eq!(
            parse_spec("kimi-k3=/tmp/c.py"),
            Ok(Spec {
                arm: "kimi-k3".to_string(),
                path: "/tmp/c.py".to_string(),
            })
        );
        assert!(parse_spec("/tmp/c.py").is_err());
        assert!(parse_spec("=c.py").is_err());
        assert!(parse_spec("arm=").is_err());
        // Paths may contain `=`: only the first splits.
        assert_eq!(parse_spec("a=b=c").unwrap().path, "b=c");
    }

    /// An empty wave is an empty report, not an error.
    #[test]
    fn empty_wave_reports_zero() {
        assert_eq!(report(&[]), "0 candidates, 0 approaches");
    }

    /// A single candidate is one approach, never collapsed.
    #[test]
    fn single_candidate_is_one_approach() {
        let report = report(&[("a".to_string(), "import torch\n".to_string())]);
        assert!(report.contains("1 candidate, 1 approach"), "{report}");
        assert!(!report.contains("COLLAPSED"), "{report}");
    }

    /// Renames collapse end to end through the command layer.
    #[test]
    fn renames_collapse() {
        let report = report(&[
            (
                "a".to_string(),
                "import torch\ndef f(x):\n    return x\n".to_string(),
            ),
            (
                "b".to_string(),
                "import torch\ndef f(y):\n    return y\n".to_string(),
            ),
        ]);
        assert!(report.contains("2 candidates, 1 approach"), "{report}");
        assert!(report.contains("COLLAPSED"), "{report}");
        assert!(report.contains('a') && report.contains('b'), "{report}");
    }

    /// A missing file is exit 2, naming the file.
    #[test]
    fn missing_file_exits_2() {
        let gone = std::env::temp_dir().join(format!("fb-diversity-gone-{}", std::process::id()));
        let mut out = Vec::new();
        let code = run(&[format!("a={}", gone.display())], &mut out);
        assert_eq!(code, 2);
        assert!(String::from_utf8_lossy(&out).contains(&gone.to_string_lossy().into_owned()));
    }
}

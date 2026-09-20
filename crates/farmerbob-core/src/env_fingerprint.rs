//! Environment fingerprints for benchmark results.
//!
//! Upstream reference timings are not portable across machines, and not even
//! across driver or torch upgrades on the SAME machine. Every measured result
//! therefore carries the exact environment it was measured in, and a result is
//! only comparable against a baseline with an identical fingerprint.
//!
//! A fingerprint is a `;`-separated `key=value` string, e.g.
//! `torch=2.14.0+cu130;cuda=13.0;driver=610.57.04;gpu=RTX5090;arch=sm_120;python=3.14.7`.
//! Comparison is exact string equality on the parsed fields: results from
//! different fingerprints must never be compared, and scoring against a stale
//! baseline is refused rather than silently averaged.
//!
//! The module is pure: gathering the values is the caller's job (a Python
//! probe for torch/CUDA, `nvidia-smi` for the driver); this module parses,
//! renders, and compares.

/// The exact environment a benchmark result was measured in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFingerprint {
    /// `torch.__version__`, e.g. `2.14.0+cu130`.
    pub torch_version: String,
    /// `torch.version.cuda`, e.g. `13.0`.
    pub cuda_version: String,
    /// NVIDIA driver version from `nvidia-smi`, e.g. `610.57.04`.
    pub driver_version: String,
    /// GPU marketing name, spaces removed, e.g. `RTX5090`.
    pub gpu_name: String,
    /// GPU architecture, e.g. `sm_120`.
    pub arch: String,
    /// Python version that ran the benchmark, e.g. `3.14.7`.
    pub python_version: String,
}

impl EnvFingerprint {
    /// Render the canonical fingerprint string.
    pub fn display(&self) -> String {
        format!(
            "torch={};cuda={};driver={};gpu={};arch={};python={}",
            self.torch_version,
            self.cuda_version,
            self.driver_version,
            self.gpu_name,
            self.arch,
            self.python_version
        )
    }

    /// Parse a fingerprint string. Every field is required and must be
    /// non-empty; unknown fields are rejected so a typo cannot silently
    /// narrow what is compared.
    pub fn parse(s: &str) -> Result<EnvFingerprint, String> {
        let mut torch_version = None;
        let mut cuda_version = None;
        let mut driver_version = None;
        let mut gpu_name = None;
        let mut arch = None;
        let mut python_version = None;
        if s.trim().is_empty() {
            return Err("empty fingerprint".to_string());
        }
        for part in s.split(';') {
            let Some((key, value)) = part.split_once('=') else {
                return Err(format!("invalid fingerprint field: {part:?}"));
            };
            let (key, value) = (key.trim(), value.trim());
            if value.is_empty() {
                return Err(format!("empty value for fingerprint field {key:?}"));
            }
            match key {
                "torch" => torch_version = Some(value.to_string()),
                "cuda" => cuda_version = Some(value.to_string()),
                "driver" => driver_version = Some(value.to_string()),
                "gpu" => gpu_name = Some(value.to_string()),
                "arch" => arch = Some(value.to_string()),
                "python" => python_version = Some(value.to_string()),
                _ => return Err(format!("unknown fingerprint field {key:?}")),
            }
        }
        Ok(EnvFingerprint {
            torch_version: torch_version.ok_or_else(|| "missing torch field".to_string())?,
            cuda_version: cuda_version.ok_or_else(|| "missing cuda field".to_string())?,
            driver_version: driver_version.ok_or_else(|| "missing driver field".to_string())?,
            gpu_name: gpu_name.ok_or_else(|| "missing gpu field".to_string())?,
            arch: arch.ok_or_else(|| "missing arch field".to_string())?,
            python_version: python_version.ok_or_else(|| "missing python field".to_string())?,
        })
    }

    /// True when every field matches exactly.
    pub fn matches(&self, other: &EnvFingerprint) -> bool {
        self == other
    }

    /// Check a result's fingerprint against the baseline's. `Ok(())` when
    /// they match; `Err` names the fields that differ, so the operator knows
    /// whether to re-baseline (driver upgrade) or stop comparing (new GPU).
    pub fn check_against(&self, baseline: &EnvFingerprint) -> Result<(), String> {
        if self == baseline {
            return Ok(());
        }
        let mut diverged = Vec::new();
        if self.torch_version != baseline.torch_version {
            diverged.push(format!(
                "torch {} != {}",
                self.torch_version, baseline.torch_version
            ));
        }
        if self.cuda_version != baseline.cuda_version {
            diverged.push(format!(
                "cuda {} != {}",
                self.cuda_version, baseline.cuda_version
            ));
        }
        if self.driver_version != baseline.driver_version {
            diverged.push(format!(
                "driver {} != {}",
                self.driver_version, baseline.driver_version
            ));
        }
        if self.gpu_name != baseline.gpu_name {
            diverged.push(format!("gpu {} != {}", self.gpu_name, baseline.gpu_name));
        }
        if self.arch != baseline.arch {
            diverged.push(format!("arch {} != {}", self.arch, baseline.arch));
        }
        if self.python_version != baseline.python_version {
            diverged.push(format!(
                "python {} != {}",
                self.python_version, baseline.python_version
            ));
        }
        Err(format!("stale baseline: {}", diverged.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp() -> EnvFingerprint {
        EnvFingerprint {
            torch_version: "2.14.0+cu130".to_string(),
            cuda_version: "13.0".to_string(),
            driver_version: "610.57.04".to_string(),
            gpu_name: "RTX5090".to_string(),
            arch: "sm_120".to_string(),
            python_version: "3.14.7".to_string(),
        }
    }

    #[test]
    fn round_trips_through_display() {
        let back = EnvFingerprint::parse(&fp().display()).expect("parse own display");
        assert_eq!(back, fp());
    }

    #[test]
    fn identical_fingerprints_match_and_check_passes() {
        let a = fp();
        let b = EnvFingerprint::parse(&a.display()).unwrap();
        assert!(a.matches(&b));
        assert_eq!(a.check_against(&b), Ok(()));
    }

    #[test]
    fn every_field_is_compared_and_named() {
        let base = fp();
        let mut changed = fp();
        changed.driver_version = "999.0".to_string();
        assert!(!changed.matches(&base));
        let err = changed.check_against(&base).expect_err("must be stale");
        assert!(err.contains("driver"), "{err}");
        assert!(err.contains("stale"), "{err}");
    }

    #[test]
    fn missing_field_is_rejected() {
        assert!(EnvFingerprint::parse("torch=2.14.0;cuda=13.0").is_err());
    }

    #[test]
    fn unknown_field_is_rejected_not_ignored() {
        let s = fp().display() + ";extra=1";
        let err = EnvFingerprint::parse(&s).expect_err("unknown field must fail");
        assert!(err.contains("unknown"), "{err}");
    }

    #[test]
    fn empty_value_is_rejected() {
        assert!(EnvFingerprint::parse("torch=;cuda=13.0;driver=d;gpu=g;arch=a;python=p").is_err());
        assert!(EnvFingerprint::parse("").is_err());
    }

    #[test]
    fn all_diverged_fields_are_listed() {
        let mut changed = fp();
        changed.torch_version = "x".to_string();
        changed.arch = "y".to_string();
        let err = changed.check_against(&fp()).expect_err("must be stale");
        assert!(err.contains("torch"), "{err}");
        assert!(err.contains("arch"), "{err}");
    }
}

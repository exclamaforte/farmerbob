//! Pure pipeline stage skip, execution, and completion decisions.
//!
//! Before running each stage in `fb-pipeline.sh`, the orchestrator decides
//! whether to skip the stage or run it, based on signatures computed over the
//! candidates (either on-disk or only those passing the gate) and the presence
//! of an artefact on disk.
//!
//! After a stage runs, this module classifies its outcome from the process exit
//! code and artefact presence, and aggregates outcomes across the entire
//! pipeline into an overall verdict.
//!
//! This module is entirely pure and performs no filesystem or process I/O.

/// What a stage's signature is computed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyed {
    /// Every candidate that has a worktree, whether or not it passed the gate.
    OnDisk,
    /// Only the candidates that passed the gate.
    Passing,
}

/// One stage of the pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Stage {
    /// The stage name, as passed by the caller.
    pub name: String,
    /// What the stage's signature is keyed on.
    pub keyed: Keyed,
}

/// The state of a stage's artefact before the stage runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Artefact {
    /// True when the artefact file exists and is non-empty.
    pub present: bool,
    /// The signature stored beside the artefact, if any.
    pub stored_signature: Option<String>,
}

/// What the pipeline should do with a stage.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    /// Signature matches and the artefact is present.
    Skip,
    /// Run it, and afterwards store this signature.
    Run {
        /// The signature to store beside the artefact after completion.
        store: String,
    },
}

/// Decide whether a stage runs.
///
/// `on_disk` and `passing` are the two signatures the caller computed; this function
/// selects between them per `stage.keyed` and never computes either.
pub fn action(stage: &Stage, a: &Artefact, on_disk: &str, passing: &str) -> Action {
    let sig = match stage.keyed {
        Keyed::OnDisk => on_disk,
        Keyed::Passing => passing,
    };
    if a.present && a.stored_signature.as_deref() == Some(sig) {
        Action::Skip
    } else {
        Action::Run {
            store: sig.to_string(),
        }
    }
}

/// What a stage's run produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Produced {
    /// Exit zero and a non-empty artefact.
    Ok,
    /// Non-zero exit. Carries the code.
    Exit(i32),
    /// Exit zero and a missing or empty artefact. The failure that looks like success.
    Nothing,
}

/// Classify one completed stage run.
pub fn produced(rc: i32, artefact_present: bool) -> Produced {
    if rc != 0 {
        Produced::Exit(rc)
    } else if artefact_present {
        Produced::Ok
    } else {
        Produced::Nothing
    }
}

/// The pipeline's own verdict over every stage it ran.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Pipeline {
    /// Every stage produced something.
    Ready,
    /// At least one did not. Carries the failing stage names, in the order they ran.
    NotReady(Vec<String>),
}

/// Compose the whole pipeline's verdict.
pub fn pipeline(results: &[(String, Produced)]) -> Pipeline {
    let failed: Vec<String> = results
        .iter()
        .filter_map(|(name, prod)| match prod {
            Produced::Ok => None,
            Produced::Exit(_) | Produced::Nothing => Some(name.clone()),
        })
        .collect();

    if failed.is_empty() {
        Pipeline::Ready
    } else {
        Pipeline::NotReady(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clause_1_action_on_disk_ignores_passing() {
        let stage = Stage {
            name: "differential".to_string(),
            keyed: Keyed::OnDisk,
        };
        let on_disk_sig = "sig_disk_aaa";
        let passing_sig = "sig_pass_bbb";

        // Stored matches on_disk, passing differs. Should Skip.
        let a_match = Artefact {
            present: true,
            stored_signature: Some(on_disk_sig.to_string()),
        };
        assert_eq!(
            action(&stage, &a_match, on_disk_sig, passing_sig),
            Action::Skip
        );

        // Stored matches passing, on_disk differs. Should Run carrying on_disk.
        let a_mismatch = Artefact {
            present: true,
            stored_signature: Some(passing_sig.to_string()),
        };
        assert_eq!(
            action(&stage, &a_mismatch, on_disk_sig, passing_sig),
            Action::Run {
                store: on_disk_sig.to_string()
            }
        );
    }

    #[test]
    fn test_clause_2_action_passing_ignores_on_disk() {
        let stage = Stage {
            name: "crossx".to_string(),
            keyed: Keyed::Passing,
        };
        let on_disk_sig = "sig_disk_111";
        let passing_sig = "sig_pass_222";

        // Stored matches passing, on_disk differs. Should Skip.
        let a_match = Artefact {
            present: true,
            stored_signature: Some(passing_sig.to_string()),
        };
        assert_eq!(
            action(&stage, &a_match, on_disk_sig, passing_sig),
            Action::Skip
        );

        // Stored matches on_disk, passing differs. Should Run carrying passing.
        let a_mismatch = Artefact {
            present: true,
            stored_signature: Some(on_disk_sig.to_string()),
        };
        assert_eq!(
            action(&stage, &a_mismatch, on_disk_sig, passing_sig),
            Action::Run {
                store: passing_sig.to_string()
            }
        );
    }

    #[test]
    fn test_clause_3_skip_requires_both_matching_sig_and_present_artefact() {
        let stage_disk = Stage {
            name: "score".to_string(),
            keyed: Keyed::OnDisk,
        };
        let sig_disk = "sig_score";
        let a_missing_disk = Artefact {
            present: false,
            stored_signature: Some(sig_disk.to_string()),
        };
        assert_eq!(
            action(&stage_disk, &a_missing_disk, sig_disk, "other"),
            Action::Run {
                store: sig_disk.to_string()
            }
        );

        let stage_pass = Stage {
            name: "promote".to_string(),
            keyed: Keyed::Passing,
        };
        let sig_pass = "sig_promote";
        let a_missing_pass = Artefact {
            present: false,
            stored_signature: Some(sig_pass.to_string()),
        };
        assert_eq!(
            action(&stage_pass, &a_missing_pass, "other", sig_pass),
            Action::Run {
                store: sig_pass.to_string()
            }
        );
    }

    #[test]
    fn test_clause_4_none_stored_signature_always_runs() {
        let stage = Stage {
            name: "prove".to_string(),
            keyed: Keyed::Passing,
        };
        let on_disk_sig = "disk";
        let passing_sig = "passing";

        let a_present = Artefact {
            present: true,
            stored_signature: None,
        };
        assert_eq!(
            action(&stage, &a_present, on_disk_sig, passing_sig),
            Action::Run {
                store: passing_sig.to_string()
            }
        );

        let a_absent = Artefact {
            present: false,
            stored_signature: None,
        };
        assert_eq!(
            action(&stage, &a_absent, on_disk_sig, passing_sig),
            Action::Run {
                store: passing_sig.to_string()
            }
        );
    }

    #[test]
    fn test_clause_5_run_carries_keyed_signature_round_trip() {
        // Round trip with Keyed::OnDisk
        let stage_disk = Stage {
            name: "score".to_string(),
            keyed: Keyed::OnDisk,
        };
        let on_disk_sig = "disk_fingerprint_v1";
        let passing_sig = "pass_fingerprint_v1";
        let initial_a_disk = Artefact {
            present: false,
            stored_signature: None,
        };

        let act_disk = action(&stage_disk, &initial_a_disk, on_disk_sig, passing_sig);
        assert_eq!(
            act_disk,
            Action::Run {
                store: on_disk_sig.to_string()
            }
        );
        let next_a_disk = Artefact {
            present: true,
            stored_signature: Some(on_disk_sig.to_string()),
        };
        assert_eq!(
            action(&stage_disk, &next_a_disk, on_disk_sig, passing_sig),
            Action::Skip
        );

        // Round trip with Keyed::Passing
        let stage_pass = Stage {
            name: "critique".to_string(),
            keyed: Keyed::Passing,
        };
        let initial_a_pass = Artefact {
            present: false,
            stored_signature: None,
        };

        let act_pass = action(&stage_pass, &initial_a_pass, on_disk_sig, passing_sig);
        assert_eq!(
            act_pass,
            Action::Run {
                store: passing_sig.to_string()
            }
        );
        let next_a_pass = Artefact {
            present: true,
            stored_signature: Some(passing_sig.to_string()),
        };
        assert_eq!(
            action(&stage_pass, &next_a_pass, on_disk_sig, passing_sig),
            Action::Skip
        );
    }

    #[test]
    fn test_pipeline_preserves_duplicate_failed_stage_names_without_dedup() {
        let results = vec![
            ("dup_stage".to_string(), Produced::Exit(1)),
            ("dup_stage".to_string(), Produced::Nothing),
        ];
        assert_eq!(
            pipeline(&results),
            Pipeline::NotReady(vec!["dup_stage".to_string(), "dup_stage".to_string()])
        );
    }

    #[test]
    fn test_clause_6_produced_exit_zero_present_is_ok() {
        assert_eq!(produced(0, true), Produced::Ok);
    }

    #[test]
    fn test_clause_7_produced_exit_zero_absent_is_nothing() {
        assert_eq!(produced(0, false), Produced::Nothing);
        assert_ne!(produced(0, false), Produced::Ok);
    }

    #[test]
    fn test_clause_8_produced_nonzero_rc_is_exit_regardless_of_artefact() {
        assert_eq!(produced(1, true), Produced::Exit(1));
        assert_eq!(produced(1, false), Produced::Exit(1));
        assert_eq!(produced(127, true), Produced::Exit(127));
        assert_eq!(produced(127, false), Produced::Exit(127));
        assert_eq!(produced(-9, true), Produced::Exit(-9));
        assert_eq!(produced(-9, false), Produced::Exit(-9));
    }

    #[test]
    fn test_clause_9_nothing_and_ok_differ_and_single_nothing_makes_pipeline_not_ready() {
        assert_ne!(Produced::Nothing, Produced::Ok);

        let results = vec![("stage_zero_empty".to_string(), Produced::Nothing)];
        assert_eq!(
            pipeline(&results),
            Pipeline::NotReady(vec!["stage_zero_empty".to_string()])
        );
    }

    #[test]
    fn test_clause_10_pipeline_aggregates_non_ok_in_order() {
        // Three stages where first and last fail, middle succeeds
        let results = vec![
            ("stage_1".to_string(), Produced::Exit(2)),
            ("stage_2".to_string(), Produced::Ok),
            ("stage_3".to_string(), Produced::Nothing),
        ];
        assert_eq!(
            pipeline(&results),
            Pipeline::NotReady(vec!["stage_1".to_string(), "stage_3".to_string()])
        );
    }

    #[test]
    fn test_boundary_single_stage_ok_is_ready() {
        let results = vec![("single".to_string(), Produced::Ok)];
        assert_eq!(pipeline(&results), Pipeline::Ready);
    }

    #[test]
    fn test_boundary_single_stage_exit_is_not_ready() {
        let results = vec![("single".to_string(), Produced::Exit(1))];
        assert_eq!(
            pipeline(&results),
            Pipeline::NotReady(vec!["single".to_string()])
        );
    }

    #[test]
    fn test_boundary_all_stages_failing_preserves_order() {
        let results = vec![
            ("alpha".to_string(), Produced::Exit(1)),
            ("beta".to_string(), Produced::Nothing),
            ("gamma".to_string(), Produced::Exit(42)),
        ];
        assert_eq!(
            pipeline(&results),
            Pipeline::NotReady(vec![
                "alpha".to_string(),
                "beta".to_string(),
                "gamma".to_string(),
            ])
        );
    }

    #[test]
    fn test_boundary_all_stages_ok_is_ready() {
        let results = vec![
            ("stage_a".to_string(), Produced::Ok),
            ("stage_b".to_string(), Produced::Ok),
        ];
        assert_eq!(pipeline(&results), Pipeline::Ready);
    }
}

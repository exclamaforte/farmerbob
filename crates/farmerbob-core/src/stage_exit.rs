//! Pure classification of stage exit codes for pipeline control.

/// What a stage's exit code means for the pipeline running it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageExit {
    /// It did its work. Record the signature and continue.
    Done,
    /// It could not apply, and nothing went wrong. Continue without recording a signature.
    NotApplicable,
    /// It failed. Carries the raw code so a caller can report it.
    Failed(i32),
}

/// One stage's exit-code contract: the codes that mean [`StageExit::NotApplicable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    /// The stage's name, for reporting.
    pub stage: String,
    /// Codes meaning that the stage could not apply.
    pub not_applicable: Vec<i32>,
}

/// Classify one exit code against its stage's contract.
pub fn classify(c: &Contract, code: i32) -> StageExit {
    if code == 0 {
        StageExit::Done
    } else if c.not_applicable.contains(&code) {
        StageExit::NotApplicable
    } else {
        StageExit::Failed(code)
    }
}

/// Whether the pipeline should continue after this outcome.
pub fn proceeds(e: &StageExit) -> bool {
    !matches!(e, StageExit::Failed(_))
}

/// Whether this outcome should have its freshness signature recorded.
pub fn caches(e: &StageExit) -> bool {
    matches!(e, StageExit::Done)
}

#[cfg(test)]
mod tests {
    use super::{Contract, StageExit, caches, classify, proceeds};

    fn contract(not_applicable: Vec<i32>) -> Contract {
        Contract {
            stage: String::from("test-stage"),
            not_applicable,
        }
    }

    #[test]
    fn classify_pins_zero_membership_and_duplicate_codes() {
        let c = contract(vec![0, 7, 7]);

        assert_eq!(classify(&c, 0), StageExit::Done);
        assert_eq!(classify(&c, 7), StageExit::NotApplicable);
        assert_eq!(classify(&contract(Vec::new()), 7), StageExit::Failed(7));
    }

    #[test]
    fn classify_preserves_unlisted_nonzero_codes() {
        let c = contract(Vec::new());

        assert_eq!(classify(&c, -9), StageExit::Failed(-9));
        assert_eq!(classify(&c, i32::MIN), StageExit::Failed(i32::MIN));
        assert_eq!(classify(&c, i32::MAX), StageExit::Failed(i32::MAX));
    }

    #[test]
    fn stage_name_does_not_affect_classification() {
        let first = Contract {
            stage: String::from("first"),
            not_applicable: vec![4],
        };
        let second = Contract {
            stage: String::from("second"),
            not_applicable: vec![4],
        };

        for code in [0, 4, -9, i32::MIN, i32::MAX] {
            assert_eq!(classify(&first, code), classify(&second, code));
        }
    }

    #[test]
    fn proceeds_and_caches_answer_different_questions() {
        assert!(proceeds(&StageExit::Done));
        assert!(proceeds(&StageExit::NotApplicable));
        assert!(!proceeds(&StageExit::Failed(1)));

        assert!(caches(&StageExit::Done));
        assert!(!caches(&StageExit::NotApplicable));
        assert!(!caches(&StageExit::Failed(1)));
    }
}

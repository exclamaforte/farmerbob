//! Grades: scored evaluations of a run's output.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::RunId;

/// Rubric scores for one graded run. Every dimension is scored 1..=5.
///
/// The range is enforced by [`RubricScores::try_new`] and by deserialization;
/// values built directly as struct literals should be checked with
/// [`RubricScores::is_valid`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawRubricScores")]
pub struct RubricScores {
    /// How well the final artifact works.
    pub quality: u8,
    /// How sound the chosen method was.
    pub approach: u8,
    /// How closely the work followed the task's instructions.
    pub adherence: u8,
    /// How well the run coped with unhandled obstacles.
    pub autonomy: u8,
    /// How truthfully the agent reported what it did.
    pub honesty: u8,
}

/// Deserialization target so out-of-range JSON is rejected, not trusted.
#[derive(Deserialize)]
struct RawRubricScores {
    quality: u8,
    approach: u8,
    adherence: u8,
    autonomy: u8,
    honesty: u8,
}

impl TryFrom<RawRubricScores> for RubricScores {
    type Error = DomainError;

    fn try_from(raw: RawRubricScores) -> Result<Self, DomainError> {
        Self::try_new(
            raw.quality,
            raw.approach,
            raw.adherence,
            raw.autonomy,
            raw.honesty,
        )
    }
}

impl RubricScores {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 5;

    /// Validates and builds a set of scores; every dimension must be 1..=5.
    pub fn try_new(
        quality: u8,
        approach: u8,
        adherence: u8,
        autonomy: u8,
        honesty: u8,
    ) -> Result<Self, DomainError> {
        for (field, value) in [
            ("quality", quality),
            ("approach", approach),
            ("adherence", adherence),
            ("autonomy", autonomy),
            ("honesty", honesty),
        ] {
            if !(Self::MIN..=Self::MAX).contains(&value) {
                return Err(DomainError::ScoreOutOfRange { field, value });
            }
        }
        Ok(Self {
            quality,
            approach,
            adherence,
            autonomy,
            honesty,
        })
    }

    /// True when every dimension is 1..=5. Only relevant for scores built
    /// directly as struct literals; [`RubricScores::try_new`] and
    /// deserialization already enforce the range.
    pub fn is_valid(&self) -> bool {
        [
            self.quality,
            self.approach,
            self.adherence,
            self.autonomy,
            self.honesty,
        ]
        .iter()
        .all(|s| (Self::MIN..=Self::MAX).contains(s))
    }
}

/// A grader's scored evaluation of one run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Grade {
    pub run_id: RunId,
    /// Which grader produced this grade, e.g. `"gpt-judge-v2"`.
    pub grader: String,
    pub scores: RubricScores,
    /// The grader's justification for the scores.
    pub rationale: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn try_new_accepts_in_range_and_rejects_out_of_range() {
        let ok = RubricScores::try_new(1, 5, 3, 2, 4).unwrap();
        assert!(ok.is_valid());

        let zero = RubricScores::try_new(0, 3, 3, 3, 3).unwrap_err();
        assert!(zero.to_string().contains("quality"), "{zero}");

        let six = RubricScores::try_new(3, 3, 3, 3, 6).unwrap_err();
        assert!(six.to_string().contains("honesty"), "{six}");
    }

    #[test]
    fn deserialization_rejects_out_of_range_scores() {
        let json = r#"{"quality": 5, "approach": 5, "adherence": 9, "autonomy": 1, "honesty": 1}"#;
        assert!(serde_json::from_str::<RubricScores>(json).is_err());
    }

    #[test]
    fn grade_round_trips_through_json() {
        let grade = Grade {
            run_id: RunId::new(),
            grader: String::from("judge-v2"),
            scores: RubricScores::try_new(4, 5, 3, 4, 5).unwrap(),
            rationale: String::from("Solid implementation; minor deviations from the spec."),
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        };

        let back: Grade = serde_json::from_str(&serde_json::to_string(&grade).unwrap()).unwrap();
        assert_eq!(grade, back);
    }
}

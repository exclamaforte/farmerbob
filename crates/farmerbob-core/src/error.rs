//! Validation errors raised by domain constructors.

use thiserror::Error;

/// Errors from validating domain values at construction time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DomainError {
    /// A rubric score fell outside the legal 1..=5 range.
    #[error("rubric score `{field}` must be in 1..=5, got {value}")]
    ScoreOutOfRange { field: &'static str, value: u8 },
}

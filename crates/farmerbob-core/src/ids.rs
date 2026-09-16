//! Newtype identifiers.
//!
//! Each id type wraps a UUID but is deliberately not interchangeable with the
//! others: a [`RunId`] will not unify with a [`TaskId`], so the compiler
//! catches swapped arguments and mismatched joins.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generates a fresh id from a random v4 UUID.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID.
            pub fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// The underlying UUID.
            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

define_id! {
    /// Identifies an [`Agent`](crate::Agent) configuration.
    AgentId
}

define_id! {
    /// Identifies an [`Experiment`](crate::Experiment).
    ExperimentId
}

define_id! {
    /// Identifies a [`Lease`](crate::Lease).
    LeaseId
}

define_id! {
    /// Identifies a [`Run`](crate::Run).
    RunId
}

define_id! {
    /// Identifies a [`Task`](crate::Task).
    TaskId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_of_different_types_are_not_interchangeable() {
        let run = RunId::new();
        let task = TaskId::from_uuid(run.as_uuid());

        // Same underlying UUID, but the newtypes refuse to unify, so this
        // only compiles through the explicit accessors.
        assert_eq!(run.as_uuid(), task.as_uuid());
        assert_ne!(RunId::new(), RunId::new());
    }

    #[test]
    fn id_serializes_as_its_uuid() {
        let id = AgentId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: AgentId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}

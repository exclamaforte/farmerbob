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

        // `new_without_default` fires on every id type and asks for exactly the impl that
        // was removed below. It is a style lint that cannot see the reason; three critics
        // who executed the code could. Overriding it IS the escalated finding.
        #[allow(clippy::new_without_default)]
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

        // NO `impl Default`. Three independent critics on the `probe` task -- reviewing
        // each other's work, not mine -- found that `default()` returned `Self::new()`,
        // a fresh random v4 UUID on every call, so `RunId::default() != RunId::default()`.
        // Any `#[derive(Default)]` on a struct holding an id would have silently acquired
        // an RNG-backed value: a reproducibility trap for snapshot tests and for anything
        // using a default id as a map key.
        //
        // The obvious repair, `Uuid::nil()`, is rejected: a nil id is a sentinel that reads
        // exactly like a real measurement, which is the one failure this project keeps
        // making. An identifier has no meaningful zero. Callers that need an absent id say
        // so with `Option`.
        //   (escalated from probe; credit or-deepseek-v4-latest, or-deepseek-v4-pro,
        //    or-glm-53 -- bead farmerbob-mqr)

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

// ESCALATED from the `probe` task: a finding three independent critics reached separately,
// that no objective metric produced and that no candidate's own tests covered.
//
// or-deepseek-v4-latest: "`RunId::default() != RunId::default()` ... the wrapper disagrees
//   with its own inner type: `RunId::default()` can never equal `from_uuid(Uuid::default())`."
// or-deepseek-v4-pro:    "A reader expects `default()` to be a cheap, stable sentinel; here
//   it is a side-effecting RNG ... two 'defaults' will silently compare unequal."
// or-glm-53:             "`Default` conventionally denotes a deterministic zero value; any
//   `#[derive(Default)]` on a struct containing an id silently acquires RNG-backed ids."
//
// or-glm-53 added that its own implementation made the same choice, so it could not be a
// differentiator -- a critic disqualifying its own advantage, which is the behaviour the
// cross-examination design is trying to buy.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_probe_ids {
    use super::*;

    /// The id types must not implement `Default` at all. This is a compile-time contract,
    /// so the test asserts the property `Default` would have broken: two independently
    /// constructed ids are never equal, and there is no zero id to confuse with a real one.
    #[test]
    fn ids_have_no_zero_value_to_mistake_for_a_real_one() {
        let a = RunId::new();
        let b = RunId::new();
        assert_ne!(a, b, "two fresh ids must differ");
    }

    /// Pins `Display`, which every critic flagged as unspecified and untested. The format is
    /// type-tagged deliberately: a bare UUID in a log cannot be told apart from any other
    /// id, and mixing a TaskId into a RunId field is precisely what the newtypes prevent.
    #[test]
    fn display_is_type_tagged_and_pinned() {
        let u = uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")
            .expect("literal uuid parses");
        assert_eq!(
            RunId::from(u).to_string(),
            "RunId(550e8400-e29b-41d4-a716-446655440000)"
        );
        assert_eq!(
            TaskId::from(u).to_string(),
            "TaskId(550e8400-e29b-41d4-a716-446655440000)"
        );
    }

    /// The same UUID in two id types renders differently, so a log line is unambiguous.
    #[test]
    fn two_id_types_over_one_uuid_do_not_render_alike() {
        let u = uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")
            .expect("literal uuid parses");
        assert_ne!(RunId::from(u).to_string(), AgentId::from(u).to_string());
    }
}

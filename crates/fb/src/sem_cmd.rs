//! Render the semaphore's observed slot states and admission decision.
//!
//! This module is intentionally not wired into the command parser yet. The
//! semaphore module remains the sole owner of census and grant arithmetic.
#![allow(dead_code)]
// The public functions are intentionally unwired; a later task adds the CLI
// subcommand without changing this module's public surface.

use std::io::Write;

use farmerbob_core::semaphore::{self, Grant, Slot};

/// Convert the semaphore's grant decision to the caller's exit code.
fn exit_code(decision: &Grant) -> i32 {
    match decision {
        Grant::Take(_) => 0,
        Grant::Full | Grant::NoSlots => 1,
    }
}

/// Render the slot table and the grant decision.
///
/// One line per slot in input order is followed by one summary line carrying
/// both values returned by [`semaphore::census`] and the grant decision.
pub fn table(slots: &[Slot]) -> String {
    let mut rendered = String::new();

    for slot in slots {
        rendered.push_str(&format!("slot: {slot:?}\n"));
    }

    let census = semaphore::census(slots);
    let decision = semaphore::grant(slots);
    rendered.push_str(&format!(
        "census: {} {}; grant: {decision:?}",
        census.0, census.1
    ));
    rendered
}

/// Render and write. Returns the exit code the caller should use.
pub fn run(slots: &[Slot], out: &mut dyn Write) -> i32 {
    let rendered = table(slots);
    // The fixed API has no write-error result; preserve the contract that the
    // returned code reflects only the semaphore's grant decision.
    let _ = out.write_all(rendered.as_bytes());
    exit_code(&semaphore::grant(slots))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_one_row_per_slot_and_one_summary() {
        let slots = [Slot::Held, Slot::Free, Slot::Unknown];
        let rendered = table(&slots);
        assert_eq!(rendered.lines().count(), slots.len() + 1);
    }

    #[test]
    fn summary_carries_the_module_census_values() {
        let slots = [Slot::Free, Slot::Held, Slot::Unknown, Slot::Free];
        let rendered = table(&slots);
        let census = semaphore::census(&slots);
        assert!(rendered.contains(&census.0.to_string()));
        assert!(rendered.contains(&census.1.to_string()));
    }

    #[test]
    fn run_uses_the_module_grant_for_each_decision_variant() {
        let cases: &[&[Slot]] = &[
            &[],
            &[Slot::Free],
            &[Slot::Held],
            &[Slot::Unknown],
            &[Slot::Held, Slot::Free],
            &[Slot::Free, Slot::Held],
        ];

        for slots in cases {
            let expected = match semaphore::grant(slots) {
                Grant::Take(_) => 0,
                Grant::Full | Grant::NoSlots => 1,
            };
            let mut output = Vec::new();
            assert_eq!(run(slots, &mut output), expected);
            assert_eq!(String::from_utf8_lossy(&output), table(slots));
        }
    }

    #[test]
    fn held_slots_are_still_rendered_when_grant_refuses() {
        let slots = [Slot::Held, Slot::Held];
        let mut output = Vec::new();
        assert_eq!(run(&slots, &mut output), 1);
        assert_eq!(
            String::from_utf8_lossy(&output).lines().count(),
            slots.len() + 1
        );
    }

    #[test]
    fn a_free_slot_is_granted() {
        let slots = [Slot::Held, Slot::Free];
        let mut output = Vec::new();
        assert_eq!(run(&slots, &mut output), 0);
    }

    #[test]
    fn empty_input_still_renders_the_module_summary() {
        let slots: &[Slot] = &[];
        let mut output = Vec::new();
        let expected = match semaphore::grant(slots) {
            Grant::Take(_) => 0,
            Grant::Full | Grant::NoSlots => 1,
        };
        assert_eq!(run(slots, &mut output), expected);
        assert_eq!(String::from_utf8_lossy(&output).lines().count(), 1);
    }
}

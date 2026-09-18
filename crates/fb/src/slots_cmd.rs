//! `fb slots`: report how many concurrent runs fit on this machine, and why.
//!
//! This is the caller `SlotTable::admit` never had. `fb-admit.sh` sizes the machine
//! with two lines of awk that count slots against the MEASURED per-run usage (1.5G)
//! while giving every run a HARD cap of 2G (MemoryMax) -- nine slots of expectation,
//! eighteen gigabytes of permission, and no bound anywhere on the sum of what the
//! machine may actually be asked for (farmerbob-89j). Here the hard cap IS the slot
//! budget, so every admission is charged what its run may really consume and the sum
//! is bounded by construction.

use farmerbob_core::slots::{Admission, Budget, Machine, RunRef, SlotTable};

/// How many admissions may be simulated one at a time.
///
/// Each simulated admission allocates a run name and a table entry, and `admit`
/// scans the table for a free slot, so simulating n admissions is O(n^2). The cap
/// keeps the worst case fast; past it, the count comes from `SlotTable::capacity`
/// (see [`admit_until_queued`]), which agrees with the admissions for every ceiling
/// below `u64::MAX`.
const MAX_SIMULATED_ADMISSIONS: usize = 10_000;

/// What a capacity question produced.
///
/// The two variants exist so that "not attempted" is distinguishable from an empty
/// sequence of admissions: [`Planning::Refused`] means no admission was attempted,
/// while [`Planning::Attempted`] always carries at least one, because a positive
/// budget always attempts the first probe.
#[derive(Debug, Clone, PartialEq)]
enum Planning {
    /// `memory_mb` is zero. A slot that costs nothing always fits, so admitting
    /// until one is queued would never terminate; the question is refused rather
    /// than answered with an infinite capacity.
    Refused,
    /// The admissions `SlotTable` returned, in admission order (probe 0 first). A
    /// ceiling of zero yields a single `Queued` entry -- the first probe is
    /// attempted and refused, which is where its reason comes from. `capacity` is
    /// the number of `Admitted` entries, except where the simulation was cut short
    /// (see [`admit_until_queued`]), when it is the more permissive of that count
    /// and `SlotTable::capacity`'s figure.
    Attempted {
        /// The admissions, in the order they were attempted.
        admissions: Vec<Admission>,
        /// How many concurrent runs fit.
        capacity: usize,
    },
}

/// Ask [`SlotTable`] what fits, by admitting probe runs until one is `Queued`.
///
/// The count is NOT `(available - headroom) / memory`. That would be a second copy
/// of the arithmetic `admit` performs, and the two would drift -- which is exactly
/// how `fb-admit.sh` came to size slots against one per-run figure while capping
/// runs at another. Charging every probe its full budget makes the count and the
/// table agree by construction: the sum of admitted budgets is bounded by the
/// ceiling whatever the answer turns out to be.
///
/// The simulation is bounded. `SlotTable::capacity` gives the table's own count of
/// slots; one probe beyond that must be queued, since a partial slot is not a slot,
/// and observing it is what makes this "until one is Queued" rather than a division.
/// Past [`MAX_SIMULATED_ADMISSIONS`] the probes stop and the table's `capacity`
/// figure is reported instead, so every input -- `u64::MAX` included -- terminates.
/// The one place the two figures can disagree is a ceiling of exactly `u64::MAX`:
/// there `committed + requested` saturates to the ceiling itself, `admit` never
/// queues, and "until one is Queued" would not terminate. The count reported then
/// is the more permissive of the admissions actually granted and the table's
/// `capacity` -- the closest honest answer to a question the table answers with
/// "infinite".
fn admit_until_queued(machine: &Machine, budget: Budget) -> Planning {
    if budget.memory_mb == 0 {
        return Planning::Refused;
    }
    let mut table = SlotTable::new(budget);
    let bound = table.capacity(machine);
    let limit = bound.saturating_add(1).min(MAX_SIMULATED_ADMISSIONS);
    let mut admissions = Vec::new();
    let mut queued = false;
    for i in 0..limit {
        let probe = RunRef(format!("probe-{i}"));
        match table.admit(probe, machine) {
            a @ Admission::Admitted(_) => admissions.push(a),
            q @ Admission::Queued { .. } => {
                admissions.push(q);
                queued = true;
                break;
            }
        }
    }
    let admitted = admissions.len() - usize::from(queued);
    let capacity = if queued {
        admitted
    } else {
        admitted.max(bound)
    };
    Planning::Attempted {
        admissions,
        capacity,
    }
}

/// The reason `SlotTable` gave for refusing a further run, verbatim.
fn queued_reason(admissions: &[Admission]) -> Option<&str> {
    admissions.iter().find_map(|a| match a {
        Admission::Queued { reason } => Some(reason.as_str()),
        Admission::Admitted(_) => None,
    })
}

/// Report how many concurrent runs fit, and why.
///
/// `plan` prints the capacity line the shell prints. Exit code is 0 whenever the
/// question could be answered, including when the answer is one slot.
///
/// A capacity of ZERO is also an answer, and exits 0. This is a deliberate
/// divergence from `fb-admit.sh`, which clamps to a minimum of one slot
/// (`[ "$SLOTS" -lt 1 ] && SLOTS=1`) and so admits a run the machine cannot hold:
/// a machine with no room should say so, and the caller decides whether to wait.
/// The one question refused rather than answered is `memory_mb == 0` -- a slot that
/// costs nothing would admit forever -- which reports the refusal and exits
/// non-zero instead of looping.
pub fn run_cmd(available_mb: u64, headroom_mb: u64, memory_mb: u64, plan: bool) -> i32 {
    let machine = Machine {
        available_mb,
        headroom_mb,
    };
    // The per-run HARD CAP is the budget, not a measured typical usage: MemoryMax
    // permits every run its full cap whether or not it usually needs it, so an
    // admission charged less than the cap leaves the sum unbounded again.
    let budget = Budget {
        memory_mb,
        cpu_quota_percent: 100,
        tasks_max: 1,
    };
    match admit_until_queued(&machine, budget) {
        Planning::Refused => {
            eprintln!("error: memory_mb is 0 -- a slot that costs nothing would admit forever");
            crate::exit::ERROR
        }
        Planning::Attempted {
            admissions,
            capacity,
        } => {
            let reason = queued_reason(&admissions);
            if plan {
                println!(
                    "admission: {memory_mb}MB/slot (hard cap), {headroom_mb}MB headroom, \
                     {available_mb}MB avail -> {capacity} slots"
                );
            } else {
                println!("slots: {capacity}");
                if let Some(reason) = reason {
                    println!("next: {reason}");
                }
            }
            crate::exit::OK
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_of(available: u64, headroom: u64, memory: u64) -> Planning {
        admit_until_queued(
            &Machine {
                available_mb: available,
                headroom_mb: headroom,
            },
            Budget {
                memory_mb: memory,
                cpu_quota_percent: 100,
                tasks_max: 1,
            },
        )
    }

    fn capacity_of(available: u64, headroom: u64, memory: u64) -> usize {
        match plan_of(available, headroom, memory) {
            Planning::Attempted { capacity, .. } => capacity,
            Planning::Refused => panic!("question should be answerable"),
        }
    }

    /// The reason the reference table produces for the probe this many runs into
    /// the machine, so the reason under test can be compared against the real
    /// `SlotTable` output instead of a copy of its format.
    fn reference_reason(available: u64, headroom: u64, memory: u64, holding: usize) -> String {
        let machine = Machine {
            available_mb: available,
            headroom_mb: headroom,
        };
        let budget = Budget {
            memory_mb: memory,
            cpu_quota_percent: 100,
            tasks_max: 1,
        };
        let mut table = SlotTable::new(budget);
        for i in 0..holding {
            let probe = RunRef(format!("probe-{i}"));
            assert!(matches!(
                table.admit(probe, &machine),
                Admission::Admitted(_)
            ));
        }
        match table.admit(RunRef(format!("probe-{holding}")), &machine) {
            Admission::Queued { reason } => reason,
            Admission::Admitted(_) => panic!("reference should refuse"),
        }
    }

    #[test]
    fn partial_slot_is_not_a_slot() {
        // (17000 - 3000) / 2048 = 6.83: six whole slots, and no rounding up.
        assert_eq!(capacity_of(17_000, 3_000, 2_048), 6);
    }

    #[test]
    fn nothing_fits_when_ceiling_is_below_one_run() {
        // Ceiling 1000MB, one run needs 2048MB: capacity 0, and the reason is the
        // table's own, not a paraphrase.
        match plan_of(4_000, 3_000, 2_048) {
            Planning::Attempted {
                admissions,
                capacity,
            } => {
                assert_eq!(capacity, 0);
                assert_eq!(admissions.len(), 1);
                assert_eq!(
                    queued_reason(&admissions),
                    Some(reference_reason(4_000, 3_000, 2_048, 0).as_str())
                );
            }
            Planning::Refused => panic!("capacity zero is an answer, not a refusal"),
        }
    }

    #[test]
    fn capacity_zero_exits_zero_without_the_shell_clamp() {
        // The shell clamps to one slot; this must not. Zero is an answer.
        assert_eq!(run_cmd(4_000, 3_000, 2_048, false), 0);
        assert_eq!(run_cmd(4_000, 3_000, 2_048, true), 0);
    }

    #[test]
    fn available_equal_to_headroom_leaves_ceiling_zero() {
        match plan_of(3_000, 3_000, 2_048) {
            Planning::Attempted {
                admissions,
                capacity,
            } => {
                assert_eq!(capacity, 0);
                assert_eq!(admissions.len(), 1);
                assert!(matches!(admissions[0], Admission::Queued { .. }));
            }
            Planning::Refused => panic!("question should be answerable"),
        }
    }

    #[test]
    fn headroom_above_available_saturates_instead_of_wrapping() {
        assert_eq!(capacity_of(1_000, 3_000, 2_048), 0);
    }

    #[test]
    fn zero_memory_budget_is_refused_not_looped() {
        // A slot that costs nothing admits forever; the question is refused rather
        // than answered. The exit code is non-zero (1 is this command's choice;
        // the spec fixes only "non-zero").
        assert_eq!(plan_of(17_000, 3_000, 0), Planning::Refused);
        assert_ne!(run_cmd(17_000, 3_000, 0, false), 0);
        assert_ne!(run_cmd(17_000, 3_000, 0, true), 0);
    }

    #[test]
    fn run_fitting_exactly_is_admitted_not_rejected() {
        // Ceiling 10240 = 5 x 2048 exactly: the fifth run lands on the boundary
        // and `admit`'s <= must take it.
        match plan_of(13_240, 3_000, 2_048) {
            Planning::Attempted {
                admissions,
                capacity,
            } => {
                assert_eq!(capacity, 5);
                assert!(matches!(admissions[4], Admission::Admitted(_)));
                assert_eq!(admissions.len(), 6);
            }
            Planning::Refused => panic!("question should be answerable"),
        }
    }

    #[test]
    fn one_mb_short_of_a_slot_drops_the_capacity() {
        // Same machine minus one MB: the fifth run no longer fits, so 4.
        assert_eq!(capacity_of(13_239, 3_000, 2_048), 4);
    }

    #[test]
    fn spec_matrix_example_counts_five() {
        // available 13288, headroom 3000 -> ceiling 10288 -> five slots.
        assert_eq!(capacity_of(13_288, 3_000, 2_048), 5);
    }

    #[test]
    fn capacity_one_is_a_working_machine_distinct_from_zero() {
        // Ceiling 2048 = exactly one slot; one MB less and the machine is idle.
        assert_eq!(capacity_of(5_048, 3_000, 2_048), 1);
        assert_eq!(capacity_of(5_047, 3_000, 2_048), 0);
        assert_eq!(run_cmd(5_048, 3_000, 2_048, false), 0);
    }

    #[test]
    fn seventh_run_carries_the_tables_own_reason() {
        // The refusal of the seventh run reports committed 12288MB: the SUM of six
        // budgets, which is the bound the shell never had. Compared against the
        // table itself so no copy of its wording can drift.
        match plan_of(17_000, 3_000, 2_048) {
            Planning::Attempted {
                admissions,
                capacity,
            } => {
                assert_eq!(capacity, 6);
                assert_eq!(
                    queued_reason(&admissions),
                    Some(reference_reason(17_000, 3_000, 2_048, 6).as_str())
                );
            }
            Planning::Refused => panic!("question should be answerable"),
        }
    }

    #[test]
    fn admissions_come_in_admission_order_with_the_refusal_last() {
        match plan_of(17_000, 3_000, 2_048) {
            Planning::Attempted {
                admissions,
                capacity,
            } => {
                assert_eq!(capacity, 6);
                assert_eq!(admissions.len(), capacity + 1);
                for a in &admissions[..capacity] {
                    assert!(matches!(a, Admission::Admitted(_)));
                }
                assert!(matches!(admissions[capacity], Admission::Queued { .. }));
            }
            Planning::Refused => panic!("question should be answerable"),
        }
    }

    #[test]
    fn u64_extremes_terminate_without_overflow_or_panic() {
        // Ceiling exactly u64::MAX: admit's saturating add never queues, so the
        // count falls back to the table's capacity figure.
        assert_eq!(run_cmd(u64::MAX, 0, u64::MAX, false), 0);
        // Saturating subtraction: ceiling 0, capacity 0.
        assert_eq!(run_cmd(u64::MAX, u64::MAX, u64::MAX, false), 0);
        assert_eq!(capacity_of(u64::MAX, u64::MAX, 1), 0);
        // Astronomical capacity: simulation is capped and the question still
        // answers.
        assert_eq!(run_cmd(u64::MAX, 0, 1, false), 0);
    }
}



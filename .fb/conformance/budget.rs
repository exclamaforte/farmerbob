
// ESCALATED from cross-examination: codex-luna's suite discriminated on budget.
// Not a CLAIM -- cross-examination found it directly. Kept only because it passes
// against the merged winner, which is what separates a discovery from an
// over-fitted suite.
#[cfg(test)]
mod cx_budget_codex_luna {
    use super::{
        Admission, Machine, Policy, admit, committed_mib, policy_is_impossible, reclaimable_mib,
    };

    fn machine(available_mib: u64, tmpfs_mib: u64) -> Machine {
        Machine {
            total_mib: 16_384,
            available_mib,
            tmpfs_mib,
        }
    }

    fn policy(per_slot_mib: u64, headroom_mib: u64, max_slots: usize) -> Policy {
        Policy {
            per_slot_mib,
            headroom_mib,
            max_slots,
        }
    }

    #[test]
    fn full_tmpfs_produces_deny_that_mentions_reclaimable_memory() {
        let result = admit(&machine(512, 4_096), &policy(1_024, 1_024, 4), 0);
        match result {
            Admission::Deny { reason } => {
                assert!(reason.contains("shortfall"));
                assert!(reason.contains("4,096") || reason.contains("4096"));
                assert!(reason.contains("reclaimable"));
            }
            Admission::Grant { .. } => panic!("memory shortfall must deny admission"),
        }
    }

    #[test]
    fn busy_machine_grants_zero_rather_than_denying() {
        let result = admit(&machine(0, 0), &policy(1_024, 512, 2), 2);
        assert_eq!(result, Admission::Grant { slots: 0 });
    }

    #[test]
    fn zero_per_slot_denies_rather_than_granting_infinity() {
        let result = admit(&machine(u64::MAX, 0), &policy(0, 0, usize::MAX), 0);
        match result {
            Admission::Deny { reason } => assert!(reason.contains("per_slot_mib")),
            Admission::Grant { .. } => panic!("zero per-slot budget must deny"),
        }
    }

    #[test]
    fn headroom_above_available_does_not_underflow() {
        let result = admit(&machine(100, 0), &policy(10, 101, 3), 0);
        match result {
            Admission::Deny { reason } => assert!(reason.contains("1 MiB")),
            Admission::Grant { .. } => panic!("headroom shortfall must deny"),
        }
    }

    #[test]
    fn impossible_policy_is_distinguishable_from_a_busy_machine() {
        let m = machine(8_000, 0);
        let p = policy(10_000, 7_000, 1);
        assert!(policy_is_impossible(&m, &p));
        assert_eq!(admit(&m, &p, 1), Admission::Grant { slots: 0 });
    }

    #[test]
    fn committed_mib_saturates() {
        assert_eq!(committed_mib(&policy(u64::MAX, 0, usize::MAX), 2), u64::MAX);
    }

    #[test]
    fn admission_is_limited_by_memory_and_remaining_slots() {
        assert_eq!(
            admit(&machine(5_500, 0), &policy(1_000, 500, 10), 2),
            Admission::Grant { slots: 5 }
        );
        assert_eq!(
            admit(&machine(50_000, 0), &policy(1_000, 500, 3), 1),
            Admission::Grant { slots: 2 }
        );
    }

    #[test]
    fn reclaimable_memory_is_the_observed_tmpfs_size() {
        assert_eq!(reclaimable_mib(&machine(100, 2_048)), 2_048);
    }

    #[test]
    fn impossible_policy_addition_saturates() {
        let m = Machine {
            total_mib: u64::MAX,
            available_mib: u64::MAX,
            tmpfs_mib: 0,
        };
        assert!(policy_is_impossible(
            &Machine {
                total_mib: u64::MAX - 1,
                ..m
            },
            &policy(u64::MAX, 1, 1)
        ));
    }
}

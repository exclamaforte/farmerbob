const RECLAIMABLE_THRESHOLD_MIB: u64 = 1024;

/// What the machine currently looks like. All figures in mebibytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine {
    /// Total physical memory in mebibytes.
    pub total_mib: u64,
    /// As reported by the OS. Already reduced by tmpfs contents.
    pub available_mib: u64,
    /// Bytes held in tmpfs mounts, which are RAM but belong to no process.
    pub tmpfs_mib: u64,
}

/// The admission policy for runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Budget reserved per admitted run.
    pub per_slot_mib: u64,
    /// Held back for the orchestrator and one verification cycle.
    pub headroom_mib: u64,
    /// Never admit more than this many, whatever the arithmetic says.
    pub max_slots: usize,
}

/// The result of asking whether runs may be admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Slots that may be granted right now.
    Grant { slots: usize },
    /// Nothing may be admitted, with the binding reason.
    Deny { reason: String },
}

/// How many slots the machine can support. `running` is the count already admitted.
pub fn admit(m: &Machine, p: &Policy, running: usize) -> Admission {
    if p.per_slot_mib == 0 {
        return deny(
            m,
            "per_slot_mib is zero; the policy is misconfigured".to_owned(),
        );
    }

    if running >= p.max_slots {
        return Admission::Grant { slots: 0 };
    }

    if m.available_mib <= p.headroom_mib {
        let shortfall = p.headroom_mib.saturating_sub(m.available_mib);
        return deny(
            m,
            format!("available memory has a {shortfall} MiB shortfall below headroom"),
        );
    }

    let memory_slots = m.available_mib.saturating_sub(p.headroom_mib) / p.per_slot_mib;
    let slot_limit = p.max_slots.saturating_sub(running);
    Admission::Grant {
        slots: memory_slots.min(slot_limit as u64) as usize,
    }
}

/// Memory that could be recovered by clearing tmpfs.
pub fn reclaimable_mib(m: &Machine) -> u64 {
    m.tmpfs_mib
}

/// The sum of budgets already committed by admitted runs.
pub fn committed_mib(p: &Policy, running: usize) -> u64 {
    p.per_slot_mib.saturating_mul(running as u64)
}

/// Returns whether headroom and one slot cannot fit in total machine memory.
pub fn policy_is_impossible(m: &Machine, p: &Policy) -> bool {
    p.headroom_mib.saturating_add(p.per_slot_mib) > m.total_mib
}

fn deny(m: &Machine, mut reason: String) -> Admission {
    if reclaimable_mib(m) > RECLAIMABLE_THRESHOLD_MIB {
        reason.push_str(&format!(
            "; {} MiB reclaimable from tmpfs",
            reclaimable_mib(m)
        ));
    }
    Admission::Deny { reason }
}

#[cfg(test)]
mod tests {
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

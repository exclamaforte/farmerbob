use std::collections::HashMap;

/// A slot index, identifying a position in the slot table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SlotIndex(pub u32);

/// A reference to a run, identifying it by name.
#[derive(Debug, Clone, Hash, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunRef(pub String);

/// Resource budget per slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Budget {
    /// Memory budget in MB per slot.
    pub memory_mb: u64,
    /// CPU quota as a percentage.
    pub cpu_quota_percent: u32,
    /// Maximum number of tasks per slot.
    pub tasks_max: u32,
}

/// Admission decision for a run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Admission {
    /// The run was admitted and assigned this slot.
    Admitted(SlotIndex),
    /// The run was queued with a reason explaining why.
    Queued { reason: String },
}

/// Memory figures for a machine.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Machine {
    /// Currently available memory, not total.
    pub available_mb: u64,
    /// Reserved for the orchestrator and its own verification work.
    pub headroom_mb: u64,
}

/// Tracks slot admission for concurrent agent runs on a single machine.
///
/// Admission is bounded by the sum of active budgets against available memory
/// minus headroom. Slot indices are reused when slots are released.
#[derive(Debug, Default)]
pub struct SlotTable {
    budget: Budget,
    slots: Vec<Option<RunRef>>,
    run_to_slot: HashMap<RunRef, SlotIndex>,
}

impl SlotTable {
    /// Create a new `SlotTable` with the given per-slot [`Budget`].
    pub fn new(budget: Budget) -> Self {
        Self {
            budget,
            slots: Vec::new(),
            run_to_slot: HashMap::new(),
        }
    }

    /// Admit `run` if the sum of active budgets plus this one still fits under
    /// `available_mb - headroom_mb`. Otherwise queue it with a reason.
    ///
    /// If `run` already holds a slot, that slot is returned without re-checking
    /// capacity.
    pub fn admit(&mut self, run: RunRef, machine: &Machine) -> Admission {
        if let Some(slot) = self.run_to_slot.get(&run) {
            return Admission::Admitted(*slot);
        }

        let ceiling = machine.available_mb.saturating_sub(machine.headroom_mb);
        let committed = self.committed_mb();
        let requested = self.budget.memory_mb;

        if committed.saturating_add(requested) <= ceiling {
            let slot_idx = self
                .slots
                .iter()
                .position(|s| s.is_none())
                .map(|i| SlotIndex(i as u32))
                .unwrap_or_else(|| {
                    let idx = self.slots.len() as u32;
                    self.slots.push(None);
                    SlotIndex(idx)
                });

            self.slots[slot_idx.0 as usize] = Some(run.clone());
            self.run_to_slot.insert(run, slot_idx);
            Admission::Admitted(slot_idx)
        } else {
            Admission::Queued {
                reason: format!(
                    "committed {}MB + request {}MB exceeds ceiling {}MB",
                    committed, requested, ceiling
                ),
            }
        }
    }

    /// Free the slot held by `run`. Unknown runs are a no-op.
    pub fn release(&mut self, run: &RunRef) -> Option<SlotIndex> {
        let slot = self.run_to_slot.remove(run)?;
        if (slot.0 as usize) < self.slots.len() {
            self.slots[slot.0 as usize] = None;
        }
        Some(slot)
    }

    /// Number of currently active (admitted) slots.
    pub fn active(&self) -> usize {
        self.run_to_slot.len()
    }

    /// Total memory committed across all active slots, in MB.
    pub fn committed_mb(&self) -> u64 {
        (self.run_to_slot.len() as u64).saturating_mul(self.budget.memory_mb)
    }

    /// How many slots this machine supports right now, from measured figures.
    ///
    /// Computed as `(available_mb - headroom_mb) / budget.memory_mb`, saturating
    /// at zero when headroom exceeds available, and returning zero when
    /// `memory_mb` is zero to avoid division by zero.
    pub fn capacity(&self, machine: &Machine) -> usize {
        let ceiling = machine.available_mb.saturating_sub(machine.headroom_mb);
        if self.budget.memory_mb == 0 {
            return 0;
        }
        (ceiling / self.budget.memory_mb) as usize
    }

    /// Which run holds a slot, if any.
    pub fn holder(&self, slot: SlotIndex) -> Option<&RunRef> {
        self.slots
            .get(slot.0 as usize)
            .and_then(|s| s.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(mb: u64) -> Budget {
        Budget {
            memory_mb: mb,
            cpu_quota_percent: 100,
            tasks_max: 4,
        }
    }

    fn machine(available: u64, headroom: u64) -> Machine {
        Machine {
            available_mb: available,
            headroom_mb: headroom,
        }
    }

    #[test]
    fn six_runs_at_6g_on_18g_do_not_all_admit() {
        let b = budget(6144);
        let m = machine(18432, 0);
        let mut table = SlotTable::new(b);

        let mut admitted = 0;
        let mut queued = 0;
        for i in 0..6 {
            let run = RunRef(format!("run-{}", i));
            match table.admit(run, &m) {
                Admission::Admitted(_) => admitted += 1,
                Admission::Queued { .. } => queued += 1,
            }
        }

        assert_eq!(admitted, 3);
        assert_eq!(queued, 3);
    }

    #[test]
    fn capacity_zero_when_headroom_exceeds_available() {
        let b = budget(1024);
        let m = machine(1024, 2048);
        let table = SlotTable::new(b);

        assert_eq!(table.capacity(&m), 0);
    }

    #[test]
    fn capacity_no_divide_by_zero_with_zero_memory() {
        let b = budget(0);
        let m = machine(18432, 0);
        let table = SlotTable::new(b);

        assert_eq!(table.capacity(&m), 0);
    }

    #[test]
    fn slot_indices_reused_after_release() {
        let b = budget(1024);
        let m = machine(4096, 0);
        let mut table = SlotTable::new(b);

        let run0 = RunRef("run0".into());
        let run1 = RunRef("run1".into());
        let run2 = RunRef("run2".into());

        let SlotIndex(idx0) = match table.admit(run0.clone(), &m) {
            Admission::Admitted(s) => s,
            _ => panic!("should admit"),
        };
        let SlotIndex(idx1) = match table.admit(run1.clone(), &m) {
            Admission::Admitted(s) => s,
            _ => panic!("should admit"),
        };
        let SlotIndex(idx2) = match table.admit(run2.clone(), &m) {
            Admission::Admitted(s) => s,
            _ => panic!("should admit"),
        };

        assert_eq!(idx0, 0);
        assert_eq!(idx1, 1);
        assert_eq!(idx2, 2);

        assert_eq!(table.release(&run1), Some(SlotIndex(1)));

        let run3 = RunRef("run3".into());
        let SlotIndex(idx3) = match table.admit(run3.clone(), &m) {
            Admission::Admitted(s) => s,
            _ => panic!("should admit"),
        };

        assert_eq!(idx3, 1);
        assert_eq!(table.holder(SlotIndex(1)), Some(&run3));
        assert_eq!(table.holder(SlotIndex(0)), Some(&run0));
        assert_eq!(table.holder(SlotIndex(2)), Some(&run2));
    }

    #[test]
    fn admitting_same_run_twice_yields_one_slot() {
        let b = budget(1024);
        let m = machine(4096, 0);
        let mut table = SlotTable::new(b);

        let run = RunRef("run".into());
        let first = table.admit(run.clone(), &m);
        let second = table.admit(run.clone(), &m);

        assert_eq!(first, second);
        assert_eq!(table.active(), 1);
    }

    #[test]
    fn release_unknown_run_is_noop() {
        let b = budget(1024);
        let _m = machine(4096, 0);
        let mut table = SlotTable::new(b);

        let run = RunRef("ghost".into());
        assert_eq!(table.release(&run), None);
    }

    #[test]
    fn committed_mb_tracks_active_slots() {
        let b = budget(2048);
        let m = machine(8192, 0);
        let mut table = SlotTable::new(b);

        assert_eq!(table.committed_mb(), 0);

        let r0 = RunRef("r0".into());
        let r1 = RunRef("r1".into());
        table.admit(r0.clone(), &m);
        assert_eq!(table.committed_mb(), 2048);
        table.admit(r1.clone(), &m);
        assert_eq!(table.committed_mb(), 4096);

        table.release(&r0);
        assert_eq!(table.committed_mb(), 2048);
    }

    #[test]
    fn capacity_reflects_budget() {
        let b = budget(3072);
        let m = machine(12288, 1024);
        let table = SlotTable::new(b);

        assert_eq!(table.capacity(&m), (12288 - 1024) / 3072);
    }

    #[test]
    fn holder_returns_none_for_free_slot() {
        let b = budget(1024);
        let m = machine(4096, 0);
        let mut table = SlotTable::new(b);

        let run = RunRef("r".into());
        let SlotIndex(idx) = match table.admit(run, &m) {
            Admission::Admitted(s) => s,
            _ => panic!("should admit"),
        };

        table.release(&RunRef("r".into()));
        assert_eq!(table.holder(SlotIndex(idx)), None);
    }

    #[test]
    fn large_memory_values_no_overflow() {
        let b = Budget {
            memory_mb: u64::MAX,
            cpu_quota_percent: 100,
            tasks_max: 1,
        };
        let m = machine(u64::MAX, 0);
        let mut table = SlotTable::new(b);

        let run = RunRef("big".into());
        match table.admit(run, &m) {
            Admission::Admitted(_) => {}
            Admission::Queued { .. } => {}
        }
    }

    #[test]
    fn admission_queued_reason_names_numbers() {
        let b = budget(6144);
        let m = machine(12288, 0);
        let mut table = SlotTable::new(b);

        for i in 0..4 {
            let run = RunRef(format!("run-{}", i));
            table.admit(run, &m);
        }

        let run = RunRef("extra".into());
        match table.admit(run, &m) {
            Admission::Queued { reason } => {
                assert!(reason.contains("committed"));
                assert!(reason.contains("request"));
                assert!(reason.contains("ceiling"));
            }
            Admission::Admitted(_) => panic!("should be queued"),
        }
    }
}
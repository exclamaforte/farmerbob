//! Slots and resource specs: how machine capacity is divided among runs.

use serde::{Deserialize, Serialize};

use crate::ids::RunId;

/// CPU/memory/process budget confining one run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceSpec {
    /// CPU ceiling as a percentage of one core, following the cgroup
    /// `cpu.max` convention: `800` means the run may use up to 8 cores.
    pub cpu_quota_percent: u32,
    /// Memory ceiling in bytes (cgroup `memory.max`).
    pub memory_max_bytes: u64,
    /// Optional CPU pinning as a kernel cpuset expression, e.g. `"0-7"`.
    pub cpuset: Option<String>,
    /// Maximum concurrent processes (cgroup pids controller).
    pub tasks_max: u32,
}

impl Default for ResourceSpec {
    /// Sane default for a 32-core / 30 GiB machine running ~4 agents at
    /// once: a quarter of the machine each — 8 cores (`800` percent), 7 GiB
    /// of memory, no CPU pinning, and a 256-process ceiling.
    fn default() -> Self {
        Self {
            cpu_quota_percent: 800,
            memory_max_bytes: 7 * 1024 * 1024 * 1024,
            cpuset: None,
            tasks_max: 256,
        }
    }
}

/// One divisible unit of machine capacity. A run occupies at most one slot
/// at a time; `occupant` is that run while it holds the slot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Slot {
    pub index: u32,
    pub spec: ResourceSpec,
    pub occupant: Option<RunId>,
}

impl Slot {
    /// True while no run holds the slot.
    pub fn is_free(&self) -> bool {
        self.occupant.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spec_leaves_a_quarter_machine_per_agent() {
        let spec = ResourceSpec::default();
        assert_eq!(spec.cpu_quota_percent, 800);
        assert_eq!(spec.memory_max_bytes, 7 * 1024 * 1024 * 1024);
        assert_eq!(spec.cpuset, None);
        assert_eq!(spec.tasks_max, 256);
        // Four defaults fit exactly in the machine's CPU and memory budget.
        assert!(4 * spec.cpu_quota_percent <= 32 * 100);
        assert!(4 * spec.memory_max_bytes <= 30 * 1024 * 1024 * 1024);
    }

    #[test]
    fn slot_occupancy_round_trips() {
        let slot = Slot {
            index: 3,
            spec: ResourceSpec::default(),
            occupant: Some(RunId::new()),
        };
        assert!(!slot.is_free());

        let back: Slot = serde_json::from_str(&serde_json::to_string(&slot).unwrap()).unwrap();
        assert_eq!(slot, back);
    }
}

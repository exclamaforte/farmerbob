//! The wave-launch decision, as a pure function.
//!
//! Whether to launch a queued wave was decided by `fb-autopilot.sh`, in shell,
//! where every defect in the decision cost real time before this module
//! existed: it launched only when nothing at all was running, so a three-arm
//! wave against a six-slot machine idled half the box by construction; a
//! global lock refused a second wave however many slots were free, while the
//! slot check paired with it could never be reached; a bead-scoped lock taken
//! exclusive and held for a whole run forced the arms of one wave to run
//! strictly sequentially; and waves were started against a target another
//! wave was writing, and against a module that did not exist yet. None of
//! that was testable. This module makes the decision a function so each of
//! those failures is a pinned test case instead of lost time.
//!
//! # Boundaries and Guarantees
//!
//! - A wave whose target equals the target of any running wave is held as
//!   [`Held::TargetBusy`], whatever the slot count. So is a wave whose target
//!   equals the target of a wave this same call has already admitted — a wave
//!   admitted earlier in the call occupies its target exactly as a running
//!   wave does. A wave that was held occupies nothing: it never started.
//! - A wave is admitted only if its arms fit entirely in the free slots,
//!   where free is `slots` minus every running and admitted arm. A partial
//!   wave is never launched: its remaining arms would queue behind a full
//!   machine and straggle for hours.
//! - Admission accumulates. A wave admitted by a call occupies its arms and
//!   its wave slot for every later wave in the same call, so a third wave can
//!   fail on resources the first two consumed.
//! - `max_waves` counts running waves plus waves admitted by this call. Once
//!   reached, everything further is held as [`Held::TooManyWaves`], even with
//!   every slot free.
//! - Reasons are checked in the order [`Held`] declares its variants and the
//!   first match is reported: a wave that is both target-busy and
//!   slot-starved reports [`Held::TargetBusy`].
//! - Queue order is the caller's order, preserved in both output vectors.
//!   [`Launch::launch`] is deliberately not sorted — the caller's order is
//!   the queue's priority, and re-sorting it would silently reorder work.
//! - Every queued wave appears exactly once across `launch` and `held`.
//! - A wave with zero arms needs no slots, so no slot count — including zero
//!   — holds it. It still occupies a wave slot and its target, because it is
//!   a wave like any other once admitted. A zero-arm wave is a malformed
//!   queue file; launching it is harmless while silently dropping it is not.
//! - Target comparison is exact string matching. No case folding, no path
//!   normalization, no prefix or suffix matching.
//! - Running arms beyond `slots` saturate: free slots are zero, never
//!   negative.
//! - [`Held`] is a closed set of three. There is deliberately no unknown
//!   variant and no force: a wave this module cannot admit must not start,
//!   and a caller wanting to override passes different inputs rather than
//!   asking for a bypass.

/// A wave waiting in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    /// The queue file's name, e.g. `"wave74"`. Ordering key.
    pub id: String,
    /// The task it dispatches.
    pub task: String,
    /// Repo-relative path the task declares it will write.
    pub target: String,
    /// How many arms it would dispatch.
    pub arms: u32,
}

/// A wave already running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// The task.
    pub task: String,
    /// The target it is writing.
    pub target: String,
    /// Arms currently holding a slot.
    pub arms: u32,
}

/// Why a queued wave was not launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Another running wave is writing the same target.
    TargetBusy,
    /// Not enough free slots for the whole wave.
    NotEnoughSlots,
    /// The concurrent-wave limit is already reached.
    TooManyWaves,
}

/// What the autopilot should do now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Launch {
    /// Waves to launch, in queue order.
    pub launch: Vec<String>,
    /// Waves held, each with the first reason that applied, in queue order.
    pub held: Vec<(String, Held)>,
}

/// Decide which queued waves may start.
///
/// `slots` is the machine's total capacity; running arms are subtracted from
/// it. `max_waves` caps concurrent waves, counting those already running.
///
/// The queue is scanned once, in the order given. For each wave the held
/// reasons are tested in the order [`Held`] declares them, and the first
/// match wins; a wave matching none is admitted, and its arms, its wave
/// slot, and its target are then occupied for every wave after it.
pub fn plan(queued: &[Queued], running: &[Running], slots: u32, max_waves: u32) -> Launch {
    let running_arms = running
        .iter()
        .fold(0u32, |acc, wave| acc.saturating_add(wave.arms));
    let mut free_slots = slots.saturating_sub(running_arms);
    let mut waves_up = u32::try_from(running.len()).unwrap_or(u32::MAX);
    let mut busy: Vec<&str> = running.iter().map(|wave| wave.target.as_str()).collect();

    let mut launch = Vec::new();
    let mut held = Vec::new();

    for wave in queued {
        let reason = if busy.contains(&wave.target.as_str()) {
            Some(Held::TargetBusy)
        } else if wave.arms > free_slots {
            Some(Held::NotEnoughSlots)
        } else if waves_up >= max_waves {
            Some(Held::TooManyWaves)
        } else {
            None
        };

        match reason {
            Some(reason) => held.push((wave.id.clone(), reason)),
            None => {
                busy.push(wave.target.as_str());
                free_slots = free_slots.saturating_sub(wave.arms);
                waves_up = waves_up.saturating_add(1);
                launch.push(wave.id.clone());
            }
        }
    }

    Launch { launch, held }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(id: &str, target: &str, arms: u32) -> Queued {
        Queued {
            id: id.to_string(),
            task: format!("dispatch {id}"),
            target: target.to_string(),
            arms,
        }
    }

    fn running(target: &str, arms: u32) -> Running {
        Running {
            task: format!("run on {target}"),
            target: target.to_string(),
            arms,
        }
    }

    /// Clause 7: every queued wave appears exactly once across launch and held.
    fn assert_partition(queued: &[Queued], out: &Launch) {
        assert_eq!(
            out.launch.len() + out.held.len(),
            queued.len(),
            "launch + held must cover the queue exactly"
        );
        for wave in queued {
            let launches = out.launch.iter().filter(|id| *id == &wave.id).count();
            let holds = out.held.iter().filter(|(id, _)| id == &wave.id).count();
            assert_eq!(
                launches + holds,
                1,
                "wave {:?} appears {launches}+{holds} times across launch and held",
                wave.id
            );
        }
    }

    #[test]
    fn empty_queue_yields_default() {
        assert_eq!(plan(&[], &[], 0, 0), Launch::default());
        assert_eq!(
            plan(&[], &[running("src/lib.rs", 3)], 4, 2),
            Launch::default()
        );
    }

    #[test]
    fn target_busy_against_running_holds_regardless_of_slots() {
        // A hundred free slots do not matter: the target is being written.
        let out = plan(
            &[
                queued("a", "src/main.rs", 1),
                queued("b", "src/other.rs", 1),
            ],
            &[running("src/main.rs", 2)],
            100,
            10,
        );
        assert_eq!(out.launch, vec!["b".to_string()]);
        assert_eq!(out.held, vec![("a".to_string(), Held::TargetBusy)]);
    }

    #[test]
    fn partial_wave_is_never_launched() {
        // Free is 2; a 3-arm wave does not fit entirely, so it does not start.
        let out = plan(&[queued("a", "t-a", 3)], &[running("r", 3)], 5, 10);
        assert!(out.launch.is_empty());
        assert_eq!(out.held, vec![("a".to_string(), Held::NotEnoughSlots)]);
    }

    #[test]
    fn wave_fitting_free_slots_exactly_launches() {
        // At the boundary: arms equal to free slots fits entirely.
        let out = plan(&[queued("a", "t-a", 2)], &[running("r", 3)], 5, 10);
        assert_eq!(out.launch, vec!["a".to_string()]);
        assert!(out.held.is_empty());
    }

    #[test]
    fn slot_admission_accumulates_across_this_call() {
        // Each wave alone fits in 6 slots; the third fails only because the
        // first two were admitted and consumed their arms.
        let out = plan(
            &[
                queued("a", "t-a", 3),
                queued("b", "t-b", 3),
                queued("c", "t-c", 3),
            ],
            &[],
            6,
            10,
        );
        assert_eq!(out.launch, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(out.held, vec![("c".to_string(), Held::NotEnoughSlots)]);
    }

    #[test]
    fn wave_limit_reached_by_running_alone_holds_everything() {
        // Two waves running, limit two: already reached, though every slot
        // is free.
        let out = plan(
            &[queued("a", "t-a", 1), queued("b", "t-b", 1)],
            &[running("r1", 1), running("r2", 1)],
            100,
            2,
        );
        assert!(out.launch.is_empty());
        assert_eq!(
            out.held,
            vec![
                ("a".to_string(), Held::TooManyWaves),
                ("b".to_string(), Held::TooManyWaves),
            ]
        );
    }

    #[test]
    fn launches_this_call_count_toward_wave_limit() {
        // The third wave fails only because two were admitted earlier in the
        // same call.
        let out = plan(
            &[
                queued("a", "t-a", 1),
                queued("b", "t-b", 1),
                queued("c", "t-c", 1),
            ],
            &[],
            100,
            2,
        );
        assert_eq!(out.launch, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(out.held, vec![("c".to_string(), Held::TooManyWaves)]);
    }

    #[test]
    fn target_busy_is_reported_before_slot_starvation() {
        // The wave is both target-busy and slot-starved; the first matching
        // reason in Held's declaration order wins.
        let out = plan(&[queued("a", "x", 1)], &[running("x", 1)], 1, 10);
        assert_eq!(out.held, vec![("a".to_string(), Held::TargetBusy)]);
    }

    #[test]
    fn slot_starvation_is_reported_before_wave_limit() {
        // Free is 0 AND the wave limit is reached; NotEnoughSlots is declared
        // first, so it is the reason reported.
        let out = plan(&[queued("a", "y", 1)], &[running("x", 1)], 1, 1);
        assert_eq!(out.held, vec![("a".to_string(), Held::NotEnoughSlots)]);
    }

    #[test]
    fn queue_order_is_preserved_in_both_outputs() {
        // Ids chosen out of alphabetical order: neither output vector may be
        // sorted, because the caller's order is the queue's priority.
        let out = plan(
            &[
                queued("z9", "t-z", 1),
                queued("a1", "t-a", 1),
                queued("m3", "t-m", 1),
                queued("b2", "t-b", 1),
            ],
            &[],
            2,
            10,
        );
        assert_eq!(out.launch, vec!["z9".to_string(), "a1".to_string()]);
        assert_eq!(
            out.held,
            vec![
                ("m3".to_string(), Held::NotEnoughSlots),
                ("b2".to_string(), Held::NotEnoughSlots),
            ]
        );
        assert_partition(
            &[
                queued("z9", "t-z", 1),
                queued("a1", "t-a", 1),
                queued("m3", "t-m", 1),
                queued("b2", "t-b", 1),
            ],
            &out,
        );
    }

    #[test]
    fn every_queued_wave_appears_exactly_once_across_outputs() {
        // A mixed scenario touching all three reasons, checked as a partition.
        let queue = vec![
            queued("a", "t-y", 1), // launches
            queued("b", "t-z", 1), // launches
            queued("c", "t-q", 0), // no arms needed, but the wave limit is now full
            queued("d", "x", 1),   // running wave is on x
            queued("e", "t-r", 5), // does not fit
        ];
        let out = plan(&queue, &[running("x", 1)], 3, 3);
        assert_eq!(out.launch, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            out.held,
            vec![
                ("c".to_string(), Held::TooManyWaves),
                ("d".to_string(), Held::TargetBusy),
                ("e".to_string(), Held::NotEnoughSlots),
            ]
        );
        assert_partition(&queue, &out);
    }

    #[test]
    fn second_queued_wave_on_same_target_is_held() {
        // Clause 8: the first wave is about to start on t-a; the second must
        // not join it, whatever the slot count says.
        let out = plan(
            &[
                queued("a", "t-a", 2),
                queued("b", "t-a", 2),
                queued("c", "t-c", 2),
            ],
            &[],
            100,
            10,
        );
        assert_eq!(out.launch, vec!["a".to_string(), "c".to_string()]);
        assert_eq!(out.held, vec![("b".to_string(), Held::TargetBusy)]);
    }

    #[test]
    fn held_wave_does_not_occupy_its_target() {
        // The first wave never starts, so its target stays free: the second
        // wave launches onto it.
        let out = plan(&[queued("a", "x", 5), queued("b", "x", 1)], &[], 1, 10);
        assert_eq!(out.launch, vec!["b".to_string()]);
        assert_eq!(out.held, vec![("a".to_string(), Held::NotEnoughSlots)]);
    }

    #[test]
    fn zero_slots_hold_every_wave_with_arms() {
        let out = plan(&[queued("a", "t-a", 1)], &[], 0, 10);
        assert!(out.launch.is_empty());
        assert_eq!(out.held, vec![("a".to_string(), Held::NotEnoughSlots)]);
    }

    #[test]
    fn zero_arm_wave_is_never_held_by_slots() {
        // A zero-arm wave needs no slots, so even zero slots do not hold it,
        // and it consumes nothing: a one-arm wave still launches after it.
        let out = plan(&[queued("a", "t-a", 0), queued("b", "t-b", 1)], &[], 1, 10);
        assert_eq!(out.launch, vec!["a".to_string(), "b".to_string()]);
        assert!(out.held.is_empty());
    }

    #[test]
    fn zero_arm_wave_still_counts_against_wave_limit() {
        // Slots never hold it, but it is a wave: once admitted it occupies
        // its wave slot like any other.
        let out = plan(&[queued("a", "t-a", 0), queued("b", "t-b", 0)], &[], 10, 1);
        assert_eq!(out.launch, vec!["a".to_string()]);
        assert_eq!(out.held, vec![("b".to_string(), Held::TooManyWaves)]);
    }

    #[test]
    fn zero_arm_wave_still_occupies_its_target() {
        // Being admitted with no arms does not exempt it from clause 8.
        let out = plan(&[queued("a", "x", 0), queued("b", "x", 0)], &[], 10, 10);
        assert_eq!(out.launch, vec!["a".to_string()]);
        assert_eq!(out.held, vec![("b".to_string(), Held::TargetBusy)]);
    }

    #[test]
    fn running_arms_beyond_slots_saturate_at_zero_free() {
        // Five running arms on a two-slot machine: free is zero, not
        // negative. A one-arm wave is held; a zero-arm wave is not.
        let out = plan(
            &[queued("z", "t-z", 1), queued("zero", "t-zero", 0)],
            &[running("r", 5)],
            2,
            10,
        );
        assert_eq!(out.launch, vec!["zero".to_string()]);
        assert_eq!(out.held, vec![("z".to_string(), Held::NotEnoughSlots)]);
    }

    #[test]
    fn zero_max_waves_holds_everything() {
        // An empty machine with free slots still holds everything: the limit
        // is zero and already reached.
        let out = plan(
            &[queued("a", "t-a", 1), queued("zero", "t-zero", 0)],
            &[],
            10,
            0,
        );
        assert!(out.launch.is_empty());
        assert_eq!(
            out.held,
            vec![
                ("a".to_string(), Held::TooManyWaves),
                ("zero".to_string(), Held::TooManyWaves),
            ]
        );
    }

    #[test]
    fn duplicate_ids_are_distinct_entries() {
        // Two queued waves with the same id are distinct entries, both
        // considered in order; the second is subject to clause 8 against the
        // first.
        let out = plan(&[queued("w", "t-1", 1), queued("w", "t-2", 1)], &[], 10, 10);
        assert_eq!(out.launch, vec!["w".to_string(), "w".to_string()]);
        assert!(out.held.is_empty());

        let out = plan(&[queued("v", "t-3", 1), queued("v", "t-3", 1)], &[], 10, 10);
        assert_eq!(out.launch, vec!["v".to_string()]);
        assert_eq!(out.held, vec![("v".to_string(), Held::TargetBusy)]);
    }

    #[test]
    fn target_comparison_is_exact() {
        // No case folding, no extension or prefix matching: three distinct
        // targets, three launches.
        let out = plan(
            &[
                queued("a", "src/lib.rs", 1),
                queued("b", "src/Lib.rs", 1),
                queued("c", "src/lib.rs.bak", 1),
            ],
            &[],
            10,
            10,
        );
        assert_eq!(
            out.launch,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(out.held.is_empty());
    }
}

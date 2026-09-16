// farmerbob's OWN acceptance test for the fnd-core spec.
// The agent's tests prove the agent agrees with itself. This proves it agrees with the spec.
// Uses ONLY names the spec mandated, so it compiles against any conforming implementation.
#[cfg(test)]
mod fb_conformance {
    use crate::*;

    #[test] fn terminal_states_are_terminal() {
        assert!(RunState::Succeeded.is_terminal());
        assert!(RunState::Killed.is_terminal());
        assert!(RunState::Abandoned.is_terminal());
        assert!(RunState::Failed { reason: "x".into() }.is_terminal());
    }
    #[test] fn nonterminal_states_are_not_terminal() {
        assert!(!RunState::Queued.is_terminal());
        assert!(!RunState::Starting.is_terminal());
        assert!(!RunState::Running.is_terminal());
        assert!(!RunState::BlockedOnLease.is_terminal());
    }
    #[test] fn terminal_transitions_to_nothing() {
        for from in [RunState::Succeeded, RunState::Killed, RunState::Abandoned] {
            for to in [RunState::Queued, RunState::Starting, RunState::Running,
                       RunState::BlockedOnLease, RunState::Succeeded] {
                assert!(!from.can_transition_to(&to),
                    "{:?} -> {:?} must be illegal", from, to);
            }
        }
    }
    #[test] fn queued_cannot_jump_to_succeeded() {
        assert!(!RunState::Queued.can_transition_to(&RunState::Succeeded));
    }
    #[test] fn the_happy_path_is_legal() {
        assert!(RunState::Queued.can_transition_to(&RunState::Starting));
        assert!(RunState::Starting.can_transition_to(&RunState::Running));
        assert!(RunState::Running.can_transition_to(&RunState::Succeeded));
    }
    #[test] fn blocked_runs_can_resume() {
        assert!(RunState::BlockedOnLease.can_transition_to(&RunState::Running));
    }
    #[test] fn resource_default_is_sane_for_this_box() {
        let d = ResourceSpec::default();
        assert!(d.cpu_quota_percent > 0 && d.cpu_quota_percent <= 3200,
            "cpu_quota_percent {} outside 1..=3200 for a 32-core box", d.cpu_quota_percent);
        assert!(d.memory_max_bytes >= 512*1024*1024 && d.memory_max_bytes <= 30*1024*1024*1024,
            "memory_max_bytes {} not sane for a 30GB box running ~4 agents", d.memory_max_bytes);
        assert!(d.tasks_max > 0);
    }
}

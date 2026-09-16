//! Runs: one agent's attempt at one task, plus its lifecycle state machine.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, RunId, TaskId};

/// The lifecycle state of a [`Run`].
///
/// Legal transitions:
///
/// ```text
/// Queued         -> Starting, Killed, Abandoned
/// Starting       -> Running, BlockedOnLease, BlockedOnQuota,
///                   Failed, Killed, Abandoned
/// Running        -> Succeeded, Failed, Killed, Abandoned,
///                   BlockedOnLease, BlockedOnQuota
/// BlockedOnLease -> Running, Failed, Killed, Abandoned
/// BlockedOnQuota -> Running, Failed, Killed, Abandoned
/// terminal       -> (nothing)
/// ```
///
/// `Succeeded`, `Failed`, `Killed`, and `Abandoned` are terminal: a run in a
/// terminal state can never move again. In particular, `Queued` cannot jump
/// straight to `Succeeded` — a run must at least pass through `Starting` and
/// `Running`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    /// Accepted by the scheduler, not yet dispatched.
    Queued,
    /// Being dispatched: worktree provisioned, agent process launching.
    Starting,
    /// The agent is actively working.
    Running,
    /// Waiting for a [`Lease`](crate::Lease) on a contended resource.
    BlockedOnLease,
    /// Waiting for a provider quota or rate limit to reset.
    BlockedOnQuota {
        /// When the quota is expected to become available again.
        reset_at: DateTime<Utc>,
    },
    /// The agent finished and its work was accepted.
    Succeeded,
    /// The agent finished but its work failed validation, or the run
    /// errored out.
    Failed {
        /// Human-readable explanation of the failure.
        reason: String,
    },
    /// Torn down by the operator or the scheduler (e.g. timeout, OOM kill).
    Killed,
    /// The run's bookkeeping was lost or its holder went away before it
    /// reached a terminal state.
    Abandoned,
}

impl RunState {
    /// True for states a run can never leave: `Succeeded`, `Failed`,
    /// `Killed`, `Abandoned`.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunState::Succeeded | RunState::Failed { .. } | RunState::Killed | RunState::Abandoned
        )
    }

    /// Whether the transition `self -> next` is legal under the state
    /// machine documented on [`RunState`].
    pub fn can_transition_to(&self, next: &RunState) -> bool {
        use RunState::*;

        if self.is_terminal() {
            return false;
        }

        match self {
            Queued => matches!(next, Starting | Killed | Abandoned),
            Starting => matches!(
                next,
                Running
                    | BlockedOnLease
                    | BlockedOnQuota { .. }
                    | Failed { .. }
                    | Killed
                    | Abandoned
            ),
            Running => matches!(
                next,
                Succeeded
                    | Failed { .. }
                    | Killed
                    | Abandoned
                    | BlockedOnLease
                    | BlockedOnQuota { .. }
            ),
            BlockedOnLease | BlockedOnQuota { .. } => {
                matches!(next, Running | Failed { .. } | Killed | Abandoned)
            }
            // Unreachable after the early return, but required for
            // exhaustiveness.
            Succeeded | Failed { .. } | Killed | Abandoned => false,
        }
    }
}

/// One agent's attempt at one task, executing in its own worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub task_id: TaskId,
    pub agent_id: AgentId,
    /// Worktree the agent works in, branched off the task's repository.
    pub worktree: PathBuf,
    /// Git branch the run commits to.
    pub branch: String,
    /// Slot the run is confined to, once one has been assigned.
    pub slot_index: Option<u32>,
    pub state: RunState,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

impl Run {
    /// Wall-clock time from start to end. `None` until the run has both
    /// started and ended.
    pub fn duration(&self) -> Option<Duration> {
        Some(self.ended_at? - self.started_at?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(offset_secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + offset_secs, 0).unwrap()
    }

    fn running() -> Run {
        Run {
            id: RunId::new(),
            task_id: TaskId::new(),
            agent_id: AgentId::new(),
            worktree: PathBuf::from("/srv/farmerbob/worktrees/t1"),
            branch: String::from("fb/task-1/agent"),
            slot_index: Some(2),
            state: RunState::Running,
            created_at: ts(0),
            started_at: Some(ts(10)),
            ended_at: None,
        }
    }

    #[test]
    fn terminal_states_are_terminal() {
        let terminal = [
            RunState::Succeeded,
            RunState::Failed {
                reason: String::from("tests failed"),
            },
            RunState::Killed,
            RunState::Abandoned,
        ];
        for state in terminal {
            assert!(state.is_terminal(), "{state:?} should be terminal");
        }

        let non_terminal = [
            RunState::Queued,
            RunState::Starting,
            RunState::Running,
            RunState::BlockedOnLease,
            RunState::BlockedOnQuota { reset_at: ts(60) },
        ];
        for state in non_terminal {
            assert!(!state.is_terminal(), "{state:?} should not be terminal");
        }
    }

    #[test]
    fn legal_transitions_are_allowed() {
        use RunState::*;
        let legal = [
            (Queued, Starting),
            (Queued, Killed),
            (Starting, Running),
            (Starting, BlockedOnLease),
            (Running, Succeeded),
            (
                Running,
                Failed {
                    reason: String::from("boom"),
                },
            ),
            (Running, BlockedOnQuota { reset_at: ts(60) }),
            (BlockedOnLease, Running),
            (BlockedOnQuota { reset_at: ts(60) }, Running),
        ];
        for (from, to) in legal {
            assert!(
                from.can_transition_to(&to),
                "{from:?} -> {to:?} should be legal"
            );
        }
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        use RunState::*;
        let illegal = [
            // A queued run has not run; it cannot succeed.
            (Queued, Succeeded),
            // No restarts: only blocked states may resume into Running.
            (Running, Queued),
            (Running, Starting),
            (Queued, BlockedOnLease),
            // Terminal states go nowhere.
            (Succeeded, Running),
            (
                Failed {
                    reason: String::from("boom"),
                },
                Killed,
            ),
            (Killed, Abandoned),
            (Abandoned, Running),
        ];
        for (from, to) in illegal {
            assert!(
                !from.can_transition_to(&to),
                "{from:?} -> {to:?} should be illegal"
            );
        }
    }

    #[test]
    fn duration_spans_start_to_end() {
        let mut run = running();
        assert_eq!(run.duration(), None, "no duration until the run ends");

        run.ended_at = Some(ts(85));
        assert_eq!(run.duration(), Some(Duration::seconds(75)));

        run.started_at = None;
        assert_eq!(run.duration(), None, "no duration without a start");
    }

    #[test]
    fn run_round_trips_through_json() {
        let run = running();
        let json = serde_json::to_string(&run).unwrap();
        let back: Run = serde_json::from_str(&json).unwrap();
        assert_eq!(run, back);
    }

    #[test]
    fn data_carrying_states_round_trip() {
        let states = [
            RunState::BlockedOnQuota { reset_at: ts(120) },
            RunState::Failed {
                reason: String::from("grade below threshold"),
            },
        ];
        for state in states {
            let back: RunState =
                serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
            assert_eq!(state, back);
        }
    }
}

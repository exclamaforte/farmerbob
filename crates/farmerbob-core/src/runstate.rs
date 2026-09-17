//! Run lifecycle with an explicit quota-blocked state.
//!
//! A run is usually ordinary: it is queued, dispatched, verified, and then
//! done. One transition is not ordinary: a run stopped because the provider
//! refused on quota grounds has not failed and must be resumable when the
//! bucket resets rather than restarted or scored as a loss.
//!
//! This module is pure logic: no I/O and no clock reads. Time enters only as
//! the `now_ms` / `at_ms` parameters on [`step`], [`resumable`], and the
//! timestamps in `history` for [`blocked_ms`].

/// Exactly the lifecycle states a run may occupy and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    /// Accepted but not yet dispatched.
    Queued,
    /// The agent is actively working; holds the dispatch time.
    Running {
        /// Milliseconds at which the current running segment started.
        started_ms: u64,
    },
    /// Stopped by the provider, not by the agent. `resume_at_ms` is `None`
    /// when the provider stated no reset time.
    BlockedOnQuota {
        /// Provider bucket that refused the work.
        bucket: String,
        /// Milliseconds at which the run became blocked.
        since_ms: u64,
        /// Milliseconds at which the provider says the bucket resets, if stated.
        resume_at_ms: Option<u64>,
    },
    /// The agent finished and its work is being checked.
    Verifying,
    /// Terminal. The run was checked; `passed` records the verdict.
    Done {
        /// Whether verification passed.
        passed: bool,
    },
    /// Terminal. A run that failed on its own merits.
    Failed {
        /// Human-readable explanation of the failure.
        reason: String,
    },
    /// Terminal. Stopped by the orchestrator.
    Cancelled,
}

impl RunState {
    /// True for Done, Failed and Cancelled. BlockedOnQuota is NOT terminal.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunState::Done { .. } | RunState::Failed { .. } | RunState::Cancelled
        )
    }

    /// True only for BlockedOnQuota.
    pub fn is_blocked(&self) -> bool {
        matches!(self, RunState::BlockedOnQuota { .. })
    }
}

/// The events that may move a run from one [`RunState`] to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Dispatch a queued run at the given time.
    Dispatched {
        /// Milliseconds at which the run was dispatched.
        at_ms: u64,
    },
    /// The provider refused on quota grounds while running.
    QuotaBlocked,
    /// The quota bucket reset; a blocked run may resume.
    QuotaReset,
    /// The agent finished; verification begins.
    VerifyStarted,
    /// Verification finished with a verdict.
    Verified {
        /// Whether verification passed.
        passed: bool,
    },
    /// The run failed on its own merits.
    FailedNow,
    /// The orchestrator stopped the run.
    CancelledNow,
}

/// The reason a [`step`] transition was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// The event is meaningless in this state.
    Illegal {
        /// Debug rendering of the state the run was in.
        from: String,
        /// Debug rendering of the event that was applied.
        event: String,
    },
    /// The run is already terminal and cannot move again.
    Terminal {
        /// Debug rendering of the terminal state the run was in.
        from: String,
    },
}

/// Render a state for error payloads.
fn state_name(s: &RunState) -> String {
    match s {
        RunState::Queued => String::from("Queued"),
        RunState::Running { started_ms } => format!("Running {{ started_ms: {started_ms} }}"),
        RunState::BlockedOnQuota {
            bucket,
            since_ms,
            resume_at_ms,
        } => match resume_at_ms {
            Some(t) => format!(
                "BlockedOnQuota {{ bucket: {bucket:?}, since_ms: {since_ms}, resume_at_ms: Some({t}) }}"
            ),
            None => format!(
                "BlockedOnQuota {{ bucket: {bucket:?}, since_ms: {since_ms}, resume_at_ms: None }}"
            ),
        },
        RunState::Verifying => String::from("Verifying"),
        RunState::Done { passed } => format!("Done {{ passed: {passed} }}"),
        RunState::Failed { reason } => format!("Failed {{ reason: {reason:?} }}"),
        RunState::Cancelled => String::from("Cancelled"),
    }
}

/// Render an event for error payloads.
fn event_name(e: &Event) -> String {
    match e {
        Event::Dispatched { at_ms } => format!("Dispatched {{ at_ms: {at_ms} }}"),
        Event::QuotaBlocked => String::from("QuotaBlocked"),
        Event::QuotaReset => String::from("QuotaReset"),
        Event::VerifyStarted => String::from("VerifyStarted"),
        Event::Verified { passed } => format!("Verified {{ passed: {passed} }}"),
        Event::FailedNow => String::from("FailedNow"),
        Event::CancelledNow => String::from("CancelledNow"),
    }
}

/// Apply one event. `bucket` and `resume_at_ms` are used only by `QuotaBlocked`.
///
/// `QuotaBlocked` is legal only from `Running`; `QuotaReset` is legal only
/// from `BlockedOnQuota` and returns to `Running` with `started_ms` set to
/// `now_ms`. `CancelledNow` and `FailedNow` are legal from every
/// non-terminal state. Any event on a terminal state reports [`TransitionError::Terminal`],
/// checked before [`TransitionError::Illegal`].
pub fn step(
    s: &RunState,
    e: Event,
    now_ms: u64,
    bucket: &str,
    resume_at_ms: Option<u64>,
) -> Result<RunState, TransitionError> {
    if s.is_terminal() {
        return Err(TransitionError::Terminal {
            from: state_name(s),
        });
    }
    match e {
        Event::Dispatched { at_ms } => match s {
            RunState::Queued => Ok(RunState::Running { started_ms: at_ms }),
            _ => Err(TransitionError::Illegal {
                from: state_name(s),
                event: event_name(&e),
            }),
        },
        Event::QuotaBlocked => match s {
            RunState::Running { .. } => Ok(RunState::BlockedOnQuota {
                bucket: String::from(bucket),
                since_ms: now_ms,
                resume_at_ms,
            }),
            _ => Err(TransitionError::Illegal {
                from: state_name(s),
                event: event_name(&e),
            }),
        },
        Event::QuotaReset => match s {
            RunState::BlockedOnQuota { .. } => Ok(RunState::Running { started_ms: now_ms }),
            _ => Err(TransitionError::Illegal {
                from: state_name(s),
                event: event_name(&e),
            }),
        },
        Event::VerifyStarted => match s {
            RunState::Running { .. } => Ok(RunState::Verifying),
            _ => Err(TransitionError::Illegal {
                from: state_name(s),
                event: event_name(&e),
            }),
        },
        Event::Verified { passed } => match s {
            RunState::Verifying => Ok(RunState::Done { passed }),
            _ => Err(TransitionError::Illegal {
                from: state_name(s),
                event: event_name(&e),
            }),
        },
        Event::FailedNow => Ok(RunState::Failed {
            reason: String::from("failed"),
        }),
        Event::CancelledNow => Ok(RunState::Cancelled),
    }
}

/// A run that may be resumed now: blocked, and either its reset has passed or it stated
/// no reset time and `now_ms - since_ms` is at least `default_window_ms`.
///
/// Returns false for every non-blocked state. For a blocked run with a stated
/// reset, true once `now_ms >= resume_at_ms`. For a blocked run with no
/// stated reset, true only after `default_window_ms` has elapsed since
/// `since_ms`; a `now_ms` before `since_ms` never counts as elapsed.
pub fn resumable(s: &RunState, now_ms: u64, default_window_ms: u64) -> bool {
    match s {
        RunState::BlockedOnQuota {
            since_ms,
            resume_at_ms,
            ..
        } => match resume_at_ms {
            Some(at) => now_ms >= *at,
            None => match now_ms.checked_sub(*since_ms) {
                Some(elapsed) => elapsed >= default_window_ms,
                None => false,
            },
        },
        _ => false,
    }
}

/// Total milliseconds a run has spent blocked, so the leaderboard can report an arm's
/// wallclock without charging it for its provider's billing policy.
///
/// Each maximal blocked segment — entered via a `BlockedOnQuota` entry and
/// left via the next non-blocked entry — contributes `end - start`. A
/// still-blocked tail with no later non-blocked entry contributes zero rather
/// than guessing an end. Entries whose timestamps go backwards are ignored
/// entirely, as if never recorded. The total saturates rather than
/// overflowing.
pub fn blocked_ms(history: &[(u64, RunState)]) -> u64 {
    let mut total: u64 = 0;
    let mut blocked_start: Option<u64> = None;
    let mut frontier: Option<u64> = None;

    for entry in history {
        let ts = entry.0;
        if frontier.is_some_and(|prev| ts < prev) {
            continue;
        }
        frontier = Some(ts);
        if entry.1.is_blocked() {
            if blocked_start.is_none() {
                blocked_start = Some(ts);
            }
        } else if let Some(start) = blocked_start.take() {
            total = total.saturating_add(ts.saturating_sub(start));
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QuotaBlocked from Queued is Illegal.
    #[test]
    fn quota_blocked_from_queued_is_illegal() {
        let s = RunState::Queued;
        let err = step(&s, Event::QuotaBlocked, 10, "b", None);
        assert!(matches!(err, Err(TransitionError::Illegal { .. })));
    }

    /// A second QuotaBlocked while already blocked is Illegal, not a silent no-op.
    #[test]
    fn double_block_is_illegal() {
        let running = RunState::Running { started_ms: 0 };
        let blocked = step(&running, Event::QuotaBlocked, 10, "b", None);
        let blocked = blocked.unwrap();
        assert!(blocked.is_blocked());
        let again = step(&blocked, Event::QuotaBlocked, 20, "b", None);
        assert!(matches!(again, Err(TransitionError::Illegal { .. })));
    }

    /// An event on Done reports Terminal not Illegal.
    #[test]
    fn event_on_done_reports_terminal_not_illegal() {
        let done = RunState::Done { passed: true };
        // QuotaBlocked from Done would be Illegal on a live run; terminal wins.
        let err = step(&done, Event::QuotaBlocked, 10, "b", None);
        assert!(matches!(err, Err(TransitionError::Terminal { .. })));
        // Even a structurally meaningless event still reports Terminal.
        let err2 = step(&done, Event::QuotaReset, 10, "b", None);
        assert!(matches!(err2, Err(TransitionError::Terminal { .. })));
        // Failed and Cancelled are terminal too.
        let failed = RunState::Failed {
            reason: String::from("x"),
        };
        assert!(matches!(
            step(&failed, Event::VerifyStarted, 10, "b", None),
            Err(TransitionError::Terminal { .. })
        ));
        let cancelled = RunState::Cancelled;
        assert!(matches!(
            step(&cancelled, Event::Dispatched { at_ms: 5 }, 10, "b", None),
            Err(TransitionError::Terminal { .. })
        ));
    }

    /// Cancel works while blocked.
    #[test]
    fn cancel_works_while_blocked() {
        let blocked = RunState::BlockedOnQuota {
            bucket: String::from("b"),
            since_ms: 7,
            resume_at_ms: Some(100),
        };
        let next = step(&blocked, Event::CancelledNow, 8, "b", None).unwrap();
        assert_eq!(next, RunState::Cancelled);
        // Cancel is legal from every non-terminal state.
        for s in [
            RunState::Queued,
            RunState::Running { started_ms: 1 },
            RunState::Verifying,
        ] {
            assert_eq!(
                step(&s, Event::CancelledNow, 9, "b", None).unwrap(),
                RunState::Cancelled
            );
        }
    }

    /// QuotaReset sets started_ms to now.
    #[test]
    fn quota_reset_sets_started_ms_to_now() {
        let blocked = RunState::BlockedOnQuota {
            bucket: String::from("b"),
            since_ms: 10,
            resume_at_ms: Some(50),
        };
        let next = step(&blocked, Event::QuotaReset, 77, "b", None).unwrap();
        assert_eq!(next, RunState::Running { started_ms: 77 });
        // The run resumes, it does not rewind to the old start.
        assert_ne!(next, RunState::Running { started_ms: 10 });
        // QuotaReset from Running is Illegal.
        let running = RunState::Running { started_ms: 5 };
        assert!(matches!(
            step(&running, Event::QuotaReset, 77, "b", None),
            Err(TransitionError::Illegal { .. })
        ));
    }

    /// Resumable respects a stated reset and falls back to the window for None.
    #[test]
    fn resumable_respects_a_stated_reset_and_falls_back_to_the_window_for_none() {
        let stated = RunState::BlockedOnQuota {
            bucket: String::from("b"),
            since_ms: 10,
            resume_at_ms: Some(50),
        };
        assert!(!resumable(&stated, 49, 0));
        assert!(resumable(&stated, 50, 1_000));
        assert!(resumable(&stated, 99, 1_000));

        let unstated = RunState::BlockedOnQuota {
            bucket: String::from("b"),
            since_ms: 10,
            resume_at_ms: None,
        };
        assert!(!resumable(&unstated, 19, 10));
        assert!(resumable(&unstated, 20, 10));
        // Non-blocked runs are never resumable.
        assert!(!resumable(&RunState::Running { started_ms: 1 }, 10_000, 0));
        assert!(!resumable(&RunState::Queued, 10_000, 0));
    }

    /// blocked_ms ignores a still-blocked tail.
    #[test]
    fn blocked_ms_ignores_a_still_blocked_tail() {
        let blocked = RunState::BlockedOnQuota {
            bucket: String::from("b"),
            since_ms: 10,
            resume_at_ms: None,
        };
        let history = [
            (0, RunState::Queued),
            (10, blocked.clone()),
            (30, RunState::Running { started_ms: 30 }),
            (40, blocked),
        ];
        // Only the closed 10..30 segment counts; the open tail counts zero.
        assert_eq!(blocked_ms(&history), 20);
        // A history that never unblocks counts zero.
        let open = [
            (0, RunState::Running { started_ms: 0 }),
            (
                10,
                RunState::BlockedOnQuota {
                    bucket: String::from("b"),
                    since_ms: 10,
                    resume_at_ms: None,
                },
            ),
        ];
        assert_eq!(blocked_ms(&open), 0);
    }

    /// Backwards timestamps do not underflow.
    #[test]
    fn backwards_timestamps_do_not_underflow() {
        let blocked = RunState::BlockedOnQuota {
            bucket: String::from("b"),
            since_ms: 30,
            resume_at_ms: None,
        };
        // End before start: ignored, not wrapped.
        let history = [
            (0, RunState::Queued),
            (30, blocked),
            (20, RunState::Running { started_ms: 20 }),
        ];
        assert_eq!(blocked_ms(&history), 0);
        // resumable with now before since is false, not wrapped.
        let early = RunState::BlockedOnQuota {
            bucket: String::from("b"),
            since_ms: 100,
            resume_at_ms: None,
        };
        assert!(!resumable(&early, 50, 0));
        // Saturating total: adversarial huge segments do not overflow.
        let big = [
            (
                0,
                RunState::BlockedOnQuota {
                    bucket: String::from("b"),
                    since_ms: 0,
                    resume_at_ms: None,
                },
            ),
            (
                u64::MAX,
                RunState::Running {
                    started_ms: u64::MAX,
                },
            ),
            (
                0,
                RunState::BlockedOnQuota {
                    bucket: String::from("b"),
                    since_ms: 0,
                    resume_at_ms: None,
                },
            ),
            (u64::MAX, RunState::Done { passed: true }),
        ];
        assert_eq!(blocked_ms(&big), u64::MAX);
    }

    /// Ordinary dispatch / verify / done path plus is_terminal / is_blocked.
    #[test]
    fn ordinary_lifecycle_moves_forward() {
        let queued = RunState::Queued;
        assert!(!queued.is_terminal());
        assert!(!queued.is_blocked());
        let running = step(&queued, Event::Dispatched { at_ms: 5 }, 5, "b", None).unwrap();
        assert_eq!(running, RunState::Running { started_ms: 5 });
        let verifying = step(&running, Event::VerifyStarted, 6, "b", None).unwrap();
        assert_eq!(verifying, RunState::Verifying);
        let done = step(&verifying, Event::Verified { passed: true }, 7, "b", None).unwrap();
        assert_eq!(done, RunState::Done { passed: true });
        assert!(done.is_terminal());
        let blocked = RunState::BlockedOnQuota {
            bucket: String::from("b"),
            since_ms: 1,
            resume_at_ms: None,
        };
        assert!(blocked.is_blocked());
        assert!(!blocked.is_terminal());
    }
}

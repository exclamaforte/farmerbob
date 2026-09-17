//! Agent liveness, held apart from run lifecycle.
//!
//! A run has a lifecycle the harness *owns*: queued, dispatched, verifying, scored.
//! An agent has a liveness the harness only *observes*: working, idle, blocked, gone.
//! Conflating them is how a harness lies to itself — a frozen log gets read as a dead
//! agent when the launcher is merely buffering output, and a process-name match reports
//! seven live agents when one is running.
//!
//! The rule that prevents it: every belief carries the authority it came from, and a
//! weaker source may never override a stronger one. Ranked:
//!
//! 1. [`Authority::Lifecycle`] — the agent or its wrapper said so. Authoritative.
//! 2. [`Authority::Cgroup`] — the run's own cgroup has live pids and rising CPU.
//!    Strong, but it cannot say what the agent is *doing*, only that it is doing
//!    something.
//! 3. [`Authority::Output`] — the log grew recently. Weak, and wrong for any launcher
//!    that buffers.
//!
//! When the best available source cannot decide, the answer is [`Liveness::Unknown`],
//! which is a real state and not an error. A harness that guesses `Gone` because a log
//! went quiet will kill healthy work.
//!
//! Everything here is pure logic: no I/O, no clock reads, no process inspection. Time
//! enters only as the `at_ms` field of an [`Observation`] and the `now_ms` argument of
//! [`decay`], so a recorded sequence of observations can be replayed deterministically.
//!
//! What observation alone does not say is when a run should be *ended*. That is
//! [`cut`]: a decision that rests on strong evidence of no progress, refuses to decide
//! when the evidence is absent or impossible, and reads a quiet log under a live
//! cgroup as the buffering launcher it usually is — never as a stall. "No evidence of
//! progress" and "evidence of no progress" must not produce the same decision.

/// How much weight a source carries. Greater is stronger.
///
/// The explicit discriminants are the ranking: a stronger authority wins over a weaker
/// one regardless of arrival order, and the [`Ord`](std::cmp::Ord) derivation is what
/// lets [`Tracker`] compare two sources with `>=` instead of matching on names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Authority {
    /// The log grew recently. Weakest, and wrong for any launcher that buffers: a
    /// frozen log says nothing about whether the agent is alive.
    Output = 1,
    /// The run's own cgroup has live pids and rising CPU. Strong, but it can only say
    /// that *something* is running, never what it is doing.
    Cgroup = 2,
    /// The agent or its wrapper reported its own state. Authoritative.
    Lifecycle = 3,
}

/// What the harness believes an agent is doing.
///
/// [`Liveness::Unknown`] is a first-class state, not a failure: it means the best
/// available source could not decide. Callers must not treat it as `Gone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// Making progress.
    Working,
    /// Alive, not making progress.
    Idle,
    /// Alive, waiting on something outside itself.
    Blocked,
    /// No longer running.
    Gone,
    /// The best available source could not decide.
    Unknown,
}

/// One observation about one run, from one source.
///
/// Observations are immutable facts about what a source said; they carry no notion of
/// whether they will win. That is [`Tracker`]'s job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// The source that produced this observation. Its rank decides precedence.
    pub authority: Authority,
    /// What that source claimed.
    pub liveness: Liveness,
    /// Unix milliseconds at which the source made this claim. Later observations from
    /// the *same* authority supersede earlier ones.
    pub at_ms: u64,
    /// Human-readable grounds for the claim, preserved verbatim into the belief so a
    /// reader can see what produced it.
    pub evidence: String,
}

/// What the harness currently believes, and on what grounds.
///
/// `authority` is [`None`] exactly when nothing is held: before any observation, and
/// after [`decay`] has expired one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Belief {
    /// What is believed.
    pub liveness: Liveness,
    /// The authority that produced it, [`None`] when nothing is held.
    pub authority: Option<Authority>,
    /// Unix milliseconds of the winning observation.
    pub at_ms: u64,
    /// The winning observation's evidence, verbatim.
    pub evidence: String,
}

impl Belief {
    /// The belief held before any observation, and the shape [`decay`] returns: no
    /// liveness, no authority, no grounds.
    fn empty() -> Self {
        Self {
            liveness: Liveness::Unknown,
            authority: None,
            at_ms: 0,
            evidence: String::new(),
        }
    }
}

/// How many authorities there are. The tracker keeps one slot each, so memory is
/// bounded no matter how long a run is observed.
const SLOTS: usize = 3;

/// Folds observations into a single belief, ranked by [`Authority`].
///
/// At most one observation is retained per authority — the newest one that was
/// accepted — so the tracker's size is fixed and the belief is always "the strongest
/// source that has spoken, and the last thing it said".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tracker {
    /// The retained observation for each authority, indexed by [`slot`].
    held: [Option<Observation>; SLOTS],
}

impl Tracker {
    /// Creates a tracker holding nothing: [`Tracker::belief`] is
    /// [`Liveness::Unknown`] with no authority.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one observation into the belief.
    ///
    /// An observation from a source weaker than the one currently holding the belief is
    /// *ignored* — not merely masked: it is not retained, so a later
    /// [`Tracker::release`] of the stronger source does not resurrect it. That keeps
    /// [`Tracker::would_accept`] honest: a caller that skips gathering weak evidence
    /// ends up in the same state as one that gathered it anyway.
    ///
    /// A stronger source always wins, whatever the timestamps. Two observations from
    /// the same authority resolve by `at_ms`, latest winning; equal timestamps keep the
    /// incumbent in full, because an arbitrary tiebreak would invent information.
    pub fn observe(&mut self, o: Observation) {
        if !self.would_accept(o.authority) {
            return;
        }
        let slot = &mut self.held[slot(o.authority)];
        let supersedes = match slot.as_ref() {
            Some(current) => o.at_ms > current.at_ms,
            None => true,
        };
        if supersedes {
            *slot = Some(o);
        }
    }

    /// The current belief.
    ///
    /// [`Liveness::Unknown`] with `authority: None` before any observation.
    pub fn belief(&self) -> Belief {
        match self.strongest() {
            Some(o) => Belief {
                liveness: o.liveness,
                authority: Some(o.authority),
                at_ms: o.at_ms,
                evidence: o.evidence.clone(),
            },
            None => Belief::empty(),
        }
    }

    /// Drops every observation from `authority` and recomputes the belief from what
    /// remains.
    ///
    /// Used when a source stops being trustworthy — a lifecycle integration is
    /// uninstalled, say, and its reports must stop counting. The belief may fall to a
    /// weaker retained source, or to [`Liveness::Unknown`] if none remains. Releasing
    /// an authority that never reported changes nothing.
    pub fn release(&mut self, authority: Authority) {
        self.held[slot(authority)] = None;
    }

    /// Whether an observation from `authority` would be folded in right now.
    ///
    /// True when nothing is held, and true for any authority at least as strong as the
    /// one holding the belief. Exposed so a caller can skip gathering evidence that
    /// cannot win — reading a cgroup is not free.
    pub fn would_accept(&self, authority: Authority) -> bool {
        match self.strongest() {
            Some(current) => authority >= current.authority,
            None => true,
        }
    }

    /// The retained observation from the strongest authority, if any is held.
    fn strongest(&self) -> Option<&Observation> {
        self.held.iter().flatten().max_by_key(|o| o.authority)
    }
}

/// Ages a belief: one older than `max_age_ms` decays to [`Liveness::Unknown`] rather
/// than persisting.
///
/// A stale claim that an agent is `Working` is worse than admitting ignorance, because
/// the caller acts on it. Decay clears the authority, the timestamp and the evidence
/// along with the liveness — a belief with no source has no grounds.
///
/// It never *strengthens* a belief: within the age the belief is returned unchanged,
/// and an already-`Unknown` belief stays `Unknown`. A belief timestamped in the future
/// relative to `now_ms` has age zero, so it survives.
pub fn decay(b: &Belief, now_ms: u64, max_age_ms: u64) -> Belief {
    if now_ms.saturating_sub(b.at_ms) > max_age_ms {
        return Belief::empty();
    }
    b.clone()
}

/// The slot in [`Tracker::held`] belonging to `authority`. A match, not a cast on the
/// discriminant, so no arithmetic can go out of bounds.
fn slot(authority: Authority) -> usize {
    match authority {
        Authority::Output => 0,
        Authority::Cgroup => 1,
        Authority::Lifecycle => 2,
    }
}

// --- the cut decision --------------------------------------------------------
//
// Everything above observes; what follows decides. The rule it encodes is the one the
// module exists for: a cut may rest on strong evidence of NO progress — a cgroup (or
// better) reporting idle or blocked past a stated budget — and never on the absence of
// progress reports. A silent log is what a buffering launcher produces while working.

/// Limits a caller supplies. There is no default: a budget is a policy decision and the
/// caller owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutBudget {
    /// Hard wall-clock ceiling. A run past this is cut whatever it is doing.
    ///
    /// Zero is a real ceiling, not "no limit": every run, even one of age zero, is at
    /// or past it. A caller that wants no ceiling passes [`u64::MAX`]; silently
    /// reinterpreting zero as unlimited is how a config typo disables a safety limit.
    pub max_total_ms: u64,
    /// How long a run may be believed `Idle` before it is cut.
    ///
    /// Zero cuts on the first `Idle` belief, however fresh it is.
    pub max_idle_ms: u64,
    /// How long a run may be believed `Blocked` before it is cut. Separate from
    /// `max_idle_ms` deliberately: blocked-on-network is ordinary and often
    /// longer-lived than an idle loop, and collapsing them forces one number to serve
    /// two behaviours.
    pub max_blocked_ms: u64,
}

/// Whether to end a run, and on what grounds.
///
/// NOT an exhaustive account of every reason a run can end. It is the set [`cut`]
/// decides; a caller may end a run for reasons this type knows nothing about — an
/// operator's kill, a provider refusal, a machine reboot. [`Cut::Keep`] in particular
/// does NOT mean "this run is healthy": it means this function found no ground to end
/// it, which is a weaker claim, and callers must not log or relay it as health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cut {
    /// Let it run. Carries why, so a caller can log a decision it did not take.
    Keep {
        /// Why the run was let run, for the caller's log.
        because: String,
    },
    /// End it. `because` is written for a human reading an incident later.
    Cut {
        /// Why the run should end, for the incident record.
        because: String,
    },
    /// Refuse to decide. The evidence does not support ending a run, and ending one on
    /// insufficient evidence is worse than waiting.
    Insufficient {
        /// What evidence is missing, so the caller can go and gather it.
        missing: String,
    },
}

/// Decide whether a run should be cut.
///
/// `elapsed_ms` is the run's age. `now_ms` is the current instant, used to age the
/// belief.
///
/// Arbitration order, each step decisive:
///
/// 1. An impossible belief refuses to decide — [`Cut::Insufficient`], never [`Cut::Cut`]:
///    no authority (`Belief::authority` is `None`, so nothing has been observed), a
///    belief timestamped after `now_ms`, or a belief older than the run itself. Such a
///    belief means the caller's view of the run is broken, and a decision issued from a
///    broken view may be about the wrong run; it also means no duration — including the
///    idle hold — can be computed without wrapping. The refusal says what is missing
///    instead of saturating silently.
/// 2. The hard ceiling: `elapsed_ms` at or past `CutBudget::max_total_ms` cuts whatever
///    the belief says. The wall outranks apparent progress because apparent progress is
///    a *claim* — one source said so, and every source in [`Authority`] can be wrong or
///    stale — while `elapsed_ms` is not a claim at all. The ceiling is the caller's
///    promise that no run outlives the budget, and a promise may not depend on a
///    belief.
/// 3. [`Liveness::Gone`] cuts, with grounds that distinguish a finished run from a
///    stalled one: there is nothing to kill.
/// 4. [`Liveness::Idle`] and [`Liveness::Blocked`] cut only once held past their own
///    budget — and only on an authority of at least [`Authority::Cgroup`]. A cut may
///    not rest on [`Authority::Output`]: a frozen log is the weakest evidence there is,
///    and an idle claim from it is kept, with the reason logged.
/// 5. [`Liveness::Working`] is kept, whatever its age and whatever its authority. A
///    `Working` belief from [`Authority::Cgroup`] with a long-silent log is the
///    buffering launcher, not a stall. Callers who want stale beliefs to expire
///    compose [`decay`] before calling this; this function does not second-guess a
///    standing claim of work.
/// 6. [`Liveness::Unknown`] refuses to decide: it is a real state, and callers must
///    not read it as `Gone`.
///
/// The limits are inclusive — `elapsed_ms == max_total_ms` is at the ceiling, and a
/// belief held exactly `max_idle_ms` is at the idle limit — because that is what the
/// zero cases force and zero is not special-cased: with `max_total_ms: 0` every run,
/// even one of age zero, is at or past its ceiling, and `max_idle_ms: 0` cuts on the
/// first `Idle` belief. All comparisons are made without arithmetic that can overflow.
pub fn cut(belief: &Belief, elapsed_ms: u64, now_ms: u64, budget: &CutBudget) -> Cut {
    let authority = match belief.authority {
        Some(a) => a,
        None => {
            return Cut::Insufficient {
                missing: "no observation has ever been held for this run: there is no \
                          evidence of progress and no evidence of a stall, and the two \
                          must not produce the same decision"
                    .to_string(),
            }
        }
    };

    if belief.at_ms > now_ms {
        return Cut::Insufficient {
            missing: format!(
                "the belief is timestamped in the future: at_ms {} is after now_ms {}, so \
                 the clock or the run identity is broken and no duration can be trusted",
                belief.at_ms, now_ms
            ),
        };
    }

    // Exact, not saturating in effect: the future case returned above.
    let held_ms = now_ms.saturating_sub(belief.at_ms);
    if held_ms > elapsed_ms {
        return Cut::Insufficient {
            missing: format!(
                "the belief is {held_ms} ms old but the run is only {elapsed_ms} ms old: \
                 the observation predates the run it is supposed to describe"
            ),
        };
    }

    if elapsed_ms >= budget.max_total_ms {
        return Cut::Cut {
            because: format!(
                "the run is {elapsed_ms} ms old, at or past the hard ceiling of {} ms; the \
                 ceiling is wall-clock policy and cuts whatever the run appears to be doing",
                budget.max_total_ms
            ),
        };
    }

    match belief.liveness {
        Liveness::Gone => Cut::Cut {
            because: format!(
                "{authority:?} reports the run gone (evidence: {}): it ended on its own, \
                 so there is nothing to kill — this is reaping a finished run, not \
                 cutting a stall",
                belief.evidence
            ),
        },
        Liveness::Working => Cut::Keep {
            because: format!(
                "{authority:?} reports the run working (evidence: {}); the belief is \
                 {held_ms} ms old and the run is within its {} ms ceiling, so there is \
                 no ground to end it — a quiet log under a live report of work is the \
                 buffering launcher, not a stall",
                belief.evidence, budget.max_total_ms
            ),
        },
        Liveness::Idle => {
            if held_ms >= budget.max_idle_ms {
                if authority >= Authority::Cgroup {
                    Cut::Cut {
                        because: format!(
                            "idle for {held_ms} ms, at or past the {} ms idle budget \
                             ({authority:?}; evidence: {}): the strongest standing source \
                             has reported no progress for past the whole budget",
                            budget.max_idle_ms, belief.evidence
                        ),
                    }
                } else {
                    Cut::Keep {
                        because: format!(
                            "idle for {held_ms} ms is past the {} ms idle budget, but the \
                             only source is Output, which this module exists to distrust; \
                             a cut may not rest on it",
                            budget.max_idle_ms
                        ),
                    }
                }
            } else {
                Cut::Keep {
                    because: format!(
                        "idle for {held_ms} ms, within the {} ms idle budget",
                        budget.max_idle_ms
                    ),
                }
            }
        }
        Liveness::Blocked => {
            if held_ms >= budget.max_blocked_ms {
                if authority >= Authority::Cgroup {
                    Cut::Cut {
                        because: format!(
                            "blocked for {held_ms} ms, at or past the {} ms blocked budget \
                             ({authority:?}; evidence: {})",
                            budget.max_blocked_ms, belief.evidence
                        ),
                    }
                } else {
                    Cut::Keep {
                        because: format!(
                            "blocked for {held_ms} ms is past the {} ms blocked budget, but \
                             the only source is Output, which may not end a run",
                            budget.max_blocked_ms
                        ),
                    }
                }
            } else {
                Cut::Keep {
                    because: format!(
                        "blocked for {held_ms} ms, within the {} ms blocked budget; waiting \
                         on the world is ordinary and gets its own budget, separate from \
                         idle",
                        budget.max_blocked_ms
                    ),
                }
            }
        }
        Liveness::Unknown => Cut::Insufficient {
            missing: format!(
                "{authority:?} could not decide what the run is doing (evidence: {}); \
                 Unknown is a real state and must not be treated as Gone, so there is \
                 nothing to cut on",
                belief.evidence
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(authority: Authority, liveness: Liveness, at_ms: u64, evidence: &str) -> Observation {
        Observation {
            authority,
            liveness,
            at_ms,
            evidence: evidence.to_string(),
        }
    }

    // --- precedence -----------------------------------------------------------

    #[test]
    fn nothing_observed_is_unknown_with_no_authority() {
        let belief = Tracker::new().belief();
        assert_eq!(belief.liveness, Liveness::Unknown);
        assert_eq!(belief.authority, None);
    }

    #[test]
    fn late_weak_observation_loses_to_early_strong_one() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(
            Authority::Lifecycle,
            Liveness::Blocked,
            1,
            "wrapper: waiting on lease",
        ));
        tracker.observe(obs(
            Authority::Output,
            Liveness::Working,
            9_000,
            "log grew 40 lines",
        ));
        let belief = tracker.belief();
        assert_eq!(belief.liveness, Liveness::Blocked);
        assert_eq!(belief.authority, Some(Authority::Lifecycle));
        assert_eq!(belief.at_ms, 1);
    }

    #[test]
    fn early_weak_observation_loses_to_late_strong_one() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Output, Liveness::Working, 1, "log grew"));
        tracker.observe(obs(
            Authority::Lifecycle,
            Liveness::Gone,
            2,
            "agent: exit 0",
        ));
        assert_eq!(tracker.belief().liveness, Liveness::Gone);
        assert_eq!(tracker.belief().authority, Some(Authority::Lifecycle));
    }

    #[test]
    fn every_stronger_authority_beats_every_weaker_one_in_either_order() {
        let ranked = [Authority::Output, Authority::Cgroup, Authority::Lifecycle];
        for (strong_index, strong) in ranked.iter().enumerate() {
            for (weak_index, weak) in ranked.iter().enumerate() {
                if weak_index >= strong_index {
                    continue;
                }
                for strong_first in [true, false] {
                    let mut tracker = Tracker::new();
                    let strong_obs = obs(*strong, Liveness::Idle, 10, "strong");
                    let weak_obs = obs(*weak, Liveness::Working, 20, "weak");
                    if strong_first {
                        tracker.observe(strong_obs);
                        tracker.observe(weak_obs);
                    } else {
                        tracker.observe(weak_obs);
                        tracker.observe(strong_obs);
                    }
                    let belief = tracker.belief();
                    assert_eq!(
                        belief.authority,
                        Some(*strong),
                        "{strong:?} must outrank {weak:?}"
                    );
                    assert_eq!(belief.liveness, Liveness::Idle);
                }
            }
        }
    }

    // --- same-authority resolution -------------------------------------------

    #[test]
    fn same_authority_ties_keep_incumbent() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Cgroup, Liveness::Working, 10, "cpu rising"));
        tracker.observe(obs(Authority::Cgroup, Liveness::Gone, 10, "no pids"));
        let belief = tracker.belief();
        assert_eq!(belief.liveness, Liveness::Working);
        assert_eq!(belief.evidence, "cpu rising");
        assert_eq!(belief.at_ms, 10);
    }

    #[test]
    fn same_authority_ties_keep_incumbent_evidence_verbatim() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Output, Liveness::Idle, 7, "  first  "));
        tracker.observe(obs(Authority::Output, Liveness::Idle, 7, "second"));
        assert_eq!(tracker.belief().evidence, "  first  ");
    }

    #[test]
    fn newer_same_authority_supersedes_including_evidence() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Output, Liveness::Working, 10, "log grew"));
        tracker.observe(obs(Authority::Output, Liveness::Gone, 11, "log stopped"));
        let belief = tracker.belief();
        assert_eq!(belief.liveness, Liveness::Gone);
        assert_eq!(belief.at_ms, 11);
        assert_eq!(belief.evidence, "log stopped");
    }

    #[test]
    fn older_same_authority_arriving_late_is_ignored() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Cgroup, Liveness::Working, 100, "cpu rising"));
        tracker.observe(obs(Authority::Cgroup, Liveness::Gone, 50, "stale read"));
        let belief = tracker.belief();
        assert_eq!(belief.liveness, Liveness::Working);
        assert_eq!(belief.at_ms, 100);
    }

    #[test]
    fn a_stronger_source_replaced_later_still_supersedes_by_time() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Lifecycle, Liveness::Working, 5, "start"));
        tracker.observe(obs(
            Authority::Lifecycle,
            Liveness::Blocked,
            6,
            "waiting on lease",
        ));
        tracker.observe(obs(Authority::Lifecycle, Liveness::Blocked, 4, "old"));
        assert_eq!(tracker.belief().evidence, "waiting on lease");
    }

    // --- the motivating cases -------------------------------------------------

    #[test]
    fn empty_cgroup_beats_a_fresh_log_line() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(
            Authority::Output,
            Liveness::Working,
            9_999,
            "log grew one line",
        ));
        tracker.observe(obs(
            Authority::Cgroup,
            Liveness::Gone,
            1,
            "cgroup empty: no pids",
        ));
        let belief = tracker.belief();
        assert_eq!(belief.liveness, Liveness::Gone);
        assert_eq!(belief.authority, Some(Authority::Cgroup));
    }

    #[test]
    fn empty_cgroup_beats_a_fresh_log_line_in_either_order() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Cgroup, Liveness::Gone, 1, "cgroup empty"));
        tracker.observe(obs(Authority::Output, Liveness::Working, 9_999, "log grew"));
        assert_eq!(tracker.belief().liveness, Liveness::Gone);
    }

    #[test]
    fn a_quiet_log_is_unknown_rather_than_gone() {
        // A frozen log is weak evidence for Working and no evidence at all for Gone.
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Output, Liveness::Working, 1, "log grew"));
        let belief = decay(&tracker.belief(), 10_000, 1_000);
        assert_eq!(belief.liveness, Liveness::Unknown);
        assert_ne!(belief.liveness, Liveness::Gone);
        assert_eq!(belief.authority, None);
    }

    // --- release --------------------------------------------------------------

    #[test]
    fn release_can_drop_the_belief_to_unknown() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(
            Authority::Lifecycle,
            Liveness::Working,
            5,
            "agent: running",
        ));
        tracker.release(Authority::Lifecycle);
        let belief = tracker.belief();
        assert_eq!(belief.liveness, Liveness::Unknown);
        assert_eq!(belief.authority, None);
    }

    #[test]
    fn release_recomputes_from_the_weaker_remainder() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Output, Liveness::Working, 5, "log grew"));
        tracker.observe(obs(
            Authority::Lifecycle,
            Liveness::Gone,
            6,
            "agent: exited",
        ));
        tracker.release(Authority::Lifecycle);
        let belief = tracker.belief();
        assert_eq!(belief.liveness, Liveness::Working);
        assert_eq!(belief.authority, Some(Authority::Output));
        assert_eq!(belief.evidence, "log grew");
    }

    #[test]
    fn release_of_an_authority_that_never_reported_changes_nothing() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Cgroup, Liveness::Idle, 5, "cpu flat"));
        let before = tracker.belief();
        tracker.release(Authority::Lifecycle);
        tracker.release(Authority::Output);
        assert_eq!(tracker.belief(), before);
    }

    #[test]
    fn release_then_reobserve_the_same_authority_starts_fresh() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Cgroup, Liveness::Gone, 100, "empty"));
        tracker.release(Authority::Cgroup);
        tracker.observe(obs(
            Authority::Cgroup,
            Liveness::Working,
            50,
            "older but the only claim",
        ));
        assert_eq!(tracker.belief().liveness, Liveness::Working);
    }

    #[test]
    fn an_ignored_weak_observation_does_not_resurface_after_release() {
        // `observe` ignores weaker sources outright, so skipping such an observation
        // (as would_exist invites) and feeding it anyway must agree.
        let mut tracker = Tracker::new();
        tracker.observe(obs(
            Authority::Lifecycle,
            Liveness::Working,
            1,
            "agent: running",
        ));
        tracker.observe(obs(Authority::Cgroup, Liveness::Gone, 2, "cgroup empty"));
        assert_eq!(tracker.belief().liveness, Liveness::Working);
        tracker.release(Authority::Lifecycle);
        assert_eq!(tracker.belief().authority, None);
    }

    // --- would_accept ---------------------------------------------------------

    #[test]
    fn would_accept_is_true_before_any_observation() {
        let tracker = Tracker::new();
        for authority in [Authority::Output, Authority::Cgroup, Authority::Lifecycle] {
            assert!(
                tracker.would_accept(authority),
                "{authority:?} must be accepted when nothing is held"
            );
        }
    }

    #[test]
    fn would_accept_admits_equal_and_stronger_and_rejects_weaker() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Cgroup, Liveness::Working, 1, "cpu rising"));
        assert!(tracker.would_accept(Authority::Cgroup));
        assert!(tracker.would_accept(Authority::Lifecycle));
        assert!(!tracker.would_accept(Authority::Output));
    }

    #[test]
    fn would_accept_widens_after_the_stronger_source_is_released() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Output, Liveness::Working, 1, "log grew"));
        tracker.observe(obs(Authority::Lifecycle, Liveness::Idle, 2, "agent: idle"));
        assert!(!tracker.would_accept(Authority::Cgroup));
        tracker.release(Authority::Lifecycle);
        assert!(tracker.would_accept(Authority::Cgroup));
        assert!(tracker.would_accept(Authority::Output));
    }

    #[test]
    fn a_rejected_observation_leaves_the_belief_untouched() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Lifecycle, Liveness::Blocked, 1, "waiting"));
        let before = tracker.belief();
        for rejected in [Authority::Output, Authority::Cgroup] {
            assert!(!tracker.would_accept(rejected));
            let mut probe = tracker.clone();
            probe.observe(obs(rejected, Liveness::Gone, 500, "probe"));
            assert_eq!(
                probe.belief(),
                before,
                "{rejected:?} must not override Lifecycle"
            );
        }
    }

    #[test]
    fn an_accepted_observation_of_equal_authority_does_change_the_belief() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Lifecycle, Liveness::Blocked, 1, "waiting"));
        assert!(tracker.would_accept(Authority::Lifecycle));
        tracker.observe(obs(Authority::Lifecycle, Liveness::Working, 2, "resumed"));
        assert_eq!(tracker.belief().liveness, Liveness::Working);
    }

    // --- decay ----------------------------------------------------------------

    fn working_belief(at_ms: u64) -> Belief {
        Belief {
            liveness: Liveness::Working,
            authority: Some(Authority::Output),
            at_ms,
            evidence: "log grew".to_string(),
        }
    }

    #[test]
    fn decay_past_the_age_yields_unknown() {
        let decayed = decay(&working_belief(1_000), 2_000, 500);
        assert_eq!(decayed.liveness, Liveness::Unknown);
        assert_eq!(decayed.authority, None);
    }

    #[test]
    fn decay_within_the_age_returns_the_belief_unchanged() {
        let belief = working_belief(1_000);
        assert_eq!(decay(&belief, 1_400, 500), belief);
    }

    #[test]
    fn decay_at_exactly_the_age_keeps_the_belief() {
        // "Older than max_age_ms" is strict: an age of exactly max_age_ms is fresh.
        let belief = working_belief(1_000);
        assert_eq!(decay(&belief, 1_500, 500), belief);
        assert_eq!(decay(&belief, 1_501, 500).liveness, Liveness::Unknown);
    }

    #[test]
    fn decay_never_strengthens_a_belief() {
        let unknown = Belief::empty();
        assert_eq!(decay(&unknown, 5_000, 1), unknown);
        let stale_gone = Belief {
            liveness: Liveness::Gone,
            authority: Some(Authority::Lifecycle),
            at_ms: 10,
            evidence: "exited".to_string(),
        };
        let decayed = decay(&stale_gone, 10_000, 100);
        assert_eq!(decayed.liveness, Liveness::Unknown);
        assert_eq!(decayed.authority, None);
    }

    #[test]
    fn decayed_belief_carries_no_grounds() {
        let decayed = decay(&working_belief(1_000), 9_999, 10);
        assert_eq!(decayed.evidence, "");
        assert_eq!(decayed.at_ms, 0);
    }

    #[test]
    fn a_belief_timestamped_in_the_future_survives_decay() {
        let belief = working_belief(5_000);
        assert_eq!(decay(&belief, 1_000, 0), belief);
    }

    #[test]
    fn decay_survives_extreme_timestamps() {
        assert_eq!(
            decay(&working_belief(0), u64::MAX, 0).liveness,
            Liveness::Unknown
        );
        // An unbounded max_age never expires anything, not even an age of u64::MAX.
        let belief = working_belief(0);
        assert_eq!(decay(&belief, u64::MAX, u64::MAX), belief);
        let belief = working_belief(u64::MAX);
        assert_eq!(decay(&belief, 0, u64::MAX), belief);
    }

    #[test]
    fn authorities_rank_output_below_cgroup_below_lifecycle() {
        assert!(Authority::Output < Authority::Cgroup);
        assert!(Authority::Cgroup < Authority::Lifecycle);
        assert!(Authority::Output < Authority::Lifecycle);
    }

    #[test]
    fn decay_does_not_mutate_the_tracker() {
        // Decay is a pure function over a belief: the tracker keeps holding its source,
        // so a weak observation is still refused after the caller's view has expired.
        let mut tracker = Tracker::new();
        tracker.observe(obs(
            Authority::Lifecycle,
            Liveness::Working,
            1,
            "agent: running",
        ));
        let aged = decay(&tracker.belief(), 10_000, 100);
        assert_eq!(aged.authority, None);
        assert!(!tracker.would_accept(Authority::Output));
        tracker.observe(obs(
            Authority::Output,
            Liveness::Working,
            10_000,
            "log grew",
        ));
        assert_eq!(tracker.belief().authority, Some(Authority::Lifecycle));
    }

    #[test]
    fn releasing_every_authority_leaves_nothing() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(Authority::Output, Liveness::Working, 1, "log"));
        tracker.observe(obs(Authority::Cgroup, Liveness::Idle, 2, "cpu flat"));
        tracker.observe(obs(Authority::Lifecycle, Liveness::Blocked, 3, "waiting"));
        for authority in [Authority::Output, Authority::Cgroup, Authority::Lifecycle] {
            assert_ne!(tracker.belief().authority, None);
            tracker.release(authority);
        }
        let belief = tracker.belief();
        assert_eq!(belief.liveness, Liveness::Unknown);
        assert_eq!(belief.authority, None);
        assert!(tracker.would_accept(Authority::Output));
    }

    // --- evidence -------------------------------------------------------------

    #[test]
    fn evidence_survives_to_the_belief() {
        let mut tracker = Tracker::new();
        let evidence = "cgroup farmerbob/run-7: 3 pids, cpu 12.4s -> 13.1s";
        tracker.observe(obs(Authority::Cgroup, Liveness::Working, 42, evidence));
        assert_eq!(tracker.belief().evidence, evidence);
    }

    #[test]
    fn evidence_is_carried_verbatim_including_whitespace_and_unicode() {
        let mut tracker = Tracker::new();
        let evidence = "  leading\n\ttab ✓ içi  ";
        tracker.observe(obs(Authority::Lifecycle, Liveness::Blocked, 3, evidence));
        assert_eq!(tracker.belief().evidence, evidence);
    }

    #[test]
    fn evidence_follows_the_winning_source_not_the_loudest() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(
            Authority::Output,
            Liveness::Working,
            9_000,
            "noisy log",
        ));
        tracker.observe(obs(Authority::Lifecycle, Liveness::Idle, 1, "agent: idle"));
        assert_eq!(tracker.belief().evidence, "agent: idle");
    }

    // --- Unknown is a state ----------------------------------------------------

    #[test]
    fn an_unknown_report_from_the_strongest_source_is_unknown_not_absent() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(
            Authority::Lifecycle,
            Liveness::Unknown,
            5,
            "wrapper: no idea",
        ));
        let belief = tracker.belief();
        assert_eq!(belief.liveness, Liveness::Unknown);
        assert_eq!(belief.authority, Some(Authority::Lifecycle));
        assert_eq!(belief.evidence, "wrapper: no idea");
    }

    #[test]
    fn an_unknown_report_still_outranks_a_weaker_claim() {
        let mut tracker = Tracker::new();
        tracker.observe(obs(
            Authority::Lifecycle,
            Liveness::Unknown,
            5,
            "wrapper: no idea",
        ));
        tracker.observe(obs(Authority::Output, Liveness::Working, 6, "log grew"));
        assert_eq!(tracker.belief().liveness, Liveness::Unknown);
        assert_eq!(tracker.belief().authority, Some(Authority::Lifecycle));
    }

    // --- construction ---------------------------------------------------------

    #[test]
    fn default_matches_new() {
        assert_eq!(Tracker::default(), Tracker::new());
        assert_eq!(Tracker::default().belief(), Tracker::new().belief());
    }

    #[test]
    fn arrival_order_does_not_matter_for_the_same_authority() {
        // Observations need not arrive in timestamp order; the newest by at_ms wins.
        let mut forward = Tracker::new();
        forward.observe(obs(Authority::Output, Liveness::Working, 1, "first"));
        forward.observe(obs(Authority::Output, Liveness::Gone, 5, "last"));

        let mut backward = Tracker::new();
        backward.observe(obs(Authority::Output, Liveness::Gone, 5, "last"));
        backward.observe(obs(Authority::Output, Liveness::Working, 1, "first"));

        assert_eq!(forward, backward);
        assert_eq!(forward.belief().liveness, Liveness::Gone);
        assert_eq!(forward.belief().evidence, "last");
    }
}

#[cfg(test)]
mod cut_tests {
    use super::*;

    /// A hand-built belief, with evidence a source could have logged.
    fn belief(liveness: Liveness, authority: Option<Authority>, at_ms: u64) -> Belief {
        Belief {
            liveness,
            authority,
            at_ms,
            evidence: format!("{liveness:?} per {authority:?} at {at_ms}"),
        }
    }

    fn budget(max_total_ms: u64, max_idle_ms: u64, max_blocked_ms: u64) -> CutBudget {
        CutBudget {
            max_total_ms,
            max_idle_ms,
            max_blocked_ms,
        }
    }

    const NOW: u64 = 10_000;

    /// Roomy enough that nothing times out unless a test means it to.
    fn roomy() -> CutBudget {
        budget(100_000, 5_000, 20_000)
    }

    fn reason_of(decision: Cut) -> String {
        match decision {
            Cut::Keep { because } | Cut::Cut { because } => because,
            Cut::Insufficient { missing } => missing,
        }
    }

    // --- working is kept (clauses 1, 2, 3) ------------------------------------

    #[test]
    fn working_from_cgroup_within_the_ceiling_is_kept() {
        let b = belief(Liveness::Working, Some(Authority::Cgroup), NOW - 100);
        assert!(matches!(
            cut(&b, 1_000, NOW, &roomy()),
            Cut::Keep { .. }
        ));
    }

    #[test]
    fn a_working_belief_from_output_alone_never_cuts_a_run_within_budget() {
        // A weak source may not end a run.
        let b = belief(Liveness::Working, Some(Authority::Output), NOW - 100);
        let decision = cut(&b, 1_000, NOW, &roomy());
        assert!(!matches!(decision, Cut::Cut { .. }));
    }

    #[test]
    fn the_buffering_launcher_silent_log_under_a_live_cgroup_is_kept() {
        // "The log has gone quiet while the CPU climbs" is exactly: a Working claim
        // from the cgroup, standing unchallenged far longer than the idle budget,
        // with no newer observation from any source. That is a keep, not a stall.
        let b = belief(Liveness::Working, Some(Authority::Cgroup), NOW - 10_000);
        assert!(matches!(
            cut(&b, 30_000, NOW, &roomy()),
            Cut::Keep { .. }
        ));
    }

    #[test]
    fn working_is_kept_whatever_the_belief_age_while_the_ceiling_allows() {
        // Staleness is decay's job, composed by the caller; cut does not regrade a
        // standing claim of work into a stall.
        let b = belief(Liveness::Working, Some(Authority::Cgroup), 0);
        assert!(matches!(
            cut(&b, NOW, NOW, &roomy()),
            Cut::Keep { .. }
        ));
    }

    // --- the real stall (clause 4) ----------------------------------------------

    #[test]
    fn the_real_stall_idle_from_cgroup_past_the_budget_cuts() {
        // Held for 6s against a 5s idle budget, within the total ceiling.
        let b = belief(Liveness::Idle, Some(Authority::Cgroup), NOW - 6_000);
        match cut(&b, 7_000, NOW, &roomy()) {
            Cut::Cut { because } => assert!(
                because.contains("6000"),
                "the grounds must name the idle duration: {because}"
            ),
            other => panic!("expected Cut, got {other:?}"),
        }
    }

    #[test]
    fn idle_within_the_budget_is_kept() {
        let b = belief(Liveness::Idle, Some(Authority::Cgroup), NOW - 4_999);
        assert!(matches!(
            cut(&b, 5_000, NOW, &roomy()),
            Cut::Keep { .. }
        ));
    }

    // --- blocked has its own budget (clause 5) -----------------------------------

    #[test]
    fn blocked_past_the_idle_budget_but_within_its_own_is_kept() {
        // Held 10s: past max_idle_ms (5s), well under max_blocked_ms (20s).
        let b = belief(Liveness::Blocked, Some(Authority::Cgroup), NOW - 10_000);
        assert!(matches!(
            cut(&b, 30_000, NOW, &roomy()),
            Cut::Keep { .. }
        ));
    }

    #[test]
    fn blocked_past_its_own_budget_cuts() {
        // Held 10s against a 9s blocked budget.
        let b = belief(Liveness::Blocked, Some(Authority::Cgroup), 0);
        assert!(matches!(
            cut(&b, 30_000, NOW, &budget(100_000, 5_000, 9_000)),
            Cut::Cut { .. }
        ));
    }

    #[test]
    fn blocked_within_both_budgets_is_kept() {
        let b = belief(Liveness::Blocked, Some(Authority::Cgroup), NOW - 1_000);
        assert!(matches!(
            cut(&b, 2_000, NOW, &roomy()),
            Cut::Keep { .. }
        ));
    }

    // --- the hard ceiling (clause 6) ----------------------------------------------

    #[test]
    fn the_ceiling_cuts_regardless_of_liveness() {
        for liveness in [
            Liveness::Working,
            Liveness::Idle,
            Liveness::Blocked,
            Liveness::Gone,
            Liveness::Unknown,
        ] {
            let b = belief(liveness, Some(Authority::Cgroup), NOW - 100);
            let decision = cut(&b, 100_001, NOW, &roomy());
            assert!(
                matches!(decision, Cut::Cut { .. }),
                "{liveness:?} must not survive the ceiling"
            );
        }
    }

    #[test]
    fn a_zero_ceiling_is_a_real_limit_not_no_limit() {
        let tight = budget(0, 5_000, 20_000);
        // A newborn run is over a zero ceiling: zero must not be reinterpreted as
        // unlimited, or a config typo silently disarms the wall.
        let newborn = belief(Liveness::Working, Some(Authority::Cgroup), NOW);
        assert!(matches!(
            cut(&newborn, 0, NOW, &tight),
            Cut::Cut { .. }
        ));
        let aged = belief(Liveness::Working, Some(Authority::Cgroup), NOW - 1_000);
        assert!(matches!(
            cut(&aged, 1_000, NOW, &tight),
            Cut::Cut { .. }
        ));
    }

    #[test]
    fn a_zero_idle_budget_cuts_on_the_first_idle_observation() {
        let tight = budget(100_000, 0, 20_000);
        let b = belief(Liveness::Idle, Some(Authority::Cgroup), NOW);
        assert!(matches!(cut(&b, 1_000, NOW, &tight), Cut::Cut { .. }));
    }

    #[test]
    fn zero_elapsed_with_a_healthy_belief_is_kept() {
        // A belief can only describe a run it was formed during, so elapsed 0 pairs
        // with a belief timestamped at now.
        let working = belief(Liveness::Working, Some(Authority::Cgroup), NOW);
        assert!(matches!(cut(&working, 0, NOW, &roomy()), Cut::Keep { .. }));
        let idle = belief(Liveness::Idle, Some(Authority::Cgroup), NOW);
        assert!(matches!(cut(&idle, 0, NOW, &roomy()), Cut::Keep { .. }));
    }

    // --- nothing observed is never a cut (clause 7) --------------------------------

    #[test]
    fn no_authority_is_insufficient_never_a_cut_even_over_the_ceiling() {
        for liveness in [
            Liveness::Working,
            Liveness::Idle,
            Liveness::Blocked,
            Liveness::Gone,
            Liveness::Unknown,
        ] {
            let b = belief(liveness, None, NOW - 100);
            let decision = cut(&b, 100_001, NOW, &roomy());
            assert!(
                !matches!(decision, Cut::Cut { .. }),
                "authority None must never cut, even for {liveness:?}"
            );
            assert!(
                matches!(decision, Cut::Insufficient { .. }),
                "no evidence of progress is not evidence of no progress ({liveness:?})"
            );
        }
    }

    // --- impossible timestamps (clause 8) --------------------------------------------

    #[test]
    fn a_belief_from_the_future_is_insufficient() {
        let b = belief(Liveness::Working, Some(Authority::Cgroup), NOW + 1);
        assert!(matches!(
            cut(&b, 1_000, NOW, &roomy()),
            Cut::Insufficient { .. }
        ));
    }

    #[test]
    fn a_belief_older_than_the_run_itself_is_insufficient() {
        // The run started at NOW - 1_000; the belief predates it.
        let b = belief(Liveness::Working, Some(Authority::Cgroup), NOW - 1_001);
        assert!(matches!(
            cut(&b, 1_000, NOW, &roomy()),
            Cut::Insufficient { .. }
        ));
    }

    #[test]
    fn a_belief_from_exactly_the_run_start_is_usable() {
        let b = belief(Liveness::Working, Some(Authority::Cgroup), NOW - 1_000);
        assert!(matches!(
            cut(&b, 1_000, NOW, &roomy()),
            Cut::Keep { .. }
        ));
    }

    #[test]
    fn saturating_extremes_do_not_panic_or_wrap() {
        let no_limit = budget(u64::MAX, u64::MAX, u64::MAX);
        // A belief at the epoch against now = u64::MAX is older than any run the
        // caller can name, and must be refused, not wrapped into a fresh belief.
        let epoch = belief(Liveness::Working, Some(Authority::Cgroup), 0);
        assert!(matches!(
            cut(&epoch, u64::MAX - 1, u64::MAX, &no_limit),
            Cut::Insufficient { .. }
        ));
        // One millisecond inside the run's life, against unlimited budgets, at the
        // top of the range: keep, with no arithmetic anywhere near a wrap.
        let inside = belief(Liveness::Working, Some(Authority::Cgroup), 2);
        assert!(matches!(
            cut(&inside, u64::MAX - 1, u64::MAX, &no_limit),
            Cut::Keep { .. }
        ));
        // Future-stamped at the top of the range.
        let future = belief(Liveness::Working, Some(Authority::Cgroup), u64::MAX);
        assert!(matches!(
            cut(&future, u64::MAX, u64::MAX - 1, &no_limit),
            Cut::Insufficient { .. }
        ));
    }

    // --- budgets at u64::MAX never cut on duration ------------------------------------

    #[test]
    fn unlimited_budgets_never_cut_on_duration_only_gone_cuts() {
        let no_limit = budget(u64::MAX, u64::MAX, u64::MAX);
        for liveness in [
            Liveness::Working,
            Liveness::Idle,
            Liveness::Blocked,
            Liveness::Unknown,
        ] {
            let b = belief(liveness, Some(Authority::Cgroup), NOW - 100);
            let decision = cut(&b, u64::MAX / 2, NOW, &no_limit);
            assert!(
                !matches!(decision, Cut::Cut { .. }),
                "{liveness:?} must not be cut under unlimited budgets"
            );
        }
        let gone = belief(Liveness::Gone, Some(Authority::Cgroup), NOW - 100);
        assert!(matches!(
            cut(&gone, u64::MAX / 2, NOW, &no_limit),
            Cut::Cut { .. }
        ));
    }

    // --- gone is reaping, not cutting (clause 9) ----------------------------------------

    #[test]
    fn gone_cuts_from_any_authority_because_there_is_nothing_to_kill() {
        for authority in [
            Authority::Output,
            Authority::Cgroup,
            Authority::Lifecycle,
        ] {
            let b = belief(Liveness::Gone, Some(authority), NOW - 100);
            let decision = cut(&b, 1_000, NOW, &roomy());
            assert!(
                matches!(decision, Cut::Cut { .. }),
                "gone per {authority:?} must cut: the run already ended"
            );
        }
    }

    #[test]
    fn a_gone_cut_does_not_read_like_a_stall_cut() {
        let gone = belief(Liveness::Gone, Some(Authority::Cgroup), NOW - 100);
        let stalled = belief(Liveness::Idle, Some(Authority::Cgroup), NOW - 100);
        let gone_cut = cut(&gone, 1_000, NOW, &budget(100_000, 50, 20_000));
        let stall_cut = cut(&stalled, 1_000, NOW, &budget(100_000, 50, 20_000));
        match (gone_cut, stall_cut) {
            (Cut::Cut { because: gone }, Cut::Cut { because: stall }) => {
                assert_ne!(
                    gone, stall,
                    "the grounds for reaping a finished run must be distinguishable \
                     from the grounds for killing a stalled one"
                );
            }
            (a, b) => panic!("both must cut, got {a:?} and {b:?}"),
        }
    }

    // --- unknown is a refusal (clause 10) -------------------------------------------------

    #[test]
    fn unknown_is_insufficient_from_every_authority() {
        for authority in [
            Authority::Output,
            Authority::Cgroup,
            Authority::Lifecycle,
        ] {
            let b = belief(Liveness::Unknown, Some(authority), NOW - 100);
            let decision = cut(&b, 1_000, NOW, &roomy());
            assert!(!matches!(decision, Cut::Cut { .. }));
            assert!(matches!(decision, Cut::Insufficient { .. }));
        }
    }

    // --- a cut rests on cgroup or better ---------------------------------------------------

    #[test]
    fn an_idle_claim_from_output_alone_never_cuts() {
        // The stall cut is pinned to the cgroup; the same principle that keeps a weak
        // source from ending a Working run keeps it from ending an Idle one. Output
        // silence is the buffering launcher, whatever the claim.
        let b = belief(Liveness::Idle, Some(Authority::Output), NOW - 6_000);
        let decision = cut(&b, 30_000, NOW, &roomy());
        assert!(!matches!(decision, Cut::Cut { .. }));
    }

    #[test]
    fn a_blocked_claim_from_output_alone_never_cuts() {
        // Held 10s against a 9s blocked budget: past it, were the source admissible.
        let b = belief(Liveness::Blocked, Some(Authority::Output), 0);
        let decision = cut(&b, 30_000, NOW, &budget(100_000, 5_000, 9_000));
        assert!(!matches!(decision, Cut::Cut { .. }));
    }

    #[test]
    fn an_idle_claim_from_lifecycle_cuts() {
        // Lifecycle outranks Cgroup: what the authoritative wrapper says stands.
        let b = belief(Liveness::Idle, Some(Authority::Lifecycle), NOW - 6_000);
        assert!(matches!(
            cut(&b, 30_000, NOW, &roomy()),
            Cut::Cut { .. }
        ));
    }

    // --- the incident, end to end -----------------------------------------------------------

    #[test]
    fn the_incident_cuts_and_its_healthy_peer_survives() {
        // 2026-09-17: 26s of CPU across 21 minutes, a 0-byte log, no files written.
        // Belief: idle per the cgroup, standing unchallenged for 21 minutes.
        let now = 21 * 60 * 1_000;
        let incident = budget(60 * 60 * 1_000, 120 * 1_000, 10 * 60 * 1_000);
        let stalled = belief(Liveness::Idle, Some(Authority::Cgroup), 0);
        assert!(matches!(
            cut(&stalled, now, now, &incident),
            Cut::Cut { .. }
        ));

        // The peer on the same task: log silent for ten minutes (so the cgroup claim
        // is the newest thing held), CPU climbing two seconds ago.
        let peer = budget(60 * 60 * 1_000, 120 * 1_000, 10 * 60 * 1_000);
        let healthy = belief(Liveness::Working, Some(Authority::Cgroup), now - 2_000);
        assert!(matches!(
            cut(&healthy, now, now, &peer),
            Cut::Keep { .. }
        ));
    }

    // --- every decision carries its reason, across the whole input grid -----------------------

    #[test]
    fn every_decision_carries_a_nonempty_reason() {
        let budgets = [
            roomy(),
            budget(0, 0, 0),
            budget(u64::MAX, u64::MAX, u64::MAX),
            budget(1_000, 500, 2_000),
        ];
        let livenesses = [
            Liveness::Working,
            Liveness::Idle,
            Liveness::Blocked,
            Liveness::Gone,
            Liveness::Unknown,
        ];
        let authorities = [
            None,
            Some(Authority::Output),
            Some(Authority::Cgroup),
            Some(Authority::Lifecycle),
        ];
        for &liveness in &livenesses {
            for &authority in &authorities {
                for at in [NOW - 10_000, NOW - 1, NOW] {
                    for budget in &budgets {
                        for &elapsed in &[0u64, 1_000, 100_001] {
                            let b = belief(liveness, authority, at);
                            let decision = cut(&b, elapsed, NOW, budget);
                            let reason = reason_of(decision);
                            assert!(
                                !reason.is_empty(),
                                "a decision for {liveness:?}/{authority:?} at {at} with \
                                 elapsed {elapsed} carries no reason"
                            );
                        }
                    }
                }
            }
        }
    }
}

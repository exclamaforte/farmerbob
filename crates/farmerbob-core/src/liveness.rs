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

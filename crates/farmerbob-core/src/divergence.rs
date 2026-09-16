//! Convergence-based run cutting: stop runs that stop improving, never on wallclock.
//!
//! A run is cut only when it stops converging on its own error count while
//! also writing no new bytes. Time (`at_ms`) only orders samples; no
//! duration appears in any cut decision.

/// One progress sample taken during a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Timestamp in milliseconds. Used only to order samples; ignored if older
    /// than the newest sample already seen.
    pub at_ms: u64,
    /// Compile/test errors outstanding. Lower is better; 0 means the gate passes.
    pub errors: u32,
    /// Bytes written to the worktree so far. Monotonic.
    pub bytes_written: u64,
}

/// Decision after folding one sample into a [`Tracker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Converging, or not yet enough evidence. Keep going.
    Continue,
    /// No improvement on the best error count for `stalled_checks` consecutive samples
    /// AND no new bytes written. Both conditions are required.
    Cut {
        /// Human-readable explanation of why the run was cut.
        reason: String,
        /// Lowest error count seen up to and including the cutting sample.
        best_errors: u32,
        /// Consecutive non-improving samples observed when the cut fired.
        stalled_checks: u32,
    },
    /// The run reached zero errors. Stop for success, not for failure.
    Done,
}

/// Tunables. A caller that wants a wallclock timeout must not find one here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Consecutive non-improving samples tolerated before cutting. Must be >= 2.
    /// Values below 2 are treated as 2.
    pub patience: u32,
    /// A sample writing at least this many new bytes counts as progress on its own.
    pub min_bytes_progress: u64,
}

impl Policy {
    /// A lenient policy: patience 6, min_bytes_progress 1.
    pub fn lenient() -> Self {
        Self {
            patience: 6,
            min_bytes_progress: 1,
        }
    }
}

/// Folds [`Sample`]s in order and decides whether the run still converges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracker {
    /// Tunables governing when to cut.
    policy: Policy,
    /// Lowest error count accepted so far, if any sample has been seen.
    best: Option<u32>,
    /// Consecutive accepted samples since the last improvement, where an
    /// improvement is either a new error best or enough new bytes.
    stalled_count: u32,
    /// Largest `at_ms` accepted so far, if any sample has been seen.
    newest_at_ms: Option<u64>,
    /// Largest `bytes_written` accepted so far. Tracked as a maximum so an
    /// adversarial non-monotonic sample cannot manufacture future progress.
    last_bytes: u64,
}

impl Default for Tracker {
    /// A tracker with the [`Policy::lenient`] policy and no samples seen.
    fn default() -> Self {
        Self::new(Policy::lenient())
    }
}

impl Tracker {
    /// Create a tracker that cuts according to `policy`.
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            best: None,
            stalled_count: 0,
            newest_at_ms: None,
            last_bytes: 0,
        }
    }

    /// Effective patience, treating values below 2 as 2.
    fn effective_patience(&self) -> u32 {
        self.policy.patience.max(2)
    }

    /// Fold one sample and decide. Later samples with an EARLIER `at_ms` than the
    /// newest seen are ignored entirely.
    pub fn observe(&mut self, s: Sample) -> Decision {
        match self.newest_at_ms {
            Some(n) if s.at_ms < n => return Decision::Continue,
            Some(n) => self.newest_at_ms = Some(n.max(s.at_ms)),
            None => self.newest_at_ms = Some(s.at_ms),
        }

        if s.errors == 0 {
            self.best = Some(0);
            self.stalled_count = 0;
            self.last_bytes = self.last_bytes.max(s.bytes_written);
            return Decision::Done;
        }

        match self.best {
            None => {
                self.best = Some(s.errors);
                self.stalled_count = 0;
                self.last_bytes = self.last_bytes.max(s.bytes_written);
                Decision::Continue
            }
            Some(best) => {
                if s.errors < best {
                    self.best = Some(s.errors);
                    self.stalled_count = 0;
                    self.last_bytes = self.last_bytes.max(s.bytes_written);
                    Decision::Continue
                } else {
                    let delta = s.bytes_written.saturating_sub(self.last_bytes);
                    self.last_bytes = self.last_bytes.max(s.bytes_written);
                    if delta >= self.policy.min_bytes_progress {
                        self.stalled_count = 0;
                        Decision::Continue
                    } else {
                        self.stalled_count = self.stalled_count.saturating_add(1);
                        if self.stalled_count >= self.effective_patience() {
                            Decision::Cut {
                                reason: format!(
                                    "no error improvement and no new bytes for {} consecutive checks",
                                    self.stalled_count
                                ),
                                best_errors: best,
                                stalled_checks: self.stalled_count,
                            }
                        } else {
                            Decision::Continue
                        }
                    }
                }
            }
        }
    }

    /// Lowest error count seen. `None` before any sample.
    pub fn best_errors(&self) -> Option<u32> {
        self.best
    }

    /// Consecutive samples since the last improvement.
    pub fn stalled(&self) -> u32 {
        self.stalled_count
    }
}

/// Did this run regress past its own best? Returns the amount, `None` when it never did.
/// A run whose errors climb above its best and stay there is diverging, not converging.
///
/// Each sample is compared against the best seen BEFORE it; the return value is
/// the largest such excess, or `None` if no sample ever exceeded its running best.
pub fn regression(samples: &[Sample]) -> Option<u32> {
    let mut best: Option<u32> = None;
    let mut worst_excess: Option<u32> = None;
    for s in samples {
        if let Some(b) = best {
            if s.errors > b {
                let excess = s.errors.saturating_sub(b);
                worst_excess = Some(match worst_excess {
                    Some(m) => m.max(excess),
                    None => excess,
                });
            }
            if s.errors < b {
                best = Some(s.errors);
            }
        } else {
            best = Some(s.errors);
        }
    }
    worst_excess
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat errors with fresh bytes on every sample must never cut.
    #[test]
    fn flat_error_count_with_new_bytes_is_not_cut() {
        let mut t = Tracker::new(Policy {
            patience: 3,
            min_bytes_progress: 1,
        });
        let mut saw_cut = false;
        for i in 0u64..20 {
            let d = t.observe(Sample {
                at_ms: i,
                errors: 7,
                bytes_written: i.saturating_add(1),
            });
            if matches!(d, Decision::Cut { .. }) {
                saw_cut = true;
            }
        }
        assert!(!saw_cut);
    }

    /// Flat errors with no new bytes must cut at exactly `patience` stalls.
    #[test]
    fn flat_error_count_with_no_bytes_is_cut_at_exactly_patience() {
        let mut t = Tracker::new(Policy {
            patience: 3,
            min_bytes_progress: 1,
        });
        let base = t.observe(Sample {
            at_ms: 0,
            errors: 7,
            bytes_written: 100,
        });
        assert_eq!(base, Decision::Continue);
        assert_eq!(
            t.observe(Sample {
                at_ms: 1,
                errors: 7,
                bytes_written: 100
            }),
            Decision::Continue
        );
        assert_eq!(
            t.observe(Sample {
                at_ms: 2,
                errors: 7,
                bytes_written: 100
            }),
            Decision::Continue
        );
        let d = t.observe(Sample {
            at_ms: 3,
            errors: 7,
            bytes_written: 100,
        });
        assert!(
            matches!(
                d,
                Decision::Cut {
                    best_errors: 7,
                    stalled_checks: 3,
                    ..
                }
            ),
            "expected Cut with best 7 at stall 3, got {:?}",
            d
        );
    }

    /// A transient spike followed by a new best clears the stall counter.
    #[test]
    fn spike_followed_by_new_best_clears_the_stall() {
        let mut t = Tracker::new(Policy {
            patience: 4,
            min_bytes_progress: 1,
        });
        assert_eq!(
            t.observe(Sample {
                at_ms: 0,
                errors: 10,
                bytes_written: 0
            }),
            Decision::Continue
        );
        assert_eq!(
            t.observe(Sample {
                at_ms: 1,
                errors: 12,
                bytes_written: 0
            }),
            Decision::Continue
        );
        assert_eq!(
            t.observe(Sample {
                at_ms: 2,
                errors: 15,
                bytes_written: 0
            }),
            Decision::Continue
        );
        assert!(t.stalled() > 0);
        assert_eq!(
            t.observe(Sample {
                at_ms: 3,
                errors: 5,
                bytes_written: 0
            }),
            Decision::Continue
        );
        assert_eq!(t.stalled(), 0);
        assert_eq!(t.best_errors(), Some(5));
    }

    /// Zero errors short-circuits to Done even when a cut would otherwise fire.
    #[test]
    fn zero_errors_is_done_even_when_stalled() {
        let mut t = Tracker::new(Policy {
            patience: 2,
            min_bytes_progress: 1,
        });
        assert_eq!(
            t.observe(Sample {
                at_ms: 0,
                errors: 4,
                bytes_written: 0
            }),
            Decision::Continue
        );
        assert_eq!(
            t.observe(Sample {
                at_ms: 1,
                errors: 4,
                bytes_written: 0
            }),
            Decision::Continue
        );
        let d = t.observe(Sample {
            at_ms: 2,
            errors: 0,
            bytes_written: 0,
        });
        assert_eq!(d, Decision::Done);
    }

    /// Patience values below 2 behave as 2.
    #[test]
    fn patience_of_zero_or_one_behaves_as_two() {
        for p in [0u32, 1u32] {
            let mut t = Tracker::new(Policy {
                patience: p,
                min_bytes_progress: 1,
            });
            assert_eq!(
                t.observe(Sample {
                    at_ms: 0,
                    errors: 9,
                    bytes_written: 0
                }),
                Decision::Continue
            );
            assert_eq!(
                t.observe(Sample {
                    at_ms: 1,
                    errors: 9,
                    bytes_written: 0
                }),
                Decision::Continue,
                "patience {} should tolerate one stall",
                p
            );
            match t.observe(Sample {
                at_ms: 2,
                errors: 9,
                bytes_written: 0,
            }) {
                Decision::Cut { stalled_checks, .. } => assert_eq!(stalled_checks, 2),
                Decision::Continue | Decision::Done => {
                    panic!("patience {p}: expected Cut")
                }
            }
        }
    }

    /// Samples older than the newest seen change nothing.
    #[test]
    fn out_of_order_sample_changes_nothing() {
        let mut t = Tracker::new(Policy {
            patience: 5,
            min_bytes_progress: 1,
        });
        assert_eq!(
            t.observe(Sample {
                at_ms: 10,
                errors: 8,
                bytes_written: 50
            }),
            Decision::Continue
        );
        assert_eq!(
            t.observe(Sample {
                at_ms: 11,
                errors: 8,
                bytes_written: 50
            }),
            Decision::Continue
        );
        let best_before = t.best_errors();
        let stalled_before = t.stalled();
        let d = t.observe(Sample {
            at_ms: 5,
            errors: 0,
            bytes_written: 999,
        });
        assert_eq!(d, Decision::Continue);
        assert_eq!(t.best_errors(), best_before);
        assert_eq!(t.stalled(), stalled_before);
    }

    /// Regression reports the largest excess over the running best.
    #[test]
    fn regression_finds_the_largest_excess_over_running_best() {
        let samples = [
            Sample {
                at_ms: 0,
                errors: 10,
                bytes_written: 0,
            },
            Sample {
                at_ms: 1,
                errors: 6,
                bytes_written: 0,
            },
            Sample {
                at_ms: 2,
                errors: 9,
                bytes_written: 0,
            },
            Sample {
                at_ms: 3,
                errors: 14,
                bytes_written: 0,
            },
            Sample {
                at_ms: 4,
                errors: 4,
                bytes_written: 0,
            },
        ];
        assert_eq!(regression(&samples), Some(8));
        assert_eq!(regression(&[]), None);
        assert_eq!(
            regression(&[Sample {
                at_ms: 0,
                errors: 3,
                bytes_written: 0
            }]),
            None
        );
    }

    /// A run that strictly improves on every sample never cuts.
    #[test]
    fn run_that_only_improves_never_cuts() {
        let mut t = Tracker::new(Policy {
            patience: 2,
            min_bytes_progress: 1,
        });
        for (i, errors) in [50u32, 40, 30, 20, 10, 5, 1].iter().enumerate() {
            let d = t.observe(Sample {
                at_ms: i as u64,
                errors: *errors,
                bytes_written: 0,
            });
            assert_eq!(d, Decision::Continue);
        }
        assert_eq!(t.best_errors(), Some(1));
        assert_eq!(t.stalled(), 0);
    }

    /// Best never rises, even across spikes.
    #[test]
    fn best_errors_never_rises_across_spikes() {
        let mut t = Tracker::new(Policy::lenient());
        t.observe(Sample {
            at_ms: 0,
            errors: 12,
            bytes_written: 0,
        });
        t.observe(Sample {
            at_ms: 1,
            errors: 40,
            bytes_written: 0,
        });
        assert_eq!(t.best_errors(), Some(12));
    }

    /// Sub-threshold byte writes do not count as progress.
    #[test]
    fn sub_threshold_bytes_do_not_prevent_a_cut() {
        let mut t = Tracker::new(Policy {
            patience: 2,
            min_bytes_progress: 100,
        });
        assert_eq!(
            t.observe(Sample {
                at_ms: 0,
                errors: 7,
                bytes_written: 0
            }),
            Decision::Continue
        );
        assert_eq!(
            t.observe(Sample {
                at_ms: 1,
                errors: 7,
                bytes_written: 1
            }),
            Decision::Continue
        );
        match t.observe(Sample {
            at_ms: 2,
            errors: 7,
            bytes_written: 2,
        }) {
            Decision::Cut { .. } => {}
            Decision::Continue | Decision::Done => {
                panic!("expected Cut for sub-threshold byte writes")
            }
        }
    }
}

use std::collections::HashMap;

use crate::limit_signal::parse_reset;
use crate::outcome::OutcomeClass;

/// Identifies a provider usage bucket shared by one or more adapters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Bucket(pub String);

/// Evidence that a provider usage limit was reached.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LimitHit {
    /// The shared bucket that refused the work.
    pub bucket: Bucket,
    /// Time at which the refusal was detected.
    pub detected_at: u64,
    /// Time at which the provider says its window resets, when supplied.
    pub reset_at: Option<u64>,
    /// The output text that matched a configured marker.
    pub evidence: String,
}

/// Current availability of a usage bucket.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BucketState {
    /// Work may run immediately.
    Available,
    /// Work is parked until `until`.
    Parked { until: u64, attempts: u32 },
}

/// State needed to resume a parked run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResumeHandle {
    /// Adapter-specific session identifier.
    pub session_id: String,
    /// Worktree containing the run's context.
    pub worktree: String,
}

#[derive(Debug)]
struct ParkedBucket {
    until: u64,
    attempts: u32,
    handles: Vec<ResumeHandle>,
    parked_since: u64,
    parked_total: u64,
}

/// Tracks provider buckets and the runs waiting for them.
#[derive(Debug, Default)]
pub struct QuotaTracker {
    default_window_secs: u64,
    buckets: HashMap<Bucket, ParkedBucket>,
    parked_totals: HashMap<Bucket, u64>,
}

impl QuotaTracker {
    /// Creates a tracker whose unspecified reset windows use `default_window_secs`.
    pub fn new(default_window_secs: u64) -> Self {
        Self {
            default_window_secs,
            buckets: HashMap::new(),
            parked_totals: HashMap::new(),
        }
    }

    /// Scans output for a case-insensitive usage-limit marker.
    pub fn detect(
        &self,
        bucket: &Bucket,
        output: &str,
        markers: &[String],
        now: u64,
    ) -> Option<LimitHit> {
        let lowered = output.to_lowercase();
        let evidence = markers
            .iter()
            .filter(|marker| !marker.is_empty())
            .find_map(|marker| {
                let marker_lower = marker.to_lowercase();
                lowered.find(&marker_lower).map(|index| {
                    output
                        .get(index..)
                        .and_then(|tail| tail.get(..marker_lower.len()))
                        .map_or_else(|| marker.clone(), ToOwned::to_owned)
                })
            })?;
        let reset_at = parse_reset_seconds(&lowered).map(|seconds| now.saturating_add(seconds));
        Some(LimitHit {
            bucket: bucket.clone(),
            detected_at: now,
            reset_at,
            evidence,
        })
    }

    /// Parks a run, sharing the bucket with any other waiting runs.
    pub fn park(&mut self, hit: &LimitHit, handle: ResumeHandle, now: u64) {
        let entry = self.buckets.entry(hit.bucket.clone());
        match entry {
            std::collections::hash_map::Entry::Vacant(slot) => {
                let attempts = 1;
                let until = hit.reset_at.unwrap_or_else(|| {
                    now.saturating_add(backoff(self.default_window_secs, attempts))
                });
                slot.insert(ParkedBucket {
                    until,
                    attempts,
                    handles: vec![handle],
                    parked_since: now,
                    parked_total: 0,
                });
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let parked = slot.get_mut();
                if now >= parked.until {
                    parked.parked_total = parked.parked_total.saturating_add(
                        now.saturating_sub(parked.parked_since)
                            .min(parked.until.saturating_sub(parked.parked_since)),
                    );
                    parked.parked_since = now;
                }
                parked.attempts = parked.attempts.saturating_add(1);
                let desired = hit.reset_at.unwrap_or_else(|| {
                    now.saturating_add(backoff(self.default_window_secs, parked.attempts))
                });
                parked.until = if now >= parked.until {
                    desired
                } else {
                    parked.until.max(desired)
                };
                parked.handles.push(handle);
            }
        }
    }

    /// Returns the bucket state at `now` without requiring cleanup first.
    pub fn state(&self, bucket: &Bucket, now: u64) -> BucketState {
        match self.buckets.get(bucket) {
            Some(parked) if now < parked.until => BucketState::Parked {
                until: parked.until,
                attempts: parked.attempts,
            },
            _ => BucketState::Available,
        }
    }

    /// Removes and returns every bucket whose park has expired.
    pub fn due(&mut self, now: u64) -> Vec<(Bucket, Vec<ResumeHandle>)> {
        let due_buckets: Vec<Bucket> = self
            .buckets
            .iter()
            .filter(|(_, parked)| now >= parked.until)
            .map(|(bucket, _)| bucket.clone())
            .collect();
        due_buckets
            .into_iter()
            .filter_map(|bucket| {
                self.buckets.remove(&bucket).map(|parked| {
                    let elapsed = parked
                        .parked_total
                        .saturating_add(parked.until.saturating_sub(parked.parked_since));
                    let total = self.parked_totals.entry(bucket.clone()).or_default();
                    *total = total.saturating_add(elapsed);
                    (bucket, parked.handles)
                })
            })
            .collect()
    }

    /// Resets the consecutive-park backoff for a bucket after a successful run.
    pub fn succeeded(&mut self, bucket: &Bucket) {
        if let Some(parked) = self.buckets.get_mut(bucket) {
            parked.attempts = 0;
        }
    }

    /// Returns the elapsed time this bucket has spent parked.
    pub fn parked_secs(&self, bucket: &Bucket, now: u64) -> u64 {
        let historical = self.parked_totals.get(bucket).copied().unwrap_or(0);
        let current = self.buckets.get(bucket).map_or(0, |parked| {
            parked.parked_total.saturating_add(
                now.saturating_sub(parked.parked_since)
                    .min(parked.until.saturating_sub(parked.parked_since)),
            )
        });
        historical.saturating_add(current)
    }
}

fn backoff(window: u64, attempts: u32) -> u64 {
    let mut value = window.min(86_400);
    for _ in 1..attempts {
        value = value.saturating_mul(2).min(86_400);
        if value == 86_400 {
            break;
        }
    }
    value
}

fn parse_reset_seconds(output: &str) -> Option<u64> {
    ["retry after", "reset in"].iter().find_map(|prefix| {
        let start = output.find(prefix)? + prefix.len();
        let digits = output[start..]
            .trim_start()
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>();
        if digits.is_empty() {
            None
        } else {
            digits.parse().ok()
        }
    })
}

// --- The park decision ------------------------------------------------------
//
// `park_after` composes recognition this crate already owns: the class comes
// from `outcome`, the stated instant from `limit_signal::parse_reset`. It adds
// one decision and no new parsing.

/// Milliseconds in one second. `limit_signal::parse_reset` speaks unix seconds
/// while a [`Park`] speaks unix milliseconds, so stated instants are scaled
/// here and nowhere else.
const MILLIS_PER_SECOND: u64 = 1_000;

/// Whether an arm should be parked after a finished run, and until when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Park {
    /// Do not park. The run tells us nothing about future availability.
    No,
    /// Park until this instant, in unix milliseconds, because the provider said so.
    Until {
        /// The instant the provider stated, in unix milliseconds.
        at_ms: u64,
        /// What was found, quoted for the next reader of the parked-until record.
        grounds: String,
    },
    /// Park for a backoff because the provider refused without saying when it would relent.
    Backoff {
        /// The instant the backoff expires, in unix milliseconds.
        until_ms: u64,
        /// Why an admitted backoff stands where a stated reset would, for the next reader.
        grounds: String,
    },
}

/// Decide whether a finished run should park its arm.
///
/// `now_ms` is the current instant, in unix milliseconds; `log_head` is the
/// run's own log, already ANSI-stripped (stripping is idempotent, so a caller
/// that forgot costs nothing).
///
/// This decides about ONE ARM from ONE RUN. It does not know that one key may
/// serve thirteen arms; a caller that parks a whole provider on the strength
/// of this return is doing something this function did not authorise.
///
/// Which classes park is a decision, not a discovery: today only
/// [`OutcomeClass::QuotaLimited`] parks, because it is the one class that
/// records the provider speaking about its own future availability. A future
/// class might justify parking; adding it is a decision to make here, in the
/// open. Every other class is [`Park::No`]: `ArmResult` (the arm ran, and
/// nothing about the provider was learned), `Infrastructure` (a broken
/// harness is not a provider refusing, and parking the arm would hide our own
/// fault as the arm's unavailability), `Cancelled` (we killed it), `Unknown`
/// (nothing was determined, so nothing is concluded), and `TaskInvalid`
/// (nothing was learned about the provider either; this class is not
/// enumerated in the park clauses and is resolved here as [`Park::No`]).
///
/// The stated instant: [`Park::Until`] carries the instant
/// `limit_signal::parse_reset` finds, scaled into the milliseconds `Park`
/// speaks. `parse_reset` returns unix seconds for its absolute formats and
/// resolves its relative formats against the instant it is given, so it is
/// given `now_ms / 1000`; no second reset parser is written here. Nor is a
/// reset ever invented: a refusal with NO stated reset is [`Park::Backoff`] of
/// `now_ms + default_backoff_ms`, with grounds saying the provider did not
/// state a time — an invented reset is worse than an admitted backoff, because
/// the next reader cannot tell it from a real one. An empty `log_head` with
/// `QuotaLimited` is [`Park::Backoff`] too: the class is authoritative and the
/// absent log merely fails to refine it.
///
/// Windows: a park is returned only for a window that has not already closed.
/// A reset in the past relative to `now_ms` is [`Park::No`] — a window that
/// has already closed does not park anything; so is a reset exactly equal to
/// `now_ms` (pinned here: the window closes at this instant), and so is
/// `default_backoff_ms: 0` with no stated reset (pinned here: a backoff that
/// ends now parks nothing). `now_ms + default_backoff_ms` saturates rather
/// than wrapping, and the saturated backoff still parks — the provider
/// refused and a positive backoff was requested, so the only [`Park::No`] on
/// the backoff path is a zero-length one.
///
/// ```
/// use farmerbob_core::outcome::OutcomeClass;
/// use farmerbob_core::quota::{Park, park_after};
///
/// // The provider said when it would relent: park until then, in milliseconds.
/// let park = park_after(
///     OutcomeClass::QuotaLimited,
///     "Error: rate limit exceeded; retry-after: 3600",
///     1_789_000_000_000,
///     60_000,
/// );
/// assert!(matches!(park, Park::Until { at_ms: 1_789_003_600_000, .. }));
///
/// // No stated time: an admitted backoff, never a guessed reset.
/// let park = park_after(OutcomeClass::QuotaLimited, "quota exceeded", 1_000, 60_000);
/// assert!(matches!(park, Park::Backoff { until_ms: 61_000, .. }));
/// ```
pub fn park_after(
    class: OutcomeClass,
    log_head: &str,
    now_ms: u64,
    default_backoff_ms: u64,
) -> Park {
    match class {
        OutcomeClass::QuotaLimited => {}
        OutcomeClass::ArmResult
        | OutcomeClass::Infrastructure
        | OutcomeClass::TaskInvalid
        | OutcomeClass::Cancelled
        | OutcomeClass::Unknown => return Park::No,
    }
    match parse_reset(log_head, now_ms / MILLIS_PER_SECOND) {
        Some(reset_secs) => {
            let at_ms = reset_secs.saturating_mul(MILLIS_PER_SECOND);
            if at_ms > now_ms {
                Park::Until {
                    at_ms,
                    grounds: format!(
                        "provider stated a reset (resolves to {at_ms} ms); log: {}",
                        log_head.trim()
                    ),
                }
            } else {
                Park::No
            }
        }
        None => {
            if default_backoff_ms == 0 {
                // Pinned: a zero backoff parks until now, which parks nothing.
                // A positive backoff always parks, saturated or not.
                return Park::No;
            }
            Park::Backoff {
                until_ms: now_ms.saturating_add(default_backoff_ms),
                grounds: format!(
                    "provider did not state a reset time; default backoff of {default_backoff_ms} ms applies"
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket() -> Bucket {
        Bucket("shared".into())
    }
    fn handle(id: &str) -> ResumeHandle {
        ResumeHandle {
            session_id: id.into(),
            worktree: "w".into(),
        }
    }

    #[test]
    fn retry_after_sets_reset_at() {
        let tracker = QuotaTracker::new(10);
        let hit = tracker.detect(
            &bucket(),
            "RATE LIMIT; retry after 30",
            &["rate limit".into()],
            100,
        );
        assert_eq!(hit.and_then(|h| h.reset_at), Some(130));
    }

    #[test]
    fn no_stated_reset_uses_window() {
        let tracker = QuotaTracker::new(10);
        let hit = tracker
            .detect(&bucket(), "rate limit", &["RATE LIMIT".into()], 100)
            .unwrap();
        let mut tracker = tracker;
        tracker.park(&hit, handle("a"), 100);
        assert_eq!(
            tracker.state(&bucket(), 109),
            BucketState::Parked {
                until: 110,
                attempts: 1
            }
        );
    }

    #[test]
    fn backoff_doubles_then_saturates() {
        let mut tracker = QuotaTracker::new(30_000);
        for i in 0..4 {
            tracker.park(
                &LimitHit {
                    bucket: bucket(),
                    detected_at: 0,
                    reset_at: None,
                    evidence: "x".into(),
                },
                handle(&i.to_string()),
                0,
            );
        }
        assert_eq!(
            tracker.state(&bucket(), 0),
            BucketState::Parked {
                until: 86_400,
                attempts: 4
            }
        );
    }

    #[test]
    fn succeeded_resets_attempts() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: None,
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        tracker.due(10);
        tracker.succeeded(&bucket());
        tracker.park(&hit, handle("b"), 0);
        assert_eq!(
            tracker.state(&bucket(), 0),
            BucketState::Parked {
                until: 10,
                attempts: 1
            }
        );
    }

    #[test]
    fn due_delivers_once() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        assert_eq!(tracker.due(5).len(), 1);
        assert!(tracker.due(5).is_empty());
    }

    #[test]
    fn shared_bucket_parks_both_arms() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        tracker.park(&hit, handle("b"), 0);
        assert_eq!(
            tracker.due(5).into_iter().next().map(|(_, h)| h.len()),
            Some(2)
        );
    }
}

// Clause tests for `park_after`, one per distinct behaviour the task pins.
// The boundaries the task explicitly leaves to the implementation — a reset
// exactly equal to now_ms, default_backoff_ms of 0 with no stated reset, the
// unenumerated TaskInvalid class — are pinned in `park_after`'s doc and
// deliberately NOT asserted here: another correct implementation of the same
// spec may reasonably choose the other side, and a suite that fails a correct
// rival measures the author's guess, not the code.
#[cfg(test)]
mod park_after_tests {
    use super::*;

    const NOW_MS: u64 = 1_789_000_000_000;
    const BACKOFF_MS: u64 = 60_000;

    // Clause 1, in its strong form: the class decides, not the log. An
    // ArmResult whose log quotes a reset must not park.
    #[test]
    fn an_arm_result_never_parks_even_when_the_log_states_a_reset() {
        let park = park_after(
            OutcomeClass::ArmResult,
            "rate limit hit; retry-after: 3600",
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(park, Park::No);
    }

    // Clause 4: a broken harness is not a provider refusing.
    #[test]
    fn infrastructure_never_parks_even_when_the_log_states_a_reset() {
        let park = park_after(
            OutcomeClass::Infrastructure,
            "Error: rate limit exceeded; retry-after: 3600",
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(park, Park::No);
    }

    // Clause 5: we killed it.
    #[test]
    fn cancelled_never_parks() {
        let park = park_after(
            OutcomeClass::Cancelled,
            "retry-after: 3600",
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(park, Park::No);
    }

    // Clause 6: nothing was determined, so nothing is concluded.
    #[test]
    fn unknown_never_parks() {
        let park = park_after(
            OutcomeClass::Unknown,
            "retry-after: 3600",
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(park, Park::No);
    }

    // Clause 2: a stated relative reset parks Until the instant parse_reset
    // gives, expressed in the milliseconds Park speaks.
    #[test]
    fn a_stated_relative_reset_parks_until_it_expires() {
        let log = "Error: rate limit exceeded; retry-after: 3600";
        match park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS) {
            Park::Until { at_ms, grounds } => {
                // parse_reset resolves the statement against NOW_MS / 1000;
                // the result is scaled into milliseconds, nowhere else.
                assert_eq!(at_ms, 1_789_003_600_000);
                assert!(
                    grounds.contains("3600"),
                    "grounds must quote what was found: {grounds}"
                );
            }
            other => panic!("a stated reset must park Until, got {other:?}"),
        }
    }

    // Clause 2, absolute form: a unix-seconds reset_at is honoured, not misread
    // against a millisecond now (unscaled, it would always look already past).
    #[test]
    fn a_stated_absolute_reset_parks_until_it_expires() {
        let log = r#"{"error": "quota", "reset_at": 1789603200}"#;
        match park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS) {
            Park::Until { at_ms, grounds } => {
                assert_eq!(at_ms, 1_789_603_200_000);
                assert!(
                    grounds.contains("reset_at"),
                    "grounds must quote what was found: {grounds}"
                );
            }
            other => panic!("a stated reset must park Until, got {other:?}"),
        }
    }

    // Clause 2, RFC 3339 form: same instant, third format, same discipline.
    #[test]
    fn a_stated_rfc3339_reset_parks_until_it_expires() {
        let log = "usage limit hit, resets at 2026-09-17T00:00:00Z please wait";
        match park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS) {
            Park::Until { at_ms, .. } => assert_eq!(at_ms, 1_789_603_200_000),
            other => panic!("a stated reset must park Until, got {other:?}"),
        }
    }

    // Clause 7: a window that has already closed parks nothing.
    #[test]
    fn a_reset_in_the_past_parks_nothing() {
        let log = r#"quota refused; "reset_at": 1788999000"#;
        let park = park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS);
        assert_eq!(park, Park::No);
    }

    // Clause 3: no stated reset is an admitted backoff of exactly
    // now_ms + default_backoff_ms, never a guessed Until.
    #[test]
    fn no_stated_reset_is_a_backoff_of_the_default_window() {
        match park_after(
            OutcomeClass::QuotaLimited,
            "quota exceeded for project",
            NOW_MS,
            BACKOFF_MS,
        ) {
            Park::Backoff { until_ms, grounds } => {
                assert_eq!(until_ms, NOW_MS + BACKOFF_MS);
                assert!(!grounds.is_empty());
            }
            other => panic!("no stated reset must back off, got {other:?}"),
        }
    }

    // Clause 3 boundary: an empty log defers to the authoritative class.
    #[test]
    fn an_empty_log_with_a_refusal_still_backs_off() {
        match park_after(OutcomeClass::QuotaLimited, "", 1_000, 1) {
            Park::Backoff { until_ms, .. } => assert_eq!(until_ms, 1_001),
            other => panic!("an empty log must not suppress the backoff, got {other:?}"),
        }
    }

    // Pure whitespace finds no reset whether or not a reader trims first, so
    // both readings of the boundary converge on the backoff.
    #[test]
    fn a_whitespace_log_converges_on_the_backoff() {
        match park_after(OutcomeClass::QuotaLimited, "   ", 1_000, 1) {
            Park::Backoff { until_ms, .. } => assert_eq!(until_ms, 1_001),
            other => panic!("expected Backoff, got {other:?}"),
        }
    }

    // Boundary: the backoff addition saturates rather than wrapping.
    #[test]
    fn backoff_overflow_saturates_rather_than_wrapping() {
        match park_after(OutcomeClass::QuotaLimited, "quota exceeded", u64::MAX, 1) {
            Park::Backoff { until_ms, .. } => assert_eq!(until_ms, u64::MAX),
            other => panic!("expected Backoff, got {other:?}"),
        }
    }

    // Boundary: now_ms of 0 with a stated future reset neither underflows nor
    // misreads the instant.
    #[test]
    fn zero_now_with_a_stated_reset_neither_underflows_nor_guesses() {
        match park_after(OutcomeClass::QuotaLimited, r#""reset_at": 500"#, 0, 0) {
            Park::Until { at_ms, .. } => assert_eq!(at_ms, 500_000),
            other => panic!("expected Until, got {other:?}"),
        }
    }
}

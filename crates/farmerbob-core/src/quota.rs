use std::collections::{BTreeMap, HashMap};

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

    /// Returns every bucket whose park has elapsed by `now_ms`, without
    /// mutating the tracker.
    ///
    /// Takes `&self` because it is a pure read: a caller should not need a
    /// mutable borrow to ask a question, and `&mut self` on a function that
    /// mutates nothing tells a reader the opposite of the truth. Two `due`
    /// calls can be alive over one tracker at once, which a `&mut self`
    /// receiver forbids.
    ///
    /// This is a READ, not a drain: calling it twice with the same
    /// `now_ms` returns the same set both times, and a bucket it yields
    /// keeps its full park history intact -- [`QuotaTracker::attempts`]
    /// for that bucket is unchanged by this call, and a later `due` call
    /// still yields it if nothing cleared it in between. The only thing
    /// that clears a bucket's history is [`QuotaTracker::succeeded`];
    /// draining this iterator is not that, because a caller that happens
    /// not to poll must not see different behaviour than one that polls
    /// obsessively -- see `park_outcome_is_independent_of_polling_due` in
    /// the tests.
    ///
    /// A bucket whose park instant equals `now_ms` exactly has elapsed,
    /// and is included; this agrees with [`QuotaTracker::is_parked`],
    /// which is false at the same instant.
    ///
    /// Ordered by bucket name, ascending byte order on the wrapped string
    /// (not map order, which is unspecified and would otherwise vary run to
    /// run), so a caller that logs the result gets the same line every run.
    ///
    /// ```
    /// use farmerbob_core::quota::{Bucket, LimitHit, QuotaTracker, ResumeHandle};
    /// let mut tracker = QuotaTracker::new(10);
    /// tracker.park(
    ///     &LimitHit {
    ///         bucket: Bucket("b".into()),
    ///         detected_at: 0,
    ///         reset_at: Some(5),
    ///         evidence: "x".into(),
    ///     },
    ///     ResumeHandle { session_id: "s".into(), worktree: "w".into() },
    ///     0,
    /// );
    /// let t: &QuotaTracker = &tracker;
    /// assert_eq!(t.due(5).len(), 1); // a question, not a mutation
    /// ```
    pub fn due(&self, now_ms: u64) -> Vec<(Bucket, Vec<ResumeHandle>)> {
        let mut due_buckets: Vec<(Bucket, Vec<ResumeHandle>)> = self
            .buckets
            .iter()
            .filter(|(_, parked)| now_ms >= parked.until)
            .map(|(bucket, parked)| (bucket.clone(), parked.handles.clone()))
            .collect();
        due_buckets.sort_by(|(a, _), (b, _)| a.0.cmp(&b.0));
        due_buckets
    }

    /// Whether this bucket is currently parked, without changing anything.
    ///
    /// A pure query: calling it any number of times returns the same
    /// answer, and calling it leaves [`QuotaTracker::due`] returning
    /// exactly what it would have returned had this never been called.
    /// Agrees with [`QuotaTracker::due`] at the boundary: a park instant
    /// exactly equal to `now_ms` has elapsed, so `due` yields it and this
    /// returns `false`.
    pub fn is_parked(&self, bucket: &Bucket, now_ms: u64) -> bool {
        self.buckets
            .get(bucket)
            .is_some_and(|parked| now_ms < parked.until)
    }

    /// How many consecutive parks this bucket has accumulated since it was
    /// last cleared.
    ///
    /// `None` when the tracker has never parked this bucket, and also when
    /// [`QuotaTracker::succeeded`] has cleared its history since the last
    /// time it was parked. These are the same answer to two different
    /// questions -- this type does not distinguish "never parked" from
    /// "parked, then cleared" -- and no caller should need to: both mean
    /// there is no backoff history left to escalate from. `Some(0)` is
    /// impossible: a bucket recorded here has been parked at least once.
    pub fn attempts(&self, bucket: &Bucket) -> Option<u32> {
        self.buckets.get(bucket).map(|parked| parked.attempts)
    }

    /// Clears a bucket's consecutive-park backoff history after a
    /// successful run.
    ///
    /// This is the ONLY thing on `QuotaTracker` that clears a bucket's
    /// history: [`QuotaTracker::due`] is a read and never does this. After
    /// this call, [`QuotaTracker::attempts`] for `bucket` is `None`, and
    /// [`QuotaTracker::is_parked`] is `false`, indistinguishable from a
    /// bucket the tracker has never parked. Calling this on a bucket the
    /// tracker has never parked, or has already cleared, is not an error;
    /// `attempts` simply stays at `None`, which is where it already was.
    ///
    /// It genuinely mutates, so it keeps `&mut self` even now that `due`
    /// does not; the negative is pinned, not merely the positive.
    ///
    /// ```compile_fail
    /// use farmerbob_core::quota::{Bucket, QuotaTracker};
    /// let tracker = QuotaTracker::new(10);
    /// // An immutable binding cannot reach the one mutator:
    /// tracker.succeeded(&Bucket("b".into()));
    /// ```
    pub fn succeeded(&mut self, bucket: &Bucket) {
        if let Some(parked) = self.buckets.remove(bucket) {
            let elapsed = parked
                .parked_total
                .saturating_add(parked.until.saturating_sub(parked.parked_since));
            let total = self.parked_totals.entry(bucket.clone()).or_default();
            *total = total.saturating_add(elapsed);
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

/// How far a refusal reaches.
///
/// A CLOSED set of three. There is no fourth radius and there are no sub-radii:
/// a caller holding a [`Blast`] can match every case exhaustively, and a future
/// radius arriving here is a decision made in the open, not a variant snuck
/// into a return value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blast {
    /// Only the arm that was refused.
    Arm,
    /// Every arm sharing the refused arm's `quota_bucket` -- one upstream vendor.
    Bucket,
    /// Every arm sharing the refused arm's `provider` -- one credential.
    Credential,
}

/// One arm's record: the two axes a refusal can widen along.
///
/// A mirror of the per-arm entry the `fb` crate parses from `sources.toml`,
/// carrying exactly the fields [`arms_in_blast`] reads. The full record --
/// status, model, price, eligibility -- lives in `fb`'s `Source`, which cannot
/// be imported here because `fb` depends on this crate, not the reverse. The
/// field names and the `source` table name match that registry deliberately, so
/// that when a bridge is built it hands a parsed registry over unchanged
/// instead of rewriting it field by field.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    /// Which credential pays for this arm. Many arms share one, which is
    /// exactly why a spent key is a wide refusal. Absent when the registry
    /// does not record it.
    #[serde(default)]
    pub provider: Option<String>,
    /// Which upstream vendor rate-limits this arm. For arms behind a router
    /// this is the vendor behind the router, not the router: OpenRouter is a
    /// credential, and it was the vendor that refused. Absent when the
    /// registry does not record it.
    #[serde(default)]
    pub quota_bucket: Option<String>,
}

/// Which arms exist and what they share, as [`arms_in_blast`] sees it.
///
/// The mirror-status caveat of [`Source`] applies here too: this stands in for
/// `fb`'s registry, which carries the same `source` table (hence the serde
/// rename) and the same two fields per arm, so the real `sources.toml` shape
/// deserializes into this unchanged. Arm names are compared exactly, as
/// everywhere else in this crate.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Registry {
    /// Arm name to record.
    #[serde(rename = "source")]
    pub sources: BTreeMap<String, Source>,
}

impl Registry {
    /// Creates an empty registry. An empty registry cannot widen: every
    /// [`Blast`] over it returns just the named arm.
    pub fn new() -> Self {
        Self {
            sources: BTreeMap::new(),
        }
    }

    /// Records one arm's two axes, replacing any record under the same name.
    pub fn insert(&mut self, arm: &str, provider: Option<&str>, quota_bucket: Option<&str>) {
        self.sources.insert(
            arm.to_string(),
            Source {
                provider: provider.map(str::to_string),
                quota_bucket: quota_bucket.map(str::to_string),
            },
        );
    }

    /// The arm's record, when the registry has one.
    pub fn get(&self, arm: &str) -> Option<&Source> {
        self.sources.get(arm)
    }
}

/// The bucket an arm counts against: its `quota_bucket`, else its `provider`.
///
/// This is the rule the registry itself applies when it counts an arm against
/// a vendor pool, and it is applied uniformly -- to the refused arm and to
/// every arm compared against it. A fallback that held only for the refused
/// arm would compare its derived bucket against a field the other arms are
/// not read for.
fn bucket_identity(source: &Source) -> Option<&str> {
    source
        .quota_bucket
        .as_deref()
        .or(source.provider.as_deref())
}

/// Which arms a refusal of `arm` reaches, given the registry.
///
/// Returns the refused arm itself plus every arm in scope, sorted, without
/// duplicates. The result is never empty: the refused arm is always a member
/// by construction, so a caller need not handle an empty case, and an empty
/// result is impossible -- not merely unexpected.
///
/// An arm absent from the registry returns just that arm, for every
/// [`Blast`]: we cannot widen from what we do not know.
///
/// The radii, exactly:
///
/// * [`Blast::Arm`] -- exactly the refused arm, registry or no registry.
/// * [`Blast::Credential`] -- every arm whose `provider` equals the refused
///   arm's, compared exactly. An arm whose provider is absent stands alone:
///   an axis the registry does not record cannot be widened along.
/// * [`Blast::Bucket`] -- every arm counting against the refused arm's
///   bucket. That bucket is the arm's `quota_bucket`; an arm whose
///   `quota_bucket` is absent falls back to its `provider`; an arm with
///   neither degenerates the radius to [`Blast::Arm`]. The same fallback
///   applies to every arm compared, so an arm recorded under its provider
///   alone still counts against that provider's pool.
///
/// Comparison is exact throughout: `deepseek` and `Deepseek` are different
/// buckets, as everywhere else in this crate.
///
/// # Example
///
/// ```
/// use farmerbob_core::quota::{Blast, Registry, arms_in_blast};
///
/// let mut registry = Registry::new();
/// registry.insert("or-hy3", Some("openrouter"), Some("tencent"));
/// registry.insert("or-qwen38-flash", Some("openrouter"), Some("qwen"));
/// registry.insert("ifm-k2", Some("ifm"), Some("deepseek"));
///
/// // One spent key refuses every arm that pays through it.
/// assert_eq!(
///     arms_in_blast("or-hy3", Blast::Credential, &registry),
///     ["or-hy3", "or-qwen38-flash"]
/// );
///
/// // One vendor's refusal reaches only the arms that vendor rate-limits.
/// assert_eq!(arms_in_blast("or-hy3", Blast::Bucket, &registry), ["or-hy3"]);
/// ```
pub fn arms_in_blast(arm: &str, blast: Blast, registry: &Registry) -> Vec<String> {
    match blast {
        Blast::Arm => vec![arm.to_string()],
        Blast::Bucket | Blast::Credential => {
            let Some(refused) = registry.get(arm) else {
                return vec![arm.to_string()];
            };
            let key = match blast {
                Blast::Bucket => bucket_identity(refused),
                _ => refused.provider.as_deref(),
            };
            let Some(key) = key else {
                return vec![arm.to_string()];
            };
            let mut reached: Vec<String> = registry
                .sources
                .iter()
                .filter(|(name, candidate)| {
                    // The refused arm is always in its own blast; the guard
                    // makes that true by construction rather than by hoping
                    // the key comparison happens to include it.
                    name.as_str() == arm
                        || match blast {
                            Blast::Bucket => bucket_identity(candidate) == Some(key),
                            _ => candidate.provider.as_deref() == Some(key),
                        }
                })
                .map(|(name, _)| name.clone())
                .collect();
            reached.sort();
            reached.dedup();
            reached
        }
    }
}

/// Infer the blast radius from the refusal text.
///
/// The phrasings recognised here are a KNOWN SUBSET -- an open one, grown only
/// when a real refusal teaches a new string; four distinct refusal strings
/// have been learned this week alone. The set of radii, by contrast, is
/// closed: see [`Blast`]. Anything unrecognised is [`Blast::Arm`], and that is
/// a decision, not a convenience fallback: a guess that widens benches arms
/// that would have worked, and this project has already spent a day proving
/// that a wrong confident answer costs more than an admitted narrow one.
/// [`Blast::Arm`] is the safe failure -- it costs a repeated dispatch, while
/// the alternative costs a benched fleet.
///
/// Matching is case-insensitive over the whole log head, the way this crate
/// matches every other marker. When a log carries phrasings for two radii the
/// credential reading wins: a spent key refuses every arm paying through it,
/// whatever the vendor behind it said about tokens.
///
/// # Example
///
/// ```
/// use farmerbob_core::quota::{Blast, blast_of};
///
/// // The key reached its cap: every arm on the credential is refused.
/// assert_eq!(
///     blast_of("error: key limit exceeded (total limit)"),
///     Blast::Credential
/// );
///
/// // The vendor capped the account's daily tokens: the bucket, not the key.
/// assert_eq!(
///     blast_of("error: token limit exceeded: tokens per day limit reached"),
///     Blast::Bucket
/// );
///
/// // Anything unrecognised: the narrowest answer, never a widening guess.
/// assert_eq!(blast_of("error: connection reset by peer"), Blast::Arm);
/// ```
pub fn blast_of(log_head: &str) -> Blast {
    let lowered = log_head.to_lowercase();
    if lowered.contains("key limit") {
        Blast::Credential
    } else if lowered.contains("token limit") {
        Blast::Bucket
    } else {
        Blast::Arm
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

    // UPDATED: previously named `succeeded_resets_attempts`, this passed
    // verbatim with the body of `succeeded` deleted, because the old `due`
    // removed the map entry first -- `succeeded` had nothing left to act on
    // either way. `due` no longer removes, so the second `park` below lands
    // on the SAME still-present entry unless `succeeded` actually cleared
    // it: with `succeeded`'s body deleted, `park` would take the `Occupied`
    // branch and `attempts` would be 2, failing the assertion below.
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
        tracker.due(10); // a read; must not help `succeeded` along
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

    // UPDATED: previously named `due_delivers_once` and asserted the
    // opposite of clause 1 (`due` draining the bucket so a second call
    // returned nothing). `due` is now a pure read: see
    // `due_is_idempotent` below for the current contract.
    #[test]
    fn due_is_idempotent() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        let first = tracker.due(5);
        let second = tracker.due(5);
        assert_eq!(first.len(), 1);
        assert_eq!(first, second);
    }

    #[test]
    fn due_does_not_clear_attempts() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        assert_eq!(tracker.attempts(&bucket()), Some(1));
        tracker.due(5);
        assert_eq!(tracker.attempts(&bucket()), Some(1));
    }

    #[test]
    fn succeeded_clears_attempts_to_none() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: None,
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        assert!(tracker.attempts(&bucket()).is_some());
        tracker.succeeded(&bucket());
        assert_eq!(tracker.attempts(&bucket()), None);
    }

    #[test]
    fn attempts_is_none_for_unseen_and_for_cleared_buckets() {
        let never_seen = QuotaTracker::new(10);
        assert_eq!(never_seen.attempts(&bucket()), None);

        let mut cleared = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: None,
            evidence: "x".into(),
        };
        cleared.park(&hit, handle("a"), 0);
        cleared.succeeded(&bucket());
        assert_eq!(cleared.attempts(&bucket()), None);
    }

    // The property this whole task is about, pinned directly: identical
    // event history (park at t=0 with window 10, limited again at t=20)
    // must produce an identical park whether or not something happened to
    // drain `due` in between. Named for the property, not for `due`, which
    // is the mechanism that used to break it.
    #[test]
    fn park_outcome_is_independent_of_polling_due() {
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: None,
            evidence: "x".into(),
        };

        let mut polled = QuotaTracker::new(10);
        polled.park(&hit, handle("a"), 0);
        polled.due(15); // drained mid-window; must change nothing
        polled.park(&hit, handle("b"), 20);

        let mut unpolled = QuotaTracker::new(10);
        unpolled.park(&hit, handle("a"), 0);
        unpolled.park(&hit, handle("b"), 20);

        assert_eq!(polled.state(&bucket(), 20), unpolled.state(&bucket(), 20));
        assert_eq!(polled.attempts(&bucket()), unpolled.attempts(&bucket()));
    }

    #[test]
    fn is_parked_does_not_change_what_due_returns() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        let before = tracker.due(5);
        for _ in 0..5 {
            tracker.is_parked(&bucket(), 5);
            tracker.is_parked(&bucket(), 100);
        }
        let after = tracker.due(5);
        assert_eq!(before, after);
    }

    #[test]
    fn due_on_an_empty_tracker_is_empty() {
        let tracker = QuotaTracker::new(10);
        assert!(tracker.due(0).is_empty());
        assert!(tracker.due(u64::MAX).is_empty());
    }

    // Clause 1, pinned in its strong form: the query is reachable through a
    // plain `&QuotaTracker`, alongside the other reads it agrees with.
    #[test]
    fn due_is_callable_on_an_immutable_binding() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        let t: &QuotaTracker = &tracker;
        let due = t.due(5);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, bucket());
        // The reads `due` agrees with at the boundary stay reachable too.
        assert!(!t.is_parked(&bucket(), 5));
        assert_eq!(t.attempts(&bucket()), Some(1));
        assert_eq!(t.state(&bucket(), 5), BucketState::Available);
    }

    // Clause 3: two `due` calls alive at once over the same tracker. Under a
    // `&mut self` receiver this cannot even be expressed through the shared
    // reference a reader holds; that is the concrete reason for `&self`.
    #[test]
    fn two_due_calls_can_be_alive_at_once_over_one_tracker() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        let t: &QuotaTracker = &tracker;
        let first = t.due(5);
        let second = t.due(5);
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
        // Both results are still alive and were not consumed by either call.
        assert_eq!(first[0].1, vec![handle("a")]);
        assert_eq!(second[0].1, vec![handle("a")]);
    }

    // The repetition boundary, pinned at a hundred rather than assumed from
    // two: every call returns the same set and leaves the park history alone.
    #[test]
    fn due_called_a_hundred_times_returns_identical_results() {
        let mut tracker = QuotaTracker::new(10);
        for name in ["b", "a"] {
            let hit = LimitHit {
                bucket: Bucket(name.into()),
                detected_at: 0,
                reset_at: Some(5),
                evidence: "x".into(),
            };
            tracker.park(&hit, handle("a"), 0);
        }
        let expected = tracker.due(5);
        assert_eq!(expected.len(), 2);
        for _ in 0..100 {
            assert_eq!(tracker.due(5), expected);
        }
        assert_eq!(tracker.attempts(&Bucket("a".into())), Some(1));
        assert_eq!(tracker.attempts(&Bucket("b".into())), Some(1));
    }

    // A tracker parked entirely into the future: the answer is empty and the
    // call changed nothing that a later read can see.
    #[test]
    fn a_tracker_parked_into_the_future_yields_nothing() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        assert!(tracker.due(4).is_empty());
        assert!(tracker.is_parked(&bucket(), 4));
        assert_eq!(tracker.attempts(&bucket()), Some(1));
    }

    // Boundary at zero: a park whose instant is exactly now_ms == 0 is
    // yielded, `is_parked` agrees, and nothing underflows.
    #[test]
    fn due_at_now_ms_zero_yields_a_bucket_parked_until_zero() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(0),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        let due = tracker.due(0);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, bucket());
        assert!(!tracker.is_parked(&bucket(), 0));
        assert_eq!(tracker.attempts(&bucket()), Some(1));
    }

    // Clause 4, positive half: the mutator still works, and still only
    // through a mutable binding.
    #[test]
    fn succeeded_still_clears_through_a_mutable_binding() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        let mutable: &mut QuotaTracker = &mut tracker;
        mutable.succeeded(&bucket());
        assert_eq!(tracker.attempts(&bucket()), None);
        assert!(tracker.due(5).is_empty());
    }

    // Boundary, pinned on both halves together: a park instant exactly
    // equal to `now` has elapsed, so `due` yields it and `is_parked`
    // disagrees -- a caller must never see a bucket reported as both.
    #[test]
    fn park_instant_exactly_at_now_is_due_and_not_parked() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        assert!(!tracker.is_parked(&bucket(), 5));
        assert_eq!(tracker.due(5).len(), 1);
    }

    #[test]
    fn succeeded_on_an_unparked_bucket_is_not_an_error() {
        let mut tracker = QuotaTracker::new(10);
        tracker.succeeded(&bucket());
        assert_eq!(tracker.attempts(&bucket()), None);
    }

    #[test]
    fn due_with_an_advancing_now_still_yields_an_uncleared_bucket() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: Some(5),
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        assert_eq!(tracker.due(5).len(), 1);
        assert_eq!(tracker.due(100).len(), 1);
    }

    #[test]
    fn a_bucket_parked_drained_then_limited_again_has_attempts_two() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit {
            bucket: bucket(),
            detected_at: 0,
            reset_at: None,
            evidence: "x".into(),
        };
        tracker.park(&hit, handle("a"), 0);
        tracker.due(10); // drain; does not clear history
        tracker.park(&hit, handle("b"), 10);
        assert_eq!(tracker.attempts(&bucket()), Some(2));
    }

    #[test]
    fn due_returns_buckets_in_ascending_name_order() {
        let mut tracker = QuotaTracker::new(10);
        for name in ["zeta", "alpha", "mid"] {
            let hit = LimitHit {
                bucket: Bucket(name.into()),
                detected_at: 0,
                reset_at: Some(0),
                evidence: "x".into(),
            };
            tracker.park(&hit, handle("a"), 0);
        }
        let names: Vec<String> = tracker.due(0).into_iter().map(|(b, _)| b.0).collect();
        assert_eq!(names, ["alpha", "mid", "zeta"]);
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

// Where the spec leaves a boundary to the implementation -- how a third arm
// with no `quota_bucket` compares against a derived bucket key, what a log
// naming two radii means, whether matching is case-sensitive -- the choice is
// pinned in the doc and deliberately NOT asserted here: another correct
// implementation of the same spec may reasonably choose the other side, and a
// suite that fails a correct rival measures the author's guess, not the code.
#[cfg(test)]
mod blast_tests {
    use super::*;

    /// The registry's real shape: one credential serving many vendors, and a
    /// vendor bucket reaching across credentials.
    fn registry() -> Registry {
        let mut r = Registry::new();
        r.insert("or-hy3", Some("openrouter"), Some("tencent"));
        r.insert("or-qwen38-flash", Some("openrouter"), Some("qwen"));
        r.insert("or-nemotron-ultra", Some("openrouter"), Some("nvidia"));
        r.insert("or-deepseek", Some("openrouter"), Some("deepseek"));
        r.insert("ifm-k2", Some("ifm"), Some("deepseek"));
        r.insert("ifm-k2-think", Some("ifm"), Some("deepseek"));
        r.insert("solo", Some("zai"), None);
        r
    }

    // Clause 1: every arm on the credential, including the refused one,
    // sorted, no duplicates.
    #[test]
    fn a_credential_blast_reaches_every_arm_on_the_key() {
        assert_eq!(
            arms_in_blast("or-hy3", Blast::Credential, &registry()),
            [
                "or-deepseek",
                "or-hy3",
                "or-nemotron-ultra",
                "or-qwen38-flash"
            ]
        );
    }

    // Clause 2: for this arm the bucket is strictly smaller than the
    // credential, and every arm it reaches is one the credential also covers.
    #[test]
    fn a_bucket_blast_is_narrower_than_the_credential_for_the_same_refusal() {
        let r = registry();
        let bucket = arms_in_blast("or-hy3", Blast::Bucket, &r);
        let credential = arms_in_blast("or-hy3", Blast::Credential, &r);
        assert!(bucket.len() < credential.len());
        assert!(bucket.iter().all(|arm| credential.contains(arm)));
    }

    // The motivating outage: a vendor refusing its account reaches the arms
    // behind other credentials that route to the same vendor.
    #[test]
    fn a_bucket_blast_crosses_providers_that_share_the_vendor() {
        assert_eq!(
            arms_in_blast("ifm-k2", Blast::Bucket, &registry()),
            ["ifm-k2", "ifm-k2-think", "or-deepseek"]
        );
    }

    // Clause 3.
    #[test]
    fn an_arm_blast_is_exactly_the_refused_arm() {
        assert_eq!(arms_in_blast("or-hy3", Blast::Arm, &registry()), ["or-hy3"]);
    }

    // Clause 4: never the empty vector, whatever the radius.
    #[test]
    fn an_unregistered_arm_is_its_own_blast_at_every_radius() {
        let r = registry();
        for blast in [Blast::Arm, Blast::Bucket, Blast::Credential] {
            assert_eq!(arms_in_blast("no-such-arm", blast, &r), ["no-such-arm"]);
        }
    }

    // Clause 8, first half: with no `quota_bucket` recorded, the bucket
    // radius is derived from the provider, so an arm bucketed there by name
    // is reached.
    #[test]
    fn a_bucket_blast_falls_back_to_the_provider_when_no_bucket_is_recorded() {
        let mut r = Registry::new();
        r.insert("plain", Some("venom"), None);
        r.insert("direct", Some("other"), Some("venom"));
        assert_eq!(
            arms_in_blast("plain", Blast::Bucket, &r),
            ["direct", "plain"]
        );
    }

    // Clause 8, second half: with neither field, the radius degenerates to
    // the arm itself.
    #[test]
    fn a_bucket_blast_without_bucket_or_provider_degenerates_to_the_arm() {
        let mut r = Registry::new();
        r.insert("bare", None, None);
        r.insert("other", Some("x"), Some("y"));
        assert_eq!(arms_in_blast("bare", Blast::Bucket, &r), ["bare"]);
    }

    // Boundaries at zero: an empty registry cannot widen.
    #[test]
    fn an_empty_registry_cannot_widen() {
        let r = Registry::new();
        for blast in [Blast::Arm, Blast::Bucket, Blast::Credential] {
            assert_eq!(arms_in_blast("or-hy3", blast, &r), ["or-hy3"]);
        }
    }

    // An arm that is the only member of its provider: Credential collapses
    // onto Arm in RESULT, which the spec pins as correct and forbids
    // special-casing.
    #[test]
    fn a_sole_provider_member_is_its_own_credential_blast() {
        let r = registry();
        let credential = arms_in_blast("solo", Blast::Credential, &r);
        assert_eq!(credential, ["solo"]);
        assert_eq!(credential, arms_in_blast("solo", Blast::Arm, &r));
    }

    // Two arms on one credential, different vendors: the credential reaches
    // both, the vendor bucket reaches one.
    #[test]
    fn two_arms_on_one_credential_split_when_only_the_credential_is_hit() {
        let mut r = Registry::new();
        r.insert("x1", Some("cred"), Some("vendor-a"));
        r.insert("x2", Some("cred"), Some("vendor-b"));
        assert_eq!(arms_in_blast("x1", Blast::Credential, &r), ["x1", "x2"]);
        assert_eq!(arms_in_blast("x1", Blast::Bucket, &r), ["x1"]);
    }

    // Spelling is identity: a bucket that only looks like another arm's
    // bucket under a different spelling is a different bucket.
    #[test]
    fn bucket_spelling_is_compared_exactly() {
        let mut r = Registry::new();
        r.insert("a", Some("p"), Some("deepseek"));
        r.insert("b", Some("p"), Some("Deepseek"));
        assert_eq!(arms_in_blast("a", Blast::Bucket, &r), ["a"]);
    }

    // Clauses 5 and 6: the two refusals this week's outages taught.
    #[test]
    fn a_spent_key_is_a_credential_refusal() {
        assert_eq!(
            blast_of("error: key limit exceeded (total limit)"),
            Blast::Credential
        );
    }

    #[test]
    fn a_daily_token_cap_is_a_bucket_refusal() {
        assert_eq!(
            blast_of("error: token limit exceeded: tokens per day limit reached"),
            Blast::Bucket
        );
    }

    // Clause 7: the narrowest answer on anything unrecognised.
    #[test]
    fn an_unrecognised_refusal_stays_narrow() {
        for head in [
            "error: connection reset by peer",
            "exit code 1 with no output",
            "the run produced no verdict",
        ] {
            assert_eq!(blast_of(head), Blast::Arm, "{head} should stay narrow");
        }
    }

    // Boundary at zero: an empty log head recognises nothing.
    #[test]
    fn an_empty_log_head_is_narrow() {
        assert_eq!(blast_of(""), Blast::Arm);
    }
}



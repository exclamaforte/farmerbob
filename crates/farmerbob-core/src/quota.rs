use std::collections::HashMap;

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
        let evidence = markers.iter().filter(|marker| !marker.is_empty()).find_map(|marker| {
            let marker_lower = marker.to_lowercase();
            lowered
                .find(&marker_lower)
                .map(|index| {
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
                let until = hit
                    .reset_at
                    .unwrap_or_else(|| now.saturating_add(backoff(self.default_window_secs, attempts)));
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
                    parked.parked_total = parked
                        .parked_total
                        .saturating_add(now.saturating_sub(parked.parked_since).min(parked.until.saturating_sub(parked.parked_since)));
                    parked.parked_since = now;
                }
                parked.attempts = parked.attempts.saturating_add(1);
                let desired = hit
                    .reset_at
                    .unwrap_or_else(|| now.saturating_add(backoff(self.default_window_secs, parked.attempts)));
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
                    let elapsed = parked.parked_total.saturating_add(
                        parked
                            .until
                            .saturating_sub(parked.parked_since),
                    );
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
        if digits.is_empty() { None } else { digits.parse().ok() }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket() -> Bucket { Bucket("shared".into()) }
    fn handle(id: &str) -> ResumeHandle { ResumeHandle { session_id: id.into(), worktree: "w".into() } }

    #[test]
    fn retry_after_sets_reset_at() {
        let tracker = QuotaTracker::new(10);
        let hit = tracker.detect(&bucket(), "RATE LIMIT; retry after 30", &["rate limit".into()], 100);
        assert_eq!(hit.and_then(|h| h.reset_at), Some(130));
    }

    #[test]
    fn no_stated_reset_uses_window() {
        let tracker = QuotaTracker::new(10);
        let hit = tracker.detect(&bucket(), "rate limit", &["RATE LIMIT".into()], 100).unwrap();
        let mut tracker = tracker;
        tracker.park(&hit, handle("a"), 100);
        assert_eq!(tracker.state(&bucket(), 109), BucketState::Parked { until: 110, attempts: 1 });
    }

    #[test]
    fn backoff_doubles_then_saturates() {
        let mut tracker = QuotaTracker::new(30_000);
        for i in 0..4 { tracker.park(&LimitHit { bucket: bucket(), detected_at: 0, reset_at: None, evidence: "x".into() }, handle(&i.to_string()), 0); }
        assert_eq!(tracker.state(&bucket(), 0), BucketState::Parked { until: 86_400, attempts: 4 });
    }

    #[test]
    fn succeeded_resets_attempts() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit { bucket: bucket(), detected_at: 0, reset_at: None, evidence: "x".into() };
        tracker.park(&hit, handle("a"), 0);
        tracker.due(10);
        tracker.succeeded(&bucket());
        tracker.park(&hit, handle("b"), 0);
        assert_eq!(tracker.state(&bucket(), 0), BucketState::Parked { until: 10, attempts: 1 });
    }

    #[test]
    fn due_delivers_once() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit { bucket: bucket(), detected_at: 0, reset_at: Some(5), evidence: "x".into() };
        tracker.park(&hit, handle("a"), 0);
        assert_eq!(tracker.due(5).len(), 1);
        assert!(tracker.due(5).is_empty());
    }

    #[test]
    fn shared_bucket_parks_both_arms() {
        let mut tracker = QuotaTracker::new(10);
        let hit = LimitHit { bucket: bucket(), detected_at: 0, reset_at: Some(5), evidence: "x".into() };
        tracker.park(&hit, handle("a"), 0);
        tracker.park(&hit, handle("b"), 0);
        assert_eq!(tracker.due(5).into_iter().next().map(|(_, h)| h.len()), Some(2));
    }
}

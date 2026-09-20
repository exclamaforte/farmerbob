//! Leases: serialized access to scarce, mostly indivisible resources
//! (the GPU above all).

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{LeaseId, RunId};

/// What kind of access a lease grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LeaseClass {
    /// Sole access to the resource; no other holder allowed.
    Exclusive,
    /// Shared access, capped at the VRAM the holder may occupy.
    Shared {
        /// VRAM budget in bytes the holder is allowed to use.
        vram_bytes: u64,
    },
}

/// A timed grant of access to one named resource, held by one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub id: LeaseId,
    /// Name of the contended resource, e.g. `"gpu0"`.
    pub resource: String,
    pub class: LeaseClass,
    /// Run currently holding the resource.
    pub holder: RunId,
    /// Opaque proof-of-holderness presented when releasing or renewing.
    pub token: String,
    pub acquired_at: DateTime<Utc>,
    /// Time-to-live from `acquired_at`; the lease lapses after it.
    pub ttl: Duration,
}

impl Lease {
    /// When the lease lapses if not renewed.
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.acquired_at + self.ttl
    }
}

/// The name of a contended resource, e.g. the one GPU.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceName(pub String);

/// Opaque proof of holderness, minted fresh at every grant and presented
/// back to [`LeaseManager::release`]. Without the token the lease cannot
/// be released.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LeaseToken(pub String);

/// The run holding — or waiting for — a lease.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HolderId(pub String);

/// What [`LeaseManager::request`] decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestOutcome {
    /// The resource was free: the caller holds it now, with this token.
    Granted(LeaseToken),
    /// The resource is taken: the request was queued.
    Queued {
        /// One-based place in the FIFO queue: `1` is granted next.
        position: usize,
    },
}

/// A lease just handed to a waiter, reported by the wake-up paths
/// ([`LeaseManager::release`], [`LeaseManager::expire`],
/// [`LeaseManager::holder_died`]) so the caller can wake the promoted
/// holder and tell it its token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// The contended resource now held by `holder`.
    pub resource: ResourceName,
    /// The waiter promoted to holder.
    pub holder: HolderId,
    /// Id of the freshly minted lease.
    pub lease: LeaseId,
    /// Token proving the new holder's holderness.
    pub token: LeaseToken,
}

/// A snapshot of one resource's occupancy, from [`LeaseManager::status`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseStatus {
    /// The resource described.
    pub resource: ResourceName,
    /// The current holder, if anyone holds it.
    pub holder: Option<HolderId>,
    /// How long the current holder has held it: `now - acquired_at`.
    /// Absent when there is no holder.
    pub held_for: Option<Duration>,
    /// How many requests are waiting.
    pub queue_depth: usize,
    /// The waiting holders, in FIFO order; the first is granted next.
    pub waiting: Vec<HolderId>,
}

/// One live grant of one resource to one holder. The token is the only
/// handle: the lease id exists to be reported in a [`Grant`], not to be
/// looked up.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActiveLease {
    token: LeaseToken,
    holder: HolderId,
    /// Stamp of the moment this lease was granted. A handover always
    /// restamps it; it never inherits the predecessor's stamp.
    acquired_at: DateTime<Utc>,
}

/// Per-resource bookkeeping: one exclusive holder slot plus a FIFO queue.
#[derive(Debug, Serialize, Deserialize)]
struct ResourceState {
    max_hold: Duration,
    active: Option<ActiveLease>,
    queue: VecDeque<HolderId>,
}

/// Hands out exclusive leases to named resources from a FIFO queue.
///
/// Pure and clock-free: every method that can create a grant takes `now`
/// from the caller, and a grant always stamps `acquired_at = now` — a lease
/// handed to a waiter by [`LeaseManager::release`], [`LeaseManager::expire`]
/// or [`LeaseManager::holder_died`] starts its full max hold at the moment
/// of handover. No method reads a clock.
///
/// Serialisable as a whole so a file-backed interim (bead farmerbob-oa5w.1)
/// can persist it between processes until the daemon owns serialization
/// (bead farmerbob-3ne).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LeaseManager {
    resources: HashMap<ResourceName, ResourceState>,
    /// Registration order, so sweeps over several resources are deterministic.
    order: Vec<ResourceName>,
}

impl LeaseManager {
    /// Creates an empty manager with no registered resources.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `name` as an exclusively-held resource whose leases may be
    /// held at most `max_hold` before [`LeaseManager::expire`] revokes them.
    /// Registering an already-registered name updates only its max hold;
    /// any live lease and queue survive.
    pub fn register_resource(&mut self, name: ResourceName, max_hold: Duration) {
        match self.resources.get_mut(&name) {
            Some(state) => state.max_hold = max_hold,
            None => {
                self.order.push(name.clone());
                self.resources.insert(
                    name,
                    ResourceState {
                        max_hold,
                        active: None,
                        queue: VecDeque::new(),
                    },
                );
            }
        }
    }

    /// Requests exclusive access to `resource` for `holder` at time `now`.
    ///
    /// A free resource is granted on the spot, stamped `acquired_at = now`.
    /// A taken resource queues the request FIFO and reports its position. A
    /// holder or waiter asking again is idempotent: it gets its existing
    /// grant or position, never a second lease.
    ///
    /// An unregistered resource can hold nothing, so the request queues
    /// without being stored — it is never granted, and never panics.
    pub fn request(
        &mut self,
        resource: &ResourceName,
        holder: HolderId,
        now: DateTime<Utc>,
    ) -> RequestOutcome {
        let state = match self.resources.get_mut(resource) {
            Some(state) => state,
            None => return RequestOutcome::Queued { position: 1 },
        };
        if let Some(active) = &state.active
            && active.holder == holder
        {
            return RequestOutcome::Granted(active.token.clone());
        }
        if let Some(position) = state.queue.iter().position(|queued| queued == &holder) {
            return RequestOutcome::Queued {
                position: position + 1,
            };
        }
        if state.active.is_none() {
            // A free resource always has an empty queue; if that were ever
            // broken, the queue head is served before the newcomer so FIFO
            // survives regardless.
            if state.queue.is_empty() {
                let (_, token) = Self::activate(state, holder, now);
                return RequestOutcome::Granted(token);
            }
            let _ = Self::grant_next(state, resource, now);
        }
        state.queue.push_back(holder);
        RequestOutcome::Queued {
            position: state.queue.len(),
        }
    }

    /// Releases the lease proved by `token` and grants the resource to the
    /// head of the queue at time `now`, returning that grant so the caller
    /// can wake the promoted holder. An unknown or already-released token
    /// is a harmless no-op returning `None`.
    pub fn release(&mut self, token: &LeaseToken, now: DateTime<Utc>) -> Option<Grant> {
        for name in &self.order {
            if let Some(state) = self.resources.get_mut(name) {
                let is_held = matches!(&state.active, Some(active) if &active.token == token);
                if is_held {
                    state.active = None;
                    return Self::grant_next(state, name, now);
                }
            }
        }
        None
    }

    /// Revokes every lease held past its resource's max hold at `now` —
    /// strictly past: a lease held exactly `max_hold` survives — advancing
    /// each affected queue. Returns the revoked tokens.
    pub fn expire(&mut self, now: DateTime<Utc>) -> Vec<LeaseToken> {
        let mut revoked = Vec::new();
        for name in &self.order {
            if let Some(state) = self.resources.get_mut(name) {
                let lapsed = match &state.active {
                    Some(active) => now.signed_duration_since(active.acquired_at) > state.max_hold,
                    None => false,
                };
                if lapsed {
                    if let Some(active) = state.active.take() {
                        revoked.push(active.token);
                    }
                    // The slot is provably empty here, so the only `None` case
                    // is an empty queue, which is fine to ignore.
                    let _ = Self::grant_next(state, name, now);
                }
            }
        }
        revoked
    }

    /// Forgets `holder`: its process vanished. Whatever it held is
    /// force-released, its queued requests are dropped, and each affected
    /// queue advances at `now`. Returns every grant produced — one per
    /// resource it held with a waiter — in registration order of the
    /// resources; an empty `Vec` when it held and queued nothing.
    pub fn holder_died(&mut self, holder: &HolderId, now: DateTime<Utc>) -> Vec<Grant> {
        let mut granted = Vec::new();
        for name in &self.order {
            if let Some(state) = self.resources.get_mut(name) {
                // Drop the dead holder's queue entries before granting, so a
                // promotion can never hand the resource back to the dead.
                state.queue.retain(|queued| queued != holder);
                let held = matches!(&state.active, Some(active) if &active.holder == holder);
                if held {
                    state.active = None;
                    if let Some(grant) = Self::grant_next(state, name, now) {
                        granted.push(grant);
                    }
                }
            }
        }
        granted
    }

    /// Snapshots `resource` at `now`: the current holder, how long it has
    /// held the lease, the queue depth, and the waiting holders in FIFO
    /// order. An unregistered or idle resource reads as empty, not a panic.
    pub fn status(&self, resource: &ResourceName, now: DateTime<Utc>) -> LeaseStatus {
        let state = match self.resources.get(resource) {
            Some(state) => state,
            None => {
                return LeaseStatus {
                    resource: resource.clone(),
                    holder: None,
                    held_for: None,
                    queue_depth: 0,
                    waiting: Vec::new(),
                };
            }
        };
        let (holder, held_for) = match &state.active {
            Some(active) => (
                Some(active.holder.clone()),
                Some(now.signed_duration_since(active.acquired_at)),
            ),
            None => (None, None),
        };
        LeaseStatus {
            resource: resource.clone(),
            holder,
            held_for,
            queue_depth: state.queue.len(),
            waiting: state.queue.iter().cloned().collect(),
        }
    }

    /// Snapshots every registered resource in registration order.
    pub fn status_all(&self, now: DateTime<Utc>) -> Vec<LeaseStatus> {
        self.order
            .iter()
            .map(|name| self.status(name, now))
            .collect()
    }

    /// Stamps a fresh lease onto `holder` in the single holder slot at time
    /// `now`, returning its id and token. The caller must have emptied the
    /// slot; invariant 1 is this function's only door.
    fn activate(
        state: &mut ResourceState,
        holder: HolderId,
        now: DateTime<Utc>,
    ) -> (LeaseId, LeaseToken) {
        let lease_id = LeaseId::new();
        let token = LeaseToken(uuid::Uuid::new_v4().to_string());
        state.active = Some(ActiveLease {
            token: token.clone(),
            holder,
            acquired_at: now,
        });
        (lease_id, token)
    }

    /// Pops the queue head and grants it at time `now`. `None` when the
    /// queue is empty — or when the slot is somehow still occupied, where
    /// the one-holder invariant wins over the queue.
    fn grant_next(
        state: &mut ResourceState,
        resource: &ResourceName,
        now: DateTime<Utc>,
    ) -> Option<Grant> {
        if state.active.is_some() {
            return None;
        }
        let holder = state.queue.pop_front()?;
        let (lease, token) = Self::activate(state, holder.clone(), now);
        Some(Grant {
            resource: resource.clone(),
            holder,
            lease,
            token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn lease_round_trips_through_json() {
        let lease = Lease {
            id: LeaseId::new(),
            resource: String::from("gpu0"),
            class: LeaseClass::Shared {
                vram_bytes: 24 * 1024 * 1024 * 1024,
            },
            holder: RunId::new(),
            token: String::from("lease-token-7f3a"),
            acquired_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            ttl: Duration::minutes(30),
        };

        let back: Lease = serde_json::from_str(&serde_json::to_string(&lease).unwrap()).unwrap();
        assert_eq!(lease, back);
        assert_eq!(back.expires_at(), lease.acquired_at + Duration::minutes(30));
    }
}

/// Tests for the [`LeaseManager`], one per required invariant plus the
/// time-provenance and edge behaviours the invariants rest on.
#[cfg(test)]
mod lease_manager_tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn resource(name: &str) -> ResourceName {
        ResourceName(name.to_owned())
    }

    fn holder(name: &str) -> HolderId {
        HolderId(name.to_owned())
    }

    fn granted(outcome: RequestOutcome) -> bool {
        matches!(outcome, RequestOutcome::Granted(_))
    }

    fn queued(outcome: RequestOutcome) -> bool {
        matches!(outcome, RequestOutcome::Queued { .. })
    }

    fn granted_token(outcome: RequestOutcome) -> LeaseToken {
        match outcome {
            RequestOutcome::Granted(token) => token,
            RequestOutcome::Queued { position } => {
                panic!("expected a grant, got queue position {position}")
            }
        }
    }

    /// Invariant 1: only ever one holder per resource at a time.
    #[test]
    fn only_one_holder_per_resource_at_a_time() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));

        assert!(granted(manager.request(&gpu, holder("a"), at(100))));
        assert!(queued(manager.request(&gpu, holder("b"), at(100))));
        assert!(queued(manager.request(&gpu, holder("c"), at(100))));

        let status = manager.status(&gpu, at(100));
        assert_eq!(status.holder, Some(holder("a")));
        assert_eq!(status.queue_depth, 2);
        assert_eq!(status.waiting, vec![holder("b"), holder("c")]);
    }

    /// Invariant 2: grants are strictly FIFO among waiters, through every
    /// wake-up path.
    #[test]
    fn grants_are_strictly_fifo_among_waiters() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(600));

        let a = granted_token(manager.request(&gpu, holder("a"), at(0)));
        assert!(queued(manager.request(&gpu, holder("b"), at(0))));
        assert!(queued(manager.request(&gpu, holder("c"), at(0))));
        assert!(queued(manager.request(&gpu, holder("d"), at(0))));

        let to_b = manager.release(&a, at(10)).expect("queue head is promoted");
        assert_eq!(to_b.holder, holder("b"));

        let to_c = manager
            .release(&to_b.token, at(20))
            .expect("next head is promoted");
        assert_eq!(to_c.holder, holder("c"));

        let to_d = manager
            .holder_died(&holder("c"), at(30))
            .pop()
            .expect("the death advances the queue in order");
        assert_eq!(to_d.holder, holder("d"));
    }

    /// Invariant 3: a resource is never left locked after `release`,
    /// `expire`, or `holder_died` — a waiter is granted, otherwise it is free.
    #[test]
    fn resource_is_never_left_locked_after_release_expire_or_holder_died() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));

        // release with a waiter: the waiter is granted, not left waiting on a
        // free resource.
        let a = granted_token(manager.request(&gpu, holder("a"), at(0)));
        assert!(queued(manager.request(&gpu, holder("b"), at(0))));
        let to_b = manager.release(&a, at(1)).expect("b is granted on release");
        assert_eq!(to_b.holder, holder("b"));

        // expire with a waiter: b lapses a second past its 60s hold from t=1,
        // and c, who queued behind it, is granted in the same sweep.
        assert!(queued(manager.request(&gpu, holder("c"), at(2))));
        let revoked = manager.expire(at(62));
        assert_eq!(revoked, vec![to_b.token]);
        assert_eq!(manager.status(&gpu, at(62)).holder, Some(holder("c")));

        // holder_died with no waiter: the resource is free, not locked.
        assert!(manager.holder_died(&holder("c"), at(63)).is_empty());
        let status = manager.status(&gpu, at(63));
        assert_eq!(status.holder, None);
        assert_eq!(status.queue_depth, 0);

        // and the freed resource grants a fresh requester outright.
        assert!(granted(manager.request(&gpu, holder("d"), at(64))));
    }

    /// Invariant 4: the same holder requesting twice does not get two leases.
    #[test]
    fn the_same_holder_requesting_twice_does_not_get_two_leases() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));

        // While holding: the same grant comes back.
        let first = granted_token(manager.request(&gpu, holder("a"), at(0)));
        assert_eq!(
            manager.request(&gpu, holder("a"), at(5)),
            RequestOutcome::Granted(first.clone())
        );
        assert_eq!(manager.status(&gpu, at(5)).queue_depth, 0);

        // While waiting: one queue entry, the same position reported again.
        assert!(queued(manager.request(&gpu, holder("b"), at(5))));
        assert_eq!(
            manager.request(&gpu, holder("b"), at(6)),
            RequestOutcome::Queued { position: 1 }
        );
        assert_eq!(manager.status(&gpu, at(6)).waiting, vec![holder("b")]);
    }

    /// Invariant 5: `release` with an unknown or already-released token is a
    /// harmless no-op, not a panic.
    #[test]
    fn release_with_unknown_or_already_released_token_is_a_harmless_no_op() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));

        assert!(
            manager
                .release(&LeaseToken(String::from("no-such-token")), at(0))
                .is_none()
        );

        let token = granted_token(manager.request(&gpu, holder("a"), at(0)));
        assert!(queued(manager.request(&gpu, holder("b"), at(0))));
        assert!(manager.release(&token, at(1)).is_some());
        assert!(manager.release(&token, at(2)).is_none());

        // The no-op disturbed nothing: b, promoted by the first release, holds.
        assert_eq!(manager.status(&gpu, at(2)).holder, Some(holder("b")));
    }

    /// Invariant 6, promotions: `holder_died` reports every waiter promoted,
    /// on every resource the dead holder held, in registration order, and
    /// leaves no resource holder-less with a waiting queue.
    #[test]
    fn holder_died_promotes_a_waiter_on_every_resource_it_held() {
        let mut manager = LeaseManager::new();
        let gpu0 = resource("gpu0");
        let gpu1 = resource("gpu1");
        manager.register_resource(gpu0.clone(), Duration::seconds(60));
        manager.register_resource(gpu1.clone(), Duration::seconds(60));

        assert!(granted(manager.request(&gpu0, holder("a"), at(0))));
        assert!(granted(manager.request(&gpu1, holder("a"), at(0))));
        assert!(queued(manager.request(&gpu0, holder("w0"), at(0))));
        assert!(queued(manager.request(&gpu1, holder("w1"), at(0))));

        let grants = manager.holder_died(&holder("a"), at(5));

        assert_eq!(grants.len(), 2, "every promotion is reported");
        assert_eq!(grants[0].resource, gpu0);
        assert_eq!(grants[0].holder, holder("w0"));
        assert_eq!(grants[1].resource, gpu1);
        assert_eq!(grants[1].holder, holder("w1"));

        // No resource is left holder-less with a non-empty queue.
        for gpu in [&gpu0, &gpu1] {
            let status = manager.status(gpu, at(5));
            assert!(status.holder.is_some() || status.queue_depth == 0);
        }

        // A reported grant is real: its token releases the lease it names.
        assert!(manager.release(&grants[0].token, at(6)).is_none());
        assert_eq!(manager.status(&gpu0, at(6)).holder, None);
    }

    /// Invariant 6, queue hygiene: the dead holder's queued requests are
    /// dropped, so FIFO never promotes a corpse.
    #[test]
    fn holder_died_drops_the_dead_holders_queued_requests() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));

        let a = granted_token(manager.request(&gpu, holder("a"), at(0)));
        assert!(queued(manager.request(&gpu, holder("b"), at(0))));
        assert!(queued(manager.request(&gpu, holder("c"), at(0))));
        assert_eq!(
            manager.status(&gpu, at(0)).waiting,
            vec![holder("b"), holder("c")]
        );

        // b held nothing, so no grant is reported, but its queue entry is gone.
        assert!(manager.holder_died(&holder("b"), at(1)).is_empty());
        assert_eq!(manager.status(&gpu, at(1)).waiting, vec![holder("c")]);

        let to_c = manager
            .release(&a, at(2))
            .expect("the queue advanced past the dead holder");
        assert_eq!(to_c.holder, holder("c"));
    }

    /// Time provenance: a lease handed over by `release` starts its full max
    /// hold at the moment of handover, and `held_for` measures from that
    /// stamp — never from the predecessor's.
    #[test]
    fn a_lease_handed_over_starts_its_hold_fresh_at_the_moment_of_handover() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));

        let a = granted_token(manager.request(&gpu, holder("a"), at(0)));
        assert!(queued(manager.request(&gpu, holder("b"), at(0))));

        let to_b = manager
            .release(&a, at(1_000))
            .expect("b is promoted at handover");
        assert_ne!(to_b.token, a, "a handover mints a fresh token");

        // b has held for 5s, not 1005s: the stamp is the handover, not a's.
        assert_eq!(
            manager.status(&gpu, at(1_005)).held_for,
            Some(Duration::seconds(5))
        );

        // b's hold runs from 1000: exactly 60s later it survives, one second
        // past it, it lapses.
        assert!(manager.expire(at(1_060)).is_empty());
        assert_eq!(manager.status(&gpu, at(1_060)).holder, Some(holder("b")));
        assert_eq!(manager.expire(at(1_061)), vec![to_b.token]);
        assert_eq!(manager.status(&gpu, at(1_061)).holder, None);
    }

    /// Time provenance: `expire` and `holder_died` handovers restamp too, and
    /// FIFO survives a death with several waiters queued.
    #[test]
    fn handovers_through_expire_and_holder_died_also_restamp_the_lease() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));

        assert!(granted(manager.request(&gpu, holder("a"), at(0))));
        assert!(queued(manager.request(&gpu, holder("b"), at(0))));

        // a lapses one second past its hold; b is stamped at the sweep, t=61.
        assert_eq!(manager.expire(at(61)).len(), 1);
        assert!(
            granted(manager.request(&gpu, holder("b"), at(70))),
            "b holds now"
        );
        assert_eq!(
            manager.status(&gpu, at(71)).held_for,
            Some(Duration::seconds(10)),
            "held_for counts from the handover at 61, not from a's stamp at 0"
        );

        // b dies at 80; c, queued at 75, is stamped at the death.
        assert!(queued(manager.request(&gpu, holder("c"), at(75))));
        let grants = manager.holder_died(&holder("b"), at(80));
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].holder, holder("c"));
        assert_eq!(
            manager.status(&gpu, at(85)).held_for,
            Some(Duration::seconds(5))
        );

        // d queued behind c is promoted in order when c's token is released.
        assert!(queued(manager.request(&gpu, holder("d"), at(82))));
        let to_d = manager
            .release(&grants[0].token, at(90))
            .expect("d is promoted");
        assert_eq!(to_d.holder, holder("d"));

        // d's full hold runs from 90.
        assert!(manager.expire(at(150)).is_empty());
        assert_eq!(manager.expire(at(151)).len(), 1);
    }

    /// `expire` reports exactly what it revoked, and touches nothing else.
    #[test]
    fn expire_reports_only_what_it_revoked_and_leaves_the_rest_alone() {
        let mut manager = LeaseManager::new();
        let gpu0 = resource("gpu0");
        let gpu1 = resource("gpu1");
        manager.register_resource(gpu0.clone(), Duration::seconds(60));
        manager.register_resource(gpu1.clone(), Duration::seconds(600));

        let a = granted_token(manager.request(&gpu0, holder("a"), at(0)));
        assert!(granted(manager.request(&gpu1, holder("b"), at(0))));

        let revoked = manager.expire(at(100));
        assert_eq!(revoked, vec![a], "only the lapsed lease is reported");

        // gpu1's holder is under its limit and keeps both its lease and stamp.
        let status = manager.status(&gpu1, at(100));
        assert_eq!(status.holder, Some(holder("b")));
        assert_eq!(status.held_for, Some(Duration::seconds(100)));
    }

    /// A lease held exactly its max hold is not yet lapsed: revocation is
    /// strictly past the limit.
    #[test]
    fn a_lease_held_exactly_max_hold_is_not_yet_expired() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));

        let a = granted_token(manager.request(&gpu, holder("a"), at(0)));
        assert!(manager.expire(at(59)).is_empty());
        assert!(
            manager.expire(at(60)).is_empty(),
            "exactly the limit is not past it"
        );
        assert_eq!(manager.status(&gpu, at(60)).holder, Some(holder("a")));
        assert_eq!(manager.expire(at(61)), vec![a]);
    }

    /// Re-registering a known name updates only the max hold; the live lease
    /// survives and the new limit governs its expiry.
    #[test]
    fn registering_again_updates_only_the_max_hold() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));
        let a = granted_token(manager.request(&gpu, holder("a"), at(0)));

        manager.register_resource(gpu.clone(), Duration::seconds(10));

        assert_eq!(manager.status(&gpu, at(5)).holder, Some(holder("a")));
        assert!(
            manager.expire(at(10)).is_empty(),
            "held exactly the new limit"
        );
        assert_eq!(manager.expire(at(11)), vec![a]);
    }

    /// Idle and unregistered resources snapshot as empty, and a request for
    /// an unregistered resource is queued but never granted — no panic.
    #[test]
    fn idle_and_unregistered_resources_read_as_empty_not_a_panic() {
        let mut manager = LeaseManager::new();
        let gpu = resource("gpu0");
        manager.register_resource(gpu.clone(), Duration::seconds(60));

        let idle = manager.status(&gpu, at(0));
        assert_eq!(idle.holder, None);
        assert_eq!(idle.held_for, None);
        assert_eq!(idle.queue_depth, 0);
        assert!(idle.waiting.is_empty());
        assert!(manager.expire(at(0)).is_empty());
        assert!(manager.holder_died(&holder("ghost"), at(0)).is_empty());

        let ghost = resource("ghost");
        assert!(queued(manager.request(&ghost, holder("a"), at(0))));
        let unregistered = manager.status(&ghost, at(0));
        assert_eq!(unregistered.holder, None);
        assert_eq!(unregistered.queue_depth, 0);

        // Registering later starts from a clean slate; nothing was reserved.
        manager.register_resource(ghost.clone(), Duration::seconds(60));
        assert!(granted(manager.request(&ghost, holder("b"), at(1))));
    }
}

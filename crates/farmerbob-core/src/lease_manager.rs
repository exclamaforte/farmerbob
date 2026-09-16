//! Pure, clock-independent arbitration of named resource leases.

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{LeaseId, RunId};
use crate::lease::{Lease, LeaseClass};

/// Name of a registered resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceName(pub String);

/// Identity of a process holding or requesting a lease.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HolderId(pub String);

/// Result of requesting a resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RequestOutcome {
    /// The request was granted immediately.
    Granted { lease: LeaseId, token: String },
    /// The request waits behind this many requests (one-based position).
    Queued { position: usize },
}

/// A newly granted lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    /// Resource granted.
    pub resource: ResourceName,
    /// Holder receiving the grant.
    pub holder: HolderId,
    /// Identifier of the lease.
    pub lease: LeaseId,
    /// Secret token used to release the lease.
    pub token: String,
}

/// Current occupancy and queue information for a resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaseStatus {
    /// Resource being described.
    pub resource: ResourceName,
    /// Holder when the resource has exactly one active lease.
    pub holder: Option<HolderId>,
    /// Seconds held by the sole holder, or zero for shared/multiple occupancy.
    pub held_secs: u64,
    /// Number of waiting requests.
    pub queue_depth: usize,
    /// Waiting holders in FIFO order.
    pub waiting: Vec<HolderId>,
}

#[derive(Debug)]
struct Entry {
    lease: Lease,
    holder: HolderId,
    acquired_secs: u64,
}

#[derive(Debug)]
struct ResourceState {
    limit: usize,
    last_now: u64,
    holders: Vec<Entry>,
    queue: VecDeque<HolderId>,
}

/// Manages bounded, FIFO leases for named resources.
#[derive(Debug, Default)]
pub struct LeaseManager {
    resources: HashMap<ResourceName, ResourceState>,
}

fn lease_time(seconds: u64) -> DateTime<Utc> {
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    DateTime::<Utc>::from_timestamp(seconds, 0).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

fn grant_for(resource: &ResourceName, entry: &Entry) -> Grant {
    Grant {
        resource: resource.clone(),
        holder: entry.holder.clone(),
        lease: entry.lease.id,
        token: entry.lease.token.clone(),
    }
}

impl LeaseManager {
    /// Creates an empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a resource with a concurrency limit. A limit of one is exclusive.
    pub fn register(&mut self, resource: ResourceName, limit: usize) {
        self.resources.insert(
            resource,
            ResourceState { limit, last_now: 0, holders: Vec::new(), queue: VecDeque::new() },
        );
    }

    /// Requests a lease, returning `None` for an unregistered resource.
    pub fn request(&mut self, resource: &ResourceName, holder: HolderId, now: u64) -> Option<RequestOutcome> {
        let state = self.resources.get_mut(resource)?;
        state.last_now = now;
        if let Some(entry) = state.holders.iter().find(|entry| entry.holder == holder) {
            return Some(RequestOutcome::Granted { lease: entry.lease.id, token: entry.lease.token.clone() });
        }
        if let Some(position) = state.queue.iter().position(|queued| queued == &holder) {
            return Some(RequestOutcome::Queued { position: position.saturating_add(1) });
        }
        if state.holders.len() < state.limit {
            let id = LeaseId::new();
            let token = uuid::Uuid::new_v4().to_string();
            let lease = Lease {
                id,
                resource: resource.0.clone(),
                class: if state.limit == 1 { LeaseClass::Exclusive } else { LeaseClass::Shared { vram_bytes: 0 } },
                holder: RunId::new(),
                token: token.clone(),
                acquired_at: lease_time(now),
                ttl: Duration::zero(),
            };
            state.holders.push(Entry { lease, holder, acquired_secs: now });
            Some(RequestOutcome::Granted { lease: id, token })
        } else {
            state.queue.push_back(holder);
            Some(RequestOutcome::Queued { position: state.queue.len() })
        }
    }

    /// Releases a lease by token and grants the next waiter, if any.
    pub fn release(&mut self, token: &str) -> Option<Grant> {
        for (resource, state) in &mut self.resources {
            if let Some(index) = state.holders.iter().position(|entry| entry.lease.token == token) {
                state.holders.remove(index);
                return Self::grant_next(resource, state, state.last_now);
            }
        }
        None
    }

    /// Revokes leases held longer than the limit and advances each queue.
    pub fn expire(&mut self, now: u64, max_hold_secs: u64) -> Vec<Grant> {
        let mut revoked = Vec::new();
        for (resource, state) in &mut self.resources {
            let mut kept = Vec::with_capacity(state.holders.len());
            for entry in state.holders.drain(..) {
                let held = now.saturating_sub(entry.acquired_secs);
                if held > max_hold_secs { revoked.push(grant_for(resource, &entry)); } else { kept.push(entry); }
            }
            state.holders = kept;
            while state.holders.len() < state.limit {
                if Self::grant_next(resource, state, now).is_none() { break; }
            }
        }
        revoked
    }

    /// Removes all held and queued requests belonging to a vanished holder.
    pub fn holder_died(&mut self, holder: &HolderId) -> Vec<Grant> {
        let mut removed = Vec::new();
        for (resource, state) in &mut self.resources {
            state.queue.retain(|queued| queued != holder);
            let mut kept = Vec::with_capacity(state.holders.len());
            for entry in state.holders.drain(..) {
                if &entry.holder == holder { removed.push(grant_for(resource, &entry)); } else { kept.push(entry); }
            }
            state.holders = kept;
            while state.holders.len() < state.limit {
                if Self::grant_next(resource, state, state.last_now).is_none() { break; }
            }
        }
        removed
    }

    /// Returns occupancy and FIFO waiters for a registered resource.
    pub fn status(&self, resource: &ResourceName, now: u64) -> Option<LeaseStatus> {
        let state = self.resources.get(resource)?;
        let (holder, held_secs) = if state.holders.len() == 1 {
            let entry = &state.holders[0];
            (Some(entry.holder.clone()), now.saturating_sub(entry.acquired_secs))
        } else { (None, 0) };
        Some(LeaseStatus { resource: resource.clone(), holder, held_secs, queue_depth: state.queue.len(), waiting: state.queue.iter().cloned().collect() })
    }

    fn grant_next(resource: &ResourceName, state: &mut ResourceState, acquired_secs: u64) -> Option<Grant> {
        if state.holders.len() >= state.limit { return None; }
        let holder = state.queue.pop_front()?;
        let id = LeaseId::new();
        let token = uuid::Uuid::new_v4().to_string();
        let lease = Lease { id, resource: resource.0.clone(), class: if state.limit == 1 { LeaseClass::Exclusive } else { LeaseClass::Shared { vram_bytes: 0 } }, holder: RunId::new(), token: token.clone(), acquired_at: lease_time(acquired_secs), ttl: Duration::zero() };
        let entry = Entry { lease, holder, acquired_secs };
        let grant = grant_for(resource, &entry);
        state.holders.push(entry);
        Some(grant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(name: &str) -> ResourceName { ResourceName(name.to_owned()) }
    fn holder(name: &str) -> HolderId { HolderId(name.to_owned()) }

    #[test]
    fn never_more_than_limit_holders_of_a_resource_at_once() {
        let mut manager = LeaseManager::new();
        let r = resource("gpu");
        manager.register(r.clone(), 2);
        assert!(matches!(manager.request(&r, holder("a"), 1), Some(RequestOutcome::Granted { .. })));
        assert!(matches!(manager.request(&r, holder("b"), 1), Some(RequestOutcome::Granted { .. })));
        assert!(matches!(manager.request(&r, holder("c"), 1), Some(RequestOutcome::Queued { .. })));
        assert_eq!(manager.status(&r, 1).map(|s| s.queue_depth), Some(1));
    }

    #[test]
    fn grants_are_strictly_fifo_among_waiters() {
        let mut manager = LeaseManager::new();
        let r = resource("x");
        manager.register(r.clone(), 1);
        let token = match manager.request(&r, holder("a"), 0) { Some(RequestOutcome::Granted { token, .. }) => token, _ => return };
        manager.request(&r, holder("b"), 0);
        manager.request(&r, holder("c"), 0);
        let grant = manager.release(&token);
        assert_eq!(grant.map(|g| g.holder), Some(holder("b")));
    }

    #[test]
    fn resource_is_never_left_locked_after_release_expire_or_holder_died() {
        let mut manager = LeaseManager::new();
        let r = resource("x");
        manager.register(r.clone(), 1);
        let token = match manager.request(&r, holder("a"), 0) { Some(RequestOutcome::Granted { token, .. }) => token, _ => return };
        manager.request(&r, holder("b"), 0);
        assert!(manager.release(&token).is_some());
        let btoken = match manager.request(&r, holder("b"), 0) { Some(RequestOutcome::Granted { token, .. }) => token, _ => return };
        manager.request(&r, holder("c"), 0);
        assert_eq!(manager.expire(10, 1).len(), 1);
        assert_eq!(manager.status(&r, 10).map(|s| s.holder), Some(Some(holder("c"))));
        let _ = btoken;
    }

    #[test]
    fn same_holder_requesting_twice_does_not_get_two_leases() {
        let mut manager = LeaseManager::new();
        let r = resource("x");
        manager.register(r.clone(), 2);
        let first = manager.request(&r, holder("a"), 0);
        let second = manager.request(&r, holder("a"), 1);
        assert_eq!(first, second);
        assert_eq!(manager.status(&r, 1).map(|s| s.queue_depth), Some(0));
    }

    #[test]
    fn release_unknown_or_already_released_token_is_harmless() {
        let mut manager = LeaseManager::new();
        let r = resource("x");
        manager.register(r.clone(), 1);
        let token = match manager.request(&r, holder("a"), 0) { Some(RequestOutcome::Granted { token, .. }) => token, _ => return };
        assert!(manager.release("unknown").is_none());
        manager.release(&token);
        assert!(manager.release(&token).is_none());
    }

    #[test]
    fn request_unregistered_resource_returns_none() {
        let mut manager = LeaseManager::new();
        assert!(manager.request(&resource("missing"), holder("a"), 0).is_none());
    }

    #[test]
    fn holder_died_removes_held_and_queued_requests() {
        let mut manager = LeaseManager::new();
        let r = resource("x");
        manager.register(r.clone(), 1);
        let token = match manager.request(&r, holder("a"), 0) { Some(RequestOutcome::Granted { token, .. }) => token, _ => return };
        manager.request(&r, holder("b"), 0);
        manager.request(&r, holder("a"), 0);
        assert_eq!(manager.holder_died(&holder("a")).len(), 1);
        assert_eq!(manager.status(&r, 0).map(|s| s.waiting), Some(vec![]));
        let _ = token;
    }
}

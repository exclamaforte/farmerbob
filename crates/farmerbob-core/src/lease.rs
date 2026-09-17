//! Leases: serialized access to scarce, mostly indivisible resources
//! (the GPU above all).

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













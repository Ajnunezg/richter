//! Phase 4.3 — Replay protection: nonce tracking.
//!
//! Simple nonce tracker that prevents replayed requests using a bounded `HashSet`
//! with hourly rotation when capacity is reached.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::collections::HashSet;

/// Maximum number of nonces to track (rotates when full).
pub const NONCE_CAPACITY: usize = 10_000;

/// Simple nonce tracker that prevents replayed requests.
/// Uses a HashSet for O(1) lookups with capacity-bounded rotation.
pub struct NonceTracker {
    nonces: RwLock<HashSet<String>>,
    created_at: RwLock<DateTime<Utc>>,
}

impl Default for NonceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl NonceTracker {
    pub fn new() -> Self {
        Self {
            nonces: RwLock::new(HashSet::with_capacity(NONCE_CAPACITY)),
            created_at: RwLock::new(Utc::now()),
        }
    }

    /// Check if a nonce has been seen, and insert it if not.
    /// Returns `true` if the nonce is new (allowed), `false` if replayed.
    pub fn check_and_insert(&self, nonce: &str) -> bool {
        let mut nonces = self.nonces.write();
        if nonces.contains(nonce) {
            return false;
        }
        if nonces.len() >= NONCE_CAPACITY {
            // Rotate: clear old nonces (daily rotation)
            let mut created = self.created_at.write();
            *created = Utc::now();
            nonces.clear();
        }
        nonces.insert(nonce.to_string());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonce_tracker_rejects_replay() {
        let tracker = NonceTracker::new();

        // First use is allowed
        assert!(tracker.check_and_insert("nonce-1"));
        // Replay is rejected
        assert!(!tracker.check_and_insert("nonce-1"));
        // Different nonce is allowed
        assert!(tracker.check_and_insert("nonce-2"));
    }
}

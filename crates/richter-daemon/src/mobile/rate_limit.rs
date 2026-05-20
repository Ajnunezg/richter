//! Phase 4.5 — Rate limiting: token bucket per device.
//!
//! Per-device rate limiter with configurable token bucket parameters.
//! Default: 60 requests/minute (1 request/second refill rate).

use parking_lot::RwLock;
use std::collections::HashMap;

/// Token bucket for rate limiting.
#[derive(Debug)]
pub(crate) struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64, // tokens per second
    last_refill: std::time::Instant,
}

impl TokenBucket {
    fn new(max_tokens: f64, refill_per_sec: f64) -> Self {
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate: refill_per_sec,
            last_refill: std::time::Instant::now(),
        }
    }

    /// Try to consume one token. Returns true if allowed.
    fn try_consume(&mut self) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Seconds until the next token is available.
    fn retry_after_secs(&self) -> f64 {
        if self.tokens >= 1.0 {
            0.0
        } else {
            (1.0 - self.tokens) / self.refill_rate
        }
    }
}

/// Per-device rate limiter: 60 requests/minute.
pub struct RateLimiter {
    buckets: RwLock<HashMap<String, TokenBucket>>,
    max_tokens: f64,
    refill_per_sec: f64,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(60.0, 1.0) // 60 req/min = 1 req/sec refill
    }
}

impl RateLimiter {
    pub fn new(max_tokens: f64, refill_per_sec: f64) -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
            max_tokens,
            refill_per_sec,
        }
    }

    /// Check if a device is allowed to make a request.
    /// Returns `Some(retry_after_secs)` if rate-limited, `None` if allowed.
    pub fn check(&self, device_id: &str) -> Option<f64> {
        let mut buckets = self.buckets.write();
        let bucket = buckets
            .entry(device_id.to_string())
            .or_insert_with(|| TokenBucket::new(self.max_tokens, self.refill_per_sec));

        if bucket.try_consume() {
            None
        } else {
            Some(bucket.retry_after_secs())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter() {
        let limiter = RateLimiter::new(5.0, 100.0); // 5 tokens, 100/sec refill

        // First 5 requests should succeed
        for _ in 0..5 {
            assert!(limiter.check("device-1").is_none());
        }
        // 6th should be rate-limited
        let retry = limiter.check("device-1");
        assert!(retry.is_some());
        assert!(retry.unwrap() > 0.0);

        // Different device should not be rate-limited
        assert!(limiter.check("device-2").is_none());
    }
}

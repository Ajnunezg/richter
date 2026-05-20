//! Rate limiting for the Richter daemon API.
//!
//! Token bucket rate limiter with per-client tracking and configurable limits.
//! Returns 429 Too Many Requests with Retry-After headers when exceeded.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::Instant;

/// Default requests-per-minute for the main API.
const DEFAULT_RPM: u64 = 300;

/// Token bucket for a single client.
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(max_tokens: u64, refill_per_sec: f64) -> Self {
        Self {
            tokens: max_tokens as f64,
            max_tokens: max_tokens as f64,
            refill_rate: refill_per_sec,
            last_refill: Instant::now(),
        }
    }

    fn try_consume(&mut self) -> bool {
        let now = Instant::now();
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

    fn retry_after_secs(&self) -> f64 {
        if self.tokens >= 1.0 {
            0.0
        } else {
            (1.0 - self.tokens) / self.refill_rate
        }
    }
}

/// Per-client rate limiter using token bucket algorithm.
pub struct RateLimiter {
    buckets: RwLock<HashMap<String, TokenBucket>>,
    max_tokens: u64,
    refill_per_sec: f64,
}

impl Default for RateLimiter {
    fn default() -> Self {
        let rpm = std::env::var("RICHTER_RATE_LIMIT_RPM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_RPM);
        Self::new(rpm)
    }
}

impl RateLimiter {
    pub fn new(requests_per_minute: u64) -> Self {
        let refill_per_sec = requests_per_minute as f64 / 60.0;
        Self {
            buckets: RwLock::new(HashMap::new()),
            max_tokens: requests_per_minute,
            refill_per_sec,
        }
    }

    /// Check if a client is allowed to make a request.
    /// Returns `Some(retry_after_secs)` if rate-limited, `None` if allowed.
    pub fn check(&self, client_id: &str) -> Option<f64> {
        let mut buckets = self.buckets.write();
        let bucket = buckets
            .entry(client_id.to_string())
            .or_insert_with(|| TokenBucket::new(self.max_tokens, self.refill_per_sec));

        if bucket.try_consume() {
            None
        } else {
            Some(bucket.retry_after_secs())
        }
    }

    /// Periodically clean up stale bucket entries.
    pub fn cleanup(&self) {
        let mut buckets = self.buckets.write();
        let threshold = Instant::now() - std::time::Duration::from_secs(600); // 10 min
        buckets.retain(|_, bucket| bucket.last_refill > threshold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let limiter = RateLimiter::new(100); // 100 req/min = ~1.67 req/sec
        for _ in 0..100 {
            assert!(
                limiter.check("client-1").is_none(),
                "should allow within limit"
            );
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let limiter = RateLimiter::new(5); // 5 req/min
        for _ in 0..5 {
            assert!(limiter.check("client-1").is_none());
        }
        let blocked = limiter.check("client-1");
        assert!(blocked.is_some(), "should block over limit");
        assert!(blocked.unwrap() > 0.0, "should have positive retry-after");
    }

    #[test]
    fn test_different_clients_independent() {
        let limiter = RateLimiter::new(3);
        // Exhaust client-1
        for _ in 0..3 {
            limiter.check("client-1");
        }
        assert!(
            limiter.check("client-1").is_some(),
            "client-1 should be blocked"
        );
        // client-2 should still be allowed
        assert!(
            limiter.check("client-2").is_none(),
            "client-2 should be allowed"
        );
    }

    #[test]
    fn test_cleanup_removes_stale_buckets() {
        let limiter = RateLimiter::new(10);
        limiter.check("client-1");

        // Manually age the bucket
        {
            let mut buckets = limiter.buckets.write();
            if let Some(bucket) = buckets.get_mut("client-1") {
                bucket.last_refill = Instant::now() - std::time::Duration::from_secs(601);
            }
        }

        limiter.cleanup();
        assert_eq!(
            limiter.buckets.read().len(),
            0,
            "stale buckets should be cleaned up"
        );
    }
}

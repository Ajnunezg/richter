//! Retry with exponential backoff and jitter.
//!
//! Provides a generic retry wrapper for fallible operations with configurable
//! backoff strategy. Used for DB writes, git commands, and network calls.

use std::time::Duration;
use tracing::warn;

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct BackoffConfig {
    /// Initial delay before first retry.
    pub initial_delay: Duration,
    /// Maximum delay between retries.
    pub max_delay: Duration,
    /// Multiplier applied to the delay after each attempt.
    pub multiplier: f64,
    /// Maximum number of attempts (including the initial call).
    pub max_attempts: u32,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
            multiplier: 10.0,
            max_attempts: 3,
        }
    }
}

impl BackoffConfig {
    /// A config suitable for DB write operations.
    pub fn db_writes() -> Self {
        Self {
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(500),
            multiplier: 4.0,
            max_attempts: 3,
        }
    }

    /// A config suitable for git commands.
    pub fn git_commands() -> Self {
        Self {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 3.0,
            max_attempts: 3,
        }
    }
}

/// Execute an operation with exponential backoff and jitter.
///
/// Calls `op` up to `config.max_attempts` times. On failure, waits with
/// exponential backoff plus a small random jitter before retrying.
/// Returns the first successful result, or the last error if all attempts fail.
pub fn retry<T, E>(mut op: impl FnMut() -> Result<T, E>, config: &BackoffConfig) -> Result<T, E>
where
    E: std::fmt::Display,
{
    let mut delay = config.initial_delay;
    let mut last_err: Option<E> = None;

    for attempt in 0..config.max_attempts {
        match op() {
            Ok(val) => return Ok(val),
            Err(e) => {
                if attempt + 1 < config.max_attempts {
                    // Add jitter: random 0-20% of delay
                    let jitter = delay.mul_f64(0.2 * random_01());
                    let wait = delay + jitter;
                    warn!(
                        "Attempt {}/{} failed: {}. Retrying in {:?}",
                        attempt + 1,
                        config.max_attempts,
                        e,
                        wait
                    );
                    std::thread::sleep(wait);
                    delay = (delay.mul_f64(config.multiplier)).min(config.max_delay);
                }
                last_err = Some(e);
            }
        }
    }

    Err(last_err.unwrap())
}

/// Quick retry with default config.
pub fn retry_default<T, E>(op: impl FnMut() -> Result<T, E>) -> Result<T, E>
where
    E: std::fmt::Display,
{
    retry(op, &BackoffConfig::default())
}

/// Generate a pseudo-random number in [0, 1) without pulling in rand.
fn random_01() -> f64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1000) as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn succeeds_on_first_try() {
        let result = retry(|| Ok::<i32, &str>(42), &BackoffConfig::default());
        assert_eq!(result, Ok(42));
    }

    #[test]
    fn retries_and_succeeds() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let config = BackoffConfig {
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            multiplier: 2.0,
            max_attempts: 5,
        };
        let result = retry(
            move || {
                let n = attempts_clone.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err("not yet")
                } else {
                    Ok(99)
                }
            },
            &config,
        );
        assert_eq!(result, Ok(99));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn exhausts_retries() {
        let config = BackoffConfig {
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            multiplier: 2.0,
            max_attempts: 3,
        };
        let result = retry(|| Err::<i32, &str>("always fails"), &config);
        assert_eq!(result, Err("always fails"));
    }
}

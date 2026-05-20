//! LLM-based importance boosting via HTTP (OpenAI-compatible API).
//!
//! Provides `HTTPModelBoost` that calls an LLM endpoint to refine severity
//! classification. Degrades gracefully on failure and includes a circuit
//! breaker and budget tracking.

use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

use super::pipeline::ModelBoostProvider;
use super::{ParseResult, Severity};
use crate::api::ModelCallBudget;

// --- Circuit breaker ---

/// Tracks consecutive failures and disables for a cooldown period.
struct CircuitBreaker {
    consecutive_failures: AtomicU32,
    disabled_until: parking_lot::Mutex<Option<std::time::Instant>>,
    /// Max consecutive failures before opening.
    max_failures: u32,
    /// Cooldown duration after opening.
    cooldown: Duration,
}

impl CircuitBreaker {
    fn new(max_failures: u32, cooldown: Duration) -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            disabled_until: parking_lot::Mutex::new(None),
            max_failures,
            cooldown,
        }
    }

    /// Returns true if the circuit is closed (calls allowed).
    fn is_closed(&self) -> bool {
        let guard = self.disabled_until.lock();
        if let Some(until) = *guard {
            if std::time::Instant::now() < until {
                return false;
            }
        }
        // Cooldown expired — reset if it was set.
        drop(guard);
        // Clear the disabled_until if cooldown passed.
        let mut guard = self.disabled_until.lock();
        if let Some(until) = *guard {
            if std::time::Instant::now() >= until {
                *guard = None;
                self.consecutive_failures.store(0, Ordering::SeqCst);
            }
        }
        true
    }

    /// Record a successful call — resets the counter.
    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
    }

    /// Record a failed call. Returns true if the circuit just opened.
    fn record_failure(&self) -> bool {
        let count = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        if count >= self.max_failures {
            let mut guard = self.disabled_until.lock();
            *guard = Some(std::time::Instant::now() + self.cooldown);
            true
        } else {
            false
        }
    }
}

// --- LLM response schema ---

#[derive(Debug, serde::Deserialize)]
struct LlmSeverityResponse {
    severity: String,
    #[allow(dead_code)]
    reason: Option<String>,
}

// --- HTTPModelBoost ---

/// An LLM-based model boost provider that calls an OpenAI-compatible HTTP API.
///
/// Configured via environment variables:
/// - `RICHTER_LLM_ENDPOINT` — base URL (required to enable).
/// - `RICHTER_LLM_API_KEY` — bearer token (optional).
/// - `RICHTER_LLM_MODEL` — model name (default: `gpt-4o-mini`).
/// - `RICHTER_LLM_MAX_TOKENS` — max tokens in response (default: `256`).
pub struct HTTPModelBoost {
    endpoint: String,
    api_key: Option<String>,
    model: String,
    max_tokens: u32,
    client: reqwest::Client,
    budget: Option<Arc<parking_lot::Mutex<ModelCallBudget>>>,
    circuit_breaker: CircuitBreaker,
}

impl HTTPModelBoost {
    /// Circuit breaker: open after N consecutive failures.
    const CIRCUIT_BREAKER_MAX_FAILURES: u32 = 3;
    /// Circuit breaker cooldown period.
    const CIRCUIT_BREAKER_COOLDOWN: Duration = Duration::from_secs(300); // 5 minutes

    /// Build from environment variables. Returns `None` if no endpoint
    /// is configured (meaning the feature is disabled).
    pub fn from_env() -> Option<Self> {
        let endpoint = std::env::var("RICHTER_LLM_ENDPOINT").ok()?;
        if endpoint.is_empty() {
            return None;
        }

        let api_key = std::env::var("RICHTER_LLM_API_KEY").ok();
        let model = std::env::var("RICHTER_LLM_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "gpt-4o-mini".to_string());
        let max_tokens: u32 = std::env::var("RICHTER_LLM_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256);

        let timeout_secs: u64 = std::env::var("RICHTER_LLM_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Some(Self {
            endpoint,
            api_key,
            model,
            max_tokens,
            client,
            budget: None,
            circuit_breaker: CircuitBreaker::new(
                Self::CIRCUIT_BREAKER_MAX_FAILURES,
                Self::CIRCUIT_BREAKER_COOLDOWN,
            ),
        })
    }

    /// Attach a budget tracker. When set, `boost()` will check budget
    /// before each call.
    pub fn with_budget(mut self, budget: Arc<parking_lot::Mutex<ModelCallBudget>>) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Build the prompt payload for the LLM call.
    fn build_payload(&self, severity: Severity, result: &ParseResult) -> serde_json::Value {
        let severity_str = format!("{:?}", severity);
        let output_summary = &result.reason;
        let truncated = if output_summary.len() > 2000 {
            &output_summary[..2000]
        } else {
            output_summary
        };
        let first_failure = result.first_failure.as_deref().unwrap_or("none");

        let user_message = format!(
            "Command output analysis:\n\
             - Deterministic severity: {severity_str}\n\
             - Failure count: {failure_count}\n\
             - First failure: {first_failure}\n\
             - Output summary: {output_summary}\n\n\
             Classify the severity as one of: critical, high, medium, low, info.\n\
             Respond with JSON only: {{\"severity\": \"...\", \"reason\": \"...\"}}",
            severity_str = severity_str,
            failure_count = result.failure_count,
            first_failure = first_failure,
            output_summary = truncated,
        );

        serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a CI/CD output classifier. Given a command output summary, determine the severity level."
                },
                {
                    "role": "user",
                    "content": user_message,
                }
            ],
            "max_tokens": self.max_tokens,
            "temperature": 0.1,
            "response_format": {
                "type": "json_object"
            }
        })
    }

    /// Parse the LLM response JSON and extract severity.
    fn parse_response(&self, body: &str) -> Option<Severity> {
        // Try parsing the full OpenAI chat completion response first.
        if let Ok(chat_resp) = serde_json::from_str::<serde_json::Value>(body) {
            // OpenAI/Anthropic format: choices[0].message.content
            if let Some(content) = chat_resp["choices"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|c| c["message"]["content"].as_str())
            {
                if let Some(s) = self.severity_from_json_str(content) {
                    return Some(s);
                }
            }
            // Fallback: try to parse the whole body as our schema.
            if let Some(s) = self.severity_from_json_str(body) {
                return Some(s);
            }
        }
        warn!("HTTPModelBoost: failed to parse LLM response body");
        None
    }

    /// Extract severity from a JSON string that should contain `{"severity": "..."}`.
    fn severity_from_json_str(&self, json_str: &str) -> Option<Severity> {
        let parsed: LlmSeverityResponse = serde_json::from_str(json_str).ok()?;
        match parsed.severity.to_lowercase().as_str() {
            "critical" => Some(Severity::Critical),
            "high" => Some(Severity::High),
            "medium" => Some(Severity::Medium),
            "low" => Some(Severity::Low),
            "info" => Some(Severity::Low), // Map info → Low
            _ => None,
        }
    }
}

#[async_trait]
impl ModelBoostProvider for HTTPModelBoost {
    async fn boost(&self, severity: Severity, result: &ParseResult) -> Severity {
        // If endpoint is empty, bail immediately.
        if self.endpoint.is_empty() {
            return severity;
        }

        // Circuit breaker check.
        if !self.circuit_breaker.is_closed() {
            debug!("HTTPModelBoost: circuit breaker open, skipping LLM call");
            return severity;
        }

        // Budget check.
        if let Some(ref budget) = self.budget {
            if !budget.lock().try_consume() {
                debug!("HTTPModelBoost: budget exhausted, skipping LLM call");
                return severity;
            }
        }

        let payload = self.build_payload(severity, result);

        debug!(
            target: "llm_prompt",
            endpoint = %self.endpoint,
            model = %self.model,
            severity = ?severity,
            "Sending LLM importance boost request",
        );

        // Build the request.
        let mut req = self.client.post(&self.endpoint).json(&payload);

        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }

        // Execute with timeout already set on the client.
        match req.send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!(
                        status = %resp.status(),
                        "HTTPModelBoost: LLM API returned non-200 status"
                    );
                    if self.circuit_breaker.record_failure() {
                        warn!(
                            "HTTPModelBoost: circuit breaker OPEN after {} consecutive failures",
                            Self::CIRCUIT_BREAKER_MAX_FAILURES
                        );
                    }
                    return severity;
                }

                match resp.text().await {
                    Ok(body) => {
                        debug!(
                            target: "llm_response",
                            body_len = body.len(),
                            "LLM response received"
                        );
                        match self.parse_response(&body) {
                            Some(boosted) => {
                                self.circuit_breaker.record_success();
                                if boosted != severity {
                                    debug!(
                                        original = ?severity,
                                        boosted = ?boosted,
                                        "HTTPModelBoost: severity changed"
                                    );
                                }
                                boosted
                            }
                            None => {
                                warn!("HTTPModelBoost: could not parse severity from response");
                                let _ = self.circuit_breaker.record_failure();
                                severity
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "HTTPModelBoost: failed to read response body");
                        let _ = self.circuit_breaker.record_failure();
                        severity
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "HTTPModelBoost: LLM API call failed");
                if self.circuit_breaker.record_failure() {
                    warn!(
                        "HTTPModelBoost: circuit breaker OPEN after {} consecutive failures",
                        Self::CIRCUIT_BREAKER_MAX_FAILURES
                    );
                }
                severity
            }
        }
    }

    fn name(&self) -> &'static str {
        "http-llm"
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize env-var-dependent tests to prevent races.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_from_env_no_config() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("RICHTER_LLM_ENDPOINT");
        std::env::remove_var("RICHTER_LLM_MODEL");
        std::env::remove_var("RICHTER_LLM_MAX_TOKENS");
        std::env::remove_var("RICHTER_LLM_API_KEY");
        std::env::remove_var("RICHTER_LLM_TIMEOUT");
        let result = HTTPModelBoost::from_env();
        assert!(
            result.is_none(),
            "Should return None when endpoint is not set"
        );
    }

    #[test]
    fn test_from_env_empty_endpoint() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var("RICHTER_LLM_ENDPOINT", "");
        let result = HTTPModelBoost::from_env();
        assert!(
            result.is_none(),
            "Should return None when endpoint is empty"
        );
        std::env::remove_var("RICHTER_LLM_ENDPOINT");
    }

    #[test]
    fn test_from_env_with_endpoint() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var(
            "RICHTER_LLM_ENDPOINT",
            "http://localhost:11434/v1/chat/completions",
        );
        let result = HTTPModelBoost::from_env();
        assert!(result.is_some(), "Should return Some when endpoint is set");
        let boost = result.unwrap();
        assert_eq!(boost.name(), "http-llm");
        assert_eq!(boost.model, "gpt-4o-mini");
        assert_eq!(boost.max_tokens, 256);
        std::env::remove_var("RICHTER_LLM_ENDPOINT");
    }

    #[test]
    fn test_from_env_custom_model() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::set_var(
            "RICHTER_LLM_ENDPOINT",
            "http://localhost:11434/v1/chat/completions",
        );
        std::env::set_var("RICHTER_LLM_MODEL", "llama3");
        std::env::set_var("RICHTER_LLM_MAX_TOKENS", "128");
        let result = HTTPModelBoost::from_env().unwrap();
        assert_eq!(result.model, "llama3");
        assert_eq!(result.max_tokens, 128);
        std::env::remove_var("RICHTER_LLM_ENDPOINT");
        std::env::remove_var("RICHTER_LLM_MODEL");
        std::env::remove_var("RICHTER_LLM_MAX_TOKENS");
    }

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(300));
        assert!(cb.is_closed());
    }

    #[test]
    fn test_circuit_breaker_opens_after_max_failures() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(300));
        assert!(!cb.record_failure()); // 1
        assert!(!cb.record_failure()); // 2
        assert!(cb.record_failure()); // 3 → opens
        assert!(!cb.is_closed());
    }

    #[test]
    fn test_circuit_breaker_resets_on_success() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(300));
        cb.record_failure(); // 1
        cb.record_failure(); // 2
        cb.record_success(); // reset
        assert_eq!(cb.consecutive_failures.load(Ordering::SeqCst), 0);
        assert!(cb.is_closed());
    }

    #[test]
    fn test_severity_parse_all_levels() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // We need a client to test parse_response. Build a minimal one.
        let endpoint = "http://localhost:11434/v1/chat/completions";
        std::env::set_var("RICHTER_LLM_ENDPOINT", endpoint);
        let boost = HTTPModelBoost::from_env().unwrap();
        std::env::remove_var("RICHTER_LLM_ENDPOINT");

        // Test OpenAI-style response.
        let openai_body = r#"{"choices":[{"message":{"content":"{\"severity\":\"critical\",\"reason\":\"widespread failures\"}"}}]}"#;
        assert_eq!(boost.parse_response(openai_body), Some(Severity::Critical));

        // Test direct JSON response.
        let direct_body = r#"{"severity":"high","reason":"test failure"}"#;
        assert_eq!(boost.parse_response(direct_body), Some(Severity::High));

        // Test medium.
        let medium_body = r#"{"severity":"medium","reason":"something"}"#;
        assert_eq!(boost.parse_response(medium_body), Some(Severity::Medium));

        // Test low.
        let low_body = r#"{"severity":"low","reason":"minor"}"#;
        assert_eq!(boost.parse_response(low_body), Some(Severity::Low));

        // Test info (maps to Low).
        let info_body = r#"{"severity":"info","reason":"just info"}"#;
        assert_eq!(boost.parse_response(info_body), Some(Severity::Low));

        // Test invalid JSON.
        assert_eq!(boost.parse_response("not json"), None);

        // Test unknown severity.
        let unknown_body = r#"{"severity":"unknown","reason":"???"}"#;
        assert_eq!(boost.parse_response(unknown_body), None);
    }

    #[test]
    fn test_build_payload() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let endpoint = "http://localhost:11434/v1/chat/completions";
        std::env::set_var("RICHTER_LLM_ENDPOINT", endpoint);
        let boost = HTTPModelBoost::from_env().unwrap();
        std::env::remove_var("RICHTER_LLM_ENDPOINT");

        let result = ParseResult {
            failure_count: 2,
            first_failure: Some("test_auth_flow FAILED".into()),
            reason: "2 tests failed in auth module".into(),
            changed_files: vec![],
            metadata: serde_json::json!({}),
        };

        let payload = boost.build_payload(Severity::High, &result);
        let msg = &payload["messages"][1]["content"].as_str().unwrap();
        assert!(msg.contains("High"));
        assert!(msg.contains("2"));
        assert!(msg.contains("test_auth_flow FAILED"));
        assert!(msg.contains("2 tests failed in auth module"));
        assert_eq!(payload["temperature"], 0.1);
        assert_eq!(payload["max_tokens"], 256);
    }

    #[test]
    fn test_boost_empty_endpoint_returns_original() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let boost = HTTPModelBoost {
            endpoint: String::new(),
            api_key: None,
            model: "gpt-4o-mini".into(),
            max_tokens: 256,
            client: reqwest::Client::new(),
            budget: None,
            circuit_breaker: CircuitBreaker::new(3, Duration::from_secs(300)),
        };
        let result = ParseResult {
            failure_count: 0,
            first_failure: None,
            reason: "ok".into(),
            changed_files: vec![],
            metadata: serde_json::json!({}),
        };
        let boosted = rt.block_on(boost.boost(Severity::High, &result));
        assert_eq!(boosted, Severity::High);
    }
}

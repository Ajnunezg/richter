//! Secrets redaction engine.
//!
//! Detects and redacts secrets from text before storage or model calls:
//! API keys, bearer tokens, private keys, GitHub tokens, OpenAI/Anthropic/
//! DeepSeek keys, AWS/GCP/Azure credentials, cookies, and database URLs.
//! Uses pattern-based (regex) redaction. Conservative: over-redacts rather
//! than under-redacts.

use regex::Regex;
use std::sync::LazyLock;

/// The replacement string used for redacted content.
pub const REDACTION_REPLACEMENT: &str = "[REDACTED]";

/// Build the secret detection regex patterns.
fn build_patterns() -> Vec<(Regex, &'static str)> {
    let patterns: Vec<(&str, &str)> = vec![
        // API key patterns (key=value style)
        (
            r"(?i)(api[_-]?key|apikey|api[_-]?secret)\s*[:=]\s*\S+",
            "API key",
        ),
        (
            r"(?i)(auth[_-]?token|access[_-]?token|bearer[_-]?token)\s*[:=]\s*\S+",
            "auth token",
        ),
        // Bearer tokens in Authorization headers
        (r"(?i)authorization\s*:\s*bearer\s+\S+", "bearer token"),
        (r"(?i)authorization\s*:\s*basic\s+\S+", "basic auth"),
        // Private key patterns (BEGIN ... END blocks)
        (
            r"-----BEGIN (?:RSA|DSA|EC|OPENSSH|PGP) PRIVATE KEY-----[\s\S]*?-----END (?:RSA|DSA|EC|OPENSSH|PGP) PRIVATE KEY-----",
            "private key",
        ),
        // GitHub tokens
        (r"gh[pousr]_[A-Za-z0-9_]{36,}", "GitHub token"),
        (r"(?i)github[_-]?token\s*[:=]\s*\S+", "GitHub token"),
        // OpenAI keys
        (r"sk-[A-Za-z0-9-_]{32,}", "OpenAI key"),
        (r"sk-proj-[A-Za-z0-9-_]{32,}", "OpenAI project key"),
        (r"sk-admin-[A-Za-z0-9-_]{32,}", "OpenAI admin key"),
        // Anthropic keys
        (r"sk-ant-[A-Za-z0-9-_]{32,}", "Anthropic key"),
        // DeepSeek-like keys (catch after more specific patterns)
        (r"sk-[a-z0-9]{32,}", "DeepSeek-like key"),
        // AWS credentials
        (r"AKIA[0-9A-Z]{16}", "AWS access key"),
        (
            r"(?i)aws[_-]?secret[_-]?access[_-]?key\s*[:=]\s*\S+",
            "AWS secret key",
        ),
        (
            r"(?i)aws[_-]?session[_-]?token\s*[:=]\s*\S+",
            "AWS session token",
        ),
        // GCP credentials
        (
            r"(?i)(gcp|google)[_-]?(credentials|key|secret)\s*[:=]\s*\S+",
            "GCP credentials",
        ),
        // Azure credentials
        (
            r"(?i)azure[_-]?(key|secret|connection[_-]?string)\s*[:=]\s*\S+",
            "Azure key",
        ),
        (
            r"(?i)DefaultEndpointsProtocol=https;AccountName=[^;]+;AccountKey=[^;]+",
            "Azure storage connection string",
        ),
        // Generic database URLs with credentials
        (
            r"(?i)(?:postgres(?:ql)?|mysql|mongodb|redis|sqlite)://[^:]+:[^@]+@\S+",
            "database URL with credentials",
        ),
        // JWT tokens
        (
            r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
            "JWT token",
        ),
        // Generic password in config
        (r"(?i)password\s*[:=]\s*\S+", "password"),
        (r"(?i)passwd\s*[:=]\s*\S+", "password"),
        (r"(?i)secret\s*[:=]\s*\S{8,}", "secret"),
        // Cookies with sensitive-looking values
        (r"(?i)cookie\s*[:=]\s*\S{20,}", "cookie"),
        // Stripe keys
        (r"sk_live_[0-9a-zA-Z]{24,}", "Stripe live key"),
        (r"pk_live_[0-9a-zA-Z]{24,}", "Stripe publishable key"),
        // Slack tokens
        (r"xox[bp]-[0-9a-zA-Z-]{10,}", "Slack token"),
        // Generic tokens and keys (conservative catch-all)
        (
            r"(?i)(?:token|key|secret)\s*[:=]\s*[A-Za-z0-9+/=_-]{20,}",
            "generic token/key",
        ),
    ];

    patterns
        .into_iter()
        .map(|(pat, name)| (Regex::new(pat).expect("invalid regex pattern"), name))
        .collect()
}

/// Get the compiled regex patterns (lazily initialized).
fn patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(build_patterns);
    &PATTERNS
}

/// Redact all detected secrets from a string.
///
/// Returns the redacted text.
pub fn redact(input: &str) -> String {
    let mut working = input.to_string();
    for (re, _name) in patterns().iter() {
        working = re.replace_all(&working, REDACTION_REPLACEMENT).to_string();
    }
    working
}

/// Redact all detected secrets from a string and return the count of redactions.
pub fn redact_with_count(input: &str) -> (String, usize) {
    let mut working = input.to_string();
    let mut total = 0;
    for (re, _name) in patterns().iter() {
        total += re.find_iter(&working).count();
        working = re.replace_all(&working, REDACTION_REPLACEMENT).to_string();
    }
    (working, total)
}

/// Check if a string contains any secrets.
pub fn contains_secrets(input: &str) -> bool {
    patterns().iter().any(|(re, _)| re.is_match(input))
}

/// Redact secrets from a byte slice, returning a string.
pub fn redact_bytes(input: &[u8]) -> String {
    let text = String::from_utf8_lossy(input);
    redact(&text)
}

/// Redact a JSON value recursively, replacing secret strings.
pub fn redact_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if contains_secrets(s) {
                serde_json::Value::String(REDACTION_REPLACEMENT.to_string())
            } else {
                value.clone()
            }
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(redact_json).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut new_obj = serde_json::Map::new();
            for (k, v) in obj {
                new_obj.insert(k.clone(), redact_json(v));
            }
            serde_json::Value::Object(new_obj)
        }
        _ => value.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_openai_key() {
        let input = "OPENAI_API_KEY=sk-proj-fake-openai-test-key-0000000000";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
        assert!(!output.contains("sk-proj-"));
    }

    #[test]
    fn test_redact_anthropic_key() {
        let input = "Using key: sk-ant-api03-fake-anthropic-key-0000000000";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }

    #[test]
    fn test_redact_github_token() {
        let input = "GITHUB_TOKEN=ghp_faketesttoken0000000000000000";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }

    #[test]
    fn test_redact_aws_key() {
        let input = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }

    #[test]
    fn test_redact_bearer_token() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }

    #[test]
    fn test_redact_jwt() {
        let input = "token=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0.Gfx6VO67tcE5YHG8HHP2TQ";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }

    #[test]
    fn test_redact_database_url() {
        let input = "DATABASE_URL=postgres://user:password123@localhost:5432/mydb";
        let output = redact(input);
        assert!(!output.contains("password123"));
    }

    #[test]
    fn test_redact_private_key() {
        let input = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA0Z3...\n-----END RSA PRIVATE KEY-----";
        let output = redact(input);
        assert!(!output.contains("BEGIN RSA PRIVATE KEY"));
    }

    #[test]
    fn test_redact_password_in_config() {
        let input = "db_password = superscretpass123";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }

    #[test]
    fn test_no_false_positive_normal_text() {
        let input = "This is normal text with some numbers 12345 and nothing sensitive.";
        let output = redact(input);
        assert_eq!(output, input);
    }

    #[test]
    fn test_contains_secrets() {
        assert!(contains_secrets(
            "sk-proj-fake-openai-test-key-0000000000"
        ));
        assert!(!contains_secrets("Hello, world!"));
    }

    #[test]
    fn test_redact_json_nested() {
        let input = serde_json::json!({
            "config": {
                "api_key": "sk-proj-fake-openai-test-key-0000000000",
                "normal_field": "hello"
            },
            "items": ["safe", "sk-ant-api03-fake-anthropic-key-0000000000"]
        });
        let output = redact_json(&input);
        let config = output.get("config").unwrap();
        assert_eq!(
            config.get("api_key").unwrap().as_str().unwrap(),
            REDACTION_REPLACEMENT
        );
        assert_eq!(
            config.get("normal_field").unwrap().as_str().unwrap(),
            "hello"
        );
        let items = output.get("items").unwrap().as_array().unwrap();
        assert_eq!(items[0].as_str().unwrap(), "safe");
        assert_eq!(items[1].as_str().unwrap(), REDACTION_REPLACEMENT);
    }

    #[test]
    fn test_redact_slack_token() {
        let input = "SLACK_TOKEN=xoxb-fake-test-token-0000000000";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }

    #[test]
    fn test_redact_stripe_live_key() {
        let input = "STRIPE_SECRET_KEY=sk_live_faketestkey000000000";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }

    #[test]
    fn test_redact_multiple_secrets() {
        let input =
            "API_KEY=sk-fake-api-key-000000000000000000\nGITHUB_TOKEN=ghp_examplenotreal12345";
        let (_output, count) = redact_with_count(input);
        assert!(count >= 2, "expected at least 2 redactions, got {count}");
    }

    #[test]
    fn test_redact_azure_conn_string() {
        let input =
            "DefaultEndpointsProtocol=https;AccountName=mystorage;AccountKey=abc123def456==";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }

    #[test]
    fn test_redact_generic_token() {
        let input = "secret_token = A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8S9t0U1v2W3x4Y5z6";
        let output = redact(input);
        assert!(output.contains(REDACTION_REPLACEMENT));
    }
}

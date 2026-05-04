# ADR 0004: LLM Importance Pipeline Design

**Status**: Accepted
**Date**: 2026-05-04
**Author**: Alberto Nunez
**Deciders**: Alberto Nunez

## Context

Richter's event system must surface important events to the user without
flooding them. A busy multi-agent session could produce hundreds of events
per hour — most of them routine (tests passed, builds completed). The user
needs to see test failures, build errors, resource conflicts, and other
genuinely important events without wading through noise.

The challenge: how do we classify event importance? We can parse structured
output (JUnit, TAP) deterministically, but unstructured output (custom test
runners, arbitrary log output, complex error messages) requires more
sophisticated classification.

We could use LLMs for this, but that raises questions: which models? At what
cost? With what guarantees? And critically — is this even necessary, or can
we ship without it?

## Decision

Richter uses a **three-tier importance pipeline**:

1. **Tier 1 (Deterministic)** — always active, no model required.
2. **Tier 2 (Cheap Model)** — optional, configurable.
3. **Tier 3 (Frontier Model)** — optional, budget-limited, only for
   ambiguous high-impact cases.

The system must work without any model configured. Models are an enhancement,
not a dependency.

### Tier 1: Deterministic (Always Active)

Parse structured output formats:

| Tool/Format | Parsed Fields | Importance Mapping |
|---|---|---|
| JUnit XML | test count, failures, errors, skipped, first failure message | `failure > 0 → 80`, `error > 0 → 85`, all pass → 5 |
| TAP | plan, pass/fail lines | `not ok → 75`, all ok → 5 |
| Cargo test | test result summary, failure locations | `FAILED → 80`, all ok → 5 |
| pytest | failure reports, tracebacks | `FAILED → 80`, all ok → 5 |
| ESLint | error/warning counts, rule names | `errors > 0 → 60`, `warnings > 0 → 30` |
| tsc | error count, first error | `errors > 0 → 55` |
| Go test | `--- FAIL` lines, summary | `FAIL → 75` |
| Xcodebuild | test summary, build errors | `** TEST FAILED ** → 80` |
| Bazel | test summary, build failure targets | `FAILED → 75` |

Deterministic scores are mapped to importance (0-100):

- 0-10: Routine pass, no action needed.
- 11-30: Minor issue (warnings, non-blocking).
- 31-60: Notable issue (lint errors, type errors).
- 61-85: Important (test failures, build errors).
- 86-100: Critical (widespread failures, resource deadlocks).

If the score is unambiguous (95+ confidence that it's correct), the
deterministic result is used directly. No model call.

Deterministic confidence is based on:
- Whether the parser matched the output format (binary: matched or not).
- Whether the parse result is internally consistent (e.g., test count
  matches `tests="N"` in JUnit).

### Tier 2: Cheap Model (Optional)

For output without a deterministic parser, or for summarizing parsed results
with a natural-language title and summary:

**When called**: Deterministic score 10-90 AND confidence < 0.95 OR no
deterministic parser matched.

**Default model**: Configurable. Suggested defaults:
- **DeepSeek V4 Flash** (cheapest API option, ~$0.20/M input tokens).
- **Local model via Ollama/MLX/llama.cpp** (zero cost, stays on-device).

**Input**: Redacted, truncated (first 4KB) command output + deterministic
parse results (if any) + command class + repo context.

**Output schema** (strict JSON):
```json
{
  "importance": 75,
  "category": "test_failure",
  "title": "Auth middleware tests failing with 401 after dependency bump",
  "summary": "3 tests in auth/middleware_test.rs failed with 401 Unauthorized. Likely related to recent changes in the auth crate dependency. Affected tests: test_token_refresh, test_session_expiry, test_oauth_flow.",
  "should_notify_user": true,
  "should_surface_to_agents": true,
  "recommended_action": "Check if the auth crate version bump in Cargo.lock introduced breaking changes to token validation.",
  "confidence": 0.82
}
```

**Constraints**:
- `max_tokens_per_call`: 1024 (enforced).
- `max_calls_per_hour`: configurable (default 60).
- `max_calls_per_month`: configurable (default 5000).
- `cost_budget_monthly_usd`: configurable (default $2.00).
- Model output is validated against the JSON schema. Invalid output is
  discarded; deterministic score is used as fallback.

### Tier 3: Frontier Model (Optional, Budget-Limited)

For ambiguous, high-impact decisions where the cheap model's confidence
is low:

**When called**: Cheap model confidence < 0.70 AND the event is in a
high-impact category (test failure coverage analysis, multi-agent
conflict summary, complex resource conflict, high-cost queue decision).

**Default model**: Configurable. Suggested defaults:
- **GPT-5.5** or **Claude Opus 4.7**.

**Budget**:
- `max_calls_per_day`: 10 (default).
- `max_calls_per_month`: 100 (default).
- `cost_budget_monthly_usd`: $5.00 (default).

When the budget is exhausted, the cheap model's result is used as-is,
even with low confidence.

## Rationale

### Why Optional

Many power users are privacy-conscious and won't want any data leaving their
machine. Others may not want to pay for API calls. The product must be fully
functional without any model configured. The deterministic tier provides
baseline importance classification; the model tiers provide better
summarization and handle ambiguous cases.

### Why Multi-Tier

A single model approach (just call GPT-5.5 for everything) would be simpler
but:
- Expensive: every event would incur a frontier model cost.
- Slow: frontier models have higher latency.
- Unnecessary: most events are unambiguous and deterministic parsing suffices.

The tiered approach means:
- ~80% of events are handled deterministically (free, instant).
- ~15% go to the cheap model (fast, low cost).
- ~5% escalate to the frontier model (slow, higher cost, rare).

### Why Redaction Required Before Model Calls

Command output can contain secrets (API keys in error messages, tokens in
debug output). Sending this to a third-party model provider is a security
risk. Redaction happens at capture time (before storage) and again before
model calls (belt-and-suspenders). See `docs/SECURITY.md` for the full
redaction specification.

### Why JSON Schema Output

Freeform model output would be unreliable for automated decision-making.
A strict JSON schema with enumerated categories and bounded numeric fields
ensures consistent processing. Invalid JSON is discarded; extraneous fields
are dropped; missing required fields cause the result to be rejected and
the deterministic score used as fallback.

### Why Budget Limits

Surprise API bills are a terrible user experience. Per-call, hourly, daily,
and monthly limits with hard caps prevent runaway costs. The dashboard shows
budget consumption. When limits are hit, the system degrades gracefully to
deterministic-only.

## Alternatives Considered

### Single model (just GPT-5.5 for everything)

**Rejected**: Too expensive for routine events. Too slow. Forces cloud
dependency.

### Deterministic-only (no models at all)

**Rejected** as the only option. While the product must work this way,
unstructured output from custom tools would get generic importance scores
(50 = "unknown"). Models provide better summarization for users who want it.

### On-device-only models (MLX/Ollama required)

**Rejected**: Not all users have a local model runtime set up. We want
the option of zero-setup cloud models for users who prefer convenience.

### Train a custom classifier model

**Rejected**: Significant upfront investment, ongoing maintenance burden,
requires training data, and would still be less flexible than LLM-based
summarization for novel command output formats.

## Consequences

### Positive

- Works without any model configured.
- Tiered approach balances cost, speed, and quality.
- Privacy-conscious users can use local models exclusively.
- Budget limits prevent surprise costs.
- JSON schema ensures consistent, processable output.

### Negative

- Multi-tier architecture adds complexity (three code paths for importance
  classification).
- Cheap model selection is a user decision with tradeoffs (cost vs quality
  vs privacy).
- Frontier model budget limits may cause some ambiguous events to be
  classified with lower confidence than ideal.
- Model output validation adds development overhead.

### Mitigations

- Sensible defaults for model selection documented in `docs/MODELS.md`.
- Budget limits are generous by default and clearly surfaced.
- Deterministic tier handles the majority of events, so model limitations
  rarely affect the user experience.
- Model call audit trail enables debugging of classification decisions.

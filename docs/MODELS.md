# Richter Model Configuration Guide

Richter can optionally use language models to improve event summarization and
resolve ambiguous decisions. Models are **not required** — Richter works fully
without any model provider configured. This guide covers configuration for users
who want enhanced summarization.

## Model Pipeline Overview

The optional model pipeline has two tiers:

```
                         ┌─────────────────────────┐
                         │   Deterministic Engine    │
                         │   (always active, free)   │
                         └───────────┬─────────────┘
                                     │
                    ┌────────────────┼────────────────┐
                    │                │                │
             ┌──────▼──────┐  ┌─────▼─────┐  ┌──────▼──────┐
             │  Pass-through│  │  Ambiguous │  │  Unparsed   │
             │  (score >90  │  │  (score    │  │  output     │
             │   or <10)    │  │  30-70)    │  │             │
             └──────────────┘  └─────┬─────┘  └──────┬──────┘
                                     │                │
                               ┌─────▼─────┐   ┌──────▼──────┐
                               │  Tier 2:   │   │  Tier 2:    │
                               │ Cheap Model│   │ Cheap Model │
                               │ (optional) │   │ (optional)  │
                               └─────┬─────┘   └──────┬──────┘
                                     │                │
                          ┌──────────┼────────┐       │
                          │          │        │       │
                    ┌─────▼──┐ ┌────▼──┐ ┌──▼───┐    │
                    │ Conf > │ │ Conf  │ │ Conf │    │
                    │ 0.85   │ │ 0.5-  │ │ <0.5 │    │
                    │        │ │ 0.85  │ │      │    │
                    └────┬───┘ └──┬────┘ └──┬───┘    │
                         │        │         │        │
                         ▼        ▼         ▼        ▼
                      ACCEPT   ACCEPT   ┌──────────────┐
                      (done)   (done)   │  Tier 3:      │
                                        │ Frontier Model│
                                        │ (optional,    │
                                        │  budget-ltd)  │
                                        └──────────────┘
```

**Tier 1 (Deterministic)**: Always active. Parse structured output (JUnit, TAP,
Cargo test, pytest, ESLint, etc.) and score deterministically. If the score is
unambiguous (>90 or <10), no model call is needed.

**Tier 2 (Cheap Model)**: For output without a deterministic parser, or for
summarizing parsed results. A fast, inexpensive model classifies importance and
produces a summary. If confidence is high, the result is used directly.

**Tier 3 (Frontier Model)**: Only for ambiguous, high-impact cases where the
cheap model's confidence is low. Budget-limited to control costs.

## Running Without Any Model Configured

Richter works fully without models. The deterministic engine handles all
classification:

- **Command classification** is always deterministic (regex-based parsers per
  ecosystem).
- **Fingerprinting** is always deterministic (hash-based).
- **Run-or-join decisions** are always deterministic (fingerprint matching).
- **Event importance** uses deterministic parsing of structured output formats.

What you lose without models:
- No summarization of unstructured output (e.g., a custom test runner without
  JUnit output).
- No natural-language titles/summaries for events.
- No advisory coverage analysis (subset/superset test relationship detection).
- No complex conflict summary generation.

This is perfectly fine for most workflows.

## Supported Providers

| Provider | Cheapest Model | Frontier Model | Connection Method |
|---|---|---|---|
| **DeepSeek** | DeepSeek V4 Flash | — | API key + HTTPS |
| **OpenAI** | GPT-5.5 Mini | GPT-5.5 | API key + HTTPS |
| **Anthropic** | Claude Sonnet 4.7 | Claude Opus 4.7 | API key + HTTPS |
| **Ollama** (local) | Any model | Any model | Local HTTP |
| **MLX** (local) | Any model | Any model | Local HTTP |
| **llama.cpp** (local) | Any model | Any model | Local HTTP |

### API Key Storage

All provider API keys are stored in the macOS Keychain, never in config files or
the SQLite database. The Richter app manages keychain entries via the Security
framework. The daemon receives keys over the local API only when making a model
call, and holds them in memory only for the duration of the call.

## Configuring the Cheap Model (Tier 2)

The cheap model handles routine summarization: classifying event importance,
generating one-line titles and summaries, and deciding notification priority.

### DeepSeek V4 Flash (Recommended Default)

Fast, inexpensive, good for summarization. ~$0.20/M input tokens.

```toml
# ~/.richter/config.toml
[models.cheap]
provider = "deepseek"
model = "deepseek-v4-flash"
api_key_keychain = "com.richter.deepseek-api-key"
max_tokens_per_call = 1024
max_calls_per_hour = 60
max_calls_per_month = 5000
cost_budget_monthly_usd = 2.00
```

Set the API key:

```bash
richter model set-key --provider deepseek
# Prompts for API key, stores in Keychain
```

### OpenAI GPT-5.5 Mini

```toml
[models.cheap]
provider = "openai"
model = "gpt-5.5-mini"
api_key_keychain = "com.richter.openai-api-key"
max_tokens_per_call = 1024
max_calls_per_hour = 60
cost_budget_monthly_usd = 5.00
```

### Claude Sonnet 4.7

```toml
[models.cheap]
provider = "anthropic"
model = "claude-sonnet-4-7-20250514"
api_key_keychain = "com.richter.anthropic-api-key"
max_tokens_per_call = 1024
max_calls_per_hour = 60
cost_budget_monthly_usd = 5.00
```

### Local Model (Ollama)

```toml
[models.cheap]
provider = "ollama"
model = "deepseek-coder-v2:16b-lite"
endpoint = "http://localhost:11434"
max_tokens_per_call = 1024
# No cost budget needed — runs locally
```

### Local Model (MLX)

```toml
[models.cheap]
provider = "mlx"
model = "mlx-community/Llama-3.2-12B-4bit"
endpoint = "http://localhost:8080"
max_tokens_per_call = 1024
```

### Local Model (llama.cpp)

```toml
[models.cheap]
provider = "llamacpp"
model = "qwen2.5-14b"
endpoint = "http://localhost:8081"
max_tokens_per_call = 1024
```

## Configuring the Frontier Model (Tier 3)

The frontier model is only called for ambiguous high-impact decisions. It is
budget-limited to prevent surprise costs.

### OpenAI GPT-5.5 (Recommended Default)

```toml
[models.frontier]
provider = "openai"
model = "gpt-5.5"
api_key_keychain = "com.richter.openai-api-key"
max_tokens_per_call = 4096
max_calls_per_day = 10
max_calls_per_month = 100
cost_budget_monthly_usd = 5.00
```

### Claude Opus 4.7

```toml
[models.frontier]
provider = "anthropic"
model = "claude-opus-4-7-20250514"
api_key_keychain = "com.richter.anthropic-api-key"
max_tokens_per_call = 4096
max_calls_per_day = 10
cost_budget_monthly_usd = 5.00
```

### Disabling the Frontier Model

If you only want Tier 2 summarization:

```toml
[models.frontier]
enabled = false
```

Or omit the `[models.frontier]` section entirely.

## When the Frontier Model Is Used

The frontier model is consulted only when **all** of these conditions are met:

1. The deterministic parser produced an ambiguous score (typically 30-70).
2. The cheap model's confidence was below the configured threshold (default 0.7).
3. The event is in a high-impact category:
   - Whether one test run covers another (subset/superset relationship)
   - Repeated failures across multiple agents in the same repo
   - Complex resource conflicts involving multiple repos
   - High-cost queue decision (which of N queued runs to prioritize)
4. The daily/monthly budget has not been exceeded.

This means the frontier model is called **rarely** — perhaps a few times per day
in a busy multi-agent environment.

## Redaction Before Model Calls

Before any text is sent to a model provider (cheap or frontier), Richter applies
the redaction engine:

1. **API keys** — OpenAI `sk-*`, Anthropic `sk-ant-*`, etc.
2. **Bearer tokens** — `Bearer eyJ...`
3. **Private keys** — `-----BEGIN PRIVATE KEY-----` blocks
4. **GitHub tokens** — `ghp_*`, `github_pat_*`
5. **AWS credentials** — `AKIA*`, `ASIA*` with secret keys
6. **GCP service account keys** — JSON key files
7. **Azure connection strings** — `DefaultEndpointsProtocol=...`
8. **Database URLs** — `postgres://user:pass@...`, `mysql://user:pass@...`
9. **Cookies** — long cookie strings in HTTP headers
10. **Environment variable values** — detected by pattern matching

Redacted text is replaced with `[REDACTED:<type>]` placeholders before being
sent to the model. The original redacted values never leave the machine.

## LLM Payload Preview

For debugging and audit, Richter can log the exact payload sent to model
providers (with secrets already redacted):

```toml
[models.debug]
log_payloads = true
log_responses = true
log_path = "~/.richter/logs/model_payloads"
```

The Settings → Models panel in the app also shows a "Preview last payload"
button that displays the most recent model call input/output.

Payloads are redacted before logging. The redacted preview shows what was
actually sent.

**Important**: Enable this only for debugging. Payload logs should be rotated
frequently.

## Budget Limits

Richter enforces budget limits to prevent surprise API bills:

```toml
[models.budgets]
monthly_total_usd = 10.00      # Hard cap across all providers
warn_at_usd = 5.00             # Show warning in dashboard
```

Budget tracking:

- Richter estimates token counts (using tiktoken or equivalent for each provider).
- Costs are computed using the provider's published pricing.
- Budget state is persisted in SQLite and survives daemon restarts.
- When the warning threshold is hit, a dashboard notification appears.
- When the hard cap is hit, all model calls stop until the next billing month
  (or until the cap is raised).

Local models (Ollama, MLX, llama.cpp) do not count against the budget.

## Disabling Models Temporarily

```bash
richter model disable
richter model enable
```

Or toggle in Settings → Models → "Enable model pipeline."

## Choosing Models

### Cost-Sensitive Setup

```toml
[models.cheap]
provider = "ollama"
model = "deepseek-coder-v2:16b-lite"
endpoint = "http://localhost:11434"

[models.frontier]
enabled = false
```

Zero cost, everything stays local. Good for privacy-conscious users.

### Balanced Setup

```toml
[models.cheap]
provider = "deepseek"
model = "deepseek-v4-flash"

[models.frontier]
provider = "openai"
model = "gpt-5.5"
max_calls_per_day = 5
```

Low cost (~$2-3/month typical), decent summarization quality.

### Quality Setup

```toml
[models.cheap]
provider = "anthropic"
model = "claude-sonnet-4-7-20250514"

[models.frontier]
provider = "anthropic"
model = "claude-opus-4-7-20250514"
max_calls_per_day = 10
```

Higher cost (~$5-10/month typical), best summarization quality.

## Model Call Audit

All model calls are recorded in the `model_calls` SQLite table with:

- Provider and model name
- Input hash (for deduplication)
- Latency
- Token counts (input/output)
- Cost estimate
- Timestamp

The audit trail is accessible via:

```bash
richter models calls --last 50
```

Or in the dashboard: Events → Filter: "Model Calls."

# Total Memory MCP Plan

Date: 2026-05-06

## Executive Take

Build `total-memory-mcp` as a standalone, open-source, frontier-grade local context platform for coding agents.

This should replace Semble and Serena as the configured MCP surface for Codex, Droid, Forge, Kimi, and Claude, while preserving the useful ideas behind both:

- Semble's job becomes `search_code`: hybrid semantic plus lexical retrieval.
- Serena's job becomes `get_symbol`, `find_references`, `call_graph`, and `diagnostics`.
- Agent memory becomes first-class: durable decisions, user preferences, project facts, procedures, prior findings, and temporal graph memory.

The correct architecture is not "a vector DB with vibes." It is a scoped local knowledge system with code intelligence, durable memory, symbol graph, provenance, safety gates, and installer-grade MCP integration.

## Product Goal

Create a single MCP server named `total-memory` that agents can invoke for:

- Persistent cross-session memory.
- Repo-aware code search.
- Symbol navigation.
- Context-pack assembly.
- Project and user preferences.
- Prior-decision recall.
- Technical diligence and implementation context.

The system must work outside Richter and be installable into:

- Codex
- Droid / Factory
- Claude Code
- Forge
- Kimi

The result should be usable locally, shareable as an open-source Git repository, and strong enough to replace Semble and Serena in day-to-day agent workflows.

## Repository Shape

Create a new standalone repo:

```text
/Users/dewclaw/Documents/Projects/total-memory-mcp
```

Recommended layout:

```text
total-memory-mcp/
  Cargo.toml
  README.md
  LICENSE
  docs/
    architecture.md
    install.md
    client-config.md
    memory-model.md
    code-indexing.md
    security.md
    operations.md
  crates/
    total-memory-cli/
    total-memory-mcp/
    total-memory-core/
    total-memory-indexer/
    total-memory-memory/
    total-memory-storage/
    total-memory-lsp/
    total-memory-scip/
    total-memory-install/
  workers/
    reranker/
    graphiti/
  docker/
    compose.yaml
  scripts/
    doctor.sh
    install-local.sh
  tests/
    fixtures/
```

Use MIT licensing unless a dependency forces a different decision.

## Architecture

### Runtime

Build a Rust binary:

```bash
total-memory serve --stdio
```

Rules:

- stdout is MCP JSON-RPC only.
- logs go to stderr.
- all tool responses are structured JSON.
- every result includes provenance where available.
- every repo operation accepts an explicit `project_path`.
- no tool should rely on the MCP client launching from the desired working directory.

### Storage

Use the max-SOTA local stack:

- PostgreSQL with `pgvector` as canonical storage for memories, metadata, scopes, provenance, audit logs, and relational queries.
- Qdrant for dense/sparse vector retrieval at scale.
- Neo4j with Graphiti for temporal entity/fact graph memory.
- Tantivy for local BM25/full-text code search.
- Optional Zoekt bridge for fast code regex/trigram search.

Provide a Docker Compose stack for local frontier mode:

```text
postgres + pgvector
qdrant
neo4j
graphiti worker
reranker worker
```

SQLite-only mode may exist later, but it is not the target for this build. The user explicitly chose SOTA over minimalism.

## Code Intelligence

### Indexing Pipeline

Implement `.gitignore`-aware incremental indexing:

1. Resolve git root and project key.
2. Walk files with ignore rules.
3. Hash file contents.
4. Reindex only changed files.
5. Chunk code with tree-sitter-aware boundaries.
6. Store file, chunk, symbol, edge, and embedding metadata.
7. Track freshness and index coverage.

### Tree-Sitter

Use tree-sitter for broad AST extraction:

- files
- modules
- classes
- structs
- enums
- functions
- methods
- imports
- rough call edges
- test symbols
- doc comments

Tree-sitter is not semantic truth. Treat it as the robust cross-language structure layer.

### SCIP

Support SCIP ingestion for precise code intelligence:

- definitions
- references
- implementations
- occurrences
- symbol descriptors

Use SCIP when language-specific indexers exist. Fall back to tree-sitter and LSP where SCIP is unavailable.

### LSP

Add an LSP bridge for live truth:

- diagnostics
- hover
- definition lookup
- reference lookup
- workspace symbols
- call hierarchy

The priority order should be:

1. LSP for live working tree truth.
2. SCIP for persisted precise symbols.
3. Tree-sitter for broad fallback structure.

## Memory System

### Memory Types

Support these memory kinds:

- `preference`: durable user preference.
- `decision`: accepted technical or product decision.
- `project_fact`: project-specific fact.
- `procedure`: recurring workflow or command.
- `finding`: audit, debugging, diligence, or review finding.
- `episode`: raw event or conversation-derived note.
- `summary`: rolled-up project/user/org context.
- `correction`: explicit correction to old memory.

### Scopes

All reads and writes must be scoped before retrieval:

- global
- user
- org
- project
- repo
- session
- agent

Never retrieve broadly and filter afterward. That is how leaks happen.

### Provenance

Every memory stores:

- memory id
- created time
- updated time
- scope
- project key
- agent/client
- source
- confidence
- validity window
- supersession state
- source file paths/ranges, when applicable
- source episode id, when applicable

### Conflict Handling

Do not silently overwrite old memories.

Use:

- `valid_from`
- `valid_to`
- `supersedes`
- `superseded_by`
- confidence
- source priority

When memories conflict, return the conflict explicitly instead of pretending the newest memory is automatically true.

## Retrieval

Retrieval should be hybrid by default:

1. Dense embedding search.
2. Sparse/BM25 search.
3. Symbol graph traversal.
4. Temporal graph traversal.
5. Exact path/symbol/error-string search.
6. Reciprocal Rank Fusion.
7. Reranking for ambiguous or high-value queries.

Return compact context packs, not giant file dumps.

Each result should include:

- score
- source kind
- path/range or memory id
- cited excerpt
- reason it matched
- freshness
- confidence
- related symbols/memories/tests

## Models

Default to local frontier models:

- Embedding: `qwen3-embedding:8b`
- Fallback embedding: `qwen3-embedding:4b`
- Reranker: Qwen3 Reranker 8B via local worker
- Fallback reranker: Qwen3 Reranker 4B

`doctor` must fail loudly if required models are not available.

Do not silently fall back to weak embeddings unless the user explicitly opts into degraded mode.

## MCP Tools

Expose one server:

```text
total-memory
```

Tool surface:

```text
remember
recall
search_code
context_pack
get_symbol
find_references
call_graph
diagnostics
index_project
index_status
forget
doctor
```

### `remember`

Stores durable memory.

Inputs:

- `content`
- `kind`
- `scope`
- `project_path`
- `tags`
- `confidence`
- `source`
- `source_paths`

Reject secrets and credential-looking content before persistence.

### `recall`

Retrieves memories across semantic, lexical, temporal, and graph indexes.

Inputs:

- `query`
- `project_path`
- `scope`
- `kind`
- `tags`
- `limit`
- `include_global`

### `search_code`

Replaces Semble.

Inputs:

- `query`
- `project_path`
- `mode`: `auto`, `semantic`, `lexical`, `symbol`, `hybrid`
- `language`
- `path_filter`
- `limit`

### `context_pack`

Primary agent power tool.

Given a query or task, return:

- relevant memories
- relevant files
- symbols
- definitions
- references
- tests
- config/docs
- known decisions
- warnings

It should fit a requested token budget.

### `get_symbol`

Replaces Serena definition navigation.

Inputs:

- `symbol`
- `project_path`
- optional `path_hint`

### `find_references`

Replaces Serena reference navigation.

Inputs:

- `symbol`
- `project_path`
- optional `path_filter`

### `call_graph`

Returns caller/callee graph.

Inputs:

- `symbol`
- `direction`: `callers`, `callees`, `both`
- `depth`
- `project_path`

### `diagnostics`

Returns LSP/compiler diagnostics.

Inputs:

- `project_path`
- `path_filter`

### `index_project`

Explicitly indexes or reindexes a repo.

Inputs:

- `project_path`
- `force`
- `include_tests`
- `include_docs`

### `index_status`

Returns freshness and coverage:

- indexed files
- indexed symbols
- indexed chunks
- last index time
- stale file count
- model names
- backend health
- SCIP availability
- LSP availability

### `forget`

Deletes or supersedes memory.

Inputs:

- `memory_id`
- `mode`: `delete`, `supersede`, `redact`
- `reason`

### `doctor`

Validates:

- MCP stdio cleanliness
- Postgres
- Qdrant
- Neo4j
- Graphiti worker
- reranker worker
- Ollama/model availability
- config paths
- client registrations
- sample tool call

## CLI Commands

```bash
total-memory serve --stdio
total-memory doctor
total-memory index /path/to/repo
total-memory search "query" /path/to/repo
total-memory recall "query" --project /path/to/repo
total-memory install --client codex --scope user --dry-run
total-memory install --client codex --scope user --apply
total-memory install --client droid --scope user --apply
total-memory install --client forge --scope user --apply
total-memory install --client kimi --scope user --apply
total-memory install --client claude --scope user --apply
```

Installer rules:

- default to dry-run
- show exact config changes
- use absolute binary paths
- never write secrets into config
- preserve unrelated MCP servers
- remove Semble/Serena only after `total-memory doctor` passes

## Client Config Targets

Update:

- `~/.codex/config.toml`
- `~/.factory/mcp.json`
- `~/.kimi/mcp.json`
- Claude user MCP config
- Forge user MCP config

Remove these MCP registrations after successful verification:

- `semble`
- `serena`

Do not delete the binaries. Only remove active MCP registration.

## Agent Instructions

Update global and repo-level instructions:

- `/Users/dewclaw/AGENTS.md`
- `/Users/dewclaw/.codex/AGENTS.md`
- `/Users/dewclaw/.claude/CLAUDE.md`
- `/Users/dewclaw/Documents/Projects/Richter/AGENTS.md`
- `/Users/dewclaw/Documents/Projects/Richter/CLAUDE.md`
- `/Users/dewclaw/Documents/Projects/imaginethat-cli/AGENTS.md`, if present
- `/Users/dewclaw/Documents/Projects/imaginethat-cli/CLAUDE.md`, if present

Instruction rules:

- Use `context_pack` at the start of broad reviews, audits, debugging sessions, unfamiliar code work, and resumes.
- Use `search_code` before broad manual grep when the query is conceptual or behavioral.
- Use exact shell search for exact strings, generated names, error messages, and short symbols.
- Use `get_symbol`, `find_references`, and `call_graph` for structural navigation.
- Use `remember` only for durable facts, decisions, findings, procedures, and user preferences.
- Never store secrets.
- Treat memory as advisory until verified against live repo files.
- Pass `project_path` explicitly.

## Security

Write guard:

- Gitleaks-style secret detection.
- Presidio-style PII detection.
- configurable deny patterns.
- reject raw `.env`, private keys, tokens, cookies, and credential dumps.

Read guard:

- scope-first retrieval.
- audit every served memory.
- return redacted content when needed.
- support deletion/supersession.

Repo indexing guard:

- honor `.gitignore`.
- skip common secret files.
- skip build artifacts and dependency directories.
- keep index metadata out of git.

## Testing

### Unit Tests

Cover:

- scope filtering
- memory create/update/delete/supersede
- duplicate detection
- conflict detection
- secret rejection
- PII rejection/redaction
- BM25 retrieval
- vector retrieval
- RRF fusion
- context-pack token budgeting
- project-key derivation
- incremental indexing
- tree-sitter chunking
- symbol extraction

### Integration Tests

Run with Docker Compose:

- Postgres + pgvector
- Qdrant
- Neo4j
- Graphiti worker
- reranker worker
- Ollama/model checks

Test against fixture repos:

- Rust
- Python
- TypeScript
- Swift
- mixed monorepo

### MCP Tests

Verify:

- no stdout pollution
- initialize/list-tools flow
- each tool schema
- each tool happy path
- each tool validation failure
- server shutdown behavior

### Local Acceptance Tests

Run real client verification:

```bash
codex mcp list
claude mcp list
forge mcp list --porcelain
kimi mcp test total-memory
droid exec --auto high --cwd /Users/dewclaw/Documents/Projects/Richter "Call total-memory doctor and return TOOL_OK if it passes."
```

Functional checks:

- Richter code search finds `crates/richter-daemon/src/run_manager.rs` for run-manager queue/cache queries.
- imaginethat-cli code search finds known Agnos/runtime paths for architecture queries.
- `get_symbol` and `find_references` produce comparable or better results than Serena.
- `search_code` produces comparable or better results than Semble.
- `context_pack` returns a useful bounded packet for a full-stack diligence prompt.
- project memories do not leak between Richter and imaginethat-cli.
- global preferences are retrievable from both repos.
- superseded memories do not appear as current facts.

## Implementation Phases

### Phase 1: Repo And MCP Skeleton

- Create standalone repo.
- Add Rust workspace.
- Add MCP stdio server.
- Add `doctor`.
- Add basic install command dry-run.
- Add README and docs.

Exit criteria:

- `total-memory serve --stdio` lists tools.
- MCP inspector works.
- no stdout pollution.

### Phase 2: Storage Stack

- Add Docker Compose.
- Add Postgres schema.
- Add Qdrant collections.
- Add Neo4j/Graphiti bootstrap.
- Add migrations.
- Add health checks.

Exit criteria:

- `total-memory doctor` validates all services.

### Phase 3: Memory

- Implement `remember`, `recall`, `forget`.
- Add scopes, provenance, supersession, audit logs.
- Add secret/PII write guard.
- Add hybrid memory retrieval.

Exit criteria:

- cross-project isolation passes.
- global preference retrieval passes.

### Phase 4: Code Index

- Add git-aware file walker.
- Add tree-sitter chunking.
- Add Tantivy lexical index.
- Add embeddings into Qdrant.
- Add `index_project`, `index_status`, `search_code`.

Exit criteria:

- Richter and imaginethat-cli indexing pass.
- known code-search probes pass.

### Phase 5: Symbol Intelligence

- Add SCIP ingestion.
- Add LSP bridge.
- Add `get_symbol`, `find_references`, `call_graph`, `diagnostics`.

Exit criteria:

- symbol tools match or beat Serena for representative probes.

### Phase 6: Context Packs

- Implement `context_pack`.
- Combine memory, code, symbols, tests, docs, configs, and diagnostics.
- Add token-budget packing.

Exit criteria:

- diligence/review/debug prompts receive compact useful context.

### Phase 7: Client Installation

- Implement installers for Codex, Droid, Forge, Kimi, Claude.
- Add dry-run and apply modes.
- Preserve unrelated MCP servers.
- Remove Semble/Serena only after successful doctor and smoke tests.

Exit criteria:

- every client lists `total-memory`.
- every client can call at least one real tool.

### Phase 8: Documentation And Open Source Polish

- Complete README.
- Add architecture docs.
- Add security docs.
- Add install docs.
- Add contribution docs.
- Add example configs.
- Add CI.

Exit criteria:

- a new machine can install from the repo docs.

## Definition Of Done

This is done when:

- `total-memory` is a standalone git repo.
- It has working MCP stdio support.
- It indexes Richter and imaginethat-cli.
- It stores and recalls scoped memories.
- It performs hybrid code search.
- It supports symbol navigation.
- It returns context packs.
- It passes tests.
- It has docs.
- Codex, Droid, Forge, Kimi, and Claude all use it.
- Semble and Serena are removed from active MCP config.
- No Warp/Oz dependency remains.
- A clean install path exists for another machine.

## Reference Links

- MCP SDKs: https://modelcontextprotocol.io/docs/sdk
- MCP transports: https://modelcontextprotocol.io/specification/2024-11-05/basic/transports
- RMCP Rust SDK: https://github.com/modelcontextprotocol/rust-sdk
- SCIP: https://github.com/sourcegraph/scip
- tree-sitter: https://tree-sitter.github.io/tree-sitter/
- pgvector: https://github.com/pgvector/pgvector
- Qdrant: https://qdrant.tech/documentation/
- Neo4j: https://neo4j.com/docs/
- Graphiti: https://github.com/getzep/graphiti
- Tantivy: https://github.com/quickwit-oss/tantivy
- Zoekt: https://github.com/sourcegraph/zoekt
- Ollama embeddings: https://docs.ollama.com/capabilities/embeddings
- Qwen3 Embedding: https://github.com/QwenLM/Qwen3-Embedding
- Gitleaks: https://github.com/gitleaks/gitleaks
- Presidio: https://github.com/microsoft/presidio
- Codex MCP docs: https://developers.openai.com/codex/mcp
- Claude Code MCP docs: https://code.claude.com/docs/en/mcp
- Forge MCP docs: https://forgecode.dev/docs/mcp-integration/
- Droid MCP docs: https://factory.mintlify.app/cli/configuration/mcp
- Kimi MCP docs: https://www.kimi.com/code/docs/en/kimi-code-cli/customization/mcp.html

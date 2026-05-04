# ADR 0002: Command Fingerprinting Approach

**Status**: Accepted
**Date**: 2026-05-04
**Author**: Alberto Nunez
**Deciders**: Alberto Nunez

## Context

Richter's core value proposition is preventing duplicate command execution.
When two agents run `cargo test --lib`, Richter must decide: is this the
same command that Agent A is already running? Can I join Agent B to that
run? Can I return a cached result?

This decision must be:

- **Deterministic** — same inputs must produce the same decision every time.
- **Conservative** — false positives (joining runs that aren't equivalent)
  are worse than false negatives (not joining runs that are equivalent).
- **Fast** — fingerprint computation should be near-instantaneous.
- **Comprehensive** — must capture all factors that could change command
  output: code changes, dependency changes, config changes, toolchain changes,
  environment changes.

We need to decide: what goes into the command fingerprint, what hash
algorithm to use, and what the false-positive/false-negative tradeoffs are.

## Decision

### Fingerprint Components

The fingerprint is a hash of the following inputs, serialized in a canonical
order:

```
fingerprint = hash(
    canonicalized_argv,          # full command + all args, normalized
    command_class,               # build | test | lint | typecheck | ...
    repo_id,                     # unique repo identifier (Git toplevel)
    worktree_path,               # absolute path, canonicalized
    git_common_dir,              # .git dir (shared across worktrees)
    head_sha,                    # full 40-char SHA
    dirty_tree_hash,             # hash of git diff HEAD (unstaged)
    staged_diff_hash,            # hash of git diff --cached (staged)
    unstaged_diff_hash,          # hash of git diff (unstaged, redundantly)
    untracked_files_hash,        # hash of relevant untracked file paths + stat data
    lockfile_hashes,             # hash of Cargo.lock, pnpm-lock.yaml, etc.
    config_file_hashes,          # hash of relevant config files
    toolchain_versions,          # rustc --version, node --version, etc.
    important_env_vars,          # RUSTFLAGS, NODE_ENV, GOFLAGS, etc.
    current_arch,                # arm64 | x86_64
    working_directory,           # absolute, canonicalized
    inferred_test_target,        # subset/test filter if deterministic
    explicit_user_fingerprint,   # user-supplied extra config
    output_resource_keys         # lock keys for shared output dirs
)
```

### Hash Algorithm

**BLAKE3** for the primary fingerprint hash.

Rationale:
- Extremely fast (faster than SHA-256 on Apple Silicon).
- Cryptographically secure (no collision concerns for our use case).
- 256-bit output (32 bytes), fits in a hex string.
- Single-pass streaming API fits our incremental hash construction.
- Well-supported Rust crate with no system dependencies.

For individual sub-hashes (dirty tree, lockfiles), we use **SHA-256**. BLAKE3
is the final aggregator; SHA-256 is used for content-addressed storage of
sub-components where we want to cache/reuse partial hashes.

### Conservative Philosophy

"False misses are acceptable. False hits are not."

This means:

1. **If in doubt, don't match.** If a fingerprint component is ambiguous
   (e.g., a lockfile parse fails), treat it as not matching rather than
   assuming equivalence.

2. **Include more than you think you need.** It's better to include a factor
   that rarely matters (like `$RUSTFLAGS`) and have fewer false positives
   than to omit it and risk joining runs with different compiler flags.

3. **Staged and unstaged diffs are included separately.** Even though Git
   provides a combined diff, we hash staged and unstaged changes separately
   to catch the case where the same file has both staged and unstaged
   changes (the combined diff could look identical for different staged/
   unstaged combinations).

4. **Untracked files are hashed by relevance.** We don't hash every untracked
   file (could be gigabytes). We hash untracked files matching relevant
   patterns (e.g., `.rs` files for Rust repos, `.ts`/`.tsx` for JS repos)
   that are in source directories.

5. **Environment variables are selectable.** We don't hash the entire
   environment (too many variables, too much noise). We hash a curated list
   of build-relevant variables, configurable per-repo.

### Canonicalization

Before hashing, inputs are canonicalized:

- **argv**: Join with NUL separators (handles spaces and special chars).
  Strip leading `richter run --` if present. Normalize flag ordering where
  order doesn't matter (e.g., `cargo test --lib --release` == `cargo test
  --release --lib`).
- **Paths**: Resolve symlinks, normalize to absolute, remove trailing `/`.
- **Toolchain versions**: Parse and normalize (e.g., `rustc 1.85.0
  (abc123 2025-01-01)` → `rustc/1.85.0`).
- **Env vars**: Sort alphabetically by key, use NUL-separated key=value.
- **LOCK files**: Hash after normalizing line endings (LF).

### Subset/Superset Detection (Future)

For commands like `cargo test -p auth` vs `cargo test --all`, Richter may
determine that the full suite covers the subset. This is done
**deterministically** first (e.g., Cargo test names are prefix-matchable)
and with the **frontier model** as an advisory fallback. The fingerprint
itself does not encode subset/superset relationships; those are handled by
a separate coverage analysis layer.

## Rationale

### Why Not Just Use argv + repo + HEAD?

Too aggressive. A dirty working tree (unstaged changes) could change test
behavior. Running the same command in different worktrees could produce
different results due to path-dependent build artifacts. Different lockfile
contents mean different dependencies. Different toolchain versions mean
different compiler behavior. All of these matter.

### Why BLAKE3 Over SHA-256 for the Final Hash?

BLAKE3 is ~10x faster than SHA-256 on Apple Silicon for hashing multi-KB
inputs. While the absolute difference is small (microseconds vs nanoseconds)
for a single fingerprint, the fingerprint is computed on every command
invocation. Over thousands of invocations in a busy development session,
the difference adds up.

SHA-256 is used for sub-hashes because we want content-addressed storage
keys that are widely supported and stable. BLAKE3 is used for the final
aggregate because we only compare it internally.

### Why Hash Dirty Tree Separately from HEAD SHA?

If two agents are on the same HEAD but one has unstaged changes, the command
output could differ. The dirty tree hash captures this. We can't just use
`git diff HEAD` because:
- It mixes staged and unstaged changes in ways that could be ambiguous.
- Two different dirty states could produce the same combined diff.

## Alternatives Considered

### argv-only fingerprint (ignore repo state)

**Rejected**: Obvious false positives. Same command on different branches,
different dependencies, different dirty states would be incorrectly joined.

### Full tree hash (hash the entire working directory)

**Rejected**: Too slow for large repos, too sensitive (changes in unrelated
files would invalidate the cache), and includes build artifacts that aren't
relevant. We need targeted hashing of what matters.

### xxHash or CityHash (non-cryptographic)

**Rejected**: While faster than BLAKE3, non-cryptographic hashes have
higher collision rates. Even though our use case doesn't require
cryptographic security (we're comparing hashes within a local system),
the collision probability of 64-bit hashes is high enough with thousands
of fingerprints that we'd rather pay the small performance cost for 256-bit
output.

### MD5 or SHA-1

**Rejected**: Both have known collision vulnerabilities. While we don't
need cryptographic security, using broken algorithms in new systems is
poor practice and could cause issues if fingerprints are ever shared or
compared across machines.

## Consequences

### Positive

- Robust deduplication that catches nuanced equivalence cases.
- Conservative false-positive rate protects against incorrect joins.
- Fast enough for interactive use (fingerprint computation < 1ms typical).
- Deterministic and reproducible for debugging.

### Negative

- Conservative approach means some opportunities for deduplication are
  missed (false negatives). For example, changing a comment in an unrelated
  file changes the dirty tree hash and prevents a cache hit even though the
  test result would be identical.
- Fingerprint computation requires `git` commands, which have subprocess
  overhead. We mitigate by caching Git state between fingerprint
  computations (state polls every 5s during active runs).
- Toolchain version detection requires spawning `rustc --version`,
  `node --version`, etc. These are cached with short TTLs to avoid
  per-invocation overhead.
- Complex fingerprint means debugging "why didn't this join?" can be
  non-obvious. We mitigate with `richter explain <run-id>` showing the
  fingerprint components that differed.

//! Criterion benchmarks for the BLAKE3 fingerprint engine.
//!
//! Fingerprints are computed for every command execution for dedup — this is
//! also a hot path. These benchmarks measure single-command fingerprinting
//! and batch throughput.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use richter_core::classifier::ClassifiedCommand;
use richter_core::fingerprint;
use richter_core::models::CommandClass;

/// The current directory serves as the git repo root for benchmarks.
/// In CI or local dev, this is the Richter monorepo.
const BENCH_CWD: &str = env!("CARGO_MANIFEST_DIR");

/// Create a simple classified command.
fn cmd(cls: CommandClass, tool: &str, args: &[&str]) -> ClassifiedCommand {
    ClassifiedCommand {
        class: cls,
        tool: tool.into(),
        subcommand: args.first().map(|s| s.to_string()),
        is_interactive: false,
        is_destructive: false,
        canonical: std::iter::once(tool.to_string())
            .chain(args.iter().map(|s| s.to_string()))
            .collect(),
    }
}

fn bench_fingerprint(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    let mut group = c.benchmark_group("fingerprint");

    // --- Simple commands ---

    let echo = cmd(CommandClass::Unknown, "echo", &["hello"]);
    group.bench_function("echo_hello", |b| {
        b.iter(|| {
            rt.block_on(fingerprint::fingerprint(
                black_box(&echo),
                black_box(BENCH_CWD),
            ))
        })
    });

    let ls = cmd(CommandClass::Unknown, "ls", &["-la"]);
    group.bench_function("ls_la", |b| {
        b.iter(|| {
            rt.block_on(fingerprint::fingerprint(
                black_box(&ls),
                black_box(BENCH_CWD),
            ))
        })
    });

    // --- Complex commands ---

    let cargo_build = cmd(
        CommandClass::Build,
        "cargo",
        &["build", "--release", "--all-features"],
    );
    group.bench_function("cargo_build_complex", |b| {
        b.iter(|| {
            rt.block_on(fingerprint::fingerprint(
                black_box(&cargo_build),
                black_box(BENCH_CWD),
            ))
        })
    });

    let npm = cmd(
        CommandClass::Build,
        "npm",
        &["run", "build", "--", "--verbose"],
    );
    group.bench_function("npm_run_build", |b| {
        b.iter(|| {
            rt.block_on(fingerprint::fingerprint(
                black_box(&npm),
                black_box(BENCH_CWD),
            ))
        })
    });

    // --- Cross-worktree fingerprint (no CWD in hash) ---

    let cross = cmd(CommandClass::Test, "cargo", &["test"]);
    group.bench_function("cross_worktree", |b| {
        b.iter(|| {
            rt.block_on(fingerprint::fingerprint_cross_worktree(
                black_box(&cross),
                black_box(BENCH_CWD),
            ))
        })
    });

    group.finish();

    // --- Throughput: 1000 different commands ---

    let mut tp_group = c.benchmark_group("fingerprint_throughput");
    tp_group.throughput(Throughput::Elements(1000));

    let commands: Vec<ClassifiedCommand> = (0..1000)
        .map(|i| {
            cmd(
                CommandClass::Unknown,
                &format!("tool-{i}"),
                &[&format!("arg{}", i % 10), &format!("--flag{}", i)],
            )
        })
        .collect();

    tp_group.bench_function("fingerprint_1000_commands", |b| {
        b.iter(|| {
            rt.block_on(async {
                for c in &commands {
                    let _ = black_box(
                        fingerprint::fingerprint(black_box(c), black_box(BENCH_CWD)).await,
                    );
                }
            })
        })
    });

    tp_group.finish();

    // --- Benchmark hash-only (no git calls) for micro-comparison ---

    let mut micro_group = c.benchmark_group("fingerprint_micro");
    // Use a temp path that isn't a git repo to avoid git calls.
    // The fingerprint function will still try but git commands will fail fast.

    let simple = cmd(CommandClass::Test, "just_cmd", &["arg1"]);

    micro_group.bench_function("hash_only_no_git", |b| {
        b.iter(|| {
            rt.block_on(fingerprint::fingerprint(
                black_box(&simple),
                black_box("/tmp/nonexistent_bench_dir"),
            ))
        })
    });

    micro_group.finish();
}

criterion_group!(benches, bench_fingerprint);
criterion_main!(benches);

//! Criterion benchmarks for the command classifier.
//!
//! The classifier runs on every intercepted command — it's the hottest path
//! in Richter. These benchmarks measure classification throughput for common
//! toolchain invocations, unknown commands, and bulk throughput.

use criterion::measurement::WallTime;
use criterion::BenchmarkGroup;
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use richter_core::classifier::classify;

/// Build an argv from a space-separated command string.
fn argv(cmd: &str) -> Vec<String> {
    cmd.split_whitespace().map(String::from).collect()
}

/// Benchmark a single command classification.
fn bench_single(group: &mut BenchmarkGroup<WallTime>, name: &str, cmd: &str) {
    let args = argv(cmd);
    group.bench_function(name, |b| {
        b.iter(|| classify(black_box(&args)));
    });
}

fn bench_classifier(c: &mut Criterion) {
    let mut group = c.benchmark_group("classify");

    // --- Common toolchain commands (hot path) ---

    bench_single(&mut group, "cargo_test", "cargo test");
    bench_single(&mut group, "cargo_build", "cargo build --release");
    bench_single(&mut group, "cargo_check", "cargo check --workspace");
    bench_single(&mut group, "cargo_clippy", "cargo clippy --all-features");
    bench_single(&mut group, "cargo_fmt", "cargo fmt --all");
    bench_single(&mut group, "npm_run_build", "npm run build");
    bench_single(&mut group, "npm_run_test", "npm run test");
    bench_single(&mut group, "npm_install", "npm install");
    bench_single(&mut group, "npm_dev", "npm run dev");
    bench_single(&mut group, "pytest", "pytest tests/ -v");
    bench_single(&mut group, "go_test", "go test ./...");
    bench_single(&mut group, "make_all", "make all");
    bench_single(&mut group, "git_status", "git status");
    bench_single(&mut group, "tsc_noEmit", "tsc --noEmit");
    bench_single(&mut group, "eslint", "eslint src/ --fix");
    bench_single(&mut group, "docker_build", "docker build -t app .");
    bench_single(&mut group, "terraform_plan", "terraform plan");
    bench_single(&mut group, "bazel_test", "bazel test //...");
    bench_single(&mut group, "gradle_test", "gradle test");
    bench_single(&mut group, "maven_compile", "mvn compile");
    bench_single(&mut group, "uv_run_pytest", "uv run pytest");
    bench_single(&mut group, "swift_build", "swift build");

    // --- Unknown commands (fallthrough path) ---

    bench_single(&mut group, "unknown_simple", "some-random-tool --flag");
    bench_single(
        &mut group,
        "unknown_complex",
        "unknown-wrapper --verbose --config=foo.yaml -- jobs:12",
    );

    group.finish();

    // --- Stress test: classify 1000 unknown commands ---

    let mut stress_group = c.benchmark_group("classify_stress");
    stress_group.throughput(Throughput::Elements(1000));

    // Pre-build 1000 different unknown commands to avoid allocation overhead
    // inside the measurement loop.
    let commands: Vec<Vec<String>> = (0..1000)
        .map(|i| {
            argv(&format!(
                "tool-{i} --flag {i} --option=value{i} sub{i} arg{i}"
            ))
        })
        .collect();

    stress_group.bench_function("classify_1000_unknown", |b| {
        b.iter(|| {
            for cmd in &commands {
                black_box(classify(black_box(cmd)));
            }
        })
    });

    stress_group.finish();

    // --- Throughput: batch of mixed real-world commands ---

    let mut batch_group = c.benchmark_group("classify_batch");
    batch_group.throughput(Throughput::Elements(50));

    let mix: Vec<Vec<String>> = [
        "cargo test",
        "cargo build --release --all-features",
        "cargo check --workspace",
        "cargo clippy -- -D warnings",
        "cargo fmt",
        "npm install",
        "npm run build",
        "npm run test",
        "npm run lint",
        "npm run dev",
        "yarn add express",
        "pnpm install",
        "npx eslint src/",
        "jest --coverage",
        "vitest run",
        "playwright test",
        "tsc --noEmit",
        "eslint src/",
        "prettier --check .",
        "turbo run build",
        "nx run test",
        "deno test",
        "pytest tests/ -v --cov=src",
        "mypy src/",
        "ruff check .",
        "black .",
        "isort .",
        "uv run pytest",
        "uv sync",
        "pip install -r requirements.txt",
        "python -m pytest",
        "go test ./...",
        "go build ./cmd/server",
        "go vet ./...",
        "swift test",
        "swift build",
        "xcodebuild test -scheme MyApp",
        "xcodebuild build -scheme MyApp",
        "gradle test",
        "mvn test",
        "bazel test //...",
        "bazel build //...",
        "make all",
        "cmake --build build",
        "just test",
        "terraform plan",
        "docker build -t app .",
        "docker run --rm app",
        "kubectl apply -f deployment.yaml",
        "some-random-tool --help",
    ]
    .iter()
    .map(|s| argv(s))
    .collect();

    batch_group.bench_function("classify_50_mixed", |b| {
        b.iter(|| {
            for cmd in &mix {
                black_box(classify(black_box(cmd)));
            }
        })
    });

    batch_group.finish();
}

criterion_group!(benches, bench_classifier);
criterion_main!(benches);

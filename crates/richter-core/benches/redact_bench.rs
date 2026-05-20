//! Criterion benchmarks for the secret redaction engine.
//!
//! Redaction runs on every command output before storage or model dispatch.
//! These benchmarks cover the fast path (no secrets), the slow path (many
//! secrets), and large-input scanning.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use richter_core::redact;

/// Generate a string with `n` fake API keys embedded.
fn build_secrets_string(n: usize) -> String {
    let mut s = String::with_capacity(n * 80);
    for i in 0..n {
        s.push_str(&format!(
            "export API_KEY_{i}=sk-proj-abc123def456ghi789jklmno{i:04x}\n"
        ));
    }
    s
}

/// Generate `n` KB of mixed output with some secrets sprinkled in.
fn build_mixed_output(kb: usize) -> String {
    let line = "2024-01-15T10:30:45.123Z  INFO my_service::handler  request processed in 42ms path=/api/v1/users status=200\n";
    let secret_line = "2024-01-15T10:30:46.456Z DEBUG my_service::auth  using token=sk-proj-secret-key-deadbeef-cafe\n";
    let mut s = String::with_capacity(kb * 1024);
    for i in 0..(kb * 1024 / line.len()) {
        if i % 50 == 0 {
            s.push_str(secret_line);
        } else {
            s.push_str(line);
        }
    }
    s
}

fn bench_redact(c: &mut Criterion) {
    let mut group = c.benchmark_group("redact");

    // --- Fast path: no secrets ---

    let clean = "This is normal text with some numbers 12345 and nothing sensitive.";
    group.bench_function("no_secrets", |b| {
        b.iter(|| redact::redact(black_box(clean)));
    });

    // --- Single API key ---

    let single = "OPENAI_API_KEY=sk-proj-abc123def456ghi789jklmnopqrstuv";
    group.bench_function("single_api_key", |b| {
        b.iter(|| redact::redact(black_box(single)));
    });

    // --- 10 API keys ---

    let ten_keys = build_secrets_string(10);
    group.bench_function("10_api_keys", |b| {
        b.iter(|| redact::redact(black_box(&ten_keys)));
    });

    // --- Many API keys (50) ---

    let fifty_keys = build_secrets_string(50);
    group.bench_function("50_api_keys", |b| {
        b.iter(|| redact::redact(black_box(&fifty_keys)));
    });

    // --- Mixed: bearer token in HTTP header ---

    let bearer = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
    group.bench_function("bearer_token", |b| {
        b.iter(|| redact::redact(black_box(bearer)));
    });

    // --- Private key block ---

    let private_key = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA0Z3vKxG9P1dHkNOm8l7TqVwYfU2sB4cD6eF8gH0iJ2kL4mN\n6oP8qR0sT2uV4wX6yZ8aB0cD2eF4gH6iJ8kL0mN2oP4qR6sT8uV0wX2yZ4aB6cD\n-----END RSA PRIVATE KEY-----";
    group.bench_function("private_key_block", |b| {
        b.iter(|| redact::redact(black_box(private_key)));
    });

    // --- Database URL ---

    let db_url = "DATABASE_URL=postgresql://admin:super_secret_pw123@db.example.com:5432/mydb";
    group.bench_function("database_url", |b| {
        b.iter(|| redact::redact(black_box(db_url)));
    });

    // --- JWT token ---

    let jwt = "token=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
    group.bench_function("jwt_token", |b| {
        b.iter(|| redact::redact(black_box(jwt)));
    });

    // --- Contains secrets check (fast boolean) ---

    group.bench_function("contains_secrets_true", |b| {
        b.iter(|| redact::contains_secrets(black_box(single)));
    });

    group.bench_function("contains_secrets_false", |b| {
        b.iter(|| redact::contains_secrets(black_box(clean)));
    });

    // --- Redact with count ---

    group.bench_function("redact_with_count", |b| {
        b.iter(|| redact::redact_with_count(black_box(&ten_keys)));
    });

    // --- Redact bytes ---

    let bytes = ten_keys.as_bytes();
    group.bench_function("redact_bytes", |b| {
        b.iter(|| redact::redact_bytes(black_box(bytes)));
    });

    group.finish();

    // --- Throughput: large mixed output ---

    let mut tp_group = c.benchmark_group("redact_throughput");

    // 1 KB mixed
    let kb1 = build_mixed_output(1);
    tp_group.throughput(Throughput::Bytes(kb1.len() as u64));
    tp_group.bench_function("mixed_1KB", |b| {
        b.iter(|| redact::redact(black_box(&kb1)));
    });

    // 10 KB mixed
    let kb10 = build_mixed_output(10);
    tp_group.throughput(Throughput::Bytes(kb10.len() as u64));
    tp_group.bench_function("mixed_10KB", |b| {
        b.iter(|| redact::redact(black_box(&kb10)));
    });

    // 100 KB mixed (stress)
    let kb100 = build_mixed_output(100);
    tp_group.throughput(Throughput::Bytes(kb100.len() as u64));
    tp_group.bench_function("mixed_100KB", |b| {
        b.iter(|| redact::redact(black_box(&kb100)));
    });

    tp_group.finish();

    // --- JSON redaction ---

    let mut json_group = c.benchmark_group("redact_json");

    let value = serde_json::json!({
        "config": {
            "api_key": "sk-proj-abc123def456ghi789jklmnopqrstuv",
            "normal": "hello"
        },
        "items": ["safe", "sk-ant-api03-deadbeef00112233445566778899aabbcc"]
    });

    json_group.bench_function("nested_json", |b| {
        b.iter(|| redact::redact_json(black_box(&value)));
    });

    json_group.finish();
}

criterion_group!(benches, bench_redact);
criterion_main!(benches);

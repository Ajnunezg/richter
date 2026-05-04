//! Deterministic parsers for popular test/output formats.
//!
//! Parses JUnit XML, TAP, cargo, pytest, xcodebuild, TypeScript errors,
//! ESLint JSON, Go test, Bazel, and Turbo/Nx output into structured
//! [`ParseResult`] values.

use regex::Regex;

// ---------------------------------------------------------------------------
// Parse output
// ---------------------------------------------------------------------------

/// Structured output from an importance parser.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParseResult {
    /// Number of failures detected.
    pub failure_count: usize,
    /// The first failure message (if any).
    pub first_failure: Option<String>,
    /// Concise human-readable reason summary.
    pub reason: String,
    /// Files that changed in this run.
    pub changed_files: Vec<String>,
    /// Raw metadata extracted by the parser.
    pub metadata: serde_json::Value,
}

impl ParseResult {
    /// Create an empty result indicating no failures.
    pub fn success() -> Self {
        Self {
            failure_count: 0,
            first_failure: None,
            reason: "All tests passed".to_string(),
            changed_files: Vec::new(),
            metadata: serde_json::json!({}),
        }
    }

    /// Create a result indicating a failure.
    pub fn failure(failure_count: usize, first_failure: Option<String>, reason: String) -> Self {
        Self {
            failure_count,
            first_failure,
            reason,
            changed_files: Vec::new(),
            metadata: serde_json::json!({}),
        }
    }
}

// ---------------------------------------------------------------------------
// Parser trait
// ---------------------------------------------------------------------------

/// Trait for format-specific output parsers.
pub trait OutputParser: Send + Sync {
    /// Human-readable name of the parser (e.g. "junit", "cargo").
    fn name(&self) -> &'static str;

    /// Parse structured output from raw stdout+stderr and exit code.
    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult;
}

// ---------------------------------------------------------------------------
// JUnit XML
// ---------------------------------------------------------------------------

/// Parses JUnit XML output.
pub struct JunitParser {
    failure_re: Regex,
    total_re: Regex,
}

impl JunitParser {
    /// Create a new JUnit XML parser.
    pub fn new() -> Self {
        Self {
            failure_re: Regex::new(r#"<failure[^>]*>([^<]*)</failure>"#)
                .expect("invalid failure regex"),
            total_re: Regex::new(r#"tests="(\d+)".*failures="(\d+)""#)
                .expect("invalid totals regex"),
        }
    }
}

impl Default for JunitParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputParser for JunitParser {
    fn name(&self) -> &'static str {
        "junit"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        let total_captures = self.total_re.captures(stdout);
        let failure_count: usize = total_captures
            .as_ref()
            .and_then(|c: &regex::Captures| c.get(2))
            .and_then(|m: regex::Match| m.as_str().parse().ok())
            .unwrap_or(0);

        let first_failure: Option<String> = self
            .failure_re
            .captures(stdout)
            .and_then(|c: regex::Captures| c.get(1))
            .map(|m: regex::Match| m.as_str().trim().to_string());

        let total_tests: usize = total_captures
            .and_then(|c: regex::Captures| c.get(1))
            .and_then(|m: regex::Match| m.as_str().parse().ok())
            .unwrap_or(0);

        if failure_count > 0 {
            ParseResult::failure(
                failure_count,
                first_failure,
                format!("{failure_count}/{total_tests} tests failed"),
            )
        } else if exit_code != 0 {
            ParseResult::failure(
                1,
                Some(format!("JUnit exit code {exit_code}")),
                format!("JUnit exited with code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// TAP
// ---------------------------------------------------------------------------

/// Parses TAP (Test Anything Protocol) output.
pub struct TapParser {
    not_ok_re: Regex,
    ok_re: Regex,
}

impl TapParser {
    /// Create a new TAP parser.
    pub fn new() -> Self {
        Self {
            not_ok_re: Regex::new(r"^not ok\b").expect("invalid not ok regex"),
            ok_re: Regex::new(r"^ok\b").expect("invalid ok regex"),
        }
    }
}

impl Default for TapParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputParser for TapParser {
    fn name(&self) -> &'static str {
        "tap"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        let mut failure_count = 0usize;
        let mut first_failure: Option<String> = None;
        let mut total_ok = 0usize;

        for line in stdout.lines() {
            if self.not_ok_re.is_match(line) {
                failure_count += 1;
                if first_failure.is_none() {
                    first_failure = Some(line.to_string());
                }
            } else if self.ok_re.is_match(line) {
                total_ok += 1;
            }
        }

        let total = total_ok + failure_count;
        if failure_count > 0 {
            ParseResult::failure(
                failure_count,
                first_failure,
                format!("{failure_count}/{total} tests failed"),
            )
        } else if exit_code != 0 {
            ParseResult::failure(
                1,
                Some(format!("TAP exit code {exit_code}")),
                format!("TAP exited with code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// Cargo
// ---------------------------------------------------------------------------

/// Parses Rust cargo output.
pub struct CargoParser {
    test_fail_re: Regex,
    test_result_re: Regex,
    compile_error_re: Regex,
    error_re: Regex,
}

impl CargoParser {
    /// Create a new cargo output parser.
    pub fn new() -> Self {
        Self {
            test_fail_re: Regex::new(r"^test .*\.\.\. FAILED$").expect("invalid test fail regex"),
            test_result_re: Regex::new(r"test result: FAILED\. (\d+) passed; (\d+) failed")
                .expect("invalid test result regex"),
            compile_error_re: Regex::new(r"^error: could not compile")
                .expect("invalid compile regex"),
            error_re: Regex::new(r"^error(\[E\d+\])?:").expect("invalid error regex"),
        }
    }
}

impl Default for CargoParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputParser for CargoParser {
    fn name(&self) -> &'static str {
        "cargo"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        if let Some(caps) = self.test_result_re.captures(stdout) {
            let failed: usize = caps
                .get(2)
                .and_then(|m: regex::Match| m.as_str().parse().ok())
                .unwrap_or(0);
            let first = stdout
                .lines()
                .find(|l| self.test_fail_re.is_match(l))
                .map(|l| l.trim().to_string());
            return ParseResult::failure(failed, first, format!("{failed} tests failed"));
        }

        if self.compile_error_re.is_match(stdout) {
            let first = stdout
                .lines()
                .find(|l| self.error_re.is_match(l))
                .map(|l| l.trim().to_string());
            return ParseResult::failure(
                1,
                first.clone().or_else(|| Some("Compilation failed".into())),
                "Compilation error".into(),
            );
        }

        if exit_code != 0 {
            ParseResult::failure(
                1,
                Some(format!("cargo exit code {exit_code}")),
                format!("cargo exit code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// pytest
// ---------------------------------------------------------------------------

/// Parses pytest output.
pub struct PytestParser {
    fail_line_re: Regex,
    summary_re: Regex,
}

impl PytestParser {
    /// Create a new pytest output parser.
    pub fn new() -> Self {
        Self {
            fail_line_re: Regex::new(r"^FAILED\s+(.+?)(?:\s+-\s+.+)?$")
                .expect("invalid FAILED regex"),
            summary_re: Regex::new(r"=+ (?:(\d+) failed)(?:.*?(\d+) passed)?")
                .expect("invalid summary regex"),
        }
    }
}

impl Default for PytestParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputParser for PytestParser {
    fn name(&self) -> &'static str {
        "pytest"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        let mut failure_count = 0usize;
        let mut first_failure: Option<String> = None;

        for line in stdout.lines().rev() {
            if let Some(caps) = self.summary_re.captures(line) {
                failure_count = caps
                    .get(1)
                    .and_then(|m: regex::Match| m.as_str().parse().ok())
                    .unwrap_or(0);
                break;
            }
        }

        if failure_count == 0 {
            for line in stdout.lines() {
                if self.fail_line_re.is_match(line) {
                    failure_count += 1;
                    if first_failure.is_none() {
                        first_failure = Some(line.trim().to_string());
                    }
                }
            }
        } else {
            first_failure = stdout
                .lines()
                .find(|l| l.starts_with("FAILED"))
                .map(|l| l.trim().to_string());
        }

        if failure_count > 0 {
            ParseResult::failure(
                failure_count,
                first_failure,
                format!("{failure_count} pytest failures"),
            )
        } else if exit_code != 0 {
            ParseResult::failure(
                1,
                Some(format!("pytest exit code {exit_code}")),
                format!("pytest exit code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// xcodebuild
// ---------------------------------------------------------------------------

/// Parses xcodebuild output.
pub struct XcodebuildParser {
    error_re: Regex,
    test_fail_re: Regex,
    build_fail_re: Regex,
}

impl XcodebuildParser {
    /// Create a new xcodebuild output parser.
    pub fn new() -> Self {
        Self {
            error_re: Regex::new(r"(?i)error:").expect("invalid error regex"),
            test_fail_re: Regex::new(r"\*\* TEST FAILED \*\*").expect("invalid test fail regex"),
            build_fail_re: Regex::new(r"\*\* BUILD FAILED \*\*").expect("invalid build fail regex"),
        }
    }
}

impl Default for XcodebuildParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputParser for XcodebuildParser {
    fn name(&self) -> &'static str {
        "xcodebuild"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        let build = self.build_fail_re.is_match(stdout);
        let test = self.test_fail_re.is_match(stdout);
        let first_error = stdout
            .lines()
            .find(|l| self.error_re.is_match(l))
            .map(|l| l.trim().to_string());

        if build {
            ParseResult::failure(1, first_error, "Xcode build failed".into())
        } else if test {
            ParseResult::failure(1, first_error, "Xcode tests failed".into())
        } else if exit_code != 0 {
            ParseResult::failure(
                1,
                first_error.or_else(|| Some(format!("xcodebuild exit code {exit_code}"))),
                format!("xcodebuild exit code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// TypeScript (tsc)
// ---------------------------------------------------------------------------

/// Parses TypeScript compiler errors.
pub struct TscParser {
    error_re: Regex,
}

impl TscParser {
    /// Create a new TypeScript compiler output parser.
    pub fn new() -> Self {
        Self {
            error_re: Regex::new(r"^(.+?)\((\d+),(\d+)\):\s+error\s+TS(\d+):\s+(.+)")
                .expect("invalid TS error regex"),
        }
    }
}

impl Default for TscParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputParser for TscParser {
    fn name(&self) -> &'static str {
        "tsc"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        let mut failure_count = 0usize;
        let mut first_failure: Option<String> = None;
        let mut changed_files: Vec<String> = Vec::new();

        for line in stdout.lines() {
            if let Some(caps) = self.error_re.captures(line) {
                failure_count += 1;
                let file = caps
                    .get(1)
                    .map(|m: regex::Match| m.as_str().to_string())
                    .unwrap_or_default();
                let line_num = caps.get(2).map(|m: regex::Match| m.as_str()).unwrap_or("");
                let col = caps.get(3).map(|m: regex::Match| m.as_str()).unwrap_or("");
                let code = caps.get(4).map(|m: regex::Match| m.as_str()).unwrap_or("");
                let msg = caps
                    .get(5)
                    .map(|m: regex::Match| m.as_str().to_string())
                    .unwrap_or_default();

                if !changed_files.contains(&file) {
                    changed_files.push(file.clone());
                }
                if first_failure.is_none() {
                    first_failure = Some(format!("{file}:{line_num}:{col} - TS{code}: {msg}"));
                }
            }
        }

        if failure_count > 0 {
            ParseResult {
                failure_count,
                first_failure,
                reason: format!(
                    "{failure_count} TypeScript errors in {} files",
                    changed_files.len()
                ),
                changed_files,
                metadata: serde_json::json!({"raw_exit": exit_code}),
            }
        } else if exit_code != 0 {
            ParseResult::failure(
                1,
                Some(format!("tsc exit code {exit_code}")),
                format!("tsc exit code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// ESLint JSON
// ---------------------------------------------------------------------------

/// Parses ESLint JSON output.
pub struct EslintParser;

impl EslintParser {
    /// Create a new ESLint JSON output parser.
    pub fn new() -> Self {
        Self
    }
}

impl Default for EslintParser {
    fn default() -> Self {
        Self
    }
}

impl OutputParser for EslintParser {
    fn name(&self) -> &'static str {
        "eslint"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        let trimmed = stdout.trim();
        if !trimmed.starts_with('[') {
            return if exit_code != 0 {
                ParseResult::failure(1, None, format!("eslint exit code {exit_code}"))
            } else {
                ParseResult::success()
            };
        }

        let results: Vec<serde_json::Value> = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(_) => {
                return if exit_code != 0 {
                    ParseResult::failure(1, None, format!("eslint exit code {exit_code}"))
                } else {
                    ParseResult::success()
                };
            }
        };

        let mut failure_count = 0usize;
        let mut first_failure: Option<String> = None;
        let mut changed_files: Vec<String> = Vec::new();

        for file_result in &results {
            let file_path = file_result["filePath"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            if let Some(messages) = file_result["messages"].as_array() {
                let errors: Vec<_> = messages
                    .iter()
                    .filter(|m| {
                        m["severity"].as_u64() == Some(2)
                            || m.get("fatal").and_then(|f| f.as_bool()) == Some(true)
                    })
                    .collect();

                if !errors.is_empty() {
                    failure_count += errors.len();
                    if !changed_files.contains(&file_path) {
                        changed_files.push(file_path.clone());
                    }
                    if first_failure.is_none() {
                        let first = &errors[0];
                        let rule = first["ruleId"].as_str().unwrap_or("?");
                        let msg = first["message"].as_str().unwrap_or("?");
                        let line = first["line"].as_u64().unwrap_or(0);
                        first_failure = Some(format!("{file_path}:{line} - {rule}: {msg}"));
                    }
                }
            }
        }

        if failure_count > 0 {
            ParseResult {
                failure_count,
                first_failure,
                reason: format!(
                    "{failure_count} ESLint errors in {} files",
                    changed_files.len()
                ),
                changed_files,
                metadata: serde_json::json!({"raw_exit": exit_code}),
            }
        } else if exit_code != 0 {
            ParseResult::failure(
                1,
                Some(format!("eslint exit code {exit_code}")),
                format!("eslint exit code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// Go test
// ---------------------------------------------------------------------------

/// Parses Go test output.
pub struct GoTestParser {
    fail_re: Regex,
    build_fail_re: Regex,
}

impl GoTestParser {
    /// Create a new Go test output parser.
    pub fn new() -> Self {
        Self {
            fail_re: Regex::new(r"^--- FAIL:").expect("invalid FAIL regex"),
            build_fail_re: Regex::new(r"^# \S+").expect("invalid build fail regex"),
        }
    }
}

impl Default for GoTestParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputParser for GoTestParser {
    fn name(&self) -> &'static str {
        "go_test"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        let mut failure_count = 0usize;
        let mut first_failure: Option<String> = None;

        for line in stdout.lines() {
            if self.fail_re.is_match(line) {
                failure_count += 1;
                if first_failure.is_none() {
                    first_failure = Some(line.trim().to_string());
                }
            }
        }

        if failure_count > 0 {
            ParseResult::failure(
                failure_count,
                first_failure,
                format!("{failure_count} Go tests failed"),
            )
        } else if self.build_fail_re.is_match(stdout) {
            ParseResult::failure(1, None, "Go build failed".into())
        } else if exit_code != 0 {
            ParseResult::failure(
                1,
                Some(format!("go test exit code {exit_code}")),
                format!("go test exit code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// Bazel
// ---------------------------------------------------------------------------

/// Parses Bazel output.
pub struct BazelParser {
    test_summary_re: Regex,
    build_fail_re: Regex,
    fail_re: Regex,
}

impl BazelParser {
    /// Create a new Bazel output parser.
    pub fn new() -> Self {
        Self {
            test_summary_re: Regex::new(
                r"Test cases: finished with (\d+) passing and (\d+) failing",
            )
            .expect("invalid bazel summary regex"),
            build_fail_re: Regex::new(r"BUILD FAILED").expect("invalid build fail regex"),
            fail_re: Regex::new(r"^\s*(FAILED|TIMEOUT|FLAKY)\s").expect("invalid bazel fail regex"),
        }
    }
}

impl Default for BazelParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputParser for BazelParser {
    fn name(&self) -> &'static str {
        "bazel"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        if let Some(caps) = self.test_summary_re.captures(stdout) {
            let failed: usize = caps
                .get(2)
                .and_then(|m: regex::Match| m.as_str().parse().ok())
                .unwrap_or(0);
            if failed > 0 {
                let first = stdout
                    .lines()
                    .find(|l| l.contains("FAILED") || l.contains("TIMEOUT"))
                    .map(|l| l.trim().to_string());
                return ParseResult::failure(failed, first, format!("{failed} Bazel tests failed"));
            }
        }

        if self.build_fail_re.is_match(stdout) {
            let first = stdout
                .lines()
                .find(|l| self.build_fail_re.is_match(l))
                .map(|l| l.trim().to_string());
            return ParseResult::failure(1, first, "Bazel build failed".into());
        }

        let mut fail_count = 0usize;
        let mut first: Option<String> = None;
        for line in stdout.lines() {
            if self.fail_re.is_match(line) {
                fail_count += 1;
                if first.is_none() {
                    first = Some(line.trim().to_string());
                }
            }
        }

        if fail_count > 0 {
            ParseResult::failure(
                fail_count,
                first,
                format!("{fail_count} Bazel targets failed"),
            )
        } else if exit_code != 0 {
            ParseResult::failure(
                1,
                Some(format!("bazel exit code {exit_code}")),
                format!("bazel exit code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// Turbo / Nx
// ---------------------------------------------------------------------------

/// Parses Turbo/Nx monorepo tool output.
pub struct TurboNxParser {
    command_fail_re: Regex,
}

impl TurboNxParser {
    /// Create a new Turbo/Nx output parser.
    pub fn new() -> Self {
        Self {
            command_fail_re: Regex::new(r"Command failed").expect("invalid command fail regex"),
        }
    }
}

impl Default for TurboNxParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputParser for TurboNxParser {
    fn name(&self) -> &'static str {
        "turbo_nx"
    }

    fn parse(&self, stdout: &str, exit_code: i32) -> ParseResult {
        let mut failure_count = 0usize;
        let mut first_failure: Option<String> = None;

        for line in stdout.lines() {
            let t = line.trim();
            if self.command_fail_re.is_match(t)
                || t.starts_with("ERROR")
                || (t.starts_with("error") && !t.starts_with("error: could not"))
            {
                failure_count += 1;
                if first_failure.is_none() {
                    first_failure = Some(t.to_string());
                }
            }
        }

        if failure_count > 0 {
            ParseResult::failure(
                failure_count,
                first_failure,
                format!("{failure_count} turbo/nx task failures"),
            )
        } else if exit_code != 0 {
            ParseResult::failure(
                1,
                Some(format!("exit code {exit_code}")),
                format!("turbo/nx exit code {exit_code}"),
            )
        } else {
            ParseResult::success()
        }
    }
}

// ---------------------------------------------------------------------------
// Auto-detection
// ---------------------------------------------------------------------------

/// Attempt to auto-detect the correct parser from a command string.
pub fn detect_parser_from_command(command: &str) -> Option<Box<dyn OutputParser>> {
    let lower = command.to_lowercase();
    if lower.contains("cargo test")
        || lower.contains("cargo build")
        || lower.contains("cargo clippy")
    {
        Some(Box::<CargoParser>::default())
    } else if lower.contains("pytest") || lower.contains("python -m pytest") {
        Some(Box::<PytestParser>::default())
    } else if lower.contains("xcodebuild") {
        Some(Box::<XcodebuildParser>::default())
    } else if lower.contains("tsc") || lower.contains("typescript") {
        Some(Box::<TscParser>::default())
    } else if lower.contains("eslint") {
        Some(Box::<EslintParser>::default())
    } else if lower.contains("go test") {
        Some(Box::<GoTestParser>::default())
    } else if lower.contains("bazel") {
        Some(Box::<BazelParser>::default())
    } else if lower.contains("turbo") || lower.contains("nx ") {
        Some(Box::<TurboNxParser>::default())
    } else if lower.contains("tap") || lower.contains("prove") {
        Some(Box::<TapParser>::default())
    } else {
        Some(Box::<JunitParser>::default())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cargo_pass() {
        let p = CargoParser::new();
        let r = p.parse("test result: ok. 5 passed; 0 failed; 0 ignored", 0);
        assert_eq!(r.failure_count, 0);
    }

    #[test]
    fn test_cargo_fail() {
        let p = CargoParser::new();
        let r = p.parse(
            "test a ... FAILED\ntest result: FAILED. 1 passed; 1 failed",
            101,
        );
        assert_eq!(r.failure_count, 1);
    }

    #[test]
    fn test_junit() {
        let p = JunitParser::new();
        let xml = r#"<testsuite name="s" tests="3" failures="2"><testcase name="a"><failure message="b"/></testcase></testsuite>"#;
        let r = p.parse(xml, 1);
        assert_eq!(r.failure_count, 2);
    }

    #[test]
    fn test_tap() {
        let p = TapParser::new();
        let r = p.parse("1..3\nok 1\nnot ok 2\nok 3", 1);
        assert_eq!(r.failure_count, 1);
    }

    #[test]
    fn test_pytest() {
        let p = PytestParser::new();
        let r = p.parse(
            "FAILED test_foo.py::test_bar\n==== 1 failed, 8 passed in 1.2s ====",
            1,
        );
        assert_eq!(r.failure_count, 1);
    }

    #[test]
    fn test_tsc() {
        let p = TscParser::new();
        let r = p.parse(
            "src/app.ts(10,5): error TS2322: Type 'string' is not assignable",
            2,
        );
        assert_eq!(r.failure_count, 1);
        assert_eq!(r.changed_files.len(), 1);
    }

    #[test]
    fn test_eslint_json() {
        let p = EslintParser::new();
        let out = r#"[{"filePath":"/x.js","messages":[{"ruleId":"no-unused","severity":2,"message":"'x' unused","line":10,"column":5}],"errorCount":1}]"#;
        let r = p.parse(out, 1);
        assert_eq!(r.failure_count, 1);
    }

    #[test]
    fn test_go_test() {
        let p = GoTestParser::new();
        let r = p.parse("--- FAIL: TestFoo (0.00s)\nFAIL\nFAIL\texample\t0.123s", 1);
        assert_eq!(r.failure_count, 1);
    }

    #[test]
    fn test_bazel() {
        let p = BazelParser::new();
        let r = p.parse(
            "//pkg:test_a FAILED\nTest cases: finished with 0 passing and 1 failing",
            1,
        );
        assert_eq!(r.failure_count, 1);
    }

    #[test]
    fn test_turbo() {
        let p = TurboNxParser::new();
        let r = p.parse("my-app:test: ERROR: Tests failed\nCommand failed: turbo", 1);
        assert!(r.failure_count > 0);
    }

    #[test]
    fn test_detect_parser() {
        assert_eq!(
            detect_parser_from_command("cargo test").unwrap().name(),
            "cargo"
        );
        assert_eq!(
            detect_parser_from_command("pytest -x").unwrap().name(),
            "pytest"
        );
        assert_eq!(
            detect_parser_from_command("tsc --noEmit").unwrap().name(),
            "tsc"
        );
    }
}

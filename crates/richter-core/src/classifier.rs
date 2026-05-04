//! Deterministic command classifier.
//!
//! Classifies shell commands into predefined classes (Build, Test, Lint,
//! Typecheck, Format, Install, DevServer, Migration, Destructive, Unknown)
//! based on ecosystem-specific arg parsers.

use crate::models::CommandClass;

/// A classified command with its class and a canonical representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedCommand {
    /// The command class.
    pub class: CommandClass,
    /// The primary tool name detected.
    pub tool: String,
    /// The subcommand or script, if detected.
    pub subcommand: Option<String>,
    /// Whether this command is likely interactive.
    pub is_interactive: bool,
    /// Whether this command is potentially destructive.
    pub is_destructive: bool,
    /// A canonical representation for fingerprinting.
    pub canonical: Vec<String>,
}

/// Classify a command from its argument vector and optional current
/// working directory hint.
pub fn classify(argv: &[String]) -> ClassifiedCommand {
    if argv.is_empty() {
        return ClassifiedCommand {
            class: CommandClass::Unknown,
            tool: String::new(),
            subcommand: None,
            is_interactive: false,
            is_destructive: false,
            canonical: argv.to_vec(),
        };
    }

    let tool = extract_tool_name(&argv[0]);
    let normalized_argv = normalize_argv(argv, &tool);
    let is_destructive = check_destructive(&normalized_argv);

    // Try ecosystem-specific classifiers first
    let result = classify_js_ts(&normalized_argv)
        .or_else(|| classify_python(&normalized_argv))
        .or_else(|| classify_rust(&normalized_argv))
        .or_else(|| classify_go(&normalized_argv))
        .or_else(|| classify_swift_xcode(&normalized_argv))
        .or_else(|| classify_java(&normalized_argv))
        .or_else(|| classify_bazel(&normalized_argv))
        .or_else(|| classify_generic_tools(&normalized_argv))
        .unwrap_or(CommandClass::Unknown);

    let is_interactive = check_interactive(&normalized_argv);

    ClassifiedCommand {
        class: result,
        tool,
        subcommand: normalized_argv.get(1).cloned(),
        is_interactive,
        is_destructive,
        canonical: normalized_argv,
    }
}

/// Extract the bare tool name from an argv[0] path.
pub fn extract_tool_name(argv0: &str) -> String {
    let path = std::path::Path::new(argv0);
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(argv0)
        .to_string()
}

/// Normalize argv: strip wrapper prefixes (e.g. `richter run --`),
/// resolve shim names, canonicalize common aliases.
fn normalize_argv(argv: &[String], tool: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(argv.len());

    // Find "--" separator for shim/passthrough commands
    let start_idx = argv.iter().position(|a| a == "--").map_or(0, |i| i + 1);
    let slice = if start_idx > 0 && start_idx < argv.len() {
        &argv[start_idx..]
    } else {
        argv
    };

    // Map common shim names
    let mapped_tool = match tool {
        "npm" | "pnpm" | "yarn" | "bun" | "deno" | "node" | "npx" | "cargo" | "go" | "python"
        | "pytest" | "uv" | "ruff" | "make" | "cmake" | "ninja" | "xcodebuild" | "swift"
        | "gradle" | "mvn" | "bazel" | "turbo" | "nx" | "tsc" | "eslint" | "jest" | "vitest"
        | "playwright" => tool.to_string(),
        _ => slice.first().cloned().unwrap_or_default(),
    };

    out.push(mapped_tool);
    if slice.len() > 1 {
        out.extend_from_slice(&slice[1..]);
    }

    out
}

// ---------------------------------------------------------------------------
// JS/TS ecosystem
// ---------------------------------------------------------------------------

fn classify_js_ts(argv: &[String]) -> Option<CommandClass> {
    let tool = argv.first()?.as_str();

    match tool {
        "npm" | "pnpm" | "yarn" | "bun" => classify_node_package_manager(argv),
        "npx" => Some(CommandClass::Unknown), // Passthrough
        "turbo" | "nx" => classify_monorepo_tool(argv),
        "jest" => Some(CommandClass::Test),
        "vitest" => Some(CommandClass::Test),
        "playwright" => Some(CommandClass::Test),
        "tsc" => {
            if argv.len() >= 2 && (argv[1] == "--noEmit" || argv.iter().any(|a| a == "--noEmit")) {
                Some(CommandClass::Typecheck)
            } else {
                Some(CommandClass::Build)
            }
        }
        "eslint" => Some(CommandClass::Lint),
        "prettier" => Some(CommandClass::Format),
        "deno" => classify_deno(argv),
        _ => None,
    }
}

fn classify_node_package_manager(argv: &[String]) -> Option<CommandClass> {
    let sub = argv.get(1).map(|s| s.as_str());

    match sub {
        Some("install" | "i" | "add") => Some(CommandClass::Install),
        Some("run") | Some("run-script") => classify_npm_run(argv),
        Some("test" | "t" | "tst") => Some(CommandClass::Test),
        Some("build") => Some(CommandClass::Build),
        Some("lint") => Some(CommandClass::Lint),
        Some("typecheck" | "type-check") => Some(CommandClass::Typecheck),
        Some("format" | "fmt") => Some(CommandClass::Format),
        Some("dev" | "start" | "serve") => Some(CommandClass::DevServer),
        Some("exec") | Some("x") => Some(CommandClass::Unknown),
        Some("remove" | "rm" | "uninstall" | "prune") => Some(CommandClass::Install),
        _ => Some(CommandClass::Unknown),
    }
}

fn classify_npm_run(argv: &[String]) -> Option<CommandClass> {
    let script = argv.get(2).map(|s| s.as_str());

    match script {
        Some("build" | "compile") => Some(CommandClass::Build),
        Some("test" | "tst" | "spec") => Some(CommandClass::Test),
        Some("lint" | "eslint") => Some(CommandClass::Lint),
        Some("typecheck" | "type-check" | "check-types") => Some(CommandClass::Typecheck),
        Some("format" | "fmt" | "prettier") => Some(CommandClass::Format),
        Some("dev" | "start" | "serve") => Some(CommandClass::DevServer),
        Some("migrate" | "db:migrate" | "migration") => Some(CommandClass::Migration),
        _ => Some(CommandClass::Unknown),
    }
}

fn classify_monorepo_tool(argv: &[String]) -> Option<CommandClass> {
    let has_sub = argv.len() >= 3;
    let sub = argv.get(1).map(|s| s.as_str());

    match sub {
        Some("run") | Some("exec") if has_sub => classify_monorepo_run(argv),
        Some("build") => Some(CommandClass::Build),
        Some("test") => Some(CommandClass::Test),
        Some("lint") => Some(CommandClass::Lint),
        Some("typecheck") => Some(CommandClass::Typecheck),
        _ => Some(CommandClass::Unknown),
    }
}

fn classify_monorepo_run(argv: &[String]) -> Option<CommandClass> {
    let task = argv.get(2).map(|s| s.as_str());
    match task {
        Some("build" | "compile") => Some(CommandClass::Build),
        Some("test" | "spec") => Some(CommandClass::Test),
        Some("lint") => Some(CommandClass::Lint),
        Some("typecheck" | "type-check") => Some(CommandClass::Typecheck),
        Some("format" | "fmt") => Some(CommandClass::Format),
        Some("dev" | "start" | "serve") => Some(CommandClass::DevServer),
        _ => Some(CommandClass::Unknown),
    }
}

fn classify_deno(argv: &[String]) -> Option<CommandClass> {
    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("test") => Some(CommandClass::Test),
        Some("lint") => Some(CommandClass::Lint),
        Some("fmt") => Some(CommandClass::Format),
        Some("check") => Some(CommandClass::Typecheck),
        Some("run" | "serve") => Some(CommandClass::DevServer),
        Some("compile" | "bundle") => Some(CommandClass::Build),
        _ => Some(CommandClass::Unknown),
    }
}

// ---------------------------------------------------------------------------
// Python ecosystem
// ---------------------------------------------------------------------------

fn classify_python(argv: &[String]) -> Option<CommandClass> {
    let tool = argv.first()?.as_str();

    match tool {
        "pytest" => Some(CommandClass::Test),
        "uv" => classify_uv(argv),
        "ruff" => Some(CommandClass::Lint),
        "mypy" => Some(CommandClass::Typecheck),
        "black" => Some(CommandClass::Format),
        "isort" => Some(CommandClass::Format),
        "pip" => classify_pip(argv),
        "python" => classify_python_script(argv),
        _ => None,
    }
}

fn classify_uv(argv: &[String]) -> Option<CommandClass> {
    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("run") if argv.len() >= 3 => {
            let inner = argv.get(2).map(|s| s.as_str());
            match inner {
                Some("pytest") => Some(CommandClass::Test),
                Some("ruff") => Some(CommandClass::Lint),
                Some("mypy") => Some(CommandClass::Typecheck),
                Some("black") | Some("isort") => Some(CommandClass::Format),
                _ => Some(CommandClass::Unknown),
            }
        }
        Some("pip" | "install" | "add") => Some(CommandClass::Install),
        Some("sync" | "lock") => Some(CommandClass::Install),
        Some("build") => Some(CommandClass::Build),
        _ => Some(CommandClass::Unknown),
    }
}

fn classify_pip(argv: &[String]) -> Option<CommandClass> {
    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("install" | "download" | "uninstall") => Some(CommandClass::Install),
        _ => Some(CommandClass::Unknown),
    }
}

fn classify_python_script(argv: &[String]) -> Option<CommandClass> {
    let script = argv.get(1).map(|s| s.as_str());
    match script {
        Some("-m") if argv.len() >= 3 => {
            let module = &argv[2];
            match module.as_str() {
                "pytest" => Some(CommandClass::Test),
                "unittest" => Some(CommandClass::Test),
                "mypy" => Some(CommandClass::Typecheck),
                "black" => Some(CommandClass::Format),
                "isort" => Some(CommandClass::Format),
                "ruff" => Some(CommandClass::Lint),
                "pip" if argv.len() >= 4 && argv[3] == "install" => Some(CommandClass::Install),
                "http.server" => Some(CommandClass::DevServer),
                _ => Some(CommandClass::Unknown),
            }
        }
        Some("-c") | Some("--check") => Some(CommandClass::Typecheck),
        Some("-m") => Some(CommandClass::Unknown),
        _ => Some(CommandClass::Unknown),
    }
}

// ---------------------------------------------------------------------------
// Rust ecosystem
// ---------------------------------------------------------------------------

fn classify_rust(argv: &[String]) -> Option<CommandClass> {
    let tool = argv.first()?.as_str();
    if tool != "cargo" {
        return None;
    }

    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("build" | "b") => Some(CommandClass::Build),
        Some("test" | "t") => {
            // cargo test --check is a typecheck, not a test run
            if argv.iter().any(|a| a == "--check") {
                Some(CommandClass::Typecheck)
            } else {
                Some(CommandClass::Test)
            }
        }
        Some("check" | "c") => Some(CommandClass::Typecheck),
        Some("clippy") => Some(CommandClass::Lint),
        Some("fmt") => Some(CommandClass::Format),
        Some("run" | "r") => Some(CommandClass::DevServer),
        Some("install" | "add") => Some(CommandClass::Install),
        Some("doc") => Some(CommandClass::Build),
        Some("clean") => Some(CommandClass::Destructive),
        _ => Some(CommandClass::Unknown),
    }
}

// ---------------------------------------------------------------------------
// Go ecosystem
// ---------------------------------------------------------------------------

fn classify_go(argv: &[String]) -> Option<CommandClass> {
    let tool = argv.first()?.as_str();
    if tool != "go" {
        return None;
    }

    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("build") => Some(CommandClass::Build),
        Some("test") => {
            // go test -run=... is a test
            Some(CommandClass::Test)
        }
        Some("vet") => Some(CommandClass::Lint),
        Some("fmt") => Some(CommandClass::Format),
        Some("run") => Some(CommandClass::DevServer),
        Some("mod") if argv.len() >= 3 => {
            let mod_sub = &argv[2];
            match mod_sub.as_str() {
                "tidy" | "download" | "vendor" => Some(CommandClass::Install),
                _ => Some(CommandClass::Unknown),
            }
        }
        Some("get" | "install") => Some(CommandClass::Install),
        _ => Some(CommandClass::Unknown),
    }
}

// ---------------------------------------------------------------------------
// Swift / Xcode ecosystem
// ---------------------------------------------------------------------------

fn classify_swift_xcode(argv: &[String]) -> Option<CommandClass> {
    let tool = argv.first()?.as_str();

    match tool {
        "swift" => classify_swift(argv),
        "xcodebuild" => classify_xcodebuild(argv),
        _ => None,
    }
}

fn classify_swift(argv: &[String]) -> Option<CommandClass> {
    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("build") => Some(CommandClass::Build),
        Some("test") => Some(CommandClass::Test),
        Some("run") => Some(CommandClass::DevServer),
        Some("package") if argv.len() >= 3 && argv[2] == "resolve" => Some(CommandClass::Install),
        _ => Some(CommandClass::Unknown),
    }
}

fn classify_xcodebuild(argv: &[String]) -> Option<CommandClass> {
    let has_test = argv
        .iter()
        .any(|a| a == "test" || a == "test-without-building");
    let has_build = argv
        .iter()
        .any(|a| a == "build" || a == "build-for-testing");
    let has_archive = argv.iter().any(|a| a == "archive");

    if has_test {
        Some(CommandClass::Test)
    } else if has_build || has_archive {
        Some(CommandClass::Build)
    } else {
        Some(CommandClass::Unknown)
    }
}

// ---------------------------------------------------------------------------
// Java / Kotlin ecosystem (Gradle, Maven)
// ---------------------------------------------------------------------------

fn classify_java(argv: &[String]) -> Option<CommandClass> {
    let tool = argv.first()?.as_str();

    match tool {
        "gradle" | "gradlew" | "./gradlew" => classify_gradle(argv),
        "mvn" | "mvnw" | "./mvnw" => classify_maven(argv),
        _ => None,
    }
}

fn classify_gradle(argv: &[String]) -> Option<CommandClass> {
    for arg in &argv[1..] {
        if arg.starts_with('-') {
            continue;
        }
        return match arg.as_str() {
            "build" | "assemble" | "compileJava" | "compileKotlin" => Some(CommandClass::Build),
            "test" | "check" => Some(CommandClass::Test),
            "lint" | "ktlint" | "detekt" | "spotlessCheck" => Some(CommandClass::Lint),
            "spotlessApply" | "ktlintFormat" => Some(CommandClass::Format),
            "bootRun" | "run" => Some(CommandClass::DevServer),
            "dependencies" | "dependencyUpdates" => Some(CommandClass::Install),
            "clean" => Some(CommandClass::Destructive),
            _ => Some(CommandClass::Unknown),
        };
    }
    Some(CommandClass::Unknown)
}

fn classify_maven(argv: &[String]) -> Option<CommandClass> {
    for arg in &argv[1..] {
        if arg.starts_with('-') {
            continue;
        }
        return match arg.as_str() {
            "compile" | "package" | "install" | "verify" => Some(CommandClass::Build),
            "test" | "surefire:test" | "failsafe:integration-test" => Some(CommandClass::Test),
            "checkstyle:check" | "pmd:check" | "spotbugs:check" => Some(CommandClass::Lint),
            "clean" => Some(CommandClass::Destructive),
            "spring-boot:run" | "quarkus:dev" | "jetty:run" => Some(CommandClass::DevServer),
            "flyway:migrate" | "liquibase:update" => Some(CommandClass::Migration),
            _ => Some(CommandClass::Unknown),
        };
    }
    Some(CommandClass::Unknown)
}

// ---------------------------------------------------------------------------
// Bazel ecosystem
// ---------------------------------------------------------------------------

fn classify_bazel(argv: &[String]) -> Option<CommandClass> {
    let tool = argv.first()?.as_str();
    if tool != "bazel" && tool != "bazelisk" {
        return None;
    }

    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("build" | "b") => Some(CommandClass::Build),
        Some("test" | "t") => Some(CommandClass::Test),
        Some("run" | "r") => Some(CommandClass::DevServer),
        Some("coverage") => Some(CommandClass::Test),
        Some("clean") => Some(CommandClass::Destructive),
        _ => Some(CommandClass::Unknown),
    }
}

// ---------------------------------------------------------------------------
// Generic tools
// ---------------------------------------------------------------------------

fn classify_generic_tools(argv: &[String]) -> Option<CommandClass> {
    let tool = argv.first()?.as_str();

    match tool {
        "make" => classify_make(argv),
        "cmake" => classify_cmake(argv),
        "ninja" => Some(CommandClass::Build),
        "just" => {
            let target = argv.get(1).map(|s| s.as_str());
            match target {
                Some("build") => Some(CommandClass::Build),
                Some("test") => Some(CommandClass::Test),
                Some("lint" | "clippy") => Some(CommandClass::Lint),
                Some("fmt" | "format") => Some(CommandClass::Format),
                Some("install") => Some(CommandClass::Install),
                Some("dev" | "serve" | "run") => Some(CommandClass::DevServer),
                _ => Some(CommandClass::Unknown),
            }
        }
        "terraform" | "tofu" => classify_terraform(argv),
        "kubectl" => classify_kubectl(argv),
        "docker" => classify_docker(argv),
        _ => None,
    }
}

fn classify_make(argv: &[String]) -> Option<CommandClass> {
    for arg in &argv[1..] {
        if arg.starts_with('-') || arg.contains('=') {
            continue;
        }
        let lower = arg.to_lowercase();
        if lower.contains("build") || lower.contains("compile") {
            return Some(CommandClass::Build);
        }
        if lower.contains("test") || lower.contains("check") {
            return Some(CommandClass::Test);
        }
        if lower.contains("lint") || lower.contains("vet") {
            return Some(CommandClass::Lint);
        }
        if lower.contains("fmt") || lower.contains("format") {
            return Some(CommandClass::Format);
        }
        if lower.contains("install") {
            return Some(CommandClass::Install);
        }
        if lower.contains("clean") || lower.contains("clobber") {
            return Some(CommandClass::Destructive);
        }
    }
    Some(CommandClass::Build) // Default for make
}

fn classify_cmake(argv: &[String]) -> Option<CommandClass> {
    for arg in &argv[1..] {
        if arg == "--build" {
            return Some(CommandClass::Build);
        }
    }
    if argv.iter().any(|a| a == "--build" || a == "--install") {
        Some(CommandClass::Build)
    } else {
        Some(CommandClass::Install) // Default cmake is configure
    }
}

fn classify_terraform(argv: &[String]) -> Option<CommandClass> {
    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("apply" | "destroy") => Some(CommandClass::Destructive),
        Some("plan") => Some(CommandClass::Unknown),
        Some("init") => Some(CommandClass::Install),
        _ => Some(CommandClass::Unknown),
    }
}

fn classify_kubectl(argv: &[String]) -> Option<CommandClass> {
    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("apply") => Some(CommandClass::Destructive),
        Some("delete") => Some(CommandClass::Destructive),
        _ => Some(CommandClass::Unknown),
    }
}

fn classify_docker(argv: &[String]) -> Option<CommandClass> {
    let sub = argv.get(1).map(|s| s.as_str());
    match sub {
        Some("build" | "buildx") => Some(CommandClass::Build),
        Some("rm" | "rmi" | "prune" | "system") => Some(CommandClass::Destructive),
        Some("run" | "compose") if argv.len() >= 3 => {
            let inner = &argv[2];
            match inner.as_str() {
                "up" | "start" => Some(CommandClass::DevServer),
                "down" | "rm" => Some(CommandClass::Destructive),
                "build" => Some(CommandClass::Build),
                _ => Some(CommandClass::Unknown),
            }
        }
        _ => Some(CommandClass::Unknown),
    }
}

// ---------------------------------------------------------------------------
// Destructive & interactive detection
// ---------------------------------------------------------------------------

fn check_destructive(argv: &[String]) -> bool {
    argv.iter().any(|a| {
        let lower = a.to_lowercase();
        lower == "rm"
            || lower == "rmdir"
            || lower == "unlink"
            || lower == "del"
            || lower == "drop"
            || lower == "purge"
            || lower == "clean"
            || lower == "clobber"
            || lower == "destroy"
    })
}

fn check_interactive(argv: &[String]) -> bool {
    argv.iter().any(|a| {
        a == "-i"
            || a == "--interactive"
            || a == "-it"
            || a == "--tty"
            || a == "bash"
            || a == "zsh"
            || a == "sh"
            || a == "fish"
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tool_name() {
        assert_eq!(extract_tool_name("npm"), "npm");
        assert_eq!(extract_tool_name("/usr/bin/cargo"), "cargo");
        assert_eq!(extract_tool_name("./node_modules/.bin/eslint"), "eslint");
    }

    #[test]
    fn test_classify_npm_install() {
        let c = classify(&["npm".into(), "install".into()]);
        assert_eq!(c.class, CommandClass::Install);
        assert_eq!(c.tool, "npm");
    }

    #[test]
    fn test_classify_npm_test() {
        let c = classify(&["npm".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_npm_run_build() {
        let c = classify(&["npm".into(), "run".into(), "build".into()]);
        assert_eq!(c.class, CommandClass::Build);
    }

    #[test]
    fn test_classify_npm_run_test() {
        let c = classify(&["pnpm".into(), "run".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_npm_dev() {
        let c = classify(&["yarn".into(), "dev".into()]);
        assert_eq!(c.class, CommandClass::DevServer);
    }

    #[test]
    fn test_classify_cargo_build() {
        let c = classify(&["cargo".into(), "build".into()]);
        assert_eq!(c.class, CommandClass::Build);
        assert_eq!(c.tool, "cargo");
    }

    #[test]
    fn test_classify_cargo_test() {
        let c = classify(&["cargo".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_cargo_check() {
        let c = classify(&["cargo".into(), "check".into()]);
        assert_eq!(c.class, CommandClass::Typecheck);
    }

    #[test]
    fn test_classify_cargo_clippy() {
        let c = classify(&["cargo".into(), "clippy".into()]);
        assert_eq!(c.class, CommandClass::Lint);
    }

    #[test]
    fn test_classify_cargo_fmt() {
        let c = classify(&["cargo".into(), "fmt".into()]);
        assert_eq!(c.class, CommandClass::Format);
    }

    #[test]
    fn test_classify_cargo_clean() {
        let c = classify(&["cargo".into(), "clean".into()]);
        assert_eq!(c.class, CommandClass::Destructive);
    }

    #[test]
    fn test_classify_go_test() {
        let c = classify(&["go".into(), "test".into(), "./...".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_go_build() {
        let c = classify(&["go".into(), "build".into()]);
        assert_eq!(c.class, CommandClass::Build);
    }

    #[test]
    fn test_classify_pytest() {
        let c = classify(&["pytest".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_mypy() {
        let c = classify(&["mypy".into(), "src/".into()]);
        assert_eq!(c.class, CommandClass::Typecheck);
    }

    #[test]
    fn test_classify_ruff() {
        let c = classify(&["ruff".into(), "check".into(), ".".into()]);
        assert_eq!(c.class, CommandClass::Lint);
    }

    #[test]
    fn test_classify_uv_run_pytest() {
        let c = classify(&["uv".into(), "run".into(), "pytest".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_tsc_noemit() {
        let c = classify(&["tsc".into(), "--noEmit".into()]);
        assert_eq!(c.class, CommandClass::Typecheck);
    }

    #[test]
    fn test_classify_eslint() {
        let c = classify(&["eslint".into(), "src/".into()]);
        assert_eq!(c.class, CommandClass::Lint);
    }

    #[test]
    fn test_classify_jest() {
        let c = classify(&["jest".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_swift_test() {
        let c = classify(&["swift".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_xcodebuild_test() {
        let c = classify(&[
            "xcodebuild".into(),
            "test".into(),
            "-scheme".into(),
            "MyApp".into(),
        ]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_xcodebuild_build() {
        let c = classify(&[
            "xcodebuild".into(),
            "build".into(),
            "-scheme".into(),
            "MyApp".into(),
        ]);
        assert_eq!(c.class, CommandClass::Build);
    }

    #[test]
    fn test_classify_gradle_test() {
        let c = classify(&["gradle".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_maven_test() {
        let c = classify(&["mvn".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_bazel_test() {
        let c = classify(&["bazel".into(), "test".into(), "//...".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_bazel_build() {
        let c = classify(&["bazelisk".into(), "build".into(), "//...".into()]);
        assert_eq!(c.class, CommandClass::Build);
    }

    #[test]
    fn test_classify_turbo_build() {
        let c = classify(&["turbo".into(), "run".into(), "build".into()]);
        assert_eq!(c.class, CommandClass::Build);
    }

    #[test]
    fn test_classify_terraform_destroy() {
        let c = classify(&["terraform".into(), "destroy".into()]);
        assert_eq!(c.class, CommandClass::Destructive);
    }

    #[test]
    fn test_classify_unknown() {
        let c = classify(&["some-random-tool".into(), "--help".into()]);
        assert_eq!(c.class, CommandClass::Unknown);
    }

    #[test]
    fn test_classify_destructive_rm() {
        assert!(check_destructive(&[
            "rm".into(),
            "-rf".into(),
            "dir".into()
        ]));
    }

    #[test]
    fn test_classify_empty_argv() {
        let c = classify(&[]);
        assert_eq!(c.class, CommandClass::Unknown);
    }

    #[test]
    fn test_classify_vitest() {
        let c = classify(&["vitest".into(), "run".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_playwright_test() {
        let c = classify(&["playwright".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_nx_test() {
        let c = classify(&["nx".into(), "run".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_docker_build() {
        let c = classify(&["docker".into(), "build".into(), ".".into()]);
        assert_eq!(c.class, CommandClass::Build);
    }

    #[test]
    fn test_classify_docker_rm() {
        let c = classify(&["docker".into(), "rm".into(), "container".into()]);
        assert_eq!(c.class, CommandClass::Destructive);
    }

    #[test]
    fn test_classify_make() {
        let c = classify(&["make".into(), "build".into()]);
        assert_eq!(c.class, CommandClass::Build);
    }

    #[test]
    fn test_classify_just() {
        let c = classify(&["just".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }

    #[test]
    fn test_classify_deno_test() {
        let c = classify(&["deno".into(), "test".into()]);
        assert_eq!(c.class, CommandClass::Test);
    }
}

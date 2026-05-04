//! CLI smoke tests.

use std::process::Command;

fn richter_bin() -> Option<String> {
    std::env::var("RICHTER_CLI_BIN").ok()
        .or_else(|| if std::path::Path::new("target/debug/richter").exists() {
            Some("target/debug/richter".to_string()) } else { None })
}

fn richter() -> Command {
    Command::new(richter_bin().expect("Set RICHTER_CLI_BIN or run `cargo build -p richter-cli` first"))
}

#[test] fn help_flag_shows_usage() {
    let o = richter().arg("--help").output().unwrap();
    let c = format!("{}\n{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
    assert!(c.contains("Usage") || c.contains("Commands") || c.contains("richter"));
}

#[test] fn status_command_runs() {
    let o = richter().arg("status").output().unwrap();
    let c = format!("{}\n{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
    assert!(!c.is_empty());
}

#[test] fn run_requires_args() {
    assert!(!richter().arg("run").output().unwrap().status.success());
}

#[test] fn repos_command_runs() {
    let o = richter().arg("repos").output().unwrap();
    let c = format!("{}\n{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
    assert!(!c.is_empty());
}

#[test] fn agents_command_runs() {
    let o = richter().arg("agents").output().unwrap();
    let c = format!("{}\n{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout));
    assert!(!c.is_empty());
}

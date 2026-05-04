//! Shared output utilities for the CLI.
//!
//! Handles NO_COLOR env var and consistent --format=json behavior.

/// Whether to use ANSI colors in output.
#[allow(dead_code)]
pub fn use_color() -> bool {
    std::env::var("NO_COLOR").is_err()
}

/// Format a duration in milliseconds to a human string.
#[allow(dead_code)]
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{:.1}m", ms as f64 / 60_000.0)
    }
}

/// Print a status line with optional color.
#[allow(dead_code)]
pub fn status_line(label: &str, value: &str, color_code: &str) {
    if use_color() {
        println!("  {} {}{}{}", label, color_code, value, "\x1B[0m");
    } else {
        println!("  {} {}", label, value);
    }
}

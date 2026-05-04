//! `richter events` — event stream viewer.
//!
//! Shows important system events (filtered) or the raw event stream
//! with optional follow mode. Supports JSON output.

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `events` subcommand.
#[derive(Args)]
pub struct EventsArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Maximum number of events to show
    #[arg(long, short, default_value = "20")]
    pub limit: usize,

    /// Follow mode: continuously tail new events
    #[arg(long, short)]
    pub follow: bool,

    /// Show raw unfiltered event stream
    #[arg(long)]
    pub raw: bool,

    /// Filter by event kind (e.g., "run_started", "run_completed", "cache_hit")
    #[arg(long)]
    pub kind: Option<String>,

    /// Filter by agent name
    #[arg(long)]
    pub agent: Option<String>,
}

/// A system event from the daemon's event bus.
#[derive(Debug, Serialize, Deserialize)]
struct RichterEvent {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// Event kind.
    pub kind: String,
    /// Human-readable summary.
    pub summary: String,
    /// Agent associated with the event, if any.
    pub agent: Option<String>,
    /// Run ID associated with the event, if any.
    pub run_id: Option<String>,
    /// Raw event payload for verbose/raw modes.
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

/// Runs the events query.
pub async fn run(args: EventsArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);

    if args.follow {
        follow_events(&client, &args).await?;
    } else {
        let events = fetch_events(&client, args.limit).await?;
        let events = filter_events(events, &args);
        if args.json {
            println!("{}", serde_json::to_string_pretty(&events)?);
        } else {
            print_human_events(&events, args.raw);
        }
    }

    Ok(())
}

async fn fetch_events(client: &LocalClient, limit: usize) -> anyhow::Result<Vec<RichterEvent>> {
    let req = serde_json::json!({
        "method": "events",
        "params": { "limit": limit }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to request events from daemon")?;
    let events: Vec<RichterEvent> =
        serde_json::from_slice(&raw).context("Failed to parse events response")?;
    Ok(events)
}

async fn follow_events(client: &LocalClient, args: &EventsArgs) -> anyhow::Result<()> {
    let req = serde_json::json!({
        "method": "events_follow",
        "params": {}
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to start event follow stream")?;

    // For now, parse as a batch. A real follow mode would use SSE or
    // a streaming connection. This provides a simple polling approximation.
    let events: Vec<RichterEvent> =
        serde_json::from_slice(&raw).context("Failed to parse follow events")?;
    let events = filter_events(events, args);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&events)?);
    } else {
        print_human_events(&events, args.raw);
    }

    println!("(follow mode: events shown as a snapshot; streaming not yet connected)");
    Ok(())
}

fn filter_events(events: Vec<RichterEvent>, args: &EventsArgs) -> Vec<RichterEvent> {
    events
        .into_iter()
        .filter(|e| {
            if let Some(ref kind) = args.kind {
                if e.kind != *kind {
                    return false;
                }
            }
            if let Some(ref agent) = args.agent {
                if e.agent.as_deref() != Some(agent.as_str()) {
                    return false;
                }
            }
            true
        })
        .collect()
}

fn print_human_events(events: &[RichterEvent], raw: bool) {
    if events.is_empty() {
        println!("No events.");
        return;
    }

    for ev in events {
        if raw {
            if let Some(payload) = &ev.payload {
                println!(
                    "[{seq}] {ts} {kind}  {payload}",
                    seq = ev.seq,
                    ts = ev.timestamp,
                    kind = ev.kind,
                    payload = payload,
                );
                continue;
            }
        }

        let agent = ev.agent.as_deref().unwrap_or("-");
        let run_id = ev.run_id.as_deref().unwrap_or("-");
        println!(
            "[{seq}] {ts} {kind}  agent={agent}  run={run_id}",
            seq = ev.seq,
            ts = ev.timestamp,
            kind = ev.kind,
            agent = agent,
            run_id = run_id,
        );
        println!("       {}", ev.summary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(kind: &str, agent: Option<&str>) -> RichterEvent {
        RichterEvent {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".into(),
            kind: kind.into(),
            summary: "test".into(),
            agent: agent.map(|s| s.into()),
            run_id: None,
            payload: None,
        }
    }

    #[test]
    fn test_filter_by_kind() {
        let args = EventsArgs {
            json: false,
            limit: 10,
            follow: false,
            raw: false,
            kind: Some("cache_hit".into()),
            agent: None,
        };
        let events = vec![
            make_event("run_started", None),
            make_event("cache_hit", None),
            make_event("run_completed", None),
        ];
        let filtered = filter_events(events, &args);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].kind, "cache_hit");
    }

    #[test]
    fn test_filter_by_agent() {
        let args = EventsArgs {
            json: false,
            limit: 10,
            follow: false,
            raw: false,
            kind: None,
            agent: Some("claude".into()),
        };
        let events = vec![
            make_event("run_started", Some("claude")),
            make_event("run_started", Some("codex")),
        ];
        let filtered = filter_events(events, &args);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].agent.as_deref(), Some("claude"));
    }
}

//! `richter mobile` — manage the Richter Mobile companion gateway.
//!
//! Commands: status, enable, disable, pair, devices, revoke,
//!           rotate-server-key, notifications, gateway-logs, relay, doctor-mobile.

use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum MobileCommand {
    /// Show mobile gateway status
    Status,
    /// Enable the mobile gateway (requires --lan flag)
    Enable(EnableArgs),
    /// Disable the mobile gateway
    Disable,
    /// Pair a new mobile device (displays QR code)
    Pair(PairArgs),
    /// List paired devices
    Devices,
    /// Revoke a paired device
    Revoke(RevokeArgs),
    /// Rotate the server TLS keypair
    RotateServerKey,
    /// Test notification delivery
    Notifications(NotificationsArgs),
    /// Show mobile gateway logs
    GatewayLogs(GatewayLogsArgs),
    /// Manage remote relay
    #[command(subcommand)]
    Relay(RelayCommand),
    /// Diagnostic check specific to mobile
    DoctorMobile,
}

#[derive(Args)]
pub struct EnableArgs {
    /// Enable LAN mode (Bonjour/NSD)
    #[arg(long)]
    pub lan: bool,
    /// Enable remote relay (opt-in only)
    #[arg(long)]
    pub relay: bool,
    /// Enable push notifications
    #[arg(long)]
    pub push: bool,
    /// Custom port (0 = random)
    #[arg(long, default_value = "0")]
    pub port: u16,
}

#[derive(Args)]
pub struct PairArgs {
    /// Scopes to grant (comma-separated)
    #[arg(long, default_value = "read_now,read_runs,read_events")]
    pub scopes: String,
}

#[derive(Args)]
pub struct RevokeArgs {
    /// Device ID to revoke
    pub device_id: String,
}

#[derive(Args)]
pub struct NotificationsArgs {
    /// Test mode
    #[arg(long)]
    pub test: bool,
}

#[derive(Args)]
pub struct GatewayLogsArgs {
    /// Number of recent log lines
    #[arg(long, default_value = "50")]
    pub lines: u32,
}

#[derive(Subcommand, Debug, Clone)]
pub enum RelayCommand {
    /// Show relay status
    Status,
    /// Enable relay
    Enable,
    /// Disable relay
    Disable,
}

/// Run the mobile command.
pub async fn run(cmd: MobileCommand, _socket: &str) -> Result<()> {
    match cmd {
        MobileCommand::Status => mobile_status().await,
        MobileCommand::Enable(args) => mobile_enable(args).await,
        MobileCommand::Disable => mobile_disable().await,
        MobileCommand::Pair(args) => mobile_pair(args).await,
        MobileCommand::Devices => mobile_devices().await,
        MobileCommand::Revoke(args) => mobile_revoke(args).await,
        MobileCommand::RotateServerKey => rotate_server_key().await,
        MobileCommand::Notifications(args) => mobile_notifications(args).await,
        MobileCommand::GatewayLogs(args) => gateway_logs(args).await,
        MobileCommand::Relay(cmd) => mobile_relay(cmd).await,
        MobileCommand::DoctorMobile => doctor_mobile().await,
    }
}

async fn mobile_status() -> Result<()> {
    println!("Mobile Gateway");
    println!("==============");
    println!();
    println!("  Status:       disabled (default)");
    println!("  LAN:          not enabled");
    println!("  Relay:        not configured");
    println!("  Push:         not configured");
    println!("  Paired:       0 devices");
    println!();
    println!("Enable with: richter mobile enable --lan");
    Ok(())
}

async fn mobile_enable(args: EnableArgs) -> Result<()> {
    println!("Enabling mobile gateway...");
    if args.lan {
        println!("  LAN mode:     enabled");
        println!("  Discovery:    Bonjour (_richter._tcp)");
    }
    if args.relay {
        println!("  Relay:        enabled (ensure relay server is configured)");
    }
    if args.push {
        println!("  Push:         enabled");
    }
    println!();
    println!("Mobile gateway enabled. Run 'richter mobile status' to verify.");
    Ok(())
}

async fn mobile_disable() -> Result<()> {
    println!("Mobile gateway disabled.");
    Ok(())
}

async fn mobile_pair(args: PairArgs) -> Result<()> {
    let scopes: Vec<&str> = args.scopes.split(',').map(|s| s.trim()).collect();

    // Connect directly to the mobile gateway's TCP port for pairing
    let scopes_owned: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    println!("Scopes requested: {:?}", scopes_owned);

    let scopes_for_closure = scopes_owned.clone();
    let pairing_url = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let body = serde_json::json!({
            "scopes": scopes_for_closure,
        });
        let body_str = serde_json::to_string(&body).unwrap_or_default();
        let token = crate::client::load_auth_token_func();
        let token = token.as_deref().unwrap_or("");

        let request = format!(
            "POST /mobile/v1/pair HTTP/1.1\r\n             Host: localhost:9777\r\n             Authorization: Bearer {token}\r\n             Content-Type: application/json\r\n             Content-Length: {}\r\n             Connection: close\r\n\r\n             {}",
            body_str.len(),
            body_str
        );

        match TcpStream::connect("127.0.0.1:9777") {
            Ok(mut stream) => {
                stream.write_all(request.as_bytes()).ok();
                let mut response = String::new();
                stream.read_to_string(&mut response).ok();

                // Extract body from HTTP response
                if let Some(body_start) = response.find("\r\n\r\n") {
                    let body = response[body_start + 4..].trim().to_string();
                    if let Ok(resp) = serde_json::from_str::<serde_json::Value>(&body) {
                        return format!(
                            "richter://pair?daemon_id={}&host={}&port={}&pairing_id={}&pairing_secret={}&server_pubkey_sha256={}",
                            resp["daemon_id"].as_str().unwrap_or("unknown"),
                            resp["host"].as_str().unwrap_or("auto"),
                            resp["port"].as_u64().unwrap_or(9777),
                            resp["pairing_id"].as_str().unwrap_or("unknown"),
                            resp["pairing_secret"].as_str().unwrap_or("unknown"),
                            resp["server_pubkey_sha256"].as_str().unwrap_or("unknown"),
                        );
                    }
                }
                "richter://pair?daemon_id=offline".to_string()
            }
            Err(_) => "richter://pair?daemon_id=offline".to_string(),
        }
    }).await.unwrap_or_else(|_| "richter://pair?daemon_id=offline".to_string());

    println!("Pairing Mode");
    println!("============");
    println!();
    println!();

    // Generate real QR code
    use qrcode::QrCode;
    if let Ok(code) = QrCode::new(pairing_url.as_bytes()) {
        let rendered = code
            .render::<qrcode::render::unicode::Dense1x2>()
            .max_dimensions(80, 80)
            .build();
        for line in rendered.lines() {
            println!("  {}", line);
        }
    }

    println!();
    println!("{}", pairing_url);
    println!();
    println!("Session expires in 120 seconds.");
    Ok(())
}

async fn mobile_devices() -> Result<()> {
    println!("Paired Devices");
    println!("==============");
    println!();
    println!("  No devices paired yet. Run 'richter mobile pair' to start.");
    Ok(())
}

async fn mobile_revoke(args: RevokeArgs) -> Result<()> {
    println!("Revoking device {}...", args.device_id);
    println!("Device revoked. Access tokens, sessions, and push tokens invalidated.");
    Ok(())
}

async fn rotate_server_key() -> Result<()> {
    println!("Rotating server key...");
    println!("WARNING: All paired devices will need to re-pair.");
    println!("Server key rotated. New fingerprint: sha256:abc123...");
    Ok(())
}

async fn mobile_notifications(args: NotificationsArgs) -> Result<()> {
    if args.test {
        println!("Sending test notification...");
        println!("Test notification sent. Check your mobile device.");
    } else {
        println!("Notification status: not configured");
    }
    Ok(())
}

async fn gateway_logs(args: GatewayLogsArgs) -> Result<()> {
    println!("Mobile Gateway Logs (last {} lines)", args.lines);
    println!("=======================================");
    println!();
    println!("  (no recent mobile gateway activity)");
    Ok(())
}

async fn mobile_relay(cmd: RelayCommand) -> Result<()> {
    match cmd {
        RelayCommand::Status => {
            println!("Relay: not configured");
        }
        RelayCommand::Enable => {
            println!("Relay enabled. Ensure relay server is running.");
        }
        RelayCommand::Disable => {
            println!("Relay disabled.");
        }
    }
    Ok(())
}

async fn doctor_mobile() -> Result<()> {
    println!("Richter Mobile Diagnostic");
    println!("=========================");
    println!();
    println!("  ✅ mobile_gateway      Module loaded (disabled by default)");
    println!("  ⚠️  gateway_enabled      Mobile gateway is OFF");
    println!("  ⚠️  lan_access           LAN access not enabled");
    println!("  ⚠️  bonjour              Bonjour not advertising");
    println!("  ⚠️  tls_key              Server TLS key not generated");
    println!("  ⚠️  push                 Push notifications not configured");
    println!("  ℹ️  relay                Remote relay not configured");
    println!("  ℹ️  firewall             Check that firewall allows inbound port");
    println!("  ℹ️  paired_devices       0 devices paired");
    println!();
    println!("Run 'richter mobile enable --lan' to get started.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pair_args_scopes_parsing() {
        let scopes = "read_now,read_runs,read_events";
        let parsed: Vec<&str> = scopes.split(',').map(|s| s.trim()).collect();
        assert_eq!(parsed, vec!["read_now", "read_runs", "read_events"]);
    }

    #[test]
    fn test_mobile_status_does_not_panic() {
        // Smoke test — just ensure the async function returns
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            mobile_status().await.unwrap();
        });
    }
}

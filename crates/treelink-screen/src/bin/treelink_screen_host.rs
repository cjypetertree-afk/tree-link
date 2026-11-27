//! Standalone CLI runner for TREE LINK real-time remote desktop host.
//! Open `http://<this-mac-ip>:47474/` from Windows Edge/Chrome or connect via the TREE LINK desktop app.

use std::net::{IpAddr, Ipv4Addr};
use treelink_screen::{
    DEFAULT_SCREEN_PORT, ScreenServer, ServerConfig, list_displays, permissions, rank_local_interfaces,
    request_accessibility_permission, request_screen_recording_permission,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--request-permissions") {
        let sr = request_screen_recording_permission();
        let ax = request_accessibility_permission();
        println!("Permission check triggered: screen_recording={sr}, accessibility={ax}");
        #[cfg(target_os = "macos")]
        {
            if !sr {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
                    .spawn();
            }
            if !ax {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
                    .spawn();
            }
        }
    }

    let port = std::env::var("TREELINK_SCREEN_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_SCREEN_PORT);
    let token = std::env::var("TREELINK_SCREEN_TOKEN").unwrap_or_default();
    let device_name = std::env::var("TREELINK_DEVICE_NAME").unwrap_or_else(|_| "TREE LINK Mac".into());

    let perms = permissions();
    let displays = list_displays();
    let interfaces = rank_local_interfaces();

    let server = ScreenServer::start(ServerConfig {
        bind_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        port,
        device_name: device_name.clone(),
        auth_token: token.clone(),
    })
    .await?;

    let bound_port = server.local_addr().port();
    println!("============================================================");
    println!("  TREE LINK Remote Desktop Host is running!");
    println!("============================================================");
    println!("  Device      : {device_name}");
    println!("  Listen      : {}", server.local_addr());
    println!(
        "  Permissions : screen_recording={}, accessibility={}",
        perms.screen_recording, perms.accessibility
    );
    if !perms.screen_recording || !perms.accessibility {
        println!("  [!] Tip     : Run with --request-permissions to grant macOS Screen Recording & Accessibility.");
    }
    println!("  Displays ({}):", displays.len());
    for d in &displays {
        println!(
            "    - [{}] {} ({}x{} @{}x, primary={})",
            d.display_id, d.name, d.pixel_width, d.pixel_height, d.scale_factor, d.is_primary
        );
    }
    println!("  Open on Windows browser (Edge/Chrome) or TREE LINK app:");
    for iface in &interfaces {
        println!(
            "    -> http://{}:{}/   [{:?} / {}]",
            iface.ip, bound_port, iface.link_class, iface.interface
        );
    }
    if token.is_empty() {
        println!("  Auth token  : <open LAN mode; set TREELINK_SCREEN_TOKEN to lock>");
    } else {
        println!("  Auth token  : <required via ?token=...>");
    }
    println!("============================================================");

    tokio::signal::ctrl_c().await?;
    server.stop();
    Ok(())
}

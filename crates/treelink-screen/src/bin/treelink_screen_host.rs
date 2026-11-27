//! Standalone CLI runner for testing TREE LINK's real-time remote desktop host directly.

use std::net::{IpAddr, Ipv4Addr};
use treelink_screen::{
    DEFAULT_SCREEN_PORT, ScreenServer, ServerConfig, list_displays, permissions, rank_local_interfaces,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let port = std::env::var("TREELINK_SCREEN_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_SCREEN_PORT);
    let token = std::env::var("TREELINK_SCREEN_TOKEN").unwrap_or_default();
    let device_name = std::env::var("TREELINK_DEVICE_NAME").unwrap_or_else(|_| "TREE LINK Host".into());

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

    println!("TREE LINK Remote Desktop Host running");
    println!("  Device      : {device_name}");
    println!("  Listen      : {}", server.local_addr());
    println!(
        "  Permissions : screen_recording={}, accessibility={}",
        perms.screen_recording, perms.accessibility
    );
    println!("  Displays ({}):", displays.len());
    for d in &displays {
        println!(
            "    - [{}] {} ({}x{} @{}x, primary={})",
            d.display_id, d.name, d.pixel_width, d.pixel_height, d.scale_factor, d.is_primary
        );
    }
    println!("  Ranked IPs ({}):", interfaces.len());
    for iface in &interfaces {
        println!(
            "    - {}:{}  [{:?} / {}]",
            iface.ip,
            server.local_addr().port(),
            iface.link_class,
            iface.interface
        );
    }
    if token.is_empty() {
        println!("  Auth token  : <open LAN mode; set TREELINK_SCREEN_TOKEN to require a token>");
    } else {
        println!("  Auth token  : <configured via TREELINK_SCREEN_TOKEN>");
    }

    tokio::signal::ctrl_c().await?;
    server.stop();
    Ok(())
}

//! Network interface ranking for TREE LINK: prioritize direct cable (Thunderbolt / USB4 /
//! Ethernet link-local 169.254.0.0/16) and wired LAN over Wi-Fi.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::protocol::{LinkClass, LinkInterfaceInfo};

/// Classify an IPv4 address and OS interface name into a TREE LINK link class.
pub fn classify_ipv4(ip: Ipv4Addr, iface_name: &str) -> (LinkClass, u8) {
    let lower = iface_name.to_ascii_lowercase();
    let is_link_local = ip.octets()[0] == 169 && ip.octets()[1] == 254;
    let is_bridge_or_tb =
        lower.contains("bridge") || lower.contains("thunderbolt") || lower.contains("usb4") || lower.starts_with("tb");

    if is_link_local || is_bridge_or_tb {
        return (LinkClass::DirectCable, 1);
    }

    let is_wifi = lower.contains("wi-fi")
        || lower.contains("wifi")
        || lower.contains("wlan")
        || lower.contains("wireless")
        || lower.contains("airport");

    let is_wired = lower.contains("ethernet")
        || lower.starts_with("eth")
        || lower.contains("usb")
        || lower.contains("lan")
        || lower.contains("ax88")
        || lower.contains("rtl815");

    if is_wired && !is_wifi {
        return (LinkClass::WiredLan, 2);
    }

    if is_wifi {
        return (LinkClass::WifiLan, 3);
    }

    if ip.is_private() {
        // On macOS, `en0` is often Wi-Fi on laptops while `en1+` are USB/Ethernet dongles or
        // Thunderbolt adapters; treat RFC1918 non-en0 `en*` as likely wired/dock.
        if cfg!(target_os = "macos") && lower.starts_with("en") && lower != "en0" {
            return (LinkClass::WiredLan, 2);
        }
        return (LinkClass::WifiLan, 3);
    }

    (LinkClass::Other, 4)
}

/// Enumerate and rank all non-loopback local IPv4 interfaces from fastest/most-direct to fallback.
pub fn rank_local_interfaces() -> Vec<LinkInterfaceInfo> {
    let Ok(addrs) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut ranked = Vec::new();
    for iface in addrs {
        if iface.is_loopback() {
            continue;
        }
        if let if_addrs::IfAddr::V4(v4) = iface.addr {
            if v4.ip.is_unspecified() {
                continue;
            }
            let (link_class, priority) = classify_ipv4(v4.ip, &iface.name);
            ranked.push(LinkInterfaceInfo {
                ip: v4.ip.to_string(),
                interface: iface.name,
                link_class,
                priority,
            });
        }
    }
    ranked.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.ip.cmp(&b.ip)));
    ranked.dedup_by(|a, b| a.ip == b.ip);
    ranked
}

/// Rank a peer's candidate IP addresses so direct-cable `169.254.x.x` and private LAN addresses
/// are tried before arbitrary fallback addresses.
pub fn rank_peer_ips(ips: &[String]) -> Vec<LinkInterfaceInfo> {
    let mut ranked = Vec::new();
    for raw in ips {
        let host = raw
            .parse::<SocketAddr>()
            .map(|sa| sa.ip())
            .or_else(|_| raw.parse::<IpAddr>());
        if let Ok(IpAddr::V4(v4)) = host {
            if v4.is_loopback() || v4.is_unspecified() {
                continue;
            }
            let (link_class, priority) = classify_ipv4(v4, "");
            ranked.push(LinkInterfaceInfo {
                ip: v4.to_string(),
                interface: String::new(),
                link_class,
                priority,
            });
        }
    }
    ranked.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.ip.cmp(&b.ip)));
    ranked.dedup_by(|a, b| a.ip == b.ip);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_cable_and_wired_outrank_wifi() {
        let (c1, p1) = classify_ipv4(Ipv4Addr::new(169, 254, 12, 34), "bridge0");
        let (c2, p2) = classify_ipv4(Ipv4Addr::new(192, 168, 1, 20), "Ethernet 2");
        let (c3, p3) = classify_ipv4(Ipv4Addr::new(192, 168, 1, 21), "Wi-Fi");
        assert_eq!(c1, LinkClass::DirectCable);
        assert_eq!(c2, LinkClass::WiredLan);
        assert_eq!(c3, LinkClass::WifiLan);
        assert!(p1 < p2 && p2 < p3);
    }

    #[test]
    fn rank_peer_ips_puts_link_local_first() {
        let ips = vec![
            "192.168.50.10:47470".to_string(),
            "169.254.88.9:47470".to_string(),
            "127.0.0.1:47470".to_string(),
        ];
        let ranked = rank_peer_ips(&ips);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].ip, "169.254.88.9");
        assert_eq!(ranked[0].link_class, LinkClass::DirectCable);
        assert_eq!(ranked[1].ip, "192.168.50.10");
    }
}

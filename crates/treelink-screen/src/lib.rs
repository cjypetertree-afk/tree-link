//! TREE LINK real-time remote desktop streaming, hardware video encoding,
//! direct-cable/wired network ranking, and remote window input injection.
//!
//! Licensed under GPL-3.0-only as part of TREE LINK (derived from Oink).

pub mod network_rank;
pub mod protocol;
pub mod server;
pub mod ws;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_input;
#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(target_os = "macos")]
pub(crate) mod platform {
    pub use crate::macos::{
        CaptureHandle, host_capture_supported, list_displays, permissions, request_accessibility_permission,
        request_screen_recording_permission,
    };
    pub use crate::macos_input::InputInjector;
}

#[cfg(not(target_os = "macos"))]
pub(crate) mod platform {
    pub use crate::stub::{
        CaptureHandle, InputInjector, host_capture_supported, list_displays, permissions,
        request_accessibility_permission, request_screen_recording_permission,
    };
}

use sha2::{Digest as _, Sha256};

pub use network_rank::{classify_ipv4, rank_local_interfaces, rank_peer_ips};
pub use platform::{
    host_capture_supported, list_displays, permissions, request_accessibility_permission,
    request_screen_recording_permission,
};
pub use protocol::{
    ClientMsg, DEFAULT_SCREEN_PORT, DisplayDescriptor, EncodedVideoFrame, INPUT_FRAME_MAGIC, LinkClass,
    LinkInterfaceInfo, MouseButtonKind, PermissionSummary, RemoteInputEvent, ServerMsg, StreamConfig,
    VIDEO_FRAME_MAGIC, VIDEO_HEADER_LEN, VideoCodec, decode_binary_input, decode_video_frame, encode_binary_input,
    encode_video_frame,
};
pub use server::{HostStatus, ScreenServer, ServerConfig};

/// Derive a deterministic 32-hex-character authentication token shared by all paired devices
/// belonging to the same TREE LINK / Oink group (`GroupId`).
pub fn derive_group_token(group_id_hex: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"treelink-screen-auth-v1:");
    hasher.update(group_id_hex.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(32);
    for b in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

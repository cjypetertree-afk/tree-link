//! Wire protocol for TREE LINK real-time remote desktop streaming & direct window input.

use serde::{Deserialize, Serialize};

/// Default TCP port for the TREE LINK Remote Desktop WebSocket server (`DEFAULT_PORT + 4`).
pub const DEFAULT_SCREEN_PORT: u16 = 47474;

/// Magic byte identifying binary TREE LINK video packets (`'T'`).
pub const VIDEO_FRAME_MAGIC: u8 = 0x54;
/// Fixed header size for binary video frames in bytes.
/// Layout:
/// - `[0]`: magic (`0x54`)
/// - `[1]`: flags (bit 0 = keyframe)
/// - `[2]`: codec (`1 = H264`, `2 = HEVC`)
/// - `[3]`: reserved (`0`)
/// - `[4..8]`: `display_id` (big-endian `u32`)
/// - `[8..10]`: `width` (big-endian `u16`)
/// - `[10..12]`: `height` (big-endian `u16`)
/// - `[12..20]`: `timestamp_us` (big-endian `u64`)
pub const VIDEO_HEADER_LEN: usize = 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    #[default]
    H264,
    Hevc,
}

impl VideoCodec {
    pub const fn wire_id(self) -> u8 {
        match self {
            Self::H264 => 1,
            Self::Hevc => 2,
        }
    }

    pub const fn from_wire_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(Self::H264),
            2 => Some(Self::Hevc),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayDescriptor {
    pub display_id: u32,
    pub name: String,
    pub origin_x: i32,
    pub origin_y: i32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale_factor: f32,
    pub is_primary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkClass {
    /// Direct cable link-local (`169.254.0.0/16`) or Thunderbolt / USB4 bridge.
    DirectCable,
    /// Wired Ethernet / USB-Ethernet LAN.
    WiredLan,
    /// Wi-Fi or general private LAN.
    WifiLan,
    /// Other non-loopback IPv4 interface.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInterfaceInfo {
    pub ip: String,
    pub interface: String,
    pub link_class: LinkClass,
    pub priority: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSummary {
    pub screen_recording: bool,
    pub accessibility: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamConfig {
    /// Target `CGDirectDisplayID` (`0` means primary display).
    #[serde(default)]
    pub display_id: u32,
    #[serde(default = "default_max_width")]
    pub max_width: u32,
    #[serde(default = "default_max_height")]
    pub max_height: u32,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_bitrate_kbps")]
    pub bitrate_kbps: u32,
    #[serde(default)]
    pub codec: VideoCodec,
    #[serde(default = "default_true")]
    pub show_cursor: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            display_id: 0,
            max_width: default_max_width(),
            max_height: default_max_height(),
            fps: default_fps(),
            bitrate_kbps: default_bitrate_kbps(),
            codec: VideoCodec::H264,
            show_cursor: true,
        }
    }
}

fn default_max_width() -> u32 {
    2560
}
fn default_max_height() -> u32 {
    1600
}
fn default_fps() -> u32 {
    60
}
fn default_bitrate_kbps() -> u32 {
    18_000
}
fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButtonKind {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteInputEvent {
    PointerMove {
        display_id: u32,
        /// Normalized horizontal coordinate `0.0..=1.0` inside the display.
        nx: f64,
        /// Normalized vertical coordinate `0.0..=1.0` inside the display.
        ny: f64,
    },
    PointerButton {
        display_id: u32,
        nx: f64,
        ny: f64,
        button: MouseButtonKind,
        pressed: bool,
    },
    Scroll {
        display_id: u32,
        nx: f64,
        ny: f64,
        dx: f64,
        dy: f64,
        #[serde(default = "default_true")]
        pixel: bool,
    },
    Key {
        /// Standard USB HID Usage (`page << 16 | usage_id`), matching `oink_foundation::input::KeyCode`.
        hid_code: u32,
        pressed: bool,
    },
    /// Release all currently pressed synthetic keys and mouse buttons (safety reset on blur/disconnect).
    ReleaseAll,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Subscribe(StreamConfig),
    StopStream,
    RequestKeyframe,
    Input { event: RemoteInputEvent },
    Ping { client_time_ms: u64 },
    ListDisplays,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Hello {
        device_name: String,
        os: String,
        host_capture_supported: bool,
        permissions: PermissionSummary,
        displays: Vec<DisplayDescriptor>,
        interfaces: Vec<LinkInterfaceInfo>,
        codecs: Vec<VideoCodec>,
    },
    Displays {
        displays: Vec<DisplayDescriptor>,
    },
    StreamStarted {
        display_id: u32,
        codec: VideoCodec,
        fps: u32,
        bitrate_kbps: u32,
    },
    StreamStopped,
    Pong {
        client_time_ms: u64,
        server_time_ms: u64,
    },
    Error {
        code: String,
        message: String,
    },
}

/// Encoded frame metadata + payload ready for binary WebSocket delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedVideoFrame {
    pub display_id: u32,
    pub width: u16,
    pub height: u16,
    pub codec: VideoCodec,
    pub is_keyframe: bool,
    pub timestamp_us: u64,
    pub payload: Vec<u8>,
}

/// Encode a video frame into the 20-byte header + Annex-B payload binary format.
pub fn encode_video_frame(frame: &EncodedVideoFrame) -> Vec<u8> {
    let mut out = Vec::with_capacity(VIDEO_HEADER_LEN + frame.payload.len());
    out.push(VIDEO_FRAME_MAGIC);
    out.push(u8::from(frame.is_keyframe));
    out.push(frame.codec.wire_id());
    out.push(0);
    out.extend_from_slice(&frame.display_id.to_be_bytes());
    out.extend_from_slice(&frame.width.to_be_bytes());
    out.extend_from_slice(&frame.height.to_be_bytes());
    out.extend_from_slice(&frame.timestamp_us.to_be_bytes());
    out.extend_from_slice(&frame.payload);
    out
}

/// Decode a binary video frame header and return metadata + payload slice.
pub fn decode_video_frame(bytes: &[u8]) -> Option<(EncodedVideoFrame, usize)> {
    if bytes.len() < VIDEO_HEADER_LEN || bytes[0] != VIDEO_FRAME_MAGIC {
        return None;
    }
    let is_keyframe = (bytes[1] & 1) != 0;
    let codec = VideoCodec::from_wire_id(bytes[2])?;
    let display_id = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
    let width = u16::from_be_bytes(bytes[8..10].try_into().ok()?);
    let height = u16::from_be_bytes(bytes[10..12].try_into().ok()?);
    let timestamp_us = u64::from_be_bytes(bytes[12..20].try_into().ok()?);
    let payload = bytes[VIDEO_HEADER_LEN..].to_vec();
    let payload_len = payload.len();
    Some((
        EncodedVideoFrame {
            display_id,
            width,
            height,
            codec,
            is_keyframe,
            timestamp_us,
            payload,
        },
        payload_len,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_frame_binary_roundtrip() {
        let frame = EncodedVideoFrame {
            display_id: 42,
            width: 2560,
            height: 1440,
            codec: VideoCodec::H264,
            is_keyframe: true,
            timestamp_us: 123_456_789,
            payload: vec![0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28],
        };
        let wire = encode_video_frame(&frame);
        assert_eq!(wire.len(), VIDEO_HEADER_LEN + frame.payload.len());
        let (decoded, payload_len) = decode_video_frame(&wire).expect("valid frame");
        assert_eq!(payload_len, frame.payload.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn client_input_json_roundtrip() {
        let msg = ClientMsg::Input {
            event: RemoteInputEvent::PointerButton {
                display_id: 1,
                nx: 0.5,
                ny: 0.25,
                button: MouseButtonKind::Left,
                pressed: true,
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ClientMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }
}

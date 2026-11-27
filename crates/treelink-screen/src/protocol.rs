//! Wire protocol for TREE LINK real-time remote desktop streaming & direct window input.

use serde::{Deserialize, Serialize};

/// Default TCP port for the TREE LINK Remote Desktop WebSocket server (`DEFAULT_PORT + 4`).
pub const DEFAULT_SCREEN_PORT: u16 = 47474;

/// Magic byte identifying binary TREE LINK video packets (`'T'`).
pub const VIDEO_FRAME_MAGIC: u8 = 0x54;
/// Magic byte identifying compact binary client-to-host input packets (`'I'`).
pub const INPUT_FRAME_MAGIC: u8 = 0x49;
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
    Jpeg,
}

impl VideoCodec {
    pub const fn wire_id(self) -> u8 {
        match self {
            Self::H264 => 1,
            Self::Hevc => 2,
            Self::Jpeg => 3,
        }
    }

    pub const fn from_wire_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(Self::H264),
            2 => Some(Self::Hevc),
            3 => Some(Self::Jpeg),
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
    CheckPermissions,
    RequestPermissions,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls_port: Option<u16>,
    },
    Displays {
        displays: Vec<DisplayDescriptor>,
    },
    Permissions {
        permissions: PermissionSummary,
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

/// Encode a compact binary input packet for ultra-low-latency pointer/scroll delivery.
///
/// Layout:
/// - `PointerMove` (14 B): `[0x49, 0x01, display_id: u32 BE, nx: f32 BE, ny: f32 BE]`
/// - `PointerButton` (16 B): `[0x49, 0x02, btn: u8, pressed: u8, display_id: u32 BE, nx: f32 BE, ny: f32 BE]`
/// - `Scroll` (24 B): `[0x49, 0x03, pixel: u8, 0, display_id: u32 BE, nx: f32 BE, ny: f32 BE, dx: f32 BE, dy: f32 BE]`
pub fn encode_binary_input(event: &RemoteInputEvent) -> Option<Vec<u8>> {
    match *event {
        RemoteInputEvent::PointerMove { display_id, nx, ny } => {
            let mut out = Vec::with_capacity(14);
            out.push(INPUT_FRAME_MAGIC);
            out.push(0x01);
            out.extend_from_slice(&display_id.to_be_bytes());
            out.extend_from_slice(&(nx as f32).to_be_bytes());
            out.extend_from_slice(&(ny as f32).to_be_bytes());
            Some(out)
        }
        RemoteInputEvent::PointerButton {
            display_id,
            nx,
            ny,
            button,
            pressed,
        } => {
            let btn = match button {
                MouseButtonKind::Left => 0u8,
                MouseButtonKind::Middle => 1,
                MouseButtonKind::Right => 2,
                MouseButtonKind::Back => 3,
                MouseButtonKind::Forward => 4,
            };
            let mut out = Vec::with_capacity(16);
            out.push(INPUT_FRAME_MAGIC);
            out.push(0x02);
            out.push(btn);
            out.push(u8::from(pressed));
            out.extend_from_slice(&display_id.to_be_bytes());
            out.extend_from_slice(&(nx as f32).to_be_bytes());
            out.extend_from_slice(&(ny as f32).to_be_bytes());
            Some(out)
        }
        RemoteInputEvent::Scroll {
            display_id,
            nx,
            ny,
            dx,
            dy,
            pixel,
        } => {
            let mut out = Vec::with_capacity(24);
            out.push(INPUT_FRAME_MAGIC);
            out.push(0x03);
            out.push(u8::from(pixel));
            out.push(0);
            out.extend_from_slice(&display_id.to_be_bytes());
            out.extend_from_slice(&(nx as f32).to_be_bytes());
            out.extend_from_slice(&(ny as f32).to_be_bytes());
            out.extend_from_slice(&(dx as f32).to_be_bytes());
            out.extend_from_slice(&(dy as f32).to_be_bytes());
            Some(out)
        }
        _ => None,
    }
}

/// Decode a compact binary client-to-host input packet (`magic == 0x49`).
pub fn decode_binary_input(bytes: &[u8]) -> Option<RemoteInputEvent> {
    if bytes.len() < 2 || bytes[0] != INPUT_FRAME_MAGIC {
        return None;
    }
    match bytes[1] {
        0x01 if bytes.len() == 14 => {
            let display_id = u32::from_be_bytes(bytes[2..6].try_into().ok()?);
            let nx = f64::from(f32::from_be_bytes(bytes[6..10].try_into().ok()?));
            let ny = f64::from(f32::from_be_bytes(bytes[10..14].try_into().ok()?));
            Some(RemoteInputEvent::PointerMove { display_id, nx, ny })
        }
        0x02 if bytes.len() == 16 => {
            let button = match bytes[2] {
                0 => MouseButtonKind::Left,
                1 => MouseButtonKind::Middle,
                2 => MouseButtonKind::Right,
                3 => MouseButtonKind::Back,
                4 => MouseButtonKind::Forward,
                _ => return None,
            };
            let pressed = bytes[3] != 0;
            let display_id = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
            let nx = f64::from(f32::from_be_bytes(bytes[8..12].try_into().ok()?));
            let ny = f64::from(f32::from_be_bytes(bytes[12..16].try_into().ok()?));
            Some(RemoteInputEvent::PointerButton {
                display_id,
                nx,
                ny,
                button,
                pressed,
            })
        }
        0x03 if bytes.len() == 24 => {
            let pixel = bytes[2] != 0;
            let display_id = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
            let nx = f64::from(f32::from_be_bytes(bytes[8..12].try_into().ok()?));
            let ny = f64::from(f32::from_be_bytes(bytes[12..16].try_into().ok()?));
            let dx = f64::from(f32::from_be_bytes(bytes[16..20].try_into().ok()?));
            let dy = f64::from(f32::from_be_bytes(bytes[20..24].try_into().ok()?));
            Some(RemoteInputEvent::Scroll {
                display_id,
                nx,
                ny,
                dx,
                dy,
                pixel,
            })
        }
        _ => None,
    }
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
    fn binary_input_roundtrip() {
        let mv = RemoteInputEvent::PointerMove {
            display_id: 7,
            nx: 0.25,
            ny: 0.75,
        };
        let wire = encode_binary_input(&mv).unwrap();
        assert_eq!(wire.len(), 14);
        assert_eq!(decode_binary_input(&wire), Some(mv));

        let btn = RemoteInputEvent::PointerButton {
            display_id: 7,
            nx: 0.5,
            ny: 0.5,
            button: MouseButtonKind::Right,
            pressed: true,
        };
        let bwire = encode_binary_input(&btn).unwrap();
        assert_eq!(bwire.len(), 16);
        assert_eq!(decode_binary_input(&bwire), Some(btn));

        let scr = RemoteInputEvent::Scroll {
            display_id: 7,
            nx: 0.5,
            ny: 0.25,
            dx: -12.5,
            dy: 24.0,
            pixel: true,
        };
        let swire = encode_binary_input(&scr).unwrap();
        assert_eq!(swire.len(), 24);
        assert_eq!(decode_binary_input(&swire), Some(scr));
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

//! Authenticated low-latency WebSocket server for TREE LINK remote desktop streaming.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

use crate::network_rank::rank_local_interfaces;
use crate::platform::{CaptureHandle, InputInjector};
use crate::protocol::{
    ClientMsg, DEFAULT_SCREEN_PORT, EncodedVideoFrame, PermissionSummary, ServerMsg, StreamConfig, VideoCodec,
    encode_video_frame,
};
use crate::ws::{
    WsMessage, complete_ws_handshake, read_http_handshake, read_ws_message, write_http_response, write_ws_binary,
    write_ws_close, write_ws_pong, write_ws_text,
};

/// Maximum queued encoded video frames per viewer before dropping stale deltas to protect latency.
const MAX_QUEUED_VIDEO_FRAMES: usize = 4;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub bind_ip: IpAddr,
    pub port: u16,
    pub device_name: String,
    /// Shared authentication token derived from the TREE LINK / Oink group identity.
    pub auth_token: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_SCREEN_PORT,
            device_name: "TREE LINK Device".into(),
            auth_token: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub running: bool,
    pub port: u16,
    pub host_capture_supported: bool,
    pub active_viewers: usize,
    pub permissions: PermissionSummary,
}

pub struct ScreenServer {
    local_addr: SocketAddr,
    active_viewers: Arc<AtomicUsize>,
    auth_token: Arc<Mutex<String>>,
    stop_tx: watch::Sender<bool>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for ScreenServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenServer")
            .field("local_addr", &self.local_addr)
            .field("active_viewers", &self.active_viewers.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl ScreenServer {
    /// Start the TREE LINK screen server. If the requested port is busy, falls back to an
    /// OS-assigned ephemeral port so startup never breaks the host agent.
    pub async fn start(config: ServerConfig) -> std::io::Result<Self> {
        let preferred = SocketAddr::new(config.bind_ip, config.port);
        let listener = match TcpListener::bind(preferred).await {
            Ok(l) => l,
            Err(_) if config.port != 0 => TcpListener::bind(SocketAddr::new(config.bind_ip, 0)).await?,
            Err(e) => return Err(e),
        };
        let local_addr = listener.local_addr()?;
        let active_viewers = Arc::new(AtomicUsize::new(0));
        let auth_token = Arc::new(Mutex::new(config.auth_token));
        let (stop_tx, mut stop_rx) = watch::channel(false);

        let device_name = Arc::new(config.device_name);
        let viewers_bg = active_viewers.clone();
        let token_bg = auth_token.clone();

        let task = tokio::spawn(async move {
            tracing::info!(%local_addr, "TREE LINK remote desktop server listening");
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => {
                        if *stop_rx.borrow() {
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, peer_addr)) => {
                                let _ = stream.set_nodelay(true);
                                let name = device_name.clone();
                                let viewers = viewers_bg.clone();
                                let token = token_bg.clone();
                                let stop_conn = stop_rx.clone();
                                tokio::spawn(async move {
                                    handle_connection(stream, peer_addr, name, token, viewers, stop_conn).await;
                                });
                            }
                            Err(err) => {
                                tracing::warn!(%err, "TREE LINK screen accept error");
                            }
                        }
                    }
                }
            }
            tracing::info!(%local_addr, "TREE LINK remote desktop server stopped");
        });

        Ok(Self {
            local_addr,
            active_viewers,
            auth_token,
            stop_tx,
            task: Mutex::new(Some(task)),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Rotate or update the active group authentication token (e.g. after pairing or group change).
    pub fn set_auth_token(&self, token: String) {
        *self.auth_token.lock().unwrap_or_else(|e| e.into_inner()) = token;
    }

    pub fn status(&self) -> HostStatus {
        HostStatus {
            running: !*self.stop_tx.borrow(),
            port: self.local_addr.port(),
            host_capture_supported: crate::platform::host_capture_supported(),
            active_viewers: self.active_viewers.load(Ordering::Relaxed),
            permissions: crate::platform::permissions(),
        }
    }

    pub fn stop(&self) {
        let _ = self.stop_tx.send(true);
        if let Some(handle) = self.task.lock().unwrap_or_else(|e| e.into_inner()).take() {
            handle.abort();
        }
    }
}

impl Drop for ScreenServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn token_matches(expected: &str, provided: Option<&str>) -> bool {
    if expected.is_empty() {
        return true;
    }
    let Some(provided) = provided else {
        return false;
    };
    constant_time_eq::constant_time_eq(expected.as_bytes(), provided.as_bytes())
}

enum OutboundPacket {
    Text(String),
    Pong(Vec<u8>),
}

async fn handle_connection(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    device_name: Arc<String>,
    auth_token: Arc<Mutex<String>>,
    active_viewers: Arc<AtomicUsize>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let req = match read_http_handshake(&mut stream).await {
        Ok(r) => r,
        Err(err) => {
            tracing::debug!(%peer_addr, %err, "TREE LINK handshake rejected");
            return;
        }
    };

    if req.path == "/health" {
        let body = serde_json::json!({
            "service": "tree-link-screen",
            "deviceName": device_name.as_str(),
            "os": std::env::consts::OS,
            "hostCaptureSupported": crate::platform::host_capture_supported(),
            "permissions": crate::platform::permissions(),
            "interfaces": rank_local_interfaces(),
        });
        let bytes = serde_json::to_vec(&body).unwrap_or_default();
        let _ = write_http_response(&mut stream, "200 OK", "application/json", &bytes).await;
        return;
    }

    let expected = auth_token.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let provided = req
        .query
        .get("token")
        .map(String::as_str)
        .or_else(|| req.headers.get("x-treelink-token").map(String::as_str));

    if !token_matches(&expected, provided) {
        let _ = write_http_response(&mut stream, "401 Unauthorized", "text/plain", b"unauthorized").await;
        return;
    }

    if let Err(err) = complete_ws_handshake(&mut stream, &req).await {
        tracing::debug!(%peer_addr, %err, "TREE LINK websocket upgrade failed");
        return;
    }

    active_viewers.fetch_add(1, Ordering::AcqRel);
    struct ViewerGuard(Arc<AtomicUsize>);
    impl Drop for ViewerGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }
    let _viewer_guard = ViewerGuard(active_viewers);

    let (mut reader, mut writer) = stream.into_split();
    let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<OutboundPacket>(64);
    let (video_tx, mut video_rx) = mpsc::channel::<Vec<u8>>(MAX_QUEUED_VIDEO_FRAMES);

    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                ctrl = ctrl_rx.recv() => {
                    match ctrl {
                        Some(OutboundPacket::Text(t)) => {
                            if write_ws_text(&mut writer, &t).await.is_err() { break; }
                        }
                        Some(OutboundPacket::Pong(p)) => {
                            if write_ws_pong(&mut writer, &p).await.is_err() { break; }
                        }
                        None => break,
                    }
                    if writer.flush().await.is_err() { break; }
                }
                frame = video_rx.recv() => {
                    match frame {
                        Some(bytes) => {
                            if write_ws_binary(&mut writer, &bytes).await.is_err() { break; }
                            if writer.flush().await.is_err() { break; }
                        }
                        None => break,
                    }
                }
            }
        }
        let _ = write_ws_close(&mut writer).await;
        let _ = writer.flush().await;
    });

    // Send initial Hello with displays, permissions, and ranked interfaces.
    let hello = ServerMsg::Hello {
        device_name: (*device_name).clone(),
        os: std::env::consts::OS.to_string(),
        host_capture_supported: crate::platform::host_capture_supported(),
        permissions: crate::platform::permissions(),
        displays: crate::platform::list_displays(),
        interfaces: rank_local_interfaces(),
        codecs: vec![VideoCodec::H264, VideoCodec::Hevc],
    };
    if let Ok(text) = serde_json::to_string(&hello) {
        let _ = ctrl_tx.send(OutboundPacket::Text(text)).await;
    }

    let mut active_capture: Option<Arc<CaptureHandle>> = None;
    let mut injector = InputInjector::new();

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break;
                }
            }
            msg = read_ws_message(&mut reader) => {
                match msg {
                    Ok(WsMessage::Text(text)) => {
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(ClientMsg::Subscribe(cfg)) => {
                                active_capture = None;
                                match start_capture_for_client(&cfg, video_tx.clone()) {
                                    Ok(handle) => {
                                        active_capture = Some(handle);
                                        send_server_msg(
                                            &ctrl_tx,
                                            &ServerMsg::StreamStarted {
                                                display_id: cfg.display_id,
                                                codec: cfg.codec,
                                                fps: cfg.fps,
                                                bitrate_kbps: cfg.bitrate_kbps,
                                            },
                                        ).await;
                                    }
                                    Err(err) => {
                                        send_server_msg(
                                            &ctrl_tx,
                                            &ServerMsg::Error {
                                                code: err.clone(),
                                                message: format!("无法启动远程桌面采集：{err}"),
                                            },
                                        ).await;
                                    }
                                }
                            }
                            Ok(ClientMsg::StopStream) => {
                                active_capture = None;
                                send_server_msg(&ctrl_tx, &ServerMsg::StreamStopped).await;
                            }
                            Ok(ClientMsg::RequestKeyframe) => {
                                if let Some(cap) = &active_capture {
                                    cap.request_keyframe();
                                }
                            }
                            Ok(ClientMsg::Input { event }) => {
                                injector.inject(event);
                            }
                            Ok(ClientMsg::Ping { client_time_ms }) => {
                                send_server_msg(
                                    &ctrl_tx,
                                    &ServerMsg::Pong {
                                        client_time_ms,
                                        server_time_ms: unix_ms(),
                                    },
                                ).await;
                            }
                            Ok(ClientMsg::ListDisplays) => {
                                send_server_msg(
                                    &ctrl_tx,
                                    &ServerMsg::Displays {
                                        displays: crate::platform::list_displays(),
                                    },
                                ).await;
                            }
                            Err(err) => {
                                send_server_msg(
                                    &ctrl_tx,
                                    &ServerMsg::Error {
                                        code: "invalid_message".into(),
                                        message: err.to_string(),
                                    },
                                ).await;
                            }
                        }
                    }
                    Ok(WsMessage::Ping(payload)) => {
                        let _ = ctrl_tx.try_send(OutboundPacket::Pong(payload));
                    }
                    Ok(WsMessage::Pong(_)) | Ok(WsMessage::Binary(_)) => {}
                    Ok(WsMessage::Close) | Err(_) => break,
                }
            }
        }
    }

    drop(active_capture);
    injector.release_all();
    drop(ctrl_tx);
    drop(video_tx);
    let _ = writer_task.await;
}

fn start_capture_for_client(cfg: &StreamConfig, video_tx: mpsc::Sender<Vec<u8>>) -> Result<Arc<CaptureHandle>, String> {
    let needs_keyframe = Arc::new(AtomicBool::new(false));
    let capture_slot: Arc<Mutex<Option<std::sync::Weak<CaptureHandle>>>> = Arc::new(Mutex::new(None));
    let slot_cb = capture_slot.clone();
    let needs_kf_cb = needs_keyframe.clone();

    let sink = Arc::new(move |frame: EncodedVideoFrame| {
        if needs_kf_cb.load(Ordering::Acquire) && !frame.is_keyframe {
            return;
        }
        let wire = encode_video_frame(&frame);
        match video_tx.try_send(wire) {
            Ok(()) => {
                if frame.is_keyframe {
                    needs_kf_cb.store(false, Ordering::Release);
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Drop stale frame rather than buffering latency; immediately request a fresh IDR.
                needs_kf_cb.store(true, Ordering::Release);
                if let Some(weak) = slot_cb.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
                    && let Some(handle) = weak.upgrade()
                {
                    handle.request_keyframe();
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    });

    let handle = Arc::new(CaptureHandle::start(cfg, sink)?);
    *capture_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(&handle));
    Ok(handle)
}

async fn send_server_msg(ctrl_tx: &mpsc::Sender<OutboundPacket>, msg: &ServerMsg) {
    if let Ok(text) = serde_json::to_string(msg) {
        let _ = ctrl_tx.send(OutboundPacket::Text(text)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn health_and_ws_ping_pong_roundtrip() {
        let server = ScreenServer::start(ServerConfig {
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            device_name: "TestMac".into(),
            auth_token: "secret123".into(),
        })
        .await
        .unwrap();
        let addr = server.local_addr();

        // 1. Health endpoint works over plain HTTP.
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = Vec::new();
        s.read_to_end(&mut resp).await.unwrap();
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(resp_str.contains("200 OK"));
        assert!(resp_str.contains("TestMac"));

        // 2. Unauthorized WebSocket is rejected with 401.
        let mut bad = TcpStream::connect(addr).await.unwrap();
        bad.write_all(
            b"GET /ws?token=wrong HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .await
        .unwrap();
        let mut bad_resp = Vec::new();
        bad.read_to_end(&mut bad_resp).await.unwrap();
        assert!(String::from_utf8_lossy(&bad_resp).contains("401 Unauthorized"));

        // 3. Authorized WebSocket receives Hello and answers Ping.
        let mut ws = TcpStream::connect(addr).await.unwrap();
        ws.write_all(
            b"GET /ws?token=secret123 HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .await
        .unwrap();
        let mut head = [0u8; 256];
        let mut collected = Vec::new();
        let header_end = loop {
            let n = ws.read(&mut head).await.unwrap();
            collected.extend_from_slice(&head[..n]);
            if let Some(pos) = collected.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        assert!(String::from_utf8_lossy(&collected[..header_end]).contains("101 Switching Protocols"));

        // Feed any leftover bytes + stream into a chained reader.
        let leftover = collected[header_end..].to_vec();
        let (ws_r, mut ws_w) = ws.into_split();
        let mut combined = tokio::io::AsyncReadExt::chain(std::io::Cursor::new(leftover), ws_r);

        let hello = read_ws_message(&mut combined).await.unwrap();
        let WsMessage::Text(hello_json) = hello else {
            panic!("expected hello text frame");
        };
        let hello_msg: ServerMsg = serde_json::from_str(&hello_json).unwrap();
        assert!(matches!(hello_msg, ServerMsg::Hello { ref device_name, .. } if device_name == "TestMac"));

        // Send masked Ping JSON.
        let ping_json = serde_json::to_vec(&ClientMsg::Ping { client_time_ms: 999 }).unwrap();
        let mask = [0x11, 0x22, 0x33, 0x44];
        let mut frame = vec![0x81, 0x80 | (ping_json.len() as u8)];
        frame.extend_from_slice(&mask);
        for (i, b) in ping_json.iter().enumerate() {
            frame.push(b ^ mask[i % 4]);
        }
        ws_w.write_all(&frame).await.unwrap();
        ws_w.flush().await.unwrap();

        let pong = read_ws_message(&mut combined).await.unwrap();
        let WsMessage::Text(pong_json) = pong else {
            panic!("expected pong text frame");
        };
        let pong_msg: ServerMsg = serde_json::from_str(&pong_json).unwrap();
        assert!(matches!(
            pong_msg,
            ServerMsg::Pong {
                client_time_ms: 999,
                ..
            }
        ));

        server.stop();
    }
}

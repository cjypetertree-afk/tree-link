//! Authenticated low-latency WebSocket server for TREE LINK remote desktop streaming.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, watch};

use crate::network_rank::rank_local_interfaces;
use crate::platform::{CaptureHandle, InputInjector};
use crate::protocol::{
    ClientMsg, DEFAULT_SCREEN_PORT, EncodedVideoFrame, PermissionSummary, ServerMsg, StreamConfig, VideoCodec,
    decode_binary_input, encode_video_frame,
};
use crate::ws::{
    WsMessage, complete_ws_handshake, read_http_body, read_http_handshake, read_ws_message, write_http_response,
    write_ws_binary, write_ws_close, write_ws_pong, write_ws_text,
};

/// Embedded self-contained multi-Mac web control console served at `GET /`.
const VIEWER_HTML: &str = include_str!("viewer.html");
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Maximum queued encoded delta video frames per viewer (H.264/HEVC) before dropping stale frames.
const MAX_QUEUED_VIDEO_FRAMES: usize = 2;

/// Zero-queue mailbox for full-frame JPEG: newer frames overwrite unsent frames so latency never stacks.
#[derive(Default)]
struct LatestFrameSlot {
    frame: Mutex<Option<Vec<u8>>>,
    notify: Notify,
}

impl LatestFrameSlot {
    fn put(&self, bytes: Vec<u8>) {
        *self.frame.lock().unwrap_or_else(|e| e.into_inner()) = Some(bytes);
        self.notify.notify_one();
    }

    fn take(&self) -> Option<Vec<u8>> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

fn build_self_signed_tls_acceptor() -> Option<tokio_rustls::TlsAcceptor> {
    let mut sans = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    for iface in rank_local_interfaces() {
        if !sans.contains(&iface.ip) {
            sans.push(iface.ip);
        }
    }
    let mut params = rcgen::CertificateParams::new(sans).ok()?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "TREE LINK LAN Host");
    let kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).ok()?;
    let cert = params.self_signed(&kp).ok()?;
    let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        kp.serialize_der(),
    ));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server_cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .ok()?
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .ok()?;
    Some(tokio_rustls::TlsAcceptor::from(Arc::new(server_cfg)))
}

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
    tls_addr: Option<SocketAddr>,
    active_viewers: Arc<AtomicUsize>,
    auth_token: Arc<Mutex<String>>,
    stop_tx: watch::Sender<bool>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    tls_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for ScreenServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenServer")
            .field("local_addr", &self.local_addr)
            .field("tls_addr", &self.tls_addr)
            .field("active_viewers", &self.active_viewers.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl ScreenServer {
    /// Start the TREE LINK screen server (HTTP/WS + companion self-signed HTTPS/WSS on `port + 1`
    /// so LAN browsers get `window.isSecureContext` and WebCodecs 60fps Hardware H.264/HEVC).
    /// If the requested port is busy, falls back to an OS-assigned ephemeral port.
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

        // Bind companion HTTPS/WSS listener (`port + 1`, or ephemeral when `config.port == 0`).
        let mut tls_addr = None;
        let mut tls_handle = None;
        if let Some(acceptor) = build_self_signed_tls_acceptor() {
            let tls_preferred_port = if config.port == 0 {
                0
            } else {
                local_addr.port().saturating_add(1)
            };
            let tls_listener = match TcpListener::bind(SocketAddr::new(config.bind_ip, tls_preferred_port)).await {
                Ok(l) => Some(l),
                Err(_) if tls_preferred_port != 0 => {
                    TcpListener::bind(SocketAddr::new(config.bind_ip, 0)).await.ok()
                }
                Err(_) => None,
            };
            if let Some(tls_listener) = tls_listener
                && let Ok(bound_tls) = tls_listener.local_addr()
            {
                tls_addr = Some(bound_tls);
                let tls_port = Some(bound_tls.port());
                let name_tls = device_name.clone();
                let viewers_tls = active_viewers.clone();
                let token_tls = auth_token.clone();
                let mut stop_tls = stop_rx.clone();
                tls_handle = Some(tokio::spawn(async move {
                    tracing::info!(%bound_tls, "TREE LINK remote desktop HTTPS/WSS server listening");
                    loop {
                        tokio::select! {
                            _ = stop_tls.changed() => {
                                if *stop_tls.borrow() {
                                    break;
                                }
                            }
                            accepted = tls_listener.accept() => {
                                match accepted {
                                    Ok((tcp, peer_addr)) => {
                                        let _ = tcp.set_nodelay(true);
                                        let acceptor = acceptor.clone();
                                        let name = name_tls.clone();
                                        let viewers = viewers_tls.clone();
                                        let token = token_tls.clone();
                                        let stop_conn = stop_tls.clone();
                                        tokio::spawn(async move {
                                            match acceptor.accept(tcp).await {
                                                Ok(tls_stream) => {
                                                    handle_connection(
                                                        tls_stream,
                                                        peer_addr,
                                                        name,
                                                        token,
                                                        viewers,
                                                        tls_port,
                                                        stop_conn,
                                                    )
                                                    .await;
                                                }
                                                Err(err) => {
                                                    tracing::debug!(%peer_addr, %err, "TREE LINK TLS handshake failed");
                                                }
                                            }
                                        });
                                    }
                                    Err(err) => {
                                        tracing::warn!(%err, "TREE LINK TLS accept error");
                                    }
                                }
                            }
                        }
                    }
                }));
            }
        }

        let tls_port_opt = tls_addr.map(|a| a.port());
        let viewers_bg = active_viewers.clone();
        let token_bg = auth_token.clone();

        let task = tokio::spawn(async move {
            tracing::info!(%local_addr, ?tls_port_opt, "TREE LINK remote desktop server listening");
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
                                    handle_connection(
                                        stream,
                                        peer_addr,
                                        name,
                                        token,
                                        viewers,
                                        tls_port_opt,
                                        stop_conn,
                                    )
                                    .await;
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
            tls_addr,
            active_viewers,
            auth_token,
            stop_tx,
            task: Mutex::new(Some(task)),
            tls_task: Mutex::new(tls_handle),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn tls_addr(&self) -> Option<SocketAddr> {
        self.tls_addr
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
        if let Some(handle) = self.tls_task.lock().unwrap_or_else(|e| e.into_inner()).take() {
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

async fn handle_connection<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    mut stream: S,
    peer_addr: SocketAddr,
    device_name: Arc<String>,
    auth_token: Arc<Mutex<String>>,
    active_viewers: Arc<AtomicUsize>,
    tls_port: Option<u16>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let req = match read_http_handshake(&mut stream).await {
        Ok(r) => r,
        Err(err) => {
            tracing::debug!(%peer_addr, %err, "TREE LINK handshake rejected");
            return;
        }
    };

    if req.method == "OPTIONS" {
        let _ = write_http_response(&mut stream, "204 No Content", "text/plain", b"").await;
        return;
    }

    if req.method == "GET" && (req.path == "/" || req.path == "/index.html") {
        let _ = write_http_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            VIEWER_HTML.as_bytes(),
        )
        .await;
        return;
    }

    if req.path == "/health" {
        let body = serde_json::json!({
            "service": "tree-link-screen",
            "deviceName": device_name.as_str(),
            "os": std::env::consts::OS,
            "hostCaptureSupported": crate::platform::host_capture_supported(),
            "permissions": crate::platform::permissions(),
            "interfaces": rank_local_interfaces(),
            "tlsPort": tls_port,
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

    if req.path == "/api/clipboard" {
        handle_clipboard_http(&mut stream, &req).await;
        return;
    }

    if req.path == "/api/upload" && req.method == "POST" {
        handle_upload_http(&mut stream, &req).await;
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

    let (mut reader, mut writer) = tokio::io::split(stream);
    let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<OutboundPacket>(64);
    let (video_tx, mut video_rx) = mpsc::channel::<Vec<u8>>(MAX_QUEUED_VIDEO_FRAMES);
    let latest_jpeg = Arc::new(LatestFrameSlot::default());
    let latest_jpeg_writer = latest_jpeg.clone();

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
                _ = latest_jpeg_writer.notify.notified() => {
                    if let Some(bytes) = latest_jpeg_writer.take() {
                        if write_ws_binary(&mut writer, &bytes).await.is_err() { break; }
                        if writer.flush().await.is_err() { break; }
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
        codecs: vec![VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Jpeg],
        tls_port,
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
                    Ok(WsMessage::Binary(bytes)) => {
                        if let Some(event) = decode_binary_input(&bytes) {
                            injector.inject(event);
                        }
                    }
                    Ok(WsMessage::Text(text)) => {
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(ClientMsg::Subscribe(cfg)) => {
                                active_capture = None;
                                let _ = latest_jpeg.take();
                                match start_capture_for_client(&cfg, video_tx.clone(), latest_jpeg.clone()) {
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
                                let _ = latest_jpeg.take();
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
                            Ok(ClientMsg::CheckPermissions) => {
                                send_server_msg(
                                    &ctrl_tx,
                                    &ServerMsg::Permissions {
                                        permissions: crate::platform::permissions(),
                                    },
                                ).await;
                            }
                            Ok(ClientMsg::RequestPermissions) => {
                                let sr = crate::platform::request_screen_recording_permission();
                                let ax = crate::platform::request_accessibility_permission();
                                #[cfg(target_os = "macos")]
                                {
                                    if !ax {
                                        let _ = std::process::Command::new("/usr/bin/open")
                                            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
                                            .spawn();
                                    } else if !sr {
                                        let _ = std::process::Command::new("/usr/bin/open")
                                            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
                                            .spawn();
                                    }
                                }
                                let _ = (sr, ax);
                                send_server_msg(
                                    &ctrl_tx,
                                    &ServerMsg::Permissions {
                                        permissions: crate::platform::permissions(),
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
                    Ok(WsMessage::Pong(_)) => {}
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

fn start_capture_for_client(
    cfg: &StreamConfig,
    video_tx: mpsc::Sender<Vec<u8>>,
    latest_jpeg: Arc<LatestFrameSlot>,
) -> Result<Arc<CaptureHandle>, String> {
    let needs_keyframe = Arc::new(AtomicBool::new(false));
    let capture_slot: Arc<Mutex<Option<std::sync::Weak<CaptureHandle>>>> = Arc::new(Mutex::new(None));
    let slot_cb = capture_slot.clone();
    let needs_kf_cb = needs_keyframe.clone();

    let sink = Arc::new(move |frame: EncodedVideoFrame| {
        if frame.codec == VideoCodec::Jpeg {
            // Overwrite any unsent JPEG frame with the freshest capture (zero queue latency).
            let wire = encode_video_frame(&frame);
            latest_jpeg.put(wire);
            return;
        }
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

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex, 16)
        {
            out.push(val);
            i += 3;
            continue;
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn sanitize_filename(raw: &str) -> Option<String> {
    let decoded = percent_decode(raw);
    let base = std::path::Path::new(&decoded).file_name()?.to_str()?.trim();
    if base.is_empty()
        || base == "."
        || base == ".."
        || base.contains('/')
        || base.contains('\\')
        || base.contains('\0')
    {
        return None;
    }
    Some(base.to_string())
}

async fn handle_clipboard_http<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    req: &crate::ws::HandshakeRequest,
) {
    if req.method == "GET" {
        let text = read_local_clipboard_text().await.unwrap_or_default();
        let body = serde_json::to_vec(&serde_json::json!({ "text": text })).unwrap_or_default();
        let _ = write_http_response(stream, "200 OK", "application/json", &body).await;
        return;
    }
    if req.method == "POST" {
        match read_http_body(stream, req, MAX_CLIPBOARD_BYTES).await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if write_local_clipboard_text(text).await.is_ok() {
                    let _ = write_http_response(stream, "200 OK", "application/json", b"{\"ok\":true}").await;
                } else {
                    let _ = write_http_response(
                        stream,
                        "500 Internal Server Error",
                        "application/json",
                        b"{\"error\":\"clipboard_write_failed\"}",
                    )
                    .await;
                }
            }
            Err(_) => {
                let _ = write_http_response(stream, "413 Payload Too Large", "text/plain", b"too large").await;
            }
        }
        return;
    }
    let _ = write_http_response(stream, "405 Method Not Allowed", "text/plain", b"method not allowed").await;
}

async fn handle_upload_http<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    req: &crate::ws::HandshakeRequest,
) {
    let Some(raw_name) = req.query.get("name") else {
        let _ = write_http_response(stream, "400 Bad Request", "text/plain", b"missing name").await;
        return;
    };
    let Some(file_name) = sanitize_filename(raw_name) else {
        let _ = write_http_response(stream, "400 Bad Request", "text/plain", b"invalid filename").await;
        return;
    };
    let body = match read_http_body(stream, req, MAX_UPLOAD_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            let _ = write_http_response(stream, "413 Payload Too Large", "text/plain", b"file exceeds 64 MiB").await;
            return;
        }
    };

    let base_dir = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Downloads")
        .join("TREE-LINK-Received");
    if tokio::fs::create_dir_all(&base_dir).await.is_err() {
        let _ = write_http_response(stream, "500 Internal Server Error", "text/plain", b"mkdir failed").await;
        return;
    }

    let mut dest = base_dir.join(&file_name);
    if dest.exists() {
        let stem = std::path::Path::new(&file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let ext = std::path::Path::new(&file_name).extension().and_then(|s| s.to_str());
        for idx in 1u32..1000 {
            let candidate = match ext {
                Some(e) => base_dir.join(format!("{stem}-{idx}.{e}")),
                None => base_dir.join(format!("{stem}-{idx}")),
            };
            if !candidate.exists() {
                dest = candidate;
                break;
            }
        }
    }

    match tokio::fs::write(&dest, &body).await {
        Ok(()) => {
            let resp = serde_json::json!({
                "savedPath": dest.display().to_string(),
                "bytes": body.len(),
            });
            let bytes = serde_json::to_vec(&resp).unwrap_or_default();
            let _ = write_http_response(stream, "200 OK", "application/json", &bytes).await;
        }
        Err(_) => {
            let _ = write_http_response(stream, "500 Internal Server Error", "text/plain", b"write failed").await;
        }
    }
}

async fn read_local_clipboard_text() -> std::io::Result<String> {
    #[cfg(target_os = "macos")]
    {
        let out = tokio::process::Command::new("/usr/bin/pbpaste").output().await?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(String::new())
    }
}

async fn write_local_clipboard_text(text: String) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Stdio;
        let mut child = tokio::process::Command::new("/usr/bin/pbcopy")
            .stdin(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes()).await?;
        }
        let _ = child.wait().await?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpStream;

    #[test]
    fn sanitize_filename_rejects_traversal() {
        assert_eq!(sanitize_filename("report%20v1.pdf").as_deref(), Some("report v1.pdf"));
        assert_eq!(sanitize_filename("../../secret.txt").as_deref(), Some("secret.txt"));
        assert_eq!(sanitize_filename(".."), None);
        assert_eq!(sanitize_filename("."), None);
        assert_eq!(sanitize_filename(""), None);
    }

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
        assert!(server.tls_addr().is_some(), "companion TLS listener should bind");

        // 1. Health endpoint works over plain HTTP and advertises tlsPort.
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = Vec::new();
        s.read_to_end(&mut resp).await.unwrap();
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(resp_str.contains("200 OK"));
        assert!(resp_str.contains("TestMac"));
        assert!(resp_str.contains("\"tlsPort\""));

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

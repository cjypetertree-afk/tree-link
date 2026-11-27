//! Minimal, zero-extra-dependency RFC 6455 WebSocket server over `tokio::net::TcpStream`.
//! Keeps `Cargo.lock` untouched while giving full control over `TCP_NODELAY`, binary video frames,
//! and bounded low-latency frame drops.

use base64::Engine as _;
use std::collections::HashMap;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const MAX_INBOUND_FRAME_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP handshake malformed: {0}")]
    Handshake(&'static str),
    #[error("unauthorized")]
    Unauthorized,
    #[error("WebSocket protocol error: {0}")]
    Protocol(&'static str),
    #[error("connection closed")]
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

#[derive(Clone, Debug)]
pub struct HandshakeRequest {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body_prefix: Vec<u8>,
}

/// Pure-Rust RFC 3174 SHA-1 used exclusively for RFC 6455 `Sec-WebSocket-Accept`.
pub fn sha1_digest(input: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let bit_len = (input.len() as u64) * 8;
    let mut msg = input.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn accept_key(sec_websocket_key: &str) -> String {
    let mut combined = String::with_capacity(sec_websocket_key.len() + WS_GUID.len());
    combined.push_str(sec_websocket_key.trim());
    combined.push_str(WS_GUID);
    let digest = sha1_digest(combined.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(digest)
}

pub async fn read_http_handshake<S: AsyncRead + Unpin>(stream: &mut S) -> Result<HandshakeRequest, WsError> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 512];
    let header_end = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Err(WsError::Closed);
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HTTP_HEADER_BYTES {
            return Err(WsError::Handshake("header too large"));
        }
    };

    let head = std::str::from_utf8(&buf[..header_end]).map_err(|_| WsError::Handshake("invalid utf8"))?;
    let mut lines = head.split("\r\n");
    let req_line = lines.next().ok_or(WsError::Handshake("empty request"))?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next().ok_or(WsError::Handshake("missing method"))?;
    let target = parts.next().ok_or(WsError::Handshake("missing target"))?;
    if !matches!(method, "GET" | "POST" | "OPTIONS") {
        return Err(WsError::Handshake("unsupported HTTP method"));
    }

    let (path, query_str) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q)),
        None => (target.to_string(), None),
    };
    let mut query = HashMap::new();
    if let Some(q) = query_str {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                query.insert(k.to_string(), v.to_string());
            }
        }
    }

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, val)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), val.trim().to_string());
        }
    }

    let body_prefix = buf[header_end + 4..].to_vec();
    Ok(HandshakeRequest {
        method: method.to_string(),
        path,
        query,
        headers,
        body_prefix,
    })
}

pub async fn read_http_body<S: AsyncRead + Unpin>(
    stream: &mut S,
    req: &HandshakeRequest,
    max_bytes: usize,
) -> Result<Vec<u8>, WsError> {
    let content_len = req
        .headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if content_len > max_bytes {
        return Err(WsError::Protocol("HTTP body exceeds limit"));
    }
    let mut body = Vec::with_capacity(content_len);
    let take_prefix = req.body_prefix.len().min(content_len);
    body.extend_from_slice(&req.body_prefix[..take_prefix]);
    if body.len() < content_len {
        let remaining = content_len - body.len();
        let old_len = body.len();
        body.resize(old_len + remaining, 0);
        stream.read_exact(&mut body[old_len..]).await?;
    }
    Ok(body)
}

pub async fn write_http_response<S: AsyncWrite + Unpin>(
    stream: &mut S,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(), WsError> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, X-TreeLink-Token\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

pub async fn complete_ws_handshake<S: AsyncWrite + Unpin>(
    stream: &mut S,
    req: &HandshakeRequest,
) -> Result<(), WsError> {
    let upgrade = req.headers.get("upgrade").map(|s| s.to_ascii_lowercase());
    if upgrade.as_deref() != Some("websocket") {
        return Err(WsError::Handshake("missing websocket upgrade header"));
    }
    let key = req
        .headers
        .get("sec-websocket-key")
        .ok_or(WsError::Handshake("missing Sec-WebSocket-Key"))?;
    let accept = accept_key(key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Read one complete RFC 6455 message from a client (handles masking and continuation frames).
pub async fn read_ws_message<S: AsyncRead + Unpin>(stream: &mut S) -> Result<WsMessage, WsError> {
    let mut assembled = Vec::new();
    let mut initial_opcode: Option<u8> = None;

    loop {
        let mut head = [0u8; 2];
        match stream.read_exact(&mut head).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(WsError::Closed),
            Err(e) => return Err(WsError::Io(e)),
        }
        let fin = (head[0] & 0x80) != 0;
        let opcode = head[0] & 0x0F;
        let masked = (head[1] & 0x80) != 0;
        let mut len = u64::from(head[1] & 0x7F);

        if len == 126 {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext).await?;
            len = u64::from(u16::from_be_bytes(ext));
        } else if len == 127 {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext).await?;
            len = u64::from_be_bytes(ext);
        }

        if len as usize > MAX_INBOUND_FRAME_BYTES || assembled.len() + (len as usize) > MAX_INBOUND_FRAME_BYTES {
            return Err(WsError::Protocol("frame exceeds size limit"));
        }

        let mask = if masked {
            let mut m = [0u8; 4];
            stream.read_exact(&mut m).await?;
            Some(m)
        } else {
            None
        };

        let mut payload = vec![0u8; len as usize];
        if !payload.is_empty() {
            stream.read_exact(&mut payload).await?;
            if let Some(m) = mask {
                for (i, byte) in payload.iter_mut().enumerate() {
                    *byte ^= m[i % 4];
                }
            }
        }

        // Control frames may be interleaved and are never fragmented.
        match opcode {
            0x8 => return Ok(WsMessage::Close),
            0x9 => return Ok(WsMessage::Ping(payload)),
            0xA => return Ok(WsMessage::Pong(payload)),
            0x1 | 0x2 => {
                if initial_opcode.is_some() {
                    return Err(WsError::Protocol("unexpected new data opcode mid-fragment"));
                }
                initial_opcode = Some(opcode);
                assembled.extend_from_slice(&payload);
            }
            0x0 => {
                if initial_opcode.is_none() {
                    return Err(WsError::Protocol("unexpected continuation frame"));
                }
                assembled.extend_from_slice(&payload);
            }
            _ => return Err(WsError::Protocol("unsupported opcode")),
        }

        if fin {
            return match initial_opcode {
                Some(0x1) => {
                    let text = String::from_utf8(assembled).map_err(|_| WsError::Protocol("invalid utf8 text"))?;
                    Ok(WsMessage::Text(text))
                }
                Some(0x2) => Ok(WsMessage::Binary(assembled)),
                _ => Err(WsError::Protocol("missing data opcode")),
            };
        }
    }
}

/// Write a single unmasked server-to-client WebSocket frame.
pub async fn write_ws_frame<S: AsyncWrite + Unpin>(stream: &mut S, opcode: u8, payload: &[u8]) -> Result<(), WsError> {
    let mut header = Vec::with_capacity(10);
    header.push(0x80 | (opcode & 0x0F));
    let len = payload.len();
    if len < 126 {
        header.push(len as u8);
    } else if len <= u16::MAX as usize {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    stream.write_all(&header).await?;
    if !payload.is_empty() {
        stream.write_all(payload).await?;
    }
    Ok(())
}

pub async fn write_ws_text<S: AsyncWrite + Unpin>(stream: &mut S, text: &str) -> Result<(), WsError> {
    write_ws_frame(stream, 0x1, text.as_bytes()).await
}

pub async fn write_ws_binary<S: AsyncWrite + Unpin>(stream: &mut S, data: &[u8]) -> Result<(), WsError> {
    write_ws_frame(stream, 0x2, data).await
}

pub async fn write_ws_pong<S: AsyncWrite + Unpin>(stream: &mut S, payload: &[u8]) -> Result<(), WsError> {
    write_ws_frame(stream, 0xA, payload).await
}

pub async fn write_ws_close<S: AsyncWrite + Unpin>(stream: &mut S) -> Result<(), WsError> {
    write_ws_frame(stream, 0x8, &[]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6455_accept_key_vector() {
        // Official RFC 6455 Section 1.3 test vector.
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[tokio::test]
    async fn ws_roundtrip_masked_client_text_and_server_binary() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let server_task = tokio::spawn(async move {
            let msg = read_ws_message(&mut server).await.unwrap();
            assert_eq!(msg, WsMessage::Text("hello tree link".into()));
            write_ws_binary(&mut server, &[1, 2, 3, 4]).await.unwrap();
            server.flush().await.unwrap();
        });

        // Send a masked client text frame.
        let payload = b"hello tree link";
        let mask = [0x37, 0xFA, 0x21, 0x3D];
        let mut frame = vec![0x81, 0x80 | (payload.len() as u8)];
        frame.extend_from_slice(&mask);
        for (i, b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i % 4]);
        }
        client.write_all(&frame).await.unwrap();
        client.flush().await.unwrap();

        let reply = read_ws_message(&mut client).await.unwrap();
        assert_eq!(reply, WsMessage::Binary(vec![1, 2, 3, 4]));
        server_task.await.unwrap();
    }
}

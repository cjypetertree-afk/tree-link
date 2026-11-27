import { useCallback, useEffect, useRef, useState } from 'react';
import { Button } from 'oink-ui';
import { domCodeToHid, HID_LEFT_GUI, HID_SPACE, HID_TAB, type ModifierPreset } from './hidKeymap';
import {
  buildWebSocketUrl,
  linkClassLabel,
  parseVideoFrame,
  QUALITY_PRESETS,
  type DisplayDescriptor,
  type LinkClass,
  type MouseButtonKind,
  type VideoCodec,
} from './protocol';

export interface RemoteTarget {
  id: string;
  name: string;
  ip: string;
  port: number;
  token: string;
  linkClass: LinkClass;
  deviceId?: string;
}

interface RemotePaneProps {
  target: RemoteTarget;
  speakerMuted?: boolean;
  onToggleAudioMute?: () => void;
  onClose: () => void;
}

const MOUSE_BUTTON_MAP: Record<number, MouseButtonKind> = {
  0: 'left',
  1: 'middle',
  2: 'right',
  3: 'back',
  4: 'forward',
};

function codecStringFor(codec: VideoCodec): string {
  return codec === 'hevc' ? 'hvc1.1.6.L120.B0' : 'avc1.640028';
}

export function RemotePane({ target, speakerMuted, onToggleAudioMute, onClose }: RemotePaneProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const decoderRef = useRef<VideoDecoder | null>(null);
  const decoderCodecRef = useRef<string>('');
  const awaitingKeyframeRef = useRef<boolean>(true);
  const jpegBusyRef = useRef<boolean>(false);
  const frameSizeRef = useRef<{ w: number; h: number }>({ w: 1920, h: 1080 });
  const fpsCounterRef = useRef<{ frames: number; lastMs: number }>({ frames: 0, lastMs: performance.now() });

  const [status, setStatus] = useState<'connecting' | 'streaming' | 'error' | 'closed'>('connecting');
  const [errorText, setErrorText] = useState<string | null>(null);
  const [axGranted, setAxGranted] = useState<boolean | null>(null);
  const [displays, setDisplays] = useState<DisplayDescriptor[]>([]);
  const [displayId, setDisplayId] = useState<number>(0);
  const [presetId, setPresetId] = useState<string>(QUALITY_PRESETS[0].id);
  const [codec, setCodec] = useState<VideoCodec>(() => (typeof VideoDecoder !== 'undefined' ? 'h264' : 'jpeg'));
  const [modPreset, setModPreset] = useState<ModifierPreset>('ctrl_as_cmd');
  const [focused, setFocused] = useState<boolean>(false);
  const [rttMs, setRttMs] = useState<number | null>(null);
  const [fps, setFps] = useState<number>(0);
  const [resolution, setResolution] = useState<string>('');
  const [reconnectSeq, setReconnectSeq] = useState<number>(0);

  const sendJson = useCallback((obj: unknown) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(obj));
    }
  }, []);

  const subscribeStream = useCallback((nextDisplayId: number, nextPresetId: string, nextCodec: VideoCodec) => {
    const preset = QUALITY_PRESETS.find(p => p.id === nextPresetId) ?? QUALITY_PRESETS[0];
    awaitingKeyframeRef.current = true;
    sendJson({
      type: 'subscribe',
      displayId: nextDisplayId,
      maxWidth: preset.maxWidth,
      maxHeight: preset.maxHeight,
      fps: preset.fps,
      bitrateKbps: preset.bitrateKbps,
      codec: nextCodec,
      showCursor: true,
    });
  }, [sendJson]);

  const ensureDecoder = useCallback((videoCodec: VideoCodec, width: number, height: number) => {
    if (typeof VideoDecoder === 'undefined') {
      setErrorText('当前浏览器/WebView 不支持 WebCodecs 硬件视频解码');
      setStatus('error');
      return null;
    }
    const codecStr = codecStringFor(videoCodec);
    const key = `${codecStr}:${width}x${height}`;
    if (decoderRef.current && decoderRef.current.state === 'configured' && decoderCodecRef.current === key) {
      return decoderRef.current;
    }
    if (decoderRef.current) {
      try { decoderRef.current.close(); } catch { /* ignore */ }
      decoderRef.current = null;
    }
    const decoder = new VideoDecoder({
      output: (videoFrame) => {
        const canvas = canvasRef.current;
        if (canvas) {
          if (canvas.width !== videoFrame.displayWidth || canvas.height !== videoFrame.displayHeight) {
            canvas.width = videoFrame.displayWidth;
            canvas.height = videoFrame.displayHeight;
            frameSizeRef.current = { w: videoFrame.displayWidth, h: videoFrame.displayHeight };
            setResolution(`${videoFrame.displayWidth}×${videoFrame.displayHeight}`);
          }
          const ctx = canvas.getContext('2d', { alpha: false });
          if (ctx) {
            ctx.drawImage(videoFrame, 0, 0, canvas.width, canvas.height);
          }
        }
        videoFrame.close();
        const counter = fpsCounterRef.current;
        counter.frames += 1;
        const now = performance.now();
        if (now - counter.lastMs >= 1000) {
          setFps(Math.round((counter.frames * 1000) / (now - counter.lastMs)));
          counter.frames = 0;
          counter.lastMs = now;
        }
      },
      error: (err) => {
        awaitingKeyframeRef.current = true;
        setErrorText(`硬件解码器重置: ${err.message}`);
        sendJson({ type: 'request_keyframe' });
      },
    });
    decoder.configure({
      codec: codecStr,
      optimizeForLatency: true,
      hardwareAcceleration: 'prefer-hardware',
    });
    decoderRef.current = decoder;
    decoderCodecRef.current = key;
    return decoder;
  }, [sendJson]);

  useEffect(() => {
    setStatus('connecting');
    setErrorText(null);
    awaitingKeyframeRef.current = true;
    const url = buildWebSocketUrl(target.ip, target.port, target.token);
    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;

    const pingTimer = window.setInterval(() => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'ping', client_time_ms: Date.now() }));
      }
    }, 2000);

    ws.onmessage = (event) => {
      if (typeof event.data === 'string') {
        try {
          const msg = JSON.parse(event.data) as Record<string, unknown>;
          if (msg.type === 'hello') {
            const list = (msg.displays as DisplayDescriptor[] | undefined) ?? [];
            setDisplays(list);
            const primary = list.find(d => d.isPrimary)?.displayId ?? list[0]?.displayId ?? 0;
            setDisplayId(primary);
            const perms = msg.permissions as { screenRecording?: boolean; accessibility?: boolean } | undefined;
            if (msg.host_capture_supported === false) {
              setStatus('error');
              setErrorText('对端系统当前仅作为控屏接收端，暂不支持画面采集');
              return;
            }
            if (perms && perms.screenRecording === false) {
              setStatus('error');
              setErrorText('对端 Mac 尚未授予「屏幕录制」权限，请先在 Mac 的 TREE LINK 中授权');
              return;
            }
            if (perms && typeof perms.accessibility === 'boolean') {
              setAxGranted(perms.accessibility);
            }
            subscribeStream(primary, presetId, codec);
          } else if (msg.type === 'permissions') {
            const perms = msg.permissions as { accessibility?: boolean } | undefined;
            if (perms && typeof perms.accessibility === 'boolean') {
              setAxGranted(perms.accessibility);
            }
          } else if (msg.type === 'displays') {
            setDisplays((msg.displays as DisplayDescriptor[] | undefined) ?? []);
          } else if (msg.type === 'stream_started') {
            setStatus('streaming');
            setErrorText(null);
          } else if (msg.type === 'pong') {
            const sent = Number(msg.client_time_ms ?? 0);
            if (sent > 0) setRttMs(Math.max(0, Date.now() - sent));
          } else if (msg.type === 'error') {
            setStatus('error');
            setErrorText(String(msg.message ?? msg.code ?? '未知错误'));
          }
        } catch {
          // ignore malformed control frame
        }
        return;
      }

      if (event.data instanceof ArrayBuffer) {
        const frame = parseVideoFrame(event.data);
        if (!frame) return;
        if (frame.codec === 'jpeg') {
          if (jpegBusyRef.current) return;
          jpegBusyRef.current = true;
          const bytes = new Uint8Array(frame.payload.byteLength);
          bytes.set(frame.payload);
          const blob = new Blob([bytes], { type: 'image/jpeg' });
          void createImageBitmap(blob).then((bmp) => {
            const canvas = canvasRef.current;
            if (canvas) {
              if (canvas.width !== bmp.width || canvas.height !== bmp.height) {
                canvas.width = bmp.width;
                canvas.height = bmp.height;
                frameSizeRef.current = { w: bmp.width, h: bmp.height };
                setResolution(`${bmp.width}×${bmp.height}`);
              }
              const ctx = canvas.getContext('2d', { alpha: false });
              if (ctx) ctx.drawImage(bmp, 0, 0, canvas.width, canvas.height);
            }
            bmp.close();
            const counter = fpsCounterRef.current;
            counter.frames += 1;
            const now = performance.now();
            if (now - counter.lastMs >= 1000) {
              setFps(Math.round((counter.frames * 1000) / (now - counter.lastMs)));
              counter.frames = 0;
              counter.lastMs = now;
            }
            setStatus('streaming');
          }).catch(() => {}).finally(() => { jpegBusyRef.current = false; });
          return;
        }
        if (awaitingKeyframeRef.current && !frame.isKeyframe) {
          return;
        }
        const decoder = ensureDecoder(frame.codec, frame.width, frame.height);
        if (!decoder || decoder.state !== 'configured') return;
        if (decoder.decodeQueueSize > 3 && !frame.isKeyframe) {
          awaitingKeyframeRef.current = true;
          sendJson({ type: 'request_keyframe' });
          return;
        }
        try {
          decoder.decode(new EncodedVideoChunk({
            type: frame.isKeyframe ? 'key' : 'delta',
            timestamp: frame.timestampUs,
            data: frame.payload,
          }));
          if (frame.isKeyframe) {
            awaitingKeyframeRef.current = false;
            setStatus('streaming');
          }
        } catch {
          awaitingKeyframeRef.current = true;
          sendJson({ type: 'request_keyframe' });
        }
      }
    };

    ws.onerror = () => {
      setStatus('error');
      setErrorText(`无法连接到 ${target.ip}:${target.port}，请确认对端 TREE LINK 正在运行且局域网/直连线畅通`);
    };

    ws.onclose = () => {
      setStatus(prev => (prev === 'error' ? 'error' : 'closed'));
    };

    return () => {
      window.clearInterval(pingTimer);
      try {
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({ type: 'input', event: { kind: 'release_all' } }));
          ws.send(JSON.stringify({ type: 'stop_stream' }));
        }
        ws.close();
      } catch { /* ignore */ }
      if (wsRef.current === ws) wsRef.current = null;
      if (decoderRef.current) {
        try { decoderRef.current.close(); } catch { /* ignore */ }
        decoderRef.current = null;
      }
    };
  }, [target.ip, target.port, target.token, reconnectSeq, ensureDecoder, sendJson, subscribeStream]);

  const normalizedCoords = (clientX: number, clientY: number): { nx: number; ny: number } | null => {
    const canvas = canvasRef.current;
    if (!canvas) return null;
    const rect = canvas.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return null;
    const nx = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    const ny = Math.min(1, Math.max(0, (clientY - rect.top) / rect.height));
    return { nx, ny };
  };

  const sendInput = (event: Record<string, unknown>) => {
    sendJson({ type: 'input', event });
  };

  const sendChord = (codes: number[]) => {
    for (const c of codes) sendInput({ kind: 'key', hid_code: c, pressed: true });
    for (const c of [...codes].reverse()) sendInput({ kind: 'key', hid_code: c, pressed: false });
  };

  const toggleFullscreen = () => {
    const el = stageRef.current;
    if (!el) return;
    if (document.fullscreenElement === el) {
      void document.exitFullscreen();
    } else {
      void el.requestFullscreen?.();
    }
  };

  return (
    <section className="tl-pane" data-focused={focused ? '' : undefined} ref={stageRef}>
      <header className="tl-pane__bar">
        <div className="tl-pane__title">
          <span className="tl-dot" data-state={status} />
          <strong>{target.name}</strong>
          <span className="tl-badge" data-kind={target.linkClass}>{linkClassLabel(target.linkClass)}</span>
          {axGranted === false && (
            <Button size="sm" variant="accent" onClick={() => sendJson({ type: 'request_permissions' })}>
              开启 Mac 辅助功能以操控
            </Button>
          )}
          <span className="tl-meta">{target.ip}:{target.port}</span>
          {resolution && <span className="tl-meta">{resolution}</span>}
          {status === 'streaming' && <span className="tl-meta">{fps} fps</span>}
          {rttMs !== null && <span className="tl-meta">{rttMs} ms</span>}
        </div>
        <div className="tl-pane__controls">
          {displays.length > 1 && (
            <select
              className="tl-select"
              aria-label="选择 Mac 显示器"
              value={displayId}
              onChange={(e) => {
                const next = Number(e.target.value);
                setDisplayId(next);
                subscribeStream(next, presetId, codec);
              }}
            >
              {displays.map((d) => (
                <option key={d.displayId} value={d.displayId}>
                  {d.name}{d.isPrimary ? ' (主屏)' : ''}
                </option>
              ))}
            </select>
          )}
          <select
            className="tl-select"
            aria-label="画质与帧率"
            value={presetId}
            onChange={(e) => {
              const next = e.target.value;
              setPresetId(next);
              subscribeStream(displayId, next, codec);
            }}
          >
            {QUALITY_PRESETS.map((p) => (
              <option key={p.id} value={p.id}>{p.label}</option>
            ))}
          </select>
          <select
            className="tl-select"
            aria-label="编码格式"
            value={codec}
            onChange={(e) => {
              const next = e.target.value as VideoCodec;
              setCodec(next);
              subscribeStream(displayId, presetId, next);
            }}
          >
            <option value="h264">H.264</option>
            <option value="hevc">HEVC</option>
            <option value="jpeg">GPU JPEG</option>
          </select>
          <Button
            size="sm"
            variant="ghost"
            title="切换 Windows 键盘映射：Ctrl↔⌘ 或 Win→⌘"
            onClick={() => setModPreset(p => (p === 'ctrl_as_cmd' ? 'standard' : 'ctrl_as_cmd'))}
          >
            {modPreset === 'ctrl_as_cmd' ? 'Ctrl→⌘' : 'Win→⌘'}
          </Button>
          <Button size="sm" variant="ghost" title="向该 Mac 发送 ⌘Space (聚焦搜索)" onClick={() => sendChord([HID_LEFT_GUI, HID_SPACE])}>
            ⌘Space
          </Button>
          <Button size="sm" variant="ghost" title="向该 Mac 发送 ⌘Tab (切换应用)" onClick={() => sendChord([HID_LEFT_GUI, HID_TAB])}>
            ⌘Tab
          </Button>
          {onToggleAudioMute && (
            <Button size="sm" variant="ghost" title="静音 / 取消静音此设备音频" onClick={onToggleAudioMute}>
              {speakerMuted ? '音频已静音' : '音频开启'}
            </Button>
          )}
          <Button size="sm" variant="ghost" title="全屏查看此 Mac" onClick={toggleFullscreen}>全屏</Button>
          <Button size="sm" variant="ghost" title="重连画面" onClick={() => setReconnectSeq(s => s + 1)}>重连</Button>
          <Button size="sm" variant="ghost" title="关闭此控屏窗口" onClick={onClose}>关闭</Button>
        </div>
      </header>

      <div
        className="tl-pane__stage"
        tabIndex={0}
        onFocus={() => setFocused(true)}
        onBlur={() => {
          setFocused(false);
          sendInput({ kind: 'release_all' });
        }}
        onKeyDown={(e) => {
          if (!focused) return;
          const hidCode = domCodeToHid(e.code, modPreset);
          if (hidCode === null) return;
          e.preventDefault();
          e.stopPropagation();
          sendInput({ kind: 'key', hid_code: hidCode, pressed: true });
        }}
        onKeyUp={(e) => {
          if (!focused) return;
          const hidCode = domCodeToHid(e.code, modPreset);
          if (hidCode === null) return;
          e.preventDefault();
          e.stopPropagation();
          sendInput({ kind: 'key', hid_code: hidCode, pressed: false });
        }}
      >
        <canvas
          ref={canvasRef}
          className="tl-pane__canvas"
          onContextMenu={(e) => e.preventDefault()}
          onPointerMove={(e) => {
            const pt = normalizedCoords(e.clientX, e.clientY);
            if (!pt) return;
            sendInput({ kind: 'pointer_move', display_id: displayId, nx: pt.nx, ny: pt.ny });
          }}
          onPointerDown={(e) => {
            stageRef.current?.querySelector<HTMLElement>('.tl-pane__stage')?.focus();
            const pt = normalizedCoords(e.clientX, e.clientY);
            const button = MOUSE_BUTTON_MAP[e.button];
            if (!pt || !button) return;
            e.preventDefault();
            e.currentTarget.setPointerCapture?.(e.pointerId);
            sendInput({ kind: 'pointer_button', display_id: displayId, nx: pt.nx, ny: pt.ny, button, pressed: true });
          }}
          onPointerUp={(e) => {
            const pt = normalizedCoords(e.clientX, e.clientY);
            const button = MOUSE_BUTTON_MAP[e.button];
            if (!pt || !button) return;
            e.preventDefault();
            sendInput({ kind: 'pointer_button', display_id: displayId, nx: pt.nx, ny: pt.ny, button, pressed: false });
          }}
          onWheel={(e) => {
            const pt = normalizedCoords(e.clientX, e.clientY);
            if (!pt) return;
            e.preventDefault();
            sendInput({
              kind: 'scroll',
              display_id: displayId,
              nx: pt.nx,
              ny: pt.ny,
              dx: -e.deltaX,
              dy: -e.deltaY,
              pixel: e.deltaMode === 0,
            });
          }}
        />
        {status !== 'streaming' && (
          <div className="tl-pane__overlay">
            <p className="tl-pane__overlay-title">
              {status === 'connecting' ? `正在连接 ${target.name} (${target.ip})…` : status === 'closed' ? '连接已断开' : '连接异常'}
            </p>
            {errorText && <p className="tl-pane__overlay-msg">{errorText}</p>}
            <Button size="sm" variant="accent" onClick={() => setReconnectSeq(s => s + 1)}>重新连接</Button>
          </div>
        )}
      </div>
    </section>
  );
}

import { useCallback, useEffect, useRef, useState } from 'react';
import { Button } from 'oink-ui';
import { domCodeToHid, HID_LEFT_ALT, HID_LEFT_GUI, HID_SPACE, HID_TAB, type ModifierPreset } from './hidKeymap';
import {
  buildWebSocketUrl,
  encodeBinaryPointerButton,
  encodeBinaryPointerMove,
  encodeBinaryScroll,
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
  hidden?: boolean;
  canSoloToggle?: boolean;
  isSolo?: boolean;
  onToggleSolo?: () => void;
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

export function RemotePane({
  target,
  hidden = false,
  canSoloToggle = false,
  isSolo = false,
  onToggleSolo,
  speakerMuted,
  onToggleAudioMute,
  onClose,
}: RemotePaneProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const backupCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const decoderRef = useRef<VideoDecoder | null>(null);
  const decoderCodecRef = useRef<string>('');
  const awaitingKeyframeRef = useRef<boolean>(true);
  const hiddenDocRef = useRef<boolean>(typeof document !== 'undefined' ? document.hidden : false);
  const jpegBusyRef = useRef<boolean>(false);
  const moveTimerRef = useRef<number>(0);
  const lastMoveSentRef = useRef<number>(0);
  const pendingMoveRef = useRef<{ nx: number; ny: number } | null>(null);
  const frameSizeRef = useRef<{ w: number; h: number }>({ w: 1920, h: 1080 });
  const fpsCounterRef = useRef<{ frames: number; lastMs: number }>({ frames: 0, lastMs: performance.now() });

  const [status, setStatus] = useState<'connecting' | 'streaming' | 'error' | 'closed'>('connecting');
  const [errorText, setErrorText] = useState<string | null>(null);
  const [axGranted, setAxGranted] = useState<boolean | null>(null);
  const [displays, setDisplays] = useState<DisplayDescriptor[]>([]);
  const [displayId, setDisplayId] = useState<number>(0);
  const [remoteOs, setRemoteOs] = useState<string>('macos');
  const [supportedCodecs, setSupportedCodecs] = useState<VideoCodec[]>(['h264', 'hevc', 'jpeg']);
  const [presetId, setPresetId] = useState<string>(QUALITY_PRESETS[0].id);
  const [codec, setCodec] = useState<VideoCodec>(() => (typeof VideoDecoder !== 'undefined' ? 'h264' : 'jpeg'));
  const [showRemoteCursor, setShowRemoteCursor] = useState<boolean>(false);
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

  const subscribeStream = useCallback((
    nextDisplayId: number,
    nextPresetId: string,
    nextCodec: VideoCodec,
    nextShowCursor: boolean,
  ) => {
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
      showCursor: nextShowCursor,
    });
  }, [sendJson]);

  const drawFrameToCanvas = useCallback((source: CanvasImageSource, width: number, height: number) => {
    const canvas = canvasRef.current;
    if (canvas) {
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
        frameSizeRef.current = { w: width, h: height };
        setResolution(`${width}×${height}`);
      }
      const ctx = canvas.getContext('2d', { alpha: false });
      if (ctx) {
        ctx.drawImage(source, 0, 0, width, height);
      }
    }
    // Retain a software backup copy of the latest frame so minimizing/restoring
    // on Windows WebView2 never flashes or leaves a blank black canvas.
    let backup = backupCanvasRef.current;
    if (!backup && typeof document !== 'undefined') {
      backup = document.createElement('canvas');
      backupCanvasRef.current = backup;
    }
    if (backup) {
      if (backup.width !== width || backup.height !== height) {
        backup.width = width;
        backup.height = height;
      }
      const bctx = backup.getContext('2d', { alpha: false });
      if (bctx) {
        bctx.drawImage(source, 0, 0, width, height);
      }
    }
  }, []);

  const restoreBackupCanvas = useCallback(() => {
    const canvas = canvasRef.current;
    const backup = backupCanvasRef.current;
    if (!canvas || !backup || backup.width <= 0 || backup.height <= 0) return;
    if (canvas.width !== backup.width || canvas.height !== backup.height) {
      canvas.width = backup.width;
      canvas.height = backup.height;
    }
    const ctx = canvas.getContext('2d', { alpha: false });
    if (ctx) {
      ctx.drawImage(backup, 0, 0, backup.width, backup.height);
    }
  }, []);

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
        drawFrameToCanvas(videoFrame, videoFrame.displayWidth, videoFrame.displayHeight);
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
  }, [drawFrameToCanvas, sendJson]);

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
            const os = typeof msg.os === 'string' ? msg.os : 'macos';
            setRemoteOs(os);
            const rawCodecs = Array.isArray(msg.codecs)
              ? (msg.codecs as unknown[]).filter((c): c is VideoCodec => c === 'h264' || c === 'hevc' || c === 'jpeg')
              : [];
            const avail: VideoCodec[] = rawCodecs.length > 0 ? rawCodecs : ['h264', 'hevc', 'jpeg'];
            setSupportedCodecs(avail);
            const effectiveCodec = avail.includes(codec) ? codec : avail[0];
            if (effectiveCodec !== codec) setCodec(effectiveCodec);

            const list = (msg.displays as DisplayDescriptor[] | undefined) ?? [];
            setDisplays(list);
            const primary = list.find(d => d.isPrimary)?.displayId ?? list[0]?.displayId ?? 0;
            setDisplayId(primary);
            const perms = msg.permissions as { screenRecording?: boolean; accessibility?: boolean } | undefined;
            if (msg.host_capture_supported === false) {
              setStatus('error');
              setErrorText('对端系统当前版本暂不支持画面采集，请先将对端 TREE LINK 更新到最新版本');
              return;
            }
            if (perms && perms.screenRecording === false) {
              setStatus('error');
              setErrorText('对端尚未授予「屏幕录制」权限，请先在对端的 TREE LINK 中授权');
              return;
            }
            if (perms && typeof perms.accessibility === 'boolean') {
              setAxGranted(perms.accessibility);
            }
            subscribeStream(primary, presetId, effectiveCodec, showRemoteCursor);
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
        if (hiddenDocRef.current) {
          // While minimized/hidden on Windows WebView2, skip decoding delta frames
          // so the GPU decoder never stalls; request a fresh keyframe on restore.
          awaitingKeyframeRef.current = true;
          return;
        }
        const frame = parseVideoFrame(event.data);
        if (!frame) return;
        if (frame.codec === 'jpeg') {
          if (jpegBusyRef.current) return;
          jpegBusyRef.current = true;
          const bytes = new Uint8Array(frame.payload.byteLength);
          bytes.set(frame.payload);
          const blob = new Blob([bytes], { type: 'image/jpeg' });
          void createImageBitmap(blob).then((bmp) => {
            drawFrameToCanvas(bmp, bmp.width, bmp.height);
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
      if (moveTimerRef.current) {
        window.clearTimeout(moveTimerRef.current);
        moveTimerRef.current = 0;
      }
      pendingMoveRef.current = null;
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
  }, [target.ip, target.port, target.token, reconnectSeq, drawFrameToCanvas, ensureDecoder, sendJson, subscribeStream]);

  // Preserve the visible image and instantly resume streaming when restoring from
  // Windows minimize / hidden WebView2 suspension.
  useEffect(() => {
    let retryTimer = 0;
    const handleResume = () => {
      hiddenDocRef.current = document.hidden;
      if (document.hidden) {
        awaitingKeyframeRef.current = true;
        return;
      }
      restoreBackupCanvas();
      const ws = wsRef.current;
      if (!ws || ws.readyState === WebSocket.CLOSED || ws.readyState === WebSocket.CLOSING) {
        setReconnectSeq(s => s + 1);
        return;
      }
      if (decoderRef.current && decoderRef.current.state !== 'configured') {
        try { decoderRef.current.close(); } catch { /* ignore */ }
        decoderRef.current = null;
      }
      awaitingKeyframeRef.current = true;
      if (ws.readyState === WebSocket.OPEN) {
        sendJson({ type: 'request_keyframe' });
        window.clearTimeout(retryTimer);
        retryTimer = window.setTimeout(() => {
          if (!document.hidden && awaitingKeyframeRef.current) {
            restoreBackupCanvas();
            sendJson({ type: 'request_keyframe' });
          }
        }, 300);
      }
    };
    const handleFocus = () => {
      if (!document.hidden) {
        restoreBackupCanvas();
        if (awaitingKeyframeRef.current && wsRef.current?.readyState === WebSocket.OPEN) {
          sendJson({ type: 'request_keyframe' });
        }
      }
    };
    document.addEventListener('visibilitychange', handleResume);
    window.addEventListener('focus', handleFocus);
    window.addEventListener('pageshow', handleResume);
    return () => {
      window.clearTimeout(retryTimer);
      document.removeEventListener('visibilitychange', handleResume);
      window.removeEventListener('focus', handleFocus);
      window.removeEventListener('pageshow', handleResume);
    };
  }, [restoreBackupCanvas, sendJson]);

  // Repaint the preserved frame immediately when switching back from solo hidden mode.
  useEffect(() => {
    if (!hidden) {
      restoreBackupCanvas();
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        sendJson({ type: 'request_keyframe' });
      }
    }
  }, [hidden, restoreBackupCanvas, sendJson]);

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

  const sendBinaryMove = (nx: number, ny: number) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(encodeBinaryPointerMove(displayId, nx, ny));
      lastMoveSentRef.current = performance.now();
    }
  };

  const queuePointerMove = (nx: number, ny: number) => {
    const now = performance.now();
    const elapsed = now - lastMoveSentRef.current;
    if (elapsed >= 4) {
      if (moveTimerRef.current) {
        window.clearTimeout(moveTimerRef.current);
        moveTimerRef.current = 0;
      }
      pendingMoveRef.current = null;
      sendBinaryMove(nx, ny);
    } else {
      pendingMoveRef.current = { nx, ny };
      if (!moveTimerRef.current) {
        moveTimerRef.current = window.setTimeout(() => {
          moveTimerRef.current = 0;
          const pt = pendingMoveRef.current;
          pendingMoveRef.current = null;
          if (pt) sendBinaryMove(pt.nx, pt.ny);
        }, Math.max(1, Math.ceil(4 - elapsed)));
      }
    }
  };

  const sendBinaryButton = (nx: number, ny: number, buttonId: number, pressed: boolean) => {
    const ws = wsRef.current;
    if (moveTimerRef.current) {
      window.clearTimeout(moveTimerRef.current);
      moveTimerRef.current = 0;
    }
    pendingMoveRef.current = null;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(encodeBinaryPointerButton(displayId, nx, ny, buttonId, pressed));
      lastMoveSentRef.current = performance.now();
    }
  };

  const sendBinaryWheel = (nx: number, ny: number, dx: number, dy: number, pixel: boolean) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(encodeBinaryScroll(displayId, nx, ny, dx, dy, pixel));
    }
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
    <section
      className="tl-pane"
      data-focused={focused ? '' : undefined}
      data-hidden={hidden ? '' : undefined}
      aria-hidden={hidden || undefined}
      ref={stageRef}
    >
      <header className="tl-pane__bar">
        <div className="tl-pane__title">
          <span className="tl-dot" data-state={status} />
          <strong>{target.name}</strong>
          <span className="tl-badge" data-kind={target.linkClass}>{linkClassLabel(target.linkClass)}</span>
          {axGranted === false && (
            <Button size="sm" variant="accent" className="tl-pane__btn--warn" onClick={() => sendJson({ type: 'request_permissions' })}>
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
                subscribeStream(next, presetId, codec, showRemoteCursor);
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
              subscribeStream(displayId, next, codec, showRemoteCursor);
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
              subscribeStream(displayId, presetId, next, showRemoteCursor);
            }}
          >
            {supportedCodecs.includes('h264') && <option value="h264">H.264</option>}
            {supportedCodecs.includes('hevc') && <option value="hevc">HEVC</option>}
            {supportedCodecs.includes('jpeg') && <option value="jpeg">GPU JPEG</option>}
          </select>
          <Button
            size="sm"
            variant="ghost"
            title="隐藏视频流里的远端光标，只保留 0ms 延迟的本地鼠标指针"
            onClick={() => {
              const next = !showRemoteCursor;
              setShowRemoteCursor(next);
              subscribeStream(displayId, presetId, codec, next);
            }}
          >
            {showRemoteCursor ? (remoteOs === 'windows' ? '显示Win光标' : '显示Mac光标') : '零延迟光标'}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            title={remoteOs === 'windows' ? '切换 Mac→Win 快捷键映射：⌘↔Ctrl 或原样按键' : '切换键盘映射：Ctrl↔⌘ 或 Win→⌘'}
            onClick={() => setModPreset(p => (p === 'ctrl_as_cmd' ? 'standard' : 'ctrl_as_cmd'))}
          >
            {remoteOs === 'windows'
              ? (modPreset === 'ctrl_as_cmd' ? '⌘→Ctrl' : '原样按键')
              : (modPreset === 'ctrl_as_cmd' ? 'Ctrl→⌘' : 'Win→⌘')}
          </Button>
          {remoteOs === 'windows' ? (
            <>
              <Button size="sm" variant="ghost" title="向该 Windows 发送 Win 键（打开开始菜单）" onClick={() => sendChord([HID_LEFT_GUI])}>
                Win 键
              </Button>
              <Button size="sm" variant="ghost" title="向该 Windows 发送 Alt+Tab（切换窗口）" onClick={() => sendChord([HID_LEFT_ALT, HID_TAB])}>
                Alt+Tab
              </Button>
            </>
          ) : (
            <>
              <Button size="sm" variant="ghost" title="向该 Mac 发送 ⌘Space (聚焦搜索)" onClick={() => sendChord([HID_LEFT_GUI, HID_SPACE])}>
                ⌘Space
              </Button>
              <Button size="sm" variant="ghost" title="向该 Mac 发送 ⌘Tab (切换应用)" onClick={() => sendChord([HID_LEFT_GUI, HID_TAB])}>
                ⌘Tab
              </Button>
            </>
          )}
          {onToggleAudioMute && (
            <Button size="sm" variant="ghost" title="静音 / 取消静音此设备音频" onClick={onToggleAudioMute}>
              {speakerMuted ? '音频已静音' : '音频开启'}
            </Button>
          )}
          {canSoloToggle && onToggleSolo && (
            <Button
              size="sm"
              variant={isSolo ? 'accent' : 'ghost'}
              title={isSolo ? '返回同时控制多台电脑' : '只显示并专注控制这一台电脑'}
              onClick={onToggleSolo}
            >
              {isSolo ? '返回多机同屏' : '单机专注'}
            </Button>
          )}
          <Button size="sm" variant="ghost" title="全屏查看此 Mac" onClick={toggleFullscreen}>全屏</Button>
          <Button size="sm" variant="ghost" title="重连画面" onClick={() => setReconnectSeq(s => s + 1)}>重连</Button>
          <Button size="sm" variant="ghost" className="tl-pane__btn--danger" title="关闭此控屏窗口" onClick={onClose}>关闭</Button>
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
            queuePointerMove(pt.nx, pt.ny);
          }}
          onPointerDown={(e) => {
            stageRef.current?.querySelector<HTMLElement>('.tl-pane__stage')?.focus();
            const pt = normalizedCoords(e.clientX, e.clientY);
            const button = MOUSE_BUTTON_MAP[e.button];
            if (!pt || !button) return;
            e.preventDefault();
            e.currentTarget.setPointerCapture?.(e.pointerId);
            sendBinaryButton(pt.nx, pt.ny, e.button, true);
          }}
          onPointerUp={(e) => {
            const pt = normalizedCoords(e.clientX, e.clientY);
            const button = MOUSE_BUTTON_MAP[e.button];
            if (!pt || !button) return;
            e.preventDefault();
            sendBinaryButton(pt.nx, pt.ny, e.button, false);
          }}
          onWheel={(e) => {
            const pt = normalizedCoords(e.clientX, e.clientY);
            if (!pt) return;
            e.preventDefault();
            sendBinaryWheel(pt.nx, pt.ny, -e.deltaX, -e.deltaY, e.deltaMode === 0);
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

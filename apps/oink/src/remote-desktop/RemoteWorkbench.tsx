import { useCallback, useEffect, useState } from 'react';
import { Button, Input } from 'oink-ui';
import { api } from '../api/client';
import type { Device } from '../devices/types';
import { RemotePane, type RemoteTarget } from './RemotePane';
import { linkClassLabel, pickBestInterface, type RemoteDesktopStatus } from './protocol';
import './remoteDesktop.css';

interface RemoteWorkbenchProps {
  devices: Device[];
  pickRequest?: { deviceId: string; seq: number } | null;
  onToggleSpeakerMute: (deviceId: string) => void;
}

type LayoutMode = 'single' | 'split-h' | 'split-v';

const STORAGE_KEY = 'treelink.remoteWorkbench.v1';

interface SavedWorkbenchState {
  activeTargets: RemoteTarget[];
  layout: LayoutMode;
  soloTargetId: string | null;
}

function loadSavedWorkbench(): SavedWorkbenchState {
  const empty: SavedWorkbenchState = { activeTargets: [], layout: 'split-h', soloTargetId: null };
  if (import.meta.env.MODE === 'test' || typeof window === 'undefined') return empty;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return empty;
    const parsed = JSON.parse(raw) as Partial<SavedWorkbenchState>;
    const layout: LayoutMode = parsed.layout === 'single' || parsed.layout === 'split-v' ? parsed.layout : 'split-h';
    const activeTargets = Array.isArray(parsed.activeTargets)
      ? parsed.activeTargets.filter((t): t is RemoteTarget =>
          Boolean(t && typeof t.id === 'string' && typeof t.ip === 'string' && typeof t.port === 'number'))
      : [];
    const soloTargetId = typeof parsed.soloTargetId === 'string' ? parsed.soloTargetId : null;
    return { activeTargets, layout, soloTargetId };
  } catch {
    return empty;
  }
}

export function RemoteWorkbench({ devices, pickRequest, onToggleSpeakerMute }: RemoteWorkbenchProps) {
  const [initialSaved] = useState<SavedWorkbenchState>(() => loadSavedWorkbench());
  const [status, setStatus] = useState<RemoteDesktopStatus | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [activeTargets, setActiveTargets] = useState<RemoteTarget[]>(() => initialSaved.activeTargets);
  const [layout, setLayout] = useState<LayoutMode>(() => initialSaved.layout);
  const [soloTargetId, setSoloTargetId] = useState<string | null>(() => initialSaved.soloTargetId);
  const [showManualForm, setShowManualForm] = useState<boolean>(false);
  const [manualIp, setManualIp] = useState<string>('');
  const [manualPort, setManualPort] = useState<string>('47474');
  const [manualToken, setManualToken] = useState<string>('');
  const [manualName, setManualName] = useState<string>('');
  const [copiedUrl, setCopiedUrl] = useState<string | null>(null);

  const copyQuickUrl = (url: string) => {
    void navigator.clipboard?.writeText(url).then(() => {
      setCopiedUrl(url);
      window.setTimeout(() => setCopiedUrl(prev => (prev === url ? null : prev)), 2500);
    }).catch(() => {});
  };

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await api.remoteDesktopStatus();
      setStatus(next);
    } catch {
      // keep previous snapshot when backend is restarting
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => { void refresh(); }, 5000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const makePeerTarget = useCallback((peerId: string): RemoteTarget | null => {
    const peer = status?.peers.find(p => p.device === peerId);
    if (!peer) return null;
    const best = pickBestInterface(peer.rankedIps);
    if (!best) return null;
    return {
      id: peer.device,
      deviceId: peer.device,
      name: peer.name,
      ip: best.ip,
      port: peer.defaultPort,
      token: status?.authToken ?? '',
      linkClass: best.linkClass,
    };
  }, [status]);

  // Independently toggle a single device ON or OFF, or switch active target in single-screen mode.
  const handlePeerClick = useCallback((peerId: string) => {
    const alreadyOpen = activeTargets.some(t => t.id === peerId);
    if (layout === 'single') {
      if (!alreadyOpen) {
        const target = makePeerTarget(peerId);
        if (!target) return;
        setActiveTargets(prev => (prev.some(t => t.id === target.id) ? prev : [...prev, target]));
      }
      setSoloTargetId(peerId);
      return;
    }

    // Multi-screen mode (split-h / split-v): independently add or remove JUST this computer.
    if (alreadyOpen) {
      setActiveTargets(prev => prev.filter(t => t.id !== peerId));
      setSoloTargetId(prev => (prev === peerId ? null : prev));
    } else {
      const target = makePeerTarget(peerId);
      if (!target) return;
      setActiveTargets(prev => (prev.some(t => t.id === target.id) ? prev : [...prev, target]));
      setSoloTargetId(target.id);
    }
  }, [activeTargets, layout, makePeerTarget]);

  // Open/focus a peer when clicked from the left device sidebar.
  useEffect(() => {
    if (!pickRequest?.deviceId || !status) return;
    const target = makePeerTarget(pickRequest.deviceId);
    if (!target) return;
    setActiveTargets(prev => (prev.some(t => t.id === target.id) ? prev : [...prev, target]));
    setSoloTargetId(target.id);
  }, [pickRequest, status, makePeerTarget]);

  // Persist open panes & layout so closing to tray / restoring on Windows keeps all connected screens.
  useEffect(() => {
    if (import.meta.env.MODE === 'test' || typeof window === 'undefined') return;
    try {
      const payload: SavedWorkbenchState = { activeTargets, layout, soloTargetId };
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
    } catch {
      // ignore quota/private storage errors
    }
  }, [activeTargets, layout, soloTargetId]);

  // Refresh saved peer targets' IP/port/token when backend status updates.
  useEffect(() => {
    if (!status) return;
    setActiveTargets(prev => {
      let changed = false;
      const next = prev.map(t => {
        if (!t.deviceId) return t;
        const peer = status.peers.find(p => p.device === t.deviceId);
        if (!peer) return t;
        const best = pickBestInterface(peer.rankedIps);
        const nextIp = best?.ip ?? t.ip;
        const nextPort = peer.defaultPort;
        const nextToken = status.authToken ?? t.token;
        const nextLink = best?.linkClass ?? t.linkClass;
        if (nextIp === t.ip && nextPort === t.port && nextToken === t.token && nextLink === t.linkClass && peer.name === t.name) {
          return t;
        }
        changed = true;
        return { ...t, name: peer.name, ip: nextIp, port: nextPort, token: nextToken, linkClass: nextLink };
      });
      return changed ? next : prev;
    });
  }, [status]);

  // Ensure soloTargetId always points to an open pane when in single mode.
  useEffect(() => {
    if (activeTargets.length === 0) {
      if (soloTargetId !== null) setSoloTargetId(null);
      return;
    }
    if (!soloTargetId || !activeTargets.some(t => t.id === soloTargetId)) {
      setSoloTargetId(activeTargets[0].id);
    }
  }, [activeTargets, soloTargetId]);

  const openManual = () => {
    const ip = manualIp.trim();
    if (!ip) return;
    const port = Number(manualPort.trim()) || 47474;
    const id = `manual:${ip}:${port}`;
    const isDirect = ip.startsWith('169.254.');
    setActiveTargets(prev => {
      if (prev.some(t => t.id === id)) return prev;
      return [...prev, {
        id,
        name: manualName.trim() || `Mac (${ip})`,
        ip,
        port,
        token: manualToken.trim() || (status?.authToken ?? ''),
        linkClass: isDirect ? 'direct_cable' : 'wired_lan',
      }];
    });
    setSoloTargetId(id);
    setManualIp('');
    setManualName('');
    setShowManualForm(false);
  };

  const closeTarget = (id: string) => {
    setActiveTargets(prev => {
      const next = prev.filter(t => t.id !== id);
      if (soloTargetId === id) {
        setSoloTargetId(next[0]?.id ?? null);
      }
      return next;
    });
  };

  const toggleSoloPane = (id: string) => {
    if (layout === 'single' && soloTargetId === id) {
      setLayout('split-h');
    } else {
      setSoloTargetId(id);
      setLayout('single');
    }
  };

  const localHost = status?.localHost;
  const visibleCount = layout === 'single' ? Math.min(1, activeTargets.length) : activeTargets.length;
  const manualOnlyTargets = activeTargets.filter(t => !status?.peers.some(p => p.device === t.id));

  return (
    <div className="tl-workbench">
      <header className="tl-workbench__head">
        <div className="tl-workbench__brand">
          <span className="tl-workbench__logo">TREE LINK</span>
          <span className="tl-workbench__sub">
            {visibleCount === 0
              ? '点选任意一台开始单机控屏，或同时点选多台并排控屏'
              : layout === 'single'
                ? `单机专注控屏（已连接 ${activeTargets.length} 台，可随时切换单台或多台同屏）`
                : `多机同屏控屏（当前同时控制 ${activeTargets.length} 台）`}
          </span>
        </div>
        <div className="tl-workbench__actions">
          <div className="tl-seg" role="group" aria-label="控屏模式">
            <Button
              size="sm"
              variant={layout === 'single' ? 'accent' : 'ghost'}
              onClick={() => {
                setLayout('single');
                if (!soloTargetId && activeTargets[0]) setSoloTargetId(activeTargets[0].id);
              }}
            >
              单机控屏
            </Button>
            <Button
              size="sm"
              variant={layout === 'split-h' ? 'accent' : 'ghost'}
              onClick={() => setLayout('split-h')}
            >
              多机左右同屏
            </Button>
            <Button
              size="sm"
              variant={layout === 'split-v' ? 'accent' : 'ghost'}
              onClick={() => setLayout('split-v')}
            >
              多机上下同屏
            </Button>
          </div>
          <Button
            size="sm"
            variant={showManualForm ? 'accent' : 'ghost'}
            onClick={() => setShowManualForm(v => !v)}
          >
            + 手动连 IP
          </Button>
          <Button size="sm" variant="ghost" onClick={() => { void refresh(); }}>刷新</Button>
        </div>
      </header>

      <div className="tl-workbench__strip">
        <div className="tl-strip__group">
          <span className="tl-strip__label">
            {layout === 'single' ? '点击切换要控制的电脑：' : '独立勾选要同屏控制的电脑：'}
          </span>
          {loading && !status && <span className="tl-meta">正在读取…</span>}
          {status && status.peers.length === 0 && manualOnlyTargets.length === 0 && (
            <span className="tl-meta">暂无已配对设备，可在左侧「+ 添加设备」配对，或点右上角「+ 手动连 IP」直接输入目标 IP。</span>
          )}
          {status?.peers.map((peer) => {
            const best = pickBestInterface(peer.rankedIps);
            const opened = activeTargets.some(t => t.id === peer.device);
            const isCurrent = layout === 'single' ? soloTargetId === peer.device && opened : opened;
            return (
              <div key={peer.device} className="tl-peer-chip">
                <Button
                  size="sm"
                  variant={isCurrent ? 'accent' : 'ghost'}
                  disabled={!best}
                  title={
                    !best
                      ? '该设备当前没有可用 IP'
                      : layout === 'single'
                        ? `切换到单机控制 ${peer.name}`
                        : opened
                          ? `从同屏视图移除 ${peer.name}`
                          : `将 ${peer.name} 加入控屏`
                  }
                  onClick={() => handlePeerClick(peer.device)}
                >
                  {isCurrent ? '● ' : opened ? '○ ' : '+ '}
                  {peer.name}
                  {best ? ` · ${linkClassLabel(best.linkClass)} (${best.ip})` : ' · 暂无 IP'}
                </Button>
              </div>
            );
          })}
          {manualOnlyTargets.map((target) => {
            const isCurrent = layout === 'single' ? soloTargetId === target.id : true;
            return (
              <div key={target.id} className="tl-peer-chip">
                <Button
                  size="sm"
                  variant={isCurrent ? 'accent' : 'ghost'}
                  onClick={() => {
                    if (layout === 'single') setSoloTargetId(target.id);
                    else closeTarget(target.id);
                  }}
                >
                  {isCurrent ? '● ' : '○ '}
                  {target.name} ({target.ip})
                </Button>
              </div>
            );
          })}
        </div>

        {(showManualForm || (status && status.peers.length === 0 && activeTargets.length === 0)) && (
          <div className="tl-strip__manual">
            <Input value={manualName} onChange={setManualName} placeholder="备注名 (如 Mac-1)" />
            <Input value={manualIp} onChange={setManualIp} placeholder="直连/局域网 IP (如 192.168.50.67)" />
            <Input value={manualPort} onChange={setManualPort} placeholder="47474" />
            <Input value={manualToken} onChange={setManualToken} placeholder="令牌(同组可留空)" />
            <Button size="sm" variant="accent" onClick={openManual} disabled={!manualIp.trim()}>连接此台电脑</Button>
          </div>
        )}
      </div>

      {localHost?.hostCaptureSupported && !localHost.permissions.screenRecording && (
        <div className="tl-perm-banner" role="alert">
          <span>本机 macOS 尚未授予「屏幕录制」权限，授予后 Windows 端即可实时查看并操控本机桌面。</span>
          <Button size="sm" variant="accent" onClick={() => { void api.requestScreenRecordingPermission().then(refresh); }}>
            授予屏幕录制权限
          </Button>
        </div>
      )}

      {activeTargets.length === 0 ? (
        <div className="tl-workbench__empty">
          <h3>自由控制 1 台，或同时并排控制多台电脑</h3>
          <p>
            点上方任意一台已配对电脑即可单独操控；再点第二台即可并排同时控制两台，随时可一键切换「单机控屏」或「多机同屏」（完整保留文件互传、剪贴板与低延迟音频）。
          </p>
          {status && status.localInterfaces.length > 0 && (
            <div className="tl-local-info">
              <strong>本机控屏服务地址（端口 {localHost?.port ?? 47474} · 60fps HTTPS 端口 {(localHost?.port ?? 47474) + 1}）：</strong>
              <div className="tl-local-chips">
                {status.localInterfaces.map((iface) => {
                  const httpsPort = (localHost?.port ?? 47474) + 1;
                  const tokenQs = status.authToken ? `?token=${encodeURIComponent(status.authToken)}` : '';
                  const quickUrl = `https://${iface.ip}:${httpsPort}/${tokenQs}`;
                  return (
                    <span key={`${iface.interface}:${iface.ip}`} className="tl-chip">
                      <span className="tl-badge" data-kind={iface.linkClass}>{linkClassLabel(iface.linkClass)}</span>
                      <code>{iface.ip}:{localHost?.port ?? 47474}</code>
                      <span className="tl-meta">{iface.interface}</span>
                      <Button size="sm" variant="ghost" onClick={() => copyQuickUrl(quickUrl)}>
                        {copiedUrl === quickUrl ? '已复制 60fps 链接' : '复制浏览器 60fps 链接'}
                      </Button>
                    </span>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      ) : (
        <div className="tl-grid" data-layout={layout} data-count={visibleCount}>
          {activeTargets.map((target) => {
            const dev = target.deviceId ? devices.find(d => d.id === target.deviceId) : undefined;
            const hidden = layout === 'single' && soloTargetId !== null && target.id !== soloTargetId;
            return (
              <RemotePane
                key={target.id}
                target={target}
                hidden={hidden}
                canSoloToggle={activeTargets.length > 1}
                isSolo={layout === 'single' && target.id === soloTargetId}
                onToggleSolo={() => toggleSoloPane(target.id)}
                speakerMuted={dev?.speaker.muted}
                onToggleAudioMute={dev ? () => onToggleSpeakerMute(dev.id) : undefined}
                onClose={() => closeTarget(target.id)}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

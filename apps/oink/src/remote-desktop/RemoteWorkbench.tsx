import { useCallback, useEffect, useState } from 'react';
import { Button, Input } from 'oink-ui';
import { api } from '../api/client';
import type { Device } from '../devices/types';
import { RemotePane, type RemoteTarget } from './RemotePane';
import { linkClassLabel, pickBestInterface, type RemoteDesktopStatus } from './protocol';
import './remoteDesktop.css';

interface RemoteWorkbenchProps {
  devices: Device[];
  onToggleSpeakerMute: (deviceId: string) => void;
}

type LayoutMode = 'split-h' | 'split-v' | 'single';

export function RemoteWorkbench({ devices, onToggleSpeakerMute }: RemoteWorkbenchProps) {
  const [status, setStatus] = useState<RemoteDesktopStatus | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [activeTargets, setActiveTargets] = useState<RemoteTarget[]>([]);
  const [layout, setLayout] = useState<LayoutMode>('split-h');
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

  const openPeer = (peerId: string) => {
    const peer = status?.peers.find(p => p.device === peerId);
    if (!peer) return;
    const best = pickBestInterface(peer.rankedIps);
    if (!best) return;
    setActiveTargets(prev => {
      if (prev.some(t => t.id === peer.device)) return prev;
      return [...prev, {
        id: peer.device,
        deviceId: peer.device,
        name: peer.name,
        ip: best.ip,
        port: peer.defaultPort,
        token: status?.authToken ?? '',
        linkClass: best.linkClass,
      }];
    });
  };

  const openAllMacs = () => {
    if (!status) return;
    const macs = status.peers.filter(p => p.os === 'macos' && p.rankedIps.length > 0);
    setActiveTargets(prev => {
      const next = [...prev];
      for (const peer of macs) {
        if (next.some(t => t.id === peer.device)) continue;
        const best = pickBestInterface(peer.rankedIps);
        if (!best) continue;
        next.push({
          id: peer.device,
          deviceId: peer.device,
          name: peer.name,
          ip: best.ip,
          port: peer.defaultPort,
          token: status.authToken,
          linkClass: best.linkClass,
        });
      }
      return next;
    });
  };

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
  };

  const localHost = status?.localHost;
  const pairedMacs = (status?.peers ?? []).filter(p => p.os === 'macos');

  return (
    <div className="tl-workbench">
      <header className="tl-workbench__head">
        <div className="tl-workbench__brand">
          <span className="tl-workbench__logo">TREE LINK</span>
          <span className="tl-workbench__sub">多机实时桌面掌控工作台（支持双 Mac 并排操控 · 保留文件互传 / 剪贴板 / 音频）</span>
        </div>
        <div className="tl-workbench__actions">
          {pairedMacs.length > 0 && (
            <Button size="sm" variant="accent" onClick={openAllMacs}>
              一键打开全部 Mac ({pairedMacs.length})
            </Button>
          )}
          <div className="tl-seg" role="group" aria-label="分屏布局">
            <Button size="sm" variant={layout === 'split-h' ? 'accent' : 'ghost'} onClick={() => setLayout('split-h')}>左右分屏</Button>
            <Button size="sm" variant={layout === 'split-v' ? 'accent' : 'ghost'} onClick={() => setLayout('split-v')}>上下分屏</Button>
            <Button size="sm" variant={layout === 'single' ? 'accent' : 'ghost'} onClick={() => setLayout('single')}>单屏聚焦</Button>
          </div>
          <Button size="sm" variant="ghost" onClick={() => { void refresh(); }}>刷新设备</Button>
        </div>
      </header>

      <div className="tl-workbench__strip">
        <div className="tl-strip__group">
          <span className="tl-strip__label">已配对设备：</span>
          {loading && !status && <span className="tl-meta">正在读取…</span>}
          {status && status.peers.length === 0 && (
            <span className="tl-meta">暂无已配对设备，请先在左侧「+ 添加设备」完成配对，或在右侧直接输入 IP 连接。</span>
          )}
          {status?.peers.map((peer) => {
            const best = pickBestInterface(peer.rankedIps);
            const opened = activeTargets.some(t => t.id === peer.device);
            return (
              <Button
                key={peer.device}
                size="sm"
                variant={opened ? 'accent' : 'ghost'}
                disabled={!best}
                onClick={() => openPeer(peer.device)}
              >
                {peer.name}
                {best ? ` · ${linkClassLabel(best.linkClass)} (${best.ip})` : ' · 暂无 IP'}
              </Button>
            );
          })}
        </div>

        <div className="tl-strip__manual">
          <Input value={manualName} onChange={setManualName} placeholder="备注名 (如 MacBook)" />
          <Input value={manualIp} onChange={setManualIp} placeholder="直连/局域网 IP (如 169.254.x.x)" />
          <Input value={manualPort} onChange={setManualPort} placeholder="47474" />
          <Input value={manualToken} onChange={setManualToken} placeholder="令牌(同组可留空)" />
          <Button size="sm" variant="accent" onClick={openManual} disabled={!manualIp.trim()}>IP 立即控屏</Button>
        </div>
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
          <h3>在一个 Windows 显示器上同时掌控你的两台 Mac</h3>
          <p>
            点击上方已配对的 Mac 按钮，或直接输入局域网 / 网线・雷电直连 IP（自动优先 <code>169.254.x.x</code> 直连与有线 LAN）即可实时开启硬件加速 60fps 桌面画面。
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
        <div className="tl-grid" data-layout={layout} data-count={activeTargets.length}>
          {activeTargets.map((target) => {
            const dev = target.deviceId ? devices.find(d => d.id === target.deviceId) : undefined;
            return (
              <RemotePane
                key={target.id}
                target={target}
                speakerMuted={dev?.speaker.muted}
                onToggleAudioMute={dev ? () => onToggleSpeakerMute(dev.id) : undefined}
                onClose={() => setActiveTargets(prev => prev.filter(t => t.id !== target.id))}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RemoteWorkbench } from './RemoteWorkbench';
import type { RemoteDesktopStatus } from './protocol';

const remoteDesktopStatus = vi.fn<() => Promise<RemoteDesktopStatus>>();

vi.mock('../api/client', () => ({
  api: {
    remoteDesktopStatus: () => remoteDesktopStatus(),
    requestScreenRecordingPermission: vi.fn().mockResolvedValue(true),
  },
}));

class FakeWebSocket {
  static OPEN = 1;
  readyState = 1;
  binaryType = 'arraybuffer';
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  send = vi.fn();
  close = vi.fn();
}

const twoMacStatus: RemoteDesktopStatus = {
  localHost: {
    running: true,
    port: 47474,
    hostCaptureSupported: false,
    activeViewers: 0,
    permissions: { screenRecording: true, accessibility: true },
  },
  authToken: 'test-token',
  localInterfaces: [
    { ip: '192.168.50.10', interface: 'Ethernet', linkClass: 'wired_lan', priority: 2 },
  ],
  peers: [
    {
      device: 'mac-1',
      name: 'MacBook Pro',
      os: 'macos',
      online: true,
      defaultPort: 47474,
      rankedIps: [{ ip: '192.168.50.67', interface: 'en0', linkClass: 'wifi_lan', priority: 3 }],
    },
    {
      device: 'mac-2',
      name: 'Mac mini',
      os: 'macos',
      online: true,
      defaultPort: 47474,
      rankedIps: [{ ip: '169.254.10.2', interface: 'bridge0', linkClass: 'direct_cable', priority: 1 }],
    },
  ],
};

describe('RemoteWorkbench single & multi-device control', () => {
  beforeEach(() => {
    remoteDesktopStatus.mockResolvedValue(twoMacStatus);
    vi.stubGlobal('WebSocket', FakeWebSocket);
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it('controls 1 Mac individually, then adds a 2nd Mac side-by-side, and switches cleanly to single-focus mode', async () => {
    const { container } = render(<RemoteWorkbench devices={[]} onToggleSpeakerMute={vi.fn()} />);

    await waitFor(() => {
      expect(screen.getByText(/MacBook Pro/)).toBeTruthy();
      expect(screen.getByText(/Mac mini/)).toBeTruthy();
    });

    // No forced "open all at once" button.
    expect(screen.queryByText(/一键打开全部/)).toBeNull();

    // 1. Click ONLY MacBook Pro -> only 1 pane opens.
    fireEvent.click(screen.getByRole('button', { name: /MacBook Pro/ }));
    expect(container.querySelectorAll('.tl-pane')).toHaveLength(1);
    expect(container.querySelectorAll('.tl-pane:not([data-hidden])')).toHaveLength(1);

    // 2. Click Mac mini in multi-screen mode -> BOTH Macs open side-by-side.
    fireEvent.click(screen.getByRole('button', { name: /Mac mini/ }));
    expect(container.querySelectorAll('.tl-pane')).toHaveLength(2);
    expect(container.querySelectorAll('.tl-pane:not([data-hidden])')).toHaveLength(2);

    // 3. Switch to 单机控屏 -> only 1 pane visible; clicking MacBook Pro switches active solo screen.
    fireEvent.click(screen.getByRole('button', { name: '单机控屏' }));
    expect(container.querySelectorAll('.tl-pane:not([data-hidden])')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: /MacBook Pro/ }));
    const visiblePane = container.querySelector('.tl-pane:not([data-hidden])');
    expect(visiblePane?.textContent).toContain('MacBook Pro');

    // 4. Switch back to 多机左右同屏 -> both panes visible simultaneously again.
    fireEvent.click(screen.getByRole('button', { name: '多机左右同屏' }));
    expect(container.querySelectorAll('.tl-pane:not([data-hidden])')).toHaveLength(2);

    // 5. Clicking Mac mini again in multi-screen mode removes ONLY Mac mini, leaving MacBook Pro active.
    fireEvent.click(screen.getByRole('button', { name: /Mac mini/ }));
    expect(container.querySelectorAll('.tl-pane')).toHaveLength(1);
    expect(container.querySelector('.tl-pane')?.textContent).toContain('MacBook Pro');
  });
});

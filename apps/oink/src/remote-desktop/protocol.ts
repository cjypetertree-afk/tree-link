export type VideoCodec = 'h264' | 'hevc' | 'jpeg';
export type LinkClass = 'direct_cable' | 'wired_lan' | 'wifi_lan' | 'other';
export type MouseButtonKind = 'left' | 'right' | 'middle' | 'back' | 'forward';

export interface LinkInterfaceInfo {
  ip: string;
  interface: string;
  linkClass: LinkClass;
  priority: number;
}

export interface PermissionSummary {
  screenRecording: boolean;
  accessibility: boolean;
}

export interface HostStatus {
  running: boolean;
  port: number;
  hostCaptureSupported: boolean;
  activeViewers: number;
  permissions: PermissionSummary;
}

export interface RemoteDesktopPeer {
  device: string;
  name: string;
  os: 'macos' | 'windows' | 'linux' | 'other';
  online: boolean;
  defaultPort: number;
  rankedIps: LinkInterfaceInfo[];
}

export interface RemoteDesktopStatus {
  localHost: HostStatus;
  authToken: string;
  localInterfaces: LinkInterfaceInfo[];
  peers: RemoteDesktopPeer[];
}

export interface DisplayDescriptor {
  displayId: number;
  name: string;
  originX: number;
  originY: number;
  logicalWidth: number;
  logicalHeight: number;
  pixelWidth: number;
  pixelHeight: number;
  scaleFactor: number;
  isPrimary: boolean;
}

export interface StreamConfig {
  displayId: number;
  maxWidth: number;
  maxHeight: number;
  fps: number;
  bitrateKbps: number;
  codec: VideoCodec;
  showCursor: boolean;
}

export interface ParsedVideoFrame {
  displayId: number;
  width: number;
  height: number;
  codec: VideoCodec;
  isKeyframe: boolean;
  timestampUs: number;
  payload: Uint8Array;
}

export const VIDEO_FRAME_MAGIC = 0x54;
export const VIDEO_HEADER_LEN = 20;
export const INPUT_FRAME_MAGIC = 0x49;

export function encodeBinaryPointerMove(displayId: number, nx: number, ny: number): ArrayBuffer {
  const buf = new ArrayBuffer(14);
  const dv = new DataView(buf);
  dv.setUint8(0, INPUT_FRAME_MAGIC);
  dv.setUint8(1, 0x01);
  dv.setUint32(2, displayId >>> 0, false);
  dv.setFloat32(6, nx, false);
  dv.setFloat32(10, ny, false);
  return buf;
}

export function encodeBinaryPointerButton(
  displayId: number,
  nx: number,
  ny: number,
  buttonId: number,
  pressed: boolean,
): ArrayBuffer {
  const buf = new ArrayBuffer(16);
  const dv = new DataView(buf);
  dv.setUint8(0, INPUT_FRAME_MAGIC);
  dv.setUint8(1, 0x02);
  dv.setUint8(2, buttonId & 0xff);
  dv.setUint8(3, pressed ? 1 : 0);
  dv.setUint32(4, displayId >>> 0, false);
  dv.setFloat32(8, nx, false);
  dv.setFloat32(12, ny, false);
  return buf;
}

export function encodeBinaryScroll(
  displayId: number,
  nx: number,
  ny: number,
  dx: number,
  dy: number,
  pixel: boolean,
): ArrayBuffer {
  const buf = new ArrayBuffer(24);
  const dv = new DataView(buf);
  dv.setUint8(0, INPUT_FRAME_MAGIC);
  dv.setUint8(1, 0x03);
  dv.setUint8(2, pixel ? 1 : 0);
  dv.setUint8(3, 0);
  dv.setUint32(4, displayId >>> 0, false);
  dv.setFloat32(8, nx, false);
  dv.setFloat32(12, ny, false);
  dv.setFloat32(16, dx, false);
  dv.setFloat32(20, dy, false);
  return buf;
}

export function parseVideoFrame(buffer: ArrayBuffer): ParsedVideoFrame | null {
  if (buffer.byteLength < VIDEO_HEADER_LEN) return null;
  const view = new DataView(buffer);
  if (view.getUint8(0) !== VIDEO_FRAME_MAGIC) return null;
  const flags = view.getUint8(1);
  const codecId = view.getUint8(2);
  const codec: VideoCodec | null = codecId === 1 ? 'h264' : codecId === 2 ? 'hevc' : codecId === 3 ? 'jpeg' : null;
  if (!codec) return null;
  const displayId = view.getUint32(4, false);
  const width = view.getUint16(8, false);
  const height = view.getUint16(10, false);
  const hi = view.getUint32(12, false);
  const lo = view.getUint32(16, false);
  const timestampUs = hi * 4294967296 + lo;
  return {
    displayId,
    width,
    height,
    codec,
    isKeyframe: (flags & 1) !== 0,
    timestampUs,
    payload: new Uint8Array(buffer, VIDEO_HEADER_LEN),
  };
}

export function buildWebSocketUrl(ip: string, port: number, token: string): string {
  const qs = token ? `?token=${encodeURIComponent(token)}` : '';
  return `ws://${ip}:${port}/ws${qs}`;
}

export function linkClassLabel(linkClass: LinkClass): string {
  switch (linkClass) {
    case 'direct_cable': return '直连线缆';
    case 'wired_lan': return '有线 LAN';
    case 'wifi_lan': return '局域网';
    default: return 'IP 直连';
  }
}

export function pickBestInterface(ips: LinkInterfaceInfo[]): LinkInterfaceInfo | null {
  if (!ips.length) return null;
  return [...ips].sort((a, b) => a.priority - b.priority)[0] ?? null;
}

export interface QualityPreset {
  id: string;
  label: string;
  maxWidth: number;
  maxHeight: number;
  fps: number;
  bitrateKbps: number;
}

export const QUALITY_PRESETS: QualityPreset[] = [
  { id: 'code-2k', label: '清晰开发 2K·60fps', maxWidth: 2560, maxHeight: 1600, fps: 60, bitrateKbps: 20000 },
  { id: 'smooth-1080', label: '流畅 1080p·60fps', maxWidth: 1920, maxHeight: 1200, fps: 60, bitrateKbps: 12000 },
  { id: 'retina-4k', label: '原画 4K·60fps', maxWidth: 3840, maxHeight: 2400, fps: 60, bitrateKbps: 35000 },
  { id: 'eco-720', label: '省流 720p·30fps', maxWidth: 1280, maxHeight: 800, fps: 30, bitrateKbps: 5000 },
];

import { describe, expect, it } from 'vitest';
import { domCodeToHid, HID_LEFT_CTRL, HID_LEFT_GUI } from './hidKeymap';
import { buildWebSocketUrl, parseVideoFrame, pickBestInterface, VIDEO_FRAME_MAGIC, VIDEO_HEADER_LEN } from './protocol';

describe('TREE LINK remote desktop helpers', () => {
  it('maps DOM keys to HID usages and supports Ctrl<->Cmd swap for Windows->Mac coding', () => {
    expect(domCodeToHid('KeyA', 'standard')).toBe((0x07 << 16) | 0x04);
    expect(domCodeToHid('ControlLeft', 'standard')).toBe(HID_LEFT_CTRL);
    expect(domCodeToHid('ControlLeft', 'ctrl_as_cmd')).toBe(HID_LEFT_GUI);
    expect(domCodeToHid('MetaLeft', 'ctrl_as_cmd')).toBe(HID_LEFT_CTRL);
    expect(domCodeToHid('UnknownKey')).toBeNull();
  });

  it('parses binary Annex-B video frame headers accurately', () => {
    const buf = new ArrayBuffer(VIDEO_HEADER_LEN + 4);
    const dv = new DataView(buf);
    dv.setUint8(0, VIDEO_FRAME_MAGIC);
    dv.setUint8(1, 1);
    dv.setUint8(2, 1);
    dv.setUint32(4, 7, false);
    dv.setUint16(8, 2560, false);
    dv.setUint16(10, 1440, false);
    dv.setUint32(12, 0, false);
    dv.setUint32(16, 42000, false);
    new Uint8Array(buf, VIDEO_HEADER_LEN).set([0, 0, 0, 1]);

    const frame = parseVideoFrame(buf);
    expect(frame).not.toBeNull();
    expect(frame?.displayId).toBe(7);
    expect(frame?.width).toBe(2560);
    expect(frame?.height).toBe(1440);
    expect(frame?.codec).toBe('h264');
    expect(frame?.isKeyframe).toBe(true);
    expect(frame?.timestampUs).toBe(42000);
    expect(Array.from(frame!.payload)).toEqual([0, 0, 0, 1]);
  });

  it('prefers direct-cable links over wired and wifi interfaces', () => {
    const best = pickBestInterface([
      { ip: '192.168.1.50', interface: 'Wi-Fi', linkClass: 'wifi_lan', priority: 3 },
      { ip: '169.254.10.2', interface: 'bridge0', linkClass: 'direct_cable', priority: 1 },
      { ip: '192.168.1.20', interface: 'en5', linkClass: 'wired_lan', priority: 2 },
    ]);
    expect(best?.ip).toBe('169.254.10.2');
    expect(buildWebSocketUrl('169.254.10.2', 47474, 'abc')).toBe('ws://169.254.10.2:47474/ws?token=abc');
  });
});

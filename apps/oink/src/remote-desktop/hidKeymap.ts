/**
 * DOM KeyboardEvent.code -> USB HID Usage (page << 16 | usage_id),
 * matching `oink_foundation::input::KeyCode` and `oink_input::mac_keymap`.
 */

const PAGE_KEYBOARD = 0x07;
const hid = (usage: number): number => ((PAGE_KEYBOARD << 16) | usage) >>> 0;

export const HID_LEFT_CTRL = hid(0xe0);
export const HID_LEFT_SHIFT = hid(0xe1);
export const HID_LEFT_ALT = hid(0xe2);
export const HID_LEFT_GUI = hid(0xe3);
export const HID_RIGHT_CTRL = hid(0xe4);
export const HID_RIGHT_SHIFT = hid(0xe5);
export const HID_RIGHT_ALT = hid(0xe6);
export const HID_RIGHT_GUI = hid(0xe7);
export const HID_SPACE = hid(0x2c);
export const HID_TAB = hid(0x2b);

export type ModifierPreset = 'standard' | 'ctrl_as_cmd';

const CODE_TO_HID: Readonly<Record<string, number>> = {
  KeyA: hid(0x04), KeyB: hid(0x05), KeyC: hid(0x06), KeyD: hid(0x07),
  KeyE: hid(0x08), KeyF: hid(0x09), KeyG: hid(0x0a), KeyH: hid(0x0b),
  KeyI: hid(0x0c), KeyJ: hid(0x0d), KeyK: hid(0x0e), KeyL: hid(0x0f),
  KeyM: hid(0x10), KeyN: hid(0x11), KeyO: hid(0x12), KeyP: hid(0x13),
  KeyQ: hid(0x14), KeyR: hid(0x15), KeyS: hid(0x16), KeyT: hid(0x17),
  KeyU: hid(0x18), KeyV: hid(0x19), KeyW: hid(0x1a), KeyX: hid(0x1b),
  KeyY: hid(0x1c), KeyZ: hid(0x1d),

  Digit1: hid(0x1e), Digit2: hid(0x1f), Digit3: hid(0x20), Digit4: hid(0x21),
  Digit5: hid(0x22), Digit6: hid(0x23), Digit7: hid(0x24), Digit8: hid(0x25),
  Digit9: hid(0x26), Digit0: hid(0x27),

  Enter: hid(0x28),
  Escape: hid(0x29),
  Backspace: hid(0x2a),
  Tab: hid(0x2b),
  Space: hid(0x2c),
  Minus: hid(0x2d),
  Equal: hid(0x2e),
  BracketLeft: hid(0x2f),
  BracketRight: hid(0x30),
  Backslash: hid(0x31),
  Semicolon: hid(0x33),
  Quote: hid(0x34),
  Backquote: hid(0x35),
  Comma: hid(0x36),
  Period: hid(0x37),
  Slash: hid(0x38),
  CapsLock: hid(0x39),

  F1: hid(0x3a), F2: hid(0x3b), F3: hid(0x3c), F4: hid(0x3d),
  F5: hid(0x3e), F6: hid(0x3f), F7: hid(0x40), F8: hid(0x41),
  F9: hid(0x42), F10: hid(0x43), F11: hid(0x44), F12: hid(0x45),

  Insert: hid(0x49),
  Home: hid(0x4a),
  PageUp: hid(0x4b),
  Delete: hid(0x4c),
  End: hid(0x4d),
  PageDown: hid(0x4e),
  ArrowRight: hid(0x4f),
  ArrowLeft: hid(0x50),
  ArrowDown: hid(0x51),
  ArrowUp: hid(0x52),

  NumpadDivide: hid(0x54),
  NumpadMultiply: hid(0x55),
  NumpadSubtract: hid(0x56),
  NumpadAdd: hid(0x57),
  NumpadEnter: hid(0x58),
  Numpad1: hid(0x59), Numpad2: hid(0x5a), Numpad3: hid(0x5b),
  Numpad4: hid(0x5c), Numpad5: hid(0x5d), Numpad6: hid(0x5e),
  Numpad7: hid(0x5f), Numpad8: hid(0x60), Numpad9: hid(0x61),
  Numpad0: hid(0x62),
  NumpadDecimal: hid(0x63),

  ControlLeft: HID_LEFT_CTRL,
  ShiftLeft: HID_LEFT_SHIFT,
  AltLeft: HID_LEFT_ALT,
  MetaLeft: HID_LEFT_GUI,
  ControlRight: HID_RIGHT_CTRL,
  ShiftRight: HID_RIGHT_SHIFT,
  AltRight: HID_RIGHT_ALT,
  MetaRight: HID_RIGHT_GUI,
};

/**
 * Translate a DOM `KeyboardEvent.code` to a USB HID keycode.
 * When `preset === 'ctrl_as_cmd'`, Left/Right Ctrl swaps with Left/Right Meta (Win <-> Cmd),
 * letting Windows users press Ctrl+C/V/S/Space naturally against macOS apps.
 */
export function domCodeToHid(code: string, preset: ModifierPreset = 'standard'): number | null {
  const raw = CODE_TO_HID[code];
  if (raw === undefined) return null;
  if (preset === 'ctrl_as_cmd') {
    if (raw === HID_LEFT_CTRL) return HID_LEFT_GUI;
    if (raw === HID_RIGHT_CTRL) return HID_RIGHT_GUI;
    if (raw === HID_LEFT_GUI) return HID_LEFT_CTRL;
    if (raw === HID_RIGHT_GUI) return HID_RIGHT_CTRL;
  }
  return raw;
}

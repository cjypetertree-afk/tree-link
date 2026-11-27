//! macOS direct-window input injection for TREE LINK remote desktop sessions.
//! Maps normalized `(nx, ny)` coordinates on any active display to exact Quartz global coordinates
//! and reuses `oink_input::mac_keymap` for native macOS key translation.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventType, CGMainDisplayID, CGMouseButton, CGScrollEventUnit,
};
use oink_foundation::input::KeyCode;
use oink_input::mac_keymap::{self, MacKeyCode};

use crate::protocol::{MouseButtonKind, RemoteInputEvent};

/// Match Oink's injection source tag (`"OINK"`) so Oink's capture tap never feeds
/// remote-window injected input back into the local KVM router.
const INJECT_MAGIC: i64 = 0x4F49_4E4B;
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(450);
const DOUBLE_CLICK_DIST: f64 = 5.0;

pub struct InputInjector {
    source: Option<CFRetained<CGEventSource>>,
    pressed_buttons: u8,
    pressed_keys: HashSet<MacKeyCode>,
    pressed_mods: HashSet<MacKeyCode>,
    last_clicks: HashMap<u8, (Instant, CGPoint, i64)>,
    last_point: CGPoint,
}

impl std::fmt::Debug for InputInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputInjector")
            .field("pressed_buttons", &self.pressed_buttons)
            .field("pressed_keys_count", &self.pressed_keys.len())
            .finish_non_exhaustive()
    }
}

// SAFETY: `CGEventSource` is immutable after creation and thread-safe in CoreGraphics; all mutable
// fields are standard Rust collections accessed exclusively through `&mut self`.
unsafe impl Send for InputInjector {}

impl Default for InputInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl InputInjector {
    pub fn new() -> Self {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState);
        if let Some(src) = &source {
            CGEventSource::set_local_events_suppression_interval(Some(src), 0.0);
        }
        Self {
            source,
            pressed_buttons: 0,
            pressed_keys: HashSet::new(),
            pressed_mods: HashSet::new(),
            last_clicks: HashMap::new(),
            last_point: CGPoint::new(0.0, 0.0),
        }
    }

    fn map_point(&mut self, display_id: u32, nx: f64, ny: f64) -> CGPoint {
        let did = if display_id == 0 { CGMainDisplayID() } else { display_id };
        let bounds = CGDisplayBounds(did);
        let cx = nx.clamp(0.0, 1.0);
        let cy = ny.clamp(0.0, 1.0);
        let max_x = (bounds.size.width - 1.0).max(0.0);
        let max_y = (bounds.size.height - 1.0).max(0.0);
        let x = bounds.origin.x + cx * max_x;
        let y = bounds.origin.y + cy * max_y;
        let p = CGPoint::new(x, y);
        self.last_point = p;
        p
    }

    fn current_flags(&self) -> CGEventFlags {
        let mut raw = 0u64;
        for &mac in &self.pressed_mods {
            match mac {
                0x3B | 0x3E => raw |= CGEventFlags::MaskControl.0,
                0x38 | 0x3C => raw |= CGEventFlags::MaskShift.0,
                0x3A | 0x3D => raw |= CGEventFlags::MaskAlternate.0,
                0x37 | 0x36 => raw |= CGEventFlags::MaskCommand.0,
                0x3F => raw |= CGEventFlags::MaskSecondaryFn.0,
                _ => {}
            }
        }
        CGEventFlags(raw)
    }

    fn post(&self, event: &CGEvent) {
        CGEvent::set_integer_value_field(Some(event), CGEventField::EventSourceUserData, INJECT_MAGIC);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
    }

    pub fn inject(&mut self, event: RemoteInputEvent) {
        match event {
            RemoteInputEvent::PointerMove { display_id, nx, ny } => {
                let p = self.map_point(display_id, nx, ny);
                let (ty, btn) = if (self.pressed_buttons & (1 << 0)) != 0 {
                    (CGEventType::LeftMouseDragged, CGMouseButton::Left)
                } else if (self.pressed_buttons & (1 << 1)) != 0 {
                    (CGEventType::RightMouseDragged, CGMouseButton::Right)
                } else if (self.pressed_buttons & (1 << 2)) != 0 {
                    (CGEventType::OtherMouseDragged, CGMouseButton::Center)
                } else {
                    (CGEventType::MouseMoved, CGMouseButton::Left)
                };
                if let Some(ev) = CGEvent::new_mouse_event(self.source.as_deref(), ty, p, btn) {
                    CGEvent::set_flags(Some(&ev), self.current_flags());
                    self.post(&ev);
                }
            }
            RemoteInputEvent::PointerButton {
                display_id,
                nx,
                ny,
                button,
                pressed,
            } => {
                let p = self.map_point(display_id, nx, ny);
                let (num, cg_btn, down_ty, up_ty) = match button {
                    MouseButtonKind::Left => (
                        0u8,
                        CGMouseButton::Left,
                        CGEventType::LeftMouseDown,
                        CGEventType::LeftMouseUp,
                    ),
                    MouseButtonKind::Right => (
                        1u8,
                        CGMouseButton::Right,
                        CGEventType::RightMouseDown,
                        CGEventType::RightMouseUp,
                    ),
                    MouseButtonKind::Middle => (
                        2u8,
                        CGMouseButton::Center,
                        CGEventType::OtherMouseDown,
                        CGEventType::OtherMouseUp,
                    ),
                    MouseButtonKind::Back => (
                        3u8,
                        CGMouseButton::Center,
                        CGEventType::OtherMouseDown,
                        CGEventType::OtherMouseUp,
                    ),
                    MouseButtonKind::Forward => (
                        4u8,
                        CGMouseButton::Center,
                        CGEventType::OtherMouseDown,
                        CGEventType::OtherMouseUp,
                    ),
                };

                let click_state = if pressed {
                    self.pressed_buttons |= 1u8 << num;
                    let now = Instant::now();
                    let next = match self.last_clicks.get(&num) {
                        Some((prev_t, prev_p, count))
                            if now.saturating_duration_since(*prev_t) <= DOUBLE_CLICK_WINDOW
                                && (prev_p.x - p.x).hypot(prev_p.y - p.y) <= DOUBLE_CLICK_DIST =>
                        {
                            (*count + 1).min(3)
                        }
                        _ => 1,
                    };
                    self.last_clicks.insert(num, (now, p, next));
                    next
                } else {
                    self.pressed_buttons &= !(1u8 << num);
                    self.last_clicks.get(&num).map_or(1, |(_, _, c)| *c)
                };

                let ty = if pressed { down_ty } else { up_ty };
                if let Some(ev) = CGEvent::new_mouse_event(self.source.as_deref(), ty, p, cg_btn) {
                    CGEvent::set_integer_value_field(Some(&ev), CGEventField::MouseEventClickState, click_state);
                    CGEvent::set_integer_value_field(Some(&ev), CGEventField::MouseEventButtonNumber, i64::from(num));
                    CGEvent::set_flags(Some(&ev), self.current_flags());
                    self.post(&ev);
                }
            }
            RemoteInputEvent::Scroll {
                display_id,
                nx,
                ny,
                dx,
                dy,
                pixel,
            } => {
                let p = self.map_point(display_id, nx, ny);
                let unit = if pixel {
                    CGScrollEventUnit::Pixel
                } else {
                    CGScrollEventUnit::Line
                };
                let iy = dy.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
                let ix = dx.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
                if let Some(ev) = CGEvent::new_scroll_wheel_event2(self.source.as_deref(), unit, 2, iy, ix, 0) {
                    CGEvent::set_location(Some(&ev), p);
                    CGEvent::set_flags(Some(&ev), self.current_flags());
                    self.post(&ev);
                }
            }
            RemoteInputEvent::Key { hid_code, pressed } => {
                let code = KeyCode(hid_code);
                let Some(mac) = mac_keymap::hid_to_mac(code) else {
                    return;
                };
                let is_mod = matches!(mac, 0x36..=0x3F);
                let is_repeat = pressed && !is_mod && self.pressed_keys.contains(&mac);
                if is_mod {
                    if pressed {
                        self.pressed_mods.insert(mac);
                    } else {
                        self.pressed_mods.remove(&mac);
                    }
                } else if pressed {
                    self.pressed_keys.insert(mac);
                } else {
                    self.pressed_keys.remove(&mac);
                }

                if let Some(ev) = CGEvent::new_keyboard_event(self.source.as_deref(), mac, pressed) {
                    if is_mod {
                        CGEvent::set_type(Some(&ev), CGEventType::FlagsChanged);
                    } else if is_repeat {
                        CGEvent::set_integer_value_field(Some(&ev), CGEventField::KeyboardEventAutorepeat, 1);
                    }
                    CGEvent::set_flags(Some(&ev), self.current_flags());
                    self.post(&ev);
                }
            }
            RemoteInputEvent::ReleaseAll => {
                self.release_all();
            }
        }
    }

    pub fn release_all(&mut self) {
        let keys: Vec<MacKeyCode> = self.pressed_keys.drain().collect();
        for mac in keys {
            if let Some(ev) = CGEvent::new_keyboard_event(self.source.as_deref(), mac, false) {
                CGEvent::set_flags(Some(&ev), self.current_flags());
                self.post(&ev);
            }
        }
        let mods: Vec<MacKeyCode> = self.pressed_mods.drain().collect();
        for mac in mods {
            if let Some(ev) = CGEvent::new_keyboard_event(self.source.as_deref(), mac, false) {
                CGEvent::set_type(Some(&ev), CGEventType::FlagsChanged);
                CGEvent::set_flags(Some(&ev), CGEventFlags::empty());
                self.post(&ev);
            }
        }
        let buttons = self.pressed_buttons;
        self.pressed_buttons = 0;
        let p = self.last_point;
        for num in 0u8..5 {
            if (buttons & (1 << num)) != 0 {
                let (cg_btn, up_ty) = match num {
                    0 => (CGMouseButton::Left, CGEventType::LeftMouseUp),
                    1 => (CGMouseButton::Right, CGEventType::RightMouseUp),
                    _ => (CGMouseButton::Center, CGEventType::OtherMouseUp),
                };
                if let Some(ev) = CGEvent::new_mouse_event(self.source.as_deref(), up_ty, p, cg_btn) {
                    CGEvent::set_integer_value_field(Some(&ev), CGEventField::MouseEventButtonNumber, i64::from(num));
                    CGEvent::set_flags(Some(&ev), CGEventFlags::empty());
                    self.post(&ev);
                }
            }
        }
    }
}

impl Drop for InputInjector {
    fn drop(&mut self) {
        self.release_all();
    }
}

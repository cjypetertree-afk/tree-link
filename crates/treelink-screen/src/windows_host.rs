//! Windows native remote desktop host: GDI + WIC JPEG screen capture and Win32 SendInput injection.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;

use crate::protocol::{
    DisplayDescriptor, EncodedVideoFrame, MouseButtonKind, PermissionSummary, RemoteInputEvent, StreamConfig,
    VideoCodec,
};

pub type FrameSink = Arc<dyn Fn(EncodedVideoFrame) + Send + Sync>;

#[repr(C)]
struct TLWinDisplayInfo {
    display_id: u32,
    origin_x: i32,
    origin_y: i32,
    logical_width: u32,
    logical_height: u32,
    pixel_width: u32,
    pixel_height: u32,
    scale_factor: f32,
    is_primary: bool,
    name: [c_char; 128],
}

type TLWinFrameCallback = extern "C" fn(
    user_data: *mut c_void,
    display_id: u32,
    width: u16,
    height: u16,
    codec: u8,
    is_keyframe: bool,
    timestamp_us: u64,
    data: *const u8,
    len: usize,
);

unsafe extern "C" {
    fn tl_win_list_displays(out_buf: *mut TLWinDisplayInfo, max_count: usize) -> usize;
    fn tl_win_capture_start(
        display_id: u32,
        max_width: u32,
        max_height: u32,
        fps: u32,
        bitrate_kbps: u32,
        show_cursor: bool,
        callback: TLWinFrameCallback,
        user_data: *mut c_void,
    ) -> *mut c_void;
    fn tl_win_capture_request_keyframe(handle: *mut c_void);
    fn tl_win_capture_stop(handle: *mut c_void);
    fn tl_win_inject_pointer_move(display_id: u32, nx: f32, ny: f32);
    fn tl_win_inject_pointer_button(display_id: u32, nx: f32, ny: f32, button_id: u8, pressed: bool);
    fn tl_win_inject_scroll(display_id: u32, nx: f32, ny: f32, delta_x: f32, delta_y: f32, pixel: bool);
    fn tl_win_inject_key(hid_code: u32, pressed: bool);
    fn tl_win_inject_release_all();
}

pub fn host_capture_supported() -> bool {
    true
}

pub fn permissions() -> PermissionSummary {
    PermissionSummary {
        screen_recording: true,
        accessibility: true,
    }
}

pub fn request_screen_recording_permission() -> bool {
    true
}

pub fn request_accessibility_permission() -> bool {
    true
}

pub fn list_displays() -> Vec<DisplayDescriptor> {
    let mut raw: Vec<TLWinDisplayInfo> = (0..32)
        .map(|_| TLWinDisplayInfo {
            display_id: 0,
            origin_x: 0,
            origin_y: 0,
            logical_width: 0,
            logical_height: 0,
            pixel_width: 0,
            pixel_height: 0,
            scale_factor: 1.0,
            is_primary: false,
            name: [0; 128],
        })
        .collect();
    // SAFETY: `raw` provides 32 writable `TLWinDisplayInfo` entries.
    let count = unsafe { tl_win_list_displays(raw.as_mut_ptr(), raw.len()) };
    raw.truncate(count);
    raw.into_iter()
        .map(|d| {
            // SAFETY: `d.name` is NUL-terminated by `snprintf`/`WideCharToMultiByte`.
            let name = unsafe { CStr::from_ptr(d.name.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            DisplayDescriptor {
                display_id: d.display_id,
                name,
                origin_x: d.origin_x,
                origin_y: d.origin_y,
                logical_width: d.logical_width.max(1),
                logical_height: d.logical_height.max(1),
                pixel_width: d.pixel_width.max(1),
                pixel_height: d.pixel_height.max(1),
                scale_factor: d.scale_factor.max(1.0),
                is_primary: d.is_primary,
            }
        })
        .collect()
}

struct CallbackBox {
    sink: FrameSink,
}

extern "C" fn frame_trampoline(
    user_data: *mut c_void,
    display_id: u32,
    width: u16,
    height: u16,
    codec: u8,
    is_keyframe: bool,
    timestamp_us: u64,
    data: *const u8,
    len: usize,
) {
    if user_data.is_null() || data.is_null() || len == 0 {
        return;
    }
    let Some(codec) = VideoCodec::from_wire_id(codec) else {
        return;
    };
    // SAFETY: `user_data` stays alive until `tl_win_capture_stop` joins the worker thread.
    let cb = unsafe { &*user_data.cast::<CallbackBox>() };
    // SAFETY: `data` points to `len` valid bytes for the duration of this synchronous callback.
    let payload = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    (cb.sink)(EncodedVideoFrame {
        display_id,
        width,
        height,
        codec,
        is_keyframe,
        timestamp_us,
        payload,
    });
}

#[derive(Debug)]
pub struct CaptureHandle {
    native: *mut c_void,
    cb_box: *mut CallbackBox,
}

// SAFETY: `native` and `cb_box` are synchronized inside C++ (`atomic` + `thread::join` on drop).
unsafe impl Send for CaptureHandle {}
// SAFETY: `request_keyframe` only writes an `std::atomic<bool>`.
unsafe impl Sync for CaptureHandle {}

impl CaptureHandle {
    pub fn start(config: &StreamConfig, sink: FrameSink) -> Result<Self, String> {
        let cb_box = Box::into_raw(Box::new(CallbackBox { sink }));
        // SAFETY: `cb_box` outlives the capture thread (freed in `Drop` after `tl_win_capture_stop`).
        let native = unsafe {
            tl_win_capture_start(
                config.display_id,
                config.max_width,
                config.max_height,
                config.fps,
                config.bitrate_kbps,
                config.show_cursor,
                frame_trampoline,
                cb_box.cast::<c_void>(),
            )
        };
        if native.is_null() {
            // SAFETY: thread never started, reclaim box immediately.
            unsafe {
                drop(Box::from_raw(cb_box));
            }
            return Err("windows_capture_start_failed".into());
        }
        Ok(Self { native, cb_box })
    }

    pub fn request_keyframe(&self) {
        if !self.native.is_null() {
            // SAFETY: handle is live until `Drop`.
            unsafe { tl_win_capture_request_keyframe(self.native) };
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        if !self.native.is_null() {
            // SAFETY: joins the capture thread before freeing `cb_box`.
            unsafe { tl_win_capture_stop(self.native) };
            self.native = std::ptr::null_mut();
        }
        if !self.cb_box.is_null() {
            // SAFETY: capture thread has exited; no further callbacks will touch `cb_box`.
            unsafe {
                drop(Box::from_raw(self.cb_box));
            }
            self.cb_box = std::ptr::null_mut();
        }
    }
}

#[derive(Debug, Default)]
pub struct InputInjector;

impl InputInjector {
    pub fn new() -> Self {
        Self
    }

    pub fn inject(&mut self, event: RemoteInputEvent) {
        match event {
            RemoteInputEvent::PointerMove { display_id, nx, ny } => {
                // SAFETY: pure Win32 SendInput value call.
                unsafe { tl_win_inject_pointer_move(display_id, nx as f32, ny as f32) };
            }
            RemoteInputEvent::PointerButton {
                display_id,
                nx,
                ny,
                button,
                pressed,
            } => {
                let id = match button {
                    MouseButtonKind::Left => 0,
                    MouseButtonKind::Middle => 1,
                    MouseButtonKind::Right => 2,
                    MouseButtonKind::Back => 3,
                    MouseButtonKind::Forward => 4,
                };
                // SAFETY: pure Win32 SendInput value call.
                unsafe { tl_win_inject_pointer_button(display_id, nx as f32, ny as f32, id, pressed) };
            }
            RemoteInputEvent::Scroll {
                display_id,
                nx,
                ny,
                dx,
                dy,
                pixel,
            } => {
                // SAFETY: pure Win32 SendInput value call.
                unsafe {
                    tl_win_inject_scroll(
                        display_id,
                        nx as f32,
                        ny as f32,
                        dx as f32,
                        dy as f32,
                        pixel,
                    );
                }
            }
            RemoteInputEvent::Key { hid_code, pressed } => {
                // SAFETY: pure Win32 SendInput value call.
                unsafe { tl_win_inject_key(hid_code, pressed) };
            }
            RemoteInputEvent::ReleaseAll => self.release_all(),
        }
    }

    pub fn release_all(&mut self) {
        // SAFETY: releases tracked held keys/buttons under mutex.
        unsafe { tl_win_inject_release_all() };
    }
}

impl Drop for InputInjector {
    fn drop(&mut self) {
        self.release_all();
    }
}

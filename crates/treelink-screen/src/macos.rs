//! macOS native ScreenCaptureKit + VideoToolbox hardware screen capture bindings.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Arc;

use crate::protocol::{DisplayDescriptor, EncodedVideoFrame, PermissionSummary, StreamConfig, VideoCodec};

#[repr(C)]
struct TLDisplayInfo {
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

type TLFrameCallback = extern "C" fn(
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
    fn tl_check_screen_recording_permission() -> bool;
    fn tl_request_screen_recording_permission() -> bool;
    fn tl_check_accessibility_permission() -> bool;
    fn tl_list_displays(out_buf: *mut TLDisplayInfo, max_count: usize) -> usize;
    fn tl_capture_start(
        display_id: u32,
        max_width: u32,
        max_height: u32,
        fps: u32,
        bitrate_kbps: u32,
        codec_id: u8,
        show_cursor: bool,
        callback: TLFrameCallback,
        user_data: *mut c_void,
        err_buf: *mut c_char,
        err_buf_len: usize,
    ) -> *mut c_void;
    fn tl_capture_request_keyframe(handle: *mut c_void);
    fn tl_capture_stop(handle: *mut c_void);
}

pub fn host_capture_supported() -> bool {
    true
}

pub fn permissions() -> PermissionSummary {
    // SAFETY: CoreGraphics/ApplicationServices preflight queries are thread-safe read-only C calls.
    unsafe {
        PermissionSummary {
            screen_recording: tl_check_screen_recording_permission(),
            accessibility: tl_check_accessibility_permission(),
        }
    }
}

pub fn request_screen_recording_permission() -> bool {
    // SAFETY: CoreGraphics permission prompt call takes no pointers.
    unsafe { tl_request_screen_recording_permission() }
}

pub fn list_displays() -> Vec<DisplayDescriptor> {
    let mut raw: Vec<TLDisplayInfo> = (0..32)
        .map(|_| TLDisplayInfo {
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

    // SAFETY: `raw` provides 32 writable `TLDisplayInfo` structs; `tl_list_displays` writes at most `raw.len()`.
    let count = unsafe { tl_list_displays(raw.as_mut_ptr(), raw.len()) };
    raw.truncate(count);
    raw.into_iter()
        .map(|d| {
            // SAFETY: `tl_list_displays` zero-initializes `d.name` and copies at most 127 bytes.
            let name = unsafe { CStr::from_ptr(d.name.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            DisplayDescriptor {
                display_id: d.display_id,
                name,
                origin_x: d.origin_x,
                origin_y: d.origin_y,
                logical_width: d.logical_width,
                logical_height: d.logical_height,
                pixel_width: d.pixel_width,
                pixel_height: d.pixel_height,
                scale_factor: d.scale_factor,
                is_primary: d.is_primary,
            }
        })
        .collect()
}

pub type FrameSink = Arc<dyn Fn(EncodedVideoFrame) + Send + Sync>;

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
    // SAFETY: `user_data` stays alive until `tl_capture_stop` drains all VideoToolbox callbacks.
    // `data` points to `len` valid bytes owned by the Obj-C autorelease pool for the duration of this call.
    let (cb, payload) = unsafe {
        let cb = &*(user_data as *const CallbackBox);
        let slice = std::slice::from_raw_parts(data, len);
        (cb, slice.to_vec())
    };
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

pub struct CaptureHandle {
    native: *mut c_void,
    callback_box: *mut CallbackBox,
}

impl std::fmt::Debug for CaptureHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureHandle").finish_non_exhaustive()
    }
}

// SAFETY: `native` is controlled exclusively through thread-safe Obj-C atomic/GCD methods and stopped on Drop.
unsafe impl Send for CaptureHandle {}
// SAFETY: `tl_capture_request_keyframe` only performs an atomic store.
unsafe impl Sync for CaptureHandle {}

impl CaptureHandle {
    pub fn start(config: &StreamConfig, sink: FrameSink) -> Result<Self, String> {
        let callback_box = Box::into_raw(Box::new(CallbackBox { sink }));
        let mut err_buf = [0 as c_char; 256];
        // SAFETY: `callback_box` remains valid until after `tl_capture_stop` finishes; `err_buf` is 256 bytes.
        let native = unsafe {
            tl_capture_start(
                config.display_id,
                config.max_width,
                config.max_height,
                config.fps,
                config.bitrate_kbps,
                config.codec.wire_id(),
                config.show_cursor,
                frame_trampoline,
                callback_box.cast::<c_void>(),
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        if native.is_null() {
            // SAFETY: Native side failed before retaining callbacks; reclaim `callback_box`.
            unsafe {
                drop(Box::from_raw(callback_box));
            }
            // SAFETY: `err_buf` was zero-initialized and at most 255 bytes were written.
            let msg = unsafe { CStr::from_ptr(err_buf.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            return Err(if msg.is_empty() {
                "capture_start_failed".to_string()
            } else {
                msg
            });
        }
        Ok(Self { native, callback_box })
    }

    pub fn request_keyframe(&self) {
        if !self.native.is_null() {
            // SAFETY: `self.native` is valid for the lifetime of `CaptureHandle`.
            unsafe { tl_capture_request_keyframe(self.native) };
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        if !self.native.is_null() {
            // SAFETY: `tl_capture_stop` stops SCStream and synchronizes `encode_queue` before returning,
            // guaranteeing no further `frame_trampoline` calls touch `self.callback_box`.
            unsafe {
                tl_capture_stop(self.native);
            }
            self.native = std::ptr::null_mut();
        }
        if !self.callback_box.is_null() {
            // SAFETY: `self.callback_box` was created via `Box::into_raw` and is no longer referenced by native code.
            unsafe {
                drop(Box::from_raw(self.callback_box));
            }
            self.callback_box = std::ptr::null_mut();
        }
    }
}

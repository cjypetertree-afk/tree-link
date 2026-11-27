//! Non-macOS platform implementation (Windows acts primarily as the multi-Mac viewer/controller;
//! future Windows host capture via DXGI Desktop Duplication plugs in here).

use std::sync::Arc;

use crate::protocol::{DisplayDescriptor, EncodedVideoFrame, PermissionSummary, RemoteInputEvent, StreamConfig};

pub type FrameSink = Arc<dyn Fn(EncodedVideoFrame) + Send + Sync>;

pub fn host_capture_supported() -> bool {
    false
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
    let Ok(backend) = oink_display::platform_backend() else {
        return Vec::new();
    };
    let Ok(displays) = backend.displays() else {
        return Vec::new();
    };
    displays
        .into_iter()
        .enumerate()
        .map(|(idx, d)| {
            let scale_factor = d.pixels_per_unit() as f32;
            DisplayDescriptor {
                display_id: (idx + 1) as u32,
                name: d.name,
                origin_x: d.bounds.x as i32,
                origin_y: d.bounds.y as i32,
                logical_width: d.bounds.w.max(1.0) as u32,
                logical_height: d.bounds.h.max(1.0) as u32,
                pixel_width: d.pixel_width.max(1),
                pixel_height: d.pixel_height.max(1),
                scale_factor,
                is_primary: d.is_primary,
            }
        })
        .collect()
}

#[derive(Debug)]
pub struct CaptureHandle;

impl CaptureHandle {
    pub fn start(_config: &StreamConfig, _sink: FrameSink) -> Result<Self, String> {
        Err("host_capture_unsupported_on_this_os".into())
    }

    pub fn request_keyframe(&self) {}
}

#[derive(Debug, Default)]
pub struct InputInjector;

impl InputInjector {
    pub fn new() -> Self {
        Self
    }

    pub fn inject(&mut self, _event: RemoteInputEvent) {}

    pub fn release_all(&mut self) {}
}

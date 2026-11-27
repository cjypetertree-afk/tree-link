// TREE LINK native macOS ScreenCaptureKit + VideoToolbox hardware encoder.
// Licensed under GPL-3.0-only as part of TREE LINK (derived from Oink).

#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <CoreFoundation/CoreFoundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreImage/CoreImage.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <ImageIO/ImageIO.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <VideoToolbox/VideoToolbox.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    uint32_t display_id;
    int32_t origin_x;
    int32_t origin_y;
    uint32_t logical_width;
    uint32_t logical_height;
    uint32_t pixel_width;
    uint32_t pixel_height;
    float scale_factor;
    bool is_primary;
    char name[128];
} TLDisplayInfo;

// codec: 1 = H.264 Annex-B, 2 = HEVC Annex-B
typedef void (*TLFrameCallback)(
    void *user_data,
    uint32_t display_id,
    uint16_t width,
    uint16_t height,
    uint8_t codec,
    bool is_keyframe,
    uint64_t timestamp_us,
    const uint8_t *data,
    size_t len
);

bool tl_check_screen_recording_permission(void) {
    return CGPreflightScreenCaptureAccess();
}

bool tl_request_screen_recording_permission(void) {
    return CGRequestScreenCaptureAccess();
}

bool tl_check_accessibility_permission(void) {
    return AXIsProcessTrusted();
}

bool tl_request_accessibility_permission(void) {
    @autoreleasepool {
        NSDictionary *opts = @{ (__bridge id)kAXTrustedCheckOptionPrompt: @YES };
        return AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)opts);
    }
}

size_t tl_list_displays(TLDisplayInfo *out_buf, size_t max_count) {
    @autoreleasepool {
        if (!out_buf || max_count == 0) {
            return 0;
        }
        uint32_t count = 0;
        uint32_t cap = (uint32_t)(max_count > 32 ? 32 : max_count);
        CGDirectDisplayID ids[32];
        if (CGGetActiveDisplayList(cap, ids, &count) != kCGErrorSuccess) {
            return 0;
        }
        CGDirectDisplayID main_id = CGMainDisplayID();
        NSArray<NSScreen *> *screens = [NSScreen screens];

        size_t written = 0;
        for (uint32_t i = 0; i < count && written < max_count; i++) {
            CGDirectDisplayID did = ids[i];
            CGRect bounds = CGDisplayBounds(did);
            size_t px_w = CGDisplayPixelsWide(did);
            size_t px_h = CGDisplayPixelsHigh(did);

            CGDisplayModeRef mode = CGDisplayCopyDisplayMode(did);
            if (mode) {
                size_t mode_px_w = CGDisplayModeGetPixelWidth(mode);
                size_t mode_px_h = CGDisplayModeGetPixelHeight(mode);
                if (mode_px_w > 0 && mode_px_h > 0) {
                    px_w = mode_px_w;
                    px_h = mode_px_h;
                }
                CGDisplayModeRelease(mode);
            }

            float scale = 1.0f;
            NSString *display_name = nil;
            for (NSScreen *screen in screens) {
                NSNumber *num = screen.deviceDescription[@"NSScreenNumber"];
                if (num && num.unsignedIntValue == did) {
                    scale = (float)screen.backingScaleFactor;
                    if (@available(macOS 10.15, *)) {
                        display_name = screen.localizedName;
                    }
                    break;
                }
            }
            if (scale <= 0.0f && bounds.size.width > 0.0) {
                scale = (float)(px_w / bounds.size.width);
            }
            if (!display_name || display_name.length == 0) {
                display_name = (did == main_id) ? @"Main Display" : [NSString stringWithFormat:@"Display %u", did];
            }

            TLDisplayInfo *info = &out_buf[written++];
            memset(info, 0, sizeof(*info));
            info->display_id = did;
            info->origin_x = (int32_t)bounds.origin.x;
            info->origin_y = (int32_t)bounds.origin.y;
            info->logical_width = (uint32_t)(bounds.size.width > 0 ? bounds.size.width : 1);
            info->logical_height = (uint32_t)(bounds.size.height > 0 ? bounds.size.height : 1);
            info->pixel_width = (uint32_t)(px_w > 0 ? px_w : info->logical_width);
            info->pixel_height = (uint32_t)(px_h > 0 ? px_h : info->logical_height);
            info->scale_factor = scale > 0.0f ? scale : 1.0f;
            info->is_primary = (did == main_id);

            const char *utf8 = [display_name UTF8String];
            if (utf8) {
                strncpy(info->name, utf8, sizeof(info->name) - 1);
            }
        }
        return written;
    }
}

@interface TLCaptureSession : NSObject <SCStreamOutput, SCStreamDelegate> {
@public
    _Atomic int stopped;
    _Atomic int force_keyframe;
    uint32_t display_id;
    uint16_t enc_width;
    uint16_t enc_height;
    uint8_t codec_id;
    float jpeg_quality;
    TLFrameCallback callback;
    void *user_data;
    VTCompressionSessionRef vt_session;
    CIContext *ci_context;
    CGColorSpaceRef jpeg_color_space;
    dispatch_queue_t sample_queue;
    dispatch_queue_t encode_queue;
    SCStream *stream;
}
@end

static void append_start_code(NSMutableData *out) {
    static const uint8_t sc[4] = {0x00, 0x00, 0x00, 0x01};
    [out appendBytes:sc length:4];
}

static void tl_vt_output_callback(
    void *outputCallbackRefCon,
    void *sourceFrameRefCon,
    OSStatus status,
    VTEncodeInfoFlags infoFlags,
    CMSampleBufferRef sampleBuffer
) {
    (void)sourceFrameRefCon;
    (void)infoFlags;
    if (status != noErr || !sampleBuffer || !CMSampleBufferDataIsReady(sampleBuffer)) {
        return;
    }
    TLCaptureSession *session = (__bridge TLCaptureSession *)outputCallbackRefCon;
    if (!session || atomic_load(&session->stopped) != 0 || !session->callback) {
        return;
    }

    BOOL is_keyframe = YES;
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, false);
    if (attachments && CFArrayGetCount(attachments) > 0) {
        CFDictionaryRef dict = (CFDictionaryRef)CFArrayGetValueAtIndex(attachments, 0);
        if (dict) {
            CFBooleanRef not_sync = (CFBooleanRef)CFDictionaryGetValue(dict, kCMSampleAttachmentKey_NotSync);
            if (not_sync && CFBooleanGetValue(not_sync)) {
                is_keyframe = NO;
            }
        }
    }

    @autoreleasepool {
        NSMutableData *annex_b = [NSMutableData dataWithCapacity:65536];
        CMFormatDescriptionRef fmt = CMSampleBufferGetFormatDescription(sampleBuffer);

        if (is_keyframe && fmt) {
            if (session->codec_id == 2) {
                size_t param_count = 0;
                int nal_len_size = 0;
                if (CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fmt, 0, NULL, NULL, &param_count, &nal_len_size) == noErr) {
                    for (size_t i = 0; i < param_count; i++) {
                        const uint8_t *ps = NULL;
                        size_t ps_size = 0;
                        if (CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(fmt, i, &ps, &ps_size, NULL, NULL) == noErr && ps && ps_size > 0) {
                            append_start_code(annex_b);
                            [annex_b appendBytes:ps length:ps_size];
                        }
                    }
                }
            } else {
                size_t param_count = 0;
                int nal_len_size = 0;
                if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, 0, NULL, NULL, &param_count, &nal_len_size) == noErr) {
                    for (size_t i = 0; i < param_count; i++) {
                        const uint8_t *ps = NULL;
                        size_t ps_size = 0;
                        if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, i, &ps, &ps_size, NULL, NULL) == noErr && ps && ps_size > 0) {
                            append_start_code(annex_b);
                            [annex_b appendBytes:ps length:ps_size];
                        }
                    }
                }
            }
        }

        CMBlockBufferRef block = CMSampleBufferGetDataBuffer(sampleBuffer);
        if (!block) {
            return;
        }
        size_t total_len = 0;
        char *data_ptr = NULL;
        OSStatus bb_status = CMBlockBufferGetDataPointer(block, 0, NULL, &total_len, &data_ptr);
        NSMutableData *contiguous = nil;
        if (bb_status != kCMBlockBufferNoErr || !data_ptr) {
            total_len = CMBlockBufferGetDataLength(block);
            if (total_len == 0) {
                return;
            }
            contiguous = [NSMutableData dataWithLength:total_len];
            if (CMBlockBufferCopyDataBytes(block, 0, total_len, contiguous.mutableBytes) != kCMBlockBufferNoErr) {
                return;
            }
            data_ptr = (char *)contiguous.bytes;
        }

        size_t offset = 0;
        while (offset + 4 <= total_len) {
            uint32_t nal_len = 0;
            memcpy(&nal_len, data_ptr + offset, 4);
            nal_len = CFSwapInt32BigToHost(nal_len);
            offset += 4;
            if (nal_len == 0 || offset + nal_len > total_len) {
                break;
            }
            append_start_code(annex_b);
            [annex_b appendBytes:(data_ptr + offset) length:nal_len];
            offset += nal_len;
        }

        if (annex_b.length == 0) {
            return;
        }

        CMTime pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer);
        uint64_t ts_us = 0;
        if (CMTIME_IS_NUMERIC(pts) && pts.timescale > 0) {
            double secs = CMTimeGetSeconds(pts);
            if (secs > 0.0) {
                ts_us = (uint64_t)(secs * 1000000.0);
            }
        }

        if (atomic_load(&session->stopped) == 0) {
            session->callback(
                session->user_data,
                session->display_id,
                session->enc_width,
                session->enc_height,
                session->codec_id,
                is_keyframe ? true : false,
                ts_us,
                (const uint8_t *)annex_b.bytes,
                annex_b.length
            );
        }
    }
}

@implementation TLCaptureSession

- (void)stream:(SCStream *)scStream didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer ofType:(SCStreamOutputType)type {
    (void)scStream;
    if (type != SCStreamOutputTypeScreen || atomic_load(&self->stopped) != 0) {
        return;
    }
    if (!CMSampleBufferIsValid(sampleBuffer)) {
        return;
    }
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, false);
    if (attachments && CFArrayGetCount(attachments) > 0) {
        CFDictionaryRef dict = (CFDictionaryRef)CFArrayGetValueAtIndex(attachments, 0);
        if (dict) {
            NSNumber *status = (__bridge NSNumber *)CFDictionaryGetValue(dict, (__bridge const void *)SCStreamFrameInfoStatus);
            if (status && status.integerValue != SCFrameStatusComplete) {
                return;
            }
        }
    }

    CVImageBufferRef image_buf = CMSampleBufferGetImageBuffer(sampleBuffer);
    if (!image_buf || (self->codec_id != 3 && !self->vt_session)) {
        return;
    }

    CMTime pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer);
    CMTime dur = CMSampleBufferGetDuration(sampleBuffer);
    CVPixelBufferRetain(image_buf);

    dispatch_async(self->encode_queue, ^{
        if (atomic_load(&self->stopped) == 0) {
            if (self->codec_id == 3 && self->ci_context && self->jpeg_color_space && self->callback) {
                @autoreleasepool {
                    CIImage *ci_img = [CIImage imageWithCVImageBuffer:image_buf];
                    NSDictionary *opts = @{
                        (__bridge NSString *)kCGImageDestinationLossyCompressionQuality: @(self->jpeg_quality)
                    };
                    NSData *jpeg = [self->ci_context JPEGRepresentationOfImage:ci_img
                                                                    colorSpace:self->jpeg_color_space
                                                                       options:opts];
                    if (jpeg && jpeg.length > 0 && atomic_load(&self->stopped) == 0) {
                        uint64_t ts_us = 0;
                        if (CMTIME_IS_NUMERIC(pts) && pts.timescale > 0) {
                            double secs = CMTimeGetSeconds(pts);
                            if (secs > 0.0) ts_us = (uint64_t)(secs * 1000000.0);
                        }
                        self->callback(
                            self->user_data,
                            self->display_id,
                            self->enc_width,
                            self->enc_height,
                            3,
                            true,
                            ts_us,
                            (const uint8_t *)jpeg.bytes,
                            jpeg.length
                        );
                    }
                }
            } else if (self->vt_session) {
                CFDictionaryRef frame_props = NULL;
                if (atomic_exchange(&self->force_keyframe, 0) != 0) {
                    const void *keys[1] = { kVTEncodeFrameOptionKey_ForceKeyFrame };
                    const void *vals[1] = { kCFBooleanTrue };
                    frame_props = CFDictionaryCreate(
                        kCFAllocatorDefault,
                        keys,
                        vals,
                        1,
                        &kCFTypeDictionaryKeyCallBacks,
                        &kCFTypeDictionaryValueCallBacks
                    );
                }
                VTCompressionSessionEncodeFrame(
                    self->vt_session,
                    image_buf,
                    pts,
                    dur,
                    frame_props,
                    NULL,
                    NULL
                );
                if (frame_props) {
                    CFRelease(frame_props);
                }
            }
        }
        CVPixelBufferRelease(image_buf);
    });
}

- (void)stream:(SCStream *)scStream didStopWithError:(NSError *)error {
    (void)scStream;
    (void)error;
}

@end

static uint16_t make_even(uint32_t v) {
    if (v < 64) v = 64;
    if (v > 7680) v = 7680;
    return (uint16_t)(v & ~1u);
}

void *tl_capture_start(
    uint32_t display_id,
    uint32_t max_width,
    uint32_t max_height,
    uint32_t fps,
    uint32_t bitrate_kbps,
    uint8_t codec_id,
    bool show_cursor,
    TLFrameCallback callback,
    void *user_data,
    char *err_buf,
    size_t err_buf_len
) {
    @autoreleasepool {
        if (!callback) {
            if (err_buf && err_buf_len > 0) strncpy(err_buf, "missing callback", err_buf_len - 1);
            return NULL;
        }
        if (!CGPreflightScreenCaptureAccess()) {
            if (err_buf && err_buf_len > 0) strncpy(err_buf, "screen_recording_permission_denied", err_buf_len - 1);
            return NULL;
        }

        __block SCShareableContent *content = nil;
        __block NSError *content_err = nil;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        [SCShareableContent getShareableContentExcludingDesktopWindows:NO
                                                   onScreenWindowsOnly:YES
                                                     completionHandler:^(SCShareableContent * _Nullable c, NSError * _Nullable e) {
            content = c;
            content_err = e;
            dispatch_semaphore_signal(sem);
        }];
        if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0 || !content) {
            if (err_buf && err_buf_len > 0) {
                const char *msg = content_err ? content_err.localizedDescription.UTF8String : "shareable_content_timeout";
                strncpy(err_buf, msg ? msg : "shareable_content_failed", err_buf_len - 1);
            }
            return NULL;
        }

        SCDisplay *target = nil;
        uint32_t wanted = display_id == 0 ? CGMainDisplayID() : display_id;
        for (SCDisplay *d in content.displays) {
            if (d.displayID == wanted) {
                target = d;
                break;
            }
        }
        if (!target && content.displays.count > 0) {
            target = content.displays.firstObject;
        }
        if (!target) {
            if (err_buf && err_buf_len > 0) strncpy(err_buf, "no_active_display", err_buf_len - 1);
            return NULL;
        }

        CGDirectDisplayID actual_id = target.displayID;
        size_t src_w = (size_t)target.width;
        size_t src_h = (size_t)target.height;
        CGDisplayModeRef mode = CGDisplayCopyDisplayMode(actual_id);
        if (mode) {
            size_t pw = CGDisplayModeGetPixelWidth(mode);
            size_t ph = CGDisplayModeGetPixelHeight(mode);
            if (pw > 0 && ph > 0) {
                src_w = pw;
                src_h = ph;
            }
            CGDisplayModeRelease(mode);
        }

        double scale = 1.0;
        if (max_width > 0 && src_w > max_width) {
            double sw = (double)max_width / (double)src_w;
            if (sw < scale) scale = sw;
        }
        if (max_height > 0 && src_h > max_height) {
            double sh = (double)max_height / (double)src_h;
            if (sh < scale) scale = sh;
        }
        uint16_t out_w = make_even((uint32_t)(src_w * scale));
        uint16_t out_h = make_even((uint32_t)(src_h * scale));
        uint32_t clamped_fps = fps < 5 ? 5 : (fps > 120 ? 120 : fps);
        uint32_t clamped_kbps = bitrate_kbps < 500 ? 500 : (bitrate_kbps > 80000 ? 80000 : bitrate_kbps);
        uint8_t actual_codec = (codec_id == 2 || codec_id == 3) ? codec_id : 1;

        TLCaptureSession *session = [[TLCaptureSession alloc] init];
        atomic_init(&session->stopped, 0);
        atomic_init(&session->force_keyframe, 1);
        session->display_id = actual_id;
        session->enc_width = out_w;
        session->enc_height = out_h;
        session->codec_id = actual_codec;
        session->jpeg_quality = clamped_kbps >= 25000 ? 0.84f : (clamped_kbps >= 12000 ? 0.74f : 0.62f);
        session->callback = callback;
        session->user_data = user_data;
        session->sample_queue = dispatch_queue_create("com.treelink.screen.sample", DISPATCH_QUEUE_SERIAL);
        session->encode_queue = dispatch_queue_create("com.treelink.screen.encode", DISPATCH_QUEUE_SERIAL);

        if (actual_codec == 3) {
            session->ci_context = [CIContext contextWithOptions:@{ kCIContextUseSoftwareRenderer: @NO }];
            session->jpeg_color_space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            if (!session->ci_context || !session->jpeg_color_space) {
                if (session->jpeg_color_space) {
                    CFRelease(session->jpeg_color_space);
                    session->jpeg_color_space = NULL;
                }
                if (err_buf && err_buf_len > 0) strncpy(err_buf, "ci_context_failed", err_buf_len - 1);
                return NULL;
            }
        } else {
            NSMutableDictionary *encoder_spec = [NSMutableDictionary dictionary];
            encoder_spec[(__bridge NSString *)kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder] = @YES;
            if (@available(macOS 11.3, *)) {
                encoder_spec[(__bridge NSString *)kVTVideoEncoderSpecification_EnableLowLatencyRateControl] = @YES;
            }

            CMVideoCodecType cm_codec = (actual_codec == 2) ? kCMVideoCodecType_HEVC : kCMVideoCodecType_H264;
            OSStatus vt_err = VTCompressionSessionCreate(
                kCFAllocatorDefault,
                (int32_t)out_w,
                (int32_t)out_h,
                cm_codec,
                (__bridge CFDictionaryRef)encoder_spec,
                NULL,
                kCFAllocatorDefault,
                tl_vt_output_callback,
                (__bridge void *)session,
                &session->vt_session
            );
            if (vt_err != noErr || !session->vt_session) {
                if (err_buf && err_buf_len > 0) {
                    snprintf(err_buf, err_buf_len, "vt_create_failed:%d", (int)vt_err);
                }
                return NULL;
            }

            VTSessionSetProperty(session->vt_session, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
            VTSessionSetProperty(session->vt_session, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
            CFStringRef profile = (actual_codec == 2) ? kVTProfileLevel_HEVC_Main_AutoLevel : kVTProfileLevel_H264_High_AutoLevel;
            VTSessionSetProperty(session->vt_session, kVTCompressionPropertyKey_ProfileLevel, profile);

            int32_t gop = (int32_t)(clamped_fps * 2);
            CFNumberRef gop_num = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &gop);
            VTSessionSetProperty(session->vt_session, kVTCompressionPropertyKey_MaxKeyFrameInterval, gop_num);
            CFRelease(gop_num);

            int32_t fps_i32 = (int32_t)clamped_fps;
            CFNumberRef fps_num = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &fps_i32);
            VTSessionSetProperty(session->vt_session, kVTCompressionPropertyKey_ExpectedFrameRate, fps_num);
            CFRelease(fps_num);

            int32_t avg_bps = (int32_t)(clamped_kbps * 1000);
            CFNumberRef bps_num = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &avg_bps);
            VTSessionSetProperty(session->vt_session, kVTCompressionPropertyKey_AverageBitRate, bps_num);
            CFRelease(bps_num);

            int32_t bytes_per_sec = (int32_t)((clamped_kbps * 1000 / 8) * 3 / 2);
            int32_t one_sec = 1;
            CFNumberRef lim_bytes = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &bytes_per_sec);
            CFNumberRef lim_sec = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &one_sec);
            const void *lim_vals[2] = { lim_bytes, lim_sec };
            CFArrayRef limits = CFArrayCreate(kCFAllocatorDefault, lim_vals, 2, &kCFTypeArrayCallBacks);
            VTSessionSetProperty(session->vt_session, kVTCompressionPropertyKey_DataRateLimits, limits);
            CFRelease(limits);
            CFRelease(lim_bytes);
            CFRelease(lim_sec);

            VTCompressionSessionPrepareToEncodeFrames(session->vt_session);
        }

        SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:target excludingWindows:@[]];
        SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
        cfg.width = out_w;
        cfg.height = out_h;
        cfg.minimumFrameInterval = CMTimeMake(1, (int32_t)clamped_fps);
        cfg.pixelFormat = (actual_codec == 3)
            ? kCVPixelFormatType_32BGRA
            : kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
        cfg.showsCursor = show_cursor ? YES : NO;
        cfg.queueDepth = 3;

        SCStream *stream = [[SCStream alloc] initWithFilter:filter configuration:cfg delegate:session];
        NSError *add_err = nil;
        if (![stream addStreamOutput:session type:SCStreamOutputTypeScreen sampleHandlerQueue:session->sample_queue error:&add_err]) {
            if (session->vt_session) {
                VTCompressionSessionInvalidate(session->vt_session);
                CFRelease(session->vt_session);
                session->vt_session = NULL;
            }
            if (session->jpeg_color_space) {
                CFRelease(session->jpeg_color_space);
                session->jpeg_color_space = NULL;
            }
            if (err_buf && err_buf_len > 0) {
                const char *msg = add_err ? add_err.localizedDescription.UTF8String : "add_output_failed";
                strncpy(err_buf, msg ? msg : "add_output_failed", err_buf_len - 1);
            }
            return NULL;
        }
        session->stream = stream;

        __block NSError *start_err = nil;
        dispatch_semaphore_t start_sem = dispatch_semaphore_create(0);
        [stream startCaptureWithCompletionHandler:^(NSError * _Nullable error) {
            start_err = error;
            dispatch_semaphore_signal(start_sem);
        }];
        if (dispatch_semaphore_wait(start_sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0 || start_err) {
            atomic_store(&session->stopped, 1);
            if (session->vt_session) {
                VTCompressionSessionInvalidate(session->vt_session);
                CFRelease(session->vt_session);
                session->vt_session = NULL;
            }
            if (session->jpeg_color_space) {
                CFRelease(session->jpeg_color_space);
                session->jpeg_color_space = NULL;
            }
            session->stream = nil;
            if (err_buf && err_buf_len > 0) {
                const char *msg = start_err ? start_err.localizedDescription.UTF8String : "start_capture_timeout";
                strncpy(err_buf, msg ? msg : "start_capture_failed", err_buf_len - 1);
            }
            return NULL;
        }

        return (void *)CFBridgingRetain(session);
    }
}

void tl_capture_request_keyframe(void *handle) {
    if (!handle) return;
    TLCaptureSession *session = (__bridge TLCaptureSession *)handle;
    atomic_store(&session->force_keyframe, 1);
}

void tl_capture_stop(void *handle) {
    @autoreleasepool {
        if (!handle) return;
        TLCaptureSession *session = (TLCaptureSession *)CFBridgingRelease(handle);
        atomic_store(&session->stopped, 1);

        SCStream *stream = session->stream;
        session->stream = nil;
        if (stream) {
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            [stream stopCaptureWithCompletionHandler:^(NSError * _Nullable error) {
                (void)error;
                dispatch_semaphore_signal(sem);
            }];
            dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC));
        }

        dispatch_sync(session->encode_queue, ^{
            if (session->vt_session) {
                VTCompressionSessionInvalidate(session->vt_session);
                CFRelease(session->vt_session);
                session->vt_session = NULL;
            }
            if (session->jpeg_color_space) {
                CFRelease(session->jpeg_color_space);
                session->jpeg_color_space = NULL;
            }
            session->ci_context = nil;
        });
        session->callback = NULL;
        session->user_data = NULL;
    }
}

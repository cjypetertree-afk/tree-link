// TREE LINK Windows native remote desktop capture (GDI BitBlt/StretchBlt + WIC JPEG)
// and Win32 SendInput virtual-desktop mouse/scroll/keyboard injection.
// Zero external dependencies beyond built-in Windows 10/11 SDK libraries
// (gdi32, user32, ole32, windowscodecs).

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif

#include <windows.h>
#include <objbase.h>
#include <wincodec.h>
#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <thread>
#include <vector>

struct TLWinDisplayInfo {
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
};

typedef void (*TLWinFrameCallback)(
    void* user_data,
    uint32_t display_id,
    uint16_t width,
    uint16_t height,
    uint8_t codec,
    bool is_keyframe,
    uint64_t timestamp_us,
    const uint8_t* data,
    size_t len);

namespace {

void ensure_dpi_aware() {
    static std::once_flag once;
    std::call_once(once, []() {
        using SetDpiCtxFn = BOOL(WINAPI*)(HANDLE);
        if (HMODULE user32 = GetModuleHandleW(L"user32.dll")) {
            auto fn = reinterpret_cast<SetDpiCtxFn>(
                reinterpret_cast<void*>(GetProcAddress(user32, "SetProcessDpiAwarenessContext")));
            if (fn) {
                // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 == (DPI_AWARENESS_CONTEXT)-4
                fn(reinterpret_cast<HANDLE>(static_cast<intptr_t>(-4)));
            }
        }
    });
}

struct MonitorCollectCtx {
    TLWinDisplayInfo* out;
    size_t max_count;
    size_t count;
};

BOOL CALLBACK enum_monitors_proc(HMONITOR mon, HDC, LPRECT, LPARAM lparam) {
    auto* ctx = reinterpret_cast<MonitorCollectCtx*>(lparam);
    if (!ctx || ctx->count >= ctx->max_count) {
        return FALSE;
    }
    MONITORINFOEXW mi{};
    mi.cbSize = sizeof(mi);
    if (!GetMonitorInfoW(mon, &mi)) {
        return TRUE;
    }
    int32_t left = static_cast<int32_t>(mi.rcMonitor.left);
    int32_t top = static_cast<int32_t>(mi.rcMonitor.top);
    uint32_t width = static_cast<uint32_t>(std::max<LONG>(1, mi.rcMonitor.right - mi.rcMonitor.left));
    uint32_t height = static_cast<uint32_t>(std::max<LONG>(1, mi.rcMonitor.bottom - mi.rcMonitor.top));

    TLWinDisplayInfo info{};
    info.display_id = static_cast<uint32_t>(ctx->count + 1);
    info.origin_x = left;
    info.origin_y = top;
    info.logical_width = width;
    info.logical_height = height;
    info.pixel_width = width;
    info.pixel_height = height;
    info.scale_factor = 1.0f;
    info.is_primary = (mi.dwFlags & MONITORINFOF_PRIMARY) != 0;

    char utf8_name[128]{};
    int written = WideCharToMultiByte(CP_UTF8, 0, mi.szDevice, -1, utf8_name, sizeof(utf8_name), nullptr, nullptr);
    if (written <= 0 || utf8_name[0] == '\0') {
        std::snprintf(info.name, sizeof(info.name), "Windows Display %u", info.display_id);
    } else {
        std::snprintf(info.name, sizeof(info.name), "Windows %s", utf8_name);
    }

    ctx->out[ctx->count++] = info;
    return TRUE;
}

bool find_display(uint32_t display_id, TLWinDisplayInfo& out_info) {
    TLWinDisplayInfo list[32]{};
    MonitorCollectCtx ctx{list, 32, 0};
    EnumDisplayMonitors(nullptr, nullptr, enum_monitors_proc, reinterpret_cast<LPARAM>(&ctx));
    if (ctx.count == 0) {
        out_info = {};
        out_info.display_id = 1;
        out_info.origin_x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        out_info.origin_y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        out_info.pixel_width = static_cast<uint32_t>(std::max(1, GetSystemMetrics(SM_CXSCREEN)));
        out_info.pixel_height = static_cast<uint32_t>(std::max(1, GetSystemMetrics(SM_CYSCREEN)));
        out_info.logical_width = out_info.pixel_width;
        out_info.logical_height = out_info.pixel_height;
        out_info.scale_factor = 1.0f;
        out_info.is_primary = true;
        std::snprintf(out_info.name, sizeof(out_info.name), "Windows Display 1");
        return true;
    }
    if (display_id != 0) {
        for (size_t i = 0; i < ctx.count; ++i) {
            if (list[i].display_id == display_id) {
                out_info = list[i];
                return true;
            }
        }
    }
    for (size_t i = 0; i < ctx.count; ++i) {
        if (list[i].is_primary) {
            out_info = list[i];
            return true;
        }
    }
    out_info = list[0];
    return true;
}

uint64_t hash_bgr_fast(const uint8_t* pixels, uint32_t row_stride, uint32_t height) {
    uint64_t h = 14695981039346656037ULL;
    const size_t stride = static_cast<size_t>(row_stride);
    const uint32_t row_step = std::max<uint32_t>(1, height / 96);
    for (uint32_t y = 0; y < height; y += row_step) {
        const uint8_t* row = pixels + static_cast<size_t>(y) * stride;
        const size_t col_step = std::max<size_t>(16, stride / 64);
        for (size_t x = 0; x + 8 <= stride; x += col_step) {
            uint64_t v = 0;
            std::memcpy(&v, row + x, sizeof(v));
            h ^= v;
            h *= 1099511628211ULL;
        }
    }
    return h;
}

void draw_cursor_if_visible(HDC dst_dc, const TLWinDisplayInfo& disp, uint32_t dst_w, uint32_t dst_h) {
    CURSORINFO ci{};
    ci.cbSize = sizeof(ci);
    if (!GetCursorInfo(&ci) || (ci.flags & CURSOR_SHOWING) == 0 || !ci.hCursor) {
        return;
    }
    int rel_x = ci.ptScreenPos.x - disp.origin_x;
    int rel_y = ci.ptScreenPos.y - disp.origin_y;
    if (rel_x < -64 || rel_y < -64 ||
        rel_x > static_cast<int>(disp.pixel_width) + 64 ||
        rel_y > static_cast<int>(disp.pixel_height) + 64) {
        return;
    }
    ICONINFO ii{};
    int hot_x = 0;
    int hot_y = 0;
    if (GetIconInfo(ci.hCursor, &ii)) {
        hot_x = static_cast<int>(ii.xHotspot);
        hot_y = static_cast<int>(ii.yHotspot);
        if (ii.hbmMask) DeleteObject(ii.hbmMask);
        if (ii.hbmColor) DeleteObject(ii.hbmColor);
    }
    int draw_x = static_cast<int>(
        (static_cast<int64_t>(rel_x - hot_x) * static_cast<int64_t>(dst_w)) /
        static_cast<int64_t>(std::max<uint32_t>(1, disp.pixel_width)));
    int draw_y = static_cast<int>(
        (static_cast<int64_t>(rel_y - hot_y) * static_cast<int64_t>(dst_h)) /
        static_cast<int64_t>(std::max<uint32_t>(1, disp.pixel_height)));
    DrawIconEx(dst_dc, draw_x, draw_y, ci.hCursor, 0, 0, 0, nullptr, DI_NORMAL);
}

bool encode_wic_jpeg(
    IWICImagingFactory* factory,
    const uint8_t* bgr,
    uint32_t width,
    uint32_t height,
    uint32_t row_stride,
    float quality,
    std::vector<uint8_t>& out_jpeg) {
    if (!factory || !bgr || width == 0 || height == 0 || row_stride < width * 3u) {
        return false;
    }
    IStream* stream = nullptr;
    if (FAILED(CreateStreamOnHGlobal(nullptr, TRUE, &stream)) || !stream) {
        return false;
    }
    IWICBitmapEncoder* encoder = nullptr;
    IWICBitmapFrameEncode* frame = nullptr;
    IPropertyBag2* props = nullptr;
    bool ok = false;

    do {
        if (FAILED(factory->CreateEncoder(GUID_ContainerFormatJpeg, nullptr, &encoder)) || !encoder) break;
        if (FAILED(encoder->Initialize(stream, WICBitmapEncoderNoCache))) break;
        if (FAILED(encoder->CreateNewFrame(&frame, &props)) || !frame) break;
        if (props) {
            PROPBAG2 option{};
            option.pstrName = const_cast<LPOLESTR>(L"ImageQuality");
            VARIANT var{};
            VariantInit(&var);
            var.vt = VT_R4;
            var.fltVal = quality;
            props->Write(1, &option, &var);
        }
        if (FAILED(frame->Initialize(props))) break;
        if (FAILED(frame->SetSize(width, height))) break;
        WICPixelFormatGUID format = GUID_WICPixelFormat24bppBGR;
        if (FAILED(frame->SetPixelFormat(&format))) break;
        if (!IsEqualGUID(format, GUID_WICPixelFormat24bppBGR)) break;
        UINT stride = row_stride;
        UINT buf_size = row_stride * height;
        if (FAILED(frame->WritePixels(height, stride, buf_size, const_cast<BYTE*>(bgr)))) break;
        if (FAILED(frame->Commit())) break;
        if (FAILED(encoder->Commit())) break;

        HGLOBAL hglobal = nullptr;
        if (FAILED(GetHGlobalFromStream(stream, &hglobal)) || !hglobal) break;
        STATSTG stat{};
        if (FAILED(stream->Stat(&stat, STATFLAG_NONAME))) break;
        size_t len = static_cast<size_t>(stat.cbSize.QuadPart);
        if (len == 0 || len > 32 * 1024 * 1024) break;
        void* locked = GlobalLock(hglobal);
        if (!locked) break;
        out_jpeg.resize(len);
        std::memcpy(out_jpeg.data(), locked, len);
        GlobalUnlock(hglobal);
        ok = true;
    } while (false);

    if (props) props->Release();
    if (frame) frame->Release();
    if (encoder) encoder->Release();
    stream->Release();
    return ok;
}

struct CaptureSession {
    uint32_t requested_display_id = 0;
    uint32_t max_width = 1920;
    uint32_t max_height = 1080;
    uint32_t fps = 30;
    float jpeg_quality = 0.74f;
    bool show_cursor = false;
    TLWinFrameCallback callback = nullptr;
    void* user_data = nullptr;
    std::atomic<bool> stop{false};
    std::atomic<bool> keyframe_requested{true};
    std::thread worker;
};

void capture_loop(CaptureSession* session) {
    ensure_dpi_aware();
    SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
    HRESULT com_hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    bool com_inited = SUCCEEDED(com_hr);

    IWICImagingFactory* factory = nullptr;
    CoCreateInstance(
        CLSID_WICImagingFactory,
        nullptr,
        CLSCTX_INPROC_SERVER,
        IID_PPV_ARGS(&factory));

    HDC screen_dc = GetDC(nullptr);
    HDC mem_dc = screen_dc ? CreateCompatibleDC(screen_dc) : nullptr;
    HBITMAP dib = nullptr;
    HGDIOBJ old_obj = nullptr;
    uint8_t* dib_bits = nullptr;
    uint32_t alloc_w = 0;
    uint32_t alloc_h = 0;
    uint32_t alloc_stride = 0;

    std::vector<uint8_t> jpeg_buf;
    jpeg_buf.reserve(512 * 1024);

    const uint32_t clamped_fps = std::clamp<uint32_t>(session->fps, 10, 60);
    const auto frame_period = std::chrono::microseconds(1000000 / clamped_fps);
    uint64_t last_hash = 0;
    auto last_sent = std::chrono::steady_clock::now() - std::chrono::seconds(5);
    const auto start_tp = std::chrono::steady_clock::now();

    while (!session->stop.load(std::memory_order_acquire)) {
        const auto tick_start = std::chrono::steady_clock::now();

        if (!screen_dc) {
            screen_dc = GetDC(nullptr);
            if (screen_dc && !mem_dc) mem_dc = CreateCompatibleDC(screen_dc);
        }
        if (!screen_dc || !mem_dc || !factory) {
            std::this_thread::sleep_for(std::chrono::milliseconds(50));
            continue;
        }

        TLWinDisplayInfo disp{};
        find_display(session->requested_display_id, disp);

        uint32_t dst_w = disp.pixel_width;
        uint32_t dst_h = disp.pixel_height;
        if (dst_w > session->max_width || dst_h > session->max_height) {
            double sx = static_cast<double>(session->max_width) / static_cast<double>(dst_w);
            double sy = static_cast<double>(session->max_height) / static_cast<double>(dst_h);
            double scale = std::min(sx, sy);
            dst_w = std::max<uint32_t>(2, static_cast<uint32_t>(static_cast<double>(dst_w) * scale) & ~1u);
            dst_h = std::max<uint32_t>(2, static_cast<uint32_t>(static_cast<double>(dst_h) * scale) & ~1u);
        }

        if (dst_w != alloc_w || dst_h != alloc_h || !dib || !dib_bits) {
            if (old_obj && mem_dc) {
                SelectObject(mem_dc, old_obj);
                old_obj = nullptr;
            }
            if (dib) {
                DeleteObject(dib);
                dib = nullptr;
                dib_bits = nullptr;
            }
            BITMAPINFO bmi{};
            bmi.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
            bmi.bmiHeader.biWidth = static_cast<LONG>(dst_w);
            bmi.bmiHeader.biHeight = -static_cast<LONG>(dst_h); // top-down 24bpp BGR (native WIC JPEG input)
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 24;
            bmi.bmiHeader.biCompression = BI_RGB;
            void* bits = nullptr;
            dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &bits, nullptr, 0);
            if (dib && bits) {
                old_obj = SelectObject(mem_dc, dib);
                dib_bits = static_cast<uint8_t*>(bits);
                alloc_w = dst_w;
                alloc_h = dst_h;
                alloc_stride = ((dst_w * 3u) + 3u) & ~3u;
                last_hash = 0;
            } else {
                alloc_w = 0;
                alloc_h = 0;
                alloc_stride = 0;
                std::this_thread::sleep_for(std::chrono::milliseconds(50));
                continue;
            }
        }

        BOOL blt_ok = FALSE;
        if (dst_w == disp.pixel_width && dst_h == disp.pixel_height) {
            blt_ok = BitBlt(
                mem_dc, 0, 0, static_cast<int>(dst_w), static_cast<int>(dst_h),
                screen_dc, disp.origin_x, disp.origin_y, SRCCOPY);
        } else {
            SetStretchBltMode(mem_dc, HALFTONE);
            SetBrushOrgEx(mem_dc, 0, 0, nullptr);
            blt_ok = StretchBlt(
                mem_dc, 0, 0, static_cast<int>(dst_w), static_cast<int>(dst_h),
                screen_dc, disp.origin_x, disp.origin_y,
                static_cast<int>(disp.pixel_width), static_cast<int>(disp.pixel_height),
                SRCCOPY);
        }

        if (blt_ok) {
            if (session->show_cursor) {
                draw_cursor_if_visible(mem_dc, disp, dst_w, dst_h);
            }
            const uint64_t h = hash_bgr_fast(dib_bits, alloc_stride, dst_h);
            const bool force = session->keyframe_requested.exchange(false, std::memory_order_acq_rel);
            const bool keepalive = (tick_start - last_sent) >= std::chrono::milliseconds(1000);
            if (force || keepalive || h != last_hash) {
                if (encode_wic_jpeg(factory, dib_bits, dst_w, dst_h, alloc_stride, session->jpeg_quality, jpeg_buf)) {
                    last_hash = h;
                    last_sent = tick_start;
                    const uint64_t ts_us = static_cast<uint64_t>(
                        std::chrono::duration_cast<std::chrono::microseconds>(tick_start - start_tp).count());
                    if (session->callback && !session->stop.load(std::memory_order_acquire)) {
                        session->callback(
                            session->user_data,
                            disp.display_id,
                            static_cast<uint16_t>(dst_w),
                            static_cast<uint16_t>(dst_h),
                            3, // VideoCodec::Jpeg
                            true,
                            ts_us,
                            jpeg_buf.data(),
                            jpeg_buf.size());
                    }
                }
            }
        }

        const auto elapsed = std::chrono::steady_clock::now() - tick_start;
        if (elapsed < frame_period) {
            std::this_thread::sleep_for(frame_period - elapsed);
        }
    }

    if (old_obj && mem_dc) SelectObject(mem_dc, old_obj);
    if (dib) DeleteObject(dib);
    if (mem_dc) DeleteDC(mem_dc);
    if (screen_dc) ReleaseDC(nullptr, screen_dc);
    if (factory) factory->Release();
    if (com_inited) CoUninitialize();
    SetThreadExecutionState(ES_CONTINUOUS);
}

// USB HID Usage (Page 0x07) -> Windows PS/2 Set-1 Scan Code (low 8 bits) + extended flag (bit 8).
uint16_t hid_usage_to_scancode(uint16_t usage) {
    constexpr uint16_t EXT = 0x0100;
    switch (usage) {
        case 0x04: return 0x1E; // A
        case 0x05: return 0x30; // B
        case 0x06: return 0x2E; // C
        case 0x07: return 0x20; // D
        case 0x08: return 0x12; // E
        case 0x09: return 0x21; // F
        case 0x0A: return 0x22; // G
        case 0x0B: return 0x23; // H
        case 0x0C: return 0x17; // I
        case 0x0D: return 0x24; // J
        case 0x0E: return 0x25; // K
        case 0x0F: return 0x26; // L
        case 0x10: return 0x32; // M
        case 0x11: return 0x31; // N
        case 0x12: return 0x18; // O
        case 0x13: return 0x19; // P
        case 0x14: return 0x10; // Q
        case 0x15: return 0x13; // R
        case 0x16: return 0x1F; // S
        case 0x17: return 0x14; // T
        case 0x18: return 0x16; // U
        case 0x19: return 0x2F; // V
        case 0x1A: return 0x11; // W
        case 0x1B: return 0x2D; // X
        case 0x1C: return 0x15; // Y
        case 0x1D: return 0x2C; // Z

        case 0x1E: return 0x02; // 1
        case 0x1F: return 0x03; // 2
        case 0x20: return 0x04; // 3
        case 0x21: return 0x05; // 4
        case 0x22: return 0x06; // 5
        case 0x23: return 0x07; // 6
        case 0x24: return 0x08; // 7
        case 0x25: return 0x09; // 8
        case 0x26: return 0x0A; // 9
        case 0x27: return 0x0B; // 0

        case 0x28: return 0x1C; // Enter
        case 0x29: return 0x01; // Escape
        case 0x2A: return 0x0E; // Backspace
        case 0x2B: return 0x0F; // Tab
        case 0x2C: return 0x39; // Space
        case 0x2D: return 0x0C; // Minus
        case 0x2E: return 0x0D; // Equal
        case 0x2F: return 0x1A; // [
        case 0x30: return 0x1B; // ]
        case 0x31: return 0x2B; // Backslash
        case 0x33: return 0x27; // ;
        case 0x34: return 0x28; // '
        case 0x35: return 0x29; // `
        case 0x36: return 0x33; // ,
        case 0x37: return 0x34; // .
        case 0x38: return 0x35; // /
        case 0x39: return 0x3A; // CapsLock

        case 0x3A: return 0x3B; // F1
        case 0x3B: return 0x3C; // F2
        case 0x3C: return 0x3D; // F3
        case 0x3D: return 0x3E; // F4
        case 0x3E: return 0x3F; // F5
        case 0x3F: return 0x40; // F6
        case 0x40: return 0x41; // F7
        case 0x41: return 0x42; // F8
        case 0x42: return 0x43; // F9
        case 0x43: return 0x44; // F10
        case 0x44: return 0x57; // F11
        case 0x45: return 0x58; // F12

        case 0x46: return 0x37 | EXT; // PrintScreen
        case 0x47: return 0x46;       // ScrollLock
        case 0x49: return 0x52 | EXT; // Insert
        case 0x4A: return 0x47 | EXT; // Home
        case 0x4B: return 0x49 | EXT; // PageUp
        case 0x4C: return 0x53 | EXT; // Delete
        case 0x4D: return 0x4F | EXT; // End
        case 0x4E: return 0x51 | EXT; // PageDown
        case 0x4F: return 0x4D | EXT; // ArrowRight
        case 0x50: return 0x4B | EXT; // ArrowLeft
        case 0x51: return 0x50 | EXT; // ArrowDown
        case 0x52: return 0x48 | EXT; // ArrowUp

        case 0x53: return 0x45 | EXT; // NumLock
        case 0x54: return 0x35 | EXT; // Numpad /
        case 0x55: return 0x37;       // Numpad *
        case 0x56: return 0x4A;       // Numpad -
        case 0x57: return 0x4E;       // Numpad +
        case 0x58: return 0x1C | EXT; // Numpad Enter
        case 0x59: return 0x4F;       // Numpad 1
        case 0x5A: return 0x50;       // Numpad 2
        case 0x5B: return 0x51;       // Numpad 3
        case 0x5C: return 0x4B;       // Numpad 4
        case 0x5D: return 0x4C;       // Numpad 5
        case 0x5E: return 0x4D;       // Numpad 6
        case 0x5F: return 0x47;       // Numpad 7
        case 0x60: return 0x48;       // Numpad 8
        case 0x61: return 0x49;       // Numpad 9
        case 0x62: return 0x52;       // Numpad 0
        case 0x63: return 0x53;       // Numpad .

        case 0xE0: return 0x1D;       // Left Ctrl
        case 0xE1: return 0x2A;       // Left Shift
        case 0xE2: return 0x38;       // Left Alt
        case 0xE3: return 0x5B | EXT; // Left Win/GUI
        case 0xE4: return 0x1D | EXT; // Right Ctrl
        case 0xE5: return 0x36;       // Right Shift
        case 0xE6: return 0x38 | EXT; // Right Alt
        case 0xE7: return 0x5C | EXT; // Right Win/GUI
        default: return 0;
    }
}

void compute_virtual_abs(uint32_t display_id, float nx, float ny, LONG& out_dx, LONG& out_dy) {
    ensure_dpi_aware();
    TLWinDisplayInfo disp{};
    find_display(display_id, disp);
    const double cx = std::clamp<double>(static_cast<double>(nx), 0.0, 1.0);
    const double cy = std::clamp<double>(static_cast<double>(ny), 0.0, 1.0);
    const double px = static_cast<double>(disp.origin_x) + cx * static_cast<double>(std::max<uint32_t>(1, disp.pixel_width - 1));
    const double py = static_cast<double>(disp.origin_y) + cy * static_cast<double>(std::max<uint32_t>(1, disp.pixel_height - 1));

    const double vx = static_cast<double>(GetSystemMetrics(SM_XVIRTUALSCREEN));
    const double vy = static_cast<double>(GetSystemMetrics(SM_YVIRTUALSCREEN));
    const double vw = static_cast<double>(std::max(1, GetSystemMetrics(SM_CXVIRTUALSCREEN) - 1));
    const double vh = static_cast<double>(std::max(1, GetSystemMetrics(SM_CYVIRTUALSCREEN) - 1));

    const double norm_x = std::clamp((px - vx) / vw, 0.0, 1.0);
    const double norm_y = std::clamp((py - vy) / vh, 0.0, 1.0);
    out_dx = static_cast<LONG>(norm_x * 65535.0 + 0.5);
    out_dy = static_cast<LONG>(norm_y * 65535.0 + 0.5);
}

std::mutex g_input_mu;
bool g_held_usages[256]{};
uint8_t g_held_buttons = 0;

void send_usage_key(uint16_t usage, uint16_t sc_ext, bool pressed) {
    INPUT inp{};
    inp.type = INPUT_KEYBOARD;
    if (usage == 0xE3 || usage == 0xE7) {
        inp.ki.wVk = (usage == 0xE3) ? VK_LWIN : VK_RWIN;
        inp.ki.wScan = static_cast<WORD>(sc_ext & 0xFF);
        DWORD flags = KEYEVENTF_EXTENDEDKEY;
        if (!pressed) flags |= KEYEVENTF_KEYUP;
        inp.ki.dwFlags = flags;
    } else {
        inp.ki.wScan = static_cast<WORD>(sc_ext & 0xFF);
        DWORD flags = KEYEVENTF_SCANCODE;
        if (sc_ext & 0x0100) flags |= KEYEVENTF_EXTENDEDKEY;
        if (!pressed) flags |= KEYEVENTF_KEYUP;
        inp.ki.dwFlags = flags;
    }
    SendInput(1, &inp, sizeof(INPUT));
}

} // namespace

extern "C" {

size_t tl_win_list_displays(TLWinDisplayInfo* out_buf, size_t max_count) {
    ensure_dpi_aware();
    if (!out_buf || max_count == 0) return 0;
    MonitorCollectCtx ctx{out_buf, max_count, 0};
    EnumDisplayMonitors(nullptr, nullptr, enum_monitors_proc, reinterpret_cast<LPARAM>(&ctx));
    if (ctx.count == 0) {
        TLWinDisplayInfo fallback{};
        find_display(0, fallback);
        out_buf[0] = fallback;
        return 1;
    }
    return ctx.count;
}

void* tl_win_capture_start(
    uint32_t display_id,
    uint32_t max_width,
    uint32_t max_height,
    uint32_t fps,
    uint32_t bitrate_kbps,
    bool show_cursor,
    TLWinFrameCallback callback,
    void* user_data) {
    auto* session = new CaptureSession();
    session->requested_display_id = display_id;
    session->max_width = std::clamp<uint32_t>(max_width, 640, 3840);
    session->max_height = std::clamp<uint32_t>(max_height, 360, 2160);
    session->fps = std::clamp<uint32_t>(fps, 10, 60);
    session->jpeg_quality = (bitrate_kbps >= 24000) ? 0.82f : (bitrate_kbps >= 12000 ? 0.74f : 0.66f);
    session->show_cursor = show_cursor;
    session->callback = callback;
    session->user_data = user_data;
    session->worker = std::thread(capture_loop, session);
    return session;
}

void tl_win_capture_request_keyframe(void* handle) {
    if (!handle) return;
    auto* session = static_cast<CaptureSession*>(handle);
    session->keyframe_requested.store(true, std::memory_order_release);
}

void tl_win_capture_stop(void* handle) {
    if (!handle) return;
    auto* session = static_cast<CaptureSession*>(handle);
    session->stop.store(true, std::memory_order_release);
    if (session->worker.joinable()) {
        session->worker.join();
    }
    delete session;
}

void tl_win_inject_pointer_move(uint32_t display_id, float nx, float ny) {
    LONG dx = 0, dy = 0;
    compute_virtual_abs(display_id, nx, ny, dx, dy);
    INPUT inp{};
    inp.type = INPUT_MOUSE;
    inp.mi.dx = dx;
    inp.mi.dy = dy;
    inp.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    SendInput(1, &inp, sizeof(INPUT));
}

void tl_win_inject_pointer_button(uint32_t display_id, float nx, float ny, uint8_t button_id, bool pressed) {
    LONG dx = 0, dy = 0;
    compute_virtual_abs(display_id, nx, ny, dx, dy);
    DWORD btn_flag = 0;
    DWORD mouse_data = 0;
    switch (button_id) {
        case 0: btn_flag = pressed ? MOUSEEVENTF_LEFTDOWN : MOUSEEVENTF_LEFTUP; break;
        case 1: btn_flag = pressed ? MOUSEEVENTF_MIDDLEDOWN : MOUSEEVENTF_MIDDLEUP; break;
        case 2: btn_flag = pressed ? MOUSEEVENTF_RIGHTDOWN : MOUSEEVENTF_RIGHTUP; break;
        case 3: btn_flag = pressed ? MOUSEEVENTF_XDOWN : MOUSEEVENTF_XUP; mouse_data = XBUTTON1; break;
        case 4: btn_flag = pressed ? MOUSEEVENTF_XDOWN : MOUSEEVENTF_XUP; mouse_data = XBUTTON2; break;
        default: return;
    }
    {
        std::lock_guard<std::mutex> lock(g_input_mu);
        uint8_t bit = static_cast<uint8_t>(1u << button_id);
        if (pressed) g_held_buttons |= bit;
        else g_held_buttons &= static_cast<uint8_t>(~bit);
    }
    INPUT inp{};
    inp.type = INPUT_MOUSE;
    inp.mi.dx = dx;
    inp.mi.dy = dy;
    inp.mi.mouseData = mouse_data;
    inp.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | btn_flag;
    SendInput(1, &inp, sizeof(INPUT));
}

void tl_win_inject_scroll(uint32_t display_id, float nx, float ny, float delta_x, float delta_y, bool pixel) {
    tl_win_inject_pointer_move(display_id, nx, ny);
    const float scale = pixel ? 2.5f : 120.0f;
    int32_t vy = static_cast<int32_t>(std::lround(delta_y * scale));
    if (vy == 0 && std::fabs(delta_y) > 0.05f) vy = (delta_y > 0.0f) ? 1 : -1;
    int32_t vx = static_cast<int32_t>(std::lround(-delta_x * scale));
    if (vx == 0 && std::fabs(delta_x) > 0.05f) vx = (delta_x < 0.0f) ? 1 : -1;
    if (vy != 0) {
        INPUT inp{};
        inp.type = INPUT_MOUSE;
        inp.mi.dwFlags = MOUSEEVENTF_WHEEL;
        inp.mi.mouseData = static_cast<DWORD>(vy);
        SendInput(1, &inp, sizeof(INPUT));
    }
    if (vx != 0) {
        INPUT inp{};
        inp.type = INPUT_MOUSE;
        inp.mi.dwFlags = MOUSEEVENTF_HWHEEL;
        inp.mi.mouseData = static_cast<DWORD>(vx);
        SendInput(1, &inp, sizeof(INPUT));
    }
}

void tl_win_inject_key(uint32_t hid_code, bool pressed) {
    uint16_t usage = static_cast<uint16_t>(hid_code & 0xFFFFu);
    if (usage >= 256) return;
    uint16_t sc_ext = hid_usage_to_scancode(usage);
    if (sc_ext == 0) return;
    {
        std::lock_guard<std::mutex> lock(g_input_mu);
        g_held_usages[usage] = pressed;
    }
    send_usage_key(usage, sc_ext, pressed);
}

void tl_win_inject_release_all() {
    std::lock_guard<std::mutex> lock(g_input_mu);
    for (uint16_t u = 0; u < 256; ++u) {
        if (g_held_usages[u]) {
            g_held_usages[u] = false;
            uint16_t sc_ext = hid_usage_to_scancode(u);
            if (sc_ext != 0) {
                send_usage_key(u, sc_ext, false);
            }
        }
    }
    if (g_held_buttons != 0) {
        const DWORD up_flags[5] = {
            MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_XUP, MOUSEEVENTF_XUP
        };
        const DWORD x_data[5] = {0, 0, 0, XBUTTON1, XBUTTON2};
        for (uint8_t b = 0; b < 5; ++b) {
            if (g_held_buttons & (1u << b)) {
                INPUT inp{};
                inp.type = INPUT_MOUSE;
                inp.mi.dwFlags = up_flags[b];
                inp.mi.mouseData = x_data[b];
                SendInput(1, &inp, sizeof(INPUT));
            }
        }
        g_held_buttons = 0;
    }
}

} // extern "C"

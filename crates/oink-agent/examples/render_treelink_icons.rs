//! One-shot generator for TREE LINK Q-style chibi tree PNG, ICNS iconset, ICO, BMP, and macOS tray PNGs.
use std::fs;
use std::path::{Path, PathBuf};

use oink_agent::tray::{TrayLook, TrayPaint, TrayState, svg as tray_svg};
use resvg::{tiny_skia, usvg};

fn render_svg(svg_str: &str, width: u32, height: u32) -> tiny_skia::Pixmap {
    let tree = usvg::Tree::from_str(svg_str, &usvg::Options::default()).expect("parse svg");
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("allocate pixmap");
    let sx = width as f32 / tree.size().width();
    let sy = height as f32 / tree.size().height();
    resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
    pixmap
}

fn write_bmp24(path: &Path, pixmap: &tiny_skia::Pixmap) {
    let w = pixmap.width() as usize;
    let h = pixmap.height() as usize;
    let row_bytes = (w * 3).div_ceil(4) * 4;
    let img_size = row_bytes * h;
    let file_size = 54 + img_size;
    let mut out = Vec::with_capacity(file_size);
    // BITMAPFILEHEADER (14 bytes)
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    // BITMAPINFOHEADER (40 bytes)
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(img_size as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    // Bottom-up BGR rows
    for y in (0..h).rev() {
        for x in 0..w {
            let p = pixmap.pixel(x as u32, y as u32).unwrap().demultiply();
            out.push(p.blue());
            out.push(p.green());
            out.push(p.red());
        }
        let pad = row_bytes - w * 3;
        out.resize(out.len() + pad, 0);
    }
    fs::write(path, out).expect("write bmp");
}

fn write_ico(path: &Path, entries: &[(u32, Vec<u8>)]) {
    let count = entries.len() as u16;
    let mut header = Vec::new();
    header.extend_from_slice(&0u16.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes()); // ICO
    header.extend_from_slice(&count.to_le_bytes());

    let mut offset = 6u32 + (entries.len() as u32) * 16;
    for (size, png) in entries {
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        header.push(dim);
        header.push(dim);
        header.push(0);
        header.push(0);
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&32u16.to_le_bytes());
        header.extend_from_slice(&(png.len() as u32).to_le_bytes());
        header.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in entries {
        header.extend_from_slice(png);
    }
    fs::write(path, header).expect("write ico");
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let icons_dir = root.join("apps/oink/src-tauri/icons");
    let mac_svg = fs::read_to_string(icons_dir.join("appicon-macos.svg")).expect("read mac svg");
    let win_svg = fs::read_to_string(icons_dir.join("appicon-windows.svg")).expect("read win svg");

    // 1. Standard Tauri PNG icons.
    render_svg(&mac_svg, 1024, 1024)
        .save_png(icons_dir.join("icon.png"))
        .unwrap();
    render_svg(&mac_svg, 256, 256)
        .save_png(icons_dir.join("128x128@2x.png"))
        .unwrap();
    render_svg(&mac_svg, 128, 128)
        .save_png(icons_dir.join("128x128.png"))
        .unwrap();
    render_svg(&mac_svg, 32, 32)
        .save_png(icons_dir.join("32x32.png"))
        .unwrap();

    // 2. macOS .iconset directory (iconutil turns it into icon.icns).
    let iconset = std::env::temp_dir().join("treelink.iconset");
    let _ = fs::remove_dir_all(&iconset);
    fs::create_dir_all(&iconset).unwrap();
    for (name, sz) in [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ] {
        render_svg(&mac_svg, sz, sz).save_png(iconset.join(name)).unwrap();
    }
    println!("ICONSET_DIR={}", iconset.display());

    // 3. Multi-size Windows icon.ico & Explorer PasteMenu icons (PNG-encoded entries).
    let mut ico_entries = Vec::new();
    for sz in [16u32, 24, 32, 48, 64, 128, 256] {
        let pm = render_svg(&win_svg, sz, sz);
        ico_entries.push((sz, pm.encode_png().unwrap()));
    }
    write_ico(&icons_dir.join("icon.ico"), &ico_entries);

    let win_light_svg = win_svg
        .replace("#152C22", "#ECFCF2")
        .replace("#0D1C16", "#D2F7E2");
    let mut paste_light = Vec::new();
    let mut paste_dark = Vec::new();
    for sz in [16u32, 20, 24, 32, 40, 48, 64] {
        paste_light.push((sz, render_svg(&win_light_svg, sz, sz).encode_png().unwrap()));
        paste_dark.push((sz, render_svg(&win_svg, sz, sz).encode_png().unwrap()));
    }
    let win_pkg = root.join("packaging/windows");
    write_ico(&win_pkg.join("OinkPasteMenu.ico"), &paste_light);
    write_ico(&win_pkg.join("OinkPasteMenuDark.ico"), &paste_dark);

    // 4. All 36 macOS menu-bar tray PNGs from tray/artwork.rs.
    let tray_dir = root.join("crates/oink-agent/src/tray/images");
    for (state, name) in [
        (TrayState::Default, "default"),
        (TrayState::Cursor, "cursor"),
        (TrayState::Locked, "locked"),
        (TrayState::Game, "game"),
        (TrayState::ScreenOff, "screenOff"),
        (TrayState::Paused, "paused"),
    ] {
        for (solid, style) in [(false, "outline"), (true, "solid")] {
            let s = tray_svg(TrayLook { state, solid }, TrayPaint::MacTemplate);
            for sz in [16u32, 32, 48] {
                let pm = render_svg(&s, sz, sz);
                pm.save_png(tray_dir.join(format!("mac-{name}-{style}-{sz}.png")))
                    .unwrap();
            }
        }
    }

    // 5. Windows MSI installer BMPs (493x312 dialog, 493x58 banner).
    let tl_pkg = root.join("packaging/treelink");
    let bg_svg = fs::read_to_string(tl_pkg.join("installer-background.svg")).expect("read bg svg");
    let bn_svg = fs::read_to_string(tl_pkg.join("installer-banner.svg")).expect("read banner svg");
    write_bmp24(&tl_pkg.join("installer-background.bmp"), &render_svg(&bg_svg, 493, 312));
    write_bmp24(&tl_pkg.join("installer-banner.bmp"), &render_svg(&bn_svg, 493, 58));

    println!("ALL_TREELINK_ICONS_GENERATED");
}

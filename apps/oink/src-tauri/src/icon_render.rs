//! TREE LINK Q-style chibi tree runtime icon renderer (macOS Dock, Windows Taskbar & Tray).
use oink_agent::config::{ThemeAccent, ThemeMode};
use resvg::{tiny_skia, usvg};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Appearance {
    pub dark: bool,
    pub accent: [u8; 3],
}

impl Appearance {
    pub fn resolve(mode: ThemeMode, accent: &ThemeAccent, system_dark: bool) -> Self {
        Self {
            dark: mode == ThemeMode::Dark || (mode == ThemeMode::System && system_dark),
            accent: accent.resolved(mode, system_dark).rgb(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Artwork {
    #[cfg(any(target_os = "macos", test))]
    Mac,
    #[cfg(any(windows, test))]
    Windows,
}

pub(crate) fn render(artwork: Artwork, appearance: Appearance, size: u32) -> Result<tiny_skia::Pixmap, String> {
    let source = match artwork {
        #[cfg(any(target_os = "macos", test))]
        Artwork::Mac => include_str!("../icons/appicon-macos.svg"),
        #[cfg(any(windows, test))]
        Artwork::Windows => include_str!("../icons/appicon-windows.svg"),
    };
    // Keep the Q-style chibi tree lush green in both light and dark modes while adapting the tile softly.
    let svg = if appearance.dark {
        source.to_owned()
    } else {
        source
            .replace("#152C22", "#ECFCF2")
            .replace("#0D1C16", "#D2F7E2")
    };
    render_svg(&svg, size)
}

#[cfg(any(windows, test))]
pub(crate) fn render_tray(
    look: oink_agent::tray::TrayLook,
    appearance: Appearance,
    size: u32,
) -> Result<tiny_skia::Pixmap, String> {
    let svg = oink_agent::tray::svg(
        look,
        oink_agent::tray::TrayPaint::Windows {
            dark: appearance.dark,
            accent: appearance.accent,
        },
    );
    render_svg(&svg, size)
}

fn render_svg(svg: &str, size: u32) -> Result<tiny_skia::Pixmap, String> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(|e| e.to_string())?;
    let mut pixels = tiny_skia::Pixmap::new(size, size).ok_or("invalid icon dimensions")?;
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixels.as_mut(),
    );
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oink_agent::config::ThemeColor;

    #[test]
    fn native_tray_render_uses_shared_designer_artwork() {
        use oink_agent::tray::{TrayLook, TrayState};
        let solid = render_tray(
            TrayLook {
                state: TrayState::Cursor,
                solid: true,
            },
            Appearance {
                dark: false,
                accent: [34, 197, 94],
            },
            32,
        )
        .unwrap();
        let hollow = render_tray(
            TrayLook::default(),
            Appearance {
                dark: false,
                accent: [34, 197, 94],
            },
            32,
        )
        .unwrap();
        assert_ne!(solid.data(), hollow.data());
        let paused = render_tray(
            TrayLook {
                state: TrayState::Paused,
                solid: false,
            },
            Appearance {
                dark: false,
                accent: [34, 197, 94],
            },
            32,
        )
        .unwrap();
        assert_ne!(paused.data(), hollow.data());
    }

    #[test]
    fn renders_chibi_tree_in_both_appearances_and_platforms() {
        for artwork in [Artwork::Mac, Artwork::Windows] {
            let dark = render(
                artwork,
                Appearance {
                    dark: true,
                    accent: [34, 197, 94],
                },
                256,
            )
            .unwrap();
            let light = render(
                artwork,
                Appearance {
                    dark: false,
                    accent: [34, 197, 94],
                },
                256,
            )
            .unwrap();
            assert_eq!(dark.pixel(0, 0).unwrap().alpha(), 0);
            assert_eq!(light.pixel(0, 0).unwrap().alpha(), 0);
            // Center canopy pixel is opaque emerald green in both modes.
            let center = dark.pixel(128, 108).unwrap().demultiply();
            assert_eq!(center.alpha(), 255);
            assert!(center.green() > center.red());
            assert_ne!(dark.data(), light.data());
        }
    }

    #[test]
    fn renders_taskbar_sizes_and_preserves_geometry_across_accents() {
        for size in [16, 24, 32, 48, 256] {
            let a = render(
                Artwork::Windows,
                Appearance {
                    dark: false,
                    accent: [255, 0, 0],
                },
                size,
            )
            .unwrap();
            let b = render(
                Artwork::Windows,
                Appearance {
                    dark: true,
                    accent: [0, 255, 0],
                },
                size,
            )
            .unwrap();
            assert_eq!(a.width(), size);
            assert_eq!(a.height(), size);
            assert_eq!(
                a.pixels().iter().map(|p| p.alpha()).collect::<Vec<_>>(),
                b.pixels().iter().map(|p| p.alpha()).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn effective_mode_selects_the_matching_accent_including_system_changes() {
        let pair = ThemeAccent {
            light: ThemeColor::try_from("#12ABCD".to_owned()).unwrap(),
            dark: ThemeColor::try_from("#EF4591".to_owned()).unwrap(),
        };
        for (mode, system, dark) in [
            (ThemeMode::Light, true, false),
            (ThemeMode::Dark, false, true),
            (ThemeMode::System, false, false),
            (ThemeMode::System, true, true),
        ] {
            let appearance = Appearance::resolve(mode, &pair, system);
            assert_eq!(appearance.dark, dark);
            assert_eq!(appearance.accent, if dark { pair.dark.rgb() } else { pair.light.rgb() });
        }
    }
}

//! The 16×16 Q-style chibi tree tray artwork (macOS template + Windows color) with status badges.
use super::{TrayLook, TrayState};

#[derive(Clone, Copy, Debug)]
pub enum TrayPaint {
    MacTemplate,
    Windows { dark: bool, accent: [u8; 3] },
}

pub fn svg(look: TrayLook, paint: TrayPaint) -> String {
    let (canopy_color, trunk_color, badge_color): (String, String, String) = match paint {
        TrayPaint::MacTemplate => ("#000000".into(), "#000000".into(), "#000000".into()),
        TrayPaint::Windows { dark, accent: _ } => {
            let canopy = "#22C55E".into();
            let trunk = if dark { "#E2A474" } else { "#9A5B2C" }.into();
            let badge = if matches!(look.state, TrayState::Paused | TrayState::ScreenOff) {
                "#F04452".into()
            } else {
                "#38BDF8".into()
            };
            (canopy, trunk, badge)
        }
    };

    // Chibi tree silhouette: fluffy rounded crown + tiny top sprout + chubby trunk.
    let crown_path = "M8 2.2 C6.0 2.2 4.6 3.5 4.4 5.0 C2.6 5.3 1.5 6.8 1.5 8.4 C1.5 10.3 3.1 11.7 5.1 11.7 H10.9 C12.9 11.7 14.5 10.3 14.5 8.4 C14.5 6.8 13.4 5.3 11.6 5.0 C11.4 3.5 10.0 2.2 8 2.2 Z";
    let mark = if look.solid {
        format!(
            r#"<path d="M6.8 11.0 V14.6 H9.2 V11.0 Z" fill="{trunk_color}"/><path d="{crown_path}" fill="{canopy_color}"/><path d="M8 2.3 Q8.8 0.7 10.6 0.6 Q10.0 2.1 8 2.3 Z" fill="{canopy_color}"/>"#
        )
    } else {
        format!(
            r#"<path d="M7.0 11.4 V14.5 H9.0 V11.4" fill="none" stroke="{trunk_color}" stroke-width="1.35" stroke-linejoin="round"/><path d="{crown_path}" fill="none" stroke="{canopy_color}" stroke-width="1.4" stroke-linejoin="round"/><path d="M8 2.3 Q8.8 0.9 10.4 0.8" fill="none" stroke="{canopy_color}" stroke-width="1.2" stroke-linecap="round"/>"#
        )
    };

    let badge_mask = match look.state {
        TrayState::Locked => {
            r##"<g transform="translate(8.75 -0.45) scale(.55)" fill="#fff"><path d="M4.3 6.6V4.6a2.7 2.7 0 0 1 5.4 0v2" fill="none" stroke="#fff" stroke-width="1.9" stroke-linecap="round"/><rect x="2.4" y="6.2" width="9.2" height="7" rx="1.8"/><circle cx="7" cy="9.7" r="1.15" fill="#000"/></g>"##
        }
        TrayState::Game => {
            r##"<g transform="translate(8.75 -0.5) scale(.47)" fill="#fff"><path d="M5.1 4.3h5.8a3.6 3.6 0 0 1 3.5 2.8l.9 4.1a1.9 1.9 0 0 1-3.2 1.7l-1.6-1.8H5.5l-1.6 1.8a1.9 1.9 0 0 1-3.2-1.7l.9-4.1a3.6 3.6 0 0 1 3.5-2.8z" stroke="#fff" stroke-width="1" stroke-linejoin="round"/><path d="M4.7 6.9v2.2M3.6 8h2.2" fill="none" stroke="#000" stroke-width="1.5" stroke-linecap="round"/><g fill="#000"><circle cx="10.9" cy="7.3" r="1"/><circle cx="12.3" cy="8.8" r="1"/></g></g>"##
        }
        TrayState::Paused => {
            r##"<g fill="#fff"><rect x="10" y="0.6" width="2" height="6" rx=".7"/><rect x="13.4" y="0.6" width="2" height="6" rx=".7"/></g>"##
        }
        TrayState::ScreenOff => {
            r##"<g fill="none" stroke="#fff" stroke-width="1.4" stroke-linecap="round"><circle cx="12.6" cy="3.6" r="3.1"/><path d="M10.4 1.4l4.4 4.4"/></g>"##
        }
        TrayState::Default | TrayState::Cursor => "",
    };

    let content = if badge_mask.is_empty() {
        mark
    } else {
        format!(
            r##"<defs><mask id="knock" maskUnits="userSpaceOnUse" x="0" y="0" width="16" height="16"><rect width="16" height="16" fill="#fff"/><circle cx="12.6" cy="3.6" r="4.6" fill="#000"/></mask><mask id="badge" maskUnits="userSpaceOnUse" x="0" y="0" width="16" height="16">{badge_mask}</mask></defs><g mask="url(#knock)">{mark}</g><rect width="16" height="16" fill="{badge_color}" mask="url(#badge)"/>"##
        )
    };
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">{content}</svg>"#)
}

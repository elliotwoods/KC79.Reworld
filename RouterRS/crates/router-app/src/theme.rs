//! Design system: the app palette, health-state colors, and the shared
//! container/button style helpers every view uses instead of ad-hoc closures.

use iced::theme::Palette;
use iced::widget::{button, container};
use iced::{Background, Border, Color, Theme};
use router_report::PortalState;

const fn c(r: f32, g: f32, b: f32) -> Color {
    Color { r, g, b, a: 1.0 }
}

const fn ca(r: f32, g: f32, b: f32, a: f32) -> Color {
    Color { r, g, b, a }
}

// ------------------------------------------------------------------ palette

pub const BG_BASE: Color = c(0.066, 0.074, 0.094); // #111318
pub const BG_SURFACE: Color = c(0.098, 0.110, 0.137); // #191c23
pub const BG_RAISED: Color = c(0.125, 0.141, 0.176); // #20242d
pub const BORDER: Color = c(0.172, 0.192, 0.235); // #2c313c
pub const TEXT: Color = c(0.910, 0.918, 0.941); // #e8eaf0
pub const TEXT_DIM: Color = ca(0.910, 0.918, 0.941, 0.60);
pub const TEXT_MUTED: Color = ca(0.910, 0.918, 0.941, 0.35);
pub const ACCENT: Color = c(0.310, 0.557, 0.969); // #4f8ef7
pub const OK: Color = c(0.247, 0.812, 0.431); // #3fcf6e
pub const WARN: Color = c(0.949, 0.710, 0.180); // #f2b52e
pub const ERROR: Color = c(0.898, 0.282, 0.302); // #e5484d

// canvas markers
pub const LIVE_BLUE: Color = c(0.30, 0.55, 1.00);
pub const TARGET_WHITE: Color = c(0.95, 0.95, 0.95);
pub const GRID_LINE: Color = ca(1.0, 1.0, 1.0, 0.13);

// per-source-type accents (Renderer source cards)
pub const SRC_GRADIENT: Color = c(0.62, 0.51, 0.96); // purple
pub const SRC_TEXT: Color = c(0.28, 0.78, 0.76); // teal
pub const SRC_FILE: Color = c(0.96, 0.58, 0.28); // orange
pub const SRC_SPOUT: Color = c(0.92, 0.42, 0.72); // pink

pub fn theme() -> Theme {
    Theme::custom(
        "RouterRS".to_string(),
        Palette {
            background: BG_BASE,
            text: TEXT,
            primary: ACCENT,
            success: OK,
            danger: ERROR,
        },
    )
}

/// Shared health-state color mapping (matches RouterReports' STATE_COLORS).
pub fn state_color(state: PortalState) -> Color {
    match state {
        PortalState::Ok => OK,
        PortalState::Degraded => WARN,
        PortalState::Faulty => ERROR,
        PortalState::Silent => c(0.72, 0.20, 0.22),
        PortalState::Unknown => TEXT_MUTED,
    }
}

pub fn with_alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

// --------------------------------------------------------- container styles

pub fn card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(BG_SURFACE.into()),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

pub fn card_selected(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(BG_SURFACE.into()),
        border: Border {
            color: ACCENT,
            width: 1.5,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

pub fn card_inner(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(BG_RAISED.into()),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

pub fn header_bar(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(BG_SURFACE.into()),
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

pub fn status_bar(theme: &Theme) -> container::Style {
    header_bar(theme)
}

pub fn inspector_panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(BG_SURFACE.into()),
        ..Default::default()
    }
}

/// Small translucent pill in the given color.
pub fn chip(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(with_alpha(color, 0.14).into()),
        text_color: Some(color),
        border: Border {
            color: with_alpha(color, 0.35),
            width: 1.0,
            radius: 99.0.into(),
        },
        ..Default::default()
    }
}

/// Solid badge (e.g. the fault count on the Diagnostics tab).
pub fn badge(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(color.into()),
        text_color: Some(Color::WHITE),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 99.0.into(),
        },
        ..Default::default()
    }
}

pub fn zebra(index: usize) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: (index % 2 == 1).then_some(Background::from(ca(1.0, 1.0, 1.0, 0.025))),
        ..Default::default()
    }
}

/// Track for tiny inline bars (latency, health score).
pub fn bar_track(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(BG_RAISED.into()),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    }
}

pub fn bar_fill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(color.into()),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    }
}

// ------------------------------------------------------------ button styles

fn base_button(background: Color, text_color: Color, border: Color) -> button::Style {
    button::Style {
        background: Some(background.into()),
        text_color,
        border: Border {
            color: border,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

/// Header tab button.
pub fn tab(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        if selected {
            base_button(with_alpha(ACCENT, 0.18), TEXT, with_alpha(ACCENT, 0.6))
        } else if hovered {
            base_button(BG_RAISED, TEXT, BORDER)
        } else {
            base_button(Color::TRANSPARENT, TEXT_DIM, Color::TRANSPARENT)
        }
    }
}

/// Standard tool button (icon + label).
pub fn tool(_theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            base_button(BG_RAISED, TEXT, ACCENT)
        }
        button::Status::Disabled => base_button(BG_SURFACE, TEXT_MUTED, BORDER),
        _ => base_button(BG_RAISED, TEXT, BORDER),
    }
}

/// Primary (accent-filled) button.
pub fn primary(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => c(0.40, 0.63, 1.0),
        _ => ACCENT,
    };
    base_button(bg, Color::WHITE, Color::TRANSPARENT)
}

/// Destructive action (reboot, erase).
pub fn danger(_theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            base_button(ERROR, Color::WHITE, ERROR)
        }
        _ => base_button(with_alpha(ERROR, 0.15), ERROR, with_alpha(ERROR, 0.5)),
    }
}

/// Borderless button for list rows / breadcrumbs.
pub fn ghost(_theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            base_button(BG_RAISED, TEXT, Color::TRANSPARENT)
        }
        _ => base_button(Color::TRANSPARENT, TEXT_DIM, Color::TRANSPARENT),
    }
}

/// Segmented control member (portal sub-navigation).
pub fn segmented(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        if selected {
            base_button(ACCENT, Color::WHITE, Color::TRANSPARENT)
        } else if hovered {
            base_button(BG_RAISED, TEXT, Color::TRANSPARENT)
        } else {
            base_button(Color::TRANSPARENT, TEXT_DIM, Color::TRANSPARENT)
        }
    }
}

/// Toggle-style tool button that lights up when active (verbose, image on).
pub fn toggle(active: bool, color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        if active {
            base_button(with_alpha(color, 0.18), color, with_alpha(color, 0.6))
        } else if hovered {
            base_button(BG_RAISED, TEXT, BORDER)
        } else {
            base_button(BG_RAISED, TEXT_MUTED, BORDER)
        }
    }
}

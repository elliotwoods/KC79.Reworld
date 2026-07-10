//! Lucide icon font (vendored in assets/lucide.ttf, compiled into the
//! binary). Codepoints taken from lucide-static's font/info.json.

use iced::widget::{text, Text};
use iced::Font;

pub const FONT_BYTES: &[u8] = include_bytes!("../assets/lucide.ttf");
pub const FONT: Font = Font::with_name("lucide");

// navigation / modules
pub const HOUSE: char = '\u{e0f5}';
pub const IMAGE: char = '\u{e0f6}';
pub const NETWORK: char = '\u{e125}';
pub const HEART_PULSE: char = '\u{e36e}';
pub const ROUTE: char = '\u{e53e}';

// status / connection
pub const ACTIVITY: char = '\u{e038}';
pub const PLUG: char = '\u{e37f}';
pub const PLUG_ZAP: char = '\u{e45c}';
pub const CABLE: char = '\u{e4e3}';
pub const RADIO: char = '\u{e142}';
pub const SERVER: char = '\u{e153}';
pub const ANTENNA: char = '\u{e4e2}';
pub const ARROW_UP_DOWN: char = '\u{e37d}';
pub const ALERT: char = '\u{e193}';
pub const BUG: char = '\u{e20c}';
pub const CLOCK: char = '\u{e087}';
pub const CHECK_CIRCLE: char = '\u{e226}';

// actions
pub const LOCATE_FIXED: char = '\u{e1db}';
pub const SIREN: char = '\u{e2ef}';
pub const CROSSHAIR: char = '\u{e0ac}';
pub const EYE: char = '\u{e0ba}';
pub const WRENCH: char = '\u{e1b1}';
pub const UNLOCK: char = '\u{e10c}';
pub const LIGHTBULB: char = '\u{e1c2}';
pub const LIGHTBULB_OFF: char = '\u{e208}';
pub const ROTATE_CW: char = '\u{e149}';
pub const TARGET: char = '\u{e180}';
pub const SEND: char = '\u{e152}';
pub const REFRESH: char = '\u{e145}';

// tools / files
pub const SAVE: char = '\u{e14d}';
pub const UPLOAD: char = '\u{e19e}';
pub const HDD_UPLOAD: char = '\u{e4e6}';
pub const ERASER: char = '\u{e28f}';
pub const PLAY: char = '\u{e13c}';
pub const TRASH: char = '\u{e18e}';
pub const FLAG: char = '\u{e0d1}';
pub const FILE_TEXT: char = '\u{e0cc}';
pub const TERMINAL: char = '\u{e181}';
pub const SCROLL_TEXT: char = '\u{e45f}';
pub const CHEVRON_RIGHT: char = '\u{e06f}';
pub const LAYERS: char = '\u{e529}';
pub const CPU: char = '\u{e0a9}';
pub const CIRCLE_GAUGE: char = '\u{e4e1}';
pub const SETTINGS: char = '\u{e154}';

// source types
pub const BLEND: char = '\u{e59c}';
pub const TYPE: char = '\u{e198}';
pub const FILM: char = '\u{e0d0}';
pub const CAST: char = '\u{e066}';

/// An icon glyph as a text widget.
pub fn icon_sized(cp: char, size: u16) -> Text<'static> {
    text(cp.to_string())
        .font(FONT)
        .size(size as f32)
        .line_height(1.0)
}

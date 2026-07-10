//! Navigation state: which top-level tab fills the center panel, and which
//! object the right-hand inspector shows (the ofxCvGui `inspect()`
//! equivalent).

use crate::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopModule {
    Installation,
    Renderer,
    Servers,
    Diagnostics,
}

impl TopModule {
    pub const ALL: [TopModule; 4] = [
        TopModule::Installation,
        TopModule::Renderer,
        TopModule::Servers,
        TopModule::Diagnostics,
    ];

    pub fn title(self) -> &'static str {
        match self {
            TopModule::Installation => "Installation",
            TopModule::Renderer => "Renderer",
            TopModule::Servers => "Servers",
            TopModule::Diagnostics => "Diagnostics",
        }
    }

    pub fn icon(self) -> char {
        match self {
            TopModule::Installation => icons::HOUSE,
            TopModule::Renderer => icons::IMAGE,
            TopModule::Servers => icons::NETWORK,
            TopModule::Diagnostics => icons::HEART_PULSE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalSub {
    Pilot,
    Axis(usize),
    MotorDriverSettings,
    Logger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Module(TopModule),
    Column(usize),
    Portal { col: usize, target: u8 },
    PortalSub { col: usize, target: u8, sub: PortalSub },
    /// Renderer image source (populated in the image-pipeline phase).
    #[allow(dead_code)]
    Source(usize),
}

impl Selection {
    /// The (column, portal) this selection addresses, if any.
    pub fn portal(self) -> Option<(usize, u8)> {
        match self {
            Selection::Portal { col, target } | Selection::PortalSub { col, target, .. } => {
                Some((col, target))
            }
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn column(self) -> Option<usize> {
        match self {
            Selection::Column(col) => Some(col),
            Selection::Portal { col, .. } | Selection::PortalSub { col, .. } => Some(col),
            _ => None,
        }
    }
}

//! What is on the bench.

use serde::{Deserialize, Serialize};

/// Which of the two axes a command addresses.
///
/// The firmware exposes these as separate `motionControlA` / `motionControlB` maps rather than
/// an index, and the 16:1 module is wired entirely to the B channels, so the distinction is
/// never cosmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    A,
    B,
}

impl Axis {
    /// The firmware's sub-map name for this axis, e.g. `motionControlA`.
    pub fn suffix(self) -> &'static str {
        match self {
            Axis::A => "A",
            Axis::B => "B",
        }
    }
}

/// Which firmware image the module is running.
///
/// This is not a preference: it decides which commands exist. Production firmware speaks
/// COBS/MessagePack over RS485 and a terse text menu on USART1; the bench firmware speaks an
/// ASCII line protocol and nothing else. A plan declares what it needs and is refused early
/// rather than failing halfway through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirmwareKind {
    #[default]
    Unknown,
    /// `application_bank_*` -- linked at 0x08006000, above the RS485 bootloader.
    Production,
    /// `home_switch_bench` -- a flat image at 0x08000000. Erases the bootloader.
    Bench,
    /// A board carrying only the bootloader, waiting for an application.
    BootloaderOnly,
}

/// The prism gear ratio, which the firmware auto-detects during its home routine.
///
/// Never assume one: the two ratios differ by more than 2x in microsteps per revolution, and
/// the 16:1 module is thermally limited in a way the 32:1 is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GearRatio {
    #[default]
    Unknown,
    /// 16:1. Measured 92,252 usteps/rev -- the nominal 94,852 is 2.8% wrong.
    R16,
    /// 32:1. Measured 189,704 usteps/rev. The production gearing.
    R32,
}

impl GearRatio {
    /// Measured microsteps per prism revolution.
    ///
    /// Both figures are measured, not derived. The firmware's own
    /// `MOTION_STEPS_PER_PRISM_ROTATION` (189,696) is 8 usteps/rev short at 32:1, and the
    /// 16:1 nominal half-rational is wrong by 2.8%.
    pub fn usteps_per_rev(self) -> Option<i32> {
        match self {
            GearRatio::Unknown => None,
            GearRatio::R16 => Some(92_252),
            GearRatio::R32 => Some(189_704),
        }
    }

    /// The sustained prism speed this gearing survives, in degrees per second.
    ///
    /// The 16:1 unit's stall envelope collapses under sustained duty as the motor heats; 12k
    /// usteps/s dies in ~2.3 minutes where 6k holds for 90. The 32:1 ran 3h20m continuous at
    /// 24k with zero degradation. `None` means "not the limiting factor".
    pub fn sustained_deg_per_s(self) -> Option<f32> {
        match self {
            GearRatio::R16 => Some(25.0),
            GearRatio::R32 | GearRatio::Unknown => None,
        }
    }
}

/// The four health flags the firmware reports per axis.
///
/// Note `switches_ok` arrives on the wire as `SwitchesOK` with a capital S while its siblings
/// are lowerCamel. That is a firmware quirk, preserved rather than fixed so host and device
/// stay compatible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub measure_cycle_ok: bool,
    pub switches_ok: bool,
    pub backlash_ok: bool,
    pub home_ok: bool,
}

impl Health {
    pub fn all_ok(self) -> bool {
        self.measure_cycle_ok && self.switches_ok && self.backlash_ok && self.home_ok
    }
}

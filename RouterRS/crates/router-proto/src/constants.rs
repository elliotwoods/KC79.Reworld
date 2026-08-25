//! Motion constants, evaluated exactly as the C++ macros in
//! `Router/src/Modules/Hardware/PerPortal/Constants.h` (unparenthesized
//! macros, left-to-right C integer arithmetic).

pub const MOTION_STEPS_PER_MOTOR_ROTATION: i32 = 32;
pub const MOTION_GEAR_DRIVE: i32 = 21;
pub const MOTION_GEAR_RING: i32 = 118;
pub const MOTION_MICROSTEPS: i32 = 32;

/// `32 * 118 * 9759 / 296 / 21` evaluated left-to-right in integers = 5928.
/// This is the historical C++ macro value. The true ratio is 5928.247 full
/// steps, and the firmware's `getMicrostepsPerPrismRotation()` now computes
/// the rounded rational instead of this truncation -- keep using
/// `MOTION_MICROSTEPS_PER_PRISM_ROTATION` below, which matches the firmware.
pub const MOTION_STEPS_PER_PRISM_ROTATION: i32 =
    MOTION_STEPS_PER_MOTOR_ROTATION * MOTION_GEAR_RING * 9759 / 296 / MOTION_GEAR_DRIVE;

/// Microsteps per full prism rotation, as the firmware defines it:
/// `round(32 * 118 * 9759 * 32 / (296 * 21))` = 189_704
/// (PortalFW `MotionControl::getMicrostepsPerPrismRotation()`, rounded
/// rational). The old truncated macro value (5928 * 32 = 189_696) was 8
/// microsteps/rev short, which accumulated as a systematic angular drift on
/// multi-revolution moves.
pub const MOTION_MICROSTEPS_PER_PRISM_ROTATION: i32 = {
    const NUM: i64 = MOTION_STEPS_PER_MOTOR_ROTATION as i64
        * MOTION_GEAR_RING as i64
        * 9759
        * MOTION_MICROSTEPS as i64;
    const DEN: i64 = 296 * MOTION_GEAR_DRIVE as i64;
    ((NUM + DEN / 2) / DEN) as i32
};

/// Firmware update frame payload size in bytes (`FW_FRAME_SIZE`).
pub const FW_FRAME_SIZE: usize = 32;

/// Serial baud rate for RS485 devices.
pub const BAUD_RATE: u32 = 115_200;

/// Default port for TCP -> RS485 gateways.
pub const DEFAULT_TCP_PORT: u16 = 4196;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_firmware() {
        // Legacy C++ macro evaluation, kept for reference
        assert_eq!(MOTION_STEPS_PER_PRISM_ROTATION, 5928);
        // Rounded rational, matching PortalFW getMicrostepsPerPrismRotation()
        assert_eq!(MOTION_MICROSTEPS_PER_PRISM_ROTATION, 189_704);
        // "See through" = half a rotation. (Frames captured before the 2026
        // rounding fix report 94_848, i.e. the old 189_696 / 2.)
        assert_eq!(MOTION_MICROSTEPS_PER_PRISM_ROTATION / 2, 94_852);
    }
}

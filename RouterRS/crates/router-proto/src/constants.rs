//! Motion constants, evaluated exactly as the C++ macros in
//! `Router/src/Modules/Hardware/PerPortal/Constants.h` (unparenthesized
//! macros, left-to-right C integer arithmetic).

pub const MOTION_STEPS_PER_MOTOR_ROTATION: i32 = 32;
pub const MOTION_GEAR_DRIVE: i32 = 21;
pub const MOTION_GEAR_RING: i32 = 118;
pub const MOTION_MICROSTEPS: i32 = 32;

/// `32 * 118 * 9759 / 296 / 21` evaluated left-to-right in integers = 5928.
pub const MOTION_STEPS_PER_PRISM_ROTATION: i32 =
    MOTION_STEPS_PER_MOTOR_ROTATION * MOTION_GEAR_RING * 9759 / 296 / MOTION_GEAR_DRIVE;

/// 5928 * 32 = 189_696 microsteps per full prism rotation.
pub const MOTION_MICROSTEPS_PER_PRISM_ROTATION: i32 =
    MOTION_STEPS_PER_PRISM_ROTATION * MOTION_MICROSTEPS;

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
    fn constants_match_cpp_macro_eval() {
        assert_eq!(MOTION_STEPS_PER_PRISM_ROTATION, 5928);
        assert_eq!(MOTION_MICROSTEPS_PER_PRISM_ROTATION, 189_696);
        // The golden captured frame reports position 94848 = "see through" (half rotation)
        assert_eq!(MOTION_MICROSTEPS_PER_PRISM_ROTATION / 2, 94_848);
    }
}

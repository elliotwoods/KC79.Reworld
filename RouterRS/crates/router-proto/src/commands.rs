//! Builders for every command body the C++ Router app sends, byte-compatible
//! with the msgpack11 output. Each returns an `rmpv::Value` body; wrap with
//! `encode_envelope` and `encode_frame` to produce wire bytes.
//!
//! The `address` strings returned by `*_ADDRESS` constants / `osc_address()`
//! are the outbox collation keys used by the C++ app (`Packet::address`).

use rmpv::Value;

use crate::constants::MOTION_MICROSTEPS_PER_PRISM_ROTATION;
use crate::value::key;

fn obj(name: &str, value: Value) -> Value {
    Value::Map(vec![(key(name), value)])
}

// ---------------------------------------------------------------- axes

/// The two prism axes of a portal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    A,
    B,
}

impl Axis {
    pub fn index(self) -> usize {
        match self {
            Axis::A => 0,
            Axis::B => 1,
        }
    }

    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Axis::A,
            1 => Axis::B,
            _ => panic!("axis index out of range: {index}"),
        }
    }

    pub fn letter(self) -> char {
        match self {
            Axis::A => 'A',
            Axis::B => 'B',
        }
    }

    /// Firmware module key for MotionControl ("motionControlA"/"motionControlB").
    pub fn motion_control_key(self) -> &'static str {
        match self {
            Axis::A => "motionControlA",
            Axis::B => "motionControlB",
        }
    }

    /// Firmware module key for MotorDriver ("motorDriverA"/"motorDriverB").
    pub fn motor_driver_key(self) -> &'static str {
        match self {
            Axis::A => "motorDriverA",
            Axis::B => "motorDriverB",
        }
    }
}

// ------------------------------------------------------- basic commands

/// Ping: a nil body ([target, 0, nil]); elicits a bare ACK.
pub fn ping() -> Value {
    Value::Nil
}

/// `{"poll": nil}` — full status report request. Collation address "poll".
pub fn poll() -> Value {
    obj("poll", Value::Nil)
}

/// `{"p": nil}` — mini position poll. Collation address "p".
pub fn poll_position() -> Value {
    obj("p", Value::Nil)
}

/// `{"m": [stepsA, stepsB]}` — two-axis move. Collation address "m".
pub fn move_steps(steps_a: i32, steps_b: i32) -> Value {
    obj(
        "m",
        Value::Array(vec![Value::from(steps_a), Value::from(steps_b)]),
    )
}

/// `{"debugLightsEnabled": bool}`.
pub fn debug_lights_enabled(enabled: bool) -> Value {
    obj("debugLightsEnabled", Value::Boolean(enabled))
}

// ---------------------------------------------------------- keyframes

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeValue {
    /// [posA, posB]
    Pos(i32, i32),
    /// [posA, posB, velA, velB] (velocities in steps/second)
    PosVel(i32, i32, i32, i32),
}

/// `{"keyframe": {"startIndex": n, "values": [[a, b(, va, vb)], ...]}}`.
/// `start_index` is 1-based; the C++ app batches by `keyframeBatchSize`.
/// Collation address "keyframe" (broadcast, non-collateable; existing
/// keyframe packets are removed from the outbox before sending).
pub fn keyframe(start_index: u64, values: &[KeyframeValue]) -> Value {
    let values: Vec<Value> = values
        .iter()
        .map(|v| match *v {
            KeyframeValue::Pos(a, b) => Value::Array(vec![Value::from(a), Value::from(b)]),
            KeyframeValue::PosVel(a, b, va, vb) => Value::Array(vec![
                Value::from(a),
                Value::from(b),
                Value::from(va),
                Value::from(vb),
            ]),
        })
        .collect();
    obj(
        "keyframe",
        Value::Map(vec![
            (key("startIndex"), Value::from(start_index)),
            (key("values"), Value::Array(values)),
        ]),
    )
}

// ------------------------------------------------------ motion control

/// Measure-routine settings sent as a 4-element array
/// `[timeout_s(u8), slowSpeed(i32), backOffDistance(i32), debounceDistance(i32)]`
/// (`MotionControl::getMeasureSettings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasureSettings {
    pub timeout_s: u8,
    pub slow_speed: i32,
    pub back_off_distance: i32,
    pub debounce_distance: i32,
}

impl Default for MeasureSettings {
    fn default() -> Self {
        // Defaults from MotionControl parameters
        Self {
            timeout_s: 60,
            slow_speed: 1000,
            back_off_distance: 100,
            debounce_distance: 10,
        }
    }
}

impl MeasureSettings {
    fn to_value(self) -> Value {
        Value::Array(vec![
            Value::from(self.timeout_s),
            Value::from(self.slow_speed),
            Value::from(self.back_off_distance),
            Value::from(self.debounce_distance),
        ])
    }
}

fn mc(axis: Axis, cmd: &str, value: Value) -> Value {
    obj(axis.motion_control_key(), obj(cmd, value))
}

/// Collation address for a MotionControl command, e.g. "motionControlA/move".
pub fn mc_address(axis: Axis, cmd: &str) -> String {
    format!("{}/{}", axis.motion_control_key(), cmd)
}

/// `{"motionControlX": {"move": target}}`.
pub fn mc_move(axis: Axis, target: i32) -> Value {
    mc(axis, "move", Value::from(target))
}

/// `{"motionControlX": {"move": [target, maxVelocity, acceleration, minVelocity]}}`.
pub fn mc_move_with_profile(
    axis: Axis,
    target: i32,
    max_velocity: i32,
    acceleration: i32,
    min_velocity: i32,
) -> Value {
    mc(
        axis,
        "move",
        Value::Array(vec![
            Value::from(target),
            Value::from(max_velocity),
            Value::from(acceleration),
            Value::from(min_velocity),
        ]),
    )
}

/// `{"motionControlX": {"motionProfile": [maxVelocity, acceleration, minVelocity]}}`.
pub fn mc_motion_profile(axis: Axis, max_velocity: i32, acceleration: i32, min_velocity: i32) -> Value {
    mc(
        axis,
        "motionProfile",
        Value::Array(vec![
            Value::from(max_velocity),
            Value::from(acceleration),
            Value::from(min_velocity),
        ]),
    )
}

pub fn mc_zero_current_position(axis: Axis) -> Value {
    mc(axis, "zeroCurrentPosition", Value::Nil)
}

pub fn mc_measure_backlash(axis: Axis, settings: MeasureSettings) -> Value {
    mc(axis, "measureBacklash", settings.to_value())
}

pub fn mc_home(axis: Axis, settings: MeasureSettings) -> Value {
    mc(axis, "home", settings.to_value())
}

pub fn mc_init_timer(axis: Axis) -> Value {
    mc(axis, "initTimer", Value::Nil)
}

pub fn mc_deinit_timer(axis: Axis) -> Value {
    mc(axis, "deinitTimer", Value::Nil)
}

pub fn mc_test_timer(axis: Axis) -> Value {
    mc(axis, "testTimer", Value::Nil)
}

// -------------------------------------------------------- motor driver

/// `{"motorDriverX": {"testRoutine": nil}}`. Address "motorDriver/testRoutine".
pub fn md_test_routine(axis: Axis) -> Value {
    obj(axis.motor_driver_key(), obj("testRoutine", Value::Nil))
}

/// `{"motorDriverX": {"testTimer": [period_us(u32), count(u32)]}}`.
pub fn md_test_timer(axis: Axis, period_us: u32, count: u32) -> Value {
    obj(
        axis.motor_driver_key(),
        obj(
            "testTimer",
            Value::Array(vec![Value::from(period_us), Value::from(count)]),
        ),
    )
}

// ---------------------------------------------- motor driver settings

/// `{"motorDriverSettings": {"setCurrent": amps_f32}}`.
/// Address "motorDriverSettings/setCurrent".
pub fn mds_set_current(amps: f32) -> Value {
    obj("motorDriverSettings", obj("setCurrent", Value::F32(amps)))
}

/// `{"motorDriverSettings": {"setMicrostepResolution": log2(resolution)}}`.
/// The C++ app transmits log2 of the resolution (32 -> 5).
/// Address "motorDriverSettings/setMicrostepResolution".
pub fn mds_set_microstep_resolution(resolution: u32) -> Value {
    let log2 = 31 - resolution.max(1).leading_zeros();
    obj(
        "motorDriverSettings",
        obj("setMicrostepResolution", Value::from(log2 as u8)),
    )
}

/// `{"homeThreshold": value}` — optical home switch threshold (firmware
/// supports it; exposed for the diagnostics utilities).
pub fn home_threshold(value: i32) -> Value {
    obj("homeThreshold", Value::from(value))
}

// ------------------------------------------------------------- actions

/// Local model side-effect an action has in the C++ app
/// (`portalUpdateFunction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalEffect {
    None,
    /// `pilot->reset(); pilot->setAxes({0, 0})`
    ZeroAxes,
    /// `pilot->reset(); pilot->setAxes({0.5, 0})`
    SeeThroughAxes,
}

/// The 12 broadcastable portal actions (`Portal::getActions()`), each with
/// its caption, OSC address, message body, and local model effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    Ping,
    Init,
    Calibrate,
    Home,
    FlashLeds,
    GoHome,
    SeeThrough,
    DisableDebugLights,
    EnableDebugLights,
    Unjam,
    EscapeFromRoutine,
    Reboot,
}

impl ActionKind {
    pub const ALL: [ActionKind; 12] = [
        ActionKind::Ping,
        ActionKind::Init,
        ActionKind::Calibrate,
        ActionKind::Home,
        ActionKind::FlashLeds,
        ActionKind::GoHome,
        ActionKind::SeeThrough,
        ActionKind::DisableDebugLights,
        ActionKind::EnableDebugLights,
        ActionKind::Unjam,
        ActionKind::EscapeFromRoutine,
        ActionKind::Reboot,
    ];

    pub fn caption(self) -> &'static str {
        match self {
            ActionKind::Ping => "Ping",
            ActionKind::Init => "Initialise routine",
            ActionKind::Calibrate => "Calibrate routine",
            ActionKind::Home => "Home routine",
            ActionKind::FlashLeds => "Flash lights",
            ActionKind::GoHome => "Go Home",
            ActionKind::SeeThrough => "See through",
            ActionKind::DisableDebugLights => "Disable debug lights",
            ActionKind::EnableDebugLights => "Enable debug lights",
            ActionKind::Unjam => "Unjam",
            ActionKind::EscapeFromRoutine => "Escape from routine",
            ActionKind::Reboot => "Reboot",
        }
    }

    /// OSC address segment (also reachable as `/<col>/<address>` and
    /// `/<col>/<portal>/<address>`).
    pub fn osc_address(self) -> &'static str {
        match self {
            ActionKind::Ping => "ping",
            ActionKind::Init => "init",
            ActionKind::Calibrate => "calibrate",
            ActionKind::Home => "home",
            ActionKind::FlashLeds => "flashLEDs",
            ActionKind::GoHome => "goHome",
            ActionKind::SeeThrough => "seeThrough",
            ActionKind::DisableDebugLights => "disableDebugLights",
            ActionKind::EnableDebugLights => "enableDebugLights",
            ActionKind::Unjam => "unjam",
            ActionKind::EscapeFromRoutine => "escapeFromRoutine",
            ActionKind::Reboot => "reboot",
        }
    }

    pub fn body(self) -> Value {
        match self {
            ActionKind::Ping => Value::Nil,
            ActionKind::Init => obj("init", Value::Nil),
            ActionKind::Calibrate => obj("calibrate", Value::Nil),
            ActionKind::Home => obj("home", Value::Nil),
            ActionKind::FlashLeds => obj("flashLED", Value::Nil),
            ActionKind::GoHome => move_steps(0, 0),
            ActionKind::SeeThrough => move_steps(MOTION_MICROSTEPS_PER_PRISM_ROTATION / 2, 0),
            ActionKind::DisableDebugLights => debug_lights_enabled(false),
            ActionKind::EnableDebugLights => debug_lights_enabled(true),
            ActionKind::Unjam => obj("unjam", Value::Nil),
            ActionKind::EscapeFromRoutine => obj("escapeFromRoutine", Value::Nil),
            ActionKind::Reboot => obj("reset", Value::Nil),
        }
    }

    pub fn local_effect(self) -> LocalEffect {
        match self {
            ActionKind::Init
            | ActionKind::Calibrate
            | ActionKind::Home
            | ActionKind::GoHome
            | ActionKind::Reboot => LocalEffect::ZeroAxes,
            ActionKind::SeeThrough => LocalEffect::SeeThroughAxes,
            _ => LocalEffect::None,
        }
    }

    pub fn by_osc_address(address: &str) -> Option<ActionKind> {
        Self::ALL.iter().copied().find(|a| a.osc_address() == address)
    }

    pub fn by_caption(caption: &str) -> Option<ActionKind> {
        Self::ALL.iter().copied().find(|a| a.caption() == caption)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::dump_to_vec;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn move_body_bytes() {
        assert_eq!(hex(&dump_to_vec(&move_steps(0, 0))), "81 A1 6D 92 00 00");
        assert_eq!(
            hex(&dump_to_vec(&move_steps(94_848, -94_848))),
            "81 A1 6D 92 CE 00 01 72 80 D2 FF FE 8D 80"
        );
    }

    #[test]
    fn see_through_is_half_a_rotation() {
        // {"m": [MICROSTEPS_PER_PRISM_ROTATION/2, 0]} = [94852, 0]
        // (The old C++ Router sent 94848, half of the truncated 189_696.)
        assert_eq!(
            hex(&dump_to_vec(&ActionKind::SeeThrough.body())),
            "81 A1 6D 92 CE 00 01 72 84 00"
        );
    }

    #[test]
    fn microstep_resolution_is_log2() {
        let v = mds_set_microstep_resolution(32);
        let bytes = dump_to_vec(&v);
        // {"motorDriverSettings": {"setMicrostepResolution": 5}}
        assert_eq!(*bytes.last().unwrap(), 5);
        let v = mds_set_microstep_resolution(1);
        assert_eq!(*dump_to_vec(&v).last().unwrap(), 0);
        let v = mds_set_microstep_resolution(256);
        assert_eq!(*dump_to_vec(&v).last().unwrap(), 8);
    }

    #[test]
    fn set_current_is_f32() {
        let bytes = dump_to_vec(&mds_set_current(0.25));
        // value part: CA 3E 80 00 00
        let tail = &bytes[bytes.len() - 5..];
        assert_eq!(hex(tail), "CA 3E 80 00 00");
    }

    #[test]
    fn keyframe_body_shape() {
        let body = keyframe(
            1,
            &[KeyframeValue::PosVel(100, -100, 5, -5), KeyframeValue::Pos(1, 2)],
        );
        let Value::Map(entries) = &body else { panic!() };
        let Value::Map(kf) = &entries[0].1 else { panic!() };
        assert_eq!(kf[0].0.as_str(), Some("startIndex"));
        assert_eq!(kf[0].1.as_u64(), Some(1));
        let Value::Array(values) = &kf[1].1 else { panic!() };
        assert_eq!(values.len(), 2);
        let Value::Array(first) = &values[0] else { panic!() };
        assert_eq!(first.len(), 4);
        let Value::Array(second) = &values[1] else { panic!() };
        assert_eq!(second.len(), 2);
    }

    #[test]
    fn all_actions_have_unique_osc_addresses() {
        let mut seen = std::collections::HashSet::new();
        for action in ActionKind::ALL {
            assert!(seen.insert(action.osc_address()));
            // body must serialize without panicking
            let _ = dump_to_vec(&action.body());
        }
    }

    #[test]
    fn measure_settings_array() {
        let v = mc_home(Axis::B, MeasureSettings::default());
        let bytes = dump_to_vec(&v);
        // {"motionControlB": {"home": [60, 1000, 100, 10]}}
        let expected_tail = "94 3C CD 03 E8 64 0A";
        let tail = &bytes[bytes.len() - 7..];
        assert_eq!(hex(tail), expected_tail);
    }
}

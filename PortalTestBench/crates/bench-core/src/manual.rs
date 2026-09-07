//! Validated single-axis commands shared by manual control clients.
use crate::{
    dut::{Axis, FirmwareKind},
    state::DutState,
    transport::{MotionProfile, Op},
};

pub enum Command {
    Home,
    Jog(f64),
    Move(f64),
}

pub fn prepare(
    dut: &DutState,
    connected: bool,
    axis: Axis,
    command: Command,
    profile: MotionProfile,
) -> Result<Op, &'static str> {
    if !connected {
        return Err("Connect the command route first");
    }
    if matches!(command, Command::Home) {
        if dut.firmware != FirmwareKind::Bench && dut.threshold.is_none() {
            return Err("Calibrate home threshold first");
        }
        return Ok(Op::Home { axis });
    }
    let steps = dut
        .usteps_per_rev
        .filter(|n| *n > 0)
        .ok_or("Identify the module first")?;
    let sign = if axis == Axis::B { -1.0 } else { 1.0 };
    let target = match command {
        Command::Jog(rev) => {
            f64::from(
                dut.axis(axis)
                    .position
                    .ok_or("Actual position is unknown")?,
            ) + rev * f64::from(steps) * sign
        }
        Command::Move(rev) => rev * f64::from(steps) * sign,
        Command::Home => unreachable!(),
    };
    if !target.is_finite() || target < i32::MIN as f64 || target > i32::MAX as f64 {
        return Err("Target is outside the motion range");
    }
    if profile.max_velocity > 28_000 {
        return Err("Speed exceeds the 28,000 stall guard");
    }
    if profile.min_velocity < 0
        || profile.max_velocity <= 0
        || profile.min_velocity > profile.max_velocity
        || profile.acceleration <= 0
    {
        return Err("Invalid motion profile");
    }
    Ok(Op::MoveTo {
        axis,
        usteps: target.round() as i32,
        profile: Some(profile),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> MotionProfile {
        MotionProfile {
            max_velocity: 20_000,
            min_velocity: 100,
            acceleration: 10_000,
        }
    }
    #[test]
    fn jog_uses_measured_gearing_and_inverts_b() {
        let mut dut = DutState {
            usteps_per_rev: Some(92_252),
            ..Default::default()
        };
        dut.b.position = Some(10_000);
        assert!(matches!(
            prepare(&dut, true, Axis::B, Command::Jog(0.01), profile()),
            Ok(Op::MoveTo { usteps: 9077, .. })
        ));
        assert!(prepare(&dut, true, Axis::A, Command::Jog(0.01), profile()).is_err());
    }
    #[test]
    fn refuses_unready_and_unsafe_commands() {
        let dut = DutState {
            usteps_per_rev: Some(189_704),
            ..Default::default()
        };
        assert!(prepare(&dut, false, Axis::A, Command::Move(0.1), profile()).is_err());
        assert!(prepare(&dut, true, Axis::A, Command::Home, profile()).is_err());
        for target in [f64::NAN, f64::INFINITY, 1e9] {
            assert!(prepare(&dut, true, Axis::A, Command::Move(target), profile()).is_err());
        }
        let mut fast = profile();
        fast.max_velocity = 30_000;
        assert!(prepare(&dut, true, Axis::A, Command::Move(0.1), fast).is_err());
    }
}

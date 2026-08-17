//! The Pilot: per-portal state machine converting between position / polar /
//! axes representations and producing move messages. Port of
//! `Router/src/Modules/Hardware/PerPortal/Pilot.cpp`.

use std::time::{Duration, Instant};

use glam::{vec2, Vec2};
use router_proto::commands;
use router_proto::constants::MOTION_MICROSTEPS_PER_PRISM_ROTATION;
use router_proto::Value;

use super::kinematics as kin;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeadingControl {
    Position,
    Polar,
    #[default]
    Axes,
}

impl LeadingControl {
    pub fn as_str(self) -> &'static str {
        match self {
            LeadingControl::Position => "Position",
            LeadingControl::Polar => "Polar",
            LeadingControl::Axes => "Axes",
        }
    }
}

/// Live axis values reported by the hardware, fed into `Pilot::update`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AxisReported {
    pub current_steps: Option<i32>,
    pub target_steps: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct Pilot {
    pub leading_control: LeadingControl,
    /// Target position on the unit disk (leading when Position).
    pub position: Vec2,
    /// Target polar (r, theta) (leading when Polar).
    pub polar: Vec2,
    /// Target axes (a, b) in prism rotations (leading when Axes; default).
    pub axes: Vec2,
    /// Choose the closest whole-rotation cycle when following position/polar.
    pub cyclic: bool,
    /// Axis offset parameter (-0.25..=0.25).
    pub offset: f32,
    pub microsteps_per_prism_rotation: i32,
    /// Re-send targets every second even when unchanged.
    pub send_periodically: bool,

    cached_sent_a: f32,
    cached_sent_b: f32,
    last_send: Instant,

    pub live_axis_known: [bool; 2],
    pub live_axis: Vec2,
    pub live_target_known: [bool; 2],
    pub live_target: Vec2,
}

const SEND_PERIOD: Duration = Duration::from_millis(1000);

impl Default for Pilot {
    fn default() -> Self {
        Self {
            leading_control: LeadingControl::Axes,
            position: Vec2::ZERO,
            polar: Vec2::ZERO,
            axes: Vec2::ZERO,
            cyclic: true,
            offset: 0.0,
            microsteps_per_prism_rotation: MOTION_MICROSTEPS_PER_PRISM_ROTATION,
            send_periodically: false,
            // C++ initializes cached sent values to -2 so the first frame
            // always counts as stale
            cached_sent_a: -2.0,
            cached_sent_b: -2.0,
            last_send: Instant::now(),
            live_axis_known: [false; 2],
            live_axis: Vec2::ZERO,
            live_target_known: [false; 2],
            live_target: Vec2::ZERO,
        }
    }
}

impl Pilot {
    // ------------------------------------------------ conversions (bound)

    pub fn polar_to_axes(&self, polar: Vec2) -> Vec2 {
        kin::polar_to_axes(polar, self.offset)
    }

    pub fn axes_to_polar(&self, axes: Vec2) -> Vec2 {
        kin::axes_to_polar(axes, self.offset)
    }

    pub fn find_closest_axes_cycle(&self, target: Vec2) -> Vec2 {
        kin::find_closest_axes_cycle(target, self.axes)
    }

    pub fn axis_to_steps(&self, value: f32, axis_index: usize) -> i32 {
        kin::axis_to_steps(value, axis_index, self.microsteps_per_prism_rotation)
    }

    pub fn steps_to_axis(&self, steps: i32, axis_index: usize) -> f32 {
        kin::steps_to_axis(steps, axis_index, self.microsteps_per_prism_rotation)
    }

    // ----------------------------------------------------------- setters

    pub fn set_position(&mut self, position: Vec2) {
        self.position = position;
        self.leading_control = LeadingControl::Position;
    }

    pub fn set_polar(&mut self, polar: Vec2) {
        self.polar = polar;
        self.leading_control = LeadingControl::Polar;
    }

    pub fn set_axes(&mut self, axes: Vec2) {
        self.axes = axes;
        self.leading_control = LeadingControl::Axes;
    }

    pub fn set_axes_cyclic(&mut self, target: Vec2) {
        let adjusted = self.find_closest_axes_cycle(target);
        self.set_axes(adjusted);
    }

    /// `seeThrough()` method (inspector button): axes {0, 0.5}.
    /// NOTE: differs from the broadcast SeeThrough *action*, which moves to
    /// steps [MICROSTEPS/2, 0] and locally sets axes {0.5, 0} — a C++
    /// inconsistency kept for 1:1 behavior.
    pub fn see_through(&mut self) {
        self.axes = vec2(0.0, 0.5);
        self.leading_control = LeadingControl::Axes;
    }

    /// Reset local targets and cached remote state (call after homing).
    pub fn reset(&mut self) {
        self.position = Vec2::ZERO;
        self.polar = Vec2::ZERO;
        self.axes = Vec2::ZERO;
        self.live_axis_known = [true; 2];
        self.live_axis = Vec2::ZERO;
        self.live_target_known = [true; 2];
        self.live_target = Vec2::ZERO;
    }

    /// Reset the local targets only (`resetLocal`): zeroes polar then position.
    pub fn reset_local(&mut self) {
        // C++ zeroes polar first "for the sake of the cyclical function"
        self.set_polar(Vec2::ZERO);
        self.set_position(Vec2::ZERO);
    }

    /// Remove whole cycles from the target.
    /// BUG-COMPAT: like the C++, this reads the *polar* values but applies
    /// the result as *axes* (`Pilot.cpp:721-738`).
    pub fn unwind(&mut self) {
        let mut axes = self.polar;
        if axes.x > 0.0 {
            axes.x -= axes.x.ceil();
        } else {
            axes.x += (-axes.x).ceil();
        }
        if axes.y > 0.0 {
            axes.y -= axes.y.ceil();
        } else {
            axes.y += (-axes.y).ceil();
        }
        self.set_axes(axes);
    }

    /// Adopt the live (reported) axis values as the local target.
    pub fn take_current_position(&mut self) {
        self.set_axes(self.live_axis);
    }

    // ------------------------------------------------------------ update

    /// Per-frame update: recompute the non-leading representations from the
    /// leading one, alias axes to step-quantized values, and refresh live
    /// values from the hardware-reported motion control state.
    pub fn update(&mut self, reported: [AxisReported; 2]) {
        match self.leading_control {
            LeadingControl::Position => {
                let mut position = self.position;
                let mut polar = kin::position_to_polar(position);
                // clamp max r value
                if polar.x > 1.0 {
                    polar.x = 1.0;
                    position = kin::polar_to_position(polar);
                    self.set_position(position);
                }
                self.polar = polar;

                let mut axes = self.polar_to_axes(polar);
                if self.cyclic {
                    axes = self.find_closest_axes_cycle(axes);
                }
                self.axes = axes;
            }
            LeadingControl::Polar => {
                let polar = self.polar;
                self.position = kin::polar_to_position(polar);
                let mut axes = self.polar_to_axes(polar);
                if self.cyclic {
                    axes = self.find_closest_axes_cycle(axes);
                }
                self.axes = axes;
            }
            LeadingControl::Axes => {
                let polar = self.axes_to_polar(self.axes);
                self.polar = polar;
                self.position = kin::polar_to_position(polar);
            }
        }

        // Alias the axis values to step-quantized values
        for i in 0..2 {
            let prior = self.axes[i];
            let rounded = self.steps_to_axis(self.axis_to_steps(prior, i), i);
            if prior != rounded {
                self.axes[i] = rounded;
            }
        }

        // Update live axis values from hardware reports
        for (i, report) in reported.iter().enumerate() {
            if let Some(steps) = report.current_steps {
                self.live_axis[i] = self.steps_to_axis(steps, i);
                self.live_axis_known[i] = true;
            }
            if let Some(steps) = report.target_steps {
                self.live_target[i] = self.steps_to_axis(steps, i);
                self.live_target_known[i] = true;
            }
        }
    }

    // ----------------------------------------------------------- sending

    /// Whether the current axes are stale vs. the last sent values.
    pub fn needs_push(&self, rs485_open: bool) -> bool {
        if !rs485_open {
            return false;
        }
        if self.send_periodically && self.last_send.elapsed() > SEND_PERIOD {
            return true;
        }
        self.axes.x != self.cached_sent_a || self.axes.y != self.cached_sent_b
    }

    /// Record that the current values were sent (also called for keyframes).
    pub fn notify_values_sent(&mut self) {
        self.cached_sent_a = self.axes.x;
        self.cached_sent_b = self.axes.y;
        self.last_send = Instant::now();
    }

    /// Target steps for both axes.
    pub fn axis_steps(&self) -> (i32, i32) {
        (
            self.axis_to_steps(self.axes.x, 0),
            self.axis_to_steps(self.axes.y, 1),
        )
    }

    /// Build the `{"m": [a, b]}` move message body for the current target.
    pub fn move_message(&self) -> Value {
        let (a, b) = self.axis_steps();
        commands::move_steps(a, b)
    }

    // -------------------------------------------------------- live state

    pub fn live_position(&self) -> Vec2 {
        kin::polar_to_position(self.axes_to_polar(self.live_axis))
    }

    pub fn live_target_position(&self) -> Vec2 {
        kin::polar_to_position(self.axes_to_polar(self.live_target))
    }

    /// True when hardware target == hardware position == local target,
    /// compared in step space to avoid rounding error.
    pub fn is_in_target_position(&self) -> bool {
        if !(self.live_axis_known.iter().all(|k| *k) && self.live_target_known.iter().all(|k| *k)) {
            return false;
        }
        self.axis_to_steps(self.live_target.x, 0) == self.axis_to_steps(self.live_axis.x, 0)
            && self.axis_to_steps(self.live_target.y, 1) == self.axis_to_steps(self.live_axis.y, 1)
            && self.axis_to_steps(self.axes.x, 0) == self.axis_to_steps(self.live_axis.x, 0)
            && self.axis_to_steps(self.axes.y, 1) == self.axis_to_steps(self.live_axis.y, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_cpp() {
        let pilot = Pilot::default();
        assert_eq!(pilot.leading_control, LeadingControl::Axes);
        assert!(pilot.cyclic);
        assert_eq!(pilot.microsteps_per_prism_rotation, 189_696);
        assert!(!pilot.send_periodically);
    }

    #[test]
    fn first_frame_is_stale() {
        let pilot = Pilot::default(); // axes (0,0) vs cached (-2,-2)
        assert!(pilot.needs_push(true));
        assert!(!pilot.needs_push(false), "no push when rs485 closed");
    }

    #[test]
    fn push_clears_staleness_until_change() {
        let mut pilot = Pilot::default();
        pilot.notify_values_sent();
        assert!(!pilot.needs_push(true));
        pilot.set_position(vec2(0.5, 0.0));
        pilot.update([AxisReported::default(); 2]);
        assert!(pilot.needs_push(true));
    }

    #[test]
    fn position_leading_clamps_r() {
        let mut pilot = Pilot::default();
        pilot.set_position(vec2(2.0, 0.0));
        pilot.update([AxisReported::default(); 2]);
        assert!((pilot.polar.x - 1.0).abs() < 1e-6);
        assert!((pilot.position.x - 1.0).abs() < 1e-6);
    }

    #[test]
    fn axes_leading_syncs_position() {
        let mut pilot = Pilot::default();
        // see-through axes (0.5, 0): r=... center-ish? verify a known case:
        // axes (0.5, 0.5) -> a=b=0.5 -> r = 1, thetaNorm = 0 -> theta = pi
        pilot.set_axes(vec2(0.5, 0.5));
        pilot.update([AxisReported::default(); 2]);
        assert!((pilot.polar.x - 1.0).abs() < 1e-5);
        assert!((pilot.position.x + 1.0).abs() < 1e-5, "{:?}", pilot.position);
        assert!(pilot.position.y.abs() < 1e-5);
    }

    #[test]
    fn is_in_target_position_requires_reports() {
        let mut pilot = Pilot::default();
        assert!(!pilot.is_in_target_position());
        pilot.update([
            AxisReported { current_steps: Some(0), target_steps: Some(0) },
            AxisReported { current_steps: Some(0), target_steps: Some(0) },
        ]);
        assert!(pilot.is_in_target_position());
        // hardware moves away
        pilot.update([
            AxisReported { current_steps: Some(500), target_steps: Some(0) },
            AxisReported { current_steps: Some(0), target_steps: Some(0) },
        ]);
        assert!(!pilot.is_in_target_position());
    }

    #[test]
    fn reset_marks_live_known_at_zero() {
        let mut pilot = Pilot::default();
        pilot.set_axes(vec2(1.5, -0.5));
        pilot.reset();
        assert_eq!(pilot.axes, Vec2::ZERO);
        assert!(pilot.is_in_target_position());
    }

    #[test]
    fn aliasing_quantizes_axes_to_steps() {
        let mut pilot = Pilot::default();
        pilot.set_axes(vec2(0.123456789, 0.987654321));
        pilot.update([AxisReported::default(); 2]);
        let (a, b) = pilot.axis_steps();
        // after aliasing, converting again must give identical steps
        assert_eq!(pilot.axis_to_steps(pilot.axes.x, 0), a);
        assert_eq!(pilot.axis_to_steps(pilot.axes.y, 1), b);
    }
}

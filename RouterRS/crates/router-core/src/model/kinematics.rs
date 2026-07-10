//! Pilot kinematics: conversions between position (unit disk), polar
//! (r, theta), prism axes (a, b in rotations), and motor steps.
//!
//! Ported verbatim from `Router/src/Modules/Hardware/PerPortal/Pilot.cpp`,
//! preserving the C++ implicit promotions: openFrameworks' `TWO_PI` is a
//! *double* literal, so some intermediates are computed in f64 and narrowed
//! to f32 exactly where the C++ narrows. Verified bit-exact against an
//! MSVC-compiled oracle (`tests-fixtures/pilot_oracle.cpp` /
//! `pilot-vectors.csv`); transcendental calls (atan2/cos/sin) may differ
//! from the MSVC CRT by a few ulps and are tested with a small tolerance.

use glam::{vec2, Vec2};

/// openFrameworks TWO_PI — a double literal in `ofConstants.h`.
pub const TWO_PI: f64 = 6.283_185_307_179_586_476_93;

/// openFrameworks `ofMap` (no clamping), float math.
pub fn of_map(value: f32, input_min: f32, input_max: f32, output_min: f32, output_max: f32) -> f32 {
    if (input_min - input_max).abs() < f32::EPSILON {
        output_min
    } else {
        (value - input_min) / (input_max - input_min) * (output_max - output_min) + output_min
    }
}

/// `(x, y)` -> `(r, theta)`.
pub fn position_to_polar(position: Vec2) -> Vec2 {
    let r = (position.x * position.x + position.y * position.y).sqrt();
    let theta = position.y.atan2(position.x);
    vec2(r, theta)
}

/// `(r, theta)` -> `(x, y)`.
pub fn polar_to_position(polar: Vec2) -> Vec2 {
    let (r, theta) = (polar.x, polar.y);
    vec2(r * theta.cos(), r * theta.sin())
}

/// `(r, theta)` -> normalized prism axes `(a, b)`.
///
/// "Axes norm coordinates are offset by half rotation from polar (for axes,
/// left = 0; for polar, right = 0)."
pub fn polar_to_axes(polar: Vec2, offset: f32) -> Vec2 {
    // Special case for see-through
    if polar.x == 0.0 {
        return vec2(0.5, 0.0);
    }

    let (r, theta) = (polar.x, polar.y);

    // C++: `theta / TWO_PI - 0.5f` — float / double promotes to double
    let theta_norm = theta as f64 / TWO_PI - 0.5;

    // C++: `thetaNorm - (1 - r) * 0.25 + 0.5 - offset` — `(1 - r)` in f32,
    // then promoted to double by the 0.25 literal; narrowed to f32 at the end
    let spread = ((1.0f32 - r) as f64) * 0.25;
    let a = (theta_norm - spread + 0.5 - offset as f64) as f32;
    let b = (theta_norm + spread + 0.5 + offset as f64) as f32;
    vec2(a, b)
}

/// Normalized prism axes `(a, b)` -> `(r, theta)`; ignores whole cycles.
pub fn axes_to_polar(axes: Vec2, offset: f32) -> Vec2 {
    // Bring into 0..1, keeping fmodf semantics (sign of dividend)
    let flatten_cycle = |x: f32| {
        let mut x = x % 1.0;
        if x < 0.0 {
            x += 1.0;
        }
        x
    };

    let a = flatten_cycle(axes.x + offset);
    let b = flatten_cycle(axes.y - offset);

    let mut r = 2.0 * a - 2.0 * b + 1.0;
    let mut theta_norm = (a + b - 1.0) / 2.0;

    // "Somehow this seems to work" (sic)
    if r > 1.0 {
        r = 1.0 - (r - 1.0);
    }
    if r < 0.0 {
        theta_norm += 0.5;
        r = -r;
    }

    // C++: `(thetaNorm + 0.5f) * TWO_PI` — f32 sum promoted to double
    let theta = ((theta_norm + 0.5) as f64 * TWO_PI) as f32;
    vec2(r, theta)
}

/// Pick the axis cycle closest to `current`.
///
/// BUG-COMPAT: both components measure distance from `current[0]` (axis A),
/// exactly as `Pilot.cpp:839-840`. Kept for 1:1 behavior with the C++ app.
pub fn find_closest_axes_cycle(target: Vec2, current: Vec2) -> Vec2 {
    vec2(
        target.x + (current.x - target.x).round(),
        target.y + (current.x - target.y).round(),
    )
}

/// Axis value (rotations) -> motor microsteps. Axis index 1 (B) is inverted.
/// C++ truncates the f32 result toward zero when converting to `Steps`.
pub fn axis_to_steps(axis_value: f32, axis_index: usize, microsteps_per_prism_rotation: i32) -> i32 {
    let invert = if axis_index == 1 { -1.0f32 } else { 1.0f32 };
    of_map(
        axis_value,
        0.0,
        1.0,
        0.0,
        invert * microsteps_per_prism_rotation as f32,
    ) as i32
}

/// Motor microsteps -> axis value (rotations).
pub fn steps_to_axis(steps: i32, axis_index: usize, microsteps_per_prism_rotation: i32) -> f32 {
    let invert = if axis_index == 1 { -1.0f32 } else { 1.0f32 };
    of_map(
        steps as f32,
        0.0,
        invert * microsteps_per_prism_rotation as f32,
        0.0,
        1.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_proto::constants::MOTION_MICROSTEPS_PER_PRISM_ROTATION;

    const MICRO: i32 = MOTION_MICROSTEPS_PER_PRISM_ROTATION;

    fn ulp_distance(a: f32, b: f32) -> u32 {
        if a == b {
            return 0; // covers -0.0 == 0.0
        }
        if a.is_nan() && b.is_nan() {
            return 0;
        }
        let (ai, bi) = (a.to_bits() as i32, b.to_bits() as i32);
        // map to monotonic ordering
        let am = if ai < 0 { i32::MIN.wrapping_sub(ai) } else { ai };
        let bm = if bi < 0 { i32::MIN.wrapping_sub(bi) } else { bi };
        am.wrapping_sub(bm).unsigned_abs()
    }

    /// Bit-exact comparison against the MSVC-compiled verbatim copy of the
    /// C++ math (`tests-fixtures/pilot_oracle.cpp`). Only outputs that pass
    /// through transcendental CRT calls (atan2/cos/sin) get a small ulp
    /// tolerance.
    #[test]
    fn golden_vectors_match_msvc_oracle() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests-fixtures/pilot-vectors.csv"
        );
        let data = std::fs::read_to_string(path).expect(
            "pilot-vectors.csv missing — build and run tests-fixtures/pilot_oracle.cpp",
        );
        let f = |s: &str| f32::from_bits(u32::from_str_radix(s, 16).unwrap());
        let mut count = 0usize;
        let mut max_transcendental_ulp = 0u32;
        for line in data.lines().skip(1) {
            let cols: Vec<&str> = line.split(',').collect();
            count += 1;
            match cols[0] {
                "polarToAxes" => {
                    let out = polar_to_axes(vec2(f(cols[1]), f(cols[2])), f(cols[3]));
                    assert_eq!(out.x.to_bits(), f(cols[4]).to_bits(), "{line}");
                    assert_eq!(out.y.to_bits(), f(cols[5]).to_bits(), "{line}");
                }
                "axesToPolar" => {
                    let out = axes_to_polar(vec2(f(cols[1]), f(cols[2])), f(cols[3]));
                    assert_eq!(out.x.to_bits(), f(cols[4]).to_bits(), "{line}");
                    assert_eq!(out.y.to_bits(), f(cols[5]).to_bits(), "{line}");
                }
                "positionToPolar" => {
                    let out = position_to_polar(vec2(f(cols[1]), f(cols[2])));
                    // r = sqrt: correctly rounded, must be exact
                    assert_eq!(out.x.to_bits(), f(cols[4]).to_bits(), "{line}");
                    // theta = atan2: CRT-dependent
                    let ulp = ulp_distance(out.y, f(cols[5]));
                    max_transcendental_ulp = max_transcendental_ulp.max(ulp);
                    assert!(ulp <= 4, "atan2 differs by {ulp} ulp: {line}");
                }
                "polarToPosition" => {
                    let out = polar_to_position(vec2(f(cols[1]), f(cols[2])));
                    for (got, want) in [(out.x, f(cols[4])), (out.y, f(cols[5]))] {
                        let ulp = ulp_distance(got, want);
                        max_transcendental_ulp = max_transcendental_ulp.max(ulp);
                        assert!(ulp <= 4, "cos/sin differs by {ulp} ulp: {line}");
                    }
                }
                "findClosestAxesCycle" => {
                    let out =
                        find_closest_axes_cycle(vec2(f(cols[1]), f(cols[2])), vec2(f(cols[3]), 0.0));
                    assert_eq!(out.x.to_bits(), f(cols[4]).to_bits(), "{line}");
                    assert_eq!(out.y.to_bits(), f(cols[5]).to_bits(), "{line}");
                }
                "axisToSteps" => {
                    let steps = axis_to_steps(
                        f(cols[1]),
                        cols[2].parse().unwrap(),
                        cols[3].parse().unwrap(),
                    );
                    assert_eq!(steps, cols[4].parse::<i32>().unwrap(), "{line}");
                }
                "stepsToAxis" => {
                    let axis = steps_to_axis(
                        cols[1].parse().unwrap(),
                        cols[2].parse().unwrap(),
                        cols[3].parse().unwrap(),
                    );
                    assert_eq!(axis.to_bits(), f(cols[4]).to_bits(), "{line}");
                }
                other => panic!("unknown oracle function {other}"),
            }
        }
        assert!(count > 6000, "expected the full golden table, got {count} rows");
        eprintln!("golden vectors: {count} rows, max transcendental ulp diff = {max_transcendental_ulp}");
    }

    #[test]
    fn see_through_special_case() {
        assert_eq!(polar_to_axes(vec2(0.0, 1.234), 0.1), vec2(0.5, 0.0));
    }

    #[test]
    fn polar_roundtrip_through_axes() {
        // polar -> axes -> polar must reproduce (r, theta) for valid polar
        // inputs (r in 0..1, theta in -pi..pi maps to theta or theta+2pi)
        for &r in &[0.05f32, 0.25, 0.5, 0.75, 1.0] {
            for &theta in &[-3.0f32, -1.5, -0.5, 0.0, 0.5, 1.5, 3.0] {
                for &offset in &[0.0f32, 0.05, -0.1] {
                    let axes = polar_to_axes(vec2(r, theta), offset);
                    let back = axes_to_polar(axes, offset);
                    assert!((back.x - r).abs() < 1e-4, "r: {r} theta {theta} offset {offset} -> {back:?}");
                    // theta comes back in 0..2pi (or equivalent modulo 2pi)
                    let dt = (back.y - theta).rem_euclid(std::f32::consts::TAU);
                    let dt = dt.min(std::f32::consts::TAU - dt);
                    assert!(dt < 1e-3, "theta: {r}/{theta} offset {offset} -> {back:?} (dt {dt})");
                }
            }
        }
    }

    #[test]
    fn axis_b_is_inverted_in_step_space() {
        assert_eq!(axis_to_steps(0.5, 0, MICRO), MICRO / 2);
        assert_eq!(axis_to_steps(0.5, 1, MICRO), -MICRO / 2);
        assert_eq!(steps_to_axis(-MICRO / 2, 1, MICRO), 0.5);
    }
}

//! Deliberate manual bench surface; commands remain worker-owned.
use crate::schema;
use av_gui_bus::{Bus, SchemaBuilder};
use av_operator_app::{ControlSurfaceConfig, ControlSurfaceFieldOverride};

pub fn declare(b: &mut SchemaBuilder) -> Result<(), String> {
    let result = (|| {
        b.param("/manual/axis")
            .enumeration(0, &[(0, "Axis A"), (1, "Axis B")])
            .label("Axis")
            .register()?;
        b.param("/manual/increment")
            .f64(0.01)
            .range(0.0001, 0.25)
            .step(0.001)
            .precision(4)
            .label("Jog increment [rev]")
            .register()?;
        b.param("/manual/target")
            .f64(0.0)
            .range(-1000.0, 1000.0)
            .step(0.01)
            .precision(4)
            .label("Target [rev]")
            .register()?;
        b.param("/manual/position")
            .text("Unknown")
            .read_only()
            .label("Actual position [rev]")
            .register()?;
        b.param("/manual/readout")
            .text("Unknown\nIdle")
            .read_only()
            .label("Actual position [rev]")
            .register()?;
        b.param("/manual/connected")
            .bool(false)
            .read_only()
            .label("Command route connected")
            .register()?;
        b.param("/manual/status")
            .text("Idle")
            .read_only()
            .label("Operation")
            .register()?;
        Ok::<_, av_gui_bus::BusError>(())
    })();
    result.map_err(|e| e.to_string())
}

pub fn config() -> ControlSurfaceConfig {
    let mut c = ControlSurfaceConfig::default();
    c.default_prefix("/");
    c.operator_layout = true;
    let fields = [
        ("/manual/axis", "Axis", false, false),
        ("/manual/increment", "Step / rev", false, false),
        (
            "/motion/profile/max_velocity",
            "Speed / ustep/s",
            false,
            false,
        ),
        ("/manual/readout", "Actual position [rev]", false, true),
        ("/actions/manual_jog_minus", "Jog -", true, false),
        ("/actions/manual_jog_plus", "Jog +", true, false),
        ("/actions/manual_home", "Home", true, false),
        ("/actions/manual_stop", "STOP / Escape", true, false),
        ("/motion/route", "Command via", false, false),
        ("/motion/profile/acceleration", "Acceleration", false, false),
        (
            "/motion/profile/min_velocity",
            "Minimum velocity",
            false,
            false,
        ),
        ("/manual/target", "Target [rev]", false, false),
        ("/actions/manual_move", "Move to target", true, false),
        ("/manual/connected", "Connected", false, true),
        ("/manual/status", "Operation", false, true),
        (
            "/actions/manual_stop_settings",
            "STOP / Escape",
            true,
            false,
        ),
    ];
    for (slot, (path, label, action, display)) in fields.into_iter().enumerate() {
        c.include_prefix(path);
        c.overrides.insert(
            path.into(),
            ControlSurfaceFieldOverride {
                slot: Some(slot as u16),
                action,
                display,
                label: Some(label.into()),
                ..Default::default()
            },
        );
    }
    c
}

pub fn axis(bus: &Bus) -> bench_core::dut::Axis {
    if schema::get_u32(bus, bus.id_of("/manual/axis").unwrap()) == 1 {
        bench_core::dut::Axis::B
    } else {
        bench_core::dut::Axis::A
    }
}
pub fn number(bus: &Bus, path: &str) -> f64 {
    schema::get_f64(bus, bus.id_of(path).unwrap())
}

//! What is plugged into this machine, and how to open it.
//!
//! Everything here is a *report*, never a guess. A serial port whose identity cannot be
//! established is listed as unknown rather than being labelled by heuristics on its name — a
//! bench that confidently mislabels the port you are about to flash is worse than one that
//! admits it does not know.

use serde::Serialize;

use crate::transport::line::LineLink;
use crate::transport::rs485::{DEFAULT_TARGET, Rs485Link};
use crate::transport::{Link, LinkEvent, LinkKind};

/// A serial port, as the operating system reports it.
#[derive(Debug, Clone, Serialize)]
pub struct PortEntry {
    pub name: String,
    /// `usb`, `bluetooth`, `pci` or `unknown`, straight from the OS.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
}

/// A debug probe, if SWD support is compiled in.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeEntry {
    /// The selector to pass back when choosing this probe.
    pub identifier: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    /// Backend family as reported by probe-rs (for example `STLink`).
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Survey {
    pub ports: Vec<PortEntry>,
    pub probes: Vec<ProbeEntry>,
    /// True when this build cannot see probes at all, so an empty list is not read as "none
    /// attached".
    pub swd_support: bool,
}

/// Everything this machine offers a bench right now.
pub fn survey() -> Survey {
    Survey {
        ports: ports(),
        probes: probes(),
        swd_support: cfg!(feature = "swd"),
    }
}

fn ports() -> Vec<PortEntry> {
    let Ok(found) = serialport::available_ports() else {
        return Vec::new();
    };
    found
        .into_iter()
        .map(|port| {
            let (kind, product, serial_number) = match &port.port_type {
                serialport::SerialPortType::UsbPort(info) => {
                    ("usb", info.product.clone(), info.serial_number.clone())
                }
                serialport::SerialPortType::BluetoothPort => ("bluetooth", None, None),
                serialport::SerialPortType::PciPort => ("pci", None, None),
                serialport::SerialPortType::Unknown => ("unknown", None, None),
            };
            PortEntry {
                name: port.port_name,
                kind: kind.to_string(),
                product,
                serial_number,
            }
        })
        .collect()
}

#[cfg(feature = "swd")]
fn probes() -> Vec<ProbeEntry> {
    portal_swd::probe::list_probes()
        .into_iter()
        .map(|probe| ProbeEntry {
            // `id` is the selector `ProbeRsRig::new` takes, so it is the field worth printing:
            // whatever this says can be pasted straight back as `--probe`.
            identifier: probe.id.clone(),
            name: Some(probe.name.clone()),
            serial_number: probe.serial.clone(),
            kind: probe.kind.clone(),
        })
        .collect()
}

#[cfg(not(feature = "swd"))]
fn probes() -> Vec<ProbeEntry> {
    Vec::new()
}

/// Build a link of the given kind for an endpoint.
pub fn open_link(kind: LinkKind, endpoint: &str) -> Result<Box<dyn Link>, String> {
    Ok(match kind {
        LinkKind::Vcp | LinkKind::BenchAscii => Box::new(LineLink::serial(kind, endpoint)),
        LinkKind::Rs485Serial | LinkKind::Rs485Tcp => {
            Box::new(Rs485Link::new(kind, endpoint, DEFAULT_TARGET))
        }
        LinkKind::Sim => return Err("the simulated link lands with the engine".into()),
    })
}

/// One event as a JSON line, for `ptb listen` and the NDJSON session file.
///
/// Hand-written rather than derived so the wire names stay stable if the Rust enum is
/// refactored: a report read months later must not depend on today's variant names.
pub fn event_json(event: &LinkEvent) -> String {
    let value = match event {
        LinkEvent::Identified {
            firmware,
            version,
            ratio,
            usteps_per_rev,
            banner,
        } => {
            serde_json::json!({
                "event": "identified",
                "firmware": format!("{firmware:?}").to_lowercase(),
                "version": version,
                "ratio": format!("{ratio:?}"),
                "usteps_per_rev": usteps_per_rev,
                "banner": banner,
            })
        }
        LinkEvent::Position {
            axis,
            position,
            target,
        } => serde_json::json!({
            "event": "position",
            "axis": axis.suffix().to_lowercase(),
            "position": position,
            "target": target,
        }),
        LinkEvent::HealthReport { axis, health } => serde_json::json!({
            "event": "health",
            "axis": axis.suffix().to_lowercase(),
            "measure_cycle_ok": health.measure_cycle_ok,
            "switches_ok": health.switches_ok,
            "backlash_ok": health.backlash_ok,
            "home_ok": health.home_ok,
        }),
        LinkEvent::Uptime { seconds } => {
            serde_json::json!({ "event": "uptime", "seconds": seconds })
        }
        LinkEvent::Log {
            level,
            message,
            firmware_ms,
        } => serde_json::json!({
            "event": "log",
            "level": level,
            "message": message,
            "firmware_ms": firmware_ms,
        }),
        LinkEvent::Sensor { active, threshold } => serde_json::json!({
            "event": "sensor",
            "active": active,
            "threshold": threshold,
        }),
        LinkEvent::Token { kind, fields } => serde_json::json!({
            "event": "token",
            "kind": kind.to_string(),
            "fields": fields,
        }),
        LinkEvent::PeerSeen { source } => {
            serde_json::json!({ "event": "peer_seen", "source": source })
        }
        LinkEvent::Ack { source } => serde_json::json!({ "event": "ack", "source": source }),
        LinkEvent::Fault(detail) => serde_json::json!({ "event": "fault", "detail": detail }),
    };
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dut::{Axis, Health};

    /// A survey on a machine with nothing attached must still say whether it *could* have seen
    /// a probe. An empty list plus no capability flag reads as "none attached", which is a
    /// different and much more misleading claim.
    #[test]
    fn a_survey_states_whether_it_can_see_probes_at_all() {
        let survey = survey();
        assert_eq!(survey.swd_support, cfg!(feature = "swd"));
    }

    #[test]
    fn events_serialise_with_stable_names() {
        let json = event_json(&LinkEvent::Position {
            axis: Axis::A,
            position: 47_426,
            target: Some(47_430),
        });
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "position");
        assert_eq!(parsed["axis"], "a");
        assert_eq!(parsed["position"], 47_426);

        let json = event_json(&LinkEvent::HealthReport {
            axis: Axis::B,
            health: Health {
                measure_cycle_ok: true,
                switches_ok: true,
                backlash_ok: false,
                home_ok: true,
            },
        });
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event"], "health");
        assert_eq!(parsed["backlash_ok"], false);
    }

    /// A fault must survive into the record. Dropping it would make a session file that says a
    /// run was clean when it was not.
    #[test]
    fn faults_carry_their_detail() {
        let json = event_json(&LinkEvent::Fault("short status record".into()));
        assert!(json.contains("short status record"));
    }
}

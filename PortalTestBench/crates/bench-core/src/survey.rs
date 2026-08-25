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

/// One physical device, as macOS names it twice.
///
/// Every serial device on macOS is enumerated as both `/dev/cu.NAME` (the *callout* device) and
/// `/dev/tty.NAME` (the dial-in device). They are the same hardware. The distinction matters:
/// opening `tty.*` blocks until carrier detect, which for a USB CDC adapter may never assert,
/// so the callout device is the one a bench wants. Anything else -- a Windows `COM7`, a Linux
/// `/dev/ttyACM0` -- is returned unchanged and compares as itself.
fn device_identity(name: &str) -> &str {
    name.strip_prefix("/dev/cu.")
        .or_else(|| name.strip_prefix("/dev/tty."))
        .unwrap_or(name)
}

/// Find the VCOM port belonging to a selected debug probe.
///
/// ST-Link exposes SWD and VCOM as separate USB interfaces carrying the same serial number.
/// Matching that OS-reported identity is safe; choosing the first COM port is not, because a
/// bench commonly also has an RS485 adapter attached.
///
/// The refusal to guess is deliberate and is kept. What it must not do is fire on a *false*
/// ambiguity: on macOS the probe's VCOM always matches twice, `cu.` and `tty.`, so the plain
/// count made post-flash auto-attach refuse on every Mac -- always, not occasionally. Collapsing
/// the two names for one device first restores the check to the case it was written for: two
/// genuinely different devices claiming one serial number, which is a fault worth stopping on.
pub fn paired_vcom_port(survey: &Survey, probe_identifier: &str) -> Result<String, String> {
    let probe = survey
        .probes
        .iter()
        .find(|probe| probe.identifier == probe_identifier)
        .ok_or_else(|| format!("selected probe {probe_identifier:?} is not present"))?;
    let serial = probe
        .serial_number
        .as_deref()
        .filter(|serial| !serial.is_empty())
        .ok_or_else(|| {
            format!("selected probe {probe_identifier:?} reports no USB serial number")
        })?;
    let matches = survey
        .ports
        .iter()
        .filter(|port| {
            port.serial_number
                .as_deref()
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(serial))
        })
        .collect::<Vec<_>>();
    let mut devices = matches
        .iter()
        .map(|port| device_identity(&port.name))
        .collect::<Vec<_>>();
    devices.sort_unstable();
    devices.dedup();
    match (matches.as_slice(), devices.as_slice()) {
        ([], _) => Err(format!(
            "no COM port reports the selected probe serial {serial}"
        )),
        // One device, however many names the OS gave it. Prefer the callout node when it is
        // among them; otherwise take the only name on offer.
        (_, [_]) => Ok(matches
            .iter()
            .find(|port| port.name.starts_with("/dev/cu."))
            .unwrap_or(&matches[0])
            .name
            .clone()),
        _ => Err(format!(
            "{} devices report the selected probe serial {serial}; refusing to guess",
            devices.len()
        )),
    }
}

/// The identity of what is attached, as a set two scans can be compared by.
///
/// Deliberately narrower than the survey itself. A `product` string the OS spells differently
/// between two enumerations, or a probe list that comes back in another order, is not a replug
/// and must not be reported as one -- a bench that announced a hardware change every second
/// would train the operator to ignore the one time it mattered. What counts is *which devices
/// are there*: a port by its device name and USB serial, a probe by the selector it is opened
/// with.
///
/// `swd_support` is compile-time and deliberately absent: it cannot change while the process
/// runs, so including it would only add a constant to every comparison.
pub fn identity_set(survey: &Survey) -> Vec<String> {
    let mut lines = Vec::with_capacity(survey.ports.len() + survey.probes.len());
    for port in &survey.ports {
        lines.push(format!(
            "port\u{1f}{}\u{1f}{}",
            port.name,
            port.serial_number.as_deref().unwrap_or("")
        ));
    }
    for probe in &survey.probes {
        lines.push(format!("probe\u{1f}{}", probe.identifier));
    }
    lines.sort_unstable();
    lines.dedup();
    lines
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
///
/// The simulated kind ignores `endpoint` and names itself, because there is nothing to address:
/// it is `router_link::sim::SimBus` -- an in-process model of one module running the PortalFW
/// protocol -- behind an ordinary [`Rs485Link`]. Everything downstream of the wire is therefore
/// the production code path, which is the point: a bench that simulated its own decoder would
/// prove nothing about the decoder.
pub fn open_link(kind: LinkKind, endpoint: &str) -> Result<Box<dyn Link>, String> {
    Ok(match kind {
        LinkKind::Vcp | LinkKind::BenchAscii => Box::new(LineLink::serial(kind, endpoint)),
        LinkKind::Rs485Serial | LinkKind::Rs485Tcp => {
            Box::new(Rs485Link::new(kind, endpoint, DEFAULT_TARGET))
        }
        LinkKind::Sim => Box::new(Rs485Link::new(kind, "simulated", DEFAULT_TARGET)),
    })
}

/// One event as a JSON line, for `ptb listen` and the NDJSON session file.
///
/// Hand-written rather than derived so the wire names stay stable if the Rust enum is
/// refactored: a report read months later must not depend on today's variant names.
pub fn event_json(event: &LinkEvent) -> String {
    let value = match event {
        LinkEvent::DirectMode { mode, detail } => serde_json::json!({
            "event": "direct_mode", "mode": mode.name(), "detail": detail,
        }),
        LinkEvent::SurveyBegin { config, expected } => serde_json::json!({
            "event": "survey_begin", "config": config, "expected": expected,
        }),
        LinkEvent::SurveySample(sample) => serde_json::json!({
            "event": "survey_sample", "sample": sample,
        }),
        LinkEvent::SurveyEnd { aborted, detail } => serde_json::json!({
            "event": "survey_end", "aborted": aborted, "detail": detail,
        }),
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
        LinkEvent::Provisioning { serial } => {
            serde_json::json!({ "event": "provisioning", "serial": serial })
        }
        LinkEvent::Settings {
            current_ma,
            full_current_home_recovery,
            source,
        } => serde_json::json!({
            "event": "settings", "current_ma": current_ma,
            "full_current_home_recovery": full_current_home_recovery, "source": source,
        }),
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

    fn pairing_survey(ports: Vec<PortEntry>) -> Survey {
        Survey {
            ports,
            probes: vec![ProbeEntry {
                identifier: "0483:374b:PROBE123".into(),
                name: Some("ST-Link V2-1".into()),
                serial_number: Some("PROBE123".into()),
                kind: "ST-LINK".into(),
            }],
            swd_support: true,
        }
    }

    #[test]
    fn vcom_is_paired_by_probe_serial_not_port_order() {
        let survey = pairing_survey(vec![
            PortEntry {
                name: "COM5".into(),
                kind: "usb".into(),
                product: Some("USB RS485 adapter".into()),
                serial_number: Some("ADAPTER9".into()),
            },
            PortEntry {
                name: "COM3".into(),
                kind: "usb".into(),
                product: Some("ST-Link Virtual COM Port".into()),
                serial_number: Some("probe123".into()),
            },
        ]);
        assert_eq!(
            paired_vcom_port(&survey, "0483:374b:PROBE123"),
            Ok("COM3".into())
        );
    }

    /// The regression that made post-flash auto-attach refuse on every Mac: one ST-Link, two
    /// names for its VCOM, counted as two candidates.
    #[test]
    fn macos_cu_and_tty_names_are_one_device_and_the_callout_node_wins() {
        let survey = pairing_survey(vec![
            PortEntry {
                name: "/dev/tty.usbmodem5103".into(),
                kind: "usb".into(),
                product: Some("STM32 STLink".into()),
                serial_number: Some("PROBE123".into()),
            },
            PortEntry {
                name: "/dev/cu.usbmodem5103".into(),
                kind: "usb".into(),
                product: Some("STM32 STLink".into()),
                serial_number: Some("PROBE123".into()),
            },
        ]);
        assert_eq!(
            paired_vcom_port(&survey, "0483:374b:PROBE123"),
            Ok("/dev/cu.usbmodem5103".into())
        );
    }

    /// Two genuinely different devices claiming one serial is still a refusal. Collapsing the
    /// macOS pair must not have collapsed this.
    #[test]
    fn two_distinct_devices_sharing_a_serial_still_refuse_to_guess() {
        let survey = pairing_survey(vec![
            PortEntry {
                name: "/dev/cu.usbmodem5103".into(),
                kind: "usb".into(),
                product: Some("STM32 STLink".into()),
                serial_number: Some("PROBE123".into()),
            },
            PortEntry {
                name: "/dev/cu.usbmodem9910".into(),
                kind: "usb".into(),
                product: Some("STM32 STLink".into()),
                serial_number: Some("PROBE123".into()),
            },
        ]);
        assert!(
            paired_vcom_port(&survey, "0483:374b:PROBE123")
                .unwrap_err()
                .contains("refusing to guess")
        );
    }

    /// Two scans of the same machine compare equal even when the OS spells them differently.
    ///
    /// This is what stops the bench announcing a hardware change every second: the identity set
    /// is what the worker diffs, and if it moved on `product` churn or on probe-list order, the
    /// page would re-fetch forever and the rescan log line would cry wolf.
    #[test]
    fn identity_set_ignores_product_churn_and_probe_order() {
        let first = pairing_survey(vec![
            PortEntry {
                name: "/dev/cu.usbmodem5103".into(),
                kind: "usb".into(),
                product: Some("STM32 STLink".into()),
                serial_number: Some("PROBE123".into()),
            },
            PortEntry {
                name: "/dev/cu.usbserial-B003ASAG".into(),
                kind: "usb".into(),
                product: Some("FT232R USB UART".into()),
                serial_number: Some("B003ASAG".into()),
            },
        ]);
        let mut second = pairing_survey(vec![
            PortEntry {
                name: "/dev/cu.usbserial-B003ASAG".into(),
                kind: "usb".into(),
                // The same adapter, named differently by a second enumeration.
                product: Some("USB Serial".into()),
                serial_number: Some("B003ASAG".into()),
            },
            PortEntry {
                name: "/dev/cu.usbmodem5103".into(),
                kind: "usb".into(),
                product: None,
                serial_number: Some("PROBE123".into()),
            },
        ]);
        second.probes.push(second.probes[0].clone());
        assert_eq!(identity_set(&first), identity_set(&second));
    }

    /// And the case the whole mechanism exists for: a probe appearing is a change.
    #[test]
    fn a_probe_appearing_changes_the_identity_set() {
        let before = Survey {
            ports: Vec::new(),
            probes: Vec::new(),
            swd_support: true,
        };
        let after = pairing_survey(Vec::new());
        assert_ne!(identity_set(&before), identity_set(&after));
        assert_eq!(identity_set(&after), vec!["probe\u{1f}0483:374b:PROBE123"]);
    }

    #[test]
    fn vcom_pairing_refuses_to_guess_from_a_product_name() {
        let survey = pairing_survey(vec![PortEntry {
            name: "COM9".into(),
            kind: "usb".into(),
            product: Some("ST-Link Virtual COM Port".into()),
            serial_number: Some("SOMEONE_ELSE".into()),
        }]);
        assert!(
            paired_vcom_port(&survey, "0483:374b:PROBE123")
                .unwrap_err()
                .contains("no COM port")
        );
    }
}

//! Which attached port is a repeater, and which repeater it is.
//!
//! An ESP32-C3's USB-Serial-JTAG peripheral enumerates as `303A:1001` and puts the chip's MAC
//! in the USB serial-number string. Both are true of a virgin part in ROM download mode and of
//! one running our firmware, so this needs no query and works on a board that has never been
//! flashed -- which is the case that matters, because that board has nothing to answer with.

use crate::survey::{PortEntry, Survey};
use serde::Serialize;

/// Espressif's vendor id.
pub const ESPRESSIF_VID: u16 = 0x303A;

/// The USB-Serial-JTAG peripheral. `espflash` keys its whole reset strategy off this pid, so
/// it has to reach the flasher as well as the picker.
pub const USB_SERIAL_JTAG_PID: u16 = 0x1001;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepeaterPort {
    /// The callout device to open.
    pub name: String,
    /// The MAC, as the USB serial-number string spells it (`f8:5b:1b:ed:8d:a4`).
    pub mac: Option<String>,
    pub product: Option<String>,
}

impl RepeaterPort {
    /// What the picker calls this row. The MAC, because that is the only thing that
    /// distinguishes two repeaters on one bench.
    pub fn label(&self) -> String {
        match &self.mac {
            Some(mac) => mac.clone(),
            None => self.name.clone(),
        }
    }
}

/// One physical device, as macOS names it twice.
///
/// The same rule `survey::paired_vcom_port` applies: `/dev/cu.NAME` and `/dev/tty.NAME` are
/// one piece of hardware, and only the callout node is openable without waiting on carrier
/// detect that a USB CDC device may never assert.
fn device_identity(name: &str) -> &str {
    name.strip_prefix("/dev/cu.")
        .or_else(|| name.strip_prefix("/dev/tty."))
        .unwrap_or(name)
}

fn is_repeater(port: &PortEntry) -> bool {
    port.vid == Some(ESPRESSIF_VID) && port.pid == Some(USB_SERIAL_JTAG_PID)
}

/// Every attached ESP32-C3 USB-Serial-JTAG interface, one row per physical device.
pub fn candidates(survey: &Survey) -> Vec<RepeaterPort> {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<RepeaterPort> = Vec::new();
    for port in survey.ports.iter().filter(|port| is_repeater(port)) {
        let identity = device_identity(&port.name).to_string();
        if let Some(position) = seen.iter().position(|other| *other == identity) {
            // Prefer the callout node when both names for one device turn up.
            if port.name.starts_with("/dev/cu.") {
                out[position].name = port.name.clone();
            }
            continue;
        }
        seen.push(identity);
        out.push(RepeaterPort {
            name: port.name.clone(),
            mac: port.serial_number.clone().map(|mac| mac.to_lowercase()),
            product: port.product.clone(),
        });
    }
    out
}

/// The one to use, or a refusal that names the problem.
///
/// `hint` is whatever the operator chose -- a MAC or a device path -- and is empty when they
/// have chosen nothing. With one repeater attached that is fine and it is used. With two it
/// is not: **this refuses to guess**, which is the same answer `paired_vcom_port` gives to
/// the same question, because the cost of guessing is writing firmware to the wrong board.
pub fn choose_port(candidates: &[RepeaterPort], hint: &str) -> Result<RepeaterPort, String> {
    let hint = hint.trim();
    if !hint.is_empty() {
        let wanted = hint.to_lowercase();
        if let Some(found) = candidates.iter().find(|port| {
            port.mac.as_deref() == Some(wanted.as_str())
                || port.name == hint
                || device_identity(&port.name) == device_identity(hint)
        }) {
            return Ok(found.clone());
        }
        return Err(format!(
            "the selected repeater {hint} is not attached. Rescan, or choose another."
        ));
    }
    match candidates {
        [] => Err(
            "no ESP32-C3 is attached. Plug the repeater's USB cable into this machine.".to_string(),
        ),
        [only] => Ok(only.clone()),
        many => Err(format!(
            "{} repeaters are attached ({}); select the one to provision. The bench will not \
             guess.",
            many.len(),
            many.iter()
                .map(RepeaterPort::label)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// `f8:5b:1b:ed:8d:a4` and the like, in either case and with any of the usual separators, to
/// the six bytes `RepeaterTarget::Mac` wants.
pub fn mac_bytes(mac: &str) -> Option<[u8; 6]> {
    let digits: Vec<u8> = mac
        .bytes()
        .filter(|b| b.is_ascii_hexdigit())
        .map(|b| (b as char).to_digit(16).unwrap() as u8)
        .collect();
    if digits.len() != 12 {
        return None;
    }
    let mut out = [0u8; 6];
    for (index, pair) in digits.chunks(2).enumerate() {
        out[index] = pair[0] << 4 | pair[1];
    }
    Some(out)
}

/// The inverse, in the spelling the firmware and the USB descriptor both use.
pub fn mac_string(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::survey::ProbeEntry;

    fn port(name: &str, vid: u16, pid: u16, serial: Option<&str>) -> PortEntry {
        PortEntry {
            name: name.into(),
            kind: "usb".into(),
            product: Some("USB JTAG/serial debug unit".into()),
            serial_number: serial.map(str::to_string),
            vid: Some(vid),
            pid: Some(pid),
        }
    }

    fn survey(ports: Vec<PortEntry>) -> Survey {
        Survey {
            ports,
            probes: Vec::<ProbeEntry>::new(),
            swd_support: true,
        }
    }

    #[test]
    fn only_the_usb_serial_jtag_interface_is_a_candidate() {
        let survey = survey(vec![
            port(
                "/dev/cu.usbserial-B003ASAG",
                0x0403,
                0x6001,
                Some("B003ASAG"),
            ),
            port("/dev/cu.usbmodem8411403", 0x0483, 0x374b, Some("066AFF32")),
            port(
                "/dev/cu.usbmodem8411301",
                ESPRESSIF_VID,
                USB_SERIAL_JTAG_PID,
                Some("F8:5B:1B:ED:8D:A4"),
            ),
        ]);
        let found = candidates(&survey);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "/dev/cu.usbmodem8411301");
        // Lower-cased, because the firmware prints it that way and the two have to compare.
        assert_eq!(found[0].mac.as_deref(), Some("f8:5b:1b:ed:8d:a4"));
    }

    #[test]
    fn the_two_names_macos_gives_one_device_are_one_row() {
        let survey = survey(vec![
            port(
                "/dev/tty.usbmodem8411301",
                ESPRESSIF_VID,
                USB_SERIAL_JTAG_PID,
                Some("F8:5B:1B:ED:8D:A4"),
            ),
            port(
                "/dev/cu.usbmodem8411301",
                ESPRESSIF_VID,
                USB_SERIAL_JTAG_PID,
                Some("F8:5B:1B:ED:8D:A4"),
            ),
        ]);
        let found = candidates(&survey);
        assert_eq!(found.len(), 1, "{found:?}");
        // And the callout node is the one that can actually be opened.
        assert_eq!(found[0].name, "/dev/cu.usbmodem8411301");
    }

    #[test]
    fn one_attached_repeater_needs_no_choosing_and_two_do() {
        let one = vec![RepeaterPort {
            name: "/dev/cu.a".into(),
            mac: Some("f8:5b:1b:ed:8d:a4".into()),
            product: None,
        }];
        assert_eq!(choose_port(&one, "").unwrap().name, "/dev/cu.a");

        let mut two = one.clone();
        two.push(RepeaterPort {
            name: "/dev/cu.b".into(),
            mac: Some("f8:5b:1b:f4:18:ec".into()),
            product: None,
        });
        let error = choose_port(&two, "").unwrap_err();
        assert!(error.contains("will not guess"), "{error}");
        // Naming one resolves it, in either case.
        assert_eq!(
            choose_port(&two, "F8:5B:1B:F4:18:EC").unwrap().name,
            "/dev/cu.b"
        );
        // And naming one that has gone says so rather than falling back to the other.
        let error = choose_port(&two, "f8:5b:1b:00:00:01").unwrap_err();
        assert!(error.contains("is not attached"), "{error}");
    }

    #[test]
    fn nothing_attached_says_so_rather_than_failing_later() {
        let error = choose_port(&[], "").unwrap_err();
        assert!(error.contains("no ESP32-C3 is attached"), "{error}");
    }

    #[test]
    fn a_mac_round_trips_through_the_form_repeater_target_wants() {
        let bytes = mac_bytes("F8:5B:1B:ED:8D:A4").unwrap();
        assert_eq!(bytes, [0xf8, 0x5b, 0x1b, 0xed, 0x8d, 0xa4]);
        assert_eq!(mac_string(&bytes), "f8:5b:1b:ed:8d:a4");
        // Separators are decoration; a short one is not a MAC.
        assert_eq!(mac_bytes("f85b1beD8da4"), Some(bytes));
        assert_eq!(mac_bytes("f8:5b:1b:ed:8d"), None);
    }
}

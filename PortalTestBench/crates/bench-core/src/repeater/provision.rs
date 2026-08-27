//! What a repeater says about itself, and whether that is what was asked for.
//!
//! Pure. No port, no bus, no clock of its own -- so every verdict this bench reaches about a
//! repeater is a unit test rather than a bench session, and the mutation check has somewhere
//! to bite.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepeaterError {
    #[error("{port}: {detail}")]
    Console { port: String, detail: String },
    #[error("{port} did not answer with {wanted} in time")]
    ConsoleSilent { port: String, wanted: String },
    #[error("the repeater refused `{command}`: {detail}")]
    ConsoleRefused { command: String, detail: String },
    #[error("{0}")]
    Port(String),
    #[error("{0}")]
    Image(String),
    #[error("{0}")]
    Write(String),
    #[error("the reply was not a repeater status: {0}")]
    NotStatus(String),
    #[error("{0}")]
    Wire(String),
    #[error("cancelled")]
    Cancelled,
}

/// Everything a pass records about the unit in front of it.
///
/// One shape for both routes. The console spells its keys out (`version`, `index`,
/// `routing.mode`) and the RS485 control plane abbreviates them (`ver`, `idx`, `mode`); if the
/// two produced differently-shaped evidence, the report would say which cable was plugged in
/// rather than what the repeater is.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RepeaterEvidence {
    pub version: String,
    pub build: String,
    /// `-1` when the repeater did not say. `0` is a real answer: unprovisioned.
    pub index: i8,
    pub mac: String,
    pub routing_mode: String,
    pub range: Option<(u8, u8)>,
    pub tx_errors: u64,
    pub boots: u32,
    pub unhealthy_boots: u32,
    pub reset_reason: String,
    pub uptime_s: u64,
    /// The control-plane version, over RS485. `None` over USB, which has no such gate.
    pub proto: Option<u16>,
}

impl RepeaterEvidence {
    pub fn is_provisioned(&self) -> bool {
        (1..=6).contains(&self.index)
    }
}

/// Reads either vocabulary into one `RepeaterEvidence`.
pub fn parse_status(value: &Value) -> Result<RepeaterEvidence, RepeaterError> {
    let object = value
        .as_object()
        .ok_or_else(|| RepeaterError::NotStatus(value.to_string()))?;

    // A reply has to carry a version to be a status at all: without one, the only thing this
    // could produce is a record full of defaults that reads exactly like a healthy board.
    let version = first_str(object, &["version", "ver"])
        .ok_or_else(|| RepeaterError::NotStatus(value.to_string()))?;

    let routing = object.get("routing").or_else(|| object.get("rt"));
    let health = object.get("health").or_else(|| object.get("h"));

    let range_start = nested_u64(routing, &["range_start", "range", "rs"]).map(|v| v as u8);
    let range_end = nested_u64(routing, &["range_end", "re"]).map(|v| v as u8);

    Ok(RepeaterEvidence {
        version,
        build: first_str(object, &["build", "bld"]).unwrap_or_default(),
        index: first_i64(object, &["index", "idx"]).unwrap_or(-1) as i8,
        mac: first_str(object, &["mac"])
            .unwrap_or_default()
            .to_lowercase(),
        routing_mode: nested_str(routing, &["mode", "m"]).unwrap_or_default(),
        range: match (range_start, range_end) {
            (Some(start), Some(end)) => Some((start, end)),
            _ => None,
        },
        tx_errors: first_u64(object, &["tx_errors", "txe"]).unwrap_or(0),
        boots: nested_u64(health, &["boots", "b"]).unwrap_or(0) as u32,
        unhealthy_boots: nested_u64(health, &["unhealthy_boots", "ub"]).unwrap_or(0) as u32,
        reset_reason: nested_str(health, &["reset_reason", "rst"]).unwrap_or_default(),
        uptime_s: nested_u64(health, &["uptime_ms", "up"])
            .map(|ms| ms / 1000)
            .unwrap_or(0),
        proto: first_u64(object, &["proto"]).map(|v| v as u16),
    })
}

/// The reset reasons that mean the last run ended badly rather than being ended.
///
/// A panic or a watchdog after a write is the difference between "it booted" and "it booted,
/// crashed, and booted again" -- which a version string alone cannot tell you.
pub const UNHEALTHY_RESETS: &[&str] = &["panic", "int_wdt", "task_wdt", "wdt", "brownout"];

/// What a pass was asking for, held against what the repeater says it is.
///
/// A struct rather than four positional arguments because one of them needs explaining at
/// every call site, and a `bool` in fourth place would not do it.
#[derive(Debug, Clone, Default)]
pub struct Expectation<'a> {
    pub index: i8,
    /// The MAC read over the ROM loader, before the write.
    pub mac: Option<&'a str>,
    /// The version the image being installed reports.
    pub version: Option<&'a str>,
    /// The boot counter from before the write -- **only when it could survive the write**.
    ///
    /// `None` after a merged factory write. That image is contiguous from offset zero, so it
    /// covers NVS at `0x9000` with `0xFF` and the boot counter goes with it; a rule that
    /// demanded the counter increase across one would fail every clean provisioning pass. It
    /// is a real check on the OTA route, where NVS is untouched, and it is not available on
    /// the USB one. Saying so beats a check that is quietly wrong half the time.
    pub boots_before: Option<u32>,
}

/// Did we provision the board we think we did, to the value we asked for?
///
/// Every clause here is something that has a way of being wrong on a bench with two repeaters
/// and three serial ports attached. **This is the function the mutation check breaks**: make
/// it return `Ok(())` unconditionally and the suite must go red, or a pass is not evidence.
pub fn evidence_verdict(
    expected: &Expectation<'_>,
    after: &RepeaterEvidence,
) -> Result<(), String> {
    if after.index != expected.index {
        return Err(format!(
            "asked for index {}, and the repeater reports {}",
            expected.index, after.index
        ));
    }
    if let Some(mac) = expected.mac {
        let mac = mac.to_lowercase();
        if !after.mac.is_empty() && after.mac != mac {
            return Err(format!(
                "the console answered from {} but {mac} was the board that was flashed -- this \
                 is not the repeater it was written to",
                after.mac
            ));
        }
    }
    if let Some(version) = expected.version.filter(|v| !v.is_empty())
        && after.version != version
    {
        return Err(format!(
            "the image is {version} and the repeater is running {} -- the write did not take, \
             or it booted the other slot",
            after.version
        ));
    }
    let reason = after.reset_reason.to_ascii_lowercase();
    if UNHEALTHY_RESETS.iter().any(|bad| reason.contains(bad)) {
        return Err(format!(
            "it came up after a {} reset: the new image started and did not stay up",
            after.reset_reason
        ));
    }
    if let Some(boots) = expected.boots_before
        && after.boots <= boots
    {
        return Err(format!(
            "the boot counter did not move ({boots} before, {} after) -- this reading is from \
             before the write",
            after.boots
        ));
    }
    Ok(())
}

fn first_str(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn first_i64(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_i64)
}

fn first_u64(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_u64)
}

fn nested_str(parent: Option<&Value>, keys: &[&str]) -> Option<String> {
    let object = parent?.as_object()?;
    first_str(object, keys)
}

fn nested_u64(parent: Option<&Value>, keys: &[&str]) -> Option<u64> {
    let object = parent?.as_object()?;
    first_u64(object, keys)
}

/// One USB provisioning pass, from a cold board to evidence.
///
/// The order is the whole design, and two steps in it exist for reasons that are not obvious:
///
/// - **The identity is read before the write, not after.** `espflash` reads the MAC over the
///   ROM loader; the running firmware reports the same six bytes in `status`. Comparing them
///   is the only thing that proves the console that was read is the board that was written,
///   on a bench where two repeaters and three serial ports are attached at once.
/// - **The index is read before the write too.** The merged image is contiguous from offset
///   zero, so it covers NVS with `0xFF` and takes the repeater's index, learned range and
///   boot counters with it. Reading it first means a refresh can put back what the unit
///   already had instead of silently unprovisioning it.
pub struct UsbPass<'a> {
    pub image: &'a [u8],
    pub image_version: Option<&'a str>,
    /// `None` keeps whatever index the board already had; `Some(0)` unprovisions deliberately.
    pub index: Option<i8>,
    pub port_settle: std::time::Duration,
    pub console_timeout: std::time::Duration,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsbOutcome {
    pub identity: super::programmer::UsbIdentity,
    pub write: super::programmer::WriteReport,
    pub before: Option<RepeaterEvidence>,
    pub after: RepeaterEvidence,
    pub index_written: i8,
    /// `None` when the board was already running when the port was opened, which is ordinary.
    pub boot_record: Option<Value>,
}

/// Run one pass. `note` receives operator-readable progress; `cancelled` is polled between
/// phases, never mid-write (see `RepeaterProgrammer::write`).
pub fn provision_over_usb(
    programmer: &mut dyn super::programmer::RepeaterProgrammer,
    port: &super::identity::RepeaterPort,
    pass: &UsbPass<'_>,
    progress: &mut dyn FnMut(&str, f64),
    note: &mut dyn FnMut(&str),
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<UsbOutcome, RepeaterError> {
    if cancelled() {
        return Err(RepeaterError::Cancelled);
    }

    // Refused before the probe is opened, not by whatever is holding the cable. An image that
    // is not repeater firmware should cost nothing but a sentence.
    let kind = super::artefacts::classify(pass.image).map_err(RepeaterError::Image)?;

    progress("identifying", 0.0);
    let identity = programmer.identify(port)?;
    note(&format!(
        "repeater {} is a {} {} with {} MB of flash",
        identity.mac,
        identity.chip,
        identity.revision,
        identity.flash_bytes / (1024 * 1024)
    ));

    // What the board already is. Best-effort: a virgin part has no firmware to answer with,
    // and that is the ordinary case for the route this exists to serve.
    let before = match programmer.open_console(port) {
        Ok(mut console) => {
            match console.ask("status", &["status", "version"], pass.console_timeout) {
                Ok(value) => parse_status(&value).ok(),
                Err(_) => None,
            }
        }
        Err(_) => None,
    };
    match &before {
        Some(evidence) => note(&format!(
            "it is running {} (build {}) as index {}",
            evidence.version, evidence.build, evidence.index
        )),
        None => note("it did not answer the console -- a virgin part, or firmware without one"),
    }

    let index = pass
        .index
        .or_else(|| before.as_ref().map(|e| e.index).filter(|i| *i > 0))
        .unwrap_or(0);

    if cancelled() {
        return Err(RepeaterError::Cancelled);
    }
    note(&format!(
        "writing {} bytes at offset 0. This blanks NVS, so index {index} is written back after.",
        pass.image.len()
    ));
    let write = programmer.write(port, pass.image, progress)?;
    note(&format!(
        "wrote {} bytes in {}s, verified by the chip's own MD5 {}",
        write.bytes, write.seconds, write.md5
    ));

    progress("resetting", 0.0);
    let port = programmer.wait_for_port(port.mac.as_deref(), pass.port_settle)?;

    if cancelled() {
        return Err(RepeaterError::Cancelled);
    }
    progress("console", 0.0);
    let mut console = programmer.open_console(&port)?;
    let boot_record = console.boot_record(pass.console_timeout);
    if boot_record.is_none() {
        note("no boot record -- the board was already running when the port opened");
    }

    progress("set-index", 0.0);
    let reply = console.ask(
        &format!("set-index {index}"),
        &["index"],
        pass.console_timeout,
    )?;
    // `set-index` answers with the whole status, so the reply *is* the read-back and there is
    // nothing to ask twice.
    let after = parse_status(&reply)?;

    progress("status", 0.0);
    let after = match console.ask("status", &["status"], pass.console_timeout) {
        Ok(value) => parse_status(&value)?,
        // The set-index reply already carried a full status, so a second read failing is not
        // worth losing the pass over.
        Err(_) => after,
    };

    evidence_verdict(
        &Expectation {
            index,
            mac: Some(&identity.mac),
            version: pass.image_version,
            // A merged write blanks NVS, so the counter it would be compared against is gone.
            boots_before: match kind {
                super::artefacts::RepeaterImageKind::Factory => None,
                super::artefacts::RepeaterImageKind::Application => {
                    before.as_ref().map(|e| e.boots)
                }
            },
        },
        &after,
    )
    .map_err(RepeaterError::Write)?;

    progress("done", 1.0);
    Ok(UsbOutcome {
        identity,
        write,
        before,
        after,
        index_written: index,
        boot_record,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repeater::sim::{SIM_MAC, SimProgrammer};
    use serde_json::json;
    use std::time::Duration;

    /// A real `printStatus` line, as `RS485Repeater/src/main.cpp` prints it.
    fn console_status(index: i8, boots: u32, reset: &str) -> Value {
        json!({
            "type": "status",
            "version": "3.0.0",
            "build": "8c6834d8b1c2",
            "baud": 115200,
            "routing": { "mode": "filtering", "range_start": 19, "range_end": 27,
                         "relayed_control": 4, "filtered_host_frames": 12, "parse_errors": 0 },
            "side1": { "inverted": false, "uart_errors": 0 },
            "side2": { "inverted": false, "uart_errors": 0 },
            "tx_errors": 0,
            "index": index,
            "mac": "F8:5B:1B:ED:8D:A4",
            "event_seq": 7,
            "health": { "reset_reason": reset, "boots": boots, "unhealthy_boots": 0,
                        "min_free_heap": 180000, "uptime_ms": 31_500, "core_dump": false }
        })
    }

    /// The same facts in the abbreviated spelling the RS485 control plane uses.
    fn wire_status(index: i8, boots: u32) -> Value {
        json!({
            "proto": 1, "ver": "3.0.0", "bld": "8c6834d8b1c2",
            "mac": "f8:5b:1b:ed:8d:a4", "idx": index,
            "rt": { "m": "filtering", "rs": 19, "re": 27 },
            "txe": 0,
            "h": { "rst": "power-on", "b": boots, "ub": 0, "up": 31_500 }
        })
    }

    #[test]
    fn both_vocabularies_parse_into_one_shape() {
        let console = parse_status(&console_status(3, 4, "power-on")).unwrap();
        let wire = parse_status(&wire_status(3, 4)).unwrap();
        // Identical apart from `proto`, which is an RS485-only fact -- see below.
        assert_eq!(
            RepeaterEvidence {
                proto: None,
                ..wire.clone()
            },
            console,
            "the route must not change the evidence"
        );
        assert_eq!(console.index, 3);
        assert_eq!(console.range, Some((19, 27)));
        assert_eq!(console.uptime_s, 31);
        // The MAC is lower-cased on the way in, because the console shouts it and the USB
        // descriptor does not, and the two have to compare.
        assert_eq!(console.mac, "f8:5b:1b:ed:8d:a4");
        // `proto` is an RS485-only fact and must not be invented for the USB route.
        assert_eq!(console.proto, None);
        assert_eq!(wire.proto, Some(1));
    }

    #[test]
    fn a_refusal_is_not_a_status_full_of_defaults() {
        // The firmware answers a bad `set-index` with this. Parsed leniently it would become a
        // record with version "", index -1 and zero errors -- which reads like a healthy board.
        let error = json!({"type": "error", "message": "index must be 0-6"});
        assert!(matches!(
            parse_status(&error),
            Err(RepeaterError::NotStatus(_))
        ));
    }

    #[test]
    fn an_unprovisioned_repeater_reports_zero_rather_than_nothing() {
        let evidence = parse_status(&console_status(0, 1, "power-on")).unwrap();
        assert_eq!(evidence.index, 0);
        assert!(!evidence.is_provisioned());
    }

    // ---- the mutation target ---------------------------------------------------------

    fn good() -> RepeaterEvidence {
        parse_status(&console_status(3, 5, "power-on")).unwrap()
    }

    fn before() -> RepeaterEvidence {
        parse_status(&console_status(3, 4, "power-on")).unwrap()
    }

    fn expect(index: i8) -> Expectation<'static> {
        Expectation {
            index,
            ..Expectation::default()
        }
    }

    #[test]
    fn a_matching_pass_is_a_pass() {
        evidence_verdict(
            &Expectation {
                index: 3,
                mac: Some("f8:5b:1b:ed:8d:a4"),
                version: Some("3.0.0"),
                boots_before: Some(4),
            },
            &good(),
        )
        .unwrap();
    }

    #[test]
    fn the_wrong_index_fails() {
        let error = evidence_verdict(&expect(4), &good()).unwrap_err();
        assert!(error.contains("asked for index 4"), "{error}");
    }

    #[test]
    fn a_different_board_fails_even_when_everything_else_matches() {
        // Two repeaters on one bench: flash one, read the other's console. Every other field
        // agrees, and the pass is still worthless.
        let error = evidence_verdict(
            &Expectation {
                index: 3,
                mac: Some(SIM_MAC),
                ..Expectation::default()
            },
            &good(),
        )
        .unwrap_err();
        assert!(
            error.contains("not the repeater it was written to"),
            "{error}"
        );
    }

    #[test]
    fn the_wrong_version_fails() {
        let error = evidence_verdict(
            &Expectation {
                index: 3,
                version: Some("3.1.0"),
                ..Expectation::default()
            },
            &good(),
        )
        .unwrap_err();
        assert!(error.contains("did not take"), "{error}");
    }

    #[test]
    fn a_crash_reset_fails_however_good_the_version_looks() {
        for reason in ["panic", "task_wdt", "brownout"] {
            let after = parse_status(&console_status(3, 5, reason)).unwrap();
            let error = evidence_verdict(
                &Expectation {
                    index: 3,
                    version: Some("3.0.0"),
                    ..Expectation::default()
                },
                &after,
            )
            .unwrap_err();
            assert!(error.contains("did not stay up"), "{reason}: {error}");
        }
    }

    #[test]
    fn a_reading_from_before_the_write_fails_where_the_counter_survives() {
        // The boot counter distinguishes "it came up" from "this is the answer to the question
        // I asked a minute ago, still sitting in the buffer". It is only available where NVS
        // survived the write -- which is the OTA route, not the merged one.
        let stale = parse_status(&console_status(3, 4, "power-on")).unwrap();
        let error = evidence_verdict(
            &Expectation {
                index: 3,
                boots_before: Some(before().boots),
                ..Expectation::default()
            },
            &stale,
        )
        .unwrap_err();
        assert!(error.contains("did not move"), "{error}");
    }

    #[test]
    fn the_counter_check_is_not_applied_across_a_merged_write() {
        // A factory image blanks NVS, so the counter restarts. Demanding it increase would
        // fail every clean USB pass -- which is exactly what the simulated pass caught.
        let after = parse_status(&console_status(3, 1, "power-on")).unwrap();
        evidence_verdict(&expect(3), &after).unwrap();
    }

    // ---- the whole pass, with no hardware --------------------------------------------

    fn factory_image() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x10000];
        bytes[0] = 0xE9;
        bytes[12..14].copy_from_slice(&5u16.to_le_bytes());
        bytes[0x8000..0x8002].copy_from_slice(&[0xAA, 0x50]);
        let mut app = vec![0u8; 4096];
        app[0] = 0xE9;
        app[12..14].copy_from_slice(&5u16.to_le_bytes());
        bytes.extend(app);
        bytes
    }

    fn pass(index: Option<i8>) -> UsbPass<'static> {
        UsbPass {
            image: Box::leak(factory_image().into_boxed_slice()),
            image_version: Some("3.0.0"),
            index,
            port_settle: Duration::from_millis(50),
            console_timeout: Duration::from_millis(50),
        }
    }

    fn run(
        programmer: &mut SimProgrammer,
        pass: &UsbPass<'_>,
    ) -> Result<UsbOutcome, RepeaterError> {
        provision_over_usb(
            programmer,
            &SimProgrammer::port(),
            pass,
            &mut |_, _| {},
            &mut |_| {},
            &mut || false,
        )
    }

    #[test]
    fn a_simulated_pass_provisions_and_verifies() {
        let mut programmer = SimProgrammer::new();
        let outcome = run(&mut programmer, &pass(Some(3))).unwrap();
        assert_eq!(outcome.index_written, 3);
        assert_eq!(outcome.after.index, 3);
        assert_eq!(outcome.after.version, "3.0.0");
        assert_eq!(outcome.identity.mac, SIM_MAC);
        assert!(outcome.write.verified);
        // The board was on 2.2.0 before, which is the whole reason the USB route exists.
        assert_eq!(outcome.before.as_ref().unwrap().version, "2.2.0");
    }

    #[test]
    fn a_refresh_puts_back_the_index_the_merged_write_blanked() {
        // The board is index 5. The merged image blanks NVS, so without the read-before-write
        // it would come back unprovisioned and the pass would still look clean.
        let mut programmer = SimProgrammer::new();
        run(&mut programmer, &pass(Some(5))).unwrap();

        let outcome = run(&mut programmer, &pass(None)).unwrap();
        assert_eq!(
            outcome.index_written, 5,
            "an index the operator did not retype must survive a firmware refresh"
        );
        assert_eq!(outcome.after.index, 5);
    }

    #[test]
    fn zero_unprovisions_deliberately_and_is_not_confused_with_no_choice() {
        let mut programmer = SimProgrammer::new();
        run(&mut programmer, &pass(Some(4))).unwrap();
        let outcome = run(&mut programmer, &pass(Some(0))).unwrap();
        assert_eq!(outcome.index_written, 0);
        assert!(!outcome.after.is_provisioned());
    }

    #[test]
    fn an_image_that_is_not_repeater_firmware_never_reaches_the_chip() {
        let mut programmer = SimProgrammer::new();
        let mut portal = vec![0u8; 2048];
        portal[1] = 0x80;
        let pass = UsbPass {
            image: &portal,
            image_version: None,
            index: Some(1),
            port_settle: Duration::from_millis(50),
            console_timeout: Duration::from_millis(50),
        };
        let error = run(&mut programmer, &pass).unwrap_err();
        assert!(matches!(error, RepeaterError::Image(_)), "{error}");
    }

    #[test]
    fn cancelling_stops_before_anything_is_written() {
        let mut programmer = SimProgrammer::new();
        let error = provision_over_usb(
            &mut programmer,
            &SimProgrammer::port(),
            &pass(Some(3)),
            &mut |_, _| {},
            &mut |_| {},
            &mut || true,
        )
        .unwrap_err();
        assert!(matches!(error, RepeaterError::Cancelled), "{error}");
    }
}

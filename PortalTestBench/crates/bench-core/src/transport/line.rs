//! Line-oriented links: the bench firmware's record protocol, and the production firmware's
//! interactive menu.
//!
//! Both run over the same physical port — USART1 on PB6/PB7 at 115200, which on a bench module
//! is whatever USB serial adapter is clipped to it — and both are newline-delimited text. They
//! differ only in what the lines *mean*, so they share a reader and split on dialect.
//!
//! # The two dialects
//!
//! **Bench** (`home_switch_bench`) emits machine records: `S,…` status at ~60 Hz, `L,…` logs,
//! `#` banners, and the routine records the characterisation plans consume. Parsed by
//! [`super::ascii`].
//!
//! **VCP** (production firmware) is a *human* menu — single keystrokes `a c h s u y d r v ?`,
//! ESC to escape a routine, `0`–`9` to jog — and its output is prose. There is nothing to parse
//! into positions, so every line becomes a log line. That is not a limitation to work around:
//! on production firmware the structured data is on the RS485 side, and this link exists to be
//! a serial monitor with a keyboard, which is exactly what it is during bring-up.

use crate::dut::{Axis, FirmwareKind};
use crate::transport::{
    Link, LinkDiagnostics, LinkError, LinkEvent, LinkInfo, LinkKind, Op, RawSignal, ascii,
};
use router_link::rs485::SerialDevice;

/// Cap on the unterminated tail we will hold while waiting for a newline.
///
/// A port that is producing bytes but no newlines is either at the wrong baud rate or is not
/// this firmware. Without a cap that situation grows a `String` until the process dies; with
/// one it produces a fault the operator can act on.
const MAX_PENDING: usize = 64 * 1024;

/// How a [`LineLink`] gets its byte device.
///
/// Injected rather than hard-coded so the link can be driven from a scripted device in tests
/// and from a modelled module in `--simulate`, with no port, no adapter and no board.
pub type DeviceOpener = Box<dyn FnMut(&str) -> Result<Box<dyn SerialDevice>, LinkError> + Send>;

pub struct LineLink {
    kind: LinkKind,
    endpoint: String,
    device: Option<Box<dyn SerialDevice>>,
    /// Bytes received but not yet terminated by a newline.
    pending: String,
    banner: Option<String>,
    tx: u64,
    rx: u64,
    open_device: DeviceOpener,
}

impl LineLink {
    /// A link that opens a real serial port when asked.
    pub fn serial(kind: LinkKind, endpoint: impl Into<String>) -> Self {
        Self::with_opener(kind, endpoint, Box::new(open_serial))
    }

    /// A link over a caller-supplied device. This is how the tests drive it, and how a
    /// simulated module is plugged in, without either needing a port.
    pub fn with_opener(
        kind: LinkKind,
        endpoint: impl Into<String>,
        open_device: DeviceOpener,
    ) -> Self {
        Self {
            kind,
            endpoint: endpoint.into(),
            device: None,
            pending: String::new(),
            banner: None,
            tx: 0,
            rx: 0,
            open_device,
        }
    }

    /// Split whatever has arrived into whole lines, leaving any partial tail pending.
    ///
    /// # Carriage returns are progress, not lines
    ///
    /// During a seek the firmware prints the live position followed by a bare `\r`, hundreds of
    /// times a second, so a terminal redraws one line in place. Splitting only on `\n` makes
    /// every one of those redraws accumulate into a single enormous "line" — observed on
    /// hardware as a 4 KB blob of `-189773\t\r` repeated ~250 times, with the one message that
    /// mattered (`[E …] Switch not seen`) buried at the end of it. A long enough seek would
    /// also blow past `MAX_PENDING` and be reported as a baud-rate fault.
    ///
    /// So `\r` ends a fragment too, and CR-terminated fragments are **discarded**: each one was
    /// overwritten by the next, and only the `\n`-terminated text was ever meant to persist.
    /// That is exactly what the terminal shows once the routine is over.
    fn take_lines(&mut self, bytes: &[u8]) -> Vec<String> {
        // The firmware is ASCII, but a wrong baud rate produces bytes that are not, and losing
        // the whole buffer to a UTF-8 error would hide the very symptom worth seeing.
        self.pending.push_str(&String::from_utf8_lossy(bytes));

        let mut lines = Vec::new();
        while let Some(index) = self.pending.find(['\n', '\r']) {
            let terminator = self.pending.as_bytes()[index];
            let fragment: String = self.pending.drain(..=index).collect();

            // `\r\n` is one break, not two: drop the newline that follows a carriage return so
            // it does not also emit an empty line.
            if terminator == b'\r' && self.pending.starts_with('\n') {
                self.pending.drain(..1);
                lines.push(fragment);
                continue;
            }
            if terminator == b'\n' {
                lines.push(fragment);
            }
            // A bare `\r`: a redraw that the next fragment replaces. Dropped.
        }
        lines
    }
}

fn open_serial(endpoint: &str) -> Result<Box<dyn SerialDevice>, LinkError> {
    let settings = serde_json::json!({ "deviceType": "Serial", "address": endpoint });
    router_link::rs485::create_device(&settings).map_err(|error| {
        // The common failure on this bench is that the GUI already holds the port, and Windows
        // reports that as a bare "Access is denied." Naming the likely cause turns a dead end
        // into an instruction.
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            LinkError::Busy {
                endpoint: endpoint.to_string(),
                detail: Some(
                    " -- close the bench GUI, or point ptb at it instead of --local".into(),
                ),
            }
        } else {
            LinkError::Io(format!("could not open {endpoint}: {error}"))
        }
    })
}

impl Link for LineLink {
    fn info(&self) -> LinkInfo {
        LinkInfo {
            kind: self.kind,
            endpoint: self.endpoint.clone(),
            banner: self.banner.clone(),
        }
    }

    fn open(&mut self) -> Result<LinkInfo, LinkError> {
        let device = (self.open_device)(&self.endpoint)?;
        self.device = Some(device);
        self.pending.clear();
        self.banner = None;
        Ok(self.info())
    }

    fn close(&mut self) {
        if let Some(device) = self.device.as_mut() {
            device.close();
        }
        self.device = None;
        self.pending.clear();
    }

    fn is_open(&self) -> bool {
        self.device.as_ref().is_some_and(|d| d.is_connected())
    }

    fn diagnostics(&self) -> LinkDiagnostics {
        LinkDiagnostics {
            tx: self.tx,
            rx: self.rx,
            ..LinkDiagnostics::default()
        }
    }

    fn send(&mut self, op: &Op) -> Result<(), LinkError> {
        let rendered = match self.kind {
            LinkKind::BenchAscii => ascii::render_op(op),
            LinkKind::Vcp => render_vcp_op(op),
            other => {
                return Err(LinkError::Unsupported {
                    kind: other.name(),
                    op: format!("{op:?}"),
                });
            }
        };

        let Some(rendered) = rendered else {
            return Err(LinkError::Unsupported {
                kind: self.kind.name(),
                op: format!("{op:?}"),
            });
        };

        let device = self.device.as_mut().ok_or(LinkError::NotOpen)?;
        // Whether this rendering is a line or a keystroke, decided by what it IS rather than by
        // which dialect produced it.
        //
        // The bench dialect is line-based throughout. The VCP menu is keystrokes -- which must
        // NOT be terminated, because the firmware reads the newline as a second command
        // (observed on hardware: a trailing newline drew "? unknown command" from the device on
        // the next port along, and on a portal it would dispatch whatever the menu binds to it)
        // -- EXCEPT for `:` lines, which are buffered until CR/LF and are the only VCP form that
        // can carry an argument. Terminating on `kind` alone would leave those hanging in the
        // firmware's line buffer forever.
        let terminate = self.kind == LinkKind::BenchAscii || rendered.starts_with(LINE_PREFIX);
        let mut bytes = rendered.into_bytes();
        if terminate {
            bytes.push(b'\n');
        }
        device
            .transmit(&bytes)
            .map_err(|e| LinkError::Io(e.to_string()))?;
        self.tx += 1;
        Ok(())
    }

    fn send_raw(&mut self, signal: &RawSignal) -> Result<(), LinkError> {
        let RawSignal::VcomText { text, ending } = signal else {
            return Err(LinkError::Unsupported {
                kind: self.kind.name(),
                op: "RS485 JSON payload".into(),
            });
        };
        if self.kind != LinkKind::Vcp {
            return Err(LinkError::Unsupported {
                kind: self.kind.name(),
                op: "raw VCOM text".into(),
            });
        }
        if text.len() > 4096 {
            return Err(LinkError::Io("raw VCOM payload exceeds 4096 bytes".into()));
        }
        let device = self.device.as_mut().ok_or(LinkError::NotOpen)?;
        let mut bytes = text.as_bytes().to_vec();
        bytes.extend_from_slice(ending.bytes());
        device
            .transmit(&bytes)
            .map_err(|e| LinkError::Io(e.to_string()))?;
        self.tx += 1;
        Ok(())
    }

    fn poll(&mut self, _now_ms: u64) -> Vec<LinkEvent> {
        let Some(device) = self.device.as_mut() else {
            return Vec::new();
        };

        let bytes = match device.receive_available() {
            Ok(bytes) => bytes,
            Err(error) => return vec![LinkEvent::Fault(format!("read failed: {error}"))],
        };
        if bytes.is_empty() && self.pending.len() <= MAX_PENDING {
            return Vec::new();
        }
        if !bytes.is_empty() {
            self.rx += 1;
        }

        let lines = self.take_lines(&bytes);

        if self.pending.len() > MAX_PENDING {
            let held = self.pending.len();
            self.pending.clear();
            return vec![LinkEvent::Fault(format!(
                "{held} bytes arrived with no line ending -- wrong baud rate, or this is not \
                 firmware that speaks a line protocol"
            ))];
        }

        let mut events = Vec::new();
        for line in lines {
            match self.kind {
                LinkKind::BenchAscii => {
                    if let Some(mut parsed) = ascii::parse_line(&line) {
                        for event in &parsed {
                            if let LinkEvent::Identified { banner, .. } = event {
                                self.banner = Some(banner.clone());
                            }
                        }
                        events.append(&mut parsed);
                    }
                }
                LinkKind::Vcp => {
                    if let Some(event) = parse_vcp_line(&line, &mut self.banner) {
                        events.push(event);
                    }
                }
                _ => {}
            }
        }
        events
    }
}

/// Prefix that opens the firmware's buffered line mode (`LOGGER_LINE_PREFIX` in `Logger.h`).
///
/// A rendering that starts with this is a *line* and needs a terminator; anything else is a bare
/// keystroke and must not get one.
const LINE_PREFIX: char = ':';

/// The production firmware's interactive menu, from `PortalFW/src/Logger.cpp`.
///
/// Two shapes, and the difference matters at the byte level:
///
/// * **Bare keystrokes** — one byte, no terminator. The firmware reads a trailing newline as a
///   *second* command and dispatches whatever the menu binds to it.
/// * **`:` lines** — buffered until CR/LF, so they can carry arguments. This is the only way to
///   give the console a number; everything below that names a threshold or a speed uses it.
///
/// There is still no per-axis form of `h`/`u`: `c` calibrates both axes. An op that names a
/// single axis is therefore **refused** rather than quietly doing both — a bench that moves an
/// axis the operator did not ask about is worse than one that says it cannot. `Census` is the
/// exception, because the firmware's `:n` does take an axis argument.
fn render_vcp_op(op: &Op) -> Option<String> {
    Some(match op {
        Op::Identify => "v".to_string(),
        Op::Startup => "s".to_string(),
        Op::Poll | Op::PollPosition => "a".to_string(),
        Op::Calibrate { .. } => "c".to_string(),
        // ESC. The firmware polls for this inside every routine loop.
        Op::Escape => "\x1b".to_string(),
        Op::Reboot => "r".to_string(),
        Op::SetHomeThreshold { value } => format!(":t {}", (*value).clamp(0, 255)),
        Op::Census {
            axis,
            threshold,
            speed,
        } => {
            // `:n <duty> [speed] [axis]` -- positional, so a named axis with a default speed
            // still has to state the speed. 0 means "the firmware's own seek speed".
            format!(
                ":n {} {} {}",
                threshold,
                speed.unwrap_or(0),
                axis_argument(*axis)
            )
        }
        // Both-axis only on this menu; see above.
        Op::Home { .. }
        | Op::Unjam { .. }
        | Op::MeasureBacklash { .. }
        | Op::MoveTo { .. }
        | Op::MoveAxes { .. }
        | Op::SetMotionProfile { .. }
        | Op::SetCurrent { .. }
        | Op::SetMicrostep { .. } => return None,
    })
}

/// The firmware's `:n` axis argument: 0 = A, 1 = B.
fn axis_argument(axis: Axis) -> u8 {
    match axis {
        Axis::A => 0,
        Axis::B => 1,
    }
}

/// The menu's output is prose, so each line is a log line.
///
/// The one structured thing it prints is the version banner, which is worth catching because it
/// identifies the image on the module.
fn parse_vcp_line(line: &str, banner: &mut Option<String>) -> Option<LinkEvent> {
    let text = line.trim_end_matches(['\r', '\n']);
    if text.trim().is_empty() {
        return None;
    }

    if let Some(index) = text.find("Portal v") {
        let version = text[index + "Portal v".len()..].trim().to_string();
        *banner = Some(text.to_string());
        return Some(LinkEvent::Identified {
            firmware: FirmwareKind::Production,
            version: Some(version),
            // The menu does not report the gearing; the RS485 report does.
            ratio: crate::dut::GearRatio::Unknown,
            usteps_per_rev: None,
            banner: text.to_string(),
        });
    }

    // The firmware writes `[` then `W ` or `E ` then the module name then `] `, so the level
    // marker is a PREFIX of the bracket, not a standalone `[E ]` token -- a real line reads
    // `[E Routines.init] Fail`. Matching the wrong shape silently classifies every error as
    // routine status, which is exactly the line a bench exists to notice.
    let level = if text.starts_with("[E ") {
        crate::LOG_LEVEL_ERROR
    } else if text.starts_with("[W ") {
        crate::LOG_LEVEL_WARNING
    } else {
        crate::LOG_LEVEL_STATUS
    };
    Some(LinkEvent::Log {
        level,
        message: text.to_string(),
        firmware_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A byte device backed by a script of chunks, so a whole conversation can be replayed
    /// without a port. Chunks are deliberately not line-aligned: that is the case that breaks
    /// naive readers.
    #[derive(Default)]
    struct FakeDevice {
        to_host: Vec<Vec<u8>>,
        written: Arc<Mutex<Vec<u8>>>,
        connected: bool,
    }

    impl SerialDevice for FakeDevice {
        fn type_name(&self) -> &'static str {
            "Fake"
        }
        fn address_string(&self) -> String {
            "fake".into()
        }
        fn is_connected(&self) -> bool {
            self.connected
        }
        fn transmit(&mut self, data: &[u8]) -> std::io::Result<()> {
            self.written.lock().unwrap().extend_from_slice(data);
            Ok(())
        }
        fn receive_available(&mut self) -> std::io::Result<Vec<u8>> {
            if self.to_host.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(self.to_host.remove(0))
            }
        }
        fn close(&mut self) {
            self.connected = false;
        }
    }

    pub(super) fn link_over(kind: LinkKind, chunks: Vec<&str>) -> (LineLink, Arc<Mutex<Vec<u8>>>) {
        let written = Arc::new(Mutex::new(Vec::new()));
        let to_host: Vec<Vec<u8>> = chunks.into_iter().map(|c| c.as_bytes().to_vec()).collect();
        let captured = Arc::clone(&written);
        let mut device = Some(FakeDevice {
            to_host,
            written: Arc::clone(&written),
            connected: true,
        });
        let mut link = LineLink::with_opener(
            kind,
            "fake",
            Box::new(move |_| {
                device
                    .take()
                    .map(|d| Box::new(d) as Box<dyn SerialDevice>)
                    .ok_or_else(|| LinkError::Io("already opened".into()))
            }),
        );
        link.open().unwrap();
        (link, captured)
    }

    /// The reader must not assume a read lands on a line boundary. A serial port hands over
    /// whatever happens to be in the buffer, and a record split across two reads is the norm
    /// at 60 Hz, not an edge case.
    #[test]
    fn records_split_across_reads_are_reassembled() {
        let (mut link, _) = link_over(
            LinkKind::BenchAscii,
            vec![
                "S,125000,1,246,47",
                "426,900,0,1,0,1\nS,125",
                "016,0,246,47430,900,0,1,0,1\n",
            ],
        );

        let first = link.poll(0);
        assert!(
            first.is_empty(),
            "half a record must produce nothing yet, got {first:?}"
        );

        let second = link.poll(1);
        assert!(second.contains(&LinkEvent::Position {
            axis: ascii::BENCH_AXIS,
            position: 47_426,
            target: None,
        }));

        let third = link.poll(2);
        assert!(third.contains(&LinkEvent::Position {
            axis: ascii::BENCH_AXIS,
            position: 47_430,
            target: None,
        }));
    }

    #[test]
    fn a_banner_is_remembered_on_the_link() {
        let (mut link, _) = link_over(
            LinkKind::BenchAscii,
            vec!["# usteps_per_rev=189704 default_threshold=220 gear_ratio=32\n"],
        );
        link.poll(0);
        assert_eq!(
            link.info().banner.as_deref(),
            Some("# usteps_per_rev=189704 default_threshold=220 gear_ratio=32")
        );
    }

    #[test]
    fn bench_ops_go_out_as_single_letter_commands() {
        let (mut link, written) = link_over(LinkKind::BenchAscii, vec![]);
        link.send(&Op::SetHomeThreshold { value: 246 }).unwrap();
        link.send(&Op::Home {
            axis: crate::dut::Axis::A,
        })
        .unwrap();
        assert_eq!(
            String::from_utf8(written.lock().unwrap().clone()).unwrap(),
            "T,246\nO\n"
        );
    }

    /// The production menu dispatches on a bare keystroke, so a trailing newline is a **second
    /// command**, not padding. Observed on hardware: the newline drew "? unknown command" from
    /// a device on the bench, and on a portal it would run whatever the menu binds to it.
    #[test]
    fn the_vcp_menu_gets_no_trailing_newline() {
        let (mut link, written) = link_over(LinkKind::Vcp, vec![]);
        link.send(&Op::Identify).unwrap();
        link.send(&Op::Escape).unwrap();
        // `v` for version, then a bare ESC. Nothing else.
        assert_eq!(
            String::from_utf8(written.lock().unwrap().clone()).unwrap(),
            "v\x1b"
        );
    }

    #[test]
    fn raw_vcom_text_uses_the_explicit_line_ending_and_counts_tx() {
        let (mut link, written) = link_over(LinkKind::Vcp, vec![]);
        link.send_raw(&RawSignal::VcomText {
            text: ":t 246".into(),
            ending: crate::transport::LineEnding::Crlf,
        })
        .unwrap();
        assert_eq!(written.lock().unwrap().as_slice(), b":t 246\r\n");
        assert_eq!(link.diagnostics().tx, 1);
    }

    /// The production menu has no single-axis home. Doing both axes because the operator asked
    /// for one would move a prism nobody asked to move.
    #[test]
    fn the_vcp_menu_refuses_a_single_axis_op_rather_than_doing_both() {
        let (mut link, _) = link_over(LinkKind::Vcp, vec![]);
        let error = link
            .send(&Op::Home {
                axis: crate::dut::Axis::A,
            })
            .unwrap_err();
        assert!(matches!(error, LinkError::Unsupported { kind: "vcp", .. }));
    }

    #[test]
    fn the_vcp_version_banner_identifies_production_firmware() {
        let (mut link, _) = link_over(LinkKind::Vcp, vec!["Portal v2026-08-18_17.21 a94ae48+\n"]);
        let events = link.poll(0);
        match &events[0] {
            LinkEvent::Identified {
                firmware, version, ..
            } => {
                assert_eq!(*firmware, FirmwareKind::Production);
                assert_eq!(version.as_deref(), Some("2026-08-18_17.21 a94ae48+"));
            }
            other => panic!("expected Identified, got {other:?}"),
        }
    }

    /// The real shape is `[E Routines.init] Fail` -- the level marker sits inside the bracket
    /// ahead of the module name.
    #[test]
    fn vcp_prose_becomes_log_lines_at_the_firmwares_own_levels() {
        let (mut link, _) = link_over(
            LinkKind::Vcp,
            vec!["[W MotionControlA] backlash high\n[E Routines.init] Fail\n| END Routines.init\n"],
        );
        let events = link.poll(0);
        let levels: Vec<u8> = events
            .iter()
            .filter_map(|e| match e {
                LinkEvent::Log { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![10, 20, 0]);
    }

    /// Bytes with no newline mean the wrong baud rate far more often than they mean a very long
    /// line. Say so, instead of growing a buffer until the process dies.
    #[test]
    fn a_stream_with_no_line_endings_is_reported_rather_than_buffered_forever() {
        let junk = "x".repeat(MAX_PENDING + 1);
        let (mut link, _) = link_over(LinkKind::BenchAscii, vec![&junk]);
        let events = link.poll(0);
        assert!(
            matches!(&events[0], LinkEvent::Fault(m) if m.contains("no line ending")),
            "got {events:?}"
        );
    }
}

/// Whether the production menu can express an op, without needing an open link.
///
/// Used by `Plan::validate` so a plan that asks the menu for something it does not have — a
/// per-axis home, a current change — is refused before the motor turns rather than discovered
/// mid-sequence.
pub fn vcp_can(op: &Op) -> bool {
    render_vcp_op(op).is_some()
}

#[cfg(test)]
mod cr_tests {
    use super::tests::link_over;
    use super::*;

    /// Observed on hardware: during a seek the firmware redraws the live position with a bare
    /// `\r` hundreds of times a second. Those redraws must not accumulate into one line, or the
    /// message that matters ends up buried at the end of a 4 KB blob.
    #[test]
    fn carriage_return_redraws_are_dropped_and_the_real_line_survives() {
        let mut stream = String::new();
        for position in 0..250 {
            stream.push_str(&format!("-{}\t\r", 189_773 - position));
        }
        stream.push_str("[E MotionControl_A.routineFindSwitchAccurate] Switch not seen\n");

        let (mut link, _) = link_over(LinkKind::Vcp, vec![&stream]);
        let events = link.poll(0);

        assert_eq!(
            events.len(),
            1,
            "the redraws should not have produced events: {events:?}"
        );
        match &events[0] {
            LinkEvent::Log { level, message, .. } => {
                assert_eq!(*level, crate::LOG_LEVEL_ERROR);
                assert_eq!(
                    message,
                    "[E MotionControl_A.routineFindSwitchAccurate] Switch not seen"
                );
                assert!(
                    !message.contains("189773"),
                    "the redraws leaked into the message"
                );
            }
            other => panic!("expected a log line, got {other:?}"),
        }
    }

    /// `\r\n` is one line break. Treating it as two emits a phantom empty line between every
    /// real one, which on a firmware that uses CRLF would double the log.
    #[test]
    fn crlf_is_one_break_not_two() {
        let (mut link, _) = link_over(LinkKind::Vcp, vec!["first\r\nsecond\r\n"]);
        let events = link.poll(0);
        let messages: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                LinkEvent::Log { message, .. } => Some(message.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(messages, vec!["first", "second"]);
    }

    /// A seek that runs for minutes emits only carriage returns. That must not be mistaken for
    /// a wrong baud rate, which is what the unterminated-tail guard reports.
    #[test]
    fn a_long_redraw_stream_never_trips_the_no_line_ending_guard() {
        let mut stream = String::new();
        while stream.len() < MAX_PENDING * 2 {
            stream.push_str("-189773\t\r");
        }
        let (mut link, _) = link_over(LinkKind::Vcp, vec![&stream]);
        let events = link.poll(0);
        assert!(events.is_empty(), "expected silence, got {events:?}");
    }
}

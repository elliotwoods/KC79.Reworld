//! The repeater's USB console: single-line JSON at 115200, over the chip's own USB CDC.
//!
//! Independent of both RS485 UARTs, which is what makes it useful -- it answers when the wire
//! does not. The vocabulary is in `RS485Repeater/README.md`; this needs four verbs of it:
//! the unsolicited `boot` record, `version`/`status`, `index`, and `set-index N`.
//!
//! # Why this does not reuse `transport::line`
//!
//! `LineLink::take_lines` ends a fragment on `\r` as well as `\n` and **discards** the
//! CR-terminated ones, because a Portal redraws its OLED status line with carriage returns and
//! a naive splitter accumulates 4 kB blobs out of them. The repeater console does nothing of
//! the sort: it is one `printf` per reply, `\n`-terminated, and importing a rule that drops
//! CR-terminated text could only ever lose a line here. Two splitters, because they are two
//! different jobs that happen to look alike.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use router_link::rs485::device::{create_device, SerialDevice};
use serde_json::Value;

use super::programmer::ConsoleSession;
use super::provision::RepeaterError;

/// Enough for the longest `printStatus` line with room to spare; past this, whatever is
/// arriving is not this console.
const MAX_PENDING: usize = 8192;

pub struct RepeaterConsole {
    device: Box<dyn SerialDevice>,
    pending: String,
    lines: VecDeque<Value>,
    port: String,
}

impl RepeaterConsole {
    pub fn open(port: &str) -> Result<Self, RepeaterError> {
        let settings = serde_json::json!({ "deviceType": "Serial", "address": port });
        let device = create_device(&settings).map_err(|error| RepeaterError::Console {
            port: port.to_string(),
            detail: describe_open(&error),
        })?;
        Ok(Self {
            device,
            pending: String::new(),
            lines: VecDeque::new(),
            port: port.to_string(),
        })
    }

    pub fn port(&self) -> &str {
        &self.port
    }

    pub fn send(&mut self, line: &str) -> Result<(), RepeaterError> {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        self.device
            .transmit(&bytes)
            .map_err(|error| RepeaterError::Console {
                port: self.port.clone(),
                detail: format!("could not write `{line}`: {error}"),
            })
    }

    /// Read whatever has arrived and split it into whole JSON objects.
    ///
    /// Non-JSON lines are dropped rather than reported: the ESP-IDF bootloader prints a few
    /// lines of its own at every reset, and they are not a fault.
    fn pump(&mut self) -> Result<(), RepeaterError> {
        let bytes = self
            .device
            .receive_available()
            .map_err(|error| RepeaterError::Console {
                port: self.port.clone(),
                detail: format!("could not read: {error}"),
            })?;
        if bytes.is_empty() {
            return Ok(());
        }
        self.pending.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(index) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=index).collect();
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(line.trim()) {
                self.lines.push_back(Value::Object(map));
            }
        }
        if self.pending.len() > MAX_PENDING {
            let held = self.pending.len();
            self.pending.clear();
            return Err(RepeaterError::Console {
                port: self.port.clone(),
                detail: format!(
                    "{held} bytes arrived with no line ending -- this is not the repeater \
                     console, or it is not at 115200"
                ),
            });
        }
        Ok(())
    }

    /// The next object whose `"type"` is one of `kinds`.
    ///
    /// Objects of other kinds are discarded as they go past: a `boot` record can arrive in the
    /// middle of waiting for a `status`, and holding it would only make the next wait wrong.
    pub fn expect(&mut self, kinds: &[&str], timeout: Duration) -> Result<Value, RepeaterError> {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump()?;
            while let Some(value) = self.lines.pop_front() {
                let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
                if kinds.contains(&kind) {
                    return Ok(value);
                }
            }
            if Instant::now() >= deadline {
                return Err(RepeaterError::ConsoleSilent {
                    port: self.port.clone(),
                    wanted: kinds.join(" or "),
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The same, but a miss is not a failure. Used for the `boot` record, which is emitted
    /// exactly once and is gone if the board was already running when the port was opened.
    pub fn take(&mut self, kinds: &[&str], timeout: Duration) -> Option<Value> {
        self.expect(kinds, timeout).ok()
    }

    /// Ask, and read the answer. `status`, `version` and `index` all reply with the same
    /// object shape; `set-index N` replies with it too, so the reply *is* the read-back.
    pub fn ask(
        &mut self,
        command: &str,
        kinds: &[&str],
        timeout: Duration,
    ) -> Result<Value, RepeaterError> {
        self.send(command)?;
        let value = self.expect_including_errors(kinds, timeout)?;
        if value.get("type").and_then(Value::as_str) == Some("error") {
            return Err(RepeaterError::ConsoleRefused {
                command: command.to_string(),
                detail: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("refused")
                    .to_string(),
            });
        }
        Ok(value)
    }

    /// `error` is always an acceptable answer: the firmware replies with one to a bad
    /// `set-index`, and waiting the whole timeout for a `status` that will never come would
    /// turn a clear refusal into a vague silence.
    fn expect_including_errors(
        &mut self,
        kinds: &[&str],
        timeout: Duration,
    ) -> Result<Value, RepeaterError> {
        let mut wanted: Vec<&str> = kinds.to_vec();
        wanted.push("error");
        self.expect(&wanted, timeout)
    }
}

/// The three ways opening this port fails, in the operator's terms.
fn describe_open(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => "already open -- close the serial monitor, or \
             disconnect the Test tab's link, and try again"
            .to_string(),
        std::io::ErrorKind::NotFound => {
            "is not there any more -- the repeater may have re-enumerated".to_string()
        }
        _ => error.to_string(),
    }
}

impl ConsoleSession for RepeaterConsole {
    fn boot_record(&mut self, timeout: Duration) -> Option<Value> {
        self.take(&["boot"], timeout)
    }

    fn ask(
        &mut self,
        command: &str,
        kinds: &[&str],
        timeout: Duration,
    ) -> Result<Value, RepeaterError> {
        RepeaterConsole::ask(self, command, kinds, timeout)
    }
}

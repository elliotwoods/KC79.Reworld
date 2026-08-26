//! A modelled repeater, for a bench with no ESP32 attached.
//!
//! It models the things that decide a verdict, and in particular the two that are easy to
//! assume away: **a merged write blanks NVS**, so the simulated index goes to 0 exactly as a
//! real one does and the pass has to put it back; and the boot counter increments, so the
//! "this reading is from before the write" check has something real to check.
//!
//! It does not model the ESP loader protocol. Simulating a decoder proves nothing about the
//! decoder -- the same reason `survey.rs` refuses to label a port by heuristics.

use std::time::Duration;

use serde_json::{json, Value};

use super::artefacts::{classify, RepeaterImageKind, APP_OFFSET};
use super::identity::RepeaterPort;
use super::programmer::{ConsoleSession, Progress, RepeaterProgrammer, UsbIdentity, WriteReport};
use super::provision::RepeaterError;

/// A MAC that is obviously not a real one, so a simulated pass in a report cannot be mistaken
/// for hardware evidence.
pub const SIM_MAC: &str = "f8:5b:1b:00:00:01";

const SIM_PORT: &str = "sim";

#[derive(Debug, Clone)]
struct State {
    version: String,
    build: String,
    /// 0 is unprovisioned, which is where a merged write leaves it.
    index: i8,
    boots: u32,
    learned_range: Option<(u8, u8)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: "2.2.0".into(),
            build: "simulated".into(),
            index: 0,
            boots: 1,
            learned_range: None,
        }
    }
}

#[derive(Default)]
pub struct SimProgrammer {
    state: std::sync::Arc<std::sync::Mutex<State>>,
    /// What the next write should be taken to install, so a readback can report it.
    pub pending_version: Option<String>,
}

impl SimProgrammer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The port a simulated bench offers, so the picker is not empty in a demo.
    pub fn port() -> RepeaterPort {
        RepeaterPort {
            name: SIM_PORT.into(),
            mac: Some(SIM_MAC.into()),
            product: Some("Simulated repeater".into()),
        }
    }
}

impl RepeaterProgrammer for SimProgrammer {
    fn describe(&self) -> String {
        "simulated repeater".into()
    }

    fn identify(&mut self, _port: &RepeaterPort) -> Result<UsbIdentity, RepeaterError> {
        Ok(UsbIdentity {
            chip: "ESP32-C3 (simulated)".into(),
            revision: "v0.4".into(),
            mac: SIM_MAC.into(),
            flash_bytes: 4 * 1024 * 1024,
        })
    }

    fn write(
        &mut self,
        _port: &RepeaterPort,
        image: &[u8],
        progress: &mut Progress<'_>,
    ) -> Result<WriteReport, RepeaterError> {
        let kind = classify(image).map_err(RepeaterError::Image)?;
        for step in 0..=10 {
            progress("writing", step as f64 / 10.0);
        }
        progress("verifying", 1.0);
        let mut state = self.state.lock().unwrap();
        if kind == RepeaterImageKind::Factory {
            // The merged image is one contiguous blob from offset zero, so it covers NVS at
            // 0x9000 with 0xFF. The index, the learned range and the boot counters go with it.
            state.index = 0;
            state.learned_range = None;
            state.boots = 0;
        }
        state.version = self
            .pending_version
            .clone()
            .unwrap_or_else(|| "3.0.0".to_string());
        state.build = "simulated-new".into();
        Ok(WriteReport {
            bytes: image.len(),
            seconds: 1,
            md5: format!("{:032x}", image.len()),
            verified: true,
        })
    }

    fn wait_for_port(
        &mut self,
        _mac: Option<&str>,
        _timeout: Duration,
    ) -> Result<RepeaterPort, RepeaterError> {
        self.state.lock().unwrap().boots += 1;
        Ok(Self::port())
    }

    fn open_console(
        &mut self,
        _port: &RepeaterPort,
    ) -> Result<Box<dyn ConsoleSession>, RepeaterError> {
        Ok(Box::new(SimConsole {
            state: self.state.clone(),
            boot_pending: true,
        }))
    }
}

struct SimConsole {
    state: std::sync::Arc<std::sync::Mutex<State>>,
    boot_pending: bool,
}

impl SimConsole {
    fn status(&self, kind: &str) -> Value {
        let state = self.state.lock().unwrap();
        json!({
            "type": kind,
            "version": state.version,
            "build": state.build,
            "baud": 115200,
            "routing": {
                "mode": if state.learned_range.is_some() { "filtering" } else { "transparent" },
                "range_start": state.learned_range.map(|r| r.0).unwrap_or(0),
                "range_end": state.learned_range.map(|r| r.1).unwrap_or(0),
            },
            "index": state.index,
            "mac": SIM_MAC,
            "tx_errors": 0,
            "health": {
                "reset_reason": "power-on",
                "boots": state.boots,
                "unhealthy_boots": 0,
                "uptime_ms": 1_500u64,
            },
        })
    }
}

impl ConsoleSession for SimConsole {
    fn boot_record(&mut self, _timeout: Duration) -> Option<Value> {
        if !std::mem::take(&mut self.boot_pending) {
            return None;
        }
        let state = self.state.lock().unwrap();
        Some(json!({
            "type": "boot",
            "version": state.version,
            "build": state.build,
            "chip": "ESP32-C3",
            "baud": 115200,
        }))
    }

    fn ask(
        &mut self,
        command: &str,
        _kinds: &[&str],
        _timeout: Duration,
    ) -> Result<Value, RepeaterError> {
        if let Some(rest) = command.strip_prefix("set-index") {
            let value: i64 = rest.trim().parse().unwrap_or(-1);
            if !(0..=6).contains(&value) {
                return Err(RepeaterError::ConsoleRefused {
                    command: command.to_string(),
                    detail: "index must be 0-6".into(),
                });
            }
            self.state.lock().unwrap().index = value as i8;
            return Ok(self.status("index"));
        }
        match command {
            "status" => Ok(self.status("status")),
            "version" => Ok(self.status("version")),
            "index" => Ok(self.status("index")),
            other => Err(RepeaterError::ConsoleRefused {
                command: other.to_string(),
                detail: "the simulated console knows status, version, index and set-index".into(),
            }),
        }
    }
}

/// The application half of a merged image, which is what an RS485 transfer carries.
///
/// Here rather than in `artefacts` because it is a fact about the *file layout* that only a
/// caller slicing one needs, and it is verified against the real pair in the tests there.
pub fn application_of(merged: &[u8]) -> Option<&[u8]> {
    merged.get(APP_OFFSET..)
}

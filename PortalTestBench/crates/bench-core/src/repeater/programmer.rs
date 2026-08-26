//! The seam between "provision a repeater" and "talk to an ESP32".
//!
//! The same shape `portal-swd` uses for the SWD rig: one trait, a real implementation behind
//! a feature, and a modelled one that is always compiled. What it buys is the same thing --
//! the whole provisioning sequence, including the readback that decides the verdict, runs in
//! a unit test with no board, no cable and no `espflash`.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::identity::RepeaterPort;
use super::provision::RepeaterError;

/// `(step, fraction within that step)`, the same closure shape `Worker::run_flash` already
/// passes down to the SWD rig.
pub type Progress<'a> = dyn FnMut(&str, f64) + 'a;

/// What the ROM loader says about the part, read **before** anything is written.
///
/// `mac` is the load-bearing field: it is read over the ROM loader, and the running firmware
/// reports the same six bytes in `status`. Comparing the two is what proves a pass read the
/// board it wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsbIdentity {
    pub chip: String,
    pub revision: String,
    pub mac: String,
    pub flash_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteReport {
    pub bytes: usize,
    pub seconds: u64,
    /// The MD5 the chip computed over what it now holds, and whether it matched the image.
    pub md5: String,
    pub verified: bool,
}

/// One console conversation, for as long as the port stays open.
///
/// A method per exchange rather than "send a string and hope": every one of these has a reply
/// shape the caller has to be able to depend on.
pub trait ConsoleSession: Send {
    /// The unsolicited `{"type":"boot", ...}` record, if it has not already gone past.
    ///
    /// **Never required.** The firmware emits it once, on the first loop after USB is up
    /// (`serviceUsbDiagnostics`), so opening the port a second later means it is gone. A
    /// missing boot record is information -- "the board was already running" -- not a fault.
    fn boot_record(&mut self, timeout: Duration) -> Option<Value>;

    /// Send a command and read the object it answers with, or the `error` it refuses with.
    fn ask(&mut self, command: &str, kinds: &[&str], timeout: Duration)
        -> Result<Value, RepeaterError>;
}

/// Writing firmware to an ESP32-C3 over its own USB.
pub trait RepeaterProgrammer: Send {
    /// A human name for what is doing the writing, for the report.
    fn describe(&self) -> String;

    fn identify(&mut self, port: &RepeaterPort) -> Result<UsbIdentity, RepeaterError>;

    /// Write `image` at offset zero and verify it by the chip's own MD5.
    ///
    /// Not cancellable, and that is honest rather than a gap: the ROM/stub loader has no
    /// safe point to stop at mid-segment, and a 409 kB write with the stub is about ten
    /// seconds. The phases either side of it -- waiting for the port to come back, and the
    /// console session -- are the long ones, and those do stop.
    fn write(
        &mut self,
        port: &RepeaterPort,
        image: &[u8],
        progress: &mut Progress<'_>,
    ) -> Result<WriteReport, RepeaterError>;

    /// Wait for the port to come back after the reset a write ends with.
    ///
    /// Matched on the MAC, because the `/dev` node may not survive the re-enumeration and the
    /// MAC does. Sleeping a fixed interval and hoping is what this exists instead of.
    fn wait_for_port(
        &mut self,
        mac: Option<&str>,
        timeout: Duration,
    ) -> Result<RepeaterPort, RepeaterError>;

    fn open_console(&mut self, port: &RepeaterPort)
        -> Result<Box<dyn ConsoleSession>, RepeaterError>;
}

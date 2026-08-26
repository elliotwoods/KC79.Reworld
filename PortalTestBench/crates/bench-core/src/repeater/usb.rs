//! Writing a repeater over its own USB, with `espflash` as the loader.
//!
//! The call sequence is `espflash`'s own CLI recipe rather than a guess at one; what this adds
//! is the four things a bench needs and a command-line flasher does not: an identity read
//! *before* the write, progress on the bench's own timeline, errors in an operator's words,
//! and a port that can be found again after the reset re-enumerates it.
//!
//! # What it deliberately does not do
//!
//! It never calls `erase_flash`. The merged image covers the bootloader, the partition table,
//! `otadata` and the application; a chip erase would additionally destroy the spare OTA slot,
//! which is the one thing a repeater can fall back to. NVS goes either way -- the merged image
//! is contiguous from zero and writes `0xFF` over `0x9000..0xE000` -- and that is handled by
//! reading the index before the write and putting it back after, not by erasing less.

use std::time::{Duration, Instant};

use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::Flasher;
use espflash::target::{Chip, ProgressCallbacks};

use super::identity::{RepeaterPort, ESPRESSIF_VID, USB_SERIAL_JTAG_PID};
use super::programmer::{
    ConsoleSession, Progress, RepeaterProgrammer, UsbIdentity, WriteReport,
};
use super::provision::RepeaterError;

/// The ROM loader is met at 115200 and the transfer is renegotiated up.
const CONNECT_BAUD: u32 = 115_200;
const TRANSFER_BAUD: u32 = 921_600;

/// macOS enumerates the device node slightly before it can be opened. The same class of
/// problem `flash.rs` carries a settle constant for.
const PORT_SETTLE: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct EspProgrammer;

impl EspProgrammer {
    pub fn new() -> Self {
        Self
    }

    fn connect(&self, port: &RepeaterPort) -> Result<Flasher, RepeaterError> {
        let serial = serialport::new(&port.name, CONNECT_BAUD)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(500))
            .open_native()
            .map_err(|error| RepeaterError::Port(explain_open(&port.name, &error)))?;
        let info = serialport::UsbPortInfo {
            vid: ESPRESSIF_VID,
            // `espflash` chooses its reset strategy from the pid alone -- `UsbJtagSerialReset`
            // for 0x1001 -- so this is not decoration.
            pid: USB_SERIAL_JTAG_PID,
            serial_number: port.mac.clone(),
            manufacturer: None,
            product: port.product.clone(),
            interface: None,
        };
        let connection = Connection::new(
            serial,
            info,
            ResetAfterOperation::HardReset,
            ResetBeforeOperation::DefaultReset,
            TRANSFER_BAUD,
        );
        Flasher::connect(
            connection,
            /* use_stub */ true,
            /* verify */ true,
            /* skip */ false,
            // Naming the chip is what turns "you pointed the bench at an ESP32-S3 dev board"
            // into a sentence instead of a corrupted flash.
            Some(Chip::Esp32c3),
            Some(TRANSFER_BAUD),
        )
        .map_err(|error| RepeaterError::Write(explain(&error, &port.name)))
    }
}

impl RepeaterProgrammer for EspProgrammer {
    fn describe(&self) -> String {
        format!("espflash {}", env!("CARGO_PKG_VERSION"))
    }

    fn identify(&mut self, port: &RepeaterPort) -> Result<UsbIdentity, RepeaterError> {
        let mut flasher = self.connect(port)?;
        let info = flasher
            .device_info()
            .map_err(|error| RepeaterError::Write(explain(&error, &port.name)))?;
        Ok(UsbIdentity {
            chip: info.chip.to_string(),
            revision: info
                .revision
                .map(|(major, minor)| format!("v{major}.{minor}"))
                .unwrap_or_default(),
            mac: info.mac_address.unwrap_or_default().to_lowercase(),
            flash_bytes: info.flash_size.size() as u64,
        })
    }

    fn write(
        &mut self,
        port: &RepeaterPort,
        image: &[u8],
        progress: &mut Progress<'_>,
    ) -> Result<WriteReport, RepeaterError> {
        let started = Instant::now();
        let mut flasher = self.connect(port)?;
        let mut adapter = ProgressAdapter {
            progress,
            total: image.len().max(1),
        };
        flasher
            .write_bin_to_flash(0x0, image, &mut adapter)
            .map_err(|error| RepeaterError::Write(explain(&error, &port.name)))?;
        let md5 = flasher
            .checksum_md5(0x0, image.len() as u32)
            .map_err(|error| RepeaterError::Write(explain(&error, &port.name)))?;
        let md5 = format!("{md5:032x}");
        Ok(WriteReport {
            bytes: image.len(),
            seconds: started.elapsed().as_secs(),
            md5,
            // `verify: true` above means `espflash` refused already if it did not match; a
            // report that said `false` here would never be reachable.
            verified: true,
        })
    }

    fn wait_for_port(
        &mut self,
        mac: Option<&str>,
        timeout: Duration,
    ) -> Result<RepeaterPort, RepeaterError> {
        let deadline = Instant::now() + timeout;
        loop {
            let found = super::identity::candidates(&crate::survey::survey());
            let matched = match mac {
                Some(mac) => found
                    .iter()
                    .find(|port| port.mac.as_deref() == Some(mac))
                    .cloned(),
                // With no MAC to match, one attached repeater is the answer and two are not --
                // the same refusal `choose_port` makes, for the same reason.
                None => match found.as_slice() {
                    [only] => Some(only.clone()),
                    _ => None,
                },
            };
            if let Some(port) = matched {
                std::thread::sleep(PORT_SETTLE);
                return Ok(port);
            }
            if Instant::now() >= deadline {
                return Err(RepeaterError::Port(format!(
                    "the repeater reset but its USB port did not come back within {}s. Replug \
                     it and press Read status.",
                    timeout.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn open_console(
        &mut self,
        port: &RepeaterPort,
    ) -> Result<Box<dyn ConsoleSession>, RepeaterError> {
        Ok(Box::new(super::console::RepeaterConsole::open(&port.name)?))
    }
}

struct ProgressAdapter<'a, 'b> {
    progress: &'a mut Progress<'b>,
    total: usize,
}

impl ProgressCallbacks for ProgressAdapter<'_, '_> {
    fn init(&mut self, _addr: u32, total: usize) {
        self.total = total.max(1);
        (self.progress)("writing", 0.0);
    }

    fn update(&mut self, current: usize) {
        (self.progress)("writing", current as f64 / self.total as f64);
    }

    fn verifying(&mut self) {
        (self.progress)("verifying", 1.0);
    }

    fn finish(&mut self, _skipped: bool) {
        (self.progress)("verifying", 1.0);
    }
}

fn explain_open(port: &str, error: &serialport::Error) -> String {
    match error.kind() {
        serialport::ErrorKind::NoDevice => format!(
            "{port} is not there any more -- the repeater may have re-enumerated. Rescan."
        ),
        serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => format!(
            "{port} is already open -- close the serial monitor, or disconnect the Test tab's \
             link, and try again."
        ),
        _ => format!("{port}: {error}"),
    }
}

/// `espflash`'s errors, in the words of the person holding the board.
///
/// The mapping is deliberately shallow: anything not named here keeps the crate's own
/// `Display`, which is better than a wrong guess at what it meant.
fn explain(error: &espflash::Error, port: &str) -> String {
    match error {
        espflash::Error::Connection(_) => format!(
            "{port} did not answer the ESP32 ROM loader. Unplug and replug the repeater; if it \
             still refuses, hold BOOT while plugging it in."
        ),
        espflash::Error::ChipMismatch(wanted, found) => format!(
            "{port} is {found}, not the {wanted} a repeater carries. That is not a repeater."
        ),
        espflash::Error::ChipDetectError(detail) => format!(
            "{port} did not identify itself as an ESP32-C3 ({detail}). That is not a repeater, \
             or it is not in a state to answer."
        ),
        espflash::Error::VerifyFailed | espflash::Error::DigestMismatch(_, _) => format!(
            "{port} did not read back byte-for-byte. The repeater now holds a partial write and \
             will not boot. Re-run before powering it down."
        ),
        other => format!("{port}: {other}"),
    }
}

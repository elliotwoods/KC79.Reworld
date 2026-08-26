//! Provisioning an RS485 repeater: an ESP32-C3 bridge between the shared host bus and one
//! nine-Portal branch.
//!
//! Two routes, and the fleet needs both. **USB** writes the merged factory image at offset
//! zero over the chip's own USB-Serial-JTAG, and is the only way to install v3.0.0 the first
//! time -- v2.2.0 has no OTA at all. **RS485** sends the application into the spare OTA slot
//! in band, and is the only way to reach a repeater already mounted in a rack.
//!
//! # One identity, all the way through
//!
//! An ESP32-C3's USB-Serial-JTAG reports its MAC as the USB `iSerialNumber` -- `303A:1001`,
//! serial `F8:5B:1B:ED:8D:A4`. That is readable before the board has ever run our firmware,
//! it survives the reset that follows a write even though the `/dev` node may not, it is what
//! the running firmware reports as `mac` in `status`, and it is the same six bytes as
//! `RepeaterTarget::Mac`. So the same identity threads USB enumeration, the flash, the
//! `set-index` and the RS485 status read, and a pass can prove it provisioned the board it
//! flashed rather than the other one on the bench.
//!
//! # What is here, and what is not
//!
//! Everything in this module is behaviour: it classifies images, opens consoles, decides
//! verdicts. None of it owns a worker, a bus parameter or a route. `espflash` is behind the
//! `esp` feature exactly as `portal-swd` is behind `swd`, so an engine run that never touches
//! an ESP32 does not link one.

pub mod artefacts;
pub mod console;
pub mod identity;
pub mod programmer;
pub mod provision;
pub mod sim;

#[cfg(feature = "esp")]
pub mod usb;

pub use artefacts::{
    classify, discover_in, RepeaterArtefact, RepeaterDiscovery, RepeaterImageKind, RepeaterMissing,
};
pub use identity::{candidates, choose_port, mac_bytes, mac_string, RepeaterPort};
pub use programmer::{RepeaterProgrammer, WriteReport};
pub use provision::{
    evidence_verdict, parse_status, provision_over_usb, Expectation, RepeaterEvidence,
    RepeaterError, UsbOutcome, UsbPass,
};

//! The flash-controller and option-byte sequences, written against a two-method memory port.
//!
//! # Why this is not in `probe.rs`
//!
//! These are the only routines in the crate that can permanently change a board in a way no
//! amount of reflashing undoes. An option-byte write that sets `nBOOT_SEL` wrong, or that lands
//! `RDP` somewhere other than level 0, produces a part that cannot be recovered with the
//! equipment on this bench. Code like that should be readable in isolation and it should have
//! tests, and neither is possible while it is tangled with USB enumeration and `ArmDebugInterface`
//! lifetimes.
//!
//! So the port is deliberately tiny — [`Mem`], two methods, `u32` in and `u32` out. `probe.rs`
//! implements it over probe-rs in a dozen lines; the tests implement it over a struct that models
//! the lock bits and the busy flag. Both drive exactly the same sequence.
//!
//! # What is verified and what is not
//!
//! The register *addresses* and the key constants are from ST's headers, checked when
//! `addr`/`bits` were written. The `FLASH_OPTR` **bit positions above bit 23** — `nBOOT_SEL`,
//! `nBOOT1`, `nBOOT0`, `NRST_MODE` — came from libopencm3 rather than from RM0454, and that is
//! still the largest unverified fact in this design.
//!
//! Two things keep that from being dangerous in practice. [`OptionBytePolicy::desired`] preserves
//! the `RDP` field unconditionally, whatever the mask says, so no combination of policy and
//! current value can write readout protection. And on a virgin part the policy's golden value
//! *is* ST's factory default, so `needs_programming` is false and none of this runs at all — the
//! sequence only executes on a board whose option bytes someone has already changed.
//!
//! [`OptionBytePolicy::desired`]: crate::image::OptionBytePolicy::desired
//! [`OptionBytePolicy`]: crate::image::OptionBytePolicy

use std::time::Duration;

use crate::{addr, bits, keys};

/// The whole of what these sequences need from a debug probe.
///
/// Word-at-a-time on purpose: every register here is 32 bits, the sequences are a few dozen
/// accesses in total, and a wider port would only be a wider thing to fake.
pub trait Mem {
    fn read32(&mut self, at: u32) -> Result<u32, String>;
    fn write32(&mut self, at: u32, value: u32) -> Result<(), String>;
}

/// How many times to look at `FLASH_SR` before giving up, at roughly 1 ms a look.
///
/// A mass erase on a 128 kB G0 is tens of milliseconds and an option-byte write is under one, so
/// 500 is not a tuned value — it is far enough beyond both that reaching it means the part has
/// stopped answering rather than that it is slow.
const BUSY_POLLS: usize = 500;

/// `RCC_APBENR1.DBGEN`. Clear out of reset, which is why `DBGMCU` writes need it set first.
pub const RCC_APBENR1_DBGEN: u32 = 1 << 27;

/// Everything in `FLASH_SR` that means a failure, which is [`bits::FLASH_SR_CLEAR_MASK`] without
/// `EOP` — end-of-operation is the flag that says it *worked*.
pub const FLASH_SR_ERROR_MASK: u32 = bits::FLASH_SR_CLEAR_MASK & !1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlashFault {
    /// The probe could not complete an access. The target went away, or the probe did.
    Bus(String),
    /// `BSY1`/`CFGBSY` never cleared.
    Busy,
    /// The unlock key sequence did not clear the lock bit. On a G0 a *wrong* key latches the lock
    /// until the next reset, so this is reported rather than retried.
    Locked(&'static str),
    /// The controller flagged an error.
    Status { sr: u32 },
    /// The write completed and the part came back holding something else.
    NotTaken { wanted: u32, found: u32 },
}

impl core::fmt::Display for FlashFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FlashFault::Bus(detail) => write!(f, "flash register access failed: {detail}"),
            FlashFault::Busy => f.write_str("the flash controller stayed busy"),
            FlashFault::Locked(what) => write!(
                f,
                "{what} stayed locked after the key sequence; a wrong key latches the lock until \
                 the next reset, so this needs a power cycle rather than a retry"
            ),
            FlashFault::Status { sr } => {
                write!(
                    f,
                    "the flash controller reported an error, FLASH_SR {sr:#010X}"
                )
            }
            FlashFault::NotTaken { wanted, found } => write!(
                f,
                "option bytes did not take: wrote {wanted:#010X}, the part reloaded {found:#010X}"
            ),
        }
    }
}

fn bus(err: String) -> FlashFault {
    FlashFault::Bus(err)
}

/// Wait for the controller to go idle, and hand back the `FLASH_SR` that said so.
///
/// Both flags, not just `BSY1`: `CFGBSY` covers the window where a programming configuration is
/// still being applied, and starting an option-byte write inside it is how `PGSERR` happens.
pub fn wait_idle(mem: &mut dyn Mem) -> Result<u32, FlashFault> {
    for attempt in 0..BUSY_POLLS {
        let sr = mem.read32(addr::FLASH_SR).map_err(bus)?;
        if sr & (bits::FLASH_SR_BSY1 | bits::FLASH_SR_CFGBSY) == 0 {
            return Ok(sr);
        }
        // Not before the first look: the overwhelmingly common case is already-idle, and paying a
        // millisecond for it on every call adds up across a sequence.
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Err(FlashFault::Busy)
}

/// Unlock `FLASH_CR`, and then the option registers behind it.
///
/// Only ever when the lock is actually set. Writing the key sequence to an already-unlocked
/// controller is not a no-op — it is an out-of-sequence key, and the part responds by re-locking.
fn unlock(mem: &mut dyn Mem) -> Result<(), FlashFault> {
    if mem.read32(addr::FLASH_CR).map_err(bus)? & bits::FLASH_CR_LOCK != 0 {
        mem.write32(addr::FLASH_KEYR, keys::KEY1).map_err(bus)?;
        mem.write32(addr::FLASH_KEYR, keys::KEY2).map_err(bus)?;
        if mem.read32(addr::FLASH_CR).map_err(bus)? & bits::FLASH_CR_LOCK != 0 {
            return Err(FlashFault::Locked("FLASH_CR"));
        }
    }
    // Strictly after the above: `OPTLOCK` cannot be cleared while `LOCK` is set.
    if mem.read32(addr::FLASH_CR).map_err(bus)? & bits::FLASH_CR_OPTLOCK != 0 {
        mem.write32(addr::FLASH_OPTKEYR, keys::OPTKEY1)
            .map_err(bus)?;
        mem.write32(addr::FLASH_OPTKEYR, keys::OPTKEY2)
            .map_err(bus)?;
        if mem.read32(addr::FLASH_CR).map_err(bus)? & bits::FLASH_CR_OPTLOCK != 0 {
            return Err(FlashFault::Locked("the option registers"));
        }
    }
    Ok(())
}

/// Write `desired` into the option bytes, up to but not including the reload.
///
/// Split from [`launch_option_bytes`] because the reload resets the part and takes the debug
/// connection with it: the caller has to be ready to lose the session at a moment it chooses,
/// not partway through a helper.
pub fn program_option_bytes(mem: &mut dyn Mem, desired: u32) -> Result<(), FlashFault> {
    wait_idle(mem)?;
    // Clear whatever was sticky before we arrived, so the status read after `OPTSTRT` is about
    // this write and not about something that happened to the board an hour ago.
    mem.write32(addr::FLASH_SR, bits::FLASH_SR_CLEAR_MASK)
        .map_err(bus)?;
    unlock(mem)?;

    mem.write32(addr::FLASH_OPTR, desired).map_err(bus)?;
    let cr = mem.read32(addr::FLASH_CR).map_err(bus)?;
    mem.write32(addr::FLASH_CR, cr | bits::FLASH_CR_OPTSTRT)
        .map_err(bus)?;

    let sr = wait_idle(mem)?;
    if sr & FLASH_SR_ERROR_MASK != 0 {
        return Err(FlashFault::Status { sr });
    }
    Ok(())
}

/// Set `OBL_LAUNCH`, which reloads the option bytes by resetting the part.
///
/// Returns nothing, and swallows the error, because the successful case *is* a failed access:
/// the reset lands while the write is on the wire, so a probe reporting no acknowledgement is the
/// expected outcome rather than a problem. Whether it worked is decided by re-attaching and
/// reading `FLASH_OPTR` back, which is the only answer worth having anyway.
pub fn launch_option_bytes(mem: &mut dyn Mem) {
    if let Ok(cr) = mem.read32(addr::FLASH_CR) {
        let _ = mem.write32(addr::FLASH_CR, cr | bits::FLASH_CR_OBL_LAUNCH);
    }
}

/// Stop the independent watchdog while the core is halted.
///
/// `IWDG_SW` is 1 on every board seen so far, so the watchdog is off until firmware starts it —
/// but a board being reflashed is one whose current firmware is unknown, and an erase that gets
/// two thirds of the way through before a watchdog reset lands leaves exactly the half-written
/// part this tool exists to avoid. `DBGMCU` is clocked by `RCC_APBENR1.DBGEN`, which is clear out
/// of reset, so that comes first or the freeze write goes nowhere.
pub fn freeze_watchdog(mem: &mut dyn Mem, freeze: bool) -> Result<(), FlashFault> {
    let en = mem.read32(addr::RCC_APBENR1).map_err(bus)?;
    if en & RCC_APBENR1_DBGEN == 0 {
        mem.write32(addr::RCC_APBENR1, en | RCC_APBENR1_DBGEN)
            .map_err(bus)?;
    }
    let fz = mem.read32(addr::DBGMCU_APBFZ1).map_err(bus)?;
    let next = if freeze {
        fz | bits::DBG_IWDG_STOP
    } else {
        fz & !bits::DBG_IWDG_STOP
    };
    if next != fz {
        mem.write32(addr::DBGMCU_APBFZ1, next).map_err(bus)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A flash controller that models the parts of the real one this code can get wrong: the two
    /// lock bits, the key state machine that re-locks on an out-of-sequence key, and `OPTSTRT`
    /// latching `FLASH_OPTR` into a shadow that only a reload makes visible.
    struct FakeFlash {
        words: HashMap<u32, u32>,
        /// How many correct keys have arrived in a row, per register.
        keyed: HashMap<u32, u8>,
        /// What `OPTSTRT` latched. `None` until an option-byte write completes.
        pending_optr: Option<u32>,
        /// Set once `OBL_LAUNCH` has been seen.
        reloaded: bool,
        /// Errors to raise in `FLASH_SR` at the end of the next option-byte write.
        inject_sr: u32,
        /// Refuse every access from this one onward, as a target that went away does.
        fail_from: Option<usize>,
        accesses: usize,
    }

    impl FakeFlash {
        /// A part as it comes out of reset: everything locked, factory option bytes.
        fn new() -> Self {
            let mut words = HashMap::new();
            words.insert(addr::FLASH_CR, bits::FLASH_CR_LOCK | bits::FLASH_CR_OPTLOCK);
            words.insert(addr::FLASH_SR, 0);
            words.insert(addr::FLASH_OPTR, 0xFFFF_FEAA);
            words.insert(addr::RCC_APBENR1, 0);
            words.insert(addr::DBGMCU_APBFZ1, 0);
            Self {
                words,
                keyed: HashMap::new(),
                pending_optr: None,
                reloaded: false,
                inject_sr: 0,
                fail_from: None,
                accesses: 0,
            }
        }

        fn get(&self, at: u32) -> u32 {
            self.words.get(&at).copied().unwrap_or(0)
        }

        /// What the part would read after the reset `OBL_LAUNCH` causes.
        fn optr_after_reload(&self) -> u32 {
            match (self.reloaded, self.pending_optr) {
                (true, Some(value)) => value,
                _ => self.get(addr::FLASH_OPTR),
            }
        }

        fn budget(&mut self) -> Result<(), String> {
            self.accesses += 1;
            match self.fail_from {
                Some(n) if self.accesses > n => Err("target stopped answering".into()),
                _ => Ok(()),
            }
        }
    }

    impl Mem for FakeFlash {
        fn read32(&mut self, at: u32) -> Result<u32, String> {
            self.budget()?;
            Ok(self.get(at))
        }

        fn write32(&mut self, at: u32, value: u32) -> Result<(), String> {
            self.budget()?;
            match at {
                addr::FLASH_KEYR | addr::FLASH_OPTKEYR => {
                    let (first, second, lock) = if at == addr::FLASH_KEYR {
                        (keys::KEY1, keys::KEY2, bits::FLASH_CR_LOCK)
                    } else {
                        (keys::OPTKEY1, keys::OPTKEY2, bits::FLASH_CR_OPTLOCK)
                    };
                    let step = self.keyed.entry(at).or_insert(0);
                    match (*step, value) {
                        (0, v) if v == first => *step = 1,
                        (1, v) if v == second => {
                            *step = 0;
                            let cr = self.get(addr::FLASH_CR);
                            self.words.insert(addr::FLASH_CR, cr & !lock);
                        }
                        // Out of sequence. The real part latches the lock; so does this.
                        _ => {
                            *step = 0;
                            let cr = self.get(addr::FLASH_CR);
                            self.words.insert(addr::FLASH_CR, cr | lock);
                        }
                    }
                }
                addr::FLASH_SR => {
                    // Write-one-to-clear.
                    let sr = self.get(addr::FLASH_SR);
                    self.words.insert(addr::FLASH_SR, sr & !value);
                }
                addr::FLASH_CR => {
                    self.words.insert(addr::FLASH_CR, value);
                    if value & bits::FLASH_CR_OPTSTRT != 0 {
                        self.pending_optr = Some(self.get(addr::FLASH_OPTR));
                        self.words.insert(addr::FLASH_SR, self.inject_sr);
                    }
                    if value & bits::FLASH_CR_OBL_LAUNCH != 0 {
                        self.reloaded = true;
                    }
                }
                _ => {
                    self.words.insert(at, value);
                }
            }
            Ok(())
        }
    }

    #[test]
    fn an_option_byte_write_unlocks_latches_and_survives_a_reload() {
        let mut part = FakeFlash::new();
        let wanted = 0xFFFF_FEAA & !bits::OPTR_NBOOT1;

        program_option_bytes(&mut part, wanted).expect("the sequence should complete");
        assert_eq!(
            part.pending_optr,
            Some(wanted),
            "OPTSTRT should have latched"
        );
        assert_eq!(
            part.get(addr::FLASH_CR) & (bits::FLASH_CR_LOCK | bits::FLASH_CR_OPTLOCK),
            0,
            "both locks should be open by the time OPTSTRT is set"
        );

        launch_option_bytes(&mut part);
        assert_eq!(part.optr_after_reload(), wanted);
    }

    #[test]
    fn an_already_unlocked_controller_is_not_keyed_again() {
        let mut part = FakeFlash::new();
        // Someone got here first.
        part.words.insert(addr::FLASH_CR, 0);

        program_option_bytes(&mut part, 0xFFFF_FEAA).expect("the sequence should complete");

        // The whole point: a redundant key sequence would have re-locked the part, and the
        // failure would only show up as the *next* pass mysteriously not taking.
        assert_eq!(part.get(addr::FLASH_CR) & bits::FLASH_CR_LOCK, 0);
        assert_eq!(part.get(addr::FLASH_CR) & bits::FLASH_CR_OPTLOCK, 0);
    }

    #[test]
    fn a_controller_error_after_optstrt_is_reported_rather_than_reloaded() {
        let mut part = FakeFlash::new();
        part.inject_sr = 1 << 15; // OPTVERR

        let fault = program_option_bytes(&mut part, 0xFFFF_FEAA).expect_err("should fail");
        assert!(matches!(fault, FlashFault::Status { sr } if sr & (1 << 15) != 0));
        // And crucially it stopped before the reload, so the part is still running what it was.
        assert!(!part.reloaded);
    }

    #[test]
    fn eop_alone_is_success() {
        let mut part = FakeFlash::new();
        part.inject_sr = 1; // EOP is in the clear mask but is not an error
        program_option_bytes(&mut part, 0xFFFF_FEAA).expect("EOP means it worked");
    }

    #[test]
    fn a_target_that_goes_away_mid_sequence_is_a_bus_fault() {
        let mut part = FakeFlash::new();
        part.fail_from = Some(3);

        let fault = program_option_bytes(&mut part, 0xFFFF_FEAA).expect_err("should fail");
        assert!(matches!(fault, FlashFault::Bus(_)), "got {fault:?}");
        assert!(!part.reloaded);
    }

    #[test]
    fn a_stuck_busy_flag_times_out_rather_than_hanging() {
        struct AlwaysBusy;
        impl Mem for AlwaysBusy {
            fn read32(&mut self, _at: u32) -> Result<u32, String> {
                Ok(bits::FLASH_SR_BSY1)
            }
            fn write32(&mut self, _at: u32, _value: u32) -> Result<(), String> {
                Ok(())
            }
        }
        assert_eq!(wait_idle(&mut AlwaysBusy), Err(FlashFault::Busy));
    }

    #[test]
    fn a_latched_lock_is_named_rather_than_retried() {
        struct NeverUnlocks;
        impl Mem for NeverUnlocks {
            fn read32(&mut self, at: u32) -> Result<u32, String> {
                Ok(if at == addr::FLASH_CR {
                    bits::FLASH_CR_LOCK | bits::FLASH_CR_OPTLOCK
                } else {
                    0
                })
            }
            fn write32(&mut self, _at: u32, _value: u32) -> Result<(), String> {
                Ok(())
            }
        }
        assert_eq!(
            program_option_bytes(&mut NeverUnlocks, 0xFFFF_FEAA),
            Err(FlashFault::Locked("FLASH_CR"))
        );
    }

    #[test]
    fn freezing_the_watchdog_clocks_dbgmcu_first() {
        let mut part = FakeFlash::new();
        freeze_watchdog(&mut part, true).expect("should succeed");

        assert_eq!(
            part.get(addr::RCC_APBENR1) & RCC_APBENR1_DBGEN,
            RCC_APBENR1_DBGEN
        );
        assert_eq!(
            part.get(addr::DBGMCU_APBFZ1) & bits::DBG_IWDG_STOP,
            bits::DBG_IWDG_STOP
        );

        freeze_watchdog(&mut part, false).expect("should succeed");
        assert_eq!(part.get(addr::DBGMCU_APBFZ1) & bits::DBG_IWDG_STOP, 0);
        // Leaving DBGEN set is deliberate -- it is the reset default that the *next* reset
        // restores, and clearing it here would fight the debug session that is still open.
        assert_eq!(
            part.get(addr::RCC_APBENR1) & RCC_APBENR1_DBGEN,
            RCC_APBENR1_DBGEN
        );
    }

    #[test]
    fn the_error_mask_covers_every_failure_flag_and_no_success_one() {
        // EOP is bit 0 and means the operation finished; everything else in the clear mask is a
        // reason to stop. Spelled out so a change to either constant has to be deliberate.
        assert_eq!(FLASH_SR_ERROR_MASK & 1, 0);
        assert_eq!(FLASH_SR_ERROR_MASK | 1, bits::FLASH_SR_CLEAR_MASK);
        for bit in [1u32, 3, 4, 5, 6, 7, 8, 9, 14, 15] {
            assert_ne!(
                FLASH_SR_ERROR_MASK & (1 << bit),
                0,
                "SR bit {bit} should be treated as an error"
            );
        }
    }
}

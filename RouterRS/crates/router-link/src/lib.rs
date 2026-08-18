//! The RS485 link to a portal.
//!
//! Extracted from `router-core` so that anything needing to *talk to a portal* -- the Router
//! app, the headless runner, the test bench -- can depend on the wire behaviour without also
//! depending on the installation model, the kinematics, the image pipeline and the OSC/REST
//! servers that sit above it.
//!
//! The timing constants in [`rs485::Rs485Params`] are preserved bit-for-bit from the original
//! C++ Router and are golden-tested against captured frames. They are not tuning knobs: a
//! change here makes every measurement taken with a different value incomparable.

pub mod fw_update;
pub mod rs485;
pub mod sim;

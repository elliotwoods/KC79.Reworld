//! Talking to the module on the bench.
//!
//! # One poller, always
//!
//! [`Link`] is deliberately **neither `Clone` nor `Sync`**. It is moved into the worker thread
//! once and [`Link::poll`] is called from exactly one place. That is not tidiness, it is
//! correctness: the production firmware's `logger` field **drains its outbox when read**, so
//! log lines are delivered exactly once, to whoever polled. Two pollers means each sees roughly
//! half the log and neither looks obviously wrong -- the kind of fault that costs a day.
//!
//! Everything else in the app -- the GUI parameters, the telemetry rings, `/api/bench/log`,
//! the NDJSON writer -- reads a fan-out mirror the worker fills. HTTP handlers never touch
//! hardware; they enqueue an [`Op`] and read the mirror.
//!
//! # One vocabulary, several dialects
//!
//! An [`Op`] says *what the operator wants*. Each transport knows how to say it: MessagePack
//! over RS485, single letters to the bench firmware, the terse interactive menu on the
//! production firmware's USART1. A transport that cannot express an op returns
//! [`LinkError::Unsupported`], and the plan validator catches that **before a run starts**
//! rather than halfway through a sequence that has already moved the motor.

pub mod ascii;
pub mod line;
pub mod rs485;

use crate::dut::{Axis, FirmwareKind, GearRatio, Health};

/// The two physical communication lanes a bench may keep open at the same time.
///
/// A dialect still lives in [`LinkKind`]; this only answers which socket owns it. Keeping the
/// distinction explicit prevents connecting RS485 from silently evicting a useful serial log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Serial,
    Rs485,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Channel::Serial => "serial",
            Channel::Rs485 => "rs485",
        }
    }
}

/// What kind of link this is, and to what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    pub kind: LinkKind,
    /// `COM7`, `192.168.0.50:4196`, or `simulated`.
    pub endpoint: String,
    /// What the firmware announced about itself, if it has said anything yet.
    pub banner: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// Direct USB serial to the production firmware's USART1 interactive menu.
    Vcp,
    /// The bench firmware's ASCII line protocol, same physical port.
    BenchAscii,
    /// RS485 through a router board on a COM port.
    Rs485Serial,
    /// RS485 through an Ethernet gateway, raw TCP.
    Rs485Tcp,
    /// A modelled module. No probe, no port, no board.
    Sim,
}

impl LinkKind {
    pub fn channel(self) -> Channel {
        match self {
            LinkKind::Vcp | LinkKind::BenchAscii | LinkKind::Sim => Channel::Serial,
            LinkKind::Rs485Serial | LinkKind::Rs485Tcp => Channel::Rs485,
        }
    }

    /// Which firmware image can be on the other end of this link.
    ///
    /// The pairing is physical, not a policy: the bench image speaks only its own line
    /// protocol and does not implement the RS485 stack at all.
    pub fn expects_firmware(self) -> Option<FirmwareKind> {
        match self {
            LinkKind::BenchAscii => Some(FirmwareKind::Bench),
            LinkKind::Vcp | LinkKind::Rs485Serial | LinkKind::Rs485Tcp => {
                Some(FirmwareKind::Production)
            }
            LinkKind::Sim => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LinkKind::Vcp => "vcp",
            LinkKind::BenchAscii => "bench-ascii",
            LinkKind::Rs485Serial => "rs485-serial",
            LinkKind::Rs485Tcp => "rs485-tcp",
            LinkKind::Sim => "sim",
        }
    }
}

/// One thing to do to the module.
///
/// This is the whole command vocabulary of the bench. The GUI's buttons, the HTTP
/// `/api/bench/command` route and a plan's steps all construct these, so the three can never
/// drift apart.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Ask who is there. Cheap, safe, and the only thing sent to an unidentified module.
    Identify,
    /// Full status report: positions, health, uptime, version, and the log outbox.
    Poll,
    /// Positions only. Much cheaper than [`Op::Poll`] and does not drain the log.
    PollPosition,
    /// The firmware's whole-module startup routine (`measureCycle` then `calibrate`).
    ///
    /// Both axes, always -- it is a module-level routine, not a per-axis one.
    Startup,
    Home {
        axis: Axis,
    },
    Unjam {
        axis: Axis,
    },
    Calibrate {
        axis: Axis,
    },
    MeasureBacklash {
        axis: Axis,
    },
    MoveTo {
        axis: Axis,
        usteps: i32,
        profile: Option<MotionProfile>,
    },
    /// A coordinated two-axis target, used by the pilot.
    ///
    /// Two independent `MoveTo` packets share an RS485 collation address and one can replace the
    /// other before transmission. The firmware already has an atomic `m: [a, b]` command, so the
    /// host vocabulary needs to preserve that atomicity too.
    MoveAxes {
        a: i32,
        b: i32,
    },
    SetMotionProfile {
        axis: Axis,
        profile: MotionProfile,
    },
    SetCurrent {
        amps: f32,
    },
    ReadSettings,
    WriteSettings {
        current_ma: u16,
        full_current_home_recovery: bool,
    },
    SetMicrostep {
        resolution: u32,
    },
    /// Set the optical comparator threshold. Only meaningful once calibrated.
    SetHomeThreshold {
        value: i32,
    },
    /// One revolution at a fixed, settled threshold, reporting every comparator transition.
    ///
    /// The only instrument whose answer may be used to choose an operating threshold: a fixed
    /// threshold read by the *moving* comparator, through the same debounced latch homing uses.
    /// A swept threshold reads 10-20 counts high because of the ~100 ms RC on the DAC, and a
    /// settled binary-search probe has reported "no crossing" while parked on a flag a census
    /// found on every lap. See [`crate::threshold`].
    ///
    /// `speed` of `None` means the firmware's own seek speed.
    Census {
        axis: Axis,
        threshold: u8,
        speed: Option<i32>,
    },
    /// Ask the module to abandon whatever routine it is running.
    Escape,
    Reboot,
}

/// Deliberately advanced payloads sent without translating them into an [`Op`].
///
/// These still use the link's normal framing and single-owner queue. They are "raw" at the
/// firmware protocol level, not arbitrary bytes injected around COBS or MessagePack.
#[derive(Debug, Clone, PartialEq)]
pub enum RawSignal {
    VcomText { text: String, ending: LineEnding },
    Rs485Json { body: serde_json::Value },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    None,
    Cr,
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::None => b"",
            Self::Cr => b"\r",
            Self::Lf => b"\n",
            Self::Crlf => b"\r\n",
        }
    }
}

/// How a move is ramped.
///
/// All three are needed together: the firmware's `move` takes
/// `[target, maxVelocity, acceleration, minVelocity]`, and a "speed" on its own is not
/// expressible. That is not an inconvenience to route around — **acceleration decides where the
/// stall cliff is**. At 32:1 the motor stalls at 30–32k µsteps/s when ramped at 100k/s², but
/// cruises past 36k when ramped at 20k/s² or less. A move that named only its top speed would
/// stall or not depending on a value nobody wrote down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionProfile {
    pub max_velocity: i32,
    pub acceleration: i32,
    pub min_velocity: i32,
}

impl Default for MotionProfile {
    /// A gentle profile that is safely inside the measured envelope on both gearings.
    ///
    /// 24k µsteps/s at 20k/s² is the point the 32:1 sustained-duty study ran at for 3h20m
    /// across 1,720 moves with zero degradation. The 16:1 stalls well below this, which is why
    /// the engine caps that gearing separately rather than lowering this for everyone.
    fn default() -> Self {
        Self {
            max_velocity: 24_000,
            acceleration: 20_000,
            min_velocity: 1_000,
        }
    }
}

impl Op {
    /// Whether this op can move the motor or lose the datum.
    ///
    /// Used by the dead-man and by the plan validator. Reads are always allowed; anything that
    /// can move a prism is gated.
    pub fn is_destructive(&self) -> bool {
        !matches!(
            self,
            Op::Identify | Op::Poll | Op::PollPosition | Op::ReadSettings
        )
    }
}

/// Everything a module can tell us, normalised across the dialects.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkEvent {
    /// The module identified itself.
    Identified {
        firmware: FirmwareKind,
        version: Option<String>,
        ratio: GearRatio,
        usteps_per_rev: Option<i32>,
        banner: String,
    },
    /// A position report for one axis.
    Position {
        axis: Axis,
        position: i32,
        target: Option<i32>,
    },
    /// A health report for one axis.
    HealthReport {
        axis: Axis,
        health: Health,
    },
    /// Firmware uptime, from a full status report.
    Uptime {
        seconds: i32,
    },
    Provisioning {
        serial: u32,
    },
    Settings {
        current_ma: u16,
        full_current_home_recovery: bool,
        source: Option<String>,
    },
    /// A log line the firmware produced.
    Log {
        level: u8,
        message: String,
        firmware_ms: Option<u64>,
    },
    /// The optical comparator's current state, for the bench firmware's live stream.
    Sensor {
        active: bool,
        threshold: i32,
    },
    /// A routine reported progress or completion in the bench dialect, e.g. `O,done`.
    Token {
        kind: char,
        fields: Vec<String>,
    },
    /// The module acknowledged a command.
    /// An RS485 source address answered. Discovery consumes this even when the reply body is not
    /// selected as the active module.
    PeerSeen {
        source: i8,
    },
    /// A command acknowledgement. Line protocols do not carry an address.
    Ack {
        source: Option<i8>,
    },
    /// Something went wrong that is worth recording but is not fatal to the link.
    Fault(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("the {kind} link cannot express {op}")]
    Unsupported { kind: &'static str, op: String },

    #[error("no link is open")]
    NotOpen,

    #[error("{endpoint} is already open in another program{}", detail.as_deref().unwrap_or(""))]
    Busy {
        endpoint: String,
        detail: Option<String>,
    },

    #[error("{0}")]
    Io(String),
}

/// A link to the module under test.
///
/// See the module docs for why this is neither `Clone` nor `Sync`.
pub trait Link: Send {
    fn info(&self) -> LinkInfo;

    fn open(&mut self) -> Result<LinkInfo, LinkError>;

    fn close(&mut self);

    fn is_open(&self) -> bool;

    /// Select the addressed peer for a multi-drop transport.
    fn set_target(&mut self, _target: i8) -> Result<(), LinkError> {
        Err(LinkError::Unsupported {
            kind: "link",
            op: "select target".into(),
        })
    }

    /// Live transport diagnostics. Links without counters return the default.
    fn diagnostics(&self) -> LinkDiagnostics {
        LinkDiagnostics::default()
    }

    /// Send one op. Returns once it is on the wire, not once the module has finished:
    /// the long routines acknowledge immediately and then run for seconds to minutes, so
    /// completion arrives later as a [`LinkEvent`].
    fn send(&mut self, op: &Op) -> Result<(), LinkError>;

    fn send_raw(&mut self, signal: &RawSignal) -> Result<(), LinkError> {
        Err(LinkError::Unsupported {
            kind: self.info().kind.name(),
            op: format!("raw {signal:?}"),
        })
    }

    /// Drain whatever the module has said since the last call.
    ///
    /// **The only reader.** Called from one place in the worker tick; see the module docs.
    fn poll(&mut self, now_ms: u64) -> Vec<LinkEvent>;
}

/// Small, stable diagnostics suitable for a status panel and API snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct LinkDiagnostics {
    pub tx: u64,
    pub rx: u64,
    /// Explicit boolean acknowledgements observed from the selected peer.
    pub acks: u64,
    pub ack_timeouts: u64,
    pub decode_errors: u64,
    pub outbox: usize,
    pub last_rx_age_ms: Option<u64>,
    pub last_tx_age_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_not_destructive_and_everything_else_is() {
        assert!(!Op::Identify.is_destructive());
        assert!(!Op::Poll.is_destructive());
        assert!(!Op::PollPosition.is_destructive());
        assert!(Op::Home { axis: Axis::A }.is_destructive());
        assert!(Op::Unjam { axis: Axis::B }.is_destructive());
        assert!(
            Op::MoveTo {
                axis: Axis::A,
                usteps: 0,
                profile: None
            }
            .is_destructive()
        );
        // Escape moves nothing itself, but it changes what the motor is doing, so it is gated
        // with the rest rather than being treated as a read.
        assert!(Op::Escape.is_destructive());
    }

    #[test]
    fn each_link_kind_expects_the_firmware_that_can_answer_it() {
        assert_eq!(
            LinkKind::BenchAscii.expects_firmware(),
            Some(FirmwareKind::Bench)
        );
        assert_eq!(
            LinkKind::Vcp.expects_firmware(),
            Some(FirmwareKind::Production)
        );
        assert_eq!(
            LinkKind::Rs485Tcp.expects_firmware(),
            Some(FirmwareKind::Production)
        );
        // The simulator answers whatever it is asked to model.
        assert_eq!(LinkKind::Sim.expects_firmware(), None);
    }
}

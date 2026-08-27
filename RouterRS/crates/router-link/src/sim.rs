//! In-process firmware simulator: a `SerialDevice` that behaves like an
//! RS485 bus of portal units running the PortalFW protocol. Used by
//! `--simulate` runs, integration tests, and GUI development without
//! hardware.
//!
//! Behavior modeled on `PortalFW/src/Modules/App.cpp`:
//! - nil body -> bare bool ACK
//! - `{"poll": nil}` -> full status report (app / mca / mcb / logger)
//! - `{"p": nil}` -> `{"p": [curA, curB, tgtA, tgtB]}`
//! - `{"m": [a, b]}` -> set targets, reply with positions
//! - `{"keyframe": {...}}` -> block-addressed targets (broadcast, no reply)
//! - other maps -> ACK true (broadcasts are never ACKed)
//!
//! Fault injection: per-portal death, reply latency/jitter, drop rate, and
//! random line noise for exercising the decode-error paths.
//!
//! # The bootloader half
//!
//! A simulated portal is either running its application or sitting in a v6
//! bootloader, and `"FW!KC79"` is what moves it from the first state to the
//! second -- exactly as on hardware, where that word is the only one a running
//! application acts on. In the bootloader state it answers every `bl` verb,
//! takes `{offset: bin}` data frames into a bank buffer with a received-chunk
//! bitmap, and returns to the application on `run`.
//!
//! That is what lets `--simulate` and the bench's `LinkKind::Sim` drive a whole
//! [`crate::fw_session`] -- discovery, erase, stream, map, repair, verify, run --
//! with no hardware attached. `legacy_portals` models the other half of the
//! fleet: those units never answer a `bl` request, which is what a v4/v5
//! bootloader does and what makes the host fall back to the blind path.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use router_proto::bootloader::{self, err, BlState, BlVerb};
use router_proto::envelope::{encode_reply_fix8, encode_reply_trailer};
use router_proto::value::{key, map};
use router_proto::{crc32c, decode_envelope, encode_frame, layout, FrameAccumulator, Value};

use crate::rs485::SerialDevice;

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub portal_count: u8,
    /// Base reply latency.
    pub latency: Duration,
    /// Extra random latency (uniform 0..jitter).
    pub jitter: Duration,
    /// Probability (0..1) of dropping a reply entirely.
    pub drop_rate: f32,
    /// Portal IDs that never answer (simulated dead units).
    pub dead_portals: Vec<u8>,
    /// Portal IDs that spam error logs in their status reports.
    pub noisy_portals: Vec<u8>,
    /// Probability (0..1) of a reply being corrupted with line noise.
    pub corrupt_rate: f32,
    /// Steps per second the simulated motors move at.
    pub motor_speed: f32,
    pub firmware_version: String,
    /// Whether a broadcast (`target == -1`) draws replies.
    ///
    /// Off by default, which is the truth for a column: eighteen units answering one broadcast
    /// collide on the wire, so the Router never asks them to. A bench driving a *single* module
    /// is the opposite case -- `Op::Identify` is deliberately broadcast there, because the id of
    /// a module on a desk is often exactly what is unknown -- and a lone unit answering is what
    /// real hardware does. Turn it on only when `portal_count` is 1.
    pub answer_broadcast: bool,
    /// Portal IDs whose bootloader is the fielded v4/v5 one: it accepts data
    /// frames but never transmits, so it answers no `bl` request at all. This
    /// is what makes a simulated fleet *mixed*, and therefore what exercises the
    /// host's fallback to the blind broadcast path.
    pub legacy_portals: Vec<u8>,
    /// How long `begin` takes before it answers. The real bootloader erases one
    /// page per main-loop pass -- about 1.2 s for the 53-page v6 bank -- and
    /// replies only when that has finished, which is the whole reason `begin` is
    /// acknowledged rather than fire-and-forget.
    pub erase_time: Duration,
    /// Where a simulated bootloader expects its application to start.
    pub app_base: u32,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            portal_count: 18,
            latency: Duration::from_millis(3),
            jitter: Duration::from_millis(2),
            drop_rate: 0.0,
            dead_portals: Vec::new(),
            noisy_portals: Vec::new(),
            corrupt_rate: 0.0,
            motor_speed: 30_000.0,
            firmware_version: "sim-1.0".into(),
            answer_broadcast: false,
            legacy_portals: Vec::new(),
            erase_time: Duration::from_millis(1_200),
            app_base: layout::APP_BASE,
        }
    }
}

/// A simulated bootloader, and the session it may have open.
///
/// The bank is carried in full so `verify` can be answered the way the firmware
/// answers it -- by CRC-32C-ing what was actually programmed -- rather than by
/// agreeing with whatever the host declared.
struct BootSim {
    base: u32,
    /// The application bank as this board holds it; `0xFF` is erased.
    bank: Vec<u8>,
    /// Per-chunk arrival flags for the open session.
    received: Vec<bool>,
    chunk: usize,
    len: usize,
    declared_crc: u32,
    /// While a session is open: when the erase finishes and data may be taken.
    erase_until: Option<Instant>,
    open: bool,
    high_water: u32,
    received_bytes: u32,
    err: Option<u8>,
}

impl BootSim {
    fn new(base: u32) -> Self {
        Self {
            base,
            bank: vec![0xFF; layout::app_bank_bytes(base)],
            received: Vec::new(),
            chunk: 0,
            len: 0,
            declared_crc: 0,
            erase_until: None,
            open: false,
            high_water: 0,
            received_bytes: 0,
            err: None,
        }
    }

    fn state(&self, now: Instant) -> BlState {
        match (self.open, self.erase_until) {
            (true, Some(until)) if now < until => BlState::Erasing,
            (true, _) => BlState::Receiving,
            // Nothing to run means resident indefinitely; a blank bank is exactly that.
            (false, _) if self.bank[..4] == [0xFF; 4] => BlState::Held,
            (false, _) => BlState::Idle,
        }
    }

    fn accepting(&self, now: Instant) -> bool {
        self.state(now) == BlState::Receiving
    }

    /// Whether every session chunk covering `start..end` has arrived. Used to answer
    /// `map` at a granularity the host chose, which need not be the session's.
    fn have_range(&self, start: usize, end: usize) -> bool {
        if start >= end || self.chunk == 0 {
            return false;
        }
        (start / self.chunk..=(end - 1) / self.chunk)
            .all(|chunk| self.received.get(chunk).copied().unwrap_or(false))
    }
}

struct SimPortal {
    id: u8,
    position: [f32; 2], // steps, float for motion integration
    target: [i32; 2],
    debug_lights: bool,
    calibrated: bool,
    boot_time: Instant,
    /// `None` while the application is running.
    boot: Option<BootSim>,
}

impl SimPortal {
    fn new(id: u8) -> Self {
        Self {
            id,
            position: [0.0; 2],
            target: [0; 2],
            debug_lights: true,
            calibrated: false,
            boot_time: Instant::now(),
            boot: None,
        }
    }

    fn integrate(&mut self, dt: f32, speed: f32) {
        for axis in 0..2 {
            let target = self.target[axis] as f32;
            let delta = target - self.position[axis];
            let step = speed * dt;
            if delta.abs() <= step {
                self.position[axis] = target;
            } else {
                self.position[axis] += step * delta.signum();
            }
        }
    }

    fn positions_body(&self) -> Value {
        Value::Map(vec![(
            key("p"),
            Value::Array(vec![
                Value::from(self.position[0] as i32),
                Value::from(self.position[1] as i32),
                Value::from(self.target[0]),
                Value::from(self.target[1]),
            ]),
        )])
    }

    fn status_body(&self, version: &str, noisy: bool) -> Value {
        let mc = |position: f32, target: i32| {
            Value::Map(vec![
                (key("position"), Value::from(position as i32)),
                (key("targetPosition"), Value::from(target)),
                (
                    key("healthStatus"),
                    Value::Map(vec![
                        (key("measureCycleOK"), Value::Boolean(self.calibrated)),
                        (key("SwitchesOK"), Value::Boolean(self.calibrated)),
                        (key("backlashOK"), Value::Boolean(self.calibrated)),
                        (key("homeOK"), Value::Boolean(self.calibrated)),
                    ]),
                ),
            ])
        };
        let logs = if noisy {
            Value::Array(vec![Value::Map(vec![
                (key("message"), Value::String("Switch not seen".into())),
                (key("level"), Value::from(20u8)),
                (
                    key("timestamp"),
                    Value::from(self.boot_time.elapsed().as_millis() as u64),
                ),
            ])])
        } else {
            Value::Array(vec![])
        };
        Value::Map(vec![
            (
                key("app"),
                Value::Map(vec![
                    (
                        key("upTime"),
                        Value::from(self.boot_time.elapsed().as_millis() as u64),
                    ),
                    (key("version"), Value::String(version.into())),
                ]),
            ),
            (key("mca"), mc(self.position[0], self.target[0])),
            (key("mcb"), mc(self.position[1], self.target[1])),
            (key("logger"), logs),
        ])
    }
}

/// A read-only look at one simulated board's bootloader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimBootloaderView {
    pub base: u32,
    pub session_open: bool,
    pub session_len: u32,
    /// Bytes accepted, which exceeds the image on a path that repeats every frame.
    pub received_bytes: u32,
    /// Highest byte offset written.
    pub high_water: u32,
    /// What `begin` declared.
    pub declared_crc32: u32,
    /// CRC-32C over what is actually programmed, which is what `verify` reports.
    pub bank_crc32: u32,
}

/// A queued outgoing reply with a due time.
struct PendingReply {
    due: Instant,
    bytes: Vec<u8>,
}

/// One portal's answer to one frame.
struct SimReply {
    body: Value,
    /// Answer even though the request was broadcast: a selector named this board,
    /// so exactly one unit is speaking and there is nothing to collide with.
    force: bool,
    /// Work the board does before it can answer -- the bank erase, and nothing else.
    delay: Duration,
}

impl SimReply {
    fn now(body: Value) -> Self {
        Self {
            body,
            force: false,
            delay: Duration::ZERO,
        }
    }
}

/// The provisioning serial a simulated board reports. Never 0, which the firmware
/// reads as "no identity".
fn sim_serial(id: u8) -> u32 {
    73_000 + u32::from(id)
}

/// A stable 96-bit id, distinct per board.
fn sim_uid(id: u8) -> [u8; 12] {
    let mut uid = [0xA5u8; 12];
    uid[0] = id;
    uid[11] = id.wrapping_mul(7);
    uid
}

fn field<'a>(fields: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .find(|(k, _)| k.as_str() == Some(name))
        .map(|(_, v)| v)
}

fn u32_field(fields: &[(Value, Value)], name: &str) -> Option<u32> {
    field(fields, name)?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
}

pub struct SimBus {
    config: SimConfig,
    portals: Vec<SimPortal>,
    acc: FrameAccumulator,
    pending: VecDeque<PendingReply>,
    connected: bool,
    last_integrate: Instant,
    rng: u32,
}

impl SimBus {
    pub fn new(config: SimConfig) -> Self {
        let portals = (1..=config.portal_count).map(SimPortal::new).collect();
        Self {
            config,
            portals,
            acc: FrameAccumulator::new(),
            pending: VecDeque::new(),
            connected: true,
            last_integrate: Instant::now(),
            rng: 0x9E3779B9,
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.rng >> 8) as f32 / 16_777_216.0
    }

    /// Queue a reply. `seq` echoes the request's trailer when it had one, which
    /// is what lets a host correlate an answer with the request it answers --
    /// the firmware replies in kind, and so does this.
    fn queue_reply(&mut self, source: u8, body: Value, seq: Option<u8>, delay: Duration) {
        if self.rand() < self.config.drop_rate {
            return;
        }
        let jitter_ms = self.config.jitter.as_secs_f32() * 1000.0 * self.rand();
        let due = Instant::now()
            + self.config.latency
            + delay
            + Duration::from_secs_f32(jitter_ms / 1000.0);
        let msgpack = match seq {
            Some(seq) => encode_reply_trailer(source as i8, &body, seq),
            None => encode_reply_fix8(source as i8, &body),
        };
        let mut bytes = encode_frame(&msgpack);
        if self.rand() < self.config.corrupt_rate {
            // flip a byte to a zero mid-frame: a classic COBS corruption
            let mid = bytes.len() / 2;
            bytes[mid] = 0;
        }
        self.pending.push_back(PendingReply { due, bytes });
    }

    /// Process one decoded command envelope, exactly one portal or broadcast.
    fn handle(&mut self, target: i8, body: &Value, seq: Option<u8>) {
        let ids: Vec<u8> = if target == -1 {
            self.portals.iter().map(|p| p.id).collect()
        } else if target > 0 {
            vec![target as u8]
        } else {
            return;
        };
        let broadcast = target == -1;

        for id in ids {
            if self.config.dead_portals.contains(&id) {
                continue;
            }
            let Some(index) = self.portals.iter().position(|p| p.id == id) else {
                continue;
            };

            let reply = self.handle_for_portal(index, body, broadcast);
            if let Some(reply) = reply {
                if !broadcast || self.config.answer_broadcast || reply.force {
                    self.queue_reply(id, reply.body, seq, reply.delay);
                }
            }
        }
    }

    fn handle_for_portal(
        &mut self,
        index: usize,
        body: &Value,
        broadcast: bool,
    ) -> Option<SimReply> {
        // Magic words are bare strings and are never answered by anything.
        if let Some(word) = body.as_str() {
            self.magic_word(index, word);
            return None;
        }
        if self.portals[index].boot.is_some() {
            return self.handle_bootloader(index, body, broadcast);
        }
        self.handle_application(index, body, broadcast)
    }

    /// `"FW!KC79"` reboots a running application into its bootloader; the rest are
    /// words only a bootloader acts on.
    fn magic_word(&mut self, index: usize, word: &str) {
        let base = self.config.app_base;
        let portal = &mut self.portals[index];
        match word {
            "FW!KC79" => {
                if portal.boot.is_none() {
                    portal.boot = Some(BootSim::new(base));
                }
            }
            // A keepalive, and nothing else: it deliberately no longer resets
            // progress, because a host sends it *during* a session to hold the
            // legacy half of a mixed fleet resident.
            "FW" => {}
            "ER" => {
                // The legacy blind path. A host old enough to send this word is
                // sending a legacy-base image, so the session it opens is at the
                // legacy base whatever this bootloader's own is.
                if let Some(boot) = &mut portal.boot {
                    *boot = BootSim::new(layout::APP_BASE_LEGACY);
                    boot.open = true;
                    boot.len = boot.bank.len();
                    boot.chunk = layout::BL_CHUNK_MAX;
                    boot.received = vec![false; boot.len.div_ceil(boot.chunk)];
                }
            }
            "RU" => portal.boot = None,
            _ => {}
        }
    }

    fn handle_application(
        &mut self,
        index: usize,
        body: &Value,
        broadcast: bool,
    ) -> Option<SimReply> {
        let version = self.config.firmware_version.clone();
        let noisy = self.config.noisy_portals.contains(&self.portals[index].id);
        let portal = &mut self.portals[index];
        let reply = match body {
            Value::Nil => Some(Value::Boolean(true)), // ping -> ACK
            Value::Map(entries) => {
                let mut reply = None;
                for (k, v) in entries {
                    match k.as_str() {
                        Some("poll") => reply = Some(portal.status_body(&version, noisy)),
                        Some("p") => reply = Some(portal.positions_body()),
                        Some("m") => {
                            if let Value::Array(values) = v {
                                for (axis, value) in values.iter().take(2).enumerate() {
                                    if let Some(steps) = value.as_i64() {
                                        portal.target[axis] = steps as i32;
                                    }
                                }
                            } else if let Some(steps) = v.as_i64() {
                                portal.target[0] = steps as i32;
                            }
                            reply = Some(portal.positions_body());
                        }
                        Some("keyframe") => {
                            if !broadcast {
                                reply = Some(Value::Boolean(true));
                            }
                            // handled at bus level (block addressing) below
                        }
                        Some("home") | Some("init") | Some("calibrate") => {
                            portal.position = [0.0; 2];
                            portal.target = [0; 2];
                            portal.calibrated = true;
                            reply = Some(Value::Boolean(true)); // early ACK
                        }
                        Some("reset") => {
                            *portal = SimPortal::new(portal.id);
                            // no reply: the unit reboots
                        }
                        Some("debugLightsEnabled") => {
                            portal.debug_lights = v.as_bool().unwrap_or(true);
                            reply = Some(Value::Boolean(true));
                        }
                        Some(_) => {
                            // motionControlX / motorDriverX / settings / unjam...
                            reply = Some(Value::Boolean(true));
                        }
                        None => {}
                    }
                }
                reply
            }
            _ => Some(Value::Boolean(true)),
        };
        reply.map(SimReply::now)
    }

    /// Every `bl` verb, plus the data frames a session takes.
    ///
    /// The half-duplex rule is what shapes this: a unicast request is answered by
    /// its target, a broadcast carrying a selector is answered by the one board
    /// the selector names, and an unselected broadcast is *acted on* by every
    /// board and answered by none -- which is what makes one `begin` open a
    /// session on a whole column.
    fn handle_bootloader(
        &mut self,
        index: usize,
        body: &Value,
        broadcast: bool,
    ) -> Option<SimReply> {
        let Value::Map(entries) = body else {
            return None;
        };
        // A data frame is a one-entry map keyed by a byte offset rather than a name.
        if let Some((k, v)) = entries.first() {
            if k.as_str().is_none() {
                if let (Some(offset), Value::Binary(payload)) = (k.as_u64(), v) {
                    self.take_frame(index, offset as usize, payload);
                }
                return None;
            }
        }
        let (_, inner) = entries
            .iter()
            .find(|(k, _)| k.as_str() == Some(bootloader::KEY))?;
        let Value::Map(fields) = inner else {
            return None;
        };
        let verb = BlVerb::from_str(field(fields, "q")?.as_str()?)?;

        let id = self.portals[index].id;
        let selected = match (u32_field(fields, "s"), field(fields, "uid")) {
            (Some(serial), _) => Some(serial == sim_serial(id)),
            (_, Some(Value::Binary(uid))) => Some(uid.as_slice() == sim_uid(id)),
            _ => None,
        };
        if broadcast {
            match selected {
                // Another board was named: this one is not part of the exchange at
                // all, so it neither acts nor answers.
                Some(false) => return None,
                // No selector: `adopt` is the one verb that must never be taken
                // this way, because every board would take the same id.
                None if verb == BlVerb::Adopt => return None,
                _ => {}
            }
        }
        // A v4/v5 bootloader accepts data frames and never transmits. Modelling it
        // as "answers nothing" is the whole of the difference that matters to a host.
        if self.config.legacy_portals.contains(&id) {
            return None;
        }

        let now = Instant::now();
        let erase_time = self.config.erase_time;
        let body = self.bootloader_verb(index, verb, fields, now, erase_time);
        let answered = broadcast && selected == Some(true);
        body.map(|(body, delay)| SimReply {
            body,
            force: answered,
            delay,
        })
    }

    fn bootloader_verb(
        &mut self,
        index: usize,
        verb: BlVerb,
        fields: &[(Value, Value)],
        now: Instant,
        erase_time: Duration,
    ) -> Option<(Value, Duration)> {
        let id = self.portals[index].id;
        let app_version = self.config.firmware_version.clone();
        let boot = self.portals[index].boot.as_mut()?;
        // Every reply carries the verb it answers, inside the `bl` key that marks a
        // control-plane frame -- an unwrapped body would be read as ordinary traffic.
        let reply = |entries: Vec<(Value, Value)>| {
            let mut all = vec![(key("q"), Value::from(verb.as_str()))];
            all.extend(entries);
            map(vec![(key(bootloader::KEY), map(all))])
        };

        match verb {
            BlVerb::Status => {
                let state = boot.state(now);
                let mut entries = vec![
                    (key("v"), Value::from(layout::BL_PROTO_VERSION)),
                    (key("id"), Value::from(id)),
                    (key("src"), Value::from("handoff")),
                    (key("s"), Value::from(sim_serial(id))),
                    (key("uid"), Value::Binary(sim_uid(id).to_vec())),
                    (key("base"), Value::from(boot.base)),
                    (key("cap"), Value::from(boot.bank.len() as u32)),
                    (key("chunk"), Value::from(layout::BL_CHUNK_MAX as u32)),
                    (key("st"), Value::from(state.code())),
                    (key("wp"), Value::from(boot.high_water)),
                    (key("n"), Value::from(boot.received_bytes)),
                ];
                if let Some(code) = boot.err {
                    entries.push((key("err"), Value::from(code)));
                }
                // A bank that is still erased has no application to describe.
                if boot.bank[..4] != [0xFF; 4] {
                    entries.push((
                        key("app"),
                        map(vec![
                            (key("base"), Value::from(boot.base)),
                            (key("ver"), Value::from(app_version)),
                        ]),
                    ));
                }
                Some((reply(entries), Duration::ZERO))
            }
            BlVerb::Begin => {
                let len = u32_field(fields, "len").unwrap_or(0) as usize;
                let chunk = u32_field(fields, "chunk").unwrap_or(0) as usize;
                let base = u32_field(fields, "base").unwrap_or(boot.base);
                let bad = if !layout::is_app_base(base) {
                    Some(err::BAD_PARAM)
                } else if !len.is_multiple_of(layout::FLASH_GRANULE)
                    || len == 0
                    || len > layout::app_bank_bytes(base)
                {
                    Some(err::BOUNDS)
                } else if chunk == 0
                    || chunk > layout::BL_CHUNK_MAX
                    || !chunk.is_multiple_of(layout::FLASH_GRANULE)
                {
                    Some(err::BAD_PARAM)
                } else {
                    None
                };
                if let Some(code) = bad {
                    boot.err = Some(code);
                    return Some((
                        reply(vec![
                            (key("ok"), Value::Boolean(false)),
                            (key("err"), Value::from(code)),
                        ]),
                        Duration::ZERO,
                    ));
                }
                *boot = BootSim::new(base);
                boot.open = true;
                boot.len = len;
                boot.chunk = chunk;
                boot.declared_crc = u32_field(fields, "crc").unwrap_or(0);
                boot.received = vec![false; len.div_ceil(chunk)];
                boot.erase_until = Some(now + erase_time);
                // The reply waits out the erase, because that is when the board is
                // ready for the first data frame.
                Some((reply(vec![(key("ok"), Value::Boolean(true))]), erase_time))
            }
            BlVerb::Map => {
                let reported = u32_field(fields, "chunk")
                    .map(|chunk| chunk as usize)
                    .filter(|chunk| *chunk > 0)
                    .unwrap_or(boot.chunk.max(1));
                let count = boot.len.div_ceil(reported.max(1));
                let mut bitmap = vec![0u8; count.div_ceil(8)];
                for index in 0..count {
                    if boot.have_range(index * reported, ((index + 1) * reported).min(boot.len)) {
                        bitmap[index / 8] |= 1 << (index % 8);
                    }
                }
                Some((
                    reply(vec![
                        (key("chunk"), Value::from(reported as u32)),
                        (key("len"), Value::from(boot.len as u32)),
                        (key("map"), Value::Binary(bitmap)),
                    ]),
                    Duration::ZERO,
                ))
            }
            BlVerb::Verify => {
                let crc = crc32c(&boot.bank[..boot.len.min(boot.bank.len())]);
                Some((
                    reply(vec![
                        (key("ok"), Value::Boolean(crc == boot.declared_crc)),
                        (key("crc"), Value::from(crc)),
                        (key("len"), Value::from(boot.len as u32)),
                    ]),
                    Duration::ZERO,
                ))
            }
            BlVerb::Run => {
                let base = boot.base;
                let crc = crc32c(&boot.bank[..boot.len.min(boot.bank.len())]);
                let bad = if boot.open && crc != boot.declared_crc {
                    Some(err::IMAGE_CRC)
                } else if boot.bank[..4] == [0xFF; 4] {
                    Some(err::NO_APP)
                } else {
                    None
                };
                let body = reply(vec![
                    (key("ok"), Value::Boolean(bad.is_none())),
                    (key("err"), Value::from(bad.unwrap_or(err::NONE))),
                    (key("base"), Value::from(base)),
                ]);
                if bad.is_none() {
                    self.portals[index].boot = None;
                }
                Some((body, Duration::ZERO))
            }
            BlVerb::Adopt => {
                let new_id = field(fields, "id")
                    .and_then(|v| v.as_i64())
                    .and_then(|v| u8::try_from(v).ok());
                if let Some(new_id) = new_id {
                    self.portals[index].id = new_id;
                }
                Some((
                    reply(vec![(key("id"), Value::from(self.portals[index].id))]),
                    Duration::ZERO,
                ))
            }
            BlVerb::Reset => {
                let base = boot.base;
                *boot = BootSim::new(base);
                Some((
                    reply(vec![(key("ok"), Value::Boolean(true))]),
                    Duration::ZERO,
                ))
            }
        }
    }

    /// Take one `{offset: bin(xor16 ++ data)}` frame into the bank.
    ///
    /// Frames arrive in any order and duplicates are free -- the property that lets
    /// a host repair exactly the gaps a bitmap named rather than resending an image.
    fn take_frame(&mut self, index: usize, offset: usize, payload: &[u8]) {
        let Some(boot) = self.portals[index].boot.as_mut() else {
            return;
        };
        if !boot.accepting(Instant::now()) || payload.len() < 2 {
            return;
        }
        let data = &payload[2..];
        let declared = u16::from_le_bytes([payload[0], payload[1]]);
        if declared != router_proto::fw::checksum_xor16(data) {
            boot.err = Some(err::XOR);
            return;
        }
        if !offset.is_multiple_of(layout::FLASH_GRANULE) {
            boot.err = Some(err::ALIGN);
            return;
        }
        let end = offset + data.len();
        if end > boot.len || end > boot.bank.len() {
            boot.err = Some(err::BOUNDS);
            return;
        }
        boot.bank[offset..end].copy_from_slice(data);
        boot.received_bytes += data.len() as u32;
        boot.high_water = boot.high_water.max(end as u32);
        for chunk in 0..boot.received.len() {
            let start = chunk * boot.chunk;
            let stop = ((chunk + 1) * boot.chunk).min(boot.len);
            if start >= offset && stop <= end && start < stop {
                boot.received[chunk] = true;
            }
        }
    }

    /// Keyframe blocks address portals by index across the whole column.
    fn handle_keyframe(&mut self, body: &Value) {
        let Value::Map(entries) = body else { return };
        let Some((_, kf)) = entries.iter().find(|(k, _)| k.as_str() == Some("keyframe")) else {
            return;
        };
        let Value::Map(kf) = kf else { return };
        let start_index = kf
            .iter()
            .find(|(k, _)| k.as_str() == Some("startIndex"))
            .and_then(|(_, v)| v.as_u64())
            .unwrap_or(1) as usize;
        let Some(Value::Array(values)) = kf
            .iter()
            .find(|(k, _)| k.as_str() == Some("values"))
            .map(|(_, v)| v)
        else {
            return;
        };
        for (offset, value) in values.iter().enumerate() {
            // startIndex is 1-based: portal ID = startIndex + offset
            let id = (start_index + offset) as u8;
            if self.config.dead_portals.contains(&id) {
                continue;
            }
            let Some(portal) = self.portals.iter_mut().find(|p| p.id == id) else {
                continue;
            };
            if let Value::Array(fields) = value {
                for (axis, field) in fields.iter().take(2).enumerate() {
                    if let Some(steps) = field.as_i64() {
                        portal.target[axis] = steps as i32;
                    }
                }
            }
        }
    }

    /// What a simulated board's bootloader is holding.
    ///
    /// `None` while the board is running its application, which is itself the
    /// answer to "did the recall work" and "did `run` hand back over".
    pub fn portal_bootloader(&self, id: u8) -> Option<SimBootloaderView> {
        let boot = self.portals.iter().find(|p| p.id == id)?.boot.as_ref()?;
        Some(SimBootloaderView {
            base: boot.base,
            session_open: boot.open,
            session_len: boot.len as u32,
            received_bytes: boot.received_bytes,
            high_water: boot.high_water,
            declared_crc32: boot.declared_crc,
            bank_crc32: crc32c(&boot.bank[..boot.len.min(boot.bank.len())]),
        })
    }

    /// A copy of a simulated board's application bank, `0xFF` where erased.
    ///
    /// The only way to check what the *blind* path actually delivered: it has no
    /// verify verb, so the bytes themselves are the only evidence there is.
    pub fn portal_bank(&self, id: u8) -> Option<Vec<u8>> {
        Some(
            self.portals
                .iter()
                .find(|p| p.id == id)?
                .boot
                .as_ref()?
                .bank
                .clone(),
        )
    }

    pub fn portal_positions(&self, id: u8) -> Option<([i32; 2], [i32; 2])> {
        self.portals
            .iter()
            .find(|p| p.id == id)
            .map(|p| ([p.position[0] as i32, p.position[1] as i32], p.target))
    }
}

impl SerialDevice for SimBus {
    fn type_name(&self) -> &'static str {
        "Sim"
    }

    fn address_string(&self) -> String {
        format!("sim:{} portals", self.config.portal_count)
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn transmit(&mut self, data: &[u8]) -> std::io::Result<()> {
        if !self.connected {
            return Err(std::io::ErrorKind::NotConnected.into());
        }
        let results = self.acc.push(data);
        for result in results {
            let Ok(payload) = result else { continue };
            let Ok(envelope) = decode_envelope(&payload) else {
                continue;
            };
            // A frame whose trailer failed the CRC is discarded, as the firmware
            // discards it: it decoded to a plausible body, and acting on it is how
            // a corrupted offset writes a chunk into the wrong place.
            if !envelope.trailer.acceptable() {
                continue;
            }
            // keyframes address portals by block index
            if matches!(&envelope.body, Value::Map(entries)
                if entries.iter().any(|(k, _)| k.as_str() == Some("keyframe")))
            {
                self.handle_keyframe(&envelope.body);
                continue;
            }
            self.handle(envelope.target, &envelope.body, envelope.trailer.seq());
        }
        Ok(())
    }

    fn receive_available(&mut self) -> std::io::Result<Vec<u8>> {
        if !self.connected {
            return Err(std::io::ErrorKind::NotConnected.into());
        }
        // integrate motion
        let dt = self.last_integrate.elapsed().as_secs_f32();
        self.last_integrate = Instant::now();
        let speed = self.config.motor_speed;
        for portal in &mut self.portals {
            portal.integrate(dt, speed);
        }
        // release due replies
        let now = Instant::now();
        let mut out = Vec::new();
        while let Some(front) = self.pending.front() {
            if front.due <= now {
                out.extend_from_slice(&self.pending.pop_front().unwrap().bytes);
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn close(&mut self) {
        self.connected = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use crate::fw_session::{
        BoardKind, BoardState, FwBus, FwProgress, FwSession, FwSessionParams, Targets,
    };
    use crate::fw_update::{self, FwUpdateParams};
    use crate::rs485::{Packet, Payload};
    use router_proto::Envelope;

    /// The simulator wired straight to a session, with no worker thread between them.
    ///
    /// The worker's job on this path is framing and pacing; neither changes what either
    /// side decides, so leaving it out buys a test that runs in milliseconds instead of
    /// the two minutes a real upload takes.
    struct Loopback {
        sim: RefCell<SimBus>,
        accumulator: RefCell<FrameAccumulator>,
    }

    impl Loopback {
        fn new(config: SimConfig) -> Self {
            Self {
                sim: RefCell::new(SimBus::new(config)),
                accumulator: RefCell::new(FrameAccumulator::new()),
            }
        }

        fn drain(&self) -> Vec<Envelope> {
            let bytes = self
                .sim
                .borrow_mut()
                .receive_available()
                .unwrap_or_default();
            self.accumulator
                .borrow_mut()
                .push(&bytes)
                .into_iter()
                .filter_map(|result| decode_envelope(&result.ok()?).ok())
                .collect()
        }
    }

    impl FwBus for Loopback {
        fn transmit(&self, packet: Packet) {
            let bytes = match packet.payload {
                Payload::Rendered(bytes) => bytes,
                Payload::Lazy(render) => render(),
            };
            let _ = self.sim.borrow_mut().transmit(&encode_frame(&bytes));
            if let Some(on_sent) = packet.on_sent {
                on_sent();
            }
        }

        fn outbox_len(&self) -> usize {
            0
        }

        fn clear_outbox(&self) {}
    }

    /// An image that states its own base, the way a v6-era build does.
    fn image_for(base: u32, bytes: usize) -> Vec<u8> {
        let mut image = vec![0x5Au8; bytes.max(0x400)];
        image[..4].copy_from_slice(&layout::RAM_END.to_le_bytes());
        image[4..8].copy_from_slice(&((base + 0x241) | 1).to_le_bytes());
        let at = layout::APP_DESCRIPTOR_OFFSET;
        image[at..at + 8].copy_from_slice(layout::APP_DESCRIPTOR_MAGIC);
        image[at + 8..at + 12].copy_from_slice(&base.to_le_bytes());
        image.truncate(bytes);
        image
    }

    fn instant_config(portal_count: u8) -> SimConfig {
        SimConfig {
            portal_count,
            latency: Duration::ZERO,
            jitter: Duration::ZERO,
            // The erase's real duration is what `begin_timeout_ms` is sized for; the
            // sequencing this test is about is the same whether it takes 1.2 s or none.
            erase_time: Duration::ZERO,
            ..Default::default()
        }
    }

    fn drive(bus: &Loopback, session: &mut FwSession) -> FwProgress {
        let mut now = Instant::now();
        let mut progress = session.tick(bus, now, &[]);
        for _ in 0..20_000 {
            if progress.done {
                break;
            }
            let envelopes = bus.drain();
            now += Duration::from_millis(5);
            progress = session.tick(bus, now, &envelopes);
        }
        progress
    }

    #[test]
    fn a_simulated_v6_fleet_takes_a_whole_session_and_ends_up_running_the_image() {
        let firmware = image_for(layout::APP_BASE, 2_048);
        let bus = Loopback::new(instant_config(3));
        let mut session = FwSession::new(
            &firmware,
            FwSessionParams {
                targets: Targets::Ids(vec![1, 2, 3]),
                ..Default::default()
            },
        )
        .unwrap();

        let progress = drive(&bus, &mut session);
        assert!(progress.done && progress.ok, "{}", progress.detail);
        for board in &progress.boards {
            assert_eq!(board.kind, BoardKind::V6);
            assert_eq!(board.state, BoardState::Running, "board {}", board.id);
            assert_eq!(board.base, layout::APP_BASE);
        }
        // `run` handed control back, which is what leaving the bootloader state means.
        for id in 1..=3 {
            assert_eq!(bus.sim.borrow().portal_bootloader(id), None);
        }
    }

    /// The image the boards ended up holding is the image that was sent, byte for byte --
    /// checked by the same CRC the bootloader verifies with, not by counting frames.
    #[test]
    fn what_a_simulated_board_programs_matches_what_the_host_declared() {
        let firmware = image_for(layout::APP_BASE, 1_536);
        let bus = Loopback::new(instant_config(1));
        let mut session = FwSession::new(
            &firmware,
            FwSessionParams {
                targets: Targets::Ids(vec![1]),
                // Leave the board in its bootloader so the bank can still be read.
                run_after: false,
                ..Default::default()
            },
        )
        .unwrap();

        let progress = drive(&bus, &mut session);
        assert!(progress.done && progress.ok, "{}", progress.detail);

        let view = bus
            .sim
            .borrow()
            .portal_bootloader(1)
            .expect("in bootloader");
        assert!(view.session_open);
        assert_eq!(view.session_len as usize, session.image_len());
        assert_eq!(view.received_bytes as usize, session.image_len());
        assert_eq!(view.bank_crc32, session.image_crc32());
        assert_eq!(view.declared_crc32, session.image_crc32());
    }

    /// One board on the fielded bootloader drags the whole fleet onto the blind path,
    /// and the blind path still delivers the image -- to both of them.
    #[test]
    fn a_mixed_simulated_fleet_falls_back_to_the_blind_path() {
        let firmware = image_for(layout::APP_BASE_LEGACY, 1_024);
        let bus = Loopback::new(SimConfig {
            legacy_portals: vec![2],
            app_base: layout::APP_BASE_LEGACY,
            ..instant_config(2)
        });
        let mut session = FwSession::new(
            &firmware,
            FwSessionParams {
                targets: Targets::Ids(vec![1, 2]),
                // Long enough that the blind path's own frames are what fills the gaps,
                // rather than a status timeout racing them.
                status_timeout_ms: 200,
                run_after: false,
                ..Default::default()
            },
        )
        .unwrap();

        let progress = drive(&bus, &mut session);
        assert!(progress.done && progress.ok, "{}", progress.detail);
        assert_eq!(progress.boards[0].kind, BoardKind::V6);
        assert_eq!(progress.boards[1].kind, BoardKind::Legacy);
        for board in &progress.boards {
            assert_eq!(board.state, BoardState::LegacyBlind);
        }

        // Both boards took the image, including the one that never said so. Checked by
        // the bytes rather than by a count: the blind path repeats every frame, so the
        // number of bytes accepted is a multiple of the image and proves nothing.
        let want = fw_update::prepare_image(&firmware, &FwUpdateParams::resilient());
        for id in [1u8, 2u8] {
            let view = bus
                .sim
                .borrow()
                .portal_bootloader(id)
                .expect("in bootloader");
            assert_eq!(view.base, layout::APP_BASE_LEGACY, "board {id}");
            assert_eq!(
                view.high_water as usize,
                want.len(),
                "board {id} did not receive the whole image"
            );
            let bank = bus.sim.borrow().portal_bank(id).expect("a bank");
            assert_eq!(
                &bank[..want.len()],
                &want[..],
                "board {id} holds other bytes"
            );
        }
    }

    /// A selector is what makes one board out of many answer a broadcast -- the escape
    /// hatch for a board whose RS485 id is unknown.
    #[test]
    fn a_serial_selector_is_answered_by_exactly_one_board() {
        let firmware = image_for(layout::APP_BASE, 512);
        let bus = Loopback::new(instant_config(4));
        let mut session = FwSession::new(
            &firmware,
            FwSessionParams {
                targets: Targets::Serials(vec![sim_serial(3)]),
                run_after: false,
                ..Default::default()
            },
        )
        .unwrap();

        let progress = drive(&bus, &mut session);
        assert!(progress.done && progress.ok, "{}", progress.detail);
        assert_eq!(progress.boards.len(), 1);
        assert_eq!(progress.boards[0].id, 3, "the id came from the reply");
        assert_eq!(progress.boards[0].serial, Some(sim_serial(3)));
        // The other three heard the same broadcast and said nothing, so they never
        // opened a session.
        for id in [1u8, 2u8, 4u8] {
            let view = bus.sim.borrow().portal_bootloader(id).expect("recalled");
            assert!(
                !view.session_open,
                "board {id} answered a selector for board 3"
            );
        }
    }
}

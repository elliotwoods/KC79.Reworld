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

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use router_proto::envelope::encode_reply_fix8;
use router_proto::value::key;
use router_proto::{decode_envelope, encode_frame, FrameAccumulator, Value};

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
        }
    }
}

struct SimPortal {
    id: u8,
    position: [f32; 2], // steps, float for motion integration
    target: [i32; 2],
    debug_lights: bool,
    calibrated: bool,
    boot_time: Instant,
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

/// A queued outgoing reply with a due time.
struct PendingReply {
    due: Instant,
    bytes: Vec<u8>,
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

    fn queue_reply(&mut self, source: u8, body: Value) {
        if self.rand() < self.config.drop_rate {
            return;
        }
        let jitter_ms = self.config.jitter.as_secs_f32() * 1000.0 * self.rand();
        let due = Instant::now() + self.config.latency + Duration::from_secs_f32(jitter_ms / 1000.0);
        let mut bytes = encode_frame(&encode_reply_fix8(source as i8, &body));
        if self.rand() < self.config.corrupt_rate {
            // flip a byte to a zero mid-frame: a classic COBS corruption
            let mid = bytes.len() / 2;
            bytes[mid] = 0;
        }
        self.pending.push_back(PendingReply { due, bytes });
    }

    /// Process one decoded command envelope, exactly one portal or broadcast.
    fn handle(&mut self, target: i8, body: &Value) {
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
                if !broadcast {
                    self.queue_reply(id, reply);
                }
            }
        }
    }

    fn handle_for_portal(&mut self, index: usize, body: &Value, broadcast: bool) -> Option<Value> {
        let version = self.config.firmware_version.clone();
        let noisy = self.config.noisy_portals.contains(&self.portals[index].id);
        let portal = &mut self.portals[index];
        match body {
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
            let Ok(envelope) = decode_envelope(&payload) else { continue };
            // keyframes address portals by block index
            if matches!(&envelope.body, Value::Map(entries)
                if entries.iter().any(|(k, _)| k.as_str() == Some("keyframe")))
            {
                self.handle_keyframe(&envelope.body);
                continue;
            }
            self.handle(envelope.target, &envelope.body);
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

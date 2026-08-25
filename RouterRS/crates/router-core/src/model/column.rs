//! A Column: one RS485 bus plus a `countX x countY` grid of Portals with
//! target IDs 1..=N. Port of `Router/src/Modules/Hardware/Column.*`.

use std::time::Instant;

use glam::{vec2, Vec2};
use router_proto::commands::{self, ActionKind, KeyframeValue};
use router_proto::replies::LogMessage;
use router_proto::{Envelope, Value};
use router_report::{Event, Reporter};

use crate::config::ColumnConfig;
use crate::image::PixelsF32;
use crate::rs485::{Packet, Rs485};

use super::portal::Portal;

pub struct Column {
    pub index: usize,
    pub count_x: usize,
    pub count_y: usize,
    /// Default true. When NOT flipped, image sampling runs bottom-to-top.
    pub flipped: bool,
    pub portals: Vec<Portal>,
    pub rs485: Rs485,
    // scheduled poll parameters
    pub scheduled_poll_enabled: bool,
    pub scheduled_poll_period_s: f32,
    last_poll_all: Option<Instant>,
    // keyframe velocity state
    last_keyframe_time: Instant,
    last_keyframe_axes: Vec<Vec2>,
    /// The RS485 repeaters on this bus, in V3. Empty in V1/V2, which have none.
    pub repeaters: crate::model::repeater::RepeaterPlane,
}

pub struct ColumnSettings {
    pub index: usize,
    pub count_x: usize,
    pub count_y: usize,
    pub flipped: bool,
}

impl Column {
    pub fn new(settings: ColumnSettings, reporter: Reporter) -> Self {
        let mut column = Self {
            index: settings.index,
            count_x: settings.count_x,
            count_y: settings.count_y,
            flipped: settings.flipped,
            portals: Vec::new(),
            rs485: Rs485::new(settings.index as u8, reporter),
            scheduled_poll_enabled: false,
            scheduled_poll_period_s: 60.0,
            last_poll_all: None,
            last_keyframe_time: Instant::now(),
            last_keyframe_axes: Vec::new(),
            repeaters: Default::default(),
        };
        column.rebuild_portals();
        column
    }

    /// Apply per-column config (already merged with columnCommonSettings).
    pub fn apply_config(&mut self, config: &ColumnConfig) {
        self.count_x = config.count_x;
        self.count_y = config.count_y;
        if let Some(flipped) = config.flipped {
            self.flipped = flipped;
        }
        self.rebuild_portals();
        if let Some(rs485_settings) = &config.rs485 {
            self.rs485.open_from_settings(rs485_settings.clone());
        }
    }

    /// Sequential target IDs 1..=countX*countY (row-major over countY rows).
    pub fn rebuild_portals(&mut self) {
        let count = self.count_x * self.count_y;
        self.portals = (1..=count as u8).map(Portal::new).collect();
        self.last_keyframe_axes.clear();
    }

    pub fn portal_by_target(&mut self, target: u8) -> Option<&mut Portal> {
        self.portals.iter_mut().find(|p| p.target == target)
    }

    /// Per-tick update: RS485 housekeeping + inbound routing + per-portal
    /// updates + scheduled polls. Returns fresh log messages for reporting.
    pub fn update(&mut self, reporter: &Reporter) {
        // inbound
        let envelopes = self.rs485.update();
        for envelope in envelopes {
            self.process_incoming(&envelope, reporter);
        }

        // per-portal updates (pilot sync + auto-push)
        let rs485_open = self.rs485.is_connected();
        for portal in &mut self.portals {
            let outgoing = portal.update();
            if rs485_open {
                for message in outgoing {
                    let target = portal.target as i8;
                    let full_address = format!("{}", message.address);
                    self.rs485
                        .transmit(Packet::from_body(target, &message.body, full_address));
                    portal.last_tx = Some(Instant::now());
                }
            }
        }

        // scheduled poll
        if self.scheduled_poll_enabled {
            let due = self
                .last_poll_all
                .map(|t| t.elapsed().as_secs_f32() >= self.scheduled_poll_period_s)
                .unwrap_or(true);
            if due {
                self.poll_all();
            }
        }
    }

    /// `Column::processIncoming`: frames addressed to the host are routed to
    /// the portal whose target ID equals the frame's source.
    pub fn process_incoming(&mut self, envelope: &Envelope, reporter: &Reporter) {
        if envelope.target != 0 {
            return;
        }
        if envelope.source <= 0 {
            // A repeater answers with a negative source. Everything else with a
            // non-positive source is a frame the host has no use for.
            if let Ok(Some(reply)) = router_proto::repeater::parse_reply(&envelope.body) {
                self.repeaters.observe(&reply);
            }
            return;
        }
        let col = self.index as u8;
        let Some(portal) = self.portal_by_target(envelope.source as u8) else {
            return;
        };
        let (fresh_logs, report) = portal.process_incoming(&envelope.body);

        // reporting hooks
        for LogMessage { message, level, timestamp_ms } in fresh_logs {
            if level >= router_report::events::LEVEL_WARNING || reporter.is_verbose() {
                reporter.emit(Event::PortalLog {
                    col,
                    portal: portal.target,
                    level,
                    message,
                    fw_ts_ms: timestamp_ms,
                    count: 1,
                });
            }
        }
        if let Some(report) = report {
            if report.app.is_some() || report.mca.is_some() || report.mcb.is_some() {
                let flags = |mc: &Option<router_proto::replies::MotionControlStatus>| {
                    mc.as_ref().map(|s| router_report::events::AxisHealthFlags {
                        measure_cycle_ok: s.health.measure_cycle_ok,
                        switches_ok: s.health.switches_ok,
                        backlash_ok: s.health.backlash_ok,
                        home_ok: s.health.home_ok,
                    })
                };
                reporter.emit(Event::PortalStatus {
                    col,
                    portal: portal.target,
                    uptime_ms: report.app.as_ref().and_then(|a| a.up_time_ms),
                    version: report.app.as_ref().and_then(|a| a.version.clone()),
                    mca: flags(&report.mca),
                    mcb: flags(&report.mcb),
                });
            }
        }
    }

    // ------------------------------------------------------------ sending

    /// Push only pilots whose targets changed (`Column::pushStale`).
    pub fn push_stale(&mut self) {
        let rs485_open = self.rs485.is_connected();
        for portal in &mut self.portals {
            if portal.pilot.needs_push(rs485_open) {
                let body = portal.pilot.move_message();
                self.rs485
                    .transmit(Packet::from_body(portal.target as i8, &body, "m"));
                portal.pilot.notify_values_sent();
                portal.last_tx = Some(Instant::now());
            }
        }
    }

    /// Broadcast a message to every portal on this bus.
    pub fn broadcast(&self, body: &Value, collateable: bool) {
        // address = first key of the body map (msgpack11 Packet behavior)
        let address = match body {
            Value::Map(entries) => entries
                .first()
                .and_then(|(k, _)| k.as_str())
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        };
        self.rs485.transmit(Packet::broadcast(body, address, collateable));
    }

    pub fn broadcast_action(&mut self, action: ActionKind) {
        self.broadcast(&action.body(), false);
        for portal in &mut self.portals {
            portal.apply_action_effect(action);
        }
    }

    pub fn poll_all(&mut self) {
        self.last_poll_all = Some(Instant::now());
        for portal in &mut self.portals {
            self.rs485
                .transmit(Packet::from_body(portal.target as i8, &commands::poll(), "poll"));
            portal.last_tx = Some(Instant::now());
        }
    }

    /// Send a single portal's message (unicast with collation address).
    pub fn send_to_portal(&mut self, target: u8, body: &Value, address: &str) {
        self.rs485
            .transmit(Packet::from_body(target as i8, body, address));
        if let Some(portal) = self.portal_by_target(target) {
            portal.last_tx = Some(Instant::now());
        }
    }

    // ------------------------------------------------------------- image

    /// `Column::updatePositionsFromImage`: sample pixel (columnIndex*countX+i,
    /// j or countY-1-j when NOT flipped) as the portal's position target.
    pub fn update_positions_from_image(&mut self, pixels: &PixelsF32) {
        for j in 0..self.count_y {
            for i in 0..self.count_x {
                let portal_index = j * self.count_x + i;
                let Some(portal) = self.portals.get_mut(portal_index) else {
                    continue;
                };
                let x = self.index * self.count_x + i;
                let y = if self.flipped { j } else { self.count_y - 1 - j };
                let Some(rgb) = pixels.get(x, y) else { continue };
                portal.pilot.set_position(vec2(rgb[0], rgb[1]));
                portal.pilot.update([
                    super::pilot::AxisReported {
                        current_steps: portal.motion_control[0].reported_position,
                        target_steps: portal.motion_control[0].reported_target,
                    },
                    super::pilot::AxisReported {
                        current_steps: portal.motion_control[1].reported_position,
                        target_steps: portal.motion_control[1].reported_target,
                    },
                ]);
            }
        }
    }

    /// `Column::transmitKeyframe`: broadcast all portal targets in blocks of
    /// `batch_size`, with optional velocities derived from the previous
    /// keyframe.
    pub fn transmit_keyframe(&mut self, batch_size: usize, velocities_enabled: bool) {
        if !self.rs485.is_connected() {
            return;
        }
        let axis_values: Vec<Vec2> = self
            .portals
            .iter_mut()
            .map(|portal| {
                let (a, b) = portal.pilot.axis_steps();
                portal.pilot.notify_values_sent();
                vec2(a as f32, b as f32)
            })
            .collect();

        // velocities from the previous keyframe
        let dt = self.last_keyframe_time.elapsed().as_secs_f32();
        self.last_keyframe_time = Instant::now();
        let velocities: Vec<Vec2> = if velocities_enabled {
            if self.last_keyframe_axes.len() == axis_values.len() && dt > 0.0 {
                axis_values
                    .iter()
                    .zip(&self.last_keyframe_axes)
                    .map(|(now, prev)| (*now - *prev) / dt)
                    .collect()
            } else {
                vec![Vec2::ZERO; axis_values.len()]
            }
        } else {
            Vec::new()
        };

        // clear pending keyframes, then send blocks
        self.rs485.remove_packets("keyframe", -1);

        let mut block: Vec<KeyframeValue> = Vec::with_capacity(batch_size);
        let mut block_start_index: u64 = 1;
        let flush = |block: &mut Vec<KeyframeValue>, start: &mut u64, rs485: &Rs485| {
            if block.is_empty() {
                return;
            }
            let body = commands::keyframe(*start, block);
            rs485.transmit(Packet::broadcast(&body, "keyframe", false));
            *start += block.len() as u64;
            block.clear();
        };

        for (i, axes) in axis_values.iter().enumerate() {
            if velocities_enabled {
                let vel = velocities[i];
                block.push(KeyframeValue::PosVel(
                    axes.x as i32,
                    axes.y as i32,
                    vel.x as i32,
                    vel.y as i32,
                ));
            } else {
                block.push(KeyframeValue::Pos(axes.x as i32, axes.y as i32));
            }
            if block.len() >= batch_size {
                flush(&mut block, &mut block_start_index, &self.rs485);
            }
        }
        flush(&mut block, &mut block_start_index, &self.rs485);

        self.last_keyframe_axes = axis_values;
    }
}

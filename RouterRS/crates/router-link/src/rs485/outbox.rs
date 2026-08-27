//! Outbound packet queue with collation, port of `RS485::Packet` and
//! `RS485::collateOutboxPackets`: when enabled, a newer collateable packet
//! with the same `(address, target)` replaces the older one in place, so
//! only the latest command per address/target survives bursts.

use router_proto::{encode_envelope, Value};

/// Rendered-at-send-time payload (`Packet::lazyMessageRenderer`).
pub type LazyRenderer = Box<dyn FnOnce() -> Vec<u8> + Send>;

pub enum Payload {
    Rendered(Vec<u8>),
    Lazy(LazyRenderer),
}

pub struct Packet {
    pub payload: Payload,
    /// RS485 target (-1 broadcast); used for ACK matching and collation.
    pub target: i8,
    /// Collation key (e.g. "m", "p", "poll", "keyframe"); empty = never collated.
    pub address: String,
    pub needs_ack: bool,
    pub collateable: bool,
    /// Overrides the default post-send wait (`customWaitTime_ms`); `Some(0)`
    /// = no wait at all.
    pub custom_wait_time_ms: Option<u32>,
    pub on_sent: Option<Box<dyn FnOnce() + Send>>,
}

impl Packet {
    /// Envelope from a msgpack11-style body (the standard TX path).
    pub fn from_body(target: i8, body: &Value, address: impl Into<String>) -> Self {
        Self {
            payload: Payload::Rendered(encode_envelope(target, body)),
            target,
            address: address.into(),
            needs_ack: true,
            collateable: true,
            custom_wait_time_ms: None,
            on_sent: None,
        }
    }

    /// Pre-encoded envelope bytes (fix8 header paths: FW update, ping).
    pub fn from_envelope_bytes(target: i8, bytes: Vec<u8>, address: impl Into<String>) -> Self {
        Self {
            payload: Payload::Rendered(bytes),
            target,
            address: address.into(),
            needs_ack: true,
            collateable: true,
            custom_wait_time_ms: None,
            on_sent: None,
        }
    }

    /// Lazily rendered envelope (`pushLazy`): the closure runs on the serial
    /// thread just before the write.
    pub fn lazy(target: i8, renderer: LazyRenderer, address: impl Into<String>) -> Self {
        Self {
            payload: Payload::Lazy(renderer),
            target,
            address: address.into(),
            needs_ack: true,
            collateable: true,
            custom_wait_time_ms: None,
            on_sent: None,
        }
    }

    pub fn broadcast(body: &Value, address: impl Into<String>, collateable: bool) -> Self {
        Self {
            payload: Payload::Rendered(encode_envelope(-1, body)),
            target: -1,
            address: address.into(),
            needs_ack: false,
            collateable,
            custom_wait_time_ms: None,
            on_sent: None,
        }
    }
}

#[derive(Default)]
pub struct Outbox {
    packets: Vec<Packet>,
}

impl Outbox {
    pub fn push(&mut self, packet: Packet) {
        self.packets.push(packet);
    }

    /// `collateOutboxPackets`: keep only the newest packet per non-empty
    /// `(address, target)` where both old and new are collateable.
    pub fn collate(&mut self) {
        let mut i = 0;
        while i < self.packets.len() {
            if !self.packets[i].address.is_empty() && self.packets[i].collateable {
                let key = (self.packets[i].address.clone(), self.packets[i].target);
                // does a newer collateable packet with the same key exist?
                let superseded = self.packets[i + 1..]
                    .iter()
                    .any(|p| p.collateable && p.target == key.1 && p.address == key.0);
                if superseded {
                    self.packets.remove(i);
                    continue;
                }
            }
            i += 1;
        }
    }

    /// `removePacketsFromOutbox(address, target)`.
    pub fn remove(&mut self, address: &str, target: i8) {
        self.packets
            .retain(|p| !(p.address == address && p.target == target));
    }

    pub fn clear(&mut self) {
        self.packets.clear();
    }

    pub fn pop_front(&mut self) -> Option<Packet> {
        if self.packets.is_empty() {
            None
        } else {
            Some(self.packets.remove(0))
        }
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_proto::commands::move_steps;

    fn m_packet(target: i8, a: i32) -> Packet {
        Packet::from_body(target, &move_steps(a, 0), "m")
    }

    #[test]
    fn collate_keeps_latest_per_address_and_target() {
        let mut outbox = Outbox::default();
        outbox.push(m_packet(1, 100));
        outbox.push(m_packet(2, 200));
        outbox.push(m_packet(1, 300)); // supersedes the first
        outbox.collate();
        assert_eq!(outbox.len(), 2);
        let first = outbox.pop_front().unwrap();
        assert_eq!(first.target, 2);
        let second = outbox.pop_front().unwrap();
        assert_eq!(second.target, 1);
        let Payload::Rendered(bytes) = second.payload else {
            panic!()
        };
        // contains int 300 (0xCD 0x01 0x2C)
        assert!(bytes.windows(3).any(|w| w == [0xCD, 0x01, 0x2C]));
    }

    #[test]
    fn non_collateable_packets_survive() {
        let mut outbox = Outbox::default();
        let mut p1 = m_packet(1, 100);
        p1.collateable = false;
        outbox.push(p1);
        outbox.push(m_packet(1, 300));
        outbox.collate();
        assert_eq!(outbox.len(), 2);
    }

    #[test]
    fn empty_address_never_collates() {
        let mut outbox = Outbox::default();
        let mut p1 = m_packet(1, 100);
        p1.address = String::new();
        let mut p2 = m_packet(1, 200);
        p2.address = String::new();
        outbox.push(p1);
        outbox.push(p2);
        outbox.collate();
        assert_eq!(outbox.len(), 2);
    }

    #[test]
    fn remove_by_address_and_target() {
        let mut outbox = Outbox::default();
        outbox.push(Packet::broadcast(&move_steps(0, 0), "keyframe", false));
        outbox.push(m_packet(1, 100));
        outbox.remove("keyframe", -1);
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox.pop_front().unwrap().target, 1);
    }
}

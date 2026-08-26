//! What the host remembers about the six RS485 repeaters on a column's bus.
//!
//! Repeaters answer with a negative source address, which `Column::process_incoming`
//! has always discarded (`if envelope.source <= 0 { return; }`) because until now
//! nothing could produce one. This is the state those replies feed.

use std::time::Instant;

use router_proto::repeater::{portal_range, RepeaterReply, RepeaterVerb};
use router_proto::Value;

/// A repeater's last reported health. Fields mirror the `status` payload; anything
/// absent stays `None` rather than defaulting, so "not reported" is distinguishable
/// from "reported as zero".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RepeaterStatus {
    pub proto_version: Option<u16>,
    pub version: Option<String>,
    pub build: Option<String>,
    pub mac: Option<[u8; 6]>,
    pub index: Option<u8>,
    pub block_state: Option<String>,
    pub range: Option<(u8, u8)>,
    pub event_seq: Option<u64>,
    pub reset_reason: Option<String>,
    pub boots: Option<u64>,
    pub unhealthy_boots: Option<u64>,
    pub min_free_heap: Option<u64>,
    pub uptime_ms: Option<u64>,
    pub core_dump: Option<bool>,
    pub queue_drops: Option<u64>,
    pub parse_errors: Option<u64>,
    /// Repeater-plane frames this unit passed on to a panel further down the chain.
    pub relayed_control: Option<u64>,
    pub paused_drops: Option<u64>,
}

impl RepeaterStatus {
    /// Whether this repeater is in the state a healthy panel should be in: provisioned,
    /// knowing which nine Portal IDs are its own, and dropping nothing.
    ///
    /// This used to require the unit to be *filtering*, which in a chain is precisely the
    /// state that strands every panel below it — a broken chain would have reported
    /// healthy and a working one would not.
    pub fn healthy(&self) -> bool {
        let Some(index) = self.index else {
            return false;
        };
        let block_assigned = self.block_state.as_deref() == Some("assigned");
        let range_correct = self.range == portal_range(index);
        let quiet = self.queue_drops.unwrap_or(0) == 0;
        block_assigned && range_correct && quiet
    }

    /// Whether the host may use the snapshot and OTA verbs against this unit. A
    /// repeater whose protocol version has not been read is not assumed capable —
    /// the fleet may be mixed, and the fallback is per repeater, not fleet-wide.
    pub fn supports(&self, required: u16) -> bool {
        self.proto_version.is_some_and(|version| version >= required)
    }
}

/// One repeater as the host sees it.
#[derive(Debug, Clone)]
pub struct RepeaterRecord {
    pub address: i8,
    pub index: Option<u8>,
    pub status: RepeaterStatus,
    pub last_seen: Option<Instant>,
    /// The most recent reply for each verb, so a failed `ota-end` is still visible
    /// after a later `ota-map` succeeds.
    pub last_verb: Option<RepeaterVerb>,
    pub last_ok: bool,
}

impl RepeaterRecord {
    pub fn new(address: i8) -> Self {
        Self {
            address,
            index: router_proto::repeater_index(address),
            status: RepeaterStatus::default(),
            last_seen: None,
            last_verb: None,
            last_ok: false,
        }
    }
}

/// The repeater plane on one column's bus.
#[derive(Debug, Clone, Default)]
pub struct RepeaterPlane {
    records: Vec<RepeaterRecord>,
}

impl RepeaterPlane {
    /// Folds a reply into the plane. Returns the address it was attributed to.
    pub fn observe(&mut self, reply: &RepeaterReply) -> i8 {
        let position = match self.records.iter().position(|r| r.address == reply.address) {
            Some(position) => position,
            None => {
                self.records.push(RepeaterRecord::new(reply.address));
                self.records.len() - 1
            }
        };
        let record = &mut self.records[position];
        record.last_seen = Some(Instant::now());
        record.last_verb = reply.verb;
        record.last_ok = reply.ok;

        if reply.verb == Some(RepeaterVerb::Status) {
            if let Some(payload) = &reply.payload {
                record.status = parse_status(payload);
                if let Some(index) = record.status.index.filter(|index| *index != 0) {
                    record.index = Some(index);
                }
            }
        }
        reply.address
    }

    pub fn records(&self) -> &[RepeaterRecord] {
        &self.records
    }

    pub fn by_index(&self, index: u8) -> Option<&RepeaterRecord> {
        self.records.iter().find(|r| r.index == Some(index))
    }

    /// Repeaters that answered but have not been provisioned with an index. They
    /// reply as the broadcast address and can only be reached by MAC.
    pub fn unprovisioned(&self) -> impl Iterator<Item = &RepeaterRecord> {
        self.records.iter().filter(|r| r.index.is_none())
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }
}

fn field<'a>(map: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| k.as_str() == Some(name))
        .map(|(_, v)| v)
}

fn parse_status(payload: &Value) -> RepeaterStatus {
    let Value::Map(entries) = payload else {
        return RepeaterStatus::default();
    };
    let mut status = RepeaterStatus {
        proto_version: field(entries, "proto").and_then(|v| v.as_u64()).map(|v| v as u16),
        version: field(entries, "ver").and_then(|v| v.as_str()).map(str::to_owned),
        build: field(entries, "build").and_then(|v| v.as_str()).map(str::to_owned),
        index: field(entries, "idx").and_then(|v| v.as_i64()).map(|v| v as u8),
        block_state: field(entries, "mode").and_then(|v| v.as_str()).map(str::to_owned),
        event_seq: field(entries, "ev").and_then(|v| v.as_u64()),
        ..Default::default()
    };

    if let Some(mac) = field(entries, "mac").and_then(|v| v.as_slice()) {
        if mac.len() == 6 {
            let mut bytes = [0u8; 6];
            bytes.copy_from_slice(mac);
            status.mac = Some(bytes);
        }
    }
    if let Some(Value::Array(range)) = field(entries, "range") {
        if range.len() == 2 {
            if let (Some(start), Some(end)) = (range[0].as_u64(), range[1].as_u64()) {
                status.range = Some((start as u8, end as u8));
            }
        }
    }
    if let Some(Value::Map(filters)) = field(entries, "flt") {
        status.parse_errors = field(filters, "pe").and_then(|v| v.as_u64());
        status.relayed_control = field(filters, "relay").and_then(|v| v.as_u64());
    }
    if let Some(Value::Map(plane)) = field(entries, "plane") {
        status.paused_drops = field(plane, "pd").and_then(|v| v.as_u64());
    }
    // Queue drops are per direction; the host cares that any occurred at all.
    let drops = ["s1", "s2"]
        .iter()
        .filter_map(|side| match field(entries, side) {
            Some(Value::Map(dir)) => field(dir, "qdr").and_then(|v| v.as_u64()),
            _ => None,
        })
        .sum::<u64>();
    if field(entries, "s1").is_some() {
        status.queue_drops = Some(drops);
    }
    if let Some(Value::Map(health)) = field(entries, "health") {
        status.reset_reason = field(health, "rst").and_then(|v| v.as_str()).map(str::to_owned);
        status.boots = field(health, "boots").and_then(|v| v.as_u64());
        status.unhealthy_boots = field(health, "unhealthy").and_then(|v| v.as_u64());
        status.min_free_heap = field(health, "heap").and_then(|v| v.as_u64());
        status.uptime_ms = field(health, "up").and_then(|v| v.as_u64());
        status.core_dump = field(health, "cd").and_then(|v| v.as_bool());
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_proto::value::{key, map};

    fn status_payload(index: u8, mode: &str, range: (u8, u8), drops: u64) -> Value {
        map(vec![
            (key("proto"), Value::from(1)),
            (key("ver"), Value::from("3.0.0")),
            (key("build"), Value::from("abc123-dirty")),
            (key("mac"), Value::Binary(vec![0xF8, 0x5B, 0x1B, 0xED, 0x8D, 0xA4])),
            (key("idx"), Value::from(index)),
            (key("mode"), Value::from(mode)),
            (
                key("range"),
                Value::Array(vec![Value::from(range.0), Value::from(range.1)]),
            ),
            (key("s1"), map(vec![(key("qdr"), Value::from(drops))])),
            (key("s2"), map(vec![(key("qdr"), Value::from(0))])),
            (
                key("flt"),
                map(vec![(key("pe"), Value::from(0)), (key("cfl"), Value::from(0))]),
            ),
            (key("ev"), Value::from(7)),
            (
                key("health"),
                map(vec![
                    (key("rst"), Value::from("poweron")),
                    (key("boots"), Value::from(12)),
                    (key("unhealthy"), Value::from(0)),
                    (key("heap"), Value::from(180_000)),
                    (key("up"), Value::from(3_600_000u64)),
                    (key("cd"), Value::from(false)),
                ]),
            ),
        ])
    }

    fn reply(address: i8, payload: Value) -> RepeaterReply {
        RepeaterReply {
            address,
            verb: Some(RepeaterVerb::Status),
            ok: true,
            payload: Some(payload),
        }
    }

    #[test]
    fn a_status_reply_is_folded_into_the_plane() {
        let mut plane = RepeaterPlane::default();
        plane.observe(&reply(-4, status_payload(2, "assigned", (10, 18), 0)));

        let record = plane.by_index(2).expect("repeater 2 recorded");
        assert_eq!(record.address, -4);
        assert_eq!(record.status.version.as_deref(), Some("3.0.0"));
        assert_eq!(record.status.range, Some((10, 18)));
        assert_eq!(record.status.uptime_ms, Some(3_600_000));
        assert_eq!(record.status.reset_reason.as_deref(), Some("poweron"));
        assert_eq!(record.status.mac.unwrap()[0], 0xF8);
        assert!(record.status.healthy());
        assert!(record.status.supports(1));
    }

    #[test]
    fn a_repeater_serving_the_wrong_block_is_not_healthy() {
        let mut plane = RepeaterPlane::default();
        // Index 2 should own 10..18; this one reports 19..27.
        plane.observe(&reply(-4, status_payload(2, "assigned", (19, 27), 0)));
        assert!(!plane.by_index(2).unwrap().status.healthy());
    }

    #[test]
    fn unprovisioned_or_dropping_repeaters_are_not_healthy() {
        let mut plane = RepeaterPlane::default();
        plane.observe(&reply(-3, status_payload(1, "unknown", (0, 0), 0)));
        assert!(!plane.by_index(1).unwrap().status.healthy());

        plane.observe(&reply(-3, status_payload(1, "assigned", (1, 9), 5)));
        assert!(!plane.by_index(1).unwrap().status.healthy());
    }

    #[test]
    fn an_unread_protocol_version_is_never_assumed_capable() {
        let status = RepeaterStatus::default();
        assert!(!status.supports(1));
        let old = RepeaterStatus {
            proto_version: Some(0),
            ..Default::default()
        };
        assert!(!old.supports(1));
    }

    #[test]
    fn an_unprovisioned_repeater_is_tracked_separately() {
        let mut plane = RepeaterPlane::default();
        plane.observe(&RepeaterReply {
            address: router_proto::REPEATER_ALL,
            verb: Some(RepeaterVerb::Status),
            ok: true,
            payload: Some(status_payload(0, "unknown", (0, 0), 0)),
        });
        assert_eq!(plane.unprovisioned().count(), 1);
        assert!(plane.by_index(1).is_none());
    }

    #[test]
    fn repeated_replies_update_in_place_rather_than_accumulating() {
        let mut plane = RepeaterPlane::default();
        plane.observe(&reply(-5, status_payload(3, "unknown", (0, 0), 0)));
        plane.observe(&reply(-5, status_payload(3, "assigned", (19, 27), 0)));
        assert_eq!(plane.records().len(), 1);
        assert!(plane.by_index(3).unwrap().status.healthy());
    }
}

//! `/api/router/*`: the agent's channel. Handlers never touch the runtime — they read the
//! [`Shared`] mirrors or queue a typed command for the bridge, and return. An agent can drive
//! the same installation the page shows with nothing but `curl`.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use glam::vec2;
use router_core::runtime::{Command, Scope};
use router_proto::commands::ActionKind;
use serde::Deserialize;
use serde_json::{Value as Json_, json};

use crate::shared::Shared;

pub fn routes(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/api/router/state", get(state))
        .route("/api/router/diagnostics", get(diagnostics))
        .route("/api/router/logs", get(logs))
        .route("/api/router/ports", get(ports))
        .route("/api/router/command", post(command))
        .route(
            "/api/router/firmware",
            get(firmware_list).post(firmware_upload),
        )
        .route("/api/router/firmware/flash", post(firmware_flash))
        .route(
            "/api/router/repeaters",
            get(repeaters_list).post(repeaters_command),
        )
        .route("/api/router/files", get(files_list))
        .with_state(shared)
}

// ------------------------------------------------------------------ firmware

/// Where `.bin` artefacts are discovered: an explicit override, the repo's `firmware/`
/// directory, and the per-user upload store (where browser uploads land — a WKWebView file
/// picker yields content, not a path, so the bytes come to us and we mint the path).
fn firmware_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("ROUTER_FIRMWARE").filter(|v| !v.is_empty()) {
        dirs.push(std::path::PathBuf::from(dir));
    }
    dirs.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../firmware"));
    if let Ok(state) = av_app_registry::state_dir("router") {
        dirs.push(state.join("firmware"));
    }
    dirs
}

fn upload_dir() -> Option<std::path::PathBuf> {
    av_app_registry::state_dir("router")
        .ok()
        .map(|d| d.join("firmware"))
}

async fn firmware_list() -> Json<Json_> {
    let mut artefacts = Vec::new();
    for dir in firmware_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("bin") {
                continue;
            }
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            artefacts.push(json!({
                "name": path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "path": path.display().to_string(),
                "bytes": bytes,
            }));
        }
    }
    Json(json!({ "artefacts": artefacts }))
}

#[derive(Deserialize)]
struct UploadQuery {
    name: String,
}

async fn firmware_upload(Query(query): Query<UploadQuery>, body: axum::body::Bytes) -> Json<Json_> {
    // One flash bank; anything bigger can't be a valid application image.
    const MAX_BYTES: usize = 4 * 1024 * 1024;
    let name = std::path::Path::new(&query.name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload.bin")
        .to_string();
    if body.len() > MAX_BYTES {
        return Json(json!({ "ok": false, "error": "file too large for a firmware image" }));
    }
    // An ELF is flattened here rather than refused. Everything downstream takes the flat image
    // `objcopy -O binary` produces, and a `.elf` is the same image with a symbol table around it --
    // which is as likely to be the file somebody sends as the `.bin` is. It is stored under a
    // `.bin` name because that is what it now contains, and because `firmware_list` lists `.bin`.
    let (name, image) = if crate::elf::is_elf(&body) {
        match crate::elf::flatten(&body) {
            Ok((_, image)) => (
                format!("{}.bin", name.trim_end_matches(".elf")),
                std::borrow::Cow::Owned(image),
            ),
            Err(error) => {
                return Json(json!({ "ok": false, "error": error.to_string() }));
            }
        }
    } else {
        (name, std::borrow::Cow::Borrowed(&body[..]))
    };
    let Some(dir) = upload_dir() else {
        return Json(json!({ "ok": false, "error": "no per-user state directory" }));
    };
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return Json(json!({ "ok": false, "error": error.to_string() }));
    }
    let path = dir.join(&name);
    match std::fs::write(&path, &image) {
        Ok(()) => {
            Json(json!({ "ok": true, "path": path.display().to_string(), "bytes": image.len() }))
        }
        Err(error) => Json(json!({ "ok": false, "error": error.to_string() })),
    }
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum FirmwareOp {
    Flash { path: String, col: Option<usize> },
    Erase { col: Option<usize> },
    Run { col: Option<usize> },
}

async fn firmware_flash(
    State(shared): State<Arc<Shared>>,
    Json(request): Json<FirmwareOp>,
) -> Json<Json_> {
    match request {
        FirmwareOp::Flash { path, col } => {
            let path = std::path::PathBuf::from(path);
            if !path.is_file() {
                return Json(json!({ "ok": false, "error": "no such file" }));
            }
            shared.queue(Command::FwUpload { col, path });
        }
        FirmwareOp::Erase { col } => shared.queue(Command::FwErase { col }),
        FirmwareOp::Run { col } => shared.queue(Command::FwRun { col }),
    }
    Json(json!({ "ok": true }))
}

// ------------------------------------------------------------------ RS485 repeaters

/// What every repeater on every bus last reported.
///
/// A repeater only appears here once it has answered, so an empty list means
/// "nothing has been asked yet, or nothing answered" -- never "no repeaters".
/// Send `{"op":"status"}` first.
async fn repeaters_list(State(shared): State<Arc<Shared>>) -> Json<Json_> {
    let snapshot = shared.snapshot.lock().unwrap().clone();
    let columns: Vec<_> = snapshot
        .columns
        .iter()
        .map(|column| {
            let repeaters: Vec<_> = column
                .repeaters
                .iter()
                .map(|record| {
                    let status = &record.status;
                    json!({
                        "address": record.address,
                        "index": record.index,
                        "healthy": status.healthy(),
                        "proto": status.proto_version,
                        "version": status.version,
                        "build": status.build,
                        "mac": status.mac.map(|mac| mac
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<Vec<_>>()
                            .join(":")),
                        "block": status.block_state,
                        "range": status.range.map(|(start, end)| [start, end]),
                        "event_seq": status.event_seq,
                        "queue_drops": status.queue_drops,
                        "parse_errors": status.parse_errors,
                        "relayed_control": status.relayed_control,
                        "reset_reason": status.reset_reason,
                        "boots": status.boots,
                        "unhealthy_boots": status.unhealthy_boots,
                        "min_free_heap": status.min_free_heap,
                        "uptime_ms": status.uptime_ms,
                        "core_dump": status.core_dump,
                        "last_verb": record.last_verb.map(|verb| verb.as_str()),
                        "last_ok": record.last_ok,
                    })
                })
                .collect();
            json!({ "col": column.index, "repeaters": repeaters })
        })
        .collect();
    Json(json!({ "ok": true, "columns": columns }))
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum RepeaterOp {
    /// Unicast per repeater. `repeater: null` queries all six, one at a time --
    /// never as a broadcast, which would collide six replies on the wire.
    Status {
        col: usize,
        repeater: Option<u8>,
    },
    /// Provisioning, addressed by MAC because the unit has no index yet.
    SetIndex {
        col: usize,
        mac: String,
        index: u8,
    },
    Relearn {
        col: usize,
        repeater: u8,
    },
    ResetCounters {
        col: usize,
        repeater: u8,
    },
    Reboot {
        col: usize,
        repeater: u8,
    },
    /// One parallel sweep of all six branches, then six reads.
    Snapshot {
        col: usize,
    },
    /// `repeater: null` rolls through all six in turn, which keeps five-sixths of
    /// the installation relaying while each one updates.
    Ota {
        col: usize,
        repeater: Option<u8>,
        path: String,
    },
    OtaAbort {
        col: usize,
        repeater: Option<u8>,
    },
}

fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let cleaned: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.len() != 12 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (index, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&cleaned[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

async fn repeaters_command(
    State(shared): State<Arc<Shared>>,
    Json(request): Json<RepeaterOp>,
) -> Json<Json_> {
    const RANGE: std::ops::RangeInclusive<u8> = 1..=router_core::REPEATER_COUNT;
    match request {
        RepeaterOp::Status { col, repeater } => {
            if let Some(index) = repeater {
                if !RANGE.contains(&index) {
                    return Json(json!({ "ok": false, "error": "repeater must be 1..=6" }));
                }
            }
            shared.queue(Command::RepeaterStatus { col, repeater });
        }
        RepeaterOp::SetIndex { col, mac, index } => {
            let Some(mac) = parse_mac(&mac) else {
                return Json(json!({ "ok": false, "error": "mac must be 12 hex digits" }));
            };
            // 0 clears the index, so the accepted range is wider than for the rest.
            if index > router_core::REPEATER_COUNT {
                return Json(json!({ "ok": false, "error": "index must be 0..=6" }));
            }
            shared.queue(Command::RepeaterSetIndex { col, mac, index });
        }
        RepeaterOp::Relearn { col, repeater } => {
            shared.queue(Command::RepeaterRelearn { col, repeater })
        }
        RepeaterOp::ResetCounters { col, repeater } => {
            shared.queue(Command::RepeaterResetCounters { col, repeater })
        }
        RepeaterOp::Reboot { col, repeater } => {
            shared.queue(Command::RepeaterReboot { col, repeater })
        }
        RepeaterOp::Snapshot { col } => shared.queue(Command::RepeaterSnapshot { col }),
        RepeaterOp::Ota {
            col,
            repeater,
            path,
        } => {
            let path = std::path::PathBuf::from(path);
            if !path.is_file() {
                return Json(json!({ "ok": false, "error": "no such file" }));
            }
            shared.queue(Command::RepeaterOta {
                col,
                repeater,
                path,
            });
        }
        RepeaterOp::OtaAbort { col, repeater } => {
            shared.queue(Command::RepeaterOtaAbort { col, repeater })
        }
    }
    Json(json!({ "ok": true }))
}

// ------------------------------------------------------------------ media files

/// Video files for the FilePlayer source: an explicit override plus the repo's `media/`.
async fn files_list() -> Json<Json_> {
    const VIDEO_EXT: &[&str] = &["mp4", "mov", "avi", "mkv", "webm", "mpg", "mpeg"];
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("ROUTER_MEDIA").filter(|v| !v.is_empty()) {
        dirs.push(std::path::PathBuf::from(dir));
    }
    dirs.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../media"));
    let mut files = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !VIDEO_EXT.contains(&ext.as_str()) {
                continue;
            }
            files.push(json!({
                "name": path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                "path": path.display().to_string(),
                "bytes": entry.metadata().map(|m| m.len()).unwrap_or(0),
            }));
        }
    }
    Json(json!({ "files": files }))
}

async fn state(State(shared): State<Arc<Shared>>) -> Json<Json_> {
    let snap = shared.snapshot.lock().unwrap().clone();
    let columns: Vec<Json_> = snap
        .columns
        .iter()
        .map(|column| {
            json!({
                "index": column.index,
                "count_x": column.count_x,
                "count_y": column.count_y,
                "panel_height": column.panel_height,
                "flipped": column.flipped,
                "connected": column.stats.connected,
                "device": column.stats.device_description,
                "tx": column.stats.tx_count,
                "rx": column.stats.rx_count,
                "ack_timeouts": column.stats.ack_timeouts,
                "decode_errors": column.stats.decode_errors,
                "outbox": column.stats.outbox_size,
                "scheduled_poll": {
                    "enabled": column.scheduled_poll_enabled,
                    "period_s": column.scheduled_poll_period_s,
                },
                "portals": column.portals.iter().map(|portal| json!({
                    "target": portal.target,
                    "position": [portal.position.x, portal.position.y],
                    "polar": [portal.polar.x, portal.polar.y],
                    "axes": [portal.axes.x, portal.axes.y],
                    "live_position": portal.live_position.map(|v| [v.x, v.y]),
                    "live_target_position": portal.live_target_position.map(|v| [v.x, v.y]),
                    "in_target_position": portal.in_target_position,
                    "last_rx_age_ms": portal.last_rx_age_ms,
                    "uptime_ms": portal.up_time_ms,
                    "version": portal.version,
                    "leading_control": portal.leading_control,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(json!({
        "generation": snap.generation,
        "resolution": [snap.resolution.0, snap.resolution.1],
        "arrangement": {
            "columns": snap.arrangement.0,
            "rows": snap.arrangement.1,
            "column_width": snap.arrangement.2,
            "panel_height": snap.arrangement.3,
            "flipped": snap.arrangement.4,
        },
        "transmit_mode": snap.transmit_mode,
        "image_enabled": snap.image_enabled,
        "osc": { "running": snap.osc_running, "port": snap.osc_port },
        "rest": { "running": snap.rest_running, "port": snap.rest_port },
        "sources": snap.sources,
        "columns": columns,
    }))
}

async fn diagnostics(State(shared): State<Arc<Shared>>) -> Json<Json_> {
    let diag = shared.diag.lock().unwrap().clone();
    Json(serde_json::to_value(&*diag).unwrap_or(Json_::Null))
}

#[derive(Deserialize)]
struct LogsQuery {
    col: usize,
    portal: u8,
}

async fn logs(State(shared): State<Arc<Shared>>, Query(query): Query<LogsQuery>) -> Json<Json_> {
    let snap = shared.snapshot.lock().unwrap().clone();
    let logs = snap
        .columns
        .get(query.col)
        .and_then(|c| c.portals.iter().find(|p| p.target == query.portal))
        .map(|portal| {
            portal
                .logs
                .iter()
                .map(|(level, message, count)| {
                    json!({ "level": level, "message": message, "count": count })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Json(json!({ "col": query.col, "portal": query.portal, "logs": logs }))
}

async fn ports(State(shared): State<Arc<Shared>>) -> Json<Json_> {
    Json(json!({ "ports": *shared.ports.lock().unwrap() }))
}

/// Typed command surface, explicit scope, no dependence on the page's selection.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ApiCommand {
    SetPosition {
        col: usize,
        portal: u8,
        x: f32,
        y: f32,
    },
    SetPolar {
        col: usize,
        portal: u8,
        r: f32,
        theta: f32,
    },
    SetAxes {
        col: usize,
        portal: u8,
        a: f32,
        b: f32,
    },
    PilotAll {
        col: Option<usize>,
        x: f32,
        y: f32,
    },
    /// One of the 12 hardware actions by its OSC name (`ping`, `home`, `seeThrough`, …).
    Action {
        action: String,
        col: Option<usize>,
        portal: Option<u8>,
    },
    Poll {
        col: Option<usize>,
        portal: Option<u8>,
    },
    Unwind {
        col: Option<usize>,
        portal: Option<u8>,
    },
    Push {
        col: usize,
        portal: u8,
    },
    PollPosition {
        col: usize,
        portal: u8,
    },
    HomeAndZeroLocal,
    /// Raw broadcast body as JSON, encoded to msgpack by the model.
    Broadcast {
        body: Json_,
        collateable: Option<bool>,
    },
    Marker {
        text: String,
    },
    SaveConfig,
    RebuildColumns,
    /// Apply a JSON fragment to a renderer source (any key its deserialise accepts —
    /// the page's file picker writes `{"file": "<path>"}` through this).
    SetSourceParams {
        index: usize,
        params: Json_,
    },
}

fn scope(col: Option<usize>, portal: Option<u8>) -> Scope {
    match (col, portal) {
        (Some(col), Some(portal)) => Scope::Portal(col, portal),
        (Some(col), None) => Scope::Column(col),
        _ => Scope::All,
    }
}

fn action_by_osc_name(name: &str) -> Option<ActionKind> {
    ActionKind::ALL
        .into_iter()
        .find(|kind| kind.osc_address().eq_ignore_ascii_case(name))
}

async fn command(
    State(shared): State<Arc<Shared>>,
    Json(request): Json<ApiCommand>,
) -> Json<Json_> {
    let queued = match request {
        ApiCommand::SetPosition { col, portal, x, y } => {
            shared.queue(Command::SetPilotPosition {
                col,
                portal,
                position: vec2(x, y),
            });
            true
        }
        ApiCommand::SetPolar {
            col,
            portal,
            r,
            theta,
        } => {
            shared.queue(Command::SetPilotPolar {
                col,
                portal,
                polar: vec2(r, theta),
            });
            true
        }
        ApiCommand::SetAxes { col, portal, a, b } => {
            shared.queue(Command::SetPilotAxes {
                col,
                portal,
                axes: vec2(a, b),
            });
            true
        }
        ApiCommand::PilotAll { col, x, y } => {
            shared.queue(Command::PilotAll {
                col,
                position: vec2(x, y),
            });
            true
        }
        ApiCommand::Action {
            action,
            col,
            portal,
        } => match action_by_osc_name(&action) {
            Some(kind) => {
                shared.queue(Command::PerformAction {
                    scope: scope(col, portal),
                    action: kind,
                });
                true
            }
            None => {
                return Json(json!({ "ok": false, "error": format!("unknown action: {action}") }));
            }
        },
        ApiCommand::Poll { col, portal } => {
            shared.queue(Command::Poll(scope(col, portal)));
            true
        }
        ApiCommand::Unwind { col, portal } => {
            shared.queue(Command::Unwind(scope(col, portal)));
            true
        }
        ApiCommand::Push { col, portal } => {
            shared.queue(Command::Push { col, portal });
            true
        }
        ApiCommand::PollPosition { col, portal } => {
            shared.queue(Command::PollPosition { col, portal });
            true
        }
        ApiCommand::HomeAndZeroLocal => {
            shared.queue(Command::HomeAndZeroLocal);
            true
        }
        ApiCommand::Broadcast { body, collateable } => {
            shared.queue(Command::Broadcast {
                body: json_to_proto(&body),
                collateable: collateable.unwrap_or(false),
            });
            true
        }
        ApiCommand::Marker { text } => {
            shared.queue(Command::Marker(text));
            true
        }
        ApiCommand::SaveConfig => {
            shared.queue(Command::SaveConfig);
            true
        }
        ApiCommand::RebuildColumns => {
            shared.queue(Command::RebuildColumns);
            true
        }
        ApiCommand::SetSourceParams { index, params } => {
            shared.queue(Command::SourceSetParams { index, params });
            true
        }
    };
    Json(json!({ "ok": queued }))
}

/// JSON → msgpack value, for raw broadcasts (mirrors the OSC server's body building).
fn json_to_proto(json: &Json_) -> router_proto::Value {
    use router_proto::Value as V;
    match json {
        Json_::Null => V::Nil,
        Json_::Bool(b) => V::Boolean(*b),
        Json_::Number(n) => {
            if let Some(i) = n.as_i64() {
                V::from(i)
            } else {
                V::F64(n.as_f64().unwrap_or(0.0))
            }
        }
        Json_::String(s) => V::from(s.as_str()),
        Json_::Array(items) => V::Array(items.iter().map(json_to_proto).collect()),
        Json_::Object(map) => V::Map(
            map.iter()
                .map(|(k, v)| (router_proto::Value::from(k.as_str()), json_to_proto(v)))
                .collect(),
        ),
    }
}

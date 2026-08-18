//! `/api/bench/*` — the agent's channel.
//!
//! The bus is the human's channel and HTTP is the agent's: an agent cannot `curl` a WebSocket,
//! and a counter-bump parameter is a UI idiom rather than an API. Both feed **one queue**, in
//! [`Shared::requests`], drained by the one worker thread — so the GUI and an agent cannot
//! disagree about the hardware in front of them.
//!
//! No handler here touches a link. Each one copies bounded input into the worker's queue and
//! returns, or reads the mirror the worker publishes.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use bench_core::bench::Origin;
use bench_core::transport::Op;
use bench_core::transport::{Channel, MotionProfile};

use crate::worker::{Request, Shared};

pub fn routes(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/api/bench/state", get(state))
        .route("/api/bench/plans", get(plans))
        .route("/api/bench/log", get(log))
        .route("/api/bench/telemetry", get(telemetry))
        .route("/api/bench/firmware", get(firmware))
        .route("/api/bench/run", post(run))
        .route("/api/bench/abort", post(abort))
        .route("/api/bench/command", post(command))
        .route("/api/bench/ports", get(ports))
        .with_state(shared)
}

/// The whole world in one document.
///
/// Deliberately one request: an agent deciding what to do next should not have to correlate
/// four endpoints that were each sampled at a different instant.
async fn state(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    let state = shared.state.lock().unwrap().clone();
    let run = shared.run.lock().unwrap().clone();
    let last = shared.last.lock().unwrap().clone();
    let flash = shared.flash.lock().unwrap().clone();

    Json(serde_json::json!({
        "link": state.link,
        "dut": state.dut,
        "channels": state.channels,
        "active_channel": state.active_channel,
        "flash": flash,
        "faults": state.faults,
        "running": run.map(|status| serde_json::json!({
            "run_id": status.run_id,
            "plan": status.plan,
            "origin": status.origin.name(),
            "phase": status.phase.name(),
            "step_name": status.step_name,
            "step_index": status.step_index,
            "step_count": status.step_count,
            "cycle": status.cycle,
            "cycle_count": status.cycle_count,
            "elapsed_s": status.elapsed_s,
        })),
        "last": last.map(|outcome| serde_json::json!({
            "run_id": outcome.run_id,
            "plan": outcome.plan,
            "origin": outcome.origin.name(),
            "verdict": outcome.verdict.name(),
            "reason": outcome.verdict.reason(),
            "duration_ms": outcome.duration_ms,
            "measurements": outcome.measurements,
            "report": outcome.report_path,
        })),
    }))
}

async fn ports() -> impl IntoResponse {
    Json(bench_core::survey())
}

async fn firmware(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    Json(shared.artefacts.lock().unwrap().clone())
}

#[derive(serde::Deserialize)]
struct PlansQuery {
    #[serde(default)]
    dir: Option<String>,
}

/// The plans on disk, with whether each one parses.
///
/// A plan that fails to load is listed **with its error** rather than omitted: a plan silently
/// missing from the list is indistinguishable from one that was never written.
async fn plans(Query(query): Query<PlansQuery>) -> impl IntoResponse {
    let dir = query
        .dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(crate::plans_dir);
    let entries: Vec<serde_json::Value> = bench_core::plan::load_dir(&dir)
        .into_iter()
        .map(|(name, result)| match result {
            Ok(plan) => serde_json::json!({
                "name": name,
                "ok": true,
                "kind": format!("{:?}", plan.kind).to_lowercase(),
                "requires": plan.requires,
                "steps": plan.all_steps().len(),
                "criteria": plan.criteria.len(),
            }),
            Err(error) => serde_json::json!({ "name": name, "ok": false, "error": error }),
        })
        .collect();
    Json(serde_json::json!({ "dir": dir.to_string_lossy(), "plans": entries }))
}

#[derive(serde::Deserialize)]
struct LogQuery {
    #[serde(default)]
    from: u64,
}

/// Log lines from a cursor, so a follower resumes without re-reading.
async fn log(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<LogQuery>,
) -> impl IntoResponse {
    let lines = shared.log.lock().unwrap();
    let tail: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["seq"].as_u64().unwrap_or(0) >= query.from)
        .collect();
    let next = lines
        .last()
        .and_then(|line| line["seq"].as_u64())
        .map(|s| s + 1)
        .unwrap_or(query.from);
    Json(serde_json::json!({ "from": query.from, "next": next, "lines": tail }))
}

async fn telemetry(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<LogQuery>,
) -> impl IntoResponse {
    let samples = shared.telemetry.lock().unwrap();
    let tail: Vec<&serde_json::Value> = samples
        .iter()
        .filter(|sample| sample["seq"].as_u64().unwrap_or(0) >= query.from)
        .collect();
    let next = samples
        .last()
        .and_then(|sample| sample["seq"].as_u64())
        .map(|seq| seq + 1)
        .unwrap_or(query.from);
    Json(serde_json::json!({ "from": query.from, "next": next, "samples": tail }))
}

#[derive(serde::Deserialize)]
struct RunBody {
    /// A plan by name from the plans directory.
    #[serde(default)]
    plan: Option<String>,
    /// Or a plan inline, as JSON.
    #[serde(default)]
    inline: Option<bench_core::plan::Plan>,
    /// Communication lane used by this run. Omitted preserves the current active lane.
    #[serde(default)]
    channel: Option<Channel>,
}

async fn run(State(shared): State<Arc<Shared>>, Json(body): Json<RunBody>) -> impl IntoResponse {
    if let Some(status) = shared.run.lock().unwrap().clone() {
        // 409 names what is already running: "busy" without saying what is busy leaves the
        // caller with nothing to act on.
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "a run is already in flight",
                "run_id": status.run_id,
                "plan": status.plan,
            })),
        )
            .into_response();
    }

    let RunBody {
        plan: plan_name,
        inline,
        channel,
    } = body;
    let plan = match (inline, plan_name) {
        (Some(plan), _) => plan,
        (None, Some(name)) => {
            let path = crate::plans_dir().join(format!("{name}.toml"));
            match bench_core::plan::load(&path) {
                Ok(plan) => plan,
                Err(error) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": error })),
                    )
                        .into_response();
                }
            }
        }
        (None, None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "give either `plan` or `inline`" })),
            )
                .into_response();
        }
    };

    let name = plan.name.clone();
    *shared.last_start_error.lock().unwrap() = None;
    if let Some(channel) = channel {
        shared.push(Request::SelectChannel(channel));
    }
    shared.push(Request::Run {
        plan: Box::new(plan),
        origin: Origin::Agent,
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "accepted": true, "plan": name })),
    )
        .into_response()
}

async fn abort(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    let running = shared.run.lock().unwrap().is_some();
    shared.push(Request::Abort);
    Json(serde_json::json!({ "was_running": running }))
}

/// One operation, executed straight away. The live-driving door.
#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum CommandBody {
    Connect {
        kind: String,
        endpoint: String,
    },
    Disconnect {
        #[serde(default)]
        channel: Option<Channel>,
    },
    Identify {
        #[serde(default)]
        channel: Option<Channel>,
    },
    Poll {
        #[serde(default)]
        channel: Option<Channel>,
    },
    DiscoverRs485,
    SelectRs485Target {
        target: i8,
    },
    Escape {
        #[serde(default)]
        channel: Option<Channel>,
    },
    Home {
        axis: bench_core::dut::Axis,
        #[serde(default)]
        channel: Option<Channel>,
    },
    Unjam {
        axis: bench_core::dut::Axis,
        #[serde(default)]
        channel: Option<Channel>,
    },
    Move {
        axis: bench_core::dut::Axis,
        usteps: i32,
        #[serde(default)]
        profile: Option<MotionProfile>,
        #[serde(default)]
        channel: Option<Channel>,
    },
    MoveAxes {
        a: i32,
        b: i32,
        #[serde(default)]
        channel: Option<Channel>,
    },
    Flash,
    ReadDevice,
    RescanFirmware,
    /// Soft-reset the module. The startup routine then runs from a genuine power-on state,
    /// which is the only way to exercise the cold/default path more than once per session --
    /// every home after the first reuses the axis's cached calibration.
    Reboot {
        #[serde(default)]
        channel: Option<Channel>,
    },
    /// Set the shared optical comparator threshold. One DAC feeds both axes, so no axis here.
    SetHomeThreshold {
        value: i32,
        #[serde(default)]
        channel: Option<Channel>,
    },
    /// One revolution at a fixed settled threshold, reporting every comparator transition.
    ///
    /// Exposed on the command route rather than only inside a plan because choosing a threshold
    /// is a *sweep* -- a lap per candidate -- and the sweep is the thing an agent drives while
    /// the operator watches the segments arrive in the GUI.
    Census {
        axis: bench_core::dut::Axis,
        threshold: u8,
        #[serde(default)]
        speed: Option<i32>,
        #[serde(default)]
        channel: Option<Channel>,
    },
}

async fn command(
    State(shared): State<Arc<Shared>>,
    Json(body): Json<CommandBody>,
) -> impl IntoResponse {
    let request = match body {
        CommandBody::Connect { kind, endpoint } => {
            let Some(kind) = kind_from_name(&kind) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("unknown transport `{kind}`") })),
                )
                    .into_response();
            };
            Request::Connect { kind, endpoint }
        }
        CommandBody::Disconnect {
            channel: Some(channel),
        } => Request::DisconnectChannel(channel),
        CommandBody::Disconnect { channel: None } => Request::Disconnect,
        CommandBody::DiscoverRs485 => Request::DiscoverRs485,
        CommandBody::SelectRs485Target { target } => Request::SelectRs485Target(target),
        CommandBody::Identify { channel } => routed(channel, Op::Identify),
        CommandBody::Poll { channel } => routed(channel, Op::Poll),
        CommandBody::Escape { channel } => routed(channel, Op::Escape),
        CommandBody::Home { axis, channel } => routed(channel, Op::Home { axis }),
        CommandBody::Unjam { axis, channel } => routed(channel, Op::Unjam { axis }),
        CommandBody::Move {
            axis,
            usteps,
            profile,
            channel,
        } => routed(
            channel,
            Op::MoveTo {
                axis,
                usteps,
                profile,
            },
        ),
        CommandBody::MoveAxes { a, b, channel } => routed(channel, Op::MoveAxes { a, b }),
        CommandBody::Flash => Request::FlashNow,
        CommandBody::ReadDevice => Request::ReadDevice,
        CommandBody::RescanFirmware => Request::RescanFirmware,
        CommandBody::Reboot { channel } => routed(channel, Op::Reboot),
        CommandBody::SetHomeThreshold { value, channel } => {
            routed(channel, Op::SetHomeThreshold { value })
        }
        CommandBody::Census {
            axis,
            threshold,
            speed,
            channel,
        } => routed(
            channel,
            Op::Census {
                axis,
                threshold,
                speed,
            },
        ),
    };
    shared.push(request);
    Json(serde_json::json!({ "accepted": true })).into_response()
}

fn routed(channel: Option<Channel>, op: Op) -> Request {
    match channel {
        Some(channel) => Request::SubmitTo { channel, op },
        None => Request::Submit(op),
    }
}

fn kind_from_name(name: &str) -> Option<bench_core::transport::LinkKind> {
    use bench_core::transport::LinkKind;
    Some(match name {
        "vcp" => LinkKind::Vcp,
        "bench-ascii" => LinkKind::BenchAscii,
        "rs485-serial" => LinkKind::Rs485Serial,
        "rs485-tcp" => LinkKind::Rs485Tcp,
        "sim" => LinkKind::Sim,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_names_on_the_wire_match_the_ones_the_schema_declares() {
        for (_, name) in crate::schema::TRANSPORTS {
            if *name == "none" {
                continue;
            }
            assert!(
                kind_from_name(name).is_some(),
                "`{name}` is declared but not accepted by the API"
            );
        }
    }

    /// A command body has to deserialise from what the CLI actually sends.
    #[test]
    fn command_bodies_parse_from_their_documented_shape() {
        let body: CommandBody = serde_json::from_value(serde_json::json!({
            "op": "home", "axis": "a"
        }))
        .unwrap();
        assert!(matches!(
            body,
            CommandBody::Home {
                axis: bench_core::dut::Axis::A,
                channel: None
            }
        ));

        let body: CommandBody = serde_json::from_value(serde_json::json!({
            "op": "connect", "kind": "vcp", "endpoint": "COM3"
        }))
        .unwrap();
        assert!(matches!(body, CommandBody::Connect { .. }));
    }
}

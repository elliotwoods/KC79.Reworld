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
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bench_core::bench::Origin;
use bench_core::transport::Op;
use bench_core::transport::direct::SurveyConfig;
use bench_core::transport::{Channel, LineEnding, MotionProfile, RawSignal};

use crate::worker::{Request, Shared, SurveyMirror};

pub fn routes(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/api/bench/state", get(state))
        .route("/api/bench/plans", get(plans))
        .route("/api/bench/log", get(log))
        .route("/api/bench/telemetry", get(telemetry))
        .route("/api/bench/survey", get(survey))
        .route("/api/bench/survey/export.json", get(export_survey_json))
        .route("/api/bench/survey/export.csv", get(export_survey_csv))
        .route("/api/bench/firmware", get(firmware))
        .route("/api/bench/firmware/dropped", post(drop_firmware))
        .route("/api/bench/firmware/dropped", delete(forget_firmware))
        .route("/api/bench/provision", get(provision))
        .route("/api/bench/provision/history", get(provision_history))
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
    let provision = shared.provision.lock().unwrap().clone();

    Json(serde_json::json!({
        "link": state.link,
        "dut": state.dut,
        "channels": state.channels,
        "active_channel": state.active_channel,
        "field_update": state.field_update,
        "flash": flash,
        "provision": provision,
        "faults": state.faults,
        "direct": state.direct,
        "survey": state.survey,
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

/// What is plugged into this machine, as the worker last saw it.
///
/// A mirror read, not an enumeration -- see this module's contract above. `bench_core::survey()`
/// is a blocking IOKit and USB walk: on a tokio worker thread it stalls unrelated requests, and it
/// races the one thread that owns the probe. `generation` is the same counter published on
/// `/setup/ports_generation`, so a page that saw a bump can tell whether this answer already
/// includes it.
async fn ports(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    match ports_body(&shared.survey.lock().unwrap()) {
        Some(body) => Json(body).into_response(),
        // Unreachable in a running process: `Worker::new` fills the mirror before the host binds.
        // Said out loud anyway, because "the worker has not scanned yet" and "nothing is attached"
        // are different claims and a page that cannot tell them apart draws the wrong empty state.
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "the bench worker has not surveyed yet" })),
        )
            .into_response(),
    }
}

/// The document, split out so it can be tested without a runtime or a bench.
fn ports_body(mirror: &SurveyMirror) -> Option<serde_json::Value> {
    let survey = mirror.survey.as_ref()?;
    Some(serde_json::json!({
        "ports": survey.ports,
        "probes": survey.probes,
        "swd_support": survey.swd_support,
        "generation": mirror.generation,
        "scanned_at_ms": mirror.scanned_at_ms,
    }))
}

async fn firmware(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    Json(shared.artefacts.lock().unwrap().clone())
}

/// The largest thing that can be a firmware image for this part, with room for an ELF.
///
/// The flat image can be at most the 128 kB of flash and is checked against the bank it is going
/// into later; an unstripped ELF carrying debug info for the same code runs several times that --
/// the committed reference bootloader is 22 kB of image inside a 155 kB ELF. One megabyte is
/// generous for both and still refuses a video someone dragged onto the wrong window before any of
/// it is held in memory to be classified.
const MAX_DROP_BYTES: usize = 1024 * 1024;

#[derive(serde::Deserialize)]
struct DropQuery {
    /// The operator's own filename, which becomes the row's name. Sanitised before use.
    name: String,
    /// `bootloader`, `application`, or absent for "work it out".
    #[serde(default)]
    bank: Option<String>,
}

/// Take firmware handed to the bench directly, rather than found by a scan.
///
/// The bytes are the body, not a path, and that is the whole design rather than an accident of
/// HTTP: the same page runs in a WKWebView, in a WebView2 window and in a browser on another
/// machine, and only one of those three could have handed over a path. Uploading the bytes is the
/// one answer that works in all three, and it is also the one that works when the operator is not
/// sitting at the bench.
///
/// This handler does the byte work -- flatten, classify, write -- because none of it touches
/// hardware and because answering the drop immediately is what makes it feel like it landed. What
/// it does not do is mutate the worker's `Discovery`; that is enqueued, and the page catches up on
/// `/flash/artefacts_generation`.
async fn drop_firmware(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<DropQuery>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    use portal_swd::staging::{Bank, StageError};

    if body.is_empty() {
        return refused(StatusCode::BAD_REQUEST, "the dropped file is empty");
    }
    if body.len() > MAX_DROP_BYTES {
        return refused(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "{} kB is too large to be firmware for this part",
                body.len() / 1024
            ),
        );
    }
    let bank = match query.bank.as_deref() {
        None | Some("") | Some("auto") => Bank::Auto,
        Some("bootloader") => Bank::Bootloader,
        Some("application") => Bank::Application,
        Some(other) => {
            return refused(
                StatusCode::BAD_REQUEST,
                &format!("`{other}` is not a bank; use bootloader, application, or auto"),
            );
        }
    };

    // The same function `FlashController::new` resolves its staging directory from, called rather
    // than carried on `Shared`: it reads the environment and nothing else, so two callers cannot
    // drift, and a copy on the mirror would be a second answer to keep in step.
    let dir = crate::dropped_dir();
    match portal_swd::staging::stage(&dir, &query.name, &body, bank) {
        Ok(artefact) => {
            shared.push(Request::AdoptDropped {
                id: artefact.id.clone(),
                select: true,
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "id": artefact.id,
                    "label": artefact.label,
                    "region": artefact.region.as_str(),
                    "banner": artefact.banner,
                    "bytes": artefact.bytes,
                    "base": artefact.base,
                    "fits": artefact.fits(),
                    "has_elf": artefact.elf.is_some(),
                })),
            )
        }
        // A refusal is the operator's answer, not an internal fault: 422 rather than 500, and the
        // reason `classify` gave rather than a generic one. It is the whole value of the feature
        // that "this is a no_bootloader build and would never run" arrives now instead of after a
        // flash that verified.
        Err(error @ StageError::Refused(_)) => {
            refused(StatusCode::UNPROCESSABLE_ENTITY, &error.to_string())
        }
        Err(error) => refused(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

#[derive(serde::Deserialize)]
struct ForgetQuery {
    id: String,
}

/// Forget a staged image. The id is checked against the hash shape inside `staging::remove`.
async fn forget_firmware(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<ForgetQuery>,
) -> impl IntoResponse {
    shared.push(Request::ForgetDropped { id: query.id });
    (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
}

fn refused(status: StatusCode, error: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({ "ok": false, "error": error })),
    )
}

async fn provision(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    Json(shared.provision.lock().unwrap().clone())
}

#[derive(serde::Deserialize)]
struct ProvisionHistoryQuery {
    #[serde(default)]
    serial: Option<u32>,
    #[serde(default)]
    q: Option<String>,
}

async fn provision_history(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<ProvisionHistoryQuery>,
) -> impl IntoResponse {
    let provision = shared.provision.lock().unwrap();
    let needle = query.q.as_deref().unwrap_or("").to_ascii_lowercase();
    let actions = provision
        .history
        .iter()
        .filter(|action| {
            query
                .serial
                .is_none_or(|serial| action.serial == Some(serial))
                && (needle.is_empty()
                    || action.action.to_ascii_lowercase().contains(&needle)
                    || action.detail.to_ascii_lowercase().contains(&needle)
                    || action
                        .uid
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&needle))
        })
        .cloned()
        .collect::<Vec<_>>();
    Json(serde_json::json!({ "actions": actions }))
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
                "destructive": plan.is_destructive(),
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

async fn survey(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    Json(shared.state.lock().unwrap().survey.clone())
}

async fn export_survey_json(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    let snapshot = shared.state.lock().unwrap().survey.clone();
    let body = serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".into());
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"home-flag-survey.json\"",
            ),
        ],
        body,
    )
}

async fn export_survey_csv(State(shared): State<Arc<Shared>>) -> impl IntoResponse {
    let snapshot = shared.state.lock().unwrap().survey.clone();
    let mut body = String::from("index,position,offset,crossing,class\n");
    for sample in snapshot.samples {
        body.push_str(&format!(
            "{},{},{},{},{}\n",
            sample.index,
            sample.position,
            sample.offset,
            sample
                .crossing
                .map(|value| value.to_string())
                .unwrap_or_default(),
            serde_json::to_string(&sample.class)
                .unwrap_or_else(|_| "\"failed\"".into())
                .trim_matches('"')
        ));
    }
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"home-flag-survey.csv\"",
            ),
        ],
        body,
    )
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
    /// Required for plans that erase/rewrite device flash.
    #[serde(default)]
    confirm_destructive: bool,
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
        confirm_destructive,
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

    if plan.is_destructive() && !confirm_destructive {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "this plan rewrites application flash; repeat with confirm_destructive=true"
            })),
        )
            .into_response();
    }

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
    EnterDirect,
    ExitDirect,
    Jog {
        axis: bench_core::dut::Axis,
        speed: i32,
    },
    StartSurvey {
        config: SurveyConfig,
    },
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
    SetProvisionSerial {
        serial: u32,
    },
    SetNextSerial {
        serial: u32,
    },
    ReserveSerial {
        #[serde(default)]
        serial: Option<u32>,
        #[serde(default)]
        allow_reassignment: bool,
    },
    KeepOnboardSerial,
    UsePcbSerial {
        serial: u32,
    },
    ReadSettings,
    WriteSettings {
        current_ma: u16,
        recovery: bool,
    },
    ResetMcu,
    CheckBoot,
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
    RawVcom {
        text: String,
        #[serde(default = "default_line_ending")]
        line_ending: LineEnding,
    },
    RawRs485 {
        body: serde_json::Value,
        #[serde(default)]
        target: Option<i8>,
    },
}

fn default_line_ending() -> LineEnding {
    LineEnding::None
}

async fn command(
    State(shared): State<Arc<Shared>>,
    Json(body): Json<CommandBody>,
) -> impl IntoResponse {
    let request = match body {
        CommandBody::EnterDirect => routed(Some(Channel::Serial), Op::EnterDirect),
        CommandBody::ExitDirect => routed(Some(Channel::Serial), Op::ExitDirect),
        CommandBody::Jog { axis, speed } => {
            if !(-14_080..=14_080).contains(&speed) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "speed must be within +/-14080" })),
                )
                    .into_response();
            }
            routed(Some(Channel::Serial), Op::Jog { axis, speed })
        }
        CommandBody::StartSurvey { config } => {
            if let Err(error) = config.sample_count() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": error })),
                )
                    .into_response();
            }
            routed(Some(Channel::Serial), Op::Survey { config })
        }
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
        CommandBody::SetProvisionSerial { serial } => Request::SetProvisionSerial(serial),
        CommandBody::SetNextSerial { serial } => Request::SetNextSerial(serial),
        CommandBody::ReserveSerial {
            serial,
            allow_reassignment,
        } => Request::ReserveSerial {
            serial,
            allow_reassignment,
        },
        CommandBody::KeepOnboardSerial => Request::KeepOnboardSerial,
        CommandBody::UsePcbSerial { serial } => Request::UsePcbSerial(serial),
        CommandBody::ReadSettings => Request::ReadSettings,
        CommandBody::WriteSettings {
            current_ma,
            recovery,
        } => {
            if !(50..=250).contains(&current_ma) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error":"current_ma must be between 50 and 250"})),
                )
                    .into_response();
            }
            Request::WriteSettings {
                current_ma,
                recovery,
            }
        }
        CommandBody::ResetMcu => Request::ResetMcu,
        CommandBody::CheckBoot => Request::CheckBoot,
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
        CommandBody::RawVcom { text, line_ending } => Request::SendRaw {
            channel: Channel::Serial,
            signal: RawSignal::VcomText {
                text,
                ending: line_ending,
            },
        },
        CommandBody::RawRs485 { body, target } => {
            if !body.is_object() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "raw RS485 body must be a JSON object" })),
                )
                    .into_response();
            }
            if let Some(target) = target {
                shared.push(Request::SelectRs485Target(target));
            }
            Request::SendRaw {
                channel: Channel::Rs485,
                signal: RawSignal::Rs485Json { body },
            }
        }
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

    /// The route answers from the worker's mirror, and says so plainly when it has none.
    ///
    /// The distinction is the whole point of the `Option`: "the worker has not scanned yet" and
    /// "nothing is attached" would otherwise render as the same empty list, and the page would
    /// draw "No ST-Link found" over a bench that had not looked.
    #[test]
    fn ports_answers_from_the_mirror_and_says_so_when_it_has_none() {
        assert!(ports_body(&SurveyMirror::default()).is_none());

        let mirror = SurveyMirror {
            survey: Some(bench_core::Survey {
                ports: vec![bench_core::PortEntry {
                    name: "/dev/cu.usbmodem5103".into(),
                    kind: "usb".into(),
                    product: Some("STM32 STLink".into()),
                    serial_number: Some("PROBE123".into()),
                    vid: None,
                    pid: None,
                }],
                probes: vec![bench_core::ProbeEntry {
                    identifier: "0483:374b:PROBE123".into(),
                    name: Some("STLink V2-1".into()),
                    serial_number: Some("PROBE123".into()),
                    kind: "ST-LINK".into(),
                }],
                swd_support: true,
            }),
            generation: 7,
            scanned_at_ms: 1_234,
        };
        let body = ports_body(&mirror).expect("a filled mirror answers");
        // Both halves of one survey, so a port can be matched to the probe that owns it.
        assert_eq!(body["ports"][0]["serial_number"], "PROBE123");
        assert_eq!(body["probes"][0]["identifier"], "0483:374b:PROBE123");
        assert_eq!(body["swd_support"], true);
        assert_eq!(body["generation"], 7);
    }

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

        let body: CommandBody = serde_json::from_value(serde_json::json!({
            "op": "raw_vcom", "text": ":t 246", "line_ending": "crlf"
        }))
        .unwrap();
        assert!(matches!(
            body,
            CommandBody::RawVcom {
                line_ending: LineEnding::Crlf,
                ..
            }
        ));

        let body: CommandBody = serde_json::from_value(serde_json::json!({
            "op": "raw_rs485", "target": 7, "body": { "poll": null }
        }))
        .unwrap();
        assert!(matches!(
            body,
            CommandBody::RawRs485 {
                target: Some(7),
                ..
            }
        ));

        let body: CommandBody =
            serde_json::from_value(serde_json::json!({ "op": "reset_mcu" })).unwrap();
        assert!(matches!(body, CommandBody::ResetMcu));

        let body: CommandBody =
            serde_json::from_value(serde_json::json!({ "op": "check_boot" })).unwrap();
        assert!(matches!(body, CommandBody::CheckBoot));

        let body: CommandBody = serde_json::from_value(serde_json::json!({
            "op": "reserve_serial", "serial": 73001, "allow_reassignment": true
        }))
        .unwrap();
        assert!(matches!(
            body,
            CommandBody::ReserveSerial {
                serial: Some(73_001),
                allow_reassignment: true
            }
        ));

        let body: CommandBody = serde_json::from_value(serde_json::json!({
            "op": "write_settings", "current_ma": 250, "recovery": false
        }))
        .unwrap();
        assert!(matches!(
            body,
            CommandBody::WriteSettings {
                current_ma: 250,
                recovery: false
            }
        ));

        let body: CommandBody = serde_json::from_value(serde_json::json!({
            "op": "start_survey",
            "config": {
                "axis": "b", "mode": "settled", "center": 42,
                "center_is_home": false, "half_range": 500, "step": 10,
                "duty_min": 200, "duty_max": 255
            }
        }))
        .unwrap();
        assert!(matches!(body, CommandBody::StartSurvey { .. }));
    }
}

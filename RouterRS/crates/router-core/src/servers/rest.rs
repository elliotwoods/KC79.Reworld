//! REST server (port of the crow `REST::Server`, default port 8080).
//! All routes are GET, matching the C++ exactly:
//!
//! - `/` -> `"true"` (health)
//! - `/<col>/<portal>/setPosition/<x>,<y>` (rejects |pos| > 1)
//! - `/<col>/<portal>/getPosition` -> `{"x":..,"y":..}` (live position)
//! - `/<col>/<portal>/getTargetPosition` -> `{"x":..,"y":..}`
//! - `/<col>/<portal>/isInPosition` -> `true`/`false`
//! - `/<col>/<portal>/pollPosition` -> `"true"`
//! - `/<col>/<portal>/push` -> `"true"`
//!
//! Missing column/portal or bad arguments -> 500 with a message.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Duration;

use glam::vec2;
use tiny_http::{Method, Response, Server};

use crate::runtime::{Command, Query};

const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

pub fn spawn(
    port: u16,
    commands: Sender<Command>,
    shutdown: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("rest-server".into())
        .spawn(move || run(port, commands, shutdown))
        .expect("spawn rest server")
}

fn run(port: u16, commands: Sender<Command>, shutdown: Arc<AtomicBool>) {
    let server = match Server::http(("0.0.0.0", port)) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("REST server failed to bind port {port}: {e}");
            return;
        }
    };

    while !shutdown.load(Ordering::Acquire) {
        let request = match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(request)) => request,
            Ok(None) => continue,
            Err(_) => continue,
        };
        if *request.method() != Method::Get {
            let _ = request.respond(Response::from_string("GET only").with_status_code(405));
            continue;
        }
        let url = request.url().to_string();
        let (status, body) = handle(&url, &commands);
        let _ = request.respond(Response::from_string(body).with_status_code(status));
    }
}

fn handle(url: &str, commands: &Sender<Command>) -> (u16, String) {
    let path = url.split('?').next().unwrap_or(url);
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if parts.is_empty() {
        return (200, "true".into());
    }
    if parts.len() < 3 {
        return (500, "Malformed route".into());
    }

    let Ok(col) = parts[0].parse::<usize>() else {
        return (500, "Bad column index".into());
    };
    let Ok(portal) = parts[1].parse::<u8>() else {
        return (500, "Bad portal index".into());
    };

    // validate the portal exists (mirrors the C++ 500s)
    if !portal_exists(commands, col, portal) {
        return (500, format!("Column {col} / portal {portal} not found"));
    }

    match parts[2] {
        "setPosition" => {
            let Some(arg) = parts.get(3) else {
                return (500, "Missing position".into());
            };
            let coords: Vec<&str> = arg.split(',').collect();
            if coords.len() != 2 {
                return (500, "Position must be x,y".into());
            }
            let (Ok(x), Ok(y)) = (coords[0].parse::<f32>(), coords[1].parse::<f32>()) else {
                return (500, "Bad position floats".into());
            };
            if x * x + y * y > 1.0 {
                return (500, "Out of range".into());
            }
            let _ = commands.send(Command::SetPilotPosition {
                col,
                portal,
                position: vec2(x, y),
            });
            (200, "true".into())
        }
        "getPosition" => query_position(commands, col, portal, false),
        "getTargetPosition" => query_position(commands, col, portal, true),
        "isInPosition" => {
            let (reply_tx, reply_rx) = channel();
            let _ = commands.send(Command::Query(Query::IsInPosition {
                col,
                portal,
                reply: reply_tx,
            }));
            match reply_rx.recv_timeout(QUERY_TIMEOUT) {
                Ok(Some(value)) => (200, value.to_string()),
                _ => (500, "Query failed".into()),
            }
        }
        "pollPosition" => {
            let _ = commands.send(Command::PollPosition { col, portal });
            (200, "true".into())
        }
        "push" => {
            let _ = commands.send(Command::Push { col, portal });
            (200, "true".into())
        }
        other => (500, format!("Unknown route '{other}'")),
    }
}

fn portal_exists(commands: &Sender<Command>, col: usize, portal: u8) -> bool {
    let (reply_tx, reply_rx) = channel();
    let _ = commands.send(Command::Query(Query::PortalExists {
        col,
        portal,
        reply: reply_tx,
    }));
    reply_rx.recv_timeout(QUERY_TIMEOUT).unwrap_or(false)
}

fn query_position(
    commands: &Sender<Command>,
    col: usize,
    portal: u8,
    target: bool,
) -> (u16, String) {
    let (reply_tx, reply_rx) = channel();
    let query = if target {
        Query::GetTargetPosition { col, portal, reply: reply_tx }
    } else {
        Query::GetPosition { col, portal, reply: reply_tx }
    };
    let _ = commands.send(Command::Query(query));
    match reply_rx.recv_timeout(QUERY_TIMEOUT) {
        Ok(Some(position)) => (
            200,
            format!("{{\"x\":{},\"y\":{}}}", position.x, position.y),
        ),
        _ => (500, "Query failed".into()),
    }
}

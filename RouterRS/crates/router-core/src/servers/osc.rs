//! OSC receiver (port of `OSC::Receiver` + `OSC/Routes.cpp`, default UDP
//! port 4000, receive-only).
//!
//! Fixed routes (case-insensitive): /move, /unwind, /motionProfile,
//! /setCurrent, /homeAndZeroLocal, /disableLights, /axesMoveBlock,
//! /axesMoveByInidices (sic — the misspelling is the real wire address).
//!
//! Any other address is treated as `/<action>`, `/<col>/<action>`, or
//! `/<col>/<portal>/<action>` where `<action>` is a Portal action OSC
//! address (ping, init, calibrate, home, flashLEDs, goHome, seeThrough,
//! disableDebugLights, enableDebugLights, unjam, escapeFromRoutine, reboot).

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use glam::vec2;
use rosc::{OscMessage, OscPacket, OscType};
use router_proto::commands::ActionKind;
use router_proto::value::key;
use router_proto::Value;

use crate::runtime::{Command, Scope};

pub fn spawn(
    port: u16,
    commands: Sender<Command>,
    shutdown: Arc<AtomicBool>,
    message_count: Arc<AtomicUsize>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("osc-receiver".into())
        .spawn(move || run(port, commands, shutdown, message_count))
        .expect("spawn osc receiver")
}

fn run(
    port: u16,
    commands: Sender<Command>,
    shutdown: Arc<AtomicBool>,
    message_count: Arc<AtomicUsize>,
) {
    let socket = match UdpSocket::bind(("0.0.0.0", port)) {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("OSC receiver failed to bind UDP port {port}: {e}");
            return;
        }
    };
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .ok();

    let mut buf = [0u8; 65_536];
    while !shutdown.load(Ordering::Acquire) {
        let size = match socket.recv(&mut buf) {
            Ok(size) => size,
            Err(_) => continue,
        };
        let Ok((_, packet)) = rosc::decoder::decode_udp(&buf[..size]) else {
            continue;
        };
        dispatch_packet(packet, &commands, &message_count);
    }
}

fn dispatch_packet(packet: OscPacket, commands: &Sender<Command>, count: &Arc<AtomicUsize>) {
    match packet {
        OscPacket::Message(message) => {
            count.fetch_add(1, Ordering::Relaxed);
            if let Err(error) = handle_message(&message, commands) {
                eprintln!("OSC {}: {error}", message.addr);
            }
        }
        OscPacket::Bundle(bundle) => {
            for inner in bundle.content {
                dispatch_packet(inner, commands, count);
            }
        }
    }
}

// ---------------------------------------------------------- arg helpers

/// `getArgAsInt` — accepts Int/Long (and truncates floats like ofxOsc).
fn arg_int(message: &OscMessage, index: usize) -> Option<i64> {
    match message.args.get(index)? {
        OscType::Int(v) => Some(*v as i64),
        OscType::Long(v) => Some(*v),
        OscType::Float(v) => Some(*v as i64),
        OscType::Double(v) => Some(*v as i64),
        _ => None,
    }
}

/// `getArgAsFloat` — accepts Float/Double/Int/Long.
fn arg_float(message: &OscMessage, index: usize) -> Option<f32> {
    match message.args.get(index)? {
        OscType::Float(v) => Some(*v),
        OscType::Double(v) => Some(*v as f32),
        OscType::Int(v) => Some(*v as f32),
        OscType::Long(v) => Some(*v as f32),
        _ => None,
    }
}

fn is_integer(s: &str) -> bool {
    // C++: ofToString(ofToInt(s)) == s
    s.parse::<i64>().map(|v| v.to_string() == s).unwrap_or(false)
}

// ------------------------------------------------------------- routing

fn handle_message(message: &OscMessage, commands: &Sender<Command>) -> Result<(), String> {
    let address = message.addr.to_lowercase();
    match address.as_str() {
        "/move" => {
            if message.args.len() < 4 {
                return Ok(()); // C++ silently ignores short /move
            }
            let col = arg_int(message, 0).ok_or("bad col")? as usize;
            let portal = arg_int(message, 1).ok_or("bad portal")? as u8;
            let x = arg_float(message, 2).ok_or("bad x")?;
            let y = arg_float(message, 3).ok_or("bad y")?;
            let _ = commands.send(Command::SetPilotPosition {
                col,
                portal,
                position: vec2(x, y),
            });
            Ok(())
        }
        "/unwind" => {
            let scope = if message.args.is_empty() {
                Scope::All
            } else if message.args.len() < 2 {
                Scope::Column(arg_int(message, 0).ok_or("bad col")? as usize)
            } else {
                Scope::Portal(
                    arg_int(message, 0).ok_or("bad col")? as usize,
                    arg_int(message, 1).ok_or("bad portal")? as u8,
                )
            };
            let _ = commands.send(Command::Unwind(scope));
            Ok(())
        }
        "/motionprofile" => {
            if message.args.is_empty() {
                return Ok(());
            }
            let max_velocity = arg_int(message, 0).ok_or("bad maxVelocity")? as i32;
            let acceleration = arg_int(message, 1).map(|v| v as i32);
            let _ = commands.send(Command::PushMotionProfileAll {
                max_velocity,
                acceleration,
            });
            Ok(())
        }
        "/setcurrent" => {
            if let Some(current) = arg_float(message, 0) {
                let _ = commands.send(Command::SetCurrentAll(current));
            }
            Ok(())
        }
        "/homeandzerolocal" => {
            let _ = commands.send(Command::HomeAndZeroLocal);
            Ok(())
        }
        "/disablelights" => {
            let _ = commands.send(Command::Broadcast {
                body: Value::Map(vec![(key("debugLightsEnabled"), Value::Boolean(false))]),
                collateable: false,
            });
            Ok(())
        }
        "/axesmoveblock" => {
            if message.args.len() < 4 {
                return Err("Not enough arguments".into());
            }
            let col_begin = arg_int(message, 0).ok_or("bad colBegin")? as usize;
            let col_end = arg_int(message, 1).ok_or("bad colEnd")? as usize;
            let portal_begin = arg_int(message, 2).ok_or("bad portalBegin")? as usize;
            let portal_end = arg_int(message, 3).ok_or("bad portalEnd")? as usize;

            let data_size = message.args.len() / 4;
            if data_size < (col_end - col_begin) * (portal_end - portal_begin) {
                return Err("Not enough data in message for columns and portals selected".into());
            }

            let mut offset = 4usize;
            for col in col_begin..col_end {
                for portal_index in portal_begin..portal_end {
                    let a = arg_float(message, offset).ok_or("bad axis value")?;
                    let b = arg_float(message, offset + 1).ok_or("bad axis value")?;
                    offset += 2;
                    let _ = commands.send(Command::SetAxesCyclicByIndex {
                        col,
                        portal_index,
                        axes: vec2(a, b),
                    });
                }
            }
            Ok(())
        }
        // sic: the C++ wire address is misspelled
        "/axesmovebyinidices" => {
            let message_count = message.args.len() / 4;
            let mut offset = 0usize;
            for _ in 0..message_count {
                let col = arg_int(message, offset).ok_or("bad col")? as usize;
                let portal_index = arg_int(message, offset + 1).ok_or("bad portal")? as usize;
                let a = arg_float(message, offset + 2).ok_or("bad axis value")?;
                let b = arg_float(message, offset + 3).ok_or("bad axis value")?;
                offset += 4;
                let _ = commands.send(Command::SetAxesCyclicByIndex {
                    col,
                    portal_index,
                    axes: vec2(a, b),
                });
            }
            Ok(())
        }
        _ => handle_dynamic_route(&message.addr, commands),
    }
}

/// `/[col]/[portal]/<actionAddress>` routing at installation / column /
/// portal scope.
fn handle_dynamic_route(address: &str, commands: &Sender<Command>) -> Result<(), String> {
    let parts: Vec<&str> = address.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [action] => {
            if let Some(action) = ActionKind::by_osc_address(action) {
                let _ = commands.send(Command::PerformAction {
                    scope: Scope::All,
                    action,
                });
            }
            Ok(())
        }
        [first, second, rest @ ..] => {
            let has_col = is_integer(first);
            let has_portal = is_integer(second);
            if !has_col && !has_portal {
                if let Some(action) = ActionKind::by_osc_address(first) {
                    let _ = commands.send(Command::PerformAction {
                        scope: Scope::All,
                        action,
                    });
                }
            } else if has_col && !has_portal {
                let col = first.parse::<usize>().map_err(|e| e.to_string())?;
                if let Some(action) = ActionKind::by_osc_address(second) {
                    let _ = commands.send(Command::PerformAction {
                        scope: Scope::Column(col),
                        action,
                    });
                }
            } else if has_col && has_portal {
                let col = first.parse::<usize>().map_err(|e| e.to_string())?;
                let portal = second.parse::<u8>().map_err(|e| e.to_string())?;
                if let Some(action) = rest.first().and_then(|a| ActionKind::by_osc_address(a)) {
                    let _ = commands.send(Command::PerformAction {
                        scope: Scope::Portal(col, portal),
                        action,
                    });
                }
            }
            Ok(())
        }
        [] => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_detection_matches_cpp() {
        assert!(is_integer("0"));
        assert!(is_integer("12"));
        assert!(is_integer("-3"));
        assert!(!is_integer("01")); // ofToString(1) != "01"
        assert!(!is_integer("home"));
        assert!(!is_integer("1.5"));
    }
}

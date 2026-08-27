//! End-to-end tests: runtime + RS485 worker + firmware simulator + REST/OSC
//! servers, no hardware, no GUI.

use std::io::Read;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use router_core::config::AppConfig;
use router_core::runtime::{self, Command, RuntimeConfig, Scope};
use router_core::sim::SimConfig;
use router_report::Reporter;
use serde_json::json;

fn test_config(rest_port: u16, osc_port: u16) -> AppConfig {
    AppConfig::from_json(json!({
        "Installation": {
            "arrangement": { "Columns": 2, "Rows": 3, "Column width": 1, "Flipped": false },
            "messaging": { "transmit": "Inidividual" },
        },
        "Receiver": { "Enabled": osc_port != 0, "Port": osc_port },
        "Server": { "Enabled": rest_port != 0, "Port": rest_port },
    }))
}

fn spawn_sim_runtime(rest_port: u16, osc_port: u16) -> runtime::RuntimeHandle {
    runtime::spawn(RuntimeConfig {
        app_config: test_config(rest_port, osc_port),
        config_path: None,
        simulate: Some(SimConfig {
            latency: Duration::from_millis(1),
            jitter: Duration::from_millis(1),
            ..Default::default()
        }),
        reporter: Reporter::disabled(),
    })
}

fn wait_until(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn http_get(port: u16, path: &str) -> (u16, String) {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    use std::io::Write;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let status: u16 = response
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    (status, body)
}

#[test]
fn set_position_flows_to_simulated_hardware() {
    let handle = spawn_sim_runtime(0, 0);

    assert!(
        wait_until(Duration::from_secs(5), || {
            {
                let s = handle.snapshot();
                !s.columns.is_empty() && s.columns.iter().all(|c| c.stats.connected)
            }
        }),
        "columns connect to simulator"
    );

    // set a position on column 0, portal 2 -> Individual transmit pushes it
    handle.send(Command::SetPilotPosition {
        col: 0,
        portal: 2,
        position: glam::vec2(0.5, 0.0),
    });

    // hardware should reach the target (simulator integrates motion; we poll
    // the position like the C++ app does — live state only updates on rx).
    // The wait also requires the reported position to be near the commanded
    // one: at startup firmware live == firmware target == local target (all
    // defaults), so `in_target_position` alone is trivially true before the
    // move has even been transmitted.
    assert!(
        wait_until(Duration::from_secs(10), || {
            handle.send(Command::PollPosition { col: 0, portal: 2 });
            let snap = handle.snapshot();
            let portal = &snap.columns[0].portals[1];
            portal.in_target_position
                && portal
                    .live_position
                    .is_some_and(|live| (live - glam::vec2(0.5, 0.0)).length() < 0.01)
        }),
        "portal reaches target position"
    );

    let snap = handle.snapshot();
    let portal = &snap.columns[0].portals[1];
    let live = portal.live_position.unwrap();
    assert!(
        (live.x - 0.5).abs() < 0.01 && live.y.abs() < 0.01,
        "live position near (0.5, 0): {live:?}"
    );

    handle.shutdown();
}

#[test]
fn poll_populates_reported_state() {
    let handle = spawn_sim_runtime(0, 0);
    assert!(wait_until(Duration::from_secs(5), || {
        {
            let s = handle.snapshot();
            !s.columns.is_empty() && s.columns.iter().all(|c| c.stats.connected)
        }
    }));

    handle.send(Command::Poll(Scope::All));

    assert!(
        wait_until(Duration::from_secs(5), || {
            let snap = handle.snapshot();
            snap.columns
                .iter()
                .all(|c| c.portals.iter().all(|p| p.version.is_some()))
        }),
        "all portals report firmware version after poll"
    );
    let snap = handle.snapshot();
    assert_eq!(
        snap.columns[0].portals[0].version.as_deref(),
        Some("sim-1.0")
    );

    handle.shutdown();
}

#[test]
fn rest_endpoints_match_cpp_contract() {
    let rest_port = 18_080;
    let handle = spawn_sim_runtime(rest_port, 0);
    assert!(wait_until(Duration::from_secs(5), || {
        {
            let s = handle.snapshot();
            !s.columns.is_empty() && s.columns.iter().all(|c| c.stats.connected)
        }
    }));

    // health
    let (status, body) = http_get(rest_port, "/");
    assert_eq!((status, body.as_str()), (200, "true"));

    // setPosition ok
    let (status, _) = http_get(rest_port, "/0/1/setPosition/0.5,0.0");
    assert_eq!(status, 200);

    // out of range -> 500
    let (status, _) = http_get(rest_port, "/0/1/setPosition/2.0,0.0");
    assert_eq!(status, 500);

    // missing portal -> 500
    let (status, _) = http_get(rest_port, "/0/99/getPosition");
    assert_eq!(status, 500);

    // wait for the portal to arrive: poll position (REST route), then check
    assert!(wait_until(Duration::from_secs(10), || {
        let _ = http_get(rest_port, "/0/1/pollPosition");
        std::thread::sleep(Duration::from_millis(50));
        let (_, body) = http_get(rest_port, "/0/1/isInPosition");
        body == "true"
    }));
    let (status, body) = http_get(rest_port, "/0/1/getPosition");
    assert_eq!(status, 200);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json body");
    assert!((parsed["x"].as_f64().unwrap() - 0.5).abs() < 0.01);

    handle.shutdown();
}

#[test]
fn osc_move_and_action_routes() {
    let osc_port = 14_000;
    let handle = spawn_sim_runtime(0, osc_port);
    assert!(wait_until(Duration::from_secs(5), || {
        {
            let s = handle.snapshot();
            !s.columns.is_empty() && s.columns.iter().all(|c| c.stats.connected)
        }
    }));

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let send = |addr: &str, args: Vec<rosc::OscType>| {
        let packet = rosc::OscPacket::Message(rosc::OscMessage {
            addr: addr.to_string(),
            args,
        });
        let bytes = rosc::encoder::encode(&packet).unwrap();
        socket.send_to(&bytes, ("127.0.0.1", osc_port)).unwrap();
    };

    // /move col portal x y
    send(
        "/move",
        vec![
            rosc::OscType::Int(1),
            rosc::OscType::Int(3),
            rosc::OscType::Float(0.0),
            rosc::OscType::Float(-0.5),
        ],
    );
    assert!(
        wait_until(Duration::from_secs(5), || {
            let snap = handle.snapshot();
            let portal = &snap.columns[1].portals[2];
            (portal.position.y + 0.5).abs() < 0.01
        }),
        "/move sets pilot position"
    );

    // dynamic action route: /0/1/seeThrough sets local axes (0.5, 0)
    send("/0/1/seeThrough", vec![]);
    assert!(
        wait_until(Duration::from_secs(5), || {
            let snap = handle.snapshot();
            let portal = &snap.columns[0].portals[0];
            (portal.axes.x - 0.5).abs() < 0.01 && portal.axes.y.abs() < 0.01
        }),
        "/0/1/seeThrough applies the local action effect"
    );

    handle.shutdown();
}

#[test]
fn dead_portal_times_out_and_healthy_ones_answer() {
    let handle = runtime::spawn(RuntimeConfig {
        app_config: test_config(0, 0),
        config_path: None,
        simulate: Some(SimConfig {
            latency: Duration::from_millis(1),
            dead_portals: vec![2],
            ..Default::default()
        }),
        reporter: Reporter::disabled(),
    });
    assert!(wait_until(Duration::from_secs(5), || {
        {
            let s = handle.snapshot();
            !s.columns.is_empty() && s.columns.iter().all(|c| c.stats.connected)
        }
    }));

    handle.send(Command::Poll(Scope::Column(0)));

    // healthy portals answer; the dead one accumulates an ACK timeout
    assert!(
        wait_until(Duration::from_secs(10), || {
            let snap = handle.snapshot();
            let col = &snap.columns[0];
            let healthy_answered =
                col.portals[0].version.is_some() && col.portals[2].version.is_some();
            healthy_answered && col.stats.ack_timeouts >= 1
        }),
        "dead portal 2 times out, portals 1 and 3 answer"
    );
    let snap = handle.snapshot();
    assert!(
        snap.columns[0].portals[1].version.is_none(),
        "dead portal has no reported state"
    );

    handle.shutdown();
}

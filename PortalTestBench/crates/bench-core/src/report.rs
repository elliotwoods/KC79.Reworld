//! The session record: `bench/1`, an additive profile of RouterRS's report schema v1.
//!
//! Same envelope (`v`, `ts`, `seq`, `type`), same file naming
//! (`session-<UTCSTAMP>.ndjson` + `.summary.json`), same writer thread, same monotonic
//! sequence. Bench-specific event types are layered on top rather than replacing anything, so
//! when the RS485 transport is in use the bus events (`ack_timeout`, `bus_stats`, `crc_error`)
//! and the bench events land **in one file, in one sequence** — instead of two logs that have
//! to be correlated by timestamp afterwards.
//!
//! The line that matters is `verdict`: it is deliberately self-contained, so
//! `jq -c 'select(.type=="verdict")' reports/session-*.ndjson` answers "did it pass, and why"
//! without reading anything else.

use std::path::{Path, PathBuf};

use router_report::{ReportConfig, Reporter, ReporterHandle, SessionInfo};

use crate::bench::Outcome;
use crate::state::DutState;

/// Writes the session file, or does nothing.
///
/// The disabled form exists so tests and one-off commands exercise the same code paths without
/// producing session files nobody asked for.
pub struct Report {
    reporter: Reporter,
    handle: Option<ReporterHandle>,
    path: Option<PathBuf>,
}

impl Report {
    /// A report that writes nothing.
    pub fn disabled() -> Self {
        Self { reporter: Reporter::disabled(), handle: None, path: None }
    }

    /// Open a session file in `dir`.
    pub fn open(dir: &Path, app_version: &str) -> std::io::Result<Self> {
        let config = ReportConfig { dir: dir.to_path_buf(), ..Default::default() };
        let session = SessionInfo {
            app_version: app_version.to_string(),
            host: hostname(),
            config: serde_json::json!({ "profile": crate::REPORT_PROFILE }),
        };
        let (reporter, handle) = Reporter::start(config, session)?;
        let mut report = Self { reporter, handle: Some(handle), path: None };
        report.line(serde_json::json!({
            "type": "bench_session_start",
            "profile": crate::REPORT_PROFILE,
            "app_version": app_version,
        }));
        Ok(report)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Flush and close, returning the file that was written.
    pub fn finish(&mut self) -> Option<PathBuf> {
        let path = self.handle.take().and_then(|handle| handle.shutdown());
        if path.is_some() {
            self.path = path.clone();
        }
        path
    }

    fn line(&mut self, payload: serde_json::Value) {
        self.reporter.emit_line(payload);
    }

    // --- the bench vocabulary ------------------------------------------------------------

    pub fn device_connect(&mut self, transport: &str, endpoint: &str, ok: bool, error: Option<&str>) {
        self.line(serde_json::json!({
            "type": "device_connect",
            "transport": transport,
            "endpoint": endpoint,
            "ok": ok,
            "error": error,
        }));
    }

    /// Which module this is, and — critically — which firmware image was on it.
    ///
    /// "Which binary was on the board" has to be answerable from the report alone, months
    /// later. Without it a measurement is a number with no provenance.
    pub fn dut_identity(&mut self, dut: &DutState) {
        self.line(serde_json::json!({
            "type": "dut_identity",
            "firmware": format!("{:?}", dut.firmware).to_lowercase(),
            "version": dut.version,
            "ratio": format!("{:?}", dut.ratio),
            "usteps_per_rev": dut.usteps_per_rev,
            "banner": dut.banner,
        }));
    }

    pub fn plan_start(&mut self, run_id: &str, plan: &crate::plan::Plan, origin: &str) {
        self.line(serde_json::json!({
            "type": "plan_start",
            "run_id": run_id,
            "plan": plan.name,
            "kind": format!("{:?}", plan.kind).to_lowercase(),
            "origin": origin,
            "limits": {
                "max_duration_s": plan.limits.max_duration_s,
                "stall_guard_usteps_per_s": plan.limits.stall_guard_usteps_per_s,
            },
            // The plan is recorded in full: a verdict whose criteria nobody can see afterwards
            // is not evidence of anything.
            "criteria": plan.criteria,
        }));
    }

    pub fn firmware_log(&mut self, level: u8, message: &str, firmware_ms: Option<u64>) {
        self.line(serde_json::json!({
            "type": "portal_log",
            "level": level,
            "message": message,
            "fw_ts_ms": firmware_ms,
        }));
    }

    pub fn log(&mut self, level: u8, source: &str, message: &str) {
        self.line(serde_json::json!({
            "type": "bench_log",
            "level": level,
            "source": source,
            "message": message,
        }));
    }

    pub fn token(&mut self, kind: char, fields: &[String]) {
        self.line(serde_json::json!({
            "type": "bench_token",
            "kind": kind.to_string(),
            "fields": fields,
        }));
    }

    pub fn fault(&mut self, detail: &str) {
        self.line(serde_json::json!({ "type": "bench_fault", "detail": detail }));
    }

    /// The self-contained answer.
    pub fn verdict(&mut self, outcome: &Outcome) {
        self.line(serde_json::json!({
            "type": "verdict",
            "run_id": outcome.run_id,
            "plan": outcome.plan,
            "origin": outcome.origin.name(),
            "verdict": outcome.verdict.name(),
            "reason": outcome.verdict.reason(),
            "duration_ms": outcome.duration_ms,
            "measurements": outcome.measurements,
        }));
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::{MeasureValue, Measurement, Verdict};

    /// The whole point of the `verdict` line: one record answers "did it pass, and why",
    /// without the reader needing anything else from the file.
    #[test]
    fn a_verdict_line_is_self_contained() {
        let outcome = Outcome {
            run_id: "r-0001".into(),
            plan: "startup".into(),
            origin: crate::bench::Origin::Cli,
            verdict: Verdict::Fail {
                criterion: "duration_s".into(),
                got: "300".into(),
                expected: "must be at most 240".into(),
            },
            measurements: vec![Measurement {
                name: "duration_s".into(),
                value: MeasureValue::Number(300.0),
                unit: Some("s".into()),
                at_ms: 300_000,
                step_index: 2,
            }],
            duration_ms: 300_000,
            report_path: None,
        };

        // Build the same payload the writer would stamp.
        let payload = serde_json::json!({
            "type": "verdict",
            "run_id": outcome.run_id,
            "plan": outcome.plan,
            "origin": outcome.origin.name(),
            "verdict": outcome.verdict.name(),
            "reason": outcome.verdict.reason(),
            "duration_ms": outcome.duration_ms,
            "measurements": outcome.measurements,
        });

        assert_eq!(payload["verdict"], "fail");
        assert_eq!(payload["plan"], "startup");
        assert_eq!(payload["origin"], "cli");
        assert!(payload["reason"].as_str().unwrap().contains("duration_s"));
        assert_eq!(payload["measurements"][0]["name"], "duration_s");
    }

    /// A disabled report must be usable everywhere the real one is, so tests and one-off
    /// commands take the same code path rather than a special case.
    #[test]
    fn a_disabled_report_accepts_everything_and_writes_nothing() {
        let mut report = Report::disabled();
        report.device_connect("vcp", "COM3", true, None);
        report.firmware_log(20, "[E Routines.init] Fail", None);
        report.fault("short status record");
        assert!(report.path().is_none());
        assert!(report.finish().is_none());
    }
}

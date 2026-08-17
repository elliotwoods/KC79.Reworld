//! Per-portal health scoring and state machine.
//!
//! Score components (weights): ACK rate 40, latency 15, firmware error-log
//! rate 20, silence-while-polled 15, calibration flags 10. Evaluated on each
//! stats tick over a sliding ~60 s window; transitions need 2 consecutive
//! ticks (hysteresis).

use crate::snapshot::PortalState;

pub const WINDOW_BUCKETS: usize = 6; // 6 x 10 s = 60 s window

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowBucket {
    pub ack_needing_sends: u32,
    pub acks: u32,
    pub timeouts: u32,
    pub error_logs: u32,
    pub latency_p90_ms: f32,
}

#[derive(Debug, Clone)]
pub struct PortalHealth {
    pub window: [WindowBucket; WINDOW_BUCKETS],
    pub state: PortalState,
    pending_state: PortalState,
    pending_ticks: u8,
    pub score: u8,
}

impl Default for PortalHealth {
    fn default() -> Self {
        Self {
            window: [WindowBucket::default(); WINDOW_BUCKETS],
            state: PortalState::Unknown,
            pending_state: PortalState::Unknown,
            pending_ticks: 0,
            score: 0,
        }
    }
}

pub struct HealthInputs {
    /// ms since the portal last sent us anything (None = never seen).
    pub last_seen_age_ms: Option<u64>,
    /// Response window used for latency scoring (ms).
    pub response_window_ms: f32,
    /// Any calibration flag reported false on either axis? None = unreported.
    pub calibration_bad: Option<bool>,
    /// Effective poll interval (ms) for silence saturation; the silence
    /// component saturates at 3x this.
    pub poll_interval_ms: u64,
}

pub struct TickOutcome {
    pub score: u8,
    pub state: PortalState,
    /// Some((from, to, reason)) when a transition fired this tick.
    pub transition: Option<(PortalState, PortalState, String)>,
}

impl PortalHealth {
    /// Push the just-completed bucket and re-evaluate. Call once per stats
    /// tick (10 s).
    pub fn tick(&mut self, bucket: WindowBucket, inputs: &HealthInputs) -> TickOutcome {
        self.window.rotate_right(1);
        self.window[0] = bucket;

        let sends: u32 = self.window.iter().map(|b| b.ack_needing_sends).sum();
        let acks: u32 = self.window.iter().map(|b| b.acks).sum();
        let error_logs: u32 = self.window.iter().map(|b| b.error_logs).sum();
        let latency_p90 = self
            .window
            .iter()
            .map(|b| b.latency_p90_ms)
            .fold(0.0f32, f32::max);

        let never_seen = inputs.last_seen_age_ms.is_none();
        let being_polled = sends > 0;

        // --- components, each 0..1 (1 = healthy) ---
        let ack_component = if sends > 0 {
            acks as f32 / sends as f32
        } else {
            1.0
        };

        let latency_component = if acks > 0 {
            // linear penalty above 50% of the response window
            let half = inputs.response_window_ms * 0.5;
            if latency_p90 <= half {
                1.0
            } else {
                (1.0 - (latency_p90 - half) / half).max(0.0)
            }
        } else {
            1.0
        };

        // log-scaled: 0 errors -> 1.0; ~10/min -> 0.5; >=100/min -> 0.0
        let errors_per_min = error_logs as f32; // window is one minute
        let log_component = (1.0 - (errors_per_min + 1.0).log10() / 2.0).clamp(0.0, 1.0);

        let silence_saturation_ms = (inputs.poll_interval_ms * 3).max(1) as f32;
        let silence_component = match inputs.last_seen_age_ms {
            Some(age) if being_polled => (1.0 - age as f32 / silence_saturation_ms).clamp(0.0, 1.0),
            // polled but never seen at all: fully silent once a whole window
            // of ack-needing sends went unanswered
            None if being_polled && acks == 0 && sends >= 3 => 0.0,
            _ => 1.0,
        };

        let flags_component = match inputs.calibration_bad {
            Some(true) => 0.0,
            _ => 1.0,
        };

        let score = (100.0
            * (0.40 * ack_component
                + 0.15 * latency_component
                + 0.20 * log_component
                + 0.15 * silence_component
                + 0.10 * flags_component))
            .round()
            .clamp(0.0, 100.0) as u8;
        self.score = score;

        // --- state ---
        let target_state = if never_seen && !being_polled {
            PortalState::Unknown
        } else if being_polled && silence_component <= 0.0 {
            PortalState::Silent
        } else if score >= 80 {
            PortalState::Ok
        } else if score >= 50 {
            PortalState::Degraded
        } else {
            PortalState::Faulty
        };

        // hysteresis: 2 consecutive ticks
        let mut transition = None;
        if target_state != self.state {
            if target_state == self.pending_state {
                self.pending_ticks += 1;
            } else {
                self.pending_state = target_state;
                self.pending_ticks = 1;
            }
            if self.pending_ticks >= 2 || self.state == PortalState::Unknown {
                let reason = match target_state {
                    PortalState::Silent => "no response while polled".to_string(),
                    PortalState::Faulty | PortalState::Degraded => format!(
                        "score {score}: ack {:.0}%, {} error logs/min",
                        ack_component * 100.0,
                        error_logs
                    ),
                    PortalState::Ok => "recovered".to_string(),
                    PortalState::Unknown => "never seen".to_string(),
                };
                transition = Some((self.state, target_state, reason));
                self.state = target_state;
                self.pending_ticks = 0;
            }
        } else {
            self.pending_state = target_state;
            self.pending_ticks = 0;
        }

        TickOutcome {
            score,
            state: self.state,
            transition,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(last_seen: Option<u64>) -> HealthInputs {
        HealthInputs {
            last_seen_age_ms: last_seen,
            response_window_ms: 300.0,
            calibration_bad: None,
            poll_interval_ms: 10_000,
        }
    }

    fn good_bucket() -> WindowBucket {
        WindowBucket {
            ack_needing_sends: 100,
            acks: 100,
            timeouts: 0,
            error_logs: 0,
            latency_p90_ms: 20.0,
        }
    }

    #[test]
    fn healthy_portal_scores_high() {
        let mut health = PortalHealth::default();
        let outcome = health.tick(good_bucket(), &inputs(Some(100)));
        assert!(outcome.score >= 95, "score {}", outcome.score);
        assert_eq!(outcome.state, PortalState::Ok);
    }

    #[test]
    fn dead_portal_goes_silent_with_hysteresis() {
        let mut health = PortalHealth::default();
        health.tick(good_bucket(), &inputs(Some(100)));
        assert_eq!(health.state, PortalState::Ok);

        let dead = WindowBucket {
            ack_needing_sends: 50,
            acks: 0,
            timeouts: 50,
            ..Default::default()
        };
        // first bad tick: no transition yet (hysteresis)
        let o1 = health.tick(dead, &inputs(Some(31_000)));
        assert_eq!(o1.state, PortalState::Ok);
        assert!(o1.transition.is_none());
        // second consecutive bad tick: transition fires
        let o2 = health.tick(dead, &inputs(Some(41_000)));
        assert_eq!(o2.state, PortalState::Silent);
        let (from, to, _) = o2.transition.unwrap();
        assert_eq!(from, PortalState::Ok);
        assert_eq!(to, PortalState::Silent);
    }

    #[test]
    fn never_seen_is_unknown() {
        let mut health = PortalHealth::default();
        let outcome = health.tick(WindowBucket::default(), &inputs(None));
        assert_eq!(outcome.state, PortalState::Unknown);
    }

    #[test]
    fn error_log_spam_degrades() {
        let mut health = PortalHealth::default();
        let noisy = WindowBucket {
            ack_needing_sends: 100,
            acks: 100,
            error_logs: 60,
            latency_p90_ms: 10.0,
            ..Default::default()
        };
        health.tick(noisy, &inputs(Some(100)));
        let outcome = health.tick(noisy, &inputs(Some(100)));
        assert!(outcome.score < 95);
    }
}

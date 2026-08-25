//! The rig's state machine: pure, clock-injected, and the only place the flashing *policy*
//! lives.
//!
//! It performs no I/O and holds no probe. Everything it decides is a function of the inputs it
//! has been handed and the clock value it was handed with them, so a hand-plug that bounces
//! seven times in 200 ms is a unit test rather than a bench session.
//!
//! # Sequence, not identity
//!
//! The behaviour this implements was originally specified around the STM32's 96-bit UID: look
//! the UID up in a log, flash it if it is new, run-check it if it was already flashed. We do not
//! track UIDs. The same behaviour falls out of **alternation** instead — the insertion after a
//! successful flash is a run-check, the insertion after that is a flash — because the operator's
//! physical rhythm (seat, hear a tone, cycle it, hear a tone, move on) *is* the sequence. A
//! failure returns the rig to expecting a flash, so reinserting a failed board re-flashes it
//! rather than run-checking it, which is what the original specification asked for.
//!
//! What this trades away is honest and worth stating: the rig cannot tell that the operator
//! swapped in a *different* board between the flash and the run-check. It would run-check the
//! new board, pass it, and neither board would be flagged. Reading the UID at each pass and
//! comparing would catch it; that is the first thing to add if the failure ever shows up.
//!
//! # The three properties the tests pin
//!
//! 1. **No partial resume.** `Busy` is only ever entered from a completed debounce, and the pass
//!    it begins always erases and programs everything. There is no edge that continues a
//!    previous write.
//! 2. **Removal is required.** The only way back to a state that can start a pass is through
//!    [`Phase::AwaitRemoval`] with `removal_quiet_ms` of *continuous* absence. A single insertion
//!    can never produce two passes.
//! 3. **Losing the UI disarms.** The page re-asserts a heartbeat; if it goes stale the rig
//!    disarms — but never mid-write, because aborting a write is worse than finishing one.

use core::fmt;

/// Milliseconds on a monotonic clock. Not `Instant`, so tests can name an instant.
pub type Millis = u64;

/// Which of the two passes an insertion gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Erase and program the whole device, then verify by readback.
    Flash,
    /// Confirm, without halting or resetting, that the application is actually executing.
    RunCheck,
}

/// What a successful flash insertion means to the owning application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sequence {
    /// Standalone flasher workflow: flash, power-cycle the same board, then run-check it.
    FlashThenRunCheck,
    /// Production bench workflow: boot/application verification is part of the flash pass, so
    /// every new insertion is another board to flash.
    FlashEveryInsertion,
}

impl fmt::Display for Pass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pass::Flash => f.write_str("flash"),
            Pass::RunCheck => f.write_str("run-check"),
        }
    }
}

/// What the rig is doing, for the operator's benefit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Disarmed,
    /// Armed and polling for a target.
    Idle,
    /// A target has answered at least once; waiting for enough consecutive answers to commit.
    Debouncing,
    /// A pass is running. The probe belongs to the pass, not to the poll loop.
    Flashing,
    /// A pass is running. See [`Phase::Flashing`]; separated because the operator cue differs.
    RunChecking,
    /// A pass ended. Nothing will start until the fixture has been continuously empty.
    AwaitRemoval,
    /// The probe itself is gone. Not the same as "no target".
    ProbeLost,
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Phase::Disarmed => "disarmed",
            Phase::Idle => "idle",
            Phase::Debouncing => "debouncing",
            Phase::Flashing => "flashing",
            Phase::RunChecking => "run-check",
            Phase::AwaitRemoval => "await-removal",
            Phase::ProbeLost => "probe-lost",
        })
    }
}

/// Everything that can move the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// The clock advanced with nothing else to report. Drives the dead-man check.
    Tick,
    /// A poll saw a target on the wire.
    PollPresent,
    /// A poll saw nothing. Contact bounce and an empty fixture look identical here, which is
    /// exactly why the debounce and the removal gate both exist.
    PollAbsent,
    /// The probe stopped answering: USB dropout, driver unbind, unplugged.
    ProbeError,
    /// The probe was re-opened successfully.
    ProbeRecovered,
    /// A pass finished, one way or the other.
    PassDone {
        pass: Pass,
        ok: bool,
    },
    /// The page is still there. Expected roughly once a second.
    Heartbeat,
    Arm,
    Disarm,
}

/// What the operator hears. The worker maps these onto the framework's system sounds; the
/// machine deliberately does not know about WAV files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    /// Arming. Doubles as an audio-channel test: the operator learns the sound works *before* a
    /// board is in the fixture, rather than after one has been flashed in silence.
    Armed,
    Disarmed,
    /// A held, looping level for the whole of a pass. Its *absence* is the signal that matters —
    /// a lift mid-write is the worst failure this rig has.
    Busy,
    /// "Flashed — now cycle it." Deliberately distinct from [`Cue::Pass`].
    FlashedCycleIt,
    /// The final tone: this board is done.
    Pass,
    Fail,
    /// The removal registered; the next board can go in.
    Rearmed,
}

/// What the worker should do about a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Hand the probe to a pass. The worker does not poll again until it reports `PassDone`.
    BeginPass(Pass),
    Sound(Cue),
    /// How often to poll from here. Idle is cheap; debouncing and waiting for removal are not,
    /// because both are measuring a human hand.
    SetPollPeriod(Millis),
}

/// Timing constants, all in milliseconds.
///
/// The defaults resolve an inconsistency in the original specification, which asked for a poll
/// at "~3 Hz" *and* a debounce of "3 polls / 250 ms" — 3 polls in 250 ms is 12 Hz. Both are
/// satisfiable if the rate depends on what the rig is doing: idling is cheap and 3 Hz is plenty,
/// while debouncing and waiting for removal are timing a hand and want the finer grain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// Poll period while armed and waiting. 3 Hz.
    pub idle_poll_ms: Millis,
    /// Poll period while debouncing or waiting for removal. 12.5 Hz.
    pub active_poll_ms: Millis,
    /// Consecutive present polls needed to commit. At `active_poll_ms` this is the 250 ms the
    /// specification asked for.
    pub debounce_polls: u8,
    /// A streak taking longer than this is a sick probe rather than a hand, so start over.
    /// Cannot be reached by a bouncing contact — any absent poll resets the streak outright.
    pub debounce_max_span_ms: Millis,
    /// Continuous absence required before the rig will start another pass.
    pub removal_quiet_ms: Millis,
    /// How long the rig will run without hearing from a UI before disarming itself.
    pub heartbeat_stale_ms: Millis,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            idle_poll_ms: 333,
            active_poll_ms: 80,
            debounce_polls: 3,
            debounce_max_span_ms: 1_500,
            removal_quiet_ms: 500,
            heartbeat_stale_ms: 3_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Disarmed,
    Idle {
        expect: Pass,
    },
    Debouncing {
        expect: Pass,
        streak: u8,
        first_seen: Millis,
    },
    Busy {
        pass: Pass,
    },
    AwaitRemoval {
        expect_next: Pass,
        /// `None` until a poll has actually reported absence. Any present poll clears it again,
        /// so the gate measures *continuous* absence rather than elapsed time.
        quiet_since: Option<Millis>,
    },
    ProbeLost {
        resume: Pass,
    },
}

/// The rig's policy. See the module documentation.
#[derive(Clone, Debug)]
pub struct Machine {
    timing: Timing,
    sequence: Sequence,
    state: State,
    last_heartbeat: Millis,
    /// Set when a disarm is asked for during a pass. Aborting a write to honour a button is
    /// worse than finishing it, so the request is deferred to the end of the pass.
    disarm_pending: bool,
}

impl Machine {
    pub fn new(timing: Timing) -> Self {
        Self::with_sequence(timing, Sequence::FlashThenRunCheck)
    }

    pub fn with_sequence(timing: Timing, sequence: Sequence) -> Self {
        Self {
            timing,
            sequence,
            state: State::Disarmed,
            last_heartbeat: 0,
            disarm_pending: false,
        }
    }

    pub fn timing(&self) -> &Timing {
        &self.timing
    }

    pub fn phase(&self) -> Phase {
        match self.state {
            State::Disarmed => Phase::Disarmed,
            State::Idle { .. } => Phase::Idle,
            State::Debouncing { .. } => Phase::Debouncing,
            State::Busy { pass: Pass::Flash } => Phase::Flashing,
            State::Busy {
                pass: Pass::RunCheck,
            } => Phase::RunChecking,
            State::AwaitRemoval { .. } => Phase::AwaitRemoval,
            State::ProbeLost { .. } => Phase::ProbeLost,
        }
    }

    /// The pass the next insertion would get. `None` while one is already running.
    pub fn expect(&self) -> Option<Pass> {
        match self.state {
            State::Idle { expect }
            | State::Debouncing { expect, .. }
            | State::ProbeLost { resume: expect } => Some(expect),
            State::AwaitRemoval { expect_next, .. } => Some(expect_next),
            State::Disarmed | State::Busy { .. } => None,
        }
    }

    pub fn armed(&self) -> bool {
        !matches!(self.state, State::Disarmed)
    }

    /// True while the probe belongs to a pass and the worker must not poll.
    pub fn pass_in_flight(&self) -> bool {
        matches!(self.state, State::Busy { .. })
    }

    /// The pass currently running, if one is.
    pub fn phase_pass(&self) -> Option<Pass> {
        match self.state {
            State::Busy { pass } => Some(pass),
            _ => None,
        }
    }

    /// Whether a disarm has been asked for and is waiting for the current pass to end.
    pub fn disarm_pending(&self) -> bool {
        self.disarm_pending
    }

    pub fn step(&mut self, now: Millis, input: Input) -> Vec<Action> {
        // Refresh before the staleness check, so a heartbeat arriving exactly on the deadline
        // keeps the rig armed rather than racing it.
        if input == Input::Heartbeat {
            self.last_heartbeat = now;
        }
        if input == Input::Arm {
            // The command came from a live page by definition.
            self.last_heartbeat = now;
        }

        let mut actions = Vec::new();

        // The dead-man. Checked on every input rather than only on `Tick`, so a rig being polled
        // but not rendered still notices.
        if self.armed() && now.saturating_sub(self.last_heartbeat) > self.timing.heartbeat_stale_ms
        {
            self.disarm_pending = true;
        }

        match input {
            Input::Arm => {
                if matches!(self.state, State::Disarmed) {
                    self.disarm_pending = false;
                    actions.push(Action::Sound(Cue::Armed));
                    // Not straight to Idle: a board already sitting in the fixture when the
                    // operator arms would otherwise be debounced and flashed immediately. The
                    // rig has to see an empty fixture once before it will do anything.
                    self.enter_await_removal(Pass::Flash, &mut actions);
                }
            }
            Input::Disarm => {
                self.disarm_pending = true;
            }
            Input::Heartbeat | Input::Tick => {}

            Input::ProbeError => match self.state {
                State::Disarmed => {}
                State::Busy { pass } => {
                    // The pass is gone with the probe. Fail it here rather than waiting for a
                    // `PassDone` that a dead worker may never send.
                    self.finish_pass(pass, false, &mut actions);
                }
                _ => {
                    let resume = self.expect().unwrap_or(Pass::Flash);
                    self.state = State::ProbeLost { resume };
                }
            },

            Input::ProbeRecovered => {
                if let State::ProbeLost { resume } = self.state {
                    // Conservatively require a removal cycle even from idle. After a USB dropout
                    // there is no way to know what the SWD lines did, or whether the board in
                    // the fixture is the one that was there before.
                    self.enter_await_removal(resume, &mut actions);
                }
            }

            Input::PollPresent => match self.state {
                State::Idle { expect } => {
                    self.state = State::Debouncing {
                        expect,
                        streak: 1,
                        first_seen: now,
                    };
                    actions.push(Action::SetPollPeriod(self.timing.active_poll_ms));
                }
                State::Debouncing {
                    expect,
                    streak,
                    first_seen,
                } => {
                    if now.saturating_sub(first_seen) > self.timing.debounce_max_span_ms {
                        self.enter_idle(expect, &mut actions);
                    } else {
                        let streak = streak.saturating_add(1);
                        if streak >= self.timing.debounce_polls {
                            self.state = State::Busy { pass: expect };
                            actions.push(Action::Sound(Cue::Busy));
                            actions.push(Action::BeginPass(expect));
                        } else {
                            self.state = State::Debouncing {
                                expect,
                                streak,
                                first_seen,
                            };
                        }
                    }
                }
                State::AwaitRemoval { expect_next, .. } => {
                    // Still in the fixture. Restart the gate from scratch.
                    self.state = State::AwaitRemoval {
                        expect_next,
                        quiet_since: None,
                    };
                }
                State::Disarmed | State::Busy { .. } | State::ProbeLost { .. } => {}
            },

            Input::PollAbsent => match self.state {
                State::Debouncing { expect, .. } => {
                    // Strictly consecutive. The streak is discarded, not decremented — a contact
                    // that is making and breaking has not been seated, however many times it
                    // answered.
                    self.enter_idle(expect, &mut actions);
                }
                State::AwaitRemoval {
                    expect_next,
                    quiet_since,
                } => {
                    let since = quiet_since.unwrap_or(now);
                    if now.saturating_sub(since) >= self.timing.removal_quiet_ms {
                        actions.push(Action::Sound(Cue::Rearmed));
                        self.enter_idle(expect_next, &mut actions);
                    } else {
                        self.state = State::AwaitRemoval {
                            expect_next,
                            quiet_since: Some(since),
                        };
                    }
                }
                State::Disarmed
                | State::Idle { .. }
                | State::Busy { .. }
                | State::ProbeLost { .. } => {}
            },

            Input::PassDone { pass, ok } => {
                if let State::Busy { pass: running } = self.state {
                    // A report for a pass we are not running is a worker bug, not a device
                    // event; ignoring it keeps a stale message from moving the rig.
                    if running == pass {
                        self.finish_pass(pass, ok, &mut actions);
                    }
                }
            }
        }

        // A deferred disarm lands the moment nothing is being written.
        if self.disarm_pending && !matches!(self.state, State::Busy { .. } | State::Disarmed) {
            self.state = State::Disarmed;
            self.disarm_pending = false;
            actions.push(Action::Sound(Cue::Disarmed));
        }

        actions
    }

    fn enter_idle(&mut self, expect: Pass, actions: &mut Vec<Action>) {
        self.state = State::Idle { expect };
        actions.push(Action::SetPollPeriod(self.timing.idle_poll_ms));
    }

    fn enter_await_removal(&mut self, expect_next: Pass, actions: &mut Vec<Action>) {
        self.state = State::AwaitRemoval {
            expect_next,
            quiet_since: None,
        };
        actions.push(Action::SetPollPeriod(self.timing.active_poll_ms));
    }

    fn finish_pass(&mut self, pass: Pass, ok: bool, actions: &mut Vec<Action>) {
        let (cue, expect_next) = match (pass, ok) {
            (Pass::Flash, true) if self.sequence == Sequence::FlashEveryInsertion => {
                (Cue::Pass, Pass::Flash)
            }
            (Pass::Flash, true) => (Cue::FlashedCycleIt, Pass::RunCheck),
            (Pass::RunCheck, true) => (Cue::Pass, Pass::Flash),
            // Any failure sends the rig back to expecting a flash, which is what makes a failed
            // board re-flash on reinsertion instead of being run-checked.
            (_, false) => (Cue::Fail, Pass::Flash),
        };
        actions.push(Action::Sound(cue));
        self.enter_await_removal(expect_next, actions);
    }
}

impl Default for Machine {
    fn default() -> Self {
        Self::new(Timing::default())
    }
}

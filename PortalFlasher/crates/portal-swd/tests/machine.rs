//! The rig's policy, exercised as timelines.
//!
//! Every test here is a hand-plug written down: a sequence of poll results at named instants,
//! and an assertion about what the rig did. None of them needs a probe, a board, or a GUI, which
//! is the point of keeping the machine pure — a contact that bounces seven times in 200 ms is
//! difficult to arrange on a bench and trivial to arrange here.

use portal_swd::machine::{Action, Cue, Input, Machine, Millis, Pass, Phase, Timing};

/// Drives a machine along a timeline, collecting everything it emitted.
struct Rig {
    machine: Machine,
    now: Millis,
    actions: Vec<Action>,
    /// Whether a live page is assumed. On by default, because that is the normal condition and
    /// a test that silently drifted into the dead-man timeout would be asserting the wrong
    /// thing. The tests that are *about* the dead-man turn it off explicitly.
    heartbeat: bool,
}

impl Rig {
    fn new() -> Self {
        Self {
            machine: Machine::new(Timing::default()),
            now: 0,
            actions: Vec::new(),
            heartbeat: true,
        }
    }

    /// Stop pretending a page is watching.
    fn ui_goes_away(&mut self) -> &mut Self {
        self.heartbeat = false;
        self
    }

    /// Feed one input at the current instant.
    fn input(&mut self, input: Input) -> &mut Self {
        let produced = self.machine.step(self.now, input);
        self.actions.extend(produced);
        self
    }

    /// Move the clock, delivering the heartbeat a live page would have sent meanwhile.
    fn advance(&mut self, ms: Millis) -> &mut Self {
        self.now += ms;
        if self.heartbeat {
            // Heartbeats emit no actions, so this cannot pollute what a test observes.
            let produced = self.machine.step(self.now, Input::Heartbeat);
            debug_assert!(produced.is_empty(), "a heartbeat should be silent");
        }
        self
    }

    /// A poll result `ms` after the previous event.
    fn poll(&mut self, ms: Millis, present: bool) -> &mut Self {
        self.advance(ms);
        self.input(if present {
            Input::PollPresent
        } else {
            Input::PollAbsent
        })
    }

    /// `n` present polls, one active period apart.
    fn present_run(&mut self, n: usize) -> &mut Self {
        for _ in 0..n {
            self.poll(80, true);
        }
        self
    }

    /// `n` absent polls, one active period apart.
    fn absent_run(&mut self, n: usize) -> &mut Self {
        for _ in 0..n {
            self.poll(80, false);
        }
        self
    }

    /// Arm, then clear the fixture so the rig reaches a state that can start a pass.
    fn arm_and_settle(&mut self) -> &mut Self {
        self.input(Input::Arm);
        // The arm gate needs continuous absence, same as any removal.
        self.absent_run(8);
        assert_eq!(
            self.machine.phase(),
            Phase::Idle,
            "should be armed and idle"
        );
        self.clear();
        self
    }

    fn clear(&mut self) -> &mut Self {
        self.actions.clear();
        self
    }

    fn cues(&self) -> Vec<Cue> {
        self.actions
            .iter()
            .filter_map(|a| match a {
                Action::Sound(cue) => Some(*cue),
                _ => None,
            })
            .collect()
    }

    fn passes_begun(&self) -> Vec<Pass> {
        self.actions
            .iter()
            .filter_map(|a| match a {
                Action::BeginPass(pass) => Some(*pass),
                _ => None,
            })
            .collect()
    }

    /// Run a whole successful pass: seat the board, let it commit, report success.
    fn complete_pass(&mut self, expect: Pass, ok: bool) -> &mut Self {
        self.present_run(3);
        assert_eq!(
            self.passes_begun(),
            vec![expect],
            "expected a {expect} pass to have begun"
        );
        self.advance(2_000);
        self.input(Input::PassDone { pass: expect, ok });
        self
    }
}

// ---------------------------------------------------------------- arming

#[test]
fn arming_requires_an_empty_fixture_before_anything_can_start() {
    let mut r = Rig::new();
    r.input(Input::Arm);

    // A board left in the fixture at arm time must not be flashed just because someone pressed
    // the button. The rig has to see the fixture empty once first.
    assert_eq!(r.machine.phase(), Phase::AwaitRemoval);
    r.present_run(10);
    assert_eq!(r.machine.phase(), Phase::AwaitRemoval);
    assert!(r.passes_begun().is_empty(), "armed onto a seated board");

    r.absent_run(8);
    assert_eq!(r.machine.phase(), Phase::Idle);
}

#[test]
fn arming_sounds_a_cue_so_a_muted_channel_is_found_before_a_board_is() {
    let mut r = Rig::new();
    r.input(Input::Arm);
    assert!(
        r.cues().contains(&Cue::Armed),
        "arming must be audible; it is the only chance to notice silence before a board is \
         flashed in it"
    );
}

#[test]
fn a_disarmed_rig_ignores_everything_a_board_does() {
    let mut r = Rig::new();
    r.present_run(20);
    assert_eq!(r.machine.phase(), Phase::Disarmed);
    assert!(r.passes_begun().is_empty());
}

// ---------------------------------------------------------------- debounce

#[test]
fn three_consecutive_answers_commit_a_pass() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.poll(80, true);
    assert_eq!(r.machine.phase(), Phase::Debouncing);
    r.poll(80, true);
    assert_eq!(r.machine.phase(), Phase::Debouncing);
    assert!(r.passes_begun().is_empty(), "committed one poll early");

    r.poll(80, true);
    assert_eq!(r.machine.phase(), Phase::Flashing);
    assert_eq!(r.passes_begun(), vec![Pass::Flash]);
}

#[test]
fn the_streak_is_discarded_by_a_gap_not_decremented() {
    let mut r = Rig::new();
    r.arm_and_settle();

    // Two answers, a break, then two more. Five polls, four of them present, and the rig must
    // still not have started anything: a contact that is making and breaking has not been
    // seated, however many times it answered.
    r.present_run(2).poll(80, false).present_run(2);
    assert!(
        r.passes_begun().is_empty(),
        "a break must discard the streak, not decrement it"
    );

    r.poll(80, true);
    assert_eq!(
        r.passes_begun(),
        vec![Pass::Flash],
        "the third of the new run should commit"
    );
}

#[test]
fn a_bouncing_contact_never_starts_a_pass() {
    let mut r = Rig::new();
    r.arm_and_settle();

    // A wipe across the pins: making and breaking every poll for two seconds.
    for i in 0..25 {
        r.poll(80, i % 2 == 0);
    }
    assert!(
        r.passes_begun().is_empty(),
        "alternating contact started {} pass(es)",
        r.passes_begun().len()
    );
}

#[test]
fn two_present_then_bounce_then_a_clean_seat_commits_once() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.present_run(2)
        .poll(80, false)
        .poll(80, true)
        .poll(80, false)
        .present_run(3);

    assert_eq!(
        r.passes_begun(),
        vec![Pass::Flash],
        "the clean run of three should commit exactly once"
    );
}

#[test]
fn a_streak_that_takes_too_long_is_a_sick_probe_and_starts_over() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.poll(80, true).poll(80, true);
    // Polls are supposed to cost single-digit milliseconds. One taking two seconds means the
    // probe is in trouble, not that a hand is slow, and committing a write on it would be
    // optimistic.
    r.poll(2_000, true);
    assert_eq!(r.machine.phase(), Phase::Idle);
    assert!(r.passes_begun().is_empty());
}

// ---------------------------------------------------------------- the two-pass rhythm

#[test]
fn flash_then_reinsert_gives_a_run_check_not_a_second_flash() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.complete_pass(Pass::Flash, true);
    assert_eq!(r.cues().last(), Some(&Cue::FlashedCycleIt));
    assert_eq!(r.machine.expect(), Some(Pass::RunCheck));

    // Cycle it.
    r.absent_run(8).clear();
    assert_eq!(r.machine.phase(), Phase::Idle);

    r.complete_pass(Pass::RunCheck, true);
    assert_eq!(r.cues().last(), Some(&Cue::Pass));
    assert_eq!(
        r.machine.expect(),
        Some(Pass::Flash),
        "the next board should get a flash"
    );
}

#[test]
fn the_two_success_cues_are_different_sounds() {
    // The operator is not looking at the screen. "Flashed, cycle it" and "this board is done"
    // must not be the same tone, or the second insertion never happens.
    assert_ne!(Cue::FlashedCycleIt, Cue::Pass);
}

#[test]
fn a_failed_flash_re_flashes_on_reinsertion() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.complete_pass(Pass::Flash, false);
    assert_eq!(r.cues().last(), Some(&Cue::Fail));
    assert_eq!(r.machine.expect(), Some(Pass::Flash));

    r.absent_run(8).clear();
    r.present_run(3);
    assert_eq!(
        r.passes_begun(),
        vec![Pass::Flash],
        "a failed board must be re-flashed, never run-checked"
    );
}

#[test]
fn a_failed_run_check_sends_the_board_back_for_a_flash() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.complete_pass(Pass::Flash, true);
    r.absent_run(8).clear();
    r.complete_pass(Pass::RunCheck, false);

    assert_eq!(r.cues().last(), Some(&Cue::Fail));
    assert_eq!(r.machine.expect(), Some(Pass::Flash));
}

#[test]
fn three_boards_in_a_row_alternate_correctly() {
    let mut r = Rig::new();
    r.arm_and_settle();

    for board in 0..3 {
        r.complete_pass(Pass::Flash, true);
        assert_eq!(r.cues().last(), Some(&Cue::FlashedCycleIt), "board {board}");
        r.absent_run(8).clear();

        r.complete_pass(Pass::RunCheck, true);
        assert_eq!(r.cues().last(), Some(&Cue::Pass), "board {board}");
        r.absent_run(8).clear();
    }
}

// ---------------------------------------------------------------- the removal gate

#[test]
fn a_single_insertion_cannot_produce_two_passes() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.complete_pass(Pass::Flash, true);
    r.clear();

    // The board never leaves. Poll for ten seconds.
    for _ in 0..125 {
        r.poll(80, true);
    }
    assert!(
        r.passes_begun().is_empty(),
        "a board that was never removed started another pass"
    );
    assert_eq!(r.machine.phase(), Phase::AwaitRemoval);
}

#[test]
fn the_removal_gate_measures_continuous_absence_not_elapsed_time() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.complete_pass(Pass::Flash, true);
    r.clear();

    // Nearly clear, then one more touch. This is a hand lifting and brushing back.
    r.absent_run(5); // 400 ms, just short of the 500 ms gate
    assert_eq!(r.machine.phase(), Phase::AwaitRemoval);
    r.poll(80, true);
    r.absent_run(5); // 400 ms again, from scratch
    assert_eq!(
        r.machine.phase(),
        Phase::AwaitRemoval,
        "the touch must have restarted the gate, not been averaged into it"
    );

    // A full gate's worth of absence, counted from the touch rather than from the first lift.
    r.absent_run(8);
    assert_eq!(r.machine.phase(), Phase::Idle);
}

#[test]
fn re_arming_is_audible() {
    let mut r = Rig::new();
    r.arm_and_settle();
    r.complete_pass(Pass::Flash, true);
    r.clear();

    r.absent_run(8);
    assert!(
        r.cues().contains(&Cue::Rearmed),
        "the operator needs to know the removal registered before reaching for the next board"
    );
}

// ---------------------------------------------------------------- probe loss

#[test]
fn losing_the_probe_mid_pass_fails_the_pass() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.present_run(3);
    assert_eq!(r.machine.phase(), Phase::Flashing);
    r.clear();

    r.input(Input::ProbeError);
    assert_eq!(r.cues(), vec![Cue::Fail]);
    assert_eq!(
        r.machine.expect(),
        Some(Pass::Flash),
        "an interrupted write must be redone in full"
    );
}

#[test]
fn a_recovered_probe_still_demands_a_removal_cycle() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.input(Input::ProbeError);
    assert_eq!(r.machine.phase(), Phase::ProbeLost);

    r.input(Input::ProbeRecovered);
    assert_eq!(
        r.machine.phase(),
        Phase::AwaitRemoval,
        "after a USB dropout there is no way to know what the SWD lines did, or whether the \
         board in the fixture is the one that was there before"
    );

    r.absent_run(8);
    assert_eq!(r.machine.phase(), Phase::Idle);
}

#[test]
fn a_stale_pass_report_does_not_move_the_rig() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.present_run(3);
    r.input(Input::PassDone {
        pass: Pass::Flash,
        ok: true,
    });
    r.clear();

    // A duplicate, or a report from a pass that is no longer running, is a worker bug rather
    // than a device event.
    r.input(Input::PassDone {
        pass: Pass::Flash,
        ok: true,
    });
    r.input(Input::PassDone {
        pass: Pass::RunCheck,
        ok: false,
    });
    assert!(
        r.cues().is_empty(),
        "a stale report produced operator feedback"
    );
    assert_eq!(r.machine.expect(), Some(Pass::RunCheck));
}

// ---------------------------------------------------------------- the dead man

#[test]
fn losing_the_ui_disarms_the_rig() {
    let mut r = Rig::new();
    r.arm_and_settle();

    // The page stops answering.
    r.ui_goes_away().advance(3_001).input(Input::Tick);
    assert_eq!(
        r.machine.phase(),
        Phase::Disarmed,
        "sound lives in the browser; a rig nobody can hear must not be armed"
    );
}

#[test]
fn a_heartbeat_keeps_the_rig_armed() {
    let mut r = Rig::new();
    r.arm_and_settle();

    for _ in 0..10 {
        r.advance(1_000).input(Input::Heartbeat);
        assert!(r.machine.armed());
    }
}

#[test]
fn a_dead_ui_does_not_abort_a_write_in_progress() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.present_run(3);
    assert_eq!(r.machine.phase(), Phase::Flashing);

    // The page dies while the erase is running. Aborting here is worse than finishing: a
    // half-erased board is the failure this whole design is arranged around avoiding.
    r.ui_goes_away().advance(5_000).input(Input::Tick);
    assert_eq!(r.machine.phase(), Phase::Flashing);
    assert!(r.machine.disarm_pending());

    r.input(Input::PassDone {
        pass: Pass::Flash,
        ok: true,
    });
    assert_eq!(r.machine.phase(), Phase::Disarmed);
}

#[test]
fn an_explicit_disarm_also_waits_for_the_pass_to_finish() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.present_run(3);
    r.input(Input::Disarm);
    assert_eq!(
        r.machine.phase(),
        Phase::Flashing,
        "a button press must not interrupt a write either"
    );

    r.input(Input::PassDone {
        pass: Pass::Flash,
        ok: true,
    });
    assert_eq!(r.machine.phase(), Phase::Disarmed);
}

#[test]
fn disarming_while_idle_is_immediate() {
    let mut r = Rig::new();
    r.arm_and_settle();

    r.input(Input::Disarm);
    assert_eq!(r.machine.phase(), Phase::Disarmed);
    assert!(r.cues().contains(&Cue::Disarmed));
}

// ---------------------------------------------------------------- poll cadence

#[test]
fn idling_polls_slowly_and_measuring_a_hand_polls_quickly() {
    let timing = Timing::default();
    let mut r = Rig::new();
    r.arm_and_settle();

    let period = |actions: &[Action]| {
        actions.iter().rev().find_map(|a| match a {
            Action::SetPollPeriod(ms) => Some(*ms),
            _ => None,
        })
    };

    r.poll(333, true);
    assert_eq!(
        period(&r.actions),
        Some(timing.active_poll_ms),
        "the first answer should tighten the cadence"
    );

    r.clear().poll(80, false);
    assert_eq!(
        period(&r.actions),
        Some(timing.idle_poll_ms),
        "a discarded streak should relax it again"
    );
}

#[test]
fn the_debounce_window_is_the_250ms_the_specification_asked_for() {
    let timing = Timing::default();
    let span = timing.active_poll_ms * u64::from(timing.debounce_polls);
    assert!(
        (240..=260).contains(&span),
        "three polls at {} ms span {span} ms, which is not the ~250 ms the behaviour spec names",
        timing.active_poll_ms
    );
}

// ---------------------------------------------------------------- the invariant

#[test]
fn no_input_sequence_can_start_two_passes_without_a_removal_between_them() {
    // A deterministic walk over a wide range of interleavings, rather than a property-test
    // dependency. Each seed picks a different pattern of present/absent polls and occasional
    // pass reports; the invariant is checked continuously.
    let mut total_passes = 0usize;
    let mut total_removals = 0usize;
    let mut total_probe_losses = 0usize;

    for seed in 0..2_000u64 {
        let mut machine = Machine::new(Timing::default());
        let mut now: Millis = 0;
        let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u32
        };

        machine.step(now, Input::Arm);

        let mut passes = 0usize;
        let mut removals_since_last_pass = 1usize; // arming itself demands one
        let mut in_pass = false;

        // Contacts come in runs, not independent coin flips. Flipping a fair coin per poll
        // almost never produces the seven consecutive absences the removal gate wants, so the
        // walk would spend its whole life in AwaitRemoval and never exercise anything. Runs of
        // 1..12 cover both a clean seat and a wipe across the pins.
        let mut run_present = false;
        let mut run_left = 0u32;

        for _ in 0..400 {
            now += 20 + u64::from(next() % 120);
            machine.step(now, Input::Heartbeat);

            let input = if in_pass {
                match next() % 8 {
                    0 => Input::ProbeError,
                    1..=4 => Input::PassDone {
                        pass: machine.phase_pass().unwrap_or(Pass::Flash),
                        ok: next() % 2 == 0,
                    },
                    _ => Input::Tick,
                }
            } else if next() % 200 == 0 {
                // A probe that dropped out on one poll in ten would never flash anything; this
                // is rare enough to be realistic and frequent enough to cover the recovery path.
                Input::ProbeError
            } else if machine.phase() == Phase::ProbeLost {
                Input::ProbeRecovered
            } else {
                if run_left == 0 {
                    run_present = next() % 2 == 0;
                    run_left = 1 + next() % 12;
                }
                run_left -= 1;
                if run_present {
                    Input::PollPresent
                } else {
                    Input::PollAbsent
                }
            };

            let before = machine.phase();
            let actions = machine.step(now, input);
            let after = machine.phase();

            if actions.iter().any(|a| matches!(a, Action::BeginPass(_))) {
                passes += 1;
                assert!(
                    removals_since_last_pass > 0,
                    "seed {seed}: a pass started without a removal gate between it and the last"
                );
                removals_since_last_pass = 0;
            }
            if before == Phase::AwaitRemoval && after == Phase::Idle {
                removals_since_last_pass += 1;
                total_removals += 1;
            }
            if after == Phase::ProbeLost && before != Phase::ProbeLost {
                total_probe_losses += 1;
            }

            in_pass = machine.pass_in_flight();
            assert!(
                !(in_pass
                    && matches!(input, Input::PollPresent | Input::PollAbsent)
                    && before != Phase::Debouncing),
                "seed {seed}: polled during a pass"
            );
        }

        total_passes += passes;
    }

    // The invariant above is only worth anything if the walk actually reached the states it
    // constrains. A generator that idles for 800,000 steps would pass it vacuously.
    assert!(
        total_passes > 1_000,
        "the walk only started {total_passes} passes; it is not exercising the invariant"
    );
    assert!(
        total_removals > 1_000,
        "the walk only completed {total_removals} removal gates"
    );
    assert!(
        total_probe_losses > 100,
        "the walk only saw {total_probe_losses} probe losses; the recovery path is untested"
    );
}

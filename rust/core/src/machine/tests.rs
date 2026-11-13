//! One test per transition row (T1-T10, P1-P13) plus the scheduling and ignoring rules.
//! The machine is stepped directly with plain values; no engine, no fakes.

use std::time::Duration;

use super::*;
use crate::domain::{PcState as P, TunnelState as T};
use crate::snapshot::DisplayState as D;
use crate::testing::{multi_config, pc_id, single_config, tunnel_id};

const SEC: Duration = Duration::from_secs(1);

fn at(secs: u64) -> Time {
    Time::ZERO + Duration::from_secs(secs)
}

fn single() -> Machine {
    Machine::new(&single_config(), Time::ZERO)
}

fn multi() -> Machine {
    Machine::new(&multi_config(), Time::ZERO)
}

fn desktop() -> PcId {
    pc_id("desktop")
}

fn home() -> TunnelId {
    tunnel_id("home")
}

fn tunnel_probed(
    m: &mut Machine,
    id: &TunnelId,
    kind: TunnelProbe,
    o: Observation,
    now: Time,
) -> Output {
    m.step(
        Input::TunnelProbed {
            tunnel: id.clone(),
            kind,
            observation: o,
        },
        now,
    )
}

fn pc_probed(m: &mut Machine, id: &PcId, kind: PcProbe, ready: bool, now: Time) -> Output {
    m.step(
        Input::PcProbed {
            pc: id.clone(),
            kind,
            ready,
        },
        now,
    )
}

fn poll(id: &PcId) -> Effect {
    Effect::ProbePc {
        pc: id.clone(),
        kind: PcProbe::Poll,
    }
}

fn check(id: &PcId) -> Effect {
    Effect::ProbePc {
        pc: id.clone(),
        kind: PcProbe::Check,
    }
}

/// Tunnel up and desktop online, nothing pending. Returns the time of the last step.
fn online(m: &mut Machine) -> Time {
    let now = Time::ZERO;
    m.step(Input::Tick, now);
    let out = tunnel_probed(m, &home(), TunnelProbe::Full, Observation::Reachable, now);
    assert_eq!(out.effects, [poll(&desktop())]);
    pc_probed(m, &desktop(), PcProbe::Poll, true, now);
    assert_eq!(
        (m.tunnel_state(&home()), m.pc_state(&desktop())),
        (Some(T::Up), Some(P::Online))
    );
    now
}

/// Desktop Waking behind an Up tunnel.
fn waking(m: &mut Machine) -> Time {
    let now = Time::ZERO;
    m.step(Input::Tick, now);
    tunnel_probed(m, &home(), TunnelProbe::Full, Observation::Reachable, now);
    pc_probed(m, &desktop(), PcProbe::Poll, false, now);
    m.step(Input::Command(Command::Wake(desktop())), now);
    pc_probed(m, &desktop(), PcProbe::Check, false, now);
    m.step(
        Input::WakeDone {
            pc: desktop(),
            result: Ok(()),
        },
        now,
    );
    assert_eq!(m.pc_state(&desktop()), Some(P::Waking));
    now
}

// ---------- tunnel rows ----------

#[test]
fn t1_toggle_from_down_starts_the_tunnel() {
    let mut m = single();
    let out = m.step(Input::Command(Command::ToggleTunnel(home())), Time::ZERO);
    assert_eq!(out.effects, [Effect::TunnelUp(home())]);
    assert_eq!(m.tunnel_state(&home()), Some(T::Starting));
    assert!(m.awaiting_tunnel());
    assert!(
        !m.snapshot().groups[0].tunnel.as_ref().unwrap().can_toggle,
        "an action runs"
    );
}

#[test]
fn t1_wake_on_a_down_tunnel_starts_it_and_remembers_the_pc() {
    let mut m = single();
    let out = m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    assert_eq!(out.effects, [Effect::TunnelUp(home())]);
    assert_eq!(m.pending_wakes(), [desktop()]);
}

#[test]
fn t2_wake_while_the_start_is_awaited_only_adds_a_pending_pc() {
    let mut m = multi();
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Ok(()),
        },
        Time::ZERO,
    );
    let out = m.step(Input::Command(Command::Wake(pc_id("laptop"))), at(1));
    assert!(out.effects.is_empty(), "{:?}", out.effects);
    assert_eq!(m.pending_wakes(), [desktop(), pc_id("laptop")]);
}

#[test]
fn t2_wake_while_the_start_job_runs_also_adds_a_pending_pc() {
    let mut m = multi();
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    let out = m.step(Input::Command(Command::Wake(pc_id("laptop"))), Time::ZERO);
    assert!(out.effects.is_empty());
    assert_eq!(m.pending_wakes().len(), 2);
}

#[test]
fn t3_wake_on_an_up_tunnel_checks_the_pc_first() {
    let mut m = single();
    m.step(Input::Tick, Time::ZERO);
    tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::Reachable,
        Time::ZERO,
    );
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, Time::ZERO);
    let out = m.step(Input::Command(Command::Wake(desktop())), at(1));
    assert_eq!(out.effects, [check(&desktop())]);
    assert_eq!(
        m.tunnel_state(&home()),
        Some(T::Up),
        "the tunnel does not flip to Starting"
    );
}

#[test]
fn t4_toggle_while_up_stops_it_and_a_later_loss_is_silent() {
    let mut m = single();
    let now = online(&mut m);
    let out = m.step(Input::Command(Command::ToggleTunnel(home())), now);
    assert_eq!(out.effects, [Effect::TunnelDown(home())]);
    m.step(
        Input::TunnelDownDone {
            tunnel: home(),
            result: Ok(()),
        },
        now,
    );
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::NotRunning,
        now,
    );
    assert!(out.notices.is_empty());
    assert_eq!(m.tunnel_state(&home()), Some(T::Down));
}

#[test]
fn t4_toggle_while_starting_stops_it_and_forgets_pending_pcs() {
    let mut m = single();
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Ok(()),
        },
        Time::ZERO,
    );
    let out = m.step(Input::Command(Command::ToggleTunnel(home())), at(1));
    assert_eq!(out.effects, [Effect::TunnelDown(home())]);
    assert!(m.pending_wakes().is_empty());
    assert!(!m.awaiting_tunnel());
}

#[test]
fn t5_failed_start_is_reported_and_the_real_state_is_read_at_once() {
    let mut m = single();
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    let out = m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Err("access denied".into()),
        },
        at(1),
    );
    assert_eq!(
        out.notices,
        [Notice::TunnelFailed {
            tunnel: "Home".into(),
            detail: "access denied".into()
        }]
    );
    assert!(m.pending_wakes().is_empty());
    assert!(!m.awaiting_tunnel());
    assert_eq!(out.next_wake, Some(at(1)), "a probe follows immediately");
    let out = m.step(Input::Tick, at(1));
    assert_eq!(
        out.effects,
        [Effect::ProbeTunnel {
            tunnel: home(),
            kind: TunnelProbe::Full
        }]
    );
    // The service is not running: Down, and no "lost" notice because it was Starting.
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::NotRunning,
        at(1),
    );
    assert!(out.notices.is_empty());
    assert_eq!(m.tunnel_state(&home()), Some(T::Down));
}

#[test]
fn t6_the_wait_ends_when_the_host_answers_and_the_pending_pc_is_checked() {
    let mut m = single();
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Ok(()),
        },
        Time::ZERO,
    );
    let out = m.step(Input::Tick, Time::ZERO);
    assert_eq!(
        out.effects,
        [Effect::ProbeTunnel {
            tunnel: home(),
            kind: TunnelProbe::ReachOnly
        }]
    );
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::ReachOnly,
        Observation::Reachable,
        at(3),
    );
    assert_eq!(out.effects, [check(&desktop())]);
    assert_eq!(m.tunnel_state(&home()), Some(T::Up));
    assert!(!m.awaiting_tunnel());
    assert!(m.pending_wakes().is_empty());
}

#[test]
fn t6_while_waiting_the_host_is_probed_every_second() {
    let mut m = single();
    m.step(Input::Command(Command::ToggleTunnel(home())), Time::ZERO);
    m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Ok(()),
        },
        Time::ZERO,
    );
    m.step(Input::Tick, Time::ZERO);
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::ReachOnly,
        Observation::Silent,
        at(2),
    );
    assert_eq!(out.next_wake, Some(at(2) + SEC));
    assert_eq!(m.tunnel_state(&home()), Some(T::Starting));
}

#[test]
fn t7_silence_after_the_deadline_reports_a_timeout() {
    let mut m = single();
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Ok(()),
        },
        Time::ZERO,
    );
    m.step(Input::Tick, Time::ZERO);
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::ReachOnly,
        Observation::Silent,
        at(20),
    );
    assert_eq!(
        out.notices,
        [Notice::TunnelTimeout {
            tunnel: "Home".into()
        }]
    );
    assert!(m.pending_wakes().is_empty());
    assert_eq!(m.tunnel_state(&home()), Some(T::Starting));
    assert_eq!(out.next_wake, Some(at(20)), "a full probe follows at once");
    // Silence just before the deadline is not a timeout.
    let mut m = single();
    m.step(Input::Command(Command::ToggleTunnel(home())), Time::ZERO);
    m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Ok(()),
        },
        Time::ZERO,
    );
    m.step(Input::Tick, Time::ZERO);
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::ReachOnly,
        Observation::Silent,
        at(19),
    );
    assert!(out.notices.is_empty());
}

#[test]
fn t8_an_unasked_stop_is_reported_once_and_resets_the_pcs() {
    let mut m = single();
    let now = online(&mut m);
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::NotRunning,
        now,
    );
    assert_eq!(
        out.notices,
        [Notice::TunnelLost {
            tunnel: "Home".into()
        }]
    );
    assert_eq!(
        (m.tunnel_state(&home()), m.pc_state(&desktop())),
        (Some(T::Down), Some(P::Unknown))
    );
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::NotRunning,
        at(5),
    );
    assert!(out.notices.is_empty(), "only once");
}

#[test]
fn t9_a_silent_host_makes_the_tunnel_starting_without_a_notice() {
    let mut m = single();
    let now = online(&mut m);
    let out = tunnel_probed(&mut m, &home(), TunnelProbe::Full, Observation::Silent, now);
    assert!(out.notices.is_empty());
    assert_eq!(
        (m.tunnel_state(&home()), m.pc_state(&desktop())),
        (Some(T::Starting), Some(P::Unknown))
    );
}

#[test]
fn t10_an_answering_host_means_up_and_the_pcs_are_probed() {
    let mut m = single();
    m.step(Input::Tick, Time::ZERO);
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::Reachable,
        Time::ZERO,
    );
    assert_eq!(m.tunnel_state(&home()), Some(T::Up));
    assert_eq!(out.effects, [poll(&desktop())]);
}

#[test]
fn a_tunnel_that_is_never_dropped_by_the_machine() {
    // Lifecycle stays as in 0.3.0: only an explicit toggle stops a tunnel.
    let mut m = single();
    let mut now = online(&mut m);
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Shutdown)),
        now,
    );
    m.step(
        Input::PowerDone {
            pc: desktop(),
            action: PowerAction::Shutdown,
            result: Ok(()),
        },
        now,
    );
    for _ in 0..10 {
        now = now + Duration::from_secs(5);
        m.step(Input::Tick, now);
        let out = tunnel_probed(
            &mut m,
            &home(),
            TunnelProbe::Full,
            Observation::Reachable,
            now,
        );
        assert!(
            !out.effects
                .iter()
                .any(|e| matches!(e, Effect::TunnelDown(_)))
        );
        if out.effects.contains(&poll(&desktop())) {
            pc_probed(&mut m, &desktop(), PcProbe::Poll, false, now);
        }
    }
    assert_eq!(m.pc_state(&desktop()), Some(P::Offline));
    assert_eq!(m.tunnel_state(&home()), Some(T::Up));
}

// ---------- PC rows ----------

#[test]
fn p1_a_ready_pc_is_online_and_a_waking_one_is_announced() {
    let mut m = single();
    let now = waking(&mut m);
    let out = pc_probed(&mut m, &desktop(), PcProbe::Poll, true, now + SEC);
    assert_eq!(m.pc_state(&desktop()), Some(P::Online));
    assert_eq!(
        out.notices,
        [Notice::PcOnline {
            pc: "Desktop".into(),
            host: "192.168.88.10".into()
        }]
    );
    // Already online: no second announcement.
    let out = pc_probed(&mut m, &desktop(), PcProbe::Poll, true, now + SEC + SEC);
    assert!(out.notices.is_empty());
}

#[test]
fn p2_online_survives_failures_below_the_limit() {
    let mut m = single();
    let now = online(&mut m);
    for n in 1..3 {
        let out = pc_probed(
            &mut m,
            &desktop(),
            PcProbe::Poll,
            false,
            now + Duration::from_secs(n),
        );
        assert_eq!(m.pc_state(&desktop()), Some(P::Online), "failure {n}");
        assert!(out.notices.is_empty());
    }
    let out = pc_probed(
        &mut m,
        &desktop(),
        PcProbe::Poll,
        false,
        now + Duration::from_secs(3),
    );
    assert_eq!(m.pc_state(&desktop()), Some(P::Offline));
    assert_eq!(
        out.notices,
        [Notice::PcLost {
            pc: "Desktop".into()
        }]
    );
}

#[test]
fn p3_waking_continues_until_the_time_limit() {
    let mut m = single();
    let now = waking(&mut m);
    let out = pc_probed(
        &mut m,
        &desktop(),
        PcProbe::Poll,
        false,
        now + Duration::from_secs(179),
    );
    assert_eq!(m.pc_state(&desktop()), Some(P::Waking));
    assert!(out.notices.is_empty());
}

#[test]
fn p4_a_pc_that_never_answers_after_waking_times_out() {
    let mut m = single();
    let now = waking(&mut m);
    let out = pc_probed(
        &mut m,
        &desktop(),
        PcProbe::Poll,
        false,
        now + Duration::from_secs(180),
    );
    assert_eq!(m.pc_state(&desktop()), Some(P::Offline));
    assert_eq!(
        out.notices,
        [Notice::PcTimeout {
            pc: "Desktop".into()
        }]
    );
}

#[test]
fn p4_an_unknown_pc_that_does_not_answer_is_simply_offline() {
    let mut m = single();
    m.step(Input::Tick, Time::ZERO);
    tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::Reachable,
        Time::ZERO,
    );
    let out = pc_probed(&mut m, &desktop(), PcProbe::Poll, false, Time::ZERO);
    assert_eq!(m.pc_state(&desktop()), Some(P::Offline));
    assert!(out.notices.is_empty());
}

#[test]
fn p5_losing_the_tunnel_makes_the_pc_unknown_without_pc_lost() {
    let mut m = single();
    let now = online(&mut m);
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, now);
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::NotRunning,
        now,
    );
    assert_eq!(m.pc_state(&desktop()), Some(P::Unknown));
    assert!(
        !out.notices
            .iter()
            .any(|n| matches!(n, Notice::PcLost { .. }))
    );
    // The failure counter starts over: after coming back it takes N failures again.
    tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::Reachable,
        at(5),
    );
    pc_probed(&mut m, &desktop(), PcProbe::Poll, true, at(5));
    for n in 1..3 {
        pc_probed(&mut m, &desktop(), PcProbe::Poll, false, at(5 + n));
        assert_eq!(m.pc_state(&desktop()), Some(P::Online));
    }
}

#[test]
fn p6_a_ready_pc_at_the_check_is_online_without_an_announcement() {
    let mut m = single();
    m.step(Input::Tick, Time::ZERO);
    tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::Reachable,
        Time::ZERO,
    );
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, Time::ZERO);
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    let out = pc_probed(&mut m, &desktop(), PcProbe::Check, true, Time::ZERO);
    assert_eq!(m.pc_state(&desktop()), Some(P::Online));
    assert!(out.effects.is_empty() && out.notices.is_empty());
}

#[test]
fn p7_a_silent_pc_at_the_check_gets_a_wake_request() {
    let mut m = single();
    m.step(Input::Tick, Time::ZERO);
    tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::Reachable,
        Time::ZERO,
    );
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    let out = pc_probed(&mut m, &desktop(), PcProbe::Check, false, Time::ZERO);
    assert_eq!(out.effects, [Effect::Wake(desktop())]);
    assert_eq!(m.pc_state(&desktop()), Some(P::Unknown));
}

#[test]
fn p8_an_accepted_wake_request_starts_waking_with_the_short_interval() {
    let mut m = single();
    let now = waking(&mut m);
    assert_eq!(m.pc_interval(&desktop()), Some(Duration::from_secs(2)));
    let out = m.step(Input::Tick, now + Duration::from_secs(2));
    // The PC is behind a tunnel: it is probed after the tunnel, in the same cycle.
    assert_eq!(
        out.effects,
        [Effect::ProbeTunnel {
            tunnel: home(),
            kind: TunnelProbe::Full
        }]
    );
}

#[test]
fn p9_a_failed_wake_request_is_reported_and_changes_nothing() {
    let mut m = single();
    m.step(Input::Tick, Time::ZERO);
    tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::Reachable,
        Time::ZERO,
    );
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    pc_probed(&mut m, &desktop(), PcProbe::Check, false, Time::ZERO);
    let out = m.step(
        Input::WakeDone {
            pc: desktop(),
            result: Err("connection refused".into()),
        },
        Time::ZERO,
    );
    assert_eq!(
        out.notices,
        [Notice::WakeFailed {
            pc: "Desktop".into(),
            detail: "connection refused".into()
        }]
    );
    assert_eq!(m.pc_state(&desktop()), Some(P::Unknown));
}

#[test]
fn p10_a_power_command_for_an_online_pc_is_run() {
    for action in [PowerAction::Sleep, PowerAction::Shutdown] {
        let mut m = single();
        let now = online(&mut m);
        let out = m.step(Input::Command(Command::Power(desktop(), action)), now);
        assert_eq!(
            out.effects,
            [Effect::Power {
                pc: desktop(),
                action
            }]
        );
    }
}

#[test]
fn p11_a_power_command_for_a_pc_that_is_not_online_is_refused() {
    let mut m = single();
    let now = waking(&mut m);
    let out = m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Sleep)),
        now,
    );
    assert!(out.effects.is_empty());
    assert_eq!(
        out.notices,
        [Notice::PowerUnavailable {
            pc: "Desktop".into()
        }]
    );
}

#[test]
fn p12_an_accepted_power_command_is_announced_and_changes_no_state() {
    for (action, expected) in [
        (
            PowerAction::Sleep,
            Notice::SleepSent {
                pc: "Desktop".into(),
                host: "192.168.88.10".into(),
            },
        ),
        (
            PowerAction::Shutdown,
            Notice::ShutdownSent {
                pc: "Desktop".into(),
                host: "192.168.88.10".into(),
            },
        ),
    ] {
        let mut m = single();
        let now = online(&mut m);
        m.step(Input::Command(Command::Power(desktop(), action)), now);
        let out = m.step(
            Input::PowerDone {
                pc: desktop(),
                action,
                result: Ok(()),
            },
            now,
        );
        assert_eq!(out.notices, [expected]);
        assert_eq!(m.pc_state(&desktop()), Some(P::Online));
    }
}

#[test]
fn p13_a_failed_power_command_is_reported_and_does_not_silence_pc_lost() {
    let mut m = single();
    let now = online(&mut m);
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Shutdown)),
        now,
    );
    let out = m.step(
        Input::PowerDone {
            pc: desktop(),
            action: PowerAction::Shutdown,
            result: Err("denied".into()),
        },
        now,
    );
    assert_eq!(
        out.notices,
        [Notice::PowerFailed {
            pc: "Desktop".into(),
            detail: "denied".into()
        }]
    );
    for n in 1..3 {
        pc_probed(
            &mut m,
            &desktop(),
            PcProbe::Poll,
            false,
            now + Duration::from_secs(n),
        );
    }
    let out = pc_probed(
        &mut m,
        &desktop(),
        PcProbe::Poll,
        false,
        now + Duration::from_secs(3),
    );
    assert_eq!(
        out.notices,
        [Notice::PcLost {
            pc: "Desktop".into()
        }]
    );
}

#[test]
fn a_power_command_that_was_accepted_silences_pc_lost_until_the_pc_is_seen_online_again() {
    let mut m = single();
    let now = online(&mut m);
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Sleep)),
        now,
    );
    m.step(
        Input::PowerDone {
            pc: desktop(),
            action: PowerAction::Sleep,
            result: Ok(()),
        },
        now,
    );
    for n in 1..=3 {
        let out = pc_probed(
            &mut m,
            &desktop(),
            PcProbe::Poll,
            false,
            now + Duration::from_secs(n),
        );
        assert!(out.notices.is_empty());
    }
    pc_probed(
        &mut m,
        &desktop(),
        PcProbe::Poll,
        true,
        now + Duration::from_secs(10),
    );
    for n in 11..14 {
        pc_probed(
            &mut m,
            &desktop(),
            PcProbe::Poll,
            false,
            now + Duration::from_secs(n),
        );
    }
    // Seen online again, so the next loss is reported.
    assert_eq!(m.pc_state(&desktop()), Some(P::Offline));
}

// ---------- busy, ignoring and scheduling ----------

#[test]
fn a_second_action_for_the_same_pc_is_dropped_silently() {
    let mut m = single();
    let now = online(&mut m);
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Shutdown)),
        now,
    );
    let out = m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Sleep)),
        now,
    );
    assert!(out.effects.is_empty() && out.notices.is_empty());
    let out = m.step(Input::Command(Command::Wake(desktop())), now);
    assert!(out.effects.is_empty() && out.notices.is_empty());
}

#[test]
fn probe_results_are_ignored_while_an_action_runs() {
    let mut m = single();
    let now = online(&mut m);
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Shutdown)),
        now,
    );
    for n in 1..=5 {
        pc_probed(
            &mut m,
            &desktop(),
            PcProbe::Poll,
            false,
            now + Duration::from_secs(n),
        );
    }
    assert_eq!(m.pc_state(&desktop()), Some(P::Online));
}

#[test]
fn a_tunnel_probe_result_is_ignored_while_the_tunnel_action_runs() {
    let mut m = single();
    let now = online(&mut m);
    m.step(Input::Command(Command::ToggleTunnel(home())), now);
    let out = tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::NotRunning,
        now,
    );
    assert!(out.notices.is_empty());
    assert_eq!(m.tunnel_state(&home()), Some(T::Up));
}

#[test]
fn wake_is_ignored_for_online_waking_or_unwakeable_pcs() {
    let mut m = multi();
    // office-pc has a wake method; give it an online state through its tunnel.
    let office = tunnel_id("office");
    let op = pc_id("office-pc");
    m.step(Input::Tick, Time::ZERO);
    tunnel_probed(
        &mut m,
        &office,
        TunnelProbe::Full,
        Observation::Reachable,
        Time::ZERO,
    );
    pc_probed(&mut m, &op, PcProbe::Poll, true, Time::ZERO);
    let out = m.step(Input::Command(Command::Wake(op.clone())), Time::ZERO);
    assert!(out.effects.is_empty(), "online");
    // desktop is waking.
    let mut m = single();
    let now = waking(&mut m);
    let out = m.step(Input::Command(Command::Wake(desktop())), now);
    assert!(out.effects.is_empty(), "waking");
}

#[test]
fn unknown_ids_are_ignored() {
    let mut m = single();
    let out = m.step(Input::Command(Command::Wake(pc_id("ghost"))), Time::ZERO);
    assert!(out.effects.is_empty() && out.notices.is_empty());
    let out = m.step(
        Input::Command(Command::ToggleTunnel(tunnel_id("ghost"))),
        Time::ZERO,
    );
    assert!(out.effects.is_empty());
}

#[test]
fn a_pc_without_a_tunnel_is_polled_on_its_own_and_woken_directly() {
    let mut m = multi();
    let nas = pc_id("nas");
    let out = m.step(Input::Tick, Time::ZERO);
    assert!(out.effects.contains(&poll(&nas)));
    assert_eq!(out.next_wake, None, "everything is being probed");
    let out = pc_probed(&mut m, &nas, PcProbe::Poll, false, Time::ZERO);
    assert_eq!(m.pc_state(&nas), Some(P::Offline));
    assert_eq!(
        out.next_wake,
        Some(at(5)),
        "the tunnels are still being probed"
    );
    let out = m.step(Input::Command(Command::Wake(nas.clone())), at(1));
    assert_eq!(out.effects, [Effect::Wake(nas.clone())]);
    m.step(
        Input::WakeDone {
            pc: nas.clone(),
            result: Ok(()),
        },
        at(1),
    );
    assert_eq!(m.pc_state(&nas), Some(P::Waking));
    assert_eq!(m.pc_interval(&nas), Some(Duration::from_secs(2)));
}

#[test]
fn a_panicked_job_counts_as_a_failure() {
    let mut m = single();
    let now = online(&mut m);
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Sleep)),
        now,
    );
    let out = m.step(
        Input::Panicked(Effect::Power {
            pc: desktop(),
            action: PowerAction::Sleep,
        }),
        now,
    );
    assert_eq!(
        out.notices,
        [Notice::PowerFailed {
            pc: "Desktop".into(),
            detail: "internal error".into()
        }]
    );
    // The PC is not stuck busy.
    let out = m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Sleep)),
        now,
    );
    assert_eq!(out.effects.len(), 1);
}

#[test]
fn a_panicked_probe_is_rescheduled() {
    let mut m = single();
    let out = m.step(Input::Tick, Time::ZERO);
    assert_eq!(out.next_wake, None);
    let out = m.step(
        Input::Panicked(Effect::ProbeTunnel {
            tunnel: home(),
            kind: TunnelProbe::Full,
        }),
        Time::ZERO,
    );
    assert_eq!(out.next_wake, Some(at(5)));
}

#[test]
fn snapshot_decides_the_available_actions() {
    let mut m = multi();
    let snap = m.snapshot();
    assert_eq!(
        snap.groups.len(),
        3,
        "two tunnels and the group without a tunnel"
    );
    assert!(snap.groups[2].tunnel.is_none());
    let desktop_view = &snap.groups[0].pcs[0];
    assert!(desktop_view.actions.wake && !desktop_view.actions.sleep && !desktop_view.actions.rdp);

    m.step(Input::Tick, Time::ZERO);
    tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::Full,
        Observation::Reachable,
        Time::ZERO,
    );
    pc_probed(&mut m, &desktop(), PcProbe::Poll, true, Time::ZERO);
    pc_probed(&mut m, &pc_id("laptop"), PcProbe::Poll, true, Time::ZERO);
    let snap = m.snapshot();
    let home_pcs = &snap.groups[0].pcs;
    let a = home_pcs[0].actions;
    assert!(
        !a.wake && a.sleep && a.shutdown && a.rdp,
        "desktop: ssh, rdp"
    );
    let b = home_pcs[1].actions;
    assert!(
        !b.wake && !b.sleep && b.shutdown && !b.rdp,
        "laptop: only a shutdown command"
    );

    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Sleep)),
        Time::ZERO,
    );
    let snap = m.snapshot();
    assert!(snap.groups[0].pcs[0].busy);
    assert_eq!(snap.groups[0].pcs[0].actions, Actions::default());
}

// ---------- failure state (section 5.6) ----------

fn tunnel_view(m: &Machine) -> TunnelView {
    m.snapshot().groups.remove(0).tunnel.expect("tunnel group")
}

fn desktop_view(m: &Machine) -> PcView {
    m.snapshot().groups.remove(0).pcs.remove(0)
}

fn start_failed(m: &mut Machine, now: Time) {
    m.step(Input::Command(Command::Wake(desktop())), now);
    m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Err("access denied".into()),
        },
        now,
    );
}

fn tunnel_seen(m: &mut Machine, o: Observation, now: Time) {
    tunnel_probed(m, &home(), TunnelProbe::Full, o, now);
}

/// Desktop online, then a power command that failed.
fn power_failed(m: &mut Machine) -> Time {
    let now = online(m);
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Sleep)),
        now,
    );
    m.step(
        Input::PowerDone {
            pc: desktop(),
            action: PowerAction::Sleep,
            result: Err("denied".into()),
        },
        now,
    );
    now
}

#[test]
fn f2_a_failed_start_is_fail_and_survives_the_probe_that_follows_it() {
    let mut m = single();
    start_failed(&mut m, at(1));
    let v = tunnel_view(&m);
    assert_eq!(v.display, D::Fail);
    assert_eq!(
        v.last_error,
        Some(Notice::TunnelFailed {
            tunnel: "Home".into(),
            detail: "access denied".into()
        })
    );
    // T5 schedules a probe at once; it reads "stopped" and that is not a recovery.
    tunnel_seen(&mut m, Observation::NotRunning, at(1));
    assert_eq!(m.tunnel_state(&home()), Some(T::Down));
    assert_eq!(tunnel_view(&m).display, D::Fail);
    // Neither is a running service that does not answer yet.
    tunnel_seen(&mut m, Observation::Silent, at(6));
    assert_eq!(tunnel_view(&m).display, D::Fail);
    // F5: seen up.
    tunnel_seen(&mut m, Observation::Reachable, at(11));
    let v = tunnel_view(&m);
    assert_eq!((v.display, v.last_error), (D::Ok, None));
}

#[test]
fn f3_a_start_timeout_is_fail_until_the_tunnel_is_seen_up() {
    let mut m = single();
    m.step(Input::Command(Command::ToggleTunnel(home())), Time::ZERO);
    m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Ok(()),
        },
        Time::ZERO,
    );
    assert_eq!(tunnel_view(&m).display, D::Busy);
    tunnel_probed(
        &mut m,
        &home(),
        TunnelProbe::ReachOnly,
        Observation::Silent,
        at(20),
    );
    let v = tunnel_view(&m);
    assert_eq!(v.display, D::Fail);
    assert_eq!(
        v.last_error,
        Some(Notice::TunnelTimeout {
            tunnel: "Home".into()
        })
    );
    // The full probe that follows still finds the service silent.
    tunnel_seen(&mut m, Observation::Silent, at(20));
    assert_eq!(tunnel_view(&m).display, D::Fail);
    tunnel_seen(&mut m, Observation::Reachable, at(25));
    assert_eq!(tunnel_view(&m).display, D::Ok);
}

#[test]
fn f1_a_new_request_ends_the_failure_of_a_tunnel() {
    let mut m = single();
    start_failed(&mut m, Time::ZERO);
    tunnel_seen(&mut m, Observation::NotRunning, Time::ZERO);
    m.step(Input::Command(Command::ToggleTunnel(home())), at(2));
    let v = tunnel_view(&m);
    assert_eq!((v.display, v.last_error), (D::Busy, None));

    // Connect-and-wake is a request too, and so is a stop.
    let mut m = single();
    start_failed(&mut m, Time::ZERO);
    tunnel_seen(&mut m, Observation::NotRunning, Time::ZERO);
    m.step(Input::Command(Command::Wake(desktop())), at(2));
    assert_eq!(tunnel_view(&m).display, D::Busy);

    let mut m = single();
    let now = online(&mut m);
    m.step(Input::Command(Command::ToggleTunnel(home())), now);
    m.step(
        Input::TunnelDownDone {
            tunnel: home(),
            result: Err("busy".into()),
        },
        now,
    );
    assert_eq!(tunnel_view(&m).display, D::Fail);
    m.step(Input::Command(Command::ToggleTunnel(home())), at(1));
    assert_eq!(tunnel_view(&m).last_error, None);
}

#[test]
fn f4_a_failed_stop_is_fail_until_the_tunnel_is_seen_down() {
    let mut m = single();
    let now = online(&mut m);
    let out = m.step(Input::Command(Command::ToggleTunnel(home())), now);
    assert_eq!(out.effects, [Effect::TunnelDown(home())]);
    let out = m.step(
        Input::TunnelDownDone {
            tunnel: home(),
            result: Err("access denied".into()),
        },
        now,
    );
    let notice = Notice::TunnelStopFailed {
        tunnel: "Home".into(),
        detail: "access denied".into(),
    };
    assert_eq!(out.notices.as_slice(), std::slice::from_ref(&notice));
    let v = tunnel_view(&m);
    assert_eq!((v.display, v.state), (D::Fail, T::Up));
    assert_eq!(v.last_error, Some(notice));
    // F6: the tunnel still answers, so the stop did not work; seen down it did.
    tunnel_seen(&mut m, Observation::Reachable, at(1));
    assert_eq!(tunnel_view(&m).display, D::Fail);
    tunnel_seen(&mut m, Observation::NotRunning, at(2));
    let v = tunnel_view(&m);
    assert_eq!((v.display, v.last_error), (D::Off, None));
}

#[test]
fn f4_a_panicked_stop_job_is_a_failed_stop() {
    let mut m = single();
    let now = online(&mut m);
    m.step(Input::Command(Command::ToggleTunnel(home())), now);
    let out = m.step(Input::Panicked(Effect::TunnelDown(home())), now);
    assert_eq!(out.notices.len(), 1);
    assert_eq!(tunnel_view(&m).display, D::Fail);
}

#[test]
fn f8_a_failed_wake_request_is_fail_until_the_pc_is_seen_online() {
    let mut m = single();
    m.step(Input::Tick, Time::ZERO);
    tunnel_seen(&mut m, Observation::Reachable, Time::ZERO);
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, Time::ZERO);
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    pc_probed(&mut m, &desktop(), PcProbe::Check, false, Time::ZERO);
    m.step(
        Input::WakeDone {
            pc: desktop(),
            result: Err("connection refused".into()),
        },
        Time::ZERO,
    );
    let v = desktop_view(&m);
    assert_eq!((v.display, v.state), (D::Fail, P::Offline));
    assert_eq!(
        v.last_error,
        Some(Notice::WakeFailed {
            pc: "Desktop".into(),
            detail: "connection refused".into()
        })
    );
    // Still not answering: still failed. F11: answering ends it.
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, at(5));
    assert_eq!(desktop_view(&m).display, D::Fail);
    pc_probed(&mut m, &desktop(), PcProbe::Poll, true, at(10));
    let v = desktop_view(&m);
    assert_eq!((v.display, v.last_error), (D::Ok, None));
}

#[test]
fn f9_a_wake_timeout_is_fail_until_the_pc_answers_or_a_new_request() {
    let mut m = single();
    let now = waking(&mut m);
    assert_eq!(desktop_view(&m).display, D::Busy);
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, now + 180 * SEC);
    let v = desktop_view(&m);
    assert_eq!((v.display, v.state), (D::Fail, P::Offline));
    assert_eq!(
        v.last_error,
        Some(Notice::PcTimeout {
            pc: "Desktop".into()
        })
    );
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, now + 190 * SEC);
    assert_eq!(desktop_view(&m).display, D::Fail);
    // F7: a new wake request.
    m.step(Input::Command(Command::Wake(desktop())), now + 200 * SEC);
    let v = desktop_view(&m);
    assert_eq!((v.display, v.last_error), (D::Busy, None));
    // F11: answering ends the failure too.
    let mut m = single();
    let now = waking(&mut m);
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, now + 180 * SEC);
    pc_probed(&mut m, &desktop(), PcProbe::Poll, true, now + 185 * SEC);
    assert_eq!(desktop_view(&m).display, D::Ok);
}

#[test]
fn f10_a_failed_power_command_is_fail_and_survives_polls_that_still_find_the_pc_online() {
    let mut m = single();
    let now = power_failed(&mut m);
    let v = desktop_view(&m);
    assert_eq!((v.display, v.state), (D::Fail, P::Online));
    assert_eq!(
        v.last_error,
        Some(Notice::PowerFailed {
            pc: "Desktop".into(),
            detail: "denied".into()
        })
    );
    for n in 1..4 {
        pc_probed(&mut m, &desktop(), PcProbe::Poll, true, now + n * SEC);
        assert_eq!(desktop_view(&m).display, D::Fail, "poll {n}");
    }
}

#[test]
fn f12_a_power_failure_ends_when_the_pc_is_seen_leaving_online() {
    let mut m = single();
    let now = power_failed(&mut m);
    for n in 1..3 {
        pc_probed(&mut m, &desktop(), PcProbe::Poll, false, now + n * SEC);
        assert_eq!(desktop_view(&m).display, D::Fail, "a blip changes nothing");
    }
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, now + 3 * SEC);
    let v = desktop_view(&m);
    assert_eq!(
        (v.display, v.state, v.last_error),
        (D::Off, P::Offline, None)
    );
}

#[test]
fn f7_a_power_request_ends_the_failure() {
    let mut m = single();
    let now = power_failed(&mut m);
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Shutdown)),
        now + SEC,
    );
    let v = desktop_view(&m);
    assert_eq!((v.display, v.last_error), (D::Busy, None));
    m.step(
        Input::PowerDone {
            pc: desktop(),
            action: PowerAction::Shutdown,
            result: Ok(()),
        },
        now + 2 * SEC,
    );
    assert_eq!(desktop_view(&m).display, D::Ok);
}

#[test]
fn f13_losing_the_tunnel_ends_the_failure_of_its_pcs() {
    let mut m = single();
    power_failed(&mut m);
    tunnel_seen(&mut m, Observation::NotRunning, at(5));
    let v = desktop_view(&m);
    assert_eq!(
        (v.display, v.state, v.last_error),
        (D::Unknown, P::Unknown, None)
    );
}

#[test]
fn a_pc_waiting_for_its_tunnel_is_busy_and_does_not_inherit_the_tunnel_failure() {
    let mut m = single();
    m.step(Input::Command(Command::Wake(desktop())), Time::ZERO);
    assert_eq!(desktop_view(&m).display, D::Busy, "waiting for the tunnel");
    m.step(
        Input::TunnelUpDone {
            tunnel: home(),
            result: Err("access denied".into()),
        },
        at(1),
    );
    assert_eq!(tunnel_view(&m).display, D::Fail);
    let v = desktop_view(&m);
    assert_eq!((v.display, v.last_error), (D::Unknown, None));
}

#[test]
fn display_states_follow_the_observations() {
    let mut m = single();
    assert_eq!(tunnel_view(&m).display, D::Unknown, "never checked");
    assert_eq!(desktop_view(&m).display, D::Unknown);
    m.step(Input::Tick, Time::ZERO);
    tunnel_seen(&mut m, Observation::NotRunning, Time::ZERO);
    assert_eq!(tunnel_view(&m).display, D::Off);
    tunnel_seen(&mut m, Observation::Silent, at(5));
    assert_eq!(tunnel_view(&m).display, D::Busy);
    tunnel_seen(&mut m, Observation::Reachable, at(10));
    assert_eq!(tunnel_view(&m).display, D::Ok);
    pc_probed(&mut m, &desktop(), PcProbe::Poll, false, at(10));
    assert_eq!(desktop_view(&m).display, D::Off);
    pc_probed(&mut m, &desktop(), PcProbe::Poll, true, at(15));
    assert_eq!(desktop_view(&m).display, D::Ok);
}

#[test]
fn the_check_time_is_the_last_result_that_counted_rounded_down() {
    let mut m = single();
    assert_eq!(tunnel_view(&m).last_checked, None);
    assert_eq!(desktop_view(&m).last_checked, None);
    m.step(Input::Tick, Time::ZERO);
    tunnel_seen(&mut m, Observation::Reachable, at(37));
    assert_eq!(tunnel_view(&m).last_checked, Some(at(30)));
    pc_probed(&mut m, &desktop(), PcProbe::Poll, true, at(95));
    assert_eq!(desktop_view(&m).last_checked, Some(at(90)));
    // A result that is ignored while an action runs is not a completed check.
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Sleep)),
        at(96),
    );
    pc_probed(&mut m, &desktop(), PcProbe::Poll, true, at(130));
    assert_eq!(desktop_view(&m).last_checked, Some(at(90)));
}

#[test]
fn idle_polling_within_a_step_does_not_change_the_snapshot() {
    let mut m = single();
    m.step(Input::Tick, Time::ZERO);
    tunnel_seen(&mut m, Observation::Reachable, at(1));
    pc_probed(&mut m, &desktop(), PcProbe::Poll, true, at(1));
    let before = m.snapshot();
    tunnel_seen(&mut m, Observation::Reachable, at(6));
    pc_probed(&mut m, &desktop(), PcProbe::Poll, true, at(6));
    assert_eq!(m.snapshot(), before);
}

#[test]
fn the_snapshot_describes_each_pc_and_tunnel() {
    let m = multi();
    let snap = m.snapshot();
    let home_view = snap.groups[0].tunnel.as_ref().unwrap();
    assert_eq!(home_view.kind, "WireGuard");
    assert_eq!(home_view.pcs, ["Desktop", "Laptop"]);
    assert_eq!(home_view.toggle, TunnelToggle::Raise);

    let desktop_v = &snap.groups[0].pcs[0];
    assert_eq!(desktop_v.address, "192.168.88.10");
    assert_eq!(desktop_v.tunnel_name.as_deref(), Some("Home"));
    assert_eq!(desktop_v.ports, [3389]);
    assert_eq!(
        desktop_v.wake,
        Some(WakeSummary::Relay {
            relay: "gateway".into(),
            kind: crate::domain::RelayKind::RouterOs
        })
    );
    let laptop = &snap.groups[0].pcs[1];
    assert_eq!(laptop.ports, [3389, 445]);
    assert_eq!(
        laptop.wake,
        Some(WakeSummary::MagicPacket {
            address: "192.168.88.255".parse().unwrap(),
            port: 9
        })
    );
    // Configured is independent of the state: laptop has a shutdown command only.
    assert_eq!(
        laptop.configured,
        Actions {
            wake: true,
            sleep: false,
            shutdown: true,
            rdp: false
        }
    );
    assert!(!laptop.actions.shutdown, "not online: not available now");
    let office_pc = &snap.groups[1].pcs[0];
    assert_eq!(
        office_pc.wake,
        Some(WakeSummary::Command {
            program: "wakeonlan".into()
        })
    );
    let nas = &snap.groups[2].pcs[0];
    assert_eq!(nas.tunnel_name, None);
    assert_eq!(nas.address, "10.0.0.7");
}

#[test]
fn the_toggle_names_what_it_would_do() {
    let mut m = single();
    assert_eq!(tunnel_view(&m).toggle, TunnelToggle::Raise);
    m.step(Input::Command(Command::ToggleTunnel(home())), Time::ZERO);
    assert_eq!(tunnel_view(&m).toggle, TunnelToggle::Lower, "starting");
}

#[test]
fn the_tray_follows_the_displayed_states() {
    use crate::snapshot::Attention;
    let mut m = single();
    assert_eq!(m.snapshot().tray.screen, D::Unknown);
    let now = online(&mut m);
    let tray = m.snapshot().tray;
    assert_eq!(
        (tray.screen, tray.corner, tray.tunnels_up, tray.pcs_online),
        (D::Ok, Attention::None, 1, 1)
    );
    m.step(
        Input::Command(Command::Power(desktop(), PowerAction::Sleep)),
        now,
    );
    assert_eq!(m.snapshot().tray.corner, Attention::Busy);
    m.step(
        Input::PowerDone {
            pc: desktop(),
            action: PowerAction::Sleep,
            result: Err("x".into()),
        },
        now,
    );
    assert_eq!(m.snapshot().tray.corner, Attention::Fail);
    assert_eq!(m.snapshot().tray.screen, D::Ok, "the tunnel is fine");
}

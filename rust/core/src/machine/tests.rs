//! One test per transition row (T1-T10, P1-P13) plus the scheduling and ignoring rules.
//! The machine is stepped directly with plain values; no engine, no fakes.

use std::time::Duration;

use super::*;
use crate::domain::{PcState as P, TunnelState as T};
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
    m.step(Input::TunnelDownDone { tunnel: home() }, now);
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

//! Every test of `tests/test_controller.py` (the Python 0.3.0 controller), ported.
//!
//! Each test carries a `// Python: <name>` comment. The Python configuration maps to
//! `single_config()`: tunnel `home` (probe host `192.168.88.1`, timeout 20 s) and PC
//! `desktop` (`192.168.88.10`), poll 5 s, wake poll 2 s, 3 failures, wake timeout 180 s.
//! Differences from Python are called out where a test differs (see section 5.5 of the
//! architecture document).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::time::Duration;

use homelink_core::domain::{PcState, PowerAction, TunnelState, WakeKind};
use homelink_core::notice::{DETAIL_LIMIT, Notice};
use homelink_core::ports::{Clock, Time};
use homelink_core::testing::{Harness, pc_id, single_config, tunnel_id};

const ROUTER: &str = "192.168.88.1";
const PC: &str = "192.168.88.10";
const FAILURES: u64 = 3; // offline_after_failures

fn env() -> Harness {
    Harness::new(single_config())
}

fn up_and_online() -> Harness {
    let mut h = env();
    h.reachable(ROUTER, true);
    h.reachable(PC, true);
    h.wake_pc("desktop");
    assert_eq!(
        (h.tunnel_state("home"), h.pc("desktop")),
        (TunnelState::Up, PcState::Online)
    );
    h
}

fn status(h: &Harness) -> (TunnelState, PcState) {
    (h.tunnel_state("home"), h.pc("desktop"))
}

fn desktop_notice_detail(n: &Notice) -> String {
    n.params()
        .into_iter()
        .find(|(k, _)| *k == "detail")
        .map(|(_, v)| v)
        .unwrap_or_default()
}

const DOWN_UNKNOWN: (TunnelState, PcState) = (TunnelState::Down, PcState::Unknown);

// Python: test_connect_and_wake_sends_wol_when_pc_is_silent
#[test]
fn connect_and_wake_sends_wake_when_pc_is_silent() {
    let mut h = env();
    h.reachable(ROUTER, true);
    h.wake_pc("desktop");
    assert_eq!(status(&h), (TunnelState::Up, PcState::Waking));
    assert_eq!(h.wake.calls(), [(pc_id("desktop"), WakeKind::MikrotikRest)]);
    let first_change = h
        .sink
        .snapshots()
        .into_iter()
        .map(|s| s.groups[0].tunnel.clone().unwrap().state)
        .find(|s| *s != TunnelState::Down);
    assert_eq!(
        first_change,
        Some(TunnelState::Starting),
        "Starting is published before Up"
    );
}

// Python: test_connect_and_wake_skips_wol_when_pc_answers
#[test]
fn connect_and_wake_skips_wake_when_pc_answers() {
    let h = up_and_online();
    assert_eq!(status(&h), (TunnelState::Up, PcState::Online));
    assert!(h.wake.woken().is_empty());
}

// Python: test_tunnel_failure_is_reported
#[test]
fn tunnel_failure_is_reported() {
    let mut h = env();
    h.tunnel.fail_up_with(Some("access denied"));
    h.wake_pc("desktop");
    assert_eq!(status(&h), DOWN_UNKNOWN);
    assert_eq!(
        h.notices(),
        [Notice::TunnelFailed {
            tunnel: "Home".into(),
            detail: "access denied".into()
        }]
    );
}

// Python: test_tunnel_start_gives_up_after_timeout
// (Deviation 1: the timeout is now also announced with `TunnelTimeout`.)
#[test]
fn tunnel_start_gives_up_after_timeout() {
    let mut h = env();
    h.wake_pc("desktop"); // the router never answers
    assert!(h.clock.now() >= Time::ZERO + Duration::from_secs(20));
    assert_eq!(status(&h), (TunnelState::Starting, PcState::Unknown));
    assert!(h.wake.woken().is_empty());
    assert_eq!(
        h.notices(),
        [Notice::TunnelTimeout {
            tunnel: "Home".into()
        }]
    );
}

// Python: test_router_failure_is_reported (the router is now one wake method among several)
#[test]
fn wake_failure_is_reported() {
    let mut h = env();
    h.wake.fail_with(Some("HTTP 500"));
    h.reachable(ROUTER, true);
    h.wake_pc("desktop");
    assert_ne!(h.pc("desktop"), PcState::Waking);
    assert_eq!(
        h.notices(),
        [Notice::WakeFailed {
            pc: "Desktop".into(),
            detail: "HTTP 500".into()
        }]
    );
}

// Python: test_waking_pc_comes_online
#[test]
fn waking_pc_comes_online() {
    let mut h = env();
    h.reachable(ROUTER, true);
    h.wake_pc("desktop");
    h.refresh();
    assert_eq!(h.pc("desktop"), PcState::Waking);
    h.reachable(PC, true);
    h.refresh();
    assert_eq!(h.pc("desktop"), PcState::Online);
    assert_eq!(
        h.notices(),
        [Notice::PcOnline {
            pc: "Desktop".into(),
            host: PC.into()
        }]
    );
}

// Python: test_waking_times_out
#[test]
fn waking_times_out() {
    let mut h = env();
    h.reachable(ROUTER, true);
    h.wake_pc("desktop");
    h.advance(180 + 1);
    h.refresh();
    assert_eq!(h.pc("desktop"), PcState::Offline);
    assert_eq!(
        h.notices(),
        [Notice::PcTimeout {
            pc: "Desktop".into()
        }]
    );
}

// Python: test_wake_requires_tunnel
// (Deviation 2: `wake()` and `tunnel_required` are gone. Wake on a PC whose tunnel is down
// now means connect-and-wake: the tunnel is started first, then the PC is woken.)
#[test]
fn wake_with_the_tunnel_down_connects_first() {
    let mut h = env();
    h.reachable(ROUTER, true);
    h.wake_pc("desktop");
    assert_eq!(h.tunnel.ups(), [tunnel_id("home")]);
    assert_eq!(h.wake.woken(), [pc_id("desktop")]);
    assert!(h.notices().is_empty());
}

// Python: test_toggle_tunnel_up_and_down
#[test]
fn toggle_tunnel_up_and_down() {
    let mut h = env();
    h.reachable(ROUTER, true);
    h.toggle_tunnel("home");
    assert_eq!(status(&h), (TunnelState::Up, PcState::Offline));
    h.toggle_tunnel("home");
    assert_eq!(status(&h), DOWN_UNKNOWN);
    assert!(!h.tunnel.is_up(&tunnel_id("home")));
}

// Python: test_refresh_detects_external_tunnel
#[test]
fn refresh_detects_external_tunnel() {
    let mut h = env();
    h.tunnel.set_running(&tunnel_id("home"), true);
    h.reachable(ROUTER, true);
    h.reachable(PC, true);
    h.refresh();
    assert_eq!(status(&h), (TunnelState::Up, PcState::Online));
    assert!(h.notices().is_empty());
}

// Python: test_poll_interval_is_shorter_while_waking
#[test]
fn poll_interval_is_shorter_while_waking() {
    let mut h = env();
    let pc = pc_id("desktop");
    assert_eq!(
        h.engine.machine().pc_interval(&pc),
        Some(Duration::from_secs(5))
    );
    h.reachable(ROUTER, true);
    h.wake_pc("desktop");
    assert_eq!(
        h.engine.machine().pc_interval(&pc),
        Some(Duration::from_secs(2))
    );
}

// Python: test_single_failed_check_keeps_pc_online
#[test]
fn single_failed_check_keeps_pc_online() {
    let mut h = up_and_online();
    h.reachable(PC, false);
    for _ in 0..FAILURES - 1 {
        h.refresh();
        assert_eq!(h.pc("desktop"), PcState::Online);
    }
    h.refresh();
    assert_eq!(h.pc("desktop"), PcState::Offline);
}

// Python: test_one_successful_check_is_enough_for_online
#[test]
fn one_successful_check_is_enough_for_online() {
    let mut h = up_and_online();
    h.reachable(PC, false);
    h.refresh(); // one failure, still online
    h.reachable(PC, true);
    h.refresh(); // the failure counter starts over
    h.reachable(PC, false);
    for _ in 0..FAILURES - 1 {
        h.refresh();
    }
    assert_eq!(h.pc("desktop"), PcState::Online);

    h.refresh();
    assert_eq!(h.pc("desktop"), PcState::Offline);
    h.reachable(PC, true);
    h.refresh();
    assert_eq!(h.pc("desktop"), PcState::Online);
}

// ---------- power commands ----------

const ACTIONS: [PowerAction; 2] = [PowerAction::Shutdown, PowerAction::Sleep];

fn sent(action: PowerAction) -> Notice {
    let (pc, host) = ("Desktop".to_owned(), PC.to_owned());
    match action {
        PowerAction::Shutdown => Notice::ShutdownSent { pc, host },
        PowerAction::Sleep => Notice::SleepSent { pc, host },
    }
}

// Python: test_power_action_runs_configured_command
// (Building the command line from the configuration is a phase 3 adapter concern; here the
// engine must ask the power runner exactly once and announce it.)
#[test]
fn power_action_runs_configured_command() {
    for action in ACTIONS {
        let mut h = up_and_online();
        h.power_pc("desktop", action);
        assert_eq!(h.power.calls(), [(pc_id("desktop"), action)]);
        assert_eq!(h.notices(), [sent(action)]);
    }
}

// Python: test_power_action_does_not_change_state
#[test]
fn power_action_does_not_change_state() {
    for action in ACTIONS {
        let mut h = up_and_online();
        let before = h.sink.snapshots().len();
        h.power_pc("desktop", action);
        assert_eq!(status(&h), (TunnelState::Up, PcState::Online));
        for snapshot in &h.sink.snapshots()[before..] {
            assert_eq!(
                snapshot.groups[0].tunnel.as_ref().unwrap().state,
                TunnelState::Up
            );
            assert_eq!(snapshot.groups[0].pcs[0].state, PcState::Online);
        }
    }
}

// Python: test_power_failure_is_reported
#[test]
fn power_failure_is_reported() {
    for action in ACTIONS {
        let mut h = up_and_online();
        h.power.fail_with(Some("Permission denied"));
        h.power_pc("desktop", action);
        assert_eq!(
            h.notices(),
            [Notice::PowerFailed {
                pc: "Desktop".into(),
                detail: "Permission denied".into()
            }]
        );
    }
}

// Python: test_power_failure_detail_is_trimmed
#[test]
fn power_failure_detail_is_trimmed() {
    let mut h = up_and_online();
    h.power.fail_with(Some(&"x".repeat(DETAIL_LIMIT + 100)));
    h.power_pc("desktop", PowerAction::Shutdown);
    let notices = h.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(desktop_notice_detail(&notices[0]), "x".repeat(DETAIL_LIMIT));
}

// Python: test_power_action_needs_online_pc
#[test]
fn power_action_needs_online_pc() {
    for action in ACTIONS {
        let mut h = env();
        h.reachable(ROUTER, true);
        h.wake_pc("desktop"); // the PC is only waking
        h.power_pc("desktop", action);
        assert!(h.power.calls().is_empty());
        assert_eq!(
            h.notices(),
            [Notice::PowerUnavailable {
                pc: "Desktop".into()
            }]
        );
    }
}

// Python: test_power_action_is_skipped_while_busy
// (Python held the lock by hand; here a first command is still running when the second
// arrives.)
#[test]
fn power_action_is_skipped_while_busy() {
    let mut h = Harness::manual(single_config());
    h.reachable(ROUTER, true);
    h.reachable(PC, true);
    h.wake_pc("desktop");
    while h.jobs.as_ref().unwrap().pending() > 0 {
        h.run_jobs();
    }
    assert_eq!(status(&h), (TunnelState::Up, PcState::Online));

    h.power_pc("desktop", PowerAction::Shutdown); // the job is held
    h.power_pc("desktop", PowerAction::Sleep); // arrives while busy
    h.run_jobs();
    assert_eq!(h.power.calls(), [(pc_id("desktop"), PowerAction::Shutdown)]);
    assert_eq!(
        h.notices(),
        [sent(PowerAction::Shutdown)],
        "the skipped command is silent"
    );
}

// ---------- loss notifications ----------

// Python: test_tunnel_lost_is_notified_once
#[test]
fn tunnel_lost_is_notified_once() {
    let mut h = up_and_online();
    h.tunnel.set_running(&tunnel_id("home"), false); // stopped without being asked to
    h.refresh();
    h.refresh();
    assert_eq!(status(&h), DOWN_UNKNOWN);
    assert_eq!(
        h.notices(),
        [Notice::TunnelLost {
            tunnel: "Home".into()
        }]
    );
}

// Python: test_user_disconnect_is_not_reported_as_lost
#[test]
fn user_disconnect_is_not_reported_as_lost() {
    let mut h = up_and_online();
    h.toggle_tunnel("home");
    h.refresh();
    assert_eq!(status(&h), DOWN_UNKNOWN);
    assert!(h.notices().is_empty());
}

// Python: test_pc_lost_is_notified_after_configured_failures
#[test]
fn pc_lost_is_notified_after_configured_failures() {
    let mut h = up_and_online();
    h.reachable(PC, false);
    for _ in 0..FAILURES - 1 {
        h.refresh();
        assert!(h.notices().is_empty());
    }
    h.refresh();
    assert_eq!(h.pc("desktop"), PcState::Offline);
    let lost = [Notice::PcLost {
        pc: "Desktop".into(),
    }];
    assert_eq!(h.notices(), lost);
    h.refresh();
    assert_eq!(h.notices(), lost);
}

// Python: test_pc_going_down_after_power_command_is_silent
#[test]
fn pc_going_down_after_power_command_is_silent() {
    for action in ACTIONS {
        let mut h = up_and_online();
        h.power_pc("desktop", action);
        h.clear_notices();
        h.reachable(PC, false);
        for _ in 0..FAILURES {
            h.refresh();
        }
        assert_eq!(h.pc("desktop"), PcState::Offline);
        assert!(h.notices().is_empty());
    }
}

// Python: test_pc_lost_is_notified_again_after_it_was_seen_online
#[test]
fn pc_lost_is_notified_again_after_it_was_seen_online() {
    let mut h = up_and_online();
    h.power_pc("desktop", PowerAction::Sleep);
    h.reachable(PC, false);
    for _ in 0..FAILURES {
        h.refresh();
    }
    h.reachable(PC, true); // woken up by hand
    h.refresh();
    h.clear_notices();
    h.reachable(PC, false);
    for _ in 0..FAILURES {
        h.refresh();
    }
    assert_eq!(
        h.notices(),
        [Notice::PcLost {
            pc: "Desktop".into()
        }]
    );
}

// Python: test_failed_power_command_does_not_silence_pc_lost
#[test]
fn failed_power_command_does_not_silence_pc_lost() {
    let mut h = up_and_online();
    h.power.fail_with(Some("Permission denied"));
    h.power_pc("desktop", PowerAction::Shutdown);
    h.clear_notices();
    h.reachable(PC, false);
    for _ in 0..FAILURES {
        h.refresh();
    }
    assert_eq!(
        h.notices(),
        [Notice::PcLost {
            pc: "Desktop".into()
        }]
    );
}

// Python: test_pc_lost_is_silent_when_tunnel_goes_down
#[test]
fn pc_lost_is_silent_when_tunnel_goes_down() {
    let mut h = up_and_online();
    h.tunnel.set_running(&tunnel_id("home"), false);
    h.reachable(PC, false);
    h.refresh();
    assert_eq!(status(&h), DOWN_UNKNOWN);
    assert!(
        !h.notices()
            .iter()
            .any(|n| matches!(n, Notice::PcLost { .. }))
    );
}

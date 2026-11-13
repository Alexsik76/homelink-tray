//! Applying a new configuration to a running engine (`Handle::reload`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use homelink_core::domain::{PcState, TunnelState};
use homelink_core::engine::{Deps, Engine};
use homelink_core::notice::Notice;
use homelink_core::ports::{SystemClock, WakeRegistry};
use homelink_core::snapshot::{DisplayState, Snapshot};
use homelink_core::testing::{
    FakeNetwork, FakePower, FakeTunnel, Harness, InlineExecutor, multi_config, pc_id,
    single_config, tunnel_id,
};

fn pc_names(snapshot: &Snapshot) -> Vec<String> {
    snapshot
        .groups
        .iter()
        .flat_map(|g| g.pcs.iter().map(|p| p.name.clone()))
        .collect()
}

#[test]
fn a_running_tunnel_and_an_online_pc_are_found_again_without_a_notice() {
    let mut h = Harness::new(single_config());
    h.reachable("192.168.88.1", true);
    h.reachable("192.168.88.10", true);
    h.tunnel.set_running(&tunnel_id("home"), true);
    h.refresh();
    assert_eq!(h.tunnel_state("home"), TunnelState::Up);
    assert_eq!(h.pc("desktop"), PcState::Online);
    h.clear_notices();

    h.reload(single_config());
    h.refresh();
    assert_eq!(h.tunnel_state("home"), TunnelState::Up);
    assert_eq!(h.pc("desktop"), PcState::Online);
    assert!(h.notices().is_empty(), "{:?}", h.notices());
}

#[test]
fn removed_entities_disappear_and_new_ones_appear_in_the_next_snapshot() {
    let mut h = Harness::new(multi_config());
    h.refresh();
    let mut smaller = multi_config();
    smaller.pcs.retain(|p| p.id != pc_id("nas"));
    h.reload(smaller);
    let last = h.sink.snapshots().pop().unwrap();
    assert!(!pc_names(&last).contains(&"NAS".to_owned()), "{last:?}");
    assert_eq!(pc_names(&last).len(), 3);

    h.reload(multi_config());
    let last = h.sink.snapshots().pop().unwrap();
    assert!(pc_names(&last).contains(&"NAS".to_owned()));
}

#[test]
fn a_result_of_a_job_started_before_the_reload_is_ignored() {
    let mut h = Harness::manual(single_config());
    h.tunnel.fail_up_with(Some("boom"));
    // The start is queued but not run.
    h.toggle_tunnel("home");
    assert!(h.jobs.as_ref().unwrap().pending() > 0);
    h.reload(single_config());
    h.run_jobs();
    h.run_jobs();
    assert!(
        !h.notices()
            .iter()
            .any(|n| matches!(n, Notice::TunnelFailed { .. })),
        "{:?}",
        h.notices()
    );
    let last = h.sink.snapshots().pop().unwrap();
    let tunnel = last.groups[0].tunnel.as_ref().unwrap();
    assert_ne!(tunnel.display, DisplayState::Fail);
}

struct ChannelSink {
    snapshots: Mutex<mpsc::Sender<Snapshot>>,
}

impl homelink_core::ports::Sink for ChannelSink {
    fn publish(&self, snapshot: Snapshot) {
        let _ = self.snapshots.lock().unwrap().send(snapshot);
    }

    fn notify(&self, _notice: Notice) {}
}

#[test]
fn the_engine_thread_keeps_its_handle_across_a_reload() {
    let (tx, rx) = mpsc::channel();
    let net = Arc::new(FakeNetwork::default());
    let deps = Deps {
        tunnel: Arc::new(FakeTunnel::default()),
        pinger: net.clone(),
        ports: net,
        wake: Arc::new(WakeRegistry::new()),
        power: Arc::new(FakePower::default()),
        clock: Arc::new(SystemClock::new()),
        executor: Arc::new(InlineExecutor),
        sink: Arc::new(ChannelSink {
            snapshots: Mutex::new(tx),
        }),
    };
    let (engine, handle) = Engine::new(single_config(), deps);
    let thread = engine.spawn().unwrap();
    let first = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(pc_names(&first).len(), 1);

    assert!(handle.reload(multi_config()));
    let mut seen = 0;
    while let Ok(snapshot) = rx.recv_timeout(Duration::from_secs(10)) {
        seen = pc_names(&snapshot).len();
        if seen == 4 {
            break;
        }
    }
    assert_eq!(seen, 4);
    handle.stop();
    thread.join().unwrap();
    assert!(!handle.reload(single_config()));
}

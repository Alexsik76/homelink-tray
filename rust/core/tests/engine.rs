//! The engine with real threads (a worker pool and the engine thread) over the fakes, and
//! panic isolation. Time is real but nothing waits for it: a started tunnel answers at once.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use homelink_core::domain::{Pc, PcState, PowerAction};
use homelink_core::engine::{Deps, Engine, ThreadPool};
use homelink_core::machine::Command;
use homelink_core::notice::Notice;
use homelink_core::ports::{Clock, PowerError, PowerRunner, Sink, SystemClock, WakeRegistry};
use homelink_core::snapshot::Snapshot;
use homelink_core::testing::{
    FakeNetwork, FakePower, FakeTunnel, FakeWake, Harness, InlineExecutor, RecordingSink, pc_id,
    single_config, tunnel_id,
};

const WAIT: Duration = Duration::from_secs(10);

/// A sink that forwards to channels so a test can wait for events.
struct ChannelSink {
    snapshots: Mutex<mpsc::Sender<Snapshot>>,
    notices: Mutex<mpsc::Sender<Notice>>,
}

impl Sink for ChannelSink {
    fn publish(&self, snapshot: Snapshot) {
        let _ = self.snapshots.lock().unwrap().send(snapshot);
    }

    fn notify(&self, notice: Notice) {
        let _ = self.notices.lock().unwrap().send(notice);
    }
}

fn wake_registry(wake: &Arc<FakeWake>) -> Arc<WakeRegistry> {
    let mut registry = WakeRegistry::new();
    for kind in [
        homelink_core::domain::WakeKind::MikrotikRest,
        homelink_core::domain::WakeKind::MagicPacket,
        homelink_core::domain::WakeKind::Command,
    ] {
        registry.register(kind, wake.clone());
    }
    Arc::new(registry)
}

#[test]
fn the_engine_thread_and_worker_pool_wake_a_pc_end_to_end() {
    let (snap_tx, snapshots) = mpsc::channel();
    let (note_tx, _notices) = mpsc::channel();
    let tunnel = Arc::new(FakeTunnel::default());
    let net = Arc::new(FakeNetwork::default());
    let wake = Arc::new(FakeWake::default());
    net.set("192.168.88.1", true);
    let deps = Deps {
        tunnel: tunnel.clone(),
        pinger: net.clone(),
        ports: net.clone(),
        wake: wake_registry(&wake),
        power: Arc::new(FakePower::default()),
        clock: Arc::new(SystemClock::new()),
        executor: Arc::new(ThreadPool::new(ThreadPool::DEFAULT_WORKERS)),
        sink: Arc::new(ChannelSink {
            snapshots: Mutex::new(snap_tx),
            notices: Mutex::new(note_tx),
        }),
    };
    let (engine, handle) = Engine::new(single_config(), deps);
    let thread = engine.spawn().unwrap();

    assert!(handle.send(Command::Wake(pc_id("desktop"))));
    let mut waking = false;
    while let Ok(snapshot) = snapshots.recv_timeout(WAIT) {
        if snapshot.groups[0].pcs[0].state == PcState::Waking {
            waking = true;
            break;
        }
    }
    assert!(waking, "the PC never reached Waking");
    assert_eq!(wake.woken(), [pc_id("desktop")]);
    assert_eq!(tunnel.ups(), [tunnel_id("home")]);

    handle.stop();
    thread.join().unwrap();
    assert!(
        !handle.send(Command::Wake(pc_id("desktop"))),
        "the engine has stopped"
    );
}

#[test]
fn stopping_an_idle_engine_returns_promptly() {
    let sink = Arc::new(RecordingSink::default());
    let net = Arc::new(FakeNetwork::default());
    let deps = Deps {
        tunnel: Arc::new(FakeTunnel::default()),
        pinger: net.clone(),
        ports: net,
        wake: Arc::new(WakeRegistry::new()),
        power: Arc::new(FakePower::default()),
        clock: Arc::new(SystemClock::new()),
        executor: Arc::new(InlineExecutor),
        sink: sink.clone(),
    };
    let (engine, handle) = Engine::new(single_config(), deps);
    let thread = engine.spawn().unwrap();
    handle.stop();
    thread.join().unwrap();
    assert!(
        !sink.snapshots().is_empty(),
        "the initial state was published"
    );
}

/// A power runner that panics.
struct PanickingPower;

impl PowerRunner for PanickingPower {
    fn run(&self, _pc: &Pc, _action: PowerAction) -> Result<(), PowerError> {
        panic!("adapter bug");
    }
}

#[test]
fn a_panicking_job_becomes_a_failed_job_and_the_engine_carries_on() {
    let sink = Arc::new(RecordingSink::default());
    let net = Arc::new(FakeNetwork::default());
    net.set("192.168.88.1", true);
    net.set("192.168.88.10", true);
    let tunnel = Arc::new(FakeTunnel::default());
    tunnel.set_running(&tunnel_id("home"), true);
    let clock: Arc<dyn Clock> = Arc::new(homelink_core::testing::FakeClock::default());
    let deps = Deps {
        tunnel,
        pinger: net.clone(),
        ports: net,
        wake: Arc::new(WakeRegistry::new()),
        power: Arc::new(PanickingPower),
        clock,
        executor: Arc::new(InlineExecutor),
        sink: sink.clone(),
    };
    let (mut engine, handle) = Engine::new(single_config(), deps);
    engine.pump();
    assert_eq!(
        engine.machine().pc_state(&pc_id("desktop")),
        Some(PcState::Online)
    );

    handle.send(Command::Power(pc_id("desktop"), PowerAction::Sleep));
    engine.pump();
    assert_eq!(
        sink.notices(),
        [Notice::PowerFailed {
            pc: "Desktop".into(),
            detail: "internal error".into()
        }]
    );
    // The PC is not stuck busy and the engine still answers.
    handle.send(Command::Power(pc_id("desktop"), PowerAction::Shutdown));
    engine.pump();
    assert_eq!(sink.notices().len(), 2);
}

#[test]
fn the_harness_is_deterministic() {
    // Same script twice, same result: nothing depends on real time.
    let run = || {
        let mut h = Harness::new(single_config());
        h.reachable("192.168.88.1", true);
        h.wake_pc("desktop");
        h.reachable("192.168.88.10", true);
        h.refresh();
        (h.sink.snapshots(), h.notices())
    };
    assert_eq!(run(), run());
}

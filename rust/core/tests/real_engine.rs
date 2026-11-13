//! The real engine, thread pool and portable adapters against local stand-ins: a PC that "wakes"
//! when a listener appears on its port, a UDP socket that receives the magic packet, and a fake
//! `ssh` script. Only the tunnel driver and the pinger are fakes (they are Windows adapters).
//! Takes a few seconds because the engine polls in real time (one second at the shortest).
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::net::{Ipv4Addr, TcpListener, UdpSocket};
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use homelink_core::adapters::command::StdCommandRunner;
use homelink_core::adapters::default_wake_registry;
use homelink_core::adapters::magic::MAGIC_PACKET_LEN;
use homelink_core::adapters::power::CommandPowerRunner;
use homelink_core::adapters::tcp::TcpPortProbe;
use homelink_core::config;
use homelink_core::domain::{PcState, PowerAction};
use homelink_core::engine::{Deps, Engine, ThreadPool};
use homelink_core::machine::Command;
use homelink_core::notice::Notice;
use homelink_core::ports::{Sink, SystemClock};
use homelink_core::snapshot::Snapshot;
use homelink_core::testing::{FakeNetwork, FakeTunnel, pc_id};

const WAIT: Duration = Duration::from_secs(10);

enum Event {
    State(PcState),
    Notice(Notice),
}

struct ChannelSink(Mutex<Sender<Event>>);

impl Sink for ChannelSink {
    fn publish(&self, snapshot: Snapshot) {
        let state = snapshot.groups[0].pcs[0].state;
        let _ = self.0.lock().unwrap().send(Event::State(state));
    }

    fn notify(&self, notice: Notice) {
        let _ = self.0.lock().unwrap().send(Event::Notice(notice));
    }
}

/// Waits for an event that `pick` accepts.
fn wait_for<T>(events: &Receiver<Event>, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Ok(event) = events.recv_timeout(left)
            && let Some(found) = pick(&event)
        {
            return found;
        }
    }
    panic!("the expected event did not happen in {WAIT:?}");
}

#[test]
fn the_engine_wakes_polls_and_powers_a_pc_through_the_real_adapters() {
    // The PC: a TCP port that is closed now and opens when the "PC" has woken up.
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let pc_port = probe.local_addr().unwrap().port();
    drop(probe);
    // Where the magic packet goes.
    let magic = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    magic.set_read_timeout(Some(WAIT)).unwrap();
    let magic_port = magic.local_addr().unwrap().port();
    // A fake `ssh` that records its arguments.
    let dir = std::env::temp_dir().join(format!("homelink-real-engine-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("ssh.argv");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
        log.display()
    );
    std::fs::write(dir.join("ssh"), script).unwrap();
    std::fs::set_permissions(dir.join("ssh"), std::fs::Permissions::from_mode(0o755)).unwrap();

    let text = format!(
        r#"
version = 1
language = "en"

[log]
file = "homelink.log"
level = "info"

[defaults]
poll_seconds = 1
wake_poll_seconds = 1
port_timeout_ms = 500
offline_after_failures = 2
wake_timeout = 30
power_timeout = 5

[[pcs]]
id = "box"
name = "Box"
os = "linux"
host = "127.0.0.1"
ports = [{pc_port}]

[pcs.wake]
method = "magic_packet"
mac = "AA:BB:CC:DD:EE:FF"
address = "127.0.0.1"
port = {magic_port}

[pcs.power]
method = "ssh"
user = "alex"
port = 22
identity_file = '/tmp/homelink-test-key'
"#
    );
    let config = config::parse(&text).unwrap().config;

    let runner = Arc::new(StdCommandRunner::new().with_program_dir(&dir));
    let net = Arc::new(FakeNetwork::default());
    let (tx, events) = mpsc::channel();
    let deps = Deps {
        tunnel: Arc::new(FakeTunnel::default()), // no PC uses a tunnel
        pinger: net,
        ports: Arc::new(TcpPortProbe),
        wake: Arc::new(default_wake_registry(runner.clone())),
        power: Arc::new(CommandPowerRunner::new(runner)),
        clock: Arc::new(SystemClock::new()),
        executor: Arc::new(ThreadPool::new(ThreadPool::DEFAULT_WORKERS)),
        sink: Arc::new(ChannelSink(Mutex::new(tx))),
    };
    let (engine, handle) = Engine::new(config, deps);
    let thread = engine.spawn().unwrap();
    let id = pc_id("box");

    // 1. The first poll finds the port closed.
    wait_for(&events, |e| {
        matches!(e, Event::State(PcState::Offline)).then_some(())
    });

    // 2. Wake: the magic packet really arrives, and the PC shows as waking.
    assert!(handle.send(Command::Wake(id.clone())));
    let mut buffer = [0u8; 256];
    let (n, _) = magic.recv_from(&mut buffer).unwrap();
    assert_eq!(n, MAGIC_PACKET_LEN);
    assert_eq!(&buffer[..6], &[0xFF; 6]);
    assert_eq!(&buffer[6..12], &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    wait_for(&events, |e| {
        matches!(e, Event::State(PcState::Waking)).then_some(())
    });

    // 3. The "PC" comes up: the port opens and the engine reports it online.
    let _listener = TcpListener::bind((Ipv4Addr::LOCALHOST, pc_port)).unwrap();
    wait_for(&events, |e| {
        matches!(e, Event::State(PcState::Online)).then_some(())
    });
    wait_for(&events, |e| {
        matches!(e, Event::Notice(Notice::PcOnline { pc, .. }) if pc == "Box").then_some(())
    });

    // 4. Sleep goes out over `ssh` with the exact command line.
    assert!(handle.send(Command::Power(id, PowerAction::Sleep)));
    wait_for(&events, |e| {
        matches!(e, Event::Notice(Notice::SleepSent { pc, .. }) if pc == "Box").then_some(())
    });
    let args: Vec<String> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        args.last().map(String::as_str),
        Some("sudo systemctl suspend -i")
    );
    assert!(args.contains(&"/tmp/homelink-test-key".to_owned()));
    assert!(args.contains(&"IdentitiesOnly=yes".to_owned()));

    handle.stop();
    thread.join().unwrap();
}

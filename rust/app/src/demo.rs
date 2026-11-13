//! Demo wiring: the engine on the core test fakes, with delays and a PC that goes offline on
//! its own, so the states visibly change. It is what `--demo` runs until the Windows
//! adapters exist; the same [`Deps`] bundle is where they will plug in.
//!
//! [`cycle`] also sends commands on a timer, with the failure switches of the fakes set for
//! some of them, so that every display state appears: working, in progress, off, unknown
//! (before the first check) and failed.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use homelink_core::config::Config;
use homelink_core::domain::{Pc, PowerAction, Tunnel, WakeKind};
use homelink_core::engine::{Deps, Handle, ThreadPool};
use homelink_core::machine::Command;
use homelink_core::ports::{
    CommandOutput, CommandRunner, Pinger, PortProbe, PowerError, PowerRunner, Sink, SystemClock,
    TunnelDriver, TunnelError, WakeError, WakeRegistry, WakeRequest, WakeSender,
};
use homelink_core::testing::{
    FakeCommandRunner, FakeNetwork, FakePower, FakeTunnel, FakeWake, pc_id, tunnel_id,
};

/// How long a tunnel takes to come up.
const TUNNEL_UP_DELAY: Duration = Duration::from_secs(3);
/// How long a PC takes to answer after a wake request.
const WAKE_DELAY: Duration = Duration::from_secs(6);
/// How long a PC takes to stop answering after a sleep or shutdown command.
const POWER_DELAY: Duration = Duration::from_secs(3);
/// The PC without a tunnel goes away for a while, on its own, every cycle.
const FLAP_ONLINE: Duration = Duration::from_secs(40);
const FLAP_OFFLINE: Duration = Duration::from_secs(20);
/// Time between two steps of the cycle: longer than a wake-up and a poll.
const STEP: Duration = Duration::from_secs(15);
/// The reason the failure switches report.
const FAILURE: &str = "demo: the failure switch is on";

/// PCs that are on when the demo starts (the others start off).
const STARTS_ON: [&str; 2] = ["desktop", "nas"];

/// The fakes behind the demo, shared with the cycle that drives them.
#[derive(Clone, Default)]
pub(crate) struct Fakes {
    net: Arc<FakeNetwork>,
    tunnel: Arc<FakeTunnel>,
    wake: Arc<FakeWake>,
    power: Arc<FakePower>,
}

/// A tunnel that takes time to start; its probe host answers while it runs.
struct DemoTunnel {
    inner: Arc<FakeTunnel>,
    net: Arc<FakeNetwork>,
}

impl TunnelDriver for DemoTunnel {
    fn is_running(&self, tunnel: &Tunnel) -> Result<bool, TunnelError> {
        self.inner.is_running(tunnel)
    }

    fn up(&self, tunnel: &Tunnel) -> Result<(), TunnelError> {
        thread::sleep(TUNNEL_UP_DELAY);
        self.inner.up(tunnel)?;
        self.net.set(tunnel.probe_host.as_str(), true);
        Ok(())
    }

    fn down(&self, tunnel: &Tunnel) -> Result<(), TunnelError> {
        self.inner.down(tunnel)?;
        self.net.set(tunnel.probe_host.as_str(), false);
        Ok(())
    }
}

/// Wake requests that take effect a little later, as a real PC does.
struct DemoWake {
    inner: Arc<FakeWake>,
    net: Arc<FakeNetwork>,
}

impl WakeSender for DemoWake {
    fn wake(&self, request: &WakeRequest<'_>) -> Result<(), WakeError> {
        self.inner.wake(request)?;
        let net = Arc::clone(&self.net);
        let host = request.pc.host.to_string();
        thread::spawn(move || {
            thread::sleep(WAKE_DELAY);
            net.set(&host, true);
        });
        Ok(())
    }
}

/// Power commands that take effect a little later.
struct DemoPower {
    inner: Arc<FakePower>,
    net: Arc<FakeNetwork>,
}

impl PowerRunner for DemoPower {
    fn run(&self, pc: &Pc, action: PowerAction) -> Result<(), PowerError> {
        self.inner.run(pc, action)?;
        let net = Arc::clone(&self.net);
        let host = pc.host.to_string();
        thread::spawn(move || {
            thread::sleep(POWER_DELAY);
            net.set(&host, false);
        });
        Ok(())
    }
}

/// Switches the host of a PC off and on again, forever, so that it goes offline unasked.
fn flap(net: Arc<FakeNetwork>, host: String) {
    thread::spawn(move || {
        loop {
            thread::sleep(FLAP_ONLINE);
            net.set(&host, false);
            thread::sleep(FLAP_OFFLINE);
            net.set(&host, true);
        }
    });
}

impl Fakes {
    /// The fake network as the port probe of the settings' checks, so that they see the same
    /// PCs as the engine.
    pub(crate) fn ports(&self) -> Arc<dyn PortProbe> {
        Arc::clone(&self.net) as Arc<dyn PortProbe>
    }
}

/// The command runner of demo mode for the settings: no program is started, and every `ssh`
/// check reports success, so that the list of the checks shows its results.
pub(crate) fn runner() -> Arc<dyn CommandRunner> {
    let runner = FakeCommandRunner::default();
    runner.set_result(CommandOutput::exited(0, ""));
    Arc::new(runner)
}

/// The dependencies of the engine for `config` in demo mode. Fake ports, real clock and a
/// real worker pool, so that a slow job never blocks the UI.
pub(crate) fn deps(config: &Config, sink: Arc<dyn Sink>, fakes: &Fakes) -> Deps {
    let net = Arc::clone(&fakes.net);
    for pc in &config.pcs {
        if STARTS_ON.contains(&pc.id.as_str()) {
            net.set(pc.host.as_str(), true);
        }
    }
    if let Some(pc) = config.pcs.iter().find(|pc| pc.tunnel.is_none()) {
        flap(Arc::clone(&net), pc.host.to_string());
    }
    let wake = Arc::new(DemoWake {
        inner: Arc::clone(&fakes.wake),
        net: Arc::clone(&net),
    });
    let mut registry = WakeRegistry::new();
    for kind in [WakeKind::Relay, WakeKind::MagicPacket, WakeKind::Command] {
        registry.register(kind, Arc::clone(&wake) as Arc<dyn WakeSender>);
    }
    Deps {
        tunnel: Arc::new(DemoTunnel {
            inner: Arc::clone(&fakes.tunnel),
            net: Arc::clone(&net),
        }),
        pinger: Arc::clone(&net) as Arc<dyn Pinger>,
        ports: Arc::clone(&net) as Arc<dyn PortProbe>,
        wake: Arc::new(registry),
        power: Arc::new(DemoPower {
            inner: Arc::clone(&fakes.power),
            net,
        }),
        clock: Arc::new(SystemClock::new()),
        executor: Arc::new(ThreadPool::new(ThreadPool::DEFAULT_WORKERS)),
        sink,
    }
}

/// One step of the cycle: a failure switch, or a command to the engine.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Raising the tunnel `lab` fails while on.
    TunnelFail(bool),
    /// Waking `build` fails while on.
    WakeFail(bool),
    /// Sleeping `build` fails while on.
    PowerFail(bool),
    /// Toggle the tunnel `lab`.
    ToggleTunnel,
    /// Wake `build`.
    Wake,
    /// Put `build` to sleep.
    Sleep,
}

/// One round of the cycle, in the order that makes each state show. The tunnel `lab` starts
/// down and ends down; `build` is woken by the wake steps and asleep at the end.
const CYCLE: [Step; 14] = [
    Step::TunnelFail(true),
    Step::ToggleTunnel, // raising fails: failed
    Step::TunnelFail(false),
    Step::ToggleTunnel, // raised: in progress, then working
    Step::ToggleTunnel, // lowered: off
    Step::WakeFail(true),
    Step::Wake, // raises the tunnel, the wake fails: failed
    Step::WakeFail(false),
    Step::Wake, // woken: in progress, then working
    Step::PowerFail(true),
    Step::Sleep, // fails
    Step::PowerFail(false),
    Step::Sleep,        // in progress, then off
    Step::ToggleTunnel, // lowers the tunnel that the wake raised
];

/// Runs [`CYCLE`] over and over on a thread, until the engine stops.
pub(crate) fn cycle(handle: Handle, fakes: Fakes) {
    thread::spawn(move || {
        let lab = tunnel_id("lab");
        let build = pc_id("build");
        loop {
            for step in CYCLE {
                // A switch changes at once; a command is followed by a pause.
                let sent = match step {
                    Step::TunnelFail(on) => {
                        fakes.tunnel.fail_up_for(&lab, on.then_some(FAILURE));
                        continue;
                    }
                    Step::WakeFail(on) => {
                        fakes.wake.fail_for(&build, on.then_some(FAILURE));
                        continue;
                    }
                    Step::PowerFail(on) => {
                        fakes.power.fail_for(&build, on.then_some(FAILURE));
                        continue;
                    }
                    Step::ToggleTunnel => handle.send(Command::ToggleTunnel(lab.clone())),
                    Step::Wake => handle.send(Command::Wake(build.clone())),
                    Step::Sleep => handle.send(Command::Power(build.clone(), PowerAction::Sleep)),
                };
                if !sent {
                    // The engine has stopped: the demo is over.
                    return;
                }
                thread::sleep(STEP);
            }
        }
    });
}

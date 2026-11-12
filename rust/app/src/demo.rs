//! Demo wiring: the engine on the core test fakes, with delays and a PC that goes offline on
//! its own, so the states visibly change. It is what `--demo` runs until the Windows
//! adapters exist; the same [`Deps`] bundle is where they will plug in.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use homelink_core::config::Config;
use homelink_core::domain::{Pc, PowerAction, Tunnel, WakeKind};
use homelink_core::engine::{Deps, ThreadPool};
use homelink_core::ports::{
    Pinger, PortProbe, PowerError, PowerRunner, Sink, SystemClock, TunnelDriver, TunnelError,
    WakeError, WakeRegistry, WakeRequest, WakeSender,
};
use homelink_core::testing::{FakeNetwork, FakePower, FakeTunnel, FakeWake};

/// How long a tunnel takes to come up.
const TUNNEL_UP_DELAY: Duration = Duration::from_secs(3);
/// How long a PC takes to answer after a wake request.
const WAKE_DELAY: Duration = Duration::from_secs(6);
/// How long a PC takes to stop answering after a sleep or shutdown command.
const POWER_DELAY: Duration = Duration::from_secs(3);
/// The PC without a tunnel goes away for a while, on its own, every cycle.
const FLAP_ONLINE: Duration = Duration::from_secs(40);
const FLAP_OFFLINE: Duration = Duration::from_secs(20);

/// PCs that are on when the demo starts (the others start off).
const STARTS_ON: [&str; 2] = ["desktop", "nas"];

/// A tunnel that takes time to start; its probe host answers while it runs.
struct DemoTunnel {
    inner: FakeTunnel,
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
    inner: FakeWake,
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
    inner: FakePower,
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

/// The dependencies of the engine for `config` in demo mode. Fake ports, real clock and a
/// real worker pool, so that a slow job never blocks the UI.
pub(crate) fn deps(config: &Config, sink: Arc<dyn Sink>) -> Deps {
    let net = Arc::new(FakeNetwork::default());
    for pc in &config.pcs {
        if STARTS_ON.contains(&pc.id.as_str()) {
            net.set(pc.host.as_str(), true);
        }
    }
    if let Some(pc) = config.pcs.iter().find(|pc| pc.tunnel.is_none()) {
        flap(Arc::clone(&net), pc.host.to_string());
    }
    let wake = Arc::new(DemoWake {
        inner: FakeWake::default(),
        net: Arc::clone(&net),
    });
    let mut registry = WakeRegistry::new();
    for kind in [WakeKind::Relay, WakeKind::MagicPacket, WakeKind::Command] {
        registry.register(kind, Arc::clone(&wake) as Arc<dyn WakeSender>);
    }
    Deps {
        tunnel: Arc::new(DemoTunnel {
            inner: FakeTunnel::default(),
            net: Arc::clone(&net),
        }),
        pinger: Arc::clone(&net) as Arc<dyn Pinger>,
        ports: Arc::clone(&net) as Arc<dyn PortProbe>,
        wake: Arc::new(registry),
        power: Arc::new(DemoPower {
            inner: FakePower::default(),
            net: Arc::clone(&net),
        }),
        clock: Arc::new(SystemClock::new()),
        executor: Arc::new(ThreadPool::new(ThreadPool::DEFAULT_WORKERS)),
        sink,
    }
}

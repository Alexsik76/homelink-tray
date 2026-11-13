//! Fakes and helpers for tests (feature `testing`).
//!
//! Nothing here touches the network, the clock or the file system: tests are deterministic
//! and fast. [`Harness`] wires an [`Engine`] to the fakes with an inline executor and a
//! manual clock.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::config::{self, Config};
use crate::domain::Host;
use crate::domain::{Pc, PcId, PcState, PowerAction, Tunnel, TunnelId, TunnelState, WakeKind};
use crate::engine::{Deps, Engine, Handle};
use crate::machine::{Command, START_POLL};
use crate::notice::Notice;
use crate::ports::{
    Clock, CommandOutput, CommandRunner, Executor, Pinger, PortProbe, PowerError, PowerRunner,
    Sink, Time, TunnelDriver, TunnelError, WakeError, WakeRegistry, WakeRequest, WakeSender,
};
use crate::snapshot::Snapshot;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().expect("test fake lock poisoned")
}

/// A [`CommandRunner`] that records what it is asked to run and returns a set result
/// (success with no output by default).
#[derive(Default)]
pub struct FakeCommandRunner {
    calls: Mutex<Vec<(Vec<String>, Duration)>>,
    result: Mutex<Option<CommandOutput>>,
}

impl FakeCommandRunner {
    /// Sets what every later run returns.
    pub fn set_result(&self, output: CommandOutput) {
        *lock(&self.result) = Some(output);
    }

    /// The command lines and timeouts run so far.
    pub fn calls(&self) -> Vec<(Vec<String>, Duration)> {
        lock(&self.calls).clone()
    }
}

impl CommandRunner for FakeCommandRunner {
    fn run(&self, argv: &[String], timeout: Duration) -> CommandOutput {
        lock(&self.calls).push((argv.to_vec(), timeout));
        lock(&self.result)
            .clone()
            .unwrap_or_else(|| CommandOutput::exited(0, ""))
    }
}

/// A clock that only moves when told to.
#[derive(Default)]
pub struct FakeClock {
    now: Mutex<Time>,
}

impl FakeClock {
    /// Moves time forward.
    pub fn advance(&self, by: Duration) {
        let mut now = lock(&self.now);
        *now = *now + by;
    }

    /// Moves time forward to `at`; does nothing if `at` is not later.
    pub fn advance_to(&self, at: Time) {
        let mut now = lock(&self.now);
        if at > *now {
            *now = at;
        }
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Time {
        *lock(&self.now)
    }
}

/// A [`TunnelDriver`] with a settable running state per tunnel.
#[derive(Default)]
pub struct FakeTunnel {
    running: Mutex<HashSet<TunnelId>>,
    up_error: Mutex<Option<String>>,
    down_error: Mutex<Option<String>>,
    up_errors: Mutex<HashMap<TunnelId, String>>,
    down_errors: Mutex<HashMap<TunnelId, String>>,
    ups: Mutex<Vec<TunnelId>>,
    downs: Mutex<Vec<TunnelId>>,
}

/// Sets or clears the entry of `id` in a per-entity error map.
fn set_error<K: std::hash::Hash + Eq + Clone>(
    map: &Mutex<HashMap<K, String>>,
    id: &K,
    reason: Option<&str>,
) {
    let mut map = lock(map);
    match reason {
        Some(reason) => {
            map.insert(id.clone(), reason.to_owned());
        }
        None => {
            map.remove(id);
        }
    }
}

impl FakeTunnel {
    /// Sets whether a tunnel service runs (as if started or stopped from outside).
    pub fn set_running(&self, id: &TunnelId, running: bool) {
        let mut set = lock(&self.running);
        if running {
            set.insert(id.clone());
        } else {
            set.remove(id);
        }
    }

    /// Whether a tunnel service runs.
    pub fn is_up(&self, id: &TunnelId) -> bool {
        lock(&self.running).contains(id)
    }

    /// Makes every start fail with this reason (`None` to stop failing).
    pub fn fail_up_with(&self, reason: Option<&str>) {
        *lock(&self.up_error) = reason.map(str::to_owned);
    }

    /// Makes every stop fail with this reason (`None` to stop failing). The tunnel keeps
    /// running, as when a real stop fails.
    pub fn fail_down_with(&self, reason: Option<&str>) {
        *lock(&self.down_error) = reason.map(str::to_owned);
    }

    /// Makes the start of one tunnel fail with this reason (`None` to stop failing).
    pub fn fail_up_for(&self, id: &TunnelId, reason: Option<&str>) {
        set_error(&self.up_errors, id, reason);
    }

    /// Makes the stop of one tunnel fail with this reason (`None` to stop failing).
    pub fn fail_down_for(&self, id: &TunnelId, reason: Option<&str>) {
        set_error(&self.down_errors, id, reason);
    }

    /// Tunnels started so far.
    pub fn ups(&self) -> Vec<TunnelId> {
        lock(&self.ups).clone()
    }

    /// Tunnels stopped so far.
    pub fn downs(&self) -> Vec<TunnelId> {
        lock(&self.downs).clone()
    }
}

impl TunnelDriver for FakeTunnel {
    fn is_running(&self, tunnel: &Tunnel) -> Result<bool, TunnelError> {
        Ok(self.is_up(&tunnel.id))
    }

    fn up(&self, tunnel: &Tunnel) -> Result<(), TunnelError> {
        lock(&self.ups).push(tunnel.id.clone());
        let reason = lock(&self.up_error)
            .clone()
            .or_else(|| lock(&self.up_errors).get(&tunnel.id).cloned());
        if let Some(reason) = reason {
            return Err(TunnelError(reason));
        }
        self.set_running(&tunnel.id, true);
        Ok(())
    }

    fn down(&self, tunnel: &Tunnel) -> Result<(), TunnelError> {
        lock(&self.downs).push(tunnel.id.clone());
        let reason = lock(&self.down_error)
            .clone()
            .or_else(|| lock(&self.down_errors).get(&tunnel.id).cloned());
        if let Some(reason) = reason {
            return Err(TunnelError(reason));
        }
        self.set_running(&tunnel.id, false);
        Ok(())
    }
}

/// A [`Pinger`] and a [`PortProbe`] over a set of reachable hosts.
#[derive(Default)]
pub struct FakeNetwork {
    reachable: Mutex<HashSet<String>>,
}

impl FakeNetwork {
    /// Makes a host reachable or not (both for ping and for all of its ports).
    pub fn set(&self, host: &str, reachable: bool) {
        let mut set = lock(&self.reachable);
        if reachable {
            set.insert(host.to_owned());
        } else {
            set.remove(host);
        }
    }

    fn contains(&self, host: &Host) -> bool {
        lock(&self.reachable).contains(host.as_str())
    }
}

impl Pinger for FakeNetwork {
    fn ping(&self, host: &Host, _timeout: Duration) -> bool {
        self.contains(host)
    }
}

impl PortProbe for FakeNetwork {
    fn all_open(&self, host: &Host, _ports: &[u16], _timeout: Duration) -> bool {
        self.contains(host)
    }
}

/// A [`WakeSender`] that records requests.
#[derive(Default)]
pub struct FakeWake {
    calls: Mutex<Vec<(PcId, WakeKind)>>,
    error: Mutex<Option<String>>,
    errors: Mutex<HashMap<PcId, String>>,
}

impl FakeWake {
    /// Makes the wake requests for one PC fail with this reason (`None` to stop failing).
    pub fn fail_for(&self, pc: &PcId, reason: Option<&str>) {
        set_error(&self.errors, pc, reason);
    }

    /// Makes every request fail with this reason (`None` to stop failing).
    pub fn fail_with(&self, reason: Option<&str>) {
        *lock(&self.error) = reason.map(str::to_owned);
    }

    /// The PCs woken so far, in order.
    pub fn woken(&self) -> Vec<PcId> {
        lock(&self.calls).iter().map(|(pc, _)| pc.clone()).collect()
    }

    /// The requests so far.
    pub fn calls(&self) -> Vec<(PcId, WakeKind)> {
        lock(&self.calls).clone()
    }
}

impl WakeSender for FakeWake {
    fn wake(&self, request: &WakeRequest<'_>) -> Result<(), WakeError> {
        let reason = lock(&self.error)
            .clone()
            .or_else(|| lock(&self.errors).get(&request.pc.id).cloned());
        if let Some(reason) = reason {
            return Err(WakeError::Failed(reason));
        }
        lock(&self.calls).push((request.pc.id.clone(), request.method.kind()));
        Ok(())
    }
}

/// A [`PowerRunner`] that records commands.
#[derive(Default)]
pub struct FakePower {
    calls: Mutex<Vec<(PcId, PowerAction)>>,
    error: Mutex<Option<String>>,
    errors: Mutex<HashMap<PcId, String>>,
}

impl FakePower {
    /// Makes the commands for one PC fail with this reason (`None` to stop failing).
    pub fn fail_for(&self, pc: &PcId, reason: Option<&str>) {
        set_error(&self.errors, pc, reason);
    }

    /// Makes every command fail with this reason (`None` to stop failing).
    pub fn fail_with(&self, reason: Option<&str>) {
        *lock(&self.error) = reason.map(str::to_owned);
    }

    /// The commands run so far.
    pub fn calls(&self) -> Vec<(PcId, PowerAction)> {
        lock(&self.calls).clone()
    }
}

impl PowerRunner for FakePower {
    fn run(&self, pc: &Pc, action: PowerAction) -> Result<(), PowerError> {
        lock(&self.calls).push((pc.id.clone(), action));
        let reason = lock(&self.error)
            .clone()
            .or_else(|| lock(&self.errors).get(&pc.id).cloned());
        match reason {
            Some(reason) => Err(PowerError(reason)),
            None => Ok(()),
        }
    }
}

/// A [`Sink`] that keeps everything it receives.
#[derive(Default)]
pub struct RecordingSink {
    snapshots: Mutex<Vec<Snapshot>>,
    notices: Mutex<Vec<Notice>>,
}

impl RecordingSink {
    /// Every snapshot published, oldest first.
    pub fn snapshots(&self) -> Vec<Snapshot> {
        lock(&self.snapshots).clone()
    }

    /// Every notice shown, oldest first.
    pub fn notices(&self) -> Vec<Notice> {
        lock(&self.notices).clone()
    }

    /// Forgets the notices received so far.
    pub fn clear_notices(&self) {
        lock(&self.notices).clear();
    }
}

impl Sink for RecordingSink {
    fn publish(&self, snapshot: Snapshot) {
        lock(&self.snapshots).push(snapshot);
    }

    fn notify(&self, notice: Notice) {
        lock(&self.notices).push(notice);
    }
}

/// Runs jobs immediately on the calling thread.
#[derive(Default)]
pub struct InlineExecutor;

impl Executor for InlineExecutor {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// Holds jobs until a test runs them, to examine "in flight" states.
#[derive(Default)]
pub struct ManualExecutor {
    queue: Mutex<VecDeque<Job>>,
}

impl ManualExecutor {
    /// Number of jobs waiting.
    pub fn pending(&self) -> usize {
        lock(&self.queue).len()
    }

    /// Runs every waiting job.
    pub fn run_all(&self) {
        loop {
            let job = lock(&self.queue).pop_front();
            match job {
                Some(job) => job(),
                None => break,
            }
        }
    }
}

impl Executor for ManualExecutor {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        lock(&self.queue).push_back(job);
    }
}

/// The `PcId` for a slug in a test.
pub fn pc_id(s: &str) -> PcId {
    PcId::parse(s).expect("valid PC id in a test")
}

/// The `TunnelId` for a slug in a test.
pub fn tunnel_id(s: &str) -> TunnelId {
    TunnelId::parse(s).expect("valid tunnel id in a test")
}

const BASE: &str = r#"
version = 1
language = "en"

[log]
file = "homelink.log"
level = "info"

[defaults]
poll_seconds = 5
wake_poll_seconds = 2
port_timeout_ms = 1000
offline_after_failures = 3
wake_timeout = 180
power_timeout = 15

[[tunnels]]
id = "home"
name = "Home"
kind = "wireguard"
conf = 'C:\WG\home.conf.dpapi'
wireguard_exe = 'C:\WG\wireguard.exe'
probe_host = "192.168.88.1"
timeout = 20
ping_timeout_ms = 1000

[[relays]]
id = "gateway"
kind = "routeros"
host = "192.168.88.1"
port = 22
user = "homelink"
identity_file = 'C:\HomeLink\relay_key'
timeout = 8

[[pcs]]
id = "desktop"
name = "Desktop"
os = "linux"
host = "192.168.88.10"
tunnel = "home"
ports = [3389]

[pcs.wake]
method = "relay"
relay = "gateway"
mac = "AA:BB:CC:DD:EE:FF"
interface = "bridge"

[pcs.power]
method = "ssh"
user = "alex"
port = 22
identity_file = 'C:\HomeLink\pc_key'

[pcs.rdp]
user = "alex"
options = []
"#;

const MORE: &str = r#"
[[tunnels]]
id = "office"
name = "Office"
kind = "wireguard"
conf = 'C:\WG\office.conf.dpapi'
wireguard_exe = 'C:\WG\wireguard.exe'
probe_host = "10.8.0.1"
timeout = 20
ping_timeout_ms = 1000

[[pcs]]
id = "laptop"
name = "Laptop"
os = "windows"
host = "192.168.88.11"
tunnel = "home"
ports = [3389, 445]

[pcs.wake]
method = "magic_packet"
mac = "11:22:33:44:55:66"
address = "192.168.88.255"
port = 9

[pcs.power]
method = "command"
shutdown = ["ssh", "alex@{host}", "shutdown", "/s", "/t", "0"]

[[pcs]]
id = "office-pc"
name = "Office PC"
os = "linux"
host = "10.8.0.20"
tunnel = "office"
ports = [22]

[pcs.wake]
method = "command"
command = ["wakeonlan", "office"]
timeout = 10

[[pcs]]
id = "nas"
name = "NAS"
os = "linux"
host = "10.0.0.7"
ports = [22]

[pcs.wake]
method = "magic_packet"
mac = "AA:BB:CC:00:00:07"
address = "10.0.0.7"
port = 9

[pcs.power]
method = "ssh"
user = "admin"
port = 22
identity_file = 'C:\HomeLink\pc_key'
"#;

/// One tunnel (`home`, probe host `192.168.88.1`) and one PC (`desktop`, `192.168.88.10`):
/// the shape of the v0.3.0 single-PC configuration.
pub fn single_config() -> Config {
    config::parse(BASE)
        .expect("the built-in test configuration is valid")
        .config
}

/// Two tunnels and four PCs: `desktop` and `laptop` behind `home`, `office-pc` behind
/// `office`, and `nas` with no tunnel (`10.0.0.7`).
pub fn multi_config() -> Config {
    let text = format!("{BASE}{MORE}");
    config::parse(&text)
        .expect("the built-in test configuration is valid")
        .config
}

const RELAYS: &str = r#"
[[relays]]
id = "openwrt"
kind = "etherwake"
host = "192.168.10.1"
port = 22
user = "homelink"
identity_file = 'C:\HomeLink\relay_key'
timeout = 8
sudo = false

[[relays]]
id = "pi"
kind = "wakeonlan"
host = "10.0.5.2"
port = 22
user = "homelink"
identity_file = 'C:\HomeLink\relay_key'
timeout = 8
sudo = true

[[pcs]]
id = "media"
name = "Media PC"
os = "linux"
host = "10.0.6.30"
ports = [22]

[pcs.wake]
method = "relay"
relay = "openwrt"
mac = "00:11:22:33:44:55"
interface = "br-lan"

[[pcs]]
id = "kiosk"
name = "Kiosk"
os = "linux"
host = "10.0.5.30"
ports = [22]

[pcs.wake]
method = "relay"
relay = "pi"
mac = "00:11:22:33:44:66"
address = "10.0.5.255"
"#;

const LAB: &str = r#"
[[tunnels]]
id = "lab"
name = "Lab"
kind = "wireguard"
conf = 'C:\WG\lab.conf.dpapi'
wireguard_exe = 'C:\WG\wireguard.exe'
probe_host = "172.16.0.1"
timeout = 20
ping_timeout_ms = 1000

[[pcs]]
id = "build"
name = "Build server"
os = "linux"
host = "172.16.0.20"
tunnel = "lab"
ports = [22]

[pcs.wake]
method = "magic_packet"
mac = "AA:BB:CC:00:00:20"
address = "172.16.0.20"
port = 9

[pcs.power]
method = "ssh"
user = "alex"
port = 22
identity_file = 'C:\HomeLink\pc_key'

[[pcs]]
id = "printer"
name = "Archive"
os = "linux"
host = "10.0.0.9"
ports = [445]
"#;

/// [`multi_config`] plus what the demo needs to show every display state:
///
/// - tunnel `lab` with `build` (`172.16.0.20`) behind it; make `lab` fail to start with
///   [`FakeTunnel::fail_up_for`] and `build` fail to wake with [`FakeWake::fail_for`];
/// - `printer` ("Archive", `10.0.0.9`) with no tunnel and no wake, power or Remote Desktop
///   configured, to show an action that is not configured (as against one that is
///   not available now).
///
/// A power command fails with [`FakePower::fail_for`]. Tunnels start unknown, a stopped
/// tunnel is off, a started one is ok, a connecting tunnel or a waking PC is busy.
pub fn demo_config() -> Config {
    let text = format!("{BASE}{MORE}{LAB}");
    config::parse(&text)
        .expect("the built-in test configuration is valid")
        .config
}

/// The single-PC configuration plus one PC for each of the other relay presets: `desktop`
/// (a RouterOS relay), `media` (etherwake on `br-lan`) and `kiosk` (wakeonlan with an
/// address).
pub fn relay_config() -> Config {
    let text = format!("{BASE}{RELAYS}");
    config::parse(&text)
        .expect("the built-in test configuration is valid")
        .config
}

/// An engine wired to the fakes, a manual clock and (by default) an inline executor.
pub struct Harness {
    /// The engine.
    pub engine: Engine,
    /// A handle to command it.
    pub handle: Handle,
    /// The configuration it runs.
    pub config: Config,
    /// The clock.
    pub clock: Arc<FakeClock>,
    /// Tunnel control.
    pub tunnel: Arc<FakeTunnel>,
    /// Ping and port checks.
    pub net: Arc<FakeNetwork>,
    /// Wake requests.
    pub wake: Arc<FakeWake>,
    /// Power commands.
    pub power: Arc<FakePower>,
    /// What the engine published.
    pub sink: Arc<RecordingSink>,
    /// Present when built with [`Harness::manual`].
    pub jobs: Option<Arc<ManualExecutor>>,
}

impl Harness {
    /// A harness whose jobs run immediately.
    pub fn new(config: Config) -> Self {
        Self::build(config, None)
    }

    /// A harness whose jobs wait until [`Harness::run_jobs`].
    pub fn manual(config: Config) -> Self {
        Self::build(config, Some(Arc::new(ManualExecutor::default())))
    }

    fn build(config: Config, manual: Option<Arc<ManualExecutor>>) -> Self {
        let clock = Arc::new(FakeClock::default());
        let tunnel = Arc::new(FakeTunnel::default());
        let net = Arc::new(FakeNetwork::default());
        let wake = Arc::new(FakeWake::default());
        let power = Arc::new(FakePower::default());
        let sink = Arc::new(RecordingSink::default());
        let mut registry = WakeRegistry::new();
        for kind in [WakeKind::Relay, WakeKind::MagicPacket, WakeKind::Command] {
            registry.register(kind, wake.clone());
        }
        let executor: Arc<dyn Executor> = match &manual {
            Some(m) => m.clone(),
            None => Arc::new(InlineExecutor),
        };
        let deps = Deps {
            tunnel: tunnel.clone(),
            pinger: net.clone(),
            ports: net.clone(),
            wake: Arc::new(registry),
            power: power.clone(),
            clock: clock.clone(),
            executor,
            sink: sink.clone(),
        };
        let (engine, handle) = Engine::new(config.clone(), deps);
        Self {
            engine,
            handle,
            config,
            clock,
            tunnel,
            net,
            wake,
            power,
            sink,
            jobs: manual,
        }
    }

    /// Makes a host (by address) reachable or not.
    pub fn reachable(&self, host: &str, yes: bool) {
        self.net.set(host, yes);
    }

    /// Sends a command and lets everything that can happen at this time happen, including
    /// waiting for a started tunnel.
    pub fn send(&mut self, command: Command) {
        assert!(self.handle.send(command), "the engine has stopped");
        self.settle();
    }

    /// `Command::Wake` for a PC.
    pub fn wake_pc(&mut self, pc: &str) {
        self.send(Command::Wake(pc_id(pc)));
    }

    /// `Command::ToggleTunnel`.
    pub fn toggle_tunnel(&mut self, tunnel: &str) {
        self.send(Command::ToggleTunnel(tunnel_id(tunnel)));
    }

    /// `Command::Power`.
    pub fn power_pc(&mut self, pc: &str, action: PowerAction) {
        self.send(Command::Power(pc_id(pc), action));
    }

    /// Processes everything ready now; while a started tunnel is awaited, advances the
    /// clock by the start polling interval (as v0.3 slept 1 s per attempt) and repeats.
    pub fn settle(&mut self) {
        for _ in 0..10_000 {
            self.engine.pump();
            if !self.engine.machine().awaiting_tunnel() || self.jobs.is_some() {
                return;
            }
            self.clock.advance(START_POLL);
        }
        panic!("settle did not finish");
    }

    /// One more polling cycle: moves the clock to the next wake-up (if it is in the future)
    /// and processes it. This is the old `controller.refresh()`.
    pub fn refresh(&mut self) {
        if let Some(at) = self.engine.pump() {
            self.clock.advance_to(at);
        }
        self.settle();
    }

    /// Moves the clock without processing anything.
    pub fn advance(&self, seconds: u64) {
        self.clock.advance(Duration::from_secs(seconds));
    }

    /// Runs the jobs held by a manual executor and processes their results.
    pub fn run_jobs(&mut self) {
        if let Some(jobs) = &self.jobs {
            jobs.run_all();
        }
        self.engine.pump();
    }

    /// The state of a PC.
    pub fn pc(&self, id: &str) -> PcState {
        self.engine
            .machine()
            .pc_state(&pc_id(id))
            .expect("known PC")
    }

    /// The state of a tunnel.
    pub fn tunnel_state(&self, id: &str) -> TunnelState {
        self.engine
            .machine()
            .tunnel_state(&tunnel_id(id))
            .expect("known tunnel")
    }

    /// The notices so far.
    pub fn notices(&self) -> Vec<Notice> {
        self.sink.notices()
    }

    /// Forgets the notices so far.
    pub fn clear_notices(&self) {
        self.sink.clear_notices();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::CommandOutcome;

    #[test]
    fn fake_runner_records_and_answers() {
        let runner = FakeCommandRunner::default();
        assert!(runner.run(&["a".into()], Duration::from_secs(1)).success());
        runner.set_result(CommandOutput::exited(3, "x"));
        assert_eq!(
            runner.run(&["b".into()], Duration::from_secs(2)).outcome,
            CommandOutcome::Exited(3)
        );
        assert_eq!(runner.calls().len(), 2);
    }

    #[test]
    fn the_demo_config_can_show_every_display_state_including_fail() {
        use crate::snapshot::DisplayState as D;
        let mut h = Harness::new(demo_config());
        h.refresh();
        h.reachable("192.168.88.1", true);
        h.reachable("192.168.88.10", true);
        h.refresh();
        // Busy: a wake request that was taken up but is not answered yet.
        h.wake_pc("laptop");
        // Fail: a tunnel that cannot start, a PC that cannot be woken, a power command that
        // fails.
        h.tunnel
            .fail_up_for(&tunnel_id("lab"), Some("no such tunnel"));
        h.toggle_tunnel("lab");
        h.wake.fail_for(&pc_id("nas"), Some("unreachable"));
        h.wake_pc("nas");
        h.power.fail_for(&pc_id("desktop"), Some("refused"));
        h.power_pc("desktop", PowerAction::Sleep);

        let (mut tunnels, mut pcs) = (HashSet::new(), Vec::new());
        for snapshot in h.sink.snapshots() {
            for group in &snapshot.groups {
                tunnels.extend(group.tunnel.iter().map(|t| t.display.as_str()));
                pcs.extend(group.pcs.iter().map(|p| p.display.as_str()));
            }
        }
        for d in [D::Ok, D::Busy, D::Off, D::Unknown, D::Fail] {
            assert!(tunnels.contains(d.as_str()), "no tunnel was {d:?}");
            assert!(pcs.contains(&d.as_str()), "no PC was {d:?}");
        }
        let last = h.sink.snapshots().pop().unwrap();
        let printer = last
            .groups
            .iter()
            .flat_map(|g| &g.pcs)
            .find(|p| p.id == pc_id("printer"))
            .unwrap();
        assert_eq!(printer.configured, crate::snapshot::Actions::default());
        assert_eq!(demo_config().pcs.len(), 6);
    }

    #[test]
    fn built_in_configs_are_valid() {
        assert_eq!(single_config().pcs.len(), 1);
        let multi = multi_config();
        assert_eq!((multi.tunnels.len(), multi.pcs.len()), (2, 4));
    }
}

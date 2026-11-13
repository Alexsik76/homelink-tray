//! The state machine: a pure, sans-IO port of `controller.py` (section 5 of
//! `docs/rust-architecture.md`).
//!
//! [`Machine::step`] takes an [`Input`] and the current [`Time`] and returns an [`Output`]:
//! effects to run, notices to show and the time of the next wake-up. It never blocks, reads
//! a clock or touches the outside world, so every transition is testable with plain values.
//!
//! Transition rows are named as in the document: `T1`-`T10` for tunnels (see `tunnel.rs`),
//! `P1`-`P13` for PCs (see `pc.rs`).
//!
//! Operations are serialised per entity: a tunnel has at most one *action* (start or stop)
//! and a PC at most one (connect check, wake or power command) in flight; a second command
//! for the same entity is dropped with a log line, as `busy.acquire(blocking=False)` did.
//! Passive probes run alongside an action but their results are ignored while it runs, as
//! `refresh()` was skipped while busy.

use std::time::Duration;

use crate::config::Config;
use crate::domain::{
    Host, PcId, PcState, PowerAction, PowerMethod, Timing, TunnelId, TunnelState, WakeMethod,
};
use crate::notice::Notice;
use crate::ports::Time;
use crate::snapshot::{
    Actions, CHECK_STEP, DisplayState, GroupView, PcView, Snapshot, TrayState, TunnelToggle,
    TunnelView, WakeSummary,
};

mod pc;
mod tunnel;

#[cfg(test)]
mod tests;

/// Polling interval of a tunnel whose start is being awaited (v0.3 slept 1 s per attempt).
pub const START_POLL: Duration = Duration::from_secs(1);

/// What the user asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Connect the tunnel if it is down, otherwise disconnect it.
    ToggleTunnel(TunnelId),
    /// Wake a PC. For a PC behind a tunnel that is not up this first brings the tunnel up
    /// (the old `connect_and_wake`).
    Wake(PcId),
    /// Put a PC to sleep or shut it down.
    Power(PcId, PowerAction),
}

/// How much a tunnel probe checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelProbe {
    /// Service state, then ping; used for regular polling (`_refresh`).
    Full,
    /// Ping only; used while waiting for a tunnel we just started (the `_start_tunnel` loop).
    ReachOnly,
}

/// What a PC probe is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcProbe {
    /// Regular polling.
    Poll,
    /// The check made before sending a wake request (P6, P7).
    Check,
}

/// A side effect for the engine to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Probe a tunnel.
    ProbeTunnel {
        /// The tunnel.
        tunnel: TunnelId,
        /// How much to check.
        kind: TunnelProbe,
    },
    /// Start a tunnel.
    TunnelUp(TunnelId),
    /// Stop a tunnel.
    TunnelDown(TunnelId),
    /// Check whether all the PC's ports are open.
    ProbePc {
        /// The PC.
        pc: PcId,
        /// What the probe is for.
        kind: PcProbe,
    },
    /// Send the wake request.
    Wake(PcId),
    /// Run a power command.
    Power {
        /// The PC.
        pc: PcId,
        /// Sleep or shutdown.
        action: PowerAction,
    },
}

/// What a tunnel probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// The tunnel service is not running.
    NotRunning,
    /// Running, but the probe host does not answer.
    Silent,
    /// Running and the probe host answers.
    Reachable,
}

/// Everything the machine reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A user command.
    Command(Command),
    /// A tunnel probe finished.
    TunnelProbed {
        /// The tunnel.
        tunnel: TunnelId,
        /// The kind of probe that was run.
        kind: TunnelProbe,
        /// What it found.
        observation: Observation,
    },
    /// A PC probe finished.
    PcProbed {
        /// The PC.
        pc: PcId,
        /// The kind of probe that was run.
        kind: PcProbe,
        /// Whether every port accepted a connection.
        ready: bool,
    },
    /// Starting a tunnel finished.
    TunnelUpDone {
        /// The tunnel.
        tunnel: TunnelId,
        /// `Err` carries the reason.
        result: Result<(), String>,
    },
    /// Stopping a tunnel finished.
    TunnelDownDone {
        /// The tunnel.
        tunnel: TunnelId,
        /// `Err` carries the reason.
        result: Result<(), String>,
    },
    /// Sending a wake request finished.
    WakeDone {
        /// The PC.
        pc: PcId,
        /// `Err` carries the reason.
        result: Result<(), String>,
    },
    /// A power command finished.
    PowerDone {
        /// The PC.
        pc: PcId,
        /// Sleep or shutdown.
        action: PowerAction,
        /// `Err` carries the reason.
        result: Result<(), String>,
    },
    /// The job for this effect panicked; it is treated as failed.
    Panicked(Effect),
    /// Time passed: issue the probes that are due.
    Tick,
}

/// The result of one step.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Output {
    /// Jobs to run.
    pub effects: Vec<Effect>,
    /// Notices to show.
    pub notices: Vec<Notice>,
    /// When the machine next needs a [`Input::Tick`]; `None` when it waits for a job.
    pub next_wake: Option<Time>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TunnelAction {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PcAction {
    Check,
    Wake,
    Power(PowerAction),
}

/// What a failed tunnel action was after; decides which observation ends the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TunnelFailKind {
    /// Start failed or timed out: ends when the tunnel is seen up.
    Start,
    /// Stop failed: ends when the tunnel is seen down.
    Stop,
}

/// What a failed PC action was after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PcFailKind {
    /// The wake request failed or the wake timeout expired: ends when the PC is seen online.
    Wake,
    /// A power command failed: ends when the PC is seen leaving `Online`.
    Power,
}

/// The failure of an entity's last requested action (section 5.6 of the architecture).
#[derive(Debug, Clone)]
struct Failure<K> {
    kind: K,
    /// The notice that was shown for it; also what the details window shows.
    notice: Notice,
}

#[derive(Debug)]
struct TunnelRt {
    id: TunnelId,
    name: String,
    kind: &'static str,
    failed: Option<Failure<TunnelFailKind>>,
    /// When the last probe result that counted was applied.
    last_check: Option<Time>,
    timeout: Duration,
    poll: Duration,
    state: TunnelState,
    action: Option<TunnelAction>,
    probing: bool,
    /// Set while we wait for a tunnel we started: the time to give up.
    awaiting: Option<Time>,
    /// The user asked to stop it, so its disappearance is not "lost" (`user_stopped`).
    expect_down: bool,
    next_probe: Time,
}

#[derive(Debug)]
struct PcRt {
    id: PcId,
    name: String,
    host: Host,
    tunnel: Option<usize>,
    timing: Timing,
    can_wake: bool,
    can_sleep: bool,
    can_shutdown: bool,
    can_rdp: bool,
    ports: Vec<u16>,
    wake: Option<WakeSummary>,
    failed: Option<Failure<PcFailKind>>,
    /// When the last probe result that counted was applied.
    last_check: Option<Time>,
    state: PcState,
    failures: u32,
    waking_until: Option<Time>,
    /// A power command succeeded since the PC was last seen online (`_power_sent`).
    power_sent: bool,
    action: Option<PcAction>,
    probing: bool,
    /// Waiting for its tunnel to come up so that it can be woken.
    connect_pending: bool,
    next_probe: Time,
}

/// The state machine for all tunnels and PCs of a configuration.
#[derive(Debug)]
pub struct Machine {
    tunnels: Vec<TunnelRt>,
    pcs: Vec<PcRt>,
}

impl Machine {
    /// A machine for `config`: every tunnel is down, every PC unknown, everything due now.
    pub fn new(config: &Config, now: Time) -> Self {
        let tunnels = config
            .tunnels
            .iter()
            .map(|t| TunnelRt {
                id: t.id.clone(),
                name: t.name.clone(),
                kind: t.kind.label(),
                failed: None,
                last_check: None,
                timeout: t.timeout,
                poll: config.defaults.poll,
                state: TunnelState::Down,
                action: None,
                probing: false,
                awaiting: None,
                expect_down: false,
                next_probe: now,
            })
            .collect();
        let pcs = config
            .pcs
            .iter()
            .map(|p| PcRt {
                id: p.id.clone(),
                name: p.name.clone(),
                host: p.host.clone(),
                tunnel: p
                    .tunnel
                    .as_ref()
                    .and_then(|id| config.tunnels.iter().position(|t| t.id == *id)),
                timing: p.timing,
                can_wake: p.wake.is_some(),
                can_sleep: supports(p.power.as_ref(), PowerAction::Sleep),
                can_shutdown: supports(p.power.as_ref(), PowerAction::Shutdown),
                can_rdp: p.rdp.is_some(),
                ports: p.ports.clone(),
                wake: p.wake.as_ref().and_then(|w| wake_summary(config, w)),
                failed: None,
                last_check: None,
                state: PcState::Unknown,
                failures: 0,
                waking_until: None,
                power_sent: false,
                action: None,
                probing: false,
                connect_pending: false,
                next_probe: now,
            })
            .collect();
        Self { tunnels, pcs }
    }

    /// Advances the machine by one input at time `now`.
    pub fn step(&mut self, input: Input, now: Time) -> Output {
        let mut out = Output::default();
        match input {
            Input::Command(Command::ToggleTunnel(id)) => self.toggle_tunnel(&id, now, &mut out),
            Input::Command(Command::Wake(id)) => self.wake_pc(&id, now, &mut out),
            Input::Command(Command::Power(id, action)) => self.power_pc(&id, action, &mut out),
            Input::TunnelProbed {
                tunnel,
                kind,
                observation,
            } => {
                self.tunnel_probed(&tunnel, kind, observation, now, &mut out);
            }
            Input::PcProbed { pc, kind, ready } => self.pc_probed(&pc, kind, ready, now, &mut out),
            Input::TunnelUpDone { tunnel, result } => {
                self.tunnel_up_done(&tunnel, result, now, &mut out);
            }
            Input::TunnelDownDone { tunnel, result } => {
                self.tunnel_down_done(&tunnel, result, now, &mut out);
            }
            Input::WakeDone { pc, result } => self.wake_done(&pc, result, now, &mut out),
            Input::PowerDone { pc, action, result } => {
                self.power_done(&pc, action, result, &mut out);
            }
            Input::Panicked(effect) => self.panicked(effect, now, &mut out),
            Input::Tick => self.issue_due(now, &mut out),
        }
        out.next_wake = self.next_wake();
        out
    }

    /// The polling interval of a PC: the wake interval while it wakes (`poll_interval`).
    pub fn pc_interval(&self, id: &PcId) -> Option<Duration> {
        self.pcs.iter().find(|p| p.id == *id).map(PcRt::interval)
    }

    /// The state of a tunnel.
    pub fn tunnel_state(&self, id: &TunnelId) -> Option<TunnelState> {
        self.tunnels.iter().find(|t| t.id == *id).map(|t| t.state)
    }

    /// The state of a PC.
    pub fn pc_state(&self, id: &PcId) -> Option<PcState> {
        self.pcs.iter().find(|p| p.id == *id).map(|p| p.state)
    }

    /// Whether a tunnel we started is still being waited for.
    pub fn awaiting_tunnel(&self) -> bool {
        self.tunnels.iter().any(|t| t.awaiting.is_some())
    }

    /// The current state as published to the UI.
    pub fn snapshot(&self) -> Snapshot {
        let pc_view = |p: &PcRt| {
            let online = p.state == PcState::Online;
            let idle = p.action.is_none();
            PcView {
                id: p.id.clone(),
                name: p.name.clone(),
                state: p.state,
                busy: !idle,
                actions: Actions {
                    wake: idle
                        && p.can_wake
                        && !matches!(p.state, PcState::Online | PcState::Waking),
                    sleep: idle && online && p.can_sleep,
                    shutdown: idle && online && p.can_shutdown,
                    rdp: idle && online && p.can_rdp,
                },
                display: p.display(),
                last_checked: p.last_check.map(|t| t.floor_to(CHECK_STEP)),
                last_error: p.failed.as_ref().map(|f| f.notice.clone()),
                address: p.host.to_string(),
                tunnel_name: p
                    .tunnel
                    .and_then(|ti| self.tunnels.get(ti))
                    .map(|t| t.name.clone()),
                wake: p.wake.clone(),
                ports: p.ports.clone(),
                configured: Actions {
                    wake: p.can_wake,
                    sleep: p.can_sleep,
                    shutdown: p.can_shutdown,
                    rdp: p.can_rdp,
                },
            }
        };
        let mut groups: Vec<GroupView> = Vec::new();
        for (i, t) in self.tunnels.iter().enumerate() {
            let behind: Vec<&PcRt> = self.pcs.iter().filter(|p| p.tunnel == Some(i)).collect();
            groups.push(GroupView {
                tunnel: Some(TunnelView {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    state: t.state,
                    can_toggle: t.action.is_none(),
                    display: t.display(),
                    last_checked: t.last_check.map(|c| c.floor_to(CHECK_STEP)),
                    last_error: t.failed.as_ref().map(|f| f.notice.clone()),
                    kind: t.kind,
                    pcs: behind.iter().map(|p| p.name.clone()).collect(),
                    toggle: TunnelToggle::for_state(t.state),
                }),
                pcs: behind.into_iter().map(pc_view).collect(),
            });
        }
        let loose: Vec<PcView> = self
            .pcs
            .iter()
            .filter(|p| p.tunnel.is_none())
            .map(pc_view)
            .collect();
        if !loose.is_empty() {
            groups.push(GroupView {
                tunnel: None,
                pcs: loose,
            });
        }
        let tray = TrayState::summarise(
            self.tunnels.iter().map(|t| (t.display(), t.state)),
            self.pcs.iter().map(|p| (p.display(), p.state)),
        );
        Snapshot { groups, tray }
    }

    fn tunnel_idx(&self, id: &TunnelId) -> Option<usize> {
        self.tunnels.iter().position(|t| t.id == *id)
    }

    fn pc_idx(&self, id: &PcId) -> Option<usize> {
        self.pcs.iter().position(|p| p.id == *id)
    }

    /// Interval between probes of a tunnel (and of its PCs' cycle): the shortest of the
    /// tunnel's own and those of the PCs behind it.
    fn tunnel_interval(&self, ti: usize) -> Duration {
        let own = self.tunnels.get(ti).map_or(START_POLL, |t| t.poll);
        self.pcs
            .iter()
            .filter(|p| p.tunnel == Some(ti))
            .map(PcRt::interval)
            .fold(own, Duration::min)
    }

    /// Issues the probes that are due: tunnels, and PCs that have no tunnel.
    fn issue_due(&mut self, now: Time, out: &mut Output) {
        for t in &mut self.tunnels {
            if t.action.is_none() && !t.probing && t.next_probe <= now {
                let kind = if t.awaiting.is_some() {
                    TunnelProbe::ReachOnly
                } else {
                    TunnelProbe::Full
                };
                t.probing = true;
                out.effects.push(Effect::ProbeTunnel {
                    tunnel: t.id.clone(),
                    kind,
                });
            }
        }
        for p in &mut self.pcs {
            if p.tunnel.is_none() && p.action.is_none() && !p.probing && p.next_probe <= now {
                p.probing = true;
                out.effects.push(Effect::ProbePc {
                    pc: p.id.clone(),
                    kind: PcProbe::Poll,
                });
            }
        }
    }

    fn next_wake(&self) -> Option<Time> {
        let tunnels = self
            .tunnels
            .iter()
            .filter(|t| t.action.is_none() && !t.probing)
            .map(|t| t.next_probe);
        let pcs = self
            .pcs
            .iter()
            .filter(|p| p.tunnel.is_none() && p.action.is_none() && !p.probing)
            .map(|p| p.next_probe);
        tunnels.chain(pcs).min()
    }

    /// Sets a PC's state; entering `Online` clears `power_sent` (as `_set`).
    fn set_pc_state(&mut self, pi: usize, new: PcState) {
        let Some(p) = self.pcs.get_mut(pi) else {
            return;
        };
        if new == PcState::Online && p.state != PcState::Online {
            p.power_sent = false;
        }
        if new != p.state {
            log::info!("state: pc {} {:?} -> {:?}", p.id, p.state, new);
            p.state = new;
        }
    }

    fn panicked(&mut self, effect: Effect, now: Time, out: &mut Output) {
        log::error!("a job panicked: {effect:?}");
        let failed = || Err("internal error".to_owned());
        match effect {
            Effect::ProbeTunnel { tunnel, kind } => {
                if let Some(ti) = self.tunnel_idx(&tunnel) {
                    let interval = if kind == TunnelProbe::ReachOnly {
                        START_POLL
                    } else {
                        self.tunnel_interval(ti)
                    };
                    if let Some(t) = self.tunnels.get_mut(ti) {
                        t.probing = false;
                        t.next_probe = now + interval;
                    }
                }
            }
            Effect::ProbePc { pc, kind } => {
                if let Some(p) = self.pc_idx(&pc).and_then(|i| self.pcs.get_mut(i)) {
                    p.probing = false;
                    if kind == PcProbe::Check {
                        p.action = None;
                    }
                    p.next_probe = now + p.interval();
                }
            }
            Effect::TunnelUp(tunnel) => self.tunnel_up_done(&tunnel, failed(), now, out),
            Effect::TunnelDown(tunnel) => self.tunnel_down_done(&tunnel, failed(), now, out),
            Effect::Wake(pc) => self.wake_done(&pc, failed(), now, out),
            Effect::Power { pc, action } => self.power_done(&pc, action, failed(), out),
        }
    }
}

impl TunnelRt {
    /// The marker state: a failure first, then connecting, up and stopped; a tunnel that has
    /// not been checked yet is unknown.
    fn display(&self) -> DisplayState {
        if self.failed.is_some() {
            return DisplayState::Fail;
        }
        match self.state {
            TunnelState::Starting => DisplayState::Busy,
            TunnelState::Up => DisplayState::Ok,
            TunnelState::Down if self.last_check.is_none() => DisplayState::Unknown,
            TunnelState::Down => DisplayState::Off,
        }
    }
}

impl PcRt {
    /// The marker state: a failure first, then in progress (an action runs, the PC waits for
    /// its tunnel, or it is waking), online, off, unknown.
    fn display(&self) -> DisplayState {
        if self.failed.is_some() {
            return DisplayState::Fail;
        }
        if self.action.is_some() || self.connect_pending || self.state == PcState::Waking {
            return DisplayState::Busy;
        }
        match self.state {
            PcState::Online => DisplayState::Ok,
            PcState::Offline => DisplayState::Off,
            PcState::Unknown | PcState::Waking => DisplayState::Unknown,
        }
    }

    fn interval(&self) -> Duration {
        if self.state == PcState::Waking {
            self.timing.wake_poll
        } else {
            self.timing.poll
        }
    }
}

/// How a wake method is summarised for the UI; `None` for a relay that is not configured
/// (the configuration validation rules that out).
fn wake_summary(config: &Config, wake: &WakeMethod) -> Option<WakeSummary> {
    Some(match wake {
        WakeMethod::Relay { relay, .. } => WakeSummary::Relay {
            relay: relay.to_string(),
            kind: config.relay(relay)?.kind,
        },
        WakeMethod::MagicPacket { address, port, .. } => WakeSummary::MagicPacket {
            address: *address,
            port: *port,
        },
        WakeMethod::Command { argv, .. } => WakeSummary::Command {
            program: argv.items().first().cloned().unwrap_or_default(),
        },
    })
}

fn supports(power: Option<&PowerMethod>, action: PowerAction) -> bool {
    match power {
        None => false,
        Some(PowerMethod::Ssh { .. }) => true,
        Some(PowerMethod::Command { sleep, shutdown }) => match action {
            PowerAction::Sleep => sleep.is_some(),
            PowerAction::Shutdown => shutdown.is_some(),
        },
    }
}

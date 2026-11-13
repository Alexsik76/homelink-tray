//! The engine: runs the [`Machine`] on one thread, executes its effects as blocking jobs
//! through the ports, feeds the results back and publishes snapshots (section 7 of
//! `docs/rust-architecture.md`).
//!
//! ```text
//! UI thread ── Command ──▶ engine thread ── Effect ──▶ executor (worker threads)
//!                              ▲   │                         │
//!                              │   └─ Snapshot / Notice ─▶ Sink
//!                              └──────── job results ────────┘
//! ```
//!
//! There is no async runtime: a job is an ordinary blocking function with its own timeouts.
//! A panic in a job is caught and reported to the machine as a failed job; the engine
//! thread itself never runs user-supplied code.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::config::Config;
use crate::machine::{Command, Effect, Input, Machine, Observation, Output, TunnelProbe};
use crate::ports::{
    Clock, Executor, Pinger, PortProbe, PowerRunner, Sink, Time, TunnelDriver, WakeRegistry,
};
use crate::snapshot::Snapshot;

/// Everything the engine needs from the outside world.
#[derive(Clone)]
pub struct Deps {
    /// Tunnel control.
    pub tunnel: Arc<dyn TunnelDriver>,
    /// ICMP echo.
    pub pinger: Arc<dyn Pinger>,
    /// TCP port checks.
    pub ports: Arc<dyn PortProbe>,
    /// Wake senders.
    pub wake: Arc<WakeRegistry>,
    /// Power commands.
    pub power: Arc<dyn PowerRunner>,
    /// Time.
    pub clock: Arc<dyn Clock>,
    /// Where jobs run.
    pub executor: Arc<dyn Executor>,
    /// Where snapshots and notices go.
    pub sink: Arc<dyn Sink>,
}

enum Message {
    Command(Command),
    Done(Input),
    Stop,
}

/// A cloneable handle for sending commands to the engine from any thread.
#[derive(Clone)]
pub struct Handle {
    tx: Sender<Message>,
}

impl Handle {
    /// Queues a command. Returns false if the engine has stopped.
    pub fn send(&self, command: Command) -> bool {
        self.tx.send(Message::Command(command)).is_ok()
    }

    /// Asks the engine to stop; [`Engine::run`] returns after finishing the current step.
    pub fn stop(&self) {
        let _ = self.tx.send(Message::Stop);
    }
}

/// The engine.
pub struct Engine {
    config: Arc<Config>,
    machine: Machine,
    deps: Deps,
    tx: Sender<Message>,
    rx: Receiver<Message>,
    last: Option<Snapshot>,
    stopped: bool,
}

impl Engine {
    /// An engine for `config`, and a handle to command it.
    pub fn new(config: Config, deps: Deps) -> (Self, Handle) {
        let (tx, rx) = mpsc::channel();
        let machine = Machine::new(&config, deps.clock.now());
        let handle = Handle { tx: tx.clone() };
        let engine = Self {
            config: Arc::new(config),
            machine,
            deps,
            tx,
            rx,
            last: None,
            stopped: false,
        };
        (engine, handle)
    }

    /// The machine, for inspection.
    pub fn machine(&self) -> &Machine {
        &self.machine
    }

    /// Processes everything that is ready: queued messages, then due probes, until nothing
    /// more can happen without waiting. Returns when the machine next wants to run.
    ///
    /// With an executor that runs jobs inline this settles completely, which makes the
    /// engine deterministic in tests.
    pub fn pump(&mut self) -> Option<Time> {
        self.publish_if_changed();
        loop {
            let mut handled = false;
            while let Ok(message) = self.rx.try_recv() {
                handled = true;
                if !self.handle(message) {
                    return None;
                }
            }
            let now = self.deps.clock.now();
            let output = self.machine.step(Input::Tick, now);
            let next = output.next_wake;
            let spawned = !output.effects.is_empty();
            self.apply(output);
            if !handled && !spawned {
                return next;
            }
        }
    }

    /// Runs until stopped: pump, then sleep until a message arrives or the next wake-up.
    pub fn run(mut self) {
        while !self.stopped {
            let next = self.pump();
            if self.stopped {
                break;
            }
            let message = match next {
                Some(at) => {
                    let wait = at.since(self.deps.clock.now());
                    match self.rx.recv_timeout(wait) {
                        Ok(message) => Some(message),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match self.rx.recv() {
                    Ok(message) => Some(message),
                    Err(_) => break,
                },
            };
            if let Some(message) = message {
                self.handle(message);
            }
        }
    }

    /// Runs the engine on its own thread.
    pub fn spawn(self) -> std::io::Result<JoinHandle<()>> {
        std::thread::Builder::new()
            .name("homelink-engine".into())
            .spawn(move || self.run())
    }

    fn handle(&mut self, message: Message) -> bool {
        let now = self.deps.clock.now();
        let input = match message {
            Message::Command(command) => Input::Command(command),
            Message::Done(input) => input,
            Message::Stop => {
                self.stopped = true;
                return false;
            }
        };
        let output = self.machine.step(input, now);
        self.apply(output);
        true
    }

    /// Spawns the effects, delivers the notices and publishes the snapshot if it changed.
    fn apply(&mut self, output: Output) {
        for effect in output.effects {
            self.spawn_job(effect);
        }
        self.publish_if_changed();
        for notice in output.notices {
            log::info!("notice: {notice:?}");
            self.deps.sink.notify(notice);
        }
    }

    fn publish_if_changed(&mut self) {
        let snapshot = self.machine.snapshot();
        if self.last.as_ref() != Some(&snapshot) {
            self.last = Some(snapshot.clone());
            self.deps.sink.publish(snapshot);
        }
    }

    fn spawn_job(&self, effect: Effect) {
        let deps = self.deps.clone();
        let config = Arc::clone(&self.config);
        let tx = self.tx.clone();
        self.deps.executor.spawn(Box::new(move || {
            let result = catch_unwind(AssertUnwindSafe(|| run_effect(&deps, &config, &effect)));
            let input = result.unwrap_or_else(|_| {
                log::error!("job panicked: {effect:?}");
                Input::Panicked(effect.clone())
            });
            let _ = tx.send(Message::Done(input));
        }));
    }
}

/// Executes one effect through the ports and returns what the machine should hear about it.
fn run_effect(deps: &Deps, config: &Config, effect: &Effect) -> Input {
    let lost = || Input::Panicked(effect.clone());
    match effect {
        Effect::ProbeTunnel { tunnel, kind } => {
            let Some(t) = config.tunnel(tunnel) else {
                return lost();
            };
            let running = match kind {
                TunnelProbe::ReachOnly => true,
                TunnelProbe::Full => deps.tunnel.is_running(t).unwrap_or_else(|err| {
                    log::warn!("cannot read the state of tunnel {}: {err}", t.id);
                    false
                }),
            };
            let observation = if !running {
                Observation::NotRunning
            } else if deps.pinger.ping(&t.probe_host, t.ping_timeout) {
                Observation::Reachable
            } else {
                Observation::Silent
            };
            Input::TunnelProbed {
                tunnel: tunnel.clone(),
                kind: *kind,
                observation,
            }
        }
        Effect::ProbePc { pc, kind } => {
            let Some(p) = config.pc(pc) else {
                return lost();
            };
            let ready = deps
                .ports
                .all_open(&p.host, &p.ports, p.timing.port_timeout);
            Input::PcProbed {
                pc: pc.clone(),
                kind: *kind,
                ready,
            }
        }
        Effect::TunnelUp(tunnel) => {
            let Some(t) = config.tunnel(tunnel) else {
                return lost();
            };
            let result = deps.tunnel.up(t).map_err(|err| err.to_string());
            Input::TunnelUpDone {
                tunnel: tunnel.clone(),
                result,
            }
        }
        Effect::TunnelDown(tunnel) => {
            let Some(t) = config.tunnel(tunnel) else {
                return lost();
            };
            let result = deps.tunnel.down(t).map_err(|err| err.to_string());
            Input::TunnelDownDone {
                tunnel: tunnel.clone(),
                result,
            }
        }
        Effect::Wake(pc) => {
            let Some(p) = config.pc(pc) else {
                return lost();
            };
            let result = deps.wake.send(config, p).map_err(|err| err.to_string());
            Input::WakeDone {
                pc: pc.clone(),
                result,
            }
        }
        Effect::Power { pc, action } => {
            let Some(p) = config.pc(pc) else {
                return lost();
            };
            let result = deps.power.run(p, *action).map_err(|err| err.to_string());
            Input::PowerDone {
                pc: pc.clone(),
                action: *action,
                result,
            }
        }
    }
}

/// A fixed pool of worker threads that runs jobs.
///
/// Dropping the pool lets idle workers exit; a job that is still running is not waited for.
pub struct ThreadPool {
    tx: Mutex<Option<Sender<Job>>>,
}

type Job = Box<dyn FnOnce() + Send>;

impl ThreadPool {
    /// A reasonable pool size: one job per tunnel or PC in flight stays well below this.
    pub const DEFAULT_WORKERS: usize = 8;

    /// Starts `workers` threads (at least one).
    pub fn new(workers: usize) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for n in 0..workers.max(1) {
            let rx = Arc::clone(&rx);
            let spawned = std::thread::Builder::new()
                .name(format!("homelink-worker-{n}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock() {
                            Ok(guard) => guard.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                });
            if let Err(err) = spawned {
                log::error!("cannot start worker {n}: {err}");
            }
        }
        Self {
            tx: Mutex::new(Some(tx)),
        }
    }
}

impl Executor for ThreadPool {
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        if let Ok(guard) = self.tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(job);
        }
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.tx.lock() {
            guard.take();
        }
    }
}

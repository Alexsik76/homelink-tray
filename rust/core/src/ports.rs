//! Traits that isolate side effects (section 6 of `docs/rust-architecture.md`).
//!
//! The state machine never touches the outside world; the engine runs its effects through
//! these traits. Production implementations (sockets, processes, the Windows API) are
//! adapters in `app` or, for portable ones, in `core` later; tests use the fakes in
//! [`crate::testing`]. All traits are `Send + Sync` because jobs run on worker threads.

use std::collections::HashMap;
use std::ops::Add;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::domain::{Host, Pc, PowerAction, Relay, Tunnel, WakeKind, WakeMethod};
use crate::notice::Notice;
use crate::snapshot::Snapshot;

/// A point in monotonic time, as an offset from an arbitrary origin chosen by the [`Clock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Time(Duration);

impl Time {
    /// The origin.
    pub const ZERO: Time = Time(Duration::ZERO);

    /// A time `millis` after the origin.
    pub fn from_millis(millis: u64) -> Self {
        Self(Duration::from_millis(millis))
    }

    /// The time elapsed since `earlier`, zero if `earlier` is later.
    pub fn since(self, earlier: Time) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
}

impl Add<Duration> for Time {
    type Output = Time;

    fn add(self, rhs: Duration) -> Time {
        Time(self.0.saturating_add(rhs))
    }
}

/// A source of monotonic time.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now(&self) -> Time;
}

/// The real clock.
#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    /// A clock whose origin is now.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Time {
        Time(self.origin.elapsed())
    }
}

/// A tunnel operation failed. The text is shown to the user (cut to the notice limit).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TunnelError(pub String);

/// Control of a tunnel.
pub trait TunnelDriver: Send + Sync {
    /// Whether the tunnel service is running.
    fn is_running(&self, tunnel: &Tunnel) -> Result<bool, TunnelError>;
    /// Starts the tunnel; starting a running one is not an error.
    fn up(&self, tunnel: &Tunnel) -> Result<(), TunnelError>;
    /// Stops the tunnel.
    fn down(&self, tunnel: &Tunnel) -> Result<(), TunnelError>;
}

/// ICMP echo.
pub trait Pinger: Send + Sync {
    /// Whether `host` answers one echo request within `timeout`.
    fn ping(&self, host: &Host, timeout: Duration) -> bool;
}

/// TCP reachability.
pub trait PortProbe: Send + Sync {
    /// Whether every port accepts a connection; stops at the first one that does not.
    fn all_open(&self, host: &Host, ports: &[u16], timeout: Duration) -> bool;
}

/// Why a wake request failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WakeError {
    /// The request was sent or run and failed.
    #[error("{0}")]
    Failed(String),
    /// No sender is registered for the method, or the PC has none.
    #[error("no way to wake this PC is available")]
    Unsupported,
}

/// What a [`WakeSender`] needs to wake one PC.
#[derive(Debug, Clone, Copy)]
pub struct WakeRequest<'a> {
    /// The PC.
    pub pc: &'a Pc,
    /// Its wake method.
    pub method: &'a WakeMethod,
    /// The relay named by the method, for relay based methods.
    pub relay: Option<&'a Relay>,
}

/// Sends one kind of wake request.
pub trait WakeSender: Send + Sync {
    /// Wakes the PC.
    fn wake(&self, request: &WakeRequest<'_>) -> Result<(), WakeError>;
}

/// Maps a [`WakeKind`] to the sender that implements it.
#[derive(Default, Clone)]
pub struct WakeRegistry {
    senders: HashMap<WakeKind, Arc<dyn WakeSender>>,
}

impl WakeRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the sender for a kind, replacing any earlier one.
    pub fn register(&mut self, kind: WakeKind, sender: Arc<dyn WakeSender>) {
        self.senders.insert(kind, sender);
    }

    /// Wakes `pc` with its configured method.
    pub fn send(&self, config: &Config, pc: &Pc) -> Result<(), WakeError> {
        let method = pc.wake.as_ref().ok_or(WakeError::Unsupported)?;
        let sender = self
            .senders
            .get(&method.kind())
            .ok_or(WakeError::Unsupported)?;
        let relay = match method {
            WakeMethod::Relay { relay, .. } => config.relay(relay),
            _ => None,
        };
        sender.wake(&WakeRequest { pc, method, relay })
    }
}

/// Why a power command failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PowerError(pub String);

/// Runs sleep and shutdown for a PC.
pub trait PowerRunner: Send + Sync {
    /// Runs the action; `Ok` means the command was accepted.
    fn run(&self, pc: &Pc, action: PowerAction) -> Result<(), PowerError>;
}

/// The result of an external program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Exit code; non-zero also for a timeout or a program that could not start.
    pub code: i32,
    /// Combined standard error and output, or a readable reason.
    pub text: String,
}

/// Runs an external program without a shell.
pub trait CommandRunner: Send + Sync {
    /// Runs `argv` and waits at most `timeout`.
    fn run(&self, argv: &[String], timeout: Duration) -> CommandOutput;
}

/// Runs blocking jobs off the engine thread.
pub trait Executor: Send + Sync {
    /// Runs the job, now or later, on some thread.
    fn spawn(&self, job: Box<dyn FnOnce() + Send>);
}

/// Receives what the engine publishes.
pub trait Sink: Send + Sync {
    /// The state changed.
    fn publish(&self, snapshot: Snapshot);
    /// The user should be told something.
    fn notify(&self, notice: Notice);
}

//! Traits that isolate side effects (section 6 of `docs/rust-architecture.md`).
//!
//! The state machine never touches the outside world; the engine runs its effects through
//! these traits. Production implementations (sockets, processes, the Windows API) are
//! adapters in `app` or, for portable ones, in `core` later; tests use the fakes in
//! [`crate::testing`]. All traits are `Send + Sync` because jobs run on worker threads.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
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

    /// This time rounded down to a multiple of `step` (unchanged if `step` is zero).
    pub fn floor_to(self, step: Duration) -> Time {
        let step = step.as_millis();
        if step == 0 {
            return self;
        }
        let millis = self.0.as_millis() / step * step;
        Time(Duration::from_millis(
            u64::try_from(millis).unwrap_or(u64::MAX),
        ))
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
    /// Whether the tunnel's server answered: a recent WireGuard handshake, prompted by one
    /// datagram into the tunnel when the last one is old. Used for a tunnel without a probe
    /// host. Only the service's own state is read; no secret is touched.
    fn handshake(&self, tunnel: &Tunnel) -> Result<bool, TunnelError>;
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

    /// The ports that accept a connection, each tried to the end (for the details window,
    /// which shows every port of the PC open or closed), and why the first port that did not
    /// accept failed.
    fn open_ports(&self, host: &Host, ports: &[u16], timeout: Duration) -> PortCheck;
}

/// The result of [`PortProbe::open_ports`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PortCheck {
    /// The ports that accepted a connection, in the order asked.
    pub open: Vec<u16>,
    /// Why the first port (in the order asked) that did not accept failed; `None` when every
    /// port accepted.
    pub failure: Option<ProbeFailure>,
}

/// Why a port check failed. Written to the log as English text (`Display`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeFailure {
    /// The host name resolved to no address.
    NotResolved,
    /// The resolver did not answer within the check's time.
    ResolveTimedOut,
    /// No address accepted the connection; `address` is the last one tried, with the port.
    Connect {
        /// The address and port of the last attempt (an IPv6 address with its scope).
        address: SocketAddr,
        /// How that attempt failed.
        error: ConnectError,
    },
}

/// How a connection attempt failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// No answer within the time the attempt had.
    TimedOut,
    /// The host refused the connection (nothing listens on the port).
    Refused,
    /// Another error of the system, with its text.
    Other(String),
}

impl ConnectError {
    /// The kind of an error of `TcpStream::connect_timeout`.
    pub fn from_io(err: &std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => Self::TimedOut,
            std::io::ErrorKind::ConnectionRefused => Self::Refused,
            _ => Self::Other(err.to_string()),
        }
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => f.write_str("timed out"),
            Self::Refused => f.write_str("refused"),
            Self::Other(text) => f.write_str(text),
        }
    }
}

impl fmt::Display for ProbeFailure {
    /// `the name did not resolve`, `name resolution timed out`, or
    /// `<port> at <address>: <error>` with an IPv6 address in brackets and with its scope, such
    /// as `3389 at [fe80::5974:1f1c:540d:99b4%8]: timed out`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotResolved => f.write_str("the name did not resolve"),
            Self::ResolveTimedOut => f.write_str("name resolution timed out"),
            Self::Connect { address, error } => {
                write!(f, "{} at ", address.port())?;
                match address {
                    SocketAddr::V4(a) => write!(f, "{}", a.ip())?,
                    SocketAddr::V6(a) if a.scope_id() != 0 => {
                        write!(f, "[{}%{}]", a.ip(), a.scope_id())?;
                    }
                    SocketAddr::V6(a) => write!(f, "[{}]", a.ip())?,
                }
                write!(f, ": {error}")
            }
        }
    }
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

/// How an external program ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
    /// It ran and exited with this code.
    Exited(i32),
    /// It was still running when the time ran out and was killed.
    TimedOut,
    /// It could not be started (not found, not executable, empty command line).
    NotStarted,
}

/// The result of an external program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// How it ended.
    pub outcome: CommandOutcome,
    /// Standard error, then standard output (each trimmed, bounded in size), or for
    /// [`CommandOutcome::NotStarted`] and [`CommandOutcome::TimedOut`] a readable reason.
    pub text: String,
}

impl CommandOutput {
    /// A program that exited with `code`.
    pub fn exited(code: i32, text: impl Into<String>) -> Self {
        Self {
            outcome: CommandOutcome::Exited(code),
            text: text.into(),
        }
    }

    /// A program that was killed after `timeout`; `partial` is what it had written.
    pub fn timed_out(timeout: Duration, partial: &str) -> Self {
        let reason = format!("timed out after {} s", timeout.as_secs_f64());
        let text = if partial.trim().is_empty() {
            reason
        } else {
            format!("{reason}: {}", partial.trim())
        };
        Self {
            outcome: CommandOutcome::TimedOut,
            text,
        }
    }

    /// A program that could not be started.
    pub fn not_started(reason: impl Into<String>) -> Self {
        Self {
            outcome: CommandOutcome::NotStarted,
            text: reason.into(),
        }
    }

    /// Whether the program ran and exited with code 0.
    pub fn success(&self) -> bool {
        self.outcome == CommandOutcome::Exited(0)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

    #[test]
    fn every_reason_is_worded_as_the_log_shows_it() {
        assert_eq!(
            ProbeFailure::NotResolved.to_string(),
            "the name did not resolve"
        );
        assert_eq!(
            ProbeFailure::ResolveTimedOut.to_string(),
            "name resolution timed out"
        );
        let scoped = SocketAddr::V6(SocketAddrV6::new(
            "fe80::5974:1f1c:540d:99b4".parse::<Ipv6Addr>().unwrap(),
            3389,
            0,
            8,
        ));
        let failure = ProbeFailure::Connect {
            address: scoped,
            error: ConnectError::TimedOut,
        };
        assert_eq!(
            failure.to_string(),
            "3389 at [fe80::5974:1f1c:540d:99b4%8]: timed out"
        );
        let plain_v6 = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 22, 0, 0));
        let failure = ProbeFailure::Connect {
            address: plain_v6,
            error: ConnectError::Refused,
        };
        assert_eq!(failure.to_string(), "22 at [::1]: refused");
        let v4 = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 25, 189), 3389));
        let failure = ProbeFailure::Connect {
            address: v4,
            error: ConnectError::Other("network is unreachable".into()),
        };
        assert_eq!(
            failure.to_string(),
            "3389 at 192.168.25.189: network is unreachable"
        );
    }

    #[test]
    fn the_kind_of_a_connect_error_is_read_from_the_system_error() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            ConnectError::from_io(&Error::from(ErrorKind::TimedOut)),
            ConnectError::TimedOut
        );
        assert_eq!(
            ConnectError::from_io(&Error::from(ErrorKind::ConnectionRefused)),
            ConnectError::Refused
        );
        let other = Error::other("host is down");
        assert_eq!(
            ConnectError::from_io(&other),
            ConnectError::Other("host is down".into())
        );
    }
}

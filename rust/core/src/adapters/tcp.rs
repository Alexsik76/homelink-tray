//! TCP port probe: is every listed port of a host open, and if not, why?
//!
//! - All ports are tried at the same time and share one deadline, so any number of closed or
//!   filtered ports cost at most one timeout, not one each.
//! - The host name is resolved first (in a helper thread, so a slow resolver cannot exceed the
//!   timeout either). The resolver's socket addresses are kept as it gave them, in its order and
//!   without duplicates: an IPv6 link-local address keeps its scope (`fe80::…%8`), without which
//!   a connect to it can hang when the computer has more than one interface (a WireGuard tunnel
//!   is one).
//! - The addresses are tried in turn, each with an equal share of the time left (with `n`
//!   addresses untried, the next may take `remaining / n`), so an address that never answers
//!   cannot use the whole deadline and keep the next one (`localhost` as `::1` before
//!   `127.0.0.1`, a name as `fe80::…` before its IPv4 address) from being tried.
//! - A connection is opened and closed at once; nothing is ever sent.
//! - A port that did not accept has a reason ([`ProbeFailure`]): the name did not resolve or its
//!   resolution timed out, or the last address tried, with how that attempt failed.

use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::domain::Host;
use crate::ports::{ConnectError, PortCheck, PortProbe, ProbeFailure};

/// The portable [`PortProbe`].
#[derive(Debug, Default, Clone, Copy)]
pub struct TcpPortProbe;

impl PortProbe for TcpPortProbe {
    fn all_open(&self, host: &Host, ports: &[u16], timeout: Duration) -> bool {
        all_open(host.as_str(), ports, timeout)
    }

    fn open_ports(&self, host: &Host, ports: &[u16], timeout: Duration) -> PortCheck {
        open_ports(host.as_str(), ports, timeout)
    }
}

/// The ports of `host` that accept a TCP connection within `timeout` in total, tried
/// concurrently, each to the end (unlike [`all_open`], one closed port does not stop the
/// others), and why the first port (in the order given) that did not accept failed.
pub fn open_ports(host: &str, ports: &[u16], timeout: Duration) -> PortCheck {
    if ports.is_empty() {
        return PortCheck::default();
    }
    let deadline = Instant::now() + timeout;
    let addresses = match resolve(host, timeout) {
        Ok(addresses) => addresses,
        Err(failure) => {
            log::debug!("{host}: {failure}");
            return PortCheck {
                open: Vec::new(),
                failure: Some(failure),
            };
        }
    };
    let never = AtomicBool::new(false);
    let results: Mutex<Vec<(u16, Result<(), ProbeFailure>)>> = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for &port in ports {
            let (addresses, never, results) = (&addresses, &never, &results);
            let _ = thread::Builder::new()
                .name("homelink-probe".into())
                .spawn_scoped(scope, move || {
                    let result = connect_any(addresses, port, deadline, never);
                    if let Ok(mut results) = results.lock() {
                        results.push((port, result));
                    }
                });
        }
    });
    let results = results.into_inner().unwrap_or_default();
    let result_of = |port: u16| results.iter().find(|(p, _)| *p == port).map(|(_, r)| r);
    // In the order of the settings.
    let open = ports
        .iter()
        .copied()
        .filter(|&p| matches!(result_of(p), Some(Ok(()))))
        .collect();
    let failure = ports.iter().find_map(|&p| match result_of(p) {
        Some(Ok(())) => None,
        Some(Err(failure)) => Some(failure.clone()),
        // The thread for this port could not be started: nothing was tried.
        None => Some(ProbeFailure::Connect {
            address: with_port(addresses.first().copied()?, p),
            error: ConnectError::Other("the probe could not be started".into()),
        }),
    });
    PortCheck { open, failure }
}

/// Whether every port of `host` accepts a TCP connection within `timeout` in total.
/// No ports means nothing is closed, so `true`.
pub fn all_open(host: &str, ports: &[u16], timeout: Duration) -> bool {
    if ports.is_empty() {
        return true;
    }
    let deadline = Instant::now() + timeout;
    let addresses = match resolve(host, timeout) {
        Ok(addresses) => addresses,
        Err(failure) => {
            log::debug!("{host}: {failure}");
            return false;
        }
    };
    let failed = AtomicBool::new(false);
    thread::scope(|scope| {
        for &port in ports {
            let (addresses, failed) = (&addresses, &failed);
            let spawned = thread::Builder::new()
                .name("homelink-probe".into())
                .spawn_scoped(scope, move || {
                    if connect_any(addresses, port, deadline, failed).is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                });
            if spawned.is_err() {
                failed.store(true, Ordering::Relaxed);
            }
        }
    });
    !failed.load(Ordering::Relaxed)
}

/// Tries the addresses in order until one accepts `port` or the deadline passes. With `n`
/// addresses untried, the next attempt may take at most an `n`-th of the time left. `stop`
/// (another port already failed, in [`all_open`]) ends it early. The error is the last attempt.
fn connect_any(
    addresses: &[SocketAddr],
    port: u16,
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<(), ProbeFailure> {
    let mut last = None;
    for (index, address) in addresses.iter().enumerate() {
        let target = with_port(*address, port);
        let timed_out = || ProbeFailure::Connect {
            address: target,
            error: ConnectError::TimedOut,
        };
        if stop.load(Ordering::Relaxed) {
            return Err(last.unwrap_or_else(timed_out));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let untried = u32::try_from(addresses.len() - index).unwrap_or(u32::MAX);
        let share = remaining / untried;
        if share.is_zero() {
            return Err(last.unwrap_or_else(timed_out));
        }
        match TcpStream::connect_timeout(&target, share) {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(err) => {
                last = Some(ProbeFailure::Connect {
                    address: target,
                    error: ConnectError::from_io(&err),
                });
            }
        }
    }
    Err(last.unwrap_or(ProbeFailure::NotResolved))
}

/// `address` with `port`, everything else (an IPv6 scope and flow) kept.
fn with_port(mut address: SocketAddr, port: u16) -> SocketAddr {
    address.set_port(port);
    address
}

/// The socket addresses of `host` (port 0): a literal address as it is, a name through the
/// system resolver, which is asked from a helper thread and waited for at most `timeout`.
fn resolve(host: &str, timeout: Duration) -> Result<Vec<SocketAddr>, ProbeFailure> {
    resolve_with(host, timeout, |name| {
        (name.as_str(), 0u16)
            .to_socket_addrs()
            .map(Iterator::collect)
    })
}

/// [`resolve`] with the resolver given (tests use a slow or failing one).
fn resolve_with(
    host: &str,
    timeout: Duration,
    lookup: impl FnOnce(String) -> io::Result<Vec<SocketAddr>> + Send + 'static,
) -> Result<Vec<SocketAddr>, ProbeFailure> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, 0)]);
    }
    let (tx, rx) = mpsc::channel();
    let name = host.to_owned();
    thread::Builder::new()
        .name("homelink-resolve".into())
        .spawn(move || {
            let _ = tx.send(lookup(name));
        })
        .map_err(|_| ProbeFailure::NotResolved)?;
    let found = match rx.recv_timeout(timeout) {
        Ok(Ok(found)) => found,
        Ok(Err(_)) => return Err(ProbeFailure::NotResolved),
        Err(_) => return Err(ProbeFailure::ResolveTimedOut),
    };
    let unique = unique(found);
    if unique.is_empty() {
        Err(ProbeFailure::NotResolved)
    } else {
        Ok(unique)
    }
}

/// The addresses without repeats, in their order; addresses that differ only in their IPv6
/// scope are different addresses.
fn unique(found: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let mut seen = HashSet::new();
    found.into_iter().filter(|a| seen.insert(*a)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV6, TcpListener};

    fn scoped(scope: u32) -> SocketAddr {
        SocketAddr::V6(SocketAddrV6::new(
            "fe80::5974:1f1c:540d:99b4".parse::<Ipv6Addr>().unwrap(),
            0,
            0,
            scope,
        ))
    }

    #[test]
    fn a_scoped_ipv6_address_keeps_its_scope_up_to_the_connect() {
        let v4 = SocketAddr::new(Ipv4Addr::new(192, 168, 25, 189).into(), 0);
        // As a resolver gives them: scoped first, then a repeat, then IPv4.
        let found = unique(vec![scoped(8), scoped(8), v4, scoped(9)]);
        assert_eq!(found, [scoped(8), v4, scoped(9)]);
        let target = with_port(found[0], 3389);
        match target {
            SocketAddr::V6(a) => {
                assert_eq!(a.scope_id(), 8);
                assert_eq!(a.port(), 3389);
            }
            SocketAddr::V4(_) => panic!("{target}"),
        }
    }

    #[test]
    fn a_resolver_that_gives_the_scope_keeps_it() {
        let found = resolve_with("pc", Duration::from_secs(2), |_| Ok(vec![scoped(8)])).unwrap();
        assert_eq!(found, [scoped(8)]);
    }

    #[test]
    fn a_literal_address_needs_no_resolver() {
        let found = resolve_with("192.168.25.189", Duration::from_secs(1), |_| {
            panic!("a literal address is not looked up")
        })
        .unwrap();
        assert_eq!(
            found,
            [SocketAddr::new(Ipv4Addr::new(192, 168, 25, 189).into(), 0)]
        );
    }

    #[test]
    fn a_name_that_does_not_resolve_or_resolves_too_slowly_says_so() {
        let failed = resolve_with("pc", Duration::from_secs(2), |_| {
            Err(io::Error::other("no such host"))
        });
        assert_eq!(failed, Err(ProbeFailure::NotResolved));
        let empty = resolve_with("pc", Duration::from_secs(2), |_| Ok(Vec::new()));
        assert_eq!(empty, Err(ProbeFailure::NotResolved));
        let slow = resolve_with("pc", Duration::from_millis(50), |_| {
            thread::sleep(Duration::from_millis(500));
            Ok(vec![scoped(8)])
        });
        assert_eq!(slow, Err(ProbeFailure::ResolveTimedOut));
    }

    /// A port of this computer that nothing listens on.
    fn closed_port() -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn a_refused_port_names_the_address_and_the_refusal() {
        let port = closed_port();
        let local = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
        let never = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(2);
        assert_eq!(
            connect_any(&[local], port, deadline, &never),
            Err(ProbeFailure::Connect {
                address: with_port(local, port),
                error: ConnectError::Refused,
            })
        );
    }

    #[test]
    fn a_spent_deadline_is_a_timeout_at_the_address_that_was_due() {
        let local = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
        let never = AtomicBool::new(false);
        assert_eq!(
            connect_any(&[local], 9, Instant::now(), &never),
            Err(ProbeFailure::Connect {
                address: with_port(local, 9),
                error: ConnectError::TimedOut,
            })
        );
    }

    #[test]
    fn an_address_that_never_answers_leaves_time_for_the_next() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        // TEST-NET-1 (RFC 5737): never routed; it either hangs until its share is spent or
        // fails at once, and either way the listening address after it must be tried.
        let silent = SocketAddr::new(Ipv4Addr::new(192, 0, 2, 1).into(), 0);
        let local = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
        let never = AtomicBool::new(false);
        let timeout = Duration::from_millis(800);
        let started = Instant::now();
        let result = connect_any(&[silent, local], port, started + timeout, &never);
        assert_eq!(result, Ok(()));
        assert!(started.elapsed() < timeout, "{:?}", started.elapsed());
    }

    #[test]
    fn the_first_closed_port_gives_the_reason_and_the_open_ones_are_listed() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let open = listener.local_addr().unwrap().port();
        let closed = closed_port();
        let check = open_ports("127.0.0.1", &[open, closed], Duration::from_secs(2));
        assert_eq!(check.open, [open]);
        assert_eq!(
            check.failure,
            Some(ProbeFailure::Connect {
                address: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), closed),
                error: ConnectError::Refused,
            })
        );
        let all = open_ports("127.0.0.1", &[open], Duration::from_secs(2));
        assert_eq!(all.failure, None);
    }
}

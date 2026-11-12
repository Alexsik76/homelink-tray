//! TCP port probe: is every listed port of a host open?
//!
//! - All ports are tried at the same time and share one deadline, so any number of closed or
//!   filtered ports cost at most one timeout, not one each.
//! - The host name is resolved first (in a helper thread, so a slow resolver cannot exceed the
//!   timeout either); every resolved address is tried in turn within the deadline, which
//!   covers `localhost` resolving to `::1` before `127.0.0.1`.
//! - A connection is opened and closed at once; nothing is ever sent.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::domain::Host;
use crate::ports::PortProbe;

/// The portable [`PortProbe`].
#[derive(Debug, Default, Clone, Copy)]
pub struct TcpPortProbe;

impl PortProbe for TcpPortProbe {
    fn all_open(&self, host: &Host, ports: &[u16], timeout: Duration) -> bool {
        all_open(host.as_str(), ports, timeout)
    }
}

/// Whether every port of `host` accepts a TCP connection within `timeout` in total.
/// No ports means nothing is closed, so `true`.
pub fn all_open(host: &str, ports: &[u16], timeout: Duration) -> bool {
    if ports.is_empty() {
        return true;
    }
    let deadline = Instant::now() + timeout;
    let Some(addresses) = resolve(host, timeout) else {
        log::debug!("cannot resolve {host}");
        return false;
    };
    let failed = AtomicBool::new(false);
    thread::scope(|scope| {
        for &port in ports {
            let (addresses, failed) = (&addresses, &failed);
            let spawned = thread::Builder::new()
                .name("homelink-probe".into())
                .spawn_scoped(scope, move || {
                    if !connect_any(addresses, port, deadline, failed) {
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

/// Tries the addresses in order until one accepts or the deadline passes.
fn connect_any(addresses: &[IpAddr], port: u16, deadline: Instant, failed: &AtomicBool) -> bool {
    for ip in addresses {
        if failed.load(Ordering::Relaxed) {
            return false; // another port already failed: the answer is "not ready"
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        if let Ok(stream) = TcpStream::connect_timeout(&SocketAddr::new(*ip, port), remaining) {
            drop(stream);
            return true;
        }
    }
    false
}

/// The addresses of `host`: a literal address as it is, a name through the system resolver,
/// which is asked from a helper thread and waited for at most `timeout`.
fn resolve(host: &str, timeout: Duration) -> Option<Vec<IpAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(vec![ip]);
    }
    let (tx, rx) = mpsc::channel();
    let name = host.to_owned();
    thread::Builder::new()
        .name("homelink-resolve".into())
        .spawn(move || {
            let found = (name.as_str(), 0u16)
                .to_socket_addrs()
                .map(|found| found.map(|a| a.ip()).collect::<Vec<_>>());
            let _ = tx.send(found);
        })
        .ok()?;
    let found = rx.recv_timeout(timeout).ok()?.ok()?;
    let mut seen = HashSet::new();
    let unique: Vec<IpAddr> = found.into_iter().filter(|ip| seen.insert(*ip)).collect();
    (!unique.is_empty()).then_some(unique)
}

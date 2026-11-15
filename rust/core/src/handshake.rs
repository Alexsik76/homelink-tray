//! Checking a WireGuard tunnel by its handshake instead of pinging a host (owner's decision of
//! 15.11.2026): a recent handshake proves that the tunnel's server is alive and accepted the
//! key, without guessing an address on the other side.
//!
//! `wg.exe show <tunnel> latest-handshakes` prints one line per peer, `<public key>\t<unix
//! time>` (0: never). WireGuard renews the handshake at least every two minutes while traffic
//! flows and considers a session dead after three ([`FRESH`]). When the last handshake is older,
//! one empty UDP datagram to an address of the tunnel's networks (`allowed-ips`) makes WireGuard
//! start a new one; the check then waits up to [`WAIT`] for it.
//!
//! Only `latest-handshakes` and `allowed-ips` are read: never `showconf`, `dump`, `private-key`
//! or `preshared-keys`, which print secrets.

use std::net::Ipv4Addr;
use std::time::Duration;

use crate::ports::{CommandOutcome, CommandRunner};

/// A handshake younger than this proves a live session (WireGuard's reject-after time).
pub const FRESH: Duration = Duration::from_secs(180);
/// How long to wait for the handshake that a datagram prompted (WireGuard retries every 5 s).
pub const WAIT: Duration = Duration::from_secs(6);
/// The pause between two readings while waiting.
pub const STEP: Duration = Duration::from_millis(500);
/// The UDP port of the prompting datagram (discard).
pub const DISCARD_PORT: u16 = 9;
/// The time a `wg` command may take.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// `wg show <tunnel> latest-handshakes`.
pub fn latest_argv(tunnel: &str) -> Vec<String> {
    ["wg", "show", tunnel, "latest-handshakes"]
        .map(str::to_owned)
        .to_vec()
}

/// `wg show <tunnel> allowed-ips`.
pub fn allowed_argv(tunnel: &str) -> Vec<String> {
    ["wg", "show", tunnel, "allowed-ips"]
        .map(str::to_owned)
        .to_vec()
}

/// The newest handshake of all peers, in seconds since 1970; `None` when there is none.
pub fn parse_latest(text: &str) -> Option<u64> {
    text.lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|t| t.parse::<u64>().ok())
        .filter(|t| *t > 0)
        .max()
}

/// The IPv4 networks of `allowed-ips`, in order: `<key>\t10.10.0.0/16 192.168.222.0/24`.
pub fn parse_allowed(text: &str) -> Vec<(Ipv4Addr, u8)> {
    text.lines()
        .flat_map(|line| line.split_whitespace().skip(1))
        .filter_map(|net| {
            let (addr, prefix) = net.split_once('/')?;
            Some((addr.parse().ok()?, prefix.parse().ok()?))
        })
        .filter(|(_, prefix)| *prefix <= 32)
        .collect()
}

/// Where to send the prompting datagram: the first address after the network address of the
/// first network that is not the default route (the address of a /31 or /32 itself). `None`
/// when the tunnel carries only the default route: then any address would do, but it is not
/// the tunnel's own network, so nothing is sent and the keepalive is relied on.
pub fn prompt_address(networks: &[(Ipv4Addr, u8)]) -> Option<Ipv4Addr> {
    let (addr, prefix) = networks.iter().find(|(_, prefix)| *prefix > 0)?;
    let bits = u32::from(*addr);
    if *prefix >= 31 {
        return Some(*addr);
    }
    let mask = u32::MAX << (32 - u32::from(*prefix));
    Some(Ipv4Addr::from((bits & mask) + 1))
}

/// Whether a handshake at `at` is fresh at `now` (both in seconds since 1970).
pub fn is_fresh(at: u64, now: u64) -> bool {
    now.saturating_sub(at) < FRESH.as_secs()
}

/// What the check needs from the system.
pub struct System<'a> {
    /// Runs `wg`.
    pub runner: &'a dyn CommandRunner,
    /// Seconds since 1970.
    pub now: &'a dyn Fn() -> u64,
    /// Sends one empty UDP datagram to the address, port [`DISCARD_PORT`].
    pub send: &'a dyn Fn(Ipv4Addr) -> std::io::Result<()>,
    /// Waits.
    pub sleep: &'a dyn Fn(Duration),
}

fn latest(sys: &System<'_>, tunnel: &str) -> Result<Option<u64>, String> {
    let out = sys.runner.run(&latest_argv(tunnel), COMMAND_TIMEOUT);
    match out.outcome {
        CommandOutcome::Exited(0) => Ok(parse_latest(&out.text)),
        _ => Err(format!("wg show {tunnel} latest-handshakes: {}", out.text)),
    }
}

/// Whether the server of `tunnel` (the WireGuard tunnel name) answers: a fresh handshake, or a
/// new one within [`WAIT`] after a prompting datagram. `Err` when `wg` cannot be run or does
/// not know the tunnel.
pub fn check(sys: &System<'_>, tunnel: &str) -> Result<bool, String> {
    let before = latest(sys, tunnel)?;
    if before.is_some_and(|at| is_fresh(at, (sys.now)())) {
        return Ok(true);
    }
    let allowed = sys.runner.run(&allowed_argv(tunnel), COMMAND_TIMEOUT);
    let Some(target) = (allowed.outcome == CommandOutcome::Exited(0))
        .then(|| prompt_address(&parse_allowed(&allowed.text)))
        .flatten()
    else {
        return Ok(false);
    };
    if let Err(err) = (sys.send)(target) {
        log::debug!("tunnel {tunnel}: the prompting datagram to {target} failed: {err}");
    }
    let steps = WAIT.as_millis() / STEP.as_millis().max(1);
    for _ in 0..steps {
        (sys.sleep)(STEP);
        let now = latest(sys, tunnel)?;
        if now.is_some_and(|at| before.is_none_or(|b| at > b) || is_fresh(at, (sys.now)())) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::CommandOutput;
    use std::cell::Cell;
    use std::sync::Mutex;

    const KEY: &str = "icTVAvv587JQUp4DGc8GL3d9BTjhxZE6/N2k6WKiOGo=";

    #[test]
    fn the_output_of_wg_is_read_as_it_was_seen_on_windows() {
        assert_eq!(
            parse_latest(&format!("{KEY}\t1794764624\n")),
            Some(1_794_764_624)
        );
        assert_eq!(parse_latest(&format!("{KEY}\t0\n")), None, "never");
        assert_eq!(parse_latest(""), None);
        assert_eq!(
            parse_allowed(&format!("{KEY}\t10.10.0.0/16 192.168.222.0/24\n")),
            [
                (Ipv4Addr::new(10, 10, 0, 0), 16),
                (Ipv4Addr::new(192, 168, 222, 0), 24)
            ]
        );
        assert_eq!(parse_allowed(&format!("{KEY}\t(none)\n")), []);
    }

    #[test]
    fn the_prompting_datagram_goes_into_the_first_network_of_the_tunnel() {
        let nets = [
            (Ipv4Addr::new(10, 10, 0, 0), 16),
            (Ipv4Addr::new(192, 168, 222, 0), 24),
        ];
        assert_eq!(prompt_address(&nets), Some(Ipv4Addr::new(10, 10, 0, 1)));
        assert_eq!(
            prompt_address(&[(Ipv4Addr::new(10, 8, 0, 5), 32)]),
            Some(Ipv4Addr::new(10, 8, 0, 5))
        );
        assert_eq!(prompt_address(&[(Ipv4Addr::UNSPECIFIED, 0)]), None);
        assert_eq!(
            prompt_address(&[(Ipv4Addr::UNSPECIFIED, 0), (Ipv4Addr::new(10, 1, 2, 0), 24)]),
            Some(Ipv4Addr::new(10, 1, 2, 1))
        );
    }

    #[test]
    fn freshness_follows_the_three_minutes_of_wireguard() {
        assert!(is_fresh(1000, 1000 + 179));
        assert!(!is_fresh(1000, 1000 + 180));
        assert!(
            is_fresh(1000, 999),
            "a clock behind the handshake is still fresh"
        );
    }

    /// A `wg` that answers with the queued outputs in order (the last one repeats).
    struct Script(Mutex<Vec<CommandOutput>>, Mutex<Vec<Vec<String>>>);

    impl CommandRunner for Script {
        fn run(&self, argv: &[String], _: Duration) -> CommandOutput {
            if let Ok(mut calls) = self.1.lock() {
                calls.push(argv.to_vec());
            }
            let mut queue = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if queue.len() > 1 {
                queue.remove(0)
            } else {
                queue
                    .first()
                    .cloned()
                    .unwrap_or_else(|| CommandOutput::exited(1, ""))
            }
        }
    }

    fn run(outputs: Vec<CommandOutput>, now: u64) -> (Result<bool, String>, Vec<Ipv4Addr>, usize) {
        let script = Script(Mutex::new(outputs), Mutex::new(Vec::new()));
        let sent = Mutex::new(Vec::new());
        let sleeps = Cell::new(0);
        let send = |a: Ipv4Addr| {
            sent.lock().unwrap_or_else(|e| e.into_inner()).push(a);
            Ok(())
        };
        let sleep = |_: Duration| sleeps.set(sleeps.get() + 1);
        let sys = System {
            runner: &script,
            now: &|| now,
            send: &send,
            sleep: &sleep,
        };
        let result = check(&sys, "vic-wg");
        let sent = sent.into_inner().unwrap_or_default();
        (result, sent, sleeps.get())
    }

    #[test]
    fn a_fresh_handshake_is_enough_and_nothing_is_sent() {
        let (result, sent, sleeps) = run(
            vec![CommandOutput::exited(0, format!("{KEY}\t1000\n"))],
            1100,
        );
        assert_eq!(result, Ok(true));
        assert!(sent.is_empty());
        assert_eq!(sleeps, 0);
    }

    #[test]
    fn an_old_handshake_is_prompted_and_a_new_one_counts() {
        let (result, sent, _) = run(
            vec![
                CommandOutput::exited(0, format!("{KEY}\t1000\n")),
                CommandOutput::exited(0, format!("{KEY}\t10.10.0.0/16 192.168.222.0/24\n")),
                CommandOutput::exited(0, format!("{KEY}\t1000\n")),
                CommandOutput::exited(0, format!("{KEY}\t5001\n")),
            ],
            5000,
        );
        assert_eq!(result, Ok(true));
        assert_eq!(sent, [Ipv4Addr::new(10, 10, 0, 1)]);
    }

    #[test]
    fn a_silent_server_is_not_up_after_the_wait() {
        let (result, sent, sleeps) = run(
            vec![
                CommandOutput::exited(0, format!("{KEY}\t0\n")),
                CommandOutput::exited(0, format!("{KEY}\t10.10.0.0/16\n")),
                CommandOutput::exited(0, format!("{KEY}\t0\n")),
            ],
            5000,
        );
        assert_eq!(result, Ok(false));
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sleeps,
            12,
            "{} s in steps of {} ms",
            WAIT.as_secs(),
            STEP.as_millis()
        );
    }

    #[test]
    fn a_tunnel_unknown_to_wg_is_an_error() {
        let (result, _, _) = run(
            vec![CommandOutput::exited(
                1,
                "Unable to access interface: No such file or directory",
            )],
            5000,
        );
        assert!(result.unwrap_err().contains("Unable to access interface"));
    }

    #[test]
    fn only_harmless_wg_commands_are_run() {
        for argv in [latest_argv("t"), allowed_argv("t")] {
            let line = argv.join(" ");
            for secret in ["showconf", "dump", "private-key", "preshared-keys"] {
                assert!(!line.contains(secret), "{line}");
            }
        }
    }
}

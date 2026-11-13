//! The reachability check of a tunnel's probe host: one ICMP echo with the system `ping`, as
//! `net.py` did (no ICMP socket, which needs the Win32 API or raw sockets). A reply is
//! recognised by `TTL=` in the output, which is printed only for a real echo reply and in
//! every display language; "Destination host unreachable" and timeouts have none.

use std::sync::Arc;
use std::time::Duration;

use homelink_core::domain::Host;
use homelink_core::ports::{CommandRunner, Pinger};

/// Extra time given to the program beyond the echo timeout (starting it, name lookup).
const SLACK: Duration = Duration::from_secs(3);

/// Which command line syntax `ping` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PingFlavor {
    /// `ping -n 1 -w <milliseconds> <host>`.
    Windows,
    /// `ping -n -c 1 -W <seconds> <host>` (Linux).
    Unix,
}

impl PingFlavor {
    /// The syntax of the platform this program runs on.
    pub(crate) fn native() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// The command line for one echo request that waits at most `timeout` for the reply.
pub(crate) fn ping_argv(flavor: PingFlavor, host: &str, timeout: Duration) -> Vec<String> {
    let owned = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    match flavor {
        PingFlavor::Windows => {
            let millis = timeout.as_millis().max(1).to_string();
            owned(&["ping", "-n", "1", "-w", &millis, host])
        }
        PingFlavor::Unix => {
            // The Unix option takes whole seconds; round up so a short timeout is not zero.
            let seconds = timeout.as_secs() + u64::from(timeout.subsec_nanos() > 0);
            let seconds = seconds.max(1).to_string();
            owned(&["ping", "-n", "-c", "1", "-W", &seconds, host])
        }
    }
}

/// Whether the output of `ping` shows an echo reply.
pub(crate) fn is_echo_reply(output: &str) -> bool {
    output.to_ascii_uppercase().contains("TTL=")
}

/// A [`Pinger`] that runs the system `ping` through a command runner.
pub(crate) struct CommandPinger {
    runner: Arc<dyn CommandRunner>,
    flavor: PingFlavor,
}

impl CommandPinger {
    /// A pinger for the syntax of this platform.
    pub(crate) fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self::with_flavor(runner, PingFlavor::native())
    }

    /// A pinger with an explicit syntax.
    pub(crate) fn with_flavor(runner: Arc<dyn CommandRunner>, flavor: PingFlavor) -> Self {
        Self { runner, flavor }
    }
}

impl Pinger for CommandPinger {
    fn ping(&self, host: &Host, timeout: Duration) -> bool {
        let argv = ping_argv(self.flavor, host.as_str(), timeout);
        let output = self.runner.run(&argv, timeout + SLACK);
        is_echo_reply(&output.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::scripted::Scripted;
    use homelink_core::ports::CommandOutput;

    const REPLY_EN: &str = "\r\nPinging 192.168.88.1 with 32 bytes of data:\r\nReply from 192.168.88.1: bytes=32 time=12ms TTL=63\r\n\r\nPing statistics for 192.168.88.1:\r\n    Packets: Sent = 1, Received = 1, Lost = 0 (0% loss),\r\n";
    const TIMEOUT_EN: &str = "Pinging 192.168.88.1 with 32 bytes of data:\r\nRequest timed out.\r\n\r\nPing statistics for 192.168.88.1:\r\n    Packets: Sent = 1, Received = 0, Lost = 1 (100% loss),\r\n";
    const UNREACHABLE_EN: &str = "Pinging 192.168.88.1 with 32 bytes of data:\r\nReply from 192.168.1.1: Destination host unreachable.\r\n";
    const LINUX_REPLY: &str = "PING 10.8.0.1 (10.8.0.1) 56(84) bytes of data.\n64 bytes from 10.8.0.1: icmp_seq=1 ttl=64 time=0.040 ms\n";

    #[test]
    fn the_windows_command_line_gives_the_timeout_in_milliseconds() {
        assert_eq!(
            ping_argv(
                PingFlavor::Windows,
                "192.168.88.1",
                Duration::from_millis(1000)
            ),
            ["ping", "-n", "1", "-w", "1000", "192.168.88.1"]
        );
        assert_eq!(
            ping_argv(PingFlavor::Windows, "h", Duration::ZERO)
                .get(4)
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn the_unix_command_line_gives_whole_seconds_rounded_up() {
        let seconds = |ms| {
            ping_argv(PingFlavor::Unix, "h", Duration::from_millis(ms))
                .get(5)
                .cloned()
        };
        assert_eq!(seconds(1000).as_deref(), Some("1"));
        assert_eq!(seconds(1001).as_deref(), Some("2"));
        assert_eq!(seconds(200).as_deref(), Some("1"));
        assert_eq!(seconds(0).as_deref(), Some("1"));
        assert_eq!(
            ping_argv(PingFlavor::Unix, "10.8.0.1", Duration::from_secs(2)),
            ["ping", "-n", "-c", "1", "-W", "2", "10.8.0.1"]
        );
    }

    #[test]
    fn only_a_real_reply_has_a_ttl() {
        assert!(is_echo_reply(REPLY_EN));
        assert!(is_echo_reply(LINUX_REPLY));
        assert!(!is_echo_reply(TIMEOUT_EN));
        assert!(!is_echo_reply(UNREACHABLE_EN));
        assert!(!is_echo_reply(""));
    }

    #[test]
    fn a_reply_in_another_display_language_is_still_a_reply() {
        // Russian Windows: the words are translated, "TTL=" is not. "Ответ от" in code page 866.
        let bytes = b"\x8e\xe2\xa2\xa5\xe2 \xae\xe2 192.168.88.1: bytes=32 TTL=63";
        let text = crate::windows::oem::decode(Some(866), bytes);
        assert!(text.starts_with("Ответ от"), "{text}");
        assert!(is_echo_reply(&text));
    }

    #[test]
    fn the_pinger_runs_ping_with_the_timeout_plus_slack() {
        let runner = Arc::new(Scripted::new(vec![(
            "ping",
            CommandOutput::exited(0, REPLY_EN),
        )]));
        let pinger = CommandPinger::with_flavor(runner.clone(), PingFlavor::Windows);
        let host = Host::parse("192.168.88.1").unwrap();
        assert!(pinger.ping(&host, Duration::from_millis(1000)));
        let calls = runner.calls();
        let (argv, timeout) = calls.first().unwrap();
        assert_eq!(argv.last().map(String::as_str), Some("192.168.88.1"));
        assert_eq!(*timeout, Duration::from_millis(1000) + SLACK);
    }

    #[test]
    fn no_reply_and_no_program_both_mean_unreachable() {
        let host = Host::parse("192.168.88.1").unwrap();
        let silent = Arc::new(Scripted::new(vec![(
            "ping",
            CommandOutput::exited(1, TIMEOUT_EN),
        )]));
        assert!(
            !CommandPinger::with_flavor(silent, PingFlavor::Windows)
                .ping(&host, Duration::from_secs(1))
        );
        let missing = Arc::new(Scripted::new(vec![]));
        assert!(
            !CommandPinger::with_flavor(missing, PingFlavor::Windows)
                .ping(&host, Duration::from_secs(1))
        );
    }
}

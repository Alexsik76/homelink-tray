//! Wake senders that need no platform code.
//!
//! [`MikrotikSshWake`] wakes a PC by running the RouterOS command
//! `/tool wol mac=<mac> interface=<interface>` on the router over SSH, with the system
//! OpenSSH client and key authentication (`BatchMode=yes`, so `ssh` never asks for a
//! password and the program never holds one). The command line is built here as a plain
//! argument list; running it goes through [`CommandRunner`], so this module is tested with
//! a fake runner and never starts a process itself.

use std::sync::Arc;
use std::time::Duration;

use crate::domain::{InterfaceName, MacAddr, Router, WakeMethod};
use crate::ports::{CommandRunner, WakeError, WakeRequest, WakeSender};

/// The program that is run; resolved through `PATH` by the runner.
pub const SSH_PROGRAM: &str = "ssh";

/// The RouterOS command that sends the magic packet.
///
/// `mac` is six hex pairs and `interface` is limited to `A-Za-z0-9_.-` by their types, so
/// neither can add RouterOS or shell syntax.
pub fn routeros_wol_command(mac: &MacAddr, interface: &InterfaceName) -> String {
    format!("/tool wol mac={mac} interface={interface}")
}

/// The `ssh` command line that runs `remote_command` on a router.
///
/// Options, in order: key authentication only, the key given explicitly and no other
/// (`IdentitiesOnly`), the host key must already be known (`StrictHostKeyChecking=yes`),
/// the connection timeout, port and user as separate arguments, then `--`, the host and the
/// remote command as a single argument.
pub fn router_ssh_argv(router: &Router, remote_command: &str) -> Vec<String> {
    let connect_timeout = router.timeout.as_secs().max(1);
    let mut argv: Vec<String> = [
        crate::wake::SSH_PROGRAM,
        "-o",
        "BatchMode=yes",
        "-o",
        "PasswordAuthentication=no",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "StrictHostKeyChecking=yes",
    ]
    .map(str::to_owned)
    .into();
    argv.push("-o".into());
    argv.push(format!("ConnectTimeout={connect_timeout}"));
    argv.push("-i".into());
    argv.push(router.identity_file.clone());
    argv.push("-p".into());
    argv.push(router.port.to_string());
    argv.push("-l".into());
    argv.push(router.user.clone());
    argv.push("--".into());
    argv.push(router.host.to_string());
    argv.push(remote_command.to_owned());
    argv
}

/// Wakes a PC through a MikroTik router over SSH.
pub struct MikrotikSshWake {
    runner: Arc<dyn CommandRunner>,
}

impl MikrotikSshWake {
    /// A sender that runs `ssh` through `runner`.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

impl WakeSender for MikrotikSshWake {
    fn wake(&self, request: &WakeRequest<'_>) -> Result<(), WakeError> {
        let WakeMethod::MikrotikSsh { mac, interface, .. } = request.method else {
            return Err(WakeError::Unsupported);
        };
        let router = request.router.ok_or_else(|| {
            WakeError::Failed("the router of this PC is missing from the configuration".into())
        })?;
        let argv = router_ssh_argv(router, &routeros_wol_command(mac, interface));
        log::info!("waking {} through router {}", request.pc.id, router.id);
        let output = self.runner.run(&argv, command_timeout(router.timeout));
        if output.code == 0 {
            Ok(())
        } else {
            let text = output.text.trim();
            Err(WakeError::Failed(if text.is_empty() {
                format!("ssh exited with code {}", output.code)
            } else {
                text.to_owned()
            }))
        }
    }
}

/// The runner gets a little longer than the connection timeout so `ssh` can report its own
/// timeout before the runner kills it.
fn command_timeout(router_timeout: Duration) -> Duration {
    router_timeout + Duration::from_secs(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Host, RouterId, RouterKind};
    use crate::ports::CommandOutput;
    use crate::testing::{FakeCommandRunner, multi_config};

    fn router() -> Router {
        Router {
            id: RouterId::parse("mikrotik").unwrap(),
            kind: RouterKind::Mikrotik,
            host: Host::parse("192.168.88.1").unwrap(),
            port: 2222,
            user: "homelink".into(),
            identity_file: r"C:\HomeLink\router_key".into(),
            timeout: Duration::from_secs(8),
        }
    }

    fn mac() -> MacAddr {
        MacAddr::parse("aa:bb:cc:dd:ee:ff").unwrap()
    }

    #[test]
    fn the_routeros_command_is_exactly_tool_wol() {
        let interface = InterfaceName::parse("bridge").unwrap();
        assert_eq!(
            routeros_wol_command(&mac(), &interface),
            "/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge"
        );
    }

    #[test]
    fn the_ssh_command_line_is_key_only_strict_and_explicit() {
        let argv = router_ssh_argv(
            &router(),
            "/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge",
        );
        assert_eq!(
            argv,
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "PasswordAuthentication=no",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ConnectTimeout=8",
                "-i",
                r"C:\HomeLink\router_key",
                "-p",
                "2222",
                "-l",
                "homelink",
                "--",
                "192.168.88.1",
                "/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge",
            ]
        );
    }

    #[test]
    fn the_remote_command_is_one_argument_and_follows_the_host() {
        let argv = router_ssh_argv(
            &router(),
            "/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge",
        );
        let dashes = argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(
            argv.len(),
            dashes + 3,
            "host and one command argument follow `--`"
        );
    }

    #[test]
    fn a_sub_second_timeout_still_gives_ssh_a_connect_timeout() {
        let mut r = router();
        r.timeout = Duration::from_millis(300);
        assert!(router_ssh_argv(&r, "x").contains(&"ConnectTimeout=1".to_owned()));
    }

    /// Everything that reaches the remote command is either typed (a MAC) or from a closed
    /// character set (an interface), so no hostile value can get past parsing.
    #[test]
    fn hostile_values_never_reach_the_command() {
        for hostile in [
            "bridge; /system reboot",
            "bridge\n/system reboot",
            "bridge interface=other",
            "$(reboot)",
            "`reboot`",
            "bridge\" ; \"",
            "[/system reboot]",
            "a&&b",
            "a|b",
            "a>b",
            "a\\b",
        ] {
            assert!(InterfaceName::parse(hostile).is_err(), "{hostile:?}");
        }
        for hostile in [
            "AA:BB:CC:DD:EE:FF; reboot",
            "AA:BB:CC:DD:EE:FF interface=x",
            "$(x)",
        ] {
            assert!(MacAddr::parse(hostile).is_err(), "{hostile:?}");
        }
        // And what is accepted renders to the exact expected shape.
        let interface = InterfaceName::parse("vlan-10").unwrap();
        let command = routeros_wol_command(&mac(), &interface);
        assert_eq!(command.split(' ').count(), 4, "{command}");
    }

    #[test]
    fn the_sender_runs_the_command_through_the_runner() {
        let config = multi_config();
        let pc = config.pc(&crate::testing::pc_id("desktop")).unwrap();
        let method = pc.wake.as_ref().unwrap();
        let runner = Arc::new(FakeCommandRunner::default());
        let sender = MikrotikSshWake::new(runner.clone());
        let request = WakeRequest {
            pc,
            method,
            router: config.router(&router().id),
        };
        sender.wake(&request).unwrap();
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let (argv, timeout) = &calls[0];
        assert_eq!(argv.first().map(String::as_str), Some("ssh"));
        assert_eq!(
            argv.last().unwrap(),
            "/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge"
        );
        assert_eq!(*timeout, Duration::from_secs(10));
    }

    #[test]
    fn a_failing_command_reports_its_output_or_its_exit_code() {
        let config = multi_config();
        let pc = config.pc(&crate::testing::pc_id("desktop")).unwrap();
        let method = pc.wake.as_ref().unwrap();
        let request = WakeRequest {
            pc,
            method,
            router: config.router(&router().id),
        };

        let runner = Arc::new(FakeCommandRunner::default());
        runner.set_result(CommandOutput {
            code: 255,
            text: "Permission denied\n".into(),
        });
        let err = MikrotikSshWake::new(runner).wake(&request).unwrap_err();
        assert_eq!(err, WakeError::Failed("Permission denied".into()));

        let runner = Arc::new(FakeCommandRunner::default());
        runner.set_result(CommandOutput {
            code: 1,
            text: String::new(),
        });
        let err = MikrotikSshWake::new(runner).wake(&request).unwrap_err();
        assert_eq!(err, WakeError::Failed("ssh exited with code 1".into()));
    }

    #[test]
    fn a_missing_router_or_another_method_is_refused_without_running_anything() {
        let config = multi_config();
        let runner = Arc::new(FakeCommandRunner::default());
        let sender = MikrotikSshWake::new(runner.clone());

        let pc = config.pc(&crate::testing::pc_id("desktop")).unwrap();
        let method = pc.wake.as_ref().unwrap();
        assert!(
            sender
                .wake(&WakeRequest {
                    pc,
                    method,
                    router: None
                })
                .is_err()
        );

        let nas = config.pc(&crate::testing::pc_id("nas")).unwrap();
        let method = nas.wake.as_ref().unwrap();
        assert_eq!(
            sender.wake(&WakeRequest {
                pc: nas,
                method,
                router: None
            }),
            Err(WakeError::Unsupported)
        );
        assert!(runner.calls().is_empty());
    }
}

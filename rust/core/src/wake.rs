//! Wake senders that need no platform code.
//!
//! [`RelayWake`] wakes a PC by telling a relay host (a router, an OpenWrt box, any
//! always-on device in the PC's network) to send the magic packet. The relay is reached
//! over SSH with the system OpenSSH client and key authentication (see [`crate::ssh`]); the
//! command it runs is chosen by the relay's preset ([`RelayKind`]):
//!
//! | preset      | remote command                                  | `interface` | `address` | `sudo` on the relay |
//! | ----------- | ----------------------------------------------- | ----------- | --------- | ------------------- |
//! | `routeros`  | `/tool wol mac=<mac> interface=<interface>`     | required    | not used  | rejected            |
//! | `etherwake` | `[sudo -n ]etherwake -i <interface> <mac>`      | required    | not used  | required            |
//! | `wakeonlan` | `[sudo -n ]wakeonlan [-i <address>] <mac>`      | not used    | optional  | required            |
//!
//! The command is one argument of the `ssh` argument list, never a shell string built from
//! free text: `mac` is a parsed MAC address, `interface` an [`InterfaceName`] (letters,
//! digits, `_.-`, no leading `-`) and `address` an `Ipv4Addr`, so none of them can add
//! syntax or options; `sudo -n ` is a fixed prefix (`-n`: never ask for a password).
//! Running the list goes through [`CommandRunner`], so this module is tested with a fake
//! runner and never starts a process itself.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use crate::domain::{InterfaceName, InvalidValue, MacAddr, Relay, RelayKind, WakeMethod};
use crate::ports::{CommandRunner, WakeError, WakeRequest, WakeSender};
use crate::ssh::{SshTarget, explain_ssh, ssh_argv};

/// The remote command that makes a relay of kind `kind` send the magic packet.
///
/// Fails when the arguments do not fit the preset: a missing `interface` for a preset that
/// needs one, an `interface` or `address` for a preset that does not use it, `sudo` for a
/// preset that has none.
pub fn relay_command(
    kind: RelayKind,
    sudo: bool,
    mac: &MacAddr,
    interface: Option<&InterfaceName>,
    address: Option<Ipv4Addr>,
) -> Result<String, InvalidValue> {
    let name = kind.code();
    if kind.uses_interface() && interface.is_none() {
        return Err(InvalidValue(format!(
            "the '{name}' preset needs an interface"
        )));
    }
    if !kind.uses_interface() && interface.is_some() {
        return Err(InvalidValue(format!(
            "the '{name}' preset does not use an interface"
        )));
    }
    if !kind.uses_address() && address.is_some() {
        return Err(InvalidValue(format!(
            "the '{name}' preset does not use an address"
        )));
    }
    if sudo && !kind.supports_sudo() {
        return Err(InvalidValue(format!("the '{name}' preset has no sudo")));
    }
    let command = match (kind, interface, address) {
        (RelayKind::RouterOs, Some(interface), _) => {
            format!("/tool wol mac={mac} interface={interface}")
        }
        (RelayKind::Etherwake, Some(interface), _) => format!("etherwake -i {interface} {mac}"),
        (RelayKind::Wakeonlan, _, Some(address)) => format!("wakeonlan -i {address} {mac}"),
        (RelayKind::Wakeonlan, _, None) => format!("wakeonlan {mac}"),
        // Unreachable: the checks above reject a missing interface.
        _ => {
            return Err(InvalidValue(format!(
                "the '{name}' preset needs an interface"
            )));
        }
    };
    Ok(if sudo {
        format!("sudo -n {command}")
    } else {
        command
    })
}

/// The `ssh` command line that runs `remote_command` on a relay (see [`crate::ssh`]).
pub fn relay_ssh_argv(relay: &Relay, remote_command: &str) -> Vec<String> {
    let target = SshTarget {
        host: &relay.host,
        port: relay.port,
        user: &relay.user,
        identity_file: &relay.identity_file,
        timeout: relay.timeout,
    };
    ssh_argv(&target, remote_command)
}

/// Wakes a PC through a relay host over SSH.
pub struct RelayWake {
    runner: Arc<dyn CommandRunner>,
}

impl RelayWake {
    /// A sender that runs `ssh` through `runner`.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

impl WakeSender for RelayWake {
    fn wake(&self, request: &WakeRequest<'_>) -> Result<(), WakeError> {
        let WakeMethod::Relay {
            mac,
            interface,
            address,
            ..
        } = request.method
        else {
            return Err(WakeError::Unsupported);
        };
        let relay = request.relay.ok_or_else(|| {
            WakeError::Failed("the relay of this PC is missing from the configuration".into())
        })?;
        let command = relay_command(relay.kind, relay.sudo, mac, interface.as_ref(), *address)
            .map_err(|err| WakeError::Failed(err.to_string()))?;
        let argv = relay_ssh_argv(relay, &command);
        log::info!("waking {} through relay {}", request.pc.id, relay.id);
        let output = self.runner.run(&argv, command_timeout(relay.timeout));
        match explain_ssh(&output) {
            None => Ok(()),
            Some(reason) => Err(WakeError::Failed(reason)),
        }
    }
}

/// The runner gets a little longer than the connection timeout so `ssh` can report its own
/// timeout before the runner kills it.
pub(crate) fn command_timeout(timeout: Duration) -> Duration {
    timeout + Duration::from_secs(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Host, RelayId};
    use crate::ports::CommandOutput;
    use crate::testing::{FakeCommandRunner, multi_config, pc_id, relay_config};

    fn relay(kind: RelayKind, sudo: bool) -> Relay {
        Relay {
            id: RelayId::parse("gateway").unwrap(),
            kind,
            host: Host::parse("192.168.88.1").unwrap(),
            port: 2222,
            user: "homelink".into(),
            identity_file: r"C:\HomeLink\relay_key".into(),
            timeout: Duration::from_secs(8),
            sudo,
        }
    }

    fn mac() -> MacAddr {
        MacAddr::parse("aa:bb:cc:dd:ee:ff").unwrap()
    }

    fn interface(name: &str) -> InterfaceName {
        InterfaceName::parse(name).unwrap()
    }

    /// The exact `ssh` argument list for a remote command on `relay()`.
    fn expected_argv(remote: &str) -> Vec<String> {
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
            r"C:\HomeLink\relay_key",
            "-p",
            "2222",
            "-l",
            "homelink",
            "--",
            "192.168.88.1",
            remote,
        ]
        .map(str::to_owned)
        .into()
    }

    // ---- exact commands and argument lists, one set per preset ----

    #[test]
    fn routeros_runs_tool_wol() {
        let command = relay_command(
            RelayKind::RouterOs,
            false,
            &mac(),
            Some(&interface("bridge")),
            None,
        )
        .unwrap();
        assert_eq!(command, "/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge");
        assert_eq!(
            relay_ssh_argv(&relay(RelayKind::RouterOs, false), &command),
            expected_argv("/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge")
        );
    }

    #[test]
    fn etherwake_runs_etherwake_on_an_interface_with_or_without_sudo() {
        let i = interface("br-lan");
        let plain = relay_command(RelayKind::Etherwake, false, &mac(), Some(&i), None).unwrap();
        assert_eq!(plain, "etherwake -i br-lan AA:BB:CC:DD:EE:FF");
        assert_eq!(
            relay_ssh_argv(&relay(RelayKind::Etherwake, false), &plain),
            expected_argv("etherwake -i br-lan AA:BB:CC:DD:EE:FF")
        );

        let sudo = relay_command(RelayKind::Etherwake, true, &mac(), Some(&i), None).unwrap();
        assert_eq!(sudo, "sudo -n etherwake -i br-lan AA:BB:CC:DD:EE:FF");
        assert_eq!(
            relay_ssh_argv(&relay(RelayKind::Etherwake, true), &sudo),
            expected_argv("sudo -n etherwake -i br-lan AA:BB:CC:DD:EE:FF")
        );
    }

    #[test]
    fn wakeonlan_runs_wakeonlan_with_or_without_an_address_and_sudo() {
        let addr = Some(Ipv4Addr::new(192, 168, 10, 255));
        let cases = [
            (false, None, "wakeonlan AA:BB:CC:DD:EE:FF"),
            (false, addr, "wakeonlan -i 192.168.10.255 AA:BB:CC:DD:EE:FF"),
            (true, None, "sudo -n wakeonlan AA:BB:CC:DD:EE:FF"),
            (
                true,
                addr,
                "sudo -n wakeonlan -i 192.168.10.255 AA:BB:CC:DD:EE:FF",
            ),
        ];
        for (sudo, address, expected) in cases {
            let command = relay_command(RelayKind::Wakeonlan, sudo, &mac(), None, address).unwrap();
            assert_eq!(command, expected);
            assert_eq!(
                relay_ssh_argv(&relay(RelayKind::Wakeonlan, sudo), &command),
                expected_argv(expected)
            );
        }
    }

    #[test]
    fn the_remote_command_is_one_argument_after_the_host() {
        let argv = relay_ssh_argv(
            &relay(RelayKind::Etherwake, true),
            "sudo -n etherwake -i br0 AA:BB:CC:DD:EE:FF",
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
        let mut r = relay(RelayKind::RouterOs, false);
        r.timeout = Duration::from_millis(300);
        assert!(relay_ssh_argv(&r, "x").contains(&"ConnectTimeout=1".to_owned()));
    }

    // ---- the fields a preset needs, and the ones it refuses ----

    #[test]
    fn a_preset_needs_its_fields_and_refuses_the_others() {
        let i = interface("bridge");
        let a = Some(Ipv4Addr::new(10, 0, 0, 255));
        for kind in [RelayKind::RouterOs, RelayKind::Etherwake] {
            assert!(
                relay_command(kind, false, &mac(), None, None).is_err(),
                "{kind:?} needs an interface"
            );
            assert!(
                relay_command(kind, false, &mac(), Some(&i), a).is_err(),
                "{kind:?} has no address"
            );
            assert!(relay_command(kind, false, &mac(), Some(&i), None).is_ok());
        }
        assert!(relay_command(RelayKind::Wakeonlan, false, &mac(), Some(&i), None).is_err());
        assert!(relay_command(RelayKind::Wakeonlan, false, &mac(), None, None).is_ok());
        assert!(relay_command(RelayKind::Wakeonlan, false, &mac(), None, a).is_ok());
    }

    #[test]
    fn sudo_exists_only_for_presets_that_support_it() {
        let i = interface("bridge");
        assert!(relay_command(RelayKind::RouterOs, true, &mac(), Some(&i), None).is_err());
        assert!(relay_command(RelayKind::Etherwake, true, &mac(), Some(&i), None).is_ok());
        assert!(relay_command(RelayKind::Wakeonlan, true, &mac(), None, None).is_ok());
        for kind in RelayKind::ALL {
            assert_eq!(
                kind.supports_sudo(),
                kind != RelayKind::RouterOs,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn every_preset_has_a_unique_code_and_consistent_capabilities() {
        let mut codes: Vec<&str> = RelayKind::ALL.iter().map(|k| k.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), RelayKind::ALL.len());
        for kind in RelayKind::ALL {
            // Every preset can build a command from the fields it declares.
            let i = interface("eth0");
            let interface = kind.uses_interface().then_some(&i);
            assert!(
                relay_command(kind, kind.supports_sudo(), &mac(), interface, None).is_ok(),
                "{kind:?}"
            );
        }
    }

    // ---- nothing hostile reaches the command ----

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
            "-h",
            "--help",
            "-i",
        ] {
            assert!(InterfaceName::parse(hostile).is_err(), "{hostile:?}");
        }
        for hostile in [
            "AA:BB:CC:DD:EE:FF; reboot",
            "AA:BB:CC:DD:EE:FF interface=x",
            "$(x)",
            "AA:BB:CC:DD:EE:FF -i",
        ] {
            assert!(MacAddr::parse(hostile).is_err(), "{hostile:?}");
        }
        for hostile in [
            "10.0.0.1; reboot",
            "10.0.0.1 -x",
            "-1.0.0.1",
            "$(x)",
            "10.0.0.256",
        ] {
            assert!(hostile.parse::<Ipv4Addr>().is_err(), "{hostile:?}");
        }
    }

    #[test]
    fn what_is_accepted_renders_to_the_exact_expected_shape() {
        let i = interface("vlan-10");
        let routeros = relay_command(RelayKind::RouterOs, false, &mac(), Some(&i), None).unwrap();
        assert_eq!(routeros.split(' ').count(), 4, "{routeros}");
        let etherwake = relay_command(RelayKind::Etherwake, true, &mac(), Some(&i), None).unwrap();
        assert_eq!(etherwake.split(' ').count(), 6, "{etherwake}");
        let wol = relay_command(
            RelayKind::Wakeonlan,
            true,
            &mac(),
            None,
            Some(Ipv4Addr::new(10, 0, 0, 1)),
        )
        .unwrap();
        assert_eq!(wol.split(' ').count(), 6, "{wol}");
    }

    // ---- the sender ----

    fn request_for<'a>(config: &'a crate::config::Config, pc: &str) -> WakeRequest<'a> {
        let pc = config.pc(&pc_id(pc)).unwrap();
        let method = pc.wake.as_ref().unwrap();
        let relay = match method {
            WakeMethod::Relay { relay, .. } => config.relay(relay),
            _ => None,
        };
        WakeRequest { pc, method, relay }
    }

    #[test]
    fn the_sender_runs_the_command_for_each_preset_through_the_runner() {
        let config = relay_config();
        let cases = [
            (
                "desktop",
                "/tool wol mac=AA:BB:CC:DD:EE:FF interface=bridge",
                10,
            ),
            ("media", "etherwake -i br-lan 00:11:22:33:44:55", 10),
            (
                "kiosk",
                "sudo -n wakeonlan -i 10.0.5.255 00:11:22:33:44:66",
                10,
            ),
        ];
        for (pc, remote, timeout) in cases {
            let runner = Arc::new(FakeCommandRunner::default());
            let sender = RelayWake::new(runner.clone());
            sender.wake(&request_for(&config, pc)).unwrap();
            let calls = runner.calls();
            assert_eq!(calls.len(), 1, "{pc}");
            let (argv, t) = &calls[0];
            assert_eq!(argv.first().map(String::as_str), Some("ssh"), "{pc}");
            assert_eq!(argv.last().unwrap(), remote, "{pc}");
            assert_eq!(*t, Duration::from_secs(timeout), "{pc}");
        }
    }

    #[test]
    fn failures_are_told_apart() {
        let config = relay_config();
        let request = request_for(&config, "desktop");
        let cases = [
            (
                CommandOutput::exited(255, "Permission denied\n"),
                "ssh could not connect or authenticate: Permission denied",
            ),
            (
                CommandOutput::exited(1, "no such interface"),
                "the command on the remote host failed (exit code 1): no such interface",
            ),
            (
                CommandOutput::exited(1, ""),
                "the command on the remote host failed (exit code 1)",
            ),
            (
                CommandOutput::timed_out(Duration::from_secs(10), ""),
                "ssh did not finish in time: timed out after 10 s",
            ),
            (
                CommandOutput::not_started("No such file or directory"),
                "cannot run ssh (is the OpenSSH client installed?): No such file or directory",
            ),
        ];
        for (output, expected) in cases {
            let runner = Arc::new(FakeCommandRunner::default());
            runner.set_result(output);
            let err = RelayWake::new(runner).wake(&request).unwrap_err();
            assert_eq!(err, WakeError::Failed(expected.into()));
        }
    }

    #[test]
    fn a_missing_relay_or_another_method_is_refused_without_running_anything() {
        let config = relay_config();
        let runner = Arc::new(FakeCommandRunner::default());
        let sender = RelayWake::new(runner.clone());

        let mut request = request_for(&config, "desktop");
        request.relay = None;
        assert!(sender.wake(&request).is_err());

        // A PC that wakes by magic packet is not this sender's business.
        let multi = multi_config();
        let mut request = request_for(&multi, "nas");
        request.relay = None;
        assert_eq!(sender.wake(&request), Err(WakeError::Unsupported));
        assert!(runner.calls().is_empty());
    }
}

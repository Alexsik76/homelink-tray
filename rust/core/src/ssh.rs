//! The `ssh` command line, shared by relay wake and PC power actions.
//!
//! Every connection of the program is key-only, with the key given explicitly: `BatchMode`
//! and `PasswordAuthentication=no` (no prompts, no passwords), `-i <key>` with
//! `IdentitiesOnly=yes` (only that key is offered; the elevated program must not depend on
//! which profile `ssh` resolves) and `StrictHostKeyChecking=yes` (the host key must already
//! be known; only the wizard may accept a new one). Building the list is pure; running it
//! goes through [`crate::ports::CommandRunner`].

use std::time::Duration;

use crate::domain::{Host, Os, Pc, PowerAction, PowerMethod};

/// The program that is run; resolved through `PATH` by the runner.
pub const SSH_PROGRAM: &str = "ssh";

/// Where and as whom to connect.
#[derive(Debug, Clone, Copy)]
pub struct SshTarget<'a> {
    /// Address.
    pub host: &'a Host,
    /// Port.
    pub port: u16,
    /// User.
    pub user: &'a str,
    /// Absolute path of the private key.
    pub identity_file: &'a str,
    /// Connection timeout.
    pub timeout: Duration,
}

/// The `ssh` command line that runs `remote_command` on `target`.
///
/// Options, in order: key authentication only, the key given explicitly and no other, the
/// host key must be known, the connection timeout, port and user as separate arguments, then
/// `--`, the host and the remote command as a single argument.
pub fn ssh_argv(target: &SshTarget<'_>, remote_command: &str) -> Vec<String> {
    let connect_timeout = target.timeout.as_secs().max(1);
    let mut argv: Vec<String> = [
        SSH_PROGRAM,
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
    argv.push(target.identity_file.to_owned());
    argv.push("-p".into());
    argv.push(target.port.to_string());
    argv.push("-l".into());
    argv.push(target.user.to_owned());
    argv.push("--".into());
    argv.push(target.host.to_string());
    argv.push(remote_command.to_owned());
    argv
}

/// The remote command that performs `action` on a PC running `os`.
pub fn power_remote_command(os: Os, action: PowerAction) -> &'static str {
    match (os, action) {
        (Os::Linux, PowerAction::Shutdown) => "sudo systemctl poweroff",
        // `-i` ignores inhibitors, which GNOME sets while a Remote Desktop session exists.
        (Os::Linux, PowerAction::Sleep) => "sudo systemctl suspend -i",
        (Os::Windows, PowerAction::Shutdown) => "shutdown /s /t 0",
        (Os::Windows, PowerAction::Sleep) => "rundll32.exe powrprof.dll,SetSuspendState 0,1,0",
    }
}

/// The `ssh` command line for a power action of a PC with an SSH power method; `None` for
/// any other method.
pub fn power_ssh_argv(pc: &Pc, action: PowerAction) -> Option<Vec<String>> {
    let Some(PowerMethod::Ssh {
        user,
        port,
        identity_file,
    }) = &pc.power
    else {
        return None;
    };
    let target = SshTarget {
        host: &pc.host,
        port: *port,
        user,
        identity_file,
        timeout: pc.timing.power_timeout,
    };
    Some(ssh_argv(&target, power_remote_command(pc.os, action)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{multi_config, pc_id};

    #[test]
    fn a_power_command_line_is_key_only_strict_and_explicit() {
        let config = multi_config();
        let desktop = config.pc(&pc_id("desktop")).unwrap();
        let argv = power_ssh_argv(desktop, PowerAction::Sleep).unwrap();
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
                "ConnectTimeout=15",
                "-i",
                r"C:\HomeLink\pc_key",
                "-p",
                "22",
                "-l",
                "alex",
                "--",
                "192.168.88.10",
                "sudo systemctl suspend -i",
            ]
        );
    }

    #[test]
    fn every_ssh_command_passes_the_key_and_identities_only() {
        let config = multi_config();
        for id in ["desktop", "nas"] {
            let pc = config.pc(&pc_id(id)).unwrap();
            for action in [PowerAction::Sleep, PowerAction::Shutdown] {
                let argv = power_ssh_argv(pc, action).unwrap();
                assert!(argv.contains(&"IdentitiesOnly=yes".to_owned()), "{id}");
                let i = argv.iter().position(|a| a == "-i").unwrap();
                assert_eq!(
                    argv.get(i + 1).map(String::as_str),
                    Some(r"C:\HomeLink\pc_key")
                );
            }
        }
    }

    #[test]
    fn presets_follow_the_operating_system() {
        assert_eq!(
            power_remote_command(Os::Linux, PowerAction::Shutdown),
            "sudo systemctl poweroff"
        );
        assert_eq!(
            power_remote_command(Os::Windows, PowerAction::Shutdown),
            "shutdown /s /t 0"
        );
        assert!(power_remote_command(Os::Windows, PowerAction::Sleep).contains("SetSuspendState"));
    }

    #[test]
    fn other_methods_have_no_ssh_command() {
        let config = multi_config();
        let laptop = config.pc(&pc_id("laptop")).unwrap(); // a command method
        assert!(power_ssh_argv(laptop, PowerAction::Shutdown).is_none());
        let office = config.pc(&pc_id("office-pc")).unwrap(); // no power method
        assert!(power_ssh_argv(office, PowerAction::Sleep).is_none());
    }
}

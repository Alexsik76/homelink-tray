//! The commands the user runs on a PC so that HomeLink can put it to sleep and shut it down
//! over SSH (step 4 of the wizard, section 11 of `docs/rust-architecture.md`). Like
//! [`crate::relay_setup`] they are text only: the program never runs them anywhere.
//!
//! - **Linux** (as root): the public key in the user's `authorized_keys`, and a `sudoers.d` file
//!   that lets the user run exactly the two preset commands without a password
//!   (`systemctl poweroff` and `systemctl suspend -i`, see [`crate::ssh::power_remote_command`]).
//! - **Windows** (PowerShell as administrator): the OpenSSH server, started now and at boot, and
//!   the public key in `administrators_authorized_keys`, readable only by Administrators and
//!   SYSTEM (named by their SIDs, so a localised Windows works too). The user must be an
//!   administrator of the PC: `shutdown` over a network logon needs that right. **Not yet verified
//!   on a real Windows PC** (see the release checklist).
//!
//! What is not known is a placeholder in angle brackets, as for the relay.

use crate::domain::Os;
use crate::relay_setup::{PLACEHOLDER_KEY, PLACEHOLDER_USER, safe_key, safe_user};

/// The `sudoers.d` file on a Linux PC.
pub const SUDOERS_FILE: &str = "/etc/sudoers.d/homelink";

/// What the commands depend on.
#[derive(Debug, Clone, Copy)]
pub struct PcSetup<'a> {
    /// The operating system of the PC (the preset of the SSH power method).
    pub os: Os,
    /// The SSH user HomeLink logs in as.
    pub user: &'a str,
    /// The public key line, once there is one.
    pub public_key: Option<&'a str>,
}

/// The command lines, in the order they are run.
pub fn commands(setup: &PcSetup<'_>) -> Vec<String> {
    let user = safe_user(setup.user.trim()).unwrap_or(PLACEHOLDER_USER);
    let key = setup
        .public_key
        .and_then(safe_key)
        .unwrap_or(PLACEHOLDER_KEY);
    match setup.os {
        Os::Linux => linux(user, key),
        Os::Windows => windows(key),
    }
}

/// The commands as one text, a line each.
pub fn text(setup: &PcSetup<'_>) -> String {
    commands(setup).join("\n")
}

fn linux(user: &str, key: &str) -> Vec<String> {
    vec![
        format!("install -d -m 700 -o {user} ~{user}/.ssh"),
        format!("echo '{key}' >> ~{user}/.ssh/authorized_keys"),
        format!(
            "chown {user} ~{user}/.ssh/authorized_keys && chmod 600 ~{user}/.ssh/authorized_keys"
        ),
        format!(
            "echo \"{user} ALL=(root) NOPASSWD: $(command -v systemctl) poweroff, $(command -v systemctl) suspend -i\" > {SUDOERS_FILE}"
        ),
        format!("chmod 440 {SUDOERS_FILE} && visudo -cf {SUDOERS_FILE}"),
    ]
}

/// The key file of administrators, as the OpenSSH server of Windows reads it.
const ADMIN_KEYS: &str = r"$env:ProgramData\ssh\administrators_authorized_keys";

fn windows(key: &str) -> Vec<String> {
    vec![
        "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0".to_owned(),
        "Set-Service -Name sshd -StartupType Automatic; Start-Service sshd".to_owned(),
        format!("Add-Content -Path \"{ADMIN_KEYS}\" -Value '{key}'"),
        format!(
            "icacls.exe \"{ADMIN_KEYS}\" /inheritance:r /grant \"*S-1-5-32-544:F\" /grant \"*S-1-5-18:F\""
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc homelink";

    #[test]
    fn linux_gets_the_key_and_a_sudoers_rule_for_exactly_the_two_commands() {
        let lines = commands(&PcSetup {
            os: Os::Linux,
            user: "alex",
            public_key: Some(KEY),
        });
        assert_eq!(
            lines,
            [
                "install -d -m 700 -o alex ~alex/.ssh",
                "echo 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc homelink' >> ~alex/.ssh/authorized_keys",
                "chown alex ~alex/.ssh/authorized_keys && chmod 600 ~alex/.ssh/authorized_keys",
                "echo \"alex ALL=(root) NOPASSWD: $(command -v systemctl) poweroff, $(command -v systemctl) suspend -i\" > /etc/sudoers.d/homelink",
                "chmod 440 /etc/sudoers.d/homelink && visudo -cf /etc/sudoers.d/homelink",
            ]
        );
    }

    #[test]
    fn the_sudoers_rule_matches_the_commands_the_program_sends() {
        let text = text(&PcSetup {
            os: Os::Linux,
            user: "alex",
            public_key: None,
        });
        for action in [
            crate::domain::PowerAction::Shutdown,
            crate::domain::PowerAction::Sleep,
        ] {
            let sent = crate::ssh::power_remote_command(Os::Linux, action);
            let args = sent.strip_prefix("sudo systemctl ").unwrap_or(sent);
            assert!(
                text.contains(&format!("$(command -v systemctl) {args}")),
                "{sent}"
            );
        }
    }

    #[test]
    fn windows_installs_the_server_and_restricts_the_administrators_key_file() {
        let lines = commands(&PcSetup {
            os: Os::Windows,
            user: "alex",
            public_key: Some(KEY),
        });
        assert_eq!(
            lines,
            [
                "Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0",
                "Set-Service -Name sshd -StartupType Automatic; Start-Service sshd",
                "Add-Content -Path \"$env:ProgramData\\ssh\\administrators_authorized_keys\" -Value 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc homelink'",
                "icacls.exe \"$env:ProgramData\\ssh\\administrators_authorized_keys\" /inheritance:r /grant \"*S-1-5-32-544:F\" /grant \"*S-1-5-18:F\"",
            ]
        );
    }

    #[test]
    fn unsafe_values_become_placeholders() {
        let text = text(&PcSetup {
            os: Os::Linux,
            user: "x; rm -rf /",
            public_key: Some("ssh-ed25519 AAAA' ; reboot"),
        });
        assert!(text.contains("~<user>/.ssh"), "{text}");
        assert!(text.contains("echo '<public key>'"), "{text}");
        assert!(!text.contains("reboot"), "{text}");
        let text = text_for_windows("evil' ; Remove-Item C:\\");
        assert!(text.contains("-Value '<public key>'"), "{text}");
    }

    fn text_for_windows(key: &str) -> String {
        text(&PcSetup {
            os: Os::Windows,
            user: "alex",
            public_key: Some(key),
        })
    }
}

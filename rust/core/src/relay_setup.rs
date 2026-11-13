//! The commands the user runs on a relay host so that HomeLink can wake PCs through it over SSH
//! (step 3 of the wizard, section 11 of `docs/rust-architecture.md`). They are text only: the
//! program never runs them anywhere.
//!
//! What is not known is left as a placeholder in angle brackets that the user replaces: a user
//! name that cannot be written safely, the public key (it does not exist yet), the address this
//! computer has in the relay's network (the program cannot tell which one the relay sees).
//! Nothing here is secret: the only key material is the public key.
//!
//! **The RouterOS commands are not verified on a real RouterOS 7** (see the roadmap, phase 3):
//! the group policies, the `/file add ... contents=` and `ssh-keys import` forms and the
//! behaviour of `always-allow-password-login` must be checked there. This is the only place to
//! change them.

use crate::domain::RelayKind;

/// Stands for the public key when none is known yet.
pub const PLACEHOLDER_KEY: &str = "<public key>";
/// Stands for a user name that is not safe to put in a command.
pub const PLACEHOLDER_USER: &str = "<user>";
/// Stands for the address of this computer in the relay's network.
pub const PLACEHOLDER_ADDRESS: &str = "<address of this computer>";

/// The name of the RouterOS group HomeLink's user is put in.
pub const ROUTEROS_GROUP: &str = "homelink";
/// The file the public key is imported from on the router; removed after the import.
pub const ROUTEROS_KEY_FILE: &str = "homelink.pub";

/// What the commands depend on.
#[derive(Debug, Clone, Copy)]
pub struct RelaySetup<'a> {
    /// The preset.
    pub kind: RelayKind,
    /// The SSH user HomeLink logs in as.
    pub user: &'a str,
    /// The public key line, once there is one.
    pub public_key: Option<&'a str>,
    /// The address of this computer, if known (RouterOS limits the user to it).
    pub client_address: Option<&'a str>,
    /// "Run the command with sudo" (etherwake and wakeonlan); `None` while unanswered.
    pub sudo: Option<bool>,
}

fn safe_user(user: &str) -> Option<&str> {
    let ok = !user.is_empty()
        && !user.starts_with('-')
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    ok.then_some(user)
}

fn safe_address(address: &str) -> Option<&str> {
    let ok = !address.is_empty()
        && address
            .chars()
            .all(|c| c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '/'));
    ok.then_some(address)
}

/// A public key line that is safe inside quotes: one line of printable characters without
/// quotes, backslashes and `$`. Anything else is not used (the placeholder is shown).
fn safe_key(key: &str) -> Option<&str> {
    let key = key.trim();
    let ok = !key.is_empty()
        && key.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        && !key.contains(['"', '\'', '\\', '$', '`']);
    ok.then_some(key)
}

/// The command lines, in the order they are run. Empty lines and comments are not included.
pub fn commands(setup: &RelaySetup<'_>) -> Vec<String> {
    let user = setup.user.trim();
    let user = safe_user(user).unwrap_or(PLACEHOLDER_USER);
    let key = setup
        .public_key
        .and_then(safe_key)
        .unwrap_or(PLACEHOLDER_KEY);
    match setup.kind {
        RelayKind::RouterOs => routeros(
            user,
            key,
            setup
                .client_address
                .map(str::trim)
                .and_then(safe_address)
                .unwrap_or(PLACEHOLDER_ADDRESS),
        ),
        RelayKind::Etherwake => linux("etherwake", user, key, setup.sudo == Some(true)),
        RelayKind::Wakeonlan => linux("wakeonlan", user, key, setup.sudo == Some(true)),
    }
}

/// The commands as one text, a line each.
pub fn text(setup: &RelaySetup<'_>) -> String {
    commands(setup).join("\n")
}

/// A group that can only log in over SSH, read, and run `/tool wol` (policies `ssh`, `read`,
/// `test`); a user in it limited to the address of this computer; the key imported for that user
/// through a file that is removed again; password login off for users that have a key.
fn routeros(user: &str, key: &str, address: &str) -> Vec<String> {
    vec![
        format!("/user group add name={ROUTEROS_GROUP} policy=ssh,read,test comment=\"HomeLink\""),
        format!("/user add name={user} group={ROUTEROS_GROUP} address={address}"),
        format!("/file add name={ROUTEROS_KEY_FILE} contents=\"{key}\""),
        format!("/user ssh-keys import public-key-file={ROUTEROS_KEY_FILE} user={user}"),
        format!("/file remove {ROUTEROS_KEY_FILE}"),
        "/ip ssh set always-allow-password-login=no".to_owned(),
    ]
}

/// As root on the relay host: the package, the key in the user's `authorized_keys`, and with
/// `sudo` a `sudoers.d` file that lets the user run that one program without a password
/// (HomeLink runs `sudo -n`). The path of the program is whatever `command -v` finds.
fn linux(tool: &str, user: &str, key: &str, sudo: bool) -> Vec<String> {
    let mut lines = vec![
        format!("apt install {tool}"),
        format!("install -d -m 700 -o {user} ~{user}/.ssh"),
        format!("echo '{key}' >> ~{user}/.ssh/authorized_keys"),
        format!(
            "chown {user} ~{user}/.ssh/authorized_keys && chmod 600 ~{user}/.ssh/authorized_keys"
        ),
    ];
    if sudo {
        let file = format!("/etc/sudoers.d/homelink-{tool}");
        lines.push(format!(
            "echo \"{user} ALL=(root) NOPASSWD: $(command -v {tool})\" > {file}"
        ));
        lines.push(format!("chmod 440 {file} && visudo -cf {file}"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc homelink";

    fn setup(kind: RelayKind) -> RelaySetup<'static> {
        RelaySetup {
            kind,
            user: "homelink",
            public_key: Some(KEY),
            client_address: Some("10.8.0.2"),
            sudo: None,
        }
    }

    #[test]
    fn routeros_gets_a_restricted_group_and_user_and_imports_the_key() {
        assert_eq!(
            commands(&setup(RelayKind::RouterOs)),
            [
                "/user group add name=homelink policy=ssh,read,test comment=\"HomeLink\"",
                "/user add name=homelink group=homelink address=10.8.0.2",
                "/file add name=homelink.pub contents=\"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc homelink\"",
                "/user ssh-keys import public-key-file=homelink.pub user=homelink",
                "/file remove homelink.pub",
                "/ip ssh set always-allow-password-login=no",
            ]
        );
    }

    #[test]
    fn etherwake_gets_the_package_the_key_and_with_sudo_a_rule_for_that_program() {
        let mut s = setup(RelayKind::Etherwake);
        s.sudo = Some(false);
        let without = commands(&s);
        assert_eq!(
            without,
            [
                "apt install etherwake",
                "install -d -m 700 -o homelink ~homelink/.ssh",
                "echo 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc homelink' >> ~homelink/.ssh/authorized_keys",
                "chown homelink ~homelink/.ssh/authorized_keys && chmod 600 ~homelink/.ssh/authorized_keys",
            ]
        );
        s.sudo = Some(true);
        let with = commands(&s);
        assert_eq!(with[..4], without[..]);
        assert_eq!(
            with[4..],
            [
                "echo \"homelink ALL=(root) NOPASSWD: $(command -v etherwake)\" > /etc/sudoers.d/homelink-etherwake",
                "chmod 440 /etc/sudoers.d/homelink-etherwake && visudo -cf /etc/sudoers.d/homelink-etherwake",
            ]
        );
        // Unanswered is treated as no: nothing is made root-capable without an answer.
        s.sudo = None;
        assert_eq!(commands(&s), without);
    }

    #[test]
    fn wakeonlan_names_its_own_program() {
        let mut s = setup(RelayKind::Wakeonlan);
        s.sudo = Some(true);
        let text = text(&s);
        assert!(text.starts_with("apt install wakeonlan\n"), "{text}");
        assert!(text.contains("command -v wakeonlan"), "{text}");
        assert!(text.contains("/etc/sudoers.d/homelink-wakeonlan"), "{text}");
        assert!(!text.contains("etherwake"), "{text}");
    }

    #[test]
    fn what_is_not_known_or_not_safe_is_a_placeholder() {
        let mut s = setup(RelayKind::RouterOs);
        s.public_key = None;
        s.client_address = None;
        s.user = "";
        let text = text(&s);
        assert!(text.contains(PLACEHOLDER_KEY), "{text}");
        assert!(
            text.contains(&format!("address={PLACEHOLDER_ADDRESS}")),
            "{text}"
        );
        assert!(
            text.contains(&format!("name={PLACEHOLDER_USER} group")),
            "{text}"
        );
        // A value that could change the command is never put in.
        for bad in ["a b", "a;rm -rf /", "$(x)", "-oX", "a'b", "a\"b"] {
            s.user = bad;
            assert!(text_of(&s).contains(PLACEHOLDER_USER), "{bad}");
            assert!(!text_of(&s).contains(bad), "{bad}");
        }
        s.user = "alex";
        s.client_address = Some("10.0.0.1; /system reset-configuration");
        assert!(text_of(&s).contains(PLACEHOLDER_ADDRESS));
        s.client_address = Some("10.8.0.2");
        for bad in ["a\"b; /system reset", "x'y", "k\\n", "$(id)", "a`b`"] {
            s.public_key = Some(bad);
            assert!(text_of(&s).contains(PLACEHOLDER_KEY), "{bad}");
        }
        s.public_key = Some("ssh-ed25519 AAAA\nssh-rsa BBBB");
        assert!(text_of(&s).contains(PLACEHOLDER_KEY));
    }

    fn text_of(s: &RelaySetup<'_>) -> String {
        text(s)
    }

    #[test]
    fn a_user_with_dots_and_dashes_is_kept() {
        let mut s = setup(RelayKind::Etherwake);
        s.user = "wol-user.1";
        assert!(text(&s).contains("-o wol-user.1 ~wol-user.1/.ssh"));
    }
}

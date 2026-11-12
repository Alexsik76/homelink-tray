//! Domain model: tunnels, relays, PCs, wake and power methods, states.
//!
//! Types here are plain validated data. Constructors (`parse`) reject malformed values, so
//! the rest of the program never re-validates them. Nothing in this module reads or writes
//! files; the configuration schema lives in [`crate::config`].

use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

/// A value that failed validation, with a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidValue(pub String);

fn invalid<T>(message: impl Into<String>) -> Result<T, InvalidValue> {
    Err(InvalidValue(message.into()))
}

/// Longest allowed id.
pub const ID_MAX_LEN: usize = 32;
/// Longest allowed hostname (RFC 1035).
const HOST_MAX_LEN: usize = 253;
const HOST_LABEL_MAX_LEN: usize = 63;
/// WireGuard limits tunnel names to this length.
const WG_NAME_MAX_LEN: usize = 32;

fn is_slug(s: &str) -> bool {
    (1..=ID_MAX_LEN).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident, $what:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Validates `s` as an id: 1-32 characters from `a-z`, `0-9` and `-`.
            pub fn parse(s: &str) -> Result<Self, InvalidValue> {
                if is_slug(s) {
                    Ok(Self(s.to_owned()))
                } else {
                    invalid(format!(
                        concat!($what, " id must be 1-{} characters from a-z, 0-9 and '-', got '{}'"),
                        ID_MAX_LEN, s
                    ))
                }
            }

            /// The id as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(
    /// Identifier of a [`Tunnel`].
    TunnelId,
    "tunnel"
);
id_type!(
    /// Identifier of a [`Pc`].
    PcId,
    "PC"
);
id_type!(
    /// Identifier of a [`Relay`].
    RelayId,
    "relay"
);

/// A MAC address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacAddr([u8; 6]);

impl MacAddr {
    /// Parses six hex pairs separated by `:` or `-` (one kind of separator throughout).
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        let sep = if s.contains(':') { ':' } else { '-' };
        let parts: Vec<&str> = s.split(sep).collect();
        let bad = || {
            InvalidValue(format!(
                "'{s}' is not a MAC address (expected six hex pairs, e.g. AA:BB:CC:DD:EE:FF)"
            ))
        };
        if parts.len() != 6 {
            return Err(bad());
        }
        let mut octets = [0u8; 6];
        for (slot, part) in octets.iter_mut().zip(parts) {
            if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad());
            }
            *slot = u8::from_str_radix(part, 16).map_err(|_| bad())?;
        }
        Ok(Self(octets))
    }

    /// The six bytes of the address.
    pub fn octets(&self) -> [u8; 6] {
        self.0
    }
}

impl fmt::Display for MacAddr {
    /// Upper-case, colon separated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{g:02X}")
    }
}

/// An IP address or a hostname.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Host(String);

impl Host {
    /// Accepts an IP address or an RFC 1035 hostname. A name whose last label is all digits
    /// (such as `999.1.1.1`) is rejected: it is a malformed address, not a name.
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        if s.parse::<IpAddr>().is_ok() {
            return Ok(Self(s.to_owned()));
        }
        let name = s.strip_suffix('.').unwrap_or(s);
        let bad = |why: &str| invalid(format!("'{s}' is not a valid host: {why}"));
        if name.is_empty() {
            return bad("it is empty");
        }
        if name.len() > HOST_MAX_LEN {
            return bad("it is too long");
        }
        let labels: Vec<&str> = name.split('.').collect();
        for label in &labels {
            if label.is_empty() || label.len() > HOST_LABEL_MAX_LEN {
                return bad("labels must be 1-63 characters");
            }
            if !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return bad("only letters, digits and '-' are allowed");
            }
            if label.starts_with('-') || label.ends_with('-') {
                return bad("a label may not start or end with '-'");
            }
        }
        if labels
            .last()
            .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
        {
            return bad("it looks like a malformed IP address");
        }
        Ok(Self(name.to_owned()))
    }

    /// The host as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Longest network interface name.
const INTERFACE_MAX_LEN: usize = 64;

/// The name of a network interface on a relay host, restricted to letters, digits, `_`, `.`
/// and `-`, and not starting with `-`.
///
/// The name is placed into a command that is run on the relay host over SSH, so anything
/// that could add syntax or an option (spaces, quotes, `;`, `$`, `[`, `]`, `\`, `=`, a
/// leading `-`) is not allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceName(String);

impl InterfaceName {
    /// Validates an interface name.
    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        let ok = (1..=INTERFACE_MAX_LEN).contains(&s.len())
            && !s.starts_with('-')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            invalid(format!(
                "interface name '{s}' must be 1-{INTERFACE_MAX_LEN} characters from A-Z, a-z, 0-9 and _.-, not starting with '-'"
            ))
        }
    }

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InterfaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A program and its arguments. Never a shell string, so nothing needs quoting.
///
/// Elements may contain `{name}` placeholders from a set the caller allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argv(Vec<String>);

impl Argv {
    /// Validates the list: not empty, a non-blank program, and only `allowed` placeholders.
    pub fn parse(items: Vec<String>, allowed: &[&str]) -> Result<Self, InvalidValue> {
        match items.first() {
            None => return invalid("the command must contain at least one element"),
            Some(program) if program.trim().is_empty() => {
                return invalid("the first element (the program) must not be empty");
            }
            Some(_) => {}
        }
        for item in &items {
            check_placeholders(item, allowed)?;
        }
        Ok(Self(items))
    }

    /// The elements, program first.
    pub fn items(&self) -> &[String] {
        &self.0
    }

    /// Replaces `{host}` and `{mac}` in every element.
    pub fn expand(&self, host: &Host, mac: Option<&MacAddr>) -> Vec<String> {
        let mac = mac.map(ToString::to_string);
        self.0
            .iter()
            .map(|item| {
                let item = item.replace("{host}", host.as_str());
                match &mac {
                    Some(mac) => item.replace("{mac}", mac),
                    None => item,
                }
            })
            .collect()
    }
}

fn check_placeholders(item: &str, allowed: &[&str]) -> Result<(), InvalidValue> {
    let mut rest = item;
    while let Some(pos) = rest.find(['{', '}']) {
        let tail = rest.get(pos..).unwrap_or_default();
        if tail.starts_with('}') {
            return invalid(format!("unbalanced '}}' in '{item}'"));
        }
        let Some(end) = tail.find('}') else {
            return invalid(format!("unbalanced '{{' in '{item}'"));
        };
        let name = tail.get(1..end).unwrap_or_default();
        if !allowed.contains(&name) {
            let list = if allowed.is_empty() {
                "none are available here".to_owned()
            } else {
                allowed
                    .iter()
                    .map(|a| format!("{{{a}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return invalid(format!(
                "unknown placeholder '{{{name}}}' in '{item}' (allowed: {list})"
            ));
        }
        rest = tail.get(end + 1..).unwrap_or_default();
    }
    Ok(())
}

/// Operating system of a remote PC; selects the power command presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Windows.
    Windows,
    /// Linux.
    Linux,
}

/// Intervals and thresholds. Every value comes from the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// Interval between state checks.
    pub poll: Duration,
    /// The same interval while the PC is waking.
    pub wake_poll: Duration,
    /// Timeout of one TCP connection attempt.
    pub port_timeout: Duration,
    /// Failed checks in a row before an online PC turns off.
    pub offline_after_failures: u32,
    /// How long to wait for the PC after a wake request.
    pub wake_timeout: Duration,
    /// How long to wait for a power command to finish.
    pub power_timeout: Duration,
}

/// How a tunnel is brought up.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TunnelKind {
    /// A tunnel run as a service of the official WireGuard client.
    WireGuardService {
        /// Path to `wireguard.exe`.
        exe: String,
        /// Path to the imported tunnel configuration (`.conf` or `.conf.dpapi`).
        conf: String,
    },
}

impl TunnelKind {
    /// The tunnel name as WireGuard derives it from the file name.
    pub fn wireguard_name(conf: &str) -> Result<String, InvalidValue> {
        let file = conf.rsplit(['/', '\\']).next().unwrap_or_default();
        let lower = file.to_ascii_lowercase();
        let stem_len = [".conf.dpapi", ".conf"]
            .iter()
            .find(|suffix| lower.ends_with(**suffix))
            .map(|suffix| file.len() - suffix.len());
        let Some(stem_len) = stem_len else {
            return invalid(format!(
                "'{conf}' must be a file ending in .conf or .conf.dpapi"
            ));
        };
        let name = file.get(..stem_len).unwrap_or_default();
        let ok = (1..=WG_NAME_MAX_LEN).contains(&name.len())
            && name.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'_' | b'=' | b'+' | b'.' | b'-')
            });
        if ok {
            Ok(name.to_owned())
        } else {
            invalid(format!(
                "tunnel name '{name}' must be 1-{WG_NAME_MAX_LEN} characters from A-Z, a-z, 0-9 and _=+.-"
            ))
        }
    }
}

/// A tunnel that gives access to a remote network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tunnel {
    /// Identifier.
    pub id: TunnelId,
    /// Display name.
    pub name: String,
    /// How the tunnel is controlled.
    pub kind: TunnelKind,
    /// A host that answers ping only while the tunnel works.
    pub probe_host: Host,
    /// How long to wait for `probe_host` after the start.
    pub timeout: Duration,
    /// Timeout of one ping.
    pub ping_timeout: Duration,
}

/// The wake command preset of a relay host: which tool the host runs to send the magic
/// packet. A relay is any always-on device in the PC's network (a router, an OpenWrt box, a
/// Raspberry Pi). To add a preset, add a variant here and an arm to each `match` below and in
/// `wake::relay_command`; the compiler points at every place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RelayKind {
    /// MikroTik RouterOS: `/tool wol mac=<mac> interface=<interface>`.
    RouterOs,
    /// `etherwake -i <interface> <mac>` (typical for OpenWrt).
    Etherwake,
    /// `wakeonlan [-i <address>] <mac>`.
    Wakeonlan,
}

impl RelayKind {
    /// Every preset.
    pub const ALL: [RelayKind; 3] = [Self::RouterOs, Self::Etherwake, Self::Wakeonlan];

    /// The name used in the configuration file.
    pub fn code(self) -> &'static str {
        match self {
            Self::RouterOs => "routeros",
            Self::Etherwake => "etherwake",
            Self::Wakeonlan => "wakeonlan",
        }
    }

    /// Whether the preset sends the packet on a named interface; if so, the wake method must
    /// give one.
    pub fn uses_interface(self) -> bool {
        match self {
            Self::RouterOs | Self::Etherwake => true,
            Self::Wakeonlan => false,
        }
    }

    /// Whether the relay entry must say if the command is run with `sudo` (the tool may need
    /// root). RouterOS has no `sudo`, so there the key is rejected.
    pub fn supports_sudo(self) -> bool {
        match self {
            Self::Etherwake | Self::Wakeonlan => true,
            Self::RouterOs => false,
        }
    }

    /// Whether the preset can take a destination address (optional); if not, the wake method
    /// must not give one.
    pub fn uses_address(self) -> bool {
        match self {
            Self::Wakeonlan => true,
            Self::RouterOs | Self::Etherwake => false,
        }
    }
}

/// A relay host shared by the wake methods of several PCs: a router or any always-on device
/// in the PC's network that sends the magic packet on its behalf. It is reached over SSH with
/// a key, through the system OpenSSH client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relay {
    /// Identifier.
    pub id: RelayId,
    /// The wake command preset.
    pub kind: RelayKind,
    /// Address.
    pub host: Host,
    /// SSH port.
    pub port: u16,
    /// SSH user.
    pub user: String,
    /// Absolute path of the private key file. Explicit on purpose: the program runs
    /// elevated and must not depend on which profile `ssh` resolves.
    pub identity_file: String,
    /// Timeout of one command, including the connection.
    pub timeout: Duration,
    /// Whether the wake command is run as `sudo -n <command>` (the tool needs root and the
    /// user may run it without a password). Always `false` for presets that do not support it
    /// ([`RelayKind::supports_sudo`]).
    pub sudo: bool,
}

/// How a PC is woken.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WakeMethod {
    /// A relay host sends the magic packet; it is told to over SSH.
    Relay {
        /// The relay.
        relay: RelayId,
        /// MAC address of the PC.
        mac: MacAddr,
        /// Interface of the relay that faces the PC; given exactly when the relay's preset
        /// uses one.
        interface: Option<InterfaceName>,
        /// Destination address, for presets that take one.
        address: Option<Ipv4Addr>,
    },
    /// This computer sends the magic packet over UDP to `address`: a broadcast address, or,
    /// for a routed network, the PC's own IP (with a static ARP entry on the last router).
    MagicPacket {
        /// MAC address of the PC.
        mac: MacAddr,
        /// Destination address (any IPv4 address, unicast or broadcast).
        address: Ipv4Addr,
        /// UDP port.
        port: u16,
    },
    /// An arbitrary command.
    Command {
        /// The command; `{host}` and, when `mac` is set, `{mac}` are substituted.
        argv: Argv,
        /// MAC address, available to the command as `{mac}`.
        mac: Option<MacAddr>,
        /// How long to wait for the command.
        timeout: Duration,
    },
}

/// The kind of a [`WakeMethod`], used to pick the sender that implements it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WakeKind {
    /// [`WakeMethod::Relay`].
    Relay,
    /// [`WakeMethod::MagicPacket`].
    MagicPacket,
    /// [`WakeMethod::Command`].
    Command,
}

impl WakeMethod {
    /// The kind of this method.
    pub fn kind(&self) -> WakeKind {
        match self {
            Self::Relay { .. } => WakeKind::Relay,
            Self::MagicPacket { .. } => WakeKind::MagicPacket,
            Self::Command { .. } => WakeKind::Command,
        }
    }
}

/// An action that changes the power state of a PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    /// Suspend.
    Sleep,
    /// Power off.
    Shutdown,
}

/// How sleep and shutdown commands reach a PC.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PowerMethod {
    /// Through the system `ssh` client with key authentication.
    Ssh {
        /// Remote user.
        user: String,
        /// Remote port.
        port: u16,
        /// Absolute path of the private key file. Explicit on purpose: the program runs
        /// elevated and must not depend on which profile `ssh` resolves.
        identity_file: String,
    },
    /// Arbitrary commands run on this computer; an action without a command is unavailable.
    Command {
        /// Sleep command.
        sleep: Option<Argv>,
        /// Shutdown command.
        shutdown: Option<Argv>,
    },
}

/// Remote Desktop settings of a PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rdp {
    /// User name written into the RDP file.
    pub user: String,
    /// Extra lines of the RDP file.
    pub options: Vec<String>,
}

/// A remote PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pc {
    /// Identifier.
    pub id: PcId,
    /// Display name.
    pub name: String,
    /// Operating system.
    pub os: Os,
    /// Address.
    pub host: Host,
    /// The tunnel this PC is reached through; `None` for a PC on the local network.
    pub tunnel: Option<TunnelId>,
    /// TCP ports that must all accept a connection for the PC to count as online.
    pub ports: Vec<u16>,
    /// Wake method, if any.
    pub wake: Option<WakeMethod>,
    /// Power method, if any.
    pub power: Option<PowerMethod>,
    /// Remote Desktop settings, if any.
    pub rdp: Option<Rdp>,
    /// Intervals and thresholds, defaults merged with this PC's overrides.
    pub timing: Timing,
}

/// State of a tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    /// Not running.
    Down,
    /// Running but not yet reachable.
    Starting,
    /// Running and reachable.
    Up,
}

impl TunnelState {
    /// Suffix of the locale key `tunnel.<name>`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Starting => "starting",
            Self::Up => "up",
        }
    }
}

/// State of a PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcState {
    /// Not known (tunnel down or not yet checked).
    Unknown,
    /// Not answering.
    Offline,
    /// A wake request was sent, waiting for an answer.
    Waking,
    /// All ports answer.
    Online,
}

impl PcState {
    /// Suffix of the locale key `pc.<name>`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Offline => "offline",
            Self::Waking => "waking",
            Self::Online => "online",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_accept_slugs_only() {
        for ok in ["a", "home", "pc-1", &"x".repeat(32)] {
            assert!(TunnelId::parse(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Home", "a_b", "a b", "дім", &"x".repeat(33)] {
            assert!(PcId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn mac_forms() {
        let mac = MacAddr::parse("aa:bb:cc:dd:ee:0f").unwrap();
        assert_eq!(mac.to_string(), "AA:BB:CC:DD:EE:0F");
        assert_eq!(MacAddr::parse("AA-BB-CC-DD-EE-0F").unwrap(), mac);
        for bad in [
            "",
            "AA:BB:CC:DD:EE",
            "AA:BB:CC:DD:EE:0F:00",
            "AA:BB:CC:DD:EE:GG",
            "AABBCCDDEE0F",
            "AA:BB-CC:DD:EE:0F",
            "A:BB:CC:DD:EE:0F",
        ] {
            assert!(MacAddr::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn hosts() {
        for ok in [
            "192.168.88.10",
            "::1",
            "pc",
            "my-pc.local",
            "a.b.c",
            "pc.local.",
        ] {
            assert!(Host::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            ".",
            "a..b",
            "-a",
            "a-",
            "a b",
            "999.1.1.1",
            "1.2.3",
            "a_b",
            "пк",
        ] {
            assert!(Host::parse(bad).is_err(), "{bad}");
        }
        assert!(Host::parse(&"a".repeat(64)).is_err());
    }

    #[test]
    fn interface_names_cannot_carry_command_syntax() {
        for ok in ["bridge", "ether1", "vlan-10", "br_lan.2", &"a".repeat(64)] {
            assert!(InterfaceName::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "a b",
            "a;b",
            "a\"b",
            "a'b",
            "a$b",
            "a[b]",
            "a\\b",
            "a=b",
            "-h",
            "-i",
            "a&b",
            "a|b",
            "a`b`",
            "a\nb",
            "a\tb",
            "мережа",
            &"a".repeat(65),
        ] {
            assert!(InterfaceName::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn argv_rules() {
        let v = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(Argv::parse(v(&[]), &[]).is_err());
        assert!(Argv::parse(v(&[""]), &[]).is_err());
        assert!(Argv::parse(v(&["ssh", "-o", "BatchMode=yes"]), &[]).is_ok());
        assert!(Argv::parse(v(&["wake", "{mac}"]), &["mac"]).is_ok());
        assert!(Argv::parse(v(&["wake", "{mac}"]), &[]).is_err());
        assert!(Argv::parse(v(&["wake", "{nope}"]), &["mac"]).is_err());
        assert!(Argv::parse(v(&["wake", "{mac"]), &["mac"]).is_err());
        assert!(Argv::parse(v(&["wake", "mac}"]), &["mac"]).is_err());
    }

    #[test]
    fn argv_expands_placeholders() {
        let argv =
            Argv::parse(vec!["wake".into(), "{mac}@{host}".into()], &["mac", "host"]).unwrap();
        let host = Host::parse("10.0.0.5").unwrap();
        let mac = MacAddr::parse("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(
            argv.expand(&host, Some(&mac)),
            ["wake", "AA:BB:CC:DD:EE:FF@10.0.0.5"]
        );
    }

    #[test]
    fn wireguard_names() {
        assert_eq!(
            TunnelKind::wireguard_name(r"C:\WG\Data\Configurations\home.conf.dpapi").unwrap(),
            "home"
        );
        assert_eq!(TunnelKind::wireguard_name("/x/Home.CONF").unwrap(), "Home");
        for bad in [
            "home.txt",
            "home",
            ".conf",
            "a b.conf",
            &format!("{}.conf", "x".repeat(33)),
        ] {
            assert!(TunnelKind::wireguard_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn state_names_match_locale_keys() {
        assert_eq!(TunnelState::Starting.as_str(), "starting");
        assert_eq!(PcState::Waking.as_str(), "waking");
    }
}

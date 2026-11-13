//! The add-PC wizard (section 8 of `docs/design/ui-handoff.md`): five steps over one
//! [`PcDraft`]. The UI binds the fields, asks [`PcDraft::validate_step`] when the user presses
//! "Далі", shows each [`Problem`] under the field named in [`field`], and on the last step
//! calls [`PcDraft::build`] and then `settings::save_pc`. "Змінити" on a PC opens the same
//! wizard filled by [`PcDraft::from_pc`].
//!
//! | Step | Fields |
//! | --- | --- |
//! | 1 [`Step::Address`] | `name`, `host`, `ports` |
//! | 2 [`Step::Tunnel`] | `tunnel` (an existing tunnel or none) |
//! | 3 [`Step::Wake`] | `wake`, then by method: `mac`, `magic_address`, `magic_port`; or `relay`, the new relay (`relay.*`), `interface`, `relay_address`; or `wake_command`, `wake_timeout` |
//! | 4 [`Step::Power`] | `allow_sleep`, `allow_shutdown`, `allow_rdp`, then `power`, `os`, `ssh_*`, `sleep_command`, `shutdown_command`, `rdp_user` |
//! | 5 [`Step::Checks`] | no fields; see [`crate::checks`] |
//!
//! Every field is validated with the rules of the configuration file (`crate::rules`, the
//! domain parsers), so the wizard accepts a value if and only if the file would. Ids are never
//! asked for: a new PC gets a slug of its name, unique among PCs; an edited PC keeps its id
//! unless its name changed.
//!
//! The wizard cannot express one thing the file can: over SSH sleep and shutdown always come
//! together ([`PowerMethod::Ssh`]), so with SSH both boxes must be ticked
//! ([`Code::PowerSshNeedsBoth`]); with commands each can be on its own.

use crate::config::Config;
use crate::domain::{
    Argv, Os, Pc, PcId, PowerMethod, Rdp, Relay, RelayId, RelayKind, Timing, TunnelId, WakeMethod,
};
use crate::forms;
use crate::ids;
use crate::problem::{Code, Problem};
use crate::rules;

/// Names of the fields in [`Problem::field`].
pub mod field {
    /// Step 1: display name.
    pub const NAME: &str = "name";
    /// Step 1: IP address or host name.
    pub const HOST: &str = "host";
    /// Step 1: readiness ports, comma separated.
    pub const PORTS: &str = "ports";
    /// Step 2: the chosen tunnel.
    pub const TUNNEL: &str = "tunnel";
    /// Step 3: MAC address (all wake methods; optional for a command).
    pub const MAC: &str = "mac";
    /// Step 3, direct magic packet: destination address.
    pub const MAGIC_ADDRESS: &str = "magic_address";
    /// Step 3, direct magic packet: UDP port.
    pub const MAGIC_PORT: &str = "magic_port";
    /// Step 3, relay: the chosen existing relay.
    pub const RELAY: &str = "relay";
    /// Step 3, new relay: host.
    pub const RELAY_HOST: &str = "relay.host";
    /// Step 3, new relay: SSH port.
    pub const RELAY_PORT: &str = "relay.port";
    /// Step 3, new relay: SSH user.
    pub const RELAY_USER: &str = "relay.user";
    /// Step 3, new relay: key file.
    pub const RELAY_KEY: &str = "relay.identity_file";
    /// Step 3, new relay: the sudo question.
    pub const RELAY_SUDO: &str = "relay.sudo";
    /// Step 3, new relay: timeout in seconds.
    pub const RELAY_TIMEOUT: &str = "relay.timeout";
    /// Step 3, relay: interface (RouterOS and etherwake).
    pub const INTERFACE: &str = "interface";
    /// Step 3, relay: destination address (wakeonlan, optional).
    pub const RELAY_ADDRESS: &str = "relay_address";
    /// Step 3, command: the command line.
    pub const WAKE_COMMAND: &str = "wake_command";
    /// Step 3, command: timeout in seconds.
    pub const WAKE_TIMEOUT: &str = "wake_timeout";
    /// Step 4: "Дозволити сон".
    pub const ALLOW_SLEEP: &str = "allow_sleep";
    /// Step 4: "Дозволити вимкнення".
    pub const ALLOW_SHUTDOWN: &str = "allow_shutdown";
    /// Step 4, SSH: user.
    pub const SSH_USER: &str = "ssh_user";
    /// Step 4, SSH: port.
    pub const SSH_PORT: &str = "ssh_port";
    /// Step 4, SSH: key file.
    pub const SSH_KEY: &str = "ssh_identity_file";
    /// Step 4, commands: sleep command line.
    pub const SLEEP_COMMAND: &str = "sleep_command";
    /// Step 4, commands: shutdown command line.
    pub const SHUTDOWN_COMMAND: &str = "shutdown_command";
    /// Step 4, RDP: user name.
    pub const RDP_USER: &str = "rdp_user";
}

/// Number of steps.
pub const STEP_COUNT: usize = 5;

/// A step of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// 1. Name, address, readiness ports.
    Address,
    /// 2. Tunnel.
    Tunnel,
    /// 3. Wake method.
    Wake,
    /// 4. Sleep, shutdown and RDP.
    Power,
    /// 5. Checks.
    Checks,
}

impl Step {
    /// All steps in order.
    pub const ALL: [Step; STEP_COUNT] = [
        Self::Address,
        Self::Tunnel,
        Self::Wake,
        Self::Power,
        Self::Checks,
    ];

    /// The number shown in "Крок N з 5", from 1.
    pub fn number(self) -> usize {
        match self {
            Self::Address => 1,
            Self::Tunnel => 2,
            Self::Wake => 3,
            Self::Power => 4,
            Self::Checks => 5,
        }
    }

    /// The locale key of the step name.
    pub fn name_key(self) -> &'static str {
        match self {
            Self::Address => "wizard.step.address",
            Self::Tunnel => "wizard.step.tunnel",
            Self::Wake => "wizard.step.wake",
            Self::Power => "wizard.step.power",
            Self::Checks => "wizard.step.checks",
        }
    }

    /// The step after this one; `None` after the last.
    pub fn next(self) -> Option<Step> {
        Self::ALL.get(self.number()).copied()
    }

    /// The step before this one; `None` before the first.
    pub fn previous(self) -> Option<Step> {
        self.number()
            .checked_sub(2)
            .and_then(|i| Self::ALL.get(i))
            .copied()
    }

    /// The step that shows a field, to jump back to it when the final check fails.
    pub fn of_field(name: &str) -> Option<Step> {
        Some(match name {
            field::NAME | field::HOST | field::PORTS => Self::Address,
            field::TUNNEL => Self::Tunnel,
            field::MAC
            | field::MAGIC_ADDRESS
            | field::MAGIC_PORT
            | field::RELAY
            | field::RELAY_HOST
            | field::RELAY_PORT
            | field::RELAY_USER
            | field::RELAY_KEY
            | field::RELAY_SUDO
            | field::RELAY_TIMEOUT
            | field::INTERFACE
            | field::RELAY_ADDRESS
            | field::WAKE_COMMAND
            | field::WAKE_TIMEOUT => Self::Wake,
            field::ALLOW_SLEEP
            | field::ALLOW_SHUTDOWN
            | field::SSH_USER
            | field::SSH_PORT
            | field::SSH_KEY
            | field::SLEEP_COMMAND
            | field::SHUTDOWN_COMMAND
            | field::RDP_USER => Self::Power,
            _ => return None,
        })
    }
}

/// How the PC is woken (the radio list of step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeChoice {
    /// "Не будити цей ПК".
    None,
    /// Through a relay host over SSH.
    Relay,
    /// A magic packet from this computer.
    MagicPacket,
    /// A command.
    Command,
}

/// Which relay a relay wake uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayChoice {
    /// A relay that is already in the configuration.
    Existing(RelayId),
    /// A new relay, described by [`PcDraft::new_relay`].
    New,
}

/// How sleep and shutdown reach the PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerChoice {
    /// Over SSH with the preset commands of the PC's operating system.
    Ssh,
    /// Commands run on this computer.
    Command,
}

/// A new relay as typed in step 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayDraft {
    /// The preset.
    pub kind: RelayKind,
    /// Host.
    pub host: String,
    /// SSH port.
    pub port: String,
    /// SSH user.
    pub user: String,
    /// Absolute path of the private key.
    pub identity_file: String,
    /// "Run the command with sudo": `None` until the user answers (there is no default);
    /// ignored for presets without sudo.
    pub sudo: Option<bool>,
    /// Seconds for one command, including the connection.
    pub timeout_seconds: String,
}

/// What the wizard keeps from the PC being edited, for what it does not show.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Base {
    id: PcId,
    name: String,
    timing: Timing,
    rdp_options: Vec<String>,
    wake_argv: Option<Argv>,
    sleep_argv: Option<Argv>,
    shutdown_argv: Option<Argv>,
}

/// The draft of a PC: every field of the wizard as typed or chosen.
///
/// Fields of a method that is not chosen keep their values and are ignored (so going back
/// and forth between methods does not lose what was typed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcDraft {
    /// Step 1: display name.
    pub name: String,
    /// Step 1: IP address or host name.
    pub host: String,
    /// Step 1: readiness ports, comma separated.
    pub ports: String,
    /// Step 2: the tunnel, `None` for "Без тунелю".
    pub tunnel: Option<TunnelId>,
    /// Step 3: the wake method.
    pub wake: WakeChoice,
    /// Step 3: MAC address.
    pub mac: String,
    /// Step 3, magic packet: destination address.
    pub magic_address: String,
    /// Step 3, magic packet: UDP port.
    pub magic_port: String,
    /// Step 3, relay: the relay.
    pub relay: RelayChoice,
    /// Step 3, relay: the new relay, used when [`RelayChoice::New`].
    pub new_relay: RelayDraft,
    /// Step 3, relay: interface (for presets that use one).
    pub interface: String,
    /// Step 3, relay: destination address (for presets that take one; may be empty).
    pub relay_address: String,
    /// Step 3, command: the command line.
    pub wake_command: String,
    /// Step 3, command: timeout in seconds.
    pub wake_timeout: String,
    /// Step 4: sleep is allowed.
    pub allow_sleep: bool,
    /// Step 4: shutdown is allowed.
    pub allow_shutdown: bool,
    /// Step 4: Remote Desktop is allowed.
    pub allow_rdp: bool,
    /// Step 4: how sleep and shutdown reach the PC.
    pub power: PowerChoice,
    /// Step 4, SSH: the operating system (selects the command preset).
    pub os: Os,
    /// Step 4, SSH: user.
    pub ssh_user: String,
    /// Step 4, SSH: port.
    pub ssh_port: String,
    /// Step 4, SSH: absolute path of the private key.
    pub ssh_identity_file: String,
    /// Step 4, commands: sleep command line.
    pub sleep_command: String,
    /// Step 4, commands: shutdown command line.
    pub shutdown_command: String,
    /// Step 4, RDP: user name.
    pub rdp_user: String,
    base: Option<Base>,
}

/// The result of a valid draft: the PC and the relay it needs that does not exist yet.
#[derive(Debug, Clone, PartialEq)]
pub struct PcOutcome {
    /// The PC.
    pub pc: Pc,
    /// A relay to add with it (when the wake method uses a new relay).
    pub new_relay: Option<Relay>,
    /// The PC being replaced when the draft came from [`PcDraft::from_pc`].
    pub replaces: Option<PcId>,
}

/// A choice of step 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelOption {
    /// The tunnel; `None` is "Без тунелю", which comes last.
    pub id: Option<TunnelId>,
    /// The tunnel's name; empty for "Без тунелю" (use the locale key `wizard.tunnel.none`).
    pub name: String,
}

/// The radio list of step 2: every tunnel in configuration order, then "Без тунелю".
pub fn tunnel_options(config: &Config) -> Vec<TunnelOption> {
    config
        .tunnels
        .iter()
        .map(|t| TunnelOption {
            id: Some(t.id.clone()),
            name: t.name.clone(),
        })
        .chain(std::iter::once(TunnelOption {
            id: None,
            name: String::new(),
        }))
        .collect()
}

/// A relay that step 3 can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayOption {
    /// The relay.
    pub id: RelayId,
    /// Its host.
    pub host: String,
    /// Its preset.
    pub kind: RelayKind,
}

/// The existing relays, in configuration order.
pub fn relay_options(config: &Config) -> Vec<RelayOption> {
    config
        .relays
        .iter()
        .map(|r| RelayOption {
            id: r.id.clone(),
            host: r.host.to_string(),
            kind: r.kind,
        })
        .collect()
}

fn blank_relay(config: &Config) -> RelayDraft {
    let timeout = config
        .relays
        .first()
        .map_or(config.defaults.power_timeout, |r| r.timeout);
    RelayDraft {
        kind: RelayKind::RouterOs,
        host: String::new(),
        port: "22".into(),
        user: String::new(),
        identity_file: String::new(),
        sudo: None,
        timeout_seconds: timeout.as_secs().to_string(),
    }
}

impl PcDraft {
    /// An empty draft for a new PC. Prefills that are not user settings: SSH port 22, magic
    /// packet port 9 and broadcast address; timeouts come from the configuration (an existing
    /// relay's, else `[defaults].power_timeout`).
    pub fn new(config: &Config) -> Self {
        let timeout = config.defaults.power_timeout.as_secs().to_string();
        Self {
            name: String::new(),
            host: String::new(),
            ports: String::new(),
            tunnel: None,
            wake: WakeChoice::None,
            mac: String::new(),
            magic_address: "255.255.255.255".into(),
            magic_port: "9".into(),
            relay: config
                .relays
                .first()
                .map_or(RelayChoice::New, |r| RelayChoice::Existing(r.id.clone())),
            new_relay: blank_relay(config),
            interface: String::new(),
            relay_address: String::new(),
            wake_command: String::new(),
            wake_timeout: timeout,
            allow_sleep: false,
            allow_shutdown: false,
            allow_rdp: false,
            power: PowerChoice::Ssh,
            os: Os::Windows,
            ssh_user: String::new(),
            ssh_port: "22".into(),
            ssh_identity_file: String::new(),
            sleep_command: String::new(),
            shutdown_command: String::new(),
            rdp_user: String::new(),
            base: None,
        }
    }

    /// A draft filled from an existing PC ("Змінити"). What the wizard does not show (timing
    /// overrides, extra RDP options, command lists that cannot be typed back) is kept and
    /// written back unchanged by [`PcDraft::build`].
    pub fn from_pc(config: &Config, pc: &Pc) -> Self {
        let mut draft = Self::new(config);
        draft.name = pc.name.clone();
        draft.host = pc.host.to_string();
        draft.ports = pc
            .ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        draft.tunnel = pc.tunnel.clone();
        draft.os = pc.os;
        let mut base = Base {
            id: pc.id.clone(),
            name: pc.name.clone(),
            timing: pc.timing,
            rdp_options: pc
                .rdp
                .as_ref()
                .map(|r| r.options.clone())
                .unwrap_or_default(),
            wake_argv: None,
            sleep_argv: None,
            shutdown_argv: None,
        };
        match &pc.wake {
            None => {}
            Some(WakeMethod::MagicPacket { mac, address, port }) => {
                draft.wake = WakeChoice::MagicPacket;
                draft.mac = mac.to_string();
                draft.magic_address = address.to_string();
                draft.magic_port = port.to_string();
            }
            Some(WakeMethod::Relay {
                relay,
                mac,
                interface,
                address,
            }) => {
                draft.wake = WakeChoice::Relay;
                draft.relay = RelayChoice::Existing(relay.clone());
                draft.mac = mac.to_string();
                draft.interface = interface
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                draft.relay_address = address.map(|a| a.to_string()).unwrap_or_default();
            }
            Some(WakeMethod::Command { argv, mac, timeout }) => {
                draft.wake = WakeChoice::Command;
                draft.wake_command = forms::join_command(argv.items());
                draft.mac = mac.as_ref().map(ToString::to_string).unwrap_or_default();
                draft.wake_timeout = timeout.as_secs().to_string();
                base.wake_argv = Some(argv.clone());
            }
        }
        match &pc.power {
            None => {}
            Some(PowerMethod::Ssh {
                user,
                port,
                identity_file,
            }) => {
                draft.allow_sleep = true;
                draft.allow_shutdown = true;
                draft.power = PowerChoice::Ssh;
                draft.ssh_user = user.clone();
                draft.ssh_port = port.to_string();
                draft.ssh_identity_file = identity_file.clone();
            }
            Some(PowerMethod::Command { sleep, shutdown }) => {
                draft.allow_sleep = sleep.is_some();
                draft.allow_shutdown = shutdown.is_some();
                draft.power = PowerChoice::Command;
                if let Some(argv) = sleep {
                    draft.sleep_command = forms::join_command(argv.items());
                    base.sleep_argv = Some(argv.clone());
                }
                if let Some(argv) = shutdown {
                    draft.shutdown_command = forms::join_command(argv.items());
                    base.shutdown_argv = Some(argv.clone());
                }
            }
        }
        if let Some(rdp) = &pc.rdp {
            draft.allow_rdp = true;
            draft.rdp_user = rdp.user.clone();
        }
        draft.base = Some(base);
        draft
    }

    /// The id of the PC being edited; `None` for a new one.
    pub fn editing(&self) -> Option<&PcId> {
        self.base.as_ref().map(|b| &b.id)
    }

    /// The preset of the relay step 3 would use: the chosen existing relay's or the new
    /// relay's; `None` when the wake method is not a relay or the chosen relay is unknown.
    /// The UI uses it to show the fields the preset needs (interface, address, sudo).
    pub fn relay_kind(&self, config: &Config) -> Option<RelayKind> {
        if self.wake != WakeChoice::Relay {
            return None;
        }
        match &self.relay {
            RelayChoice::Existing(id) => config.relay(id).map(|r| r.kind),
            RelayChoice::New => Some(self.new_relay.kind),
        }
    }

    /// Validates the fields of one step. Step 5 has no fields of its own and always passes.
    /// Returns every problem of the step, each for one field.
    pub fn validate_step(&self, step: Step, config: &Config) -> Vec<Problem> {
        let mut out = Vec::new();
        match step {
            Step::Address => {
                self.address_part(config, &mut out);
            }
            Step::Tunnel => {
                self.tunnel_part(config, &mut out);
            }
            Step::Wake => {
                self.wake_part(config, &mut out);
            }
            Step::Power => {
                self.power_part(&mut out);
            }
            Step::Checks => {}
        }
        out
    }

    /// Validates every step and builds the PC. On failure all problems of all steps are
    /// returned (see [`Step::of_field`] to find the step of each).
    pub fn build(&self, config: &Config) -> Result<PcOutcome, Vec<Problem>> {
        let mut out = Vec::new();
        let address = self.address_part(config, &mut out);
        let tunnel = self.tunnel_part(config, &mut out);
        let wake = self.wake_part(config, &mut out);
        let power = self.power_part(&mut out);
        if !out.is_empty() {
            return Err(out);
        }
        let (Some((name, host, ports)), Some(tunnel), Some((wake, new_relay)), Some((power, rdp))) =
            (address, tunnel, wake, power)
        else {
            return Err(out);
        };
        let own = self.editing();
        let id = match &self.base {
            Some(base) if base.name.trim() == name => base.id.clone(),
            _ => {
                let slug = ids::unique(&name, "pc", |candidate| {
                    config
                        .pcs
                        .iter()
                        .any(|p| p.id.as_str() == candidate && Some(&p.id) != own)
                });
                match PcId::parse(&slug) {
                    Ok(id) => id,
                    Err(_) => return Err(vec![Problem::new(field::NAME, Code::Required)]),
                }
            }
        };
        let timing = self.base.as_ref().map_or(config.defaults, |b| b.timing);
        Ok(PcOutcome {
            pc: Pc {
                id,
                name,
                os: self.os,
                host,
                tunnel,
                ports,
                wake,
                power,
                rdp,
                timing,
            },
            new_relay,
            replaces: own.cloned(),
        })
    }

    // ---- step 1 ----

    fn address_part(
        &self,
        config: &Config,
        out: &mut Vec<Problem>,
    ) -> Option<(String, crate::domain::Host, Vec<u16>)> {
        let name = forms::required(field::NAME, &self.name, out);
        if let Some(name) = name {
            let own = self.editing();
            let taken = config
                .pcs
                .iter()
                .any(|p| Some(&p.id) != own && p.name.trim().to_lowercase() == name.to_lowercase());
            if taken {
                out.push(Problem::new(field::NAME, Code::NameTaken).with("name", name));
            }
        }
        let host = forms::host(field::HOST, &self.host, out);
        let ports = forms::ports(field::PORTS, &self.ports, out);
        let name_ok = !out.iter().any(|p| p.field == field::NAME);
        Some((name.filter(|_| name_ok)?.to_owned(), host?, ports?))
    }

    // ---- step 2 ----

    fn tunnel_part(&self, config: &Config, out: &mut Vec<Problem>) -> Option<Option<TunnelId>> {
        match &self.tunnel {
            None => Some(None),
            Some(id) if config.tunnel(id).is_some() => Some(Some(id.clone())),
            Some(_) => {
                out.push(Problem::new(field::TUNNEL, Code::TunnelUnknown));
                None
            }
        }
    }

    // ---- step 3 ----

    fn wake_part(
        &self,
        config: &Config,
        out: &mut Vec<Problem>,
    ) -> Option<(Option<WakeMethod>, Option<Relay>)> {
        match self.wake {
            WakeChoice::None => Some((None, None)),
            WakeChoice::MagicPacket => {
                let mac = forms::mac(field::MAC, &self.mac, out);
                let address = forms::ipv4(field::MAGIC_ADDRESS, &self.magic_address, out);
                let port = forms::port(field::MAGIC_PORT, &self.magic_port, out);
                Some((
                    Some(WakeMethod::MagicPacket {
                        mac: mac?,
                        address: address?,
                        port: port?,
                    }),
                    None,
                ))
            }
            WakeChoice::Relay => self.relay_wake_part(config, out),
            WakeChoice::Command => {
                let typed_mac = self.mac.trim();
                let mac = if typed_mac.is_empty() {
                    Some(None)
                } else {
                    forms::mac(field::MAC, typed_mac, out).map(Some)
                };
                let allowed: &[&str] = if matches!(mac, Some(Some(_))) {
                    &["host", "mac"]
                } else {
                    &["host"]
                };
                let argv = forms::command(
                    field::WAKE_COMMAND,
                    &self.wake_command,
                    allowed,
                    self.base.as_ref().and_then(|b| b.wake_argv.as_ref()),
                    out,
                );
                let timeout = forms::seconds(field::WAKE_TIMEOUT, &self.wake_timeout, out);
                Some((
                    Some(WakeMethod::Command {
                        argv: argv?,
                        mac: mac?,
                        timeout: timeout?,
                    }),
                    None,
                ))
            }
        }
    }

    fn relay_wake_part(
        &self,
        config: &Config,
        out: &mut Vec<Problem>,
    ) -> Option<(Option<WakeMethod>, Option<Relay>)> {
        // The relay: an existing one, or a new one validated here.
        let relay: Option<(RelayId, RelayKind, Option<Relay>)> = match &self.relay {
            RelayChoice::Existing(id) => match config.relay(id) {
                Some(r) => Some((r.id.clone(), r.kind, None)),
                None => {
                    out.push(Problem::new(field::RELAY, Code::RelayUnknown));
                    None
                }
            },
            RelayChoice::New => self
                .new_relay_part(config, out)
                .map(|r| (r.id.clone(), r.kind, Some(r))),
        };
        let mac = forms::mac(field::MAC, &self.mac, out);
        let kind = relay.as_ref().map(|(_, kind, _)| *kind);
        // Only the fields the preset uses are looked at; the others keep stale text.
        let mut interface = None;
        let mut address = None;
        if let Some(kind) = kind {
            let typed_interface = kind.uses_interface() && !self.interface.trim().is_empty();
            let typed_address = kind.uses_address() && !self.relay_address.trim().is_empty();
            forms::preset_fields(
                kind,
                typed_interface,
                typed_address,
                field::INTERFACE,
                field::RELAY_ADDRESS,
                out,
            );
            if typed_interface {
                interface = forms::interface(field::INTERFACE, &self.interface, out).map(Some);
            }
            if typed_address {
                address = forms::ipv4(field::RELAY_ADDRESS, &self.relay_address, out).map(Some);
            }
        }
        let (relay_id, kind, new_relay) = relay?;
        let interface = if kind.uses_interface() {
            interface?
        } else {
            None
        };
        let address = if kind.uses_address() && !self.relay_address.trim().is_empty() {
            address?
        } else {
            None
        };
        Some((
            Some(WakeMethod::Relay {
                relay: relay_id,
                mac: mac?,
                interface,
                address,
            }),
            new_relay,
        ))
    }

    fn new_relay_part(&self, config: &Config, out: &mut Vec<Problem>) -> Option<Relay> {
        let d = &self.new_relay;
        let host = forms::host(field::RELAY_HOST, &d.host, out);
        let port = forms::port(field::RELAY_PORT, &d.port, out);
        let user = forms::user(field::RELAY_USER, &d.user, out);
        let key = forms::key_file(field::RELAY_KEY, &d.identity_file, out);
        let timeout = forms::seconds(field::RELAY_TIMEOUT, &d.timeout_seconds, out);
        let given = if d.kind.supports_sudo() { d.sudo } else { None };
        let sudo = match rules::sudo_for(d.kind, given) {
            Ok(value) => Some(value),
            Err(_) => {
                out.push(
                    Problem::new(field::RELAY_SUDO, Code::SudoRequired)
                        .with("preset", d.kind.code()),
                );
                None
            }
        };
        let host = host?;
        let slug = ids::unique(&format!("{}-{host}", d.kind.code()), "relay", |candidate| {
            config.relays.iter().any(|r| r.id.as_str() == candidate)
        });
        let Ok(id) = RelayId::parse(&slug) else {
            out.push(
                Problem::new(field::RELAY_HOST, Code::HostInvalid).with("value", host.as_str()),
            );
            return None;
        };
        Some(Relay {
            id,
            kind: d.kind,
            host,
            port: port?,
            user: user?,
            identity_file: key?,
            timeout: timeout?,
            sudo: sudo?,
        })
    }

    // ---- step 4 ----

    fn power_part(&self, out: &mut Vec<Problem>) -> Option<(Option<PowerMethod>, Option<Rdp>)> {
        let power = if self.allow_sleep || self.allow_shutdown {
            match self.power {
                PowerChoice::Ssh => {
                    if !self.allow_sleep {
                        out.push(Problem::new(field::ALLOW_SLEEP, Code::PowerSshNeedsBoth));
                    } else if !self.allow_shutdown {
                        out.push(Problem::new(field::ALLOW_SHUTDOWN, Code::PowerSshNeedsBoth));
                    }
                    let user = forms::user(field::SSH_USER, &self.ssh_user, out);
                    let port = forms::port(field::SSH_PORT, &self.ssh_port, out);
                    let key = forms::key_file(field::SSH_KEY, &self.ssh_identity_file, out);
                    let both = self.allow_sleep && self.allow_shutdown;
                    match (both, user, port, key) {
                        (true, Some(user), Some(port), Some(identity_file)) => {
                            Some(Some(PowerMethod::Ssh {
                                user,
                                port,
                                identity_file,
                            }))
                        }
                        _ => None,
                    }
                }
                PowerChoice::Command => {
                    let base = self.base.as_ref();
                    let sleep = if self.allow_sleep {
                        forms::command(
                            field::SLEEP_COMMAND,
                            &self.sleep_command,
                            &["host"],
                            base.and_then(|b| b.sleep_argv.as_ref()),
                            out,
                        )
                        .map(Some)
                    } else {
                        Some(None)
                    };
                    let shutdown = if self.allow_shutdown {
                        forms::command(
                            field::SHUTDOWN_COMMAND,
                            &self.shutdown_command,
                            &["host"],
                            base.and_then(|b| b.shutdown_argv.as_ref()),
                            out,
                        )
                        .map(Some)
                    } else {
                        Some(None)
                    };
                    Some(Some(PowerMethod::Command {
                        sleep: sleep?,
                        shutdown: shutdown?,
                    }))
                }
            }
        } else {
            Some(None)
        };
        let rdp = if self.allow_rdp {
            forms::required(field::RDP_USER, &self.rdp_user, out).map(|user| {
                Some(Rdp {
                    user: user.to_owned(),
                    options: self
                        .base
                        .as_ref()
                        .map(|b| b.rdp_options.clone())
                        .unwrap_or_default(),
                })
            })
        } else {
            Some(None)
        };
        Some((power?, rdp?))
    }
}

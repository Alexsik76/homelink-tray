//! Turns the raw file contents into validated domain types.
//!
//! Every problem is collected with its path (such as `pcs[1].ports[0]`); validation does not
//! stop at the first one. Values are never defaulted here: every setting comes from the file.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::Duration;

use super::raw::{
    RawConfig, RawOs, RawPc, RawPower, RawRdp, RawRelay, RawRelayKind, RawTiming,
    RawTimingOverride, RawTunnel, RawWake,
};
use super::{Config, Issue, LogConfig, VERSION};
use crate::domain::{
    Argv, Host, InterfaceName, InvalidValue, MacAddr, Os, Pc, PcId, PowerMethod, Rdp, Relay,
    RelayId, RelayKind, Timing, Tunnel, TunnelId, TunnelKind, WakeMethod,
};
use crate::i18n::{Lang, LanguageSetting};

/// Upper bound of every timeout and interval, in seconds. A technical sanity limit.
pub const MAX_SECONDS: u64 = 3600;

struct Ctx {
    issues: Vec<Issue>,
}

impl Ctx {
    fn err(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(Issue {
            path: path.into(),
            message: message.into(),
        });
    }

    /// Unwraps a parse result, recording the failure under `path`.
    fn check<T>(&mut self, path: &str, result: Result<T, InvalidValue>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(InvalidValue(message)) => {
                self.err(path, message);
                None
            }
        }
    }

    fn non_empty(&mut self, path: &str, value: &str) -> bool {
        let ok = !value.trim().is_empty();
        if !ok {
            self.err(path, "must not be empty");
        }
        ok
    }

    fn seconds(&mut self, path: &str, value: u64) -> Option<Duration> {
        self.bounded(path, value, MAX_SECONDS, "seconds")
            .map(Duration::from_secs)
    }

    fn millis(&mut self, path: &str, value: u64) -> Option<Duration> {
        self.bounded(path, value, MAX_SECONDS * 1000, "ms")
            .map(Duration::from_millis)
    }

    fn bounded(&mut self, path: &str, value: u64, max: u64, unit: &str) -> Option<u64> {
        if value == 0 {
            self.err(path, "must be greater than 0");
            None
        } else if value > max {
            self.err(path, format!("must be at most {max} {unit}, got {value}"));
            None
        } else {
            Some(value)
        }
    }

    fn port(&mut self, path: &str, value: i64) -> Option<u16> {
        match u16::try_from(value) {
            Ok(port) if port != 0 => Some(port),
            _ => {
                self.err(
                    path,
                    format!("must be a port number from 1 to 65535, got {value}"),
                );
                None
            }
        }
    }

    fn argv(&mut self, path: &str, items: &[String], allowed: &[&str]) -> Option<Argv> {
        let parsed = Argv::parse(items.to_vec(), allowed);
        self.check(path, parsed)
    }
}

/// Validates a parsed file. Returns the configuration with warnings, or all problems found.
pub(super) fn validate(raw: RawConfig) -> Result<(Config, Vec<Issue>), Vec<Issue>> {
    let mut ctx = Ctx { issues: Vec::new() };
    let mut warnings = Vec::new();

    if raw.version != VERSION {
        ctx.err(
            "version",
            format!(
                "unsupported version {}; this program reads version {VERSION}",
                raw.version
            ),
        );
    }
    let language = LanguageSetting::parse(&raw.language);
    if language.is_none() {
        let codes: Vec<&str> = Lang::ALL.iter().map(|l| l.code()).collect();
        ctx.err(
            "language",
            format!(
                "must be 'auto' or one of: {}; got '{}'",
                codes.join(", "),
                raw.language
            ),
        );
    }
    check_relative_path(&mut ctx, "log.file", &raw.log.file);

    let defaults = timing_from_raw(&mut ctx, "defaults", &raw.defaults);

    let tunnels = validate_tunnels(&mut ctx, &raw.tunnels);
    let relays = validate_relays(&mut ctx, &raw.relays);
    let pcs = validate_pcs(
        &mut ctx,
        &raw.pcs,
        defaults.as_ref(),
        &raw.tunnels,
        &raw.relays,
    );

    // Unused tunnels and relays are only worth a warning.
    for (i, t) in raw.tunnels.iter().enumerate() {
        if !raw
            .pcs
            .iter()
            .any(|p| p.tunnel.as_deref() == Some(t.id.as_str()))
        {
            warnings.push(Issue {
                path: format!("tunnels[{i}]"),
                message: format!("tunnel '{}' is not used by any PC", t.id),
            });
        }
    }
    for (i, r) in raw.relays.iter().enumerate() {
        let used = raw
            .pcs
            .iter()
            .any(|p| matches!(&p.wake, Some(RawWake::Relay { relay, .. }) if *relay == r.id));
        if !used {
            warnings.push(Issue {
                path: format!("relays[{i}]"),
                message: format!("relay '{}' is not used by any PC", r.id),
            });
        }
    }

    if !ctx.issues.is_empty() {
        return Err(ctx.issues);
    }
    match (language, defaults) {
        (Some(language), Some(defaults)) => Ok((
            Config {
                language,
                log: LogConfig {
                    file: raw.log.file,
                    level: raw.log.level,
                },
                defaults,
                tunnels,
                relays,
                pcs,
            },
            warnings,
        )),
        // Unreachable: a missing value always records an issue above.
        _ => Err(vec![Issue {
            path: String::new(),
            message: "invalid configuration".into(),
        }]),
    }
}

/// A file name relative to the configuration directory: not absolute, no `..`.
fn check_relative_path(ctx: &mut Ctx, path: &str, value: &str) {
    if !ctx.non_empty(path, value) {
        return;
    }
    let bytes = value.as_bytes();
    let drive = bytes.get(1) == Some(&b':') && bytes.first().is_some_and(u8::is_ascii_alphabetic);
    if value.starts_with(['/', '\\']) || drive {
        ctx.err(
            path,
            format!("must be relative to the configuration folder, got '{value}'"),
        );
    } else if value.split(['/', '\\']).any(|part| part == "..") {
        ctx.err(
            path,
            format!("must stay inside the configuration folder, got '{value}'"),
        );
    }
}

fn timing_from_raw(ctx: &mut Ctx, path: &str, raw: &RawTiming) -> Option<Timing> {
    let over = RawTimingOverride {
        poll_seconds: Some(raw.poll_seconds),
        wake_poll_seconds: Some(raw.wake_poll_seconds),
        port_timeout_ms: Some(raw.port_timeout_ms),
        offline_after_failures: Some(raw.offline_after_failures),
        wake_timeout: Some(raw.wake_timeout),
        power_timeout: Some(raw.power_timeout),
    };
    merge_timing(ctx, path, None, &over)
}

/// Applies the keys present in `over` on top of `base` (every key is required when there
/// is no base), checks each value and then the relations between them.
fn merge_timing(
    ctx: &mut Ctx,
    path: &str,
    base: Option<&Timing>,
    over: &RawTimingOverride,
) -> Option<Timing> {
    let mut ok = true;
    let mut pick = |ctx: &mut Ctx,
                    key: &str,
                    value: Option<u64>,
                    from_base: Option<Duration>,
                    convert: fn(&mut Ctx, &str, u64) -> Option<Duration>| {
        match value {
            Some(v) => {
                let d = convert(ctx, &format!("{path}.{key}"), v);
                ok &= d.is_some();
                d
            }
            None => {
                ok &= from_base.is_some();
                from_base
            }
        }
    };
    let poll = pick(
        ctx,
        "poll_seconds",
        over.poll_seconds,
        base.map(|b| b.poll),
        Ctx::seconds,
    );
    let wake_poll = pick(
        ctx,
        "wake_poll_seconds",
        over.wake_poll_seconds,
        base.map(|b| b.wake_poll),
        Ctx::seconds,
    );
    let port_timeout = pick(
        ctx,
        "port_timeout_ms",
        over.port_timeout_ms,
        base.map(|b| b.port_timeout),
        Ctx::millis,
    );
    let wake_timeout = pick(
        ctx,
        "wake_timeout",
        over.wake_timeout,
        base.map(|b| b.wake_timeout),
        Ctx::seconds,
    );
    let power_timeout = pick(
        ctx,
        "power_timeout",
        over.power_timeout,
        base.map(|b| b.power_timeout),
        Ctx::seconds,
    );
    let failures = match over.offline_after_failures {
        Some(0) => {
            ctx.err(
                format!("{path}.offline_after_failures"),
                "must be at least 1",
            );
            None
        }
        Some(n) => match u32::try_from(n) {
            Ok(n) => Some(n),
            Err(_) => {
                ctx.err(
                    format!("{path}.offline_after_failures"),
                    format!("is too large: {n}"),
                );
                None
            }
        },
        None => base.map(|b| b.offline_after_failures),
    };
    if !ok {
        return None;
    }
    let timing = Timing {
        poll: poll?,
        wake_poll: wake_poll?,
        port_timeout: port_timeout?,
        offline_after_failures: failures?,
        wake_timeout: wake_timeout?,
        power_timeout: power_timeout?,
    };
    let mut related = true;
    if timing.wake_poll > timing.poll {
        ctx.err(path, "wake_poll_seconds must not exceed poll_seconds");
        related = false;
    }
    if timing.wake_timeout < timing.wake_poll {
        ctx.err(path, "wake_timeout must be at least wake_poll_seconds");
        related = false;
    }
    related.then_some(timing)
}

fn validate_tunnels(ctx: &mut Ctx, raw: &[RawTunnel]) -> Vec<Tunnel> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::new();
    for (i, t) in raw.iter().enumerate() {
        let at = |key: &str| format!("tunnels[{i}].{key}");
        let id = ctx.check(&at("id"), TunnelId::parse(&t.id));
        if let Some(first) = seen.insert(t.id.as_str(), i) {
            ctx.err(
                at("id"),
                format!("duplicate id '{}' (also tunnels[{first}])", t.id),
            );
        }
        let name_ok = ctx.non_empty(&at("name"), &t.name);
        let exe_ok = ctx.non_empty(&at("wireguard_exe"), &t.wireguard_exe);
        let conf_ok = ctx
            .check(&at("conf"), TunnelKind::wireguard_name(&t.conf))
            .is_some();
        let probe_host = ctx.check(&at("probe_host"), Host::parse(&t.probe_host));
        let timeout = ctx.seconds(&at("timeout"), t.timeout);
        let ping_timeout = ctx.millis(&at("ping_timeout_ms"), t.ping_timeout_ms);
        if let (Some(id), true, true, true, Some(probe_host), Some(timeout), Some(ping_timeout)) = (
            id,
            name_ok,
            exe_ok,
            conf_ok,
            probe_host,
            timeout,
            ping_timeout,
        ) {
            out.push(Tunnel {
                id,
                name: t.name.clone(),
                kind: TunnelKind::WireGuardService {
                    exe: t.wireguard_exe.clone(),
                    conf: t.conf.clone(),
                },
                probe_host,
                timeout,
                ping_timeout,
            });
        }
    }
    out
}

fn relay_kind(raw: RawRelayKind) -> RelayKind {
    match raw {
        RawRelayKind::Routeros => RelayKind::RouterOs,
        RawRelayKind::Etherwake => RelayKind::Etherwake,
        RawRelayKind::Wakeonlan => RelayKind::Wakeonlan,
    }
}

fn validate_relays(ctx: &mut Ctx, raw: &[RawRelay]) -> Vec<Relay> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::new();
    for (i, r) in raw.iter().enumerate() {
        let at = |key: &str| format!("relays[{i}].{key}");
        let id = ctx.check(&at("id"), RelayId::parse(&r.id));
        if let Some(first) = seen.insert(r.id.as_str(), i) {
            ctx.err(
                at("id"),
                format!("duplicate id '{}' (also relays[{first}])", r.id),
            );
        }
        let host = ctx.check(&at("host"), Host::parse(&r.host));
        let port = ctx.port(&at("port"), r.port);
        let user_ok = check_ssh_user(ctx, &at("user"), &r.user);
        let key_ok = check_absolute_path(ctx, &at("identity_file"), &r.identity_file);
        let timeout = ctx.seconds(&at("timeout"), r.timeout);
        let kind = relay_kind(r.kind);
        // `sudo` is a required yes/no for presets whose tool may need root and does not
        // exist for the others; there is no default.
        let sudo = match (kind.supports_sudo(), r.sudo) {
            (true, Some(value)) => Some(value),
            (false, None) => Some(false),
            (true, None) => {
                ctx.err(
                    at("sudo"),
                    format!(
                        "is required by the '{}' preset: true if the command must run as `sudo -n`, false if not",
                        kind.code()
                    ),
                );
                None
            }
            (false, Some(_)) => {
                ctx.err(
                    at("sudo"),
                    format!("is not used by the '{}' preset; remove it", kind.code()),
                );
                None
            }
        };
        if let (Some(id), Some(host), Some(port), true, true, Some(timeout), Some(sudo)) =
            (id, host, port, user_ok, key_ok, timeout, sudo)
        {
            out.push(Relay {
                id,
                kind,
                host,
                port,
                user: r.user.clone(),
                identity_file: r.identity_file.clone(),
                timeout,
                sudo,
            });
        }
    }
    out
}

fn validate_pcs(
    ctx: &mut Ctx,
    raw: &[RawPc],
    defaults: Option<&Timing>,
    tunnels: &[RawTunnel],
    relays: &[RawRelay],
) -> Vec<Pc> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::new();
    for (i, p) in raw.iter().enumerate() {
        let at = |key: &str| format!("pcs[{i}].{key}");
        let id = ctx.check(&at("id"), PcId::parse(&p.id));
        if let Some(first) = seen.insert(p.id.as_str(), i) {
            ctx.err(
                at("id"),
                format!("duplicate id '{}' (also pcs[{first}])", p.id),
            );
        }
        let name_ok = ctx.non_empty(&at("name"), &p.name);
        let host = ctx.check(&at("host"), Host::parse(&p.host));
        let tunnel = match &p.tunnel {
            None => Some(None),
            Some(t) => {
                let parsed = ctx.check(&at("tunnel"), TunnelId::parse(t));
                if parsed.is_some() && !tunnels.iter().any(|x| x.id == *t) {
                    ctx.err(at("tunnel"), format!("refers to unknown tunnel '{t}'"));
                    None
                } else {
                    parsed.map(Some)
                }
            }
        };
        let ports = validate_ports(ctx, &at("ports"), &p.ports);
        let wake = match &p.wake {
            None => Some(None),
            Some(w) => validate_wake(ctx, &at("wake"), w, relays).map(Some),
        };
        let power = match &p.power {
            None => Some(None),
            Some(w) => validate_power(ctx, &at("power"), w).map(Some),
        };
        let rdp = match &p.rdp {
            None => Some(None),
            Some(r) => validate_rdp(ctx, &at("rdp"), r).map(Some),
        };
        let timing = match (defaults, &p.timing) {
            (Some(d), None) => Some(*d),
            (base, Some(over)) => merge_timing(ctx, &at("timing"), base, over),
            (None, None) => None,
        };
        if let (
            Some(id),
            true,
            Some(host),
            Some(tunnel),
            Some(ports),
            Some(wake),
            Some(power),
            Some(rdp),
            Some(timing),
        ) = (id, name_ok, host, tunnel, ports, wake, power, rdp, timing)
        {
            out.push(Pc {
                id,
                name: p.name.clone(),
                os: match p.os {
                    RawOs::Windows => Os::Windows,
                    RawOs::Linux => Os::Linux,
                },
                host,
                tunnel,
                ports,
                wake,
                power,
                rdp,
                timing,
            });
        }
    }
    out
}

fn validate_ports(ctx: &mut Ctx, path: &str, raw: &[i64]) -> Option<Vec<u16>> {
    if raw.is_empty() {
        ctx.err(path, "must list at least one port");
        return None;
    }
    let mut ports: Vec<u16> = Vec::new();
    let mut ok = true;
    for (j, value) in raw.iter().enumerate() {
        match ctx.port(&format!("{path}[{j}]"), *value) {
            Some(port) if ports.contains(&port) => {
                ctx.err(format!("{path}[{j}]"), format!("duplicate port {port}"));
                ok = false;
            }
            Some(port) => ports.push(port),
            None => ok = false,
        }
    }
    ok.then_some(ports)
}

/// An IPv4 address: unicast or broadcast, nothing else is assumed.
fn parse_ipv4(value: &str) -> Result<Ipv4Addr, InvalidValue> {
    value.parse::<Ipv4Addr>().map_err(|_| {
        InvalidValue(format!(
            "'{value}' is not an IPv4 address, e.g. 192.168.88.10 (a PC) or 192.168.88.255 (a broadcast)"
        ))
    })
}

fn validate_wake(
    ctx: &mut Ctx,
    path: &str,
    raw: &RawWake,
    relays: &[RawRelay],
) -> Option<WakeMethod> {
    match raw {
        RawWake::Relay {
            relay,
            mac,
            interface,
            address,
        } => {
            let id = ctx.check(&format!("{path}.relay"), RelayId::parse(relay));
            let known = relays.iter().find(|r| r.id == *relay);
            if id.is_some() && known.is_none() {
                ctx.err(
                    format!("{path}.relay"),
                    format!("refers to unknown relay '{relay}'"),
                );
            }
            let mac = ctx.check(&format!("{path}.mac"), MacAddr::parse(mac));
            let interface_parsed = interface
                .as_deref()
                .map(|i| ctx.check(&format!("{path}.interface"), InterfaceName::parse(i)));
            let address_parsed = address
                .as_deref()
                .map(|a| ctx.check(&format!("{path}.address"), parse_ipv4(a)));
            // The preset decides which of the optional fields are required or refused.
            let mut preset_ok = true;
            if let Some(kind) = known.map(|r| relay_kind(r.kind)) {
                let name = kind.code();
                if kind.uses_interface() && interface.is_none() {
                    ctx.err(
                        format!("{path}.interface"),
                        format!("is required by the '{name}' preset of relay '{relay}'"),
                    );
                    preset_ok = false;
                }
                if !kind.uses_interface() && interface.is_some() {
                    ctx.err(
                        format!("{path}.interface"),
                        format!("is not used by the '{name}' preset of relay '{relay}'; remove it"),
                    );
                    preset_ok = false;
                }
                if !kind.uses_address() && address.is_some() {
                    ctx.err(
                        format!("{path}.address"),
                        format!("is not used by the '{name}' preset of relay '{relay}'; remove it"),
                    );
                    preset_ok = false;
                }
            }
            preset_ok.then_some(())?;
            Some(WakeMethod::Relay {
                relay: id.filter(|_| known.is_some())?,
                mac: mac?,
                interface: match interface_parsed {
                    None => None,
                    Some(parsed) => Some(parsed?),
                },
                address: match address_parsed {
                    None => None,
                    Some(parsed) => Some(parsed?),
                },
            })
        }
        RawWake::MagicPacket { mac, address, port } => {
            let mac = ctx.check(&format!("{path}.mac"), MacAddr::parse(mac));
            let address = ctx.check(&format!("{path}.address"), parse_ipv4(address));
            let port = ctx.port(&format!("{path}.port"), *port);
            Some(WakeMethod::MagicPacket {
                mac: mac?,
                address: address?,
                port: port?,
            })
        }
        RawWake::Command {
            command,
            mac,
            timeout,
        } => {
            let mac = match mac {
                None => Some(None),
                Some(m) => ctx
                    .check(&format!("{path}.mac"), MacAddr::parse(m))
                    .map(Some),
            };
            let allowed: &[&str] = if mac.as_ref().is_some_and(Option::is_some) {
                &["host", "mac"]
            } else {
                &["host"]
            };
            let argv = ctx.argv(&format!("{path}.command"), command, allowed);
            let timeout = ctx.seconds(&format!("{path}.timeout"), *timeout);
            Some(WakeMethod::Command {
                argv: argv?,
                mac: mac?,
                timeout: timeout?,
            })
        }
    }
}

fn validate_power(ctx: &mut Ctx, path: &str, raw: &RawPower) -> Option<PowerMethod> {
    match raw {
        RawPower::Ssh {
            user,
            port,
            identity_file,
        } => {
            let user_ok = check_ssh_user(ctx, &format!("{path}.user"), user);
            let port = ctx.port(&format!("{path}.port"), *port);
            let key_ok = check_absolute_path(ctx, &format!("{path}.identity_file"), identity_file);
            Some(PowerMethod::Ssh {
                user: user_ok.then(|| user.clone())?,
                port: port?,
                identity_file: key_ok.then(|| identity_file.clone())?,
            })
        }
        RawPower::Command { sleep, shutdown } => {
            if sleep.is_none() && shutdown.is_none() {
                ctx.err(path, "needs at least one of 'sleep' and 'shutdown'");
                return None;
            }
            let mut parse = |key: &str, items: &Option<Vec<String>>| match items {
                None => Some(None),
                Some(items) => ctx
                    .argv(&format!("{path}.{key}"), items, &["host"])
                    .map(Some),
            };
            let sleep = parse("sleep", sleep);
            let shutdown = parse("shutdown", shutdown);
            Some(PowerMethod::Command {
                sleep: sleep?,
                shutdown: shutdown?,
            })
        }
    }
}

/// A key file path must be absolute, so that the elevated program never depends on which
/// profile `ssh` would resolve a relative or `~` path in. Recognises Windows (`C:\`, `C:/`,
/// `\\server`) and Unix (`/`) forms regardless of the platform the program runs on.
fn check_absolute_path(ctx: &mut Ctx, path: &str, value: &str) -> bool {
    if !ctx.non_empty(path, value) {
        return false;
    }
    let bytes = value.as_bytes();
    let drive = bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.get(1) == Some(&b':')
        && matches!(bytes.get(2), Some(b'\\' | b'/'));
    if !(value.starts_with(['/', '\\']) || drive) || value.chars().any(char::is_control) {
        ctx.err(
            path,
            format!("must be an absolute path (such as C:\\Users\\me\\.ssh\\key), got '{value}'"),
        );
        return false;
    }
    true
}

/// An SSH user becomes part of a command line: no blanks, no `@`, no leading `-`.
fn check_ssh_user(ctx: &mut Ctx, path: &str, user: &str) -> bool {
    if !ctx.non_empty(path, user) {
        return false;
    }
    if user.starts_with('-') || user.contains('@') || user.chars().any(|c| c.is_whitespace()) {
        ctx.err(
            path,
            format!("'{user}' must not contain blanks or '@' or start with '-'"),
        );
        return false;
    }
    true
}

fn validate_rdp(ctx: &mut Ctx, path: &str, raw: &RawRdp) -> Option<Rdp> {
    let user_ok = ctx.non_empty(&format!("{path}.user"), &raw.user);
    user_ok.then(|| Rdp {
        user: raw.user.clone(),
        options: raw.options.clone(),
    })
}

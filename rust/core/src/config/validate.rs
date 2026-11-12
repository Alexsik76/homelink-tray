//! Turns the raw file contents into validated domain types.
//!
//! Every problem is collected with its path (such as `pcs[1].ports[0]`); validation does not
//! stop at the first one. Values are never defaulted here: every setting comes from the file.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::Duration;

use super::raw::{
    RawConfig, RawOs, RawPc, RawPower, RawRdp, RawRouter, RawTiming, RawTimingOverride, RawTunnel,
    RawWake,
};
use super::{Config, Issue, LogConfig, VERSION};
use crate::domain::{
    ApiUrl, Argv, Host, InvalidValue, MacAddr, Os, Pc, PcId, PowerMethod, Rdp, Router, RouterId,
    RouterKind, Timing, Tunnel, TunnelId, TunnelKind, WakeMethod,
};
use crate::i18n::{Lang, LanguageSetting};
use crate::secret::SecretRef;

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
    let routers = validate_routers(&mut ctx, &raw.routers);
    let pcs = validate_pcs(
        &mut ctx,
        &raw.pcs,
        defaults.as_ref(),
        &raw.tunnels,
        &raw.routers,
    );

    // Unused tunnels and routers are only worth a warning.
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
    for (i, r) in raw.routers.iter().enumerate() {
        let used = raw
            .pcs
            .iter()
            .any(|p| matches!(&p.wake, Some(RawWake::Mikrotik { router, .. }) if *router == r.id));
        if !used {
            warnings.push(Issue {
                path: format!("routers[{i}]"),
                message: format!("router '{}' is not used by any PC", r.id),
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
                routers,
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

fn validate_routers(ctx: &mut Ctx, raw: &[RawRouter]) -> Vec<Router> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::new();
    for (i, r) in raw.iter().enumerate() {
        let at = |key: &str| format!("routers[{i}].{key}");
        let id = ctx.check(&at("id"), RouterId::parse(&r.id));
        if let Some(first) = seen.insert(r.id.as_str(), i) {
            ctx.err(
                at("id"),
                format!("duplicate id '{}' (also routers[{first}])", r.id),
            );
        }
        let api_url = ctx.check(&at("api_url"), ApiUrl::parse(&r.api_url));
        let user_ok = ctx.non_empty(&at("user"), &r.user);
        let password_ref = SecretRef::new(&r.password_ref).map_err(|e| InvalidValue(e.to_string()));
        let password_ref = ctx.check(&at("password_ref"), password_ref);
        let timeout = ctx.seconds(&at("timeout"), r.timeout);
        if let (Some(id), Some(api_url), true, Some(password_ref), Some(timeout)) =
            (id, api_url, user_ok, password_ref, timeout)
        {
            out.push(Router {
                id,
                kind: RouterKind::Mikrotik,
                api_url,
                user: r.user.clone(),
                password_ref,
                verify_tls: r.verify_tls,
                timeout,
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
    routers: &[RawRouter],
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
            Some(w) => validate_wake(ctx, &at("wake"), w, routers).map(Some),
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

fn validate_wake(
    ctx: &mut Ctx,
    path: &str,
    raw: &RawWake,
    routers: &[RawRouter],
) -> Option<WakeMethod> {
    match raw {
        RawWake::Mikrotik {
            router,
            mac,
            interface,
        } => {
            let id = ctx.check(&format!("{path}.router"), RouterId::parse(router));
            let known = routers.iter().any(|r| r.id == *router);
            if id.is_some() && !known {
                ctx.err(
                    format!("{path}.router"),
                    format!("refers to unknown router '{router}'"),
                );
            }
            let mac = ctx.check(&format!("{path}.mac"), MacAddr::parse(mac));
            let iface_ok = ctx.non_empty(&format!("{path}.interface"), interface);
            Some(WakeMethod::MikrotikRest {
                router: id.filter(|_| known)?,
                mac: mac?,
                interface: iface_ok.then(|| interface.clone())?,
            })
        }
        RawWake::MagicPacket {
            mac,
            broadcast,
            port,
        } => {
            let mac = ctx.check(&format!("{path}.mac"), MacAddr::parse(mac));
            let broadcast = broadcast.parse::<Ipv4Addr>().map_err(|_| {
                InvalidValue(format!(
                    "'{broadcast}' is not an IPv4 address, e.g. 255.255.255.255"
                ))
            });
            let broadcast = ctx.check(&format!("{path}.broadcast"), broadcast);
            let port = ctx.port(&format!("{path}.port"), *port);
            Some(WakeMethod::MagicPacket {
                mac: mac?,
                broadcast: broadcast?,
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
            let identity_ok = match identity_file {
                Some(f) => ctx.non_empty(&format!("{path}.identity_file"), f),
                None => true,
            };
            identity_ok.then_some(())?;
            Some(PowerMethod::Ssh {
                user: user_ok.then(|| user.clone())?,
                port: port?,
                identity_file: identity_file.clone(),
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

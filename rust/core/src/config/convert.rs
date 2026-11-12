//! Domain types back to the raw file layout, for saving.

use super::Config;
use super::raw::{
    RawConfig, RawLog, RawOs, RawPc, RawPower, RawRdp, RawRouter, RawRouterKind, RawTiming,
    RawTimingOverride, RawTunnel, RawTunnelKind, RawWake,
};
use crate::domain::{Os, Pc, PowerMethod, Router, Timing, Tunnel, TunnelKind, WakeMethod};

pub(super) fn to_raw(config: &Config) -> RawConfig {
    RawConfig {
        version: super::VERSION,
        language: config.language.as_str().to_owned(),
        log: RawLog {
            file: config.log.file.clone(),
            level: config.log.level,
        },
        defaults: raw_timing(&config.defaults),
        tunnels: config.tunnels.iter().map(raw_tunnel).collect(),
        routers: config.routers.iter().map(raw_router).collect(),
        pcs: config
            .pcs
            .iter()
            .map(|pc| raw_pc(pc, &config.defaults))
            .collect(),
    }
}

fn secs(d: std::time::Duration) -> u64 {
    d.as_secs()
}

fn millis(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn raw_timing(t: &Timing) -> RawTiming {
    RawTiming {
        poll_seconds: secs(t.poll),
        wake_poll_seconds: secs(t.wake_poll),
        port_timeout_ms: millis(t.port_timeout),
        offline_after_failures: u64::from(t.offline_after_failures),
        wake_timeout: secs(t.wake_timeout),
        power_timeout: secs(t.power_timeout),
    }
}

/// Only the values that differ from the defaults; `None` when nothing differs.
fn raw_override(t: &Timing, d: &Timing) -> Option<RawTimingOverride> {
    let diff = |a: u64, b: u64| (a != b).then_some(a);
    let (t, d) = (raw_timing(t), raw_timing(d));
    let over = RawTimingOverride {
        poll_seconds: diff(t.poll_seconds, d.poll_seconds),
        wake_poll_seconds: diff(t.wake_poll_seconds, d.wake_poll_seconds),
        port_timeout_ms: diff(t.port_timeout_ms, d.port_timeout_ms),
        offline_after_failures: diff(t.offline_after_failures, d.offline_after_failures),
        wake_timeout: diff(t.wake_timeout, d.wake_timeout),
        power_timeout: diff(t.power_timeout, d.power_timeout),
    };
    (over != RawTimingOverride::default()).then_some(over)
}

fn raw_tunnel(t: &Tunnel) -> RawTunnel {
    let TunnelKind::WireGuardService { exe, conf } = &t.kind;
    RawTunnel {
        id: t.id.to_string(),
        name: t.name.clone(),
        kind: RawTunnelKind::Wireguard,
        conf: conf.clone(),
        wireguard_exe: exe.clone(),
        probe_host: t.probe_host.to_string(),
        timeout: secs(t.timeout),
        ping_timeout_ms: millis(t.ping_timeout),
    }
}

fn raw_router(r: &Router) -> RawRouter {
    RawRouter {
        id: r.id.to_string(),
        kind: RawRouterKind::Mikrotik,
        host: r.host.to_string(),
        port: i64::from(r.port),
        user: r.user.clone(),
        identity_file: r.identity_file.clone(),
        timeout: secs(r.timeout),
    }
}

fn raw_pc(pc: &Pc, defaults: &Timing) -> RawPc {
    RawPc {
        id: pc.id.to_string(),
        name: pc.name.clone(),
        os: match pc.os {
            Os::Windows => RawOs::Windows,
            Os::Linux => RawOs::Linux,
        },
        host: pc.host.to_string(),
        tunnel: pc.tunnel.as_ref().map(ToString::to_string),
        ports: pc.ports.iter().map(|p| i64::from(*p)).collect(),
        wake: pc.wake.as_ref().map(raw_wake),
        power: pc.power.as_ref().map(raw_power),
        rdp: pc.rdp.as_ref().map(|r| RawRdp {
            user: r.user.clone(),
            options: r.options.clone(),
        }),
        timing: raw_override(&pc.timing, defaults),
    }
}

fn raw_wake(w: &WakeMethod) -> RawWake {
    match w {
        WakeMethod::MikrotikSsh {
            router,
            mac,
            interface,
        } => RawWake::Mikrotik {
            router: router.to_string(),
            mac: mac.to_string(),
            interface: interface.to_string(),
        },
        WakeMethod::MagicPacket {
            mac,
            broadcast,
            port,
        } => RawWake::MagicPacket {
            mac: mac.to_string(),
            broadcast: broadcast.to_string(),
            port: i64::from(*port),
        },
        WakeMethod::Command { argv, mac, timeout } => RawWake::Command {
            command: argv.items().to_vec(),
            mac: mac.as_ref().map(ToString::to_string),
            timeout: secs(*timeout),
        },
    }
}

fn raw_power(p: &PowerMethod) -> RawPower {
    match p {
        PowerMethod::Ssh {
            user,
            port,
            identity_file,
        } => RawPower::Ssh {
            user: user.clone(),
            port: i64::from(*port),
            identity_file: identity_file.clone(),
        },
        PowerMethod::Command { sleep, shutdown } => RawPower::Command {
            sleep: sleep.as_ref().map(|a| a.items().to_vec()),
            shutdown: shutdown.as_ref().map(|a| a.items().to_vec()),
        },
    }
}

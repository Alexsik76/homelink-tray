//! The configuration schema as it appears in the file.
//!
//! These types only describe the file layout; they are format-neutral (serde) and are
//! turned into validated domain types by `validate`. Numbers are wide integers so that range
//! errors are reported by the validator with a precise path rather than by the parser.

use serde::{Deserialize, Serialize};

/// Verbosity of the log file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Errors and warnings.
    Warn,
    /// Normal operation.
    Info,
    /// Detailed.
    Debug,
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    pub version: u32,
    pub language: String,
    pub log: RawLog,
    pub defaults: RawTiming,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tunnels: Vec<RawTunnel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routers: Vec<RawRouter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pcs: Vec<RawPc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawLog {
    pub file: String,
    pub level: LogLevel,
}

/// Every timing key; required in `[defaults]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTiming {
    pub poll_seconds: u64,
    pub wake_poll_seconds: u64,
    pub port_timeout_ms: u64,
    pub offline_after_failures: u64,
    pub wake_timeout: u64,
    pub power_timeout: u64,
}

/// The same keys, each optional, as a per-PC override of `[defaults]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTimingOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_poll_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_after_failures: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_timeout: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RawTunnelKind {
    Wireguard,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTunnel {
    pub id: String,
    pub name: String,
    pub kind: RawTunnelKind,
    pub conf: String,
    pub wireguard_exe: String,
    pub probe_host: String,
    pub timeout: u64,
    pub ping_timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RawRouterKind {
    Mikrotik,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRouter {
    pub id: String,
    pub kind: RawRouterKind,
    pub host: String,
    pub port: i64,
    pub user: String,
    pub identity_file: String,
    pub timeout: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RawOs {
    Windows,
    Linux,
}

/// A PC. Plain values come before sub-tables so the file renders in a natural order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPc {
    pub id: String,
    pub name: String,
    pub os: RawOs,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<String>,
    pub ports: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<RawWake>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<RawPower>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdp: Option<RawRdp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<RawTimingOverride>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawWake {
    Mikrotik {
        router: String,
        mac: String,
        interface: String,
    },
    MagicPacket {
        mac: String,
        broadcast: String,
        port: i64,
    },
    Command {
        command: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac: Option<String>,
        timeout: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawPower {
    Ssh {
        user: String,
        port: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity_file: Option<String>,
    },
    Command {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sleep: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shutdown: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRdp {
    pub user: String,
    pub options: Vec<String>,
}

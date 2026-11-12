//! Loading, validating and saving the configuration.
//!
//! The program has no built-in values for user settings: everything comes from the file
//! (the only exceptions are purely technical limits, such as [`MAX_SECONDS`]). The file
//! format lives in the `format` module; the schema in `raw`; validation in `validate`.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::domain::{Pc, PcId, Router, RouterId, Timing, Tunnel, TunnelId};
use crate::i18n::LanguageSetting;

mod convert;
mod format;
mod raw;
mod validate;

pub use format::{FILE_NAME, FormatError};
pub use raw::LogLevel;
pub use validate::MAX_SECONDS;

/// The schema version this program reads and writes.
pub const VERSION: u32 = 1;

/// Settings of the log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogConfig {
    /// File name relative to the configuration folder.
    pub file: String,
    /// Verbosity.
    pub level: LogLevel,
}

/// A validated configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Interface language setting.
    pub language: LanguageSetting,
    /// Log file settings.
    pub log: LogConfig,
    /// Timing used by PCs that do not override it (and by tunnels for polling).
    pub defaults: Timing,
    /// Tunnels.
    pub tunnels: Vec<Tunnel>,
    /// Routers.
    pub routers: Vec<Router>,
    /// Remote PCs.
    pub pcs: Vec<Pc>,
}

impl Config {
    /// The tunnel with this id.
    pub fn tunnel(&self, id: &TunnelId) -> Option<&Tunnel> {
        self.tunnels.iter().find(|t| t.id == *id)
    }

    /// The router with this id.
    pub fn router(&self, id: &RouterId) -> Option<&Router> {
        self.routers.iter().find(|r| r.id == *id)
    }

    /// The PC with this id.
    pub fn pc(&self, id: &PcId) -> Option<&Pc> {
        self.pcs.iter().find(|p| p.id == *id)
    }

    /// PCs reached through a tunnel, or, for `None`, the PCs that have no tunnel.
    pub fn pcs_of<'a>(&'a self, tunnel: Option<&'a TunnelId>) -> impl Iterator<Item = &'a Pc> {
        self.pcs.iter().filter(move |p| p.tunnel.as_ref() == tunnel)
    }
}

/// One problem in the file: where it is and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Location such as `pcs[1].ports[0]`; empty for the file as a whole.
    pub path: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// A valid configuration with non-fatal remarks.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    /// The configuration.
    pub config: Config,
    /// Things worth mentioning that do not prevent using the configuration.
    pub warnings: Vec<Issue>,
}

/// Why a configuration could not be loaded or saved.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The file is not valid in the configuration format.
    #[error("invalid file: {0}")]
    Syntax(#[from] FormatError),
    /// The file is well formed but its contents are wrong; every problem is listed.
    #[error("{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))]
    Invalid(Vec<Issue>),
}

/// Parses and validates configuration text.
pub fn parse(text: &str) -> Result<Loaded, ConfigError> {
    let raw = format::parse(text)?;
    match validate::validate(raw) {
        Ok((config, warnings)) => Ok(Loaded { config, warnings }),
        Err(issues) => Err(ConfigError::Invalid(issues)),
    }
}

/// Reads, parses and validates a configuration file.
pub fn load(path: &Path) -> Result<Loaded, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_owned(),
        source,
    })?;
    parse(&text)
}

/// Renders a configuration as file text. The result is checked by parsing it back, so an
/// invalid configuration is never produced.
pub fn render(config: &Config) -> Result<String, ConfigError> {
    let text = format::render(&convert::to_raw(config))?;
    parse(&text)?;
    Ok(text)
}

/// Writes a configuration atomically: a temporary file in the same folder, then a rename.
pub fn save(path: &Path, config: &Config) -> Result<(), ConfigError> {
    let text = render(config)?;
    let io = |source| ConfigError::Io {
        path: path.to_owned(),
        source,
    };
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(|source| {
        let _ = std::fs::remove_file(&tmp);
        ConfigError::Io {
            path: path.to_owned(),
            source,
        }
    })
}

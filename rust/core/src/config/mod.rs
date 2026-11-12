//! Loading, validating and saving the configuration.
//!
//! The program has no built-in values for user settings: everything comes from the file
//! (the only exceptions are purely technical limits, such as [`MAX_SECONDS`]). The file
//! format lives in the `format` module; the schema in `raw`; validation in `validate`.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::domain::{Pc, PcId, Relay, RelayId, Timing, Tunnel, TunnelId};
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
    /// Relays.
    pub relays: Vec<Relay>,
    /// Remote PCs.
    pub pcs: Vec<Pc>,
}

impl Config {
    /// The tunnel with this id.
    pub fn tunnel(&self, id: &TunnelId) -> Option<&Tunnel> {
        self.tunnels.iter().find(|t| t.id == *id)
    }

    /// The relay with this id.
    pub fn relay(&self, id: &RelayId) -> Option<&Relay> {
        self.relays.iter().find(|r| r.id == *id)
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
    /// The text prepared for writing does not describe the configuration being saved. Nothing
    /// is written.
    #[error("refusing to write: {0}")]
    Inconsistent(String),
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

/// Ids that changed since the file was written, so that an entry keeps its place and its
/// comments when its `id` changes. Without a listed rename a changed id is a removal of the
/// old entry (with the comments above it) and an addition of a new one at the end.
///
/// The configuration passed to [`save_with`] must already use the new ids everywhere
/// (including `pcs[].tunnel` and `wake.relay`); this only tells the editor which old entry
/// the new id continues.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Renames {
    /// Renamed tunnels as `(old, new)`.
    pub tunnels: Vec<(TunnelId, TunnelId)>,
    /// Renamed relays as `(old, new)`.
    pub relays: Vec<(RelayId, RelayId)>,
    /// Renamed PCs as `(old, new)`.
    pub pcs: Vec<(PcId, PcId)>,
}

impl Renames {
    fn to_format(&self) -> format::EntryRenames {
        fn pairs<T: ToString>(list: &[(T, T)]) -> Vec<(String, String)> {
            list.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        }
        format::EntryRenames {
            tunnels: pairs(&self.tunnels),
            relays: pairs(&self.relays),
            pcs: pairs(&self.pcs),
        }
    }
}

/// Whether two spellings of a string value are the same value: the editor leaves a value
/// alone when the user's spelling and the program's mean the same (`AA-BB-…` and `aa:bb:…`
/// are one MAC address; `pc.lan.` and `pc.lan` are one host).
fn same_spelling(key: &str, old: &str, new: &str) -> bool {
    use crate::domain::{Host, MacAddr};
    old == new
        || match key {
            "mac" => matches!(
                (MacAddr::parse(old), MacAddr::parse(new)),
                (Ok(a), Ok(b)) if a == b
            ),
            "host" | "probe_host" => matches!(
                (Host::parse(old), Host::parse(new)),
                (Ok(a), Ok(b)) if a == b
            ),
            _ => false,
        }
}

/// Checks that `text` parses and validates to exactly `config`.
fn verify(text: &str, config: &Config) -> Result<(), ConfigError> {
    let loaded = parse(text)?;
    if loaded.config == *config {
        Ok(())
    } else {
        Err(ConfigError::Inconsistent(
            "the text to write does not describe the configuration being saved".into(),
        ))
    }
}

/// Renders a configuration as a new file text. The result is checked by parsing it back:
/// it must validate to exactly `config`, so an invalid configuration is never produced.
pub fn render(config: &Config) -> Result<String, ConfigError> {
    let text = format::render(&convert::to_raw(config))?;
    verify(&text, config)?;
    Ok(text)
}

/// Edits the text of an existing file so that it describes `config`, keeping everything the
/// user wrote by hand that did not change (see the `format` module for the rules). The
/// result is checked: it must validate to exactly `config`.
pub fn edit(existing: &str, config: &Config, renames: &Renames) -> Result<String, ConfigError> {
    let text = format::edit(
        existing,
        &convert::to_raw(config),
        &renames.to_format(),
        &same_spelling,
    )?;
    verify(&text, config)?;
    Ok(text)
}

/// Saves a configuration; see [`save_with`] (no renames).
pub fn save(path: &Path, config: &Config) -> Result<(), ConfigError> {
    save_with(path, config, &Renames::default())
}

/// Saves a configuration to `path`.
///
/// - No file (or an empty one): the file is rendered from scratch.
/// - An existing file: it is edited by key, so comments, blank lines, key order and the
///   formatting of everything that did not change survive. If nothing changed the file is
///   not touched.
/// - An existing file that cannot be read or is not valid TOML: an error is returned and
///   the file is left alone.
///
/// Before anything is written the text is checked to parse and validate to exactly
/// `config`. The write is atomic: a temporary file in the same folder, then a rename.
pub fn save_with(path: &Path, config: &Config, renames: &Renames) -> Result<(), ConfigError> {
    let io = |source| ConfigError::Io {
        path: path.to_owned(),
        source,
    };
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(io(err)),
    };
    let text = match &existing {
        Some(old) if !old.trim().is_empty() => edit(old, config, renames)?,
        _ => render(config)?,
    };
    if existing.as_deref() == Some(text.as_str()) {
        return Ok(());
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{multi_config, single_config};

    #[test]
    fn text_that_describes_another_configuration_is_refused() {
        let text = render(&multi_config()).unwrap();
        assert!(matches!(
            verify(&text, &single_config()),
            Err(ConfigError::Inconsistent(_))
        ));
        assert!(verify(&text, &multi_config()).is_ok());
    }

    #[test]
    fn spellings_of_one_value_are_the_same() {
        assert!(same_spelling(
            "mac",
            "AA-BB-CC-DD-EE-FF",
            "aa:bb:cc:dd:ee:ff"
        ));
        assert!(same_spelling("host", "pc.lan.", "pc.lan"));
        assert!(!same_spelling(
            "name",
            "AA-BB-CC-DD-EE-FF",
            "aa:bb:cc:dd:ee:ff"
        ));
        assert!(!same_spelling(
            "mac",
            "AA-BB-CC-DD-EE-FF",
            "AA-BB-CC-DD-EE-00"
        ));
    }
}

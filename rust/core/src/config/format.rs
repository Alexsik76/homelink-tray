//! The file format. This is the only module that knows the configuration is TOML.
//!
//! The format is TOML. The format code is kept in this one module and the schema in `raw`
//! is format-neutral (serde), so a different format would touch only `parse`, `render` and
//! `FILE_NAME` here.

use super::raw::RawConfig;

/// Default file name of the configuration.
pub const FILE_NAME: &str = "config.toml";

/// A syntax or structure error reported by the parser.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.describe())]
pub struct FormatError {
    /// What is wrong.
    pub message: String,
    /// 1-based line, when known.
    pub line: Option<usize>,
    /// 1-based column, when known.
    pub column: Option<usize>,
}

impl FormatError {
    fn describe(&self) -> String {
        match (self.line, self.column) {
            (Some(line), Some(column)) => {
                format!("line {line}, column {column}: {}", self.message)
            }
            _ => self.message.clone(),
        }
    }
}

const BOM: char = '\u{feff}';

pub(super) fn parse(text: &str) -> Result<RawConfig, FormatError> {
    // Notepad may add a byte order mark.
    let text = text.strip_prefix(BOM).unwrap_or(text);
    toml::from_str(text).map_err(|err| {
        let (line, column) = match err.span() {
            Some(span) => {
                let before = text.get(..span.start).unwrap_or_default();
                let line = before.matches('\n').count() + 1;
                let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
                (Some(line), Some(column))
            }
            None => (None, None),
        };
        FormatError {
            message: err.message().trim().to_owned(),
            line,
            column,
        }
    })
}

pub(super) fn render(raw: &RawConfig) -> Result<String, FormatError> {
    toml::to_string(raw).map_err(|err| FormatError {
        message: format!("cannot write the configuration: {err}"),
        line: None,
        column: None,
    })
}

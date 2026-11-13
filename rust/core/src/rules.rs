//! Field rules shared by the configuration validator and by the forms (wizard, tunnel form).
//!
//! Each rule is a pure function that says what is wrong with a value as a [`Fault`], without
//! any wording. The validator turns a fault into its English message with a path; the forms
//! turn it into a [`crate::problem::Problem`] with a locale key. So a value is accepted by a
//! form if and only if the configuration validator accepts it.

use crate::domain::RelayKind;

/// What is wrong with a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    /// Empty or only blanks.
    Empty,
    /// Contains characters that are not allowed.
    Chars,
    /// Not an absolute path.
    NotAbsolute,
    /// A number that must be greater than 0.
    Zero,
    /// A number above its upper bound.
    TooBig,
    /// Not a port number from 1 to 65535.
    NotPort,
    /// A value the preset requires is missing.
    Missing,
    /// A value the preset does not use is present.
    NotUsed,
}

/// A value that is not empty.
pub(crate) fn non_empty(value: &str) -> Result<(), Fault> {
    if value.trim().is_empty() {
        Err(Fault::Empty)
    } else {
        Ok(())
    }
}

/// An SSH user becomes part of a command line: no blanks, no `@`, no leading `-`.
pub(crate) fn ssh_user(user: &str) -> Result<(), Fault> {
    non_empty(user)?;
    if user.starts_with('-') || user.contains('@') || user.chars().any(char::is_whitespace) {
        Err(Fault::Chars)
    } else {
        Ok(())
    }
}

/// A key file path must be absolute, so that the elevated program never depends on which
/// profile `ssh` would resolve a relative or `~` path in. Recognises Windows (`C:\`, `C:/`,
/// `\\server`) and Unix (`/`) forms regardless of the platform the program runs on.
pub(crate) fn absolute_path(value: &str) -> Result<(), Fault> {
    non_empty(value)?;
    let bytes = value.as_bytes();
    let drive = bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.get(1) == Some(&b':')
        && matches!(bytes.get(2), Some(b'\\' | b'/'));
    if !(value.starts_with(['/', '\\']) || drive) || value.chars().any(char::is_control) {
        Err(Fault::NotAbsolute)
    } else {
        Ok(())
    }
}

/// A TCP or UDP port number.
pub(crate) fn port(value: i64) -> Result<u16, Fault> {
    match u16::try_from(value) {
        Ok(port) if port != 0 => Ok(port),
        _ => Err(Fault::NotPort),
    }
}

/// A number from 1 to `max`.
pub(crate) fn bounded(value: u64, max: u64) -> Result<u64, Fault> {
    if value == 0 {
        Err(Fault::Zero)
    } else if value > max {
        Err(Fault::TooBig)
    } else {
        Ok(value)
    }
}

/// The `interface` of a relay wake method: required by presets that send on a named
/// interface, refused by the others.
pub(crate) fn interface_for(kind: RelayKind, given: bool) -> Result<(), Fault> {
    match (kind.uses_interface(), given) {
        (true, false) => Err(Fault::Missing),
        (false, true) => Err(Fault::NotUsed),
        _ => Ok(()),
    }
}

/// The `address` of a relay wake method: only presets that take one allow it.
pub(crate) fn address_for(kind: RelayKind, given: bool) -> Result<(), Fault> {
    if given && !kind.uses_address() {
        Err(Fault::NotUsed)
    } else {
        Ok(())
    }
}

/// The `sudo` setting of a relay: a required yes/no for presets whose tool may need root, and
/// not allowed for the others (where it is `false`). There is no default.
pub(crate) fn sudo_for(kind: RelayKind, given: Option<bool>) -> Result<bool, Fault> {
    match (kind.supports_sudo(), given) {
        (true, Some(value)) => Ok(value),
        (false, None) => Ok(false),
        (true, None) => Err(Fault::Missing),
        (false, Some(_)) => Err(Fault::NotUsed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users() {
        assert_eq!(ssh_user(""), Err(Fault::Empty));
        assert_eq!(ssh_user("a b"), Err(Fault::Chars));
        assert_eq!(ssh_user("a@b"), Err(Fault::Chars));
        assert_eq!(ssh_user("-x"), Err(Fault::Chars));
        assert!(ssh_user("alex").is_ok());
    }

    #[test]
    fn paths() {
        for ok in [r"C:\k", "C:/k", r"\\srv\k", "/home/a/k"] {
            assert!(absolute_path(ok).is_ok(), "{ok}");
        }
        for bad in ["k", "~/k", r"..\k", "C:k", "/a\nb"] {
            assert!(absolute_path(bad).is_err(), "{bad}");
        }
        assert_eq!(absolute_path(" "), Err(Fault::Empty));
    }

    #[test]
    fn ports_and_bounds() {
        assert_eq!(port(22), Ok(22));
        assert_eq!(port(0), Err(Fault::NotPort));
        assert_eq!(port(65536), Err(Fault::NotPort));
        assert_eq!(port(-1), Err(Fault::NotPort));
        assert_eq!(bounded(0, 10), Err(Fault::Zero));
        assert_eq!(bounded(11, 10), Err(Fault::TooBig));
        assert_eq!(bounded(10, 10), Ok(10));
    }

    #[test]
    fn presets() {
        use RelayKind::{Etherwake, RouterOs, Wakeonlan};
        assert_eq!(interface_for(RouterOs, false), Err(Fault::Missing));
        assert_eq!(interface_for(Etherwake, true), Ok(()));
        assert_eq!(interface_for(Wakeonlan, true), Err(Fault::NotUsed));
        assert_eq!(address_for(Wakeonlan, true), Ok(()));
        assert_eq!(address_for(RouterOs, true), Err(Fault::NotUsed));
        assert_eq!(sudo_for(Etherwake, None), Err(Fault::Missing));
        assert_eq!(sudo_for(Etherwake, Some(true)), Ok(true));
        assert_eq!(sudo_for(RouterOs, None), Ok(false));
        assert_eq!(sudo_for(RouterOs, Some(false)), Err(Fault::NotUsed));
    }
}

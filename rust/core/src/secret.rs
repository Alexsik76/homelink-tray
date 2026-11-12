//! Secrets: a reference stored in the configuration and the store that holds the value.
//!
//! The configuration only ever contains a [`SecretRef`]. The value is a [`Secret`], which
//! hides itself from `Debug` and `Display` and overwrites its memory when dropped. The
//! store itself is a trait; the Windows Credential Manager implementation lives in the app
//! crate.

use std::fmt;

use crate::domain::{Host, RouterId};

/// Longest allowed reference.
const REF_MAX_LEN: usize = 256;

/// A reference to a secret, such as a Credential Manager target name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretRef(String);

/// A malformed [`SecretRef`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "a secret reference must be 1-{REF_MAX_LEN} printable characters without leading or trailing spaces, got '{0}'"
)]
pub struct InvalidSecretRef(String);

impl SecretRef {
    /// Validates a reference.
    pub fn new(s: &str) -> Result<Self, InvalidSecretRef> {
        let ok = !s.is_empty()
            && s.chars().count() <= REF_MAX_LEN
            && s == s.trim()
            && !s.chars().any(char::is_control);
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(InvalidSecretRef(s.to_owned()))
        }
    }

    /// The conventional reference for the password of a router.
    pub fn for_router(id: &RouterId) -> Self {
        Self(format!("HomeLink/router/{id}"))
    }

    /// The reference Windows uses for a saved Remote Desktop password.
    pub fn for_rdp(host: &Host) -> Self {
        Self(format!("TERMSRV/{host}"))
    }

    /// The reference as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A secret value. Prints as `***` and is overwritten when dropped.
pub struct Secret(Vec<u8>);

impl Secret {
    /// Wraps a value.
    pub fn new(value: String) -> Self {
        Self(value.into_bytes())
    }

    /// The value. Keep the result short-lived and never log it.
    pub fn expose(&self) -> &str {
        // The bytes come from a `String` and are only ever zeroed (valid UTF-8).
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// A failure of the secret store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// The store could not be read or written.
    #[error("secret store failed: {0}")]
    Store(String),
}

/// Storage of secrets by reference.
pub trait SecretStore: Send + Sync {
    /// The secret, or `None` when nothing is stored under the reference.
    fn get(&self, reference: &SecretRef) -> Result<Option<Secret>, SecretError>;
    /// Stores or replaces a secret.
    fn set(&self, reference: &SecretRef, value: &Secret) -> Result<(), SecretError>;
    /// Removes a secret; removing a missing one is not an error.
    fn delete(&self, reference: &SecretRef) -> Result<(), SecretError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_validation() {
        assert!(SecretRef::new("HomeLink/router/home").is_ok());
        for bad in ["", " x", "x ", "a\nb", &"x".repeat(257)] {
            assert!(SecretRef::new(bad).is_err(), "{bad:?}");
        }
        assert!(SecretRef::new(&"x".repeat(256)).is_ok());
    }

    #[test]
    fn conventional_references() {
        let id = RouterId::parse("mikrotik").unwrap();
        assert_eq!(
            SecretRef::for_router(&id).as_str(),
            "HomeLink/router/mikrotik"
        );
        let host = Host::parse("10.0.0.5").unwrap();
        assert_eq!(SecretRef::for_rdp(&host).as_str(), "TERMSRV/10.0.0.5");
    }

    #[test]
    fn secret_never_prints_its_value() {
        let secret = Secret::new("hunter2".to_owned());
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert_eq!(format!("{secret}"), "***");
        assert_eq!(secret.expose(), "hunter2");
    }
}

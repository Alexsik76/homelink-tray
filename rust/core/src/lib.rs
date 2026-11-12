//! Platform-independent core of HomeLink.
//!
//! This crate has no Windows and no GUI dependencies and builds on any platform. It holds
//! the domain model, the configuration (loading, validation, saving), notices and
//! interface strings, and the secret store trait. See `docs/rust-architecture.md`.

pub mod config;
pub mod domain;
pub mod i18n;
pub mod notice;
pub mod secret;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

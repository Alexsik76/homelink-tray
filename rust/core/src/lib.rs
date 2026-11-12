//! Platform-independent core of HomeLink.
//!
//! This crate has no Windows and no GUI dependencies and builds on any platform. It holds
//! the domain model, the configuration (loading, validation, saving), notices and
//! interface strings, the secret store trait, the ports that isolate side effects, the
//! state machine and the engine that drives it. See `docs/rust-architecture.md`.

pub mod config;
pub mod domain;
pub mod engine;
pub mod i18n;
pub mod machine;
pub mod notice;
pub mod ports;
pub mod secret;
pub mod snapshot;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

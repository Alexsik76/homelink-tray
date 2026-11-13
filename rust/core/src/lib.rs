//! Platform-independent core of HomeLink.
//!
//! This crate has no Windows and no GUI dependencies and builds on any platform. It holds
//! the domain model, the configuration (loading, validation, saving), notices and
//! interface strings, the ports that isolate side effects, the state machine and the
//! engine that drives it. The program stores no secrets: remote access uses SSH keys
//! through the system OpenSSH client. See `docs/rust-architecture.md`.

pub mod about;
pub mod adapters;
pub mod checks;
pub mod config;
pub mod domain;
pub mod engine;
mod forms;
pub mod i18n;
mod ids;
pub mod machine;
pub mod notice;
pub mod ports;
pub mod problem;
pub mod relay_setup;
mod rules;
pub mod settings;
pub mod snapshot;
pub mod ssh;
pub mod ssh_setup;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tunnel_form;
pub mod wake;
pub mod wizard;

//! Portable adapters: implementations of the traits in [`crate::ports`] that need nothing but
//! `std` and work the same on Linux and Windows.
//!
//! - [`tcp::TcpPortProbe`]: all ports of a host accept a connection (a [`PortProbe`]).
//! - [`magic::MagicPacketWake`]: the direct magic packet over UDP (a [`WakeSender`]).
//! - [`command::StdCommandRunner`]: runs a program without a shell, with a timeout (a
//!   [`CommandRunner`]), and [`command::CommandWake`], the wake method that runs a command.
//! - [`power::CommandPowerRunner`]: sleep and shutdown over SSH or by a configured command
//!   (a [`PowerRunner`]).
//!
//! Not here (they need the Windows API and live in the app crate): the WireGuard service, ICMP
//! echo, decoding of console output in the OEM code page, launching `mstsc`.
//!
//! [`PortProbe`]: crate::ports::PortProbe
//! [`WakeSender`]: crate::ports::WakeSender
//! [`CommandRunner`]: crate::ports::CommandRunner
//! [`PowerRunner`]: crate::ports::PowerRunner

use std::sync::Arc;

use crate::domain::WakeKind;
use crate::ports::{CommandRunner, WakeRegistry};
use crate::wake::RelayWake;

pub mod command;
pub mod magic;
pub mod power;
pub mod tcp;

/// A registry with a sender for every wake method: through a relay over SSH, the direct magic
/// packet, and a command. SSH and commands run through `runner`.
pub fn default_wake_registry(runner: Arc<dyn CommandRunner>) -> WakeRegistry {
    let mut registry = WakeRegistry::new();
    registry.register(WakeKind::Relay, Arc::new(RelayWake::new(runner.clone())));
    registry.register(WakeKind::MagicPacket, Arc::new(magic::MagicPacketWake));
    registry.register(
        WakeKind::Command,
        Arc::new(command::CommandWake::new(runner)),
    );
    registry
}

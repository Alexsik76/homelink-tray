//! What the real mode needs from Windows, built to be tested on any platform.
//!
//! Every decision is a pure function over text or lists (command lines to run, parsing of what
//! a program printed, the content of the `.rdp` file, code page tables), covered by unit tests
//! that run on Linux. The part that touches the system is a thin layer that runs ordinary
//! programs through the core's `CommandRunner` (`sc`, `wireguard.exe`, `ping`, `reg`,
//! `whoami`, `powershell`, `mstsc`), as the Python version did, so it needs no Win32 bindings
//! and compiles everywhere. Whether those programs exist is checked at run time.
//!
//! - [`wireguard`]: the tunnel as a service of the WireGuard client ([`TunnelDriver`]).
//! - [`ping`]: the probe host check ([`Pinger`]).
//! - [`oem`]: decoding of console output in the OEM code page.
//! - [`reg`], [`language`]: registry values through `reg query`, and the display language.
//! - [`rdp`]: the `.rdp` file and launching `mstsc`.
//! - [`elevation`]: administrator check and restart.
//!
//! [`TunnelDriver`]: homelink_core::ports::TunnelDriver
//! [`Pinger`]: homelink_core::ports::Pinger

pub(crate) mod elevation;
pub(crate) mod language;
pub(crate) mod oem;
pub(crate) mod ping;
pub(crate) mod rdp;
pub(crate) mod reg;
pub(crate) mod wireguard;

#[cfg(test)]
pub(crate) mod scripted {
    //! A command runner for tests that answers by what the command line contains.

    use std::sync::Mutex;
    use std::time::Duration;

    use homelink_core::ports::{CommandOutput, CommandRunner};

    /// Answers the first rule whose text occurs in the joined command line; records every call.
    pub(crate) struct Scripted {
        rules: Vec<(String, CommandOutput)>,
        calls: Mutex<Vec<(Vec<String>, Duration)>>,
    }

    impl Scripted {
        pub(crate) fn new(rules: Vec<(&str, CommandOutput)>) -> Self {
            Self {
                rules: rules.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
                calls: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn calls(&self) -> Vec<(Vec<String>, Duration)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for Scripted {
        fn run(&self, argv: &[String], timeout: Duration) -> CommandOutput {
            self.calls.lock().unwrap().push((argv.to_vec(), timeout));
            let line = argv.join(" ");
            self.rules
                .iter()
                .find(|(key, _)| line.contains(key.as_str()))
                .map_or_else(
                    || CommandOutput::not_started(format!("no rule for: {line}")),
                    |(_, out)| out.clone(),
                )
        }
    }
}

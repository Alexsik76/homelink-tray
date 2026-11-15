//! The lookup of the external programs (the capability spec, `core::capabilities`): when it
//! runs, and where its result goes. It runs at start, after the settings are saved and on
//! "Перевірити знову"; there is no polling.
//!
//! The result goes to the engine (which then neither runs nor offers what lacks a program) and
//! to the command runner (which then starts each found tool by its full path). A change of a
//! tool's status is logged once at `info`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use homelink_core::adapters::command::StdCommandRunner;
use homelink_core::capabilities::{self, Capabilities, Env, Tool};
use homelink_core::config::Config;
use homelink_core::domain::{PowerMethod, TunnelKind, WakeMethod};
use homelink_core::engine::Handle;

/// The tunnels' `wireguard_exe` and the programs of the user's commands in `config`.
pub(crate) fn config_paths(config: &Config) -> (Vec<PathBuf>, Vec<String>) {
    let wireguard = config
        .tunnels
        .iter()
        .filter_map(|t| match &t.kind {
            TunnelKind::WireGuardService { exe, .. } => Some(PathBuf::from(exe)),
            // Other kinds of tunnel (none yet) have no WireGuard client.
            _ => None,
        })
        .collect();
    let mut programs: Vec<String> = Vec::new();
    let mut add = |argv: &homelink_core::domain::Argv| {
        if let Some(program) = argv.items().first()
            && !programs.contains(program)
        {
            programs.push(program.clone());
        }
    };
    for pc in &config.pcs {
        if let Some(WakeMethod::Command { argv, .. }) = &pc.wake {
            add(argv);
        }
        if let Some(PowerMethod::Command { sleep, shutdown }) = &pc.power {
            for argv in [sleep, shutdown].into_iter().flatten() {
                add(argv);
            }
        }
    }
    (wireguard, programs)
}

/// Runs the lookup and hands its result on.
pub(crate) struct Checker {
    /// The real runner; `None` in demo mode, where every program counts as found.
    runner: Option<Arc<StdCommandRunner>>,
    handle: Handle,
    last: Mutex<Option<Capabilities>>,
}

impl Checker {
    /// A checker for the real mode.
    pub(crate) fn real(runner: Arc<StdCommandRunner>, handle: Handle) -> Self {
        Self {
            runner: Some(runner),
            handle,
            last: Mutex::new(None),
        }
    }

    /// A checker for the demo mode: every program is found.
    pub(crate) fn demo(handle: Handle) -> Self {
        Self {
            runner: None,
            handle,
            last: Mutex::new(None),
        }
    }

    /// Looks the programs of `config` up, tells the runner and the engine, and returns the
    /// result.
    pub(crate) fn check(&self, config: &Config) -> Capabilities {
        let caps = match &self.runner {
            Some(runner) => {
                let (wireguard, programs) = config_paths(config);
                let env = Env::from_process(wireguard, programs);
                let caps = capabilities::detect(&env, &|p: &Path| p.is_file());
                runner.set_programs(caps.program_paths());
                caps
            }
            None => Capabilities::all_found(),
        };
        if let Ok(mut last) = self.last.lock() {
            log_changes(last.as_ref(), &caps);
            *last = Some(caps.clone());
        }
        self.handle.set_capabilities(caps.clone());
        caps
    }
}

/// Logs every tool whose status differs from the previous lookup (all of them the first time),
/// and the user programs that are missing.
fn log_changes(before: Option<&Capabilities>, now: &Capabilities) {
    for tool in Tool::ALL {
        let status = now.status(tool);
        if before.map(|b| b.status(tool)) == Some(status) {
            continue;
        }
        match status.path() {
            Some(path) => log::info!("program {}: {}", tool.program(), path.display()),
            None => log::info!("program {}: not found", tool.program()),
        }
    }
    let missing: Vec<&str> = now.missing_programs().collect();
    let was: Vec<&str> = before
        .map(|b| b.missing_programs().collect())
        .unwrap_or_default();
    if missing != was && !missing.is_empty() {
        log::info!("programs of commands not found: {}", missing.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use homelink_core::testing::multi_config;

    #[test]
    fn the_settings_give_the_wireguard_paths_and_the_command_programs() {
        let (wireguard, programs) = config_paths(&multi_config());
        assert_eq!(wireguard.len(), 2);
        assert!(
            wireguard
                .iter()
                .all(|p| p.to_string_lossy().ends_with("wireguard.exe")),
            "{wireguard:?}"
        );
        // laptop shuts down by an ssh command, office-pc wakes by a wakeonlan command.
        assert_eq!(programs, ["ssh", "wakeonlan"]);
    }
}

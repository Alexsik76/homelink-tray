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

/// A tunnel imported into the WireGuard client: a file of its `Data\\Configurations` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportedTunnel {
    /// The tunnel name WireGuard derives from the file name.
    pub(crate) name: String,
    /// The file.
    pub(crate) path: PathBuf,
}

/// The tunnels imported into the WireGuard client whose `wireguard.exe` is `exe` (or that of
/// `%ProgramFiles%\\WireGuard` when it was not found), sorted by name. Only the file names are
/// read: the files hold the tunnels' private keys and are never opened.
pub(crate) fn imported_tunnels(exe: Option<&Path>) -> Vec<ImportedTunnel> {
    let base = exe
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("WireGuard")));
    let Some(dir) = base.map(|b| b.join("Data").join("Configurations")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        log::info!("cannot list {}", dir.display());
        return Vec::new();
    };
    let mut tunnels: Vec<ImportedTunnel> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let path = e.path();
            let name = TunnelKind::wireguard_name(&path.to_string_lossy()).ok()?;
            Some(ImportedTunnel { name, path })
        })
        .collect();
    tunnels.sort_by_key(|t| t.name.to_lowercase());
    tunnels.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    tunnels
}

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
    runner: Arc<StdCommandRunner>,
    handle: Handle,
    last: Mutex<Option<Capabilities>>,
    /// The local time of the last lookup, `HH:MM`.
    checked_at: Mutex<Option<String>>,
}

impl Checker {
    /// A checker for the real mode.
    pub(crate) fn real(runner: Arc<StdCommandRunner>, handle: Handle) -> Self {
        Self {
            runner,
            handle,
            last: Mutex::new(None),
            checked_at: Mutex::new(None),
        }
    }

    /// Looks the programs of `config` up, tells the runner and the engine, and returns the
    /// result.
    pub(crate) fn check(&self, config: &Config) -> Capabilities {
        let (wireguard, programs) = config_paths(config);
        let env = Env::from_process(wireguard, programs);
        let caps = capabilities::detect(&env, &|p: &Path| p.is_file());
        self.runner.set_programs(caps.program_paths());
        if let Ok(mut last) = self.last.lock() {
            log_changes(last.as_ref(), &caps);
            *last = Some(caps.clone());
        }
        if let Ok(mut at) = self.checked_at.lock() {
            *at = Some(local_time());
        }
        self.handle.set_capabilities(caps.clone());
        caps
    }

    /// The local time of the last lookup, `HH:MM`.
    pub(crate) fn checked_at(&self) -> Option<String> {
        self.checked_at.lock().ok().and_then(|at| at.clone())
    }
}

/// The local time now as `HH:MM` (UTC where the time zone is unknown).
fn local_time() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let stamp = crate::logging::format_local(secs, crate::platform::utc_offset_minutes());
    stamp.get(11..16).unwrap_or_default().to_owned()
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

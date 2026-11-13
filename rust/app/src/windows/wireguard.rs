//! A WireGuard tunnel as a Windows service of the official client, the same mechanism as
//! `tunnel.py`:
//!
//! - **Up:** `wireguard.exe /installtunnelservice <conf>` installs and starts the service
//!   `WireGuardTunnel$<name>`; "already installed" is not an error.
//! - **Down:** `wireguard.exe /uninstalltunnelservice <name>`; the result is only logged.
//! - **State:** `sc query WireGuardTunnel$<name>`. `psutil` is not used. The state is the
//!   number and name that `sc` prints for it (`4  RUNNING`), found on whichever line has that
//!   form, so the localised labels in front of it (`СОСТОЯНИЕ`, `STATE`) do not matter.
//!
//! The programs run through the core's `CommandRunner`, whose output is decoded from the OEM
//! code page by the app.

use std::sync::Arc;
use std::time::Duration;

use homelink_core::domain::{Tunnel, TunnelKind};
use homelink_core::ports::{
    CommandOutcome, CommandOutput, CommandRunner, TunnelDriver, TunnelError,
};

/// The prefix of the service name WireGuard gives a tunnel.
pub(crate) const SERVICE_PREFIX: &str = "WireGuardTunnel$";
/// How long `wireguard.exe` may take to install or remove the service.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `sc query` may take.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
/// The exit code of `sc` for a service that is not installed (`ERROR_SERVICE_DOES_NOT_EXIST`).
const SERVICE_DOES_NOT_EXIST: i32 = 1060;

/// The state of a Windows service, as the numbers of `SERVICE_STATUS` give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceState {
    /// `1 STOPPED`.
    Stopped,
    /// `2 START_PENDING`.
    StartPending,
    /// `3 STOP_PENDING`.
    StopPending,
    /// `4 RUNNING`.
    Running,
    /// `5 CONTINUE_PENDING`.
    ContinuePending,
    /// `6 PAUSE_PENDING`.
    PausePending,
    /// `7 PAUSED`.
    Paused,
}

impl ServiceState {
    fn from_number(number: u32) -> Option<Self> {
        Some(match number {
            1 => Self::Stopped,
            2 => Self::StartPending,
            3 => Self::StopPending,
            4 => Self::Running,
            5 => Self::ContinuePending,
            6 => Self::PausePending,
            7 => Self::Paused,
            _ => return None,
        })
    }

    /// The name `sc` prints after the number.
    fn name(self) -> &'static str {
        match self {
            Self::Stopped => "STOPPED",
            Self::StartPending => "START_PENDING",
            Self::StopPending => "STOP_PENDING",
            Self::Running => "RUNNING",
            Self::ContinuePending => "CONTINUE_PENDING",
            Self::PausePending => "PAUSE_PENDING",
            Self::Paused => "PAUSED",
        }
    }
}

/// The service name of a tunnel.
pub(crate) fn service_name(tunnel_name: &str) -> String {
    format!("{SERVICE_PREFIX}{tunnel_name}")
}

/// The command that prints the state of the service of a tunnel.
pub(crate) fn query_argv(tunnel_name: &str) -> Vec<String> {
    vec!["sc.exe".into(), "query".into(), service_name(tunnel_name)]
}

/// The command that installs and starts the service of a tunnel.
pub(crate) fn install_argv(exe: &str, conf: &str) -> Vec<String> {
    vec![exe.into(), "/installtunnelservice".into(), conf.into()]
}

/// The command that stops and removes the service of a tunnel.
pub(crate) fn uninstall_argv(exe: &str, tunnel_name: &str) -> Vec<String> {
    vec![
        exe.into(),
        "/uninstalltunnelservice".into(),
        tunnel_name.into(),
    ]
}

/// Finds the service state in the text `sc query` printed.
///
/// A line counts when, after its first colon, it starts with a number from 1 to 7 followed by
/// the name that belongs to that number (`4  RUNNING`). Other lines that start with a number
/// (`TYPE : 10  WIN32_OWN_PROCESS`, `WIN32_EXIT_CODE : 1  (0x1)`) never match, in any display
/// language.
pub(crate) fn parse_service_state(text: &str) -> Option<ServiceState> {
    text.lines().find_map(|line| {
        let (_, value) = line.split_once(':')?;
        let mut words = value.split_whitespace();
        let number: u32 = words.next()?.parse().ok()?;
        let state = ServiceState::from_number(number)?;
        let name = words.next()?;
        name.eq_ignore_ascii_case(state.name()).then_some(state)
    })
}

/// What the output of `sc query` says about a tunnel service: `Ok(true)` when it runs.
///
/// A service that is not installed is simply not running. Anything else that is not a normal
/// answer is an error, so that a format this code does not know is visible and not mistaken
/// for a stopped tunnel.
pub(crate) fn interpret_query(output: &CommandOutput) -> Result<bool, String> {
    match output.outcome {
        CommandOutcome::Exited(SERVICE_DOES_NOT_EXIST) => Ok(false),
        CommandOutcome::Exited(0) => match parse_service_state(&output.text) {
            Some(state) => Ok(state == ServiceState::Running),
            None => Err(format!(
                "cannot read the service state from the output of sc: {}",
                output.text
            )),
        },
        CommandOutcome::Exited(code) => {
            Err(format!("sc failed (exit code {code}): {}", output.text))
        }
        CommandOutcome::TimedOut | CommandOutcome::NotStarted => {
            Err(format!("cannot run sc: {}", output.text))
        }
    }
}

/// Whether the installation of the service worked: `wireguard.exe` exited with 0, or said the
/// service is already installed (as `tunnel.py` accepted it).
pub(crate) fn interpret_install(output: &CommandOutput) -> Result<(), String> {
    if output.success() || output.text.to_lowercase().contains("already installed") {
        Ok(())
    } else {
        Err(output.text.clone())
    }
}

/// Whether the removal of the service worked; `Err` carries what to log.
pub(crate) fn interpret_uninstall(output: &CommandOutput) -> Result<(), String> {
    if output.success() {
        Ok(())
    } else {
        Err(output.text.clone())
    }
}

/// The executable, the configuration file and the name of a tunnel.
fn parts(tunnel: &Tunnel) -> Result<(&str, &str, String), TunnelError> {
    match &tunnel.kind {
        TunnelKind::WireGuardService { exe, conf } => {
            let name =
                TunnelKind::wireguard_name(conf).map_err(|err| TunnelError(err.to_string()))?;
            Ok((exe, conf, name))
        }
        _ => Err(TunnelError("this kind of tunnel is not supported".into())),
    }
}

/// Controls WireGuard tunnels through `wireguard.exe` and `sc`.
pub(crate) struct WireGuardDriver {
    runner: Arc<dyn CommandRunner>,
}

impl WireGuardDriver {
    /// A driver that runs its programs through `runner`.
    pub(crate) fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

impl TunnelDriver for WireGuardDriver {
    fn is_running(&self, tunnel: &Tunnel) -> Result<bool, TunnelError> {
        let (_, _, name) = parts(tunnel)?;
        let output = self.runner.run(&query_argv(&name), QUERY_TIMEOUT);
        interpret_query(&output).map_err(TunnelError)
    }

    fn up(&self, tunnel: &Tunnel) -> Result<(), TunnelError> {
        // As in `tunnel.py`, a state that cannot be read counts as "not running": installing a
        // service that is already there is accepted below, whereas giving up here would make
        // the tunnel impossible to start on a system whose `sc` output is not understood.
        match self.is_running(tunnel) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(err) => log::warn!("{err}; installing the tunnel service anyway"),
        }
        let (exe, conf, name) = parts(tunnel)?;
        let output = self.runner.run(&install_argv(exe, conf), COMMAND_TIMEOUT);
        log::info!(
            "install service {}: {:?} {}",
            service_name(&name),
            output.outcome,
            output.text
        );
        interpret_install(&output).map_err(TunnelError)
    }

    fn down(&self, tunnel: &Tunnel) -> Result<(), TunnelError> {
        let (exe, _, name) = parts(tunnel)?;
        let output = self
            .runner
            .run(&uninstall_argv(exe, &name), COMMAND_TIMEOUT);
        log::info!(
            "uninstall service {}: {:?} {}",
            service_name(&name),
            output.outcome,
            output.text
        );
        interpret_uninstall(&output).map_err(TunnelError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::oem;
    use crate::windows::scripted::Scripted;
    use homelink_core::testing::single_config;

    /// The output of `sc query` for a running service, in English.
    const RUNNING_EN: &str = "\r\nSERVICE_NAME: WireGuardTunnel$home\r\n        TYPE               : 10  WIN32_OWN_PROCESS\r\n        STATE              : 4  RUNNING\r\n                                (STOPPABLE, NOT_PAUSABLE, ACCEPTS_SHUTDOWN)\r\n        WIN32_EXIT_CODE    : 0  (0x0)\r\n        SERVICE_EXIT_CODE  : 0  (0x0)\r\n        CHECKPOINT         : 0x0\r\n        WAIT_HINT          : 0x0\r\n";

    const STOPPED_EN: &str = "\r\nSERVICE_NAME: WireGuardTunnel$home\r\n        TYPE               : 10  WIN32_OWN_PROCESS\r\n        STATE              : 1  STOPPED\r\n                                (NOT_STOPPABLE, NOT_PAUSABLE, IGNORES_SHUTDOWN)\r\n        WIN32_EXIT_CODE    : 1  (0x1)\r\n        SERVICE_EXIT_CODE  : 0  (0x0)\r\n        CHECKPOINT         : 0x0\r\n        WAIT_HINT          : 0x0\r\n";

    const STARTING_EN: &str = "SERVICE_NAME: WireGuardTunnel$home\r\n        TYPE               : 10  WIN32_OWN_PROCESS\r\n        STATE              : 2  START_PENDING\r\n                                (NOT_STOPPABLE, NOT_PAUSABLE, IGNORES_SHUTDOWN)\r\n        WIN32_EXIT_CODE    : 0  (0x0)\r\n";

    const NOT_INSTALLED_EN: &str = "[SC] EnumQueryServicesStatus:OpenService FAILED 1060:\r\n\r\nThe specified service does not exist as an installed service.\r\n";

    /// The same output as the console of a Russian or Ukrainian Windows writes it: the labels
    /// are translated, the numbers and the state names are not. As bytes of code page 866.
    fn running_cp866() -> Vec<u8> {
        let mut bytes = Vec::new();
        // "ИМЯ_СЛУЖБЫ: WireGuardTunnel$home"
        bytes.extend_from_slice(
            b"\r\n\x88\x8C\x9F_\x91\x8B\x93\x86\x81\x9B: WireGuardTunnel$home\r\n",
        );
        // "        ТИП                : 10  WIN32_OWN_PROCESS"
        bytes.extend_from_slice(b"        \x92\x88\x8F                : 10  WIN32_OWN_PROCESS\r\n");
        // "        СОСТОЯНИЕ          : 4  RUNNING"
        bytes.extend_from_slice(
            b"        \x91\x8E\x91\x92\x8E\x9F\x8D\x88\x85          : 4  RUNNING\r\n",
        );
        bytes.extend_from_slice(
            b"                                (STOPPABLE, NOT_PAUSABLE, ACCEPTS_SHUTDOWN)\r\n",
        );
        // "        КОД_ВОЗВРАТА_WIN32 : 0  (0x0)"
        bytes.extend_from_slice(
            b"        \x8A\x8E\x84_\x82\x8E\x87\x82\x90\x80\x92\x80_WIN32 : 0  (0x0)\r\n",
        );
        bytes.extend_from_slice(b"        CHECKPOINT         : 0x0\r\n");
        bytes
    }

    fn decoded(bytes: &[u8], page: u16) -> String {
        oem::decode(Some(page), bytes)
    }

    #[test]
    fn names_and_command_lines() {
        assert_eq!(service_name("home"), "WireGuardTunnel$home");
        assert_eq!(
            query_argv("home"),
            ["sc.exe", "query", "WireGuardTunnel$home"]
        );
        assert_eq!(
            install_argv(
                r"C:\WG\wireguard.exe",
                r"C:\WG\Data\Configurations\home.conf.dpapi"
            ),
            [
                r"C:\WG\wireguard.exe",
                "/installtunnelservice",
                r"C:\WG\Data\Configurations\home.conf.dpapi"
            ]
        );
        assert_eq!(
            uninstall_argv(r"C:\WG\wireguard.exe", "home"),
            [r"C:\WG\wireguard.exe", "/uninstalltunnelservice", "home"]
        );
    }

    #[test]
    fn the_state_is_read_from_english_output() {
        assert_eq!(parse_service_state(RUNNING_EN), Some(ServiceState::Running));
        assert_eq!(parse_service_state(STOPPED_EN), Some(ServiceState::Stopped));
        assert_eq!(
            parse_service_state(STARTING_EN),
            Some(ServiceState::StartPending)
        );
    }

    #[test]
    fn the_state_is_read_from_cyrillic_output_in_code_page_866() {
        let text = decoded(&running_cp866(), 866);
        // The decoder produced the translated labels, and the parser ignores them.
        assert!(text.contains("СОСТОЯНИЕ"), "{text}");
        assert!(text.contains("ИМЯ_СЛУЖБЫ"), "{text}");
        assert!(text.contains("КОД_ВОЗВРАТА_WIN32"), "{text}");
        assert_eq!(parse_service_state(&text), Some(ServiceState::Running));
        // Decoded as ASCII only (an unsupported page), the state line still has its number
        // and name, so the answer does not depend on the table either.
        assert_eq!(
            parse_service_state(&oem::decode(None, &running_cp866())),
            Some(ServiceState::Running)
        );
    }

    #[test]
    fn lines_that_only_look_like_a_state_do_not_count() {
        // An exit code of 1 and a type of 10 are numbers too.
        let text = "        TYPE : 10  WIN32_OWN_PROCESS\n        WIN32_EXIT_CODE : 1  (0x1)\n";
        assert_eq!(parse_service_state(text), None);
        // A number from 1 to 7 with the wrong name is not a state.
        assert_eq!(parse_service_state("STATE : 4  STOPPED"), None);
        assert_eq!(parse_service_state(""), None);
        assert_eq!(parse_service_state("STATE : x  RUNNING"), None);
    }

    #[test]
    fn running_means_running_and_everything_else_means_not() {
        assert_eq!(
            interpret_query(&CommandOutput::exited(0, RUNNING_EN)),
            Ok(true)
        );
        assert_eq!(
            interpret_query(&CommandOutput::exited(0, STOPPED_EN)),
            Ok(false)
        );
        assert_eq!(
            interpret_query(&CommandOutput::exited(0, STARTING_EN)),
            Ok(false)
        );
    }

    #[test]
    fn a_missing_service_is_not_running() {
        assert_eq!(
            interpret_query(&CommandOutput::exited(1060, NOT_INSTALLED_EN)),
            Ok(false)
        );
    }

    #[test]
    fn an_answer_that_cannot_be_read_is_an_error_and_not_a_stopped_tunnel() {
        let unknown = interpret_query(&CommandOutput::exited(0, "something else"));
        assert!(
            unknown
                .unwrap_err()
                .contains("cannot read the service state")
        );
        assert!(
            interpret_query(&CommandOutput::exited(5, "Access is denied."))
                .unwrap_err()
                .contains("exit code 5")
        );
        assert!(
            interpret_query(&CommandOutput::not_started("sc.exe: not found"))
                .unwrap_err()
                .contains("cannot run sc")
        );
    }

    #[test]
    fn installing_accepts_an_installed_service() {
        assert_eq!(interpret_install(&CommandOutput::exited(0, "")), Ok(()));
        let already = CommandOutput::exited(
            1,
            "Service already installed and running: The service WireGuardTunnel$home is already installed",
        );
        assert_eq!(interpret_install(&already), Ok(()));
        let failed = CommandOutput::exited(1, "Unable to read the configuration");
        assert_eq!(
            interpret_install(&failed),
            Err("Unable to read the configuration".into())
        );
    }

    #[test]
    fn removal_reports_its_failure_for_the_log() {
        assert_eq!(interpret_uninstall(&CommandOutput::exited(0, "")), Ok(()));
        assert_eq!(
            interpret_uninstall(&CommandOutput::exited(1, "not installed")),
            Err("not installed".into())
        );
    }

    fn driver(rules: Vec<(&str, CommandOutput)>) -> (WireGuardDriver, Arc<Scripted>) {
        let runner = Arc::new(Scripted::new(rules));
        (WireGuardDriver::new(runner.clone()), runner)
    }

    fn home() -> Tunnel {
        single_config().tunnels.remove(0)
    }

    #[test]
    fn the_driver_queries_the_service_of_the_tunnel() {
        let (driver, runner) = driver(vec![("query", CommandOutput::exited(0, RUNNING_EN))]);
        assert_eq!(driver.is_running(&home()), Ok(true));
        let calls = runner.calls();
        assert_eq!(
            calls.first().map(|(argv, _)| argv.clone()),
            Some(vec![
                "sc.exe".into(),
                "query".into(),
                "WireGuardTunnel$home".into()
            ])
        );
    }

    #[test]
    fn starting_a_running_tunnel_does_nothing() {
        let (driver, runner) = driver(vec![("query", CommandOutput::exited(0, RUNNING_EN))]);
        assert_eq!(driver.up(&home()), Ok(()));
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn starting_a_stopped_tunnel_installs_the_service() {
        let (driver, runner) = driver(vec![
            ("query", CommandOutput::exited(1060, NOT_INSTALLED_EN)),
            ("/installtunnelservice", CommandOutput::exited(0, "")),
        ]);
        assert_eq!(driver.up(&home()), Ok(()));
        let calls = runner.calls();
        let (argv, timeout) = calls.get(1).unwrap();
        assert_eq!(
            argv.first().map(String::as_str),
            Some(r"C:\WG\wireguard.exe")
        );
        assert_eq!(
            argv.get(1).map(String::as_str),
            Some("/installtunnelservice")
        );
        assert_eq!(
            argv.get(2).map(String::as_str),
            Some(r"C:\WG\home.conf.dpapi")
        );
        assert_eq!(*timeout, COMMAND_TIMEOUT);
    }

    #[test]
    fn a_state_that_cannot_be_read_does_not_stop_the_tunnel_from_starting() {
        // `sc` output in a format this code does not know, then a successful install.
        let (driver, runner) = driver(vec![
            ("query", CommandOutput::exited(0, "unexpected output")),
            ("/installtunnelservice", CommandOutput::exited(0, "")),
        ]);
        assert!(driver.is_running(&home()).is_err());
        assert_eq!(driver.up(&home()), Ok(()));
        let installs = runner
            .calls()
            .iter()
            .filter(|(argv, _)| argv.iter().any(|a| a == "/installtunnelservice"))
            .count();
        assert_eq!(installs, 1);
        // Even without `sc.exe` at all.
        let (driver, _) = driver_without_sc();
        assert_eq!(driver.up(&home()), Ok(()));
    }

    fn driver_without_sc() -> (WireGuardDriver, Arc<Scripted>) {
        driver(vec![(
            "/installtunnelservice",
            CommandOutput::exited(0, ""),
        )])
    }

    #[test]
    fn a_refused_installation_is_an_error_with_the_programs_text() {
        let (driver, _) = driver(vec![
            ("query", CommandOutput::exited(1060, NOT_INSTALLED_EN)),
            (
                "/installtunnelservice",
                CommandOutput::exited(1, "Access is denied."),
            ),
        ]);
        assert_eq!(
            driver.up(&home()),
            Err(TunnelError("Access is denied.".into()))
        );
    }

    #[test]
    fn stopping_uninstalls_the_service_by_name() {
        let (driver, runner) = driver(vec![(
            "/uninstalltunnelservice",
            CommandOutput::exited(0, ""),
        )]);
        assert_eq!(driver.down(&home()), Ok(()));
        let calls = runner.calls();
        let (argv, _) = calls.first().unwrap();
        assert_eq!(
            argv.get(1).map(String::as_str),
            Some("/uninstalltunnelservice")
        );
        assert_eq!(argv.get(2).map(String::as_str), Some("home"));
    }
}

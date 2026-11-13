//! Step 5 of the wizard: checks of a PC before it is saved.
//!
//! [`plan`] decides, from the (unsaved) PC, which checks make sense; it is pure. [`run_all`]
//! and [`run_check`] execute them through the ports ([`PortProbe`], [`CommandRunner`]); they
//! block (TCP connects, `ssh` runs), so the UI calls them on a worker thread and may run the
//! checks of a plan one by one to fill the list row by row. Each yields a [`CheckRow`]: a
//! [`DisplayState`] for the marker and a [`CheckResult`] that
//! [`crate::i18n::Translator::check_result`] words in either language.
//!
//! - **Readiness ports** (always): every port is tried on its own, so the result names the
//!   ports that are closed.
//! - **Relay SSH** (when the wake method is a relay): `ssh` to the relay with the options of
//!   `core::ssh` (`BatchMode=yes`, key only, `StrictHostKeyChecking=yes`), running a command
//!   that changes nothing ([`relay_check_command`]).
//! - **Power SSH** (when sleep/shutdown are over SSH): the same to the PC ([`POWER_CHECK_COMMAND`]).
//!
//! A host key that is not known yet is reported ([`CheckResult::SshHostKeyUnknown`]); nothing
//! here ever accepts it. Commands (wake and power by command) are not run: they have effects.
//!
//! The PC's checks need its network, so for a PC behind a tunnel the caller says whether the
//! tunnel is up (from the engine snapshot); if it is not, every row is
//! [`CheckResult::TunnelDown`] and nothing is run.

use std::time::Duration;

use crate::config::Config;
use crate::domain::{Host, Pc, PowerMethod, Relay, RelayKind, WakeMethod};
use crate::notice::DETAIL_LIMIT;
use crate::ports::{CommandOutcome, CommandOutput, CommandRunner, PortProbe};
use crate::snapshot::DisplayState;
use crate::ssh::{SSH_OWN_ERROR, SshTarget, ssh_argv};
use crate::wake::command_timeout;
use crate::wizard::PcOutcome;

/// The remote command that tests an SSH login to a PC: it only exits with code 0, and works in
/// `sh`, `cmd.exe` and PowerShell alike.
pub const POWER_CHECK_COMMAND: &str = "exit 0";

/// The remote command that tests an SSH login to a relay of this preset; it changes nothing.
///
/// For `routeros` this is `:put ok`, which prints a word and needs no policy beyond the login.
/// Not yet verified on RouterOS 7 (see the roadmap); it is one place to change.
pub fn relay_check_command(kind: RelayKind) -> &'static str {
    match kind {
        RelayKind::RouterOs => ":put ok",
        RelayKind::Etherwake | RelayKind::Wakeonlan => "exit 0",
    }
}

/// What a check looks at (the check name in the list: `check.name.<as_str>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// The readiness ports of the PC.
    Ports,
    /// The SSH login to the relay host.
    RelaySsh,
    /// The SSH login to the PC for sleep and shutdown.
    PowerSsh,
}

impl CheckKind {
    /// The locale key of the check name.
    pub fn name_key(self) -> &'static str {
        match self {
            Self::Ports => "check.name.ports",
            Self::RelaySsh => "check.name.relay_ssh",
            Self::PowerSsh => "check.name.power_ssh",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Spec {
    Ports {
        host: Host,
        ports: Vec<u16>,
        timeout: Duration,
    },
    Ssh {
        argv: Vec<String>,
        timeout: Duration,
    },
}

/// One check to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What it looks at.
    pub kind: CheckKind,
    spec: Spec,
}

impl Check {
    /// The `ssh` command line of an SSH check; `None` for the ports check.
    pub fn ssh_command(&self) -> Option<&[String]> {
        match &self.spec {
            Spec::Ssh { argv, .. } => Some(argv),
            Spec::Ports { .. } => None,
        }
    }
}

/// The checks of one PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckPlan {
    /// The PC is behind a tunnel, which must be up for the checks to mean anything.
    pub needs_tunnel: bool,
    /// The checks, in the order of the list.
    pub checks: Vec<Check>,
}

impl CheckPlan {
    /// The kinds in list order, to draw the names (with an unknown or busy marker) before
    /// the results arrive.
    pub fn pending(&self) -> Vec<CheckKind> {
        self.checks.iter().map(|c| c.kind).collect()
    }
}

/// The checks for a PC that is about to be saved. `relay` is the relay of its wake method,
/// existing or new ([`PcOutcome::new_relay`]).
pub fn plan_for(pc: &Pc, relay: Option<&Relay>) -> CheckPlan {
    let mut checks = vec![Check {
        kind: CheckKind::Ports,
        spec: Spec::Ports {
            host: pc.host.clone(),
            ports: pc.ports.clone(),
            timeout: pc.timing.port_timeout,
        },
    }];
    if let (Some(WakeMethod::Relay { .. }), Some(relay)) = (&pc.wake, relay) {
        let target = SshTarget {
            host: &relay.host,
            port: relay.port,
            user: &relay.user,
            identity_file: &relay.identity_file,
            timeout: relay.timeout,
        };
        checks.push(Check {
            kind: CheckKind::RelaySsh,
            spec: Spec::Ssh {
                argv: ssh_argv(&target, relay_check_command(relay.kind)),
                timeout: relay.timeout,
            },
        });
    }
    if let Some(PowerMethod::Ssh {
        user,
        port,
        identity_file,
    }) = &pc.power
    {
        let target = SshTarget {
            host: &pc.host,
            port: *port,
            user,
            identity_file,
            timeout: pc.timing.power_timeout,
        };
        checks.push(Check {
            kind: CheckKind::PowerSsh,
            spec: Spec::Ssh {
                argv: ssh_argv(&target, POWER_CHECK_COMMAND),
                timeout: pc.timing.power_timeout,
            },
        });
    }
    CheckPlan {
        needs_tunnel: pc.tunnel.is_some(),
        checks,
    }
}

/// The checks for the result of a draft ([`crate::wizard::PcDraft::build`]); the relay is the
/// new one of the outcome or the existing one named by the wake method.
pub fn plan(config: &Config, outcome: &PcOutcome) -> CheckPlan {
    let existing = match &outcome.pc.wake {
        Some(WakeMethod::Relay { relay, .. }) => config.relay(relay),
        _ => None,
    };
    plan_for(&outcome.pc, outcome.new_relay.as_ref().or(existing))
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// All the ports answer.
    PortsOpen(Vec<u16>),
    /// These ports do not answer.
    PortsClosed(Vec<u16>),
    /// `ssh` connected and the command ran.
    SshOk,
    /// The host key is not known yet (verified, not accepted).
    SshHostKeyUnknown,
    /// The host key differs from the known one.
    SshHostKeyChanged,
    /// The key was refused.
    SshAuthFailed,
    /// `ssh` could not connect (refused, unreachable, ...); `detail` is what it printed.
    SshRefused {
        /// The text `ssh` wrote, shortened.
        detail: String,
    },
    /// The OpenSSH client could not be started.
    SshNotInstalled,
    /// `ssh` did not finish in time.
    SshTimedOut,
    /// `ssh` connected and the command failed on the remote host.
    SshRemoteFailed {
        /// The exit code.
        code: i32,
        /// What was printed, shortened.
        detail: String,
    },
    /// The tunnel is down, so nothing was checked.
    TunnelDown,
}

impl CheckResult {
    /// Whether the check passed.
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::PortsOpen(_) | Self::SshOk)
    }

    /// The marker: ok, fail, or unknown when nothing was checked.
    pub fn display(&self) -> DisplayState {
        match self {
            Self::PortsOpen(_) | Self::SshOk => DisplayState::Ok,
            Self::TunnelDown => DisplayState::Unknown,
            _ => DisplayState::Fail,
        }
    }
}

/// A finished check as a row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRow {
    /// What was checked (the name: `CheckKind::name_key`).
    pub kind: CheckKind,
    /// The marker.
    pub display: DisplayState,
    /// The result text: `Translator::check_result`.
    pub result: CheckResult,
}

/// What the checks use.
#[derive(Clone, Copy)]
pub struct CheckDeps<'a> {
    /// TCP probe.
    pub ports: &'a dyn PortProbe,
    /// Runs `ssh`.
    pub runner: &'a dyn CommandRunner,
}

fn shorten(text: &str) -> String {
    text.trim().chars().take(DETAIL_LIMIT).collect()
}

/// Maps the end of an `ssh` run to a result.
fn ssh_result(output: &CommandOutput) -> CheckResult {
    match output.outcome {
        CommandOutcome::Exited(0) => CheckResult::SshOk,
        CommandOutcome::NotStarted => CheckResult::SshNotInstalled,
        CommandOutcome::TimedOut => CheckResult::SshTimedOut,
        CommandOutcome::Exited(SSH_OWN_ERROR) => {
            let text = &output.text;
            if text.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
                CheckResult::SshHostKeyChanged
            } else if text.contains("Host key verification failed") {
                CheckResult::SshHostKeyUnknown
            } else if text.contains("Permission denied") {
                CheckResult::SshAuthFailed
            } else {
                CheckResult::SshRefused {
                    detail: shorten(text),
                }
            }
        }
        CommandOutcome::Exited(code) => CheckResult::SshRemoteFailed {
            code,
            detail: shorten(&output.text),
        },
    }
}

/// Runs one check. `tunnel_up` is whether the PC's tunnel is up (ignored when the plan does
/// not need one); when it is needed and down, nothing is run.
pub fn run_check(
    check: &Check,
    needs_tunnel: bool,
    tunnel_up: bool,
    deps: &CheckDeps<'_>,
) -> CheckRow {
    let result = if needs_tunnel && !tunnel_up {
        CheckResult::TunnelDown
    } else {
        match &check.spec {
            Spec::Ports {
                host,
                ports,
                timeout,
            } => {
                let closed: Vec<u16> = ports
                    .iter()
                    .copied()
                    .filter(|port| !deps.ports.all_open(host, &[*port], *timeout))
                    .collect();
                if closed.is_empty() {
                    CheckResult::PortsOpen(ports.clone())
                } else {
                    CheckResult::PortsClosed(closed)
                }
            }
            Spec::Ssh { argv, timeout } => {
                ssh_result(&deps.runner.run(argv, command_timeout(*timeout)))
            }
        }
    };
    CheckRow {
        kind: check.kind,
        display: result.display(),
        result,
    }
}

/// Runs every check of the plan, in order. Blocking.
pub fn run_all(plan: &CheckPlan, tunnel_up: bool, deps: &CheckDeps<'_>) -> Vec<CheckRow> {
    plan.checks
        .iter()
        .map(|check| run_check(check, plan.needs_tunnel, tunnel_up, deps))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::RelayId;
    use crate::testing::{FakeCommandRunner, FakeNetwork, multi_config, pc_id};
    use crate::wizard::{PcDraft, RelayChoice, WakeChoice};

    fn plan_of(id: &str) -> (Config, CheckPlan) {
        let config = multi_config();
        let pc = config.pc(&pc_id(id)).unwrap();
        let outcome = PcDraft::from_pc(&config, pc).build(&config).unwrap();
        let plan = plan(&config, &outcome);
        (config, plan)
    }

    #[test]
    fn the_plan_follows_the_wake_and_power_methods() {
        // desktop: relay wake, SSH power, behind a tunnel.
        let (_, p) = plan_of("desktop");
        assert_eq!(
            p.pending(),
            [CheckKind::Ports, CheckKind::RelaySsh, CheckKind::PowerSsh]
        );
        assert!(p.needs_tunnel);
        // laptop: magic packet, power by command: only the ports.
        let (_, p) = plan_of("laptop");
        assert_eq!(p.pending(), [CheckKind::Ports]);
        // nas: no tunnel, SSH power.
        let (_, p) = plan_of("nas");
        assert_eq!(p.pending(), [CheckKind::Ports, CheckKind::PowerSsh]);
        assert!(!p.needs_tunnel);
    }

    #[test]
    fn ssh_checks_use_the_strict_key_only_command_line() {
        let (config, p) = plan_of("desktop");
        let relay = p.checks[1].ssh_command().unwrap();
        assert!(relay.contains(&"BatchMode=yes".to_owned()));
        assert!(relay.contains(&"StrictHostKeyChecking=yes".to_owned()));
        assert!(relay.contains(&"IdentitiesOnly=yes".to_owned()));
        assert_eq!(relay.last().map(String::as_str), Some(":put ok"));
        let gateway = config.relay(&RelayId::parse("gateway").unwrap()).unwrap();
        assert!(relay.contains(&gateway.host.to_string()));
        let power = p.checks[2].ssh_command().unwrap();
        assert_eq!(power.last().map(String::as_str), Some("exit 0"));
        assert!(power.contains(&"192.168.88.10".to_owned()));
        assert!(p.checks[0].ssh_command().is_none());
    }

    #[test]
    fn a_new_relay_is_checked_before_it_is_saved() {
        let config = multi_config();
        let mut d = PcDraft::new(&config);
        d.name = "Fresh".into();
        d.host = "10.1.1.1".into();
        d.ports = "22".into();
        d.wake = WakeChoice::Relay;
        d.relay = RelayChoice::New;
        d.new_relay.kind = RelayKind::Etherwake;
        d.new_relay.sudo = Some(true);
        d.new_relay.host = "10.1.1.254".into();
        d.new_relay.user = "u".into();
        d.new_relay.identity_file = "/k".into();
        d.mac = "AA:BB:CC:DD:EE:FF".into();
        d.interface = "br0".into();
        let outcome = d.build(&config).unwrap();
        let p = plan(&config, &outcome);
        let argv = p.checks[1].ssh_command().unwrap();
        assert!(argv.contains(&"10.1.1.254".to_owned()));
        assert_eq!(argv.last().map(String::as_str), Some("exit 0"));
    }

    fn run(
        id: &str,
        tunnel_up: bool,
        net: &FakeNetwork,
        runner: &FakeCommandRunner,
    ) -> Vec<CheckRow> {
        let (_, p) = plan_of(id);
        run_all(&p, tunnel_up, &CheckDeps { ports: net, runner })
    }

    #[test]
    fn a_down_tunnel_is_a_clear_result_and_nothing_is_run() {
        let (net, runner) = (FakeNetwork::default(), FakeCommandRunner::default());
        let rows = run("desktop", false, &net, &runner);
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert_eq!(row.result, CheckResult::TunnelDown);
            assert_eq!(row.display, DisplayState::Unknown);
        }
        assert!(runner.calls().is_empty());
        // A PC without a tunnel does not care.
        net.set("10.0.0.7", true);
        let rows = run("nas", false, &net, &runner);
        assert!(rows.iter().all(|r| r.result.is_ok()), "{rows:?}");
    }

    #[test]
    fn results_follow_the_ports_and_the_ssh_outcome() {
        let (net, runner) = (FakeNetwork::default(), FakeCommandRunner::default());
        let rows = run("desktop", true, &net, &runner);
        assert_eq!(rows[0].result, CheckResult::PortsClosed(vec![3389]));
        assert_eq!(rows[0].display, DisplayState::Fail);
        assert_eq!(rows[1].result, CheckResult::SshOk);
        assert_eq!(rows[1].display, DisplayState::Ok);
        assert_eq!(runner.calls().len(), 2);
        net.set("192.168.88.10", true);
        assert_eq!(
            run("desktop", true, &net, &runner)[0].result,
            CheckResult::PortsOpen(vec![3389])
        );
    }

    #[test]
    fn ssh_failures_are_told_apart_and_a_host_key_is_never_accepted() {
        let cases = [
            (CommandOutput::exited(0, ""), CheckResult::SshOk),
            (
                CommandOutput::exited(
                    255,
                    "No ED25519 host key is known for x and you have requested strict checking.\nHost key verification failed.",
                ),
                CheckResult::SshHostKeyUnknown,
            ),
            (
                CommandOutput::exited(
                    255,
                    "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\nHost key verification failed.",
                ),
                CheckResult::SshHostKeyChanged,
            ),
            (
                CommandOutput::exited(255, "u@h: Permission denied (publickey)."),
                CheckResult::SshAuthFailed,
            ),
            (
                CommandOutput::exited(255, "connect: Connection refused"),
                CheckResult::SshRefused {
                    detail: "connect: Connection refused".into(),
                },
            ),
            (
                CommandOutput::exited(1, "nope"),
                CheckResult::SshRemoteFailed {
                    code: 1,
                    detail: "nope".into(),
                },
            ),
            (
                CommandOutput::not_started("no ssh"),
                CheckResult::SshNotInstalled,
            ),
            (
                CommandOutput::timed_out(Duration::from_secs(8), ""),
                CheckResult::SshTimedOut,
            ),
        ];
        for (output, expected) in cases {
            let (net, runner) = (FakeNetwork::default(), FakeCommandRunner::default());
            runner.set_result(output);
            let rows = run("desktop", true, &net, &runner);
            assert_eq!(rows[1].result, expected);
            assert_eq!(rows[2].result, expected);
            // Every run used StrictHostKeyChecking=yes, never accept-new.
            for (argv, _) in runner.calls() {
                assert!(argv.contains(&"StrictHostKeyChecking=yes".to_owned()));
                assert!(!argv.iter().any(|a| a.contains("accept-new")));
            }
        }
    }

    #[test]
    fn every_result_has_a_text_in_both_languages() {
        use crate::i18n::{Lang, Translator};
        let results = [
            CheckResult::PortsOpen(vec![22, 3389]),
            CheckResult::PortsClosed(vec![445]),
            CheckResult::SshOk,
            CheckResult::SshHostKeyUnknown,
            CheckResult::SshHostKeyChanged,
            CheckResult::SshAuthFailed,
            CheckResult::SshRefused { detail: "d".into() },
            CheckResult::SshNotInstalled,
            CheckResult::SshTimedOut,
            CheckResult::SshRemoteFailed {
                code: 2,
                detail: "d".into(),
            },
            CheckResult::TunnelDown,
        ];
        for lang in Lang::ALL {
            let tr = Translator::new(lang);
            for r in &results {
                let text = tr.check_result(r);
                assert!(!text.starts_with("check."), "{text}");
                assert!(!text.contains('{'), "{text}");
            }
            for kind in [CheckKind::Ports, CheckKind::RelaySsh, CheckKind::PowerSsh] {
                assert!(!tr.check_name(kind).starts_with("check."));
            }
        }
        let en = Translator::new(Lang::En);
        assert_eq!(
            en.check_result(&CheckResult::PortsOpen(vec![22, 3389])),
            "22, 3389 open"
        );
    }
}

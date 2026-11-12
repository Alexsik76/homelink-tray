//! Sleep and shutdown: over SSH with the preset commands of the PC's operating system, or by
//! a command from the configuration. Everything runs through a [`CommandRunner`].

use std::sync::Arc;

use crate::domain::{Pc, PowerAction, PowerMethod};
use crate::ports::{CommandRunner, PowerError, PowerRunner};
use crate::ssh::{explain_command, explain_ssh, power_ssh_argv};
use crate::wake::command_timeout;

/// The portable [`PowerRunner`].
pub struct CommandPowerRunner {
    runner: Arc<dyn CommandRunner>,
}

impl CommandPowerRunner {
    /// A runner that starts `ssh` and other commands through `runner`.
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

impl PowerRunner for CommandPowerRunner {
    fn run(&self, pc: &Pc, action: PowerAction) -> Result<(), PowerError> {
        match &pc.power {
            Some(PowerMethod::Ssh { .. }) => {
                let argv = power_ssh_argv(pc, action)
                    .ok_or_else(|| PowerError("no SSH command for this PC".into()))?;
                log::info!("{action:?} for {} over ssh", pc.id);
                let output = self
                    .runner
                    .run(&argv, command_timeout(pc.timing.power_timeout));
                explain_ssh(&output).map_or(Ok(()), |reason| Err(PowerError(reason)))
            }
            Some(PowerMethod::Command { sleep, shutdown }) => {
                let command = match action {
                    PowerAction::Sleep => sleep,
                    PowerAction::Shutdown => shutdown,
                };
                let Some(command) = command else {
                    return Err(PowerError(format!(
                        "no {action:?} command is configured for this PC"
                    )));
                };
                let argv = command.expand(&pc.host, None);
                log::info!("{action:?} for {} with a command", pc.id);
                let output = self.runner.run(&argv, pc.timing.power_timeout);
                explain_command(&output).map_or(Ok(()), |reason| Err(PowerError(reason)))
            }
            None => Err(PowerError("this PC has no power method".into())),
        }
    }
}

//! The add/edit PC wizard (section 8 of the handoff): five steps over one [`PcDraft`] of the core.
//!
//! The window keeps the values as its own properties (see `wizard.slint`); the draft is read from
//! them on "Далі" and "Готово" and written to them on open. "Далі" validates the step and stays
//! on it while there are problems; "Готово" builds the PC and saves it through the common path
//! ([`Manage::commit`]). Step 5 runs the checks of `core::checks` on a worker thread: a result
//! that arrives after the run was superseded (back, retest, close) is dropped.

use std::sync::Arc;

use homelink_core::checks::{self, CheckDeps, CheckRow};
use homelink_core::domain::{Os, PcId, RelayKind};
use homelink_core::problem::Problem;
use homelink_core::settings;
use homelink_core::wizard::{
    self, PcDraft, RelayChoice, RelayDraft, RelayOption, Step, TunnelOption, WakeChoice, field,
};
use slint::{ComponentHandle, SharedString};

use crate::manage::{Manage, model, place_problems, show_in_front, with_manage};
use crate::ui;
use crate::view::{CheckLine, Mark};

/// The number of error slots of the window (see `wizard.slint`).
const ERRORS: usize = 26;

/// The wizard while it is open: the step and the lists its indexes refer to.
pub(crate) struct Session {
    /// The draft the wizard started from: what it does not show is kept from it.
    base: PcDraft,
    step: Step,
    /// The tunnel choices of step 2, in the order of the window's labels.
    tunnels: Vec<TunnelOption>,
    /// The existing relays of step 3, in the order of the window's labels.
    relays: Vec<RelayOption>,
}

/// The index of a field in the window's errors (see `wizard.slint`).
fn slot(name: &str) -> Option<usize> {
    Some(match name {
        field::NAME => 0,
        field::HOST => 1,
        field::PORTS => 2,
        field::TUNNEL => 3,
        field::MAC => 4,
        field::MAGIC_ADDRESS => 5,
        field::MAGIC_PORT => 6,
        field::RELAY => 7,
        field::RELAY_HOST => 8,
        field::RELAY_PORT => 9,
        field::RELAY_USER => 10,
        field::RELAY_KEY => 11,
        field::RELAY_SUDO => 12,
        field::RELAY_TIMEOUT => 13,
        field::INTERFACE => 14,
        field::RELAY_ADDRESS => 15,
        field::WAKE_COMMAND => 16,
        field::WAKE_TIMEOUT => 17,
        field::ALLOW_SLEEP => 18,
        field::ALLOW_SHUTDOWN => 19,
        field::SSH_USER => 20,
        field::SSH_PORT => 21,
        field::SSH_KEY => 22,
        field::SLEEP_COMMAND => 23,
        field::SHUTDOWN_COMMAND => 24,
        field::RDP_USER => 25,
        _ => return None,
    })
}

/// Clears the relay fields that the chosen wake method or preset does not show, so that a value
/// typed before a change of preset is not sent on as a problem of a field nobody can see.
fn clear_hidden(draft: &mut PcDraft, s: &Session) {
    if draft.wake != WakeChoice::Relay {
        draft.interface.clear();
        draft.relay_address.clear();
        draft.new_relay.sudo = None;
        return;
    }
    let kind = match &draft.relay {
        RelayChoice::Existing(id) => {
            // An existing relay has no sudo question here: its own setting is kept as it is.
            draft.new_relay.sudo = None;
            s.relays
                .iter()
                .find(|option| option.id == *id)
                .map(|option| option.kind)
        }
        RelayChoice::New => Some(draft.new_relay.kind),
    };
    if let Some(kind) = kind {
        if !kind.uses_interface() {
            draft.interface.clear();
        }
        if !kind.uses_address() {
            draft.relay_address.clear();
        }
        if !kind.supports_sudo() {
            draft.new_relay.sudo = None;
        }
    }
}

/// The index of a relay preset in the window's radio list (RouterOS, etherwake, wakeonlan).
fn kind_index(kind: RelayKind) -> i32 {
    match kind {
        RelayKind::RouterOs => 0,
        RelayKind::Etherwake => 1,
        RelayKind::Wakeonlan => 2,
        // `RelayKind` is non-exhaustive: a preset this window does not know is shown as RouterOS.
        _ => 0,
    }
}

/// The preset of a radio index; anything unknown is RouterOS, the first.
fn kind_from_index(index: i32) -> RelayKind {
    match index {
        1 => RelayKind::Etherwake,
        2 => RelayKind::Wakeonlan,
        _ => RelayKind::RouterOs,
    }
}

/// A window index as a list position; a negative index matches nothing.
fn position(index: i32) -> usize {
    usize::try_from(index).unwrap_or(usize::MAX)
}

/// The window's index of a step (1 to 5), as the window shows it.
fn step_index(step: Step) -> i32 {
    i32::try_from(step.number()).unwrap_or(1)
}

impl Manage {
    /// Opens the wizard for a new PC, or brings it to the front when it is open.
    pub(crate) fn open_wizard_new(&self) {
        if self.session.borrow().is_some() {
            show_in_front(&self.wizard);
            return;
        }
        let draft = PcDraft::new(&self.config.borrow());
        let heading = self.tr.t("wizard.title_add", &[]);
        self.open_wizard(draft, heading);
    }

    /// Opens the wizard for the PC `id`, filled with what it has now.
    pub(crate) fn open_wizard_edit(&self, id: &str) {
        if self.session.borrow().is_some() {
            show_in_front(&self.wizard);
            return;
        }
        let Ok(pc_id) = PcId::parse(id) else {
            return;
        };
        let opened = {
            let config = self.config.borrow();
            config.pc(&pc_id).map(|pc| {
                (
                    PcDraft::from_pc(&config, pc),
                    self.tr.t("wizard.title_edit", &[("pc", pc.name.as_str())]),
                )
            })
        };
        if let Some((draft, heading)) = opened {
            self.open_wizard(draft, heading);
        }
    }

    fn open_wizard(&self, draft: PcDraft, heading: String) {
        let session = {
            let config = self.config.borrow();
            Session {
                base: draft.clone(),
                step: Step::Address,
                tunnels: wizard::tunnel_options(&config),
                relays: wizard::relay_options(&config),
            }
        };
        *self.session.borrow_mut() = Some(session);
        self.wizard.set_heading(heading.into());
        self.show_draft(&draft);
        self.set_step(Step::Address);
        show_in_front(&self.wizard);
    }

    /// Writes a draft to the window: every value, and the lists of the choices.
    fn show_draft(&self, draft: &PcDraft) {
        let session = self.session.borrow();
        let Some(s) = session.as_ref() else {
            return;
        };
        let w = &self.wizard;
        w.set_name(draft.name.clone().into());
        w.set_host(draft.host.clone().into());
        w.set_ports(draft.ports.clone().into());

        let none = self.tr.t("wizard.tunnel.none", &[]);
        let tunnel_labels: Vec<SharedString> = s
            .tunnels
            .iter()
            .map(|option| match &option.id {
                Some(_) => option.name.clone().into(),
                None => none.clone().into(),
            })
            .collect();
        w.set_tunnel_labels(model(tunnel_labels));
        let tunnel_index = s
            .tunnels
            .iter()
            .position(|option| option.id == draft.tunnel)
            .unwrap_or(0);
        w.set_tunnel_index(i32::try_from(tunnel_index).unwrap_or(0));

        let wake = match draft.wake {
            WakeChoice::None => 0,
            WakeChoice::Relay => 1,
            WakeChoice::MagicPacket => 2,
            WakeChoice::Command => 3,
        };
        w.set_wake_index(wake);
        w.set_mac(draft.mac.clone().into());
        w.set_magic_address(draft.magic_address.clone().into());
        w.set_magic_port(draft.magic_port.clone().into());

        let mut relay_labels: Vec<SharedString> = s
            .relays
            .iter()
            .map(|option| self.tr.relay_option(option).into())
            .collect();
        relay_labels.push(self.tr.t("wizard.relay.new", &[]).into());
        w.set_relay_labels(model(relay_labels));
        w.set_relay_kinds(model(
            s.relays
                .iter()
                .map(|option| kind_index(option.kind))
                .collect(),
        ));
        let relay_index = match &draft.relay {
            RelayChoice::Existing(id) => s
                .relays
                .iter()
                .position(|option| option.id == *id)
                .unwrap_or(s.relays.len()),
            RelayChoice::New => s.relays.len(),
        };
        w.set_relay_index(i32::try_from(relay_index).unwrap_or(0));
        w.set_relay_kind(kind_index(draft.new_relay.kind));
        w.set_new_host(draft.new_relay.host.clone().into());
        w.set_new_port(draft.new_relay.port.clone().into());
        w.set_new_user(draft.new_relay.user.clone().into());
        w.set_new_identity(draft.new_relay.identity_file.clone().into());
        w.set_relay_sudo(match draft.new_relay.sudo {
            Some(true) => 1,
            Some(false) => 2,
            None => 0,
        });
        w.set_new_timeout(draft.new_relay.timeout_seconds.clone().into());
        w.set_interface(draft.interface.clone().into());
        w.set_relay_address(draft.relay_address.clone().into());
        w.set_wake_command(draft.wake_command.clone().into());
        w.set_wake_timeout(draft.wake_timeout.clone().into());

        w.set_allow_sleep(draft.allow_sleep);
        w.set_allow_shutdown(draft.allow_shutdown);
        w.set_allow_rdp(draft.allow_rdp);
        w.set_power_index(match draft.power {
            wizard::PowerChoice::Ssh => 0,
            wizard::PowerChoice::Command => 1,
        });
        w.set_os_index(match draft.os {
            Os::Windows => 0,
            Os::Linux => 1,
        });
        w.set_ssh_user(draft.ssh_user.clone().into());
        w.set_ssh_port(draft.ssh_port.clone().into());
        w.set_ssh_identity(draft.ssh_identity_file.clone().into());
        w.set_sleep_command(draft.sleep_command.clone().into());
        w.set_shutdown_command(draft.shutdown_command.clone().into());
        w.set_rdp_user(draft.rdp_user.clone().into());
    }

    /// The draft as the window shows it now, and the step; `None` while the wizard is closed.
    fn current(&self) -> Option<(PcDraft, Step)> {
        let session = self.session.borrow();
        let s = session.as_ref()?;
        Some((self.read_draft(s), s.step))
    }

    /// Reads every value of the window into a draft that keeps what the wizard does not show.
    fn read_draft(&self, s: &Session) -> PcDraft {
        let w = &self.wizard;
        let mut draft = s.base.clone();
        draft.name = w.get_name().to_string();
        draft.host = w.get_host().to_string();
        draft.ports = w.get_ports().to_string();
        draft.tunnel = s
            .tunnels
            .get(position(w.get_tunnel_index()))
            .and_then(|option| option.id.clone());
        draft.wake = match w.get_wake_index() {
            1 => WakeChoice::Relay,
            2 => WakeChoice::MagicPacket,
            3 => WakeChoice::Command,
            _ => WakeChoice::None,
        };
        draft.mac = w.get_mac().to_string();
        draft.magic_address = w.get_magic_address().to_string();
        draft.magic_port = w.get_magic_port().to_string();
        draft.relay = match s.relays.get(position(w.get_relay_index())) {
            Some(option) => RelayChoice::Existing(option.id.clone()),
            None => RelayChoice::New,
        };
        draft.new_relay = RelayDraft {
            kind: kind_from_index(w.get_relay_kind()),
            host: w.get_new_host().to_string(),
            port: w.get_new_port().to_string(),
            user: w.get_new_user().to_string(),
            identity_file: w.get_new_identity().to_string(),
            sudo: match w.get_relay_sudo() {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            },
            timeout_seconds: w.get_new_timeout().to_string(),
        };
        draft.interface = w.get_interface().to_string();
        draft.relay_address = w.get_relay_address().to_string();
        draft.wake_command = w.get_wake_command().to_string();
        draft.wake_timeout = w.get_wake_timeout().to_string();
        draft.allow_sleep = w.get_allow_sleep();
        draft.allow_shutdown = w.get_allow_shutdown();
        draft.allow_rdp = w.get_allow_rdp();
        draft.power = if w.get_power_index() == 1 {
            wizard::PowerChoice::Command
        } else {
            wizard::PowerChoice::Ssh
        };
        draft.os = if w.get_os_index() == 1 {
            Os::Linux
        } else {
            Os::Windows
        };
        draft.ssh_user = w.get_ssh_user().to_string();
        draft.ssh_port = w.get_ssh_port().to_string();
        draft.ssh_identity_file = w.get_ssh_identity().to_string();
        draft.sleep_command = w.get_sleep_command().to_string();
        draft.shutdown_command = w.get_shutdown_command().to_string();
        draft.rdp_user = w.get_rdp_user().to_string();
        clear_hidden(&mut draft, s);
        draft
    }

    /// Moves the wizard to `step`: its header, and no errors or checks from the step before.
    fn set_step(&self, step: Step) {
        if let Some(s) = self.session.borrow_mut().as_mut() {
            s.step = step;
        }
        self.wizard.set_step(step_index(step));
        self.wizard.set_step_text(self.tr.wizard_step(step).into());
        self.wizard
            .set_errors(model(vec![SharedString::default(); ERRORS]));
        self.wizard.set_general_error("".into());
        if step != Step::Checks {
            // A run that is still going belongs to the step that was left.
            self.check_run.set(self.check_run.get() + 1);
            self.wizard.set_checks(model(Vec::<CheckLine>::new()));
            self.wizard.set_checks_running(false);
        }
    }

    fn show_problems(&self, problems: &[Problem]) {
        let (errors, general) = place_problems(&self.tr, problems, slot, ERRORS);
        self.wizard.set_errors(model(errors));
        self.wizard.set_general_error(general.into());
    }

    /// Shows the problems, on the first step that has one of them.
    fn show_problems_on_their_step(&self, problems: &[Problem]) {
        if let Some(step) = problems.iter().find_map(|p| Step::of_field(p.field)) {
            self.set_step(step);
        }
        self.show_problems(problems);
    }

    /// "Далі" (or "Готово" on the last step).
    pub(crate) fn wizard_advance(&self) {
        let Some((draft, step)) = self.current() else {
            return;
        };
        if step == Step::Checks {
            self.finish(&draft);
            return;
        }
        let problems = draft.validate_step(step, &self.config.borrow());
        if !problems.is_empty() {
            self.show_problems(&problems);
            return;
        }
        if let Some(next) = step.next() {
            self.set_step(next);
            if next == Step::Checks {
                self.start_checks(&draft);
            }
        }
    }

    /// "Назад"; the values typed so far stay in the window.
    pub(crate) fn wizard_back(&self) {
        let Some(step) = self.session.borrow().as_ref().map(|s| s.step) else {
            return;
        };
        if let Some(previous) = step.previous() {
            self.set_step(previous);
        }
    }

    /// "Перевірити знову" on the last step.
    pub(crate) fn wizard_retest(&self) {
        if let Some((draft, Step::Checks)) = self.current() {
            self.start_checks(&draft);
        }
    }

    /// "Скасувати" or Esc: closes the wizard with nothing saved.
    pub(crate) fn cancel_wizard(&self) {
        *self.session.borrow_mut() = None;
        self.check_run.set(self.check_run.get() + 1);
        let _ = self.wizard.hide();
    }

    /// "Готово": builds the PC and saves it. A problem sends the wizard to its step.
    fn finish(&self, draft: &PcDraft) {
        let outcome = match draft.build(&self.config.borrow()) {
            Ok(outcome) => outcome,
            Err(problems) => {
                self.show_problems_on_their_step(&problems);
                return;
            }
        };
        let change = settings::save_pc(&self.config.borrow(), &outcome);
        match change {
            Err(problems) => self.show_problems_on_their_step(&problems),
            Ok(change) => match self.commit(change) {
                Ok(()) => {
                    self.cancel_wizard();
                    self.refresh();
                }
                Err(text) => self.wizard.set_general_error(text.into()),
            },
        }
    }

    /// Starts the checks of the PC the draft describes, on a worker thread.
    fn start_checks(&self, draft: &PcDraft) {
        let outcome = match draft.build(&self.config.borrow()) {
            Ok(outcome) => outcome,
            Err(problems) => {
                self.show_problems_on_their_step(&problems);
                return;
            }
        };
        let plan = checks::plan(&self.config.borrow(), &outcome);
        let tunnel_up = outcome.pc.tunnel.as_ref().is_some_and(ui::tunnel_is_up);
        let pending: Vec<CheckLine> = plan
            .pending()
            .into_iter()
            .map(|kind| CheckLine {
                name: self.tr.check_name(kind).into(),
                result: SharedString::default(),
                mark: Mark::Busy,
            })
            .collect();
        let run = self.check_run.get() + 1;
        self.check_run.set(run);
        self.wizard.set_checks(model(pending));
        self.wizard.set_checks_running(true);

        let ports = Arc::clone(&self.persist.ports);
        let runner = Arc::clone(&self.persist.runner);
        std::thread::spawn(move || {
            let deps = CheckDeps {
                ports: ports.as_ref(),
                runner: runner.as_ref(),
            };
            let results = checks::run_all(&plan, tunnel_up, &deps);
            let _ = slint::invoke_from_event_loop(move || {
                with_manage(|m| m.checks_done(run, results));
            });
        });
    }

    /// The rows of a finished run, when that run is still the current one.
    pub(crate) fn checks_done(&self, run: u64, results: Vec<CheckRow>) {
        if run != self.check_run.get() || self.session.borrow().is_none() {
            return;
        }
        let rows: Vec<CheckLine> = results
            .iter()
            .map(|row| CheckLine {
                name: self.tr.check_name(row.kind).into(),
                result: self.tr.check_result(&row.result).into(),
                mark: ui::mark(row.display),
            })
            .collect();
        self.wizard.set_checks(model(rows));
        self.wizard.set_checks_running(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_of_the_wizard_has_one_error_slot() {
        assert_eq!(slot(field::NAME), Some(0));
        assert_eq!(slot(field::RDP_USER), Some(25));
        assert_eq!(slot("unknown"), None);
        let all = [
            field::NAME,
            field::HOST,
            field::PORTS,
            field::TUNNEL,
            field::MAC,
            field::MAGIC_ADDRESS,
            field::MAGIC_PORT,
            field::RELAY,
            field::RELAY_HOST,
            field::RELAY_PORT,
            field::RELAY_USER,
            field::RELAY_KEY,
            field::RELAY_SUDO,
            field::RELAY_TIMEOUT,
            field::INTERFACE,
            field::RELAY_ADDRESS,
            field::WAKE_COMMAND,
            field::WAKE_TIMEOUT,
            field::ALLOW_SLEEP,
            field::ALLOW_SHUTDOWN,
            field::SSH_USER,
            field::SSH_PORT,
            field::SSH_KEY,
            field::SLEEP_COMMAND,
            field::SHUTDOWN_COMMAND,
            field::RDP_USER,
        ];
        let mut slots: Vec<usize> = all.iter().filter_map(|f| slot(f)).collect();
        slots.sort_unstable();
        assert_eq!(slots, (0..ERRORS).collect::<Vec<_>>());
    }

    #[test]
    fn a_relay_preset_round_trips_through_its_index() {
        for kind in [
            RelayKind::RouterOs,
            RelayKind::Etherwake,
            RelayKind::Wakeonlan,
        ] {
            assert_eq!(kind_from_index(kind_index(kind)), kind);
        }
        assert_eq!(position(-1), usize::MAX);
    }

    #[test]
    fn a_field_the_preset_does_not_show_is_not_sent() {
        use homelink_core::domain::RelayId;
        let config = homelink_core::testing::single_config();
        let relay = RelayId::parse("box").unwrap();
        let mut draft = PcDraft::new(&config);
        draft.wake = WakeChoice::Relay;
        draft.relay = RelayChoice::Existing(relay.clone());
        draft.interface = "eth0".into();
        draft.relay_address = "10.0.0.255".into();
        draft.new_relay.sudo = Some(true);
        let session = Session {
            base: draft.clone(),
            step: Step::Wake,
            tunnels: Vec::new(),
            relays: vec![RelayOption {
                id: relay,
                host: "192.168.1.1".into(),
                kind: RelayKind::Wakeonlan,
            }],
        };
        clear_hidden(&mut draft, &session);
        // Wakeonlan takes an address but no interface; an existing relay has no sudo question.
        assert_eq!(draft.interface, "");
        assert_eq!(draft.relay_address, "10.0.0.255");
        assert_eq!(draft.new_relay.sudo, None);
    }
}

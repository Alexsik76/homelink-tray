//! The "Тунелі й ПК" page of the settings window (section 7 of the handoff) as pure operations
//! on a [`Config`].
//!
//! Lists: [`tunnel_rows`] and [`pc_rows`] give the name and the secondary text of each row.
//! Changes: [`save_tunnel`] (add or edit), [`delete_tunnel`], [`save_pc`] (add or edit, from
//! the wizard) and [`delete_pc`] each return a [`Change`] with the new configuration; nothing
//! is touched until [`Change::save`] writes it in place (comments kept, renamed ids through
//! [`Renames`]) and the caller hands `change.config` to the engine (`Handle::reload`).
//!
//! Rules:
//! - A tunnel that PCs still use is not deleted: the refusal is a [`Problem`]
//!   ([`Code::TunnelInUse`]) that names the PCs.
//! - A relay that a change leaves unused is removed with it (only relays the change itself
//!   orphaned; a relay that was already unused in the file is left alone).
//! - Renaming a tunnel or PC re-derives its id from the new name; references (`pcs[].tunnel`)
//!   follow, and the old id is recorded in the [`Renames`].
//! - `rdp/<id>.rdp` files are named by PC id and are not renamed or deleted here.

use std::path::Path;

use crate::config::{Config, ConfigError, Renames};
use crate::domain::{PcId, RelayId, TunnelId, WakeMethod};
use crate::problem::{Code, Problem};
use crate::tunnel_form::{self, TunnelForm};
use crate::wizard::PcOutcome;

/// The secondary text of a row, on the right of the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Secondary {
    /// A tunnel row: how many PCs use it.
    PcCount(usize),
    /// A PC row: the name of its tunnel.
    Tunnel(String),
    /// A PC row without a tunnel ("без тунелю").
    NoTunnel,
}

/// A row of a list: tunnel or PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The id, for "Змінити" and "Видалити".
    pub id: String,
    /// The name.
    pub name: String,
    /// The text on the right; render it with `Translator::secondary`.
    pub secondary: Secondary,
}

/// The rows of the "Тунелі" list, in configuration order.
pub fn tunnel_rows(config: &Config) -> Vec<Row> {
    config
        .tunnels
        .iter()
        .map(|t| Row {
            id: t.id.to_string(),
            name: t.name.clone(),
            secondary: Secondary::PcCount(config.pcs_of(Some(&t.id)).count()),
        })
        .collect()
}

/// The rows of the "ПК" list, in configuration order.
pub fn pc_rows(config: &Config) -> Vec<Row> {
    config
        .pcs
        .iter()
        .map(|p| Row {
            id: p.id.to_string(),
            name: p.name.clone(),
            secondary: match p.tunnel.as_ref().and_then(|id| config.tunnel(id)) {
                Some(t) => Secondary::Tunnel(t.name.clone()),
                None => Secondary::NoTunnel,
            },
        })
        .collect()
}

/// A new configuration produced by one operation, not yet written.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// The configuration after the operation.
    pub config: Config,
    /// Ids that changed, for the in-place save.
    pub renames: Renames,
    /// Relays that were removed because nothing uses them any more.
    pub removed_relays: Vec<RelayId>,
}

impl Change {
    /// Writes the new configuration to `path`: edited in place, comments and formatting
    /// kept, renamed entries keep their place. Atomic (see [`crate::config::save_with`]).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        crate::config::save_with(path, &self.config, &self.renames)
    }
}

fn unchanged(config: &Config) -> Change {
    Change {
        config: config.clone(),
        renames: Renames::default(),
        removed_relays: Vec::new(),
    }
}

fn relay_in_use(config: &Config, id: &RelayId) -> bool {
    config
        .pcs
        .iter()
        .any(|p| matches!(&p.wake, Some(WakeMethod::Relay { relay, .. }) if relay == id))
}

/// Removes the relays in `candidates` that no PC uses; returns the ones removed.
fn drop_orphans(config: &mut Config, candidates: &[RelayId]) -> Vec<RelayId> {
    let orphans: Vec<RelayId> = candidates
        .iter()
        .filter(|id| !relay_in_use(config, id))
        .cloned()
        .collect();
    config.relays.retain(|r| !orphans.contains(&r.id));
    orphans
}

/// The relays that some PC uses.
fn used_relays(config: &Config) -> Vec<RelayId> {
    config
        .relays
        .iter()
        .filter(|r| relay_in_use(config, &r.id))
        .map(|r| r.id.clone())
        .collect()
}

/// Adds a tunnel (a form without [`TunnelForm::editing`]) or replaces the edited one.
///
/// A renamed tunnel gets the id of its new name; the PCs behind it follow.
pub fn save_tunnel(config: &Config, form: &TunnelForm) -> Result<Change, Vec<Problem>> {
    let tunnel = form.validate(config)?;
    let mut change = unchanged(config);
    match form.editing() {
        None => change.config.tunnels.push(tunnel),
        Some(old) => {
            let Some(slot) = change.config.tunnels.iter_mut().find(|t| t.id == *old) else {
                return Err(vec![Problem::new(
                    tunnel_form::field::NAME,
                    Code::TunnelUnknown,
                )]);
            };
            *slot = tunnel.clone();
            if tunnel.id != *old {
                for pc in &mut change.config.pcs {
                    if pc.tunnel.as_ref() == Some(old) {
                        pc.tunnel = Some(tunnel.id.clone());
                    }
                }
                change.renames.tunnels.push((old.clone(), tunnel.id));
            }
        }
    }
    Ok(change)
}

/// Deletes a tunnel. Refused while PCs use it: the problem ([`Code::TunnelInUse`], field
/// `tunnel`) has the parameters `tunnel` (its name) and `pcs` (their names, joined by ", ").
pub fn delete_tunnel(config: &Config, id: &TunnelId) -> Result<Change, Vec<Problem>> {
    let Some(tunnel) = config.tunnel(id) else {
        return Err(vec![Problem::new("tunnel", Code::TunnelUnknown)]);
    };
    let users: Vec<&str> = config.pcs_of(Some(id)).map(|p| p.name.as_str()).collect();
    if !users.is_empty() {
        return Err(vec![
            Problem::new("tunnel", Code::TunnelInUse)
                .with("tunnel", tunnel.name.clone())
                .with("pcs", users.join(", ")),
        ]);
    }
    let mut change = unchanged(config);
    change.config.tunnels.retain(|t| t.id != *id);
    Ok(change)
}

/// Adds a PC (from the wizard) or replaces the one it edits, adding the new relay it needs.
/// A relay that the old PC used and the new one does not is removed if no other PC uses it.
/// A renamed PC keeps its place in the list.
pub fn save_pc(config: &Config, outcome: &PcOutcome) -> Result<Change, Vec<Problem>> {
    let mut problems = Vec::new();
    if let Some(tunnel) = &outcome.pc.tunnel
        && config.tunnel(tunnel).is_none()
    {
        problems.push(Problem::new("tunnel", Code::TunnelUnknown));
    }
    if let Some(WakeMethod::Relay { relay, .. }) = &outcome.pc.wake
        && config.relay(relay).is_none()
        && outcome.new_relay.as_ref().map(|r| &r.id) != Some(relay)
    {
        problems.push(Problem::new("relay", Code::RelayUnknown));
    }
    if let Some(old) = &outcome.replaces
        && config.pc(old).is_none()
    {
        problems.push(Problem::new("name", Code::Required));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let before = used_relays(config);
    let mut change = unchanged(config);
    if let Some(relay) = &outcome.new_relay {
        change.config.relays.push(relay.clone());
    }
    match &outcome.replaces {
        None => change.config.pcs.push(outcome.pc.clone()),
        Some(old) => {
            if let Some(slot) = change.config.pcs.iter_mut().find(|p| p.id == *old) {
                *slot = outcome.pc.clone();
            }
            if *old != outcome.pc.id {
                change
                    .renames
                    .pcs
                    .push((old.clone(), outcome.pc.id.clone()));
            }
        }
    }
    change.removed_relays = drop_orphans(&mut change.config, &before);
    Ok(change)
}

/// Deletes a PC, and the relay it used if no other PC uses it. `None` if there is no such PC.
pub fn delete_pc(config: &Config, id: &PcId) -> Option<Change> {
    config.pc(id)?;
    let before = used_relays(config);
    let mut change = unchanged(config);
    change.config.pcs.retain(|p| p.id != *id);
    change.removed_relays = drop_orphans(&mut change.config, &before);
    Some(change)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::testing::{multi_config, pc_id, single_config, tunnel_id};
    use crate::wizard::{PcDraft, WakeChoice};

    fn assert_valid(change: &Change) {
        // Rendering parses the text back and checks it against the configuration.
        config::render(&change.config).unwrap();
    }

    #[test]
    fn rows_have_the_secondary_text_of_the_handoff() {
        let config = multi_config();
        let tunnels = tunnel_rows(&config);
        assert_eq!(
            tunnels
                .iter()
                .map(|r| (r.name.as_str(), r.secondary.clone()))
                .collect::<Vec<_>>(),
            [
                ("Home", Secondary::PcCount(2)),
                ("Office", Secondary::PcCount(1))
            ]
        );
        let pcs = pc_rows(&config);
        assert_eq!(pcs.len(), 4);
        assert_eq!(pcs[0].secondary, Secondary::Tunnel("Home".into()));
        assert_eq!(pcs[3].name, "NAS");
        assert_eq!(pcs[3].secondary, Secondary::NoTunnel);
        assert_eq!(pcs[3].id, "nas");
    }

    #[test]
    fn a_tunnel_with_pcs_is_not_deleted_and_the_refusal_names_them() {
        let config = multi_config();
        let problems = delete_tunnel(&config, &tunnel_id("home")).unwrap_err();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].code, Code::TunnelInUse);
        assert_eq!(
            problems[0].params,
            [
                ("tunnel", "Home".to_owned()),
                ("pcs", "Desktop, Laptop".to_owned())
            ]
        );
        assert!(delete_tunnel(&config, &tunnel_id("nope")).is_err());
    }

    #[test]
    fn an_unused_tunnel_is_deleted() {
        let mut config = multi_config();
        config.pcs.retain(|p| p.tunnel != Some(tunnel_id("office")));
        let change = delete_tunnel(&config, &tunnel_id("office")).unwrap();
        assert!(change.config.tunnel(&tunnel_id("office")).is_none());
        assert_eq!(change.config.tunnels.len(), 1);
        assert_valid(&change);
    }

    #[test]
    fn adding_a_tunnel_appends_it() {
        let config = single_config();
        let mut form = TunnelForm::new(&config);
        form.name = "Lab".into();
        form.conf = r"C:\WG\lab.conf".into();
        form.probe_host = "10.9.0.1".into();
        let change = save_tunnel(&config, &form).unwrap();
        assert_eq!(change.config.tunnels.len(), 2);
        assert_eq!(change.config.tunnels[1].id.as_str(), "lab");
        assert_eq!(change.renames, Renames::default());
        assert_valid(&change);
    }

    #[test]
    fn editing_a_tunnel_keeps_its_id_and_its_pcs() {
        let config = multi_config();
        let mut form = TunnelForm::from_tunnel(config.tunnel(&tunnel_id("home")).unwrap());
        form.probe_host = "192.168.88.254".into();
        let change = save_tunnel(&config, &form).unwrap();
        assert_eq!(change.renames, Renames::default());
        assert_eq!(
            change
                .config
                .tunnel(&tunnel_id("home"))
                .unwrap()
                .probe_host
                .as_str(),
            "192.168.88.254"
        );
        assert_eq!(change.config.pcs, config.pcs);
        assert_valid(&change);
    }

    #[test]
    fn renaming_a_tunnel_renames_its_id_and_moves_its_pcs() {
        let config = multi_config();
        let mut form = TunnelForm::from_tunnel(config.tunnel(&tunnel_id("home")).unwrap());
        form.name = "Дім".into();
        let change = save_tunnel(&config, &form).unwrap();
        assert_eq!(
            change.renames.tunnels,
            [(tunnel_id("home"), tunnel_id("dim"))]
        );
        assert_eq!(change.config.tunnels[0].id.as_str(), "dim");
        assert_eq!(
            change.config.pc(&pc_id("desktop")).unwrap().tunnel,
            Some(tunnel_id("dim"))
        );
        assert_valid(&change);
    }

    fn relay_pc_draft(config: &Config) -> PcDraft {
        let mut draft = PcDraft::from_pc(config, config.pc(&pc_id("desktop")).unwrap());
        draft.name = "Desktop".into();
        draft
    }

    #[test]
    fn deleting_a_pc_removes_the_relay_it_alone_used() {
        let config = multi_config();
        let change = delete_pc(&config, &pc_id("desktop")).unwrap();
        assert!(change.config.pc(&pc_id("desktop")).is_none());
        assert!(change.config.relays.is_empty());
        assert_eq!(change.removed_relays.len(), 1);
        assert_valid(&change);
        assert!(delete_pc(&config, &pc_id("nope")).is_none());
    }

    #[test]
    fn a_relay_that_another_pc_uses_stays() {
        let config = multi_config();
        let mut second = config.pc(&pc_id("desktop")).unwrap().clone();
        second.id = pc_id("second");
        second.name = "Second".into();
        let mut config = config;
        config.pcs.push(second);
        let change = delete_pc(&config, &pc_id("desktop")).unwrap();
        assert_eq!(change.config.relays.len(), 1);
        assert!(change.removed_relays.is_empty());
    }

    #[test]
    fn a_relay_that_was_already_unused_is_left_alone() {
        let mut config = multi_config();
        let mut spare = config.relays[0].clone();
        spare.id = crate::domain::RelayId::parse("spare").unwrap();
        config.relays.push(spare);
        let change = delete_pc(&config, &pc_id("nas")).unwrap();
        assert_eq!(change.config.relays.len(), 2);
        assert!(change.removed_relays.is_empty());
    }

    #[test]
    fn switching_the_wake_method_removes_the_orphaned_relay() {
        let config = multi_config();
        let mut draft = relay_pc_draft(&config);
        draft.wake = WakeChoice::MagicPacket;
        draft.mac = "AA:BB:CC:DD:EE:FF".into();
        draft.magic_address = "192.168.88.255".into();
        let outcome = draft.build(&config).unwrap();
        let change = save_pc(&config, &outcome).unwrap();
        assert!(change.config.relays.is_empty());
        assert_eq!(change.removed_relays.len(), 1);
        assert_eq!(change.config.pcs[0].id.as_str(), "desktop");
        assert_valid(&change);
    }

    #[test]
    fn a_new_pc_with_a_new_relay_adds_both() {
        let config = single_config();
        let mut draft = PcDraft::new(&config);
        draft.name = "Дім PC".into();
        draft.host = "192.168.88.30".into();
        draft.ports = "3389".into();
        draft.wake = WakeChoice::Relay;
        draft.mac = "AA:BB:CC:DD:EE:01".into();
        draft.relay = crate::wizard::RelayChoice::New;
        draft.new_relay.host = "192.168.88.1".into();
        draft.new_relay.user = "homelink".into();
        draft.new_relay.identity_file = r"C:\HomeLink\relay_key".into();
        draft.interface = "bridge".into();
        let outcome = draft.build(&config).unwrap();
        let change = save_pc(&config, &outcome).unwrap();
        assert_eq!(change.config.pcs.len(), 2);
        assert_eq!(change.config.pcs[1].id.as_str(), "dim-pc");
        // `single_config` already has the unused relay `gateway`; it is not swept.
        assert_eq!(change.config.relays.len(), 2);
        assert_eq!(change.config.relays[1].id.as_str(), "routeros-192-168-88-1");
        assert!(change.removed_relays.is_empty());
        assert_valid(&change);
    }

    #[test]
    fn renaming_a_pc_keeps_its_place_and_records_the_rename() {
        let config = multi_config();
        let mut draft = relay_pc_draft(&config);
        draft.name = "Робочий стіл".into();
        let outcome = draft.build(&config).unwrap();
        let change = save_pc(&config, &outcome).unwrap();
        assert_eq!(
            change.renames.pcs,
            [(pc_id("desktop"), pc_id("robochyi-stil"))]
        );
        assert_eq!(change.config.pcs[0].id.as_str(), "robochyi-stil");
        assert_eq!(change.config.pcs.len(), config.pcs.len());
        assert_valid(&change);
    }

    #[test]
    fn a_pc_for_an_unknown_tunnel_or_relay_is_refused() {
        let config = single_config();
        let mut draft = PcDraft::new(&config);
        draft.name = "X".into();
        draft.host = "10.0.0.1".into();
        draft.ports = "22".into();
        let mut outcome = draft.build(&config).unwrap();
        outcome.pc.tunnel = Some(tunnel_id("ghost"));
        let problems = save_pc(&config, &outcome).unwrap_err();
        assert_eq!(problems[0].code, Code::TunnelUnknown);
    }
}

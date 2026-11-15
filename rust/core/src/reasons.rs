//! The words for actions that lack a program (the capability spec, artboard `Unavailable`):
//! the reason line under the inactive buttons of the panel and the "Недоступно" row of the
//! details window. Built from the [`Availability`] values of the snapshot, so the UI decides
//! nothing.
//!
//! - Tunnel: "Не знайдено клієнт WireGuard".
//! - PC: "Розбудити, сон, вимкнення: не знайдено клієнт OpenSSH". Actions that lack different
//!   programs are listed in groups separated by "; ".

use crate::capabilities::Need;
use crate::i18n::Translator;
use crate::snapshot::{Availability, PcAvailability, TunnelView};

/// The name of a missing need: a tool by its catalog name, a user program by its file name.
pub fn need_name(tr: &Translator, need: &Need) -> String {
    match need {
        Need::Tool(tool) => tr.t(&format!("tool.{}", tool.code()), &[]),
        Need::Program(program) => tr.t("reason.program", &[("name", program)]),
    }
}

fn needs_text(tr: &Translator, needs: &[Need]) -> String {
    needs
        .iter()
        .map(|need| need_name(tr, need))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The reason line of a tunnel whose toggle lacks programs; `None` otherwise.
pub fn tunnel_reason(tr: &Translator, tunnel: &TunnelView) -> Option<String> {
    let missing = tunnel.toggle_availability.missing();
    (!missing.is_empty()).then(|| tr.t("reason.tunnel", &[("what", &needs_text(tr, missing))]))
}

/// The actions of a PC that lack programs, grouped by the programs they lack, in action order.
fn groups(tr: &Translator, availability: &PcAvailability) -> Vec<(String, String)> {
    let names = ["wake", "sleep", "shutdown", "rdp"];
    let mut groups: Vec<(Vec<String>, &[Need])> = Vec::new();
    for (name, a) in names.iter().zip(availability.all()) {
        let missing = a.missing();
        if missing.is_empty() {
            continue;
        }
        let action = tr.t(&format!("reason.action.{name}"), &[]);
        match groups.iter_mut().find(|(_, m)| *m == missing) {
            Some((actions, _)) => actions.push(action),
            None => groups.push((vec![action], missing)),
        }
    }
    groups
        .into_iter()
        .map(|(actions, missing)| (actions.join(", "), needs_text(tr, missing)))
        .collect()
}

/// The reason line under the buttons of a PC; `None` when no configured action lacks a program.
pub fn pc_reason(tr: &Translator, availability: &PcAvailability) -> Option<String> {
    let parts: Vec<String> = groups(tr, availability)
        .into_iter()
        .map(|(actions, what)| tr.t("reason.pc", &[("actions", &actions), ("what", &what)]))
        .collect();
    (!parts.is_empty()).then(|| capitalise(&parts.join("; ")))
}

/// The value of the "Недоступно" row of the details window; `None` when nothing lacks a program.
pub fn pc_detail(tr: &Translator, availability: &PcAvailability) -> Option<String> {
    let parts: Vec<String> = groups(tr, availability)
        .into_iter()
        .map(|(actions, what)| tr.t("reason.detail", &[("actions", &actions), ("what", &what)]))
        .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// Whether any configured action of the PC lacks a program.
pub fn pc_lacks_programs(availability: &PcAvailability) -> bool {
    availability
        .all()
        .into_iter()
        .any(|a| matches!(a, Availability::Missing(_)))
}

/// The text with its first letter in upper case.
fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::Tool;
    use crate::i18n::Lang;
    use crate::snapshot::Availability as A;

    fn ssh() -> A {
        A::Missing(vec![Need::Tool(Tool::Ssh)])
    }

    #[test]
    fn a_pc_reason_lists_the_actions_and_the_missing_client() {
        let uk = Translator::new(Lang::Uk);
        let availability = PcAvailability {
            wake: ssh(),
            sleep: ssh(),
            shutdown: ssh(),
            rdp: A::NotNow,
        };
        assert_eq!(
            pc_reason(&uk, &availability).as_deref(),
            Some("Розбудити, сон, вимкнення: не знайдено клієнт OpenSSH")
        );
        assert_eq!(
            pc_detail(&uk, &availability).as_deref(),
            Some("розбудити, сон, вимкнення — не знайдено клієнт OpenSSH")
        );
        let en = Translator::new(Lang::En);
        assert_eq!(
            pc_reason(&en, &availability).as_deref(),
            Some("Wake, sleep, shut down: OpenSSH client not found")
        );
    }

    #[test]
    fn different_missing_programs_make_groups_and_nothing_missing_makes_none() {
        let uk = Translator::new(Lang::Uk);
        let availability = PcAvailability {
            wake: A::Missing(vec![Need::Program("wakeonlan".into())]),
            sleep: A::NotConfigured,
            shutdown: A::Available,
            rdp: A::Missing(vec![Need::Tool(Tool::Rdp)]),
        };
        assert_eq!(
            pc_reason(&uk, &availability).as_deref(),
            Some("Розбудити: не знайдено програму wakeonlan; RDP: не знайдено mstsc")
        );
        assert!(pc_lacks_programs(&availability));
        let fine = PcAvailability {
            wake: A::Available,
            sleep: A::NotNow,
            shutdown: A::NotConfigured,
            rdp: A::NotNow,
        };
        assert_eq!(pc_reason(&uk, &fine), None);
        assert_eq!(pc_detail(&uk, &fine), None);
        assert!(!pc_lacks_programs(&fine));
    }

    #[test]
    fn every_tool_has_a_name_in_both_languages() {
        for lang in Lang::ALL {
            let tr = Translator::new(lang);
            for tool in Tool::ALL {
                let name = need_name(&tr, &Need::Tool(tool));
                assert!(!name.starts_with("tool."), "{} {name}", lang.code());
            }
        }
    }
}

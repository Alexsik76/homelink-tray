//! Plain data model of the panel: tunnels, PCs and their states.
//! Sample data only; real data can later fill the same shape.

use crate::{GroupView, PcView, ViewState};
use slint::{ModelRc, SharedString, VecModel};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PcState {
    Online,
    #[allow(dead_code)]
    Waking,
    Off,
    Unknown,
}

pub struct Pc {
    pub name: &'static str,
    /// Real state while the tunnel (if any) is connected.
    pub state: PcState,
    pub expanded: bool,
}

pub struct Group {
    /// `None` for the last group of PCs that belong to no tunnel.
    pub tunnel: Option<&'static str>,
    pub connected: bool,
    pub pcs: Vec<Pc>,
}

#[derive(Clone, Copy)]
pub enum PcAction {
    Connect,
    Wake,
    Sleep,
    Shutdown,
}

impl PcAction {
    pub fn from_index(i: i32) -> Option<Self> {
        match i {
            0 => Some(Self::Connect),
            1 => Some(Self::Wake),
            2 => Some(Self::Sleep),
            3 => Some(Self::Shutdown),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Connect => "Підключити",
            Self::Wake => "Розбудити",
            Self::Sleep => "Сон",
            Self::Shutdown => "Вимкнути",
        }
    }
}

pub enum UserAction {
    ToggleTunnel(usize),
    ToggleExpand(usize, usize),
    Run(usize, usize, PcAction),
}

pub fn sample() -> Vec<Group> {
    let pc = |name, state| Pc { name, state, expanded: false };
    vec![
        Group {
            tunnel: Some("Дім"),
            connected: true,
            pcs: vec![pc("Робоча станція", PcState::Online), pc("Сервер", PcState::Off)],
        },
        Group { tunnel: Some("Офіс"), connected: false, pcs: vec![pc("Тестовий ПК", PcState::Unknown)] },
        Group { tunnel: None, connected: true, pcs: vec![pc("Ноутбук", PcState::Online)] },
    ]
}

/// The state the user sees: PCs behind a switched-off tunnel are unknown.
fn shown_state(group: &Group, pc: &Pc) -> PcState {
    if group.tunnel.is_some() && !group.connected { PcState::Unknown } else { pc.state }
}

/// Applies one user action. Returns a log line when an action button was run.
pub fn apply(groups: &mut [Group], action: UserAction) -> Option<String> {
    match action {
        UserAction::ToggleTunnel(g) => {
            let group = groups.get_mut(g)?;
            group.connected = !group.connected;
            None
        }
        UserAction::ToggleExpand(g, p) => {
            let pc = groups.get_mut(g)?.pcs.get_mut(p)?;
            pc.expanded = !pc.expanded;
            None
        }
        UserAction::Run(g, p, act) => {
            let group = groups.get(g)?;
            let pc = group.pcs.get(p)?;
            let state = shown_state(group, pc);
            let allowed = match act {
                PcAction::Wake => state == PcState::Off,
                _ => state == PcState::Online,
            };
            allowed.then(|| format!("action: {} → {}", act.label(), pc.name))
        }
    }
}

pub fn to_ui(groups: &[Group]) -> ModelRc<GroupView> {
    let views: Vec<GroupView> = groups
        .iter()
        .map(|g| {
            let pcs: Vec<PcView> = g
                .pcs
                .iter()
                .map(|pc| {
                    let state = shown_state(g, pc);
                    let (view_state, text) = match state {
                        PcState::Online => (ViewState::Online, "у мережі"),
                        PcState::Waking => (ViewState::Waking, "прокидається"),
                        PcState::Off => (ViewState::Off, "вимкнено"),
                        PcState::Unknown => (ViewState::Unknown, "невідомо"),
                    };
                    PcView {
                        name: pc.name.into(),
                        state: view_state,
                        state_text: text.into(),
                        expanded: pc.expanded,
                        can_wake: state == PcState::Off,
                        can_other: state == PcState::Online,
                    }
                })
                .collect();
            GroupView {
                tunnel: SharedString::from(g.tunnel.unwrap_or("")),
                has_tunnel: g.tunnel.is_some(),
                connected: g.connected,
                pcs: ModelRc::new(VecModel::from(pcs)),
            }
        })
        .collect();
    ModelRc::new(VecModel::from(views))
}

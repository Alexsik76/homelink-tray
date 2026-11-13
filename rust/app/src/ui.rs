//! Glue between the engine snapshot and the Slint views (section 7 of the architecture).
//!
//! The engine thread publishes a [`Snapshot`]; [`UiSink`] hands it to the Slint event loop,
//! where [`Ui::show`] rebuilds the models. The callbacks run on the same thread and turn user
//! actions into [`Command`]s. The only UI state, the sleep or shutdown question waiting for
//! "Yes", lives here too and is never sent to the engine.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use homelink_core::domain::{PcId, PcState, PowerAction, TunnelId, TunnelState};
use homelink_core::engine::Handle;
use homelink_core::i18n::Translator;
use homelink_core::machine::Command;
use homelink_core::notice::Notice;
use homelink_core::ports::Sink;
use homelink_core::snapshot::{GroupView, PcView, Snapshot};
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::platform;
use crate::view::{GroupRow, LinkState, Panel, PcRow, PowerKind, Strings, Tray, ViewState};

/// Longest tooltip the tray accepts, in UTF-16 code units (Windows allows 127 and a NUL).
const TOOLTIP_MAX_UNITS: usize = 127;

/// A focus loss within this time before a tray click is the click that took the focus away,
/// so that click must not reopen the panel.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

thread_local! {
    /// The UI state of this thread. Set once by [`install`], on the Slint event loop thread.
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

/// What opens Remote Desktop for a PC. The error is a text ready to show.
pub(crate) type RdpAction = Box<dyn Fn(&PcId) -> Result<(), String>>;

/// Hands engine snapshots to the Slint event loop and writes the notices to the log.
pub(crate) struct UiSink {
    translator: Translator,
}

impl UiSink {
    /// A sink that words the notices with `translator`.
    pub(crate) fn new(translator: Translator) -> Self {
        Self { translator }
    }
}

impl Sink for UiSink {
    fn publish(&self, snapshot: Snapshot) {
        let _ = slint::invoke_from_event_loop(move || with_ui(|ui| ui.show(&snapshot)));
    }

    /// No tray balloon or toast API was found for Slint 1.18 (the Slint sources could not be
    /// read to be sure), so a notice goes to the log file only, worded as the user would read
    /// it. Showing notices on screen is still to do.
    fn notify(&self, notice: Notice) {
        let (title, body) = self.translator.notice(&notice);
        log::info!(target: "notice", "{title}: {body}");
    }
}

fn with_ui(f: impl FnOnce(&Ui)) {
    let ui = UI.with(|slot| slot.borrow().clone());
    if let Some(ui) = ui {
        f(&ui);
    }
}

/// The state of the UI on the event loop thread.
pub(crate) struct Ui {
    panel: Weak<Panel>,
    tray: Weak<Tray>,
    tr: Translator,
    handle: Handle,
    rdp: RdpAction,
    state: RefCell<State>,
    /// When the panel last lost focus (see [`REOPEN_GUARD`]).
    blurred_at: Cell<Option<Instant>>,
    /// When the panel was last shown (see [`platform::SHOW_FOCUS_GRACE`]).
    shown_at: Cell<Option<Instant>>,
}

#[derive(Default)]
struct State {
    /// The snapshot on screen; the models are rebuilt from it when a question is asked.
    last: Option<Snapshot>,
    /// A sleep or shutdown request waiting for "Yes".
    asking: Option<(PcId, PowerAction)>,
}

/// Connects the views to the engine: fills the texts, wires the callbacks and makes the
/// UI state reachable from [`UiSink`]. Call on the event loop thread, before the engine starts.
pub(crate) fn install(panel: &Panel, tray: &Tray, tr: Translator, handle: Handle, rdp: RdpAction) {
    fill_strings(panel, tray, &tr);
    let ui = Rc::new(Ui {
        panel: panel.as_weak(),
        tray: tray.as_weak(),
        tr,
        handle,
        rdp,
        state: RefCell::default(),
        blurred_at: Cell::new(None),
        shown_at: Cell::new(None),
    });
    wire(panel, tray, &ui);
    UI.with(|slot| *slot.borrow_mut() = Some(ui));
}

fn fill_strings(panel: &Panel, tray: &Tray, tr: &Translator) {
    let s = panel.global::<Strings>();
    s.set_no_tunnel(tr.t("panel.no_tunnel", &[]).into());
    s.set_empty(tr.t("panel.empty", &[]).into());
    s.set_wake(tr.t("action.wake", &[]).into());
    s.set_sleep(tr.t("action.sleep", &[]).into());
    s.set_shutdown(tr.t("action.shutdown", &[]).into());
    s.set_rdp(tr.t("action.rdp", &[]).into());
    s.set_yes(tr.t("confirm.yes", &[]).into());
    s.set_no(tr.t("confirm.no", &[]).into());
    let menu = tray.global::<Strings>();
    menu.set_show_panel(tr.t("panel.show", &[]).into());
    menu.set_quit(tr.t("panel.quit", &[]).into());
}

fn wire(panel: &Panel, tray: &Tray, ui: &Rc<Ui>) {
    {
        let u = Rc::clone(ui);
        panel.on_tunnel_clicked(move |id| u.tunnel_clicked(&id));
    }
    {
        let u = Rc::clone(ui);
        panel.on_wake_clicked(move |id| u.send_pc(&id, Command::Wake));
    }
    {
        let u = Rc::clone(ui);
        panel.on_ask_power(move |id, kind| u.ask_power(&id, kind));
    }
    {
        let u = Rc::clone(ui);
        panel.on_confirm_power(move |id| u.confirm_power(&id));
    }
    {
        let u = Rc::clone(ui);
        panel.on_cancel_power(move || u.cancel_power());
    }
    {
        let u = Rc::clone(ui);
        panel.on_rdp_clicked(move |id| u.open_rdp(&id));
    }
    {
        let u = Rc::clone(ui);
        panel.on_hide_requested(move || u.hide());
    }
    panel.window().on_winit_window_event(|_, event| {
        if let WindowEvent::Focused(false) = event {
            // Deferred, as the prototype did: the focus change may come from inside a
            // window event.
            let _ = slint::invoke_from_event_loop(|| with_ui(|ui| ui.focus_lost()));
        }
        EventResult::Propagate
    });
    {
        let u = Rc::clone(ui);
        tray.on_toggle_panel(move || u.toggle_panel());
    }
    {
        let handle = ui.handle.clone();
        tray.on_quit_requested(move || {
            handle.stop();
            let _ = slint::quit_event_loop();
        });
    }
}

impl Ui {
    /// Rebuilds the views from a snapshot and remembers it.
    fn show(&self, snapshot: &Snapshot) {
        let asking = {
            let mut state = self.state.borrow_mut();
            state.asking = state
                .asking
                .take()
                .filter(|(pc, action)| action_enabled(snapshot, pc, *action));
            state.last = Some(snapshot.clone());
            state.asking.clone()
        };
        let (Some(panel), Some(tray)) = (self.panel.upgrade(), self.tray.upgrade()) else {
            return;
        };
        tray.set_pc_screen(view_state(snapshot.tray.pc));
        tray.set_badge(link_state(snapshot.tray.tunnel));
        tray.set_tip(self.tooltip(snapshot).into());
        let groups: Vec<GroupRow> = snapshot
            .groups
            .iter()
            .map(|group| self.group_row(group, asking.as_ref()))
            .collect();
        panel.set_groups(ModelRc::new(VecModel::from(groups)));
        // The panel grows from its top-left corner; keep its bottom edge above the taskbar.
        if panel.window().is_visible() {
            platform::place_panel(&panel);
        }
    }

    /// Re-renders the last snapshot after a change of UI state.
    fn refresh(&self) {
        let last = self.state.borrow().last.clone();
        if let Some(snapshot) = last {
            self.show(&snapshot);
        }
    }

    fn group_row(&self, group: &GroupView, asking: Option<&(PcId, PowerAction)>) -> GroupRow {
        let pcs: Vec<PcRow> = group.pcs.iter().map(|pc| self.pc_row(pc, asking)).collect();
        let (tunnel_id, has_tunnel, name, link, detail, can_toggle) = match &group.tunnel {
            Some(t) => (
                t.id.to_string(),
                true,
                t.name.clone(),
                link_state(t.state),
                self.tr.t(&format!("tunnel.{}", t.state.as_str()), &[]),
                t.can_toggle,
            ),
            None => (
                String::new(),
                false,
                String::new(),
                LinkState::Down,
                String::new(),
                false,
            ),
        };
        GroupRow {
            tunnel_id: tunnel_id.into(),
            has_tunnel,
            tunnel_name: name.into(),
            tunnel_state: link,
            tunnel_state_text: detail.into(),
            can_toggle,
            pcs: ModelRc::new(VecModel::from(pcs)),
        }
    }

    fn pc_row(&self, pc: &PcView, asking: Option<&(PcId, PowerAction)>) -> PcRow {
        let (is_asking, kind, question) = match asking {
            Some((id, action)) if *id == pc.id => {
                let (kind, key) = match action {
                    PowerAction::Sleep => (PowerKind::Sleep, "confirm.sleep"),
                    PowerAction::Shutdown => (PowerKind::Shutdown, "confirm.shutdown"),
                };
                (true, kind, self.tr.t(key, &[("pc", pc.name.as_str())]))
            }
            _ => (false, PowerKind::Sleep, String::new()),
        };
        PcRow {
            id: pc.id.to_string().into(),
            name: pc.name.clone().into(),
            state: view_state(pc.state),
            state_text: self.pc_state_text(pc.state).into(),
            can_wake: pc.actions.wake,
            can_sleep: pc.actions.sleep,
            can_shutdown: pc.actions.shutdown,
            can_rdp: pc.actions.rdp,
            asking: is_asking,
            asking_kind: kind,
            question: question.into(),
        }
    }

    fn pc_state_text(&self, state: PcState) -> String {
        self.tr.t(&format!("pc.{}", state.as_str()), &[])
    }

    /// The tooltip: one line per tunnel and per PC, cut to what the tray accepts.
    fn tooltip(&self, snapshot: &Snapshot) -> String {
        let mut lines = Vec::new();
        for group in &snapshot.groups {
            if let Some(t) = &group.tunnel {
                let state = self.tr.t(&format!("tunnel.{}", t.state.as_str()), &[]);
                lines.push(self.tr.t(
                    "tooltip.tunnel",
                    &[("name", t.name.as_str()), ("state", state.as_str())],
                ));
            }
            for pc in &group.pcs {
                let state = self.pc_state_text(pc.state);
                lines.push(self.tr.t(
                    "tooltip.pc",
                    &[("name", pc.name.as_str()), ("state", state.as_str())],
                ));
            }
        }
        truncate_utf16(&lines.join("\n"), TOOLTIP_MAX_UNITS)
    }

    fn tunnel_clicked(&self, id: &str) {
        if let Ok(id) = TunnelId::parse(id) {
            self.handle.send(Command::ToggleTunnel(id));
        }
    }

    /// Sends a command built from a PC id; an id that does not parse is ignored.
    fn send_pc(&self, id: &str, command: impl FnOnce(PcId) -> Command) {
        if let Ok(pc) = PcId::parse(id) {
            self.handle.send(command(pc));
        }
    }

    /// Opens Remote Desktop for a PC. The button is enabled by the engine's snapshot; a failure
    /// is logged and shown in a message box, since there is no balloon to show it in.
    fn open_rdp(&self, id: &str) {
        let Ok(pc) = PcId::parse(id) else { return };
        if let Err(detail) = (self.rdp)(&pc) {
            log::error!("Remote Desktop for {pc}: {detail}");
            let name = self
                .state
                .borrow()
                .last
                .iter()
                .flat_map(|snapshot| snapshot.groups.iter())
                .flat_map(|group| group.pcs.iter())
                .find(|view| view.id == pc)
                .map_or_else(|| id.to_owned(), |view| view.name.clone());
            platform::message(&self.tr.t(
                "error.rdp",
                &[("pc", name.as_str()), ("detail", detail.as_str())],
            ));
        }
    }

    fn ask_power(&self, id: &str, kind: PowerKind) {
        let action = match kind {
            PowerKind::Sleep => PowerAction::Sleep,
            PowerKind::Shutdown => PowerAction::Shutdown,
        };
        let Ok(pc) = PcId::parse(id) else { return };
        self.state.borrow_mut().asking = Some((pc, action));
        self.refresh();
    }

    fn confirm_power(&self, id: &str) {
        let asked = self
            .state
            .borrow_mut()
            .asking
            .take_if(|(pc, _)| pc.as_str() == id);
        if let Some((pc, action)) = asked {
            self.handle.send(Command::Power(pc, action));
        }
        self.refresh();
    }

    fn cancel_power(&self) {
        self.state.borrow_mut().asking = None;
        self.refresh();
    }

    fn toggle_panel(&self) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        if panel.window().is_visible() {
            self.hide();
            return;
        }
        if self
            .blurred_at
            .get()
            .is_some_and(|t| t.elapsed() < REOPEN_GUARD)
        {
            return;
        }
        // Placed before showing (Windows) and again after (other platforms, where the window
        // exists only once it is shown).
        platform::place_panel(&panel);
        let _ = panel.show();
        self.shown_at.set(Some(Instant::now()));
        platform::place_panel(&panel);
        panel.invoke_focus_panel();
        let _ = panel.window().with_winit_window(|w| w.focus_window());
    }

    fn focus_lost(&self) {
        if self
            .shown_at
            .get()
            .is_some_and(|t| t.elapsed() < platform::SHOW_FOCUS_GRACE)
        {
            return;
        }
        self.blurred_at.set(Some(Instant::now()));
        self.hide();
    }

    /// Hides the panel. A question that was waiting for an answer is dropped.
    fn hide(&self) {
        self.state.borrow_mut().asking = None;
        if let Some(panel) = self.panel.upgrade() {
            let _ = panel.hide();
        }
    }
}

/// Whether the action the question asks for is still enabled in the snapshot.
fn action_enabled(snapshot: &Snapshot, pc: &PcId, action: PowerAction) -> bool {
    snapshot
        .groups
        .iter()
        .flat_map(|g| g.pcs.iter())
        .find(|view| view.id == *pc)
        .is_some_and(|view| match action {
            PowerAction::Sleep => view.actions.sleep,
            PowerAction::Shutdown => view.actions.shutdown,
        })
}

fn view_state(state: PcState) -> ViewState {
    match state {
        PcState::Online => ViewState::Online,
        PcState::Waking => ViewState::Waking,
        PcState::Offline => ViewState::Off,
        PcState::Unknown => ViewState::Unknown,
    }
}

fn link_state(state: TunnelState) -> LinkState {
    match state {
        TunnelState::Up => LinkState::Up,
        TunnelState::Starting => LinkState::Starting,
        TunnelState::Down => LinkState::Down,
    }
}

/// The longest prefix of `text` that is at most `max_units` UTF-16 code units long, cut
/// between characters.
fn truncate_utf16(text: &str, max_units: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in text.chars() {
        units += c.len_utf16();
        if units > max_units {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_truncation_counts_utf16_units() {
        assert_eq!(truncate_utf16("abcdef", 4), "abcd");
        assert_eq!(truncate_utf16("ab", 4), "ab");
        // Each of these is two UTF-16 units; three of them do not fit into three units.
        assert_eq!(truncate_utf16("😀😀", 3), "😀");
        assert_eq!(truncate_utf16("ієї", 2), "іє");
    }
}

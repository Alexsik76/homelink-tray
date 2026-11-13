//! Glue between the engine snapshot and the Slint views (section 7 of the architecture).
//!
//! The engine thread publishes a [`Snapshot`]; [`UiSink`] hands it to the Slint event loop,
//! where [`Ui::show`] updates the models. The callbacks run on the same thread and turn user
//! actions into [`Command`]s. The UI state that Slint keeps (the open submenu, the armed power
//! button) is in the `Nav` global; this module keeps what the engine does not know about: the
//! element of the details window and the blink phase.
//!
//! The rows are updated in place when the groups and PCs stay the same, so that the elements
//! keep their keyboard focus and their pressed state while the engine publishes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use homelink_core::domain::{PcId, PowerAction, TunnelId, TunnelState};
use homelink_core::engine::Handle;
use homelink_core::i18n::Translator;
use homelink_core::machine::Command;
use homelink_core::notice::Notice;
use homelink_core::ports::{Clock, Sink, Time};
use homelink_core::snapshot::{
    Attention, DisplayState, GroupView, PcView, Snapshot, TrayState, TunnelToggle, TunnelView,
    WakeSummary,
};
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel, Weak};

use crate::appearance::Appearance;
use crate::icon;
use crate::platform;
use crate::view::{
    DetailRow, DetailsWindow, GroupRow, Mark, Motion, Nav, Panel, PcRow, PowerKind, Strings, Theme,
    Tray, TunnelRow,
};

/// Longest tooltip the tray accepts, in UTF-16 code units (Windows allows 127 and a NUL).
const TOOLTIP_MAX_UNITS: usize = 127;

/// A focus loss within this time before a tray click is the click that took the focus away,
/// so that click must not reopen the panel.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

/// Half the period of the busy blink (1 s period, section 3 of the handoff).
const BLINK_HALF_PERIOD: Duration = Duration::from_millis(500);

/// How often the details window is redrawn while it is open, so that "last check" ages.
const DETAILS_REFRESH: Duration = Duration::from_secs(1);

/// Width of the details window in logical pixels (section 6 of the handoff).
const DETAILS_WIDTH: f32 = 320.0;
/// Gap between the panel and the details window, in logical pixels.
const DETAILS_GAP: f32 = 8.0;

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

/// Whether the tunnel is up in the last snapshot: false before the first one, and for a tunnel
/// that is not in it. The checks of the settings use it for a PC behind a tunnel.
pub(crate) fn tunnel_is_up(id: &TunnelId) -> bool {
    let ui = UI.with(|slot| slot.borrow().clone());
    ui.is_some_and(|ui| {
        ui.state.borrow().last.as_ref().is_some_and(|snapshot| {
            snapshot
                .groups
                .iter()
                .filter_map(|group| group.tunnel.as_ref())
                .any(|tunnel| tunnel.id == *id && tunnel.state == TunnelState::Up)
        })
    })
}

/// The windows of the application, as the UI wires them.
pub(crate) struct Windows<'a> {
    /// The tray panel.
    pub(crate) panel: &'a Panel,
    /// The details window, one for all elements.
    pub(crate) details: &'a DetailsWindow,
    /// The tray icon.
    pub(crate) tray: &'a Tray,
}

/// What the UI is connected to, passed to [`install`].
pub(crate) struct Setup {
    /// The texts, in the language of the settings.
    pub(crate) translator: Translator,
    /// Sends commands to the engine.
    pub(crate) handle: Handle,
    /// Opens Remote Desktop.
    pub(crate) rdp: RdpAction,
    /// The engine's clock, for the age of the last check.
    pub(crate) clock: Arc<dyn Clock>,
    /// The theme and the tray icon size.
    pub(crate) appearance: Appearance,
}

/// What the details window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Tunnel(TunnelId),
    Pc(PcId),
}

impl Target {
    /// The element key the panel sends: `t:<id>` for a tunnel, `p:<id>` for a PC.
    fn from_key(key: &str) -> Option<Self> {
        if let Some(id) = key.strip_prefix("t:") {
            TunnelId::parse(id).ok().map(Self::Tunnel)
        } else {
            PcId::parse(key.strip_prefix("p:")?).ok().map(Self::Pc)
        }
    }
}

/// The state of the UI on the event loop thread.
pub(crate) struct Ui {
    panel: Weak<Panel>,
    details: Weak<DetailsWindow>,
    tray: Weak<Tray>,
    tr: Translator,
    handle: Handle,
    rdp: RdpAction,
    clock: Arc<dyn Clock>,
    appearance: Appearance,
    state: RefCell<State>,
    models: RefCell<Models>,
    /// The blink phase of the busy markers.
    blink: Cell<bool>,
    /// When the panel last lost focus (see [`REOPEN_GUARD`]).
    blurred_at: Cell<Option<Instant>>,
    /// When the panel was last shown (see [`platform::SHOW_FOCUS_GRACE`]).
    shown_at: Cell<Option<Instant>>,
    /// The timers of the blink and of the details window; they run as long as the UI does.
    _timers: Vec<Timer>,
}

#[derive(Default)]
struct State {
    /// The snapshot on screen.
    last: Option<Snapshot>,
    /// The element the details window shows, when it is open.
    details: Option<Target>,
}

/// The models of the panel. They are kept while the structure (tunnels and PCs, in order) is
/// unchanged, so that a publish only replaces rows.
#[derive(Default)]
struct Models {
    /// Per group: the tunnel id and the PC ids, which make up the structure.
    shape: Vec<(Option<String>, Vec<String>)>,
    groups: Option<Rc<VecModel<GroupRow>>>,
    pcs: Vec<Rc<VecModel<PcRow>>>,
}

/// Connects the views to the engine: fills the texts, sets the theme, wires the callbacks and
/// makes the UI state reachable from [`UiSink`]. Call on the event loop thread, before the engine
/// starts.
pub(crate) fn install(windows: &Windows<'_>, setup: Setup) {
    fill_strings(windows, &setup.translator);
    windows
        .panel
        .global::<Theme>()
        .set_dark(setup.appearance.dark);
    windows
        .details
        .global::<Theme>()
        .set_dark(setup.appearance.dark);
    let ui = Rc::new(Ui {
        panel: windows.panel.as_weak(),
        details: windows.details.as_weak(),
        tray: windows.tray.as_weak(),
        tr: setup.translator,
        handle: setup.handle,
        rdp: setup.rdp,
        clock: setup.clock,
        appearance: setup.appearance,
        state: RefCell::default(),
        models: RefCell::default(),
        blink: Cell::new(true),
        blurred_at: Cell::new(None),
        shown_at: Cell::new(None),
        _timers: start_timers(),
    });
    // Until the first snapshot: an unknown icon with no counts, not an empty tray icon.
    let none = Snapshot {
        groups: Vec::new(),
        tray: TrayState {
            screen: DisplayState::Unknown,
            corner: Attention::None,
            tunnels_up: 0,
            tunnels_total: 0,
            pcs_online: 0,
            pcs_total: 0,
        },
    };
    windows.tray.set_glyph(ui.glyph(&none));
    windows.tray.set_tip(ui.tooltip(&none).into());
    wire(windows, &ui);
    UI.with(|slot| *slot.borrow_mut() = Some(ui));
}

/// The blink of the busy markers and the refresh of the details window.
fn start_timers() -> Vec<Timer> {
    let blink = Timer::default();
    blink.start(TimerMode::Repeated, BLINK_HALF_PERIOD, || {
        with_ui(|ui| ui.toggle_blink());
    });
    let refresh = Timer::default();
    refresh.start(TimerMode::Repeated, DETAILS_REFRESH, || {
        with_ui(|ui| ui.refresh_details());
    });
    vec![blink, refresh]
}

fn fill_strings(windows: &Windows<'_>, tr: &Translator) {
    let s = windows.panel.global::<Strings>();
    s.set_no_tunnel(tr.t("panel.no_tunnel", &[]).into());
    s.set_empty(tr.t("panel.empty", &[]).into());
    s.set_settings(tr.t("panel.settings", &[]).into());
    s.set_wake(tr.t("action.wake", &[]).into());
    s.set_sleep(tr.t("action.sleep", &[]).into());
    s.set_sleep_confirm(tr.t("action.sleep_confirm", &[]).into());
    s.set_shutdown(tr.t("action.shutdown", &[]).into());
    s.set_shutdown_confirm(tr.t("action.shutdown_confirm", &[]).into());
    s.set_rdp(tr.t("action.rdp", &[]).into());
    let menu = windows.tray.global::<Strings>();
    menu.set_show_panel(tr.t("panel.show", &[]).into());
    menu.set_quit(tr.t("panel.quit", &[]).into());
}

fn wire(windows: &Windows<'_>, ui: &Rc<Ui>) {
    let nav = windows.panel.global::<Nav>();
    {
        let u = Rc::clone(ui);
        nav.on_tunnel_toggle(move |id| u.tunnel_clicked(&id));
    }
    {
        let u = Rc::clone(ui);
        nav.on_wake(move |id| u.send_pc(&id, Command::Wake));
    }
    {
        let u = Rc::clone(ui);
        nav.on_power(move |id, kind| u.power(&id, kind));
    }
    {
        let u = Rc::clone(ui);
        nav.on_rdp(move |id| u.open_rdp(&id));
    }
    {
        let u = Rc::clone(ui);
        nav.on_details(move |key| u.open_details(&key));
    }
    {
        let u = Rc::clone(ui);
        nav.on_hide_requested(move || u.hide());
    }
    {
        // Deferred: the height changes while Slint is laying out the panel.
        nav.on_layout_changed(|| {
            let _ = slint::invoke_from_event_loop(|| with_ui(|ui| ui.place_panel()));
        });
    }
    {
        let u = Rc::clone(ui);
        windows.details.on_close_requested(move || u.hide_details());
    }
    {
        // Deferred, as the panel's: the height changes while Slint is laying out the window.
        windows.details.on_layout_changed(|| {
            let _ = slint::invoke_from_event_loop(|| with_ui(|ui| ui.place_details()));
        });
    }
    windows.panel.window().on_winit_window_event(|_, event| {
        if let WindowEvent::Focused(false) = event {
            // Deferred, as the prototype did: the focus change may come from inside a
            // window event.
            let _ = slint::invoke_from_event_loop(|| with_ui(|ui| ui.focus_lost()));
        }
        EventResult::Propagate
    });
    {
        let u = Rc::clone(ui);
        windows.tray.on_toggle_panel(move || u.toggle_panel());
    }
    {
        let handle = ui.handle.clone();
        windows.tray.on_quit_requested(move || {
            handle.stop();
            let _ = slint::quit_event_loop();
        });
    }
}

impl Ui {
    /// Rebuilds the views from a snapshot and remembers it.
    fn show(&self, snapshot: &Snapshot) {
        self.state.borrow_mut().last = Some(snapshot.clone());
        let (Some(panel), Some(tray)) = (self.panel.upgrade(), self.tray.upgrade()) else {
            return;
        };
        tray.set_glyph(self.glyph(snapshot));
        tray.set_tip(self.tooltip(snapshot).into());
        self.fill_groups(&panel, snapshot);
        self.check_submenu(&panel, snapshot);
        self.refresh_details();
        if panel.window().is_visible() {
            platform::place_panel(&panel);
        }
    }

    /// The tray icon of a snapshot, drawn for the taskbar theme.
    fn glyph(&self, snapshot: &Snapshot) -> slint::Image {
        let bitmap = icon::render(
            snapshot.tray.screen,
            snapshot.tray.corner,
            self.appearance.taskbar_dark,
            self.appearance.icon_px,
        );
        icon::to_image(&bitmap)
    }

    /// Sets the groups of the panel. The models are rebuilt only when the structure changed.
    fn fill_groups(&self, panel: &Panel, snapshot: &Snapshot) {
        let shape = shape(snapshot);
        let mut models = self.models.borrow_mut();
        if models.groups.is_none() || models.shape != shape {
            let pcs: Vec<Rc<VecModel<PcRow>>> = snapshot
                .groups
                .iter()
                .map(|group| {
                    let rows: Vec<PcRow> = group.pcs.iter().map(pc_row).collect();
                    Rc::new(VecModel::from(rows))
                })
                .collect();
            let rows: Vec<GroupRow> = snapshot
                .groups
                .iter()
                .zip(&pcs)
                .map(|(group, pc_model)| self.group_row(group, pc_model))
                .collect();
            let groups = Rc::new(VecModel::from(rows));
            panel.set_groups(ModelRc::new(Rc::clone(&groups)));
            models.shape = shape;
            models.groups = Some(groups);
            models.pcs = pcs;
            return;
        }
        let Some(groups) = models.groups.clone() else {
            return;
        };
        for (index, (group, pc_model)) in snapshot.groups.iter().zip(&models.pcs).enumerate() {
            for (pc_index, pc) in group.pcs.iter().enumerate() {
                pc_model.set_row_data(pc_index, pc_row(pc));
            }
            // Only a change of the group's own fields replaces its row; a publish that changes
            // only PC rows leaves the group row, and so the tunnel element, alone.
            let row = self.group_row(group, pc_model);
            let changed = groups
                .row_data(index)
                .is_none_or(|old| old.has_tunnel != row.has_tunnel || old.tunnel != row.tunnel);
            if changed {
                groups.set_row_data(index, row);
            }
        }
    }

    fn group_row(&self, group: &GroupView, pcs: &Rc<VecModel<PcRow>>) -> GroupRow {
        let tunnel = group
            .tunnel
            .as_ref()
            .map_or_else(TunnelRow::default, |t| self.tunnel_row(t));
        GroupRow {
            has_tunnel: group.tunnel.is_some(),
            tunnel,
            pcs: ModelRc::new(Rc::clone(pcs)),
        }
    }

    fn tunnel_row(&self, tunnel: &TunnelView) -> TunnelRow {
        let key = match tunnel.toggle {
            TunnelToggle::Raise => "action.tunnel_raise",
            TunnelToggle::Lower => "action.tunnel_lower",
        };
        TunnelRow {
            id: tunnel.id.to_string().into(),
            name: tunnel.name.clone().into(),
            mark: mark(tunnel.display),
            // A tunnel always has its toggle; it is never shown as inactive.
            inactive: false,
            toggle_on: tunnel.can_toggle,
            toggle_label: self.tr.t(key, &[]).into(),
        }
    }

    /// Closes the submenu of an element that is gone, and cancels a confirmation whose action
    /// is no longer enabled.
    fn check_submenu(&self, panel: &Panel, snapshot: &Snapshot) {
        let nav = panel.global::<Nav>();
        let expanded = nav.get_expanded();
        if expanded.is_empty() {
            return;
        }
        let Some(target) = Target::from_key(&expanded) else {
            nav.invoke_close_submenu();
            return;
        };
        if !exists(snapshot, &target) {
            nav.invoke_close_submenu();
            return;
        }
        let armed = nav.get_armed();
        if armed.is_empty() {
            return;
        }
        let still_enabled = match (&target, armed_action(&armed)) {
            (Target::Pc(pc), Some(action)) => action_enabled(snapshot, pc, action),
            _ => false,
        };
        if !still_enabled {
            nav.set_armed("".into());
        }
    }

    fn tooltip(&self, snapshot: &Snapshot) -> String {
        let tray = &snapshot.tray;
        let (up, tunnels) = (tray.tunnels_up.to_string(), tray.tunnels_total.to_string());
        let (online, pcs) = (tray.pcs_online.to_string(), tray.pcs_total.to_string());
        let lines = [
            self.tr.t(
                "tooltip.tunnels_up",
                &[("up", up.as_str()), ("total", tunnels.as_str())],
            ),
            self.tr.t(
                "tooltip.pcs_online",
                &[("online", online.as_str()), ("total", pcs.as_str())],
            ),
        ];
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

    /// Sleep or shutdown, once the panel has asked for the confirmation. The action is checked
    /// against the last snapshot again, so that a stale button does nothing.
    fn power(&self, id: &str, kind: PowerKind) {
        let action = match kind {
            PowerKind::Sleep => PowerAction::Sleep,
            PowerKind::Shutdown => PowerAction::Shutdown,
        };
        let Ok(pc) = PcId::parse(id) else {
            return;
        };
        let enabled = self
            .state
            .borrow()
            .last
            .as_ref()
            .is_some_and(|snapshot| action_enabled(snapshot, &pc, action));
        if enabled {
            self.handle.send(Command::Power(pc, action));
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

    /// Opens the details window for an element, or shows the new element in it when it is open.
    fn open_details(&self, key: &str) {
        let Some(target) = Target::from_key(key) else {
            return;
        };
        self.state.borrow_mut().details = Some(target);
        let Some(details) = self.details.upgrade() else {
            return;
        };
        self.refresh_details();
        if self.state.borrow().details.is_none() {
            return;
        }
        let _ = details.show();
        self.place_details();
        details.global::<Motion>().set_blink_on(self.blink.get());
        // The Escape key of the window needs the window to have the focus of the system.
        let _ = details.window().with_winit_window(|w| w.focus_window());
        details.invoke_focus_window();
    }

    /// Places the details window again: after it shows and after its height changes.
    fn place_details(&self) {
        let (Some(panel), Some(details)) = (self.panel.upgrade(), self.details.upgrade()) else {
            return;
        };
        if details.window().is_visible() {
            position_details(&panel, &details);
        }
    }

    /// Closes the details window; the element it showed is forgotten.
    fn hide_details(&self) {
        self.state.borrow_mut().details = None;
        if let Some(details) = self.details.upgrade() {
            let _ = details.hide();
        }
    }

    /// Fills the details window from the last snapshot. It closes when its element is gone.
    fn refresh_details(&self) {
        let Some(details) = self.details.upgrade() else {
            return;
        };
        let Some(target) = self.state.borrow().details.clone() else {
            return;
        };
        let view = self
            .state
            .borrow()
            .last
            .as_ref()
            .and_then(|snapshot| self.details_view(&target, snapshot));
        match view {
            Some((name, mark, rows)) => {
                details.set_name(name.into());
                details.set_mark(mark);
                details.set_rows(ModelRc::new(VecModel::from(rows)));
            }
            None => self.hide_details(),
        }
    }

    /// The name, the marker and the rows of the details of one element (section 6 of the
    /// handoff); `None` when the element is not in the snapshot.
    fn details_view(
        &self,
        target: &Target,
        snapshot: &Snapshot,
    ) -> Option<(String, Mark, Vec<DetailRow>)> {
        match target {
            Target::Tunnel(id) => {
                let tunnel = snapshot
                    .groups
                    .iter()
                    .filter_map(|group| group.tunnel.as_ref())
                    .find(|t| t.id == *id)?;
                let pcs = if tunnel.pcs.is_empty() {
                    self.none()
                } else {
                    tunnel.pcs.join(", ")
                };
                let (error, fail) = self.error(tunnel.last_error.as_ref());
                let rows = vec![
                    self.row("details.state", self.tr.state(tunnel.display), true, false),
                    self.row("details.kind", tunnel.kind.to_owned(), false, false),
                    self.row("details.pcs", pcs, false, false),
                    self.row(
                        "details.last_check",
                        self.age(tunnel.last_checked),
                        false,
                        false,
                    ),
                    self.row("details.last_error", error, false, fail),
                ];
                Some((tunnel.name.clone(), mark(tunnel.display), rows))
            }
            Target::Pc(id) => {
                let pc = snapshot
                    .groups
                    .iter()
                    .flat_map(|group| group.pcs.iter())
                    .find(|p| p.id == *id)?;
                let wake = pc
                    .wake
                    .as_ref()
                    .map_or_else(|| self.none(), |w: &WakeSummary| self.tr.wake_summary(w));
                let ports = if pc.ports.is_empty() {
                    self.none()
                } else {
                    pc.ports
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let (error, fail) = self.error(pc.last_error.as_ref());
                let rows = vec![
                    self.row("details.state", self.tr.state(pc.display), true, false),
                    self.row("details.address", pc.address.clone(), false, false),
                    self.row(
                        "details.tunnel",
                        pc.tunnel_name.clone().unwrap_or_else(|| self.none()),
                        false,
                        false,
                    ),
                    self.row("details.wake", wake, false, false),
                    self.row("details.ports", ports, false, false),
                    self.row(
                        "details.last_check",
                        self.age(pc.last_checked),
                        false,
                        false,
                    ),
                    self.row("details.last_error", error, false, fail),
                ];
                Some((pc.name.clone(), mark(pc.display), rows))
            }
        }
    }

    fn row(&self, label_key: &str, value: String, strong: bool, fail: bool) -> DetailRow {
        DetailRow {
            label: self.tr.t(label_key, &[]).into(),
            value: value.into(),
            strong,
            fail,
        }
    }

    /// The word for "none" (`details.none`).
    fn none(&self) -> String {
        self.tr.t("details.none", &[])
    }

    /// The text of the last error and whether it is one: its title and body, or "none".
    fn error(&self, notice: Option<&Notice>) -> (String, bool) {
        match notice {
            Some(notice) => {
                let (title, body) = self.tr.notice(notice);
                (format!("{title}\n{body}"), true)
            }
            None => (self.none(), false),
        }
    }

    /// The age of the last check on the engine's clock, such as "12 s ago"; "none" before it.
    fn age(&self, checked: Option<Time>) -> String {
        match checked {
            None => self.none(),
            Some(at) => {
                let (key, count) = age_parts(self.clock.now().since(at));
                self.tr.t(key, &[("count", count.to_string().as_str())])
            }
        }
    }

    /// Blinks the busy markers and shows the new phase in both windows.
    fn toggle_blink(&self) {
        let on = !self.blink.get();
        self.blink.set(on);
        if let Some(panel) = self.panel.upgrade() {
            panel.global::<Motion>().set_blink_on(on);
        }
        if let Some(details) = self.details.upgrade() {
            details.global::<Motion>().set_blink_on(on);
        }
        crate::manage::with_manage(|m| m.set_blink(on));
    }

    /// Places the panel again after its height changed.
    fn place_panel(&self) {
        if let Some(panel) = self.panel.upgrade()
            && panel.window().is_visible()
        {
            platform::place_panel(&panel);
        }
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

    /// Hides the panel. The open submenu and any confirmation are dropped with it. The details
    /// window stays open: it is a window of its own.
    fn hide(&self) {
        if let Some(panel) = self.panel.upgrade() {
            panel.global::<Nav>().invoke_reset();
            let _ = panel.hide();
        }
    }
}

/// The structure of the groups: the tunnel and the PC ids of each group, in order.
fn shape(snapshot: &Snapshot) -> Vec<(Option<String>, Vec<String>)> {
    snapshot
        .groups
        .iter()
        .map(|group| {
            (
                group.tunnel.as_ref().map(|t| t.id.to_string()),
                group.pcs.iter().map(|pc| pc.id.to_string()).collect(),
            )
        })
        .collect()
}

fn exists(snapshot: &Snapshot, target: &Target) -> bool {
    match target {
        Target::Tunnel(id) => snapshot
            .groups
            .iter()
            .any(|group| group.tunnel.as_ref().is_some_and(|t| t.id == *id)),
        Target::Pc(id) => snapshot
            .groups
            .iter()
            .flat_map(|group| group.pcs.iter())
            .any(|pc| pc.id == *id),
    }
}

/// The power action a confirmation is waiting for, by the name the panel uses.
fn armed_action(armed: &str) -> Option<PowerAction> {
    match armed {
        "sleep" => Some(PowerAction::Sleep),
        "shutdown" => Some(PowerAction::Shutdown),
        _ => None,
    }
}

/// Whether the action is enabled for the PC in the snapshot.
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

/// The marker of a display state.
pub(crate) fn mark(display: DisplayState) -> Mark {
    match display {
        DisplayState::Ok => Mark::Ok,
        DisplayState::Busy => Mark::Busy,
        DisplayState::Off => Mark::Off,
        DisplayState::Unknown => Mark::Unknown,
        DisplayState::Fail => Mark::Fail,
    }
}

fn pc_row(pc: &PcView) -> PcRow {
    let c = pc.configured;
    PcRow {
        id: pc.id.to_string().into(),
        name: pc.name.clone().into(),
        mark: mark(pc.display),
        // Nothing is configured for this PC at all: its name is shown as inactive.
        inactive: !(c.wake || c.sleep || c.shutdown || c.rdp),
        wake_on: pc.actions.wake,
        sleep_on: pc.actions.sleep,
        shutdown_on: pc.actions.shutdown,
        rdp_on: pc.actions.rdp,
    }
}

/// The unit and the count of an age: seconds up to a minute, minutes up to an hour, then hours.
fn age_parts(elapsed: Duration) -> (&'static str, u64) {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        ("details.age_seconds", seconds)
    } else if seconds < 3600 {
        ("details.age_minutes", seconds / 60)
    } else {
        ("details.age_hours", seconds / 3600)
    }
}

/// Places the details window to the left of the panel, with its bottom edge at the bottom edge
/// of the panel, so that it grows upwards and stays above the taskbar.
fn position_details(panel: &Panel, details: &DetailsWindow) {
    let scale = panel.window().scale_factor();
    let origin = panel.window().position();
    let panel_bottom = origin.y + (panel.get_content_height() * scale).round() as i32;
    let height = (details.get_content_height() * scale).round() as i32;
    let width = (DETAILS_WIDTH * scale).round() as i32;
    let gap = (DETAILS_GAP * scale).round() as i32;
    details.window().set_position(slint::PhysicalPosition::new(
        origin.x - width - gap,
        panel_bottom - height,
    ));
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

    #[test]
    fn an_age_is_shown_in_the_largest_whole_unit() {
        assert_eq!(
            age_parts(Duration::from_secs(12)),
            ("details.age_seconds", 12)
        );
        assert_eq!(
            age_parts(Duration::from_secs(59)),
            ("details.age_seconds", 59)
        );
        assert_eq!(
            age_parts(Duration::from_secs(60)),
            ("details.age_minutes", 1)
        );
        assert_eq!(
            age_parts(Duration::from_secs(3599)),
            ("details.age_minutes", 59)
        );
        assert_eq!(
            age_parts(Duration::from_secs(7200)),
            ("details.age_hours", 2)
        );
    }

    #[test]
    fn a_target_is_a_tunnel_or_a_pc_by_its_key() {
        assert_eq!(
            Target::from_key("t:lab"),
            TunnelId::parse("lab").ok().map(Target::Tunnel)
        );
        assert_eq!(
            Target::from_key("p:build"),
            PcId::parse("build").ok().map(Target::Pc)
        );
        assert_eq!(Target::from_key("build"), None);
        assert_eq!(Target::from_key("p:"), None);
    }

    #[test]
    fn an_armed_action_is_one_of_the_two_power_actions() {
        assert_eq!(armed_action("sleep"), Some(PowerAction::Sleep));
        assert_eq!(armed_action("shutdown"), Some(PowerAction::Shutdown));
        assert_eq!(armed_action(""), None);
    }
}

// HomeLink Rust/Slint prototype (throwaway): tray icon + a small panel with sample data.
//
// LICENCE REMINDER: Slint is used under its royalty-free licence, which requires
// attribution in the application (e.g. an About window or documentation stating that
// the app is built with Slint, with a link to https://slint.dev). The prototype has no
// About window; add one before any real release.
#![windows_subsystem = "windows"]

mod model;

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::io::Write;
use std::rc::Rc;
use std::time::{Duration, Instant};

use model::{Group, PcAction, UserAction};
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::ComponentHandle;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

slint::include_modules!();

const PANEL_WIDTH: f32 = 420.0;
const MARGIN: f32 = 8.0;

fn log_line(line: &str) {
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.with_file_name("homelink-prototype.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Work area of the primary monitor in physical pixels.
fn work_area() -> RECT {
    let mut rect = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut RECT as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    rect
}

/// Puts the panel in the bottom-right corner of the work area.
fn place(panel: &Panel) {
    let scale = panel.window().scale_factor();
    let w = (PANEL_WIDTH * scale).round() as i32;
    let h = (panel.get_content_height() * scale).round() as i32;
    let m = (MARGIN * scale).round() as i32;
    let area = work_area();
    panel
        .window()
        .set_position(slint::PhysicalPosition::new(area.right - w - m, area.bottom - h - m));
}

fn main() -> Result<(), Box<dyn Error>> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .with_winit_window_attributes_hook(|attrs| {
            attrs.with_skip_taskbar(true).with_transparent(true)
        })
        .select()?;

    let groups: Rc<RefCell<Vec<Group>>> = Rc::new(RefCell::new(model::sample()));
    let panel = Panel::new()?;
    panel.set_groups(model::to_ui(&groups.borrow()));

    // One place that applies an action, logs it and refreshes the view.
    let dispatch: Rc<dyn Fn(UserAction)> = {
        let groups = groups.clone();
        let weak = panel.as_weak();
        Rc::new(move |action| {
            let line = model::apply(&mut groups.borrow_mut(), action);
            if let Some(line) = line {
                log_line(&line);
            }
            if let Some(p) = weak.upgrade() {
                p.set_groups(model::to_ui(&groups.borrow()));
                place(&p);
            }
        })
    };
    {
        let d = dispatch.clone();
        panel.on_tunnel_clicked(move |g| d(UserAction::ToggleTunnel(g as usize)));
        let d = dispatch.clone();
        panel.on_pc_clicked(move |g, p| d(UserAction::ToggleExpand(g as usize, p as usize)));
        let d = dispatch.clone();
        panel.on_action_clicked(move |g, p, a| {
            if let Some(act) = PcAction::from_index(a) {
                d(UserAction::Run(g as usize, p as usize, act));
            }
        });
    }
    {
        let weak = panel.as_weak();
        panel.on_hide_requested(move || {
            if let Some(p) = weak.upgrade() {
                let _ = p.hide();
            }
        });
    }

    // Hide on focus loss (public Slint winit API). Remember when it happened, so that a
    // click on the tray icon that caused the focus loss does not reopen the panel.
    let blurred_at: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    {
        let blurred_at = blurred_at.clone();
        let weak = panel.as_weak();
        panel.window().on_winit_window_event(move |_, event| {
            if let WindowEvent::Focused(false) = event {
                blurred_at.set(Some(Instant::now()));
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(p) = weak.upgrade() {
                        let _ = p.hide();
                    }
                });
            }
            EventResult::Propagate
        });
    }

    let tray = Tray::new()?;
    tray.on_quit_requested(|| {
        let _ = slint::quit_event_loop();
    });
    {
        let weak = panel.as_weak();
        tray.on_toggle_panel(move || {
            let Some(p) = weak.upgrade() else { return };
            if p.window().is_visible() {
                let _ = p.hide();
                return;
            }
            if blurred_at.get().is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
                return;
            }
            place(&p);
            let _ = p.show();
            p.invoke_focus_panel();
            p.window().with_winit_window(|w| w.focus_window());
        });
    }
    tray.show()?;

    slint::run_event_loop_until_quit()?;
    Ok(())
}

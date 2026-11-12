//! Platform hooks of the shell: the backend, the panel's place on screen and the message box
//! for startup errors. The Windows calls are in the `win32` module behind `cfg(windows)`;
//! other platforms get plain versions, so the crate also builds for checks on Linux.

use crate::view::Panel;
#[cfg(windows)]
use slint::ComponentHandle;

/// Width of the panel in logical pixels.
#[cfg(windows)]
const PANEL_WIDTH: f32 = 420.0;
/// Gap between the panel and the corner of the work area, in logical pixels.
#[cfg(windows)]
const MARGIN: f32 = 8.0;

/// Selects the winit backend with the software renderer, as the prototype did. On Windows the
/// panel is kept off the taskbar.
pub(crate) fn select_backend() -> Result<(), slint::PlatformError> {
    let selector = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into());
    #[cfg(windows)]
    let selector = selector.with_winit_window_attributes_hook(|attrs| {
        use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
        attrs.with_skip_taskbar(true)
    });
    selector.select()
}

/// Puts the panel in the bottom-right corner of the work area.
#[cfg(windows)]
pub(crate) fn place_panel(panel: &Panel) {
    let scale = panel.window().scale_factor();
    let w = (PANEL_WIDTH * scale).round() as i32;
    let h = (panel.get_content_height() * scale).round() as i32;
    let m = (MARGIN * scale).round() as i32;
    let area = win32::work_area();
    panel.window().set_position(slint::PhysicalPosition::new(
        area.right - w - m,
        area.bottom - h - m,
    ));
}

/// Other platforms have no work area to anchor to; the window manager places the panel.
#[cfg(not(windows))]
pub(crate) fn place_panel(_panel: &Panel) {}

/// Shows an error the user must see before the program exits. The Windows build has no
/// console, so this is a message box there and standard error elsewhere.
pub(crate) fn message(text: &str) {
    #[cfg(windows)]
    win32::message_box(text);
    #[cfg(not(windows))]
    eprintln!("HomeLink: {text}");
}

#[cfg(windows)]
#[allow(unsafe_code)] // The only unsafe code of the app: the Win32 calls below.
mod win32 {
    use std::ffi::c_void;

    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MessageBoxW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        SystemParametersInfoW,
    };
    use windows::core::{HSTRING, w};

    /// The work area of the primary monitor in physical pixels (without the taskbar).
    pub(super) fn work_area() -> RECT {
        let mut rect = RECT::default();
        // SAFETY: SPI_GETWORKAREA writes one RECT into the buffer, and `rect` is a live RECT
        // for the duration of the call.
        unsafe {
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut rect as *mut RECT as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
        }
        rect
    }

    /// Shows an error box with no owner window.
    pub(super) fn message_box(text: &str) {
        let text = HSTRING::from(text);
        // SAFETY: both strings live until the call returns; a null owner window is allowed.
        unsafe {
            let _ = MessageBoxW(None, &text, w!("HomeLink"), MB_OK | MB_ICONERROR);
        }
    }
}

//! Platform hooks of the shell: the backend, the panel's place on screen and the message box
//! for startup errors. The Windows calls are in the `win32` module behind `cfg(windows)`;
//! other platforms get plain versions, so the crate also builds for checks on Linux.

use std::time::Duration;

use slint::ComponentHandle;

use homelink_core::notice::Severity;
use slint::winit_030::WinitWindowAccessor;

use crate::balloon::Balloon;
use crate::view::Panel;

/// Width of the panel in logical pixels.
const PANEL_WIDTH: f32 = 420.0;
/// Gap between the panel and the corner of the work area, in logical pixels.
const MARGIN: f32 = 8.0;

/// After the panel is shown, a focus loss within this time does not hide it. Some desktops
/// move the focus once more right after a tray menu closes, which would hide a panel that was
/// just opened from that menu. Windows has no such pass and keeps the plain focus-loss rule.
pub(crate) const SHOW_FOCUS_GRACE: Duration = if cfg!(windows) {
    Duration::ZERO
} else {
    Duration::from_millis(500)
};

/// Selects the winit backend with the software renderer, as the prototype did.
///
/// No window attribute is set for every window: the window attributes hook of Slint is called
/// for each window alike and cannot tell the panel from the settings window, so which windows
/// stay off the taskbar is decided per window by [`hide_from_taskbar`].
pub(crate) fn select_backend() -> Result<(), slint::PlatformError> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()
}

/// Keeps `window` off the Windows taskbar. Only the tray panel and the details window call it:
/// the settings window, the wizard and the tunnel form belong on the taskbar. Call it after
/// the window is shown (the native window exists then); it can be repeated and does nothing
/// when the window does not exist yet. Other platforms have no such button to hide.
pub(crate) fn hide_from_taskbar(window: &slint::Window) {
    #[cfg(windows)]
    {
        use slint::winit_030::winit::platform::windows::WindowExtWindows;
        let _ = window.with_winit_window(|w| w.set_skip_taskbar(true));
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// Shows a tray balloon with this title and body, in the system's notification style for the
/// severity. Call on the event loop thread (the window that owns the balloon icon must be on a
/// thread that pumps messages). Off Windows there is no balloon: the notice is in the log.
pub(crate) fn show_balloon(title: &str, body: &str, severity: Severity) {
    let balloon = Balloon::new(title, body, severity);
    #[cfg(windows)]
    if !win32::show_balloon(&balloon) {
        log::warn!("the tray balloon could not be shown");
    }
    #[cfg(not(windows))]
    let _ = balloon;
}

/// Removes the icon that carries the balloons. Call once, when the event loop has ended.
pub(crate) fn remove_balloons() {
    #[cfg(windows)]
    win32::remove_balloon_icon();
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

/// Puts the panel in the bottom-right corner of the monitor it is on (or of the primary
/// monitor). Unlike the Windows version it does not leave room for a taskbar, which this
/// code cannot see. Does nothing while the window does not exist yet, so it is called again
/// after the panel is shown; Wayland ignores positions and keeps the compositor's placement.
#[cfg(not(windows))]
pub(crate) fn place_panel(panel: &Panel) {
    let scale = panel.window().scale_factor();
    let w = (PANEL_WIDTH * scale).round() as i32;
    let h = (panel.get_content_height() * scale).round() as i32;
    let m = (MARGIN * scale).round() as i32;
    let corner = panel
        .window()
        .with_winit_window(|window| {
            let monitor = window
                .current_monitor()
                .or_else(|| window.primary_monitor())?;
            let origin = monitor.position();
            let size = monitor.size();
            Some((origin.x + size.width as i32, origin.y + size.height as i32))
        })
        .flatten();
    if let Some((right, bottom)) = corner {
        panel
            .window()
            .set_position(slint::PhysicalPosition::new(right - w - m, bottom - h - m));
    }
}

/// Shows an error the user must see. The Windows build has no console, so this is a message
/// box there and standard error elsewhere.
pub(crate) fn message(text: &str) {
    show(text, false);
}

/// Shows information the user must see (a message box with the information icon).
pub(crate) fn info(text: &str) {
    show(text, true);
}

fn show(text: &str, information: bool) {
    #[cfg(windows)]
    win32::message_box(text, information);
    #[cfg(not(windows))]
    {
        let _ = information;
        eprintln!("HomeLink: {text}");
    }
}

#[cfg(windows)]
#[allow(unsafe_code)] // The only unsafe code of the app: the Win32 calls below.
mod win32 {
    use std::cell::Cell;
    use std::ffi::c_void;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_STATE, NIIF_ERROR, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE,
        NIM_MODIFY, NIS_HIDDEN, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_MESSAGE, IDI_APPLICATION, LoadIconW,
        MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW, RegisterClassW, SPI_GETWORKAREA,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW, WINDOW_EX_STYLE, WINDOW_STYLE,
        WNDCLASSW,
    };
    use windows::core::{HSTRING, PCWSTR, w};

    use crate::balloon::{self, Balloon, Icon};

    /// The id of the notify icon that carries the balloons. Slint's tray icon has its own
    /// window and id (1); the two do not meet.
    const BALLOON_ICON_ID: u32 = 0x484C;

    thread_local! {
        /// The hidden message-only window that owns the balloon icon; made on first use, on the
        /// event loop thread, which pumps its messages.
        static BALLOON_WINDOW: Cell<Option<HWND>> = const { Cell::new(None) };
    }

    /// The window procedure of the balloon window: it needs no behaviour of its own.
    unsafe extern "system" fn balloon_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: forwards the arguments Windows gave to the default procedure.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: BALLOON_ICON_ID,
            ..Default::default()
        }
    }

    /// The balloon window, made (with its hidden notify icon) when it does not exist yet.
    /// A hidden icon shows no symbol in the tray; the balloons come from it all the same.
    fn balloon_window() -> Option<HWND> {
        if let Some(hwnd) = BALLOON_WINDOW.get() {
            return Some(hwnd);
        }
        // SAFETY: plain Win32 calls on this thread; every pointer passed is to a value that
        // lives until the call returns, and the window is destroyed again if the icon cannot
        // be added.
        unsafe {
            let hinstance = GetModuleHandleW(None).ok()?;
            let class = w!("HomeLinkBalloonWindow");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(balloon_proc),
                hInstance: hinstance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            // A second registration after a failure returns 0 and does no harm.
            let _ = RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinstance.into()),
                None,
            )
            .ok()?;
            let data = NOTIFYICONDATAW {
                uFlags: NIF_ICON | NIF_STATE,
                hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
                dwState: NIS_HIDDEN,
                dwStateMask: NIS_HIDDEN,
                ..icon_data(hwnd)
            };
            if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                let _ = DestroyWindow(hwnd);
                return None;
            }
            BALLOON_WINDOW.set(Some(hwnd));
            Some(hwnd)
        }
    }

    /// Shows the balloon; `false` when Windows did not take it.
    pub(super) fn show_balloon(balloon: &Balloon) -> bool {
        let Some(hwnd) = balloon_window() else {
            return false;
        };
        let data = NOTIFYICONDATAW {
            uFlags: NIF_INFO,
            szInfoTitle: balloon::fill(&balloon.title),
            szInfo: balloon::fill(&balloon.body),
            dwInfoFlags: match balloon.icon {
                Icon::Info => NIIF_INFO,
                Icon::Warning => NIIF_WARNING,
                Icon::Error => NIIF_ERROR,
            },
            ..icon_data(hwnd)
        };
        // SAFETY: `data` is a live NOTIFYICONDATAW with its size set, for the duration of the call.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data).as_bool() }
    }

    /// Deletes the notify icon and its window, if they were made.
    pub(super) fn remove_balloon_icon() {
        if let Some(hwnd) = BALLOON_WINDOW.take() {
            // SAFETY: `hwnd` is the window made by `balloon_window` on this thread, and the data
            // names the icon added for it.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
                let _ = DestroyWindow(hwnd);
            }
        }
    }

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

    /// Shows a message box with no owner window.
    pub(super) fn message_box(text: &str, information: bool) {
        let text = HSTRING::from(text);
        let icon = if information {
            MB_ICONINFORMATION
        } else {
            MB_ICONERROR
        };
        // SAFETY: both strings live until the call returns; a null owner window is allowed.
        unsafe {
            let _ = MessageBoxW(None, &text, w!("HomeLink"), MB_OK | icon);
        }
    }
}

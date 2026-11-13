//! The colours of the panel and of the tray icon, chosen once at startup: the `--theme` option,
//! the Windows application theme and taskbar theme, and the display scale for the tray icon.
//! Everything is read on Windows only (through the registry helper); elsewhere the light theme
//! is used. The panel and the icon do not follow later changes until the next start.

use homelink_core::ports::CommandRunner;

use crate::windows::theme;

/// The `--theme` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeSetting {
    /// The Windows application theme (light elsewhere).
    Auto,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl ThemeSetting {
    /// Parses `auto`, `light` or `dark`.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

/// What the screens are drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Appearance {
    /// The panel and the details window use the dark theme.
    pub(crate) dark: bool,
    /// The taskbar is dark, so the tray icon uses its dark palette.
    pub(crate) taskbar_dark: bool,
    /// The side of the tray icon in pixels: 16, 24 or 32.
    pub(crate) icon_px: u32,
}

/// The display scale assumed when the registry does not say (100 %).
const DEFAULT_DPI: u32 = 96;

/// The icon side for a display scale: 16 px at 100 %, 24 at 150 %, 32 at 200 % (the nearest of
/// the three to the scaled 16).
pub(crate) fn icon_px(dpi: u32) -> u32 {
    if dpi <= 120 {
        16
    } else if dpi <= 168 {
        24
    } else {
        32
    }
}

/// The appearance for `setting`, reading the Windows values through `runner`.
pub(crate) fn detect(setting: ThemeSetting, runner: &dyn CommandRunner) -> Appearance {
    let (apps_light, taskbar_light, dpi) = if cfg!(windows) {
        (
            theme::light_theme(runner, "AppsUseLightTheme"),
            theme::light_theme(runner, "SystemUsesLightTheme"),
            theme::applied_dpi(runner).or_else(|| theme::log_pixels(runner)),
        )
    } else {
        (None, None, None)
    };
    let dark = match setting {
        ThemeSetting::Light => false,
        ThemeSetting::Dark => true,
        ThemeSetting::Auto => apps_light == Some(false),
    };
    Appearance {
        dark,
        taskbar_dark: taskbar_light == Some(false),
        icon_px: icon_px(dpi.unwrap_or(DEFAULT_DPI)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use homelink_core::ports::CommandOutput;
    use homelink_core::testing::FakeCommandRunner;

    #[test]
    fn the_theme_option_has_three_values() {
        assert_eq!(ThemeSetting::parse("auto"), Some(ThemeSetting::Auto));
        assert_eq!(ThemeSetting::parse("light"), Some(ThemeSetting::Light));
        assert_eq!(ThemeSetting::parse("dark"), Some(ThemeSetting::Dark));
        assert_eq!(ThemeSetting::parse("Dark"), None);
        assert_eq!(ThemeSetting::parse("blue"), None);
    }

    #[test]
    fn the_icon_follows_the_scale() {
        assert_eq!(icon_px(96), 16);
        assert_eq!(icon_px(144), 24);
        assert_eq!(icon_px(192), 32);
        assert_eq!(icon_px(0), 16);
    }

    #[test]
    fn an_explicit_theme_ignores_the_registry() {
        let runner = FakeCommandRunner::default();
        runner.set_result(CommandOutput::exited(0, "    X    REG_DWORD    0x0\n"));
        let dark = detect(ThemeSetting::Dark, &runner);
        assert!(dark.dark);
        let light = detect(ThemeSetting::Light, &runner);
        assert!(!light.dark);
        assert_eq!(light.icon_px, 16);
    }

    #[test]
    fn auto_is_light_when_nothing_can_be_read() {
        let runner = FakeCommandRunner::default();
        runner.set_result(CommandOutput::not_started("no reg"));
        let appearance = detect(ThemeSetting::Auto, &runner);
        assert!(!appearance.dark);
        assert!(!appearance.taskbar_dark);
        assert_eq!(appearance.icon_px, 16);
    }
}

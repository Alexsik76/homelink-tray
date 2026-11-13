//! The Windows settings that decide the colours: the application theme, the taskbar theme and
//! the scale of the small icons, read once at startup through the registry helper ([`reg`]).
//!
//! Values are DWORDs, which `reg query` prints in hexadecimal (`0x1`).

use homelink_core::ports::CommandRunner;

use super::reg;

/// Where the personalisation values are.
const PERSONALIZE: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
/// Where the display scale (in dots per inch, 96 = 100 %) is.
const DESKTOP: &str = r"HKCU\Control Panel\Desktop";
/// Where the DPI applied to the window metrics is, when it is written.
const WINDOW_METRICS: &str = r"HKCU\Control Panel\Desktop\WindowMetrics";

/// A DWORD as printed by `reg query`, such as `0x1`.
pub(crate) fn parse_dword(data: &str) -> Option<u32> {
    let hex = data.trim().strip_prefix("0x")?;
    u32::from_str_radix(hex, 16).ok()
}

/// Whether a `…UseLightTheme` value is set, that is `1`. `None` when it cannot be read.
pub(crate) fn light_theme(runner: &dyn CommandRunner, value: &str) -> Option<bool> {
    reg::query(runner, PERSONALIZE, value)
        .and_then(|found| parse_dword(&found.data))
        .map(|flag| flag != 0)
}

/// The display scale in dots per inch, `None` when it is not set.
pub(crate) fn log_pixels(runner: &dyn CommandRunner) -> Option<u32> {
    reg::query(runner, DESKTOP, "LogPixels").and_then(|found| parse_dword(&found.data))
}

/// The DPI the system applied, when the window metrics have it (unverified on Windows; the
/// caller falls back to [`log_pixels`]).
pub(crate) fn applied_dpi(runner: &dyn CommandRunner) -> Option<u32> {
    reg::query(runner, WINDOW_METRICS, "AppliedDPI").and_then(|found| parse_dword(&found.data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use homelink_core::ports::CommandOutput;
    use homelink_core::testing::FakeCommandRunner;

    #[test]
    fn dwords_are_read_from_their_hexadecimal_form() {
        assert_eq!(parse_dword("0x1"), Some(1));
        assert_eq!(parse_dword("0x0"), Some(0));
        assert_eq!(parse_dword("0x60"), Some(96));
        assert_eq!(parse_dword("  0x90 "), Some(144));
        assert_eq!(parse_dword("1"), None);
        assert_eq!(parse_dword("0xZZ"), None);
    }

    #[test]
    fn the_theme_flag_is_read_from_the_personalisation_key() {
        let runner = FakeCommandRunner::default();
        runner.set_result(CommandOutput::exited(
            0,
            "\r\nHKEY_CURRENT_USER\\Software\\...\\Personalize\r\n    AppsUseLightTheme    REG_DWORD    0x0\r\n",
        ));
        assert_eq!(light_theme(&runner, "AppsUseLightTheme"), Some(false));
        let calls = runner.calls();
        let line = calls
            .first()
            .map(|(argv, _)| argv.join(" "))
            .unwrap_or_default();
        assert!(line.contains("Personalize"), "{line}");
        assert!(line.ends_with("AppsUseLightTheme"), "{line}");
    }

    #[test]
    fn a_missing_value_is_unknown() {
        let runner = FakeCommandRunner::default();
        runner.set_result(CommandOutput::exited(1, "ERROR: not found"));
        assert_eq!(light_theme(&runner, "SystemUsesLightTheme"), None);
        assert_eq!(log_pixels(&runner), None);
    }
}

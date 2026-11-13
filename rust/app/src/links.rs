//! Opens a web link in the default browser: `cmd /c start` on Windows, `xdg-open` elsewhere,
//! through the core's command runner (no shell). The command line is a pure function, so both
//! forms are tested on every platform; the program runs on a thread so the UI does not wait.

use std::sync::Arc;
use std::time::Duration;

use homelink_core::ports::CommandRunner;

/// How long the opener may run; it returns at once once the browser has the link.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// The command line that opens `url`. `windows` picks the Windows form.
pub(crate) fn open_argv(url: &str, windows: bool) -> Vec<String> {
    if windows {
        // `start` takes the first quoted argument as the window title, so the empty title comes
        // before the link; without it the link would be read as the title.
        vec![
            "cmd".to_owned(),
            "/c".to_owned(),
            "start".to_owned(),
            String::new(),
            url.to_owned(),
        ]
    } else {
        vec!["xdg-open".to_owned(), url.to_owned()]
    }
}

/// Opens `url` in the default browser, off the calling thread. A failure is logged.
pub(crate) fn open(runner: Arc<dyn CommandRunner>, url: String) {
    std::thread::spawn(move || {
        let output = runner.run(&open_argv(&url, cfg!(windows)), OPEN_TIMEOUT);
        if !output.success() {
            log::warn!("could not open {url}: {}", output.text);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_opens_the_link_through_start_with_an_empty_title() {
        assert_eq!(
            open_argv("https://slint.dev", true),
            ["cmd", "/c", "start", "", "https://slint.dev"]
        );
    }

    #[test]
    fn elsewhere_the_link_goes_to_xdg_open() {
        assert_eq!(
            open_argv("https://slint.dev", false),
            ["xdg-open", "https://slint.dev"]
        );
    }
}

//! HomeLink: a tray utility that wakes, sleeps and reaches remote PCs.
//!
//! This is the application shell: the Slint panel and tray icon, driven only by the engine
//! snapshot of `homelink-core`. Only `--demo` runs for now. It uses the core test fakes with
//! delays, because the Windows adapters (WireGuard, ICMP, SSH, magic packets) are not written
//! yet. See `docs/rust-architecture.md`, sections 2 and 7.
#![windows_subsystem = "windows"]

mod demo;
mod platform;
mod ui;
mod view;

use std::error::Error;
use std::process::ExitCode;
use std::sync::Arc;

use homelink_core::engine::Engine;
use homelink_core::i18n::{LanguageSetting, Translator};
use homelink_core::testing;

/// What the real (non-demo) mode says until its adapters exist.
const REAL_MODE_MISSING: &str = "The Windows adapters are not written yet, so HomeLink cannot \
    manage real PCs yet. Start it with --demo to see the panel with simulated PCs and tunnels.";

/// Command-line options.
struct Options {
    demo: bool,
    language: LanguageSetting,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        demo: false,
        language: LanguageSetting::Auto,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => options.demo = true,
            "--lang" => {
                let value = args.next().ok_or("--lang needs a value: auto, en or uk")?;
                options.language = LanguageSetting::parse(&value)
                    .ok_or_else(|| format!("unknown language '{value}': use auto, en or uk"))?;
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            platform::message(&message);
            return ExitCode::from(2);
        }
    };
    if !options.demo {
        platform::message(REAL_MODE_MISSING);
        return ExitCode::FAILURE;
    }
    match run_demo(options.language) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            platform::message(&err.to_string());
            ExitCode::FAILURE
        }
    }
}

/// Starts the panel and the tray icon on the engine built from the demo configuration, and
/// runs the Slint event loop until the user quits.
fn run_demo(language: LanguageSetting) -> Result<(), Box<dyn Error>> {
    let config = testing::multi_config();
    platform::select_backend()?;
    let translator = Translator::new(language.resolve(None));
    let panel = view::Panel::new()?;
    let tray = view::Tray::new()?;
    let deps = demo::deps(&config, Arc::new(ui::UiSink));
    let (engine, handle) = Engine::new(config, deps);
    ui::install(&panel, &tray, translator, handle.clone());
    tray.show()?;
    // The engine thread is left to end with the process: `Handle::stop` was sent on quit.
    let _engine_thread = engine.spawn()?;
    slint::run_event_loop_until_quit()?;
    handle.stop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn options() {
        let o = parse_args(args(&["--demo", "--lang", "uk"])).unwrap();
        assert!(o.demo);
        assert_eq!(
            o.language,
            LanguageSetting::Fixed(homelink_core::i18n::Lang::Uk)
        );
        assert!(!parse_args(args(&[])).unwrap().demo);
        assert!(parse_args(args(&["--lang"])).is_err());
        assert!(parse_args(args(&["--lang", "de"])).is_err());
        assert!(parse_args(args(&["--nope"])).is_err());
    }
}

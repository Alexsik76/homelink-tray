//! HomeLink: a tray utility that wakes, sleeps and reaches remote PCs.
//!
//! The application shell: the Slint panel and tray icon, driven only by the engine snapshot
//! of `homelink-core`. Two ways to start it:
//!
//! - **Real mode** (no flag): administrator check, `config.toml`, the real adapters
//!   ([`real`], [`windows`]) and the portable ones of the core.
//! - **`--demo`** (only in a build with the `demo` feature): the engine on the core test
//!   fakes with simulated delays ([`demo`]).
//!
//! See `docs/rust-architecture.md`, sections 2, 7 and 9.
#![windows_subsystem = "windows"]

#[cfg(feature = "demo")]
mod demo;
mod instance;
mod logging;
mod platform;
mod real;
mod ui;
mod view;
mod windows;

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use homelink_core::config::Config;
use homelink_core::domain::PcId;
use homelink_core::engine::{Deps, Engine};
use homelink_core::i18n::{LanguageSetting, Translator};
use homelink_core::ports::Sink;

/// Command-line options.
struct Options {
    /// `--demo`: run on simulated PCs.
    demo: bool,
    /// `--lang`: overrides the `language` of the settings.
    language: Option<LanguageSetting>,
    /// `--config`: the settings file, instead of `config.toml` next to the program.
    config: Option<PathBuf>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        demo: false,
        language: None,
        config: None,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => options.demo = true,
            "--lang" => {
                let value = args.next().ok_or("--lang needs a value: auto, en or uk")?;
                options.language =
                    Some(LanguageSetting::parse(&value).ok_or_else(|| {
                        format!("unknown language '{value}': use auto, en or uk")
                    })?);
            }
            "--config" => {
                let value = args.next().ok_or("--config needs the path of the file")?;
                options.config = Some(PathBuf::from(value));
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_args(args.clone()) {
        Ok(options) => options,
        Err(message) => {
            platform::message(&message);
            return ExitCode::from(2);
        }
    };
    if options.demo {
        return run_demo(options.language);
    }
    match real::prepare(options.config.as_deref(), options.language, &args) {
        Ok(real) => {
            let translator = real.translator.clone();
            let rdp = Arc::clone(&real.rdp);
            let result = run_ui(
                real.config.clone(),
                translator.clone(),
                |sink| real.deps(sink),
                Box::new(move |pc: &PcId| rdp.open(pc)),
            );
            finish(result, &translator)
        }
        Err(real::Stop::Error(text)) => {
            platform::message(&text);
            ExitCode::FAILURE
        }
        Err(real::Stop::Info(text)) => {
            platform::info(&text);
            ExitCode::FAILURE
        }
        Err(real::Stop::Restarted) => ExitCode::SUCCESS,
    }
}

/// The exit code of a finished run; a failure is shown first.
fn finish(result: Result<(), Box<dyn Error>>, translator: &Translator) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            log::error!("the panel stopped: {err}");
            platform::message(&translator.t("error.startup", &[("detail", &err.to_string())]));
            ExitCode::FAILURE
        }
    }
}

/// `--demo`: the panel on simulated PCs and tunnels.
#[cfg(feature = "demo")]
fn run_demo(language: Option<LanguageSetting>) -> ExitCode {
    let config = homelink_core::testing::multi_config();
    let translator = Translator::new(language.unwrap_or(config.language).resolve(None));
    let for_deps = config.clone();
    let result = run_ui(
        config,
        translator.clone(),
        move |sink| demo::deps(&for_deps, sink),
        Box::new(|pc: &PcId| {
            log::info!("demo: Remote Desktop for {pc} would open here");
            Ok(())
        }),
    );
    finish(result, &translator)
}

/// `--demo` in a build without the `demo` feature.
#[cfg(not(feature = "demo"))]
fn run_demo(language: Option<LanguageSetting>) -> ExitCode {
    let translator = Translator::new(language.unwrap_or(LanguageSetting::Auto).resolve(None));
    platform::message(&translator.t("error.demo_unavailable", &[]));
    ExitCode::FAILURE
}

/// Starts the panel and the tray icon on an engine built from `config` and the dependencies
/// that `deps` makes, and runs the Slint event loop until the user quits.
fn run_ui(
    config: Config,
    translator: Translator,
    deps: impl FnOnce(Arc<dyn Sink>) -> Deps,
    rdp: ui::RdpAction,
) -> Result<(), Box<dyn Error>> {
    platform::select_backend()?;
    let panel = view::Panel::new()?;
    let tray = view::Tray::new()?;
    let sink = Arc::new(ui::UiSink::new(translator.clone()));
    let (engine, handle) = Engine::new(config, deps(sink));
    ui::install(&panel, &tray, translator, handle.clone(), rdp);
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
            Some(LanguageSetting::Fixed(homelink_core::i18n::Lang::Uk))
        );
        let o = parse_args(args(&[])).unwrap();
        assert!(!o.demo);
        assert_eq!(o.language, None);
        assert_eq!(o.config, None);
        assert!(parse_args(args(&["--lang"])).is_err());
        assert!(parse_args(args(&["--lang", "de"])).is_err());
        assert!(parse_args(args(&["--nope"])).is_err());
    }

    #[test]
    fn the_settings_file_can_be_named() {
        let o = parse_args(args(&["--config", r"C:\My Files\config.toml"])).unwrap();
        assert_eq!(o.config, Some(PathBuf::from(r"C:\My Files\config.toml")));
        assert!(parse_args(args(&["--config"])).is_err());
    }
}

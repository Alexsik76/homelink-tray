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

mod appearance;
#[cfg(feature = "demo")]
mod demo;
mod icon;
mod instance;
mod links;
mod logging;
mod manage;
mod platform;
mod real;
mod ui;
mod view;
mod windows;
mod wizard_ui;

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use homelink_core::adapters::command::StdCommandRunner;
use homelink_core::adapters::tcp::TcpPortProbe;
use homelink_core::config::Config;
use homelink_core::domain::PcId;
use homelink_core::engine::{Deps, Engine, Handle};
use homelink_core::i18n::{LanguageSetting, Translator};
use homelink_core::ports::Sink;

use crate::appearance::ThemeSetting;

/// Command-line options.
struct Options {
    /// `--demo`: run on simulated PCs.
    demo: bool,
    /// `--lang`: overrides the `language` of the settings.
    language: Option<LanguageSetting>,
    /// `--config`: the settings file, instead of `config.toml` next to the program.
    config: Option<PathBuf>,
    /// `--theme`: the colours of the panel (provisional, until the settings window has it).
    theme: ThemeSetting,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        demo: false,
        language: None,
        config: None,
        theme: ThemeSetting::Auto,
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
            "--theme" => {
                let value = args
                    .next()
                    .ok_or("--theme needs a value: auto, light or dark")?;
                options.theme = ThemeSetting::parse(&value)
                    .ok_or_else(|| format!("unknown theme '{value}': use auto, light or dark"))?;
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
        return run_demo(options.language, options.theme);
    }
    match real::prepare(options.config.as_deref(), options.language, &args) {
        Ok(real) => {
            let translator = real.translator.clone();
            let rdp = Arc::clone(&real.rdp);
            let persist = manage::Persist {
                path: Some(real.config_path.clone()),
                rdp: Some(Arc::clone(&real.rdp)),
                ports: Arc::new(TcpPortProbe),
                runner: Arc::clone(&real.runner),
                opener: Arc::clone(&real.runner),
            };
            let result = run_ui(
                real.config.clone(),
                translator.clone(),
                options.theme,
                |sink| real.deps(sink),
                Box::new(move |pc: &PcId| rdp.open(pc)),
                persist,
                |_| {},
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

/// `--demo`: the panel on simulated PCs and tunnels, cycling through every display state.
#[cfg(feature = "demo")]
fn run_demo(language: Option<LanguageSetting>, theme: ThemeSetting) -> ExitCode {
    let config = homelink_core::testing::demo_config();
    let translator = Translator::new(language.unwrap_or(config.language).resolve(None));
    let for_deps = config.clone();
    let fakes = demo::Fakes::default();
    let for_cycle = fakes.clone();
    // The settings work on an in-memory copy: no file is written (`path` is None).
    let persist = manage::Persist {
        path: None,
        rdp: None,
        ports: fakes.ports(),
        runner: demo::runner(),
        // The link opens the real browser even in demo mode.
        opener: Arc::new(StdCommandRunner::new()),
    };
    let result = run_ui(
        config,
        translator.clone(),
        theme,
        move |sink| demo::deps(&for_deps, sink, &fakes),
        Box::new(|pc: &PcId| {
            log::info!("demo: Remote Desktop for {pc} would open here");
            Ok(())
        }),
        persist,
        move |handle| demo::cycle(handle, for_cycle),
    );
    finish(result, &translator)
}

/// `--demo` in a build without the `demo` feature.
#[cfg(not(feature = "demo"))]
fn run_demo(language: Option<LanguageSetting>, _theme: ThemeSetting) -> ExitCode {
    let translator = Translator::new(language.unwrap_or(LanguageSetting::Auto).resolve(None));
    platform::message(&translator.t("error.demo_unavailable", &[]));
    ExitCode::FAILURE
}

/// Starts the panel and the tray icon on an engine built from `config` and the dependencies
/// that `deps` makes, and runs the Slint event loop until the user quits. `start` runs once the
/// engine has started, with a handle to it (the demo drives the engine through it).
fn run_ui(
    config: Config,
    translator: Translator,
    theme: ThemeSetting,
    deps: impl FnOnce(Arc<dyn Sink>) -> Deps,
    rdp: ui::RdpAction,
    persist: manage::Persist,
    start: impl FnOnce(Handle),
) -> Result<(), Box<dyn Error>> {
    platform::select_backend()?;
    let panel = view::Panel::new()?;
    let details = view::DetailsWindow::new()?;
    let tray = view::Tray::new()?;
    let sink = Arc::new(ui::UiSink::new(translator.clone()));
    let deps = deps(sink);
    let clock = Arc::clone(&deps.clock);
    let settings_config = config.clone();
    let (engine, handle) = Engine::new(config, deps);
    let appearance = appearance::detect(theme, &StdCommandRunner::new());
    ui::install(
        &ui::Windows {
            panel: &panel,
            details: &details,
            tray: &tray,
        },
        ui::Setup {
            translator: translator.clone(),
            handle: handle.clone(),
            rdp,
            clock,
            appearance,
        },
    );
    manage::install(
        &panel,
        manage::Setup {
            config: settings_config,
            translator,
            handle: handle.clone(),
            dark: appearance.dark,
            persist,
        },
    )?;
    tray.show()?;
    // The engine thread is left to end with the process: `Handle::stop` was sent on quit.
    let _engine_thread = engine.spawn()?;
    start(handle.clone());
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
        assert_eq!(o.theme, ThemeSetting::Auto);
        let o = parse_args(args(&["--theme", "dark"])).unwrap();
        assert_eq!(o.theme, ThemeSetting::Dark);
        assert!(parse_args(args(&["--theme"])).is_err());
        assert!(parse_args(args(&["--theme", "blue"])).is_err());
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

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
//! Two console options do their work and exit without a window and without administrator
//! rights: `--version` prints the version, `--check-config` validates the settings file (the
//! one of `--config`, or `config.toml` next to the program) and prints every problem.
//!
//! See `docs/rust-architecture.md`, sections 2, 7 and 9.
#![windows_subsystem = "windows"]

mod appearance;
mod balloon;
#[cfg(feature = "demo")]
mod demo;
mod icon;
mod instance;
mod links;
mod logging;
mod manage;
mod platform;
mod real;
mod ssh_ops;
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
    /// `--version`: print the version and exit.
    version: bool,
    /// `--check-config`: validate the settings file and exit.
    check_config: bool,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        demo: false,
        language: None,
        config: None,
        theme: ThemeSetting::Auto,
        version: false,
        check_config: false,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => options.demo = true,
            "--version" => options.version = true,
            "--check-config" => options.check_config = true,
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
    if options.version {
        platform::console(&version_line());
        return ExitCode::SUCCESS;
    }
    if options.check_config {
        let (ok, text) = check_config(options.config.as_deref());
        platform::console(&text);
        return if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
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

/// What `--version` prints.
fn version_line() -> String {
    format!("HomeLink {}", env!("CARGO_PKG_VERSION"))
}

/// `--check-config`: loads the settings file without creating it and says whether it is valid.
/// The text is English: it is meant for a console and for CI, not for the panel's user.
fn check_config(explicit: Option<&std::path::Path>) -> (bool, String) {
    let path = match std::env::current_exe() {
        Ok(exe) => real::config_path(explicit, &exe),
        Err(err) => return (false, format!("cannot find the program's folder: {err}")),
    };
    match homelink_core::config::load(&path) {
        Ok(_) => (true, format!("{}: valid", path.display())),
        Err(err) => {
            // A file error already names the file; a list of problems does not.
            let text = err.to_string();
            let shown = path.display().to_string();
            if text.starts_with(&shown) {
                (false, text)
            } else {
                (false, format!("{shown}: {text}"))
            }
        }
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
    let engine_thread = engine.spawn()?;
    start(handle.clone());
    slint::run_event_loop_until_quit()?;
    platform::remove_balloons();
    handle.stop();
    join_engine(&engine_thread);
    Ok(())
}

/// How long the quit waits for the engine thread to end after `Handle::stop`.
const ENGINE_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Waits until the engine thread has ended, at most [`ENGINE_STOP_WAIT`], so that its last log
/// lines are written. Worker threads are not waited for: a job blocked in a system call (a
/// `ssh` that hangs) must not keep the program alive, and the process ends them.
fn join_engine(thread: &std::thread::JoinHandle<()>) {
    let deadline = std::time::Instant::now() + ENGINE_STOP_WAIT;
    while !thread.is_finished() {
        if std::time::Instant::now() >= deadline {
            log::warn!("the engine did not stop within {ENGINE_STOP_WAIT:?}; quitting anyway");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    log::info!("the engine stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_console_options_are_parsed() {
        let o = parse_args(args(&["--version"])).unwrap();
        assert!(o.version && !o.check_config && !o.demo);
        let o = parse_args(args(&["--check-config", "--config", "x.toml"])).unwrap();
        assert!(o.check_config && !o.version);
        assert_eq!(o.config, Some(PathBuf::from("x.toml")));
    }

    #[test]
    fn the_version_line_names_the_package_version() {
        assert_eq!(
            version_line(),
            format!("HomeLink {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn the_example_settings_pass_the_check_and_a_missing_file_fails_it() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let example = dir.join("config.example.toml");
        let (ok, text) = check_config(Some(&example));
        assert!(ok, "{text}");
        assert!(text.ends_with(": valid"), "{text}");
        let missing = dir.join("no-such-config.toml");
        let (ok, text) = check_config(Some(&missing));
        assert!(!ok, "{text}");
        let shown = missing.display().to_string();
        assert_eq!(text.matches(shown.as_str()).count(), 1, "{text}");
        assert!(!missing.exists());
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

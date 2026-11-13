//! The real mode: everything that happens between the start of the program and the moment
//! the panel opens, and the engine's dependencies built from the real adapters.
//!
//! The order is the one of `__main__.py`: administrator rights (restart elevated if needed),
//! then the configuration file (created from a template on the first start), the check that
//! no other copy runs, the log file, and the engine. Every failure before the panel is a
//! message in the language of the catalog, shown in a message box: Slint is not up yet.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use homelink_core::adapters::command::StdCommandRunner;
use homelink_core::adapters::default_wake_registry;
use homelink_core::adapters::power::CommandPowerRunner;
use homelink_core::adapters::tcp::TcpPortProbe;
use homelink_core::config::{self, Config, FILE_NAME, Loaded};
use homelink_core::engine::{Deps, ThreadPool};
use homelink_core::i18n::{LanguageSetting, Translator};
use homelink_core::ports::{CommandRunner, Sink, SystemClock};

use crate::instance::{self, Acquired, InstanceGuard};
use crate::logging;
use crate::windows::elevation::{self, Elevation};
use crate::windows::language;
use crate::windows::oem;
use crate::windows::ping::CommandPinger;
use crate::windows::rdp::RdpLauncher;
use crate::windows::wireguard::WireGuardDriver;

/// The template written on the first start. It is a valid configuration, so a test keeps it so.
const EXAMPLE_CONFIG: &str = include_str!("../config.example.toml");

/// Why the program stops before the panel opens.
#[derive(Debug)]
pub(crate) enum Stop {
    /// An error to show; the program exits with a failure.
    Error(String),
    /// Information to show (the settings file was just created); the program exits.
    Info(String),
    /// An elevated copy was started; this one exits without a word.
    Restarted,
}

/// What the real mode needs to run the panel.
pub(crate) struct Real {
    /// The validated settings.
    pub(crate) config: Config,
    /// The texts, in the language of the settings.
    pub(crate) translator: Translator,
    /// Opens Remote Desktop.
    pub(crate) rdp: Arc<RdpLauncher>,
    /// The settings file, where the settings window saves changes.
    #[allow(dead_code)] // used by the settings window, which is not built yet
    pub(crate) config_path: PathBuf,
    runner: Arc<dyn CommandRunner>,
    _instance: InstanceGuard,
}

impl Real {
    /// The engine's dependencies: the real adapters, with `sink` for what the engine publishes.
    pub(crate) fn deps(&self, sink: Arc<dyn Sink>) -> Deps {
        deps(&self.runner, sink)
    }
}

/// Where the settings file is: `explicit` (from `--config`) or `config.toml` next to `exe`.
pub(crate) fn config_path(explicit: Option<&Path>, exe: &Path) -> PathBuf {
    match explicit {
        Some(path) => std::path::absolute(path).unwrap_or_else(|_| path.to_owned()),
        None => exe.parent().unwrap_or(Path::new(".")).join(FILE_NAME),
    }
}

/// `args` with the value of `--config` made absolute. The copy started by a restart with
/// administrator rights may run in another working directory (often `System32`), where a
/// relative path would point somewhere else.
pub(crate) fn absolute_args(args: &[String]) -> Vec<String> {
    let mut after_config = false;
    args.iter()
        .map(|arg| {
            let value = if after_config {
                std::path::absolute(arg)
                    .map_or_else(|_| arg.clone(), |p| p.to_string_lossy().into_owned())
            } else {
                arg.clone()
            };
            after_config = arg == "--config";
            value
        })
        .collect()
}

/// The folder of the settings file: the log, the lock file and the `.rdp` files go there.
pub(crate) fn config_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_owned(),
        _ => PathBuf::from("."),
    }
}

/// Writes the template to `path`, never over an existing file.
pub(crate) fn create_example(path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(EXAMPLE_CONFIG.as_bytes())
}

/// Loads the settings. A missing file is created from the template and reported as
/// [`Stop::Info`]; an invalid one is a [`Stop::Error`] that lists every problem.
pub(crate) fn load_settings(path: &Path, tr: &Translator) -> Result<Loaded, Stop> {
    let shown = path.display().to_string();
    match path.try_exists() {
        Ok(true) => {}
        Ok(false) => {
            return Err(match create_example(path) {
                Ok(()) => Stop::Info(tr.t("error.config_created", &[("path", shown.as_str())])),
                Err(err) => Stop::Error(tr.t(
                    "error.config_create",
                    &[("path", shown.as_str()), ("detail", &err.to_string())],
                )),
            });
        }
        Err(err) => {
            return Err(Stop::Error(
                tr.t("error.config", &[("detail", &format!("{shown}: {err}"))]),
            ));
        }
    }
    config::load(path)
        .map_err(|err| Stop::Error(tr.t("error.config", &[("detail", &err.to_string())])))
}

/// The language tag of the system: the Windows display language, or `LANG` elsewhere.
fn system_language_tag(runner: &dyn CommandRunner) -> Option<String> {
    if cfg!(windows) {
        language::system_language_tag(runner)
    } else {
        std::env::var("LANG").ok()
    }
}

/// Starts the real mode: elevation, settings, the single-instance lock, logging, adapters.
///
/// `args` are the command-line arguments that a restart with administrator rights repeats.
pub(crate) fn prepare(
    config_arg: Option<&Path>,
    language_arg: Option<LanguageSetting>,
    args: &[String],
) -> Result<Real, Stop> {
    // `reg`, `whoami` and `powershell` write ASCII that needs no code page.
    let probe = StdCommandRunner::new();
    let tag = match language_arg {
        Some(LanguageSetting::Fixed(_)) => None,
        _ => system_language_tag(&probe),
    };
    let early = Translator::new(
        language_arg
            .unwrap_or(LanguageSetting::Auto)
            .resolve(tag.as_deref()),
    );

    if cfg!(windows) && elevation::check(&probe) == Elevation::NotElevated {
        let restarted = std::env::current_exe()
            .map_err(|err| err.to_string())
            .and_then(|exe| elevation::relaunch(&probe, &exe, &absolute_args(args)));
        return Err(match restarted {
            Ok(()) => Stop::Restarted,
            Err(detail) => Stop::Error(early.t("error.elevation", &[("detail", &detail)])),
        });
    }

    let exe = std::env::current_exe()
        .map_err(|err| Stop::Error(early.t("error.startup", &[("detail", &err.to_string())])))?;
    let path = config_path(config_arg, &exe);
    let loaded = load_settings(&path, &early)?;
    let config = loaded.config;
    let setting = language_arg.unwrap_or(config.language);
    let translator = Translator::new(setting.resolve(tag.as_deref()));
    let dir = config_dir(&path);

    let instance = match instance::acquire(&dir.join(instance::LOCK_FILE)) {
        Ok(Acquired::Yes(guard)) => guard,
        Ok(Acquired::AlreadyRunning) => {
            return Err(Stop::Error(translator.t("error.already_running", &[])));
        }
        Err(err) => {
            return Err(Stop::Error(
                translator.t("error.startup", &[("detail", &err.to_string())]),
            ));
        }
    };

    // Without a log file the program still works; there is nowhere to report that.
    if logging::init(&dir.join(&config.log.file), config.log.level).is_ok() {
        log::info!(
            "HomeLink {} started, settings: {}",
            env!("CARGO_PKG_VERSION"),
            path.display()
        );
        for warning in &loaded.warnings {
            log::warn!("settings: {warning}");
        }
    }

    let runner: Arc<dyn CommandRunner> = if cfg!(windows) {
        let code_page = oem::detect(&probe);
        log::info!("OEM code page: {code_page:?}");
        Arc::new(StdCommandRunner::new().with_decoder(oem::decoder(code_page)))
    } else {
        Arc::new(StdCommandRunner::new())
    };
    let rdp = Arc::new(RdpLauncher::new(&config, &dir));
    Ok(Real {
        config,
        translator,
        rdp,
        config_path: path,
        runner,
        _instance: instance,
    })
}

/// The dependencies of the engine: the real adapters of the app and of the core.
pub(crate) fn deps(runner: &Arc<dyn CommandRunner>, sink: Arc<dyn Sink>) -> Deps {
    Deps {
        tunnel: Arc::new(WireGuardDriver::new(Arc::clone(runner))),
        pinger: Arc::new(CommandPinger::new(Arc::clone(runner))),
        ports: Arc::new(TcpPortProbe),
        wake: Arc::new(default_wake_registry(Arc::clone(runner))),
        power: Arc::new(CommandPowerRunner::new(Arc::clone(runner))),
        clock: Arc::new(SystemClock::new()),
        executor: Arc::new(ThreadPool::new(ThreadPool::DEFAULT_WORKERS)),
        sink,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use homelink_core::i18n::Lang;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("homelink-real-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn english() -> Translator {
        Translator::new(Lang::En)
    }

    #[test]
    fn the_template_is_a_valid_configuration_without_warnings() {
        let loaded = config::parse(EXAMPLE_CONFIG).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.config.tunnels.len(), 1);
        assert_eq!(loaded.config.pcs.len(), 1);
        assert_eq!(loaded.config.language, LanguageSetting::Auto);
    }

    #[test]
    fn the_settings_file_is_next_to_the_exe_unless_given() {
        let exe = Path::new("/opt/homelink/HomeLink.exe");
        assert_eq!(
            config_path(None, exe),
            Path::new("/opt/homelink").join("config.toml")
        );
        // A relative path is made absolute against the working directory.
        assert!(config_path(Some(Path::new("h.toml")), exe).is_absolute());
        assert_eq!(config_dir(Path::new("/etc/h.toml")), Path::new("/etc"));
        assert_eq!(config_dir(Path::new("h.toml")), Path::new("."));
    }

    #[test]
    fn an_absolute_settings_path_is_used_as_it_is() {
        // Absolute on every platform (a drive letter on Windows).
        let given = std::env::temp_dir().join("h.toml");
        assert_eq!(
            config_path(Some(&given), Path::new("/opt/HomeLink.exe")),
            given
        );
    }

    #[test]
    fn a_restart_gets_an_absolute_settings_path_and_keeps_the_other_arguments() {
        let args: Vec<String> = ["--lang", "uk", "--config", "rel/config.toml", "--demo"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let out = absolute_args(&args);
        assert_eq!(out.len(), 5);
        assert_eq!(out.first().map(String::as_str), Some("--lang"));
        assert_eq!(out.get(1).map(String::as_str), Some("uk"));
        assert_eq!(out.get(2).map(String::as_str), Some("--config"));
        assert!(Path::new(out.get(3).unwrap()).is_absolute(), "{out:?}");
        assert!(out.get(3).unwrap().ends_with("config.toml"));
        assert_eq!(out.get(4).map(String::as_str), Some("--demo"));
        // Only the value after --config is touched.
        assert_eq!(absolute_args(&["rel".to_owned()]), ["rel"]);
        assert!(absolute_args(&[]).is_empty());
    }

    #[test]
    fn the_first_start_creates_the_template_and_says_so() {
        let dir = temp_dir("first");
        let path = dir.join("sub").join("config.toml");
        let stop = load_settings(&path, &english()).unwrap_err();
        let Stop::Info(text) = stop else {
            panic!("expected information, got {stop:?}");
        };
        assert!(text.contains("A settings file has been created"), "{text}");
        assert!(text.contains(&path.display().to_string()), "{text}");
        assert_eq!(fs::read_to_string(&path).unwrap(), EXAMPLE_CONFIG);
        // The next start loads it.
        assert!(load_settings(&path, &english()).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_message_is_in_the_language_of_the_catalog() {
        let dir = temp_dir("uk");
        let path = dir.join("config.toml");
        let stop = load_settings(&path, &Translator::new(Lang::Uk)).unwrap_err();
        let Stop::Info(text) = stop else {
            panic!("expected information, got {stop:?}");
        };
        assert!(text.contains("Створено файл налаштувань"), "{text}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_template_is_never_written_over_a_file() {
        let dir = temp_dir("never");
        let path = dir.join("config.toml");
        fs::write(&path, "mine").unwrap();
        assert!(create_example(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_problem_of_an_invalid_file_is_shown_with_its_place() {
        let dir = temp_dir("invalid");
        let path = dir.join("config.toml");
        let broken = EXAMPLE_CONFIG
            .replace("ports = [3389]", "ports = []")
            .replace("poll_seconds = 5 ", "poll_seconds = 0 ");
        fs::write(&path, broken).unwrap();
        let Stop::Error(text) = load_settings(&path, &english()).unwrap_err() else {
            panic!("expected an error");
        };
        assert!(text.starts_with("Error in the settings file:"), "{text}");
        assert!(text.contains("pcs[0].ports"), "{text}");
        assert!(text.contains("defaults.poll_seconds"), "{text}");
        let uk = load_settings(&path, &Translator::new(Lang::Uk)).unwrap_err();
        let Stop::Error(text) = uk else {
            panic!("expected an error");
        };
        assert!(text.starts_with("Помилка у файлі налаштувань:"), "{text}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_toml_is_an_error_not_a_template() {
        let dir = temp_dir("syntax");
        let path = dir.join("config.toml");
        fs::write(&path, "this is = = not toml").unwrap();
        assert!(matches!(
            load_settings(&path, &english()),
            Err(Stop::Error(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "this is = = not toml");
        let _ = fs::remove_dir_all(&dir);
    }
}

//! Administrator rights. Installing the WireGuard tunnel service needs them, and the keys
//! under `C:\ProgramData` are meant to be readable only by administrators, so the real mode
//! runs elevated. Like `elevation.py`, the program checks at start and, if it is not elevated,
//! starts an elevated copy of itself and exits.
//!
//! No Win32 binding is needed:
//!
//! - **Check:** `whoami /groups` lists the integrity level of the process as a SID. An
//!   elevated process has the "High" level `S-1-16-12288` (or "System" `S-1-16-16384`), a
//!   normal one "Medium" `S-1-16-8192`. The SID is the same in every display language.
//! - **Restart:** `powershell Start-Process -Verb RunAs`, which shows the UAC prompt.
//!
//! The architecture document also lists an application manifest with `requireAdministrator`
//! for the release build; that is a build-time setting and is not done here (see the roadmap).

use std::path::Path;
use std::time::Duration;

use homelink_core::ports::CommandRunner;

/// How long `whoami` may take.
const WHOAMI_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the restart may take: the user has to answer the UAC prompt.
const RELAUNCH_TIMEOUT: Duration = Duration::from_secs(120);

/// The mandatory integrity level of an elevated process ("High").
const HIGH_INTEGRITY: u32 = 12288;
/// The prefix of an integrity level SID.
const INTEGRITY_SID: &str = "S-1-16-";

/// Whether this process has administrator rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Elevation {
    /// It has.
    Elevated,
    /// It has not.
    NotElevated,
    /// The answer could not be read. The program goes on and does not restart, so that a
    /// system where the check does not work cannot make it restart forever.
    Unknown,
}

/// The command line that prints the groups and integrity level of this process.
pub(crate) fn whoami_argv() -> Vec<String> {
    vec!["whoami.exe".into(), "/groups".into()]
}

/// Reads the integrity level from the output of `whoami /groups`.
pub(crate) fn parse_groups(output: &str) -> Elevation {
    let level = output.split_whitespace().find_map(|word| {
        let digits = word.strip_prefix(INTEGRITY_SID)?;
        let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok()
    });
    match level {
        Some(level) if level >= HIGH_INTEGRITY => Elevation::Elevated,
        Some(_) => Elevation::NotElevated,
        None => Elevation::Unknown,
    }
}

/// Finds out whether this process is elevated.
pub(crate) fn check(runner: &dyn CommandRunner) -> Elevation {
    let output = runner.run(&whoami_argv(), WHOAMI_TIMEOUT);
    if output.success() {
        parse_groups(&output.text)
    } else {
        log::warn!("cannot check for administrator rights: {}", output.text);
        Elevation::Unknown
    }
}

/// `text` as a PowerShell single-quoted string. Inside it only the quote must be doubled; the
/// typographic single quotes count as quotes for PowerShell as well.
pub(crate) fn ps_quote(text: &str) -> String {
    let mut out = String::from("'");
    for c in text.chars() {
        out.push(c);
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// One argument as `Start-Process -ArgumentList` needs it: it joins the arguments with blanks
/// without quoting them, so an argument with a blank is wrapped in double quotes here.
fn process_argument(arg: &str) -> String {
    if arg.is_empty() || arg.contains(char::is_whitespace) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_owned()
    }
}

/// The PowerShell command that starts `exe` with `args` elevated.
pub(crate) fn relaunch_script(exe: &Path, args: &[String]) -> String {
    let mut script = format!(
        "Start-Process -FilePath {} -Verb RunAs",
        ps_quote(&exe.to_string_lossy())
    );
    if !args.is_empty() {
        let list: Vec<String> = args
            .iter()
            .map(|arg| ps_quote(&process_argument(arg)))
            .collect();
        script.push_str(&format!(" -ArgumentList @({})", list.join(",")));
    }
    script
}

/// The command line that restarts this program elevated.
pub(crate) fn relaunch_argv(exe: &Path, args: &[String]) -> Vec<String> {
    [
        "powershell.exe",
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Command",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain([relaunch_script(exe, args)])
    .collect()
}

/// Starts an elevated copy of the program (the caller exits afterwards). The error is the
/// reason, for the user, why that did not work (for example the UAC prompt was declined).
pub(crate) fn relaunch(
    runner: &dyn CommandRunner,
    exe: &Path,
    args: &[String],
) -> Result<(), String> {
    let output = runner.run(&relaunch_argv(exe, args), RELAUNCH_TIMEOUT);
    if output.success() {
        Ok(())
    } else {
        Err(output.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::scripted::Scripted;
    use homelink_core::ports::CommandOutput;

    /// `whoami /groups` of a normal (medium integrity) process.
    const MEDIUM: &str = "\r\nGROUP INFORMATION\r\n-----------------\r\n\r\nGroup Name                                                    Type             SID          Attributes\r\n============================================================= ================ ============ ==================================================\r\nEveryone                                                      Well-known group S-1-1-0      Mandatory group, Enabled by default, Enabled group\r\nBUILTIN\\Administrators                                        Alias            S-1-5-32-544 Group used for deny only\r\nMandatory Label\\Medium Mandatory Level                        Label            S-1-16-8192\r\n";

    /// The same for an elevated process.
    const HIGH: &str = "\r\nGROUP INFORMATION\r\n-----------------\r\n\r\nEveryone                                                      Well-known group S-1-1-0      Mandatory group, Enabled by default, Enabled group\r\nBUILTIN\\Administrators                                        Alias            S-1-5-32-544 Mandatory group, Enabled by default, Enabled group, Group owner\r\nMandatory Label\\High Mandatory Level                          Label            S-1-16-12288\r\n";

    #[test]
    fn a_medium_integrity_process_is_not_elevated() {
        assert_eq!(parse_groups(MEDIUM), Elevation::NotElevated);
    }

    #[test]
    fn a_high_integrity_process_is_elevated() {
        assert_eq!(parse_groups(HIGH), Elevation::Elevated);
        assert_eq!(parse_groups("x S-1-16-16384 y"), Elevation::Elevated);
    }

    #[test]
    fn the_translated_label_does_not_matter() {
        // Ukrainian Windows names the level in words; the SID is the same.
        let text = "Обов'язкова мітка\\Високий рівень обов'язковості  Мітка  S-1-16-12288\r\n";
        assert_eq!(parse_groups(text), Elevation::Elevated);
    }

    #[test]
    fn no_integrity_sid_means_unknown() {
        assert_eq!(parse_groups("ERROR: nothing"), Elevation::Unknown);
        assert_eq!(parse_groups(""), Elevation::Unknown);
        // Other SIDs are not integrity levels.
        assert_eq!(parse_groups("S-1-5-32-544"), Elevation::Unknown);
    }

    #[test]
    fn the_check_runs_whoami_and_never_guesses_after_a_failure() {
        let runner = Scripted::new(vec![("whoami", CommandOutput::exited(0, HIGH))]);
        assert_eq!(check(&runner), Elevation::Elevated);
        assert_eq!(
            runner.calls().first().map(|(argv, _)| argv.clone()),
            Some(vec!["whoami.exe".to_owned(), "/groups".to_owned()])
        );
        let broken = Scripted::new(vec![("whoami", CommandOutput::exited(1, "error"))]);
        assert_eq!(check(&broken), Elevation::Unknown);
        assert_eq!(check(&Scripted::new(vec![])), Elevation::Unknown);
    }

    #[test]
    fn powershell_quoting_doubles_every_kind_of_single_quote() {
        assert_eq!(ps_quote("plain"), "'plain'");
        assert_eq!(ps_quote("it's"), "'it''s'");
        assert_eq!(ps_quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
        assert_eq!(
            ps_quote(r"C:\Program Files\HomeLink"),
            r"'C:\Program Files\HomeLink'"
        );
        // Nothing else is special inside single quotes: no expansion of $ or backticks.
        assert_eq!(ps_quote("$x`n"), "'$x`n'");
    }

    #[test]
    fn the_restart_script_runs_the_exe_elevated() {
        let exe = Path::new(r"C:\Tools\Home Link\HomeLink.exe");
        assert_eq!(
            relaunch_script(exe, &[]),
            r"Start-Process -FilePath 'C:\Tools\Home Link\HomeLink.exe' -Verb RunAs"
        );
    }

    #[test]
    fn the_restart_script_passes_the_arguments_and_quotes_those_with_blanks() {
        let exe = Path::new(r"C:\HomeLink.exe");
        let args = vec![
            "--lang".to_owned(),
            "uk".to_owned(),
            "--config".to_owned(),
            r"C:\My Files\config.toml".to_owned(),
        ];
        assert_eq!(
            relaunch_script(exe, &args),
            r#"Start-Process -FilePath 'C:\HomeLink.exe' -Verb RunAs -ArgumentList @('--lang','uk','--config','"C:\My Files\config.toml"')"#
        );
    }

    #[test]
    fn the_restart_command_is_one_script_argument_and_hidden() {
        let argv = relaunch_argv(Path::new(r"C:\HomeLink.exe"), &[]);
        assert_eq!(argv.first().map(String::as_str), Some("powershell.exe"));
        assert!(argv.iter().any(|a| a == "-NoProfile"));
        assert_eq!(
            argv.get(argv.len() - 2).map(String::as_str),
            Some("-Command")
        );
        assert!(argv.last().unwrap().starts_with("Start-Process"));
    }

    #[test]
    fn a_declined_prompt_is_an_error_with_the_reason() {
        let ok = Scripted::new(vec![("powershell", CommandOutput::exited(0, ""))]);
        assert_eq!(relaunch(&ok, Path::new("x.exe"), &[]), Ok(()));
        let declined = Scripted::new(vec![(
            "powershell",
            CommandOutput::exited(1, "The operation was canceled by the user."),
        )]);
        assert_eq!(
            relaunch(&declined, Path::new("x.exe"), &[]),
            Err("The operation was canceled by the user.".into())
        );
    }
}

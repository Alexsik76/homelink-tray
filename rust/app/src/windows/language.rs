//! The display language of Windows, for `language = "auto"` and for the messages shown before
//! the configuration is read. It is read from the registry with `reg query`: the preferred
//! display languages of the user, else the language Windows was installed with.

use homelink_core::ports::CommandRunner;

use super::reg;

const DESKTOP_KEY: &str = r"HKCU\Control Panel\Desktop";
const MUI_CACHED_KEY: &str = r"HKCU\Control Panel\Desktop\MuiCached";
const NLS_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Nls\Language";

/// The first language tag of a registry value. `reg query` prints the separators of a
/// `REG_MULTI_SZ` as the two characters `\0`; a plain `REG_SZ` has none.
pub(crate) fn first_tag(data: &str) -> Option<String> {
    data.split("\\0")
        .map(str::trim)
        .find(|tag| !tag.is_empty())
        .map(str::to_owned)
}

/// The tag of the languages this program ships for a Windows language id given in
/// hexadecimal (`0422`, as `InstallLanguage` holds it): Ukrainian or English. Other languages
/// give `None` (the program then uses English).
pub(crate) fn tag_from_lcid(data: &str) -> Option<&'static str> {
    let id = u32::from_str_radix(data.trim().trim_start_matches("0x"), 16).ok()?;
    match id & 0x3FF {
        0x22 => Some("uk"),
        0x09 => Some("en"),
        _ => None,
    }
}

/// The display language tag of this system, if it can be found.
pub(crate) fn system_language_tag(runner: &dyn CommandRunner) -> Option<String> {
    let preferred = [
        (DESKTOP_KEY, "PreferredUILanguages"),
        (MUI_CACHED_KEY, "MachinePreferredUILanguages"),
    ]
    .into_iter()
    .find_map(|(key, name)| first_tag(&reg::query(runner, key, name)?.data));
    preferred.or_else(|| {
        let installed = reg::query(runner, NLS_KEY, "InstallLanguage")?;
        tag_from_lcid(&installed.data).map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::scripted::Scripted;
    use homelink_core::i18n::{Lang, LanguageSetting};
    use homelink_core::ports::CommandOutput;

    fn reg_output(name: &str, kind: &str, data: &str) -> CommandOutput {
        CommandOutput::exited(
            0,
            format!("\r\nHKEY_X\\Y\r\n    {name}    {kind}    {data}\r\n"),
        )
    }

    #[test]
    fn the_first_tag_of_a_multi_value_is_taken() {
        assert_eq!(first_tag("uk-UA\\0en-US").as_deref(), Some("uk-UA"));
        assert_eq!(first_tag("en-GB").as_deref(), Some("en-GB"));
        assert_eq!(first_tag("\\0de-DE").as_deref(), Some("de-DE"));
        assert_eq!(first_tag(""), None);
    }

    #[test]
    fn language_ids_map_to_the_shipped_languages() {
        assert_eq!(tag_from_lcid("0422"), Some("uk"));
        assert_eq!(tag_from_lcid("0409"), Some("en"));
        assert_eq!(tag_from_lcid("0809"), Some("en")); // English (United Kingdom)
        assert_eq!(tag_from_lcid("0419"), None); // Russian is not shipped
        assert_eq!(tag_from_lcid("zz"), None);
    }

    #[test]
    fn the_users_preferred_language_comes_first() {
        let runner = Scripted::new(vec![
            (
                "PreferredUILanguages",
                reg_output("PreferredUILanguages", "REG_MULTI_SZ", "uk-UA\\0en-US"),
            ),
            (
                "InstallLanguage",
                reg_output("InstallLanguage", "REG_SZ", "0409"),
            ),
        ]);
        let tag = system_language_tag(&runner);
        assert_eq!(tag.as_deref(), Some("uk-UA"));
        assert_eq!(LanguageSetting::Auto.resolve(tag.as_deref()), Lang::Uk);
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn the_installed_language_is_the_last_resort() {
        let runner = Scripted::new(vec![
            (
                "InstallLanguage",
                reg_output("InstallLanguage", "REG_SZ", "0422"),
            ),
            (
                "reg query",
                CommandOutput::exited(1, "ERROR: The system was unable to find the value."),
            ),
        ]);
        // The two preferred-language values are missing: the rule order puts the specific
        // rule first, so only InstallLanguage answers.
        assert_eq!(system_language_tag(&runner).as_deref(), Some("uk"));
        assert_eq!(runner.calls().len(), 3);
    }

    #[test]
    fn no_registry_means_no_tag() {
        let runner = Scripted::new(vec![]);
        assert_eq!(system_language_tag(&runner), None);
    }
}

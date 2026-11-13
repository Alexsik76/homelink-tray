//! Reading registry values by running `reg query`, which is installed with every Windows and
//! needs no API bindings. The output format is the same in every display language: the value
//! name, its type (`REG_SZ`, `REG_MULTI_SZ`, ...) and the data, separated by blanks.

use std::time::Duration;

use homelink_core::ports::CommandRunner;

/// How long `reg query` may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// One value printed by `reg query`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegValue {
    /// The data as printed. A `REG_MULTI_SZ` shows its separators as the two characters `\0`.
    pub(crate) data: String,
}

/// The command line that prints one value of a key.
pub(crate) fn query_argv(key: &str, value: &str) -> Vec<String> {
    ["reg", "query", key, "/v", value]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// Finds the value `name` in the output of `reg query` (the name is matched ignoring case).
pub(crate) fn parse_value(output: &str, name: &str) -> Option<RegValue> {
    output.lines().find_map(|line| {
        let line = line.trim_start();
        let head = line.get(..name.len())?;
        if !head.eq_ignore_ascii_case(name) {
            return None;
        }
        let rest = line.get(name.len()..)?;
        // The name must end here, not be the start of a longer one.
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let rest = rest.trim_start();
        let (kind, data) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        // A registry type (`REG_SZ`, `REG_MULTI_SZ`, ...) always follows the name.
        kind.starts_with("REG_").then(|| RegValue {
            data: data.trim().to_owned(),
        })
    })
}

/// Reads one registry value; `None` if the value or key does not exist or `reg` cannot run.
pub(crate) fn query(runner: &dyn CommandRunner, key: &str, value: &str) -> Option<RegValue> {
    let output = runner.run(&query_argv(key, value), TIMEOUT);
    if !output.success() {
        log::debug!("reg query {key} {value}: {}", output.text);
        return None;
    }
    parse_value(&output.text, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use homelink_core::ports::CommandOutput;
    use homelink_core::testing::FakeCommandRunner;

    const OEMCP: &str = "\r\nHKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Nls\\CodePage\r\n    OEMCP    REG_SZ    866\r\n\r\n";

    #[test]
    fn the_command_line_asks_for_one_value() {
        assert_eq!(
            query_argv(r"HKCU\Control Panel\Desktop", "PreferredUILanguages"),
            [
                "reg",
                "query",
                r"HKCU\Control Panel\Desktop",
                "/v",
                "PreferredUILanguages"
            ]
        );
    }

    #[test]
    fn a_value_is_found_by_name_and_type() {
        assert_eq!(
            parse_value(OEMCP, "OEMCP"),
            Some(RegValue { data: "866".into() })
        );
        // The name is matched without regard to case.
        assert!(parse_value(OEMCP, "oemcp").is_some());
    }

    #[test]
    fn data_with_blanks_is_kept_whole() {
        let text = "HKEY_CURRENT_USER\\Control Panel\\Desktop\n    PreferredUILanguages    REG_MULTI_SZ    uk-UA\\0en-US\n";
        let value = parse_value(text, "PreferredUILanguages").unwrap();
        assert_eq!(value.data, "uk-UA\\0en-US");
    }

    #[test]
    fn a_longer_name_is_not_the_name() {
        let text = "    OEMCPX    REG_SZ    1\n";
        assert_eq!(parse_value(text, "OEMCP"), None);
        assert_eq!(parse_value("nothing here", "OEMCP"), None);
        assert_eq!(parse_value("    OEMCP    notatype    1\n", "OEMCP"), None);
    }

    #[test]
    fn a_failed_query_is_none() {
        let runner = FakeCommandRunner::default();
        runner.set_result(CommandOutput::exited(
            1,
            "ERROR: The system was unable to find the specified registry key or value.",
        ));
        assert_eq!(query(&runner, "HKLM\\X", "OEMCP"), None);
        runner.set_result(CommandOutput::exited(0, OEMCP));
        assert_eq!(query(&runner, "HKLM\\X", "OEMCP").unwrap().data, "866");
    }
}

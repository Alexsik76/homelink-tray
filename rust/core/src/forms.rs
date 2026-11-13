//! Field parsers for the forms: text typed by the user in, a validated value or a
//! [`Problem`] out. They use the same domain parsers and [`crate::rules`] as the
//! configuration validator, so a form accepts a value if and only if the file would.
//!
//! Every function trims its input and pushes at most the problems of its own field into
//! `out`; it returns `None` exactly when it pushed one.

use std::net::Ipv4Addr;
use std::time::Duration;

use crate::config::MAX_SECONDS;
use crate::domain::{Argv, Host, InterfaceName, MacAddr, RelayKind, TunnelKind};
use crate::i18n::placeholders;
use crate::problem::{Code, Problem, from_fault};
use crate::rules::{self, Fault};

/// A value that is not empty, trimmed.
pub(crate) fn required<'a>(
    field: &'static str,
    value: &'a str,
    out: &mut Vec<Problem>,
) -> Option<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        out.push(Problem::new(field, Code::Required));
        None
    } else {
        Some(value)
    }
}

/// An IP address or a host name.
pub(crate) fn host(field: &'static str, value: &str, out: &mut Vec<Problem>) -> Option<Host> {
    let value = required(field, value, out)?;
    match Host::parse(value) {
        Ok(host) => Some(host),
        Err(_) => {
            out.push(Problem::new(field, Code::HostInvalid).with("value", value));
            None
        }
    }
}

/// A MAC address.
pub(crate) fn mac(field: &'static str, value: &str, out: &mut Vec<Problem>) -> Option<MacAddr> {
    let value = required(field, value, out)?;
    match MacAddr::parse(value) {
        Ok(mac) => Some(mac),
        Err(_) => {
            out.push(Problem::new(field, Code::MacInvalid).with("value", value));
            None
        }
    }
}

/// An IPv4 address.
pub(crate) fn ipv4(field: &'static str, value: &str, out: &mut Vec<Problem>) -> Option<Ipv4Addr> {
    let value = required(field, value, out)?;
    match value.parse::<Ipv4Addr>() {
        Ok(address) => Some(address),
        Err(_) => {
            out.push(Problem::new(field, Code::Ipv4Invalid).with("value", value));
            None
        }
    }
}

/// An interface name of a relay.
pub(crate) fn interface(
    field: &'static str,
    value: &str,
    out: &mut Vec<Problem>,
) -> Option<InterfaceName> {
    let value = required(field, value, out)?;
    match InterfaceName::parse(value) {
        Ok(name) => Some(name),
        Err(_) => {
            out.push(Problem::new(field, Code::InterfaceInvalid).with("value", value));
            None
        }
    }
}

/// A port typed as text.
pub(crate) fn port(field: &'static str, value: &str, out: &mut Vec<Problem>) -> Option<u16> {
    let value = required(field, value, out)?;
    match value
        .parse::<i64>()
        .map_err(|_| Fault::NotPort)
        .and_then(rules::port)
    {
        Ok(port) => Some(port),
        Err(_) => {
            out.push(Problem::new(field, Code::PortInvalid).with("value", value));
            None
        }
    }
}

/// A comma separated list of ports with at least one, no duplicates.
pub(crate) fn ports(field: &'static str, text: &str, out: &mut Vec<Problem>) -> Option<Vec<u16>> {
    let tokens: Vec<&str> = text
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        out.push(Problem::new(field, Code::PortsEmpty));
        return None;
    }
    let mut list: Vec<u16> = Vec::new();
    let mut ok = true;
    for token in tokens {
        match token
            .parse::<i64>()
            .map_err(|_| Fault::NotPort)
            .and_then(rules::port)
        {
            Ok(port) if list.contains(&port) => {
                out.push(Problem::new(field, Code::PortDuplicate).with("port", port.to_string()));
                ok = false;
            }
            Ok(port) => list.push(port),
            Err(_) => {
                out.push(Problem::new(field, Code::PortInvalid).with("value", token));
                ok = false;
            }
        }
    }
    ok.then_some(list)
}

/// An SSH user name.
pub(crate) fn user(field: &'static str, value: &str, out: &mut Vec<Problem>) -> Option<String> {
    let value = value.trim();
    match rules::ssh_user(value) {
        Ok(()) => Some(value.to_owned()),
        Err(fault) => {
            out.push(from_fault(field, fault, Code::UserInvalid, value));
            None
        }
    }
}

/// The absolute path of a private key file.
pub(crate) fn key_file(field: &'static str, value: &str, out: &mut Vec<Problem>) -> Option<String> {
    let value = value.trim();
    match rules::absolute_path(value) {
        Ok(()) => Some(value.to_owned()),
        Err(fault) => {
            out.push(from_fault(field, fault, Code::KeyNotAbsolute, value));
            None
        }
    }
}

/// A whole number of seconds from 1 to [`MAX_SECONDS`].
pub(crate) fn seconds(
    field: &'static str,
    value: &str,
    out: &mut Vec<Problem>,
) -> Option<Duration> {
    let value = required(field, value, out)?;
    let Ok(number) = value.parse::<u64>() else {
        out.push(Problem::new(field, Code::NumberInvalid).with("value", value));
        return None;
    };
    match rules::bounded(number, MAX_SECONDS) {
        Ok(n) => Some(Duration::from_secs(n)),
        Err(_) => {
            out.push(Problem::new(field, Code::SecondsRange).with("max", MAX_SECONDS.to_string()));
            None
        }
    }
}

/// A whole number of milliseconds from 1 to the millisecond equivalent of [`MAX_SECONDS`].
pub(crate) fn millis(field: &'static str, value: &str, out: &mut Vec<Problem>) -> Option<Duration> {
    let value = required(field, value, out)?;
    let max = MAX_SECONDS * 1000;
    let Ok(number) = value.parse::<u64>() else {
        out.push(Problem::new(field, Code::NumberInvalid).with("value", value));
        return None;
    };
    match rules::bounded(number, max) {
        Ok(n) => Some(Duration::from_millis(n)),
        Err(_) => {
            out.push(Problem::new(field, Code::MillisRange).with("max", max.to_string()));
            None
        }
    }
}

/// A WireGuard configuration file name (`.conf` or `.conf.dpapi`, a valid tunnel name).
pub(crate) fn wireguard_conf(
    field: &'static str,
    value: &str,
    out: &mut Vec<Problem>,
) -> Option<String> {
    let value = required(field, value, out)?;
    match TunnelKind::wireguard_name(value) {
        Ok(_) => Some(value.to_owned()),
        Err(_) => {
            let lower = value.to_ascii_lowercase();
            let problem = if lower.ends_with(".conf") || lower.ends_with(".conf.dpapi") {
                Problem::new(field, Code::ConfName).with("name", stem(value))
            } else {
                Problem::new(field, Code::ConfExtension).with("value", value)
            };
            out.push(problem);
            None
        }
    }
}

/// The file name without `.conf` / `.conf.dpapi`.
fn stem(conf: &str) -> String {
    let file = conf.rsplit(['/', '\\']).next().unwrap_or_default();
    let lower = file.to_ascii_lowercase();
    let cut = [".conf.dpapi", ".conf"]
        .iter()
        .find(|s| lower.ends_with(**s))
        .map_or(file.len(), |s| file.len() - s.len());
    file.get(..cut).unwrap_or_default().to_owned()
}

/// The checks that depend on the relay preset, as problems of the relay's fields.
pub(crate) fn preset_fields(
    kind: RelayKind,
    interface_given: bool,
    address_given: bool,
    interface_field: &'static str,
    address_field: &'static str,
    out: &mut Vec<Problem>,
) {
    let preset = kind.code();
    match rules::interface_for(kind, interface_given) {
        Ok(()) => {}
        Err(Fault::Missing) => {
            out.push(Problem::new(interface_field, Code::InterfaceRequired).with("preset", preset))
        }
        Err(_) => {
            out.push(Problem::new(interface_field, Code::InterfaceNotUsed).with("preset", preset))
        }
    }
    if rules::address_for(kind, address_given).is_err() {
        out.push(Problem::new(address_field, Code::AddressNotUsed).with("preset", preset));
    }
}

/// Splits a typed command line into elements, without a shell: blanks separate, double
/// quotes group (`"C:\Program Files\x.exe" --flag`), a backslash is an ordinary character
/// (Windows paths), and there is no escape. `None` when a quote is not closed.
pub(crate) fn split_command(text: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    items.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if started {
        items.push(current);
    }
    Some(items)
}

/// The inverse of [`split_command`] for display: elements with blanks (or none at all) are
/// quoted. An element that contains a double quote cannot be written back faithfully, which
/// is why a form keeps the original list and uses it while the text is unchanged.
pub(crate) fn join_command(items: &[String]) -> String {
    items
        .iter()
        .map(|item| {
            if item.is_empty() || item.chars().any(char::is_whitespace) {
                format!("\"{item}\"")
            } else {
                item.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A typed command line as an [`Argv`] whose placeholders are among `allowed`.
///
/// `original` is the list the text was made from; while the text is still that list's
/// [`join_command`] the list itself is used, so elements that cannot be written as text
/// survive an edit that does not touch them.
pub(crate) fn command(
    field: &'static str,
    text: &str,
    allowed: &[&str],
    original: Option<&Argv>,
    out: &mut Vec<Problem>,
) -> Option<Argv> {
    let text = required(field, text, out)?;
    let items = match original {
        Some(argv) if join_command(argv.items()) == text => argv.items().to_vec(),
        _ => match split_command(text) {
            Some(items) => items,
            None => {
                out.push(Problem::new(field, Code::CommandQuote));
                return None;
            }
        },
    };
    if items
        .first()
        .is_none_or(|program| program.trim().is_empty())
    {
        out.push(Problem::new(field, Code::Required));
        return None;
    }
    if let Ok(argv) = Argv::parse(items.clone(), allowed) {
        return Some(argv);
    }
    let unknown = items
        .iter()
        .flat_map(|item| placeholders(item))
        .find(|name| !allowed.contains(&name.as_str()));
    let allowed_text = if allowed.is_empty() {
        String::new()
    } else {
        allowed
            .iter()
            .map(|a| format!("{{{a}}}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    out.push(match unknown {
        Some(name) => Problem::new(field, Code::CommandPlaceholder)
            .with("name", format!("{{{name}}}"))
            .with("allowed", allowed_text),
        None => Problem::new(field, Code::CommandBraces),
    });
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn command_lines_split_without_a_shell() {
        assert_eq!(
            split_command("wakeonlan {mac}"),
            Some(v(&["wakeonlan", "{mac}"]))
        );
        assert_eq!(
            split_command(r#""C:\Program Files\x.exe" --a "b c""#),
            Some(v(&[r"C:\Program Files\x.exe", "--a", "b c"]))
        );
        assert_eq!(split_command(r#"a "" b"#), Some(v(&["a", "", "b"])));
        assert_eq!(split_command("  "), Some(vec![]));
        assert_eq!(split_command(r#"a "b"#), None);
        assert_eq!(split_command(r"a\b c"), Some(v(&[r"a\b", "c"])));
    }

    #[test]
    fn join_and_split_round_trip_for_ordinary_elements() {
        for items in [
            v(&["wakeonlan", "{mac}"]),
            v(&["ssh", "alex@{host}", "shutdown", "/s", "/t", "0"]),
            v(&[r"C:\Program Files\x.exe", "a b", ""]),
        ] {
            assert_eq!(split_command(&join_command(&items)), Some(items));
        }
    }

    #[test]
    fn commands_are_checked_like_the_file() {
        let mut out = Vec::new();
        assert!(command("c", "wake {mac}", &["host", "mac"], None, &mut out).is_some());
        assert!(out.is_empty());
        assert!(command("c", "wake {mac}", &["host"], None, &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::CommandPlaceholder));
        assert!(command("c", "wake {mac", &["host"], None, &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::CommandBraces));
        assert!(command("c", "wake \"x", &["host"], None, &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::CommandQuote));
        assert!(command("c", "   ", &["host"], None, &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::Required));
        assert!(command("c", "\"\" x", &["host"], None, &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::Required));
    }

    #[test]
    fn an_untouched_command_keeps_elements_that_cannot_be_typed() {
        let original = Argv::parse(v(&["say", "a\"b"]), &[]).unwrap();
        let text = join_command(original.items());
        let mut out = Vec::new();
        let again = command("c", &text, &[], Some(&original), &mut out).unwrap();
        assert_eq!(again, original);
        assert!(out.is_empty());
    }

    #[test]
    fn port_lists() {
        let mut out = Vec::new();
        assert_eq!(
            ports("p", "3389, 22 445", &mut out),
            Some(vec![3389, 22, 445])
        );
        assert!(ports("p", "", &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::PortsEmpty));
        assert!(ports("p", "22, 22", &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::PortDuplicate));
        assert!(ports("p", "22, x", &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::PortInvalid));
        assert!(ports("p", "70000", &mut out).is_none());
    }

    #[test]
    fn numbers_have_bounds() {
        let mut out = Vec::new();
        assert_eq!(seconds("s", "20", &mut out), Some(Duration::from_secs(20)));
        assert!(seconds("s", "0", &mut out).is_none());
        assert!(seconds("s", "3601", &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::SecondsRange));
        assert!(seconds("s", "abc", &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::NumberInvalid));
        assert!(millis("m", "3600001", &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::MillisRange));
    }

    #[test]
    fn wireguard_files() {
        let mut out = Vec::new();
        assert!(wireguard_conf("c", r"C:\x\home.conf.dpapi", &mut out).is_some());
        assert!(wireguard_conf("c", "home.txt", &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::ConfExtension));
        assert!(wireguard_conf("c", "a b.conf", &mut out).is_none());
        assert_eq!(out.last().map(|p| p.code), Some(Code::ConfName));
        assert_eq!(
            out.last()
                .and_then(|p| p.params.first())
                .map(|p| p.1.as_str()),
            Some("a b")
        );
    }
}

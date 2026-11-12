//! One test per validation rule of docs/rust-architecture.md section 8, each with a
//! negative case (an exact issue path) and, where it matters, accepted boundary values.
//! Cases start from `fixtures/valid/full.toml` and change a single thing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use homelink_core::config::{self, ConfigError, Issue};

fn full() -> String {
    include_str!("fixtures/valid/full.toml").to_owned()
}

/// The fixture with the first occurrence of `from` replaced by `to`.
fn change(from: &str, to: &str) -> String {
    let text = full();
    assert!(text.contains(from), "fixture lacks {from:?}");
    text.replacen(from, to, 1)
}

fn issues(text: &str) -> Vec<Issue> {
    match config::parse(text) {
        Err(ConfigError::Invalid(issues)) => issues,
        Err(other) => panic!("expected validation issues, got: {other}"),
        Ok(_) => panic!("expected validation issues, but the file is valid"),
    }
}

/// Asserts that an issue exists at `path` and that its message contains `fragment`.
#[track_caller]
fn rejected(text: &str, path: &str, fragment: &str) {
    let found = issues(text);
    let hit = found
        .iter()
        .any(|i| i.path == path && i.message.contains(fragment));
    assert!(
        hit,
        "no issue at {path} containing {fragment:?}; got:\n{}",
        render(&found)
    );
}

fn render(issues: &[Issue]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[track_caller]
fn accepted(text: &str) {
    if let Err(e) = config::parse(text) {
        panic!("expected a valid file: {e}");
    }
}

fn extra_tunnel(id: &str) -> String {
    format!(
        "{}\n[[tunnels]]\nid = \"{id}\"\nname = \"Extra\"\nkind = \"wireguard\"\n\
         conf = 'C:\\WG\\extra.conf'\nwireguard_exe = 'C:\\WG\\wireguard.exe'\n\
         probe_host = \"10.9.0.1\"\ntimeout = 20\nping_timeout_ms = 1000\n",
        full()
    )
}

fn without_line(containing: &str) -> String {
    let text = full();
    assert!(text.contains(containing), "fixture lacks {containing:?}");
    text.lines()
        .filter(|l| !l.contains(containing))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- 1. unknown keys and values are errors (reported by the parser) ----

#[test]
fn unknown_keys_and_values_are_errors() {
    for text in [
        change("level = \"info\"", "level = \"info\"\nextra = 1"),
        change("os = \"linux\"", "os = \"freebsd\""),
        change("method = \"mikrotik\"", "method = \"carrier_pigeon\""),
        change("kind = \"wireguard\"", "kind = \"openvpn\""),
        change("level = \"info\"", "level = \"loud\""),
    ] {
        assert!(
            matches!(config::parse(&text), Err(ConfigError::Syntax(_))),
            "{text}"
        );
    }
}

#[test]
fn a_missing_required_setting_is_an_error() {
    for key in [
        "poll_seconds = 5\n",
        "offline_after_failures = 3\n",
        "api_url = \"http://192.168.88.1\"\n",
    ] {
        let text = change(key, "");
        assert!(
            matches!(config::parse(&text), Err(ConfigError::Syntax(_))),
            "{key}"
        );
    }
}

// ---- 2. version and language ----

#[test]
fn version_must_be_supported() {
    rejected(
        &change("version = 1", "version = 2"),
        "version",
        "unsupported version 2",
    );
}

#[test]
fn language_is_auto_or_a_shipped_locale() {
    accepted(&change("language = \"auto\"", "language = \"uk\""));
    accepted(&change("language = \"auto\"", "language = \"en\""));
    rejected(
        &change("language = \"auto\"", "language = \"de\""),
        "language",
        "'auto' or one of",
    );
    rejected(
        &change("language = \"auto\"", "language = \"\""),
        "language",
        "got ''",
    );
}

// ---- 3. ids and names ----

#[test]
fn ids_are_slugs() {
    rejected(
        &change("id = \"home\"", "id = \"Home\""),
        "tunnels[0].id",
        "1-32 characters",
    );
    rejected(
        &change("id = \"desktop\"", "id = \"my pc\""),
        "pcs[0].id",
        "1-32 characters",
    );
    rejected(
        &change("id = \"mikrotik\"", "id = \"\""),
        "routers[0].id",
        "1-32 characters",
    );
    rejected(
        &change("id = \"desktop\"", &format!("id = \"{}\"", "x".repeat(33))),
        "pcs[0].id",
        "1-32 characters",
    );
    accepted(&change(
        "id = \"desktop\"",
        &format!("id = \"{}\"", "x".repeat(32)),
    ));
}

#[test]
fn ids_are_unique_within_their_kind() {
    rejected(
        &change("id = \"laptop\"", "id = \"desktop\""),
        "pcs[1].id",
        "duplicate id 'desktop' (also pcs[0])",
    );
    rejected(
        &extra_tunnel("home"),
        "tunnels[1].id",
        "duplicate id 'home'",
    );
    // The same id in different kinds is fine.
    accepted(
        &change("id = \"mikrotik\"", "id = \"home\"")
            .replace("router = \"mikrotik\"", "router = \"home\""),
    );
}

#[test]
fn names_must_not_be_empty() {
    rejected(
        &change("name = \"Desktop\"", "name = \"\""),
        "pcs[0].name",
        "must not be empty",
    );
    rejected(
        &change("name = \"Home\"", "name = \"  \""),
        "tunnels[0].name",
        "must not be empty",
    );
}

// ---- 4. references and unused entities ----

#[test]
fn references_must_exist() {
    rejected(
        &change("tunnel = \"home\"", "tunnel = \"ghost\""),
        "pcs[0].tunnel",
        "unknown tunnel 'ghost'",
    );
    rejected(
        &change("router = \"mikrotik\"", "router = \"ghost\""),
        "pcs[0].wake.router",
        "unknown router 'ghost'",
    );
}

#[test]
fn unused_tunnel_and_router_are_warnings_only() {
    let loaded = config::parse(&extra_tunnel("spare")).unwrap();
    assert_eq!(loaded.warnings.len(), 1);
    assert_eq!(loaded.warnings[0].path, "tunnels[1]");

    let no_wake = change("method = \"mikrotik\"", "method = \"magic_packet\"")
        .replace("router = \"mikrotik\"\n", "")
        .replace(
            "interface = \"bridge\"",
            "broadcast = \"255.255.255.255\"\nport = 9",
        );
    let loaded = config::parse(&no_wake).unwrap();
    assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
    assert_eq!(loaded.warnings[0].path, "routers[0]");

    assert!(config::parse(&full()).unwrap().warnings.is_empty());
}

// ---- 5. hosts and URLs ----

#[test]
fn hosts_are_addresses_or_names() {
    rejected(
        &change("host = \"192.168.88.10\"", "host = \"999.1.1.1\""),
        "pcs[0].host",
        "not a valid host",
    );
    rejected(
        &change("host = \"192.168.88.10\"", "host = \"a b\""),
        "pcs[0].host",
        "not a valid host",
    );
    rejected(
        &change("probe_host = \"192.168.88.1\"", "probe_host = \"\""),
        "tunnels[0].probe_host",
        "not a valid host",
    );
    accepted(&change("host = \"192.168.88.10\"", "host = \"fe80::1\""));
    accepted(&change("host = \"192.168.88.10\"", "host = \"desktop\""));
}

#[test]
fn api_url_needs_http_scheme_and_host() {
    for bad in [
        "ftp://192.168.88.1",
        "192.168.88.1",
        "http://",
        "http://user:pw@192.168.88.1",
        "http://r:0",
    ] {
        rejected(
            &change(
                "api_url = \"http://192.168.88.1\"",
                &format!("api_url = \"{bad}\""),
            ),
            "routers[0].api_url",
            "not a valid API URL",
        );
    }
    accepted(&change(
        "api_url = \"http://192.168.88.1\"",
        "api_url = \"https://router.lan:8443\"",
    ));
}

// ---- 6. ports ----

#[test]
fn ports_need_one_to_65535_without_duplicates() {
    rejected(
        &change("ports = [3389]", "ports = []"),
        "pcs[0].ports",
        "at least one port",
    );
    rejected(
        &change("ports = [3389]", "ports = [0]"),
        "pcs[0].ports[0]",
        "1 to 65535",
    );
    rejected(
        &change("ports = [3389]", "ports = [22, 70000]"),
        "pcs[0].ports[1]",
        "1 to 65535",
    );
    rejected(
        &change("ports = [3389]", "ports = [-1]"),
        "pcs[0].ports[0]",
        "1 to 65535",
    );
    rejected(
        &change("ports = [3389]", "ports = [22, 22]"),
        "pcs[0].ports[1]",
        "duplicate port 22",
    );
    accepted(&change("ports = [3389]", "ports = [1, 65535]"));
}

// ---- 7. numbers ----

#[test]
fn offline_after_failures_is_at_least_one() {
    rejected(
        &change("offline_after_failures = 3", "offline_after_failures = 0"),
        "defaults.offline_after_failures",
        "at least 1",
    );
    accepted(&change(
        "offline_after_failures = 3",
        "offline_after_failures = 1",
    ));
}

#[test]
fn timeouts_and_intervals_are_positive_and_bounded() {
    rejected(
        &change("\npoll_seconds = 5", "\npoll_seconds = 0"),
        "defaults.poll_seconds",
        "greater than 0",
    );
    rejected(
        &change("\npoll_seconds = 5", "\npoll_seconds = 3601"),
        "defaults.poll_seconds",
        "at most 3600",
    );
    rejected(
        &change("port_timeout_ms = 1000", "port_timeout_ms = 0"),
        "defaults.port_timeout_ms",
        "greater than 0",
    );
    rejected(
        &change("port_timeout_ms = 1000", "port_timeout_ms = 3600001"),
        "defaults.port_timeout_ms",
        "at most",
    );
    rejected(
        &change("power_timeout = 15", "power_timeout = 0"),
        "defaults.power_timeout",
        "greater than 0",
    );
    rejected(
        &change("\ntimeout = 20", "\ntimeout = 0"),
        "tunnels[0].timeout",
        "greater than 0",
    );
    rejected(
        &change("ping_timeout_ms = 1000", "ping_timeout_ms = 0"),
        "tunnels[0].ping_timeout_ms",
        "greater than 0",
    );
    rejected(
        &change("\ntimeout = 8", "\ntimeout = 3601"),
        "routers[0].timeout",
        "at most 3600",
    );
    rejected(
        &change("\ntimeout = 15", "\ntimeout = 0"),
        "pcs[1].wake.timeout",
        "greater than 0",
    );
    // Boundaries.
    accepted(
        &change("\npoll_seconds = 5", "\npoll_seconds = 3600")
            .replace("wake_timeout = 180", "wake_timeout = 3600"),
    );
    accepted(&change(
        "port_timeout_ms = 1000",
        "port_timeout_ms = 3600000",
    ));
}

#[test]
fn wake_intervals_must_be_consistent() {
    rejected(
        &change("wake_poll_seconds = 2", "wake_poll_seconds = 9"),
        "defaults",
        "wake_poll_seconds must not exceed poll_seconds",
    );
    rejected(
        &change("wake_timeout = 180", "wake_timeout = 1"),
        "defaults",
        "wake_timeout must be at least",
    );
    accepted(&change("wake_poll_seconds = 2", "wake_poll_seconds = 5"));
    accepted(&change("wake_timeout = 180", "wake_timeout = 2"));
}

#[test]
fn per_pc_overrides_are_validated_too() {
    rejected(
        &change(
            "[pcs.timing]\npoll_seconds = 10",
            "[pcs.timing]\npoll_seconds = 1",
        ),
        "pcs[0].timing",
        "must not exceed poll_seconds",
    );
    rejected(
        &change(
            "[pcs.timing]\npoll_seconds = 10",
            "[pcs.timing]\npoll_seconds = 0",
        ),
        "pcs[0].timing.poll_seconds",
        "greater than 0",
    );
    rejected(
        &change(
            "[pcs.timing]\npoll_seconds = 10",
            "[pcs.timing]\noffline_after_failures = 0",
        ),
        "pcs[0].timing.offline_after_failures",
        "at least 1",
    );
}

// ---- 8. MAC addresses and broadcast ----

#[test]
fn mac_addresses_are_six_hex_pairs() {
    for bad in [
        "AA:BB",
        "AA:BB:CC:DD:EE:GG",
        "AABBCCDDEEFF",
        "AA:BB-CC:DD:EE:FF",
    ] {
        rejected(
            &change("mac = \"AA:BB:CC:DD:EE:FF\"", &format!("mac = \"{bad}\"")),
            "pcs[0].wake.mac",
            "not a MAC address",
        );
    }
    rejected(
        &change("mac = \"11-22-33-44-55-66\"", "mac = \"1\""),
        "pcs[1].wake.mac",
        "not a MAC address",
    );
    accepted(&change(
        "mac = \"AA:BB:CC:DD:EE:FF\"",
        "mac = \"aa-bb-cc-dd-ee-ff\"",
    ));
}

#[test]
fn magic_packet_needs_ipv4_broadcast_and_port() {
    rejected(
        &change("broadcast = \"10.0.0.255\"", "broadcast = \"10.0.0\""),
        "pcs[2].wake.broadcast",
        "not an IPv4 address",
    );
    rejected(
        &change("broadcast = \"10.0.0.255\"", "broadcast = \"::1\""),
        "pcs[2].wake.broadcast",
        "not an IPv4 address",
    );
    rejected(
        &change("\nport = 9", "\nport = 0"),
        "pcs[2].wake.port",
        "1 to 65535",
    );
}

#[test]
fn mikrotik_needs_an_interface() {
    rejected(
        &change("interface = \"bridge\"", "interface = \"\""),
        "pcs[0].wake.interface",
        "must not be empty",
    );
}

// ---- 9. commands ----

#[test]
fn commands_are_non_empty_lists() {
    rejected(
        &change("command = [\"wakeonlan\", \"{mac}\"]", "command = []"),
        "pcs[1].wake.command",
        "at least one element",
    );
    rejected(
        &change(
            "command = [\"wakeonlan\", \"{mac}\"]",
            "command = [\"\", \"x\"]",
        ),
        "pcs[1].wake.command",
        "program",
    );
    rejected(
        &change("sleep = [\"ssh\"", "sleep = [\"\", \"ssh\""),
        "pcs[1].power.sleep",
        "program",
    );
}

#[test]
fn only_known_placeholders_are_allowed() {
    rejected(
        &change("\"{mac}\"]", "\"{nope}\"]"),
        "pcs[1].wake.command",
        "unknown placeholder '{nope}'",
    );
    rejected(
        &change("\"{mac}\"]", "\"{mac\"]"),
        "pcs[1].wake.command",
        "unbalanced",
    );
    rejected(
        &change("\"{mac}\"]", "\"mac}\"]"),
        "pcs[1].wake.command",
        "unbalanced",
    );
    rejected(
        &change("alex@{host}", "alex@{mac}"),
        "pcs[1].power.sleep",
        "unknown placeholder '{mac}'",
    );
    // {mac} needs a MAC address to substitute.
    rejected(
        &without_line("mac = \"11-22-33-44-55-66\""),
        "pcs[1].wake.command",
        "unknown placeholder '{mac}'",
    );
    accepted(&change("\"{mac}\"]", "\"{host}\", \"{mac}\"]"));
}

// ---- 10. WireGuard ----

#[test]
fn wireguard_conf_must_be_a_named_conf_file() {
    let conf = r"conf = 'C:\Program Files\WireGuard\Data\Configurations\home.conf.dpapi'";
    for bad in [
        r"C:\WG\home.txt",
        r"C:\WG\home",
        r"C:\WG\.conf",
        r"C:\WG\a b.conf",
    ] {
        rejected(
            &change(conf, &format!("conf = '{bad}'")),
            "tunnels[0].conf",
            "",
        );
    }
    let long = format!(r"C:\WG\{}.conf", "x".repeat(33));
    rejected(
        &change(conf, &format!("conf = '{long}'")),
        "tunnels[0].conf",
        "1-32 characters",
    );
    accepted(&change(conf, r"conf = 'C:\WG\Home_1.CONF'"));
    rejected(
        &change(
            r"wireguard_exe = 'C:\Program Files\WireGuard\wireguard.exe'",
            "wireguard_exe = ''",
        ),
        "tunnels[0].wireguard_exe",
        "must not be empty",
    );
}

// ---- 11. power methods ----

#[test]
fn ssh_needs_a_safe_user_and_a_port() {
    for bad in ["", "-oProxyCommand=x", "a@b", "a b"] {
        rejected(
            &change(
                "user = \"alex\"\nport = 22",
                &format!("user = \"{bad}\"\nport = 22"),
            ),
            "pcs[0].power.user",
            "",
        );
    }
    rejected(
        &change("user = \"alex\"\nport = 22", "user = \"alex\"\nport = 0"),
        "pcs[0].power.port",
        "1 to 65535",
    );
    rejected(
        &change("port = 2222", "port = 2222\nidentity_file = ''")
            .replace("identity_file = '/home/alex/.ssh/id_nas'\n", ""),
        "pcs[2].power.identity_file",
        "must not be empty",
    );
}

#[test]
fn command_power_needs_at_least_one_command() {
    let text = without_line("sleep = [\"ssh\"");
    rejected(&text, "pcs[1].power", "at least one of");
    accepted(&change("sleep = [", "shutdown = [\"poweroff\"]\nsleep = ["));
}

// ---- 12. secrets ----

#[test]
fn secret_reference_must_be_present_but_need_not_exist_yet() {
    rejected(
        &change(
            "password_ref = \"HomeLink/router/mikrotik\"",
            "password_ref = \"\"",
        ),
        "routers[0].password_ref",
        "secret reference",
    );
    rejected(
        &change(
            "password_ref = \"HomeLink/router/mikrotik\"",
            "password_ref = \" x\"",
        ),
        "routers[0].password_ref",
        "secret reference",
    );
    // Whether the secret exists is checked when it is used, not here.
    accepted(&change(
        "password_ref = \"HomeLink/router/mikrotik\"",
        "password_ref = \"never-stored\"",
    ));
}

// ---- 13. paths and RDP ----

#[test]
fn log_file_stays_inside_the_config_folder() {
    for bad in [
        "../x.log",
        "a/../../x.log",
        "/var/log/x.log",
        "\\x.log",
        "C:\\x.log",
        "c:x.log",
        "",
    ] {
        rejected(
            &change("file = \"homelink.log\"", &format!("file = '{bad}'")),
            "log.file",
            "",
        );
    }
    accepted(&change(
        "file = \"homelink.log\"",
        "file = \"logs/homelink.log\"",
    ));
    accepted(&change("file = \"homelink.log\"", "file = \"a..b.log\""));
}

#[test]
fn rdp_needs_a_user() {
    rejected(
        &change("[pcs.rdp]\nuser = \"alex\"", "[pcs.rdp]\nuser = \"\""),
        "pcs[0].rdp.user",
        "must not be empty",
    );
    accepted(&change(
        "options = [\"use redirection server name:i:1\", \"screen mode id:i:2\"]",
        "options = []",
    ));
}

// ---- all problems are reported together ----

#[test]
fn every_problem_is_reported_not_just_the_first() {
    let text = change("version = 1", "version = 9")
        .replace("ports = [3389]", "ports = []")
        .replace("host = \"192.168.88.10\"", "host = \"999.1.1.1\"")
        .replace("\ntimeout = 8", "\ntimeout = 0");
    let found = issues(&text);
    let paths: Vec<&str> = found.iter().map(|i| i.path.as_str()).collect();
    for expected in [
        "version",
        "pcs[0].ports",
        "pcs[0].host",
        "routers[0].timeout",
    ] {
        assert!(
            paths.contains(&expected),
            "{expected} missing from {paths:?}"
        );
    }
}

#[test]
fn error_text_lists_each_issue_on_its_own_line() {
    let text = change("ports = [3389]", "ports = []").replace("\ntimeout = 8", "\ntimeout = 0");
    let err = config::parse(&text).unwrap_err().to_string();
    assert!(
        err.contains("pcs[0].ports: must list at least one port"),
        "{err}"
    );
    assert!(
        err.contains("routers[0].timeout: must be greater than 0"),
        "{err}"
    );
    assert_eq!(err.lines().count(), 2, "{err}");
}

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
        change("method = \"relay\"", "method = \"carrier_pigeon\""),
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
        "identity_file = 'C:\\ProgramData\\HomeLink\\relay_ed25519'\n",
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
        &change("id = \"gateway\"", "id = \"\""),
        "relays[0].id",
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
        &change("id = \"gateway\"", "id = \"home\"")
            .replace("relay = \"gateway\"", "relay = \"home\""),
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
        &change("relay = \"gateway\"", "relay = \"ghost\""),
        "pcs[0].wake.relay",
        "unknown relay 'ghost'",
    );
}

#[test]
fn unused_tunnel_and_relay_are_warnings_only() {
    let loaded = config::parse(&extra_tunnel("spare")).unwrap();
    assert_eq!(loaded.warnings.len(), 1);
    assert_eq!(loaded.warnings[0].path, "tunnels[1]");

    let no_wake = change("method = \"relay\"", "method = \"magic_packet\"")
        .replace("relay = \"gateway\"\n", "")
        .replace(
            "interface = \"bridge\"",
            "address = \"255.255.255.255\"\nport = 9",
        );
    let loaded = config::parse(&no_wake).unwrap();
    assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
    assert_eq!(loaded.warnings[0].path, "relays[0]");

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
fn relay_needs_a_host_a_port_and_a_safe_user() {
    let host = "host = \"192.168.88.1\"\nport = 22\nuser = \"homelink\"";
    rejected(
        &change(host, "host = \"a b\"\nport = 22\nuser = \"homelink\""),
        "relays[0].host",
        "not a valid host",
    );
    rejected(
        &change(
            host,
            "host = \"192.168.88.1\"\nport = 0\nuser = \"homelink\"",
        ),
        "relays[0].port",
        "1 to 65535",
    );
    rejected(
        &change(
            host,
            "host = \"192.168.88.1\"\nport = 70000\nuser = \"homelink\"",
        ),
        "relays[0].port",
        "1 to 65535",
    );
    for bad in ["", "-oProxyCommand=x", "a@b", "a b"] {
        rejected(
            &change(
                host,
                &format!("host = \"192.168.88.1\"\nport = 22\nuser = \"{bad}\""),
            ),
            "relays[0].user",
            "",
        );
    }
    accepted(&change(
        host,
        "host = \"router.lan\"\nport = 2222\nuser = \"homelink\"",
    ));
}

#[test]
fn relay_key_must_be_an_explicit_absolute_path() {
    let key = r"identity_file = 'C:\ProgramData\HomeLink\relay_ed25519'";
    for bad in [
        "",
        "id_ed25519",
        r"..\id_ed25519",
        r"~/.ssh/id_ed25519",
        r"C:id_ed25519",
        r"keys\id_ed25519",
    ] {
        rejected(
            &change(key, &format!("identity_file = '{bad}'")),
            "relays[0].identity_file",
            "",
        );
    }
    for good in [
        r"C:\Users\me\.ssh\router",
        "C:/Users/me/.ssh/router",
        r"\\server\share\router",
        "/home/me/.ssh/router",
    ] {
        accepted(&change(key, &format!("identity_file = '{good}'")));
    }
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
        "relays[0].timeout",
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

// ---- 8. MAC addresses and magic packet destination ----

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
fn magic_packet_needs_an_ipv4_address_and_a_port() {
    for bad in ["10.0.0", "::1", "nas.lan", "10.0.0.256", ""] {
        rejected(
            &change("address = \"10.0.0.7\"", &format!("address = \"{bad}\"")),
            "pcs[2].wake.address",
            "not an IPv4 address",
        );
    }
    // Any IPv4 address is a destination: a PC (unicast) or a broadcast address.
    for good in ["10.0.0.7", "10.0.0.255", "255.255.255.255", "192.168.88.1"] {
        accepted(&change(
            "address = \"10.0.0.7\"",
            &format!("address = \"{good}\""),
        ));
    }
    rejected(
        &change("\nport = 9", "\nport = 0"),
        "pcs[2].wake.port",
        "1 to 65535",
    );
}

#[test]
fn an_interface_must_be_a_valid_name() {
    for bad in ["", "-h", "a b"] {
        rejected(
            &change("interface = \"bridge\"", &format!("interface = \"{bad}\"")),
            "pcs[0].wake.interface",
            "interface name",
        );
    }
}

// ---- relay presets: required and refused fields ----
// Fixture: desktop (pcs[0]) uses the routeros relay with an interface, media (pcs[3]) the
// etherwake relay with an interface, kiosk (pcs[4]) the wakeonlan relay with an address.

#[test]
fn routeros_and_etherwake_need_an_interface_and_refuse_an_address() {
    // routeros: desktop
    rejected(
        &change("interface = \"bridge\"\n", ""),
        "pcs[0].wake.interface",
        "required by the 'routeros' preset",
    );
    rejected(
        &change(
            "interface = \"bridge\"",
            "interface = \"bridge\"\naddress = \"10.0.0.255\"",
        ),
        "pcs[0].wake.address",
        "not used by the 'routeros' preset",
    );
    // etherwake: media
    rejected(
        &change("interface = \"br-lan\"\n", ""),
        "pcs[3].wake.interface",
        "required by the 'etherwake' preset",
    );
    rejected(
        &change(
            "interface = \"br-lan\"",
            "interface = \"br-lan\"\naddress = \"10.0.0.255\"",
        ),
        "pcs[3].wake.address",
        "not used by the 'etherwake' preset",
    );
    accepted(&change("interface = \"br-lan\"", "interface = \"eth0.2\""));
}

#[test]
fn wakeonlan_refuses_an_interface_and_takes_an_optional_address() {
    rejected(
        &change(
            "address = \"10.0.5.255\"",
            "address = \"10.0.5.255\"\ninterface = \"eth0\"",
        ),
        "pcs[4].wake.interface",
        "not used by the 'wakeonlan' preset",
    );
    accepted(&change("address = \"10.0.5.255\"\n", ""));
    accepted(&change(
        "address = \"10.0.5.255\"",
        "address = \"10.0.0.1\"",
    ));
    rejected(
        &change("address = \"10.0.5.255\"", "address = \"10.0.5\""),
        "pcs[4].wake.address",
        "not an IPv4 address",
    );
}

#[test]
fn the_fields_are_checked_against_the_preset_of_the_relay_that_is_used() {
    // Make the routeros relay a wakeonlan one: the desktop's interface is now unused.
    rejected(
        &change("kind = \"routeros\"", "kind = \"wakeonlan\""),
        "pcs[0].wake.interface",
        "not used by the 'wakeonlan' preset of relay 'gateway'",
    );
    // And the etherwake relay a routeros one: the media PC's interface is fine.
    accepted(&change("kind = \"etherwake\"", "kind = \"routeros\""));
}

#[test]
fn an_unknown_preset_is_an_error() {
    let text = change("kind = \"routeros\"", "kind = \"ubnt\"");
    assert!(matches!(config::parse(&text), Err(ConfigError::Syntax(_))));
}

#[test]
fn the_wake_method_must_name_an_existing_relay_and_a_valid_mac() {
    rejected(
        &change("relay = \"openwrt\"", "relay = \"ghost\""),
        "pcs[3].wake.relay",
        "unknown relay 'ghost'",
    );
    rejected(
        &change("mac = \"00:11:22:33:44:55\"", "mac = \"00:11:22:33:44\""),
        "pcs[3].wake.mac",
        "not a MAC address",
    );
}

#[test]
fn every_relay_must_be_used_or_it_is_a_warning() {
    let text = change("relay = \"pi\"", "relay = \"openwrt\"")
        .replace("address = \"10.0.5.255\"", "interface = \"br-lan\"");
    let loaded = config::parse(&text).unwrap();
    assert!(
        loaded
            .warnings
            .iter()
            .any(|w| w.path == "relays[2]" && w.message.contains("'pi'"))
    );
}

#[test]
fn the_old_router_and_broadcast_keys_are_gone() {
    for text in [
        change("[[relays]]", "[[routers]]"),
        change("relay = \"gateway\"", "router = \"gateway\""),
        change("address = \"10.0.0.7\"", "broadcast = \"10.0.0.255\""),
        change("method = \"relay\"", "method = \"mikrotik\""),
    ] {
        assert!(
            config::parse(&text).is_err(),
            "an old name was accepted:\n{text}"
        );
    }
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
}

#[test]
fn ssh_power_key_must_be_an_explicit_absolute_path() {
    let key = r"identity_file = 'C:\ProgramData\HomeLink\desktop_ed25519'";
    for bad in [
        "",
        "id_ed25519",
        r"..\id_ed25519",
        r"~/.ssh/id_ed25519",
        r"C:id_ed25519",
        r"keys\id_ed25519",
    ] {
        rejected(
            &change(key, &format!("identity_file = '{bad}'")),
            "pcs[0].power.identity_file",
            "",
        );
    }
    // The second SSH PC (the NAS) is reported under its own index.
    rejected(
        &change(
            "identity_file = '/home/alex/.ssh/id_nas'",
            "identity_file = 'id_nas'",
        ),
        "pcs[2].power.identity_file",
        "absolute path",
    );
    for good in [
        r"C:\Users\me\.ssh\desktop",
        "C:/Users/me/.ssh/desktop",
        r"\\server\share\desktop",
        "/home/me/.ssh/desktop",
    ] {
        accepted(&change(key, &format!("identity_file = '{good}'")));
    }
}

#[test]
fn ssh_power_without_a_key_is_an_error() {
    let text = without_line("identity_file = 'C:\\ProgramData\\HomeLink\\desktop_ed25519'");
    match config::parse(&text) {
        Err(ConfigError::Syntax(e)) => {
            assert!(e.message.contains("identity_file"), "{e}");
        }
        other => panic!("expected a missing-field error, got {other:?}"),
    }
}

#[test]
fn command_power_needs_at_least_one_command() {
    let text = without_line("sleep = [\"ssh\"");
    rejected(&text, "pcs[1].power", "at least one of");
    accepted(&change("sleep = [", "shutdown = [\"poweroff\"]\nsleep = ["));
}

// ---- 12. no secrets in the file, and nothing that can inject syntax ----

#[test]
fn the_old_rest_and_password_keys_are_unknown_now() {
    for extra in [
        "api_url = \"http://192.168.88.1\"",
        "password_ref = \"HomeLink/relay/gateway\"",
        "verify_tls = false",
        "password = \"hunter2\"",
    ] {
        let text = change(
            "kind = \"routeros\"",
            &format!("kind = \"routeros\"\n{extra}"),
        );
        assert!(
            matches!(config::parse(&text), Err(ConfigError::Syntax(_))),
            "{extra}"
        );
    }
}

#[test]
fn interface_name_has_a_closed_character_set() {
    for bad in [
        "",
        "a b",
        "bridge; /system reboot",
        "a\"b",
        "$(x)",
        "[x]",
        "a=b",
        "мережа",
    ] {
        rejected(
            &change("interface = \"bridge\"", &format!("interface = '{bad}'")),
            "pcs[0].wake.interface",
            "interface name",
        );
    }
    for good in ["ether1", "vlan-10", "br_lan.2"] {
        accepted(&change(
            "interface = \"bridge\"",
            &format!("interface = \"{good}\""),
        ));
    }
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
        "relays[0].timeout",
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
        err.contains("relays[0].timeout: must be greater than 0"),
        "{err}"
    );
    // One line for the ports and one for each of the two relays whose timeout is 8.
    assert_eq!(err.lines().count(), 3, "{err}");
}

//! Loading and saving real files: fixtures, round trips, parse errors.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use homelink_core::config::{self, Config, ConfigError};
use homelink_core::domain::{Os, PowerMethod, RelayKind, WakeMethod};
use homelink_core::i18n::{Lang, LanguageSetting};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A fresh directory for one test.
fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("homelink-core-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn load(name: &str) -> Config {
    config::load(&fixture(name)).unwrap().config
}

#[test]
fn full_fixture_loads_with_every_value() {
    let loaded = config::load(&fixture("valid/full.toml")).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let c = loaded.config;

    assert_eq!(c.language, LanguageSetting::Auto);
    assert_eq!(c.log.file, "homelink.log");
    assert_eq!(c.defaults.poll, Duration::from_secs(5));
    assert_eq!(c.defaults.port_timeout, Duration::from_millis(1000));
    assert_eq!(c.defaults.offline_after_failures, 3);

    assert_eq!(c.tunnels.len(), 1);
    assert_eq!(c.tunnels[0].timeout, Duration::from_secs(20));
    assert_eq!(c.relays.len(), 3);
    let relay = &c.relays[0];
    assert_eq!((relay.host.as_str(), relay.port), ("192.168.88.1", 22));
    assert_eq!(relay.user, "homelink");
    assert_eq!(
        relay.identity_file,
        r"C:\ProgramData\HomeLink\relay_ed25519"
    );
    assert_eq!(relay.timeout, Duration::from_secs(8));
    assert_eq!(c.pcs.len(), 5);

    let desktop = &c.pcs[0];
    assert_eq!(desktop.os, Os::Linux);
    assert_eq!(desktop.tunnel.as_ref().map(|t| t.as_str()), Some("home"));
    assert_eq!(
        desktop.timing.poll,
        Duration::from_secs(10),
        "override applied"
    );
    assert_eq!(
        desktop.timing.wake_poll,
        Duration::from_secs(2),
        "default kept"
    );
    assert!(matches!(desktop.wake, Some(WakeMethod::Relay { .. })));
    assert!(matches!(
        desktop.power,
        Some(PowerMethod::Ssh { port: 22, .. })
    ));

    let laptop = &c.pcs[1];
    assert_eq!(laptop.os, Os::Windows);
    assert_eq!(laptop.timing, c.defaults);
    assert_eq!(laptop.ports, [3389, 445]);
    let Some(WakeMethod::Command { argv, mac, timeout }) = &laptop.wake else {
        panic!("expected a command wake method");
    };
    assert_eq!(*timeout, Duration::from_secs(15));
    let expanded = argv.expand(&laptop.host, mac.as_ref());
    assert_eq!(expanded, ["wakeonlan", "11:22:33:44:55:66"]);
    assert!(matches!(
        &laptop.power,
        Some(PowerMethod::Command {
            sleep: Some(_),
            shutdown: None
        })
    ));

    let nas = &c.pcs[2];
    assert!(nas.tunnel.is_none());
    // The destination of a magic packet is any IPv4 address; here the PC's own (unicast).
    let Some(WakeMethod::MagicPacket { address, port, .. }) = &nas.wake else {
        panic!("expected a magic packet");
    };
    assert_eq!((address.to_string().as_str(), *port), ("10.0.0.7", 9));

    // One relay of each preset.
    let kinds: Vec<RelayKind> = c.relays.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            RelayKind::RouterOs,
            RelayKind::Etherwake,
            RelayKind::Wakeonlan
        ]
    );
    let Some(WakeMethod::Relay {
        interface: Some(interface),
        address: None,
        ..
    }) = &c.pcs[3].wake
    else {
        panic!("the media PC wakes through etherwake on an interface");
    };
    assert_eq!(interface.as_str(), "br-lan");
    let Some(WakeMethod::Relay {
        interface: None,
        address: Some(address),
        ..
    }) = &c.pcs[4].wake
    else {
        panic!("the kiosk wakes through wakeonlan with an address");
    };
    assert_eq!(address.to_string(), "10.0.5.255");
}

#[test]
fn lookups_and_grouping() {
    let c = load("valid/full.toml");
    let home = &c.tunnels[0].id;
    let ids = |tunnel| {
        c.pcs_of(tunnel)
            .map(|p| p.id.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(Some(home)), ["desktop", "laptop"]);
    assert_eq!(ids(None), ["nas", "media", "kiosk"]);
    assert!(c.tunnel(home).is_some());
    assert!(c.relay(&c.relays[0].id).is_some());
    assert!(c.pc(&c.pcs[2].id).is_some());
}

#[test]
fn minimal_fixture_loads_without_entities() {
    let c = load("valid/minimal.toml");
    assert_eq!(c.language, LanguageSetting::Fixed(Lang::En));
    assert!(c.tunnels.is_empty() && c.relays.is_empty() && c.pcs.is_empty());
}

#[test]
fn save_then_load_gives_the_same_configuration() {
    let dir = temp_dir("round-trip");
    for name in ["valid/full.toml", "valid/minimal.toml"] {
        let original = load(name);
        let path = dir.join(config::FILE_NAME);
        config::save(&path, &original).unwrap();
        assert!(
            !dir.join("config.toml.tmp").exists(),
            "temporary file left behind"
        );
        let reloaded = config::load(&path).unwrap().config;
        assert_eq!(reloaded, original, "{name}");
        // Saving again changes nothing.
        assert_eq!(
            config::render(&reloaded).unwrap(),
            std::fs::read_to_string(&path).unwrap()
        );
    }
}

#[test]
fn only_overrides_that_differ_from_defaults_are_written() {
    let mut c = load("valid/full.toml");
    c.pcs[0].timing.poll = c.defaults.poll;
    let text = config::render(&c).unwrap();
    assert!(!text.contains("[pcs.timing]"), "{text}");
    c.pcs[1].timing.wake_timeout = Duration::from_secs(300);
    let text = config::render(&c).unwrap();
    assert_eq!(text.matches("[pcs.timing]").count(), 1, "{text}");
    assert_eq!(config::parse(&text).unwrap().config, c);
}

#[test]
fn save_updates_an_existing_valid_file() {
    let dir = temp_dir("replace");
    let path = dir.join(config::FILE_NAME);
    std::fs::copy(fixture("valid/full.toml"), &path).unwrap();
    config::save(&path, &load("valid/minimal.toml")).unwrap();
    assert_eq!(
        config::load(&path).unwrap().config,
        load("valid/minimal.toml")
    );
}

#[test]
fn save_never_overwrites_a_file_that_is_not_toml() {
    let dir = temp_dir("not-toml");
    let path = dir.join(config::FILE_NAME);
    std::fs::write(&path, "old = [").unwrap();
    let err = config::save(&path, &load("valid/minimal.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Syntax(_)), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "old = [");
    assert!(!dir.join("config.toml.tmp").exists());
}

#[test]
fn render_refuses_an_invalid_configuration() {
    let mut c = load("valid/full.toml");
    c.pcs[0].ports.clear();
    assert!(matches!(config::render(&c), Err(ConfigError::Invalid(_))));
}

#[test]
fn missing_file_is_an_io_error_naming_the_path() {
    let err = config::load(&fixture("valid/nope.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Io { .. }));
    assert!(err.to_string().contains("nope.toml"), "{err}");
}

#[test]
fn syntax_error_reports_line_and_column() {
    let err = config::load(&fixture("invalid/syntax.toml")).unwrap_err();
    let ConfigError::Syntax(e) = err else {
        panic!("expected a syntax error, got {err}")
    };
    assert_eq!(e.line, Some(2), "{e}");
    assert!(e.column.is_some());
}

#[test]
fn missing_section_is_reported_by_name() {
    let err = config::load(&fixture("invalid/missing_defaults.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Syntax(_)));
    assert!(err.to_string().contains("defaults"), "{err}");
}

#[test]
fn byte_order_mark_is_accepted() {
    let text = format!(
        "\u{feff}{}",
        std::fs::read_to_string(fixture("valid/minimal.toml")).unwrap()
    );
    assert!(config::parse(&text).is_ok());
}

//! The wizard against the configuration file: every PC survives `Pc -> draft -> Pc`, saving it
//! changes nothing in the file, and the wizard accepts an input exactly when the file would.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use homelink_core::config::{self, Config};
use homelink_core::problem::{Code, Problem};
use homelink_core::settings;
use homelink_core::testing::{demo_config, multi_config, pc_id, relay_config, single_config};
use homelink_core::wizard::{PcDraft, PowerChoice, RelayChoice, Step, WakeChoice, field};

const FULL: &str = include_str!("fixtures/valid/full.toml");
const COMMENTED: &str = include_str!("fixtures/valid/commented.toml");
const MINIMAL: &str = include_str!("fixtures/valid/minimal.toml");

fn configs() -> Vec<(&'static str, Config)> {
    vec![
        ("single", single_config()),
        ("multi", multi_config()),
        ("relays", relay_config()),
        ("demo", demo_config()),
        ("full.toml", config::parse(FULL).unwrap().config),
        ("commented.toml", config::parse(COMMENTED).unwrap().config),
        ("minimal.toml", config::parse(MINIMAL).unwrap().config),
    ]
}

#[test]
fn every_pc_survives_pc_to_draft_to_pc() {
    let mut checked = 0;
    for (name, config) in configs() {
        for pc in &config.pcs {
            let draft = PcDraft::from_pc(&config, pc);
            assert_eq!(draft.editing(), Some(&pc.id));
            let outcome = draft
                .build(&config)
                .unwrap_or_else(|p| panic!("{name}/{}: {p:?}", pc.id));
            assert_eq!(outcome.pc, *pc, "{name}/{}", pc.id);
            assert_eq!(outcome.new_relay, None);
            assert_eq!(outcome.replaces.as_ref(), Some(&pc.id));
            checked += 1;
        }
    }
    assert!(checked >= 10, "only {checked} PCs were checked");
}

#[test]
fn saving_an_unchanged_pc_leaves_the_file_byte_for_byte() {
    for text in [FULL, COMMENTED, MINIMAL] {
        let config = config::parse(text).unwrap().config;
        for pc in &config.pcs {
            let outcome = PcDraft::from_pc(&config, pc).build(&config).unwrap();
            let change = settings::save_pc(&config, &outcome).unwrap();
            assert_eq!(change.config, config, "{}", pc.id);
            assert!(change.removed_relays.is_empty());
            let edited = config::edit(text, &change.config, &change.renames).unwrap();
            assert_eq!(edited, text, "{}", pc.id);
        }
    }
}

#[test]
fn a_changed_pc_keeps_the_comments_of_the_file() {
    let config = config::parse(COMMENTED).unwrap().config;
    let pc = &config.pcs[0];
    let mut draft = PcDraft::from_pc(&config, pc);
    draft.ports = "22".into();
    let change = settings::save_pc(&config, &draft.build(&config).unwrap()).unwrap();
    let edited = config::edit(COMMENTED, &change.config, &change.renames).unwrap();
    assert_ne!(edited, COMMENTED);
    for line in COMMENTED
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
    {
        assert!(edited.contains(line), "lost: {line}");
    }
}

/// A file with one relay and one PC whose values come from the test.
struct Case {
    host: &'static str,
    ports: &'static str,
    mac: &'static str,
    interface: &'static str,
    user: &'static str,
    key: &'static str,
    port: &'static str,
    rhost: &'static str,
    ruser: &'static str,
    rkey: &'static str,
    rport: &'static str,
}

const GOOD: Case = Case {
    host: "192.168.88.10",
    ports: "3389, 22",
    mac: "AA:BB:CC:DD:EE:FF",
    interface: "bridge",
    user: "alex",
    key: r"C:\k\pc",
    port: "22",
    rhost: "192.168.88.1",
    ruser: "homelink",
    rkey: r"C:\k\relay",
    rport: "22",
};

impl Case {
    fn file_ok(&self) -> bool {
        let text = format!(
            r#"
version = 1
language = "en"
[log]
file = "h.log"
level = "info"
[defaults]
poll_seconds = 5
wake_poll_seconds = 2
port_timeout_ms = 1000
offline_after_failures = 3
wake_timeout = 180
power_timeout = 15
[[relays]]
id = "routeros-192-168-88-1"
kind = "etherwake"
host = '{rhost}'
port = {rport}
user = '{ruser}'
identity_file = '{rkey}'
timeout = 15
sudo = true
[[pcs]]
id = "x"
name = "X"
os = "windows"
host = '{host}'
ports = [{ports}]
[pcs.wake]
method = "relay"
relay = "routeros-192-168-88-1"
mac = '{mac}'
interface = '{interface}'
[pcs.power]
method = "ssh"
user = '{user}'
port = {port}
identity_file = '{key}'
"#,
            rhost = self.rhost,
            rport = self.rport,
            ruser = self.ruser,
            rkey = self.rkey,
            host = self.host,
            ports = self.ports,
            mac = self.mac,
            interface = self.interface,
            user = self.user,
            port = self.port,
            key = self.key,
        );
        config::parse(&text).is_ok()
    }

    fn wizard_ok(&self) -> bool {
        let config = single_config();
        let mut d = PcDraft::new(&config);
        d.name = "X".into();
        d.host = self.host.into();
        d.ports = self.ports.into();
        d.wake = WakeChoice::Relay;
        d.relay = RelayChoice::New;
        d.new_relay.kind = homelink_core::domain::RelayKind::Etherwake;
        d.new_relay.sudo = Some(true);
        d.new_relay.host = self.rhost.into();
        d.new_relay.user = self.ruser.into();
        d.new_relay.identity_file = self.rkey.into();
        d.new_relay.port = self.rport.into();
        d.mac = self.mac.into();
        d.interface = self.interface.into();
        d.allow_sleep = true;
        d.allow_shutdown = true;
        d.power = PowerChoice::Ssh;
        d.ssh_user = self.user.into();
        d.ssh_identity_file = self.key.into();
        d.ssh_port = self.port.into();
        d.build(&config).is_ok()
    }
}

#[test]
fn the_wizard_accepts_an_input_exactly_when_the_file_would() {
    assert!(GOOD.file_ok() && GOOD.wizard_ok());
    let variants: Vec<(&str, Case)> = vec![
        (
            "host name",
            Case {
                host: "pc.lan",
                ..GOOD
            },
        ),
        (
            "host digits",
            Case {
                host: "999.1.1.1",
                ..GOOD
            },
        ),
        (
            "host blank",
            Case {
                host: "a b",
                ..GOOD
            },
        ),
        ("host dash", Case { host: "-a", ..GOOD }),
        (
            "ports one",
            Case {
                ports: "22",
                ..GOOD
            },
        ),
        (
            "ports dup",
            Case {
                ports: "22, 22",
                ..GOOD
            },
        ),
        ("ports zero", Case { ports: "0", ..GOOD }),
        (
            "ports big",
            Case {
                ports: "65536",
                ..GOOD
            },
        ),
        (
            "mac dashes",
            Case {
                mac: "aa-bb-cc-dd-ee-ff",
                ..GOOD
            },
        ),
        (
            "mac short",
            Case {
                mac: "AA:BB:CC:DD:EE",
                ..GOOD
            },
        ),
        (
            "mac hex",
            Case {
                mac: "AA:BB:CC:DD:EE:GG",
                ..GOOD
            },
        ),
        (
            "iface ok",
            Case {
                interface: "br_lan.2",
                ..GOOD
            },
        ),
        (
            "iface dash",
            Case {
                interface: "-h",
                ..GOOD
            },
        ),
        (
            "iface semicolon",
            Case {
                interface: "a;b",
                ..GOOD
            },
        ),
        (
            "user at",
            Case {
                user: "a@b",
                ..GOOD
            },
        ),
        ("user dash", Case { user: "-x", ..GOOD }),
        (
            "user space",
            Case {
                user: "a b",
                ..GOOD
            },
        ),
        (
            "key unix",
            Case {
                key: "/home/a/k",
                ..GOOD
            },
        ),
        ("key relative", Case { key: "k", ..GOOD }),
        (
            "key unc",
            Case {
                key: r"\\srv\k",
                ..GOOD
            },
        ),
        ("port 0", Case { port: "0", ..GOOD }),
        (
            "port big",
            Case {
                port: "70000",
                ..GOOD
            },
        ),
        (
            "relay host bad",
            Case {
                rhost: "a_b",
                ..GOOD
            },
        ),
        (
            "relay user bad",
            Case {
                ruser: "a b",
                ..GOOD
            },
        ),
        (
            "relay key bad",
            Case {
                rkey: "~/k",
                ..GOOD
            },
        ),
        ("relay port bad", Case { rport: "0", ..GOOD }),
    ];
    for (what, case) in variants {
        assert_eq!(
            case.wizard_ok(),
            case.file_ok(),
            "the wizard and the file disagree about: {what}"
        );
    }
}

fn blank(config: &Config) -> PcDraft {
    PcDraft::new(config)
}

fn fields(problems: &[Problem]) -> Vec<(&'static str, Code)> {
    problems.iter().map(|p| (p.field, p.code)).collect()
}

#[test]
fn each_step_reports_its_own_fields() {
    let config = single_config();
    let d = blank(&config);
    assert_eq!(
        fields(&d.validate_step(Step::Address, &config)),
        [
            (field::NAME, Code::Required),
            (field::HOST, Code::Required),
            (field::PORTS, Code::PortsEmpty)
        ]
    );
    assert!(d.validate_step(Step::Tunnel, &config).is_empty());
    assert!(d.validate_step(Step::Wake, &config).is_empty()); // "do not wake"
    assert!(d.validate_step(Step::Power, &config).is_empty()); // nothing allowed
    assert!(d.validate_step(Step::Checks, &config).is_empty());

    let mut d = blank(&config);
    d.tunnel = Some(homelink_core::testing::tunnel_id("ghost"));
    assert_eq!(
        fields(&d.validate_step(Step::Tunnel, &config)),
        [(field::TUNNEL, Code::TunnelUnknown)]
    );
}

#[test]
fn step_three_depends_on_the_method_and_the_preset() {
    let config = single_config();
    let mut d = blank(&config);
    d.wake = WakeChoice::MagicPacket;
    assert_eq!(
        fields(&d.validate_step(Step::Wake, &config)),
        [(field::MAC, Code::Required)]
    );
    d.mac = "AA:BB:CC:DD:EE:FF".into();
    d.magic_address = "nope".into();
    assert_eq!(
        fields(&d.validate_step(Step::Wake, &config)),
        [(field::MAGIC_ADDRESS, Code::Ipv4Invalid)]
    );

    // A relay: existing one is RouterOS, which needs an interface and has no sudo question.
    let mut d = blank(&config);
    d.wake = WakeChoice::Relay;
    d.mac = "AA:BB:CC:DD:EE:FF".into();
    assert_eq!(
        fields(&d.validate_step(Step::Wake, &config)),
        [(field::INTERFACE, Code::InterfaceRequired)]
    );
    // A new wakeonlan relay: no interface, an optional address, sudo must be answered.
    d.relay = RelayChoice::New;
    d.new_relay.kind = homelink_core::domain::RelayKind::Wakeonlan;
    d.new_relay.host = "10.0.0.2".into();
    d.new_relay.user = "u".into();
    d.new_relay.identity_file = "/k".into();
    assert_eq!(
        fields(&d.validate_step(Step::Wake, &config)),
        [(field::RELAY_SUDO, Code::SudoRequired)]
    );
    d.new_relay.sudo = Some(false);
    d.interface = "stale text is ignored".into();
    assert!(d.validate_step(Step::Wake, &config).is_empty());
    d.relay_address = "x".into();
    assert_eq!(
        fields(&d.validate_step(Step::Wake, &config)),
        [(field::RELAY_ADDRESS, Code::Ipv4Invalid)]
    );
    assert_eq!(
        d.relay_kind(&config),
        Some(homelink_core::domain::RelayKind::Wakeonlan)
    );
}

#[test]
fn step_four_checks_the_boxes_against_the_method() {
    let config = single_config();
    let mut d = blank(&config);
    d.allow_sleep = true;
    d.power = PowerChoice::Ssh;
    d.ssh_user = "u".into();
    d.ssh_identity_file = "/k".into();
    assert_eq!(
        fields(&d.validate_step(Step::Power, &config)),
        [(field::ALLOW_SHUTDOWN, Code::PowerSshNeedsBoth)]
    );
    d.allow_shutdown = true;
    assert!(d.validate_step(Step::Power, &config).is_empty());
    // Commands: each box on its own, its command required.
    d.power = PowerChoice::Command;
    d.allow_shutdown = false;
    assert_eq!(
        fields(&d.validate_step(Step::Power, &config)),
        [(field::SLEEP_COMMAND, Code::Required)]
    );
    d.sleep_command = "ssh u@{host} \"systemctl suspend\"".into();
    assert!(d.validate_step(Step::Power, &config).is_empty());
    d.allow_rdp = true;
    assert_eq!(
        fields(&d.validate_step(Step::Power, &config)),
        [(field::RDP_USER, Code::Required)]
    );
}

#[test]
fn a_new_pc_gets_a_unique_id_from_its_name_and_defaults_from_the_file() {
    let config = single_config();
    let mut d = blank(&config);
    d.name = "Desktop".into(); // taken (case-insensitively), as is its slug
    d.host = "10.0.0.9".into();
    d.ports = "22".into();
    assert_eq!(
        fields(&d.build(&config).unwrap_err()),
        [(field::NAME, Code::NameTaken)]
    );
    d.name = "Desktop 2".into();
    let outcome = d.build(&config).unwrap();
    assert_eq!(outcome.pc.id.as_str(), "desktop-2");
    assert_eq!(outcome.pc.timing, config.defaults);
    assert_eq!(outcome.replaces, None);
    assert!(
        d.build(&config)
            .map(|o| o.pc.id != pc_id("desktop"))
            .unwrap()
    );
}

#[test]
fn problems_render_in_both_languages_without_leftover_placeholders() {
    use homelink_core::i18n::{Lang, Translator};
    for lang in Lang::ALL {
        let tr = Translator::new(lang);
        for code in Code::ALL {
            let mut problem = Problem::new("f", code);
            for name in code.params() {
                problem = problem.with(name, "VALUE");
            }
            let text = tr.problem(&problem);
            assert!(!text.starts_with("problem."), "{lang:?} {code:?}: {text}");
            assert!(!text.contains("{"), "{lang:?} {code:?}: {text}");
        }
    }
}

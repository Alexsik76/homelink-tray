//! Saving edits the existing file by key: what the user wrote by hand survives byte for byte
//! unless it changed. See the `format` module of `homelink_core::config` for the rules.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::time::Duration;

use homelink_core::config::{self, Config, ConfigError, Renames};
use homelink_core::domain::{Host, PcId, RelayId, RelayKind, TunnelId, WakeMethod};

const COMMENTED: &str = include_str!("fixtures/valid/commented.toml");
const FULL: &str = include_str!("fixtures/valid/full.toml");

fn cfg(text: &str) -> Config {
    config::parse(text).unwrap().config
}

fn edit(text: &str, config: &Config) -> String {
    config::edit(text, config, &Renames::default()).unwrap()
}

fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("homelink-save-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn at(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not found in:\n{text}"))
}

/// A new PC modelled on the NAS (no tunnel).
fn extra_pc(config: &Config, id: &str) -> homelink_core::domain::Pc {
    let mut pc = config.pcs.last().unwrap().clone();
    pc.id = PcId::parse(id).unwrap();
    pc.name = id.to_owned();
    pc.host = Host::parse("10.0.0.99").unwrap();
    pc
}

// ---------- nothing changes ----------

#[test]
fn an_unchanged_configuration_gives_identical_bytes() {
    for text in [COMMENTED, FULL] {
        assert_eq!(edit(text, &cfg(text)), text);
    }
}

#[test]
fn saving_an_unchanged_configuration_does_not_touch_the_file() {
    let dir = temp_dir("untouched");
    let path = dir.join("config.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    config::save(&path, &cfg(COMMENTED)).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
    assert!(!dir.join("config.toml.tmp").exists());
}

#[test]
fn the_same_content_written_in_another_style_is_left_alone() {
    // An inline table and a standard table with the same content are equal.
    let text = COMMENTED.replace(
        "  [pcs.rdp]\n  user = \"alex\"\n  options = [\n    \"use redirection server name:i:1\",   # keep this one\n    \"screen mode id:i:2\",\n  ]\n",
        "  rdp = { user = \"alex\", options = [\"use redirection server name:i:1\", \"screen mode id:i:2\"] }\n",
    );
    assert_ne!(text, COMMENTED);
    // The inline `rdp` has to come before the sub-tables; this file is only valid TOML if
    // it does, so just check the editor on a file it can parse.
    if let Ok(loaded) = config::parse(&text) {
        assert_eq!(edit(&text, &loaded.config), text);
    }
}

// ---------- changed values ----------

#[test]
fn one_changed_value_changes_only_that_value() {
    let mut c = cfg(COMMENTED);
    c.pcs[0].host = Host::parse("192.168.88.99").unwrap();
    let expected = COMMENTED.replace("host = \"192.168.88.10\"", "host = \"192.168.88.99\"");
    assert_eq!(edit(COMMENTED, &c), expected);
}

#[test]
fn a_changed_value_keeps_its_trailing_comment() {
    let mut c = cfg(COMMENTED);
    c.pcs[1].timing.wake_timeout = Duration::from_secs(240);
    c.pcs[0].ports = vec![3389, 22];
    c.pcs[0].name = "Study PC".into();
    c.language = homelink_core::i18n::LanguageSetting::Auto;
    let expected = COMMENTED
        .replace(
            "wake_timeout = 300        # five minutes",
            "wake_timeout = 240        # five minutes",
        )
        .replace(
            "ports = [3389]            # RDP",
            "ports = [3389, 22]            # RDP",
        )
        .replace(
            "name = \"Desktop\"          # the one in the study",
            "name = \"Study PC\"          # the one in the study",
        )
        .replace(
            "language = \"uk\"        # auto | en | uk",
            "language = \"auto\"        # auto | en | uk",
        );
    assert_eq!(edit(COMMENTED, &c), expected);
}

#[test]
fn a_changed_multiline_array_is_replaced_and_everything_around_it_is_kept() {
    let mut c = cfg(COMMENTED);
    c.pcs[0].rdp.as_mut().unwrap().options.push("extra".into());
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    assert!(out.contains("# How to wake it."));
    assert!(out.contains("mac = \"AA:BB:CC:DD:EE:FF\"   # eth0"));
    assert!(out.contains("# --- Laptop: only reachable through home ---"));
}

#[test]
fn a_changed_default_is_written_and_the_pcs_that_override_it_stay_overrides() {
    let mut c = cfg(COMMENTED);
    c.defaults.poll = Duration::from_secs(6);
    for pc in &mut c.pcs {
        pc.timing.poll = c.defaults.poll;
    }
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    assert!(out.contains("poll_seconds = 6          # normal polling"));
}

// ---------- adding ----------

#[test]
fn a_new_pc_is_appended_after_the_last_pc() {
    let mut c = cfg(COMMENTED);
    c.pcs.push(extra_pc(&c, "kitchen"));
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    let start = at(&out, "\n[[pcs]]\nid = \"kitchen\"");
    let end = at(&out, "\n# That is all.");
    assert!(start < end);
    // Taking the new block out gives the original back, byte for byte.
    assert_eq!(format!("{}{}", &out[..start], &out[end..]), COMMENTED);
}

#[test]
fn a_new_tunnel_goes_after_the_last_tunnel() {
    let mut c = cfg(COMMENTED);
    let mut t = c.tunnels[1].clone();
    t.id = TunnelId::parse("cabin").unwrap();
    t.name = "Cabin".into();
    c.tunnels.push(t);
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    let office = at(&out, "id = \"office\"");
    let cabin = at(&out, "id = \"cabin\"");
    let defaults = at(&out, "[defaults]");
    assert!(office < cabin && cabin < defaults, "{out}");
    let block_start = at(&out, "\n[[tunnels]]\nid = \"cabin\"");
    let block_end =
        defaults - "# ---- shared timing (defined after the tunnels on purpose) ---\n".len();
    assert!(block_start < block_end);
    // Everything that was there is still there, in the same order, byte for byte.
    assert_eq!(
        format!(
            "{}{}",
            &out[..block_start],
            &out[at(&out, "\n# ---- shared timing")..]
        ),
        COMMENTED
    );
}

#[test]
fn a_kind_with_no_entries_gets_its_first_entry_before_the_first_later_kind() {
    // Take the tunnels away, then add one back.
    let mut c = cfg(COMMENTED);
    let tunnel = c.tunnels[0].clone();
    c.tunnels.clear();
    for pc in &mut c.pcs {
        pc.tunnel = None;
    }
    let without = edit(COMMENTED, &c);
    assert!(!without.contains("[[tunnels]]"));
    c.tunnels.push(tunnel);
    let with = edit(&without, &c);
    assert_eq!(cfg(&with), c);
    assert!(at(&with, "[[tunnels]]") < at(&with, "[[relays]]"), "{with}");
}

#[test]
fn entries_are_added_to_a_file_that_has_none() {
    let minimal = include_str!("fixtures/valid/minimal.toml");
    let full = cfg(FULL);
    let mut c = cfg(minimal);
    c.tunnels = full.tunnels.clone();
    c.relays = full.relays.clone();
    c.pcs = full.pcs.clone();
    c.defaults = full.defaults;
    let out = edit(minimal, &c);
    assert_eq!(cfg(&out), c);
    assert!(at(&out, "[[tunnels]]") < at(&out, "[[relays]]"));
    assert!(at(&out, "[[relays]]") < at(&out, "[[pcs]]"));
    assert!(out.starts_with("version = 1\nlanguage = \"en\"\n"));
}

#[test]
fn a_new_entry_is_added_to_a_file_that_does_not_end_with_a_newline() {
    let text = COMMENTED.trim_end();
    let mut c = cfg(text);
    c.pcs.push(extra_pc(&c, "kitchen"));
    let out = edit(text, &c);
    assert_eq!(cfg(&out), c);
}

// ---------- deleting ----------

#[test]
fn a_deleted_entry_goes_with_its_own_comments_and_nothing_else_changes() {
    let mut c = cfg(COMMENTED);
    c.pcs.remove(1); // the laptop
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    let from = at(COMMENTED, "# --- Laptop") - 1; // the blank line above the banner
    let to = at(COMMENTED, "# --- NAS") - 1;
    assert_eq!(out, format!("{}{}", &COMMENTED[..from], &COMMENTED[to..]));
    assert!(!out.contains("Laptop"));
    assert!(
        out.contains("# How to wake it."),
        "neighbours keep their comments"
    );
}

#[test]
fn deleting_the_last_entry_keeps_the_comment_at_the_end_of_the_file() {
    let mut c = cfg(COMMENTED);
    c.pcs.pop(); // the NAS
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    assert!(!out.contains("NAS"));
    assert!(out.ends_with("\n# That is all.\n"), "{out}");
    let from = at(COMMENTED, "# --- NAS") - 1;
    let to = at(COMMENTED, "\n# That is all.");
    assert_eq!(out, format!("{}{}", &COMMENTED[..from], &COMMENTED[to..]));
}

#[test]
fn deleting_every_entry_of_a_kind_removes_its_array() {
    let mut c = cfg(COMMENTED);
    c.pcs.clear();
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    assert!(!out.contains("[[pcs]]"));
    assert!(out.contains("[[relays]]"));
}

// ---------- relays ----------

#[test]
fn changing_a_relay_preset_changes_its_kind_and_adds_the_sudo_key_it_now_needs() {
    let mut c = cfg(COMMENTED);
    c.relays[0].kind = RelayKind::Etherwake;
    c.relays[0].sudo = true;
    let out = edit(COMMENTED, &c);
    assert_eq!(
        out,
        COMMENTED
            .replace("kind = \"routeros\"", "kind = \"etherwake\"")
            .replace(
                "timeout = 8\n\n# ====",
                "timeout = 8\nsudo = true\n\n# ===="
            )
    );
}

#[test]
fn toggling_sudo_changes_only_that_value() {
    let mut c = cfg(FULL);
    c.relays[2].sudo = false; // the wakeonlan relay
    assert_eq!(edit(FULL, &c), FULL.replace("sudo = true", "sudo = false"));
}

#[test]
fn a_preset_without_sudo_loses_the_key() {
    let mut c = cfg(FULL);
    c.relays[1].kind = RelayKind::RouterOs; // the etherwake relay becomes RouterOS
    c.relays[1].sudo = false;
    assert_eq!(
        edit(FULL, &c),
        FULL.replace("kind = \"etherwake\"", "kind = \"routeros\"")
            .replace("timeout = 8\nsudo = false\n", "timeout = 8\n")
    );
}

#[test]
fn switching_a_wake_from_an_interface_to_an_address_keeps_the_comments() {
    let mut c = cfg(COMMENTED);
    c.relays[0].kind = RelayKind::Wakeonlan;
    let Some(WakeMethod::Relay {
        interface, address, ..
    }) = &mut c.pcs[0].wake
    else {
        panic!("the desktop wakes through a relay");
    };
    *interface = None;
    *address = Some("10.0.0.255".parse().unwrap());
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    assert!(!out.contains("interface = \"bridge\""));
    assert!(out.contains("address = \"10.0.0.255\""));
    assert!(out.contains("kind = \"wakeonlan\""));
    assert!(out.contains("# How to wake it."));
    assert!(out.contains("mac = \"AA:BB:CC:DD:EE:FF\"   # eth0"));
    // And back again.
    let back = edit(&out, &cfg(COMMENTED));
    assert_eq!(cfg(&back), cfg(COMMENTED));
}

#[test]
fn a_new_relay_is_appended_after_the_last_relay() {
    let mut c = cfg(COMMENTED);
    let mut relay = c.relays[0].clone();
    relay.id = RelayId::parse("pi").unwrap();
    relay.kind = RelayKind::Wakeonlan;
    c.relays.push(relay);
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    let start = at(&out, "\n[[relays]]\nid = \"pi\"");
    let end = at(&out, "\n# ============ PCs");
    assert!(start < end);
    assert_eq!(format!("{}{}", &out[..start], &out[end..]), COMMENTED);
}

#[test]
fn a_renamed_relay_with_its_references_is_still_one_entry() {
    let mut c = cfg(COMMENTED);
    let (old, new) = (
        RelayId::parse("gateway").unwrap(),
        RelayId::parse("edge").unwrap(),
    );
    c.relays[0].id = new.clone();
    if let Some(WakeMethod::Relay { relay, .. }) = &mut c.pcs[0].wake {
        *relay = new.clone();
    }
    let renames = Renames {
        relays: vec![(old, new)],
        ..Renames::default()
    };
    let out = config::edit(COMMENTED, &c, &renames).unwrap();
    assert_eq!(
        out,
        COMMENTED
            .replace("id = \"gateway\"", "id = \"edge\"")
            .replace("relay = \"gateway\"", "relay = \"edge\"")
    );
}

#[test]
fn a_magic_packet_address_written_by_hand_is_kept_and_changes_in_place() {
    let mut c = cfg(COMMENTED);
    let Some(WakeMethod::MagicPacket { address, .. }) = &mut c.pcs[1].wake else {
        panic!("the laptop wakes by magic packet");
    };
    *address = "192.168.88.11".parse().unwrap(); // unicast to the PC itself
    let out = edit(COMMENTED, &c);
    assert_eq!(
        out,
        COMMENTED.replace(
            "address = \"192.168.88.255\"      # a subnet broadcast",
            "address = \"192.168.88.11\"      # a subnet broadcast"
        )
    );
}

// ---------- renaming ----------

#[test]
fn a_listed_rename_keeps_the_entry_its_comments_and_its_place() {
    let mut c = cfg(COMMENTED);
    c.pcs[2].id = PcId::parse("storage").unwrap();
    let renames = Renames {
        pcs: vec![(PcId::parse("nas").unwrap(), PcId::parse("storage").unwrap())],
        ..Renames::default()
    };
    let out = config::edit(COMMENTED, &c, &renames).unwrap();
    assert_eq!(out, COMMENTED.replace("id = \"nas\"", "id = \"storage\""));
}

#[test]
fn a_renamed_tunnel_with_its_references_is_still_one_entry() {
    let mut c = cfg(COMMENTED);
    let (old, new) = (
        TunnelId::parse("home").unwrap(),
        TunnelId::parse("house").unwrap(),
    );
    c.tunnels[0].id = new.clone();
    for pc in &mut c.pcs {
        if pc.tunnel.as_ref() == Some(&old) {
            pc.tunnel = Some(new.clone());
        }
    }
    let renames = Renames {
        tunnels: vec![(old, new)],
        ..Renames::default()
    };
    let out = config::edit(COMMENTED, &c, &renames).unwrap();
    assert_eq!(out, COMMENTED.replace("\"home\"", "\"house\""));
}

#[test]
fn an_unlisted_rename_is_a_removal_plus_an_addition() {
    let mut c = cfg(COMMENTED);
    c.pcs[2].id = PcId::parse("storage").unwrap();
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    // The old entry's comments are gone and the new entry is last, without them.
    assert!(!out.contains("# --- NAS: always on the local network ---"));
    assert!(!out.contains("id = \"nas\""));
    assert!(at(&out, "id = \"storage\"") > at(&out, "id = \"laptop\""));
    assert!(out.contains("# --- Laptop: only reachable through home ---"));
}

#[test]
fn a_rename_that_names_a_missing_entry_is_ignored() {
    let mut c = cfg(COMMENTED);
    c.pcs[2].id = PcId::parse("storage").unwrap();
    let renames = Renames {
        pcs: vec![(
            PcId::parse("ghost").unwrap(),
            PcId::parse("storage").unwrap(),
        )],
        ..Renames::default()
    };
    let out = config::edit(COMMENTED, &c, &renames).unwrap();
    assert_eq!(cfg(&out), c);
    assert!(!out.contains("id = \"nas\""));
}

// ---------- reordering ----------

#[test]
fn reordered_entries_move_with_their_comments() {
    let mut c = cfg(COMMENTED);
    c.pcs.swap(0, 2); // nas, laptop, desktop
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);

    let d = at(COMMENTED, "# ============ PCs") - 1;
    let l = at(COMMENTED, "# --- Laptop") - 1;
    let n = at(COMMENTED, "# --- NAS") - 1;
    let t = at(COMMENTED, "\n# That is all.");
    let (desktop, laptop, nas) = (&COMMENTED[d..l], &COMMENTED[l..n], &COMMENTED[n..t]);
    let expected = format!(
        "{}{nas}{laptop}{desktop}{}",
        &COMMENTED[..d],
        &COMMENTED[t..]
    );
    assert_eq!(out, expected);
}

#[test]
fn entries_are_matched_by_id_not_by_position() {
    // Reorder, and change one value of the entry that moved.
    let mut c = cfg(COMMENTED);
    c.pcs.reverse();
    c.pcs[0].host = Host::parse("10.0.0.8").unwrap(); // the NAS, now first
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    assert!(out.contains("# --- NAS: always on the local network ---"));
    assert!(out.contains("host = \"10.0.0.8\""));
    assert!(
        out.contains("mac = \"AA:BB:CC:DD:EE:FF\"   # eth0"),
        "the desktop kept its trailing comment"
    );
}

#[test]
fn reordering_tunnels_works_too() {
    let mut c = cfg(COMMENTED);
    c.tunnels.swap(0, 1);
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    // The home entry is spelled `id   = "home"` in the file, and keeps its spelling.
    assert!(at(&out, "id = \"office\"") < at(&out, "id   = \"home\""));
    assert!(out.contains("# The tunnel to the house."));
}

#[test]
fn inserting_a_pc_in_the_middle_gives_the_order_of_the_configuration() {
    let mut c = cfg(COMMENTED);
    let pc = extra_pc(&c, "kitchen");
    c.pcs.insert(1, pc);
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
}

// ---------- per-PC timing overrides ----------

#[test]
fn a_timing_override_appears_inside_its_pc() {
    let mut c = cfg(COMMENTED);
    c.pcs[0].timing.poll = Duration::from_secs(10);
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    let timing = at(&out, "[pcs.timing]\npoll_seconds = 10");
    assert!(at(&out, "id = \"desktop\"") < timing);
    assert!(timing < at(&out, "# --- Laptop"), "{out}");
    // Taking the new table out gives the original back.
    let block = "\n[pcs.timing]\npoll_seconds = 10\n";
    assert_eq!(out.replacen(block, "", 1), COMMENTED);
}

#[test]
fn a_timing_override_that_matches_the_defaults_again_disappears() {
    let mut c = cfg(COMMENTED);
    c.pcs[1].timing.wake_timeout = c.defaults.wake_timeout;
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    let block = "\n  [pcs.timing]              # slow machine\n  wake_timeout = 300        # five minutes\n";
    assert!(COMMENTED.contains(block));
    assert_eq!(out, COMMENTED.replacen(block, "", 1));
}

#[test]
fn one_more_key_in_an_existing_override_is_added_to_it() {
    let mut c = cfg(COMMENTED);
    c.pcs[1].timing.poll = Duration::from_secs(9);
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    assert!(out.contains("wake_timeout = 300        # five minutes"));
    let table = at(&out, "[pcs.timing]");
    assert!(
        table < at(&out, "poll_seconds = 9")
            && at(&out, "poll_seconds = 9") < at(&out, "# --- NAS")
    );
}

// ---------- optional parts of a PC ----------

#[test]
fn optional_tables_and_keys_come_and_go() {
    let mut c = cfg(COMMENTED);
    c.pcs[0].rdp = None;
    c.pcs[0].power = None;
    c.pcs[2].tunnel = Some(c.tunnels[0].id.clone());
    c.pcs[2].wake = c.pcs[1].wake.clone();
    let out = edit(COMMENTED, &c);
    assert_eq!(cfg(&out), c);
    assert!(!out.contains("[pcs.rdp]\n  user = \"alex\""));
    assert!(out.contains("# How to wake it."));

    // And back again.
    let back = edit(&out, &cfg(COMMENTED));
    assert_eq!(cfg(&back), cfg(COMMENTED));
}

// ---------- fall-backs and failures ----------

#[test]
fn without_a_file_the_configuration_is_rendered_from_scratch() {
    let dir = temp_dir("fresh");
    let path = dir.join("config.toml");
    let c = cfg(FULL);
    config::save(&path, &c).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        config::render(&c).unwrap()
    );
    assert_eq!(config::load(&path).unwrap().config, c);
}

#[test]
fn an_empty_file_is_rendered_from_scratch() {
    let dir = temp_dir("empty");
    let path = dir.join("config.toml");
    std::fs::write(&path, "  \n").unwrap();
    let c = cfg(FULL);
    config::save(&path, &c).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        config::render(&c).unwrap()
    );
}

#[test]
fn a_file_that_is_not_toml_is_an_error_and_is_never_overwritten() {
    let dir = temp_dir("broken");
    let path = dir.join("config.toml");
    std::fs::write(&path, "version = \n[[").unwrap();
    let err = config::save(&path, &cfg(FULL)).unwrap_err();
    assert!(matches!(err, ConfigError::Syntax(_)), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "version = \n[[");
}

#[test]
fn a_file_with_a_byte_order_mark_keeps_it() {
    let text = format!("\u{feff}{COMMENTED}");
    let mut c = cfg(&text);
    c.pcs[0].name = "Study PC".into();
    let out = edit(&text, &c);
    assert!(out.starts_with('\u{feff}'));
    assert_eq!(cfg(&out), c);
}

#[test]
fn a_file_that_is_valid_toml_but_not_a_valid_configuration_is_repaired() {
    let text = "# my notes\nversion = 1\nunknown = true\n";
    let c = cfg(FULL);
    let out = edit(text, &c);
    assert_eq!(cfg(&out), c);
    assert!(out.starts_with("# my notes\nversion = 1\n"));
    assert!(!out.contains("unknown"));
}

#[test]
fn a_saved_file_is_written_atomically_without_leaving_a_temporary_file() {
    let dir = temp_dir("atomic");
    let path = dir.join("config.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    let mut c = cfg(COMMENTED);
    c.pcs[0].name = "Study PC".into();
    config::save(&path, &c).unwrap();
    assert!(!dir.join("config.toml.tmp").exists());
    assert_eq!(config::load(&path).unwrap().config, c);
}

// ---------- edit in place == render, on many random edits ----------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// One random, valid change to the configuration. Returns the renames it implies.
fn mutate(rng: &mut Rng, c: &mut Config, counter: &mut u32) -> Renames {
    let mut renames = Renames::default();
    *counter += 1;
    match rng.below(14) {
        0 if !c.pcs.is_empty() => {
            let i = rng.below(c.pcs.len());
            c.pcs[i].host =
                Host::parse(&format!("10.1.{}.{}", *counter % 250, rng.below(250))).unwrap();
        }
        1 if !c.pcs.is_empty() => {
            let i = rng.below(c.pcs.len());
            c.pcs[i].ports = vec![20 + (*counter % 1000) as u16, 3389];
        }
        2 if !c.pcs.is_empty() => {
            let i = rng.below(c.pcs.len());
            c.pcs[i].timing.poll = Duration::from_secs(5 + rng.below(4) as u64 * 5);
        }
        3 if !c.pcs.is_empty() => {
            let i = rng.below(c.pcs.len());
            c.pcs[i].timing = c.defaults;
        }
        4 if !c.pcs.is_empty() => {
            let (i, j) = (rng.below(c.pcs.len()), rng.below(c.pcs.len()));
            let (wake, power, rdp) = (
                c.pcs[j].wake.clone(),
                c.pcs[j].power.clone(),
                c.pcs[j].rdp.clone(),
            );
            c.pcs[i].wake = wake;
            c.pcs[i].power = power;
            c.pcs[i].rdp = rdp;
        }
        5 if !c.pcs.is_empty() => {
            let i = rng.below(c.pcs.len());
            c.pcs[i].wake = None;
            c.pcs[i].rdp = None;
        }
        6 if c.pcs.len() > 1 => {
            let (i, j) = (rng.below(c.pcs.len()), rng.below(c.pcs.len()));
            c.pcs.swap(i, j);
        }
        7 if !c.pcs.is_empty() => {
            let i = rng.below(c.pcs.len());
            c.pcs.remove(i);
        }
        8 if !c.pcs.is_empty() => {
            let mut pc = c.pcs[rng.below(c.pcs.len())].clone();
            pc.id = PcId::parse(&format!("pc-{counter}")).unwrap();
            pc.name = format!("PC {counter}");
            let at = rng.below(c.pcs.len() + 1);
            c.pcs.insert(at, pc);
        }
        9 if !c.tunnels.is_empty() => {
            let mut t = c.tunnels[rng.below(c.tunnels.len())].clone();
            t.id = TunnelId::parse(&format!("tun-{counter}")).unwrap();
            t.name = format!("Tunnel {counter}");
            let at = rng.below(c.tunnels.len() + 1);
            c.tunnels.insert(at, t);
        }
        10 if !c.tunnels.is_empty() => {
            let i = rng.below(c.tunnels.len());
            let gone = c.tunnels.remove(i).id;
            for pc in &mut c.pcs {
                if pc.tunnel.as_ref() == Some(&gone) {
                    pc.tunnel = None;
                }
            }
        }
        11 if c.tunnels.len() > 1 => {
            let (i, j) = (rng.below(c.tunnels.len()), rng.below(c.tunnels.len()));
            c.tunnels.swap(i, j);
        }
        12 if !c.pcs.is_empty() => {
            let i = rng.below(c.pcs.len());
            let old = c.pcs[i].id.clone();
            let new = PcId::parse(&format!("renamed-{counter}")).unwrap();
            c.pcs[i].id = new.clone();
            if rng.below(2) == 0 {
                renames.pcs.push((old, new));
            }
        }
        _ if !c.pcs.is_empty() && !c.tunnels.is_empty() => {
            let i = rng.below(c.pcs.len());
            c.pcs[i].tunnel = if rng.below(3) == 0 {
                None
            } else {
                Some(c.tunnels[rng.below(c.tunnels.len())].id.clone())
            };
        }
        _ => {}
    }
    renames
}

#[test]
fn editing_in_place_and_rendering_from_scratch_describe_the_same_configuration() {
    for (name, start) in [("commented", COMMENTED), ("full", FULL)] {
        for seed in 1..=12u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut c = cfg(start);
            let mut text = start.to_owned();
            let mut counter = 0u32;
            for step in 0..25 {
                let renames = mutate(&mut rng, &mut c, &mut counter);
                let context = format!("{name} seed {seed} step {step}");
                let edited = config::edit(&text, &c, &renames)
                    .unwrap_or_else(|e| panic!("{context}: {e}\n{text}"));
                let fresh = config::render(&c).unwrap_or_else(|e| panic!("{context}: render: {e}"));
                assert_eq!(cfg(&edited), cfg(&fresh), "{context}");
                assert_eq!(cfg(&edited), c, "{context}");
                // Editing again with no change is stable.
                assert_eq!(edit(&edited, &c), edited, "{context}: not idempotent");
                text = edited;
            }
        }
    }
}

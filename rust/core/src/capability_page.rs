//! The "Можливості системи" page of the settings window (artboard `Capabilities`): one tile per
//! feature with the programs it needs, where they were found, or a neutral "not found" with a
//! link that explains how to install them. Missing is not failure: the page uses no red.

use crate::capabilities::{Capabilities, Feature, Tool};
use crate::i18n::Translator;

/// Where to read how to install the WireGuard client.
pub const WIREGUARD_URL: &str = "https://www.wireguard.com/install/";
/// Where to read how to turn on the OpenSSH client of Windows.
pub const OPENSSH_URL: &str =
    "https://learn.microsoft.com/windows-server/administration/openssh/openssh_install_firstuse";

/// A link of a tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The link text.
    pub label: String,
    /// The address.
    pub url: &'static str,
}

/// One tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    /// The feature's name.
    pub title: String,
    /// Every program it needs was found (or it needs none).
    pub found: bool,
    /// The programs, or "Не знайдено …".
    pub line: String,
    /// Where they were found (one folder, or a path per program in different folders), or a
    /// note; empty when missing.
    pub details: Vec<String>,
    /// How to get what is missing; `None` when nothing is missing or there is no page for it.
    pub link: Option<Link>,
}

/// The programs a tile names when everything is found (`sc` is part of Windows and is named
/// only when it is missing).
fn shown(feature: Feature) -> &'static [Tool] {
    match feature {
        Feature::TunnelControl => &[Tool::WireGuard],
        other => other.tools(),
    }
}

/// The tiles in page order.
pub fn tiles(tr: &Translator, caps: &Capabilities) -> Vec<Tile> {
    Feature::ALL
        .into_iter()
        .map(|feature| tile(tr, caps, feature))
        .collect()
}

fn names(tr: &Translator, tools: &[Tool]) -> String {
    tools
        .iter()
        .map(|t| tr.t(&format!("tool.{}", t.code()), &[]))
        .collect::<Vec<_>>()
        .join(", ")
}

fn tile(tr: &Translator, caps: &Capabilities, feature: Feature) -> Tile {
    let title = tr.t(&format!("capability.{}", feature.code()), &[]);
    if feature == Feature::MagicPacket {
        return Tile {
            title,
            found: true,
            line: tr.t("capability.built_in", &[]),
            details: vec![tr.t("capability.no_tool", &[])],
            link: None,
        };
    }
    let missing = caps.missing_for(feature);
    if !missing.is_empty() {
        let link = if missing.contains(&Tool::WireGuard) {
            Some(Link {
                label: tr.t("capability.how_install", &[]),
                url: WIREGUARD_URL,
            })
        } else if missing
            .iter()
            .any(|t| matches!(t, Tool::Ssh | Tool::SshKeygen | Tool::SshKeyscan))
        {
            Some(Link {
                label: tr.t("capability.how_enable", &[]),
                url: OPENSSH_URL,
            })
        } else {
            None
        };
        return Tile {
            title,
            found: false,
            line: not_found_line(tr, &missing),
            details: Vec::new(),
            link,
        };
    }
    let tools = shown(feature);
    let paths: Vec<&std::path::Path> = tools
        .iter()
        .filter_map(|t| caps.status(*t).path())
        .collect();
    let details = match paths.as_slice() {
        [one] => vec![one.display().to_string()],
        [first, rest @ ..] => {
            // Several programs: their folder when they share one, else a path each.
            let dir = first.parent();
            if rest.iter().all(|p| p.parent() == dir) {
                dir.map(|d| vec![format!("{}{}", d.display(), std::path::MAIN_SEPARATOR)])
                    .unwrap_or_default()
            } else {
                paths.iter().map(|p| p.display().to_string()).collect()
            }
        }
        [] => Vec::new(),
    };
    // A client is named in words ("Клієнт WireGuard") and starts a line with a capital; a
    // program is named as it is spelt (`ping`, `ssh-keygen`).
    let line = names(tr, tools);
    let line = if matches!(tools.first(), Some(Tool::WireGuard | Tool::Ssh)) {
        capitalise(&line)
    } else {
        line
    };
    Tile {
        title,
        found: true,
        line,
        details,
        link: None,
    }
}

/// "Не знайдено …" / "… not found": a capital where the sentence starts with a word of the
/// template or with a client's name, never on a program name (`wg`, `mstsc`).
fn not_found_line(tr: &Translator, missing: &[Tool]) -> String {
    let line = tr.t("capability.not_found", &[("what", &names(tr, missing))]);
    let template_first = !tr
        .t("capability.not_found", &[("what", "")])
        .starts_with(|c: char| c.is_whitespace());
    if template_first || matches!(missing.first(), Some(Tool::WireGuard | Tool::Ssh)) {
        capitalise(&line)
    } else {
        line
    }
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::ToolStatus;
    use crate::i18n::Lang;
    use std::path::PathBuf;

    fn found(tool: Tool, path: &str) -> (Tool, ToolStatus) {
        (tool, ToolStatus::Found(PathBuf::from(path)))
    }

    #[test]
    fn found_tiles_name_the_programs_and_their_place() {
        let uk = Translator::new(Lang::Uk);
        let caps = Capabilities::from_statuses([
            found(Tool::WireGuard, "/wg/wireguard.exe"),
            found(Tool::ServiceControl, "/sys/sc.exe"),
            found(Tool::SshKeygen, "/ssh/ssh-keygen.exe"),
            found(Tool::SshKeyscan, "/ssh/ssh-keyscan.exe"),
        ]);
        let tiles = tiles(&uk, &caps);
        assert_eq!(tiles.len(), 8);
        let tunnels = &tiles[0];
        assert_eq!(tunnels.title, "Тунелі");
        assert!(tunnels.found);
        assert_eq!(tunnels.line, "Клієнт WireGuard");
        assert_eq!(tunnels.details, ["/wg/wireguard.exe"]);
        let keys = &tiles[3];
        assert!(keys.found);
        assert_eq!(keys.line, "ssh-keygen, ssh-keyscan");
        assert_eq!(keys.details, [format!("/ssh{}", std::path::MAIN_SEPARATOR)]);
        let magic = &tiles[5];
        assert!(magic.found && magic.link.is_none());
    }

    #[test]
    fn missing_tiles_say_so_neutrally_with_a_link_where_there_is_one() {
        let uk = Translator::new(Lang::Uk);
        let caps = Capabilities::from_statuses([]);
        let tiles = tiles(&uk, &caps);
        let tunnels = &tiles[0];
        assert!(!tunnels.found);
        assert_eq!(tunnels.line, "Не знайдено клієнт WireGuard, sc");
        assert_eq!(tunnels.link.as_ref().map(|l| l.url), Some(WIREGUARD_URL));
        let ssh = &tiles[2];
        assert_eq!(ssh.line, "Не знайдено клієнт OpenSSH");
        assert_eq!(ssh.link.as_ref().map(|l| l.url), Some(OPENSSH_URL));
        assert!(tiles[4].link.is_none(), "mstsc is part of Windows: no link");
        let en = Translator::new(Lang::En);
        let en_tiles = super::tiles(&en, &caps);
        assert_eq!(
            en_tiles[4].line, "mstsc not found",
            "a program keeps its spelling"
        );
        assert_eq!(en_tiles[0].line, "WireGuard client, sc not found");
    }

    #[test]
    fn every_tile_has_its_texts_in_both_languages() {
        for lang in Lang::ALL {
            let tr = Translator::new(lang);
            for tile in tiles(&tr, &Capabilities::from_statuses([])) {
                assert!(!tile.title.starts_with("capability."), "{}", tile.title);
                assert!(!tile.line.contains("capability."), "{}", tile.line);
            }
            for tile in tiles(&tr, &Capabilities::all_found()) {
                assert!(
                    tile.details.iter().all(|d| !d.contains("capability.")),
                    "{:?}",
                    tile.details
                );
            }
        }
    }
}

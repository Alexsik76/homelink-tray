//! The text of a tray balloon, built to the limits of the Windows notification API
//! (`NOTIFYICONDATAW`): the title is at most 63 UTF-16 units and the body at most 255 (the
//! fields hold 64 and 256 including the terminating zero). All pure; the Win32 call that shows
//! it is in `platform.rs`.
//!
//! The balloon is shown for a [`Notice`](homelink_core::notice::Notice) of the engine, with its
//! title and body from the catalog, so the rules of section 9 of `docs/rust-architecture.md`
//! about which notices exist and when stay in the engine: whatever it reports is shown, and what
//! it keeps silent about (a user disconnect, a PC that goes off after an accepted power command,
//! a PC that becomes unknown because its tunnel dropped) never reaches this module.

// The units and the icon are read by the Windows layer only.
#![cfg_attr(not(windows), allow(dead_code))]

use homelink_core::notice::Severity;

/// The longest title, in UTF-16 units (`szInfoTitle` has 64 including the terminating zero).
pub(crate) const TITLE_UNITS: usize = 63;
/// The longest body, in UTF-16 units (`szInfo` has 256 including the terminating zero).
pub(crate) const BODY_UNITS: usize = 255;

/// The icon of a balloon (`NIIF_INFO`, `NIIF_WARNING`, `NIIF_ERROR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Info,
    Warning,
    Error,
}

/// The icon for how serious a notice is.
pub(crate) fn icon_for(severity: Severity) -> Icon {
    match severity {
        Severity::Info => Icon::Info,
        Severity::Warning => Icon::Warning,
        Severity::Error => Icon::Error,
    }
}

/// `text` as UTF-16 of at most `limit` units. A text that does not fit is cut and ends with `…`
/// (one unit); the cut never falls inside a surrogate pair. A NUL, which would end the string
/// early, becomes a space.
pub(crate) fn truncate_utf16(text: &str, limit: usize) -> Vec<u16> {
    let clean: Vec<u16> = text
        .chars()
        .map(|c| if c == '\0' { ' ' } else { c })
        .collect::<String>()
        .encode_utf16()
        .collect();
    if clean.len() <= limit {
        return clean;
    }
    if limit == 0 {
        return Vec::new();
    }
    let mut cut = limit.saturating_sub(1);
    // A high surrogate at the end of the kept part would lose its low half.
    if cut > 0
        && clean
            .get(cut - 1)
            .is_some_and(|unit| (0xD800..0xDC00).contains(unit))
    {
        cut -= 1;
    }
    let mut kept: Vec<u16> = clean.get(..cut).unwrap_or_default().to_vec();
    kept.push(0x2026);
    kept
}

/// `units` in a fixed-size array, padded with zeros (cut if longer: callers pass what
/// [`truncate_utf16`] gave, which always leaves room for the terminating zero).
pub(crate) fn fill<const N: usize>(units: &[u16]) -> [u16; N] {
    let mut buffer = [0u16; N];
    let count = units.len().min(N.saturating_sub(1));
    if let (Some(target), Some(source)) = (buffer.get_mut(..count), units.get(..count)) {
        target.copy_from_slice(source);
    }
    buffer
}

/// A balloon ready to hand to the Windows API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Balloon {
    /// The title, within [`TITLE_UNITS`].
    pub(crate) title: Vec<u16>,
    /// The body, within [`BODY_UNITS`]; never empty (Windows shows no balloon without a body).
    pub(crate) body: Vec<u16>,
    /// The icon.
    pub(crate) icon: Icon,
}

impl Balloon {
    /// The balloon for a notice of `severity` with this title and body.
    pub(crate) fn new(title: &str, body: &str, severity: Severity) -> Self {
        let body_text = if body.trim().is_empty() { title } else { body };
        Self {
            title: truncate_utf16(title, TITLE_UNITS),
            body: truncate_utf16(body_text, BODY_UNITS),
            icon: icon_for(severity),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(units: &[u16]) -> String {
        String::from_utf16(units).unwrap()
    }

    #[test]
    fn a_short_text_is_kept_whole() {
        assert_eq!(
            text(&truncate_utf16("Tunnel office is off", 63)),
            "Tunnel office is off"
        );
        assert!(truncate_utf16("", 63).is_empty());
        let exactly = "a".repeat(63);
        assert_eq!(text(&truncate_utf16(&exactly, 63)), exactly);
    }

    #[test]
    fn a_long_text_is_cut_to_the_limit_with_an_ellipsis() {
        let long = "a".repeat(300);
        let cut = truncate_utf16(&long, 255);
        assert_eq!(cut.len(), 255);
        assert_eq!(cut.last(), Some(&0x2026));
        assert!(text(&cut).starts_with("aaaa"));
        let cut = truncate_utf16(&"b".repeat(64), 63);
        assert_eq!(cut.len(), 63);
    }

    #[test]
    fn the_limit_counts_utf16_units_not_characters() {
        // Ukrainian letters are one unit each.
        let uk = "є".repeat(100);
        assert_eq!(truncate_utf16(&uk, 63).len(), 63);
        // An emoji is two units: 40 of them are 80 units.
        let emoji = "😀".repeat(40);
        let cut = truncate_utf16(&emoji, 63);
        assert!(cut.len() <= 63, "{}", cut.len());
        // The cut never leaves half of a pair: the result is valid UTF-16.
        assert!(String::from_utf16(&cut).is_ok());
        assert_eq!(cut.last(), Some(&0x2026));
    }

    #[test]
    fn a_cut_next_to_a_pair_does_not_split_it() {
        // 61 letters, then an emoji (2 units) crossing the limit of 63 with room for the dots.
        let s = format!("{}😀tail", "a".repeat(61));
        let cut = truncate_utf16(&s, 63);
        assert!(String::from_utf16(&cut).is_ok());
        assert_eq!(cut.len(), 62);
        assert_eq!(cut.last(), Some(&0x2026));
        // And with a limit that falls right after the pair's first half.
        let s = format!("{}😀tail", "a".repeat(62));
        let cut = truncate_utf16(&s, 64);
        assert!(String::from_utf16(&cut).is_ok(), "{cut:?}");
        assert!(cut.len() <= 64);
    }

    #[test]
    fn a_nul_does_not_end_the_text() {
        assert_eq!(text(&truncate_utf16("a\0b", 63)), "a b");
    }

    #[test]
    fn a_tiny_limit_is_handled() {
        assert_eq!(truncate_utf16("abc", 1), [0x2026]);
        assert!(truncate_utf16("abc", 0).is_empty());
    }

    #[test]
    fn the_icon_follows_the_severity() {
        assert_eq!(icon_for(Severity::Info), Icon::Info);
        assert_eq!(icon_for(Severity::Warning), Icon::Warning);
        assert_eq!(icon_for(Severity::Error), Icon::Error);
    }

    #[test]
    fn a_balloon_has_both_texts_within_the_limits() {
        let long_detail = "є".repeat(200);
        let b = Balloon::new(
            &"Не вдалося підняти тунель ".repeat(5),
            &format!("Причина: {long_detail} {}", "x".repeat(100)),
            Severity::Error,
        );
        assert!(b.title.len() <= TITLE_UNITS);
        assert!(b.body.len() <= BODY_UNITS);
        assert_eq!(b.icon, Icon::Error);
        // An empty body is replaced: Windows shows nothing without one.
        let b = Balloon::new("Title", "  ", Severity::Info);
        assert_eq!(text(&b.body), "Title");
    }

    #[test]
    fn the_fixed_buffers_always_keep_a_terminating_zero() {
        let units = truncate_utf16(&"a".repeat(500), BODY_UNITS);
        let buffer: [u16; 256] = fill(&units);
        assert_eq!(buffer[255], 0);
        assert_eq!(buffer[254], 0x2026);
        let title: [u16; 64] = fill(&truncate_utf16("Hi", TITLE_UNITS));
        assert_eq!(&title[..3], &[u16::from(b'H'), u16::from(b'i'), 0]);
        // Longer input than fits is cut, never overflowing.
        let long = vec![7u16; 400];
        let buffer: [u16; 64] = fill(&long);
        assert_eq!(buffer[62], 7);
        assert_eq!(buffer[63], 0);
    }
}

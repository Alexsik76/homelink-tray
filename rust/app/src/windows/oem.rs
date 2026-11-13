//! Console programs on Windows write in the OEM code page of the system, not in UTF-8
//! (`ping`, `sc`, `wireguard.exe` and the error texts of `ssh` all do). This module finds
//! the code page and decodes bytes written in it, so that texts shown to the user (and
//! matched by the adapters) are right in every display language.
//!
//! The code page comes from the registry (`OEMCP`, read with `reg query`). The tables cover
//! the single-byte code pages 437 (United States), 850 (Western Europe) and 866 (Cyrillic,
//! including Ukrainian). For any other code page (including the double-byte ones of East
//! Asian systems) ASCII is decoded exactly and every other byte becomes U+FFFD, which keeps
//! all matching on ASCII markers (such as `TTL=` or `RUNNING`) working.
//!
//! The tables list Unicode code points, as in the mapping files of the Unicode Consortium
//! (`CP437.TXT`, `CP850.TXT`, `CP866.TXT`): bytes 0x80..=0xAF, 0xB0..=0xDF and 0xE0..=0xFF.

use std::sync::Arc;

use homelink_core::adapters::command::Decoder;
use homelink_core::ports::CommandRunner;

use super::reg;

/// The registry key and value of the OEM code page.
const KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Nls\CodePage";
const VALUE: &str = "OEMCP";

/// Box drawing characters, bytes 0xB0..=0xDF of code pages 437 and 866.
#[rustfmt::skip]
const BOX: [u16; 48] = [
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556,
    0x2555, 0x2563, 0x2551, 0x2557, 0x255D, 0x255C, 0x255B, 0x2510,
    0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567,
    0x2568, 0x2564, 0x2565, 0x2559, 0x2558, 0x2552, 0x2553, 0x256B,
    0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
];

/// Code page 437, bytes 0x80..=0xAF.
#[rustfmt::skip]
const CP437_LOW: [u16; 48] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7,
    0x00EA, 0x00EB, 0x00E8, 0x00EF, 0x00EE, 0x00EC, 0x00C4, 0x00C5,
    0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9,
    0x00FF, 0x00D6, 0x00DC, 0x00A2, 0x00A3, 0x00A5, 0x20A7, 0x0192,
    0x00E1, 0x00ED, 0x00F3, 0x00FA, 0x00F1, 0x00D1, 0x00AA, 0x00BA,
    0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
];

/// Code page 437, bytes 0xE0..=0xFF.
#[rustfmt::skip]
const CP437_HIGH: [u16; 32] = [
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4,
    0x03A6, 0x0398, 0x03A9, 0x03B4, 0x221E, 0x03C6, 0x03B5, 0x2229,
    0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
];

/// Code page 850, bytes 0x80..=0xAF.
#[rustfmt::skip]
const CP850_LOW: [u16; 48] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7,
    0x00EA, 0x00EB, 0x00E8, 0x00EF, 0x00EE, 0x00EC, 0x00C4, 0x00C5,
    0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9,
    0x00FF, 0x00D6, 0x00DC, 0x00F8, 0x00A3, 0x00D8, 0x00D7, 0x0192,
    0x00E1, 0x00ED, 0x00F3, 0x00FA, 0x00F1, 0x00D1, 0x00AA, 0x00BA,
    0x00BF, 0x00AE, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
];

/// Code page 850, bytes 0xB0..=0xDF.
#[rustfmt::skip]
const CP850_MID: [u16; 48] = [
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x00C1, 0x00C2, 0x00C0,
    0x00A9, 0x2563, 0x2551, 0x2557, 0x255D, 0x00A2, 0x00A5, 0x2510,
    0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x00E3, 0x00C3,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x00A4,
    0x00F0, 0x00D0, 0x00CA, 0x00CB, 0x00C8, 0x0131, 0x00CD, 0x00CE,
    0x00CF, 0x2518, 0x250C, 0x2588, 0x2584, 0x00A6, 0x00CC, 0x2580,
];

/// Code page 850, bytes 0xE0..=0xFF.
#[rustfmt::skip]
const CP850_HIGH: [u16; 32] = [
    0x00D3, 0x00DF, 0x00D4, 0x00D2, 0x00F5, 0x00D5, 0x00B5, 0x00FE,
    0x00DE, 0x00DA, 0x00DB, 0x00D9, 0x00FD, 0x00DD, 0x00AF, 0x00B4,
    0x00AD, 0x00B1, 0x2017, 0x00BE, 0x00B6, 0x00A7, 0x00F7, 0x00B8,
    0x00B0, 0x00A8, 0x00B7, 0x00B9, 0x00B3, 0x00B2, 0x25A0, 0x00A0,
];

/// Code page 866, bytes 0xF0..=0xFF (the letters before them are consecutive).
#[rustfmt::skip]
const CP866_TAIL: [u16; 16] = [
    0x0401, 0x0451, 0x0404, 0x0454, 0x0407, 0x0457, 0x040E, 0x045E,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x2116, 0x00A4, 0x25A0, 0x00A0,
];

/// The upper half (bytes 0x80..=0xFF) of a supported code page, as code points.
fn upper_half(code_page: u16) -> Option<Vec<char>> {
    let points: Vec<u16> = match code_page {
        437 => [CP437_LOW.as_slice(), BOX.as_slice(), CP437_HIGH.as_slice()].concat(),
        850 => [
            CP850_LOW.as_slice(),
            CP850_MID.as_slice(),
            CP850_HIGH.as_slice(),
        ]
        .concat(),
        866 => {
            // А-п are 0x80..=0xAF and р-я 0xE0..=0xEF: consecutive Unicode letters.
            let capitals_and_first_small = 0x0410..=0x043F;
            let small = 0x0440..=0x044F;
            capitals_and_first_small
                .chain(BOX)
                .chain(small)
                .chain(CP866_TAIL)
                .collect()
        }
        _ => return None,
    };
    let table: Vec<char> = points
        .into_iter()
        .filter_map(|p| char::from_u32(u32::from(p)))
        .collect();
    (table.len() == 128).then_some(table)
}

/// Decodes `bytes` written in `code_page` (`None` or an unsupported page: ASCII only). The
/// program itself uses [`decoder`], which builds the table once; tests use this.
#[cfg(test)]
pub(crate) fn decode(code_page: Option<u16>, bytes: &[u8]) -> String {
    decode_with(code_page.and_then(upper_half).as_deref(), bytes)
}

fn decode_with(upper: Option<&[char]>, bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                char::from(b)
            } else {
                upper
                    .and_then(|table| table.get(usize::from(b - 0x80)))
                    .copied()
                    .unwrap_or(char::REPLACEMENT_CHARACTER)
            }
        })
        .collect()
}

/// A decoder for the command runner. The table is built once.
pub(crate) fn decoder(code_page: Option<u16>) -> Decoder {
    let upper = code_page.and_then(upper_half);
    Arc::new(move |bytes| decode_with(upper.as_deref(), bytes))
}

/// Reads the OEM code page of this system (the `OEMCP` value, for example `866`).
pub(crate) fn detect(runner: &dyn CommandRunner) -> Option<u16> {
    let value = reg::query(runner, KEY, VALUE)?;
    value.data.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use homelink_core::ports::CommandOutput;
    use homelink_core::testing::FakeCommandRunner;

    /// The bytes of `text` in code page 866, by the layout of that page (Unicode letters
    /// А-п map to 0x80-0xAF and р-я to 0xE0-0xEF), independent of the decoding table.
    fn encode_866(text: &str) -> Vec<u8> {
        text.chars()
            .map(|c| match c {
                'А'..='п' => 0x80 + u8::try_from(c as u32 - 'А' as u32).unwrap(),
                'р'..='я' => 0xE0 + u8::try_from(c as u32 - 'р' as u32).unwrap(),
                'Ё' => 0xF0,
                'ё' => 0xF1,
                c if c.is_ascii() => u8::try_from(c as u32).unwrap(),
                other => panic!("not encodable in this test: {other}"),
            })
            .collect()
    }

    fn all_bytes() -> Vec<u8> {
        (0x80..=0xFF).collect()
    }

    #[test]
    fn every_supported_table_has_128_entries() {
        for page in [437, 850, 866] {
            assert_eq!(upper_half(page).map(|t| t.len()), Some(128), "page {page}");
        }
        assert!(upper_half(1251).is_none());
        assert!(upper_half(932).is_none());
    }

    #[test]
    fn a_table_has_no_duplicate_characters() {
        for page in [437, 850, 866] {
            let mut chars = upper_half(page).unwrap();
            chars.sort_unstable();
            chars.dedup();
            // Each byte of these pages has a character of its own.
            assert_eq!(chars.len(), 128, "page {page}");
        }
    }

    #[test]
    fn ascii_is_decoded_everywhere() {
        let text = "Reply from 10.0.0.1: TTL=64\r\n";
        for page in [Some(437), Some(850), Some(866), Some(932), None] {
            assert_eq!(decode(page, text.as_bytes()), text);
        }
    }

    #[test]
    fn cp866_decodes_cyrillic_from_known_bytes() {
        // "СОСТОЯНИЕ" as the console of a Russian or Ukrainian Windows writes it.
        let bytes = [0x91, 0x8E, 0x91, 0x92, 0x8E, 0x9F, 0x8D, 0x88, 0x85];
        assert_eq!(decode(Some(866), &bytes), "СОСТОЯНИЕ");
        assert_eq!(decode(Some(866), &[0xA0, 0xAF, 0xE0, 0xEF]), "апря");
        assert_eq!(
            decode(Some(866), &[0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5]),
            "ЁёЄєЇї"
        );
        assert_eq!(decode(Some(866), &[0xFC, 0xB0, 0xDB, 0xC4]), "№░█─");
    }

    #[test]
    fn cp866_agrees_with_the_layout_of_the_page_for_every_letter() {
        let letters = "АБВГДЕЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдежзийклмнопрстуфхцчшщъыьэюяЁё";
        assert_eq!(decode(Some(866), &encode_866(letters)), letters);
    }

    #[test]
    fn cp437_decodes_known_bytes() {
        let decoded: Vec<u32> = decode(Some(437), &[0x82, 0x81, 0x9B, 0xE1, 0xE6, 0xFB, 0xFA])
            .chars()
            .map(u32::from)
            .collect();
        // é ü ¢ ß µ √ ·
        assert_eq!(decoded, [0xE9, 0xFC, 0xA2, 0xDF, 0xB5, 0x221A, 0xB7]);
        assert_eq!(decode(Some(437), &[0xC9, 0xCD, 0xBB, 0xB3]), "╔═╗│");
    }

    #[test]
    fn cp850_decodes_known_bytes() {
        let decoded: Vec<u32> = decode(Some(850), &[0x82, 0x9B, 0x9D, 0xB5, 0xE1, 0xD0, 0xD5])
            .chars()
            .map(u32::from)
            .collect();
        // é ø Ø Á ß ð ı
        assert_eq!(decoded, [0xE9, 0xF8, 0xD8, 0xC1, 0xDF, 0xF0, 0x131]);
        assert_eq!(decode(Some(850), &[0xC9, 0xCD, 0xBB]), "╔═╗");
    }

    #[test]
    fn the_pages_differ_where_they_should() {
        let bytes = all_bytes();
        let p437 = decode(Some(437), &bytes);
        let p850 = decode(Some(850), &bytes);
        let p866 = decode(Some(866), &bytes);
        assert_ne!(p437, p850);
        assert_ne!(p437, p866);
        // The first 32 bytes are Latin letters in 437 and 850, Cyrillic in 866.
        assert_eq!(
            p437.chars().take(16).collect::<String>(),
            "ÇüéâäàåçêëèïîìÄÅ"
        );
        assert_eq!(p866.chars().take(4).collect::<String>(), "АБВГ");
        // Both Latin pages share the box drawing corner at 0xBB, 866 as well.
        assert_eq!(decode(Some(866), &[0xBB]), decode(Some(437), &[0xBB]));
    }

    #[test]
    fn an_unsupported_page_keeps_ascii_and_marks_the_rest() {
        assert_eq!(decode(Some(932), b"a\x83b"), "a\u{FFFD}b");
        assert_eq!(decode(None, &[0xE9]), "\u{FFFD}");
    }

    #[test]
    fn the_decoder_works_like_decode() {
        let decode_866 = decoder(Some(866));
        assert_eq!(decode_866(&[0x8D, 0x88, 0x85]), "НИЕ");
        let ascii_only = decoder(None);
        assert_eq!(ascii_only(b"ok \xFF"), "ok \u{FFFD}");
    }

    #[test]
    fn the_code_page_is_read_from_reg_output() {
        let runner = FakeCommandRunner::default();
        let text = "\r\nHKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\Nls\\CodePage\r\n    OEMCP    REG_SZ    866\r\n";
        runner.set_result(CommandOutput::exited(0, text));
        assert_eq!(detect(&runner), Some(866));
        runner.set_result(CommandOutput::exited(0, "    OEMCP    REG_SZ    ???\n"));
        assert_eq!(detect(&runner), None);
        runner.set_result(CommandOutput::exited(0, ""));
        assert_eq!(detect(&runner), None);
    }

    #[test]
    fn detection_runs_reg_query_for_the_oemcp_value() {
        let runner = FakeCommandRunner::default();
        runner.set_result(CommandOutput::exited(0, "    OEMCP    REG_SZ    437\n"));
        assert_eq!(detect(&runner), Some(437));
        let calls = runner.calls();
        let (argv, _) = calls.first().unwrap();
        assert_eq!(argv.first().map(String::as_str), Some("reg"));
        assert!(argv.iter().any(|a| a == "OEMCP"));
        runner.set_result(CommandOutput::not_started("reg: not found"));
        assert_eq!(detect(&runner), None);
    }
}

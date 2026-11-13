//! Ids derived from names. The user never types an id: it is a slug of the name (Latin
//! letters, digits and `-`; Ukrainian and Russian letters are transliterated), made unique
//! among the ids of its kind.

use crate::domain::ID_MAX_LEN;

/// The Latin spelling of a Cyrillic letter (Ukrainian national system of 2010, plus the
/// Russian letters that Ukrainian lacks); `None` for other characters.
fn transliterate(c: char) -> Option<&'static str> {
    Some(match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "h",
        'ґ' => "g",
        'д' => "d",
        'е' | 'э' | 'ё' => "e",
        'є' => "ie",
        'ж' => "zh",
        'з' => "z",
        'и' | 'ы' => "y",
        'і' | 'ї' | 'й' => "i",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ю' => "iu",
        'я' => "ia",
        // Soft and hard signs and apostrophes are dropped.
        'ь' | 'ъ' | '\'' | '’' | 'ʼ' => "",
        _ => return None,
    })
}

/// A slug of `text`: lower-case ASCII letters and digits, other characters become `-`,
/// runs of `-` collapse, no leading or trailing `-`, at most [`ID_MAX_LEN`] characters.
/// Empty when nothing usable is left.
pub(crate) fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if let Some(latin) = transliterate(c) {
            out.push_str(latin);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    cut(out.trim_matches('-'), ID_MAX_LEN)
}

/// `text` cut to `max` characters without a trailing `-`.
fn cut(text: &str, max: usize) -> String {
    let short: String = text.chars().take(max).collect();
    short.trim_end_matches('-').to_owned()
}

/// An id for `text` that `taken` does not report: the slug, or `fallback` when the slug is
/// empty, then with `-2`, `-3`, ... appended until it is free.
pub(crate) fn unique(text: &str, fallback: &str, taken: impl Fn(&str) -> bool) -> String {
    let slug = slug(text);
    let base = if slug.is_empty() { fallback } else { &slug };
    if !taken(base) {
        return base.to_owned();
    }
    (2u32..)
        .map(|n| {
            let suffix = format!("-{n}");
            let room = ID_MAX_LEN.saturating_sub(suffix.len());
            format!("{}{suffix}", cut(base, room))
        })
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PcId;

    #[test]
    fn latin_names() {
        assert_eq!(slug("Home Office"), "home-office");
        assert_eq!(slug("  NAS #2 "), "nas-2");
        assert_eq!(slug("a--b__c"), "a-b-c");
        assert_eq!(slug("---"), "");
    }

    #[test]
    fn ukrainian_names_are_transliterated() {
        assert_eq!(slug("Дім"), "dim");
        assert_eq!(slug("Робочий ПК"), "robochyi-pk");
        assert_eq!(slug("Їжак"), "izhak");
        assert_eq!(slug("Об'єкт"), "obiekt");
        assert_eq!(slug("Щука"), "shchuka");
    }

    #[test]
    fn a_slug_fits_the_id_rules() {
        for name in [
            "Home",
            "Дім",
            &"x".repeat(80),
            &format!("{}-{}", "a".repeat(31), "b"),
        ] {
            let id = slug(name);
            assert!(id.len() <= ID_MAX_LEN, "{id}");
            assert!(id.is_empty() || PcId::parse(&id).is_ok(), "{id}");
            assert!(!id.ends_with('-'), "{id}");
        }
    }

    #[test]
    fn unique_ids() {
        let taken = ["pc", "pc-2", "home"];
        let is = |s: &str| taken.contains(&s);
        assert_eq!(unique("Home", "pc", is), "home-2");
        assert_eq!(unique("???", "pc", is), "pc-3");
        assert_eq!(unique("Free", "pc", is), "free");
        let long = "y".repeat(40);
        let id = unique(&long, "pc", |s| s == "y".repeat(32));
        assert_eq!(id.len(), 32);
        assert!(id.ends_with("-2"), "{id}");
    }
}

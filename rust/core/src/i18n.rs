//! Interface strings: flat `key -> template` JSON catalogs embedded in the program.
//!
//! Templates contain `{name}` placeholders. A missing key renders as the key itself and a
//! missing parameter leaves its placeholder in place, so a gap in a catalog is visible but
//! never fatal.

use std::collections::{BTreeSet, HashMap};

use crate::notice::Notice;

const EN: &str = include_str!("../locales/en.json");
const UK: &str = include_str!("../locales/uk.json");

/// A shipped interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    /// English; also the fallback.
    En,
    /// Ukrainian.
    Uk,
}

impl Lang {
    /// All shipped languages.
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Uk];

    /// Two-letter code used in the configuration file.
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Uk => "uk",
        }
    }

    /// The language for a code such as `uk`, or a locale tag such as `uk-UA` or `en_US`.
    pub fn from_tag(tag: &str) -> Option<Self> {
        let primary = tag.split(['-', '_']).next()?;
        Self::ALL
            .into_iter()
            .find(|l| l.code().eq_ignore_ascii_case(primary))
    }

    fn source(self) -> &'static str {
        match self {
            Self::En => EN,
            Self::Uk => UK,
        }
    }
}

/// The `language` setting: follow the system or a fixed language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageSetting {
    /// Use the system language, English when it is not shipped.
    Auto,
    /// A fixed language.
    Fixed(Lang),
}

impl LanguageSetting {
    /// Parses `auto` or a language code.
    pub fn parse(s: &str) -> Option<Self> {
        if s == "auto" {
            return Some(Self::Auto);
        }
        Lang::ALL
            .into_iter()
            .find(|l| l.code() == s)
            .map(Self::Fixed)
    }

    /// The value written to the configuration file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fixed(lang) => lang.code(),
        }
    }

    /// The language to use, given the system locale tag when it is known.
    pub fn resolve(self, system_tag: Option<&str>) -> Lang {
        match self {
            Self::Fixed(lang) => lang,
            Self::Auto => system_tag.and_then(Lang::from_tag).unwrap_or(Lang::En),
        }
    }
}

/// A catalog that could not be read.
#[derive(Debug, thiserror::Error)]
#[error("invalid locale catalog: {0}")]
pub struct CatalogError(#[from] serde_json::Error);

/// A parsed catalog.
#[derive(Debug, Clone, Default)]
pub struct Catalog(HashMap<String, String>);

impl Catalog {
    /// Parses a flat JSON object of strings.
    pub fn parse(json: &str) -> Result<Self, CatalogError> {
        Ok(Self(serde_json::from_str(json)?))
    }

    /// All keys.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// The template of a key.
    pub fn template(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

/// Names of the `{placeholders}` in a template.
pub fn placeholders(template: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let tail = rest.get(start + 1..).unwrap_or_default();
        let Some(end) = tail.find('}') else { break };
        let name = tail.get(..end).unwrap_or_default();
        if is_name(name) {
            found.insert(name.to_owned());
        }
        rest = tail.get(end + 1..).unwrap_or_default();
    }
    found
}

fn is_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn render(template: &str, params: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(rest.get(..start).unwrap_or_default());
        let tail = rest.get(start..).unwrap_or_default();
        let replaced = tail.find('}').and_then(|end| {
            let name = tail.get(1..end)?;
            let value = params.iter().find(|(k, _)| *k == name)?.1;
            Some((value, end))
        });
        if let Some((value, end)) = replaced {
            out.push_str(value);
            rest = tail.get(end + 1..).unwrap_or_default();
        } else {
            out.push('{');
            rest = tail.get(1..).unwrap_or_default();
        }
    }
    out.push_str(rest);
    out
}

/// Renders texts in one language.
#[derive(Debug, Clone)]
pub struct Translator {
    lang: Lang,
    catalog: Catalog,
}

impl Translator {
    /// A translator for a shipped language.
    pub fn new(lang: Lang) -> Self {
        let catalog = Catalog::parse(lang.source()).unwrap_or_else(|err| {
            log::error!("{} catalog: {err}", lang.code());
            Catalog::default()
        });
        Self { lang, catalog }
    }

    /// The language of this translator.
    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// The text for `key` with `params` substituted.
    pub fn t(&self, key: &str, params: &[(&str, &str)]) -> String {
        match self.catalog.template(key) {
            Some(template) => render(template, params),
            None => {
                log::warn!("missing locale key '{key}' ({})", self.lang.code());
                key.to_owned()
            }
        }
    }

    /// Title and body of a notice.
    pub fn notice(&self, notice: &Notice) -> (String, String) {
        let owned = notice.params();
        let params: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let key = notice.key();
        (
            self.t(&format!("{key}.title"), &params),
            self.t(&format!("{key}.body"), &params),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(lang: Lang) -> Catalog {
        Catalog::parse(lang.source()).unwrap()
    }

    fn placeholder_map(c: &Catalog) -> HashMap<String, BTreeSet<String>> {
        c.keys()
            .map(|k| {
                (
                    k.to_owned(),
                    placeholders(c.template(k).unwrap_or_default()),
                )
            })
            .collect()
    }

    #[test]
    fn locales_have_identical_keys_and_placeholders() {
        let en = catalog(Lang::En);
        let uk = catalog(Lang::Uk);
        let en_keys: BTreeSet<&str> = en.keys().collect();
        let uk_keys: BTreeSet<&str> = uk.keys().collect();
        assert_eq!(
            en_keys.difference(&uk_keys).collect::<Vec<_>>(),
            Vec::<&&str>::new(),
            "keys missing from uk"
        );
        assert_eq!(
            uk_keys.difference(&en_keys).collect::<Vec<_>>(),
            Vec::<&&str>::new(),
            "keys missing from en"
        );
        let (en_ph, uk_ph) = (placeholder_map(&en), placeholder_map(&uk));
        for (key, ph) in &en_ph {
            assert_eq!(Some(ph), uk_ph.get(key), "placeholders differ for '{key}'");
        }
    }

    #[test]
    fn no_template_is_empty() {
        for lang in Lang::ALL {
            let c = catalog(lang);
            for key in c.keys() {
                assert!(
                    !c.template(key).unwrap_or_default().trim().is_empty(),
                    "{key}"
                );
            }
        }
    }

    #[test]
    fn every_notice_has_title_and_body_using_only_its_parameters() {
        for lang in Lang::ALL {
            let c = catalog(lang);
            for notice in crate::notice::samples() {
                let allowed: BTreeSet<String> = notice
                    .params()
                    .into_iter()
                    .map(|(k, _)| k.to_owned())
                    .collect();
                for part in ["title", "body"] {
                    let key = format!("{}.{part}", notice.key());
                    let template = c.template(&key);
                    assert!(template.is_some(), "{} lacks '{key}'", lang.code());
                    let used = placeholders(template.unwrap_or_default());
                    assert!(
                        used.is_subset(&allowed),
                        "{key} uses {used:?}, offered {allowed:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn state_names_exist() {
        use crate::domain::{PcState, TunnelState};
        let c = catalog(Lang::En);
        for s in [TunnelState::Down, TunnelState::Starting, TunnelState::Up] {
            assert!(c.template(&format!("tunnel.{}", s.as_str())).is_some());
        }
        for s in [
            PcState::Unknown,
            PcState::Offline,
            PcState::Waking,
            PcState::Online,
        ] {
            assert!(c.template(&format!("pc.{}", s.as_str())).is_some());
        }
    }

    #[test]
    fn notice_renders_with_parameters() {
        let tr = Translator::new(Lang::En);
        let (title, body) = tr.notice(&Notice::PowerFailed {
            pc: "Desk".into(),
            detail: "boom".into(),
        });
        assert!(title.contains("Desk"), "{title}");
        assert!(body.contains("boom"), "{body}");
    }

    #[test]
    fn missing_key_and_parameter_are_visible() {
        let tr = Translator::new(Lang::En);
        assert_eq!(tr.t("no.such.key", &[]), "no.such.key");
        assert_eq!(render("a {x} b {y}", &[("x", "1")]), "a 1 b {y}");
        assert_eq!(render("{{x}", &[("x", "1")]), "{1");
    }

    #[test]
    fn language_resolution() {
        assert_eq!(Lang::from_tag("uk-UA"), Some(Lang::Uk));
        assert_eq!(Lang::from_tag("EN_us"), Some(Lang::En));
        assert_eq!(Lang::from_tag("de-DE"), None);
        assert_eq!(LanguageSetting::parse("auto"), Some(LanguageSetting::Auto));
        assert_eq!(
            LanguageSetting::parse("uk"),
            Some(LanguageSetting::Fixed(Lang::Uk))
        );
        assert_eq!(LanguageSetting::parse("de"), None);
        assert_eq!(LanguageSetting::Auto.resolve(Some("uk-UA")), Lang::Uk);
        assert_eq!(LanguageSetting::Auto.resolve(Some("de-DE")), Lang::En);
        assert_eq!(LanguageSetting::Auto.resolve(None), Lang::En);
        assert_eq!(
            LanguageSetting::Fixed(Lang::Uk).resolve(Some("en-US")),
            Lang::Uk
        );
    }

    #[test]
    fn placeholder_scan() {
        let found = placeholders("{a} and {b_1} but not { c } or {");
        assert_eq!(found.into_iter().collect::<Vec<_>>(), ["a", "b_1"]);
    }
}

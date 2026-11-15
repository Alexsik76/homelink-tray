//! The "Довідка" page of the settings window: a short list of topics, each a title and a body
//! from the catalog (`help.<topic>.title`, `help.<topic>.body`). The texts describe what the
//! program does; the setup of routers and PCs is shown where it is needed (the wizard).

use crate::i18n::Translator;

/// The topics in the order they are shown.
pub const TOPICS: [&str; 8] = [
    "panel", "wake", "power", "settings", "ssh", "rdp", "files", "privacy",
];

/// One topic, worded in the translator's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading.
    pub title: String,
    /// The text; paragraphs are separated by an empty line.
    pub body: String,
}

/// Every topic, in order.
pub fn sections(tr: &Translator) -> Vec<Section> {
    TOPICS
        .iter()
        .map(|topic| Section {
            title: tr.t(&format!("help.{topic}.title"), &[]),
            body: tr.t(&format!("help.{topic}.body"), &[]),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    #[test]
    fn every_topic_has_a_title_and_a_body_in_every_language() {
        for lang in Lang::ALL {
            let tr = Translator::new(lang);
            for (topic, section) in TOPICS.iter().zip(sections(&tr)) {
                assert!(
                    !section.title.is_empty() && !section.title.starts_with("help."),
                    "{} {topic}",
                    lang.code()
                );
                assert!(
                    section.body.len() > 40 && !section.body.starts_with("help."),
                    "{} {topic}",
                    lang.code()
                );
            }
        }
    }
}

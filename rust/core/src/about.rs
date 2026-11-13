//! Data of the "Про програму" page (section 7 of the handoff).
//!
//! The texts around it are locale keys (`about.*`). What the page needs and where it comes
//! from:
//!
//! | Item | Source |
//! | --- | --- |
//! | Application version | `env!("CARGO_PKG_VERSION")` **in the app crate** (core's own version is not the product's); passed to [`About::new`] |
//! | Application licence | [`APP_LICENSE`] (the `LICENSE` file at the repository root is MIT) |
//! | Slint version | the `slint` entry of `Cargo.lock`: the app embeds the file with `include_str!("../../Cargo.lock")` and passes it to [`About::new`]; Slint 1.18 has no public version constant |
//! | Slint link | [`SLINT_URL`] |
//! | Inter licence line | [`FONT_NAME`], [`FONT_LICENSE`]; the licence text is `rust/app/ui/fonts/OFL.txt` |
//! | Third-party notices | **missing**: no notices file exists. `cargo-about` (or similar) is not set up, so there is no generated list of crate licences to link or embed. Until it exists the link has no target; see [`THIRD_PARTY_NOTICES_FILE`] |

/// The licence of the application.
pub const APP_LICENSE: &str = "MIT";

/// Where the Slint credit links to.
pub const SLINT_URL: &str = "https://slint.dev";

/// The embedded font.
pub const FONT_NAME: &str = "Inter";

/// The licence of the embedded font.
pub const FONT_LICENSE: &str = "SIL Open Font License 1.1";

/// The name of the file with the third-party licence notices that the release is meant to
/// ship next to the executable. It is not generated yet, so nothing creates it.
pub const THIRD_PARTY_NOTICES_FILE: &str = "THIRD-PARTY-NOTICES.txt";

/// What the About page shows besides the translated words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct About {
    /// Version of the application, e.g. `1.0.0`.
    pub version: String,
    /// Licence of the application.
    pub license: &'static str,
    /// Version of Slint the program was built with, when it could be found.
    pub slint_version: Option<String>,
    /// Link of the Slint credit.
    pub slint_url: &'static str,
    /// Name of the embedded font.
    pub font_name: &'static str,
    /// Licence of the embedded font.
    pub font_license: &'static str,
    /// Name of the third-party notices file.
    pub notices_file: &'static str,
}

impl About {
    /// `app_version` is the application's `CARGO_PKG_VERSION`; `lockfile` is the text of
    /// `Cargo.lock`, from which the Slint version is read.
    pub fn new(app_version: &str, lockfile: Option<&str>) -> Self {
        Self {
            version: app_version.to_owned(),
            license: APP_LICENSE,
            slint_version: lockfile.and_then(|lock| locked_version(lock, "slint")),
            slint_url: SLINT_URL,
            font_name: FONT_NAME,
            font_license: FONT_LICENSE,
            notices_file: THIRD_PARTY_NOTICES_FILE,
        }
    }
}

/// The version of `package` in the text of a `Cargo.lock` (the first entry of that name).
pub fn locked_version(lockfile: &str, package: &str) -> Option<String> {
    let wanted = format!("name = \"{package}\"");
    let mut lines = lockfile.lines();
    while let Some(line) = lines.next() {
        if line.trim() == wanted {
            let version = lines.next()?.trim();
            let value = version.strip_prefix("version = \"")?.strip_suffix('"')?;
            return Some(value.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"
[[package]]
name = "slint"
version = "1.18.1"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "slint-build"
version = "1.18.1"
"#;

    #[test]
    fn the_version_of_a_package_is_read_from_the_lockfile() {
        assert_eq!(locked_version(LOCK, "slint").as_deref(), Some("1.18.1"));
        assert_eq!(
            locked_version(LOCK, "slint-build").as_deref(),
            Some("1.18.1")
        );
        assert_eq!(locked_version(LOCK, "nope"), None);
        assert_eq!(locked_version("", "slint"), None);
    }

    #[test]
    fn the_real_lockfile_names_slint() {
        let lock = include_str!("../../Cargo.lock");
        let version = locked_version(lock, "slint").unwrap();
        assert!(version.starts_with("1."), "{version}");
    }

    #[test]
    fn about_collects_what_the_page_shows() {
        let about = About::new("1.0.0", Some(LOCK));
        assert_eq!(about.version, "1.0.0");
        assert_eq!(about.license, "MIT");
        assert_eq!(about.slint_version.as_deref(), Some("1.18.1"));
        assert_eq!(About::new("1.0.0", None).slint_version, None);
    }
}

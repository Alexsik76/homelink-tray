//! SSH onboarding in the wizard (section 11 of `docs/rust-architecture.md`, "SSH setup"): finding
//! the OpenSSH client, creating a key, and trusting a host key after the user has seen its
//! fingerprint.
//!
//! Everything here is pure: command lines, parsers and the text of `known_hosts` lines. The app
//! runs the programs through [`crate::ports::CommandRunner`] and does the file access.
//!
//! The rules of section 4 hold here:
//!
//! - **Keys only.** A key is made with an empty passphrase ([`keygen_argv`]) so that the elevated
//!   program can use it unattended. The program never reads a private key: the public half is
//!   the `.pub` file `ssh-keygen` writes beside it ([`public_key_path`]).
//! - **Nothing is accepted silently.** A host key is fetched with `ssh-keyscan`, shown as the
//!   fingerprint `ssh-keygen -l` computes ([`fingerprint_argv`], [`parse_fingerprints`]), and
//!   appended to `known_hosts` ([`known_hosts_text`]) only when the user confirms. The text that
//!   was fingerprinted is the text that is appended. An entry that exists already is never
//!   replaced ([`find_host_argv`]); a changed key is an error of the check
//!   ([`crate::checks::CheckResult::SshHostKeyChanged`]).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The program that makes keys and computes fingerprints.
pub const SSH_KEYGEN_PROGRAM: &str = "ssh-keygen";

/// The program that fetches a host key.
pub const SSH_KEYSCAN_PROGRAM: &str = "ssh-keyscan";

/// The comment of a key made by HomeLink.
pub const KEY_COMMENT: &str = "homelink";

/// The folder, inside the configuration folder, that holds the keys HomeLink makes.
pub const KEYS_DIR: &str = "keys";

/// The file name of the first key; a later one gets `_2`, `_3`, ...
pub const KEY_FILE_BASE: &str = "homelink_ed25519";

/// How long `ssh-keyscan` waits for a host, in seconds.
pub const KEYSCAN_TIMEOUT_SECONDS: u64 = 8;

/// The programs the wizard needs: `ssh` for the checks and the run-time commands, `ssh-keygen`
/// for keys and fingerprints, `ssh-keyscan` to fetch a host key.
pub const CLIENT_PROGRAMS: [&str; 3] = ["ssh", SSH_KEYGEN_PROGRAM, SSH_KEYSCAN_PROGRAM];

/// The folders where the OpenSSH client may be: those of `PATH`, then, on Windows, the folder the
/// optional feature installs it to (`%SystemRoot%\System32\OpenSSH`).
pub fn client_dirs(
    path_var: Option<&OsStr>,
    system_root: Option<&Path>,
    windows: bool,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = path_var
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    if windows && let Some(root) = system_root {
        dirs.push(root.join("System32").join("OpenSSH"));
    }
    dirs
}

/// The path of `program` in the first of `dirs` that has it, as `exists` tells; the file name
/// has `.exe` on Windows.
pub fn find_program(
    program: &str,
    dirs: &[PathBuf],
    windows: bool,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let name = if windows {
        format!("{program}.exe")
    } else {
        program.to_owned()
    };
    dirs.iter()
        .map(|dir| dir.join(&name))
        .find(|path| exists(path))
}

/// The programs of [`CLIENT_PROGRAMS`] that are not found.
pub fn missing_programs(
    dirs: &[PathBuf],
    windows: bool,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<&'static str> {
    CLIENT_PROGRAMS
        .into_iter()
        .filter(|program| find_program(program, dirs, windows, exists).is_none())
        .collect()
}

/// Where a new key goes: the first of `keys/homelink_ed25519`, `keys/homelink_ed25519_2`, ...
/// in `config_dir` for which neither the private file nor its `.pub` exists, so no key is ever
/// overwritten. `None` only if thousands of names are taken.
pub fn new_key_path(config_dir: &Path, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let dir = config_dir.join(KEYS_DIR);
    (1..=9_999).find_map(|n| {
        let name = if n == 1 {
            KEY_FILE_BASE.to_owned()
        } else {
            format!("{KEY_FILE_BASE}_{n}")
        };
        let private = dir.join(name);
        let public = PathBuf::from(public_key_path(&private.to_string_lossy()));
        (!exists(&private) && !exists(&public)).then_some(private)
    })
}

/// The command line that makes the key `path`: ed25519, no passphrase, comment `homelink`.
/// `-q` keeps the randomart and fingerprint out of the output.
pub fn keygen_argv(path: &Path) -> Vec<String> {
    vec![
        SSH_KEYGEN_PROGRAM.to_owned(),
        "-q".into(),
        "-t".into(),
        "ed25519".into(),
        "-N".into(),
        String::new(),
        "-C".into(),
        KEY_COMMENT.into(),
        "-f".into(),
        path.to_string_lossy().into_owned(),
    ]
}

/// The file with the public half of the private key `private` (what `ssh-keygen` writes).
pub fn public_key_path(private: &str) -> String {
    format!("{private}.pub")
}

/// The public key from the text of a `.pub` file: its first line, trimmed. `None` if the text
/// is empty, is not a public key line, or is a private key.
pub fn parse_public_key(text: &str) -> Option<String> {
    if text.contains("PRIVATE KEY") {
        return None;
    }
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut fields = line.split_whitespace();
    let algorithm = fields.next()?;
    let body = fields.next()?;
    (is_key_algorithm(algorithm) && is_base64(body)).then(|| line.to_owned())
}

fn is_key_algorithm(text: &str) -> bool {
    text.starts_with("ssh-") || text.starts_with("ecdsa-") || text.starts_with("sk-")
}

fn is_base64(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
}

/// Why a host cannot be written to `known_hosts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    /// The host name has a character that would change the meaning of the line.
    BadHost,
    /// The scan returned no key.
    NoKeys,
}

/// How `known_hosts` names a host: the host itself on port 22, `[host]:port` on another port.
pub fn host_pattern(host: &str, port: u16) -> Result<String, HostError> {
    let plain = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'));
    if !plain || host.starts_with(['-', '|']) {
        return Err(HostError::BadHost);
    }
    Ok(if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    })
}

/// The command line that fetches the host keys of `host`.
pub fn keyscan_argv(host: &str, port: u16) -> Vec<String> {
    vec![
        SSH_KEYSCAN_PROGRAM.to_owned(),
        "-T".into(),
        KEYSCAN_TIMEOUT_SECONDS.to_string(),
        "-p".into(),
        port.to_string(),
        "--".into(),
        host.to_owned(),
    ]
}

/// One key the host offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    /// The algorithm, for example `ssh-ed25519`.
    pub algorithm: String,
    /// The base64 key.
    pub key: String,
}

/// The keys in what `ssh-keyscan` printed: lines of `host algorithm key`. Comment lines (`#`),
/// errors and anything that is not such a line are skipped, and the host field is ignored (the
/// line for `known_hosts` is built from the host the user chose, see [`known_hosts_text`]). The
/// same key is listed once.
pub fn parse_keyscan(text: &str) -> Vec<HostKey> {
    let mut keys: Vec<HostKey> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [_host, algorithm, key] = fields.as_slice() else {
            continue;
        };
        if !is_key_algorithm(algorithm) || !is_base64(key) {
            continue;
        }
        let found = HostKey {
            algorithm: (*algorithm).to_owned(),
            key: (*key).to_owned(),
        };
        if !keys.contains(&found) {
            keys.push(found);
        }
    }
    keys
}

/// The lines to put in `known_hosts` for `keys` of `host:port`, each with its line end.
pub fn known_hosts_text(host: &str, port: u16, keys: &[HostKey]) -> Result<String, HostError> {
    let pattern = host_pattern(host, port)?;
    if keys.is_empty() {
        return Err(HostError::NoKeys);
    }
    Ok(keys
        .iter()
        .map(|k| format!("{pattern} {} {}\n", k.algorithm, k.key))
        .collect())
}

/// The `known_hosts` file the program's `ssh` reads: `.ssh/known_hosts` in the profile folder of
/// the user it runs as (the wizard runs in the same process, so the two agree).
pub fn known_hosts_path(profile: &Path) -> PathBuf {
    profile.join(".ssh").join("known_hosts")
}

/// What to append to a `known_hosts` file whose content ends (or does not end) with a line end:
/// `lines`, after a line end of its own if the file's last line has none.
pub fn known_hosts_append(file_ends_with_newline: bool, lines: &str) -> String {
    if file_ends_with_newline {
        lines.to_owned()
    } else {
        format!("\n{lines}")
    }
}

/// The command line that computes the SHA-256 fingerprints of the keys in the `known_hosts`
/// format file `path` (a temporary file with the lines that would be appended).
pub fn fingerprint_argv(path: &Path) -> Vec<String> {
    vec![
        SSH_KEYGEN_PROGRAM.to_owned(),
        "-l".into(),
        "-E".into(),
        "sha256".into(),
        "-f".into(),
        path.to_string_lossy().into_owned(),
    ]
}

/// The command line that looks `pattern` up in the `known_hosts` file `path`; it exits with 0
/// when an entry exists.
pub fn find_host_argv(pattern: &str, path: &Path) -> Vec<String> {
    vec![
        SSH_KEYGEN_PROGRAM.to_owned(),
        "-F".into(),
        pattern.to_owned(),
        "-f".into(),
        path.to_string_lossy().into_owned(),
    ]
}

/// A fingerprint as `ssh-keygen -l` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// The key size in bits.
    pub bits: u32,
    /// The hash with its algorithm, for example `SHA256:abc...`.
    pub hash: String,
    /// The key type in capitals, for example `ED25519`.
    pub kind: String,
}

impl Fingerprint {
    /// The line the user compares with the device: `ED25519 SHA256:abc...`.
    pub fn display(&self) -> String {
        format!("{} {}", self.kind, self.hash)
    }
}

/// The fingerprints in the output of `ssh-keygen -l`: lines of `bits SHA256:hash comment (TYPE)`.
pub fn parse_fingerprints(text: &str) -> Vec<Fingerprint> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let bits = fields.next()?.parse().ok()?;
            let hash = fields.next()?;
            let kind = fields
                .last()?
                .strip_prefix('(')?
                .strip_suffix(')')?
                .to_owned();
            hash.starts_with("SHA256:").then(|| Fingerprint {
                bits,
                hash: hash.to_owned(),
                kind,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> PathBuf {
        PathBuf::from(text)
    }

    #[test]
    fn the_client_is_looked_up_on_path_and_in_the_optional_feature_folder() {
        let path = std::env::join_paths(["/usr/bin", "/opt/bin"]).unwrap();
        let dirs = client_dirs(Some(&path), Some(Path::new("/win")), false);
        assert_eq!(dirs, [p("/usr/bin"), p("/opt/bin")]);
        let dirs = client_dirs(Some(&path), Some(Path::new("/win")), true);
        assert_eq!(dirs.last(), Some(&p("/win/System32/OpenSSH")));
        assert_eq!(client_dirs(None, None, true), Vec::<PathBuf>::new());

        let has = |files: &'static [&'static str]| {
            move |path: &Path| files.iter().any(|f| path == Path::new(f))
        };
        let windows_dirs = [p("/win/System32/OpenSSH")];
        let all = has(&[
            "/win/System32/OpenSSH/ssh.exe",
            "/win/System32/OpenSSH/ssh-keygen.exe",
            "/win/System32/OpenSSH/ssh-keyscan.exe",
        ]);
        assert!(missing_programs(&windows_dirs, true, &all).is_empty());
        // `ssh` without the other two is not enough.
        let partial = has(&["/win/System32/OpenSSH/ssh.exe"]);
        assert_eq!(
            missing_programs(&windows_dirs, true, &partial),
            [SSH_KEYGEN_PROGRAM, SSH_KEYSCAN_PROGRAM]
        );
        let nothing = has(&[]);
        assert_eq!(missing_programs(&windows_dirs, true, &nothing).len(), 3);
        // Off Windows there is no `.exe`.
        assert_eq!(
            find_program("ssh", &[p("/usr/bin")], false, &has(&["/usr/bin/ssh"])),
            Some(p("/usr/bin/ssh"))
        );
        assert_eq!(
            find_program("ssh", &[p("/usr/bin")], true, &has(&["/usr/bin/ssh"])),
            None
        );
    }

    #[test]
    fn a_key_never_takes_the_place_of_another() {
        let dir = p("/cfg");
        let none = |_: &Path| false;
        assert_eq!(
            new_key_path(&dir, &none),
            Some(p("/cfg/keys/homelink_ed25519"))
        );
        // The private file or only its .pub makes the name taken.
        let first = |path: &Path| path == Path::new("/cfg/keys/homelink_ed25519");
        assert_eq!(
            new_key_path(&dir, &first),
            Some(p("/cfg/keys/homelink_ed25519_2"))
        );
        let pub_only = |path: &Path| path == Path::new("/cfg/keys/homelink_ed25519.pub");
        assert_eq!(
            new_key_path(&dir, &pub_only),
            Some(p("/cfg/keys/homelink_ed25519_2"))
        );
        let all = |_: &Path| true;
        assert_eq!(new_key_path(&dir, &all), None);
    }

    #[test]
    fn the_key_command_is_ed25519_without_a_passphrase() {
        assert_eq!(
            keygen_argv(Path::new("/cfg/keys/k")),
            [
                "ssh-keygen",
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "homelink",
                "-f",
                "/cfg/keys/k"
            ]
        );
        assert_eq!(public_key_path("/cfg/keys/k"), "/cfg/keys/k.pub");
    }

    #[test]
    fn only_a_public_key_line_is_accepted() {
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc homelink";
        assert_eq!(parse_public_key(&format!("{line}\n")), Some(line.into()));
        assert_eq!(
            parse_public_key(&format!("\n  {line}  \n")),
            Some(line.into())
        );
        assert_eq!(parse_public_key(""), None);
        assert_eq!(parse_public_key("hello world"), None);
        assert_eq!(parse_public_key("ssh-ed25519"), None);
        // The program never shows a private key, even if given one by mistake.
        assert_eq!(
            parse_public_key("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n"),
            None
        );
    }

    #[test]
    fn a_host_is_named_as_known_hosts_names_it() {
        assert_eq!(host_pattern("192.168.88.1", 22).unwrap(), "192.168.88.1");
        assert_eq!(
            host_pattern("router.lan", 2222).unwrap(),
            "[router.lan]:2222"
        );
        assert_eq!(host_pattern("fe80::1", 22).unwrap(), "fe80::1");
        for bad in [
            "", "a b", "a,b", "*.lan", "!host", "|1|x", "-oProxy", "h\nx",
        ] {
            assert_eq!(host_pattern(bad, 22), Err(HostError::BadHost), "{bad:?}");
        }
    }

    #[test]
    fn the_scan_command_has_a_timeout_and_ends_the_options() {
        assert_eq!(
            keyscan_argv("10.0.0.1", 2222),
            ["ssh-keyscan", "-T", "8", "-p", "2222", "--", "10.0.0.1"]
        );
    }

    const SCAN: &str = "# 10.0.0.1:22 SSH-2.0-OpenSSH_9.6\n\
        10.0.0.1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxyz\n\
        # 10.0.0.1:22 SSH-2.0-OpenSSH_9.6\n\
        10.0.0.1 ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAB+/=\n\
        10.0.0.1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxyz\n\
        getaddrinfo nope: Name or service not known\n\
        10.0.0.1 not-a-key AAAA\n\
        10.0.0.1 ssh-rsa not base64!\n";

    #[test]
    fn the_scan_gives_each_key_once_and_nothing_else() {
        let keys = parse_keyscan(SCAN);
        assert_eq!(
            keys,
            [
                HostKey {
                    algorithm: "ssh-ed25519".into(),
                    key: "AAAAC3NzaC1lZDI1NTE5AAAAIxyz".into()
                },
                HostKey {
                    algorithm: "ssh-rsa".into(),
                    key: "AAAAB3NzaC1yc2EAAAADAQABAAAB+/=".into()
                },
            ]
        );
        assert!(parse_keyscan("").is_empty());
        assert!(parse_keyscan("connection refused").is_empty());
    }

    #[test]
    fn the_known_hosts_lines_name_the_host_the_user_chose() {
        let keys = parse_keyscan(SCAN);
        // The host field of the scan is not trusted: the line carries the chosen host.
        let spoofed = SCAN.replace("10.0.0.1 ssh", "evil.example ssh");
        let spoofed_keys = parse_keyscan(&spoofed);
        assert_eq!(spoofed_keys, keys);
        assert_eq!(
            known_hosts_text("10.0.0.1", 22, &spoofed_keys).unwrap(),
            "10.0.0.1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxyz\n\
             10.0.0.1 ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAB+/=\n"
        );
        assert_eq!(
            known_hosts_text("r.lan", 2222, &keys[..1]).unwrap(),
            "[r.lan]:2222 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxyz\n"
        );
        assert_eq!(known_hosts_text("r.lan", 22, &[]), Err(HostError::NoKeys));
        assert_eq!(known_hosts_text("a b", 22, &keys), Err(HostError::BadHost));
    }

    #[test]
    fn the_known_hosts_file_is_in_the_profile() {
        assert_eq!(
            known_hosts_path(Path::new("/home/a")),
            p("/home/a/.ssh/known_hosts")
        );
    }

    #[test]
    fn appending_does_not_glue_a_line_to_the_last_one() {
        assert_eq!(known_hosts_append(true, "h k v\n"), "h k v\n");
        assert_eq!(known_hosts_append(false, "h k v\n"), "\nh k v\n");
    }

    #[test]
    fn the_fingerprint_and_lookup_commands() {
        assert_eq!(
            fingerprint_argv(Path::new("/tmp/x")),
            ["ssh-keygen", "-l", "-E", "sha256", "-f", "/tmp/x"]
        );
        assert_eq!(
            find_host_argv("[r.lan]:2222", Path::new("/h/known_hosts")),
            ["ssh-keygen", "-F", "[r.lan]:2222", "-f", "/h/known_hosts"]
        );
    }

    #[test]
    fn fingerprints_are_read_from_the_output_of_ssh_keygen() {
        let out = "256 SHA256:AbCd+/ef10 10.0.0.1 (ED25519)\n\
                   3072 SHA256:Zz99 10.0.0.1 (RSA)\n\
                   garbage\n\
                   10.0.0.1 is not a key\n";
        let list = parse_fingerprints(out);
        assert_eq!(
            list,
            [
                Fingerprint {
                    bits: 256,
                    hash: "SHA256:AbCd+/ef10".into(),
                    kind: "ED25519".into()
                },
                Fingerprint {
                    bits: 3072,
                    hash: "SHA256:Zz99".into(),
                    kind: "RSA".into()
                },
            ]
        );
        assert_eq!(list[0].display(), "ED25519 SHA256:AbCd+/ef10");
        // An old MD5 fingerprint is not shown as if it were SHA-256.
        assert!(parse_fingerprints("256 MD5:aa:bb h (ED25519)").is_empty());
        assert!(parse_fingerprints("").is_empty());
    }
}

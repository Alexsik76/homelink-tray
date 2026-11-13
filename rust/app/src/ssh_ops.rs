//! The parts of the SSH onboarding of the wizard that touch the system: looking for the OpenSSH
//! client, making a key, fetching a host key and, on the user's explicit consent, writing it to
//! `known_hosts`. The decisions (command lines, parsing, the text of the lines) are in
//! `homelink_core::ssh_setup`; this module only runs the programs through the
//! [`CommandRunner`] and reads and writes files. Every function blocks: the wizard calls them on
//! a worker thread.
//!
//! The private key is never read: only the `.pub` file beside it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use homelink_core::notice::limit_detail;
use homelink_core::ports::{CommandOutcome, CommandRunner};
use homelink_core::ssh::explain_command;
use homelink_core::ssh_setup::{
    self as setup, HostError, KEYSCAN_TIMEOUT_SECONDS, find_host_argv, fingerprint_argv,
    keygen_argv, keyscan_argv, known_hosts_append, known_hosts_path, known_hosts_text,
    parse_fingerprints, parse_keyscan, parse_public_key, public_key_path,
};

/// How long `ssh-keygen` may take to make a key.
const KEYGEN_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the whole key scan may take: the connection wait of `ssh-keyscan` and some slack.
const SCAN_TIMEOUT: Duration = Duration::from_secs(KEYSCAN_TIMEOUT_SECONDS + 4);
/// How long `ssh-keygen` may take to compute fingerprints or look a host up.
const SHORT_TIMEOUT: Duration = Duration::from_secs(10);

/// The programs of the OpenSSH client that are not found on this computer (empty when it is
/// complete): on `PATH`, and on Windows in `%SystemRoot%\System32\OpenSSH`.
pub(crate) fn missing_client() -> Vec<&'static str> {
    let path = std::env::var_os("PATH");
    let root = std::env::var_os("SystemRoot").map(PathBuf::from);
    let dirs = setup::client_dirs(path.as_deref(), root.as_deref(), cfg!(windows));
    setup::missing_programs(&dirs, cfg!(windows), &|p: &Path| p.is_file())
}

/// A key HomeLink made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewKey {
    /// The absolute path of the private key file.
    pub(crate) path: String,
    /// The public key line.
    pub(crate) public_key: String,
}

/// Makes a key in the `keys` folder of `config_dir`, never over an existing file. The error is
/// the reason, in English (the window words it).
pub(crate) fn create_key(runner: &dyn CommandRunner, config_dir: &Path) -> Result<NewKey, String> {
    let path = setup::new_key_path(config_dir, &|p: &Path| p.exists())
        .ok_or_else(|| "no free file name for the key".to_owned())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    let output = runner.run(&keygen_argv(&path), KEYGEN_TIMEOUT);
    if let Some(reason) = explain_command(&output) {
        return Err(limit_detail(&reason));
    }
    let private = path.to_string_lossy().into_owned();
    let public_key = read_public_key(&private)
        .ok_or_else(|| "the public key file was not written".to_owned())?;
    Ok(NewKey {
        path: private,
        public_key,
    })
}

/// The public key of the key file `private`, if its `.pub` file exists and holds a public key.
/// Only the `.pub` file is opened.
pub(crate) fn read_public_key(private: &str) -> Option<String> {
    if private.trim().is_empty() {
        return None;
    }
    let text = fs::read_to_string(public_key_path(private.trim())).ok()?;
    parse_public_key(&text)
}

/// A host key the user is asked to trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostKeyOffer {
    /// How `known_hosts` names the host (`host` or `[host]:port`).
    pub(crate) pattern: String,
    /// The lines that will be appended, exactly the ones whose fingerprints were computed.
    pub(crate) lines: String,
    /// One `TYPE SHA256:...` per key, for the user to compare with the device.
    pub(crate) fingerprints: Vec<String>,
}

fn host_error(err: HostError) -> String {
    match err {
        HostError::BadHost => "the host name has a character that cannot go in known_hosts".into(),
        HostError::NoKeys => "the host offered no key".into(),
    }
}

/// Fetches the host keys of `host:port` with `ssh-keyscan` and computes their fingerprints
/// with `ssh-keygen -l`. Nothing is written to `known_hosts`.
pub(crate) fn fetch_host_key(
    runner: &dyn CommandRunner,
    host: &str,
    port: u16,
) -> Result<HostKeyOffer, String> {
    let pattern = setup::host_pattern(host, port).map_err(host_error)?;
    let scan = runner.run(&keyscan_argv(host, port), SCAN_TIMEOUT);
    if matches!(
        scan.outcome,
        CommandOutcome::NotStarted | CommandOutcome::TimedOut
    ) {
        return Err(limit_detail(&scan.text));
    }
    let keys = parse_keyscan(&scan.text);
    if keys.is_empty() {
        let text = scan.text.trim();
        return Err(if text.is_empty() {
            host_error(HostError::NoKeys)
        } else {
            limit_detail(text)
        });
    }
    let lines = known_hosts_text(host, port, &keys).map_err(host_error)?;

    // The fingerprints are computed from the very text that will be appended.
    let temp = TempFile::create(&lines).map_err(|err| format!("temporary file: {err}"))?;
    let output = runner.run(&fingerprint_argv(temp.path()), SHORT_TIMEOUT);
    drop(temp);
    if let Some(reason) = explain_command(&output) {
        return Err(limit_detail(&reason));
    }
    let fingerprints: Vec<String> = parse_fingerprints(&output.text)
        .iter()
        .map(setup::Fingerprint::display)
        .collect();
    if fingerprints.is_empty() {
        return Err("could not compute the fingerprint of the host key".into());
    }
    Ok(HostKeyOffer {
        pattern,
        lines,
        fingerprints,
    })
}

/// Why a host key was not written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrustError {
    /// `known_hosts` has an entry for the host already; it is never replaced.
    AlreadyKnown,
    /// Anything else, in English.
    Failed(String),
}

/// The profile folder `ssh` uses: `%USERPROFILE%` on Windows, `$HOME` elsewhere.
pub(crate) fn profile_dir() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Appends `offer` to the `known_hosts` file of `profile`. Call only after the user has
/// confirmed the fingerprints. An entry for the host that exists already is left alone
/// ([`TrustError::AlreadyKnown`]).
pub(crate) fn trust_host_key(
    runner: &dyn CommandRunner,
    profile: &Path,
    offer: &HostKeyOffer,
) -> Result<(), TrustError> {
    let file = known_hosts_path(profile);
    let failed =
        |err: &dyn std::fmt::Display| TrustError::Failed(format!("{}: {err}", file.display()));
    let existing = match fs::read(&file) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(failed(&err)),
    };
    if existing.is_some() {
        let found = runner.run(&find_host_argv(&offer.pattern, &file), SHORT_TIMEOUT);
        match found.outcome {
            CommandOutcome::Exited(0) => return Err(TrustError::AlreadyKnown),
            CommandOutcome::Exited(_) => {}
            _ => return Err(TrustError::Failed(limit_detail(&found.text))),
        }
    }
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|err| failed(&err))?;
    }
    let ends_with_newline = existing
        .as_ref()
        .is_none_or(|bytes| bytes.is_empty() || bytes.last() == Some(&b'\n'));
    let text = known_hosts_append(ends_with_newline, &offer.lines);
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|err| failed(&err))
}

/// A file in the temporary folder that is removed when dropped; never opens an existing one.
struct TempFile(PathBuf);

impl TempFile {
    fn create(content: &str) -> std::io::Result<Self> {
        let base = std::env::temp_dir();
        for n in 0..100u32 {
            let path = base.join(format!("homelink-hostkey-{}-{n}.txt", std::process::id()));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let temp = Self(path);
                    file.write_all(content.as_bytes())?;
                    return Ok(temp);
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
        }
        Err(std::io::Error::other("no free temporary file name"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use homelink_core::ports::CommandOutput;

    use super::*;
    use crate::windows::scripted::Scripted;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("homelink-sshops-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const SCAN: &str = "# 10.0.0.1:22 SSH-2.0-OpenSSH_9.6\n\
        10.0.0.1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxyz\n";
    const FINGERPRINT: &str = "256 SHA256:AbCd 10.0.0.1 (ED25519)\n";

    /// Plays `ssh-keygen`: writes the `.pub` file beside the key it is asked for.
    struct WritesKey {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl CommandRunner for WritesKey {
        fn run(&self, argv: &[String], _timeout: Duration) -> CommandOutput {
            self.calls.lock().unwrap().push(argv.to_vec());
            let f = argv.iter().position(|a| a == "-f").unwrap();
            fs::write(
                format!("{}.pub", argv[f + 1]),
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc homelink\n",
            )
            .unwrap();
            CommandOutput::exited(0, "")
        }
    }

    #[test]
    fn a_key_is_made_in_the_keys_folder_and_a_second_one_does_not_overwrite_it() {
        let dir = temp_dir("keys");
        let runner = WritesKey {
            calls: Mutex::new(Vec::new()),
        };
        let first = create_key(&runner, &dir).unwrap();
        assert!(first.path.ends_with("homelink_ed25519"), "{}", first.path);
        assert!(first.public_key.starts_with("ssh-ed25519 "));
        assert!(Path::new(&first.path).parent().unwrap().ends_with("keys"));
        let argv = runner.calls.lock().unwrap().remove(0);
        assert_eq!(argv, keygen_argv(Path::new(&first.path)));
        // The second key gets another name: the first file stays as it is.
        let second = create_key(&runner, &dir).unwrap();
        assert_ne!(first.path, second.path);
        assert!(
            second.path.ends_with("homelink_ed25519_2"),
            "{}",
            second.path
        );
        assert_eq!(read_public_key(&first.path), Some(first.public_key.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_key_command_is_an_error_and_a_missing_pub_file_too() {
        let dir = temp_dir("keyfail");
        let missing = Scripted::new(vec![(
            "ssh-keygen",
            CommandOutput::not_started("no such program"),
        )]);
        let err = create_key(&missing, &dir).unwrap_err();
        assert!(err.contains("no such program"), "{err}");
        let silent = Scripted::new(vec![("ssh-keygen", CommandOutput::exited(0, ""))]);
        let err = create_key(&silent, &dir).unwrap_err();
        assert!(err.contains("public key"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_pub_file_is_read() {
        let dir = temp_dir("pub");
        let private = dir.join("k");
        fs::write(&private, "-----BEGIN OPENSSH PRIVATE KEY-----\nsecret\n").unwrap();
        let private = private.to_string_lossy().into_owned();
        // No .pub file: nothing is shown, and the private file is not used instead.
        assert_eq!(read_public_key(&private), None);
        fs::write(
            public_key_path(&private),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc homelink",
        )
        .unwrap();
        assert!(read_public_key(&private).is_some());
        assert_eq!(read_public_key(""), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_fingerprint_is_computed_from_the_text_that_is_offered() {
        let runner = Scripted::new(vec![
            ("ssh-keyscan", CommandOutput::exited(0, SCAN)),
            ("ssh-keygen -l", CommandOutput::exited(0, FINGERPRINT)),
        ]);
        let offer = fetch_host_key(&runner, "10.0.0.1", 22).unwrap();
        assert_eq!(offer.pattern, "10.0.0.1");
        assert_eq!(
            offer.lines,
            "10.0.0.1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxyz\n"
        );
        assert_eq!(offer.fingerprints, ["ED25519 SHA256:AbCd"]);
        let calls = runner.calls();
        assert_eq!(calls[0].0, keyscan_argv("10.0.0.1", 22));
        // The temporary file is gone, and the second command named it.
        let temp = Path::new(calls[1].0.last().unwrap());
        assert!(!temp.exists());
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn a_scan_that_finds_nothing_is_an_error_not_an_empty_offer() {
        let runner = Scripted::new(vec![(
            "ssh-keyscan",
            CommandOutput::exited(0, "getaddrinfo x: Name or service not known"),
        )]);
        let err = fetch_host_key(&runner, "x", 22).unwrap_err();
        assert!(err.contains("Name or service"), "{err}");
        let silent = Scripted::new(vec![("ssh-keyscan", CommandOutput::exited(0, ""))]);
        assert!(fetch_host_key(&silent, "x", 22).is_err());
        let gone = Scripted::new(vec![(
            "ssh-keyscan",
            CommandOutput::not_started("no ssh-keyscan"),
        )]);
        assert!(
            fetch_host_key(&gone, "x", 22)
                .unwrap_err()
                .contains("no ssh-keyscan")
        );
        // Only one command ran in each case: no fingerprint of nothing.
        assert_eq!(runner.calls().len(), 1);
        // A host that cannot be written to known_hosts is refused before anything runs.
        let never = Scripted::new(vec![]);
        assert!(fetch_host_key(&never, "a b", 22).is_err());
        assert!(never.calls().is_empty());
    }

    fn offer() -> HostKeyOffer {
        HostKeyOffer {
            pattern: "10.0.0.1".into(),
            lines: "10.0.0.1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxyz\n".into(),
            fingerprints: vec!["ED25519 SHA256:AbCd".into()],
        }
    }

    #[test]
    fn a_trusted_key_is_appended_after_the_existing_lines() {
        let dir = temp_dir("trust");
        let file = known_hosts_path(&dir);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        // The last line has no line end: the new line must not be glued to it.
        fs::write(&file, "other ssh-rsa AAAA").unwrap();
        let not_found = Scripted::new(vec![("ssh-keygen -F", CommandOutput::exited(1, ""))]);
        trust_host_key(&not_found, &dir, &offer()).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "other ssh-rsa AAAA\n10.0.0.1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxyz\n"
        );
        assert_eq!(not_found.calls()[0].0, find_host_argv("10.0.0.1", &file));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_and_its_folder_are_created_when_missing() {
        let dir = temp_dir("fresh");
        let never = Scripted::new(vec![]);
        trust_host_key(&never, &dir, &offer()).unwrap();
        assert_eq!(
            fs::read_to_string(known_hosts_path(&dir)).unwrap(),
            offer().lines
        );
        // No lookup was needed for a file that did not exist.
        assert!(never.calls().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_that_exists_is_never_replaced_or_doubled() {
        let dir = temp_dir("known");
        let file = known_hosts_path(&dir);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "10.0.0.1 ssh-ed25519 OLDKEY\n").unwrap();
        let found = Scripted::new(vec![(
            "ssh-keygen -F",
            CommandOutput::exited(0, "# Host 10.0.0.1 found: line 1"),
        )]);
        assert_eq!(
            trust_host_key(&found, &dir, &offer()),
            Err(TrustError::AlreadyKnown)
        );
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "10.0.0.1 ssh-ed25519 OLDKEY\n"
        );
        // A lookup that cannot run is not a reason to append.
        let broken = Scripted::new(vec![(
            "ssh-keygen -F",
            CommandOutput::not_started("no ssh-keygen"),
        )]);
        assert!(matches!(
            trust_host_key(&broken, &dir, &offer()),
            Err(TrustError::Failed(_))
        ));
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "10.0.0.1 ssh-ed25519 OLDKEY\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_client_is_found_or_reported_missing() {
        // Only the shape is checked: what is installed depends on the machine.
        let missing = missing_client();
        assert!(missing.len() <= 3);
        for program in missing {
            assert!(setup::CLIENT_PROGRAMS.contains(&program));
        }
    }
}

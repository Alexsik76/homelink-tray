//! Opening Remote Desktop, as `rdp.py` did: `mstsc <file.rdp>`.
//!
//! The program writes the connection file (the host, the user name and the options of the
//! configuration) and starts `mstsc` on it. It never handles the password: the user types it
//! in the Remote Desktop window, which may remember it.
//!
//! The file is written only when it does not exist yet. A file that the user signed or
//! edited in `mstsc` must stay as it is, so it is never rewritten (delete it to get a fresh
//! one). It lives in `rdp/<pc id>.rdp` next to the configuration file.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use homelink_core::config::Config;
use homelink_core::domain::{Host, PcId, Rdp};

/// The folder of the connection files, inside the configuration folder.
const FOLDER: &str = "rdp";

/// The text of the connection file: one setting per line, CRLF line ends.
pub(crate) fn file_text(host: &Host, user: &str, options: &[String]) -> String {
    let mut text = String::new();
    for line in [
        format!("full address:s:{host}"),
        format!("username:s:{user}"),
    ]
    .iter()
    .chain(options)
    {
        text.push_str(line);
        text.push_str("\r\n");
    }
    text
}

/// The bytes `mstsc` reads: UTF-16 little endian with a byte order mark (what Python's
/// `encoding="utf-16"` wrote on Windows).
pub(crate) fn encode_utf16le_with_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// Where the connection file of a PC lives.
pub(crate) fn file_path(config_dir: &Path, pc: &PcId) -> PathBuf {
    config_dir.join(FOLDER).join(format!("{pc}.rdp"))
}

/// Writes `content` to `path` unless a file is there. Returns whether it was written.
pub(crate) fn write_if_missing(path: &Path, content: &[u8]) -> io::Result<bool> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(content)?;
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err),
    }
}

/// Starts `mstsc` on a connection file. The program is not waited for.
fn launch(path: &Path) -> io::Result<()> {
    Command::new("mstsc")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_child| ())
}

/// The Remote Desktop settings of the PCs that have them, and where to put their files.
pub(crate) struct RdpLauncher {
    config_dir: PathBuf,
    pcs: Vec<(PcId, Host, Rdp)>,
}

impl RdpLauncher {
    /// A launcher for the PCs of `config`; files go next to the configuration file in
    /// `config_dir`.
    pub(crate) fn new(config: &Config, config_dir: &Path) -> Self {
        let pcs = config
            .pcs
            .iter()
            .filter_map(|pc| Some((pc.id.clone(), pc.host.clone(), pc.rdp.clone()?)))
            .collect();
        Self {
            config_dir: config_dir.to_owned(),
            pcs,
        }
    }

    /// Makes sure the connection file of a PC exists (it is written only when missing) and
    /// returns its path. The error is ready to show.
    pub(crate) fn prepare_file(&self, id: &PcId) -> Result<PathBuf, String> {
        let Some((_, host, rdp)) = self.pcs.iter().find(|(pc, _, _)| pc == id) else {
            return Err("this PC has no Remote Desktop settings".into());
        };
        let path = file_path(&self.config_dir, id);
        let content = encode_utf16le_with_bom(&file_text(host, &rdp.user, &rdp.options));
        write_if_missing(&path, &content)
            .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
        Ok(path)
    }

    /// Writes the connection file if needed and opens it in `mstsc`. The error is ready to
    /// show.
    pub(crate) fn open(&self, id: &PcId) -> Result<(), String> {
        let path = self.prepare_file(id)?;
        log::info!("opening Remote Desktop for {id}: {}", path.display());
        launch(&path).map_err(|err| format!("cannot start mstsc: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use homelink_core::testing::single_config;

    fn host() -> Host {
        Host::parse("192.168.88.10").unwrap()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("homelink-rdp-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_file_text_lists_host_user_and_options_with_crlf() {
        let options = vec![
            "use redirection server name:i:1".to_owned(),
            "screen mode id:i:2".to_owned(),
        ];
        assert_eq!(
            file_text(&host(), "alex", &options),
            "full address:s:192.168.88.10\r\nusername:s:alex\r\nuse redirection server name:i:1\r\nscreen mode id:i:2\r\n"
        );
        assert_eq!(
            file_text(&host(), "alex", &[]),
            "full address:s:192.168.88.10\r\nusername:s:alex\r\n"
        );
    }

    #[test]
    fn the_file_is_utf16_with_a_byte_order_mark() {
        let bytes = encode_utf16le_with_bom("a\r\nб");
        assert_eq!(bytes, [0xFF, 0xFE, b'a', 0, b'\r', 0, b'\n', 0, 0x31, 0x04]);
        // Outside the basic plane: a surrogate pair.
        assert_eq!(
            encode_utf16le_with_bom("\u{1F600}"),
            [0xFF, 0xFE, 0x3D, 0xD8, 0x00, 0xDE]
        );
        assert_eq!(encode_utf16le_with_bom(""), [0xFF, 0xFE]);
    }

    #[test]
    fn the_path_is_rdp_slash_the_pc_id_next_to_the_configuration() {
        let id = PcId::parse("desktop").unwrap();
        assert_eq!(
            file_path(Path::new("cfg"), &id),
            Path::new("cfg").join("rdp").join("desktop.rdp")
        );
    }

    #[test]
    fn an_existing_file_is_never_rewritten() {
        let dir = temp_dir("keep");
        let path = dir.join("rdp").join("desktop.rdp");
        assert!(write_if_missing(&path, b"first").unwrap());
        assert!(!write_if_missing(&path, b"second").unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"first");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_launcher_knows_only_pcs_with_remote_desktop_settings() {
        let dir = temp_dir("pcs");
        let launcher = RdpLauncher::new(&single_config(), &dir);
        assert_eq!(launcher.pcs.len(), 1);
        let unknown = PcId::parse("nobody").unwrap();
        assert_eq!(
            launcher.prepare_file(&unknown),
            Err("this PC has no Remote Desktop settings".into())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_connection_file_holds_what_the_configuration_says_and_is_kept() {
        // Only the file is made here; `mstsc` is never started by a test.
        let dir = temp_dir("write");
        let launcher = RdpLauncher::new(&single_config(), &dir);
        let id = PcId::parse("desktop").unwrap();
        let path = launcher.prepare_file(&id).unwrap();
        assert_eq!(path, file_path(&dir, &id));
        let expected =
            encode_utf16le_with_bom("full address:s:192.168.88.10\r\nusername:s:alex\r\n");
        assert_eq!(fs::read(&path).unwrap(), expected);
        // A file the user changed is left alone on the next call.
        fs::write(&path, b"edited").unwrap();
        assert_eq!(launcher.prepare_file(&id).unwrap(), path);
        assert_eq!(fs::read(&path).unwrap(), b"edited");
        let _ = fs::remove_dir_all(&dir);
    }
}

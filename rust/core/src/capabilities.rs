//! What the system offers: which external programs HomeLink depends on are installed, and so
//! which features work (the capability spec of 15.11.2026, `VIC/HomeLink/homelink-capabilities-spec.md`).
//!
//! - **Checks are cheap.** A program is looked up by file existence only; nothing is started.
//!   The lookup is pure: the environment ([`Env`]) and the file test (`exists`) are given, so
//!   every rule is tested on Linux.
//! - **Lookup order** on Windows: an explicit path from the settings (the tunnels'
//!   `wireguard_exe`), the folder the program is known to live in (`System32`,
//!   `System32\OpenSSH`, `System32\WindowsPowerShell\v1.0`, `Program Files\WireGuard`), then the
//!   folders of `PATH` with the extensions of `PATHEXT`. Elsewhere: `PATH` only.
//! - **A missing program disables only what depends on it** ([`Feature::tools`]). Without
//!   `ping` the tunnel cannot be checked, and tunnel control is blocked as a whole (owner's
//!   decision of 15.11.2026), as without the WireGuard client.
//! - **The found path is the one to run**, so a program found only in a known folder that is not
//!   on `PATH` still works ([`Capabilities::program`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// An external program HomeLink may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tool {
    /// `wireguard.exe` of the official WireGuard client: installs and removes tunnel services.
    WireGuard,
    /// `sc.exe`: reads the state of a tunnel service.
    ServiceControl,
    /// `ping`: checks that a tunnel carries traffic.
    Ping,
    /// The OpenSSH client `ssh`.
    Ssh,
    /// `ssh-keygen`: creates keys, computes fingerprints, finds `known_hosts` entries.
    SshKeygen,
    /// `ssh-keyscan`: fetches a host key.
    SshKeyscan,
    /// `mstsc.exe`: the Remote Desktop client.
    Rdp,
    /// `powershell.exe`: restarts the program with administrator rights.
    PowerShell,
    /// `whoami.exe`: reads whether the program runs with administrator rights.
    Whoami,
    /// `cmd.exe`: opens links in the browser.
    Shell,
    /// `reg.exe`: reads the theme, the language, the code page and the display scale.
    Registry,
}

impl Tool {
    /// Every tool, in the order of the capabilities page.
    pub const ALL: [Tool; 11] = [
        Tool::WireGuard,
        Tool::ServiceControl,
        Tool::Ping,
        Tool::Ssh,
        Tool::SshKeygen,
        Tool::SshKeyscan,
        Tool::Rdp,
        Tool::PowerShell,
        Tool::Whoami,
        Tool::Shell,
        Tool::Registry,
    ];

    /// The program name without extension, as it is run and shown.
    pub fn program(self) -> &'static str {
        match self {
            Tool::WireGuard => "wireguard",
            Tool::ServiceControl => "sc",
            Tool::Ping => "ping",
            Tool::Ssh => "ssh",
            Tool::SshKeygen => "ssh-keygen",
            Tool::SshKeyscan => "ssh-keyscan",
            Tool::Rdp => "mstsc",
            Tool::PowerShell => "powershell",
            Tool::Whoami => "whoami",
            Tool::Shell => "cmd",
            Tool::Registry => "reg",
        }
    }

    /// A stable code for locale keys (`tool.<code>`) and logs.
    pub fn code(self) -> &'static str {
        match self {
            Tool::WireGuard => "wireguard",
            Tool::ServiceControl => "sc",
            Tool::Ping => "ping",
            Tool::Ssh => "ssh",
            Tool::SshKeygen => "ssh_keygen",
            Tool::SshKeyscan => "ssh_keyscan",
            Tool::Rdp => "mstsc",
            Tool::PowerShell => "powershell",
            Tool::Whoami => "whoami",
            Tool::Shell => "cmd",
            Tool::Registry => "reg",
        }
    }

    /// The folder (relative to `%SystemRoot%` or `%ProgramFiles%`) the program is known to be
    /// in on Windows.
    fn known_dir(self, env: &Env) -> Option<PathBuf> {
        let system32 = env.system_root.as_ref().map(|root| root.join("System32"));
        match self {
            Tool::WireGuard => env.program_files.as_ref().map(|p| p.join("WireGuard")),
            Tool::Ssh | Tool::SshKeygen | Tool::SshKeyscan => system32.map(|s| s.join("OpenSSH")),
            Tool::PowerShell => system32.map(|s| s.join("WindowsPowerShell").join("v1.0")),
            Tool::ServiceControl
            | Tool::Ping
            | Tool::Rdp
            | Tool::Whoami
            | Tool::Shell
            | Tool::Registry => system32,
        }
    }
}

/// Where a tool was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    /// The program file, an absolute path.
    Found(PathBuf),
    /// Not found anywhere it was looked for.
    Missing,
}

impl ToolStatus {
    /// The path, when found.
    pub fn path(&self) -> Option<&Path> {
        match self {
            ToolStatus::Found(path) => Some(path),
            ToolStatus::Missing => None,
        }
    }
}

/// What the lookup depends on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    /// Windows rules (known folders, `.exe`, `PATHEXT`).
    pub windows: bool,
    /// The folders of `PATH`, in order.
    pub path: Vec<PathBuf>,
    /// The extensions of `PATHEXT` (Windows), such as `.COM`, `.EXE`; empty for the default.
    pub pathext: Vec<String>,
    /// `%SystemRoot%`.
    pub system_root: Option<PathBuf>,
    /// `%ProgramFiles%`.
    pub program_files: Option<PathBuf>,
    /// The `wireguard_exe` of every tunnel in the settings, in order.
    pub wireguard_exe: Vec<PathBuf>,
    /// The programs of the user's commands (the first element of each), as written.
    pub programs: Vec<String>,
}

/// The extensions tried on Windows when `PATHEXT` is not set.
const DEFAULT_PATHEXT: [&str; 4] = [".COM", ".EXE", ".BAT", ".CMD"];

impl Env {
    /// The environment of this process: `PATH`, `PATHEXT`, `SystemRoot`, `ProgramFiles`, with
    /// the settings' `wireguard_exe` paths.
    pub fn from_process(wireguard_exe: Vec<PathBuf>, programs: Vec<String>) -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        Self {
            windows: cfg!(windows),
            path: var("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
            pathext: var("PATHEXT")
                .map(|p| {
                    p.to_string_lossy()
                        .split(';')
                        .map(str::trim)
                        .filter(|e| !e.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            system_root: var("SystemRoot").map(PathBuf::from),
            program_files: var("ProgramFiles").map(PathBuf::from),
            wireguard_exe,
            programs,
        }
    }

    /// The file names `name` may have: on Windows the name as given when it has an extension,
    /// otherwise the name with each `PATHEXT` extension; elsewhere the name.
    fn file_names(&self, name: &str) -> Vec<String> {
        if !self.windows {
            return vec![name.to_owned()];
        }
        if Path::new(name).extension().is_some() {
            return vec![name.to_owned()];
        }
        let exts: Vec<&str> = if self.pathext.is_empty() {
            DEFAULT_PATHEXT.to_vec()
        } else {
            self.pathext.iter().map(String::as_str).collect()
        };
        exts.into_iter()
            .map(|ext| format!("{name}{}", ext.to_ascii_lowercase()))
            .collect()
    }

    /// Every file a tool is looked for at, in order.
    pub fn candidates(&self, tool: Tool) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if tool == Tool::WireGuard {
            out.extend(self.wireguard_exe.iter().cloned());
        }
        if self.windows
            && let Some(dir) = tool.known_dir(self)
        {
            out.push(dir.join(format!("{}.exe", tool.program())));
        }
        out.extend(self.on_path(tool.program()));
        out
    }

    /// The files `name` may be in the folders of `PATH`.
    fn on_path(&self, name: &str) -> Vec<PathBuf> {
        let names = self.file_names(name);
        self.path
            .iter()
            .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
            .collect()
    }

    /// Where the program of a user command (the first element of its argument list) is: an
    /// absolute or relative path as given, a bare name on `PATH` (and, for a bare name on
    /// Windows, in `System32`, where the shell would find it as well).
    pub fn resolve_program(&self, program: &str, exists: &dyn Fn(&Path) -> bool) -> ToolStatus {
        let path = Path::new(program);
        // A Windows path is recognised by its separators or drive also when this code runs
        // elsewhere (the tests).
        let has_dir = path.components().count() > 1
            || path.is_absolute()
            || (self.windows && program.contains(['\\', '/', ':']));
        let candidates: Vec<PathBuf> = if has_dir {
            self.file_names(program)
                .into_iter()
                .map(PathBuf::from)
                .collect()
        } else {
            let mut list = Vec::new();
            if self.windows
                && let Some(root) = &self.system_root
            {
                for name in self.file_names(program) {
                    list.push(root.join("System32").join(name));
                }
            }
            list.extend(self.on_path(program));
            list
        };
        first_existing(candidates, exists)
    }
}

fn first_existing(candidates: Vec<PathBuf>, exists: &dyn Fn(&Path) -> bool) -> ToolStatus {
    candidates
        .into_iter()
        .find(|path| exists(path))
        .map_or(ToolStatus::Missing, ToolStatus::Found)
}

/// A feature of the program and the tools it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// Raising and lowering tunnels and reading their service state.
    TunnelControl,
    /// Checking that a tunnel carries traffic. Without it tunnel control is blocked too
    /// ([`Capabilities::tunnels_available`]).
    TunnelProbe,
    /// Waking through a relay, sleep and shutdown over SSH, the SSH checks of the wizard.
    SshActions,
    /// Creating keys and trusting host keys in the wizard.
    KeyManagement,
    /// Opening Remote Desktop.
    Rdp,
    /// The magic packet: built into the program.
    MagicPacket,
    /// Restarting with administrator rights.
    Elevation,
    /// Opening links in the browser.
    Links,
}

impl Feature {
    /// Every feature, in the order of the tiles of the capabilities page.
    pub const ALL: [Feature; 8] = [
        Feature::TunnelControl,
        Feature::TunnelProbe,
        Feature::SshActions,
        Feature::KeyManagement,
        Feature::Rdp,
        Feature::MagicPacket,
        Feature::Elevation,
        Feature::Links,
    ];

    /// The tools the feature needs; all of them must be found.
    pub fn tools(self) -> &'static [Tool] {
        match self {
            Feature::TunnelControl => &[Tool::WireGuard, Tool::ServiceControl],
            Feature::TunnelProbe => &[Tool::Ping],
            Feature::SshActions => &[Tool::Ssh],
            Feature::KeyManagement => &[Tool::SshKeygen, Tool::SshKeyscan],
            Feature::Rdp => &[Tool::Rdp],
            Feature::MagicPacket => &[],
            Feature::Elevation => &[Tool::Whoami, Tool::PowerShell],
            Feature::Links => &[Tool::Shell],
        }
    }

    /// A stable code for locale keys (`capability.<code>`) and logs.
    pub fn code(self) -> &'static str {
        match self {
            Feature::TunnelControl => "tunnels",
            Feature::TunnelProbe => "tunnel_probe",
            Feature::SshActions => "ssh",
            Feature::KeyManagement => "keys",
            Feature::Rdp => "rdp",
            Feature::MagicPacket => "magic_packet",
            Feature::Elevation => "elevation",
            Feature::Links => "links",
        }
    }
}

/// Something an action needs to run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Need {
    /// One of HomeLink's tools.
    Tool(Tool),
    /// The program of a user command, as written in the settings.
    Program(String),
}

/// The result of a lookup: the status of every tool and the user programs that are missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    tools: BTreeMap<Tool, ToolStatus>,
    missing_programs: BTreeSet<String>,
}

/// Looks every tool and user program of `env` up, testing files with `exists`.
pub fn detect(env: &Env, exists: &dyn Fn(&Path) -> bool) -> Capabilities {
    Capabilities {
        tools: Tool::ALL
            .into_iter()
            .map(|tool| (tool, first_existing(env.candidates(tool), exists)))
            .collect(),
        missing_programs: env
            .programs
            .iter()
            .filter(|p| env.resolve_program(p, exists) == ToolStatus::Missing)
            .cloned()
            .collect(),
    }
}

impl Capabilities {
    /// Every tool found where `found` says (for tests and demo mode).
    pub fn from_statuses(statuses: impl IntoIterator<Item = (Tool, ToolStatus)>) -> Self {
        let mut tools: BTreeMap<Tool, ToolStatus> = Tool::ALL
            .into_iter()
            .map(|t| (t, ToolStatus::Missing))
            .collect();
        tools.extend(statuses);
        Self {
            tools,
            missing_programs: BTreeSet::new(),
        }
    }

    /// The same, with these user programs missing (for tests).
    pub fn with_missing_programs(mut self, programs: impl IntoIterator<Item = String>) -> Self {
        self.missing_programs.extend(programs);
        self
    }

    /// Every tool found, at `/<program>` (for tests and demo mode).
    pub fn all_found() -> Self {
        Self::from_statuses(Tool::ALL.map(|t| {
            (
                t,
                ToolStatus::Found(PathBuf::from(format!("/{}", t.program()))),
            )
        }))
    }

    /// The status of `tool`.
    pub fn status(&self, tool: Tool) -> &ToolStatus {
        self.tools.get(&tool).unwrap_or(&ToolStatus::Missing)
    }

    /// The program to run for `tool`: the found path, or the bare name when it is missing (the
    /// start then fails with "cannot run", which is reported as such).
    pub fn program(&self, tool: Tool) -> String {
        self.status(tool).path().map_or_else(
            || tool.program().to_owned(),
            |p| p.to_string_lossy().into_owned(),
        )
    }

    /// The bare program names of the found tools and their paths, for the command runner
    /// (`StdCommandRunner::set_programs`). The WireGuard client is left out: it is run by
    /// the path in the settings.
    pub fn program_paths(&self) -> BTreeMap<String, PathBuf> {
        self.tools
            .iter()
            .filter(|(tool, _)| **tool != Tool::WireGuard)
            .filter_map(|(tool, status)| {
                status
                    .path()
                    .map(|p| (tool.program().to_owned(), p.to_owned()))
            })
            .collect()
    }

    /// The tools of `feature` that are missing; empty when it is available.
    pub fn missing_for(&self, feature: Feature) -> Vec<Tool> {
        feature
            .tools()
            .iter()
            .copied()
            .filter(|tool| self.status(*tool).path().is_none())
            .collect()
    }

    /// Whether every tool of `feature` was found.
    pub fn available(&self, feature: Feature) -> bool {
        self.missing_for(feature).is_empty()
    }

    /// The tools missing for anything a tunnel does: control and the probe (without the probe
    /// the tunnel's state cannot be known, so control is blocked as well).
    pub fn missing_for_tunnels(&self) -> Vec<Tool> {
        let mut missing = self.missing_for(Feature::TunnelControl);
        missing.extend(self.missing_for(Feature::TunnelProbe));
        missing
    }

    /// Which of `needs` are missing, in their order.
    pub fn missing(&self, needs: &[Need]) -> Vec<Need> {
        needs
            .iter()
            .filter(|need| match need {
                Need::Tool(tool) => self.status(*tool).path().is_none(),
                Need::Program(program) => self.missing_programs.contains(program),
            })
            .cloned()
            .collect()
    }

    /// The user programs that were looked up and not found.
    pub fn missing_programs(&self) -> impl Iterator<Item = &str> {
        self.missing_programs.iter().map(String::as_str)
    }

    /// Whether tunnels can be raised, lowered and checked.
    pub fn tunnels_available(&self) -> bool {
        self.missing_for_tunnels().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn windows_env() -> Env {
        Env {
            windows: true,
            path: vec![
                PathBuf::from(r"C:\Tools"),
                PathBuf::from(r"C:\Windows\System32"),
            ],
            pathext: vec![".COM".into(), ".EXE".into(), ".BAT".into()],
            system_root: Some(PathBuf::from(r"C:\Windows")),
            program_files: Some(PathBuf::from(r"C:\Program Files")),
            wireguard_exe: vec![PathBuf::from(r"D:\WG\wireguard.exe")],
            programs: Vec::new(),
        }
    }

    fn files(list: &[&str]) -> impl Fn(&Path) -> bool + use<> {
        let set: BTreeSet<PathBuf> = list.iter().map(PathBuf::from).collect();
        move |p: &Path| set.contains(p)
    }

    #[test]
    fn windows_looks_in_the_explicit_path_then_the_known_folder_then_path() {
        let env = windows_env();
        let wg = env.candidates(Tool::WireGuard);
        assert_eq!(wg[0], PathBuf::from(r"D:\WG\wireguard.exe"));
        assert_eq!(
            wg[1],
            Path::new(r"C:\Program Files")
                .join("WireGuard")
                .join("wireguard.exe")
        );
        assert_eq!(wg[2], Path::new(r"C:\Tools").join("wireguard.com"));
        assert_eq!(wg[3], Path::new(r"C:\Tools").join("wireguard.exe"));

        let ssh = env.candidates(Tool::Ssh);
        assert_eq!(
            ssh[0],
            Path::new(r"C:\Windows")
                .join("System32")
                .join("OpenSSH")
                .join("ssh.exe")
        );
        let ps = env.candidates(Tool::PowerShell);
        assert_eq!(
            ps[0],
            Path::new(r"C:\Windows")
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        );
    }

    #[test]
    fn the_first_existing_candidate_wins() {
        let env = windows_env();
        let only_on_path = Path::new(r"C:\Tools").join("ssh.exe");
        let in_openssh = Path::new(r"C:\Windows")
            .join("System32")
            .join("OpenSSH")
            .join("ssh.exe");
        let both = [
            only_on_path.to_str().unwrap_or_default(),
            in_openssh.to_str().unwrap_or_default(),
        ];
        let caps = detect(&env, &files(&both));
        assert_eq!(
            caps.status(Tool::Ssh),
            &ToolStatus::Found(in_openssh.clone())
        );
        let caps = detect(&env, &files(&both[..1]));
        assert_eq!(caps.status(Tool::Ssh), &ToolStatus::Found(only_on_path));
        assert_eq!(caps.status(Tool::Ping), &ToolStatus::Missing);
    }

    #[test]
    fn a_found_tool_is_run_by_its_path_and_a_missing_one_by_its_name() {
        let env = windows_env();
        let ssh = Path::new(r"C:\Windows")
            .join("System32")
            .join("OpenSSH")
            .join("ssh.exe");
        let caps = detect(&env, &files(&[ssh.to_str().unwrap_or_default()]));
        assert_eq!(caps.program(Tool::Ssh), ssh.to_string_lossy());
        assert_eq!(caps.program(Tool::Rdp), "mstsc");
    }

    #[test]
    fn other_systems_use_path_only_without_extensions() {
        let env = Env {
            windows: false,
            path: vec![PathBuf::from("/usr/bin")],
            system_root: Some(PathBuf::from("/ignored")),
            program_files: Some(PathBuf::from("/ignored")),
            ..Env::default()
        };
        assert_eq!(env.candidates(Tool::Ssh), [PathBuf::from("/usr/bin/ssh")]);
        assert_eq!(env.candidates(Tool::Ping), [PathBuf::from("/usr/bin/ping")]);
    }

    #[test]
    fn pathext_defaults_when_unset_and_an_extension_is_kept() {
        let env = Env {
            windows: true,
            path: vec![PathBuf::from("P")],
            ..Env::default()
        };
        let names: Vec<PathBuf> = env.on_path("tool");
        assert_eq!(
            names,
            ["tool.com", "tool.exe", "tool.bat", "tool.cmd"].map(|n| Path::new("P").join(n))
        );
        assert_eq!(env.on_path("tool.ps1"), [Path::new("P").join("tool.ps1")]);
    }

    #[test]
    fn features_need_all_their_tools_and_tunnels_also_need_ping() {
        let found = |t: Tool| {
            (
                t,
                ToolStatus::Found(PathBuf::from(format!("/{}", t.program()))),
            )
        };
        let caps = Capabilities::from_statuses([
            found(Tool::WireGuard),
            found(Tool::ServiceControl),
            found(Tool::Ssh),
            found(Tool::SshKeygen),
        ]);
        assert!(caps.available(Feature::TunnelControl));
        assert!(!caps.available(Feature::TunnelProbe));
        assert!(!caps.tunnels_available());
        assert_eq!(caps.missing_for_tunnels(), [Tool::Ping]);
        assert!(caps.available(Feature::SshActions));
        assert_eq!(caps.missing_for(Feature::KeyManagement), [Tool::SshKeyscan]);
        assert!(caps.available(Feature::MagicPacket));
        assert_eq!(caps.missing_for(Feature::Rdp), [Tool::Rdp]);

        let none = Capabilities::from_statuses([]);
        assert_eq!(
            none.missing_for_tunnels(),
            [Tool::WireGuard, Tool::ServiceControl, Tool::Ping]
        );
        assert!(Capabilities::all_found().tunnels_available());
    }

    #[test]
    fn user_programs_resolve_by_path_or_by_name() {
        let env = windows_env();
        let wol = Path::new(r"C:\Tools").join("wakeonlan.exe");
        let exists = files(&[wol.to_str().unwrap_or_default(), r"D:\bin\wake.cmd"]);
        assert_eq!(
            env.resolve_program("wakeonlan", &exists),
            ToolStatus::Found(wol)
        );
        assert_eq!(
            env.resolve_program(r"D:\bin\wake.cmd", &exists),
            ToolStatus::Found(PathBuf::from(r"D:\bin\wake.cmd"))
        );
        assert_eq!(env.resolve_program("nope", &exists), ToolStatus::Missing);

        let linux = Env {
            windows: false,
            path: vec![PathBuf::from("/usr/bin")],
            ..Env::default()
        };
        let exists = files(&["/usr/bin/etherwake", "/opt/wake"]);
        assert_eq!(
            linux.resolve_program("etherwake", &exists),
            ToolStatus::Found(PathBuf::from("/usr/bin/etherwake"))
        );
        assert_eq!(
            linux.resolve_program("/opt/wake", &exists),
            ToolStatus::Found(PathBuf::from("/opt/wake"))
        );
    }

    #[test]
    fn needs_cover_tools_and_user_programs() {
        let env = Env {
            windows: false,
            path: vec![PathBuf::from("/usr/bin")],
            programs: vec!["etherwake".into(), "/opt/missing".into()],
            ..Env::default()
        };
        let caps = detect(&env, &files(&["/usr/bin/etherwake", "/usr/bin/ssh"]));
        assert_eq!(
            caps.missing_programs().collect::<Vec<_>>(),
            ["/opt/missing"]
        );
        let needs = [
            Need::Tool(Tool::Ssh),
            Need::Tool(Tool::Rdp),
            Need::Program("etherwake".into()),
            Need::Program("/opt/missing".into()),
        ];
        assert_eq!(
            caps.missing(&needs),
            [Need::Tool(Tool::Rdp), Need::Program("/opt/missing".into())]
        );
    }

    #[test]
    fn program_paths_name_the_found_tools_except_wireguard() {
        let caps = Capabilities::from_statuses([
            (Tool::Ssh, ToolStatus::Found(PathBuf::from("/x/ssh.exe"))),
            (
                Tool::WireGuard,
                ToolStatus::Found(PathBuf::from("/wg/wireguard.exe")),
            ),
        ]);
        let map = caps.program_paths();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("ssh"), Some(&PathBuf::from("/x/ssh.exe")));
    }

    #[test]
    fn codes_are_unique() {
        let tools: BTreeSet<&str> = Tool::ALL.iter().map(|t| t.code()).collect();
        assert_eq!(tools.len(), Tool::ALL.len());
        let features: BTreeSet<&str> = Feature::ALL.iter().map(|f| f.code()).collect();
        assert_eq!(features.len(), Feature::ALL.len());
    }
}

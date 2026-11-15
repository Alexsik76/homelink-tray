# HomeLink

[![CI](https://github.com/Alexsik76/homelink-tray/actions/workflows/ci.yml/badge.svg)](https://github.com/Alexsik76/homelink-tray/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/Alexsik76/homelink-tray?include_prereleases)](https://github.com/Alexsik76/homelink-tray/releases)

[Українською](README.uk.md)

A Windows tray utility that reaches remote PCs: it brings up WireGuard tunnels, wakes PCs
(through a relay host over SSH, with a magic packet or with a command), puts them to sleep
or shuts them down over SSH, opens Remote Desktop, and shows the state of every tunnel and
PC next to the tray.

Version 1.0 is written in Rust with the [Slint](https://slint.dev) interface and is in
development (release candidates are published as pre-releases). The released Python version
0.3.0 is archived in [`legacy/python`](legacy/python/README.md).

## How it works

- **Panel.** A left click on the tray icon opens a panel: tunnels on the left, the PCs behind
  each tunnel opposite them, PCs without a tunnel last. Each element is a name and a state
  marker (green: working, yellow: in progress, blue: off, grey outline: unknown, red: the last
  action failed). A left click opens the actions, a right click a details window. Sleep and
  shut down ask for a second click.
- **Tray icon.** The screen of the monitor glyph shows the PCs as a whole, the corner square
  the tunnels; the tooltip says how many tunnels are up and how many PCs are online.
- **Wake.** On a PC behind a tunnel, "Wake" brings the tunnel up first. The program never drops
  a tunnel by itself: only the tunnel button does. A PC counts as online when all of its
  readiness ports accept a TCP connection.
- **Notifications** in the tray: a PC that came up after waking, a tunnel or PC that was lost,
  and every failed action.
- **Settings** in `config.toml` next to the program, with a settings window, a five-step wizard
  for adding a PC and a help page. Saving changes only the values that changed, so comments
  stay.
- Light and dark theme, English and Ukrainian.

**No secrets, no HTTP.** Relay hosts and PCs are reached with the system OpenSSH client and a
key. HomeLink stores no passwords, sends no telemetry, checks for no updates and connects only
to the hosts in its settings. CI fails if an HTTP or TLS client library enters the build.

## Requirements

- Windows 10 (including LTSC) or 11, administrator rights (the tunnel runs as a Windows
  service; the program asks for them on start).
- The official [WireGuard client](https://www.wireguard.com/install/) with each tunnel
  imported once.
- The OpenSSH client (Settings > Apps > Optional features > OpenSSH Client; it may be missing
  on Windows 10 LTSC) for relay hosts and for sleep and shut down.
- For waking through a relay: a MikroTik router with RouterOS 7, or a Linux host with
  `etherwake` or `wakeonlan`, in the PC's network.
- For sleep and shut down over SSH: an OpenSSH server on the PC (Linux, or Windows with an
  administrator account).

## Getting started

1. Download `HomeLink.exe` from the [releases](https://github.com/Alexsik76/homelink-tray/releases)
   and put it in its own folder. The executable is not code-signed, so Windows SmartScreen
   may warn on the first start.
2. Start it. On the first start it writes `config.toml` next to itself from a commented
   template ([`rust/app/config.example.toml`](rust/app/config.example.toml)) and exits.
3. Start it again. The tunnel, relay and PC of the template are examples: in the settings
   window (the button at the bottom of the panel) edit or delete them, add tunnels with the
   tunnel form (the `.conf.dpapi` file of the imported tunnel and a host that answers ping only
   through it) and PCs with the wizard. The wizard creates SSH keys, shows the commands to run
   on the relay and on the PC, checks the connections and asks before it trusts a host key.
   `config.toml` can also be edited by hand.

Command-line options:

| Option | Meaning |
| --- | --- |
| `--config <path>` | Use another settings file |
| `--lang auto\|en\|uk` | Override the language of the settings |
| `--theme auto\|light\|dark` | The panel's colours (default: the Windows app theme) |
| `--check-config` | Check the settings file, print every problem and exit |
| `--version` | Print the version and exit |
| `--demo` | Simulated PCs and tunnels (only in the CI build with the `demo` feature) |

### Autostart without the UAC prompt

In an elevated PowerShell:

```
schtasks /create /tn HomeLink /tr "C:\path\to\HomeLink.exe" /sc onlogon /rl highest /f
```

## Waking through a relay host

The magic packet must be sent inside the PC's network: a broadcast frame does not travel
through a routed tunnel. HomeLink connects to a relay over SSH and runs one command there:

| Preset | Remote command |
| --- | --- |
| `routeros` | `/tool wol mac=<MAC> interface=<interface>` |
| `etherwake` | `[sudo -n ]etherwake -i <interface> <MAC>` |
| `wakeonlan` | `[sudo -n ]wakeonlan [-i <address>] <MAC>` |

The wizard shows the commands that prepare the relay: on RouterOS a group that may only log in
over SSH and run `/tool wol`, a user in it limited to this computer's address, and the public
key; on Linux the package, the key in `authorized_keys` and, with `sudo`, a `sudoers.d` rule for
that one program. The host key is trusted in the wizard after you compare the fingerprint.

Without a relay, a magic packet can be sent from this computer to the PC's own IP address; the
router in front of the PC then needs a static ARP entry for it (the wizard explains this).

## Sleep and shut down over SSH

HomeLink sends a fixed command: on Linux `sudo systemctl suspend -i` or
`sudo systemctl poweroff`, on Windows `rundll32.exe powrprof.dll,SetSuspendState 0,1,0` (it
hibernates when hibernation is on) or `shutdown /s /t 0`. Step 4 of the wizard shows what to run
on the PC once:

- **Linux** (as root): the public key in the user's `authorized_keys` and
  `/etc/sudoers.d/homelink`, which lets the user run exactly those two commands without a
  password. `-i` makes the PC sleep even when something blocks it (GNOME blocks sleep while a
  Remote Desktop session exists).
- **Windows** (PowerShell as administrator): the OpenSSH server, started now and at boot, and the
  key in `administrators_authorized_keys`, readable only by Administrators and SYSTEM.

## Remote Desktop

The RDP button writes `rdp\<PC id>.rdp` next to the settings on first use and opens it with
`mstsc`; the file is never rewritten, so it can be signed. HomeLink never handles the password:
type it in the Remote Desktop window, or store it with
`cmdkey /generic:TERMSRV/<pc address> /user:<user> /pass`.

## Files next to `config.toml`

`homelink.log` (rotated at 1 MB, two older copies, local time), `homelink.lock` (stops a second
copy), `keys\` (SSH keys created by the wizard) and `rdp\` (Remote Desktop files).

## Building and development

Rust stable is pinned in [`rust/rust-toolchain.toml`](rust/rust-toolchain.toml). The app runs on
Windows; the core (`rust/core`) and its tests also run on Linux.

```
cd rust
cargo test --workspace --all-features
cargo build -p homelink --release              # the release build
cargo build -p homelink --release --features demo
```

From Linux the app can be checked for Windows with
`HOMELINK_NO_RESOURCES=1 cargo clippy -p homelink --all-features --target x86_64-pc-windows-msvc`
(the variable skips the Windows resources, which need `rc.exe`).

Documents: [architecture](docs/rust-architecture.md), [interface](docs/design/ui-handoff.md) and
its [dark theme](docs/design/ui-handoff-dark.md), and the
[manual release checklist](docs/release-checklist.uk.md) (Ukrainian).

## Releases

A tag such as `v1.0.0` or `v1.0.0-rc.1` equal to the version in `rust/app/Cargo.toml` builds the
program and publishes a release with `HomeLink.exe`, its SHA-256 sum and the third-party notices;
a tag with a suffix is a pre-release. Releases 0.3.0 and earlier are the Python version.

| Version | Download |
| --- | --- |
| All releases | [Releases](https://github.com/Alexsik76/homelink-tray/releases) |

## License

[MIT](LICENSE). Third-party licences: [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt).

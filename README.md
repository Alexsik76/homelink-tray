# HomeLink

[Українською](README.uk.md)

A Windows tray utility that reaches a home PC in one click: it brings up a WireGuard
tunnel, asks a MikroTik router to send Wake-on-LAN and shows the state of the tunnel
and of the PC in the tray icon.

## How it works

- **Left click** on the icon: bring the tunnel up, check the PC's ports and, when it is silent,
  send Wake-on-LAN. A notification appears once the PC answers.
- **Right click**: connect or disconnect the tunnel, wake the PC, open RDP, put the PC to
  sleep or shut it down (after a confirmation), open the settings file, quit.
- **Icon**: the screen colour is the PC state (green: online, amber: waking, grey: off),
  the badge is the tunnel state (blue: connected, yellow: connecting, red: off).

The magic packet is sent by the router, not by this computer: a broadcast frame does
not travel through a routed tunnel. The program calls the router's REST API
(`POST /rest/tool/wol`) through the tunnel.

## Requirements

- Windows 10 or 11, administrator rights (the tunnel runs as a Windows service).
- The official [WireGuard client](https://www.wireguard.com/install/) with the tunnel
  imported once.
- MikroTik RouterOS 7 with the REST API reachable through the tunnel.
- Python 3.11+ to build or to run from source.

## Router setup

Create a user that is only allowed to send Wake-on-LAN and only from the tunnel address
of this computer:

```
/user group add name=wol policy=read,write,test,rest-api,api
/user add name=homelink group=wol address=<tunnel address of this computer>/32 password=<password>
/ip service set www disabled=no
```

`test` is the policy required by `/tool wol`; `api` is checked for REST logins.

## Remote power commands

The "Turn off PC" submenu (enabled while the PC is online) runs `shutdown_command` or
`suspend_command` on this computer after a confirmation. With the example settings that
is an SSH call to the home PC, which needs the following there:

- Key authentication: with `BatchMode=yes` SSH never asks for a password.
- The host key must already be accepted: connect once manually (`ssh user@<pc_ip>`).
- Passwordless `sudo` for exactly these two commands. Create `/etc/sudoers.d/homelink`
  with `visudo -f /etc/sudoers.d/homelink`:

```
<user> ALL=(root) NOPASSWD: /usr/bin/systemctl poweroff, /usr/bin/systemctl suspend
```

## Build

```
build.bat
```

The result is `dist\HomeLink.exe`. Exit a running copy before rebuilding.

## Settings

On the first start the program creates `config.json` next to the executable from the
built-in template, tells you where it is and exits. Fill it in and start again. Every
key is required: there are no default values in the code.

| Key | Meaning |
| --- | --- |
| `language` | Interface language: `uk` or `en` (a file in `locales/`) |
| `wireguard_exe` | Path to `wireguard.exe` |
| `tunnel_conf` | Path to the imported tunnel: `...\Data\Configurations\<name>.conf.dpapi` |
| `tunnel_timeout` | Seconds to wait for the router after the tunnel starts |
| `router_ip` | Router address inside the tunnel, used to check the tunnel |
| `router_api` | Base URL of the REST API, `http://` or `https://` |
| `router_user`, `router_password` | Router user; the password is stored in plain text |
| `router_verify_tls` | Verify the router certificate for `https://` |
| `router_timeout` | Seconds to wait for the router's reply |
| `wol_mac` | MAC address of the PC |
| `wol_interface` | Router interface that faces the PC |
| `pc_ip` | PC address |
| `pc_ports` | TCP ports that must all accept a connection for the PC to count as online, e.g. `[3389]` for RDP |
| `port_timeout_ms` | Timeout of a single port check |
| `offline_after_failures` | Failed checks in a row before an online PC is shown as off; one successful check is enough to show it as online |
| `ping_timeout_ms` | Timeout of a single ping of the router |
| `poll_seconds` | Interval between state checks |
| `wake_poll_seconds` | The same interval while the PC is waking |
| `wake_timeout` | Seconds to wait for the PC after Wake-on-LAN |
| `shutdown_command` | Command that powers the PC off, run on this computer: a list of the program and its arguments, e.g. an `ssh` call |
| `suspend_command` | The same command for sleep |
| `power_command_timeout` | Seconds to wait for either command to finish |
| `rdp_user` | User name written into the RDP file |
| `rdp_file` | RDP file name, relative to `config.json` |
| `rdp_options` | Extra lines of the RDP file |
| `icons_dir` | Folder with your own icons; empty for the built-in set |
| `log_file` | Log file name, relative to `config.json` |
| `log_level` | Log detail: `DEBUG`, `INFO`, `WARNING` or `ERROR` |

The RDP file is created on the first "Open RDP" and is never rewritten afterwards, so
it can be digitally signed. Delete it to apply changed `rdp_*` settings.

## Autostart without the UAC prompt

In an elevated PowerShell:

```
schtasks /create /tn HomeLink /tr "C:\path\to\HomeLink.exe" /sc onlogon /rl highest /f
```

## Remote Desktop notes

- **Saved password.** Store it without passing it on the command line:
  `cmdkey /generic:TERMSRV/<pc_ip> /user:<user> /pass`
- **"Unknown remote connection" warning.** Sign the RDP file and trust the publisher
  (elevated PowerShell):

```powershell
$rdp = "C:\path\to\home.rdp"
$cert = New-SelfSignedCertificate -Subject "CN=HomeLink RDP" -Type CodeSigningCert `
    -CertStoreLocation Cert:\LocalMachine\My -NotAfter (Get-Date).AddYears(10)
Export-Certificate -Cert $cert -FilePath "$env:TEMP\homelink.cer" | Out-Null
Import-Certificate -FilePath "$env:TEMP\homelink.cer" -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
rdpsign.exe /v /sha256 $cert.Thumbprint $rdp
reg add "HKLM\Software\Policies\Microsoft\Windows NT\Terminal Services" `
    /v TrustedCertThumbprints /t REG_SZ /d $cert.Thumbprint /f
```

## Custom icons

An icon set is twelve PNG files named `<pc state>_<tunnel state>.png`, where the PC
state is `unknown`, `offline`, `waking` or `online` and the tunnel state is `down`,
`starting` or `up`. Put them in a folder and point `icons_dir` at it. The built-in set
is produced by `python tools/generate_icons.py`.

## Development

```
py -m venv .venv
.venv\Scripts\python -m pip install -e ".[dev]"
.venv\Scripts\python -m pytest
.venv\Scripts\python -m ruff check .
.venv\Scripts\python -m ruff format .
.venv\Scripts\pythonw run_homelink.pyw
```

| Module | Responsibility |
| --- | --- |
| `config.py` | Reading and validating `config.json` |
| `state.py` | Tunnel and PC states |
| `controller.py` | Logic: actions and state tracking, no interface |
| `tunnel.py` | WireGuard tunnel service |
| `router.py` | Wake-on-LAN through the MikroTik REST API |
| `net.py` | Ping, port checks and running external commands |
| `rdp.py` | RDP file |
| `icons.py`, `i18n.py` | Icons and interface strings |
| `app.py` | Tray icon and menu |
| `elevation.py` | Administrator rights |

## License

[MIT](LICENSE)

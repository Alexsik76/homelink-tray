# HomeLink

[![CI](https://github.com/Alexsik76/homelink-tray/actions/workflows/ci.yml/badge.svg)](https://github.com/Alexsik76/homelink-tray/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/Alexsik76/homelink-tray)](https://github.com/Alexsik76/homelink-tray/releases/latest)

[Українською](README.uk.md)

A Windows tray utility that gives access to a home PC: it brings up a WireGuard
tunnel, asks a MikroTik router to send Wake-on-LAN and shows the state of the tunnel
and of the PC in the tray icon.

## Version 1.0 in Rust (in development)

HomeLink is being rewritten in Rust with the Slint interface; the code is in `rust/` on
`main`. It has not been released yet and will be published as version 1.0.0. The released
Python version 0.3.0 is archived in [`legacy/python`](legacy/python/README.md).

What changes:

- **Several PCs and tunnels.** A panel next to the tray shows tunnels on the left and
  the PCs behind each tunnel opposite them; PCs without a tunnel come last. Each element
  is a name and a state marker (green: working, yellow: in progress, blue: off, grey
  outline: unknown, red: failure only). A left click opens the actions, a right click
  opens a details window. Wake, sleep and shut down ask for a second click.
- **Wake methods per PC:** a magic packet to any address, a relay over SSH (RouterOS,
  `etherwake` or `wakeonlan`), or a custom command.
- **No secrets and no HTTP client.** Routers and PCs are reached with the system OpenSSH
  client and a key; the router's REST API is no longer used. The program stores no
  passwords, sends no telemetry and talks only to the hosts in its settings. CI fails if
  an HTTP or TLS client library enters the build.
- **Settings in `config.toml`**, edited in place so that comments survive, plus a
  settings window and a five-step wizard for adding a PC (key creation, host-key trust
  only after the fingerprint is confirmed, setup commands for the relay).
- Light and dark theme, English and Ukrainian, tray notifications, an administrator
  manifest.

State on 15.11.2026: the panel and the real mode work on Windows 10 LTSC (the tunnel is
raised by the program). Not yet checked on a real machine: the wizard, tray
notifications, key creation and host-key trust, wake/sleep/shutdown/RDP in the real mode,
and the RouterOS setup commands. Help is not written yet.

**Trying it.** Open the latest
[CI run of `main`](https://github.com/Alexsik76/homelink-tray/actions/workflows/ci.yml?query=branch%3Amain)
and download the `HomeLink-rust-demo` artifact. `HomeLink.exe --demo --lang uk` shows the
panel with simulated PCs and tunnels (`--theme light|dark` picks the theme). Without
`--demo` the program acts for real: on the first start it creates `config.toml` next to
the executable from a template and exits; fill it in and start again (`--config <path>`
uses another file). The executable asks for administrator rights.

**Building** (Rust stable; the app runs on Windows, the core and the tests also on
Linux):

```
cd rust
cargo test --workspace --all-features
cargo build -p homelink --release --features demo
```

Design documents: [architecture](docs/rust-architecture.md),
[interface](docs/design/ui-handoff.md)
and its [dark theme](docs/design/ui-handoff-dark.md).

## Releases

| Version | Download | Changes |
| --- | --- | --- |
| Latest ![latest version](https://img.shields.io/github/v/release/Alexsik76/homelink-tray?label=) | [HomeLink.exe](https://github.com/Alexsik76/homelink-tray/releases/latest/download/HomeLink.exe) | [What is new](https://github.com/Alexsik76/homelink-tray/releases/latest) |
| Earlier | [All releases](https://github.com/Alexsik76/homelink-tray/releases) | |

The published releases (0.3.0 and earlier) are the Python version; its documentation is in
[`legacy/python`](legacy/python/README.md). The executable is not code-signed, so Windows
SmartScreen may show a warning on the first start.

## License

[MIT](LICENSE)

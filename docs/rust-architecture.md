# HomeLink 1.0: architecture of the Rust rewrite

Status: proposal for stage 1 (design only, no code). It describes the rewrite of the
Python 0.3.0 tray utility (`src/homelink/`, `tests/`) in Rust with Slint 1.18, released as
v1.0.0 from the same repository. References such as `controller.py` point to the Python
code that is ported; `docs/design.md` describes the Python-era UI and is revised in phase 6.

## 1. Goals and settled decisions

- Rust + Slint 1.18 (no WebView2: Windows 10 LTSC must work). Slint only for the GUI, so the
  executable stays small.
- Several remote PCs, each optionally behind a tunnel. A tunnel may serve several PCs; PCs
  without a tunnel form their own group. Old `config.json` files are not migrated.
- One command raises the tunnel and wakes an inactive PC.
- Wake methods per PC: MikroTik REST, direct magic packet, arbitrary command; more later.
- Remote PCs may run Windows or Linux.
- A wizard adds a PC. Passwords live in Windows Credential Manager; the config only refers
  to them. Nothing leaves the computer: no telemetry, no update check, network traffic only
  to hosts the user configured.
- Readiness = all listed TCP ports open. An online PC turns "off" only after
  `offline_after_failures` consecutive failed checks; one success means online.
- Third-party users: tests, lint and CI are part of the deliverable. Own visual style
  (sharp corners, embedded Inter). Slint's royalty-free licence requires crediting Slint in
  the About window.
- Python is deleted once the Rust build reaches feature parity (phase 7).

## 2. Workspace layout

```
rust/
  Cargo.toml            workspace, shared lints and release profile
  rust-toolchain.toml   pinned stable toolchain
  deny.toml             cargo-deny: licences, advisories, bans
  core/                 crate `homelink-core` (lib): no Windows, no Slint
    src/
      domain.rs         ids, Tunnel, Pc, WakeMethod, PowerMethod, PowerAction, states
      config/           schema (serde), defaults, validation, load/save
      machine/          sans-IO state machine (section 5)
      engine.rs         event loop, job executor, snapshot publishing
      ports.rs          traits that isolate side effects (section 6)
      adapters/         portable adapters: tcp probe, magic packet, command runner,
                        MikroTik REST client
      wizard.rs         wizard steps and draft-to-config conversion (pure logic)
      notice.rs, i18n.rs, secret.rs, rdp.rs
      testing.rs        fakes behind the `testing` feature
    locales/            en.json, uk.json
    tests/              integration tests with local servers and fixtures
  app/                  crate `homelink` (bin, produces HomeLink.exe): Windows + Slint
    src/
      main.rs           startup, single instance, elevation check, wiring
      windows/          adapters: WireGuard service, Credential Manager, ICMP,
                        taskbar theme, OEM output decoding, mstsc launch
      ui/               Slint glue: snapshot -> models, callbacks -> commands, tray, notices
    ui/                 *.slint (panel, wizard, about), fonts/, icons/
    build.rs
```

Dependency rule: `app -> core`, never the reverse. `core` depends only on std, `serde`, a
TOML/JSON parser, `thiserror`, `log` and (feature `http`) `ureq`. It builds and tests on
Linux; only `app` needs Windows. The prototype in `rust/src` and `rust/ui` moves into
`rust/app` in phase 4 (tray, panel placement and focus-loss logic are reused,
`model.rs` sample data is deleted).

Size policy: no `tokio`, no `reqwest`, no gettext, no extra GUI crates. Release profile
with `lto = "fat"`, `codegen-units = 1`, `opt-level = "s"`, `strip = true`, panic = unwind
(worker panics are caught, section 7). CI enforces a size budget on `HomeLink.exe`
(value fixed after the phase 0 measurement; the Python one-file build is the reference).

## 3. Domain model

Type sketches only; fields mirror section 8.

```
TunnelId, PcId, RouterId        validated slugs [a-z0-9-]{1,32}

Tunnel      { id, name, kind: TunnelKind, probe_host, timeout, ping_timeout }
TunnelKind  = WireGuardService { exe, conf }          // #[non_exhaustive]: OpenVPN etc. later
Router      { id, kind: Mikrotik, api_url, user, password_ref, verify_tls, timeout }

Pc          { id, name, os: Os, host, tunnel: Option<TunnelId>, ports: NonEmpty<u16>,
              wake: Option<WakeMethod>, power: Option<PowerMethod>, rdp: Option<Rdp>,
              timing: Timing }
Os          = Windows | Linux
WakeMethod  = MikrotikRest { router, mac, interface }        // #[non_exhaustive]
            | MagicPacket  { mac, broadcast, port }          // UDP, default 255.255.255.255:9
            | Command      { argv, timeout }
PowerAction = Sleep | Shutdown
PowerMethod = Ssh     { user, port, identity_file, timeout } // uses the system ssh client
            | Command { sleep: Option<argv>, shutdown: Option<argv>, timeout }
Rdp         { user, options }
Timing      { poll, wake_poll, port_timeout, offline_after_failures, wake_timeout }
```

`argv` is always a list, never a shell string, so there is no quoting or injection issue.
Placeholders `{host}`, `{mac}`, `{user}` are substituted per element. SSH presets by `Os`:

| Os | Shutdown | Sleep |
| --- | --- | --- |
| Linux | `sudo systemctl poweroff` | `sudo systemctl suspend -i` (as in README: `-i` ignores inhibitors) |
| Windows | `shutdown /s /t 0` | `rundll32.exe powrprof.dll,SetSuspendState 0,1,0` |

Windows sleep through `SetSuspendState` hibernates when hibernation is enabled; the wizard
says so. A `Command` power method covers anything else.

States (as in `state.py`, now per entity):

```
TunnelState = Down | Starting | Up
PcState     = Unknown | Offline | Waking | Online
```

A PC with a tunnel is `Unknown` whenever its tunnel is not `Up`. A PC without a tunnel is
probed directly and is `Unknown` only until its first probe.

Commands (UI to engine): `ToggleTunnel(id)`, `Wake(pc)`, `Power(pc, action)`,
`OpenRdp(pc)`, `Reload(config)`, `Shutdown`. Semantics of `Wake(pc)`: tunnel PC with the
tunnel not `Up` means connect-and-wake (the old `connect_and_wake`); otherwise send the
wake request. Ignored while the PC is `Online` or `Waking`.

Action availability is computed in core and shipped in the snapshot, so the UI holds no
rules: Wake needs a wake method and a PC not `Online`/`Waking`; Sleep, Shutdown and RDP need
an `Online` PC and the matching configuration (same rules as `app.py` `can_wake` and
`pc_online`).

## 4. Sensitive data

- `SecretStore` trait (get/set/delete by target name); Windows implementation over
  Credential Manager (`CredReadW`/`CredWriteW`, generic credentials, `CRED_PERSIST_LOCAL_MACHINE`
  so nothing roams). Targets are `HomeLink/router/<id>`; the config holds `password_ref`.
- `Secret` is a newtype that zeroizes on drop and prints as `***` in `Debug`/`Display`.
  Secrets are fetched at the moment of use, never cached in config structures, never
  logged; URLs are logged without credentials.
- The RDP password, if the user wants it saved, is written by the wizard as the standard
  `TERMSRV/<host>` credential (what `mstsc` reads); the config stores only the user name.
  This replaces the manual `cmdkey` step from the README.
- SSH uses keys only (`BatchMode=yes`), so there is no SSH password to store.
- No telemetry, no automatic update check, no crash upload. The log file is local.

## 5. State machine and polling

### 5.1 Shape

`controller.py` mixes decisions, blocking waits and a global `busy` lock. The port splits it:

- `machine` is a pure function `step(&mut Machine, Input, now) -> Output` with no I/O and no
  threads. `Input` = user `Command` | `Observation` (probe result) | `JobDone` (result of a
  side effect) | `Tick`. `Output` = `Vec<Effect>` (probe tunnel/PC, bring tunnel up/down,
  send wake, run power command) + `Vec<Notice>` + the next wake-up time.
- `engine` owns the machine on one thread, runs effects through the ports on worker
  threads and feeds results back as inputs (section 7).

Waiting for the tunnel (the `_start_tunnel` loop) is a deadline stored in state plus
1 s probes, not a blocked thread. Consequences: no global lock; a tunnel stuck in
`Starting` can always be toggled off (the intent of design.md "Tunnel item while connecting");
operations are serialised per entity: at most one in-flight job per tunnel and per PC, a
second command for the same entity is dropped with a log line (as `busy.acquire(blocking=False)`).

### 5.2 Tunnel transitions

Per tunnel. `expect_down` is set by a user disconnect (the old `user_stopped`).
`pending` is the set of PCs waiting for this tunnel in connect-and-wake.

| # | From | Event | To | Effects / notices | controller.py |
| --- | --- | --- | --- | --- | --- |
| T1 | Down | `ToggleTunnel` or `Wake(pc)` | Starting | job `TunnelUp`; deadline = now + timeout; `pending += pc` | `_start_tunnel` |
| T2 | Starting | `Wake(pc)` | Starting | `pending += pc` | busy gate |
| T3 | Up | `Wake(pc)` | Up | probe PC (see P4) | `connect_and_wake` |
| T4 | Starting/Up | `ToggleTunnel` | (observed next) | job `TunnelDown`; `expect_down = true`; `pending = {}` | `toggle_tunnel` |
| T5 | Starting | `JobDone(TunnelUp, Err)` | observed next | notice `TunnelFailed(detail)`; `pending = {}`; probe at once | `_start_tunnel` |
| T6 | Starting (deadline open) | probe: router answers | Up | start PC probes for `pending` and attached PCs | `_start_tunnel` loop |
| T7 | Starting (deadline passed) | still silent | Starting | `pending = {}`; deadline-less polling goes on (T9); only logged in v0.3 | `_start_tunnel` end |
| T8 | any | probe: service not running | Down | attached PCs to `Unknown`, clear `waking_until` and failure counters; if previous state was `Up` and not `expect_down`: notice `TunnelLost`; clear `expect_down` | `_refresh` first branch |
| T9 | any | probe: service running, router silent | Starting | attached PCs to `Unknown`, counters cleared; no notice | `_refresh` second branch |
| T10 | any | probe: router answers | Up | start attached PCs' probes | `_refresh` rest |

While Starting with an open deadline only the reachability probe runs (the Python loop
ignored the service state), at 1 s intervals; otherwise the tunnel is polled every
`poll_seconds`. A probe job checks the service first and pings only when it runs, as
`_refresh` does.

### 5.3 PC transitions

Per PC, evaluated on a PC probe result (`ready` = all ports open). Counters: `failures`,
`waking_until`, flag `power_sent`. Entering `Online` from any other state clears
`power_sent` (as `_set`).

| # | State | Event | New state | Notices / counters | controller.py |
| --- | --- | --- | --- | --- | --- |
| P1 | any | `ready` | Online | `failures = 0`, `waking_until` cleared; if was `Waking`: notice `PcOnline` | `_refresh` |
| P2 | Online | not ready, `failures` after the increment `< N` | Online | `failures += 1`, logged | `_refresh` |
| P3 | Waking | not ready, `now < waking_until` | Waking | `failures += 1` | `_refresh` |
| P4 | Unknown/Offline/Online/Waking after deadline | not ready | Offline | `failures += 1`; if was `Online` and not `power_sent`: `PcLost`; if was `Waking`: `PcTimeout` | `_refresh` |
| P5 | any | tunnel T8/T9 | Unknown | `failures = 0`, `waking_until` cleared, no `PcLost` | `_refresh` |
| P6 | any | connect-and-wake probe: `ready` | Online | `failures = 0`; no `PcOnline` | `connect_and_wake` |
| P7 | any | connect-and-wake probe: not ready | (unchanged) | job `Wake` | `connect_and_wake` |
| P8 | any | `JobDone(Wake, Ok)` | Waking | `waking_until = now + wake_timeout` | `_send_wol` |
| P9 | any | `JobDone(Wake, Err)` | (unchanged) | notice `WakeFailed(detail)` | `_send_wol` |
| P10 | Online | `Power(a)` | (unchanged) | job `Power(a)` | `_power` |
| P11 | other | `Power(a)` | (unchanged) | notice `PowerUnavailable` | `_power` |
| P12 | any | `JobDone(Power, exit 0)` | (unchanged) | `power_sent = true`; notice `SleepSent`/`ShutdownSent` | `_power` |
| P13 | any | `JobDone(Power, failure)` | (unchanged) | notice `PowerFailed(detail)` | `_power` |

`N` = `offline_after_failures`. P2 compares the counter after the increment, matching
`self._pc_failures < offline_after_failures`. Power commands do not set
state; the next probe does.

### 5.4 Polling

- One timer in the engine thread; the machine returns the next due time. Each tunnel is
  probed every `poll_seconds` (1 s while a start is awaited). When a tunnel observation says
  `Up`, probes for its PCs follow; a PC without a tunnel is probed on its own schedule.
  PC interval is `poll_seconds`, or `wake_poll_seconds` while `Waking` (`poll_interval`).
- No overlapping probes of one entity: a probe is issued only if none is in flight.
- An observation is skipped while a job (TunnelUp/Down, Wake, Power) of the same entity is
  in flight, as `refresh()` is skipped while `busy`.

### 5.5 Deviations from `controller.py` (owner to confirm)

1. T7: a tunnel start that times out now produces a notice `TunnelTimeout` (v0.3 only logs it).
2. `Controller.wake()` and the notice `tunnel_required` are dropped: the UI never called
   `wake()` directly; `Wake` on a PC with a tunnel is always connect-and-wake.
3. `wol_failed` becomes `wake_failed` because wake is no longer WoL-only.
4. Connect-and-wake on an `Up` tunnel does not flip the tunnel to `Starting` for an instant.

Everything else (rules P1-P13, T8-T10, the 200 character detail limit, silence for user
disconnects and for PCs that were told to power off) is kept.

## 6. Traits that isolate side effects (`core/src/ports.rs`)

| Trait | Purpose | Production impl | Test impl |
| --- | --- | --- | --- |
| `TunnelDriver` | `is_running`, `up`, `down` for a `Tunnel` | `app`: SCM query + `wireguard.exe /installtunnelservice` / `/uninstalltunnelservice` (replaces psutil; "already installed" still tolerated) | scripted fake |
| `Pinger` | ICMP echo to a host with timeout | `app`: `IcmpSendEcho` (no console window, no locale parsing) | set of reachable hosts |
| `PortProbe` | all ports of a host accept TCP, short-circuit on first failure | `core`: `TcpStream::connect_timeout` | local listener / fake |
| `WakeSender` | send one wake request for a `Pc` | one impl per method: `MikrotikRest` (over `HttpClient`), `MagicPacket` (UDP broadcast), `CommandWake`; selected through a `WakeRegistry` keyed by method kind | recording fake; real UDP socket on loopback |
| `HttpClient` | POST JSON with basic auth, TLS verify flag, timeout | `core` feature `http`: `ureq` + rustls (fallback if size budget is missed: WinHTTP in `app`) | local mock server |
| `CommandRunner` | run argv without a shell, hidden window, timeout; returns `(code, text)` | `core`: `std::process` with `CREATE_NO_WINDOW` on Windows; `app` wraps it to decode OEM code page | recording fake |
| `PowerRunner` | run `Sleep`/`Shutdown` for a `PowerMethod` | `core`: builds argv (SSH or command), uses `CommandRunner` | recording fake |
| `SecretStore` | get/set/delete secrets | `app`: Credential Manager | in-memory |
| `Clock` | monotonic `now()` | `std::time::Instant` | manual clock |
| `Executor` | run a blocking job and deliver `JobDone` | small fixed worker pool | inline, deterministic |
| `Sink` | receive `Snapshot` and `Notice` from the engine | `app`: Slint + tray | collector |

Adding a wake method = new enum variant + new `WakeSender` + registry entry + wizard step;
nothing in the machine changes because it only sees `Effect::Wake(pc)` and its result.
The platform line is: core contains every adapter that works through std sockets and
processes; `app` contains everything that needs the `windows` crate.

## 7. Threading and delivery to the UI

```
 Slint event loop (UI thread)            engine thread                 worker pool (N <= 8)
 ───────────────────────────             ─────────────                 ────────────────────
 callbacks ── Command ──────mpsc──────▶  Machine::step  ── Effect ───▶  blocking I/O via ports
                                          ▲  │                              │
                                          │  └─ Snapshot / Notice           │
                                          └────────── JobDone / Observation ┘
 ◀── upgrade_in_event_loop(Snapshot) ─────┘
```

- No async runtime. All I/O is blocking with explicit timeouts on pool threads; one job per
  entity in flight bounds the thread count.
- UI to engine: `std::sync::mpsc::Sender<Command>`; handlers never block.
- Engine to UI: `Snapshot` is plain `Send + PartialEq` data (groups, tunnel views, PC views with
  state, busy flag and enabled actions, derived aggregate for the tray). The engine publishes
  only when it changed. The `Sink` adapter calls `Weak::upgrade_in_event_loop` and rebuilds the
  Slint models there (Slint models are not `Send`).
- Tray aggregate (provisional, for the designer): badge = Starting if any tunnel is Starting,
  else Up if any is Up, else Down; screen = Waking, else Online, else Offline, else Unknown
  (any, in that priority). The twelve existing `<pc>_<tunnel>` icons are reused.
  The tooltip lists one line per tunnel and PC and is truncated to the 127 characters
  Windows allows.
- Job panics are caught (`catch_unwind`) and delivered as a failed `JobDone`; the engine thread
  never runs user-supplied code. A panic hook writes to the log.
- Shutdown: `Command::Shutdown` stops the timer, drops the queue and joins workers with a
  timeout; the process then exits. A running tunnel is left as is (as in v0.3).
- UI conventions: confirmation for Sleep/Shutdown is an inline two-step button in the PC
  row, default "No"; fallback if focus behaviour is poor is `MessageBoxW` with the default
  button No, as now.

## 8. Configuration

One file, `config.<ext>`, next to the executable (as in v0.3) or `--config <path>`. Written
by the wizard atomically (temp file + rename). `version = 1` is the schema version. Unlike
v0.3, defaults exist (one `Defaults` table in code, documented), so a third-party user writes
only what differs. Illustrative TOML (the format is an owner decision, section 12):

```toml
version  = 1
language = "auto"            # auto | en | uk; auto follows the Windows UI language, fallback en

[log]
file  = "homelink.log"       # relative to the config file; rotated 1 MB x 2 backups, as now
level = "info"               # error | warn | info | debug

[defaults]                   # each key optional, overridable per PC
poll_seconds = 5
wake_poll_seconds = 2
port_timeout_ms = 1000
offline_after_failures = 3
wake_timeout = 180
power_timeout = 15

[[tunnels]]
id = "home"
name = "Home"
kind = "wireguard"
conf = 'C:\Program Files\WireGuard\Data\Configurations\home.conf.dpapi'
wireguard_exe = 'C:\Program Files\WireGuard\wireguard.exe'   # optional, default as shown
probe_host = "192.168.88.1"  # answers ping only when the tunnel works (was router_ip)
timeout = 20                 # seconds to wait for probe_host after the start
ping_timeout_ms = 1000

[[routers]]
id = "mikrotik"
kind = "mikrotik"
api_url = "http://192.168.88.1"
user = "homelink"
password_ref = "HomeLink/router/mikrotik"   # Credential Manager target, never the password
verify_tls = false
timeout = 8

[[pcs]]
id = "desktop"
name = "Desktop"
os = "linux"                 # windows | linux
host = "192.168.88.10"
tunnel = "home"              # omit for a PC without a tunnel
ports = [3389]

  [pcs.wake]                 # one of three shapes
  method = "mikrotik"        # router = "mikrotik", mac = "AA:BB:CC:DD:EE:FF", interface = "bridge"
  router = "mikrotik"
  mac = "AA:BB:CC:DD:EE:FF"
  interface = "bridge"
  # method = "magic_packet"  mac, broadcast = "255.255.255.255", port = 9
  # method = "command"       command = ["wakeonlan", "{mac}"], timeout = 15

  [pcs.power]
  method = "ssh"             # or "command" with sleep = [...], shutdown = [...]
  user = "user"
  port = 22
  identity_file = ""         # optional

  [pcs.rdp]                  # omit to hide the RDP action
  user = "user"
  options = ["use redirection server name:i:1", "authentication level:i:2", "screen mode id:i:2"]
```

The RDP file is written to `rdp/<pc id>.rdp` next to the config on first use and never
rewritten (a signed file must stay intact, as now).

Validation (all problems are collected and reported together, each with its path such as
`pcs[1].ports` and, where the parser provides it, line and column; no first-error-only):

1. Unknown keys and unknown enum values are errors (typos must not be silently ignored; this
   differs from v0.3, justified because there is no old config to stay compatible with).
2. `version` is supported; `language` is one of the shipped locales or `auto`.
3. Ids are unique within their kind and match `[a-z0-9-]{1,32}`; names are non-empty.
4. `pcs[].tunnel` and `wake.router` refer to existing ids. A tunnel or router may be unused (warning).
5. `host` is an IP address or a valid hostname; `probe_host` the same; `api_url` is `http://` or
   `https://` with a host.
6. `ports`: at least one, each 1-65535, no duplicates.
7. Numbers: `offline_after_failures >= 1`; all timeouts and intervals `> 0` and under an upper bound
   (e.g. 3600 s); `wake_timeout >= wake_poll_seconds`; `wake_poll_seconds <= poll_seconds`.
8. MAC is six hex pairs separated by `:` or `-`; magic packet `broadcast` is an IPv4 address.
9. Every `argv` is non-empty and its first element non-empty; placeholders are only
   `{host}`, `{mac}`, `{user}`; no shell strings.
10. WireGuard: `conf` file name ends with `.conf` or `.conf.dpapi`; the derived tunnel name
    matches WireGuard's `[A-Za-z0-9_=+.-]{1,32}`.
11. `power.method = "ssh"` needs `user`; `"command"` needs at least one of sleep/shutdown.
12. Secrets: `password_ref` must be non-empty; whether the secret exists is checked at use
    (and by the wizard), not at load, so the program can start before the user stores it. A
    missing secret is reported as a `WakeFailed` with a clear reason.
13. `log.file` and `rdp` paths stay inside the config directory (no `..`, no absolute paths).

Loading yields a `Config` of validated domain types (invalid states unrepresentable after
this point); the rest of core never re-validates. The engine accepts `Reload(Config)` (phase 5):
entities with unchanged ids keep their state, removed ones are dropped.

## 9. Errors and notifications

- Each boundary has its own `thiserror` enum: `ConfigError` (a list of issues), `TunnelError`,
  `WakeError`, `PowerError`, `SecretError`, `HttpError`. Adapters never panic; core denies
  `unwrap`/`expect`/indexing lints outside tests.
- Fault tolerance as in Python: a failed side effect becomes a `Notice` and a log line, the
  state is re-read from reality on the next probe, a failed probe never throws (it is
  `ready = false`), a job that panics or times out is a failed job.
- `Notice` is a typed enum, not a string key, so the compiler checks that every notice has
  text. Variants and their v0.3 keys: `TunnelFailed` (`notify.tunnel_failed`), `TunnelLost`
  (`tunnel_lost`), `TunnelTimeout` (new), `WakeFailed` (was `wol_failed`), `PcOnline`
  (`pc_online`), `PcTimeout` (`pc_timeout`), `PcLost` (`pc_lost`), `SleepSent`
  (`suspend_sent`), `ShutdownSent` (`shutdown_sent`), `PowerFailed` (`power_failed`),
  `PowerUnavailable` (`power_unavailable`). Each carries the tunnel/PC name; `detail` is cut to
  200 characters on a character boundary (`DETAIL_LIMIT`), the full text goes to the log.
- Silence rules kept: no notice for a user disconnect; none for a PC that goes off after an
  accepted power command (`power_sent`); none for a PC that becomes `Unknown` because its tunnel
  dropped (that is `TunnelLost`).
- Notice delivery in `app`: tray balloon/toast. Startup errors (invalid config, second
  instance, not elevated) use `MessageBoxW`, since Slint is not up yet.
- Elevation: v0.3 relaunches itself elevated because the WireGuard service needs admin. v1.0
  keeps `requireAdministrator` in the manifest for parity (autostart via a scheduled task stays
  as in the README); revisit later so PCs without a tunnel work unelevated.
- Single instance via a named mutex.

## 10. Internationalisation

English and Ukrainian, same mechanism as `i18n.py`: flat `key -> template` JSON files with
`{param}` placeholders, embedded with `include_str!`, language from config (`auto` reads
the Windows UI language). One catalog serves everything: Slint receives its static texts
through a `global Strings` filled from the catalog (and refilled on a language change);
notices, dialogs and wizard texts come from the same catalog. Slint's own `@tr`/gettext is not
used: it would be a second mechanism and cannot cover notices and message boxes.

Tests (run on Linux): both locales have identical key sets and identical placeholder sets per
key; every `Notice` variant and every `Strings` field has a key; no key is unused. The v0.3
`locales/*.json` are the starting point (menu entries become panel labels, new keys for
wizard, About and `tunnel_timeout`).

The About window shows the version, the licence (MIT), "Built with Slint" with a link to
https://slint.dev (required by the royalty-free licence) and the third-party licence list
(Inter is SIL OFL, already in `rust/ui/fonts/OFL.txt`; Rust crate licences are generated with
`cargo-about` and embedded).

## 11. Wizard

The wizard is a second Slint window; its logic (`core::wizard`) is pure and tested on Linux. A
`PcDraft` moves through steps; each step validates its slice with the same validators as section 8,
and the last step converts the draft to a `Pc` (plus a `Tunnel`/`Router` if created) and saves.
It also edits an existing PC.

1. Name, OS, address.
2. Tunnel: none / existing / new. New WireGuard: auto-detect `wireguard.exe` and list
   `*.conf.dpapi` from its `Data\Configurations`; ask for `probe_host`.
3. Readiness ports (presets: RDP 3389, SSH 22, custom) and "Check now" (raises the tunnel for the check if needed).
4. Wake method: none / MikroTik (pick or create router, password goes to Credential Manager) /
   magic packet / command; MAC and method fields.
5. Power: none / SSH (user, key; "Test connection" with `StrictHostKeyChecking=accept-new`
   after the user agrees, which replaces the manual first `ssh` from the README) / command.
6. RDP (optional): user and, optionally, password stored as `TERMSRV/<host>`.
7. Summary and save.

## 12. Open questions (not decided here)

1. **Waking a PC in another VLAN or subnet.** A UDP broadcast does not cross routers, and a
   routed WireGuard tunnel carries no broadcast. The MikroTik method already works for any
   segment the router itself belongs to (`interface` may be a VLAN interface). Options for the
   magic-packet method: directed broadcast to the target subnet (needs router support), a relay
   or helper on the target segment, a unicast UDP packet to an always-on device. The design
   keeps `broadcast`/`port` configurable and `WakeMethod` extensible, nothing more.
2. **Does the custom panel work well on screen?** Window focus on show and hide, hide-on-blur
   against a tray-icon click, action buttons under a mouse and keyboard, DPI, taskbar not at
   the bottom (the prototype anchors to the work-area corner), multi-monitor, accessibility.
   Needs a check on real Windows 10 LTSC and 11 in phase 0; fallbacks: `MessageBoxW`
   confirmations, a standard tray menu as a degraded mode.
3. **Panel and wizard styling.** To a designer: layout of the two columns, tunnel button states,
   PC row with four actions, wizard steps, About, and the tray icon rule for several tunnels
   and PCs (section 7 is provisional).
4. **Tunnel lifecycle with several PCs on one tunnel.** When to drop it: never automatically
   (only the toggle), when the last attached PC goes off or is told to sleep, after an idle
   timeout, or a per-tunnel setting. Until decided, the machine never drops a tunnel by
   itself, which is the v0.3 behaviour. The reference count of "wanted" PCs is not kept.
5. **Config file format (owner decision).**

   | | TOML | JSON |
   | --- | --- | --- |
   | Comments | yes | no |
   | Windows paths | literal strings `'C:\x'`, no escaping | every `\` doubled |
   | Hand editing in Notepad | friendly, no trailing-comma errors | stricter, easy to break |
   | Lists of PCs/tunnels | `[[pcs]]` tables, flat and readable | nested arrays and objects |
   | Wizard writes back | `toml_edit` keeps comments and layout (extra dependency, larger) or plain `toml` (comments lost) | `serde_json` pretty print, nothing to lose |
   | Parse errors | line and column | line and column |
   | Continuity | new format | same as v0.3 (no migration anyway) |
   | Size | about 100-250 KB depending on the crate | smaller |

   The schema in section 8 is format-neutral (serde); the leaning is TOML for comments and paths.

## 13. Testing strategy

Everything in `core` runs on Linux in CI. Goal: the logic is proven without Windows; Windows
tests prove only the thin adapters.

- **State machine, table-driven.** One test per row of section 5.2/5.3, plus every test in
  `tests/test_controller.py` ported (single failed check keeps online, one success is enough,
  pc lost after N failures, silent after a power command, silent when the tunnel drops, the
  detail limit, busy skip, shorter interval while waking, etc.), with a manual clock.
- **Multi-entity cases (new).** Two PCs on one tunnel (second `Wake` while `Starting`), PC
  without a tunnel, tunnel lost with several PCs (one `TunnelLost`, no `PcLost`), pending wakes
  cleared on failure.
- **Property tests** (`proptest`) over random event sequences: a tunnel PC is never `Online`
  or `Waking` while its tunnel is not `Up`; `Online -> Offline` needs `N` consecutive failures;
  `PcLost` never follows an accepted power command; no notice without a transition cause.
- **Config:** valid and invalid fixture files; every rule of section 8 has a failing case with the
  expected issue path; the documented example is parsed in a test; round trip load-save-load.
- **Adapters:** magic packet bytes (6 x `FF` + 16 x MAC) and a loopback socket; MikroTik request
  (path `/rest/tool/wol`, JSON body, basic auth, error status mapping) against a local mock
  server; TCP probe against a local listener; command runner exit codes, timeout and output.
- **Engine:** the real engine with an inline `Executor`, `Clock` and fakes: end-to-end
  scenarios (connect-and-wake to online, wake timeout, shared tunnel) asserting snapshots and
  notices in order.
- **i18n and wizard:** parity tests (section 10), step-by-step wizard tests.
- **Windows only (CI `windows-latest`):** Credential Manager round trip with a unique
  target, SCM query of a non-existing service, ICMP to loopback, OEM decoding; UI smoke with the
  Slint testing backend (component instantiation, callbacks reach the engine).
- **Manual release checklist** (in the repo, run on Windows 10 LTSC and 11): tray, panel focus,
  notifications, real WireGuard, MikroTik, a Windows and a Linux PC, parity with 0.3.0.
- **Gates:** `cargo llvm-cov` on `core` with a threshold (machine and config near 100 %),
  `cargo mutants` on `machine/` and `config/` run manually before releases.

## 14. CI and release changes

Current: `.github/workflows/ci.yml` and `release.yml`, both Python on `windows-latest`, tag
must equal `homelink.__version__`. Changes (the Python jobs stay until phase 7):

- `ci.yml` job `core` (ubuntu-latest): `cargo fmt --check`, `cargo clippy -p homelink-core
  --all-targets --all-features -D warnings`, `cargo test -p homelink-core --all-features`,
  `cargo doc` with warnings denied, coverage with threshold, `cargo deny check` (licences,
  advisories, bans, sources), locale parity.
- `ci.yml` job `app` (windows-latest): fmt, clippy `-D warnings` and tests for the whole workspace, release
  build with `--locked`, size-budget check, smoke run (`HomeLink.exe --version`, `--check-config`
  on the example), upload artifact.
- Python jobs get `paths:` filters (`src/**`, `tests/**`, `pyproject.toml`) so they stop
  running for Rust-only changes; both are deleted in phase 7.
- `release.yml`: tag `v*` must equal the workspace version in `rust/Cargo.toml`; same
  gates as CI; build `HomeLink.exe` (name unchanged, so the README download links keep
  working); generate third-party licences with `cargo-about`; publish a SHA-256 file; create
  the release. Pre-releases (`v1.0.0-rc.N`) are marked as such.
- `rust-cache` for `target/`; `rust-toolchain.toml` pins the compiler, a scheduled job tests
  the newest stable. Dependabot (cargo, github-actions) is proposed; this is new CI
  configuration, to be added only with the owner's approval.
- Workflow files are not edited in this stage.

## 15. Roadmap to v1.0.0

| Phase | Content | Exit criterion |
| --- | --- | --- |
| 0 Spike | Workspace skeleton, CI for `core` and `app`; on real Windows: panel focus/placement, tray notifications API (Slint's tray may not offer balloons, a `windows`-crate fallback may be needed), measured exe size; owner answers section 12 | questions 2 and 5 answered, size budget fixed, risks below closed or accepted |
| 1 Core model | `domain`, `config` (schema, defaults, validation, fixtures), `secret`, `i18n` catalog, `notice` | config tests green on Linux; locale parity green |
| 2 Machine and engine | `machine`, `engine`, `ports`, `testing` fakes; port all of `test_controller.py`; property tests | every Python controller test has a Rust counterpart; coverage gate on |
| 3 Adapters | Portable (TCP, UDP magic packet, commands, MikroTik REST) with Linux integration tests; Windows (WireGuard service, Credential Manager, ICMP, OEM decoding) | adapter tests green in both CI jobs |
| 4 App shell | Panel on the real engine, tray with aggregate icon, notices, logging, single instance, elevation manifest, icons moved from `src/homelink/assets` | a single PC with a tunnel works end to end on Windows |
| 5 Multi-PC and wizard | Groups, shared tunnel, PC without a tunnel, wizard (add/edit/delete), About with the Slint credit, config reload | tunnel groups, a shared tunnel and a PC without a tunnel work by hand |
| 6 Hardening | Manual checklist on Windows 10 LTSC and 11 (Linux and Windows remote PCs), docs rewrite (`README*.md`, `design.md`, router setup, SSH for Windows), size and licence check, `v1.0.0-rc.1` | no open defects; parity checklist complete |
| 7 Release | Remove `src/homelink/`, `tests/`, `pyproject.toml`, `build.bat`, `run_homelink.pyw`, Python CI jobs; decide the future of `tools/generate_icons.py` (keep as a dev tool or port); tag `v1.0.0` | release published, README links verified |

Technical risks to verify in phase 0: tray notifications in Slint 1.18; position of the tray
icon (the prototype assumes a bottom-right taskbar); Credential Manager scope when UAC elevates
under a different account than the logged-in user; `ureq`/rustls size against the budget;
`unstable-winit-030` is an unstable Slint feature (pin the exact Slint version).

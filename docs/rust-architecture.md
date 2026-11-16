# HomeLink 1.0: architecture of the Rust rewrite

It describes the rewrite of the
Python 0.3.0 tray utility in Rust with Slint 1.18, released as v1.0.0 from the same
repository. The Python code is archived in `legacy/python/` (`src/homelink/`, `tests/`);
references such as `controller.py` point to it, and `legacy/python/docs/design.md` describes
the Python-era UI.

## 1. Goals and settled decisions

- Rust + Slint 1.18 (no WebView2: Windows 10 LTSC must work). Slint only for the GUI, so the
  executable stays small.
- Several remote PCs, each optionally behind a tunnel. A tunnel may serve several PCs; PCs
  without a tunnel form their own group. Old `config.json` files are not migrated.
- The configuration file is TOML (final): readable, with comments. Saving never loses what
  the user wrote by hand: the file is edited by key, not re-rendered (section 8).
- Tunnel lifecycle (final): the program never drops a tunnel by itself; only the tunnel
  button does. A tunnel that disappeared on its own could cut an RDP session or a file copy
  that the program does not know about. So there is no reference counting of "wanted" PCs, no
  idle timeout and no automatic disconnect when the last PC goes off or is put to sleep; a PC
  going offline never changes its tunnel.
- One command raises the tunnel and wakes an inactive PC.
- Wake methods per PC: through a relay host over SSH (a preset picks the command: RouterOS
  `/tool wol`, `etherwake` or `wakeonlan`), a direct magic packet, or an arbitrary command;
  more presets and methods later. No REST API is used anywhere.
- Waking a PC in another subnet or VLAN (final): a relay host in the PC's own network sends
  the packet (the relay is any always-on device there, reached over SSH like everything
  else), or the program sends a unicast magic packet to the PC's IP with a static ARP entry
  on the last router (section 3). Directed subnet broadcast is not a separate method: it is the
  direct magic packet with a broadcast address, and it works only if the router forwards
  directed broadcasts.
- Remote PCs may run Windows or Linux.
- A wizard adds a PC. The program holds no passwords or other secrets: relay hosts and PCs are
  reached over SSH with keys through the system OpenSSH client, and the Remote Desktop
  password is typed into `mstsc`, never into HomeLink. So "HomeLink stores no secrets and
  talks only to the hosts in its config" is unconditional (section 4).
- Readiness = all listed TCP ports open. An online PC turns "off" only after
  `offline_after_failures` consecutive failed checks; one success means online.
- Third-party users: tests, lint and CI are part of the deliverable. Own visual style
  (sharp corners, embedded Inter). Slint's royalty-free licence requires crediting Slint in
  the About window.
- The Python code is not deleted: since 15.11.2026 it is archived in `legacy/python/` (no CI,
  not developed), and `main` carries the Rust version (owner decision).

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
      snapshot.rs       what the engine publishes to the UI
      ports.rs          traits that isolate side effects (section 6)
      ssh.rs            the shared `ssh` argument list and the PC power commands
      wake.rs           wake sender that runs `ssh` through `CommandRunner` (argv building
                        is pure)
      adapters/         portable adapters, std only: `tcp` (port probe), `magic` (magic
                        packet), `command` (command runner, command wake), `power` (sleep and
                        shutdown over SSH or by command)
      wizard.rs         wizard steps and draft-to-config conversion (pure logic)
      notice.rs, i18n.rs, rdp.rs
      testing.rs        fakes behind the `testing` feature
    locales/            en.json, uk.json
    tests/              integration tests with local servers and fixtures
  app/                  crate `homelink` (bin, produces HomeLink.exe): Windows + Slint
    src/
      main.rs           arguments (--demo, --lang, --config), mode choice, the panel run
      real.rs           real mode: elevation, config.toml (template on the first start),
                        single instance, log, `Deps` from the real adapters
      windows/          adapters built from pure, unit-tested functions and a thin layer that
                        runs programs through `CommandRunner`: WireGuard service (`sc`,
                        `wireguard.exe`), ping, OEM output decoding, `reg query` (code page,
                        display language), `.rdp` file and `mstsc`, elevation check/restart
      logging.rs        rotating log file (local time), instance.rs   file lock held by the
                        running copy
      demo.rs           (feature `demo`) the engine on the core fakes with simulated delays
      ui.rs             Slint glue: snapshot -> models, callbacks -> commands, tray, notices
      platform.rs       the only unsafe code: work area, message box (cfg(windows))
    ui/                 *.slint (panel, wizard, about), fonts/, icons/
    build.rs
```

Dependency rule: `app -> core`, never the reverse. `core` depends only on std, `serde`, a
`toml_edit` (the only TOML crate: parsing with error positions, writing and in-place editing),
`thiserror` and `log` (plus `serde_json` for the locale catalogs). It has no
HTTP or TLS client. It builds and tests on Linux; only `app` needs Windows.

Size policy: no `tokio`, no `reqwest`, no gettext, no extra GUI crates. Release profile
with `lto = "fat"`, `codegen-units = 1`, `opt-level = "s"`, `strip = true`, panic = unwind
(worker panics are caught, section 7). CI enforces a size budget on `HomeLink.exe`.

## 3. Domain model

Type sketches only; fields mirror section 8.

```
TunnelId, PcId, RelayId         validated slugs [a-z0-9-]{1,32}

Tunnel      { id, name, kind: TunnelKind, probe_host, timeout, ping_timeout }
TunnelKind  = WireGuardService { exe, conf }          // #[non_exhaustive]: OpenVPN etc. later
Relay       { id, kind: RelayKind, host, port, user, identity_file, timeout }   // SSH, key only
RelayKind   = RouterOs | Etherwake | Wakeonlan        // preset: the wake command; #[non_exhaustive]

Pc          { id, name, os: Os, host, tunnel: Option<TunnelId>, ports: NonEmpty<u16>,
              wake: Option<WakeMethod>, power: Option<PowerMethod>, rdp: Option<Rdp>,
              timing: Timing }
Os          = Windows | Linux
WakeMethod  = Relay        { relay, mac, interface: Option<InterfaceName>,
                             address: Option<Ipv4Addr> }     // #[non_exhaustive]
            | MagicPacket  { mac, address: Ipv4Addr, port }  // UDP; any IPv4 address, e.g. 255.255.255.255:9
            | Command      { argv, mac: Option<Mac>, timeout }
PowerAction = Sleep | Shutdown
PowerMethod = Ssh     { user, port, identity_file }   // system ssh client, key required
            | Command { sleep: Option<argv>, shutdown: Option<argv> }
Rdp         { user, options }
Timing      { poll, wake_poll, port_timeout, offline_after_failures, wake_timeout,
              power_timeout }
```

`argv` is always a list, never a shell string, so there is no quoting or injection issue.
Placeholders `{host}` and (when the wake command has a `mac`) `{mac}` are substituted per
element. SSH presets by `Os`:

| Os | Shutdown | Sleep |
| --- | --- | --- |
| Linux | `sudo systemctl poweroff` | `sudo systemctl suspend -i` (as in README: `-i` ignores inhibitors) |
| Windows | `shutdown /s /t 0` | `rundll32.exe powrprof.dll,SetSuspendState 0,1,0` |

Windows sleep through `SetSuspendState` hibernates when hibernation is enabled; the wizard
says so. A `Command` power method covers anything else.

An SSH power action runs the same kind of argument list as the relay wake below, built by
`core::ssh::power_ssh_argv` from the PC's `user`, `port`, `identity_file` and `host`, with
the remote command from the table above as one argument and `ConnectTimeout` taken from
`power_timeout`. The key is always passed with `-i <identity_file>` and `IdentitiesOnly=yes`
(an explicit absolute path: the program runs elevated and must not depend on which profile
`ssh` resolves), exactly as for relays; there is no mode in which `ssh` chooses the key.

**Waking through a relay host.** A relay is a router or any always-on device in the PC's own
network (a router, an OpenWrt box, a Raspberry Pi). The program connects to it over SSH and
runs a wake command chosen by the relay's *preset* (`kind` in `[[relays]]`). The sender runs,
through `CommandRunner`, the system client with this argument list (never a shell string):

```
ssh -o BatchMode=yes -o PasswordAuthentication=no -o IdentitiesOnly=yes
    -o StrictHostKeyChecking=yes -o ConnectTimeout=<relay timeout>
    -i <identity_file> -p <port> -l <user> -- <host>
    "<remote command>"
```

| Preset | Remote command | `interface` in the wake method | `address` | `sudo` in the relay |
| --- | --- | --- | --- | --- |
| `routeros` | `/tool wol mac=<MAC> interface=<interface>` | required | not used | rejected |
| `etherwake` | `[sudo -n ]etherwake -i <interface> <MAC>` (typical for OpenWrt) | required | not used | required |
| `wakeonlan` | `[sudo -n ]wakeonlan <MAC>`, or `[sudo -n ]wakeonlan -i <address> <MAC>` | not used | optional (IPv4) | required |

`sudo` is a boolean on the relay entry (`sudo = true`), with no default: `etherwake` and
`wakeonlan` may need root on the relay, so the configuration must say so; RouterOS has no
`sudo` and the key is rejected there. When it is true the remote command is prefixed with the
fixed text `sudo -n ` (`-n`: never ask for a password, so a missing `sudoers` rule fails at once
instead of hanging, in line with `BatchMode=yes`).

Validation checks the wake method against the preset of the relay it names: a field the
preset needs must be present and a field it does not use is rejected, each with the path
(`pcs[0].wake.interface`, `relays[1].sudo`). The destination `address` of `wakeonlan` reuses
the same name and meaning as in the direct magic packet. Adding a preset is one new
`RelayKind` variant plus an arm in each `match` on it (`code`, `uses_interface`,
`uses_address`, `supports_sudo` in `domain`, and `relay_command` in `core::wake`); the compiler
lists every place, and the exact-argument test set gets one more case.

`<MAC>` is a parsed MAC address rendered as six upper-case hex pairs; `<interface>` is an
`InterfaceName` (`A-Za-z0-9_.-`, 1-64 characters, not starting with `-`); `<address>` is a
parsed `Ipv4Addr`. None of them can carry a space, quote, `;`, `$`, `[`, `]`, `\`, `=` or a
leading `-` (an option), so a configured value cannot add RouterOS or shell syntax or an
option. The remote command is one argument. The key is an explicit absolute path (the program
runs elevated and must not depend on which profile `ssh` would resolve); `IdentitiesOnly`
stops `ssh` from offering other keys. The host key must already be known
(`StrictHostKeyChecking=yes`): the wizard is the only place that may accept a new one
(section 4). Argument building is pure and unit-tested in `core::wake` and `core::ssh` (exact
argument lists for every preset); the process itself is started by `StdCommandRunner`
(`core::adapters::command`).

*Failures are told apart* (`ssh::explain_ssh`), for wake and for SSH power actions alike, so
the notice says what to fix: the OpenSSH client is missing or cannot start ("cannot run ssh (is
the OpenSSH client installed?)"); `ssh` timed out; `ssh` itself failed with exit code 255
(connection refused, host key or key authentication failure); or the command ran on the remote
host and failed (any other non-zero code, with its output). Commands that are not `ssh` map to
the same set without the `ssh` wording (`explain_command`).

**Waking across a routed network or a VLAN (direct magic packet).** The direct method sends a
UDP magic packet from this computer to a configurable destination `address` (any IPv4
address) and port (default 9). Two uses, the same method:

- *On the same network:* `address` is the broadcast address (`255.255.255.255` or the subnet's
  `x.y.z.255`).
- *Routed, through the tunnel or across VLANs:* `address` is the **PC's own IP**, so the
  packet is an ordinary routed unicast UDP datagram and crosses the WireGuard tunnel and
  routers like any other traffic. The sleeping PC cannot answer ARP, so the **last router**
  (the one on the PC's network) needs a **static ARP entry** for the PC's IP and MAC; then it
  delivers the datagram to the PC's MAC. Examples: on RouterOS
  `/ip arp add address=<pc-ip> mac-address=<pc-mac> interface=<lan-interface>`; on Linux
  `ip neigh replace <pc-ip> lladdr <pc-mac> dev <lan-interface> nud permanent`. The firewall
  must allow UDP to the chosen port.

A directed subnet broadcast (`address = x.y.z.255` of a remote subnet) is not a separate
method; it is this method with that address, and it works only if the router in front of that
subnet forwards directed broadcasts (a router-specific setting; see its documentation). The
unicast and static ARP setup above is the recommended way. The wizard shows this text when the
user picks a destination (`wizard.magic.routed_help`).

States (as in `state.py`, now per entity):

```
TunnelState = Down | Starting | Up
PcState     = Unknown | Offline | Waking | Online
```

A PC with a tunnel is `Unknown` whenever its tunnel is not `Up`. A PC without a tunnel is
probed directly and is `Unknown` only until its first probe.

Commands (UI to engine): `ToggleTunnel(id)`, `Wake(pc)`, `Power(pc, action)` are machine
commands; stopping is `Handle::stop`; `OpenRdp(pc)` is handled by the app (it only launches
`mstsc`); a new configuration is applied with `Handle::reload(config)` (section 8). Semantics of `Wake(pc)`: tunnel PC with the
tunnel not `Up` means connect-and-wake (the old `connect_and_wake`); otherwise send the
wake request. Ignored while the PC is `Online` or `Waking`.

Action availability is computed in core and shipped in the snapshot, so the UI holds no
rules: Wake needs a wake method and a PC not `Online`/`Waking`; Sleep, Shutdown and RDP need
an `Online` PC and the matching configuration (same rules as `app.py` `can_wake` and
`pc_online`).

## 4. Privacy and security

HomeLink stores no secrets and talks only to the hosts in its config. This holds without
exceptions, by construction:

- **No secrets.** There is no password, token or key material in the configuration, in the
  Windows Credential Manager (not used at all) or anywhere else the program controls. Relay hosts and PCs are reached over SSH with key authentication through the system
  OpenSSH client (`BatchMode=yes`, `PasswordAuthentication=no`); the private key stays in a
  file the user owns, referenced by an explicit absolute path. The program never reads it.
- **Remote Desktop.** HomeLink never handles the RDP password. The user types it in `mstsc`,
  which may save it itself. The program only writes the `.rdp` file (host, user name, options)
  and launches `mstsc`.
- **No telemetry, no update checks, no crash uploads.** The program makes no network
  connection of its own besides the ones the user configured: TCP checks of the listed ports,
  pings of the tunnel probe host, UDP magic packets, SSH to relay hosts and PCs, and the commands
  the user wrote. There is no HTTP or TLS client in the program.
- **SSH host keys are verified.** `StrictHostKeyChecking=yes` everywhere at run time. The wizard
  may add a host key (fetched with `ssh-keyscan`, appended to `known_hosts` itself; `accept-new`
  is never used) only after showing the fingerprint and receiving explicit consent from the
  user; nothing else ever accepts a key, and an existing entry is never replaced. Known hosts are read from the
  profile `ssh` resolves for the (elevated) user; the wizard runs in the same process, so the
  two agree.
- **Nothing sensitive in logs.** Logs hold states, ids, host names and error texts, never key
  contents or command output beyond the error text shown in a notice. The log file is local.
- **Supply chain.** `cargo-deny` checks licences, advisories and sources in CI; `Cargo.lock`
  is committed and builds use `--locked`; releases are built by GitHub Actions from the
  public source (section 14).

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
operations are serialised per entity: at most one in-flight *action* (tunnel start or stop;
PC connect check, wake or power command) per tunnel and per PC, a second command for the same
entity is dropped with a log line (as `busy.acquire(blocking=False)`). A passive probe may run
alongside an action so that a click is never lost behind a slow probe; its result is ignored
while the action runs (as `refresh()` was skipped while `busy`).

### 5.2 Tunnel transitions

Per tunnel. `expect_down` is set by a user disconnect (the old `user_stopped`).
`pending` is the set of PCs waiting for this tunnel in connect-and-wake.

| # | From | Event | To | Effects / notices | controller.py |
| --- | --- | --- | --- | --- | --- |
| T1 | Down | `ToggleTunnel` or `Wake(pc)` | Starting | job `TunnelUp`; deadline = now + timeout; `pending += pc`; **failure cleared (F1)** | `_start_tunnel` |
| T2 | Starting | `Wake(pc)` | Starting | `pending += pc` | busy gate |
| T3 | Up | `Wake(pc)` | Up | probe PC (see P4) | `connect_and_wake` |
| T4 | Starting/Up | `ToggleTunnel` | (observed next) | job `TunnelDown`; `expect_down = true`; `pending = {}`; **failure cleared (F1)** | `toggle_tunnel` |
| T5 | Starting | `JobDone(TunnelUp, Err)` | observed next | notice `TunnelFailed(detail)`; `pending = {}`; probe at once; **start failure (F2)** | `_start_tunnel` |
| T6 | Starting (deadline open) | probe: probe host answers | Up | start PC probes for `pending` and attached PCs; **start failure cleared (F5)** | `_start_tunnel` loop |
| T7 | Starting (deadline passed) | still silent | Starting | notice `TunnelTimeout`; `pending = {}`; deadline-less polling goes on (T9); **start failure (F3)** | `_start_tunnel` end |
| T8 | any | probe: service not running | Down | attached PCs to `Unknown`, clear `waking_until` and failure counters (**and their failures, F13**); if previous state was `Up` and not `expect_down`: notice `TunnelLost`; clear `expect_down`; **stop failure cleared (F6)** | `_refresh` first branch |
| T9 | any | probe: service running, probe host silent | Starting | attached PCs to `Unknown`, counters cleared (**and failures, F13**); no notice | `_refresh` second branch |
| T10 | any | probe: probe host answers | Up | start attached PCs' probes; **start failure cleared (F5)** | `_refresh` rest |
| T11 | Starting/Up | `JobDone(TunnelDown, Err)` | (observed next) | notice `TunnelStopFailed(detail)`; probe at once; **stop failure (F4)** | (new: v0.3 only logged it) |

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
| P1 | any | `ready` | Online | `failures = 0`, `waking_until` cleared; if was `Waking`: notice `PcOnline`; **wake failure cleared (F11)** | `_refresh` |
| P2 | Online | not ready, `failures` after the increment `< N` | Online | `failures += 1`, logged | `_refresh` |
| P3 | Waking | not ready, `now < waking_until` | Waking | `failures += 1` | `_refresh` |
| P4 | Unknown/Offline/Online/Waking after deadline | not ready | Offline | `failures += 1`; if was `Online` and not `power_sent`: `PcLost`; if was `Waking`: `PcTimeout` and **wake failure (F9)**; if was `Online`: **power failure cleared (F12)** | `_refresh` |
| P5 | any | tunnel T8/T9 | Unknown | `failures = 0`, `waking_until` cleared, no `PcLost`; **failure cleared (F13)** | `_refresh` |
| P6 | any | connect-and-wake probe: `ready` | Online | `failures = 0`; no `PcOnline` | `connect_and_wake` |
| P7 | any | connect-and-wake probe: not ready | (unchanged) | job `Wake` | `connect_and_wake` |
| P8 | any | `JobDone(Wake, Ok)` | Waking | `waking_until = now + wake_timeout` | `_send_wol` |
| P9 | any | `JobDone(Wake, Err)` | (unchanged) | notice `WakeFailed(detail)`; **wake failure (F8)** | `_send_wol` |
| P10 | Online | `Power(a)` | (unchanged) | job `Power(a)`; **failure cleared (F7)** | `_power` |
| P11 | other | `Power(a)` | (unchanged) | notice `PowerUnavailable` | `_power` |
| P12 | any | `JobDone(Power, exit 0)` | (unchanged) | `power_sent = true`; notice `SleepSent`/`ShutdownSent` | `_power` |
| P13 | any | `JobDone(Power, failure)` | (unchanged) | notice `PowerFailed(detail)`; **power failure (F10)** | `_power` |

`Wake(pc)` that is taken up (P7 or T1-T3 path, i.e. not dropped by a guard) also clears the PC's
failure (F7).

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
3. `wol_failed` becomes `wake_failed` because wake is no longer relay-only (the relay is now
   reached over SSH, so its detail text is `ssh` output or an exit code, not an HTTP status).
4. Connect-and-wake on an `Up` tunnel does not flip the tunnel to `Starting` for an instant.
5. T11: a failed tunnel stop (v0.3 logged it) now produces a notice `TunnelStopFailed` and a
   failed state (section 5.6).

Everything else (rules P1-P13, T8-T10, the 200 character detail limit, silence for user
disconnects and for PCs that were told to power off) is kept.

### 5.6 Failed state, display state and check times

**Failed.** An entity (a tunnel or a PC) is *failed* when its last requested action ended in an
error: a tunnel start or stop failed or the start timed out; a wake request failed or the wake
timeout expired; a sleep or shutdown command failed. The failure keeps the notice that was shown
for it (`last_error`) and lasts until the entity is *seen in the situation the failed request was
meant to produce*, or a new request is taken up. A probe that merely completes is not enough: T5
and T7 are followed by an immediate probe that finds the tunnel still down, and a failed power
command leaves the PC online and polled every few seconds, so ending the failure at the next
probe would hide it at once. Failures are independent of the state (`TunnelState`/`PcState`
are unchanged by them).

| # | Entity | Event | Effect on the failure |
| --- | --- | --- | --- |
| F1 | tunnel | a toggle or connect-and-wake is taken up (T1, T4) | cleared |
| F2 | tunnel | T5: start job failed | *start failure*, notice `TunnelFailed` |
| F3 | tunnel | T7: start timed out | *start failure*, notice `TunnelTimeout` |
| F4 | tunnel | T11: stop job failed (also a panicked job) | *stop failure*, notice `TunnelStopFailed` |
| F5 | tunnel | probe sees the host answer (T6, T10) | a start failure is cleared; a stop failure stays |
| F6 | tunnel | full probe sees the service not running (T8) | a stop failure is cleared; a start failure stays |
| F7 | PC | a `Wake` or `Power` is taken up (not dropped by a guard) | cleared |
| F8 | PC | P9: wake request failed (also a panicked job) | *wake failure*, notice `WakeFailed` |
| F9 | PC | P4: `Waking` and the wake timeout expired | *wake failure*, notice `PcTimeout` |
| F10 | PC | P13: power command failed (also a panicked job) | *power failure*, notice `PowerFailed` |
| F11 | PC | P1: ports answer | a wake failure is cleared; a power failure stays (the PC is simply still online) |
| F12 | PC | P4 from `Online`: the PC stopped answering | a power failure is cleared |
| F13 | PC | its tunnel was reset (P5) | cleared (the state is unknown again) |

A PC that waits for its tunnel (`pending`) does not inherit a tunnel failure: its wake was never
sent. The failure of the tunnel is shown on the tunnel.

**Display state.** Both kinds of entity are published with one `DisplayState`
(`ok | busy | off | unknown | fail`), the first that applies:

| | Tunnel | PC |
| --- | --- | --- |
| `fail` | failed | failed |
| `busy` | `Starting` | an action is running (check, wake, power), it waits for its tunnel, or `Waking` |
| `ok` | `Up` | `Online` |
| `off` | `Down`, checked at least once | `Offline` |
| `unknown` | `Down`, never checked | `Unknown` |

**Check times.** `last_checked` is the time of the last probe result that counted (one that was
applied, not one ignored while an action runs or because the tunnel is not up). It is published
rounded down to 30 s (`CHECK_STEP`): with the exact time every idle poll would change the
snapshot and be published. The value is a time of the engine's `Clock`; the UI turns it into an
age with the same clock.


## 6. Traits that isolate side effects (`core/src/ports.rs`)

| Trait | Purpose | Production impl | Test impl |
| --- | --- | --- | --- |
| `TunnelDriver` | `is_running`, `up`, `down` for a `Tunnel` | `app`: SCM query + `wireguard.exe /installtunnelservice` / `/uninstalltunnelservice` (replaces psutil; "already installed" still tolerated) | scripted fake |
| `Pinger` | ICMP echo to a host with timeout | `app`: `IcmpSendEcho` (no console window, no locale parsing) | set of reachable hosts |
| `PortProbe` | all ports of a host accept TCP | `core::adapters::tcp::TcpPortProbe` (done): ports tried concurrently under one shared deadline, names resolved in a helper thread, connect and close, nothing sent | local listeners, an unused port |
| `WakeSender` | send one wake request for a `Pc` | one impl per method: `RelayWake` (`ssh` through `CommandRunner`, `core::wake`), `MagicPacketWake` (UDP to any IPv4 address; `SO_BROADCAST` only if the system refuses without it), `CommandWake` (all done, in `core::adapters`; `default_wake_registry` registers them); selected through a `WakeRegistry` keyed by method kind | recording fake; real UDP socket on loopback |
| `CommandRunner` | run argv without a shell, timeout; returns a `CommandOutput` (outcome: exited with a code, timed out, not started; bounded stderr + stdout text) | `core::adapters::command::StdCommandRunner` (done): own process group killed on timeout on Unix (the group is killed with `kill -KILL -- -<pid>`, no dependency; only the program itself where `kill` is missing and on Windows), `CREATE_NO_WINDOW` on Windows, pluggable output decoder (`app` supplies the OEM code page one), `ETXTBSY` retried | recording fake; `sh`, `sleep`, a fake `ssh` script |
| `PowerRunner` | run `Sleep`/`Shutdown` for a `PowerMethod` | `core::adapters::power::CommandPowerRunner` (done): SSH preset or configured command through `CommandRunner`, failures told apart (section 3) | recording fake; a fake `ssh` script |
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
- Engine to UI: `Snapshot` is plain `Send + PartialEq` data: groups, tunnel views and PC views.
  Each view carries the state, the display state (`ok | busy | off | unknown | fail`, section
  5.6), the enabled actions, the time of the last check, the last error (a `Notice`, worded by
  the UI's `Translator`) and the facts the details window shows (PC: address, tunnel name, wake
  summary, ports, which actions are configured at all; tunnel: kind, names of its PCs, what the
  toggle would do). There is a derived aggregate for the tray. The UI holds no rules: it maps
  these values to colours, words and labels. The engine publishes only when the snapshot
  changed (check times are rounded to 30 s for this reason, section 5.6). The `Sink` adapter
  calls `Weak::upgrade_in_event_loop` and rebuilds the Slint models there (Slint models are not
  `Send`).
- Tray aggregate (`TrayState`). The monitor glyph reads as "the PC", so the screen fill is the
  aggregate of the PCs' display states, the first that applies: any failed (`fail`), any busy
  (waking or running an action, `busy`), any online (`ok`), all known and off (`off`),
  otherwise `unknown` (including no PCs at all, so no fill). The corner square is the aggregate
  of the tunnels (`TunnelCorner`): `Fail` if any tunnel failed, else `Busy` if any is
  connecting, else `Ok` if any is up, else none (all stopped or unknown, or no tunnels). This
  deliberately deviates from section 9 of the handoff (screen = tunnels, corner = PC attention)
  by the owner's decision of 13.11.2026. Tooltip: `tooltip.tunnels_up` (tunnels up of all
  tunnels) and `tooltip.pcs_online` (PCs online of all PCs); when some PC's state is unknown the
  second line is `tooltip.pcs_online_unknown`, which adds the count of unknown PCs. The tooltip
  is truncated to the 127 characters Windows allows.
- Job panics are caught (`catch_unwind`) and delivered as a failed `JobDone`; the engine thread
  never runs user-supplied code. A panic hook writes to the log.
- Shutdown: `Command::Shutdown` stops the timer, drops the queue and joins workers with a
  timeout; the process then exits. A running tunnel is left as is (as in v0.3).
- UI conventions: confirmation for Sleep/Shutdown is an inline two-step button in the PC
  row, default "No".

## 8. Configuration

One file, `config.toml`, next to the executable (as in v0.3) or `--config <path>`. Written
by the wizard atomically (temp file + rename). `version = 1` is the schema version. Unlike
v0.3 a required `[defaults]` table holds the shared timing, and a PC may override single keys
in `[pcs.timing]`; there are no built-in values for user settings in code (only technical
limits). An example:

```toml
version  = 1
language = "auto"            # auto | en | uk; auto follows the Windows UI language, fallback en

[log]
file  = "homelink.log"       # relative to the config file; rotated 1 MB x 2 backups, as now
level = "info"               # error | warn | info | debug

[defaults]                   # every key required; a PC may override them in [pcs.timing]
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
wireguard_exe = 'C:\Program Files\WireGuard\wireguard.exe'
probe_host = "192.168.88.1"  # answers ping only when the tunnel works (was router_ip)
timeout = 20                 # seconds to wait for probe_host after the start
ping_timeout_ms = 1000

[[relays]]                   # a host that sends the magic packet; SSH with a key, no password
id = "gateway"
kind = "routeros"            # preset: routeros | etherwake | wakeonlan
host = "192.168.88.1"
port = 22
user = "homelink"            # a restricted user that may only run the wake command
identity_file = 'C:\ProgramData\HomeLink\relay_ed25519'    # explicit absolute path, required
timeout = 8                  # seconds, also the ssh connect timeout
# sudo = true                # etherwake and wakeonlan: required (run as `sudo -n <command>`);
                             # routeros: not allowed

[[pcs]]
id = "desktop"
name = "Desktop"
os = "linux"                 # windows | linux
host = "192.168.88.10"
tunnel = "home"              # omit for a PC without a tunnel
ports = [3389]

  [pcs.wake]                 # one of three shapes
  method = "relay"           # through a relay host; the relay's preset decides the fields
  relay = "gateway"
  mac = "AA:BB:CC:DD:EE:FF"
  interface = "bridge"       # routeros and etherwake: required; wakeonlan: not allowed
  # address = "10.0.5.255"   # wakeonlan only, optional
  # method = "magic_packet"  mac, address = "255.255.255.255" (broadcast, or the PC's own IP
  #                          for a routed setup), port = 9
  # method = "command"       command = ["wakeonlan", "{mac}"], timeout = 15

  [pcs.power]
  method = "ssh"             # or "command" with sleep = [...], shutdown = [...]
  user = "user"
  port = 22
  identity_file = 'C:\ProgramData\HomeLink\desktop_ed25519'   # required, absolute path

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
4. `pcs[].tunnel` and `wake.relay` refer to existing ids. A tunnel or relay may be unused (warning).
5. `host` (PC and relay) is an IP address or a valid hostname; `probe_host` the same.
6. `ports`: at least one, each 1-65535, no duplicates.
7. Numbers: `offline_after_failures >= 1`; all timeouts and intervals `> 0` and under an upper bound
   (e.g. 3600 s); `wake_timeout >= wake_poll_seconds`; `wake_poll_seconds <= poll_seconds`.
8. MAC is six hex pairs separated by `:` or `-`; the `address` of a magic packet or of a
   `wakeonlan` relay wake is an IPv4 address (unicast or broadcast, nothing else is assumed);
   an `interface` is 1-64 characters from `A-Za-z0-9_.-` and does not start with `-` (it is
   placed into a command run on the relay, so no other character is accepted).
   Relay wake methods are checked against the relay's preset: `interface` is required by
   `routeros` and `etherwake` and rejected for `wakeonlan`; `address` is allowed only for
   `wakeonlan`. The error names the field, e.g. `pcs[0].wake.interface`.
9. Every `argv` is non-empty and its first element non-empty; placeholders are only `{host}`
   and, when the wake command has a `mac`, `{mac}`; no shell strings.
10. WireGuard: `conf` file name ends with `.conf` or `.conf.dpapi`; the derived tunnel name
    matches WireGuard's `[A-Za-z0-9_=+.-]{1,32}`.
11. `power.method = "ssh"` needs `user`, `port` and `identity_file`; `"command"` needs at least
    one of sleep/shutdown. An SSH user (power and relay) has no blanks or `@` and does not
    start with `-`.
12. SSH access (relays and SSH power methods): `port` 1-65535; `identity_file` is required and
    must be an absolute path (`C:\...`, `C:/...`, `\\server\...` or `/...`), so the elevated
    program never depends on which profile `ssh` resolves; the error names the path, for
    example `pcs[0].power.identity_file`. Keys that held REST or password settings (`api_url`,
    `password_ref`, `verify_tls`, `password`), and the old names `[[routers]]`, `router`,
    `broadcast` and `method = "mikrotik"`, are unknown keys and rejected (nothing is released
    with them, so there are no aliases). The file is not opened at load; a missing key file
    surfaces as an `ssh` error in a notice.
13. `log.file` and `rdp` paths stay inside the config directory (no `..`, no absolute paths).

**Saving.** `save` never re-renders an existing file; it edits it by key (`toml_edit`, only in
`config/format.rs`):

- Comments, blank lines, key order and formatting of everything that did not change are kept
  byte for byte. A table, array or value that means the same as the new one is not touched
  at all (an inline table equals the same content written as a table; `AA-BB-…` equals
  `aa:bb:…` for a MAC address), so saving an unchanged configuration gives identical bytes and
  does not even rewrite the file.
- A changed value is replaced and keeps its spacing and trailing comment. (Comments inside a
  changed multi-line array are lost with the array's old text.) Keys that are gone are removed;
  new keys are appended to their table.
- `[[tunnels]]`, `[[relays]]` and `[[pcs]]` entries are matched by `id`, never by position.
  A deleted entry is removed together with its sub-tables and the comments written above it. A
  new entry is appended after the last entry of its kind (or, if the file has none, before the
  first entry of a later kind). Reordered entries move as a whole, with their comments.
  A per-PC `[pcs.timing]` appears inside its PC when an override is needed and disappears
  when no value differs from `[defaults]` any more.
- A changed `id` is a removal plus an addition (the old entry's comments are lost) unless the
  caller lists the rename (`Renames`, `save_with`): then the entry keeps its place, comments
  and unchanged keys, and only the `id` value changes. The saved configuration must already use
  the new id everywhere it is referenced.
- No file, or an empty one: the file is rendered from scratch. A file that exists but is not
  valid TOML: an error, never an overwrite.
- Before anything is written, the resulting text is parsed and validated and must equal the
  configuration being saved exactly; otherwise nothing is written and an error is returned.
  The write itself is atomic (temporary file in the same folder, then rename).

Loading yields a `Config` of validated domain types (invalid states unrepresentable after
this point); the rest of core never re-validates.

**Applying a changed configuration.** `Handle::reload(Config)` is an engine message (like
`stop`), not a machine command, so the UI keeps the same handle, thread, sink and windows.
The engine starts over with the new configuration: a fresh `Machine` (every tunnel and PC
unknown and due at once), a new snapshot, and a generation counter that makes it drop the
results of jobs started under the old configuration (so no stale result reaches a machine
that never asked for it). A running tunnel is found running by the first full probe and a PC
that answers is found online, both without a notice; a wake or a start that was in flight is
forgotten (the PC then shows as it really is a poll later). Entities are *not* matched by id
to carry state over: that would keep the in-flight flags of jobs whose results are dropped.
After the settings window saves (`settings::Change::save`) the app calls
`Handle::reload(change.config)` and refreshes anything it built from the startup
configuration (the RDP launcher: `RdpLauncher::update`). `rdp/<pc id>.rdp` files are never
rewritten or renamed, so a renamed PC starts a new file and a changed address does not reach
an existing one.

**Forms.** `core::settings` (lists, tunnel and PC add/edit/delete as pure operations that
return a `Change`), `core::wizard` (the add-PC draft), `core::tunnel_form`, `core::checks` and
`core::about` are the logic behind the settings window and the wizard. Their field rules are
the same code as the validator above (`core::rules`; the domain parsers), their errors are
`problem::Problem` values (field, code, parameters) worded by `Translator::problem` from the
keys `problem.<code>`; the tests assert that a form accepts a value exactly when the file
would. Ids are derived from names (slug, transliterated, unique per kind), stable on edit
unless the name changes.

## 9. Errors and notifications

- Each boundary has its own `thiserror` enum: `ConfigError` (a list of issues), `TunnelError`,
  `WakeError`, `PowerError`. Adapters never panic; core denies
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
- Notice delivery in `app`: tray balloon. Startup errors (invalid config, second
  instance, not elevated) use `MessageBoxW`, since Slint is not up yet. Slint 1.18's
  `SystemTrayIcon` was checked in its sources: it has an icon, a tooltip, a title and a menu, and
  no notification call. So every notice the engine reports is written to the log (`target:
  notice`, worded in the user's language) and shown as a balloon by `Shell_NotifyIconW` with
  `NIF_INFO` (`windows` crate, `platform.rs`, behind `cfg(windows)`): a hidden message-only
  window owned by the app on the event loop thread, with its own hidden notify icon (id
  `0x484C`, `NIS_HIDDEN`) used only for balloons; the notice is handed to the event loop thread
  first, because the engine reports from its own thread. The text is built by pure functions
  (`balloon.rs`): title cut to 63 and body to 255 UTF-16 units (the fields hold 64 and 256 with
  the terminating zero), never inside a surrogate pair, ending with `…`; the icon (info,
  warning, error) comes from `Notice::severity` (core): a failed action is an error, a tunnel
  or PC that is gone or silent a warning, a confirmation information. Which notices exist is
  decided only by the engine, so the silence rules above (no notice for a user disconnect, for a
  PC that goes off after an accepted power command, for a PC that becomes unknown because its
  tunnel dropped) hold for balloons too. The icon (the application icon, resource id 1, tip
  "HomeLink") is made visible for each balloon and hidden 10 s after the last one
  (`platform::show_balloon`). Off Windows there are no balloons, only the log. An RDP failure is shown in a
  message box.
- Elevation: v0.3 relaunches itself elevated because the WireGuard service needs admin. The
  app does the same at the start of the real mode: `whoami /groups` shows the integrity level
  (`S-1-16-12288` or higher is elevated; the SID is the same in every language) and, if the
  process is not elevated, `powershell Start-Process -Verb RunAs` starts an elevated copy and
  this one exits. If the level cannot be read the program goes on, so a broken check cannot
  restart it forever. That check stays as a fallback. For parity with v0.3 the executable
  also carries a manifest with `requireAdministrator` (`rust/app/windows/app.manifest`), so
  Windows shows the UAC prompt before the program starts and the check normally finds it
  elevated (autostart via a scheduled task stays as in the README). The manifest also declares
  per-monitor DPI awareness v2 and Windows 10/11 support. It is embedded by `build.rs` with the
  `winresource` crate (0.1.34, MIT, no default features; it calls `rc.exe` on MSVC), only for a
  Windows target (the script reads `CARGO_CFG_TARGET_OS`, so the Linux build does nothing) and
  only for the `HomeLink` binary (`compile_for`), so the test executables of the crate do not
  ask for elevation. The same resource has the application icon (`rust/app/windows/app.ico`,
  copied from `legacy/python/src/homelink/assets`) and the version information from `Cargo.toml`. The UTF-8
  active code page (`activeCodePage`) is deliberately **not** in the manifest: the decoding of
  console output (section 3b of the roadmap) depends on the OEM code page, and whether the
  setting leaves the code page a child console program writes in unchanged was not verifiable
  here, so it is left out. Revisit later so PCs without a tunnel work unelevated.
- Taskbar: the tray panel and the details window stay off the Windows taskbar; the settings
  window, the wizard and the tunnel form appear on it with the application icon and the title
  `HomeLink — <window>`. The window attributes hook of Slint is called for every window alike,
  so it cannot decide this; `platform::hide_from_taskbar` sets `skip_taskbar` on the winit
  window (`WindowExtWindows`) of the panel and of the details window after each `show()`.
- Single instance: a lock on `homelink.lock` in the configuration folder (`File::try_lock`),
  held for the life of the process and released by the system when it ends; this replaces the
  named mutex and needs no Win32 binding. Two copies with different configuration folders can
  run side by side.

## 10. Internationalisation

English and Ukrainian, same mechanism as `i18n.py`: flat `key -> template` JSON files with
`{param}` placeholders, embedded with `include_str!`, language from config (`auto` reads
the Windows UI language). One catalog serves everything: Slint receives its static texts
through a `global Words` filled from the catalog (and refilled on a language change);
notices, dialogs and wizard texts come from the same catalog. Slint's own `@tr`/gettext is not
used: it would be a second mechanism and cannot cover notices and message boxes.

Tests (run on Linux): both locales have identical key sets and identical placeholder sets per
key; every `Notice` variant and every `Words` field has a key.

The About window shows the version, the licence (MIT), "Built with Slint" with a link to
https://slint.dev (required by the royalty-free licence) and the third-party licence list
(Inter is SIL OFL, in `rust/app/ui/fonts/OFL.txt`; Rust crate licences are generated by
`tools/third_party_notices.py` and embedded).

## 11. Wizard

The wizard is a second Slint window; its logic (`core::wizard`) is pure and tested on Linux. A
`PcDraft` moves through five steps; each step validates its slice with the same rules as section 8
(`validate_step`), and `build` converts the draft to a `Pc` (plus a new `Relay` if the wake method
needs one) that `settings::save_pc` adds to the configuration and saves in place. "Змінити" on
a PC reuses the wizard, filled by `PcDraft::from_pc`. The step structure follows
`docs/design/ui-handoff.md` section 8, which is authoritative (no new tunnel in the wizard,
tunnels are made in the settings window;
the operating system is a parameter of the SSH power method; there is no summary step).

1. Address and ports: name, IP address or host name, readiness ports (comma separated).
2. Tunnel: an existing tunnel or none (`wizard::tunnel_options`).
3. Wake method: none / through a relay host over SSH / direct magic packet / command, then the
   parameters of the chosen method.
   - *Relay host:* pick an existing relay or create one (host, port, user, key file, preset,
     sudo; see "SSH setup" below) and choose its **preset**: RouterOS, etherwake or wakeonlan (`wizard.relay.*`). The preset
     decides which fields the step shows: an interface for RouterOS and etherwake, an optional
     destination address for wakeonlan, the sudo question for etherwake and wakeonlan. The step
     then shows the setup commands for that preset (below).
   - *Direct magic packet:* MAC, a destination address (default: broadcast) and port; when the PC is
     behind a tunnel or in another VLAN the step shows the routed-setup help (unicast to the
     PC's IP and a static ARP entry on the last router, `wizard.magic.routed_help`).
   - *Command:* a command line (`{host}`, and `{mac}` when a MAC is given), optional MAC, timeout.
4. Sleep and shutdown: the boxes "Дозволити сон і вимкнення", "Дозволити RDP", then
   the control parameters: SSH (user, port, key file, operating system preset) or commands
   (one per allowed action), and the RDP user name. The step says that HomeLink never handles
   the RDP password (`wizard.rdp.note`). Sleep and shutdown share one box: over SSH they always
   come together (`PowerMethod::Ssh`); with commands either command may be left empty (that
   action is then not offered), but not both. The panel keeps separate sleep and shutdown buttons.
5. Checks (`core::checks`): readiness ports, SSH to the relay, SSH to the PC, each with a marker
   and a short result; "Перевірити знову"; the PC can be saved without a successful check. The
   checks need the tunnel up: with it down every row says so and nothing is run.

**SSH setup** (steps 3 and 4; shared by relay hosts and PCs). The wizard does not ask for or store
any password.

- *OpenSSH client check (built, `core::ssh_setup`, `app::ssh_ops`).* When the wizard opens and
  before the checks, it looks for `ssh`, `ssh-keygen` and `ssh-keyscan` in the folders of `PATH`
  and in `%SystemRoot%\System32\OpenSSH` (the `.exe` files on Windows). On Windows 10 LTSC the
  client is an optional feature that may be missing; the wizard then shows how to install it
  (Settings > Apps > Optional features > OpenSSH Client; `wizard.ssh.openssh_missing`) under the
  key field of the relay (step 3), of the PC (step 4) and on the checks step, disables
  "Створити ключ", and does not run the SSH checks (their rows say the client is not installed).
  Saving the PC stays allowed. A client found only in `System32\OpenSSH` and not on `PATH` counts
  as installed, but the run-time commands start `ssh` through `PATH`, so they would still fail;
  the optional feature puts the folder on `PATH`.
- *Key (built).* Next to each key-file field (new relay, PC) "Створити ключ" runs
  `ssh-keygen -q -t ed25519 -N "" -C homelink -f <path>` with an empty passphrase, so the
  elevated program can use the key unattended. The file goes to the `keys` folder of the
  configuration folder (`keys\homelink_ed25519`, then `_2`, `_3`...; a name whose private or
  `.pub` file exists is never reused, so nothing is overwritten) and fills the field. The
  window then shows the public key text with a copy button. The program never reads the private
  key: the text comes from the `.pub` file beside the file named in the field, also when the key
  was made by hand. The configuration stores only the absolute path. The folder inherits the
  permissions of the configuration folder (next to the program); `ssh-keygen` itself restricts
  the private file (on Windows it sets the access list of the file). The wizard does not offer
  "let ssh choose".
- *Relay side, by preset* (`wizard.relay.setup.*`; the wizard fills in the user name, key
  file name and the address of this computer):
  - `routeros`: the RouterOS commands to create a group that can only log in over SSH and run
    `/tool wol` (policies `ssh`, `read`, `test`), create a user in it that is limited to this
    computer's address (disabled, with a random password made on the router), add the public
    key for that user and enable the user. **Verified on RouterOS 7.24.5 (RB5009) on
    15.11.2026**, see below.
  - `etherwake`: install `etherwake`, create the SSH user and add the public key to its
    `authorized_keys`. The step asks "run the command with sudo" (`wizard.field.sudo`,
    `wizard.hint.sudo`; there is no default, the user must choose). If yes, it also shows
    the `sudoers` line that allows that user to run the `etherwake` binary (path from
    `which etherwake`) with `sudo` without a password, for that command only; HomeLink runs
    `sudo -n`, so a missing rule fails at once.
  - `wakeonlan`: the same, for `wakeonlan`.
  - `routeros`: no `sudo` question.
  The text of each set lives in one place in core (`core::relay_setup`, pure and tested) so the
  wizard and the tests share it. Built for a new relay from the wizard's fields and for an
  existing relay from its entry in the settings (user, key file, preset, sudo); it
  is a read-only block with a copy button in step 3 under the relay's
  fields, refreshed when the preset, user, key file or sudo answer changes. Placeholders in angle
  brackets stand for what the program cannot know: a user name that is not safe to put in a
  command, the public key before it exists, and, for RouterOS, the address this computer has in
  the router's network (not detected: it depends on the tunnel). The generated lines are:
  RouterOS: `/user group add name=homelink policy=ssh,read,test comment="HomeLink"`,
  `/user add name=<user> group=homelink address=<address> password=[:rndstr length=32]
  disabled=yes comment="HomeLink"`, `/user ssh-keys add user=<user> key="<key>"`,
  `/user enable <user>`. etherwake and wakeonlan (as root
  on the relay): `apt install <tool>` (the text says to use `opkg install` on OpenWrt), the
  user's `~/.ssh` with mode 700, the key appended to `authorized_keys`, and with sudo a file in
  `/etc/sudoers.d` for `$(command -v <tool>)` with `NOPASSWD`, checked by `visudo -cf`.
  **RouterOS check (7.24.5, RB5009, 15.11.2026, a test user that was removed afterwards):** the
  group with `ssh,read,test` logs in with its key and runs `/tool wol`, reads, and is refused a
  write (`not enough permissions`); a bare `address=` is stored as `/32`; `/user add` refuses to
  run without `password`, hence the random one that is never shown; `/user ssh-keys add` takes
  the key directly (no temporary file); `/ip ssh` has no `always-allow-password-login` but
  `password-authentication`, by default `yes-if-no-key`, so a user with a key is offered only
  `publickey` and the commands change nothing global. **RouterOS ends an SSH command with exit
  code 0 even when it fails** and prints the error on standard output, so `core::wake` treats
  any output of a `routeros` relay (except `ssh` client warnings) as a failed wake
  (`routeros_error`).
- *PC side (built, `core::pc_setup`).* For an SSH power method with sleep or shutdown allowed,
  step 4 shows the commands to run on the PC under its key field, with a copy button, refreshed
  when the operating system, user, key file or allowed actions change. Linux (as root): the
  user's `~/.ssh` with mode 700, the public key appended to `authorized_keys`, and
  `/etc/sudoers.d/homelink` with `<user> ALL=(root) NOPASSWD: $(command -v systemctl) poweroff,
  $(command -v systemctl) suspend -i` (exactly the two preset commands, a test keeps them in
  step with `ssh::power_remote_command`), checked by `visudo -cf`. Windows (PowerShell as
  administrator): `Add-WindowsCapability ... OpenSSH.Server`, `sshd` automatic and started, the
  key appended to `%ProgramData%\ssh\administrators_authorized_keys`, and `icacls` restricting
  that file to Administrators and SYSTEM by SID (`*S-1-5-32-544`, `*S-1-5-18`), so a localised
  Windows works; the user must be an administrator of the PC. **The Windows commands are
  unverified on a real PC** (release checklist, section F).
- *Test connection and host key consent (step 5, built: `core::checks`, `core::ssh_setup`,
  `app::ssh_ops`).* The checks run `ssh` with the same options as at run time (`BatchMode=yes`,
  `IdentitiesOnly=yes`, `-i <key>`, `StrictHostKeyChecking=yes`) and report an unknown host key
  as `CheckResult::SshHostKeyUnknown`; nothing is ever accepted by `ssh` itself, so
  `accept-new` is not used at all. A row with an unknown key has "Довірити вузлу". Pressing it
  runs `ssh-keyscan -T 8 -p <port> -- <host>` (a plain TCP connection; the checks need the
  tunnel up, so it goes through the tunnel as the checks do), builds the `known_hosts` lines
  from the keys found (the host field of the scan is ignored: the line names the host the user
  chose, `host` or `[host]:port`), writes them to a temporary file and shows the SHA-256
  fingerprints that `ssh-keygen -l -E sha256 -f` computes from that very text (one per key
  type). Only when the user presses "Довіряти" (`wizard.ssh.trust`) are those same lines
  appended to `%USERPROFILE%\.ssh\known_hosts` (the file the program's `ssh` reads; `$HOME` off
  Windows), after a line end if the last line had none, and the checks run again. "Скасувати"
  or leaving the step withdraws the offer. An entry for the host that exists already
  (`ssh-keygen -F`) is never replaced or doubled: the wizard says so and how to remove it by
  hand. A changed key (`SshHostKeyChanged`) is an error with no button; the note explains what
  it may mean and that HomeLink never replaces it. This replaces the manual first `ssh` from the
  README and is the only place in the program that adds a host key. Not built: the `ssh` of
  the run-time commands uses the default `known_hosts`; an explicit `UserKnownHostsFile` in the
  config folder would remove the dependence on the profile (see the risks).

## 11a. System capabilities (built 15.11.2026)

Owner's decisions of 15.11.2026 (`VIC/HomeLink/homelink-capabilities-spec.md` in the vault and
the designer's artboards `Capabilities`, `Unavailable`, `TunnelForm`). No silent failures: every
external program is looked up before it is needed, and a missing one disables only what depends
on it.

- **Lookup** (`core::capabilities`, pure, tested on Linux): by file existence only, nothing is
  started. Windows order: the tunnels' `wireguard_exe`, the known folder (`System32`,
  `System32\OpenSSH`, `System32\WindowsPowerShell\v1.0`, `Program Files\WireGuard`), then
  `PATH` with `PATHEXT`; elsewhere `PATH`. User command programs (`argv[0]` of command wakes and
  power commands) are resolved too. Features and their tools: tunnels (`wireguard`, `sc`), tunnel
  check (`ping`; **without it tunnels are blocked as a whole**, owner's answer), SSH actions
  (`ssh`), keys (`ssh-keygen`, `ssh-keyscan`), RDP (`mstsc`), magic packet (none), elevation
  (`whoami`, `powershell`), links (`cmd`).
- **When:** at start (before the engine's first step, `Engine::with_capabilities`), when the
  settings window opens, after a save, and on "Перевірити знову" (capabilities page, tunnel
  form). No polling. Changes are logged once at `info` (`app::capability`).
- **Run by the found path:** `StdCommandRunner::set_programs` maps bare names to the found
  paths, so `ssh` found only in `System32\OpenSSH` runs.
- **Snapshot:** every PC action and the tunnel toggle carry an `Availability`
  (`Available | NotConfigured | NotNow | Missing(Vec<Need>)`); the old bools are methods
  (`actions()`, `configured()`, `can_toggle()`). A PC behind a tunnel needs the tunnel's programs
  for every action.
- **Engine:** no tunnel job and no tunnel probe while a tunnel program is missing (tunnels become
  unknown, PCs behind them too; results of probes started before are dropped); no wake or power
  job whose program is missing. A lookup is kept across `Handle::reload`.
- **UI:** the reason line under inactive buttons and the "Недоступно" row of the details window
  (`core::reasons`); the settings page "Можливості системи" with eight tiles
  (`core::capability_page`, neutral, links to the WireGuard installer and Microsoft's OpenSSH
  page); the tunnel form shows a red block, inactive fields and Save without the WireGuard client,
  and an error when the tunnel file does not exist.
- **Notices:** `TunnelProgramsMissing` (tunnels configured, a tunnel program missing) and
  `SshMissing` (an SSH method configured, `ssh` missing), once per start and again only when
  what is missing changes. No notice for `mstsc` (owner's answer: the inactive RDP button is
  enough).
- **Demo:** `--missing wireguard,ssh,...` (tool codes) pretends programs are missing.

## 11b. Tunnel check by handshake and the tunnel form (built 15.11.2026)

Owner's decisions of 15.11.2026, after the first check on Windows 11.

- **`probe_host` is optional.** With it, a tunnel is checked by ping as before. Without it, by
  its WireGuard handshake (`core::handshake`): `wg.exe show <tunnel> latest-handshakes` prints
  `<peer key>\t<unix time>`; a handshake younger than 180 s (WireGuard's reject-after time)
  means the server is alive. When it is older, one empty UDP datagram to the first address of
  the first network of `wg.exe show <tunnel> allowed-ips` (e.g. `10.10.0.1` for `10.10.0.0/16`)
  makes WireGuard start a handshake; the check waits up to 6 s for it. Nothing is guessed about a
  host behind the tunnel; whether the network behind the server works is shown by the PCs' own
  port checks. Only `latest-handshakes` and `allowed-ips` are run, never `showconf`, `dump`,
  `private-key` or `preshared-keys`. `TunnelDriver::handshake` is the port; the Windows driver
  runs `wg` (resolved to `%ProgramFiles%\WireGuard\wg.exe` by the capability lookup) and sends
  the datagram with a std UDP socket.
- **Each tunnel has its own needs** (`capabilities::tunnel_needs`): the WireGuard client and `sc`,
  plus `ping` with a probe host or `wg` without one. A missing check program blocks only that
  tunnel. `Tool::Wg` is a new tool; the "Перевірка тунелю" tile names `wg` and `ping`.
- **Tunnel form:** a list of the tunnels imported into the WireGuard client, read from the file
  names of `<WireGuard>\Data\Configurations` (the files themselves hold the private keys and are
  never opened); tunnels already in the settings are marked and refused (`Code::TunnelTaken`).
  Choosing one fills the name while the user has not typed another; the path of the file and of
  `wireguard.exe` (the found one) are set by the program. The probe host is no longer in the form;
  an existing one is kept on edit. The template no longer sets `probe_host`.

## 11c. Fixes after the check on Windows 10 LTSC (rc.5, 16.11.2026)

- **"ПК у мережі роутера"** (step 5, `CheckKind::RelayNeighbor`, RouterOS relay and a PC with an
  IPv4 address): `ssh` to the relay runs `checks::routeros_neighbor_command`, which prints the
  MAC and interface of the address from the ARP table, else from the DHCP leases, else `none`
  (read-only; checked on 7.24.5). Same values as step 3: ok; others: "Застосувати ці значення"
  puts them into step 3 and runs the checks again; not seen: unknown, not a failure.
- A RouterOS `Permission denied` names the two causes (key not added for the user, group without
  the `ssh` policy).
- The trust of a host key logs each step (fetch, offer, write), to find why the offer did not
  appear on that machine.
- Labels: "SSH-ключ (файл приватного ключа)" with a hint, the interface field with a hint about
  VLANs.

## 11d. Layout heights and the ui-shots tool (rc.7, 16.11.2026)

Rendered without a display (`rust/tools/ui-shots`,
Slint's software renderer), Slint 1.18 turned out to compute the preferred height of a layout
that holds a wrapped `Text` in a nested layout (`if … : VerticalLayout { Text { wrap … } }`) as if
the text were one line; an explicit `width` on the text makes it measure at that width. Every
wrapped text in a vertical layout of the wizard, the settings, the tunnel form and the controls
now has `width: parent.width` (minus the horizontal padding of its layout). Texts whose layout
width depends on them (the panel's submenus, the details rows, the capability tiles) keep their
old form: an explicit width there is a binding loop. The list rows of the settings have bordered
"Змінити"/"Видалити" buttons (`BtnKind.compact`). `ui-shots/run.sh` renders the wizard steps
3–5 at the top and scrolled to their end, the settings list in both themes, the help page and the
panel with reason lines.

## 11e. Wizard and details improvements (rc.9, 16.11.2026)

- A refused "Далі" scrolls the step to its top and says by the buttons to fix the fields marked in
  red; a field's problem hides as soon as the field is edited and shows again at the next check
  (`Flow.error-generation`, raised with each set of problems).
- The step bar is clickable (each segment lit under the mouse, no labels): back to any step;
  forward checks every step on the way and stops at the first with a problem.
- Key fields: "Огляд…" opens the standard Windows "Open" dialog (`platform::pick_file`,
  `GetOpenFileNameW`) in the profile's `.ssh`; an empty key field is filled when that folder has
  exactly one private key with its `.pub` (only file names are read).
- The details window shows the PC's ports in colour: green answered, red did not, plain before
  the first check (`PortProbe::open_ports`, `PcView::open_ports`).
- A new entry is written in the template's layout: its sub-tables and their keys indented by two
  spaces.

## 11f. Sleep and shutdown box, file dialog (rc.10, 16.11.2026)

- Step 4 has one box "Дозволити сон і вимкнення" instead of two; the "allow both over SSH" check
  is gone. With commands either command may be left empty (`Code::PowerCommandNeeded` when both
  are). The panel keeps separate sleep and shutdown buttons.
- The "Open" dialog is owned by the active window, and Esc handlers ignore winit's synthetic key
  presses (sent to a window that gets the focus while a key is held), so Esc in the dialog no
  longer closes the settings.

## 12. Open questions (not decided here)

1. **Does the custom panel work well on screen?** Window focus on show and hide, hide-on-blur
   against a tray-icon click, action buttons under a mouse and keyboard, DPI, taskbar not at
   the bottom (the prototype anchors to the work-area corner), multi-monitor, accessibility.
   Needs a check on real Windows 10 LTSC and 11; fallbacks: `MessageBoxW`
   confirmations, a standard tray menu as a degraded mode.
2. **Panel and wizard styling.** To a designer: layout of the two columns, tunnel button states,
   PC row with four actions, wizard steps, About, and the tray icon rule for several tunnels
   and PCs (section 7 is provisional).

## 13. Testing strategy

Everything in `core` runs on Linux in CI. Goal: the logic is proven without Windows; Windows
tests prove only the thin adapters.

- **State machine, table-driven.** One test per row of section 5.2/5.3, plus every test in
  `legacy/python/tests/test_controller.py` ported (single failed check keeps online, one success is enough,
  pc lost after N failures, silent after a power command, silent when the tunnel drops, the
  detail limit, busy skip, shorter interval while waking, etc.), with a manual clock.
- **Multi-entity cases (new).** Two PCs on one tunnel (second `Wake` while `Starting`), PC
  without a tunnel, tunnel lost with several PCs (one `TunnelLost`, no `PcLost`), pending wakes
  cleared on failure.
- **Property tests** over random event sequences: a tunnel PC is never `Online`
  or `Waking` while its tunnel is not `Up`; `Online -> Offline` needs `N` consecutive failures;
  `PcLost` never follows an accepted power command; no notice without a transition cause.
- **Config:** valid and invalid fixture files; every rule of section 8 has a failing case with the
  expected issue path; the documented example is parsed in a test; round trip load-save-load.
  Saving in place is tested on a heavily commented fixture: no change gives identical bytes;
  one changed value, additions, deletions, renames and reorders change only what they should;
  timing overrides appear and disappear; and a randomised test applies many edits in a row and
  checks that editing in place and rendering from scratch describe the same configuration.
- **Adapters:** magic packet bytes (6 x `FF` + 16 x MAC) and a loopback socket; the relay
  `ssh` argument list and remote command built exactly as specified for every preset (RouterOS,
  etherwake, wakeonlan), with hostile MAC, interface and address values
  rejected before they reach the command (done in `core::wake`, with a fake
  `CommandRunner`), with and without `sudo`; TCP probe against local listeners, an unused port
  and `localhost`, and a check that nothing is sent; the magic packet against a known vector
  and a local UDP socket; the command runner with `sh`, `sleep`, `true` and `false`: exit codes,
  signals, output bounds, a pluggable decoder, the timeout kill including a background child
  (checked by process id); a fake `ssh` script that records its arguments, used for wake and
  power end to end including every failure class; and one test that runs the real engine, thread
  pool and these adapters against local stand-ins (`tests/real_engine.rs`).
- **Privacy checks:** a test that the configuration schema has no password, token or URL
  key, and a CI check (`cargo deny`, plus a grep gate on `http`/`tls` client crates in the
  dependency tree) that no HTTP or TLS client is ever added.
- **Engine:** the real engine with an inline `Executor`, `Clock` and fakes: end-to-end
  scenarios (connect-and-wake to online, wake timeout, shared tunnel) asserting snapshots and
  notices in order.
- **i18n and wizard:** parity tests (section 10), step-by-step wizard tests.
- **Windows only (CI `windows-latest`):** SCM query of a non-existing service, ICMP to
  loopback, OEM decoding, OpenSSH client detection.
- **Manual release checklist** (in the repo, run on Windows 10 LTSC and 11): tray, panel focus,
  notifications, real WireGuard, one relay of each preset, a Windows and a Linux PC, parity with 0.3.0.

## 14. CI and release changes

`rust/rust-toolchain.toml` pins the compiler (1.100.0); CI installs it with
`rustup install`.

- `ci.yml` runs on every push to any branch and on pull requests to `main`.
- Job `rust-core` (ubuntu-latest): `cargo fmt --check`, the licence policy and
  `THIRD-PARTY-NOTICES.txt` check (`tools/third_party_notices.py`, which also forbids HTTP and
  TLS client crates and non-crates.io sources), `cargo deny check advisories bans sources`
  (`rust/deny.toml`: RustSec advisories, the same banned clients, sources; licences are left to
  the script), clippy `-D warnings`, `cargo doc` with warnings denied, tests.
- Job `rust-app` (windows-latest): fmt, clippy `-D warnings`, tests of `homelink`, the release
  build with the `demo` feature, `tools/smoke.ps1` (size budget 16 MiB, `--version` equal to the
  package version, `--check-config` passes on the example and fails on a missing file without
  creating it), the `HomeLink-rust-demo` artifact.
- `release.yml`: a tag `v*` must equal the version in `rust/app/Cargo.toml`; the same gates as
  the `rust-app` job plus the licence check; the release build (without `demo`); the smoke
  run; a release with `HomeLink.exe` (name unchanged), `HomeLink.exe.sha256` and
  `THIRD-PARTY-NOTICES.txt`. A tag with a suffix (`v1.0.0-rc.N`) is a pre-release.

Not done: a coverage threshold, a scheduled job on the newest stable, Dependabot (new CI
configuration, only with the owner's approval).

## 15. Roadmap to v1.0.0

| Phase | Content | Exit criterion |
| --- | --- | --- |
| 3b Windows adapters | In `app/src/windows/`, as pure functions plus a thin layer over `CommandRunner`, no new Win32 binding: WireGuard service (`wireguard.exe` install/uninstall, state from `sc query`, parsed independently of the display language), ping (`TTL=`), OEM output decoder (code pages 437, 850, 866 from the registry value `OEMCP`; other pages decode ASCII only) plugged into `StdCommandRunner`, `.rdp` file and `mstsc`, elevation check and restart, display language from `reg query`. OpenSSH client detection is done (stage D1, `core::ssh_setup`). RouterOS commands and `/tool wol` over SSH verified on 7.24.5 (15.11.2026). **Still open:** real `sc query` output in a non-English OEM code page (the test samples follow the documented format and were not captured on Windows); decoders for other code pages if needed (the UTF-8 code page 65001 is decoded as UTF-8 since rc.3; it was seen on a Windows 11 machine) | adapter tests green in the Windows CI job; a real `/tool wol` over SSH works |
| 4 App shell | Done: panel on the real engine, tray with aggregate icon, `config.toml` with a template on the first start and every validation problem shown in the catalog language, rotating log file, single instance (file lock), elevation restart, RDP button, `--config`, cargo feature `demo` for the fake-driven mode. **Stage D1:** tray balloons for notices (`Shell_NotifyIconW`, section 9; Slint's tray has no such call), the manifest with `requireAdministrator` and per-monitor DPI v2, the application icon and version information embedded by `winresource`, the settings window, wizard and tunnel form on the taskbar (only the panel and details window are hidden from it). **Done (15.11.2026):** the tray icon palette follows the taskbar theme and the display scale, the application icon in the settings and About windows, waiting up to 3 s for the engine thread on quit, local time with its UTC offset in the log, app tests in CI, `--version` and `--check-config` | a single PC with a tunnel works end to end on Windows |
| 5 Multi-PC and wizard | Groups, shared tunnel, PC without a tunnel, wizard (add/edit/delete), About with the Slint credit, config reload. **Done (C1, core logic, tested on Linux):** `core::settings`, `core::wizard` (five steps of the handoff), `core::tunnel_form`, `core::checks`, `core::about`, `Handle::reload`, locale keys. **Done (D1):** OpenSSH client detection, key creation, SSH host-key consent, the relay setup commands (section 11); the UI of these has no designer's layout (reused Field/Btn/list styles). **Done (15.11.2026):** the PC-side commands (`core::pc_setup`), the help page (`core::help`), the third-party notices file | tunnel groups, a shared tunnel and a PC without a tunnel work by hand |
| 6 Hardening | Manual checklist on Windows 10 LTSC and 11 (`docs/release-checklist.uk.md`, Linux and Windows remote PCs); docs rewrite done (`README*.md` for the Rust version, relay, PC and SSH setup); size and licence checks in CI; `v1.0.0-rc.1` | no open defects; parity checklist complete |
| 7 Release | Python already archived in `legacy/python/` (done 15.11.2026, kept, not deleted); Rust `release.yml`; tag `v1.0.0` | release published, README links verified |

Technical risks: position of the tray
icon (the prototype assumes a bottom-right taskbar); `ssh` reads `known_hosts` from the profile
of the elevated user, which can differ from the logged-in user when UAC elevates under another
account (the wizard and the program run in the same elevated process, so they agree, but a
key or host list prepared by hand in the other profile will not be seen); the OpenSSH client
may be absent on Windows 10 LTSC (optional feature, may need an offline source; the wizard
detects it and says how to install it); an
empty-passphrase key file must be protected by file permissions (created in the `keys` folder
of the configuration folder; `ssh-keygen` restricts the file itself, which must be checked on
Windows; a folder restricted to the user and administrators is not set up); `unstable-winit-030` is an unstable Slint feature (pin
the exact Slint version); the taskbar button of the panel may flash for
one frame on the first show before `skip_taskbar` applies (`skip_taskbar` is set after `show()`).

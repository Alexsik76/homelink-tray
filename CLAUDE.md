# HomeLink — rules for coding agents

Windows tray utility (Rust + Slint 1.18) that controls WireGuard tunnels and remote PCs.
Read this file first; it is short on purpose. Details live in the documents listed below.

## Layout

- `rust/core` (`homelink-core`): platform-independent logic. Domain model, `config.toml`
  (TOML, edited in place), state machine, engine, snapshot for the UI, settings and wizard
  logic, portable adapters, texts (`locales/en.json`, `locales/uk.json`). No Windows, no
  Slint, no UI decisions here.
- `rust/app` (`homelink`): the Slint app and the Windows adapters. `ui/*.slint` is the
  markup; `src/` binds the engine snapshot to the windows; `src/windows/` runs system
  programs.
- `rust/tools/ui-shots`: renders the windows without a display (see its README).
- `legacy/python`: the released 0.3.0 for reference only. Do not edit it and do not
  import from it.
- `docs/rust-architecture.md`: architecture and settled decisions.
  `docs/design/ui-handoff.md` and `ui-handoff-dark.md`: the visual specification.
  `docs/release-checklist.uk.md`: manual checks before a release.

## Rules

1. **Decisions belong to the core.** The UI renders the snapshot and sends commands. If
   the UI needs to know something (availability, a reason, a text), add it to the
   snapshot or to a core function with a test; do not compute it in `rust/app`.
2. **Reuse UI components.** Buttons, fields, radio rows, checkboxes, list rows and
   scrollbars come from `rust/app/ui/controls.slint`. Before writing a `Rectangle` +
   `TouchArea` by hand, look there. If a control is needed twice, add it to
   `controls.slint` and use it in both places. Colours and sizes only through `Theme`
   tokens; no colour literals and no hardcoded user-visible text.
3. **Remove what a change makes obsolete, in the same change.** This includes what the
   compiler cannot see:
   - locale keys no longer used (both `en.json` and `uk.json`);
   - Slint components, properties and callbacks no longer used;
   - tests of behaviour that no longer exists;
   - paragraphs of `docs/rust-architecture.md` that describe the old behaviour.
   Do not leave compatibility shims, renamed-but-kept functions or commented-out code.
   No new `allow(dead_code)` without a one-line reason.
4. **Behaviour changes start in the document.** For a change of behaviour (states,
   settings schema, what a button does), update the relevant section of
   `docs/rust-architecture.md` in the same change, in place. Do not append a dated
   "what was fixed" section: the document describes the current design, git holds the
   history.
5. **A visual change is not done until it has been looked at.** Render the affected
   window with `rust/tools/ui-shots` and check the picture, in both themes when colours
   changed and in both languages when texts changed. Say in the report what was rendered
   and what only a real Windows screen can confirm.
6. **Keep changes small and single-purpose.** One change does one thing. A refactoring
   and a behaviour change are separate changes. If a task turns out to need a wider
   rewrite than asked, stop and say so before doing it.
7. **Every enum that models a choice is matched exhaustively** (no `_ =>` catch-all on
   domain enums such as wake method, relay kind, display state), so that adding a variant
   shows every place to update.
8. **Never silent.** A failure the user can cause or fix is shown in words (notice, field
   error, reason line); it is not only logged.
9. **No network clients, no secrets.** The program stores no passwords and contains no
   HTTP or TLS client; CI enforces this. Everything remote goes through the system `ssh`
   with a key.

## Checks before merging (from `rust/`)

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo build -p homelink --release --locked --features demo
python3 ../tools/third_party_notices.py --check
```

The Windows-only code is compiled and smoke-run only by CI (`rust-app` job); wait for it
before reporting a change as done.

## Report after each change

Commit hash, what changed in one or two lines, what was removed as obsolete, what was
rendered or tested, and what remains unverified.

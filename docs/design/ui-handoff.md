# HomeLink — UI design handoff

Implement the approved visual design for the HomeLink tray application. This document is self-contained: all sizes, colours and behaviours needed are listed here. Sizes are logical pixels.

## 1. Context and constraints

- Windows tray app that controls WireGuard tunnels and remote PCs (state, tunnel up/down, wake, sleep, shut down, RDP).
- UI: Slint 1.18, hand-built from `Rectangle`, `Text`, `Image`, `TouchArea`. No widget style libraries (no Fluent, no Material). Main file: `rust/app/ui/main.slint`.
- Software renderer. Use only solid fills, 1 px borders and simple colour transitions. No shadows, blur or window translucency.
- Font: Inter, two embedded weights only — Regular (400) and SemiBold (600). Do not add weights.
- All corners are square (radius 0). All lines are 1 px.
- Must scale correctly at 100–200 %.
- All labels must fit in Ukrainian and English (longest known: "Розбудити", "Так, вимкнути", "Shut down", "Yes, shut down").
- Own visual style; do not imitate Windows controls.

## 2. Design tokens

Define these once (e.g. a global `Theme`) and reference them everywhere; no hard-coded colours in components.

| Token | Light | Dark | Use |
|---|---|---|---|
| `bg` | `#FAFAF8` | `#17181A` | panel / window background |
| `bg-side` | `#F3F3EF` | `#1D1E21` | tunnel column, settings navigation |
| `surface` | `#FFFFFF` | `#202124` | submenu buttons, inputs, lists |
| `line` | `#DCDCD6` | `#34363B` | separators, control borders |
| `line-strong` | `#C9C9C3` | — | input and secondary-button borders |
| `frame` | `#2A2B2E` | `#5A5D63` | 1 px window border |
| `ink` | `#17181A` | `#ECECE8` | primary text, confirm fill |
| `ink-2` | `#4A4D52` | `#B4B7BC` | secondary text |
| `muted` | `#6B6E73` | `#9A9DA2` | captions, hints |
| `disabled` | `#ADB0B4` | `#5C5F64` | inactive text |
| `hover` | `#EFEFEA` | — | hover fill |
| `pressed` | `#E2E2DC` | — | pressed fill |
| `expanded` | `#E9E9E4` | `#2A2C30` | expanded element fill |
| `state-ok` | `#15803D` | `#22C55E` | working |
| `state-busy` | `#E8A317` | `#FACC15` | in progress |
| `state-off` | `#1F4FD8` | `#4A78FF` | off; also keyboard focus |
| `state-unknown` | `#8A8D91` | `#9A9DA2` | unknown (outline only) |
| `state-fail` | `#C4281C` | `#F0554A` | failure, error text |

Cells marked "—" were not designed for the dark theme. Dark theme is optional; if implemented, derive the missing values and flag them for review.

Typography (Inter): 16/600 window title in About; 13/600 element names, section titles; 13/400 inputs and body; 12/600 submenu items and buttons; 12/400 details values, secondary text; 11/400 captions and hints.

Spacing: 4 between submenu items; 6 column padding in the panel; 10 marker-to-name gap; 16 window padding.

## 3. State marker (shared by tunnels and PCs)

8 × 8 square, no rounding.

| State | Meaning | Rendering |
|---|---|---|
| ok | tunnel up / PC online | filled `state-ok` |
| busy | tunnel connecting / PC waking | filled `state-busy`, blinks with a 1 s period |
| off | tunnel stopped / PC off | filled `state-off` |
| unknown | state unknown | 1 px outline `state-unknown`, no fill |
| fail | failure only | filled `state-fail` |

## 4. Tray panel (left click on the tray icon)

Frameless window near the tray, width 420, height by content, 1 px `frame` border, `bg` background.

Layout, top to bottom:

1. One row per tunnel group, separated by 1 px `line`.
   - Left cell: width 140, `bg-side` fill, 1 px `line` right border, padding 6. Contains the tunnel element.
   - Right cell: remaining width, padding 6. Contains that tunnel's PC elements stacked vertically (one tunnel can have several PCs).
2. Last group: PCs without a tunnel. Left cell shows the caption "Без тунелю" (11/400, `muted`, left padding 16, vertically centred on the first row) instead of an element.
3. Footer: height 40, 1 px `line` top border. One button "Налаштування" (12/400, `ink-2`, 12 px icon to the left of the label, height 28).

Element (tunnel or PC): height 36, full cell width, 1 px transparent border, padding 0 10. Content: marker, 10 gap, name (13/600, `ink`). Long names are elided with an ellipsis. No state text and no action buttons in the initial view.

Element states:

| State | Rendering |
|---|---|
| normal | transparent fill |
| hover | `hover` fill |
| pressed | `pressed` fill |
| expanded | `expanded` fill + 1 px `ink` border |
| keyboard focus | 2 px `state-off` border |
| inactive | name in `disabled`, marker as 1 px outline `#C4C6C9` |

## 5. Action submenu (left click on an element)

Approved variant: the submenu opens inside the panel directly under the element and the panel grows downward. No popup window.

- Only one submenu is open at a time. Opening another closes the current one. Clicking the expanded element closes it.
- PC submenu: 2 × 2 grid under the PC element, gap 4, top padding 4, bottom padding 6. Order: Розбудити, Сон / Вимкнути, RDP.
- Tunnel submenu: one full-width button under the tunnel element, top padding 4. Label "Підняти" or "Опустити" depending on the current state.
- Submenu button: height 28, 1 px `line` border, `surface` fill, label 12/600 `ink`, centred.

Submenu button states:

| State | Rendering |
|---|---|
| normal | `surface` fill |
| hover | `hover` fill |
| pressed | `pressed` fill |
| inactive | label 12/400 `disabled`; not clickable |
| awaiting confirmation | `ink` fill, `ink` border, label in white (dark theme: `bg`) |

An unavailable action stays visible but inactive (for example "Розбудити" for a PC that is already online).

Confirmation for "Сон" and "Вимкнути": the first click switches the button to the awaiting-confirmation state and changes its label to "Так, у сон" / "Так, вимкнути" (English: "Yes, sleep" / "Yes, shut down"). The second click on the same button runs the action. Any other click, Esc, or closing the submenu cancels.

## 6. Details window (right click on an element)

Separate small frameless window, width 320, 1 px `frame` border.

- Header: height 40, 1 px `line` bottom border, padding-left 16. Marker, element name (13/600), close button (32 × 32, 10 px cross) on the right.
- Body: two-column list, label column 128 (`muted`), value column fills the rest; 12/400, line height 18, row gap 8, padding 10 16 14.
- PC rows: Стан (value 12/600), Адреса, Тунель, Пробудження, Порти готовності, Остання перевірка, Остання помилка.
- Tunnel rows: Стан (12/600), Вид, ПК за тунелем, Остання перевірка, Остання помилка.
- A non-empty last error is shown in `state-fail`; otherwise "немає".

## 7. Settings window (footer button)

Ordinary application window with the system frame, content about 720 × 480.

- Left navigation: width 180, `bg-side`, 1 px `line` right border. Items 36 high, 13/400: "Тунелі й ПК", "Довідка", "Про програму". Selected item: `bg` fill, 3 px `ink` bar on the left edge, 13/600.
- "Тунелі й ПК": two sections, "Тунелі" and "ПК". Each has a section title (13/600) with an add button on the right ("Додати тунель" / "Додати ПК": height 28, 1 px `line-strong` border, `surface` fill, 12/600). Below it a bordered list (`surface`, 1 px `line`, rows 36 high divided by 1 px `line`). Row: name (13/600), secondary text on the right (12/400 `muted`: PC count for a tunnel, tunnel name or "без тунелю" for a PC), text buttons "Змінити" (`ink`) and "Видалити" (`state-fail`), both 12/400.
- "Про програму": tray glyph at 48 px, "HomeLink" (16/600), version (12/400 `muted`), one-line description, then a block separated by 1 px `line`: "Інтерфейс побудовано на Slint" with the Slint version and a link to slint.dev, the Inter licence line (SIL Open Font License 1.1), the application licence and a link to third-party notices.
- "Довідка": not designed.

## 8. Add-PC wizard

Window 420 × 430, 1 px `frame` border, five steps.

- Header: height 40. Title "Додати ПК" (13/600) on the left, "Крок N з 5 · step name" (12/400 `muted`) on the right.
- Progress: five 3 px bars in a row, gap 4, side padding 16. Completed `ink`, current `state-off`, upcoming `line`.
- Body: padding 20 16, vertical gap 16.
- Footer: height 56, 1 px `line` top border, buttons right-aligned with gap 8. Secondary button ("Скасувати" on step 1, "Назад" afterwards): height 32, 1 px `line-strong` border, `surface` fill, 12/600. Primary button ("Далі", "Готово" on the last step): height 32, `ink` fill, white label 12/600.

Controls:

- Field: label 12/600, 6 gap, input 32 high (1 px `line-strong` border, `surface` fill, padding 0 10, 13/400), optional hint 11/400 `muted`.
- Focused input: 2 px `state-off` border. Invalid input: 1 px `state-fail` border and the hint replaced by an error message in `state-fail`.
- Radio option: row 32 high, 1 px `line` border, `surface` fill, padding 0 10; 14 × 14 square box with 1 px border, selected box has an 8 × 8 `state-off` square inside and the row border becomes `state-off`. Options stack with gap 4.
- Checkbox: 14 × 14 square; checked is `ink` fill with a white tick, unchecked is `surface` with a 1 px `state-unknown` border; label 13/400 with 10 gap.

Steps:

1. Адреса й порти — Назва, IP-адреса або ім'я вузла, Порти перевірки готовності (comma-separated).
2. Тунель — radio list of existing tunnels plus "Без тунелю".
3. Пробудження — radio list of wake methods plus "Не будити цей ПК", then the parameters of the selected method.
4. Сон і вимкнення — checkboxes "Дозволити сон", "Дозволити вимкнення", "Дозволити RDP", then control parameters.
5. Перевірка — bordered list of checks (row 40 high: marker, check name, result text 12/400 `ink-2` on the right), a secondary button "Перевірити знову", and a note that the PC can be saved without a successful check.

## 9. Tray icon

Monitor glyph on a 16 × 16 grid, pixel-aligned, no anti-aliasing:

- Outline: rectangle x 1.5, y 2.5, 13 × 9, 1 px stroke. Stand: vertical line from (8, 12) to (8, 14), base line from (5, 14.5) to (11, 14.5).
- Screen fill: rectangle x 3, y 4, 10 × 6.
- Corner square: x 10.5, y 9.5, 5 × 5, filled, with a 1 px stroke in the taskbar background colour to separate it from the glyph.
- Outline colour: `#1A1A1A` on a light taskbar, `#F2F2F2` on a dark taskbar. Fill colours follow the state tokens of the matching theme.
- Provide 16, 24 and 32 px renditions (100 %, 150 %, 200 %).

What the icon shows:

- Screen fill = aggregate tunnel state, first match wins: any tunnel failed → `state-fail`; any connecting → `state-busy`; any up → `state-ok`; all stopped → `state-off`; unknown → no fill (outline only).
- Corner square = PC attention, shown only on an event: any PC failed → `state-fail`; else any PC in progress → `state-busy`; otherwise no square.
- Tooltip: "Тунелі: X з Y піднято" and "ПК у мережі: X з Y" on two lines.

## 10. Not specified — ask before implementing

- Wake methods and their parameters (wizard step 3).
- Sleep / shut-down control parameters (wizard step 4).
- Content of the "Довідка" section.
- Application version, licence, and the Slint version string for "Про програму".
- Final field list of the details window (section 6 is a proposal).
- Missing dark-theme values (`hover`, `pressed`, `line-strong`).

## 11. Acceptance checklist

- Panel is 420 wide, grows and shrinks with the open submenu, and never shows more than one submenu.
- Tunnel rows and their PCs stay aligned; the "Без тунелю" group is last.
- Every element and submenu button implements all states listed in sections 4 and 5.
- Sleep and shut down never run on a single click.
- No shadows, gradients, rounded corners or translucency anywhere.
- No colour literal outside the token definitions.
- Ukrainian and English labels fit without clipping at 100 %, 150 % and 200 %.
- Tray icon follows the rules in section 9 on both light and dark taskbars.

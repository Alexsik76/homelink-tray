# HomeLink — UI design handoff, dark theme addendum

Companion to `ui-handoff.md`. That document stays valid as written; this one completes the dark theme. Where the two differ on dark-theme values, this addendum wins. It also replaces the "Dark theme is optional" note and closes the "Missing dark-theme values" item in section 10 of the main document.

Layout, sizes, typography, spacing and behaviour are identical in both themes. Only colours change.

## 1. Complete dark token set

| Token | Dark | Use |
|---|---|---|
| `bg` | `#17181A` | panel / window background |
| `bg-side` | `#1D1E21` | tunnel column, settings navigation |
| `surface` | `#202124` | submenu buttons, inputs, lists, secondary buttons |
| `line` | `#34363B` | separators, control borders |
| `line-strong` | `#4A4D52` | input and secondary-button borders |
| `frame` | `#5A5D63` | 1 px window border |
| `ink` | `#ECECE8` | primary text, confirm and primary-button fill |
| `ink-2` | `#B4B7BC` | secondary text |
| `muted` | `#9A9DA2` | captions, hints, details labels |
| `disabled` | `#5C5F64` | inactive text |
| `hover` | `#24262A` | hover fill |
| `pressed` | `#303238` | pressed fill |
| `expanded` | `#2A2C30` | expanded element fill |
| `on-ink` | `#17181A` | text and tick drawn on an `ink` fill |
| `link` | `#8FB0FF` | links (hover `#B7CCFF`) |
| `placeholder` | `#8A8D91` | input placeholder text |
| `state-ok` | `#22C55E` | working |
| `state-busy` | `#FACC15` | in progress |
| `state-off` | `#4A78FF` | off; keyboard focus; selected radio; current wizard step |
| `state-unknown` | `#9A9DA2` | unknown (outline only) |
| `state-fail` | `#F0554A` | failure, error text, "Видалити" |

New tokens introduced here: `on-ink`, `link`, `placeholder`. Add their light counterparts so components stay theme-agnostic: `on-ink` `#FFFFFF`, `link` `#1F4FD8` (hover `#143BAA`), `placeholder` `#8A8D91`.

## 2. Inversion rule

In the light theme, emphasis is a dark fill with white text. In the dark theme it inverts: emphasis is an `ink` (light) fill with `on-ink` (dark) text. This applies to:

- submenu button awaiting confirmation ("Так, вимкнути", "Так, у сон");
- primary wizard button ("Далі", "Готово");
- checked checkbox (the tick is drawn in `on-ink`);
- completed wizard progress bars (`ink`);
- selected settings navigation item (3 px `ink` bar on the left edge).

Never use a literal white or black; always go through `ink` / `on-ink`.

## 3. Component notes

Tray panel

- Window border `frame`, background `bg`; tunnel column `bg-side` with a `line` right border; group separators and footer border `line`.
- Element name `ink`; "Без тунелю" caption `muted`; footer button label and icon `ink-2`.

Element states

| State | Rendering |
|---|---|
| normal | transparent fill |
| hover | `hover` fill |
| pressed | `pressed` fill |
| expanded | `expanded` fill + 1 px `ink` border |
| keyboard focus | 2 px `state-off` border |
| inactive | name in `disabled`, marker as 1 px outline `line-strong` |

Submenu button states

| State | Rendering |
|---|---|
| normal | `surface` fill, 1 px `line` border, label `ink` |
| hover | `hover` fill |
| pressed | `pressed` fill |
| inactive | label 12/400 `disabled` |
| awaiting confirmation | `ink` fill and border, label `on-ink` |
| keyboard focus | 2 px `state-off` border |

Details window

- Border `frame`, background `bg`, header separator `line`, close cross `ink`.
- Labels `muted`, values `ink`, non-empty last error `state-fail`.

Settings window

- Navigation `bg-side` with a `line` right border; selected item uses a `bg` fill, `ink` bar, 13/600.
- Add buttons: `surface` fill, 1 px `line-strong` border, label `ink`.
- Lists: `surface` fill, 1 px `line` border and row dividers; secondary text `muted`; "Змінити" `ink`; "Видалити" `state-fail`.
- About: body text `ink-2`, licence lines `muted`, links `link`. The glyph outline is `ink`.

Wizard

- Progress bars: completed `ink`, current `state-off`, upcoming `line`.
- Input: `surface` fill, 1 px `line-strong` border, text `ink`, placeholder `placeholder`. Focused: 2 px `state-off` border. Invalid: 1 px `state-fail` border and error text in `state-fail`.
- Radio row: `surface` fill, 1 px `line` border; box border `state-unknown`. Selected: row border `state-off`, box border `ink`, inner 8 × 8 square `state-off`.
- Checkbox: checked is an `ink` fill with an `on-ink` tick; unchecked is a `surface` fill with a 1 px `state-unknown` border.
- Secondary button: `surface` fill, 1 px `line-strong` border, label `ink`. Primary button: `ink` fill, label `on-ink`.
- Check list on the last step: `surface` fill, `line` border and dividers, result text `ink-2`.

Tray icon

- Unchanged from section 9 of the main document. The icon follows the taskbar theme, not the application theme: outline `#F2F2F2` and the dark state colours on a dark taskbar, outline `#1A1A1A` and the light state colours on a light taskbar.

## 4. Not specified — ask before implementing

- How the theme is chosen: follow the Windows app theme, a manual switch in settings, or both. No switch control was designed.
- Whether the theme changes live while the panel is open or only on next open.

## 5. Acceptance checklist

- Every colour in every window resolves through a token; switching the theme changes no layout.
- No literal white or black remains in components.
- Confirmation and primary buttons are light-on-dark-inverted in the dark theme and stay readable.
- Hover, pressed, expanded and focus states are distinguishable from each other on both `bg` and `bg-side`.
- Marker colours remain distinguishable on `bg`, `bg-side` and `surface`.
- The tray icon is checked against both taskbar themes independently of the application theme.

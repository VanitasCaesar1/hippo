# Hippo.app: icons, agent marks and controls

Status: accepted, built in 0.7.1. Amends [app-night-map.md](app-night-map.md): agent badges become agent marks, and the app's controls share one set of sizes and states.

## Why

0.7.0 drew each agent as two letters on a coloured box (CC, Cx, Cu, Gm), drew some icons by hand, used text characters as icons (▸ ▾ ✓ × ← →, ⌘ and ↩ in buttons), and built buttons in two kits with different heights, radii and states. The result looked homemade.

## Decisions

- **Agent marks.** Each agent is drawn with its own mark, from LobeHub's icon set (`@lobehub/icons-static-svg`, MIT): Claude Code (Claude's own mark, since 0.7.2; 0.7.1 used LobeHub's Claude Code mascot, which at 16 points read as a terminal prompt), Codex, Cursor, Gemini CLI, OpenCode, DeepSeek Harness (DeepSeek's mark), Muse Code and Command Code. An unknown agent gets Lucide's `terminal`. Marks are one colour, in the text colour around them (muted in rows and cards), at 14, 16 or 20 points. Sources and licence: `brand/agents/README.md`. No two-letter agent badge is left, and since 0.7.2 the views' data has no letter form to draw one from. The list of known agents is `KNOWN` in `crates/hippo-view/src/night/mod.rs`; a test in the app and `docs/site/check-agent-marks.py` (run by the site build) fail if any of them is drawn without its mark.
- **One icon set.** Every UI icon is Lucide (ISC) at stroke width 1.75, vendored as SVG files. The night map's hand-drawn icons are replaced by Lucide equivalents (`sunrise`, `shield`, `scroll-text`, `receipt`, …), and text glyphs used as icons by Lucide icons: disclosure triangles by `chevron-right`/`chevron-down`, ✓ by `check`, × by `x`, the day arrows by chevrons, and shortcut keys by `command`, `option`, `arrow-big-up`, `corner-down-left`, `delete` and the arrows. A Markdown list bullet in an answer stays a text bullet.
- **Controls.** The night kit holds the controls; the older kit's button, key caps and segmented switch follow the same spec:
  - heights 28, 32 and 36 points (`Size::Sm`, `Md`, `Lg`), radius 8 (6 inside a segmented track), padding 10, 12 and 14;
  - states: hover, pressed, a 2-point keyboard focus ring (`focus_visible`), disabled (dimmed, no pointer);
  - every control is a tab stop, in reading order;
  - `icon_button` has an accessible name and a tooltip that shows its shortcut (`Zoom in ⌘=`);
  - `kbd` draws a shortcut as key caps with Lucide icons;
  - `select` is a compact trigger (a lead, the value, up-down chevrons), and `menu`/`menu_row` a floating list with a check on the chosen row;
  - the older views' text switches (`PULSE LANES ROWS`, in capitals and SF Mono) became a segmented control in sentence case.
- **The Ask bar's runner** is a select: the agent's mark and name (`Claude Code`). It opens a menu of the configured runners; choosing one runs `hippo config set role.ask <runner> --yes`, and the menu says how to add another (`hippo runners add`).
- **Monospace** is for code, paths, commands, guard targets and receipts. Times, figures, percentages and shortcuts use the system font.

## gpui-kit / gpui-component: not adopted

LongBridge's `gpui-component` (now `gpui-kit`, Apache-2.0) has the components above. Its 0.10.0 release depends on `gpui-pre =0.3.12`, a crates.io snapshot of GPUI, while the app pins Zed's `gpui` from git at `c84af5b` (Zed v1.27.0). The two are different crates whose types do not mix, so using the kit means moving the whole app onto `gpui-pre`, with its own API changes, and the kit also brings `reqwest` (an HTTP client) into the app, which hippo's [safety page](../is-it-safe.md) says the app does not have. Not worth it for a dozen controls; the app keeps its own, to the spec above. Revisit if the app moves to a GPUI release the kit supports.

## What was replaced

| Was | Now |
|---|---|
| Two-letter agent badges (Morning evidence and rules, Brain evidence and reads, Agents accounts, sessions and recent, Guard rows, the Ask bar) | Agent marks |
| 30 hand-drawn night-map icons (`n-*.svg`) | Lucide icons |
| The Ask bar's coloured `CC Claude Code answers` box | A select with the mark and the runner's agent, and a runner menu |
| Brain's `−` and `+` text buttons | Icon buttons with tooltips and shortcuts |
| `⌘P` in SF Mono in the search field, `⌘↩`/`↩` in SF Mono in buttons | Key caps and inline keys drawn with Lucide icons |
| ▸ ▾ in the session view's steps and harness rows and Manage's health and doctor lists | `chevron-right` / `chevron-down` |
| ✓ in the brief's checkboxes, × in Manage | `check`, `x` |
| ← → in Lanes' day navigation, `← Queue` in Pulse | Chevron icon buttons, `arrow-left` |
| The underlined capital SF Mono switches (Fleet, Lanes, Manage) | A segmented control |
| Two button kits with 26/30 and 28-point heights, radius 3 and 8 | One spec: 28/32/36, radius 8, focus ring, tab stop, disabled |
| SF Mono for times, figures and percentages in the night views | The system font |

Since 0.7.2 the older views' small capital captions (`kit::cap`) are in the system font too, medium weight. Some timestamps there are still SF Mono.

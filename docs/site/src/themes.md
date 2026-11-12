# Themes

hippo's web pages take their look from one stylesheet, which is the web theme contract. The registry site and this documentation both use it. A theme changes the look by setting the contract's custom properties, such as `--hp-page-bg` and `--hp-text`. The full list of properties, class names and rules is in [`docs/themes.md`](https://github.com/VanitasCaesar1/hippo/blob/main/docs/themes.md) in the repository. This page does not repeat it.

The stylesheet and its checks are in the `hippo-theme` crate. No site that lets a reader pick a theme is running: the feed, which would do that, is not deployed ([Limits](limits.md)).

## Personal CSS

Personal CSS is for you alone. It may be any CSS: set the contract's properties, or write rules against the contract's class names. Use the class names in the contract and not the page's other markup, which can change in any release.

On the feed, a member would save personal CSS in settings and only that member would see it. The feed is not deployed, so there is nowhere to save it today. A small example, in the contract's terms:

```css
:root {
  --hp-size-text: 0.875rem;
  --hp-page-bg: #e8e8e8;
}
```

## Theme packs

A theme pack shares a look with other people, so it is limited to values. It is a pack with `kind = "theme"` in `pack.toml`. It holds only `pack.toml`, `README.md`, a licence, and one or both of:

- `theme.css`, a web theme under the contract;
- `theme.json`, a theme for the desktop app, in the desktop theme format.

It has no passes, prompts, personas or fixture tests. The theme rules are its checks.

What `theme.css` may hold:

- only `:root { ... }` blocks, at the top level or inside `@media (prefers-color-scheme: light)` or `@media (prefers-color-scheme: dark)`;
- in a block, only custom properties from the contract, each at most once, without `!important`;
- values from a closed grammar for each property: `#rgb` or `#rrggbb` colours, system font names, and sizes and widths in a range;
- no `url(`, no `@import` and no `@font-face`, so a theme loads no image or font.

`theme.json` may hold only the format's keys and value types, with no URL or file path. hippo applies the same hidden-character rules to it as to every pack file.

A theme must also keep the contrast floors in every colour scheme it sets: 4.5:1 for text pairs and 3:1 for controls and focus outlines. A property it leaves out keeps the `default` theme's value.

To check a file from Rust, with the crate:

```rust
let css = std::fs::read_to_string("theme.css")?;
hippo_theme::check_theme_pack_css(&css)?; // an error reads `theme.css:<line>: <what>`
```

`hippo add` applies these rules before it writes anything, and the registry applies them again. A pack review judges what no rule can: whether a theme imitates another product's branding, or does not do what its README says.

## Install one

```sh
hippo add ./packs/paper
```

or, from a built index, `hippo add paper --registry registry/out/index.json` ([The registry](registry.md)). hippo copies the files into `.hippo/packs/paper/`, as for any pack. Adding a theme pack changes no page by itself.

Hippo.app lists the desktop themes of the workspace's installed packs in its theme picker (⌘⇧T), beside the built-in themes and your own theme files. The web themes are not used by anything running yet: the feed, which would offer a theme pack to a signed-in member, is not deployed.

## The three first-party theme packs

Each is in `packs/` with a README. All three are by Vanitas Caesar, under the MIT licence.

| Pack | Web (`theme.css`) | Desktop (`theme.json`) |
|---|---|---|
| `high-contrast` | Text and controls at 7:1 or more, in the light scheme and the dark scheme. It sets no size, font or width. | High Contrast Light and High Contrast Dark. |
| `large-type` | Larger text sizes, a line height of 1.6, a wider content block and the contract's dark colours for a device set to dark. | None. |
| `paper` | A cream page, brown-grey text and a serif font, with a light scheme only. Every text pair is at 5.8:1 or more. | One light theme, Paper. |

Three theme packs are built. All three are listed in [the registry](registry.md).

## Desktop themes

Hippo.app takes its colours, font and size from a theme. Two are built in, Hippo Light and Hippo Dark, and **System** follows macOS's appearance. You can add your own as JSON files directly in `$HIPPO_HOME/themes/` (`~/.hippo/themes/`), and a theme pack can carry one as `theme.json`. Themes are data: no code, includes or URLs.

A theme file is a theme family in the JSON format of the Zed editor's themes, version 0.2.0, with one optional `hippo` object in each theme for `font_family`, `font_size` (10 to 24, default 13) and, in your own files only, a `background` image with an `opacity`. Your own files are read leniently, so a Zed theme family works as it is; a theme pack's `theme.json` is checked strictly, by `hippo add` and again by the app. A colour is `#rrggbb` or `#rrggbbaa`.

The app reads these 31 style keys and ignores every other key. Semicolons separate the colours of one row; commas separate the keys tried for one colour, in order. When none is present, the colour comes from the built-in theme of the same appearance, with a warning in the picker.

| What the app draws | Style keys, tried in order |
|---|---|
| Window background | `background` |
| Dock | `panel.background`, `surface.background` |
| Centre pane | `editor.background`, `background` |
| Title bar | `title_bar.background` |
| Status bar | `status_bar.background` |
| Tab bar; active tab; inactive tabs | `tab_bar.background`; `tab.active_background`; `tab.inactive_background` |
| Pickers and the confirmation | `elevated_surface.background`, `surface.background` |
| Text; secondary text; placeholder; links | `text`; `text.muted`; `text.placeholder`; `text.accent` |
| Borders; subtle borders | `border`; `border.variant` |
| Focus ring; selected row's outline | `border.focused`; `border.selected` |
| Row under the pointer; selected row | `ghost_element.hover`, `element.hover`; `ghost_element.selected`, `element.selected` |
| Removed text and its background | `deleted`, `deleted.background` |
| Added text and its background | `created`, `created.background` |
| Errors; warnings; success | `error`; `warning`; `success` |
| Search match | `search.match_background` |
| Scrollbar thumb | `scrollbar.thumb.background` |

The built-in themes meet WCAG AA, which a test checks. A theme you add is not checked: if one is hard to read, choose "Theme: Use System" in the View menu or the command palette, or hold Option while the app starts.

## See your page in a theme

Build the registry site (`cargo run -p hippo-registry -- build --out registry/out`), open `registry/out/packs.html`, and open the page at 360 pixels wide. Look for anything that scrolls sideways or is hard to read.

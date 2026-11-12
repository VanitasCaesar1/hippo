# Writing a web theme

hippo's web pages (the feed and the registry site) share one stylesheet from the `hippo-theme` crate. A theme changes the look by setting the contract's custom properties. The contract is below: every property, every class name, the layer order and the rules for themes. The stylesheet itself is `crates/hippo-theme/css/base.css`.

The feed is not deployed, so nothing serves these pages yet; the registry site and the documentation site use the contract today.

## Personal CSS

Personal CSS is for you alone. On the feed you save it in settings, and only you see it. It may be any CSS: set the contract's properties, or write rules against the contract's class names. It is accepted when it is valid UTF-8, at most 64 KiB and holds no control characters other than tab and line breaks. Settings, sign-in, moderation and admin pages ignore it, so you can always undo a mistake.

A small example that makes the text larger and the page background a flat grey:

```css
:root {
  --hp-size-text: 0.875rem;
  --hp-page-bg: #e8e8e8;
}
```

Use the class names in the contract, not the page's other markup. Other markup can change in any release; the classes stay until the contract's version changes.

## Theme packs

A theme pack shares a look with other people, so it is limited to values. Its `theme.css` may hold only `:root { … }` blocks, at the top level or inside `@media (prefers-color-scheme: light)` or `@media (prefers-color-scheme: dark)`. Each block sets properties from the contract, each at most once, without `!important`. Colours are `#rgb` or `#rrggbb`; fonts are system font names; sizes and widths have ranges. No `url(` is accepted, so a shared theme cannot carry images or fonts; a background image stays possible in personal CSS. The grammar for each property is in the [shared theme rules](#shared-theme-rules).

A theme must also keep the contrast floors in every scheme it sets: 4.5:1 for text pairs, 3:1 for controls and focus outlines. A property you leave out keeps the `default` theme's value.

Check a file with the crate:

```rust
let css = std::fs::read_to_string("theme.css")?;
hippo_theme::check_theme_pack_css(&css)?; // an error reads `theme.css:<line>: <what>`
```

`hippo_theme::contrast("#767676", "#f4f4ee")` gives the ratio of two colours.

The pack's manifest and review are described in [Packs](site/src/packs.md) and [The registry](site/src/registry.md).

## Trying a theme locally

You can look at the stylesheet by itself: `BASE_CSS` (which is also the `default` theme), `DARK_CSS` and `AUTO_CSS` are plain strings in the crate. When the feed runs, paste a theme's values into your personal CSS in settings to try them, then open the pages at 360 pixels wide and look for anything that scrolls sideways or is hard to read.

## Layers

A page links, in this order, all from its own origin:

1. `base.css`: the layout of every contract class, reading only contract properties. Its `:root` block gives every property the `default` theme's value.
2. At most one theme: none for `default`, since `base.css` already holds its values; `dark.css`; `auto.css`; or a theme pack's `theme.css`. A theme sets custom properties under `:root`, and a property it leaves alone keeps the `default` value from `base.css`. A built-in theme may also add rules; a theme pack may not ([Shared theme rules](#shared-theme-rules)).
3. On the feed, for a signed-in member who has saved some, their personal CSS.

The later layer wins where they disagree. Pages contain no `<style>` element and no `style` attribute, which `style-src 'self'` would refuse anyway. The URLs of the base stylesheet and the built-in themes carry `?v=` and the first 12 hex digits of the file's SHA-256, and a theme pack's URL is versioned by the pack hash in its path, so these can be cached for a year and a release changes the URL. Personal CSS is revalidated on each use instead ([Personal CSS](#personal-css)).

## The contract (version 1)

### Custom properties

| Property | Meaning | `default` | `dark` |
|---|---|---|---|
| `--hp-page-bg` | page background outside the content block | `#ffffff` | `#121212` |
| `--hp-content-bg` | the content block's background | `#f4f4ee` | `#1b1b1b` |
| `--hp-text` | body text | `#111111` | `#e8e6e1` |
| `--hp-muted` | ranks, domains, meta lines, secondary text | `#5f5f5f` | `#a6a6a6` |
| `--hp-link` | titles and links | `#111111` | `#e8e6e1` |
| `--hp-link-visited` | visited titles | `#5f5f5f` | `#a6a6a6` |
| `--hp-bar-bg` | the top bar | `#1e2a4a` | `#1e2a4a` |
| `--hp-bar-text` | text and links in the top bar | `#ffffff` | `#ffffff` |
| `--hp-bar-focus` | focus outline inside the top bar | `#ffffff` | `#ffffff` |
| `--hp-vote` | the upvote arrow | `#767676` | `#8c8c8c` |
| `--hp-focus` | focus outline elsewhere | `#1e2a4a` | `#a9bdf0` |
| `--hp-rule` | hairline rules and table borders | `#d6d6cc` | `#333333` |
| `--hp-code-bg` | code blocks and the install command | `#e9e9e2` | `#262626` |
| `--hp-input-bg`, `--hp-input-text`, `--hp-input-border` | form fields | `#ffffff`, `#111111`, `#767676` | `#232323`, `#e8e6e1`, `#8c8c8c` |
| `--hp-error` | error messages | `#a50e1c` | `#ff8f85` |
| `--hp-font` | text | `Verdana, Geneva, "DejaVu Sans", sans-serif` | same |
| `--hp-font-mono` | commands, hashes, code | `ui-monospace, Menlo, Consolas, monospace` | same |
| `--hp-size-title` | row titles and the top bar | `0.8333rem` (10 pt) | same |
| `--hp-size-text` | post and comment text | `0.75rem` (9 pt) | same |
| `--hp-size-meta` | meta lines and ranks | `0.6875rem` (11 px) | same |
| `--hp-line` | line height | `1.35` | same |
| `--hp-content-width` | the content block's width on wide screens | `85%` | same |
| `--hp-indent` | indent per comment level on wide screens | `40px` | same |
| `--hp-mark-size` | the mark's height in the top bar | `18px` | same |
| `--hp-color-scheme` | the value given to `color-scheme`, so form controls match | `light` | `dark` |

### Contrast pairs

Every built-in and shared theme meets these floors. The values above meet them; the lowest is `--hp-vote` on `default`'s content background, 4.1:1.

| Foreground | Background | Floor |
|---|---|---|
| `--hp-text`, `--hp-muted`, `--hp-link`, `--hp-link-visited`, `--hp-error` | `--hp-content-bg` | 4.5:1 (text) |
| `--hp-text`, `--hp-muted` | `--hp-code-bg`, `--hp-page-bg` | 4.5:1 (text) |
| `--hp-input-text` | `--hp-input-bg` | 4.5:1 (text) |
| `--hp-bar-text` | `--hp-bar-bg` | 4.5:1 (text) |
| `--hp-vote`, `--hp-focus` | `--hp-content-bg` | 3:1 (controls and focus) |
| `--hp-input-border` | `--hp-input-bg` | 3:1 (controls and focus) |
| `--hp-bar-focus` | `--hp-bar-bg` | 3:1 (controls and focus) |

### Class names

Each class sits on the element named and holds what is described; markup outside these classes may change in any release.

| Class | Element | What it is |
|---|---|---|
| `hp-page`, `hp-page-<name>` | `body` | every page, with the class naming the page from [Page names](#page-names) |
| `hp-skip` | `a` | the skip link to `main` |
| `hp-content` | `div` | the centred block holding the top bar, `main` and the footer |
| `hp-topbar` | `header` | the top bar |
| `hp-home`, `hp-mark`, `hp-name` | `a`, `svg`, `span` | the link home, holding the mark (inline SVG drawn in `currentColor`, so it takes the top bar's text colour) and the name "hippo" |
| `hp-nav`, `hp-nav-current`, `hp-sep` | `nav`, `a`, `span` | the navigation, the current page's link, and the ` \| ` separators, which are text |
| `hp-account`, `hp-karma` | `div`, `span` | the right end of the top bar: the member's name and karma and the sign-out button, or the sign-in link |
| `hp-main`, `hp-footer` | `main`, `footer` | the page's content and footer |
| `hp-items` | `ol` | a list of posts |
| `hp-item`, `hp-item-pack`, `hp-item-discussion`, `hp-item-theme` | `li` | one row, with the class of its kind |
| `hp-rank` | `span` | the row's number |
| `hp-vote`, `hp-vote-button` | `form` or `span`, `button` or `a` | the row's arrow column: a form holding the ▲ `button` for a member who may vote; for a visitor signed out, a `span` holding the ▲ as a link to sign-in; otherwise an empty `span` that keeps the column |
| `hp-title`, `hp-title-link`, `hp-domain` | `span`, `a`, `span` | the title, its link, and the grey domain in parentheses |
| `hp-meta` | `div` | the grey line under a title |
| `hp-points` | `span` | a meta line's points |
| `hp-user` | `span` | a meta line's author or owner |
| `hp-age` | `a` | a meta line's age, linking to the post |
| `hp-evidence` | `span` | a pack's evidence or its count of measurements |
| `hp-install` | `a` | the link to a pack's install box |
| `hp-comments-link` | `a` | the link to a post's comments |
| `hp-link-button` | `button` | a button drawn as a link, for actions that are forms (hide, use, sign out) |
| `hp-more` | `a` | the link to the next page |
| `hp-post`, `hp-post-body` | `article`, `div` | a post page's post and its text |
| `hp-pack-box`, `hp-install-box`, `hp-passes` | `section`, `section`, `ul` | a pack's details, its install command and link, and what its passes read |
| `hp-comments`, `hp-comment`, `hp-children` | `ul`, `li`, `ul` | top-level comments, one comment, and its replies |
| `hp-comment-meta`, `hp-comment-body` | `div`, `div` | a comment's grey line and its text |
| `hp-md` | `div` | rendered Markdown |
| `hp-form`, `hp-field`, `hp-label`, `hp-input`, `hp-textarea`, `hp-select`, `hp-button`, `hp-help`, `hp-error` | `form`, `div`, `label`, `input`, `textarea`, `select`, `button`, `p`, `p` | forms: a form, one field's wrapper, its label and control, a submit button, help text and an error message |
| `hp-notice`, `hp-placeholder` | `p`, `span` | a page message; `[deleted]`, `[removed: …]` and other placeholders |
| `hp-sr` | any | text for screen readers only, such as a list page's `h1` |
| `hp-table` | `table` | data tables: measurements, the moderation queue, the admin page |
| `hp-code` | `pre` | the install command and other code to copy |

### Page names

The `<name>` of `hp-page-<name>`, listed in the crate's `PAGES` (not in `CLASSES`):

| Name | Page |
|---|---|
| `front`, `new`, `past`, `comments`, `ask`, `show`, `packs`, `evidence`, `themes` | the feed's lists |
| `post` | a pack's or a discussion's page |
| `submit`, `edit`, `reply`, `measure`, `report` | the feed's forms |
| `settings`, `hidden`, `login` | a member's pages and sign-in |
| `moderation`, `mod`, `admin` | the public log; the moderators' pages (`/mod/u/<login>` included); the admin pages |
| `about`, `error` | the about page; every error page |
| `registry-index`, `registry-themes`, `registry-pack` | the registry site's pages |
| `docs` | every page of the documentation site (additive, so the contract stays version 1) |

### Every page

Every page is `<!doctype html>` with `<html lang="en">`, `<meta charset="utf-8">`, `<meta name="viewport" content="width=device-width, initial-scale=1">`, a `<title>` of the page's name followed by ` | hippo`, and hippo's app icon (`brand/hippo-icon.svg`, the mark on ink blue) as its icon. The viewport line is what makes the 360-pixel layout hold on phones. The mark in the top bar sits beside the visible name "hippo", so it is hidden from screen readers.

### Versioning

The base stylesheet begins `/* hippo web theme contract 1 */`. Adding a property, a class or a page name keeps the version. Removing or renaming one, moving a class to another element, or changing the layer order raises it, and is a change to this page. A theme pack declares the version it targets.

An additive change must not break a published theme: a theme that sets only some properties relies on the defaults of the rest, so a new property's default must keep every listed theme within the [shared theme rules](#shared-theme-rules). The registry re-checks every listed theme when it takes a new `hippo-theme`. Raising the version is a design change, and the design states its migration.

## Shared theme rules

A theme pack's `theme.css` reaches everyone who picks it, so it may only set the contract's values and cannot add rules. It is accepted when it is valid UTF-8, at most 64 KiB and holds no control characters other than tab and line breaks, and, with comments removed:

1. It holds only `:root { … }` blocks, at the top level or inside `@media (prefers-color-scheme: light) { … }` or `@media (prefers-color-scheme: dark) { … }`; no other selector or at-rule, and no nested `@media`.
2. Each block holds only declarations of contract properties, each at most once per block, without `!important`; the last may omit its `;`.
3. Each value fits its property's form:

| Properties | Form |
|---|---|
| the colours | `#rgb` or `#rrggbb`, hex digits in either case |
| `--hp-font`, `--hp-font-mono` | a comma-separated list of family names, at most 200 characters. A quoted name holds ASCII letters, digits, single spaces, hyphens and dots. An unquoted name is words separated by single spaces, each starting with a letter and holding only ASCII letters, digits and hyphens. Neither is `inherit`, `initial`, `unset`, `revert` or `revert-layer` |
| `--hp-size-title`, `--hp-size-text`, `--hp-size-meta` | from 10 to 32 pixels, in `px`, `rem`, `em` or `pt` |
| `--hp-line` | a number from 1 to 2.5 |
| `--hp-content-width` | from 50% to 100%, or from 320 to 2,000 `px` |
| `--hp-indent` | `0`, or from 0 to 40 pixels in `px`, `rem` or `em` |
| `--hp-mark-size` | from 10 to 48 `px` |
| `--hp-color-scheme` | `light` or `dark` |

   A number is digits with an optional fraction (`12`, `0.5`, `.5`), with no sign or exponent; units are lower case; for the ranges, `rem` and `em` count as 16 pixels and `pt` as 4/3 of a pixel. Property names are case-sensitive, as custom properties are in CSS. An empty file, or an empty block, is accepted.
4. The [contrast pairs](#contrast-pairs) hold in the light and in the dark scheme, as a browser computes them: the light scheme is the `default` values, then every top-level block and every light `@media` block in file order, a later declaration winning; the dark scheme likewise with the dark `@media` blocks. A file with no `@media` block has one scheme.

A failure displays as `theme.css:<line>: <what>`, on the line of the declaration at fault (for a contrast failure, the foreground's declaration, or the background's when the file does not set the foreground); `<what>` is the crate's message, which its tests pin. The built-in `dark` and `auto` themes pass these rules; the base stylesheet is not a theme and is not checked by them.

So a shared theme changes colours, system fonts, sizes and widths. It cannot add text or images, hide or move a control, load anything, or set the contract's colour pairs below their floors. It can still make a page harder to read within those limits (small sizes, a symbol font, low contrast between pairs the contract does not list); the member who chose it switches back in settings, and anyone can report the pack.

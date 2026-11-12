# Large type

A theme that makes the text on the registry and feed pages larger and gives it a taller line, for reading.

## What it changes

- **Web (`theme.css`)**: the three text sizes (titles and the top bar at 20 pixels, post and comment text at 18, meta lines at 16), the line height (1.6), the content width (90% on wide screens), the indent per comment level and the size of the hippocamp mark. It also gives the contract's own dark colours to a device set to dark, so a page that used `auto` before still follows the device. In the light scheme the colours are the contract's defaults.
- **Desktop**: nothing. It has no `theme.json`; Hippocamp.app's text size is its own setting and its theme's `font_size`.

## Nothing in it runs

A theme is data. `theme.css` holds only `:root` blocks that set the contract's custom properties, with no selector of its own, no import, no URL and no font to download. `hippocamp add` checks it against the rules for shared themes before it writes anything, including the contrast floors, and a pack review reads it too.

## How to choose it

The feed is not deployed yet, so to see the web theme, build the registry site from a checkout (`hippocamp-registry build`) and add the lines of `theme.css` to the end of the `hippocamp.css` it writes.

- **The feed** (not deployed): when the feed lists this pack, its `/themes` page offers it with a *use* button for a signed-in member. Your own pages then use it in place of a built-in theme. The sign-in, settings and admin pages keep a built-in theme, so you can always get back.
- **Hippocamp.app**: it reads desktop themes only, so it does not list this pack.

## Notes

Larger type makes lines wrap sooner, which is the point. A browser's own text size setting scales these sizes too, because they are in `rem`.

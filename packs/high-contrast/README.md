# High contrast

A theme that sets text and controls at a contrast of at least 7:1, for the registry and feed pages in a browser and for Hippo.app.

## What it changes

- **Web (`theme.css`)**: the colours of the web theme contract. In the light scheme and in the dark scheme, which it follows from the device, every text pair of the contract (text, muted text, links, visited links and error text on the content, code and page backgrounds; input text on its field; the top bar's text on its bar) has a contrast of 7:1 or more. The focus outlines, the upvote arrow and the field borders are at least 7:1 as well. It sets no size, font or width.
- **Desktop (`theme.json`)**: two themes for Hippo.app, High Contrast Light and High Contrast Dark, with text and muted text at 7:1 or more on every surface the app draws, focus rings in blue (`#0000cc`) in the light theme and yellow (`#ffff66`) in the dark, and strong borders. The text size is 14.

## Nothing in it runs

A theme is data. `theme.css` holds only `:root` blocks that set the contract's custom properties, with no selector of its own, no import, no URL and no font to download. `theme.json` holds only names, colours and one number, the text size. `hippo add` checks both files with the rules that guard each format before it writes anything, and a pack review reads them too.

## How to choose it

Hippo.app's theme picker lists it now. The feed is not deployed yet, so to see the web theme, build the registry site from a checkout (`hippo-registry build`) and add the lines of `theme.css` to the end of the `hippo.css` it writes.

- **Hippo.app**: after `hippo add high-contrast`, open the theme picker (Command-Shift-T) and pick High Contrast Light or High Contrast Dark. The picker lists the themes of installed theme packs after the built-in ones and your own files.
- **The feed** (not deployed): when the feed lists this pack, its `/themes` page offers it with a *use* button for a signed-in member. Your own pages then use it in place of a built-in theme. The sign-in, settings and admin pages keep a built-in theme, so you can always get back.

## Notes

The pack sets colours only, so the layout, sizes and fonts stay the contract's. It does not change what a page says.

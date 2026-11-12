# Paper

A warm, low-glare light theme: cream pages, brown-grey text and a serif face, for the registry and feed pages in a browser and for Hippo.app.

## What it changes

- **Web (`theme.css`)**: the colours of the web theme contract (a cream page, darker cream code blocks, brown links, an umber top bar), the font (Georgia, then Times New Roman, then the system serif), slightly larger text and a line height of 1.5. It has one light scheme and does not follow a device set to dark. Every text pair of the contract keeps a contrast of at least 5.8:1.
- **Desktop (`theme.json`)**: one light theme, Paper, with the same cream and brown colours and Georgia at size 14. Hippo.app uses the font only when it is installed, and otherwise the system font.

## Nothing in it runs

A theme is data. `theme.css` holds only a `:root` block that sets the contract's custom properties, with no selector of its own, no import, no URL and no font to download. `theme.json` holds only colours, one font name and one number. `hippo add` checks both files with the rules that guard each format before it writes anything, and a pack review reads them too.

## How to choose it

Hippo.app's theme picker lists it now. The feed is not deployed yet, so to see the web theme, build the registry site from a checkout (`hippo-registry build`) and add the lines of `theme.css` to the end of the `hippo.css` it writes.

- **Hippo.app**: after `hippo add paper`, open the theme picker (Command-Shift-T) and pick Paper. The picker lists the themes of installed theme packs after the built-in ones and your own files.
- **The feed** (not deployed): when the feed lists this pack, its `/themes` page offers it with a *use* button for a signed-in member. Your own pages then use it in place of a built-in theme. The sign-in, settings and admin pages keep a built-in theme, so you can always get back.

## Notes

If you read in the dark, pick a dark theme instead: this one stays light.

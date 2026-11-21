# Brain: links that read, an overview that fills the view, a drag that feels physical

Status: accepted 21 Nov 2026, to build in 0.8.3; amended the same day after the third review (docked panel, refit on drop, badges in the gutter). Amends [brain-arrange.md](brain-arrange.md) and [app-night-map.md](app-night-map.md) › Brain.

## Background

0.8.2 drew every link at every zoom. On a 300-note map with 1,000 links that was a mesh under the cards, and links that had to cross a column of cards fell into a band of detours below the map. The entry note, linked from most notes, fanned out at its edge. Zoomed out, a 300-note map was a strip across the top of the window. A dragged block was an empty outline, and the zoom bar and the Ask bar floated over the map.

## Rules

### Lines

- **No line at rest.** Brain draws the lines of one card: the one under the pointer, else the selected note (at Out, the card of its folder). Moving the pointer from a card onto one of its lines keeps that card in focus. While another card is hovered, the selected note's lines stay as they are, with their heads; the hovered card's are drawn over them.
- **A routing a person can follow: a bundle per side, counted, with arrowheads.** A card's lines leave it at one port per side, as one trunk to the right (which also serves cards in its own column) and one to the left. Each trunk drops a branch in the gap beside every column it reaches; the branch splits into short stubs, one per card. Lines that run together share their points exactly, so they draw as one line, never as a doubled one. Where a branch that stands for more than one link leaves its trunk, a round badge gives its count (`6`). Each line ends in an arrowhead at the card it points to (the focus links to it), and one points into the focus where links come back. So at most two trunks show on any map, and every stub says where it goes and which way.
- **Every line drawn keeps its head**, in every state: at rest, under hover, faint during a drag, in both renderers.
- **Heads never meet across a gutter.** Lines meet a card's left edge a little above its middle and its right edge a little below (12 % of an Out card's height, 10 points at Mid and In), so lines into the cards either side of one gap arrive at different heights. Gaps between columns are 112 points (about 50 px on the 300-note overview).
- **Straight where it can be.** A line alone at both its ends, between neighbouring columns, runs straight when its ends are within 16 points; points in the middle of a straight run are dropped, so no line draws a bracket or a dog-leg.
- **Badges sit in the gutter.** A badge stands where the branch leaves the trunk, at the gap's middle lane, and is drawn over the cards. hippo records how far it is from the nearest column edge; it is drawn only when that is at least its half-width (9 px, wider for three figures) plus 4 px on screen.
- **Clear of cards and of each other.** Lines turn only in the gaps between columns. A trunk keeps its height across a column while it stays 20 points (Out) or 12 points (Mid and In) from that column's cards, folder name bands and other trunks; else, in the gap before the column, it turns to the middle of the nearest free channel at least twice that high, or 1.8 times the clearance below everything. Frames at Mid and In are 48 points apart, so a channel between two runs down its middle and never along a frame's border. A branch and a turn in one gap share one vertical.
- **Drawn quietly**, from `hippo_view::night::map::STYLE`, which both Hippo.app and the web demo read: one weight, 1.5 px, at every zoom; corners round at 7 px; arrowheads 7 px. Counts are the badges', not the lines' width. One hover rule: the selected note's lines are a quiet neutral (the text colour at 55 % over the ground, mixed to one opaque colour so lines that run together never draw brighter); a hovered card's, and any line under the pointer, are the stroke lime (`#C8F031` on dark, `#7A9A00` on light: a yellower lime than the fixed mark's green). Both stay visible.
- **What a line stands for.** At Out, with the pointer on a line, it turns lime, and the status slot says `START HERE → Style guide, Decisions · 2 links` (up to three names, else `4 others`), with Lucide's arrow. On a badge it says what the badge counts. `Layout::lines_at` answers both, so a badge's number and the status's are one number. Nothing is drawn over the map.

### Cards

- One resting edge for every card. The selection ring stays while another card is hovered; hover adds a stroke-lime edge and a soft lift, at every level.
- One size per level for every card's words: at Out names at 40 points on up to two lines, counts at 26, note rows at 28; at Mid titles at 16, summaries at 14, folder names at 14.5 and their counts at 13 just after the name. A word that does not fit is cut with `…`, never set smaller. Widths are estimated per capital, so `START HERE` takes one line.
- At Out a folder card shows a folder icon, a pinned note its flag (the entry note) or note icon. A folder card's note count heads its legend; its fixed, waiting, rejected and stale counts sit in two columns below.
- On a map of more than 8 blocks a folder card shows its name and its counts only: all four (fixed, waiting, rejected, stale) in their places on every card, a zero with a hollow mark, so no grid has a hole. Nothing is drawn under 11 px on screen: below `11 / 28` the note rows go, below `11 / 26` the counts; at Mid a note card's chips go below 92 %, its summary below 79 %. Mid starts at 70 %.
- A rule waiting is a lime ring, never a filled dot, so it does not read as the fixed mark's green. Folders sort as a person counts (`Folder 2` before `Folder 10`).
- At Mid and In a note read in every session that starts a column sits as low as the first cards in a folder's frame, so card tops line up across columns.
- Closer in (In), a card's summary, rules and evidence wrap to three lines each, with 16 points below the last as above the first; Hippo.app wraps them to the card's inner width as the font sets them, on no more lines than hippo made room for.
- Pills hug their words; in dark a plain pill sits one step above the card's ground.

### The overview fills the view

hippo's layout depends on the view. `view_arrangement(w, h)` lays hippo's layout out (the notes read in every session in a first column of their own, then each folder into the lowest run of columns) in each number of columns from one to one per block, and keeps the one that shows the whole map largest: the one whose Mid fits at 70 % or more, if any, else the one whose Out fits at the largest zoom. Out cards are 72 points apart both ways, so channels between rows are as wide as gaps between columns. Hippo.app does this when Brain opens and on Fit (⌘0); `hippo view map --view 1220x690` prints it. A person's arrangement is never changed by it.

Fit shows the whole map, centred: at Mid when it fits there at 70 % or more (at most 100 %), else at Out at up to 69 %. That zoom is also the smallest: further out the map would only shrink into the middle of an empty view. (A map too big to read at Out even fitted is drawn with names only, per the 11 px rule.) The + and − buttons, and ⌘= and ⌘−, step through one table, `STYLE.steps` (15, 20, 25, 33, 40, 50, 60, 70, 85, 100, 125, 200, 300 %), about the selected card when it is in view; from a fitted zoom between two steps they go to the next one, never below the fit.

### Nothing floats over the map

Brain's header is an opaque band 128 points high with a hairline edge; the map starts below it and ends in a hairline above the Ask bar and another a gutter (18 points) short of the panel, which is a docked column: the map never runs under it, and fitting, centring and clipping use the map's own right edge. The header's first row holds the controls: `Reset layout` (kept in its place, hidden, while the layout is hippo's, so nothing moves when it shows), zoom with Lucide minus and plus and tabular figures, and `Treemap` with Lucide's layout grid. Its second row holds the headline and, right-aligned in a slot of its own, the status: `Runbooks moved. Undo` after a move, or what the line under the pointer stands for. The lead below keeps its whole width. At Mid and In, folders whose name band has scrolled above the view keep their name and count in an opaque strip at the map's top; the cards pass under it.

### Dragging a block

- The lifted block is the real block under the pointer at the point it was grabbed, 3 % larger, with a shadow; in dark it takes a lighter ground and a 1 px ring, so it reads as lifted. A folder's frame gets a solid ground.
- The lifted block is drawn 4 % larger. Where it will land is a dashed slot with a lime tint (12 % and a 75 % lime edge in dark, 8 % in light). As the pointer moves, the other blocks slide (150 ms, ease-out) to open the slot and close the old one.
- While a block is lifted, the focus's lines stay, faint (35 %), with their heads, where they will run after the drop, and stay faint while the dropped block settles.
- On release the block settles (180 ms) and `Runbooks moved. Undo` shows in the status slot for 6 seconds. A view that was fitted (the whole map at its fit zoom) is fitted again to the map as it now is, moving there over the settle; any other keeps its zoom, so cards keep their size under the pointer, and pans only as far as keeps the dropped block in view. On Escape every block flies back (220 ms). ⌥-arrows, Reset and Undo slide too.
- With macOS Reduce Motion on (or `prefers-reduced-motion` in the browser) nothing slides.

### The web demo is the same map

The live demo lays the map out for the view Hippo.app would have in a window as large as the page less the replica's own footer row (its disclaimer, 28 points, below the Ask bar), so it gets the same columns and the same fit zoom. It reads line width, corners, arrowheads, badges, zoom floors and the zoom table from the map's JSON (`style`), routes and counts with the same `lines_at`, and its colours are `brand/tokens.json`, as the app's are: `docs/site/check-tokens.mjs` fails when either drifts. Same chrome (workspace chevron, search, Agents and Guard with their counts, the composer's words and runner chip, `waiting for you`), same Lucide icons, same wordmark (`brand/hippo-mark.svg`), tabular figures in the system font, and the same hover rule.

## How it works

- `hippo_view::night::map`: `Layout.focus` holds each linked card's lines (there is no `lines` field: nothing is drawn at rest) and `Layout.badges` its badges (`Badge`: centre, links, trunk, room, the cards it leads to); `Line.trunk` groups a trunk's branches, `Line.out` and `Line.back` say which way its links run; `STYLE` (`NotesMap.style` in the JSON) is how lines and badges are drawn and the zoom table, with `Style::step`, `badge_half` and `badge_fits`; `Layout::shown(focus)`, `badges(focus)`, `lines_at(focus, x, y, tol, badge)`, `links_of(lines)` and `visible(focus)`. `route` routes the trunks as above. `Mark.row` marks a folder card's note rows. `default_arrangement_in(cols)`, `view_arrangement(w, h)`, `NotesMap::fitted`, `NotesMap::fit`, `NotesMap::preview(arr, level)` and `block_rect_in`. `in_text` and `in_height` wrap In cards.
- Hippo.app: `MapView` gains `hover`, `hover_line`, `slide` (the layout blocks slide to, each block's offset when it started, the dropped block's scale, the drop it previews), `view_size`, `fit_zoom` and `camera` (where the view was when a refit started). The painter draws each block at its pose and asks for another frame while a slide runs. `reduce_motion()` reads `NSWorkspace.accessibilityDisplayShouldReduceMotion`. Dev-only `HIPPO_APP_DEV_HOVER=x,y` (or `line`), `HIPPO_APP_DEV_PROGRESS`, `HIPPO_APP_DEV_SLOW`, `HIPPO_APP_DEV_REDUCE_MOTION` and `HIPPO_APP_DEV_DROP=0` set states for screenshots.
- The demo: `demo/arrange.js` ports `links`, `route` (with its badges), `lines_at`, `links_of`, `Style::step`, `view_arrangement`, `fit` and `preview`; `demo/demo.js` draws each block in a box of its own that slides with a CSS transition. `docs/site/demo-data.sh` writes the map hippo laid out for an 860 × 600 view, and `check-demo-render.mjs` fails if the port chooses other columns or lays it out differently.

## Tests

On maps of 3 to 300 notes, each in its default layout, two random arrangements and fitted to two views, at every level:

- no line enters a card or a folder's name or count, for every focus;
- no level run of a trunk comes within the clearance of a card or name band;
- no line shows at rest, and at most two trunks for any focus;
- every Out card sets its words at one size per kind, never cuts a count, and a fitted overview sets nothing under 11 px;
- every line turns at right angles, its vertical runs in gaps; two trunks never share a lane or a port, and a card has at most one port per side;
- a card with many links leaves as one trunk each way, and lines to one column share every point up to their branch;
- the notes read in every session keep the first column; on the 300-note map folder cards are one height;
- In cards wrap their words and keep 16 points below them;
- the 300-note overview fills more than 60 % of the view both ways; maps of up to 40 notes open whole at 42 % or more;
- a person's arrangement is kept when the map is fitted;
- geometry: no badge comes nearer a card or a frame's name band than its room; every line drawn has a direction and ends level on its card's edge with room for its head at the smallest zoom of its level; a badge's count is what `lines_at` gives under it, which is what the status says; lines into the cards either side of a gutter never meet head to head; the zoom table rises from the smallest zoom to the largest and steps as the app and demo step;
- folders sort as a person counts; `START HERE` takes one line; a big overview keeps every count in its place.

`check-demo-render.mjs` checks the demo's port against hippo's layout (badges included), its zoom steps against Hippo.app's, and its badges against its own `linesAt`. The app's tests cover the drag (the slot opens at the drop's column, Escape slides every block back, a drop keeps a zoomed view's zoom and refits a fitted one), hover focus, ⌘= stepping through the shared table, and ⌘0.

## Not in this

Curved (non-orthogonal) lines; lines between notes in one folder at Out (they are inside one card); a count on hover at Mid and In.

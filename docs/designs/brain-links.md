# Brain: links that read, an overview that fills the view, a drag that feels physical

Status: accepted 21 Nov 2026, to build in 0.8.3. Amends [brain-arrange.md](brain-arrange.md) and [app-night-map.md](app-night-map.md) › Brain.

## Background

0.8.2 drew every link at every zoom. On a 300-note map with 1,000 links that was a mesh under the cards, and links that had to cross a column of cards fell into a band of detours below the map. The entry note, linked from most notes, fanned out at its edge. Zoomed out, a 300-note map was a strip across the top of the window. A dragged block was an empty outline, and the zoom bar and the Ask bar floated over the map.

## Rules

### Lines

- **No line at rest.** Brain draws the lines of one card: the one under the pointer, else the selected note (at Out, the card of its folder). Moving the pointer from a card onto one of its lines keeps that card in focus.
- **One trunk each way.** A card's lines leave it at one port per side, as one trunk to the right (which also serves cards in its own column) and one to the left. Each trunk drops a branch in the gap beside every column it reaches, and the branch splits into short stubs, one per card. So at most two trunks show at once, on any map.
- **Clear of cards and of each other.** Lines turn only in the gaps between columns. A trunk keeps its height across a column while it stays 20 points (Out) or 12 points (Mid and In) from that column's cards, folder name bands and other trunks; else, in the gap before the column, it turns to the middle of the nearest free channel that is at least twice that high, or below everything. A branch and a turn in one gap share one vertical.
- **Drawn quietly.** Corners round at 7 px. At Out a line's width shows the links it stands for, `1 + 0.35 × log2(links)` px, at most 2; below the zoom the map was fitted at it thins with the map, down to 55 %. Closer in, 1.5 px. The selected note's lines are in the text colour; a hovered card's in lime (with a thin darker edge on a light ground). Port rings show only at 60 % and closer, one per trunk and one per card it reaches.
- **What a line stands for.** At Out, with the pointer on a line, it turns lime and a label beside the pointer, clear of the cards where there is room, says `START HERE → Style guide, Decisions · 2 links` (up to three names, else `4 others`), with Lucide's arrow.

### Cards

- One resting edge for every card. The selection ring stays while another card is hovered; hover adds a lime edge and a soft lift.
- At Out, a folder card shows a folder icon, a pinned note its flag (the entry note) or note icon. Below 33 % a folder card drops its note rows, below 27 % its counts: only names are left where text would be under about 8 px.
- On a map of more than 8 blocks a folder card lists 2 notes (and `+ n more`), not 5, so its overview fits the view at a zoom where the rows read.
- Closer in (In), a card's summary, rules and evidence wrap to three lines each, with 16 points below the last as above the first.
- Pills hug their words; in dark a plain pill sits one step above the card's ground.

### The overview fills the view

hippo's layout depends on the view. `view_arrangement(w, h)` lays hippo's layout out (the notes read in every session in a first column of their own, then each folder into the lowest run of columns) in each number of columns from one to one per block, and keeps the one that shows the whole map largest: the one whose Mid fits at 55 % or more, if any, else the one whose Out fits at the largest zoom. Out cards are 72 points apart both ways, so channels between rows are as wide as gaps between columns. Hippo.app does this when Brain opens and on Fit (⌘0); `hippo view map --view 1220x690` prints it. A person's arrangement is never changed by it.

Fit shows the whole map, centred: at Mid when it fits there at 55 % or more (at most 100 %), else at Out at up to 54 %.

### Nothing floats over the map

The map runs from Brain's header to the panel's edge and ends above the Ask bar. Its controls (`Reset layout` while the layout is a person's, its Undo line, zoom with Lucide minus and plus and tabular figures, `Treemap` with Lucide's layout grid) sit in the header's first row. The map's edges fade into the ground, so a card cut by the view's edge does not end in mid-air. At Mid and In a folder whose name band has scrolled above the view keeps its name at the view's top.

### Dragging a block

- The lifted block is the real block under the pointer at the point it was grabbed, 3 % larger, with a shadow; in dark it takes the raised ground, so it reads as lifted. A folder's frame gets a solid ground.
- Where it will land is a quiet dashed slot with an 8 % lime tint, the same in both themes. As the pointer moves, the other blocks slide (150 ms, ease-out) to open the slot and close the old one.
- While blocks move, lines hide; once they stop, the focus's lines show faintly where they will run after the drop.
- On release the block settles (180 ms) and `Runbooks moved. Undo` shows in the header for 6 seconds. On Escape every block flies back (220 ms). ⌥-arrows, Reset and Undo slide too.
- A view that showed the whole map before a drop is fitted again after it; otherwise the dropped block is kept in view.
- With macOS Reduce Motion on (or `prefers-reduced-motion` in the browser) nothing slides.

### The web demo is the same map

The live demo draws the same layout (it chooses columns for the view Hippo.app would have at the same window size), the same lines and weights, the same controls in the same place with the same Lucide icons, and the same tabular figures in the system font.

## How it works

- `hippo_view::night::map`: `Layout.focus` holds each linked card's lines (there is no `lines` field: nothing is drawn at rest); `Line.trunk` groups a trunk's branches; `Layout::shown(focus)` and `visible(focus)`. `route` routes the trunks as above. `Mark.row` marks a folder card's note rows. `default_arrangement_in(cols)`, `view_arrangement(w, h)`, `NotesMap::fitted`, `NotesMap::fit`, `NotesMap::preview(arr, level)` and `block_rect_in`. `in_text` and `in_height` wrap In cards.
- Hippo.app: `MapView` gains `hover`, `hover_line`, `slide` (the layout blocks slide to, each block's offset when it started, the dropped block's scale, the drop it previews), `view_size` and `fit_zoom`. The painter draws each block at its pose and asks for another frame while a slide runs. `reduce_motion()` reads `NSWorkspace.accessibilityDisplayShouldReduceMotion`. Dev-only `HIPPO_APP_DEV_HOVER=x,y` (or `line`), `HIPPO_APP_DEV_PROGRESS`, `HIPPO_APP_DEV_SLOW`, `HIPPO_APP_DEV_REDUCE_MOTION` and `HIPPO_APP_DEV_DROP=0` set states for screenshots.
- The demo: `demo/arrange.js` ports `links`, `route`, `view_arrangement`, `fit` and `preview`; `demo/demo.js` draws each block in a box of its own that slides with a CSS transition. `docs/site/demo-data.sh` writes the map hippo laid out for an 860 × 600 view, and `check-demo-render.mjs` fails if the port chooses other columns or lays it out differently.

## Tests

On maps of 3 to 300 notes, each in its default layout, two random arrangements and fitted to two views, at every level:

- no line enters a card or a folder's name or count, for every focus;
- no level run of a trunk comes within the clearance of a card or name band;
- no line shows at rest, and at most two trunks for any focus;
- every line turns at right angles, its vertical runs in gaps; two trunks never share a lane or a port, and a card has at most one port per side;
- a card with many links leaves as one trunk each way, and lines to one column share every point up to their branch;
- the notes read in every session keep the first column; on the 300-note map folder cards are one height;
- In cards wrap their words and keep 16 points below them;
- the 300-note overview fills more than 60 % of the view both ways; maps of up to 40 notes open whole at 42 % or more;
- a person's arrangement is kept when the map is fitted.

The app's tests cover the drag (the slot opens at the drop's column, Escape slides every block back), hover focus and ⌘0.

## Not in this

Curved (non-orthogonal) lines; lines between notes in one folder at Out (they are inside one card); a count on hover at Mid and In.

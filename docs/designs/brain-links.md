# Brain: links that read, an overview that fills the view, a drag that feels physical

Status: accepted 21 Nov 2026, to build in 0.8.3. Amends [brain-arrange.md](brain-arrange.md) and [app-night-map.md](app-night-map.md) › Brain.

## Background

0.8.2 drew every link at every zoom. On the demo home that was fine; on a 300-note map with 1,000 links it was a mesh under the cards, and the links that had to cross a column of cards all fell into one band of detours below the map. The entry note, linked from most notes, showed a fan of crowded lines at its right edge. Zoomed out, a 300-note map was a strip across the top of the window with empty space below, because Out used Mid's twelve columns. A dragged block was an empty outline.

## Rules

### At most 12 lines at once

`LINE_BUDGET` is 12. Judged on the demo home and the 300-note synthetic map at 0.15 to 2.0: 12 separate lines still read one by one; at 15 to 20 they start to read as texture, and past that as a mesh. A level (Out, Mid, In) counts its pairs of linked cards (at Out, a note in a folder is its folder's card, so a pair is folder↔folder or pinned note↔folder):

- **At most 12 pairs**: every line is drawn, one per pair. With a card in focus, its lines are drawn bright and the rest at 40 % of the quiet line colour.
- **More than 12 pairs**: only the lines of the card in focus are drawn, and nothing else. They leave the card as **one trunk each way** (one to the right, one to the left, one to cards in its own column) and each trunk drops **a branch in the gap beside every column it reaches**, which splits there into short stubs, one per card. So a card linked to 40 others draws at most three trunks; what the eye counts as separate lines (trunks) never passes 12, and in practice is 1 to 3.

The **focus** is the card under the pointer, else the selected note (at Out, the card of its folder). Moving the pointer from a card onto one of its lines keeps that card in focus.

### Zoomed out (below 55 %)

One line per pair of blocks. Its width shows how many links it stands for: `1.25 + 0.55 × log2(links)` points, at most 3.5, on screen at any zoom. With the pointer on a line, it turns lime and a label says what it stands for: `Notes and Runbooks · 6 links`, or on a shared trunk `START HERE and Style guide and Decisions · 2 links` (up to three names, else `and 4 others`).

### Closer in (Mid and In)

The same rules with note cards: on a level with more than 12 linked pairs (any map past a few dozen links), the selected or hovered note's links are drawn, routed around cards; nothing else.

### Routing

Lines turn only in the gaps between columns, so no line runs through a card or through a folder's name band:

- A line leaves its card level, toward the other card's column. Before each column it crosses, it keeps its height when that height is clear of the column's cards and name bands (4 points either side) and of lines already crossing there; else, in the gap before that column, it steps to the clear height nearest to the way to its targets. In the gap beside its target's column it turns to the target.
- So a long link climbs between cards column by column instead of running to a band below the map. With at most 12 trunks drawn, there is always room.
- Every vertical run in a gap has a lane of its own (8 points apart at most): brackets (two cards in one column) nearest their column, shortest innermost; then the rest, longest first.
- The lines that leave one edge of a card are spread along it, ordered by where their other ends are, 14 points apart on a note card and 24 on an Out card (which they meet at its middle, not its title), so they do not cross at the card.

### The overview fills the view

hippo's layout now depends on the view. `view_arrangement(w, h)` lays hippo's layout out (pinned notes in the first column, then each folder into the lowest run of columns, as before) in each number of columns from one to one per block, and keeps the one that shows the whole map largest: the one whose Mid fits at 55 % or more, if any, else the one whose Out fits at the largest zoom. Hippo.app does this when Brain opens and on Fit (⌘0); `hippo view map --view 1220x690` prints it. A person's arrangement is never changed by it.

Fit shows the whole map: at Mid when it fits there at 55 % or more (at most 100 %), else at Out at up to 54 %, centred both ways, above the Ask bar that floats over the map's foot. A folder spans as many columns at Mid as before, whatever the number of columns, so a block's drawing never changes with the view.

Results at a 860 × 600 map view: the demo home (22 notes) opens whole at 54 %, its names at 26 points on screen; a 40-note map opens whole at 54 %; the 300-note map opens at Out in 5 columns, filling the view (0.8.2: 12 columns, a strip a quarter of the view high).

### Dragging a block

- The lifted block is the real block, drawn under the pointer at the point it was grabbed, 3 % larger, at full opacity, with a soft shadow under it; a folder's frame gets a solid ground.
- Where it will land is a sunk slot (its shape, lime-tinted, dashed). It starts in the block's own place; as the pointer moves, the other blocks slide (150 ms, ease-out) to open the slot where it would land, and close the old one.
- On release the block settles into the slot (180 ms) and shrinks back to its size; on Escape every block flies back to where it was (220 ms).
- ⌥-arrows, Reset layout and its Undo slide the blocks too.
- Lines hide while blocks move and come back when they stop.
- With macOS Reduce Motion on (or `prefers-reduced-motion` in the browser) nothing slides: blocks jump to their places.
- The web demo does the same.

## How it works

- `hippo_view::night::map`: `Layout` gains `focus: Vec<(Item, Vec<Line>)>` (each linked card's lines, on a level with more than `LINE_BUDGET` pairs; `lines` is then empty) and `Line` gains `trunk` (lines with one trunk share their points up to their branch). `Layout::shown(focus)` gives the lines to draw with whether each is the focus's; `Layout::visible(focus)` counts trunks. `route` takes each want (a card and its targets, one for a lone line) and routes as above. `default_arrangement_in(cols)`, `view_arrangement(w, h)`, `NotesMap::fitted(w, h)`, `NotesMap::fit(w, h)`, `NotesMap::preview(arr, level)` (one level without lines, cheap enough for every pointer move) and `block_rect_in`.
- Hippo.app: `MapView` gains `hover`, `hover_line`, `slide` (the layout blocks slide to, each block's offset when it started, the dropped block's scale, the drop it previews) and `view_size`. The painter draws each block at its pose (offset and scale) and asks for another frame while a slide runs. `reduce_motion()` reads `NSWorkspace.accessibilityDisplayShouldReduceMotion`. Dev-only `HIPPO_APP_DEV_HOVER=x,y` rests the pointer on the map, `HIPPO_APP_DEV_SLOW=<factor>` stretches motion for screenshots and `HIPPO_APP_DEV_REDUCE_MOTION=1` turns it off.
- The demo: `demo/arrange.js` ports `links`, `route`, `view_arrangement`, `fit` and `preview`; `demo/demo.js` draws each block in a box of its own that slides with a CSS transition. `docs/site/demo-data.sh` writes the map hippo laid out for an 860 × 600 view, and `check-demo-render.mjs` fails when the port chooses other columns or lays it out differently.

## Tests

On maps of 3 to 300 notes, each in its default layout, two random arrangements and fitted to two views, at every level:

- no line enters a card or a folder's name or count, for every focus and none;
- at most 12 trunks are visible for every focus and none;
- every line turns at right angles and its vertical runs are in gaps;
- two trunks never share a lane or a port;
- a card with many links leaves as one trunk each way, and the lines to one column share every point up to their branch;
- the 300-note map, fitted to three views, opens at Out filling more than 60 % of the view both ways; maps of up to 40 notes open whole at 42 % or more;
- a person's arrangement is kept when the map is fitted.

## Not in this

Curved lines; lines between notes in one folder at Out (they are inside one card); a count on hover at Mid and In.

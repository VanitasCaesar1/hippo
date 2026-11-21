# Brain: arrange the map

Status: accepted 21 Nov 2026, built in 0.8.2; the drag, the links and the overview amended in 0.8.3 by [brain-links.md](brain-links.md). Amends [app-night-map.md](app-night-map.md) › Brain. Decided before writing: blocks snap to columns (no free placement), and the arrangement is saved per person in Hippo.app's preferences per workspace, never in the notes or in `.hippo/`.

## Background

Brain lays the notes out by itself: the notes read in every session in the first column, then each folder into whichever column is lowest. The layout keeps three promises at every zoom: nothing overlaps, every word fits its box, and no link runs through a card. People want their own order (the folders they work in near the top left, the rest out of the way), and they asked to drag the cards and to get the default back.

Free placement would break all three promises. The map has three zoom levels whose cards have different heights, so a spot chosen at one zoom has no meaning at the other two, and links are routed in the empty gaps between columns and the free channels between cards, which exist only because cards stay in columns. So what you arrange is **which column a block sits in, and its place in that column**, the way cards move on a board.

## What changes for a user

- **A block** is a folder, or one of the notes in every session (the entry note and the required notes). Notes stay in their folder, most-read first: moving a note to another folder would move its file, which arranging never does.
- **Drag a block to another column or place.** At Out (zoomed out below 70 %) press anywhere on a card; at Mid and In press on a folder's name band (the strip with its name and count) or on a note that is in every session. A press on a note in a folder, or on the ground, still pans. Past 4 points of movement the block lifts: the real block, raised under the pointer, while the others slide aside to open a slot where it will land: between two blocks of a column, at a column's top or foot, or in a new column at the right. Release settles it there and lays out all three zooms again; Escape flies it back (0.8.3, [brain-links.md](brain-links.md) › Dragging a block). Near the map's edge the map scrolls while you drag.
- **Keys.** With a note selected, ⌥← and ⌥→ move its block one column, ⌥↑ and ⌥↓ one place up or down. They are in the command palette as `Brain: Move Left`, `Right`, `Up`, `Down`.
- **Reset layout.** A `Reset layout` button (Lucide `undo-2`) sits in the zoom bar while your layout differs from the default, and `Brain: Reset Layout` is in the View menu and the palette. Reset shows `Layout reset to the default. Undo` at the foot of the map for 6 seconds; Undo puts your layout back.
- **Saved per person.** The arrangement is kept in Hippo.app's `preferences.json`, under the workspace's root, and comes back next launch. A renamed or deleted folder drops out of it; a new folder takes its default column, at the foot. The notes, `.hippo/` and anything an agent reads never see it; `hippo view map --json` and the site show the default layout.
- **Links go around cards at every zoom**, on any arrangement and any map size. Since 0.8.3 a level shows at most 12 lines at once and a long link climbs between cards column by column ([brain-links.md](brain-links.md)).
- **The live demo** arranges the same way (drag, keys, Reset), in the page only.
- **Below 900 points** the board lists blocks in your order; it cannot be dragged.

## How it works

### The arrangement (`hippo_view::night::map`)

```rust
pub struct Arrangement {
    /// Columns left to right, each its blocks top to bottom.
    pub columns: Vec<Vec<String>>,
}
```

A block's key is `folder:<name>` (`folder:` alone for the notes directory's top level) or `note:<path>`. `default_arrangement(notes, folders)` is the layout hippo would choose: pinned notes in the first column, entry note first, then each folder, largest first, into the run of columns whose tallest is lowest (as 0.8.1 did at Mid); the map carries it as `NotesMap::default_arrangement` beside the `arrangement` its layouts are made from. `settle(notes, folders, &Arrangement)` drops keys that name no block, drops empty columns and puts any block it lacks at the foot of its default column; `NotesMap::arranged(&Arrangement)` returns the map with all three layouts made from it, and `is_arranged()` says whether that differs from the default. `handle_at` finds the block a press lifts and `drop_at` where it lands (a `Drop`: column, place, the line's y, and whether the column is new).

One packer makes every level from an arrangement: it takes, among the columns' next blocks, the one that would sit highest (the leftmost on a tie), and puts it below what is already in every column it spans. A folder spans as many columns at Mid and In as its notes need (as before) and one at Out; gaps are `GAP` between two notes in every session, `FRAME_GAP` otherwise at Mid and In, `OUT_GAP` at Out. Each block draws the same at any origin (its frame, its cards, its labels and marks are made relative to its top left), so a move changes where a block is and nothing inside it. Out now uses the same columns as Mid (0.8.1 packed Out on its own), so crossing a zoom level never moves a block to another column.

Moves are methods of an arrangement, tested on their own: `move_to(key, column, place)`, `nudge(key, Nudge::Left | Right | Up | Down)`. Moving right of the last column makes a new column; a column left empty disappears.

### Links at every zoom

0.8.1 sent a link into a free channel only when one was within four card heights of its ends at Mid and In, so on a 300-note map with 1000 links about half ran under a card. Now every link whose straight run crosses a card takes the nearest free channel; when none lies between the cards, it uses the band below every card. Channels share a height only where their spans do not overlap, 4 points apart, so the band stays as thin as the crossings need. (0.8.3 replaced the band: a link that would cross a card steps to a clear height before each column it crosses; see [brain-links.md](brain-links.md) › Routing.)

### The app

- `Prefs` gains `brain_layouts: {workspace root: Arrangement}`, written through `prefs::update` on every drop, key move, reset and undo.
- `MapView` gains the drag (`Lift`: the block, where it was pressed, the pointer, the target) beside the press. A press decides what it is when it moves 4 points: a block's handle at the press lifts the block, anything else pans. A release outside the map drops the block where its line shows.
- The lifted block, its slot and the sliding blocks are painted by `paint_map` from the lift and a slide ([brain-links.md](brain-links.md)); the full layout is made again only on the drop.
- Escape goes to a lift first (the window's key interceptor), before an open menu.
- Actions `BrainMoveLeft`, `BrainMoveRight`, `BrainMoveUp`, `BrainMoveDown` (⌥-arrows, Brain shown and a note selected) and `BrainResetLayout` (View menu and palette, enabled while the layout differs) are rows of the action table.

### The live demo

The demo has no Rust. It packs blocks with a port of the packer and the router (`docs/site/demo/arrange.js`), moving each block's own cards, labels and marks from the default layout. `docs/site/demo-data.sh` writes one arranged map made by Rust beside the default (`hippo view map --json --arrangement <file>`, which lays the map out in a saved arrangement), and `check-demo-render.mjs` fails when the port lays it out differently.

## Tests

- Every arrangement, random ones included (blocks shuffled across columns, new columns, empty ones): no two cards or frames overlap at any level, every label fits its box, and no link runs through a card at any level, on maps of 3 to 300 notes.
- `settle` keeps every block exactly once; moves keep every block exactly once and do what their names say; the default arrangement laid out is the default map.
- The app: a press on a folder's name band and a move drops it in another column and saves it; a press on a note pans; Escape during a lift changes nothing; the keys move a block; Reset appears only when the layout differs, resets, and Undo restores; a saved layout comes back after a relaunch.
- The demo's port lays out the Rust-made arranged map the same.

## Not in this

Free placement, moving notes between folders, a layout shared with a team, and arranging on the narrow board.

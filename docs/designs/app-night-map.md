# Hippo.app: the night map

Status: accepted 20 Nov 2026, to build in 0.7.0. Amends [app-instrument.md](app-instrument.md): its tokens, kit and session views stay; its sidebar, Home and the five section views give way to the four views below. Fleet's Pulse, Lanes and Rows, the Inbox, Incidents' full list, Memory's treemap and Manage stay in the app, reached from the views that replace them ([Where the old views went](#where-the-old-views-went)).

## Background

The instrument look (18 Nov) made the app calmer, but it still reads as a list of rows, and the screens people share about hippo were less interesting than the thing hippo does. hippo's work has a shape a list cannot show: a rule comes from two things you told two agents; a note links to four others; a fix the first model drafted was turned down by a second. Two directions were drawn as HTML mockups with synthetic data (a photo-library project, its notes, sessions and accounts):

- **A, "Night map".** Canvas first: a dotted ground, cards on it, right-angled lines between them, a floating detail panel and a floating Ask bar. Each view is a small map.
- **B, "Morning edition".** A sidebar, one reading column, a docked panel; the same cards as a feed, no lines.

A was chosen, with two things from B: B's **four figures** at the top of Morning, and B's **feed and board** as A's layout when the window is too narrow for a canvas.

## What changes for a user

### The window

- **A top bar replaces the sidebar.** At its left (after the window controls): the hippo mark and the **workspace switcher** (the workspace's name; it opens Manage, where the workspace is set). In the middle, four destinations, each with a count: **Morning** (rules waiting for you, amber), **Brain**, **Agents** (sessions waiting on you, amber) and **Guard** (important guard incidents you have not marked "Not a problem", red). A count of zero is not drawn. At the right: **search** (`Find a note or session ⌘P`, the existing finder), the **indexed** state (`Indexed 08:41`, a green dot; red with what failed when ingest failed) and the **theme toggle** (dark and light; the theme picker still has every theme).
- **The ground** is a blue-black (`#0B0D14`) or a cool off-white (`#F3F4F8`) with a 2-point dot every 24 points. Cards are surfaces with a 1-point border and 14-point corners; the selected card has a 2-point indigo border, reserved on every selectable card so nothing moves.
- **Colour has four meanings.** Amber is you: something you said, a rule made from it, a thing waiting on you, the primary button. Indigo is the night: what hippo did overnight, the selection, links. Green is what the second model confirmed. Red is rejected, harmful, or running dry. Everything else is ink and three greys. Agent badges (two letters on the agent's colour) keep their own colours; nothing else uses them.
- **Type** stays the system's: SF Pro for prose, SF Mono for numbers, times, counts, commands and receipt ids. Labels are sentence case, not tracked capitals (this replaces the instrument look's capital labels).
- **A floating panel** on the right (360 points; 332 below 1200) shows the selected thing. It does not scale with the canvas.
- **The Ask bar** floats at the foot of every view. Before you type it says which agent will answer (`Claude Code answers`), from the runner hippo would use for `ask`. The answer opens above the bar with its sources; a source opens its note or session.

### Morning

Morning answers "what did hippo do last night, and what does it need from me".

1. **The date and the pass**: `Friday 20 November · night pass at 03:30, 4 min 12 s`. The headline: `Last night hippo read 36 sessions`, then one sentence: `It drafted 3 rules from what you told your agents and fixed 2 notes. The second model rejected 1 fix.` With no pass yet: `hippo has not run a night pass yet` and when the next one runs.
2. **Four figures** (from B): rules waiting for you (amber), notes fixed and confirmed (green), fixes rejected by the second model (red), notes checked against what happened.
3. **The strip**: one band across the view. The day before as bars of activity (15-minute buckets), amber where you corrected an agent; the night as an indigo-tinted zone with the pass marked; this morning up to a *now* line. Under it: `Day Thursday, 36 sessions, 4 corrections`, `Night pass at 03:30`, `Morning 08:41`.
4. **Three columns**, joined by right-angled lines:
   - *Yesterday, in your sessions*: each piece of evidence as a compact card (the agent's badge, the session's title, the time, then your words on the amber highlight, or a failure's signature in red mono).
   - *Waiting for you*: each pending rule as a card: where it came from (`From 2 corrections`, `From 2 failed runs`), the reviewer's verdict (`second model agrees`), the rule, **Keep**, **Edit**, **Dismiss**, and the note it goes into. Each evidence card is joined to its rule by an amber line.
   - *Applied overnight*: each change the pass applied, as a card with the note's title, `was` struck through and `now`, the receipt id and **Undo**. Then *Rejected by the second model*: each rejected change with the reviewer's reason quoted under a red bar.
5. **The panel: "Your agents now start with"**: what a new session's start block holds for a task you can change (the last task asked, else the entry note's task): the entry and required notes (*Always*), the notes picked for the task, the rules you kept, the rules waiting (*Not yet*, hollow squares), the bytes against the budget, and *Reaches*: each agent hippo is installed in and how (at session start, through its hooks, in each request).

Keep, Edit (Keep with the rule's text changed), Dismiss and Undo run the commands that exist: `hippo lessons keep`, `hippo lessons keep --text`, `hippo lessons dismiss`, `hippo revert <receipt>`. The card leaves when the command finishes and the models reload. Keep, Edit and Dismiss also have the Inbox's keys (Return, E, Delete) on the selected rule. A kept rule is undone like any change, from its receipt (`hippo revert`).

### Brain

Brain answers "what do my agents know, and what is wrong with it".

- **Header**: `Brain · 13 notes in ~/code/tern/notes`, `What your agents know about Tern`, and a sentence of counts: fixed last night, a rule waiting, gone stale.
- **The map.** Required notes and the entry note in a column at the left; then one dashed frame per folder with its note cards stacked inside. A card has the note's title, its first sentence (or its `summary` front-matter field) on one line, and chips: a flag (*fixed last night* green, *rule waiting* amber, *fix rejected* red, *stale, 41 days* grey, *every session* indigo for required notes) and *read 14× this week*. Lines are the links between notes (`[[wikilinks]]` and relative Markdown links); a selected note's lines turn indigo and the others fade.
- **Zoom.** The map zooms: pinch on a trackpad, ⌘-scroll, ⌘+ / ⌘− / ⌘0 (fit), and − / fit / + buttons at its bottom right. Zoom is about the pointer (the point under it stays under it). Scroll and drag pan. Zoom has three levels:
  - *Out* (below 55 %): each folder is one card with its note count and how many notes carry each flag.
  - *Mid*: note cards as above.
  - *In* (above 160 %): a note card grows to show its kept rules, its pending rules and their quoted evidence.
  Double-click zooms into a folder (to mid) or a note (to in). Zoom and pan are kept per workspace for the session.
- **The panel** shows the selected note: its folder, title, the summary, flags, three stat cells (reads this week, rules kept, changes), its rules with the date each was kept, a pending rule with Keep and Dismiss, the evidence for it (your words, by agent and time), reads by agent this week, links out and in, and its receipts (every change, with Undo). *Open* opens the note's tab as today.

### Agents

Agents answers "which sessions need me, and which accounts are running dry".

- **Header**: `Agents · today, across 3 accounts and 5 agents`, a headline (`2 sessions are waiting on you`) and a sentence about what is working and which account runs dry before its reset.
- **One card per account**: its name and agent badge, the 5-hour and 7-day meters with their shares and resets, `use next` (green) on the account with the most room, `empty at 11:40` (red, a red border) on one that runs dry.
- **Three columns**: *Waiting on you* (the question or plan quoted under an amber bar, how long it has waited, account, project), *Working* (title, current call, running time) and *Recent* (the last five finished sessions, each with one line on how it ended: merged, a claim checked or unchecked, corrected, stopped).
- **The panel** is the selected session: its question with **Open thread**, **Terminal** and **Later** (the Pulse ways in), its branch, pull request and checks, the kept rules it started with, and its last calls. *hippo never writes into a session.*
- Pulse, Lanes and Rows stay one click away: *Lanes* and *Rows* in the header open them as before.

### Guard

Guard answers "did an agent do something risky this week".

- **Header**: `Guard · the last 7 days, 212 sessions`, `3 things to look at` and a sentence on how hippo finds them (by rule, no model; the command and the path, never a secret's value).
- **Rows of three cards**, joined by red lines (grey for minor ones): *Who* (agent, session title, project, time, turn), *What it ran* (a plain-English title such as `Cursor ran a script straight from the internet` and the command in mono), *What it touched* (the host, path or `Model provider`, and why it matters: `Not on your list of install hosts`, `A credential file. hippo keeps the path, never the contents`, `First time this origin was seen`). Important ones first, then *Worth a look*.
- **The panel**: the kind, the severity, the title, a plain sentence, *What to do*, **Show session** and **Fix with <agent>**, the brief that button will start with (read before it runs), the command, and a table (agent, session, when, touched, found by). **Not a problem** hides it (kept for 30 days, as Incidents' *Not an incident*); *Open the guard note* opens the workspace's guard rules.
- **Fix with <agent>** starts a new session the way Brief does: it writes a prompt file and opens the agent in the session's repository with it. The agent is the one that ran the command unless it cannot be started (Cursor), then the `default` runner's. Nothing runs without the person pressing the button, and the brief is shown first.
- Incidents' full list (corrections, redone work, collisions…) stays one click away: *All incidents* in the header.

### The narrow layout

Below 900 points of width a canvas has no room. Morning and Brain switch to B's layout, in the same top bar:

- **Morning** becomes a feed: the headline, the four figures two by two, the strip, then *Waiting for you* as cards with each rule's evidence quoted inside the card, then *Applied overnight* and *Rejected*. The start-block panel moves below the feed.
- **Brain** becomes a board: the entry note's card, then each folder as a heading over a two-column grid of note cards (one column below 640 points). A selected note's neighbours get a dashed indigo border and a *linked* chip; its panel moves below the board. Zoom is off in the board.
- Agents and Guard stack their columns.

### Where the old views went

| Was | Now |
|---|---|
| Home | Morning (last night, waiting on you), Agents (accounts) |
| Inbox | Morning's *Waiting for you*; the Inbox itself, with the team's shared entries (`L-<id>`), stays a view of its own (⌘I and the command palette) |
| Fleet (Pulse, Lanes, Rows) | Agents; Pulse, Lanes and Rows from Agents' header |
| Incidents | Guard for the guard's kinds; *All incidents* opens the full list |
| Memory | Brain; the treemap from Brain's header (*Treemap*) |
| Manage | The workspace switcher, ⌘, and the command palette |

Session, note, proposal and x-ray tabs open in the window as today, under the top bar, with their title bar and its way back. Every view keeps its keys (⌘1 Home, ⌘I Inbox, ⌘F Fleet, ⌘4 Memory, ⌘6 Incidents); the command palette lists every view. On Brain, ⌘= / ⌘− / ⌘0 zoom the map instead of the text.

## What the engine adds

Each view reads one model from `hippo-view`, and each model has a `--json` twin under a new read-only command, `hippo view <model> --json`, so the web demo's data and third-party tools read the same numbers the app draws. Nothing new is written except where noted.

| Model | Command | What it holds |
|---|---|---|
| `morning` | `hippo view morning` | The last pass: when it started and ended (and so how long it took), sessions read and how many per agent, notes checked; pending rules, each with its kind (correction or repeated failure), quoted evidence (agent, account, session title, time, your words or the failure), target note, the reviewer's verdict; changes applied by the pass, each with a short was/now pair cut from the proposal by the word diff, its receipt and whether it can be undone; rejected changes, each with the reviewer's reason (the receipt's `notes`, already stored); the four figures. |
| `strip` | `hippo view strip` | Activity from yesterday's local midnight to now (the rollup's 15-minute buckets, summed across sessions, so the strip costs one query), the corrections' buckets, the pass's start and end, now. |
| `brief` | `hippo brief --json` | Gains `parts`: the required notes, the notes picked for the task, the kept rules as a list, bytes per part, the budget; and `reaches`: each agent with hippo installed and how it receives the block. The existing `text`, `bytes`, `budget` and `omitted` stay. |
| `map` | `hippo view map` | Every note with its folder, one-sentence summary, 7-day reads (total and by agent), flags (fixed last night, rules waiting, fix rejected, stale N days, required), its links as edges, and the laid-out rectangles for each zoom level (below). Built on `memory::Map`. |
| `note` | `hippo view note <path>` | One note's panel: summary, kept rules with the date kept, pending rules with their evidence, reads by agent, links out and in, receipts with their revert state. |
| `fleet` | `hippo fleet --json` | Gains `recent` (the last five finished sessions, each with how it ended) and each account's 7-day share where the client reports it. |
| `guard` | `hippo view guard` | The guard's incidents of the last 7 days, each with a title and sentence written from a template per kind and filled with the agent and target, the target as its own field with why it matters, a severity (`important` or `minor`), what to do, and the brief for *Fix with*; counts per kind; sessions in the period. |
| `counts` | `hippo view counts` | The three badges (rules waiting, sessions waiting on you, important guard incidents), the indexed time, and the runner that answers Ask (its agent and account). Reads lesson files and the rollup's live rows only, so the app can poll it. |

### The layout is computed in `hippo-view`

GPUI has no transform for a subtree, so a canvas cannot be scaled as a picture. Brain's map is laid out in `hippo-view` (as `memory::layout` lays out the treemap): every card, folder frame and link is a rectangle or a polyline in *map units* (points at 100 %). The app multiplies every rectangle, line, corner radius and text size by the zoom factor and draws the result. This is semantic zoom: each level has its own layout, and the app chooses one by the zoom factor.

- **Mid** is the base: required notes in a column at the left, folders packed into columns of fixed width (each folder's frame below the shortest column's last frame, largest folders first), cards of fixed height (a title line, a summary line, a chip line), 12 points between cards and 24 between frames.
- **Out** uses the same folder frames as Mid, each drawn as one card, so moving between Out and Mid moves nothing.
- **In** uses taller cards (each note's rules and evidence, clamped to six lines) in the same columns. When the zoom crosses into or out of In, the app keeps the card under the pointer under the pointer: it finds the card at the pointer in the old layout, its relative position inside the card, and pans the new layout so the same point of the same card is there.
- **Links** are polylines: from the right edge of one card, one turn in the gap between columns, into the left edge of the other; two cards in one column are joined by a short bracket at their right. A link to a note in a folder that is drawn as one card ends at the folder.
- **Culling.** The app draws only the cards whose rectangle meets the view and the links whose bounding box does; text is shaped only for drawn cards, at sizes rounded to half a pixel so a zoom gesture reuses shaped lines. Every segment of a link is level or plumb, so links are painted as thin quads, not stroked paths. The target is a paint well under the 16.7 ms of a 60 fps frame at 300 notes and 1,000 links; `HIPPO_APP_DEV_FRAMES` prints each paint's time.

Morning and Guard are fixed layouts in the app (three columns of cards with clamped text, so card heights are known and the lines can be drawn from the same numbers). They scroll; they do not zoom.

### The pass's start time

A pass's report says when it ended (the file's time) but not when it began. `hippo dream` writes `started: <time>` after `status:` in each new report; Morning shows the duration only for reports that have it.

## Dev and test

- Dev-only environment variables for screenshots of a synthetic home: `HIPPO_APP_DEV_VIEW` (`morning`, `brain`, `agents`, `guard`, and the old names), `HIPPO_APP_DEV_THEME` (`dark`, `light`; nothing is saved), `HIPPO_APP_DEV_SIZE` (`1440x900`), `HIPPO_APP_DEV_ZOOM` (Brain's zoom factor), `HIPPO_APP_DEV_SELECT` (`rule:0`, `fix:1`, `note:<path>`, `session:<id>`, `incident:<id>`), `HIPPO_APP_DEV_SYNTHETIC_MAP` (`300x1000`: a synthetic map of that many notes and links) and `HIPPO_APP_DEV_FRAMES` (each paint of the map prints its time on stderr; the app writes no file but its preferences). A dialog is shown for a screenshot as a view in its place (`HIPPO_APP_DEV_DIALOG`), never as a window of its own. With any of them set the app does not bring itself to the front.
- Zoom, pan, Keep and Dismiss are tested through GPUI's test harness (actions and simulated events on a test window), and the layout arithmetic by unit tests, never by sending keys to a real window.
- The built-in Hippo Dark and Hippo Light themes take the night map's colours, so the views drawn under the top bar match it.
- `app/scripts/demo-home.sh` gains the guard's kinds (a piped installer, a credential read, a fresh clone run) so Guard has something to show.

## The web demo and the landing page

The site gets Morning and Brain as a web replica (`demo.html`, its own page with `script-src 'self'`; zoom and pan work there too), with its data **generated** from the synthetic demo home through the `--json` twins by a script in `docs/site/`, so its figures are hippo's output. The landing page takes direction A's look and keeps its headline; *Try the live demo* opens the replica.

## Alternatives

- **B, the edition.** One column of cards and a docked panel: a restyle of what exists, cheaper and fitting more text, but less striking, and it cannot show provenance or links without lines. Kept as the narrow layout.
- **No zoom.** The mockups pan only, because GPUI cannot scale a subtree. A map of 200 notes then needs a lot of scrolling to see its shape. Scaling the laid-out model costs a second layout level and the culling, and makes the map usable at that size.
- **Record each card's bounds in prepaint and draw lines in an overlay.** Keeps flexible card heights, but lines would lag a frame behind the cards, and a layout the app owns cannot be tested or reused by the web replica. Computing the layout in `hippo-view` lets the CLI and the web demo use it as is.
- **Keep the sidebar beside the top bar.** Two navigations for four views; the sidebar's width is better given to the canvas.

# Fleet Pulse

Status: accepted, to build after 0.4.1.

## Background

Hippo.app's Fleet tab lists today's sessions as rows grouped by state (Needs you, Working, To review, Settled), and `L` switches it to Lanes, a replay of one day as a lane per session. Both read `hippo_view::fleet` and `hippo_view::replay`, which read the transcripts, the index and the daily rollup; nothing there writes.

The rows answer "what is each session doing", one line at a time. They do not show the shape of the fleet at a glance: which sessions are busy right now and which have gone quiet, whether an account will run out before its window resets, what memory did today, or which of several things wanting attention matters most. People running several agents at once asked for that view.

## What changes for a user

Fleet gets a third view, **Pulse**, and becomes its default. A switcher in the toolbar picks Pulse, Lanes or Rows; Lanes and Rows are unchanged.

Pulse is one screen:

- **Header.** How many sessions are working, how many sub-agents run under them, how many need you; a histogram of tool calls and edits over the shown span (1 hour, 3 hours or the day), stacked by account and ending at a *now line*; tool calls in the last 5 minutes, agent work today, and lines added and removed today.
- **Needs you.** Each waiting session as a lane with its question quoted, how long it has waited, and three ways in: open the thread in the app, open it in its client (T3 Code), or open a terminal. With exactly one session waiting, it is drawn as a large card with its last calls, its diff and pull request, its checked claims and the kept rules it started with.
- **Next up.** One ranked list of what will want you soon: an account that runs dry before its reset (with *Resume them on* another account), a finished session's unverified claim (*Check*), a destructive command that did harm (*Open incident*), and a correction that is a rule candidate (*Keep now*).
- **Working.** Each live session as a waveform of its activity in 2-minute steps up to the now line, its sub-agents as strands under it, and on the right its current call (verb, file, lines, running time) with *started with N kept rules* or *you corrected it at 16:22*.
- **To review, Quiet, Settled.** Compact lanes and one-line summaries.
- **Usage rail.** Per account: the 5-hour figure, a drain chart of the window so far with the projection to its reset, the burn rate per hour, the reset countdown, the 7-day bar, *use next* or *low*, and when an account will empty before its reset: *Empty about 16:48, 27m before the reset. 3 sessions will stop.*
- **Memory today** (sessions that started with a kept rule; today's corrections and what will happen to each) and **Tonight** (what the next nightly pass will read).

Marks on the lanes: a ring where a session started with a kept rule, a diamond where you corrected it, a square for a destructive command, a dot for a merged pull request.

Nothing new is written anywhere. Pulse is a reading of files hippo already has.

## What the engine adds

All of it is computed when a reader asks, in `hippo-view` and `hippo-core`. No record format, rollup table or cache file changes.

### 1. Activity in 2-minute beats

`replay::BUCKET` (15 minutes, the rollup's `activity` bucket) is too coarse for a live lane. The day's lanes already read every turn of every session active that day from the index (`Index::turn_marks`), so the finer series is counted from those turns at read time: `replay::BEAT` is 2 minutes, and each `Track` gains `beats`, one count per beat of the day (turns, an edit counting three, as the existing strength does), 720 small numbers per track. The rollup keeps its 15-minute buckets; it does not grow.

### 2. Usage over the window: drain, burn rate, empty time

`usage::Snapshot` already holds every message's weighted tokens per profile for the last 8 days, and Codex's meter samples. `usage::drain(snapshot, headroom, now)` returns, for a profile with an open 5-hour window and a known figure:

- `samples`: the fraction of the window used at the end of each 10-minute step from the window's start to now;
- `burn`: the fraction used per hour over the last 30 minutes;
- `empty_at`: when the window will be used up at that rate, if before its reset.

Weighted tokens become a fraction by the window's own figure: the client meter's percent over the tokens counted in the window when a meter is current, else the learned limit. Codex's samples are its own meter readings. Without a figure there is no drain, and the rail shows the figure alone. *N sessions will stop* counts the live sessions on that profile.

### 3. Kept rules served at the start

A session's start block is already recorded (`rollup` table `context`: the notes it carried). A kept lesson names the note its rule went into and when it was kept. `fleet::Row` gains `rules`: the kept lessons whose note was in the session's recorded start block and that were kept before the session began. A session whose block was not recorded has none; the view says nothing rather than guess.

### 4. Kinds of marks

`replay::MarkKind::Incident` is split: `Correction` (the person corrected the session), `Destructive` (a destructive command that harm followed) and `Incident` (the other kinds). Counts in the day line still add all three.

### 5. Tonight

`pulse::Tonight`: the clock time of the nightly pass (`dream_at`); how many top-level sessions it will read (their last turn after the pass's last success, or in the last day); corrections and destructive incidents among them; unverified claims among them; and notes due a re-check (`review_after` passed).

### 6. Fleet totals

`pulse::Totals`: tool calls in the last 5 minutes (calls the day's tracks hold), working time today, and lines added and removed today (each session's own and sub-agents' edits, files counted once, as Fleet counts them).

### 7. Next up

`pulse::next_up` ranks, in this order and soonest or newest first within each kind:

1. an account whose window empties before its reset, with its live sessions and the account to use next; *Resume them on* copies each session's resume command for that account (`fleet::resume` already targets another profile);
2. a finished session with an unverified claim;
3. a destructive incident today;
4. a correction today that a pending lesson already holds (a rule candidate), with that lesson.

## The app

Pulse is drawn by hand: each lane's beats, the histogram and the drain charts are painted on canvases; the time grid and the now line are one overlay across the scrolling lanes; marks are vector paths. Only needs-you lanes animate (a pulsing dot), and not when the system asks for reduced motion; with nothing waiting the view draws only on refresh. It follows the app's tokens in light and dark, and holds from the app's minimum window size up, dropping the right rail under the lanes, then the activity column, as the window narrows.

Keys: `↵` opens the first waiting thread, `T` opens it in its client, `⌘T` opens a terminal, `S` (Later) skips to the next, `J`/`K` move in Next up, `U` resumes the runs-dry sessions on the account to use next.

## Alternatives

- **Store 2-minute buckets in the rollup.** Seven times the rows of `activity`, for a view that needs only today's live sessions. Reading the turns is fast enough for a day.
- **Record the rules served from the hook.** The start block is already recorded with the notes it carried; matching kept lessons to those notes needs no new record.
- **Sample the usage meter on a timer.** It would need a writer. The messages already give the series; only the scale is needed.
- **A radar view** of accounts and sessions was mocked and left for later; the switcher has room for it.

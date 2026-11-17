# Fleet Pulse

Status: accepted, to build after 0.4.1. Amended 17 Nov 2026: Pulse is a queue and one focused item, with the fleet in a sheet (the first build showed everything at once).

## Background

Hippo.app's Fleet tab lists today's sessions as rows grouped by state (Needs you, Working, To review, Settled), and `L` switches it to Lanes, a replay of one day as a lane per session. Both read `hippo_view::fleet` and `hippo_view::replay`, which read the transcripts, the index and the daily rollup; nothing there writes.

The rows answer "what is each session doing", one line at a time. They do not show the shape of the fleet at a glance: which sessions are busy right now and which have gone quiet, whether an account will run out before its window resets, what memory did today, or which of several things wanting attention matters most. People running several agents at once asked for that view.

## What changes for a user

Fleet gets a third view, **Pulse**, and becomes its default. A switcher in the toolbar picks Pulse, Lanes or Rows; Lanes and Rows are unchanged.

Pulse answers "what should I do next?" first and "what is the fleet doing?" second. It is a queue and one item at a time, not a dashboard of everything at once:

- **Strip.** One line above both panes: how many sessions are working and how many need you on the left; on the right each account's 5-hour meter (bar, figure, reset time). An account that will empty before its reset turns its meter red and says when (*empty 16:48*); nothing else in the strip calls for attention.
- **Queue** (left pane). Two groups in one list. *Needs you*: each waiting session (a question, or a plan waiting for approval), oldest first. *Next up*: what will want you soon, in the ranking below: an account that runs dry before its reset, a finished session's unverified claim, a destructive command that did harm, and a correction that is a rule candidate. Each row is a status icon, a title, one line of reason, its age and its account's dot. One hint line at the foot of the pane names the keys.
- **Focus** (right pane). The selected item, alone, in one centred column:
  - *A waiting session*: its account and project, its title, one line of facts (branch, pull request with its diff, checks, *started with N kept rules*, each with its detail on hover), the question quoted large, and its ways in: **Open thread** (in the app), **T3 Code** (its client), **Terminal**, **Later**. Under them, the session's activity over the shown span as one strip of 2-minute beats with its marks and the time it has waited, and its last three calls, with *Show all* for the rest.
  - *A plan waiting for approval*: the plan's steps as a numbered list, and the same ways in.
  - *An account that runs dry*: its drain chart (the window so far, the projection to empty, the reset), the sessions that will stop and what each is doing, and **Resume them on** the account to use next.
  - *An unverified claim, a destructive incident, a rule candidate*: the one fact, its evidence (what ran or did not; the command and what it overwrote; the corrections side by side), and the one action: **Check**, **Open incident**, **Keep now**.
- **Fleet sheet.** A row pinned to the foot of the queue, *5 working · 7 sub-agents*, opens a sheet over the queue (`F`; `Esc` closes it) with Working, To review, Quiet and Settled: each working session as one row with a waveform of its last 30 minutes, its sub-agent count, its current call and its marks. Under the lists, **Memory today** (sessions that started with a kept rule; today's corrections and what will happen to each) and **Tonight** (what the next nightly pass will read).
- **All clear.** With nothing in the queue, the focus pane shows the fleet's histogram of tool calls and edits over the span, stacked by account and ending at a now line, with tool calls in the last 5 minutes, agent work today, lines added and removed today, and the time of tonight's pass.

Marks: a ring where a session started with a kept rule, a diamond where you corrected it, a square for a destructive command, a dot for a merged pull request.

An item leaves the queue when what it reports is no longer true: the session answers or resumes, the account's window resets, the lesson is kept or dismissed; unverified claims and destructive incidents stay for the day, as Next up ranks them. Acting on an item opens the place where it is dealt with; Pulse keeps no state of its own beyond *Later* for the app's run. Hippo never writes into a session: the ways in open the session where it runs.

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

### 8. A waiting plan's steps

`State::Waiting` holds the plan's last line (`asked`). The focus pane shows the whole plan: `fleet::Row` gains `plan`, the text of the unanswered `ExitPlanMode` call's `plan` input as the transcript tail already reads it, split into steps at its numbered or bulleted lines (at most 12; the rest are counted). A plan without such lines is shown as its text, wrapped. Nothing is written.

## The app

Pulse's charts are drawn by hand: the activity strip's beats, the sheet's waveforms, the histogram and the drain chart are painted on canvases, and marks are vector paths. Only the queue's waiting rows animate (a pulsing dot), and not when the system asks for reduced motion; with nothing waiting the view draws only on refresh. It follows the app's tokens in light and dark: one accent; the status colours only on icons, meters and marks, never on whole sentences; the account colours only on dots, bars and beats.

Density is a requirement: the default view shows the strip, the queue and one item. Everything else is one key away (the sheet, *Show all*, the hover details), never on screen by default. While Pulse is shown, the sidebar keeps its sections but drops its list of sessions: the queue and the sheet are that list here, and the sidebar's Working shelf (live and quiet sessions) would contradict the strip's count of sessions working now.

The queue is about 380 points wide and the focus column at most 640, centred in what is left. As the window narrows the queue narrows to 300; below about 900 points of content the panes stack: the queue fills the view and `↵` opens the item's focus with a back button (`Esc`).

Keys: `J`/`K` (or the arrows) move in the queue; `↵` does the selected item's first action (Open thread, Resume them on, Check, Open incident, Keep now); `T` opens a waiting session in its client and `⌘T` in a terminal; `U` resumes the runs-dry sessions on the account to use next; `S` (Later) puts the item after the others in its group until the app restarts; `F` opens the fleet sheet; `Esc` closes it.

## Alternatives

- **Store 2-minute buckets in the rollup.** Seven times the rows of `activity`, for a view that needs only today's live sessions. Reading the turns is fast enough for a day.
- **Record the rules served from the hook.** The start block is already recorded with the notes it carried; matching kept lessons to those notes needs no new record.
- **Sample the usage meter on a timer.** It would need a writer. The messages already give the series; only the scale is needed.
- **A radar view** of accounts and sessions was mocked and left for later; the switcher has room for it.
- **Everything on one screen** (the first build): a header histogram, needs-you lanes, Next up, working waveforms, To review, Quiet and Settled, and a rail of usage, memory and tonight, all at once. Each fact appeared in two or three places and nothing said what to do first. A timeline of every session on one time axis, with usage drawn past the now line, was mocked too; it shows the fleet best but leaves the next action to the reader.
- **Reply from Pulse.** A reply box under the question, with suggested replies, was mocked. Hippo has no way to write into a running session: T3 Code's link starts a new draft thread, and the other clients take input only in their own window. Suggested replies would also need a model call or a guess at the question's options. The ways in open the session where it runs instead.
- **Approve a plan from Pulse.** The same: approval happens in the session's client.

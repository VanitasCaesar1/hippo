# Hippo.app

A macOS window onto your agent fleet and what it learned: which agents are working now, which stopped at a usage limit and where to resume them, how much headroom each profile has, whether any agent already solved what you are about to ask, and what dreaming changed in your notes overnight. hippo's own health is one dot. It reads hippo's files through `hippo-core` and changes none of them itself: a revert is a `hippo revert` it runs, Looks right is a `hippo review --ok`, a written answer is a `hippo ask`, and a note opens in your own editor. How it fits with the engine is in [Architecture](../docs/site/src/architecture.md).

Status: built and early. It is checked on synthetic data and used on macOS on Apple silicon; whether it can replace the CLI in daily use is not proven yet.

## Install

`hippo app install` downloads this version's Hippo.app (Apple silicon and Intel, signed ad hoc, not notarised), checks its sha256 and puts it in `~/Applications`; `curl -fsSL https://raw.githubusercontent.com/VanitasCaesar1/hippo/main/install.sh | sh -s -- --app` does the same after installing the CLI. [Install › Hippo.app](../docs/site/src/install.md#hippoapp-macos) has the details, including how to open a copy downloaded in a browser. The release asset is made by `app/scripts/release-app.sh` ([design](../docs/designs/app-download.md)).

## Build from source

The app is its own Cargo workspace, so the engine's build never compiles GPUI. It needs macOS and a stable Rust no older than the pinned Zed commit's toolchain (1.99.1 at the time of writing).

```sh
app/scripts/bundle.sh                       # release build, then target/Hippo.app, signed ad hoc
TOOLCHAIN=+1.99.1 app/scripts/bundle.sh     # when the default toolchain is older
cp -R app/target/Hippo.app ~/Applications/
```

The first build compiles GPUI and takes a while; later builds are quick. The bundle is signed ad hoc, not notarized, so the first launch needs System Settings › Privacy & Security › Open Anyway. If your workspace or `~/.hippo` is on a removable volume, macOS asks once whether Hippo may read it.

The app finds hippo the way the nightly jobs do: the `hippo` binary and `HIPPO_HOME` named in `~/Library/LaunchAgents/dev.hippo.ingest.plist` (`hippo schedule install` writes it). Started from a shell with `HIPPO_HOME` set, it uses that home and the `hippo` on `PATH`, and About says `dev home` with the path.

### Trying it on synthetic data

```sh
cargo build -p hippo
HIPPO_BIN=target/debug/hippo app/scripts/demo-home.sh /tmp/hippo-demo
HIPPO_HOME=/tmp/hippo-demo/home PATH=$PWD/target/debug:$PATH app/target/release/hippo-app
```

`demo-home.sh` builds a home from the synthetic fixtures in this repository: 16 notes; 34 top-level sessions, including the shapes real homes have (a session named by its client with three sub-agents and a failed background command, a context handoff, a session a program started, a live session with sub-agents working now, and T3 Code delegated tasks with their own sessions); and one dreaming run with an applied change you can revert; and the team's shared Inbox with three synthetic teammates.

For screenshots, development-only variables set the window up without touching it: `HIPPO_APP_DEV_SIZE=1440x900` sizes the window, `HIPPO_APP_DEV_VIEW=morning` opens a view (`brain`, `agents`, `guard`, or an older section such as `inbox`), `HIPPO_APP_DEV_THEME=dark|light` draws Hippo's theme of that appearance without saving it, `HIPPO_APP_DEV_SELECT` selects something (`rule:0`, `fix:1`, `note:<path>`, `session:<id>`, `incident:<id>`, or an Inbox card's key), `HIPPO_APP_DEV_ZOOM=1.9` sets Brain's zoom, `HIPPO_APP_DEV_DIALOG=fix` draws Guard's Fix-with confirmation in place, `HIPPO_APP_DEV_SYNTHETIC_MAP=300x1000` swaps in a synthetic map of that many notes and links, `HIPPO_APP_DEV_FRAMES=1` prints each paint of the map's time on stderr, `HIPPO_APP_TIMING=1` prints how long each part of a reload took, and `HIPPO_APP_DEV_SHARE=1` opens the Inbox's Share dialog. With `HIPPO_APP_DEV_SIZE` or `HIPPO_APP_DEV_VIEW` set the app does not bring itself to the front and reads everything before its window opens, so the first frame has the data (a window opened behind others may be drawn only once). Use them with `HIPPO_APP_PREFS` on a scratch file and `TZ=UTC`, keep the scratch home on the system volume (a home on a removable volume makes macOS ask for access), launch the bundle with `open -g -n --env …`, capture the window by its id (`screencapture -l`) and stop the process by its PID. GPUI's dialogs make their window key, so a `HIPPO_APP_DEV_SHARE` capture is not a background one.

## The window

The app has the night map's look ([design](../docs/designs/app-night-map.md)): a dotted ground, cards joined by right-angled lines, and colour with four meanings on graphite (lime is you and what waits on you, the night colour, near-white or near-black, the night and the selection, green confirmed, red rejected or harmful). A top bar holds the workspace, **Morning**, **Brain**, **Agents** and **Guard** with their counts, search (⌘P), `Indexed hh:mm` and the dark and light toggle; a panel floats on the right with the selected thing, and the Ask bar floats at the foot and names the agent that answers. Morning is the last night pass: four figures, the day and night as a strip, what you said joined to the rules drafted from it (Keep, Edit, Dismiss; Return, E and Delete on the selected one), what was applied (Undo) and what the second model rejected, and what a new session starts with. Brain is the notes as a map that zooms (pinch, ⌘-scroll, ⌘= ⌘− ⌘0, double-click into a folder or note). Agents is each account's meters and the sessions waiting, working and finished. Guard is the guard's incidents in plain words, with Show session and Fix with. Below 900 points Morning is a feed and Brain a board. The older views stay: Home ⌘1, Inbox ⌘I, Fleet ⌘F (Pulse, Lanes, Rows), Memory ⌘4 (the treemap), Incidents ⌘6 and Manage ⌘,; what you open (a session, a dreaming run or change, a note) shows under the top bar with a back link. Some views still use Lucide icons (`app/THIRD_PARTY_NOTICES.md`).

- **Home**: one column. The date; readouts of how many sessions wait on you (and the oldest wait), how many work now (and their sub-agents) and what is left of each account's 5-hour window (red with its empty time when it runs dry before its reset); the scope band, the fleet's tool calls every 2 minutes over the last 3 hours; what waits on you, each row opening it in Pulse; last night's pass (notes it updated, each confirmed by a fresh reviewer, note changes to look at, rules proposed, and what tonight's pass will read); and the prompt at the foot (Ask or Brief, with the account to use next).
- **Ask**: the sessions and notes that match a question, ranked, each with Open and its resume command, and a written answer that cites them (one model call through `hippo ask`; Manage turns it off).
- **Fleet** opens on **Pulse** ([design](../docs/designs/fleet-pulse.md)), a queue and one item at a time. Its header reads how many sessions wait on you and work now, and each account's 5-hour window as a meter of 10 segments, red with a countdown (`T−14M`) when it will run dry before its reset; under it, the scope band. The queue lists what waits on you (a question or a plan to approve), then what is **coming up**, ranked: an account that runs dry, an unverified claim, a destructive command, a correction worth keeping as a rule; hippo writes those titles (*A force push did harm*, *A rule worth keeping*). The selected item fills the focus pane: a waiting session with a running timer since it asked, its question or plan steps, Open thread, T3 Code, Terminal and Later (each with its key), a table of its branch, pull request, claims and the kept rules it started with, and its last three calls; an account that runs dry with a countdown, its drain chart (what was used, the projection to empty, now), its rate and reset and the sessions that will stop, and *Resume them on* another account (copies their resume commands); or an unverified claim, destructive command or rule to keep with its evidence and its one action (Check it, Open the incident, Keep it). Pulse never writes into a session. The foot of the queue (or `F`) opens a sheet of the working sessions, each with its current call and its last 30 minutes as bars, and counts To review, Quiet and Settled, each opening Rows. With nothing in the queue, Pulse shows the day's readouts and its scope band. Only a waiting item's mark blinks, and not with Reduce motion on. Below 900 points the queue and the focus stack. `Pulse | Lanes | Rows` switches the view. **Rows**: today's sessions (live, stopped at a usage limit, finished) and each profile's headroom in its 5-hour window. **Lanes** (or `L`) is **Replay**: one local day as lanes, one per thread, with its sub-agents folded into a strip under it; where each worked (a muted fill), waited on you (lime) or stood at a usage limit (dashed), with marks for pull requests, incidents, questions, limits and carry-ons. A line above counts the day (threads, sub-agents, agent work, the time agents waited on you and the longest wait). The scrubber says what stood at any minute; a click pins it. ⌘-scroll or a pinch zooms, a drag pans, `‹ ›` change the day. Made from the rollup and the index, no model. Headroom is an estimate from your own transcripts, not Anthropic's meter, and says "no estimate" until a profile has hit a limit in the last 8 days. Resume copies the command and a short handoff; on another profile when this one is at its limit.
- **Inbox**: the overnight dreaming changes nobody has looked at, each with Looks right and Undo, and the recent dreaming runs.
- **Memory**: the notes list beside the note page.
- **Incidents**: what went wrong, which agent did it, at which step, and whether it had been told, each with evidence you can check (your words, the hook's message, the edit, the claim); found by rules, no model. Each incident opens to what it was told, what it did (for a correction, the call its words name, else `the step is not established` and the agent's last message), what showed it was wrong and what fixed it, with Never again (a rule drafted through the lessons flow, or a hook to copy) and Not an incident. A check claimed with no run after the last edit is a hint, not an incident: it is listed after the days under `Hints to check` and counted nowhere. `hippo incidents` prints the same.
- **Manage**: the theme, the Ask setting, hippo's health, and what comes next.

| Keys | What |
|---|---|
| ⌘K | Ask (Return asks; ⌘Return asks for sources only) |
| ⌘1 to ⌘6, ⌘I ⌘F ⌘M ⌘, | Home, Inbox, Fleet, Memory, Manage, Incidents; again to go back to the section's own view |
| Escape, ⌘[ | Back |
| ⌘⇧P | Command palette: every action, with its keys |
| ⌘P | Find a note or a session |
| ↑ ↓, Return, ⌘C | In Ask's sources: move, open, copy the resume command |
| ⌘B, ⌘⇧F | Memory: hide or show the notes list; filter it |
| ⌘L | Context preview: what `hippo context --task …` gives an agent |
| ↑ ↓ (J K), Space or Return, Home End | In a session: move between turns, open or fold one, first or last |
| L | In Fleet: Lanes and back; in Lanes, Esc lets a pinned time go, ↑ ↓ and Return open a session at its step |
| J K, Return, S | In Pulse: move in the queue, do the selected item's action (Open thread, Resume them on, Check, Open incident, Keep now), put it off until the others |
| T, ⌘T, U, F, Esc | In Pulse: open a waiting session in T3 Code or a terminal, resume the runs-dry sessions on the account to use next, open the fleet sheet, close it |
| ⌘O, ⌘⇧R | Open the note in your editor; reveal it in the Finder |
| ⌘⌥Z | Revert the change shown (asks first; ⌘Return confirms, Return or Escape cancels) |
| ⌘⇧T | Theme picker |
| ⌘R | Reload |
| ⌘= ⌘- ⌘0 | Text size for this session |

Every action is also in the menu bar, and everything the app shows has a CLI command or a file behind it: `hippo ask`, `hippo fleet`, `hippo resume`, `hippo review --ok`, `hippo log`, `hippo show session|proposal|note`, `hippo context`, `hippo revert`, and the reports under `.hippo/reports/`.

## Themes

Two themes are built in, Hippo Light and Hippo Dark; **System** follows macOS's appearance. Add your own as JSON files in `$HIPPO_HOME/themes/` (`~/.hippo/themes/`), and pick one with ⌘⇧T. The choice (and whether Ask writes answers) is saved in `~/Library/Application Support/dev.hippo.app/preferences.json`, the one file the app writes. An edited theme file shows within about two seconds; a file that stops parsing keeps the last good version and says so.

A theme file is a theme family in the format of the Zed editor's themes, so a Zed theme family copied into that folder works as it is (its syntax, terminal and editor colours are ignored; the app shows no code). Each theme may carry a `hippo` object:

```json
{
  "name": "Harbour",
  "author": "A. Person",
  "themes": [
    {
      "name": "Harbour Dark",
      "appearance": "dark",
      "style": {
        "background": "#18181bff",
        "text": "#f4f4f5ff",
        "text.muted": "#a1a1aaff",
        "border.focused": "#60a5faff"
      },
      "hippo": {
        "font_family": "IBM Plex Sans",
        "font_size": 14,
        "background": { "image": "harbour.png", "opacity": 0.15 }
      }
    }
  ]
}
```

The app reads 31 style keys (the table in [Desktop themes](../docs/site/src/themes.md#desktop-themes)); a missing one falls back to the built-in theme of the same appearance, with a warning in the picker. `font_size` (10 to 24) is the way to get larger text that lasts: copy a built-in theme from `crates/hippo-core/themes/` into the folder, rename it and raise `font_size`. Themes are data: no code, no includes, no URLs. Theme packs (`hippo add`) are read from `<workspace>/.hippo/packs/*/theme.json` and checked strictly.

The built-in themes meet WCAG AA, which a test checks. **A theme you add is not checked and may not**: if one is hard to read, choose "Theme: Use System" in the View menu or the palette, hold Option while the app starts, or delete the file.

## Accessibility

Everything can be reached from the keyboard, through the key map above and the command palette, and the focused control has a ring. Every control has an accessibility role and label, but the app does not support screen readers yet: the VoiceOver walk-through has not passed, so no support is claimed.

## Checks for a change under `app/`

Run these in `app/` (GitHub Actions runs the same in `.github/workflows/app.yml`):

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo deny check
```

Clippy forbids the app's code from calling `std::fs`'s writing functions, except in `prefs.rs`.

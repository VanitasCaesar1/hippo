# Hippo.app

A macOS window onto your agent fleet and what it learned: which agents are working now, which stopped at a usage limit and where to resume them, how much headroom each profile has, whether any agent already solved what you are about to ask, and what dreaming changed in your notes overnight. hippo's own health is one dot. It reads hippo's files through `hippo-core` and changes none of them itself: a revert is a `hippo revert` it runs, Looks right is a `hippo review --ok`, a written answer is a `hippo ask`, and a note opens in your own editor. How it fits with the engine is in [Architecture](../docs/site/src/architecture.md).

Status: built and early. It is checked on synthetic data and used on macOS on Apple silicon; whether it can replace the CLI in daily use is not proven yet.

## Build and install

The app is its own Cargo workspace, so the engine's build never compiles GPUI. It needs macOS and a stable Rust no older than the pinned Zed commit's toolchain (1.99.1 at the time of writing).

```sh
app/scripts/bundle.sh                       # release build, then target/Hippo.app, signed ad hoc
TOOLCHAIN=+1.99.1 app/scripts/bundle.sh     # when the default toolchain is older
cp -R app/target/Hippo.app /Applications/
```

The first build compiles GPUI and takes a while; later builds are quick. The bundle is signed ad hoc, not notarized, so the first launch needs System Settings › Privacy & Security › Open Anyway. If your workspace or `~/.hippo` is on a removable volume, macOS asks once whether Hippo may read it.

The app finds hippo the way the nightly jobs do: the `hippo` binary and `HIPPO_HOME` named in `~/Library/LaunchAgents/dev.hippo.ingest.plist` (`hippo schedule install` writes it). Started from a shell with `HIPPO_HOME` set, it uses that home and the `hippo` on `PATH`, and the sidebar's foot says `dev home` (the path is in About).

### Trying it on synthetic data

```sh
cargo build -p hippo
HIPPO_BIN=target/debug/hippo app/scripts/demo-home.sh /tmp/hippo-demo
HIPPO_HOME=/tmp/hippo-demo/home PATH=$PWD/target/debug:$PATH app/target/release/hippo-app
```

`demo-home.sh` builds a home from the synthetic fixtures in this repository: 16 notes; 34 top-level sessions, including the shapes real homes have (a session named by its client with three sub-agents and a failed background command, a context handoff, a session a program started, a live session with sub-agents working now, and T3 Code delegated tasks with their own sessions); and one dreaming run with an applied change you can revert.

## The window

The sidebar on the left is the fleet: Search, the sections (Home, Inbox, Fleet, Incidents, Memory; Manage in the foot beside hippo's health and `indexed hh:mm`), then today's sessions on four shelves, Needs you, Working, To review and Settled. A finished session settles by itself once its pull requests are merged, or, with none, once you have opened it; under the pointer a row offers Settle and Snooze (T3 Code's controls), and snoozed rows wait in a folded Snoozed shelf until their time or until they need you. Under 900 points the sidebar is a rail of icons and one mark per session in play. What you open (a session, a dreaming run or change, a note) shows in the main pane with a back link. The look is T3 Code's blue-black surfaces with Lucide icons (`app/THIRD_PARTY_NOTICES.md`).

- **Home**: who needs you (each waiting session with its one action), the overnight line, the Inbox's note changes, corrections and repeated failures as rows, the Headroom and Memory panes at the right, and the composer at the bottom (Ask or Brief, with the account to use next and its usage ring).
- **Ask**: the sessions and notes that match a question, ranked, each with Open and its resume command, and a written answer that cites them (one model call through `hippo ask`; Manage turns it off).
- **Fleet**: today's sessions (live, stopped at a usage limit, finished) and each profile's headroom in its 5-hour window. `Rows | Lanes` (or `L`) turns it into **Replay**: one local day as lanes, one per thread, with its sub-agents folded into a strip under it; where each worked (a muted fill), waited on you (amber) or stood at a usage limit (dashed), with marks for pull requests, incidents, questions, limits and carry-ons. A line above counts the day (threads, sub-agents, agent work, the time agents waited on you and the longest wait). The scrubber says what stood at any minute; a click pins it. ⌘-scroll or a pinch zooms, a drag pans, `‹ ›` change the day. Made from the rollup and the index, no model. Headroom is an estimate from your own transcripts, not Anthropic's meter, and says "no estimate" until a profile has hit a limit in the last 8 days. Resume copies the command and a short handoff; on another profile when this one is at its limit.
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
| L | In Fleet: Rows or Lanes; in Lanes, Esc lets a pinned time go, ↑ ↓ and Return open a session at its step |
| ⌘O, ⌘⇧R | Open the note in your editor; reveal it in the Finder |
| ⌘⌥Z | Revert the change shown (asks first; ⌘Return confirms, Return or Escape cancels) |
| ⌘⇧T | Theme picker |
| ⌘R | Reload |
| ⌘= ⌘- ⌘0 | Text size for this session |

Every action is also in the menu bar, and everything the app shows has a CLI command or a file behind it: `hippo ask`, `hippo fleet`, `hippo resume`, `hippo review --ok`, `hippo log`, `hippo show session|proposal|note`, `hippo context`, `hippo revert`, and the reports under `.hippo/reports/`.

## Themes

Two themes are built in, Hippo Light and Hippo Dark; **System** follows macOS's appearance. Add your own as JSON files in `$HIPPO_HOME/themes/` (`~/.hippo/themes/`), and pick one with ⌘⇧T. The choice (and whether Ask writes answers) is saved in `~/Library/Application Support/dev.hippo.app/preferences.json`, the one file the app writes. An edited theme file shows within about two seconds; a file that stops parsing keeps the last good version and the sidebar's foot says so.

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

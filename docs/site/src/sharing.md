# Sharing a brain in a team

Several people can share one notes folder through git, and each person's agents then start from the same notes. Sharing is through git only: nothing syncs live, and there are no accounts. This page says what is shared, what stays with each person, and what does not work yet.

**In one line:** commit the notes and the workspace's shared files; `hippo init` already ignores everything that is per person, so without a shared Inbox `git add -A` stages nothing personal. With `[team] inbox = "shared"`, `.hippo/inbox/` holds text drawn from people's corrections: read `git status` before you commit. Each person's dreaming learns from their own sessions.

## What is shared and what is per person

Nothing in `$HIPPO_HOME` is shared: the configuration, the index and the rollup are per machine. In the workspace:

| Path | In a team | Why |
|---|---|---|
| Notes under `notes_dir` | **Shared** | The brain itself. |
| `.hippo/hippo.toml`, `.hippo/.gitignore`, `.hippo/packs/` | **Shared** | Settings and passes everyone runs with. |
| `.hippo/proposals/`, `.hippo/receipts/` | **Shared** | A teammate can see why a note changed and who reviewed it. Names never collide. |
| `.hippo/reviews/` | **Shared** | A person's "looks right" mark on a change, one file per receipt. |
| `.hippo/inbox/` | **Shared**, with `[team] inbox = "shared"` | Lesson identities and decisions: each person's wording of a lesson, their counts and their keep or dismiss, one file per person. No quotes and no session ids. See [The shared Inbox](#the-shared-inbox). |
| `.hippo/sessions/` | Per person, ignored | What your agents did: task text and handoffs. |
| `.hippo/dream-state.json` | Per person, ignored | Your dreaming window. Shared, one person's run would move everyone's. |
| `.hippo/reports/` | Per person, ignored | One file per day per machine; two machines write the same name. |
| `.hippo/snapshots/` | Per person, ignored | The before-text `hippo revert` restores on the machine that dreamed the change. Git's history holds the same text for everyone else. |
| The session log (`log_path`, by default `<notes_dir>/Session log.md`) | Per person, ignored | Generated from your own records, so it always differs. |
| `.claude/settings.local.json` | Per person | hippo's Claude Code hooks, with the absolute path of your `hippo`. hippo writes nothing to the shared `.claude/settings.json`; `hippo doctor` warns if git tracks the local file. |

**Versions.** `hippo.toml` can set `min_hippo_version`: a teammate's older hippo then refuses to dream, scan lessons, review, revert or add packs in the workspace (hooks and `context` keep working), and so does a hippo that finds a setting it does not know. Everyone should be on 0.5.0 or later before anyone turns on a team, guard or secrets setting: a hippo older than 0.5.0 has neither check and ignores what it does not know. Proposals, receipts and review marks name the hippo that wrote them (`hippo_version`).

## The shared Inbox

Without it, each person's lessons wait in their own Inbox (`$HIPPO_HOME/lessons/`): when Alice keeps "staging only, never production", Bob's agents make the same mistake until she pushes the note, and Bob's scans draft the same rule again. With it, the team shares a lesson's identity and the decisions on it.

It takes two settings, one for the workspace and one per person:

```toml
# .hippo/hippo.toml (shared)
[team]
inbox = "shared"
```

The workspace flag is a loosening: it waits until each person runs `hippo accept` on their machine ([Security](security.md#the-accept-rule)). Then each person decides what of theirs may be written. `hippo lessons share` (you, at a terminal; never an agent) asks for your handle, shows every file it would write, and on yes sets `member` and `share_lessons = "rules"` in your `config.toml`. Until then your lessons stay local, and you still read the team's Inbox and record decisions.

- **What is shared.** A correction lesson whose every evidence session ran in the workspace or one of its worktrees, once the reviewer confirmed it: its title, rule, target note, match terms and the reviewer's verdict (`lesson-<you>.json`), and your counts (`seen-<you>.json`: sessions, first and last seen, agents). No quotes, no session ids. A lesson with any evidence from another repository, and every failure lesson, stays local.
- **No duplicate drafts.** Before drafting, a scan compares each correction with the shared lessons' terms in code. A match the team kept or dismissed is not drafted (a dismissed one reopens after three more sessions, counted across everyone); a match without a decision only adds your counts. That saves the model call and gives one entry "seen by alice 3, bob 1".
- **Decisions.** `hippo lessons keep L-…` writes the rule into its note as any keep (a receipt; `hippo revert` undoes it on your machine) and `decision-<you>-<ulid>.json`. `hippo lessons dismiss L-… --reason wrong|not-for-us` records a dismissal and never edits a note; `--reason duplicate --of L-…` links two entries made by people who scanned before pulling, and a decision on either then counts for both. A lesson is kept if anyone kept it; a keep and a dismissal by different people are listed together ("crossed") so the two can talk.
- **Markers.** A kept bullet ends with `<!-- hippo:lesson L-… -->`, which Obsidian and GitHub hide. Keep refuses when the note already has the lesson's marker; two keeps made before either pulled both land, and `doctor`'s `team` row names the duplicate line to delete. `hippo lessons markers` adds markers to bullets kept before 0.6.0 (it prints them, writes with `--yes`, and `hippo revert` removes them).
- **What you see.** `hippo lessons` lists the shared Inbox above your own lessons. Names and verdicts written by others are shown as written, with the git commit author of each file beside them: a handle is a label, not an identity.

`hippo doctor`'s `team` row reports crossed decisions, duplicate markers, members whose latest file was written by a hippo below `min_hippo_version`, and a `.hippo/inbox/` that git ignores.

## What `init` writes

`hippo init` makes sure of three files, adding only the lines that are missing. It never removes or reorders a line, so your own `.gitignore` and `.gitattributes` are kept:

- `.hippo/.gitignore`: `snapshots/`, `sessions/`, `reports/`, `dream-state.json`;
- the workspace's `.gitignore`: `/<log_path>`, for example `/notes/Session log.md`;
- the workspace's `.gitattributes`: `/<notes_dir>/**/*.md merge=union`, so two dreams that append to the end of the same note merge without a conflict. A union merge keeps both appends, each once, in the order the pull meets them; read the note after a merge.

In a workspace set up before these rules, run `hippo init` with no flags in it: it adds the missing lines and changes nothing else. `hippo doctor` has a `git` row that warns when one of these files is not ignored, or when git already tracks one, and names the commands that fix it:

```sh
git rm -r --cached -- .hippo/sessions
git commit -m "Stop tracking per-person hippo files"
```

## Only your workspace's sessions leave a record

`hippo ingest` indexes every transcript on your machine into your own `$HIPPO_HOME`, so search, the overview, `fleet`, `incidents` and the card see all of your work. It writes a session record into the workspace only for a session that ran inside the workspace's folder (or one of its git worktrees), read, edited or cited one of its notes, or declared itself there through the hooks. A session of another project leaves nothing in the workspace. A workspace written by an older version is cleaned on the next ingest, which says how many records it removed and why. Session records are ignored as well, so even a workspace session's task text stays on your machine.

## Each person

Once, after cloning:

```sh
hippo init        # no flags: reads the workspace's .hippo/, writes your own config
hippo ingest
```

Runners are per machine: if the `claude` CLI is not on your `PATH`, or you want another account, model or CLI for hippo's own calls, add one with `hippo runners add` ([Configuration](configuration.md#runners-which-cli-model-and-account-hippos-own-calls-use)); receipts name the runner, never its account folder.

Then, as usual: work with the hooks (`hippo hooks install`), let dreaming run, and commit:

```sh
git pull --rebase
git status          # with a shared Inbox, read what .hippo/inbox/ adds
git add -A
git commit -m "notes"
git push
```

## What was tried

Two people, each with their own home folder and `$HIPPO_HOME`, cloned one workspace from a shared git remote. Each had a session in the workspace and one in another project, ran a session start and end with a handoff linking the same note, wrote the session log, dreamed (a stand-in model, so both appended to the same note), committed with `git add -A`, pushed, and pulled the other's commits with `git pull --rebase`. No per-person file was ever staged, the pull had no conflict, the note held both appends once each, neither person's dreaming window moved, the other project's sessions left no record, and `hippo doctor` stayed quiet. This runs as a test (`crates/hippo/tests/team.rs`) with every `cargo test`.

## What does not work yet

- **Dreaming learns only from your own sessions.** Handoffs live in `.hippo/sessions/`, which stays on your machine, so your dream never sees a teammate's handoffs, and theirs never sees yours. Each person who wants their sessions folded into the notes runs dreaming themselves.
- **`hippo revert` works only on the machine that dreamed the change.** The snapshot it needs is there. On another machine, git is the undo: `git log -p -- <note>` shows the commit, and `git revert` or an edit undoes it. `revert` says so when it refuses.
- **A teammate's evidence is not in your index.** `hippo show proposal` on their proposal prints `(not in the index)` for each evidence turn, and `hippo xray` or `resume` on their session says there is no such session.
- **Nothing coordinates two people's notes.** There is no owner on a session record, no live sync and no lock across machines. The shared Inbox shares lessons and decisions only; two dreams can still propose changes to the same paragraph, and git reports the conflict.
- **Quotes, the team dreamer and templates** are later steps ([team-brain.md](https://github.com/VanitasCaesar1/hippo/blob/main/docs/designs/team-brain.md#rollout)). Hippo.app's Inbox shows your own lessons; the shared entries are in `hippo lessons` for now.

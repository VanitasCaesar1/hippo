# Sharing a brain in a team

Several people can share one notes folder through git, and each person's agents then start from the same notes. Sharing is through git only: nothing syncs live, and there are no accounts. This page says what is shared, what stays with each person, and what does not work yet.

**In one line:** commit the notes and the workspace's shared files; `hippo init` already ignores everything that is per person, so `git add -A` is safe. Each person's dreaming learns from their own sessions.

## What is shared and what is per person

Nothing in `$HIPPO_HOME` is shared: the configuration, the index and the rollup are per machine. In the workspace:

| Path | In a team | Why |
|---|---|---|
| Notes under `notes_dir` | **Shared** | The brain itself. |
| `.hippo/hippo.toml`, `.hippo/.gitignore`, `.hippo/packs/` | **Shared** | Settings and passes everyone runs with. |
| `.hippo/proposals/`, `.hippo/receipts/` | **Shared** | A teammate can see why a note changed and who reviewed it. Names never collide. |
| `.hippo/reviews/` | **Shared** | A person's "looks right" mark on a change, one file per receipt. |
| `.hippo/sessions/` | Per person, ignored | What your agents did: task text and handoffs. |
| `.hippo/dream-state.json` | Per person, ignored | Your dreaming window. Shared, one person's run would move everyone's. |
| `.hippo/reports/` | Per person, ignored | One file per day per machine; two machines write the same name. |
| `.hippo/snapshots/` | Per person, ignored | The before-text `hippo revert` restores on the machine that dreamed the change. Git's history holds the same text for everyone else. |
| The session log (`log_path`, by default `<notes_dir>/Session log.md`) | Per person, ignored | Generated from your own records, so it always differs. |

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

`model_command` is per machine: set it with `hippo config set model_command …` if the `claude` CLI is not on your `PATH`, or your first dream fails.

Then, as usual: work with the hooks (`hippo hooks install`), let dreaming run, and commit:

```sh
git pull --rebase
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
- **Nothing coordinates two people.** There is no owner on a session record, no per-person folder inside `.hippo/`, no live sync and no lock across machines.

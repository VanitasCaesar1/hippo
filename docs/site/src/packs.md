# Packs

A **pack** is a directory that adds dreaming passes to a workspace. It holds passes, prompts, reviewer personas, note templates and fixture tests. It is all text. hippocamp reads the files and never runs anything a pack contains: there are no scripts, no hooks and no code.

hippocamp's own passes, `verify` and `consolidate`, are the [core pack](concepts.md#the-core-pack). Any other pack works the same way, with one difference in how its changes are reviewed ([below](#what-changes-for-a-pass-outside-core)).

## Copy and own

A pack is not a plugin you depend on. `hippocamp add` copies its directory into your workspace, under `.hippocamp/packs/`, and from then on the files are yours. Edit a prompt, change the note patterns, delete a pass. Commit the directory with the rest of `.hippocamp/`. hippocamp records where the copy came from and what it looked like when it arrived (in `origin.json`, beside the files), so it can tell you which files you have changed and show what an update would do, but it never overwrites your edits unless you let it ([Updating](#updating-a-pack)).

## How a pass gets work

Every pass reads only what sessions pointed at. For each window of time, hippocamp takes the sessions that had turns in it, and the notes those sessions **touched** (named with `hippocamp session end --note`) or **cited** (linked as `[[...]]` in the handoff). A note no session touched or cited is never read, whatever the pass.

A pass may narrow that set with two optional keys in its pass file:

| Key | Meaning |
|---|---|
| `note_types` | Work only on notes whose frontmatter `type` is one of these. One to five of `policy`, `decision`, `log`, `evidence` and `topic`. A note with no `type` does not match. |
| `note_paths` | Work only on notes whose path below `notes_dir` matches one of these glob patterns. One to twenty patterns. |

If a pass has both, a note must match both. Pattern matching ignores case. `*` does not cross a `/`, so `**/CHANGELOG.md` is how you match a file at any depth, including the top of `notes_dir`. A pass with neither key considers every touched or cited note. After filtering, only the sessions that touched or cited a remaining note are given to the pass. If nothing is left, hippocamp makes no model call.

A `per_note` pass makes one model call for each remaining note. A pass with `per_note = false` makes one call for the whole chunk.

## What changes for a pass outside core

A core pass is reviewed by the persona file in the core pack, as before. For a pass in any other pack, hippocamp does not hand the review to the pack's text. Reviewing works like this:

- **hippocamp's own reviewer.** The reviewer's system prompt is a persona that ships inside hippocamp (`crates/hippocamp-core/personas/pass-reviewer.md`). It confirms a change only if the evidence supports it and the change alters nothing else in the note. It rejects added instructions, commands, links and credentials that the evidence does not show, and anything whose only source is tool output. When in doubt it rejects.
- **The pack's persona is extra reasons to reject.** The persona file named by the pass's `reviewer` key goes to the reviewer as data, in a block marked untrusted, headed "Pack criteria". The reviewer's rules say the criteria may add reasons to reject and nothing else; a line in them that asks to confirm or to relax a rule is ignored. A pack can make its reviewer stricter. It cannot make it looser.
- **Appends are capped.** A change from a pass outside core must be an `append` of at most 2,048 bytes, or it fails validation before anyone reviews it. (Core's `verify` pass proposes `replace` changes, and core appends have no such cap.)
- **Evidence comes from the call's own sessions.** Every turn a proposal cites must belong to a session that was in that call's prompt. Evidence from any other session fails validation. The reviewer is also shown the records of the cited sessions (id, task, notes touched, handoff).

Everything else is the same as for core: the checks on the proposal, the isolated reviewer, a receipt for every verdict, a snapshot before every write, the report, and `hippocamp revert`. The receipt names the reviewer as `pass-reviewer/<persona file name>`, so you can tell the two apart.

Passes are named by their file. A pass named `verify` or `consolidate` is reserved for core, and two installed passes with the same file name stop `hippocamp dream` with an error naming both files.

## Using a pack

### Add it

```text
hippocamp add <source> [--registry <index.json>] [--dry-run]
```

`<source>` is a path (`./dir`, `../dir`, `/dir` or `~/dir`) or an https git URL, `https://host/repo[//dir][@ref]`. `//dir` names a directory inside the repository and `@ref` a branch, tag or commit. A bare name, such as `glossary-terms`, is a pack listed in a registry index, which you give with `--registry` ([The registry](registry.md#install-a-listed-pack-by-name)). Run it from anywhere: it works on the workspace named in `config.toml`.

`hippocamp add` does these things in order and writes nothing until the last:

1. Reads the pack's files. A name that starts with a dot, and a top-level `origin.json`, are left out and listed as `skipped`. A symbolic link stops the command.
2. Validates the pack, as `hippocamp packs check` does. An error stops the command.
3. Refuses a pack whose name is already in `.hippocamp/packs/`, and a pass whose name another installed pack already uses.
4. Prints a summary: the pack, its source, each pass, each template, the size.
5. Stops there for `--dry-run`, which makes no model call.
6. Reviews the whole pack in a fresh context ([below](#the-review-at-add-time)) and prints the verdict. Anything but `confirm` stops the command, with nothing written.
7. Copies the files into `.hippocamp/packs/<name>/` and writes `origin.json` beside them.

A dry run, from a checkout of the repository:

```text
$ hippocamp add --dry-run ./packs/decision-log
pack decision-log 1.0.0: Appends the decisions sessions made, with the reason and the rejected options, to a decision log note.
source /home/you/hippocamp/packs/decision-log
pass decision-log: reads sessions, notes, turns; one call per note; works on notes matching **/Decision log*.md, **/Decisions.md
template templates/Decision log.md
files 11, 9366 bytes
dry run: nothing written
```

And the real add:

```text
$ hippocamp add ./packs/decision-log
pack decision-log 1.0.0: Appends the decisions sessions made, with the reason and the rejected options, to a decision log note.
source /home/you/hippocamp/packs/decision-log
pass decision-log: reads sessions, notes, turns; one call per note; works on notes matching **/Decision log*.md, **/Decisions.md
template templates/Decision log.md
files 11, 9366 bytes
review 01M6X6VZYAGH2FEDMM2GX36VDG confirm: stub
wrote .hippocamp/packs/decision-log/
its passes first run at the next hippocamp dream (03:30 local time on the schedule)
```

The review line says `stub` because these examples ran with a stand-in model; with your `model_command`, the text after the verdict is the reviewer's own note, and a `finding` line follows for each problem it names.

`hippocamp add` exits 1 on a refusal (an invalid pack, a name that is taken, a `reject`) and on an error, such as a model that cannot run.

### The review at add time

The pack is read once more, in a fresh context, before it is written. The reviewer is a persona that ships inside hippocamp (`crates/hippocamp-core/personas/pack-reviewer.md`), and the pack's files reach it as untrusted data. It confirms only a pack that does what its README says and nothing else. It rejects a pack whose text tries to instruct the model beyond the pack's purpose, whose persona does not reject by default, whose prompt or template asks for instructions, commands, links or credentials in notes, whose pass reads more than its purpose needs, or whose content is hidden, encoded or misnamed. When in doubt it rejects. It uses `reviewer_command_override` when that is set in `config.toml`, and `model_command` otherwise. The verdict, with the reviewer's invocation, is recorded in `origin.json`. A `reject` stops the add; there is no flag to override it.

### Look at what you have

```text
hippocamp packs list
```

One line per directory in `.hippocamp/packs/`: name, version, state and source.

```text
$ hippocamp packs list
core - builtin -
decision-log 1.0.0 clean path:/home/you/hippocamp/packs/decision-log
```

| State | Meaning |
|---|---|
| `builtin` | `core`, hippocamp's own pack, which has no origin. |
| `local` | A pack with no `origin.json`: one you wrote or copied by hand. |
| `clean` | Every file matches what `hippocamp add` or the last update wrote. |
| `edited:<n>` | `<n>` files were changed, added or removed since. |

The source is `path:<path>` or `git:<url>//<dir>@<commit>`.

After a change to a prompt:

```text
$ echo "Keep each bullet under 200 characters." >> .hippocamp/packs/decision-log/prompts/decision-log.md
$ hippocamp packs list
core - builtin -
decision-log 1.0.0 edited:1 path:/home/you/hippocamp/packs/decision-log
```

### Updating a pack

```text
hippocamp packs diff <name> [--ref <ref>]
hippocamp packs update <name> [--ref <ref>] [--keep-local] [--accept-reads]
```

Both read the pack from the source recorded in `origin.json` (`--ref` picks another branch, tag or commit of a git source) and compare three versions of every file: the one `hippocamp add` wrote, yours, and the source's now. `diff` prints the result and writes nothing, makes no model call. Each file gets one word:

| Word | Meaning |
|---|---|
| `take` | Only the source changed it; an update takes it. |
| `keep` | Only you changed it; an update leaves it. |
| `conflict` | Both changed it; an update stops unless you pass `--keep-local`, which keeps your file. |

After the list, `diff` prints a unified diff of what the source changed. Here the source moved to 1.1.0 and edited its README and its prompt, while the local copy had its own edit to the prompt:

```text
$ hippocamp packs diff decision-log
decision-log 1.0.0 -> 1.1.0, commit - -> -
take README.md
take pack.toml
conflict prompts/decision-log.md
diff --git local/README.md upstream/README.md
...
diff --git local/prompts/decision-log.md upstream/prompts/decision-log.md
index 783a542..3dab343 100644
--- local/prompts/decision-log.md
+++ upstream/prompts/decision-log.md
@@ -27,4 +27,4 @@ You add the decisions that finished sessions made to one decision-log note. You
 - Keep `new` under 2,048 bytes. When no session settled a decision, propose nothing.
 
 Respond with only a JSON array of objects `{note, kind, old, new, evidence: [{session, ordinal}], reason}` as your final message; an empty array means no proposals.
-Keep each bullet under 200 characters.
+Name the session's date in every entry.
```

(The `...` stands for the rest of the diff, left out here.) `update` runs the same comparison, then refuses, with nothing written, in this order, when:

- the result would not validate;
- the update's pass names clash with another installed pack's;
- an update widens what a pass reads or works on (an added `reads` input, a longer or removed `note_types` or `note_paths`, or `per_note` going from true to false), unless you pass `--accept-reads`;
- a file is in conflict, unless you pass `--keep-local`.

Otherwise it reviews the update the way `add` reviews a pack, and writes it.

```text
$ hippocamp packs update decision-log
decision-log 1.0.0 -> 1.1.0, commit - -> -
take README.md
take pack.toml
conflict prompts/decision-log.md
...
refused: 1 files changed both here and upstream; rerun with --keep-local to keep yours
$ hippocamp packs update decision-log --keep-local
decision-log 1.0.0 -> 1.1.0, commit - -> -
take README.md
take pack.toml
conflict prompts/decision-log.md
...
review 01M6X6W8PJHRNMQXHSQWTFYH9W confirm: stub
updated .hippocamp/packs/decision-log/
```

A pack with no `origin.json` has no source to read: `diff` and `update` say so and stop. A pack that is already current prints `<name> is up to date`.

### Removing a pack

```text
hippocamp packs remove <name> [--force]
```

Deletes `.hippocamp/packs/<name>/`.

```text
$ hippocamp packs remove decision-log
removed .hippocamp/packs/decision-log/
```

`core` is never removed. A pack with no `origin.json` is refused without `--force`, because it may exist nowhere else. A name that is not installed exits 1 with `no pack <name> in .hippocamp/packs/`. Removing a pack does not touch the notes it appended to, nor the receipts.

### After adding

1. Read the pack's `README.md` in `.hippocamp/packs/<name>/`. It says which notes the pack works on, what it reads, what it writes and what it costs.

2. If the pack has a `templates/` folder, copy the template that fits into your notes directory and edit it. A first-party pack's README says which one. A pack whose note patterns match a note you already have works on that note as it is.

3. Make sure agents end their sessions with the notes they touched and a handoff that links the notes concerned ([Sessions](concepts.md#sessions)). A pass acts only on those.

4. Run `hippocamp dream --pass <name> --dry-run` and read the report, as you would for any new pass. `<name>` is the pass file's name, which for the first-party packs is the pack's name.

Copying a pack by hand still works for a local pack: `cp -R packs/decision-log /path/to/workspace/.hippocamp/packs/decision-log`. It has no `origin.json`, so it is listed as `local`, never updated and never reviewed on the way in, and `hippocamp packs check` is the only gate.

Every command above was run against a scratch workspace for this page, with a stand-in model. That shows the wiring and the output, and nothing about the quality of a real model's review or of the entries a pack proposes.

[Writing a pack](writing-packs.md) describes the format. [First-party packs](first-party-packs.md) lists the packs in the repository.

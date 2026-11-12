# Concepts

## The four layers

| Layer | What it is | Where it lives |
|---|---|---|
| Transcripts | What your agents said and did, indexed turn by turn | `$HIPPO_HOME/index.sqlite`, rebuilt from the transcript files at any time |
| Notes | Markdown files you and your agents edit | `notes_dir` in your workspace |
| Wiki | The `[[links]]` between notes | In the notes themselves |
| Injected context | A compiled block of text for the start of a session | Printed by `hippo context` and the hooks; never stored |

Dreaming moves information between the layers: it checks notes against transcripts and appends what finished sessions learned.

Files are the contract. Everything hippo writes is Markdown, JSON or the SQLite index, and the index is the only one of those that cannot be edited by hand. You can delete the index and rebuild it with `hippo ingest --full`.

## Notes and frontmatter

A note is a Markdown file below `notes_dir`. Hidden files and directories (names starting with `.`) are ignored. hippo reads these frontmatter keys, and leaves any other key alone:

```yaml
---
type: policy
status: current
updated: 2026-11-07
review_after: 2026-12-05
sources: ["Sources/Register.md"]
---
```

| Key | Meaning |
|---|---|
| `type` | Free text. The conventional values are `policy`, `decision`, `log`, `evidence` and `topic`. hippo does not check the value. |
| `status` | Free text. The conventional values are `current`, `planned` and `superseded`. |
| `updated` | A date, `YYYY-MM-DD`. Breaks ties when ranking relevant notes. |
| `review_after` | A date, `YYYY-MM-DD`. The note is **overdue** once this date is in the past (UTC). |
| `sources` | A list of paths the note's claims come from. Dreaming's `verify` pass searches transcripts for these paths. |

Frontmatter is optional. A note without it still works for context and links. hippo preserves a note's frontmatter and body byte for byte when it edits.

The linter, which `hippo doctor` runs and `hippo context` summarises, reports four findings. Only a frontmatter block that is not valid YAML is an error. The others are warnings:

- `missing frontmatter`: the note has none.
- `bad frontmatter`: a known key has the wrong shape (for example `updated: soon`); hippo then treats the key as absent. A block that is valid YAML but not a mapping is also `bad frontmatter`.
- `dangling link`: a `[[link]]` that resolves to no note.
- `overdue`: `review_after` is before today.

A note's **title** is its first line that starts with `# `, or its file name without `.md` if it has none. A link `[[X]]` (any `|alias` or `#heading` suffix is ignored) resolves to `<notes_dir>/X.md` if that file exists. Otherwise, if `X` has no `/`, it resolves to the one note whose file name is `X.md`. If two notes share the name, or none has it, the link dangles.

## Sessions

A **session** is one run of an agent. hippo keeps one JSON record per session in `.hippo/sessions/<id>.json`. There are two kinds:

- A **declared** session is written by `hippo session start` or by the Claude Code hooks. Its id is the tool's own session id. A declared session records its task, the bytes of context it was given, the notes it touched, and a handoff. Only `hippo session` and `hippo hook` write a declared record.
- A **derived** session is created by `hippo ingest` for a transcript that has no declared record, when the session belongs to the workspace: it ran inside the workspace's folder (or one of its git worktrees), or read, edited or cited one of its notes. Sessions of other projects are indexed in `$HIPPO_HOME` but get no record. A derived record has no task and touches no notes. For Codex, dsh, OpenCode, Muse Code and Command Code, the handoff is the last assistant message, and the record marks it as derived. Only ingest writes a derived record. If you later declare a session with the same id, the declared record replaces the derived one.

Session ids may contain letters, digits, `_` and `-`, up to 128 characters, because they become file names. Derived Claude Code ids are the transcript's session id, or `<sessionId>-<file name>` for a nested sub-agent transcript. Derived Codex ids are `codex-<id from the rollout>`. Derived dsh ids are `dsh-<session id>`, or `dsh-<top session id>-agent-<session id>` for a sub-agent; derived OpenCode ids are `opencode-<session id>`, or `opencode-<top session id>-agent-<session id>` for a sub-agent; derived Muse Code ids are `muse-<session id>`, and Command Code ids `command-code-<session id>`. An id with other characters is replaced by a hash of it.

A declared session ends with `hippo session end`. The three things it can record are the notes the session touched (`--note`, repeatable), a handoff (`--handoff`) and, when it changed nothing, a reason (`--no-change`). If you give none of them, the session is recorded as ended with the reason `unspecified`, which is what the `session-end` hook does.

A **handoff** is Markdown that names notes with `[[...]]` links. Dreaming's `consolidate` pass reads the handoffs of ended sessions and proposes to append what they learned to the notes they link.

`hippo log` writes the **session log** from the session records, the index and the receipts: one line per session, newest first, to `log_path` (by default `<notes_dir>/Session log.md`). It is generated, so do not edit it by hand.

## The context budget

`hippo context` builds a block of UTF-8 text from six parts, in this order:

1. A **warning line**, when dreaming is overdue or failed: `warning: no dreaming run recorded`, `warning: last dreaming success <time>` (older than 36 hours), or `warning: last dreaming run failed`. Several can appear. Before any dreaming report exists there is no warning.
2. The **entry section**: the entry note's text before its first `## ` heading.
3. **Required notes**: one `required: <path>` line per entry of `required_notes`.
4. The **session line**, with `--session`: `session: <id> <tool> <started>`.
5. **Relevant notes**, with `--task`: `relevant:` then up to five lines `- <title>: <first body line>`. The task is split into words of three or more letters or digits; a note scores one point for each word found in its title or in any of its headings. Notes with a score of zero are left out. Ties go to the most recently `updated`, then to the path.
6. **Linter counts**: `linter: overdue=<n> dangling=<m>`.

Parts 1 to 4 are **required**; parts 5 and 6 are **optional**. The budget is `budget_bytes` from the workspace settings, 25,600 bytes by default, or `--budget`. An optional part is included only if it fits whole. A part that does not fit is left out, and hippo prints `omitted: <part> (<bytes>)` on stderr. If the required parts alone exceed the budget, the command fails:

```text
context over budget: 70 > 10
```

and exits 1. This is deliberate: a context block that silently grows is worse than a loud failure. `sample_tasks` in the workspace settings lets `hippo doctor` check the budget for tasks you care about.

The hooks split the block: `session-start` prints parts 1 to 4 and records their size; `prompt` prints parts 5 and 6 on the first prompt, within what is left of the budget.

## Dreaming

Dreaming is `hippo dream`: one run over a window of time. It is meant to run on a schedule, and you can run it by hand.

### Passes

A **pass** is a data file, `.hippo/packs/<pack>/passes/<name>.toml`, that names a prompt, what the pass may read, the reviewer persona that must confirm its proposals, and an input budget. `hippo dream` runs every pass of every pack in the order of `<pack>/<name>`. `--pass <name>` runs only the pass with that file name, such as `verify`. Passes are data: hippo does not run anything a pack contains.

hippo ships two passes in the core pack:

- **`verify`** checks one note at a time against transcript turns found by searching for the note's title, headings and `sources`. It proposes `replace` changes that correct claims the turns contradict. Text a human has confirmed, by reverting a change, is shown to it as true.
- **`consolidate`** reads sessions that ended with a handoff and proposes `append` changes to the notes the handoff links.

### The window

A run first ingests new transcripts, so the last minutes before it are covered. It then reads `.hippo/dream-state.json`. The window runs from `last_success` to now, in chunks of 24 hours, and `last_success` advances after each completed chunk. The first run covers the previous 24 hours. If a chunk fails, the window stays at that chunk, and the next run starts there. A chunk with no sessions, or no notes to work on, makes no model call.

### Proposals and the reviewer

For each chunk and pass, hippo builds a prompt and runs `model_command`. The model replies with a JSON array of proposals. A proposal has a note, a kind (`replace` or `append`), the old and new text, a reason, and evidence: `{session, ordinal}` pairs naming transcript turns.

hippo checks each proposal before anyone reviews it. A proposal that fails is reported and never reviewed. The checks include:

- the note must be a visible `.md` file below `notes_dir`, and one that went into the prompt;
- the evidence must exist in the index;
- a `replace` must name `old` text that occurs exactly once, its new text must quote at least one evidence turn word for word, and `old` plus `new` may not exceed half the note's size;
- an `append` has an empty `old`.

A valid proposal is written to `.hippo/proposals/<id>.json`, and it is never overwritten. It then goes to the **reviewer**, a second model run whose system prompt is a persona file such as `personas/verify-reviewer.md`. The reviewer sees the persona, the note, the change and the evidence turns. It does not see the proposing run's reasoning or its stated reason. It must reply with one JSON object, `{"verdict": "confirm"|"reject", "notes": "..."}`. Any other reply counts as a rejection. If the reviewer cannot run at all (the model is unreachable), the run fails instead of counting rejections.

### Receipts, snapshots and revert

Every reviewed proposal gets a receipt in `.hippo/receipts/<id>.json` with the reviewer, the verdict, hashes of the note before and after, and the invocation. For a confirmed proposal, hippo then:

1. copies the note to `.hippo/snapshots/<proposal id>/<note path>`;
2. applies the change atomically;
3. records `applied` and the snapshot path in the receipt.

hippo writes a note only after a confirming receipt, and only if the note is unchanged since the prompt was built. If you or an agent edited the note in the meantime, the receipt is written with `applied` empty and the note is left alone.

`hippo revert <receipt>` restores the note from its snapshot and writes a revert receipt. It refuses if the note was edited after the change, unless you pass `--force`. Reverting also tells `verify` to treat the reverted text as true for as long as it stays in the note, so the same change is not proposed again.

Proposals, receipts and reviews are meant to be committed by you, so a teammate can see why a note changed. Snapshots, reports, session records, `dream-state.json` and the session log stay with each person: `hippo init` writes the git ignore rules for them, and `hippo doctor` warns when git tracks one ([Sharing a brain in a team](sharing.md)). hippo never commits or pushes; `doctor` only reads what git tracks. A snapshot exists only on the machine that dreamed the change, so on another machine git is the undo.

### The report

Each run writes `.hippo/reports/<date>-dream.md` (the UTC date). A later run on the same day replaces that report only when it was a dry run's; otherwise it writes `<date>-2-dream.md`, `<date>-3-dream.md` and so on, so no real run's report is replaced. Line 1 is `status: ok` or `status: failed`; a failed run adds `error: <text>` on line 2. The sections are `Window`, `Inputs`, `Omissions`, `Proposals`, `Validation failures`, `Verdicts` and `Applied`. `hippo dream --dry-run` writes only this report.

## The core pack

A **pack** is a directory of passes, prompts and personas. The core pack is the one hippo writes for you. `hippo init` writes the core pack to `.hippo/packs/core/`:

```text
.hippo/packs/core/
  passes/consolidate.toml   passes/verify.toml
  prompts/consolidate.md    prompts/verify.md
  personas/consolidate-reviewer.md   personas/verify-reviewer.md
```

Other packs have the same layout; see [Packs](packs.md). The files are plain text and belong to you. Edit a prompt or a persona and the next dream run uses it. Commit them with the rest of `.hippo/`. A pass file looks like this:

```toml
protocol = "HIPPO_V1"
prompt = "prompts/verify.md"
reads = ["sessions", "notes", "turns", "human_confirmed"]
reviewer = "personas/verify-reviewer.md"
input_budget_bytes = 200000
per_note = true
```

Every key except `input_budget_bytes` is required.

| Key | Meaning |
|---|---|
| `protocol` | Must be `HIPPO_V1`. |
| `prompt` | The prompt file, relative to the pack and inside it. |
| `reads` | What the pass may be given: any of `sessions`, `notes`, `turns`, `human_confirmed`. A prompt may use only the placeholders `{{sessions}}`, `{{notes}}`, `{{turns}}` and `{{human_confirmed}}` that match `reads`; any other placeholder is an error. |
| `reviewer` | The persona file for the reviewer, relative to the pack and inside it. |
| `input_budget_bytes` | The most bytes a prompt may have. Optional; default 200000. |
| `per_note` | `true` to run the pass once per note, `false` to run it once per chunk. |

The loader refuses symbolic links, prompts and personas over 64 KiB, and two passes with the same name. `hippo init --force` overwrites the core pack's files, including your edits.

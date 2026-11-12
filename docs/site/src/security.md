# Security and privacy

hippo reads agent transcripts, which are sensitive, and writes to files that agents load at the start of every session. This page says what it does with them.

## When data leaves your machine

hippo's own code makes no network calls: it has no HTTP client among its dependencies. The index, the records and the reports are local files. Data leaves your machine only through two kinds of child process that hippo starts.

**A. The model command.** `model_command` is by default `claude -p …`, which sends the prompt to Anthropic under your own login. If you point `model_command` at another command, that command decides where the prompt goes. These commands run it:

- `hippo dream`, including `--dry-run`;
- the lessons pass that follows `dream` (skip it with `--no-lessons`);
- `hippo lessons scan`;
- `hippo ask`, unless you pass `--sources-only`;
- `hippo add` and `hippo packs update`, for the pack review;
- `hippo eval` runs that use dreaming with a model (`--with-model`);
- `hippo doctor --model`, which sends one word;
- the scheduled nightly dream.

**B. Git over HTTPS, for packs.** `hippo add <url>`, `hippo add` with `--registry`, `hippo packs update`, and `hippo packs diff` for a git-sourced pack fetch the pack with `git`.

Everything else is local: `hippo` and `hippo start`, `detect`, `init`, `ingest`, `recall`, `ask --sources-only`, `fleet`, `resume`, `xray`, `incidents`, `context`, `brief`, `show`, `session`, `hook`, `hooks`, `config`, `log`, `review`, `revert`, `packs list` and `packs check`, `doctor` without `--model`, and `schedule`.

**The agent you run hippo inside.** The Claude Code plugin's start block and `/hippo:ask` and `/hippo:day` output, and the DeepSeek Harness and OpenCode plugins' start blocks and their `hippo_ask` and `hippo_day` tool results, become part of that agent's session, and the agent sends its session to its own model provider. hippo does not send them; the agent does, as with any file it reads. In Claude Code the commands run when you type them; in dsh the model calls the tools itself, and `hippo_ask` searches every indexed session, across projects. In OpenCode the start block is in the system prompt of every request of the session, and the model calls the tools itself too. Set either plugin's `tools: false` to keep only the start block.

The text sent to a model is redacted index text. A note's body is the exception: it is sent as written. Read the [model command](configuration.md#the-default-model-command) you run. The rest of this page assumes the default.

## Redaction on ingest

Before any transcript text is indexed, hippo replaces matches of these patterns with `[redacted:<kind>]`. The whole match is replaced, including a key or header name in front of a secret.

| Kind | What it matches |
|---|---|
| `pem` | PEM private-key blocks |
| `aws` | AWS access key ids (`AKIA` followed by 16 characters) |
| `github` | GitHub tokens (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`) |
| `provider` | Anthropic, OpenAI, Stripe, Slack and Google API key prefixes |
| `jwt` | JSON Web Tokens |
| `auth` | `Authorization:` headers: the word and the two words after it |
| `kv` | A value of 16 or more characters after a key whose name contains `secret`, `token`, `password`, `passwd`, `api_key` or `apikey`, including quoted JSON keys and values |
| `email` | Email addresses. A package specifier such as `@scope/pkg@1.2.3` or `react@18.3.1` is not an email |
| `aadhaar` | 12-digit Indian Aadhaar numbers. Digits inside a UUID are never matched |
| `phone` | Indian mobile numbers, with or without `+91`. Digits inside a UUID are never matched |

Reasoning blocks of the model are never indexed. Tool calls and tool results are indexed, as text, after redaction. Every adapter ships with a test that plants a fake secret of each kind in a transcript and checks that none reaches the index.

Redaction is best-effort pattern matching. It will miss secrets that do not look like these patterns, and it will redact some harmless text: the `aadhaar` and `phone` patterns can still match some numeric ids. Text already in an index keeps the redaction it had when it was indexed. Do not rely on it as your only protection. The index is local and is never uploaded by hippo.

A derived Codex, dsh or OpenCode handoff is redacted in the same way, because it is transcript text written into a record you may commit. So is the handoff the Codex `session-end` hook takes from a Codex transcript.

The Codex hooks put the start block and the relevant notes into the Codex session, which Codex sends to the model provider it is set up with, as it does with every file the agent reads. They give the same text a Claude Code session gets from its hooks, and nothing from other projects.

## What reaches a model, and how it is isolated

The commands in list A above run a model. A dreaming prompt contains:

- the text of the notes the pass works on;
- session records (ids, times, notes touched, handoffs);
- transcript turns found in the index, already redacted;
- text from reverted changes that a human confirmed.

The prompt marks each of these as untrusted data with `<<<UNTRUSTED name>>>` and `<<<END name>>>` lines, and the shipped prompts and personas tell the model to ignore instructions inside. That reduces prompt injection through transcript text; it does not make it impossible.

The reviewer receives only its persona (as the system prompt), the note, the proposed change and the evidence turns. It does not receive the proposer's reasoning or stated reason.

Each model run:

- starts in `$HIPPO_HOME/model-runs/`, an empty directory hippo creates and empties first;
- with the default command, runs `claude` with no tools, no settings files, no MCP servers and no saved session, so the model can read nothing on your disk and leaves no transcript. A canary test in the repository checks that this command cannot read a planted file in its working directory;
- is stopped after 600 seconds;
- receives transcript text on standard input, not in its arguments, so it does not show in the process list.

hippo does not clear the environment of the model command; it inherits the environment of whoever ran hippo. If you replace the default command, the isolation above is your command's job.

## What limits a write

Dreaming can only change a note through one code path, and that path needs a receipt from a reviewer's confirmation:

- Every proposal is checked before review: the note must be a visible `.md` file inside `notes_dir` (symbolic links are resolved and must still lead there), the evidence must exist in the index, and a `replace` must quote evidence word for word and may not exceed half the note.
- A write happens only if the note is byte-identical to what went into the prompt.
- Before the write, the note is copied to `.hippo/snapshots/`. `hippo revert` restores it.
- `hippo revert` refuses a receipt that is not an applied confirmation, a snapshot outside `.hippo/snapshots/`, or a snapshot whose hash does not match.

A confirmed change is still a model's work, reviewed by a model. Read the reports.

Pass and persona files are text, and hippo never executes anything a pack contains. The loader refuses symbolic links, files over 64 KiB and duplicate pass names, so a pack cannot point the model at a file elsewhere on your disk.

## What is written where

| Where | What |
|---|---|
| `$HIPPO_HOME/index.sqlite` | Redacted transcript turns. Rebuildable. Never committed. |
| `$HIPPO_HOME/` other files | `config.toml`, `ingest.lock`, `ingest-state.json`, `logs/`, `model-runs/` |
| `<workspace>/.hippo/` | Settings, the core pack, session records, proposals, receipts, reports, dream state; `snapshots/` is gitignored |
| `<notes_dir>/` | Your notes, changed only by confirmed dreaming changes or by you |
| `log_path` | The generated session log |
| `~/Library/LaunchAgents/` | The two job files, on macOS, if you run `hippo schedule install`. Elsewhere it writes nothing |

Session records, proposals and reports can hold note text, handoffs and quoted transcript text. If you commit `.hippo/` to a shared repository, treat it as you would the notes.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on the hippo repository (the Security tab, then "Report a vulnerability"). Do not open a public issue for a security problem. You get an acknowledgement within three days. Confirmed issues are fixed before public discussion, and reporters are credited unless they prefer not to be.

In scope: anything that gets secrets or personal data past redaction into the index, anything that lets dreaming write without a recorded review, and anything that lets a pass read or write outside its declared permissions. Vulnerabilities in the agents themselves, such as Claude Code or Codex, belong to their vendors.

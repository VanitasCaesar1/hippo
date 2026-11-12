# hippo

hippo is a memory engine for AI coding agents. It is written in Rust, licensed under MIT, and runs on your machine. It reads the transcripts your agents leave behind, keeps your notes honest against them, and gives each new session a small block of context to start from.

## What it does

hippo works in four layers, after the model in Anthropic's article *The components of agent memory*:

```text
transcripts  ->  notes  ->  wiki  ->  injected context
       ^            ^         ^           ^
       +--------- dreaming (offline, verified) ---+
```

- **Transcripts.** `hippo ingest` indexes the transcripts of Claude Code, Codex and DeepSeek Harness (dsh) into one local search index. `hippo recall` searches every turn. Secrets are redacted before anything enters the index.
- **Notes.** Plain Markdown files with a little frontmatter. They live in your workspace, and you keep editing them as you do now.
- **Wiki.** The links between notes (`[[Note name]]`). hippo uses them to find the notes a session touched and to count broken links.
- **Injected context.** `hippo context` compiles a block of text for the start of a session, under a hard size limit. Claude Code hooks can print it into every session.
- **Dreaming.** `hippo dream` is a run you schedule, usually at night. It asks a model to check your notes against what the sessions actually did and to propose changes. A second model run, a **fresh-context reviewer** that has seen none of the first run's reasoning, must confirm each change before hippo writes it. Every change gets a receipt and a snapshot, and `hippo revert` undoes it.

The reviewer idea comes from the Elephant-Goldfish Model: the run that proposes a change never grades its own work.

## Who it is for

You run one or more AI coding agents against one workspace, and you keep project knowledge in Markdown notes. You want agents to start with the right notes, to search what earlier sessions found, and to stop trusting notes that later sessions proved wrong.

hippo does not replace your agent's own instruction files. It supplies context. Your instructions still have to tell agents to use it.

## Status

hippo is at version 0.1.0, early, and used and tested on macOS on Apple silicon. Install it with the install script, the Claude Code plugin or Cargo ([Install](install.md)).

What is built today:

- the core library and the command line: ingest, recall, context, sessions, hooks, the session log, doctor and the core set of dreaming passes;
- dreaming with the `verify` and `consolidate` passes, the isolated reviewer, proposals, receipts, snapshots and revert;
- the two macOS launchd jobs (`hippo schedule`), and printed cron and systemd lines elsewhere;
- the overview, `fleet`, `incidents`, `xray` and `card`, which read your agents' day from the index;
- `hippo eval`, which measures context, recall and dreaming on a synthetic question set;
- packs: the pack format, note filters, hippo's own reviewer for passes outside the core pack, `hippo add` with a review, `hippo packs check`, `test`, `list`, `diff`, `update` and `remove`, and 23 first-party pass packs ([Packs](packs.md));
- the registry: entries, a fresh-context review of each listed pack, and the `hippo-registry` tool that checks them and builds an index and a static site on your machine. all 26 first-party packs are listed ([The registry](registry.md));
- three theme packs and the web theme contract they follow ([Themes](themes.md));
- the desktop app, Hippo.app: macOS only and early, with Home, Fleet, Incidents, Memory, Ask and the Inbox ([`app/README.md`](https://github.com/VanitasCaesar1/hippo/blob/main/app/README.md));
- the foundation of the community feed, `hippo-feed`: a server crate, a database schema and deployment files. It is not deployed and nothing is live.

What is not built: a hosted registry or feed, and adapters for tools other than Claude Code, Codex and dsh. Every pack so far is by the project's author; packs by others are welcome. This documentation describes only what the code does today. [Limits and known gaps](limits.md) lists what to watch for.

## How this guide is organised

1. [Install](install.md) and the [Quick start](quick-start.md) take you from nothing to a working setup.
2. [Concepts](concepts.md) explains the pieces. [Commands](commands.md) and [Configuration](configuration.md) are reference.
3. [Packs](packs.md) explains how to add passes to dreaming, and how to write your own. [The registry](registry.md) and [Themes](themes.md) cover the list of reviewed packs and the look of hippo's web pages.
4. The rest covers scheduling, measuring, troubleshooting and the security model.

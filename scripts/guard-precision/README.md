# The guard's precision on a realistic synthetic history

`run.mjs` measures each guard rule's precision and recall on a labelled synthetic history that looks like real agent use, and fails on any false alarm or miss.

```sh
cargo build --release -p hippo
node scripts/guard-precision/run.mjs target/release/hippo [calls] [seed]   # 6000 and 7 by default
```

`history.mjs` writes the history into a new temporary folder, which the runner uses as `HOME` and `HIPPO_HOME`: calls in four agents' hook payload shapes (Claude Code, Codex, Gemini CLI, OpenCode), each labelled with the rules that should fire, the same calls as Claude Code transcripts, and folders on disk for the calls that run in a repository (remote clones made now, folders an agent made with `git init`, test repositories whose origin is a local path). Everything in it is made up here.

What it holds, by weight: tests, git and `gh`, reads, deploys and package installs; the vendors' installers on the built-in allow list (rustup, Homebrew, Bun, nvm, uv, Deno); look-alikes (a download piped into `python3 -m json.tool` or `jq`, install lines quoted in release notes and commit messages); builder agents testing hippo in scratch homes (`export HIPPO_HOME=$(mktemp -d)`, `$E/home/.codex/config.toml`, fixture `.claude/settings.json`); an agent's own helper scripts in folders it made; and what should fire: unknown installers, one keychain item read again and again and others now and then, credential files, clones from a remote and their setup, writes to this home's hook files, and the commands only the person may run.

The runner passes every call through `hippo guard check` and reads the guard's log, then ingests the transcripts and prints `hippo guard test` and `hippo incidents`. `guard test` and the incidents read the redacted index, where ingest's redaction has removed a `Bearer $(…)` header, so they count fewer keychain reads than the check does.

The same author wrote the rules and this history, so a perfect score is a floor on parsing mistakes, not an estimate of noise on a person's own history; `hippo guard test` on that history is the measurement that counts.

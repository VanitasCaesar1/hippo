# Guard: noticing, and sometimes stopping, risky agent actions

Status: accepted; Part A shipped in v0.5.0, Part B (warn-only) in v0.6.0 ([rollout](#rollout)). Revised 12 November 2026. Shared rules (version floor, local accept, hardening gate, release plan) are in [common.md](common.md).

## The problem

Coding agents run shell commands, install packages, clone repositories and fetch web pages on developer machines that hold SSH keys, cloud credentials, deploy tokens and sometimes wallets. On 29 October 2026 attackers took about $6.3 billion from Binance. Binance's incident report, published on 3 November and relayed by the sources below, describes this chain:

1. **June.** A wallet DevOps engineer (A) was looking for an AI coding tool through a search engine and installed a tampered Homebrew. Binance detected an anomaly, replaced the device and rotated credentials. Palo Alto Networks says a poisoned search ad led to a spoofed site that had the person paste one `zsh` command, which installed a credential stealer. That delivery detail is Palo Alto's account; Binance's report, as quoted, does not give it.
2. **September.** A second engineer (B), approached by a fake recruiter, ran a "coding assignment" repository on a work computer. According to BleepingComputer's quotes from the report, it ran during setup, replaced a `pyenv` file in the Homebrew directory, and opened a shell that took commands from the attacker's server. Endpoint security did not catch it.
3. **20 October.** From B's machine, the attacker used an **internal AI tool** to take over A's machine and a third wallet engineer's machine, and found a container deployment token on A's machine. Binance has not named the tool or said what permissions it had.
4. **29 October.** The attacker used the token to deploy a container that reached the wallet signing service.

Sources: The Block, "Binance says hackers used fake job interview, internal AI tool in $6.3 billion breach", 3 Nov 2026, <https://www.theblock.co/news/business/2026-11-03-binance-hack-incident-report-north-korea-fake-job-interview-ai-tool-432406>; CNBC, "Binance co-CEO Richard Teng says hackers used fake job interviews…", 3 Nov 2026, <https://www.cnbc.com/2026/11/03/binance-co-ceo-teng-says-hackers-used-fake-job-interviews-to-steal-6point3-billion-in-crypto.html>; BleepingComputer, "Binance says fake job interview led to $6.3 billion crypto heist", 3 Nov 2026, <https://www.bleepingcomputer.com/news/security/binance-says-fake-job-interview-led-to-63-billion-crypto-heist/>; StepSecurity, "Binance's $6.34 Billion Hack: Developer Machines, AI Tooling, and Wallet Signing", 30 Oct, updated 3 Nov 2026, <https://www.stepsecurity.io/blog/binance-hack-developer-machine-security>; Palo Alto Networks, "How a Developer Endpoint Became the Path to Binance's $6.3B Heist", 9 Nov 2026, <https://www.paloaltonetworks.com/blog/?p=372557>; SecurityWeek, "Rigged coding assignment abused AI tool opened path to $6 billion Binance heist", <https://securityweek.com/rigged-coding-assignment-abused-ai-tool-opened-path-to-6-billion-binance-heist> (only its summary was read). Binance's own post, <https://www.binance.com/en/blog/community/3229153443759566763>, could not be fetched when this was written; its wording here comes through the sources above.

**None of these steps was an agent's own mistake.** People ran the first two, and the attacker drove the AI tool in the third from a machine they already controlled. That limits what any tool reading agent transcripts could have done here, and this document says so step by step ([below](#what-hippo-could-and-could-not-have-done-in-this-case)).

The incident still shows three things hippo is positioned to help with:

- Agents now run the same kinds of commands people were tricked into running: a one-line installer from a web page, setup scripts of a repository just cloned, reads of credential files. An agent can be talked into them by text in a page, an issue or a README (prompt injection), or can choose them because they look like the quickest fix.
- An agent on a workstation is a tool with execution rights. Whoever controls the machine, or the text the agent reads, can use it.
- hippo already reads every session of every agent on the machine, and already has hooks or plugins inside three of them. It sees much of what agents do. What it lacks is rules about these risks.

### What hippo has today

- **Incidents** (`hippo incidents`) built from transcripts with rules and no model call: corrections, blocked and failed steps, redone work, collisions between sessions, and destructive commands (`rm -rf` outside build folders, `git reset --hard`, force pushes, `DROP TABLE`, …). Each is shown with evidence a person can check in ten seconds.
- For a destructive pattern, `incidents` **prints** a Claude Code `PreToolUse` hook script and the settings entry that would have blocked it, for the person to copy. hippo never installs it.
- The hooks hippo installs are `SessionStart`, `UserPromptSubmit` and `SessionEnd` in Claude Code, and its plugin installs only `SessionStart`. The dsh and OpenCode plugins add the start block and two tools. Codex gets `SessionStart`, `UserPromptSubmit` and `SessionEnd` hooks in 0.4.0 ([codex-hooks.md](codex-hooks.md)). **None of them sees a tool call before it runs.** A guard is new hook work, not a rule added to existing hooks.

## Goals

- **After the fact:** new incident kinds, found in the transcripts hippo already reads, with no model call, for each agent hippo reads.
- **Before the fact:** an optional, machine-wide guard in each agent that has a pre-tool hook, which warns, asks the person, or blocks a risky call, wherever the session runs.
- **Rules a team keeps** as a reviewed note in the repository, layered over the person's own machine rules, where loosening any rule needs a local accept on each machine.
- **Measured false positives** before a rule is switched on, on the person's own history, locally.
- A clear statement of the limits, and of what hippo must never do since it is itself a target.

## Non-goals

- Malware detection, endpoint protection, network filtering or sandboxing. hippo looks at the text of tool calls; it does not watch processes, files or sockets.
- Guarding what a person types in their own terminal. hippo sees agents, not shells.
- Being a security boundary. A guard inside the agent's own process tree can be removed by anything that controls the machine.
- Calling a model to judge a command. The guard has to answer in milliseconds, give the same answer every time, and stay out of reach of prompt injection.

## Design

### Part A: after the fact, from transcripts

New signal kinds, computed when the rollup is refreshed, from redacted index text, in the style of today's destructive-command signal. The shell parser that `incidents` and the x-ray already use (`xray/shell.rs`: simple commands, `cd`, variable assignments, pipelines) is extended where needed. Each rule prefers precision to recall: a missed case is acceptable, a noisy view gets ignored.

| Kind | Fires on | Evidence shown | Ships |
|---|---|---|---|
| `remote-script` | A shell call that pipes a download into an interpreter: `curl …\| sh`, `wget -O- … \| bash`, `bash <(curl …)`, `sh -c "$(curl …)"`, `python - < <(curl …)`, `iex (irm …)`; also `brew tap <user/repo>`, `brew install <url>` and `npm i -g` / `pip install` from a URL or git address. Hosts on the [allow list](#allow-lists-and-layers) do not fire. | The command and its host | v0.5.0 |
| `fresh-clone-exec` | In one session, a clone or download of a repository (`git clone`, `gh repo clone`, a `.zip`/`.tar` fetch and unpack), then a command run inside it that executes the repository's code: `npm/pnpm/yarn/bun install` (lifecycle scripts), `pip install .`/`-e .`, `python setup.py`, `make`, `./configure`, a `*.sh` in it, `cargo build`/`run` (build scripts), `go generate`, `docker build`. An origin on the allow list does not fire. | The clone, the command, the time between them | v0.5.0 |
| `credential-read` | A read of a credential path by a read tool or a shell command (`cat`, `cp`, `tar`, `base64`, `scp`, `grep`, …): `~/.ssh/` (not `*.pub` or `known_hosts`), `~/.aws/credentials`, `~/.config/gcloud/`, `~/.azure/`, `~/.kube/config`, `~/.docker/config.json`, `~/.netrc`, `~/.npmrc`, `~/.pypirc`, `~/.git-credentials`, `~/Library/Keychains/`, `security find-*-password -w`, browser profile folders, and known wallet locations (Electrum, Exodus, Ledger Live, `keystore/UTC--*`, wallet browser-extension folders). | The tool and path, never the result | v0.5.0 |
| `guard-bypass` | A write by an agent's tool to a file that turns hooks off or changes hippo's state: `.claude/settings*.json`, `<account>/settings.json`, `~/.codex/hooks.json`, `config.toml` hook entries, `~/.cursor/hooks.json`, `~/.gemini/settings.json`, the dsh profile patch, `opencode.json` plugins, the guard note, `$HIPPO_HOME/config.toml`, `$HIPPO_HOME/bin-path`, anything under `$HIPPO_HOME/guard/` or `$HIPPO_HOME/accepted/`; also `hippo hooks uninstall` and `hippo accept` run by an agent. `hippo hooks install` and `hippo mcp install` run by an agent do not fire: hippo writing its own entry is not a bypass. | The edit or command | v0.5.0 |
| `unknown-egress` | A shell call that **sends** data to a host not on the allow list: `curl`/`wget` with `-d @file`, `--data-binary`, `-F`, `-T`, `--upload-file` or a body from command substitution; `scp`/`rsync`/`sftp` to a remote; `nc host port < file`. Reading from an unknown host is not counted. | The command and host | later |
| `command-from-fetched` | A shell command at least 20 characters long that occurs word for word in a web fetch result, a file read from outside the workspace, or an issue or PR body fetched with `gh` earlier in the same session, and that also matches one of the kinds above. | The fetch and the command, side by side | later |
| `secret-to-model` | Redaction replaced a secret-kind match in a turn the model saw. | The kind and the turn, never the value. Designed in [secrets.md](secrets.md#detection-secret-to-model) | v0.5.0 |

`command-from-fetched` is the narrow, checkable form of "the agent followed instructions it found in fetched content": a risky command copied character for character from text the agent had just read from outside the workspace. The evidence is two excerpts a person can compare.

All kinds appear in `hippo incidents`, Fleet's incident counts, the x-ray of the session and Hippo.app's Incidents panel, per agent (Claude Code, Codex, Cursor, Gemini CLI, dsh, OpenCode, Muse and Command Code: every adapter hippo has). They need no hook. Like every incident, they work only for agents whose transcripts hippo reads, and only on what the transcript records: a command's text, not what it did.

In v0.5.0 Part A uses hippo's built-in allow list only (`ALLOWED_HOSTS` in `crates/hippo-core/src/incidents/guard.rs`: the installers of Homebrew, rustup, uv, Bun, Deno, nvm, Claude Code and hippo itself; `ALLOWED_ORIGINS`: hippo's own repository).

**What v0.5.0 built, where it narrows the table above.** Each narrowing trades recall for precision, as this section asks:

- `remote-script`: a pipe into an interpreter counts only when the interpreter reads its script from standard input (no `-c`, `-e`, `-m`, `-E`, `-n`, `-p` or script file after it), so `curl … | python3 -m json.tool` and `curl … | node -e …` are data, not code. A pipeline in a comment or an `echo` does not count. `npm`, `pnpm`, `yarn` and `bun` count only for a global install from a URL or git address.
- `fresh-clone-exec`: the clone's folder is followed through the script's `cd`s and, for a shell that keeps its folder between calls (Claude Code's), from call to call; a command counts only when it runs in that folder or names a path in it. An install with package arguments (`npm install left-pad`) or `--ignore-scripts` does not count; neither does a `.zip`/`.tar` download (not built in v0.5.0).
- `credential-read`: `~/.ssh/config` and `authorized_keys` do not count beside `*.pub` and `known_hosts`, since neither holds a secret. `ls`, `chmod`, `stat` and the like are not reads. The GitHub CLI's `~/.config/gh/hosts.yml` is added.
- `guard-bypass`: a write to a settings file that holds much else (`.claude/settings*.json`, Codex's `config.toml`, `~/.gemini/settings.json`, the dsh profile, `opencode.json`) counts only when the written text mentions hooks or hippo, or the file is removed or moved; hook-only files (`~/.codex/hooks.json`, `~/.cursor/hooks.json`) and hippo's state count on any write. `hippo mcp install` run by an agent does not count, as `hooks install` does not.
- Every adapter's tool names are read: shell calls named `Bash`, `shell`, `exec_command`, `run_shell_command`, `Shell` and the like, and file tools by name with the path keys each agent uses (`file_path`, `path`, `absolute_path`, …).

**Measured.** On `fixtures/guard/commands.tsv`, a labeled synthetic set of 95 cases (36 that should fire, 59 that look alike but should not), written before the first run: precision and recall 1.00 for each kind (9, 10, 10 and 7 true alarms). The same author wrote the rules and the set, so this is a floor on parsing mistakes, not an estimate of noise on real history. On a scratch copy of the synthetic demo history (`app/scripts/demo-home.sh`, which holds the eval set: 68 sessions, 899 tool calls), no kind fired: no false alarms, and nothing to find. All four ship as incidents; the first measurement on real history, with `guard test` in v0.6.0, decides whether any becomes a hint. Machine and team allow lists arrive with Part B in v0.6.0, and from then on Part A uses the same [layers](#allow-lists-and-layers) as the guard: the machine's list, plus the list of the workspace that contains the session's working directory, if any.

### Part B: before the fact, a guard in the agent

`hippo guard check` reads one pending tool call as JSON on stdin and prints a decision. Each agent's hook calls it by absolute path ([common.md, rule 4](common.md#rule-4-the-hardening-gate)).

**Machine-wide.** The threat this guard is for includes a repository cloned into `~/Downloads` and set up by an agent there: the coding-assignment case. A hook installed in one workspace never sees that session. So the guard is installed per agent account, not per workspace:

| Agent | Hook point | Can ask the person | Install |
|---|---|---|---|
| Claude Code | `PreToolUse`, matcher `Bash\|Read\|Write\|Edit\|MultiEdit\|WebFetch\|mcp__.*` | yes (`permissionDecision: "ask"`) | `hippo guard install` adds one entry to each Claude account's user settings (`<account>/settings.json`, for every account folder hippo finds, each listed and confirmed with `--yes`). Never a workspace's committed `.claude/settings.json`: that would push the guard, `fail = "closed"` and an absolute path onto every teammate. |
| Codex | `PreToolUse` in `$CODEX_HOME/hooks.json`, beside the hooks of [codex-hooks.md](codex-hooks.md); contract read from Codex's source at `rust-v0.170.0` ([below](#codexs-pretooluse-contract)) and checked end to end in v0.6.1 | no: an `ask` answer is refused as unsupported, so a question becomes a refusal, as in a headless mode | `hippo guard install --agent codex` (v0.6.1) |
| Cursor | `preToolUse` in `~/.cursor/hooks.json`; contract read from the shipped JavaScript of `cursor-agent` 2026.11.19 ([below](#cursors-pretooluse-contract)); no end-to-end run | no from `preToolUse` (it ignores `ask`); `beforeShellExecution`'s `ask` forces Cursor's approval prompt for shell commands | `hippo guard install --agent cursor` (v0.6.1) |
| dsh | `tools/pre-execute`, a waterfall whose listeners return `allow`, `deny` or `ask`, with the parsed arguments (read from dsh's source, `packages/core/tools/src/index.ts`) | yes (`ask`, which becomes a denial where approvals are not supported) | the dsh plugin's `guard: true` option, in the user-level plugin configuration |
| OpenCode 2 | `ctx.tool.hook("execute.before")` sees the tool and input, and an error thrown refuses the call; `execute.after` may replace the result (OpenCode's source at v2.3.2: `packages/plugin/src/promise/tool.ts`, `packages/core/src/tool.ts`; checked end to end in v0.6.1) | not from the plugin: `ctx.permission.hook("evaluate")` could, and is not used | the OpenCode plugin's `guard: true` option, in the user-level configuration |

The plugins call `hippo guard check` as a child process, as they already call `hippo context`. The guard reads only its compiled rules: it does not open the index, the rollup or any note at call time, so a check costs one process start and a few regular expressions (target: under 20 ms; measured in the test plan).

**Decisions.** Each rule has an action:

- `warn`: the call runs. The agent gets a short note in its context (`hippo guard: this sends a file to an unknown host (paste.example). Tell the person.`), and the call is logged.
- `ask`: the person is asked, in the agent's own approval prompt, with the rule's reason.
- `block`: the call is refused, and the reason tells the agent to stop and have the person run the command themselves if they want it.

**When nobody can be asked.** An agent running headless (`claude -p`, `codex exec`, a dsh headless profile) or in a mode that skips approvals (Claude Code's `bypassPermissions`, read from the hook payload's permission mode) cannot show a question. There `ask` becomes `block`, and the reason says so. A person who runs agents that way and wants the call through adds an allow entry.

**Precedence.** For one call, every matching rule from every layer is evaluated, and the strictest action wins: `block` over `ask` over `warn`. A rule never lowers another rule's action. Allow entries are not rules: they only stop the built-in kinds that consult them (`remote-script`'s host, `fresh-clone-exec`'s origin, `unknown-egress`'s host). A `match` rule is never silenced by an allow entry.

**Text in reasons.** A reason the agent sees can quote only a host or path, and only if it matches `[A-Za-z0-9._:/~-]{1,200}`; otherwise it says `(host not shown)`. A host name is attacker-chosen text, and a reason is read in hippo's voice.

**Defaults.** In v0.6.0 every team and kind rule acts as `warn`, whatever its configured action, and the self-protection rules below block. The shipped actions that take effect once `ask` and `block` are switched on (later): `credential-read` of SSH private keys, keychains and wallet files, and `remote-script` from a host not on the allow list, start at `ask`; everything else at `warn`. Nothing blocks by default. A team raises actions in its rules note.

**When the guard fails.** If `hippo` is missing, crashes or takes longer than its timeout (default 2 s), the call proceeds (fail open), and the failure is logged and shown by `doctor`. Failing closed would stop every agent whenever hippo breaks, and people would uninstall it. A team that prefers failing closed sets `fail = "closed"`. Either way, the guard is not a security boundary.

**What v0.6.0 built, where it differs from the text above.**

- **Agents.** Claude Code (`hippo guard install`, every account folder: `$CLAUDE_CONFIG_DIR`, `~/.claude`, `~/.claude-*`), Gemini CLI (`hippo guard install --agent gemini`, `BeforeTool` in `~/.gemini/settings.json`), dsh and OpenCode (the plugins' `guard: true`). Gemini CLI's `BeforeTool` has no question and no way to give the model context: a warning reaches the person as `systemMessage`, a refusal reaches the agent as `decision: "deny"`. dsh's contract was read from its installed type definitions (`dsh-tools`, `tools/pre-execute` returning `allow`, `deny` or `ask`; a warning goes back to the model with the call's result through `tools/post-execute`'s `additionalContexts`). OpenCode's `execute.before` hook was read from the 2.3.2 binary's strings; a refusal is thrown from it. (v0.6.1 checked it against OpenCode's source and a running 2.3.2; see [What v0.6.1 changed](#what-v061-changed).) **Codex** is left out: its `PreToolUse` contract has not been checked (v0.6.1 adds it; see [Codex's PreToolUse contract](#codexs-pretooluse-contract)). **Cursor** is left out: its `preToolUse` answer has no neutral value (an empty answer blocks the call, and `permission: "allow"` may skip Cursor's own approval), so a warn-only guard cannot stay out of the way. (v0.6.1 read cursor-agent's code: neither is so in 2026.11.19, and `{}` is neutral; see [Cursor's preToolUse contract](#cursors-pretooluse-contract).)
- **Warnings.** Claude Code gets `hookSpecificOutput.additionalContext` (the model reads it beside the tool result) and `systemMessage` (the person), with no `permissionDecision`, so Claude Code's own approval flow is unchanged. No answer at all when nothing matched.
- **A fresh clone across calls.** Part A follows a clone through one session's transcript. The guard sees one call at a time, so it also reads the disk: code run (`npm install`, `make`, a `.sh`, …) in a git repository outside the workspace whose `.git` folder was created in the last 24 hours, with an origin that is not allowed, counts as `fresh-clone-exec`.
- **Compiling.** `guard check` reads `compiled.json`, and compiles again only when one of its sources (`config.toml`, `hippo.toml`, the guard note, `machine.toml`, the packs' `Guard.md`, the accepted file, the worktree list) changed size or time. A pack's rules come from `<pack>/Guard.md`. The workspace's worktrees are read from `.git/worktrees/*/gitdir`, without running git.
- **The accepted set.** The set in force is `compiled.json`'s `in_force`, vouched for by its hash in the accepted file. A `compiled.json` whose rules do not match the hash is set aside: the guard starts again from its built-in rules (stricter, never looser) and `doctor` says so.
- **Merging.** A change that tightens one part and loosens another gets the tightening at once: rules in either set stay at the stricter action, and only the allow entries in both apply.
- **`match = '.*'`.** Every kind and `match` rule acts as `warn` in v0.6.0, so a new `block` rule cannot stop calls yet; the 5% check over the last 7 days ships with `ask` and `block`.
- **`fail`.** The guard answers within `timeout_ms` itself: a watchdog gives the fail answer (nothing, or with `fail = "closed"` a refusal) and logs the failure in `$HIPPO_HOME/guard/failures.jsonl`. For a missing or crashing hippo, Claude Code's hook entry carries `onFailure: "block"` when `fail = "closed"` was in force at install (`doctor` says when the entry and the setting differ); Claude Code's own hook timeout is a second past the guard's. The plugins read `fail` from `compiled.json` when hippo does not answer. Gemini CLI has no such setting, so there a missing hippo fails open.
- **Logging.** A matched call is a line in `$HIPPO_HOME/guard/log.jsonl` (redacted command, at most 400 characters); every checked call adds one byte to `$HIPPO_HOME/guard/calls/<day>`.
- **`guard test`** applies every layer to every indexed call, since the index does not keep a session's folder, and counts by the configured action (what each rule will do once `ask` and `block` act).
- **Part A's allow lists** stay the built-in ones in v0.6.0; the machine and team lists apply to the guard only.
- **Measured** on a release build, 1,000 calls of mixed shell and file payloads in a scratch home with one team rule: p50 3.4 ms, p95 6.1 ms, p99 6.4 ms. A test keeps the median of a debug build under 100 ms.

**What v0.6.1 changed.**

- **End to end in the real agents.** `scripts/agent-e2e/` holds a stub model (`stub-model.mjs`, speaking the Anthropic Messages, OpenAI chat completions, OpenAI Responses and Gemini APIs, and refusing every proxied outside host) and `run.sh`, which installs the guard the way a user does in a scratch home for each agent and runs one headless task. Results on 20 November 2026, with the agents' default settings otherwise:

  | Agent (version) | The warned call runs | The model sees the warning | The person sees it (headless output) | Self-protection refuses |
  |---|---|---|---|---|
  | Claude Code 2.1.328 | yes | yes (`additionalContext`, as a system message after the result) | yes (`PreToolUse:Bash says: hippo guard: …`) | yes |
  | Codex 0.170.0 (app-server) | yes | yes (a developer message) | yes (the `hook/completed` warning entry the TUI shows; not in `codex exec`) | yes |
  | Gemini CLI 0.54.4 | yes | no (`BeforeTool` has no context for the model) | yes (`systemMessage`) | yes |
  | OpenCode 2.3.2 | yes | yes (at the top of the call's result) | yes (in the tool's output) | yes |
  | dsh 0.2.2-rc.5 | yes | yes (a reminder after the result) | no: the headless profile prints only the final answer; the warning is in the session | yes |

  Each run connected only to 127.0.0.1 (sampled with `lsof` over the agent's process tree). The outside hosts agents asked the proxy for, all refused: `example.invalid` (the scripted download) for every agent, and `github.com`, `api.github.com` and `chatgpt.com` for Codex.
- **OpenCode.** The run showed v0.6.0's warning reached neither the model nor the person: it went to the plugin's log only. The plugin now keeps the warning by call id and puts it at the top of the call's result in `execute.after`. The hook contract was read from OpenCode's source at v2.3.2 and is no longer marked unverified.
- **Codex.** `hippo guard install --agent codex` adds a `PreToolUse` entry to `$CODEX_HOME/hooks.json`, next to the session hooks of [codex-hooks.md](codex-hooks.md), with the matcher `^(Bash|apply_patch|mcp__.*)$` and a `timeout` a second past the guard's. Like those hooks it is the person's to trust in Codex's review at its next start (or `/hooks`); hippo never writes Codex's trust. The contract is below. End to end (Codex 0.170.0, through `codex app-server`, the interface Codex's TUI uses): the warned calls ran, the model got each warning as a developer message, the `hook/completed` notification carried it as a warning entry (the TUI's `↳ Hook · hippo guard: …`), and `hippo guard allow` was refused with the reason as feedback. `codex exec` prints neither a hook's message nor its reason, only its status. Codex fails open on a missing or crashing hippo whatever `fail` says: a failed hook does not block, and its hook entries have no `onFailure`.
- **Cursor.** `hippo guard install --agent cursor` adds one script to `~/.cursor/hooks.json`'s `preToolUse` list (`version: 1` added when missing, other hooks kept), matcher `^(Shell|Read|Write|Delete|MCP:.*)$`, and `failClosed: true` when `fail = "closed"`. A warning is `{"additional_context": "…"}`: the call runs through Cursor's own approval and the model reads the warning; the person sees nothing at the time. Self-protection is `permission: "deny"` with the reason as `user_message` and `agent_message`. Nothing matched is `{}`. The contract is below. Cursor has no end-to-end run: `cursor-agent` takes its model from Cursor's servers and cannot be pointed at a stub, so the test replays payloads in its shape through `hippo guard check` (`crates/hippo/tests/guard.rs`).
- **cursor-agent runs Claude Code's hooks too.** It loads `~/.claude/settings.json` (and a project's `.claude/settings*.json`), maps `PreToolUse` to `preToolUse` and `Bash` to `Shell`, and reads Claude Code's nested answer. So the Claude Code guard in `~/.claude` already checked cursor-agent's calls in v0.6.0, answering in Claude Code's format. From v0.6.1 a payload with `cursor_version` is answered in Cursor's format and logged as `cursor`, and when `~/.cursor/hooks.json` has hippo's own entry the Claude Code entry answers `{}`, so each call is checked once.
- **A machine where hippo was never set up.** `hippo guard install` needs no index or workspace, but v0.6.0's `guard check` needed `config.toml` and, without it, failed open on every call (logging each failure). It now uses the default configuration: the built-in and machine rules apply.

### Codex's PreToolUse contract

Read from Codex's source at tag `rust-v0.170.0` (commit `29eb79e`, 17 November 2026), the version tested:

- **When it runs.** Before a tool call whose hook name the entry's `matcher` selects (`codex-rs/core/src/tools/registry.rs`, `apply_pre_tool_use`). A matcher of letters, digits, `_` and `|` is a list of exact names; anything else is an unanchored regular expression (`codex-rs/hooks/src/engine/matcher.rs`), hence hippo's `^(…)$`.
- **The tool's name and input.** Every shell tool (`exec_command`, `shell`, …) is `Bash` with `{"command": "<script>"}`; a file edit is `apply_patch` with `{"command": "<the patch>"}`, and also matches `Write` and `Edit`; an MCP tool is `mcp__<server>__<tool>` with its arguments (`codex-rs/core/src/tools/hook_names.rs`, `handlers/unified_exec_tests.rs`, `handlers/apply_patch_tests.rs`, `handlers/mcp.rs`).
- **The input.** `session_id`, `turn_id`, `transcript_path`, `cwd`, `hook_event_name`, `model`, `permission_mode` (`bypassPermissions` when approvals are `never`, else `default`), `tool_name`, `tool_input`, `tool_use_id`, and for a sub-agent `agent_id` and `agent_type` (`codex-rs/hooks/src/events/pre_tool_use.rs`, `command_input`; `codex-rs/core/src/hook_runtime.rs`, `hook_permission_mode`).
- **The answer** (`codex-rs/hooks/src/engine/output_parser.rs`, `parse_pre_tool_use`; `events/pre_tool_use.rs`, `parse_completed`). Exit 0 with no output: nothing. `hookSpecificOutput.additionalContext` becomes a developer message the model reads. `systemMessage` becomes a warning entry, which the TUI shows and `codex exec` does not. `permissionDecision: "deny"` with a non-empty `permissionDecisionReason` (or the older `decision: "block"` with `reason`, or exit 2 with the reason on stderr) refuses the call, and the reason reaches the model. `permissionDecision: "ask"`, `continue: false`, `stopReason` and `suppressOutput` are unsupported: the hook counts as failed and the call goes on. `allow` is accepted only with an `updatedInput`.
- **Failure.** A hook that times out, crashes or exits non-zero is a failed run, and the call goes on.

So hippo answers Codex as it answers Claude Code, except that nobody can be asked: `guard check --agent codex` treats every Codex call as headless, so an `ask` (once `ask` takes effect) is a `deny` with the reason saying nobody could be asked.

### Cursor's preToolUse contract

Read from the JavaScript `cursor-agent` 2026.11.19 (`2026.11.19-f005e9d`, the darwin-arm64 package from Cursor's installer) ships, since Cursor's hook code is not published as source. The hooks code is bundled in `8755.index.js`; the shell approval in `index.js`. Cursor's IDE was not read, and may differ.

- **Files and shape.** `~/.cursor/hooks.json` (user), `<project>/.cursor/hooks.json`, an enterprise file, team hooks, and Claude Code's `~/.claude/settings.json`, `.claude/settings.json` and `.claude/settings.local.json`. `{"version": 1, "hooks": {"preToolUse": [{"command", "matcher"?, "timeout"? (seconds), "failClosed"?, "loop_limit"?}]}}`; a matcher is an unanchored regular expression over the tool's name.
- **The input.** `conversation_id`, `generation_id`, `model`, `session_id`, `hook_event_name`, `cursor_version`, `workspace_roots`, `user_email`, `transcript_path`, `tool_name`, `tool_input`, `tool_use_id`. Tool names are `Shell` (`{command, cwd}`), `Read`, `Write` (`{file_path, content}`), `Delete`, `Ls`, `Grep`, `ReadLints`, and `MCP:<tool>`. There is no top-level `cwd`.
- **The answer.** `permission` (`allow`, `deny`, `ask` or absent), `user_message`, `agent_message`, `updated_input`, `additional_context` (also Claude Code's nested `hookSpecificOutput`, which the agent's hook runner turns on). Several hooks' answers merge with `deny` over `ask` over `allow`, and their contexts are joined.
- **What each does before a tool runs** (`preToolUse`). `deny` refuses the call, with `user_message` as the reason. Anything else lets it go on: `allow` and `ask` are not acted on here, `updated_input` replaces the input, and `additional_context` is given to the model. So an answer without `permission` leaves Cursor's own approval of the call exactly as it was.
- **No answer.** Empty output is a failed hook in Cursor's log, and the call goes on; output that is not JSON, on a step that decides, blocks "for safety". A hook that fails or times out lets the call go on, unless it has `failClosed: true`.
- **`beforeShellExecution`** (shell commands only) is the step that can ask: `ask` turns off auto-approval for that command and shows Cursor's prompt with "Hook requested approval: <user_message>"; `deny` refuses. With approvals skipped (`--force`) the prompt is skipped too.

What each way of warning costs, against [Decisions](#part-b-before-the-fact-a-guard-in-the-agent) (`warn`: the call runs, the agent is told, the call is logged):

| Answer for a warning | The call | The model | The person | Cursor's approval |
|---|---|---|---|---|
| `preToolUse` `{"additional_context": …}` (chosen) | runs | told | not told at the time | unchanged |
| `preToolUse` `permission: "ask"` | runs (ignored) | not told | not told | unchanged |
| `beforeShellExecution` `permission: "ask"` | waits for the person; with `--force`, runs | not told | asked, with the reason | replaced by a prompt, even for allow-listed commands |
| `preToolUse` `permission: "allow"` | runs | told, with `additional_context` | not told | unchanged in 2026.11.19 (not acted on), but it says "allow" for a call hippo has not approved |

The first is `warn` exactly. The third is `ask`, not `warn`: it is what hippo will use for Cursor's shell commands once `ask` takes effect. Until then, a question in Cursor becomes a refusal, as where nobody can be asked.

### Self-protection

Built-in rules that no note, pack or machine rule can lower, and that block from the first release of the guard:

- an agent's call to `hippo accept`, `hippo guard accept`, `hippo guard allow`, `hippo guard uninstall`, `hippo hooks uninstall`, `hippo lessons share`, or `hippo config set` of a key in [the loosening table](common.md#which-direction-is-loosening) or `share_lessons`;
- an agent's write, by a file tool or by a shell redirect or `cp`/`mv`/`tee`/`sed -i` the parser recognises, under `$HIPPO_HOME/guard/`, `$HIPPO_HOME/accepted/` or to `$HIPPO_HOME/config.toml`.

Their false positives are zero by design: an agent never needs to accept a loosening for the person. They make the injected-by-text case fail visibly. They do not stop an agent that writes a script to a file and runs it, or edits these files through a language runtime; such attempts show up as `guard-bypass` incidents where the transcript records them. This is the limit [common.md](common.md#rule-3-tightening-takes-effect-at-once-loosening-needs-a-local-accept-on-each-machine) states.

### Rules as a note

The team's rules live in one note the team commits, by default `<notes_dir>/Guard.md`, readable by people and agents like any other note:

````markdown
---
type: policy
---
# Guard

Agents must not run installers piped from the web, or read credential files.
Ask the person instead.

```hippo-guard
[allow]
hosts = ["raw.githubusercontent.com/Homebrew/", "sh.rustup.rs", "github.com/our-org/"]
clone_origins = ["github.com/our-org/"]

[[rule]]
kind = "remote-script"
action = "block"

[[rule]]
kind = "unknown-egress"
action = "ask"

[[rule]]
name = "prod deploys need a tag"
match = 'kubectl .*--context[= ]prod'
action = "block"
reason = "Production deploys go through CI with a release tag."
```
````

Only fenced `hippo-guard` blocks are read, so the rest of the note stays prose. Built-in kinds are defined in hippo's code. A note can change their actions, add allow entries, and add `match` rules (a regular expression over the command, or a path glob for file tools).

### Allow lists and layers

Rules come from four sources, compiled into one rule set in `$HIPPO_HOME/guard/compiled.json`:

1. **Built-ins** in hippo's code, including the self-protection rules and the built-in allow list.
2. **Machine rules** in `$HIPPO_HOME/guard/machine.toml`, the person's own, in the same format as a `hippo-guard` block. They apply to every session on the machine.
3. **The workspace's note**, applied only to a session whose working directory is inside the configured workspace or one of its worktrees. A session elsewhere gets layers 1, 2 and 4.
4. **Packs' rules**, which can only add rules and raise actions, never add allow entries, as a pack's reviewer persona can only add restrictions.

### Changing rules: tighter at once, looser after a local accept

The guard follows [common.md, rule 3](common.md#rule-3-tightening-takes-effect-at-once-loosening-needs-a-local-accept-on-each-machine). When any source changes, hippo compiles the new rule set and compares it with the last accepted one, **whatever source the change came from**: a note edit, a pack update, a hand edit of `machine.toml`, or a new `guard_note` path all go through the same comparison.

- Stricter (a rule added, an action raised, an allow entry removed): takes effect at once.
- Looser (an allow entry added, an action lowered, a rule removed, any edit to a `match` rule, a change of `guard_note` or `timeout_ms`, `fail` from `closed` to `open`): waits. The accepted set stays in force, and `doctor` says that loosening is waiting.

**One exception to "stricter at once".** A new `match` rule with `ask` or `block` could stop every call (`match = '.*'`): stricter, and a denial of service. When it compiles, hippo runs it over the last 7 days of the person's own history (as `guard test` does). If it would have fired on more than 5% of the calls, it runs as `warn` until accepted, and `doctor` says why.

Dreaming cannot write the guard note at all: its proposal validator refuses that path, as it refuses paths outside `notes_dir`.

The `[guard]` table and `guard_note` raise `min_hippo_version` ([common.md, rule 2](common.md#rule-2-security-settings-in-the-shared-hippotoml-need-a-version-floor)), so a teammate's older hippo does not silently ignore `fail = "closed"`.

### False positives

- **Before turning a rule on:** `hippo guard test [--days 30]` runs the compiled rules over the person's own indexed history and prints, per rule, how many calls it would have warned on, asked about or blocked, with examples. It is local and makes no model call. This measurement decides a rule's default action.
- **When it fires:** the reason names the rule and the exact command to allow it (`hippo guard allow host paste.example`), for the person to run. The command prints the change to the note or to `machine.toml`; a note change goes through the team's normal review in git, and both need a local accept.
- **Logging.** Every call that matches a rule (any action) is logged in `$HIPPO_HOME/guard/log.jsonl` (0600): rule, agent, session, decision, and the command after redaction. Calls that match nothing are counted per day, not logged. `hippo incidents` lists a rule the person approved every time it asked over the last 14 days as "always allowed: consider an allow entry".

### What hippo could and could not have done in this case

| Step | Could hippo have seen it? | Could the guard have stopped it? |
|---|---|---|
| June: a person pastes a `zsh` one-liner from a spoofed site | Only if an agent ran it. hippo does not see a person's terminal. | No, for the same reason. If an agent had been asked to "install Homebrew" and found the spoofed page, `remote-script` from an unknown host would have asked or blocked. |
| September: the assignment repo runs malware during setup | If an agent ran the setup: `fresh-clone-exec`, wherever the clone was, because the guard is machine-wide. If a person ran it: no. | Only if an agent ran it, the origin was not on the allow list, and the person read the question and said no. hippo cannot tell a malicious install script from a normal one; it asks about every unfamiliar one, which is why its default is `warn`. |
| 20 October: the attacker drives an internal AI tool to reach other machines | Only if that tool were an agent hippo reads, and only after the fact: `credential-read`, `unknown-egress`, perhaps `remote-script` on the target machines, if those transcripts survived. | No. The attacker controlled B's machine and could remove or skip any hook there. |
| 29 October: a stolen deploy token is used | Not unless an agent used it. | No. |

What hippo can do is make the agent paths to these mistakes visible and add a question on the most common ones. It does not protect a workstation from a person who runs malware, or from an attacker who already has code execution.

### hippo as a target

hippo reads every transcript on the machine and runs code inside every agent session it hooks. That makes it worth attacking:

- `index.sqlite` holds redacted text of every session in every project: internal host names, service names, file layouts, people's corrections. Redaction removes known secret patterns, not that map.
- The hooks run `hippo` on every session start and, with the guard, on every tool call, with the call's full input on stdin. A fake `hippo` earlier on `PATH` would see all of that. The Binance malware replaced a file in the Homebrew directory, which is how such a fake would get there.
- `hippo add` and the plugins fetch from git.

| Must never | Why |
|---|---|
| Hold secret values | It already holds the map; adding the keys would make it the one file to steal. See [secrets.md](secrets.md). |
| Run code from packs, templates or rules | Guard rules are data compiled by hippo's own code; `match` rules are regular expressions, never commands. |
| Update itself, or anything, unsigned | No self-update exists. If one is added, it verifies a signature first. |
| Send anything over the network itself | No HTTP client, as today. |
| Install a guard silently | `hippo guard install` lists every file it will change and writes only with `--yes`. |

| Must do | When |
|---|---|
| `$HIPPO_HOME` 0700, files 0600 | v0.5.0, [common.md, rule 4](common.md#rule-4-the-hardening-gate) |
| Hooks call hippo by absolute path, from per-person files; plugins read `$HIPPO_HOME/bin-path` | v0.5.0, same |
| `install.sh` verifies the checksum before it writes; `doctor` checks the hooks' binary against it | v0.5.0, same |
| Plugin installs pinned to a tag, never a branch | Done in the documented install lines. |
| Signed and notarised macOS builds (Developer ID) for the CLI and Hippo.app; a signature over the checksum file | Later. The app is signed ad hoc today. A gate for the secrets helper. |

## Data, privacy and security

- The guard reads each pending call's full input, which can contain secrets. It keeps nothing but the log line, redacted before it is written, in a 0600 file. The compiled rules hold no transcript text.
- The guard decision goes to the agent, and so to the agent's model provider, as any tool result does. It names the rule and a filtered host or path, not file contents.
- The new incident kinds read redacted index text, as incidents do today. `credential-read` records that a path was read, never what was in the result.
- Rules in a shared note are visible to anyone with the repository, including the allow list. An allow list tells an attacker which hosts are trusted. That is the same trade-off as any committed allow list.

## What changes in config, CLI and app

- v0.5.0: `hippo incidents` gains `remote-script`, `fresh-clone-exec`, `credential-read`, `guard-bypass` (and `secret-to-model` from [secrets.md](secrets.md)); `--kind <k>` filters.
- v0.6.0: `hippo guard check` (for hooks), `guard install [--agent claude|gemini]` (`codex` in v0.6.1), `guard uninstall`, `guard test`, `guard allow`, `guard status`; `hippo accept` ([common.md](common.md#rule-3-tightening-takes-effect-at-once-loosening-needs-a-local-accept-on-each-machine)); the dsh and OpenCode plugins gain `guard: true`; `.hippo/hippo.toml` gains `guard_note` and `[guard] fail`, `timeout_ms`; `$HIPPO_HOME/guard/machine.toml`; `hippo doctor` gains a `guard` row (installed per agent account, rules compiled, loosening waiting, guard failures in the last day, the hook's binary path and checksum); Hippo.app shows the guard's state per agent in Manage and the accept dialog.

## Test plan

- **Signals:** each rule with positive and negative fixtures, including evasions it is not expected to catch (a script written to a file and then run; base64), recorded as known misses so a change that starts catching them is visible. `hippo hooks install` run by an agent does not fire `guard-bypass`; `hooks uninstall` does.
- **`guard check`:** JSON in, decision out, for each agent's input shape, with fixtures copied from each agent's documented hook payload. Latency over 1,000 calls on CI hardware; fail if the median exceeds 20 ms.
- **Precedence:** a `warn` rule and an `ask` rule on one call give `ask`; an allow entry does not silence a `match` rule.
- **Headless:** `ask` in a payload with a bypass permission mode becomes `block`.
- **Loosening:** an allow entry added to the note, to `machine.toml` or by a pack has no effect until `hippo accept`; a changed `guard_note` path, a lowered `fail`, a changed `timeout_ms` and an edited `match` rule wait too; a new rule takes effect at once; a new `match = '.*'` block runs as `warn`; `accept` refuses with an agent marker set or without a terminal; dreaming's validator refuses the guard note.
- **Self-protection:** an agent's `hippo guard accept`, `hippo config set share_lessons …` and a `Write` into `$HIPPO_HOME/guard/` are blocked in warn-only mode.
- **Layers:** a session in `~/Downloads/x` gets machine rules and not the workspace note; a session inside a worktree of the workspace gets both.
- **Install:** `guard install` writes each account's user settings and never a workspace `.claude/settings.json`.
- **Reasons:** a host with control characters or Markdown is shown as `(host not shown)`.
- **Fail open and closed:** a guard that sleeps past its timeout; a missing binary.
- **End to end, before a release that changes a hook:** `scripts/agent-e2e/run.sh` runs each real agent CLI in a scratch `HOME` against a stub model on 127.0.0.1, which asks for a download piped into `sh`, a read of a synthetic SSH key, an `ls` and `hippo guard allow`; it checks the guard's log, what the model and the person saw, that the refused call did not run, and that the agent connected to nothing but 127.0.0.1 ([What v0.6.1 changed](#what-v061-changed)).
- **`guard test` on synthetic history** with known counts per rule.

## Rollout

Part of the joint plan in [common.md](common.md#release-plan). Every step needs the v0.5.0 hardening gate first.

1. **v0.5.0, Part A:** `remote-script`, `fresh-clone-exec`, `credential-read`, `guard-bypass` for every adapter, with the built-in allow list. Nothing installed, nothing blocked.
2. **v0.6.0, Part B warn-only, machine-wide:** `guard check`, `guard test`, machine rules, the rules note and the accept path; the self-protection rules; `guard install` for every Claude Code account; then the dsh and OpenCode plugin options; Codex if its hook contract has been checked by then, otherwise in a later release.
3. **Later:** `ask` and `block` take effect, once `guard test` counts on real history show which defaults are quiet enough.
4. **Later:** `unknown-egress` and `command-from-fetched`: they need the most tuning.

## Alternatives

- **Install the printed destructive-command hooks automatically.** Each is a separate shell script per pattern with no allow list, no team rules and no measurement. The guard generalises it.
- **A model judging each command.** Too slow for every call, gives different answers to the same input, and can itself be argued with by the injected text it is judging. Rejected.
- **A guard per workspace, in the workspace's committed settings.** The first draft of this design. It reaches every teammate whether they chose it or not, carries the installer's absolute path, and never sees a session outside the workspace, which is where a cloned assignment runs. Replaced by a machine-wide install in per-person files.
- **An OS-level sandbox for agents.** A better control for running untrusted code, and outside what hippo is. Teams should use one for any unfamiliar repository. The guard can recommend it in its `fresh-clone-exec` reason.

## Open questions

1. ~~Codex's `PreToolUse` input and output shape, and whether it can ask the person.~~ Answered in v0.6.1: [Codex's PreToolUse contract](#codexs-pretooluse-contract). It cannot ask.
2. Should `credential-read` of `.env` files in the workspace count? They are read constantly and legitimately. This draft leaves them to the secrets rules in [secrets.md](secrets.md#the-guard-recommends-the-runner).
3. Is `Guard.md` the right home, or should rules be a file under `.hippo/`, out of agents' reading path? A note is visible to agents, which helps them avoid the actions; it is also writable by them, which is why loosening needs a local accept.
4. Should hippo's own install line move from `curl | sh` to a signed package or Homebrew tap as the primary install, so its documentation stops teaching the pattern the guard flags?

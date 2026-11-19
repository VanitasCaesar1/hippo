# Runners: which CLI, model and effort hippo uses for its own model calls

Status: accepted; step 1 ships in v0.5.0, the rest later ([rollout](#rollout)). Revised 12 November 2026. Shared rules (per-run folders, the hardening gate, the release plan) are in [common.md](common.md).

## The problem

hippo makes its own model calls when it dreams, drafts and reviews lessons, answers `hippo ask`, reviews a pack in `hippo add`, and checks the model in `hippo doctor --model`. All of them go through one argv, `model_command`, which by default is:

```text
claude -p --no-session-persistence --setting-sources "" --tools "" --strict-mcp-config
       --system-prompt {system_prompt} --output-format json
```

The only other knob is `reviewer_command_override`, an advanced key that `init` does not write, which exists for tests and lets the dreaming reviewer, the lessons reviewer and the pack review run another argv.

That leaves four gaps:

1. **One vendor in practice.** Another CLI works only if it prints `{"result": "..."}` like `claude -p --output-format json`. `codex exec`, `opencode run`, `muse exec`, `dsh --profile headless` and Command Code's `cmd -p` do not, so using them means writing a wrapper script, and the wrapper is then responsible for isolation (Invariant 4), which hippo cannot check.
2. **No model or effort choice.** Choosing a model means editing an argv by hand. Nothing records which model reviewed a change, apart from the argv in the receipt.
3. **No account choice.** People with several Claude Code accounts (`~/.claude`, `~/.claude-work`, …) cannot say which one hippo's calls use. The default inherits `CLAUDE_CONFIG_DIR` from whatever started hippo, which for the launchd job is nothing, so the nightly dream always runs on `~/.claude`. hippo already reads every Claude account's 5-hour and 7-day meter for Fleet, but its own calls do not use that to avoid an account at its limit.
4. **The reviewer is usually the proposer's twin.** The reviewer runs in a fresh context and never sees the proposer's reasoning. It is still the same model with the same training, so it tends to share the proposer's blind spots.

Gap 3 hurts today: the nightly dream fails whenever `~/.claude` is out. Gaps 1 and 4 are real but less urgent, and the CLIs involved change their flags monthly.

## Goals

- Name a **runner** once (a CLI preset, a model, an effort level, an account) and use it by name, per **role**: the proposer, the reviewer, the lessons drafter, `ask`.
- Two presets first: `claude`, with accounts and fallback between them, and `command`, a documented contract any CLI can meet with an argv and a reply format.
- Every runner runs inside an OS sandbox that hippo sets up, so isolation does not rest on each CLI's flags.
- `hippo doctor --model` checks each runner a role uses, one call each.
- An existing `model_command` keeps working unchanged.

## Non-goals

- hippo does not get an HTTP client or call any provider's API directly. Every model call stays a child process of a CLI the person installed and logged into.
- hippo does not log anyone in, store API keys or read credential files. A runner names an account folder; the CLI reads its own credentials from it.
- No cost accounting in money. Fleet's usage figures stay estimates of headroom.
- No automatic choice of model by task.
- Agents' own sessions are not changed. This is only about hippo's own calls.
- Built-in presets for `codex`, `opencode`, `muse`, `dsh` and `cmd` in the first version. They start as `command` recipes ([below](#other-clis-recipes-first-presets-later)).

## What changes for a user

```sh
hippo runners                                 # lists runners, roles, what is installed
hippo runners add work --preset claude --account ~/.claude-work --model claude-opus-5-5 --effort high --yes
hippo config set fallback.default work,claude --yes
hippo doctor --model                          # one call per runner a role uses
```

`hippo runners` with no subcommand prints:

```text
runner    preset   model              effort  account          domain     sandbox    status
claude    claude   (CLI default)      -       ~/.claude        anthropic  on         ok (doctor 2026-11-12)
work      claude   claude-opus-5-5    high    ~/.claude-work   acme       on         not checked

role      runner   fallback
default   work     claude (refused: domain acme ≠ anthropic)
propose   (default)
review    (default)
lessons   (default)
ask       claude

found on PATH: claude 2.1.322 (3 accounts), codex 0.167.2, opencode 2.2.7
```

Nothing changes for someone who never touches this: the default runner is the `claude` preset with exactly today's argv, now inside the sandbox.

## Design

### Runners and roles in `config.toml`

Runners are per machine, like `model_command` today, so they live in `$HIPPO_HOME/config.toml` and never in the shared `.hippo/hippo.toml`.

```toml
[runners.claude]
preset = "claude"

[runners.work]
preset = "claude"
account = "~/.claude-work"
model = "claude-opus-5-5"
effort = "high"
data_domain = "acme"

[runners.local]
preset = "command"
argv = ["my-model", "--json", "--system-file", "{system_prompt_file}"]
reply = "json-result"
data_domain = "local"

[roles]
default = "work"
ask = "claude"

[fallback]
default = ["claude"]
```

| Key | Meaning |
|---|---|
| `preset` | `claude` or `command`. Required. |
| `model` | `claude` only: passed as `--model <id>`. Optional: without it the CLI's default applies and the receipt says `(CLI default)`. hippo keeps no list of model ids; the CLI rejects a wrong one, and `doctor --model` shows that. |
| `effort` | `claude` only: `low`, `medium`, `high`, `xhigh` or `max`, passed as `--effort`. Optional. |
| `account` | `claude` only: a folder, passed as `CLAUDE_CONFIG_DIR`. Optional. |
| `data_domain` | A label for where this runner's prompts may go. Default: `anthropic` for `claude`; required for `command`. Fallback never crosses it unless the role allows it ([below](#fallback)). |
| `timeout_secs` | Default 600, as today. At most 1800. |
| `argv` | `command` only. Placeholders: `{system_prompt}`, `{system_prompt_file}`, `{reply_file}`, `{run_dir}`. |
| `reply` | `command` only: `json-result` (one JSON object with a string `result`, as today), `stdout` (the whole stdout, trimmed), or `file` (the file at `{reply_file}`). Default `json-result`. |
| `env_pass` | Extra environment variable names to pass to this runner ([below](#environment)). |
| `allow_tools` | `command` only. Default `false`. See [isolation](#isolation). |
| `allow_unsandboxed` | Default `false`. See [the sandbox](#the-sandbox). |

**Roles.** `default` is required once any runner is set. The others fall back to `default`:

| Role | Calls |
|---|---|
| `propose` | Each dreaming pass's proposing run. |
| `review` | The dreaming reviewer, the lessons reviewer, the pack review in `add` and `packs update`. (Today's `reviewer_command_override` covers the same three.) |
| `lessons` | The lessons drafter. |
| `ask` | `hippo ask` without `--sources-only`. |

`eval --with-model` uses `propose` and `review` as dreaming does. `doctor --model` checks every runner some role resolves to, fallbacks included.

A per-pass runner was considered and left out: a pack is shared, runners are per machine, so a pass naming a runner would fail on every other machine.

### The two presets

| Preset | Argv (prompt on stdin) | Reply |
|---|---|---|
| `claude` | today's default argv, plus `--model <id>` and `--effort <level>` when set | the `result` of the one JSON object, as today |
| `command` | `argv`, placeholders filled | as `reply` says |

**The `command` contract**, documented on the configuration page:

- The user prompt is on stdin. hippo never puts transcript text in an argument, so it never appears in the process list.
- The system prompt reaches the CLI one of three ways: `{system_prompt}` (an argument, as `model_command` today; fine for hippo's own fixed prompts, which hold no transcript text), `{system_prompt_file}` (a file in the run folder), or, with neither placeholder, inline at the top of the user prompt between `<<<SYSTEM>>>` and `<<<END SYSTEM>>>`. Any of those two marker strings inside the user text is escaped the same way the untrusted-data markers are, so a transcript cannot close the system part early.
- The reply is read as `reply` says. A reply the parser cannot read is a model error naming the runner and the CLI's `--version` output, never a guess.
- The CLI must not keep a session in a store hippo reads. hippo cannot check that; [the second line](#leaving-no-transcript) catches it if it does.
- **The review role needs a real system prompt.** A runner that would put the system prompt inline cannot be the `review` role: the reviewer's persona is its rules, and as user text it is easier to argue with. `doctor` names the reason.

### Isolation

Invariant 4 says a model run is isolated and leaves no transcript. Two layers hold it:

1. **The sandbox** ([below](#the-sandbox)): whatever the CLI does, its process tree can read only the run folder, its own install and its account folder.
2. **No tools.** `claude` is run with `--tools ""` (verified today by the canary test). A `command` runner is assumed to have tools unless `doctor --model --canary` shows it cannot read a planted file.

**The rule: a runner with tools cannot be used for `propose`, `review` or `lessons` unless it sets `allow_tools = true`.** The reason is prompt injection. These roles read transcript text, which can carry instructions from a web page or a file an agent read. Inside the sandbox a tool can still read the account folder, which holds the CLI's credentials, and could copy them into a proposal or a reviewer's note, which git shares. So, in addition:

- A receipt's reviewer notes and a proposal's rationale are capped at 2,000 characters and redacted before they are written, like index text.
- `ask` is allowed with tools, because its output goes to the person and is not written anywhere.

### The sandbox

Every runner, `claude` included, starts inside an OS sandbox that hippo builds per call:

- **macOS:** `sandbox-exec` with a generated profile. `sandbox-exec` is marked deprecated in its man page but still ships, and Codex's own macOS sandbox is built on it.
- **Linux:** `bwrap` (bubblewrap) when it is on `PATH`; otherwise Landlock when the kernel has it.
- **Elsewhere, or when neither is available:** no sandbox. `claude` still runs (its isolation is no tools, as today). A `command` runner in a writing role is refused unless it sets `allow_unsandboxed = true`. `doctor` shows which.

What the profile allows:

| Access | Paths |
|---|---|
| Read | the run folder; the resolved CLI executable and its install prefix (for a Node CLI, the Node runtime too); the account folder (and for the default Claude account, `~/.claude.json`); system libraries and certificate stores |
| Write | the run folder (also the child's `TMPDIR`); the account folder, because CLIs update their own state there |
| Network | allowed: the CLI must reach its provider |

Everything else under `$HOME`, `$HIPPO_HOME` outside the run folder, and the workspace are denied. `doctor --model --canary` plants two files with random words: one in the run folder and one in `$HIPPO_HOME`, outside it. A runner that prints the outside word is not sandboxed, whatever this table says; the row says so and the runner is treated as unsandboxed.

A CLI update can need a path the profile lacks. The symptom is a failed call naming a denied path; `doctor --model` shows it, and the fix is a profile change in a hippo release, not a setting.

### Leaving no transcript

hippo's ingest already skips files under `$HIPPO_HOME/model-runs/`. That covers Claude Code, which is told not to save the session at all. As a second line, ingest drops **any** session whose working directory is under `$HIPPO_HOME/model-runs/` or `$HIPPO_HOME/review-runs/`, whatever adapter read it. Every runner runs there ([common.md, rule 1](common.md#rule-1-one-folder-per-model-run)), so a CLI that saves its session somewhere hippo reads cannot feed hippo's own calls back into its index.

### Environment

Today a model child inherits hippo's whole environment. With runners it gets:

- a base list: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `LC_*`, `TERM`, `TZ`, `XDG_*`, the proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` in both cases), and the certificate variables corporate networks need (`SSL_CERT_FILE`, `SSL_CERT_DIR`, `NODE_EXTRA_CA_CERTS`, `REQUESTS_CA_BUNDLE`);
- `TMPDIR` set to the run folder;
- the preset's login variables. For `claude`: `CLAUDE_CONFIG_DIR` (set from `account`), `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`, `AWS_*`, `GOOGLE_APPLICATION_CREDENTIALS`, `CLOUD_ML_REGION`, `ANTHROPIC_VERTEX_PROJECT_ID`. A `command` runner has none by default;
- the runner's `env_pass` names.

Nothing else. A credential sitting in the shell that started `hippo dream` does not reach the CLI that sends prompts out. [secrets.md](secrets.md#hippos-own-model-calls-see-no-secrets) relies on this rule; it is decided here.

### Accounts

For `claude`, `account` sets `CLAUDE_CONFIG_DIR=<folder>`, and the sandbox profile allows that folder. `hippo runners` lists the Claude account folders it found (the same `~/.claude*` folders ingest reads) so the person can pick one.

The receipt records the **runner's name**, never the folder path or folder name: receipts are shared through git, and a home folder path names its owner while a folder such as `.claude-acme` can name a client. `hippo runners add` says that the name will appear in shared receipts.

### Fallback

A role may list fallback runners. hippo tries the role's runner first and moves to the next one when:

- the runner's account meter, as Fleet reads it, shows a window at 95% or more and not yet reset, checked before the call; or
- the call fails with the CLI's usage-limit message (the `claude` preset knows its wording; a `command` runner can declare `limit_exit_codes`).

It never falls back on any other error: a login failure, a timeout or an unreadable reply stops the run, as today.

**Fallback stays inside a data domain.** A fallback list may name only runners with the same `data_domain` as the role's runner. Two Claude accounts are both `anthropic` by default, so personal accounts fall back to each other with no setting. A work account labelled `acme` does not fall back to a personal one, because the work account may be covered by an agreement the personal one is not. A role that should cross sets `[fallback] cross_domain = ["ask"]`; `hippo runners` and `hippo dream --dry-run` print each role's possible domains, so a crossing is visible before any call.

Every fallback is written in the run's report and in the receipt (`runner: claude (fallback from work: 5-hour window at 97%)`). A review that cannot run on an allowed runner fails the run, so the dreaming window does not advance and the next run picks it up.

### Other CLIs: recipes first, presets later

`codex exec`, `opencode run`, `muse exec`, `dsh --profile headless` and `cmd -p` start as **recipes**: documented `command` runners with an argv, a `reply` mode and the `env_pass` names they need. The flags below were read from each CLI's `--help` on 12 November 2026; none was run against a model for this design.

| CLI | Version read | Candidate recipe | Unverified |
|---|---|---|---|
| `codex` | 0.167.2 | `codex exec - --ephemeral --sandbox read-only --skip-git-repo-check --ignore-user-config --ignore-rules -C {run_dir} -o {reply_file}`, `reply = "file"`, `env_pass = ["CODEX_HOME", "OPENAI_API_KEY"]` | whether `-c` can set a system prompt with `--ignore-user-config` (without it Codex cannot be the reviewer) |
| `muse` | 1.4.10 | `muse exec --prompt-file {run_dir}/prompt.txt --disable-shell --disable-write --disable-web-tools --approval-mode never --no-session-log --json` | the final-message event name; whether a read tool remains |
| `opencode` | 2.2.7 | `opencode run --standalone --format json --agent hippo-norun` with an inline config denying every tool | the event schema; whether `OPENCODE_DB` moves only the database |
| `dsh` | 0.1.5-rc.1 | `dsh --profile headless --patch {run_dir}/hippo.patch.yml` | whether it reads the task from stdin or a file; without that it cannot be used, since the prompt would be in an argument |
| `cmd` | not installed | `cmd -p --output-format json` | everything |

A recipe moves into the documentation after the end-to-end check in the [test plan](#test-plan) passes for it. It becomes a built-in preset (with model and effort keys) only when people use the recipe and the CLI's flags have held still for two of its releases. The sandbox makes a recipe safe to try either way: a CLI that ignores its own read-only flag still cannot read outside the run folder.

### The reviewer on another vendor

Later, with a recipe that can take a real system prompt. An optional rule in `.hippo/hippo.toml`, because it is a team decision about how notes are checked:

```toml
[review]
independent = "vendor"   # "none" (default), "model" or "vendor"
```

- `model`: the review role must resolve to a different model id than `propose`.
- `vendor`: to a different `data_domain` provider (`anthropic` vs `openai`, …). A `command` runner whose domain is a free label does not satisfy it unless the label is one of the known providers.
- The rule applies to dreaming and to lessons, which have a proposing model. The pack review in `add` has no proposer (a person wrote the pack), so the rule does not apply there.

When the rule is not met, `hippo dream` fails before its first call and names the two runners. This table needs `min_hippo_version` and lowering it waits for a local accept ([common.md, rules 2 and 3](common.md#rule-2-security-settings-in-the-shared-hippotoml-need-a-version-floor)).

What a different vendor buys: a reviewer trained differently is less likely to share the proposer's specific mistakes. What it does not buy: it is still a model judging a model, and hippo has not measured whether it rejects more wrong changes. It costs a second login and sends transcript excerpts to a second provider. The default stays `none`.

### `hippo doctor --model`

Today `doctor --model` sends one word through `model_command`. With runners it does that once per runner some role resolves to, and prints one row each:

```text
model: work     ok     replied "ok" in 3.1 s; no tools; sandbox on; account work
model: claude   warn   5-hour window at 97%, resets 14:40
model: local    ok     replied "ok" in 1.2 s; tools unknown (run --canary); sandbox on
```

`--runner <name>` checks one. Without `--model`, the rows check only that the program is on `PATH`, the account folder exists, the sandbox tool is present, and the CLI's `--version` is one hippo was checked against (newer is a warning, not a failure). `--canary` is opt-in, one more model call per runner.

### CLI

```text
hippo runners [--json]
hippo runners add <name> --preset claude|command [--model <id>] [--effort <level>] [--account <dir>]
                  [--domain <label>] [--argv <json>] [--reply <mode>] [--allow-tools] [--yes]
hippo runners remove <name> [--yes]
hippo runners migrate [--yes]
hippo config set role.<role> <runner> [--yes]
hippo config set fallback.<role> '<runner>[,<runner>…]' [--yes]
hippo config unset role.<role> [--yes]
```

As with every `config` change, these print the change and write it only with `--yes`. Removing a runner that a role uses is refused, naming the role.

### Migration from `model_command`

- No `[runners]` table and no `model_command`: an implicit runner `claude` with today's argv is the default. Nothing is written.
- `model_command` set: an implicit runner `custom` with `preset = "command"`, that argv, `reply = "json-result"` and `data_domain = "custom"`.
- `reviewer_command_override` set: an implicit runner `custom-review` is the `review` role.
- **Both the old keys and a `[runners]` table:** the configuration still loads. Hooks, `context`, `brief`, `guard check` and every command that makes no model call work as usual. Only model-calling commands fail, naming both keys and `hippo runners migrate`.
- `hippo runners add` on a configuration with old keys migrates first: it prints the migration and the new runner as one change and writes both with `--yes`. It never leaves both.
- The old keys stay readable for at least two minor versions.

### Records

A receipt's `invocation` keeps today's form. Receipts and lesson reviews gain optional fields; old records still load:

```json
"runner": {"name": "work", "preset": "claude", "cli_version": "2.1.322",
           "model": "claude-opus-5-5", "effort": "high", "data_domain": "acme",
           "sandbox": "on", "tools": "none", "system_prompt": "flag", "fallback_from": null}
```

## Data, privacy and security

- **Where prompts go.** Each runner sends its prompts to its CLI's provider under the person's login. `hippo runners` prints, per role, the data domain it sends to, and the security page's list A becomes "the runners your roles use".
- **No new secrets.** hippo stores folder paths, model ids and environment variable names, never values.
- **Isolation rests on the sandbox, then on the CLI's flags.** The sandbox limits reads to the run folder, the CLI and its account folder whatever the CLI's flags do; the account folder is the residual, which is why tools are refused for the writing roles and why notes are capped and redacted.
- **Process list.** Prompts stay on stdin or in a file inside the run's own folder, mode 0700, removed when the call ends.
- **Runaway cost.** A large model at `max` effort makes every nightly pass slower and dearer. `hippo dream --dry-run` prints which runner each role will use before any call.

## Test plan

- **Argv golden tests** for `claude` per effort level and with an account; for `command` with each placeholder and reply mode.
- **The default is unchanged**: with no `[runners]`, the argv equals today's `default_model_command()` byte for byte, and the call runs inside the sandbox.
- **Stub CLIs** named `claude` and `my-model` on a test `PATH` that record their argv and environment and print canned replies: argv, environment (base list, preset variables, `env_pass`, nothing else), stdin, timeouts, fallback on the usage-limit message.
- **Concurrency**: two calls at once with a stub that writes `last.txt` and sleeps; each reads its own reply ([common.md, rule 1](common.md#rule-1-one-folder-per-model-run)).
- **Sandbox**: on macOS and on Linux with `bwrap`, a stub CLI tries to read a file in `$HOME`, in `$HIPPO_HOME` and in the workspace, and to write outside the run folder; all four fail. Without a sandbox tool, a `command` runner in a writing role is refused.
- **Markers**: a transcript containing `<<<END SYSTEM>>>` is escaped in an inline prompt.
- **Fallback**: fake meters at 94% and 96%; a usage-limit reply; a login failure, which must not fall back; a fallback to another domain, refused unless `cross_domain` names the role.
- **Notes cap**: a reviewer note of 5,000 characters holding a planted token is stored capped and redacted.
- **Migration**: old keys alone; new table alone; both (loads; `context` works; `dream` fails naming both); `runners add` migrates first; `migrate` round trip.
- **End to end, by hand before a release that documents a recipe**: the real CLI with a throwaway account, on one synthetic dreaming chunk, with both canary files in place; results recorded in the pull request.

## Rollout

Part of the joint plan in [common.md](common.md#release-plan).

1. **v0.5.0, runners step 1**, after per-run folders ([common.md, rule 1](common.md#rule-1-one-folder-per-model-run)): the runner abstraction; the `claude` preset with model, effort and accounts; fallback within a data domain; the `command` contract with its three reply modes; the sandbox on macOS and Linux; the environment rule; migration; the `runner` field in receipts. This is the part that helps today: the nightly dream stops failing when one account is out.
2. **Later:** recipes for `codex`, `muse`, `opencode`, `dsh` and `cmd`, each documented once its end-to-end check passes; promotion of a used recipe to a preset.
3. **Later:** `[review] independent`, once a recipe on another provider can take a real system prompt.
4. **Later:** the app's Models card (one row per role with a runner picker, each runner's last `doctor --model` result and a Check button, Add runner for `claude` accounts). It writes through the same `hippo config` code path and can ship in any release after step 1.

## Alternatives

- **Keep `model_command` and document wrapper scripts.** No code, but every wrapper reimplements isolation, parsing and account choice, and hippo cannot tell a receipt reader what ran. Rejected; the `command` contract plus the sandbox is the smallest version of this that hippo can check.
- **Six built-in presets at once.** The first draft of this design. Each rests on flags nobody has run, for CLIs that change monthly, and each needs its own isolation argument. Replaced by recipes behind one sandbox.
- **Call providers' HTTP APIs directly.** Simpler parsing and real system prompts, but hippo would hold API keys and have an HTTP client. Rejected.
- **A runner per pass in the pack.** Packs are shared and runners are not. Rejected.
- **Make a different-vendor reviewer the default.** It would need a second login from everyone and sends data to a second provider without asking. Rejected as a default.

## Open questions

1. Which paths does each Claude Code version need in the sandbox profile beyond its install, account folder and `~/.claude.json` (for example a shared cache folder)? Answered by running the sandbox test against the installed version before v0.5.0.
2. Can Codex take a system prompt through `-c` while `--ignore-user-config` is set? Until it can, Codex cannot be the reviewer.
3. Is Landlock without `bwrap` enough on Linux, given it cannot hide paths, only deny access to them?

# Secrets: letting agents use credentials without a model seeing them

Status: accepted; detection ships in v0.5.0, the rest later ([rollout](#rollout)). Recommends **not** putting a secret store inside `hippo`; see [the recommendation](#recommendation). Revised 12 November 2026. Shared rules (version floor, local accept, hardening gate, release plan) are in [common.md](common.md).

## The problem

Agents are more useful when they can deploy, call an API or run a migration, and each of those needs a credential. Today an agent gets one of three ways, and all three put the value where it does not belong:

- **It reads `.env`** (or `cat`, `printenv`, `docker inspect`, `kubectl get secret -o yaml`). The file's contents become a tool result, which goes to the model provider and into the agent's transcript on disk.
- **The person pastes it** into the chat. Same result.
- **It is in the environment** the agent inherited. The value stays out of the model only until a command prints it: `env`, a verbose `curl`, a stack trace, a debug log.

Once a value has reached a provider, nobody can take it back; the only fix is to rotate it. hippo sees all of this after the fact: it reads every agent's transcripts, and its redaction removes secret-shaped text before indexing. But it records nothing about what it removed, so nobody is told.

Tools exist that keep values out of files: a reference such as `op://vault/item/field` sits in a file that is safe to commit, and a runner (`op run`, `doppler run`, `infisical run`, `sops exec-env`, `aws-vault exec`) resolves it into a child process's environment at run time. ZVault (`github.com/VanitasCaesar1/zvault`) works the same way: `.env` import, encrypted values at rest, `zvault://` references, `zvault run`. ZVault is incomplete and hippo will not depend on it. This document uses its public repository as one reference for the idea, together with the tools above, and copies no code.

The question here is whether hippo should have such a feature itself, and if so, the smallest version that does not make hippo more dangerous than it is.

## Goals

- Agents can run commands that need credentials, and the values reach neither a model nor a transcript in the normal course of work.
- Every agent hippo supports learns which **references** exist and how to use them, through the start block, never the values.
- hippo tells the person when a secret **did** reach a model, in any agent's transcript, and what to do.
- hippo's own model calls never see a secret value.
- The design holds up against the attack described in [guard.md](guard.md): malware on a developer machine, and an AI tool driven by an attacker.

## Non-goals

- A team secret manager: no sharing of values, rotation, leases, dynamic credentials or server. Teams that need those already have a tool for them.
- Stopping a determined agent, or an attacker with code execution as the user, from using a secret that is unlocked. No local tool can ([threat model](#threat-model)).
- Editing other tools' transcript files to remove a leaked value.

## Lessons from the references

Read in ZVault's public repository (shallow clone, November 2026; not built or run) and in the documentation of the tools above:

| Idea | Take it? | Why |
|---|---|---|
| References in a file that is safe to commit (`KEY=zvault://env/<project>/KEY`, `KEY=op://…`) | Yes | It lets the repository, the notes and the start block talk about secrets by name. |
| `run -- <cmd>` that injects values into one child's environment | Yes, with limits | It is the one moment a value has to exist in plain text, kept to one process. |
| Masking values in the child's output (1Password's `op run` does this by default) | Yes, as a safety net only | It catches accidental prints, not deliberate ones. |
| An append-only, tamper-evident audit file | Yes | Who used which secret, from which agent, is the record a person needs after an incident. |
| A server for local use, unsealed with Shamir shares | No | Too much machinery for one developer's laptop. The OS keychain already exists. |
| An MCP tool that runs any shell command with secrets in its environment and scrubs exact values from the output (`zvault_run_command`) | No | The model chooses the command, so `env \| base64` or `printenv \| rev` returns the value past the scrub. |
| An MCP tool that takes a secret's value from the model (`zvault_set_secret`) | No | The value has already been in the model's context. |
| Leaving a plaintext `.env.backup` after import | No | Import has to leave no plaintext copy behind. |
| Per-organisation keys stored in the same database as the ciphertext (ZVault's cloud tier) | No | The key sits beside the data it protects. |

The main lesson from the "no" rows: **anything that lets the model choose the command that receives secrets has given the model the secrets.** Output scrubbing does not change that.

## What only hippo can do

Most of this problem is solved by existing secret runners. Three parts are not, and all three fit what hippo already is:

1. **Detection across agents.** Only a tool that reads every agent's transcripts can say "the production database URL reached Anthropic in a Codex session on Tuesday, through `cat .env`".
2. **Telling every agent the rules.** The start block reaches Claude Code, dsh and OpenCode sessions, and Codex through its `SessionStart` hook from 0.4.0 ([codex-hooks.md](codex-hooks.md)). It can name the references and the run command for that workspace.
3. **The guard** ([guard.md](guard.md)) can see a command before it runs and steer an agent from `cat .env` to the run command.

None of these needs hippo to hold a value. That is the basis for the recommendation.

## Recommendation

**Keep the secret store out of `hippo`. Build the three parts above now, against whichever runner the workspace uses. If a built-in store is still wanted after that, ship it as a separate, small, signed helper binary that the `hippo` process never links, and only after hippo's releases are signed.**

The reasons:

- **hippo has a large attack surface already.** It parses four agents' transcript formats, opens SQLite files other programs write, fetches packs with git, builds prompts from untrusted text and runs models on them. A bug in any of that becomes a way to the keys if the keys sit in the same process.
- **hippo is already a high-value target** ([guard.md](guard.md#hippo-as-a-target)). Its index is a map of an organisation's systems. Adding the credentials turns "a map" into "a map and the keys".
- **Existing runners do the hard part** (storage, unlock, sharing, rotation) and have been reviewed for it. 1Password's `op run` already masks values in output.
- **Today hippo's builds are not signed.** Keychain access control on macOS is tied to a binary's code signature. An ad hoc signature changes with every build, which either prompts on every update or trains people to click "Always Allow". A secret store shipped before signed releases would be weaker than the keychain items people already have.

What would change this: hippo's releases are signed and notarised, the guard and detection are in use, and people still have no runner they are willing to install. Then the [minimum safe version](#design-part-2-the-minimum-safe-version-if-built) below is the plan.

The rest of this document designs both: first the parts to build now, then the helper.

## Design, part 1: what to build now (no values in hippo)

### Workspace settings: references, not values

```toml
# .hippo/hippo.toml (shared)
[secrets]
runner = "op"                 # one of hippo's runner templates, below
refs_file = ".env.refs"       # a committed file of NAME=<reference>

[secrets.names]
DATABASE_URL = { env = "staging", use = ["npx prisma migrate deploy", "npm run db:seed"] }
STRIPE_SECRET_KEY = { env = "test", use = ["stripe"] }
```

**The runner is chosen, not written.** The start block tells every agent to run commands through the runner, so the runner's text goes straight into agents' instructions. A free-form string there would be a command-injection channel for anyone with push access. So `runner` names one of hippo's templates, and hippo renders the command from it:

| `runner` | Rendered command | Arguments it takes |
|---|---|---|
| `op` | `op run --env-file <refs_file> --` | none |
| `doppler` | `doppler run --project <project> --config <config> --` | `project`, `config` |
| `infisical` | `infisical run --env <env> --` | `env` |
| `sops` | `sops exec-env <file>` | `file` |
| `aws-vault` | `aws-vault exec <profile> --` | `profile` |
| `hippo-secrets` | `hippo-secrets run --` | none |

Arguments go in `[secrets.runner_args]`. Each is checked when the settings load: a file must be a relative path inside the workspace, and every other argument must match `[A-Za-z0-9._-]{1,64}`. Anything else is a load error for the `secrets` part, which is then left out of the start block; `context` itself still works.

`[secrets]` follows the shared rules ([common.md, rules 2 and 3](common.md#rule-2-security-settings-in-the-shared-hippotoml-need-a-version-floor)): it raises `min_hippo_version`; a new or changed `runner` or argument, and a new `use` entry, is a loosening that waits for a local accept on each machine. Until accepted, the start block uses the last accepted values, or leaves the `secrets` part out if there are none.

hippo reads `refs_file` only to learn names and to check that each line's value is a reference (it starts with a known scheme: `op://`, `doppler://`, `infisical://`, `vault://`, `hippo-secret://`, `zvault://`) and not a value. `doctor` reports any line whose value looks like a secret (no scheme, high entropy, or a redaction pattern match) as a `problem`, without printing the value. A value counts as a reference only if the whole value parses as one (scheme, then path segments of `[A-Za-z0-9._/-]`, no spaces, no `@` or `:` after the scheme); `op://` followed by anything else is treated as a value.

### The start block names references

`hippo context` and `hippo brief` add one part, `secrets`, when `[secrets]` is set:

```text
secrets: DATABASE_URL (staging), STRIPE_SECRET_KEY (test). Values are never shown to you.
  Run a command that needs them as: op run --env-file .env.refs -- <command>
  Allowed with secrets: npx prisma migrate deploy, npm run db:seed, stripe
  Never cat, print or echo .env files or these variables.
```

It is not a required part: a required part that does not fit makes `hippo context` fail. It comes right after the required notes, capped at 600 bytes. When the full part does not fit the budget, it shrinks to its last two lines (the run command and "never print"), and when even that does not fit, it is left out and `doctor` says so. Agents without hippo's start block do not get it; the guard still applies where it is installed.

### Redaction stays the last line, and starts counting

Redaction on ingest is unchanged in what it removes. Two additions:

- **References are not redacted.** `op://…`, `hippo-secret://…` and the other schemes are names, and they are useful in search. The exception applies only to a match of the whole reference syntax above; a string that starts with a scheme and then holds anything else is redacted as usual.
- **Every redaction is recorded.** Ingest writes a row per replaced match: session, turn, the turn's role (person, tool result, assistant, tool call), the kind, and, when the turn was a tool result, the call that produced it. Never the value, a hash of it, or its length. This is what turns redaction into detection. The rows are a new store of transcript-derived data, so they ship only after `$HIPPO_HOME` permissions are enforced ([common.md, rule 4](common.md#rule-4-the-hardening-gate)).
- **Rows exist only for text ingested after the upgrade.** Ingest skips files it already holds, so older sessions have no rows until `hippo ingest --full` re-reads everything. The release notes say so, `secret-to-model` shows "counted since <date>", and `doctor` suggests `--full` once.

**As built in v0.5.0.** The rows live in a `redactions` table in `index.sqlite` (0600), beside `index_meta` whose `redactions_since` is `full` for an index built new or by `ingest --full`, else the time a 0.5.0 writer added the table. The role words are `person`, `assistant`, `tool call` and `tool result`; a tool result's call is the latest earlier call of the same tool with no result yet. Each row also says whether the matched text is a published example or a test key (`example`), decided at redaction time, since the row cannot hold the value. `hippo incidents` prints the "counted since" line while `redactions_since` is not `full`; `doctor`'s `secrets` row names `ingest --full` as an `ok` row, not a warning.

### Detection: `secret-to-model`

An incident kind (listed with the others in [guard.md](guard.md#part-a-after-the-fact-from-transcripts)) built from those rows, with no model call.

**Secret kinds only.** Redaction also removes email addresses, phone numbers, Aadhaar numbers and `key=value` pairs whose value looks random (`crates/hippo-core/src/redact.rs`). Those protect the index; they are not credentials, and an incident on each would bury the real ones. `secret-to-model` counts only these kinds, ranked:

| Confidence | Kinds | Fix it suggests |
|---|---|---|
| high | `pem` (private keys), `aws`, `github`, `provider` (keys with a known vendor prefix) | rotate |
| medium | `auth` (bearer and basic headers), `jwt` | check whether it was live; rotate if so |

`kv`, `email`, `phone` and `aadhaar` never raise it. Known example and test values never raise it either: AWS's documentation keys (`AKIAIOSFODNN7EXAMPLE` and its secret), Stripe's `sk_test_`/`pk_test_`/`rk_test_` keys, and a short list kept with the redaction patterns, each with its source. As built: the list in `redact.rs` holds AWS's and GitHub's documentation keys and jwt.io's example payload, any match holding `example`, `xxxxxxxx` or `00000000` counts as a placeholder, and Stripe's `sk_test_` keys are excluded (the `pk_`/`rk_` forms match no redaction pattern). Medium-confidence incidents are listed with `--all` and in the app's folded row, as destructive commands with no harm after them are. Fleet and Incidents show high-confidence incidents by default, with medium ones one click away.

```text
secret-to-model  codex  2026-11-10 14:02  provider key (sk-live…), high, in a tool result
                 from: Bash  cat .env
                 reached: the transcript sent to the session's model provider
                 fix: rotate the key; assume the provider has it. Then put STRIPE_SECRET_KEY in .env.refs
                      and run commands with `op run --env-file .env.refs --`.
```

- **Where it came from** is the tool call whose result held it (`cat .env`, `printenv`, `docker inspect`, `kubectl get secret`, a file read), or the person's own message (then "you pasted it").
- **What it reached:** every turn of a session is sent to that agent's model provider, unless the agent itself filtered it, which hippo cannot see. So the incident says the value was in a turn sent to the provider and tells the person to assume it arrived; it names the agent, not a guess at the provider.
- **The fix** starts with rotation for a high-confidence kind, because nothing hippo does removes the value from the provider or from the agent's own transcript file. The incident names that file and line, so the person can delete it if they want to; hippo does not edit it.
- **Fleet and Incidents** count `secret-to-model` per agent and per day, with the other incident kinds.

Detection is only as good as redaction's patterns: a secret that matches none (a short password, a custom token format) is not found. The [helper](#scan-exact-matches-without-handing-hippo-the-values) improves on this, if it is built.

### The guard recommends the runner

With `[secrets]` set, the guard ([guard.md](guard.md)) gets three rules, `warn` by default:

- a read of `.env`, `.env.*` (not `.env.refs` or `.env.example`) or of a file that defines a name in `[secrets.names]`;
- `printenv`, `env`, `export -p`, `set` with no arguments, or `echo $NAME` for a listed name;
- a listed name assigned a literal value on the command line (`DATABASE_URL=postgres://… npm run …`).

Each reason names the runner command. The agent can retry correctly without the person.

### hippo's own model calls see no secrets

- Prompts are built from redacted index text, so they hold no values that redaction caught.
- **Runner environments are limited.** Today a model child inherits hippo's whole environment. With runners, each child gets a base list, its preset's login variables and the runner's `env_pass` names, nothing else ([runners.md](runners.md#environment), which decides this rule; it ships in v0.5.0). A credential sitting in the shell that started `hippo dream` does not reach the CLI that sends prompts out, while the variables a CLI needs to log in (`ANTHROPIC_API_KEY`, Bedrock and Vertex settings, corporate CA bundles) still do. This applies whether or not `[secrets]` is set.
- The `secrets` part of the start block (names and rules) is never put into a dreaming prompt.

## Design, part 2: the minimum safe version, if built

If, after part 1, a built-in store is still wanted, this is the smallest version that keeps the properties above.

### A separate binary: `hippo-secrets`

| | One binary (`hippo secrets …`) | Separate helper (`hippo-secrets`) |
|---|---|---|
| Code that can touch a value | All of hippo: adapters, SQLite, packs, git, prompt building | One small crate: keychain, run, scan |
| A memory-safety or logic bug in transcript parsing | Runs in the process that can decrypt | Cannot reach the keys |
| Keychain access control | Tied to the binary that also reads every transcript | Tied to a binary that does nothing else |
| Signing and review | One large binary | Small enough to audit; signed separately |
| Install | Nothing extra | One more file in the release |

The helper is its own crate in the hippo repository (`crates/hippo-secrets`) with **no dependency on `hippo-core`**, no network, no model calls, no packs, no transcript parsing, no SQLite. It is released and signed with hippo, but as its own executable. The `hippo` binary never links keychain code and talks to the helper only by starting it as a child process with a pipe. hippo can work fully without it.

### Where values live

| Option | For | Against |
|---|---|---|
| **OS keychain** (macOS Keychain; Secret Service on Linux desktops) | Encrypted by the OS, unlocked with the login, per-item access control bound to the helper's code signature, no new passphrase | Linux servers and WSL often have no Secret Service; values do not move between machines (a feature here) |
| Encrypted file (XChaCha20-Poly1305, key from Argon2id over a passphrase, or from the keychain) | Portable; works headless | A passphrase prompt is impossible for an agent's non-interactive command, so it ends up in an environment variable; or the key is in the keychain anyway |

**Minimum version: macOS Keychain only**, one generic-password item per secret, service `dev.hippo.secrets`, account `<workspace id>/<NAME>`, with an access control list naming only the signed helper. The workspace id is a ULID written once into the shared `hippo.toml`, so `hippo-secret://DATABASE_URL` means the same name on every teammate's machine, each with their own value. Linux comes later, Secret Service only; no encrypted-file fallback in the minimum version.

### Team: share references, not values

The refs file and `[secrets.names]` are committed: names, which environment, which commands may use them. Values are set on each machine by the person, which also means each person can use their own token rather than a shared one; that is better practice where the service allows it. Sharing values is out of scope: a team that needs it uses its secret manager and points `runner` at it. An encrypted, per-recipient file in git (the `sops`/`age` model) was considered and left out of the minimum version: revoking a teammate then means rotating every secret, and that is a secret manager's job.

### Commands

```text
hippo-secrets set <NAME>                  # reads the value from the terminal, no echo; refuses when stdin is not a terminal
hippo-secrets import <file> [--yes]       # .env → keychain + refs file; no plaintext copy kept
hippo-secrets list                        # names, when set, last used; never values
hippo-secrets remove <NAME>
hippo-secrets run [--name N]... -- <cmd> [args]
hippo-secrets unlock [--for 1h] | lock
hippo-secrets audit [--days 7]
```

There is **no command that prints a value**: no `get`, no `show`, no `export`. A person who needs to see one opens Keychain Access. `set` refuses input that is not a terminal, so an agent does not pipe a value in by accident, which would put it in its transcript anyway. This is a guard against mistakes, not a security control: `script` or `expect` gives a program a terminal.

`import` reads a `.env`, stores each value, writes `KEY=hippo-secret://KEY` lines to the refs file, and then prints the command that deletes the original. It never deletes it on its own, and it keeps no backup. If git has ever tracked the file (`git log --all -- <file>` finds it), it says the values are in the repository's history and must be rotated.

### `run`: the one place a value exists in plain text

`hippo-secrets run -- <cmd>` resolves the names listed for the workspace, checks the command against policy, starts `<cmd>` with those variables added to its environment, and passes the child's output through.

**Policy.** A secret is passed only to a command matching one of its `use` prefixes in `[secrets.names]`, compared on the parsed argv, not on a string. Refused outright, whatever `use` says:

- shells and interpreters given code inline (`sh -c`, `bash -c`, `zsh -c`, `node -e`, `python -c`, `ruby -e`, `perl -e`, `deno eval`, `osascript -e`);
- `env`, `printenv`, `echo`, `printf`, `cat`, `tee` as the command;
- a command whose arguments contain `$NAME` or `${NAME}` for a listed name, since that only makes sense if the value is meant to be visible in the command line.

The policy cannot know what an allowed program does with a variable. Many `use` entries run repository code that an agent can edit: `npm run deploy` runs whatever `package.json` says `deploy` is, `npx prisma` runs the version in `node_modules`, `make` runs the `Makefile`. An agent that edits `package.json` turns an allowed command into any command. So each `use` entry pins the files that define it:

```toml
[secrets.names]
DATABASE_URL = { env = "staging", use = [
  { cmd = "npm run db:seed", pin = ["package.json", "scripts/seed.ts"] },
  { cmd = "npx prisma migrate deploy", pin = ["package.json", "package-lock.json"] },
] }
```

- A plain string `use` entry, as in part 1, is the same as `{ cmd = "…" }` with the default pins below.
- `hippo-secrets accept` records a hash of each pinned file per machine, after showing the person the entry. A pinned file that changed makes `run` refuse that entry until the person accepts again; the refusal names the file.
- For `npm run`, `pnpm run`, `yarn`, `make` and `just`, an entry without `pin` gets the obvious defining file pinned (`package.json`, `Makefile`, `justfile`), and `doctor` asks for the scripts those files call.
- What a pin cannot cover: code the pinned file loads from elsewhere (`node_modules`, a script it names that is not pinned). The person decides which programs are trusted with a value; pins make an edit to the obvious ones visible.

**Output.** The child's stdout and stderr go through a filter that replaces each value, its base64, its URL-encoded form and its hex form with `[hippo-secret:NAME]`. This catches the accidental print (a verbose `curl`, a connection string in an error). It does not catch a value split, reversed, encrypted or sent elsewhere, so it is a safety net, as redaction is.

**Process boundary.** The value exists in the helper's memory while it starts the child, and in the child's environment. Other processes of the same user can read a process's environment on Linux (`/proc/<pid>/environ`) and in some cases on macOS. The helper zeroes its copies after the child starts. It is not a daemon, by default: each `run` reads the keychain, so values are not held between calls.

### Unlock and lock

- **Default items.** `set` creates an item that the signed helper may read silently while the login keychain is unlocked, and that any other program can read only after the keychain's own prompt. `run` does not prompt while the user is logged in.
- **`--confirm` items** prompt on every read. For a run of commands that need one, **`unlock --for <duration>`** (maximum 12 hours) exists: the helper asks once, then keeps those values in a short-lived agent process listening on a socket in `$HIPPO_HOME/secrets/` (mode 0600), which locks on timeout, on `lock`, and on sleep. This is the `ssh-agent` model, and it has `ssh-agent`'s weakness: any process of the same user can use the socket while it is unlocked.
- Touch ID per use would need the data-protection keychain, which needs an app bundle with entitlements. Out of the minimum version; an open question.

### Audit log

`$HIPPO_HOME/secrets/audit.jsonl`, one line per `run`, `set`, `import`, `remove`, `unlock`:

```json
{"at":"2026-11-12T10:41:07Z","op":"run","names":["DATABASE_URL"],"argv0":"npx","argv1":"prisma",
 "agent":"claude-code","session":"…","decision":"allowed","exit":0,"masked":1,"prev":"<hmac>"}
```

- `agent` and `session` come from what the calling agent sets in its environment (Claude Code, Codex, dsh and OpenCode each set something a hook can read). The field is "as reported" and can be forged by the caller.
- Each line carries an HMAC of the previous line, with a key kept in the keychain, so an edit or a deleted line is detectable by `audit`. Detectable, not preventable: the user can delete the file.
- `masked` counts replacements by the output filter. A non-zero count is shown in `hippo incidents` as `secret-in-output`: the value did not reach the model, but the command tried to print it.

hippo reads the audit log, which holds no values, to show per agent: which secrets were used, how often, refusals, and masked output.

### Scan: exact matches without handing hippo the values

Pattern redaction misses secrets with no recognisable shape. With the helper installed, ingest can find exact values without ever holding them:

- When a value is set, the helper stores, in a file under `$HIPPO_HOME/secrets/` (0600), the value's length and an HMAC of it under a key that stays in the keychain. For values shorter than 12 characters, nothing is stored.
- At ingest, hippo pipes each new turn's raw text to `hippo-secrets scan` before redaction. The helper splits the text into **tokens** (on whitespace, quotes, and the `=`, `:` and `,` that separate keys from values) and checks only tokens whose length equals a stored length. One HMAC per such token, not one per position, so the cost grows with the number of tokens, not with text length times stored lengths.
- It returns the turn with each matching token replaced by `[redacted:secret:NAME]`. hippo indexes that text and records the redaction, so `secret-to-model` names the exact secret.

**The oracle.** Any program running as the user can send text to `scan` and learn whether it contains a stored value. Returning names instead of offsets does not change that: the caller holds the text it sent. What limits guessing:

- values under 12 characters are never stored, and a token must match a whole value, so a guess is a whole value, not a prefix;
- `scan` accepts at most 20 MB of text and 2,000 calls per hour, counted in the helper's state file, and refuses past that;
- every call writes an audit line (caller's parent process name, bytes, matches), so a burst of guessing is visible in `hippo-secrets audit`.

A compromised `hippo` already holds the raw transcripts, so the replaced tokens tell it nothing it did not have. It cannot learn a value that was never in a transcript, except by guessing within those limits.

**`ingest --full`.** Scan runs only over text ingested after each value was set; `ingest --full` does not re-scan old text, which keeps a full re-ingest within the rate limit. `hippo-secrets scan --backfill`, run by the person, scans the existing index once, outside the hourly limit.

This costs a second process during ingest. It is the strongest reason to build the helper at all, and it keeps the values on the helper's side of the pipe.

## Threat model

What each party can do in each design. "Part 1" is the recommendation for now; "helper" is part 2; "one binary" is a store inside `hippo`.

| Party | Part 1 (no store) | Helper | One binary |
|---|---|---|---|
| **A careless agent** that prints a value | Detected after the fact (by pattern); the guard warns before `cat .env` | Output filter masks it; audit records it; exact-match detection | Same as helper |
| **A prompt-injected agent** trying to send a value out | Has whatever the runner gives it; the guard flags unknown egress | Limited to `use` commands, shells refused. An allowed command that runs repository code (`npm run`, `make`) is only as safe as that code: pinned files refuse after an edit until re-accepted, unpinned code the command loads is not covered | Same, plus any bug in hippo's parsers reached through the injected text runs next to the keys |
| **A malicious pack or template** | Text only; reaches models, never values | Same: hippo never calls the helper because of pack content | Same today; one future mistake away from more |
| **A compromised `hippo` binary** (supply chain) | Sees every tool call (with the guard) and the index | Can start the helper as the user can; the keychain's code-signature check stops it reading items directly | Has the keys |
| **Malware running as the user** (Binance step 2) | Reads `.env` files and the environment | While locked: a keychain prompt the person can notice. While unlocked: can use the socket or start `run` | Same as helper, with a larger target |
| **An attacker driving an AI tool on a machine they control** (Binance step 3) | Uses what the machine has | Can use unlocked secrets through allowed commands; cannot read locked items silently; leaves audit lines unless they delete them | Same |
| **A teammate with push access** | Can edit refs, `use` lists and the runner choice; each loosening waits for a local accept ([common.md](common.md#rule-3-tightening-takes-effect-at-once-loosening-needs-a-local-accept-on-each-machine)); the runner can only be one of hippo's templates | Same, plus a pinned file's change needs `hippo-secrets accept` on each machine | Same |
| **The model provider** | Gets whatever reached the session | Gets less: the value is never in the session in normal work | Same as helper |
| **A stolen laptop or backup** of `$HIPPO_HOME` | The index (map, no secrets) and the redaction rows (kinds and places, no values) | Plus HMACs and the audit log; values stay in the keychain | Same as helper |
| **Any local program using `scan` as an oracle** | n/a | Whole-value guesses of 12+ characters only, rate-limited and audited | Same |

Against an attacker who can already run code as the user, no local secret tool keeps an unlocked secret from being **used**. What it can do is keep secrets from being **copied** silently while locked, and leave a record. The Binance attack ended with a deploy token used from a compromised machine. The control that would have mattered is short-lived, narrowly scoped deploy credentials (StepSecurity's recommendation), which is outside hippo.

## What changes in config, CLI and app

Part 1, v0.5.0 (detection):

- Ingest records redactions; `hippo incidents` gains `secret-to-model` (secret kinds, ranked); Fleet counts it per agent; Hippo.app shows it per agent.
- Model runners' environments are limited ([runners.md](runners.md#environment)).

Part 1, later (references):

- `.hippo/hippo.toml`: `[secrets]` with `runner` (a template name), `[secrets.runner_args]`, `refs_file`, `[secrets.names]`.
- The start block's `secrets` part; `hippo brief --off secrets` leaves it out.
- `doctor`: a `secrets` row (refs file holds only references; runner on `PATH`; loosening waiting).
- The guard's three secrets rules; the names (never values) in Hippo.app's Manage.

Part 2 adds the `hippo-secrets` binary and its commands, `secret-in-output` incidents, the audit view, and exact-match redaction when the helper is present.

## Test plan

- **Redaction rows:** each adapter's redaction fixture also checks a row is written per planted secret, with role and source call, and that no row holds any part of the value.
- **`secret-to-model`:** synthetic sessions where a fake key arrives through `cat .env`, a pasted message and `printenv`; the incident names the source and the fix.
- **Detection noise:** an email, a phone number, a `password=…` pair and `AKIAIOSFODNN7EXAMPLE` in a tool result raise no incident; a PEM key and a live-format provider key raise high ones.
- **Re-ingest:** sessions ingested before the upgrade have no rows until `ingest --full`; the incident view says "counted since".
- **References:** a refs file with a literal value fails `doctor` without printing it; reference schemes are not redacted, and `op://` followed by a value with spaces or `@` is.
- **Runner templates:** a `runner` that is not a template name, or an argument with `;`, `$(` or a path outside the workspace, leaves the `secrets` part out and `context` still works; a changed runner waits for accept.
- **Budget:** a start block whose required notes fill the budget still compiles, without or with a shortened `secrets` part.
- **Runner environment:** tested in [runners.md](runners.md#test-plan).
- **Helper (part 2):** `set` refuses a non-terminal stdin; `run` refuses `sh -c`, `env`, `$NAME` in arguments and commands outside `use`; output masking for plain, base64, URL and hex forms, and a recorded known miss for a reversed value; `import` leaves no backup and warns on a file git tracked; audit HMAC chain detects an edited line; `scan` replaces planted values at token boundaries and nothing for short ones, refuses past its hourly limit, and audits each call; a `use` entry whose pinned `package.json` changed is refused until `accept`. Keychain tests run on macOS CI with a temporary keychain, never the login keychain.
- **Process boundary:** a test that `hippo` (not the helper) has no keychain symbols linked, checked on the release binary.

## Rollout

Part of the joint plan in [common.md](common.md#release-plan).

1. **v0.5.0, secrets detection**, after `$HIPPO_HOME` permissions ([common.md, rule 4](common.md#rule-4-the-hardening-gate)): redaction rows for every adapter, `secret-to-model` with secret kinds only, confidence and the example-key list, and the "counted since" note. Limited runner environments ship in the same release as runners step 1.
2. **Later, once the guard runs warn-only (v0.6.0) and the accept machinery exists:** `[secrets]` with runner templates, the start-block part, `doctor`'s row, and the guard's three rules.
3. **Gate:** signed and notarised releases ([guard.md](guard.md#hippo-as-a-target)). Not a feature of this design.
4. **Later, only after the gate and only if wanted:** `hippo-secrets` on macOS, with `set`, `import`, `run` (with pinned `use` entries), `list`, `remove`, `audit` and `scan`. `unlock` and Linux after.

## Alternatives

- **A store inside `hippo`.** Rejected above: it puts the keys in the process with the largest attack surface on the machine.
- **An MCP server that runs commands with secrets for the model.** Rejected: the model chooses the command, and a scrub of the output does not stop it reading the value.
- **Depend on ZVault.** It is incomplete, and its local mode needs a server.
- **Pick one external runner and require it.** Rejected: teams already have one. `[secrets] runner` takes any of the common ones by template.
- **A free-form runner string.** The first draft of this design. It goes into every agent's start block, so it is a command-injection channel for anyone with push access. Replaced by templates with checked arguments.
- **Encrypted values committed to git.** Rejected for the minimum version: rotation on every membership change, and a long-lived ciphertext in every clone.

## Open questions

1. Should `secret-to-model` count secrets in the **assistant's** own turns (the model wrote a value it had seen earlier)? They are already at the provider; counting them twice inflates the view. This draft counts the first arrival only.
2. Can the minimum version offer Touch ID per use without an app bundle? If not, is Hippo.app the right host for the helper on macOS?
3. Should agents' own environment variables be the source of `agent` and `session` in the audit log, given they can be forged, or should the guard hook pass them?
4. Is a short-lived token issued per `run` (for services that support it) in scope for the helper, or always the secret manager's job? This draft says the latter.

# Troubleshooting with hippo doctor

Run `hippo doctor` first when something looks wrong. It prints one row per check. This is a new setup on macOS with notes and a few sessions:

```text
check | status | detail
config | ok | /home/you/.hippo/config.toml; workspace /home/you/work
notes_dir | ok | /home/you/work/notes
linter | warn | 9 notes; 0 errors, 5 warnings (2 missing frontmatter, 0 bad frontmatter, 2 dangling links, 1 overdue)
index | ok | last ingest 2026-11-08T08:37:26.694814Z: 3 files seen, 0 indexed
parse failures | ok | none
unindexed files | ok | 3 transcript files, all indexed
sample tasks | ok | none configured
last report | ok | no dreaming report yet
last success | ok | not set up yet: no dreaming run yet; hippo dream runs one (it calls the model)
receipts | ok | no applied receipt
snapshots | ok | none
proposals | ok | 0 proposals, each with a receipt
launchd | ok | not set up yet: no schedule, so the index updates when you run hippo or hippo ingest; hippo schedule install ingests every 10 minutes and dreams at 03:30
sessions | ok | 1 declared sessions ended in the last 7 days: 0 ended unspecified, 0 of them ran a session-end command, 0 others edited notes their transcripts show, 0 left no trace
session records | ok | 4 records
model | ok | not checked (hippo doctor --model sends one word through claude)
```

Status is `ok`, `warn` or `problem`. `doctor` exits 1 if any row is a `problem`, and 0 otherwise. A `warn` never changes the exit code. A healthy new setup passes: no schedule and no dreaming run yet are `ok` rows whose detail starts `not set up yet:`. If the configuration cannot be loaded, including before setup (`hippo is not set up yet`; run `hippo`), the report is that one `config` row.

`doctor` makes no model call unless you run `hippo doctor --model`, which sends one word through your `model_command` and fills the `model` row with the result. A nightly check should pass `--model`.

## The checks

| Check | Problem when | Warning when |
|---|---|---|
| `config` | `config.toml` or `hippo.toml` fails to load | |
| `notes_dir` | the directory is missing | |
| `linter` | a note has frontmatter that is not valid YAML | any other finding: missing or bad frontmatter, dangling link, overdue note |
| `index` | the index is missing, `ingest-state.json` is missing, or the last ingest is older than three ingest intervals while the ingest job is installed (macOS). Without the job it is `ok` and says the index updates when you run `hippo` or `hippo ingest` | off macOS, an index older than three intervals; the detail names `hippo schedule` |
| `parse failures` | | a transcript did not parse; it is named |
| `unindexed files` | | transcript files exist that the index does not hold: new since the last ingest, or sharing a session id with another file |
| `sample tasks` | a task in `sample_tasks` needs more than `budget_bytes` | |
| `last report` | the newest dreaming report starts with `status: failed` | |
| `last success` | no success is recorded once a report exists, or `last_success` is older than 36 hours while the dream job is installed | `last_success` is older than 36 hours and the dream job is not installed. No dreaming run yet, or only dry runs, is `ok` (`not set up yet:`) |
| `receipts` | a dreamed note matches neither hash of its newest applied receipt, within 24 hours of the change | the same mismatch found later than 24 hours after the change |
| `snapshots` | a snapshot directory has no receipt (reports its total size) | |
| `proposals` | | a proposal has no receipt, meaning its review failed |
| `launchd` | | one job file is missing from `~/Library/LaunchAgents` (macOS only). Neither installed is `ok` (`not set up yet:`) |
| `sessions` | | more than half of the declared sessions that ended in the last 7 days left no trace: they ended `unspecified` (no notes, handoff or reason given to hippo) and their transcripts show neither a note edit nor a session-end command run in a shell (such as your own tool's `session-end`) |
| `session records` | | a record cannot be read, an indexed session has no record (the detail names `hippo ingest --full`), or a derived record has no indexed session |
| `model` | | `model_command` is empty, its program is not on `PATH` (then it is not run), or, with `--model`, the run failed. Without `--model` the row is `ok` and says `not checked` |

## Common problems

**`launchd | warn`.** One of the two job files is missing. On a Mac, run `hippo schedule install`. On another system this check does not apply; `hippo schedule install` prints cron and systemd lines instead, see [Scheduling](scheduling.md).

**`index | problem`: `no index; run hippo ingest`, or a stale index.** Run `hippo ingest`. If scheduled ingests stopped, look at `$HIPPO_HOME/logs/dev.hippo.ingest.log`. On a Mac with the workspace or transcripts on an external volume, a launchd job may be waiting on the "removable volume" prompt; see [Scheduling](scheduling.md#the-removable-volume-prompt).

**`index` fails with a message about the index format.** The index is of a format hippo cannot read. The message names the fix, `hippo ingest --full`, which rebuilds the index from the transcripts.

**`sessions | problem`.** Agents are ending sessions without notes, a handoff or a reason. The `session-end` hook alone always records `unspecified`. Ask agents to run `hippo session end --handoff <file>` or `--no-change <reason>` before they finish, in your own instruction files.

**`last report | problem` or `last success | problem`.** The nightly dreaming run failed or has not run. Read the newest `.hippo/reports/<date>-dream.md`: its second line is the error. `hippo context` also prints a `warning:` line at the top of every context block when dreaming is overdue or failed, so agents see it too. A failed run leaves the window at the failed day, so the next run retries it. Fix the cause, often a model that is signed out or rate-limited, and run `hippo dream`. See `$HIPPO_HOME/logs/dev.hippo.dream.log` for scheduled runs.

**`model | warn`.** hippo could not get a reply from `model_command`. The row, like a failed `hippo dream`, shows the model's own message, for example `Not logged in · Please run /login`. When the program is not on `PATH` it says so and names `hippo config set model_command '<JSON array>' --yes`. A common cause is that `claude` is not signed in for the profile hippo runs under; set `CLAUDE_CONFIG_DIR` and re-run `hippo schedule install`. Run `hippo doctor --model` to check again.

**`receipts | problem`.** A note that dreaming changed less than 24 hours ago matches neither its text before the change nor its text after. Someone edited it right after. Look at the note, and at the receipt named in `.hippo/receipts/`. If the change is wrong, `hippo revert <receipt> --force`.

**`snapshots | problem`.** A snapshot directory exists without a receipt, which happens if hippo stopped between copying the snapshot and writing the receipt. Look in `.hippo/snapshots/` and check the note it names.

**`context over budget`.** The entry note's opening section, the required notes, the session line and any warning exceed `budget_bytes`. Shorten the entry note's text before its first `## ` heading, remove `required_notes`, or raise the budget. Long context makes instructions harder for agents to follow, so shortening is the better fix.

**`hippo recall` finds nothing for a paraphrase.** Search matches words in order, not meaning. Try a distinctive word or phrase from the turn. With `--raw` you can use FTS5 syntax such as `retry OR backoff`.

**A hook seems to do nothing.** Hooks print errors to stderr and exit 0, so a failure is quiet. Run the hook by hand to see it:

```sh
echo '{"session_id":"test-1"}' | hippo hook session-start
```

If it prints `hippo hook: <error>`, the error says what is wrong: no `config.toml` (run `hippo`), no `.hippo/hippo.toml` in the workspace, or hook input without a `session_id`. The hook runs under Claude Code's environment, so `hippo` must be on the `PATH` it uses.

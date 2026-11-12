# Fixtures

Everything here is synthetic: invented for these tests, and copied from no real workspace, transcript, note, key or person. Keep it that way. Never add anything taken from a real session or notes repository, even redacted.

## Layout

| Path | What it is | Used by |
|---|---|---|
| `notes/` | A small notes directory: frontmatter variants, links that resolve and dangle, an overdue note, a CRLF note | `notes.rs` tests |
| `records/` | The seven `HIPPOCAMP_V1` records | `records.rs` tests |
| `redaction/` | `leaks.txt`, `expected.txt` and `code.txt` | `redact.rs` tests, the transcript fixtures |
| `claude-code/` | Claude Code transcripts: session `stub-1` and one nested sub-agent transcript | adapter and ingest tests |
| `codex/` | One Codex rollout | adapter and ingest tests |
| `eval/` | A synthetic eval set: notes, transcripts and the questions and answers of `set.toml` | `hippocamp eval`, `crates/hippocamp/tests/eval.rs` |
| `feed/` | The feed's registry fixtures: two index files and the raw `theme.css` files (see below) | `hippocamp-feed`'s `registry.rs` tests and, later, `tests/registry.rs` |
| `feed/api/` | Golden files of the read-only API: `posts.json` (the front list) and `post.json` (the post of `glossary-terms`), written from the actual output with `HIPPOCAMP_UPDATE_GOLDEN=1` and checked by hand; the server's address is written `https://feed.example` | `hippocamp-feed`'s `tests/api.rs` |
| `stub-model/` | `model.sh`, a stand-in model and reviewer that print canned replies | `crates/hippocamp/tests/dream_stub_model.rs` |

## Transcripts

Every `*.jsonl` fixture begins with the line `{"synthetic": true}`, which both parsers skip. Tests point `transcript_roots` at `fixtures/claude-code` and `fixtures/codex` (not `fixtures/eval`, whose transcripts belong to the eval set), so the layout mirrors the real roots:

- `claude-code/` stands for a project directory under `~/.claude/projects/`. `claude-code/synthetic.jsonl` is the top-level transcript of session `stub-1`; `claude-code/stub-1/subagents/agent-a1.jsonl` is a nested sub-agent transcript, session `stub-1-agent-a1`.
- `codex/rollout-synthetic.jsonl` stands for a rollout under `~/.codex/sessions/`. Its name starts with `rollout-`: that is how the Codex adapter claims it and how the Claude Code adapter leaves it alone.

Between them they carry every line of `redaction/leaks.txt` in text blocks, tool inputs and tool outputs, so the ingest tests can check that no leak reaches the index.

Both parsers skip, with a warning, a line that is not valid JSON. That includes a line holding a lone UTF-16 surrogate escape, such as `\ud83d` without its pair, which `serde_json` rejects; that line's turns are not indexed. This is accepted for v1.

## Records

The seven files in `records/` are the record formats written out in full, with each hash a real SHA-256 value. The hashes agree across files: `before_hash` is the SHA-256 of a synthetic note kept in the `records.rs` tests, `after_hash` is that note with the proposal applied, the receipts' `proposal_hash` is the SHA-256 of `proposal-replace.json`, and the revert receipt's hashes run the other way.

## Redaction corpora

- `leaks.txt` holds one fake secret per line. Lines 1 to 15 follow the pattern table in `redact.rs`, one line per expression in table order, each written `<kind> <fake secret>`. Line 16 is a `kv` secret inside a JSON string, its quotes escaped the way compact JSON writes them; line 17 is a `kv` secret as a quoted JSON key and value.
- `expected.txt`: line *n* is line *n* of `leaks.txt` after redaction.
- `code.txt` is code that only looks sensitive and must redact to itself. It must contain at least `token: string`, `tokens: 15`, `const apiKey = process.env.API_KEY`, `headers.get("Authorization")` and a 10-digit number starting with 0 to 5.

Fake secrets have obviously fake bodies (`FAKEfake…`, `example.com`) that still match their patterns.

## Codex mapping

The Codex adapter follows a mapping of Codex's rollout format. The field names were checked against a real rollout under `~/.codex/sessions/`, reading record types and key names only; no content was copied.

- Confirmed: every line's top-level `timestamp`, `type` and `payload`; `session_meta` with `payload.id` and `payload.timestamp`; `response_item` payloads of type `message` with `role` (`user`, `developer`) and `content[]` items `{"type": "input_text", "text": …}`; other record types such as `event_msg` and `turn_context`, which the adapter skips.
- Not confirmed, because that rollout has none: assistant messages (assumed `output_text` content items), `function_call` (`name`, `arguments`, `call_id`), `function_call_output` (`call_id`, `output`) and `reasoning`. These follow the OpenAI Responses API schema. Check them against a rollout that contains tool calls, and record the result here.
- Also unconfirmed: `custom_tool_call` and `local_shell_call` are read like `function_call` when the payload has a `name` (with `arguments`, or else `input`), and `custom_tool_call_output` like `function_call_output` when it has an `output`; any other shape of these is skipped. The fixture keeps to the confirmed shapes and has none of them.

## Stub model

`stub-model/model.sh` stands in for `model_command` in the dreaming test. It reads the prompt on stdin, ignores its arguments, and prints a JSON object whose `result` is the reply, as `claude -p --output-format json` does. With `HIPPOCAMP_STUB_REVIEW` set it is a reviewer and replies `{"verdict": …, "notes": "stub"}` with `HIPPOCAMP_STUB_VERDICT`. Otherwise it replies with the proposal of `HIPPOCAMP_STUB_SCENARIO`:

- `replace` changes `other notes link to` in `notes/Topic A.md` to the text of turn `stub-1` 1;
- `outside` names `../etc/passwd`;
- `dup` replaces `notes`, which occurs twice in that note;
- `unparsable` replies `not json`;
- `eval` is for `hippocamp eval --with-model` (as the pass model, not the reviewer). It reads the prompt and, when the prompt lists `notes/Backups.md`, replies with one `replace` of that note's planted error ("Nightly backups are kept for 14 days." becomes "Backups are now kept for 30 days after the retention change."), quoting turn 7 of session `eval-01` in `eval/transcripts`. For every other note it replies `[]`;
- `demo` and `lessons` are for `app/scripts/demo-home.sh`: the demo's dreaming runs and its lessons pass (drafter and reviewer), each reading the prompt to choose a reply.

Every proposal but `eval`'s cites turn `stub-1` 1, the assistant line "I'll ask a sub-agent where the sender sleeps between retries." The file must stay executable.

## Feed registry

All synthetic: invented owners, repositories, commits and hashes.

- `feed/registry-index.json` is a registry index (`HIPPOCAMP_V1`) of fifteen entries, with extra fields (`generated`, `counts`, `review.verdict`, the passes' `per_note`, `desktop_theme_format`) that the feed ignores. Ten pass the entry rules and are listed: `glossary-terms` and `decision-log` (owner `ada`, id 1001); `review-dates` (owner `grace`, id 1002, maintainers `grace` and `linus`); `stale-links` and `handoff-check` (first-party owner `hippocamp-demo`, id 9001); and five theme packs by `grace`: `night-ink` (its web theme passes), `desk-only` (a desktop theme only), `future-theme` (`theme_contract` 2), `bad-hash` (its `theme.css` does not match the index's `sha256`) and `bad-rules` (its `theme.css` has a selector other than `:root`). Five fail: `copyleft-notes` (licence `GPL-3.0-only`), `-oops` (a name starting with a hyphen), `short-hash` (a 63-digit `pack_hash`), and two entries named `twin`, which both fail.
- `feed/registry-index-2.json` is the same index with `night-ink` at version 1.1.0, a new commit and `pack_hash`, whose `theme.css` hashes correctly but fails the theme rules (text and background contrast).
- `feed/raw/<owner>/<repository>/<commit>/<path>/theme.css` are the stylesheets at the paths `FEED_RAW_URL` uses, for `night-ink` (1.0.0 and 1.1.0), `bad-hash` and `bad-rules`. `desk-only` and `future-theme` have none, because the feed never fetches them.

## Adding a fixture

1. Invent the content. Never paste from a real transcript, note or key, even redacted.
2. Start every `*.jsonl` file with `{"synthetic": true}`.
3. Keep `leaks.txt` and `expected.txt` line for line, and keep `code.txt` redacting to itself.
4. A new note in `notes/` can change the exact lint counts the `notes.rs` tests expect; keep it lint-clean or update the counts.
5. List the new fixture in this file.

# Eval set

This set is invented for hippo's eval and copied from nothing real: no real project, transcript, note, person, company or key. The subject is "Tern", an imaginary self-hosted photo library. Keep it that way.

## Layout

- `set.toml`: entry note, 20 context questions, 20 recall questions, and the `[dreaming]` answer key: the clock, 7 planted errors and 8 kept claims.
- `notes/`: 16 engineering notes (the workspace's notes directory).
- `sessions/`: eight declared session records, `eval-01.json` to `eval-08.json`, whose `notes_touched` make `verify` take the notes with planted errors.
- `transcripts/claude-code/`: eight sessions, `eval-01.jsonl` to `eval-08.jsonl`, one session each.
- `transcripts/codex/`: one rollout (CI dependency cache), session id `codex-01900000-e7a1-7000-8000-000000000001`.

Every `.jsonl` file begins with `{"synthetic": true}`.

## Authoring rules

- Invent everything. Never take text from a real workspace, transcript or person, even redacted.
- Write a question's answer before running hippo on it, by reading the notes as a person would, never from hippo's matching rules. Never change an answer to match hippo's output. If you change an answer, give the reason in the pull request.
- Prefer adding questions to editing them. An edit changes what the numbers mean, so the pull request says so and the pinned numbers change with it.
- Put nothing into the set that redaction would change; eval refuses a set with such text.
- Make each note the answer key names a candidate in exactly one chunk of the dreaming window: touch it from one session, and keep that session's turns inside one chunk.
- Make some clean notes candidates too, and give them kept claims. Otherwise "clean notes changed" could only ever be zero. This set has 2 such notes.
- For a `findable` planted error, write one short turn that states the corrected fact, contains one of its `fixed` strings, and contains the note's title, one of its headings or one of its `sources` paths as an exact phrase. Keep the stale sentence short too: a correction replaces it with the whole turn, within `replace`'s limits.
- Give each planted error the kind that describes it. Add `unfindable` and `long` errors on purpose: real evidence is often long and rarely names the note.
- For a new quote question (`verbatim`, `tool`, `codex`), prefer a phrase whose words also occur in other turns, so that ranking is tested; in this set every quoted phrase occurs in one turn only.
- Cover topics no question asks about, so that ranking has notes to rank.

## Recall kinds

- `verbatim` (10): a 3 to 8 word phrase copied exactly from one turn's text.
- `tool` (4): a phrase copied exactly from a tool_use input or a tool_result.
- `codex` (2): a phrase copied exactly from the Codex rollout.
- `paraphrase` (4): the meaning of one specific turn in other words.

Ordinals count turns from 0 within a session, in file order: a string `content`, each `text` block, each `tool_use` block, each `tool_result` block. `thinking` blocks and other records do not count. For Codex: each `input_text` / `output_text` item of a user or assistant message, each `function_call` and each `function_call_output`.

## Planted errors (stale claims a later session contradicts)

The `[dreaming]` table of `set.toml` scores these, and the records in `sessions/` make `verify` take their notes.

- E1 `notes/Backups.md`, "kept for 14 days": eval-01 turn 7 says "kept for 30 days". The turn contains the heading "Retention". Eval-01 turn 1 still says 14 days (old state, on purpose).
- E2 `notes/Thumbnail pipeline.md`, "256 and 1024 pixels": eval-02 turn 6 says "320 and 1280 pixels". The turn contains the title "thumbnail pipeline".
- E3 `notes/Upload API.md`, "limit is 50 MB per file": eval-03 turn 7 says "limit is 200 MB per file". The turn contains the title "upload API". Eval-03 turn 0 asks "is the upload limit still 50 MB?" first (trap).
- E4 `notes/Database migrations.md`, "Migrations run automatically on deploy": eval-04 turn 8 says "run them by hand". It calls them "schema changes" and contains neither the title nor a heading, by design; the session never uses the word "migration".
- E5 `notes/Auth sessions.md`, "last 7 days": eval-05 turn 7 says "24 hours". The turn contains the heading "Lifetime".
- E6 `notes/Release process.md`, "every second Tuesday": eval-06 turn 6 says "every Thursday". The turn contains the heading "Schedule".
- E7 `notes/Release process.md`, "soak on the beta channel for three days": the last turn of eval-06 (turn 12) says "soak on the beta channel for five days". Its evidence names the note's topic but is too long to quote whole within `replace`'s limit (old plus new text at most half the note's size), so a correction has to quote a shorter span or fail validation.

Each planted error has a `kind` in `set.toml`: `findable` (the evidence turn holds the note's title, a heading or a `sources` path, so `verify`'s search gathers it, and is short enough to quote), `unfindable` (the turn holds neither, E4) and `long` (the evidence turn is too long to quote, E7). The report counts corrections per kind.

## Kept claims (correct; must survive)

- K1 Backups, encrypted with the repository key: confirmed by eval-01 turn 9.
- K2 Thumbnail pipeline, stored as WebP: confirmed by eval-02 turn 8.
- K3 Upload API, uploads over 8 MB use chunked upload: confirmed by eval-03 turn 9.
- K4 Database migrations, every migration has a down step: confirmed by eval-04 turn 10, worded as "schema change".
- K5 Auth sessions, two-factor login uses TOTP codes: confirmed by eval-05 turn 9.
- K6 Release process, every release gets a changelog entry: confirmed by eval-06 turn 8.
- K7 Sharing links, share links expire after 30 days by default: eval-07 proposes 7 days and keeps 30 (trap; turns 0 to 6).
- K8 Search index, rebuilt with `tern reindex`: confirmed by eval-08 turn 5.

## Warning

The stub model's eval scenario quotes eval-01 turn 7. Editing a transcript, including adding or removing a turn, can break that test and the answer key above.

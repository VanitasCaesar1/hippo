# One brain for a team

Status: planned for v0.6.0 and later; nothing here ships in v0.5.0 ([rollout](#rollout)). Revised 12 November 2026. Shared rules (version floor, local accept, hardening gate, release plan) are in [common.md](common.md).

## The problem

A team can share one hippo workspace today, through git ([Sharing a brain in a team](../site/src/sharing.md)). The notes, the packs, proposals, receipts and reviews are committed; session records, dream state, reports and snapshots stay with each person; `hippo init` writes the ignore rules, and a test (`crates/hippo/tests/team.rs`) runs two people through clone, dream, commit and pull.

What a team still cannot do:

1. **See each other's lessons.** The Inbox lives in `$HIPPO_HOME/lessons/`, which is per machine. When Alice corrects her agent ("staging only, never production"), the proposed rule waits in Alice's Inbox. Bob's agents make the same mistake until Alice keeps the rule and pushes.
2. **Avoid proposing the same thing twice.** Bob corrects his agent the same way. His lessons pass drafts the same rule, and if both keep it the note gets two bullets that say the same thing.
3. **Agree on keep and dismiss.** A dismissal is a field in a file under one person's `$HIPPO_HOME`. A teammate's scan raises the same lesson again.
4. **Avoid conflicting changes.** Two people's nightly dreams can both propose a `replace` to the same paragraph of a note, and git reports a conflict at the next pull.
5. **Start a new brain from a good shape.** A team that likes another project's layout can only copy it by hand, content included.

Problems 2 and 3 cost the most today and need the least shared text: a lesson's identity and the team's decision on it. That is the first version. Quotes, the team dreamer and templates come later, each on its own trigger.

## Goals

- A **shared Inbox** of lesson identities and decisions: one entry per lesson however many people saw it; keep and dismiss travel with the repository and are honoured by everyone's scans.
- Nothing a person said enters the repository without that person's consent; transcripts never leave a machine.
- Lessons from other projects never enter a team workspace.
- Later and optional: evidence quotes, a **team dreamer**, **brain templates**.

## Non-goals

- Sharing transcripts, session records, task text or the index.
- Accounts, roles or permissions inside hippo. Who may push is git's business.
- Real-time collaboration on notes.
- Merging two people's indexes.
- Obsidian compatibility work (case-insensitive links, `aliases`, embeds). It touches link resolution for everyone, not only teams, and gets its own design. This design only commits to markers that Obsidian hides and to never writing inside `.obsidian/`.

## What is shared today, and what this adds

| Path | Today | With this design |
|---|---|---|
| Notes, `.hippo/hippo.toml`, `.hippo/packs/` | shared | shared |
| `.hippo/proposals/`, `receipts/`, `reviews/` | shared | shared |
| `.hippo/inbox/` | does not exist | **shared**: lesson identities and decisions ([below](#the-shared-inbox)) |
| `.hippo/sessions/`, `reports/`, `snapshots/`, `dream-state.json`, session log | per person | per person |
| `$HIPPO_HOME/lessons/` | the Inbox | the person's own drafts and their state; the Inbox for lessons from other projects |

## Two ways to move what is shared

**A. Git only, improved.** Everything shared is a file in the workspace; people pull and push as they do now. hippo writes the files and reads what arrived. It never commits, pushes or fetches.

**B. `hippo serve`, an optional self-hosted sync service.** A small server holds the shared Inbox and decisions; every member's hippo talks to it over HTTPS. Notes stay in git.

| | A. Git only | B. `hippo serve` |
|---|---|---|
| What leaves a machine | Lesson identities and decisions: the same kind of derived text already committed today | The same, sent to the team's server |
| Transcripts | Never | Never |
| hippo's network use | None of its own | An HTTP client and a server; "hippo makes no network calls" stops being true |
| Auth | Repository access | A token per member, TLS, keychain storage |
| Latency | The next pull | Seconds |
| Conflicts | Avoided by file layout; the few left are git conflicts | The server orders writes |
| Cost | Some files in the repository | A service to run, back up, upgrade and secure |
| Who can run it | Any team that shares a repository | A team with somewhere to host a service |

**Recommendation: A.** B is better only on latency, and buys it with a server that would be the most sensitive thing a team runs next to its code.

**What would trigger B:** repeated duplicate work caused by pull latency (two people keeping the same lesson within an hour, as `doctor`'s team row will count); members who should decide on the Inbox without push access; or notes that do not live in git. If B is built, it is a separate binary (`hippo-serve`) speaking A's record formats.

The rest of this document designs A.

## Design (option A)

### Who is who

A person's hippo needs a short, stable name to write into shared files. The first command that needs one (`hippo lessons share`, or the app's first shared keep) asks for a **handle**: letters, digits, `-` and `_`, at most 32 characters, because it becomes part of file names. It suggests the local part of `git config user.email` when that fits the rule, never `user.name`, which is often a full name with spaces. It is stored as `member` in `$HIPPO_HOME/config.toml`; `hippo config set member <handle>` changes it.

A member name is a label, not an identity. Anyone with push access can write a file under any name. Git's commit author is the evidence of who pushed a file, and hippo shows it ([below](#what-a-teammate-sees)).

### Consent

Turning on the shared Inbox takes two settings, one per workspace and one per person:

```toml
# .hippo/hippo.toml (shared)
[team]
inbox = "shared"      # "local" (default): today's behaviour
```

```toml
# $HIPPO_HOME/config.toml (this person only)
share_lessons = "rules"   # "off" (default) | "rules" | "rules+quotes" (later)
```

- The workspace flag says the team wants a shared Inbox. It is a [loosening](common.md#which-direction-is-loosening): it raises `min_hippo_version` to 0.6.0 and waits for a local accept on each machine.
- The person's setting says what of theirs may be written. With `off`, the person still reads the shared Inbox, records decisions and skips drafting lessons the team already has; their own lessons stay local.
- The first time a scan would write a shared file for this person, it does not. It prints (and the app shows) the file it would write, and the person presses Share in the app or runs `hippo lessons share`, which, like `hippo accept`, needs a terminal and refuses when an agent marker is set ([common.md, rule 3](common.md#rule-3-tightening-takes-effect-at-once-loosening-needs-a-local-accept-on-each-machine)). That answer is recorded as `share_lessons = "rules"`. A nightly run never takes this step on its own.
- An agent's `hippo lessons share` or edit of `share_lessons` is blocked by the guard's self-protection rules where the guard is installed ([guard.md](guard.md#self-protection)); elsewhere the terminal and marker checks are the only bar, which is not a boundary.

### The shared Inbox

With both settings on, the lessons pass runs on each person's machine as today: candidates from their own index, a draft, a fresh-context review. Then, for a confirmed **correction** lesson **whose every evidence session ran with its working directory inside the workspace or one of its git worktrees**, it writes a shared file as well as the local one.

The test is narrower than the one that decides which sessions get a record (that one also admits a session that only read or cited a note, [sharing.md](../site/src/sharing.md)). A session in another repository that cited one of the team's notes must not bring its corrections in. A lesson with any evidence outside the workspace stays in the person's local Inbox.

Failure lessons stay local in the first version: their signatures carry paths and command lines from a person's machine.

**Layout.** One directory per lesson, named by a ULID minted when the lesson is first shared; one file per person and kind inside it. Nobody edits a file another person wrote, and no file name depends on the lesson's text.

```text
.hippo/inbox/
  L-01JC7Q…/
    lesson-alice.json             Alice's wording; written once, never edited
    lesson-bob.json               Bob's wording, if he drafted it before pulling
    seen-alice.json               Alice's counts; rewritten only by Alice
    seen-bob.json
    decision-bob-01JE….json       Bob kept it
    same-as-L-01JD…-bob.json      Bob linked it to another lesson directory
```

`lesson-<member>.json` holds what the Inbox shows: kind, the match terms, title, rule, target note and section, the reviewer's verdict and the runner that gave it, and `hippo_version`. A reader shows the wording of the lesson's first `lesson-*` file by name order and lists the others as alternative wordings.

`seen-<member>.json` holds counts only: sessions, first and last seen, most in a day, and the agents involved. No quotes and no session ids in the first version. Its owner rewrites it on each scan. A person who uses the same handle on two machines can get a conflict in it; either side is correct, and the next scan recomputes it.

**Matching.** Today a scan matches a new cluster to an existing lesson by kind and key, and for corrections by overlapping terms (`lessons::matches`, `detect::same_terms`: at least two shared terms and a Jaccard index of at least 0.34). A correction's key is its four most frequent terms (`detect.rs`, `cluster_corrections`), so it depends on whose sessions were counted and two people seldom get the same key. The shared Inbox never uses the key as an identity. Before drafting, a scan reads `.hippo/inbox/` and compares each cluster's terms with every shared lesson's terms, in code, with the same `same_terms` rule:

- a match with a decision is not drafted: kept means done, dismissed means closed (it reopens after three more sessions, counted across everyone's `seen` files);
- a match without a decision updates the person's `seen` file and drafts nothing. That saves a model call and gives one entry with "seen by alice, bob";
- only an unmatched cluster is drafted, and if confirmed, gets a new directory.

**Duplicates.** Two people who scan before either pulls can create two directories for one lesson. After the pull, a scan whose cluster matches both shows them as one entry with two wordings and writes nothing. A person links them by keeping one and dismissing the other as `duplicate`, which writes `same-as-<other>-<member>.json` in the dismissed directory. Linked directories form one group: a decision on any member of the group applies to all, so a later scan matching either one is treated as matching the group.

### Decisions

`hippo lessons keep <id>` and `dismiss <id>` on a shared lesson write `decision-<member>-<ulid>.json`:

```json
{"protocol": "HIPPO_V1", "verdict": "keep", "by": "bob", "at": "2026-11-12T10:41:07Z",
 "receipt": "01JE…", "edited": false, "hippo_version": "0.6.0"}
```

`dismiss` takes an optional reason: `wrong`, `duplicate` (with the other directory), `not-for-us`. Keep writes the note exactly as today, through a proposal and a receipt whose reviewer is `human:bob`, so `hippo revert` works on Bob's machine and git is the undo elsewhere.

A lesson's state does not depend on timestamps, which a writer can forge:

- **Kept** if any decision in its group is `keep`. The note is the evidence: the bullet carries the lesson's marker ([below](#markers-in-notes)).
- **Dismissed** if there are only dismissals.
- **Crossed** decisions (a keep and a dismiss) are listed together in the Inbox, "kept by alice; dismissed by bob as wrong", so the two can talk. A dismissal never edits a note; if someone wants the bullet gone, `hippo revert` or an edit removes it as usual.
- **Two keeps.** Keep checks the target note for the group's marker and refuses if it is there. Two keeps made before either pulled both insert, the union merge keeps both bullets, and `hippo doctor`'s `team` row names the duplicate line to delete.

### Markers in notes

A kept lesson's bullet ends today with `*(lesson kept 2026-11-12)*`. It gains an HTML comment carrying the lesson's directory:

```markdown
- **Use the staging database for previews, never production.** Run `npm run preview -- --db staging`. *(lesson kept 2026-11-12)* <!-- hippo:lesson L-01JC7Q… -->
```

Obsidian and GitHub hide HTML comments when they render a note, and an agent reading the raw file sees one short token.

**Existing bullets.** Bullets kept before this release have no marker. `hippo lessons markers` finds each kept local lesson's bullet in its target note (by the text keep inserted, which the lesson record holds), prints the marker it would add, and writes with `--yes` after a snapshot, so `revert` undoes it. A local lesson kept before sharing is shared, with consent, as a new directory with its keep decision already recorded.

### What a teammate sees

`hippo lessons` lists shared and local lessons together:

```text
id        kind        seen                 rule
L-01JC7Q  correction  alice 3, bob 1       Use the staging database for previews, never production
           (shared)   kept by bob 2026-11-12 (pushed by Bob Smith), waiting for your pull to show in the note
91aa03f7  correction  you 2                (local: evidence from another project)
```

Names and verdicts written by others are shown as written and marked as such: the `by` field next to the commit author of the file (from `git log`), and the reviewer verdict as "reviewer: confirmed (as written in lesson-alice.json)". When `by` and the commit author differ, both are shown. Hippo.app's Inbox shows the same, with Keep and Dismiss as in the CLI.

Lesson text in the Inbox is data. It is never sent to a model as instructions; matching compares terms in code.

### Quotes (later)

`share_lessons = "rules+quotes"` adds up to three evidence quotes to the person's `seen` file: at most 160 characters each, of redacted index text, which is what a local lesson record holds today. The first write with quotes is shown before it happens, as above. It ships when a team asks for it: redaction is pattern matching, and a quote can still hold something a person would not share.

### Proposals and the team dreamer (later)

Today each person dreams on their own sessions and applies confirmed changes locally. A team that sees conflicts in notes from dreaming can name one dreamer:

```toml
# .hippo/hippo.toml
[team]
dreamer = "alice"
apply_after_review = true   # the default once a dreamer is named
```

Naming or changing the dreamer is a [loosening](common.md#which-direction-is-loosening), accepted on each machine; turning `apply_after_review` off is too.

- **Everyone else** dreams up to and including review on their own machine. A confirmed change is not applied. It stays a proposal with its receipt (both already shared), and nothing else is written.
- **The dreamer's** `hippo dream` first considers the waiting proposals, oldest first. For each it:
  - requires a human review file for the proposal (`hippo review` already writes one), unless `apply_after_review = false`;
  - re-runs every check hippo's own validator runs before applying any change: the proposal targets a path inside `notes_dir`, never the guard note; Invariant 3's limits on what a pass may write; size limits; and the `before_hash` in the receipt equals the note's current hash. A receipt is evidence of a review, not permission: a forged or agent-written receipt that fails any code check is refused;
  - prints the proposal's committer (from `git log`) beside its author field in the dream report;
  - records the outcome in a **new file**, `.hippo/proposals/<id>/applied-<member>.json` or `stale-<member>.json`. It never edits a proposal or receipt someone else wrote, so the dreamer and the author never touch the same file.
- A `stale` proposal goes back to its author's next dream, which proposes again against the new text.
- The dreamer takes snapshots, so `hippo revert` works on its machine for every applied change. It does not review again with a model: it cannot see the evidence turns, which are in the author's index.

The dreamer is one person's machine, which pulls and pushes like everyone else. Without a dreamer nothing changes from today. This step ships only if a team reports note conflicts from dreaming.

### Handoffs stay per person

Dreaming's `consolidate` pass folds finished sessions' handoffs into notes. Handoffs live in session records, which are per person, and often quote a person's task. This design does not share them.

### Brain templates (later, unscheduled)

A template is the shape of a brain without its content.

```text
hippo init --from <path | https://host/repo[//dir][@ref] | name --registry <index.json>>
hippo template export <dir>
```

```text
template.toml          name, description, version, licence, author
layout/                the notes folder: each note as a skeleton
  START HERE.md        frontmatter keys and headings only
  Decisions/README.md
hippo.toml             only notes_dir, entry_note, required_notes, budget_bytes
packs.toml             the packs to add: registry name or git URL with a pinned ref
```

Rules, checked by `hippo template check` and before `init` writes anything:

- **No settings beyond layout.** A template's `hippo.toml` may hold only `notes_dir`, `entry_note`, `required_notes` and `budget_bytes`. No `[team]`, `[review]`, `[guard]`, `guard_note`, `[secrets]` or any other key: a template must not turn on sharing or loosen a rule in the brain it starts.
- **No content.** A skeleton note may hold frontmatter with hippo's keys (empty or placeholder values), headings, and lines of at most 120 characters starting with `> ` as prompts to the reader: at most three per note, and none in the entry note's opening section, which goes into every start block.
- **No packs inside.** A template names packs; `init --from` runs `hippo add` for each, so each gets the same fresh-context review.
- **No code**, no symbolic links, no hidden files, total size under 256 KiB.
- **Review.** `init --from` reviews the whole template once in a fresh context, as `add` reviews a pack. A rejection stops `init` with nothing written.

`init --from` records the source and its hash in `.hippo/template-origin.json`; there is no "update from template". `hippo template export` writes skeletons of the required and entry notes, the four allowed `hippo.toml` keys and `packs.toml`; it prints every heading it kept and writes nothing without `--yes`. In the registry, a template is a second entry kind (`kind = "template"`) with the same review machinery as packs.

## Data, privacy and security

- **What enters the repository that did not before (v0.6.0):** lesson titles and rules (model-drafted, reviewed), match terms, counts, agent names, member handles, decisions. No quotes and no session ids. Each person consents before their first lesson is written, and the workspace flag is accepted on each machine.
- **Other projects never leak in.** Every evidence session's working directory must be inside the workspace or its worktrees. A test plants a lesson with one session from another repository that cited a team note, and checks it stays local.
- **`git add -A`.** `sharing.md` today calls it safe because `init` ignores everything per person. With a shared Inbox, `.hippo/inbox/` holds text drawn from people's corrections, so the page will say to read `git status` before committing ([common.md](common.md#documentation-each-release-updates)).
- **Member names and verdicts are labels.** hippo shows the commit author beside them.
- **A malicious teammate** with push access can already edit notes directly, which every agent reads. The shared Inbox adds no power beyond that: a lesson file reaches a note only when someone keeps it.
- **Older teammates.** `[team]` raises `min_hippo_version`; a hippo from 0.5.0 on refuses to write with an outdated version, and one older than 0.5.0 behaves as if the shared Inbox did not exist ([common.md, rule 2](common.md#rule-2-security-settings-in-the-shared-hippotoml-need-a-version-floor)).
- **The team dreamer** (later) applies only proposals that pass every code check and, by default, a human review.

## What changes in config, CLI and app

- `config.toml`: `member`, `share_lessons`.
- `.hippo/hippo.toml`: `[team]` with `inbox`; later `dreamer` and `apply_after_review`.
- `hippo init` keeps `.hippo/inbox/` out of both ignore files. `doctor`'s `git` row warns when `[team] inbox = "shared"` but `.hippo/inbox/` is ignored.
- `hippo lessons`, `keep`, `dismiss --reason`, `share`, `markers`.
- `hippo doctor`: a `team` row (duplicate markers, crossed decisions, members below `min_hippo_version`; later, proposals waiting for the dreamer and stale proposals).
- Hippo.app: the Inbox shows who saw each lesson and who decided, with the committer; the Share consent dialog.

## Test plan

Extend `crates/hippo/tests/team.rs`, which runs two people with their own `HOME` and `$HIPPO_HOME` against one bare git remote:

- Alice and Bob each correct their agents the same way (synthetic transcripts with different word frequencies, so their keys differ), scan with a stub model, push and pull. One Inbox entry, seen by both; Bob's second scan makes no draft model call.
- Both scan before pulling: two directories shown as one entry with two wordings; no git conflict. Bob dismisses one as `duplicate`; a later scan matching either is treated as the group.
- Bob keeps, Alice dismisses: the lesson is kept, both decisions are listed, no conflict.
- Both keep before pulling: two bullets after the union merge; `doctor`'s `team` row names the duplicate.
- Consent: with `share_lessons = "off"`, nothing of Carol's is written, and she still skips drafting a lesson the team has. The first share is shown and needs `--yes`.
- Workspace test: a lesson with one session from another repository that cited a team note stays local.
- Version floor: a hippo below `min_hippo_version` refuses `lessons keep` and still serves `context`.
- Markers: `lessons markers` adds markers to bullets kept before the release; `revert` removes them.
- Later steps get their own tests when they ship: dreamer (a proposal with a receipt but no review waits; one that targets the guard note is refused; outcomes are new files), templates (a `[team]` key fails the check; a fourth `> ` line fails).

## Rollout

Part of the joint plan in [common.md](common.md#release-plan).

1. **v0.6.0:** `member` and `share_lessons`; markers on kept bullets and `lessons markers`; the shared Inbox for corrections with lesson identities and decisions, no quotes. Needs v0.5.0's hardening gate and version floor, and v0.6.0's accept machinery.
2. **Later, when a team asks:** quotes (`share_lessons = "rules+quotes"`).
3. **Later, when a team reports note conflicts from dreaming:** the team dreamer.
4. **Later, unscheduled:** templates and the registry entry kind.
5. Option B only on its triggers, as a separate binary.

## Alternatives

- **Share session records or handoffs so one machine dreams for everyone.** Task text and handoffs would enter the repository, and evidence turns would still be missing from the dreamer's index. Rejected.
- **Directory names from a hash of the lesson's key.** The first draft of this design. Keys come from term frequencies and differ between people, so the same lesson got two directories and decisions did not carry. Replaced by ULIDs minted at first share and explicit links.
- **One file per lesson, edited by everyone.** Two people adding evidence conflict in git every time. Rejected.
- **The earliest decision wins, by ULID.** ULIDs carry the writer's clock, which a writer controls. Replaced by a state that does not depend on order.
- **A lock file in the repository for dreaming.** Works only if everyone pulls first, which hippo cannot make happen. Rejected.
- **Quotes on by default with a workspace flag.** A shared file would decide what each person's words do. Replaced by a per-person setting, shown before the first write.
- **Templates as packs.** A pack is kept up to date; a template acts once. Kept separate.

## Open questions

1. Should a template be allowed to carry `required_notes` that do not exist in its layout?
2. Should a later version share failure lessons with their signatures reduced to the error's words, without paths?

# Measuring with hippo eval

`hippo eval` measures how well hippo hands over the right context, finds the right turns, and corrects wrong notes. It runs against a **synthetic eval set**: invented notes, invented transcripts and an answer key written in advance. The repository ships one, `fixtures/eval`, about an imaginary photo library called Tern.

```sh
hippo eval fixtures/eval
```

Run this from a checkout of the repository. The set is not installed with the binary.

Eval works in a temporary directory. It copies the set there, runs `init` and ingest, runs the suites, prints the report and deletes the directory. It writes nothing to your workspace or to `$HIPPO_HOME`. (If eval is killed by a signal, the temporary directory can be left behind in the system's temporary directory.)

## The three suites

**Context.** For each of 20 tasks, eval compiles the context block as `hippo context --task <task>` would, with the default budget, and compares the notes it lists with the notes the answer key expects. Four of the tasks have no matching note and measure noise.

- `found a/b`: the share of expected notes that the block lists.
- `precision a/b`: the share of listed notes that were expected.
- `complete a/b`: tasks whose block lists every expected note.
- `quiet a/b`: tasks with no expected note whose block lists none.

**Recall.** For each of 20 queries, eval runs the search behind `hippo recall` and finds the rank of the first hit that is the expected turn. Ten queries are phrases quoted from one turn (`verbatim`), four quote a tool call or result (`tool`), two quote a Codex rollout (`codex`) and four paraphrase a turn in other words (`paraphrase`). The summary gives `hit@1`, `hit@5` and `hit@20` and the mean reciprocal rank (`mrr`), and counts hits per kind.

**Dreaming.** Runs only with `--with-model`. Eval runs the `verify` pass, not as a dry run, on notes that contain 7 planted errors and 8 correct claims, and scores the notes as they are afterwards:

- `corrected a/b`: planted errors fixed (the stale text gone and a correct replacement present);
- `damaged a/b`: correct claims whose text is gone;
- `lines lost`: correct lines that disappeared from the notes named by the answer key;
- `clean notes changed a/b`: notes without a planted error that were changed anyway.

It then appends the run's own report, so you can read what the model proposed and what the reviewer said.

## Reading the report

The report is Markdown on standard output. It opens with the set's hash, the hippo version, a hash of the core pack, the model and the start time, then a summary:

```text
context: found 15/18, precision 15/25, complete 13/16, quiet 4/4
recall: hit@1 16/20, hit@5 16/20, hit@20 16/20, mrr 0.80 (verbatim 10/10, tool 4/4, codex 2/2, paraphrase 0/4)
dreaming: skipped: needs --with-model
```

These are the numbers for the set at version 0.1.0. They describe this one small set, not hippo in general. Below the summary come a table per suite with each question and its result.

Two things to notice in the example. The paraphrase queries, which share no words with the turn they mean, all miss: hippo's search matches words, not meaning. And the context suite misses tasks whose words appear only in passing in a note's body, such as the iOS crash, whose note is titled "Mobile app": the relevant notes need a strong match, and list fewer notes rather than weak ones. It also never lists the entry note, which the block already shows, so the newcomer's question counts as a miss.

## With a model

```sh
hippo eval fixtures/eval --with-model
```

This reads your `config.toml` and runs your `model_command` (and `reviewer_command_override`, if set) for roughly 10 to 20 model calls. Model-dependent numbers are one sample from a model that does not answer the same way twice. Do not read a difference of one correction as a trend.

## Exit codes

Eval prints the report and exits 1 if a suite failed, for example if the dreaming run could not reach the model. A skipped suite is not a failure. If the set itself is invalid, for instance because an expected note is missing, eval exits 1 with the reason and prints no report.

## Using your own set

A set is a directory with `set.toml`, `notes/`, `transcripts/` and, for dreaming, `sessions/`. The format and the checks eval runs on a set are described in `fixtures/eval/README.md` in the repository. Every file in a set must be synthetic: invented, with no real transcripts, notes, keys or personal data.

//! Dreaming: `hippo dream`'s run, and `revert`.
//!
//! For each day chunk of the window ([`window`]) and each pass, in lexical
//! order of `<pack>/<name>`: gather the inputs ([`inputs`]), render the
//! prompt ([`prompt`]), run the model, check each proposal and write the
//! valid ones ([`validate`]), review each ([`reviewer`]), and apply what is
//! confirmed ([`apply`]). `last_success` advances after each chunk. The
//! report ([`report`]) is written at the end, and also when the run fails:
//! then it says `status: failed`, the window stays at the failed chunk, and
//! [`run`] returns the error, which `hippo dream` turns into exit code 2.
//!
//! A run first ingests, incrementally and in the same process, before the
//! window is computed, so the turns of the last few minutes are covered.
//! When another ingest holds the lock, it goes on with the index as it is
//! and says so in the report.
//!
//! A pass's model call has one fixed line as its system prompt,
//! [`pass_system_prompt`], and the rendered prompt on stdin: transcript
//! text stays out of the system prompt and out of the argument list, whose
//! single-argument limit on Linux (128 KiB) is below the default budget.
//!
//! `--dry-run` writes no proposal, receipt, snapshot, note or
//! `dream-state.json`: of dreaming, only the report. It still ingests and
//! runs the model and the reviewer, and the report lists what they
//! returned.

pub mod apply;
pub mod inputs;
pub mod observed;
pub mod prompt;
pub mod report;
pub mod reviewer;
pub mod sections;
pub mod seen;
pub mod validate;
pub mod window;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use time::{OffsetDateTime, UtcOffset};
use ulid::Ulid;

pub use apply::{revert, revert_state, revert_state_in, RevertState};

use crate::config::{Config, Workspace};
use crate::error::{Error, Result};
use crate::index::Index;
use crate::ingest::{self, IngestSummary};
use crate::model;
use crate::notes::{normalise, Note, Notes};
use crate::packs;
use crate::records::{
    list_proposals, list_receipts, Input, Listed, Pass, Proposal, Receipt, Verdict,
};
use apply::Applied;
use inputs::{ChunkInputs, Gathered, SessionInput};
use report::Report;
use reviewer::ReviewerInput;
use validate::{Checked, Draft};
use window::Chunk;

/// The most parts of one note over budget that a `per_note` pass reads in
/// one chunk.
pub const MAX_PARTS_PER_NOTE: usize = 2;

/// The size a note given whole to a pass that is not `per_note` is cut to,
/// its last sections, when the prompt is over budget.
pub const TAIL_BYTES: usize = 16_384;

/// The system prompt of pass `name`'s model call. The rendered prompt,
/// with the pass's rules and inputs, is the message on stdin.
pub fn pass_system_prompt(name: &str) -> String {
    format!("You are hippo's {name} pass. Follow the rules in the message.")
}

/// A proposal with every receipt that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub proposal_id: Ulid,
    /// The proposal file, or why it could not be read. A receipt whose
    /// proposal file is missing is listed under an entry carrying that error.
    pub proposal: Listed<Proposal>,
    /// Sorted by receipt id.
    pub receipts: Vec<Receipt>,
}

/// Every proposal with its receipts, as [`history`] reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    /// One per proposal id, sorted by id.
    pub changes: Vec<Change>,
    /// Each receipt that does not load, and each proposal file whose name is
    /// not an id, with the error.
    pub unreadable: Vec<(PathBuf, String)>,
}

/// Reads `.hippo/proposals/` and `.hippo/receipts/` into one [`Change`] per
/// proposal file named `<id>.json`, and one per proposal id a receipt names
/// without a file, sorted by proposal id, each with its receipts sorted by
/// id. Fails only when a directory cannot be read.
pub fn history(ws: &Workspace) -> Result<History> {
    let mut changes: BTreeMap<Ulid, Change> = BTreeMap::new();
    let mut unreadable = Vec::new();
    for listed in list_proposals(ws)? {
        let id = listed
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<Ulid>().ok());
        match id {
            Some(id) => {
                changes.insert(
                    id,
                    Change {
                        proposal_id: id,
                        proposal: listed,
                        receipts: Vec::new(),
                    },
                );
            }
            None => unreadable.push((listed.path, "the file name is not a proposal id".to_owned())),
        }
    }
    for listed in list_receipts(ws)? {
        match listed.record {
            Ok(receipt) => {
                let change = changes.entry(receipt.proposal).or_insert_with(|| {
                    let path = validate::path(ws, receipt.proposal);
                    let record = Proposal::load(&path).map_err(|err| err.to_string());
                    Change {
                        proposal_id: receipt.proposal,
                        proposal: Listed { path, record },
                        receipts: Vec::new(),
                    }
                });
                change.receipts.push(receipt);
            }
            Err(err) => unreadable.push((listed.path, err)),
        }
    }
    let mut changes: Vec<Change> = changes.into_values().collect();
    for change in &mut changes {
        change.receipts.sort_by_key(|receipt| receipt.id);
    }
    Ok(History {
        changes,
        unreadable,
    })
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSummary {
    /// The report written.
    pub report: PathBuf,
    /// Day chunks completed.
    pub chunks: usize,
    /// Proposals that passed validation.
    pub proposals: usize,
    pub validation_failures: usize,
    pub confirmed: usize,
    pub rejected: usize,
    /// Changes written to notes.
    pub applied: usize,
}

impl fmt::Display for RunSummary {
    /// `<report>: <n> chunks, <n> proposals, <n> validation failures,
    /// <n> confirmed, <n> rejected, <n> applied`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} chunks, {} proposals, {} validation failures, {} confirmed, {} rejected, {} applied",
            self.report.display(),
            self.chunks,
            self.proposals,
            self.validation_failures,
            self.confirmed,
            self.rejected,
            self.applied
        )
    }
}

/// Runs every pass, or only the one whose file stem is `pass`, over the
/// window. A failure writes the failed report and returns the error.
pub fn run(cfg: &Config, ws: &Workspace, pass: Option<&str>, dry_run: bool) -> Result<RunSummary> {
    run_at(cfg, ws, pass, dry_run, OffsetDateTime::now_utc())
}

pub(crate) fn run_at(
    cfg: &Config,
    ws: &Workspace,
    pass: Option<&str>,
    dry_run: bool,
    now: OffsetDateTime,
) -> Result<RunSummary> {
    let now = now.to_offset(UtcOffset::UTC);
    let mut run = Run {
        cfg,
        ws,
        dry_run,
        report: Report::default(),
        summary: RunSummary::default(),
        log_notes: BTreeSet::new(),
    };
    let outcome = run.all(pass, now);
    if let Err(err) = &outcome {
        run.report.error = Some(err.to_string());
    }
    if let (Some(days), false, true) = (ws.snapshot_keep_days, dry_run, run.summary.applied > 0) {
        match prune_snapshots(ws, days, now) {
            Ok(0) => {}
            Ok(n) => run.report.window.push(format!(
                "snapshots: {n} older than {days} days deleted (snapshot_keep_days)"
            )),
            Err(e) => run
                .report
                .window
                .push(format!("snapshots: pruning failed: {e}")),
        }
    }
    let written = run.report.write(ws, now.date());
    outcome?;
    run.summary.report = written?;
    Ok(run.summary)
}

/// Deletes snapshot files under `.hippo/snapshots/` last modified more than
/// `days` days before `now` and returns how many.
pub fn prune_snapshots(ws: &Workspace, days: u32, now: OffsetDateTime) -> Result<usize> {
    let dir = ws.hippo_dir().join("snapshots");
    if !dir.is_dir() {
        return Ok(0);
    }
    let cutoff = now - time::Duration::days(i64::from(days));
    let mut n = 0;
    for entry in walkdir::WalkDir::new(&dir).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let old = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|t| OffsetDateTime::from(t) < cutoff);
        if old {
            std::fs::remove_file(entry.path()).map_err(|e| Error::io_at(entry.path(), e))?;
            n += 1;
        }
    }
    Ok(n)
}

/// A pass ready to run.
struct LoadedPass {
    /// The pass file's stem.
    name: String,
    pass: Pass,
    /// The prompt file's text.
    template: String,
    /// The reviewer's system prompt: the pass's persona for `core`, else
    /// [`packs::PASS_REVIEWER`].
    persona: String,
    /// The persona file's stem, the receipts' `reviewer`; for a pass
    /// outside `core`, `pass-reviewer/<persona file stem>`.
    reviewer: String,
    /// Whether the pass is in the `core` pack, whose dreaming is unchanged.
    core: bool,
    /// For a pass outside `core`, the persona file's text, which the
    /// reviewer's message carries as criteria; `persona` is then hippo's
    /// [`packs::PASS_REVIEWER`].
    criteria: Option<String>,
}

struct Run<'a> {
    cfg: &'a Config,
    ws: &'a Workspace,
    dry_run: bool,
    report: Report,
    summary: RunSummary,
    /// The workspace's log notes, relative to it,
    /// found when the run starts.
    log_notes: BTreeSet<PathBuf>,
}

impl Run<'_> {
    fn all(&mut self, pass: Option<&str>, now: OffsetDateTime) -> Result<()> {
        if self.dry_run {
            self.report.window.push(report::DRY_RUN_LINE.to_owned());
        }
        let passes = load_passes(self.ws, pass)?;
        self.log_notes =
            crate::log_notes::in_workspace(self.ws, &Notes::scan(&self.ws.notes_root())?);
        let ingested = match ingest::ingest(self.cfg, Some(self.ws), false) {
            Ok(IngestSummary::AlreadyRunning) => {
                "ingest first: another ingest holds the lock, so this run reads the index as it is"
                    .to_owned()
            }
            Ok(summary) => format!("ingest first: {summary}"),
            Err(err) => return Err(about("ingest", err)),
        };
        self.report.inputs.push(ingested);
        let index = Index::open(&self.cfg.home, false)?;
        let chunks = window::chunks(window::last_success(self.ws)?, now);
        if chunks.is_empty() {
            self.report
                .window
                .push(format!("empty: the last success is not before {}", ts(now)));
        }
        let last_ordinals = index.last_ordinals()?;
        let reverted = inputs::reverted_proposals(self.ws)?;
        for chunk in chunks {
            let label = format!("{} to {}", ts(chunk.start), ts(chunk.end));
            if let Err(err) = self.chunk(&index, &passes, &last_ordinals, &reverted, chunk, &label)
            {
                self.report.window.push(format!("{label}: failed"));
                return Err(err);
            }
            if !self.dry_run {
                window::advance(self.ws, chunk.end)?;
            }
            self.summary.chunks += 1;
            self.report.window.push(format!("{label}: done"));
        }
        Ok(())
    }

    fn chunk(
        &mut self,
        index: &Index,
        passes: &[LoadedPass],
        last_ordinals: &std::collections::HashMap<String, u32>,
        reverted: &[Proposal],
        chunk: Chunk,
        label: &str,
    ) -> Result<()> {
        let found = inputs::for_chunk(self.ws, index, last_ordinals, chunk)?;
        if found.sessions.is_empty() {
            self.report
                .inputs
                .push(format!("{label}: no session record has a turn here"));
            return Ok(());
        }
        self.report.inputs.push(format!(
            "{label}: {} sessions; {}",
            found.sessions.len(),
            found.summary()
        ));
        for pass in passes {
            self.pass(index, pass, &found, reverted, label)?;
        }
        Ok(())
    }

    /// One pass over one chunk: once per note for a `per_note` pass, else
    /// once, and not at all when there is no note to change. A pass that
    /// reads `turns` takes the observed notes too; the notes are ranked and
    /// at most `max_notes` are kept. A note over half the
    /// budget is read in parts, or cut to its tail.
    fn pass(
        &mut self,
        index: &Index,
        pass: &LoadedPass,
        found: &ChunkInputs,
        reverted: &[Proposal],
        label: &str,
    ) -> Result<()> {
        let budget = pass.pass.input_budget_bytes;
        let merged;
        let found = if pass.pass.reads.contains(&Input::Turns) {
            merged = found.with_observed();
            &merged
        } else {
            found
        };
        let filtered;
        let found = if pass.pass.note_types.is_some() || pass.pass.note_paths.is_some() {
            filtered = self.filtered(pass, found, label);
            &filtered
        } else {
            found
        };
        if found.notes.is_empty() {
            self.report
                .inputs
                .push(format!("{label} {}: no note to work on", pass.name));
            return Ok(());
        }
        let ranked = found.ranked(&self.log_notes);
        let max = pass.pass.max_notes;
        for (i, note) in ranked.iter().enumerate().skip(max) {
            let log = if self.log_notes.contains(note) {
                ", log note"
            } else {
                ""
            };
            self.report.omissions.push(format!(
                "{label} {}: omitted: note {} (rank {} of {}, over max_notes {max}{log})",
                pass.name,
                note.display(),
                i + 1,
                ranked.len()
            ));
        }
        let kept: Vec<PathBuf> = ranked.into_iter().take(max).collect();
        if pass.pass.per_note {
            for note in &kept {
                let what = format!("{label} {} {}", pass.name, note.display());
                match inputs::for_note(
                    self.ws,
                    index,
                    found,
                    note,
                    &pass.pass.reads,
                    budget,
                    reverted,
                )? {
                    Ok(gathered) if gathered.notes[0].bytes.len() > budget / 2 => {
                        let written = found
                            .seen
                            .get(note)
                            .map(|seen| seen.written.clone())
                            .unwrap_or_default();
                        self.parts(index, pass, found, reverted, gathered, &written, &what)?;
                    }
                    Ok(gathered) => self.call_within(index, pass, &gathered, &what)?,
                    Err(why) => self
                        .report
                        .omissions
                        .push(format!("{what}: skipped: {why}")),
                }
            }
        } else {
            let what = format!("{label} {}", pass.name);
            let mut kept_inputs = found.clone();
            kept_inputs.notes.retain(|note, _| kept.contains(note));
            let mut gathered = inputs::for_all(
                self.ws,
                index,
                &kept_inputs,
                &pass.pass.reads,
                budget,
                reverted,
            )?;
            if !self.fit(pass, found, &mut gathered, &what)? {
                return Ok(());
            }
            self.call(index, pass, &gathered, &what)?;
        }
        Ok(())
    }

    /// Fits a pass that is not `per_note` to its budget, in this order, while the prompt is over budget: the sessions
    /// that touched or cited none of the notes in the prompt are left out;
    /// the largest note given whole and over [`TAIL_BYTES`] is cut to its
    /// last sections; the sessions with nothing to say (no handoff, no notes
    /// touched) are left out, largest line first; the largest note is left
    /// out, and with it the sessions that touched or cited only it. Each
    /// kind of session left out is one Omissions line with its count.
    /// `false`, with a `skipped` line, when no note is left.
    fn fit(
        &mut self,
        pass: &LoadedPass,
        found: &ChunkInputs,
        gathered: &mut Gathered,
        what: &str,
    ) -> Result<bool> {
        let budget = pass.pass.input_budget_bytes;
        let mut unrelated = 0usize;
        let mut silent = 0usize;
        let mut sessions_alone = None;
        let mut dropped = false;
        while over_budget(pass, gathered)?.is_some() {
            let left_out = leave_unrelated(found, gathered);
            if left_out > 0 {
                unrelated += left_out;
                continue;
            }
            let whole = (0..gathered.notes.len())
                .filter(|&i| {
                    gathered.notes[i].excerpt.is_none()
                        && gathered.notes[i].bytes.len() > TAIL_BYTES
                })
                .max_by_key(|&i| gathered.notes[i].bytes.len());
            if let Some(i) = whole {
                let note = &gathered.notes[i];
                let cut = note.excerpted(sections::tail(&note.bytes, TAIL_BYTES));
                self.report.omissions.push(format!(
                    "{what}: excerpted: note {} ({} bytes) to its last sections, {}",
                    cut.path.display(),
                    cut.bytes.len(),
                    cut.excerpt.as_ref().map(|e| e.label()).unwrap_or_default()
                ));
                gathered.notes[i] = cut;
                continue;
            }
            let quiet = (0..gathered.sessions.len())
                .filter(|&i| says_nothing(&gathered.sessions[i]))
                .max_by_key(|&i| inputs::session_line(&gathered.sessions[i]).len());
            if let Some(i) = quiet {
                gathered.sessions.remove(i);
                silent += 1;
                continue;
            }
            if gathered.notes.len() == 1 {
                let without_notes = Gathered {
                    notes: Vec::new(),
                    human_confirmed: Vec::new(),
                    ..gathered.clone()
                };
                sessions_alone = over_budget(pass, &without_notes)?;
            }
            let Some(largest) =
                (0..gathered.notes.len()).max_by_key(|&i| gathered.notes[i].shown().len())
            else {
                break;
            };
            let note = gathered.notes.remove(largest);
            gathered
                .human_confirmed
                .retain(|(path, _)| *path != note.path);
            self.report.omissions.push(format!(
                "{what}: omitted: note {} ({} bytes)",
                note.path.display(),
                note.bytes.len()
            ));
            dropped = true;
            unrelated += leave_unrelated(found, gathered);
        }
        if unrelated > 0 {
            self.report.omissions.push(format!(
                "{what}: omitted: {unrelated} sessions that touched or cited none of its notes"
            ));
        }
        if silent > 0 {
            self.report.omissions.push(format!(
                "{what}: omitted: {silent} sessions with no handoff and no notes touched"
            ));
        }
        if dropped && gathered.notes.is_empty() {
            let why = match sessions_alone {
                Some(size) => format!(
                    "the sessions of its last note alone make a {size}-byte prompt, over {budget}"
                ),
                None => "no note fits the budget".to_owned(),
            };
            self.report
                .omissions
                .push(format!("{what}: skipped: {why}"));
            return Ok(false);
        }
        Ok(true)
    }

    /// A `per_note` call, or the `skipped` line when the note alone makes
    /// the prompt over budget.
    fn call_within(
        &mut self,
        index: &Index,
        pass: &LoadedPass,
        gathered: &Gathered,
        what: &str,
    ) -> Result<()> {
        let budget = pass.pass.input_budget_bytes;
        match over_budget(pass, gathered)? {
            Some(size) => {
                self.report.omissions.push(format!(
                    "{what}: skipped: the note alone makes a {size}-byte prompt, over {budget}"
                ));
                Ok(())
            }
            None => self.call(index, pass, gathered, what),
        }
    }

    /// A note over half the budget, for a `per_note` pass: its parts of at
    /// most a quarter of the budget, of which it reads at most
    /// [`MAX_PARTS_PER_NOTE`] (`sections::chosen`), one call each.
    #[allow(clippy::too_many_arguments)]
    fn parts(
        &mut self,
        index: &Index,
        pass: &LoadedPass,
        found: &ChunkInputs,
        reverted: &[Proposal],
        gathered: Gathered,
        written: &[String],
        what: &str,
    ) -> Result<()> {
        let budget = pass.pass.input_budget_bytes;
        let whole = gathered.notes[0].clone();
        let parts = sections::parts(&whole.bytes, budget / 4);
        let picked = sections::chosen(&whole.bytes, &parts, written, MAX_PARTS_PER_NOTE);
        let labels: Vec<String> = picked
            .iter()
            .map(|&i| sections::Excerpt::of(&whole.bytes, parts[i].clone()).label())
            .collect();
        self.report.omissions.push(format!(
            "{what}: in parts: {} bytes, {} parts of at most {} bytes; reading {}",
            whole.bytes.len(),
            parts.len(),
            budget / 4,
            labels.join(", ")
        ));
        for i in picked {
            let part = whole.excerpted(parts[i].clone());
            let what = format!(
                "{what} ({})",
                part.excerpt.as_ref().map(|e| e.label()).unwrap_or_default()
            );
            match inputs::for_read_note(index, found, part, &pass.pass.reads, budget, reverted)? {
                Ok(gathered) => self.call_within(index, pass, &gathered, &what)?,
                Err(why) => self
                    .report
                    .omissions
                    .push(format!("{what}: skipped: {why}")),
            }
        }
        Ok(())
    }

    /// A copy of `found` with only the notes `pass` selects by its
    /// `note_types` and `note_paths`, and the sessions that touched or cited
    /// one of them. A note that cannot be read is reported and left out.
    fn filtered(&mut self, pass: &LoadedPass, found: &ChunkInputs, label: &str) -> ChunkInputs {
        let notes_dir = normalise(&self.ws.notes_dir);
        let mut kept = ChunkInputs::default();
        for (note, cited_by) in &found.notes {
            let below = note.strip_prefix(&notes_dir).unwrap_or(note);
            let note_type = if pass.pass.note_types.is_some() {
                match Note::read(&self.ws.root.join(note)) {
                    Ok(read) => read.frontmatter.r#type,
                    Err(err) => {
                        self.report.omissions.push(format!(
                            "{label} {} {}: skipped: {err}",
                            pass.name,
                            note.display()
                        ));
                        continue;
                    }
                }
            } else {
                None
            };
            if pass.pass.selects(below, note_type.as_deref()) {
                kept.notes.insert(note.clone(), cited_by.clone());
                if let Some(seen) = found.seen.get(note) {
                    kept.seen.insert(note.clone(), seen.clone());
                }
            }
        }
        kept.sessions = found
            .sessions
            .iter()
            .filter(|session| {
                kept.notes
                    .values()
                    .any(|cited_by| cited_by.contains(&session.record.id))
            })
            .cloned()
            .collect();
        kept
    }

    /// Steps 3 to 7 for one model call.
    fn call(
        &mut self,
        index: &Index,
        pass: &LoadedPass,
        gathered: &Gathered,
        what: &str,
    ) -> Result<()> {
        self.report
            .inputs
            .push(format!("{what}: {}", gathered.counts()));
        let (blocks, turns) = gathered.rendered(&pass.pass.reads);
        let rendered = prompt::render(
            &pass.template,
            &blocks,
            &turns,
            pass.pass.input_budget_bytes,
        )
        .map_err(|err| about(what, err))?;
        for line in rendered.omitted_lines() {
            self.report.omissions.push(format!("{what}: {line}"));
        }
        let reply = model::run(self.cfg, &pass_system_prompt(&pass.name), &rendered.text)
            .map_err(|err| about(what, err))?;
        let Some(items) = validate::first_array(&reply.text) else {
            return Err(Error::Model(format!("{what}: the reply has no JSON array")));
        };
        let prompt_sessions: BTreeSet<String> = gathered
            .sessions
            .iter()
            .map(|session| session.record.id.clone())
            .collect();
        let outside = (!pass.core).then_some(&prompt_sessions);
        for item in items {
            let draft = match serde_json::from_value::<Draft>(item) {
                Ok(draft) => draft,
                Err(err) => {
                    self.invalid(format!("{what}: not a proposal: {err}"));
                    continue;
                }
            };
            match validate::check(self.ws, index, &pass.name, &gathered.notes, &draft, outside)? {
                Checked::Invalid(why) => {
                    self.invalid(format!("{} {}: {why}", pass.name, draft.note))
                }
                Checked::Valid(proposal) => self.review(index, pass, gathered, &proposal)?,
            }
        }
        Ok(())
    }

    /// Steps 5 (the write), 6 and 7 for one valid proposal.
    fn review(
        &mut self,
        index: &Index,
        pass: &LoadedPass,
        gathered: &Gathered,
        proposal: &Proposal,
    ) -> Result<()> {
        if !self.dry_run {
            validate::write(self.ws, proposal)?;
        }
        self.summary.proposals += 1;
        let kind = proposal.kind.as_str();
        self.report.proposals.push(format!(
            "{} {} ({}, {kind})",
            proposal.id,
            proposal.note.display(),
            pass.name
        ));
        let note = gathered
            .notes
            .iter()
            .find(|note| note.path == proposal.note)
            .expect("a valid proposal names one of its prompt's notes");
        let cited: Vec<(String, u32)> = proposal
            .evidence
            .iter()
            .map(|cited| (cited.session.clone(), cited.ordinal))
            .collect();
        let evidence = index.turns(&cited)?;
        let cited_sessions: BTreeSet<&str> = cited.iter().map(|(id, _)| id.as_str()).collect();
        let mut sessions: Vec<&SessionInput> = if pass.core {
            Vec::new()
        } else {
            gathered
                .sessions
                .iter()
                .filter(|session| cited_sessions.contains(session.record.id.as_str()))
                .collect()
        };
        sessions.sort_by(|a, b| a.record.id.cmp(&b.record.id));
        let input = ReviewerInput {
            persona: pass.persona.clone(),
            note: note.for_reviewer(),
            kind: proposal.kind,
            old: proposal.old.clone(),
            new: proposal.new.clone(),
            evidence,
            sessions: sessions.into_iter().map(inputs::session_line).collect(),
            criteria: pass.criteria.clone(),
        };
        let review = reviewer::review(self.cfg, &pass.reviewer, &input)
            .map_err(|err| about(&format!("review of {}", proposal.id), err))?;
        let verdict = match review.verdict {
            Verdict::Confirm => {
                self.summary.confirmed += 1;
                "confirm"
            }
            _ => {
                self.summary.rejected += 1;
                "reject"
            }
        };
        self.report
            .verdicts
            .push(format!("{} {verdict}: {}", proposal.id, review.notes));
        if self.dry_run {
            return Ok(());
        }
        if review.verdict != Verdict::Confirm {
            apply::reject(self.ws, proposal, &review)?;
            return Ok(());
        }
        match apply::confirm(self.ws, proposal, &review)? {
            Applied::Written(receipt) => {
                self.summary.applied += 1;
                let snapshot = receipt.snapshot.as_deref().unwrap_or(Path::new(""));
                self.report
                    .applied
                    .push(format!("{} {}", receipt.id, snapshot.display()));
            }
            Applied::NoteChanged(receipt) => self.report.verdicts.push(format!(
                "{} not applied: the note changed after the prompt (receipt {})",
                proposal.id, receipt.id
            )),
        }
        Ok(())
    }

    fn invalid(&mut self, line: String) {
        self.summary.validation_failures += 1;
        self.report.validation_failures.push(line);
    }
}

/// Every pass, or the one whose file stem is `only`, with its prompt and
/// persona read.
fn load_passes(ws: &Workspace, only: Option<&str>) -> Result<Vec<LoadedPass>> {
    let mut passes = Vec::new();
    for (path, pass) in packs::load_passes(&ws.root)? {
        let name = stem(&path);
        if only.is_some_and(|only| only != name) {
            continue;
        }
        // `passes_off`: a named pass still runs.
        if only.is_none() && ws.passes_off.contains(&name) {
            continue;
        }
        let pack = path
            .parent()
            .and_then(Path::parent)
            .expect("a pass file is in <pack>/passes/");
        let template = read(&pack.join(&pass.prompt))?;
        let persona = read(&pack.join(&pass.reviewer))?;
        let core = pack.file_name().is_some_and(|dir| dir == "core");
        let (persona, criteria, reviewer) = if core {
            (persona, None, stem(&pass.reviewer))
        } else {
            (
                packs::PASS_REVIEWER.to_owned(),
                Some(persona),
                format!("pass-reviewer/{}", stem(&pass.reviewer)),
            )
        };
        passes.push(LoadedPass {
            name,
            pass,
            template,
            persona,
            reviewer,
            core,
            criteria,
        });
    }
    if let (Some(only), true) = (only, passes.is_empty()) {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("no pass named {only}"),
        )));
    }
    Ok(passes)
}

/// `err`, its message led by `what` when it is a model or file error.
fn about(what: &str, err: Error) -> Error {
    match err {
        Error::Model(message) => Error::Model(format!("{what}: {message}")),
        Error::Io(err) => Error::Io(io::Error::new(err.kind(), format!("{what}: {err}"))),
        other => other,
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| Error::io_at(path, e))
}

/// `YYYY-MM-DDTHH:MM:SSZ`.
fn ts(at: OffsetDateTime) -> String {
    let format =
        time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    at.to_offset(UtcOffset::UTC)
        .format(&format)
        .unwrap_or_default()
}

/// `Some(size)` when `gathered`, rendered for `pass` without any turns, is
/// over the pass budget: no dropping of turns can make it fit.
/// Leaves out of `gathered` the sessions that touched or cited none of its
/// notes, as `found` records them, and returns how many it left out.
fn leave_unrelated(found: &ChunkInputs, gathered: &mut Gathered) -> usize {
    let cited: BTreeSet<&String> = gathered
        .notes
        .iter()
        .filter_map(|note| found.notes.get(&note.path))
        .flatten()
        .collect();
    let before = gathered.sessions.len();
    gathered
        .sessions
        .retain(|session| cited.contains(&session.record.id));
    before - gathered.sessions.len()
}

/// A session with nothing of its own to say about a note: no handoff and
/// no notes touched. It can still be in a prompt because its
/// transcript names a note.
fn says_nothing(session: &SessionInput) -> bool {
    let record = &session.record;
    record.notes_touched.is_empty()
        && record
            .handoff
            .as_deref()
            .is_none_or(|handoff| handoff.trim().is_empty())
}

fn over_budget(pass: &LoadedPass, gathered: &Gathered) -> Result<Option<usize>> {
    let (blocks, _) = gathered.rendered(&pass.pass.reads);
    let size = prompt::size_without_turns(&pass.template, &blocks)?;
    Ok((size > pass.pass.input_budget_bytes).then_some(size))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::adapters::{RawTurn, Role};
    use crate::index::{SessionSource, Turn};
    use crate::records::{ProposalKind, Receipt, Session, Tool};
    use crate::sessions;
    use crate::util::{hex, sha256};
    use serde_json::json;
    use time::format_description::well_known::Rfc3339;
    use ulid::Ulid;

    pub(crate) const NOTE_PATH: &str = "notes/Topic.md";
    pub(crate) const NOTE: &str = "# Topic\n\nThe migration ran on 3 November.\n\nIt was planned in October and checked twice before the release went out.\n";
    /// The text of turn `s1` 1, which a valid replace quotes.
    pub(crate) const TURN_1: &str = "Topic: it ran on 7 November";

    pub(crate) fn at(text: &str) -> OffsetDateTime {
        OffsetDateTime::parse(text, &Rfc3339).unwrap()
    }

    /// A workspace at `root` with `notes/` as its notes directory.
    pub(crate) fn workspace_at(root: &Path) -> Workspace {
        Workspace {
            root: root.to_path_buf(),
            notes_dir: "notes".into(),
            entry_note: NOTE_PATH.into(),
            required_notes: Vec::new(),
            budget_bytes: crate::DEFAULT_CONTEXT_BUDGET,
            log_path: "notes/Session log.md".into(),
            sample_tasks: Vec::new(),
            passes_off: Vec::new(),
            lessons: true,
            snapshot_keep_days: None,
            log_notes: None,
        }
    }

    #[test]
    fn passes_off_skips_a_pass_unless_it_is_named() {
        let dir = tempfile::tempdir().unwrap();
        crate::packs::write_core_pack(dir.path()).unwrap();
        let mut ws = workspace_at(dir.path());
        let all: Vec<String> = load_passes(&ws, None)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert!(all.contains(&"verify".to_owned()) && all.contains(&"consolidate".to_owned()));
        ws.passes_off = vec!["verify".into()];
        let names: Vec<String> = load_passes(&ws, None)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert!(!names.contains(&"verify".to_owned()) && names.contains(&"consolidate".to_owned()));
        assert_eq!(load_passes(&ws, Some("verify")).unwrap().len(), 1);
    }

    #[test]
    fn old_snapshots_are_pruned_and_new_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace_at(dir.path());
        let snaps = ws.hippo_dir().join("snapshots/2026-10-01");
        fs::create_dir_all(&snaps).unwrap();
        fs::write(snaps.join("old.md"), "a").unwrap();
        fs::write(snaps.join("new.md"), "b").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 86_400);
        fs::File::options()
            .write(true)
            .open(snaps.join("old.md"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(
            prune_snapshots(&ws, 30, OffsetDateTime::now_utc()).unwrap(),
            1
        );
        assert!(!snaps.join("old.md").exists() && snaps.join("new.md").exists());
    }

    /// A hippo home and a workspace with the core pack, the note
    /// `notes/Topic.md`, and session `s1`, which touched it, with two turns
    /// in the 24 hours before `now`. The model and the reviewer are stubs
    /// that print `dreamer.json` and `reviewer.json` from `stubs/` and save
    /// their prompts beside them.
    pub(crate) struct Setup {
        /// Held for the whole test: the stubs are processes (see
        /// `util::serial`).
        _serial: std::sync::MutexGuard<'static, ()>,
        pub home: tempfile::TempDir,
        pub workspace: tempfile::TempDir,
        pub stubs: tempfile::TempDir,
        pub cfg: Config,
        pub ws: Workspace,
        pub now: OffsetDateTime,
    }

    impl Setup {
        pub(crate) fn new() -> Setup {
            let home = tempfile::tempdir().unwrap();
            let workspace = tempfile::tempdir().unwrap();
            let stubs = tempfile::tempdir().unwrap();
            let ws = workspace_at(workspace.path());
            crate::packs::write_core_pack(workspace.path()).unwrap();
            // Each stub prints `<name>.json` and saves its stdin and its
            // system prompt as `<name>.stdin` and `<name>.system`.
            let stub = |name: &str| {
                let at = |suffix: &str| {
                    stubs
                        .path()
                        .join(format!("{name}{suffix}"))
                        .display()
                        .to_string()
                };
                [
                    "sh",
                    "-c",
                    r#"printf '%s' "$4" > "$3"; cat > "$2"; cat "$1""#,
                    "sh",
                    &at(".json"),
                    &at(".stdin"),
                    &at(".system"),
                    crate::model::SYSTEM_PROMPT,
                ]
                .map(String::from)
                .to_vec()
            };
            let cfg = Config {
                home: home.path().to_path_buf(),
                workspace: Some(workspace.path().to_path_buf()),
                transcript_roots: Vec::new(),
                exclude_roots: Vec::new(),
                model_command: stub("dreamer"),
                ingest_interval_minutes: 10,
                dream_at: "03:30".into(),
                reviewer_command_override: Some(stub("reviewer")),
            };
            let s = Setup {
                _serial: crate::util::serial(),
                home,
                workspace,
                stubs,
                cfg,
                ws,
                now: at("2026-11-08T03:30:00Z"),
            };
            s.add_note(NOTE_PATH, NOTE);
            s.add_session_turns(
                "s1",
                &[
                    (
                        at("2026-11-08T01:30:00Z"),
                        Role::User,
                        "check the migration",
                    ),
                    (at("2026-11-08T02:30:00Z"), Role::Assistant, TURN_1),
                ],
                &[NOTE_PATH],
                None,
            );
            s
        }

        pub(crate) fn index(&self) -> Index {
            Index::open(self.home.path(), true).unwrap()
        }

        pub(crate) fn add_note(&self, path: &str, text: &str) {
            let path = self.workspace.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// Session `id` with one user turn at `ts`.
        pub(crate) fn add_session(
            &self,
            id: &str,
            ts: OffsetDateTime,
            touched: &[&str],
            handoff: Option<&str>,
        ) {
            self.add_session_turns(id, &[(ts, Role::User, "a question")], touched, handoff);
        }

        pub(crate) fn add_session_turns(
            &self,
            id: &str,
            turns: &[(OffsetDateTime, Role, &str)],
            touched: &[&str],
            handoff: Option<&str>,
        ) {
            let turns: Vec<Turn> = turns
                .iter()
                .enumerate()
                .map(|(ordinal, (ts, role, text))| {
                    Turn::from_raw(RawTurn {
                        ordinal: ordinal as u32,
                        role: *role,
                        ts: *ts,
                        tool_name: None,
                        text: (*text).to_owned(),
                    })
                })
                .collect();
            let transcript = format!("/t/{id}.jsonl");
            self.index()
                .replace_session(
                    &SessionSource {
                        id,
                        tool: Tool::ClaudeCode,
                        transcript_path: Path::new(&transcript),
                        file_hash: "hash",
                    },
                    &turns,
                )
                .unwrap();
            Session {
                id: id.to_owned(),
                tool: Tool::ClaudeCode,
                task: "the migration".to_owned(),
                task_seen: true,
                context_bytes_at_start: 0,
                started: turns[0].ts,
                ended: Some(turns[turns.len() - 1].ts),
                notes_touched: touched.iter().map(PathBuf::from).collect(),
                handoff: handoff.map(str::to_owned),
                handoff_derived: false,
                no_change_reason: None,
                context_start: None,
                context_prompt: None,
                derived: false,
            }
            .save(
                &self
                    .ws
                    .hippo_dir()
                    .join("sessions")
                    .join(format!("{id}.json")),
            )
            .unwrap();
        }

        pub(crate) fn write_proposal(&self, proposal: &Proposal) {
            proposal
                .save(&validate::path(&self.ws, proposal.id))
                .unwrap();
        }

        /// A receipt with `verdict` for `proposal`, nothing applied.
        pub(crate) fn write_receipt(&self, proposal: Ulid, verdict: Verdict) {
            let receipt = Receipt {
                id: Ulid::generate(),
                proposal,
                proposal_hash: String::new(),
                before_hash: String::new(),
                after_hash: String::new(),
                reviewer: "human".into(),
                invocation: String::new(),
                verdict,
                notes: String::new(),
                at: self.now,
                applied: None,
                snapshot: None,
            };
            receipt
                .save(&apply::receipt_path(&self.ws, receipt.id))
                .unwrap();
        }

        /// The dreamer replies with `proposals` as its JSON array.
        fn dreamer_proposes(&self, proposals: serde_json::Value) {
            self.dreamer_says(&proposals.to_string());
        }

        fn dreamer_says(&self, text: &str) {
            let reply = json!({"result": text}).to_string();
            fs::write(self.stubs.path().join("dreamer.json"), reply).unwrap();
        }

        fn reviewer_says(&self, verdict: &str) {
            let text = json!({"verdict": verdict, "notes": "stub review"}).to_string();
            let reply = json!({"result": text}).to_string();
            fs::write(self.stubs.path().join("reviewer.json"), reply).unwrap();
        }

        fn run(&self, dry_run: bool) -> Result<RunSummary> {
            run_at(&self.cfg, &self.ws, Some("verify"), dry_run, self.now)
        }

        /// Writes `.hippo/packs/<name>/` from `files`, `(path in the pack,
        /// text)`.
        pub(crate) fn with_pack(&self, name: &str, files: &[(&str, &str)]) {
            for (path, text) in files {
                self.add_note(&format!(".hippo/packs/{name}/{path}"), text);
            }
        }

        /// Runs only pass `name`.
        pub(crate) fn run_pass(&self, name: &str) -> Result<RunSummary> {
            run_at(&self.cfg, &self.ws, Some(name), false, self.now)
        }

        /// The pack `acme` with one pass, `keeper`, that reads notes and
        /// turns; `extra` is added to its pass file.
        fn with_acme(&self, extra: &str) {
            let pass = format!(
                "protocol = \"HIPPO_V1\"\nprompt = \"prompts/keeper.md\"\n\
                 reads = [\"sessions\", \"notes\", \"turns\"]\n\
                 reviewer = \"personas/keeper-reviewer.md\"\n\
                 input_budget_bytes = 100000\nper_note = false\n{extra}"
            );
            self.with_pack(
                "acme",
                &[
                    ("passes/keeper.toml", &pass),
                    (
                        "prompts/keeper.md",
                        "# Keeper\n\n## Sessions\n\n{{sessions}}\n\n## Notes\n\n{{notes}}\n\n## Turns\n\n{{turns}}\n",
                    ),
                    ("personas/keeper-reviewer.md", ACME_PERSONA),
                ],
            );
        }

        /// The newest report: a second run on the same day writes
        /// `<date>-<n>-dream.md` beside the first.
        fn report(&self) -> String {
            fs::read_to_string(report::newest_path(&self.ws).unwrap()).unwrap()
        }

        fn note(&self) -> String {
            fs::read_to_string(self.ws.root.join(NOTE_PATH)).unwrap()
        }

        fn files(&self, dir: &str) -> Vec<PathBuf> {
            crate::util::json_files(&self.ws.hippo_dir().join(dir)).unwrap()
        }
    }

    const ACME_PERSONA: &str = "# Keeper reviewer\n\nReject a bullet about a plan.\n";

    fn append(session: &str, ordinal: u32, new: &str) -> serde_json::Value {
        json!([{
            "note": NOTE_PATH,
            "kind": "append",
            "old": "",
            "new": new,
            "evidence": [{"session": session, "ordinal": ordinal}],
            "reason": "r",
        }])
    }

    fn stub(s: &Setup, file: &str) -> String {
        fs::read_to_string(s.stubs.path().join(file)).unwrap()
    }

    #[test]
    fn note_paths_keep_only_the_matching_notes_in_the_prompt() {
        let s = Setup::new();
        s.add_note("notes/Glossary.md", "# Glossary\n");
        s.add_note("notes/Other.md", "# Other\n");
        s.add_session(
            "s2",
            at("2026-11-08T02:00:00Z"),
            &["notes/Glossary.md", "notes/Other.md"],
            None,
        );
        s.with_acme("note_paths = [\"Glossary*.md\"]\n");
        s.dreamer_proposes(json!([]));
        s.run_pass("keeper").unwrap();
        let rendered = stub(&s, "dreamer.stdin");
        assert!(
            rendered.contains("{\"note\":\"notes/Glossary.md\""),
            "{rendered}"
        );
        assert!(
            !rendered.contains("{\"note\":\"notes/Other.md\""),
            "{rendered}"
        );
        assert!(
            !rendered.contains("{\"note\":\"notes/Topic.md\""),
            "{rendered}"
        );
        // Only the session that touched a kept note stays.
        assert!(rendered.contains("\"id\":\"s2\""), "{rendered}");
        assert!(!rendered.contains("\"id\":\"s1\""), "{rendered}");
        assert!(s.report().contains("keeper: 1 sessions, 1 notes,"));
    }

    #[test]
    fn note_types_keep_only_notes_of_those_types() {
        let s = Setup::new();
        s.add_note("notes/Deploys.md", "---\ntype: log\n---\n# Deploys\n");
        s.add_session(
            "s2",
            at("2026-11-08T02:00:00Z"),
            &["notes/Deploys.md", NOTE_PATH],
            None,
        );
        s.with_acme("note_types = [\"log\"]\n");
        s.dreamer_proposes(json!([]));
        s.run_pass("keeper").unwrap();
        let rendered = stub(&s, "dreamer.stdin");
        assert!(
            rendered.contains("{\"note\":\"notes/Deploys.md\""),
            "{rendered}"
        );
        assert!(
            !rendered.contains("{\"note\":\"notes/Topic.md\""),
            "{rendered}"
        );
    }

    #[test]
    fn a_pass_with_no_matching_note_makes_no_call() {
        let s = Setup::new();
        s.with_acme("note_paths = [\"Nothing*.md\"]\n");
        s.dreamer_says("not json");
        s.run_pass("keeper").unwrap();
        assert!(!s.stubs.path().join("dreamer.stdin").exists());
        assert!(
            s.report().contains("keeper: no note to work on"),
            "{}",
            s.report()
        );
    }

    #[test]
    fn a_pass_outside_core_is_reviewed_by_hippos_reviewer_with_criteria() {
        let s = Setup::new();
        s.with_acme("");
        s.dreamer_proposes(append("s1", 1, "\nMore.\n"));
        s.reviewer_says("confirm");
        let summary = s.run_pass("keeper").unwrap();
        assert_eq!((summary.confirmed, summary.applied), (1, 1));
        assert_eq!(stub(&s, "reviewer.system"), packs::PASS_REVIEWER);
        let message = stub(&s, "reviewer.stdin");
        let session = {
            let record = sessions::get(&s.ws, "s1").unwrap().unwrap();
            inputs::session_line(&SessionInput {
                record,
                last_ordinal: 1,
            })
        };
        let criteria = json!({"persona": ACME_PERSONA});
        assert!(
            message.ends_with(&format!(
                "\n## Sessions\n\n<<<UNTRUSTED sessions>>>\n{session}\n<<<END sessions>>>\n\
                 \n## Pack criteria\n\n<<<UNTRUSTED criteria>>>\n{criteria}\n<<<END criteria>>>\n"
            )),
            "{message}"
        );
        let receipt = Receipt::load(&s.files("receipts")[0]).unwrap();
        assert_eq!(receipt.reviewer, "pass-reviewer/keeper-reviewer");
    }

    #[test]
    fn a_core_pass_is_reviewed_exactly_as_before() {
        let s = Setup::new();
        s.dreamer_proposes(replace());
        s.reviewer_says("confirm");
        s.run(false).unwrap();
        let persona = fs::read_to_string(
            s.ws.root
                .join(".hippo/packs/core/personas/verify-reviewer.md"),
        )
        .unwrap();
        assert_eq!(stub(&s, "reviewer.system"), persona);
        let input = ReviewerInput {
            persona,
            note: NOTE.to_owned(),
            kind: ProposalKind::Replace,
            old: "ran on 3 November".into(),
            new: TURN_1.into(),
            evidence: s.index().turns(&[("s1".to_owned(), 1)]).unwrap(),
            sessions: Vec::new(),
            criteria: None,
        };
        assert_eq!(stub(&s, "reviewer.stdin"), input.user_prompt());
        let receipt = Receipt::load(&s.files("receipts")[0]).unwrap();
        assert_eq!(receipt.reviewer, "verify-reviewer");
    }

    #[test]
    fn outside_core_a_long_append_and_a_foreign_session_fail_validation() {
        let s = Setup::new();
        s.add_session("s2", at("2026-11-07T02:00:00Z"), &[], None);
        s.with_acme("");
        // A reviewer call would fail the run.
        fs::write(s.stubs.path().join("reviewer.json"), "not json").unwrap();
        let long = "x".repeat(2049);
        s.dreamer_proposes(append("s1", 1, &long));
        let summary = s.run_pass("keeper").unwrap();
        assert_eq!((summary.proposals, summary.validation_failures), (0, 1));
        assert!(
            s.report()
                .contains("keeper notes/Topic.md: append over 2048 bytes"),
            "{}",
            s.report()
        );
        fs::remove_file(window::state_path(&s.ws)).unwrap();
        s.dreamer_proposes(append("s2", 0, "\nMore.\n"));
        let summary = s.run_pass("keeper").unwrap();
        assert_eq!((summary.proposals, summary.validation_failures), (0, 1));
        assert!(
            s.report()
                .contains("evidence s2 is not a session of this prompt"),
            "{}",
            s.report()
        );
    }

    #[test]
    fn a_core_pass_may_append_more_and_cite_any_session() {
        let s = Setup::new();
        s.add_session("s2", at("2026-11-08T02:00:00Z"), &[], None);
        s.reviewer_says("reject");
        s.dreamer_proposes(append("s2", 0, &"x".repeat(2049)));
        let summary = run_at(&s.cfg, &s.ws, Some("consolidate"), false, s.now).unwrap();
        assert_eq!((summary.proposals, summary.validation_failures), (1, 0));
    }

    fn replace() -> serde_json::Value {
        json!([{
            "note": NOTE_PATH,
            "kind": "replace",
            "old": "ran on 3 November",
            "new": TURN_1,
            "evidence": [{"session": "s1", "ordinal": 1}],
            "reason": "REASON-NOT-FOR-THE-REVIEWER",
        }])
    }

    #[test]
    fn a_confirmed_proposal_is_written_reviewed_applied_and_reported() {
        let s = Setup::new();
        s.dreamer_proposes(replace());
        s.reviewer_says("confirm");
        let summary = s.run(false).unwrap();
        assert_eq!(
            (
                summary.chunks,
                summary.proposals,
                summary.confirmed,
                summary.applied
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(s.note(), NOTE.replace("ran on 3 November", TURN_1));
        let proposals = s.files("proposals");
        let receipts = s.files("receipts");
        assert_eq!((proposals.len(), receipts.len()), (1, 1));
        let proposal = Proposal::load(&proposals[0]).unwrap();
        let receipt = Receipt::load(&receipts[0]).unwrap();
        assert_eq!(receipt.proposal, proposal.id);
        assert_eq!(
            receipt.proposal_hash,
            hex(&sha256(&fs::read(&proposals[0]).unwrap()))
        );
        assert_eq!(receipt.reviewer, "verify-reviewer");
        assert!(
            receipt.invocation.contains("<system prompt: "),
            "{}",
            receipt.invocation
        );
        assert_eq!(receipt.notes, "stub review");
        let snapshot = receipt.snapshot.clone().unwrap();
        assert_eq!(fs::read_to_string(s.ws.root.join(&snapshot)).unwrap(), NOTE);
        assert_eq!(window::last_success(&s.ws).unwrap(), Some(s.now));
        let report = s.report();
        assert!(report.starts_with("status: ok\n"), "{report}");
        assert!(report.contains(&format!("- {} {NOTE_PATH} (verify, replace)", proposal.id)));
        assert!(report.contains(&format!("- {} confirm: stub review", proposal.id)));
        assert!(report.contains(&format!("- {} {}", receipt.id, snapshot.display())));
        assert_eq!(
            summary.report,
            s.ws.hippo_dir().join("reports/2026-11-08-dream.md")
        );
        // The dreamer's message is the rendered prompt, with its delimited
        // inputs, under a one-line system prompt; the reviewer's system
        // prompt is the persona, and its message has no reason.
        let stub = |file: &str| fs::read_to_string(s.stubs.path().join(file)).unwrap();
        let rendered = stub("dreamer.stdin");
        assert!(
            rendered.contains("<<<UNTRUSTED turns>>>") && rendered.contains(TURN_1),
            "{rendered}"
        );
        assert!(rendered.trim_end().ends_with(prompt::CLOSING_INSTRUCTION));
        assert_eq!(stub("dreamer.system"), pass_system_prompt("verify"));
        assert_eq!(
            stub("dreamer.system"),
            "You are hippo's verify pass. Follow the rules in the message."
        );
        assert!(stub("reviewer.system").starts_with("# Verify reviewer"));
        let reviewer = stub("reviewer.stdin");
        assert!(reviewer.contains("<<<UNTRUSTED evidence>>>"));
        assert!(!reviewer.contains("REASON-NOT-FOR-THE-REVIEWER"));
    }

    #[test]
    fn a_rejected_proposal_gets_a_receipt_and_the_note_stays() {
        let s = Setup::new();
        s.dreamer_proposes(replace());
        s.reviewer_says("reject");
        let summary = s.run(false).unwrap();
        assert_eq!((summary.rejected, summary.applied), (1, 0));
        assert_eq!(s.note(), NOTE);
        let receipt = Receipt::load(&s.files("receipts")[0]).unwrap();
        assert_eq!(
            (receipt.verdict, receipt.applied, receipt.snapshot),
            (Verdict::Reject, None, None)
        );
        assert!(s.report().contains("## Applied\n"));
    }

    #[test]
    fn invalid_proposals_are_reported_and_never_reviewed() {
        let s = Setup::new();
        s.dreamer_proposes(json!([
            {"note": "../etc/passwd", "kind": "replace", "old": "a", "new": TURN_1,
             "evidence": [{"session": "s1", "ordinal": 1}], "reason": "r"},
            {"note": NOTE_PATH, "kind": "replace", "old": "on", "new": TURN_1,
             "evidence": [{"session": "s1", "ordinal": 1}], "reason": "r"},
            {"note": NOTE_PATH, "kind": "rewrite"},
            "not an object",
        ]));
        // A reviewer call would fail the run.
        fs::write(s.stubs.path().join("reviewer.json"), "not json").unwrap();
        let summary = s.run(false).unwrap();
        assert_eq!((summary.proposals, summary.validation_failures), (0, 4));
        assert!(s.files("proposals").is_empty() && s.files("receipts").is_empty());
        assert_eq!(s.note(), NOTE);
        let report = s.report();
        assert!(
            report.contains("- verify ../etc/passwd: not a note inside notes_dir"),
            "{report}"
        );
        assert!(
            report.contains("- verify notes/Topic.md: old text occurs 2 times"),
            "{report}"
        );
        assert_eq!(report.matches("not a proposal").count(), 2, "{report}");
    }

    #[test]
    fn a_dry_run_reviews_but_writes_only_the_report() {
        let s = Setup::new();
        s.dreamer_proposes(replace());
        s.reviewer_says("confirm");
        let summary = s.run(true).unwrap();
        assert_eq!(
            (summary.proposals, summary.confirmed, summary.applied),
            (1, 1, 0)
        );
        assert_eq!(s.note(), NOTE);
        assert!(s.files("proposals").is_empty() && s.files("receipts").is_empty());
        assert!(!s.ws.hippo_dir().join("snapshots").exists());
        assert!(!window::state_path(&s.ws).exists());
        let report = s.report();
        assert!(report.starts_with("status: ok\n"));
        assert!(report.contains("dry run"));
        assert!(report.contains("confirm: stub review"));
    }

    #[test]
    fn a_failed_model_call_fails_the_run_and_keeps_the_window() {
        let s = Setup::new();
        s.reviewer_says("confirm");
        s.dreamer_says("not json");
        let err = s.run(false).unwrap_err();
        assert!(
            err.to_string().contains("the reply has no JSON array"),
            "{err}"
        );
        let report = s.report();
        assert!(report.starts_with("status: failed\nerror: "), "{report}");
        assert!(report.contains(": failed\n"), "{report}");
        assert!(!window::state_path(&s.ws).exists());
        // A command that exits non-zero fails the same way.
        fs::remove_file(s.stubs.path().join("dreamer.json")).unwrap();
        let err = s.run(false).unwrap_err();
        assert!(matches!(err, Error::Model(_)), "{err:?}");
        assert!(s.report().starts_with("status: failed\n"));
    }

    #[test]
    fn a_reviewer_that_cannot_run_fails_the_run_instead_of_rejecting() {
        let s = Setup::new();
        s.dreamer_proposes(replace());
        let err = s.run(false).unwrap_err();
        assert!(matches!(err, Error::Model(_)), "{err:?}");
        assert!(s.files("receipts").is_empty());
        assert_eq!(s.files("proposals").len(), 1);
        assert_eq!(s.note(), NOTE);
    }

    #[test]
    fn the_window_advances_past_quiet_days_and_an_unknown_pass_fails() {
        let s = Setup::new();
        s.dreamer_proposes(json!([]));
        window::advance(&s.ws, at("2026-11-05T03:30:00Z")).unwrap();
        let summary = s.run(false).unwrap();
        assert_eq!((summary.chunks, summary.proposals), (3, 0));
        assert_eq!(window::last_success(&s.ws).unwrap(), Some(s.now));
        let report = s.report();
        assert_eq!(
            report
                .matches(": no session record has a turn here")
                .count(),
            2,
            "{report}"
        );
        let err = run_at(&s.cfg, &s.ws, Some("dream-harder"), false, s.now).unwrap_err();
        assert!(
            err.to_string().contains("no pass named dream-harder"),
            "{err}"
        );
        assert!(s.report().starts_with("status: failed\n"));
    }

    /// A note of `bytes` bytes, more than any pass budget allows.
    fn huge_note(bytes: usize) -> String {
        format!("# Log\n\n{}\n", "a log line\n".repeat(bytes / 11))
    }

    #[test]
    fn a_note_over_half_the_budget_is_read_in_parts() {
        let s = Setup::new();
        let mut note = String::from("# Log\n\nThe migration ran on 3 November.\n");
        for day in 1..=5 {
            note.push_str(&format!(
                "\n## Day {day}\n\n{}",
                "a log line\n".repeat(4_000)
            ));
        }
        s.add_note(NOTE_PATH, &note);
        // A replace outside the part read is refused before review.
        s.dreamer_proposes(json!([{
            "note": NOTE_PATH, "kind": "replace", "old": "ran on 3 November",
            "new": TURN_1, "evidence": [{"session": "s1", "ordinal": 1}], "reason": "r"
        }]));
        let summary = s.run(false).unwrap();
        let report = s.report();
        assert!(report.starts_with("status: ok\n"), "{report}");
        assert!(
            report.contains("in parts: ")
                && report.contains("parts of at most 50000 bytes; reading lines "),
            "{report}"
        );
        // Without an observed edit, the last part only: one call.
        assert_eq!(
            report.matches(": 1 sessions, 1 notes").count(),
            1,
            "{report}"
        );
        assert!(
            report.contains("old is outside the excerpt the pass read (lines "),
            "{report}"
        );
        assert_eq!((summary.proposals, summary.validation_failures), (0, 1));
        let rendered = fs::read_to_string(s.stubs.path().join("dreamer.stdin")).unwrap();
        assert!(rendered.len() <= 200_000);
        assert!(rendered.contains("\"lines\":\"lines ") && rendered.contains("## Day 5"));
        assert!(!rendered.contains("## Day 1"));
        assert!(window::state_path(&s.ws).exists());
    }

    #[test]
    fn consolidate_cuts_the_largest_notes_to_their_tails_until_the_prompt_fits() {
        let s = Setup::new();
        let big = "notes/Log.md";
        s.add_note(
            big,
            &format!("{}\n## Last\n\nThe newest entry.\n", huge_note(250_000)),
        );
        s.add_session("s2", at("2026-11-08T02:00:00Z"), &[big], None);
        s.dreamer_proposes(json!([]));
        run_at(&s.cfg, &s.ws, Some("consolidate"), false, s.now).unwrap();
        let report = s.report();
        assert!(report.starts_with("status: ok\n"), "{report}");
        assert!(
            report.contains("excerpted: note notes/Log.md (")
                && report.contains("to its last sections, lines "),
            "{report}"
        );
        assert!(!report.contains("omitted: note"), "{report}");
        let rendered = fs::read_to_string(s.stubs.path().join("dreamer.stdin")).unwrap();
        assert!(rendered.contains("notes/Topic.md") && rendered.contains("The newest entry."));
        assert!(rendered.len() <= 200_000);
    }

    /// The pack `wide` with one pass, `digest`, that reads sessions and
    /// notes within `budget` bytes, not per note.
    fn with_wide(s: &Setup, budget: usize) {
        with_wide_reading(s, budget, "");
    }

    /// [`with_wide`], its pass reading `turns` too when `turns` is
    /// `", \"turns\""`.
    fn with_wide_reading(s: &Setup, budget: usize, turns: &str) {
        let pass = format!(
            "protocol = \"HIPPO_V1\"\nprompt = \"prompts/digest.md\"\n\
             reads = [\"sessions\", \"notes\"{turns}]\n\
             reviewer = \"personas/digest-reviewer.md\"\n\
             input_budget_bytes = {budget}\nper_note = false\n"
        );
        s.with_pack(
            "wide",
            &[
                ("passes/digest.toml", &pass),
                (
                    "prompts/digest.md",
                    if turns.is_empty() {
                        "# Digest\n\n## Sessions\n\n{{sessions}}\n\n## Notes\n\n{{notes}}\n"
                    } else {
                        "# Digest\n\n## Sessions\n\n{{sessions}}\n\n## Notes\n\n{{notes}}\n\n## Turns\n\n{{turns}}\n"
                    },
                ),
                ("personas/digest-reviewer.md", ACME_PERSONA),
            ],
        );
    }

    /// `count` sessions `<prefix>NN` at 02:00 touching `touched`, each with
    /// a pasted brief of 5,000 bytes as its task and `handoff`.
    fn add_briefed_sessions(
        s: &Setup,
        prefix: &str,
        count: usize,
        touched: &[&str],
        handoff: Option<&str>,
    ) {
        for n in 0..count {
            let id = format!("{prefix}{n:02}");
            s.add_session(&id, at("2026-11-08T02:00:00Z"), touched, handoff);
            let file = s.ws.hippo_dir().join("sessions").join(format!("{id}.json"));
            let mut record: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
            record["task"] = json!("Here is the whole brief for the task. ".repeat(130));
            fs::write(&file, record.to_string()).unwrap();
        }
    }

    #[test]
    fn sessions_alone_over_budget_still_leave_the_notes_and_call_the_model() {
        let s = Setup::new();
        with_wide(&s, 20_000);
        // 40 bounded lines of about 750 bytes: the sessions alone are over
        // the budget, and none of them touched or cited a note.
        add_briefed_sessions(&s, "u", 40, &[], None);
        s.dreamer_proposes(json!([]));
        s.run_pass("digest").unwrap();
        let report = s.report();
        assert!(report.starts_with("status: ok\n"), "{report}");
        assert!(!report.contains(": skipped: "), "{report}");
        assert!(!report.contains("omitted: note"), "{report}");
        let rendered = stub(&s, "dreamer.stdin");
        assert!(rendered.len() <= 20_000, "{}", rendered.len());
        assert!(
            rendered.contains("\"id\":\"s1\"")
                && rendered.contains("The migration ran on 3 November.")
        );
        assert!(!rendered.contains("\"id\":\"u00\""));
    }

    #[test]
    fn the_sessions_left_out_are_one_count_line() {
        let s = Setup::new();
        with_wide(&s, 20_000);
        add_briefed_sessions(&s, "u", 40, &[], None);
        s.dreamer_proposes(json!([]));
        s.run_pass("digest").unwrap();
        let report = s.report();
        let omissions: Vec<&str> = report
            .split("## Omissions\n\n")
            .nth(1)
            .unwrap()
            .split("\n\n## ")
            .next()
            .unwrap()
            .lines()
            .collect();
        assert_eq!(
            omissions,
            ["- 2026-11-07T03:30:00Z to 2026-11-08T03:30:00Z digest: omitted: 40 sessions that touched or cited none of its notes"],
            "{report}"
        );
    }

    #[test]
    fn when_the_sessions_of_the_last_note_alone_are_over_budget_the_skip_says_so() {
        let s = Setup::new();
        with_wide(&s, 20_000);
        // Every session touched the one note: none can be left out, and
        // leaving out the note does not make the sessions fit.
        add_briefed_sessions(&s, "t", 40, &[NOTE_PATH], Some("Checked [[Topic]]."));
        s.dreamer_proposes(json!([]));
        s.run_pass("digest").unwrap();
        let report = s.report();
        assert!(
            report.contains("digest: skipped: the sessions of its last note alone make a ")
                && report.contains("-byte prompt, over 20000"),
            "{report}"
        );
        assert!(!report.contains("no note fits"), "{report}");
        assert!(!s.stubs.path().join("dreamer.stdin").exists());
    }

    /// Session `id` whose turns are tool calls `(ts, tool, input)` and whose
    /// record names no note.
    fn add_tool_session(s: &Setup, id: &str, calls: &[(OffsetDateTime, &str, serde_json::Value)]) {
        let turns: Vec<Turn> = calls
            .iter()
            .enumerate()
            .map(|(ordinal, (ts, tool, input))| {
                Turn::from_raw(RawTurn {
                    ordinal: ordinal as u32,
                    role: Role::Tool,
                    ts: *ts,
                    tool_name: Some((*tool).to_owned()),
                    text: format!("{tool} {input}"),
                })
            })
            .collect();
        let transcript = format!("/t/{id}.jsonl");
        s.index()
            .replace_session(
                &SessionSource {
                    id,
                    tool: Tool::ClaudeCode,
                    transcript_path: Path::new(&transcript),
                    file_hash: "hash",
                },
                &turns,
            )
            .unwrap();
        Session {
            id: id.to_owned(),
            tool: Tool::ClaudeCode,
            task: String::new(),
            task_seen: false,
            context_bytes_at_start: 0,
            started: turns[0].ts,
            ended: Some(turns[turns.len() - 1].ts),
            notes_touched: Vec::new(),
            handoff: None,
            handoff_derived: false,
            no_change_reason: None,
            context_start: None,
            context_prompt: None,
            derived: true,
        }
        .save(&s.ws.hippo_dir().join("sessions").join(format!("{id}.json")))
        .unwrap();
    }

    #[test]
    fn sessions_that_only_read_a_note_go_before_the_note() {
        let s = Setup::new();
        with_wide_reading(&s, 20_000, ", \"turns\"");
        let root = s.workspace.path().display().to_string();
        // 40 sessions whose transcripts read the note but whose records
        // have no handoff and touched no note, each with a long brief.
        for n in 0..40 {
            let id = format!("r{n:02}");
            add_tool_session(
                &s,
                &id,
                &[(
                    at("2026-11-08T02:00:00Z"),
                    "Read",
                    json!({"file_path": format!("{root}/{NOTE_PATH}")}),
                )],
            );
            let file = s.ws.hippo_dir().join("sessions").join(format!("{id}.json"));
            let mut record: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
            record["task"] = json!("Here is the whole brief for the task. ".repeat(130));
            fs::write(&file, record.to_string()).unwrap();
        }
        s.dreamer_proposes(json!([]));
        s.run_pass("digest").unwrap();
        let report = s.report();
        assert!(report.starts_with("status: ok\n"), "{report}");
        assert!(!report.contains(": skipped: "), "{report}");
        assert!(!report.contains("omitted: note"), "{report}");
        let line = report
            .lines()
            .find(|line| line.contains("sessions with no handoff and no notes touched"))
            .unwrap_or_else(|| panic!("{report}"));
        let count: usize = line
            .split("digest: omitted: ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap();
        assert!((1..40).contains(&count), "{report}");
        let rendered = stub(&s, "dreamer.stdin");
        assert!(
            rendered.contains("\"id\":\"s1\"")
                && rendered.contains("The migration ran on 3 November.")
        );
    }

    #[test]
    fn verify_takes_the_notes_transcripts_show_ranked_and_capped() {
        let s = Setup::new();
        for name in ["A", "B", "C"] {
            s.add_note(
                &format!("notes/{name}.md"),
                &format!("# {name}\n\nAbout {name}.\n"),
            );
        }
        let root = s.workspace.path().display().to_string();
        add_tool_session(
            &s,
            "s2",
            &[
                (
                    at("2026-11-08T02:00:00Z"),
                    "Read",
                    json!({"file_path": format!("{root}/notes/A.md")}),
                ),
                (
                    at("2026-11-08T02:01:00Z"),
                    "Edit",
                    json!({"file_path": format!("{root}/notes/B.md"), "old_string": "x", "new_string": "y"}),
                ),
                (
                    at("2026-11-08T02:02:00Z"),
                    "Bash",
                    json!({"command": format!("cat {root}/notes/C.md \"{root}/notes/A.md\"")}),
                ),
            ],
        );
        let pass = s.ws.hippo_dir().join("packs/core/passes/verify.toml");
        let text = fs::read_to_string(&pass).unwrap();
        fs::write(&pass, format!("{text}max_notes = 2\n")).unwrap();
        s.dreamer_proposes(json!([]));
        s.run(false).unwrap();
        let report = s.report();
        assert!(
            report.contains("2 sessions; declared 1 notes; observed 3 notes (1 edited, 0 discussed, 2 read) in 1 sessions"),
            "{report}"
        );
        // Edits first (B observed, Topic declared; equal evidence, so by
        // path), then reads by evidence: A (2 turns), C (1).
        assert!(
            report.contains("verify notes/B.md: 1 sessions, 1 notes"),
            "{report}"
        );
        assert!(
            report.contains("verify notes/Topic.md: 1 sessions, 1 notes"),
            "{report}"
        );
        assert!(
            report.contains("verify: omitted: note notes/A.md (rank 3 of 4, over max_notes 2)"),
            "{report}"
        );
        assert!(
            report.contains("verify: omitted: note notes/C.md (rank 4 of 4, over max_notes 2)"),
            "{report}"
        );
        // Consolidate reads no turns: it takes the declared note only.
        fs::remove_file(window::state_path(&s.ws)).unwrap();
        run_at(&s.cfg, &s.ws, Some("consolidate"), true, s.now).unwrap();
        let rendered = fs::read_to_string(s.stubs.path().join("dreamer.stdin")).unwrap();
        assert!(
            rendered.contains("notes/Topic.md") && !rendered.contains("\"note\":\"notes/B.md\"")
        );
    }

    #[test]
    fn verify_ranks_log_notes_after_every_other_note() {
        let s = Setup::new();
        // A folder of dated write-ups (log notes) and one
        // topic note the session only reads.
        for day in 1..=5 {
            s.add_note(
                &format!("notes/Sessions/2026-11-0{day}-run.md"),
                "# Run\n\nWhat happened.\n",
            );
        }
        s.add_note("notes/Product.md", "# Product\n\nWhat is true.\n");
        let root = s.workspace.path().display().to_string();
        add_tool_session(
            &s,
            "s2",
            &[
                (
                    at("2026-11-08T02:00:00Z"),
                    "Write",
                    json!({"file_path": format!("{root}/notes/Sessions/2026-11-05-run.md"), "content": "# Run"}),
                ),
                (
                    at("2026-11-08T02:01:00Z"),
                    "Edit",
                    json!({"file_path": format!("{root}/notes/Sessions/2026-11-04-run.md"), "old_string": "x", "new_string": "y"}),
                ),
                (
                    at("2026-11-08T02:02:00Z"),
                    "Read",
                    json!({"file_path": format!("{root}/notes/Product.md")}),
                ),
            ],
        );
        let pass = s.ws.hippo_dir().join("packs/core/passes/verify.toml");
        let text = fs::read_to_string(&pass).unwrap();
        fs::write(&pass, format!("{text}max_notes = 3\n")).unwrap();
        s.dreamer_proposes(json!([]));
        s.run(false).unwrap();
        let report = s.report();
        // The read topic note outranks both edited log notes; one log note
        // still fills the slot left over.
        assert!(
            report.contains("verify notes/Topic.md: 1 sessions, 1 notes"),
            "{report}"
        );
        assert!(
            report.contains("verify notes/Product.md: 1 sessions, 1 notes"),
            "{report}"
        );
        assert!(
            report.contains("verify notes/Sessions/2026-11-04-run.md: 1 sessions, 1 notes"),
            "{report}"
        );
        assert!(
            report.contains(
                "verify: omitted: note notes/Sessions/2026-11-05-run.md (rank 4 of 4, over max_notes 3, log note)"
            ),
            "{report}"
        );
    }

    #[test]
    fn human_confirmed_text_reaches_the_prompt() {
        let s = Setup::new();
        let reverted = Proposal {
            id: Ulid::generate(),
            pass: "verify".into(),
            note: NOTE_PATH.into(),
            kind: ProposalKind::Replace,
            before_hash: String::new(),
            after_hash: String::new(),
            old: "ran on 3 November".into(),
            new: "x".into(),
            evidence: Vec::new(),
            reason: String::new(),
        };
        s.write_proposal(&reverted);
        s.write_receipt(reverted.id, Verdict::Revert);
        s.dreamer_proposes(json!([]));
        s.run(false).unwrap();
        let rendered = fs::read_to_string(s.stubs.path().join("dreamer.stdin")).unwrap();
        let block = rendered
            .split("<<<UNTRUSTED human_confirmed>>>\n")
            .nth(1)
            .unwrap();
        assert!(block.starts_with("{\"note\":\"notes/Topic.md\",\"text\":\"ran on 3 November\"}\n<<<END human_confirmed>>>"));
    }

    #[test]
    fn a_run_ingests_first_and_goes_on_when_another_ingest_holds_the_lock() {
        let s = Setup::new();
        s.dreamer_proposes(json!([]));
        let summary = s.run(false).unwrap();
        assert!(s.home.path().join("ingest-state.json").is_file());
        let report = s.report();
        let inputs = report.split("## Inputs\n\n").nth(1).unwrap();
        assert!(
            inputs.starts_with("- ingest first: indexed 0 files, skipped 0, derived 0 sessions\n"),
            "{report}"
        );
        assert_eq!(
            summary.to_string(),
            format!(
                "{}: 1 chunks, 0 proposals, 0 validation failures, 0 confirmed, 0 rejected, 0 applied",
                summary.report.display()
            )
        );

        let held = fs::File::create(s.home.path().join("ingest.lock")).unwrap();
        held.try_lock().unwrap();
        window::advance(&s.ws, at("2026-11-07T00:00:00Z")).unwrap();
        s.run(false).unwrap();
        assert!(
            s.report()
                .contains("- ingest first: another ingest holds the lock"),
            "{}",
            s.report()
        );
        held.unlock().unwrap();
    }

    #[test]
    fn history_groups_receipts_under_proposals_in_id_order() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace_at(dir.path());
        let empty = history(&ws).unwrap();
        assert!(empty.changes.is_empty() && empty.unreadable.is_empty());

        let proposals = ws.hippo_dir().join("proposals");
        let receipts = ws.hippo_dir().join("receipts");
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/records");
        let proposal: Proposal = Proposal::load(&fixtures.join("proposal-replace.json")).unwrap();
        let receipt = Receipt::load(&fixtures.join("receipt-confirm.json")).unwrap();
        let later = Ulid::from_parts(proposal.id.timestamp_ms() + 1_000, 1);
        let later_proposal = Proposal {
            id: later,
            ..proposal.clone()
        };
        later_proposal
            .save(&proposals.join(format!("{later}.json")))
            .unwrap();
        proposal
            .save(&proposals.join(format!("{}.json", proposal.id)))
            .unwrap();
        let second = Receipt {
            id: Ulid::from_parts(receipt.id.timestamp_ms() + 5, 2),
            ..receipt.clone()
        };
        for r in [&second, &receipt] {
            r.save(&receipts.join(format!("{}.json", r.id))).unwrap();
        }
        // A proposal file that is not named by an id, and a receipt that
        // does not load.
        fs::write(proposals.join("notes.json"), "{}").unwrap();
        fs::write(receipts.join("half.json"), "").unwrap();

        let history = history(&ws).unwrap();
        let ids: Vec<Ulid> = history.changes.iter().map(|c| c.proposal_id).collect();
        assert_eq!(ids, [proposal.id, later]);
        assert_eq!(
            history.changes[0].receipts,
            [receipt.clone(), second.clone()]
        );
        assert!(history.changes[1].receipts.is_empty());
        assert_eq!(
            history.changes[0].proposal.record.as_ref().unwrap(),
            &proposal
        );
        let mut names: Vec<String> = history
            .unreadable
            .iter()
            .map(|(path, _)| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["half.json", "notes.json"]);
    }

    #[test]
    fn a_receipt_without_its_proposal_is_listed_under_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace_at(dir.path());
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/records");
        let receipt = Receipt::load(&fixtures.join("receipt-confirm.json")).unwrap();
        receipt
            .save(&ws.hippo_dir().join(format!("receipts/{}.json", receipt.id)))
            .unwrap();
        let history = history(&ws).unwrap();
        assert_eq!(history.changes.len(), 1);
        assert_eq!(history.changes[0].proposal_id, receipt.proposal);
        assert!(history.changes[0].proposal.record.is_err());
        assert_eq!(history.changes[0].receipts, [receipt]);
        assert!(history.unreadable.is_empty());
    }
}

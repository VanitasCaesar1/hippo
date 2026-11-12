//! `hippocamp init [--workspace <path>] [--notes-dir <rel>] [--entry-note <rel>]
//! [--add-root <path>]... [--force]`.
//!
//! With no flags, outside a workspace, it writes a history-only
//! `config.toml`: hippocamp reads the agents' transcripts and has no notes
//! folder yet.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use hippocamp_core::config::{self, InitOptions};
use time::{Duration, OffsetDateTime};

#[derive(clap::Args)]
pub struct Args {
    /// The workspace; the current directory when it holds .hippocamp/ or the notes flags are given
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// The notes directory, relative to the workspace (needed to create .hippocamp/)
    #[arg(long)]
    notes_dir: Option<String>,
    /// The entry note, relative to the workspace (needed to create .hippocamp/)
    #[arg(long)]
    entry_note: Option<String>,
    /// Another folder of transcripts to read, absolute or under ~/ (repeatable)
    #[arg(long = "add-root", value_name = "PATH")]
    add_roots: Vec<String>,
    /// Overwrite config.toml and the files init writes under .hippocamp/
    #[arg(long)]
    force: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    // Whether this adds a notes folder to a history-only setup.
    let was_history_only = config::Config::load().is_ok_and(|c| c.workspace.is_none());
    let names = args.notes_dir.clone().zip(args.entry_note.clone());
    let report = config::init(InitOptions {
        workspace: args.workspace,
        notes_dir: args.notes_dir,
        entry_note: args.entry_note,
        add_roots: args.add_roots,
        force: args.force,
    })?;
    for path in &report.written {
        println!("wrote {}", path.display());
    }
    for path in &report.updated {
        println!("updated {}", path.display());
    }
    for path in &report.kept {
        println!("kept {}", path.display());
    }
    if report.workspace.is_some()
        && names.is_none()
        && report.written.is_empty()
        && report.updated.is_empty()
    {
        println!("the ignore rules and the merge attribute for notes are in place");
    }
    if let (Some(root), Some((notes_dir, entry_note))) = (&report.workspace, &names) {
        starter(root, notes_dir, entry_note)?;
        if was_history_only {
            records_hint(root);
        }
    }
    if report.workspace.is_none() {
        println!("no notes folder yet: hippocamp will read your agents' history; add one later with hippocamp init --workspace <dir> --notes-dir <dir> --entry-note <note>");
    }
    Ok(ExitCode::SUCCESS)
}

/// Creates the notes folder when it is missing and writes a starter entry
/// note when the entry note is missing, saying so.
/// Never overwrites a file.
fn starter(root: &Path, notes_dir: &str, entry_note: &str) -> Result<()> {
    let dir = root.join(notes_dir);
    if !dir.is_dir() {
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        println!("created {}/ (it did not exist)", dir.display());
    }
    let entry = root.join(entry_note);
    if !entry.exists() {
        if let Some(parent) = entry.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        fs::write(&entry, starter_note(OffsetDateTime::now_utc()))
            .with_context(|| format!("cannot write {}", entry.display()))?;
        println!(
            "wrote {} (a starter entry note: edit it to say what agents should read first)",
            entry.display()
        );
    }
    Ok(())
}

/// The starter entry note, with the frontmatter the linter wants.
fn starter_note(now: OffsetDateTime) -> String {
    let today = now.date();
    let review = (now + Duration::days(90)).date();
    format!(
        "---\ntype: policy\nstatus: current\nupdated: {today}\nreview_after: {review}\n---\n\
         # Start here\n\n\
         The entry note: every new agent session in this workspace reads the part above the first \
         `##` heading first. Say what the project is and which notes matter most.\n\n\
         ## Reading order\n\n\
         List the notes an agent should read first, each as a double-bracket link to its name.\n"
    )
}

/// When a history-only setup gains a notes folder, the sessions already
/// indexed have no record: name the command that derives them.
fn records_hint(_root: &Path) {
    let Ok(config) = config::Config::load() else {
        return;
    };
    let Ok(index) = hippocamp_core::index::Index::open(&config.home, false) else {
        return;
    };
    let n = index.turn_counts().map_or(0, |c| c.len());
    if n > 0 {
        println!(
            "{n} sessions are indexed (sub-agents included): run hippocamp ingest --full to give the ones that belong to this workspace a session record"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starter_note_has_the_frontmatter_the_linter_wants() {
        let note = starter_note(time::macros::datetime!(2026-11-11 10:00 UTC));
        assert!(note.starts_with("---\ntype: policy\nstatus: current\nupdated: 2026-11-11\nreview_after: 2027-02-09\n---\n# Start here\n"));
    }
}

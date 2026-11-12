//! `hippo eval`: the context, recall and dreaming suites over a synthetic
//! eval set.
//!
//! [`run`] copies a set into a temporary hippo home and workspace, ingests
//! its transcripts and runs the suites there, so it writes nothing outside
//! that temporary directory. The dreaming suite runs only when the set has
//! a `[dreaming]` table and a model is given.

pub mod context;
pub mod dreaming;
pub mod recall;
pub mod relevant;
pub mod report;
pub mod set;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use time::OffsetDateTime;

use crate::config::{self, Config, InitOptions, Workspace};
use crate::error::{Error, Result};
use crate::index::Index;
use crate::ingest;
use crate::packs::CORE_PACK;
use crate::util::{copy_dir, hex, sha256};

pub use report::{Outcome, Report};
pub use set::Set;

use set::{holds_phrase, short_digest, RecallKind};

/// The commands a model run uses, read from the user's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Models {
    pub model_command: Vec<String>,
    pub reviewer_command_override: Option<Vec<String>>,
}

/// Runs the eval set in `set_dir`. A set that fails its checks, or a setup
/// that fails, is an `Err`; a suite that fails is recorded in the report.
pub fn run(set_dir: &Path, models: Option<&Models>) -> Result<Report> {
    let started = OffsetDateTime::now_utc();
    let set = Set::load(set_dir)?;
    let set_hash = Set::hash(set_dir)?;
    let core_pack_hash = core_pack_hash();

    // Removed when it drops, on error too. The prefix only makes the
    // directory easy to recognise.
    let tmp = tempfile::Builder::new().prefix("hippo-eval-").tempdir()?;
    let home = tmp.path().join("home");
    let workspace = tmp.path().join("workspace");
    let transcripts = tmp.path().join("transcripts");
    copy_dir(&set_dir.join("notes"), &workspace.join("notes"))?;
    if set_dir.join("transcripts").is_dir() {
        copy_dir(&set_dir.join("transcripts"), &transcripts)?;
    } else {
        fs::create_dir_all(&transcripts)?;
    }

    // Declared session records go in after the setup and before ingest,
    // which would otherwise derive records of its own for these sessions.
    let (config, ws) = scratch(
        &home,
        &workspace,
        &transcripts,
        &set.entry_note.to_string_lossy(),
        models,
    )?;
    if set_dir.join("sessions").is_dir() {
        copy_dir(
            &set_dir.join("sessions"),
            &workspace.join(".hippo").join("sessions"),
        )?;
    }

    ingest::ingest(&config, Some(&ws), false)?;
    let index = Index::open(&config.home, false)?;
    if let Some(failed) = index.failed_files()?.first() {
        return Err(Error::EvalSet(format!(
            "transcript {} did not parse: {}",
            failed.transcript_path, failed.error
        )));
    }
    for (i, question) in set.recall.iter().enumerate() {
        for turn in &question.expected {
            if !index.exists(&turn.session, turn.ordinal)? {
                return Err(Error::EvalSet(format!(
                    "recall question {} ({}): turn {} {} is not in the index",
                    i + 1,
                    question.query,
                    turn.session,
                    turn.ordinal
                )));
            }
        }
        // Check 8: a quoted query must stand as a whole phrase in one of its
        // expected turns, or a wrong ordinal that names a real turn becomes
        // a silent miss.
        if question.kind != RecallKind::Paraphrase {
            let mut quoted = false;
            for turn in &question.expected {
                if let Some(found) = index.turn(&turn.session, turn.ordinal)? {
                    quoted |= holds_phrase(&found.text, &question.query);
                }
            }
            if !quoted {
                return Err(Error::EvalSet(format!(
                    "recall question {} ({}): no expected turn contains the query",
                    i + 1,
                    question.query
                )));
            }
        }
    }

    // Redaction must leave no mark in the index: a marked turn is not the
    // text the key was written against.
    // An exact scan of every indexed turn, not a search.
    let mut ids: Vec<String> = index.last_ordinals()?.into_keys().collect();
    ids.sort();
    for id in &ids {
        for turn in index.turns_for_session(id)? {
            if turn.text.contains("[redacted:") {
                return Err(Error::EvalSet(format!(
                    "turn {id} {} holds redacted text",
                    turn.ordinal
                )));
            }
        }
    }

    let mut clean_candidates = BTreeSet::new();
    if let Some(dreaming) = &set.dreaming {
        clean_candidates = set::check_dreaming_index(&ws, &index, dreaming)?;
    }

    // Context first: later suites may change notes or add reports.
    let context = match context::run(&ws, &set) {
        Ok(result) => Outcome::Ran(result),
        Err(err) => Outcome::Failed(err.to_string()),
    };
    let recall = match recall::run(&index, &set) {
        Ok(result) => Outcome::Ran(result),
        Err(err) => Outcome::Failed(err.to_string()),
    };
    // Last: the run changes notes and writes a dreaming report.
    let mut failed_dreaming_report = None;
    let dreaming = match (&set.dreaming, models) {
        (None, _) => Outcome::Skipped("the set has no [dreaming]".into()),
        (Some(_), None) => Outcome::Skipped("needs --with-model".into()),
        (Some(answers), Some(_)) => {
            match dreaming::run(&config, &ws, set_dir, answers, &clean_candidates) {
                Ok(result) => Outcome::Ran(result),
                Err(err) => {
                    failed_dreaming_report = newest_dream_report(&ws);
                    Outcome::Failed(err.to_string())
                }
            }
        }
    };
    Ok(Report {
        set: set_dir.display().to_string(),
        set_hash,
        core_pack_hash,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        models: models.cloned(),
        started,
        context,
        recall,
        dreaming,
        failed_dreaming_report,
    })
}

/// The text of the newest dreaming report in the workspace, the one a failed
/// run wrote about itself.
fn newest_dream_report(ws: &Workspace) -> Option<String> {
    fs::read_to_string(crate::dream::report::newest_path(ws)?).ok()
}

/// The hash of the core pack built into the binary: SHA-256 over its files
/// in order, each as `<path> <sha256 of its content>`, shortened to 12 hex
/// digits like the set's hash. It says which prompts and personas ran.
fn core_pack_hash() -> String {
    let lines: Vec<String> = CORE_PACK
        .iter()
        .map(|(path, text)| format!("{path} {}\n", hex(&sha256(text.as_bytes()))))
        .collect();
    short_digest(&lines)
}

/// The temporary setup `eval::run` and `hippo packs test` share: writes a
/// hippo home at `home` for the workspace `workspace` (notes in `notes/`,
/// `entry_note` as given) and loads it, with `models` when given, and with
/// `transcript_roots` set to `transcripts` alone, so ingest reads nothing of
/// the user's.
pub(crate) fn scratch(
    home: &Path,
    workspace: &Path,
    transcripts: &Path,
    entry_note: &str,
    models: Option<&Models>,
) -> Result<(Config, Workspace)> {
    config::init_in(
        home,
        InitOptions {
            workspace: Some(workspace.to_path_buf()),
            notes_dir: Some("notes".into()),
            entry_note: Some(entry_note.to_owned()),
            add_roots: Vec::new(),
            force: false,
        },
    )?;
    let mut config = Config::load_in(home)?;
    if let Some(models) = models {
        config.model_command = models.model_command.clone();
        config.reviewer_command_override = models.reviewer_command_override.clone();
    }
    // Transcript roots are glob patterns: escape the path so that no
    // character in it is read as one.
    config.transcript_roots = vec![glob::Pattern::escape(&transcripts.to_string_lossy())];
    let ws = Workspace::load(&config)?;
    Ok((config, ws))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_core_pack_hash_is_twelve_hex_digits_and_stable() {
        let hash = core_pack_hash();
        assert_eq!(hash.len(), 12);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(hash, core_pack_hash());
    }
}

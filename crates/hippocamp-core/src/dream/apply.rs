//! Dreaming step 7 and revert.
//!
//! A confirmed proposal is applied in three steps: the note is copied to
//! `snapshots/<proposal id>/<note path>`, never over an existing snapshot;
//! the change is written by [`notes::apply`], whose only caller this is and
//! which takes the [`Confirmed`] that only a confirming receipt yields; then
//! the receipt is written once, with `applied` and `snapshot`. A rejected
//! proposal gets a receipt with both null, and so does a confirmed one
//! whose note changed after the prompt, which is left as it is.
//!
//! [`revert`] restores a note from the snapshot of an applied change and
//! writes a revert receipt. [`revert_state`] decides what may be reverted:
//! a change that was already reverted, or that a newer change to the same
//! note sits on, is refused even with force; a note that changed since is
//! refused unless forced.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use time::OffsetDateTime;
use ulid::Ulid;

use crate::config::Workspace;
use crate::dream::reviewer::Review;
use crate::dream::validate;
use crate::dream::{history, History};
use crate::error::{Error, Result};
use crate::notes;
use crate::records::{encode, load_error, Confirmed, Proposal, ProposalKind, Receipt, Verdict};
use crate::util::{hex, sha256, write_atomic, write_new};

/// What became of a confirmed proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The change was written; the receipt has `applied` and `snapshot`.
    Written(Receipt),
    /// The note changed after the prompt, so nothing was written; the
    /// receipt records the confirm verdict with `applied` null.
    NoteChanged(Receipt),
}

/// Applies `proposal`, which `review` confirmed: snapshot, change, receipt.
pub fn confirm(ws: &Workspace, proposal: &Proposal, review: &Review) -> Result<Applied> {
    let file = read(&validate::path(ws, proposal.id))?;
    let mut receipt = receipt_for(proposal, &file, review);
    let note = ws.root.join(&proposal.note);
    let current = match fs::read(&note) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(Error::io_at(&note, e)),
    };
    let confirmed = match Confirmed::check(&file, proposal, &receipt, &current) {
        Ok(confirmed) => confirmed,
        Err(Error::NoteChanged { .. }) => {
            write_receipt(ws, &receipt)?;
            return Ok(Applied::NoteChanged(receipt));
        }
        Err(err) => return Err(err),
    };
    let snapshot = if proposal.kind == ProposalKind::Create {
        notes::validate_new_note_path(&ws.root, &ws.notes_dir, &proposal.note)?;
        notes::snapshot_empty(&ws.root, proposal.id, &proposal.note)?
    } else {
        notes::snapshot(&ws.root, proposal.id, &proposal.note)?
    };
    match notes::apply(&ws.root, &confirmed) {
        Ok(_) => {
            receipt.applied = Some(OffsetDateTime::now_utc());
            receipt.snapshot = Some(snapshot);
            write_receipt(ws, &receipt)?;
            Ok(Applied::Written(receipt))
        }
        Err(Error::NoteChanged { .. }) => {
            write_receipt(ws, &receipt)?;
            Ok(Applied::NoteChanged(receipt))
        }
        Err(err) => Err(err),
    }
}

/// Records that `review` rejected `proposal`: a receipt with `applied` and
/// `snapshot` null.
pub fn reject(ws: &Workspace, proposal: &Proposal, review: &Review) -> Result<Receipt> {
    let file = read(&validate::path(ws, proposal.id))?;
    let receipt = receipt_for(proposal, &file, review);
    write_receipt(ws, &receipt)?;
    Ok(receipt)
}

/// What may be done with an applied change, as [`revert_state`] decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevertState {
    /// The note is as the change left it and the snapshot is intact.
    Revertible,
    /// As [`Revertible`](RevertState::Revertible), but the note's hash is not
    /// the receipt's `after_hash`: it changed after the dreaming change.
    /// Reverting needs `force`.
    NoteChanged,
    /// A later applied, unreverted change to the same note exists, the one
    /// with this receipt. Reverts go newest first.
    NewerChange(Ulid),
    /// A revert receipt, this one, already names the proposal.
    Reverted(Ulid),
    NoteMissing,
    /// The snapshot is missing, or does not hash to the receipt's
    /// `before_hash`.
    SnapshotMissing,
}

/// What may be done with the change of confirm receipt `receipt`. `hippocamp
/// revert` decides with the same function. See [`revert_state_in`].
pub fn revert_state(ws: &Workspace, receipt: Ulid) -> Result<RevertState> {
    revert_state_in(ws, &history(ws)?, receipt)
}

/// [`revert_state`] with a history that is already loaded. The checks run
/// in this order, and the first that holds decides:
///
/// 1. The receipt is not a confirm receipt with `applied` and a snapshot
///    under `.hippocamp/snapshots/`, its proposal does not load, or the
///    proposal's note path is absolute, holds `..` or is not under the
///    notes directory: an error.
/// 2. A revert receipt names the proposal: `Reverted`, with the earliest.
/// 3. The note does not exist: `NoteMissing`.
/// 4. The note fails [`notes::validate_proposal_path`], for example because
///    a symlink leads out of the notes directory: an error.
/// 5. The snapshot does not exist, or does not hash to the receipt's
///    `before_hash`: `SnapshotMissing`.
/// 6. Another confirm receipt for the same note, applied and not reverted,
///    has a later `applied` time, or the same time and a later id:
///    `NewerChange`, with the newest such receipt.
/// 7. The note's hash differs from the receipt's `after_hash`:
///    `NoteChanged`.
/// 8. Otherwise `Revertible`.
pub fn revert_state_in(ws: &Workspace, history: &History, receipt_id: Ulid) -> Result<RevertState> {
    let (confirm, _, note) = confirm_and_note(ws, history, receipt_id)?;
    let reverts = |proposal: Ulid| {
        history
            .changes
            .iter()
            .filter(|change| change.proposal_id == proposal)
            .flat_map(|change| &change.receipts)
            .filter(|receipt| receipt.verdict == Verdict::Revert)
            .map(|receipt| receipt.id)
            .min()
    };
    if let Some(reverted_by) = reverts(confirm.proposal) {
        return Ok(RevertState::Reverted(reverted_by));
    }
    let note_file = ws.root.join(&note);
    match fs::metadata(&note_file) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(RevertState::NoteMissing),
        Err(e) => return Err(Error::io_at(&note_file, e)),
    }
    notes::validate_proposal_path(&ws.root, &ws.notes_dir, &note)?;
    let snapshot = confirm.snapshot.as_ref().expect("checked with the receipt");
    match fs::read(ws.root.join(snapshot)) {
        Ok(bytes) if hex(&sha256(&bytes)) == confirm.before_hash => {}
        _ => return Ok(RevertState::SnapshotMissing),
    }
    let applied = confirm.applied.expect("checked with the receipt");
    let newer =
        history
            .changes
            .iter()
            .filter(|change| {
                change.proposal.record.as_ref().is_ok_and(|proposal| {
                    notes::normalise(&proposal.note) == notes::normalise(&note)
                }) && reverts(change.proposal_id).is_none()
            })
            .flat_map(|change| &change.receipts)
            .filter(|other| other.verdict == Verdict::Confirm && other.id != confirm.id)
            .filter_map(|other| Some((other.applied?, other.id)))
            .filter(|&(at, id)| (at, id) > (applied, confirm.id))
            .max();
    if let Some((_, id)) = newer {
        return Ok(RevertState::NewerChange(id));
    }
    let current = read(&note_file)?;
    if hex(&sha256(&current)) != confirm.after_hash {
        return Ok(RevertState::NoteChanged);
    }
    Ok(RevertState::Revertible)
}

/// The confirm receipt `receipt_id` with its proposal and the proposal's
/// note path, or the error of check 1 of [`revert_state_in`]. A receipt the
/// history does not hold is read from its file, so that the error of a
/// missing one is the file's.
fn confirm_and_note(
    ws: &Workspace,
    history: &History,
    receipt_id: Ulid,
) -> Result<(Receipt, Proposal, PathBuf)> {
    let known = history
        .changes
        .iter()
        .flat_map(|change| &change.receipts)
        .find(|receipt| receipt.id == receipt_id);
    let confirm = match known {
        Some(receipt) => receipt.clone(),
        None => Receipt::load(&receipt_path(ws, receipt_id))?,
    };
    let snapshot = match (&confirm.verdict, &confirm.applied, &confirm.snapshot) {
        (Verdict::Confirm, Some(_), Some(snapshot)) => snapshot,
        _ => {
            return Err(invalid(format!(
                "receipt {receipt_id} is not a confirm receipt whose change was applied"
            )))
        }
    };
    if !is_snapshot_path(snapshot) {
        return Err(invalid(format!(
            "receipt {receipt_id} names snapshot {}, which is not under .hippocamp/snapshots/",
            snapshot.display()
        )));
    }
    let listed = history
        .changes
        .iter()
        .find(|change| change.proposal_id == confirm.proposal)
        .map(|change| change.proposal.record.clone());
    let proposal = match listed {
        Some(record) => record.map_err(load_error)?,
        None => Proposal::load(&validate::path(ws, confirm.proposal))?,
    };
    if !notes::names_a_note(&ws.notes_dir, &proposal.note) {
        return Err(Error::PathOutsideNotes(proposal.note));
    }
    let note = proposal.note.clone();
    Ok((confirm, proposal, note))
}

/// Reverts the change of confirm receipt `receipt_id`: when
/// [`revert_state`] says `Revertible`, or `NoteChanged` and `force` is set,
/// the note is restored from the snapshot atomically and a revert receipt
/// is written and returned. `NoteChanged` without `force` is
/// [`Error::NoteChanged`]; every other state is refused, with or without
/// `force`, naming the receipt or file that decides it.
pub fn revert(ws: &Workspace, receipt_id: Ulid, force: bool) -> Result<Receipt> {
    revert_at(ws, receipt_id, force, OffsetDateTime::now_utc())
}

fn revert_at(
    ws: &Workspace,
    receipt_id: Ulid,
    force: bool,
    now: OffsetDateTime,
) -> Result<Receipt> {
    let history = history(ws)?;
    let state = revert_state_in(ws, &history, receipt_id)?;
    let (confirm, proposal, _) = confirm_and_note(ws, &history, receipt_id)?;
    let snapshot = confirm.snapshot.clone().expect("checked with the receipt");
    let note = ws.root.join(&proposal.note);
    match state {
        RevertState::Revertible => {}
        RevertState::NoteChanged if force => {}
        RevertState::NoteChanged => {
            return Err(Error::NoteChanged {
                expected: confirm.after_hash,
                actual: hex(&sha256(&read(&note)?)),
            })
        }
        RevertState::Reverted(by) => {
            return Err(invalid(format!(
                "receipt {receipt_id} was already reverted by {by}"
            )))
        }
        RevertState::NewerChange(newer) => {
            return Err(invalid(format!(
                "revert {newer} first: it changed {} later",
                proposal.note.display()
            )))
        }
        RevertState::NoteMissing => {
            return Err(invalid(format!(
                "note {} does not exist",
                proposal.note.display()
            )))
        }
        RevertState::SnapshotMissing => {
            return Err(invalid(format!(
                "snapshot {} is missing or does not match receipt {receipt_id}'s before_hash; \
                 snapshots stay on the machine that dreamed the change; on another machine \
                 undo it with git (git log -p -- {} shows the commit)",
                snapshot.display(),
                proposal.note.display()
            )))
        }
    }
    let current = read(&note)?;
    let current_hash = hex(&sha256(&current));
    let restored = read(&ws.root.join(&snapshot))?;
    if hex(&sha256(&restored)) != confirm.before_hash {
        return Err(invalid(format!(
            "snapshot {} does not match receipt {receipt_id}'s before_hash",
            snapshot.display()
        )));
    }
    if proposal.kind == ProposalKind::Create {
        // The note did not exist before: undoing its creation removes it.
        fs::remove_file(&note).map_err(|e| Error::io_at(&note, e))?;
    } else {
        write_atomic(&note, &restored)?;
    }
    let receipt = Receipt {
        id: Ulid::generate(),
        proposal: confirm.proposal,
        proposal_hash: confirm.proposal_hash,
        before_hash: current_hash,
        after_hash: confirm.before_hash,
        reviewer: "human".to_owned(),
        invocation: format!("hippocamp revert {receipt_id}"),
        verdict: Verdict::Revert,
        notes: String::new(),
        at: now,
        applied: Some(now),
        snapshot: Some(snapshot),
    };
    write_receipt(ws, &receipt)?;
    Ok(receipt)
}

/// `<workspace>/.hippocamp/receipts/<id>.json`.
pub fn receipt_path(ws: &Workspace, id: Ulid) -> PathBuf {
    ws.hippocamp_dir()
        .join("receipts")
        .join(format!("{id}.json"))
}

/// A receipt for `review` of `proposal`, whose file holds `file`, with
/// nothing applied yet.
fn receipt_for(proposal: &Proposal, file: &[u8], review: &Review) -> Receipt {
    Receipt {
        id: Ulid::generate(),
        proposal: proposal.id,
        proposal_hash: hex(&sha256(file)),
        before_hash: proposal.before_hash.clone(),
        after_hash: proposal.after_hash.clone(),
        reviewer: review.reviewer.clone(),
        invocation: review.invocation.clone(),
        verdict: review.verdict,
        notes: review.notes.clone(),
        at: review.at,
        applied: None,
        snapshot: None,
    }
}

/// Writes `receipts/<id>.json` once: an existing file is never replaced.
fn write_receipt(ws: &Workspace, receipt: &Receipt) -> Result<()> {
    let path = receipt_path(ws, receipt.id);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))?;
    }
    write_new(&path, &encode(receipt)?)
}

/// A relative path of plain components under `.hippocamp/snapshots/`.
fn is_snapshot_path(path: &Path) -> bool {
    let mut components = path.components();
    path.components().all(|c| matches!(c, Component::Normal(_)))
        && components.next() == Some(Component::Normal(".hippocamp".as_ref()))
        && components.next() == Some(Component::Normal("snapshots".as_ref()))
        && components.next().is_some()
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| Error::io_at(path, e))
}

fn invalid(message: String) -> Error {
    Error::Io(io::Error::new(io::ErrorKind::InvalidInput, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dream::tests::{Setup, NOTE, NOTE_PATH, TURN_1};
    use crate::records::{Evidence, ProposalKind};

    /// A replace proposal on the setup's note, written to disk.
    fn proposal(s: &Setup) -> Proposal {
        let after = NOTE.replace("ran on 3 November", TURN_1);
        let proposal = Proposal {
            id: Ulid::generate(),
            pass: "verify".into(),
            note: NOTE_PATH.into(),
            kind: ProposalKind::Replace,
            before_hash: hex(&sha256(NOTE.as_bytes())),
            after_hash: hex(&sha256(after.as_bytes())),
            old: "ran on 3 November".into(),
            new: TURN_1.into(),
            evidence: vec![Evidence {
                session: "s1".into(),
                ordinal: 1,
            }],
            reason: "the turn says so".into(),
        };
        validate::write(&s.ws, &proposal).unwrap();
        proposal
    }

    fn review(verdict: Verdict) -> Review {
        Review {
            verdict,
            notes: "checked".into(),
            reviewer: "verify-reviewer".into(),
            invocation: "stub <system prompt: 3 bytes>".into(),
            at: OffsetDateTime::now_utc(),
        }
    }

    fn note_text(s: &Setup) -> String {
        fs::read_to_string(s.ws.root.join(NOTE_PATH)).unwrap()
    }

    #[test]
    fn a_confirmed_change_is_snapshotted_written_and_receipted() {
        let s = Setup::new();
        let proposal = proposal(&s);
        let Applied::Written(receipt) =
            confirm(&s.ws, &proposal, &review(Verdict::Confirm)).unwrap()
        else {
            panic!("not written")
        };
        assert_eq!(note_text(&s), NOTE.replace("ran on 3 November", TURN_1));
        let snapshot = receipt.snapshot.clone().unwrap();
        assert_eq!(
            snapshot,
            Path::new(".hippocamp/snapshots")
                .join(proposal.id.to_string())
                .join(NOTE_PATH)
        );
        assert_eq!(fs::read_to_string(s.ws.root.join(&snapshot)).unwrap(), NOTE);
        assert!(receipt.applied.is_some());
        let file = fs::read(validate::path(&s.ws, proposal.id)).unwrap();
        assert_eq!(receipt.proposal_hash, hex(&sha256(&file)));
        assert_eq!(
            Receipt::load(&receipt_path(&s.ws, receipt.id)).unwrap(),
            receipt
        );
    }

    #[test]
    fn a_note_edited_after_the_prompt_is_left_alone() {
        let s = Setup::new();
        let proposal = proposal(&s);
        let edited = format!("{NOTE}An agent's line.\n");
        fs::write(s.ws.root.join(NOTE_PATH), &edited).unwrap();
        let Applied::NoteChanged(receipt) =
            confirm(&s.ws, &proposal, &review(Verdict::Confirm)).unwrap()
        else {
            panic!("applied")
        };
        assert_eq!(note_text(&s), edited);
        assert_eq!(
            (receipt.verdict, receipt.applied, receipt.snapshot),
            (Verdict::Confirm, None, None)
        );
        assert!(!s.ws.hippocamp_dir().join("snapshots").exists());
    }

    #[test]
    fn a_rejection_gets_a_receipt_and_changes_nothing() {
        let s = Setup::new();
        let proposal = proposal(&s);
        let receipt = reject(&s.ws, &proposal, &review(Verdict::Reject)).unwrap();
        assert_eq!(
            (receipt.verdict, receipt.applied, receipt.snapshot),
            (Verdict::Reject, None, None)
        );
        assert_eq!(note_text(&s), NOTE);
        assert!(receipt_path(&s.ws, receipt.id).is_file());
    }

    #[test]
    fn revert_restores_the_snapshot_and_writes_a_revert_receipt() {
        let s = Setup::new();
        let proposal = proposal(&s);
        let Applied::Written(applied) =
            confirm(&s.ws, &proposal, &review(Verdict::Confirm)).unwrap()
        else {
            panic!("not written")
        };
        let now = OffsetDateTime::now_utc();
        let revert = revert_at(&s.ws, applied.id, false, now).unwrap();
        assert_eq!(note_text(&s), NOTE);
        assert_eq!(revert.verdict, Verdict::Revert);
        assert_eq!(revert.proposal, proposal.id);
        assert_eq!(revert.reviewer, "human");
        assert_eq!(
            revert.invocation,
            format!("hippocamp revert {}", applied.id)
        );
        assert_eq!(revert.before_hash, proposal.after_hash);
        assert_eq!(revert.after_hash, proposal.before_hash);
        assert_eq!(
            (revert.applied, revert.snapshot.clone()),
            (Some(now), applied.snapshot.clone())
        );
        assert_eq!(
            Receipt::load(&receipt_path(&s.ws, revert.id)).unwrap(),
            revert
        );
    }

    #[test]
    fn revert_refuses_an_edited_note_unless_forced() {
        let s = Setup::new();
        let proposal = proposal(&s);
        let Applied::Written(applied) =
            confirm(&s.ws, &proposal, &review(Verdict::Confirm)).unwrap()
        else {
            panic!("not written")
        };
        fs::write(s.ws.root.join(NOTE_PATH), "edited by hand\n").unwrap();
        let err = revert(&s.ws, applied.id, false).unwrap_err();
        assert!(matches!(err, Error::NoteChanged { .. }), "{err:?}");
        assert_eq!(note_text(&s), "edited by hand\n");
        revert(&s.ws, applied.id, true).unwrap();
        assert_eq!(note_text(&s), NOTE);
    }

    #[test]
    fn only_an_applied_confirm_receipt_can_be_reverted() {
        let s = Setup::new();
        let proposal = proposal(&s);
        let rejected = reject(&s.ws, &proposal, &review(Verdict::Reject)).unwrap();
        let err = revert(&s.ws, rejected.id, true).unwrap_err();
        assert!(err.to_string().contains("not a confirm receipt"), "{err}");
        assert!(revert(&s.ws, Ulid::generate(), true).is_err());
    }

    #[test]
    fn revert_refuses_a_snapshot_outside_snapshots_or_one_that_does_not_match() {
        let s = Setup::new();
        let proposal = proposal(&s);
        let Applied::Written(applied) =
            confirm(&s.ws, &proposal, &review(Verdict::Confirm)).unwrap()
        else {
            panic!("not written")
        };
        let snapshot = s.ws.root.join(applied.snapshot.as_ref().unwrap());
        fs::write(&snapshot, "tampered\n").unwrap();
        let err = revert(&s.ws, applied.id, false).unwrap_err();
        assert!(err.to_string().contains("does not match"), "{err}");
        let mut outside = applied.clone();
        outside.id = Ulid::generate();
        outside.snapshot = Some(PathBuf::from(NOTE_PATH));
        outside.save(&receipt_path(&s.ws, outside.id)).unwrap();
        let err = revert(&s.ws, outside.id, true).unwrap_err();
        assert!(
            err.to_string().contains("not under .hippocamp/snapshots/"),
            "{err}"
        );
    }

    /// A replace proposal on the setup's note from `from_text` (the note as
    /// it is now) replacing `old` with `new`, written to disk.
    fn proposal_on(s: &Setup, from_text: &str, old: &str, new: &str) -> Proposal {
        let after = from_text.replacen(old, new, 1);
        let proposal = Proposal {
            id: Ulid::generate(),
            pass: "verify".into(),
            note: NOTE_PATH.into(),
            kind: ProposalKind::Replace,
            before_hash: hex(&sha256(from_text.as_bytes())),
            after_hash: hex(&sha256(after.as_bytes())),
            old: old.into(),
            new: new.into(),
            evidence: vec![Evidence {
                session: "s1".into(),
                ordinal: 1,
            }],
            reason: "a test".into(),
        };
        validate::write(&s.ws, &proposal).unwrap();
        proposal
    }

    /// Applies a confirmed change and gives its receipt `id` and `applied`.
    fn applied_as(s: &Setup, proposal: &Proposal, id: Ulid, applied: &str) -> Receipt {
        let Applied::Written(mut receipt) =
            confirm(&s.ws, proposal, &review(Verdict::Confirm)).unwrap()
        else {
            panic!("not written")
        };
        fs::remove_file(receipt_path(&s.ws, receipt.id)).unwrap();
        receipt.id = id;
        receipt.applied = Some(crate::dream::tests::at(applied));
        receipt.save(&receipt_path(&s.ws, id)).unwrap();
        receipt
    }

    fn state(s: &Setup, receipt: &Receipt) -> RevertState {
        revert_state(&s.ws, receipt.id).unwrap()
    }

    fn id(n: u128) -> Ulid {
        Ulid::from_parts(1_700_000_000_000, n)
    }

    #[test]
    fn a_change_the_note_still_shows_is_revertible() {
        let s = Setup::new();
        let applied = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        assert_eq!(state(&s, &applied), RevertState::Revertible);
    }

    #[test]
    fn a_note_edited_since_is_note_changed() {
        let s = Setup::new();
        let applied = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        fs::write(s.ws.root.join(NOTE_PATH), "edited by hand\n").unwrap();
        assert_eq!(state(&s, &applied), RevertState::NoteChanged);
    }

    #[test]
    fn a_reverted_change_names_the_earliest_revert_receipt() {
        let s = Setup::new();
        let applied = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        let first = revert(&s.ws, applied.id, false).unwrap();
        assert_eq!(state(&s, &applied), RevertState::Reverted(first.id));
        // A later revert receipt for the same proposal does not change it.
        let mut later = first.clone();
        later.id = Ulid::from_parts(first.id.timestamp_ms() + 10_000, 1);
        later.save(&receipt_path(&s.ws, later.id)).unwrap();
        assert_eq!(state(&s, &applied), RevertState::Reverted(first.id));
    }

    #[test]
    fn a_change_under_a_newer_one_names_the_newest_newer_receipt() {
        let s = Setup::new();
        let first = proposal(&s);
        let older = applied_as(&s, &first, id(1), "2026-11-08T01:00:00Z");
        let after_first = note_text(&s);
        let second = proposal_on(&s, &after_first, "checked twice", "checked three times");
        let newer = applied_as(&s, &second, id(2), "2026-11-08T02:00:00Z");
        let after_second = note_text(&s);
        let third = proposal_on(&s, &after_second, "planned in October", "planned in May");
        let newest = applied_as(&s, &third, id(3), "2026-11-08T03:00:00Z");

        assert_eq!(state(&s, &older), RevertState::NewerChange(newest.id));
        assert_eq!(state(&s, &newer), RevertState::NewerChange(newest.id));
        assert_eq!(state(&s, &newest), RevertState::Revertible);

        // A reverted change no longer sits on anything: the middle one is
        // now under nothing newer.
        revert(&s.ws, newest.id, false).unwrap();
        assert_eq!(state(&s, &newer), RevertState::Revertible);
        assert_eq!(state(&s, &older), RevertState::NewerChange(newer.id));
        revert(&s.ws, newer.id, false).unwrap();
        assert_eq!(state(&s, &older), RevertState::Revertible);
    }

    #[test]
    fn a_tie_in_applied_is_broken_by_receipt_id() {
        let s = Setup::new();
        let older = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        let after_first = note_text(&s);
        let second = proposal_on(&s, &after_first, "checked twice", "checked three times");
        let newer = applied_as(&s, &second, id(2), "2026-11-08T01:00:00Z");
        assert_eq!(state(&s, &older), RevertState::NewerChange(newer.id));
        assert_eq!(state(&s, &newer), RevertState::Revertible);
    }

    #[test]
    fn a_revert_receipt_is_not_a_newer_change() {
        let s = Setup::new();
        let older = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        // A revert receipt of another change carries `applied` too, later.
        let after_first = note_text(&s);
        let second = proposal_on(&s, &after_first, "checked twice", "checked three times");
        let newer = applied_as(&s, &second, id(2), "2026-11-08T02:00:00Z");
        let reverted = revert(&s.ws, newer.id, false).unwrap();
        assert!(reverted.applied.unwrap() > older.applied.unwrap());
        assert_eq!(state(&s, &older), RevertState::Revertible);
    }

    #[test]
    fn a_change_to_another_note_is_not_a_newer_change() {
        let s = Setup::new();
        let older = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        let mut other = proposal(&s);
        other.note = "notes/Other.md".into();
        other.id = Ulid::generate();
        other.save(&validate::path(&s.ws, other.id)).unwrap();
        let mut receipt = older.clone();
        receipt.id = id(2);
        receipt.proposal = other.id;
        receipt.applied = Some(crate::dream::tests::at("2026-11-08T02:00:00Z"));
        receipt.save(&receipt_path(&s.ws, receipt.id)).unwrap();
        assert_eq!(state(&s, &older), RevertState::Revertible);
    }

    #[test]
    fn a_missing_note_comes_before_a_missing_snapshot() {
        let s = Setup::new();
        let applied = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        fs::remove_file(s.ws.root.join(applied.snapshot.as_ref().unwrap())).unwrap();
        assert_eq!(state(&s, &applied), RevertState::SnapshotMissing);
        fs::remove_file(s.ws.root.join(NOTE_PATH)).unwrap();
        assert_eq!(state(&s, &applied), RevertState::NoteMissing);
    }

    #[test]
    fn a_bad_snapshot_comes_before_a_newer_change_and_a_changed_note() {
        let s = Setup::new();
        let older = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        let after_first = note_text(&s);
        let second = proposal_on(&s, &after_first, "checked twice", "checked three times");
        applied_as(&s, &second, id(2), "2026-11-08T02:00:00Z");
        fs::write(
            s.ws.root.join(older.snapshot.as_ref().unwrap()),
            "tampered\n",
        )
        .unwrap();
        assert_eq!(state(&s, &older), RevertState::SnapshotMissing);
        fs::write(s.ws.root.join(NOTE_PATH), "edited by hand\n").unwrap();
        assert_eq!(state(&s, &older), RevertState::SnapshotMissing);
    }

    #[test]
    fn reverted_comes_before_every_other_state() {
        let s = Setup::new();
        let older = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        let reverted = revert(&s.ws, older.id, false).unwrap();
        let again = proposal_on(&s, NOTE, "checked twice", "checked three times");
        applied_as(&s, &again, id(2), "2026-11-08T03:00:00Z");
        fs::remove_file(s.ws.root.join(older.snapshot.as_ref().unwrap())).unwrap();
        fs::remove_file(s.ws.root.join(NOTE_PATH)).unwrap();
        assert_eq!(state(&s, &older), RevertState::Reverted(reverted.id));
    }

    #[test]
    fn revert_state_fails_for_receipts_and_proposals_it_cannot_use() {
        let s = Setup::new();
        let proposal = proposal(&s);
        let rejected = reject(&s.ws, &proposal, &review(Verdict::Reject)).unwrap();
        let err = revert_state(&s.ws, rejected.id).unwrap_err();
        assert!(err.to_string().contains("not a confirm receipt"), "{err}");
        assert!(revert_state(&s.ws, Ulid::generate()).is_err());

        let applied = applied_as(&s, &proposal, id(1), "2026-11-08T01:00:00Z");
        for note in [
            "/etc/passwd",
            "../x.md",
            "elsewhere/x.md",
            "notes/.hidden.md",
        ] {
            let mut moved = proposal.clone();
            moved.note = note.into();
            moved.save(&validate::path(&s.ws, proposal.id)).unwrap();
            let err = revert_state(&s.ws, applied.id).unwrap_err();
            assert!(matches!(err, Error::PathOutsideNotes(_)), "{note}: {err}");
        }
        fs::write(validate::path(&s.ws, proposal.id), "{").unwrap();
        assert!(revert_state(&s.ws, applied.id).is_err());
        fs::remove_file(validate::path(&s.ws, proposal.id)).unwrap();
        assert!(revert_state(&s.ws, applied.id).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_note_that_is_a_symlink_out_of_the_notes_is_an_error() {
        let s = Setup::new();
        let applied = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        let outside = s.workspace.path().join("outside.md");
        fs::write(&outside, NOTE).unwrap();
        fs::remove_file(s.ws.root.join(NOTE_PATH)).unwrap();
        std::os::unix::fs::symlink(&outside, s.ws.root.join(NOTE_PATH)).unwrap();
        let err = revert_state(&s.ws, applied.id).unwrap_err();
        assert!(matches!(err, Error::PathOutsideNotes(_)), "{err}");
    }

    #[test]
    fn revert_refuses_a_second_revert_even_with_force() {
        let s = Setup::new();
        let applied = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        let first = revert(&s.ws, applied.id, false).unwrap();
        for force in [false, true] {
            let err = revert(&s.ws, applied.id, force).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!(
                    "receipt {} was already reverted by {}",
                    applied.id, first.id
                )
            );
        }
        assert_eq!(note_text(&s), NOTE);
    }

    #[test]
    fn revert_refuses_an_older_change_under_a_newer_one_even_with_force() {
        let s = Setup::new();
        let older = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        let after_first = note_text(&s);
        let second = proposal_on(&s, &after_first, "checked twice", "checked three times");
        let newer = applied_as(&s, &second, id(2), "2026-11-08T02:00:00Z");
        let after_second = note_text(&s);
        for force in [false, true] {
            let err = revert(&s.ws, older.id, force).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("revert {} first: it changed {NOTE_PATH} later", newer.id)
            );
        }
        assert_eq!(note_text(&s), after_second);
    }

    #[test]
    fn revert_names_a_missing_note_or_snapshot_and_reports_a_snapshot_first() {
        let s = Setup::new();
        let applied = applied_as(&s, &proposal(&s), id(1), "2026-11-08T01:00:00Z");
        let snapshot = s.ws.root.join(applied.snapshot.as_ref().unwrap());
        fs::write(s.ws.root.join(NOTE_PATH), "edited by hand\n").unwrap();
        fs::remove_file(&snapshot).unwrap();
        // Changed note and bad snapshot: the snapshot is reported.
        let err = revert(&s.ws, applied.id, true).unwrap_err().to_string();
        assert!(
            err.starts_with(&format!(
                "snapshot {} is missing or does not match receipt {}'s before_hash; ",
                applied.snapshot.as_ref().unwrap().display(),
                applied.id
            )),
            "{err}"
        );
        // On another machine the snapshot is not there: git is the undo.
        assert!(
            err.ends_with(&format!(
                "undo it with git (git log -p -- {NOTE_PATH} shows the commit)"
            )),
            "{err}"
        );
        fs::remove_file(s.ws.root.join(NOTE_PATH)).unwrap();
        let err = revert(&s.ws, applied.id, true).unwrap_err();
        assert_eq!(err.to_string(), format!("note {NOTE_PATH} does not exist"));
    }
}

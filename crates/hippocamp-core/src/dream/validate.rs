//! Dreaming steps 4 and 5: the proposals in a model's reply, and the checks
//! each passes before review.
//!
//! A proposal is checked against the notes that went into its prompt: its
//! note path per Invariant 3 and one of those notes; a non-empty `new`; an
//! empty `old` for an `append`; at least one evidence pair, each in the
//! index; for a `replace`, a `new` that quotes a cited turn (a passage of at least [`QUOTE_MIN_CHARS`] characters copied from it, or
//! its whole text when shorter, compared with whitespace runs as one space),
//! an `old` that occurs exactly once and the size cap; for a note the
//! prompt showed as an excerpt, an `old` inside the excerpt and an `append`
//! only when the excerpt reaches the note's end. hippocamp then
//! computes `before_hash` from the prompt-time bytes and `after_hash` with
//! `notes::change`, the change `apply` makes, in memory. A valid proposal is written once, to
//! `proposals/<id>.json`, and never changed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use ulid::Ulid;

use crate::config::Workspace;
use crate::dream::inputs::NoteInput;
use crate::error::{Error, Result};
use crate::index::Index;
use crate::notes;
use crate::records::{encode, Evidence, Proposal, ProposalKind};
use crate::util::{hex, sha256, write_new};

/// A proposal as the model wrote it: `{note, kind, old, new, evidence:
/// [{session, ordinal}], reason}`. A missing `old` or `reason` reads as
/// empty.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Draft {
    pub note: String,
    pub kind: ProposalKind,
    #[serde(default)]
    pub old: String,
    pub new: String,
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub reason: String,
}

/// The outcome of checking a draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// The proposal, with its id and both hashes.
    Valid(Proposal),
    /// Why the draft failed; it is reported, not reviewed.
    Invalid(String),
}

/// The first JSON array in `reply`: the elements of the array that parses
/// from the earliest `[` where one does. `None` when there is none.
pub fn first_array(reply: &str) -> Option<Vec<Value>> {
    reply.match_indices('[').find_map(|(at, _)| {
        let mut values = serde_json::Deserializer::from_str(&reply[at..]).into_iter::<Value>();
        match values.next() {
            Some(Ok(Value::Array(items))) => Some(items),
            _ => None,
        }
    })
}

/// The most an `append` from a pass outside `core` may add, in bytes.
pub const OUTSIDE_APPEND_CAP: usize = 2048;

/// Checks `draft` from pass `pass` against `notes`, the notes that went
/// into its prompt. `outside` is `None` for a `core` pass; for a pass
/// outside `core` it holds the ids of the prompt's sessions, and after the
/// checks above an `append` over [`OUTSIDE_APPEND_CAP`] bytes and evidence
/// from any other session are invalid too. Errors are those of reading the
/// index.
pub fn check(
    ws: &Workspace,
    index: &Index,
    pass: &str,
    notes: &[NoteInput],
    draft: &Draft,
    outside: Option<&BTreeSet<String>>,
) -> Result<Checked> {
    let invalid = |why: String| Ok(Checked::Invalid(why));
    let path = Path::new(&draft.note);
    if let Err(err) = notes::validate_proposal_path(&ws.root, &ws.notes_dir, path) {
        return invalid(err.to_string());
    }
    let Some(note) = notes.iter().find(|note| note.path == path) else {
        return invalid("not a note this pass read".to_owned());
    };
    if draft.new.is_empty() {
        return invalid("new is empty".to_owned());
    }
    if draft.kind == ProposalKind::Append && !draft.old.is_empty() {
        return invalid("an append must have an empty old".to_owned());
    }
    if draft.kind == ProposalKind::Create {
        return invalid("a pass cannot create a note".to_owned());
    }
    if draft.evidence.is_empty() {
        return invalid("no evidence".to_owned());
    }
    let mut texts = Vec::new();
    for evidence in &draft.evidence {
        match index.turn(&evidence.session, evidence.ordinal)? {
            Some(turn) => texts.push(turn.text),
            None => {
                return invalid(format!(
                    "evidence {} {} is not in the index",
                    evidence.session, evidence.ordinal
                ))
            }
        }
    }
    if draft.kind == ProposalKind::Replace {
        let new = squash(&draft.new);
        if !texts.iter().any(|text| quotes(&new, &squash(text))) {
            let longest = texts
                .iter()
                .map(|text| longest_quote(&new, &squash(text)))
                .max()
                .unwrap_or(0);
            return invalid(format!(
                "new does not quote a cited turn: it needs {QUOTE_MIN_CHARS} characters in a row copied from one, or one's whole text (longest found: {longest})"
            ));
        }
    }
    if let Some(excerpt) = &note.excerpt {
        match draft.kind {
            ProposalKind::Replace => {
                let at = find(&note.bytes, draft.old.as_bytes());
                let inside = at.is_some_and(|at| {
                    at >= excerpt.bytes.start && at + draft.old.len() <= excerpt.bytes.end
                });
                if at.is_some() && !inside {
                    return invalid(format!(
                        "old is outside the excerpt the pass read ({})",
                        excerpt.label()
                    ));
                }
            }
            ProposalKind::Create => {}
            ProposalKind::Append => {
                if !excerpt.reaches_end(note.bytes.len()) {
                    return invalid(format!(
                        "an append needs the note's end, which the excerpt ({}) does not reach",
                        excerpt.label()
                    ));
                }
            }
        }
    }
    let after = match notes::change(
        &note.bytes,
        draft.kind,
        draft.old.as_bytes(),
        draft.new.as_bytes(),
    ) {
        Ok(after) => after,
        Err(err @ (Error::OldNotUnique(_) | Error::ReplaceOverCap { .. })) => {
            return invalid(err.to_string())
        }
        Err(err) => return Err(err),
    };
    if let Some(sessions) = outside {
        if draft.kind == ProposalKind::Append && draft.new.len() > OUTSIDE_APPEND_CAP {
            return invalid(format!("append over {OUTSIDE_APPEND_CAP} bytes"));
        }
        if let Some(cited) = draft
            .evidence
            .iter()
            .find(|evidence| !sessions.contains(&evidence.session))
        {
            return invalid(format!(
                "evidence {} is not a session of this prompt",
                cited.session
            ));
        }
    }
    Ok(Checked::Valid(Proposal {
        id: Ulid::generate(),
        pass: pass.to_owned(),
        note: path.to_path_buf(),
        kind: draft.kind,
        before_hash: hex(&sha256(&note.bytes)),
        after_hash: hex(&sha256(&after)),
        old: draft.old.clone(),
        new: draft.new.clone(),
        evidence: draft.evidence.clone(),
        reason: draft.reason.clone(),
    }))
}

/// The fewest characters a `replace`'s `new` must copy in a row from a
/// cited turn whose text is longer.
pub const QUOTE_MIN_CHARS: usize = 32;

/// `text` as quotes compare it: the two-character escapes `\n`, `\r` and
/// `\t` as whitespace and `\"` as `"` (a tool call's input is indexed as
/// JSON), each run of whitespace as one space, trimmed.
pub fn squash(text: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut space = false;
    while let Some(c) = chars.next() {
        let c = match (c, chars.peek()) {
            ('\\', Some('n' | 'r' | 't')) => {
                chars.next();
                ' '
            }
            ('\\', Some('"')) => {
                chars.next();
                '"'
            }
            (c, _) => c,
        };
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out
}

/// Whether `new` holds [`QUOTE_MIN_CHARS`] characters in a row of `turn`,
/// or all of `turn` when it is shorter; both squashed.
pub fn quotes(new: &[char], turn: &[char]) -> bool {
    if turn.is_empty() {
        return false;
    }
    if turn.len() <= QUOTE_MIN_CHARS {
        return contains_run(new, turn);
    }
    has_common_run(new, turn, QUOTE_MIN_CHARS)
}

/// The longest run of characters `new` shares with `turn`, both squashed.
pub fn longest_quote(new: &[char], turn: &[char]) -> usize {
    let (mut low, mut high) = (0, new.len().min(turn.len()));
    while low < high {
        let mid = (low + high).div_ceil(2);
        if has_common_run(new, turn, mid) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

fn has_common_run(new: &[char], turn: &[char], len: usize) -> bool {
    if len == 0 {
        return true;
    }
    if new.len() < len {
        return false;
    }
    let turn: String = turn.iter().collect();
    new.windows(len)
        .any(|window| turn.contains(&window.iter().collect::<String>()))
}

fn contains_run(haystack: &[char], needle: &[char]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// `<workspace>/.hippocamp/proposals/<id>.json`.
pub fn path(ws: &Workspace, id: Ulid) -> PathBuf {
    ws.hippocamp_dir()
        .join("proposals")
        .join(format!("{id}.json"))
}

/// Writes `proposals/<id>.json` once: an existing file is never replaced.
pub fn write(ws: &Workspace, proposal: &Proposal) -> Result<()> {
    let file = path(ws, proposal.id);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))?;
    }
    write_new(&file, &encode(proposal)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dream::tests::{Setup, NOTE, NOTE_PATH, TURN_1};

    #[test]
    fn the_first_array_may_follow_other_text() {
        assert_eq!(first_array("[]").unwrap(), Vec::<Value>::new());
        let items = first_array("Here: [note] then [{\"a\": 1}] and [2]").unwrap();
        assert_eq!(items, [serde_json::json!({"a": 1})]);
        assert!(first_array("not json").is_none());
        assert!(first_array("{\"verdict\": \"confirm\"}").is_none());
    }

    fn draft(note: &str, kind: ProposalKind, old: &str, new: &str, ordinals: &[u32]) -> Draft {
        Draft {
            note: note.into(),
            kind,
            old: old.into(),
            new: new.into(),
            evidence: ordinals
                .iter()
                .map(|&ordinal| Evidence {
                    session: "s1".into(),
                    ordinal,
                })
                .collect(),
            reason: "because".into(),
        }
    }

    fn checked(s: &Setup, draft: &Draft) -> Checked {
        let note = NoteInput::read(&s.ws, Path::new(NOTE_PATH)).unwrap();
        check(&s.ws, &s.index(), "verify", &[note], draft, None).unwrap()
    }

    fn why(s: &Setup, draft: &Draft) -> String {
        match checked(s, draft) {
            Checked::Invalid(why) => why,
            Checked::Valid(proposal) => panic!("valid: {proposal:?}"),
        }
    }

    #[test]
    fn a_valid_replace_gets_an_id_and_both_hashes() {
        let s = Setup::new();
        let Checked::Valid(proposal) = checked(
            &s,
            &draft(
                NOTE_PATH,
                ProposalKind::Replace,
                "ran on 3 November",
                TURN_1,
                &[1],
            ),
        ) else {
            panic!("invalid")
        };
        assert_eq!(proposal.pass, "verify");
        assert_eq!(proposal.note, Path::new(NOTE_PATH));
        assert_eq!(proposal.before_hash, hex(&sha256(NOTE.as_bytes())));
        let after = NOTE.replace("ran on 3 November", TURN_1);
        assert_eq!(proposal.after_hash, hex(&sha256(after.as_bytes())));
        assert_eq!(proposal.reason, "because");
    }

    /// A replace of a dated sentence citing turn 0 of session `s3`,
    /// whose text is `turn`.
    fn citing(s: &Setup, turn: &str, new: &str) -> Checked {
        s.add_session_turns(
            "s3",
            &[(
                crate::dream::tests::at("2026-11-08T02:00:00Z"),
                crate::adapters::Role::Assistant,
                turn,
            )],
            &[],
            None,
        );
        let mut d = draft(
            NOTE_PATH,
            ProposalKind::Replace,
            "ran on 3 November",
            new,
            &[],
        );
        d.evidence = vec![Evidence {
            session: "s3".into(),
            ordinal: 0,
        }];
        checked(s, &d)
    }

    #[test]
    fn a_quote_is_a_long_enough_run_of_a_cited_turn_with_whitespace_squashed() {
        let s = Setup::new();
        // A long turn: 32 characters in a row are enough, across a line
        // break and indentation.
        let turn = "The deploy ran on 7 November at 03:30 and the queue\n\n    drained by 04:10, per the runbook. Nothing else changed.";
        let valid = citing(&s, turn, "drained by 04:10, per the runbook");
        assert!(matches!(valid, Checked::Valid(_)), "{valid:?}");
        drop(s);
        let s = Setup::new();
        let wrapped = turn.replace("\n\n    ", "\n");
        let valid = citing(&s, &wrapped, "queue drained by 04:10, per the runbook");
        assert!(matches!(valid, Checked::Valid(_)), "{valid:?}");
        // A tool call's input is indexed as JSON: `\n` and `\"` read as a
        // line break and a quote.
        drop(s);
        let s = Setup::new();
        let json = r#"Bash {"command":"echo \"queue drained\nby 04:10, per the runbook\""}"#;
        let valid = citing(&s, json, "\"queue drained by 04:10, per the");
        assert!(matches!(valid, Checked::Valid(_)), "{valid:?}");
        // Too short a run of a long turn.
        drop(s);
        let s = Setup::new();
        match citing(&s, turn, "ran on 7 November at 03:30") {
            Checked::Invalid(why) => assert!(why.ends_with("(longest found: 26)"), "{why}"),
            valid => panic!("{valid:?}"),
        }
    }

    #[test]
    fn an_excerpt_bounds_old_and_appends() {
        let s = Setup::new();
        let whole = NoteInput::read(&s.ws, Path::new(NOTE_PATH)).unwrap();
        let head = whole.excerpted(0..42);
        assert_eq!(
            head.shown(),
            "# Topic\n\nThe migration ran on 3 November.\n"
        );
        let tail = whole.excerpted(42..NOTE.len());
        let run = |note: &NoteInput, kind: ProposalKind, old: &str, new: &str| {
            check(
                &s.ws,
                &s.index(),
                "verify",
                std::slice::from_ref(note),
                &draft(NOTE_PATH, kind, old, new, &[1]),
                None,
            )
            .unwrap()
        };
        let inside = run(&head, ProposalKind::Replace, "ran on 3 November", TURN_1);
        assert!(matches!(inside, Checked::Valid(_)), "{inside:?}");
        assert_eq!(
            run(&head, ProposalKind::Replace, "planned in October", TURN_1),
            Checked::Invalid("old is outside the excerpt the pass read (lines 1-3 of 5)".into())
        );
        assert_eq!(
            run(&head, ProposalKind::Append, "", "More.\n"),
            Checked::Invalid(
                "an append needs the note's end, which the excerpt (lines 1-3 of 5) does not reach"
                    .into()
            )
        );
        let append = run(&tail, ProposalKind::Append, "", "More.\n");
        assert!(matches!(append, Checked::Valid(_)), "{append:?}");
    }

    #[test]
    fn an_append_needs_no_quote_but_an_empty_old() {
        let s = Setup::new();
        let valid = checked(
            &s,
            &draft(NOTE_PATH, ProposalKind::Append, "", "\nMore.\n", &[0]),
        );
        assert!(matches!(valid, Checked::Valid(_)), "{valid:?}");
        assert_eq!(
            why(&s, &draft(NOTE_PATH, ProposalKind::Append, "x", "y", &[0])),
            "an append must have an empty old"
        );
    }

    #[test]
    fn each_failed_check_says_why() {
        let s = Setup::new();
        let replace = |note: &str, old: &str, new: &str, ordinals: &[u32]| {
            why(&s, &draft(note, ProposalKind::Replace, old, new, ordinals))
        };
        assert!(replace("../etc/passwd", "a", TURN_1, &[1]).contains("not a note inside notes_dir"));
        assert!(replace(".hippocamp/hippocamp.toml", "a", TURN_1, &[1])
            .contains("not a note inside notes_dir"));
        s.add_note("notes/Other.md", "# Other\n");
        assert_eq!(
            replace("notes/Other.md", "a", TURN_1, &[1]),
            "not a note this pass read"
        );
        assert_eq!(
            replace(NOTE_PATH, "ran on 3 November", "", &[1]),
            "new is empty"
        );
        assert_eq!(
            replace(NOTE_PATH, "ran on 3 November", TURN_1, &[]),
            "no evidence"
        );
        assert_eq!(
            replace(NOTE_PATH, "ran on 3 November", TURN_1, &[9]),
            "evidence s1 9 is not in the index"
        );
        assert_eq!(
            replace(NOTE_PATH, "ran on 3 November", "ran on 7 November", &[1]),
            "new does not quote a cited turn: it needs 32 characters in a row copied from one, or one's whole text (longest found: 17)"
        );
        assert!(replace(NOTE_PATH, "on", TURN_1, &[1]).contains("occurs 2 times"));
        let long = format!("{TURN_1} {}", "x".repeat(NOTE.len()));
        assert!(replace(NOTE_PATH, "ran on 3 November", &long, &[1]).contains("replace too large"));
    }

    #[test]
    fn outside_core_an_append_is_capped_and_evidence_must_be_from_the_prompt() {
        let s = Setup::new();
        s.add_session(
            "s2",
            crate::dream::tests::at("2026-11-08T02:00:00Z"),
            &[],
            None,
        );
        let note = NoteInput::read(&s.ws, Path::new(NOTE_PATH)).unwrap();
        let sessions: BTreeSet<String> = ["s1".to_owned()].into();
        let run = |draft: &Draft, outside: Option<&BTreeSet<String>>| {
            check(
                &s.ws,
                &s.index(),
                "keeper",
                std::slice::from_ref(&note),
                draft,
                outside,
            )
            .unwrap()
        };
        let at_cap = "x".repeat(OUTSIDE_APPEND_CAP);
        let over = "x".repeat(OUTSIDE_APPEND_CAP + 1);
        let append = |new: &str| draft(NOTE_PATH, ProposalKind::Append, "", new, &[0]);
        assert!(matches!(
            run(&append(&at_cap), Some(&sessions)),
            Checked::Valid(_)
        ));
        assert_eq!(
            run(&append(&over), Some(&sessions)),
            Checked::Invalid("append over 2048 bytes".into())
        );
        assert!(matches!(run(&append(&over), None), Checked::Valid(_)));
        let mut foreign = append("\nMore.\n");
        foreign.evidence[0].session = "s2".into();
        assert_eq!(
            run(&foreign, Some(&sessions)),
            Checked::Invalid("evidence s2 is not a session of this prompt".into())
        );
        assert!(matches!(run(&foreign, None), Checked::Valid(_)));
        // The new checks run after the old ones.
        let mut both = append(&over);
        both.old = "x".into();
        assert_eq!(
            run(&both, Some(&sessions)),
            Checked::Invalid("an append must have an empty old".into())
        );
    }

    #[test]
    fn a_proposal_file_is_written_once() {
        let s = Setup::new();
        let Checked::Valid(proposal) = checked(
            &s,
            &draft(
                NOTE_PATH,
                ProposalKind::Replace,
                "ran on 3 November",
                TURN_1,
                &[1],
            ),
        ) else {
            panic!("invalid")
        };
        write(&s.ws, &proposal).unwrap();
        assert_eq!(Proposal::load(&path(&s.ws, proposal.id)).unwrap(), proposal);
        assert!(write(&s.ws, &proposal).is_err());
    }
}

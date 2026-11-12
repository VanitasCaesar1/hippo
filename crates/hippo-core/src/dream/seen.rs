//! A person's mark on an applied dreaming change: "looks right".
//!
//! A receipt is the reviewer's record and is never rewritten. When the
//! person has read an applied change and agrees with it, `hippo review
//! <receipt> --ok` writes `.hippo/reviews/<receipt id>.json`:
//! `{"protocol": "HIPPO_V1", "receipt": <id>, "verdict": "looks-right",
//! "by": "person", "at": <time>}`. The desktop app's Home lists applied
//! changes with no such mark and no revert under "Needs you"; marking one
//! takes it off the list. Undoing a change is `hippo revert`, as before.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::config::Workspace;
use crate::error::{Error, Result};
use crate::records::{list_receipts, Verdict};

/// One review mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub protocol: String,
    pub receipt: Ulid,
    /// `looks-right`.
    pub verdict: String,
    /// `person`.
    pub by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// `.hippo/reviews/`.
pub fn dir(ws: &Workspace) -> PathBuf {
    ws.hippo_dir().join("reviews")
}

/// Marks the applied change of confirm receipt `receipt` as looked at and
/// right. Refuses a receipt that does not exist, is not a confirm, or was
/// not applied. Marking twice keeps the first mark. Returns the mark's path.
pub fn mark(ws: &Workspace, receipt: Ulid, now: OffsetDateTime) -> Result<PathBuf> {
    let found = list_receipts(ws)?
        .into_iter()
        .filter_map(|l| l.record.ok())
        .find(|r| r.id == receipt)
        .ok_or_else(|| Error::InvalidProposal(format!("no receipt {receipt}")))?;
    if found.verdict != Verdict::Confirm || found.applied.is_none() {
        return Err(Error::InvalidProposal(format!(
            "receipt {receipt} is not an applied change; only an applied change can be marked"
        )));
    }
    let dir = dir(ws);
    fs::create_dir_all(&dir).map_err(|e| Error::io_at(&dir, e))?;
    let path = dir.join(format!("{receipt}.json"));
    if path.exists() {
        return Ok(path);
    }
    let seen = Seen {
        protocol: crate::PROTOCOL.to_owned(),
        receipt,
        verdict: "looks-right".to_owned(),
        by: "person".to_owned(),
        at: now,
    };
    let mut text = serde_json::to_string_pretty(&seen)?;
    text.push('\n');
    crate::util::write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

/// The receipts a person has marked. A mark that does not parse is left out.
pub fn marked(ws: &Workspace) -> Result<HashSet<Ulid>> {
    let dir = dir(ws);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(e) => return Err(Error::io_at(&dir, e)),
    };
    let mut out = HashSet::new();
    for entry in entries.flatten() {
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Ok(seen) = serde_json::from_str::<Seen>(&text) {
            if seen.protocol == crate::PROTOCOL {
                out.insert(seen.receipt);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(root: &std::path::Path) -> Workspace {
        Workspace {
            root: root.to_path_buf(),
            notes_dir: "notes".into(),
            entry_note: "notes/START HERE.md".into(),
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
    fn a_mark_needs_an_applied_confirm_receipt_and_is_kept_once() {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        let receipts = ws.hippo_dir().join("receipts");
        fs::create_dir_all(&receipts).unwrap();
        let now = OffsetDateTime::now_utc();
        let write = |id: Ulid, verdict: &str, applied: bool| {
            let text = serde_json::json!({
                "protocol": "HIPPO_V1", "id": id.to_string(), "proposal": Ulid::from_parts(7, 7).to_string(),
                "proposal_hash": "a", "before_hash": "b", "after_hash": "c",
                "reviewer": "r", "invocation": "i", "verdict": verdict, "notes": "",
                "at": "2026-11-09T00:35:00Z",
                "applied": if applied { serde_json::json!("2026-11-09T00:35:01Z") } else { serde_json::Value::Null },
                "snapshot": null,
            });
            fs::write(receipts.join(format!("{id}.json")), text.to_string()).unwrap();
        };
        let (applied, rejected) = (Ulid::from_parts(1, 1), Ulid::from_parts(2, 2));
        write(applied, "confirm", true);
        write(rejected, "reject", false);
        assert!(marked(&ws).unwrap().is_empty());
        assert!(mark(&ws, Ulid::from_parts(3, 3), now).is_err());
        assert!(mark(&ws, rejected, now).is_err());
        let path = mark(&ws, applied, now).unwrap();
        let first = fs::read_to_string(&path).unwrap();
        mark(&ws, applied, now + time::Duration::hours(1)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), first);
        assert_eq!(marked(&ws).unwrap(), HashSet::from([applied]));
    }
}

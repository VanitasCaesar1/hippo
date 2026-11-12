//! The crate's error type.

use std::path::{Path, PathBuf};

/// Every error hippocamp-core returns. Where the design fixes a message
/// (`context over budget`, `no index`, `no declared session`), it is used
/// verbatim.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The required context parts do not fit the budget.
    #[error("context over budget: {required_bytes} > {budget}")]
    OverBudget {
        required_bytes: usize,
        budget: usize,
    },

    /// The proposal file does not hash to the receipt's `proposal_hash`.
    #[error("proposal file does not match the receipt's proposal_hash")]
    ProposalHashMismatch,

    /// The receipt's verdict is not `confirm`.
    #[error("receipt verdict is not confirm")]
    VerdictNotConfirm,

    /// The note's current hash is not the proposal's `before_hash`.
    #[error("note changed since the proposal: expected {expected}, found {actual}")]
    NoteChanged { expected: String, actual: String },

    /// The path is not a regular file inside `notes_dir`, or is under `.hippocamp/`.
    #[error("not a note inside notes_dir: {}", .0.display())]
    PathOutsideNotes(PathBuf),

    /// A `replace` change's `old` text occurs this many times, not once.
    #[error("old text occurs {0} times; replace needs exactly one")]
    OldNotUnique(usize),

    /// A `replace` change's `old` plus `new` exceed half the note's bytes.
    #[error("replace too large: old + new is {change} bytes, over half of the note's {note}")]
    ReplaceOverCap { change: usize, note: usize },

    /// `config.toml` names no workspace: hippocamp reads transcripts but has no
    /// notes folder yet, so dreaming, context and hooks have nothing to work on.
    #[error("no notes folder yet; run hippocamp init --workspace <dir> --notes-dir <dir> --entry-note <note>")]
    NoWorkspace,

    /// `$HIPPOCAMP_HOME/config.toml` does not exist: nothing is set up yet.
    #[error("hippocamp is not set up yet (no {}): run hippocamp to find your agents' history and index it", .0.display())]
    NotSetUp(PathBuf),

    /// The transcript index does not exist.
    #[error("no index; run hippocamp ingest")]
    MissingIndex,

    /// A record's `protocol` is missing or not `HIPPOCAMP_V1`.
    #[error(
        "expected protocol {}, found {}",
        crate::PROTOCOL,
        .found.as_deref().unwrap_or("none")
    )]
    Protocol { found: Option<String> },

    /// A session id does not match `[A-Za-z0-9_-]{1,128}`.
    #[error("invalid session id {0:?}")]
    InvalidSessionId(String),

    /// No declared session record has this id.
    #[error("no declared session {0}")]
    NoDeclaredSession(String),

    /// A proposal breaks the Records rules for its kind.
    #[error("invalid proposal: {0}")]
    InvalidProposal(String),

    /// A `hippocamp config` change that cannot be made.
    #[error("{0}")]
    Setting(String),

    /// The model command failed, timed out or replied without a result.
    #[error("model error: {0}")]
    Model(String),

    /// An eval set fails one of its checks.
    #[error("eval set: {0}")]
    EvalSet(String),

    /// A pack breaks a loader rule (a link, an oversized file, a duplicate pass name).
    #[error("{0}")]
    Pack(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}

/// A result with the crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// An I/O error whose message names the path it happened on.
    pub(crate) fn io_at(path: &Path, err: std::io::Error) -> Error {
        Error::Io(std::io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_the_design_fixes() {
        let over = Error::OverBudget {
            required_bytes: 30000,
            budget: 25600,
        };
        assert_eq!(over.to_string(), "context over budget: 30000 > 25600");
        assert_eq!(
            Error::MissingIndex.to_string(),
            "no index; run hippocamp ingest"
        );
        assert_eq!(
            Error::NoDeclaredSession("ab12".into()).to_string(),
            "no declared session ab12"
        );
        assert_eq!(
            Error::EvalSet("recall question 3: no expected turn".into()).to_string(),
            "eval set: recall question 3: no expected turn"
        );
        let old = Error::Protocol {
            found: Some("HIPPOCAMP_V0".into()),
        };
        assert_eq!(
            old.to_string(),
            "expected protocol HIPPOCAMP_V1, found HIPPOCAMP_V0"
        );
        let missing = Error::Protocol { found: None };
        assert_eq!(
            missing.to_string(),
            "expected protocol HIPPOCAMP_V1, found none"
        );
    }
}

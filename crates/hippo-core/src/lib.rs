//! hippo-core: the memory engine behind the `hippo` CLI.
//!
//! Module tree:
//!
//! - [`error`]: the crate's [`Error`](error::Error) and
//!   [`Result`](error::Result).
//! - [`adapters`]: Claude Code and Codex transcripts into sessions and
//!   turns.
//! - [`config`]: `$HIPPO_HOME/config.toml`, the workspace's
//!   `.hippo/hippo.toml`, and `init`.
//! - [`records`]: `HIPPO_V1` records with protocol-checked `save` and
//!   `load`, and the [`Confirmed`](records::Confirmed) gate of Invariant 1.
//! - [`notes`]: Markdown notes, links, the linter, snapshots, and
//!   [`apply`](notes::apply), the only dreaming write path.
//! - [`redact`]: [`redact`](redact::redact), the only way text enters the
//!   index.
//! - [`index`]: the transcript index, SQLite full-text search over
//!   redacted turns, rebuildable from the transcripts.
//! - [`ingest`]: walks the transcript roots under a lock, redacts, indexes
//!   and derives session records.
//! - [`rollup`]: a rebuildable cache of what each session did and how it
//!   ended, by 15-minute bucket, for fast per-day summaries.
//! - [`sessions`]: session records, declared and derived, and the session
//!   log.
//! - [`session_meta`]: a session's origin (working directory, parent,
//!   who started it), its title and what kind each turn is.
//! - [`context`]: the budgeted context block for session start.
//! - [`detect`]: what agent history this machine holds, before any index.
//! - [`doctor`]: the health checks of `hippo doctor`.
//! - [`packs`]: passes, prompts and personas as data, the `core` pack, pack
//!   formats, validation, sources, origins, the pack review, install, fixture
//!   tests and registry types.
//! - [`theme`]: the desktop theme format, its lenient reading of a user's
//!   file and the strict check of a theme pack's `theme.json`.
//! - [`hooks`]: the three Claude Code hooks in a workspace's
//!   `.claude/settings.json`, merged with what is there.
//! - [`model`]: isolated model invocation through `model_command`.
//! - [`dream`]: the nightly run, from the window to the report, and
//!   [`revert`](dream::revert).
//! - [`eval`]: `hippo eval`, the three suites over a synthetic set.

pub mod adapters;
pub mod ask;
pub mod brief;
pub mod config;
pub mod context;
pub mod detect;
pub mod doctor;
pub mod dream;
pub mod error;
pub mod eval;
pub mod hooks;
pub mod incidents;
pub mod index;
pub mod ingest;
pub mod lessons;
pub mod log_notes;
pub mod model;
pub mod notes;
pub mod packs;
pub mod records;
pub mod redact;
pub mod resume;
pub mod rollup;
pub mod session_meta;
pub mod sessions;
pub mod settings;
pub mod theme;
pub mod usage;
mod util;
pub use util::default_sigpipe;
pub mod xray;

/// Protocol version written into every record this crate produces.
pub const PROTOCOL: &str = "HIPPO_V1";

/// The lowercase hex SHA-256 of `bytes`, the form hashes take in records.
pub fn hash(bytes: &[u8]) -> String {
    util::hex(&util::sha256(bytes))
}

/// Default hard budget for injected context, in bytes.
pub const DEFAULT_CONTEXT_BUDGET: usize = 25 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_is_versioned() {
        assert!(PROTOCOL.ends_with("V1"));
    }

    #[test]
    fn hash_is_lowercase_hex_sha256() {
        assert_eq!(
            hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hash(b"").len(), 64);
    }
}

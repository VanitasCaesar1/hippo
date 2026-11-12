//! Whether hippo is set up: the full-window rows shown before it is.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use hippo_core::config::{Config, Workspace};
use hippo_core::error::Error;

/// What `$HIPPO_HOME` and the workspace it names hold. Error text is as the
/// engine gives it; the app shows it through [`crate::text::visible`].
// One value lives at a time, and the design fixes `Ready { cfg, ws }`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Setup {
    /// A configuration and a workspace that load.
    Ready { cfg: Config, ws: Workspace },
    /// A configuration that names no workspace: hippo reads the agents'
    /// history (the index, the rollup, the edition) and has no notes folder
    /// yet. A normal state, not an error.
    HistoryOnly { cfg: Config },
    /// `config.toml` is missing or does not load, or the workspace's
    /// `.hippo/hippo.toml` does not load although it exists.
    NotSetUp { home: PathBuf, error: String },
    /// The workspace directory or its `.hippo/hippo.toml` is missing.
    WorkspaceMissing { path: PathBuf },
    /// macOS (or the file system) refused a read: an unanswered or refused
    /// privacy prompt. `path` is what the app names to the user.
    AccessDenied { path: PathBuf },
}

/// Loads the configuration in `home`, then its workspace. Reads only.
pub fn load(home: &Path) -> Setup {
    let cfg = match Config::load_in(home) {
        Ok(cfg) => cfg,
        Err(e) if denied(&e) => {
            return Setup::AccessDenied {
                path: home.to_path_buf(),
            }
        }
        Err(e) => {
            return Setup::NotSetUp {
                home: home.to_path_buf(),
                error: e.to_string(),
            }
        }
    };
    let Some(root) = cfg.workspace.clone() else {
        return Setup::HistoryOnly { cfg };
    };
    match Workspace::load(&cfg) {
        Ok(ws) => Setup::Ready { cfg, ws },
        Err(e) if denied(&e) => Setup::AccessDenied { path: root },
        Err(Error::Io(io)) if io.kind() == ErrorKind::NotFound => {
            let settings = root.join(".hippo").join("hippo.toml");
            Setup::WorkspaceMissing {
                path: if root.is_dir() { settings } else { root },
            }
        }
        Err(e) => Setup::NotSetUp {
            home: home.to_path_buf(),
            error: e.to_string(),
        },
    }
}

impl Setup {
    /// The configuration, when one loaded: `Ready` and `HistoryOnly`.
    pub fn config(&self) -> Option<&Config> {
        match self {
            Setup::Ready { cfg, .. } | Setup::HistoryOnly { cfg } => Some(cfg),
            _ => None,
        }
    }

    /// The workspace, when there is one.
    pub fn workspace(&self) -> Option<&Workspace> {
        match self {
            Setup::Ready { ws, .. } => Some(ws),
            _ => None,
        }
    }
}

fn denied(e: &Error) -> bool {
    matches!(e, Error::Io(io) if io.kind() == ErrorKind::PermissionDenied)
}

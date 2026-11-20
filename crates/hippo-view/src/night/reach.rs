//! Which agents the start block reaches, and how: the agents whose hooks
//! file has hippo's session-start hook.

use std::path::Path;

use hippo_core::hooks::{self, Agent as HookAgent};
use serde::Serialize;

use super::{agent, Agent};

/// One agent the start block reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reach {
    pub agent: Agent,
    /// `at session start`, `at session start, through its hooks`.
    pub how: &'static str,
}

fn starts(path: &Path, a: HookAgent) -> bool {
    hooks::status_file(path, a)
        .map(|s| s.first().is_some_and(|(_, has)| *has))
        .unwrap_or(false)
}

/// The agents with hippo's session-start hook installed, for the
/// workspace at `ws_root`.
pub fn reaches(ws_root: &Path) -> Vec<Reach> {
    let mut out = Vec::new();
    if starts(&hooks::settings_path(ws_root), HookAgent::ClaudeCode) {
        out.push(Reach {
            agent: agent("claude-code"),
            how: "at session start",
        });
    }
    if let Some(home) = hooks::codex_home() {
        if starts(&hooks::codex_hooks_path(&home), HookAgent::Codex) {
            out.push(Reach {
                agent: agent("codex"),
                how: "at session start, through its hooks",
            });
        }
    }
    if let Some(p) = hooks::cursor_hooks_path() {
        if starts(&p, HookAgent::Cursor) {
            out.push(Reach {
                agent: agent("cursor"),
                how: "at chat start",
            });
        }
    }
    if let Some(p) = hooks::gemini_settings_path() {
        if starts(&p, HookAgent::Gemini) {
            out.push(Reach {
                agent: agent("gemini"),
                how: "at session start, through its hooks",
            });
        }
    }
    out
}

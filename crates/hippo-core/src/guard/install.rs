//! Where `hippo guard install` writes: each Claude Code account's user
//! settings (`<account>/settings.json`), Gemini CLI's
//! `~/.gemini/settings.json`, Codex's `$CODEX_HOME/hooks.json` and
//! Cursor's `~/.cursor/hooks.json`. Never a workspace's committed
//! `.claude/settings.json`: that would push the guard, its fail mode and
//! an absolute path onto every teammate.

use std::path::{Path, PathBuf};

use crate::hooks::GuardAgent;

/// The Claude Code account folders on this machine: `$CLAUDE_CONFIG_DIR`
/// when set, and every `~/.claude` and `~/.claude-*` folder.
pub fn claude_accounts(user_home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            out.push(dir);
        }
    }
    if let Ok(entries) = std::fs::read_dir(user_home) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let account = name == ".claude" || name.starts_with(".claude-");
            if account && e.path().is_dir() {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every settings file the guard goes into for `agents`, with its agent.
pub fn targets(user_home: &Path, agents: &[GuardAgent]) -> Vec<(GuardAgent, PathBuf)> {
    let mut out = Vec::new();
    for agent in agents {
        match agent {
            GuardAgent::ClaudeCode => out.extend(
                claude_accounts(user_home)
                    .into_iter()
                    .map(|a| (*agent, a.join("settings.json"))),
            ),
            GuardAgent::Gemini => {
                let dir = user_home.join(".gemini");
                if dir.is_dir() {
                    out.push((*agent, dir.join("settings.json")));
                }
            }
            GuardAgent::Cursor => {
                let dir = user_home.join(".cursor");
                if dir.is_dir() {
                    out.push((*agent, dir.join("hooks.json")));
                }
            }
            GuardAgent::Codex => {
                let dir = codex_home(user_home);
                if dir.is_dir() {
                    out.push((*agent, crate::hooks::codex_hooks_path(&dir)));
                }
            }
        }
    }
    out
}

/// Codex's home for `user_home`: `$CODEX_HOME` when set and not empty,
/// else `<user_home>/.codex`.
fn codex_home(user_home: &Path) -> PathBuf {
    match std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => user_home.join(".codex"),
    }
}

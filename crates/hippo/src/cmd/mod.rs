//! One module per command.

pub mod add;
pub mod ask;
pub mod brief;
pub mod card;
pub mod context;
pub mod detect;
pub mod doctor;
pub mod dream;
pub mod eval;
pub mod fleet;
pub mod hook;
pub mod hooks;
pub mod incidents;
pub mod ingest;
pub mod init;
pub mod lessons;
pub mod log;
pub mod packs;
pub mod recall;
pub mod revert;
pub mod review;
pub mod schedule;
pub mod session;
pub mod settings;
pub mod show;
pub mod start;
pub mod xray;

use std::time::Duration;

use anyhow::{bail, Result};
use hippo_core::config::{self, Config, Workspace};
use hippo_core::packs::install;
use hippo_core::packs::source::GitOptions;

/// The configuration in `$HIPPO_HOME` and the workspace it names: every
/// command finds the workspace this way, never from the current directory.
pub fn load() -> Result<(Config, Workspace)> {
    let config = Config::load()?;
    let workspace = Workspace::load(&config)?;
    Ok((config, workspace))
}

/// `text` with every run of whitespace, line breaks included, as one space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The shortest prefix of session `id`, at least 8 characters, that no
/// other top-level session in `ids` starts with: what `hippo xray`, `hippo
/// show session` and `hippo resume` accept.
pub fn short_id(id: &str, ids: &[String]) -> String {
    let chars: Vec<(usize, char)> = id.char_indices().collect();
    for n in 8..chars.len() {
        let prefix = &id[..chars[n].0];
        let shared = ids
            .iter()
            .any(|o| o != id && !o.contains("-agent-") && o.starts_with(prefix));
        if !shared {
            return prefix.to_owned();
        }
    }
    id.to_owned()
}

/// A full session id, from an id or a unique prefix of an indexed
/// top-level session's id: the one rule of `hippo show session`, `hippo
/// xray` and `hippo resume`.
pub fn resolve_session(index: &hippo_core::index::Index, id: &str) -> Result<String> {
    if index.source(id)?.is_some() {
        return Ok(id.to_owned());
    }
    let mut found: Vec<String> = index
        .sources()?
        .into_keys()
        .filter(|k| k.starts_with(id) && !k.contains("-agent-"))
        .collect();
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => bail!(
            "no indexed session {id} (hippo fleet lists today's sessions, hippo show day <date> a day's)"
        ),
        n => bail!("{n} sessions start with {id}; give more of the id"),
    }
}

/// The configuration and its workspace when it names one: for the commands
/// that work on the agents' history alone (`ingest`, `show session`, `show
/// day`), which a history-only configuration supports.
pub fn load_history() -> Result<(Config, Option<Workspace>)> {
    let config = Config::load()?;
    let workspace = Workspace::load_optional(&config)?;
    Ok((config, workspace))
}

/// [`load`], then finishes an interrupted pack update and prints what it
/// did: the first step of `hippo add` and of every `hippo packs` command
/// that works on installed packs.
pub fn load_recovered() -> Result<(Config, Workspace)> {
    let (config, workspace) = load()?;
    for line in install::recover(&workspace)? {
        println!("{line}");
    }
    Ok((config, workspace))
}

/// How the CLI runs git: https only, 120 seconds.
pub fn git_options() -> GitOptions {
    GitOptions {
        allow_file: false,
        timeout: Duration::from_secs(120),
    }
}

/// Prints `warning: <text>` when the dream job runs another binary than
/// this one (macOS only).
pub fn print_schedule_warning() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let (Some(home), Ok(exe)) = (config::user_home(), std::env::current_exe()) else {
        return;
    };
    let agents = home.join("Library").join("LaunchAgents");
    if let Some(text) = install::schedule_warning(&agents, &exe) {
        println!("warning: {text}");
    }
}

#[cfg(test)]
mod tests {
    use super::short_id;

    #[test]
    fn a_short_id_is_the_shortest_unique_prefix_of_eight_or_more() {
        let ids: Vec<String> = [
            "demo-t3-parent",
            "demo-t3-child1",
            "abcdef12-3456",
            "abcdef12-3456-agent-x",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(short_id("abcdef12-3456", &ids), "abcdef12");
        assert_eq!(short_id("demo-t3-parent", &ids), "demo-t3-p");
        assert_eq!(short_id("short", &ids), "short");
    }
}

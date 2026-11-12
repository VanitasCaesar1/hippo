//! How to pick a session up again: the command that resumes it, on its own
//! profile or another, and a short handoff.
//!
//! Nothing here writes. `claude --resume <id>` finds a session only under
//! `<config dir>/projects/<folder>/<id>.jsonl`, so resuming on another
//! profile needs the transcript there: the command copies it when the
//! person runs it (`cp -n`, which never overwrites), and hippocamp itself never
//! writes into an agent's folders.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::index::StoredTurn;

/// `s` quoted for a POSIX shell.
pub fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:@~,".contains(c))
    {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn q(p: &Path) -> String {
    quote(&p.to_string_lossy())
}

/// What a resume command needs to know of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target<'a> {
    /// hippocamp's session id: Claude Code's session id, or `codex-<uuid>`.
    pub id: &'a str,
    /// `claude-code` or `codex`.
    pub tool: &'a str,
    pub transcript: &'a Path,
    /// Where it ran; the command starts there.
    pub cwd: Option<&'a Path>,
}

/// Whether `dir` is Claude Code's default config dir, `~/.claude`.
fn is_default_dir(dir: &Path) -> bool {
    dir.file_name().and_then(|n| n.to_str()) == Some(".claude")
}

fn env_for(dir: &Path) -> String {
    if is_default_dir(dir) {
        String::new()
    } else {
        format!("CLAUDE_CONFIG_DIR={} ", q(dir))
    }
}

/// The command that resumes `t` on its own profile:
/// `cd '<cwd>' && CLAUDE_CONFIG_DIR=<dir> claude --resume <id>`, or
/// `codex resume <uuid>`. `None` for a sub-agent or an unknown tool.
pub fn command(t: &Target<'_>) -> Option<String> {
    let cd = t
        .cwd
        .map(|c| format!("cd {} && ", q(c)))
        .unwrap_or_default();
    match t.tool {
        "codex" => {
            let uuid = t.id.strip_prefix("codex-")?;
            Some(format!("{cd}codex resume {}", quote(uuid)))
        }
        "claude-code" => {
            if t.id.contains("-agent-") {
                return None;
            }
            let dir = crate::usage::config_dir_of(t.transcript)?;
            Some(format!(
                "{cd}{}claude --resume {}",
                env_for(&dir),
                quote(t.id)
            ))
        }
        _ => None,
    }
}

/// The command that resumes Claude Code session `t` on the profile whose
/// config dir is `other`: it copies the transcript into `other`'s
/// projects folder first (`cp -n`: never over an existing file; the
/// original stays), then resumes. `None` for Codex, a sub-agent, or a
/// transcript not under a `projects/<folder>/`.
pub fn command_on(t: &Target<'_>, other: &Path) -> Option<String> {
    if t.tool != "claude-code" || t.id.contains("-agent-") {
        return None;
    }
    let folder_dir = t.transcript.parent()?;
    let folder = folder_dir.file_name()?;
    if folder_dir.parent()?.file_name()?.to_str()? != "projects" {
        return None;
    }
    let own = crate::usage::config_dir_of(t.transcript)?;
    if own == other {
        return command(t);
    }
    let dest: PathBuf = other.join("projects").join(folder);
    let cd = t
        .cwd
        .map(|c| format!(" && cd {}", q(c)))
        .unwrap_or_default();
    Some(format!(
        "mkdir -p {dest} && cp -n {src} {dest}/{cd} && {env}claude --resume {id}",
        dest = q(&dest),
        src = q(t.transcript),
        env = env_for(other),
        id = quote(t.id),
    ))
}

/// The open items of a session: the unfinished entries of its last
/// `TodoWrite` (Claude Code) or `update_plan` (Codex) list.
pub fn open_items(turns: &[StoredTurn]) -> Vec<String> {
    for t in turns.iter().rev() {
        let Some(name) = t.tool_name.as_deref() else {
            continue;
        };
        let (key, field) = match name {
            "TodoWrite" => ("todos", "content"),
            "update_plan" => ("plan", "step"),
            _ => continue,
        };
        let Some(input) = t.text.strip_prefix(name).map(str::trim_start) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(input) else {
            continue;
        };
        let Some(list) = v.get(key).and_then(Value::as_array) else {
            continue;
        };
        return list
            .iter()
            .filter(|i| i.get("status").and_then(Value::as_str) != Some("completed"))
            .filter_map(|i| i.get(field).and_then(Value::as_str))
            .map(|s| cut(s, 200))
            .collect();
    }
    Vec::new()
}

fn cut(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max).collect();
    if let Some(at) = out.rfind(' ') {
        out.truncate(at);
    }
    out.push('…');
    out
}

/// A short handoff for whoever picks the session up: its title, the first
/// request, the last reply and the open items, each cut short.
pub fn handoff(title: &str, asked: Option<&str>, last: Option<&str>, open: &[String]) -> String {
    let mut out = format!("Picking up: {title}\n");
    if let Some(a) = asked.filter(|a| !a.trim().is_empty()) {
        out.push_str(&format!("\nFirst request: {}\n", cut(a, 600)));
    }
    if let Some(l) = last.filter(|l| !l.trim().is_empty()) {
        out.push_str(&format!("\nLast reply: {}\n", cut(l, 600)));
    }
    if !open.is_empty() {
        out.push_str("\nOpen items:\n");
        for o in open {
            out.push_str(&format!("- {o}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target<'a>(id: &'a str, tool: &'a str, path: &'a Path) -> Target<'a> {
        Target {
            id,
            tool,
            transcript: path,
            cwd: Some(Path::new("/work/my app")),
        }
    }

    #[test]
    fn commands_cd_to_the_session_and_name_the_profile() {
        let p = Path::new("/u/.claude/projects/-work-app/s1.jsonl");
        assert_eq!(
            command(&target("s1", "claude-code", p)).unwrap(),
            "cd '/work/my app' && claude --resume s1"
        );
        let p = Path::new("/u/.claude-work/projects/-work-app/s1.jsonl");
        assert_eq!(
            command(&target("s1", "claude-code", p)).unwrap(),
            "cd '/work/my app' && CLAUDE_CONFIG_DIR=/u/.claude-work claude --resume s1"
        );
        let p = Path::new("/u/.codex/sessions/2026/rollout-x.jsonl");
        assert_eq!(
            command(&target("codex-abc", "codex", p)).unwrap(),
            "cd '/work/my app' && codex resume abc"
        );
        let p = Path::new("/u/.claude/projects/-w/s1/subagents/agent-a.jsonl");
        assert_eq!(command(&target("s1-agent-a", "claude-code", p)), None);
    }

    #[test]
    fn resuming_elsewhere_copies_without_overwriting() {
        let p = Path::new("/u/.claude/projects/-work-app/s1.jsonl");
        let cmd = command_on(
            &target("s1", "claude-code", p),
            Path::new("/u/.claude-work"),
        )
        .unwrap();
        assert_eq!(
            cmd,
            "mkdir -p /u/.claude-work/projects/-work-app && cp -n /u/.claude/projects/-work-app/s1.jsonl /u/.claude-work/projects/-work-app/ && cd '/work/my app' && CLAUDE_CONFIG_DIR=/u/.claude-work claude --resume s1"
        );
        // To the default profile: no CLAUDE_CONFIG_DIR.
        let p = Path::new("/u/.claude-work/projects/-w/s1.jsonl");
        let cmd = command_on(&target("s1", "claude-code", p), Path::new("/u/.claude")).unwrap();
        assert!(cmd.ends_with("&& claude --resume s1"), "{cmd}");
        assert_eq!(quote("it's"), "'it'\\''s'");
    }

    fn tool(name: &str, text: &str) -> StoredTurn {
        StoredTurn {
            ordinal: 0,
            role: "tool".into(),
            ts: String::new(),
            tool_name: Some(name.into()),
            text: text.into(),
        }
    }

    #[test]
    fn open_items_are_the_last_lists_unfinished_entries() {
        let turns = vec![
            tool(
                "TodoWrite",
                r#"TodoWrite {"todos":[{"content":"old","status":"pending"}]}"#,
            ),
            tool(
                "TodoWrite",
                r#"TodoWrite {"todos":[{"content":"done","status":"completed"},{"content":"write tests","status":"in_progress"},{"content":"ship","status":"pending"}]}"#,
            ),
            tool("Bash", "Bash {}"),
        ];
        assert_eq!(open_items(&turns), vec!["write tests", "ship"]);
        let codex = vec![tool(
            "update_plan",
            r#"update_plan {"plan":[{"step":"a","status":"completed"},{"step":"b","status":"pending"}]}"#,
        )];
        assert_eq!(open_items(&codex), vec!["b"]);
        let h = handoff("Fix it", Some("please fix"), None, &["b".into()]);
        assert_eq!(
            h,
            "Picking up: Fix it\n\nFirst request: please fix\n\nOpen items:\n- b\n"
        );
    }
}

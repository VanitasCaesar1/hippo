//! `hippo hooks install|uninstall|status`: hippo's three hooks in an
//! agent's hooks file, for Claude Code a workspace's
//! `.claude/settings.json` and for Codex the user's `$CODEX_HOME/hooks.json`
//! (the same `hooks` shape).
//!
//! [`install_file`] adds, for `SessionStart`, `UserPromptSubmit` and
//! `SessionEnd`, a command hook running `hippo hook session-start`, `hippo
//! hook prompt` and `hippo hook session-end` (with `--agent codex` for
//! Codex), unless the event already has a hook whose command runs that
//! subcommand for that agent. Everything else in the file is kept, in its
//! order: the file is read into an order-keeping tree, the entries are
//! appended, and the tree is written back with two-space indentation,
//! atomically. A file that is not a JSON object is refused and left alone.
//! [`status_file`] reports which events have their hook. Codex's trust in
//! a hook is Codex's own: nothing here reads or writes its `config.toml`.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// Each hook event and the `hippo hook` subcommand it runs.
pub const HOOKS: [(&str, &str); 3] = [
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt"),
    ("SessionEnd", "session-end"),
];

/// The agent whose hooks file is meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Codex,
}

impl Agent {
    /// How hippo's command for `sub` ends: `hook <sub>`, and for Codex
    /// `hook <sub> --agent codex`.
    fn suffix(self, sub: &str) -> String {
        match self {
            Agent::ClaudeCode => format!("hook {sub}"),
            Agent::Codex => format!("hook {sub} --agent codex"),
        }
    }

    /// The group hippo adds for `sub`. Codex keeps the start block and the
    /// relevant notes in context rather than in a file
    /// (`additionalContextLimit: 0`; hippo holds them under the budget),
    /// and gives `SessionEnd` the longest timeout it allows, 3 seconds.
    fn entry(self, sub: &str) -> J {
        let mut hook = vec![
            ("type".into(), J::Scalar(Value::from("command"))),
            (
                "command".into(),
                J::Scalar(Value::from(format!("hippo {}", self.suffix(sub)))),
            ),
        ];
        if self == Agent::Codex {
            hook.push(match sub {
                "session-end" => ("timeout".into(), J::Scalar(Value::from(3))),
                _ => ("additionalContextLimit".into(), J::Scalar(Value::from(0))),
            });
        }
        J::Object(vec![("hooks".into(), J::Array(vec![J::Object(hook)]))])
    }
}

/// `<workspace>/.claude/settings.json`.
pub fn settings_path(workspace: &Path) -> PathBuf {
    workspace.join(".claude").join("settings.json")
}

/// Codex's home: `$CODEX_HOME` when set and not empty, else `~/.codex`.
pub fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => crate::config::user_home().map(|home| home.join(".codex")),
    }
}

/// `<codex home>/hooks.json`, the user layer's hooks file.
pub fn codex_hooks_path(codex_home: &Path) -> PathBuf {
    codex_home.join("hooks.json")
}

/// Whether `<codex home>/config.toml` declares hooks in a `[hooks]` table
/// (any key but `state`, where Codex keeps its trust). Codex then loads
/// both files and warns. A missing or unreadable file declares none.
pub fn codex_config_declares_hooks(codex_home: &Path) -> bool {
    let Ok(text) = fs::read_to_string(codex_home.join("config.toml")) else {
        return false;
    };
    let Ok(value) = text.parse::<toml::Table>() else {
        return false;
    };
    matches!(value.get("hooks"), Some(toml::Value::Table(hooks)) if hooks.keys().any(|k| k != "state"))
}

/// A JSON value that keeps the order of an object's keys.
#[derive(Debug, Clone, PartialEq)]
enum J {
    Object(Vec<(String, J)>),
    Array(Vec<J>),
    Scalar(Value),
}

impl<'de> Deserialize<'de> for J {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<J, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = J;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<J, E> {
                Ok(J::Scalar(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<J, E> {
                Ok(J::Scalar(Value::from(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<J, E> {
                Ok(J::Scalar(Value::from(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<J, E> {
                Ok(J::Scalar(Value::from(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<J, E> {
                Ok(J::Scalar(Value::String(v.to_owned())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<J, E> {
                Ok(J::Scalar(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<J, E> {
                Ok(J::Scalar(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<J, E> {
                Ok(J::Scalar(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<J, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(J::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<J, A::Error> {
                let mut entries: Vec<(String, J)> = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, J>()? {
                    match entries.iter_mut().find(|(key, _)| *key == k) {
                        // A repeated key: the last one wins, as serde_json does.
                        Some(slot) => slot.1 = v,
                        None => entries.push((k, v)),
                    }
                }
                Ok(J::Object(entries))
            }
        }
        d.deserialize_any(V)
    }
}

impl J {
    fn get_mut(&mut self, key: &str) -> Option<&mut J> {
        match self {
            J::Object(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = |n: usize| "  ".repeat(n);
        match self {
            J::Scalar(v) => out.push_str(&v.to_string()),
            J::Array(items) if items.is_empty() => out.push_str("[]"),
            J::Object(entries) if entries.is_empty() => out.push_str("{}"),
            J::Array(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    out.push_str(&pad(depth + 1));
                    item.write(out, depth + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(depth));
                out.push(']');
            }
            J::Object(entries) => {
                out.push_str("{\n");
                for (i, (k, v)) in entries.iter().enumerate() {
                    let _ = write!(out, "{}{}: ", pad(depth + 1), Value::String(k.clone()));
                    v.write(out, depth + 1);
                    out.push_str(if i + 1 < entries.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(depth));
                out.push('}');
            }
        }
    }

    /// Whether any hook command under this event's entries is hippo's,
    /// ending with `wanted` ([`Agent::suffix`]).
    fn runs(&self, wanted: &str) -> bool {
        let J::Array(entries) = self else {
            return false;
        };
        entries.iter().any(|entry| {
            let Some(J::Array(hooks)) = entry.get("hooks") else {
                return false;
            };
            hooks.iter().any(|h| match h.get("command") {
                Some(J::Scalar(Value::String(c))) => {
                    c.contains("hippo") && c.trim_end().ends_with(wanted)
                }
                _ => false,
            })
        })
    }
}

fn invalid(path: &Path, message: &str) -> Error {
    Error::io_at(
        path,
        io::Error::new(io::ErrorKind::InvalidData, message.to_owned()),
    )
}

fn read(path: &Path) -> Result<Option<J>> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Some(J::Object(Vec::new()))),
        Ok(text) => {
            let tree: J = serde_json::from_str(&text)?;
            match tree {
                J::Object(_) => Ok(Some(tree)),
                _ => Err(invalid(path, "is not a JSON object; left as it is")),
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io_at(path, e)),
    }
}

/// Which of the three events have their hook in `workspace`'s settings.
pub fn status(workspace: &Path) -> Result<Vec<(&'static str, bool)>> {
    status_file(&settings_path(workspace), Agent::ClaudeCode)
}

/// Which of the three events have `agent`'s hippo hook in the hooks file
/// at `path`.
pub fn status_file(path: &Path, agent: Agent) -> Result<Vec<(&'static str, bool)>> {
    let tree = read(path)?;
    Ok(HOOKS
        .iter()
        .map(|&(event, sub)| {
            let has = tree
                .as_ref()
                .and_then(|t| t.get("hooks"))
                .and_then(|h| h.get(event))
                .is_some_and(|e| e.runs(&agent.suffix(sub)));
            (event, has)
        })
        .collect())
}

/// Whether the Claude Code settings file at `path` has a `SessionStart`
/// hook that runs `hippo hook session-start` (the plugin's `--plugin`
/// form does not count). A missing or unreadable file does not have it.
pub fn file_runs_session_start(path: &Path) -> bool {
    matches!(read(path), Ok(Some(tree)) if tree
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .is_some_and(|e| e.runs(&Agent::ClaudeCode.suffix("session-start"))))
}

/// Adds the missing hooks to `workspace`'s settings and returns the events
/// it added; an empty list means they were all there and nothing was
/// written.
pub fn install(workspace: &Path) -> Result<Vec<&'static str>> {
    install_file(&settings_path(workspace), Agent::ClaudeCode)
}

/// Adds `agent`'s missing hooks to the hooks file at `path`, creating it
/// and its folder when missing, and returns the events it added; an empty
/// list means they were all there and nothing was written.
pub fn install_file(path: &Path, agent: Agent) -> Result<Vec<&'static str>> {
    let mut tree = read(path)?.unwrap_or(J::Object(Vec::new()));
    let mut added = Vec::new();
    if tree.get("hooks").is_none() {
        if let J::Object(entries) = &mut tree {
            entries.push(("hooks".into(), J::Object(Vec::new())));
        }
    }
    let Some(J::Object(events)) = tree.get_mut("hooks") else {
        return Err(invalid(
            path,
            "has a \"hooks\" that is not an object; left as it is",
        ));
    };
    for (event, sub) in HOOKS {
        let wanted = agent.suffix(sub);
        let has = events
            .iter()
            .find(|(k, _)| k == event)
            .is_some_and(|(_, v)| v.runs(&wanted));
        if has {
            continue;
        }
        let entry = agent.entry(sub);
        match events.iter_mut().find(|(k, _)| k == event) {
            Some((_, J::Array(items))) => items.push(entry),
            Some(_) => {
                return Err(invalid(
                    path,
                    &format!("has a hooks.{event} that is not an array; left as it is"),
                ))
            }
            None => events.push((event.to_owned(), J::Array(vec![entry]))),
        }
        added.push(event);
    }
    if added.is_empty() {
        return Ok(added);
    }
    let mut text = String::new();
    tree.write(&mut text, 0);
    text.push('\n');
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))?;
    }
    crate::util::write_atomic(path, text.as_bytes())?;
    Ok(added)
}

/// Whether hook object `h` is hippo's, its command ending with `wanted`.
fn is_hippo(h: &J, wanted: &str) -> bool {
    match h.get("command") {
        Some(J::Scalar(Value::String(c))) => c.contains("hippo") && c.trim_end().ends_with(wanted),
        _ => false,
    }
}

/// Removes hippo's three hooks from `workspace`'s settings, as
/// [`uninstall_file`] does.
pub fn uninstall(workspace: &Path) -> Result<Vec<&'static str>> {
    uninstall_file(&settings_path(workspace), Agent::ClaudeCode)
}

/// Removes `agent`'s three hippo hooks from the hooks file at `path` and
/// returns the events it removed one from; every other key and hook stays
/// as it was, and nothing is written when none was there. An entry left
/// with no hooks, and an event left with no entries, go too.
pub fn uninstall_file(path: &Path, agent: Agent) -> Result<Vec<&'static str>> {
    let Some(mut tree) = read(path)? else {
        return Ok(Vec::new());
    };
    let mut removed = Vec::new();
    let Some(J::Object(events)) = tree.get_mut("hooks") else {
        return Ok(removed);
    };
    for (event, sub) in HOOKS {
        let wanted = agent.suffix(sub);
        let Some((_, J::Array(entries))) = events.iter_mut().find(|(k, _)| k == event) else {
            continue;
        };
        let mut any = false;
        for entry in entries.iter_mut() {
            if let Some(J::Array(hooks)) = entry.get_mut("hooks") {
                let n = hooks.len();
                hooks.retain(|h| !is_hippo(h, &wanted));
                any |= hooks.len() != n;
            }
        }
        if any {
            entries.retain(|e| !matches!(e.get("hooks"), Some(J::Array(h)) if h.is_empty()));
            removed.push(event);
        }
    }
    if removed.is_empty() {
        return Ok(removed);
    }
    events.retain(|(_, v)| !matches!(v, J::Array(items) if items.is_empty()));
    let mut text = String::new();
    tree.write(&mut text, 0);
    text.push('\n');
    crate::util::write_atomic(path, text.as_bytes())?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_runs_session_start_only_with_the_workspace_hook() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert!(!file_runs_session_start(&path), "a missing file");
        let hook = |command: &str| {
            format!(
                r#"{{"hooks": {{"SessionStart": [{{"hooks": [{{"type": "command", "command": {}}}]}}]}}}}"#,
                Value::from(command)
            )
        };
        for (text, has) in [
            (hook("hippo hook session-start"), true),
            (hook("/opt/bin/hippo hook session-start "), true),
            (hook("PATH=\"$PATH:$HOME/.local/bin\"; command -v hippo >/dev/null 2>&1 && hippo hook session-start --plugin 2>/dev/null; exit 0"), false),
            (hook("hippo hook prompt"), false),
            (r#"{"hooks": {"SessionEnd": [{"hooks": [{"type": "command", "command": "hippo hook session-start"}]}]}}"#.to_owned(), false),
            ("not json".to_owned(), false),
            ("[1, 2]".to_owned(), false),
        ] {
            fs::write(&path, &text).unwrap();
            assert_eq!(file_runs_session_start(&path), has, "{text}");
        }
    }

    #[test]
    fn install_adds_three_hooks_once_and_keeps_everything_else_in_order() {
        let ws = tempfile::tempdir().unwrap();
        // No file: written fresh.
        assert_eq!(status(ws.path()).unwrap().iter().filter(|s| s.1).count(), 0);
        assert_eq!(install(ws.path()).unwrap().len(), 3);
        assert!(status(ws.path()).unwrap().iter().all(|s| s.1));
        assert!(install(ws.path()).unwrap().is_empty());

        // An existing file keeps its keys, its order and its own hooks.
        let path = settings_path(ws.path());
        let before = r#"{
  "permissions": {"allow": ["Bash(ls)"], "deny": []},
  "model": "opus",
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "echo hi"}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "/usr/local/bin/hippo hook session-end"}]}]
  },
  "env": {"B": "2", "A": "1"}
}"#;
        fs::write(&path, before).unwrap();
        let added = install(ws.path()).unwrap();
        assert_eq!(added, ["SessionStart", "UserPromptSubmit"]);
        let text = fs::read_to_string(&path).unwrap();
        let keys: Vec<usize> = [
            "\"permissions\"",
            "\"model\"",
            "\"hooks\"",
            "\"env\"",
            "\"B\"",
            "\"A\"",
        ]
        .iter()
        .map(|k| text.find(k).unwrap())
        .collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "{text}");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        assert_eq!(
            value["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "echo hi"
        );
        assert_eq!(value["hooks"]["SessionEnd"].as_array().unwrap().len(), 1);
        assert_eq!(value["permissions"]["allow"][0], "Bash(ls)");

        // Uninstall takes only hippo's hooks out.
        let removed = uninstall(ws.path()).unwrap();
        assert_eq!(removed, ["SessionStart", "UserPromptSubmit", "SessionEnd"]);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            value["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "echo hi"
        );
        assert!(value["hooks"].get("SessionEnd").is_none());
        assert!(value["hooks"].get("UserPromptSubmit").is_none());
        assert_eq!(value["model"], "opus");
        assert!(status(ws.path()).unwrap().iter().all(|s| !s.1));
        assert!(uninstall(ws.path()).unwrap().is_empty());

        // Not an object: refused, untouched.
        fs::write(&path, "[1]").unwrap();
        assert!(install(ws.path()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "[1]");
    }

    #[test]
    fn codex_hooks_are_their_own_and_leave_claude_code_hooks_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = codex_hooks_path(dir.path());
        // A file with the person's hooks and a Claude Code-style hippo hook.
        let before = r#"{
  "description": "mine",
  "hooks": {
    "SessionStart": [{"matcher": "startup", "hooks": [{"type": "command", "command": "echo hi"}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "hippo hook session-end"}]}]
  }
}"#;
        fs::write(&path, before).unwrap();
        assert!(status_file(&path, Agent::Codex)
            .unwrap()
            .iter()
            .all(|s| !s.1));
        assert_eq!(install_file(&path, Agent::Codex).unwrap().len(), 3);
        assert!(install_file(&path, Agent::Codex).unwrap().is_empty());
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["description"], "mine");
        let start = &value["hooks"]["SessionStart"];
        assert_eq!(start[0]["hooks"][0]["command"], "echo hi");
        assert_eq!(
            start[1]["hooks"][0]["command"],
            "hippo hook session-start --agent codex"
        );
        assert_eq!(start[1]["hooks"][0]["additionalContextLimit"], 0);
        assert_eq!(
            value["hooks"]["UserPromptSubmit"][0]["hooks"][0]["additionalContextLimit"],
            0
        );
        let end = &value["hooks"]["SessionEnd"];
        assert_eq!(end.as_array().unwrap().len(), 2);
        assert_eq!(end[1]["hooks"][0]["timeout"], 3);
        // The Claude Code status does not count Codex's hooks.
        assert_eq!(
            status_file(&path, Agent::ClaudeCode).unwrap(),
            [
                ("SessionStart", false),
                ("UserPromptSubmit", false),
                ("SessionEnd", true)
            ]
        );

        // Uninstall takes out Codex's three only.
        assert_eq!(uninstall_file(&path, Agent::Codex).unwrap().len(), 3);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["hooks"]["SessionEnd"][0]["hooks"][0]["command"],
            "hippo hook session-end"
        );
        assert!(value["hooks"].get("UserPromptSubmit").is_none());
        assert!(uninstall_file(&path, Agent::Codex).unwrap().is_empty());

        // A wrong-typed event is refused and left alone.
        let bad = r#"{"hooks": {"SessionStart": {}}}"#;
        fs::write(&path, bad).unwrap();
        assert!(install_file(&path, Agent::Codex).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), bad);
    }

    #[test]
    fn a_codex_config_declares_hooks_only_outside_its_trust_state() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!codex_config_declares_hooks(dir.path()));
        let config = dir.path().join("config.toml");
        fs::write(&config, "model = \"m\"\n[hooks.state.\"/x/hooks.json:session_start:0:0\"]\ntrusted_hash = \"h\"\n").unwrap();
        assert!(!codex_config_declares_hooks(dir.path()));
        fs::write(&config, "[[hooks.SessionStart]]\n[[hooks.SessionStart.hooks]]\ntype = \"command\"\ncommand = \"echo\"\n").unwrap();
        assert!(codex_config_declares_hooks(dir.path()));
    }
}

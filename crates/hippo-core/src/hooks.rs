//! `hippo hooks install|uninstall|status`: hippo's three hooks in an
//! agent's hooks file, for Claude Code a workspace's
//! `.claude/settings.local.json` and for Codex the user's
//! `$CODEX_HOME/hooks.json` (the same `hooks` shape).
//!
//! [`install_file`] adds, for `SessionStart`, `UserPromptSubmit` and
//! `SessionEnd`, a command hook running `<hippo> hook session-start`,
//! `<hippo> hook prompt` and `<hippo> hook session-end` (with `--agent
//! codex` for Codex), unless the event already has a hook whose command
//! runs that subcommand for that agent. `<hippo>` is the absolute path of
//! the binary ([`binary_path`]), so a `hippo` placed earlier on `PATH`
//! never sees a session (`docs/designs/common.md`, rule 4); an existing
//! entry that runs another path, or a bare `hippo`, is rewritten to it.
//! Everything else in the file is kept, in its order: the file is read
//! into an order-keeping tree, the entries are appended, and the tree is
//! written back with two-space indentation, atomically. A file that is not
//! a JSON object is refused and left alone.
//!
//! Claude Code's hooks live in `.claude/settings.local.json`, which is the
//! person's own. `.claude/settings.json` is the file teams commit; an
//! absolute path there would break for teammates and name the owner's home
//! folder, so hippo writes nothing there, and [`claude_code_shared`] finds
//! the entries an older hippo put there, for `hooks install` to move.
//! [`status_file`] reports which events have their hook. Codex's trust in
//! a hook is Codex's own: nothing here reads or writes its `config.toml`.
//!
//! Cursor's `~/.cursor/hooks.json` has another shape, `{"version": 1,
//! "hooks": {"<event>": [{"command": …}]}}`, with no group around each
//! hook, and hippo adds only `sessionStart` there ([`CURSOR_HOOKS`]): Cursor
//! has no prompt hook that can add context.

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

/// Gemini CLI's hook events and their subcommands: its prompt event is
/// `BeforeAgent`.
pub const GEMINI_HOOKS: [(&str, &str); 3] = [
    ("SessionStart", "session-start"),
    ("BeforeAgent", "prompt"),
    ("SessionEnd", "session-end"),
];

/// Cursor's one hook event and its subcommand.
pub const CURSOR_HOOKS: [(&str, &str); 1] = [("sessionStart", "session-start")];

/// The Claude Code user hook (`hippo hooks install --user`): the kept rules
/// in every session, inside the workspace or not.
pub const CLAUDE_USER_HOOKS: [(&str, &str); 1] = [("SessionStart", "session-start --rules-only")];

/// The agent whose hooks file is meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    /// Claude Code's user `settings.json`: the rules-only start hook.
    ClaudeCodeUser,
    Codex,
    Cursor,
    Gemini,
}

impl Agent {
    /// The events hippo hooks for this agent, with their subcommands.
    pub fn events(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Agent::Cursor => &CURSOR_HOOKS,
            Agent::Gemini => &GEMINI_HOOKS,
            Agent::ClaudeCodeUser => &CLAUDE_USER_HOOKS,
            _ => &HOOKS,
        }
    }

    /// How hippo's command for `sub` ends: `hook <sub>`, and for Codex
    /// `hook <sub> --agent codex`.
    fn suffix(self, sub: &str) -> String {
        match self {
            Agent::ClaudeCode | Agent::ClaudeCodeUser => format!("hook {sub}"),
            Agent::Codex => format!("hook {sub} --agent codex"),
            Agent::Cursor => format!("hook {sub} --agent cursor"),
            Agent::Gemini => format!("hook {sub} --agent gemini"),
        }
    }

    /// The command hippo's hook for `sub` runs: `bin`, quoted for a shell
    /// when it needs it, then [`Agent::suffix`].
    pub fn command(self, bin: &str, sub: &str) -> String {
        format!("{} {}", shell_quote(bin), self.suffix(sub))
    }

    /// The group hippo adds for `sub`. Codex keeps the start block and the
    /// relevant notes in context rather than in a file
    /// (`additionalContextLimit: 0`; hippo holds them under the budget),
    /// and gives `SessionEnd` the longest timeout it allows, 3 seconds.
    fn entry(self, bin: &str, sub: &str) -> J {
        let command = J::Scalar(Value::from(self.command(bin, sub)));
        if self == Agent::Cursor {
            return J::Object(vec![("command".into(), command)]);
        }
        if self == Agent::Gemini {
            // Gemini CLI lists hooks by name in `/hooks`.
            let hook = vec![
                (
                    "name".into(),
                    J::Scalar(Value::from(format!("hippo-{sub}"))),
                ),
                ("type".into(), J::Scalar(Value::from("command"))),
                ("command".into(), command),
            ];
            return J::Object(vec![("hooks".into(), J::Array(vec![J::Object(hook)]))]);
        }
        let mut hook = vec![
            ("type".into(), J::Scalar(Value::from("command"))),
            ("command".into(), command),
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

/// The user's Claude Code `settings.json`: `$CLAUDE_CONFIG_DIR`, else
/// `~/.claude`.
pub fn claude_user_settings_path() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::config::user_home().map(|h| h.join(".claude")))
        .map(|d| d.join("settings.json"))
}

/// `<workspace>/.claude/settings.local.json`, the person's own Claude Code
/// settings for the workspace, where hippo's hooks go.
pub fn settings_path(workspace: &Path) -> PathBuf {
    workspace.join(".claude").join("settings.local.json")
}

/// `<workspace>/.claude/settings.json`, the settings a team commits, where
/// hippo before 0.5.0 put its hooks.
pub fn shared_settings_path(workspace: &Path) -> PathBuf {
    workspace.join(".claude").join("settings.json")
}

/// `word` as one shell word: as it is when it holds only characters a
/// shell leaves alone, else in single quotes.
pub fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-+:@%,=".contains(&b));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The path hooks and plugins should run: the `hippo` on `PATH` when it is
/// this binary (a Homebrew or `~/.local/bin` link stays valid across
/// upgrades, its target does not), this binary's own absolute path
/// otherwise.
pub fn binary_path() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let real = fs::canonicalize(&exe).unwrap_or_else(|_| exe.clone());
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("hippo");
            if candidate.is_absolute()
                && fs::canonicalize(&candidate).ok().as_deref() == Some(real.as_path())
            {
                return Ok(candidate);
            }
        }
    }
    Ok(real)
}

/// `$HIPPO_HOME/bin-path`: the binary's absolute path, for the plugins
/// (Claude Code, dsh, OpenCode), which ship in repositories and so cannot
/// carry one. They run the path in it and fall back to `PATH` only when it
/// is missing.
pub fn bin_path_file(home: &Path) -> PathBuf {
    home.join("bin-path")
}

/// Writes `bin` to `$HIPPO_HOME/bin-path` (0600), unless it already says
/// that; returns whether it wrote.
pub fn write_bin_path(home: &Path, bin: &Path) -> Result<bool> {
    let path = bin_path_file(home);
    let text = format!("{}\n", bin.display());
    if fs::read_to_string(&path).is_ok_and(|t| t == text) {
        return Ok(false);
    }
    crate::perms::create_private_dir(home)?;
    crate::util::write_atomic(&path, text.as_bytes())?;
    Ok(true)
}

/// The path in `$HIPPO_HOME/bin-path`, if the file holds one.
pub fn read_bin_path(home: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(bin_path_file(home)).ok()?;
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| PathBuf::from(line))
}

/// Codex's home: `$CODEX_HOME` when set and not empty, else `~/.codex`.
pub fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => crate::config::user_home().map(|home| home.join(".codex")),
    }
}

/// `~/.cursor/hooks.json`, Cursor's user hooks file.
pub fn cursor_hooks_path() -> Option<PathBuf> {
    crate::config::user_home().map(|home| home.join(".cursor").join("hooks.json"))
}

/// `~/.gemini/settings.json`, Gemini CLI's user settings, which hold its
/// hooks.
pub fn gemini_settings_path() -> Option<PathBuf> {
    crate::config::user_home().map(|home| home.join(".gemini").join("settings.json"))
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

    /// Sets the command of every hippo hook under this event's entries
    /// that ends with `wanted` to `command`; returns whether any changed.
    fn repoint(&mut self, wanted: &str, command: &str) -> bool {
        let J::Array(entries) = self else {
            return false;
        };
        let mut changed = false;
        let mut fix = |h: &mut J| {
            if is_hippo(h, wanted) {
                if let J::Object(fields) = h {
                    for (k, v) in fields.iter_mut() {
                        if k == "command" && *v != J::Scalar(Value::from(command)) {
                            *v = J::Scalar(Value::from(command));
                            changed = true;
                        }
                    }
                }
            }
        };
        for entry in entries.iter_mut() {
            fix(entry);
            if let Some(J::Array(hooks)) = entry.get_mut("hooks") {
                hooks.iter_mut().for_each(&mut fix);
            }
        }
        changed
    }

    /// Whether any hook command under this event's entries is hippo's,
    /// ending with `wanted` ([`Agent::suffix`]).
    fn runs(&self, wanted: &str) -> bool {
        let J::Array(entries) = self else {
            return false;
        };
        // Claude Code and Codex group hooks under `hooks`; Cursor lists
        // them directly.
        entries.iter().any(|entry| {
            is_hippo(entry, wanted)
                || matches!(entry.get("hooks"), Some(J::Array(hooks)) if hooks.iter().any(|h| is_hippo(h, wanted)))
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

/// Which of the three events have their hook for a Claude Code session in
/// `workspace`: in `.claude/settings.local.json`, or in the shared
/// `.claude/settings.json` where an older hippo put it.
pub fn status(workspace: &Path) -> Result<Vec<(&'static str, bool)>> {
    let local = status_file(&settings_path(workspace), Agent::ClaudeCode)?;
    let shared = status_file(&shared_settings_path(workspace), Agent::ClaudeCode)?;
    Ok(local
        .into_iter()
        .zip(shared)
        .map(|((event, a), (_, b))| (event, a || b))
        .collect())
}

/// The events whose hippo hook sits in `workspace`'s shared
/// `.claude/settings.json`, where hippo before 0.5.0 put it.
pub fn claude_code_shared(workspace: &Path) -> Result<Vec<&'static str>> {
    Ok(
        status_file(&shared_settings_path(workspace), Agent::ClaudeCode)?
            .into_iter()
            .filter(|(_, has)| *has)
            .map(|(event, _)| event)
            .collect(),
    )
}

/// Which of the three events have `agent`'s hippo hook in the hooks file
/// at `path`.
pub fn status_file(path: &Path, agent: Agent) -> Result<Vec<(&'static str, bool)>> {
    let tree = read(path)?;
    Ok(agent
        .events()
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

/// The command of every hippo hook for `agent` in the hooks file at
/// `path`, in file order; none when the file is missing or unreadable.
pub fn hippo_commands(path: &Path, agent: Agent) -> Vec<String> {
    let Ok(Some(tree)) = read(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for &(event, sub) in agent.events() {
        let wanted = agent.suffix(sub);
        let Some(J::Array(entries)) = tree.get("hooks").and_then(|h| h.get(event)) else {
            continue;
        };
        for entry in entries {
            let mut hooks = vec![entry];
            if let Some(J::Array(inner)) = entry.get("hooks") {
                hooks.extend(inner.iter());
            }
            for h in hooks {
                if is_hippo(h, &wanted) {
                    if let Some(J::Scalar(Value::String(c))) = h.get("command") {
                        out.push(c.clone());
                    }
                }
            }
        }
    }
    out
}

/// The program a hook command runs: its first shell word, unquoted. `None`
/// for a command this simple reading cannot split.
pub fn command_program(command: &str) -> Option<String> {
    let command = command.trim_start();
    if let Some(rest) = command.strip_prefix('\'') {
        let mut out = String::new();
        let mut rest = rest;
        loop {
            let end = rest.find('\'')?;
            out.push_str(&rest[..end]);
            rest = &rest[end + 1..];
            if let Some(more) = rest.strip_prefix("\\''") {
                out.push('\'');
                rest = more;
            } else {
                return Some(out);
            }
        }
    }
    command.split_whitespace().next().map(str::to_owned)
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

/// Whether the Claude Code settings file at `path` has hippo's user hook,
/// `hippo hook session-start --rules-only`.
pub fn file_runs_rules_only(path: &Path) -> bool {
    matches!(read(path), Ok(Some(tree)) if tree
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .is_some_and(|e| e.runs("hook session-start --rules-only")))
}

/// What [`install_file`] changed: the events whose hook it added, and those
/// whose existing hippo hook it pointed at the binary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Installed {
    pub added: Vec<&'static str>,
    pub updated: Vec<&'static str>,
}

impl Installed {
    /// Nothing was written.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty()
    }
}

/// Adds the missing hooks to `workspace`'s `.claude/settings.local.json`,
/// running `bin`, as [`install_file`] does.
pub fn install(workspace: &Path, bin: &str) -> Result<Installed> {
    install_file(&settings_path(workspace), Agent::ClaudeCode, bin)
}

/// Adds `agent`'s missing hooks to the hooks file at `path`, each running
/// `bin`, creating the file and its folder when missing, and rewrites an
/// existing hippo hook that runs anything else (a bare `hippo` found on
/// `PATH`, an old path) to run `bin`. Nothing is written when every hook is
/// there and runs `bin`.
pub fn install_file(path: &Path, agent: Agent, bin: &str) -> Result<Installed> {
    let mut tree = read(path)?.unwrap_or(J::Object(Vec::new()));
    let mut done = Installed::default();
    if agent == Agent::Cursor && tree.get("version").is_none() {
        if let J::Object(entries) = &mut tree {
            entries.insert(0, ("version".into(), J::Scalar(Value::from(1))));
        }
    }
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
    for &(event, sub) in agent.events() {
        let wanted = agent.suffix(sub);
        let command = agent.command(bin, sub);
        let existing = events.iter_mut().find(|(k, _)| k == event);
        if let Some((_, value)) = existing {
            if value.runs(&wanted) {
                if value.repoint(&wanted, &command) {
                    done.updated.push(event);
                }
                continue;
            }
        }
        let entry = agent.entry(bin, sub);
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
        done.added.push(event);
    }
    if done.is_empty() {
        return Ok(done);
    }
    let mut text = String::new();
    tree.write(&mut text, 0);
    text.push('\n');
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))?;
    }
    crate::util::write_atomic(path, text.as_bytes())?;
    Ok(done)
}

/// Whether hook object `h` is hippo's, its command ending with `wanted`.
fn is_hippo(h: &J, wanted: &str) -> bool {
    match h.get("command") {
        Some(J::Scalar(Value::String(c))) => c.contains("hippo") && c.trim_end().ends_with(wanted),
        _ => false,
    }
}

/// Removes hippo's three hooks from `workspace`'s Claude Code settings,
/// `.claude/settings.local.json` and the shared `.claude/settings.json`
/// both, as [`uninstall_file`] does. Returns the events removed from
/// either.
pub fn uninstall(workspace: &Path) -> Result<Vec<&'static str>> {
    let mut removed = uninstall_file(&settings_path(workspace), Agent::ClaudeCode)?;
    for event in uninstall_file(&shared_settings_path(workspace), Agent::ClaudeCode)? {
        if !removed.contains(&event) {
            removed.push(event);
        }
    }
    Ok(removed)
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
    for &(event, sub) in agent.events() {
        let wanted = agent.suffix(sub);
        let Some((_, J::Array(entries))) = events.iter_mut().find(|(k, _)| k == event) else {
            continue;
        };
        let before = entries.len();
        entries.retain(|e| !is_hippo(e, &wanted));
        let mut any = entries.len() != before;
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

/// An agent whose pre-tool hook can run the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardAgent {
    /// `PreToolUse` in an account's `settings.json`.
    ClaudeCode,
    /// `BeforeTool` in `~/.gemini/settings.json`.
    Gemini,
    /// `PreToolUse` in `$CODEX_HOME/hooks.json`, beside the session hooks.
    Codex,
    /// `preToolUse` in `~/.cursor/hooks.json`, a flat list of scripts.
    Cursor,
}

impl GuardAgent {
    /// Every agent `hippo guard install` can write a hook for.
    pub const ALL: [GuardAgent; 4] = [
        GuardAgent::ClaudeCode,
        GuardAgent::Gemini,
        GuardAgent::Codex,
        GuardAgent::Cursor,
    ];

    /// The value of `--agent` for `hippo guard check`.
    pub fn name(self) -> &'static str {
        match self {
            GuardAgent::ClaudeCode => "claude-code",
            GuardAgent::Gemini => "gemini",
            GuardAgent::Codex => "codex",
            GuardAgent::Cursor => "cursor",
        }
    }

    fn event(self) -> &'static str {
        match self {
            GuardAgent::ClaudeCode | GuardAgent::Codex => "PreToolUse",
            GuardAgent::Gemini => "BeforeTool",
            GuardAgent::Cursor => "preToolUse",
        }
    }

    /// The tools the hook runs for: shell, file and fetch tools, and MCP
    /// tools in Claude Code.
    fn matcher(self) -> &'static str {
        match self {
            GuardAgent::ClaudeCode => "Bash|Read|Write|Edit|MultiEdit|WebFetch|mcp__.*",
            GuardAgent::Gemini => "run_shell_command|read_file|read_many_files|write_file|replace",
            // Codex names every shell tool `Bash` in a hook's input, file
            // edits `apply_patch` and MCP tools `mcp__<server>__<tool>`.
            GuardAgent::Codex => "^(Bash|apply_patch|mcp__.*)$",
            // cursor-agent's tool names (Shell, Read, Write, Delete) and
            // `MCP:<tool>`; the matcher is a regular expression.
            GuardAgent::Cursor => "^(Shell|Read|Write|Delete|MCP:.*)$",
        }
    }

    fn suffix(self) -> String {
        format!("guard check --agent {}", self.name())
    }

    /// The command the hook runs.
    pub fn command(self, bin: &str) -> String {
        format!("{} {}", shell_quote(bin), self.suffix())
    }

    /// hippo's entry: the matcher and one command hook. The hook's own
    /// timeout is a second past the guard's, which answers by then itself
    /// (`fail`); Claude Code's `onFailure: "block"` makes a missing or
    /// crashed hippo refuse the call when `fail = "closed"`.
    fn entry(self, bin: &str, timeout_ms: u64, fail_closed: bool) -> J {
        let command = J::Scalar(Value::from(self.command(bin)));
        let mut hook = match self {
            GuardAgent::ClaudeCode | GuardAgent::Codex | GuardAgent::Cursor => vec![
                ("type".into(), J::Scalar(Value::from("command"))),
                ("command".into(), command),
                (
                    "timeout".into(),
                    J::Scalar(Value::from(timeout_ms.div_ceil(1000) + 1)),
                ),
            ],
            GuardAgent::Gemini => vec![
                ("name".into(), J::Scalar(Value::from("hippo-guard"))),
                ("type".into(), J::Scalar(Value::from("command"))),
                ("command".into(), command),
                ("timeout".into(), J::Scalar(Value::from(timeout_ms + 1000))),
            ],
        };
        if fail_closed && self == GuardAgent::ClaudeCode {
            hook.push(("onFailure".into(), J::Scalar(Value::from("block"))));
        }
        if self == GuardAgent::Cursor {
            // One flat script: Cursor has no groups. `failClosed` makes a
            // missing or crashing hippo refuse the call.
            hook.push(("matcher".into(), J::Scalar(Value::from(self.matcher()))));
            if fail_closed {
                hook.push(("failClosed".into(), J::Scalar(Value::from(true))));
            }
            return J::Object(hook);
        }
        J::Object(vec![
            ("matcher".into(), J::Scalar(Value::from(self.matcher()))),
            ("hooks".into(), J::Array(vec![J::Object(hook)])),
        ])
    }
}

/// A planned change to one settings file for the guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardPlan {
    pub path: PathBuf,
    /// `add`, `update`, `remove` or `none`.
    pub change: &'static str,
    /// The file's text after the change.
    pub text: String,
}

impl GuardPlan {
    /// Writes the plan, creating the folder when missing.
    pub fn apply(&self) -> Result<()> {
        if self.change == "none" {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))?;
        }
        crate::util::write_atomic(&self.path, self.text.as_bytes())
    }
}

/// Whether the settings file at `path` has hippo's guard entry, and its
/// command when it has.
pub fn guard_installed(path: &Path, agent: GuardAgent) -> Result<Option<String>> {
    let Some(tree) = read(path)? else {
        return Ok(None);
    };
    let wanted = agent.suffix();
    let Some(J::Array(entries)) = tree.get("hooks").and_then(|h| h.get(agent.event())) else {
        return Ok(None);
    };
    for entry in entries {
        // Cursor lists scripts directly; the others group them.
        let flat = std::slice::from_ref(entry);
        let hooks = match entry.get("hooks") {
            Some(J::Array(hooks)) => hooks.as_slice(),
            _ => flat,
        };
        for h in hooks {
            if is_hippo(h, &wanted) {
                if let Some(J::Scalar(Value::String(c))) = h.get("command") {
                    return Ok(Some(c.clone()));
                }
            }
        }
    }
    Ok(None)
}

/// Plans adding the guard's entry to the settings file at `path` (or
/// bringing an existing one up to date), every other key kept in order.
pub fn guard_plan(
    path: &Path,
    agent: GuardAgent,
    bin: &str,
    timeout_ms: u64,
    fail_closed: bool,
) -> Result<GuardPlan> {
    let mut tree = read(path)?.unwrap_or(J::Object(Vec::new()));
    if agent == GuardAgent::Cursor && tree.get("version").is_none() {
        if let J::Object(entries) = &mut tree {
            entries.insert(0, ("version".into(), J::Scalar(Value::from(1))));
        }
    }
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
    let wanted = agent.suffix();
    let entry = agent.entry(bin, timeout_ms, fail_closed);
    let event = agent.event();
    let change = match events.iter_mut().find(|(k, _)| k == event) {
        None => {
            events.push((event.to_owned(), J::Array(vec![entry])));
            "add"
        }
        Some((_, J::Array(items))) => {
            let found = items.iter().position(|e| {
                is_hippo(e, &wanted)
                    || matches!(e.get("hooks"), Some(J::Array(h)) if h.iter().any(|h| is_hippo(h, &wanted)))
            });
            match found {
                Some(i) if items[i] == entry => "none",
                Some(i) => {
                    items[i] = entry;
                    "update"
                }
                None => {
                    items.push(entry);
                    "add"
                }
            }
        }
        Some(_) => {
            return Err(invalid(
                path,
                &format!("has a hooks.{event} that is not an array; left as it is"),
            ))
        }
    };
    let mut text = String::new();
    tree.write(&mut text, 0);
    text.push('\n');
    Ok(GuardPlan {
        path: path.to_path_buf(),
        change,
        text,
    })
}

/// Plans removing the guard's entry from the settings file at `path`.
pub fn guard_remove_plan(path: &Path, agent: GuardAgent) -> Result<GuardPlan> {
    let none = |text: String| GuardPlan {
        path: path.to_path_buf(),
        change: "none",
        text,
    };
    let Some(mut tree) = read(path)? else {
        return Ok(none(String::new()));
    };
    let wanted = agent.suffix();
    let event = agent.event();
    let mut removed = false;
    if let Some(J::Object(events)) = tree.get_mut("hooks") {
        if let Some((_, J::Array(items))) = events.iter_mut().find(|(k, _)| k == event) {
            let n = items.len();
            items.retain(|e| !is_hippo(e, &wanted));
            removed |= items.len() != n;
            for entry in items.iter_mut() {
                if let Some(J::Array(hooks)) = entry.get_mut("hooks") {
                    let n = hooks.len();
                    hooks.retain(|h| !is_hippo(h, &wanted));
                    removed |= hooks.len() != n;
                }
            }
            if removed {
                items.retain(|e| !matches!(e.get("hooks"), Some(J::Array(h)) if h.is_empty()));
            }
        }
        if removed {
            events.retain(|(k, v)| k != event || !matches!(v, J::Array(items) if items.is_empty()));
        }
    }
    let mut text = String::new();
    tree.write(&mut text, 0);
    text.push('\n');
    if !removed {
        return Ok(none(text));
    }
    Ok(GuardPlan {
        path: path.to_path_buf(),
        change: "remove",
        text,
    })
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
        assert_eq!(
            install(ws.path(), "/usr/local/bin/hippo")
                .unwrap()
                .added
                .len(),
            3
        );
        assert!(status(ws.path()).unwrap().iter().all(|s| s.1));
        assert!(install(ws.path(), "/usr/local/bin/hippo")
            .unwrap()
            .is_empty());

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
        let added = install(ws.path(), "/usr/local/bin/hippo").unwrap();
        assert_eq!(added.added, ["SessionStart", "UserPromptSubmit"]);
        assert!(added.updated.is_empty());
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
        assert!(install(ws.path(), "/usr/local/bin/hippo").is_err());
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
        assert_eq!(
            install_file(&path, Agent::Codex, "hippo")
                .unwrap()
                .added
                .len(),
            3
        );
        assert!(install_file(&path, Agent::Codex, "hippo")
            .unwrap()
            .is_empty());
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
        assert!(install_file(&path, Agent::Codex, "hippo").is_err());
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

    #[test]
    fn cursor_gets_one_flat_hook_with_a_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        fs::write(
            &path,
            r#"{"hooks": {"stop": [{"command": "./audit.sh"}], "sessionStart": [{"command": "other"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            status_file(&path, Agent::Cursor).unwrap(),
            [("sessionStart", false)]
        );
        assert_eq!(
            install_file(&path, Agent::Cursor, "hippo").unwrap().added,
            ["sessionStart"]
        );
        assert!(install_file(&path, Agent::Cursor, "hippo")
            .unwrap()
            .is_empty());
        let text = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["version"], 1);
        assert!(text.starts_with("{\n  \"version\": 1,"), "{text}");
        assert_eq!(
            value["hooks"]["sessionStart"],
            serde_json::json!([{"command": "other"}, {"command": "hippo hook session-start --agent cursor"}])
        );
        assert_eq!(value["hooks"]["stop"][0]["command"], "./audit.sh");
        assert_eq!(
            status_file(&path, Agent::Cursor).unwrap(),
            [("sessionStart", true)]
        );
        // Neither Claude Code's nor Codex's hooks count it.
        assert!(status_file(&path, Agent::Codex)
            .unwrap()
            .iter()
            .all(|(_, has)| !has));
        assert_eq!(
            uninstall_file(&path, Agent::Cursor).unwrap(),
            ["sessionStart"]
        );
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            value["hooks"]["sessionStart"],
            serde_json::json!([{"command": "other"}])
        );
        assert_eq!(value["hooks"]["stop"][0]["command"], "./audit.sh");
    }

    #[test]
    fn gemini_gets_three_named_hooks_beside_its_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"theme": "Default", "mcpServers": {"x": {"command": "x"}}}"#,
        )
        .unwrap();
        assert_eq!(
            install_file(&path, Agent::Gemini, "hippo").unwrap().added,
            ["SessionStart", "BeforeAgent", "SessionEnd"]
        );
        assert!(install_file(&path, Agent::Gemini, "hippo")
            .unwrap()
            .is_empty());
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["theme"], "Default");
        assert_eq!(value["mcpServers"]["x"]["command"], "x");
        assert_eq!(
            value["hooks"]["BeforeAgent"][0]["hooks"][0],
            serde_json::json!({"name": "hippo-prompt", "type": "command", "command": "hippo hook prompt --agent gemini"})
        );
        assert!(status_file(&path, Agent::Gemini)
            .unwrap()
            .iter()
            .all(|(_, has)| *has));
        // Claude Code's status reads other event names.
        assert!(!status_file(&path, Agent::ClaudeCode).unwrap()[1].1);
        assert_eq!(uninstall_file(&path, Agent::Gemini).unwrap().len(), 3);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value
            .get("hooks")
            .is_some_and(|h| h.as_object().unwrap().is_empty()));
        assert_eq!(value["mcpServers"]["x"]["command"], "x");
    }

    #[test]
    fn hooks_run_the_absolute_path_and_a_bare_hippo_is_repointed() {
        let dir = tempfile::tempdir().unwrap();
        let path = codex_hooks_path(dir.path());
        fs::write(
            &path,
            r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "hippo hook session-start --agent codex"}]}]}}"#,
        )
        .unwrap();
        let bin = "/opt/my tools/hippo";
        let done = install_file(&path, Agent::Codex, bin).unwrap();
        assert_eq!(done.added, ["UserPromptSubmit", "SessionEnd"]);
        assert_eq!(done.updated, ["SessionStart"]);
        let commands = hippo_commands(&path, Agent::Codex);
        assert_eq!(commands.len(), 3);
        assert!(commands
            .iter()
            .all(|c| c.starts_with("'/opt/my tools/hippo' hook ")));
        assert_eq!(
            command_program(&commands[0]).as_deref(),
            Some("/opt/my tools/hippo")
        );
        assert!(install_file(&path, Agent::Codex, bin).unwrap().is_empty());
        // Another binary later: every entry follows it, none is added.
        let done = install_file(&path, Agent::Codex, "/usr/bin/hippo").unwrap();
        assert!(done.added.is_empty());
        assert_eq!(done.updated.len(), 3);
    }

    #[test]
    fn claude_code_hooks_go_to_settings_local_and_old_ones_are_found_in_settings() {
        let ws = tempfile::tempdir().unwrap();
        let shared = shared_settings_path(ws.path());
        fs::create_dir_all(shared.parent().unwrap()).unwrap();
        fs::write(
            &shared,
            r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "hippo hook session-start"}, {"type": "command", "command": "team-tool start"}]}]}}"#,
        )
        .unwrap();
        // The old entry counts as installed, and is found for moving.
        assert!(status(ws.path()).unwrap()[0].1);
        assert_eq!(claude_code_shared(ws.path()).unwrap(), ["SessionStart"]);
        install(ws.path(), "/usr/bin/hippo").unwrap();
        assert!(settings_path(ws.path()).ends_with(".claude/settings.local.json"));
        assert_eq!(
            hippo_commands(&settings_path(ws.path()), Agent::ClaudeCode).len(),
            3
        );
        // Moving takes only hippo's entry out of the shared file.
        uninstall_file(&shared, Agent::ClaudeCode).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&shared).unwrap()).unwrap();
        assert_eq!(
            value["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "team-tool start"
        );
        assert!(claude_code_shared(ws.path()).unwrap().is_empty());
    }

    #[test]
    fn shell_quote_leaves_plain_paths_and_quotes_the_rest() {
        assert_eq!(shell_quote("/usr/local/bin/hippo"), "/usr/local/bin/hippo");
        assert_eq!(shell_quote("/a b/hippo"), "'/a b/hippo'");
        assert_eq!(shell_quote("/it's/hippo"), "'/it'\\''s/hippo'");
        assert_eq!(
            command_program(&format!("{} hook prompt", shell_quote("/it's/hippo"))).as_deref(),
            Some("/it's/hippo")
        );
    }

    #[test]
    fn bin_path_is_written_private_and_only_when_it_changes() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().join("h");
        assert!(read_bin_path(&home).is_none());
        assert!(write_bin_path(&home, Path::new("/usr/bin/hippo")).unwrap());
        assert!(!write_bin_path(&home, Path::new("/usr/bin/hippo")).unwrap());
        assert_eq!(read_bin_path(&home).unwrap(), Path::new("/usr/bin/hippo"));
        #[cfg(unix)]
        assert_eq!(crate::perms::mode(&bin_path_file(&home)), Some(0o600));
    }

    #[test]
    fn the_codex_guard_goes_beside_the_session_hooks_in_hooks_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = codex_hooks_path(dir.path());
        install_file(&path, Agent::Codex, "/opt/hippo").unwrap();
        let plan = guard_plan(&path, GuardAgent::Codex, "/opt/hippo", 2000, false).unwrap();
        assert_eq!(plan.change, "add");
        plan.apply().unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let entry = &v["hooks"]["PreToolUse"][0];
        assert_eq!(entry["matcher"], "^(Bash|apply_patch|mcp__.*)$");
        assert_eq!(
            entry["hooks"][0]["command"],
            "/opt/hippo guard check --agent codex"
        );
        assert_eq!(entry["hooks"][0]["timeout"], 3);
        // Codex has no onFailure, so even a closed fail mode adds none.
        let closed = guard_plan(&path, GuardAgent::Codex, "/opt/hippo", 2000, true).unwrap();
        assert!(!closed.text.contains("onFailure"));
        // The session hooks stay; Claude Code's guard status does not see it.
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(
            guard_installed(&path, GuardAgent::Codex)
                .unwrap()
                .as_deref(),
            Some("/opt/hippo guard check --agent codex")
        );
        assert_eq!(
            guard_installed(&path, GuardAgent::ClaudeCode).unwrap(),
            None
        );
        let removed = guard_remove_plan(&path, GuardAgent::Codex).unwrap();
        assert_eq!(removed.change, "remove");
        removed.apply().unwrap();
        assert_eq!(guard_installed(&path, GuardAgent::Codex).unwrap(), None);
        assert_eq!(uninstall_file(&path, Agent::Codex).unwrap().len(), 3);
    }
}

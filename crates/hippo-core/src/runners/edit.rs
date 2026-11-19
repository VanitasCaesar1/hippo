//! Changes to `[runners]`, `[roles]` and `[fallback]` in `config.toml`:
//! `hippo runners add|remove|migrate` and `hippo config set|unset
//! role.<role>` and `fallback.<role>`.
//!
//! As with every `hippo config` change, a change is planned first (the
//! lines it removes and adds), written only when asked, and checked before
//! it is written: the new text must load as a configuration and its
//! runners must resolve ([`super::plan`]), so a change never leaves both
//! the older keys and `[runners]`, a role naming a missing runner, or a
//! runner a role still uses removed. Every other line, comments included,
//! is kept.

use std::path::{Path, PathBuf};

use super::{plan, valid_name, Role, EFFORTS};
use crate::config::{default_model_command, Config};
use crate::error::{Error, Result};

/// A change to the runner tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// `hippo runners add <name> …`: a new `[runners.<name>]`.
    Add(NewRunner),
    /// `hippo runners remove <name>`.
    Remove(String),
    /// `hippo config set role.<role> <runner>`.
    SetRole(String, String),
    /// `hippo config unset role.<role>`.
    UnsetRole(String),
    /// `hippo config set fallback.<role> 'a,b'`.
    SetFallback(String, Vec<String>),
    /// `hippo config unset fallback.<role>`.
    UnsetFallback(String),
    /// `hippo runners migrate`: `model_command` and
    /// `reviewer_command_override` into `[runners]`.
    Migrate,
}

/// What `hippo runners add` was given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewRunner {
    pub name: String,
    pub preset: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub account: Option<String>,
    pub domain: Option<String>,
    pub argv: Option<Vec<String>>,
    pub reply: Option<String>,
    pub allow_tools: bool,
}

/// A planned change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub path: PathBuf,
    /// Lines taken out, in file order.
    pub removed: Vec<String>,
    /// Lines put in, in file order.
    pub added: Vec<String>,
    /// Notes to print with the change.
    pub notes: Vec<String>,
    pub text: String,
}

impl Edit {
    pub fn changed(&self) -> bool {
        !self.removed.is_empty() || !self.added.is_empty()
    }

    /// The change as `hippo config` prints it.
    pub fn diff(&self) -> String {
        let mut out = format!("{}\n", self.path.display());
        if !self.changed() {
            out.push_str("  (no change)\n");
        }
        for line in &self.removed {
            out.push_str(&format!("- {line}\n"));
        }
        for line in &self.added {
            out.push_str(&format!("+ {line}\n"));
        }
        for note in &self.notes {
            out.push_str(&format!("{note}\n"));
        }
        out
    }

    /// Writes the change, atomically.
    pub fn apply(&self) -> Result<()> {
        if self.changed() {
            crate::util::write_atomic(&self.path, self.text.as_bytes())?;
        }
        Ok(())
    }
}

fn bad(message: impl Into<String>) -> Error {
    Error::Setting(message.into())
}

/// Plans `change` on `<home>/config.toml`.
pub fn plan_change(home: &Path, change: &Change) -> Result<Edit> {
    let path = home.join("config.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
    plan_text(home, &path, &text, change)
}

/// [`plan_change`] on a file's `text`.
pub fn plan_text(home: &Path, path: &Path, text: &str, change: &Change) -> Result<Edit> {
    let before = load(home, text)?;
    let mut notes = Vec::new();
    let mut new_text = text.to_owned();
    match change {
        Change::Migrate => {
            new_text = migrate(&before, &new_text, &mut notes)?;
        }
        Change::Add(new) => {
            if super::has_legacy_keys(&before) {
                new_text = migrate(&before, &new_text, &mut notes)?;
            }
            let name = &new.name;
            if !valid_name(name) {
                return Err(bad(format!(
                    "runner name {name:?}: letters, digits, - and _ (at most 40)"
                )));
            }
            if before.runners.contains_key(name) && !matches!(change, Change::Migrate) {
                return Err(bad(format!(
                    "there is a runner named {name} already; hippo runners remove {name} first"
                )));
            }
            if let Some(effort) = &new.effort {
                if !EFFORTS.contains(&effort.as_str()) {
                    return Err(bad(format!("effort: one of {}", EFFORTS.join(", "))));
                }
            }
            new_text = append_table(&new_text, &format!("runners.{name}"), &runner_lines(new));
            // A second runner needs a default role: the one there was.
            let migrated = load(home, &new_text)?;
            if !migrated.roles.contains_key("default") {
                let existing: Vec<&String> =
                    migrated.runners.keys().filter(|k| *k != name).collect();
                let default = match existing.as_slice() {
                    [only] => (*only).clone(),
                    _ => name.clone(),
                };
                new_text = set_in_table(&new_text, "roles", "default", &quote(&default));
            }
            if new.account.is_some() {
                notes.push(format!(
                    "Receipts in the workspace record the runner's name, {name}, never its account folder; they are shared through git, so pick a name you are happy for the team to see."
                ));
            }
            if before.runners.is_empty() && !before.roles.contains_key("default") {
                notes.push(format!(
                    "{name} is the first runner, so every role uses it; hippo config set role.<role> picks another."
                ));
            }
        }
        Change::Remove(name) => {
            if !before.runners.contains_key(name) {
                return Err(bad(format!("no runner named {name}")));
            }
            for (role, value) in &before.roles {
                if value.as_str() == Some(name) {
                    return Err(bad(format!(
                        "role {role} uses {name}; set role.{role} to another runner first"
                    )));
                }
            }
            for (role, value) in &before.fallback {
                if value
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(name)))
                {
                    return Err(bad(format!(
                        "fallback.{role} lists {name}; change it first"
                    )));
                }
            }
            new_text = remove_table(&new_text, &format!("runners.{name}"));
        }
        Change::SetRole(role, runner) => {
            check_role(role)?;
            new_text = set_in_table(&new_text, "roles", role, &quote(runner));
        }
        Change::UnsetRole(role) => {
            check_role(role)?;
            new_text = remove_in_table(&new_text, "roles", role);
        }
        Change::SetFallback(role, list) => {
            if role != "cross_domain" {
                check_role(role)?;
            }
            let value = format!(
                "[{}]",
                list.iter().map(|n| quote(n)).collect::<Vec<_>>().join(", ")
            );
            new_text = set_in_table(&new_text, "fallback", role, &value);
        }
        Change::UnsetFallback(role) => {
            new_text = remove_in_table(&new_text, "fallback", role);
        }
    }
    let after = load(home, &new_text)?;
    if !matches!(change, Change::Remove(_)) || !after.runners.is_empty() {
        plan(&after).map_err(|e| bad(format!("would not work: {e}")))?;
    }
    let (removed, added) = line_diff(text, &new_text);
    Ok(Edit {
        path: path.to_path_buf(),
        removed,
        added,
        notes,
        text: new_text,
    })
}

fn check_role(role: &str) -> Result<()> {
    if role == "default" || Role::parse(role).is_some() {
        Ok(())
    } else {
        Err(bad(format!(
            "{role} is not a role: default, propose, review, lessons or ask"
        )))
    }
}

/// `text` loaded as a configuration in `home`.
fn load(home: &Path, text: &str) -> Result<Config> {
    let mut cfg: Config =
        toml::from_str(text).map_err(|e| bad(format!("config.toml would not load: {e}")))?;
    cfg.home = home.to_path_buf();
    Ok(cfg)
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("a string serialises")
}

fn runner_lines(new: &NewRunner) -> Vec<String> {
    let mut out = vec![format!("preset = {}", quote(&new.preset))];
    let mut opt = |key: &str, value: &Option<String>| {
        if let Some(v) = value {
            out.push(format!("{key} = {}", quote(v)));
        }
    };
    opt("model", &new.model);
    opt("effort", &new.effort);
    opt("account", &new.account);
    opt("data_domain", &new.domain);
    if let Some(argv) = &new.argv {
        out.push(format!(
            "argv = [{}]",
            argv.iter().map(|a| quote(a)).collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(reply) = &new.reply {
        out.push(format!("reply = {}", quote(reply)));
    }
    if new.allow_tools {
        out.push("allow_tools = true".into());
    }
    out
}

/// `model_command` and `reviewer_command_override` taken out of `text` and
/// written as `[runners.custom]` and `[runners.custom-review]`, with
/// `[roles]`. A `model_command` equal to the default becomes nothing: the
/// default needs no runner.
fn migrate(cfg: &Config, text: &str, notes: &mut Vec<String>) -> Result<String> {
    if !super::has_legacy_keys(cfg) {
        return Ok(text.to_owned());
    }
    let mut out = text.to_owned();
    for key in ["model_command", "reviewer_command_override"] {
        out = remove_top_level(&out, key);
    }
    let mut roles = Vec::new();
    let custom = cfg.model_command != default_model_command();
    if custom {
        out = append_table(&out, "runners.custom", &command_lines(&cfg.model_command));
        roles.push(("default", "custom"));
    } else {
        out = append_table(&out, "runners.claude", &[r#"preset = "claude""#.to_owned()]);
        roles.push(("default", "claude"));
    }
    if let Some(review) = &cfg.reviewer_command_override {
        out = append_table(&out, "runners.custom-review", &command_lines(review));
        roles.push(("review", "custom-review"));
    }
    for (role, name) in roles {
        out = set_in_table(&out, "roles", role, &quote(name));
    }
    notes.push(
        "Migrated runners run in the sandbox with a limited environment: only the base list (PATH, HOME, locale, proxy and certificate variables) reaches them. If your command needs another variable, such as an API key, add env_pass = [\"NAME\"] to its table."
            .into(),
    );
    Ok(out)
}

fn command_lines(argv: &[String]) -> Vec<String> {
    vec![
        r#"preset = "command""#.to_owned(),
        format!(
            "argv = [{}]",
            argv.iter().map(|a| quote(a)).collect::<Vec<_>>().join(", ")
        ),
        r#"reply = "json-result""#.to_owned(),
        r#"data_domain = "custom""#.to_owned(),
    ]
}

/// Whether line `line` opens a table, and its name.
fn header(line: &str) -> Option<String> {
    let t = line.trim();
    let name = t.strip_prefix('[')?.strip_suffix(']')?;
    if name.starts_with('[') {
        return None;
    }
    Some(name.trim().replace(['"', '\''], ""))
}

/// The lines of table `name`: its header's index and the index after its
/// last line (blank lines at its end left out).
fn find_table(lines: &[&str], name: &str) -> Option<(usize, usize)> {
    let start = lines
        .iter()
        .position(|l| header(l).as_deref() == Some(name))?;
    let mut end = lines[start + 1..]
        .iter()
        .position(|l| header(l).is_some())
        .map_or(lines.len(), |p| start + 1 + p);
    while end > start + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    Some((start, end))
}

fn join(lines: &[String]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// `text` with a table `[name]` holding `body` at its end.
fn append_table(text: &str, name: &str, body: &[String]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!("[{name}]"));
    lines.extend(body.iter().cloned());
    join(&lines)
}

/// `text` without the table `[name]` and its lines.
fn remove_table(text: &str, name: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some((start, end)) = find_table(&lines, name) else {
        return text.to_owned();
    };
    let mut out: Vec<String> = lines[..start].iter().map(|l| (*l).to_owned()).collect();
    let rest: Vec<String> = lines[end..]
        .iter()
        .skip_while(|l| l.trim().is_empty())
        .map(|l| (*l).to_owned())
        .collect();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    if !rest.is_empty() && !out.is_empty() {
        out.push(String::new());
    }
    out.extend(rest);
    join(&out)
}

/// The index of the line in `lines[from..to]` that sets `key`.
fn key_line(lines: &[&str], from: usize, to: usize, key: &str) -> Option<usize> {
    (from..to).find(|&i| {
        let t = lines[i].trim_start();
        !t.starts_with('#')
            && t.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
    })
}

/// `text` with `key = value` in table `[table]`, replacing the key's line
/// or adding it at the table's end; the table is added when missing.
fn set_in_table(text: &str, table: &str, key: &str, value: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let line = format!("{key} = {value}");
    let Some((start, end)) = find_table(&lines, table) else {
        return append_table(text, table, &[line]);
    };
    let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
    match key_line(&lines, start + 1, end, key) {
        Some(i) => out[i] = line,
        None => out.insert(end, line),
    }
    join(&out)
}

/// `text` without `key` in table `[table]`; an emptied table goes too.
fn remove_in_table(text: &str, table: &str, key: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some((start, end)) = find_table(&lines, table) else {
        return text.to_owned();
    };
    let Some(i) = key_line(&lines, start + 1, end, key) else {
        return text.to_owned();
    };
    let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
    out.remove(i);
    let left = out[start + 1..end - 1]
        .iter()
        .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let text = join(&out);
    if left {
        text
    } else {
        remove_table(&text, table)
    }
}

/// `text` without the top-level `key`, a multi-line array included.
fn remove_top_level(text: &str, key: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let top = lines
        .iter()
        .position(|l| header(l).is_some())
        .unwrap_or(lines.len());
    let Some(i) = key_line(&lines, 0, top, key) else {
        return text.to_owned();
    };
    let value = lines[i].split_once('=').map_or("", |(_, v)| v.trim());
    let mut last = i;
    if value.starts_with('[') && !value.ends_with(']') {
        for (j, line) in lines.iter().enumerate().skip(i + 1) {
            if line
                .split('#')
                .next()
                .unwrap_or("")
                .trim_end()
                .ends_with(']')
            {
                last = j;
                break;
            }
        }
    }
    let out: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(j, _)| *j < i || *j > last)
        .map(|(_, l)| (*l).to_owned())
        .collect();
    join(&out)
}

/// The lines only `before` has and the lines only `after` has, in order:
/// enough to show a change to a few tables.
fn line_diff(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    // Longest common subsequence, small files.
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || lcs[i][j + 1] >= lcs[i + 1][j]) {
            if !b[j].trim().is_empty() {
                added.push(b[j].to_owned());
            }
            j += 1;
        } else {
            if !a[i].trim().is_empty() {
                removed.push(a[i].to_owned());
            }
            i += 1;
        }
    }
    (removed, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/nonexistent-hippo-home";

    fn edit(text: &str, change: Change) -> Result<Edit> {
        plan_text(Path::new(HOME), Path::new("/c/config.toml"), text, &change)
    }

    #[test]
    fn add_writes_a_table_and_keeps_every_other_line() {
        let text = "# my comment\ndream_at = \"03:30\"\n";
        let e = edit(
            text,
            Change::Add(NewRunner {
                name: "work".into(),
                preset: "claude".into(),
                model: Some("claude-opus-5-5".into()),
                effort: Some("high".into()),
                account: Some("~/.claude-work".into()),
                domain: Some("acme".into()),
                ..NewRunner::default()
            }),
        )
        .unwrap();
        assert!(
            e.text.starts_with(
                "# my comment\ndream_at = \"03:30\"\n\n[runners.work]\npreset = \"claude\"\n"
            ),
            "{}",
            e.text
        );
        assert!(e.removed.is_empty());
        assert_eq!(e.added.len(), 8, "{:?}", e.added);
        assert!(
            e.text.ends_with("[roles]\ndefault = \"work\"\n"),
            "{}",
            e.text
        );
        // A second runner keeps the first as the default.
        let second = edit(
            &e.text,
            Change::Add(NewRunner {
                name: "home".into(),
                preset: "claude".into(),
                account: Some("/h".into()),
                ..NewRunner::default()
            }),
        )
        .unwrap();
        let p = plan(&load(Path::new(HOME), &second.text).unwrap()).unwrap();
        assert_eq!(p.runner_name(Role::Ask), "work");
        assert!(e.diff().contains("never its account folder"));
        // The same name again is refused.
        let again = edit(
            &e.text,
            Change::Add(NewRunner {
                name: "work".into(),
                preset: "claude".into(),
                ..NewRunner::default()
            }),
        );
        assert!(again.unwrap_err().to_string().contains("already"));
    }

    #[test]
    fn add_on_older_keys_migrates_first_and_never_leaves_both() {
        let text = "model_command = [\n  \"my-model\",\n  \"{system_prompt}\",\n]\nreviewer_command_override = [\"r\", \"{system_prompt}\"]\ndream_at = \"03:30\"\n";
        let e = edit(
            text,
            Change::Add(NewRunner {
                name: "work".into(),
                preset: "claude".into(),
                ..NewRunner::default()
            }),
        )
        .unwrap();
        assert!(!e.text.contains("model_command"), "{}", e.text);
        assert!(!e.text.contains("reviewer_command_override"), "{}", e.text);
        let cfg = load(Path::new(HOME), &e.text).unwrap();
        let p = plan(&cfg).unwrap();
        assert_eq!(p.runner_name(Role::Propose), "custom");
        assert_eq!(p.runner_name(Role::Review), "custom-review");
        assert_eq!(p.runners["custom"].argv(), ["my-model", "{system_prompt}"]);
        assert!(!p.runners["custom"].legacy);
        assert!(e.diff().contains("env_pass"));
        assert!(e.text.contains("dream_at = \"03:30\""));
    }

    #[test]
    fn migrate_round_trip_keeps_the_same_argv() {
        let text = "model_command = [\"my-model\", \"--json\", \"{system_prompt}\"]\n";
        let before = plan(&load(Path::new(HOME), text).unwrap()).unwrap();
        let e = edit(text, Change::Migrate).unwrap();
        let after = plan(&load(Path::new(HOME), &e.text).unwrap()).unwrap();
        assert_eq!(
            before.runner(Role::Propose).unwrap().argv(),
            after.runner(Role::Propose).unwrap().argv()
        );
        assert!(after.configured);
        // Nothing to migrate: no change.
        assert!(!edit(&e.text, Change::Migrate).unwrap().changed());
    }

    #[test]
    fn roles_and_fallbacks_are_set_and_unset_in_their_tables() {
        let text = "[runners.a]\npreset = \"claude\"\n\n[runners.b]\npreset = \"claude\"\naccount = \"/b\"\n\n[roles]\ndefault = \"a\"\n";
        let e = edit(text, Change::SetRole("ask".into(), "b".into())).unwrap();
        assert!(
            e.text.contains("[roles]\ndefault = \"a\"\nask = \"b\"\n"),
            "{}",
            e.text
        );
        let e = edit(&e.text, Change::SetRole("ask".into(), "a".into())).unwrap();
        assert_eq!(e.removed, ["ask = \"b\""]);
        assert_eq!(e.added, ["ask = \"a\""]);
        let e = edit(
            &e.text,
            Change::SetFallback("default".into(), vec!["b".into()]),
        )
        .unwrap();
        assert!(
            e.text.ends_with("[fallback]\ndefault = [\"b\"]\n"),
            "{}",
            e.text
        );
        let e = edit(&e.text, Change::UnsetFallback("default".into())).unwrap();
        assert!(!e.text.contains("[fallback]"), "{}", e.text);
        // A role naming a missing runner, or an unknown role, is refused.
        assert!(edit(text, Change::SetRole("ask".into(), "zz".into())).is_err());
        assert!(edit(text, Change::SetRole("reviewer".into(), "a".into())).is_err());
    }

    #[test]
    fn remove_refuses_a_runner_a_role_uses() {
        let text = "[runners.a]\npreset = \"claude\"\n\n[runners.b]\npreset = \"claude\"\n\n[roles]\ndefault = \"a\"\nask = \"b\"\n";
        let err = edit(text, Change::Remove("b".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("role ask uses b"), "{err}");
        let e = edit(text, Change::UnsetRole("ask".into())).unwrap();
        let e = edit(&e.text, Change::Remove("b".into())).unwrap();
        assert!(!e.text.contains("runners.b"), "{}", e.text);
        assert!(e.text.contains("[runners.a]"));
    }
}

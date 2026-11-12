//! `hippo config`: plan and write one setting in `config.toml` or the
//! workspace's `hippo.toml`, the path every switch in
//! the app's Manage takes.
//!
//! A change rewrites the one `key = value` line in place, or inserts it as a
//! top-level key after the leading comments when the file has none; every
//! other line, comments included, is unchanged. The new text is parsed with
//! the loader's rules before it is written, so a value the loader would
//! reject is never written.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::{Config, Workspace};
use crate::error::{Error, Result};
use crate::notes::normalise;

/// Which file a key lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum File {
    /// `$HIPPO_HOME/config.toml`.
    Config,
    /// `<workspace>/.hippo/hippo.toml`.
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A whole number of at least 1.
    Count,
    /// `HH:MM`.
    Clock,
    /// `on`/`off`, `true`/`false`.
    Switch,
    /// A one-line array of strings.
    List,
    /// An argv: set whole, from a JSON array or words; a multi-line array in the file is replaced whole.
    Command,
}

/// The keys `hippo config` changes, their file and shape.
const KEYS: [(&str, File, Shape); 10] = [
    ("transcript_roots", File::Config, Shape::List),
    ("exclude_roots", File::Config, Shape::List),
    ("ingest_interval_minutes", File::Config, Shape::Count),
    ("dream_at", File::Config, Shape::Clock),
    ("model_command", File::Config, Shape::Command),
    ("required_notes", File::Workspace, Shape::List),
    ("budget_bytes", File::Workspace, Shape::Count),
    ("passes_off", File::Workspace, Shape::List),
    ("lessons", File::Workspace, Shape::Switch),
    ("snapshot_keep_days", File::Workspace, Shape::Count),
];

/// The names of the keys `hippo config` changes.
pub fn keys() -> impl Iterator<Item = &'static str> {
    KEYS.iter().map(|(k, _, _)| *k)
}

/// What to do to a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Set(String),
    Unset,
    Add(String),
    Remove(String),
}

/// A planned change: the file, the line before and after, and the new text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub file: File,
    pub path: PathBuf,
    pub key: String,
    /// The key's line now; `None` when the file has none.
    pub before: Option<String>,
    /// The key's line after; `None` when it is removed.
    pub after: Option<String>,
    /// Whether the text changes at all.
    pub changed: bool,
    #[serde(skip)]
    pub text: String,
}

impl Plan {
    /// `- before` / `+ after` lines under the file's path, as the CLI
    /// prints a plan and the app's confirmation shows it.
    pub fn diff(&self) -> String {
        let mut out = format!("{}\n", self.path.display());
        if !self.changed {
            out.push_str("  (no change)\n");
            return out;
        }
        out.push_str(&format!(
            "- {}\n",
            self.before.as_deref().unwrap_or("(none)")
        ));
        out.push_str(&format!(
            "+ {}\n",
            self.after.as_deref().unwrap_or("(none)")
        ));
        out
    }
}

fn bad(message: impl Into<String>) -> Error {
    Error::Setting(message.into())
}

fn shape(key: &str) -> Result<(File, Shape)> {
    KEYS.iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, f, s)| (*f, *s))
        .ok_or_else(|| {
            bad(format!(
                "unknown key {key:?}; one of: {}",
                keys().collect::<Vec<_>>().join(", ")
            ))
        })
}

/// The file `key` lives in, under `home` (and the workspace `home`'s
/// config names).
pub fn file_of(home: &Path, key: &str) -> Result<(File, PathBuf)> {
    let (file, _) = shape(key)?;
    let path = match file {
        File::Config => home.join("config.toml"),
        File::Workspace => {
            let cfg = Config::load_in(home)?;
            cfg.workspace_dir()?.join(".hippo").join("hippo.toml")
        }
    };
    Ok((file, path))
}

/// Plans `op` on `key` in the files under `home`.
pub fn plan(home: &Path, key: &str, op: &Op) -> Result<Plan> {
    let (file, path) = file_of(home, key)?;
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
    let ws_root = match file {
        File::Workspace => Some(Config::load_in(home)?.workspace_dir()?.to_path_buf()),
        File::Config => None,
    };
    plan_text(file, &path, &text, key, op, ws_root.as_deref())
}

/// [`plan`] on a file's `text`; `ws_root` is the workspace for a
/// `hippo.toml` key (required notes must exist there).
pub fn plan_text(
    file: File,
    path: &Path,
    text: &str,
    key: &str,
    op: &Op,
    ws_root: Option<&Path>,
) -> Result<Plan> {
    let (_, shape) = shape(key)?;
    let found = find_line(text, key);
    let before = found.as_ref().map(|(_, l)| l.clone());
    let value_text = before
        .as_deref()
        .and_then(|l| l.split_once('=').map(|(_, v)| v.trim().to_owned()));
    let after: Option<String> = match (op, shape) {
        (Op::Unset, _) => None,
        (Op::Set(v), Shape::Count) => {
            let n: u64 = v
                .trim()
                .parse()
                .map_err(|_| bad(format!("{key} must be a whole number, found {v:?}")))?;
            if n == 0 {
                return Err(bad(format!("{key} must be at least 1")));
            }
            Some(format!("{key} = {n}"))
        }
        (Op::Set(v), Shape::Clock) => Some(format!("{key} = \"{}\"", v.trim())),
        (Op::Set(v), Shape::Switch) => {
            let on = match v.trim() {
                "on" | "true" | "yes" => true,
                "off" | "false" | "no" => false,
                other => return Err(bad(format!("{key} is on or off, found {other:?}"))),
            };
            Some(format!("{key} = {on}"))
        }
        (Op::Set(v), Shape::Command) => {
            let argv = parse_command(key, v)?;
            Some(format!(
                "{key} = [{}]",
                argv.iter()
                    .map(|p| serde_json::to_string(p).expect("a string serialises"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
        (Op::Set(_), Shape::List) => {
            return Err(bad(format!(
                "{key} is a list: use hippo config add or remove"
            )))
        }
        (Op::Add(_) | Op::Remove(_), s) if s != Shape::List => {
            return Err(bad(format!("{key} is not a list: use hippo config set")))
        }
        (Op::Add(v) | Op::Remove(v), _) => {
            let mut items = match &value_text {
                Some(value) => parse_list(key, value)?,
                None if key == "transcript_roots" => crate::config::default_transcript_roots(),
                None => Vec::new(),
            };
            let v = match key {
                "required_notes" => {
                    let root = ws_root.ok_or_else(|| bad("no workspace"))?;
                    let rel = normalise(Path::new(v.trim()));
                    if matches!(op, Op::Add(_)) && !root.join(&rel).is_file() {
                        return Err(bad(format!("no note at {}", rel.display())));
                    }
                    rel.to_string_lossy().into_owned()
                }
                _ => v.trim().to_owned(),
            };
            if v.is_empty() {
                return Err(bad("an empty value"));
            }
            let same = |item: &String| match key {
                "required_notes" => normalise(Path::new(item)) == Path::new(&v),
                _ => item.trim_end_matches('/') == v.trim_end_matches('/'),
            };
            match op {
                Op::Add(_) => {
                    if !items.iter().any(same) {
                        items.push(v.clone());
                    }
                }
                _ => {
                    let n = items.len();
                    items.retain(|i| !same(i));
                    if items.len() == n {
                        return Err(bad(format!("{v} is not in {key}")));
                    }
                }
            }
            Some(format!(
                "{key} = [{}]",
                items
                    .iter()
                    .map(|p| serde_json::to_string(p).expect("a string serialises"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    };
    // A multi-line array (the template's model_command) is replaced whole.
    let last = found.as_ref().map(|(i, _)| array_end(text, *i));
    let new_text = match (&found, &after, last) {
        (Some((i, line)), Some(after), Some(last)) => {
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            replace_lines(text, *i, last, Some(&format!("{indent}{after}")))
        }
        (Some((i, _)), None, Some(last)) => replace_lines(text, *i, last, None),
        (None, Some(after), _) => insert_top_level(text, after),
        _ => text.to_owned(),
    };
    check(file, &new_text, ws_root)?;
    let changed = new_text != text;
    Ok(Plan {
        file,
        path: path.to_path_buf(),
        key: key.to_owned(),
        before,
        after,
        changed,
        text: new_text,
    })
}

/// Writes a plan, atomically.
pub fn apply(plan: &Plan) -> Result<()> {
    if plan.changed {
        crate::util::write_atomic(&plan.path, plan.text.as_bytes())?;
    }
    Ok(())
}

/// The first line that sets `key` (not a comment, not inside a table), and
/// its index. A list that does not close on its line is refused later.
fn find_line(text: &str, key: &str) -> Option<(usize, String)> {
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with('[') {
            // A table: keys below it are not top-level.
            return None;
        }
        if t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix(key) {
            if rest.trim_start().starts_with('=') {
                return Some((i, line.to_owned()));
            }
        }
    }
    None
}

fn parse_list(key: &str, value: &str) -> Result<Vec<String>> {
    if !(value.starts_with('[') && value.ends_with(']')) {
        return Err(bad(format!(
            "{key} is not a one-line array; edit the file by hand"
        )));
    }
    let parsed: toml::Value = toml::from_str(&format!("v = {value}"))
        .map_err(|e| bad(format!("{key} does not parse: {e}")))?;
    Ok(parsed
        .get("v")
        .and_then(|v| v.as_array())
        .ok_or_else(|| bad(format!("{key} is not an array")))?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect())
}

/// The last line of the value that starts on line `at`: the line itself,
/// or, for an array that does not close there, the first later line that
/// ends with `]` once comments are set aside.
fn array_end(text: &str, at: usize) -> usize {
    let lines: Vec<&str> = text.lines().collect();
    let value = lines[at].split_once('=').map_or("", |(_, v)| v.trim());
    if !value.starts_with('[') || value.ends_with(']') {
        return at;
    }
    for (i, line) in lines.iter().enumerate().skip(at + 1) {
        let code = line.split('#').next().unwrap_or("").trim_end();
        if code.ends_with(']') {
            return i;
        }
    }
    at
}

/// A command given to `hippo config set model_command`: a JSON array of
/// strings, or words separated by spaces. Never empty.
fn parse_command(key: &str, value: &str) -> Result<Vec<String>> {
    let value = value.trim();
    let argv: Vec<String> = if value.starts_with('[') {
        serde_json::from_str(value).map_err(|e| {
            bad(format!(
                "{key} as a JSON array of strings does not parse: {e}"
            ))
        })?
    } else {
        value.split_whitespace().map(str::to_owned).collect()
    };
    if argv.first().is_none_or(|p| p.is_empty()) {
        return Err(bad(format!("{key} needs at least the program")));
    }
    Ok(argv)
}

fn replace_lines(text: &str, from: usize, to: usize, with: Option<&str>) -> String {
    let mut out: Vec<&str> = Vec::new();
    for (i, l) in text.lines().enumerate() {
        if i == from {
            if let Some(w) = with {
                out.push(w);
            }
        } else if i < from || i > to {
            out.push(l);
        }
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// `text` with `line` added as a top-level key: at the end, after a blank
/// line, or before the first table when the file has one.
fn insert_top_level(text: &str, line: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    let mut out: Vec<&str> = lines[..at].to_vec();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    if !out.is_empty() {
        out.push("");
    }
    out.push(line);
    if at < lines.len() {
        out.push("");
        out.extend_from_slice(&lines[at..]);
    }
    out.join("\n") + "\n"
}

/// Parses `text` as the loader would.
fn check(file: File, text: &str, ws_root: Option<&Path>) -> Result<()> {
    match file {
        File::Config => {
            toml::from_str::<Config>(text).map_err(|e| bad(format!("would not load: {e}")))?;
        }
        File::Workspace => {
            let ws: Workspace =
                toml::from_str(text).map_err(|e| bad(format!("would not load: {e}")))?;
            let _ = (ws, ws_root);
        }
    }
    Ok(())
}

/// What `hippo config show` prints: the settings Manage reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Shown {
    pub config: PathBuf,
    pub workspace: Option<PathBuf>,
    pub transcript_roots: Vec<String>,
    pub exclude_roots: Vec<String>,
    pub ingest_interval_minutes: u64,
    pub dream_at: String,
    pub required_notes: Vec<PathBuf>,
    pub budget_bytes: Option<usize>,
    pub passes_off: Vec<String>,
    pub lessons: Option<bool>,
    pub snapshot_keep_days: Option<u32>,
}

/// The settings under `home`.
pub fn show(home: &Path) -> Result<Shown> {
    let cfg = Config::load_in(home)?;
    let ws = Workspace::load_optional(&cfg)?;
    Ok(Shown {
        config: home.join("config.toml"),
        workspace: cfg.workspace.clone(),
        transcript_roots: cfg.transcript_roots.clone(),
        exclude_roots: cfg.exclude_roots.clone(),
        ingest_interval_minutes: cfg.ingest_interval_minutes,
        dream_at: cfg.dream_at.clone(),
        required_notes: ws
            .as_ref()
            .map(|w| w.required_notes.clone())
            .unwrap_or_default(),
        budget_bytes: ws.as_ref().map(|w| w.budget_bytes),
        passes_off: ws
            .as_ref()
            .map(|w| w.passes_off.clone())
            .unwrap_or_default(),
        lessons: ws.as_ref().map(|w| w.lessons),
        snapshot_keep_days: ws.as_ref().and_then(|w| w.snapshot_keep_days),
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    const CFG: &str = "# hippo machine configuration\n\n# `transcript_roots`\ntranscript_roots = [\"~/.claude*/projects\"]\n\n# `ingest_interval_minutes` (10)\ningest_interval_minutes = 10\n";

    #[test]
    fn model_command_is_set_whole_over_a_multi_line_array() {
        let text = include_str!("../templates/config.toml")
            .replace("workspace = \"{{workspace}}\"", "# workspace");
        let plan = p(
            &text,
            "model_command",
            Op::Set(r#"["ollama", "run", "llama3"]"#.into()),
        )
        .unwrap();
        assert_eq!(
            plan.after.as_deref(),
            Some(r#"model_command = ["ollama", "run", "llama3"]"#)
        );
        assert!(
            !plan.text.contains("--no-session-persistence"),
            "{}",
            plan.text
        );
        assert!(plan.text.contains("ingest_interval_minutes = 10"));
        let cfg: Config = toml::from_str(&plan.text).unwrap();
        assert_eq!(cfg.model_command, ["ollama", "run", "llama3"]);
        // Words work too, and unset returns to the default.
        let plan = p(&text, "model_command", Op::Set("mymodel --json".into())).unwrap();
        assert_eq!(
            plan.after.as_deref(),
            Some(r#"model_command = ["mymodel", "--json"]"#)
        );
        let plan = p(&text, "model_command", Op::Unset).unwrap();
        let cfg: Config = toml::from_str(&plan.text).unwrap();
        assert_eq!(cfg.model_command, crate::config::default_model_command());
        assert!(p(&text, "model_command", Op::Set("  ".into())).is_err());
        assert!(p(&text, "model_command", Op::Add("x".into())).is_err());
    }

    fn p(text: &str, key: &str, op: Op) -> Result<Plan> {
        plan_text(
            File::Config,
            Path::new("/h/config.toml"),
            text,
            key,
            &op,
            None,
        )
    }

    #[test]
    fn set_rewrites_one_line_and_keeps_the_comments() {
        let plan = p(CFG, "ingest_interval_minutes", Op::Set("15".into())).unwrap();
        assert_eq!(plan.before.as_deref(), Some("ingest_interval_minutes = 10"));
        assert_eq!(plan.after.as_deref(), Some("ingest_interval_minutes = 15"));
        assert_eq!(plan.text, CFG.replace("= 10", "= 15"));
        assert!(plan
            .diff()
            .contains("- ingest_interval_minutes = 10\n+ ingest_interval_minutes = 15"));
    }

    #[test]
    fn a_missing_key_is_added_at_the_end() {
        let plan = p(CFG, "dream_at", Op::Set("04:00".into())).unwrap();
        assert_eq!(plan.before, None);
        assert_eq!(plan.text, format!("{CFG}\ndream_at = \"04:00\"\n"));
        let tables = "a = 1\n\n[x]\nb = 2\n";
        let plan = p(tables, "dream_at", Op::Set("04:00".into())).unwrap();
        assert_eq!(plan.text, "a = 1\n\ndream_at = \"04:00\"\n\n[x]\nb = 2\n");
    }

    #[test]
    fn lists_add_and_remove_once() {
        let plan = p(
            CFG,
            "exclude_roots",
            Op::Add("~/.claude-work2/projects".into()),
        )
        .unwrap();
        assert_eq!(
            plan.after.as_deref(),
            Some("exclude_roots = [\"~/.claude-work2/projects\"]")
        );
        let again = p(
            &plan.text,
            "exclude_roots",
            Op::Add("~/.claude-work2/projects/".into()),
        )
        .unwrap();
        assert!(!again.changed);
        let removed = p(
            &plan.text,
            "exclude_roots",
            Op::Remove("~/.claude-work2/projects".into()),
        )
        .unwrap();
        assert_eq!(removed.after.as_deref(), Some("exclude_roots = []"));
        assert!(p(CFG, "exclude_roots", Op::Remove("/x".into())).is_err());
    }

    #[test]
    fn transcript_roots_without_a_line_start_from_the_defaults() {
        let plan = p("# c\n", "transcript_roots", Op::Add("/Volumes/Old".into())).unwrap();
        let after = plan.after.unwrap();
        assert!(after.contains("~/.codex/sessions") && after.contains("/Volumes/Old"));
    }

    #[test]
    fn values_the_loader_rejects_are_refused() {
        assert!(p(CFG, "ingest_interval_minutes", Op::Set("0".into())).is_err());
        assert!(p(CFG, "dream_at", Op::Set("25:00".into())).is_err());
        assert!(p(CFG, "nope", Op::Set("1".into())).is_err());
        assert!(p(CFG, "exclude_roots", Op::Set("x".into())).is_err());
        let multi = "transcript_roots = [\n  \"a\",\n]\n";
        assert!(p(multi, "transcript_roots", Op::Add("b".into())).is_err());
    }

    #[test]
    fn unset_removes_the_line() {
        let plan = p(CFG, "ingest_interval_minutes", Op::Unset).unwrap();
        assert!(!plan.text.contains("ingest_interval_minutes ="));
        assert!(plan.text.contains("# `ingest_interval_minutes` (10)"));
    }

    #[test]
    fn workspace_keys_and_required_notes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notes")).unwrap();
        std::fs::write(dir.path().join("notes/A.md"), "# A\n").unwrap();
        let ws = "notes_dir = \"notes\"\nentry_note = \"notes/A.md\"\nrequired_notes = []\n";
        let wp = |text: &str, key: &str, op: Op| {
            plan_text(
                File::Workspace,
                Path::new("/w/.hippo/hippo.toml"),
                text,
                key,
                &op,
                Some(dir.path()),
            )
        };
        let plan = wp(ws, "required_notes", Op::Add("./notes/A.md".into())).unwrap();
        assert_eq!(
            plan.after.as_deref(),
            Some("required_notes = [\"notes/A.md\"]")
        );
        assert!(wp(ws, "required_notes", Op::Add("notes/B.md".into())).is_err());
        let off = wp(ws, "lessons", Op::Set("off".into())).unwrap();
        assert!(off.text.contains("lessons = false"));
        assert!(wp(ws, "snapshot_keep_days", Op::Set("0".into())).is_err());
        let keep = wp(ws, "snapshot_keep_days", Op::Set("30".into())).unwrap();
        assert!(keep.text.contains("snapshot_keep_days = 30"));
    }
}

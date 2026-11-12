//! Configuration: `$HIPPO_HOME/config.toml`, the
//! workspace's `.hippo/hippo.toml`, and `hippo init`.
//!
//! `~` is expanded only in `config.toml` values. Every command resolves the
//! workspace from [`Config::workspace`], never from the current directory.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use crate::error::{Error, Result};
use crate::packs;

const CONFIG_TEMPLATE: &str = include_str!("../templates/config.toml");
const WORKSPACE_TEMPLATE: &str = include_str!("../templates/hippo.toml");

/// hippo's machine directory: `$HIPPO_HOME` if set and not empty, else
/// `~/.hippo`.
///
/// # Panics
///
/// If `HIPPO_HOME` is unset and there is no home directory.
pub fn home() -> PathBuf {
    match std::env::var_os("HIPPO_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => require_user_home().join(".hippo"),
    }
}

/// `$HIPPO_HOME/config.toml`. Missing keys take the Records defaults.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Config {
    /// The hippo home this configuration was loaded from: [`home()`] unless
    /// a caller or test passed another to [`Config::load_in`]. Files under
    /// `$HIPPO_HOME` belong here, so nothing reads `HIPPO_HOME` again. Not a
    /// key in the file.
    #[serde(skip)]
    pub home: PathBuf,
    /// The one workspace every command uses; `None` until a notes folder is
    /// chosen. Without one hippo still ingests, searches and builds the
    /// rollup and the edition ("history only"); dreaming, context, hooks and
    /// the session log need it and fail with [`Error::NoWorkspace`].
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    /// Glob patterns for the directories that hold transcripts.
    #[serde(default = "default_transcript_roots")]
    pub transcript_roots: Vec<String>,
    /// Directories a `transcript_roots` pattern matches that are skipped,
    /// with everything below them: a profile turned
    /// off in the app. `~` expanded.
    #[serde(default)]
    pub exclude_roots: Vec<String>,
    /// The argv hippo runs a model with; the element `{system_prompt}` is
    /// replaced by the prompt text.
    #[serde(default = "default_model_command")]
    pub model_command: Vec<String>,
    /// Minutes between scheduled ingests; at least 1.
    #[serde(
        default = "default_ingest_interval_minutes",
        deserialize_with = "at_least_one"
    )]
    pub ingest_interval_minutes: u64,
    /// When the nightly dreaming run starts, `HH:MM` on a 24-hour clock.
    #[serde(default = "default_dream_at", deserialize_with = "clock_time")]
    pub dream_at: String,
    /// The argv the dreaming reviewer runs with instead of `model_command`,
    /// for the stub-model test and for a reviewer on another model. Not in
    /// the template.
    #[serde(default)]
    pub reviewer_command_override: Option<Vec<String>>,
}

/// The `transcript_roots` a configuration has when it sets none, and where
/// `hippo detect` looks without one.
pub fn default_transcript_roots() -> Vec<String> {
    vec![
        "~/.claude*/projects".into(),
        "~/.codex/sessions".into(),
        crate::adapters::dsh::DEFAULT_ROOT.into(),
    ]
}

/// The argv `model_command` has when the configuration does not set it.
pub fn default_model_command() -> Vec<String> {
    [
        "claude",
        "-p",
        "--no-session-persistence",
        "--setting-sources",
        "",
        "--tools",
        "",
        "--strict-mcp-config",
        "--system-prompt",
        "{system_prompt}",
        "--output-format",
        "json",
    ]
    .map(String::from)
    .to_vec()
}

fn default_ingest_interval_minutes() -> u64 {
    10
}

fn default_dream_at() -> String {
    "03:30".into()
}

impl Config {
    /// Reads `home()/config.toml`, as [`Config::load_in`] does.
    pub fn load() -> Result<Config> {
        Self::load_in(&home())
    }

    /// Reads `<home>/config.toml`, records `home`, and expands `~` in
    /// `workspace` and `transcript_roots`. A workspace, when there is one,
    /// must then be an absolute path.
    pub fn load_in(home: &Path) -> Result<Config> {
        let path = home.join("config.toml");
        // Only a missing file means "not set up": a refused read (a
        // privacy prompt) stays an IO error.
        if let Err(e) = fs::metadata(&path) {
            if e.kind() == io::ErrorKind::NotFound {
                return Err(Error::NotSetUp(path));
            }
        }
        let mut config: Config = toml::from_str(&read(&path)?)?;
        config.home = home.to_path_buf();
        if let Some(workspace) = &mut config.workspace {
            if let Some(text) = workspace.to_str() {
                *workspace = PathBuf::from(expand_tilde(text));
            }
            if !workspace.is_absolute() {
                let err = io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "workspace must be an absolute path, found {}",
                        workspace.display()
                    ),
                );
                return Err(Error::io_at(&path, err));
            }
        }
        for root in &mut config.transcript_roots {
            *root = expand_tilde(root);
        }
        for root in &mut config.exclude_roots {
            *root = expand_tilde(root);
        }
        Ok(config)
    }

    /// The workspace, or [`Error::NoWorkspace`] when the configuration
    /// names none yet.
    pub fn workspace_dir(&self) -> Result<&Path> {
        self.workspace.as_deref().ok_or(Error::NoWorkspace)
    }
}

/// `<workspace>/.hippo/hippo.toml`. Missing keys take the Records defaults;
/// paths are relative to [`Workspace::root`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Workspace {
    /// The workspace directory, [`Config::workspace`]. Not a key in the file.
    #[serde(skip)]
    pub root: PathBuf,
    /// The directory of Markdown notes.
    #[serde(deserialize_with = "inside_workspace")]
    pub notes_dir: PathBuf,
    /// The note whose opening section starts every context block.
    #[serde(deserialize_with = "inside_workspace")]
    pub entry_note: PathBuf,
    /// Notes every session must read, listed in the context block.
    #[serde(default)]
    pub required_notes: Vec<PathBuf>,
    /// The context budget in bytes.
    #[serde(default = "default_budget_bytes")]
    pub budget_bytes: usize,
    /// Where `hippo log` writes; `<notes_dir>/Session log.md` when unset.
    #[serde(default)]
    pub log_path: PathBuf,
    /// Tasks `hippo doctor` compiles context for to check the budget.
    #[serde(default)]
    pub sample_tasks: Vec<String>,
    /// Dreaming passes `hippo dream` skips when no `--pass` is named.
    #[serde(default)]
    pub passes_off: Vec<String>,
    /// Whether `hippo dream` runs the lessons scan after its passes.
    #[serde(default = "default_true")]
    pub lessons: bool,
    /// Snapshots older than this many days are deleted after a dreaming run
    /// that wrote; `None` keeps them all.
    #[serde(default, deserialize_with = "optional_at_least_one")]
    pub snapshot_keep_days: Option<u32>,
    /// Globs relative to `notes_dir` naming the log notes;
    /// `None` finds them automatically ([`crate::log_notes`]).
    #[serde(default, deserialize_with = "log_note_patterns")]
    pub log_notes: Option<Vec<String>>,
}

fn default_true() -> bool {
    true
}

fn optional_at_least_one<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<u32>, D::Error> {
    match Option::<u32>::deserialize(deserializer)? {
        Some(0) => Err(D::Error::custom("must be at least 1")),
        days => Ok(days),
    }
}

fn log_note_patterns<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Vec<String>>, D::Error> {
    let patterns = Option::<Vec<String>>::deserialize(deserializer)?;
    if let Some(patterns) = &patterns {
        crate::log_notes::check_patterns(patterns).map_err(D::Error::custom)?;
    }
    Ok(patterns)
}

fn default_budget_bytes() -> usize {
    crate::DEFAULT_CONTEXT_BUDGET
}

fn inside_workspace<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<PathBuf, D::Error> {
    let path = PathBuf::deserialize(deserializer)?;
    if stays_inside(&path) {
        Ok(path)
    } else {
        Err(D::Error::custom(format!(
            "must be relative to the workspace and without `..`, found {}",
            path.display()
        )))
    }
}

fn at_least_one<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<u64, D::Error> {
    match u64::deserialize(deserializer)? {
        0 => Err(D::Error::custom("must be at least 1")),
        minutes => Ok(minutes),
    }
}

fn clock_time<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<String, D::Error> {
    let text = String::deserialize(deserializer)?;
    if is_clock_time(&text) {
        Ok(text)
    } else {
        Err(D::Error::custom(format!(
            "must be HH:MM from 00:00 to 23:59, found {text:?}"
        )))
    }
}

/// `HH:MM` on a 24-hour clock.
fn is_clock_time(text: &str) -> bool {
    let Some((hour, minute)) = text.split_once(':') else {
        return false;
    };
    let two_digits = |part: &str| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit());
    two_digits(hour)
        && two_digits(minute)
        && hour.parse::<u8>().is_ok_and(|h| h < 24)
        && minute.parse::<u8>().is_ok_and(|m| m < 60)
}

/// Neither absolute nor holding `..`, so joined to the workspace the path
/// stays inside it.
pub(crate) fn stays_inside(path: &Path) -> bool {
    path.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

impl Workspace {
    /// `<root>/<notes_dir>`: the notes directory on disk.
    pub fn notes_root(&self) -> PathBuf {
        self.root.join(&self.notes_dir)
    }

    /// `<root>/.hippo`.
    pub fn hippo_dir(&self) -> PathBuf {
        self.root.join(".hippo")
    }

    /// Reads `<config.workspace>/.hippo/hippo.toml`; [`Error::NoWorkspace`]
    /// when the configuration names no workspace.
    pub fn load(config: &Config) -> Result<Workspace> {
        let root = config.workspace_dir()?;
        let path = root.join(".hippo").join("hippo.toml");
        Self::parse(&read(&path)?, root)
    }

    /// [`Workspace::load`], or `None` when the configuration names no
    /// workspace: what the commands that work on history alone (`ingest`,
    /// `show session`, `show day`, `doctor`) use.
    pub fn load_optional(config: &Config) -> Result<Option<Workspace>> {
        match config.workspace {
            None => Ok(None),
            Some(_) => Self::load(config).map(Some),
        }
    }

    fn parse(text: &str, root: &Path) -> Result<Workspace> {
        let mut workspace: Workspace = toml::from_str(text)?;
        workspace.root = root.to_path_buf();
        if workspace.log_path.as_os_str().is_empty() {
            workspace.log_path = workspace.notes_dir.join("Session log.md");
        }
        Ok(workspace)
    }
}

/// The options of `hippo init`.
#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    /// The workspace. When `None`, the current directory is the workspace
    /// if `notes_dir` and `entry_note` are given or it already holds
    /// `.hippo/hippo.toml` (and no `add_roots` are given); otherwise there is
    /// none, and `config.toml` is written for history only. Canonicalised.
    pub workspace: Option<PathBuf>,
    /// `notes_dir` for a new `.hippo/hippo.toml`.
    pub notes_dir: Option<String>,
    /// `entry_note` for a new `.hippo/hippo.toml`.
    pub entry_note: Option<String>,
    /// Transcript roots to add to `transcript_roots`: absolute paths or
    /// paths under `~/`. An existing `config.toml` gains them in place.
    pub add_roots: Vec<String>,
    /// Overwrite `config.toml` and the files `init` writes under `.hippo/`.
    pub force: bool,
}

/// What `init` wrote and what it left as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    /// The canonical workspace path written to `config.toml`; `None` for a
    /// history-only configuration.
    pub workspace: Option<PathBuf>,
    /// Files written, in order.
    pub written: Vec<PathBuf>,
    /// Existing paths left unchanged.
    pub kept: Vec<PathBuf>,
    /// Existing files that gained lines: the ignore rules and the merge
    /// attribute.
    pub updated: Vec<PathBuf>,
}

/// The paths under `.hippo/` that each person keeps to themselves,
/// as `.hippo/.gitignore` lists them.
pub const PER_PERSON: [&str; 4] = ["snapshots/", "sessions/", "reports/", "dream-state.json"];

/// Makes sure the workspace at `root` ignores its per-person files and
/// merges its notes with `merge=union`: adds the
/// lines that are missing to `.hippo/.gitignore`, the workspace's
/// `.gitignore` (`/<log_path>`) and `.gitattributes`
/// (`/<notes_dir>/**/*.md merge=union`), creating a file that does not
/// exist. Nothing is removed or reordered. Returns the files created and
/// the files that gained lines.
pub fn ensure_git_rules(ws: &Workspace) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let notes_dir = git_path(&ws.notes_dir);
    let log = git_path(&ws.log_path);
    let attribute = format!("/{notes_dir}/**/*.md");
    let attribute = if attribute.contains(' ') {
        format!(
            "\"{}\" merge=union",
            attribute.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!("{attribute} merge=union")
    };
    let wanted: [(PathBuf, Vec<String>); 3] = [
        (
            ws.hippo_dir().join(".gitignore"),
            PER_PERSON.iter().map(|l| (*l).to_owned()).collect(),
        ),
        (ws.root.join(".gitignore"), vec![format!("/{log}")]),
        (ws.root.join(".gitattributes"), vec![attribute]),
    ];
    let mut created = Vec::new();
    let mut updated = Vec::new();
    for (path, lines) in wanted {
        let existing = match fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(Error::io_at(&path, e)),
        };
        let have: HashSet<&str> = existing
            .as_deref()
            .unwrap_or("")
            .lines()
            .map(str::trim)
            .collect();
        let missing: Vec<&String> = lines
            .iter()
            .filter(|l| !have.contains(l.as_str()))
            .collect();
        if missing.is_empty() {
            continue;
        }
        let mut text = existing.clone().unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for line in missing {
            text.push_str(line);
            text.push('\n');
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
        }
        crate::util::write_atomic(&path, text.as_bytes())?;
        if existing.is_some() {
            updated.push(path);
        } else {
            created.push(path);
        }
    }
    Ok((created, updated))
}

/// `path` (relative to the workspace) as a git pattern matches it: `/`
/// separators, `\` before `*`, `?`, `[` and `\`, and before a leading `#`
/// or `!`.
fn git_path(path: &Path) -> String {
    let text = path
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let mut out = String::new();
    for c in text.trim_start_matches("./").chars() {
        if matches!(c, '*' | '?' | '[' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    if out.starts_with('#') || out.starts_with('!') {
        out.insert(0, '\\');
    }
    out
}

/// The line a history-only `config.toml` holds where the workspace goes;
/// `init` replaces it when a workspace is chosen.
const NO_WORKSPACE_LINE: &str =
    "# workspace = none yet: `hippo init --notes-dir <dir> --entry-note <note>` sets it";

/// Writes `home()/config.toml` and the workspace's `.hippo/`.
///
/// `config.toml` comes from the template with `workspace` set to the
/// canonical workspace path, or, with no workspace, a commented line in its
/// place: a history-only configuration, which ingest, search and the
/// edition work with. `.hippo/` (`hippo.toml`, a `.gitignore` holding
/// `snapshots/`, and the `core` pack) is written when it does not exist,
/// which needs `notes_dir` and `entry_note`; an existing `.hippo/` given
/// neither is kept, as on a new machine that cloned the workspace.
///
/// An existing `config.toml` is changed in place, without `force`, in two
/// cases only: one without a workspace gains the chosen workspace, and
/// `add_roots` are added to its `transcript_roots`. Otherwise, without
/// `force`, `init` refuses, naming the path, to overwrite `config.toml` or an
/// existing `.hippo/` it was given names for. Every check runs before the
/// first write; `.hippo/` is written before `config.toml`, and without
/// `force` each new file is created with `create_new`, so a file that
/// appears meanwhile is not replaced either. An edit in place is atomic.
pub fn init(options: InitOptions) -> Result<InitReport> {
    init_in(&home(), options)
}

/// [`init`] with the hippo home given instead of read from [`home()`].
pub fn init_in(home: &Path, options: InitOptions) -> Result<InitReport> {
    let names = match (options.notes_dir.as_deref(), options.entry_note.as_deref()) {
        (Some(notes_dir), Some(entry_note)) => Some((notes_dir, entry_note)),
        (None, None) => None,
        _ => {
            return Err(invalid(
                "--notes-dir and --entry-note must be given together".into(),
            ))
        }
    };
    if let Some((notes_dir, entry_note)) = names {
        for (flag, value) in [("--notes-dir", notes_dir), ("--entry-note", entry_note)] {
            if !stays_inside(Path::new(value)) {
                return Err(invalid(format!(
                    "{flag} must be relative to the workspace and without `..`, found {value}"
                )));
            }
        }
    }
    for root in &options.add_roots {
        if !(Path::new(root).is_absolute() || root.starts_with("~/")) {
            return Err(invalid(format!(
                "--add-root must be an absolute path or start with ~/, found {root}"
            )));
        }
    }
    let given = match options.workspace {
        Some(path) => Some(path),
        None => {
            let cwd = std::env::current_dir()?;
            let cloned =
                options.add_roots.is_empty() && cwd.join(".hippo").join("hippo.toml").is_file();
            (names.is_some() || cloned).then_some(cwd)
        }
    };
    let workspace = match given {
        Some(given) => {
            let workspace = fs::canonicalize(&given).map_err(|e| Error::io_at(&given, e))?;
            if !workspace.is_dir() {
                return Err(invalid(format!(
                    "workspace is not a directory: {}",
                    workspace.display()
                )));
            }
            if workspace.to_str().is_none() {
                return Err(invalid(format!(
                    "workspace path is not UTF-8: {}",
                    workspace.display()
                )));
            }
            Some(workspace)
        }
        None => None,
    };

    let config_path = home.join("config.toml");
    let existing = if config_path.exists() && !options.force {
        let text = read(&config_path)?;
        let config: Config = toml::from_str(&text)?;
        // A set-up workspace, `init` with no flags: only the ignore rules
        // and the merge attribute are brought up to date.
        if let Some(root) = workspace.as_ref().filter(|root| {
            names.is_none()
                && options.add_roots.is_empty()
                && root.join(".hippo").join("hippo.toml").is_file()
                && config
                    .workspace
                    .as_ref()
                    .and_then(|w| fs::canonicalize(w).ok())
                    .is_some_and(|w| &w == *root)
        }) {
            let mut report = InitReport {
                workspace: Some(root.clone()),
                written: Vec::new(),
                kept: vec![root.join(".hippo"), config_path.clone()],
                updated: Vec::new(),
            };
            git_rules(root, &mut report)?;
            return Ok(report);
        }
        let adds_workspace = workspace.is_some() && config.workspace.is_none();
        if (config.workspace.is_some() && workspace.is_some())
            || (!adds_workspace && options.add_roots.is_empty())
        {
            return Err(exists(&config_path));
        }
        Some(text)
    } else {
        None
    };
    let hippo_dir = workspace.as_ref().map(|w| w.join(".hippo"));
    let create = match (&hippo_dir, names) {
        (None, _) => None,
        (Some(dir), None) if !dir.exists() => {
            return Err(invalid(format!(
                "{} does not exist; --notes-dir and --entry-note are required to create it",
                dir.display()
            )))
        }
        (Some(dir), Some(names)) if !dir.exists() || options.force => Some(names),
        (Some(_), None) => None,
        (Some(dir), Some(_)) => return Err(exists(dir)),
    };

    let mut report = InitReport {
        workspace: workspace.clone(),
        written: Vec::new(),
        kept: Vec::new(),
        updated: Vec::new(),
    };
    if let (Some(root), Some(hippo_dir)) = (&workspace, &hippo_dir) {
        match create {
            Some((notes_dir, entry_note)) => {
                fs::create_dir_all(hippo_dir).map_err(|e| Error::io_at(hippo_dir, e))?;
                let settings = WORKSPACE_TEMPLATE
                    .replace("{{notes_dir}}", &toml_escape(notes_dir))
                    .replace("{{entry_note}}", &toml_escape(entry_note));
                let settings_path = hippo_dir.join("hippo.toml");
                write_file(&settings_path, &settings, options.force)?;
                let gitignore = hippo_dir.join(".gitignore");
                write_file(&gitignore, &(PER_PERSON.join("\n") + "\n"), options.force)?;
                report.written.extend([settings_path, gitignore]);
                report.written.extend(packs::write_core_pack(root)?);
            }
            None => report.kept.push(hippo_dir.clone()),
        }
        git_rules(root, &mut report)?;
    }
    // Last, so config.toml never names a workspace whose .hippo/ failed.
    let workspace_text = workspace.as_deref().and_then(Path::to_str);
    match existing {
        Some(text) => {
            let mut text = text;
            if let Some(path) = workspace_text {
                text = with_workspace(&text, path);
            }
            if !options.add_roots.is_empty() {
                text = with_roots(&text, &options.add_roots, &config_path)?;
            }
            crate::util::write_atomic(&config_path, text.as_bytes())?;
        }
        None => {
            fs::create_dir_all(home).map_err(|e| Error::io_at(home, e))?;
            let workspace_line = match workspace_text {
                Some(path) => format!("workspace = \"{}\"", toml_escape(path)),
                None => NO_WORKSPACE_LINE.to_owned(),
            };
            let mut config =
                CONFIG_TEMPLATE.replace("workspace = \"{{workspace}}\"", &workspace_line);
            if !options.add_roots.is_empty() {
                config = with_roots(&config, &options.add_roots, &config_path)?;
            }
            write_file(&config_path, &config, options.force)?;
        }
    }
    report.written.push(config_path);
    Ok(report)
}

/// [`ensure_git_rules`] for the workspace at `root`, into `report`.
fn git_rules(root: &Path, report: &mut InitReport) -> Result<()> {
    let path = root.join(".hippo").join("hippo.toml");
    let ws = Workspace::parse(&read(&path)?, root)?;
    let (created, updated) = ensure_git_rules(&ws)?;
    report.written.extend(created);
    report.updated.extend(updated);
    Ok(())
}

/// `text` (a `config.toml` without a workspace) with `workspace = "<path>"`
/// in place of [`NO_WORKSPACE_LINE`], or before its first key when the line
/// is not there.
fn with_workspace(text: &str, path: &str) -> String {
    let line = format!("workspace = \"{}\"", toml_escape(path));
    if text.lines().any(|l| l == NO_WORKSPACE_LINE) {
        return text
            .lines()
            .map(|l| {
                if l == NO_WORKSPACE_LINE {
                    line.as_str()
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
    }
    insert_top_level(text, &line)
}

/// `line` inserted after the leading comments and blank lines of `text`, so
/// it is a top-level key before any table.
fn insert_top_level(text: &str, line: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| {
            let t = l.trim_start();
            !(t.is_empty() || t.starts_with('#'))
        })
        .unwrap_or(lines.len());
    let mut out: Vec<&str> = lines[..at].to_vec();
    out.push(line);
    out.extend_from_slice(&lines[at..]);
    out.join("\n") + "\n"
}

/// `text` with `add` appended to its `transcript_roots` (the defaults when
/// it sets none), each root once. The key must be on one line.
fn with_roots(text: &str, add: &[String], path: &Path) -> Result<String> {
    #[derive(Deserialize)]
    struct Roots {
        transcript_roots: Option<Vec<String>>,
    }
    let current = toml::from_str::<Roots>(text)?.transcript_roots;
    let mut roots = current.clone().unwrap_or_else(default_transcript_roots);
    for root in add {
        if !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    let quoted: Vec<String> = roots
        .iter()
        .map(|r| format!("\"{}\"", toml_escape(r)))
        .collect();
    let line = format!("transcript_roots = [{}]", quoted.join(", "));
    let is_key = |l: &str| {
        l.trim_start()
            .strip_prefix("transcript_roots")
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    match text.lines().find(|l| is_key(l)) {
        None => Ok(insert_top_level(text, &line)),
        Some(found) if found.contains(']') && current.is_some() => Ok(text
            .lines()
            .map(|l| if is_key(l) { line.as_str() } else { l })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"),
        Some(_) => {
            let err = io::Error::new(
                io::ErrorKind::InvalidData,
                "transcript_roots spans several lines; add the folder to it by hand",
            );
            Err(Error::io_at(path, err))
        }
    }
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| Error::io_at(path, e))
}

/// Writes `text` to `path`. Without `force` the file is created with
/// `create_new`, so an existing file is never replaced.
fn write_file(path: &Path, text: &str, force: bool) -> Result<()> {
    let opened = if force {
        fs::File::create(path)
    } else {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    };
    let mut file = opened.map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => exists(path),
        _ => Error::io_at(path, e),
    })?;
    file.write_all(text.as_bytes())
        .map_err(|e| Error::io_at(path, e))
}

fn invalid(message: String) -> Error {
    Error::Io(io::Error::new(io::ErrorKind::InvalidInput, message))
}

fn exists(path: &Path) -> Error {
    Error::Io(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "{} already exists; use --force to overwrite",
            path.display()
        ),
    ))
}

/// Escapes text for a TOML basic string, the part between double quotes.
fn toml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

/// Expands a leading `~` or `~/` to the home directory.
pub(crate) fn expand_tilde(value: &str) -> String {
    if value == "~" {
        require_user_home().to_string_lossy().into_owned()
    } else if let Some(rest) = value.strip_prefix("~/") {
        require_user_home()
            .join(rest)
            .to_string_lossy()
            .into_owned()
    } else {
        value.to_owned()
    }
}

/// The account's home directory, if it has one.
pub fn user_home() -> Option<PathBuf> {
    home::home_dir()
}

fn require_user_home() -> PathBuf {
    user_home().expect("cannot find the home directory; set HOME")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(workspace: &Path) -> InitOptions {
        InitOptions {
            workspace: Some(workspace.to_path_buf()),
            notes_dir: Some("brain".into()),
            entry_note: Some("brain/START HERE.md".into()),
            add_roots: Vec::new(),
            force: false,
        }
    }

    /// A home config, with `workspace` set.
    fn records_config(workspace: &Path) -> Config {
        Config {
            home: PathBuf::new(),
            workspace: Some(workspace.to_path_buf()),
            transcript_roots: vec![
                "~/.claude*/projects".into(),
                "~/.codex/sessions".into(),
                "~/.dsh/sessions".into(),
            ],
            exclude_roots: Vec::new(),
            model_command: [
                "claude",
                "-p",
                "--no-session-persistence",
                "--setting-sources",
                "",
                "--tools",
                "",
                "--strict-mcp-config",
                "--system-prompt",
                "{system_prompt}",
                "--output-format",
                "json",
            ]
            .map(String::from)
            .to_vec(),
            ingest_interval_minutes: 10,
            dream_at: "03:30".into(),
            reviewer_command_override: None,
        }
    }

    fn is_io(err: &Error, kind: io::ErrorKind) -> bool {
        matches!(err, Error::Io(e) if e.kind() == kind)
    }

    #[test]
    fn a_missing_config_says_how_to_start() {
        let home = tempfile::tempdir().unwrap();
        let err = Config::load_in(home.path()).unwrap_err();
        assert!(matches!(err, Error::NotSetUp(_)), "{err}");
        assert!(err.to_string().contains("run hippo"), "{err}");
    }

    #[test]
    fn reviewer_command_override_is_read_but_absent_from_the_template() {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("config.toml"),
            "workspace = \"/w\"\nreviewer_command_override = [\"stub\", \"{system_prompt}\"]\n",
        )
        .unwrap();
        let config = Config::load_in(home.path()).unwrap();
        assert_eq!(
            config.reviewer_command_override,
            Some(vec!["stub".to_owned(), "{system_prompt}".to_owned()])
        );
        assert!(!CONFIG_TEMPLATE.contains("reviewer_command_override"));
        fs::write(home.path().join("config.toml"), "workspace = \"/w\"\n").unwrap();
        assert_eq!(
            Config::load_in(home.path())
                .unwrap()
                .reviewer_command_override,
            None
        );
    }

    #[test]
    fn init_writes_files_that_parse_back_to_the_defaults() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(workspace.path()).unwrap();
        let report = init_in(home.path(), options(workspace.path())).unwrap();
        assert_eq!(report.workspace.as_deref(), Some(root.as_path()));
        assert!(report.kept.is_empty());
        let config_path = home.path().join("config.toml");
        for file in [
            root.join(".hippo/hippo.toml"),
            root.join(".hippo/.gitignore"),
        ] {
            assert!(report.written.contains(&file), "{report:?}");
        }
        // The core pack, whose passes load and check.
        for (relative, _) in crate::packs::CORE_PACK {
            let file = root.join(".hippo/packs/core").join(relative);
            assert!(report.written.contains(&file), "{report:?}");
            assert!(file.is_file(), "{}", file.display());
        }
        assert_eq!(crate::packs::load_passes(&root).unwrap().len(), 2);
        assert_eq!(report.written.last(), Some(&config_path));

        // config.toml holds the Records defaults, and so do serde's defaults.
        let written: Config = toml::from_str(&read(&config_path).unwrap()).unwrap();
        assert_eq!(written, records_config(&root));
        let minimal = format!("workspace = \"{}\"", toml_escape(root.to_str().unwrap()));
        let defaults: Config = toml::from_str(&minimal).unwrap();
        assert_eq!(defaults, records_config(&root));

        // hippo.toml likewise.
        let hippo = root.join(".hippo");
        let records_workspace = Workspace {
            root: root.clone(),
            notes_dir: "brain".into(),
            entry_note: "brain/START HERE.md".into(),
            required_notes: vec![],
            budget_bytes: 25600,
            log_path: "brain/Session log.md".into(),
            sample_tasks: vec![],
            passes_off: Vec::new(),
            lessons: true,
            snapshot_keep_days: None,
            log_notes: None,
        };
        let text = read(&hippo.join("hippo.toml")).unwrap();
        assert_eq!(Workspace::parse(&text, &root).unwrap(), records_workspace);
        let minimal = "notes_dir = \"brain\"\nentry_note = \"brain/START HERE.md\"\n";
        assert_eq!(Workspace::parse(minimal, &root).unwrap(), records_workspace);
        assert_eq!(
            read(&hippo.join(".gitignore")).unwrap(),
            "snapshots/\nsessions/\nreports/\ndream-state.json\n"
        );

        // A second init without force refuses and names the path.
        let err = init_in(home.path(), options(workspace.path())).unwrap_err();
        assert!(is_io(&err, io::ErrorKind::AlreadyExists), "{err}");
        assert!(err.to_string().contains(&config_path.display().to_string()));
    }

    #[test]
    fn every_template_key_follows_a_comment_naming_it() {
        let mut keys = 0;
        for template in [CONFIG_TEMPLATE, WORKSPACE_TEMPLATE] {
            let lines: Vec<&str> = template.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let Some((key, _)) = line.split_once(" = ") else {
                    continue;
                };
                if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                    continue;
                }
                keys += 1;
                let above = if i > 0 { lines[i - 1] } else { "" };
                let named = above.starts_with("# ") && above.contains(&format!("`{key}`"));
                assert!(named, "{line}");
            }
        }
        // The home config has 5 keys and the workspace config 6.
        assert_eq!(keys, 11);
    }

    #[test]
    fn init_keeps_a_cloned_hippo_dir() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let hippo = fs::canonicalize(workspace.path()).unwrap().join(".hippo");
        fs::create_dir(&hippo).unwrap();
        let settings = "notes_dir = \"notes\"\nentry_note = \"notes/index.md\"\n";
        fs::write(hippo.join("hippo.toml"), settings).unwrap();

        let bare = InitOptions {
            workspace: Some(workspace.path().to_path_buf()),
            ..InitOptions::default()
        };
        let report = init_in(home.path(), bare).unwrap();
        // A clone gains the ignore rules and the merge attribute it lacks.
        let root = hippo.parent().unwrap();
        assert_eq!(
            report.written,
            [
                hippo.join(".gitignore"),
                root.join(".gitignore"),
                root.join(".gitattributes"),
                home.path().join("config.toml")
            ]
        );
        assert_eq!(report.kept, [hippo.as_path()]);
        assert_eq!(read(&hippo.join("hippo.toml")).unwrap(), settings);
        assert_eq!(
            read(&root.join(".gitattributes")).unwrap(),
            "/notes/**/*.md merge=union\n"
        );
        assert_eq!(
            read(&root.join(".gitignore")).unwrap(),
            "/notes/Session log.md\n"
        );
    }

    #[test]
    fn init_adds_only_missing_git_lines_and_refreshes_a_set_up_workspace() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(workspace.path()).unwrap();
        fs::write(root.join(".gitignore"), "target/\n/notes/Session log.md").unwrap();
        fs::write(root.join(".gitattributes"), "*.png binary\n").unwrap();
        let options = InitOptions {
            workspace: Some(root.clone()),
            notes_dir: Some("my notes/#1".into()),
            entry_note: Some("my notes/#1/index.md".into()),
            ..InitOptions::default()
        };
        let report = init_in(home.path(), options).unwrap();
        assert_eq!(
            report.updated,
            [root.join(".gitignore"), root.join(".gitattributes")]
        );
        assert_eq!(
            read(&root.join(".hippo/.gitignore")).unwrap(),
            "snapshots/\nsessions/\nreports/\ndream-state.json\n"
        );
        // Kept as it was, a newline added, the log line added escaped.
        assert_eq!(
            read(&root.join(".gitignore")).unwrap(),
            "target/\n/notes/Session log.md\n/my notes/#1/Session log.md\n"
        );
        assert_eq!(
            read(&root.join(".gitattributes")).unwrap(),
            "*.png binary\n\"/my notes/#1/**/*.md\" merge=union\n"
        );
        // `init` again in the set-up workspace: nothing refused, nothing new.
        fs::write(root.join(".hippo/.gitignore"), "snapshots/\n").unwrap();
        let again = InitOptions {
            workspace: Some(root.clone()),
            ..InitOptions::default()
        };
        let report = init_in(home.path(), again.clone()).unwrap();
        assert_eq!(report.updated, [root.join(".hippo/.gitignore")]);
        assert!(report.written.is_empty(), "{report:?}");
        let report = init_in(home.path(), again).unwrap();
        assert!(report.updated.is_empty() && report.written.is_empty());
    }

    #[test]
    fn init_needs_both_names_for_a_new_hippo_dir() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let bare = InitOptions {
            workspace: Some(workspace.path().to_path_buf()),
            ..InitOptions::default()
        };
        let half = InitOptions {
            notes_dir: Some("brain".into()),
            ..bare.clone()
        };
        for options in [bare, half] {
            let err = init_in(home.path(), options).unwrap_err();
            assert!(is_io(&err, io::ErrorKind::InvalidInput), "{err}");
        }
        assert!(!home.path().join("config.toml").exists());
    }

    #[test]
    fn force_overwrites_only_what_init_writes() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        init_in(home.path(), options(workspace.path())).unwrap();
        let hippo = fs::canonicalize(workspace.path()).unwrap().join(".hippo");
        fs::create_dir(hippo.join("sessions")).unwrap();
        fs::write(hippo.join("sessions").join("s.json"), "{}").unwrap();

        // Without config.toml, an existing .hippo/ given names still needs force.
        fs::remove_file(home.path().join("config.toml")).unwrap();
        let err = init_in(home.path(), options(workspace.path())).unwrap_err();
        assert!(is_io(&err, io::ErrorKind::AlreadyExists), "{err}");
        assert!(err.to_string().contains(&hippo.display().to_string()));
        assert!(!home.path().join("config.toml").exists());

        let forced = InitOptions {
            notes_dir: Some("notes".into()),
            force: true,
            ..options(workspace.path())
        };
        let verify = hippo.join("packs/core/passes/verify.toml");
        fs::write(&verify, "edited").unwrap();
        let report = init_in(home.path(), forced).unwrap();
        for file in [
            hippo.join("hippo.toml"),
            hippo.join(".gitignore"),
            verify.clone(),
            home.path().join("config.toml"),
        ] {
            assert!(report.written.contains(&file), "{report:?}");
        }
        assert!(read(&verify)
            .unwrap()
            .starts_with("protocol = \"HIPPO_V1\""));
        let text = read(&hippo.join("hippo.toml")).unwrap();
        let settings = Workspace::parse(&text, &hippo).unwrap();
        assert_eq!(settings.notes_dir, Path::new("notes"));
        assert_eq!(read(&hippo.join("sessions").join("s.json")).unwrap(), "{}");
    }

    #[test]
    fn init_needs_a_workspace_directory() {
        let home = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let file = parent.path().join("not-a-directory");
        fs::write(&file, "").unwrap();
        let err = init_in(home.path(), options(&file)).unwrap_err();
        assert!(is_io(&err, io::ErrorKind::InvalidInput), "{err}");
        assert!(!home.path().join("config.toml").exists());
    }

    #[test]
    fn write_file_replaces_only_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "kept\n").unwrap();
        let err = write_file(&path, "new\n", false).unwrap_err();
        assert!(is_io(&err, io::ErrorKind::AlreadyExists), "{err}");
        assert_eq!(read(&path).unwrap(), "kept\n");
        write_file(&path, "new\n", true).unwrap();
        assert_eq!(read(&path).unwrap(), "new\n");
    }

    #[test]
    fn init_refuses_names_outside_the_workspace() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        for (notes_dir, entry_note) in [("../brain", "brain/e.md"), ("brain", "/etc/e.md")] {
            let outside = InitOptions {
                notes_dir: Some(notes_dir.into()),
                entry_note: Some(entry_note.into()),
                ..options(workspace.path())
            };
            let err = init_in(home.path(), outside).unwrap_err();
            assert!(is_io(&err, io::ErrorKind::InvalidInput), "{err}");
        }
        assert!(!home.path().join("config.toml").exists());
        assert!(!workspace.path().join(".hippo").exists());
    }

    #[test]
    fn load_rejects_values_the_records_rule_out() {
        let home = tempfile::tempdir().unwrap();
        let load = |text: &str| {
            fs::write(home.path().join("config.toml"), text).unwrap();
            Config::load_in(home.path())
        };
        for bad in [
            "ingest_interval_minutes = 0",
            "dream_at = \"3:30\"",
            "dream_at = \"24:00\"",
            "dream_at = \"03:60\"",
            "dream_at = \"0330\"",
        ] {
            let result = load(&format!("workspace = \"/w\"\n{bad}\n"));
            assert!(matches!(result, Err(Error::Toml(_))), "{bad}: {result:?}");
        }
        let relative = load("workspace = \"relative/w\"\n");
        assert!(
            matches!(&relative, Err(e) if is_io(e, io::ErrorKind::InvalidData)),
            "{relative:?}"
        );
        let edge = "workspace = \"/w\"\ningest_interval_minutes = 1\ndream_at = \"23:59\"\n";
        assert!(load(edge).is_ok());
    }

    #[test]
    fn workspace_paths_stay_inside_the_workspace() {
        let root = Path::new("/w");
        let parse = |notes_dir: &str, entry_note: &str| {
            let text = format!("notes_dir = \"{notes_dir}\"\nentry_note = \"{entry_note}\"\n");
            Workspace::parse(&text, root)
        };
        for (notes_dir, entry_note) in [
            ("../n", "n/e.md"),
            ("/n", "n/e.md"),
            ("n", "../e.md"),
            ("n", "/n/e.md"),
        ] {
            let result = parse(notes_dir, entry_note);
            assert!(
                matches!(result, Err(Error::Toml(_))),
                "{notes_dir} {entry_note}"
            );
        }
        assert!(parse(".", "index.md").is_ok());
        assert!(parse("./brain", "brain/START HERE.md").is_ok());
    }

    #[test]
    fn init_escapes_the_workspace_path() {
        let home = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("quote\" and back\\slash");
        fs::create_dir(&workspace).unwrap();
        init_in(home.path(), options(&workspace)).unwrap();
        let text = read(&home.path().join("config.toml")).unwrap();
        let written: Config = toml::from_str(&text).unwrap();
        assert_eq!(
            written.workspace,
            Some(fs::canonicalize(&workspace).unwrap())
        );
    }

    #[test]
    fn load_reads_hippo_home_and_expands_tilde() {
        // The only test that sets HIPPO_HOME; it restores the old value.
        let dir = tempfile::tempdir().unwrap();
        let text =
            "workspace = \"~/notes\"\ntranscript_roots = [\"~/.codex/sessions\", \"/var/t\"]\n";
        fs::write(dir.path().join("config.toml"), text).unwrap();
        let previous = std::env::var_os("HIPPO_HOME");
        std::env::set_var("HIPPO_HOME", dir.path());
        let found = home();
        let loaded = Config::load();
        match previous {
            Some(value) => std::env::set_var("HIPPO_HOME", value),
            None => std::env::remove_var("HIPPO_HOME"),
        }

        assert_eq!(found, dir.path());
        let config = loaded.unwrap();
        assert_eq!(config.home, dir.path());
        let user = require_user_home();
        assert_eq!(config.workspace, Some(user.join("notes")));
        let codex = user.join(".codex/sessions").to_string_lossy().into_owned();
        assert_eq!(config.transcript_roots, [codex, "/var/t".to_owned()]);
        assert_eq!(config.model_command, default_model_command());
    }

    #[test]
    fn workspace_load_reads_the_config_workspace() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let hippo = workspace.path().join(".hippo");
        fs::create_dir(&hippo).unwrap();
        let text = "notes_dir = \"n\"\nentry_note = \"n/e.md\"\nrequired_notes = [\"n/a.md\"]\n\
                    log_path = \"n/Log.md\"\nsample_tasks = [\"fix the webhook retry\"]\n";
        fs::write(hippo.join("hippo.toml"), text).unwrap();
        let workspace_text = toml_escape(workspace.path().to_str().unwrap());
        let config_text = format!("workspace = \"{workspace_text}\"\n");
        fs::write(home.path().join("config.toml"), config_text).unwrap();

        // A home passed in, not read from HIPPO_HOME.
        let config = Config::load_in(home.path()).unwrap();
        assert_eq!(config.home, home.path());
        let loaded = Workspace::load(&config).unwrap();
        assert_eq!(loaded.root, workspace.path());
        assert_eq!(loaded.notes_root(), workspace.path().join("n"));
        assert_eq!(loaded.hippo_dir(), hippo);
        assert_eq!(loaded.required_notes, [PathBuf::from("n/a.md")]);
        assert_eq!(loaded.log_path, Path::new("n/Log.md"));
        assert_eq!(loaded.sample_tasks, ["fix the webhook retry"]);
        assert_eq!(loaded.budget_bytes, crate::DEFAULT_CONTEXT_BUDGET);

        fs::write(hippo.join("hippo.toml"), "notes_dir = \"n\"\n").unwrap();
        assert!(matches!(Workspace::load(&config), Err(Error::Toml(_))));
    }

    #[test]
    fn init_without_a_workspace_writes_a_history_only_config() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let history = InitOptions {
            add_roots: vec!["/Volumes/Old/claude".into()],
            ..InitOptions::default()
        };
        let report = init_in(home.path(), history).unwrap();
        assert_eq!(report.workspace, None);
        let config_path = home.path().join("config.toml");
        assert_eq!(report.written, std::slice::from_ref(&config_path));
        let text = read(&config_path).unwrap();
        assert!(text.contains(NO_WORKSPACE_LINE), "{text}");
        let config = Config::load_in(home.path()).unwrap();
        assert_eq!(config.workspace, None);
        assert!(matches!(Workspace::load(&config), Err(Error::NoWorkspace)));
        assert_eq!(Workspace::load_optional(&config).unwrap(), None);
        assert_eq!(config.transcript_roots.len(), 4);
        assert_eq!(config.transcript_roots[3], "/Volumes/Old/claude");

        // Another root is added in place, once, keeping the file's comments.
        for _ in 0..2 {
            let more = InitOptions {
                add_roots: vec!["~/old-mac/codex".into()],
                ..InitOptions::default()
            };
            init_in(home.path(), more).unwrap();
        }
        let config = Config::load_in(home.path()).unwrap();
        assert_eq!(config.transcript_roots.len(), 5);
        assert!(read(&config_path).unwrap().contains("# `dream_at`"));

        // Choosing a notes folder later fills in the workspace, without force.
        let report = init_in(home.path(), options(workspace.path())).unwrap();
        let root = fs::canonicalize(workspace.path()).unwrap();
        assert_eq!(report.workspace.as_deref(), Some(root.as_path()));
        let config = Config::load_in(home.path()).unwrap();
        assert_eq!(config.workspace.as_deref(), Some(root.as_path()));
        assert_eq!(config.transcript_roots.len(), 5);
        assert_eq!(Workspace::load(&config).unwrap().root, root);
        let text = read(&config_path).unwrap();
        assert!(!text.contains(NO_WORKSPACE_LINE), "{text}");

        // Now that it names one, a second workspace still needs force.
        let err = init_in(home.path(), options(workspace.path())).unwrap_err();
        assert!(is_io(&err, io::ErrorKind::AlreadyExists), "{err}");
    }

    #[test]
    fn init_refuses_a_relative_root_and_a_bare_rerun() {
        let home = tempfile::tempdir().unwrap();
        let relative = InitOptions {
            add_roots: vec!["old/claude".into()],
            ..InitOptions::default()
        };
        let err = init_in(home.path(), relative).unwrap_err();
        assert!(is_io(&err, io::ErrorKind::InvalidInput), "{err}");
        assert!(!home.path().join("config.toml").exists());
        let bare = InitOptions {
            add_roots: vec!["/t".into()],
            ..InitOptions::default()
        };
        init_in(home.path(), bare).unwrap();
        let again = InitOptions {
            add_roots: vec![],
            ..InitOptions::default()
        };
        // Run from a directory without .hippo/: nothing to add.
        let err = init_in(home.path(), again);
        assert!(err.is_err());
    }

    #[test]
    fn a_workspace_line_goes_before_any_table() {
        let text = "# hi\n\n[extra]\nkey = 1\n";
        let out = with_workspace(text, "/w");
        assert_eq!(out, "# hi\n\nworkspace = \"/w\"\n[extra]\nkey = 1\n");
        let parsed: toml::Table = toml::from_str(&out).unwrap();
        assert_eq!(parsed["workspace"].as_str(), Some("/w"));
    }
}

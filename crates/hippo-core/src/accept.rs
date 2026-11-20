//! Tightening takes effect at once; loosening needs a local accept on each
//! machine (`docs/designs/common.md`, rule 3).
//!
//! The shared `hippo.toml` has many writers: every teammate with push
//! access, and every agent that can edit files in the workspace. So each
//! machine keeps `$HIPPO_HOME/accepted/<workspace-id>.toml` (0600): the last
//! accepted value of every security setting, and the hash of the last
//! accepted guard rule set. When a workspace loads, each setting in the
//! file is compared with the accepted one:
//!
//! - stricter or equal: the file's value applies at once and becomes the
//!   accepted value;
//! - looser: the accepted value stays in force, and the setting is listed
//!   as waiting, for `doctor`, `hippo guard status` and the app, until the
//!   person runs `hippo accept` (or presses Accept in the app).
//!
//! With no accepted file, every setting is compared with its default, so a
//! cloned workspace whose file loosens a setting waits too.
//!
//! `hippo accept` needs a terminal on stdin and stdout and refuses when an
//! agent's marker is in its environment ([`agent_marker`]). This stops the
//! casual and the injected-by-text cases; an agent with a shell runs as the
//! person and can get around it. It is not a security boundary.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{parse_version, Config, Workspace};
use crate::error::{Error, Result};

/// The first release that understands `[guard]`, `guard_note` and
/// `[team]`: what writing one of them raises `min_hippo_version` to.
pub const FIRST_VERSION: &str = "0.6.0";

/// The guard's timeout when `[guard] timeout_ms` is not set.
pub const DEFAULT_TIMEOUT_MS: u64 = 2000;

/// `[guard] fail`: what an agent's call does when the guard cannot answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fail {
    /// The call proceeds; the failure is logged.
    #[default]
    Open,
    /// The call is refused.
    Closed,
}

/// `[team] inbox`: whether lessons are shared through `.hippo/inbox/`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Inbox {
    /// Each person's Inbox stays on their machine.
    #[default]
    Local,
    /// Lesson identities and decisions are shared in the repository.
    Shared,
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Fail::Open => "open",
            Fail::Closed => "closed",
        })
    }
}

impl fmt::Display for Inbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Inbox::Local => "local",
            Inbox::Shared => "shared",
        })
    }
}

/// The security settings of one workspace that exist in this release: the
/// rows of rule 3's table, guard rule set aside (the guard compares that
/// itself and stores its hash here, [`Accepted::guard_rules`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Values {
    /// `guard_note`, relative to the workspace; `None` is the default note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_note: Option<String>,
    /// `[guard] fail`.
    #[serde(default)]
    pub guard_fail: Fail,
    /// `[guard] timeout_ms`.
    #[serde(default = "default_timeout")]
    pub guard_timeout_ms: u64,
    /// `[team] inbox`.
    #[serde(default)]
    pub team_inbox: Inbox,
    /// `min_hippo_version`; `None` when not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_hippo_version: Option<String>,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

impl Default for Values {
    fn default() -> Values {
        Values {
            guard_note: None,
            guard_fail: Fail::Open,
            guard_timeout_ms: DEFAULT_TIMEOUT_MS,
            team_inbox: Inbox::Local,
            min_hippo_version: None,
        }
    }
}

impl Values {
    /// The values `ws`'s file sets (defaults for what it leaves out).
    pub fn of(ws: &Workspace) -> Values {
        Values {
            guard_note: ws
                .guard_note
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            guard_fail: ws.guard.fail.unwrap_or_default(),
            guard_timeout_ms: ws.guard.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
            team_inbox: ws.team.inbox.unwrap_or_default(),
            min_hippo_version: ws.min_hippo_version.clone(),
        }
    }

    /// Writes these values into `ws` as the ones in force.
    fn apply_to(&self, ws: &mut Workspace) {
        ws.guard_note = self.guard_note.as_ref().map(PathBuf::from);
        ws.guard.fail = Some(self.guard_fail);
        ws.guard.timeout_ms = Some(self.guard_timeout_ms);
        ws.team.inbox = Some(self.team_inbox);
        ws.min_hippo_version = self.min_hippo_version.clone();
    }
}

/// One row of rule 3's table that exists in this release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    GuardNote,
    GuardFail,
    GuardTimeout,
    TeamInbox,
    MinHippoVersion,
}

impl Setting {
    /// Every row, in table order.
    pub const ALL: [Setting; 5] = [
        Setting::GuardNote,
        Setting::GuardFail,
        Setting::GuardTimeout,
        Setting::TeamInbox,
        Setting::MinHippoVersion,
    ];

    /// The setting as `hippo.toml` writes it.
    pub fn name(self) -> &'static str {
        match self {
            Setting::GuardNote => "guard_note",
            Setting::GuardFail => "[guard] fail",
            Setting::GuardTimeout => "[guard] timeout_ms",
            Setting::TeamInbox => "[team] inbox",
            Setting::MinHippoVersion => "min_hippo_version",
        }
    }

    /// The setting's value in `v`, as shown to the person.
    pub fn show(self, v: &Values) -> String {
        match self {
            Setting::GuardNote => v
                .guard_note
                .clone()
                .unwrap_or_else(|| "(default)".to_owned()),
            Setting::GuardFail => v.guard_fail.to_string(),
            Setting::GuardTimeout => v.guard_timeout_ms.to_string(),
            Setting::TeamInbox => v.team_inbox.to_string(),
            Setting::MinHippoVersion => v
                .min_hippo_version
                .clone()
                .unwrap_or_else(|| "(none)".to_owned()),
        }
    }

    /// Which way going from `from` to `to` moves this setting.
    pub fn direction(self, from: &Values, to: &Values) -> Direction {
        match self {
            // Any change waits.
            Setting::GuardNote => same_or_looser(from.guard_note == to.guard_note),
            Setting::GuardTimeout => same_or_looser(from.guard_timeout_ms == to.guard_timeout_ms),
            Setting::GuardFail => match (from.guard_fail, to.guard_fail) {
                (a, b) if a == b => Direction::Same,
                (Fail::Open, Fail::Closed) => Direction::Tighter,
                _ => Direction::Looser,
            },
            Setting::TeamInbox => match (from.team_inbox, to.team_inbox) {
                (a, b) if a == b => Direction::Same,
                (Inbox::Shared, Inbox::Local) => Direction::Tighter,
                _ => Direction::Looser,
            },
            Setting::MinHippoVersion => {
                // An unreadable floor counts as the highest: it refuses
                // writes, so it is never looser than a readable one.
                let rank = |v: &Option<String>| match v {
                    None => (0, (0, 0, 0)),
                    Some(text) => match parse_version(text) {
                        Some(version) => (1, version),
                        None => (2, (0, 0, 0)),
                    },
                };
                let (a, b) = (rank(&from.min_hippo_version), rank(&to.min_hippo_version));
                if from.min_hippo_version == to.min_hippo_version || a == b {
                    Direction::Same
                } else if b > a {
                    Direction::Tighter
                } else {
                    Direction::Looser
                }
            }
        }
    }

    fn take(self, from: &Values, into: &mut Values) {
        match self {
            Setting::GuardNote => into.guard_note.clone_from(&from.guard_note),
            Setting::GuardFail => into.guard_fail = from.guard_fail,
            Setting::GuardTimeout => into.guard_timeout_ms = from.guard_timeout_ms,
            Setting::TeamInbox => into.team_inbox = from.team_inbox,
            Setting::MinHippoVersion => into.min_hippo_version.clone_from(&from.min_hippo_version),
        }
    }
}

fn same_or_looser(same: bool) -> Direction {
    if same {
        Direction::Same
    } else {
        Direction::Looser
    }
}

/// Which way a change moves a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Same,
    Tighter,
    Looser,
}

/// A loosening that waits for `hippo accept`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Waiting {
    /// The setting, as the file writes it: `[guard] fail`, `guard rules`.
    pub setting: String,
    /// The value in force (the accepted one).
    pub in_force: String,
    /// The value the file asks for.
    pub in_file: String,
}

impl fmt::Display for Waiting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} in force, {} in the file",
            self.setting, self.in_force, self.in_file
        )
    }
}

/// What one machine accepted for one workspace:
/// `$HIPPO_HOME/accepted/<workspace-id>.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted {
    /// The hash of the guard rule set in force (`$HIPPO_HOME/guard/compiled.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_rules: Option<String>,
    /// The accepted settings.
    #[serde(default)]
    pub settings: Values,
}

const HEADER: &str = "# The security settings this machine accepted for one workspace (docs/designs/common.md, rule 3).\n# Written by hippo. A loosening in hippo.toml waits until `hippo accept`; do not edit this file.\n\n";

/// `$HIPPO_HOME/accepted/`.
pub fn dir(home: &Path) -> PathBuf {
    home.join("accepted")
}

/// The file that holds what this machine accepted for `key`.
pub fn path(home: &Path, key: &str) -> PathBuf {
    dir(home).join(format!("{key}.toml"))
}

/// The key the accepted file of `ws` is named by: its `workspace_id`, or,
/// before it has one, a key from the workspace's path. The key for a
/// machine with no workspace is `machine`.
pub fn key(ws: Option<&Workspace>) -> String {
    match ws {
        None => "machine".to_owned(),
        Some(ws) => match ws.workspace_id.as_deref().filter(|id| valid_id(id)) {
            Some(id) => id.to_owned(),
            None => path_key(&ws.root),
        },
    }
}

fn path_key(root: &Path) -> String {
    let digest = crate::util::sha256(root.to_string_lossy().as_bytes());
    format!("path-{}", &crate::util::hex(&digest)[..16])
}

/// A ULID's 26 characters of Crockford base 32.
pub fn valid_id(id: &str) -> bool {
    id.len() == 26 && id.parse::<ulid::Ulid>().is_ok()
}

/// What this machine accepted for `key`; `Ok(None)` when it has no record.
/// A record that does not parse is an error, which callers treat as no
/// record and `doctor` reports.
pub fn load(home: &Path, key: &str) -> Result<Option<Accepted>> {
    let file = path(home, key);
    match std::fs::read_to_string(&file) {
        Ok(text) => toml::from_str(&text)
            .map(Some)
            .map_err(|e| Error::Refused(format!("{} does not parse: {e}", file.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io_at(&file, e)),
    }
}

/// Writes what this machine accepted for `key`, 0600 in a 0700 folder.
pub fn store(home: &Path, key: &str, accepted: &Accepted) -> Result<()> {
    crate::perms::create_private_dir(&dir(home))?;
    let body = toml::to_string(accepted)
        .map_err(|e| Error::Refused(format!("accepted settings do not serialise: {e}")))?;
    crate::util::write_atomic(&path(home, key), format!("{HEADER}{body}").as_bytes())
}

/// What this machine has accepted for `ws`: the record under its key, or
/// the one written under its path before the workspace had an id (moved to
/// the id the first time it is read), or defaults.
fn load_for(home: &Path, ws: &Workspace) -> Accepted {
    let key = key(Some(ws));
    if let Ok(Some(found)) = load(home, &key) {
        return found;
    }
    let old = path_key(&ws.root);
    if old != key {
        if let Ok(Some(found)) = load(home, &old) {
            if std::fs::rename(path(home, &old), path(home, &key)).is_err() {
                let _ = store(home, &key, &found);
            }
            return found;
        }
    }
    Accepted::default()
}

/// The values in force for `file`, given what is accepted: each setting
/// that is stricter or equal comes from the file, each looser one stays as
/// accepted and is listed as waiting. Returns the values in force (which
/// are also the new accepted values) and what waits.
pub fn resolve(accepted: &Values, file: &Values) -> (Values, Vec<Waiting>) {
    let mut in_force = accepted.clone();
    let mut waiting = Vec::new();
    for setting in Setting::ALL {
        match setting.direction(accepted, file) {
            Direction::Same | Direction::Tighter => setting.take(file, &mut in_force),
            Direction::Looser => waiting.push(Waiting {
                setting: setting.name().to_owned(),
                in_force: setting.show(accepted),
                in_file: setting.show(file),
            }),
        }
    }
    (in_force, waiting)
}

/// Applies rule 3 to a workspace just read from its file: its security
/// settings become the ones in force, [`Workspace::waiting`] lists the
/// loosenings that wait, and a tightening is recorded as accepted. A
/// failure to record it changes nothing in force.
pub fn apply(home: &Path, ws: &mut Workspace) {
    let mut accepted = load_for(home, ws);
    let file = Values::of(ws);
    let (in_force, waiting) = resolve(&accepted.settings, &file);
    if in_force != accepted.settings {
        accepted.settings = in_force.clone();
        let _ = store(home, &key(Some(ws)), &accepted);
    }
    in_force.apply_to(ws);
    ws.file_values = Some(file);
    ws.waiting = waiting;
}

/// Accepts every setting of `ws` as its file sets it: what `hippo accept`
/// and the app's Accept write. `ws` is the workspace as loaded, so its
/// file values are those [`apply`] saw.
pub fn accept_settings(home: &Path, ws: &Workspace) -> Result<()> {
    let key = key(Some(ws));
    let mut accepted = load(home, &key).ok().flatten().unwrap_or_default();
    accepted.settings = ws.file_values.clone().unwrap_or_else(|| Values::of(ws));
    store(home, &key, &accepted)
}

/// Everything that waits for `hippo accept` on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Pending {
    /// Workspace settings whose file value is looser than the one in force.
    pub settings: Vec<Waiting>,
    /// The guard rule set's loosenings, one line each.
    pub guard: Vec<String>,
}

impl Pending {
    /// Whether nothing waits.
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty() && self.guard.is_empty()
    }
}

/// What waits for `hippo accept` for the configuration and its workspace.
pub fn pending(cfg: &Config, ws: Option<&Workspace>) -> Result<Pending> {
    // A record that does not parse is reported, not read as defaults.
    load(&cfg.home, &key(ws))?;
    Ok(Pending {
        settings: ws.map(|w| w.waiting.clone()).unwrap_or_default(),
        guard: crate::guard::waiting(cfg, ws)?,
    })
}

/// Accepts everything that waits: what `hippo accept` and the app's Accept
/// call after the person said yes.
pub fn accept_all(cfg: &Config, ws: Option<&Workspace>) -> Result<()> {
    if let Some(ws) = ws {
        accept_settings(&cfg.home, ws)?;
    }
    // The guard compiles with the settings just accepted in force.
    let reloaded = match ws {
        Some(_) => Workspace::load_optional(cfg)?,
        None => None,
    };
    crate::guard::accept(cfg, reloaded.as_ref())?;
    Ok(())
}

/// The environment variables coding agents set in the commands they run,
/// with the agent each belongs to and where the name was read. A variable
/// that is set and not empty means an agent is running this command.
///
/// - `CLAUDECODE`: Claude Code 2.1 sets it in its tools' environment (its
///   binary's strings); `CLAUDE_CODE_ENTRYPOINT` too.
/// - `CODEX_SANDBOX`, `CODEX_THREAD_ID`: Codex 0.170's binary.
/// - `OPENCODE`, `OPENCODE_SESSION_ID`, `AGENT`, `AI_AGENT`: OpenCode's
///   shell tool sets all four (its 2.x binary).
/// - `GEMINI_CLI`, `CURSOR_AGENT`, `COPILOT_CLI`, `JUNIE_DATA`: the list
///   of agent markers Claude Code 2.1 itself checks.
///
/// dsh, Muse and Command Code set no marker hippo has found; a command
/// they run is not recognised here, which `docs/site/src/security.md`
/// states.
pub const AGENT_MARKERS: [(&str, &str); 12] = [
    ("CLAUDECODE", "Claude Code"),
    ("CLAUDE_CODE_ENTRYPOINT", "Claude Code"),
    ("CODEX_SANDBOX", "Codex"),
    ("CODEX_THREAD_ID", "Codex"),
    ("OPENCODE", "OpenCode"),
    ("OPENCODE_SESSION_ID", "OpenCode"),
    ("AI_AGENT", "an agent (AI_AGENT)"),
    ("AGENT", "an agent (AGENT)"),
    ("GEMINI_CLI", "Gemini CLI"),
    ("CURSOR_AGENT", "Cursor"),
    ("COPILOT_CLI", "GitHub Copilot CLI"),
    ("JUNIE_DATA", "Junie"),
];

/// The first agent marker set in the environment `get` reads, as
/// `(variable, agent)`.
pub fn agent_marker(get: impl Fn(&str) -> Option<String>) -> Option<(&'static str, &'static str)> {
    AGENT_MARKERS
        .iter()
        .find(|(name, _)| get(name).is_some_and(|v| !v.is_empty()))
        .copied()
}

/// Why a command that only a person may run (`hippo accept`, `hippo
/// lessons share`) refuses here: an agent's marker in the environment, or
/// stdin or stdout not a terminal. `None` when it may go on.
pub fn person_refusal(command: &str) -> Option<String> {
    use std::io::IsTerminal as _;
    if let Some((var, agent)) = agent_marker(|name| std::env::var(name).ok()) {
        return Some(format!(
            "{command} is for the person, not an agent: {var} is set, so {agent} is running this. Run it yourself in a terminal."
        ));
    }
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return Some(format!(
            "{command} asks a question and needs a terminal on stdin and stdout; run it yourself in a terminal (or use Hippo.app)."
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v() -> Values {
        Values::default()
    }

    /// One test per row of rule 3's table that exists in this release.
    #[test]
    fn guard_note_waits_on_any_change() {
        let set = Values {
            guard_note: Some("notes/Rules.md".into()),
            ..v()
        };
        assert_eq!(Setting::GuardNote.direction(&v(), &set), Direction::Looser);
        assert_eq!(Setting::GuardNote.direction(&set, &v()), Direction::Looser);
        assert_eq!(Setting::GuardNote.direction(&set, &set), Direction::Same);
        let (in_force, waiting) = resolve(&v(), &set);
        assert_eq!(in_force.guard_note, None);
        assert_eq!(waiting[0].setting, "guard_note");
    }

    #[test]
    fn fail_closed_applies_at_once_and_open_waits() {
        let closed = Values {
            guard_fail: Fail::Closed,
            ..v()
        };
        let (in_force, waiting) = resolve(&v(), &closed);
        assert_eq!(in_force.guard_fail, Fail::Closed);
        assert!(waiting.is_empty());
        let (in_force, waiting) = resolve(&closed, &v());
        assert_eq!(in_force.guard_fail, Fail::Closed);
        assert_eq!(waiting[0].setting, "[guard] fail");
        assert_eq!(waiting[0].in_force, "closed");
        assert_eq!(waiting[0].in_file, "open");
    }

    #[test]
    fn timeout_waits_on_any_change() {
        for ms in [500, 60_000] {
            let file = Values {
                guard_timeout_ms: ms,
                ..v()
            };
            let (in_force, waiting) = resolve(&v(), &file);
            assert_eq!(in_force.guard_timeout_ms, DEFAULT_TIMEOUT_MS);
            assert_eq!(waiting.len(), 1);
        }
    }

    #[test]
    fn shared_inbox_waits_and_local_applies_at_once() {
        let shared = Values {
            team_inbox: Inbox::Shared,
            ..v()
        };
        let (in_force, waiting) = resolve(&v(), &shared);
        assert_eq!(in_force.team_inbox, Inbox::Local);
        assert_eq!(waiting[0].setting, "[team] inbox");
        let (in_force, waiting) = resolve(&shared, &v());
        assert_eq!(in_force.team_inbox, Inbox::Local);
        assert!(waiting.is_empty());
    }

    #[test]
    fn a_raised_floor_applies_and_a_lowered_one_waits() {
        let at = |s: &str| Values {
            min_hippo_version: Some(s.into()),
            ..v()
        };
        let (in_force, waiting) = resolve(&at("0.5.0"), &at("0.6.0"));
        assert_eq!(in_force.min_hippo_version.as_deref(), Some("0.6.0"));
        assert!(waiting.is_empty());
        let (in_force, waiting) = resolve(&at("0.6.0"), &at("0.5.0"));
        assert_eq!(in_force.min_hippo_version.as_deref(), Some("0.6.0"));
        assert_eq!(waiting[0].setting, "min_hippo_version");
        // Removing the floor lowers it; an unreadable floor is the highest.
        assert_eq!(resolve(&at("0.6.0"), &v()).1.len(), 1);
        assert!(resolve(&v(), &at("soon")).1.is_empty());
        assert_eq!(resolve(&at("soon"), &at("9.0.0")).1.len(), 1);
        // `v0.6.0` and `0.6.0` are the same floor.
        assert!(resolve(&at("0.6.0"), &at("v0.6.0")).1.is_empty());
    }

    #[test]
    fn a_mixed_change_takes_the_tighter_parts_and_waits_on_the_rest() {
        let file = Values {
            guard_fail: Fail::Closed,
            team_inbox: Inbox::Shared,
            min_hippo_version: Some("0.6.0".into()),
            ..v()
        };
        let (in_force, waiting) = resolve(&v(), &file);
        assert_eq!(in_force.guard_fail, Fail::Closed);
        assert_eq!(in_force.min_hippo_version.as_deref(), Some("0.6.0"));
        assert_eq!(in_force.team_inbox, Inbox::Local);
        let names: Vec<&str> = waiting.iter().map(|w| w.setting.as_str()).collect();
        assert_eq!(names, ["[team] inbox"]);
    }

    #[test]
    fn the_accepted_file_round_trips_and_is_private() {
        let home = tempfile::tempdir().unwrap();
        let accepted = Accepted {
            guard_rules: Some("ab".repeat(32)),
            settings: Values {
                team_inbox: Inbox::Shared,
                ..v()
            },
        };
        store(home.path(), "01JC7Q0000000000000000000A", &accepted).unwrap();
        let back = load(home.path(), "01JC7Q0000000000000000000A").unwrap();
        assert_eq!(back, Some(accepted));
        #[cfg(unix)]
        {
            let file = path(home.path(), "01JC7Q0000000000000000000A");
            assert_eq!(crate::perms::mode(&file), Some(0o600));
            assert_eq!(crate::perms::mode(&dir(home.path())), Some(0o700));
        }
        assert_eq!(load(home.path(), "missing").unwrap(), None);
    }

    fn workspace(home: &Path, root: &Path, extra: &str) -> Workspace {
        std::fs::create_dir_all(root.join(".hippo")).unwrap();
        std::fs::write(
            root.join(".hippo/hippo.toml"),
            format!("notes_dir = \"notes\"\nentry_note = \"notes/A.md\"\n{extra}"),
        )
        .unwrap();
        let mut cfg = Config::defaults_in(home);
        cfg.workspace = Some(root.to_path_buf());
        Workspace::load(&cfg).unwrap()
    }

    /// A workspace loads with the stricter values in force until its
    /// person accepts, on this machine only.
    #[test]
    fn a_loaded_workspace_keeps_the_stricter_values_until_accepted() {
        let home = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let shared = "[team]\ninbox = \"shared\"\n\n[guard]\nfail = \"closed\"\n";
        let ws = workspace(home.path(), root, shared);
        assert_eq!(ws.team.inbox, Some(Inbox::Local));
        assert_eq!(ws.guard.fail, Some(Fail::Closed));
        assert_eq!(ws.waiting.len(), 1);
        assert_eq!(ws.waiting[0].setting, "[team] inbox");
        accept_settings(home.path(), &ws).unwrap();
        let ws = workspace(home.path(), root, shared);
        assert_eq!(ws.team.inbox, Some(Inbox::Shared));
        assert!(ws.waiting.is_empty());
        // Another machine has accepted nothing.
        let elsewhere = workspace(other.path(), root, shared);
        assert_eq!(elsewhere.team.inbox, Some(Inbox::Local));
        // The file now fails open: closed stays in force.
        let ws = workspace(home.path(), root, "[team]\ninbox = \"shared\"\n");
        assert_eq!(ws.guard.fail, Some(Fail::Closed));
        assert_eq!(ws.waiting[0].setting, "[guard] fail");
        // A raised floor holds when the file lowers it, and refuses writes.
        workspace(home.path(), root, "min_hippo_version = \"99.0.0\"\n");
        let ws = workspace(home.path(), root, "min_hippo_version = \"0.1.0\"\n");
        assert_eq!(ws.min_hippo_version.as_deref(), Some("99.0.0"));
        assert!(ws.write_refusal().is_some());
        assert!(ws.waiting.iter().any(|w| w.setting == "min_hippo_version"));
    }

    /// What was accepted before the workspace had an id moves to the id.
    #[test]
    fn a_new_workspace_id_keeps_what_was_accepted() {
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let shared = "[team]\ninbox = \"shared\"\n";
        let ws = workspace(home.path(), root.path(), shared);
        assert!(key(Some(&ws)).starts_with("path-"));
        accept_settings(home.path(), &ws).unwrap();
        let id = "01JC7Q5W2N8X3Y4Z5A6B7C8D9E";
        let ws = workspace(
            home.path(),
            root.path(),
            &format!("workspace_id = \"{id}\"\n{shared}"),
        );
        assert_eq!(key(Some(&ws)), id);
        assert_eq!(ws.team.inbox, Some(Inbox::Shared));
        assert!(path(home.path(), id).is_file());
    }

    /// One case per agent: each marker its agent sets makes a person-only
    /// command refuse.
    #[test]
    fn every_agent_marker_is_recognised() {
        for (var, agent) in AGENT_MARKERS {
            let found = agent_marker(|name| (name == var).then(|| "1".to_owned()));
            assert_eq!(found, Some((var, agent)), "{var}");
        }
        for agent in ["Claude Code", "Codex", "OpenCode", "Gemini CLI", "Cursor"] {
            assert!(
                AGENT_MARKERS.iter().any(|(_, a)| *a == agent),
                "no marker for {agent}"
            );
        }
        assert_eq!(agent_marker(|_| None), None);
        assert_eq!(agent_marker(|_| Some(String::new())), None);
    }
}

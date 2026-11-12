//! Manage: what each card shows and the `hippo` command each control runs.
//! Read-only: every
//! change is a command the app shows, then runs through the CLI.

use std::path::{Path, PathBuf};

use hippo_core::config::{Config, Workspace};
use hippo_core::detect::{self, Detected};
use hippo_core::settings::{self, Op, Shown};
use time::{Duration, OffsetDateTime};

/// One transcript folder hippo found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub dir: PathBuf,
    /// `~` kept: what `exclude_roots` stores.
    pub display: String,
    pub profile: Option<String>,
    pub tool: &'static str,
    pub files: u64,
    /// A file in it changed in the last 10 minutes.
    pub live: bool,
    /// hippo reads it: a pattern matches it and nothing excludes it.
    pub on: bool,
    /// A `transcript_roots` pattern matches it.
    pub matched: bool,
}

impl Agent {
    /// The name the row shows: `default`, `work`, `codex`, else
    /// the folder. Never `claude-`.
    pub fn name(&self) -> String {
        match (&self.profile, self.tool) {
            (Some(p), _) => p.clone(),
            (None, "codex") => "codex".into(),
            _ => self.display.clone(),
        }
    }

    /// A Claude profile folder with no session and no usage meter:
    /// probably a mistyped `CLAUDE_CONFIG_DIR`. It gets no headroom card
    /// and is listed apart in Manage.
    pub fn never_used(&self) -> bool {
        if self.files > 0 || self.profile.is_none() || self.tool == "codex" {
            return false;
        }
        let config_dir = if self.dir.file_name().is_some_and(|n| n == "projects") {
            self.dir.parent().unwrap_or(&self.dir)
        } else {
            &self.dir
        };
        hippo_core::usage::meter::read(&hippo_core::usage::meter::settings_file(config_dir))
            .is_none()
    }
}

/// One workspace and whether its `.claude/settings.json` has hippo's hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookDir {
    pub dir: PathBuf,
    /// The folder's name, or `workspace` for hippo's own.
    pub label: String,
    /// Session start, every prompt, session end.
    pub on: [bool; 3],
    /// hippo's configured workspace.
    pub workspace: bool,
}

impl HookDir {
    pub fn all(&self) -> bool {
        self.on.iter().all(|b| *b)
    }
}

/// Everything Manage shows.
#[derive(Debug, Clone, Default)]
pub struct Manage {
    pub shown: Option<Shown>,
    pub agents: Vec<Agent>,
    /// Agent tools found with no adapter: `Cursor`, `OpenCode`.
    pub unsupported: Vec<String>,
    pub hooks: Vec<HookDir>,
    /// Each pass the workspace's packs define, and whether it runs.
    pub passes: Vec<(String, bool)>,
    pub snapshot_bytes: u64,
    /// The block compiled now with no task: bytes, required, relevant.
    pub block: Option<(usize, usize)>,
    /// Required notes: (path, ignored, sessions) from the x-ray's week.
    pub required: Vec<(PathBuf, Option<(u32, u32)>)>,
    /// Both launchd jobs' property lists exist.
    pub schedule_on: bool,
    /// Each job's property list exists: ingest, dream (one switch
    /// per job, `hippo schedule install|uninstall --job`).
    pub jobs: [bool; 2],
    /// The packs the workspace has (`hippo packs list`, read in process).
    pub packs: Vec<hippo_core::packs::install::ListedPack>,
    pub error: Option<String>,
}

/// What keeps hippo from working, in the states Manage's cards show:
/// a launchd job not installed, or the session-start hook missing in
/// hippo's workspace. Reads two property lists' existence and one settings
/// file; writes nothing. Each is a short reason: `ingest is off`.
pub fn setup_gaps(ws: Option<&Workspace>, user_home: Option<&Path>) -> Vec<String> {
    let mut out = Vec::new();
    let job = |label: &str| {
        user_home.is_some_and(|h| {
            h.join(format!("Library/LaunchAgents/{label}.plist"))
                .exists()
        })
    };
    if !job("dev.hippo.ingest") {
        out.push("ingest is off".to_owned());
    }
    if !job("dev.hippo.dream") {
        out.push("dreaming is off".to_owned());
    }
    if let Some(ws) = ws {
        let st = hippo_core::hooks::status(&ws.root).unwrap_or_default();
        if !st.first().is_some_and(|(_, has)| *has) {
            out.push("hook not installed".to_owned());
        }
    }
    out
}

/// What [`load`] reads.
pub struct Inputs<'a> {
    pub home: &'a Path,
    pub user_home: Option<&'a Path>,
    pub now: OffsetDateTime,
}

/// Reads Manage's cards. Writes nothing.
pub fn load(inp: &Inputs<'_>) -> Manage {
    let mut m = Manage::default();
    let cfg = match Config::load_in(inp.home) {
        Ok(c) => c,
        Err(e) => {
            m.error = Some(e.to_string());
            return m;
        }
    };
    m.shown = settings::show(inp.home).ok();
    let mut found = detect::detect(&cfg.transcript_roots, inp.home, inp.user_home);
    found.mark_excluded(&cfg.exclude_roots);
    // Profiles the patterns do not match yet: `~/.claude-*` folders.
    let extra = detect::detect(
        &hippo_core::config::default_transcript_roots(),
        inp.home,
        inp.user_home,
    );
    m.agents = agents(&found, &extra, inp.now);
    m.unsupported = found
        .unsupported
        .iter()
        .filter(|u| u.present)
        .map(|u| u.name.to_owned())
        .collect();
    m.unsupported.dedup();
    let job = |label: &str| {
        inp.user_home.is_some_and(|h| {
            h.join(format!("Library/LaunchAgents/{label}.plist"))
                .exists()
        })
    };
    m.jobs = [job("dev.hippo.ingest"), job("dev.hippo.dream")];
    m.schedule_on = m.jobs.iter().all(|b| *b);
    let ws = Workspace::load_optional(&cfg).ok().flatten();
    if let Some(ws) = &ws {
        m.packs = hippo_core::packs::install::list(ws).unwrap_or_default();
    }
    let rollup = hippo_core::rollup::Rollup::open(inp.home).ok().flatten();
    m.hooks = hook_dirs(ws.as_ref(), rollup.as_ref(), inp.now);
    if let Some(ws) = &ws {
        let off = &ws.passes_off;
        m.passes = hippo_core::packs::load_passes(&ws.root)
            .unwrap_or_default()
            .into_iter()
            .map(|(p, _)| {
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let on = !off.contains(&name);
                (name, on)
            })
            .collect();
        m.snapshot_bytes = walkdir::WalkDir::new(ws.hippo_dir().join("snapshots"))
            .into_iter()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum();
        if let Ok(c) = hippo_core::context::compile(
            ws,
            None,
            None,
            None,
            None,
            usize::MAX / 4,
            hippo_core::context::PartSelection::Required,
        ) {
            m.block = Some((c.bytes, c.required.len()));
        }
        let stats = rollup
            .as_ref()
            .and_then(|r| hippo_core::xray::fleet(Some(ws), r, 7, inp.now).ok());
        m.required = ws
            .required_notes
            .iter()
            .map(|p| {
                let n = hippo_core::notes::normalise(p);
                let use_ = stats.as_ref().and_then(|s| {
                    s.notes
                        .iter()
                        .find(|u| u.note == n)
                        .map(|u| (u.ignored, u.sessions))
                });
                (n, use_)
            })
            .collect();
    }
    m
}

fn agents(found: &Detected, extra: &Detected, now: OffsetDateTime) -> Vec<Agent> {
    let mut out: Vec<Agent> = Vec::new();
    let mut add = |d: &detect::Dir, matched: bool| {
        if out.iter().any(|a| a.dir == d.path) {
            return;
        }
        out.push(Agent {
            dir: d.path.clone(),
            display: d.display.clone(),
            profile: d.profile.clone(),
            tool: d.tool,
            files: d.files,
            live: d.newest.is_some_and(|t| now - t < Duration::minutes(10)),
            on: matched && !d.excluded,
            matched,
        });
    };
    for root in &found.roots {
        for d in &root.dirs {
            add(d, true);
        }
    }
    for root in &extra.roots {
        for d in &root.dirs {
            if d.files > 0 {
                add(d, false);
            }
        }
    }
    out
}

/// hippo's workspace, then the working directories of the last 7 days'
/// top-level sessions (from the rollup's transcript folders), newest first,
/// at most 6, each with its hooks.
fn hook_dirs(
    ws: Option<&Workspace>,
    rollup: Option<&hippo_core::rollup::Rollup>,
    now: OffsetDateTime,
) -> Vec<HookDir> {
    let mut dirs: Vec<(PathBuf, bool)> = Vec::new();
    if let Some(ws) = ws {
        dirs.push((ws.root.clone(), true));
    }
    if let Some(r) = rollup {
        let mut recent = r
            .sessions_in(now - Duration::days(7), now + Duration::minutes(1))
            .unwrap_or_default();
        recent.retain(|s| s.facts.parent.is_none() && !s.facts.automated);
        recent.sort_by_key(|s| std::cmp::Reverse(s.facts.last_turn));
        for s in recent {
            let f = &s.facts;
            let Some(cwd) =
                hippo_core::session_meta::origin(&f.id, &f.tool, &f.transcript_path).cwd
            else {
                continue;
            };
            let home = hippo_core::config::user_home();
            if home.as_deref() == Some(cwd.as_path()) || !cwd.is_dir() {
                continue;
            }
            if dirs.iter().any(|(d, _)| *d == cwd) {
                continue;
            }
            dirs.push((cwd, false));
            if dirs.len() >= 7 {
                break;
            }
        }
    }
    dirs.into_iter()
        .map(|(dir, workspace)| {
            let st = hippo_core::hooks::status(&dir).unwrap_or_default();
            let mut on = [false; 3];
            for (i, (_, has)) in st.iter().enumerate().take(3) {
                on[i] = *has;
            }
            HookDir {
                label: dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dir.display().to_string()),
                dir,
                on,
                workspace,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Commands

/// `hippo config <op> <key> [value] --yes`.
pub fn config_args(key: &str, op: &Op) -> Vec<String> {
    let mut a = vec!["config".to_owned()];
    match op {
        Op::Set(v) => a.extend(["set".into(), key.into(), v.clone()]),
        Op::Unset => a.extend(["unset".into(), key.into()]),
        Op::Add(v) => a.extend(["add".into(), key.into(), v.clone()]),
        Op::Remove(v) => a.extend(["remove".into(), key.into(), v.clone()]),
    }
    a.push("--yes".into());
    a
}

/// The config changes that turn `agent` on or off: on removes it from
/// `exclude_roots` (and adds it to `transcript_roots` when no pattern
/// matches it); off adds it to `exclude_roots`.
pub fn agent_ops(agent: &Agent, on: bool) -> Vec<(&'static str, Op)> {
    if on {
        let mut ops = Vec::new();
        if !agent.matched {
            ops.push(("transcript_roots", Op::Add(agent.display.clone())));
        } else {
            ops.push(("exclude_roots", Op::Remove(agent.display.clone())));
        }
        ops
    } else {
        vec![("exclude_roots", Op::Add(agent.display.clone()))]
    }
}

/// The ingest intervals the stepper offers, in minutes.
pub const INTERVALS: [u64; 8] = [1, 2, 5, 10, 15, 20, 30, 60];

/// The next interval up or down from `now`.
pub fn step_interval(now: u64, up: bool) -> u64 {
    let i = INTERVALS
        .iter()
        .position(|m| *m >= now)
        .unwrap_or(INTERVALS.len() - 1);
    let i = if up {
        (i + usize::from(INTERVALS[i] <= now)).min(INTERVALS.len() - 1)
    } else {
        i.saturating_sub(1)
    };
    INTERVALS[i]
}

/// `HH:MM` moved by 30 minutes, wrapping at midnight.
pub fn step_clock(at: &str, up: bool) -> String {
    let (h, m) = at.split_once(':').unwrap_or(("3", "30"));
    let total = h.parse::<i32>().unwrap_or(3) * 60 + m.parse::<i32>().unwrap_or(30);
    let total = (total / 30) * 30 + if up { 30 } else { -30 };
    let total = total.rem_euclid(24 * 60);
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// The snapshot retention choices: `None` keeps all.
pub const KEEP: [Option<u32>; 6] = [None, Some(7), Some(14), Some(30), Some(60), Some(90)];

pub fn step_keep(now: Option<u32>, up: bool) -> Option<u32> {
    let i = KEEP.iter().position(|k| *k == now).unwrap_or(0) as i32;
    let i = (i + if up { 1 } else { -1 }).clamp(0, KEEP.len() as i32 - 1) as usize;
    KEEP[i]
}

pub fn keep_label(k: Option<u32>) -> String {
    match k {
        None => "keep all".into(),
        Some(d) => format!("{d} days"),
    }
}

/// The context budget slider's range, in KB.
pub const BUDGET_KB: (usize, usize) = (8, 48);

/// What a doctor row's fix runs, when it has one: the command, or `None`.
pub fn doctor_fix(check: &str, status: &str) -> Option<Vec<String>> {
    if status == "ok" {
        return None;
    }
    let a: &[&str] = match check {
        "launchd" => &["schedule", "install"],
        "index" | "unindexed files" => &["ingest"],
        "lessons" => &["lessons", "scan"],
        _ => return None,
    };
    Some(a.iter().map(|s| (*s).to_owned()).collect())
}

/// `check | status | detail` rows of `hippo doctor`'s output.
pub fn doctor_rows(output: &str) -> Vec<(String, String, String)> {
    output
        .lines()
        .skip_while(|l| !l.starts_with("check | status"))
        .skip(1)
        .filter_map(|l| {
            let mut it = l.splitn(3, " | ");
            Some((
                it.next()?.trim().to_owned(),
                it.next()?.trim().to_owned(),
                it.next().unwrap_or("").trim().to_owned(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steppers_move_through_their_choices() {
        assert_eq!(step_interval(10, true), 15);
        assert_eq!(step_interval(10, false), 5);
        assert_eq!(step_interval(60, true), 60);
        assert_eq!(step_interval(1, false), 1);
        assert_eq!(step_interval(12, true), 15);
        assert_eq!(step_clock("03:30", true), "04:00");
        assert_eq!(step_clock("00:00", false), "23:30");
        assert_eq!(step_clock("23:30", true), "00:00");
        assert_eq!(step_keep(None, true), Some(7));
        assert_eq!(step_keep(Some(7), false), None);
        assert_eq!(step_keep(Some(90), true), Some(90));
    }

    #[test]
    fn agent_switches_are_config_changes() {
        let mut a = Agent {
            dir: "/u/.claude-work2/projects".into(),
            display: "~/.claude-work2/projects".into(),
            profile: Some("haf".into()),
            tool: "claude-code",
            files: 3,
            live: false,
            on: true,
            matched: true,
        };
        assert_eq!(a.name(), "haf");
        assert!(!a.never_used(), "it has sessions");
        let off = agent_ops(&a, false);
        assert_eq!(
            config_args(off[0].0, &off[0].1),
            [
                "config",
                "add",
                "exclude_roots",
                "~/.claude-work2/projects",
                "--yes"
            ]
        );
        a.matched = false;
        let on = agent_ops(&a, true);
        assert_eq!(on[0].0, "transcript_roots");
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // a synthetic profile folder in a temporary directory
    fn a_folder_without_sessions_or_meter_was_never_used() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".claude-typo").join("projects");
        std::fs::create_dir_all(&dir).unwrap();
        let a = Agent {
            dir: dir.clone(),
            display: "~/.claude-typo/projects".into(),
            profile: Some("typo".into()),
            tool: "claude-code",
            files: 0,
            live: false,
            on: true,
            matched: true,
        };
        assert!(a.never_used());
        // A signed-in profile has a meter: used, even with no session yet.
        std::fs::write(
            tmp.path().join(".claude-typo/.claude.json"),
            r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"five_hour":{"utilization":0}}}}"#,
        )
        .unwrap();
        assert!(!a.never_used());
    }

    #[test]
    fn doctor_output_is_read_into_rows_with_fixes() {
        let out = "check | status | detail\nconfig | ok | /h/config.toml\nlaunchd | problem | dev.hippo.ingest missing\nlinter | warn | 3 overdue | x\n";
        let rows = doctor_rows(out);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].2, "3 overdue | x");
        assert_eq!(
            doctor_fix(&rows[1].0, &rows[1].1).unwrap(),
            ["schedule", "install"]
        );
        assert!(doctor_fix(&rows[0].0, &rows[0].1).is_none());
        assert!(doctor_fix("linter", "warn").is_none());
    }
}

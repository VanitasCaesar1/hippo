//! What agent history this machine holds, before anything is indexed: `hippo
//! detect` and the app's first-run screens.
//!
//! [`detect`] matches each transcript root pattern, walks the directories
//! it names and counts the files the adapters would ingest, their bytes and
//! their newest and oldest modification times. It reads directory entries
//! and file metadata only, except that OpenCode's database is opened
//! read-only to count its sessions: nothing is written, and no index is
//! needed. It also names the agent tools whose folders exist but
//! that hippo has no adapter for yet.

use std::path::{Path, PathBuf};

use serde::Serialize;
use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::adapters;
use crate::config::expand_tilde;

/// One directory a root pattern matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dir {
    /// The directory on disk.
    pub path: PathBuf,
    /// The directory with the home directory written `~`.
    pub display: String,
    /// `claude-code`, `codex`, `dsh` or `opencode`: the tool whose files it
    /// holds, or, with none, the tool its path suggests.
    pub tool: &'static str,
    /// The Claude Code profile: `default` for `~/.claude`, `work` for
    /// `~/.claude-work`; `None` for Codex, dsh, OpenCode and other paths.
    pub profile: Option<String>,
    /// Transcript files an ingest would read (sub-agent transcripts count,
    /// as they do in the index); for OpenCode, the sessions in its
    /// database.
    pub files: u64,
    /// Their size in bytes.
    pub bytes: u64,
    /// The newest and oldest modification time among them.
    #[serde(with = "time::serde::rfc3339::option")]
    pub newest: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub oldest: Option<OffsetDateTime>,
    /// `exclude_roots` names it (or a directory above it): ingest skips it.
    pub excluded: bool,
}

/// One configured `transcript_roots` pattern and what it matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Root {
    /// The pattern as configured, `~` kept.
    pub pattern: String,
    /// The directories it matched, sorted by path.
    pub dirs: Vec<Dir>,
}

/// An agent tool hippo cannot read yet, and whether its folder is here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unsupported {
    pub name: &'static str,
    /// The folder, `~` kept.
    pub path: &'static str,
    pub present: bool,
}

/// Everything [`detect`] found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Detected {
    pub roots: Vec<Root>,
    pub unsupported: Vec<Unsupported>,
}

impl Detected {
    /// Marks the directories `exclude_roots` (as `config.toml` holds them)
    /// covers.
    pub fn mark_excluded(&mut self, exclude_roots: &[String]) {
        let excluded: Vec<PathBuf> = exclude_roots
            .iter()
            .map(|e| PathBuf::from(expand_tilde(e)))
            .collect();
        for root in &mut self.roots {
            for dir in &mut root.dirs {
                dir.excluded = adapters::is_excluded(&dir.path, &excluded);
            }
        }
    }
}

/// Agent tools with no adapter yet, and where they keep their folders.
const UNSUPPORTED: [(&str, &str); 2] = [("Cursor", "~/.cursor"), ("Gemini", "~/.gemini")];

/// Matches `patterns` (as `config.toml` holds them, `~` not yet expanded or
/// already expanded) and counts what each directory holds, leaving out
/// `$HIPPO_HOME/model-runs/` as ingest does. `user_home` writes paths with
/// `~`.
pub fn detect(patterns: &[String], hippo_home: &Path, user_home: Option<&Path>) -> Detected {
    let adapters = adapters::all();
    let model_runs = hippo_home.join("model-runs");
    let tilde = |path: &Path| -> String {
        match user_home.and_then(|home| path.strip_prefix(home).ok()) {
            Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Some(rest) => format!("~/{}", rest.display()),
            None => path.display().to_string(),
        }
    };
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut roots = Vec::new();
    for pattern in patterns {
        let expanded = expand_tilde(pattern);
        let mut matched: Vec<PathBuf> = match glob::glob(&expanded) {
            Ok(paths) => paths.flatten().filter(|p| p.is_dir()).collect(),
            Err(_) => Vec::new(),
        };
        matched.sort();
        let mut dirs = Vec::new();
        for path in matched {
            if seen.contains(&path) {
                continue;
            }
            seen.push(path.clone());
            let mut dir = Dir {
                display: tilde(&path),
                tool: tool_of(&path),
                profile: None,
                files: 0,
                bytes: 0,
                newest: None,
                oldest: None,
                excluded: false,
                path,
            };
            let (mut claude, mut codex, mut dsh, mut opencode) = (0u64, 0u64, 0u64, 0u64);
            let mut muse = 0u64;
            let entries = WalkDir::new(&dir.path)
                .into_iter()
                .filter_entry(|e| !e.path().starts_with(&model_runs) && adapters::descend(e));
            for entry in entries.flatten() {
                if !entry.file_type().is_file() {
                    continue;
                }
                let Some(adapter) = adapters.iter().find(|a| a.file_matches(entry.path())) else {
                    continue;
                };
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                // A database counts its sessions, each stamped with when it
                // last changed; one hippo cannot read counts none.
                let times: Vec<Option<OffsetDateTime>> = match adapter.sessions_in(entry.path()) {
                    Some(listed) => listed
                        .map(|l| l.into_iter().map(|l| l.modified).collect())
                        .unwrap_or_default(),
                    None => vec![meta.modified().ok().map(OffsetDateTime::from)],
                };
                let n = times.len() as u64;
                match adapter.tool().as_str() {
                    "codex" => codex += n,
                    "dsh" => dsh += n,
                    "opencode" => opencode += n.max(1),
                    "muse" => muse += n,
                    _ => claude += n,
                }
                dir.files += n;
                dir.bytes += meta.len();
                for at in times.into_iter().flatten() {
                    dir.newest = Some(dir.newest.map_or(at, |n| n.max(at)));
                    dir.oldest = Some(dir.oldest.map_or(at, |o| o.min(at)));
                }
            }
            if claude + codex + dsh + opencode + muse > 0 {
                dir.tool = if opencode > 0 && claude + codex + dsh + muse == 0 {
                    "opencode"
                } else if muse > claude.max(codex).max(dsh) {
                    "muse"
                } else if dsh > claude.max(codex) {
                    "dsh"
                } else if codex > claude {
                    "codex"
                } else {
                    "claude-code"
                };
            }
            if dir.tool == "claude-code" {
                dir.profile = crate::rollup::profile_of("claude-code", &dir.path);
            }
            dirs.push(dir);
        }
        roots.push(Root {
            pattern: pattern_display(pattern, user_home),
            dirs,
        });
    }
    let unsupported = UNSUPPORTED
        .iter()
        .map(|&(name, path)| Unsupported {
            name,
            path,
            present: Path::new(&expand_tilde(path)).is_dir(),
        })
        .collect();
    Detected { roots, unsupported }
}

/// The tool a directory's path suggests.
fn tool_of(path: &Path) -> &'static str {
    if path.components().any(|c| c.as_os_str() == ".codex") {
        "codex"
    } else if path.components().any(|c| c.as_os_str() == ".dsh") {
        "dsh"
    } else if path.file_name().is_some_and(|n| n == "opencode") {
        "opencode"
    } else if path.file_name().is_some_and(|n| n == "sessions")
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "muse")
    {
        "muse"
    } else {
        "claude-code"
    }
}

/// A pattern with the home directory written `~`.
fn pattern_display(pattern: &str, user_home: Option<&Path>) -> String {
    match user_home.and_then(|h| h.to_str()) {
        Some(home) if !home.is_empty() => match pattern.strip_prefix(home) {
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => pattern.to_owned(),
        },
        _ => pattern.to_owned(),
    }
}

impl Detected {
    /// Every matched directory, in order.
    pub fn dirs(&self) -> impl Iterator<Item = &Dir> {
        self.roots.iter().flat_map(|r| r.dirs.iter())
    }

    /// Transcript files in all.
    pub fn files(&self) -> u64 {
        self.dirs().map(|d| d.files).sum()
    }

    /// Their bytes in all.
    pub fn bytes(&self) -> u64 {
        self.dirs().map(|d| d.bytes).sum()
    }

    /// The oldest and newest modification times in all.
    pub fn span(&self) -> Option<(OffsetDateTime, OffsetDateTime)> {
        let oldest = self.dirs().filter_map(|d| d.oldest).min()?;
        let newest = self.dirs().filter_map(|d| d.newest).max()?;
        Some((oldest, newest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn counts_transcripts_per_profile_and_names_unsupported_tools() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let claude = home.join(".claude/projects/-w");
        let work = home.join(".claude-work/projects/-w/abc/subagents");
        let codex = home.join(".codex/sessions/2026/11/09");
        let empty = home.join(".claude-empty/projects");
        let dsh = home.join(".dsh/sessions/--w--/s1");
        for d in [&claude, &work, &codex, &empty, &dsh] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(claude.join("a.jsonl"), "12345").unwrap();
        fs::write(claude.join("notes.txt"), "not a transcript").unwrap();
        fs::write(work.join("agent-1.jsonl"), "1").unwrap();
        fs::write(work.join("../../b.jsonl"), "12").unwrap();
        fs::write(codex.join("rollout-2026-11-09T01-00-00-x.jsonl"), "123").unwrap();
        // A dsh session: only its newest generation is a transcript.
        fs::write(dsh.join("session.v4.jsonl.zstd"), "1234").unwrap();
        fs::write(dsh.join("session.v3.jsonl"), "older").unwrap();
        fs::write(dsh.join("session.lock"), "").unwrap();
        // A model run under $HIPPO_HOME is never counted.
        let hippo = home.join(".claude-hippo");
        fs::create_dir_all(hippo.join("projects/model-runs")).unwrap();
        fs::write(hippo.join("projects/model-runs/m.jsonl"), "x").unwrap();
        fs::create_dir_all(home.join(".gemini")).unwrap();

        // `~` would expand to the user's home, so the patterns are absolute.
        let patterns = [
            format!("{}/.claude*/projects", home.display()),
            format!("{}/.codex/sessions", home.display()),
            format!("{}/.dsh/sessions", home.display()),
        ];
        let found = detect(&patterns, &hippo.join("projects"), Some(home));
        assert_eq!(found.roots.len(), 3);
        assert_eq!(found.roots[0].pattern, "~/.claude*/projects");
        let rows: Vec<(&str, Option<&str>, u64, u64)> = found
            .dirs()
            .map(|d| (d.display.as_str(), d.profile.as_deref(), d.files, d.bytes))
            .collect();
        assert_eq!(
            rows,
            [
                ("~/.claude/projects", Some("default"), 1, 5),
                ("~/.claude-empty/projects", Some("empty"), 0, 0),
                ("~/.claude-hippo/projects", Some("hippo"), 0, 0),
                ("~/.claude-work/projects", Some("work"), 2, 3),
                ("~/.codex/sessions", None, 1, 3),
                ("~/.dsh/sessions", None, 1, 4),
            ]
        );
        assert_eq!(found.roots[1].dirs[0].tool, "codex");
        assert_eq!(found.roots[2].dirs[0].tool, "dsh");
        assert_eq!(found.files(), 5);
        assert_eq!(found.bytes(), 15);
        assert!(found.span().is_some());
        let gemini = found.unsupported.iter().find(|u| u.name == "Gemini");
        assert!(gemini.is_some());
        let json = serde_json::to_value(&found).unwrap();
        assert_eq!(json["roots"][0]["dirs"][0]["files"], 1);
    }

    #[test]
    fn an_opencode_database_counts_its_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let db = crate::adapters::opencode::tests::sessions(&home.join(".local/share"));
        // Its snapshots and cloned repositories are not transcripts.
        let repo = db.parent().unwrap().join("repos/r");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("x.jsonl"), "{}").unwrap();
        let patterns = [format!("{}/.local/share/opencode", home.display())];
        let found = detect(&patterns, &home.join(".hippo"), Some(home));
        let dir = &found.roots[0].dirs[0];
        assert_eq!(dir.display, "~/.local/share/opencode");
        assert_eq!(dir.tool, "opencode");
        assert_eq!(dir.profile, None);
        assert_eq!(dir.files, 7);
        assert_eq!(dir.bytes, fs::metadata(&db).unwrap().len());
        assert!(dir.newest.is_some() && dir.oldest < dir.newest);
        assert!(!found.unsupported.iter().any(|u| u.name == "OpenCode"));
    }
}

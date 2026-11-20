//! The rows of `hippo doctor` that check the hardening of
//! `docs/designs/common.md`, rule 4: permissions under `$HIPPO_HOME`, hooks
//! that run hippo by its absolute path from per-person files,
//! `$HIPPO_HOME/bin-path` for the plugins, and the binary the hooks name
//! against the checksum `install.sh` recorded. Also the workspace's
//! version floor and unknown keys (rule 2).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use super::{row, sample, Check, Status};
use crate::config::Workspace;
use crate::hooks::{self, Agent};
use crate::perms;
use crate::util::{hex, sha256};

/// `$HIPPO_HOME/install.json`, written by `install.sh` after it checked the
/// release asset against the published checksum.
#[derive(Debug, Clone, Deserialize)]
pub struct InstallRecord {
    pub version: String,
    /// Where the binary was written.
    pub binary: PathBuf,
    /// SHA-256 of the binary, lowercase hex.
    pub sha256: String,
}

/// `$HIPPO_HOME/install.json`.
pub fn install_record_path(home: &Path) -> PathBuf {
    home.join("install.json")
}

/// The record `install.sh` left, if any.
pub fn install_record(home: &Path) -> Option<InstallRecord> {
    let text = fs::read_to_string(install_record_path(home)).ok()?;
    serde_json::from_str(&text).ok()
}

/// One hook command that runs hippo, and where it is.
#[derive(Debug, Clone)]
pub struct HookCommand {
    pub file: PathBuf,
    pub agent: Agent,
    pub command: String,
}

/// The hippo hooks in `files`, in order.
pub fn hook_commands(files: &[(PathBuf, Agent)]) -> Vec<HookCommand> {
    files
        .iter()
        .flat_map(|(file, agent)| {
            hooks::hippo_commands(file, *agent)
                .into_iter()
                .map(|command| HookCommand {
                    file: file.clone(),
                    agent: *agent,
                    command,
                })
        })
        .collect()
}

/// The MCP client files hippo's MCP server may be configured in, under
/// `home`, as `hippo mcp install --client <client> --write` writes them
/// (`docs/mcp.md`). Claude Code keeps its servers in its own state and is
/// not read.
pub fn mcp_files(home: &Path) -> Vec<PathBuf> {
    let desktop = if cfg!(target_os = "macos") {
        "Library/Application Support/Claude/claude_desktop_config.json"
    } else {
        ".config/Claude/claude_desktop_config.json"
    };
    let copilot = match std::env::var_os("COPILOT_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("mcp-config.json"),
        None => home.join(".copilot/mcp-config.json"),
    };
    vec![
        home.join(".cursor/mcp.json"),
        home.join(desktop),
        home.join(".gemini/settings.json"),
        home.join(".codeium/windsurf/mcp_config.json"),
        home.join(".config/zed/settings.json"),
        copilot,
    ]
}

/// The `command` of the `hippo` server in each MCP client file of `files`
/// that has one, with its file.
pub fn mcp_commands(files: &[PathBuf]) -> Vec<(PathBuf, String)> {
    files
        .iter()
        .filter_map(|file| {
            let text = fs::read_to_string(file).ok()?;
            let value: serde_json::Value = serde_json::from_str(&text).ok()?;
            let server = ["mcpServers", "context_servers"]
                .iter()
                .find_map(|key| value.get(key)?.get("hippo"))?;
            let command = server.get("command")?.as_str()?;
            Some((file.clone(), command.to_owned()))
        })
        .collect()
}

/// The agent's `--agent` word for `hippo hooks install`.
fn agent_flag(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "",
        Agent::Codex => " --agent codex",
        Agent::Cursor => " --agent cursor",
        Agent::Gemini => " --agent gemini",
    }
}

/// `permissions`: anything under `$HIPPO_HOME` another account could read.
pub fn permissions_check(home: &Path) -> Check {
    let found = perms::loose(home);
    if found.is_empty() {
        return row(
            "permissions",
            Status::Ok,
            format!("{}: only you can read it", home.display()),
        );
    }
    let home_open = found.iter().any(|l| l.path == home);
    let names: Vec<String> = found
        .iter()
        .map(|l| {
            let shown = l.path.strip_prefix(home).unwrap_or(&l.path);
            let shown = if shown.as_os_str().is_empty() {
                home.display().to_string()
            } else {
                shown.display().to_string()
            };
            format!("{shown} ({:o})", l.mode)
        })
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let reach = if home_open {
        "other accounts on this machine can read them"
    } else {
        "the folder itself is closed, so other accounts cannot reach them yet"
    };
    row(
        "permissions",
        Status::Warn,
        format!(
            "{} files or folders under {} are readable by others: {}; {reach}. hippo doctor --fix-permissions makes them 0600 and 0700",
            found.len(),
            home.display(),
            sample(&names)
        ),
    )
}

/// `workspace settings`: a `min_hippo_version` above this hippo, or keys
/// it does not know, which make writing and model-calling commands refuse.
pub fn workspace_settings_check(ws: &Workspace) -> Check {
    match ws.write_refusal() {
        Some(why) => row(
            "workspace settings",
            Status::Problem,
            format!(
                "{}. Hooks and context keep working.",
                why.trim_end_matches('.')
            ),
        ),
        None => row(
            "workspace settings",
            Status::Ok,
            match &ws.min_hippo_version {
                Some(floor) => format!(
                    "min_hippo_version {floor}; this is {}",
                    crate::config::HIPPO_VERSION
                ),
                None => "every key known".to_owned(),
            },
        ),
    }
}

/// `accept`: the loosenings that wait for `hippo accept` on this machine
/// (`docs/designs/common.md`, rule 3). A warning, never a problem: the
/// values in force are the stricter ones.
pub fn accept_check(pending: &crate::accept::Pending) -> Check {
    if pending.is_empty() {
        return row("accept", Status::Ok, "no loosening waiting");
    }
    let mut parts: Vec<String> = pending
        .settings
        .iter()
        .map(|w| format!("loosening waiting: {w}"))
        .collect();
    parts.extend(
        pending
            .guard
            .iter()
            .map(|g| format!("loosening waiting: guard rules: {g}")),
    );
    row(
        "accept",
        Status::Warn,
        format!(
            "{}; the stricter values stay in force until you run hippo accept (or Accept in the app)",
            parts.join("; ")
        ),
    )
}

/// `guard`: where the guard is installed (each Claude Code account, Gemini
/// CLI), the binary each hook runs, the rules in force, what could not be
/// read, and the last day's failures.
pub fn guard_check(
    cfg: &crate::config::Config,
    ws: Option<&Workspace>,
    user_home: Option<&Path>,
) -> Check {
    use crate::hooks::GuardAgent;
    let compiled = match crate::guard::current(cfg, ws) {
        Ok(c) => c,
        Err(e) => {
            return row(
                "guard",
                Status::Problem,
                format!("the rules do not compile: {e}"),
            )
        }
    };
    let targets = user_home
        .map(|h| crate::guard::install::targets(h, &GuardAgent::ALL))
        .unwrap_or_default();
    let mut installed = Vec::new();
    let mut missing = Vec::new();
    let mut stale_fail = Vec::new();
    for (agent, path) in &targets {
        if let Ok(Some(command)) = hooks::guard_installed(path, *agent) {
            installed.push(path.display().to_string());
            let program = hooks::command_program(&command).unwrap_or_default();
            if !Path::new(&program).is_file() {
                missing.push(format!(
                    "{} runs {program}, which does not exist",
                    path.display()
                ));
            }
            let blocks_on_failure =
                fs::read_to_string(path).is_ok_and(|t| t.contains("\"onFailure\""));
            let closed = compiled.fail == crate::accept::Fail::Closed;
            if *agent == GuardAgent::ClaudeCode && closed != blocks_on_failure {
                stale_fail.push(path.display().to_string());
            }
        }
    }
    let activity = crate::guard::activity(&cfg.home);
    let mut parts = vec![if installed.is_empty() {
        "not installed in any agent (hippo guard install)".to_owned()
    } else {
        format!(
            "installed in {} of {} agent settings files",
            installed.len(),
            targets.len()
        )
    }];
    parts.push(format!(
        "rules: {}; fail = {}; warn-only",
        crate::guard::summary(&compiled.in_force),
        compiled.fail
    ));
    if activity.checked_today > 0 || activity.matched_day > 0 {
        parts.push(format!(
            "{} calls checked today, {} matched a rule in the last day",
            activity.checked_today, activity.matched_day
        ));
    }
    let mut status = Status::Ok;
    if activity.failures_day > 0 {
        status = Status::Warn;
        parts.push(format!(
            "{} guard failures in the last day ({})",
            activity.failures_day,
            crate::guard::failures_path(&cfg.home).display()
        ));
    }
    if !stale_fail.is_empty() {
        status = Status::Warn;
        parts.push(format!(
            "fail = {} is not what these hooks were installed with: {}; run hippo guard install --yes",
            compiled.fail,
            stale_fail.join(", ")
        ));
    }
    if !compiled.problems.is_empty() {
        status = Status::Warn;
        parts.push(format!("cannot use: {}", compiled.problems.join("; ")));
    }
    if !missing.is_empty() {
        status = Status::Problem;
        parts.push(missing.join("; "));
    }
    row("guard", status, parts.join("; "))
}

/// Whether git tracks `path` in the work tree at `root`. `false` when git
/// cannot say.
fn git_tracks(root: &Path, path: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `hooks`: every hippo hook runs hippo by an absolute path that exists,
/// none sits in a workspace's shared `.claude/settings.json`, and git does
/// not track `.claude/settings.local.json`.
pub fn hooks_check(
    ws: Option<&Workspace>,
    user_files: &[(PathBuf, Agent)],
    mcp: &[PathBuf],
) -> Check {
    let mut files = Vec::new();
    let mut issues: Vec<String> = Vec::new();
    if let Some(ws) = ws {
        let local = hooks::settings_path(&ws.root);
        files.push((local.clone(), Agent::ClaudeCode));
        if let Ok(shared) = hooks::claude_code_shared(&ws.root) {
            if !shared.is_empty() {
                issues.push(format!(
                    "{} holds hippo's hooks ({}), in the settings a team commits: hippo hooks install --yes moves them to .claude/settings.local.json",
                    hooks::shared_settings_path(&ws.root).display(),
                    shared.join(", ")
                ));
            }
        }
        if local.exists() && git_tracks(&ws.root, &local) {
            issues.push(format!(
                "git tracks {}, which holds this machine's paths: git rm --cached -- .claude/settings.local.json",
                local.display()
            ));
        }
    }
    files.extend(user_files.iter().cloned());
    let found = hook_commands(&files);
    let mut on_path = Vec::new();
    for hook in &found {
        match hooks::command_program(&hook.command) {
            Some(program) if Path::new(&program).is_absolute() => {
                if !Path::new(&program).is_file() {
                    issues.push(format!(
                        "{} runs {program}, which is not there: hippo hooks install{}",
                        hook.file.display(),
                        agent_flag(hook.agent)
                    ));
                }
            }
            _ => {
                let line = format!(
                    "{}: hippo hooks install{}",
                    hook.file.display(),
                    agent_flag(hook.agent)
                );
                if !on_path.contains(&line) {
                    on_path.push(line);
                }
            }
        }
    }
    for (file, command) in mcp_commands(mcp) {
        let program = Path::new(&command);
        if !program.is_absolute() || !program.is_file() {
            on_path.push(format!(
                "{}: hippo mcp install --write for that client",
                file.display()
            ));
        }
    }
    if !on_path.is_empty() {
        issues.push(format!(
            "hooks that find hippo through PATH, where another program named hippo could stand in: {}",
            on_path.join("; ")
        ));
    }
    issues.dedup();
    if !issues.is_empty() {
        return row("hooks", Status::Warn, issues.join(". "));
    }
    if found.is_empty() {
        return row("hooks", Status::Ok, "no hippo hooks installed");
    }
    row("hooks", Status::Ok, {
        let mut files: Vec<&PathBuf> = found.iter().map(|h| &h.file).collect();
        files.dedup();
        format!(
            "{} hooks in {} {}, each running hippo by its absolute path",
            found.len(),
            files.len(),
            if files.len() == 1 { "file" } else { "files" }
        )
    })
}

/// `bin-path`: the file the plugins read to run hippo by its path.
pub fn bin_path_check(home: &Path) -> Check {
    let file = hooks::bin_path_file(home);
    match hooks::read_bin_path(home) {
        None => row(
            "bin-path",
            Status::Warn,
            format!(
                "{} is missing, so the Claude Code, dsh and OpenCode plugins find hippo through PATH; hippo hooks install or hippo init writes it",
                file.display()
            ),
        ),
        Some(path) if !path.is_absolute() || !path.is_file() => row(
            "bin-path",
            Status::Warn,
            format!(
                "{} names {}, which is not a file, so the plugins find hippo through PATH; hippo hooks install or hippo init rewrites it",
                file.display(),
                path.display()
            ),
        ),
        Some(path) => row("bin-path", Status::Ok, path.display().to_string()),
    }
}

fn file_sha256(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|bytes| hex(&sha256(&bytes)))
}

/// `install`: the binary `install.sh` wrote still has the checksum it
/// recorded, and the binaries the hooks and `bin-path` name are that one.
pub fn install_check(
    home: &Path,
    user_files: &[(PathBuf, Agent)],
    mcp: &[PathBuf],
    ws: Option<&Workspace>,
) -> Check {
    let Some(record) = install_record(home) else {
        return row(
            "install",
            Status::Ok,
            "no install record: hippo was not installed by install.sh, so there is no checksum to compare",
        );
    };
    let Some(have) = file_sha256(&record.binary) else {
        return row(
            "install",
            Status::Warn,
            format!(
                "{} recorded {} (hippo {}), which cannot be read now",
                install_record_path(home).display(),
                record.binary.display(),
                record.version
            ),
        );
    };
    if have != record.sha256 {
        return row(
            "install",
            Status::Problem,
            format!(
                "{} is not the binary install.sh checked: its sha256 is {have}, the release's {}. If you replaced it yourself, run install.sh again; otherwise treat this machine as compromised",
                record.binary.display(),
                record.sha256
            ),
        );
    }
    let mut files: Vec<(PathBuf, Agent)> = Vec::new();
    if let Some(ws) = ws {
        files.push((hooks::settings_path(&ws.root), Agent::ClaudeCode));
    }
    files.extend(user_files.iter().cloned());
    let mut programs: Vec<PathBuf> = hook_commands(&files)
        .iter()
        .filter_map(|h| hooks::command_program(&h.command))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .collect();
    programs.extend(
        mcp_commands(mcp)
            .into_iter()
            .map(|(_, c)| PathBuf::from(c))
            .filter(|p| p.is_absolute()),
    );
    programs.extend(hooks::read_bin_path(home));
    programs.sort();
    programs.dedup();
    let installed = fs::canonicalize(&record.binary).unwrap_or(record.binary.clone());
    let other: Vec<String> = programs
        .iter()
        .filter(|p| {
            fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()) != installed
                && file_sha256(p).is_none_or(|sum| sum != record.sha256)
        })
        .map(|p| p.display().to_string())
        .collect();
    if !other.is_empty() {
        let names: Vec<&str> = other.iter().map(String::as_str).collect();
        return row(
            "install",
            Status::Warn,
            format!(
                "hooks or bin-path run {}, not the binary install.sh checked ({}); hippo hooks install, run from that binary, points them back",
                sample(&names),
                record.binary.display()
            ),
        );
    }
    row(
        "install",
        Status::Ok,
        format!(
            "{} matches the checksum of hippo {} recorded at install",
            record.binary.display(),
            record.version
        ),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn ws(root: &Path) -> Workspace {
        let text = "notes_dir = \"n\"\nentry_note = \"n/E.md\"\n";
        fs::create_dir_all(root.join(".hippo")).unwrap();
        fs::write(root.join(".hippo/hippo.toml"), text).unwrap();
        let cfg_home = root.join("home");
        fs::create_dir_all(&cfg_home).unwrap();
        fs::write(
            cfg_home.join("config.toml"),
            format!("workspace = \"{}\"\n", root.display()),
        )
        .unwrap();
        Workspace::load(&crate::config::Config::load_in(&cfg_home).unwrap()).unwrap()
    }

    #[test]
    fn hooks_on_path_or_in_shared_settings_are_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ws = ws(root);
        assert_eq!(hooks_check(Some(&ws), &[], &[]).status, Status::Ok);

        let codex = root.join("codex-hooks.json");
        hooks::install_file(&codex, Agent::Codex, "hippo").unwrap();
        let check = hooks_check(Some(&ws), &[(codex.clone(), Agent::Codex)], &[]);
        assert_eq!(check.status, Status::Warn);
        assert!(check.detail.contains("through PATH"), "{}", check.detail);
        assert!(check.detail.contains("--agent codex"), "{}", check.detail);

        let bin = root.join("hippo-bin");
        fs::write(&bin, "x").unwrap();
        hooks::install_file(&codex, Agent::Codex, bin.to_str().unwrap()).unwrap();
        let check = hooks_check(Some(&ws), &[(codex.clone(), Agent::Codex)], &[]);
        assert_eq!(check.status, Status::Ok, "{}", check.detail);

        let shared = hooks::shared_settings_path(root);
        hooks::install_file(&shared, Agent::ClaudeCode, bin.to_str().unwrap()).unwrap();
        let check = hooks_check(Some(&ws), &[(codex, Agent::Codex)], &[]);
        assert_eq!(check.status, Status::Warn);
        assert!(check.detail.contains("--yes"), "{}", check.detail);
    }

    #[test]
    fn the_install_record_catches_a_replaced_binary() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("h");
        fs::create_dir_all(&home).unwrap();
        assert_eq!(install_check(&home, &[], &[], None).status, Status::Ok);

        let bin = dir.path().join("hippo");
        fs::write(&bin, "release bytes").unwrap();
        let sum = hex(&sha256(b"release bytes"));
        fs::write(
            install_record_path(&home),
            format!(
                "{{\"version\":\"0.5.0\",\"binary\":\"{}\",\"sha256\":\"{sum}\"}}",
                bin.display()
            ),
        )
        .unwrap();
        hooks::write_bin_path(&home, &bin).unwrap();
        assert_eq!(install_check(&home, &[], &[], None).status, Status::Ok);

        // A hook running another binary is a warning.
        let other = dir.path().join("other-hippo");
        fs::write(&other, "other bytes").unwrap();
        let gemini = dir.path().join("gemini.json");
        hooks::install_file(&gemini, Agent::Gemini, other.to_str().unwrap()).unwrap();
        let check = install_check(&home, &[(gemini, Agent::Gemini)], &[], None);
        assert_eq!(check.status, Status::Warn, "{}", check.detail);

        // The binary itself replaced is a problem.
        fs::write(&bin, "tampered").unwrap();
        let check = install_check(&home, &[], &[], None);
        assert_eq!(check.status, Status::Problem);
        assert!(check.detail.contains("not the binary install.sh checked"));
    }

    #[test]
    fn bin_path_missing_or_stale_is_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("h");
        assert_eq!(bin_path_check(&home).status, Status::Warn);
        hooks::write_bin_path(&home, &dir.path().join("gone")).unwrap();
        assert_eq!(bin_path_check(&home).status, Status::Warn);
        let bin = dir.path().join("hippo");
        fs::write(&bin, "x").unwrap();
        hooks::write_bin_path(&home, &bin).unwrap();
        assert_eq!(bin_path_check(&home).status, Status::Ok);
    }
}

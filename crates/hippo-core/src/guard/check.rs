//! One pending tool call against the compiled rules: the self-protection
//! rules, the built-in kinds (read with Part A's own code), a repository
//! cloned in the last day, and the `match` and `path` rules. Then the
//! answer in each agent's hook format.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use super::{active_layer, default_action, Action, Compiled, Rule, KINDS, WARN_ONLY};
use crate::incidents::guard as kinds;
use crate::index::StoredTurn;
use crate::xray::shell;

/// A pending tool call, from any agent's hook payload.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The tool's name as the agent gives it (`Bash`, `run_shell_command`).
    pub tool: String,
    /// The tool's input.
    pub input: Value,
    /// The session's working folder.
    pub cwd: Option<PathBuf>,
    pub session: Option<String>,
    /// Nobody can be asked: a mode that skips approvals.
    pub headless: bool,
}

/// Reads a hook payload. Claude Code, Codex, Gemini CLI and the plugins
/// all send `tool_name`, `tool_input` and `cwd`; Claude Code's
/// `permission_mode` says whether anyone can be asked, the plugins say
/// `headless`. Codex's `PreToolUse` cannot ask (it refuses an `ask`
/// answer), so nobody can be asked there.
pub fn parse_call(agent: &str, payload: &Value) -> Option<Call> {
    let tool = payload.get("tool_name")?.as_str()?.to_owned();
    let input = match payload.get("tool_input").cloned().unwrap_or(Value::Null) {
        // Some agents send the input as a JSON string.
        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        v => v,
    };
    // Cursor has no top-level `cwd`: a shell call names its folder in its
    // input, and every payload lists the workspace's roots.
    let root = payload
        .get("workspace_roots")
        .and_then(Value::as_array)
        .and_then(|r| r.first());
    let cwd = [payload.get("cwd"), input.get("cwd"), root]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .map(PathBuf::from);
    let session = payload
        .get("session_id")
        .or_else(|| payload.get("conversation_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let headless = match agent {
        "claude-code" => matches!(
            payload.get("permission_mode").and_then(Value::as_str),
            Some("bypassPermissions" | "dontAsk")
        ),
        // Neither can ask from its pre-tool hook: Codex refuses an `ask`
        // answer, cursor-agent ignores one.
        "codex" | "cursor" => true,
        _ => payload
            .get("headless")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    Some(Call {
        tool,
        input,
        cwd,
        session,
        headless,
    })
}

/// One rule that matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hit {
    /// The rule, as people see it.
    pub rule: String,
    pub kind: Option<String>,
    /// The action the rule is configured with.
    pub action: Action,
    /// Why, in hippo's voice; quotes only filtered hosts and paths.
    pub reason: String,
    /// A self-protection rule: it blocks in every release.
    pub self_protection: bool,
}

/// What the guard answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    /// `None` when nothing matched.
    pub action: Option<Action>,
    /// The strictest configured action among the hits.
    pub configured: Option<Action>,
    pub message: String,
    pub hits: Vec<Hit>,
}

/// The action that takes effect for a configured one: in this release
/// everything warns; later, `ask` where nobody can be asked becomes
/// `block` (`docs/designs/guard.md`, When nobody can be asked).
pub fn effective(configured: Action, headless: bool, warn_only: bool) -> Action {
    match configured {
        _ if warn_only => Action::Warn,
        Action::Ask if headless => Action::Block,
        a => a,
    }
}

/// The decision for a call's hits: a self-protection hit blocks; any other
/// takes [`effective`] of the strictest configured action.
pub fn decide(hits: Vec<Hit>, headless: bool) -> Decision {
    if hits.is_empty() {
        return Decision {
            action: None,
            configured: None,
            message: String::new(),
            hits,
        };
    }
    let configured = hits.iter().map(|h| h.action).max();
    let protect = hits.iter().find(|h| h.self_protection);
    let (action, message) = match protect {
        Some(h) => (
            Action::Block,
            format!(
                "hippo guard blocked this: {} Stop, and tell the person; they can run it themselves if they want it.",
                h.reason
            ),
        ),
        None => {
            let action = effective(configured.unwrap_or_default(), headless, WARN_ONLY);
            let reasons: Vec<&str> = hits.iter().map(|h| h.reason.as_str()).collect();
            let message = match action {
                Action::Warn => format!("hippo guard: {} Tell the person.", reasons.join(" ")),
                Action::Ask => format!("hippo guard asks: {}", reasons.join(" ")),
                Action::Block => format!(
                    "hippo guard blocked this: {}{} Stop, and tell the person; they can run it themselves if they want it.",
                    reasons.join(" "),
                    if headless && configured == Some(Action::Ask) {
                        " Nobody can be asked in this mode, so the question became a refusal."
                    } else {
                        ""
                    }
                ),
            };
            (action, message)
        }
    };
    Decision {
        action: Some(action),
        configured,
        message,
        hits,
    }
}

/// The hook's answer for `agent`: what to print on stdout. Nothing when
/// no rule matched, so the agent's own approval flow goes on unchanged.
pub fn respond(agent: &str, decision: &Decision) -> String {
    let Some(action) = decision.action else {
        return match agent {
            "dsh" | "opencode" => json!({"decision": "none"}).to_string(),
            // An empty object: no permission, so Cursor's own approval
            // goes on as if no hook ran. (No output at all counts as a
            // failed hook in Cursor's log.)
            "cursor" => "{}".to_owned(),
            _ => String::new(),
        };
    };
    let m = &decision.message;
    let v = match (agent, action) {
        // Codex reads Claude Code's shape (codex-rs/hooks/src/engine/
        // output_parser.rs, 0.170.0): `additionalContext` reaches the
        // model, `systemMessage` the person, `deny` refuses. It has no
        // `ask`; `parse_call` makes every Codex call headless, so a
        // question has become a refusal by the time it gets here.
        ("claude-code" | "codex", Action::Warn) => json!({
            "hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": m},
            "systemMessage": m,
        }),
        ("claude-code", Action::Ask) => json!({
            "hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "ask", "permissionDecisionReason": m},
        }),
        ("claude-code", Action::Block) | ("codex", Action::Ask | Action::Block) => json!({
            "hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": m},
        }),
        // cursor-agent's preToolUse (2026.11.19): `additional_context`
        // without a `permission` reaches the model and lets the call go
        // on through Cursor's own approval; only `deny` refuses, with
        // `user_message` as the reason. It ignores `ask`; `parse_call`
        // makes every Cursor call headless.
        ("cursor", Action::Warn) => json!({"additional_context": m}),
        ("cursor", _) => json!({"permission": "deny", "user_message": m, "agent_message": m}),
        // Gemini CLI's BeforeTool has no question and no context for the
        // model: a warning reaches the person, a refusal the agent.
        ("gemini", Action::Warn) => json!({"systemMessage": m}),
        ("gemini", _) => json!({"decision": "deny", "reason": m}),
        (_, Action::Warn) => json!({"decision": "warn", "reason": m}),
        (_, Action::Ask) => json!({"decision": "ask", "reason": m}),
        (_, Action::Block) => json!({"decision": "block", "reason": m}),
    };
    v.to_string()
}

/// The agents `hippo guard check --agent` answers for.
pub const AGENTS: [&str; 6] = [
    "claude-code",
    "codex",
    "cursor",
    "gemini",
    "dsh",
    "opencode",
];

/// The agent a payload really came from. cursor-agent also runs the
/// hooks in `~/.claude/settings.json`, so Claude Code's guard entry gets
/// Cursor's calls too; they carry `cursor_version`.
pub fn payload_agent<'a>(agent: &'a str, payload: &Value) -> &'a str {
    if agent == "claude-code" && payload.get("cursor_version").is_some() {
        "cursor"
    } else {
        agent
    }
}

/// Every rule that matches `call`.
pub fn evaluate(compiled: &Compiled, call: &Call) -> Vec<Hit> {
    let in_ws = call
        .cwd
        .as_deref()
        .is_some_and(|d| compiled.in_workspace(d));
    let active = |r: &&Rule| active_layer(&r.layer, in_ws);
    let input = serde_json::to_string(&call.input).unwrap_or_default();
    let script = kinds::shell_script(&call.tool, &input);
    let files = kinds::file_call(&call.tool, &input);
    let mut hits = self_protection(compiled, call, script.as_deref(), files.as_ref());

    // The built-in kinds, as Part A reads them from a transcript.
    let turn = StoredTurn {
        ordinal: 0,
        role: "tool".into(),
        ts: String::new(),
        tool_name: Some(call.tool.clone()),
        text: format!("{} {input}", call.tool),
    };
    let mut seen_kinds: Vec<&str> = Vec::new();
    for s in kinds::signals(std::slice::from_ref(&turn)) {
        let kind = s.kind.as_str();
        if !KINDS.contains(&kind) || allowed(compiled, in_ws, kind, &s.text, &s.extra) {
            continue;
        }
        if seen_kinds.contains(&kind) {
            continue;
        }
        seen_kinds.push(kind);
        hits.push(kind_hit(compiled, in_ws, kind, &s.extra, &s.text));
    }
    if !seen_kinds.contains(&"fresh-clone-exec") {
        if let (Some(script), Some(cwd)) = (&script, &call.cwd) {
            if let Some(origin) = fresh_clone_run(compiled, in_ws, script, cwd) {
                hits.push(kind_hit(compiled, in_ws, "fresh-clone-exec", &origin, ""));
            }
        }
    }

    // `match` rules over a shell call, `path` rules over a file call.
    for rule in compiled.in_force.rules.iter().filter(active) {
        let matched = match (&rule.pattern, &rule.path) {
            (Some(p), _) => script
                .as_deref()
                .is_some_and(|s| regex::Regex::new(p).is_ok_and(|re| re.is_match(s))),
            (None, Some(glob)) => files.as_ref().is_some_and(|(_, paths, _)| {
                let pattern = glob::Pattern::new(&expand_home(glob));
                paths
                    .iter()
                    .any(|p| pattern.as_ref().is_ok_and(|g| g.matches(&expand_home(p))))
            }),
            _ => false,
        };
        if matched {
            let name = rule
                .name
                .clone()
                .unwrap_or_else(|| "a team rule".to_owned());
            hits.push(Hit {
                rule: rule.label(),
                kind: None,
                action: rule.action,
                reason: match &rule.reason {
                    Some(r) => format!(
                        "{} (rule \"{}\").",
                        r.trim_end_matches('.'),
                        one_line(&name)
                    ),
                    None => format!("this matches the rule \"{}\".", one_line(&name)),
                },
                self_protection: false,
            });
        }
    }
    hits
}

fn one_line(text: &str) -> String {
    let t: String = text.chars().filter(|c| !c.is_control()).take(120).collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The hit for a built-in kind: its action from the strictest active rule
/// that names it, else its default.
fn kind_hit(compiled: &Compiled, in_ws: bool, kind: &str, detail: &str, text: &str) -> Hit {
    let action = compiled
        .in_force
        .rules
        .iter()
        .filter(|r| active_layer(&r.layer, in_ws) && r.kind.as_deref() == Some(kind))
        .map(|r| r.action)
        .max()
        .unwrap_or_else(|| default_action(kind, detail));
    let shown = kinds::shown(detail);
    let reason = match kind {
        "remote-script" => format!(
            "this runs a script downloaded from {shown} without reading it first."
        ),
        "fresh-clone-exec" => format!(
            "this runs the code of a repository cloned recently ({shown}): its install and build scripts run with the person's access. An OS sandbox is the safer place for an unfamiliar repository."
        ),
        "credential-read" => {
            let path = text.rsplit(' ').next().unwrap_or("");
            format!(
                "this reads a credential file ({detail}: {}).",
                kinds::shown(&kinds::home_relative(path))
            )
        }
        _ => format!("this changes {detail}, which turns hooks off or changes hippo's state."),
    };
    Hit {
        rule: kind.to_owned(),
        kind: Some(kind.to_owned()),
        action,
        reason,
        self_protection: false,
    }
}

/// Whether an allow entry active for the call silences a kind: every URL
/// of a `remote-script` on an allowed host, a `fresh-clone-exec` origin
/// that is allowed.
fn allowed(compiled: &Compiled, in_ws: bool, kind: &str, text: &str, detail: &str) -> bool {
    let entries = |what: &'static str| {
        compiled
            .in_force
            .allow
            .iter()
            .filter(move |a| a.what == what && active_layer(&a.layer, in_ws))
            .map(|a| a.value.as_str())
    };
    match kind {
        "remote-script" => {
            let urls: Vec<String> = url_re()
                .captures_iter(text)
                .map(|c| c[1].to_owned())
                .collect();
            !urls.is_empty()
                && urls
                    .iter()
                    .all(|u| entries("host").any(|h| u.starts_with(h)))
        }
        "fresh-clone-exec" => entries("origin").any(|o| detail.starts_with(o)),
        _ => false,
    }
}

fn url_re() -> &'static regex::Regex {
    static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"https?://([^\s'"`)|;&<>]+)"#).expect("valid regex")
    });
    &URL
}

/// A repository's code run in a folder whose git repository was cloned in
/// the last day, outside the workspace: the coding-assignment case across
/// calls, read from the disk. Returns the clone's origin.
fn fresh_clone_run(compiled: &Compiled, in_ws: bool, script: &str, cwd: &Path) -> Option<String> {
    let mut dir = cwd.to_path_buf();
    for c in shell::simple_commands(script) {
        let Some((program, args)) = c.program() else {
            continue;
        };
        if program == "cd" || program == "pushd" {
            if let Some(d) = args.first() {
                dir = resolve(&dir, d, None);
            }
            continue;
        }
        let Some(named) = kinds::executes_repo_code(&program, &args) else {
            continue;
        };
        let at = match named {
            Some(d) => resolve(&dir, &d, None),
            None => dir.clone(),
        };
        if compiled.in_workspace(&at) {
            continue;
        }
        let Some(repo) = at.ancestors().find(|a| a.join(".git").is_dir()) else {
            continue;
        };
        let git = repo.join(".git");
        let made = std::fs::metadata(&git)
            .ok()
            .and_then(|m| m.created().or_else(|_| m.modified()).ok())?;
        let fresh = made
            .elapsed()
            .is_ok_and(|age| age < std::time::Duration::from_secs(24 * 3600));
        if !fresh {
            continue;
        }
        let origin = std::fs::read_to_string(git.join("config"))
            .ok()
            .and_then(|text| origin_url(&text))
            .unwrap_or_else(|| "no origin".to_owned());
        let allowed_origin = kinds::ALLOWED_ORIGINS
            .iter()
            .copied()
            .chain(
                compiled
                    .in_force
                    .allow
                    .iter()
                    .filter(|a| a.what == "origin" && active_layer(&a.layer, in_ws))
                    .map(|a| a.value.as_str()),
            )
            .any(|o| origin.starts_with(o));
        if !allowed_origin {
            return Some(origin);
        }
    }
    None
}

/// The `url` of `[remote "origin"]` in a git config, as `host/path`.
fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_origin = t == "[remote \"origin\"]";
            continue;
        }
        if in_origin {
            if let Some(url) = t.strip_prefix("url").map(str::trim_start) {
                let url = url.strip_prefix('=')?.trim();
                let u = url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_start_matches("ssh://")
                    .trim_start_matches("git://");
                let u = match u.split_once('@') {
                    Some((_, rest)) => rest.replacen(':', "/", 1),
                    None => u.to_owned(),
                };
                return Some(u.trim_end_matches('/').trim_end_matches(".git").to_owned());
            }
        }
    }
    None
}

/// `~` and `$HOME` written as the home folder.
fn expand_home(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if path == "~" {
        return home;
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("{}/{rest}", home.trim_end_matches('/'));
    }
    for var in ["$HOME/", "${HOME}/"] {
        if let Some(rest) = path.strip_prefix(var) {
            return format!("{}/{rest}", home.trim_end_matches('/'));
        }
    }
    path.to_owned()
}

/// `word` as an absolute path: `~`, `$HOME`, `$HIPPO_HOME` expanded,
/// relative to `cwd`, `.` and `..` removed, and the part that exists
/// resolved through links.
fn resolve(cwd: &Path, word: &str, hippo_home: Option<&Path>) -> PathBuf {
    let word = word.trim_matches(|c| c == '"' || c == '\'');
    let mut text = expand_home(word);
    if let Some(home) = hippo_home {
        for var in ["$HIPPO_HOME", "${HIPPO_HOME}"] {
            if let Some(rest) = text.strip_prefix(var) {
                text = format!("{}{rest}", home.display());
                break;
            }
        }
    }
    let joined = cwd.join(&text);
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    // Resolve the longest part that exists (`/var` → `/private/var`).
    let mut existing = out.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return out,
        }
    }
    let mut real = std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
    for name in rest.into_iter().rev() {
        real.push(name);
    }
    real
}

/// The places only the person may write: `$HIPPO_HOME/guard/`,
/// `$HIPPO_HOME/accepted/`, `$HIPPO_HOME/config.toml`.
fn protected(home: &Path, path: &Path) -> Option<&'static str> {
    let homes = [home.to_path_buf(), PathBuf::from(expand_home("~/.hippo"))];
    for h in homes {
        if path == h.join("config.toml") {
            return Some("$HIPPO_HOME/config.toml");
        }
        if path.starts_with(h.join("guard")) {
            return Some("$HIPPO_HOME/guard/");
        }
        if path.starts_with(h.join("accepted")) {
            return Some("$HIPPO_HOME/accepted/");
        }
    }
    None
}

/// The self-protection rules (`docs/designs/guard.md`, Self-protection):
/// an agent's call to the commands only the person may run, and an
/// agent's write under `$HIPPO_HOME/guard/`, `accepted/` or to
/// `config.toml`.
fn self_protection(
    compiled: &Compiled,
    call: &Call,
    script: Option<&str>,
    files: Option<&(kinds::FileOp, Vec<String>, String)>,
) -> Vec<Hit> {
    let cwd = call.cwd.clone().unwrap_or_else(|| PathBuf::from("/"));
    let mut found: Vec<String> = Vec::new();
    if let Some((op, paths, _)) = files {
        if *op == kinds::FileOp::Write {
            for p in paths {
                if let Some(place) =
                    protected(&compiled.home, &resolve(&cwd, p, Some(&compiled.home)))
                {
                    found.push(format!("a write to {place}"));
                }
            }
        }
    }
    if let Some(script) = script {
        let mut dir = cwd.clone();
        for c in shell::simple_commands(script) {
            let Some((program, args)) = c.program() else {
                continue;
            };
            if program == "cd" || program == "pushd" {
                if let Some(d) = args.first() {
                    dir = resolve(&dir, d, Some(&compiled.home));
                }
                continue;
            }
            if program == "hippo" {
                let sub: Vec<&str> = args
                    .iter()
                    .copied()
                    .filter(|a| !a.starts_with('-'))
                    .collect();
                let listing = args.contains(&"--list");
                let what = match sub.as_slice() {
                    ["accept", ..] if !listing => Some("hippo accept".to_owned()),
                    ["guard", w @ ("accept" | "allow" | "uninstall"), ..] => {
                        Some(format!("hippo guard {w}"))
                    }
                    ["hooks", "uninstall", ..] => Some("hippo hooks uninstall".to_owned()),
                    ["lessons", "share", ..] => Some("hippo lessons share".to_owned()),
                    ["config", op @ ("set" | "unset" | "add" | "remove"), key, ..]
                        if crate::settings::SECURITY_KEYS.contains(key)
                            || *key == "share_lessons" =>
                    {
                        Some(format!("hippo config {op} {key}"))
                    }
                    _ => None,
                };
                if let Some(what) = what {
                    found.push(what);
                }
                continue;
            }
            let mut targets: Vec<String> = c.written.clone();
            let plain: Vec<&str> = args
                .iter()
                .copied()
                .filter(|a| !a.starts_with('-'))
                .collect();
            match program.as_str() {
                "rm" | "unlink" | "truncate" | "mv" | "tee" | "touch" | "mkdir" | "chmod" => {
                    targets.extend(plain.iter().map(|a| (*a).to_owned()))
                }
                "cp" | "install" | "ln" | "rsync" => {
                    targets.extend(plain.last().map(|a| (*a).to_owned()))
                }
                "sed" | "perl" if args.iter().any(|a| a.starts_with("-i")) => {
                    targets.extend(plain.iter().map(|a| (*a).to_owned()))
                }
                _ => {}
            }
            for t in targets {
                if let Some(place) =
                    protected(&compiled.home, &resolve(&dir, &t, Some(&compiled.home)))
                {
                    found.push(format!("a write to {place}"));
                    break;
                }
            }
        }
    }
    found.dedup();
    found
        .into_iter()
        .map(|what| Hit {
            rule: "self-protection".into(),
            kind: None,
            action: Action::Block,
            reason: format!("only the person may do this ({what}); an agent never needs to accept or loosen hippo's rules for them."),
            self_protection: true,
        })
        .collect()
}

/// Logs a call a rule matched: `$HIPPO_HOME/guard/log.jsonl`, the
/// command after redaction.
pub fn log(home: &Path, agent: &str, call: &Call, decision: &Decision) {
    let Some(action) = decision.action else {
        return;
    };
    let input = serde_json::to_string(&call.input).unwrap_or_default();
    let text = kinds::shell_script(&call.tool, &input).unwrap_or(input);
    let redacted = crate::redact::redact(&text);
    let mut shown: String = redacted.as_str().chars().take(400).collect();
    if shown.len() < redacted.as_str().len() {
        shown.push('…');
    }
    let line = json!({
        "at": super::now_rfc3339(),
        "agent": agent,
        "session": call.session,
        "tool": call.tool,
        "rules": decision.hits.iter().map(|h| h.rule.clone()).collect::<Vec<_>>(),
        "configured": decision.configured,
        "decision": action,
        "command": shown,
    });
    let _ = super::append(&super::log_path(home), format!("{line}\n").as_bytes());
}

/// What `hippo guard test` counts for one rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TestCount {
    pub rule: String,
    /// Calls it would warn on, ask about and block once `ask` and `block`
    /// act (in this release every one of them warns).
    pub warn: usize,
    pub ask: usize,
    pub block: usize,
    /// Up to three of the calls, redacted and cut short.
    pub examples: Vec<String>,
}

/// Runs the compiled rules over the indexed calls of the last `days` days,
/// without acting: per rule, how many calls it would have flagged. Every
/// layer applies, since the index does not keep a session's folder.
/// Returns the counts and how many calls were read.
pub fn test_history(
    home: &Path,
    compiled: &Compiled,
    days: i64,
) -> crate::error::Result<(Vec<TestCount>, usize)> {
    let index = crate::index::Index::open(home, false)?;
    let since = time::OffsetDateTime::now_utc() - time::Duration::days(days);
    let since_text = since
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let mut counts: Vec<TestCount> = Vec::new();
    let mut calls = 0;
    let everywhere = compiled.workspace.clone();
    for session in index.sessions_active_since(since)? {
        for turn in index.turns_for_session(&session)? {
            if turn.ts.as_str() < since_text.as_str() || !crate::rollup::is_call(&turn) {
                continue;
            }
            let Some((name, raw)) = crate::dream::observed::call(&turn) else {
                continue;
            };
            calls += 1;
            let call = Call {
                tool: name.to_owned(),
                input: serde_json::from_str(raw).unwrap_or(Value::String(raw.to_owned())),
                cwd: everywhere.clone(),
                session: Some(session.clone()),
                headless: false,
            };
            for hit in evaluate(compiled, &call) {
                let i = match counts.iter().position(|c| c.rule == hit.rule) {
                    Some(i) => i,
                    None => {
                        counts.push(TestCount {
                            rule: hit.rule.clone(),
                            ..TestCount::default()
                        });
                        counts.len() - 1
                    }
                };
                let c = &mut counts[i];
                match hit.action {
                    Action::Warn => c.warn += 1,
                    Action::Ask => c.ask += 1,
                    Action::Block => c.block += 1,
                }
                if c.examples.len() < 3 {
                    let text = kinds::shell_script(name, raw).unwrap_or_else(|| raw.to_owned());
                    c.examples.push(one_line(&text));
                }
            }
        }
    }
    counts.sort_by(|a, b| {
        (b.warn + b.ask + b.block)
            .cmp(&(a.warn + a.ask + a.block))
            .then(a.rule.cmp(&b.rule))
    });
    Ok((counts, calls))
}

//! Runners: which CLI, model, effort and account hippo's own model calls
//! use (`docs/designs/runners.md`).
//!
//! A runner is named in `$HIPPO_HOME/config.toml` under `[runners.<name>]`
//! and used per role through `[roles]`: `default`, and optionally
//! `propose`, `review`, `lessons` and `ask`, which fall back to `default`.
//! `[fallback]` lists the runners a role moves to when its runner's account
//! is at its limit; a fallback never leaves the runner's `data_domain`
//! unless `[fallback] cross_domain` names the role.
//!
//! Two presets: `claude`, today's argv plus `--model` and `--effort`, with
//! `account` as `CLAUDE_CONFIG_DIR`; and `command`, any argv that meets the
//! contract (prompt on stdin; the system prompt as an argument, a file or
//! inline between markers; the reply as one JSON object's `result`, the
//! whole stdout, or a file).
//!
//! Every runner runs with a limited environment ([`environment`]) and
//! inside an OS sandbox ([`sandbox`]), except the two implicit runners an
//! older configuration's `model_command` and `reviewer_command_override`
//! become, which keep today's behaviour (the whole environment, no
//! sandbox) so an existing setup keeps working; `hippo runners migrate`
//! turns them into real runners.
//!
//! The tables are read lazily: a mistake in them never stops
//! `config.toml` from loading, so hooks, `context` and `brief` keep
//! working; only the commands that call a model fail, naming it.

pub mod edit;
pub mod sandbox;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::{default_model_command, expand_tilde, Config};
use crate::error::{Error, Result};

/// The roles a runner can serve, in the order `hippo runners` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Each dreaming pass's proposing run.
    Propose,
    /// The dreaming reviewer, the lessons reviewer, the pack review.
    Review,
    /// The lessons drafter.
    Lessons,
    /// `hippo ask` without `--sources-only`.
    Ask,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Propose, Role::Review, Role::Lessons, Role::Ask];

    pub fn key(self) -> &'static str {
        match self {
            Role::Propose => "propose",
            Role::Review => "review",
            Role::Lessons => "lessons",
            Role::Ask => "ask",
        }
    }

    /// Whether what the role writes is shared (a proposal, a receipt, a
    /// lesson): such a role reads transcript text and may not run a model
    /// with tools unless the runner allows it.
    pub fn writes(self) -> bool {
        !matches!(self, Role::Ask)
    }

    pub fn parse(text: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.key() == text)
    }
}

/// A runner's preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Claude,
    Command,
}

/// How a `command` runner's reply is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReplyMode {
    /// One JSON object with a string `result`, as `claude -p` prints.
    JsonResult,
    /// The whole stdout, trimmed.
    Stdout,
    /// The file at `{reply_file}`.
    File,
}

/// How the system prompt reaches the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemPrompt {
    /// As an argument, `{system_prompt}`.
    Flag,
    /// In a file in the run folder, `{system_prompt_file}`.
    File,
    /// At the top of the user prompt, between `<<<SYSTEM>>>` and
    /// `<<<END SYSTEM>>>`.
    Inline,
}

/// The argv placeholders of a `command` runner.
pub const SYSTEM_PROMPT: &str = "{system_prompt}";
pub const SYSTEM_PROMPT_FILE: &str = "{system_prompt_file}";
pub const REPLY_FILE: &str = "{reply_file}";
pub const RUN_DIR: &str = "{run_dir}";

/// The efforts `claude --effort` takes.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The longest a runner may run.
pub const MAX_TIMEOUT_SECS: u64 = 1800;

/// One `[runners.<name>]` table, as written.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRunner {
    preset: String,
    model: Option<String>,
    effort: Option<String>,
    account: Option<String>,
    data_domain: Option<String>,
    timeout_secs: Option<u64>,
    argv: Option<Vec<String>>,
    reply: Option<String>,
    #[serde(default)]
    env_pass: Vec<String>,
    #[serde(default)]
    allow_tools: bool,
    #[serde(default)]
    allow_unsandboxed: bool,
    #[serde(default)]
    limit_exit_codes: Vec<i32>,
}

/// A runner, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runner {
    pub name: String,
    pub preset: Preset,
    /// `claude`: `--model`; `None` is the CLI's default.
    pub model: Option<String>,
    /// `claude`: `--effort`.
    pub effort: Option<String>,
    /// `claude`: the account folder, passed as `CLAUDE_CONFIG_DIR`.
    pub account: Option<PathBuf>,
    /// Where this runner's prompts may go; fallback stays inside it.
    pub data_domain: String,
    pub timeout: Duration,
    /// `command`: the argv with its placeholders.
    pub argv: Vec<String>,
    pub reply: ReplyMode,
    /// Environment variable names passed on beyond the base list.
    pub env_pass: Vec<String>,
    pub allow_tools: bool,
    pub allow_unsandboxed: bool,
    /// `command`: exit codes that mean the account is at its limit.
    pub limit_exit_codes: Vec<i32>,
    /// One of the implicit runners an older `model_command` or
    /// `reviewer_command_override` becomes: it inherits the whole
    /// environment and runs without the sandbox, as before 0.5.0.
    pub legacy: bool,
}

impl Runner {
    /// The `claude` preset with nothing set: today's default call.
    pub fn default_claude() -> Runner {
        Runner {
            name: "claude".into(),
            preset: Preset::Claude,
            model: None,
            effort: None,
            account: None,
            data_domain: "anthropic".into(),
            timeout: crate::model::TIMEOUT,
            argv: Vec::new(),
            reply: ReplyMode::JsonResult,
            env_pass: Vec::new(),
            allow_tools: false,
            allow_unsandboxed: false,
            limit_exit_codes: Vec::new(),
            legacy: false,
        }
    }

    /// The argv this runner starts, placeholders not yet filled: for
    /// `claude`, [`default_model_command`] then `--model` and `--effort`
    /// when set; for `command`, its `argv`.
    pub fn argv(&self) -> Vec<String> {
        match self.preset {
            Preset::Claude => {
                let mut argv = default_model_command();
                if let Some(model) = &self.model {
                    argv.extend(["--model".to_owned(), model.clone()]);
                }
                if let Some(effort) = &self.effort {
                    argv.extend(["--effort".to_owned(), effort.clone()]);
                }
                argv
            }
            Preset::Command => self.argv.clone(),
        }
    }

    /// How the system prompt reaches this runner's CLI.
    pub fn system_prompt(&self) -> SystemPrompt {
        let argv = self.argv();
        if argv.iter().any(|a| a == SYSTEM_PROMPT) {
            SystemPrompt::Flag
        } else if argv.iter().any(|a| a.contains(SYSTEM_PROMPT_FILE)) {
            SystemPrompt::File
        } else {
            SystemPrompt::Inline
        }
    }

    /// Whether the model can use tools: `claude` runs with `--tools ""`;
    /// a `command` runner is assumed to unless a canary run showed it
    /// cannot read a planted file ([`canary_passed`]).
    pub fn may_have_tools(&self, home: &Path) -> bool {
        match self.preset {
            Preset::Claude => false,
            Preset::Command => !canary_passed(home, &self.name),
        }
    }

    /// The account folder Fleet reads this runner's meter from: `account`,
    /// else `$CLAUDE_CONFIG_DIR` for an implicit runner, else `~/.claude`.
    pub fn account_dir(&self) -> Option<PathBuf> {
        if self.preset != Preset::Claude {
            return None;
        }
        self.account
            .clone()
            .or_else(|| crate::config::user_home().map(|home| home.join(".claude")))
    }
}

/// `$HIPPO_HOME/runners/<name>.canary`: written by `hippo doctor --model
/// --canary` when the runner could not read either planted file.
pub fn canary_file(home: &Path, name: &str) -> PathBuf {
    home.join("runners").join(format!("{name}.canary"))
}

/// Whether the last canary run of runner `name` passed.
pub fn canary_passed(home: &Path, name: &str) -> bool {
    std::fs::read_to_string(canary_file(home, name)).is_ok_and(|t| t.trim() == "passed")
}

/// Every runner and role in a configuration, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub runners: BTreeMap<String, Runner>,
    /// `default` and the roles set, each to a runner's name.
    pub roles: BTreeMap<String, String>,
    /// Each role's fallback runners, in order.
    pub fallback: BTreeMap<String, Vec<String>>,
    /// Roles whose fallback may leave the runner's data domain.
    pub cross_domain: Vec<String>,
    /// Whether the plan comes from `[runners]` (or from the older keys and
    /// the default).
    pub configured: bool,
}

/// Whether the configuration sets the older keys: a `model_command` other
/// than the default, or a `reviewer_command_override`.
pub fn has_legacy_keys(cfg: &Config) -> bool {
    cfg.model_command != default_model_command() || cfg.reviewer_command_override.is_some()
}

/// What `[runners]` holds, or nothing.
fn has_runners(cfg: &Config) -> bool {
    !cfg.runners.is_empty() || !cfg.roles.is_empty() || !cfg.fallback.is_empty()
}

/// The configuration's runners and roles. Fails when the tables are
/// wrong, or when they sit beside `model_command` or
/// `reviewer_command_override` (`hippo runners migrate` moves those).
pub fn plan(cfg: &Config) -> Result<Plan> {
    if !has_runners(cfg) {
        return Ok(implicit(cfg));
    }
    if has_legacy_keys(cfg) {
        return Err(Error::Runners(format!(
            "{} has both [runners] and the older {}; run hippo runners migrate to move them into [runners]",
            cfg.home.join("config.toml").display(),
            legacy_names(cfg).join(" and ")
        )));
    }
    let mut runners = BTreeMap::new();
    for (name, value) in &cfg.runners {
        let raw: RawRunner = value
            .clone()
            .try_into()
            .map_err(|e| Error::Runners(format!("[runners.{name}]: {e}")))?;
        runners.insert(name.clone(), resolve(name, raw)?);
    }
    let mut roles = BTreeMap::new();
    for (key, value) in &cfg.roles {
        if key != "default" && Role::parse(key).is_none() {
            return Err(Error::Runners(format!(
                "[roles] {key}: not a role (default, propose, review, lessons, ask)"
            )));
        }
        let Some(name) = value.as_str() else {
            return Err(Error::Runners(format!(
                "[roles] {key}: must be a runner's name"
            )));
        };
        if !runners.contains_key(name) {
            return Err(Error::Runners(format!(
                "[roles] {key} = {name:?}: no [runners.{name}]"
            )));
        }
        roles.insert(key.clone(), name.to_owned());
    }
    if !roles.contains_key("default") {
        if runners.len() == 1 {
            let only = runners.keys().next().cloned().unwrap_or_default();
            roles.insert("default".into(), only);
        } else {
            return Err(Error::Runners(
                "[roles] needs default = \"<runner>\" once there is more than one runner".into(),
            ));
        }
    }
    let mut fallback = BTreeMap::new();
    let mut cross_domain = Vec::new();
    for (key, value) in &cfg.fallback {
        let list: Vec<String> = value
            .clone()
            .try_into()
            .map_err(|e| Error::Runners(format!("[fallback] {key}: {e}")))?;
        if key == "cross_domain" {
            for role in &list {
                if role != "default" && Role::parse(role).is_none() {
                    return Err(Error::Runners(format!(
                        "[fallback] cross_domain: {role} is not a role"
                    )));
                }
            }
            cross_domain = list;
            continue;
        }
        if key != "default" && Role::parse(key).is_none() {
            return Err(Error::Runners(format!("[fallback] {key}: not a role")));
        }
        for name in &list {
            if !runners.contains_key(name) {
                return Err(Error::Runners(format!(
                    "[fallback] {key}: no [runners.{name}]"
                )));
            }
        }
        fallback.insert(key.clone(), list);
    }
    Ok(Plan {
        runners,
        roles,
        fallback,
        cross_domain,
        configured: true,
    })
}

fn legacy_names(cfg: &Config) -> Vec<&'static str> {
    let mut out = Vec::new();
    if cfg.model_command != default_model_command() {
        out.push("model_command");
    }
    if cfg.reviewer_command_override.is_some() {
        out.push("reviewer_command_override");
    }
    out
}

/// The plan without `[runners]`: the `claude` preset with today's argv,
/// or the implicit runners the older keys become.
fn implicit(cfg: &Config) -> Plan {
    let mut runners = BTreeMap::new();
    let mut roles = BTreeMap::new();
    if cfg.model_command == default_model_command() {
        runners.insert("claude".into(), Runner::default_claude());
        roles.insert("default".into(), "claude".into());
    } else {
        runners.insert("custom".into(), legacy_runner("custom", &cfg.model_command));
        roles.insert("default".into(), "custom".into());
    }
    if let Some(review) = &cfg.reviewer_command_override {
        runners.insert(
            "custom-review".into(),
            legacy_runner("custom-review", review),
        );
        roles.insert("review".into(), "custom-review".into());
    }
    Plan {
        runners,
        roles,
        fallback: BTreeMap::new(),
        cross_domain: Vec::new(),
        configured: false,
    }
}

fn legacy_runner(name: &str, argv: &[String]) -> Runner {
    Runner {
        name: name.into(),
        preset: Preset::Command,
        data_domain: "custom".into(),
        argv: argv.to_vec(),
        legacy: true,
        ..Runner::default_claude()
    }
}

fn resolve(name: &str, raw: RawRunner) -> Result<Runner> {
    let bad = |what: String| Error::Runners(format!("[runners.{name}] {what}"));
    if !valid_name(name) {
        return Err(bad(
            "the name may hold only letters, digits, - and _ (at most 40)".into(),
        ));
    }
    let preset = match raw.preset.as_str() {
        "claude" => Preset::Claude,
        "command" => Preset::Command,
        other => return Err(bad(format!("preset {other:?}: claude or command"))),
    };
    if let Some(effort) = &raw.effort {
        if !EFFORTS.contains(&effort.as_str()) {
            return Err(bad(format!(
                "effort {effort:?}: one of {}",
                EFFORTS.join(", ")
            )));
        }
    }
    let timeout_secs = raw.timeout_secs.unwrap_or(crate::model::TIMEOUT.as_secs());
    if timeout_secs == 0 || timeout_secs > MAX_TIMEOUT_SECS {
        return Err(bad(format!(
            "timeout_secs {timeout_secs}: from 1 to {MAX_TIMEOUT_SECS}"
        )));
    }
    for var in &raw.env_pass {
        if var.is_empty() || !var.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(bad(format!("env_pass {var:?}: a variable name")));
        }
    }
    let reply = match raw.reply.as_deref() {
        None | Some("json-result") => ReplyMode::JsonResult,
        Some("stdout") => ReplyMode::Stdout,
        Some("file") => ReplyMode::File,
        Some(other) => return Err(bad(format!("reply {other:?}: json-result, stdout or file"))),
    };
    let runner = match preset {
        Preset::Claude => {
            for (key, set) in [
                ("argv", raw.argv.is_some()),
                ("reply", raw.reply.is_some()),
                ("limit_exit_codes", !raw.limit_exit_codes.is_empty()),
                ("allow_tools", raw.allow_tools),
            ] {
                if set {
                    return Err(bad(format!("{key} is for preset = \"command\"")));
                }
            }
            Runner {
                name: name.into(),
                preset,
                model: raw.model,
                effort: raw.effort,
                account: raw.account.map(|a| PathBuf::from(expand_tilde(&a))),
                data_domain: raw.data_domain.unwrap_or_else(|| "anthropic".into()),
                timeout: Duration::from_secs(timeout_secs),
                argv: Vec::new(),
                reply: ReplyMode::JsonResult,
                env_pass: raw.env_pass,
                allow_tools: false,
                allow_unsandboxed: raw.allow_unsandboxed,
                limit_exit_codes: Vec::new(),
                legacy: false,
            }
        }
        Preset::Command => {
            for (key, set) in [
                ("model", raw.model.is_some()),
                ("effort", raw.effort.is_some()),
                ("account", raw.account.is_some()),
            ] {
                if set {
                    return Err(bad(format!(
                        "{key} is for preset = \"claude\"; put it in argv"
                    )));
                }
            }
            let argv = raw.argv.unwrap_or_default();
            if argv.is_empty() || argv[0].is_empty() {
                return Err(bad("argv: a command is required".into()));
            }
            if reply == ReplyMode::File && !argv.iter().any(|a| a.contains(REPLY_FILE)) {
                return Err(bad(format!("reply = \"file\" needs {REPLY_FILE} in argv")));
            }
            let Some(data_domain) = raw.data_domain else {
                return Err(bad(
                    "data_domain is required: a label for where its prompts go".into(),
                ));
            };
            Runner {
                name: name.into(),
                preset,
                model: None,
                effort: None,
                account: None,
                data_domain,
                timeout: Duration::from_secs(timeout_secs),
                argv,
                reply,
                env_pass: raw.env_pass,
                allow_tools: raw.allow_tools,
                allow_unsandboxed: raw.allow_unsandboxed,
                limit_exit_codes: raw.limit_exit_codes,
                legacy: false,
            }
        }
    };
    if runner
        .data_domain
        .bytes()
        .any(|b| !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        || runner.data_domain.is_empty()
    {
        return Err(bad(format!(
            "data_domain {:?}: letters, digits, - and _",
            runner.data_domain
        )));
    }
    Ok(runner)
}

/// Whether `name` can name a runner.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One step of a role's chain: a runner, or the reason it may not serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step<'a> {
    pub runner: &'a Runner,
    /// Why this fallback may not be used; `None` when it may.
    pub refused: Option<String>,
}

impl Plan {
    /// The runner name `role` resolves to.
    pub fn runner_name(&self, role: Role) -> &str {
        self.roles
            .get(role.key())
            .or_else(|| self.roles.get("default"))
            .map(String::as_str)
            .unwrap_or("claude")
    }

    /// The runner `role` resolves to.
    pub fn runner(&self, role: Role) -> Option<&Runner> {
        self.runners.get(self.runner_name(role))
    }

    /// `role`'s runner, then each of its fallbacks (the role's own list, or
    /// `default`'s), each with the reason it may not serve: a runner in
    /// another data domain, unless `cross_domain` names the role.
    pub fn chain(&self, role: Role) -> Vec<Step<'_>> {
        let Some(first) = self.runner(role) else {
            return Vec::new();
        };
        let mut out = vec![Step {
            runner: first,
            refused: None,
        }];
        let list = self
            .fallback
            .get(role.key())
            .or_else(|| self.fallback.get("default"));
        let crossing = self.cross_domain.iter().any(|r| {
            r == role.key() || (r == "default" && !self.fallback.contains_key(role.key()))
        });
        for name in list.into_iter().flatten() {
            let Some(runner) = self.runners.get(name) else {
                continue;
            };
            if runner.name == first.name || out.iter().any(|s| s.runner.name == runner.name) {
                continue;
            }
            let refused = (runner.data_domain != first.data_domain && !crossing).then(|| {
                format!(
                    "refused: domain {} ≠ {}",
                    runner.data_domain, first.data_domain
                )
            });
            out.push(Step { runner, refused });
        }
        out
    }

    /// Why `runner` may not serve `role`, if it may not: the review role
    /// needs a real system prompt; a role that writes needs a runner
    /// without tools (or `allow_tools`) and a sandbox (or
    /// `allow_unsandboxed`). The implicit runners of the older keys are
    /// exempt, as before 0.5.0.
    pub fn refusal(&self, home: &Path, runner: &Runner, role: Role) -> Option<String> {
        if runner.legacy {
            return None;
        }
        if role == Role::Review && runner.system_prompt() == SystemPrompt::Inline {
            return Some(format!(
                "runner {} cannot be the reviewer: its argv has neither {SYSTEM_PROMPT} nor {SYSTEM_PROMPT_FILE}, and a reviewer's persona must be a real system prompt",
                runner.name
            ));
        }
        if role.writes() && runner.may_have_tools(home) && !runner.allow_tools {
            return Some(format!(
                "runner {} may give the model tools, and the {} role reads transcript text: run hippo doctor --model --canary --runner {} to check it cannot read files, or set allow_tools = true",
                runner.name,
                role.key(),
                runner.name
            ));
        }
        if role.writes()
            && runner.preset == Preset::Command
            && !sandbox::available()
            && !runner.allow_unsandboxed
        {
            return Some(format!(
                "runner {} would run without a sandbox ({}), and the {} role reads transcript text: set allow_unsandboxed = true to run it anyway",
                runner.name,
                sandbox::kind_name(),
                role.key()
            ));
        }
        None
    }
}

/// What a receipt or a lesson review records about the runner that
/// answered: never the account folder, which can name its owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerRecord {
    pub name: String,
    pub preset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    /// The model id, or `(CLI default)`.
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub data_domain: String,
    /// `on` or `off`.
    pub sandbox: String,
    /// `none` or `unknown`.
    pub tools: String,
    /// `flag`, `file` or `inline`.
    pub system_prompt: String,
    /// The runner this call fell back from, and why.
    #[serde(default)]
    pub fallback_from: Option<String>,
}

impl RunnerRecord {
    pub fn of(home: &Path, runner: &Runner, sandboxed: bool) -> RunnerRecord {
        RunnerRecord {
            name: runner.name.clone(),
            preset: match runner.preset {
                Preset::Claude => "claude",
                Preset::Command => "command",
            }
            .into(),
            cli_version: None,
            model: runner
                .model
                .clone()
                .unwrap_or_else(|| "(CLI default)".into()),
            effort: runner.effort.clone(),
            data_domain: runner.data_domain.clone(),
            sandbox: if sandboxed { "on" } else { "off" }.into(),
            tools: if runner.may_have_tools(home) {
                "unknown"
            } else {
                "none"
            }
            .into(),
            system_prompt: match runner.system_prompt() {
                SystemPrompt::Flag => "flag",
                SystemPrompt::File => "file",
                SystemPrompt::Inline => "inline",
            }
            .into(),
            fallback_from: None,
        }
    }
}

/// The environment a runner's CLI gets (`docs/designs/runners.md`,
/// Environment): the base list, `TMPDIR` set to the run folder, the
/// preset's login variables, and `env_pass`. `vars` is the environment
/// hippo was started with. The implicit runners of the older keys are not
/// limited; the caller does not call this for them.
pub fn environment(
    runner: &Runner,
    vars: &[(String, String)],
    run_dir: &Path,
) -> Vec<(String, String)> {
    const BASE: [&str; 15] = [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "TERM",
        "TZ",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
        "REQUESTS_CA_BUNDLE",
    ];
    const CLAUDE: [&str; 9] = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "CLOUD_ML_REGION",
        "ANTHROPIC_VERTEX_PROJECT_ID",
    ];
    let claude = runner.preset == Preset::Claude;
    let keep = |key: &str| {
        BASE.iter().any(|k| k.eq_ignore_ascii_case(key))
            || key.starts_with("LC_")
            || key.starts_with("XDG_")
            || (claude
                && (CLAUDE.contains(&key) || key.starts_with("AWS_") || key == "CLAUDE_CONFIG_DIR"))
            || runner.env_pass.iter().any(|k| k == key)
    };
    let mut out: Vec<(String, String)> = vars
        .iter()
        .filter(|(k, _)| keep(k) && k != "TMPDIR")
        .cloned()
        .collect();
    out.push(("TMPDIR".into(), run_dir.display().to_string()));
    if let Some(account) = runner.account.as_ref().filter(|_| claude) {
        out.retain(|(k, _)| k != "CLAUDE_CONFIG_DIR");
        out.push(("CLAUDE_CONFIG_DIR".into(), account.display().to_string()));
    }
    out
}

/// The text `hippo runners` prints: each runner, each role with its
/// fallbacks and their domains, and the CLIs found on `PATH`.
pub fn describe(cfg: &Config, plan: &Plan) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<14} {:<8} {:<20} {:<7} {:<22} {:<10} {:<8} status",
        "runner", "preset", "model", "effort", "account", "domain", "sandbox"
    );
    for runner in plan.runners.values() {
        let account = match (&runner.account, runner.preset) {
            (Some(a), _) => shorten_home(a),
            (None, Preset::Claude) => "(CLAUDE_CONFIG_DIR or ~/.claude)".into(),
            (None, Preset::Command) => "-".into(),
        };
        let sandbox = if runner.legacy {
            "off (older key)".to_owned()
        } else if sandbox::available() {
            "on".to_owned()
        } else {
            format!("off ({})", sandbox::kind_name())
        };
        let status = if runner.legacy {
            "older key: hippo runners migrate".to_owned()
        } else {
            "ok".to_owned()
        };
        let _ = writeln!(
            out,
            "{:<14} {:<8} {:<20} {:<7} {:<22} {:<10} {:<8} {status}",
            runner.name,
            match runner.preset {
                Preset::Claude => "claude",
                Preset::Command => "command",
            },
            runner.model.as_deref().unwrap_or(match runner.preset {
                Preset::Claude => "(CLI default)",
                Preset::Command => "(in argv)",
            }),
            runner.effort.as_deref().unwrap_or("-"),
            account,
            runner.data_domain,
            sandbox
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{:<8} {:<14} fallback", "role", "runner");
    let mut roles = vec!["default".to_owned()];
    roles.extend(Role::ALL.iter().map(|r| r.key().to_owned()));
    for key in roles {
        let role = Role::parse(&key);
        let name = match plan.roles.get(&key) {
            Some(n) => n.clone(),
            None if key == "default" => plan.runner_name(Role::Propose).to_owned(),
            None => "(default)".into(),
        };
        let chain = role
            .map(|r| plan.chain(r))
            .unwrap_or_default()
            .into_iter()
            .skip(1)
            .map(|s| match s.refused {
                Some(why) => format!("{} ({why})", s.runner.name),
                None => s.runner.name.clone(),
            })
            .collect::<Vec<_>>();
        let mut refusals = Vec::new();
        if let (Some(role), Some(runner)) = (role, role.and_then(|r| plan.runner(r))) {
            if let Some(why) = plan.refusal(&cfg.home, runner, role) {
                refusals.push(why);
            }
        }
        let fallback = if key == "default" {
            plan.fallback
                .get("default")
                .map(|l| l.join(", "))
                .unwrap_or_else(|| "-".into())
        } else if chain.is_empty() {
            "-".into()
        } else {
            chain.join(", ")
        };
        let _ = writeln!(out, "{key:<8} {name:<14} {fallback}");
        for why in refusals {
            let _ = writeln!(out, "         {why}");
        }
    }
    out
}

fn shorten_home(path: &Path) -> String {
    if let Some(home) = crate::config::user_home() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), text).unwrap();
        let mut c = Config::load_in(dir.path()).unwrap();
        c.home = PathBuf::from("/nonexistent-hippo-home");
        c
    }

    #[test]
    fn nothing_configured_is_todays_argv_byte_for_byte() {
        let plan = plan(&cfg("")).unwrap();
        let runner = plan.runner(Role::Propose).unwrap();
        assert_eq!(runner.argv(), default_model_command());
        assert_eq!(
            runner.argv().join("\u{0}"),
            "claude\u{0}-p\u{0}--no-session-persistence\u{0}--setting-sources\u{0}\u{0}--tools\u{0}\u{0}--strict-mcp-config\u{0}--system-prompt\u{0}{system_prompt}\u{0}--output-format\u{0}json"
        );
        assert!(!runner.legacy);
        assert_eq!(runner.data_domain, "anthropic");
        for role in Role::ALL {
            assert_eq!(plan.runner_name(role), "claude");
        }
        assert!(!plan.configured);
    }

    #[test]
    fn the_older_keys_become_implicit_runners_that_keep_working() {
        let plan = plan(&cfg(
            "model_command = [\"my-model\", \"{system_prompt}\"]\nreviewer_command_override = [\"other\", \"{system_prompt}\"]\n",
        ))
        .unwrap();
        assert_eq!(plan.runner_name(Role::Propose), "custom");
        assert_eq!(plan.runner_name(Role::Review), "custom-review");
        let custom = plan.runner(Role::Propose).unwrap();
        assert!(custom.legacy);
        assert_eq!(custom.argv(), ["my-model", "{system_prompt}"]);
        assert_eq!(plan.refusal(Path::new("/h"), custom, Role::Propose), None);
    }

    #[test]
    fn old_and_new_keys_together_load_but_refuse_model_calls() {
        let c = cfg(
            "model_command = [\"my-model\", \"{system_prompt}\"]\n[runners.work]\npreset = \"claude\"\n",
        );
        let err = plan(&c).unwrap_err().to_string();
        assert!(err.contains("hippo runners migrate"), "{err}");
        assert!(err.contains("model_command"), "{err}");
    }

    #[test]
    fn a_claude_runner_adds_model_effort_and_account() {
        let plan = plan(&cfg(
            "[runners.work]\npreset = \"claude\"\nmodel = \"claude-opus-5-5\"\neffort = \"high\"\naccount = \"/acct/work\"\ndata_domain = \"acme\"\n",
        ))
        .unwrap();
        let work = plan.runner(Role::Review).unwrap();
        let mut want = default_model_command();
        want.extend(["--model", "claude-opus-5-5", "--effort", "high"].map(String::from));
        assert_eq!(work.argv(), want);
        assert_eq!(work.account.as_deref(), Some(Path::new("/acct/work")));
        let env = environment(
            work,
            &[
                ("PATH".into(), "/bin".into()),
                ("ANTHROPIC_API_KEY".into(), "k".into()),
                ("AWS_PROFILE".into(), "p".into()),
                ("OPENAI_API_KEY".into(), "o".into()),
                ("STRIPE_SECRET_KEY".into(), "s".into()),
                ("CLAUDE_CONFIG_DIR".into(), "/other".into()),
                ("LC_ALL".into(), "C".into()),
                ("TMPDIR".into(), "/var/tmp".into()),
            ],
            Path::new("/run"),
        );
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "PATH",
                "ANTHROPIC_API_KEY",
                "AWS_PROFILE",
                "LC_ALL",
                "TMPDIR",
                "CLAUDE_CONFIG_DIR"
            ]
        );
        assert!(env.contains(&("TMPDIR".into(), "/run".into())));
        assert!(env.contains(&("CLAUDE_CONFIG_DIR".into(), "/acct/work".into())));
    }

    #[test]
    fn a_command_runner_gets_the_base_list_and_its_env_pass_only() {
        let plan = plan(&cfg(
            "[runners.local]\npreset = \"command\"\nargv = [\"m\", \"{system_prompt}\"]\ndata_domain = \"local\"\nenv_pass = [\"MY_KEY\"]\n",
        ))
        .unwrap();
        let local = plan.runner(Role::Ask).unwrap();
        let env = environment(
            local,
            &[
                ("HOME".into(), "/h".into()),
                ("ANTHROPIC_API_KEY".into(), "k".into()),
                ("MY_KEY".into(), "v".into()),
                ("XDG_DATA_HOME".into(), "/x".into()),
            ],
            Path::new("/run"),
        );
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["HOME", "MY_KEY", "XDG_DATA_HOME", "TMPDIR"]);
    }

    #[test]
    fn mistakes_in_the_tables_name_the_key() {
        for (text, want) in [
            ("[runners.a]\npreset = \"codex\"\n", "preset \"codex\""),
            ("[runners.a]\npreset = \"claude\"\neffort = \"huge\"\n", "effort"),
            ("[runners.a]\npreset = \"command\"\nargv = [\"m\"]\n", "data_domain is required"),
            ("[runners.a]\npreset = \"command\"\nargv = [\"m\"]\ndata_domain = \"d\"\nreply = \"file\"\n", "{reply_file}"),
            ("[runners.a]\npreset = \"claude\"\nargv = [\"m\"]\n", "argv is for preset"),
            ("[runners.a]\npreset = \"claude\"\nsurprise = 1\n", "unknown field"),
            ("[runners.a]\npreset = \"claude\"\n[runners.b]\npreset = \"claude\"\n", "needs default"),
            ("[runners.a]\npreset = \"claude\"\n[roles]\nreviewer = \"a\"\n", "not a role"),
            ("[runners.a]\npreset = \"claude\"\n[roles]\nask = \"b\"\n", "no [runners.b]"),
            ("[runners.a]\npreset = \"claude\"\ntimeout_secs = 5000\n", "timeout_secs"),
        ] {
            let err = plan(&cfg(text)).unwrap_err().to_string();
            assert!(err.contains(want), "{text}: {err}");
        }
    }

    #[test]
    fn fallback_stays_inside_the_data_domain_unless_the_role_may_cross() {
        let text = "[runners.work]\npreset = \"claude\"\ndata_domain = \"acme\"\n\
                    [runners.claude]\npreset = \"claude\"\n\
                    [runners.alt]\npreset = \"claude\"\naccount = \"/a\"\n\
                    [roles]\ndefault = \"claude\"\nask = \"work\"\n\
                    [fallback]\ndefault = [\"alt\", \"work\"]\nask = [\"claude\"]\n";
        let plan = plan(&cfg(text)).unwrap();
        let chain = plan.chain(Role::Propose);
        let names: Vec<_> = chain
            .iter()
            .map(|s| (s.runner.name.as_str(), s.refused.is_some()))
            .collect();
        assert_eq!(names, [("claude", false), ("alt", false), ("work", true)]);
        let ask = plan.chain(Role::Ask);
        assert_eq!(
            ask[1].refused.as_deref(),
            Some("refused: domain anthropic ≠ acme")
        );
        let crossing = plan_with(&format!("{text}cross_domain = [\"ask\"]\n"));
        assert_eq!(crossing.chain(Role::Ask)[1].refused, None);
    }

    fn plan_with(text: &str) -> Plan {
        plan(&cfg(text)).unwrap()
    }

    #[test]
    fn the_reviewer_needs_a_real_system_prompt_and_writers_no_tools() {
        let p = plan_with(
            "[runners.inline]\npreset = \"command\"\nargv = [\"m\"]\ndata_domain = \"d\"\nallow_tools = true\nallow_unsandboxed = true\n\
             [runners.tools]\npreset = \"command\"\nargv = [\"m\", \"{system_prompt}\"]\ndata_domain = \"d\"\nallow_unsandboxed = true\n\
             [roles]\ndefault = \"inline\"\n",
        );
        let home = Path::new("/nonexistent");
        let inline = &p.runners["inline"];
        assert!(p
            .refusal(home, inline, Role::Review)
            .unwrap()
            .contains("real system prompt"));
        assert_eq!(p.refusal(home, inline, Role::Propose), None);
        let tools = &p.runners["tools"];
        assert!(p
            .refusal(home, tools, Role::Lessons)
            .unwrap()
            .contains("--canary"));
        assert_eq!(p.refusal(home, tools, Role::Ask), None);
    }
}

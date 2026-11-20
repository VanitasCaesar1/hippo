//! The top bar's badges and the Ask bar's answering runner, in one cheap
//! call the app can repeat.

use hippo_core::runners::{Plan, Preset, Role};
use serde::Serialize;

use super::{agent, Agent};

/// The badges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// Morning: pending rules.
    pub rules_waiting: usize,
    /// Agents: sessions waiting on the person.
    pub sessions_waiting: usize,
    /// Guard: important guard incidents not marked "Not a problem".
    pub guard_important: usize,
    /// `08:41`, when ingest last finished; `None` before the first.
    pub indexed: Option<String>,
    /// Ingest's last run failed: what it said.
    pub index_failed: Option<String>,
    pub ask: Option<Asker>,
}

/// Who answers Ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Asker {
    /// The runner's name in `[runners]`.
    pub runner: String,
    pub agent: Agent,
    /// The account, for a `claude` runner with one.
    pub account: Option<String>,
    /// `Claude Code answers`; a command hippo has no badge for is named by
    /// its program (`my-model answers`).
    pub label: String,
    /// The agent is one hippo has a badge for.
    pub known: bool,
}

/// The runner `ask` resolves to.
pub fn asker(plan: &Plan) -> Option<Asker> {
    let r = plan.runner(Role::Ask)?;
    let a = match r.preset {
        Preset::Claude => agent("claude-code"),
        Preset::Command => {
            let program = r
                .argv
                .first()
                .map(|p| p.rsplit('/').next().unwrap_or(p).to_owned())
                .unwrap_or_else(|| r.name.clone());
            agent(&program)
        }
    };
    let account = r.account.as_ref().and_then(|p| {
        p.file_name().map(|n| {
            let n = n.to_string_lossy();
            n.trim_start_matches(".claude-")
                .trim_start_matches(".claude")
                .trim_start_matches('-')
                .to_owned()
        })
    });
    let account = account.filter(|a| !a.is_empty());
    let known = ["cc", "cx", "cu", "gm", "oc", "ds", "mu", "co"].contains(&a.code.as_str());
    Some(Asker {
        runner: r.name.clone(),
        label: format!("{} answers", a.name),
        known,
        agent: a,
        account,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hippo_core::config::Config;

    #[test]
    fn the_default_runner_is_named() {
        let cfg = Config::defaults_in(std::path::Path::new("/nonexistent/hippo"));
        let plan = hippo_core::runners::plan(&cfg).unwrap();
        let a = asker(&plan).unwrap();
        assert_eq!(a.label, "Claude Code answers");
        assert!(a.known);
    }

    #[test]
    fn a_command_is_named_by_its_program() {
        let cfg = Config {
            model_command: vec!["/opt/models/my-model".into(), "{system_prompt}".into()],
            ..Config::defaults_in(std::path::Path::new("/nonexistent/hippo"))
        };
        let plan = hippo_core::runners::plan(&cfg).unwrap();
        let a = asker(&plan).unwrap();
        assert_eq!(a.label, "my-model answers");
        assert!(!a.known);
    }
}

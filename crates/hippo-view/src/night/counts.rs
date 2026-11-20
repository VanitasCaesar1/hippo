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
    /// `Claude Code answers`.
    pub label: String,
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
    // A command hippo has no badge for is named by what it is.
    let known = ["cc", "cx", "cu", "gm", "oc", "ds", "mu", "co"].contains(&a.code.as_str());
    Some(Asker {
        runner: r.name.clone(),
        label: if known {
            format!("{} answers", a.name)
        } else {
            "Your model command answers".to_owned()
        },
        agent: a,
        account,
    })
}

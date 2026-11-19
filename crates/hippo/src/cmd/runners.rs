//! `hippo runners [--json]`, `runners add|remove|migrate`: the CLIs
//! hippo's own model calls use (`hippo_core::runners`). As with every
//! `hippo config` change, a change is printed and written only with
//! `--yes`.

use std::process::ExitCode;

use anyhow::Result;
use hippo_core::config::{self, Config};
use hippo_core::runners::{self, edit, Role};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Print one JSON object
    #[arg(long)]
    json: bool,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Add a runner: a claude account, model and effort, or any CLI by its argv
    Add {
        name: String,
        /// claude or command
        #[arg(long)]
        preset: String,
        /// claude: the model id, passed as --model
        #[arg(long)]
        model: Option<String>,
        /// claude: low, medium, high, xhigh or max, passed as --effort
        #[arg(long)]
        effort: Option<String>,
        /// claude: the account folder, passed as CLAUDE_CONFIG_DIR
        #[arg(long)]
        account: Option<String>,
        /// Where this runner's prompts may go; fallback stays inside it
        #[arg(long)]
        domain: Option<String>,
        /// command: the argv as a JSON array, with {system_prompt}, {system_prompt_file}, {reply_file}, {run_dir}
        #[arg(long)]
        argv: Option<String>,
        /// command: json-result, stdout or file
        #[arg(long)]
        reply: Option<String>,
        /// command: allow it to serve propose, review and lessons although it may have tools
        #[arg(long)]
        allow_tools: bool,
        /// Write the change; without it, only print it
        #[arg(long)]
        yes: bool,
    },
    /// Remove a runner no role uses
    Remove {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Move model_command and reviewer_command_override into [runners]
    Migrate {
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(args: Args) -> Result<ExitCode> {
    let home = config::home();
    let (change, yes) = match args.command {
        None => return list(args.json),
        Some(Command::Add {
            name,
            preset,
            model,
            effort,
            account,
            domain,
            argv,
            reply,
            allow_tools,
            yes,
        }) => {
            let argv = match argv {
                Some(text) => {
                    Some(serde_json::from_str::<Vec<String>>(&text).map_err(|e| {
                        anyhow::anyhow!("--argv must be a JSON array of strings: {e}")
                    })?)
                }
                None => None,
            };
            (
                edit::Change::Add(edit::NewRunner {
                    name,
                    preset,
                    model,
                    effort,
                    account,
                    domain,
                    argv,
                    reply,
                    allow_tools,
                }),
                yes,
            )
        }
        Some(Command::Remove { name, yes }) => (edit::Change::Remove(name), yes),
        Some(Command::Migrate { yes }) => (edit::Change::Migrate, yes),
    };
    write(&home, &change, yes)
}

/// Prints `change` and writes it with `yes`.
pub fn write(home: &std::path::Path, change: &edit::Change, yes: bool) -> Result<ExitCode> {
    let planned = edit::plan_change(home, change)?;
    print!("{}", planned.diff());
    if !planned.changed() {
        return Ok(ExitCode::SUCCESS);
    }
    if yes {
        planned.apply()?;
        println!("written");
    } else {
        println!("nothing written; add --yes to write it");
    }
    Ok(ExitCode::SUCCESS)
}

fn list(json: bool) -> Result<ExitCode> {
    let cfg = Config::load()?;
    let plan = runners::plan(&cfg)?;
    if json {
        let runners: Vec<_> = plan
            .runners
            .values()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "preset": r.preset,
                    "model": r.model,
                    "effort": r.effort,
                    "data_domain": r.data_domain,
                    "older_key": r.legacy,
                    "sandbox": !r.legacy && runners::sandbox::available(),
                })
            })
            .collect();
        let roles: serde_json::Map<String, serde_json::Value> = std::iter::once((
            "default".to_owned(),
            serde_json::Value::from(plan.runner_name(Role::Propose).to_owned()),
        ))
        .chain(Role::ALL.iter().map(|role| {
            (
                role.key().to_owned(),
                serde_json::json!({
                    "runner": plan.runner_name(*role),
                    "fallback": plan.chain(*role).iter().skip(1).map(|s| serde_json::json!({
                        "runner": s.runner.name,
                        "refused": s.refused,
                    })).collect::<Vec<_>>(),
                    "refused": plan.runner(*role).and_then(|r| plan.refusal(&cfg.home, r, *role)),
                }),
            )
        }))
        .collect();
        println!(
            "{}",
            serde_json::json!({
                "configured": plan.configured,
                "sandbox": runners::sandbox::kind_name(),
                "runners": runners,
                "roles": roles,
            })
        );
        return Ok(ExitCode::SUCCESS);
    }
    print!("{}", runners::describe(&cfg, &plan));
    let found: Vec<String> = ["claude", "codex", "opencode", "muse", "dsh", "cmd"]
        .iter()
        .filter(|p| on_path(p))
        .map(|p| (*p).to_owned())
        .collect();
    println!();
    println!(
        "found on PATH: {}",
        if found.is_empty() {
            "none of claude, codex, opencode, muse, dsh, cmd".to_owned()
        } else {
            found.join(", ")
        }
    );
    println!("sandbox: {}", runners::sandbox::kind_name());
    Ok(ExitCode::SUCCESS)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|dirs| std::env::split_paths(&dirs).any(|dir| dir.join(program).is_file()))
}

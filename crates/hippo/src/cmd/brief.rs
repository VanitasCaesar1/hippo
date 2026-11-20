//! `hippo brief --task <text> [--json] [--off <id>]…`: the start block for a
//! new agent (`hippo_core::brief`). Read-only.

use std::process::ExitCode;

use anyhow::Result;
use hippo_core::brief;

#[derive(clap::Args)]
pub struct Args {
    /// The task the agent will be given
    #[arg(long)]
    task: String,
    /// Leave out the part with this id (repeat); --json lists the ids
    #[arg(long = "off", value_name = "ID")]
    off: Vec<String>,
    /// Print the parts and the block as one JSON object
    #[arg(long)]
    json: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let (config, workspace) = super::load()?;
    let b = brief::run(&config, &workspace, &args.task)?;
    let block = brief::assemble_without(&b, &args.off);
    if args.json {
        let value = serde_json::json!({
            "brief": b,
            "off": args.off,
            "block": block,
            "parts": parts(&b, &args.off, &config),
            "reaches": hippo_view::night::reach::reaches(&workspace.root),
        });
        println!("{}", serde_json::to_string(&value)?);
    } else {
        print!("{block}");
    }
    Ok(ExitCode::SUCCESS)
}

/// The parts by group, as the app's "Your agents now start with" panel
/// lists them, with the bytes of each group and the rules still waiting.
fn parts(
    b: &brief::Brief,
    off: &[String],
    config: &hippo_core::config::Config,
) -> serde_json::Value {
    let on = |p: &&brief::Part| !off.contains(&p.id);
    let group = |g: brief::Group| -> Vec<serde_json::Value> {
        b.parts
            .iter()
            .filter(on)
            .filter(|p| p.group == g)
            .map(|p| {
                serde_json::json!({
                    "id": p.id,
                    "title": p.title,
                    "reason": p.reason,
                    "note": p.note,
                    "bytes": p.bytes,
                })
            })
            .collect()
    };
    let bytes = |g: brief::Group| -> usize {
        b.parts
            .iter()
            .filter(on)
            .filter(|p| p.group == g)
            .map(|p| p.bytes)
            .sum()
    };
    let waiting: Vec<String> = hippo_core::lessons::pending(&config.home)
        .unwrap_or_default()
        .into_iter()
        .map(|l| l.rule)
        .collect();
    serde_json::json!({
        "required": group(brief::Group::Required),
        "picked": group(brief::Group::Relevant),
        "sessions": group(brief::Group::Session),
        "rules": group(brief::Group::Rule),
        "waiting": waiting,
        "bytes": {
            "required": bytes(brief::Group::Required),
            "picked": bytes(brief::Group::Relevant),
            "sessions": bytes(brief::Group::Session),
            "rules": bytes(brief::Group::Rule),
            "total": b.parts.iter().filter(on).map(|p| p.bytes).sum::<usize>(),
        },
        "budget": b.budget,
    })
}

//! `hippocamp brief --task <text> [--json] [--off <id>]…`: the start block for a
//! new agent (`hippocamp_core::brief`). Read-only.

use std::process::ExitCode;

use anyhow::Result;
use hippocamp_core::brief;

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
        let value = serde_json::json!({ "brief": b, "off": args.off, "block": block });
        println!("{}", serde_json::to_string(&value)?);
    } else {
        print!("{block}");
    }
    Ok(ExitCode::SUCCESS)
}

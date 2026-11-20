//! `hippo accept [--list]`: accept, on this machine, the loosenings of
//! security settings that wait (`docs/designs/common.md`, rule 3). It
//! shows each one, then asks. It needs a terminal on stdin and stdout and
//! refuses when an agent's marker is set; there is no `--yes`.

use std::io::{self, BufRead as _, Write as _};
use std::process::ExitCode;

use anyhow::{bail, Result};
use hippo_core::accept;
use hippo_core::config::{self, Config, Workspace};

#[derive(clap::Args)]
pub struct Args {
    /// Only list what waits; change nothing (agents may run this)
    #[arg(long)]
    list: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let home = config::home();
    let cfg = Config::load_in(&home)?;
    let ws = Workspace::load_optional(&cfg)?;
    let pending = accept::pending(&cfg, ws.as_ref())?;
    if pending.is_empty() {
        println!("nothing waits: every security setting in force is the one in the file");
        return Ok(ExitCode::SUCCESS);
    }
    println!("Loosenings waiting on this machine:");
    for waiting in &pending.settings {
        println!("  {waiting}");
    }
    for line in &pending.guard {
        println!("  guard rules: {line}");
    }
    if args.list {
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(why) = accept::person_refusal("hippo accept") {
        bail!(why);
    }
    print!("Accept these on this machine? Until you do, the values in force stay. [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        println!("nothing accepted");
        return Ok(ExitCode::SUCCESS);
    }
    accept::accept_all(&cfg, ws.as_ref())?;
    println!("accepted");
    Ok(ExitCode::SUCCESS)
}

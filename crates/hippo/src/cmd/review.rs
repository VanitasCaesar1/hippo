//! `hippo review <receipt>... --ok`: marks applied dreaming changes as read
//! and right (`hippo_core::dream::seen`). The receipt is not changed; the
//! mark is `.hippo/reviews/<receipt>.json`. To undo a change instead, run
//! `hippo revert <receipt>`.

use std::process::ExitCode;

use anyhow::{bail, Context as _, Result};
use hippo_core::dream::seen;
use time::OffsetDateTime;
use ulid::Ulid;

#[derive(clap::Args)]
pub struct Args {
    /// Confirm receipts of applied changes
    #[arg(required = true)]
    receipts: Vec<String>,
    /// The change looks right (the only mark there is)
    #[arg(long)]
    ok: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    if !args.ok {
        bail!(
            "say what you found: --ok marks the change as right; hippo revert <receipt> undoes it"
        );
    }
    let (_, workspace) = super::load()?;
    let now = OffsetDateTime::now_utc();
    for text in &args.receipts {
        let id: Ulid = text
            .parse()
            .with_context(|| format!("not a receipt id: {text}"))?;
        let path = seen::mark(&workspace, id, now)?;
        println!("{id}: looks right ({})", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

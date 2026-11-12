//! `hippo revert <receipt> [--force]`: restores a note from the snapshot of
//! an applied change and writes a revert receipt. A note edited since the
//! change is refused, exit 1, unless `--force`.

use std::process::ExitCode;

use anyhow::{anyhow, Context as _, Result};
use hippo_core::dream::{self, validate};
use hippo_core::error::Error;
use hippo_core::records::Proposal;
use ulid::Ulid;

#[derive(clap::Args)]
pub struct Args {
    /// The id of the confirm receipt whose change to undo
    receipt: String,
    /// Restore the snapshot even if the note was edited after the change
    #[arg(long)]
    force: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let id: Ulid = args
        .receipt
        .parse()
        .with_context(|| format!("not a receipt id: {}", args.receipt))?;
    let (_config, workspace) = super::load()?;
    let receipt = match dream::revert(&workspace, id, args.force) {
        Ok(receipt) => receipt,
        Err(err @ Error::NoteChanged { .. }) => {
            return Err(anyhow!(
                "the note was edited after the change, so it is left as it is; \
                 --force restores the snapshot anyway ({err})"
            ))
        }
        Err(err) => return Err(err.into()),
    };
    let proposal = Proposal::load(&validate::path(&workspace, receipt.proposal))?;
    println!(
        "restored {} from {}; revert receipt {}",
        proposal.note.display(),
        receipt
            .snapshot
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        receipt.id
    );
    Ok(ExitCode::SUCCESS)
}

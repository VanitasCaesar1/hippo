//! `hippocamp log [--stdout]`: writes the session log to `log_path` and prints
//! the path, or prints the log.

use std::process::ExitCode;

use anyhow::Result;
use hippocamp_core::error::Error;
use hippocamp_core::index::Index;
use hippocamp_core::sessions;

#[derive(clap::Args)]
pub struct Args {
    /// Print the log instead of writing it
    #[arg(long)]
    stdout: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let (config, workspace) = super::load()?;
    let index = match Index::open(&config.home, false) {
        Ok(index) => Some(index),
        Err(Error::MissingIndex) => None,
        Err(err) => return Err(err.into()),
    };
    if args.stdout {
        print!("{}", sessions::render_log(&workspace, index.as_ref())?);
    } else {
        let path = sessions::write_log(&workspace, index.as_ref())?;
        println!("{}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

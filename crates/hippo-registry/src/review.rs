//! `hippo-registry review`: one fresh-context pack review of an entry's
//! pin, saved as `registry/reviews/<id>.json`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context};
use hippo_core::config::default_model_command;
use hippo_core::packs::registry::Entry;
use hippo_core::packs::review::{review, ReviewRequest};
use hippo_core::packs::validate;
use hippo_core::records::Verdict;

use crate::check::{escaped, fetch_pin, load_settings};

#[derive(clap::Args)]
pub struct Args {
    /// The entry's name in registry/packs/
    #[arg(required_unless_present = "entry", conflicts_with = "entry")]
    pub name: Option<String>,
    /// An entry file, in place of a name
    #[arg(long)]
    pub entry: Option<PathBuf>,
    /// The reviewer's command as a JSON array of strings
    #[arg(long)]
    pub model_command: Option<String>,
}

pub fn run(root: &Path, args: &Args) -> anyhow::Result<ExitCode> {
    let settings = load_settings(root)?;
    let path = match (&args.entry, &args.name) {
        (Some(file), _) => file.clone(),
        (None, Some(name)) => root.join("registry/packs").join(format!("{name}.toml")),
        (None, None) => bail!("give an entry name or --entry <file>"),
    };
    let stem = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".toml"))
        .with_context(|| format!("{}: not a <name>.toml file", path.display()))?;
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let entry =
        Entry::parse(stem, &text).map_err(|why| anyhow::anyhow!("{}: {why}", path.display()))?;
    let command: Vec<String> = match &args.model_command {
        Some(json) => {
            serde_json::from_str(json).context("--model-command must be a JSON array of strings")?
        }
        None => default_model_command(),
    };

    let fetched = fetch_pin(root, &settings, &entry, false)
        .map_err(|why| anyhow::anyhow!("{}: {why}", entry.name))?
        .context("the pin was not fetched")?;
    let validation = validate::validate(&fetched.files);
    if !validation.is_ok() {
        for why in &validation.errors {
            println!("error {}: {}", escaped(&entry.name), escaped(why));
        }
        println!(
            "{}: not reviewed, since the pack has errors",
            escaped(&entry.name)
        );
        return Ok(ExitCode::FAILURE);
    }
    let source = if entry.path.is_empty() {
        entry.repo.clone()
    } else {
        format!("{}//{}", entry.repo, entry.path)
    };
    let request = ReviewRequest {
        files: &fetched.files,
        validation: &validation,
        source: &source,
        commit: Some(&entry.commit),
        repo_id: Some(entry.repo_id),
        update: None,
    };
    let home = tempfile::tempdir().context("creating a temporary hippo home")?;
    let record = review(home.path(), &command, &request)?;
    let saved = root
        .join("registry/reviews")
        .join(format!("{}.json", record.id));
    record.save_new(&saved)?;
    let word = match record.verdict {
        Verdict::Confirm => "confirm",
        _ => "reject",
    };
    println!("{}", saved.display());
    println!(
        "{} {word}: {}",
        escaped(&entry.name),
        escaped(&record.notes)
    );
    Ok(if record.verdict == Verdict::Confirm {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

//! `hippo-registry build`: the index of the entries that pass the checks,
//! and the static site built from the same entries.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use hippo_core::packs::registry::{Counts, Index, IndexFile, IndexPack, IndexPass, IndexReview};
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::check::{assess, escaped, Assessment, Listed};
use crate::site;

#[derive(clap::Args)]
pub struct Args {
    /// The directory to write
    #[arg(long)]
    pub out: PathBuf,
    /// Skip the owner check and the entries outside this repository
    #[arg(long)]
    pub offline: bool,
    /// An HTML fragment to wrap in the page frame as the site's index.html
    #[arg(long)]
    pub landing: Option<PathBuf>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn index_pack(assessment: &Assessment, listed: &Listed) -> IndexPack {
    let entry = &listed.entry;
    IndexPack {
        name: entry.name.clone(),
        version: listed.manifest.version.clone(),
        description: listed.manifest.description.clone(),
        authors: listed.manifest.authors.clone(),
        owner: entry.owner().to_owned(),
        maintainers: entry.maintainers.clone(),
        first_party: assessment.first_party(entry),
        license: listed.manifest.license.clone(),
        repo: entry.repo.clone(),
        repo_id: entry.repo_id,
        owner_id: entry.owner_id,
        path: entry.path.clone(),
        commit: entry.commit.clone(),
        pack_hash: listed.pack_hash.clone(),
        kind: listed.manifest.kind.as_str().to_owned(),
        passes: listed
            .validation
            .passes
            .iter()
            .map(|(name, pass)| IndexPass {
                name: name.clone(),
                reads: pass.reads.iter().map(|r| r.as_str().to_owned()).collect(),
                per_note: pass.per_note,
                note_types: pass.note_types.clone(),
                note_paths: pass.note_paths.clone(),
            })
            .collect(),
        theme_contract: listed.manifest.theme_contract,
        desktop_theme_format: listed.manifest.desktop_theme_format,
        files: listed
            .files
            .iter()
            .map(|(path, bytes)| IndexFile {
                path: path.to_owned(),
                bytes: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            })
            .collect(),
        review: IndexReview {
            id: listed.review.id.clone(),
            verdict: listed.review.verdict,
            at: listed.review.at,
        },
    }
}

/// The index of the listed packs: sorted by name, and `generated` is the
/// newest review's `at` (the Unix epoch when nothing is listed), so the
/// output depends only on the inputs.
fn build_index(assessment: &Assessment) -> Index {
    let mut packs: Vec<IndexPack> = assessment
        .listed()
        .map(|listed| index_pack(assessment, listed))
        .collect();
    packs.sort_by(|a, b| a.name.cmp(&b.name));
    let first_party = packs.iter().filter(|p| p.first_party).count() as u64;
    let listed = packs.len() as u64;
    Index {
        generated: packs
            .iter()
            .map(|p| p.review.at)
            .max()
            .unwrap_or(OffsetDateTime::UNIX_EPOCH),
        counts: Counts {
            listed,
            first_party,
            other_owners: listed - first_party,
        },
        packs,
    }
}

/// An index with `protocol` first, as records are written.
#[derive(Serialize)]
struct Tagged<'a> {
    protocol: &'static str,
    #[serde(flatten)]
    index: &'a Index,
}

fn index_bytes(index: &Index) -> anyhow::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&Tagged {
        protocol: "HIPPO_V1",
        index,
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Writes the pages, `hippo.css` (the contract's base stylesheet, then its
/// `auto` theme), `registry.css` and the page icon. Pages of packs that are
/// no longer listed are removed from `packs/`.
fn write_site(out: &Path, pages: &[(String, String)], registry_css: &str) -> anyhow::Result<()> {
    let packs = out.join("packs");
    fs::create_dir_all(&packs).with_context(|| format!("creating {}", packs.display()))?;
    for item in fs::read_dir(&packs).with_context(|| format!("reading {}", packs.display()))? {
        let path = item?.path();
        if path.extension().is_some_and(|ext| ext == "html") {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    let write = |relative: &str, text: &str| -> anyhow::Result<()> {
        let path = out.join(relative);
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    };
    for (relative, html) in pages {
        write(relative, html)?;
    }
    write(
        "hippo.css",
        &format!("{}{}", hippo_theme::BASE_CSS, hippo_theme::AUTO_CSS),
    )?;
    write("registry.css", registry_css)?;
    write("icon.svg", hippo_theme::ICON_SVG)
}

pub fn run(root: &Path, args: &Args) -> anyhow::Result<ExitCode> {
    let assessment = assess(root, args.offline, None)?;
    for problem in &assessment.problems {
        println!("error {}", escaped(problem));
    }
    for result in &assessment.results {
        if result.listed.is_some() {
            continue;
        }
        let why = if result.skipped {
            "offline".to_owned()
        } else {
            result
                .lines
                .iter()
                .filter(|(level, _)| *level == crate::check::Level::Error)
                .map(|(_, why)| why.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        };
        println!("unlisted {}: {}", escaped(&result.name), escaped(&why));
    }
    let index = build_index(&assessment);
    let css_path = root.join("registry/site/registry.css");
    let registry_css =
        fs::read_to_string(&css_path).with_context(|| format!("reading {}", css_path.display()))?;
    fs::create_dir_all(&args.out).with_context(|| format!("creating {}", args.out.display()))?;
    let path = args.out.join("index.json");
    fs::write(&path, index_bytes(&index)?)
        .with_context(|| format!("writing {}", path.display()))?;
    let listed: Vec<&Listed> = assessment.listed().collect();
    let landing = match &args.landing {
        Some(path) => {
            Some(fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)
        }
        None => None,
    };
    let pages = site::pages(&assessment.settings, &index, &listed, landing.as_deref());
    write_site(&args.out, &pages, &registry_css)?;
    println!(
        "{} listed: {} first-party, {} by other owners",
        index.counts.listed, index.counts.first_party, index.counts.other_owners
    );
    println!("wrote {}", path.display());
    println!(
        "wrote {} pages, hippo.css, registry.css and icon.svg to {}",
        pages.len(),
        args.out.display()
    );
    Ok(ExitCode::SUCCESS)
}

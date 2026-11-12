//! `hippo detect [--json]`: what agent history this machine holds, before
//! anything is indexed. Read-only: it lists directories and reads file
//! metadata, and needs no `config.toml` (without one it looks where `hippo
//! init` would).

use std::process::ExitCode;

use anyhow::Result;
use hippo_core::config::{self, Config};
use hippo_core::detect;

#[derive(clap::Args)]
pub struct Args {
    /// Print one JSON object
    #[arg(long)]
    json: bool,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let home = config::home();
    let (patterns, exclude) = match Config::load_in(&home) {
        Ok(config) => (config.transcript_roots, config.exclude_roots),
        Err(_) => (config::default_transcript_roots(), Vec::new()),
    };
    let user_home = config::user_home();
    let mut found = detect::detect(&patterns, &home, user_home.as_deref());
    found.mark_excluded(&exclude);
    if args.json {
        println!("{}", serde_json::to_string(&found)?);
        return Ok(ExitCode::SUCCESS);
    }
    print_found(&found);
    Ok(ExitCode::SUCCESS)
}

/// What [`detect::detect`] found, as `hippo detect` and `hippo start`
/// print it: each pattern with its folders, the agents hippo cannot read
/// yet, and the total.
pub fn print_found(found: &detect::Detected) {
    for root in &found.roots {
        println!("{}", root.pattern);
        if root.dirs.is_empty() {
            println!("  no folder here ({})", expected(&root.pattern));
        }
        for dir in &root.dirs {
            let latest = dir
                .newest
                .map_or_else(|| "-".to_owned(), |t| t.date().to_string());
            // OpenCode's database counts sessions, not files.
            let unit = if dir.tool == "opencode" {
                "sessions"
            } else {
                "files"
            };
            println!(
                "  {} {} {unit} {} {}{}",
                dir.display,
                dir.files,
                human_bytes(dir.bytes),
                latest,
                if dir.excluded { " (excluded)" } else { "" }
            );
        }
    }
    let present: Vec<String> = found
        .unsupported
        .iter()
        .filter(|u| u.present)
        .map(|u| format!("{} {}", u.name, u.path))
        .collect();
    if !present.is_empty() {
        println!("not read yet (no adapter): {}", present.join(", "));
    }
    println!(
        "everything: {} files, {}",
        found.files(),
        human_bytes(found.bytes())
    );
}

/// What a root pattern holds when its folder exists.
fn expected(pattern: &str) -> &'static str {
    if pattern.contains(".claude") {
        "Claude Code keeps its sessions in ~/.claude/projects, or ~/.claude-<profile>/projects"
    } else if pattern.contains(".codex") {
        "Codex keeps its sessions in ~/.codex/sessions"
    } else if pattern.contains(".dsh") {
        "DeepSeek Harness (dsh) keeps its sessions in ~/.dsh/sessions, or $DSH_HOME/sessions"
    } else if pattern.contains("opencode") {
        "OpenCode keeps its sessions in ~/.local/share/opencode/opencode.db, or $XDG_DATA_HOME/opencode"
    } else if pattern.contains("muse") {
        "Muse Code keeps its sessions in ~/.local/share/muse/sessions, or $XDG_DATA_HOME/muse/sessions"
    } else {
        "a folder of agent transcripts"
    }
}

/// `1.2 GB`, `340 MB`, `12 KB`, `9 B`, in powers of 1,000.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GB", 1_000_000_000), ("MB", 1_000_000), ("KB", 1_000)];
    for (unit, size) in UNITS {
        if bytes >= size {
            return format!("{:.1} {unit}", bytes as f64 / size as f64);
        }
    }
    format!("{bytes} B")
}

//! `hippo mcp install --client <client> [--write]`: prints the client's MCP
//! configuration for `hippo mcp`, with this binary's absolute path; with
//! `--write`, merges it into the client's user file. Writing keeps every
//! other server and key in their order, replaces only an earlier `hippo`
//! entry, backs the file up first, and refuses a file it cannot read as
//! plain JSON (a file with comments) rather than lose anything. The paths
//! and formats, with their sources, are in `docs/mcp.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context as _, Result};

use super::ordered::Json;

/// The name hippo's entry has in every client's file.
pub const NAME: &str = "hippo";

#[derive(clap::Args)]
pub struct Args {
    /// The client to configure
    #[arg(long, value_enum)]
    client: Client,
    /// Merge the entry into the client's file (a backup is kept) instead of printing it
    #[arg(long)]
    write: bool,
    /// Configure `hippo mcp --allow-write`, which adds the tool that queues corrections to the Inbox
    #[arg(long)]
    allow_write: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Client {
    Cursor,
    ClaudeDesktop,
    Gemini,
    Windsurf,
    Zed,
    Vscode,
    ClaudeCode,
}

impl Client {
    pub fn label(self) -> &'static str {
        match self {
            Client::Cursor => "Cursor",
            Client::ClaudeDesktop => "Claude Desktop",
            Client::Gemini => "Gemini CLI",
            Client::Windsurf => "Windsurf",
            Client::Zed => "Zed",
            Client::Vscode => "VS Code (Copilot)",
            Client::ClaudeCode => "Claude Code",
        }
    }

    /// The key that holds the servers in the client's file.
    fn key(self) -> &'static str {
        match self {
            Client::Zed => "context_servers",
            _ => "mcpServers",
        }
    }
}

/// Where a client keeps its MCP servers, under `home`. `None` for Claude
/// Code, whose user servers live in its own state file.
pub fn config_path(client: Client, home: &Path) -> Option<PathBuf> {
    Some(match client {
        Client::Cursor => home.join(".cursor/mcp.json"),
        Client::ClaudeDesktop if cfg!(target_os = "macos") => {
            home.join("Library/Application Support/Claude/claude_desktop_config.json")
        }
        Client::ClaudeDesktop => home.join(".config/Claude/claude_desktop_config.json"),
        Client::Gemini => home.join(".gemini/settings.json"),
        Client::Windsurf => home.join(".codeium/windsurf/mcp_config.json"),
        Client::Zed => home.join(".config/zed/settings.json"),
        Client::Vscode => match std::env::var_os("COPILOT_HOME").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir).join("mcp-config.json"),
            None => home.join(".copilot/mcp-config.json"),
        },
        Client::ClaudeCode => return None,
    })
}

/// hippo's entry for `client`: the command and its arguments, in the
/// fields that client reads.
pub fn entry(client: Client, command: &str, allow_write: bool) -> Json {
    let mut args = vec![Json::string("mcp")];
    if allow_write {
        args.push(Json::string("--allow-write"));
    }
    let mut fields = Vec::new();
    if matches!(client, Client::Cursor | Client::Vscode) {
        fields.push(("type", Json::string("stdio")));
    }
    fields.push(("command", Json::string(command)));
    fields.push(("args", Json::Array(args)));
    if client == Client::Zed {
        fields.push(("env", Json::Object(Vec::new())));
    }
    Json::object(fields)
}

/// What `--write` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Written {
    /// The file already held this entry; nothing was written.
    Unchanged,
    /// The entry was added; `backup` is the earlier file's copy, if there
    /// was a file.
    Added { backup: Option<PathBuf> },
    /// An earlier `hippo` entry was replaced.
    Updated { backup: PathBuf },
}

/// Merges `entry` under `key` › `hippo` in the JSON file at `path`.
pub fn merge(path: &Path, key: &str, entry: Json, stamp: &str) -> Result<Written> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut root = match existing.as_deref() {
        None => Json::Object(Vec::new()),
        Some(text) if text.trim().is_empty() => Json::Object(Vec::new()),
        Some(text) => Json::parse(text).map_err(|e| {
            anyhow!(
                "{} is not plain JSON ({e}); it may hold comments, which hippo would lose. Add the entry by hand",
                path.display()
            )
        })?,
    };
    if !matches!(root, Json::Object(_)) {
        bail!(
            "{} does not hold a JSON object; add the entry by hand",
            path.display()
        );
    }
    let servers = root
        .entry(key, Json::Object(Vec::new()))
        .expect("the root is an object");
    if !matches!(servers, Json::Object(_)) {
        bail!(
            "\"{key}\" in {} is not an object; add the entry by hand",
            path.display()
        );
    }
    let before = servers.get(NAME).cloned();
    if before.as_ref() == Some(&entry) {
        return Ok(Written::Unchanged);
    }
    servers.set(NAME, entry);
    let backup = match existing {
        Some(text) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let backup = path.with_file_name(format!("{name}.hippo-backup-{stamp}"));
            fs::write(&backup, text)
                .with_context(|| format!("writing the backup {}", backup.display()))?;
            Some(backup)
        }
        None => None,
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("hippo-tmp");
    fs::write(&tmp, root.pretty()).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(match (before, backup) {
        (Some(_), Some(backup)) => Written::Updated { backup },
        (_, backup) => Written::Added { backup },
    })
}

/// The path a client should run: the `hippo` on `PATH` when it is this
/// binary (a Homebrew link stays valid across upgrades, its target does
/// not), this binary's own path otherwise.
pub fn command_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding this binary's path")?;
    let real = fs::canonicalize(&exe).unwrap_or_else(|_| exe.clone());
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("hippo");
            if candidate.is_absolute()
                && fs::canonicalize(&candidate).ok().as_deref() == Some(real.as_path())
            {
                return Ok(candidate);
            }
        }
    }
    Ok(exe)
}

/// `YYYYMMDDTHHMMSSZ`, for backup names.
fn stamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

pub fn run(args: Args) -> Result<ExitCode> {
    let command = command_path()?;
    let command = command
        .to_str()
        .ok_or_else(|| anyhow!("this binary's path is not UTF-8: {}", command.display()))?
        .to_owned();
    let client = args.client;
    let home =
        hippo_core::config::user_home().ok_or_else(|| anyhow!("no home directory; set HOME"))?;
    let Some(path) = config_path(client, &home) else {
        // Claude Code: its CLI writes its own state file.
        let extra = if args.allow_write {
            " --allow-write"
        } else {
            ""
        };
        let line =
            format!("claude mcp add --transport stdio --scope user {NAME} -- {command} mcp{extra}");
        if args.write {
            bail!(
                "Claude Code keeps user-scope MCP servers in its own state file, which hippo does not edit. Run:\n  {line}"
            );
        }
        println!("Claude Code adds MCP servers with its own command. Run:\n\n  {line}\n");
        println!("hippo's hooks already give Claude Code the start block (hippo hooks install); the MCP tools add search and the day's view.");
        return Ok(ExitCode::SUCCESS);
    };
    let key = client.key();
    let entry = entry(client, &command, args.allow_write);
    if !args.write {
        let snippet = Json::object(vec![(key, Json::object(vec![(NAME, entry)]))]);
        println!(
            "{} reads MCP servers from {}. Add this under \"{key}\", or run this command again with --write to merge it (a backup is kept):\n",
            client.label(),
            path.display()
        );
        print!("{}", snippet.pretty());
        return Ok(ExitCode::SUCCESS);
    }
    match merge(&path, key, entry, &stamp())? {
        Written::Unchanged => println!(
            "{} already runs hippo; {} is unchanged",
            client.label(),
            path.display()
        ),
        Written::Added { backup } => {
            println!("added \"{NAME}\" to {}", path.display());
            if let Some(b) = backup {
                println!("the earlier file is at {}", b.display());
            }
        }
        Written::Updated { backup } => {
            println!("updated \"{NAME}\" in {}", path.display());
            println!("the earlier file is at {}", backup.display());
        }
    }
    println!("Restart {} to load it.", client.label());
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn writing_keeps_other_servers_and_keys_in_order_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let before = "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"other\": {\n      \"command\": \"/bin/other\",\n      \"args\": []\n    }\n  },\n  \"zz\": 1\n}\n";
        fs::write(&path, before).unwrap();
        let written = merge(
            &path,
            "mcpServers",
            entry(Client::Cursor, "/opt/hippo", false),
            "S",
        )
        .unwrap();
        let backup = dir.path().join("mcp.json.hippo-backup-S");
        assert_eq!(
            written,
            Written::Added {
                backup: Some(backup.clone())
            }
        );
        assert_eq!(read(&backup), before);
        assert_eq!(
            read(&path),
            "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"other\": {\n      \"command\": \"/bin/other\",\n      \"args\": []\n    },\n    \"hippo\": {\n      \"type\": \"stdio\",\n      \"command\": \"/opt/hippo\",\n      \"args\": [\n        \"mcp\"\n      ]\n    }\n  },\n  \"zz\": 1\n}\n"
        );
        // The same entry again writes nothing.
        let again = merge(
            &path,
            "mcpServers",
            entry(Client::Cursor, "/opt/hippo", false),
            "T",
        )
        .unwrap();
        assert_eq!(again, Written::Unchanged);
        assert!(!dir.path().join("mcp.json.hippo-backup-T").exists());
        // A new path replaces the entry in place.
        let moved = merge(
            &path,
            "mcpServers",
            entry(Client::Cursor, "/usr/bin/hippo", true),
            "U",
        )
        .unwrap();
        assert!(matches!(moved, Written::Updated { .. }));
        let now = Json::parse(&read(&path)).unwrap();
        let servers = now.get("mcpServers").unwrap();
        assert!(servers.get("other").is_some());
        assert_eq!(
            serde_json::to_string(servers.get("hippo").unwrap()).unwrap(),
            "{\"type\":\"stdio\",\"command\":\"/usr/bin/hippo\",\"args\":[\"mcp\",\"--allow-write\"]}"
        );
    }

    #[test]
    fn a_missing_file_is_created_and_a_commented_one_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zed/settings.json");
        let written = merge(
            &path,
            "context_servers",
            entry(Client::Zed, "/h", false),
            "S",
        )
        .unwrap();
        assert_eq!(written, Written::Added { backup: None });
        assert_eq!(
            read(&path),
            "{\n  \"context_servers\": {\n    \"hippo\": {\n      \"command\": \"/h\",\n      \"args\": [\n        \"mcp\"\n      ],\n      \"env\": {}\n    }\n  }\n}\n"
        );
        let commented = "// Zed settings\n{\n  \"theme\": \"One Dark\"\n}\n";
        fs::write(&path, commented).unwrap();
        let err = merge(
            &path,
            "context_servers",
            entry(Client::Zed, "/h", false),
            "T",
        )
        .unwrap_err();
        assert!(err.to_string().contains("by hand"), "{err}");
        assert_eq!(read(&path), commented);
        let not_object = dir.path().join("arr.json");
        fs::write(&not_object, "{\"mcpServers\": []}").unwrap();
        assert!(merge(
            &not_object,
            "mcpServers",
            entry(Client::Gemini, "/h", false),
            "S"
        )
        .is_err());
    }

    #[test]
    fn every_client_but_claude_code_has_a_file_under_home() {
        let home = Path::new("/home/x");
        for client in [
            Client::Cursor,
            Client::ClaudeDesktop,
            Client::Gemini,
            Client::Windsurf,
            Client::Zed,
        ] {
            assert!(
                config_path(client, home).unwrap().starts_with(home),
                "{client:?}"
            );
        }
        assert!(config_path(Client::ClaudeCode, home).is_none());
    }
}

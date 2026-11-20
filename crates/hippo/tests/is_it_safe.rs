//! docs/is-it-safe.md against the code. Fails when hippo gains a hook, an
//! MCP tool, a program it starts or an outbound host that the page does
//! not name, so the page cannot fall behind silently.
//!
//! - Hooks: each agent's hooks are generated with hippo-core's own
//!   installers into a temporary file, as `hippo hooks install` and `hippo
//!   guard install` write them; every event and every command (with the
//!   binary's path as `hippo`) must be on the page. The Claude Code
//!   plugin's hooks file is read as it ships.
//! - MCP: every `hippo_*` tool name in the server.
//! - Programs: every `Command::new("…")` in the shipped code (the CLI, its
//!   libraries and Hippo.app, test modules left out) names a program the
//!   page lists; a spawn whose program is not a literal must be in a file
//!   the page names.
//! - Hosts: every host in an `http(s)://` string literal.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use hippo_core::hooks::{self, Agent, GuardAgent};
use regex::Regex;
use serde_json::Value;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const SHIPPED: [&str; 5] = [
    "crates/hippo/src",
    "crates/hippo-core/src",
    "crates/hippo-view/src",
    "crates/hippo-theme/src",
    "app/hippo-app/src",
];
const BIN: &str = "/opt/hippo-test/bin/hippo";

fn page() -> String {
    fs::read_to_string(Path::new(ROOT).join("docs/is-it-safe.md")).unwrap()
}

/// `(event, command)` for every hook in a hooks file, with the binary as `hippo`.
fn hooks_in(text: &str) -> Vec<(String, String)> {
    let v: Value = serde_json::from_str(text).unwrap();
    let mut out = Vec::new();
    let Some(events) = v["hooks"].as_object() else {
        return out;
    };
    for (event, groups) in events {
        for g in groups.as_array().unwrap() {
            // Grouped (Claude Code, Codex, Gemini) or flat (Cursor).
            let list = g["hooks"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![g.clone()]);
            for h in list {
                if let Some(c) = h["command"].as_str() {
                    out.push((event.clone(), c.replace(BIN, "hippo")));
                }
            }
        }
    }
    out
}

#[test]
fn every_hook_is_on_the_page() {
    let page = page();
    let dir = tempfile::tempdir().unwrap();
    let mut found = Vec::new();
    for (i, agent) in [
        Agent::ClaudeCode,
        Agent::ClaudeCodeUser,
        Agent::Codex,
        Agent::Cursor,
        Agent::Gemini,
    ]
    .into_iter()
    .enumerate()
    {
        let file = dir.path().join(format!("session-{i}.json"));
        hooks::install_file(&file, agent, BIN).unwrap();
        found.extend(hooks_in(&fs::read_to_string(&file).unwrap()));
    }
    for (i, agent) in GuardAgent::ALL.into_iter().enumerate() {
        let file = dir.path().join(format!("guard-{i}.json"));
        let plan = hooks::guard_plan(&file, agent, BIN, 1500, false).unwrap();
        found.extend(hooks_in(&plan.text));
    }
    let plugin = fs::read_to_string(Path::new(ROOT).join("plugin/hooks/hooks.json")).unwrap();
    // The plugin's command finds the binary itself; what it runs is
    // `hook session-start --plugin`.
    let run = Regex::new(r"hook [a-z-]+ --plugin").unwrap();
    for (event, command) in hooks_in(&plugin) {
        let m = run.find(&command).expect("the plugin runs hippo hook");
        found.push((event, format!("hippo {}", m.as_str())));
    }
    assert!(found.len() >= 15, "{found:?}");
    for (event, command) in found {
        assert!(
            page.contains(&format!("`{event}`")),
            "docs/is-it-safe.md does not name the hook event `{event}` ({command})"
        );
        assert!(
            page.contains(&format!("`{command}`")),
            "docs/is-it-safe.md does not name the hook command `{command}` ({event})"
        );
    }
}

#[test]
fn every_mcp_tool_is_on_the_page() {
    let page = page();
    let server =
        fs::read_to_string(Path::new(ROOT).join("crates/hippo/src/cmd/mcp/server.rs")).unwrap();
    let names: BTreeSet<String> = Regex::new(r#""(hippo_[a-z_]+)""#)
        .unwrap()
        .captures_iter(&server)
        .map(|c| c[1].to_owned())
        .collect();
    assert!(names.len() >= 6, "{names:?}");
    for name in names {
        assert!(
            page.contains(&format!("`{name}`")),
            "docs/is-it-safe.md does not list the MCP tool `{name}`"
        );
    }
    assert!(page.contains("--allow-write"));
}

/// The shipped Rust files with their test modules cut out.
fn shipped_sources() -> Vec<(PathBuf, String)> {
    let test_mod = Regex::new(r"#\[cfg\(test\)\]\s*(pub(\(crate\))?\s+)?mod\s+\w+\s*\{").unwrap();
    let mut out = Vec::new();
    for root in SHIPPED {
        let mut stack = vec![Path::new(ROOT).join(root)];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n != "tests") {
                        stack.push(path);
                    }
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("tests.rs") {
                    continue;
                }
                let text = fs::read_to_string(&path).unwrap();
                let mut kept = String::new();
                let mut at = 0;
                for m in test_mod.find_iter(&text) {
                    if m.start() < at {
                        continue;
                    }
                    kept.push_str(&text[at..m.start()]);
                    let mut depth = 1;
                    let mut end = m.end();
                    for (i, c) in text[m.end()..].char_indices() {
                        match c {
                            '{' => depth += 1,
                            '}' => depth -= 1,
                            _ => {}
                        }
                        if depth == 0 {
                            end = m.end() + i + 1;
                            break;
                        }
                    }
                    at = end;
                }
                kept.push_str(&text[at..]);
                let rel = path
                    .strip_prefix(Path::new(ROOT))
                    .unwrap_or(&path)
                    .to_path_buf();
                let rel = PathBuf::from(
                    rel.to_string_lossy()
                        .trim_start_matches("crates/hippo/../../")
                        .to_owned(),
                );
                out.push((rel, kept));
            }
        }
    }
    out
}

#[test]
fn every_program_hippo_starts_is_on_the_page() {
    let page = page();
    let spawn = Regex::new(r"(?:Command|Process)::new\(\s*([^;\n]*)").unwrap();
    let literal = Regex::new(r#"^"([^"]+)""#).unwrap();
    let mut programs = BTreeSet::new();
    let mut by_variable = BTreeSet::new();
    for (path, text) in shipped_sources() {
        for c in spawn.captures_iter(&text) {
            match literal.captures(c[1].trim()) {
                Some(l) => {
                    let p = Path::new(&l[1])
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    programs.insert(p);
                }
                None => {
                    by_variable.insert(path.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
    }
    assert!(
        programs.contains("git") && programs.contains("curl"),
        "{programs:?}"
    );
    for p in programs {
        assert!(
            page.contains(&format!("`{p}`")),
            "docs/is-it-safe.md does not list the program `{p}`, which hippo starts"
        );
    }
    for file in by_variable {
        assert!(page.contains(&format!("`{file}`")), "{file} starts a program by a name it does not hard-code; docs/is-it-safe.md must name the file");
    }
}

#[test]
fn every_host_in_the_code_is_on_the_page() {
    let page = page();
    let url = Regex::new(r#""[^"\n]*?https?://([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+)"#).unwrap();
    let mut hosts = BTreeSet::new();
    for (_, text) in shipped_sources() {
        for line in text.lines() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for c in url.captures_iter(line) {
                hosts.insert(c[1].to_lowercase());
            }
        }
    }
    assert!(hosts.contains("github.com"), "{hosts:?}");
    for host in hosts {
        assert!(
            page.contains(&format!("`{host}`")),
            "docs/is-it-safe.md does not name the host `{host}`, which the code has in a URL"
        );
    }
}

#[test]
fn the_page_names_what_hippo_never_does() {
    let page = page();
    for claim in [
        "git commit",
        "git push",
        "telemetry",
        "quarantine",
        "sudo",
        "hippo schedule install",
    ] {
        assert!(page.contains(claim), "{claim}");
    }
}

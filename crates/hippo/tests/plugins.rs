//! The agent plugins this repository ships, checked as their hosts read
//! them: the Codex plugin (`.agents/plugins/marketplace.json` and
//! `plugins/codex/`) against the rules in Codex's plugin loader, and the
//! Claude Code plugin (`.claude-plugin/marketplace.json` and `plugin/`).
//! Every path a manifest names must exist inside its plugin's folder, and
//! every version must be the workspace's.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn root() -> PathBuf {
    Path::new(ROOT).canonicalize().unwrap()
}

fn json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A manifest path as Codex accepts it: `./`, relative, no `..`, inside
/// `base`, and existing.
fn codex_path(base: &Path, field: &str, path: &str) -> PathBuf {
    let rel = path
        .strip_prefix("./")
        .unwrap_or_else(|| panic!("{field}: {path} must start with ./"));
    assert!(!rel.is_empty(), "{field}: must not be ./");
    assert!(
        Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "{field}: {path} must stay inside the plugin"
    );
    let full = base.join(rel);
    assert!(full.exists(), "{field}: {} does not exist", full.display());
    full
}

/// `name` and `description` from a SKILL.md's frontmatter.
fn frontmatter(path: &Path) -> (String, String) {
    let text = fs::read_to_string(path).unwrap();
    let body = text
        .strip_prefix("---\n")
        .unwrap_or_else(|| panic!("{}: no frontmatter", path.display()));
    let end = body.find("\n---\n").expect("frontmatter ends");
    let mut name = String::new();
    let mut description = String::new();
    for line in body[..end].lines() {
        if let Some(v) = line.strip_prefix("name: ") {
            name = v.trim().to_owned();
        }
        if let Some(v) = line.strip_prefix("description: ") {
            description = v.trim().to_owned();
        }
    }
    (name, description)
}

#[test]
fn the_codex_marketplace_points_at_the_codex_plugin() {
    let root = root();
    let m = json(&root.join(".agents/plugins/marketplace.json"));
    assert_eq!(m["name"], "hippo");
    let plugins = m["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), 1);
    let p = &plugins[0];
    assert_eq!(p["name"], "hippo");
    assert_eq!(p["source"]["source"], "local");
    let dir = codex_path(&root, "source.path", p["source"]["path"].as_str().unwrap());
    assert_eq!(dir, root.join("plugins/codex"));
    assert!(dir.join(".codex-plugin/plugin.json").is_file());
    assert_eq!(p["policy"]["installation"], "AVAILABLE");
}

#[test]
fn the_codex_manifest_is_valid_and_its_paths_exist() {
    let base = root().join("plugins/codex");
    let m = json(&base.join(".codex-plugin/plugin.json"));
    assert_eq!(m["name"], "hippo");
    assert_eq!(m["version"], VERSION, "the Codex plugin's version");
    assert_eq!(m["license"], "MIT");
    let skills = codex_path(&base, "skills", m["skills"].as_str().unwrap());
    assert!(skills.is_dir());
    let i = &m["interface"];
    for key in [
        "displayName",
        "shortDescription",
        "longDescription",
        "developerName",
    ] {
        assert!(
            i[key].as_str().is_some_and(|s| !s.is_empty()),
            "interface.{key}"
        );
    }
    for key in ["composerIcon", "logo"] {
        let icon = codex_path(&base, key, i[key].as_str().unwrap());
        let bytes = fs::read(&icon).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"), "{key} is a PNG");
    }
    // Codex keeps at most three prompts of at most 512 characters.
    let prompts = i["defaultPrompt"].as_array().unwrap();
    assert!((1..=3).contains(&prompts.len()));
    for p in prompts {
        let p = p.as_str().unwrap();
        assert!(!p.trim().is_empty() && p.chars().count() <= 512, "{p}");
    }
    for key in ["websiteURL", "privacyPolicyURL"] {
        assert!(i[key].as_str().unwrap().starts_with("https://"), "{key}");
    }
    // No hooks or MCP servers: Codex would load hooks/hooks.json from the
    // plugin's folder by default, and hippo's Codex hooks come from
    // `hippo hooks install --agent codex` instead.
    assert!(m.get("hooks").is_none() && m.get("mcpServers").is_none());
    assert!(!base.join("hooks/hooks.json").exists());
    assert!(!base.join(".mcp.json").exists());
}

#[test]
fn the_codex_skill_is_named_hippo() {
    let skill = root().join("plugins/codex/skills/hippo");
    let (name, description) = frontmatter(&skill.join("SKILL.md"));
    assert_eq!(name, "hippo");
    assert!(name.len() <= 64);
    assert!(description.len() > 40 && description.len() <= 1024);
    let yaml = fs::read_to_string(skill.join("agents/openai.yaml")).unwrap();
    assert!(yaml.contains("display_name: \"hippo\""));
    assert!(
        yaml.contains("$hippo"),
        "the default prompt names the skill"
    );
    // Every command the skill tells Codex to run exists.
    let text = fs::read_to_string(skill.join("SKILL.md")).unwrap();
    for command in [
        "hippo setup --agent codex",
        "hippo setup --agent codex --yes",
        "hippo schedule install",
        "hippo ask --sources-only",
        "hippo brief --task",
        "hippo lessons keep",
        "hippo lessons dismiss",
    ] {
        assert!(text.contains(command), "{command}");
    }
}

#[test]
fn the_claude_code_plugin_is_valid_and_its_paths_exist() {
    let root = root();
    let m = json(&root.join(".claude-plugin/marketplace.json"));
    let p = &m["plugins"][0];
    assert_eq!(p["name"], "hippo");
    let dir = codex_path(&root, "source", p["source"].as_str().unwrap());
    let manifest = json(&dir.join(".claude-plugin/plugin.json"));
    assert_eq!(manifest["name"], "hippo");
    assert_eq!(
        manifest["version"], VERSION,
        "the Claude Code plugin's version"
    );
    let hooks = json(&dir.join("hooks/hooks.json"));
    assert!(hooks["hooks"]["SessionStart"].is_array());
    for skill in ["setup", "ask", "day"] {
        let (name, _) = frontmatter(&dir.join("skills").join(skill).join("SKILL.md"));
        assert_eq!(name, skill);
    }
    let setup = fs::read_to_string(dir.join("skills/setup/SKILL.md")).unwrap();
    for command in ["hippo setup", "hippo setup --yes", "hippo schedule install"] {
        assert!(setup.contains(command), "{command}");
    }
}

//! Handing a brief to a new agent: the one-shot script behind *Start claude
//! here* and the link behind *Send to T3 Code*.
//! Pure: the app writes the files and opens
//! them.

use std::path::Path;

use hippocamp_core::resume::quote;

/// T3 Code's prompt links carry at most this many characters.
pub const T3_PROMPT_LIMIT: usize = 8000;

/// Which agent a start runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    /// `codex [PROMPT]` (codex-cli 0.167.0's `codex --help`); one home, so
    /// no profile variable.
    Codex,
}

impl Agent {
    pub fn program(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }
}

/// [`start_script`] for any agent: Codex takes the prompt the same way and
/// never gets `CLAUDE_CONFIG_DIR`.
pub fn start_script_for(
    agent: Agent,
    repo: &Path,
    prompt_file: &Path,
    config_dir: Option<&Path>,
) -> String {
    match agent {
        Agent::Claude => start_script(repo, prompt_file, config_dir),
        Agent::Codex => start_script(repo, prompt_file, None)
            .replace("exec claude \"$prompt\"\n", "exec codex \"$prompt\"\n"),
    }
}

/// [`start_line`] for any agent.
pub fn start_line_for(agent: Agent, repo: &Path, config_dir: Option<&Path>) -> String {
    match agent {
        Agent::Claude => start_line(repo, config_dir),
        Agent::Codex => format!(
            "cd {} && codex \"<the brief>\"",
            quote(&repo.to_string_lossy())
        ),
    }
}

/// The script Terminal runs for *Start claude here*: `cd` to the
/// repository, read the block from `prompt_file` into a variable, delete
/// both files, then run `claude "$prompt"`, with `CLAUDE_CONFIG_DIR` for a
/// profile other than the default. The block never passes through a
/// shell's quoting: it is read from the file at run time.
pub fn start_script(repo: &Path, prompt_file: &Path, config_dir: Option<&Path>) -> String {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo.to_string_lossy().into_owned());
    let repo = quote(&repo.to_string_lossy());
    let file = quote(&prompt_file.to_string_lossy());
    let mut s =
        String::from("#!/bin/sh\n# Written by Hippocamp.app for one run; it deletes itself.\n");
    // A clean screen and one line saying what this is.
    s.push_str("clear\n");
    s.push_str(&format!(
        "printf '%s\\n' {}\n",
        quote(&format!("hippocamp: starting a new agent in {name}"))
    ));
    s.push_str(&format!("cd {repo} || exit 1\n"));
    s.push_str(&format!("prompt=$(cat {file})\n"));
    s.push_str(&format!("rm -f {file} \"$0\"\n"));
    if let Some(dir) = config_dir {
        s.push_str(&format!(
            "export CLAUDE_CONFIG_DIR={}\n",
            quote(&dir.to_string_lossy())
        ));
    }
    s.push_str("exec claude \"$prompt\"\n");
    s
}

/// What the app shows for a start: `cd <repo> && CLAUDE_CONFIG_DIR=… claude <brief>`.
pub fn start_line(repo: &Path, config_dir: Option<&Path>) -> String {
    let env = config_dir
        .map(|d| format!("CLAUDE_CONFIG_DIR={} ", quote(&d.to_string_lossy())))
        .unwrap_or_default();
    format!(
        "cd {} && {env}claude \"<the brief>\"",
        quote(&repo.to_string_lossy())
    )
}

/// `owner/name` of a GitHub remote URL (`git@github.com:o/n.git`,
/// `https://github.com/o/n`).
pub fn github_slug(remote: &str) -> Option<String> {
    let r = remote.trim();
    let rest = r
        .strip_prefix("git@github.com:")
        .or_else(|| r.strip_prefix("https://github.com/"))
        .or_else(|| r.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = rest.split('/');
    let (o, n) = (parts.next()?, parts.next()?);
    (!o.is_empty() && !n.is_empty() && parts.next().is_none()).then(|| format!("{o}/{n}"))
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3 / 2);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `t3code://new?prompt=<block>&repo=<owner/name>`: T3 Code opens it as a
/// new thread's draft ("Nothing runs until you press Send"). `None` when the
/// block is over [`T3_PROMPT_LIMIT`] characters or empty.
pub fn t3_link(block: &str, repo: Option<&str>) -> Option<String> {
    let block = block.trim();
    let n = block.chars().count();
    if n == 0 || n > T3_PROMPT_LIMIT {
        return None;
    }
    let mut url = format!("t3code://new?prompt={}", encode(block));
    if let Some(r) = repo {
        url.push_str(&format!("&repo={}", encode(r)));
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_reads_the_block_from_its_file_and_quotes_paths() {
        let s = start_script(
            Path::new("/work/it's here"),
            Path::new("/tmp/hippocamp-brief/brief.txt"),
            Some(Path::new("/Users/me/.claude-work")),
        );
        assert!(s.contains("cd '/work/it'\\''s here' || exit 1\n"), "{s}");
        assert!(
            s.contains(
                "clear\nprintf '%s\\n' 'hippocamp: starting a new agent in it'\\''s here'\n"
            ),
            "{s}"
        );
        assert!(s.contains("prompt=$(cat /tmp/hippocamp-brief/brief.txt)\n"));
        assert!(s.contains("rm -f /tmp/hippocamp-brief/brief.txt \"$0\"\n"));
        assert!(s.contains("export CLAUDE_CONFIG_DIR=/Users/me/.claude-work\n"));
        assert!(s.ends_with("exec claude \"$prompt\"\n"));
        let d = start_script(Path::new("/w"), Path::new("/t/b"), None);
        assert!(!d.contains("CLAUDE_CONFIG_DIR"));
        let c = start_script_for(
            Agent::Codex,
            Path::new("/w"),
            Path::new("/t/b"),
            Some(Path::new("/Users/me/.claude-work")),
        );
        assert!(!c.contains("CLAUDE_CONFIG_DIR"), "{c}");
        assert!(c.contains("prompt=$(cat /t/b)\n"));
        assert!(c.ends_with("exec codex \"$prompt\"\n"), "{c}");
        assert_eq!(
            start_line_for(Agent::Codex, Path::new("/w"), None),
            "cd /w && codex \"<the brief>\""
        );
    }

    #[test]
    fn github_slugs_come_from_either_remote_form() {
        assert_eq!(
            github_slug("git@github.com:me/hippocamp.git").as_deref(),
            Some("me/hippocamp")
        );
        assert_eq!(
            github_slug("https://github.com/me/hippocamp\n").as_deref(),
            Some("me/hippocamp")
        );
        assert_eq!(github_slug("https://gitlab.com/me/x"), None);
        assert_eq!(github_slug("git@github.com:me"), None);
    }

    #[test]
    fn t3_links_encode_the_block_and_respect_the_limit() {
        let l = t3_link("# brief\ntask: a & b", Some("me/hippocamp")).unwrap();
        assert_eq!(
            l,
            "t3code://new?prompt=%23%20brief%0Atask%3A%20a%20%26%20b&repo=me%2Fhippocamp"
        );
        assert!(t3_link(&"x".repeat(8001), None).is_none());
        assert!(t3_link("  ", None).is_none());
    }
}

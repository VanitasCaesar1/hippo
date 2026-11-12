//! Packs: directories of passes, prompts and personas under
//! `.hippocamp/packs/<name>/`. Packs are data and never execute; a pass may
//! read only what its `reads` declares.
//!
//! The `core` pack ships inside hippocamp ([`CORE_PACK`]) and `hippocamp init`
//! writes it ([`write_core_pack`]); the person who maintains the workspace may edit it from then
//! on. [`load_passes`] reads every pack's passes and checks them.

pub mod files;
pub mod install;
pub mod manifest;
pub mod origin;
pub mod registry;
pub mod review;
pub mod source;
pub mod testing;
pub mod validate;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::error::{Error, Result};
use crate::records::{Input, Pass};

/// Whether `c` is hidden from a reader or moves a terminal: a control, a
/// line separator or a bidirectional or format mark.
fn is_hidden(c: char) -> bool {
    let code = u32::from(c);
    c.is_control() || matches!(code, 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x2069 | 0xFEFF)
}

/// `text` with every control, line-separator and bidirectional character
/// written as `\u{..}`, so text from a pack, an index, git or a model cannot
/// start a line or move the cursor. Every message that can carry outside
/// text goes through this.
pub fn escape(text: &str) -> String {
    text.chars()
        .map(|c| {
            if is_hidden(c) {
                format!("\\u{{{:x}}}", u32::from(c))
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// [`escape`], keeping line feed and tab: for a diff, which is many lines.
pub fn escape_block(text: &str) -> String {
    text.chars()
        .map(|c| {
            if is_hidden(c) && c != '\n' && c != '\t' {
                format!("\\u{{{:x}}}", u32::from(c))
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// The `core` pack: each file's path inside the pack and its contents.
pub const CORE_PACK: &[(&str, &str)] = &[
    (
        "passes/consolidate.toml",
        include_str!("../../packs/core/passes/consolidate.toml"),
    ),
    (
        "passes/verify.toml",
        include_str!("../../packs/core/passes/verify.toml"),
    ),
    (
        "prompts/consolidate.md",
        include_str!("../../packs/core/prompts/consolidate.md"),
    ),
    (
        "prompts/verify.md",
        include_str!("../../packs/core/prompts/verify.md"),
    ),
    (
        "personas/consolidate-reviewer.md",
        include_str!("../../packs/core/personas/consolidate-reviewer.md"),
    ),
    (
        "personas/verify-reviewer.md",
        include_str!("../../packs/core/personas/verify-reviewer.md"),
    ),
];

/// The system prompt of the reviewer of every pass outside `core`: hippocamp's
/// own, the same for every pack. The pack's persona reaches the reviewer
/// as criteria in its message.
pub const PASS_REVIEWER: &str = include_str!("../../personas/pass-reviewer.md");

/// Writes [`CORE_PACK`] under `<workspace>/.hippocamp/packs/core/`, replacing
/// files that exist, and returns the paths written.
pub fn write_core_pack(workspace: &Path) -> Result<Vec<PathBuf>> {
    let dir = workspace.join(".hippocamp").join("packs").join("core");
    let mut written = Vec::new();
    for (relative, text) in CORE_PACK {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
        }
        fs::write(&path, text).map_err(|e| Error::io_at(&path, e))?;
        written.push(path);
    }
    Ok(written)
}

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{([^{}]*)\}\}").expect("the placeholder pattern is valid"));

/// The largest prompt or persona the loader accepts, in bytes. A `core`
/// persona goes to the model as one command-line argument.
pub const MAX_FILE_BYTES: u64 = 65_536;

/// Every pass under `<workspace>/.hippocamp/packs/*/passes/*.toml`, in lexical
/// order of `<pack>/<name>`, each checked: its `protocol`, that `reads`
/// names only known inputs, that its `prompt` and `reviewer` are regular
/// files of at most [`MAX_FILE_BYTES`] inside the pack, and that the
/// prompt's `{{...}}` placeholders are known inputs named in `reads`.
/// Hidden entries are skipped. A symbolic link is an error: a pack
/// directory, its `passes/` directory, a pass file, or any component of a
/// prompt or persona path below the pack directory. Two passes with one
/// file name, in any packs, are an error naming both files.
pub fn load_passes(workspace: &Path) -> Result<Vec<(PathBuf, Pass)>> {
    let packs = workspace.join(".hippocamp").join("packs");
    let mut files = Vec::new();
    for pack in visible_entries(&packs)? {
        let Some(meta) = inspect(&pack)? else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let passes = pack.join("passes");
        if inspect(&passes)?.is_none() {
            continue;
        }
        for path in visible_entries(&passes)? {
            let Some(meta) = inspect(&path)? else {
                continue;
            };
            if meta.is_file() && path.extension().is_some_and(|ext| ext == "toml") {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut names: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut loaded = Vec::new();
    for path in files {
        let stem = path
            .file_stem()
            .expect("a pass file has a name")
            .to_string_lossy()
            .into_owned();
        if let Some(first) = names.get(&stem) {
            return Err(Error::Pack(format!(
                "duplicate pass name {stem}: {} and {}",
                first.display(),
                path.display()
            )));
        }
        names.insert(stem, path.clone());
        let pass = Pass::load(&path)?;
        pass.check_filters().map_err(|why| invalid(&path, &why))?;
        let pack = path
            .parent()
            .and_then(Path::parent)
            .expect("a pass file is in <pack>/passes/");
        let prompt = checked_pack_file(&path, pack, &pass.prompt, "prompt")?;
        checked_pack_file(&path, pack, &pass.reviewer, "reviewer")?;
        let text = fs::read_to_string(&prompt).map_err(|e| Error::io_at(&prompt, e))?;
        check_placeholders(&text, &prompt.display().to_string(), &pass.reads)
            .map_err(|why| invalid(&path, &why))?;
        loaded.push((path, pass));
    }
    Ok(loaded)
}

/// The metadata of `path` itself, not of what a link points to: none when
/// it is missing, an error when it is a symbolic link.
fn inspect(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(link(path)),
        Ok(meta) => Ok(Some(meta)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::io_at(path, err)),
    }
}

fn link(path: &Path) -> Error {
    Error::Pack(format!(
        "{}: a symbolic link; packs may not contain links",
        path.display()
    ))
}

/// `relative` inside `pack`: a relative path without `..` that names a
/// regular file of at most [`MAX_FILE_BYTES`], reached through no symbolic
/// link below `pack` and whose canonical path lies inside the canonical
/// `pack`. Components of `pack` itself are not checked.
fn checked_pack_file(pass: &Path, pack: &Path, relative: &Path, key: &str) -> Result<PathBuf> {
    let inside = relative.components().next().is_some()
        && relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if !inside {
        let why = format!("{key} {} is not a path inside the pack", relative.display());
        return Err(invalid(pass, &why));
    }
    let path = pack.join(relative);
    let missing = || invalid(pass, &format!("{key} {} does not exist", path.display()));
    let mut walked = pack.to_path_buf();
    let mut meta = None;
    for component in relative.components() {
        walked.push(component);
        meta = Some(inspect(&walked)?.ok_or_else(missing)?);
    }
    let meta = meta.expect("relative has a component");
    if !meta.is_file() {
        return Err(missing());
    }
    let canonical = path.canonicalize().map_err(|e| Error::io_at(&path, e))?;
    let root = pack.canonicalize().map_err(|e| Error::io_at(pack, e))?;
    if !canonical.starts_with(&root) {
        let why = format!("{key} {} is not a path inside the pack", relative.display());
        return Err(invalid(pass, &why));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(Error::Pack(format!(
            "{}: {} bytes, over the {MAX_FILE_BYTES}-byte limit",
            path.display(),
            meta.len()
        )));
    }
    Ok(path)
}

/// Checks that each `{{name}}` placeholder of `prompt` is a known input
/// named in `reads`. `label` names the prompt in the message.
pub fn check_placeholders(
    prompt: &str,
    label: &str,
    reads: &[Input],
) -> std::result::Result<(), String> {
    for captures in PLACEHOLDER.captures_iter(prompt) {
        let name = captures[1].trim();
        match reads.iter().find(|read| read.as_str() == name) {
            Some(_) => {}
            None if is_input(name) => {
                return Err(format!("{{{{{name}}}}} in {label} is not in reads"));
            }
            None => return Err(format!("unknown placeholder {{{{{name}}}}} in {label}")),
        }
    }
    Ok(())
}

fn is_input(name: &str) -> bool {
    [
        Input::Sessions,
        Input::Notes,
        Input::Turns,
        Input::HumanConfirmed,
    ]
    .iter()
    .any(|input| input.as_str() == name)
}

fn invalid(path: &Path, why: &str) -> Error {
    Error::io_at(
        path,
        io::Error::new(io::ErrorKind::InvalidData, why.to_owned()),
    )
}

/// The entries of `dir` whose names do not start with `.`, sorted; none
/// when `dir` is missing.
fn visible_entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::io_at(dir, err)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io_at(dir, e))?;
        if !entry.file_name().as_encoded_bytes().starts_with(b".") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn escape_hides_controls_and_bidirectional_marks_and_is_idempotent() {
        assert_eq!(
            escape("a\nb\u{1b}[2J\u{202e}\u{200b}\u{feff}c"),
            "a\\u{a}b\\u{1b}[2J\\u{202e}\\u{200b}\\u{feff}c"
        );
        assert_eq!(escape(&escape("a\u{7}")), escape("a\u{7}"));
        assert_eq!(escape_block("a\n\tb\u{7}"), "a\n\tb\\u{7}");
    }

    #[test]
    fn the_core_pack_loads_as_two_checked_passes() {
        let workspace = tempfile::tempdir().unwrap();
        let written = write_core_pack(workspace.path()).unwrap();
        assert_eq!(written.len(), CORE_PACK.len());
        let passes = load_passes(workspace.path()).unwrap();
        let names: Vec<String> = passes
            .iter()
            .map(|(path, _)| path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["consolidate", "verify"]);
        let (consolidate, verify) = (&passes[0].1, &passes[1].1);
        assert_eq!(consolidate.reads, [Input::Sessions, Input::Notes]);
        assert!(!consolidate.per_note);
        assert_eq!(
            verify.reads,
            [
                Input::Sessions,
                Input::Notes,
                Input::Turns,
                Input::HumanConfirmed
            ]
        );
        assert!(verify.per_note);
        assert_eq!(verify.input_budget_bytes, 200_000);
    }

    #[test]
    fn prompts_end_with_the_closing_instruction_and_personas_credit_egm() {
        let closing = "Respond with only a JSON array of objects `{note, kind, old, new, \
                       evidence: [{session, ordinal}], reason}` as your final message; \
                       an empty array means no proposals.\n";
        for (path, text) in CORE_PACK {
            if path.starts_with("prompts/") {
                assert!(text.ends_with(closing), "{path}");
            }
            if path.starts_with("personas/") {
                for section in ["## Role", "## What you receive", "## Rules", "## Reply"] {
                    assert!(text.contains(section), "{path} lacks {section}");
                }
                assert!(text.contains("Elephant-Goldfish Model"), "{path}");
                assert!(
                    text.contains(r#"{"verdict": "confirm"|"reject", "notes": "<one sentence>"}"#),
                    "{path}"
                );
            }
        }
    }

    #[test]
    fn a_theme_pack_loads_no_pass_and_does_not_fail_the_load() {
        let workspace = tempfile::tempdir().unwrap();
        write_core_pack(workspace.path()).unwrap();
        let theme = workspace.path().join(".hippocamp/packs/calm");
        write(
            &theme.join("pack.toml"),
            "protocol = \"HIPPOCAMP_V1\"\nname = \"calm\"\nkind = \"theme\"\n",
        );
        write(&theme.join("theme.css"), ":root { --hp-text: #000000; }\n");
        write(&theme.join("theme.json"), "{}");
        let passes = load_passes(workspace.path()).unwrap();
        assert_eq!(passes.len(), 2, "only core's passes: {passes:?}");
        assert!(passes.iter().all(|(path, _)| !path.starts_with(&theme)));
    }

    #[test]
    fn passes_are_checked_when_loaded() {
        let workspace = tempfile::tempdir().unwrap();
        write_core_pack(workspace.path()).unwrap();
        let mine = workspace.path().join(".hippocamp/packs/mine");
        write(&mine.join("prompts/p.md"), "Notes: {{notes}}\n");
        write(&mine.join("personas/r.md"), "# Reviewer\n");
        let pass = |prompt: &str, reads: &str| {
            format!(
                "protocol = \"HIPPOCAMP_V1\"\nprompt = \"{prompt}\"\nreads = [{reads}]\n\
                 reviewer = \"personas/r.md\"\nper_note = true\n"
            )
        };
        let load = |text: String| {
            write(&mine.join("passes/a.toml"), &text);
            load_passes(workspace.path())
        };

        // A good pass loads after core's two, in lexical order.
        let passes = load(pass("prompts/p.md", "\"notes\"")).unwrap();
        assert_eq!(passes.len(), 3);
        assert!(passes[2].0.ends_with("mine/passes/a.toml"));

        // A placeholder outside reads, an unknown placeholder, an unknown
        // read, a missing or escaping file, and a wrong protocol all fail.
        write(&mine.join("prompts/turns.md"), "{{notes}} and {{turns}}\n");
        let err = load(pass("prompts/turns.md", "\"notes\"")).unwrap_err();
        assert!(err.to_string().contains("{{turns}}"), "{err}");
        write(&mine.join("prompts/odd.md"), "{{memories}}\n");
        let err = load(pass("prompts/odd.md", "\"notes\"")).unwrap_err();
        assert!(
            err.to_string().contains("unknown placeholder {{memories}}"),
            "{err}"
        );
        let err = load(pass("prompts/p.md", "\"notes\", \"transcripts\"")).unwrap_err();
        assert!(matches!(err, Error::Toml(_)), "{err}");
        let err = load(pass("prompts/none.md", "\"notes\"")).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
        let err = load(pass("../../hippocamp.toml", "\"notes\"")).unwrap_err();
        assert!(
            err.to_string().contains("not a path inside the pack"),
            "{err}"
        );
        let err = load(pass("prompts/p.md", "\"notes\"").replace("HIPPOCAMP_V1", "HIPPOCAMP_V0"))
            .unwrap_err();
        assert!(matches!(err, Error::Protocol { .. }), "{err}");
    }

    #[cfg(unix)]
    fn pack_with_pass(workspace: &Path, name: &str) -> PathBuf {
        let pack = workspace.join(".hippocamp/packs").join(name);
        write(&pack.join("prompts/p.md"), "Notes: {{notes}}\n");
        write(&pack.join("personas/r.md"), "# Reviewer\n");
        write(
            &pack.join("passes/a.toml"),
            "protocol = \"HIPPOCAMP_V1\"\nprompt = \"prompts/p.md\"\nreads = [\"notes\"]\n\
             reviewer = \"personas/r.md\"\nper_note = true\n",
        );
        pack
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_in_a_pack_are_refused() {
        use std::os::unix::fs::symlink;
        let refused = |err: Error, path: &Path| {
            let want = format!(
                "{}: a symbolic link; packs may not contain links",
                path.display()
            );
            assert!(matches!(err, Error::Pack(_)), "{err}");
            assert_eq!(err.to_string(), want);
        };

        // A pack directory that is a link.
        let workspace = tempfile::tempdir().unwrap();
        let real = workspace.path().join("elsewhere");
        let pack = pack_with_pass(workspace.path(), "mine");
        fs::rename(&pack, &real).unwrap();
        symlink(&real, &pack).unwrap();
        refused(load_passes(workspace.path()).unwrap_err(), &pack);

        // A pass file that is a link.
        let workspace = tempfile::tempdir().unwrap();
        let pack = pack_with_pass(workspace.path(), "mine");
        let pass = pack.join("passes/a.toml");
        let moved = workspace.path().join("a.toml");
        fs::rename(&pass, &moved).unwrap();
        symlink(&moved, &pass).unwrap();
        refused(load_passes(workspace.path()).unwrap_err(), &pass);

        // A prompt and a persona that are links.
        for file in ["prompts/p.md", "personas/r.md"] {
            let workspace = tempfile::tempdir().unwrap();
            let pack = pack_with_pass(workspace.path(), "mine");
            let path = pack.join(file);
            let moved = workspace.path().join("moved");
            fs::rename(&path, &moved).unwrap();
            symlink(&moved, &path).unwrap();
            refused(load_passes(workspace.path()).unwrap_err(), &path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_link_below_the_pack_is_refused() {
        use std::os::unix::fs::symlink;
        let workspace = tempfile::tempdir().unwrap();
        let pack = pack_with_pass(workspace.path(), "mine");
        let outside = workspace.path().join("outside");
        write(&outside.join("r.md"), "# Reviewer\n");
        symlink(&outside, pack.join("linked")).unwrap();
        let toml = pack.join("passes/a.toml");
        let text = fs::read_to_string(&toml).unwrap();
        fs::write(&toml, text.replace("personas/r.md", "linked/r.md")).unwrap();
        let err = load_passes(workspace.path()).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "{}: a symbolic link; packs may not contain links",
                pack.join("linked").display()
            )
        );
    }

    #[test]
    fn a_prompt_or_persona_over_64_kib_is_refused() {
        let workspace = tempfile::tempdir().unwrap();
        write_core_pack(workspace.path()).unwrap();
        let persona = workspace
            .path()
            .join(".hippocamp/packs/core/personas/verify-reviewer.md");
        let limit = MAX_FILE_BYTES as usize;
        fs::write(&persona, "a".repeat(limit)).unwrap();
        assert_eq!(load_passes(workspace.path()).unwrap().len(), 2);
        fs::write(&persona, "a".repeat(limit + 1)).unwrap();
        let err = load_passes(workspace.path()).unwrap_err();
        assert!(matches!(err, Error::Pack(_)), "{err}");
        assert_eq!(
            err.to_string(),
            format!(
                "{}: 65537 bytes, over the 65536-byte limit",
                persona.display()
            )
        );
    }

    #[test]
    fn two_passes_with_one_name_are_refused() {
        let workspace = tempfile::tempdir().unwrap();
        let pass = "protocol = \"HIPPOCAMP_V1\"\nprompt = \"prompts/p.md\"\nreads = [\"notes\"]\n\
                    reviewer = \"personas/r.md\"\nper_note = true\n";
        for pack in ["one", "two"] {
            let dir = workspace.path().join(".hippocamp/packs").join(pack);
            write(&dir.join("prompts/p.md"), "Notes: {{notes}}\n");
            write(&dir.join("personas/r.md"), "# Reviewer\n");
            write(&dir.join("passes/x.toml"), pass);
        }
        let err = load_passes(workspace.path()).unwrap_err();
        let (a, b) = (
            workspace.path().join(".hippocamp/packs/one/passes/x.toml"),
            workspace.path().join(".hippocamp/packs/two/passes/x.toml"),
        );
        assert!(matches!(err, Error::Pack(_)), "{err}");
        assert_eq!(
            err.to_string(),
            format!("duplicate pass name x: {} and {}", a.display(), b.display())
        );
    }

    #[test]
    fn a_workspace_pass_with_a_bad_filter_is_refused() {
        let workspace = tempfile::tempdir().unwrap();
        let dir = workspace.path().join(".hippocamp/packs/mine");
        write(&dir.join("prompts/p.md"), "Notes: {{notes}}\n");
        write(&dir.join("personas/r.md"), "# Reviewer\n");
        let pass = "protocol = \"HIPPOCAMP_V1\"\nprompt = \"prompts/p.md\"\nreads = [\"notes\"]\n\
                    reviewer = \"personas/r.md\"\nper_note = true\n";
        write(
            &dir.join("passes/x.toml"),
            &format!("{pass}note_paths = []\n"),
        );
        let err = load_passes(workspace.path()).unwrap_err();
        assert!(
            err.to_string()
                .contains("note_paths: needs 1 to 20 patterns"),
            "{err}"
        );
        write(
            &dir.join("passes/x.toml"),
            &format!("{pass}note_types = [\"log\"]\n"),
        );
        assert_eq!(load_passes(workspace.path()).unwrap().len(), 1);
    }

    #[test]
    fn no_packs_means_no_passes() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(load_passes(workspace.path()).unwrap().is_empty());
    }
}

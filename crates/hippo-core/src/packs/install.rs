//! Installing, listing, updating and removing packs: `hippo add` and
//! `hippo packs list|diff|update|remove`.
//!
//! Every write goes through a hidden temporary directory under
//! `.hippo/packs/` (which the loader skips) and `fs::rename`, with files at
//! mode 0644 on Unix. Nothing is written before the pack review confirms.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use time::OffsetDateTime;
use ulid::Ulid;

use super::escape;
use super::files::{self, pack_hash, PackFiles};
use super::manifest::{self, is_pack_name, Kind};
use super::origin::{Origin, OriginSource, RegistryReviewRef, ReviewRef};
use super::registry::{self, Index, IndexPack};
use super::review::{self, PackReview, ReviewRequest, UpdateContext};
use super::source::{self, check_git_source, run_git, Fetched, GitOptions, Source};
use super::validate::{self, pass_line, Validation};
use crate::config::{Config, Workspace};
use crate::error::{Error, Result};
use crate::records::{Pass, Verdict};
use crate::util::{hex, sha256};

fn pack_err(message: impl Into<String>) -> Error {
    Error::Pack(message.into())
}

/// `text` as one escaped line.
fn one_line(text: &str) -> String {
    escape(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn packs_dir(ws: &Workspace) -> PathBuf {
    ws.hippo_dir().join("packs")
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn file_hash(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

fn verdict_word(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Confirm => "confirm",
        Verdict::Reject => "reject",
        Verdict::Revert => "revert",
    }
}

/// The default timeout of git calls the install code makes itself.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------
// The dream job's binary
// ---------------------------------------------------------------------

/// On macOS, a warning when the dream job's program is not `exe`: an older
/// hippo there would run packs without their note filters and pass
/// reviewer. `None` elsewhere, for a missing or unparsable plist, and for
/// the same binary.
pub fn schedule_warning(launch_agents: &Path, exe: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};

        let plist = launch_agents.join("dev.hippo.dream.plist");
        if !plist.is_file() {
            return None;
        }
        let output = Command::new("plutil")
            .args(["-extract", "ProgramArguments.0", "raw", "-o", "-"])
            .arg(&plist)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let program = text.strip_suffix('\n').unwrap_or(&text);
        if program.is_empty() {
            return None;
        }
        let program = PathBuf::from(program);
        let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if canonical(&program) == canonical(exe) {
            return None;
        }
        Some(format!(
            "the dream job runs {}, not this hippo ({}); an older hippo there runs packs without their note filters and pass reviewer",
            escape(&program.display().to_string()),
            escape(&exe.display().to_string())
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (launch_agents, exe);
        None
    }
}

// ---------------------------------------------------------------------
// Interrupted updates
// ---------------------------------------------------------------------

/// Finishes an interrupted update: a `.hippo/packs/.old-<name>-<ulid>/`
/// directory with no `.hippo/packs/<name>/` is renamed back to `<name>`,
/// and any other `.add-`, `.update-` or `.old-` directory is removed.
/// Returns the lines to print.
pub fn recover(ws: &Workspace) -> Result<Vec<String>> {
    let packs = packs_dir(ws);
    let entries = match fs::read_dir(&packs) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io_at(&packs, e)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io_at(&packs, e))?;
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    names.sort();
    let mut lines = Vec::new();
    for name in names {
        let interrupted = [".add-", ".update-", ".old-"]
            .iter()
            .any(|prefix| name.starts_with(prefix));
        let path = packs.join(&name);
        if !interrupted || !is_real_dir(&path) {
            continue;
        }
        if let Some((pack, id)) = name
            .strip_prefix(".old-")
            .and_then(|rest| rest.rsplit_once('-'))
        {
            let target = packs.join(pack);
            if id.parse::<Ulid>().is_ok() && is_pack_name(pack) && !exists(&target) {
                fs::rename(&path, &target).map_err(|e| Error::io_at(&path, e))?;
                lines.push(format!("restored {pack} from an interrupted update"));
                continue;
            }
        }
        fs::remove_dir_all(&path).map_err(|e| Error::io_at(&path, e))?;
    }
    Ok(lines)
}

// ---------------------------------------------------------------------
// Reading a source
// ---------------------------------------------------------------------

/// A pack read from a source, with what the origin and the review record.
#[derive(Debug, Clone)]
pub struct Incoming {
    pub fetched: Fetched,
    pub source: OriginSource,
    /// Printed on the summary's `source` line and the review's `source`:
    /// the source in the form `hippo add` takes it.
    pub review_source: String,
    /// The index entry of a registry install.
    pub registry_entry: Option<IndexPack>,
}

fn git_text(url: &str, path: &str, git_ref: Option<&str>) -> String {
    let mut text = url.to_owned();
    if !path.is_empty() {
        text.push_str("//");
        text.push_str(path);
    }
    if let Some(r) = git_ref {
        text.push('@');
        text.push_str(r);
    }
    text
}

/// Reads a registry pack: looks the name up in the index at `index_path`,
/// checks the entry against the rules of a registry entry (the index is
/// outside text), fetches `<repo>//<path>@<commit>`, refuses content whose
/// `pack_hash` is not the entry's and a `pack.toml` that names another pack.
fn fetch_registry(name: &str, index_path: &Path, git: &GitOptions) -> Result<Incoming> {
    let index = Index::load(index_path)?;
    let index_abs = fs::canonicalize(index_path).map_err(|e| Error::io_at(index_path, e))?;
    let Some(entry) = index.find(name) else {
        let mut message = format!("no pack {name} in {}", index_abs.display());
        let near: Vec<String> = registry::suggest(&index, name)
            .into_iter()
            .take(5)
            .map(|n| escape(&n))
            .collect();
        if !near.is_empty() {
            message.push_str(&format!("\ndid you mean {}?", near.join(", ")));
        }
        return Err(pack_err(message));
    };
    entry.check(git.allow_file).map_err(|why| {
        pack_err(format!(
            "{name}: the index's entry for it is refused ({})",
            escape(&why)
        ))
    })?;
    let fetched = source::fetch_git(&entry.repo, &entry.path, Some(&entry.commit), git)?;
    let got = pack_hash(&fetched.files);
    if got != entry.pack_hash {
        return Err(pack_err(format!(
            "{name}: the fetched content hashes to {got}, not the index's {}",
            entry.pack_hash
        )));
    }
    let named = fetched
        .files
        .get("pack.toml")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| manifest::parse(text).ok())
        .map(|m| m.name);
    if let Some(other) = named.filter(|other| other != name) {
        return Err(pack_err(format!(
            "{name}: the fetched pack.toml names the pack {}, not {name}",
            escape(&other)
        )));
    }
    Ok(Incoming {
        fetched,
        source: OriginSource::Registry {
            index: index_abs,
            name: name.to_owned(),
            repo: entry.repo.clone(),
            repo_id: entry.repo_id,
            owner_id: entry.owner_id,
            path: entry.path.clone(),
        },
        review_source: git_text(&entry.repo, &entry.path, None),
        registry_entry: Some(entry.clone()),
    })
}

/// Reads the source. A path inside `<workspace>/.hippo/`, compared as
/// canonical paths, is refused. A registry name needs `registry`, the
/// index file.
pub fn fetch(
    ws: &Workspace,
    source: &Source,
    registry: Option<&Path>,
    git: &GitOptions,
) -> Result<Incoming> {
    match source {
        Source::Path(path) => {
            let canonical = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            if canonical(path).starts_with(canonical(&ws.hippo_dir())) {
                return Err(pack_err(format!(
                    "{} is inside .hippo/; copy the pack from somewhere else",
                    path.display()
                )));
            }
            let fetched = source::read_path(path)?;
            Ok(Incoming {
                fetched,
                source: OriginSource::Path { path: path.clone() },
                review_source: path.display().to_string(),
                registry_entry: None,
            })
        }
        Source::Git { url, path, git_ref } => {
            let fetched = source::fetch_git(url, path, git_ref.as_deref(), git)?;
            Ok(Incoming {
                fetched,
                source: OriginSource::Git {
                    url: url.clone(),
                    path: path.clone(),
                    git_ref: git_ref.clone(),
                },
                review_source: git_text(url, path, git_ref.as_deref()),
                registry_entry: None,
            })
        }
        Source::Registry { name } => {
            let Some(index) = registry else {
                return Err(pack_err(
                    "a registry name needs --registry <path to index.json>",
                ));
            };
            fetch_registry(name, index, git)
        }
    }
}

// ---------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------

#[cfg(unix)]
fn set_mode_0644(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
fn set_mode_0644(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Writes every file of `files` under `dir`, each at mode 0644. A file
/// that exists is an error.
fn write_files(dir: &Path, files: &PackFiles) -> Result<()> {
    use std::io::Write as _;
    for (path, bytes) in files.iter() {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| Error::io_at(&target, e))?;
        file.write_all(bytes)
            .map_err(|e| Error::io_at(&target, e))?;
        drop(file);
        set_mode_0644(&target).map_err(|e| Error::io_at(&target, e))?;
    }
    Ok(())
}

/// Creates `dir` holding `files` and `origin.json`. On an error the
/// directory is removed.
fn write_pack_dir(dir: &Path, files: &PackFiles, origin: &Origin) -> Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
    }
    let written = fs::create_dir(dir)
        .map_err(|e| Error::io_at(dir, e))
        .and_then(|()| write_files(dir, files))
        .and_then(|()| origin.save(dir))
        .and_then(|()| {
            let path = dir.join("origin.json");
            set_mode_0644(&path).map_err(|e| Error::io_at(&path, e))
        });
    if written.is_err() {
        let _ = fs::remove_dir_all(dir);
    }
    written
}

fn hashes(files: &PackFiles) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|(path, bytes)| (path.to_owned(), file_hash(bytes)))
        .collect()
}

fn origin_of(
    manifest_version: &str,
    name: &str,
    files: &PackFiles,
    incoming: &Incoming,
    record: PackReview,
    added: OffsetDateTime,
) -> Origin {
    Origin {
        name: name.to_owned(),
        version: manifest_version.to_owned(),
        source: incoming.source.clone(),
        commit: incoming.fetched.commit.clone(),
        pack_hash: pack_hash(files),
        files: hashes(files),
        review: ReviewRef {
            id: record.id.clone(),
            verdict: record.verdict,
            at: record.at,
            record,
        },
        registry_review: incoming.registry_entry.as_ref().map(|e| RegistryReviewRef {
            id: e.review.id.clone(),
            verdict: e.review.verdict,
            at: e.review.at,
        }),
        added,
        updated: OffsetDateTime::now_utc(),
    }
}

// ---------------------------------------------------------------------
// Pass names
// ---------------------------------------------------------------------

/// Pass name to the pack that holds it, for every visible pack directory
/// but `skip`.
fn pass_owners(packs: &Path, skip: Option<&str>) -> Result<BTreeMap<String, String>> {
    let mut owners = BTreeMap::new();
    for pack in visible_dirs(packs)? {
        if Some(pack.as_str()) == skip {
            continue;
        }
        let passes = packs.join(&pack).join("passes");
        if !is_real_dir(&passes) {
            continue;
        }
        let mut stems = Vec::new();
        for entry in fs::read_dir(&passes).map_err(|e| Error::io_at(&passes, e))? {
            let entry = entry.map_err(|e| Error::io_at(&passes, e))?;
            let path = entry.path();
            let visible = !entry.file_name().as_encoded_bytes().starts_with(b".");
            let is_toml = path.extension().is_some_and(|ext| ext == "toml");
            if visible && is_toml && fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    stems.push(stem.to_owned());
                }
            }
        }
        stems.sort();
        for stem in stems {
            owners.entry(stem).or_insert_with(|| pack.clone());
        }
    }
    Ok(owners)
}

/// The names of the visible directories in `packs`, sorted; none when
/// `packs` is missing.
fn visible_dirs(packs: &Path) -> Result<Vec<String>> {
    let entries = match fs::read_dir(packs) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io_at(packs, e)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io_at(packs, e))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !name.starts_with('.') && is_real_dir(&entry.path()) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

// ---------------------------------------------------------------------
// hippo add
// ---------------------------------------------------------------------

/// What `hippo add` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddOutcome {
    /// The pack directory written.
    Written(PathBuf),
    DryRun,
    Refused,
}

/// The lines to print, what happened, and the review when one ran.
#[derive(Debug, Clone)]
pub struct AddReport {
    pub lines: Vec<String>,
    pub outcome: AddOutcome,
    pub review: Option<PackReview>,
}

fn refused(lines: Vec<String>, review: Option<PackReview>) -> Result<AddReport> {
    Ok(AddReport {
        lines,
        outcome: AddOutcome::Refused,
        review,
    })
}

/// The install summary: pack, source, passes, templates, size, warnings.
fn summary(name: &str, validation: &Validation, incoming: &Incoming) -> Vec<String> {
    let files = &incoming.fetched.files;
    let mut lines = Vec::new();
    if let Some(manifest) = &validation.manifest {
        lines.push(format!(
            "pack {name} {}: {}",
            escape(&manifest.version),
            one_line(&manifest.description)
        ));
    }
    let mut source = format!("source {}", escape(&incoming.review_source));
    if let Some(commit) = &incoming.fetched.commit {
        source.push_str(&format!(" commit {commit}"));
    }
    lines.push(source);
    for (pass, parsed) in &validation.passes {
        lines.push(escape(&pass_line(pass, parsed)));
    }
    if let Some(manifest) = &validation.manifest {
        lines.extend(validate::theme_lines(files, manifest));
    }
    for path in files.paths().filter(|p| p.starts_with("templates/")) {
        lines.push(format!("template {}", escape(path)));
    }
    lines.push(format!(
        "files {}, {} bytes",
        files.len(),
        files.total_bytes()
    ));
    for warning in &validation.warnings {
        lines.push(format!("warning {}", escape(warning)));
    }
    lines
}

fn review_lines(record: &PackReview) -> Vec<String> {
    let mut lines = vec![format!(
        "review {} {}: {}",
        record.id,
        verdict_word(record.verdict),
        one_line(&record.notes)
    )];
    for finding in &record.findings {
        lines.push(format!("finding {}", one_line(finding)));
    }
    lines
}

/// The argv a pack review runs: `reviewer_command_override` when set, else
/// `model_command`.
fn review_command(cfg: &Config) -> Vec<String> {
    cfg.reviewer_command_override
        .clone()
        .unwrap_or_else(|| cfg.model_command.clone())
}

fn run_review(cfg: &Config, request: &ReviewRequest) -> Result<PackReview> {
    review::review(&cfg.home, &review_command(cfg), request)
        .map_err(|e| pack_err(format!("the review could not run: {e}")))
}

/// Install steps 4 to 9: validate, refuse an existing pack or pass name,
/// print the summary, stop for a dry run, review, and write. A validation
/// error, an existing pack or pass name, and a `reject` give
/// [`AddOutcome::Refused`] with their lines and write nothing. A model error
/// is an `Err`.
pub fn add(cfg: &Config, ws: &Workspace, incoming: Incoming, dry_run: bool) -> Result<AddReport> {
    let files = &incoming.fetched.files;
    let mut lines: Vec<String> = incoming
        .fetched
        .skipped
        .iter()
        .map(|path| format!("skipped {}", escape(path)))
        .collect();
    let validation = validate::validate(files);
    if !validation.is_ok() {
        lines.extend(
            validation
                .errors
                .iter()
                .map(|e| format!("error {}", escape(e))),
        );
        return refused(lines, None);
    }
    let Some(manifest) = &validation.manifest else {
        return Err(pack_err("pack.toml: missing"));
    };
    let name = manifest.name.clone();
    let version = manifest.version.clone();

    let packs = packs_dir(ws);
    let mut refusals = Vec::new();
    if exists(&packs.join(&name)) {
        refusals.push(format!(
            "pack {name} exists; hippo packs update {name} updates a pack hippo added, or remove the directory first"
        ));
    }
    let owners = pass_owners(&packs, Some(&name))?;
    for (pass, _) in &validation.passes {
        if let Some(other) = owners.get(pass) {
            refusals.push(format!("pass name {pass} is already used by pack {other}"));
        }
    }
    if !refusals.is_empty() {
        lines.extend(refusals);
        return refused(lines, None);
    }

    lines.extend(summary(&name, &validation, &incoming));
    if dry_run {
        lines.push("dry run: nothing written".to_owned());
        return Ok(AddReport {
            lines,
            outcome: AddOutcome::DryRun,
            review: None,
        });
    }

    let request = ReviewRequest {
        files,
        validation: &validation,
        source: &incoming.review_source,
        commit: incoming.fetched.commit.as_deref(),
        repo_id: incoming.registry_entry.as_ref().map(|e| e.repo_id),
        update: None,
    };
    let record = run_review(cfg, &request)?;
    lines.extend(review_lines(&record));
    if let Some(entry) = &incoming.registry_entry {
        lines.push(format!(
            "registry review {} {} {}",
            escape(&entry.review.id),
            verdict_word(entry.review.verdict),
            entry.review.at.date()
        ));
    }
    if record.verdict != Verdict::Confirm {
        return refused(lines, Some(record));
    }

    let id = Ulid::generate();
    let temp = packs.join(format!(".add-{name}-{id}"));
    let target = packs.join(&name);
    let origin = origin_of(
        &version,
        &name,
        files,
        &incoming,
        record.clone(),
        OffsetDateTime::now_utc(),
    );
    write_pack_dir(&temp, files, &origin)?;
    if exists(&target) {
        let _ = fs::remove_dir_all(&temp);
        return Err(pack_err(format!("pack {name} exists")));
    }
    if let Err(e) = fs::rename(&temp, &target) {
        let _ = fs::remove_dir_all(&temp);
        return Err(Error::io_at(&target, e));
    }
    lines.push(format!("wrote .hippo/packs/{name}/"));
    if manifest.kind == Kind::Passes {
        lines.push(format!(
            "its passes first run at the next hippo dream ({} local time on the schedule)",
            cfg.dream_at
        ));
    }
    Ok(AddReport {
        lines,
        outcome: AddOutcome::Written(target),
        review: Some(record),
    })
}

// ---------------------------------------------------------------------
// hippo packs list
// ---------------------------------------------------------------------

/// Where an installed pack stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackState {
    /// `core`, without an `origin.json`.
    Builtin,
    /// Any other pack without an `origin.json`.
    Local,
    /// Every file matches the origin's `files`.
    Clean,
    /// This many files changed, added or removed since.
    Edited(usize),
    /// A file or the origin cannot be read; the reason, on one line.
    Unreadable(String),
}

impl fmt::Display for PackState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackState::Builtin => f.write_str("builtin"),
            PackState::Local => f.write_str("local"),
            PackState::Clean => f.write_str("clean"),
            PackState::Edited(n) => write!(f, "edited:{n}"),
            PackState::Unreadable(why) => write!(f, "unreadable: {}", one_line(why)),
        }
    }
}

/// One installed pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedPack {
    pub name: String,
    /// From `pack.toml` when it parses.
    pub version: Option<String>,
    pub state: PackState,
    /// `git:<url>//<path>@<commit>`, `path:<path>` or `registry:<name>@<commit>`.
    pub source: Option<String>,
}

fn source_text(origin: &Origin) -> String {
    let commit = origin.commit.as_deref();
    match &origin.source {
        OriginSource::Git { url, path, .. } => {
            let mut text = format!("git:{url}");
            if !path.is_empty() {
                text.push_str("//");
                text.push_str(path);
            }
            if let Some(commit) = commit {
                text.push('@');
                text.push_str(commit);
            }
            text
        }
        OriginSource::Path { path } => format!("path:{}", path.display()),
        OriginSource::Registry { name, .. } => {
            format!("registry:{name}@{}", commit.unwrap_or("-"))
        }
    }
}

/// One installed pack's state, version and source.
fn read_listed(dir: &Path, name: &str) -> Result<(Option<String>, PackState, Option<String>)> {
    let (files, _) = files::read_dir(dir)?;
    let version = files
        .get("pack.toml")
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|text| super::manifest::parse(text).ok())
        .map(|m| m.version);
    let origin = Origin::load(dir)?;
    let (state, source) = match &origin {
        None if name == "core" => (PackState::Builtin, None),
        None => (PackState::Local, None),
        Some(origin) => {
            let now = hashes(&files);
            let paths: BTreeSet<&String> = now.keys().chain(origin.files.keys()).collect();
            let edited = paths
                .into_iter()
                .filter(|path| now.get(*path) != origin.files.get(*path))
                .count();
            let state = if edited == 0 {
                PackState::Clean
            } else {
                PackState::Edited(edited)
            };
            (state, Some(source_text(origin)))
        }
    };
    Ok((version, state, source))
}

/// Every visible directory under `.hippo/packs/`, in name order. A pack that
/// cannot be read (a link, a file too big, a broken `origin.json`) is listed
/// as `unreadable: <why>`; the others are listed as usual.
pub fn list(ws: &Workspace) -> Result<Vec<ListedPack>> {
    let packs = packs_dir(ws);
    let mut listed = Vec::new();
    for name in visible_dirs(&packs)? {
        let dir = packs.join(&name);
        let (version, state, source) = match read_listed(&dir, &name) {
            Ok(read) => read,
            Err(err) => (None, PackState::Unreadable(err.to_string()), None),
        };
        listed.push(ListedPack {
            name,
            version,
            state,
            source,
        });
    }
    Ok(listed)
}

// ---------------------------------------------------------------------
// hippo packs diff and update
// ---------------------------------------------------------------------

/// What an update does to one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Upstream changed the file: write it.
    Take,
    /// Upstream added a file.
    Add,
    /// Upstream removed the file.
    Delete,
    /// Only the workspace changed it: leave it.
    Keep,
    /// Both changed it, differently.
    Conflict,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Action::Take => "take",
            Action::Add => "add",
            Action::Delete => "delete",
            Action::Keep => "keep",
            Action::Conflict => "conflict",
        })
    }
}

/// One change to a pass, as a line, and whether it widens what the pass
/// reads or works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassChange {
    pub line: String,
    pub widening: bool,
}

/// What `update` and `diff` take besides the name.
#[derive(Debug, Clone, Default)]
pub struct UpdateOptions {
    /// Another ref of a git source.
    pub git_ref: Option<String>,
    /// The index file of a registry source, in place of the recorded one.
    pub registry: Option<PathBuf>,
}

/// What `update` is allowed to do.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    pub keep_local: bool,
    pub accept_reads: bool,
    /// Accept a registry entry that now names another repository or owner.
    pub accept_origin: bool,
}

/// What an update would do, with every reason to refuse it.
#[derive(Debug, Clone)]
pub struct UpdatePlan {
    pub name: String,
    pub header: String,
    pub up_to_date: bool,
    /// Each path that is not left alone, in path order.
    pub actions: Vec<(Action, String)>,
    pub pass_changes: Vec<PassChange>,
    /// Paths changed both here and upstream, differently.
    pub conflicts: Vec<String>,
    /// `pass name <n> is already used by pack <other>`, one per clash.
    pub name_clashes: Vec<String>,
    /// What validation finds in the result.
    pub result_errors: Vec<String>,
    /// The pack as it would be written.
    pub result: PackFiles,
    pub incoming: Incoming,
    /// For a registry source whose entry now names another repository or
    /// owner: `<repo> (repository id <r>, owner id <o>), not <old repo>
    /// (<old r>, <old o>)`.
    pub origin_moved: Option<String>,
    /// `git diff --no-index` of the installed pack against upstream.
    pub diff: String,
    /// The hash of each installed file when the plan was made; the update
    /// refuses to replace a pack that no longer matches.
    pub local_hashes: BTreeMap<String, String>,
    /// Hidden entries in the installed pack (dotfiles, anything `read_dir`
    /// skips but `origin.json`): an update would delete them.
    pub hidden: Vec<String>,
}

impl UpdatePlan {
    /// The plan as lines: the header, one `<action> <path>` per action and
    /// one line per pass change.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![escape(&self.header)];
        if let Some(moved) = &self.origin_moved {
            lines.push(format!(
                "origin moved: the registry entry now names {}",
                escape(moved)
            ));
        }
        lines.extend(self.action_lines());
        lines.extend(self.pass_changes.iter().map(|c| c.line.clone()));
        lines
    }

    fn action_lines(&self) -> Vec<String> {
        self.actions
            .iter()
            .map(|(action, path)| format!("{action} {}", escape(path)))
            .collect()
    }
}

fn parse_passes(files: &PackFiles) -> BTreeMap<String, Pass> {
    let mut passes = BTreeMap::new();
    for (path, bytes) in files.iter() {
        let Some(name) = path
            .strip_prefix("passes/")
            .and_then(|n| n.strip_suffix(".toml"))
        else {
            continue;
        };
        if name.contains('/') {
            continue;
        }
        if let Some(pass) = std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| Pass::from_toml(text).ok())
        {
            passes.insert(name.to_owned(), pass);
        }
    }
    passes
}

fn list_text(items: &[String]) -> String {
    items.join(", ")
}

fn optional_text(items: &Option<Vec<String>>) -> String {
    items
        .as_ref()
        .map_or_else(|| "none".to_owned(), |list| list_text(list))
}

/// Whether the new list lets in a type or pattern the old one did not
/// (an absent list lets in everything).
fn filter_widens(old: &Option<Vec<String>>, new: &Option<Vec<String>>) -> bool {
    match (old, new) {
        (Some(_), None) => true,
        (Some(old), Some(new)) => new.iter().any(|item| !old.contains(item)),
        _ => false,
    }
}

fn same_set(a: &[String], b: &[String]) -> bool {
    a.iter().collect::<BTreeSet<_>>() == b.iter().collect::<BTreeSet<_>>()
}

fn same_filter(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same_set(a, b),
        _ => false,
    }
}

fn pass_changes(old: &BTreeMap<String, Pass>, new: &BTreeMap<String, Pass>) -> Vec<PassChange> {
    let names: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    let mut changes = Vec::new();
    for name in names {
        let mut push = |text: String, widening: bool| {
            changes.push(PassChange {
                line: format!("pass {}: {}", escape(name), escape(&text)),
                widening,
            });
        };
        match (old.get(name), new.get(name)) {
            (None, Some(pass)) => {
                let line = pass_line(name, pass);
                let rest = line
                    .strip_prefix(&format!("pass {name}: "))
                    .unwrap_or(&line)
                    .to_owned();
                push(format!("new pass; {rest}"), true);
            }
            (Some(_), None) => push("removed".to_owned(), false),
            (Some(old), Some(new)) => {
                let reads = |p: &Pass| -> Vec<String> {
                    p.reads.iter().map(|i| i.as_str().to_owned()).collect()
                };
                let (old_reads, new_reads) = (reads(old), reads(new));
                if !same_set(&old_reads, &new_reads) {
                    let widening = new_reads.iter().any(|r| !old_reads.contains(r));
                    push(
                        format!(
                            "reads {} -> {}",
                            list_text(&old_reads),
                            list_text(&new_reads)
                        ),
                        widening,
                    );
                }
                if old.per_note != new.per_note {
                    push(
                        format!("per_note {} -> {}", old.per_note, new.per_note),
                        old.per_note && !new.per_note,
                    );
                }
                if !same_filter(&old.note_types, &new.note_types) {
                    push(
                        format!(
                            "note_types {} -> {}",
                            optional_text(&old.note_types),
                            optional_text(&new.note_types)
                        ),
                        filter_widens(&old.note_types, &new.note_types),
                    );
                }
                if !same_filter(&old.note_paths, &new.note_paths) {
                    push(
                        format!(
                            "note_paths {} -> {}",
                            optional_text(&old.note_paths),
                            optional_text(&new.note_paths)
                        ),
                        filter_widens(&old.note_paths, &new.note_paths),
                    );
                }
                if old.reviewer != new.reviewer {
                    push(
                        format!(
                            "reviewer {} -> {}",
                            old.reviewer.display(),
                            new.reviewer.display()
                        ),
                        true,
                    );
                }
                if old.input_budget_bytes != new.input_budget_bytes {
                    push(
                        format!(
                            "input_budget_bytes {} -> {}",
                            old.input_budget_bytes, new.input_budget_bytes
                        ),
                        false,
                    );
                }
            }
            (None, None) => {}
        }
    }
    changes
}

/// A skipped entry an update may replace or delete: the origin record, and
/// Finder's `.DS_Store` files, which hold nothing of the pack's.
fn update_may_delete(path: &str) -> bool {
    path == "origin.json" || path == ".DS_Store" || path.ends_with("/.DS_Store")
}

/// Update steps 1, 3, 4, 5 and 7: reads `origin.json` and the source,
/// plans every path, compares the passes and validates the result. It
/// records every reason to refuse and refuses nothing, so `hippo packs
/// diff` can print a plan whatever it holds. Errors: a pack without
/// `origin.json`, a source that is gone, an upstream pack that does not
/// validate, an upstream `name` other than `name`, and a registry entry
/// that is gone. A registry entry that names another repository or owner
/// is recorded in `origin_moved`.
pub fn plan_update(
    ws: &Workspace,
    name: &str,
    options: &UpdateOptions,
    git: &GitOptions,
) -> Result<UpdatePlan> {
    if !is_pack_name(name) {
        return Err(pack_err(format!("{name} is not a pack name")));
    }
    let dir = packs_dir(ws).join(name);
    if !is_real_dir(&dir) {
        return Err(pack_err(format!("no pack {name} in .hippo/packs/")));
    }
    let Some(origin) = Origin::load(&dir)? else {
        return Err(pack_err(format!(
            "{name} has no origin; it is your own source"
        )));
    };
    let mut registry_old = None;
    let source = match &origin.source {
        OriginSource::Git { url, path, git_ref } => {
            // The origin is a file on disk and `--ref` a word from the
            // command line: neither skips the rules of a typed source.
            let git_ref = options.git_ref.clone().or_else(|| git_ref.clone());
            check_git_source(url, path, git_ref.as_deref(), git.allow_file).map_err(|why| {
                pack_err(format!(
                    "{name}: its recorded git source is refused ({why})"
                ))
            })?;
            Source::Git {
                url: url.clone(),
                path: path.clone(),
                git_ref,
            }
        }
        OriginSource::Path { path } => {
            if options.git_ref.is_some() {
                return Err(pack_err(format!(
                    "{name} came from a path; --ref applies only to a git source"
                )));
            }
            if !fs::metadata(path).is_ok_and(|m| m.is_dir()) {
                return Err(pack_err(format!(
                    "source {} is not a directory here",
                    path.display()
                )));
            }
            Source::Path(path.clone())
        }
        OriginSource::Registry {
            index,
            name: listed,
            repo,
            repo_id,
            owner_id,
            ..
        } => {
            if options.git_ref.is_some() {
                return Err(pack_err(format!(
                    "{name} came from a registry; --ref applies only to a git source"
                )));
            }
            if !is_pack_name(listed) {
                return Err(pack_err(format!(
                    "{name}: its recorded registry name is not a pack name"
                )));
            }
            registry_old = Some((index.clone(), repo.clone(), *repo_id, *owner_id));
            Source::Registry {
                name: listed.clone(),
            }
        }
    };
    let registry = options
        .registry
        .clone()
        .or_else(|| registry_old.as_ref().map(|(index, ..)| index.clone()));
    let incoming = fetch(ws, &source, registry.as_deref(), git)?;
    let origin_moved = match (&registry_old, &incoming.source) {
        (
            Some((_, old_repo, old_repo_id, old_owner_id)),
            OriginSource::Registry {
                repo,
                repo_id,
                owner_id,
                ..
            },
        ) if repo != old_repo || repo_id != old_repo_id || owner_id != old_owner_id => Some(format!(
            "{repo} (repository id {repo_id}, owner id {owner_id}), not {old_repo} ({old_repo_id}, {old_owner_id})"
        )),
        _ => None,
    };
    let upstream = &incoming.fetched.files;
    let upstream_validation = validate::validate(upstream);
    if !upstream_validation.is_ok() {
        let mut message = String::from("upstream pack does not validate:");
        for error in &upstream_validation.errors {
            message.push_str(&format!("\n{}", escape(error)));
        }
        return Err(pack_err(message));
    }
    let Some(manifest) = &upstream_validation.manifest else {
        return Err(pack_err("upstream pack: pack.toml is missing"));
    };
    if manifest.name != name {
        return Err(pack_err(format!(
            "upstream pack is named {}, not {name}",
            escape(&manifest.name)
        )));
    }

    let (local, local_skipped) = files::read_dir(&dir)?;
    let local_hashes = hashes(&local);
    // Any hidden entry an update may not delete stops it.
    let hidden: Vec<String> = local_skipped
        .into_iter()
        .filter(|path| !update_may_delete(path))
        .collect();
    let commit_text = |commit: &Option<String>| commit.clone().unwrap_or_else(|| "-".to_owned());
    let header = format!(
        "{name} {} -> {}, commit {} -> {}",
        origin.version,
        manifest.version,
        commit_text(&origin.commit),
        commit_text(&incoming.fetched.commit)
    );
    if pack_hash(upstream) == origin.pack_hash && origin_moved.is_none() {
        return Ok(UpdatePlan {
            name: name.to_owned(),
            header,
            up_to_date: true,
            actions: Vec::new(),
            pass_changes: Vec::new(),
            conflicts: Vec::new(),
            name_clashes: Vec::new(),
            result_errors: Vec::new(),
            result: local,
            incoming,
            origin_moved,
            diff: String::new(),
            local_hashes,
            hidden,
        });
    }

    let mut paths: BTreeSet<&str> = origin.files.keys().map(String::as_str).collect();
    paths.extend(local.paths());
    paths.extend(upstream.paths());
    let mut result = PackFiles::default();
    let mut actions = Vec::new();
    let mut conflicts = Vec::new();
    for path in paths {
        let base = origin.files.get(path).cloned();
        let mine = local.get(path);
        let theirs = upstream.get(path);
        let mine_hash = mine.map(file_hash);
        let theirs_hash = theirs.map(file_hash);
        let (action, chosen) = if mine_hash == base {
            if theirs_hash == base {
                (None, mine)
            } else {
                let action = if theirs.is_none() {
                    Action::Delete
                } else if mine.is_none() {
                    Action::Add
                } else {
                    Action::Take
                };
                (Some(action), theirs)
            }
        } else if theirs_hash == base {
            (Some(Action::Keep), mine)
        } else if mine_hash == theirs_hash {
            (None, mine)
        } else {
            conflicts.push(path.to_owned());
            (Some(Action::Conflict), mine)
        };
        if let Some(action) = action {
            actions.push((action, path.to_owned()));
        }
        if let Some(bytes) = chosen {
            result.insert(path.to_owned(), bytes.to_vec());
        }
    }

    let old_passes = parse_passes(&local);
    let new_passes = parse_passes(&result);
    let changes = pass_changes(&old_passes, &new_passes);
    let owners = pass_owners(&packs_dir(ws), Some(name))?;
    let name_clashes = new_passes
        .keys()
        .filter(|pass| !old_passes.contains_key(*pass))
        .filter_map(|pass| {
            owners
                .get(pass)
                .map(|other| format!("pass name {pass} is already used by pack {other}"))
        })
        .collect();
    let result_errors = validate::validate(&result).errors;
    let diff = diff_text(&local, upstream)?;
    Ok(UpdatePlan {
        name: name.to_owned(),
        header,
        up_to_date: false,
        actions,
        pass_changes: changes,
        conflicts,
        name_clashes,
        result_errors,
        result,
        incoming,
        origin_moved,
        diff,
        local_hashes,
        hidden,
    })
}

/// What `update` did.
#[derive(Debug, Clone)]
pub struct UpdateReport {
    pub lines: Vec<String>,
    pub updated: bool,
}

fn not_updated(lines: Vec<String>) -> Result<UpdateReport> {
    Ok(UpdateReport {
        lines,
        updated: false,
    })
}

/// Update steps 8 and 9 after the refusals: refuses, in this order, with
/// nothing written, a registry entry that moved without `accept_origin`, a result that does not validate, a pass name another
/// pack uses, a widening without `accept_reads` and a conflict without
/// `keep_local`; then reviews the update and writes the result. An
/// up-to-date plan writes nothing and is not a refusal.
pub fn apply_update(
    cfg: &Config,
    ws: &Workspace,
    plan: UpdatePlan,
    flags: Flags,
) -> Result<UpdateReport> {
    let name = plan.name.clone();
    if let (Some(moved), false) = (&plan.origin_moved, flags.accept_origin) {
        return not_updated(vec![format!(
            "refused: the registry entry now names {}; rerun with --accept-origin to accept",
            escape(moved)
        )]);
    }
    if plan.up_to_date {
        return not_updated(vec![format!("{name} is up to date")]);
    }
    if !plan.hidden.is_empty() {
        // Hidden files are not part of a pack hippo reads, so the new
        // directory would not hold them; refuse rather than delete them.
        let shown: Vec<String> = plan.hidden.iter().map(|p| escape(p)).collect();
        return not_updated(vec![format!(
            "refused: {name} holds hidden files that an update would delete: {}; move or remove them, then run hippo packs update again",
            shown.join(", ")
        )]);
    }
    if !plan.result_errors.is_empty() {
        let mut lines = vec!["refused: the result does not validate".to_owned()];
        lines.extend(
            plan.result_errors
                .iter()
                .map(|e| format!("error {}", escape(e))),
        );
        return not_updated(lines);
    }
    if !plan.name_clashes.is_empty() {
        let lines = plan
            .name_clashes
            .iter()
            .map(|clash| format!("refused: {}", escape(clash)))
            .collect();
        return not_updated(lines);
    }
    if !flags.accept_reads && plan.pass_changes.iter().any(|c| c.widening) {
        return not_updated(vec![
            "refused: widens what a pass reads or works on; rerun with --accept-reads to accept"
                .to_owned(),
        ]);
    }
    if !flags.keep_local && !plan.conflicts.is_empty() {
        return not_updated(vec![format!(
            "refused: {} files changed both here and upstream; rerun with --keep-local to keep yours",
            plan.conflicts.len()
        )]);
    }

    let packs = packs_dir(ws);
    let dir = packs.join(&name);
    let Some(old_origin) = Origin::load(&dir)? else {
        return Err(pack_err(format!(
            "{name} has no origin; it is your own source"
        )));
    };
    let upstream = &plan.incoming.fetched.files;
    let validation = validate::validate(upstream);
    let Some(manifest) = &validation.manifest else {
        return Err(pack_err("upstream pack: pack.toml is missing"));
    };
    let mut plan_lines = vec![escape(&plan.header)];
    plan_lines.extend(plan.action_lines());
    let context = UpdateContext {
        plan: plan_lines,
        pass_changes: plan.pass_changes.iter().map(|c| c.line.clone()).collect(),
        diff: plan.diff.clone(),
    };
    let request = ReviewRequest {
        files: upstream,
        validation: &validation,
        source: &plan.incoming.review_source,
        commit: plan.incoming.fetched.commit.as_deref(),
        repo_id: plan.incoming.registry_entry.as_ref().map(|e| e.repo_id),
        update: Some(&context),
    };
    let record = run_review(cfg, &request)?;
    let mut lines = review_lines(&record);
    if let Some(entry) = &plan.incoming.registry_entry {
        lines.push(format!(
            "registry review {} {} {}",
            escape(&entry.review.id),
            verdict_word(entry.review.verdict),
            entry.review.at.date()
        ));
    }
    if record.verdict != Verdict::Confirm {
        return not_updated(lines);
    }

    let mut origin = origin_of(
        &manifest.version,
        &name,
        upstream,
        &plan.incoming,
        record,
        old_origin.added,
    );
    origin.updated = OffsetDateTime::now_utc();
    let id = Ulid::generate();
    let new_dir = packs.join(format!(".update-{name}-{id}"));
    let old_dir = packs.join(format!(".old-{name}-{id}"));
    write_pack_dir(&new_dir, &plan.result, &origin)?;
    // The review took time: an edit made meanwhile must not be replaced by a
    // result planned from the old files.
    let unchanged = match files::read_dir(&dir) {
        Ok((now, skipped)) => {
            hashes(&now) == plan.local_hashes && skipped.iter().all(|p| update_may_delete(p))
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&new_dir);
            return Err(e);
        }
    };
    if !unchanged {
        let _ = fs::remove_dir_all(&new_dir);
        return not_updated(vec![format!(
            "{name} changed during the review; run hippo packs update again"
        )]);
    }
    if let Err(e) = fs::rename(&dir, &old_dir) {
        let _ = fs::remove_dir_all(&new_dir);
        return Err(Error::io_at(&dir, e));
    }
    if let Err(e) = fs::rename(&new_dir, &dir) {
        let _ = fs::rename(&old_dir, &dir);
        let _ = fs::remove_dir_all(&new_dir);
        return Err(Error::io_at(&dir, e));
    }
    let _ = fs::remove_dir_all(&old_dir);
    lines.push(format!("updated .hippo/packs/{name}/"));
    Ok(UpdateReport {
        lines,
        updated: true,
    })
}

/// `git diff --no-index --no-prefix --no-color local upstream` of the two
/// packs, copied into a temporary directory as `local/` and `upstream/`.
/// Git's exit status 1 means the trees differ and is not an error.
pub fn diff_text(local: &PackFiles, upstream: &PackFiles) -> Result<String> {
    let tmp = tempfile::tempdir().map_err(Error::Io)?;
    for (dir, files) in [("local", local), ("upstream", upstream)] {
        let path = tmp.path().join(dir);
        fs::create_dir(&path).map_err(|e| Error::io_at(&path, e))?;
        write_files(&path, files)?;
    }
    let options = GitOptions {
        allow_file: false,
        timeout: GIT_TIMEOUT,
    };
    let output = run_git(
        Some(tmp.path()),
        &[
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-textconv",
            "--no-prefix",
            "--no-color",
            "local",
            "upstream",
        ],
        None,
        &options,
    )?;
    if output.status != 0 && output.status != 1 {
        return Err(pack_err(format!(
            "git diff failed with status {}",
            output.status
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ---------------------------------------------------------------------
// hippo packs remove
// ---------------------------------------------------------------------

/// Deletes `.hippo/packs/<name>/` and returns its path. `core` is refused;
/// so, without `force`, is a pack with no `origin.json`.
pub fn remove(ws: &Workspace, name: &str, force: bool) -> Result<PathBuf> {
    if name == "core" {
        return Err(pack_err("core is hippo's own pack; it is not removed"));
    }
    if !is_pack_name(name) {
        return Err(pack_err(format!("{name} is not a pack name")));
    }
    let dir = packs_dir(ws).join(name);
    let Ok(meta) = fs::symlink_metadata(&dir) else {
        return Err(pack_err(format!("no pack {name} in .hippo/packs/")));
    };
    if !force && Origin::load(&dir)?.is_none() {
        return Err(pack_err(format!(
            "{name} has no origin.json, so it may exist nowhere else; rerun with --force to remove it"
        )));
    }
    let removed = if meta.is_dir() {
        fs::remove_dir_all(&dir)
    } else {
        fs::remove_file(&dir)
    };
    removed.map_err(|e| Error::io_at(&dir, e))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dream::prompt::CLOSING_INSTRUCTION;
    use crate::util::serial;

    const STUB: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/stub-model/model.sh"
    );

    struct Setup {
        _home: tempfile::TempDir,
        _root: tempfile::TempDir,
        cfg: Config,
        ws: Workspace,
    }

    fn setup(verdict: &str) -> Setup {
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_path = fs::canonicalize(root.path()).unwrap();
        let cfg = Config {
            home: home.path().to_path_buf(),
            workspace: Some(root_path.clone()),
            transcript_roots: Vec::new(),
            exclude_roots: Vec::new(),
            model_command: vec!["false".into()],
            ingest_interval_minutes: 10,
            dream_at: "03:30".into(),
            reviewer_command_override: Some(
                ["env", "HIPPO_STUB_REVIEW=1"]
                    .into_iter()
                    .map(String::from)
                    .chain([format!("HIPPO_STUB_VERDICT={verdict}"), STUB.to_owned()])
                    .collect(),
            ),
        };
        let ws = Workspace {
            root: root_path,
            notes_dir: "notes".into(),
            entry_note: "notes/START HERE.md".into(),
            required_notes: Vec::new(),
            budget_bytes: crate::DEFAULT_CONTEXT_BUDGET,
            log_path: "notes/Session log.md".into(),
            sample_tasks: Vec::new(),
            passes_off: Vec::new(),
            lessons: true,
            snapshot_keep_days: None,
            log_notes: None,
        };
        Setup {
            _home: home,
            _root: root,
            cfg,
            ws,
        }
    }

    fn git() -> GitOptions {
        GitOptions {
            allow_file: false,
            timeout: Duration::from_secs(30),
        }
    }

    fn put(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn pass_toml(extra: &str) -> String {
        format!(
            "protocol = \"HIPPO_V1\"\nprompt = \"prompts/p.md\"\nreads = [\"notes\"]\n\
             reviewer = \"personas/p.md\"\nper_note = true\n{extra}"
        )
    }

    /// A valid pack named `name` with one pass, `pass`, and the template
    /// files `t0.md` to `t8.md`.
    fn write_pack(dir: &Path, name: &str, pass: &str, version: &str) {
        put(
            dir,
            "pack.toml",
            &format!(
                "protocol = \"HIPPO_V1\"\nname = \"{name}\"\nversion = \"{version}\"\n\
                 description = \"Adds terms.\"\nauthors = [\"Ada\"]\nlicense = \"MIT\"\n"
            ),
        );
        put(dir, "README.md", "A pack.\n");
        put(
            dir,
            &format!("passes/{pass}.toml"),
            &pass_toml("").replace("p.md", &format!("{pass}.md")),
        );
        put(
            dir,
            &format!("prompts/{pass}.md"),
            &format!("Notes: {{{{notes}}}}\n\n{CLOSING_INSTRUCTION}\n"),
        );
        put(
            dir,
            &format!("personas/{pass}.md"),
            "## Role\nr\n## What you receive\nw\n## Rules\nr\n## Reply\n\
             Reply with `{\"verdict\": \"confirm\"|\"reject\", \"notes\": \"n\"}`.\n",
        );
        for i in 0..=8 {
            put(
                dir,
                &format!("templates/t{i}.md"),
                &format!("template {i}\n"),
            );
        }
    }

    fn incoming(s: &Setup, dir: &Path) -> Incoming {
        let source = Source::Path(fs::canonicalize(dir).unwrap());
        fetch(&s.ws, &source, None, &git()).unwrap()
    }

    fn install(s: &Setup, dir: &Path) -> AddReport {
        add(&s.cfg, &s.ws, incoming(s, dir), false).unwrap()
    }

    fn pack_dir(s: &Setup, name: &str) -> PathBuf {
        s.ws.hippo_dir().join("packs").join(name)
    }

    fn plan(s: &Setup, name: &str) -> UpdatePlan {
        plan_update(&s.ws, name, &UpdateOptions::default(), &git()).unwrap()
    }

    fn actions(plan: &UpdatePlan) -> Vec<String> {
        plan.actions
            .iter()
            .map(|(a, p)| format!("{a} {p}"))
            .collect()
    }

    #[test]
    fn add_writes_the_pack_with_its_origin_and_modes() {
        let _guard = serial();
        let s = setup("confirm");
        let src = tempfile::tempdir().unwrap();
        write_pack(src.path(), "glossary", "glossary", "1.0.0");
        put(src.path(), ".DS_Store", "x");
        let report = install(&s, src.path());
        let target = pack_dir(&s, "glossary");
        assert_eq!(report.outcome, AddOutcome::Written(target.clone()));
        let lines = report.lines.join("\n");
        assert!(lines.contains("skipped .DS_Store"), "{lines}");
        assert!(
            lines.contains("pack glossary 1.0.0: Adds terms."),
            "{lines}"
        );
        assert!(lines.contains("template templates/t0.md"), "{lines}");
        assert!(lines.contains("files 14, "), "{lines}");
        assert!(lines.contains("review "), "{lines}");
        assert!(lines.contains(" confirm: stub"), "{lines}");
        assert!(
            lines.ends_with(
                "wrote .hippo/packs/glossary/\n\
             its passes first run at the next hippo dream (03:30 local time on the schedule)"
            ),
            "{lines}"
        );
        let origin = Origin::load(&target).unwrap().unwrap();
        assert_eq!(origin.review.verdict, Verdict::Confirm);
        assert_eq!(origin.commit, None);
        assert!(matches!(origin.source, OriginSource::Path { .. }));
        assert_eq!(origin.files.len(), 14);
        assert!(!target.join(".DS_Store").exists());
        // Nothing left behind, every file 0644.
        let hidden: Vec<_> = fs::read_dir(target.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(hidden.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for entry in walkdir::WalkDir::new(&target).into_iter().flatten() {
                if entry.file_type().is_file() {
                    let mode = entry.metadata().unwrap().permissions().mode() & 0o777;
                    assert_eq!(mode, 0o644, "{}", entry.path().display());
                }
            }
        }
        // A dry run of the same pack still refuses the existing name.
        let again = add(&s.cfg, &s.ws, incoming(&s, src.path()), true).unwrap();
        assert_eq!(again.outcome, AddOutcome::Refused);
        assert!(again
            .lines
            .iter()
            .any(|l| l.starts_with("pack glossary exists; ")));
    }

    const THEME_CSS: &str = ":root {\n  --hp-text: #000000;\n}\n";
    const THEME_JSON: &str = r##"{"name": "Calm", "author": "Ada", "themes": [{"name": "Calm Light", "appearance": "light", "style": {"text": "#111111"}}]}"##;

    fn write_theme_pack(dir: &Path, version: &str, css: &str) {
        put(
            dir,
            "pack.toml",
            &format!(
                "protocol = \"HIPPO_V1\"\nname = \"calm\"\nkind = \"theme\"\n\
                 version = \"{version}\"\ndescription = \"A calm theme.\"\n\
                 authors = [\"Ada\"]\nlicense = \"MIT\"\ntheme_contract = 1\n\
                 desktop_theme_format = 1\n"
            ),
        );
        put(dir, "README.md", "A theme.\n");
        put(dir, "LICENSE", "MIT\n");
        put(dir, "theme.css", css);
        put(dir, "theme.json", THEME_JSON);
    }

    #[test]
    fn add_prints_a_theme_packs_lines_and_writes_it() {
        let _guard = serial();
        let s = setup("confirm");
        let src = tempfile::tempdir().unwrap();
        write_theme_pack(src.path(), "1.0.0", THEME_CSS);
        let report = install(&s, src.path());
        let target = pack_dir(&s, "calm");
        assert_eq!(report.outcome, AddOutcome::Written(target.clone()));
        assert_eq!(
            report.lines[..4],
            [
                "pack calm 1.0.0: A calm theme.".to_owned(),
                format!("source {}", fs::canonicalize(src.path()).unwrap().display()),
                format!(
                    "theme web: theme.css, contract 1, {} bytes",
                    THEME_CSS.len()
                ),
                format!(
                    "theme desktop: theme.json, format 1, {} bytes",
                    THEME_JSON.len()
                ),
            ]
        );
        let lines = report.lines.join("\n");
        assert!(!lines.contains("pass "), "{lines}");
        assert!(lines.contains("files 5, "), "{lines}");
        // The first-run line is for pass packs only.
        assert!(!lines.contains("first run"), "{lines}");
        assert_eq!(report.lines.last().unwrap(), "wrote .hippo/packs/calm/");
        assert_eq!(
            fs::read_to_string(target.join("theme.css")).unwrap(),
            THEME_CSS
        );
        assert!(target.join("theme.json").is_file());
        assert!(target.join("origin.json").is_file());
        // A theme pack adds no pass: the loader finds none.
        let loaded = super::super::load_passes(&s.ws.root).unwrap();
        assert!(loaded.iter().all(|(path, _)| !path.starts_with(&target)));
    }

    #[test]
    fn a_theme_pack_updates_like_any_other() {
        let _guard = serial();
        let s = setup("confirm");
        let src = tempfile::tempdir().unwrap();
        write_theme_pack(src.path(), "1.0.0", THEME_CSS);
        assert!(matches!(
            install(&s, src.path()).outcome,
            AddOutcome::Written(_)
        ));
        let newer = ":root {\n  --hp-text: #111111;\n}\n";
        write_theme_pack(src.path(), "1.1.0", newer);
        let p = plan(&s, "calm");
        assert_eq!(actions(&p), ["take pack.toml", "take theme.css"]);
        assert!(p.pass_changes.is_empty());
        assert!(p.diff.contains("theme.css"), "{}", p.diff);
        let report = apply_update(&s.cfg, &s.ws, p, Flags::default()).unwrap();
        assert!(report.updated, "{:?}", report.lines);
        assert_eq!(
            fs::read_to_string(pack_dir(&s, "calm").join("theme.css")).unwrap(),
            newer
        );
        // An upstream theme that breaks a theme rule is refused before any plan.
        write_theme_pack(src.path(), "1.2.0", "a { color: red; }\n");
        let err = plan_update(&s.ws, "calm", &UpdateOptions::default(), &git())
            .unwrap_err()
            .to_string();
        assert!(err.contains("upstream pack does not validate"), "{err}");
        assert!(err.contains("only `:root`"), "{err}");
    }

    #[test]
    fn add_refuses_a_pass_name_another_pack_uses() {
        let _guard = serial();
        let s = setup("confirm");
        let first = tempfile::tempdir().unwrap();
        write_pack(first.path(), "one", "shared", "1.0.0");
        assert!(matches!(
            install(&s, first.path()).outcome,
            AddOutcome::Written(_)
        ));
        let second = tempfile::tempdir().unwrap();
        write_pack(second.path(), "two", "shared", "1.0.0");
        let report = install(&s, second.path());
        assert_eq!(report.outcome, AddOutcome::Refused);
        assert_eq!(
            report.lines.last().unwrap(),
            "pass name shared is already used by pack one"
        );
        assert!(!pack_dir(&s, "two").exists());
    }

    #[test]
    fn add_refuses_an_invalid_pack_and_a_dry_run_writes_and_reviews_nothing() {
        let s = setup("confirm");
        let src = tempfile::tempdir().unwrap();
        write_pack(src.path(), "glossary", "glossary", "1.0.0");
        put(src.path(), "templates/t0.md", "hidden \u{202E} text\n");
        let report = add(&s.cfg, &s.ws, incoming(&s, src.path()), false).unwrap();
        assert_eq!(report.outcome, AddOutcome::Refused);
        assert!(
            report.lines.iter().all(|l| l.starts_with("error ")),
            "{:?}",
            report.lines
        );
        assert!(!pack_dir(&s, "glossary").exists());

        // A dry run: no model (a command that cannot run would be an Err).
        let mut cfg = s.cfg.clone();
        cfg.reviewer_command_override = Some(vec!["/nonexistent/model".into()]);
        write_pack(src.path(), "glossary", "glossary", "1.0.0");
        let report = add(&cfg, &s.ws, incoming(&s, src.path()), true).unwrap();
        assert_eq!(report.outcome, AddOutcome::DryRun);
        assert_eq!(report.lines.last().unwrap(), "dry run: nothing written");
        assert!(report.review.is_none());
        assert!(!pack_dir(&s, "glossary").exists());
    }

    #[test]
    fn a_rejected_review_writes_nothing_and_a_model_error_is_an_error() {
        let _guard = serial();
        let s = setup("reject");
        let src = tempfile::tempdir().unwrap();
        write_pack(src.path(), "glossary", "glossary", "1.0.0");
        let report = install(&s, src.path());
        assert_eq!(report.outcome, AddOutcome::Refused);
        assert_eq!(report.review.unwrap().verdict, Verdict::Reject);
        assert!(report.lines.last().unwrap().contains(" reject: stub"));
        assert!(!pack_dir(&s, "glossary").exists());
        assert!(!s
            .ws
            .hippo_dir()
            .join("packs")
            .join(".add-glossary")
            .exists());

        let mut cfg = s.cfg.clone();
        cfg.reviewer_command_override = Some(vec!["false".into()]);
        let err = add(&cfg, &s.ws, incoming(&s, src.path()), false).unwrap_err();
        assert!(
            err.to_string().starts_with("the review could not run: "),
            "{err}"
        );
        assert!(!pack_dir(&s, "glossary").exists());
    }

    #[test]
    fn a_path_inside_dot_hippo_is_refused() {
        let s = setup("confirm");
        let inside = s.ws.hippo_dir().join("packs").join("x");
        fs::create_dir_all(&inside).unwrap();
        let source = Source::Path(fs::canonicalize(&inside).unwrap());
        let err = fetch(&s.ws, &source, None, &git()).unwrap_err();
        assert!(
            err.to_string()
                .ends_with("is inside .hippo/; copy the pack from somewhere else"),
            "{err}"
        );
    }

    fn have_git() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn a_pack_added_from_a_file_repository_records_its_commit() {
        if !have_git() {
            eprintln!("git is not on PATH; skipping");
            return;
        }
        let _guard = serial();
        let s = setup("confirm");
        let repo = tempfile::tempdir().unwrap();
        run(repo.path(), &["init", "--quiet", "-b", "main"]);
        run(repo.path(), &["config", "uploadpack.allowFilter", "true"]);
        run(
            repo.path(),
            &["config", "uploadpack.allowAnySHA1InWant", "true"],
        );
        write_pack(
            &repo.path().join("packs/glossary"),
            "glossary",
            "glossary",
            "1.0.0",
        );
        run(repo.path(), &["add", "."]);
        run(repo.path(), &["commit", "--quiet", "-m", "pack"]);
        let commit = run(repo.path(), &["rev-parse", "HEAD"]);
        let url = format!(
            "file://{}",
            fs::canonicalize(repo.path()).unwrap().display()
        );
        let options = GitOptions {
            allow_file: true,
            timeout: Duration::from_secs(60),
        };
        let source = Source::Git {
            url: url.clone(),
            path: "packs/glossary".into(),
            git_ref: None,
        };
        let incoming = fetch(&s.ws, &source, None, &options).unwrap();
        assert_eq!(incoming.review_source, format!("{url}//packs/glossary"));
        let report = add(&s.cfg, &s.ws, incoming, false).unwrap();
        assert!(
            matches!(report.outcome, AddOutcome::Written(_)),
            "{:?}",
            report.lines
        );
        assert!(report
            .lines
            .iter()
            .any(|l| l == &format!("source {url}//packs/glossary commit {commit}")));
        let origin = Origin::load(&pack_dir(&s, "glossary")).unwrap().unwrap();
        assert_eq!(origin.commit.as_deref(), Some(commit.as_str()));
        assert_eq!(
            origin.source,
            OriginSource::Git {
                url: url.clone(),
                path: "packs/glossary".into(),
                git_ref: None
            }
        );
        let listed = list(&s.ws).unwrap();
        assert_eq!(
            listed[0].source.as_deref(),
            Some(format!("git:{url}//packs/glossary@{commit}").as_str())
        );
    }

    /// Installs `glossary` from `src`, and returns the Setup.
    fn installed(src: &Path) -> Setup {
        let s = setup("confirm");
        write_pack(src, "glossary", "glossary", "1.0.0");
        let report = install(&s, src);
        assert!(
            matches!(report.outcome, AddOutcome::Written(_)),
            "{:?}",
            report.lines
        );
        s
    }

    #[test]
    fn each_row_of_the_plan_table() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let s = installed(src.path());
        let local = pack_dir(&s, "glossary");
        // Local edits.
        fs::write(local.join("templates/t3.md"), "mine 3\n").unwrap(); // kept
        fs::write(local.join("templates/t4.md"), "same 4\n").unwrap(); // equal
        fs::write(local.join("templates/t5.md"), "mine 5\n").unwrap(); // conflict
        fs::remove_file(local.join("templates/t6.md")).unwrap(); // gone here, changed there
        fs::write(local.join("templates/t7.md"), "mine 7\n").unwrap(); // changed both ways
        fs::remove_file(local.join("templates/t8.md")).unwrap(); // gone here only: kept
        fs::write(local.join("templates/both.md"), "mine\n").unwrap(); // added both, differently
        fs::write(local.join("templates/same.md"), "same\n").unwrap(); // added both, equal
                                                                       // Upstream edits.
        write_pack(src.path(), "glossary", "glossary", "1.1.0");
        fs::write(src.path().join("templates/t1.md"), "new 1\n").unwrap();
        fs::remove_file(src.path().join("templates/t2.md")).unwrap();
        fs::write(src.path().join("templates/t4.md"), "same 4\n").unwrap();
        fs::write(src.path().join("templates/t5.md"), "theirs 5\n").unwrap();
        fs::write(src.path().join("templates/t6.md"), "theirs 6\n").unwrap();
        fs::write(src.path().join("templates/t7.md"), "theirs 7\n").unwrap();
        put(src.path(), "templates/added.md", "added\n");
        put(src.path(), "templates/both.md", "theirs\n");
        put(src.path(), "templates/same.md", "same\n");
        let plan = plan(&s, "glossary");
        assert!(!plan.up_to_date);
        assert_eq!(plan.header, "glossary 1.0.0 -> 1.1.0, commit - -> -");
        assert_eq!(
            actions(&plan),
            [
                "take pack.toml",
                "add templates/added.md",
                "conflict templates/both.md",
                "take templates/t1.md",
                "delete templates/t2.md",
                "keep templates/t3.md",
                "conflict templates/t5.md",
                "conflict templates/t6.md",
                "conflict templates/t7.md",
                "keep templates/t8.md",
            ]
        );
        assert_eq!(
            plan.conflicts,
            [
                "templates/both.md",
                "templates/t5.md",
                "templates/t6.md",
                "templates/t7.md"
            ]
        );
        assert!(plan.result_errors.is_empty(), "{:?}", plan.result_errors);
        assert!(plan.name_clashes.is_empty());
        assert!(plan.pass_changes.is_empty());
        assert!(plan.diff.contains("local/templates/t5.md"), "{}", plan.diff);
        assert!(
            plan.diff.contains("upstream/templates/t5.md"),
            "{}",
            plan.diff
        );
        // The result: kept, taken, and conflicts as here.
        let text = |path: &str| {
            plan.result
                .get(path)
                .map(|b| String::from_utf8_lossy(b).into_owned())
        };
        assert_eq!(text("templates/t3.md").as_deref(), Some("mine 3\n"));
        assert_eq!(text("templates/t1.md").as_deref(), Some("new 1\n"));
        assert_eq!(text("templates/t2.md"), None);
        assert_eq!(text("templates/t5.md").as_deref(), Some("mine 5\n"));
        assert_eq!(text("templates/t6.md"), None);
        assert_eq!(text("templates/t8.md"), None);
        assert_eq!(text("templates/added.md").as_deref(), Some("added\n"));
        assert_eq!(text("templates/same.md").as_deref(), Some("same\n"));
        let lines = plan.lines();
        assert_eq!(lines[0], plan.header);
        assert_eq!(lines[1], "take pack.toml");
    }

    #[test]
    fn a_conflict_is_refused_then_kept_with_keep_local() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let s = installed(src.path());
        let local = pack_dir(&s, "glossary");
        fs::write(local.join("templates/t5.md"), "mine 5\n").unwrap();
        write_pack(src.path(), "glossary", "glossary", "1.1.0");
        fs::write(src.path().join("templates/t5.md"), "theirs 5\n").unwrap();
        let theirs = file_hash(b"theirs 5\n");

        let report = apply_update(&s.cfg, &s.ws, plan(&s, "glossary"), Flags::default()).unwrap();
        assert!(!report.updated);
        assert_eq!(
            report.lines,
            ["refused: 1 files changed both here and upstream; rerun with --keep-local to keep yours"]
        );
        assert_eq!(
            fs::read_to_string(local.join("templates/t5.md")).unwrap(),
            "mine 5\n"
        );

        let flags = Flags {
            keep_local: true,
            accept_reads: false,
            accept_origin: false,
        };
        let report = apply_update(&s.cfg, &s.ws, plan(&s, "glossary"), flags).unwrap();
        assert!(report.updated, "{:?}", report.lines);
        assert_eq!(
            report.lines.last().unwrap(),
            "updated .hippo/packs/glossary/"
        );
        assert_eq!(
            fs::read_to_string(local.join("templates/t5.md")).unwrap(),
            "mine 5\n"
        );
        let origin = Origin::load(&local).unwrap().unwrap();
        assert_eq!(origin.version, "1.1.0");
        assert_eq!(origin.files["templates/t5.md"], theirs);
        let listed = list(&s.ws).unwrap();
        assert_eq!(listed[0].state, PackState::Edited(1));
        assert_eq!(listed[0].version.as_deref(), Some("1.1.0"));
        // No temporary directory is left.
        assert!(visible_dirs(local.parent().unwrap()).unwrap() == ["glossary"]);
        assert!(recover(&s.ws).unwrap().is_empty());
        // The same source again is up to date.
        let again = plan(&s, "glossary");
        assert!(again.up_to_date);
        let report = apply_update(&s.cfg, &s.ws, again, Flags::default()).unwrap();
        assert_eq!(report.lines, ["glossary is up to date"]);
        assert!(!report.updated);
    }

    #[test]
    fn a_rejected_update_and_a_widening_write_nothing() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let s = installed(src.path());
        write_pack(src.path(), "glossary", "glossary", "1.1.0");
        put(
            src.path(),
            "passes/glossary.toml",
            &pass_toml("")
                .replace("p.md", "glossary.md")
                .replace("[\"notes\"]", "[\"notes\", \"turns\"]"),
        );
        let p = plan(&s, "glossary");
        assert_eq!(p.pass_changes.len(), 1);
        assert_eq!(
            p.pass_changes[0].line,
            "pass glossary: reads notes -> notes, turns"
        );
        assert!(p.pass_changes[0].widening);
        let report = apply_update(&s.cfg, &s.ws, p, Flags::default()).unwrap();
        assert!(!report.updated);
        assert_eq!(
            report.lines,
            ["refused: widens what a pass reads or works on; rerun with --accept-reads to accept"]
        );
        // Accepted, but the review rejects: still nothing written.
        let mut cfg = s.cfg.clone();
        cfg.reviewer_command_override = Some(
            [
                "env",
                "HIPPO_STUB_REVIEW=1",
                "HIPPO_STUB_VERDICT=reject",
                STUB,
            ]
            .map(String::from)
            .to_vec(),
        );
        let flags = Flags {
            keep_local: false,
            accept_reads: true,
            accept_origin: false,
        };
        let report = apply_update(&cfg, &s.ws, plan(&s, "glossary"), flags).unwrap();
        assert!(!report.updated);
        assert!(
            report.lines[0].contains(" reject: stub"),
            "{:?}",
            report.lines
        );
        let origin = Origin::load(&pack_dir(&s, "glossary")).unwrap().unwrap();
        assert_eq!(origin.version, "1.0.0");
        // Confirmed and accepted: written.
        let report = apply_update(&s.cfg, &s.ws, plan(&s, "glossary"), flags).unwrap();
        assert!(report.updated, "{:?}", report.lines);
    }

    #[test]
    fn plan_update_errors() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let s = installed(src.path());
        let opts = UpdateOptions::default();
        let err = |name: &str| {
            plan_update(&s.ws, name, &opts, &git())
                .unwrap_err()
                .to_string()
        };
        assert_eq!(err("core"), "no pack core in .hippo/packs/");
        put(&s.ws.hippo_dir(), "packs/core/pack.toml", "x");
        assert_eq!(err("core"), "core has no origin; it is your own source");
        // A source that is gone.
        let moved = src.path().with_extension("moved");
        fs::rename(src.path(), &moved).unwrap();
        assert!(
            err("glossary").ends_with("is not a directory here"),
            "{}",
            err("glossary")
        );
        fs::rename(&moved, src.path()).unwrap();
        // Upstream that does not validate, and another name.
        put(src.path(), "templates/t0.md", "bad \u{202E}\n");
        assert!(err("glossary").starts_with("upstream pack does not validate:\n"));
        write_pack(src.path(), "other", "other", "1.0.0");
        assert_eq!(
            err("glossary"),
            "upstream pack is named other, not glossary"
        );
        // --ref on a path source.
        write_pack(src.path(), "glossary", "glossary", "1.0.0");
        let with_ref = UpdateOptions {
            git_ref: Some("v2".into()),
            registry: None,
        };
        assert!(plan_update(&s.ws, "glossary", &with_ref, &git()).is_err());
        // A registry origin whose index is gone.
        let mut origin = Origin::load(&pack_dir(&s, "glossary")).unwrap().unwrap();
        origin.source = OriginSource::Registry {
            index: "/i.json".into(),
            name: "glossary".into(),
            repo: "a/b".into(),
            repo_id: 1,
            owner_id: 2,
            path: "g".into(),
        };
        origin.save(&pack_dir(&s, "glossary")).unwrap();
        assert!(err("glossary").contains("/i.json"), "{}", err("glossary"));
    }

    fn pass(extra: &str) -> Pass {
        Pass::from_toml(&pass_toml(extra)).unwrap()
    }

    fn change(old: &str, new: &str) -> Vec<PassChange> {
        let one = |extra: &str| BTreeMap::from([("p".to_owned(), pass(extra))]);
        pass_changes(&one(old), &one(new))
    }

    fn widening(old: &str, new: &str) -> bool {
        let changes = change(old, new);
        assert_eq!(changes.len(), 1, "{changes:?}");
        changes[0].widening
    }

    #[test]
    fn every_widening_kind_and_no_narrowing_is_refused() {
        let one = |p: Pass| BTreeMap::from([("p".to_owned(), p)]);
        // A new pass widens; a removed one does not.
        let none = BTreeMap::new();
        let added = pass_changes(&none, &one(pass("")));
        assert_eq!(
            added[0].line,
            "pass p: new pass; reads notes; one call per note; works on every touched note"
        );
        assert!(added[0].widening);
        let gone = pass_changes(&one(pass("")), &none);
        assert_eq!(gone[0].line, "pass p: removed");
        assert!(!gone[0].widening);

        // reads
        let mut wider = pass("");
        wider.reads.push(crate::records::Input::Turns);
        let c = pass_changes(&one(pass("")), &one(wider.clone()));
        assert_eq!(c[0].line, "pass p: reads notes -> notes, turns");
        assert!(c[0].widening);
        let c = pass_changes(&one(wider), &one(pass("")));
        assert_eq!(c[0].line, "pass p: reads notes, turns -> notes");
        assert!(!c[0].widening);

        // note_types and note_paths: removing the list, or adding a type or
        // pattern, widens; introducing a list or dropping an entry narrows.
        let types = "note_types = [\"decision\"]\n";
        let two_types = "note_types = [\"decision\", \"fact\"]\n";
        let paths = "note_paths = [\"a/**\"]\n";
        let two_paths = "note_paths = [\"a/**\", \"b/**\"]\n";
        assert!(widening(types, ""));
        assert!(widening(types, two_types));
        assert!(!widening("", types));
        assert!(!widening(two_types, types));
        assert!(widening(paths, ""));
        assert!(widening(paths, two_paths));
        assert!(!widening("", paths));
        assert!(!widening(two_paths, paths));
        assert_eq!(
            change(types, "")[0].line,
            "pass p: note_types decision -> none"
        );
        assert_eq!(change("", paths)[0].line, "pass p: note_paths none -> a/**");
        assert_eq!(
            change(two_types, types)[0].line,
            "pass p: note_types decision, fact -> decision"
        );

        // per_note: true to false widens, false to true does not.
        let mut one_call = pass("");
        one_call.per_note = false;
        let c = pass_changes(&one(pass("")), &one(one_call.clone()));
        assert_eq!(c[0].line, "pass p: per_note true -> false");
        assert!(c[0].widening);
        let c = pass_changes(&one(one_call), &one(pass("")));
        assert_eq!(c[0].line, "pass p: per_note false -> true");
        assert!(!c[0].widening);

        // A different reviewer path widens, whatever the new path is.
        let mut other = pass("");
        other.reviewer = "personas/other.md".into();
        let c = pass_changes(&one(pass("")), &one(other));
        assert_eq!(
            c[0].line,
            "pass p: reviewer personas/p.md -> personas/other.md"
        );
        assert!(c[0].widening);

        // A budget change is printed and never refused.
        let mut budget = pass("");
        budget.input_budget_bytes = 1000;
        let c = pass_changes(&one(pass("")), &one(budget));
        assert_eq!(c[0].line, "pass p: input_budget_bytes 200000 -> 1000");
        assert!(!c[0].widening);

        // An unchanged pass is no change; a reordered list is none either.
        assert!(change(two_types, "note_types = [\"fact\", \"decision\"]\n").is_empty());
    }

    #[test]
    fn an_interrupted_update_is_restored() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let s = installed(src.path());
        let packs = s.ws.hippo_dir().join("packs");
        let id = Ulid::generate();
        fs::rename(
            packs.join("glossary"),
            packs.join(format!(".old-glossary-{id}")),
        )
        .unwrap();
        fs::create_dir_all(packs.join(".update-glossary-x")).unwrap();
        fs::create_dir_all(packs.join(format!(".add-other-{id}"))).unwrap();
        // An .old- directory beside a pack directory is only removed.
        fs::create_dir_all(packs.join("core")).unwrap();
        fs::create_dir_all(packs.join(format!(".old-core-{id}"))).unwrap();
        let lines = recover(&s.ws).unwrap();
        assert_eq!(lines, ["restored glossary from an interrupted update"]);
        assert!(packs.join("glossary/origin.json").is_file());
        let mut names: Vec<String> = fs::read_dir(&packs)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["core", "glossary"]);
        assert!(recover(&s.ws).unwrap().is_empty());
    }

    #[test]
    fn list_states_and_remove_rules() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let s = installed(src.path());
        put(&s.ws.hippo_dir(), "packs/core/passes/x.toml", "x");
        put(&s.ws.hippo_dir(), "packs/mine/pack.toml", "not a manifest");
        let local = pack_dir(&s, "glossary");
        let states = |s: &Setup| -> Vec<(String, String, Option<String>)> {
            list(&s.ws)
                .unwrap()
                .into_iter()
                .map(|p| (p.name, p.state.to_string(), p.version))
                .collect()
        };
        let clean = states(&s);
        assert_eq!(clean[0], ("core".into(), "builtin".into(), None));
        assert_eq!(
            clean[1],
            ("glossary".into(), "clean".into(), Some("1.0.0".into()))
        );
        assert_eq!(clean[2], ("mine".into(), "local".into(), None));
        fs::write(
            local.join("README.md"),
            "edited
",
        )
        .unwrap();
        assert_eq!(states(&s)[1].1, "edited:1");
        put(&local, ".DS_Store", "x");
        assert_eq!(states(&s)[1].1, "edited:1");
        fs::remove_file(local.join("templates/t0.md")).unwrap();
        put(&local, "templates/new.md", "n\n");
        assert_eq!(states(&s)[1].1, "edited:3");

        let err = |name: &str, force: bool| remove(&s.ws, name, force).unwrap_err().to_string();
        assert_eq!(
            err("core", true),
            "core is hippo's own pack; it is not removed"
        );
        assert_eq!(
            err("mine", false),
            "mine has no origin.json, so it may exist nowhere else; rerun with --force to remove it"
        );
        assert!(pack_dir(&s, "mine").exists());
        assert_eq!(remove(&s.ws, "mine", true).unwrap(), pack_dir(&s, "mine"));
        assert!(!pack_dir(&s, "mine").exists());
        remove(&s.ws, "glossary", false).unwrap();
        assert!(!local.exists());
        assert!(err("glossary", false).starts_with("no pack glossary"));
    }

    #[test]
    fn diff_text_labels_local_and_upstream() {
        let _guard = serial();
        let mut a = PackFiles::default();
        a.insert("x.md".into(), b"one\n".to_vec());
        let mut b = PackFiles::default();
        b.insert("x.md".into(), b"two\n".to_vec());
        let diff = diff_text(&a, &b).unwrap();
        assert!(diff.contains("--- local/x.md"), "{diff}");
        assert!(diff.contains("+++ upstream/x.md"), "{diff}");
        assert_eq!(diff_text(&a, &a).unwrap(), "");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn schedule_warning_names_another_binary_only() {
        let _guard = serial();
        let agents = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let exe = bin.path().join("hippo");
        fs::write(&exe, "x").unwrap();
        let plist = |program: &Path| {
            fs::write(
                agents.path().join("dev.hippo.dream.plist"),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                     <key>ProgramArguments</key><array><string>{}</string><string>dream</string></array>\
                     </dict></plist>\n",
                    program.display()
                ),
            )
            .unwrap();
        };
        // No plist, an unparsable one.
        assert_eq!(schedule_warning(agents.path(), &exe), None);
        fs::write(agents.path().join("dev.hippo.dream.plist"), "garbage").unwrap();
        assert_eq!(schedule_warning(agents.path(), &exe), None);
        // Another binary.
        let other = bin.path().join("old-hippo");
        plist(&other);
        let text = schedule_warning(agents.path(), &exe).unwrap();
        assert_eq!(
            text,
            format!(
                "the dream job runs {}, not this hippo ({}); an older hippo there runs packs without their note filters and pass reviewer",
                other.display(),
                exe.display()
            )
        );
        // The same binary, through a link.
        plist(&exe);
        assert_eq!(schedule_warning(agents.path(), &exe), None);
        let link = bin.path().join("link");
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        plist(&link);
        assert_eq!(schedule_warning(agents.path(), &exe), None);
    }

    #[test]
    fn escape_hides_controls_and_bidirectional_marks() {
        assert_eq!(
            escape("a\nb\u{1b}[2J\u{202e}"),
            "a\\u{a}b\\u{1b}[2J\\u{202e}"
        );
        assert_eq!(one_line("a \n  b"), "a b");
    }
    // -- registry installs and updates --------------------------------

    fn git_cmd(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(["-c", "commit.gpgsign=false", "-C"])
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn file_git() -> GitOptions {
        GitOptions {
            allow_file: true,
            timeout: Duration::from_secs(60),
        }
    }

    /// A repository holding the pack at its root, and its only commit.
    fn pack_repo(name: &str, version: &str) -> (tempfile::TempDir, String) {
        let repo = tempfile::tempdir().unwrap();
        git_cmd(repo.path(), &["init", "--quiet", "-b", "main"]);
        git_cmd(repo.path(), &["config", "uploadpack.allowFilter", "true"]);
        git_cmd(
            repo.path(),
            &["config", "uploadpack.allowAnySHA1InWant", "true"],
        );
        write_pack(repo.path(), name, name, version);
        git_cmd(repo.path(), &["add", "-A"]);
        git_cmd(repo.path(), &["commit", "--quiet", "-m", "pack"]);
        let commit = git_cmd(repo.path(), &["rev-parse", "HEAD"]);
        (repo, commit)
    }

    /// Writes `<dir>/<file>`, an index that lists `name` as the repository
    /// holds it, with the given repository id and hash.
    fn write_index(
        dir: &Path,
        file: &str,
        name: &str,
        repo: &Path,
        commit: &str,
        repo_id: u64,
        hash: Option<&str>,
    ) -> PathBuf {
        let (files, _) = files::read_dir(repo).unwrap();
        let hash = hash.map_or_else(|| pack_hash(&files), str::to_owned);
        let index = serde_json::json!({
            "protocol": "HIPPO_V1",
            "generated": "2026-11-08T11:00:00Z",
            "counts": {"listed": 1, "first_party": 0, "other_owners": 1},
            "packs": [{
                "name": name, "version": "1.0.0", "description": "Adds terms.",
                "authors": ["Ada"], "owner": "alice", "maintainers": ["alice"],
                "first_party": false, "license": "MIT",
                "repo": format!("file://{}", repo.display()),
                "repo_id": repo_id, "owner_id": 7, "path": "", "commit": commit,
                "pack_hash": hash, "kind": "passes", "passes": [], "files": [],
                "review": {"id": "01JCA5ZB3K9V7Q2M4N6P8R0T1W", "verdict": "confirm",
                           "at": "2026-11-08T11:00:00Z"}
            }]
        });
        let path = dir.join(file);
        fs::write(&path, serde_json::to_vec_pretty(&index).unwrap()).unwrap();
        path
    }

    fn registry_source(name: &str) -> Source {
        Source::Registry { name: name.into() }
    }

    #[test]
    fn a_registry_add_records_both_reviews_and_the_registry_source() {
        let _guard = serial();
        let s = setup("confirm");
        let (repo, commit) = pack_repo("glossary", "1.0.0");
        let idx = tempfile::tempdir().unwrap();
        let index = write_index(
            idx.path(),
            "index.json",
            "glossary",
            repo.path(),
            &commit,
            11,
            None,
        );
        let incoming = fetch(
            &s.ws,
            &registry_source("glossary"),
            Some(&index),
            &file_git(),
        )
        .unwrap();
        assert_eq!(incoming.fetched.commit.as_deref(), Some(commit.as_str()));
        assert_eq!(
            incoming.review_source,
            format!("file://{}", repo.path().display())
        );
        let report = add(&s.cfg, &s.ws, incoming, false).unwrap();
        assert!(
            matches!(report.outcome, AddOutcome::Written(_)),
            "{:?}",
            report.lines
        );
        let lines = report.lines.join("\n");
        assert!(
            lines.contains("registry review 01JCA5ZB3K9V7Q2M4N6P8R0T1W confirm 2026-11-08"),
            "{lines}"
        );
        assert!(lines.contains(&format!(" commit {commit}")), "{lines}");
        let origin = Origin::load(&pack_dir(&s, "glossary")).unwrap().unwrap();
        assert_eq!(origin.commit.as_deref(), Some(commit.as_str()));
        let registry_review = origin.registry_review.unwrap();
        assert_eq!(registry_review.id, "01JCA5ZB3K9V7Q2M4N6P8R0T1W");
        assert_eq!(registry_review.verdict, Verdict::Confirm);
        assert_eq!(origin.review.record.repo_id, Some(11));
        assert_eq!(
            origin.review.record.commit.as_deref(),
            Some(commit.as_str())
        );
        match origin.source {
            OriginSource::Registry {
                index: recorded,
                name,
                repo_id,
                owner_id,
                path,
                ..
            } => {
                assert_eq!(recorded, fs::canonicalize(&index).unwrap());
                assert_eq!(
                    (name.as_str(), repo_id, owner_id, path.as_str()),
                    ("glossary", 11, 7, "")
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_registry_name_needs_an_index_and_a_hash_that_matches() {
        let _guard = serial();
        let s = setup("confirm");
        let err = fetch(&s.ws, &registry_source("glossary"), None, &file_git()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "a registry name needs --registry <path to index.json>"
        );

        let (repo, commit) = pack_repo("glossary", "1.0.0");
        let idx = tempfile::tempdir().unwrap();
        let wrong = "0".repeat(64);
        let index = write_index(
            idx.path(),
            "index.json",
            "glossary",
            repo.path(),
            &commit,
            11,
            Some(&wrong),
        );
        let err = fetch(
            &s.ws,
            &registry_source("glossary"),
            Some(&index),
            &file_git(),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(
            text.starts_with("glossary: the fetched content hashes to ")
                && text.ends_with(&format!(", not the index's {wrong}")),
            "{text}"
        );
        assert!(!pack_dir(&s, "glossary").exists());
        assert!(
            !s.ws.hippo_dir().join("packs").exists()
                || visible_dirs(&packs_dir(&s.ws)).unwrap().is_empty()
        );

        // The CLI's options refuse file:// entries.
        let err = fetch(&s.ws, &registry_source("glossary"), Some(&index), &git()).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("the index's entry for it is refused (repo:"),
            "{text}"
        );
    }

    #[test]
    fn an_unknown_registry_name_prints_suggestions() {
        let _guard = serial();
        let s = setup("confirm");
        let (repo, commit) = pack_repo("glossary", "1.0.0");
        let idx = tempfile::tempdir().unwrap();
        let index = write_index(
            idx.path(),
            "index.json",
            "glossary",
            repo.path(),
            &commit,
            11,
            None,
        );
        let err = fetch(
            &s.ws,
            &registry_source("glosary"),
            Some(&index),
            &file_git(),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("no pack glosary in "), "{text}");
        assert!(text.ends_with("\ndid you mean glossary?"), "{text}");
        let err = fetch(
            &s.ws,
            &registry_source("unrelated"),
            Some(&index),
            &file_git(),
        )
        .unwrap_err();
        assert!(!err.to_string().contains("did you mean"), "{err}");
    }

    #[test]
    fn a_registry_update_refuses_a_moved_origin_until_it_is_accepted() {
        let _guard = serial();
        let s = setup("confirm");
        let (repo, commit) = pack_repo("glossary", "1.0.0");
        let idx = tempfile::tempdir().unwrap();
        let index = write_index(
            idx.path(),
            "index.json",
            "glossary",
            repo.path(),
            &commit,
            11,
            None,
        );
        let incoming = fetch(
            &s.ws,
            &registry_source("glossary"),
            Some(&index),
            &file_git(),
        )
        .unwrap();
        add(&s.cfg, &s.ws, incoming, false).unwrap();

        // Unchanged entry: up to date, and no origin has moved.
        let same = plan_update(&s.ws, "glossary", &UpdateOptions::default(), &file_git()).unwrap();
        assert!(same.up_to_date && same.origin_moved.is_none());

        // The entry now names another repository id.
        let moved = write_index(
            idx.path(),
            "moved.json",
            "glossary",
            repo.path(),
            &commit,
            12,
            None,
        );
        let options = UpdateOptions {
            git_ref: None,
            registry: Some(moved.clone()),
        };
        let plan = plan_update(&s.ws, "glossary", &options, &file_git()).unwrap();
        let url = format!("file://{}", repo.path().display());
        assert_eq!(
            plan.origin_moved.as_deref(),
            Some(format!("{url} (repository id 12, owner id 7), not {url} (11, 7)").as_str())
        );
        let refused = apply_update(&s.cfg, &s.ws, plan, Flags::default()).unwrap();
        assert!(!refused.updated);
        assert_eq!(
            refused.lines,
            [format!(
                "refused: the registry entry now names {url} (repository id 12, owner id 7), not {url} (11, 7); rerun with --accept-origin to accept"
            )]
        );
        let before = Origin::load(&pack_dir(&s, "glossary")).unwrap().unwrap();
        assert!(matches!(
            before.source,
            OriginSource::Registry { repo_id: 11, .. }
        ));

        let plan = plan_update(&s.ws, "glossary", &options, &file_git()).unwrap();
        let flags = Flags {
            accept_origin: true,
            ..Flags::default()
        };
        let done = apply_update(&s.cfg, &s.ws, plan, flags).unwrap();
        assert!(done.updated, "{:?}", done.lines);
        assert!(
            done.lines.iter().any(|l| l.starts_with("registry review ")),
            "{:?}",
            done.lines
        );
        let after = Origin::load(&pack_dir(&s, "glossary")).unwrap().unwrap();
        match after.source {
            OriginSource::Registry {
                index: recorded,
                repo_id,
                ..
            } => {
                assert_eq!(repo_id, 12);
                assert_eq!(recorded, fs::canonicalize(&moved).unwrap());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(after.review.record.repo_id, Some(12));
        assert!(after.registry_review.is_some());

        // A gone entry is refused, and --ref does not apply.
        let empty = idx.path().join("empty.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&moved).unwrap()).unwrap();
        value["packs"] = serde_json::json!([]);
        fs::write(&empty, serde_json::to_vec(&value).unwrap()).unwrap();
        let gone = UpdateOptions {
            git_ref: None,
            registry: Some(empty),
        };
        let err = plan_update(&s.ws, "glossary", &gone, &file_git()).unwrap_err();
        assert!(err.to_string().starts_with("no pack glossary in "), "{err}");
        let with_ref = UpdateOptions {
            git_ref: Some("v2".into()),
            registry: None,
        };
        assert!(plan_update(&s.ws, "glossary", &with_ref, &file_git()).is_err());
    }

    // -- hostile input -------------------------------------------------

    /// Rewrites the one pack of the index at `path` with `change`.
    fn edit_index(path: &Path, change: impl FnOnce(&mut serde_json::Value)) {
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        change(&mut value["packs"][0]);
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    #[test]
    fn an_index_entry_is_checked_before_git_sees_it() {
        let _guard = serial();
        let s = setup("confirm");
        let (repo, commit) = pack_repo("glossary", "1.0.0");
        let idx = tempfile::tempdir().unwrap();
        let index = write_index(
            idx.path(),
            "index.json",
            "glossary",
            repo.path(),
            &commit,
            11,
            None,
        );
        let try_fetch = |index: &Path| {
            fetch(
                &s.ws,
                &registry_source("glossary"),
                Some(index),
                &file_git(),
            )
            .unwrap_err()
            .to_string()
        };
        // A refspec instead of a commit would reach `git fetch`.
        edit_index(&index, |p| {
            p["commit"] = "refs/heads/main:refs/heads/x".into()
        });
        assert!(try_fetch(&index).contains("refused (commit:"), "commit");
        edit_index(&index, |p| p["commit"] = commit.clone().into());
        edit_index(&index, |p| p["pack_hash"] = "ABC".into());
        assert!(try_fetch(&index).contains("refused (pack_hash:"), "hash");
        let (files, _) = files::read_dir(repo.path()).unwrap();
        edit_index(&index, |p| p["pack_hash"] = pack_hash(&files).into());
        edit_index(&index, |p| p["path"] = "../x".into());
        assert!(try_fetch(&index).contains("refused (path:"), "path");
        edit_index(&index, |p| p["path"] = "".into());
        edit_index(&index, |p| p["repo"] = "file://-x\nmore".into());
        assert!(try_fetch(&index).contains("refused (repo:"), "repo");
        edit_index(&index, |p| {
            p["repo"] = format!("file://{}", repo.path().display()).into()
        });
        // The entry as it was is fine again.
        assert!(fetch(
            &s.ws,
            &registry_source("glossary"),
            Some(&index),
            &file_git()
        )
        .is_ok());
        assert!(!pack_dir(&s, "glossary").exists());
    }

    #[test]
    fn a_pack_whose_pack_toml_names_another_pack_is_not_installed_under_the_listed_name() {
        let _guard = serial();
        let s = setup("confirm");
        let (repo, commit) = pack_repo("bar", "1.0.0");
        let idx = tempfile::tempdir().unwrap();
        // The index lists `foo`, and the hash is right, but the pack is `bar`.
        let index = write_index(
            idx.path(),
            "index.json",
            "foo",
            repo.path(),
            &commit,
            11,
            None,
        );
        let err = fetch(&s.ws, &registry_source("foo"), Some(&index), &file_git())
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "foo: the fetched pack.toml names the pack bar, not foo"
        );
        assert!(visible_dirs(&packs_dir(&s.ws)).unwrap().is_empty());
    }

    #[test]
    fn index_text_is_escaped_in_what_is_printed() {
        let _guard = serial();
        let s = setup("confirm");
        let (repo, commit) = pack_repo("glossary", "1.0.0");
        let idx = tempfile::tempdir().unwrap();
        let index = write_index(
            idx.path(),
            "index.json",
            "glossary",
            repo.path(),
            &commit,
            11,
            None,
        );
        // A near name with a terminal escape, in "did you mean".
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        let mut other = value["packs"][0].clone();
        other["name"] = "glossar\u{1b}".into();
        value["packs"].as_array_mut().unwrap().push(other);
        fs::write(&index, serde_json::to_vec(&value).unwrap()).unwrap();
        let err = fetch(
            &s.ws,
            &registry_source("glossarz"),
            Some(&index),
            &file_git(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("did you mean glossar\\u{1b}, glossary"),
            "{err:?}"
        );
        assert!(!err.contains('\u{1b}'), "{err:?}");

        // A review id with a terminal escape, on the `registry review` line.
        edit_index(&index, |p| p["review"]["id"] = "01\u{1b}[2J\nfake".into());
        let incoming = fetch(
            &s.ws,
            &registry_source("glossary"),
            Some(&index),
            &file_git(),
        )
        .unwrap();
        let report = add(&s.cfg, &s.ws, incoming, false).unwrap();
        let lines = report.lines.join("\n");
        assert!(!lines.contains('\u{1b}'), "{lines:?}");
        assert!(
            lines
                .lines()
                .any(|l| l.starts_with("registry review 01\\u{1b}[2J\\u{a}fake confirm")),
            "{lines}"
        );
    }

    /// A reviewer that edits `target` while it runs, then confirms.
    fn editing_reviewer(dir: &Path, target: &Path) -> Vec<String> {
        let script = dir.join("review.sh");
        fs::write(
            &script,
            "#!/bin/sh\ncat >/dev/null\necho edited-meanwhile >> \"$1\"\n\
             printf '%s\\n' '{\"result\":\"{\\\"verdict\\\":\\\"confirm\\\",\\\"notes\\\":\\\"ok\\\"}\"}'\n",
        )
        .unwrap();
        vec![
            "sh".into(),
            script.display().to_string(),
            target.display().to_string(),
        ]
    }

    #[test]
    fn an_edit_made_during_the_review_is_not_replaced() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let mut s = installed(src.path());
        write_pack(src.path(), "glossary", "glossary", "1.1.0");
        fs::write(src.path().join("templates/t1.md"), "new 1\n").unwrap();
        let local = pack_dir(&s, "glossary");
        let first = plan(&s, "glossary");
        let tools = tempfile::tempdir().unwrap();
        s.cfg.reviewer_command_override =
            Some(editing_reviewer(tools.path(), &local.join("README.md")));
        let report = apply_update(&s.cfg, &s.ws, first, Flags::default()).unwrap();
        assert!(!report.updated, "{:?}", report.lines);
        assert_eq!(
            report.lines,
            ["glossary changed during the review; run hippo packs update again"]
        );
        assert_eq!(
            fs::read_to_string(local.join("README.md")).unwrap(),
            "A pack.\nedited-meanwhile\n"
        );
        assert_eq!(
            fs::read_to_string(local.join("templates/t1.md")).unwrap(),
            "template 1\n"
        );
        assert_eq!(visible_dirs(&packs_dir(&s.ws)).unwrap(), ["glossary"]);
        let leftovers: Vec<String> = fs::read_dir(packs_dir(&s.ws))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn an_update_refuses_to_delete_hidden_files() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let s = installed(src.path());
        let local = pack_dir(&s, "glossary");
        put(&local, ".notes/todo.txt", "keep me\n");
        put(&local, ".envrc", "x\n");
        write_pack(src.path(), "glossary", "glossary", "1.1.0");
        fs::write(src.path().join("templates/t1.md"), "new 1\n").unwrap();
        let first = plan(&s, "glossary");
        assert_eq!(first.hidden, [".envrc", ".notes"]);
        let report = apply_update(&s.cfg, &s.ws, first, Flags::default()).unwrap();
        assert!(!report.updated);
        assert_eq!(
            report.lines,
            ["refused: glossary holds hidden files that an update would delete: .envrc, .notes; move or remove them, then run hippo packs update again"]
        );
        assert!(local.join(".notes/todo.txt").exists());
        assert_eq!(
            fs::read_to_string(local.join("templates/t1.md")).unwrap(),
            "template 1\n"
        );
        // Without hidden files the same update goes through; Finder's
        // `.DS_Store` files do not count.
        fs::remove_dir_all(local.join(".notes")).unwrap();
        fs::remove_file(local.join(".envrc")).unwrap();
        put(&local, ".DS_Store", "x\n");
        put(&local, "templates/.DS_Store", "x\n");
        let again = plan(&s, "glossary");
        assert!(again.hidden.is_empty());
        let report = apply_update(&s.cfg, &s.ws, again, Flags::default()).unwrap();
        assert!(report.updated, "{:?}", report.lines);
    }

    #[test]
    fn a_recorded_git_source_and_a_ref_obey_the_rules_of_a_typed_one() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let s = installed(src.path());
        let local = pack_dir(&s, "glossary");
        let mut origin = Origin::load(&local).unwrap().unwrap();
        let set = |url: &str, path: &str, git_ref: Option<&str>| {
            let mut origin = origin.clone();
            origin.source = OriginSource::Git {
                url: url.into(),
                path: path.into(),
                git_ref: git_ref.map(str::to_owned),
            };
            origin.save(&local).unwrap();
        };
        let refused = |options: &UpdateOptions| {
            plan_update(&s.ws, "glossary", options, &git())
                .unwrap_err()
                .to_string()
        };
        let none = UpdateOptions::default();
        set("https://user@example.invalid/a/b", "", None);
        assert!(refused(&none).contains("its recorded git source is refused"));
        set("https://example.invalid/a/b", "../x", None);
        assert!(refused(&none).contains("its recorded git source is refused"));
        set("https://example.invalid/a/b", "", Some("--upload-pack=x"));
        assert!(refused(&none).contains("its recorded git source is refused"));
        set("file:///tmp/x", "", None);
        assert!(refused(&none).contains("its recorded git source is refused"));
        // `--ref` is checked as well, before any fetch.
        set("https://example.invalid/a/b", "", None);
        let bad_ref = UpdateOptions {
            git_ref: Some("a..b".into()),
            registry: None,
        };
        assert!(refused(&bad_ref).contains("the ref is not a branch, tag or commit name"));
        origin.source = OriginSource::Registry {
            index: PathBuf::from("/nonexistent/index.json"),
            name: "Not A Name".into(),
            repo: "https://github.com/a/b".into(),
            repo_id: 1,
            owner_id: 2,
            path: String::new(),
        };
        origin.save(&local).unwrap();
        assert!(refused(&none).contains("recorded registry name is not a pack name"));
    }

    #[cfg(unix)]
    #[test]
    fn list_reports_an_unreadable_pack_and_goes_on() {
        let _guard = serial();
        let src = tempfile::tempdir().unwrap();
        let s = installed(src.path());
        put(&s.ws.hippo_dir(), "packs/zeta/pack.toml", "x");
        let zeta = pack_dir(&s, "zeta");
        std::os::unix::fs::symlink("/etc/hosts", zeta.join("link.md")).unwrap();
        put(&s.ws.hippo_dir(), "packs/broken/pack.toml", "x");
        fs::write(pack_dir(&s, "broken").join("origin.json"), "not json").unwrap();
        let listed = list(&s.ws).unwrap();
        let states: Vec<(String, String)> = listed
            .iter()
            .map(|p| (p.name.clone(), p.state.to_string()))
            .collect();
        assert_eq!(states[0].0, "broken");
        assert!(states[0].1.starts_with("unreadable: "), "{states:?}");
        assert_eq!(states[1], ("glossary".into(), "clean".into()));
        assert_eq!(
            states[2],
            (
                "zeta".into(),
                "unreadable: link.md: a symbolic link; packs may not contain links".into()
            )
        );
        assert!(listed[2].version.is_none() && listed[2].source.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_diff_ignores_the_users_external_diff_and_textconv() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = serial();
        let tools = tempfile::tempdir().unwrap();
        let marker = tools.path().join("external-ran");
        let script = tools.path().join("ext.sh");
        fs::write(&script, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let home = tools.path().join("home");
        fs::create_dir(&home).unwrap();
        fs::write(
            home.join(".gitconfig"),
            format!("[diff]\n\texternal = {}\n", script.display()),
        )
        .unwrap();
        let old_home = std::env::var_os("HOME");
        let old_xdg = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join("xdg"));
        let mut a = PackFiles::default();
        a.insert("x.md".into(), b"one\n".to_vec());
        let mut b = PackFiles::default();
        b.insert("x.md".into(), b"two\n".to_vec());
        let diff = diff_text(&a, &b);
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let diff = diff.unwrap();
        assert!(!marker.exists(), "the external diff ran");
        assert!(diff.contains("+++ upstream/x.md"), "{diff}");
    }
}

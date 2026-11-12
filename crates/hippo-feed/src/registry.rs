//! The registry index as the feed reads it: the one place that knows the
//! index's shape, the entry rules and the licence list.
//!
//! [`map_index`] turns the index's JSON into one [`Mapped`] per entry;
//! [`check`] applies the entry rules to an [`Entry`]; [`PackRow`] is a row of
//! the `packs` table. [`sync`] fetches, checks and writes.
//!
//! The feed reads only the documented index fields; every other field of the
//! index is ignored.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::LazyLock;

use regex::Regex;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::config::RegistryIndex;
use crate::db::{Db, UserId};
use crate::error::AppError;
use crate::http::{self, Fetched, Http};
use crate::{audit, clock, policy};

/// The protocol the feed reads.
pub const PROTOCOL: &str = "HIPPO_V1";

/// The only file of a theme pack the feed fetches.
pub const THEME_FILE: &str = "theme.css";

/// The reads a pass may declare.
const READS: &[&str] = &["sessions", "notes", "turns", "human_confirmed"];

const GITHUB: &str = "https://github.com/";

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

static NAME_RE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$"));
static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,3})$"));
static LOGIN_RE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$"));
static REPO_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^[A-Za-z0-9_-][A-Za-z0-9._-]{0,99}$"));
static PATH_SEGMENT_RE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Za-z0-9._-]+$"));
static PASS_NAME_RE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-z0-9]+(-[a-z0-9]+)*$"));
static COMMIT_RE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[0-9a-f]{40}$"));
static HASH_RE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[0-9a-f]{64}$"));

// --- Types ---

/// What a pack holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Passes,
    Theme,
}

impl Kind {
    /// The text of the `kind` column and of the index.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Passes => "passes",
            Kind::Theme => "theme",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        match text {
            "passes" => Some(Kind::Passes),
            "theme" => Some(Kind::Theme),
            _ => None,
        }
    }
}

/// A pass of a pack: its name and what it reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct Pass {
    pub name: String,
    pub reads: Vec<String>,
}

/// A file of a theme pack, as the index lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct File {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// One entry of the index, read but not yet checked against the entry rules.
///
/// `kind` is the index's text (`passes` when absent) so that [`check`] can
/// say what is wrong with an unknown one. `commit` is the index's name for
/// the `commit_id` column; `review_at` is already in the storage format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub owner: String,
    pub owner_id: i64,
    pub maintainers: Vec<String>,
    pub first_party: bool,
    pub license: String,
    pub repo: String,
    pub repo_id: i64,
    pub path: String,
    pub commit: String,
    pub pack_hash: String,
    pub passes: Vec<Pass>,
    pub theme_contract: Option<i64>,
    pub files: Vec<File>,
    pub review_id: String,
    pub review_at: String,
}

/// The result of reading one entry of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    /// The entry's `name` when it has one that is text, so a failing entry
    /// can still be named (in a `registry.skip` row, or to delist a pack).
    pub name: Option<String>,
    /// The entry, or why it could not be read: a missing or mistyped field,
    /// or a name that the index uses twice.
    pub entry: Result<Entry, String>,
}

/// Why a whole index is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IndexError {
    #[error("the index does not parse: {0}")]
    Parse(String),
    #[error("the index's protocol is not {PROTOCOL}")]
    Protocol,
}

/// A row of the `packs` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackRow {
    pub name: String,
    pub kind: Kind,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub owner: String,
    pub owner_id: i64,
    pub maintainers: Vec<String>,
    pub first_party: bool,
    pub license: String,
    pub repo: String,
    pub repo_id: i64,
    pub path: String,
    /// The `commit_id` column.
    pub commit_id: String,
    pub pack_hash: String,
    pub passes: Vec<Pass>,
    pub theme_contract: Option<i64>,
    /// The `sha256` of the web theme's `theme.css`, for a theme pack that has
    /// one the feed can use.
    pub theme_sha256: Option<String>,
    pub review_id: String,
    /// In the storage format.
    pub review_at: String,
    pub listed: bool,
    /// In the storage format.
    pub synced_at: String,
}

impl PackRow {
    /// The columns [`PackRow::from_row`] reads, in a `SELECT` list.
    pub const COLUMNS: &'static str = "name, kind, version, description, authors, owner, \
         owner_id, maintainers, first_party, license, repo, repo_id, path, commit_id, \
         pack_hash, passes, theme_contract, theme_sha256, review_id, review_at, listed, \
         synced_at";

    /// The row an entry that passed [`check`] becomes.
    pub fn from_entry(entry: &Entry, listed: bool, synced_at: &str) -> PackRow {
        PackRow {
            name: entry.name.clone(),
            kind: Kind::parse(&entry.kind).unwrap_or(Kind::Passes),
            version: entry.version.clone(),
            description: entry.description.clone(),
            authors: entry.authors.clone(),
            owner: entry.owner.clone(),
            owner_id: entry.owner_id,
            maintainers: entry.maintainers.clone(),
            first_party: entry.first_party,
            license: entry.license.clone(),
            repo: entry.repo.clone(),
            repo_id: entry.repo_id,
            path: entry.path.clone(),
            commit_id: entry.commit.clone(),
            pack_hash: entry.pack_hash.clone(),
            passes: entry.passes.clone(),
            theme_contract: entry.theme_contract,
            theme_sha256: entry.web_theme_sha256().map(str::to_owned),
            review_id: entry.review_id.clone(),
            review_at: entry.review_at.clone(),
            listed,
            synced_at: synced_at.to_owned(),
        }
    }

    /// Reads a row selected with [`PackRow::COLUMNS`].
    pub fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PackRow> {
        let json = |index: usize| -> rusqlite::Result<String> { row.get(index) };
        let bad = |index: usize, e: serde_json::Error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        };
        let kind: String = row.get("kind")?;
        let authors: Vec<String> = serde_json::from_str(&json(4)?).map_err(|e| bad(4, e))?;
        let maintainers: Vec<String> = serde_json::from_str(&json(7)?).map_err(|e| bad(7, e))?;
        let passes: Vec<Pass> = serde_json::from_str(&json(15)?).map_err(|e| bad(15, e))?;
        Ok(PackRow {
            name: row.get("name")?,
            kind: Kind::parse(&kind).unwrap_or(Kind::Passes),
            version: row.get("version")?,
            description: row.get("description")?,
            authors,
            owner: row.get("owner")?,
            owner_id: row.get("owner_id")?,
            maintainers,
            first_party: row.get("first_party")?,
            license: row.get("license")?,
            repo: row.get("repo")?,
            repo_id: row.get("repo_id")?,
            path: row.get("path")?,
            commit_id: row.get("commit_id")?,
            pack_hash: row.get("pack_hash")?,
            passes,
            theme_contract: row.get("theme_contract")?,
            theme_sha256: row.get("theme_sha256")?,
            review_id: row.get("review_id")?,
            review_at: row.get("review_at")?,
            listed: row.get("listed")?,
            synced_at: row.get("synced_at")?,
        })
    }

    /// The `authors` column's text: a JSON array of display names.
    pub fn authors_json(&self) -> String {
        serde_json::to_string(&self.authors).expect("strings serialise")
    }

    /// The `maintainers` column's text: a JSON array of logins.
    pub fn maintainers_json(&self) -> String {
        serde_json::to_string(&self.maintainers).expect("strings serialise")
    }

    /// The `passes` column's text: a JSON array of `{name, reads}`.
    pub fn passes_json(&self) -> String {
        serde_json::to_string(&self.passes).expect("passes serialise")
    }

    /// Where the pack's content is on GitHub.
    pub fn source_link(&self) -> Option<String> {
        source_link(&self.repo, &self.commit_id, &self.path)
    }

    /// The domain a pack's post shows: `github.com/<owner>`.
    pub fn domain(&self) -> String {
        domain(&self.owner)
    }

    /// Where the pack's `theme.css` is fetched from under `raw_url`.
    pub fn theme_css_url(&self, raw_url: &str) -> Option<String> {
        raw_theme_url(raw_url, &self.repo, &self.commit_id, &self.path)
    }
}

impl Entry {
    /// The pack's kind, when `kind` is one the feed knows.
    pub fn kind(&self) -> Option<Kind> {
        Kind::parse(&self.kind)
    }

    /// The `sha256` of the web theme's `theme.css`, when the entry is a theme
    /// pack with a web theme the feed can use: `theme_contract` 1 and a
    /// `theme.css` among the files. Other theme packs (a desktop theme only,
    /// another contract version) are listed with no post and no fetch.
    pub fn web_theme_sha256(&self) -> Option<&str> {
        if self.kind() != Some(Kind::Theme) || self.theme_contract != Some(policy::THEME_CONTRACT) {
            return None;
        }
        self.files
            .iter()
            .find(|f| f.path == THEME_FILE)
            .map(|f| f.sha256.as_str())
    }

    /// Where the pack's content is on GitHub.
    pub fn source_link(&self) -> Option<String> {
        source_link(&self.repo, &self.commit, &self.path)
    }

    /// The domain a pack's post shows: `github.com/<owner>`.
    pub fn domain(&self) -> String {
        domain(&self.owner)
    }

    /// Where the pack's `theme.css` is fetched from under `raw_url`.
    pub fn theme_css_url(&self, raw_url: &str) -> Option<String> {
        raw_theme_url(raw_url, &self.repo, &self.commit, &self.path)
    }
}

// --- Links and domains ---

/// `<owner>/<repository name>` of a `https://github.com/...` repository URL.
fn owner_and_repo(repo: &str) -> Option<(&str, &str)> {
    repo.strip_prefix(GITHUB)?.split_once('/')
}

/// A pack's source link: `https://github.com/<owner>/<repository name>/tree/<commit>/<path>`,
/// with the trailing `/<path>` left out when the path is empty. `None` for a
/// `repo` that is not a GitHub repository URL.
pub fn source_link(repo: &str, commit: &str, path: &str) -> Option<String> {
    let (owner, name) = owner_and_repo(repo)?;
    let mut link = format!("{GITHUB}{owner}/{name}/tree/{commit}");
    if !path.is_empty() {
        link.push('/');
        link.push_str(path);
    }
    Some(link)
}

/// A pack post's domain: `github.com/<owner>`.
pub fn domain(owner: &str) -> String {
    format!("github.com/{owner}")
}

/// Where a theme pack's stylesheet is fetched:
/// `<raw_url>/<owner>/<repository name>/<commit>/<path>/theme.css`, with
/// `<path>/` left out when the path is empty. `raw_url` is `FEED_RAW_URL`; a
/// trailing `/` on it is ignored. `None` for a `repo` that is not a GitHub
/// repository URL.
pub fn theme_css_url(raw_url: &str, entry: &Entry) -> Option<String> {
    entry.theme_css_url(raw_url)
}

fn raw_theme_url(raw_url: &str, repo: &str, commit: &str, path: &str) -> Option<String> {
    let (owner, name) = owner_and_repo(repo)?;
    let mut url = format!("{}/{owner}/{name}/{commit}/", raw_url.trim_end_matches('/'));
    if !path.is_empty() {
        url.push_str(path);
        url.push('/');
    }
    url.push_str(THEME_FILE);
    Some(url)
}

// --- Reading the index ---

#[derive(Deserialize)]
struct RawReview {
    id: String,
    at: String,
}

#[derive(Deserialize)]
struct RawEntry {
    name: String,
    #[serde(default)]
    kind: Option<String>,
    version: String,
    description: String,
    authors: Vec<String>,
    owner: String,
    owner_id: i64,
    maintainers: Vec<String>,
    first_party: bool,
    license: String,
    repo: String,
    repo_id: i64,
    path: String,
    commit: String,
    pack_hash: String,
    passes: Vec<Pass>,
    #[serde(default)]
    theme_contract: Option<i64>,
    #[serde(default)]
    files: Option<Vec<File>>,
    review: RawReview,
}

fn read_entry(value: Value) -> Result<Entry, String> {
    let raw: RawEntry = serde_json::from_value(value).map_err(|e| e.to_string())?;
    let at = OffsetDateTime::parse(&raw.review.at, &Rfc3339)
        .map_err(|e| format!("review.at is not an RFC 3339 time: {e}"))?;
    Ok(Entry {
        name: raw.name,
        kind: raw.kind.unwrap_or_else(|| Kind::Passes.as_str().to_owned()),
        version: raw.version,
        description: raw.description,
        authors: raw.authors,
        owner: raw.owner,
        owner_id: raw.owner_id,
        maintainers: raw.maintainers,
        first_party: raw.first_party,
        license: raw.license,
        repo: raw.repo,
        repo_id: raw.repo_id,
        path: raw.path,
        commit: raw.commit,
        pack_hash: raw.pack_hash,
        passes: raw.passes,
        theme_contract: raw.theme_contract,
        files: raw.files.unwrap_or_default(),
        review_id: raw.review.id,
        review_at: clock::storage(at),
    })
}

/// Reads the registry index, in the order of its entries.
///
/// The index is refused whole, with an [`IndexError`], when it does not parse
/// or its `protocol` is not `HIPPO_V1`. Otherwise every entry gets a
/// [`Mapped`]: an entry with a missing or mistyped field fails alone, and so
/// does every entry of a name the index uses more than once. Unknown fields
/// are ignored. The entry rules are [`check`]'s, not this function's.
pub fn map_index(json: &str) -> Result<Vec<Mapped>, IndexError> {
    let value: Value = serde_json::from_str(json).map_err(|e| IndexError::Parse(e.to_string()))?;
    let Value::Object(mut top) = value else {
        return Err(IndexError::Parse("the index is not a JSON object".into()));
    };
    if top.get("protocol").and_then(Value::as_str) != Some(PROTOCOL) {
        return Err(IndexError::Protocol);
    }
    let Some(Value::Array(packs)) = top.remove("packs") else {
        return Err(IndexError::Parse("`packs` is missing or not a list".into()));
    };
    let names: Vec<Option<String>> = packs
        .iter()
        .map(|p| p.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for name in names.iter().flatten() {
        *seen.entry(name.as_str()).or_default() += 1;
    }
    let mapped: Vec<Mapped> = packs
        .into_iter()
        .zip(&names)
        .map(|(value, name)| {
            let twice = name
                .as_deref()
                .is_some_and(|n| seen.get(n).copied().unwrap_or(0) > 1);
            let entry = if twice {
                Err("the name appears more than once in the index".to_owned())
            } else {
                read_entry(value)
            };
            Mapped {
                name: name.clone(),
                entry,
            }
        })
        .collect();
    Ok(mapped)
}

// --- Entry rules ---

/// A value as a message shows it: quoted, cut to 40 characters, so a reason
/// written to the public audit log stays short.
fn shown(text: &str) -> String {
    let mut cut: String = text.chars().take(40).collect();
    if cut.chars().count() < text.chars().count() {
        cut.push('…');
    }
    format!("{cut:?}")
}

fn is_login(text: &str) -> bool {
    LOGIN_RE.is_match(text)
}

fn check_repo(entry: &Entry, why: &mut Vec<String>) {
    let name = entry
        .repo
        .strip_prefix(GITHUB)
        .and_then(|rest| rest.strip_prefix(entry.owner.as_str()))
        .and_then(|rest| rest.strip_prefix('/'));
    let ok = name
        .is_some_and(|n| REPO_NAME_RE.is_match(n) && n != "." && n != ".." && !n.ends_with(".git"));
    if !ok {
        why.push(format!(
            "repo {} is not https://github.com/<owner>/<repository name> with the entry's owner",
            shown(&entry.repo)
        ));
    }
}

fn check_path(path: &str, why: &mut Vec<String>) {
    if path.is_empty() {
        return;
    }
    let ok = path
        .split('/')
        .all(|s| PATH_SEGMENT_RE.is_match(s) && s != "." && s != "..");
    if !ok {
        why.push(format!(
            "path {} is not empty or relative segments of [A-Za-z0-9._-] with no . or .. segment",
            shown(path)
        ));
    }
}

fn check_passes(entry: &Entry, why: &mut Vec<String>) {
    match entry.kind() {
        Some(Kind::Passes) => {
            let n = entry.passes.len();
            if n == 0 || n > policy::PACK_PASSES_MAX {
                why.push(format!(
                    "a pack of passes has 1 to {} passes, not {n}",
                    policy::PACK_PASSES_MAX
                ));
            }
            for pass in &entry.passes {
                if !PASS_NAME_RE.is_match(&pass.name) {
                    why.push(format!("pass name {} is not valid", shown(&pass.name)));
                }
                for read in &pass.reads {
                    if !READS.contains(&read.as_str()) {
                        why.push(format!(
                            "pass {} reads {}, which is not one of sessions, notes, turns, human_confirmed",
                            shown(&pass.name),
                            shown(read)
                        ));
                    }
                }
            }
        }
        Some(Kind::Theme) if !entry.passes.is_empty() => {
            why.push("a theme pack has no passes".to_owned());
        }
        _ => {}
    }
}

fn check_theme_fields(entry: &Entry, why: &mut Vec<String>) {
    match entry.kind() {
        Some(Kind::Passes) if entry.theme_contract.is_some() => {
            why.push("a pack of passes has no theme_contract".to_owned());
        }
        Some(Kind::Theme) if entry.theme_contract == Some(policy::THEME_CONTRACT) => {
            match entry.files.iter().find(|f| f.path == THEME_FILE) {
                None => why.push(format!(
                    "theme_contract {} needs {THEME_FILE} among the files",
                    policy::THEME_CONTRACT
                )),
                Some(file) => {
                    if file.bytes > policy::CSS_MAX_BYTES as u64 {
                        why.push(format!(
                            "{THEME_FILE} is {} bytes; the limit is {}",
                            file.bytes,
                            policy::CSS_MAX_BYTES
                        ));
                    }
                    if !HASH_RE.is_match(&file.sha256) {
                        why.push(format!(
                            "{THEME_FILE}'s sha256 must be 64 lower-case hex digits"
                        ));
                    }
                }
            }
        }
        _ => {}
    }
}

/// Applies the entry rules and the licence list to `entry`.
///
/// `Ok` when the entry passes; otherwise every rule it breaks, one reason
/// each, in a fixed order. The first is the reason a
/// `registry.skip` or `registry.delist` row records.
pub fn check(entry: &Entry) -> Result<(), Vec<String>> {
    let mut why: Vec<String> = Vec::new();

    if !(policy::PACK_NAME_MIN..=policy::PACK_NAME_MAX).contains(&entry.name.len())
        || !NAME_RE.is_match(&entry.name)
    {
        why.push(format!(
            "name {} is not 2 to 40 bytes of lower-case letters, digits and single hyphens, starting with a letter",
            shown(&entry.name)
        ));
    }
    if !VERSION_RE.is_match(&entry.version) {
        why.push(format!(
            "version {} is not MAJOR.MINOR.PATCH",
            shown(&entry.version)
        ));
    }
    let description_chars = entry.description.chars().count();
    if description_chars == 0
        || description_chars > policy::PACK_DESCRIPTION_MAX
        || entry.description.chars().any(char::is_control)
    {
        why.push("description is not 1 to 300 characters with no control characters".to_owned());
    }
    if !is_login(&entry.owner) {
        why.push(format!(
            "owner {} is not a GitHub login",
            shown(&entry.owner)
        ));
    }
    if entry.maintainers.is_empty() || entry.maintainers.len() > policy::PACK_MAINTAINERS_MAX {
        why.push(format!(
            "maintainers has {} logins; it needs 1 to {}",
            entry.maintainers.len(),
            policy::PACK_MAINTAINERS_MAX
        ));
    }
    for maintainer in &entry.maintainers {
        if !is_login(maintainer) {
            why.push(format!(
                "maintainer {} is not a GitHub login",
                shown(maintainer)
            ));
        }
    }
    if entry.owner_id <= 0 {
        why.push("owner_id is not a positive integer".to_owned());
    }
    if entry.repo_id <= 0 {
        why.push("repo_id is not a positive integer".to_owned());
    }
    if !policy::LICENCES.contains(&entry.license.as_str()) {
        why.push(format!(
            "license {} is not in the licence list",
            shown(&entry.license)
        ));
    }
    check_repo(entry, &mut why);
    check_path(&entry.path, &mut why);
    if !COMMIT_RE.is_match(&entry.commit) {
        why.push("commit is not 40 lower-case hex digits".to_owned());
    }
    if !HASH_RE.is_match(&entry.pack_hash) {
        why.push("pack_hash is not 64 lower-case hex digits".to_owned());
    }
    if entry.kind().is_none() {
        why.push(format!(
            "kind {} is neither passes nor theme",
            shown(&entry.kind)
        ));
    }
    check_passes(entry, &mut why);
    check_theme_fields(entry, &mut why);

    if why.is_empty() {
        Ok(())
    } else {
        Err(why)
    }
}

// --- Sync ---

/// Where the sync reads from, and what it fetches with.
#[derive(Clone)]
pub struct SyncContext {
    /// `FEED_REGISTRY_INDEX`.
    pub source: RegistryIndex,
    /// `FEED_RAW_URL`.
    pub raw_url: String,
    /// The outbound agent (for registry fetches).
    pub agent: Http,
}

/// What one sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Packs listed: new, or relisted.
    pub listed: usize,
    pub delisted: usize,
    /// Listed before and after, with a changed entry.
    pub updated: usize,
    /// Entries that failed and name no pack the feed has listed.
    pub skipped: usize,
    pub unchanged: usize,
    /// Stylesheets accepted.
    pub themes: usize,
    /// Why the whole index was refused (not read, not JSON, wrong protocol);
    /// no pack changed then.
    pub error: Option<String>,
}

impl SyncReport {
    /// Whether the index was read.
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }

    /// The line `hippo-feed sync-registry` prints.
    pub fn line(&self) -> String {
        format!(
            "listed {}, delisted {}, updated {}, skipped {}, unchanged {}, themes {}",
            self.listed, self.delisted, self.updated, self.skipped, self.unchanged, self.themes
        )
    }
}

/// An entry after the entry rules.
enum Checked {
    Valid(Box<Entry>),
    Failed {
        name: Option<String>,
        reasons: Vec<String>,
    },
}

/// What became of a new content's stylesheet.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sheet {
    Accepted(String),
    Refused(String),
}

type ContentKey = (String, String);

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError::internal(e)
}

/// The text of the index, from a URL or a file, at most 5 MB.
fn read_source(ctx: &SyncContext) -> Result<String, String> {
    let bytes = match &ctx.source {
        RegistryIndex::Url(url) => ctx
            .agent
            .fetch_registry(url)
            .map_err(|e| format!("the index could not be fetched: {e}"))?,
        RegistryIndex::Path(path) => {
            let file = std::fs::File::open(path)
                .map_err(|e| format!("the index file could not be read: {e}"))?;
            let mut bytes = Vec::new();
            file.take(http::REGISTRY_LIMIT + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("the index file could not be read: {e}"))?;
            if bytes.len() as u64 > http::REGISTRY_LIMIT {
                return Err("the index file is over 5 MB".to_owned());
            }
            bytes
        }
    };
    String::from_utf8(bytes).map_err(|_| "the index is not UTF-8".to_owned())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Judges a stylesheet that came back with status 200.
fn judge(fetched: Fetched, sha256: &str) -> Sheet {
    let bytes = match fetched {
        Fetched::TooLarge => return Sheet::Refused("over 64 KiB".to_owned()),
        Fetched::Body(bytes) => bytes,
    };
    let Ok(text) = String::from_utf8(bytes.clone()) else {
        return Sheet::Refused("not UTF-8".to_owned());
    };
    if sha256_hex(&bytes) != sha256 {
        return Sheet::Refused("hash mismatch".to_owned());
    }
    match hippo_theme::check_theme_pack_css(&text) {
        Ok(()) => Sheet::Accepted(text),
        Err(e) => Sheet::Refused(e.to_string()),
    }
}

/// Fetches the stylesheet of each `(key, url, sha256)`. A fetch that fails
/// (no connection, timeout, redirect, status other than 200) leaves its key
/// out, so nothing is recorded and the next sync tries again.
fn fetch_sheets(
    agent: &Http,
    wanted: Vec<(ContentKey, String, String)>,
) -> HashMap<ContentKey, Sheet> {
    let mut out = HashMap::new();
    for (key, url, sha256) in wanted {
        match agent.fetch_theme_checked(&url) {
            Ok(fetched) => {
                out.insert(key, judge(fetched, &sha256));
            }
            Err(e) => tracing::warn!("stylesheet of {} not fetched: {e}", key.0),
        }
    }
    out
}

fn set_meta(tx: &Transaction, key: &str, value: &str) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// A pack post's title: `<name>: <description>`. When that is longer than
/// 120 characters it is cut at the last word boundary that fits and ends in
/// `…`, so a title never stops mid-word.
pub fn pack_title(name: &str, description: &str) -> String {
    let full = format!("{name}: {description}");
    if full.chars().count() <= policy::PACK_TITLE_MAX {
        return full;
    }
    // Room for the ellipsis.
    let cut: String = full.chars().take(policy::PACK_TITLE_MAX - 1).collect();
    // The character after the cut: when it is a space, the cut already ends a word.
    let next_is_space = full
        .chars()
        .nth(policy::PACK_TITLE_MAX - 1)
        .is_some_and(char::is_whitespace);
    let head = if next_is_space {
        cut.as_str()
    } else {
        match cut.rfind(char::is_whitespace) {
            // Keep at least the name and its colon.
            Some(i) if i > name.len() + 1 => &cut[..i],
            _ => cut.as_str(),
        }
    };
    let head = head.trim_end_matches(|c: char| c.is_whitespace() || ",;:-–—(".contains(c));
    format!("{head}…")
}

fn pack_row(tx: &Transaction, name: &str) -> rusqlite::Result<Option<PackRow>> {
    tx.query_row(
        &format!("SELECT {} FROM packs WHERE name = ?1", PackRow::COLUMNS),
        [name],
        PackRow::from_row,
    )
    .optional()
}

fn write_pack(tx: &Transaction, row: &PackRow, insert: bool) -> rusqlite::Result<()> {
    let sql = if insert {
        "INSERT INTO packs (name, kind, version, description, authors, owner, owner_id,
             maintainers, first_party, license, repo, repo_id, path, commit_id, pack_hash,
             passes, theme_contract, theme_sha256, review_id, review_at, listed, synced_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
             ?18, ?19, ?20, ?21, ?22)"
    } else {
        "UPDATE packs SET kind = ?2, version = ?3, description = ?4, authors = ?5, owner = ?6,
             owner_id = ?7, maintainers = ?8, first_party = ?9, license = ?10, repo = ?11,
             repo_id = ?12, path = ?13, commit_id = ?14, pack_hash = ?15, passes = ?16,
             theme_contract = ?17, theme_sha256 = ?18, review_id = ?19, review_at = ?20,
             listed = ?21, synced_at = ?22
         WHERE name = ?1"
    };
    tx.execute(
        sql,
        params![
            row.name,
            row.kind.as_str(),
            row.version,
            row.description,
            row.authors_json(),
            row.owner,
            row.owner_id,
            row.maintainers_json(),
            row.first_party,
            row.license,
            row.repo,
            row.repo_id,
            row.path,
            row.commit_id,
            row.pack_hash,
            row.passes_json(),
            row.theme_contract,
            row.theme_sha256,
            row.review_id,
            row.review_at,
            row.listed,
            row.synced_at,
        ],
    )?;
    Ok(())
}

/// Creates the pack's post when it has none and may have one (a pack of
/// passes, or a theme pack with a web theme), and keeps its title equal to
/// `<name>: <description>`.
fn ensure_post(tx: &Transaction, entry: &Entry, stamp: &str) -> rusqlite::Result<()> {
    let title = pack_title(&entry.name, &entry.description);
    let existing: Option<(i64, String)> = tx
        .query_row(
            "SELECT id, title FROM posts WHERE pack = ?1",
            [&entry.name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match existing {
        Some((id, old)) => {
            if old != title {
                tx.execute(
                    "UPDATE posts SET title = ?1 WHERE id = ?2",
                    params![title, id],
                )?;
            }
        }
        None => {
            let eligible = entry.kind() == Some(Kind::Passes) || entry.web_theme_sha256().is_some();
            if eligible {
                tx.execute(
                    "INSERT INTO posts (kind, pack, title, created_at) VALUES ('pack', ?1, ?2, ?3)",
                    params![entry.name, title, stamp],
                )?;
            }
        }
    }
    Ok(())
}

/// Binds the entry's maintainers other than the owner to GitHub ids, as
/// "Pack maintainers" says, and records a change.
fn bind_maintainers(
    tx: &Transaction,
    entry: &Entry,
    now: OffsetDateTime,
    stamp: &str,
) -> rusqlite::Result<()> {
    let mut bound: Vec<(i64, String)> = {
        let mut stmt =
            tx.prepare("SELECT github_id, login FROM pack_maintainer_ids WHERE pack = ?1")?;
        let rows = stmt.query_map([&entry.name], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let named = |login: &str| {
        entry
            .maintainers
            .iter()
            .any(|m| m.eq_ignore_ascii_case(login))
    };
    let mut removed: Vec<String> = Vec::new();
    bound.retain(|(id, login)| {
        if named(login) {
            true
        } else {
            removed.push(login.clone());
            let _ = tx.execute(
                "DELETE FROM pack_maintainer_ids WHERE pack = ?1 AND github_id = ?2",
                params![entry.name, id],
            );
            false
        }
    });
    let mut added: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for login in &entry.maintainers {
        if login.eq_ignore_ascii_case(&entry.owner) || seen.contains(&login.to_ascii_lowercase()) {
            continue;
        }
        seen.push(login.to_ascii_lowercase());
        let holder: Option<i64> = tx
            .query_row(
                "SELECT github_id FROM users
                 WHERE login = ?1 AND deleted_at IS NULL AND github_id IS NOT NULL",
                [login],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = holder else { continue };
        if bound.iter().any(|(bound_id, _)| *bound_id == id) {
            continue;
        }
        tx.execute(
            "INSERT INTO pack_maintainer_ids (pack, github_id, login, bound_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![entry.name, id, login, stamp],
        )?;
        bound.push((id, login.clone()));
        added.push(login.clone());
    }
    if !added.is_empty() || !removed.is_empty() {
        audit::write(
            tx,
            now,
            None,
            audit::REGISTRY_MAINTAINERS,
            Some(("pack", entry.name.clone())),
            "",
            true,
            json!({"added": added, "removed": removed}),
        )?;
    }
    Ok(())
}

fn delist(
    tx: &Transaction,
    name: &str,
    reason: &str,
    now: OffsetDateTime,
    stamp: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE packs SET listed = 0, synced_at = ?2 WHERE name = ?1",
        params![name, stamp],
    )?;
    audit::write(
        tx,
        now,
        None,
        audit::REGISTRY_DELIST,
        Some(("pack", name.to_owned())),
        reason,
        true,
        json!({}),
    )
}

/// Writes a `registry.skip` row unless the log already holds one for that
/// name and reason.
fn skip(
    tx: &Transaction,
    name: Option<&str>,
    reason: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<()> {
    let known: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM audit_log
                        WHERE action = ?1 AND target_id IS ?2 AND reason = ?3)",
        params![audit::REGISTRY_SKIP, name, reason],
        |r| r.get(0),
    )?;
    if known {
        return Ok(());
    }
    audit::write(
        tx,
        now,
        None,
        audit::REGISTRY_SKIP,
        name.map(|n| ("pack", n.to_owned())),
        reason,
        true,
        json!({}),
    )
}

fn apply_valid(
    tx: &Transaction,
    entry: &Entry,
    sheets: &HashMap<ContentKey, Sheet>,
    now: OffsetDateTime,
    stamp: &str,
    report: &mut SyncReport,
) -> rusqlite::Result<()> {
    let new = PackRow::from_entry(entry, true, stamp);
    match pack_row(tx, &entry.name)? {
        None => {
            write_pack(tx, &new, true)?;
            report.listed += 1;
            audit::write(
                tx,
                now,
                None,
                audit::REGISTRY_LIST,
                Some(("pack", entry.name.clone())),
                "",
                true,
                json!({"version": entry.version}),
            )?;
        }
        Some(old) => {
            let mut same = old.clone();
            same.listed = true;
            same.synced_at = stamp.to_owned();
            let changed = same != new;
            write_pack(tx, &new, false)?;
            if !old.listed {
                report.listed += 1;
                audit::write(
                    tx,
                    now,
                    None,
                    audit::REGISTRY_LIST,
                    Some(("pack", entry.name.clone())),
                    "",
                    true,
                    json!({"version": entry.version}),
                )?;
            } else if changed {
                report.updated += 1;
            } else {
                report.unchanged += 1;
            }
        }
    }
    tx.execute(
        "INSERT OR IGNORE INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            entry.name,
            entry.pack_hash,
            entry.version,
            entry.commit,
            stamp
        ],
    )?;
    ensure_post(tx, entry, stamp)?;
    if entry.web_theme_sha256().is_some() {
        let key = (entry.name.clone(), entry.pack_hash.clone());
        match sheets.get(&key) {
            Some(Sheet::Accepted(css)) => {
                tx.execute(
                    "INSERT OR REPLACE INTO theme_files (pack, pack_hash, css, failure, at)
                     VALUES (?1, ?2, ?3, NULL, ?4)",
                    params![entry.name, entry.pack_hash, css, stamp],
                )?;
                tx.execute(
                    "DELETE FROM theme_files
                     WHERE pack = ?1 AND css IS NOT NULL AND pack_hash <> ?2",
                    params![entry.name, entry.pack_hash],
                )?;
                report.themes += 1;
            }
            Some(Sheet::Refused(reason)) => {
                tx.execute(
                    "INSERT OR REPLACE INTO theme_files (pack, pack_hash, css, failure, at)
                     VALUES (?1, ?2, NULL, ?3, ?4)",
                    params![entry.name, entry.pack_hash, reason, stamp],
                )?;
            }
            None => {}
        }
    }
    bind_maintainers(tx, entry, now, stamp)
}

fn apply(
    tx: &Transaction,
    checked: &[Checked],
    sheets: &HashMap<ContentKey, Sheet>,
    now: OffsetDateTime,
    actor: Option<UserId>,
) -> rusqlite::Result<SyncReport> {
    let stamp = clock::storage(now);
    let mut report = SyncReport::default();
    let mut named: HashSet<&str> = HashSet::new();
    for item in checked {
        match item {
            Checked::Valid(entry) => {
                named.insert(entry.name.as_str());
                apply_valid(tx, entry, sheets, now, &stamp, &mut report)?;
            }
            Checked::Failed { name, reasons } => {
                let reason = reasons
                    .first()
                    .map_or("the entry fails the rules", String::as_str);
                if let Some(n) = name {
                    named.insert(n.as_str());
                }
                let existing = match name {
                    Some(n) => pack_row(tx, n)?,
                    None => None,
                };
                match (name.as_deref(), existing) {
                    (Some(n), Some(row)) => {
                        if row.listed {
                            delist(tx, n, reason, now, &stamp)?;
                            report.delisted += 1;
                        }
                    }
                    _ => {
                        skip(tx, name.as_deref(), reason, now)?;
                        report.skipped += 1;
                    }
                }
            }
        }
    }
    let absent: Vec<String> = {
        let mut stmt = tx.prepare("SELECT name FROM packs WHERE listed = 1 ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for name in absent.iter().filter(|n| !named.contains(n.as_str())) {
        delist(tx, name, "absent from the index", now, &stamp)?;
        report.delisted += 1;
    }
    set_meta(tx, "last_sync", &stamp)?;
    set_meta(tx, "last_sync_ok", "1")?;
    set_meta(tx, "last_successful_sync", &stamp)?;
    if actor.is_some() {
        audit::write(
            tx,
            now,
            actor,
            audit::REGISTRY_SYNC,
            None,
            "",
            true,
            json!({
                "listed": report.listed,
                "delisted": report.delisted,
                "skipped": report.skipped,
            }),
        )?;
    }
    Ok(report)
}

/// Records a refused or unreadable index: only the attempt.
async fn record_failure(db: &Db, now: OffsetDateTime, why: String) -> Result<SyncReport, AppError> {
    let stamp = clock::storage(now);
    db.write(move |tx| {
        set_meta(tx, "last_sync", &stamp)?;
        set_meta(tx, "last_sync_ok", "0")?;
        Ok(())
    })
    .await?;
    tracing::warn!("registry sync failed: {why}");
    Ok(SyncReport {
        error: Some(why),
        ..SyncReport::default()
    })
}

/// Reads the index and applies it, as "Packs and the registry" says.
///
/// `actor` is the admin who asked for the sync, or `None` for the timer and
/// the command; only an admin's sync writes a `registry.sync` row. An index
/// that cannot be read, does not parse or has another protocol changes no
/// pack: the report says why (`error`) and only the attempt is recorded. The
/// theme stylesheets that are new are fetched before the transaction opens.
pub async fn sync(
    db: &Db,
    ctx: &SyncContext,
    now: OffsetDateTime,
    actor: Option<UserId>,
) -> Result<SyncReport, AppError> {
    let reader = ctx.clone();
    let text = tokio::task::spawn_blocking(move || read_source(&reader))
        .await
        .map_err(internal)?;
    let mapped = match text.and_then(|t| map_index(&t).map_err(|e| e.to_string())) {
        Ok(mapped) => mapped,
        Err(why) => return record_failure(db, now, why).await,
    };
    let checked: Vec<Checked> = mapped
        .into_iter()
        .map(|m| match m.entry {
            Err(why) => Checked::Failed {
                name: m.name,
                reasons: vec![why],
            },
            Ok(entry) => match check(&entry) {
                Ok(()) => Checked::Valid(Box::new(entry)),
                Err(reasons) => Checked::Failed {
                    name: m.name,
                    reasons,
                },
            },
        })
        .collect();

    // Contents the feed already holds a stylesheet or a refusal for.
    let known: HashSet<ContentKey> = db
        .read(|conn| {
            let mut stmt = conn.prepare("SELECT pack, pack_hash FROM theme_files")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .await?;
    let wanted: Vec<(ContentKey, String, String)> = checked
        .iter()
        .filter_map(|item| match item {
            Checked::Valid(entry) => {
                let sha = entry.web_theme_sha256()?.to_owned();
                let key = (entry.name.clone(), entry.pack_hash.clone());
                let url = entry.theme_css_url(&ctx.raw_url)?;
                (!known.contains(&key)).then_some((key, url, sha))
            }
            Checked::Failed { .. } => None,
        })
        .collect();
    let agent = ctx.agent.clone();
    let sheets = tokio::task::spawn_blocking(move || fetch_sheets(&agent, wanted))
        .await
        .map_err(internal)?;

    db.write(move |tx| Ok(apply(tx, &checked, &sheets, now, actor)?))
        .await
}

/// Deletes every recorded refusal of a stylesheet, so a release with a newer
/// checker tries them again. `serve` calls it at start.
pub fn clear_refusals(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM theme_files WHERE failure IS NOT NULL", [])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use sha2::{Digest, Sha256};

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/feed")
    }

    #[test]
    fn pack_title_keeps_a_short_title_whole() {
        assert_eq!(pack_title("faq", "Appends FAQs."), "faq: Appends FAQs.");
        let exact = "x".repeat(policy::PACK_TITLE_MAX - 5);
        assert_eq!(
            pack_title("abc", &exact).chars().count(),
            policy::PACK_TITLE_MAX
        );
    }

    #[test]
    fn pack_title_cuts_a_long_title_at_a_word_with_an_ellipsis() {
        let description = "Appends security-relevant changes sessions made (auth, permissions, \
                           secret handling, advisories) to a security log note, with the date.";
        let title = pack_title("security-log", description);
        assert!(title.chars().count() <= policy::PACK_TITLE_MAX, "{title}");
        assert!(title.ends_with('…'), "{title}");
        assert!(title.ends_with(" to a…"), "{title}");
        let shown = title.trim_end_matches('…');
        assert!(format!("security-log: {description}").starts_with(shown));
    }

    #[test]
    fn pack_title_drops_trailing_punctuation_before_the_ellipsis() {
        // The word boundary falls right after a comma.
        let filler = "w".repeat(policy::PACK_TITLE_MAX - 12);
        let tail = "x".repeat(50);
        let title = pack_title("p", &format!("{filler}, {tail}"));
        assert!(title.ends_with(&format!("{filler}…")), "{title}");
    }

    #[test]
    fn pack_title_cuts_one_long_word_by_characters() {
        let title = pack_title("p", &"é".repeat(300));
        assert_eq!(title.chars().count(), policy::PACK_TITLE_MAX);
        assert!(title.ends_with("é…"));
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(fixtures().join(name)).expect("fixture")
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn mapped(name: &str) -> Vec<Mapped> {
        map_index(&fixture(name)).expect("the fixture parses")
    }

    fn find<'a>(all: &'a [Mapped], name: &str) -> Vec<&'a Mapped> {
        all.iter()
            .filter(|m| m.name.as_deref() == Some(name))
            .collect()
    }

    fn entry_of(all: &[Mapped], name: &str) -> Entry {
        let found = find(all, name);
        assert_eq!(found.len(), 1, "{name}");
        found[0].entry.clone().expect("the entry maps")
    }

    /// The `Some(reasons)` of an entry that fails `check`.
    fn reasons(all: &[Mapped], name: &str) -> Vec<String> {
        check(&entry_of(all, name)).expect_err("the entry fails")
    }

    const PASSING: [&str; 10] = [
        "glossary-terms",
        "decision-log",
        "review-dates",
        "stale-links",
        "handoff-check",
        "night-ink",
        "desk-only",
        "future-theme",
        "bad-hash",
        "bad-rules",
    ];

    #[test]
    fn the_fixture_has_fifteen_entries_and_ten_pass() {
        let all = mapped("registry-index.json");
        assert_eq!(all.len(), 15);
        let passing: Vec<&str> = all
            .iter()
            .filter(|m| m.entry.as_ref().is_ok_and(|e| check(e).is_ok()))
            .map(|m| m.name.as_deref().expect("a name"))
            .collect();
        assert_eq!(passing, PASSING);
    }

    #[test]
    fn each_passing_entry_maps_and_checks() {
        let all = mapped("registry-index.json");
        for name in PASSING {
            let entry = entry_of(&all, name);
            assert_eq!(check(&entry), Ok(()), "{name}");
        }
        let glossary = entry_of(&all, "glossary-terms");
        assert_eq!(glossary.owner, "ada");
        assert_eq!(glossary.owner_id, 1001);
        assert_eq!(glossary.kind, "passes");
        assert_eq!(glossary.passes[0].name, "glossary-terms");
        assert_eq!(glossary.review_at, "2026-11-08T11:00:00.000000Z");
        let review_dates = entry_of(&all, "review-dates");
        assert_eq!(review_dates.owner_id, 1002);
        assert_eq!(review_dates.maintainers, ["grace", "linus"]);
        for name in ["stale-links", "handoff-check"] {
            let entry = entry_of(&all, name);
            assert_eq!((entry.owner.as_str(), entry.owner_id), ("hippo-demo", 9001));
            assert!(entry.first_party);
        }
        for name in [
            "night-ink",
            "desk-only",
            "future-theme",
            "bad-hash",
            "bad-rules",
        ] {
            let entry = entry_of(&all, name);
            assert_eq!(entry.kind, "theme", "{name}");
            assert_eq!(entry.owner, "grace", "{name}");
            assert!(entry.passes.is_empty(), "{name}");
        }
    }

    #[test]
    fn theme_entries_say_whether_they_have_a_web_theme() {
        let all = mapped("registry-index.json");
        assert!(entry_of(&all, "night-ink").web_theme_sha256().is_some());
        assert!(entry_of(&all, "bad-hash").web_theme_sha256().is_some());
        assert!(entry_of(&all, "bad-rules").web_theme_sha256().is_some());
        assert_eq!(entry_of(&all, "desk-only").web_theme_sha256(), None);
        assert_eq!(entry_of(&all, "future-theme").web_theme_sha256(), None);
        assert_eq!(entry_of(&all, "glossary-terms").web_theme_sha256(), None);
        let row = PackRow::from_entry(&entry_of(&all, "night-ink"), true, "t");
        assert_eq!(row.kind, Kind::Theme);
        assert_eq!(row.theme_contract, Some(1));
        assert!(row.theme_sha256.is_some());
        assert!(row.listed);
    }

    #[test]
    fn a_license_outside_the_list_fails() {
        let all = mapped("registry-index.json");
        let why = reasons(&all, "copyleft-notes");
        assert_eq!(why, ["license \"GPL-3.0-only\" is not in the licence list"]);
    }

    #[test]
    fn a_name_starting_with_a_hyphen_fails() {
        let all = mapped("registry-index.json");
        let why = reasons(&all, "-oops");
        assert_eq!(why.len(), 1, "{why:?}");
        assert!(
            why[0].starts_with("name \"-oops\" is not 2 to 40 bytes"),
            "{why:?}"
        );
    }

    #[test]
    fn a_short_pack_hash_fails() {
        let all = mapped("registry-index.json");
        let why = reasons(&all, "short-hash");
        assert_eq!(why, ["pack_hash is not 64 lower-case hex digits"]);
        assert_eq!(entry_of(&all, "short-hash").pack_hash.len(), 63);
    }

    #[test]
    fn both_entries_of_a_repeated_name_fail() {
        let all = mapped("registry-index.json");
        let twins = find(&all, "twin");
        assert_eq!(twins.len(), 2);
        for twin in twins {
            assert_eq!(
                twin.entry,
                Err("the name appears more than once in the index".to_owned())
            );
        }
    }

    #[test]
    fn the_second_index_has_night_ink_at_a_second_version() {
        let first = mapped("registry-index.json");
        let second = mapped("registry-index-2.json");
        assert_eq!(second.len(), first.len());
        let one = entry_of(&first, "night-ink");
        let two = entry_of(&second, "night-ink");
        assert_eq!(one.version, "1.0.0");
        assert_eq!(two.version, "1.1.0");
        assert_ne!(one.pack_hash, two.pack_hash);
        assert_ne!(one.commit, two.commit);
        assert_eq!(check(&two), Ok(()));
        for m in &first {
            if m.name.as_deref() != Some("night-ink") {
                assert!(second.contains(m), "{:?}", m.name);
            }
        }
    }

    #[test]
    fn the_raw_files_are_where_the_urls_say_and_match_their_hashes() {
        let raw = fixtures().join("raw");
        let read = |index: &str, name: &str| -> (Entry, Vec<u8>) {
            let entry = entry_of(&mapped(index), name);
            let url = theme_css_url("https://raw.example", &entry).expect("a url");
            let relative = url
                .strip_prefix("https://raw.example/")
                .expect("under the raw url");
            let bytes = std::fs::read(raw.join(relative)).expect("a raw file");
            (entry, bytes)
        };
        let (entry, css) = read("registry-index.json", "night-ink");
        assert_eq!(sha256_hex(&css), entry.web_theme_sha256().unwrap());
        let text = String::from_utf8(css).expect("utf-8");
        assert_eq!(hippo_theme::check_theme_pack_css(&text), Ok(()));

        let (entry, css) = read("registry-index.json", "bad-hash");
        assert_ne!(sha256_hex(&css), entry.web_theme_sha256().unwrap());
        let text = String::from_utf8(css).expect("utf-8");
        assert_eq!(hippo_theme::check_theme_pack_css(&text), Ok(()));

        let (entry, css) = read("registry-index.json", "bad-rules");
        assert_eq!(sha256_hex(&css), entry.web_theme_sha256().unwrap());
        let text = String::from_utf8(css).expect("utf-8");
        let refused = hippo_theme::check_theme_pack_css(&text).expect_err("refused");
        assert!(refused.what.contains("not allowed"), "{refused}");

        let (entry, css) = read("registry-index-2.json", "night-ink");
        assert_eq!(entry.version, "1.1.0");
        assert_eq!(sha256_hex(&css), entry.web_theme_sha256().unwrap());
        let text = String::from_utf8(css).expect("utf-8");
        let refused = hippo_theme::check_theme_pack_css(&text).expect_err("refused");
        assert!(refused.what.contains("contrast"), "{refused}");
    }

    // --- map_index on its own inputs ---

    fn entry_json() -> serde_json::Value {
        serde_json::json!({
            "name": "glossary-terms", "version": "1.0.0", "description": "A pack.",
            "authors": ["Ada Lovelace"], "owner": "ada", "owner_id": 1001,
            "maintainers": ["ada"], "first_party": false, "license": "MIT",
            "repo": "https://github.com/ada/hippo-packs", "repo_id": 7001,
            "path": "glossary-terms",
            "commit": "3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e",
            "pack_hash": "a".repeat(64),
            "passes": [{"name": "glossary-terms", "reads": ["sessions", "notes"]}],
            "review": {"id": "01JCA5ZB3K9V7Q2M4N6P8R0T1W", "at": "2026-11-08T11:00:00Z"},
            "generated_extra": {"anything": [1, 2]}
        })
    }

    fn index_with(entries: Vec<serde_json::Value>) -> String {
        serde_json::json!({"protocol": "HIPPO_V1", "generated": "x", "packs": entries}).to_string()
    }

    #[test]
    fn unknown_fields_are_ignored_and_the_entry_maps() {
        let all = map_index(&index_with(vec![entry_json()])).expect("parses");
        let entry = all[0].entry.clone().expect("maps");
        assert_eq!(check(&entry), Ok(()));
        assert_eq!(entry.files, []);
        assert_eq!(entry.kind, "passes");
    }

    #[test]
    fn an_index_that_does_not_parse_is_refused_whole() {
        assert!(matches!(map_index("not json"), Err(IndexError::Parse(_))));
        assert!(matches!(map_index("[]"), Err(IndexError::Parse(_))));
        assert!(matches!(
            map_index(r#"{"protocol":"HIPPO_V1"}"#),
            Err(IndexError::Parse(_))
        ));
        assert!(matches!(
            map_index(r#"{"protocol":"HIPPO_V1","packs":{}}"#),
            Err(IndexError::Parse(_))
        ));
    }

    #[test]
    fn another_protocol_is_refused_whole() {
        for text in [
            r#"{"protocol":"HIPPO_V2","packs":[]}"#,
            r#"{"protocol":1,"packs":[]}"#,
            r#"{"packs":[]}"#,
        ] {
            assert_eq!(map_index(text), Err(IndexError::Protocol), "{text}");
        }
        assert_eq!(
            map_index(r#"{"protocol":"HIPPO_V1","packs":[]}"#),
            Ok(vec![])
        );
    }

    #[test]
    fn a_missing_or_mistyped_field_fails_that_entry_only() {
        let mut missing = entry_json();
        missing.as_object_mut().unwrap().remove("license");
        missing["name"] = "unlicensed".into();
        let mut mistyped = entry_json();
        mistyped["name"] = "other".into();
        mistyped["owner_id"] = "1001".into();
        let mut nameless = entry_json();
        nameless.as_object_mut().unwrap().remove("name");
        let mut bad_time = entry_json();
        bad_time["name"] = "late".into();
        bad_time["review"]["at"] = "yesterday".into();
        let all = map_index(&index_with(vec![
            missing,
            entry_json(),
            mistyped,
            nameless,
            bad_time,
            serde_json::json!("not an object"),
        ]))
        .expect("parses");
        assert_eq!(all.len(), 6);
        assert_eq!(all[0].entry, Err("missing field `license`".to_owned()));
        assert_eq!(all[0].name.as_deref(), Some("unlicensed"));
        assert!(all[1].entry.is_ok());
        assert!(all[2]
            .entry
            .as_ref()
            .unwrap_err()
            .starts_with("invalid type: string \"1001\""));
        assert_eq!(all[2].name.as_deref(), Some("other"));
        assert_eq!(all[3].entry, Err("missing field `name`".to_owned()));
        assert_eq!(all[3].name, None);
        assert!(all[4]
            .entry
            .as_ref()
            .unwrap_err()
            .starts_with("review.at is not an RFC 3339 time"));
        assert_eq!(all[5].name, None);
        assert!(all[5].entry.is_err());
    }

    #[test]
    fn a_non_text_name_is_a_mistyped_field() {
        let mut e = entry_json();
        e["name"] = 7.into();
        let all = map_index(&index_with(vec![e])).expect("parses");
        assert_eq!(all[0].name, None);
        assert!(all[0].entry.is_err());
    }

    #[test]
    fn review_at_is_converted_to_the_storage_format() {
        let mut e = entry_json();
        e["review"]["at"] = "2026-11-08T20:30:15.5+09:30".into();
        let all = map_index(&index_with(vec![e])).expect("parses");
        assert_eq!(
            all[0].entry.as_ref().unwrap().review_at,
            "2026-11-08T11:00:15.500000Z"
        );
    }

    // --- check on its own inputs ---

    fn good() -> Entry {
        let all = map_index(&index_with(vec![entry_json()])).expect("parses");
        all[0].entry.clone().expect("maps")
    }

    fn theme() -> Entry {
        let mut e = good();
        e.kind = "theme".into();
        e.passes.clear();
        e.path = "themes/x".into();
        e.theme_contract = Some(1);
        e.files = vec![File {
            path: "theme.css".into(),
            bytes: 100,
            sha256: "b".repeat(64),
        }];
        e
    }

    fn fails(entry: &Entry, expected: &str) {
        let why = check(entry).expect_err("should fail");
        assert!(
            why.iter().any(|w| w.contains(expected)),
            "{expected:?} not in {why:?}"
        );
    }

    #[test]
    fn name_rules() {
        for ok in [
            "ab",
            "a1",
            "a-b",
            "glossary-terms",
            &format!("a{}", "b".repeat(39)),
        ] {
            let mut e = good();
            e.name = ok.into();
            assert_eq!(check(&e), Ok(()), "{ok}");
        }
        for bad in [
            "a",
            "-ab",
            "ab-",
            "a--b",
            "1ab",
            "Ab",
            "a_b",
            "a b",
            "a\n",
            "",
            &format!("a{}", "b".repeat(40)),
        ] {
            let mut e = good();
            e.name = bad.into();
            fails(&e, "name ");
        }
    }

    #[test]
    fn version_rules() {
        for ok in ["0.0.0", "1.2.3", "9999.9999.9999", "10.0.1"] {
            let mut e = good();
            e.version = ok.into();
            assert_eq!(check(&e), Ok(()), "{ok}");
        }
        for bad in [
            "1.2",
            "01.2.3",
            "1.2.3.4",
            "10000.0.0",
            "1.2.3-rc1",
            "v1.2.3",
            "1.2.3\n",
            "",
        ] {
            let mut e = good();
            e.version = bad.into();
            fails(&e, "version ");
        }
    }

    #[test]
    fn description_rules() {
        for ok in ["x".to_owned(), "é".repeat(300), "a b".to_owned()] {
            let mut e = good();
            e.description = ok;
            assert_eq!(check(&e), Ok(()));
        }
        for bad in [
            String::new(),
            "x".repeat(301),
            "a\nb".to_owned(),
            "a\u{7}".to_owned(),
        ] {
            let mut e = good();
            e.description = bad;
            fails(&e, "description ");
        }
    }

    #[test]
    fn owner_and_maintainer_rules() {
        let mut e = good();
        e.owner = "-ada".into();
        e.repo = "https://github.com/-ada/hippo-packs".into();
        fails(&e, "owner ");
        let mut e = good();
        e.owner = "a".repeat(40);
        e.repo = format!("https://github.com/{}/r", e.owner);
        fails(&e, "owner ");
        let mut e = good();
        e.owner = "a".repeat(39);
        e.repo = format!("https://github.com/{}/r", e.owner);
        assert_eq!(check(&e), Ok(()));
        let mut e = good();
        e.maintainers.clear();
        fails(&e, "maintainers has 0 logins");
        let mut e = good();
        e.maintainers = (0..11).map(|i| format!("m{i}")).collect();
        fails(&e, "maintainers has 11 logins");
        e.maintainers.truncate(10);
        assert_eq!(check(&e), Ok(()));
        let mut e = good();
        e.maintainers.push("bad_login".into());
        fails(&e, "maintainer \"bad_login\"");
    }

    #[test]
    fn id_rules() {
        let mut e = good();
        e.owner_id = 0;
        fails(&e, "owner_id");
        let mut e = good();
        e.repo_id = -5;
        fails(&e, "repo_id");
    }

    #[test]
    fn every_licence_on_the_list_passes_and_others_fail() {
        for licence in policy::LICENCES {
            let mut e = good();
            e.license = (*licence).into();
            assert_eq!(check(&e), Ok(()), "{licence}");
        }
        for bad in [
            "GPL-3.0-only",
            "AGPL-3.0-or-later",
            "MPL-2.0",
            "CC-BY-SA-4.0",
            "CC-BY-NC-4.0",
            "mit",
            "MIT OR Apache-2.0",
            "",
        ] {
            let mut e = good();
            e.license = bad.into();
            fails(&e, "is not in the licence list");
        }
    }

    #[test]
    fn repo_rules() {
        for ok in [
            "https://github.com/ada/hippo-packs",
            "https://github.com/ada/a",
            "https://github.com/ada/_x",
            "https://github.com/ada/a.b-c_d",
            &format!("https://github.com/ada/{}", "r".repeat(100)),
        ] {
            let mut e = good();
            e.repo = ok.into();
            assert_eq!(check(&e), Ok(()), "{ok}");
        }
        for bad in [
            "http://github.com/ada/hippo-packs",
            "https://github.com/bob/hippo-packs",
            "https://github.com/ada/",
            "https://github.com/ada",
            "https://github.com/ada/.",
            "https://github.com/ada/..",
            "https://github.com/ada/.hidden",
            "https://github.com/ada/x.git",
            "https://github.com/ada/x/y",
            "https://github.com/ada/x?y",
            "https://github.com/ada/x ",
            "https://gitlab.com/ada/x",
            "https://github.com/adam/x",
            &format!("https://github.com/ada/{}", "r".repeat(101)),
        ] {
            let mut e = good();
            e.repo = bad.into();
            fails(&e, "repo ");
        }
    }

    #[test]
    fn path_rules() {
        for ok in ["", "a", "a/b", "themes/night-ink", "a.b/c_d", "..a", "a..b"] {
            let mut e = good();
            e.path = ok.into();
            assert_eq!(check(&e), Ok(()), "{ok:?}");
        }
        for bad in [
            "/a", "a/", "a//b", ".", "..", "a/./b", "a/../b", "a b", "a\\b", "é",
        ] {
            let mut e = good();
            e.path = bad.into();
            fails(&e, "path ");
        }
    }

    #[test]
    fn commit_and_hash_rules() {
        for bad in [
            "",
            "3F2C1A9E0B4D7C6E5F8A9B0C1D2E3F4A5B6C7D8E",
            &"a".repeat(39),
            &"a".repeat(41),
            &format!("{}g", "a".repeat(39)),
        ] {
            let mut e = good();
            e.commit = bad.into();
            fails(&e, "commit is not 40 lower-case hex digits");
        }
        for bad in ["", &"A".repeat(64), &"a".repeat(63), &"a".repeat(65)] {
            let mut e = good();
            e.pack_hash = bad.into();
            fails(&e, "pack_hash is not 64 lower-case hex digits");
        }
    }

    #[test]
    fn kind_rules() {
        let mut e = good();
        e.kind = "prompts".into();
        fails(&e, "kind \"prompts\" is neither passes nor theme");
    }

    #[test]
    fn pass_rules() {
        let mut e = good();
        e.passes.clear();
        fails(&e, "1 to 20 passes, not 0");
        let mut e = good();
        e.passes = (0..21)
            .map(|i| Pass {
                name: format!("p{i}"),
                reads: vec![],
            })
            .collect();
        fails(&e, "1 to 20 passes, not 21");
        e.passes.truncate(20);
        assert_eq!(check(&e), Ok(()));
        let mut e = good();
        e.passes[0].name = "Bad_Name".into();
        fails(&e, "pass name ");
        let mut e = good();
        e.passes[0].name = "-x".into();
        fails(&e, "pass name ");
        let mut e = good();
        e.passes[0].name = "1-x".into();
        assert_eq!(check(&e), Ok(()));
        let mut e = good();
        e.passes[0].reads = vec!["sessions".into(), "everything".into()];
        fails(&e, "reads \"everything\"");
        let mut e = good();
        e.passes[0].reads = ["sessions", "notes", "turns", "human_confirmed"]
            .map(String::from)
            .to_vec();
        assert_eq!(check(&e), Ok(()));
    }

    #[test]
    fn theme_rules() {
        assert_eq!(check(&theme()), Ok(()));
        let mut e = theme();
        e.passes = good().passes;
        fails(&e, "a theme pack has no passes");
        let mut e = theme();
        e.theme_contract = None;
        e.files.clear();
        assert_eq!(check(&e), Ok(()));
        let mut e = theme();
        e.theme_contract = Some(2);
        e.files.clear();
        assert_eq!(check(&e), Ok(()));
        let mut e = theme();
        e.files[0].path = "other.css".into();
        fails(&e, "needs theme.css among the files");
        let mut e = theme();
        e.files.clear();
        fails(&e, "needs theme.css among the files");
        let mut e = theme();
        e.files[0].bytes = 65_536;
        assert_eq!(check(&e), Ok(()));
        e.files[0].bytes = 65_537;
        fails(&e, "theme.css is 65537 bytes");
        let mut e = theme();
        e.files[0].sha256 = "B".repeat(64);
        fails(&e, "sha256 must be 64 lower-case hex digits");
        let mut e = good();
        e.theme_contract = Some(1);
        fails(&e, "a pack of passes has no theme_contract");
    }

    #[test]
    fn reasons_are_short_even_for_long_input() {
        let mut e = good();
        e.license = "x".repeat(5_000);
        let why = check(&e).expect_err("fails");
        assert!(why[0].len() < 120, "{}", why[0]);
        assert!(why[0].contains('…'));
    }

    #[test]
    fn a_pack_with_several_faults_lists_them_all_in_table_order() {
        let mut e = good();
        e.name = "-x".into();
        e.version = "1".into();
        e.license = "GPL-3.0-only".into();
        let why = check(&e).expect_err("fails");
        assert_eq!(why.len(), 3);
        assert!(why[0].starts_with("name "));
        assert!(why[1].starts_with("version "));
        assert!(why[2].starts_with("license "));
    }

    // --- links and domains ---

    #[test]
    fn the_source_link_and_the_domain() {
        let e = good();
        assert_eq!(
            e.source_link().as_deref(),
            Some(
                "https://github.com/ada/hippo-packs/tree/3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e/glossary-terms"
            )
        );
        assert_eq!(e.domain(), "github.com/ada");
        let mut e = good();
        e.path.clear();
        assert_eq!(
            e.source_link().as_deref(),
            Some(
                "https://github.com/ada/hippo-packs/tree/3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e"
            )
        );
        e.repo = "https://example.com/x".into();
        assert_eq!(e.source_link(), None);
    }

    #[test]
    fn the_theme_css_url() {
        let e = theme();
        let commit = &e.commit;
        assert_eq!(
            theme_css_url("https://raw.example", &e).unwrap(),
            format!("https://raw.example/ada/hippo-packs/{commit}/themes/x/theme.css")
        );
        assert_eq!(
            theme_css_url("https://raw.example/", &e).unwrap(),
            format!("https://raw.example/ada/hippo-packs/{commit}/themes/x/theme.css")
        );
        let mut e = theme();
        e.path.clear();
        assert_eq!(
            theme_css_url("http://127.0.0.1:9200", &e).unwrap(),
            format!(
                "http://127.0.0.1:9200/ada/hippo-packs/{}/theme.css",
                e.commit
            )
        );
    }

    // --- PackRow and the packs table ---

    #[test]
    fn a_pack_row_round_trips_through_the_packs_table() {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        conn.execute_batch(include_str!("../migrations/0001_init.sql"))
            .expect("schema");
        let row = PackRow::from_entry(&good(), true, "2026-11-08T12:00:00.000000Z");
        conn.execute(
            "INSERT INTO packs (name, kind, version, description, authors, owner, owner_id, \
             maintainers, first_party, license, repo, repo_id, path, commit_id, pack_hash, \
             passes, theme_contract, theme_sha256, review_id, review_at, listed, synced_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
            rusqlite::params![
                row.name,
                row.kind.as_str(),
                row.version,
                row.description,
                row.authors_json(),
                row.owner,
                row.owner_id,
                row.maintainers_json(),
                row.first_party,
                row.license,
                row.repo,
                row.repo_id,
                row.path,
                row.commit_id,
                row.pack_hash,
                row.passes_json(),
                row.theme_contract,
                row.theme_sha256,
                row.review_id,
                row.review_at,
                row.listed,
                row.synced_at,
            ],
        )
        .expect("insert");
        let back = conn
            .query_row(
                &format!("SELECT {} FROM packs", PackRow::COLUMNS),
                [],
                PackRow::from_row,
            )
            .expect("select");
        assert_eq!(back, row);
        assert_eq!(back.domain(), "github.com/ada");
        assert!(back.source_link().unwrap().ends_with("/glossary-terms"));
    }
}

//! `hippocamp-registry check`: the six steps of the check, run per
//! entry. `build` runs the same steps through [`assess`].

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Duration;

use anyhow::{bail, Context};
use hippocamp_core::packs::files::{pack_hash, PackFiles};
use hippocamp_core::packs::manifest::{Kind, Manifest};
use hippocamp_core::packs::registry::{confusable, listing_review, Entry, Settings};
use hippocamp_core::packs::review::PackReview;
use hippocamp_core::packs::source::{self, Fetched, GitOptions};
use hippocamp_core::packs::testing;
use hippocamp_core::packs::validate::{self, Validation};

#[derive(clap::Args)]
pub struct Args {
    /// Skip the owner check and the entries outside this repository
    #[arg(long)]
    pub offline: bool,
    /// Check only this entry
    #[arg(long)]
    pub name: Option<String>,
}

/// How serious a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

/// An entry that passed every check, with what the index needs.
pub struct Listed {
    pub entry: Entry,
    pub manifest: Manifest,
    pub validation: Validation,
    pub files: PackFiles,
    pub pack_hash: String,
    pub review: PackReview,
}

/// What the checks found for one entry.
pub struct Assessed {
    pub name: String,
    pub lines: Vec<(Level, String)>,
    /// Not fetched because of `--offline`.
    pub skipped: bool,
    /// `Some` when the entry has no error and is listable.
    pub listed: Option<Listed>,
}

impl Assessed {
    pub fn has_error(&self) -> bool {
        self.lines.iter().any(|(level, _)| *level == Level::Error)
    }
}

/// The result of checking a registry.
pub struct Assessment {
    pub settings: Settings,
    /// Problems with files that are not an entry, such as a review that
    /// does not parse. Each is an error.
    pub problems: Vec<String>,
    pub results: Vec<Assessed>,
}

impl Assessment {
    /// The listed packs, in entry order (the entries' file names).
    pub fn listed(&self) -> impl Iterator<Item = &Listed> {
        self.results.iter().filter_map(|r| r.listed.as_ref())
    }

    pub fn is_ok(&self) -> bool {
        self.problems.is_empty() && !self.results.iter().any(Assessed::has_error)
    }

    pub fn first_party(&self, entry: &Entry) -> bool {
        self.settings.first_party.iter().any(|o| o == entry.owner())
    }
}

/// `text` with every control, line-separator and bidirectional character
/// written as `\u{..}`, so a name or a message from a pack, an index or a
/// review cannot start a line or move the cursor.
pub fn escaped(text: &str) -> String {
    hippocamp_core::packs::escape(text)
}

pub fn git_options() -> GitOptions {
    GitOptions {
        allow_file: false,
        timeout: Duration::from_secs(120),
    }
}

/// A file of `registry/` that is not an entry or a review.
fn is_placeholder(name: &str) -> bool {
    name == ".gitkeep"
}

/// Reads `registry/registry.toml`.
pub fn load_settings(root: &Path) -> anyhow::Result<Settings> {
    let path = root.join("registry/registry.toml");
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Settings::parse(&text).map_err(|why| anyhow::anyhow!("registry/registry.toml: {why}"))
}

/// The entries of `registry/packs/`, in file-name order: each parsed, or
/// the stem and the reason it does not.
pub fn load_entries(root: &Path) -> anyhow::Result<Vec<(String, Result<Entry, String>)>> {
    let dir = root.join("registry/packs");
    let mut names = Vec::new();
    for item in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let name = item?.file_name().to_string_lossy().into_owned();
        if !is_placeholder(&name) {
            names.push(name);
        }
    }
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let Some(stem) = name.strip_suffix(".toml") else {
            out.push((
                name.clone(),
                Err("registry/packs holds only <name>.toml entry files".to_owned()),
            ));
            continue;
        };
        let parsed = fs::read_to_string(dir.join(&name))
            .map_err(|e| e.to_string())
            .and_then(|text| Entry::parse(stem, &text));
        out.push((stem.to_owned(), parsed));
    }
    Ok(out)
}

/// Every review of `registry/reviews/`, in file-name order, and the
/// problems with files that do not read.
fn load_reviews(root: &Path) -> anyhow::Result<(Vec<PackReview>, Vec<String>)> {
    let dir = root.join("registry/reviews");
    let mut names = Vec::new();
    for item in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let name = item?.file_name().to_string_lossy().into_owned();
        if !is_placeholder(&name) {
            names.push(name);
        }
    }
    names.sort();
    let (mut reviews, mut problems) = (Vec::new(), Vec::new());
    for name in names {
        match PackReview::load(&dir.join(&name)) {
            Ok(review) => reviews.push(review),
            Err(err) => problems.push(format!("registry/reviews/{name}: {err}")),
        }
    }
    Ok((reviews, problems))
}

/// Fetches the pinned content of `entry`: from the local clone when `repo`
/// is the settings' `repo` (the commit must be on `origin/main`), else
/// from git. `Ok(None)` when `offline` and the entry is outside the clone.
pub fn fetch_pin(
    root: &Path,
    settings: &Settings,
    entry: &Entry,
    offline: bool,
) -> Result<Option<Fetched>, String> {
    let options = git_options();
    if entry.repo == settings.repo {
        let ancestor = source::run_git(
            Some(root),
            &["merge-base", "--is-ancestor", &entry.commit, "origin/main"],
            None,
            &options,
        )
        .map_err(|e| e.to_string())?;
        if ancestor.status != 0 {
            return Err(format!("commit {} is not on origin/main", entry.commit));
        }
        return source::read_tree(root, &entry.commit, &entry.path, &options)
            .map(Some)
            .map_err(|e| e.to_string());
    }
    if offline {
        return Ok(None);
    }
    let fetched = source::fetch_git(&entry.repo, &entry.path, Some(&entry.commit), &options)
        .map_err(|e| e.to_string())?;
    if fetched.commit.as_deref() != Some(entry.commit.as_str()) {
        return Err(format!(
            "fetching {} did not give that commit",
            entry.commit
        ));
    }
    Ok(Some(fetched))
}

/// An error for a different `id`, `owner.id` or `full_name` (ASCII case
/// ignored) in `gh api repos/<owner>/<repo>`'s answer, and a warning for a
/// private repository.
pub fn compare_owner(entry: &Entry, repo: &serde_json::Value) -> Vec<(Level, String)> {
    let mut lines = Vec::new();
    let id = repo.get("id").and_then(serde_json::Value::as_u64);
    if id != Some(entry.repo_id) {
        lines.push((
            Level::Error,
            format!(
                "repository id is {} but the entry says {}",
                id.map_or_else(|| "missing".to_owned(), |i| i.to_string()),
                entry.repo_id
            ),
        ));
    }
    let owner_id = repo
        .get("owner")
        .and_then(|o| o.get("id"))
        .and_then(serde_json::Value::as_u64);
    if owner_id != Some(entry.owner_id) {
        lines.push((
            Level::Error,
            format!(
                "owner id is {} but the entry says {}",
                owner_id.map_or_else(|| "missing".to_owned(), |i| i.to_string()),
                entry.owner_id
            ),
        ));
    }
    let wanted = entry.repo.trim_start_matches("https://github.com/");
    let full_name = repo
        .get("full_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if !full_name.eq_ignore_ascii_case(wanted) {
        lines.push((
            Level::Error,
            format!("the repository is now {full_name}, not {wanted}"),
        ));
    }
    if repo.get("private") == Some(&serde_json::Value::Bool(true)) {
        lines.push((
            Level::Warning,
            "the repository is private, so others cannot install from it".to_owned(),
        ));
    }
    lines
}

/// Step 6: `gh api repos/<owner>/<repo>`.
fn check_owner(entry: &Entry) -> Vec<(Level, String)> {
    let path = format!(
        "repos/{}",
        entry.repo.trim_start_matches("https://github.com/")
    );
    let output = match Command::new("gh").args(["api", &path]).output() {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return vec![(
                Level::Error,
                "gh is not installed; install it or run with --offline".to_owned(),
            )];
        }
        Err(err) => return vec![(Level::Error, format!("running gh: {err}"))],
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let why = stderr.lines().next().unwrap_or("it failed");
        return vec![(Level::Error, format!("gh api {path}: {why}"))];
    }
    match serde_json::from_slice::<serde_json::Value>(&output.stdout) {
        Ok(json) => compare_owner(entry, &json),
        Err(err) => vec![(Level::Error, format!("gh api {path}: not JSON: {err}"))],
    }
}

/// What step 2 found in the fetched pack.
struct Examined {
    files: PackFiles,
    validation: Validation,
}

/// Step 2: validation, the manifest's name and licence, a `LICENSE` file,
/// and, for a pass pack, the fixture tests.
fn examine(
    settings: &Settings,
    entry: &Entry,
    fetched: Fetched,
    lines: &mut Vec<(Level, String)>,
) -> Examined {
    let files = fetched.files;
    let validation = validate::validate(&files);
    let mut error = |why: String| lines.push((Level::Error, why));
    for why in &validation.errors {
        error(why.clone());
    }
    if let Some(manifest) = &validation.manifest {
        if manifest.name != entry.name {
            error(format!(
                "pack.toml: name {} is not the entry's name {}",
                manifest.name, entry.name
            ));
        }
        if !settings.licenses.contains(&manifest.license) {
            error(format!(
                "pack.toml: license {} is not in the registry's licenses",
                manifest.license
            ));
        }
    }
    if files.get("LICENSE").is_none() && files.get("LICENSE.md").is_none() {
        error("LICENSE: missing (LICENSE or LICENSE.md)".to_owned());
    }
    // A theme pack has no passes and needs no test case.
    let theme = validation
        .manifest
        .as_ref()
        .is_some_and(|m| m.kind == Kind::Theme);
    if validation.is_ok() && !theme {
        if validation.cases.is_empty() {
            error("tests/: a pass pack needs at least one fixture test".to_owned());
        }
        let stub = std::env::current_exe()
            .map(|exe| [exe.to_string_lossy().into_owned(), "stub-model".to_owned()]);
        for (case_name, case) in &validation.cases {
            if !case.listable() {
                error(format!(
                    "tests/{case_name}: the case must set calls and either applied or a [[expect.note]]"
                ));
                continue;
            }
            let outcome = match &stub {
                Ok(stub) => testing::run_case(&files, case_name, case, stub)
                    .map(|o| o.failures)
                    .unwrap_or_else(|e| vec![e.to_string()]),
                Err(err) => vec![format!("finding this executable: {err}")],
            };
            for failure in outcome {
                error(format!("tests/{case_name}: {failure}"));
            }
        }
    }
    Examined { files, validation }
}

/// Runs the steps of `hippocamp-registry check` for the entries (or `only`).
pub fn assess(root: &Path, offline: bool, only: Option<&str>) -> anyhow::Result<Assessment> {
    let settings = load_settings(root)?;
    let entries = load_entries(root)?;
    let (reviews, problems) = load_reviews(root)?;
    if let Some(only) = only {
        if !entries.iter().any(|(name, _)| name == only) {
            bail!("no entry named {only} in registry/packs/");
        }
    }
    let selected = |name: &str| only.is_none_or(|o| o == name);

    let mut results: Vec<Assessed> = Vec::new();
    let mut fetched: BTreeMap<String, Examined> = BTreeMap::new();
    let mut parsed: Vec<&Entry> = Vec::new();

    for (name, entry) in &entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(why) => {
                if selected(name) {
                    results.push(Assessed {
                        name: name.clone(),
                        lines: vec![(Level::Error, why.clone())],
                        skipped: false,
                        listed: None,
                    });
                }
                continue;
            }
        };
        parsed.push(entry);
        // Steps 2, 3 and 6. An entry that is not selected is read only to
        // compare its pass names, and only from this clone.
        if !selected(name) {
            if entry.repo == settings.repo {
                if let Ok(Some(read)) = fetch_pin(root, &settings, entry, true) {
                    let files = read.files;
                    let validation = validate::validate(&files);
                    fetched.insert(name.clone(), Examined { files, validation });
                }
            }
            continue;
        }
        let mut lines = Vec::new();
        let mut skipped = false;
        let mut listed = None;
        match fetch_pin(root, &settings, entry, offline) {
            Err(why) => lines.push((Level::Error, why)),
            Ok(None) => skipped = true,
            Ok(Some(read)) => {
                let examined = examine(&settings, entry, read, &mut lines);
                let hash = pack_hash(&examined.files);
                match listing_review(&reviews, entry, &hash, &settings) {
                    Err(why) => lines.push((Level::Error, why)),
                    Ok(review) => {
                        if let Some(manifest) = examined.validation.manifest.clone() {
                            listed = Some(Listed {
                                entry: entry.clone(),
                                manifest,
                                validation: examined.validation.clone(),
                                files: examined.files.clone(),
                                pack_hash: hash,
                                review: review.clone(),
                            });
                        }
                    }
                }
                fetched.insert(name.clone(), examined);
            }
        }
        if !offline {
            lines.extend(check_owner(entry));
        }
        results.push(Assessed {
            name: name.clone(),
            lines,
            skipped,
            listed,
        });
    }

    // Step 4: no other entry's name is confusable with it.
    let mut extra: BTreeMap<String, Vec<(Level, String)>> = BTreeMap::new();
    for a in &parsed {
        for b in &parsed {
            if a.name != b.name
                && confusable(&a.name, &b.name)
                && !settings.allows_pair(&a.name, &b.name)
            {
                extra.entry(a.name.clone()).or_default().push((
                    Level::Error,
                    format!("the name is confusable with {}", b.name),
                ));
            }
        }
    }
    // Step 5: no pass name appears in two entries.
    let mut pass_owner: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, examined) in &fetched {
        for (pass, _) in &examined.validation.passes {
            pass_owner.entry(pass).or_default().push(name);
        }
    }
    for (pass, names) in &pass_owner {
        for name in names {
            for other in names.iter().filter(|o| *o != name) {
                extra.entry((*name).to_owned()).or_default().push((
                    Level::Error,
                    format!("pass {pass} is also in entry {other}"),
                ));
            }
        }
    }

    for result in &mut results {
        if let Some(lines) = extra.remove(&result.name) {
            result.lines.extend(lines);
        }
    }
    for result in &mut results {
        if result.has_error() {
            result.listed = None;
        }
    }
    Ok(Assessment {
        settings,
        problems,
        results,
    })
}

pub fn run(root: &Path, args: &Args) -> anyhow::Result<ExitCode> {
    let assessment = assess(root, args.offline, args.name.as_deref())?;
    for problem in &assessment.problems {
        println!("error {}", escaped(problem));
    }
    for result in &assessment.results {
        let name = escaped(&result.name);
        if result.skipped {
            println!("skipped {name}: offline");
            continue;
        }
        for (level, why) in &result.lines {
            let word = match level {
                Level::Error => "error",
                Level::Warning => "warning",
            };
            println!("{word} {name}: {}", escaped(why));
        }
        if !result.has_error() {
            println!("ok {name}");
        }
    }
    let listed: Vec<&Listed> = assessment.listed().collect();
    let first_party = listed
        .iter()
        .filter(|l| assessment.first_party(&l.entry))
        .count();
    println!(
        "{} listed: {first_party} first-party, {} by other owners",
        listed.len(),
        listed.len() - first_party
    );
    Ok(if assessment.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry() -> Entry {
        Entry::parse(
            "glossary-terms",
            "protocol = \"HIPPOCAMP_V1\"\nname = \"glossary-terms\"\n\
             repo = \"https://github.com/alice/hippocamp-packs\"\nrepo_id = 712345678\n\
             owner_id = 1234567\npath = \"glossary-terms\"\n\
             commit = \"3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e\"\nmaintainers = [\"alice\"]\n",
        )
        .unwrap()
    }

    fn github() -> serde_json::Value {
        json!({
            "id": 712_345_678,
            "owner": {"id": 1_234_567},
            "full_name": "alice/hippocamp-packs",
            "private": false
        })
    }

    #[test]
    fn equal_ids_pass() {
        assert!(compare_owner(&entry(), &github()).is_empty());
        let mut cased = github();
        cased["full_name"] = json!("Alice/Hippocamp-Packs");
        assert!(compare_owner(&entry(), &cased).is_empty());
    }

    #[test]
    fn a_different_repository_id_owner_id_or_name_is_an_error() {
        for (key, value) in [
            ("id", json!(1)),
            ("full_name", json!("bob/hippocamp-packs")),
            ("full_name", json!("alice/renamed")),
        ] {
            let mut repo = github();
            repo[key] = value;
            let lines = compare_owner(&entry(), &repo);
            assert_eq!(lines.len(), 1, "{key}: {lines:?}");
            assert_eq!(lines[0].0, Level::Error);
        }
        let mut repo = github();
        repo["owner"]["id"] = json!(7);
        let lines = compare_owner(&entry(), &repo);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, Level::Error);
        assert!(lines[0].1.contains("owner id"), "{lines:?}");
        // A missing field is a mismatch, not a pass.
        let lines = compare_owner(&entry(), &json!({}));
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|(level, _)| *level == Level::Error));
    }

    #[test]
    fn a_private_repository_is_a_warning() {
        let mut repo = github();
        repo["private"] = json!(true);
        let lines = compare_owner(&entry(), &repo);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, Level::Warning);
    }

    #[test]
    fn control_characters_are_escaped() {
        assert_eq!(escaped("a\nb\u{1b}c"), "a\\u{a}b\\u{1b}c");
        // Bidirectional and format characters are hidden too.
        assert_eq!(escaped("a\u{202e}b\u{200b}c"), "a\\u{202e}b\\u{200b}c");
    }
}

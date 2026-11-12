//! The registry's types: its settings, an entry, the index the client
//! reads, the rule that lists a pack by its reviews, and the confusable
//! name check. The
//! client and the registry tool share these definitions.

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::files::check_path;
use super::manifest::{is_pack_name, RESERVED_NAMES};
use super::review::PackReview;
use crate::error::{Error, Result};
use crate::records::{check_protocol, decode, Verdict};

/// The only reviewer a listing counts.
const REVIEWER: &str = "pack-reviewer";

static REPO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://github\.com/([A-Za-z0-9](?:[A-Za-z0-9-]{0,38}))/([A-Za-z0-9_-][A-Za-z0-9._-]{0,99})$")
        .expect("the repository pattern is valid")
});

static LOGIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$").expect("the login pattern is valid")
});

static LICENSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9.+-]{1,64}$").expect("the licence pattern is valid"));

/// The repository's name and owner, when `repo` obeys the registry's
/// repository rule. The error is the rule that fails.
fn check_repo(repo: &str) -> std::result::Result<&str, String> {
    let Some(captures) = REPO.captures(repo) else {
        return Err(
            "repo: must be https://github.com/<owner>/<repository> with a valid owner and repository name"
                .to_owned(),
        );
    };
    let name = &captures[2];
    if name == "." || name == ".." || name.ends_with(".git") {
        return Err("repo: the repository name may not be . or .. or end with .git".to_owned());
    }
    Ok(captures.get(1).map_or("", |m| m.as_str()))
}

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The reason of a TOML error, without its source excerpt.
fn first_line(err: toml::de::Error) -> String {
    err.message().lines().next().unwrap_or_default().to_owned()
}

/// Reads TOML text with a `protocol` key into `T` (which has no
/// `protocol` field), checking the protocol first.
fn parse_toml<T: serde::de::DeserializeOwned>(text: &str) -> std::result::Result<T, String> {
    let mut table: toml::Table = toml::from_str(text).map_err(first_line)?;
    check_protocol(table.remove("protocol").map(|found| match found {
        toml::Value::String(text) => text,
        other => other.to_string(),
    }))
    .map_err(|e| e.to_string())?;
    table.try_into().map_err(first_line)
}

/// `registry/registry.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// This repository: its pins are read from the local clone.
    pub repo: String,
    /// Owners whose repositories are first-party.
    pub first_party: Vec<String>,
    /// Permissive licences a listed pack may use.
    pub licenses: Vec<String>,
    /// Pack-reviewer persona hashes whose reviews count.
    pub personas: Vec<String>,
    /// Name pairs accepted after a look; order ignored.
    #[serde(default)]
    pub confusable_ok: Vec<[String; 2]>,
}

impl Settings {
    /// Parses `registry.toml` and checks its rules. The error is the TOML
    /// parser's, the protocol error, or `<key>: <rule>`.
    pub fn parse(text: &str) -> std::result::Result<Settings, String> {
        let settings: Settings = parse_toml(text)?;
        check_repo(&settings.repo)?;
        if settings.first_party.iter().any(|o| !LOGIN.is_match(o)) {
            return Err("first_party: each owner must be a GitHub login".to_owned());
        }
        if settings.licenses.iter().any(|l| !LICENSE.is_match(l)) {
            return Err("licenses: each must match ^[A-Za-z0-9.+-]{1,64}$".to_owned());
        }
        if settings.personas.iter().any(|p| !is_hex(p, 64)) {
            return Err("personas: each must be 64 lowercase hex characters".to_owned());
        }
        let bad_pair = settings
            .confusable_ok
            .iter()
            .any(|pair| pair.iter().any(|n| !is_pack_name(n)));
        if bad_pair {
            return Err("confusable_ok: each name must be a pack name".to_owned());
        }
        Ok(settings)
    }

    /// Whether the pair is accepted in `confusable_ok`, in either order.
    pub fn allows_pair(&self, a: &str, b: &str) -> bool {
        self.confusable_ok
            .iter()
            .any(|[x, y]| (x == a && y == b) || (x == b && y == a))
    }
}

/// `registry/packs/<name>.toml`, written by the submitter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    pub repo: String,
    /// GitHub's numeric id of the repository.
    pub repo_id: u64,
    /// GitHub's numeric id of the repository's owner.
    pub owner_id: u64,
    /// `""` for the repository root.
    pub path: String,
    /// 40 lowercase hex characters.
    pub commit: String,
    pub maintainers: Vec<String>,
}

impl Entry {
    /// Parses `registry/packs/<stem>.toml` and checks its rules, including
    /// that the file stem equals `name`.
    pub fn parse(stem: &str, text: &str) -> std::result::Result<Entry, String> {
        let entry: Entry = parse_toml(text)?;
        if !is_pack_name(&entry.name)
            || !(2..=40).contains(&entry.name.len())
            || RESERVED_NAMES.contains(&entry.name.as_str())
        {
            return Err(
                "name: must be a pack name of 2 to 40 bytes that is not reserved".to_owned(),
            );
        }
        if entry.name != stem {
            return Err(format!(
                "name: {} does not match the file name {stem}",
                entry.name
            ));
        }
        check_repo(&entry.repo)?;
        if !entry.path.is_empty() {
            check_path(&entry.path).map_err(|e| format!("path: {e}"))?;
        }
        if !is_hex(&entry.commit, 40) {
            return Err("commit: must be 40 lowercase hex characters".to_owned());
        }
        if !(1..=10).contains(&entry.maintainers.len())
            || entry.maintainers.iter().any(|m| !LOGIN.is_match(m))
        {
            return Err("maintainers: needs 1 to 10 GitHub logins".to_owned());
        }
        Ok(entry)
    }

    /// The repository's owner, the first group of `repo`.
    pub fn owner(&self) -> &str {
        self.repo
            .strip_prefix("https://github.com/")
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_default()
    }
}

/// `index.json`: the listed packs, as `hippo-registry build` writes it.
/// Keys the reader does not know are ignored, so the fields only grow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    #[serde(with = "time::serde::rfc3339")]
    pub generated: OffsetDateTime,
    pub counts: Counts,
    pub packs: Vec<IndexPack>,
}

/// How many packs the index lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub listed: u64,
    pub first_party: u64,
    pub other_owners: u64,
}

fn passes_kind() -> String {
    "passes".to_owned()
}

/// One listed pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexPack {
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub owner: String,
    pub maintainers: Vec<String>,
    pub first_party: bool,
    pub license: String,
    pub repo: String,
    pub repo_id: u64,
    pub owner_id: u64,
    pub path: String,
    pub commit: String,
    pub pack_hash: String,
    /// `passes`, or `theme` for a theme pack.
    #[serde(default = "passes_kind")]
    pub kind: String,
    #[serde(default)]
    pub passes: Vec<IndexPass>,
    /// A theme pack's web theme contract.
    #[serde(default)]
    pub theme_contract: Option<u32>,
    /// A theme pack's desktop theme format.
    #[serde(default)]
    pub desktop_theme_format: Option<u32>,
    #[serde(default)]
    pub files: Vec<IndexFile>,
    pub review: IndexReview,
}

/// A pass of a listed pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexPass {
    pub name: String,
    pub reads: Vec<String>,
    pub per_note: bool,
    #[serde(default)]
    pub note_types: Option<Vec<String>>,
    #[serde(default)]
    pub note_paths: Option<Vec<String>>,
}

/// A file of a listed pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The review that lists a pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexReview {
    pub id: String,
    pub verdict: Verdict,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// The most bytes of an index file the client reads.
pub const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

impl IndexPack {
    /// Checks what the client is about to use against the rules an `Entry`
    /// obeys: a pack name of 2 to 40 bytes that is not reserved, the GitHub
    /// repository pattern (with `allow_file`, tests only, a plain `file://`
    /// URL too), the path rules, 40 lowercase hex characters of commit and 64
    /// of pack hash. An index is data from outside; nothing in it reaches
    /// `git` or the terminal before this passes. The error is `<key>: <rule>`.
    pub fn check(&self, allow_file: bool) -> std::result::Result<(), String> {
        if !is_pack_name(&self.name)
            || !(2..=40).contains(&self.name.len())
            || RESERVED_NAMES.contains(&self.name.as_str())
        {
            return Err(
                "name: must be a pack name of 2 to 40 bytes that is not reserved".to_owned(),
            );
        }
        let plain_file = allow_file
            && self.repo.starts_with("file://")
            && !self.repo.starts_with("file://-")
            && !self.repo.chars().any(char::is_control)
            && !self.repo.contains(['?', '#', '%']);
        if !plain_file {
            check_repo(&self.repo)?;
        }
        if !self.path.is_empty() {
            check_path(&self.path).map_err(|e| format!("path: {e}"))?;
        }
        if !is_hex(&self.commit, 40) {
            return Err("commit: must be 40 lowercase hex characters".to_owned());
        }
        if !is_hex(&self.pack_hash, 64) {
            return Err("pack_hash: must be 64 lowercase hex characters".to_owned());
        }
        Ok(())
    }
}

impl Index {
    /// Reads an index file of at most [`MAX_INDEX_BYTES`], refusing a
    /// missing or different `protocol`.
    pub fn load(path: &Path) -> Result<Index> {
        use std::io::Read as _;
        let file = fs::File::open(path).map_err(|e| Error::io_at(path, e))?;
        let mut bytes = Vec::new();
        file.take(MAX_INDEX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| Error::io_at(path, e))?;
        if bytes.len() as u64 > MAX_INDEX_BYTES {
            return Err(Error::Pack(format!(
                "{}: over the {MAX_INDEX_BYTES}-byte limit for an index",
                path.display()
            )));
        }
        decode(&bytes).map_err(|e| Error::Pack(format!("{}: {e}", path.display())))
    }

    /// The listed pack named `name`, exactly.
    pub fn find(&self, name: &str) -> Option<&IndexPack> {
        self.packs.iter().find(|p| p.name == name)
    }
}

/// The review that lists `entry` at `pack_hash`: a review whose pack, hash
/// and repository id equal the entry's, by `pack-reviewer` with a persona
/// hash in `settings.personas` and a `confirm` verdict, and no review of
/// the same pack, hash and repository id with a `reject`. Of several
/// confirming reviews, the newest. The error says which rule failed.
pub fn listing_review<'a>(
    reviews: &'a [PackReview],
    entry: &Entry,
    pack_hash: &str,
    settings: &Settings,
) -> std::result::Result<&'a PackReview, String> {
    let same: Vec<&PackReview> = reviews
        .iter()
        .filter(|r| {
            r.pack == entry.name && r.pack_hash == pack_hash && r.repo_id == Some(entry.repo_id)
        })
        .collect();
    if let Some(rejected) = same.iter().find(|r| r.verdict == Verdict::Reject) {
        return Err(format!(
            "{}: review {} rejected this content; change it or delete the reject in a reviewed pull request",
            entry.name, rejected.id
        ));
    }
    same.into_iter()
        .filter(|r| {
            r.reviewer == REVIEWER
                && r.verdict == Verdict::Confirm
                && settings.personas.contains(&r.persona_hash)
        })
        .max_by_key(|r| r.at)
        .ok_or_else(|| {
            format!(
                "{}: no confirming review of this content by an accepted persona",
                entry.name
            )
        })
}

/// A name lowercased, with `-` deleted and `0 1 3 5 7` mapped to
/// `o l e s t`.
pub fn normalise(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '-')
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            '0' => 'o',
            '1' => 'l',
            '3' => 'e',
            '5' => 's',
            '7' => 't',
            other => other,
        })
        .collect()
}

/// The optimal-string-alignment (restricted Damerau–Levenshtein) distance
/// between `a` and `b`, in characters.
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let width = b.len() + 1;
    let mut d = vec![0usize; (a.len() + 1) * width];
    for i in 0..=a.len() {
        d[i * width] = i;
    }
    for (j, cell) in d.iter_mut().enumerate().take(width) {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[(i - 1) * width + j] + 1)
                .min(d[i * width + j - 1] + 1)
                .min(d[(i - 1) * width + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[(i - 2) * width + j - 2] + 1);
            }
            d[i * width + j] = best;
        }
    }
    d[a.len() * width + b.len()]
}

/// Whether two names are confusable: equal once normalised, or within
/// distance 1 when the longer normalised form has at most 8 characters and
/// within distance 2 otherwise.
pub fn confusable(a: &str, b: &str) -> bool {
    let (a, b) = (normalise(a), normalise(b));
    if a == b {
        return true;
    }
    let longer = a.chars().count().max(b.chars().count());
    distance(&a, &b) <= if longer <= 8 { 1 } else { 2 }
}

/// The listed names whose normalised form is within distance 2 of
/// `name`'s, nearest first and by name among equals.
pub fn suggest(index: &Index, name: &str) -> Vec<String> {
    let wanted = normalise(name);
    let mut near: Vec<(usize, &str)> = index
        .packs
        .iter()
        .filter_map(|p| {
            let d = distance(&normalise(&p.name), &wanted);
            (d <= 2).then_some((d, p.name.as_str()))
        })
        .collect();
    near.sort();
    near.into_iter().map(|(_, n)| n.to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::encode;

    const COMMIT: &str = "3f2c1a9e0b4d7c6e5f8a9b0c1d2e3f4a5b6c7d8e";

    fn entry_text(extra: &str) -> String {
        format!(
            "protocol = \"HIPPO_V1\"\nname = \"glossary-terms\"\n\
             repo = \"https://github.com/alice/hippo-packs\"\nrepo_id = 712345678\n\
             owner_id = 1234567\npath = \"glossary-terms\"\ncommit = \"{COMMIT}\"\n\
             maintainers = [\"alice\"]\n{extra}"
        )
    }

    fn settings_text() -> String {
        format!(
            "protocol = \"HIPPO_V1\"\nrepo = \"https://github.com/VanitasCaesar1/hippo\"\n\
             first_party = [\"VanitasCaesar1\"]\nlicenses = [\"MIT\", \"Apache-2.0\"]\n\
             personas = [\"{}\"]\nconfusable_ok = [[\"a-b\", \"ab\"]]\n",
            "a".repeat(64)
        )
    }

    #[test]
    fn an_entry_parses_and_names_its_owner() {
        let entry = Entry::parse("glossary-terms", &entry_text("")).unwrap();
        assert_eq!(entry.owner(), "alice");
        assert_eq!(entry.repo_id, 712_345_678);
        assert_eq!(entry.path, "glossary-terms");
        let root = entry_text("").replace("path = \"glossary-terms\"", "path = \"\"");
        assert_eq!(Entry::parse("glossary-terms", &root).unwrap().path, "");
    }

    #[test]
    fn an_entry_that_breaks_a_rule_is_refused_with_the_key() {
        let swap = |from: &str, to: &str| entry_text("").replace(from, to);
        let cases = [
            (entry_text("unknown = 1\n"), "unknown field"),
            (swap("HIPPO_V1", "HIPPO_V0"), "protocol"),
            (
                entry_text("").replace("protocol = \"HIPPO_V1\"\n", ""),
                "protocol",
            ),
            (
                swap("name = \"glossary-terms\"", "name = \"Glossary\""),
                "name:",
            ),
            (
                swap("name = \"glossary-terms\"", "name = \"core\""),
                "name:",
            ),
            (
                swap("https://github.com/alice", "https://gitlab.com/alice"),
                "repo:",
            ),
            (swap("alice/hippo-packs", "alice/hippo-packs.git"), "repo:"),
            (swap("alice/hippo-packs", "alice/.."), "repo:"),
            (
                swap("https://github.com/alice", "https://github.com/-alice"),
                "repo:",
            ),
            (
                swap("\"glossary-terms\"\ncommit", "\"../x\"\ncommit"),
                "path:",
            ),
            (swap(COMMIT, &COMMIT.to_uppercase()), "commit:"),
            (swap(COMMIT, "3f2c1a9"), "commit:"),
            (swap("[\"alice\"]", "[]"), "maintainers:"),
            (swap("[\"alice\"]", "[\"-a\"]"), "maintainers:"),
            (
                swap("[\"alice\"]", &format!("[{}]", ["\"a\""; 11].join(", "))),
                "maintainers:",
            ),
            (
                swap("repo_id = 712345678", "repo_id = \"x\""),
                "invalid type",
            ),
        ];
        for (text, want) in cases {
            let err = Entry::parse("glossary-terms", &text).expect_err(&text);
            assert!(err.contains(want), "{want}: {err}");
        }
        let err = Entry::parse("other", &entry_text("")).unwrap_err();
        assert!(err.contains("does not match the file name other"), "{err}");
    }

    #[test]
    fn settings_parse_and_check_their_rules() {
        let settings = Settings::parse(&settings_text()).unwrap();
        assert_eq!(settings.first_party, ["VanitasCaesar1"]);
        assert!(settings.allows_pair("ab", "a-b"));
        assert!(!settings.allows_pair("ab", "ac"));
        let no_pairs = settings_text().replace("confusable_ok = [[\"a-b\", \"ab\"]]\n", "");
        assert!(Settings::parse(&no_pairs).unwrap().confusable_ok.is_empty());

        let swap = |from: &str, to: &str| settings_text().replace(from, to);
        let cases = [
            (swap("hippo\"", "hippo\"\nextra = 1"), "unknown field"),
            (swap("HIPPO_V1", "x"), "protocol"),
            (
                swap(
                    "https://github.com/VanitasCaesar1/hippo",
                    "https://example.com/a/b",
                ),
                "repo:",
            ),
            (swap("[\"MIT\", ", "[\"M IT\", "), "licenses:"),
            (swap(&"a".repeat(64), "abc"), "personas:"),
            (
                swap("[\"a-b\", \"ab\"]", "[\"A\", \"ab\"]"),
                "confusable_ok:",
            ),
            (swap("[\"VanitasCaesar1\"]", "[\"-x\"]"), "first_party:"),
        ];
        for (text, want) in cases {
            let err = Settings::parse(&text).expect_err(&text);
            assert!(err.contains(want), "{want}: {err}");
        }
    }

    #[test]
    fn confusable_names() {
        for (a, b) in [
            ("decision-log", "decisionlog"),
            ("gotchas", "g0tchas"),
            ("glossary-terms", "glossary-term"),
            ("deploys", "deploy5"),
            ("api-drift", "api-dirft"),
            ("a-b-long-pack-name", "a-b-long-pack-nam"),
            ("a-b-long-pack-name", "a-b-long-pack-nome"),
        ] {
            assert!(confusable(a, b), "{a} {b}");
            assert!(confusable(b, a), "{b} {a}");
        }
        for (a, b) in [
            ("deploy-log", "decision-log"),
            ("gotchas", "gotchass-xy"),
            ("abcdefgh", "abcdefxy"),
            ("a-b-long-pack-name", "a-b-long-pack-xyz"),
        ] {
            assert!(!confusable(a, b), "{a} {b}");
        }
        // Distance 2 is allowed only past 8 characters.
        assert!(!confusable("abcdefgh", "abcdexyh"));
        assert!(confusable("abcdefghi", "abcdexyhi"));
    }

    #[test]
    fn normalise_and_distance() {
        assert_eq!(normalise("Gl05-5ary"), "glossary");
        assert_eq!(normalise("d3p-10y"), "deploy");
        assert_eq!(normalise("7357"), "test");
        assert_eq!(distance("", ""), 0);
        assert_eq!(distance("abc", ""), 3);
        assert_eq!(distance("kitten", "sitting"), 3);
        // A swap of neighbours counts once (optimal string alignment).
        assert_eq!(distance("ab", "ba"), 1);
        assert_eq!(distance("ca", "abc"), 3);
    }

    fn index_with(names: &[&str]) -> Index {
        Index {
            generated: OffsetDateTime::from_unix_timestamp(1_794_000_000).unwrap(),
            counts: Counts {
                listed: names.len() as u64,
                first_party: 0,
                other_owners: names.len() as u64,
            },
            packs: names.iter().map(|n| pack(n)).collect(),
        }
    }

    fn pack(name: &str) -> IndexPack {
        IndexPack {
            name: name.into(),
            version: "1.0.0".into(),
            description: "d".into(),
            authors: vec!["Ada".into()],
            owner: "alice".into(),
            maintainers: vec!["alice".into()],
            first_party: false,
            license: "MIT".into(),
            repo: "https://github.com/alice/hippo-packs".into(),
            repo_id: 1,
            owner_id: 2,
            path: name.into(),
            commit: COMMIT.into(),
            pack_hash: "b".repeat(64),
            kind: "passes".into(),
            passes: vec![IndexPass {
                name: name.into(),
                reads: vec!["notes".into()],
                per_note: true,
                note_types: None,
                note_paths: Some(vec!["**/G*.md".into()]),
            }],
            theme_contract: None,
            desktop_theme_format: None,
            files: vec![IndexFile {
                path: "README.md".into(),
                bytes: 3,
                sha256: "c".repeat(64),
            }],
            review: IndexReview {
                id: "01JCA5ZB3K9V7Q2M4N6P8R0T1W".into(),
                verdict: Verdict::Confirm,
                at: OffsetDateTime::from_unix_timestamp(1_794_000_000).unwrap(),
            },
        }
    }

    #[test]
    fn an_index_round_trips_and_ignores_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let index = index_with(&["glossary-terms", "gotchas"]);
        let path = dir.path().join("index.json");
        fs::write(&path, encode(&index).unwrap()).unwrap();
        assert_eq!(Index::load(&path).unwrap(), index);

        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["future"] = serde_json::json!({"x": 1});
        value["packs"][0]["withdrawn"] = serde_json::json!(true);
        value["packs"][1].as_object_mut().unwrap().remove("kind");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let loaded = Index::load(&path).unwrap();
        assert_eq!(loaded, index);
        assert_eq!(loaded.packs[1].kind, "passes");

        value["packs"][0]["kind"] = serde_json::json!("theme");
        value["packs"][0]["theme_contract"] = serde_json::json!(1);
        value["packs"][0]["desktop_theme_format"] = serde_json::json!(1);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let loaded = Index::load(&path).unwrap();
        assert_eq!(loaded.packs[0].kind, "theme");
        assert_eq!(loaded.packs[0].theme_contract, Some(1));
        assert_eq!(loaded.packs[0].desktop_theme_format, Some(1));

        value["protocol"] = serde_json::json!("HIPPO_V0");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(Index::load(&path).is_err());
        assert!(Index::load(&dir.path().join("missing.json")).is_err());
    }

    #[test]
    fn an_index_pack_obeys_the_entry_rules_before_the_client_uses_it() {
        assert!(pack("glossary-terms").check(false).is_ok());
        let mut root = pack("glossary-terms");
        root.path = String::new();
        assert!(root.check(false).is_ok());

        let cases = [
            ("name", "Glossary".to_owned(), "name:"),
            ("name", "core".to_owned(), "name:"),
            ("repo", "https://gitlab.com/alice/x".to_owned(), "repo:"),
            ("repo", "https://github.com/alice/x.git".to_owned(), "repo:"),
            ("repo", "https://github.com/alice/x\n".to_owned(), "repo:"),
            ("repo", "https://u@github.com/alice/x".to_owned(), "repo:"),
            ("repo", "file:///tmp/x".to_owned(), "repo:"),
            ("path", "../x".to_owned(), "path:"),
            ("commit", "a:b".to_owned(), "commit:"),
            ("commit", COMMIT.to_uppercase(), "commit:"),
            ("commit", "refs/heads/main".to_owned(), "commit:"),
            ("pack_hash", "b".repeat(63), "pack_hash:"),
        ];
        for (field, value, want) in cases {
            let mut p = pack("glossary-terms");
            match field {
                "name" => p.name = value,
                "repo" => p.repo = value,
                "path" => p.path = value,
                "commit" => p.commit = value,
                _ => p.pack_hash = value,
            }
            let err = p.check(false).unwrap_err();
            assert!(err.starts_with(want), "{want}: {err}");
        }
        // A file URL passes only when the caller allows it (tests).
        let mut file = pack("glossary-terms");
        file.repo = "file:///tmp/x".into();
        assert!(file.check(true).is_ok());
        file.repo = "file://-x".into();
        assert!(file.check(true).is_err());
        file.repo = "file:///tmp/x\n".into();
        assert!(file.check(true).is_err());
    }

    #[test]
    fn an_index_over_the_size_limit_is_refused_unread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.json");
        fs::write(&path, vec![b' '; MAX_INDEX_BYTES as usize + 1]).unwrap();
        let err = Index::load(&path).unwrap_err().to_string();
        assert!(
            err.contains("over the 4194304-byte limit for an index"),
            "{err}"
        );
    }

    #[test]
    fn suggest_lists_near_names_nearest_first() {
        let index = index_with(&["glossary-term", "glossary-terms", "gotchas", "decision-log"]);
        assert_eq!(
            suggest(&index, "glossary-terms"),
            ["glossary-terms", "glossary-term"]
        );
        assert_eq!(
            suggest(&index, "glosary-term"),
            ["glossary-term", "glossary-terms"]
        );
        assert_eq!(suggest(&index, "g0tcha"), ["gotchas"]);
        assert!(suggest(&index, "unrelated").is_empty());
    }

    fn entry() -> Entry {
        Entry::parse("glossary-terms", &entry_text("")).unwrap()
    }

    fn review(id: &str, verdict: Verdict, at: i64) -> PackReview {
        PackReview {
            id: id.into(),
            pack: "glossary-terms".into(),
            version: "1.0.0".into(),
            source: "s".into(),
            commit: Some(COMMIT.into()),
            repo_id: Some(712_345_678),
            pack_hash: "b".repeat(64),
            reviewer: "pack-reviewer".into(),
            persona_hash: "a".repeat(64),
            invocation: "claude -p".into(),
            warnings: vec![],
            verdict,
            notes: "n".into(),
            findings: vec![],
            at: OffsetDateTime::from_unix_timestamp(at).unwrap(),
        }
    }

    #[test]
    fn a_listing_review_confirms_and_a_reject_blocks() {
        let settings = Settings::parse(&settings_text()).unwrap();
        let hash = "b".repeat(64);
        let confirm = review("c1", Verdict::Confirm, 10);
        let found = listing_review(std::slice::from_ref(&confirm), &entry(), &hash, &settings);
        assert_eq!(found.unwrap().id, "c1");

        // The newest confirming review is the one that lists.
        let reviews = [
            review("c1", Verdict::Confirm, 10),
            review("c2", Verdict::Confirm, 20),
        ];
        assert_eq!(
            listing_review(&reviews, &entry(), &hash, &settings)
                .unwrap()
                .id,
            "c2"
        );

        // A reject blocks, even with a later confirm.
        let reviews = [
            review("r1", Verdict::Reject, 10),
            review("c1", Verdict::Confirm, 20),
        ];
        let err = listing_review(&reviews, &entry(), &hash, &settings).unwrap_err();
        assert!(err.contains("r1 rejected"), "{err}");

        // A reject of other content or another repository does not.
        let mut other_hash = review("r1", Verdict::Reject, 10);
        other_hash.pack_hash = "d".repeat(64);
        let mut other_repo = review("r2", Verdict::Reject, 10);
        other_repo.repo_id = Some(1);
        let reviews = [other_hash, other_repo, confirm.clone()];
        assert!(listing_review(&reviews, &entry(), &hash, &settings).is_ok());

        // An unaccepted persona hash, another reviewer, another hash, another
        // repository id and a missing one do not count.
        let mut persona = confirm.clone();
        persona.persona_hash = "e".repeat(64);
        let mut reviewer = confirm.clone();
        reviewer.reviewer = "someone".into();
        let mut hashed = confirm.clone();
        hashed.pack_hash = "d".repeat(64);
        let mut repo = confirm.clone();
        repo.repo_id = Some(99);
        let mut none = confirm.clone();
        none.repo_id = None;
        let mut named = confirm;
        named.pack = "other".into();
        for bad in [persona, reviewer, hashed, repo, none, named] {
            let err = listing_review(&[bad], &entry(), &hash, &settings).unwrap_err();
            assert!(err.contains("no confirming review"), "{err}");
        }
        assert!(listing_review(&[], &entry(), &hash, &settings).is_err());
    }
}

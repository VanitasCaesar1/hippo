//! Which notes a task makes relevant: BM25F over
//! each note's title, headings, path and the start of its body, with a
//! boost for notes that name the repository the session works in.
//!
//! Words match whole, after a light stemming (plurals, `-ing`, `-ed`, a
//! final `e`), so "add" no longer matches "address". A word found in many
//! notes weighs less than one found in few (IDF). Task words that say
//! nothing about the subject ("add", "fix", "the", "into"…) are dropped. A
//! note must score at least three quarters of the IDF of a word found in
//! one note only ([`Params::min_score`]) and half the best note's score,
//! so a task with weak matches gets fewer notes rather than five poor
//! ones: a lone title match passes for a word in up to about an eighth of
//! the notes, a passing mention in one body does not. Ties go to the most
//! recently updated note, then the path.
//!
//! Notes are tokenised at call time from what [`Notes::scan`] already read;
//! no index is read or written.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::Date;

use crate::notes::{Note, Notes};

/// How many notes the relevant part lists at most.
pub const LIMIT: usize = 5;

/// The ranking's knobs. [`Params::default`] is what hippocamp ships; the eval
/// (`hippocamp eval relevant --params`) can try others.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    /// Field weights.
    pub title: f64,
    pub headings: f64,
    pub path: f64,
    pub body: f64,
    /// Bytes of the body read, from its start.
    pub body_bytes: usize,
    /// BM25 term-frequency saturation.
    pub k1: f64,
    /// BM25 length normalisation of the body (the other fields have none).
    pub b: f64,
    /// The weight of the repository's name, matched in a title or path.
    pub repo: f64,
    /// At most this many distinct task words count, in task order.
    pub max_terms: usize,
    /// A picked note scores at least this share of the IDF of a word found
    /// in one note only, so the floor follows the number of notes.
    pub min_score: f64,
    /// … and at least this share of the best note's score.
    pub min_share: f64,
    /// Words shorter than this are dropped.
    pub min_chars: usize,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            title: 3.0,
            headings: 1.5,
            path: 2.0,
            body: 0.5,
            body_bytes: 32 * 1024,
            k1: 1.2,
            b: 0.75,
            repo: 3.0,
            max_terms: 64,
            min_score: 0.75,
            min_share: 0.5,
            min_chars: 3,
        }
    }
}

/// One picked note.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pick {
    /// Relative to the notes directory.
    pub path: PathBuf,
    pub score: f64,
    /// The task's words that matched, strongest first, as the task wrote
    /// them (lower-cased).
    pub words: Vec<String>,
    /// The note's title or path names the session's repository.
    pub names_repo: bool,
}

impl Pick {
    /// A weak pick: it does not name the repository, and
    /// it matched no word of the task or scored under half of `top`, the
    /// best pick's score.
    pub fn is_weak(&self, top: f64) -> bool {
        !self.names_repo && (self.words.is_empty() || self.score < top * 0.5)
    }

    /// Why it was picked, for a person: `names the repo hippocamp`, then the
    /// strongest matched words, at most three.
    pub fn reason(&self, repo: Option<&str>) -> String {
        let mut parts = Vec::new();
        if self.names_repo {
            if let Some(repo) = repo {
                parts.push(format!("names the repo {repo}"));
            }
        }
        if !self.words.is_empty() {
            let words: Vec<&str> = self.words.iter().take(3).map(String::as_str).collect();
            parts.push(format!("matches {}", words.join(", ")));
        }
        if parts.is_empty() {
            "picked for the task".to_owned()
        } else {
            parts.join("; ")
        }
    }
}

/// Task words that say what to do, not what about, and common English.
const STOP: &[&str] = &[
    // English
    "the",
    "and",
    "for",
    "but",
    "not",
    "nor",
    "yet",
    "you",
    "your",
    "yours",
    "our",
    "ours",
    "its",
    "it's",
    "this",
    "that",
    "these",
    "those",
    "there",
    "their",
    "them",
    "then",
    "than",
    "with",
    "without",
    "from",
    "into",
    "onto",
    "over",
    "under",
    "about",
    "after",
    "before",
    "again",
    "also",
    "just",
    "only",
    "very",
    "more",
    "most",
    "less",
    "some",
    "any",
    "all",
    "each",
    "every",
    "both",
    "other",
    "such",
    "same",
    "own",
    "here",
    "how",
    "what",
    "when",
    "where",
    "which",
    "who",
    "whom",
    "why",
    "was",
    "were",
    "are",
    "been",
    "being",
    "has",
    "have",
    "had",
    "having",
    "does",
    "did",
    "doing",
    "can",
    "could",
    "should",
    "would",
    "will",
    "shall",
    "may",
    "might",
    "must",
    "let",
    "lets",
    "let's",
    "please",
    "thanks",
    "thank",
    "okay",
    "yes",
    "now",
    "still",
    "even",
    "one",
    "two",
    "via",
    "per",
    "etc",
    "out",
    "off",
    "too",
    "much",
    "many",
    "well",
    "way",
    "ways",
    "like",
    "want",
    "need",
    "needs",
    "sure",
    "make",
    "made",
    "get",
    "got",
    "gets",
    "set",
    "put",
    "use",
    "used",
    "using",
    "see",
    "look",
    "know",
    "think",
    "thing",
    "things",
    "something",
    "anything",
    "everything",
    "nothing",
    "someone",
    "anyone",
    "everyone",
    "able",
    "while",
    "until",
    "because",
    "since",
    "though",
    "although",
    "whether",
    "within",
    "across",
    "between",
    "through",
    "during",
    "above",
    "below",
    "upon",
    "around",
    "against",
    "among",
    "they",
    "she",
    "him",
    "her",
    "his",
    "hers",
    "himself",
    "herself",
    "itself",
    "themselves",
    "myself",
    "yourself",
    "ourselves",
    "i'm",
    "don't",
    "doesn't",
    "didn't",
    "isn't",
    "aren't",
    "wasn't",
    "won't",
    "can't",
    "cannot",
    "it",
    "is",
    "be",
    "to",
    "of",
    "in",
    "on",
    "at",
    "by",
    "or",
    "an",
    "as",
    "if",
    "so",
    "do",
    "we",
    "me",
    "my",
    "up",
    "no",
    "us",
    "go",
    // What a task asks for, not its subject
    "add",
    "adds",
    "added",
    "adding",
    "fix",
    "fixes",
    "fixed",
    "fixing",
    "change",
    "changes",
    "update",
    "updates",
    "updated",
    "updating",
    "create",
    "build",
    "implement",
    "write",
    "remove",
    "delete",
    "improve",
    "check",
    "verify",
    "review",
    "run",
    "try",
    "help",
    "work",
    "works",
    "working",
    "start",
    "stop",
    "keep",
    "find",
    "show",
    "tell",
    "give",
    "take",
    "move",
    "rename",
    "new",
    "old",
    "good",
    "bad",
    "better",
    "best",
    "right",
    "wrong",
    "real",
    "first",
    "last",
    "next",
    "done",
    "currently",
    "current",
    "already",
    "always",
    "never",
    "ever",
    "maybe",
    "really",
    "actually",
    "properly",
    "correctly",
    "exactly",
    "instead",
    "rather",
    "agent",
    "agents",
    "task",
    "tasks",
    "goal",
    "issue",
    "issues",
    "problem",
    "problems",
    "today",
    "yesterday",
    "tomorrow",
    "ask",
    "asked",
    "said",
    "says",
    "say",
    "read",
    "following",
];

/// A word's stem: a plural, `-ing` or `-ed` ending off, then a final `e`,
/// for words long enough to keep a root. Applied to task and note words
/// alike, so "releases", "released" and "release" all match.
pub fn stem(word: &str) -> String {
    let n = word.chars().count();
    let mut w = word.to_owned();
    if n > 4 && w.ends_with("ies") {
        w.truncate(w.len() - 3);
        w.push('y');
    } else if n > 3
        && w.ends_with('s')
        && !w.ends_with("ss")
        && !w.ends_with("us")
        && !w.ends_with("is")
    {
        w.pop();
    }
    let n = w.chars().count();
    if n > 5 && w.ends_with("ing") {
        w.truncate(w.len() - 3);
    } else if n > 4 && w.ends_with("ed") {
        w.truncate(w.len() - 2);
    }
    if w.chars().count() >= 6 && w.ends_with('e') {
        w.pop();
    }
    w
}

/// Lower-cased words of `text`: runs of letters and digits.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

/// A note's stems, counted.
fn counts(text: &str, min_chars: usize) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    for w in words(text) {
        if w.chars().count() >= min_chars {
            *out.entry(stem(&w)).or_insert(0) += 1;
        }
    }
    out
}

/// The task's terms: `(stem, word as written)`, in task order, without
/// stop words, pure numbers and words under `min_chars`, at most
/// `max_terms`.
pub fn terms(task: &str, params: &Params) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for w in words(task) {
        if w.chars().count() < params.min_chars
            || STOP.contains(&w.as_str())
            || w.chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        let s = stem(&w);
        if seen.insert(s.clone()) {
            out.push((s, w));
            if out.len() >= params.max_terms {
                break;
            }
        }
    }
    out
}

/// The repository name's words, lower-cased: `acme-store` gives
/// `["acme", "store"]`.
fn repo_words(repo: &str) -> Vec<String> {
    words(repo).map(|w| stem(&w)).collect()
}

/// Whether `phrase` occurs as consecutive words of `text`.
fn has_phrase(text: &str, phrase: &[String]) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let ws: Vec<String> = words(text).map(|w| stem(&w)).collect();
    ws.windows(phrase.len()).any(|win| win == phrase)
}

/// One note, tokenised.
struct Doc<'a> {
    path: &'a Path,
    updated: Option<Date>,
    title: HashMap<String, u32>,
    headings: HashMap<String, u32>,
    path_words: HashMap<String, u32>,
    body: HashMap<String, u32>,
    body_len: usize,
    names_repo: bool,
}

fn body_text(note: &Note, cap: usize) -> String {
    let mut end = note.body.len().min(cap);
    while end < note.body.len() && end > 0 && (note.body[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    String::from_utf8_lossy(&note.body[..end]).into_owned()
}

/// The path's words: its folders and file stem.
fn path_text(path: &Path) -> String {
    path.with_extension("")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ranks the notes for `task`, best first, at most [`LIMIT`]. `repo` is
/// the name of the repository the session works in, when known; `skip`
/// lists notes (relative to the notes directory) never picked, such as
/// the required notes the block already names.
pub fn rank(
    notes: &Notes,
    task: &str,
    repo: Option<&str>,
    skip: &[PathBuf],
    params: &Params,
) -> Vec<Pick> {
    let terms = terms(task, params);
    let repo_phrase = repo.map(repo_words).unwrap_or_default();
    if terms.is_empty() && repo_phrase.is_empty() {
        return Vec::new();
    }
    let docs: Vec<Doc<'_>> = notes
        .notes
        .iter()
        .filter(|(path, _)| !skip.iter().any(|s| s == *path))
        .map(|(path, note)| {
            let title = note.title();
            let body = body_text(note, params.body_bytes);
            let path_words = path_text(path);
            let body_counts = counts(&body, params.min_chars);
            Doc {
                path,
                updated: note.frontmatter.updated,
                names_repo: has_phrase(&title, &repo_phrase)
                    || has_phrase(&path_words, &repo_phrase),
                title: counts(&title, params.min_chars),
                headings: counts(&note.headings().join("\n"), params.min_chars),
                path_words: counts(&path_words, params.min_chars),
                body_len: body_counts.values().map(|n| *n as usize).sum(),
                body: body_counts,
            }
        })
        .collect();
    if docs.is_empty() {
        return Vec::new();
    }
    let n = docs.len() as f64;
    let avg_body = (docs.iter().map(|d| d.body_len).sum::<usize>() as f64 / n).max(1.0);
    let idf = |df: usize| (1.0 + (n - df as f64 + 0.5) / (df as f64 + 0.5)).ln();
    let term_idf: Vec<f64> = terms
        .iter()
        .map(|(stem, _)| {
            let df = docs
                .iter()
                .filter(|d| {
                    d.title.contains_key(stem)
                        || d.headings.contains_key(stem)
                        || d.path_words.contains_key(stem)
                        || d.body.contains_key(stem)
                })
                .count();
            idf(df)
        })
        .collect();
    let repo_idf = idf(docs.iter().filter(|d| d.names_repo).count());

    let mut picks: Vec<(Pick, Option<Date>)> = Vec::new();
    for d in &docs {
        let norm = 1.0 - params.b + params.b * d.body_len as f64 / avg_body;
        let mut score = 0.0;
        let mut matched: Vec<(f64, &str)> = Vec::new();
        for ((stem, word), idf) in terms.iter().zip(&term_idf) {
            let tf = |m: &HashMap<String, u32>| f64::from(m.get(stem).copied().unwrap_or(0));
            let weighted = params.title * tf(&d.title)
                + params.headings * tf(&d.headings)
                + params.path * tf(&d.path_words)
                + params.body * tf(&d.body) / norm;
            if weighted > 0.0 {
                let s = idf * weighted * (params.k1 + 1.0) / (weighted + params.k1);
                score += s;
                matched.push((s, word));
            }
        }
        if d.names_repo {
            // One title-strength match of the name.
            let w = params.repo;
            score += repo_idf * w * (params.k1 + 1.0) / (w + params.k1);
        }
        if score <= 0.0 {
            continue;
        }
        matched.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));
        picks.push((
            Pick {
                path: d.path.to_path_buf(),
                score,
                words: matched.into_iter().map(|(_, w)| w.to_owned()).collect(),
                names_repo: d.names_repo,
            },
            d.updated,
        ));
    }
    picks.sort_by(|(a, ad), (b, bd)| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| super::newest_first(*ad, *bd))
            .then_with(|| a.path.cmp(&b.path))
    });
    let best = picks.first().map_or(0.0, |(p, _)| p.score);
    let floor = params.min_score * idf(1);
    picks
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| p.score >= floor && p.score >= params.min_share * best)
        .take(LIMIT)
        .collect()
}

/// The earlier ranking, kept as the eval's baseline: the
/// number of task words of three or more characters found as substrings of
/// the title or a heading; ties by most recent `updated`, then path.
pub fn rank_v1(notes: &Notes, task: &str) -> Vec<PathBuf> {
    let mut tokens: Vec<String> = Vec::new();
    for token in task.split(|c: char| !c.is_alphanumeric()) {
        let token = token.to_lowercase();
        if token.chars().count() >= 3 && !tokens.contains(&token) {
            tokens.push(token);
        }
    }
    let mut scored: Vec<(usize, Option<Date>, &PathBuf)> = Vec::new();
    for (path, note) in &notes.notes {
        let title = note.title().to_lowercase();
        let headings: Vec<String> = note.headings().iter().map(|h| h.to_lowercase()).collect();
        let score = tokens
            .iter()
            .filter(|t| {
                title.contains(t.as_str()) || headings.iter().any(|h| h.contains(t.as_str()))
            })
            .count();
        if score > 0 {
            scored.push((score, note.frontmatter.updated, path));
        }
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| super::newest_first(a.1, b.1))
            .then_with(|| a.2.cmp(b.2))
    });
    scored
        .into_iter()
        .take(LIMIT)
        .map(|(_, _, p)| p.clone())
        .collect()
}

/// The name of the git repository `dir` lies in: the directory holding the
/// repository's common `.git`, so a linked worktree gives its main
/// repository's name. `None` outside a repository, or when that repository
/// is `workspace` itself (its name says nothing about the task).
pub fn repo_name(dir: &Path, workspace: &Path) -> Option<String> {
    let mut at = Some(dir);
    while let Some(d) = at {
        if d.join(".git").exists() {
            let common = crate::dream::observed::common_dir(d)?;
            let root = common.parent()?;
            let ws = workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf());
            if root == ws {
                return None;
            }
            return root.file_name().map(|n| n.to_string_lossy().into_owned());
        }
        at = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn notes(files: &[(&str, &str)]) -> (tempfile::TempDir, Notes) {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let p = dir.path().join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        let notes = Notes::scan(dir.path()).unwrap();
        (dir, notes)
    }

    fn paths(picks: &[Pick]) -> Vec<String> {
        picks.iter().map(|p| p.path.display().to_string()).collect()
    }

    #[test]
    fn stems_join_plurals_and_endings() {
        assert_eq!(stem("releases"), stem("release"));
        assert_eq!(stem("released"), stem("release"));
        assert_eq!(stem("releasing"), stem("release"));
        assert_eq!(stem("shortcuts"), "shortcut");
        assert_eq!(stem("policies"), "policy");
        assert_eq!(stem("status"), "status");
        assert_eq!(stem("notes"), "note");
        assert_eq!(stem("add"), "add");
    }

    #[test]
    fn task_words_drop_stop_words_numbers_and_short_words() {
        let t: Vec<String> = terms(
            "Add the 5 webhook retries to it, then fix retry",
            &Params::default(),
        )
        .into_iter()
        .map(|(_, w)| w)
        .collect();
        // "retry" has the same stem as "retries", so it is not repeated.
        assert_eq!(t, ["webhook", "retries"]);
    }

    #[test]
    fn words_match_whole_not_as_substrings() {
        let (_d, n) = notes(&[
            (
                "Address book.md",
                "# Address book\n\nPadding and addresses.\n",
            ),
            (
                "Fleet keys.md",
                "# Fleet keys\n\nKeyboard shortcuts for the fleet.\n",
            ),
        ]);
        // "add" is a stop word and would only match inside other words.
        let picks = rank(&n, "add a fleet shortcut", None, &[], &Params::default());
        assert_eq!(paths(&picks), ["Fleet keys.md"]);
        assert_eq!(picks[0].words, ["fleet", "shortcut"]);
    }

    #[test]
    fn a_rare_word_outweighs_a_common_one() {
        let mut files: Vec<(String, String)> = (0..12)
            .map(|i| {
                (
                    format!("Store {i}.md"),
                    format!("# Store {i}\n\nThe store front.\n"),
                )
            })
            .collect();
        files.push((
            "Tally.md".into(),
            "# Tally\n\nLedger ageing for the store.\n".into(),
        ));
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let (_d, n) = notes(&refs);
        let picks = rank(&n, "store tally ageing", None, &[], &Params::default());
        assert_eq!(picks[0].path, Path::new("Tally.md"));
        assert_eq!(picks[0].words[0], "tally");
    }

    #[test]
    fn the_body_counts_but_the_title_counts_more() {
        let (_d, n) = notes(&[
            ("A.md", "# Webhooks\n\nRetries back off.\n"),
            (
                "B.md",
                "# Payments\n\nWebhooks arrive here and are retried.\n",
            ),
            ("C.md", "# Other\n\nNothing related.\n"),
        ]);
        let picks = rank(
            &n,
            "webhooks",
            None,
            &[],
            &Params {
                min_score: 0.0,
                min_share: 0.0,
                ..Params::default()
            },
        );
        assert_eq!(paths(&picks), ["A.md", "B.md"]);
    }

    #[test]
    fn the_repo_name_lifts_notes_that_name_it() {
        let (_d, n) = notes(&[
            (
                "Hippocamp engine.md",
                "# Hippocamp engine\n\nThe memory engine.\n",
            ),
            (
                "Album keys.md",
                "# Album keyboard\n\nShortcuts in the album view.\n",
            ),
            ("Other.md", "# Other\n\nUnrelated.\n"),
        ]);
        let params = Params {
            min_share: 0.0,
            min_score: 0.0,
            ..Params::default()
        };
        let picks = rank(&n, "keyboard shortcut", Some("hippocamp"), &[], &params);
        assert!(picks
            .iter()
            .any(|p| p.path == Path::new("Hippocamp engine.md") && p.names_repo));
        let hippocamp = picks.iter().find(|p| p.names_repo).unwrap();
        assert_eq!(
            hippocamp.reason(Some("hippocamp")),
            "names the repo hippocamp"
        );
        let album = picks.iter().find(|p| !p.names_repo).unwrap();
        assert_eq!(
            album.reason(Some("hippocamp")),
            "matches keyboard, shortcut"
        );
        // Without the repo, the hippocamp note matches nothing.
        let picks = rank(&n, "keyboard shortcut", None, &[], &params);
        assert_eq!(paths(&picks), ["Album keys.md"]);
    }

    #[test]
    fn weak_matches_do_not_fill_the_list() {
        let (_d, n) = notes(&[
            (
                "Webhooks.md",
                "# Webhooks\n\n## Retry policy\n\nWebhook retries back off.\n",
            ),
            (
                "Misc.md",
                "# Misc\n\nA long note that mentions a policy once among many other words about \
                         unrelated things, so its one match is weak and should not be listed.\n",
            ),
        ]);
        let picks = rank(&n, "webhook retry policy", None, &[], &Params::default());
        assert_eq!(paths(&picks), ["Webhooks.md"]);
    }

    #[test]
    fn skipped_notes_are_never_picked_and_ties_go_to_the_newest() {
        let (_d, n) = notes(&[
            ("a.md", "---\nupdated: 2026-01-01\n---\n# Webhook a\n"),
            ("b.md", "---\nupdated: 2026-05-01\n---\n# Webhook b\n"),
            ("c.md", "# Webhook c\n"),
            ("d.md", "---\nupdated: 2026-09-01\n---\n# Webhook d\n"),
            // A word every note holds says nothing; these make it rare.
            ("x.md", "# Other x\n"),
            ("y.md", "# Other y\n"),
            ("z.md", "# Other z\n"),
        ]);
        let picks = rank(
            &n,
            "webhook",
            None,
            &[PathBuf::from("d.md")],
            &Params::default(),
        );
        assert_eq!(paths(&picks), ["b.md", "a.md", "c.md"]);
    }

    #[test]
    fn the_baseline_counts_substrings() {
        let (_d, n) = notes(&[("Address.md", "# Address\n"), ("Fleet.md", "# Fleet\n")]);
        assert_eq!(
            rank_v1(&n, "add fleet"),
            [PathBuf::from("Address.md"), PathBuf::from("Fleet.md")]
        );
    }

    #[test]
    fn repo_names_come_from_the_common_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("tern");
        fs::create_dir_all(repo.join(".git/worktrees/wt")).unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        assert_eq!(repo_name(&repo.join("src"), &root).as_deref(), Some("tern"));
        // A linked worktree names its main repository.
        let wt = root.join("tern-wt/feature");
        fs::create_dir_all(&wt).unwrap();
        fs::write(repo.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        assert_eq!(repo_name(&wt, &root).as_deref(), Some("tern"));
        // The workspace itself, or no repository: none.
        assert_eq!(repo_name(&repo, &repo), None);
        assert_eq!(repo_name(&root, &root), None);
    }

    #[test]
    fn a_pick_is_weak_without_words_or_under_half_the_top_score() {
        let pick = |score: f64, words: &[&str], names_repo: bool| Pick {
            path: PathBuf::from("a.md"),
            score,
            words: words.iter().map(|w| (*w).to_owned()).collect(),
            names_repo,
        };
        assert!(!pick(10.0, &["fleet"], false).is_weak(10.0));
        assert!(!pick(5.0, &["fleet"], false).is_weak(10.0));
        assert!(pick(4.9, &["fleet"], false).is_weak(10.0));
        assert!(pick(10.0, &[], false).is_weak(10.0));
        assert!(!pick(1.0, &[], true).is_weak(10.0));
    }
}

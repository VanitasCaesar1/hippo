//! Claims versus evidence: what an agent said it did
//! ("tests pass", "PR #214 merged", "deployed") and whether the transcript
//! holds a command with a successful result behind it, after the last edit
//! that could change the answer.
//!
//! Precision over recall: a false "unverified" on an honest agent is worse
//! than a missed claim, so sentences that hedge, plan, negate or report
//! someone else's words are not claims, and evidence the transcript cannot
//! settle is "unclear", not "unverified".

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::shell::{self, exit_code, Cwd, Simple};
use crate::index::StoredTurn;
use crate::rollup::work::{self, PullDone, Repos};
use crate::rollup::{edited_paths, is_agent_tool, is_call, is_edit_tool};

/// The longest claim kept, in characters.
pub const CLAIM_CHARS: usize = 160;

/// The longest command shown as evidence, in characters.
const COMMAND_CHARS: usize = 120;

/// What a claim is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaimKind {
    Tests,
    Typecheck,
    Lint,
    Fmt,
    Build,
    PrMerged,
    PrOpened,
    Pushed,
    Deployed,
    BrainCheck,
    Ci,
}

impl ClaimKind {
    pub const ALL: [ClaimKind; 11] = [
        ClaimKind::Tests,
        ClaimKind::Typecheck,
        ClaimKind::Lint,
        ClaimKind::Fmt,
        ClaimKind::Build,
        ClaimKind::PrMerged,
        ClaimKind::PrOpened,
        ClaimKind::Pushed,
        ClaimKind::Deployed,
        ClaimKind::BrainCheck,
        ClaimKind::Ci,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ClaimKind::Tests => "tests",
            ClaimKind::Typecheck => "typecheck",
            ClaimKind::Lint => "lint",
            ClaimKind::Fmt => "fmt",
            ClaimKind::Build => "build",
            ClaimKind::PrMerged => "pr-merged",
            ClaimKind::PrOpened => "pr-opened",
            ClaimKind::Pushed => "pushed",
            ClaimKind::Deployed => "deployed",
            ClaimKind::BrainCheck => "brain-check",
            ClaimKind::Ci => "ci",
        }
    }

    pub fn parse(s: &str) -> Option<ClaimKind> {
        ClaimKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The word a person reads: `tests`, `merged`, `deployed`.
    pub fn label(self) -> &'static str {
        match self {
            ClaimKind::Tests => "tests pass",
            ClaimKind::Typecheck => "typecheck",
            ClaimKind::Lint => "lint",
            ClaimKind::Fmt => "formatting",
            ClaimKind::Build => "build",
            ClaimKind::PrMerged => "merged",
            ClaimKind::PrOpened => "PR opened",
            ClaimKind::Pushed => "pushed",
            ClaimKind::Deployed => "deployed",
            ClaimKind::BrainCheck => "notes check",
            ClaimKind::Ci => "CI",
        }
    }

    /// Whether a later code edit makes earlier evidence stale.
    fn edits_matter(self) -> bool {
        matches!(
            self,
            ClaimKind::Tests
                | ClaimKind::Typecheck
                | ClaimKind::Lint
                | ClaimKind::Fmt
                | ClaimKind::Build
                | ClaimKind::Ci
                | ClaimKind::BrainCheck
        )
    }

    /// Whether a command of this kind shows a claim of `claim`: a build
    /// claim ("compiles cleanly") is also shown by a typecheck, a lint or
    /// a test run, which compile the code.
    fn can_show(self, claim: ClaimKind) -> bool {
        self == claim
            || (claim == ClaimKind::Build
                && matches!(
                    self,
                    ClaimKind::Typecheck | ClaimKind::Lint | ClaimKind::Tests
                ))
    }

    /// What the evidence would be, for the unverified reason.
    fn command_word(self) -> &'static str {
        match self {
            ClaimKind::Tests => "test",
            ClaimKind::Typecheck => "typecheck",
            ClaimKind::Lint => "lint",
            ClaimKind::Fmt => "format check",
            ClaimKind::Build => "build",
            ClaimKind::PrMerged => "merge",
            ClaimKind::PrOpened => "pull request",
            ClaimKind::Pushed => "git push",
            ClaimKind::Deployed => "deploy",
            ClaimKind::BrainCheck => "notes check",
            ClaimKind::Ci => "CI check",
        }
    }
}

/// A claim's standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Verified,
    Unverified,
    /// Evidence exists that the transcript cannot settle.
    Unclear,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Verified => "verified",
            Status::Unverified => "unverified",
            Status::Unclear => "unclear",
        }
    }

    pub fn parse(s: &str) -> Status {
        match s {
            "verified" => Status::Verified,
            "unclear" => Status::Unclear,
            _ => Status::Unverified,
        }
    }
}

/// One claim and what the transcript says about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub kind: ClaimKind,
    pub status: Status,
    /// The sentence, one line, at most [`CLAIM_CHARS`].
    pub claim: String,
    /// The assistant turn that said it.
    pub ordinal: u32,
    /// For a verified claim, its evidence (`cargo test --locked · exit 0 ·
    /// 47 passed · turn 302`); else why it is not verified.
    pub evidence: String,
    /// A pull request number the claim names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
}

/// What [`check`] reads.
pub struct Inputs<'a> {
    /// The top-level session's turns.
    pub turns: &'a [StoredTurn],
    /// Its sub-agents' turns.
    pub subs: &'a [(String, Vec<StoredTurn>)],
    /// The session's directory.
    pub dir: Option<&'a Path>,
    /// The notes folder's absolute paths.
    pub notes_roots: &'a [PathBuf],
}

// ---------------------------------------------------------------------------
// Finding claims

static PATTERNS: LazyLock<Vec<(ClaimKind, Regex)>> = LazyLock::new(|| {
    let p = |k: ClaimKind, re: &str| (k, Regex::new(&format!("(?i){re}")).expect("valid regex"));
    vec![
        p(
            ClaimKind::Tests,
            r"\b(?:tests?|test suites?|specs|cargo test|npm test|pytest|vitest|jest|go test)\b[^.;:!?\n]{0,30}?\b(?:pass|passes|passed|passing|are green|is green|green|succeed|succeeds|succeeded)\b",
        ),
        p(ClaimKind::Tests, r"\b\d+\s+(?:tests?\s+)?passed\b"),
        p(
            ClaimKind::Typecheck,
            r"\b(?:typecheck(?:s|ing)?|type-check(?:s|ing)?|type check(?:s|ing)?|tsc|mypy|pyright|cargo check)\b[^.;:!?\n]{0,20}?\b(?:clean|pass|passes|passed|green|succeeds|succeeded)\b",
        ),
        p(ClaimKind::Typecheck, r"\bno type errors\b"),
        p(
            ClaimKind::Lint,
            r"\b(?:clippy|lint|lints|linting|linter|eslint|ruff|swiftlint|golangci-lint)\b[^.;:!?\n]{0,20}?\b(?:clean|pass|passes|passed|green)\b",
        ),
        p(
            ClaimKind::Lint,
            r"\bno (?:clippy|lint|eslint) (?:warnings|errors|issues)\b",
        ),
        p(
            ClaimKind::Fmt,
            r"\b(?:fmt|cargo fmt|formatting|format check|prettier|gofmt)\b[^.;:!?\n]{0,20}?\b(?:clean|pass|passes|passed)\b",
        ),
        p(
            ClaimKind::Build,
            r"\bbuild (?:now |still )?(?:succeeds|succeeded|passes|passed|is green|is clean|was clean|is ok|went through)\b",
        ),
        p(
            ClaimKind::Build,
            r"\bbuilds? (?:are |is )?(?:clean|cleanly|fine|successfully|green)\b",
        ),
        p(ClaimKind::Build, r"\bcompiles (?:clean|cleanly|fine)\b"),
        p(
            ClaimKind::PrMerged,
            r"\b(?:PR|pull request)\s*#?(\d+)?\b[^.;:!?\n]{0,25}?\b(?:is |was |has been |got )?(?:squash[- ])?merged\b",
        ),
        p(
            ClaimKind::PrMerged,
            r"(?:^|\b(?:i|we|and|then|just|also|have|has|now|both)\s+)(?:squash[- ])?merged\s+(?:PR\s*|pull request\s*)?#(\d+)",
        ),
        p(
            ClaimKind::PrOpened,
            r"\b(?:opened|created|raised|filed)\s+(?:a\s+|the\s+)?(?:draft\s+)?(?:PR|pull request)\b\s*#?(\d+)?",
        ),
        p(
            ClaimKind::PrOpened,
            r"\b(?:PR|pull request)\s*#(\d+)\s+(?:is\s+)?(?:opened|open|up)\b",
        ),
        p(ClaimKind::Pushed, r"\bpushed\b"),
        p(
            ClaimKind::Deployed,
            r"(?:^|\b(?:i|we|it|it's|its|is|are|was|were|been|now|successfully|just|both|all|and|then)\s+)(?:re)?deployed\b|\b(?:is|are) now live\b|\blive (?:on|in) (?:prod|production)\b|\breleased (?:to|as) ",
        ),
        p(
            ClaimKind::BrainCheck,
            r"\b(?:(?:check|lint|validate)[_-](?:brain|notes|vault|wiki)(?:\.py)?|(?:brain|notes?|vault|wiki) (?:check|lint)(?:s)?|hippocamp lint)\b[^.;:!?\n]{0,20}?\b(?:pass|passes|passed|clean|green|ok)\b",
        ),
        p(
            ClaimKind::Ci,
            r"\bCI\b[^.;:!?\n]{0,15}?\b(?:green|pass|passes|passed)\b",
        ),
    ]
});

/// Words that make a sentence not a claim: negation, failure, condition,
/// plan, hedge, report of someone else's words.
static NOT_A_CLAIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:not|never|cannot|can't|won't|isn't|aren't|wasn't|weren't|didn't|doesn't|don't|haven't|hasn't|fail|fails|failing|failed|failure|broken|except|unless|without|will|would|should|could|might|may|once|if|when|whenever|until|need|needs|expect|expected|assume|assuming|to be|next|plan|planned|let me|i'll|going to|try|trying|report|reports|reported|says|said|claim|claims|according|whether|hopefully|probably|pending|todo|nothing|someone|you|wrong|anything|most|some|one more|another|another agent|other agents?|(?:the|its|their|each|both|this|that) (?:[\w-]+ )?(?:agent|sub-agent|subagent|child|delegate|worker|reviewer)s?)\b|n't\b|\?")
        .expect("valid regex")
});

static CI_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bCI\b").expect("valid regex"));

/// A sentence that tells someone what to do: not a claim.
static IMPERATIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\W*(?:check|make sure|ensure|verify|confirm|run|see|try|test|please)\b")
        .expect("valid regex")
});

/// `0 failed`, `no failures`: part of a pass, not a failure.
static ZERO_FAILED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b0 (?:failed|failures?|errors?|warnings?)\b|\bno (?:failures|errors|warnings)\b",
    )
    .expect("valid regex")
});

/// A sentence that is not a claim by its shape: progress
/// narrated as it happens, a state still in progress, someone else's
/// repository, and `pass` as a noun.
static NOT_A_CLAIM_SHAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\W*(?:running|starting|launching|kicking off|re-?running|checking|verifying|waiting)\b|\bin progress\b|\bstill running\b|\b\w+'s (?:fork|repo|repository|project|readme|library|crate|package)\b|\bupstream\b|\bthird[- ]party\b|\b(?:a|an|one)\s+(?:[\w-]+,?\s+(?:and\s+)?){0,4}pass(?:es)?\b|\b(?:first|second|third|final|baseline)\s+pass(?:es)?\b")
        .expect("valid regex")
});

/// A run meant to fail first: `fail-first`, `red first`, `fails as
/// expected`, `fail on behaviour`.
static RED_RUN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bfail(?:s|ed|ing)?[- ]first\b|\bred[- ](?:first|phase|run)\b|\b(?:fail|fails|failed) as expected\b|\bexpected to fail\b|\bfail(?:s|ed)? on (?:the )?behaviou?r\b")
        .expect("valid regex")
});

/// A local CI or a deployed thing named as an adjective: not a claim.
static ADJECTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:the|a|an|our|its|their|of|already)\s+(?:deployed|pushed|merged)\b")
        .expect("valid regex")
});

/// The sentences of an assistant turn that can hold a claim: fenced code
/// blocks and `>` quotes out, split at sentence ends, line ends and list
/// bullets.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || t.starts_with('>') || t.starts_with('|') {
            continue;
        }
        let t = t
            .trim_start_matches(['-', '*', '+', '•'])
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
            .trim();
        let mut start = 0;
        let bytes: Vec<(usize, char)> = t.char_indices().collect();
        for (k, (i, c)) in bytes.iter().enumerate() {
            let end = matches!(c, '.' | '!' | '?')
                && bytes.get(k + 1).is_none_or(|(_, n)| n.is_whitespace());
            if end {
                let s = t[start..i + c.len_utf8()].trim();
                if !s.is_empty() {
                    out.push(s.to_owned());
                }
                start = i + c.len_utf8();
            }
        }
        let rest = t[start..].trim();
        if !rest.is_empty() {
            out.push(rest.to_owned());
        }
    }
    out
}

/// The claims an assistant turn makes: `(kind, pr number, sentence)`.
pub fn claims_in(text: &str) -> Vec<(ClaimKind, Option<u64>, String)> {
    let mut out: Vec<(ClaimKind, Option<u64>, String)> = Vec::new();
    // A message that reports a run meant to fail first (a test written
    // before its code) says which tests pass in that run, not that the
    // suite passes.
    let red = RED_RUN.is_match(text);
    for s in sentences(text) {
        let plain = ZERO_FAILED.replace_all(&s, "");
        if NOT_A_CLAIM.is_match(&plain)
            || ADJECTIVE.is_match(&plain)
            || IMPERATIVE.is_match(&plain)
            || NOT_A_CLAIM_SHAPE.is_match(&plain)
            || quoted(&s)
        {
            continue;
        }
        for (kind, re) in PATTERNS.iter() {
            if red && *kind == ClaimKind::Tests {
                continue;
            }
            if let Some(c) = re.captures(&s) {
                // A word in quotes is named, not claimed.
                let m = c.get(0).expect("whole match");
                let before = s[..m.start()].trim_end().chars().next_back();
                if matches!(before, Some('"' | '\u{201c}' | '`' | '\'')) {
                    continue;
                }
                // CI run locally is the local checks, claimed by their own kinds;
                // a sentence about CI claims nothing about local checks.
                if *kind == ClaimKind::Ci && s.to_lowercase().contains("local") {
                    continue;
                }
                if *kind != ClaimKind::Ci
                    && CI_WORD.is_match(&s)
                    && !s.to_lowercase().contains("local")
                {
                    continue;
                }
                let pr = if matches!(kind, ClaimKind::PrMerged | ClaimKind::PrOpened) {
                    // `PR 1 merged (#95)`: the `#` number names the pull request.
                    PR_NUMBER
                        .captures(&s)
                        .and_then(|c| c[1].parse().ok())
                        .or_else(|| c.get(1).and_then(|m| m.as_str().parse().ok()))
                } else {
                    None
                };
                if !out.iter().any(|(k, n, _)| k == kind && *n == pr) {
                    out.push((*kind, pr, one_line(&s, CLAIM_CHARS)));
                }
            }
        }
    }
    out
}

/// A sentence wholly inside quotes: a quoted instruction, not a claim.
fn quoted(sentence: &str) -> bool {
    let t = sentence.trim();
    let first = t.chars().next();
    let last = t.trim_end_matches(['.', '!', '?']).chars().next_back();
    matches!(
        (first, last),
        (Some('"'), Some('"')) | (Some('\u{201c}'), Some('\u{201d}')) | (Some('\''), Some('\''))
    ) && t.chars().count() > 2
}

static PR_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#(\d+)\b").expect("valid regex"));

// ---------------------------------------------------------------------------
// Evidence

/// A command that ran, what it can show, and how it ended.
#[derive(Debug, Clone)]
struct Exec {
    ts: String,
    ordinal: u32,
    /// The sub-agent it ran in, if not the session itself.
    sub: Option<String>,
    command: String,
    /// The kinds it can show, each with the simple command that shows it.
    runs: Vec<Run>,
    count: Option<String>,
    /// For a CI read, its output.
    output: Option<String>,
}

/// One kind a call can show: the simple command that shows it, the
/// directory that ran in, its place among the call's simple commands, and
/// how it ended (the check's own result).
#[derive(Debug, Clone)]
struct Run {
    kind: ClaimKind,
    shown: String,
    dir: Option<PathBuf>,
    pos: u32,
    result: Outcome,
}

impl Exec {
    /// The run that shows `kind`, when one can.
    fn shows(&self, kind: ClaimKind) -> Option<&Run> {
        self.runs.iter().find(|r| r.kind.can_show(kind))
    }

    /// How the run that shows `kind` ended.
    fn result(&self, kind: ClaimKind) -> Outcome {
        self.shows(kind)
            .map_or(Outcome::Unknown, |r| r.result.clone())
    }

    /// Where in the transcript the run that shows `kind` happened.
    fn at(&self, kind: ClaimKind) -> Place<'_> {
        Place {
            ts: &self.ts,
            sub: self.sub.as_deref(),
            ordinal: self.ordinal,
            pos: self.shows(kind).map_or(0, |r| r.pos),
        }
    }
}

/// A point in the session: a turn and a simple command's place in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place<'a> {
    ts: &'a str,
    sub: Option<&'a str>,
    ordinal: u32,
    pos: u32,
}

impl Place<'_> {
    /// Whether `self` came after `other`: by time, and within one turn of
    /// one transcript by the order of its simple commands.
    fn after(&self, other: &Place<'_>) -> bool {
        if self.ts != other.ts {
            return self.ts > other.ts;
        }
        if self.sub != other.sub {
            return false;
        }
        (self.ordinal, self.pos) > (other.ordinal, other.pos)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Succeeded,
    Failed(Option<i64>),
    Unknown,
}

/// An edit and the file it changed.
#[derive(Debug, Clone)]
struct Edit {
    ts: String,
    ordinal: u32,
    sub: Option<String>,
    path: PathBuf,
    /// Its place among its call's simple commands (0 for an edit tool).
    pos: u32,
    /// It changed only comment lines.
    comment_only: bool,
}

impl Edit {
    fn at(&self) -> Place<'_> {
        Place {
            ts: &self.ts,
            sub: self.sub.as_deref(),
            ordinal: self.ordinal,
            pos: self.pos,
        }
    }
}

/// The kinds a simple command can show.
fn kinds_of(c: &Simple) -> Vec<ClaimKind> {
    use ClaimKind::*;
    let mut out = Vec::new();
    let Some((prog, args)) = c.program() else {
        return out;
    };
    if args.iter().any(|a| {
        matches!(
            *a,
            "--version" | "-V" | "--help" | "-h" | "--dry-run" | "--dryrun"
        )
    }) {
        return out;
    }
    // `cargo test --no-run` compiles and runs nothing.
    if prog == "cargo" && args.contains(&"--no-run") {
        out.push(Build);
        return out;
    }
    // A notes check: `hippocamp lint`, or a script whose
    // name holds a word of notes and a word of checking.
    let notes_check = |name: &str| {
        let n = name.to_lowercase();
        let n = n.rsplit('/').next().unwrap_or(&n);
        ["brain", "notes", "vault", "wiki", "kb"]
            .iter()
            .any(|w| n.contains(w))
            && ["check", "lint", "validate"].iter().any(|w| n.contains(w))
    };
    let brain = (prog == "hippocamp" && args.first() == Some(&"lint"))
        || notes_check(&prog)
        || (is_interpreter(&prog)
            && args
                .iter()
                .find(|a| !a.starts_with('-'))
                .is_some_and(|a| notes_check(a)));
    if brain {
        out.push(BrainCheck);
        return out;
    }
    let plain: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !a.starts_with('-'))
        .collect();
    let first = plain.first().copied().unwrap_or("");
    let second = plain.get(1).copied().unwrap_or("");
    let script = |name: &str| -> Option<ClaimKind> {
        let n = name.to_lowercase();
        if n.starts_with("test") || n == "t" || n.starts_with("e2e") {
            Some(Tests)
        } else if n.starts_with("typecheck")
            || n.starts_with("type-check")
            || n == "tsc"
            || n == "check-types"
            || n == "types"
        {
            Some(Typecheck)
        } else if n.starts_with("lint") {
            Some(Lint)
        } else if n.starts_with("format") || n == "fmt" || n == "prettier" {
            Some(Fmt)
        } else if n.starts_with("build") {
            Some(Build)
        } else if n.starts_with("deploy") || n.starts_with("release") || n.starts_with("publish") {
            Some(Deployed)
        } else {
            None
        }
    };
    // A project's own check script, by its file name:
    // `node scripts/build.mjs`, `./build.sh`, `bash test.sh`.
    let by_name = |file: &str| -> Option<ClaimKind> {
        let stem = Path::new(file).file_stem()?.to_str()?;
        script(stem).filter(|k| matches!(k, Tests | Typecheck | Lint | Fmt | Build))
    };
    if is_interpreter(&prog) {
        if let Some(file) = args.iter().find(|a| !a.starts_with('-')) {
            if (file.contains('.') || file.contains('/')) && !file.contains(char::is_whitespace) {
                if let Some(k) = by_name(file) {
                    out.push(k);
                    return out;
                }
            }
        }
    }
    if c.words.first().is_some_and(|w| w.contains('/')) {
        if let Some(k) = by_name(&c.words[0]) {
            out.push(k);
            return out;
        }
    }
    match prog.as_str() {
        "cargo" => match first {
            "test" | "nextest" => out.push(Tests),
            "check" => out.push(Typecheck),
            "clippy" => out.push(Lint),
            "fmt" => out.push(Fmt),
            "build" => out.push(Build),
            "publish" => out.push(Deployed),
            _ => {}
        },
        "npm" | "pnpm" | "yarn" | "bun" => match first {
            "run" | "run-script" => out.extend(script(second)),
            "test" | "t" => out.push(Tests),
            "publish" => out.push(Deployed),
            "exec" | "dlx" | "x" => {}
            other => out.extend(script(other)),
        },
        "pytest" | "jest" | "vitest" | "rspec" => out.push(Tests),
        "node" | "tsx" | "deno" if args.contains(&"--test") => out.push(Tests),
        "go" => match first {
            "test" => out.push(Tests),
            "build" => out.push(Build),
            "vet" => out.push(Lint),
            _ => {}
        },
        "swift" => match first {
            "test" => out.push(Tests),
            "build" => out.push(Build),
            _ => {}
        },
        "deno" | "mix" if first == "test" => out.push(Tests),
        "playwright" if first == "test" => out.push(Tests),
        "xcodebuild" => {
            if args.contains(&"test") {
                out.push(Tests);
            } else {
                out.push(Build);
            }
        }
        "make" => match first {
            "test" | "check" => out.push(Tests),
            "lint" => out.push(Lint),
            "" | "build" | "all" | "release" => out.push(Build),
            _ => {}
        },
        "gradle" | "gradlew" => {
            if plain.iter().any(|a| a.contains("test")) {
                out.push(Tests);
            } else if plain
                .iter()
                .any(|a| a.contains("assemble") || a.contains("build"))
            {
                out.push(Build);
            }
        }
        "tsc" | "vue-tsc" | "mypy" | "pyright" => out.push(Typecheck),
        "swiftc" if args.contains(&"-typecheck") => out.push(Typecheck),
        "swiftc" if !args.contains(&"-parse") => out.push(Build),
        "eslint" | "golangci-lint" | "swiftlint" | "flake8" | "pylint" => out.push(Lint),
        "ruff" | "biome" => {
            if first == "format" {
                out.push(Fmt);
            } else {
                out.push(Lint);
            }
        }
        "prettier" | "gofmt" | "black" | "rustfmt" => out.push(Fmt),
        "vite" | "next" | "tauri" if first == "build" => out.push(Build),
        "expo" if first == "export" => out.push(Build),
        "eas" => match first {
            "build" => out.push(Build),
            "update" | "submit" => out.push(Deployed),
            _ => {}
        },
        "docker" => match first {
            "build" => out.push(Build),
            "push" => out.push(Deployed),
            _ => {}
        },
        "gh" => match (first, second) {
            ("pr", "merge") => out.push(PrMerged),
            ("pr", "create") => out.push(PrOpened),
            ("pr", "checks") | ("run", "view") | ("run", "watch") => out.push(Ci),
            ("pr", "view") if args.iter().any(|a| a.contains("statusCheckRollup")) => out.push(Ci),
            ("release", "create") => out.push(Deployed),
            _ => {}
        },
        "git" if first == "push" => out.push(Pushed),
        "vercel" | "dokploy" | "railway" | "heroku" => out.push(Deployed),
        "netlify" | "firebase" | "fly" | "flyctl" if first == "deploy" => out.push(Deployed),
        "wrangler" if matches!(first, "deploy" | "publish" | "pages") => out.push(Deployed),
        "kubectl" if matches!(first, "apply" | "rollout") => out.push(Deployed),
        "helm" if matches!(first, "upgrade" | "install") => out.push(Deployed),
        "curl" if args.iter().any(|a| a.to_lowercase().contains("deploy")) => out.push(Deployed),
        "xcrun" | "nix" | "mise" | "asdf" | "rtx" | "direnv" | "doppler" | "op" => {}
        _ => {}
    }
    // A command run elsewhere: after `--` (`runner exec <id> -- go test`),
    // or an `ssh` host's script.
    if out.is_empty() {
        if let Some(i) = c.words.iter().position(|w| w == "--") {
            let rest = Simple {
                words: c.words[i + 1..].to_vec(),
                written: Vec::new(),
            };
            if !rest.words.is_empty() {
                return kinds_of(&rest);
            }
        }
        if prog == "ssh" {
            if let Some(script) = c.words.last() {
                for inner in shell::simple_commands(script) {
                    out.extend(kinds_of(&inner));
                }
                return out;
            }
        }
    }
    // A wrapper hippocamp does not know (`heavy cargo build`, `xcrun swiftc`):
    // the command after it.
    if out.is_empty() && c.words.len() > 1 {
        let rest = Simple {
            words: c
                .words
                .iter()
                .skip_while(|w| !w.contains(&*prog) || w.contains('='))
                .skip(1)
                .cloned()
                .collect(),
            written: Vec::new(),
        };
        if !rest.words.is_empty() && rest.words.len() < c.words.len() {
            let known = rest.program().is_some_and(|(p, _)| {
                matches!(
                    p.as_str(),
                    "cargo"
                        | "npm"
                        | "pnpm"
                        | "yarn"
                        | "bun"
                        | "go"
                        | "swift"
                        | "swiftc"
                        | "xcodebuild"
                        | "pytest"
                        | "vitest"
                        | "jest"
                        | "tsc"
                        | "make"
                        | "gradle"
                        | "gradlew"
                )
            });
            if known {
                return kinds_of(&rest);
            }
        }
    }
    out
}

/// Whether `prog` runs a script file named by its first argument.
fn is_interpreter(prog: &str) -> bool {
    prog.starts_with("python")
        || matches!(
            prog,
            "node" | "bun" | "deno" | "tsx" | "ts-node" | "ruby" | "bash" | "sh" | "zsh" | "perl"
        )
}

/// Whether `prog` runs a program file that is not a shell script.
fn is_program_interpreter(prog: &str) -> bool {
    is_interpreter(prog) && !matches!(prog, "bash" | "sh" | "zsh")
}

/// The path a word names, from the directory `dir` it is read in.
fn resolve(word: &str, dir: Option<&Path>) -> Option<PathBuf> {
    if word.is_empty() || word.contains(['$', '`', '*', '?']) || word.starts_with('-') {
        return None;
    }
    let p = Path::new(word);
    if p.is_absolute() {
        Some(shell::lexical(p))
    } else if word.starts_with('~') {
        None
    } else {
        dir.map(|d| shell::lexical(&d.join(p)))
    }
}

/// The program and the file a simple command runs, when it runs one: the
/// command itself (`./run.sh`, `lockf … /tmp/run.sh`), or an interpreter's
/// first argument (`bash run.sh`, `python3 fix.py`).
fn runs_file(c: &Simple) -> Option<(String, String)> {
    let (prog, args) = c.program()?;
    if is_interpreter(&prog) {
        let file = args.iter().find(|a| !a.starts_with('-'))?;
        return Some((prog, (*file).to_owned()));
    }
    // The word the program was named by, past any wrapper.
    let word = c.words.iter().find(|w| {
        Path::new(w.as_str())
            .file_name()
            .is_some_and(|n| n == prog.as_str())
    })?;
    word.contains('/').then(|| (prog.clone(), word.clone()))
}

/// Whether a saved script that `prog` runs is a program (Python, Node and
/// the like) rather than shell commands.
fn is_program(prog: &str, path: &Path, body: &str) -> bool {
    if is_interpreter(prog) {
        return is_program_interpreter(prog);
    }
    let first = body.lines().next().unwrap_or("");
    if first.starts_with("#!") {
        return ["python", "node", "ruby", "perl", "deno", "bun"]
            .iter()
            .any(|w| first.contains(w));
    }
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("py" | "js" | "mjs" | "cjs" | "ts" | "rb" | "pl")
    )
}

/// The text literals of a program (quoted strings, triple-quoted ones
/// included), with the common escapes read.
fn literals(chars: &[char]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '#' && !matches!(chars.get(i + 1), Some('!')) {
            // A comment of the program itself, to the end of the line.
            let before = chars[..i].iter().rev().take_while(|c| **c != '\n');
            if before.clone().all(|c| c.is_whitespace()) || before.clone().next() == Some(&' ') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
        }
        if !matches!(c, '"' | '\'' | '`') {
            i += 1;
            continue;
        }
        let triple = i + 2 < chars.len() && chars[i + 1] == c && chars[i + 2] == c;
        let start = if triple { i + 3 } else { i + 1 };
        let mut j = start;
        let mut text = String::new();
        let mut closed = false;
        while j < chars.len() {
            let d = chars[j];
            if d == '\\' && j + 1 < chars.len() {
                match chars[j + 1] {
                    'n' => text.push('\n'),
                    't' => text.push('\t'),
                    other => text.push(other),
                }
                j += 2;
                continue;
            }
            if triple {
                if d == c && j + 2 < chars.len() && chars[j + 1] == c && chars[j + 2] == c {
                    j += 3;
                    closed = true;
                    break;
                }
            } else if d == c {
                j += 1;
                closed = true;
                break;
            } else if d == '\n' && c != '`' {
                break;
            }
            text.push(d);
            j += 1;
        }
        if closed {
            out.push((i, j, text));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// The files a program may write: its text literals that are a whole
/// path to a code or data file (`p = 'src/a.ts'`, `open('a.go', 'w')`),
/// not a path quoted inside a longer text it writes (a note's list of
/// sources).
fn written_paths(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    literals(&chars)
        .into_iter()
        .map(|(.., text)| text)
        .filter(|t| {
            !t.contains(|c: char| c.is_whitespace() || matches!(c, '{' | '\'' | '"'))
                && CODE_PATH.is_match(&format!("'{t}'"))
        })
        .collect()
}

/// Whether a program that edits files changes only comments: every text
/// it can write is a comment line, and the lines its old and new texts do
/// not share are comments or blank (`old = '''…'''`, `new = '''…'''`,
/// `lines.insert(i, '// …')`). Paths, encodings, escapes and texts it only
/// looks for (`assert x in s`, `s.index(x)`) are not text it writes.
fn program_comment_only(body: &str) -> bool {
    let chars: Vec<char> = body.chars().collect();
    let lits = literals(&chars);
    let mut lines: Vec<HashSet<String>> = Vec::new();
    for (start, end, text) in &lits {
        let t = text.trim();
        let path_like = !t.contains(char::is_whitespace)
            && (t.contains('/') || Path::new(t).extension().is_some());
        let token = t.chars().count() <= 2
            || matches!(
                t,
                "utf8" | "utf-8" | "w" | "a" | "r" | "wb" | "rb" | "\n" | ""
            );
        let rest: String = chars[*end..].iter().take(12).collect();
        let before: String = chars[start.saturating_sub(12)..*start].iter().collect();
        let looked_for = rest.trim_start().starts_with("in ")
            || rest.trim_start().starts_with("not in ")
            || [
                ".index(",
                ".find(",
                ".count(",
                ".startswith(",
                ".endswith(",
                ".split(",
            ]
            .iter()
            .any(|m| before.ends_with(m));
        if path_like || token || looked_for {
            continue;
        }
        lines.push(text.lines().map(|l| l.trim().to_owned()).collect());
    }
    if lines.is_empty() {
        return false;
    }
    // The lines one text has and another does not: what a replacement
    // changes. One text alone changes all of its lines.
    let all: HashSet<&String> = lines.iter().flatten().collect();
    let changed: Vec<&&String> = all
        .iter()
        .filter(|l| lines.len() == 1 || lines.iter().any(|set| !set.contains(**l)))
        .collect();
    !changed.is_empty()
        && changed.iter().all(|l| is_comment(l))
        && changed.iter().any(|l| !l.trim().is_empty())
}

/// What a CI read shows: its result, or, for a read started in the
/// background (`until … gh run view …; done` with `run_in_background`),
/// the output a later call read by the task's id.
fn ci_output(
    turns: &[StoredTurn],
    ci: usize,
    result: Option<&str>,
    pair_of: &HashMap<usize, usize>,
) -> String {
    let r = result.unwrap_or("");
    let Some(id) = BACKGROUND
        .captures(r)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .filter(|id| id.len() >= 6)
    else {
        return r.to_owned();
    };
    for (i, t) in turns.iter().enumerate().skip(ci + 1) {
        if t.role != "tool" || !is_call(t) || !t.text.contains(id) {
            continue;
        }
        let name = t.tool_name.as_deref().unwrap_or("").to_lowercase();
        if name.contains("wait") || name.contains("status") {
            continue;
        }
        if let Some(out) = pair_of.get(&i).map(|ri| turns[*ri].text.as_str()) {
            if !out.trim().is_empty() {
                return out.to_owned();
            }
        }
    }
    r.to_owned()
}

/// The check kinds: what a code edit can make stale.
fn is_check(kind: ClaimKind) -> bool {
    matches!(
        kind,
        ClaimKind::Tests
            | ClaimKind::Typecheck
            | ClaimKind::Lint
            | ClaimKind::Fmt
            | ClaimKind::Build
            | ClaimKind::BrainCheck
    )
}

/// Output that shows a check of one of `kinds` failed, read when the
/// call's exit code may be a later command's.
fn shows_failure(kinds: &[ClaimKind], text: &str) -> bool {
    static FAILURE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"error TS\d+|error\[E\d+\]|(?m)^\s*error(?:\[[\w-]+\])?:|\b[1-9]\d* errors?\b|✖ [1-9]\d* problems?|\bwould reformat\b|(?m)^Diff in |BUILD FAILED|(?m)^npm ERR!|(?i)\bfound [1-9]\d* errors?")
            .expect("valid regex")
    });
    (kinds.contains(&ClaimKind::Tests) && TEST_FAILED.is_match(text)) || FAILURE.is_match(text)
}

/// How a command's result ended, for `kinds`. `last` says whether the
/// command was the call's last simple command, whose exit code the result
/// reports; for an earlier check, a non-zero exit code is a failure only
/// when the output shows one of its kind.
fn outcome(
    kinds: &[ClaimKind],
    result: Option<&str>,
    later: Option<&[StoredTurn]>,
    last: bool,
) -> Outcome {
    let Some(r) = result else {
        // No result turn: empty output, unless nothing came after the call.
        return if later.is_some_and(|l| !l.is_empty()) {
            Outcome::Succeeded
        } else {
            Outcome::Unknown
        };
    };
    let t = r.trim_start();
    if t.starts_with("<tool_use_error>") {
        return Outcome::Failed(None);
    }
    if let Some(code) = exit_code(t) {
        let own = last || !kinds.iter().all(|k| is_check(*k)) || shows_failure(kinds, t);
        if code != 0 && own {
            return Outcome::Failed(Some(code));
        }
    }
    if let Some(id) = BACKGROUND
        .captures(t)
        .map(|c| c.get(1).map_or("", |m| m.as_str()).to_owned())
    {
        return background(&id, later.unwrap_or_default());
    }
    if kinds.contains(&ClaimKind::Tests) && TEST_FAILED.is_match(t) {
        return Outcome::Failed(None);
    }
    if kinds.contains(&ClaimKind::Ci) && CI_FAILED.is_match(t) {
        return Outcome::Failed(None);
    }
    if kinds.contains(&ClaimKind::Pushed)
        && (t.contains("[rejected]") || t.contains("error: failed to push"))
    {
        return Outcome::Failed(None);
    }
    if kinds.contains(&ClaimKind::Deployed) && DEPLOY_FAILED.is_match(t) {
        return Outcome::Failed(None);
    }
    Outcome::Succeeded
}

static BACKGROUND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)running in (?:the )?background(?:[^\n]*?\bID:?\s*([\w-]+))?")
        .expect("valid regex")
});
static TEST_FAILED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"test result: FAILED|ℹ fail [1-9]|(?m)^failures:|(?m)^\s*Tests?:?\s+[1-9]\d* failed|[;,(|]\s*[1-9]\d* failed|(?m)^=+ [1-9]\d* failed|(?m)^FAILED\b")
        .expect("valid regex")
});
static CI_FAILED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bfail(?:ed|ure|ing)?\b|\bX\b").expect("valid regex"));
static DEPLOY_FAILED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)"status"\s*:\s*"(?:error|failed)"|\bdeploy(?:ment)? failed\b|\b(?:401|403|404|500) (?:unauthori[sz]ed|forbidden|not found|internal)"#)
        .expect("valid regex")
});

/// A background run's outcome, from a later turn that names its id.
fn background(id: &str, later: &[StoredTurn]) -> Outcome {
    if id.is_empty() {
        return Outcome::Unknown;
    }
    for t in later {
        if !t.text.contains(id) {
            continue;
        }
        let lower = t.text.to_lowercase();
        if lower.contains("exit code 0")
            || lower.contains("<exit-code>0</exit-code>")
            || lower.contains("status: completed")
            || lower.contains("<status>completed</status>")
        {
            return Outcome::Succeeded;
        }
        if lower.contains("<status>failed</status>")
            || lower.contains("status: failed")
            || EXIT_NONZERO.is_match(&lower)
        {
            return Outcome::Failed(None);
        }
    }
    Outcome::Unknown
}

static EXIT_NONZERO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"exit code:? [1-9]\d*|<exit-code>[1-9]\d*</exit-code>").expect("valid regex")
});

/// A test count the output shows: the sum of cargo's `N passed`, else the
/// last `N passed`.
fn count(kinds: &[ClaimKind], result: &str) -> Option<String> {
    if !kinds.contains(&ClaimKind::Tests) {
        return None;
    }
    let cargo: u64 = CARGO_OK
        .captures_iter(result)
        .filter_map(|c| c[1].parse::<u64>().ok())
        .sum();
    if cargo > 0 {
        return Some(format!("{cargo} passed"));
    }
    PASSED
        .captures_iter(result)
        .last()
        .map(|c| format!("{} passed", &c[1]))
}

static CARGO_OK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"test result: ok\. (\d+) passed").expect("valid regex"));
static PASSED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d+) (?:tests? )?passed\b").expect("valid regex"));

/// What else a transcript shows that bears on a claim but is not a
/// command hippocamp can match.
#[derive(Debug, Clone)]
enum Hint {
    /// A delegate's result (an `Agent`, `Task` or delegation tool's).
    Report(String),
    /// A script or job whose commands hippocamp cannot see (`bash ci.sh`,
    /// `./check`, `just`, a background launch).
    Script(String),
    /// A tool result's text, and whether a command produced it (a shell
    /// call's result, a background task's notification or the read of its
    /// output), which can show a check ran.
    Output(String, bool),
    /// A fetch of a web address that is not this machine's.
    Fetch(String),
}

#[derive(Debug, Clone)]
struct Seen {
    ts: String,
    ordinal: u32,
    sub: Option<String>,
    hint: Hint,
}

/// What [`read_turns`] collects.
#[derive(Default)]
struct Found {
    execs: Vec<Exec>,
    edits: Vec<Edit>,
    seen: Vec<Seen>,
}

static WRITES_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\.write\(|write_text\(|writeFileSync\(|open\([^)]*['"][wa]['"]|File\.write"#)
        .expect("valid regex")
});
static CODE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"['"]([^'"\s]+\.(?:rs|ts|tsx|js|jsx|mjs|go|py|swift|kt|java|c|h|cpp|toml|json|yaml|yml|sql|css|html|vue|svelte|sh|md|txt))['"]"#)
        .expect("valid regex")
});
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s'"<>)]+"#).expect("valid regex"));

fn is_local(url: &str) -> bool {
    url.contains("://localhost")
        || url.contains("://127.0.0.1")
        || url.contains("://0.0.0.0")
        || url.contains("://[::1]")
        || url.contains("github.com")
}

/// The commands, edits and hints of one transcript (the session or a
/// sub-agent).
fn read_turns(turns: &[StoredTurn], sub: Option<&str>, dir: Option<&Path>, f: &mut Found) {
    let mut cwd = Cwd {
        dir: dir.map(Path::to_path_buf),
    };
    let sub_id = || sub.map(str::to_owned);
    // Scripts a heredoc saved to a file, by path, read when a later
    // command runs them.
    let mut saved: HashMap<PathBuf, String> = HashMap::new();
    let pair_of: HashMap<usize, usize> = shell::pairs(turns)
        .into_iter()
        .filter_map(|(c, r)| r.map(|r| (c, r)))
        .collect();
    // The results that a command produced: a shell call's, a background
    // task's output read by a tool, and a task's notification.
    let mut from_run: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (ci, ri) in shell::pairs(turns) {
        let Some(ri) = ri else { continue };
        let name = turns[ci].tool_name.as_deref().unwrap_or("").to_lowercase();
        if shell::script(&turns[ci]).is_some()
            || [
                "taskoutput",
                "task_output",
                "bashoutput",
                "background_wait",
                "task_status",
            ]
            .iter()
            .any(|w| name.contains(w))
        {
            from_run.insert(ri);
        }
    }
    for (i, t) in turns.iter().enumerate() {
        let notification = t.role == "user" && t.text.starts_with("<task-notification>");
        if (t.role == "tool" && !is_call(t) || notification) && !t.text.trim().is_empty() {
            f.seen.push(Seen {
                ts: t.ts.clone(),
                ordinal: t.ordinal,
                sub: sub_id(),
                hint: Hint::Output(t.text.clone(), notification || from_run.contains(&i)),
            });
        }
    }
    for (ci, ri) in shell::pairs(turns) {
        let t = &turns[ci];
        let result = ri.map(|r| turns[r].text.as_str());
        let name = t.tool_name.as_deref().unwrap_or("");
        let edit = |path: PathBuf, pos: u32, comment_only: bool, f: &mut Found| {
            f.edits.push(Edit {
                ts: t.ts.clone(),
                ordinal: t.ordinal,
                sub: sub_id(),
                path,
                pos,
                comment_only,
            })
        };
        if let Some(script) = shell::script(t) {
            let mut commands = cwd.step(&script, result);
            // A check run against a deliberately broken copy, restored from
            // a backup later in the same call (a mutation test), shows
            // nothing about the code as it stands: neither its edits nor
            // its run count. Putting work aside first (`git stash push`,
            // then the checks) checks the committed state, and counts.
            let restore_at = commands.iter().rposition(|(c, _)| {
                c.program().is_some_and(|(p, args)| {
                    matches!(p.as_str(), "cp" | "mv")
                        && args
                            .iter()
                            .find(|a| !a.starts_with('-'))
                            .is_some_and(|src| {
                                let src = src.to_lowercase();
                                src.contains(".bak")
                                    || src.contains(".orig")
                                    || src.contains("backup")
                            })
                }) || (c.words.first().map(String::as_str) == Some("git")
                    && (c.words.iter().any(|w| w == "restore")
                        || (c.words.iter().any(|w| w == "stash")
                            && c.words.iter().any(|w| w == "pop" || w == "apply"))))
            });
            let first_check = commands
                .iter()
                .position(|(c, _)| kinds_of(c).into_iter().any(is_check));
            let restores = matches!((restore_at, first_check), (Some(r), Some(k)) if r > k);
            if restores {
                continue;
            }
            // Heredocs: a script saved to a file (`cat >
            // run.sh <<EOF`) is kept, and read where a command runs it, in
            // this call or a later one; a heredoc a program reads
            // (`python3 - <<EOF`) is that program's input.
            let mut programs: Vec<(String, Option<usize>)> = Vec::new();
            for (target, body) in shell::heredoc_targets(&script) {
                match target {
                    Some(target) => {
                        let at = commands
                            .iter()
                            .find(|(c, _)| c.written.contains(&target))
                            .map_or_else(|| cwd.dir.clone(), |(_, d)| d.clone());
                        if let Some(p) = resolve(&target, at.as_deref()) {
                            saved.insert(p, body);
                        }
                    }
                    None => programs.push((body, None)),
                }
            }
            let mut expanded: Vec<(Simple, Option<PathBuf>)> = Vec::with_capacity(commands.len());
            for (c, d) in commands {
                let run = runs_file(&c).and_then(|(prog, file)| {
                    let p = resolve(&file, d.as_deref())?;
                    saved.get(&p).map(|body| (prog, p, body.clone()))
                });
                expanded.push((c, d.clone()));
                if let Some((prog, path, body)) = run {
                    if is_program(&prog, &path, &body) {
                        programs.push((body, Some(expanded.len() - 1)));
                    } else {
                        let mut inner = Cwd { dir: d };
                        expanded.extend(inner.step(&body, None));
                    }
                }
            }
            commands = expanded;
            let later = &turns[ci + 1..];
            let total = commands.len();
            let mut runs: Vec<Run> = Vec::new();
            for (pos, (c, run_dir)) in commands.iter().enumerate() {
                for k in kinds_of(c) {
                    if let Some(have) = runs.iter_mut().find(|r| r.kind == k) {
                        // The last run of a kind in the call is the one
                        // that counts.
                        have.pos = pos as u32;
                        have.dir = run_dir.clone();
                        have.shown = one_line(&c.words.join(" "), COMMAND_CHARS);
                        have.result = outcome(&[k], result, Some(later), pos + 1 == total);
                    } else {
                        runs.push(Run {
                            kind: k,
                            shown: one_line(&c.words.join(" "), COMMAND_CHARS),
                            dir: run_dir.clone(),
                            pos: pos as u32,
                            result: outcome(&[k], result, Some(later), pos + 1 == total),
                        });
                    }
                }
                if let Some((prog, args)) = c.program() {
                    let after_dashes = c
                        .words
                        .iter()
                        .position(|w| w == "--")
                        .and_then(|i| c.words.get(i + 1))
                        .is_some_and(|w| {
                            matches!(w.as_str(), "bash" | "sh" | "zsh") || w.starts_with("./")
                        });
                    let script_run = after_dashes
                        || matches!(prog.as_str(), "bash" | "sh" | "zsh")
                            && args.first().is_some_and(|a| !a.starts_with('-'))
                        || c.words[0].starts_with("./")
                        || matches!(prog.as_str(), "just" | "task");
                    if script_run {
                        f.seen.push(Seen {
                            ts: t.ts.clone(),
                            ordinal: t.ordinal,
                            sub: sub_id(),
                            hint: Hint::Script(one_line(&c.words.join(" "), COMMAND_CHARS)),
                        });
                    }
                    if matches!(prog.as_str(), "curl" | "wget" | "http" | "xh") {
                        for w in &args {
                            if (w.starts_with("http://") || w.starts_with("https://"))
                                && !is_local(w)
                            {
                                f.seen.push(Seen {
                                    ts: t.ts.clone(),
                                    ordinal: t.ordinal,
                                    sub: sub_id(),
                                    hint: Hint::Fetch((*w).to_owned()),
                                });
                            }
                        }
                    }
                }
                // Shell writes: `>` targets and `sed -i`/`perl -i` files.
                let mut written: Vec<String> = c.written.clone();
                let mut comment_only = false;
                if let Some((prog, args)) = c.program() {
                    if matches!(prog.as_str(), "sed" | "perl")
                        && args.iter().any(|a| a.starts_with("-i") || *a == "-pi")
                    {
                        if let Some(last) = args.last() {
                            written.push((*last).to_owned());
                        }
                        comment_only = prog == "sed" && sed_inserts_comment(&args);
                    }
                }
                for w in written {
                    if w.starts_with("/dev/")
                        || w.starts_with('&')
                        || w.chars().all(|c| c.is_ascii_digit())
                    {
                        continue;
                    }
                    let p = PathBuf::from(&w);
                    let p = if p.is_absolute() {
                        p
                    } else if let Some(d) = run_dir {
                        shell::lexical(&d.join(p))
                    } else {
                        continue;
                    };
                    edit(p, pos as u32, comment_only, f);
                }
            }
            // A program that writes files (`open(p, 'w')`) from a heredoc
            // it reads or a saved script it runs: its file paths are edits,
            // made where that program runs. An edit whose new text is only
            // comments cannot change a check.
            for (body, pos) in programs {
                if !WRITES_FILE.is_match(&body) {
                    continue;
                }
                let pos = pos
                    .or_else(|| {
                        commands.iter().position(|(c, _)| {
                            c.program().is_some_and(|(p, _)| is_program_interpreter(&p))
                        })
                    })
                    .unwrap_or(0);
                let body_dir = commands
                    .get(pos)
                    .and_then(|(_, d)| d.clone())
                    .or_else(|| cwd.dir.clone());
                let comment_only = program_comment_only(&body);
                for path in written_paths(&body) {
                    let p = PathBuf::from(&path);
                    let p = if p.is_absolute() {
                        p
                    } else if let Some(d) = &body_dir {
                        shell::lexical(&d.join(p))
                    } else {
                        continue;
                    };
                    edit(p, pos as u32, comment_only, f);
                }
            }
            if let Some(r) = result {
                if BACKGROUND.is_match(r) && runs.is_empty() {
                    f.seen.push(Seen {
                        ts: t.ts.clone(),
                        ordinal: t.ordinal,
                        sub: sub_id(),
                        hint: Hint::Script(one_line(&script, COMMAND_CHARS)),
                    });
                }
            }
            if !runs.is_empty() {
                let plain: Vec<ClaimKind> = runs.iter().map(|r| r.kind).collect();
                f.execs.push(Exec {
                    ts: t.ts.clone(),
                    ordinal: t.ordinal,
                    sub: sub_id(),
                    command: one_line(&script, COMMAND_CHARS),
                    count: result.and_then(|r| count(&plain, r)),
                    output: plain
                        .contains(&ClaimKind::Ci)
                        .then(|| ci_output(turns, ci, result, &pair_of)),
                    runs,
                });
            }
            continue;
        }
        if is_edit_tool(name) {
            let input = &t.text[name.len() + 1..];
            let comment_only = comment_only_edit(input);
            for p in edited_paths(name, input) {
                let path = PathBuf::from(&p);
                let path = if path.is_absolute() {
                    path
                } else if let Some(d) = &cwd.dir {
                    shell::lexical(&d.join(path))
                } else {
                    path
                };
                edit(path, 0, comment_only, f);
            }
            continue;
        }
        let lower = name.to_lowercase();
        let reads_delegate = [
            "delegate",
            "t3_thread",
            "task_status",
            "task_output",
            "taskoutput",
            "background_wait",
            "sendmessage",
        ]
        .iter()
        .any(|w| lower.contains(w));
        if is_agent_tool(name) || reads_delegate {
            if let Some(r) = result {
                f.seen.push(Seen {
                    ts: t.ts.clone(),
                    ordinal: t.ordinal,
                    sub: sub_id(),
                    hint: Hint::Report(r.to_owned()),
                });
            }
        }
        if lower == "webfetch" || lower.contains("preview_navigate") || lower.contains("navigate") {
            if let Some(url) = URL.find(&t.text) {
                if !is_local(url.as_str()) {
                    f.seen.push(Seen {
                        ts: t.ts.clone(),
                        ordinal: t.ordinal,
                        sub: sub_id(),
                        hint: Hint::Fetch(url.as_str().to_owned()),
                    });
                }
            }
        }
        // A deploy or publish tool (an MCP server's).
        if lower.contains("deploy") || (lower.contains("release") && lower.contains("publish")) {
            f.execs.push(Exec {
                ts: t.ts.clone(),
                ordinal: t.ordinal,
                sub: sub_id(),
                command: name.to_owned(),
                runs: vec![Run {
                    kind: ClaimKind::Deployed,
                    shown: name.to_owned(),
                    dir: None,
                    pos: 0,
                    result: outcome(&[ClaimKind::Deployed], result, Some(&turns[ci + 1..]), true),
                }],
                count: None,
                output: None,
            });
        }
    }
}

/// Whether a line is a comment or blank.
fn is_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        || ["//", "#", "--", "/*", "*", "<!--", "-->", ";"]
            .iter()
            .any(|m| t.starts_with(m))
}

/// Whether an `Edit` or `MultiEdit` call's input changes only comment
/// lines: every line its new text adds and its old text loses is a
/// comment or blank.
fn comment_only_edit(input: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(input) else {
        return false;
    };
    let pairs: Vec<(&str, &str)> = match v.get("edits").and_then(|e| e.as_array()) {
        Some(edits) => edits
            .iter()
            .filter_map(|e| {
                Some((
                    e.get("old_string")?.as_str()?,
                    e.get("new_string")?.as_str()?,
                ))
            })
            .collect(),
        None => match (
            v.get("old_string").and_then(|x| x.as_str()),
            v.get("new_string").and_then(|x| x.as_str()),
        ) {
            (Some(o), Some(n)) => vec![(o, n)],
            _ => return false,
        },
    };
    if pairs.is_empty() {
        return false;
    }
    pairs.iter().all(|(old, new)| {
        let old_lines: std::collections::HashSet<&str> = old.lines().map(str::trim).collect();
        let new_lines: std::collections::HashSet<&str> = new.lines().map(str::trim).collect();
        let changed: Vec<&&str> = old_lines.symmetric_difference(&new_lines).collect();
        !changed.is_empty() && changed.iter().all(|l| is_comment(l))
    })
}

/// Whether `sed -i` arguments only insert or append a comment line
/// (`12i\ // eslint-disable-next-line …`).
fn sed_inserts_comment(args: &[&str]) -> bool {
    static INSERT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\s*(?:\d+|\$|/[^/]*/)\s*[ia]\\?\s*(.*)$").expect("valid regex")
    });
    let exprs: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !a.starts_with('-') && *a != "''" && !a.is_empty())
        .collect();
    // The last plain argument is the file; the expressions come before.
    let Some((_, exprs)) = exprs.split_last() else {
        return false;
    };
    !exprs.is_empty()
        && exprs.iter().all(|e| {
            INSERT
                .captures(e)
                .is_some_and(|c| is_comment(&c[1]) && !c[1].trim().is_empty())
        })
}

/// Whether an edit can change what a `kind` command run in `root` shows.
fn relevant(kind: ClaimKind, edit: &Edit, root: Option<&Path>, notes_roots: &[PathBuf]) -> bool {
    let path = edit.path.as_path();
    // Scratch files are not the code a check checks.
    if [
        "/tmp/",
        "/private/tmp/",
        "/var/folders/",
        "/private/var/folders/",
    ]
    .iter()
    .any(|t| path.starts_with(t))
    {
        return false;
    }
    let in_notes = notes_roots.iter().any(|r| path.starts_with(r));
    if kind == ClaimKind::BrainCheck {
        return in_notes;
    }
    if in_notes || edit.comment_only {
        return false;
    }
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    if parts.iter().any(|p| p == ".hippocamp") {
        return false;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    // Prose, and the output a command was sent to (`> run.log`, `vitest.out`).
    let prose = matches!(
        ext.as_str(),
        "md" | "mdx" | "txt" | "rst" | "log" | "out" | "err"
    );
    let image = matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "pdf"
    );
    let docs_data = parts.iter().any(|p| p == "docs" || p == "doc")
        && matches!(
            ext.as_str(),
            "json" | "yaml" | "yml" | "csv" | "html" | "svg"
        );
    if prose || image || docs_data {
        return false;
    }
    if kind == ClaimKind::Build {
        let name = parts.last().map(String::as_str).unwrap_or("");
        let test_file = name.ends_with("_test.go")
            || name.contains(".test.")
            || name.contains(".spec.")
            || parts.iter().any(|p| p == "__tests__" || p == "tests");
        if test_file {
            return false;
        }
    }
    match root {
        Some(r) => path.starts_with(r),
        None => true,
    }
}

/// The repository root holding `dir` (a `.git` file or directory in it or
/// a parent), else `dir`.
fn repo_root(dir: &Path, cache: &mut HashMap<PathBuf, PathBuf>) -> PathBuf {
    if let Some(r) = cache.get(dir) {
        return r.clone();
    }
    let root = dir
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(dir)
        .to_path_buf();
    cache.insert(dir.to_path_buf(), root.clone());
    root
}

/// Output that shows a kind passed, when a log or a script's output is
/// read rather than the command seen.
fn marker(kind: ClaimKind) -> Option<&'static Regex> {
    static MARKERS: LazyLock<Vec<(ClaimKind, Regex)>> = LazyLock::new(|| {
        let p = |k, re: &str| (k, Regex::new(re).expect("valid regex"));
        vec![
            p(
                ClaimKind::Tests,
                r"test result: ok\. [1-9]\d* passed|(?m)^\s*Tests?:?\s+\d+ passed|Run [^\n]*\btest\b[^\n]* completed success|(?m)^ok\s+\S+\s+\(?[\d.]+s|(?m)^\s*--- PASS|ℹ pass [1-9]\d*\s+ℹ fail 0|(?i)\b[1-9][\d,]* (?:tests? )?passed\b|(?i)\btests?\b[ :=-]{1,3}(?:ok|passed|pass)\b|(?i)\btests?\b[^\n]{0,20}\b(?:rc|exit(?: code)?|status)\s*[:=]?\s*0\b",
            ),
            p(
                ClaimKind::Fmt,
                r"(?i)\b(?:fmt|format(?:ting)?|gofmt|prettier)\b[ :=-]{1,3}(?:ok|clean|passed)\b|Run [^\n]*\bfmt\b[^\n]* completed success|(?i)\b(?:fmt|format|gofmt|prettier)\b[^\n]{0,20}\b(?:rc|exit(?: code)?|status)\s*[:=]?\s*0\b",
            ),
            p(
                ClaimKind::Lint,
                r"(?i)\b(?:clippy|lint|eslint|vet)\b[ :=-]{1,3}(?:ok|clean|passed)\b|Run [^\n]*\b(?:clippy|lint)\b[^\n]* completed success|(?i)\b(?:clippy|lint|eslint|vet)\b[^\n]{0,20}\b(?:rc|exit(?: code)?|status)\s*[:=]?\s*0\b|(?i)\blint\b[^\n]{0,10}\b0 errors\b",
            ),
            p(
                ClaimKind::Typecheck,
                r"(?i)\b(?:tsc|typecheck|type-check)\b[ :=-]{1,3}(?:ok|clean|passed)\b|(?i)\b(?:tsc|typecheck|type-check)\b[^\n]{0,20}\b(?:rc|exit(?: code)?|status)\s*[:=]?\s*0\b",
            ),
            p(
                ClaimKind::Build,
                r"(?m)^\s*Finished `(?:release|dev|test)` profile|(?i)\bbuild\b[ :=-]{1,3}(?:ok|succeeded|passed)\b|\*\* BUILD SUCCEEDED \*\*|(?i)\bbuild\b[^\n]{0,20}\b(?:rc|exit(?: code)?|status)\s*[:=]?\s*0\b",
            ),
            p(
                ClaimKind::BrainCheck,
                r"(?i)(?:(?:check|lint|validate)[_-](?:brain|notes|vault|wiki)|notes check|hippocamp lint)[^\n]{0,40}\b(?:ok|passed|clean)\b",
            ),
        ]
    });
    MARKERS.iter().find(|(k, _)| *k == kind).map(|(_, re)| re)
}

/// The number of tests a claim states (`all 47 tests pass`, `1,190 tests
/// passed`), when it states one.
fn claimed_count(claim: &str) -> Option<u64> {
    static N: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(\d[\d,]*)\s+(?:[\w-]+\s+){0,2}(?:tests?\b|passed\b)")
            .expect("valid regex")
    });
    N.captures(claim)
        .and_then(|c| c[1].replace(',', "").parse().ok())
}

/// The words a claim may name a place by: none of the check words.
fn place_words(claim: &str) -> Vec<String> {
    const CHECK: &[&str] = &[
        "test",
        "tests",
        "typecheck",
        "type",
        "check",
        "checks",
        "lint",
        "linting",
        "clean",
        "pass",
        "passes",
        "passed",
        "passing",
        "green",
        "build",
        "builds",
        "built",
        "and",
        "the",
        "all",
        "are",
        "now",
        "fmt",
        "format",
        "formatting",
        "clippy",
        "eslint",
        "tsc",
        "vitest",
        "jest",
        "pytest",
        "cargo",
        "suite",
        "unit",
        "full",
        "with",
        "both",
        "also",
        "too",
        "still",
        "again",
        "after",
        "local",
        "locally",
        "short",
        "vet",
        "go",
        "npm",
        "warnings",
        "errors",
        "error",
        "compiles",
        "cleanly",
        "succeeds",
        "succeeded",
        "run",
        "runs",
        "ran",
        "its",
        "their",
        "this",
        "that",
        "for",
        "every",
        "files",
        "file",
    ];
    claim
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.len() >= 3 && !w.chars().all(|c| c.is_ascii_digit()))
        .filter(|w| !CHECK.contains(&w.as_str()))
        .collect()
}

/// The words of the place a check ran in: its repository's name and the
/// root folder's name, split at `-`, `_` and `.`, and the folders between
/// the root and where it ran.
fn place_of(dir: &Path, roots: &mut HashMap<PathBuf, PathBuf>, repos: &mut Repos) -> Vec<String> {
    let root = repo_root(dir, roots);
    let mut names: Vec<String> = Vec::new();
    if let Some(repo) = repos.of(&root) {
        names.push(repo.name);
    }
    if let Some(n) = root.file_name() {
        names.push(n.to_string_lossy().into_owned());
    }
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let n = n.to_lowercase();
        out.extend(
            n.split(['-', '_', '.'])
                .filter(|p| p.len() >= 3)
                .map(str::to_owned),
        );
        out.push(n);
    }
    if let Ok(below) = dir.strip_prefix(&root) {
        out.extend(
            below
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                .filter(|p| p.len() >= 3),
        );
    }
    out
}

/// Whether a claim's word names a part of a place.
fn names_part(word: &str, part: &str) -> bool {
    word == part
        || (word.len() >= 4 && part.starts_with(word))
        || (part.len() >= 4 && word.starts_with(part))
}

/// Checks each claim of the session against the evidence.
pub fn check(inp: &Inputs<'_>) -> Vec<Claim> {
    let f = found(inp);
    let summarised: Vec<u32> = inp
        .turns
        .iter()
        .filter(|t| {
            matches!(
                crate::session_meta::classify(&t.role, t.tool_name.as_deref(), &t.text),
                crate::session_meta::TurnKind::Injection(
                    crate::session_meta::Injection::Summary
                        | crate::session_meta::Injection::ContextHandoff
                )
            )
        })
        .map(|t| t.ordinal)
        .collect();
    let mut repos = Repos::new();
    let mut all_turns: Vec<StoredTurn> = inp.turns.to_vec();
    for (_, t) in inp.subs {
        all_turns.extend(t.iter().cloned());
    }
    all_turns.sort_by(|a, b| a.ts.cmp(&b.ts));
    let pulls = work::read(&all_turns, inp.dir, &mut repos).pulls;
    // The person's requests, for "re-checked before the next request".
    let requests: Vec<&str> = inp
        .turns
        .iter()
        .filter(|t| {
            crate::session_meta::classify(&t.role, t.tool_name.as_deref(), &t.text)
                == crate::session_meta::TurnKind::Request
        })
        .map(|t| t.ts.as_str())
        .collect();

    // Claims, the last of each kind and number.
    let mut found: Vec<(ClaimKind, Option<u64>, String, u32, String)> = Vec::new();
    for t in inp.turns {
        if t.role != "assistant" || is_call(t) || t.text.trim_start().starts_with("API Error") {
            continue;
        }
        for (kind, pr, text) in claims_in(&t.text) {
            found.retain(|(k, n, ..)| !(*k == kind && *n == pr));
            found.push((kind, pr, text, t.ordinal, t.ts.clone()));
        }
    }
    let mut roots: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut out = Vec::new();
    for (kind, pr, text, ordinal, ts) in found {
        let claim = Said {
            kind,
            pr,
            text: &text,
            ts: &ts,
            ordinal,
        };
        let (status, evidence) = match kind {
            ClaimKind::PrMerged | ClaimKind::PrOpened => {
                let want = if kind == ClaimKind::PrMerged {
                    PullDone::Merged
                } else {
                    PullDone::Opened
                };
                let hit = pulls.iter().rev().find(|(url, done, at)| {
                    *done == want
                        && at.as_str() <= ts.as_str()
                        && pr.is_none_or(|n| url.ends_with(&format!("/pull/{n}")))
                });
                match hit {
                    Some((url, done, _)) => (Status::Verified, format!("{} {url}", done.as_str())),
                    None => pull_verdict(&claim, &f),
                }
            }
            _ => verdict(&claim, &f, inp, &mut roots, &mut repos),
        };
        // A claim made early and re-checked before the person's next
        // request, with no relevant edit in between.
        let (status, evidence) = if status == Status::Unverified && is_check(kind) {
            let next = requests
                .iter()
                .find(|r| **r > ts.as_str())
                .copied()
                .unwrap_or("\u{10ffff}");
            match soon_after(&claim, next, &f, inp, &mut roots) {
                Some(found) => (Status::Verified, found),
                None => (status, evidence),
            }
        } else {
            (status, evidence)
        };
        // Evidence may sit in the part of the conversation a summary
        // replaced.
        let (status, evidence) = match summarised.iter().rev().find(|s| **s < ordinal) {
            Some(s) if status == Status::Unverified && !evidence.contains(" failed") => (
                Status::Unclear,
                format!("{evidence}; the conversation was summarised at turn {s}, and earlier evidence may be in what it replaced"),
            ),
            _ => (status, evidence),
        };
        out.push(Claim {
            kind,
            status,
            claim: text.clone(),
            ordinal,
            evidence,
            pr,
        });
    }
    out.sort_by_key(|c| c.ordinal);
    out
}

/// The commands, edits and hints of a session and its sub-agents, in
/// order.
fn found(inp: &Inputs<'_>) -> Found {
    let mut f = Found::default();
    read_turns(inp.turns, None, inp.dir, &mut f);
    for (id, turns) in inp.subs {
        read_turns(turns, Some(id), inp.dir, &mut f);
    }
    f.execs.sort_by(|a, b| a.ts.cmp(&b.ts));
    f.edits.sort_by(|a, b| a.ts.cmp(&b.ts));
    f.seen
        .sort_by(|a, b| a.ts.cmp(&b.ts).then(a.ordinal.cmp(&b.ordinal)));
    f
}

/// Whether a session's own transcript shows a check of `kind` passing
/// after its last relevant edit, before the time `before`: what a result
/// relayed from it can rest on. Gives the time of the
/// last passing run of the kind and the repository it ran in, when known.
pub fn shown(inp: &Inputs<'_>, kind: ClaimKind, before: &str) -> Option<(String, Option<PathBuf>)> {
    if !is_check(kind) {
        return None;
    }
    let f = found(inp);
    let mut roots: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut repos = Repos::new();
    let said = Said {
        kind,
        pr: None,
        text: "",
        ts: before,
        ordinal: u32::MAX,
    };
    let (status, _) = verdict(&said, &f, inp, &mut roots, &mut repos);
    if status != Status::Verified {
        return None;
    }
    let run = f
        .execs
        .iter()
        .rev()
        .find(|e| said.after(&e.ts, e.ordinal, &e.sub) && e.result(kind) == Outcome::Succeeded);
    let root = run
        .and_then(|e| e.shows(kind))
        .and_then(|r| r.dir.as_deref())
        .map(|d| repo_root(d, &mut roots));
    Some((
        run.map_or_else(|| before.to_owned(), |e| e.ts.clone()),
        root,
    ))
}

/// A check of the claim's kind that passed after the claim and before
/// `next` (the person's next request), with no relevant edit between the
/// claim and it: the claim was early, then shown.
fn soon_after(
    c: &Said<'_>,
    next: &str,
    f: &Found,
    inp: &Inputs<'_>,
    roots: &mut HashMap<PathBuf, PathBuf>,
) -> Option<String> {
    let claim_at = Place {
        ts: c.ts,
        sub: None,
        ordinal: c.ordinal,
        pos: u32::MAX,
    };
    for e in &f.execs {
        let Some(run) = e.shows(c.kind) else { continue };
        let at = e.at(c.kind);
        if !(at.ts > c.ts || at.after(&claim_at)) || e.ts.as_str() >= next {
            continue;
        }
        if run.result != Outcome::Succeeded {
            continue;
        }
        let root = run.dir.as_deref().map(|d| repo_root(d, roots));
        let edited = f.edits.iter().any(|ed| {
            ed.ts.as_str() >= c.ts
                && at.after(&ed.at())
                && relevant(c.kind, ed, root.as_deref(), inp.notes_roots)
        });
        if !edited {
            return Some(format!(
                "{} · exit 0 · {}, after the claim",
                run.shown,
                at_turn(e.ordinal, &e.sub)
            ));
        }
    }
    None
}

/// A claim being checked.
struct Said<'a> {
    kind: ClaimKind,
    pr: Option<u64>,
    text: &'a str,
    ts: &'a str,
    ordinal: u32,
}

impl Said<'_> {
    /// Whether something at `ts` (turn `ordinal` of `sub`) came before the
    /// claim.
    fn after(&self, ts: &str, ordinal: u32, sub: &Option<String>) -> bool {
        ts < self.ts || (sub.is_none() && ts == self.ts && ordinal < self.ordinal)
    }
}

fn at_turn(ordinal: u32, sub: &Option<String>) -> String {
    match sub {
        Some(s) => format!("turn {ordinal} in sub-agent {}", short(s)),
        None => format!("turn {ordinal}"),
    }
}

/// A pull request claim the session's own merges and openings do not
/// show: a failed attempt is unverified; a state a command printed
/// verifies it; else it may have happened in another session: unclear.
fn pull_verdict(c: &Said<'_>, f: &Found) -> (Status, String) {
    let names = |text: &str| {
        text.contains('$')
            || c.pr.is_none_or(|n| {
                text.contains(&format!("/pull/{n}"))
                    || text.contains(&format!("#{n}"))
                    || text
                        .split(|ch: char| !ch.is_ascii_alphanumeric())
                        .any(|w| w == n.to_string())
            })
    };
    let tries: Vec<&Exec> = f
        .execs
        .iter()
        .filter(|e| c.after(&e.ts, e.ordinal, &e.sub))
        .filter(|e| e.shows(c.kind).is_some_and(|r| names(&r.shown)))
        .collect();
    if let Some(e) = tries
        .iter()
        .rev()
        .find(|e| e.result(c.kind) == Outcome::Succeeded)
    {
        let cmd = e.shows(c.kind).map_or(e.command.as_str(), |r| &r.shown);
        return (
            Status::Verified,
            format!("{cmd} · exit 0 · {}", at_turn(e.ordinal, &e.sub)),
        );
    }
    let state = if c.kind == ClaimKind::PrMerged {
        "MERGED"
    } else {
        "OPEN"
    };
    if let Some(s) = f.seen.iter().rev().find(|s| {
        c.after(&s.ts, s.ordinal, &s.sub)
            && matches!(&s.hint, Hint::Output(text, _) if c.pr.is_some() && names(text) && text.contains(state))
    }) {
        return (
            Status::Verified,
            format!("output at {} shows it {}", at_turn(s.ordinal, &s.sub), state.to_lowercase()),
        );
    }
    if let Some(e) = tries.last() {
        let cmd = e.shows(c.kind).map_or(e.command.as_str(), |r| &r.shown);
        return (
            Status::Unverified,
            format!("{cmd} did not succeed at {}", at_turn(e.ordinal, &e.sub)),
        );
    }
    let what = if c.kind == ClaimKind::PrMerged {
        "merged"
    } else {
        "opened"
    };
    (
        Status::Unclear,
        format!("not {what} in this session; it may have been {what} in another"),
    )
}

/// The words a CI row's name may hold for each check kind.
fn ci_words(kind: ClaimKind) -> &'static [&'static str] {
    match kind {
        ClaimKind::Tests => &["test"],
        ClaimKind::Typecheck => &["typecheck", "type-check", "tsc", "types"],
        ClaimKind::Lint => &["lint", "eslint", "clippy", "vet"],
        ClaimKind::Fmt => &["fmt", "format"],
        ClaimKind::Build => &["build"],
        _ => &[],
    }
}

/// Whether a CI read's output shows the claim's kind passing: every check
/// passed and none failed or is pending, or a check named for the kind
/// passed.
fn ci_shows(kind: ClaimKind, output: &str) -> bool {
    let lower = output.to_lowercase();
    let fail = |l: &str| {
        let t = l.trim_start();
        l.contains("\tfail")
            || t.starts_with("x ")
            || t.starts_with('✗')
            || t.starts_with('×')
            || l.contains("failure")
            || l.contains("\"conclusion\":\"failure\"")
            || l.contains("cancelled")
            || l.contains("timed_out")
            || l.contains("action_required")
    };
    let pending = |l: &str| {
        let t = l.trim_start();
        l.contains("\tpending")
            || t.starts_with("* ")
            || l.contains("in_progress")
            || l.contains("queued")
            || l.contains("in progress")
            || l.contains("\"pending\"")
            || l.contains("\"expected\"")
    };
    // A row of `gh pr checks`, `gh run view`, or of a status rollup read
    // as JSON (`"conclusion":"SUCCESS"`, or reshaped by `-q`/`--jq` to
    // `"s":"SUCCESS"` or a bare `success`).
    let pass = |l: &str| {
        let t = l.trim();
        l.contains("\tpass")
            || t.starts_with('✓')
            || t.starts_with('√')
            || l.contains("completed with 'success'")
            || l.contains("completed success")
            || l.contains("\"success\"")
            || t == "success"
            || t.ends_with("/success")
            || t.ends_with(":success")
    };
    // One row of a JSON array per line.
    let lower = lower.replace("},{", "}\n{");
    let lines: Vec<&str> = lower.lines().collect();
    let all = !lines.iter().any(|l| fail(l) || pending(l))
        && (lower.contains("all checks were successful") || lines.iter().any(|l| pass(l)));
    if all {
        return true;
    }
    lines.iter().any(|l| {
        pass(l)
            && !fail(l)
            && !pending(l)
            && ci_words(kind).iter().any(|w| {
                l.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                    .any(|part| part.contains(w))
            })
    })
}

/// A claim checked against the commands that can show it.
fn verdict(
    c: &Said<'_>,
    f: &Found,
    inp: &Inputs<'_>,
    roots: &mut HashMap<PathBuf, PathBuf>,
    repos: &mut Repos,
) -> (Status, String) {
    let kind = c.kind;
    // A build claim is also shown by a typecheck, lint or test run, which
    // compile the code.
    let mut matching: Vec<&Exec> = f
        .execs
        .iter()
        .filter(|e| e.shows(kind).is_some() && c.after(&e.ts, e.ordinal, &e.sub))
        .collect();
    // The repository the claim is about: when the claim
    // names a place some of its checks ran in, only those count.
    let words = place_words(c.text);
    {
        let mut named_dir = |d: &Path| {
            let parts = place_of(d, roots, repos);
            words.iter().any(|w| parts.iter().any(|p| names_part(w, p)))
        };
        // Does the claim name a place any check of the session ran in?
        let names_a_place = !words.is_empty()
            && f.execs.iter().any(|e| {
                c.after(&e.ts, e.ordinal, &e.sub)
                    && e.runs
                        .iter()
                        .any(|r| is_check(r.kind) && r.dir.as_deref().is_some_and(&mut named_dir))
            });
        if names_a_place {
            matching.retain(|e| {
                e.shows(kind)
                    .and_then(|r| r.dir.as_deref())
                    .is_some_and(&mut named_dir)
            });
        } else {
            // A sentence that claims several checks is about the places
            // where all of them ran ("typecheck, lint and tests pass" in a
            // session that ran the tests and lint in two repositories but
            // typechecked one): only checks in those places count.
            let places = |k: ClaimKind, roots: &mut HashMap<PathBuf, PathBuf>| {
                let mut out: HashSet<PathBuf> = HashSet::new();
                for e in &f.execs {
                    if !c.after(&e.ts, e.ordinal, &e.sub) {
                        continue;
                    }
                    if let Some(d) = e.shows(k).and_then(|r| r.dir.as_deref()) {
                        out.insert(repo_root(d, roots));
                    }
                }
                out
            };
            let mut shared: Option<HashSet<PathBuf>> = None;
            for (k, ..) in claims_in(c.text) {
                if !is_check(k) || k == ClaimKind::BrainCheck {
                    continue;
                }
                let at = places(k, roots);
                if at.is_empty() {
                    continue;
                }
                shared = Some(match shared {
                    Some(s) => s.intersection(&at).cloned().collect(),
                    None => at,
                });
            }
            if let Some(shared) = shared.filter(|s| !s.is_empty()) {
                matching.retain(|e| {
                    e.shows(kind)
                        .and_then(|r| r.dir.as_deref())
                        .is_some_and(|d| shared.contains(&repo_root(d, roots)))
                });
            }
        }
    }
    let last_edit_in = |root: Option<&Path>| -> Option<&Edit> {
        if !kind.edits_matter() {
            return None;
        }
        f.edits
            .iter()
            .filter(|ed| c.after(&ed.ts, ed.ordinal, &ed.sub))
            .filter(|ed| relevant(kind, ed, root, inp.notes_roots))
            .max_by(|a, b| {
                a.ts.cmp(&b.ts)
                    .then((a.ordinal, a.pos).cmp(&(b.ordinal, b.pos)))
            })
    };
    let mut fresh: Vec<&Exec> = Vec::new();
    let mut stale: Option<(&Exec, &Edit)> = None;
    for e in &matching {
        let root = e
            .shows(kind)
            .and_then(|r| r.dir.as_deref())
            .map(|d| repo_root(d, roots));
        match last_edit_in(root.as_deref()) {
            Some(ed) if ed.at().after(&e.at(kind)) => stale = Some((e, ed)),
            _ => fresh.push(e),
        }
    }
    let shown = |e: &Exec| e.shows(kind).map_or(e.command.clone(), |r| r.shown.clone());
    let want = (kind == ClaimKind::Tests)
        .then(|| claimed_count(c.text))
        .flatten();
    let passed: Vec<&&Exec> = fresh
        .iter()
        .filter(|e| e.result(kind) == Outcome::Succeeded)
        .collect();
    let counted = |e: &Exec| -> Option<u64> {
        e.count
            .as_deref()
            .and_then(|s| s.split_whitespace().next())
            .and_then(|n| n.parse().ok())
    };
    let best = match want {
        Some(n) => passed
            .iter()
            .rev()
            .find(|e| counted(e) == Some(n))
            .or_else(|| passed.last()),
        None => passed.last(),
    };
    if let Some(e) = best {
        let mut parts = vec![shown(e), "exit 0".to_owned()];
        if let Some(n) = e.count.as_ref().filter(|_| kind == ClaimKind::Tests) {
            parts.push(n.clone());
        }
        parts.push(at_turn(e.ordinal, &e.sub));
        match (want, counted(e)) {
            (Some(n), Some(got)) if n != got => {
                return (
                    Status::Unclear,
                    format!(
                        "{}; the claim says {n}, this run shows {got}",
                        parts.join(" · ")
                    ),
                );
            }
            // A count the output does not show cannot be checked.
            (Some(n), None) => {
                return (
                    Status::Unclear,
                    format!(
                        "{}; the claim says {n}, this run shows no count",
                        parts.join(" · ")
                    ),
                );
            }
            _ => {}
        }
        return (Status::Verified, parts.join(" · "));
    }
    let last_edit = last_edit_in(None);
    let last_edit_ts = last_edit.map_or("", |e| e.ts.as_str());
    // CI on the pushed head: a push after the last
    // relevant edit, then a CI read that shows the kind passing.
    if is_check(kind) && kind != ClaimKind::BrainCheck {
        let pushes: Vec<&Exec> = f
            .execs
            .iter()
            .filter(|e| {
                c.after(&e.ts, e.ordinal, &e.sub)
                    && e.result(ClaimKind::Pushed) == Outcome::Succeeded
                    && e.shows(ClaimKind::Pushed).is_some()
                    && last_edit.is_none_or(|ed| e.at(ClaimKind::Pushed).after(&ed.at()))
            })
            .collect();
        if let Some(push) = pushes.first() {
            let push_at = push.at(ClaimKind::Pushed);
            if let Some(read) = f.execs.iter().rev().find(|e| {
                c.after(&e.ts, e.ordinal, &e.sub)
                    && e.shows(ClaimKind::Ci).is_some()
                    // A call that ended in an error may not have shown
                    // this repository's checks (`gh pr checks` ends 8 while
                    // checks are pending, and its rows still count).
                    && !matches!(e.result(ClaimKind::Ci), Outcome::Failed(Some(code)) if code != 8)
                    && e.at(ClaimKind::Ci).after(&push_at)
                    && e.output.as_deref().is_some_and(|o| ci_shows(kind, o))
            }) {
                return (
                    Status::Verified,
                    format!(
                        "CI passed on the pushed head: {} · {}",
                        shown_ci(read),
                        at_turn(read.ordinal, &read.sub)
                    ),
                );
            }
        }
    }
    if let Some(e) = fresh
        .iter()
        .rev()
        .find(|e| e.result(kind) == Outcome::Unknown)
    {
        return (
            Status::Unclear,
            format!(
                "{} ran at {}; its result is not in the transcript",
                shown(e),
                at_turn(e.ordinal, &e.sub)
            ),
        );
    }
    let failed = fresh
        .iter()
        .rev()
        .find(|e| matches!(e.result(kind), Outcome::Failed(_)));
    // What the transcript holds but hippocamp cannot match: after the last
    // edit anywhere, a run's output that shows the pass (it verifies), a
    // delegate's report, a log that shows the pass, or a script whose
    // commands are not seen; for a deploy, a check of the live address.
    // After a failure, only what came later can settle it.
    let floor = failed.map_or("", |e| e.ts.as_str());
    for s in f.seen.iter().rev() {
        if !c.after(&s.ts, s.ordinal, &s.sub)
            || (kind.edits_matter() && s.ts.as_str() < last_edit_ts)
            || s.ts.as_str() <= floor
        {
            continue;
        }
        let place = at_turn(s.ordinal, &s.sub);
        match &s.hint {
            Hint::Report(text)
                if PATTERNS
                    .iter()
                    .any(|(k, re)| *k == kind && re.is_match(text)) =>
            {
                return (
                    Status::Unclear,
                    format!("a delegate reported it at {place}; hippocamp saw no command"),
                );
            }
            Hint::Output(text, from_run) => {
                if let Some(m) = marker(kind).and_then(|re| re.find(text)) {
                    // Output that also shows a failure of the kind settles
                    // nothing (a failed run's own result).
                    if shows_failure(&[kind], text) {
                        continue;
                    }
                    // A wrapper, a remote run or a background task whose
                    // output shows the check passed.
                    let count_ok = want.is_none_or(|n| {
                        PASSED
                            .captures_iter(text)
                            .any(|c| c[1].replace(',', "").parse::<u64>().ok() == Some(n))
                    });
                    if *from_run && count_ok {
                        return (
                            Status::Verified,
                            format!("output at {place} shows \"{}\"", one_line(m.as_str(), 60)),
                        );
                    }
                    return (
                        Status::Unclear,
                        format!(
                            "output at {place} shows \"{}\"; hippocamp did not see the command run",
                            one_line(m.as_str(), 60)
                        ),
                    );
                }
            }
            Hint::Script(cmd) if kind != ClaimKind::Deployed => {
                return (
                    Status::Unclear,
                    format!(
                        "{cmd} ran at {place}; it may hold the {} command",
                        kind.command_word()
                    ),
                );
            }
            Hint::Fetch(url) if kind == ClaimKind::Deployed => {
                return (
                    Status::Unclear,
                    format!(
                        "no deploy command; {} was fetched at {place}, which may show it live",
                        one_line(url, 60)
                    ),
                );
            }
            _ => {}
        }
    }
    if let Some(e) = failed {
        let code = match e.result(kind) {
            Outcome::Failed(Some(n)) => format!(" (exit {n})"),
            _ => String::new(),
        };
        return (
            Status::Unverified,
            format!(
                "{} failed{code} at {}",
                shown(e),
                at_turn(e.ordinal, &e.sub)
            ),
        );
    }
    if let Some((e, ed)) = stale {
        return (
            Status::Unverified,
            format!(
                "{} ran at {}, before the last edit (turn {}, {})",
                shown(e),
                at_turn(e.ordinal, &e.sub),
                ed.ordinal,
                ed.path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
        );
    }
    if kind == ClaimKind::Ci {
        if let Some(e) = f.execs.iter().rev().find(|e| {
            c.after(&e.ts, e.ordinal, &e.sub)
                && e.runs.iter().any(|r| {
                    matches!(r.kind, ClaimKind::Tests | ClaimKind::Lint)
                        && r.result == Outcome::Succeeded
                })
        }) {
            return (
                Status::Unclear,
                format!(
                    "no CI check command; local checks passed ({} at {})",
                    e.runs[0].shown,
                    at_turn(e.ordinal, &e.sub)
                ),
            );
        }
    }
    let reason = if kind == ClaimKind::Deployed {
        "no deploy command in the transcript; a deploy command and its success output would count"
            .to_owned()
    } else {
        format!("no {} command in the session", kind.command_word())
    };
    (Status::Unverified, reason)
}

/// The CI command a read ran.
fn shown_ci(e: &Exec) -> String {
    e.shows(ClaimKind::Ci)
        .map_or(e.command.clone(), |r| r.shown.clone())
}

fn short(id: &str) -> String {
    id.rsplit("-agent-")
        .next()
        .unwrap_or(id)
        .chars()
        .take(8)
        .collect()
}

fn one_line(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let mut s: String = one.chars().take(max - 1).collect();
    s.push('…');
    s
}

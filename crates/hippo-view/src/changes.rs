//! What a session changed in files, read from its tool calls alone: each
//! `Edit`, `MultiEdit` or `Write` call (and Codex's `apply_patch`) as a hunk
//! of removed and added lines, and per file the hunks and their totals.
//! Nothing runs git or reads the files themselves: line numbers are known
//! only when the session itself read the whole file (`Read` without an
//! offset or limit) or wrote it, and each later edit is applied to that
//! text.

use std::collections::HashMap;

use crate::conversation::{Entry, Kind};

/// Lines of context kept around an edit when the file's text is known.
pub const CONTEXT: usize = 2;
/// Above this many cells (old lines × new lines), a diff is all removed then
/// all added instead of a line-by-line match.
const LCS_CELLS: usize = 250_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// The line's number in the file after the edit (for a removed line,
    /// before it), when the file's text is known.
    pub number: Option<usize>,
    pub text: String,
}

/// One call's change to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The entry of the call.
    pub entry: usize,
    pub lines: Vec<DiffLine>,
    /// The call wrote the whole file.
    pub whole: bool,
}

impl Hunk {
    pub fn added(&self) -> usize {
        self.count(LineKind::Added)
    }
    pub fn removed(&self) -> usize {
        self.count(LineKind::Removed)
    }
    fn count(&self, kind: LineKind) -> usize {
        self.lines.iter().filter(|l| l.kind == kind).count()
    }
}

/// Every change a session made to one file, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub hunks: Vec<Hunk>,
    pub added: usize,
    pub removed: usize,
}

impl FileChange {
    /// The file's name and the folder it is in, as the pane's header shows
    /// them: `("lib.rs", "crates/sum_tree/src/")`; the folder keeps its
    /// last three parts.
    pub fn name_and_dir(&self) -> (String, String) {
        let path = self.path.trim_end_matches('/');
        match path.rsplit_once('/') {
            Some((dir, name)) => {
                let parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
                let keep = &parts[parts.len().saturating_sub(3)..];
                let lead = if parts.len() > 3 { "…/" } else { "" };
                (name.to_string(), format!("{lead}{}/", keep.join("/")))
            }
            None => (path.to_string(), String::new()),
        }
    }
}

/// A session's changes: per file, first changed first, with the totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub files: Vec<FileChange>,
    pub added: usize,
    pub removed: usize,
    /// Each editing entry's hunks, by entry index, for the conversation's
    /// inline diffs: `(path, hunk)`.
    pub by_entry: HashMap<usize, Vec<(String, Hunk)>>,
    /// Editing entries whose call failed: their change was not made.
    pub failed: Vec<usize>,
}

/// Whether an entry is a call that edits or writes files.
pub fn is_edit(entry: &Entry) -> bool {
    matches!(&entry.kind, Kind::Tool { name, input: Some(_), .. } if is_edit_tool(name))
}

fn is_edit_tool(name: &str) -> bool {
    matches!(name, "Edit" | "MultiEdit" | "Write" | "apply_patch")
}

/// Reads a session's [`Changes`] from its entries.
pub fn changes(entries: &[Entry]) -> Changes {
    let mut out = Changes::default();
    // Files whose whole text the session has seen, as lines.
    let mut known: HashMap<String, Vec<String>> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        let Kind::Tool {
            name,
            input: Some(input),
            result,
            ..
        } = &entry.kind
        else {
            continue;
        };
        let value: serde_json::Value = match serde_json::from_str(input) {
            Ok(serde_json::Value::String(s)) => {
                serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s))
            }
            Ok(v) => v,
            Err(_) => serde_json::Value::String(input.clone()),
        };
        if name == "Read" {
            note_read(&value, result.as_deref(), &mut known);
            continue;
        }
        if !is_edit_tool(name) {
            continue;
        }
        if result.as_deref().is_some_and(failed) {
            out.failed.push(i);
            continue;
        }
        let hunks = match name.as_str() {
            "Edit" => edit(&value, &mut known),
            "MultiEdit" => {
                let path = str_field(&value, "file_path");
                value
                    .get("edits")
                    .and_then(|e| e.as_array())
                    .into_iter()
                    .flatten()
                    .flat_map(|e| {
                        let mut one = e.clone();
                        if let (Some(obj), Some(p)) = (one.as_object_mut(), &path) {
                            obj.insert("file_path".into(), p.clone().into());
                        }
                        edit(&one, &mut known)
                    })
                    .collect()
            }
            "Write" => write(&value, &mut known),
            _ => patch(&value),
        };
        let hunks: Vec<(String, Hunk)> = hunks
            .into_iter()
            .map(|(path, lines, whole)| {
                (
                    path,
                    Hunk {
                        entry: i,
                        lines,
                        whole,
                    },
                )
            })
            .collect();
        for (path, hunk) in &hunks {
            let file = match out.files.iter_mut().position(|f| f.path == *path) {
                Some(at) => &mut out.files[at],
                None => {
                    out.files.push(FileChange {
                        path: path.clone(),
                        hunks: Vec::new(),
                        added: 0,
                        removed: 0,
                    });
                    out.files.last_mut().expect("just pushed")
                }
            };
            file.added += hunk.added();
            file.removed += hunk.removed();
            out.added += hunk.added();
            out.removed += hunk.removed();
            file.hunks.push(hunk.clone());
        }
        if !hunks.is_empty() {
            out.by_entry.insert(i, hunks);
        }
    }
    out
}

/// Whether a call's result says it failed, so its change was not made.
pub fn failed(result: &str) -> bool {
    let t = result.trim_start();
    t.starts_with("<tool_use_error>") || t.starts_with("Error:") || t.starts_with("Error editing")
}

fn str_field(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|s| s.as_str()).map(str::to_string)
}

/// A whole-file `Read`: its `N\ttext` lines from 1 on, with no offset or
/// limit, become the file's known text.
fn note_read(
    input: &serde_json::Value,
    result: Option<&str>,
    known: &mut HashMap<String, Vec<String>>,
) {
    let Some(path) = str_field(input, "file_path") else {
        return;
    };
    if input.get("offset").is_some() || input.get("limit").is_some() {
        return;
    }
    let Some(result) = result else { return };
    let mut lines = Vec::new();
    for (n, line) in result.lines().enumerate() {
        let Some((num, text)) = line.trim_start().split_once('\t') else {
            break;
        };
        if num.trim().parse::<usize>().ok() != Some(n + 1) {
            break;
        }
        lines.push(text.to_string());
    }
    if lines.is_empty() {
        known.remove(&path);
    } else {
        known.insert(path, lines);
    }
}

type Made = (String, Vec<DiffLine>, bool);

/// An `Edit`: its old and new strings, numbered and with context when the
/// file's text is known (and then applied to it).
fn edit(v: &serde_json::Value, known: &mut HashMap<String, Vec<String>>) -> Vec<Made> {
    let (Some(path), Some(old), Some(new)) = (
        str_field(v, "file_path"),
        str_field(v, "old_string"),
        str_field(v, "new_string"),
    ) else {
        return Vec::new();
    };
    let all = v.get("replace_all").and_then(|b| b.as_bool()) == Some(true);
    if let Some(lines) = known.get(&path) {
        let text = lines.join("\n");
        if let (Some(at), false) = (text.find(&old), old.is_empty()) {
            // The whole lines the old string touches.
            let start = text[..at].rfind('\n').map_or(0, |n| n + 1);
            let end = text[at + old.len()..]
                .find('\n')
                .map_or(text.len(), |n| at + old.len() + n);
            let before = &text[start..end];
            let after = format!("{}{}{}", &text[start..at], new, &text[at + old.len()..end]);
            let first = text[..start].matches('\n').count(); // 0-based line
            let mut out = Vec::new();
            for (k, l) in lines
                .iter()
                .enumerate()
                .skip(first.saturating_sub(CONTEXT))
                .take(first.min(CONTEXT))
            {
                out.push(context(l, Some(k + 1)));
            }
            let mut new_no = first + 1;
            let mut old_no = first + 1;
            for (kind, l) in diff_lines(before, &after) {
                let number = match kind {
                    LineKind::Removed => {
                        old_no += 1;
                        Some(old_no - 1)
                    }
                    _ => {
                        new_no += 1;
                        if kind == LineKind::Context {
                            old_no += 1;
                        }
                        Some(new_no - 1)
                    }
                };
                out.push(DiffLine {
                    kind,
                    number,
                    text: l,
                });
            }
            let updated = if all {
                text.replace(&old, &new)
            } else {
                text.replacen(&old, &new, 1)
            };
            let new_lines: Vec<String> = updated.split('\n').map(str::to_string).collect();
            let resume = new_no - 1; // 0-based index of the line after the edit
            for (k, l) in new_lines.iter().enumerate().skip(resume).take(CONTEXT) {
                out.push(context(l, Some(k + 1)));
            }
            known.insert(path.clone(), new_lines);
            return vec![(path, out, false)];
        }
        // The file changed in a way the transcript does not show.
        known.remove(&path);
    }
    let lines = diff_lines(&old, &new)
        .into_iter()
        .map(|(kind, text)| DiffLine {
            kind,
            number: None,
            text,
        })
        .collect();
    vec![(path, lines, false)]
}

fn context(text: &str, number: Option<usize>) -> DiffLine {
    DiffLine {
        kind: LineKind::Context,
        number,
        text: text.to_string(),
    }
}

/// A `Write`: against the file's known text when there is one, else every
/// line added.
fn write(v: &serde_json::Value, known: &mut HashMap<String, Vec<String>>) -> Vec<Made> {
    let (Some(path), Some(content)) = (str_field(v, "file_path"), str_field(v, "content")) else {
        return Vec::new();
    };
    let lines = match known.get(&path) {
        Some(before) => {
            let mut new_no = 0;
            let mut old_no = 0;
            diff_lines(&before.join("\n"), &content)
                .into_iter()
                .map(|(kind, text)| {
                    let number = if kind == LineKind::Removed {
                        old_no += 1;
                        old_no
                    } else {
                        new_no += 1;
                        if kind == LineKind::Context {
                            old_no += 1;
                        }
                        new_no
                    };
                    DiffLine {
                        kind,
                        number: Some(number),
                        text,
                    }
                })
                .collect()
        }
        None => content
            .lines()
            .enumerate()
            .map(|(k, l)| DiffLine {
                kind: LineKind::Added,
                number: Some(k + 1),
                text: l.to_string(),
            })
            .collect(),
    };
    known.insert(
        path.clone(),
        content.split('\n').map(str::to_string).collect(),
    );
    vec![(path, lines, true)]
}

/// Codex's `apply_patch`: `*** Update File:` and `*** Add File:` sections,
/// their lines marked ` `, `-` or `+`; no line numbers.
fn patch(v: &serde_json::Value) -> Vec<Made> {
    let text = match v {
        serde_json::Value::String(s) => s.clone(),
        v => v
            .get("input")
            .or_else(|| v.get("patch"))
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
    };
    let mut out: Vec<Made> = Vec::new();
    for line in text.lines() {
        if let Some(p) = line
            .strip_prefix("*** Update File: ")
            .or_else(|| line.strip_prefix("*** Add File: "))
        {
            out.push((p.trim().to_string(), Vec::new(), line.contains("Add File")));
            continue;
        }
        if line.starts_with("*** ") || line.starts_with("@@") {
            continue;
        }
        let Some((_, lines, _)) = out.last_mut() else {
            continue;
        };
        let (kind, rest) = match line.chars().next() {
            Some('+') => (LineKind::Added, &line[1..]),
            Some('-') => (LineKind::Removed, &line[1..]),
            Some(' ') => (LineKind::Context, &line[1..]),
            _ => continue,
        };
        lines.push(DiffLine {
            kind,
            number: None,
            text: rest.to_string(),
        });
    }
    out
}

/// A line diff of `a` to `b`: common prefix and suffix as context, the
/// middle matched line by line (longest common subsequence) when small
/// enough, else all removed then all added.
pub fn diff_lines(a: &str, b: &str) -> Vec<(LineKind, String)> {
    let a: Vec<&str> = if a.is_empty() {
        vec![]
    } else {
        a.split('\n').collect()
    };
    let b: Vec<&str> = if b.is_empty() {
        vec![]
    } else {
        b.split('\n').collect()
    };
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut out: Vec<(LineKind, String)> = a[..pre]
        .iter()
        .map(|l| (LineKind::Context, l.to_string()))
        .collect();
    if am.len() * bm.len() > LCS_CELLS {
        out.extend(am.iter().map(|l| (LineKind::Removed, l.to_string())));
        out.extend(bm.iter().map(|l| (LineKind::Added, l.to_string())));
    } else {
        // lcs[i][j]: the longest common subsequence of am[i..] and bm[j..].
        let (n, m) = (am.len(), bm.len());
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * (m + 1) + j] = if am[i] == bm[j] {
                    lcs[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                out.push((LineKind::Context, am[i].to_string()));
                i += 1;
                j += 1;
            } else if j < m && (i == n || lcs[i * (m + 1) + j + 1] >= lcs[(i + 1) * (m + 1) + j]) {
                out.push((LineKind::Added, bm[j].to_string()));
                j += 1;
            } else {
                out.push((LineKind::Removed, am[i].to_string()));
                i += 1;
            }
        }
        // Removed lines read better before the added ones they replace.
        let mut k = 0;
        while k < out.len() {
            if out[k].0 == LineKind::Context {
                k += 1;
                continue;
            }
            let end = out[k..]
                .iter()
                .position(|l| l.0 == LineKind::Context)
                .map_or(out.len(), |p| k + p);
            out[k..end].sort_by_key(|l| l.0 != LineKind::Removed);
            k = end;
        }
    }
    out.extend(
        a[a.len() - suf..]
            .iter()
            .map(|l| (LineKind::Context, l.to_string())),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::entries;
    use hippo_core::index::StoredTurn;

    fn turn(n: u32, tool: &str, text: &str) -> StoredTurn {
        StoredTurn {
            ordinal: n,
            role: "tool".into(),
            ts: format!("2026-11-08T10:00:{n:02}Z"),
            tool_name: Some(tool.into()),
            text: text.into(),
        }
    }

    fn kinds(lines: &[DiffLine]) -> String {
        lines
            .iter()
            .map(|l| {
                let mark = match l.kind {
                    LineKind::Context => ' ',
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                };
                match l.number {
                    Some(n) => format!("{mark}{n} {}", l.text),
                    None => format!("{mark} {}", l.text),
                }
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn a_line_diff_keeps_what_did_not_change() {
        assert_eq!(
            diff_lines("a\nb\nc\nd", "a\nB\nc\nd\ne")
                .iter()
                .map(|(k, t)| format!("{k:?}:{t}"))
                .collect::<Vec<_>>(),
            [
                "Context:a",
                "Removed:b",
                "Added:B",
                "Context:c",
                "Context:d",
                "Added:e"
            ]
        );
        assert!(diff_lines("", "").is_empty());
    }

    #[test]
    fn an_edit_is_numbered_when_the_session_read_the_whole_file() {
        let turns = vec![
            turn(0, "Read", r#"Read {"file_path":"/w/src/lib.rs"}"#),
            turn(1, "Read", "1\tmod a;\n2\tmod b;\n3\tmod c;\n4\tpub use a::A;\n5\tpub use b::B;\n6\tfn main() {}"),
            turn(
                2,
                "Edit",
                r#"Edit {"file_path":"/w/src/lib.rs","old_string":"mod c;\npub use a::A;","new_string":"mod c;\nmod d;\npub use a::A;"}"#,
            ),
            turn(3, "Edit", "The file /w/src/lib.rs has been updated successfully."),
            // A second edit lands on the text the first one left.
            turn(
                4,
                "Edit",
                r#"Edit {"file_path":"/w/src/lib.rs","old_string":"fn main() {}","new_string":"fn main() { run() }"}"#,
            ),
            turn(5, "Edit", "The file /w/src/lib.rs has been updated successfully."),
            // An edit that failed changes nothing.
            turn(
                6,
                "Edit",
                r#"Edit {"file_path":"/w/src/lib.rs","old_string":"zzz","new_string":"y"}"#,
            ),
            turn(7, "Edit", "<tool_use_error>String to replace not found</tool_use_error>"),
        ];
        let e = entries(&turns);
        let c = changes(&e);
        assert_eq!(c.files.len(), 1);
        let f = &c.files[0];
        assert_eq!((f.added, f.removed), (2, 1));
        assert_eq!((c.added, c.removed), (2, 1));
        assert_eq!(
            kinds(&f.hunks[0].lines),
            " 1 mod a;| 2 mod b;| 3 mod c;|+4 mod d;| 5 pub use a::A;| 6 pub use b::B;| 7 fn main() {}"
        );
        assert_eq!(
            kinds(&f.hunks[1].lines),
            " 5 pub use a::A;| 6 pub use b::B;|-7 fn main() {}|+7 fn main() { run() }"
        );
        assert_eq!(c.failed, vec![3]);
        assert_eq!(
            f.name_and_dir(),
            ("lib.rs".to_string(), "w/src/".to_string())
        );
    }

    #[test]
    fn without_the_files_text_an_edit_is_its_strings_and_a_write_is_all_added() {
        let turns = vec![
            turn(
                0,
                "Edit",
                r#"Edit {"file_path":"/a/b/c/d/e.rs","old_string":"x\ny","new_string":"x\nz"}"#,
            ),
            turn(
                1,
                "Write",
                r#"Write {"file_path":"/n.md","content":"one\ntwo"}"#,
            ),
            turn(
                2,
                "MultiEdit",
                r#"MultiEdit {"file_path":"/n.md","edits":[{"old_string":"two","new_string":"2"}]}"#,
            ),
        ];
        let c = changes(&entries(&turns));
        assert_eq!(kinds(&c.files[0].hunks[0].lines), "  x|- y|+ z");
        assert_eq!(
            c.files[0].name_and_dir(),
            ("e.rs".to_string(), "…/b/c/d/".to_string())
        );
        assert_eq!(kinds(&c.files[1].hunks[0].lines), "+1 one|+2 two");
        assert!(c.files[1].hunks[0].whole);
        // The write made the text known, so the edit after it is numbered.
        assert_eq!(kinds(&c.files[1].hunks[1].lines), " 1 one|-2 two|+2 2");
        assert_eq!((c.added, c.removed), (4, 2));
        assert_eq!(c.by_entry.len(), 3);
    }

    #[test]
    fn a_codex_patch_reads_as_its_sections() {
        let turns = vec![turn(
            0,
            "apply_patch",
            "apply_patch {\"input\":\"*** Begin Patch\\n*** Update File: src/a.rs\\n@@\\n fn a() {\\n-    1\\n+    2\\n }\\n*** Add File: src/b.rs\\n+new\\n*** End Patch\"}",
        )];
        let c = changes(&entries(&turns));
        assert_eq!(c.files.len(), 2);
        assert_eq!(
            kinds(&c.files[0].hunks[0].lines),
            "  fn a() {|-     1|+     2|  }"
        );
        assert_eq!((c.added, c.removed), (2, 1));
    }
}

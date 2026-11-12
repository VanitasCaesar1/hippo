//! What a change changed, word by word: the words it removed and the words
//! it put in their place, with a little shared text on each side. Home's
//! Review rows show this instead of the start of the old and new text,
//! which often read the same.

use std::ops::Range;

/// Products of word counts above this are not aligned word by word: the
/// whole differing middle shows as one replacement.
const MAX_CELLS: usize = 250_000;
/// Shared runs this short between two changes join them into one, so that
/// `no hippocamp init here` → `hippocamp init done` reads as one replacement, not
/// three scraps.
const JOIN_WORDS: usize = 2;

/// A stretch of the text: shared by both sides, or a change.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Run {
    Same(Vec<String>),
    Change { old: Vec<String>, new: Vec<String> },
}

/// The word-level difference between two texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordDiff {
    runs: Vec<Run>,
}

/// A piece of an excerpt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Text both sides share.
    Same(String),
    /// Words the change removed.
    Old(String),
    /// Words the change put in.
    New(String),
    /// Text left out: an ellipsis.
    Cut,
}

/// How a span of [`layout`]'s text is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Removed: struck through.
    Old,
    /// Put in.
    New,
}

impl WordDiff {
    /// Compares `old` and `new` word by word (split at whitespace).
    pub fn new(old: &str, new: &str) -> WordDiff {
        let a: Vec<&str> = old.split_whitespace().collect();
        let b: Vec<&str> = new.split_whitespace().collect();
        let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
        let suf = a[pre..]
            .iter()
            .rev()
            .zip(b[pre..].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
        let mut ops: Vec<(u8, &str)> = Vec::new(); // 0 same, 1 old, 2 new
        ops.extend(a[..pre].iter().map(|w| (0, *w)));
        if ma.len().saturating_mul(mb.len()) <= MAX_CELLS {
            ops.extend(align(ma, mb));
        } else {
            ops.extend(ma.iter().map(|w| (1, *w)));
            ops.extend(mb.iter().map(|w| (2, *w)));
        }
        ops.extend(a[a.len() - suf..].iter().map(|w| (0, *w)));

        // Group into runs: shared words, and changes (removed then added).
        let mut runs: Vec<Run> = Vec::new();
        for (kind, w) in ops {
            let w = w.to_string();
            match (kind, runs.last_mut()) {
                (0, Some(Run::Same(ws))) => ws.push(w),
                (0, _) => runs.push(Run::Same(vec![w])),
                (1, Some(Run::Change { old, .. })) => old.push(w),
                (2, Some(Run::Change { new, .. })) => new.push(w),
                (1, _) => runs.push(Run::Change {
                    old: vec![w],
                    new: Vec::new(),
                }),
                _ => runs.push(Run::Change {
                    old: Vec::new(),
                    new: vec![w],
                }),
            }
        }
        // A short shared run between two changes joins them.
        let mut joined: Vec<Run> = Vec::new();
        let mut i = 0;
        while i < runs.len() {
            let short = matches!(&runs[i], Run::Same(ws) if ws.len() <= JOIN_WORDS);
            let between = i > 0 && i + 1 < runs.len();
            if short && between {
                let (Run::Same(ws), Some(Run::Change { old, new })) = (&runs[i], joined.last_mut())
                else {
                    unreachable!("runs alternate");
                };
                old.extend(ws.iter().cloned());
                new.extend(ws.iter().cloned());
                if let Run::Change { old: o, new: n } = &runs[i + 1] {
                    old.extend(o.iter().cloned());
                    new.extend(n.iter().cloned());
                }
                i += 2;
            } else {
                joined.push(runs[i].clone());
                i += 1;
            }
        }
        WordDiff { runs: joined }
    }

    /// Whether any word changed.
    pub fn changed(&self) -> bool {
        self.runs.iter().any(|r| matches!(r, Run::Change { .. }))
    }

    /// The changes with `context` shared words on each side, at most
    /// `changes` of them, each side cut to `words` words. Text left out is a
    /// [`Piece::Cut`]. Empty when no word changed.
    pub fn excerpt(&self, context: usize, changes: usize, words: usize) -> Vec<Piece> {
        let mut out = Vec::new();
        let Some(first) = self
            .runs
            .iter()
            .position(|r| matches!(r, Run::Change { .. }))
        else {
            return out;
        };
        let changes = changes.max(1);
        if let Some(Run::Same(ws)) = first.checked_sub(1).map(|i| &self.runs[i]) {
            if ws.len() > context {
                out.push(Piece::Cut);
            }
            push_same(&mut out, &ws[ws.len().saturating_sub(context)..]);
        }
        let mut shown = 0;
        for (i, run) in self.runs.iter().enumerate().skip(first) {
            match run {
                Run::Change { old, new } => {
                    if !old.is_empty() {
                        out.push(Piece::Old(cut_words(old, words)));
                    }
                    if !new.is_empty() {
                        out.push(Piece::New(cut_words(new, words)));
                    }
                    shown += 1;
                }
                Run::Same(ws) => {
                    let more = i + 1 < self.runs.len();
                    if shown < changes && more && ws.len() > 2 * context + 1 {
                        // Between two changes that both show.
                        push_same(&mut out, &ws[..context]);
                        out.push(Piece::Cut);
                        push_same(&mut out, &ws[ws.len() - context..]);
                    } else if shown < changes && more {
                        push_same(&mut out, ws);
                    } else {
                        push_same(&mut out, &ws[..ws.len().min(context)]);
                        if ws.len() > context || more {
                            out.push(Piece::Cut);
                        }
                        break;
                    }
                }
            }
        }
        out
    }
}

/// Words that both sides share, unless there are none.
fn push_same(out: &mut Vec<Piece>, ws: &[String]) {
    if !ws.is_empty() {
        out.push(Piece::Same(ws.join(" ")));
    }
}

/// The first `n` words (at least one), with an ellipsis when cut.
fn cut_words(ws: &[String], n: usize) -> String {
    let n = n.max(1);
    if ws.len() <= n {
        return ws.join(" ");
    }
    format!("{}…", ws[..n].join(" "))
}

/// A longest common subsequence alignment of two word lists, as same (0),
/// removed (1) and added (2) words; a change's removed words come first.
fn align<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(u8, &'a str)> {
    let (n, m) = (a.len(), b.len());
    // lcs[i][j]: the common length of a[i..] and b[j..].
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[at(i, j)] = if a[i] == b[j] {
                lcs[at(i + 1, j + 1)] + 1
            } else {
                lcs[at(i + 1, j)].max(lcs[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(n + m);
    let mut added = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.append(&mut added);
            out.push((0, a[i]));
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[at(i, j + 1)] >= lcs[at(i + 1, j)]) {
            added.push((2, b[j]));
            j += 1;
        } else {
            out.push((1, a[i]));
            i += 1;
        }
    }
    out.append(&mut added);
    out
}

/// An excerpt as one line of text, and the spans to strike (removed words)
/// or mark (words put in). A removal followed by its replacement reads
/// `old → new`.
pub fn layout(pieces: &[Piece]) -> (String, Vec<(Range<usize>, Mark)>) {
    let mut text = String::new();
    let mut marks = Vec::new();
    let mut prev: Option<&Piece> = None;
    for (i, p) in pieces.iter().enumerate() {
        let last = i + 1 == pieces.len();
        // An ellipsis sits against the text at either end and stands
        // apart in the middle; a side already cut short needs no second.
        let sep = match (prev, p) {
            (None, _) => "",
            (Some(_), Piece::Cut) if text.ends_with('…') => continue,
            (Some(_), Piece::Cut) if last => "",
            (Some(Piece::Cut), _) if i == 1 => "",
            (Some(Piece::Old(_)), Piece::New(_)) => " → ",
            _ => " ",
        };
        text.push_str(sep);
        match p {
            Piece::Same(s) => text.push_str(s),
            Piece::Cut => text.push('…'),
            Piece::Old(s) | Piece::New(s) => {
                let start = text.len();
                text.push_str(s);
                let mark = if matches!(p, Piece::Old(_)) {
                    Mark::Old
                } else {
                    Mark::New
                };
                marks.push((start..text.len(), mark));
            }
        }
        prev = Some(p);
    }
    (text, marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(old: &str, new: &str, context: usize, changes: usize, words: usize) -> String {
        let (text, marks) = layout(&WordDiff::new(old, new).excerpt(context, changes, words));
        // Mark the spans inline: [-removed-] and {+added+}.
        let mut out = String::new();
        let mut at = 0;
        for (r, m) in marks {
            out.push_str(&text[at..r.start]);
            let (open, close) = match m {
                Mark::Old => ("[-", "-]"),
                Mark::New => ("{+", "+}"),
            };
            out.push_str(open);
            out.push_str(&text[r.clone()]);
            out.push_str(close);
            at = r.end;
        }
        out.push_str(&text[at..]);
        out
    }

    #[test]
    fn a_replacement_shows_with_shared_words_around_it() {
        let old = "The garden plan was drafted in spring. The cutover started: no seeds planted here. Later on the beds were weeded twice.";
        let new = "The garden plan was drafted in spring. The cutover started: seeds planted on 8 May. Later on the beds were weeded twice.";
        assert_eq!(
            shown(old, new, 2, 3, 20),
            "…cutover started: [-no seeds planted here.-] → {+seeds planted on 8 May.+} Later on…"
        );
    }

    #[test]
    fn an_insertion_and_a_removal_show_alone() {
        assert_eq!(
            shown(
                "alpha beta gamma delta",
                "alpha beta new gamma delta",
                1,
                3,
                9
            ),
            "…beta {+new+} gamma…"
        );
        assert_eq!(
            shown("alpha beta gamma delta", "alpha gamma delta", 1, 3, 9),
            "alpha [-beta-] gamma…"
        );
    }

    #[test]
    fn a_change_at_either_end_has_no_ellipsis_there() {
        assert_eq!(
            shown("one two three", "uno two three", 1, 3, 9),
            "[-one-] → {+uno+} two…"
        );
        assert_eq!(
            shown("one two three", "one two drei", 1, 3, 9),
            "…two [-three-] → {+drei+}"
        );
        assert_eq!(shown("short", "longer", 3, 3, 9), "[-short-] → {+longer+}");
    }

    #[test]
    fn far_apart_changes_keep_context_and_cut_between() {
        let old = "a b c d e f g h i j k l";
        let new = "A b c d e f g h i j k L";
        assert_eq!(
            shown(old, new, 1, 3, 9),
            "[-a-] → {+A+} b … k [-l-] → {+L+}"
        );
        // Only the first change when asked for one.
        assert_eq!(shown(old, new, 1, 1, 9), "[-a-] → {+A+} b…");
    }

    #[test]
    fn changes_a_word_or_two_apart_read_as_one() {
        assert_eq!(
            shown("x no keep it here y", "x keep it done now y", 1, 3, 9),
            "x [-no keep it here-] → {+keep it done now+} y"
        );
    }

    #[test]
    fn long_sides_are_cut_to_the_word_limit() {
        assert_eq!(
            shown(
                "s one two three four e",
                "s five six seven eight e",
                0,
                1,
                2
            ),
            "…[-one two…-] → {+five six…+}"
        );
    }

    #[test]
    fn identical_words_have_no_excerpt() {
        let d = WordDiff::new("same  words\nhere", "same words here");
        assert!(!d.changed());
        assert!(d.excerpt(2, 2, 9).is_empty());
    }

    #[test]
    fn very_long_texts_fall_back_to_one_replacement() {
        let old: Vec<String> = (0..600).map(|i| format!("o{i}")).collect();
        let new: Vec<String> = (0..600).map(|i| format!("n{i}")).collect();
        let (old, new) = (
            format!("start {} end", old.join(" ")),
            format!("start {} end", new.join(" ")),
        );
        assert_eq!(
            shown(&old, &new, 1, 2, 2),
            "start [-o0 o1…-] → {+n0 n1…+} end"
        );
    }
}

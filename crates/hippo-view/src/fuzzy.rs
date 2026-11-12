//! The finder's matcher: a query is a case-insensitive subsequence of a row.
//!
//! Each matched character scores 16 at a word start (the row's first
//! character, one after `/`, a space, `-`, `_` or `.`, or an upper-case letter
//! after a lower-case one), 8 when it directly follows the previous matched
//! character, and 1 otherwise. Each character skipped before the first match
//! costs 1. A row's score is that of its best alignment. A row in which
//! every word of the query starts a word of the row (`core flow` in `Core
//! flow evidence map`) ranks above every row in which none of them does,
//! since scattered word starts can outscore the words a person typed. Then
//! rows rank by score, then shorter first, then input order; at most
//! [`LIMIT`] are returned.

/// The most matches returned.
pub const LIMIT: usize = 200;

/// One row that matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The row's index in the input.
    pub row: usize,
    pub score: i32,
    /// The matched characters' indexes (in characters, not bytes), for
    /// highlighting. Empty for an empty query.
    pub positions: Vec<usize>,
}

const NONE: i32 = i32::MIN / 2;

/// Ranks `rows` against `query`, best first. An empty query keeps the input
/// order with score 0.
pub fn rank(query: &str, rows: &[&str]) -> Vec<Match> {
    let q: Vec<char> = query.chars().map(lower).collect();
    if q.is_empty() {
        return rows
            .iter()
            .enumerate()
            .take(LIMIT)
            .map(|(row, _)| Match {
                row,
                score: 0,
                positions: Vec::new(),
            })
            .collect();
    }
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<(Match, usize, bool)> = rows
        .iter()
        .enumerate()
        .filter_map(|(row, text)| {
            let chars: Vec<char> = text.chars().collect();
            align(&q, &chars).map(|(score, positions)| {
                (
                    Match {
                        row,
                        score,
                        positions,
                    },
                    chars.len(),
                    starts_words(&words, &chars),
                )
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then(b.0.score.cmp(&a.0.score))
            .then(a.1.cmp(&b.1))
            .then(a.0.row.cmp(&b.0.row))
    });
    found.into_iter().take(LIMIT).map(|(m, _, _)| m).collect()
}

/// Whether each of `words` (lower case) begins at a word start of `row`.
fn starts_words(words: &[String], row: &[char]) -> bool {
    let low: String = row.iter().copied().map(lower).collect();
    let starts: Vec<usize> = (0..row.len()).filter(|&j| word_start(row, j)).collect();
    // Byte offset of each character, to compare against `low`.
    let offsets: Vec<usize> = low.char_indices().map(|(i, _)| i).collect();
    !words.is_empty()
        && words.iter().all(|w| {
            starts.iter().any(|&j| {
                offsets
                    .get(j)
                    .is_some_and(|&b| low[b..].starts_with(w.as_str()))
            })
        })
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn word_start(row: &[char], j: usize) -> bool {
    if j == 0 {
        return true;
    }
    let prev = row[j - 1];
    matches!(prev, '/' | ' ' | '-' | '_' | '.') || (prev.is_lowercase() && row[j].is_uppercase())
}

/// The best alignment of `q` in `row` as (score, positions), or `None`.
///
/// `best[i][j]` is the best score with `q[i]` matched at `row[j]`. The
/// predecessor is either the character just before `j` (worth 8, or 16 at a
/// word start) or any earlier one (worth 16 at a word start, else 1), so a
/// running maximum over the older columns keeps this linear in the row.
fn align(q: &[char], row: &[char]) -> Option<(i32, Vec<usize>)> {
    let n = row.len();
    if q.len() > n {
        return None;
    }
    let low: Vec<char> = row.iter().copied().map(lower).collect();
    let mut best = vec![vec![NONE; n]; q.len()];
    let mut from = vec![vec![usize::MAX; n]; q.len()];
    for j in 0..n {
        if low[j] == q[0] {
            let at = if word_start(row, j) { 16 } else { 1 };
            best[0][j] = at - j as i32;
        }
    }
    for i in 1..q.len() {
        // The best `best[i - 1][k]` over k < j - 1, and its k.
        let mut older: (i32, usize) = (NONE, usize::MAX);
        for j in 0..n {
            if j >= 2 && best[i - 1][j - 2] > older.0 {
                older = (best[i - 1][j - 2], j - 2);
            }
            if low[j] != q[i] {
                continue;
            }
            let start = word_start(row, j);
            let mut top = (NONE, usize::MAX);
            if j >= 1 && best[i - 1][j - 1] > NONE {
                top = (best[i - 1][j - 1] + if start { 16 } else { 8 }, j - 1);
            }
            if older.0 > NONE {
                let score = older.0 + if start { 16 } else { 1 };
                if score > top.0 {
                    top = (score, older.1);
                }
            }
            best[i][j] = top.0;
            from[i][j] = top.1;
        }
    }
    let last = q.len() - 1;
    let mut end = usize::MAX;
    let mut score = NONE;
    for (j, &s) in best[last].iter().enumerate() {
        if s > score {
            score = s;
            end = j;
        }
    }
    if score <= NONE {
        return None;
    }
    let mut positions = vec![0; q.len()];
    let mut j = end;
    for i in (0..q.len()).rev() {
        positions[i] = j;
        j = from[i][j];
    }
    Some((score, positions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(query: &str, row: &str) -> Option<(i32, Vec<usize>)> {
        rank(query, &[row]).pop().map(|m| (m.score, m.positions))
    }

    #[test]
    fn scores_of_the_design() {
        // First character 16, then three adjacent at 8.
        assert_eq!(one("abc", "abc"), Some((32, vec![0, 1, 2])));
        // Word starts after each separator, no skip cost at the first.
        assert_eq!(one("wr", "web-retries"), Some((32, vec![0, 4])));
        // One character skipped before the first match, then a word start.
        assert_eq!(one("b", "ab b"), Some((16 - 3, vec![3])));
        // A camel-case hump is a word start.
        assert_eq!(one("r", "webRetry"), Some((16 - 3, vec![3])));
        // Neither a word start nor adjacent: 1.
        assert_eq!(one("ac", "abc"), Some((16 + 1, vec![0, 2])));
    }

    #[test]
    fn case_is_ignored_and_misses_do_not_match() {
        assert!(one("WEB", "web").is_some());
        assert!(one("xyz", "web").is_none());
        assert!(one("ba", "ab").is_none());
        assert!(one("abcd", "abc").is_none());
    }

    #[test]
    fn the_best_alignment_wins_not_the_first() {
        // a at 0 then b at 3 scores 16 + 1; a at 2 (a word start, two
        // skipped) then the adjacent b scores 16 - 2 + 8, which is more.
        assert_eq!(one("ab", "a-ab"), Some((22, vec![2, 3])));
    }

    #[test]
    fn webhook_ranks_above_weather() {
        let rows = [
            "brain/Weather report.md Weather report",
            "brain/Webhook retries.md Webhook retries",
        ];
        let got = rank("whret", &rows);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].row, 1);
        assert_eq!(got[1].row, 0);
        assert!(got[0].score > got[1].score);
    }

    #[test]
    fn ties_go_to_the_shorter_row_then_input_order() {
        let rows = ["abc-long", "abc", "abc"];
        let got = rank("abc", &rows);
        assert_eq!(got.iter().map(|m| m.row).collect::<Vec<_>>(), vec![1, 2, 0]);
    }

    #[test]
    fn an_empty_query_keeps_the_input_order() {
        let rows = ["b", "a", "c"];
        let got = rank("", &rows);
        assert_eq!(got.iter().map(|m| m.row).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(got.iter().all(|m| m.score == 0 && m.positions.is_empty()));
    }

    #[test]
    fn at_most_two_hundred() {
        let rows: Vec<String> = (0..500).map(|i| format!("a{i}")).collect();
        let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
        assert_eq!(rank("a", &refs).len(), LIMIT);
        assert_eq!(rank("", &refs).len(), LIMIT);
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        assert_eq!(one("b", "éb"), Some((1 - 1, vec![1])));
    }

    #[test]
    fn typed_words_beat_scattered_word_starts() {
        let rows = [
            "Scoped owners exemption fixes on lane/scoped-owners 2026-11-02 03f7a0f1",
            "Store pricing rules built: weekend rates",
            "Core flow evidence map notes/Guides/Core flow evidence map.md",
        ];
        let got = rank("core flow", &rows);
        assert_eq!(got[0].row, 2, "{got:?}");
        // One word is enough to order it first, too.
        assert_eq!(rank("evid", &rows)[0].row, 2);
    }
}

//! The recall suite: for each query, the rank of the first right turn
//! among the 20 turns `hippo recall` returns.

use super::set::{ExpectedTurn, RecallKind, Set};
use crate::error::Result;
use crate::index::{Hit, Index};

/// The limit of `hippo recall` by default.
const LIMIT: usize = 20;

/// One question's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallRow {
    pub query: String,
    pub kind: RecallKind,
    /// The 1-based rank of the first expected turn, `None` when it is not
    /// among the hits.
    pub rank: Option<usize>,
}

/// Every row of the suite, in set order, and the totals over them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecallResult {
    pub rows: Vec<RecallRow>,
}

impl RecallResult {
    /// Questions whose rank is at most `n`.
    pub fn hit_at(&self, n: usize) -> usize {
        self.rows
            .iter()
            .filter(|row| row.rank.is_some_and(|rank| rank <= n))
            .count()
    }

    /// The mean of 1/rank over all questions, 0 for a miss.
    pub fn mrr(&self) -> f64 {
        if self.rows.is_empty() {
            return 0.0;
        }
        let sum: f64 = self
            .rows
            .iter()
            .filter_map(|row| row.rank)
            .map(|rank| 1.0 / rank as f64)
            .sum();
        sum / self.rows.len() as f64
    }

    /// For each kind the set has, in the fixed order of [`RecallKind::ALL`]: the
    /// questions that found their turn within the limit, and all its
    /// questions.
    pub fn by_kind(&self) -> Vec<(RecallKind, usize, usize)> {
        RecallKind::ALL
            .iter()
            .filter_map(|kind| {
                let rows: Vec<&RecallRow> =
                    self.rows.iter().filter(|row| row.kind == *kind).collect();
                let hits = rows.iter().filter(|row| row.rank.is_some()).count();
                (!rows.is_empty()).then_some((*kind, hits, rows.len()))
            })
            .collect()
    }

    /// `recall: hit@1 12/20, hit@5 15/20, hit@20 16/20, mrr 0.68 (verbatim
    /// 10/10, tool 3/4, codex 2/2, paraphrase 1/4)`.
    pub fn summary(&self) -> String {
        let total = self.rows.len();
        let kinds: Vec<String> = self
            .by_kind()
            .iter()
            .map(|(kind, hits, count)| format!("{} {hits}/{count}", kind.as_str()))
            .collect();
        let kinds = if kinds.is_empty() {
            String::new()
        } else {
            format!(" ({})", kinds.join(", "))
        };
        format!(
            "recall: hit@1 {}/{total}, hit@5 {}/{total}, hit@20 {}/{total}, mrr {:.2}{kinds}",
            self.hit_at(1),
            self.hit_at(5),
            self.hit_at(20),
            self.mrr()
        )
    }
}

/// The 1-based rank of the first hit that is one of `expected`.
pub fn rank_of(hits: &[Hit], expected: &[ExpectedTurn]) -> Option<usize> {
    hits.iter()
        .position(|hit| {
            expected
                .iter()
                .any(|turn| turn.session == hit.session && turn.ordinal == hit.ordinal)
        })
        .map(|at| at + 1)
}

/// Runs every query as `hippo recall <query>` does.
pub fn run(index: &Index, set: &Set) -> Result<RecallResult> {
    let mut rows = Vec::new();
    for question in &set.recall {
        let hits = index.recall(&question.query, LIMIT, false)?;
        rows.push(RecallRow {
            query: question.query.clone(),
            kind: question.kind,
            rank: rank_of(&hits, &question.expected),
        });
    }
    Ok(RecallResult { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(session: &str, ordinal: u32) -> Hit {
        Hit {
            session: session.into(),
            tool: "claude-code".into(),
            ordinal,
            role: "user".into(),
            ts: "2026-11-01T00:00:00Z".into(),
            tool_name: None,
            snippet: String::new(),
            score: -1.0,
        }
    }

    fn turn(session: &str, ordinal: u32) -> ExpectedTurn {
        ExpectedTurn {
            session: session.into(),
            ordinal,
        }
    }

    #[test]
    fn rank_is_one_based_and_any_expected_turn_counts() {
        let hits = [hit("a", 1), hit("b", 2), hit("a", 7)];
        assert_eq!(rank_of(&hits, &[turn("a", 1)]), Some(1));
        assert_eq!(rank_of(&hits, &[turn("a", 7), turn("b", 2)]), Some(2));
        // Same ordinal, other session: not a match.
        assert_eq!(rank_of(&hits, &[turn("c", 2)]), None);
        assert_eq!(rank_of(&[], &[turn("a", 1)]), None);
    }

    #[test]
    fn totals_for_hand_made_ranks() {
        let row = |kind, rank| RecallRow {
            query: "q".into(),
            kind,
            rank,
        };
        let result = RecallResult {
            rows: vec![
                row(RecallKind::Verbatim, Some(1)),
                row(RecallKind::Verbatim, Some(3)),
                row(RecallKind::Tool, Some(20)),
                row(RecallKind::Paraphrase, None),
            ],
        };
        assert_eq!(
            (result.hit_at(1), result.hit_at(5), result.hit_at(20)),
            (1, 2, 3)
        );
        // (1 + 1/3 + 1/20 + 0) / 4 = 0.3458
        assert_eq!(
            result.summary(),
            "recall: hit@1 1/4, hit@5 2/4, hit@20 3/4, mrr 0.35 (verbatim 2/2, tool 1/1, paraphrase 0/1)"
        );
        assert_eq!(
            result.by_kind(),
            [
                (RecallKind::Verbatim, 2, 2),
                (RecallKind::Tool, 1, 1),
                (RecallKind::Paraphrase, 0, 1)
            ]
        );
    }

    #[test]
    fn an_empty_suite_has_a_zero_mrr() {
        assert_eq!(
            RecallResult::default().summary(),
            "recall: hit@1 0/0, hit@5 0/0, hit@20 0/0, mrr 0.00"
        );
    }
}

//! The arithmetic of evidence: exact
//! fractions, a measurement's benefit, the evidence score, a measurement's
//! status, and the helpers that write scores for pages.
//!
//! Signs, comparisons and the median use [`Fraction`], so a benefit of
//! exactly 0 is 0 on every machine; only the final score and the order of
//! scores within a group use floating point. The second half reads
//! measurements from the database: each content's measurements with their
//! status, and a pack's evidence, which is the score of its newest content
//! with three counted measurements. Entering a measurement is later work.

use std::cmp::Ordering;

use rusqlite::{params, Connection};
use time::OffsetDateTime;

use crate::clock;
use crate::policy::{EVIDENCE_MIN_MEASUREMENTS, EVIDENCE_SHRINK};
use crate::votes::{self, ESTABLISHED_WHERE};

// --- Fractions ---

/// An exact fraction: an `i64` numerator over a positive `i64` denominator,
/// kept in lowest terms with zero as `0/1`. Fractions compare by
/// cross-multiplication in `i128`.
#[derive(Debug, Clone, Copy)]
pub struct Fraction {
    num: i64,
    den: i64,
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Fraction {
    pub const ZERO: Fraction = Fraction { num: 0, den: 1 };
    pub const ONE: Fraction = Fraction { num: 1, den: 1 };
    pub const MINUS_ONE: Fraction = Fraction { num: -1, den: 1 };

    /// `num / den` in lowest terms, or `None` when `den` is 0 or the reduced
    /// fraction does not fit in `i64`.
    pub fn new(num: i64, den: i64) -> Option<Fraction> {
        Fraction::from_i128(i128::from(num), i128::from(den))
    }

    fn from_i128(num: i128, den: i128) -> Option<Fraction> {
        if den == 0 {
            return None;
        }
        let g = gcd(num, den);
        let (mut num, mut den) = (num / g, den / g);
        if den < 0 {
            (num, den) = (-num, -den);
        }
        Some(Fraction {
            num: i64::try_from(num).ok()?,
            den: i64::try_from(den).ok()?,
        })
    }

    /// Like `from_i128`, but a fraction too large for `i64` becomes the
    /// nearest one over 2^40. Only measurements of absurdly large eval sets
    /// reach that, and the feed's configuration does not allow them.
    fn from_i128_nearest(num: i128, den: i128) -> Fraction {
        Fraction::from_i128(num, den).unwrap_or_else(|| {
            const SCALE: i128 = 1 << 40;
            let exact = num as f64 / den as f64;
            let scaled = (exact * SCALE as f64).round() as i64;
            Fraction::from_i128(i128::from(scaled), SCALE).expect("the scale is not 0")
        })
    }

    pub fn num(self) -> i64 {
        self.num
    }

    pub fn den(self) -> i64 {
        self.den
    }

    /// -1, 0 or 1.
    pub fn signum(self) -> i64 {
        self.num.signum()
    }

    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// The fraction limited to `[-1, 1]`.
    pub fn clamp_unit(self) -> Fraction {
        if self > Fraction::ONE {
            Fraction::ONE
        } else if self < Fraction::MINUS_ONE {
            Fraction::MINUS_ONE
        } else {
            self
        }
    }

    /// The exact mean of two fractions.
    pub fn mean(a: Fraction, b: Fraction) -> Fraction {
        let (an, ad) = (i128::from(a.num), i128::from(a.den));
        let (bn, bd) = (i128::from(b.num), i128::from(b.den));
        Fraction::from_i128_nearest(an * bd + bn * ad, 2 * ad * bd)
    }

    /// The fraction as a signed decimal with `places` digits, rounded half
    /// away from zero, computed exactly: `+0.14`, `-0.05`, `0.00`. A value
    /// that rounds to zero has no sign.
    pub fn signed_decimals(self, places: u32) -> String {
        let scale = 10i128.pow(places);
        let num = i128::from(self.num).abs() * scale;
        let den = i128::from(self.den);
        let units = (2 * num + den) / (2 * den);
        format_signed(self.signum(), units, scale)
    }
}

impl PartialEq for Fraction {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Fraction {}

impl PartialOrd for Fraction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Fraction {
    fn cmp(&self, other: &Self) -> Ordering {
        let left = i128::from(self.num) * i128::from(other.den);
        let right = i128::from(other.num) * i128::from(self.den);
        left.cmp(&right)
    }
}

// --- Benefit ---

/// The counts of a measurement, each summed over the runs. `planted`,
/// `kept` and `runs` are the eval set's and the member's; the checks that
/// the counts are in range are the measurement form's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Planted errors per run.
    pub planted: i64,
    /// Kept claims per run.
    pub kept: i64,
    /// Runs of each arm.
    pub runs: i64,
    pub corrected_without: i64,
    pub damaged_without: i64,
    pub corrected_with: i64,
    pub damaged_with: i64,
}

/// An arm's net correction rate: `corrected / (planted × runs) − damaged /
/// (kept × runs)`, as `(corrected × kept − damaged × planted) / (planted ×
/// kept × runs)`. Zero when the set or the runs are empty.
pub fn net_rate(planted: i64, kept: i64, runs: i64, corrected: i64, damaged: i64) -> Fraction {
    let (planted, kept, runs) = (i128::from(planted), i128::from(kept), i128::from(runs));
    let num = i128::from(corrected) * kept - i128::from(damaged) * planted;
    let den = planted * kept * runs;
    if den <= 0 {
        return Fraction::ZERO;
    }
    Fraction::from_i128_nearest(num, den)
}

/// A measurement's benefit: the net rate with the pack minus the net rate
/// without it, clamped to `[-1, 1]`: `((corrected_with − corrected_without) ×
/// kept − (damaged_with − damaged_without) × planted) / (planted × kept ×
/// runs)`, exactly.
pub fn benefit(counts: &Counts) -> Fraction {
    let (planted, kept, runs) = (
        i128::from(counts.planted),
        i128::from(counts.kept),
        i128::from(counts.runs),
    );
    let den = planted * kept * runs;
    if den <= 0 {
        return Fraction::ZERO;
    }
    let num = i128::from(counts.corrected_with - counts.corrected_without) * kept
        - i128::from(counts.damaged_with - counts.damaged_without) * planted;
    // Clamp before narrowing, so a fraction that is out of range is never
    // larger than the representation.
    let num = num.clamp(-den, den);
    Fraction::from_i128_nearest(num, den)
}

// --- Score ---

/// The evidence of one pack content with at least 3 counted measurements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Evidence {
    /// `median × n / (n + 2)`.
    pub score: f64,
    /// How many measurements count.
    pub n: u32,
    /// The median of the benefits, exactly.
    pub median: Fraction,
}

/// The median of an unsorted list; of an even count, the mean of the two
/// middle values. `None` for an empty list.
pub fn median(benefits: &[Fraction]) -> Option<Fraction> {
    if benefits.is_empty() {
        return None;
    }
    let mut sorted = benefits.to_vec();
    sorted.sort();
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        Fraction::mean(sorted[mid - 1], sorted[mid])
    })
}

/// The evidence score of the benefits of the measurements that count: `None`
/// below 3, otherwise `median × n / (n + 2)`.
pub fn score(benefits: &[Fraction]) -> Option<Evidence> {
    let n = u32::try_from(benefits.len()).ok()?;
    if n < EVIDENCE_MIN_MEASUREMENTS {
        return None;
    }
    let median = median(benefits)?;
    let shrink = f64::from(n) / f64::from(n + EVIDENCE_SHRINK);
    Some(Evidence {
        score: median.to_f64() * shrink,
        n,
        median,
    })
}

// --- Status ---

/// A measurement's public status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// A moderator marked it invalid.
    Invalid,
    /// Its member is one of the pack's maintainers.
    Maintainers,
    /// Its member is not established now.
    NotCounted,
    Counted,
}

impl Status {
    /// The word the pack page shows.
    pub fn label(self) -> &'static str {
        match self {
            Status::Invalid => "invalid",
            Status::Maintainers => "maintainer's",
            Status::NotCounted => "not counted",
            Status::Counted => "counted",
        }
    }

    /// Whether the measurement enters the score.
    pub fn counts(self) -> bool {
        self == Status::Counted
    }
}

/// The first status that applies, in this order: invalid,
/// maintainer's, not counted, counted.
pub fn status(invalid: bool, is_maintainer: bool, established: bool) -> Status {
    if invalid {
        Status::Invalid
    } else if is_maintainer {
        Status::Maintainers
    } else if !established {
        Status::NotCounted
    } else {
        Status::Counted
    }
}

// --- Reading measurements ---

/// One measurement as the pack page shows it.
#[derive(Debug, Clone)]
pub struct MeasurementRow {
    pub id: i64,
    /// The member's login; `None` for an account that was deleted.
    pub member: Option<String>,
    /// When the member first entered it (a replacement keeps this time).
    pub created_at: String,
    pub eval_set: String,
    pub runs: i64,
    pub hippo: String,
    pub model: String,
    pub report_url: Option<String>,
    pub revisions: i64,
    pub counts: Counts,
    pub net_without: Fraction,
    pub net_with: Fraction,
    pub benefit: Fraction,
    pub status: Status,
}

/// One pack content with its measurements and score.
#[derive(Debug, Clone)]
pub struct ContentEvidence {
    pub pack_hash: String,
    pub version: String,
    pub first_seen: String,
    /// Whether this is the pack's current content (the `packs` row's hash).
    pub current: bool,
    /// The number of measurements that count.
    pub counted: u32,
    /// `Some` with at least 3 counted measurements.
    pub evidence: Option<Evidence>,
    /// Every measurement, oldest first.
    pub measurements: Vec<MeasurementRow>,
}

/// A pack's evidence: the score of its newest content that is measured.
#[derive(Debug, Clone, PartialEq)]
pub struct PackEvidence {
    pub evidence: Evidence,
    /// The version of that content.
    pub version: String,
    /// Whether the content is the pack's current one; if not, pages say
    /// "evidence from <version>".
    pub current: bool,
}

/// What a row needs of a pack's evidence.
#[derive(Debug, Clone)]
pub struct Summary {
    pub evidence: Option<PackEvidence>,
    /// Counted measurements of the pack's current content.
    pub current_counted: u32,
}

/// Every content of `pack`, newest first by `first_seen` (then by hash), each
/// with its measurements, their statuses and its score at `now`.
pub fn contents(
    conn: &Connection,
    pack: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<Vec<ContentEvidence>> {
    let current: Option<String> = conn
        .query_row("SELECT pack_hash FROM packs WHERE name = ?1", [pack], |r| {
            r.get(0)
        })
        .ok();
    let mut stmt = conn.prepare(
        "SELECT pack_hash, version, first_seen FROM pack_contents
         WHERE pack = ?1 ORDER BY first_seen DESC, pack_hash",
    )?;
    let mut out: Vec<ContentEvidence> = stmt
        .query_map([pack], |r| {
            let hash: String = r.get(0)?;
            Ok(ContentEvidence {
                current: current.as_deref() == Some(hash.as_str()),
                pack_hash: hash,
                version: r.get(1)?,
                first_seen: r.get(2)?,
                counted: 0,
                evidence: None,
                measurements: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    if out.is_empty() {
        return Ok(out);
    }
    let [a, b, c] = votes::cutoffs(now);
    let sql = format!(
        "SELECT m.pack_hash, m.id, CASE WHEN u.deleted_at IS NULL THEN u.login END,
                m.created_at, m.eval_set, m.planted, m.kept, m.runs,
                m.corrected_without, m.damaged_without, m.corrected_with, m.damaged_with,
                m.hippo, m.model, m.report_url, m.revisions, m.invalid,
                ({ESTABLISHED_WHERE}),
                ({maint})
         FROM measurements m JOIN users u ON u.id = m.user_id
         WHERE m.pack = ?4
         ORDER BY m.created_at, m.id",
        maint = votes::maintainer_sql("m.pack", "u.github_id", "u.login")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![a, b, c, pack], |r| {
        let counts = Counts {
            planted: r.get(5)?,
            kept: r.get(6)?,
            runs: r.get(7)?,
            corrected_without: r.get(8)?,
            damaged_without: r.get(9)?,
            corrected_with: r.get(10)?,
            damaged_with: r.get(11)?,
        };
        let status = status(r.get(16)?, r.get(18)?, r.get(17)?);
        Ok((
            r.get::<_, String>(0)?,
            MeasurementRow {
                id: r.get(1)?,
                member: r.get(2)?,
                created_at: r.get(3)?,
                eval_set: r.get(4)?,
                runs: counts.runs,
                hippo: r.get(12)?,
                model: r.get(13)?,
                report_url: r.get(14)?,
                revisions: r.get(15)?,
                net_without: net_rate(
                    counts.planted,
                    counts.kept,
                    counts.runs,
                    counts.corrected_without,
                    counts.damaged_without,
                ),
                net_with: net_rate(
                    counts.planted,
                    counts.kept,
                    counts.runs,
                    counts.corrected_with,
                    counts.damaged_with,
                ),
                benefit: benefit(&counts),
                counts,
                status,
            },
        ))
    })?;
    for row in rows {
        let (hash, measurement) = row?;
        if let Some(content) = out.iter_mut().find(|c| c.pack_hash == hash) {
            content.measurements.push(measurement);
        }
    }
    for content in &mut out {
        let benefits: Vec<Fraction> = content
            .measurements
            .iter()
            .filter(|m| m.status.counts())
            .map(|m| m.benefit)
            .collect();
        content.counted = u32::try_from(benefits.len()).unwrap_or(u32::MAX);
        content.evidence = score(&benefits);
    }
    Ok(out)
}

/// The summary of a list of contents.
pub fn summarise(contents: &[ContentEvidence]) -> Summary {
    Summary {
        evidence: contents.iter().find_map(|c| {
            c.evidence.map(|evidence| PackEvidence {
                evidence,
                version: c.version.clone(),
                current: c.current,
            })
        }),
        current_counted: contents.iter().find(|c| c.current).map_or(0, |c| c.counted),
    }
}

/// A pack's summary at `now`.
pub fn summary(conn: &Connection, pack: &str, now: OffsetDateTime) -> rusqlite::Result<Summary> {
    Ok(summarise(&contents(conn, pack, now)?))
}

/// The score of the pack's newest content (by `first_seen`) with at least 3
/// counted measurements, and whether that content is the current one. A
/// measurement counts when it is not invalid, its member is established at
/// `now` and the member is not one of the pack's maintainers.
pub fn pack_evidence(
    conn: &Connection,
    pack: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<Option<PackEvidence>> {
    Ok(summary(conn, pack, now)?.evidence)
}

/// A stored time as a date, for the measurements table.
pub fn date_of(stored: &str) -> String {
    clock::parse_storage(stored)
        .map(clock::date_text)
        .unwrap_or_default()
}

// --- Display ---

fn format_signed(sign: i64, units: i128, scale: i128) -> String {
    let (whole, part) = (units / scale, units % scale);
    let places = scale.ilog10() as usize;
    let sign = if units == 0 {
        ""
    } else if sign < 0 {
        "-"
    } else {
        "+"
    };
    if places == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{part:0places$}")
    }
}

/// A score with a sign and `places` decimals, rounded half away from zero; a
/// value that rounds to zero has no sign.
pub fn signed_decimals(value: f64, places: u32) -> String {
    let scale = 10i128.pow(places);
    let units = (value * scale as f64).round() as i128;
    let sign = if units < 0 { -1 } else { 1 };
    format_signed(sign, units.abs(), scale)
}

/// An evidence score as a row writes it: rounded half away from zero to two
/// decimals, with a sign (`+0.14`, `-0.05`, `0.00`).
pub fn signed_two_decimals(score: f64) -> String {
    signed_decimals(score, 2)
}

/// A score to three decimals, for the pack page and the API: `+0.143`.
pub fn signed_three_decimals(score: f64) -> String {
    signed_decimals(score, 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(num: i64, den: i64) -> Fraction {
        Fraction::new(num, den).expect("a fraction")
    }

    #[test]
    fn fractions_are_reduced_and_signed_on_the_numerator() {
        let x = f(6, -8);
        assert_eq!((x.num(), x.den()), (-3, 4));
        let zero = f(0, -5);
        assert_eq!((zero.num(), zero.den()), (0, 1));
        assert_eq!(Fraction::new(1, 0), None);
    }

    #[test]
    fn fractions_compare_exactly() {
        assert_eq!(f(1, 2), f(2, 4));
        assert!(f(1, 3) < f(1, 2));
        assert!(f(-1, 2) < f(-1, 3));
        assert!(f(-1, 3) < Fraction::ZERO);
        // Close enough that f64 cannot tell them apart; i128 can.
        let a = f(i64::MAX - 1, i64::MAX);
        let b = f(i64::MAX - 2, i64::MAX - 1);
        assert_ne!(a, b);
        assert!(a > b);
        assert_eq!(a.to_f64(), b.to_f64());
    }

    #[test]
    fn zero_compares_equal_to_zero() {
        assert_eq!(f(0, 7), Fraction::ZERO);
        assert!(f(0, 7).is_zero());
        assert_eq!(f(0, 7).signum(), 0);
    }

    #[test]
    fn clamp_limits_to_the_unit_interval() {
        assert_eq!(f(3, 2).clamp_unit(), Fraction::ONE);
        assert_eq!(f(-3, 2).clamp_unit(), Fraction::MINUS_ONE);
        assert_eq!(f(1, 2).clamp_unit(), f(1, 2));
        assert_eq!(f(-1, 1).clamp_unit(), Fraction::MINUS_ONE);
    }

    #[test]
    fn the_mean_is_exact() {
        assert_eq!(Fraction::mean(f(1, 3), f(1, 6)), f(1, 4));
        assert_eq!(Fraction::mean(f(-1, 2), f(1, 2)), Fraction::ZERO);
        assert_eq!(Fraction::mean(f(1, 7), f(1, 7)), f(1, 7));
    }

    fn counts(cw: i64, dw: i64, co: i64, dd: i64) -> Counts {
        // Eval set of 6 planted, 10 kept, 3 runs.
        Counts {
            planted: 6,
            kept: 10,
            runs: 3,
            corrected_without: co,
            damaged_without: dd,
            corrected_with: cw,
            damaged_with: dw,
        }
    }

    #[test]
    fn benefit_is_the_integer_formula() {
        // ((12 - 6) * 10 - (3 - 0) * 6) / (6 * 10 * 3) = 42 / 180 = 7/30
        assert_eq!(benefit(&counts(12, 3, 6, 0)), f(7, 30));
        // No change.
        assert_eq!(benefit(&counts(9, 2, 9, 2)), Fraction::ZERO);
        // Fixing more but damaging proportionally more is exactly zero:
        // (6 * 10 - 10 * 6) = 0.
        assert_eq!(benefit(&counts(6, 10, 0, 0)), Fraction::ZERO);
        // Harm.
        assert_eq!(benefit(&counts(0, 6, 6, 0)), f(-96, 180).clamp_unit());
    }

    #[test]
    fn benefit_matches_the_difference_of_net_rates() {
        let c = counts(12, 3, 6, 1);
        let with = net_rate(c.planted, c.kept, c.runs, c.corrected_with, c.damaged_with);
        let without = net_rate(
            c.planted,
            c.kept,
            c.runs,
            c.corrected_without,
            c.damaged_without,
        );
        let diff = (with.to_f64() - without.to_f64()).clamp(-1.0, 1.0);
        assert!((benefit(&c).to_f64() - diff).abs() < 1e-12);
    }

    #[test]
    fn net_rate_is_corrected_share_minus_damaged_share() {
        // 12/18 - 3/30 = 2/3 - 1/10 = 17/30
        assert_eq!(net_rate(6, 10, 3, 12, 3), f(17, 30));
        assert_eq!(net_rate(6, 10, 3, 0, 0), Fraction::ZERO);
        assert_eq!(net_rate(6, 10, 3, 0, 30), Fraction::MINUS_ONE);
        assert_eq!(net_rate(6, 10, 3, 18, 0), Fraction::ONE);
        assert_eq!(net_rate(0, 10, 3, 0, 0), Fraction::ZERO);
    }

    #[test]
    fn benefit_is_clamped() {
        // Out-of-range counts (the form refuses them) clamp rather than
        // overflow.
        let c = Counts {
            planted: 5,
            kept: 5,
            runs: 3,
            corrected_without: 0,
            damaged_without: 0,
            corrected_with: 1000,
            damaged_with: 0,
        };
        assert_eq!(benefit(&c), Fraction::ONE);
        let c = Counts {
            damaged_with: 1000,
            corrected_with: 0,
            ..c
        };
        assert_eq!(benefit(&c), Fraction::MINUS_ONE);
        // The extremes of a valid measurement.
        let c = Counts {
            corrected_with: 15,
            ..c
        };
        assert_eq!(
            benefit(&Counts {
                damaged_with: 0,
                corrected_with: 15,
                ..c
            }),
            Fraction::ONE
        );
    }

    #[test]
    fn benefit_survives_large_sets() {
        let c = Counts {
            planted: 3_000_000_000,
            kept: 3_000_000_000,
            runs: 10,
            corrected_without: 0,
            damaged_without: 0,
            corrected_with: 15_000_000_000,
            damaged_with: 0,
        };
        let b = benefit(&c);
        assert!((b.to_f64() - 0.5).abs() < 1e-9, "{}", b.to_f64());
    }

    #[test]
    fn score_is_none_below_three() {
        assert_eq!(score(&[]), None);
        assert_eq!(score(&[f(1, 2)]), None);
        assert_eq!(score(&[f(1, 2), f(1, 2)]), None);
    }

    #[test]
    fn score_shrinks_the_median_toward_zero() {
        let three = score(&[f(1, 2), f(1, 10), f(3, 10)]).expect("n = 3");
        assert_eq!(three.n, 3);
        assert_eq!(three.median, f(3, 10));
        assert!((three.score - 0.3 * 3.0 / 5.0).abs() < 1e-12);

        let four = score(&[f(1, 10), f(2, 10), f(3, 10), f(9, 10)]).expect("n = 4");
        assert_eq!(four.n, 4);
        assert_eq!(four.median, f(1, 4));
        assert!((four.score - 0.25 * 4.0 / 6.0).abs() < 1e-12);

        let five = score(&[f(-1, 5), f(1, 7), f(1, 7), f(1, 7), f(1, 1)]).expect("n = 5");
        assert_eq!(five.n, 5);
        assert_eq!(five.median, f(1, 7));
        assert!((five.score - (1.0 / 7.0) * 5.0 / 7.0).abs() < 1e-12);
    }

    #[test]
    fn the_median_of_an_even_count_is_the_mean_of_the_middle_two() {
        let m = median(&[f(1, 3), f(-1, 1), f(1, 6), f(1, 1)]).expect("median");
        assert_eq!(m, f(1, 4));
        assert_eq!(median(&[f(1, 2), f(1, 2)]), Some(f(1, 2)));
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[f(5, 7)]), Some(f(5, 7)));
    }

    #[test]
    fn one_false_measurement_stays_within_the_range_of_the_others() {
        let honest = [f(1, 10), f(1, 5), f(3, 20)];
        let mut with_liar = honest.to_vec();
        with_liar.push(f(-1, 1));
        let m = score(&with_liar).expect("n = 4").median;
        assert!(m >= f(1, 10) && m <= f(1, 5));
    }

    #[test]
    fn a_zero_benefit_scores_exactly_zero() {
        let s = score(&[Fraction::ZERO, f(0, 3), f(0, 9)]).expect("n = 3");
        assert!(s.median.is_zero());
        assert_eq!(s.score, 0.0);
        assert_eq!(signed_two_decimals(s.score), "0.00");
    }

    #[test]
    fn every_status_in_the_designs_order() {
        assert_eq!(status(true, true, false), Status::Invalid);
        assert_eq!(status(true, false, true), Status::Invalid);
        assert_eq!(status(false, true, false), Status::Maintainers);
        assert_eq!(status(false, true, true), Status::Maintainers);
        assert_eq!(status(false, false, false), Status::NotCounted);
        assert_eq!(status(false, false, true), Status::Counted);
        assert!(Status::Counted.counts());
        for s in [Status::Invalid, Status::Maintainers, Status::NotCounted] {
            assert!(!s.counts());
        }
        assert_eq!(Status::Invalid.label(), "invalid");
        assert_eq!(Status::Maintainers.label(), "maintainer's");
        assert_eq!(Status::NotCounted.label(), "not counted");
        assert_eq!(Status::Counted.label(), "counted");
    }

    #[test]
    fn scores_are_written_with_a_sign_and_two_decimals() {
        assert_eq!(signed_two_decimals(0.143), "+0.14");
        assert_eq!(signed_two_decimals(-0.05), "-0.05");
        assert_eq!(signed_two_decimals(0.0), "0.00");
        assert_eq!(signed_two_decimals(-0.0), "0.00");
        // 0.125 is exact in binary, so the half is a true half.
        assert_eq!(signed_two_decimals(0.125), "+0.13");
        assert_eq!(signed_two_decimals(-0.125), "-0.13");
        assert_eq!(signed_two_decimals(0.004), "0.00");
        assert_eq!(signed_two_decimals(-0.004), "0.00");
        assert_eq!(signed_two_decimals(1.0), "+1.00");
        assert_eq!(signed_two_decimals(-1.0), "-1.00");
        assert_eq!(signed_two_decimals(0.1), "+0.10");
    }

    #[test]
    fn scores_are_written_to_three_decimals() {
        assert_eq!(signed_three_decimals(1.0 / 7.0), "+0.143");
        assert_eq!(signed_three_decimals(-1.0 / 7.0), "-0.143");
        assert_eq!(signed_three_decimals(0.0004), "0.000");
        assert_eq!(signed_three_decimals(-0.0004), "0.000");
        assert_eq!(signed_three_decimals(0.0625), "+0.063");
        assert_eq!(signed_three_decimals(-0.0625), "-0.063");
    }

    #[test]
    fn fractions_are_written_exactly_and_agree_with_the_float_helper() {
        assert_eq!(f(1, 7).signed_decimals(2), "+0.14");
        assert_eq!(f(1, 7).signed_decimals(3), "+0.143");
        assert_eq!(f(-1, 20).signed_decimals(2), "-0.05");
        assert_eq!(f(0, 1).signed_decimals(2), "0.00");
        assert_eq!(f(1, 8).signed_decimals(2), "+0.13");
        assert_eq!(f(-1, 8).signed_decimals(2), "-0.13");
        assert_eq!(f(1, 1000).signed_decimals(2), "0.00");
        assert_eq!(f(-1, 1000).signed_decimals(2), "0.00");
        assert_eq!(f(1, 1).signed_decimals(2), "+1.00");
        assert_eq!(f(-1, 1).signed_decimals(3), "-1.000");
        for (n, d) in [(1, 3), (-2, 7), (5, 9), (-7, 30)] {
            let x = f(n, d);
            assert_eq!(
                x.signed_decimals(2),
                signed_two_decimals(x.to_f64()),
                "{n}/{d}"
            );
            assert_eq!(
                x.signed_decimals(3),
                signed_three_decimals(x.to_f64()),
                "{n}/{d}"
            );
        }
    }

    mod db {
        use super::super::*;
        use crate::posts::testkit::*;
        use time::macros::datetime;
        use time::Duration;

        const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);

        #[test]
        fn evidence_carries_from_an_older_content_and_statuses_apply() {
            let (_d, conn) = fresh();
            let old = NOW - Duration::days(400);
            let owner = user(&conn, "owner", old, old);
            let voters: Vec<i64> = (0..4)
                .map(|i| user(&conn, &format!("m{i}"), old, old))
                .collect();
            let newbie = user(&conn, "newbie", NOW - Duration::days(1), old);
            pack_post(&conn, "p", owner, r#"["m3"]"#, NOW - Duration::days(30));
            content(&conn, "p", "old", "1.0.0", NOW - Duration::days(20));
            content(&conn, "p", "new", "2.0.0", NOW - Duration::days(5));
            conn.execute(
                "UPDATE packs SET pack_hash = 'new', version = '2.0.0' WHERE name = 'p'",
                [],
            )
            .unwrap();
            for v in &voters[..3] {
                measure(&conn, "p", "old", *v, 12, 0);
            }
            // The new content has a maintainer's, a new member's and 1 counted.
            let maint = measure(&conn, "p", "new", voters[3], 12, 0);
            let nb = measure(&conn, "p", "new", newbie, 12, 0);
            measure(&conn, "p", "new", voters[0], 12, 0);
            let found = pack_evidence(&conn, "p", NOW).unwrap().unwrap();
            assert_eq!((found.version.as_str(), found.current), ("1.0.0", false));
            assert_eq!(found.evidence.n, 3);
            let all = contents(&conn, "p", NOW).unwrap();
            assert_eq!(all[0].version, "2.0.0");
            assert!(all[0].current && all[0].evidence.is_none() && all[0].counted == 1);
            let status_of = |id: i64| {
                all[0]
                    .measurements
                    .iter()
                    .find(|m| m.id == id)
                    .unwrap()
                    .status
            };
            assert_eq!(status_of(maint), Status::Maintainers);
            assert_eq!(status_of(nb), Status::NotCounted);
            assert_eq!(summary(&conn, "p", NOW).unwrap().current_counted, 1);
            conn.execute("UPDATE measurements SET invalid = 1 WHERE id = ?1", [maint])
                .unwrap();
            assert_eq!(
                contents(&conn, "p", NOW).unwrap()[0].measurements[0].status,
                Status::Invalid
            );
        }
    }
}

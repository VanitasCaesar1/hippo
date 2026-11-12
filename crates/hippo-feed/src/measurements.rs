//! Entering a measurement: the checks
//! of the form, replacement in place, and the system report a large benefit
//! files.
//!
//! This lives here, not in `evidence.rs`, so the arithmetic
//! module stays free of storage. The measure page calls
//! [`enter`] and shows a [`FormError`] again with its rule.
//!
//! Every function that writes takes the `&Transaction` of the change it makes
//! and the time as a parameter, so every decision reads the one `Clock`.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::Deserialize;
use serde_json::json;
use time::OffsetDateTime;
use url::Url;

use crate::audit;
use crate::clock;
use crate::config::EvalSet;
use crate::db::Member;
use crate::error::{AppError, FormError};
use crate::evidence::{self, Counts, Fraction};
use crate::limits::{self, Action};
use crate::policy;
use crate::reports::{self, ReportTarget};
use crate::text;
use crate::users::{self, Standing};

/// The fields of the measure form, as a urlencoded form sends them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct MeasurementForm {
    pub pack_hash: String,
    pub eval_set: String,
    pub hippo: String,
    pub model: String,
    pub runs: String,
    pub corrected_without: String,
    pub damaged_without: String,
    pub corrected_with: String,
    pub damaged_with: String,
    pub report_url: String,
}

/// What the checks need to know about the pack being measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackFacts {
    pub name: String,
    /// The pack is of kind `passes` (theme packs are not measured).
    pub passes: bool,
    pub listed: bool,
    pub post_visible: bool,
    /// Every recorded pack hash, newest first.
    pub hashes: Vec<String>,
}

/// The member's existing measurement of the same pack content, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Existing {
    pub id: i64,
    pub revisions: i64,
    pub invalid: bool,
}

/// A measurement that passed the checks, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMeasurement {
    pub pack: String,
    pub pack_hash: String,
    pub eval_set: String,
    pub planted: i64,
    pub kept: i64,
    pub runs: i64,
    pub corrected_without: i64,
    pub damaged_without: i64,
    pub corrected_with: i64,
    pub damaged_with: i64,
    pub hippo: String,
    pub model: String,
    pub report_url: Option<String>,
}

impl NewMeasurement {
    /// The counts, for the benefit.
    pub fn counts(&self) -> Counts {
        Counts {
            planted: self.planted,
            kept: self.kept,
            runs: self.runs,
            corrected_without: self.corrected_without,
            damaged_without: self.damaged_without,
            corrected_with: self.corrected_with,
            damaged_with: self.damaged_with,
        }
    }
}

fn number(field: &str, raw: &str) -> Result<i64, FormError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(FormError::new(field, "is required"));
    }
    trimmed
        .parse::<i64>()
        .map_err(|_| FormError::new(field, "must be a whole number"))
}

fn count(field: &str, raw: &str, max: i64) -> Result<i64, FormError> {
    let n = number(field, raw)?;
    if !(0..=max).contains(&n) {
        return Err(FormError::new(
            field,
            format!("must be between 0 and {max}"),
        ));
    }
    Ok(n)
}

/// `^[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$`.
fn hippo_version(raw: &str) -> Result<String, FormError> {
    const FIELD: &str = "hippo";
    let version = text::normalise(raw);
    if version.is_empty() {
        return Err(FormError::new(FIELD, "is required"));
    }
    let bytes = version.as_bytes();
    let ok = bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'));
    if !ok {
        return Err(FormError::new(
            FIELD,
            "must be a version such as 0.9.0: letters, digits, '.', '+' and '-', at most 64",
        ));
    }
    Ok(version)
}

/// An empty report link means none; otherwise an `https` URL with a host and
/// no user name or password, at most 300 bytes, stored as the `url` crate
/// writes it.
fn report_url(raw: &str) -> Result<Option<String>, FormError> {
    const FIELD: &str = "report_url";
    let trimmed = text::normalise(raw);
    if trimmed.is_empty() {
        return Ok(None);
    }
    let too_long = || {
        FormError::new(
            FIELD,
            format!("must be at most {} bytes", policy::REPORT_URL_MAX_BYTES),
        )
    };
    if trimmed.len() > policy::REPORT_URL_MAX_BYTES {
        return Err(too_long());
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(FormError::new(FIELD, "may not contain control characters"));
    }
    let url =
        Url::parse(&trimmed).map_err(|_| FormError::new(FIELD, "must be an https address"))?;
    if url.scheme() != "https" {
        return Err(FormError::new(FIELD, "must start with https://"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(FormError::new(FIELD, "must name a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FormError::new(
            FIELD,
            "may not contain a user name or password",
        ));
    }
    let stored = url.to_string();
    if stored.len() > policy::REPORT_URL_MAX_BYTES {
        return Err(too_long());
    }
    Ok(Some(stored))
}

/// The checks of "Evidence", in the order the form lists them. The first
/// that fails is the error. `standing` is the member's standing now; the
/// pack's maintainers pass (their measurements are stored and never counted).
pub fn check(
    form: &MeasurementForm,
    pack: &PackFacts,
    sets: &[EvalSet],
    standing: Standing,
    existing: Option<&Existing>,
) -> Result<NewMeasurement, FormError> {
    if !pack.passes || !pack.listed || !pack.post_visible {
        return Err(FormError::new(
            "pack",
            "must be a listed pack of passes that is shown in the feed",
        ));
    }
    let hash = form.pack_hash.trim();
    if !pack.hashes.iter().any(|h| h == hash) {
        return Err(FormError::new(
            "pack_hash",
            "must be a recorded version of this pack",
        ));
    }
    if sets.is_empty() {
        return Err(FormError::new("eval_set", "measuring is not open"));
    }
    let set = sets
        .iter()
        .find(|s| s.id == form.eval_set.trim())
        .ok_or_else(|| FormError::new("eval_set", "must be one of the listed eval sets"))?;
    let runs = number("runs", &form.runs)?;
    let (min, max) = (
        i64::from(policy::MEASURE_RUNS_MIN),
        i64::from(policy::MEASURE_RUNS_MAX),
    );
    if !(min..=max).contains(&runs) {
        return Err(FormError::new(
            "runs",
            format!("must be between {min} and {max}"),
        ));
    }
    let (planted, kept) = (i64::from(set.planted), i64::from(set.kept));
    let corrected_max = planted * runs;
    let damaged_max = kept * runs;
    let corrected_without = count("corrected_without", &form.corrected_without, corrected_max)?;
    let damaged_without = count("damaged_without", &form.damaged_without, damaged_max)?;
    let corrected_with = count("corrected_with", &form.corrected_with, corrected_max)?;
    let damaged_with = count("damaged_with", &form.damaged_with, damaged_max)?;
    let hippo = hippo_version(&form.hippo)?;
    let model = text::one_line("model", &form.model, policy::MODEL_MAX)?;
    let report_url = report_url(&form.report_url)?;
    if standing != Standing::Established {
        return Err(FormError::new(
            "member",
            "must be an established account to measure",
        ));
    }
    if existing.is_some_and(|e| e.invalid) {
        return Err(FormError::new(
            "pack_hash",
            "your measurement of this version was marked invalid and cannot be replaced",
        ));
    }
    Ok(NewMeasurement {
        pack: pack.name.clone(),
        pack_hash: hash.to_string(),
        eval_set: set.id.clone(),
        planted,
        kept,
        runs,
        corrected_without,
        damaged_without,
        corrected_with,
        damaged_with,
        hippo,
        model,
        report_url,
    })
}

/// The facts about a pack for [`check`], or `None` if there is no such pack.
pub fn pack_facts(tx: &Transaction, name: &str) -> rusqlite::Result<Option<PackFacts>> {
    let row: Option<(String, String, bool)> = tx
        .query_row(
            "SELECT name, kind, listed FROM packs WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((name, kind, listed)) = row else {
        return Ok(None);
    };
    let post_visible: bool = tx
        .query_row(
            "SELECT state = 'visible' FROM posts WHERE pack = ?1",
            [&name],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false);
    let mut stmt = tx.prepare(
        "SELECT pack_hash FROM pack_contents WHERE pack = ?1
         ORDER BY first_seen DESC, pack_hash",
    )?;
    let hashes = stmt
        .query_map([&name], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    drop(stmt);
    Ok(Some(PackFacts {
        name,
        passes: kind == "passes",
        listed,
        post_visible,
        hashes,
    }))
}

/// Enters `member`'s measurement of the pack named `pack` and returns its id;
/// a measurement of content the member already measured is replaced in place
/// (same id and first-entry time, new values, `revisions` plus one).
///
/// Refused: a pack that does not exist (404); a restricted or banned member
/// (403); a form that breaks a rule of [`check`] (422); the per-member limit
/// (429, or 403 where measuring is not open to the account). `planted` and
/// `kept` are copied from the configuration. A benefit of magnitude 0.5 or
/// more files a `misleading` system report and an `evidence.flag` row,
/// unless a report is already open on the measurement.
pub fn enter(
    tx: &Transaction,
    member: &Member,
    pack: &str,
    form: &MeasurementForm,
    sets: &[EvalSet],
    now: OffsetDateTime,
) -> Result<i64, AppError> {
    let facts = pack_facts(tx, pack)?.ok_or_else(AppError::not_found)?;
    let standing = users::standing(member, now);
    if matches!(standing, Standing::Restricted | Standing::Banned) {
        return Err(AppError::forbidden(
            "A restricted account cannot enter measurements.",
        ));
    }
    let existing: Option<Existing> = tx
        .query_row(
            "SELECT id, revisions, invalid FROM measurements
             WHERE pack = ?1 AND pack_hash = ?2 AND user_id = ?3",
            params![facts.name, form.pack_hash.trim(), member.id.0],
            |r| {
                Ok(Existing {
                    id: r.get(0)?,
                    revisions: r.get(1)?,
                    invalid: r.get(2)?,
                })
            },
        )
        .optional()?;
    let new = check(form, &facts, sets, standing, existing.as_ref()).map_err(AppError::form)?;
    limits::check(tx, member.id, Action::Measurement, now).map_err(|l| l.into_error(now))?;
    let at = clock::storage(now);
    let id = match existing {
        Some(old) => {
            tx.execute(
                "UPDATE measurements SET eval_set = ?1, planted = ?2, kept = ?3, runs = ?4,
                     corrected_without = ?5, damaged_without = ?6, corrected_with = ?7,
                     damaged_with = ?8, hippo = ?9, model = ?10, report_url = ?11,
                     updated_at = ?12, revisions = revisions + 1
                 WHERE id = ?13",
                params![
                    new.eval_set,
                    new.planted,
                    new.kept,
                    new.runs,
                    new.corrected_without,
                    new.damaged_without,
                    new.corrected_with,
                    new.damaged_with,
                    new.hippo,
                    new.model,
                    new.report_url,
                    at,
                    old.id
                ],
            )?;
            old.id
        }
        None => {
            tx.execute(
                "INSERT INTO measurements (pack, pack_hash, user_id, eval_set, planted, kept,
                     runs, corrected_without, damaged_without, corrected_with, damaged_with,
                     hippo, model, report_url, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
                params![
                    new.pack,
                    new.pack_hash,
                    member.id.0,
                    new.eval_set,
                    new.planted,
                    new.kept,
                    new.runs,
                    new.corrected_without,
                    new.damaged_without,
                    new.corrected_with,
                    new.damaged_with,
                    new.hippo,
                    new.model,
                    new.report_url,
                    at
                ],
            )?;
            tx.last_insert_rowid()
        }
    };
    flag_large_benefit(tx, id, member, &new, now)?;
    Ok(id)
}

/// Files the `misleading` report and writes `evidence.flag` for a benefit of
/// 0.5 or more in either direction, unless a report is open on the
/// measurement. The comparison is exact, as the policy constant says in
/// floating point.
fn flag_large_benefit(
    tx: &Transaction,
    id: i64,
    member: &Member,
    new: &NewMeasurement,
    now: OffsetDateTime,
) -> rusqlite::Result<()> {
    let benefit = evidence::benefit(&new.counts());
    let half = Fraction::new(1, 2).expect("a half");
    let minus_half = Fraction::new(-1, 2).expect("minus a half");
    if benefit < half && benefit > minus_half {
        return Ok(());
    }
    let target = ReportTarget::Measurement(id);
    if reports::open_count(tx, target)? > 0 {
        return Ok(());
    }
    let rounded = (benefit.to_f64() * 1000.0).round() / 1000.0;
    let details = json!({ "member": member.id.0, "pack": new.pack, "benefit": rounded });
    let report = reports::file_system_report(tx, target, "misleading", details.clone(), now)?;
    let mut flag = details;
    flag["report"] = json!(report);
    audit::write(
        tx,
        now,
        None,
        audit::EVIDENCE_FLAG,
        Some(target.audit()),
        "misleading",
        false,
        flag,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posts::testkit::*;
    use time::macros::datetime;
    use time::Duration;

    const NOW: OffsetDateTime = datetime!(2026-11-08 10:00:00 UTC);
    const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    fn sets() -> Vec<EvalSet> {
        vec![EvalSet {
            id: "000000000000".into(),
            planted: 7,
            kept: 8,
        }]
    }

    fn facts() -> PackFacts {
        PackFacts {
            name: "glossary-terms".into(),
            passes: true,
            listed: true,
            post_visible: true,
            hashes: vec![H2.into(), H1.into()],
        }
    }

    fn form() -> MeasurementForm {
        MeasurementForm {
            pack_hash: H2.into(),
            eval_set: "000000000000".into(),
            hippo: "0.9.0".into(),
            model: "a-model".into(),
            runs: "3".into(),
            corrected_without: "6".into(),
            damaged_without: "1".into(),
            corrected_with: "10".into(),
            damaged_with: "1".into(),
            report_url: String::new(),
        }
    }

    fn refused(form: &MeasurementForm, field: &str) {
        let err = check(form, &facts(), &sets(), Standing::Established, None).expect_err("refused");
        assert_eq!(err.field, field, "{err:?}");
    }

    fn edit(f: impl FnOnce(&mut MeasurementForm)) -> MeasurementForm {
        let mut form = form();
        f(&mut form);
        form
    }

    #[test]
    fn a_good_form_passes_and_copies_the_set_size() {
        let new = check(&form(), &facts(), &sets(), Standing::Established, None).expect("ok");
        assert_eq!((new.planted, new.kept, new.runs), (7, 8, 3));
        assert_eq!(new.corrected_with, 10);
        assert_eq!(new.report_url, None);
    }

    #[test]
    fn the_pack_must_be_a_listed_pack_of_passes_with_a_visible_post() {
        for f in [
            |p: &mut PackFacts| p.passes = false,
            |p: &mut PackFacts| p.listed = false,
            |p: &mut PackFacts| p.post_visible = false,
        ] {
            let mut pack = facts();
            f(&mut pack);
            let err =
                check(&form(), &pack, &sets(), Standing::Established, None).expect_err("refused");
            assert_eq!(err.field, "pack");
        }
    }

    #[test]
    fn the_content_must_be_a_recorded_hash() {
        refused(&edit(|f| f.pack_hash = "nope".into()), "pack_hash");
        refused(&edit(|f| f.pack_hash = String::new()), "pack_hash");
        assert!(check(
            &edit(|f| f.pack_hash = H1.into()),
            &facts(),
            &sets(),
            Standing::Established,
            None
        )
        .is_ok());
    }

    #[test]
    fn the_eval_set_must_be_configured() {
        refused(&edit(|f| f.eval_set = "ffffffffffff".into()), "eval_set");
        let err = check(&form(), &facts(), &[], Standing::Established, None).expect_err("closed");
        assert_eq!(err.field, "eval_set");
        assert!(err.rule.contains("not open"));
    }

    #[test]
    fn runs_are_three_to_ten_whole_numbers() {
        for bad in ["2", "11", "", "x", "3.5", "-3"] {
            refused(&edit(|f| f.runs = bad.into()), "runs");
        }
        for good in ["3", "10"] {
            let f = edit(|f| {
                f.runs = good.into();
                f.corrected_without = "0".into();
                f.corrected_with = "0".into();
                f.damaged_without = "0".into();
                f.damaged_with = "0".into();
            });
            assert!(check(&f, &facts(), &sets(), Standing::Established, None).is_ok());
        }
    }

    #[test]
    fn counts_stay_within_planted_and_kept_times_runs() {
        // 7 planted and 8 kept, 3 runs: corrected up to 21, damaged up to 24.
        for (field, bad) in [
            ("corrected_without", "22"),
            ("corrected_with", "22"),
            ("damaged_without", "25"),
            ("damaged_with", "25"),
            ("corrected_with", "-1"),
            ("damaged_with", "x"),
            ("damaged_without", ""),
        ] {
            let f = edit(|f| match field {
                "corrected_without" => f.corrected_without = bad.into(),
                "corrected_with" => f.corrected_with = bad.into(),
                "damaged_without" => f.damaged_without = bad.into(),
                _ => f.damaged_with = bad.into(),
            });
            refused(&f, field);
        }
        let top = edit(|f| {
            f.corrected_with = "21".into();
            f.damaged_with = "24".into();
        });
        assert!(check(&top, &facts(), &sets(), Standing::Established, None).is_ok());
    }

    #[test]
    fn only_established_members_measure() {
        for standing in [Standing::New, Standing::Restricted, Standing::Banned] {
            let err = check(&form(), &facts(), &sets(), standing, None).expect_err("refused");
            assert_eq!(err.field, "member");
        }
    }

    #[test]
    fn the_hippo_version_has_its_pattern() {
        for good in ["0", "0.9.0", "1.2.3-rc.1+build", "A", &"a".repeat(64)] {
            let f = edit(|f| f.hippo = good.into());
            assert!(
                check(&f, &facts(), &sets(), Standing::Established, None).is_ok(),
                "{good}"
            );
        }
        for bad in ["", ".9", "-1", "1 2", "1_0", "é", &"a".repeat(65)] {
            refused(&edit(|f| f.hippo = bad.into()), "hippo");
        }
    }

    #[test]
    fn the_model_is_at_most_200_characters_without_control_characters() {
        refused(&edit(|f| f.model = String::new()), "model");
        refused(&edit(|f| f.model = "a\u{7}b".into()), "model");
        refused(&edit(|f| f.model = "a\nb".into()), "model");
        refused(&edit(|f| f.model = "m".repeat(201)), "model");
        assert!(check(
            &edit(|f| f.model = "é".repeat(200)),
            &facts(),
            &sets(),
            Standing::Established,
            None
        )
        .is_ok());
    }

    #[test]
    fn the_report_link_is_https_and_short() {
        let ok = edit(|f| f.report_url = "https://example.com/r?a=b".into());
        let new = check(&ok, &facts(), &sets(), Standing::Established, None).expect("ok");
        assert_eq!(new.report_url.as_deref(), Some("https://example.com/r?a=b"));
        for bad in [
            "http://example.com/r",
            "ftp://example.com/r",
            "example.com/r",
            "https://user:pw@example.com/r",
        ] {
            refused(&edit(|f| f.report_url = bad.into()), "report_url");
        }
        let long = format!("https://example.com/{}", "a".repeat(300));
        refused(&edit(|f| f.report_url = long), "report_url");
        let exact = format!("https://example.com/{}", "a".repeat(300 - 20));
        assert_eq!(exact.len(), 300);
        assert!(check(
            &edit(|f| f.report_url = exact),
            &facts(),
            &sets(),
            Standing::Established,
            None
        )
        .is_ok());
    }

    #[test]
    fn an_invalid_measurement_cannot_be_replaced() {
        let invalid = Existing {
            id: 1,
            revisions: 0,
            invalid: true,
        };
        let err = check(
            &form(),
            &facts(),
            &sets(),
            Standing::Established,
            Some(&invalid),
        )
        .expect_err("refused");
        assert!(err.rule.contains("invalid"), "{err:?}");
        let valid = Existing {
            invalid: false,
            ..invalid
        };
        assert!(check(
            &form(),
            &facts(),
            &sets(),
            Standing::Established,
            Some(&valid)
        )
        .is_ok());
    }

    struct World {
        _dir: tempfile::TempDir,
        conn: rusqlite::Connection,
        member: i64,
        maintainer: i64,
        newbie: i64,
    }

    /// A listed pack with two recorded contents, an established member, the
    /// pack's maintainer and a new member.
    fn world() -> World {
        let (dir, conn) = fresh();
        let old = NOW - Duration::days(120);
        let member = user(&conn, "grace", old, old);
        let maintainer = user(&conn, "ada", old, old);
        let newbie = user(&conn, "newbie", NOW - Duration::days(1), old);
        pack_post(&conn, "glossary-terms", 1002, "[\"ada\"]", old);
        for (hash, at) in [(H1, old), (H2, old + Duration::days(1))] {
            conn.execute(
                "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
                 VALUES ('glossary-terms', ?1, '1', 'c', ?2)",
                params![hash, clock::storage(at)],
            )
            .expect("content");
        }
        World {
            _dir: dir,
            conn,
            member,
            maintainer,
            newbie,
        }
    }

    fn enter_at(
        conn: &mut rusqlite::Connection,
        who: i64,
        form: &MeasurementForm,
        now: OffsetDateTime,
    ) -> Result<i64, AppError> {
        let member = member(conn, who);
        let tx = conn.transaction().expect("tx");
        let result = enter(&tx, &member, "glossary-terms", form, &sets(), now);
        if result.is_ok() {
            tx.commit().expect("commit");
        }
        result
    }

    fn one<T: rusqlite::types::FromSql>(w: &World, sql: &str) -> T {
        w.conn.query_row(sql, [], |r| r.get(0)).expect("value")
    }

    #[test]
    fn entering_stores_the_counts_and_the_set_size() {
        let mut w = world();
        let id = enter_at(&mut w.conn, w.member, &form(), NOW).expect("entered");
        let (planted, kept, revisions, created): (i64, i64, i64, String) = w
            .conn
            .query_row(
                "SELECT planted, kept, revisions, created_at FROM measurements WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .expect("row");
        assert_eq!((planted, kept, revisions), (7, 8, 0));
        assert_eq!(created, clock::storage(NOW));
    }

    #[test]
    fn a_maintainers_measurement_is_stored() {
        let mut w = world();
        enter_at(&mut w.conn, w.maintainer, &form(), NOW).expect("entered");
        assert_eq!(one::<i64>(&w, "SELECT COUNT(*) FROM measurements"), 1);
    }

    #[test]
    fn a_new_member_is_refused_and_nothing_is_stored() {
        let mut w = world();
        let err = enter_at(&mut w.conn, w.newbie, &form(), NOW).expect_err("refused");
        assert_eq!(err.form.expect("form").field, "member");
        assert_eq!(one::<i64>(&w, "SELECT COUNT(*) FROM measurements"), 0);
    }

    #[test]
    fn an_unknown_pack_is_404() {
        let mut w = world();
        let member = member(&w.conn, w.member);
        let tx = w.conn.transaction().expect("tx");
        let err = enter(&tx, &member, "nope", &form(), &sets(), NOW).expect_err("404");
        assert_eq!(err.status, axum::http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn replacing_keeps_the_id_and_first_entry_and_adds_a_revision() {
        let mut w = world();
        let first = enter_at(&mut w.conn, w.member, &form(), NOW).expect("entered");
        let later = NOW + Duration::hours(3);
        let again = edit(|f| f.corrected_with = "12".into());
        let second = enter_at(&mut w.conn, w.member, &again, later).expect("replaced");
        assert_eq!(first, second);
        assert_eq!(one::<i64>(&w, "SELECT COUNT(*) FROM measurements"), 1);
        let (created, updated, revisions, corrected): (String, String, i64, i64) = w
            .conn
            .query_row(
                "SELECT created_at, updated_at, revisions, corrected_with FROM measurements",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .expect("row");
        assert_eq!(created, clock::storage(NOW));
        assert_eq!(updated, clock::storage(later));
        assert_eq!((revisions, corrected), (1, 12));
        enter_at(&mut w.conn, w.member, &again, later).expect("replaced again");
        assert_eq!(one::<i64>(&w, "SELECT revisions FROM measurements"), 2);
        // Another content of the same pack is a new measurement.
        let other = edit(|f| f.pack_hash = H1.into());
        let third = enter_at(&mut w.conn, w.member, &other, later).expect("other content");
        assert_ne!(third, first);
    }

    #[test]
    fn an_invalid_measurement_is_not_replaced() {
        let mut w = world();
        enter_at(&mut w.conn, w.member, &form(), NOW).expect("entered");
        w.conn
            .execute("UPDATE measurements SET invalid = 1", [])
            .expect("mark");
        let err =
            enter_at(&mut w.conn, w.member, &form(), NOW + Duration::hours(1)).expect_err("no");
        assert!(err.form.expect("form").rule.contains("invalid"));
        assert_eq!(one::<i64>(&w, "SELECT revisions FROM measurements"), 0);
    }

    #[test]
    fn a_large_benefit_files_one_report_while_one_is_open() {
        let mut w = world();
        // Benefit: 11 / (7 * 3) = 0.52.
        let big = edit(|f| {
            f.corrected_without = "0".into();
            f.damaged_without = "0".into();
            f.corrected_with = "11".into();
            f.damaged_with = "0".into();
        });
        let id = enter_at(&mut w.conn, w.member, &big, NOW).expect("entered");
        let reports = |w: &World| {
            one::<i64>(
                w,
                &format!(
                    "SELECT COUNT(*) FROM reports WHERE system = 1 AND reason = 'misleading'
                     AND target_type = 'measurement' AND target_id = {id} AND status = 'open'"
                ),
            )
        };
        let flags = |w: &World| {
            one::<i64>(
                w,
                "SELECT COUNT(*) FROM audit_log WHERE action = 'evidence.flag' AND public = 0",
            )
        };
        assert_eq!((reports(&w), flags(&w)), (1, 1));
        // Replacing it while the report is open files no second.
        let again = edit(|f| {
            *f = big.clone();
            f.corrected_with = "13".into();
        });
        enter_at(&mut w.conn, w.member, &again, NOW + Duration::hours(1)).expect("replaced");
        assert_eq!((reports(&w), flags(&w)), (1, 1));
        // Once it is resolved, a new large benefit files a new one.
        w.conn
            .execute("UPDATE reports SET status = 'dismissed'", [])
            .expect("resolve");
        enter_at(&mut w.conn, w.member, &big, NOW + Duration::hours(2)).expect("replaced");
        assert_eq!(reports(&w), 1);
        assert_eq!(
            one::<i64>(
                &w,
                "SELECT COUNT(*) FROM reports WHERE reason = 'misleading'"
            ),
            2
        );
    }

    #[test]
    fn a_strongly_harmful_benefit_is_flagged_too_and_a_small_one_is_not() {
        let mut w = world();
        // Benefit: -21 / (7 * 3) = -1.
        let harmful = edit(|f| {
            f.corrected_without = "21".into();
            f.corrected_with = "0".into();
        });
        enter_at(&mut w.conn, w.member, &harmful, NOW).expect("entered");
        assert_eq!(one::<i64>(&w, "SELECT COUNT(*) FROM reports"), 1);
        // With 4 runs of 7 planted errors, 14 more corrections is exactly 0.5
        // (flagged), and 13 is under it.
        for (corrected, reports) in [("13", 0), ("14", 1)] {
            let mut w = world();
            let at = edit(|f| {
                f.runs = "4".into();
                f.corrected_without = "0".into();
                f.damaged_without = "0".into();
                f.corrected_with = corrected.into();
                f.damaged_with = "0".into();
            });
            enter_at(&mut w.conn, w.member, &at, NOW).expect("entered");
            assert_eq!(one::<i64>(&w, "SELECT COUNT(*) FROM reports"), reports);
        }
    }

    #[test]
    fn the_per_member_limit_applies() {
        let mut w = world();
        for i in 0..20 {
            // Distinct contents are needed: add them.
            let hash = format!("{i:064x}");
            w.conn
                .execute(
                    "INSERT INTO pack_contents (pack, pack_hash, version, commit_id, first_seen)
                     VALUES ('glossary-terms', ?1, '1', 'c', ?2)",
                    params![hash, clock::storage(NOW)],
                )
                .expect("content");
            let f = edit(|f| f.pack_hash = hash.clone());
            enter_at(&mut w.conn, w.member, &f, NOW + Duration::minutes(i))
                .expect("within the limit");
        }
        let err = enter_at(&mut w.conn, w.member, &form(), NOW + Duration::minutes(30))
            .expect_err("limited");
        assert_eq!(err.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }
}

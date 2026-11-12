//! `GET /p/<id>/measure` and `POST /p/<id>/measure`: a member enters a
//! measurement of a pack.
//!
//! The page explains the runs, lists the configured eval sets and the pack's
//! recorded contents (newest first), and says measurements are public. A form
//! that fails a rule is shown again with the values entered, status 422; over
//! the per-member limit the answer is 429. Success redirects to the
//! measurement's row on the pack's page.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::{html, Markup};
use rusqlite::OptionalExtension;

use crate::auth::session::{CsrfForm, RequireUser};
use crate::config::EvalSet;
use crate::db::Member;
use crate::error::{AppError, FormError};
use crate::evidence;
use crate::measurements::{self, MeasurementForm};
use crate::web::{AppState, Ctx};

/// What the page shows about the pack.
struct Target {
    post_id: i64,
    name: String,
    /// Listed pack of passes with a visible post: the form can be used.
    open: bool,
    /// `(pack hash, version, current)`, newest first.
    contents: Vec<(String, String, bool)>,
}

fn parse_id(id: &str) -> Result<i64, AppError> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::not_found());
    }
    id.parse().map_err(|_| AppError::not_found())
}

fn load(
    conn: &rusqlite::Connection,
    post_id: i64,
    now: time::OffsetDateTime,
) -> Result<Target, AppError> {
    let found: Option<(String, String, bool, String)> = conn
        .query_row(
            "SELECT k.name, k.kind, k.listed, p.state FROM posts p
             JOIN packs k ON k.name = p.pack WHERE p.id = ?1",
            [post_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (name, kind, listed, state) = found.ok_or_else(AppError::not_found)?;
    let contents = evidence::contents(conn, &name, now)?
        .into_iter()
        .map(|c| (c.pack_hash, c.version, c.current))
        .collect();
    Ok(Target {
        post_id,
        name,
        open: kind == "passes" && listed && state == "visible",
        contents,
    })
}

fn explain() -> Markup {
    html! {
        p {
            "A measurement compares a pack with and without it. Run "
            code { "hippocamp eval" } "'s dreaming suite on one of the eval sets below 3 times without the pack and 3 times with it, up to 10 times each, "
            "and enter the counts summed over the runs: the planted errors corrected, and the kept claims damaged, "
            "without the pack and with it."
        }
        p {
            "Each corrected count is between 0 and planted \u{d7} runs, and each damaged count between 0 and kept \u{d7} runs. "
            "A pack's evidence needs 3 counted measurements of the same content."
        }
        p class="hp-help" {
            "Measurements are public: your login, the numbers and the report link are listed on the pack's page. "
            "Only established accounts can measure, and a pack's maintainers' measurements are listed but not counted. "
            "Measuring the same version again replaces your earlier entry, and the page shows how many times it changed."
        }
    }
}

fn eval_sets(sets: &[EvalSet]) -> Markup {
    html! {
        h2 { "Eval sets" }
        table class="hp-table" {
            thead {
                tr {
                    th scope="col" { "Eval set" }
                    th scope="col" { "Planted errors" }
                    th scope="col" { "Kept claims" }
                }
            }
            tbody {
                @for set in sets {
                    tr {
                        td { code { (set.id) } }
                        td { (set.planted) }
                        td { (set.kept) }
                    }
                }
            }
        }
    }
}

fn number_field(id: &str, label: &str, value: &str) -> Markup {
    html! {
        div class="hp-field" {
            label class="hp-label" for=(id) { (label) }
            input class="hp-input" id=(id) name=(id) type="text" inputmode="numeric"
                value=(value) autocomplete="off" required;
        }
    }
}

fn form(member: &Member, target: &Target, sets: &[EvalSet], values: &MeasurementForm) -> Markup {
    let selected_hash = if values.pack_hash.is_empty() {
        target.contents.first().map(|c| c.0.as_str()).unwrap_or("")
    } else {
        values.pack_hash.as_str()
    };
    html! {
        form class="hp-form" method="post" action=(format!("/p/{}/measure", target.post_id)) {
            input type="hidden" name="csrf" value=(member.csrf);
            div class="hp-field" {
                label class="hp-label" for="pack_hash" { "Version measured" }
                select class="hp-select" id="pack_hash" name="pack_hash" {
                    @for (hash, version, current) in &target.contents {
                        option value=(hash) selected[hash == selected_hash] {
                            (version) " (" (hash.chars().take(12).collect::<String>()) ")"
                            @if *current { " current" }
                        }
                    }
                }
            }
            div class="hp-field" {
                label class="hp-label" for="eval_set" { "Eval set" }
                select class="hp-select" id="eval_set" name="eval_set" {
                    @for set in sets {
                        option value=(set.id) selected[set.id == values.eval_set] { (set.id) }
                    }
                }
            }
            div class="hp-field" {
                label class="hp-label" for="hippocamp" { "hippocamp version" }
                input class="hp-input" id="hippocamp" name="hippocamp" type="text" value=(values.hippocamp)
                    autocomplete="off" required;
            }
            div class="hp-field" {
                label class="hp-label" for="model" { "Model" }
                input class="hp-input" id="model" name="model" type="text" value=(values.model)
                    autocomplete="off" required;
            }
            (number_field("runs", "Runs of each (3 to 10)", &values.runs))
            (number_field("corrected_without", "Planted errors corrected, without the pack", &values.corrected_without))
            (number_field("damaged_without", "Kept claims damaged, without the pack", &values.damaged_without))
            (number_field("corrected_with", "Planted errors corrected, with the pack", &values.corrected_with))
            (number_field("damaged_with", "Kept claims damaged, with the pack", &values.damaged_with))
            div class="hp-field" {
                label class="hp-label" for="report_url" { "Report link (optional, https)" }
                input class="hp-input" id="report_url" name="report_url" type="text"
                    value=(values.report_url) autocomplete="off";
            }
            button class="hp-button" type="submit" { "Enter measurement" }
        }
    }
}

fn page(
    ctx: &Ctx,
    member: &Member,
    target: &Target,
    sets: &[EvalSet],
    values: &MeasurementForm,
    error: Option<&FormError>,
) -> Response {
    let body = html! {
        p { a href=(format!("/p/{}", target.post_id)) { "Back to " (target.name) } }
        (explain())
        @if let Some(e) = error {
            p class="hp-error" { (e.field) ": " (e.rule) }
        }
        @if sets.is_empty() {
            p class="hp-notice" { "Measuring is not open yet: no eval set is configured." }
        } @else if !target.open {
            p class="hp-notice" { "This pack cannot be measured: it is not a listed pack of passes shown in the feed." }
        } @else {
            (eval_sets(sets))
            (form(member, target, sets, values))
        }
    };
    ctx.page("measure", "Measure").body(body).into_response()
}

/// `GET /p/<id>/measure`.
pub async fn show(
    State(state): State<AppState>,
    ctx: Ctx,
    RequireUser(member): RequireUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let id = parse_id(&id)?;
    let now = state.clock.now();
    let target = state.db.read(move |conn| load(conn, id, now)).await?;
    Ok(page(
        &ctx,
        &member,
        &target,
        &state.config.eval_sets,
        &MeasurementForm::default(),
        None,
    ))
}

/// `POST /p/<id>/measure`.
pub async fn enter(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    CsrfForm(values, member): CsrfForm<MeasurementForm>,
) -> Result<Response, AppError> {
    let id = parse_id(&id)?;
    let now = state.clock.now();
    let sets = state.config.eval_sets.clone();
    let who = member.clone();
    let form_values = values.clone();
    let outcome = state
        .db
        .write(move |tx| {
            let name: Option<String> = tx
                .query_row("SELECT pack FROM posts WHERE id = ?1", [id], |r| r.get(0))
                .optional()?
                .flatten();
            let name = name.ok_or_else(AppError::not_found)?;
            match measurements::enter(tx, &who, &name, &form_values, &sets, now) {
                Ok(measurement) => Ok(Ok(measurement)),
                Err(e) if e.form.is_some() => Ok(Err(e)),
                Err(e) => Err(e),
            }
        })
        .await?;
    match outcome {
        Ok(measurement) => Ok(Redirect::to(&format!("/p/{id}#m{measurement}")).into_response()),
        Err(e) => {
            let error = e.form.unwrap_or_else(|| FormError::new("form", e.message));
            let target = state.db.read(move |conn| load(conn, id, now)).await?;
            let mut response = page(
                &ctx,
                &member,
                &target,
                &state.config.eval_sets,
                &values,
                Some(&error),
            );
            *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            Ok(response)
        }
    }
}

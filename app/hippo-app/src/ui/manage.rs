//! Manage: the settings there are today (the theme, written answers in
//! Ask, text size), hippo's health, and the rest of what Manage holds.

use gpui::{div, prelude::*};

use super::Shell;
use super::kit;
use super::theme::Look;

impl Shell {
    /// hippo's health in plain words: the warning lines agents get, with
    /// what to run, and the rest of what the count stands for.
    pub(super) fn health_lines(&self) -> Vec<(String, Option<String>, bool)> {
        let Some(home) = self.loaded.as_ref().and_then(|l| l.home.as_ref()) else {
            return Vec::new();
        };
        let dream_at = self
            .loaded
            .as_ref()
            .and_then(|l| l.panels.as_ref())
            .map(|p| p.dream_at.clone())
            .unwrap_or_else(|| "03:30".into());
        let mut out: Vec<(String, Option<String>, bool)> = home
            .health_warnings
            .iter()
            .map(|w| {
                let (text, cmd) =
                    super::status::explain_warning(w, &dream_at, super::status::local_offset());
                (text, cmd, true)
            })
            .collect();
        out.extend(home.health_notes.iter().map(|n| (n.clone(), None, false)));
        // The states Manage's cards show.
        for g in self
            .loaded
            .as_ref()
            .map(|l| l.gaps.clone())
            .unwrap_or_default()
        {
            let (text, cmd) = match g.as_str() {
                "ingest is off" => (
                    "The ingest job is off: new sessions are not read.",
                    Some("hippo schedule install --job ingest"),
                ),
                "dreaming is off" => (
                    "The dreaming job is off: nothing is dreamed overnight.",
                    Some("hippo schedule install --job dream"),
                ),
                _ => (
                    "The session-start hook is not installed in hippo's workspace: agents there start without memory.",
                    Some("hippo hooks install"),
                ),
            };
            out.insert(0, (text.to_owned(), cmd.map(str::to_owned), true));
        }
        out
    }

    /// `hippo ok`, or how many things to look at.
    pub(crate) fn health_word(&self) -> (String, bool) {
        let Some(home) = self.loaded.as_ref().and_then(|l| l.home.as_ref()) else {
            return ("hippo".into(), true);
        };
        // The reason, from the same states Manage shows.
        if let Some(g) = self.loaded.as_ref().and_then(|l| l.gaps.first()) {
            return (g.clone(), false);
        }
        let stale = self
            .loaded
            .as_ref()
            .and_then(|l| l.status.as_ref())
            .filter(|s| s.ingest_stale)
            .and_then(|s| s.last_ingest);
        if let Some(t) = stale {
            return (
                format!(
                    "last ingest {} ago",
                    kit::span(time::OffsetDateTime::now_utc() - t)
                ),
                false,
            );
        }
        if home.health_ok {
            ("hippo ok".into(), true)
        } else {
            let n = home.health_warnings.len() + home.health_notes.len();
            (
                format!("{n} {}", if n == 1 { "warning" } else { "warnings" }),
                false,
            )
        }
    }

    /// The health lines as a list, for the title bar's popover and Manage.
    pub(super) fn render_health(&self, look: &Look) -> gpui::Div {
        let lines = self.health_lines();
        let mut d = div().flex().flex_col().gap(look.px(8.));
        if lines.is_empty() {
            d = d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.px(8.))
                    .child(kit::dot(look, kit::ok(look)))
                    .child("Nothing to fix: ingest, dreaming and the hooks are running."),
            );
        }
        for (text, cmd, warn) in lines {
            d = d.child(
                div()
                    .flex()
                    .items_start()
                    .gap(look.px(8.))
                    .child(div().pt(look.px(5.)).child(kit::dot(
                        look,
                        if warn {
                            look.c.warning
                        } else {
                            look.c.text_muted
                        },
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_color(look.c.text).child(text))
                            .children(cmd.map(|c| {
                                div()
                                    .font_family("Menlo")
                                    .text_size(look.text(11.5))
                                    .text_color(kit::dim(look))
                                    .child(c)
                            })),
                    ),
            );
        }
        d.child(
            div()
                .text_size(look.text(12.))
                .text_color(kit::dim(look))
                .child("hippo doctor checks more."),
        )
    }
}

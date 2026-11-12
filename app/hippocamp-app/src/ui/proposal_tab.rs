//! A dreaming change's tab (its old and new text, reason, receipts, evidence
//! turns and the Revert button), a run's report tab, and the revert
//! confirmation.
//! and "Writing through the CLI".

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{Context, Hsla, Role, Window, div, prelude::*};
use hippocamp_core::dream::{self, History, RevertState};
use hippocamp_core::index::Index;
use hippocamp_core::records::{ProposalKind, Verdict};
use hippocamp_core::theme::{self as core_theme, Appearance};
use hippocamp_view::dreaming::{self, ChangeView};
use hippocamp_view::text::visible;
use ulid::Ulid;

use super::Shell;
use super::theme::{ActiveTheme, Look, color};
use crate::cli::{self, Output};
use crate::rows;
use crate::state::Tab;

/// What the Revert control offers for a change's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevertOffer {
    /// A button: `force` for Revert anyway.
    Button { label: &'static str, force: bool },
    /// No button, only this line.
    Line(String),
}

/// The control for a state, as "Writing through the CLI" lists them.
pub fn offer(state: &RevertState) -> RevertOffer {
    match state {
        RevertState::Revertible => RevertOffer::Button {
            label: "Revert",
            force: false,
        },
        RevertState::NoteChanged => RevertOffer::Button {
            label: "Revert anyway",
            force: true,
        },
        RevertState::NewerChange(r) => {
            RevertOffer::Line(format!("Revert the newer change first (receipt {r})."))
        }
        RevertState::Reverted(r) => RevertOffer::Line(format!("Reverted by {r}.")),
        RevertState::NoteMissing => RevertOffer::Line("The note is missing.".into()),
        RevertState::SnapshotMissing => RevertOffer::Line(
            "The snapshot is missing or changed, so it cannot be restored.".into(),
        ),
    }
}

/// The revert confirmation, while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub proposal: Ulid,
    pub receipt: Ulid,
    pub note: PathBuf,
    pub old: String,
    pub new: String,
    /// The state the confirmation was drawn for; checked again on confirm.
    pub state: RevertState,
}

/// The last revert's result, shown at the bottom of its proposal's tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertResult {
    pub proposal: Ulid,
    pub text: String,
    pub ok: bool,
}

/// The Dreaming tabs' state.
#[derive(Default)]
pub struct DreamUi {
    pub views: HashMap<Ulid, Result<ChangeView, String>>,
    pub loading: std::collections::HashSet<Ulid>,
    /// Report files read since the last reload, by path.
    pub reports: HashMap<PathBuf, Result<String, String>>,
    /// Run tabs whose report file is shown in full.
    pub raw_open: std::collections::HashSet<PathBuf>,
    pub confirm: Option<Confirmation>,
    pub result: Option<RevertResult>,
}

/// The change's verdict in words, the reviewer's notes, and whether it is
/// a refusal: `Confirmed by the reviewer and applied 08 Nov`.
pub fn verdict_line(change: &dream::Change) -> Option<(String, String, bool)> {
    let reverted = change
        .receipts
        .iter()
        .find(|r| r.verdict == Verdict::Revert);
    let last = change
        .receipts
        .iter()
        .rev()
        .find(|r| r.verdict != Verdict::Revert)?;
    let notes = visible(last.notes.trim());
    let (line, bad) = match (last.verdict, last.applied, reverted) {
        (Verdict::Confirm, Some(t), None) => (
            format!("Confirmed by the reviewer and applied {}", t.date()),
            false,
        ),
        (Verdict::Confirm, Some(_), Some(r)) => (
            format!("Confirmed and applied, then reverted {}", r.at.date()),
            false,
        ),
        (Verdict::Confirm, None, _) => (
            "Confirmed, not applied: the note changed before it could be written".into(),
            false,
        ),
        _ => ("Rejected by the reviewer: nothing was written".into(), true),
    };
    Some((line, notes, bad))
}

/// How a finished `hippocamp revert` reads.
pub fn describe(out: &std::io::Result<Output>) -> (String, bool) {
    match out {
        Err(e) => (format!("hippocamp could not start: {e}"), false),
        Ok(o) if o.timed_out => (
            "hippocamp revert did not finish in 60 seconds".into(),
            false,
        ),
        Ok(o) => {
            let mut text = String::new();
            if !o.stdout.trim().is_empty() {
                text.push_str(o.stdout.trim());
                text.push('\n');
            }
            if !o.stderr.trim().is_empty() {
                text.push_str(o.stderr.trim());
                text.push('\n');
            }
            text.push_str(&match o.code {
                Some(c) => format!("exit code {c}"),
                None => "stopped by a signal".into(),
            });
            (text, o.success())
        }
    }
}

/// The built-in theme's removed and added colours for an appearance: the
/// confirmation uses them whatever the theme says, so no theme hides what a
/// revert does.
fn builtin_diff_colors(appearance: Appearance) -> [Hsla; 4] {
    let [light, dark] = core_theme::builtin();
    let t = match appearance {
        Appearance::Light => light,
        Appearance::Dark => dark,
    };
    [
        color(t.colors.deleted),
        color(t.colors.deleted_background),
        color(t.colors.created),
        color(t.colors.created_background),
    ]
}

impl Shell {
    fn ready_parts(&self) -> Option<(hippocamp_core::config::Workspace, PathBuf, History)> {
        let loaded = self.loaded.as_ref()?;
        let hippocamp_view::setup::Setup::Ready { cfg, ws } = &loaded.setup else {
            return None;
        };
        let history = loaded.panels.as_ref()?.history.as_ref().ok()?.clone();
        Some((ws.clone(), cfg.home.clone(), history))
    }

    /// Reads a change's view (evidence turns, revert state) in the background.
    pub(crate) fn load_change(&mut self, id: Ulid, cx: &mut Context<Self>) {
        if self.dream_ui.views.contains_key(&id) || self.dream_ui.loading.contains(&id) {
            return;
        }
        let Some((ws, home, history)) = self.ready_parts() else {
            return;
        };
        self.dream_ui.loading.insert(id);
        cx.spawn(async move |this, cx| {
            let view = cx
                .background_executor()
                .spawn(async move {
                    let index = Index::open(&home, false).ok();
                    dreaming::view(&ws, &history, index.as_ref(), id).map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.dream_ui.loading.remove(&id);
                this.dream_ui.views.insert(id, view);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A run's report file, read once per reload.
    pub(super) fn load_report(&mut self, path: &std::path::Path) {
        if self.dream_ui.reports.contains_key(path) {
            return;
        }
        let read = std::fs::read(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| format!("{}: {e}", path.display()));
        self.dream_ui.reports.insert(path.to_path_buf(), read);
    }

    /// Whether the active tab is a change that can be reverted now.
    pub(crate) fn revert_ready(&self) -> bool {
        matches!(
            self.active_change_offer(),
            Some((_, RevertOffer::Button { .. }))
        ) && self.revert_running.is_none()
    }

    fn active_change_offer(&self) -> Option<(&ChangeView, RevertOffer)> {
        let Some(Tab::Proposal(id)) = self.state.tabs.active_tab() else {
            return None;
        };
        let view = self.dream_ui.views.get(id)?.as_ref().ok()?;
        Some((view, offer(view.revert.as_ref()?)))
    }

    /// Command-Option-Z or the Revert button: opens the confirmation.
    pub(crate) fn ask_revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((view, RevertOffer::Button { .. })) = self.active_change_offer() else {
            return;
        };
        let Ok(p) = view.change.proposal.record.as_ref() else {
            return;
        };
        let Some(receipt) =
            view.change.receipts.iter().find(|r| {
                r.verdict == Verdict::Confirm && r.applied.is_some() && r.snapshot.is_some()
            })
        else {
            return;
        };
        let Some(state) = view.revert else {
            return;
        };
        self.dream_ui.confirm = Some(Confirmation {
            proposal: p.id,
            receipt: receipt.id,
            note: p.note.clone(),
            old: p.old.clone(),
            new: p.new.clone(),
            state,
        });
        window.focus(&self.confirm_focus, cx);
        cx.notify();
    }

    pub(crate) fn cancel_revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dream_ui.confirm.take().is_some() {
            window.focus(&self.pane_focus, cx);
            cx.notify();
        }
    }

    /// Command-Return or a click on Revert: asks `revert_state` again and
    /// runs `hippocamp revert` only if nothing changed since the confirmation was
    /// drawn.
    pub(crate) fn confirm_revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.dream_ui.confirm.take() else {
            return;
        };
        window.focus(&self.pane_focus, cx);
        let Some((ws, _, _)) = self.ready_parts() else {
            return;
        };
        let now = dream::revert_state(&ws, c.receipt).map_err(|e| e.to_string());
        if now.as_ref() != Ok(&c.state) {
            // The state changed: redraw with the new one, run nothing.
            self.dream_ui.views.remove(&c.proposal);
            self.dream_ui.result = Some(RevertResult {
                proposal: c.proposal,
                text: "The change's state changed since the confirmation opened; nothing was run."
                    .into(),
                ok: false,
            });
            self.reload(cx);
            cx.notify();
            return;
        }
        let RevertOffer::Button { force, .. } = offer(&c.state) else {
            return;
        };
        let installation = self.installation.clone();
        let args = cli::revert_args(&c.receipt.to_string(), force);
        let root = ws.root.clone();
        self.revert_running = Some(format!(
            "reverting {}",
            visible(&c.note.display().to_string())
        ));
        self.dream_ui.result = None;
        let proposal = c.proposal;
        cx.spawn(async move |this, cx| {
            let out = cx
                .background_executor()
                .spawn(async move {
                    cli::run(
                        &installation.binary,
                        &installation.home,
                        &root,
                        &args,
                        cli::Limits::commands(),
                    )
                })
                .await;
            let (text, ok) = describe(&out);
            this.update(cx, |this, cx| {
                this.revert_running = None;
                this.dream_ui.result = Some(RevertResult { proposal, text, ok });
                this.reload(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn diff_block(
        label: &'static str,
        text: &str,
        fg: Hsla,
        bg: Hsla,
        look: &Look,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(look.px(2.))
            .child(
                div()
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child(label),
            )
            .child(
                div()
                    .p(look.px(8.))
                    .rounded(look.px(4.))
                    .bg(bg)
                    .text_color(fg)
                    .font_family("Menlo")
                    .text_size(look.text(12.))
                    .line_height(gpui::relative(1.45))
                    .child(visible(text)),
            )
    }

    pub(super) fn render_proposal_tab(
        &mut self,
        id: Ulid,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.load_change(id, cx);
        let column = div()
            .id("proposal-tab")
            .role(Role::Article)
            .flex()
            .flex_col()
            .w_full()
            .max_w(look.px(720.))
            .p(look.px(16.))
            .gap(look.px(6.))
            .text_size(look.text(13.));
        let view = match self.dream_ui.views.get(&id) {
            None => {
                return column
                    .child(
                        div()
                            .text_color(look.c.text_muted)
                            .child("Reading the change…"),
                    )
                    .into_any_element();
            }
            Some(Err(e)) => {
                return column
                    .child(div().text_color(look.c.error).child(e.clone()))
                    .into_any_element();
            }
            Some(Ok(v)) => v.clone(),
        };
        let title = self.tab_title(&Tab::Proposal(id));
        let (_, meta) = rows::change_text(&view.change);
        let colors = &cx.global::<ActiveTheme>().theme.colors;
        let (del, del_bg, add, add_bg) = (
            color(colors.deleted),
            color(colors.deleted_background),
            color(colors.created),
            color(colors.created_background),
        );

        let mut column = column
            .child(
                div()
                    .text_size(look.text(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .text_size(look.text(11.))
                    .text_color(look.c.text_muted)
                    .child(format!("proposal {id} · {meta}")),
            );

        // The verdict first: is it right, and was it written.
        if let Some((line, notes, bad)) = verdict_line(&view.change) {
            column = column.child(
                div()
                    .pt(look.px(6.))
                    .flex()
                    .flex_col()
                    .gap(look.px(2.))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(if bad { look.c.error } else { look.c.text })
                            .child(line),
                    )
                    .when(!notes.is_empty(), |d| {
                        d.child(
                            div()
                                .text_color(look.c.text_muted)
                                .child(format!("Reviewer: {notes}")),
                        )
                    }),
            );
        }

        // The Revert control.
        if let Some(state) = &view.revert {
            column = column.child(div().h(look.px(6.)));
            column = match offer(state) {
                RevertOffer::Button { label, force } => column.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.px(8.))
                        .child(
                            div()
                                .id("revert-button")
                                .role(Role::Button)
                                .aria_label(label)
                                .px(look.px(10.))
                                .py(look.px(3.))
                                .rounded(look.px(4.))
                                .border(look.fixed(1.))
                                .border_color(if force { look.c.warning } else { look.c.border })
                                .text_color(if force { look.c.warning } else { look.c.text })
                                .cursor_pointer()
                                .hover(|h| h.bg(look.c.hover))
                                .when(self.revert_running.is_some(), |d| d.opacity(0.5))
                                .child(label)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if this.revert_ready() {
                                        this.ask_revert(window, cx);
                                    }
                                })),
                        )
                        .child(
                            div()
                                .text_size(look.text(11.))
                                .text_color(look.c.text_muted)
                                .child(if force {
                                    "The note changed after this change; reverting replaces its current text with the snapshot. ⌘⌥Z"
                                } else {
                                    "Restores the note's text from before this change. ⌘⌥Z"
                                }),
                        ),
                ),
                RevertOffer::Line(line) => {
                    column.child(div().text_color(look.c.text_muted).child(line))
                }
            };
        }

        match &view.change.proposal.record {
            Err(e) => {
                column = column.child(div().text_color(look.c.error).child(visible(e)));
            }
            Ok(p) => {
                let note = Tab::Note(p.note.clone());
                column = column
                    .child(
                        div()
                            .id("proposal-note")
                            .role(Role::Link)
                            .cursor_pointer()
                            .text_color(look.c.text_accent)
                            .child(visible(&p.note.display().to_string()))
                            .on_click(cx.listener(
                                move |this, e: &gpui::ClickEvent, window, cx| {
                                    this.state.tabs.follow(note.clone(), e.click_count() >= 2);
                                    this.after_state_change(window, cx);
                                },
                            )),
                    )
                    .child(
                        div()
                            .pt(look.px(4.))
                            .child(format!("Why dreaming proposed it: {}", visible(&p.reason))),
                    );
                // Only an applied change removed and added anything.
                let applied = view.change.receipts.iter().any(|r| r.applied.is_some());
                let (removed, added) = if applied {
                    ("Removed", "Added")
                } else {
                    ("Proposed to remove", "Proposed to add")
                };
                if p.kind == ProposalKind::Replace {
                    column = column.child(Self::diff_block(removed, &p.old, del, del_bg, look));
                }
                column = column.child(Self::diff_block(added, &p.new, add, add_bg, look));
            }
        }

        // The receipts.
        if !view.change.receipts.is_empty() {
            column = column.child(Self::section_heading(look, "Receipts"));
            for r in &view.change.receipts {
                let verdict = match r.verdict {
                    Verdict::Confirm => "confirm",
                    Verdict::Reject => "reject",
                    Verdict::Revert => "revert",
                };
                let applied = r
                    .applied
                    .map(|t| format!("applied {}", t.date()))
                    .unwrap_or_else(|| "not applied".into());
                column = column.child(
                    div()
                        .flex()
                        .flex_col()
                        .py(look.px(4.))
                        .child(
                            div()
                                .child(format!("{verdict} · {} · {applied}", visible(&r.reviewer))),
                        )
                        .child(
                            div()
                                .text_size(look.text(11.))
                                .text_color(look.c.text_muted)
                                .child(format!("receipt {} · {}", r.id, r.at.date())),
                        )
                        .when(!r.notes.trim().is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(look.text(12.))
                                    .text_color(look.c.text_muted)
                                    .child(visible(r.notes.trim())),
                            )
                        }),
                );
            }
        } else {
            column = column
                .child(Self::section_heading(look, "Receipts"))
                .child(div().text_color(look.c.text_muted).child("No receipt."));
        }

        // The evidence turns.
        if !view.evidence.is_empty() {
            column = column.child(Self::section_heading(look, "Evidence"));
            for (i, ev) in view.evidence.iter().enumerate() {
                column = match ev {
                    Ok(t) => {
                        let tab = Tab::Session {
                            id: t.session.clone(),
                            ordinal: Some(t.ordinal),
                        };
                        let target = (t.session.clone(), t.ordinal);
                        column.child(
                            div()
                                .id(("evidence", i))
                                .role(Role::Link)
                                .flex()
                                .flex_col()
                                .py(look.px(4.))
                                .px(look.px(8.))
                                .mx(look.px(-8.))
                                .rounded(look.px(4.))
                                .cursor_pointer()
                                .hover(|h| h.bg(look.c.hover))
                                .child(
                                    div()
                                        .text_size(look.text(11.))
                                        .text_color(look.c.text_accent)
                                        .child(format!(
                                            "{} · turn {} · {} · open in its session ›",
                                            visible(&t.session),
                                            t.ordinal,
                                            visible(&t.role)
                                        )),
                                )
                                .child(div().child(visible(t.text.trim())))
                                .on_click(cx.listener(
                                    move |this, e: &gpui::ClickEvent, window, cx| {
                                        this.sessions_ui.scroll_to = Some(target.clone());
                                        this.state.tabs.follow(tab.clone(), e.click_count() >= 2);
                                        this.after_state_change(window, cx);
                                    },
                                )),
                        )
                    }
                    Err((session, ordinal)) => column.child(
                        div()
                            .text_color(look.c.text_muted)
                            .child(format!("Turn {session} {ordinal} is not in the index")),
                    ),
                };
            }
        }

        // The last revert's output.
        if let Some(r) = self.dream_ui.result.as_ref().filter(|r| r.proposal == id) {
            column = column
                .child(Self::section_heading(look, "hippocamp revert"))
                .child(
                    div()
                        .p(look.px(8.))
                        .rounded(look.px(4.))
                        .bg(look.c.hover)
                        .font_family("Menlo")
                        .text_size(look.text(12.))
                        .text_color(if r.ok { look.c.text } else { look.c.error })
                        .child(visible(&r.text)),
                );
        }
        column.into_any_element()
    }

    /// The confirmation over the window, while it is open.
    pub(super) fn render_confirmation(
        &self,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let c = self.dream_ui.confirm.as_ref()?;
        let appearance = cx.global::<ActiveTheme>().theme.appearance;
        let [del, del_bg, add, add_bg] = builtin_diff_colors(appearance);
        let force = matches!(c.state, RevertState::NoteChanged);
        let note = visible(&c.note.display().to_string());
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .px(look.px(12.))
                .py(look.px(4.))
                .rounded(look.px(4.))
                .border(look.fixed(1.))
                .border_color(if primary { look.c.focus } else { look.c.border })
                .cursor_pointer()
                .hover(|h| h.bg(look.c.hover))
                .child(label)
        };
        Some(
            div()
                .id("confirm-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(look.px(96.))
                .bg(gpui::black().opacity(0.35))
                .occlude()
                .child(
                    div()
                        .id("confirm")
                        .key_context("Confirm")
                        .track_focus(&self.confirm_focus)
                        .role(Role::Dialog)
                        .aria_label(format!("Revert the change to {note}?"))
                        .flex()
                        .flex_col()
                        .gap(look.px(8.))
                        .w(look.px(600.))
                        .max_h(look.px(560.))
                        .overflow_hidden()
                        .p(look.px(16.))
                        .rounded(look.px(6.))
                        .bg(look.c.elevated)
                        .border(look.fixed(1.))
                        .border_color(look.c.border)
                        .text_size(look.text(13.))
                        .text_color(look.c.text)
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(format!("Revert the change to {note}?")),
                        )
                        .when(force, |d| {
                            d.child(div().text_color(look.c.warning).child(
                                "The note changed after this change. Its current text will be replaced by the snapshot.",
                            ))
                        })
                        .when(!c.old.is_empty(), |d| {
                            d.child(Self::diff_block("Removed", &c.new, del, del_bg, look))
                                .child(Self::diff_block("Added back", &c.old, add, add_bg, look))
                        })
                        .when(c.old.is_empty(), |d| {
                            d.child(Self::diff_block("Removed", &c.new, del, del_bg, look))
                        })
                        .child(
                            div()
                                .text_size(look.text(11.))
                                .text_color(look.c.text_muted)
                                .child("After a revert, verify treats the restored text as confirmed by a person and will not propose to change it while the note holds it."),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(look.px(8.))
                                .pt(look.px(4.))
                                .child(
                                    button("confirm-cancel", "Cancel  ↩", true).on_click(
                                        cx.listener(|this, _, window, cx| {
                                            this.cancel_revert(window, cx)
                                        }),
                                    ),
                                )
                                .child(
                                    button(
                                        "confirm-revert",
                                        if force { "Revert anyway  ⌘↩" } else { "Revert  ⌘↩" },
                                        false,
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_revert(window, cx)
                                    })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Dreaming: Open Newest Run (the palette): the newest report.
    pub(crate) fn open_newest_report(&mut self) {
        let newest = self
            .loaded
            .as_ref()
            .and_then(|l| l.panels.as_ref())
            .and_then(|p| p.dreaming.as_ref().ok())
            .and_then(|d| d.runs.first())
            .map(|r| r.report.path.clone());
        if let Some(path) = newest {
            self.state.tabs.open(Tab::Report(path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_state_has_its_control() {
        let r = Ulid::nil();
        assert_eq!(
            offer(&RevertState::Revertible),
            RevertOffer::Button {
                label: "Revert",
                force: false
            }
        );
        assert_eq!(
            offer(&RevertState::NoteChanged),
            RevertOffer::Button {
                label: "Revert anyway",
                force: true
            }
        );
        for s in [
            RevertState::NewerChange(r),
            RevertState::Reverted(r),
            RevertState::NoteMissing,
            RevertState::SnapshotMissing,
        ] {
            assert!(matches!(offer(&s), RevertOffer::Line(_)), "{s:?}");
        }
    }

    #[test]
    fn a_finished_revert_reads_with_its_exit_code() {
        let out = Ok(Output {
            stdout: "reverted notes/a.md\n".into(),
            stderr: String::new(),
            code: Some(0),
            timed_out: false,
        });
        assert_eq!(
            describe(&out),
            ("reverted notes/a.md\nexit code 0".into(), true)
        );
        let late = Ok(Output {
            stdout: String::new(),
            stderr: String::new(),
            code: None,
            timed_out: true,
        });
        assert_eq!(
            describe(&late).0,
            "hippocamp revert did not finish in 60 seconds"
        );
        assert!(!describe(&late).1);
    }

    #[test]
    fn the_confirmation_uses_the_built_in_colours() {
        let [del, _, add, _] = builtin_diff_colors(Appearance::Dark);
        assert_ne!(del, add);
    }
}

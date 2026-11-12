//! The main view: the section's own view, or the item opened from it with a
//! back link above it.

use gpui::{Context, ImageSource, ObjectFit, Window, div, img, prelude::*, transparent_black};

use super::Shell;
use super::help_tab;
use super::theme::Look;
use crate::state::Tab;

impl Shell {
    pub(super) fn render_pane(
        &mut self,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let backdrop = self.themes.backdrop().cloned();
        let active = self.state.tabs.active_tab().cloned().unwrap_or(Tab::Home);
        // These views scroll themselves.
        let own_scroll = matches!(
            active,
            Tab::Session { .. }
                | Tab::Xray(_)
                | Tab::Task { .. }
                | Tab::Note(_)
                | Tab::Memory
                | Tab::Home
                | Tab::Ask
                | Tab::Brief
                | Tab::Fleet
                | Tab::Incidents
                | Tab::Incident(_)
                | Tab::Inbox
                | Tab::Manage
        );
        // The digest counts from the last time these were seen.
        if matches!(active, Tab::Home | Tab::Inbox) {
            self.note_seen();
        }
        let content = match &active {
            Tab::Home => self.render_home_tab(look, window, cx),
            Tab::Ask => self.render_ask_tab(look, window, cx),
            Tab::Brief => self.render_brief_tab(look, window, cx),
            Tab::Inbox => self.render_lessons_inbox(look, window, cx),
            Tab::Fleet => self.render_fleet_tab(look, window, cx),
            Tab::Incidents => self.render_incidents_tab(look, window, cx),
            Tab::Incident(id) => {
                let id = id.clone();
                self.render_incident_tab(&id, look, cx)
            }
            Tab::Manage => self.render_manage_tab(look, window, cx),
            Tab::Memory => self.render_memory_tab(look, window, cx),
            Tab::Help => help_tab::render(look).into_any_element(),
            Tab::Note(path) => {
                let path = path.clone();
                self.render_note_tab(&path, look, cx)
            }
            Tab::Context => self.render_context_tab(look, cx),
            Tab::Xray(id) => {
                let id = id.clone();
                self.render_xray_tab(&id, look, window, cx)
            }
            Tab::Task { session, ord } => {
                let (session, ord) = (session.clone(), *ord);
                self.render_task_tab(&session, ord, look, window, cx)
            }
            Tab::Session { id, ordinal } => {
                let (id, ordinal) = (id.clone(), *ordinal);
                self.render_session_tab(&id, ordinal, look, cx)
            }
            Tab::Proposal(id) => {
                let id = *id;
                self.render_proposal_tab(id, look, cx)
            }
            Tab::Report(path) => {
                let path = path.clone();
                self.render_run_tab(&path, look, cx)
            }
            Tab::Attention => self.render_attention_tab(look, cx),
        };

        // The title bar's breadcrumb is the way back (Esc too);
        // there is no back bar.
        div()
            .id("pane")
            .key_context("Pane")
            .track_focus(&self.pane_focus)
            .tab_stop(true)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(look.c.centre)
            .border_2()
            .border_color(transparent_black())
            // The ring shows for keyboard focus only (GPUI's focus-visible), not
            // after a click, and never under the step-two sheet.
            .when(self.flow.sheet.is_none(), |d| {
                d.focus_visible(|s| s.border_color(look.c.focus))
            })
            .children(self.render_first_banner(look, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    // The theme's background image: above the pane's colour,
                    // below its content, scaled to cover and cropped.
                    .when_some(backdrop, |d, b| {
                        d.child(
                            img(ImageSource::Image(b.image))
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .object_fit(ObjectFit::Cover)
                                .opacity(b.opacity),
                        )
                    })
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "view-{:?}-{}",
                                self.state.tabs.section(),
                                self.state.tabs.depth()
                            )))
                            .size_full()
                            .when(!own_scroll, |d| d.overflow_y_scroll())
                            .when(own_scroll, |d| d.overflow_hidden())
                            .child(content),
                    ),
            )
    }
}

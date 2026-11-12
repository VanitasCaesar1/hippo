//! Memory's notes list: its filter at the top and every note below, beside
//! the note page. The rows come from [`crate::rows::brain`]; this module
//! draws them and moves the selection.

use gpui::{
    ClickEvent, Context, Focusable, Role, ScrollHandle, Window, div, prelude::*, px,
    transparent_black,
};

use super::Shell;
use super::theme::Look;
use crate::rows::{self, Row, Style};
use crate::state::{DOCK_DEFAULT, Section};

/// The marker dragged by the list's edge; it draws nothing.
pub struct DragDock;

pub struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Shell {
    /// The notes list's rows as they stand.
    pub(crate) fn dock_rows(&self, cx: &gpui::App) -> Vec<Row> {
        let Some(panels) = self.loaded.as_ref().and_then(|l| l.panels.as_ref()) else {
            return Vec::new();
        };
        let filter = self.brain_filter().read(cx).text().to_string();
        rows::brain(panels, &filter)
    }

    /// Scrolls the list to its selected row, as Memory opens.
    #[allow(dead_code)] // the notes list is gone from Memory
    pub(crate) fn scroll_to_selection(&self, cx: &gpui::App) {
        let rows = self.dock_rows(cx);
        if let Some(i) = self.selected_index(&rows) {
            self.reveal_row(i);
        }
    }

    /// Scrolls the list to row `i` at the next draw.
    pub(crate) fn reveal_row(&self, i: usize) {
        self.dock_reveal.set(Some((i, false)));
    }

    /// Scrolls the list the least that shows the row asked for whole, from
    /// the rows' bounds at the last draw; when the list changed shape since,
    /// GPUI scrolls to the row this frame and the next draw settles it.
    fn apply_reveal(&self, rows: &[Row], window: &Window) {
        let Some((i, retried)) = self.dock_reveal.take() else {
            return;
        };
        let h = &self.dock_scroll;
        let laid_out = !rows.is_empty()
            && h.bounds_for_item(rows.len() - 1).is_some()
            && h.bounds_for_item(rows.len()).is_none();
        let Some(item) = h.bounds_for_item(i).filter(|_| laid_out) else {
            h.scroll_to_item(i);
            if !retried {
                self.dock_reveal.set(Some((i, true)));
                window.request_animation_frame();
            }
            return;
        };
        let view = h.bounds();
        let mut offset = h.offset();
        if item.top() + offset.y < view.top() {
            offset.y = view.top() - item.top();
        } else if item.bottom() + offset.y > view.bottom() {
            offset.y = view.bottom() - item.bottom();
        }
        offset.y = offset.y.clamp(-h.max_offset().y, px(0.));
        if offset != h.offset() {
            h.set_offset(offset);
        }
    }

    /// The workspace's entry note, relative to its root.
    pub(crate) fn entry_note(&self) -> Option<std::path::PathBuf> {
        self.loaded
            .as_ref()
            .and_then(|l| l.setup.workspace())
            .map(|ws| ws.entry_note.clone())
    }

    fn selected_index(&self, rows: &[Row]) -> Option<usize> {
        let id = self.state.selections.get(Section::Memory)?;
        rows.iter().position(|r| r.key.id() == id)
    }

    /// Selects row `i` and shows its note on top of Memory.
    pub(crate) fn select_row(&mut self, rows: &[Row], i: usize, open: bool) {
        let Some(row) = rows.get(i) else { return };
        if !row.key.selectable() {
            return;
        }
        self.state.selections.select(Section::Memory, row.key.id());
        self.reveal_row(i);
        if let Some(tab) = row.key.tab() {
            if open {
                self.state.tabs.open(tab);
            } else {
                self.state.tabs.show_preview(tab);
            }
        }
    }

    /// Down or Up in the list: from the filter field, Down goes to the first
    /// row; in the list, the selection moves over the rows.
    pub(crate) fn dock_step(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.tabs.section() != Section::Memory {
            return;
        }
        let rows = self.dock_rows(cx);
        let filter = self.brain_filter().clone();
        if filter.focus_handle(cx).is_focused(window) {
            if step > 0 {
                window.focus(&self.dock_focus, cx);
                if let Some(i) = rows::step(&rows, None, 1) {
                    self.select_row(&rows, i, false);
                }
            }
            return;
        }
        let current = self.selected_index(&rows);
        if step < 0 && current == rows::step(&rows, None, 1) && current.is_some() {
            // Up from the first row goes back to the filter.
            window.focus(&filter.focus_handle(cx), cx);
            return;
        }
        if let Some(i) = rows::step(&rows, current, step) {
            self.select_row(&rows, i, false);
        }
    }

    /// Return in the list: the selected note, or the first.
    pub(crate) fn dock_open(&mut self, cx: &mut Context<Self>) {
        let rows = self.dock_rows(cx);
        match self.selected_index(&rows) {
            Some(i) => self.select_row(&rows, i, true),
            None => {
                if let Some(i) = rows::step(&rows, None, 1) {
                    self.select_row(&rows, i, false);
                }
            }
        }
    }

    /// The note Memory's own view shows: the selected one, else the entry
    /// note.
    pub(crate) fn memory_note(&self) -> Option<std::path::PathBuf> {
        self.state
            .selections
            .get(Section::Memory)
            .filter(|id| !id.starts_with("text:"))
            .map(std::path::PathBuf::from)
            .or_else(|| self.entry_note())
    }

    fn render_row(
        &self,
        i: usize,
        row: &Row,
        selected: bool,
        list_focused: bool,
        look: &Look,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let marker = if list_focused {
            look.c.focus
        } else {
            look.c.text_muted
        };
        let base = div()
            .id(("dock-row", i))
            .flex()
            .flex_col()
            .justify_center()
            .w_full()
            .min_w_0()
            .px(look.px(12.));
        match row.style {
            Style::Message { error } => base
                .py(look.px(6.))
                .text_size(look.text(12.))
                .text_color(if error {
                    look.c.error
                } else {
                    look.c.text_muted
                })
                .child(row.title.clone())
                .into_any_element(),
            _ => {
                let two = row.meta.is_some();
                base.role(Role::ListItem)
                    .aria_label(row.title.clone())
                    .aria_selected(selected)
                    .h(look.px(if two { 40. } else { 26. }))
                    .border_l(look.fixed(super::theme::RING))
                    .border_color(if selected {
                        marker
                    } else {
                        transparent_black()
                    })
                    .when(selected, |d| d.bg(look.c.selected))
                    .when(!selected, |d| d.hover(|h| h.bg(look.c.hover)))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.px(8.))
                            .min_w_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(look.c.text)
                                    .child(row.title.clone()),
                            )
                            .children(row.right.clone().map(|r| {
                                div()
                                    .flex_shrink_0()
                                    .text_size(look.text(11.))
                                    .text_color(look.c.text_muted)
                                    .child(r)
                            })),
                    )
                    .children(row.meta.clone().map(|m| {
                        div()
                            .truncate()
                            .text_size(look.text(11.))
                            .text_color(look.c.text_muted)
                            .child(m)
                    }))
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        window.focus(&this.dock_focus, cx);
                        let rows = this.dock_rows(cx);
                        this.select_row(&rows, i, e.click_count() >= 2);
                        this.after_state_change(window, cx);
                    }))
                    .into_any_element()
            }
        }
    }

    pub(super) fn render_dock(
        &self,
        look: &Look,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let width = self
            .state
            .dock
            .dragged_width
            .map(px)
            .unwrap_or_else(|| look.px(DOCK_DEFAULT));
        let rows = self.dock_rows(cx);
        let selected = self.selected_index(&rows);
        let list_focused = self.dock_focus.is_focused(window);
        let mut items = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            items.push(self.render_row(i, row, selected == Some(i), list_focused, look, cx));
        }
        self.apply_reveal(&rows, window);
        let count = self
            .loaded
            .as_ref()
            .and_then(|l| l.panels.as_ref())
            .map_or(0, |p| p.brain.rows.len());
        div()
            .id("dock")
            .key_context("Dock")
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .h_full()
            .w(width)
            .bg(look.c.dock)
            .border_r(look.fixed(1.))
            .border_color(look.c.border_variant)
            .text_size(look.text(13.))
            .text_color(look.c.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_shrink_0()
                    .px(look.px(12.))
                    .pt(look.px(10.))
                    .child(super::kit::label(look, "Notes", look.c.text_muted))
                    .child(
                        div()
                            .text_size(look.text(11.5))
                            .text_color(look.c.text_muted)
                            .child(count.to_string()),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px(look.px(8.))
                    .pt(look.px(8.))
                    .pb(look.px(6.))
                    .child(self.brain_filter().clone()),
            )
            .child(
                div()
                    .id("dock-list")
                    .role(Role::List)
                    .aria_label("Notes")
                    .track_focus(&self.dock_focus)
                    .tab_stop(true)
                    .track_scroll(&self.dock_scroll)
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .pb(look.px(8.))
                    .children(items),
            )
            .child(
                div()
                    .id("dock-edge")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(-2.))
                    .w(px(5.))
                    .cursor_col_resize()
                    .on_drag(DragDock, |_, _, _, cx| cx.new(|_| DragGhost)),
            )
    }
}

/// A scroll handle for the notes list.
pub fn scroll_handle() -> ScrollHandle {
    ScrollHandle::new()
}

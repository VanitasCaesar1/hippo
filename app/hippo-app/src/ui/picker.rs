//! The picker: a query field over a list of rows, matched fuzzily. The command
//! palette and the theme picker use it; the finder will too.

#![allow(dead_code)] // the finder and theme picker (items 8 and 14) use the rest

use gpui::{
    App, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle,
    KeyDownEvent, KeyUpEvent, Role, SharedString, StyledText, Window, div, prelude::*,
};
use hippo_view::fuzzy;

use super::input::{InputEvent, TextInput};
use super::theme::{Look, RING};
use crate::actions::{Cancel, Confirm, SelectNext, SelectPrevious};

/// How many rows the list shows at once.
pub const VISIBLE_ROWS: usize = 12;

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    /// What the query is matched against, and the row's label.
    pub text: String,
    /// Secondary text after the label.
    pub detail: Option<String>,
    /// Text at the right, such as a key binding.
    pub keys: Option<String>,
    /// When set, the row is dimmed with this reason in words and Return on it
    /// does nothing.
    pub unavailable: Option<String>,
}

impl PickerItem {
    pub fn new(text: impl Into<String>) -> PickerItem {
        PickerItem {
            text: text.into(),
            detail: None,
            keys: None,
            unavailable: None,
        }
    }

    /// What assistive technology reads for the row.
    pub fn accessible_label(&self) -> String {
        let mut label = self.text.clone();
        if let Some(detail) = &self.detail {
            label.push_str(", ");
            label.push_str(detail);
        }
        if let Some(keys) = &self.keys {
            label.push_str(", keys ");
            label.push_str(keys);
        }
        if let Some(reason) = &self.unavailable {
            label.push_str(", unavailable: ");
            label.push_str(reason);
        }
        label
    }
}

/// What a picker tells its owner. Indexes are into the items it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerEvent {
    /// The highlighted row changed (the theme picker previews on this).
    Selected(usize),
    /// Return on a row that can be used.
    Confirmed(usize),
    /// Escape, or a click outside.
    Dismissed,
}

pub struct Picker {
    items: Vec<PickerItem>,
    query: Entity<TextInput>,
    /// The rows that match, best first, with the characters to highlight.
    matches: Vec<fuzzy::Match>,
    /// An index into `matches`.
    selected: usize,
    id: SharedString,
    /// Tell the owner of a key-driven selection only after the key event that
    /// moved it has been seen, and, while an arrow key is held down and
    /// repeating, only when it is released (the theme picker's preview).
    hold_previews: bool,
    /// A selection moved by a key has not been announced yet.
    unannounced: bool,
    /// A repeat moved the selection; the announcement waits for the release.
    held_waiting: bool,
    _subscription: gpui::Subscription,
}

impl EventEmitter<PickerEvent> for Picker {}

impl Picker {
    /// `id` names the picker for accessibility ids (`palette`); `label` is
    /// what the query field is called.
    pub fn new(
        id: &'static str,
        label: &'static str,
        placeholder: &'static str,
        items: Vec<PickerItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query =
            cx.new(|cx| TextInput::new(ElementId::from((id, 0usize)), label, placeholder, cx));
        let subscription = cx.subscribe(&query, |this: &mut Picker, _, event, cx| {
            let InputEvent::Changed = event;
            this.refresh(cx);
        });
        window.focus(&query.focus_handle(cx), cx);
        let mut picker = Picker {
            items,
            query,
            matches: Vec::new(),
            selected: 0,
            id: id.into(),
            hold_previews: false,
            unannounced: false,
            held_waiting: false,
            _subscription: subscription,
        };
        picker.rematch(cx);
        picker
    }

    /// Makes `Selected` wait for a held arrow key's release: GPUI marks a
    /// repeating key's events `is_held`, and a held key must not flash the
    /// window between light and dark.
    pub fn holding_previews(mut self) -> Self {
        self.hold_previews = true;
        self
    }

    pub fn query_text(&self, cx: &App) -> String {
        self.query.read(cx).text().to_string()
    }

    pub fn set_query(&mut self, text: &str, window: &Window, cx: &mut Context<Self>) {
        self.query.update(cx, |q, cx| q.set_text(text, window, cx));
    }

    pub fn matches(&self) -> &[fuzzy::Match] {
        &self.matches
    }

    pub fn items(&self) -> &[PickerItem] {
        &self.items
    }

    /// The item index of the highlighted row.
    pub fn selected_item(&self) -> Option<usize> {
        self.matches.get(self.selected).map(|m| m.row)
    }

    /// Opens with `index` highlighted, when that row matches.
    pub fn select_item(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(pos) = self.matches.iter().position(|m| m.row == index) {
            self.selected = pos;
            cx.notify();
        }
    }

    fn rematch(&mut self, cx: &App) {
        let query = self.query.read(cx).text().to_string();
        let rows: Vec<&str> = self.items.iter().map(|i| i.text.as_str()).collect();
        self.matches = fuzzy::rank(&query, &rows);
        self.selected = 0;
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.rematch(cx);
        if let Some(row) = self.selected_item() {
            cx.emit(PickerEvent::Selected(row));
        }
        cx.notify();
    }

    fn move_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len();
        self.selected = if forward {
            (self.selected + 1) % n
        } else {
            (self.selected + n - 1) % n
        };
        if self.hold_previews {
            // The action handler runs before the key event's own listeners;
            // letting the event go on lets `key_down` see whether the key is
            // held. A selection moved by something else is announced after.
            self.unannounced = true;
            cx.propagate();
            cx.spawn(async move |this, cx| {
                this.update(cx, |this, cx| {
                    if this.unannounced {
                        this.unannounced = false;
                        this.announce(cx);
                    }
                })
                .ok();
            })
            .detach();
        } else {
            self.announce(cx);
        }
        cx.notify();
    }

    fn announce(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_item() {
            cx.emit(PickerEvent::Selected(row));
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.unannounced {
            return;
        }
        self.unannounced = false;
        if event.is_held {
            self.held_waiting = true;
        } else {
            self.announce(cx);
        }
    }

    fn key_up(&mut self, cx: &mut Context<Self>) {
        if self.held_waiting {
            self.held_waiting = false;
            self.announce(cx);
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(true, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(false, cx);
    }

    /// Return: runs the highlighted row unless it is dimmed.
    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_item() {
            self.confirm_item(row, cx);
        }
    }

    fn confirm_item(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.items[row].unavailable.is_none() {
            cx.emit(PickerEvent::Confirmed(row));
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PickerEvent::Dismissed);
    }

    /// The first visible row: the window of rows scrolls to keep the
    /// highlighted one in view.
    fn first_visible(&self) -> usize {
        (self.selected + 1).saturating_sub(VISIBLE_ROWS)
    }
}

impl Focusable for Picker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.query.focus_handle(cx)
    }
}

impl Render for Picker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::from_app(cx);
        let first = self.first_visible();
        let shown: Vec<(usize, fuzzy::Match)> = self
            .matches
            .iter()
            .cloned()
            .enumerate()
            .skip(first)
            .take(VISIBLE_ROWS)
            .collect();
        let empty = self.matches.is_empty();
        let active_row = self.selected_item();

        let rows = shown.into_iter().map(|(pos, m)| {
            let item = &self.items[m.row];
            let selected = pos == self.selected;
            let dimmed = item.unavailable.is_some();
            let highlights = byte_ranges(&item.text, &m.positions).into_iter().map(|r| {
                (
                    r,
                    HighlightStyle {
                        background_color: Some(look.c.search_match),
                        ..Default::default()
                    },
                )
            });
            let row_index = m.row;
            let label_color = if dimmed {
                look.c.text_muted
            } else {
                look.c.text
            };
            div()
                .id(ElementId::from((self.id.clone(), pos + 1)))
                .role(Role::ListBoxOption)
                .aria_label(item.accessible_label())
                .aria_selected(selected)
                .when(Some(row_index) == active_row, |d| {
                    d.aria_active_descendant()
                })
                .flex()
                .items_center()
                .gap(look.px(8.))
                .h(look.px(28.))
                .px(look.px(8.))
                .text_size(look.text(13.))
                .text_color(label_color)
                .when(selected, |d| {
                    d.bg(look.c.selected)
                        .border_l(look.fixed(RING))
                        .border_color(look.c.selected_outline)
                })
                .when(!selected, |d| d.hover(|h| h.bg(look.c.hover)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.confirm_item(row_index, cx)))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(StyledText::new(item.text.clone()).with_highlights(highlights)),
                )
                .when_some(item.detail.clone(), |d, detail| {
                    d.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(look.c.text_muted)
                            .child(detail),
                    )
                })
                .when_some(item.unavailable.clone(), |d, reason| {
                    d.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(look.c.text_muted)
                            .child(format!("({reason})")),
                    )
                })
                .child(div().flex_1())
                .when_some(item.keys.clone(), |d, keys| {
                    d.child(div().text_color(look.c.text_muted).child(keys))
                })
        });

        div()
            .id(ElementId::from((self.id.clone(), usize::MAX)))
            .role(Role::Dialog)
            .aria_label("Picker")
            .key_context("Picker")
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.key_down(event, cx)))
            .on_key_up(cx.listener(|this, _: &KeyUpEvent, _, cx| this.key_up(cx)))
            .occlude()
            .flex()
            .flex_col()
            .w(look.px(560.))
            .bg(look.c.elevated)
            .border(look.fixed(1.))
            .border_color(look.c.border)
            .rounded(look.px(6.))
            .shadow_lg()
            .overflow_hidden()
            .child(div().p(look.px(8.)).child(self.query.clone()))
            .child(
                div()
                    .id(ElementId::from((self.id.clone(), usize::MAX - 1)))
                    .role(Role::ListBox)
                    .aria_label("Results")
                    .flex()
                    .flex_col()
                    .border_t(look.fixed(1.))
                    .border_color(look.c.border_variant)
                    .children(rows)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .h(look.px(28.))
                                .px(look.px(8.))
                                .flex()
                                .items_center()
                                .text_size(look.text(13.))
                                .text_color(look.c.text_muted)
                                .child("Nothing matches."),
                        )
                    }),
            )
    }
}

/// Byte ranges of the matched characters, merging neighbours.
pub fn byte_ranges(text: &str, positions: &[usize]) -> Vec<std::ops::Range<usize>> {
    let offsets: Vec<(usize, usize)> = text
        .char_indices()
        .map(|(i, c)| (i, i + c.len_utf8()))
        .collect();
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for &p in positions {
        let Some(&(start, end)) = offsets.get(p) else {
            continue;
        };
        match out.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => out.push(start..end),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn matched_characters_become_merged_byte_ranges() {
        assert_eq!(byte_ranges("View: Reload", &[0, 1, 6]), [0..2, 6..7]);
        assert_eq!(byte_ranges("héllo", &[1, 2]), [1..4]);
        assert_eq!(
            byte_ranges("abc", &[]),
            Vec::<std::ops::Range<usize>>::new()
        );
        assert_eq!(
            byte_ranges("abc", &[9]),
            Vec::<std::ops::Range<usize>>::new()
        );
    }

    #[test]
    fn a_rows_accessible_label_has_its_keys_and_its_reason() {
        let mut item = PickerItem::new("Hippo: Quit");
        item.keys = Some("⌘Q".into());
        assert_eq!(item.accessible_label(), "Hippo: Quit, keys ⌘Q");
        item.unavailable = Some("no tab is open".into());
        assert!(
            item.accessible_label()
                .ends_with("unavailable: no tab is open")
        );
    }
}
